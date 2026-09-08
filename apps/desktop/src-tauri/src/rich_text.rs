//! Detached rich-text import/export. Clipboard access is deliberately absent.
use objc2::{AnyThread, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAttributedStringAppKitDocumentFormats, NSAttributedStringDocumentFormats,
    NSAttributedStringKitAdditions, NSCharacterEncodingDocumentOption,
    NSDocumentTypeDocumentAttribute, NSHTMLTextDocumentType, NSTimeoutDocumentOption,
};
use objc2_foundation::{
    NSAttributedString, NSData, NSDictionary, NSNumber, NSRange, NSString, NSUTF8StringEncoding,
};
use paste_domain::{CapturedRepresentation, RepresentationKind};

pub const EDIT_LIMIT: usize = 4 * 1024 * 1024;

pub struct Document {
    pub text: Retained<NSAttributedString>,
    pub warning: Option<String>,
}

pub fn decode(representations: &[CapturedRepresentation]) -> Result<Document, String> {
    if representations
        .iter()
        .try_fold(0usize, |sum, item| sum.checked_add(item.bytes.len()))
        .is_none_or(|size| size > EDIT_LIMIT)
    {
        return Err("富文本编辑最多支持 4 MiB 内容。".into());
    }
    let rtfd = representations
        .iter()
        .find(|item| item.native_type.as_deref() == Some("com.apple.flat-rtfd"));
    let rtf = representations
        .iter()
        .find(|item| item.kind == RepresentationKind::Rtf);
    if let Some(item) = rtfd.or(rtf) {
        let bytes = NSData::with_bytes(&item.bytes);
        // The input is a byte buffer, never a URL or a file wrapper path.
        let text = unsafe {
            if rtfd.is_some() {
                NSAttributedString::initWithRTFD_documentAttributes(
                    NSAttributedString::alloc(),
                    &bytes,
                    None,
                )
            } else {
                NSAttributedString::initWithRTF_documentAttributes(
                    NSAttributedString::alloc(),
                    &bytes,
                    None,
                )
            }
        }
        .ok_or("无法读取原始富文本；未降级为纯文本，也未修改记录。")?;
        return Ok(Document {
            text,
            warning: None,
        });
    }
    if let Some(item) = representations
        .iter()
        .find(|item| item.kind == RepresentationKind::Html)
    {
        if objc2::MainThreadMarker::new().is_none() {
            return Err("HTML 编辑必须在主线程打开。".into());
        }
        let html = std::str::from_utf8(&item.bytes)
            .map_err(|_| "HTML 编码不是 UTF-8；原数据保持不变。")?;
        let (html, altered) = offline_html(html)?;
        let bytes = NSData::with_bytes(html.as_bytes());
        // offline_html returns a parsed allowlisted document: no executable
        // elements, resource attributes, URL-bearing CSS or external stylesheet.
        let encoding = NSNumber::new_usize(NSUTF8StringEncoding);
        let timeout = NSNumber::new_f64(3.0);
        let options = unsafe {
            NSDictionary::from_slices(
                &[NSCharacterEncodingDocumentOption, NSTimeoutDocumentOption],
                &[&*encoding as &AnyObject, &*timeout as &AnyObject],
            )
        };
        let text = unsafe {
            NSAttributedString::initWithHTML_options_documentAttributes(
                NSAttributedString::alloc(),
                &bytes,
                &options,
                None,
            )
        }
        .ok_or("无法读取 HTML 富文本；原数据保持不变。")?;
        return Ok(Document { text, warning: altered.then(|| "安全编辑已排除脚本、外部资源或不支持的样式；保存会使用当前显示的格式，取消可保留原内容。".into()) });
    }
    let text = representations
        .iter()
        .find_map(CapturedRepresentation::decoded_text)
        .ok_or("没有可编辑文本。")?;
    Ok(Document {
        text: NSAttributedString::initWithString(
            NSAttributedString::alloc(),
            &NSString::from_str(&text),
        ),
        warning: None,
    })
}

pub fn encode(text: &NSAttributedString) -> Result<Vec<CapturedRepresentation>, String> {
    let plain = text.string().to_string();
    if plain.trim().is_empty() || plain.len() > EDIT_LIMIT {
        return Err("内容不能为空或超过 4 MiB。".into());
    }
    let range = NSRange::new(0, text.length());
    let attrs = NSDictionary::new();
    let rtf = unsafe { text.RTFFromRange_documentAttributes(range, &attrs) }
        .ok_or("无法生成 RTF，原内容未修改。")?
        .to_vec();
    let html_attributes = unsafe {
        NSDictionary::from_slices(
            &[NSDocumentTypeDocumentAttribute],
            &[NSHTMLTextDocumentType as &AnyObject],
        )
    };
    let html = unsafe { text.dataFromRange_documentAttributes_error(range, &html_attributes) }
        .map_err(|_| "无法生成 HTML，原内容未修改。")?
        .to_vec();
    let mut result = vec![
        representation(
            RepresentationKind::PlainText,
            "public.utf8-plain-text",
            "text/plain; charset=utf-8",
            plain.into_bytes(),
        ),
        representation(RepresentationKind::Rtf, "public.rtf", "text/rtf", rtf),
        representation(RepresentationKind::Html, "public.html", "text/html", html),
    ];
    if text.containsAttachmentsInRange(range) {
        let rtfd = unsafe { text.RTFDFromRange_documentAttributes(range, &attrs) }
            .ok_or("无法保留富文本附件，原内容未修改。")?
            .to_vec();
        result.push(representation(
            RepresentationKind::Custom("com.apple.flat-rtfd".into()),
            "com.apple.flat-rtfd",
            "application/rtfd",
            rtfd,
        ));
    }
    if result.iter().map(|item| item.bytes.len()).sum::<usize>() > EDIT_LIMIT {
        return Err("编辑后的多格式内容超过 4 MiB，未保存。".into());
    }
    Ok(result)
}

