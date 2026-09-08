//! Validate and prepare a complete write without touching any pasteboard.
use crate::{CaptureLimits, ClipboardError};
use paste_domain::{CapturedRepresentation, RepresentationKind};
use std::{borrow::Cow, collections::HashMap};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardWriteMode {
    Original,
    PlainText,
}

pub(crate) struct NativeRepresentation<'a> {
    pub native_type: Cow<'a, str>,
    pub bytes: Cow<'a, [u8]>,
}

pub(crate) fn plan_write<'a>(
    items: &[&'a [CapturedRepresentation]],
    mode: ClipboardWriteMode,
    limits: CaptureLimits,
) -> Result<Vec<Vec<NativeRepresentation<'a>>>, ClipboardError> {
    if items.is_empty() {
        return Err(ClipboardError::NoTextualRepresentation);
    }
    if items.len() > limits.max_items {
        return Err(ClipboardError::TooManyItems {
            actual: items.len(),
            limit: limits.max_items,
        });
    }
    let mut total_bytes = 0usize;
    let mut output_bytes = 0usize;
    let mut plan = Vec::with_capacity(items.len());
    for representations in items {
        if representations.is_empty() {
            return Err(ClipboardError::NoTextualRepresentation);
        }
        if representations.len() > limits.max_representations_per_item {
            return Err(ClipboardError::TooManyRepresentations {
                actual: representations.len(),
                limit: limits.max_representations_per_item,
            });
        }
        for representation in *representations {
            representation
                .validate()
                .map_err(|_| ClipboardError::InvalidNativeType)?;
            total_bytes = total_bytes
                .checked_add(representation.bytes.len())
                .ok_or(ClipboardError::SizeOverflow)?;
            if representation.bytes.len() > limits.max_representation_bytes {
                return Err(ClipboardError::RepresentationTooLarge {
                    uti: representation.native_type.clone().unwrap_or_default(),
                    actual: representation.bytes.len(),
                    limit: limits.max_representation_bytes,
                });
            }
            if total_bytes > limits.max_total_bytes {
                return Err(ClipboardError::SnapshotTooLarge {
                    actual: total_bytes,
                    limit: limits.max_total_bytes,
                });
            }
        }
        if mode == ClipboardWriteMode::PlainText {
            let text = representations
                .iter()
                .filter(|representation| representation.kind == RepresentationKind::Url)
                .find_map(CapturedRepresentation::decoded_text)
                .or_else(|| {
                    representations
                        .iter()
                        .find_map(CapturedRepresentation::decoded_text)
                })
                .ok_or(ClipboardError::NoTextualRepresentation)?;
            let bytes = match text {
                Cow::Borrowed(value) => Cow::Borrowed(value.as_bytes()),
                Cow::Owned(value) => Cow::Owned(value.into_bytes()),
            };
            if bytes.len() > limits.max_representation_bytes {
                return Err(ClipboardError::RepresentationTooLarge {
                    uti: "public.utf8-plain-text".into(),
                    actual: bytes.len(),
                    limit: limits.max_representation_bytes,
                });
            }
            output_bytes = output_bytes
                .checked_add(bytes.len())
                .ok_or(ClipboardError::SizeOverflow)?;
            if output_bytes > limits.max_total_bytes {
                return Err(ClipboardError::SnapshotTooLarge {
                    actual: output_bytes,
                    limit: limits.max_total_bytes,
                });
            }
            plan.push(vec![NativeRepresentation {
                native_type: Cow::Borrowed("public.utf8-plain-text"),
                bytes,
            }]);
            continue;
        }
        let mut seen = HashMap::<&str, &[u8]>::new();
        let mut planned = Vec::with_capacity(representations.len());
        for representation in *representations {
            let native_type = original_type(representation)?;
            if let Some(previous) = seen.insert(native_type, &representation.bytes) {
                if previous != representation.bytes {
                    return Err(ClipboardError::DuplicateNativeType);
                }
                continue;
            }
            planned.push(NativeRepresentation {
                native_type: Cow::Borrowed(native_type),
                bytes: Cow::Borrowed(&representation.bytes),
            });
        }
        plan.push(planned);
    }
    Ok(plan)
}

