//! Local PDF validation and first-page raster. Original bytes are never changed.
use paste_platform::GeneratedThumbnail;

pub const MAX_PDF_BYTES: usize = 12 * 1024 * 1024;
const MAX_OUTPUT_SIDE: u32 = 1_200;
#[cfg(test)]
#[path = "../examples/support/pdf_fixture.rs"]
pub(crate) mod fixture;

/// Validate a full document before handing it to the embedded viewer. Parsing
/// through CoreGraphics does not execute document actions, request a password,
/// render all pages or rewrite the original PDF.
#[cfg(target_os = "macos")]
pub fn inspect(bytes: &[u8]) -> Result<usize, String> {
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{CGDataProvider, CGPDFDocument};
    validate_bytes(bytes)?;
    let data = CFData::from_bytes(bytes);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("无法读取 PDF 数据。")?;
    let document = CGPDFDocument::with_provider(Some(&provider)).ok_or("PDF 数据无效。")?;
    if !CGPDFDocument::is_unlocked(Some(&document)) {
        return Err("PDF 已加密，需要解锁后才能预览；原始文档保持不变。".into());
    }
    let pages = CGPDFDocument::number_of_pages(Some(&document));
    if pages == 0 || CGPDFDocument::page(Some(&document), 1).is_none() {
        return Err("PDF 没有可预览的页面。".into());
    }
    Ok(pages)
}

#[cfg(not(target_os = "macos"))]
pub fn inspect(_bytes: &[u8]) -> Result<usize, String> {
    Err("此平台尚不支持 PDF 文档预览。".into())
}

