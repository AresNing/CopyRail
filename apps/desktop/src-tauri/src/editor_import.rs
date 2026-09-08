//! One bounded import path for paste, drop and Services. No general clipboard
//! is obtained here; AppKit supplies the pasteboard for the user's operation.
use crate::rich_text::{Document, EDIT_LIMIT};
use objc2::{AnyThread, rc::Retained};
use objc2_app_kit::{NSAttributedStringAttachmentConveniences, NSPasteboard, NSTextAttachment};
use objc2_foundation::{
    NSArray, NSAttributedString, NSData, NSFileWrapper, NSMutableAttributedString, NSRange,
    NSString,
};
use paste_domain::{CapturedRepresentation, RepresentationKind};
use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::PathBuf};

const MAX_ITEMS: usize = 200;
pub const RICH_TYPES: &[&str] = &[
    "com.apple.flat-rtfd",
    "public.rtf",
    "NeXT Rich Text Format v1.0 pasteboard type",
    "public.html",
    "Apple HTML pasteboard type",
    "public.png",
    "public.tiff",
    "public.file-url",
    "NSFilenamesPboardType",
    "public.utf8-plain-text",
    "public.utf16-external-plain-text",
    "NSStringPboardType",
    "public.plain-text",
    "public.url",
];
const TEXT_TYPES: &[&str] = &[
    "public.utf8-plain-text",
    "public.utf16-external-plain-text",
    "NSStringPboardType",
    "public.plain-text",
    "public.url",
];

fn kind(uti: &str) -> Option<RepresentationKind> {
    Some(match uti {
        "com.apple.flat-rtfd" => RepresentationKind::Custom(uti.into()),
        "public.rtf" | "NeXT Rich Text Format v1.0 pasteboard type" => RepresentationKind::Rtf,
        "public.html" | "Apple HTML pasteboard type" => RepresentationKind::Html,
        "public.png" => RepresentationKind::Png,
        "public.tiff" => RepresentationKind::Tiff,
        "public.file-url" | "NSFilenamesPboardType" => RepresentationKind::FileUrls,
        "public.url" => RepresentationKind::Url,
        value if TEXT_TYPES.contains(&value) => RepresentationKind::PlainText,
        _ => return None,
    })
}

fn choose_type<'a>(
    types: &'a [String],
    requested: Option<&str>,
    plain: bool,
) -> Result<&'a str, String> {
    if types.len() > 32 {
        return Err("导入表示数量超过限制。".into());
    }
    if let Some(requested) = requested {
        return types
            .iter()
            .find(|value| value.as_str() == requested && kind(value).is_some())
            .map(String::as_str)
            .ok_or("不支持请求的导入类型。".into());
    }
    let candidates = if plain {
        TEXT_TYPES
            .iter()
            .chain(RICH_TYPES.iter())
            .collect::<Vec<_>>()
    } else {
        RICH_TYPES.iter().collect()
    };
    candidates
        .into_iter()
        .find_map(|wanted| {
            types
                .iter()
                .find(|value| value.as_str() == *wanted)
                .map(String::as_str)
        })
        .ok_or("此内容没有可安全导入的文本、图片或文件表示。".into())
}

fn copy_data(
    uti: &str,
    data: Option<Retained<NSData>>,
    budget: &mut usize,
) -> Result<CapturedRepresentation, String> {
    let data = data.ok_or("导入数据不可用。")?;
    if data.is_empty() || data.len() > *budget {
        return Err("导入内容为空或超过 4 MiB 总量限制。".into());
    }
    *budget -= data.len();
    Ok(CapturedRepresentation {
        kind: kind(uti).ok_or("不支持的导入类型。")?,
        native_type: Some(uti.into()),
        bytes: data.to_vec(),
        mime_type: None,
        file_name: None,
    })
}

fn native_types(types: &NSArray<NSString>) -> Result<Vec<String>, String> {
    if types.len() > 32 || types.iter().any(|value| value.length() > 256) {
        return Err("导入表示数量或类型名称超过限制。".into());
    }
    Ok(types.iter().map(|value| value.to_string()).collect())
}

