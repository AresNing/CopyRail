//! A real AppKit editor hosted by a Tauri native (non-WebView) window.
use objc2::{
    DefinedClass, MainThreadOnly, Message, define_class, msg_send, rc::Retained,
    runtime::AnyObject, sel,
};
use objc2_app_kit::{
    NSAlert, NSAutoresizingMaskOptions as Resize, NSButton, NSEventModifierFlags, NSFontManager,
    NSScrollView, NSTextField, NSTextView, NSWindow, NSWritingToolsBehavior,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use paste_domain::ClipId;
use paste_storage::{RichTextEditSnapshot, SqliteStore};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::Arc,
};
use tauri::Manager;

thread_local! {
    static EDITORS: RefCell<BTreeMap<String, Retained<Editor>>> = const { RefCell::new(BTreeMap::new()) };
}

struct EditorState {
    window: tauri::Window,
    store: Arc<SqliteStore>,
    id: ClipId,
    expected_hash: [u8; 32],
    original: Retained<NSAttributedString>,
    text: Retained<crate::guarded_text_view::GuardedTextView>,
    status: Retained<NSTextField>,
    saving: Cell<bool>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = EditorState]
    struct Editor;
    unsafe impl NSObjectProtocol for Editor {}
    impl Editor {
        #[unsafe(method(save:))]
        fn save(&self, _sender: Option<&AnyObject>) {
            let _keep_alive = self.retain();
            let state = self.ivars();
            if state.saving.get() || state.text.is_importing() { return; }
            if writing_tools_active(&state.text) {
                state.status.setStringValue(&NSString::from_str(crate::locale::t("请等待 Writing Tools 完成后再保存。")));
                return;
            }
            let Some(storage) = (unsafe { state.text.textStorage() }) else { return; };
            if storage.isEqualToAttributedString(&state.original) {
                let _ = state.window.destroy();
                return;
            }
            let payload = match crate::rich_text::encode(&storage) {
                Ok(payload) => payload,
                Err(error) => { state.status.setStringValue(&NSString::from_str(&error)); return; }
            };
            state.saving.set(true);
            state.text.setEditable(false);
            state.status.setStringValue(&NSString::from_str(crate::locale::t("正在保存…")));
            let store = Arc::clone(&state.store);
            let window = state.window.clone();
            let id = state.id;
            let expected_hash = state.expected_hash;
            // SQLite may be busy syncing. Never block the AppKit event thread.
            tauri::async_runtime::spawn_blocking(move || {
                let result = store.update_rich_text_clip(id, expected_hash, &payload).map_err(|error| error.to_string());
                let callback_window = window.clone();
                let _ = window.run_on_main_thread(move || {
                    if let Err(error) = result {
                        with_editor(callback_window.label(), |editor| {
                            editor.ivars().saving.set(false);
                            editor.ivars().text.setEditable(true);
                            editor.ivars().status.setStringValue(&NSString::from_str(&error));
                        });
                    } else {
                        callback_window.app_handle().state::<crate::DesktopState>().cloud_sync.wake();
                        let _ = callback_window.destroy();
                    }
                });
            });
        }

        #[unsafe(method(cancel:))]
        fn cancel(&self, _sender: Option<&AnyObject>) { self.request_close(); }
    }
);

fn with_editor(label: &str, action: impl FnOnce(&Editor)) {
    // Clone before callbacks: destroying a window may re-enter cleanup.
    let editor = EDITORS.with(|editors| editors.borrow().get(label).cloned());
    if let Some(editor) = editor {
        action(&editor);
    }
}

impl Editor {
    fn request_close(&self) {
        let _keep_alive = self.retain();
        let state = self.ivars();
        if state.saving.get() || state.text.is_importing() || writing_tools_active(&state.text) {
            return;
        }
        if let Some(storage) = unsafe { state.text.textStorage() }
            && !storage.isEqualToAttributedString(&state.original)
        {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(&NSString::from_str(crate::locale::t(
                "放弃尚未保存的修改？",
            )));
            alert.setInformativeText(&NSString::from_str(crate::locale::t(
                "原来的剪贴板记录不会改变。",
            )));
            alert.addButtonWithTitle(&NSString::from_str(crate::locale::t("继续编辑")));
            alert.addButtonWithTitle(&NSString::from_str(crate::locale::t("放弃更改")));
            if alert.runModal() != 1001 {
                return;
            }
        }
        let _ = state.window.destroy();
    }
}

