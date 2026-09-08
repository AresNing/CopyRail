#![cfg_attr(not(test), forbid(unsafe_code))]
#![cfg_attr(test, deny(unsafe_code))]

mod clipboard;
#[cfg(any(target_os = "macos", test))]
mod paste_target;
#[cfg(any(target_os = "macos", test))]
mod read_session;
#[cfg(target_os = "macos")]
pub use paste_target::{PasteOutcome, PasteProgress};
mod write_plan;
pub use write_plan::ClipboardWriteMode;
#[cfg(target_os = "macos")]
mod ocr;
mod preview;

#[cfg(target_os = "macos")]
mod macos;

pub use clipboard::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
    INTERNAL_PASTEBOARD_TYPE, IgnoreReason, PasteboardDescriptor, RawPasteboardItem,
    RawPasteboardRepresentation, RawPasteboardSnapshot, captures_from_raw,
};
#[cfg(target_os = "macos")]
pub use ocr::{OcrError, RecognizedText, recognize_text};
pub use preview::{
    GeneratedThumbnail, ImageRotation, PreviewError, generate_image_thumbnail, rotate_image,
};

#[cfg(target_os = "macos")]
pub use macos::{
    MacClipboardReader, MacClipboardWriter, MacPasteAttempt, MacPasteInvocation, MacPasteTarget,
};
