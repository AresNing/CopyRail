#[cfg(target_arch = "wasm32")]
mod i18n;

#[cfg(target_arch = "wasm32")]
mod app;

#[cfg(target_arch = "wasm32")]
mod context_action;

#[cfg(target_arch = "wasm32")]
mod drag_drop;

#[cfg(target_arch = "wasm32")]
mod gesture_trace;

#[cfg(any(target_arch = "wasm32", test))]
mod search;

#[cfg(any(target_arch = "wasm32", test))]
mod preview_edit;

#[cfg(any(target_arch = "wasm32", test))]
mod drag_gesture;

#[cfg(any(target_arch = "wasm32", test))]
mod drag_lifecycle;

#[cfg(any(target_arch = "wasm32", test))]
#[allow(dead_code)]
mod drag_feedback;

#[cfg(any(target_arch = "wasm32", test))]
mod context_selection;

#[cfg(any(target_arch = "wasm32", test))]
mod card_visual;

#[cfg(any(target_arch = "wasm32", test))]
mod card_navigation;

#[cfg(any(target_arch = "wasm32", test))]
#[allow(dead_code)]
mod source_icon;

#[cfg(target_arch = "wasm32")]
fn main() {
    leptos::mount::mount_to_body(app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