fn writing_tools_active(text: &NSTextView) -> bool {
    text.respondsToSelector(sel!(isWritingToolsActive)) && text.isWritingToolsActive()
}

/// Application quit must not silently discard any editor, including a stale
/// draft whose optimistic save was rejected by a background content update.
pub fn confirm_application_exit() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let editors = EDITORS.with(|editors| editors.borrow().values().cloned().collect::<Vec<_>>());
    if editors.iter().any(|editor| {
        editor.ivars().saving.get()
            || editor.ivars().text.is_importing()
            || writing_tools_active(&editor.ivars().text)
    }) {
        return false;
    }
    let dirty = editors.iter().any(|editor| {
        unsafe { editor.ivars().text.textStorage() }
            .is_some_and(|text| !text.isEqualToAttributedString(&editor.ivars().original))
    });
    if !dirty {
        return true;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(crate::locale::t(
        "有尚未保存的富文本修改",
    )));
    alert.setInformativeText(&NSString::from_str(crate::locale::t(
        "继续编辑可保留草稿；退出会放弃所有未保存修改，原记录保持不变。",
    )));
    alert.addButtonWithTitle(&NSString::from_str(crate::locale::t("继续编辑")));
    alert.addButtonWithTitle(&NSString::from_str(crate::locale::t("放弃并退出")));
    alert.runModal() == 1001
}

struct PendingWindow(Option<tauri::Window>);
impl Drop for PendingWindow {
    fn drop(&mut self) {
        if let Some(window) = &self.0 {
            let _ = window.destroy();
        }
    }
}