fn representation(
    kind: RepresentationKind,
    native_type: &str,
    mime: &str,
    bytes: Vec<u8>,
) -> CapturedRepresentation {
    CapturedRepresentation {
        kind,
        native_type: Some(native_type.into()),
        mime_type: Some(mime.into()),
        file_name: None,
        bytes,
    }
}

/// Parsed allowlist, not a regex blacklist. CSS tokenization resolves escaped
/// identifiers so u\\72l(...) cannot bypass the no-resource policy.
fn offline_html(html: &str) -> Result<(String, bool), String> {
    if html.len() > EDIT_LIMIT {
        return Err("HTML 超过编辑限制。".into());
    }
    let document = dom_query::Document::from(html);
    let forbidden = document.select("script,iframe,frame,frameset,object,embed,link,base,meta,img,picture,source,video,audio,svg,math,template,form,input,button,textarea,select");
    let mut altered = !forbidden.nodes().is_empty();
    forbidden.remove();
    let nodes = document.select("*");
    if nodes.nodes().len() > 25_000 {
        return Err("HTML 元素过多，未打开编辑器。".into());
    }
    for node in nodes.nodes() {
        let name = node.node_name().unwrap_or_default();
        if !matches!(
            name.as_ref(),
            "html"
                | "head"
                | "body"
                | "title"
                | "style"
                | "p"
                | "div"
                | "span"
                | "b"
                | "strong"
                | "i"
                | "em"
                | "u"
                | "s"
                | "strike"
                | "del"
                | "sub"
                | "sup"
                | "small"
                | "big"
                | "font"
                | "br"
                | "hr"
                | "pre"
                | "code"
                | "blockquote"
                | "ul"
                | "ol"
                | "li"
                | "dl"
                | "dt"
                | "dd"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "table"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "th"
                | "td"
                | "caption"
                | "colgroup"
                | "col"
                | "a"
                | "abbr"
                | "acronym"
                | "bdo"
                | "bdi"
                | "q"
                | "cite"
                | "mark"
                | "ins"
        ) {
            node.remove_from_parent();
            altered = true;
            continue;
        }
        for attr in node.attrs() {
            let name = attr.name.local.as_ref();
            let value = attr.value.as_ref();
            let allowed = match name {
                "style" => safe_css(value),
                "href" => url::Url::parse(value)
                    .is_ok_and(|url| matches!(url.scheme(), "https" | "http" | "mailto")),
                "class" | "id" | "dir" | "lang" | "align" | "color" | "face" | "size" | "width"
                | "height" | "border" | "cellpadding" | "cellspacing" | "colspan" | "rowspan"
                | "start" | "type" => true,
                _ => false,
            };
            if !allowed {
                node.remove_attr(name);
                altered = true;
            }
        }
    }
    for style in document.select("style").nodes() {
        if !safe_css(&style.text()) {
            style.remove_from_parent();
            altered = true;
        }
    }
    Ok((document.html().to_string(), altered))
}

