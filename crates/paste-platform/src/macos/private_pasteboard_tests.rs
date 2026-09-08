//! Opt-in AppKit/pasteboard-server checks. Fresh unique pasteboards only: never
//! request the general board, foreground metadata, accessibility, or key events.
use super::*;
use crate::{IgnoreReason, clipboard::CONCEALED_TYPE};
use paste_domain::{DeviceId, RepresentationKind};
use std::{cell::Cell, io::Cursor};

struct PrivatePasteboard(Retained<NSPasteboard>);

impl PrivatePasteboard {
    fn new() -> Self {
        Self(NSPasteboard::pasteboardWithUniqueName())
    }

    fn reader(&self, capture_existing: bool) -> MacClipboardReader {
        MacClipboardReader {
            pasteboard: self.0.clone(),
            cursor: ReadCursor::new(self.0.changeCount() as i64, capture_existing),
        }
    }

    fn write(&self, items: &[Vec<CapturedRepresentation>], mode: ClipboardWriteMode) -> i64 {
        MacClipboardWriter::write_slices_to(
            &items.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            mode,
            || self.0.clone(),
        )
        .expect("write synthetic private destination")
    }

    // Independent simulated external producer: no PasteRS internal marker and
    // no production write planner. The receiver still uses real AppKit items.
    fn publish(&self, items: &[Vec<CapturedRepresentation>]) {
        let native = items
            .iter()
            .map(|representations| {
                let item = NSPasteboardItem::new();
                for representation in representations {
                    assert!(item.setData_forType(
                        &NSData::with_bytes(&representation.bytes),
                        &NSString::from_str(representation.native_type.as_deref().expect("type")),
                    ));
                }
                ProtocolObject::<dyn NSPasteboardWriting>::from_retained(item)
            })
            .collect::<Vec<_>>();
        let objects = NSArray::from_retained_slice(&native);
        let _ = self.0.clearContents();
        assert!(self.0.writeObjects(&objects));
    }
}

impl Drop for PrivatePasteboard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // Only a freshly created unique board can enter this guard. Never use
        // releaseGlobally on a standard or externally supplied pasteboard.
        // SAFETY: objc2-app-kit 0.3.2 omits this method because its ObjC return
        // is `oneway void` (translation-config.toml). AppKit implements this
        // zero-argument void selector on the live retained NSPasteboard. This
        // test-owned unique board is not used again after the call. Production
        // retains forbid(unsafe_code); the exception is local to test cleanup.
        unsafe {
            let (): () = objc2::msg_send![&*self.0, releaseGlobally];
        }
    }
}

// Delegate all native data/count operations to the production adapter, but do
// not query the user's foreground app merely to label synthetic test content.
struct TestAccess<'a> {
    native: NativePasteboardAccess<'a>,
    data_reads: Cell<usize>,
}

impl PasteboardAccess for TestAccess<'_> {
    type Item = Retained<NSPasteboardItem>;
    type Data = Retained<NSData>;

    fn change_count(&self) -> i64 {
        self.native.change_count()
    }
    fn source(&self) -> SourceApplication {
        SourceApplication::unknown()
    }
    fn items(&self) -> Option<Vec<Self::Item>> {
        self.native.items()
    }
    fn types(&self, item: &Self::Item) -> Vec<String> {
        self.native.types(item)
    }
    fn data(&self, item: &Self::Item, uti: &str) -> Option<Self::Data> {
        self.data_reads.set(self.data_reads.get() + 1);
        self.native.data(item, uti)
    }
}

fn access(board: &PrivatePasteboard) -> TestAccess<'_> {
    TestAccess {
        native: NativePasteboardAccess(&board.0),
        data_reads: Cell::new(0),
    }
}

fn poll(reader: &mut MacClipboardReader, access: &TestAccess<'_>) -> ClipboardPoll {
    reader
        .cursor
        .poll(
            access,
            &ClipboardPrivacyPolicy::secure_default(),
            CaptureLimits::default(),
            &DeviceMetadata {
                id: DeviceId::from_uuid(uuid::Uuid::nil()),
                display_name: "Synthetic native check".into(),
            },
        )
        .expect("read private pasteboard")
}

