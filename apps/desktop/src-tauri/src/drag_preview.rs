//! A bounded content preview for the native drag image, not the app's logo.
use paste_domain::{ClipItem, ContentKind};

pub struct DragPreview {
    title: String,
    text: String,
    source: String,
    kind: &'static str,
    count: usize,
    content_kind: ContentKind,
    thumbnail: Option<paste_platform::GeneratedThumbnail>,
    source_icon: Option<paste_platform::GeneratedThumbnail>,
}

#[allow(dead_code)]
#[path = "../../src/card_visual.rs"]
mod card_visual;

fn bounded_line(value: &str, limit: usize) -> String {
    let mut chars = value.chars().map(|c| if c.is_control() { ' ' } else { c });
    let mut text: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        text.push('…');
    }
    text
}

impl DragPreview {
    pub fn new(item: &ClipItem, count: usize) -> Self {
        Self {
            title: bounded_line(&item.title, 30),
            text: bounded_line(&item.searchable_text, 160),
            source: bounded_line(&item.source.display_name, 25),
            kind: match item.content_kind {
                ContentKind::Text => "Text",
                ContentKind::RichText | ContentKind::Html => "Rich Text",
                ContentKind::Link => "Link",
                ContentKind::Image => "Image",
                ContentKind::File => "File",
                ContentKind::Pdf => "PDF",
                ContentKind::Color => "Color",
                ContentKind::Unknown => "Content",
            },
            count,
            content_kind: item.content_kind,
            thumbnail: None,
            source_icon: None,
        }
    }

    /// Decode cached PNGs using the bounded Rust decoder, not AppKit's parser.
    pub fn set_thumbnail(&mut self, png: Vec<u8>) -> Result<(), paste_platform::PreviewError> {
        use paste_domain::{CapturedRepresentation, RepresentationKind};
        self.thumbnail = Some(paste_platform::generate_image_thumbnail(
            &[CapturedRepresentation {
                kind: RepresentationKind::Png,
                native_type: None,
                mime_type: Some("image/png".into()),
                file_name: None,
                bytes: png,
            }],
            328,
            104,
        )?);
        Ok(())
    }