fn safe_css(css: &str) -> bool {
    use cssparser::{Parser, ParserInput, Token};
    fn tokens(parser: &mut Parser<'_, '_>, depth: usize) -> Result<(), ()> {
        if depth > 32 {
            return Err(());
        }
        loop {
            let token = match parser.next_including_whitespace_and_comments() {
                Ok(token) => token.clone(),
                Err(_) => break,
            };
            match token {
                Token::UnquotedUrl(_)
                | Token::AtKeyword(_)
                | Token::BadUrl(_)
                | Token::BadString(_) => return Err(()),
                Token::Function(name) => {
                    if !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "rgb" | "rgba" | "hsl" | "hsla" | "calc" | "min" | "max" | "clamp"
                    ) {
                        return Err(());
                    }
                    parser
                        .parse_nested_block(|inner| {
                            tokens(inner, depth + 1)
                                .map_err(|()| inner.new_custom_error::<(), ()>(()))
                        })
                        .map_err(|_| ())?;
                }
                Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                    parser
                        .parse_nested_block(|inner| {
                            tokens(inner, depth + 1)
                                .map_err(|()| inner.new_custom_error::<(), ()>(()))
                        })
                        .map_err(|_| ())?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    tokens(&mut Parser::new(&mut ParserInput::new(css)), 0).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_embedded_attachment_survives_flat_rtfd_export_and_reload() {
        use objc2_app_kit::{NSAttributedStringAttachmentConveniences, NSTextAttachment};
        let bytes = NSData::with_bytes(include_bytes!("../generated-icons/32x32.png"));
        let attachment = NSTextAttachment::initWithData_ofType(
            NSTextAttachment::alloc(),
            Some(&bytes),
            Some(&NSString::from_str("public.png")),
        );
        let document = NSAttributedString::attributedStringWithAttachment(&attachment);
        let payload = encode(&document).expect("export attachment");
        assert!(
            payload
                .iter()
                .any(|item| item.native_type.as_deref() == Some("com.apple.flat-rtfd"))
        );
        let reloaded = decode(&payload).expect("RTFD import");
        assert!(
            reloaded
                .text
                .containsAttachmentsInRange(NSRange::new(0, reloaded.text.length()))
        );
        assert_eq!(
            reloaded.text.string().to_string(),
            document.string().to_string()
        );
        let restored = unsafe {
            reloaded.text.attribute_atIndex_effectiveRange(
                objc2_app_kit::NSAttachmentAttributeName,
                0,
                std::ptr::null_mut(),
            )
        }
        .expect("attachment attribute")
        .downcast::<NSTextAttachment>()
        .expect("native attachment");
        let restored_bytes = restored
            .fileWrapper()
            .and_then(|wrapper| wrapper.regularFileContents())
            .or_else(|| restored.contents())
            .expect("embedded file bytes");
        assert_eq!(restored_bytes.to_vec(), bytes.to_vec());
    }

    #[test]
    fn native_rtf_edit_roundtrip_keeps_underline_and_all_exported_text_in_sync() {
        use objc2_app_kit::NSUnderlineStyleAttributeName;
        use objc2_foundation::NSMutableAttributedString;
        let original = representation(
            RepresentationKind::Rtf,
            "public.rtf",
            "text/rtf",
            br"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}\f0\fs24\ul Alpha\ul0  Beta}".to_vec(),
        );
        let document = decode(&[original]).expect("native RTF import");
        assert_eq!(document.text.string().to_string(), "Alpha Beta");
        assert!(document.warning.is_none());
        let underline = unsafe {
            document.text.attribute_atIndex_effectiveRange(
                NSUnderlineStyleAttributeName,
                0,
                std::ptr::null_mut(),
            )
        }
        .expect("underlined run");
        let edited = NSMutableAttributedString::initWithAttributedString(
            NSMutableAttributedString::alloc(),
            &document.text,
        );
        edited
            .replaceCharactersInRange_withString(NSRange::new(6, 4), &NSString::from_str("Gamma"));
        let payload = encode(&edited).expect("native export");
        assert_eq!(payload[0].decoded_text().expect("plain"), "Alpha Gamma");
        assert!(
            payload
                .iter()
                .any(|item| item.kind == RepresentationKind::Html)
        );
        let reloaded = decode(&payload).expect("RTF reload");
        assert_eq!(reloaded.text.string().to_string(), "Alpha Gamma");
        let restored = unsafe {
            reloaded.text.attribute_atIndex_effectiveRange(
                NSUnderlineStyleAttributeName,
                0,
                std::ptr::null_mut(),
            )
        }
        .expect("underline restored");
        assert_eq!(
            underline
                .downcast::<NSNumber>()
                .expect("numeric underline")
                .intValue(),
            restored
                .downcast::<NSNumber>()
                .expect("restored numeric underline")
                .intValue()
        );
        assert_eq!(document.text.string().to_string(), "Alpha Beta");
    }

    #[test]
    fn html_import_removes_network_active_content_and_escaped_css_urls() {
        let (clean, warning) = offline_html(r#"<head><link href="https://example.com/a.css"><style>.good {color:rgb(3,4,5);font-weight:bold}</style><style>@import 'https://example.com/b';</style></head><body onload="alert(1)"><b style="color:red">Hello</b><img src="file:///private/synthetic.png"><span style="background:u\72l(https://example.com)">World</span><a href="javascript:alert(1)">bad</a><a href="https://example.com">link</a></body>"#).expect("sanitize");
        assert!(warning);
        assert!(
            !clean.contains("onload")
                && !clean.contains("<img")
                && !clean.contains("<link")
                && !clean.contains("@import")
                && !clean.contains("javascript:")
                && !clean.contains("u\\72l")
        );
        assert!(
            clean.contains("font-weight:bold")
                && clean.contains("color:red")
                && clean.contains("https://example.com")
        );
        assert!(!safe_css(
            "color:red;background:image-set('https://example.com')"
        ));
        assert!(!safe_css(
            "background: u\\000072l('file:///private/synthetic')"
        ));
        assert!(safe_css(
            "font-family:'Times New Roman';font-size:18px;color:rgb(20,30,40)"
        ));
        let (clean, warning) = offline_html("<noscript><img src='https://example.com'></noscript><custom-element href='https://example.com'>unsafe context</custom-element><b>safe</b>").expect("unknown element handling");
        assert!(
            warning
                && !clean.contains("noscript")
                && !clean.contains("custom-element")
                && clean.contains("<b>safe</b>")
        );
    }
}
