//! Read-only previews own their edit commands. No action in this module may
//! fall through to the hidden timeline's undo/selection/writeback behavior.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Route {
    NativeDocument,
    SelectPreviewText,
    CopyClip,
    Ignore,
}

pub fn route(action: &str, document_focused: bool, has_text: bool, selected_text: bool) -> Route {
    if !matches!(
        action,
        "undo" | "redo" | "cut" | "copy" | "paste" | "select_all"
    ) {
        return Route::Ignore;
    }
    if document_focused {
        return Route::NativeDocument;
    }
    match action {
        "select_all" if has_text => Route::SelectPreviewText,
        "copy" if has_text && selected_text => Route::NativeDocument,
        "copy" => Route::CopyClip,
        _ => Route::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_commands_stay_in_the_native_document_not_the_history() {
        for action in ["undo", "redo", "cut", "copy", "paste", "select_all"] {
            assert_eq!(route(action, true, false, false), Route::NativeDocument);
        }
        assert_eq!(route("delete", true, false, false), Route::Ignore);
    }

    #[test]
    fn text_and_image_preview_copy_have_explicit_targets() {
        assert_eq!(
            route("select_all", false, true, false),
            Route::SelectPreviewText
        );
        assert_eq!(route("copy", false, true, true), Route::NativeDocument);
        assert_eq!(route("copy", false, true, false), Route::CopyClip);
        assert_eq!(route("copy", false, false, false), Route::CopyClip);
        assert_eq!(route("select_all", false, false, false), Route::Ignore);
    }

    #[test]
    fn read_only_previews_never_fall_through_to_timeline_mutations() {
        for action in ["undo", "redo", "cut", "paste", "delete", "", "COPY"] {
            for has_text in [false, true] {
                assert_eq!(route(action, false, has_text, true), Route::Ignore);
            }
        }
    }
}