fn typed(native_type: &str, bytes: impl Into<Vec<u8>>) -> CapturedRepresentation {
    CapturedRepresentation {
        kind: crate::clipboard::representation_kind(native_type),
        native_type: Some(native_type.into()),
        bytes: bytes.into(),
        mime_type: None,
        file_name: None,
    }
}

fn samples() -> Vec<Vec<CapturedRepresentation>> {
    let text = "合成样例 🦀\nline two";
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("synthetic PNG");
    vec![
        vec![
            typed("public.utf16-external-plain-text", utf16),
            typed("public.utf8-plain-text", text.as_bytes()),
            typed("public.rtf", br"{\rtf1 synthetic}".as_slice()),
            typed("public.html", b"<b>synthetic</b>".as_slice()),
            typed("io.pasters.synthetic.binary", vec![0, 255, 3, 0, 128]),
        ],
        vec![
            typed("public.url-name", b"Synthetic title".as_slice()),
            typed("public.url", b"https://example.com/native-check".as_slice()),
        ],
        vec![typed("public.png", png.into_inner())],
    ]
}

#[test]
#[ignore = "requires macOS pasteboard server; only fresh private synthetic pasteboards"]
fn native_private_roundtrip_preserves_raw_multi_item_data_and_self_filtering() {
    let source = PrivatePasteboard::new();
    let destination = PrivatePasteboard::new();
    let originals = samples();
    source.publish(&originals);
    let mut reader = source.reader(true);
    let source_access = access(&source);
    let ClipboardPoll::Captured { items, .. } = poll(&mut reader, &source_access) else {
        panic!("external synthetic content must be captured");
    };
    assert_eq!(items.len(), originals.len());
    // Compare capture to the actual published source, not data passed to a
    // producer before AppKit materializes its representations. A separate
    // pre-reader probe established that AppKit already exposes a normalized
    // UTF-16 value for this deliberately inconsistent rich fixture. It does
    // not establish which of the rich representations supplied that value.
    // Do not warm the provider before our reader in this test.
    let published = source
        .0
        .pasteboardItems()
        .expect("published source")
        .to_vec();
    for ((item, expected), native) in items.iter().zip(&originals).zip(&published) {
        assert_eq!(item.representations.len(), expected.len());
        for original in expected {
            let native_type = original.native_type.as_deref().expect("source type");
            let actual = native
                .dataForType(&NSString::from_str(native_type))
                .expect("published bytes")
                .to_vec();
            let captured = item
                .representations
                .iter()
                .find(|representation| representation.native_type == original.native_type)
                .expect("all native types");
            assert_eq!(captured.bytes, actual);
            if native_type != "public.utf16-external-plain-text" {
                assert_eq!(
                    actual, original.bytes,
                    "other fixture representations remain exact"
                );
            }
        }
    }
    assert_eq!(poll(&mut reader, &source_access), ClipboardPoll::Unchanged);
    let payloads = items
        .into_iter()
        .map(|item| item.representations)
        .collect::<Vec<_>>();
    let written_count = destination.write(&payloads, ClipboardWriteMode::Original);
    assert_eq!(written_count, destination.0.changeCount() as i64);
    let native = destination.0.pasteboardItems().expect("written items");
    assert_eq!(native.len(), originals.len());
    for (item, expected) in native.to_vec().iter().zip(&payloads) {
        for original in expected {
            assert_eq!(
                item.dataForType(&NSString::from_str(
                    original.native_type.as_deref().expect("type")
                ))
                .expect("restored bytes")
                .to_vec(),
                original.bytes
            );
        }
    }
    let destination_access = access(&destination);
    assert!(matches!(
        poll(&mut destination.reader(true), &destination_access),
        ClipboardPoll::Ignored {
            reason: IgnoreReason::ApplicationGenerated,
            ..
        }
    ));
    assert_eq!(
        destination_access.data_reads.get(),
        0,
        "self marker is checked before obtaining any body"
    );
}

