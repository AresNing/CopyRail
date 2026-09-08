//! Uses production drawing with synthetic geometry; no window or services.
#[allow(dead_code)]
#[path = "../../src/drag_feedback.rs"]
mod drag_feedback;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "../src/native_drop_feedback.rs"]
mod native_drop_feedback;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
    };
    use objc2_core_graphics::CGContext;
    use objc2_foundation::NSSize;
    let _app = NSApplication::sharedApplication(MainThreadMarker::new().ok_or("main thread")?);
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/native-drop-feedback");
    std::fs::create_dir_all(&output)?;
    let mut checks = Vec::new();
    for (name, label, count, x) in [
        ("work", "工作", 1, 170.0),
        ("batch", "归档", 3, 170.0),
        ("edge", "很长的分类名称用于检查省略与边缘安全", 200, 8.0),
        ("insert", "", 1, 170.0),
        ("insert-left", "", 3, 8.0),
        ("insert-right", "", 200, 392.0),
    ] {
        let bitmap = unsafe { NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),std::ptr::null_mut(),800,224,8,4,true,false,NSDeviceRGBColorSpace,0,0) }.ok_or("bitmap")?;
        bitmap.setSize(NSSize::new(400.0, 112.0));
        let context =
            NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap).ok_or("context")?;
        let cg = context.CGContext();
        CGContext::translate_ctm(Some(&cg), 0.0, 112.0);
        CGContext::scale_ctm(Some(&cg), 1.0, -1.0);
        let flipped = NSGraphicsContext::graphicsContextWithCGContext_flipped(&cg, true);
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&flipped));
        if name.starts_with("insert") {
            native_drop_feedback::paint_insertion(x, 8.0, 94.0, count, 400.0, 112.0);
        } else {
            native_drop_feedback::paint(
                &drag_feedback::DragTab {
                    id: paste_domain::PinboardId::new(),
                    x,
                    y: 8.0,
                    width: 60.0,
                    height: 27.0,
                },
                label,
                count,
                400.0,
                112.0,
            );
        }
        assert_eq!(
            NSGraphicsContext::currentContext()
                .as_ref()
                .map(objc2::rc::Retained::as_ptr),
            Some(objc2::rc::Retained::as_ptr(&flipped))
        );
        NSGraphicsContext::restoreGraphicsState_class();
        let data = bitmap.TIFFRepresentation().ok_or("tiff")?;
        let png = image::load_from_memory_with_format(&data.to_vec(), image::ImageFormat::Tiff)?
            .to_rgba8();
        assert_eq!(png.dimensions(), (800, 224));
        assert_eq!(png.get_pixel(799, 223).0[3], 0);
        let blue = if name.starts_with("insert") {
            png.get_pixel((x * 2.0) as u32, 120).0
        } else {
            png.get_pixel(((x + 30.0) * 2.0) as u32, 24).0
        };
        assert!(
            blue[2] > 150 && blue[0] < 20 && blue[3] == 255,
            "target pill or insertion line visible"
        );
        assert!(
            png.pixels()
                .filter(|p| p.0[0] > 220 && p.0[1] > 220 && p.0[2] > 220 && p.0[3] > 240)
                .count()
                > 80,
            "white text rendered"
        );
        png.save(output.join(format!("feedback-{name}.png")))?;
        checks.push(name);
    }
    let report = serde_json::json!({"result":"passed","nativeGesture":false,"pixelSize":[800,224],"pointSize":[400,112],"checks":checks,"rendererBlake3":blake3::hash(include_bytes!("../src/native_drop_feedback.rs")).to_hex().to_string()});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("AppKit renderer requires macOS");
}