fn original_type(representation: &CapturedRepresentation) -> Result<&str, ClipboardError> {
    if let Some(native_type) = &representation.native_type {
        return Ok(native_type);
    }
    Ok(match &representation.kind {
        RepresentationKind::PlainText
            if representation.bytes.starts_with(&[0xff, 0xfe])
                || representation.bytes.starts_with(&[0xfe, 0xff]) =>
        {
            "public.utf16-external-plain-text"
        }
        RepresentationKind::PlainText => "public.utf8-plain-text",
        RepresentationKind::Html => "public.html",
        RepresentationKind::Rtf => "public.rtf",
        RepresentationKind::Url => "public.url",
        RepresentationKind::Png => "public.png",
        RepresentationKind::Tiff => "public.tiff",
        RepresentationKind::Pdf => "com.adobe.pdf",
        RepresentationKind::FileUrls => {
            if representation.bytes.starts_with(b"bplist")
                || representation
                    .bytes
                    .iter()
                    .find(|byte| !byte.is_ascii_whitespace())
                    == Some(&b'<')
            {
                "NSFilenamesPboardType"
            } else {
                let value = std::str::from_utf8(&representation.bytes)
                    .map_err(|_| ClipboardError::AmbiguousLegacyRepresentation)?;
                if value.trim().lines().count() != 1 {
                    return Err(ClipboardError::AmbiguousLegacyRepresentation);
                }
                "public.file-url"
            }
        }
        RepresentationKind::Color
            if representation
                .utf8_text()
                .is_some_and(|text| text.starts_with('#')) =>
        {
            "public.utf8-plain-text"
        }
        RepresentationKind::Color => "com.apple.cocoa.pasteboard.color",
        RepresentationKind::Custom(value) => value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(kind: RepresentationKind, native_type: &str, bytes: &[u8]) -> CapturedRepresentation {
        CapturedRepresentation {
            kind,
            native_type: Some(native_type.into()),
            bytes: bytes.to_vec(),
            mime_type: None,
            file_name: None,
        }
    }

    #[test]
    fn original_plan_retains_native_aliases_and_binary_types_byte_for_byte() {
        let items = vec![
            typed(
                RepresentationKind::PlainText,
                "public.utf8-plain-text",
                b"text",
            ),
            typed(
                RepresentationKind::PlainText,
                "public.utf16-external-plain-text",
                &[0xff, 0xfe, b't', 0],
            ),
            typed(
                RepresentationKind::Url,
                "public.url",
                b"https://example.com",
            ),
            typed(
                RepresentationKind::PlainText,
                "public.url-name",
                b"Link title",
            ),
            typed(
                RepresentationKind::FileUrls,
                "NSFilenamesPboardType",
                b"bplist00\0\xff",
            ),
            typed(
                RepresentationKind::Rtf,
                "NeXT Rich Text Format v1.0 pasteboard type",
                br"{\rtf1 synthetic}",
            ),
            typed(
                RepresentationKind::Color,
                "com.apple.cocoa.pasteboard.color",
                b"bplist00\0\xfd",
            ),
            typed(
                RepresentationKind::Custom("synthetic.type".into()),
                "synthetic.type",
                b"\0\xff",
            ),
        ];
        let plan = plan_write(
            &[&items],
            ClipboardWriteMode::Original,
            CaptureLimits::default(),
        )
        .expect("native plan");
        assert_eq!(plan[0].len(), items.len());
        for (written, source) in plan[0].iter().zip(&items) {
            assert_eq!(
                Some(written.native_type.as_ref()),
                source.native_type.as_deref()
            );
            assert_eq!(written.bytes.as_ref(), source.bytes);
        }
    }

    #[test]
    fn plain_text_decodes_utf16_and_a_bad_second_item_rejects_the_entire_plan() {
        let original = vec![typed(
            RepresentationKind::PlainText,
            "public.utf16-external-plain-text",
            &[0xff, 0xfe, b'A', 0],
        )];
        let plan = plan_write(
            &[&original],
            ClipboardWriteMode::PlainText,
            CaptureLimits::default(),
        )
        .expect("plain text plan");
        assert_eq!(plan[0][0].native_type, "public.utf8-plain-text");
        assert_eq!(plan[0][0].bytes.as_ref(), b"A");
        let image = vec![typed(
            RepresentationKind::Png,
            "public.png",
            b"synthetic binary",
        )];
        assert!(matches!(
            plan_write(
                &[&original, &image],
                ClipboardWriteMode::PlainText,
                CaptureLimits::default()
            ),
            Err(ClipboardError::NoTextualRepresentation)
        ));
        assert_eq!(original[0].bytes, [0xff, 0xfe, b'A', 0]);
    }

    #[test]
    fn conflicting_native_types_are_rejected_before_any_system_write() {
        let a = typed(
            RepresentationKind::PlainText,
            "public.utf8-plain-text",
            b"one",
        );
        let b = typed(
            RepresentationKind::PlainText,
            "public.utf8-plain-text",
            b"two",
        );
        assert!(matches!(
            plan_write(
                &[&[a.clone(), b]],
                ClipboardWriteMode::Original,
                CaptureLimits::default()
            ),
            Err(ClipboardError::DuplicateNativeType)
        ));
        let duplicates = vec![a.clone(), a];
        assert_eq!(
            plan_write(
                &[&duplicates],
                ClipboardWriteMode::Original,
                CaptureLimits::default()
            )
            .expect("identical duplicates")[0]
                .len(),
            1
        );
        let invalid = vec![typed(RepresentationKind::PlainText, "bad\0type", b"one")];
        assert!(matches!(
            plan_write(
                &[&invalid],
                ClipboardWriteMode::Original,
                CaptureLimits::default()
            ),
            Err(ClipboardError::InvalidNativeType)
        ));
    }

    #[test]
    fn validation_enforces_whole_batch_and_individual_payload_budgets() {
        let items = vec![CapturedRepresentation::plain_text("1234")];
        assert!(matches!(
            plan_write(
                &[&items],
                ClipboardWriteMode::Original,
                CaptureLimits {
                    max_representation_bytes: 3,
                    ..Default::default()
                }
            ),
            Err(ClipboardError::RepresentationTooLarge { .. })
        ));
        assert!(matches!(
            plan_write(
                &[&items, &items],
                ClipboardWriteMode::Original,
                CaptureLimits {
                    max_total_bytes: 7,
                    ..Default::default()
                }
            ),
            Err(ClipboardError::SnapshotTooLarge { .. })
        ));
        assert!(matches!(
            plan_write(
                &[&items, &items],
                ClipboardWriteMode::Original,
                CaptureLimits {
                    max_items: 1,
                    ..Default::default()
                }
            ),
            Err(ClipboardError::TooManyItems { .. })
        ));
        assert!(plan_write(&[], ClipboardWriteMode::Original, CaptureLimits::default()).is_err());
        // Native-endian UTF-16 can expand when converted to UTF-8. Check the
        // output budget as well as the original input before clearing anything.
        let expanded = vec![typed(
            RepresentationKind::PlainText,
            "public.utf16-external-plain-text",
            &0x4e2du16.to_ne_bytes(),
        )];
        assert!(matches!(
            plan_write(
                &[&expanded],
                ClipboardWriteMode::PlainText,
                CaptureLimits {
                    max_representation_bytes: 2,
                    ..Default::default()
                }
            ),
            Err(ClipboardError::RepresentationTooLarge { actual: 3, .. })
        ));
        assert!(matches!(
            plan_write(
                &[&expanded, &expanded],
                ClipboardWriteMode::PlainText,
                CaptureLimits {
                    max_total_bytes: 5,
                    ..Default::default()
                }
            ),
            Err(ClipboardError::SnapshotTooLarge { actual: 6, .. })
        ));
    }

    #[test]
    fn legacy_file_arrays_are_not_mislabeled_as_a_single_file_url() {
        let legacy = vec![CapturedRepresentation {
            kind: RepresentationKind::FileUrls,
            native_type: None,
            bytes: b"bplist00synthetic".to_vec(),
            mime_type: None,
            file_name: None,
        }];
        let plan = plan_write(
            &[&legacy],
            ClipboardWriteMode::Original,
            CaptureLimits::default(),
        )
        .expect("legacy signature");
        assert_eq!(plan[0][0].native_type, "NSFilenamesPboardType");
        assert_eq!(plan[0][0].bytes.as_ref(), legacy[0].bytes);
        let ambiguous = vec![CapturedRepresentation {
            bytes: b"file:///a\nfile:///b".to_vec(),
            ..legacy[0].clone()
        }];
        assert!(matches!(
            plan_write(
                &[&ambiguous],
                ClipboardWriteMode::Original,
                CaptureLimits::default()
            ),
            Err(ClipboardError::AmbiguousLegacyRepresentation)
        ));
    }
}
