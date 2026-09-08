//! Render the production alpha mask for visual review, without starting AppKit.
#[path = "../src/tray_icon.rs"]
mod tray_icon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::path::Path::new("target/tray-icon-verification");
    std::fs::create_dir_all(output)?;
    let icon = tray_icon::template_icon();
    image::save_buffer(
        output.join("tray-template.png"),
        icon.rgba(),
        icon.width(),
        icon.height(),
        image::ColorType::Rgba8,
    )?;
    // Preview composition only; AppKit supplies the actual menu-bar colours.
    let preview = image::RgbImage::from_fn(144, 72, |x, y| {
        let dark = x >= 72;
        let background = if dark { 35_u16 } else { 245_u16 };
        let foreground = if dark { 245_u16 } else { 20_u16 };
        let local_x = x % 72;
        let alpha = if (18..54).contains(&local_x) && (18..54).contains(&y) {
            u16::from(icon.rgba()[(((y - 18) * 36 + local_x - 18) * 4 + 3) as usize])
        } else {
            0
        };
        let grey = ((foreground * alpha + background * (255 - alpha)) / 255) as u8;
        image::Rgb([grey; 3])
    });
    preview.save(output.join("tray-preview.png"))?;
    println!("{}", output.join("tray-template.png").display());
    Ok(())
}
