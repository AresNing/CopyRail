use chrono::Utc;
use paste_domain::{DeviceId, DeviceMetadata, SourceApplication};
use paste_platform::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, IgnoreReason,
    PasteboardDescriptor, RawPasteboardItem, RawPasteboardRepresentation, RawPasteboardSnapshot,
    captures_from_raw,
};

fn descriptor(types: &[&str]) -> PasteboardDescriptor {
    PasteboardDescriptor {
        change_count: 7,
        source: SourceApplication {
            bundle_identifier: "com.example.Editor".into(),
            display_name: "Editor".into(),
        },
        item_types: vec![types.iter().map(ToString::to_string).collect()],
    }
}

fn device() -> DeviceMetadata {
    DeviceMetadata {
        id: DeviceId::from_uuid(uuid::Uuid::nil()),
        display_name: "Test Mac".into(),
    }
}

#[test]
fn rejects_concealed_content_before_conversion() {
    let snapshot = RawPasteboardSnapshot {
        descriptor: descriptor(&["public.utf8-plain-text", "org.nspasteboard.ConcealedType"]),
        items: vec![RawPasteboardItem {
            representations: vec![RawPasteboardRepresentation {
                uti: "public.utf8-plain-text".into(),
                bytes: b"password".to_vec(),
            }],
        }],
    };
    let result = captures_from_raw(
        snapshot,
        &ClipboardPrivacyPolicy::secure_default(),
        CaptureLimits::default(),
        &device(),
        Utc::now(),
    )
    .expect("evaluate capture");

    assert!(matches!(
        result,
        ClipboardPoll::Ignored {
            reason: IgnoreReason::Confidential,
            ..
        }
    ));
}

#[test]
fn excluded_bundle_ids_are_case_insensitive() {
    let mut policy = ClipboardPrivacyPolicy::secure_default();
    policy.exclude_bundle_id("COM.EXAMPLE.EDITOR");
    assert_eq!(
        policy.evaluate(&descriptor(&["public.utf8-plain-text"])),
        Some(IgnoreReason::ExcludedApplication)
    );
}

#[test]
fn converts_multiple_native_representations_without_losing_bytes() {
    let snapshot = RawPasteboardSnapshot {
        descriptor: descriptor(&["public.utf8-plain-text", "public.html"]),
        items: vec![RawPasteboardItem {
            representations: vec![
                RawPasteboardRepresentation {
                    uti: "public.utf8-plain-text".into(),
                    bytes: b"Hello".to_vec(),
                },
                RawPasteboardRepresentation {
                    uti: "public.html".into(),
                    bytes: b"<b>Hello</b>".to_vec(),
                },
            ],
        }],
    };
    let result = captures_from_raw(
        snapshot,
        &ClipboardPrivacyPolicy::secure_default(),
        CaptureLimits::default(),
        &device(),
        Utc::now(),
    )
    .expect("convert capture");

    let ClipboardPoll::Captured { items, .. } = result else {
        panic!("expected captured items");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].representations.len(), 2);
    assert_eq!(
        items[0].representations[0].native_type.as_deref(),
        Some("public.utf8-plain-text")
    );
    assert_eq!(
        items[0].representations[1].native_type.as_deref(),
        Some("public.html")
    );
    assert_eq!(items[0].primary_text(), Some("Hello"));
}

#[test]
fn link_title_and_url_remain_distinct_and_search_uses_the_real_url() {
    let snapshot = RawPasteboardSnapshot {
        descriptor: descriptor(&["public.url-name", "public.url"]),
        items: vec![RawPasteboardItem {
            representations: vec![
                RawPasteboardRepresentation {
                    uti: "public.url-name".into(),
                    bytes: b"A title".to_vec(),
                },
                RawPasteboardRepresentation {
                    uti: "public.url".into(),
                    bytes: b"https://example.com".to_vec(),
                },
            ],
        }],
    };
    let ClipboardPoll::Captured { items, .. } = captures_from_raw(
        snapshot,
        &ClipboardPrivacyPolicy::secure_default(),
        CaptureLimits::default(),
        &device(),
        Utc::now(),
    )
    .expect("convert aliases") else {
        panic!("capture");
    };
    assert_eq!(
        items[0].primary_decoded_text().as_deref(),
        Some("https://example.com")
    );
    assert_eq!(
        items[0].representations[0].native_type.as_deref(),
        Some("public.url-name")
    );
    assert_eq!(
        items[0].representations[0].kind,
        paste_domain::RepresentationKind::PlainText
    );
    assert_eq!(
        items[0].representations[1].kind,
        paste_domain::RepresentationKind::Url
    );
}

#[test]
fn enforces_representation_size_before_persistence() {
    let snapshot = RawPasteboardSnapshot {
        descriptor: descriptor(&["public.png"]),
        items: vec![RawPasteboardItem {
            representations: vec![RawPasteboardRepresentation {
                uti: "public.png".into(),
                bytes: vec![0; 5],
            }],
        }],
    };
    let result = captures_from_raw(
        snapshot,
        &ClipboardPrivacyPolicy::secure_default(),
        CaptureLimits {
            max_representation_bytes: 4,
            ..CaptureLimits::default()
        },
        &device(),
        Utc::now(),
    );

    assert!(matches!(
        result,
        Err(ClipboardError::RepresentationTooLarge { .. })
    ));
}

#[test]
fn ignores_content_written_back_by_the_application() {
    assert_eq!(
        ClipboardPrivacyPolicy::secure_default().evaluate(&descriptor(&[
            "public.utf8-plain-text",
            "io.pasters.internal.clip",
        ])),
        Some(IgnoreReason::ApplicationGenerated)
    );
}
