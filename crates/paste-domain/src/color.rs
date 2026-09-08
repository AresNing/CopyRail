/// Interpret a standalone opaque RGB code without changing its source text.
/// A leading `#` permits digits only; unprefixed codes need an A–F letter so
/// six-digit numbers (for example verification codes) remain ordinary text.
/// Surrounding whitespace is ignored for display/classification only.
#[must_use]
pub fn parse_color_code(text: &str) -> Option<[u8; 3]> {
    let value = text.trim();
    let (digits, prefixed) = value
        .strip_prefix('#')
        .map_or((value, false), |digits| (digits, true));
    if digits.len() != 6
        || !digits.as_bytes().iter().all(u8::is_ascii_hexdigit)
        || (!prefixed && !digits.as_bytes().iter().any(u8::is_ascii_alphabetic))
    {
        return None;
    }
    Some([
        u8::from_str_radix(&digits[0..2], 16).ok()?,
        u8::from_str_radix(&digits[2..4], 16).ok()?,
        u8::from_str_radix(&digits[4..6], 16).ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_six_digit_hex_accepts_optional_hash_and_mixed_case() {
        for text in ["#1A2B3C", "1a2b3c", " 1A2b3C\n"] {
            assert_eq!(parse_color_code(text), Some([0x1a, 0x2b, 0x3c]));
        }
        assert_eq!(parse_color_code("#235442"), Some([0x23, 0x54, 0x42]));
        assert_eq!(parse_color_code("ABCDEF"), Some([0xab, 0xcd, 0xef]));
    }

    #[test]
    fn numbers_shorthand_other_formats_and_mixed_content_remain_text() {
        for text in [
            "235442",
            "000000",
            "#FFF",
            "fff",
            "#12345678",
            "rgba(1,2,3,1)",
            "red",
            "Use #123456",
            "#123456\n#abcdef",
            "#123456;background:red",
            "#12 345",
            "##123456",
            "#12345g",
            "0xABCD",
            "#ééé",
            "ＡＢＣＤＥＦ",
            "#12\u{200b}3456",
            "#123456\0",
            "",
            "\n",
            "#",
        ] {
            assert_eq!(parse_color_code(text), None, "{text:?}");
        }
    }
}
