use std::{
    collections::HashSet,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::Duration,
};

use paste_domain::{ClipId, RepresentationKind};
use paste_storage::SqliteStore;
use thiserror::Error;

const DRAG_EXPORT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
pub(crate) const MAX_DRAG_FILES: usize = 1_000;
const MAX_FILE_LIST_BYTES: usize = 1024 * 1024;

pub fn prepare_drag_files(
    store: &SqliteStore,
    cache_root: &Path,
    clip_ids: &[ClipId],
) -> Result<Vec<PathBuf>, DragExportError> {
    if clip_ids.is_empty() {
        return Err(DragExportError::Empty);
    }
    if clip_ids.len() > 200 {
        return Err(DragExportError::TooManyFiles);
    }
    let export_root = cache_root.join("drag-exports");
    fs::create_dir_all(&export_root)?;
    cleanup_stale_exports(&export_root);
    let session = export_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&session)?;

    let result = export_selection(store, &session, clip_ids);
    if result.is_err() {
        // A bad item later in the selection must not leave earlier exported
        // clipboard bytes behind. This directory was created by this call only.
        fs::remove_dir_all(&session)?;
    }
    result
}

fn export_selection(
    store: &SqliteStore,
    session: &Path,
    clip_ids: &[ClipId],
) -> Result<Vec<PathBuf>, DragExportError> {
    let mut paths = Vec::with_capacity(clip_ids.len());
    for (index, clip_id) in clip_ids.iter().copied().enumerate() {
        let item = store
            .get_clip(clip_id)?
            .ok_or(paste_storage::StorageError::NotFound)?;
        let representations = store.load_clipboard_payload(clip_id)?;

        let mut original_paths = Vec::new();
        let mut seen = HashSet::new();
        for representation in representations
            .iter()
            .filter(|representation| representation.kind == RepresentationKind::FileUrls)
        {
            for path in file_url_paths(&representation.bytes)? {
                if !path.try_exists()? {
                    return Err(DragExportError::MissingOriginalFile);
                }
                if seen.insert(path.clone()) {
                    original_paths.push(path);
                }
            }
        }
        if !original_paths.is_empty() {
            if paths.len() + original_paths.len() > MAX_DRAG_FILES {
                return Err(DragExportError::TooManyFiles);
            }
            paths.extend(original_paths);
            continue;
        }

        let representation = preferred_export_representation(&representations)
            .ok_or(DragExportError::UnsupportedRepresentation)?;
        let extension = export_extension(&representation.kind);
        let stem = sanitize_file_stem(&item.title);
        let path = session.join(format!("{:02}-{stem}.{extension}", index + 1));
        if paths.len() >= MAX_DRAG_FILES {
            return Err(DragExportError::TooManyFiles);
        }
        if matches!(
            representation.kind,
            RepresentationKind::PlainText | RepresentationKind::Url | RepresentationKind::Color
        ) {
            // Exported .txt files have an explicit UTF-8 contract; a native
            // UTF-16 payload without a BOM must not become an unreadable file.
            let text = representation
                .decoded_text()
                .ok_or(DragExportError::UnsupportedRepresentation)?;
            fs::write(&path, text.as_bytes())?;
        } else {
            fs::write(&path, &representation.bytes)?;
        }
        paths.push(path);
    }

    if paths.is_empty() {
        Err(DragExportError::Empty)
    } else {
        Ok(paths)
    }
}

fn preferred_export_representation(
    representations: &[paste_domain::CapturedRepresentation],
) -> Option<&paste_domain::CapturedRepresentation> {
    const ORDER: [RepresentationKind; 8] = [
        RepresentationKind::Png,
        RepresentationKind::Tiff,
        RepresentationKind::Pdf,
        RepresentationKind::Html,
        RepresentationKind::Rtf,
        RepresentationKind::Url,
        RepresentationKind::PlainText,
        RepresentationKind::Color,
    ];
    ORDER.into_iter().find_map(|kind| {
        representations
            .iter()
            .find(|representation| representation.kind == kind)
    })
}