#[cfg(target_os = "macos")]
pub fn render(bytes: &[u8], max_width: u32, max_height: u32) -> Result<GeneratedThumbnail, String> {
    use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImageAlphaInfo,
        CGImageByteOrderInfo, CGPDFBox, CGPDFDocument, CGPDFPage,
    };
    use std::io::Cursor;
    validate_input(bytes, max_width, max_height)?;
    // Owned CFData: no file URL, external resource callback, password prompt,
    // JavaScript interpreter, NSApplication or user clipboard is involved.
    let data = CFData::from_bytes(bytes);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("无法读取 PDF 数据。")?;
    let document = CGPDFDocument::with_provider(Some(&provider)).ok_or("PDF 数据无效。")?;
    if !CGPDFDocument::is_unlocked(Some(&document)) {
        return Err("PDF 已加密，无法生成首页缩略图。".into());
    }
    let page = CGPDFDocument::page(Some(&document), 1).ok_or("PDF 没有可预览的页面。")?;
    let bounds = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
    let (width, height) = raster_size(
        bounds.size.width,
        bounds.size.height,
        CGPDFPage::rotation_angle(Some(&page)),
        max_width,
        max_height,
    )?;
    if !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || bounds.origin.x.abs() > 1_000_000.0
        || bounds.origin.y.abs() > 1_000_000.0
    {
        return Err("PDF 页面坐标无效。".into());
    }
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    let color_space = CGColorSpace::new_device_rgb().ok_or("无法创建 PDF 色彩空间。")?;
    // SAFETY: tightly packed RGBA storage is bounded above and lives until
    // after context destruction. No references to it exist while CG draws.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            Some(&color_space),
            CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .ok_or("无法创建 PDF 缩略图画布。")?;
    let target = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64, height as f64),
    );
    // PDF paper is opaque white in both themes. Rotation and non-zero CropBox
    // origins are handled by CG, not by guessing an unrotated media box.
    CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
    CGContext::fill_rect(Some(&context), target);
    // CGPDFPage's fit transform may leave small pages at 1x. Apply the desired
    // backing scale explicitly, then ask it only for page rotation/origin fit.
    let (natural_width, natural_height) =
        if CGPDFPage::rotation_angle(Some(&page)).rem_euclid(180) == 90 {
            (bounds.size.height, bounds.size.width)
        } else {
            (bounds.size.width, bounds.size.height)
        };
    let scale = (width as f64 / natural_width).min(height as f64 / natural_height);
    CGContext::scale_ctm(Some(&context), scale, scale);
    let point_target = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64 / scale, height as f64 / scale),
    );
    let transform =
        CGPDFPage::drawing_transform(Some(&page), CGPDFBox::CropBox, point_target, 0, true);
    if ![
        transform.a,
        transform.b,
        transform.c,
        transform.d,
        transform.tx,
        transform.ty,
    ]
    .into_iter()
    .all(f64::is_finite)
    {
        return Err("PDF 页面变换无效。".into());
    }
    CGContext::concat_ctm(Some(&context), transform);
    CGContext::draw_pdf_page(Some(&context), Some(&page));
    drop(context);
    let image = image::RgbaImage::from_raw(width, height, pixels).ok_or("PDF 位图尺寸无效。")?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(GeneratedThumbnail {
        media_type: "image/png".into(),
        bytes: png.into_inner(),
        pixel_width: width,
        pixel_height: height,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn render(
    _bytes: &[u8],
    _max_width: u32,
    _max_height: u32,
) -> Result<GeneratedThumbnail, String> {
    Err("此平台尚不支持 PDF 首页缩略图。".into())
}

fn validate_input(bytes: &[u8], width: u32, height: u32) -> Result<(), String> {
    validate_bytes(bytes)?;
    if !(1..=MAX_OUTPUT_SIDE).contains(&width) || !(1..=MAX_OUTPUT_SIDE).contains(&height) {
        return Err("PDF 缩略图尺寸超出限制。".into());
    }
    Ok(())
}

fn validate_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err("PDF 超过 12 MiB 预览限制。".into());
    }
    // PDF headers may follow a small leading prefix (ISO-compatible readers).
    if !bytes[..bytes.len().min(1_024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        return Err("PDF 文件头无效。".into());
    }
    Ok(())
}

fn raster_size(
    width: f64,
    height: f64,
    rotation: i32,
    max_width: u32,
    max_height: u32,
) -> Result<(u32, u32), String> {
    if ![width, height]
        .into_iter()
        .all(|n| n.is_finite() && (0.01..=1_000_000.0).contains(&n))
        || rotation.rem_euclid(90) != 0
    {
        return Err("PDF 页面尺寸或旋转无效。".into());
    }
    let (width, height) = if rotation.rem_euclid(180) == 90 {
        (height, width)
    } else {
        (width, height)
    };
    let scale = (f64::from(max_width) / width).min(f64::from(max_height) / height);
    Ok((
        (width * scale).round().clamp(1.0, f64::from(max_width)) as u32,
        (height * scale).round().clamp(1.0, f64::from(max_height)) as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_document_validation_preserves_all_pages_and_rejects_unreadable_input() {
        let valid = include_bytes!("../fixtures/native-preview-acceptance.pdf");
        let original = valid.to_vec();
        assert_eq!(inspect(valid).expect("three real pages"), 3);
        assert_eq!(*valid, original.as_slice());
        assert!(inspect(b"%PDF-1.4\ninvalid").is_err());
        assert!(inspect(&vec![0; MAX_PDF_BYTES + 1]).is_err());
        let locked = include_bytes!("../fixtures/native-preview-locked.pdf");
        assert!(inspect(locked).expect_err("locked PDF").contains("已加密"));
        assert!(render(locked, 320, 240).is_err());
        let empty = String::from_utf8(fixture::document(0, false))
            .expect("ASCII fixture")
            .replace("/Kids [3 0 R 6 0 R] /Count 2", "/Kids [] /Count 0");
        assert!(inspect(empty.as_bytes()).is_err());
    }

    #[test]
    fn input_limits_and_non_finite_geometry_are_rejected() {
        assert!(validate_input(b"no PDF", 320, 240).is_err());
        assert!(validate_input(b"%PDF-1.4", 0, 240).is_err());
        assert!(validate_input(b"%PDF-1.4", u32::MAX, 240).is_err());
        assert!(validate_input(&vec![0; MAX_PDF_BYTES + 1], 320, 240).is_err());
        for n in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1_000_001.0] {
            assert!(raster_size(n, 100.0, 0, 320, 240).is_err());
        }
        assert!(raster_size(100.0, 200.0, 45, 320, 240).is_err());
    }

    #[test]
    fn bounded_sizes_follow_page_rotation() {
        assert_eq!(
            raster_size(200.0, 300.0, 0, 400, 400).expect("portrait"),
            (267, 400)
        );
        assert_eq!(
            raster_size(200.0, 300.0, 90, 400, 400).expect("landscape"),
            (400, 267)
        );
        assert_eq!(
            raster_size(200.0, 300.0, -90, 400, 400).expect("negative rotation"),
            (400, 267)
        );
    }

    #[test]
    fn real_pdf_first_page_rotation_crop_and_original_bytes() {
        for (name, rotation, crop, dimensions) in [
            ("portrait", 0, false, (200, 300)),
            ("rotated", 90, false, (300, 200)),
            ("cropped", 0, true, (200, 300)),
        ] {
            let pdf = fixture::document(rotation, crop);
            let original = pdf.clone();
            let thumbnail = render(&pdf, 300, 300).expect(name);
            assert_eq!((thumbnail.pixel_width, thumbnail.pixel_height), dimensions);
            let image = image::load_from_memory(&thumbnail.bytes)
                .expect("generated PNG")
                .to_rgba8();
            // First page has red at its top and blue at its bottom, not the
            // green second page. These assert orientation and RGBA byte order.
            let (red, blue) = if rotation == 90 {
                (image.get_pixel(270, 100), image.get_pixel(30, 100))
            } else {
                (image.get_pixel(100, 30), image.get_pixel(100, 270))
            };
            assert!(
                red[0] > 240 && red[1] < 10 && red[2] < 10,
                "{name} red {red:?}"
            );
            assert!(
                blue[2] > 240 && blue[0] < 10 && blue[1] < 10,
                "{name} blue {blue:?}"
            );
            assert!(image.pixels().all(|p| p[3] == 255));
            assert_eq!(pdf, original);
        }
    }

    #[test]
    fn malformed_pdf_does_not_generate_a_false_thumbnail() {
        assert!(render(b"%PDF-1.4\ninvalid", 320, 240).is_err());
    }

    #[test]
    fn retina_raster_scales_content_not_just_the_surrounding_canvas() {
        for (rotation, cropped) in [(0, false), (90, false), (0, true)] {
            let thumbnail =
                render(&fixture::document(rotation, cropped), 600, 600).expect("2x raster");
            let image = image::load_from_memory(&thumbnail.bytes)
                .expect("generated PNG")
                .to_rgba8();
            let pixel = if rotation == 90 {
                image.get_pixel(580, 20)
            } else {
                image.get_pixel(20, 20)
            };
            assert!(
                pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10,
                "page must fill the 2x raster: {pixel:?}"
            );
        }
    }
}
