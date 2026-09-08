//! A real AppKit popup, returning an exact choice after native tracking ends.
//! It never writes a clipboard or database and never installs global shortcuts.
use crate::context_action::ContextAction as Action;
use paste_domain::{ContentKind, RepresentationKind};
use paste_storage::ClipActionContext;

#[derive(Debug)]
enum Entry {
    Item {
        label: String,
        action: Action,
        enabled: bool,
    },
    Submenu {
        label: String,
        children: Vec<Entry>,
    },
    Separator,
}

fn item(label: &str, action: Action, enabled: bool) -> Entry {
    Entry::Item {
        label: label.into(),
        action,
        enabled,
    }
}

fn plan(context: &ClipActionContext, isolated: bool) -> Vec<Entry> {
    let single = context.clips.len() == 1;
    let writable = context.clips.iter().all(|item| item.writable);
    let editable = single && context.clips.first().is_some_and(|item| {
        let rich = matches!(item.clip.content_kind, ContentKind::RichText | ContentKind::Html)
            || item.clip.representations.iter().any(|r| matches!(r.kind, RepresentationKind::Rtf | RepresentationKind::Html)
                || r.native_type.as_deref() == Some("com.apple.flat-rtfd")
                || matches!(&r.kind, RepresentationKind::Custom(kind) if kind == "com.apple.flat-rtfd"));
        item.writable && if rich { !isolated } else {
            matches!(item.clip.content_kind, ContentKind::Text | ContentKind::Link | ContentKind::Color | ContentKind::Image)
        }
    });
    let mut entries = vec![
        item("粘贴", Action::Paste, !isolated),
        item("粘贴为纯文本", Action::PastePlain, !isolated),
        item("复制", Action::Copy, !isolated),
        item("复制为纯文本", Action::CopyPlain, !isolated),
        Entry::Separator,
        item("快速预览", Action::Preview, single),
        item("编辑", Action::Edit, editable),
        item("重命名", Action::Rename, single && writable),
        item("加入或移出 顺序粘贴", Action::ToggleStack, true),
        Entry::Separator,
    ];
    entries.push(Entry::Submenu {
        label: "Pin 到…".into(),
        children: context
            .boards
            .iter()
            .map(|board| {
                item(
                    &board.name,
                    Action::Pin(board.id),
                    board.writable
                        && writable
                        && context
                            .clips
                            .iter()
                            .all(|clip| !clip.move_restricted || clip.boards.contains(&board.id)),
                )
            })
            .collect(),
    });
    let sources = context
        .boards
        .iter()
        .filter(|board| {
            context
                .clips
                .iter()
                .any(|clip| clip.boards.contains(&board.id))
        })
        .map(|board| item(&board.name, Action::Unpin(board.id), board.writable))
        .collect::<Vec<_>>();
    if !sources.is_empty() {
        entries.push(Entry::Submenu {
            label: "移出 Pinboard…".into(),
            children: sources,
        });
    }
    entries.extend([
        item("在剪贴板历史中显示", Action::Locate, single),
        Entry::Separator,
        item(
            if single {
                "删除"
            } else {
                "删除选中内容"
            },
            Action::Delete,
            writable,
        ),
    ]);
    entries
}

