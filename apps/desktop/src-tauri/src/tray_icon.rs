//! CopyRail's canonical cards-and-rail mark as an AppKit alpha template.
//! Regenerate with `node scripts/generate-brand-icons.mjs` after SVG edits.

pub fn template_icon() -> tauri::image::Image<'static> {
    let image = image::load_from_memory(include_bytes!("../icons/tray/36x36.png"))
        .expect("bundled CopyRail tray artwork must be a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    tauri::image::Image::new_owned(image.into_raw(), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_icon_contains_only_black_and_alpha_not_the_application_background() {
        let icon = template_icon();
        assert_eq!((icon.width(), icon.height()), (36, 36));
        let (pixels, remainder) = icon.rgba().as_chunks::<4>();
        assert!(remainder.is_empty());
        assert!(pixels.iter().all(|pixel| pixel[..3] == [0, 0, 0]));
        assert!(pixels.iter().any(|pixel| pixel[3] == 255));
        assert!(pixels.iter().any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }

    #[test]
    fn template_has_transparent_edges_and_readable_interior_at_menu_size() {
        let icon = template_icon();
        let alpha = |x: u32, y: u32| icon.rgba()[((y * 36 + x) * 4 + 3) as usize];
        for position in 0..36 {
            assert_eq!(alpha(position, 0), 0);
            assert_eq!(alpha(position, 35), 0);
            assert_eq!(alpha(0, position), 0);
            assert_eq!(alpha(35, position), 0);
        }
        assert_eq!(alpha(10, 16), 0); // Outlined card stays hollow.
        assert_eq!(alpha(25, 16), 255); // Foreground card remains solid.
        assert_eq!(alpha(18, 30), 255); // Rail remains legible.
    }
}
