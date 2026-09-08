pub use crate::drag_feedback::DropTarget;
use leptos::{prelude::*, task::spawn_local};
use paste_domain::{ClipId, PinboardId};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    fn listen_js(name: &str, handler: &js_sys::Function) -> js_sys::Promise;
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragEvent {
    pub phase: String,
    pub session_id: String,
    pub clip_ids: Vec<ClipId>,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub tab_feedback: Option<crate::drag_feedback::TabFeedback>,
    #[serde(default)]
    pub placement_feedback: Option<crate::drag_feedback::PlacementFeedback>,
}

#[derive(Deserialize)]
struct EventEnvelope<T> {
    payload: T,
}

/// Own both the JS callback and its unlisten function until the app unmounts.
/// A registration finishing after unmount is immediately unregistered too.
pub fn subscribe(name: &'static str, callback: Callback<JsValue>, on_error: Callback<String>) {
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |value| callback.run(value));
    let listener = StoredValue::new_local((handler, None::<js_sys::Function>));
    on_cleanup(move || {
        let _ = listener.try_update_value(|(_, unlisten)| {
            if let Some(unlisten) = unlisten.take() {
                let _ = unlisten.call0(&JsValue::NULL);
            }
        });
    });
    spawn_local(async move {
        let pending =
            listener.with_value(|(handler, _)| listen_js(name, handler.as_ref().unchecked_ref()));
        match JsFuture::from(pending)
            .await
            .and_then(|value| value.dyn_into::<js_sys::Function>())
        {
            Ok(unlisten) => {
                if listener
                    .try_update_value(|(_, current)| *current = Some(unlisten.clone()))
                    .is_none()
                {
                    let _ = unlisten.call0(&JsValue::NULL);
                }
            }
            Err(_) => on_error.run(format!("无法连接界面事件 {name}，请重新打开窗口。")),
        }
    });
}