fn export_extension(kind: &RepresentationKind) -> &'static str {
    match kind {
        RepresentationKind::Png => "png",
        RepresentationKind::Tiff => "tiff",
        RepresentationKind::Pdf => "pdf",
        RepresentationKind::Html => "html",
        RepresentationKind::Rtf => "rtf",
        RepresentationKind::Url | RepresentationKind::PlainText | RepresentationKind::Color => {
            "txt"
        }
        RepresentationKind::FileUrls | RepresentationKind::Custom(_) => "bin",
    }
}

pub(crate) fn file_url_paths(bytes: &[u8]) -> Result<Vec<PathBuf>, DragExportError> {
    if bytes.len() > MAX_FILE_LIST_BYTES {
        return Err(DragExportError::TooManyFiles);
    }
    // public.file-url is UTF-8; NSFilenamesPboardType is a property-list
    // array of absolute names. Preserve all files and their order in either.
    let paths = if bytes.starts_with(b"bplist")
        || bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'<')
    {
        let value = plist::Value::from_reader(Cursor::new(bytes))
            .map_err(|_| DragExportError::InvalidFileList)?;
        let values = value.as_array().ok_or(DragExportError::InvalidFileList)?;
        if values.len() > MAX_DRAG_FILES {
            return Err(DragExportError::TooManyFiles);
        }
        values
            .iter()
            .map(|value| {
                let path =
                    PathBuf::from(value.as_string().ok_or(DragExportError::InvalidFileList)?);
                if path.is_absolute() {
                    Ok(path)
                } else {
                    Err(DragExportError::InvalidFileList)
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let value = std::str::from_utf8(bytes).map_err(|_| DragExportError::InvalidFileList)?;
        value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .take(MAX_DRAG_FILES + 1)
            .map(|line| {
                url::Url::parse(line)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())
                    .ok_or(DragExportError::InvalidFileList)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    if paths.is_empty() {
        return Err(DragExportError::InvalidFileList);
    }
    if paths.len() > MAX_DRAG_FILES {
        return Err(DragExportError::TooManyFiles);
    }
    Ok(paths)
}

fn sanitize_file_stem(value: &str) -> String {
    let value = value
        .chars()
        .filter(|character| {
            character.is_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.')
        })
        .take(72)
        .scan(0, |bytes, character| {
            *bytes += character.len_utf8();
            (*bytes <= 200).then_some(character)
        })
        .collect::<String>();
    let value = value.trim_matches([' ', '.']);
    if value.is_empty() {
        "CopyRail Item".into()
    } else {
        value.into()
    }
}

fn cleanup_stale_exports(export_root: &Path) {
    let Ok(entries) = fs::read_dir(export_root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_stale_directory = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > DRAG_EXPORT_LIFETIME);
        if is_stale_directory {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[derive(Debug, Error)]
pub enum DragExportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Storage(#[from] paste_storage::StorageError),
    #[error("clipboard item has no representation that can be dragged as a file")]
    UnsupportedRepresentation,
    #[error("drag selection is empty")]
    Empty,
    #[error("拖放最多包含 200 项历史和 1000 个文件，文件列表不能超过 1 MiB。")]
    TooManyFiles,
    #[error("原文件列表格式无效，无法完整拖出所选内容。")]
    InvalidFileList,
    #[error("原文件已移动或删除，无法完整拖出所选内容。")]
    MissingOriginalFile,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_export_names_without_path_components() {
        assert_eq!(sanitize_file_stem("../../Launch / Notes"), "Launch  Notes");
        assert_eq!(sanitize_file_stem(" ... "), "CopyRail Item");
        assert!(sanitize_file_stem(&"𐐀".repeat(72)).len() <= 200);
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(
            file_url_paths(b"file:///tmp/CopyRail%20Export.png").expect("file URL"),
            vec![PathBuf::from("/tmp/CopyRail Export.png")]
        );
    }

    #[test]
    fn decodes_every_uri_and_both_property_list_encodings_in_order() {
        let expected = vec![
            PathBuf::from("/synthetic/a b.txt"),
            PathBuf::from("/synthetic/中文.txt"),
        ];
        assert_eq!(file_url_paths(b"# URI list\nfile:///synthetic/a%20b.txt\nfile:///synthetic/%E4%B8%AD%E6%96%87.txt\n").expect("all URIs"), expected);
        let value = plist::Value::Array(
            expected
                .iter()
                .map(|path| plist::Value::String(path.to_string_lossy().into_owned()))
                .collect(),
        );
        let mut xml = Vec::new();
        value.to_writer_xml(&mut xml).expect("XML fixture");
        let mut binary = Vec::new();
        value.to_writer_binary(&mut binary).expect("binary fixture");
        for bytes in [&xml, &binary] {
            assert_eq!(file_url_paths(bytes).expect("legacy filenames"), expected);
        }
    }

    #[test]
    fn malformed_remote_relative_empty_and_excessive_file_lists_are_rejected() {
        for bytes in [
            b"".as_slice(),
            b"https://example.com/file",
            b"file://foreign-host/path",
            b"relative/path",
            b"\xff\xfe",
            b"<plist><array><string>relative</string></array></plist>",
            b"file:///synthetic/valid\ninvalid second entry",
        ] {
            assert!(file_url_paths(bytes).is_err());
        }
        assert!(matches!(
            file_url_paths(&vec![b'x'; MAX_FILE_LIST_BYTES + 1]),
            Err(DragExportError::TooManyFiles)
        ));
        assert!(matches!(
            file_url_paths(
                "file:///synthetic/file\n"
                    .repeat(MAX_DRAG_FILES + 1)
                    .as_bytes()
            ),
            Err(DragExportError::TooManyFiles)
        ));
    }

    fn capture(
        store: &SqliteStore,
        representations: Vec<paste_domain::CapturedRepresentation>,
    ) -> ClipId {
        store
            .insert_capture(&paste_domain::CapturedItem {
                captured_at: chrono::Utc::now(),
                source: paste_domain::SourceApplication::unknown(),
                device: paste_domain::DeviceMetadata {
                    id: paste_domain::DeviceId::new(),
                    display_name: "Synthetic Mac".into(),
                },
                flags: paste_domain::CaptureFlags::default(),
                representations,
            })
            .expect("synthetic capture")
            .id
    }

    fn representation(
        kind: RepresentationKind,
        bytes: Vec<u8>,
    ) -> paste_domain::CapturedRepresentation {
        paste_domain::CapturedRepresentation {
            native_type: None,
            kind,
            bytes,
            mime_type: None,
            file_name: None,
        }
    }

    #[test]
    fn utf16_text_exports_as_readable_utf8_without_changing_the_stored_representation() {
        let directory = tempfile::tempdir().expect("isolated files");
        let store = SqliteStore::open_in_memory().expect("store");
        let text = "合成文本 🦀";
        let original = paste_domain::CapturedRepresentation {
            native_type: Some("public.utf16-external-plain-text".into()),
            bytes: text.encode_utf16().flat_map(u16::to_ne_bytes).collect(),
            ..paste_domain::CapturedRepresentation::plain_text("")
        };
        let clip = capture(&store, vec![original.clone()]);
        let paths = prepare_drag_files(&store, directory.path(), &[clip]).expect("export");
        assert_eq!(fs::read_to_string(&paths[0]).expect("UTF-8 text"), text);
        assert_eq!(
            store
                .load_clipboard_payload(clip)
                .expect("unchanged raw bytes"),
            vec![original]
        );
        let color = capture(
            &store,
            vec![paste_domain::CapturedRepresentation {
                native_type: Some("com.apple.cocoa.pasteboard.color".into()),
                ..representation(RepresentationKind::Color, b"bplist00\0\xff".to_vec())
            }],
        );
        assert!(matches!(
            prepare_drag_files(&store, directory.path(), &[color]),
            Err(DragExportError::UnsupportedRepresentation)
        ));
        assert!(paths[0].exists());
    }

    #[test]
    fn mixed_selection_exports_exact_bytes_and_all_original_files_without_moving_them() {
        let directory = tempfile::tempdir().expect("isolated files");
        let original_a = directory.path().join("original A.txt");
        let original_b = directory.path().join("中文.txt");
        fs::write(&original_a, b"original A").expect("fixture A");
        fs::write(&original_b, b"original B").expect("fixture B");
        let store = SqliteStore::open_in_memory().expect("synthetic store");
        let text = capture(
            &store,
            vec![paste_domain::CapturedRepresentation::plain_text(
                "synthetic text",
            )],
        );
        let png_bytes = include_bytes!("../generated-icons/32x32.png").to_vec();
        let png = capture(
            &store,
            vec![representation(RepresentationKind::Png, png_bytes.clone())],
        );
        let files = capture(
            &store,
            vec![representation(
                RepresentationKind::FileUrls,
                format!(
                    "{}\n{}\n{}",
                    url::Url::from_file_path(&original_a).expect("URL A"),
                    url::Url::from_file_path(&original_b).expect("URL B"),
                    url::Url::from_file_path(&original_a).expect("duplicate URL")
                )
                .into_bytes(),
            )],
        );
        let paths =
            prepare_drag_files(&store, &directory.path().join("cache"), &[text, files, png])
                .expect("real export preparation");
        assert_eq!(paths.len(), 4);
        assert_eq!(fs::read(&paths[0]).expect("text bytes"), b"synthetic text");
        assert_eq!(&paths[1..3], &[original_a.clone(), original_b.clone()]);
        assert_eq!(fs::read(&paths[3]).expect("PNG bytes"), png_bytes);
        assert_eq!(
            fs::read(original_a).expect("original still present"),
            b"original A"
        );
        assert_eq!(
            fs::read(original_b).expect("original still present"),
            b"original B"
        );
        assert!(paths[0].starts_with(directory.path().join("cache/drag-exports")));
    }

    #[test]
    fn a_failed_batch_removes_only_its_partial_exports_and_never_falls_back_to_text() {
        let directory = tempfile::tempdir().expect("isolated files");
        let store = SqliteStore::open_in_memory().expect("synthetic store");
        let text = capture(
            &store,
            vec![paste_domain::CapturedRepresentation::plain_text(
                "before failure",
            )],
        );
        let missing = capture(
            &store,
            vec![
                representation(
                    RepresentationKind::FileUrls,
                    url::Url::from_file_path(directory.path().join("missing.txt"))
                        .expect("missing URL")
                        .to_string()
                        .into_bytes(),
                ),
                paste_domain::CapturedRepresentation::plain_text("must not export this fallback"),
            ],
        );
        let good = prepare_drag_files(&store, directory.path(), &[text])
            .expect("prior successful session");
        assert!(matches!(
            prepare_drag_files(&store, directory.path(), &[text, missing]),
            Err(DragExportError::MissingOriginalFile)
        ));
        assert!(prepare_drag_files(&store, directory.path(), &[text, ClipId::new()]).is_err());
        assert_eq!(
            fs::read(&good[0]).expect("prior successful export retained"),
            b"before failure"
        );
        assert_eq!(
            fs::read_dir(directory.path().join("drag-exports"))
                .expect("session directories")
                .count(),
            1
        );
    }

    #[test]
    fn repeated_export_uses_distinct_sessions_and_invalid_selection_creates_no_files() {
        let directory = tempfile::tempdir().expect("isolated files");
        let store = SqliteStore::open_in_memory().expect("synthetic store");
        assert!(prepare_drag_files(&store, directory.path(), &[]).is_err());
        assert!(!directory.path().join("drag-exports").exists());
        let id = capture(
            &store,
            vec![paste_domain::CapturedRepresentation::plain_text(
                "same data",
            )],
        );
        let first = prepare_drag_files(&store, directory.path(), &[id]).expect("first export");
        let second = prepare_drag_files(&store, directory.path(), &[id]).expect("second export");
        assert_ne!(first, second);
        assert_eq!(
            fs::read(&first[0]).expect("first bytes"),
            fs::read(&second[0]).expect("second bytes")
        );
    }
}