#[test]
#[ignore = "requires macOS pasteboard server; only fresh private synthetic pasteboards"]
fn native_private_invalid_batch_preserves_destination_and_plain_text_decodes() {
    let board = PrivatePasteboard::new();
    let originals = samples();
    let count = board.write(&originals, ClipboardWriteMode::Original);
    let before = board
        .0
        .pasteboardItems()
        .expect("original contents")
        .to_vec();
    let invalid = [
        typed("public.utf8-plain-text", b"A".as_slice()),
        typed("public.utf8-plain-text", b"B".as_slice()),
    ];
    let reached_destination = Cell::new(false);
    assert_eq!(
        MacClipboardWriter::write_slices_to(
            &[&originals[0], &invalid],
            ClipboardWriteMode::Original,
            || {
                reached_destination.set(true);
                board.0.clone()
            }
        ),
        Err(ClipboardError::DuplicateNativeType)
    );
    assert!(
        !reached_destination.get(),
        "validation finishes before acquiring destination"
    );
    assert_eq!(board.0.changeCount() as i64, count);
    assert_eq!(
        board.0.pasteboardItems().expect("unchanged items").len(),
        before.len()
    );
    assert_eq!(
        before[0]
            .dataForType(&NSString::from_str("public.utf8-plain-text"))
            .expect("unchanged data")
            .to_vec(),
        originals[0][1].bytes
    );
    // UTF-16 conversion and URL preference must survive the real server too.
    let plain = vec![vec![originals[0][0].clone()], originals[1].clone()];
    board.write(&plain, ClipboardWriteMode::PlainText);
    let items = board.0.pasteboardItems().expect("plain items").to_vec();
    assert_eq!(items.len(), 2);
    for (item, expected) in items
        .iter()
        .zip(["合成样例 🦀\nline two", "https://example.com/native-check"])
    {
        assert_eq!(
            item.stringForType(&NSString::from_str("public.utf8-plain-text"))
                .expect("native decoded string")
                .to_string(),
            expected
        );
        assert!(
            item.dataForType(&NSString::from_str("public.rtf"))
                .is_none()
        );
    }
}

#[test]
#[ignore = "requires macOS pasteboard server; only fresh private synthetic pasteboards"]
fn native_private_startup_resume_and_concealed_content_do_not_replay() {
    let board = PrivatePasteboard::new();
    let text = |value: &str| vec![vec![typed("public.utf8-plain-text", value.as_bytes())]];
    board.publish(&text("preexisting synthetic"));
    let mut reader = board.reader(false);
    let source_access = access(&board);
    assert_eq!(poll(&mut reader, &source_access), ClipboardPoll::Unchanged);
    board.publish(&text("last paused synthetic copy"));
    reader.discard_current().expect("real counter boundary");
    assert_eq!(poll(&mut reader, &source_access), ClipboardPoll::Unchanged);
    assert_eq!(source_access.data_reads.get(), 0);
    board.publish(&text("after resume synthetic copy"));
    assert!(matches!(
        poll(&mut reader, &source_access),
        ClipboardPoll::Captured { .. }
    ));
    let before = source_access.data_reads.get();
    let mut concealed = text("synthetic secret marker test, not a real secret");
    concealed[0].push(typed(CONCEALED_TYPE, b"1".as_slice()));
    assert!(matches!(
        concealed[0][1].kind,
        RepresentationKind::Custom(_)
    ));
    board.publish(&concealed);
    assert!(matches!(
        poll(&mut reader, &source_access),
        ClipboardPoll::Ignored {
            reason: IgnoreReason::Confidential,
            ..
        }
    ));
    assert_eq!(
        source_access.data_reads.get(),
        before,
        "privacy ignore must not request payload data"
    );
    assert_eq!(poll(&mut reader, &source_access), ClipboardPoll::Unchanged);
}