pub fn snapshot(
    pasteboard: &NSPasteboard,
    requested: Option<&str>,
    plain: bool,
) -> Result<Vec<CapturedRepresentation>, String> {
    let change = pasteboard.changeCount();
    let mut budget = EDIT_LIMIT;
    let mut payload = Vec::new();
    if let Some(items) = pasteboard
        .pasteboardItems()
        .filter(|items| !items.is_empty())
    {
        if items.len() > MAX_ITEMS {
            return Err("一次最多导入 200 项。".into());
        }
        for item in items.to_vec() {
            let types = native_types(&item.types())?;
            let uti = choose_type(&types, requested, plain)?;
            payload.push(copy_data(
                uti,
                item.dataForType(&NSString::from_str(uti)),
                &mut budget,
            )?);
        }
    } else {
        let available = pasteboard.types().ok_or("导入数据不可用。")?;
        let types = native_types(&available)?;
        let uti = choose_type(&types, requested, plain)?;
        payload.push(copy_data(
            uti,
            pasteboard.dataForType(&NSString::from_str(uti)),
            &mut budget,
        )?);
    }
    if change != pasteboard.changeCount() {
        return Err("剪贴板在读取期间发生变化，请重试。".into());
    }
    Ok(payload)
}

pub fn decode_items(
    payload: &[CapturedRepresentation],
    plain: bool,
    approve_files: impl FnOnce(&[PathBuf]) -> bool,
) -> Result<Document, String> {
    let payload_size = payload
        .iter()
        .try_fold(0usize, |sum, item| sum.checked_add(item.bytes.len()))
        .filter(|size| *size <= EDIT_LIMIT)
        .ok_or("导入内容超过体积限制。")?;
    if payload.is_empty() || payload.len() > MAX_ITEMS {
        return Err("导入内容超过数量或体积限制。".into());
    }
    let mut files = Vec::new();
    for item in payload
        .iter()
        .filter(|item| item.kind == RepresentationKind::FileUrls)
    {
        files.extend(
            crate::drag_export::file_url_paths(&item.bytes).map_err(|_| "无效的本地文件列表。")?,
        );
    }
    if files.len() > MAX_ITEMS {
        return Err("一次最多插入 200 个文件。".into());
    }
    if plain && !files.is_empty() {
        return Err("文件无法作为纯文本导入。".into());
    }
    if !files.is_empty() && !approve_files(&files) {
        return Err("已取消文件导入，未读取文件内容。".into());
    }
    let combined = NSMutableAttributedString::new();
    let mut warning = None;
    let mut file_budget = EDIT_LIMIT - payload_size;
    for (index, item) in payload.iter().enumerate() {
        let document = match item.kind {
            RepresentationKind::FileUrls => {
                let paths = crate::drag_export::file_url_paths(&item.bytes)
                    .map_err(|_| "无效的本地文件列表。")?;
                let result = NSMutableAttributedString::new();
                for (file_index, path) in paths.iter().enumerate() {
                    let file = read_approved_file(path, &mut file_budget)?;
                    let wrapper = NSFileWrapper::initRegularFileWithContents(
                        NSFileWrapper::alloc(),
                        &NSData::with_bytes(&file),
                    );
                    wrapper.setPreferredFilename(Some(&NSString::from_str(
                        &path.file_name().ok_or("文件名无效。")?.to_string_lossy(),
                    )));
                    let attachment = NSTextAttachment::initWithFileWrapper(
                        NSTextAttachment::alloc(),
                        Some(&wrapper),
                    );
                    if file_index > 0 {
                        result.appendAttributedString(&plain_text("\n"));
                    }
                    result.appendAttributedString(
                        &NSAttributedString::attributedStringWithAttachment(&attachment),
                    );
                }
                Document {
                    text: NSAttributedString::initWithAttributedString(
                        NSAttributedString::alloc(),
                        &result,
                    ),
                    warning: None,
                }
            }
            RepresentationKind::Png | RepresentationKind::Tiff => {
                if plain {
                    return Err("图片没有可用的纯文本表示。".into());
                }
                // Decode with the existing image safety limits before AppKit
                // handles the original bytes; the thumbnail is not substituted.
                paste_platform::generate_image_thumbnail(std::slice::from_ref(item), 1, 1)
                    .map_err(|_| "图片损坏或超过安全解码限制。")?;
                let attachment = NSTextAttachment::initWithData_ofType(
                    NSTextAttachment::alloc(),
                    Some(&NSData::with_bytes(&item.bytes)),
                    Some(&NSString::from_str(
                        if item.kind == RepresentationKind::Png {
                            "public.png"
                        } else {
                            "public.tiff"
                        },
                    )),
                );
                Document {
                    text: NSAttributedString::attributedStringWithAttachment(&attachment),
                    warning: None,
                }
            }
            _ => crate::rich_text::decode(std::slice::from_ref(item))?,
        };
        if index > 0 {
            combined.appendAttributedString(&plain_text("\n"));
        }
        if plain {
            combined.appendAttributedString(&plain_text(&document.text.string().to_string()));
        } else {
            combined.appendAttributedString(&document.text);
        }
        if document.warning.is_some() {
            warning = document.warning;
        }
        if combined.length() > EDIT_LIMIT {
            return Err("解码后的内容超过编辑限制。".into());
        }
    }
    Ok(Document {
        text: NSAttributedString::initWithAttributedString(NSAttributedString::alloc(), &combined),
        warning,
    })
}

