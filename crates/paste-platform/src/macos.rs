use std::{sync::Mutex, time::Instant};

use crate::{
    ClipboardWriteMode,
    paste_target::{Invocation, PasteAttempt, PasteEnvironment, PasteProgress, TargetSession},
    read_session::{PasteboardAccess, ReadCursor, SnapshotData},
    write_plan::{NativeRepresentation, plan_write},
};
use core_graphics::{
    event::{CGEvent, CGEventFlags, KeyCode},
    event_source::{CGEventSource, CGEventSourceStateID},
};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSPasteboard, NSPasteboardItem,
    NSPasteboardWriting, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSObjectProtocol, NSString};
use paste_domain::{CapturedRepresentation, DeviceMetadata, SourceApplication};

use crate::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
    INTERNAL_PASTEBOARD_TYPE,
};

pub struct MacClipboardWriter;

#[derive(Default)]
pub struct MacPasteTarget {
    session: Mutex<TargetSession<Retained<NSRunningApplication>>>,
}

#[derive(Clone)]
pub struct MacPasteInvocation(Invocation<Retained<NSRunningApplication>>);

pub struct MacPasteAttempt(PasteAttempt<Retained<NSRunningApplication>>);

impl MacPasteTarget {
    #[must_use]
    pub fn accessibility_permission_granted() -> bool {
        macos_accessibility_client::accessibility::application_is_trusted()
    }

    #[must_use]
    pub fn request_accessibility_permission() -> bool {
        macos_accessibility_client::accessibility::application_is_trusted_with_prompt()
    }

    pub fn remember_frontmost_application(&self) -> Result<bool, ClipboardError> {
        let application = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .filter(|application| {
                !application.isTerminated()
                    && application.processIdentifier() > 0
                    && !is_current_application(application)
            });
        let found = application.is_some();
        // Even an unknown/self invocation replaces the previous target. Never
        // carry a target from an older invocation into a new menu/reopen flow.
        self.session
            .lock()
            .map_err(|_| ClipboardError::PasteTargetPoisoned)?
            .replace(application);
        Ok(found)
    }

    pub fn invalidate(&self) -> Result<(), ClipboardError> {
        self.session
            .lock()
            .map_err(|_| ClipboardError::PasteTargetPoisoned)?
            .replace(None);
        Ok(())
    }

    pub fn snapshot(&self) -> Result<MacPasteInvocation, ClipboardError> {
        Ok(MacPasteInvocation(
            self.session
                .lock()
                .map_err(|_| ClipboardError::PasteTargetPoisoned)?
                .snapshot(),
        ))
    }

    pub fn is_current(&self, invocation: &MacPasteInvocation) -> Result<bool, ClipboardError> {
        Ok(self
            .session
            .lock()
            .map_err(|_| ClipboardError::PasteTargetPoisoned)?
            .is_current(&invocation.0))
    }

    pub fn advance(
        &self,
        attempt: &mut MacPasteAttempt,
        deadline: Instant,
    ) -> Result<PasteProgress, ClipboardError> {
        objc2::MainThreadMarker::new().ok_or(ClipboardError::PasteRequiresMainThread)?;
        let session = self
            .session
            .lock()
            .map_err(|_| ClipboardError::PasteTargetPoisoned)?;
        let mut environment = MacPasteEnvironment {
            expected_clipboard_count: attempt.0.clipboard_change_count(),
            deadline,
        };
        attempt
            .0
            .advance(&session, &mut environment, Instant::now() >= deadline)
    }

    #[must_use]
    pub fn owns_foreground() -> bool {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .is_some_and(|application| is_current_application(&application))
    }
}

impl MacPasteInvocation {
    #[must_use]
    pub fn has_target(&self) -> bool {
        self.0.target.is_some()
    }

    #[must_use]
    pub fn begin(self, clipboard_change_count: i64) -> MacPasteAttempt {
        MacPasteAttempt(PasteAttempt::new(self.0, clipboard_change_count))
    }
}

