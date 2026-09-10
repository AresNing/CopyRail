//! Route AppKit pasteboard imports through the same bounded, offline decoder
//! used when opening history. This is not a clipboard-isolated test surface.
use objc2::{
    AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::AnyObject, sel,
};
use objc2_app_kit::{NSAlert, NSPasteboard, NSTextField, NSTextView};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSObjectProtocol, NSRange, NSRect, NSString,
    NSValue,
};
use std::cell::{Cell, RefCell};

#[derive(Default)]
pub struct ImportState {
    importing: Cell<bool>,
    status: RefCell<Option<Retained<NSTextField>>>,
}

define_class!(
    #[unsafe(super = NSTextView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ImportState]
    pub struct GuardedTextView;
    unsafe impl NSObjectProtocol for GuardedTextView {}
    impl GuardedTextView {
        #[unsafe(method_id(readablePasteboardTypes))]
        fn readable_types(&self) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&crate::editor_import::RICH_TYPES.iter().map(|value| NSString::from_str(value)).collect::<Vec<_>>())
        }

        #[unsafe(method_id(acceptableDragTypes))]
        fn acceptable_drag_types(&self) -> Retained<NSArray<NSString>> {
            self.readablePasteboardTypes()
        }

        #[unsafe(method(readSelectionFromPasteboard:))]
        fn read_selection(&self, pasteboard: &NSPasteboard) -> bool {
            self.import(pasteboard, None, false)
        }

        #[unsafe(method(readSelectionFromPasteboard:type:))]
        fn read_selection_typed(&self, pasteboard: &NSPasteboard, kind: &NSString) -> bool {
            self.import(pasteboard, Some(&kind.to_string()), false)
        }

        // Do not forward these actions to the superclass: AppKit's plain-text
        // action may otherwise bypass the rich-text readSelection hook.
        #[unsafe(method(paste:))]
        fn paste_content(&self, _sender: Option<&AnyObject>) {
            self.import(&NSPasteboard::generalPasteboard(), None, false);
        }

        #[unsafe(method(pasteAsRichText:))]
        fn paste_rich(&self, _sender: Option<&AnyObject>) {
            self.import(&NSPasteboard::generalPasteboard(), None, false);
        }

        #[unsafe(method(pasteAsPlainText:))]
        fn paste_plain(&self, _sender: Option<&AnyObject>) {
            self.import(&NSPasteboard::generalPasteboard(), None, true);
        }
    }
);

struct ImportGuard<'a>(&'a Cell<bool>);
impl Drop for ImportGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl GuardedTextView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        unsafe {
            msg_send![super(Self::alloc(mtm).set_ivars(ImportState::default())), initWithFrame: frame]
        }
    }

    pub fn set_status(&self, status: &NSTextField) {
        use objc2::Message;
        *self.ivars().status.borrow_mut() = Some(status.retain());
    }

    pub fn is_importing(&self) -> bool {
        self.ivars().importing.get()
    }

    fn show_status(&self, message: &str) {
        let status = self.ivars().status.borrow().clone();
        if let Some(status) = status {
            status.setStringValue(&NSString::from_str(message));
            status.setToolTip(Some(&NSString::from_str(message)));
        }
    }

    fn selection(&self) -> Result<Vec<NSRange>, String> {
        self.selectedRanges()
            .to_vec()
            .iter()
            .map(|value| value.get_range().ok_or("文本选择已失效。".into()))
            .collect()
    }

    fn import(&self, pasteboard: &NSPasteboard, requested: Option<&str>, plain: bool) -> bool {
        // HTML parsing and confirmation can run a nested main run loop. Keep
        // both the object and guard alive, then verify the draft again before
        // committing so another action cannot silently be overwritten.
        use objc2::Message;
        let _keep_alive = self.retain();
        if self.ivars().importing.replace(true) {
            return false;
        }
        let _guard = ImportGuard(&self.ivars().importing);
        match self.try_import(pasteboard, requested, plain) {
            Ok(warning) => {
                self.show_status(
                    warning
                        .as_deref()
                        .unwrap_or("内容已插入；⌘Z 可撤销，⌘S 保存。"),
                );
                true
            }
            Err(error) => {
                self.show_status(&error);
                false
            }
        }
    }

    fn try_import(
        &self,
        pasteboard: &NSPasteboard,
        requested: Option<&str>,
        plain: bool,
    ) -> Result<Option<String>, String> {
        if !self.isEditable()
            || (self.respondsToSelector(sel!(isWritingToolsActive)) && self.isWritingToolsActive())
        {
            return Err("当前正在保存或处理文本，请稍后导入。".into());
        }
        let storage = unsafe { self.textStorage() }.ok_or("文本存储不可用。")?;
        let original =
            NSAttributedString::initWithAttributedString(NSAttributedString::alloc(), &storage);
        let ranges = self.selection()?;
        let payload = crate::editor_import::snapshot(pasteboard, requested, plain)?;
        let document = crate::editor_import::decode_items(&payload, plain, |files| {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(&NSString::from_str(&format!(
                "将 {} 个本地文件嵌入文档？",
                files.len()
            )));
            let names = files
                .iter()
                .take(5)
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            alert.setInformativeText(&NSString::from_str(&format!(
                "确认后才会读取文件内容；保存后附件将随此记录存储和同步。总量最多 4 MiB。\n{names}"
            )));
            alert.addButtonWithTitle(&NSString::from_str("取消"));
            alert.addButtonWithTitle(&NSString::from_str("读取并插入"));
            alert.runModal() == 1001
        })?;
        let replacement =
            crate::editor_import::prepare_replacement(&original, &ranges, &document.text)?;
        if !self.isEditable()
            || !storage.isEqualToAttributedString(&original)
            || self.selection()? != ranges
        {
            return Err("导入期间文档或选择发生变化，未插入；请重试。".into());
        }
        let values = NSArray::from_retained_slice(
            &replacement
                .ranges
                .iter()
                .map(|range| NSValue::new(*range))
                .collect::<Vec<_>>(),
        );
        let strings = NSArray::from_retained_slice(
            &replacement
                .ranges
                .iter()
                .map(|_| document.text.string())
                .collect::<Vec<_>>(),
        );
        self.breakUndoCoalescing();
        if !self.shouldChangeTextInRanges_replacementStrings(&values, Some(&strings)) {
            return Err("文本控件未接受此次修改。".into());
        }
        // shouldChange/didChange is AppKit's text-editing contract: it registers
        // undo and notifies delegates. Mutate only after every item and the
        // full prospective output have passed validation.
        storage.beginEditing();
        for range in replacement.ranges.iter().rev() {
            storage.replaceCharactersInRange_withAttributedString(*range, &document.text);
        }
        storage.endEditing();
        self.didChangeText();
        self.setSelectedRanges(&NSArray::from_retained_slice(
            &replacement
                .cursors
                .iter()
                .map(|range| NSValue::new(*range))
                .collect::<Vec<_>>(),
        ));
        self.breakUndoCoalescing();
        Ok(document.warning)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_import_subclass_registers_required_entry_points_without_a_window() {
        use objc2::{ClassType, sel};
        let class = super::GuardedTextView::class();
        for selector in [
            sel!(readablePasteboardTypes),
            sel!(acceptableDragTypes),
            sel!(readSelectionFromPasteboard:),
            sel!(readSelectionFromPasteboard:type:),
            sel!(paste:),
            sel!(pasteAsPlainText:),
            sel!(pasteAsRichText:),
        ] {
            assert!(class.instance_method(selector).is_some(), "{selector}");
        }
    }
}
