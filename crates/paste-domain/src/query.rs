use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ClipItem, ContentKind, DeviceId, DomainError, PinboardId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFacet {
    pub bundle_identifier: String,
    pub display_name: String,
    pub item_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceFacet {
    pub id: DeviceId,
    pub display_name: String,
    pub item_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchFacets {
    pub sources: Vec<SourceFacet>,
    pub devices: Vec<DeviceFacet>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchFilters {
    pub content_kinds: Vec<ContentKind>,
    pub source_bundle_ids: Vec<String>,
    pub device_ids: Vec<DeviceId>,
    pub pinboard_ids: Vec<PinboardId>,
    pub copied_after: Option<DateTime<Utc>>,
    pub copied_before: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchPage {
    pub limit: u32,
    pub offset: u32,
}

impl SearchPage {
    #[must_use]
    pub const fn new(limit: u32, offset: u32) -> Self {
        Self {
            limit: if limit > 200 { 200 } else { limit },
            offset,
        }
    }
}

impl Default for SearchPage {
    fn default() -> Self {
        Self::new(50, 0)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SearchHit {
    pub item: ClipItem,
    pub rank: f64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetentionPolicy {
    pub max_age_days: Option<u32>,
    pub max_unpinned_items: Option<u32>,
}

impl RetentionPolicy {
    pub fn validate(self) -> Result<(), DomainError> {
        if self.max_age_days == Some(0) || self.max_unpinned_items == Some(0) {
            return Err(DomainError::InvalidRetentionLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapturePreferences {
    pub retention: RetentionPolicy,
    pub excluded_bundle_ids: Vec<String>,
}

impl CapturePreferences {
    pub fn normalized(mut self) -> Result<Self, DomainError> {
        self.retention.validate()?;
        for bundle_id in &mut self.excluded_bundle_ids {
            *bundle_id = bundle_id.trim().to_ascii_lowercase();
            if bundle_id.is_empty()
                || bundle_id.len() > 255
                || !bundle_id
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '-'))
            {
                return Err(DomainError::InvalidBundleIdentifier(bundle_id.clone()));
            }
        }
        self.excluded_bundle_ids.sort();
        self.excluded_bundle_ids.dedup();
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct DesktopPreferences {
    pub language: crate::LanguagePreference,
    pub opening_position: OpeningPosition,
    pub launch_at_login: bool,
    pub screen_share_protection: bool,
    pub compact_mode: bool,
    #[serde(deserialize_with = "deserialize_transparency")]
    pub background_transparency: u8,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            language: Default::default(),
            opening_position: OpeningPosition::default(),
            launch_at_login: false,
            screen_share_protection: false,
            compact_mode: false,
            background_transparency: 50,
        }
    }
}

fn deserialize_transparency<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u8, D::Error> {
    let value = u8::deserialize(deserializer)?;
    if value > 100 {
        return Err(serde::de::Error::custom(
            "background transparency must be 0–100",
        ));
    }
    Ok(value)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SearchContext {
    pub text: String,
    pub board: Option<PinboardId>,
    pub kind: Option<ContentKind>,
    pub source: Option<String>,
    pub device: Option<DeviceId>,
    pub days: Option<i64>,
    pub history_offset: u32,
}

impl SearchContext {
    pub fn is_search(&self) -> bool {
        !self.text.trim().is_empty()
            || self.kind.is_some()
            || self.source.is_some()
            || self.device.is_some()
            || self.days.is_some()
    }

    /// A board is a browsing context, never an implicit global-search filter.
    pub fn scoped_board(&self) -> Option<PinboardId> {
        if self.is_search() { None } else { self.board }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningPosition {
    #[default]
    Latest,
    Last,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RailPosition {
    pub context: SearchContext,
    pub clip_id: crate::ClipId,
}
