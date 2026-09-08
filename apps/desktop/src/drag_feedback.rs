//! Point-space geometry shared by the WebView snapshot and native feedback.
//! This is presentation data, not authority to move clipboard contents.
use paste_domain::{ClipId, PinboardId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DragTab {
    pub id: PinboardId,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DragLayout {
    pub width: f64,
    pub height: f64,
    pub tabs: Vec<DragTab>,
    #[serde(default)]
    pub timeline: Option<DragTimeline>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DragRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DragRect {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.x.abs() <= 16384.0
            && self.y.abs() <= 16384.0
            && (4.0..=16384.0).contains(&self.width)
            && (4.0..=16384.0).contains(&self.height)
    }
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
    fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DragCard {
    pub id: ClipId,
    pub bounds: DragRect,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DragTimeline {
    pub pinboard_id: PinboardId,
    pub bounds: DragRect,
    pub cards: Vec<DragCard>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DropTarget {
    pub pinboard_id: PinboardId,
    pub anchor: Option<ClipId>,
    pub after: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetVisual {
    Tab,
    Insertion { x: f64, y: f64, height: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetHit {
    pub placement: DropTarget,
    pub visual: TargetVisual,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementFeedback {
    pub target: Option<DropTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutUpdate {
    pub session_id: String,
    pub revision: u32,
    pub layout: DragLayout,
}

/// Present only for a native destination that paints tab feedback. The DOM
/// must not silently choose another board after scroll/resize during delivery.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TabFeedback {
    pub target: Option<PinboardId>,
}

impl DragLayout {
    pub fn valid(&self) -> bool {
        [self.width, self.height]
            .into_iter()
            .all(|v| v.is_finite() && (1.0..=16384.0).contains(&v))
            && self.tabs.len() <= 64
            && self.tabs.iter().enumerate().all(|(i, t)| {
                [t.x, t.y, t.width, t.height]
                    .into_iter()
                    .all(f64::is_finite)
                    && t.x >= 0.0
                    && t.y >= 0.0
                    && t.width >= 4.0
                    && t.height >= 4.0
                    && t.x + t.width <= self.width + 0.5
                    && t.y + t.height <= self.height + 0.5
                    && !self.tabs[..i].iter().any(|other| other.id == t.id)
            })
            && self.timeline.as_ref().is_none_or(|timeline| {
                let r = timeline.bounds;
                r.valid()
                    && r.x >= 0.0
                    && r.y >= 0.0
                    && r.x + r.width <= self.width + 0.5
                    && r.y + r.height <= self.height + 0.5
                    && timeline.cards.len() <= 64
                    && timeline.cards.iter().enumerate().all(|(i, card)| {
                        card.bounds.valid()
                            && card.bounds.intersects(r)
                            && !timeline.cards[..i].iter().any(|other| other.id == card.id)
                            && (i == 0
                                || card.bounds.x
                                    >= timeline.cards[i - 1].bounds.x
                                        + timeline.cards[i - 1].bounds.width
                                        - 0.5)
                    })
            })
    }

    pub fn hit(&self, x: f64, y: f64, width: f64, height: f64) -> Option<&DragTab> {
        // A resized window must not display a stale target; coordinates are
        // logical view points on both sides, with no extra Retina conversion.
        if !self.valid()
            || ![x, y, width, height].into_iter().all(f64::is_finite)
            || (width - self.width).abs() > 0.5
            || (height - self.height).abs() > 0.5
        {
            return None;
        }
        self.tabs
            .iter()
            .find(|t| x >= t.x && x < t.x + t.width && y >= t.y && y < t.y + t.height)
    }

    pub fn target(
        &self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        dragged: &[ClipId],
    ) -> Option<TargetHit> {
        if !self.valid()
            || ![x, y, width, height].into_iter().all(f64::is_finite)
            || (width - self.width).abs() > 0.5
            || (height - self.height).abs() > 0.5
        {
            return None;
        }
        if let Some(tab) = self.hit(x, y, width, height) {
            return Some(TargetHit {
                placement: DropTarget {
                    pinboard_id: tab.id,
                    anchor: None,
                    after: true,
                },
                visual: TargetVisual::Tab,
            });
        }
        let timeline = self.timeline.as_ref()?;
        let bounds = timeline.bounds;
        if !bounds.contains(x, y) {
            return None;
        }
        // Hovering a dragged item is a no-op, not a success insertion marker.
        if timeline
            .cards
            .iter()
            .any(|c| c.bounds.contains(x, y) && dragged.contains(&c.id))
        {
            return None;
        }
        let card = timeline
            .cards
            .iter()
            .filter(|c| !dragged.contains(&c.id))
            .find(|c| x < c.bounds.x + c.bounds.width)
            .or_else(|| {
                timeline
                    .cards
                    .iter()
                    .rev()
                    .find(|c| !dragged.contains(&c.id))
            });
        let (anchor, after, line_x) = if let Some(card) = card {
            let after = x >= card.bounds.x + card.bounds.width / 2.0;
            (
                Some(card.id),
                after,
                if after {
                    card.bounds.x + card.bounds.width + 5.0
                } else {
                    card.bounds.x - 5.0
                },
            )
        } else if timeline.cards.is_empty() {
            (None, true, bounds.x + 8.0)
        } else {
            return None;
        };
        Some(TargetHit {
            placement: DropTarget {
                pinboard_id: timeline.pinboard_id,
                anchor,
                after,
            },
            visual: TargetVisual::Insertion {
                x: line_x.clamp(bounds.x + 2.0, bounds.x + bounds.width - 2.0),
                y: bounds.y + 4.0,
                height: (bounds.height - 8.0).max(1.0),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with_timeline() -> DragLayout {
        let mut l = layout();
        l.timeline = Some(DragTimeline {
            pinboard_id: l.tabs[0].id,
            bounds: DragRect {
                x: 20.0,
                y: 48.0,
                width: 1400.0,
                height: 185.0,
            },
            cards: (0..4)
                .map(|i| DragCard {
                    id: ClipId::new(),
                    bounds: DragRect {
                        x: 22.0 + 168.0 * f64::from(i),
                        y: 51.0,
                        width: 156.0,
                        height: 170.0,
                    },
                })
                .collect(),
        });
        l
    }
    #[test]
    fn targets_cover_tabs_card_halves_gaps_and_end_but_not_toolbar_or_source() {
        let l = with_timeline();
        let t = l.timeline.as_ref().expect("timeline");
        let dragged = [t.cards[0].id];
        let target = |x, y| l.target(x, y, 1440.0, 248.0, &dragged);
        assert_eq!(target(641.0, 25.0).expect("tab").visual, TargetVisual::Tab);
        for (x, anchor, after) in [
            (200.0, t.cards[1].id, false),
            (330.0, t.cards[1].id, true),
            (350.0, t.cards[2].id, false),
            (1000.0, t.cards[3].id, true),
        ] {
            let hit = target(x, 125.0).expect("insertion");
            assert_eq!(
                hit.placement,
                DropTarget {
                    pinboard_id: t.pinboard_id,
                    anchor: Some(anchor),
                    after
                }
            );
            assert!(matches!(hit.visual, TargetVisual::Insertion { .. }));
        }
        for (x, y) in [(50.0, 20.0), (1100.0, 25.0), (40.0, 125.0), (1430.0, 125.0)] {
            assert!(target(x, y).is_none());
        }
        assert!(l.target(210.0, 125.0, 1440.0, 148.0, &dragged).is_none());
    }
    #[test]
    fn clipped_cards_keep_their_original_midpoint_and_empty_board_accepts_append() {
        let mut l = with_timeline();
        let t = l.timeline.as_mut().expect("timeline");
        t.cards[0].bounds.x = -100.0;
        t.cards[1].bounds.x = 68.0;
        let first = t.cards[0].id;
        let hit = l
            .target(30.0, 125.0, 1440.0, 248.0, &[])
            .expect("clipped card");
        assert_eq!(hit.placement.anchor, Some(first));
        assert!(hit.placement.after);
        l.timeline.as_mut().expect("timeline").cards.clear();
        assert_eq!(
            l.target(200.0, 125.0, 1440.0, 248.0, &[])
                .expect("empty append")
                .placement
                .anchor,
            None
        );
        l.timeline = None;
        assert!(
            l.target(200.0, 125.0, 1440.0, 248.0, &[]).is_none(),
            "history/search cannot reorder"
        );
        assert!(
            l.target(640.0, 25.0, 1440.0, 248.0, &[]).is_some(),
            "explicit tab remains usable"
        );
    }
    #[test]
    fn timeline_geometry_rejects_duplicates_overlap_offscreen_and_excess_targets() {
        let mut l = with_timeline();
        let t = l.timeline.as_mut().expect("timeline");
        t.cards[1].id = t.cards[0].id;
        assert!(!l.valid());
        l = with_timeline();
        l.timeline.as_mut().expect("timeline").cards[1].bounds.x = 30.0;
        assert!(!l.valid());
        l = with_timeline();
        l.timeline.as_mut().expect("timeline").bounds.x = -1.0;
        assert!(!l.valid());
        l = with_timeline();
        l.timeline.as_mut().expect("timeline").cards[0].bounds.width = f64::INFINITY;
        assert!(!l.valid());
        l = with_timeline();
        let card = l.timeline.as_ref().expect("timeline").cards[0].clone();
        l.timeline.as_mut().expect("timeline").cards = vec![card; 65];
        assert!(!l.valid());
    }
    fn layout() -> DragLayout {
        DragLayout {
            width: 1440.0,
            height: 248.0,
            tabs: vec![DragTab {
                id: PinboardId::new(),
                x: 612.0,
                y: 11.0,
                width: 52.0,
                height: 27.0,
            }],
            timeline: None,
        }
    }
    #[test]
    fn feedback_uses_view_points_and_rejects_gaps_resize_and_nonfinite_values() {
        let l = layout();
        assert!(l.hit(641.25, 32.3, 1440.0, 248.0).is_some());
        assert!(l.hit(664.0, 32.3, 1440.0, 248.0).is_none());
        assert!(l.hit(1282.5, 64.6, 2880.0, 496.0).is_none());
        assert!(l.hit(641.0, 32.0, 1400.0, 248.0).is_none());
        assert!(l.hit(f64::NAN, 32.0, 1440.0, 248.0).is_none());
    }
    #[test]
    fn feedback_rejects_unbounded_duplicate_and_offscreen_geometry() {
        let mut l = layout();
        l.tabs.push(l.tabs[0].clone());
        assert!(!l.valid());
        l.tabs.pop();
        l.tabs[0].x = -1.0;
        assert!(!l.valid());
        l.tabs[0].x = 1400.0;
        assert!(!l.valid());
        l.tabs[0].x = f64::INFINITY;
        assert!(!l.valid());
        l = layout();
        l.tabs = (0..65)
            .map(|_| DragTab {
                id: PinboardId::new(),
                ..l.tabs[0].clone()
            })
            .collect();
        assert!(!l.valid());
    }
}