fn is_current_application(application: &NSRunningApplication) -> bool {
    application.processIdentifier() == i32::try_from(std::process::id()).unwrap_or(i32::MAX)
}

struct MacPasteEnvironment {
    expected_clipboard_count: i64,
    deadline: Instant,
}
impl PasteEnvironment<Retained<NSRunningApplication>> for MacPasteEnvironment {
    fn trusted(&self) -> bool {
        MacPasteTarget::accessibility_permission_granted()
    }
    fn clipboard_change_count(&self) -> i64 {
        NSPasteboard::generalPasteboard().changeCount() as i64
    }
    fn is_live(&self, target: &Retained<NSRunningApplication>) -> bool {
        !target.isTerminated()
            && target.processIdentifier() > 0
            && NSRunningApplication::runningApplicationWithProcessIdentifier(
                target.processIdentifier(),
            )
            // NSObject equality compares the running *instance*, not its pid.
            .is_some_and(|current| &current == target && !current.isTerminated())
    }
    fn frontmost(&self) -> Option<Retained<NSRunningApplication>> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .filter(|app| app.isActive())
    }
    fn is_self(&self, application: &Retained<NSRunningApplication>) -> bool {
        is_current_application(application)
    }
    fn activate(&mut self, target: &Retained<NSRunningApplication>) -> bool {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return false;
        };
        let app = NSApplication::sharedApplication(mtm);
        // macOS 14+ cooperative activation: explicitly hand off from this
        // active app before asking the remembered target to activate. Do not
        // use deprecated IgnoreOtherApps or activate every target window.
        if app.respondsToSelector(objc2::sel!(yieldActivationToApplication:))
            && target.respondsToSelector(objc2::sel!(activateFromApplication:options:))
        {
            app.yieldActivationToApplication(target);
            return target.activateFromApplication_options(
                &NSRunningApplication::currentApplication(),
                NSApplicationActivationOptions::empty(),
            );
        }
        target.activateWithOptions(NSApplicationActivationOptions::empty())
    }
    fn post(&mut self, target: &Retained<NSRunningApplication>) -> Result<(), ClipboardError> {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|()| ClipboardError::EventSourceUnavailable)?;
        let key_down = CGEvent::new_keyboard_event(source.clone(), KeyCode::ANSI_V, true)
            .map_err(|()| ClipboardError::EventCreationFailed)?;
        let key_up = CGEvent::new_keyboard_event(source, KeyCode::ANSI_V, false)
            .map_err(|()| ClipboardError::EventCreationFailed)?;
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);
        // Allocation above can fail or take time. Revalidate immediately
        // before submitting the pair; never emit just a keyDown on failure.
        if Instant::now() >= self.deadline
            || !self.trusted()
            || !self.is_live(target)
            || self.frontmost().as_ref() != Some(target)
            || self.clipboard_change_count() != self.expected_clipboard_count
        {
            return Err(ClipboardError::PasteContextChanged);
        }
        let process_id = target.processIdentifier();
        key_down.post_to_pid(process_id);
        key_up.post_to_pid(process_id);
        Ok(())
    }
}

impl MacClipboardWriter {
    pub fn write(
        representations: &[CapturedRepresentation],
        mode: ClipboardWriteMode,
    ) -> Result<i64, ClipboardError> {
        Self::write_slices(&[representations], mode)
    }

    pub fn write_many(
        items: &[Vec<CapturedRepresentation>],
        mode: ClipboardWriteMode,
    ) -> Result<i64, ClipboardError> {
        Self::write_slices(&items.iter().map(Vec::as_slice).collect::<Vec<_>>(), mode)
    }

    fn write_slices(
        items: &[&[CapturedRepresentation]],
        mode: ClipboardWriteMode,
    ) -> Result<i64, ClipboardError> {
        Self::write_slices_to(items, mode, NSPasteboard::generalPasteboard)
    }

