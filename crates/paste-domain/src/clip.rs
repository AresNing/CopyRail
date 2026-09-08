use std::{borrow::Cow, fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

id_type!(ClipId);
id_type!(DeviceId);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceApplication {
    pub bundle_identifier: String,
    pub display_name: String,
}

impl SourceApplication {
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            bundle_identifier: "unknown".into(),
            display_name: "Unknown".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceMetadata {
    pub id: DeviceId,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureFlags {
    pub confidential: bool,
    pub transient: bool,
    pub local_only: bool,
}

impl CaptureFlags {
    #[must_use]
    pub const fn should_persist(self) -> bool {
        !self.confidential && !self.transient
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RepresentationKind {
    PlainText,
    Html,
    Rtf,
    Url,
    Png,
    Tiff,
    Pdf,
    FileUrls,
    Color,
    Custom(String),
}

impl RepresentationKind {
    #[must_use]
    pub fn storage_key(&self) -> String {
        match self {
            Self::PlainText => "text/plain".into(),
            Self::Html => "text/html".into(),
            Self::Rtf => "text/rtf".into(),
            Self::Url => "public.url".into(),
            Self::Png => "image/png".into(),
            Self::Tiff => "image/tiff".into(),
            Self::Pdf => "application/pdf".into(),
            Self::FileUrls => "public.file-url-list".into(),
            Self::Color => "public.color".into(),
            Self::Custom(value) => format!("custom:{value}"),
        }
    }

    #[must_use]
    pub fn from_storage_key(value: &str) -> Self {
        match value {
            "text/plain" => Self::PlainText,
            "text/html" => Self::Html,
            "text/rtf" => Self::Rtf,
            "public.url" => Self::Url,
            "image/png" => Self::Png,
            "image/tiff" => Self::Tiff,
            "application/pdf" => Self::Pdf,
            "public.file-url-list" => Self::FileUrls,
            "public.color" => Self::Color,
            custom => Self::Custom(custom.trim_start_matches("custom:").into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapturedRepresentation {
    pub kind: RepresentationKind,
    /// Original pasteboard type, not the normalized content family. None is
    /// retained for older backups and app-created representations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_type: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

impl CapturedRepresentation {
    #[must_use]
    pub fn plain_text(value: impl Into<String>) -> Self {
        Self {
            kind: RepresentationKind::PlainText,
            native_type: None,
            mime_type: Some("text/plain; charset=utf-8".into()),
            file_name: None,
            bytes: value.into().into_bytes(),
        }
    }

    #[must_use]
    pub fn utf8_text(&self) -> Option<&str> {
        if self.native_type.as_deref() == Some("public.utf16-external-plain-text")
            || self.bytes.starts_with(&[0xff, 0xfe])
            || self.bytes.starts_with(&[0xfe, 0xff])
            || (self.kind == RepresentationKind::Color
                && self
                    .native_type
                    .as_deref()
                    .is_some_and(|kind| kind != "public.color"))
        {
            return None;
        }
        match self.kind {
            RepresentationKind::PlainText | RepresentationKind::Url | RepresentationKind::Color => {
                std::str::from_utf8(&self.bytes).ok()
            }
            _ => None,
        }
    }

    pub fn decoded_text(&self) -> Option<Cow<'_, str>> {
        if !matches!(
            self.kind,
            RepresentationKind::PlainText | RepresentationKind::Url | RepresentationKind::Color
        ) {
            return None;
        }
        if self.native_type.as_deref() == Some("public.utf16-external-plain-text")
            || self.bytes.starts_with(&[0xff, 0xfe])
            || self.bytes.starts_with(&[0xfe, 0xff])
        {
            let (little_endian, bytes) = if self.bytes.starts_with(&[0xff, 0xfe]) {
                (true, &self.bytes[2..])
            } else if self.bytes.starts_with(&[0xfe, 0xff]) {
                (false, &self.bytes[2..])
            }
            // Apple's external plain-text UTI permits native byte order
            // when its optional BOM is absent.
            else {
                (cfg!(target_endian = "little"), self.bytes.as_slice())
            };
            let (pairs, remainder) = bytes.as_chunks::<2>();
            if !remainder.is_empty() {
                return None;
            }
            let units = pairs
                .iter()
                .map(|pair| {
                    if little_endian {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    }
                })
                .collect::<Vec<_>>();
            return String::from_utf16(&units).ok().map(Cow::Owned);
        }
        self.utf8_text().map(Cow::Borrowed)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self
            .native_type
            .as_deref()
            .is_some_and(|value| !valid_native_type(value))
            || matches!(&self.kind, RepresentationKind::Custom(value) if !valid_native_type(value))
        {
            return Err(DomainError::InvalidNativeType);
        }
        Ok(())
    }
}

#[must_use]
pub fn valid_native_type(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapturedItem {
    pub captured_at: DateTime<Utc>,
    pub source: SourceApplication,
    pub device: DeviceMetadata,
    pub flags: CaptureFlags,
    pub representations: Vec<CapturedRepresentation>,
}

impl CapturedItem {
    pub fn validate(&self) -> Result<(), DomainError> {
        for representation in &self.representations {
            representation.validate()?;
        }
        if self.representations.is_empty() {
            return Err(DomainError::NoRepresentations);
        }
        if self
            .representations
            .iter()
            .all(|item| item.bytes.is_empty())
        {
            return Err(DomainError::EmptyContent);
        }
        Ok(())
    }

    #[must_use]
    pub fn primary_text(&self) -> Option<&str> {
        self.representations
            .iter()
            .find_map(CapturedRepresentation::utf8_text)
    }

    pub fn primary_decoded_text(&self) -> Option<Cow<'_, str>> {
        self.representations
            .iter()
            .filter(|item| item.kind == RepresentationKind::Url)
            .find_map(CapturedRepresentation::decoded_text)
            .or_else(|| {
                self.representations
                    .iter()
                    .find_map(CapturedRepresentation::decoded_text)
            })
    }

    #[must_use]
    pub fn infer_content_kind(&self) -> ContentKind {
        let has = |needle| self.representations.iter().any(|item| item.kind == needle);

        if has(RepresentationKind::FileUrls) {
            ContentKind::File
        } else if has(RepresentationKind::Pdf) {
            ContentKind::Pdf
        } else if has(RepresentationKind::Png) || has(RepresentationKind::Tiff) {
            ContentKind::Image
        } else if has(RepresentationKind::Color) {
            ContentKind::Color
        } else if has(RepresentationKind::Url)
            || self
                .primary_decoded_text()
                .is_some_and(|text| looks_like_url(&text))
        {
            ContentKind::Link
        } else if self.representations.iter()
            .filter(|item| item.kind == RepresentationKind::PlainText)
            .find_map(CapturedRepresentation::decoded_text)
            .is_some_and(|text| crate::parse_color_code(&text).is_some())
        {
            ContentKind::Color
        } else if has(RepresentationKind::Rtf)
            || self.representations.iter().any(|item| item.native_type.as_deref() == Some("com.apple.flat-rtfd")
                || matches!(&item.kind, RepresentationKind::Custom(kind) if kind == "com.apple.flat-rtfd")) {
            ContentKind::RichText
        } else if has(RepresentationKind::Html) {
            ContentKind::Html
        } else if has(RepresentationKind::PlainText) {
            ContentKind::Text
        } else {
            ContentKind::Unknown
        }
    }
}

fn looks_like_url(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.contains(char::is_whitespace)
        && (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    RichText,
    Html,
    Link,
    Image,
    File,
    Pdf,
    Color,
    Unknown,
}

impl ContentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::RichText => "rich_text",
            Self::Html => "html",
            Self::Link => "link",
            Self::Image => "image",
            Self::File => "file",
            Self::Pdf => "pdf",
            Self::Color => "color",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for ContentKind {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "text" => Ok(Self::Text),
            "rich_text" => Ok(Self::RichText),
            "html" => Ok(Self::Html),
            "link" => Ok(Self::Link),
            "image" => Ok(Self::Image),
            "file" => Ok(Self::File),
            "pdf" => Ok(Self::Pdf),
            "color" => Ok(Self::Color),
            "unknown" => Ok(Self::Unknown),
            other => Err(DomainError::UnknownContentKind(other.into())),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedRepresentation {
    pub kind: RepresentationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_type: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub byte_len: u64,
    pub content_hash: [u8; 32],
    pub text_preview: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClipItem {
    pub id: ClipId,
    pub captured_at: DateTime<Utc>,
    pub last_copied_at: DateTime<Utc>,
    pub source: SourceApplication,
    pub device: DeviceMetadata,
    pub content_kind: ContentKind,
    pub title: String,
    pub searchable_text: String,
    pub content_hash: [u8; 32],
    pub representations: Vec<PersistedRepresentation>,
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("clipboard item does not contain a representation")]
    NoRepresentations,
    #[error("clipboard item contains only empty representations")]
    EmptyContent,
    #[error(
        "native pasteboard type must be nonempty, at most 512 bytes, and contain no control characters"
    )]
    InvalidNativeType,
    #[error("unknown content kind: {0}")]
    UnknownContentKind(String),
    #[error("retention limits must be greater than zero")]
    InvalidRetentionLimit,
    #[error("invalid application bundle identifier: {0}")]
    InvalidBundleIdentifier(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_rtfd_is_rich_text_even_without_an_rtf_alias() {
        let capture = CapturedItem {
            captured_at: Utc::now(),
            source: SourceApplication::unknown(),
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Synthetic Mac".into(),
            },
            flags: CaptureFlags::default(),
            representations: vec![
                CapturedRepresentation {
                    kind: RepresentationKind::Custom("com.apple.flat-rtfd".into()),
                    native_type: Some("com.apple.flat-rtfd".into()),
                    mime_type: None,
                    file_name: None,
                    bytes: b"synthetic attachment document".to_vec(),
                },
                CapturedRepresentation::plain_text("attachment text"),
            ],
        };
        assert_eq!(capture.infer_content_kind(), ContentKind::RichText);
    }

    #[test]
    fn utf16_decoding_preserves_raw_bytes_and_handles_both_byte_orders() {
        let text = "原样保留 🦀";
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let representation = CapturedRepresentation {
                native_type: Some("public.utf16-external-plain-text".into()),
                bytes: bytes.clone(),
                ..CapturedRepresentation::plain_text("")
            };
            assert_eq!(representation.decoded_text().as_deref(), Some(text));
            assert!(representation.utf8_text().is_none());
            assert_eq!(representation.bytes, bytes);
        }
        let bytes = text
            .encode_utf16()
            .flat_map(u16::to_ne_bytes)
            .collect::<Vec<_>>();
        let representation = CapturedRepresentation {
            native_type: Some("public.utf16-external-plain-text".into()),
            bytes,
            ..CapturedRepresentation::plain_text("")
        };
        assert_eq!(representation.decoded_text().as_deref(), Some(text));
    }

    #[test]
    fn invalid_unicode_and_native_types_are_not_silently_repaired() {
        for bytes in [vec![0xff, 0xfe, 1], vec![0xff, 0xfe, 0, 0xd8]] {
            let representation = CapturedRepresentation {
                native_type: Some("public.utf16-external-plain-text".into()),
                bytes,
                ..CapturedRepresentation::plain_text("")
            };
            assert!(representation.decoded_text().is_none());
        }
        for native_type in [String::new(), "x\0y".into(), "x\ny".into(), "x".repeat(513)] {
            let representation = CapturedRepresentation {
                native_type: Some(native_type),
                ..CapturedRepresentation::plain_text("synthetic")
            };
            assert!(representation.validate().is_err());
        }
        assert!(valid_native_type(
            "NeXT Rich Text Format v1.0 pasteboard type"
        ));
    }

    fn capture(value: &str) -> CapturedItem {
        CapturedItem {
            captured_at: Utc::now(),
            source: SourceApplication::unknown(),
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Test Mac".into(),
            },
            flags: CaptureFlags::default(),
            representations: vec![CapturedRepresentation::plain_text(value)],
        }
    }

    #[test]
    fn recognizes_links_without_losing_plain_text_representation() {
        assert_eq!(
            capture("https://pasteapp.io/help").infer_content_kind(),
            ContentKind::Link
        );
        assert_eq!(
            capture("hello world").infer_content_kind(),
            ContentKind::Text
        );
    }

    #[test]
    fn recognizes_color_text_without_changing_representations() {
        for value in ["#1A2B3C", "1a2b3c", "#235442", "  aBcDeF\n"] {
            let capture = capture(value);
            let originals = capture.representations.clone();
            assert_eq!(capture.infer_content_kind(), ContentKind::Color);
            assert_eq!(capture.representations, originals);
            assert_eq!(capture.primary_decoded_text().as_deref(), Some(value));
        }
        for value in ["235442", "#FFF", "color: #123456", "rgb(1,2,3)"] {
            assert_eq!(capture(value).infer_content_kind(), ContentKind::Text);
        }
    }

    #[test]
    fn color_detection_decodes_utf16_but_keeps_higher_priority_types() {
        let mut capture = capture("");
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend("aB12Ef".encode_utf16().flat_map(u16::to_le_bytes));
        capture.representations[0].native_type = Some("public.utf16-external-plain-text".into());
        capture.representations[0].bytes = bytes.clone();
        assert_eq!(capture.infer_content_kind(), ContentKind::Color);
        assert_eq!(capture.representations[0].bytes, bytes);
        for (kind, expected) in [
            (RepresentationKind::Png, ContentKind::Image),
            (RepresentationKind::Pdf, ContentKind::Pdf),
            (RepresentationKind::FileUrls, ContentKind::File),
            (RepresentationKind::Url, ContentKind::Link),
        ] {
            capture.representations.push(CapturedRepresentation {
                kind,
                ..CapturedRepresentation::plain_text("synthetic")
            });
            assert_eq!(capture.infer_content_kind(), expected);
            capture.representations.pop();
        }
        capture.representations.push(CapturedRepresentation {
            kind: RepresentationKind::Html,
            ..CapturedRepresentation::plain_text("<b>aB12Ef</b>")
        });
        assert_eq!(capture.infer_content_kind(), ContentKind::Color);
        capture.representations.remove(0);
        assert_eq!(capture.infer_content_kind(), ContentKind::Html);
    }

    #[test]
    fn confidential_and_transient_content_is_not_persistable() {
        assert!(
            !CaptureFlags {
                confidential: true,
                ..CaptureFlags::default()
            }
            .should_persist()
        );
        assert!(
            !CaptureFlags {
                transient: true,
                ..CaptureFlags::default()
            }
            .should_persist()
        );
    }
}
