//! One bounded read of a pasteboard ownership generation. No system access here;
//! macOS provides the native adapter, and tests inject ownership changes.
use chrono::Utc;
use paste_domain::{DeviceMetadata, SourceApplication};

use crate::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, PasteboardDescriptor,
    RawPasteboardItem, RawPasteboardRepresentation, RawPasteboardSnapshot, captures_from_raw,
};

pub(crate) trait SnapshotData {
    fn byte_len(&self) -> usize;
    fn owned_bytes(&self) -> Vec<u8>;
}

pub(crate) trait PasteboardAccess {
    type Item;
    type Data: SnapshotData;
    fn change_count(&self) -> i64;
    fn source(&self) -> SourceApplication;
    fn items(&self) -> Option<Vec<Self::Item>>;
    fn types(&self, item: &Self::Item) -> Vec<String>;
    fn data(&self, item: &Self::Item, uti: &str) -> Option<Self::Data>;
}

pub(crate) struct ReadCursor {
    last_change_count: Option<i64>,
}

impl ReadCursor {
    pub(crate) fn new(change_count: i64, capture_existing: bool) -> Self {
        Self {
            last_change_count: (!capture_existing).then_some(change_count),
        }
    }

    pub(crate) fn discard_current(&mut self, access: &impl PasteboardAccess) {
        // This counter read is the resume boundary. Never ask the provider for
        // items/types/data: it may hold private or delayed-rendered contents.
        self.last_change_count = Some(access.change_count());
    }

    pub(crate) fn poll(
        &mut self,
        access: &impl PasteboardAccess,
        policy: &ClipboardPrivacyPolicy,
        limits: CaptureLimits,
        device: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        let change_count = access.change_count();
        if self.last_change_count == Some(change_count) {
            return Ok(ClipboardPoll::Unchanged);
        }
        let result = read_snapshot(access, change_count, policy, limits, device);
        // NSPasteboardItem objects become stale on an ownership change. Never
        // publish a partially read generation or consume the replacement's
        // counter. One attempt per poll; the next ordinary tick reads afresh.
        if access.change_count() != change_count
            || matches!(result, Err(ClipboardError::ChangedDuringRead))
        {
            return Err(ClipboardError::ChangedDuringRead);
        }
        // Preserve the existing bounded handling of stable policy ignores,
        // empty values and validation/unavailable errors. This is not a generic
        // provider retry or a promise to capture every intermediate copy.
        self.last_change_count = Some(change_count);
        result
    }
}