fn menu_point(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    flipped: bool,
) -> Result<(f64, f64), String> {
    if ![x, y, width, height].into_iter().all(f64::is_finite)
        || width <= 0.0
        || height <= 0.0
        || x < 0.0
        || y < 0.0
        || x > width
        || y > height
    {
        return Err("菜单位置已失效，请重新打开。".into());
    }
    Ok((x, if flipped { y } else { height - y }))
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, rc::Retained, sel};
    use objc2_app_kit::{NSApplication, NSEvent, NSMenu, NSMenuItem, NSView};
    use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSString};
    use std::cell::Cell;

    thread_local! { static OPEN: Cell<bool> = const { Cell::new(false) }; }
    struct OpenGuard;
    impl Drop for OpenGuard {
        fn drop(&mut self) {
            OPEN.with(|open| open.set(false));
        }
    }

    #[derive(Default)]
    struct Choice {
        index: Cell<Option<usize>>,
    }
    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = Choice]
        struct ClipMenuTarget;
        unsafe impl NSObjectProtocol for ClipMenuTarget {}
        impl ClipMenuTarget {
            #[unsafe(method(choose:))]
            fn choose(&self, item: &NSMenuItem) {
                if item.isEnabled() {
                    self.ivars().index.set(usize::try_from(item.tag()).ok());
                }
            }
        }
    );

    fn build(
        entries: &[Entry],
        target: &ClipMenuTarget,
        actions: &mut Vec<Action>,
        mtm: MainThreadMarker,
    ) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        for entry in entries {
            let menu_item = match entry {
                Entry::Separator => NSMenuItem::separatorItem(mtm),
                Entry::Item {
                    label,
                    action,
                    enabled,
                } => {
                    // Selector is implemented above; target is retained for the
                    // complete synchronous native menu-tracking lifetime.
                    let entry = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(label),
                            Some(sel!(choose:)),
                            &NSString::new(),
                        )
                    };
                    entry.setEnabled(*enabled);
                    entry.setTag(actions.len() as isize);
                    unsafe {
                        entry.setTarget(Some(target));
                    }
                    actions.push(*action);
                    entry
                }
                Entry::Submenu { label, children } => {
                    let entry = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(label),
                            None,
                            &NSString::new(),
                        )
                    };
                    entry.setEnabled(!children.is_empty());
                    entry.setSubmenu(Some(&build(children, target, actions, mtm)));
                    entry
                }
            };
            menu.addItem(&menu_item);
        }
        menu
    }

    pub fn popup(
        window: &tauri::WebviewWindow,
        context: &ClipActionContext,
        isolated: bool,
        x: f64,
        y: f64,
    ) -> Result<Option<Action>, String> {
        let mtm = MainThreadMarker::new().ok_or("原生菜单必须在主线程打开。")?;
        if OPEN.with(|open| open.replace(true)) {
            return Err("已有菜单正在显示。".into());
        }
        let _guard = OpenGuard;
        let pointer = window.ns_view().map_err(|error| error.to_string())?;
        // Tauri owns this view; the WebviewWindow remains alive throughout popup.
        let view = unsafe { (pointer as *const NSView).as_ref() }
            .ok_or("窗口已关闭。")?
            .retain();
        let bounds = view.bounds();
        let (x, y) = menu_point(
            x,
            y,
            bounds.size.width,
            bounds.size.height,
            view.isFlipped(),
        )?;
        let target: Retained<ClipMenuTarget> = unsafe {
            msg_send![
                super(ClipMenuTarget::alloc(mtm).set_ivars(Choice::default())),
                init
            ]
        };
        let mut actions = Vec::new();
        let menu = build(&plan(context, isolated), &target, &mut actions, mtm);
        let started = std::time::Instant::now();
        if isolated {
            eprintln!(
                "Native UI test menu tracking begin: x={x} y={y} buttons={} event={:?} focused={:?}",
                NSEvent::pressedMouseButtons(),
                NSApplication::sharedApplication(mtm)
                    .currentEvent()
                    .map(|event| event.r#type()),
                window.is_focused()
            );
        }
        menu.popUpMenuPositioningItem_atLocation_inView(
            None,
            NSPoint::new(bounds.origin.x + x, bounds.origin.y + y),
            Some(&view),
        );
        if isolated {
            eprintln!(
                "Native UI test menu tracking end: elapsed_ms={} buttons={}",
                started.elapsed().as_millis(),
                NSEvent::pressedMouseButtons()
            );
        }
        Ok(target
            .ivars()
            .index
            .get()
            .and_then(|index| actions.get(index).copied()))
    }
}

#[cfg(target_os = "macos")]
pub use mac::popup;

