use paste_domain::{CapturePreferences, RetentionPolicy};

pub fn capture_draft(
    days: &str,
    items: &str,
    excluded: &str,
) -> Result<CapturePreferences, &'static str> {
    fn positive(value: &str) -> Result<Option<u32>, &'static str> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        value
            .parse::<u32>()
            .ok()
            .filter(|number| *number > 0)
            .map(Some)
            .ok_or("请输入正整数，留空表示不限。")
    }
    CapturePreferences {
        retention: RetentionPolicy {
            max_age_days: positive(days)?,
            max_unpinned_items: positive(items)?,
        },
        excluded_bundle_ids: excluded
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
    }
    .normalized()
    .map_err(|_| "请输入有效的应用 Bundle ID。")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_limits_never_become_unlimited() {
        for value in ["0", "-1", "1.5", "e", "4294967296"] {
            assert!(capture_draft(value, "", "").is_err());
            assert!(capture_draft("", value, "").is_err());
        }
        assert_eq!(
            capture_draft("", "", "")
                .expect("valid synthetic preferences")
                .retention
                .max_age_days,
            None
        );
        assert_eq!(
            capture_draft(" 7 ", "10", "")
                .expect("valid synthetic preferences")
                .retention
                .max_age_days,
            Some(7)
        );
    }
    #[test]
    fn ignored_apps_are_validated_and_normalized() {
        assert!(capture_draft("7", "", "invalid app").is_err());
        assert_eq!(
            capture_draft("7", "", " COM.Example.Test\ncom.example.test\n")
                .expect("valid synthetic preferences")
                .excluded_bundle_ids,
            vec!["com.example.test"]
        );
    }
}
