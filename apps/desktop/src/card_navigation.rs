//! Keyboard rules for the real timeline and its nested action buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKey {
    Exit,
    Previous,
    Next,
    PreviousOrExit,
    NextOrExit,
}

pub fn navigation_index(key: &str, current: usize, count: usize, modified: bool) -> Option<usize> {
    if count == 0 || modified {
        return None;
    }
    let current = current.min(count - 1);
    match key {
        "ArrowRight" => Some(current.saturating_add(1).min(count - 1)),
        "ArrowLeft" => Some(current.saturating_sub(1)),
        "Home" => Some(0),
        "End" => Some(count - 1),
        _ => None,
    }
}

pub fn action_key(key: &str, shift: bool, modified: bool) -> Option<ActionKey> {
    if modified {
        return None;
    }
    match key {
        "Escape" | "F2" => Some(ActionKey::Exit),
        "Tab" if shift => Some(ActionKey::PreviousOrExit),
        "Tab" => Some(ActionKey::NextOrExit),
        "ArrowLeft" | "ArrowUp" => Some(ActionKey::Previous),
        "ArrowRight" | "ArrowDown" => Some(ActionKey::Next),
        _ => None,
    }
}

pub fn action_target(action: ActionKey, current: usize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let current = current.min(count - 1);
    match action {
        ActionKey::Exit => None,
        ActionKey::Previous => Some(current.saturating_sub(1)),
        ActionKey::Next => Some(current.saturating_add(1).min(count - 1)),
        ActionKey::PreviousOrExit => current.checked_sub(1),
        ActionKey::NextOrExit => current.checked_add(1).filter(|index| *index < count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_is_bounded_and_home_end_cover_loaded_results() {
        assert_eq!(navigation_index("Home", 4, 8, false), Some(0));
        assert_eq!(navigation_index("End", 4, 8, false), Some(7));
        assert_eq!(navigation_index("ArrowLeft", 0, 8, false), Some(0));
        assert_eq!(
            navigation_index("ArrowRight", usize::MAX, 8, false),
            Some(7)
        );
        assert_eq!(navigation_index("Home", 0, 0, false), None);
    }

    #[test]
    fn modified_keys_and_button_activation_are_not_repurposed() {
        for key in ["Home", "End", "ArrowRight", "ArrowLeft"] {
            assert_eq!(navigation_index(key, 0, 8, true), None);
        }
        for key in ["Enter", " ", "Delete", "a"] {
            assert_eq!(action_key(key, false, false), None);
        }
        assert_eq!(action_key("ArrowRight", false, true), None);
    }

    #[test]
    fn button_navigation_can_exit_without_trapping_tab() {
        assert_eq!(action_key("F2", false, false), Some(ActionKey::Exit));
        assert_eq!(action_key("Escape", false, false), Some(ActionKey::Exit));
        assert_eq!(
            action_key("Tab", true, false),
            Some(ActionKey::PreviousOrExit)
        );
        assert_eq!(action_target(ActionKey::PreviousOrExit, 0, 2), None);
        assert_eq!(action_target(ActionKey::NextOrExit, 1, 2), None);
        assert_eq!(action_target(ActionKey::NextOrExit, 0, 2), Some(1));
        assert_eq!(action_target(ActionKey::Next, 1, 2), Some(1));
        assert_eq!(action_target(ActionKey::Previous, 0, 2), Some(0));
        assert_eq!(action_target(ActionKey::Exit, 1, 2), None);
        assert_eq!(action_target(ActionKey::Next, 0, 0), None);
    }
}
