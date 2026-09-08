use serde::Serialize;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    pub isolated: bool,
    pub registered: bool,
    pub error: Option<String>,
}

pub fn default_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV)
}

trait Registrar {
    // Only reports ownership by this application, not availability system-wide.
    fn is_registered(&self) -> bool;
    fn register(&self) -> Result<(), String>;
}

impl Registrar for tauri::AppHandle {
    fn is_registered(&self) -> bool {
        self.global_shortcut().is_registered(default_shortcut())
    }
    fn register(&self) -> Result<(), String> {
        self.global_shortcut()
            .register(default_shortcut())
            .map_err(|error| error.to_string())
    }
}

/// Startup and explicit retry call this on the main thread. Registration failure
/// is a degraded status, never an error that aborts the rest of app setup.
pub fn register_or_report(app: &tauri::AppHandle, isolated: bool) -> ShortcutStatus {
    attempt(app, isolated)
}

fn attempt(registrar: &impl Registrar, isolated: bool) -> ShortcutStatus {
    if isolated {
        return ShortcutStatus {
            isolated: true,
            registered: false,
            error: None,
        };
    }
    let result = if registrar.is_registered() {
        Ok(())
    } else {
        registrar.register()
    };
    ShortcutStatus {
        isolated: false,
        registered: result.is_ok(),
        error: result.err(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeRegistrar {
        calls: Cell<usize>,
        owned: Cell<bool>,
        conflict: Cell<bool>,
    }
    impl Registrar for FakeRegistrar {
        fn is_registered(&self) -> bool {
            self.owned.get()
        }
        fn register(&self) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            if self.conflict.get() {
                Err("synthetic registration conflict".into())
            } else {
                self.owned.set(true);
                Ok(())
            }
        }
    }

    #[test]
    fn registration_failure_is_a_status_and_does_not_abort_startup() {
        let registrar = FakeRegistrar {
            conflict: Cell::new(true),
            ..Default::default()
        };
        let status = attempt(&registrar, false);
        assert!(!status.registered && !status.isolated);
        assert_eq!(
            status.error.as_deref(),
            Some("synthetic registration conflict")
        );
        assert_eq!(registrar.calls.get(), 1);
        // Callers receive an ordinary status and can continue initializing the
        // tray/history. No unwrap, `?` or alternate system binding is required.
        assert_eq!(
            serde_json::to_value(status).expect("status JSON")["registered"],
            false
        );
    }

    #[test]
    fn explicit_retry_recovers_without_replacing_an_owned_binding() {
        let registrar = FakeRegistrar {
            conflict: Cell::new(true),
            ..Default::default()
        };
        assert!(!attempt(&registrar, false).registered);
        registrar.conflict.set(false);
        let restored = attempt(&registrar, false);
        assert!(restored.registered && restored.error.is_none());
        assert_eq!(registrar.calls.get(), 2);
        assert_eq!(attempt(&registrar, false), restored);
        assert_eq!(
            registrar.calls.get(),
            2,
            "owned key must not be registered twice"
        );
    }

    #[test]
    fn repeated_failure_never_claims_registration_or_changes_the_key() {
        let registrar = FakeRegistrar {
            conflict: Cell::new(true),
            ..Default::default()
        };
        for _ in 0..3 {
            assert!(!attempt(&registrar, false).registered);
        }
        assert_eq!(registrar.calls.get(), 3);
        assert_eq!(
            default_shortcut(),
            Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV)
        );
    }

    #[test]
    fn isolation_does_not_query_or_register_system_keys() {
        struct MustNotRun;
        impl Registrar for MustNotRun {
            fn is_registered(&self) -> bool {
                panic!("isolation must not query registration")
            }
            fn register(&self) -> Result<(), String> {
                panic!("isolation must not register")
            }
        }
        let status = attempt(&MustNotRun, true);
        assert!(status.isolated && !status.registered && status.error.is_none());
    }
}