pub fn decode(value: JsValue) -> Option<DragEvent> {
    serde_wasm_bindgen::from_value::<EventEnvelope<DragEvent>>(value)
        .ok()
        .map(|event| event.payload)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragEnded {
    pub session_id: String,
    pub cancelled: bool,
}

pub fn decode_end(value: JsValue) -> Option<DragEnded> {
    serde_wasm_bindgen::from_value::<EventEnvelope<DragEnded>>(value)
        .ok()
        .map(|e| e.payload)
}

pub fn feedback_layout() -> crate::drag_feedback::DragLayout {
    let document = leptos::prelude::document();
    let window = leptos::prelude::window();
    let mut layout = crate::drag_feedback::DragLayout {
        width: window
            .inner_width()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        height: window
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        tabs: Vec::new(),
        timeline: None,
    };
    let Ok(Some(nav)) = document.query_selector(".pinboards") else {
        return layout;
    };
    let bounds = nav.get_bounding_client_rect();
    let Ok(nodes) = document.query_selector_all("[data-drop-board]") else {
        return layout;
    };
    for i in 0..nodes.length() {
        if layout.tabs.len() == 64 {
            break;
        }
        let Some(el) = nodes
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let r = el.get_bounding_client_rect();
        let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() else {
            continue;
        };
        // The highlight scales around its center. Use the unscaled hit box,
        // rounded to quarter points, so its animation cannot feed back into
        // native geometry and generate an endless stream of layout updates.
        let stable = |v: f64| (v * 4.0).round() / 4.0;
        let raw_width = f64::from(html.offset_width());
        let raw_height = f64::from(html.offset_height());
        let left = stable(r.x() + (r.width() - raw_width) / 2.0);
        let top = stable(r.y() + (r.height() - raw_height) / 2.0);
        let x = left.max(bounds.left()).max(0.0);
        let y = top.max(bounds.top()).max(0.0);
        let width = (left + raw_width).min(bounds.right()).min(layout.width) - x;
        let height = (top + raw_height).min(bounds.bottom()).min(layout.height) - y;
        if width < 4.0 || height < 4.0 {
            continue;
        }
        // Do not report tabs covered by an editor/modal as native targets.
        let Some(hit) = document
            .element_from_point((x + width / 2.0) as f32, (y + height / 2.0) as f32)
            .and_then(|e| e.closest("[data-drop-board]").ok().flatten())
        else {
            continue;
        };
        if hit.get_attribute("data-drop-board") != el.get_attribute("data-drop-board") {
            continue;
        }
        if let Some(id) = el
            .get_attribute("data-drop-board")
            .and_then(|s| s.parse().ok())
        {
            layout.tabs.push(crate::drag_feedback::DragTab {
                id,
                x,
                y,
                width,
                height,
            });
        }
    }
    layout.timeline = timeline_layout(layout.width, layout.height);
    layout
}

fn timeline_layout(width: f64, height: f64) -> Option<crate::drag_feedback::DragTimeline> {
    use crate::drag_feedback::{DragCard, DragRect, DragTimeline};
    let document = leptos::prelude::document();
    let timeline = document
        .query_selector(".timeline[data-drop-pinboard]")
        .ok()??;
    let pinboard_id = timeline.get_attribute("data-drop-pinboard")?.parse().ok()?;
    let r = timeline.get_bounding_client_rect();
    let bounds = DragRect {
        x: r.left().max(0.0),
        y: r.top().max(0.0),
        width: r.right().min(width) - r.left().max(0.0),
        height: r.bottom().min(height) - r.top().max(0.0),
    };
    document
        .element_from_point(
            (bounds.x + bounds.width / 2.0) as f32,
            (bounds.y + bounds.height / 2.0) as f32,
        )?
        .closest(".timeline")
        .ok()??;
    let nodes = timeline.query_selector_all("[data-drop-clip]").ok()?;
    let mut cards = Vec::new();
    for i in 0..nodes.length() {
        let Some(el) = nodes
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let r = el.get_bounding_client_rect();
        if r.right() <= bounds.x || r.left() >= bounds.x + bounds.width {
            continue;
        }
        let Some(id) = el
            .get_attribute("data-drop-clip")
            .and_then(|s| s.parse().ok())
        else {
            continue;
        };
        if cards.len() == 64 {
            return None;
        } // Do not misrepresent omitted visible anchors as an empty end.
        cards.push(DragCard {
            id,
            bounds: DragRect {
                x: r.x(),
                y: r.y(),
                width: r.width(),
                height: r.height(),
            },
        });
    }
    Some(DragTimeline {
        pinboard_id,
        bounds,
        cards,
    })
}

pub fn tab_at_point(x: f64, y: f64) -> Option<PinboardId> {
    let layout = feedback_layout();
    layout
        .hit(x, y, layout.width, layout.height)
        .map(|tab| tab.id)
}

pub fn hit_test(
    x: f64,
    y: f64,
    active_board: Option<PinboardId>,
    allow_reorder: bool,
    dragged: &[ClipId],
) -> Option<DropTarget> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let document = leptos::prelude::document();
    let element = document.element_from_point(x as f32, y as f32)?;
    element.closest("[data-drop-board], .timeline").ok()??;
    let mut layout = feedback_layout();
    if !allow_reorder
        || layout
            .timeline
            .as_ref()
            .is_some_and(|t| Some(t.pinboard_id) != active_board)
    {
        layout.timeline = None;
    }
    layout
        .target(x, y, layout.width, layout.height, dragged)
        .map(|hit| hit.placement)
}

pub fn scroll_at_edge(x: f64, y: f64) {
    for selector in [".card-track", ".pinboards"] {
        if let Ok(Some(element)) = leptos::prelude::document().query_selector(selector) {
            let rect = element.get_bounding_client_rect();
            if y < rect.top() || y > rect.bottom() || x < rect.left() || x > rect.right() {
                continue;
            }
            let delta = if x < rect.left() + 28.0 {
                -18
            } else if x > rect.right() - 28.0 {
                18
            } else {
                0
            };
            if delta != 0 {
                element.set_scroll_left(element.scroll_left() + delta);
            }
        }
    }
}

pub fn tab_target(x: i32, y: i32) -> Option<(PinboardId, bool)> {
    let element = leptos::prelude::document().element_from_point(x as f32, y as f32)?;
    let board = element.closest("[data-drop-board]").ok().flatten()?;
    let rect = board.get_bounding_client_rect();
    Some((
        board.get_attribute("data-drop-board")?.parse().ok()?,
        f64::from(x) >= rect.x() + rect.width() / 2.0,
    ))
}