#[cfg(not(target_os = "macos"))]
pub fn popup(
    _: &tauri::WebviewWindow,
    _: &ClipActionContext,
    _: bool,
    _: f64,
    _: f64,
) -> Result<Option<Action>, String> {
    Err("原生菜单当前仅支持 macOS。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paste_domain::{ClipId, SourceApplication};
    use paste_storage::SqliteStore;
    fn context() -> ClipActionContext {
        let store = SqliteStore::open_in_memory().expect("store");
        let device = store.get_or_create_device("Synthetic").expect("device");
        let board = store
            .create_pinboard("Synthetic Work", "#ff9500")
            .expect("board");
        let clip = store
            .create_textual_item_in_pinboard(
                ContentKind::Text,
                "Synthetic",
                SourceApplication::unknown(),
                device,
                Some(board.id),
            )
            .expect("clip");
        store.clip_action_context(&[clip.id]).expect("context")
    }
    fn enabled(entries: &[Entry], action: Action) -> Option<bool> {
        entries.iter().find_map(|entry| match entry {
            Entry::Item {
                action: current,
                enabled,
                ..
            } if *current == action => Some(*enabled),
            Entry::Submenu { children, .. } => enabled(children, action),
            _ => None,
        })
    }
    #[test]
    fn isolated_menu_disables_clipboard_actions_but_allows_local_text_edit() {
        let context = context();
        let entries = plan(&context, true);
        for action in [
            Action::Copy,
            Action::CopyPlain,
            Action::Paste,
            Action::PastePlain,
        ] {
            assert_eq!(enabled(&entries, action), Some(false));
        }
        for action in [Action::Edit, Action::Rename, Action::Preview] {
            assert_eq!(enabled(&entries, action), Some(true));
        }
        assert_eq!(enabled(&plan(&context, false), Action::Copy), Some(true));
    }
    #[test]
    fn multi_selection_and_read_only_members_disable_unsafe_actions() {
        let mut context = context();
        let mut another = context.clips[0].clip.clone();
        another.id = ClipId::new();
        context.clips.push(paste_storage::ClipActionItem {
            clip: another,
            boards: vec![],
            writable: false,
            move_restricted: false,
        });
        let entries = plan(&context, false);
        for action in [
            Action::Edit,
            Action::Rename,
            Action::Preview,
            Action::Locate,
            Action::Delete,
            Action::Pin(context.boards[0].id),
        ] {
            assert_eq!(enabled(&entries, action), Some(false));
        }
        assert_eq!(enabled(&entries, Action::Copy), Some(true));
    }

    #[test]
    fn rich_representations_on_text_cards_keep_the_isolation_boundary() {
        let mut context = context();
        context.clips[0].clip.representations[0].native_type = Some("com.apple.flat-rtfd".into());
        assert_eq!(enabled(&plan(&context, true), Action::Edit), Some(false));
        assert_eq!(enabled(&plan(&context, true), Action::Rename), Some(true));
        assert_eq!(enabled(&plan(&context, false), Action::Edit), Some(true));
    }
    #[test]
    fn shared_cross_scope_and_read_only_destination_are_disabled() {
        let mut context = context();
        context.clips[0].move_restricted = true;
        context.clips[0].boards.clear();
        assert_eq!(
            enabled(&plan(&context, false), Action::Pin(context.boards[0].id)),
            Some(false)
        );
        context.clips[0].move_restricted = false;
        context.boards[0].writable = false;
        assert_eq!(
            enabled(&plan(&context, false), Action::Pin(context.boards[0].id)),
            Some(false)
        );
    }
    #[test]
    fn popup_positions_account_for_view_orientation_and_reject_invalid_points() {
        assert_eq!(
            menu_point(12.0, 20.0, 1440.0, 248.0, false),
            Ok((12.0, 228.0))
        );
        assert_eq!(
            menu_point(12.0, 20.0, 1440.0, 248.0, true),
            Ok((12.0, 20.0))
        );
        for x in [-1.0, 1441.0, f64::NAN, f64::INFINITY] {
            assert!(menu_point(x, 20.0, 1440.0, 248.0, false).is_err());
        }
    }
}