    // Private injection seam for native tests; production always supplies the
    // general pasteboard factory above. Resolve the destination only after all
    // validation and detached-object preparation, preserving failure safety.
    fn write_slices_to(
        items: &[&[CapturedRepresentation]],
        mode: ClipboardWriteMode,
        destination: impl FnOnce() -> Retained<NSPasteboard>,
    ) -> Result<i64, ClipboardError> {
        let plan = plan_write(
            items,
            mode,
            CaptureLimits {
                max_items: 200,
                ..Default::default()
            },
        )?;
        let mut native_items = Vec::with_capacity(plan.len());
        for representations in plan {
            let item = prepare_native_item(representations)?;
            native_items.push(ProtocolObject::from_retained(item));
        }

        let objects =
            NSArray::<ProtocolObject<dyn NSPasteboardWriting>>::from_retained_slice(&native_items);
        // All parsing, validation and native item creation succeed before the
        // current system clipboard is cleared. Both single and batch writes
        // use the same raw-data path; aliases never overwrite each other.
        let pasteboard = destination();
        let _ = pasteboard.clearContents();
        if !pasteboard.writeObjects(&objects) {
            return Err(ClipboardError::WriteFailed {
                uti: "multiple pasteboard items".into(),
            });
        }
        Ok(pasteboard.changeCount() as i64)
    }
}

pub struct MacClipboardReader {
    pasteboard: Retained<NSPasteboard>,
    cursor: ReadCursor,
}

impl MacClipboardReader {
    #[must_use]
    pub fn new(capture_existing: bool) -> Self {
        let pasteboard = NSPasteboard::generalPasteboard();
        let change_count = pasteboard.changeCount();
        Self {
            pasteboard,
            cursor: ReadCursor::new(change_count as i64, capture_existing),
        }
    }

    fn poll_inner(
        &mut self,
        policy: &ClipboardPrivacyPolicy,
        limits: CaptureLimits,
        device: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        self.cursor.poll(
            &NativePasteboardAccess(&self.pasteboard),
            policy,
            limits,
            device,
        )
    }
}

struct NativePasteboardAccess<'a>(&'a NSPasteboard);

impl SnapshotData for Retained<NSData> {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn owned_bytes(&self) -> Vec<u8> {
        self.to_vec()
    }
}

impl PasteboardAccess for NativePasteboardAccess<'_> {
    type Item = Retained<NSPasteboardItem>;
    type Data = Retained<NSData>;

    fn change_count(&self) -> i64 {
        self.0.changeCount() as i64
    }

    fn source(&self) -> SourceApplication {
        frontmost_application()
    }

    fn items(&self) -> Option<Vec<Self::Item>> {
        self.0.pasteboardItems().map(|items| items.to_vec())
    }

    fn types(&self, item: &Self::Item) -> Vec<String> {
        item.types()
            .to_vec()
            .into_iter()
            .map(|value| value.to_string())
            .collect()
    }

    fn data(&self, item: &Self::Item, uti: &str) -> Option<Self::Data> {
        item.dataForType(&NSString::from_str(uti))
    }
}

impl ClipboardSource for MacClipboardReader {
    fn discard_current(&mut self) -> Result<(), ClipboardError> {
        self.cursor
            .discard_current(&NativePasteboardAccess(&self.pasteboard));
        Ok(())
    }

    fn poll(
        &mut self,
        policy: &ClipboardPrivacyPolicy,
        limits: CaptureLimits,
        device: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        self.poll_inner(policy, limits, device)
    }
}

// This builds detached objects only; validation tests never request a named or
// general pasteboard and cannot overwrite the user's current clipboard.
fn prepare_native_item(
    representations: Vec<NativeRepresentation<'_>>,
) -> Result<Retained<NSPasteboardItem>, ClipboardError> {
    let item = NSPasteboardItem::new();
    for representation in representations {
        let native_type = NSString::from_str(&representation.native_type);
        let data = NSData::with_bytes(&representation.bytes);
        if !item.setData_forType(&data, &native_type) {
            return Err(ClipboardError::WriteFailed {
                uti: representation.native_type.into_owned(),
            });
        }
    }
    write_item_string(&item, INTERNAL_PASTEBOARD_TYPE, "1")?;
    Ok(item)
}