pub fn open(
    app: &tauri::AppHandle,
    store: Arc<SqliteStore>,
    snapshot: RichTextEditSnapshot,
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("原生编辑器必须在主线程打开。")?;
    let label = format!("rich-editor-{}", snapshot.item.id);
    if let Some(window) = app.get_window(&label) {
        window.show().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }
    if EDITORS.with(|editors| editors.borrow().len()) >= 8 {
        return Err(crate::locale::t("请先关闭一个编辑窗口（最多 8 个）。").into());
    }
    let document = crate::rich_text::decode(&snapshot.representations)?;
    let protected = store
        .load_desktop_preferences()
        .map_err(|error| error.to_string())?
        .screen_share_protection;
    let window = tauri::window::WindowBuilder::new(app, &label)
        .title(format!(
            "{} · {}",
            crate::locale::t("编辑"),
            snapshot.item.title
        ))
        .inner_size(720., 480.)
        .min_inner_size(640., 340.)
        .center()
        .visible(false)
        .content_protected(protected)
        .build()
        .map_err(|error| error.to_string())?;
    let mut pending = PendingWindow(Some(window.clone()));
    let native_ptr = window.ns_window().map_err(|error| error.to_string())?;
    // The Tauri window owns this NSWindow for the lifetime of the editor.
    let native = unsafe { &*native_ptr.cast::<NSWindow>() };
    let content = native.contentView().ok_or("原生窗口没有内容区域。")?;
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(16., 76., 688., 346.));
    scroll.setHasVerticalScroller(true);
    let text = crate::guarded_text_view::GuardedTextView::new(mtm, rect(0., 0., 688., 346.));
    text.setRichText(true);
    text.setImportsGraphics(true);
    text.setAllowsUndo(true);
    text.setUsesFontPanel(true);
    text.setHorizontallyResizable(false);
    text.setVerticallyResizable(true);
    text.setAutomaticQuoteSubstitutionEnabled(false);
    text.setAutomaticDashSubstitutionEnabled(false);
    text.setAutomaticSpellingCorrectionEnabled(false);
    // Availability-guarded: older macOS versions must never receive a new selector.
    if text.respondsToSelector(sel!(setWritingToolsBehavior:)) {
        text.setWritingToolsBehavior(NSWritingToolsBehavior::Complete);
    }
    unsafe {
        text.setAutoresizingMask(Resize::ViewWidthSizable);
        scroll.setAutoresizingMask(Resize::ViewWidthSizable | Resize::ViewHeightSizable);
        text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        if let Some(container) = text.textContainer() {
            container.setWidthTracksTextView(true);
            container.setContainerSize(NSSize::new(688., f64::MAX));
        }
        text.textStorage()
            .ok_or("文本存储不可用。")?
            .setAttributedString(&document.text);
        content.addSubview(&scroll);
    }
    scroll.setDocumentView(Some(&text));
    let status = NSTextField::labelWithString(
        &NSString::from_str(document.warning.as_deref().unwrap_or(crate::locale::t(
            "⌘S 保存 · Esc 取消；支持系统字体面板、撤销与右键文本操作。",
        ))),
        mtm,
    );
    status.setFrame(rect(16., 16., 688., 44.));
    status.setAutoresizingMask(Resize::ViewWidthSizable | Resize::ViewMaxYMargin);
    content.addSubview(&status);
    text.set_status(&status);
    let controller = unsafe {
        msg_send![
            super(Editor::alloc(mtm).set_ivars(EditorState {
                window: window.clone(),
                store,
                id: snapshot.item.id,
                expected_hash: snapshot.item.content_hash,
                original: document.text,
                text: text.clone(),
                status,
                saving: Cell::new(false),
            })),
            init
        ]
    };
    let controller: Retained<Editor> = controller;
    let fonts = NSFontManager::sharedFontManager(mtm);
    let controls = [
        (
            crate::locale::t("字体…"),
            &*fonts as &AnyObject,
            sel!(orderFrontFontPanel:),
            16.,
            80.,
            "",
            NSEventModifierFlags::empty(),
        ),
        (
            crate::locale::t("下划线"),
            &*text as &AnyObject,
            sel!(underline:),
            104.,
            80.,
            "u",
            NSEventModifierFlags::Command,
        ),
        (
            crate::locale::t("左对齐"),
            &*text as &AnyObject,
            sel!(alignLeft:),
            192.,
            80.,
            "",
            NSEventModifierFlags::empty(),
        ),
        (
            crate::locale::t("居中"),
            &*text as &AnyObject,
            sel!(alignCenter:),
            280.,
            64.,
            "",
            NSEventModifierFlags::empty(),
        ),
        (
            crate::locale::t("取消"),
            &*controller as &AnyObject,
            sel!(cancel:),
            512.,
            80.,
            "\u{1b}",
            NSEventModifierFlags::empty(),
        ),
        (
            crate::locale::t("保存"),
            &*controller as &AnyObject,
            sel!(save:),
            600.,
            104.,
            "s",
            NSEventModifierFlags::Command,
        ),
    ];
    for (title, target, action, x, width, key, modifiers) in controls {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(target),
                Some(action),
                mtm,
            )
        };
        button.setKeyEquivalent(&NSString::from_str(key));
        button.setKeyEquivalentModifierMask(modifiers);
        button.setFrame(rect(x, 434., width, 30.));
        button.setAutoresizingMask(
            Resize::ViewMinYMargin
                | if x >= 512. {
                    Resize::ViewMinXMargin
                } else {
                    Resize::ViewMaxXMargin
                },
        );
        content.addSubview(&button);
    }
    EDITORS.with(|editors| editors.borrow_mut().insert(label.clone(), controller));
    let callback = window.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let label = callback.label().to_owned();
            let _ = callback.run_on_main_thread(move || with_editor(&label, Editor::request_close));
        }
        tauri::WindowEvent::Destroyed => {
            let label = callback.label().to_owned();
            let _ = callback.run_on_main_thread(move || {
                EDITORS.with(|editors| editors.borrow_mut().remove(&label));
            });
        }
        _ => {}
    });
    native.makeFirstResponder(Some(&text));
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    let _ = pending.0.take();
    Ok(())
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
