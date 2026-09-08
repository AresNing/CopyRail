//! Pointer threshold used by real cards, before AppKit owns the drag session.
use paste_domain::ClipId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardPress {
    pub clip_id: ClipId,
    pub pointer_id: i32,
    x: i32,
    y: i32,
}

impl CardPress {
    pub fn begin(
        clip_id: ClipId,
        pointer_id: i32,
        x: i32,
        y: i32,
        button: i16,
        primary: bool,
    ) -> Option<Self> {
        (primary && button == 0).then_some(Self {
            clip_id,
            pointer_id,
            x,
            y,
        })
    }

    pub fn belongs_to(self, clip_id: ClipId, pointer_id: i32) -> bool {
        self.clip_id == clip_id && self.pointer_id == pointer_id
    }

    pub fn ready(self, pointer_id: i32, x: i32, y: i32, buttons: u16) -> bool {
        self.pointer_id == pointer_id
            && buttons & 1 != 0
            && (i64::from(x).abs_diff(i64::from(self.x)) >= 6
                || i64::from(y).abs_diff(i64::from(self.y)) >= 6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_primary_left_presses_arm_a_card() {
        let id = ClipId::new();
        assert!(CardPress::begin(id, 1, 0, 0, 0, true).is_some());
        for button in [1, 2, -1] {
            assert!(CardPress::begin(id, 1, 0, 0, button, true).is_none());
        }
        assert!(CardPress::begin(id, 2, 0, 0, 0, false).is_none());
    }
    #[test]
    fn tiny_motion_is_a_click_and_threshold_requires_a_held_left_button() {
        let press = CardPress::begin(ClipId::new(), 1, 20, 30, 0, true).expect("left press");
        assert!(!press.ready(1, 25, 35, 1));
        assert!(press.ready(1, 26, 30, 1));
        assert!(press.ready(1, 20, 24, 1));
        assert!(!press.ready(1, 100, 100, 0));
        assert!(!press.ready(1, 100, 100, 2));
    }
    #[test]
    fn another_pointer_or_card_cannot_take_over_or_cancel_the_press() {
        let id = ClipId::new();
        let press = CardPress::begin(id, 7, 0, 0, 0, true).expect("left press");
        assert!(press.belongs_to(id, 7));
        assert!(!press.belongs_to(ClipId::new(), 7));
        assert!(!press.belongs_to(id, 8));
        assert!(!press.ready(8, 100, 100, 1));
    }
    #[test]
    fn negative_and_extreme_pointer_coordinates_do_not_overflow() {
        let press =
            CardPress::begin(ClipId::new(), 1, i32::MIN, -100, 0, true).expect("left press");
        assert!(press.ready(1, i32::MAX, -100, 1));
        assert!(press.ready(1, i32::MIN, -106, 1));
    }
}