fn frontmost_application() -> SourceApplication {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map_or_else(SourceApplication::unknown, |application| {
            SourceApplication {
                bundle_identifier: application
                    .bundleIdentifier()
                    .map_or_else(|| "unknown".into(), |value| value.to_string()),
                display_name: application
                    .localizedName()
                    .map_or_else(|| "Unknown".into(), |value| value.to_string()),
            }
        })
}

fn write_item_string(
    item: &NSPasteboardItem,
    uti: &str,
    value: &str,
) -> Result<(), ClipboardError> {
    let native_type = NSString::from_str(uti);
    let value = NSString::from_str(value);
    if item.setString_forType(&value, &native_type) {
        Ok(())
    } else {
        Err(ClipboardError::WriteFailed { uti: uti.into() })
    }
}

#[cfg(test)]
mod private_pasteboard_tests;

#[cfg(test)]
mod write_tests {
    use super::*;
    use paste_domain::RepresentationKind;

    #[test]
    fn detached_native_items_preserve_raw_data_and_separate_batch_boundaries() {
        let typed = |kind, native_type: &str, bytes: &[u8]| CapturedRepresentation {
            kind,
            native_type: Some(native_type.into()),
            bytes: bytes.into(),
            mime_type: None,
            file_name: None,
        };
        let rich = vec![
            typed(
                RepresentationKind::PlainText,
                "public.utf16-external-plain-text",
                &[0xff, 0xfe, b'A', 0],
            ),
            typed(
                RepresentationKind::PlainText,
                "public.utf8-plain-text",
                b"A",
            ),
            typed(RepresentationKind::Rtf, "public.rtf", br"{\rtf1 A}"),
            typed(
                RepresentationKind::Color,
                "com.apple.cocoa.pasteboard.color",
                b"bplist00\0\xff",
            ),
        ];
        let link = [
            typed(
                RepresentationKind::Url,
                "public.url",
                b"https://example.com",
            ),
            typed(
                RepresentationKind::PlainText,
                "public.url-name",
                b"Synthetic title",
            ),
        ];
        let inputs = [&rich[..], &link[..]];
        let plan = plan_write(
            &inputs,
            ClipboardWriteMode::Original,
            CaptureLimits::default(),
        )
        .expect("valid plan");
        let native = plan
            .into_iter()
            .map(prepare_native_item)
            .collect::<Result<Vec<_>, _>>()
            .expect("detached objects");
        assert_eq!(native.len(), 2);
        for (item, originals) in native.iter().zip(inputs) {
            for source in originals {
                let native_type = NSString::from_str(source.native_type.as_deref().expect("type"));
                assert_eq!(
                    item.dataForType(&native_type)
                        .expect("raw native bytes")
                        .to_vec(),
                    source.bytes
                );
            }
            assert_eq!(
                item.stringForType(&NSString::from_str(INTERNAL_PASTEBOARD_TYPE))
                    .expect("internal marker")
                    .to_string(),
                "1"
            );
        }
        let plain = plan_write(
            &[&rich],
            ClipboardWriteMode::PlainText,
            CaptureLimits::default(),
        )
        .expect("decoded plain plan")
        .pop()
        .expect("one item");
        let plain = prepare_native_item(plain).expect("plain native object");
        assert_eq!(
            plain
                .stringForType(&NSString::from_str("public.utf8-plain-text"))
                .expect("system text decoder")
                .to_string(),
            "A"
        );
        assert!(
            plain
                .dataForType(&NSString::from_str("public.rtf"))
                .is_none()
        );
    }
}
