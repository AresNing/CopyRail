//! Offline installed-resource verification. No window, app launch, history,
//! clipboard, permissions or network; only two named system application icons.
#[allow(unsafe_code, dead_code)]
#[path = "../src/app_icons.rs"]
mod app_icons;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use image::GenericImageView;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSGraphicsContext};
    let mtm = MainThreadMarker::new().ok_or("main thread required")?;
    let _app = NSApplication::sharedApplication(mtm);
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/native-source-icons");
    std::fs::create_dir_all(&output)?;
    let mut rows = Vec::new();
    for (bundle, name) in [
        ("com.apple.TextEdit", "textedit.png"),
        ("com.apple.finder", "finder.png"),
    ] {
        let before = NSGraphicsContext::currentContext().map(|c| objc2::rc::Retained::as_ptr(&c));
        let bytes = app_icons::load(bundle)?.ok_or("required named system app unavailable")?;
        let after = NSGraphicsContext::currentContext().map(|c| objc2::rc::Retained::as_ptr(&c));
        assert_eq!(before, after, "graphics context must be restored");
        assert_eq!(app_icons::cached(bundle), Some(bytes.clone()));
        assert_eq!(
            app_icons::load(bundle)?,
            Some(bytes.clone()),
            "warm load unchanged"
        );
        assert!(bytes.len() <= app_icons::policy::MAX_ICON_BYTES);
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?;
        assert_eq!(image.dimensions(), (64, 64));
        assert!(
            image
                .pixels()
                .filter(|(_, _, pixel)| pixel.0[3] > 0)
                .count()
                > 256
        );
        assert!(
            image.pixels().any(|(_, _, pixel)| pixel.0[3] == 0),
            "transparent margin expected"
        );
        std::fs::write(output.join(name), &bytes)?;
        rows.push(serde_json::json!({"bundleId":bundle,"file":name,"pixels":[64,64],"bytes":bytes.len(),"blake3":blake3::hash(&bytes).to_hex().to_string()}));
    }
    assert!(app_icons::load("io.pasters.not-installed-fixture")?.is_none());
    assert!(app_icons::load("file:///Applications/TextEdit.app")?.is_none());
    let report = serde_json::json!({"result":"passed","nativeWindowEndToEnd":false,"sourceBlake3":blake3::hash(include_bytes!("../src/app_icons.rs")).to_hex().to_string(),"policyBlake3":blake3::hash(include_bytes!("../../src/source_icon.rs")).to_hex().to_string(),"checks":["two explicitly named installed system icons", "64px PNG with transparent margins", "warm cache identical", "missing and invalid identifiers fall back", "graphics context restored"],"rows":rows});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Requires macOS");
    std::process::exit(2);
}
