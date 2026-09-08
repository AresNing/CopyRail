//! Offline renderer check: never runs desktop services or creates a window.
//! It shares the production AppKit drawing module, using synthetic metadata.
#[allow(unsafe_code)]
#[path = "../src/drag_preview.rs"]
mod drag_preview;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use chrono::Utc;
    use image::{GenericImageView, ImageFormat};
    use objc2::AnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSGraphicsContext, NSImage};
    use objc2_foundation::NSData;
    use paste_domain::{
        ClipId, ClipItem, ContentKind, DeviceId, DeviceMetadata, SourceApplication,
    };
    let mtm = MainThreadMarker::new().ok_or("main thread required")?;
    let _app = NSApplication::sharedApplication(mtm);
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/native-preview-verification");
    std::fs::create_dir_all(&output)?;
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
        content_kind: ContentKind::Text,
        title: "合成便签 A 🦀".into(),
        searchable_text: "这是原生内容缩略卡。\nRust / AppKit 渲染，不来自真实剪贴板。".into(),
        content_hash: [0; 32],
        representations: vec![],
    };
    let mut rows = Vec::new();
    let cases = [
        (
            "text",
            ContentKind::Text,
            "合成便签 A 🦀",
            item.searchable_text.as_str(),
            1,
        ),
        (
            "multi",
            ContentKind::Text,
            "合成多选",
            item.searchable_text.as_str(),
            12,
        ),
        (
            "many",
            ContentKind::Text,
            "合成多选",
            item.searchable_text.as_str(),
            120,
        ),
        (
            "long",
            ContentKind::Text,
            "长标题 🦀 中英混排abcdefghijklmnopqrstuvwxyz",
            "<img src='never-load'> 不执行 HTML。مرحبا بالعالم שלום עולם",
            1,
        ),
        (
            "link",
            ContentKind::Link,
            "Synthetic reference",
            "https://example.invalid/long/path?fixture=true",
            1,
        ),
        (
            "image",
            ContentKind::Image,
            "合成渐变图片",
            "不读取用户照片",
            1,
        ),
        (
            "invalid-image",
            ContentKind::Image,
            "损坏缓存的回退",
            "保留可读元数据，不影响原始导出",
            1,
        ),
        (
            "source-icon",
            ContentKind::Text,
            "Synthetic icon",
            "Only fixture pixels, no installed app lookup.",
            1,
        ),
        (
            "color-light",
            ContentKind::Color,
            "Sage green",
            "#58ad97",
            1,
        ),
        ("color-dark", ContentKind::Color, "Midnight", "#17243b", 1),
        ("color-bare", ContentKind::Color, "Copied hex", "58aD97", 1),
        (
            "numeric-text",
            ContentKind::Text,
            "Number stays text",
            "235442",
            1,
        ),
        (
            "pdf",
            ContentKind::Pdf,
            "Synthetic.pdf",
            "PDF 暂用文字摘要；未加载原始文件。",
            1,
        ),
    ];
    let pixels = image::RgbaImage::from_fn(320, 180, |_x, y| {
        if y < 90 {
            image::Rgba([230, 80, 50, 255])
        } else {
            image::Rgba([30, 110, 225, 255])
        }
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut encoded, ImageFormat::Png)?;
    for dark in [false, true] {
        for (case, kind, title, body, count) in cases {
            let mut fixture = item.clone();
            fixture.content_kind = kind;
            fixture.title = title.into();
            fixture.searchable_text = body.into();
            let mut preview = drag_preview::DragPreview::new(&fixture, count);
            if case == "image" {
                preview.set_thumbnail(encoded.get_ref().clone())?;
            }
            if case == "source-icon" {
                preview.set_source_icon(encoded.get_ref().clone())?;
            }
            if case == "invalid-image" {
                assert!(preview.set_thumbnail(vec![0, 1, 2, 3]).is_err());
            }
            let before = NSGraphicsContext::currentContext()
                .map(|context| objc2::rc::Retained::as_ptr(&context));
            let bytes = drag_preview::render(&preview, dark)?;
            let after = NSGraphicsContext::currentContext()
                .map(|context| objc2::rc::Retained::as_ptr(&context));
            assert_eq!(
                before, after,
                "renderer must restore the caller's graphics context"
            );
            let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Tiff)?;
            let (width, height) = decoded.dimensions();
            assert_eq!((width, height), (392, 312), "Retina backing required");
            let native_image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))
                .ok_or("invalid native image")?;
            assert_eq!(
                (native_image.size().width, native_image.size().height),
                (196.0, 156.0),
                "native drag point size must not double"
            );
            assert_eq!(
                decoded.get_pixel(0, 0).0[3],
                0,
                "outer corner must be transparent"
            );
            // Region checks catch wrong Retina/flip transforms, not just sizes.
            assert_eq!(
                decoded.get_pixel(36, 40).0[3],
                255,
                "type tag must be at top, not cropped"
            );
            let body_color = decoded.get_pixel(370, 260).0;
            assert!(
                body_color[3] >= 240
                    && if dark {
                        body_color[0] < 70
                    } else {
                        body_color[0] > 240
                    },
                "body bounds/theme"
            );
            assert_eq!(
                decoded.get_pixel(390, 310).0[3],
                0,
                "bottom padding must remain clear"
            );
            if case == "image" {
                let top = decoded.get_pixel(196, 155).0;
                let bottom = decoded.get_pixel(196, 225).0;
                assert!(
                    top[0] > top[2] && bottom[2] > bottom[0],
                    "image content must not flip vertically"
                );
            }
            if case.starts_with("color-") {
                let swatch = decoded.get_pixel(345, 210).0;
                if case == "color-light" || case == "color-bare" {
                    assert!(swatch[1] > swatch[0] && swatch[1] > swatch[2]);
                } else {
                    assert!(swatch[2] > swatch[0] && swatch[2] < 100);
                }
            }
            let name = format!(
                "drag-{case}-{}-{count}.png",
                if dark { "dark" } else { "light" }
            );
            decoded.save(output.join(&name))?;
            rows.push(serde_json::json!({"file":name,"width":width,"height":height,"count":count,"dark":dark}));
        }
    }
    let report = serde_json::json!({
        "result":"passed", "renderer":"production AppKit module", "nativeDragEndToEnd":false,
        "rendererSourceBlake3":blake3::hash(include_bytes!("../src/drag_preview.rs")).to_hex().to_string(),
        "cardVisualSourceBlake3":blake3::hash(include_bytes!("../../src/card_visual.rs")).to_hex().to_string(),
        "colorParserSourceBlake3":blake3::hash(include_bytes!("../../../../crates/paste-domain/src/color.rs")).to_hex().to_string(),
        "logicalPointSize":[196,156], "pixelSize":[392,312],
        "checks":["transparent corners and padding", "native point-size metadata", "header/body location and theme", "image orientation", "valid color swatches", "invalid image fallback", "graphics context restored"],
        "rows":rows,
    });
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This offline renderer check requires macOS.");
    std::process::exit(2);
}
