use paste_domain::{ContentKind, parse_color_code};

/// Only a canonical, opaque RGB value may enter the inline swatch style.
/// Unsupported legacy representations remain readable text, never CSS.
pub fn color_swatch(kind: ContentKind, text: &str) -> Option<(String, &'static str)> {
    if kind != ContentKind::Color {
        return None;
    }
    let [r, g, b] = parse_color_code(text)?;
    let channel = |byte| {
        let srgb = f64::from(byte) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
    // Choose the higher of black/white contrast, including mid-tone colors.
    let ink = if (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) {
        "#000000"
    } else {
        "#ffffff"
    };
    Some((format!("#{r:02X}{g:02X}{b:02X}"), ink))
}

pub fn text_summary(kind: ContentKind, text: &str) -> Option<String> {
    matches!(
        kind,
        ContentKind::Text | ContentKind::RichText | ContentKind::Html
    )
    .then(|| format!("{} 字符", text.chars().count()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_color_and_contrasting_ink() {
        assert_eq!(
            color_swatch(ContentKind::Color, " #fAfAfA\n"),
            Some(("#FAFAFA".into(), "#000000"))
        );
        assert_eq!(
            color_swatch(ContentKind::Color, "#123456"),
            Some(("#123456".into(), "#ffffff"))
        );
        assert_eq!(
            color_swatch(ContentKind::Color, "#777777"),
            Some(("#777777".into(), "#000000"))
        );
        assert_eq!(
            color_swatch(ContentKind::Color, "aBcDeF"),
            Some(("#ABCDEF".into(), "#000000"))
        );
    }

    #[test]
    fn text_and_unsupported_colors_cannot_inject_style() {
        assert_eq!(color_swatch(ContentKind::Text, "#123456"), None);
        for value in [
            "235442",
            "#123",
            "#12345678",
            "#zzzzzz",
            "#123456;background:url(x)",
            "éééé",
            "rgb(1,2,3)",
            "",
        ] {
            assert_eq!(color_swatch(ContentKind::Color, value), None);
        }
    }

    #[test]
    fn every_rgb_sample_selects_ink_above_minimum_text_contrast() {
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let linear = |n: i32| {
                        let s = f64::from(n) / 255.0;
                        if s <= 0.04045 {
                            s / 12.92
                        } else {
                            ((s + 0.055) / 1.055).powf(2.4)
                        }
                    };
                    let luma = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
                    let (_, ink) =
                        color_swatch(ContentKind::Color, &format!("#{r:02x}{g:02x}{b:02x}"))
                            .expect("RGB");
                    let ratio = if ink == "#000000" {
                        (luma + 0.05) / 0.05
                    } else {
                        1.05 / (luma + 0.05)
                    };
                    assert!(ratio >= 4.5, "contrast {ratio}");
                }
            }
        }
    }

    #[test]
    fn text_count_uses_unicode_scalars_not_utf8_bytes() {
        assert_eq!(
            text_summary(ContentKind::Text, "中文🦀\n"),
            Some("4 字符".into())
        );
        assert_eq!(
            text_summary(ContentKind::RichText, ""),
            Some("0 字符".into())
        );
        assert_eq!(text_summary(ContentKind::Image, "OCR result"), None);
    }
}