fn read_approved_file(path: &std::path::Path, budget: &mut usize) -> Result<Vec<u8>, String> {
    // No directories, terminal-component symlinks, devices or FIFOs; read races cannot
    // cause an unbounded allocation or make opening a special file block.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "无法读取已确认的文件（符号链接不支持）。")?;
    let metadata = file.metadata().map_err(|_| "无法检查文件。")?;
    if !metadata.is_file() || metadata.len() > *budget as u64 {
        return Err("只支持总量不超过 4 MiB 的普通文件。".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(*budget as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取文件失败。")?;
    if bytes.len() > *budget {
        return Err("文件在读取期间变大，导入已取消。".into());
    }
    *budget -= bytes.len();
    Ok(bytes)
}

pub fn plain_text(text: &str) -> Retained<NSAttributedString> {
    NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(text))
}

pub struct Replacement {
    pub ranges: Vec<NSRange>,
    pub cursors: Vec<NSRange>,
}

pub fn prepare_replacement(
    original: &NSAttributedString,
    ranges: &[NSRange],
    incoming: &NSAttributedString,
) -> Result<Replacement, String> {
    if ranges.is_empty() || ranges.len() > MAX_ITEMS {
        return Err("没有有效的文本选择。".into());
    }
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|range| range.location);
    let source = original.string();
    let mut previous_end = None;
    let mut cursors = Vec::new();
    let mut removed = 0usize;
    for (index, range) in ranges.iter().enumerate() {
        let end = range
            .location
            .checked_add(range.length)
            .filter(|end| *end <= original.length())
            .ok_or("文本选择已失效。")?;
        for boundary in [range.location, end] {
            if boundary > 0
                && boundary < source.length()
                && (0xd800..=0xdbff).contains(&source.characterAtIndex(boundary - 1))
                && (0xdc00..=0xdfff).contains(&source.characterAtIndex(boundary))
            {
                return Err("选择不能拆开 Unicode 字符。".into());
            }
        }
        if previous_end.is_some_and(|last| range.location < last)
            || (index > 0 && ranges[index - 1].location == range.location)
        {
            return Err("文本选择不能重叠或重复。".into());
        }
        previous_end = Some(end);
        removed = removed.checked_add(range.length).ok_or("文本长度溢出。")?;
        let inserted = incoming
            .length()
            .checked_mul(index + 1)
            .ok_or("导入内容太大。")?;
        let cursor = end
            .checked_sub(removed)
            .and_then(|value| value.checked_add(inserted))
            .ok_or("文本长度溢出。")?;
        cursors.push(NSRange::new(cursor, 0));
    }
    let final_length = incoming
        .length()
        .checked_mul(ranges.len())
        .and_then(|inserted| {
            original
                .length()
                .checked_sub(removed)
                .and_then(|current| current.checked_add(inserted))
        })
        .ok_or("导入内容太大。")?;
    if final_length > EDIT_LIMIT {
        return Err("合并后的内容超过编辑限制。".into());
    }
    let result = NSMutableAttributedString::initWithAttributedString(
        NSMutableAttributedString::alloc(),
        original,
    );
    for range in ranges.iter().rev() {
        result.replaceCharactersInRange_withAttributedString(*range, incoming);
    }
    // Includes RTF, HTML and embedded attachments, not just character count.
    crate::rich_text::encode(&result)?;
    Ok(Replacement { ranges, cursors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_app_kit::{NSPasteboardItem, NSUnderlineStyleAttributeName};
    use objc2_foundation::NSNumber;

    fn representation(uti: &str, bytes: &[u8]) -> CapturedRepresentation {
        CapturedRepresentation {
            kind: kind(uti).expect("test type"),
            native_type: Some(uti.into()),
            bytes: bytes.to_vec(),
            mime_type: None,
            file_name: None,
        }
    }

    fn rtf() -> CapturedRepresentation {
        representation(
            "public.rtf",
            br"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}\f0\fs24\ul Alpha\ul0  Beta}",
        )
    }

    fn underline(text: &NSAttributedString, index: usize) -> Option<i32> {
        unsafe {
            text.attribute_atIndex_effectiveRange(
                NSUnderlineStyleAttributeName,
                index,
                std::ptr::null_mut(),
            )
        }
        .and_then(|value| value.downcast::<NSNumber>().ok())
        .map(|value| value.intValue())
    }

    fn apply(
        original: &NSAttributedString,
        replacement: &Replacement,
        incoming: &NSAttributedString,
    ) -> Retained<NSMutableAttributedString> {
        let result = NSMutableAttributedString::initWithAttributedString(
            NSMutableAttributedString::alloc(),
            original,
        );
        for range in replacement.ranges.iter().rev() {
            result.replaceCharactersInRange_withAttributedString(*range, incoming);
        }
        result
    }

    #[test]
    fn native_item_type_negotiation_preserves_bytes_and_checks_budget() {
        // Detached item only: never create/read a general or named pasteboard.
        let item = NSPasteboardItem::new();
        let utf16 = [0xff, 0xfe, b'A', 0, 0x3d, 0xd8, 0x00, 0xde];
        assert!(item.setData_forType(
            &NSData::with_bytes(&utf16),
            &NSString::from_str("public.utf16-external-plain-text")
        ));
        assert!(item.setData_forType(
            &NSData::with_bytes(&rtf().bytes),
            &NSString::from_str("public.rtf")
        ));
        let types = item
            .types()
            .to_vec()
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            choose_type(&types, None, false).expect("rich type"),
            "public.rtf"
        );
        let selected = choose_type(&types, None, true).expect("plain type");
        assert_eq!(selected, "public.utf16-external-plain-text");
        let mut budget = utf16.len();
        let copied = copy_data(
            selected,
            item.dataForType(&NSString::from_str(selected)),
            &mut budget,
        )
        .expect("copy bounded native bytes");
        assert_eq!(copied.bytes, utf16);
        assert_eq!(copied.decoded_text().expect("UTF-16 text"), "A😀");
        assert_eq!(budget, 0);
        assert!(
            copy_data(
                selected,
                item.dataForType(&NSString::from_str(selected)),
                &mut budget
            )
            .is_err()
        );
        assert!(choose_type(&types, Some("public.png"), false).is_err());
        assert!(choose_type(&["com.example.unknown".into()], None, false).is_err());
        assert!(choose_type(&vec!["public.rtf".into(); 33], None, false).is_err());
        assert!(
            native_types(&NSArray::from_retained_slice(&[NSString::from_str(
                &"x".repeat(257)
            )]))
            .is_err()
        );
        assert!(copy_data("public.rtf", None, &mut budget).is_err());
    }

    #[test]
    fn mixed_items_keep_order_rich_runs_and_support_explicit_plain_text() {
        let payload = [
            rtf(),
            representation("NSStringPboardType", "中文😀".as_bytes()),
        ];
        let rich = decode_items(&payload, false, |_| panic!("no files")).expect("rich import");
        assert_eq!(rich.text.string().to_string(), "Alpha Beta\n中文😀");
        assert_eq!(underline(&rich.text, 0), Some(1));
        let plain = decode_items(&payload, true, |_| panic!("no files")).expect("plain import");
        assert_eq!(
            plain.text.string().to_string(),
            rich.text.string().to_string()
        );
        assert_eq!(underline(&plain.text, 0), None);
    }

    #[test]
    fn bad_later_item_empty_or_oversized_batch_is_rejected() {
        let source = plain_text("unchanged");
        let bad = representation("public.png", b"not an image");
        assert!(decode_items(&[rtf(), bad], false, |_| panic!("no files")).is_err());
        assert!(decode_items(&[], false, |_| false).is_err());
        assert!(decode_items(&vec![rtf(); MAX_ITEMS + 1], false, |_| false).is_err());
        let large = representation("public.utf8-plain-text", &vec![b'x'; EDIT_LIMIT + 1]);
        assert!(decode_items(&[large], false, |_| false).is_err());
        assert_eq!(source.string().to_string(), "unchanged");
    }

    #[test]
    fn multiple_unicode_selections_are_planned_atomically_with_correct_cursors() {
        let source = plain_text("A😀B 中文 C");
        let incoming = decode_items(&[rtf()], false, |_| false)
            .expect("RTF import")
            .text;
        // Input order need not be ascending; offsets are UTF-16, not UTF-8.
        let replacement = prepare_replacement(
            &source,
            &[NSRange::new(5, 2), NSRange::new(1, 2)],
            &incoming,
        )
        .expect("valid Unicode replacement");
        assert_eq!(
            replacement.cursors,
            vec![NSRange::new(11, 0), NSRange::new(23, 0)]
        );
        let result = apply(&source, &replacement, &incoming);
        assert_eq!(result.string().to_string(), "AAlpha BetaB Alpha Beta C");
        assert_eq!(underline(&result, 1), Some(1));
        assert_eq!(underline(&result, 13), Some(1));
        assert_eq!(source.string().to_string(), "A😀B 中文 C");
    }

    #[test]
    fn invalid_overlapping_and_split_surrogate_selections_are_rejected() {
        let source = plain_text("A😀B");
        let incoming = plain_text("X");
        for ranges in [
            vec![],
            vec![NSRange::new(2, 0)],
            vec![NSRange::new(1, 1)],
            vec![NSRange::new(10, 0)],
            vec![NSRange::new(usize::MAX, 1)],
            vec![NSRange::new(0, 3), NSRange::new(1, 0)],
            vec![NSRange::new(0, 0); 2],
            vec![NSRange::new(0, 0); MAX_ITEMS + 1],
        ] {
            assert!(
                prepare_replacement(&source, &ranges, &incoming).is_err(),
                "{ranges:?}"
            );
        }
        let replacement =
            prepare_replacement(&source, &[NSRange::new(4, 0)], &incoming).expect("append at end");
        assert_eq!(
            apply(&source, &replacement, &incoming).string().to_string(),
            "A😀BX"
        );
    }

    #[test]
    fn replacement_checks_encoded_total_not_only_incoming_character_count() {
        let source = plain_text("unchanged");
        let incoming = plain_text(&"x".repeat(EDIT_LIMIT / 2));
        assert!(prepare_replacement(&source, &[NSRange::new(0, 9)], &incoming).is_err());
        assert_eq!(source.string().to_string(), "unchanged");
    }

    #[test]
    fn files_require_consent_before_reading_and_plain_mode_never_asks() {
        let directory = tempfile::tempdir().expect("isolated directory");
        let missing = directory.path().join("not-created.txt");
        let uri = url::Url::from_file_path(&missing)
            .expect("local URI")
            .to_string();
        let payload = [representation("public.file-url", uri.as_bytes())];
        let mut requested = false;
        let error = decode_items(&payload, false, |paths| {
            requested = true;
            assert_eq!(paths, &[missing]);
            false
        })
        .err()
        .expect("cancellation error before file access");
        assert!(requested);
        assert!(error.contains("已取消")); // would be read failure if opened first
        assert!(
            decode_items(&payload, true, |_| panic!(
                "plain files must fail before consent"
            ))
            .is_err()
        );
    }

    #[test]
    fn confirmed_file_reads_reject_symlinks_directories_and_excess_bytes() {
        use std::io::Write;
        let directory = tempfile::tempdir().expect("isolated directory");
        let mut file = tempfile::NamedTempFile::new_in(directory.path()).expect("isolated file");
        file.write_all(b"synthetic").expect("write fixture");
        assert_eq!(
            read_approved_file(file.path(), &mut 9).expect("read approved fixture"),
            b"synthetic"
        );
        assert!(read_approved_file(file.path(), &mut 8).is_err());
        let mut budget = EDIT_LIMIT;
        assert!(read_approved_file(directory.path(), &mut budget).is_err());
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(file.path(), &link).expect("fixture symlink");
        assert!(read_approved_file(&link, &mut budget).is_err());
    }

    #[test]
    fn image_and_confirmed_file_attachments_keep_original_bytes_after_export() {
        use std::io::Write;
        let mut file = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .expect("isolated attachment");
        file.write_all(b"synthetic file bytes")
            .expect("write fixture");
        let uri = url::Url::from_file_path(file.path())
            .expect("local URI")
            .to_string();
        let png = include_bytes!("../generated-icons/32x32.png");
        let payload = [
            representation("public.png", png),
            representation("public.file-url", uri.as_bytes()),
        ];
        let imported = decode_items(&payload, false, |_| true).expect("approved import");
        let reloaded = crate::rich_text::decode(
            &crate::rich_text::encode(&imported.text).expect("export attachments"),
        )
        .expect("reload attachments");
        for (index, expected) in [(0, png.as_slice()), (2, b"synthetic file bytes".as_slice())] {
            let attachment = unsafe {
                reloaded.text.attribute_atIndex_effectiveRange(
                    objc2_app_kit::NSAttachmentAttributeName,
                    index,
                    std::ptr::null_mut(),
                )
            }
            .expect("attachment attribute")
            .downcast::<NSTextAttachment>()
            .expect("native attachment");
            let bytes = attachment
                .fileWrapper()
                .and_then(|wrapper| wrapper.regularFileContents())
                .or_else(|| attachment.contents())
                .expect("embedded file bytes");
            assert_eq!(bytes.to_vec(), expected);
        }
    }
}
