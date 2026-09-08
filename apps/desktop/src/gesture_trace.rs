//! Typed diagnostic metadata, never clipboard text or arbitrary log strings.
use paste_domain::ClipId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GesturePhase {
    RootPointerDown,
    RootPointerMove,
    RootPointerUp,
    RootMouseDown,
    RootMouseMove,
    RootMouseUp,
    CardDown,
    CardMove,
    CardUp,
    CardCancel,
    CardLostCapture,
    Armed,
    CaptureFailed,
    Threshold,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GestureTrace {
    pub phase: GesturePhase,
    pub clip_id: Option<ClipId>,
    pub pointer_id: Option<i32>,
    pub x: i32,
    pub y: i32,
    pub button: i16,
    pub buttons: u16,
    pub trusted: bool,
    pub control_target: bool,
    pub timestamp_ms: u64,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    fn trace_schema_cannot_carry_arbitrary_text() {
        let value = serde_json::json!({"phase":"armed", "clip_id":null, "pointer_id":1,
            "x":20,"y":30,"button":0,"buttons":1,"trusted":true,"control_target":false,"timestamp_ms":100});
        assert!(serde_json::from_value::<GestureTrace>(value.clone()).is_ok());
        let mut unknown = value.clone();
        unknown["text"] = serde_json::json!("synthetic extra payload");
        assert!(serde_json::from_value::<GestureTrace>(unknown).is_err());
        let mut phase = value;
        phase["phase"] = serde_json::json!("arbitrary content");
        assert!(serde_json::from_value::<GestureTrace>(phase).is_err());
    }
}

#[cfg(target_arch = "wasm32")]
pub fn pointer(
    phase: GesturePhase,
    id: Option<ClipId>,
    event: &web_sys::PointerEvent,
) -> GestureTrace {
    let mut trace = mouse(phase, event.as_ref());
    trace.clip_id = id;
    trace.pointer_id = Some(event.pointer_id());
    trace
}

#[cfg(target_arch = "wasm32")]
pub fn mouse(phase: GesturePhase, event: &web_sys::MouseEvent) -> GestureTrace {
    use wasm_bindgen::JsCast;
    GestureTrace {
        phase,
        clip_id: None,
        pointer_id: None,
        x: event.client_x(),
        y: event.client_y(),
        button: event.button(),
        buttons: event.buttons(),
        trusted: event.is_trusted(),
        control_target: event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|element| element.closest("button, a, input").ok().flatten().is_some()),
        timestamp_ms: event.time_stamp() as u64,
    }
}
