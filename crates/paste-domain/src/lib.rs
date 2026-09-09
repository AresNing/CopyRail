#![forbid(unsafe_code)]

mod clip;
mod color;
pub mod i18n;
mod pinboard;
mod query;
pub use i18n::Language;

pub use clip::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, ClipItem, ContentKind, DeviceId,
    DeviceMetadata, DomainError, PersistedRepresentation, RepresentationKind, SourceApplication,
    valid_native_type,
};
pub use color::parse_color_code;
pub use pinboard::{Pinboard, PinboardId};
pub use query::{
    CapturePreferences, DesktopPreferences, DeviceFacet, RetentionPolicy, SearchFacets,
    SearchFilters, SearchHit, SearchPage, SearchQuery, SourceFacet,
};
