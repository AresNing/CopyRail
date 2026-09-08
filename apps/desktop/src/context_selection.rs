use paste_domain::ClipId;
use std::collections::HashSet;

/// Resolve once when opening a menu, never from the later live selection.
pub fn resolve(
    visible: &[ClipId],
    selected: &HashSet<ClipId>,
    clicked: ClipId,
) -> Option<Vec<ClipId>> {
    if !visible.contains(&clicked) {
        return None;
    }
    Some(if selected.contains(&clicked) {
        visible
            .iter()
            .copied()
            .filter(|id| selected.contains(id))
            .collect()
    } else {
        vec![clicked]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_preserves_non_contiguous_selection_in_visual_order() {
        let ids = [ClipId::new(), ClipId::new(), ClipId::new()];
        let selection = HashSet::from([ids[2], ids[0], ClipId::new()]);
        assert_eq!(
            resolve(&ids, &selection, ids[2]),
            Some(vec![ids[0], ids[2]])
        );
    }
    #[test]
    fn right_click_outside_selection_replaces_it_and_missing_targets_are_rejected() {
        let ids = [ClipId::new(), ClipId::new()];
        assert_eq!(
            resolve(&ids, &HashSet::from([ids[0]]), ids[1]),
            Some(vec![ids[1]])
        );
        assert_eq!(resolve(&ids, &HashSet::new(), ClipId::new()), None);
    }
}