fn read_snapshot(
    access: &impl PasteboardAccess,
    change_count: i64,
    policy: &ClipboardPrivacyPolicy,
    limits: CaptureLimits,
    device: &DeviceMetadata,
) -> Result<ClipboardPoll, ClipboardError> {
    let source = access.source();
    let native_items = access.items().ok_or(ClipboardError::Unavailable)?;
    let item_types = native_items.iter().map(|item| access.types(item)).collect();
    let descriptor = PasteboardDescriptor {
        change_count,
        source,
        item_types,
    };
    if let Some(reason) = policy.evaluate(&descriptor) {
        return Ok(ClipboardPoll::Ignored {
            change_count,
            reason,
        });
    }
    if native_items.len() > limits.max_items {
        return Err(ClipboardError::TooManyItems {
            actual: native_items.len(),
            limit: limits.max_items,
        });
    }
    let mut total_bytes = 0usize;
    let mut items = Vec::with_capacity(native_items.len());
    for (native_item, types) in native_items.iter().zip(&descriptor.item_types) {
        if types.len() > limits.max_representations_per_item {
            return Err(ClipboardError::TooManyRepresentations {
                actual: types.len(),
                limit: limits.max_representations_per_item,
            });
        }
        let mut representations = Vec::with_capacity(types.len());
        for uti in types {
            if !paste_domain::valid_native_type(uti) {
                return Err(ClipboardError::InvalidNativeType);
            }
            if let Some(data) = access.data(native_item, uti) {
                if data.byte_len() > limits.max_representation_bytes {
                    return Err(ClipboardError::RepresentationTooLarge {
                        uti: uti.clone(),
                        actual: data.byte_len(),
                        limit: limits.max_representation_bytes,
                    });
                }
                total_bytes = total_bytes
                    .checked_add(data.byte_len())
                    .ok_or(ClipboardError::SizeOverflow)?;
                if total_bytes > limits.max_total_bytes {
                    return Err(ClipboardError::SnapshotTooLarge {
                        actual: total_bytes,
                        limit: limits.max_total_bytes,
                    });
                }
                representations.push(RawPasteboardRepresentation {
                    uti: uti.clone(),
                    bytes: data.owned_bytes(),
                });
            }
        }
        items.push(RawPasteboardItem { representations });
    }
    // changeCount tracks ownership, not every in-place data/type update. Catch
    // a changed inventory (including late privacy markers) before publishing.
    // Equal inventories cannot prove same-owner bytes were never modified.
    let final_types = native_items
        .iter()
        .map(|item| access.types(item))
        .collect::<Vec<_>>();
    if final_types != descriptor.item_types {
        return Err(ClipboardError::ChangedDuringRead);
    }
    captures_from_raw(
        RawPasteboardSnapshot { descriptor, items },
        policy,
        limits,
        device,
        Utc::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IgnoreReason;
    use crate::clipboard::CONCEALED_TYPE;
    use paste_domain::DeviceId;
    use std::cell::Cell;

    #[derive(Clone)]
    struct Item {
        generation: i64,
        representations: Vec<(String, Vec<u8>)>,
    }
    struct Source {
        generation: Cell<i64>,
        replace_on_types: Cell<bool>,
        replace_on_data: Cell<Option<usize>>,
        confidential_replacement: bool,
        concealed_in_place: Cell<bool>,
        conceal_on_data: Cell<bool>,
        item_reads: Cell<usize>,
        data_reads: Cell<usize>,
    }
    impl Source {
        fn new() -> Self {
            Self {
                generation: Cell::new(7),
                replace_on_types: Cell::new(false),
                replace_on_data: Cell::new(None),
                confidential_replacement: false,
                concealed_in_place: Cell::new(false),
                conceal_on_data: Cell::new(false),
                item_reads: Cell::new(0),
                data_reads: Cell::new(0),
            }
        }
    }
    impl SnapshotData for Vec<u8> {
        fn byte_len(&self) -> usize {
            self.len()
        }
        fn owned_bytes(&self) -> Vec<u8> {
            self.clone()
        }
    }
    impl PasteboardAccess for Source {
        type Item = Item;
        type Data = Vec<u8>;
        fn change_count(&self) -> i64 {
            self.generation.get()
        }
        fn source(&self) -> SourceApplication {
            SourceApplication::unknown()
        }
        fn items(&self) -> Option<Vec<Item>> {
            self.item_reads.set(self.item_reads.get() + 1);
            let mut representations = if self.generation.get() == 7 {
                vec![
                    (
                        "public.utf16-external-plain-text".into(),
                        vec![0xff, 0xfe, 65, 0],
                    ),
                    ("public.rtf".into(), br"{\rtf1 A}".to_vec()),
                ]
            } else {
                vec![
                    ("public.utf8-plain-text".into(), b"New copy".to_vec()),
                    ("public.html".into(), b"<b>New copy</b>".to_vec()),
                ]
            };
            if (self.generation.get() != 7 && self.confidential_replacement)
                || self.concealed_in_place.get()
            {
                representations.push((CONCEALED_TYPE.into(), vec![]));
            }
            Some(vec![Item {
                generation: self.generation.get(),
                representations,
            }])
        }
        fn types(&self, item: &Item) -> Vec<String> {
            let mut types = item
                .representations
                .iter()
                .map(|(uti, _)| uti.clone())
                .collect::<Vec<_>>();
            if self.concealed_in_place.get() && !types.iter().any(|uti| uti == CONCEALED_TYPE) {
                types.push(CONCEALED_TYPE.into());
            }
            if self.replace_on_types.replace(false) {
                self.generation.set(self.generation.get() + 1);
            }
            types
        }
        fn data(&self, item: &Item, uti: &str) -> Option<Vec<u8>> {
            self.data_reads.set(self.data_reads.get() + 1);
            // Stale NSPasteboardItem access returns nil after ownership changes.
            if item.generation != self.generation.get() {
                return None;
            }
            let result = item
                .representations
                .iter()
                .find(|(kind, _)| kind == uti)
                .map(|(_, bytes)| bytes.clone());
            if self.replace_on_data.get() == Some(self.data_reads.get()) {
                self.replace_on_data.set(None);
                self.generation.set(self.generation.get() + 1);
            }
            if self.conceal_on_data.replace(false) {
                self.concealed_in_place.set(true);
            }
            result
        }
    }
    fn poll(cursor: &mut ReadCursor, source: &Source) -> Result<ClipboardPoll, ClipboardError> {
        cursor.poll(
            source,
            &ClipboardPrivacyPolicy::default(),
            CaptureLimits::default(),
            &DeviceMetadata {
                id: DeviceId::from_uuid(uuid::Uuid::nil()),
                display_name: "Synthetic Mac".into(),
            },
        )
    }

    #[test]
    fn stable_generation_preserves_all_original_bytes_and_is_consumed_once() {
        let source = Source::new();
        let mut cursor = ReadCursor::new(7, true);
        let ClipboardPoll::Captured {
            change_count,
            items,
        } = poll(&mut cursor, &source).expect("stable capture")
        else {
            panic!("capture");
        };
        assert_eq!(change_count, 7);
        assert_eq!(items[0].representations[0].bytes, [0xff, 0xfe, 65, 0]);
        assert_eq!(items[0].representations[1].bytes, br"{\rtf1 A}");
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
        assert_eq!(source.item_reads.get(), 1);
        assert_eq!(source.data_reads.get(), 2);
    }

    #[test]
    fn ownership_change_during_data_read_rejects_partial_and_complete_old_payloads() {
        for replace_at in [1, 2] {
            let source = Source::new();
            source.replace_on_data.set(Some(replace_at));
            let mut cursor = ReadCursor::new(7, true);
            assert_eq!(
                poll(&mut cursor, &source),
                Err(ClipboardError::ChangedDuringRead)
            );
            assert_eq!(source.item_reads.get(), 1, "one attempt, no busy retry");
            let ClipboardPoll::Captured {
                change_count,
                items,
            } = poll(&mut cursor, &source).expect("next generation")
            else {
                panic!("capture");
            };
            assert_eq!(change_count, 8);
            assert_eq!(items[0].representations.len(), 2);
            assert_eq!(items[0].representations[0].bytes, b"New copy");
            assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
        }
    }

    #[test]
    fn ownership_change_during_type_read_is_not_misreported_as_stable_empty_content() {
        let source = Source::new();
        source.replace_on_types.set(true);
        let mut cursor = ReadCursor::new(7, true);
        assert_eq!(
            poll(&mut cursor, &source),
            Err(ClipboardError::ChangedDuringRead)
        );
        assert_eq!(cursor.last_change_count, None);
        assert!(matches!(
            poll(&mut cursor, &source),
            Ok(ClipboardPoll::Captured {
                change_count: 8,
                ..
            })
        ));
    }

    #[test]
    fn replacement_is_rechecked_for_privacy_without_reading_its_payload() {
        let mut source = Source::new();
        source.confidential_replacement = true;
        source.replace_on_data.set(Some(1));
        let mut cursor = ReadCursor::new(7, true);
        assert_eq!(
            poll(&mut cursor, &source),
            Err(ClipboardError::ChangedDuringRead)
        );
        let reads_before = source.data_reads.get();
        assert_eq!(
            poll(&mut cursor, &source),
            Ok(ClipboardPoll::Ignored {
                change_count: 8,
                reason: IgnoreReason::Confidential
            })
        );
        assert_eq!(source.data_reads.get(), reads_before);
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
    }

    #[test]
    fn resume_boundary_reads_only_the_counter_and_keeps_future_generations() {
        let source = Source::new();
        let mut cursor = ReadCursor::new(7, true);
        source.generation.set(8);
        cursor.discard_current(&source);
        assert_eq!(cursor.last_change_count, Some(8));
        assert_eq!(source.item_reads.get(), 0);
        assert_eq!(source.data_reads.get(), 0);
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
        source.generation.set(9);
        assert!(matches!(
            poll(&mut cursor, &source),
            Ok(ClipboardPoll::Captured {
                change_count: 9,
                ..
            })
        ));
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
    }

    #[test]
    fn default_startup_does_not_read_preexisting_contents() {
        let source = Source::new();
        let mut cursor = ReadCursor::new(7, false);
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
        assert_eq!(source.item_reads.get(), 0);
        assert_eq!(source.data_reads.get(), 0);
        source.generation.set(8);
        assert!(matches!(
            poll(&mut cursor, &source),
            Ok(ClipboardPoll::Captured {
                change_count: 8,
                ..
            })
        ));
    }

    #[test]
    fn late_privacy_marker_without_ownership_change_discards_and_rechecks() {
        let source = Source::new();
        source.conceal_on_data.set(true);
        let mut cursor = ReadCursor::new(7, true);
        assert_eq!(
            poll(&mut cursor, &source),
            Err(ClipboardError::ChangedDuringRead)
        );
        assert_eq!(source.generation.get(), 7);
        assert_eq!(cursor.last_change_count, None);
        let reads_before = source.data_reads.get();
        assert_eq!(
            poll(&mut cursor, &source),
            Ok(ClipboardPoll::Ignored {
                change_count: 7,
                reason: IgnoreReason::Confidential,
            })
        );
        assert_eq!(source.data_reads.get(), reads_before);
    }

    #[test]
    fn unstable_validation_error_does_not_consume_a_new_generation() {
        let source = Source::new();
        source.replace_on_data.set(Some(1));
        let mut cursor = ReadCursor::new(7, true);
        let limits = CaptureLimits {
            max_representation_bytes: 3,
            ..Default::default()
        };
        let device = DeviceMetadata {
            id: DeviceId::from_uuid(uuid::Uuid::nil()),
            display_name: "Synthetic Mac".into(),
        };
        assert_eq!(
            cursor.poll(&source, &ClipboardPrivacyPolicy::default(), limits, &device),
            Err(ClipboardError::ChangedDuringRead)
        );
        assert_eq!(cursor.last_change_count, None);
        assert!(matches!(
            cursor.poll(&source, &ClipboardPrivacyPolicy::default(), limits, &device),
            Err(ClipboardError::RepresentationTooLarge { .. })
        ));
        let reads_before = source.data_reads.get();
        assert_eq!(poll(&mut cursor, &source), Ok(ClipboardPoll::Unchanged));
        assert_eq!(
            source.data_reads.get(),
            reads_before,
            "stable rejected generation is not repeatedly copied"
        );
    }
}