    pub fn set_source_icon(&mut self, png: Vec<u8>) -> Result<(), paste_platform::PreviewError> {
        use paste_domain::{CapturedRepresentation, RepresentationKind};
        self.source_icon = Some(paste_platform::generate_image_thumbnail(
            &[CapturedRepresentation {
                kind: RepresentationKind::Png,
                native_type: None,
                mime_type: None,
                file_name: None,
                bytes: png,
            }],
            32,
            32,
        )?);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn render(preview: &DragPreview, dark: bool) -> Result<Vec<u8>, String> {
    use objc2::{AnyThread, MainThreadMarker, runtime::AnyObject};
    use objc2_app_kit::{
        NSAttributedStringNSStringDrawing, NSBezierPath, NSBitmapImageRep, NSColor,
        NSCompositingOperation, NSDeviceRGBColorSpace, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSLineBreakMode,
        NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    };
    use objc2_core_graphics::CGContext;
    use objc2_foundation::{
        NSAttributedString, NSData, NSDictionary, NSPoint, NSRect, NSSize, NSString,
    };
    MainThreadMarker::new().ok_or("拖动预览必须在主线程绘制。")?;
    let rect = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let color = |r, g, b, a| NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
    let text = |value: &str, bounds: NSRect, size: f64, bold: bool, ink: &NSColor| {
        let font = if bold {
            NSFont::boldSystemFontOfSize(size)
        } else {
            NSFont::systemFontOfSize(size)
        };
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setLineBreakMode(if bounds.size.height <= 28.0 {
            NSLineBreakMode::ByTruncatingTail
        } else {
            NSLineBreakMode::ByWordWrapping
        });
        let attrs = unsafe {
            NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[font.as_ref(), ink.as_ref(), paragraph.as_ref()],
            )
        };
        let string = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(value),
                Some(&attrs),
            )
        };
        string.drawInRect(bounds);
    };
    // Explicit 2x backing with point-size metadata. Image consumers must see a
    // 196×156 point image, not a blurry 1x image or a double-sized 392pt card.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), 392, 312, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
        )
    }.ok_or("无法分配拖动预览位图。")?;
    bitmap.setSize(NSSize::new(196.0, 156.0));
    let bitmap_context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .ok_or("无法创建拖动预览画布。")?;
    let cg = bitmap_context.CGContext();
    // NSGraphicsContext already derives the 2x transform from bitmap.size.
    // Only flip point-space vertically; another 2x scale would crop the card.
    CGContext::translate_ctm(Some(&cg), 0.0, 156.0);
    CGContext::scale_ctm(Some(&cg), 1.0, -1.0);
    let context = NSGraphicsContext::graphicsContextWithCGContext_flipped(&cg, true);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    NSGraphicsContext::saveGraphicsState_class();
    // Transparent padding and rounded corners; no opaque square logo canvas.
    let card = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect(5.0, 5.0, 186.0, 146.0),
        8.0,
        8.0,
    );
    if dark {
        color(0.15, 0.15, 0.16, 0.97)
    } else {
        color(1.0, 1.0, 1.0, 0.97)
    }
    .setFill();
    card.fill();
    card.addClip();
    let (header, header_ink) = match preview.content_kind {
        ContentKind::Link => ([35, 115, 195], [255, 255, 255]),
        ContentKind::Image => ([201, 54, 64], [255, 255, 255]),
        ContentKind::File | ContentKind::Pdf => ([89, 107, 129], [255, 255, 255]),
        ContentKind::Color => ([121, 80, 167], [255, 255, 255]),
        _ => ([237, 194, 77], [70, 52, 12]),
    };
    let rgb = |value: [u8; 3]| {
        color(
            f64::from(value[0]) / 255.0,
            f64::from(value[1]) / 255.0,
            f64::from(value[2]) / 255.0,
            1.0,
        )
    };
    rgb(header).setFill();
    // Use the same small type tag as the timeline, leaving content dominant.
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect(16.0, 13.0, preview.kind.len() as f64 * 6.0 + 12.0, 18.0),
        4.0,
        4.0,
    )
    .fill();
    text(
        preview.kind,
        rect(22.0, 15.0, 126.0, 15.0),
        9.0,
        true,
        &rgb(header_ink),
    );
    let ink = if dark {
        color(0.97, 0.97, 0.98, 1.0)
    } else {
        color(0.12, 0.12, 0.14, 1.0)
    };
    text(
        &preview.title,
        rect(16.0, 42.0, 164.0, 19.0),
        13.0,
        true,
        &ink,
    );
    if let Some((hex, foreground)) = card_visual::color_swatch(preview.content_kind, &preview.text)
    {
        let channel = |i| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)) / 255.0;
        color(channel(1), channel(3), channel(5), 1.0).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(16.0, 69.0, 164.0, 52.0),
            6.0,
            6.0,
        )
        .fill();
        let swatch_ink = if foreground == "#000000" {
            NSColor::blackColor()
        } else {
            NSColor::whiteColor()
        };
        text(&hex, rect(28.0, 83.0, 140.0, 28.0), 17.0, true, &swatch_ink);
    } else if let Some(thumbnail) = preview.thumbnail.as_ref()
        && let Some(image) =
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&thumbnail.bytes))
    {
        let width = f64::from(thumbnail.pixel_width) / 2.0;
        let height = f64::from(thumbnail.pixel_height) / 2.0;
        // Aspect fit: no crop, upscaling, or stretching.
        unsafe {
            image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                rect(
                    16.0 + (164.0 - width) / 2.0,
                    69.0 + (52.0 - height) / 2.0,
                    width,
                    height,
                ),
                rect(0.0, 0.0, image.size().width, image.size().height),
                NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            );
        }
    } else {
        text(
            &preview.text,
            rect(16.0, 69.0, 164.0, 52.0),
            11.0,
            false,
            &ink,
        );
    }
    let secondary = if dark {
        color(0.75, 0.75, 0.77, 1.0)
    } else {
        color(0.39, 0.39, 0.42, 1.0)
    };
    let source_x = if let Some(icon) = preview.source_icon.as_ref()
        && let Some(image) =
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&icon.bytes))
    {
        let width = f64::from(icon.pixel_width) / 2.0;
        let height = f64::from(icon.pixel_height) / 2.0;
        unsafe {
            image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                rect(
                    14.0 + (16.0 - width) / 2.0,
                    128.0 + (16.0 - height) / 2.0,
                    width,
                    height,
                ),
                rect(0.0, 0.0, image.size().width, image.size().height),
                NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            );
        }
        35.0
    } else {
        16.0
    };
    text(
        &preview.source,
        rect(source_x, 130.0, 180.0 - source_x, 14.0),
        9.0,
        false,
        &secondary,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    if preview.count > 1 {
        color(0.08, 0.43, 0.88, 1.0).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(154.0, 0.0, 40.0, 23.0),
            11.5,
            11.5,
        )
        .fill();
        let count = if preview.count > 99 {
            "99+".to_owned()
        } else {
            preview.count.to_string()
        };
        text(
            &count,
            rect(163.0, 4.0, 28.0, 17.0),
            11.0,
            true,
            &NSColor::whiteColor(),
        );
    }
    NSGraphicsContext::restoreGraphicsState_class();
    bitmap
        .TIFFRepresentation()
        .map(|data| data.to_vec())
        .ok_or("无法生成拖动内容预览。".into())
}

#[cfg(not(target_os = "macos"))]
pub fn render(_preview: &DragPreview, _dark: bool) -> Result<Vec<u8>, String> {
    Err("当前平台尚未实现原生内容拖动预览。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_text_is_bounded_on_unicode_boundaries_and_has_no_control_characters() {
        assert_eq!(bounded_line("中🦀文\n\t", 3), "中🦀文…");
        assert_eq!(bounded_line("a\nb\t", 9), "a b ");
        assert_eq!(bounded_line(&"🦀".repeat(1000), 160).chars().count(), 161);
    }
}
