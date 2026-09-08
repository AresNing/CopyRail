//! Shared bounded cache policy; no application discovery or IO here.
use std::collections::HashMap;

pub const MAX_ICON_BYTES: usize = 64 * 1024;
pub const MAX_ICON_BATCH: usize = 16;
const CAPACITY: usize = 256;
const FOUND_TTL_MS: u64 = 300_000;
const MISSING_TTL_MS: u64 = 30_000;

#[derive(Clone)]
struct Entry<T> {
    value: Option<T>,
    loaded_at: u64,
}

#[derive(Clone)]
pub struct IconCache<T> {
    entries: HashMap<String, Entry<T>>,
}

impl<T> Default for IconCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<T> IconCache<T> {
    pub fn get(&self, key: &str, now: u64) -> Option<&Option<T>> {
        let entry = self.entries.get(key)?;
        let ttl = if entry.value.is_some() {
            FOUND_TTL_MS
        } else {
            MISSING_TTL_MS
        };
        let age = now.checked_sub(entry.loaded_at)?;
        (age < ttl).then_some(&entry.value)
    }

    /// Keep the previous image visible during a background refresh.
    pub fn peek(&self, key: &str) -> Option<&T> {
        self.entries.get(key)?.value.as_ref()
    }

    pub fn insert(&mut self, key: String, value: Option<T>, now: u64) {
        // Expiry schedules a reload, not removal of every other visible image.
        // Capacity eviction still bounds memory while multi-batch refresh runs.
        if self.entries.len() >= CAPACITY
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.loaded_at)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            Entry {
                value,
                loaded_at: now,
            },
        );
    }
}

pub fn valid_bundle_id(value: &str) -> bool {
    (3..=255).contains(&value.len())
        && value.contains('.')
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

pub fn safe_icon_url(value: &str) -> bool {
    value.len() <= MAX_ICON_BYTES.div_ceil(3) * 4 + 22
        && value
            .strip_prefix("data:image/png;base64,")
            .is_some_and(|body| {
                !body.is_empty()
                    && body
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positive_and_negative_ttls_expire_and_clock_rollback_is_not_fresh() {
        let mut cache = IconCache::default();
        cache.insert("a".into(), Some(1), 100);
        cache.insert("b".into(), None, 100);
        assert_eq!(cache.get("a", 299_999), Some(&Some(1)));
        assert_eq!(cache.get("a", 300_100), None);
        assert_eq!(cache.peek("a"), Some(&1));
        assert_eq!(cache.get("b", 30_099), Some(&None));
        assert_eq!(cache.get("b", 30_100), None);
        assert_eq!(cache.get("a", 99), None);
        cache.insert("c".into(), Some(2), 300_101);
        assert_eq!(
            cache.peek("a"),
            Some(&1),
            "another batch must not clear stale visible icons"
        );
    }
    #[test]
    fn cache_never_exceeds_capacity_and_can_replace_an_existing_key() {
        let mut cache = IconCache::default();
        for i in 0..=CAPACITY {
            cache.insert(i.to_string(), Some(i), i as u64);
        }
        assert_eq!(cache.entries.len(), CAPACITY);
        assert_eq!(cache.peek("0"), None);
        cache.insert("1".into(), Some(999), 1000);
        assert_eq!(cache.entries.len(), CAPACITY);
        assert_eq!(cache.peek("1"), Some(&999));
    }
    #[test]
    fn bundle_ids_are_identifiers_not_paths_or_urls() {
        assert!(valid_bundle_id("com.apple.TextEdit"));
        assert!(valid_bundle_id("com.example.App-beta_1"));
        for value in [
            "",
            "com..app",
            "com.app/../../x",
            "file:///Applications/x.app",
            "com.app\n",
            "应用.test",
            ".com.app",
            "com.app.",
        ] {
            assert!(!valid_bundle_id(value), "{value:?}");
        }
        assert!(!valid_bundle_id(&format!("com.{}", "a".repeat(252))));
    }
    #[test]
    fn image_sources_are_bounded_png_data_not_external_or_active_documents() {
        assert!(safe_icon_url("data:image/png;base64,aGVsbG8="));
        for value in [
            "https://example.invalid/icon.png",
            "data:image/svg+xml,<svg/>",
            "file:///tmp/a.png",
            "data:image/png;base64,",
            "data:image/png;base64,a\n",
        ] {
            assert!(!safe_icon_url(value));
        }
        assert!(!safe_icon_url(&format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_ICON_BYTES * 2)
        )));
    }
}
