//! Read an installed application's icon only, never launch or enumerate apps.
#[allow(dead_code)]
#[path = "../../src/source_icon.rs"]
pub(crate) mod policy;

#[cfg(target_os = "macos")]
thread_local! {
    static CACHE: std::cell::RefCell<policy::IconCache<Vec<u8>>> = Default::default();
    static START: std::time::Instant = std::time::Instant::now();
}

#[cfg(target_os = "macos")]
fn now_ms() -> u64 {
    START.with(|start| u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// Read-only warm-cache path for latency-sensitive drag startup.
#[cfg(target_os = "macos")]
pub fn cached(bundle_id: &str) -> Option<Vec<u8>> {
    CACHE.with(|cache| cache.borrow().get(bundle_id, now_ms()).cloned().flatten())
}

#[cfg(target_os = "macos")]
pub fn load(bundle_id: &str) -> Result<Option<Vec<u8>>, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    MainThreadMarker::new().ok_or("来源图标必须在主线程读取。")?;
    if !policy::valid_bundle_id(bundle_id) {
        return Ok(None);
    }
    let now = now_ms();
    if let Some(value) = CACHE.with(|cache| cache.borrow().get(bundle_id, now).cloned()) {
        return Ok(value);
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let bytes = if let Some(url) =
        workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
        && url.isFileURL()
        && let Some(path) = url.path()
    {
        Some(render(&workspace.iconForFile(&path))?)
    } else {
        None
    };
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(bundle_id.to_owned(), bytes.clone(), now)
    });
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn render(icon: &objc2_app_kit::NSImage) -> Result<Vec<u8>, String> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), 64, 64, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
        )
    }.ok_or("无法创建来源图标位图。")?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .ok_or("无法创建图标绘图上下文。")?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    unsafe {
        icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0)),
            NSRect::new(NSPoint::new(0.0, 0.0), icon.size()),
            NSCompositingOperation::Copy,
            1.0,
            false,
            None,
        );
    }
    NSGraphicsContext::restoreGraphicsState_class();
    let bytes = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("无法编码来源图标。")?
    .to_vec();
    if bytes.len() > policy::MAX_ICON_BYTES {
        return Err("来源图标超过显示预算。".into());
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn load(_bundle_id: &str) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn cached(_bundle_id: &str) -> Option<Vec<u8>> {
    None
}
