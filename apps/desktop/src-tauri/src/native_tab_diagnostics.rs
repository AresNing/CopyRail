//! Debug-only, isolated metadata probe. It neither drives the UI nor changes
//! styles/animation state. Native gestures still come from acceptance tools.
use std::sync::Mutex;

use paste_domain::PinboardId;
use serde::{Deserialize, Serialize};

pub const PROBE: &str = include_str!("native_tab_probe.js");

#[derive(Default)]
pub struct TabDiagnostics {
    state: Mutex<TraceBudget>,
}

#[derive(Default)]
struct TraceBudget {
    last_context: Option<(Option<PinboardId>, bool)>,
    sequences: u8,
}

impl TabDiagnostics {
    fn sequence(&self, board: Option<PinboardId>, search: bool) -> Option<u8> {
        let mut state = self.state.lock().ok()?;
        if state.sequences >= 32 || state.last_context == Some((board, search)) {
            return None;
        }
        state.last_context = Some((board, search));
        state.sequences += 1;
        Some(state.sequences)
    }

    pub fn observe(&self, window: &tauri::WebviewWindow, board: Option<PinboardId>, search: bool) {
        if !cfg!(debug_assertions) || window.label() != "main" {
            return;
        }
        let Some(sequence) = self.sequence(board, search) else {
            return;
        };
        let window = window.clone();
        tauri::async_runtime::spawn(async move {
            for (delay_ms, elapsed_ms) in [(0, 0), (200, 200), (600, 800)] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                let native_visible = window.is_visible().ok();
                let native_focused = window.is_focused().ok();
                // Never raise/resize/focus the window to collect a sample.
                let result = window.eval_with_callback(PROBE, move |json| {
                    let Ok(snapshot) = parse(&json) else {
                        eprintln!("Native UI test tabs: sequence={sequence} elapsed_ms={elapsed_ms} invalid_snapshot=true");
                        return;
                    };
                    #[cfg(target_os = "macos")]
                    let app_hidden_active = objc2::MainThreadMarker::new().map(|mtm| {
                        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                        (app.isHidden(), app.isActive())
                    });
                    #[cfg(not(target_os = "macos"))]
                    let app_hidden_active = None::<(bool, bool)>;
                    if let Ok(snapshot) = serde_json::to_string(&snapshot) {
                        eprintln!("Native UI test tabs: sequence={sequence} scheduled_ms={elapsed_ms} scoped={} search={search} native_visible={native_visible:?} native_focused={native_focused:?} app_hidden_active={app_hidden_active:?} snapshot={snapshot}", board.is_some());
                    }
                });
                if result.is_err() {
                    eprintln!(
                        "Native UI test tabs: sequence={sequence} elapsed_ms={elapsed_ms} evaluation_failed=true"
                    );
                }
            }
        });
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    visible: bool,
    focused: bool,
    now_ms: f64,
    timeline_ms: Option<f64>,
    tab_count: u16,
    tabs: Vec<Tab>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Tab {
    ordinal: u8,
    active: bool,
    current: bool,
    hovered: bool,
    background: [f64; 4],
    animation_count: u16,
    animations: Vec<Animation>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Animation {
    background_transition: bool,
    state: i8,
    pending: bool,
    current_ms: Option<f64>,
    start_ms: Option<f64>,
    timeline_ms: Option<f64>,
}

fn parse(json: &str) -> Result<Snapshot, ()> {
    if json.len() > 32_768 {
        return Err(());
    }
    let snapshot: Snapshot = serde_json::from_str(json).map_err(|_| ())?;
    let time = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && v.abs() <= 1e12);
    if !time(Some(snapshot.now_ms))
        || !time(snapshot.timeline_ms)
        || snapshot.tabs.len() > 16
        || usize::from(snapshot.tab_count) < snapshot.tabs.len()
        || snapshot.tabs.iter().enumerate().any(|(ordinal, tab)| {
            usize::from(tab.ordinal) != ordinal
                || tab.animations.len() > 8
                || usize::from(tab.animation_count) < tab.animations.len()
                || tab.background.iter().enumerate().any(|(i, &v)| {
                    !v.is_finite() || v < 0.0 || v > if i == 3 { 1.0 } else { 255.0 }
                })
                || tab.animations.iter().any(|a| {
                    !(-1..=3).contains(&a.state)
                        || !time(a.current_ms)
                        || !time(a.start_ms)
                        || !time(a.timeline_ms)
                })
        })
    {
        return Err(());
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> serde_json::Value {
        serde_json::json!({"visible":true,"focused":false,"now_ms":100.0,"timeline_ms":90.0,
            "tab_count":1,"tabs":[{"ordinal":0,"active":true,"current":true,"hovered":false,
            "background":[255,255,255,0.18],"animation_count":1,"animations":[{
            "background_transition":true,"state":1,"pending":true,"current_ms":0.0,
            "start_ms":null,"timeline_ms":90.0}]}]})
    }

    #[test]
    fn accepts_only_bounded_typed_metadata() {
        assert!(parse(&valid().to_string()).is_ok());
        let mut text = valid();
        text["text"] = serde_json::json!("unwanted synthetic text");
        assert!(parse(&text.to_string()).is_err());
        let mut nested = valid();
        nested["tabs"][0]["label"] = serde_json::json!("not allowed");
        assert!(parse(&nested.to_string()).is_err());
        let mut color = valid();
        color["tabs"][0]["background"][3] = serde_json::json!(2.0);
        assert!(parse(&color.to_string()).is_err());
        assert!(parse(&" ".repeat(32_769)).is_err());
    }

    #[test]
    fn rejects_truncated_counts_and_unbounded_arrays() {
        let mut value = valid();
        value["tab_count"] = serde_json::json!(0);
        assert!(parse(&value.to_string()).is_err());
        let mut value = valid();
        value["tabs"] = serde_json::json!(vec![value["tabs"][0].clone(); 17]);
        assert!(parse(&value.to_string()).is_err());
        let mut value = valid();
        value["tabs"][0]["animation_count"] = serde_json::json!(9);
        value["tabs"][0]["animations"] =
            serde_json::json!(vec![value["tabs"][0]["animations"][0].clone(); 9]);
        assert!(parse(&value.to_string()).is_err());
    }

    #[test]
    fn unchanged_polling_does_not_spend_the_session_budget() {
        let trace = TabDiagnostics::default();
        assert_eq!(trace.sequence(None, false), Some(1));
        for _ in 0..100 {
            assert_eq!(trace.sequence(None, false), None);
        }
        let board = PinboardId::new();
        assert_eq!(trace.sequence(Some(board), false), Some(2));
        assert_eq!(trace.sequence(Some(board), false), None);
        for sequence in 3..=32 {
            assert_eq!(
                trace.sequence(Some(PinboardId::new()), false),
                Some(sequence)
            );
        }
        assert_eq!(trace.sequence(None, true), None);
        assert_eq!(TabDiagnostics::default().sequence(None, true), Some(1));
    }
}
