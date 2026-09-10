//! Shared, host-testable rules for the actual WASM search interaction.
#[cfg(test)]
use paste_domain::{ContentKind, DeviceId, PinboardId};

pub use paste_domain::SearchContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKeyAction {
    Results,
    Search,
    Dismiss,
    Type,
}

pub fn key_action(
    key: &str,
    search_focused: bool,
    results_focused: bool,
    active: bool,
    command_modifier: bool,
    composing: bool,
) -> Option<SearchKeyAction> {
    if composing || command_modifier {
        return None;
    }
    match key {
        "Enter" | "Tab" if search_focused => Some(SearchKeyAction::Results),
        "Tab" if results_focused => Some(SearchKeyAction::Search),
        "Escape" if active => Some(SearchKeyAction::Dismiss),
        "Escape" if search_focused => Some(SearchKeyAction::Results),
        // Space remains Quick Look. Never manufacture text from IME/control keys.
        _ if results_focused
            && key.chars().count() == 1
            && !key.chars().any(char::is_whitespace) =>
        {
            Some(SearchKeyAction::Type)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_search_is_global_but_dismissal_preserves_the_browsed_board() {
        let board = PinboardId::new();
        let mut context = SearchContext {
            board: Some(board),
            ..Default::default()
        };
        assert_eq!(context.scoped_board(), Some(board));
        context.text = "  E  ".into();
        assert!(context.is_search());
        assert_eq!(context.scoped_board(), None);
        context.text = " \n ".into();
        assert_eq!(context.scoped_board(), Some(board));
    }

    #[test]
    fn each_filter_alone_enters_global_search() {
        let base = SearchContext {
            board: Some(PinboardId::new()),
            ..Default::default()
        };
        for filtered in [
            SearchContext {
                kind: Some(ContentKind::Text),
                ..base.clone()
            },
            SearchContext {
                source: Some("test.synthetic".into()),
                ..base.clone()
            },
            SearchContext {
                device: Some(DeviceId::new()),
                ..base.clone()
            },
            SearchContext {
                days: Some(7),
                ..base.clone()
            },
        ] {
            assert!(filtered.is_search());
            assert_eq!(filtered.scoped_board(), None);
            assert_ne!(
                filtered, base,
                "stale responses must include filters in their identity"
            );
        }
        assert_ne!(
            SearchContext {
                history_offset: 200,
                ..base.clone()
            },
            base
        );
    }

    #[test]
    fn return_and_tab_transfer_focus_without_activating_a_result() {
        assert_eq!(
            key_action("Enter", true, false, true, false, false),
            Some(SearchKeyAction::Results)
        );
        assert_eq!(key_action("Enter", false, true, true, false, false), None);
        assert_eq!(
            key_action("Tab", true, false, true, false, false),
            Some(SearchKeyAction::Results)
        );
        assert_eq!(
            key_action("Tab", false, true, true, false, false),
            Some(SearchKeyAction::Search)
        );
        assert_eq!(key_action("Tab", false, false, true, false, false), None);
    }

    #[test]
    fn escape_dismisses_search_before_hiding_and_does_not_intercept_composition() {
        assert_eq!(
            key_action("Escape", false, true, true, false, false),
            Some(SearchKeyAction::Dismiss)
        );
        assert_eq!(
            key_action("Escape", true, false, false, false, false),
            Some(SearchKeyAction::Results)
        );
        assert_eq!(key_action("Escape", false, true, false, false, false), None);
        for key in ["Enter", "Tab", "Escape", "e"] {
            assert_eq!(key_action(key, true, false, true, false, true), None);
            assert_eq!(key_action(key, true, false, true, true, false), None);
        }
    }

    #[test]
    fn type_to_search_is_limited_to_unmodified_printable_keys_in_the_results() {
        assert_eq!(
            key_action("e", false, true, false, false, false),
            Some(SearchKeyAction::Type)
        );
        assert_eq!(
            key_action("中", false, true, false, false, false),
            Some(SearchKeyAction::Type)
        );
        for key in [" ", "ArrowRight", "Dead", "Process", "Backspace"] {
            assert_eq!(key_action(key, false, true, false, false, false), None);
        }
        assert_eq!(key_action("e", true, false, true, false, false), None);
    }
}
