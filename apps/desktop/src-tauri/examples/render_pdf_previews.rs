//! Synthetic vector PDFs -> production CG raster -> production drag image.
//! No desktop window, clipboard, user files, cloud or accounts.
#[allow(unsafe_code, dead_code)]
#[path = "../src/drag_preview.rs"]
mod drag_preview;
#[path = "support/pdf_fixture.rs"]
mod fixture;
#[allow(unsafe_code)]
#[path = "../src/pdf_preview.rs"]
mod pdf_preview;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use chrono::Utc;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSGraphicsContext, NSImage};
    use objc2_foundation::NSData;
    use paste_domain::{
        ClipId, ClipItem, ContentKind, DeviceId, DeviceMetadata, SourceApplication,
    };
    let _app = NSApplication::sharedApplication(MainThreadMarker::new().ok_or("main thread")?);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let pdfs = root.join("tmp/pdfs");
    let output = root.join("target/pdf-preview-verification");
    std::fs::create_dir_all(&pdfs)?;
    std::fs::create_dir_all(&output)?;
    let mut cases = Vec::new();
    for (name, rotation, crop) in [
        ("portrait", 0, false),
        ("rotated", 90, false),
        ("cropped", 0, true),
    ] {
        let pdf = fixture::document(rotation, crop);
        assert_eq!(pdf_preview::inspect(&pdf)?, 2);
        std::fs::write(pdfs.join(format!("synthetic-{name}.pdf")), &pdf)?;
        let raster = pdf_preview::render(&pdf, 600, 600)?;
        std::fs::write(output.join(format!("{name}.png")), &raster.bytes)?;
        let item = ClipItem {
            id: ClipId::new(),
            captured_at: Utc::now(),
            last_copied_at: Utc::now(),
            source: SourceApplication {
                bundle_identifier: "io.pasters.synthetic".into(),
                display_name: "Synthetic".into(),
            },
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Synthetic Mac".into(),
            },
            content_kind: ContentKind::Pdf,
            title: format!("{name}.pdf"),
            searchable_text: "Synthetic two-page PDF".into(),
            content_hash: [0; 32],
            representations: vec![],
        };
        for dark in [false, true] {
            let mut drag = drag_preview::DragPreview::new(&item, 1);
            drag.set_thumbnail(raster.bytes.clone())?;
            let before =
                NSGraphicsContext::currentContext().map(|ctx| objc2::rc::Retained::as_ptr(&ctx));
            let bytes = drag_preview::render(&drag, dark)?;
            assert_eq!(
                before,
                NSGraphicsContext::currentContext().map(|ctx| objc2::rc::Retained::as_ptr(&ctx))
            );
            let image =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Tiff)?.to_rgba8();
            assert_eq!(image.dimensions(), (392, 312));
            assert_eq!(image.get_pixel(0, 0)[3], 0);
            // Count pixels within content region, not the colored card header.
            let body = image::imageops::crop_imm(&image, 32, 138, 328, 104).to_image();
            assert!(
                body.pixels()
                    .filter(|p| p[0] > 230 && p[1] < 20 && p[2] < 20)
                    .count()
                    > 100
            );
            assert!(
                body.pixels()
                    .filter(|p| p[2] > 230 && p[0] < 20 && p[1] < 20)
                    .count()
                    > 100
            );
            let native = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))
                .ok_or("drag image")?;
            assert_eq!((native.size().width, native.size().height), (196.0, 156.0));
            image.save(output.join(format!(
                "drag-{name}-{}.png",
                if dark { "dark" } else { "light" }
            )))?;
        }
        cases.push(serde_json::json!({"name":name, "pixelSize":[raster.pixel_width,raster.pixel_height], "pdfBlake3":blake3::hash(&pdf).to_hex().to_string()}));
    }
    let acceptance_pdf = include_bytes!("../fixtures/native-preview-acceptance.pdf");
    assert_eq!(pdf_preview::inspect(acceptance_pdf)?, 3);
    let acceptance_raster = pdf_preview::render(acceptance_pdf, 900, 900)?;
    assert_eq!(
        (
            acceptance_raster.pixel_width,
            acceptance_raster.pixel_height
        ),
        (675, 900)
    );
    std::fs::write(
        output.join("native-acceptance.png"),
        &acceptance_raster.bytes,
    )?;
    let report = serde_json::json!({"result":"passed", "nativeGesture":false, "rendererBlake3":blake3::hash(include_bytes!("../src/pdf_preview.rs")).to_hex().to_string(), "cases":cases, "nativeAcceptance":{"pages":3,"pixelSize":[675,900],"pdfBlake3":blake3::hash(acceptance_pdf).to_hex().to_string()}, "dragChecks":["six light/dark images", "actual PDF content", "transparent corners", "Retina point size", "graphics context restored"]});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}
