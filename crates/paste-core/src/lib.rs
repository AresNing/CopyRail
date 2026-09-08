#![forbid(unsafe_code)]

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use paste_domain::{CapturePreferences, CapturedItem, ClipId, DeviceMetadata, RetentionPolicy};
use paste_platform::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
    IgnoreReason,
};
use paste_storage::{SqliteStore, StorageError};
use thiserror::Error;

pub struct CaptureCoordinator<S> {
    source: S,
    store: Arc<SqliteStore>,
    privacy_policy: ClipboardPrivacyPolicy,
    limits: CaptureLimits,
    device: DeviceMetadata,
    pause: PauseController,
    retention: RetentionPolicy,
    pending: Option<PendingCapture>,
    discarded: Option<CaptureEvent>,
}

struct PendingCapture {
    change_count: i64,
    items: Vec<CapturedItem>,
    privacy_policy: ClipboardPrivacyPolicy,
}

/// A control processed after reading invalidates that in-flight snapshot even
/// if a later control immediately resumes capture or restores the old policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureInterruption {
    Pause,
    Privacy,
}

impl CaptureInterruption {
    fn discarded(self, change_count: i64, item_count: usize) -> CaptureEvent {
        match self {
            Self::Pause => CaptureEvent::DiscardedWhilePaused {
                change_count,
                item_count,
            },
            Self::Privacy => CaptureEvent::DiscardedForPrivacyChange {
                change_count,
                item_count,
            },
        }
    }
}

impl<S: ClipboardSource> CaptureCoordinator<S> {
    #[must_use]
    pub fn new(source: S, store: Arc<SqliteStore>, device: DeviceMetadata) -> Self {
        Self {
            source,
            store,
            privacy_policy: ClipboardPrivacyPolicy::secure_default(),
            limits: CaptureLimits::default(),
            device,
            pause: PauseController::default(),
            retention: RetentionPolicy::default(),
            pending: None,
            discarded: None,
        }
    }

    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    #[must_use]
    pub fn privacy_policy(&self) -> &ClipboardPrivacyPolicy {
        &self.privacy_policy
    }

    pub fn privacy_policy_mut(&mut self) -> &mut ClipboardPrivacyPolicy {
        self.discard_pending_for_privacy_change();
        &mut self.privacy_policy
    }

    pub fn set_limits(&mut self, limits: CaptureLimits) {
        self.limits = limits;
    }

    pub fn set_retention(&mut self, retention: RetentionPolicy) -> Result<(), CoreError> {
        retention.validate()?;
        self.retention = retention;
        Ok(())
    }

    pub fn set_preferences(&mut self, preferences: CapturePreferences) -> Result<(), CoreError> {
        let preferences = preferences.normalized()?;
        self.retention = preferences.retention;
        let mut policy = ClipboardPrivacyPolicy::secure_default();
        for bundle_id in preferences.excluded_bundle_ids {
            policy.exclude_bundle_id(bundle_id);
        }
        if policy != self.privacy_policy {
            self.discard_pending_for_privacy_change();
        }
        self.privacy_policy = policy;
        Ok(())
    }

    pub fn pause_for(&mut self, now: DateTime<Utc>, duration: Duration) {
        self.pause.pause_until(now + duration);
        self.discard_pending_for_pause();
    }

    pub fn pause_indefinitely(&mut self) {
        self.pause.pause_indefinitely();
        self.discard_pending_for_pause();
    }

    pub fn resume(&mut self) -> Result<(), CoreError> {
        if self.pause.is_paused() {
            // A copy may have arrived after the last paused poll. Establish a
            // content-free boundary before acknowledging resume. A failure
            // leaves the original pause (including its deadline) intact.
            self.source.discard_current()?;
            self.pause.resume();
        }
        Ok(())
    }

    #[must_use]
    pub fn pause_state(&self) -> PauseState {
        self.pause.state()
    }

    #[must_use]
    pub fn pending_item_count(&self) -> usize {
        self.pending
            .as_ref()
            .map_or(0, |pending| pending.items.len())
    }

    fn discard_pending_for_pause(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.discarded = Some(CaptureEvent::DiscardedWhilePaused {
                change_count: pending.change_count,
                item_count: pending.items.len(),
            });
        }
    }

    fn discard_pending_for_privacy_change(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.discarded = Some(CaptureEvent::DiscardedForPrivacyChange {
                change_count: pending.change_count,
                item_count: pending.items.len(),
            });
        }
    }

    fn persist_pending(&mut self, now: DateTime<Utc>) -> Result<CaptureEvent, CoreError> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(CaptureEvent::Unchanged);
        };
        if pending.privacy_policy != self.privacy_policy {
            // Privacy changes invalidate uncommitted memory, including marker
            // metadata not represented by an item's persisted content types.
            let event = CaptureEvent::DiscardedForPrivacyChange {
                change_count: pending.change_count,
                item_count: pending.items.len(),
            };
            self.pending = None;
            return Ok(event);
        }
        if let Err(error) = validate_snapshot(&pending.items, self.limits) {
            self.pending = None;
            return Err(error);
        }
        // Keep this exact owned snapshot on failure. Do not reread the current
        // clipboard or insert twice after a separately failed cleanup.
        let stored =
            self.store
                .insert_captures_with_retention(&pending.items, self.retention, now)?;
        let change_count = pending.change_count;
        self.pending = None;
        Ok(CaptureEvent::Stored {
            change_count,
            clip_ids: stored.into_iter().map(|item| item.id).collect(),
        })
    }

    pub fn tick(&mut self, now: DateTime<Utc>) -> Result<CaptureEvent, CoreError> {
        self.tick_with_checkpoint(now, |_| None)
    }

    /// Service controls can arrive while a platform read blocks. Apply them
    /// before any newly-read content or existing pending batch is persisted.
    /// The caller acknowledges controls here, on the same storage worker; a
    /// request arriving after this checkpoint stays pending until the next one.
    pub fn tick_with_checkpoint(
        &mut self,
        now: DateTime<Utc>,
        checkpoint: impl FnOnce(&mut Self) -> Option<CaptureInterruption>,
    ) -> Result<CaptureEvent, CoreError> {
        if self.pause.has_expired(now)
            && let Err(error) = self.resume()
        {
            checkpoint(self);
            return Err(error);
        }
        let paused = self.pause.is_paused();
        if let Some(discarded) = self.discarded.take() {
            checkpoint(self);
            return Ok(discarded);
        }
        if self.pending.is_some() {
            let interruption = checkpoint(self);
            if let Some(discarded) = self.discarded.take() {
                return Ok(discarded);
            }
            if let Some(reason) = interruption
                && let Some(pending) = self.pending.take()
            {
                return Ok(reason.discarded(pending.change_count, pending.items.len()));
            }
            return self.persist_pending(now);
        }
        let poll = self
            .source
            .poll(&self.privacy_policy, self.limits, &self.device);
        let interruption = checkpoint(self);
        let paused = paused || self.pause.is_paused();
        match poll? {
            ClipboardPoll::Unchanged => Ok(CaptureEvent::Unchanged),
            ClipboardPoll::Ignored {
                change_count,
                reason,
            } => Ok(CaptureEvent::Ignored {
                change_count,
                reason,
            }),
            ClipboardPoll::Captured {
                change_count,
                items,
            } => {
                if let Some(reason) = interruption {
                    return Ok(reason.discarded(change_count, items.len()));
                }
                if paused {
                    return Ok(CaptureEvent::DiscardedWhilePaused {
                        change_count,
                        item_count: items.len(),
                    });
                }
                validate_snapshot(&items, self.limits)?;
                self.pending = Some(PendingCapture {
                    change_count,
                    items,
                    privacy_policy: self.privacy_policy.clone(),
                });
                self.persist_pending(now)
            }
        }
    }

    pub fn apply_retention(&self, now: DateTime<Utc>) -> Result<usize, CoreError> {
        self.store
            .apply_retention(self.retention, now)
            .map_err(Into::into)
    }
}

/// Bound the retained batch even when a non-macOS source violates its contract.
fn validate_snapshot(items: &[CapturedItem], limits: CaptureLimits) -> Result<(), CoreError> {
    if items.len() > limits.max_items {
        return Err(ClipboardError::TooManyItems {
            actual: items.len(),
            limit: limits.max_items,
        }
        .into());
    }
    let mut total = 0usize;
    for item in items {
        item.validate()?;
        if !item.flags.should_persist() {
            return Err(StorageError::CaptureRejected.into());
        }
        if item.representations.len() > limits.max_representations_per_item {
            return Err(ClipboardError::TooManyRepresentations {
                actual: item.representations.len(),
                limit: limits.max_representations_per_item,
            }
            .into());
        }
        for representation in &item.representations {
            let len = representation.bytes.len();
            if len > limits.max_representation_bytes {
                return Err(ClipboardError::RepresentationTooLarge {
                    uti: representation
                        .native_type
                        .clone()
                        .unwrap_or_else(|| representation.kind.storage_key()),
                    actual: len,
                    limit: limits.max_representation_bytes,
                }
                .into());
            }
            total = total.checked_add(len).ok_or(ClipboardError::SizeOverflow)?;
            if total > limits.max_total_bytes {
                return Err(ClipboardError::SnapshotTooLarge {
                    actual: total,
                    limit: limits.max_total_bytes,
                }
                .into());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PauseState {
    #[default]
    Running,
    Until(DateTime<Utc>),
    Indefinite,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PauseController {
    state: PauseState,
}

impl PauseController {
    fn pause_until(&mut self, until: DateTime<Utc>) {
        self.state = PauseState::Until(until);
    }

    fn pause_indefinitely(&mut self) {
        self.state = PauseState::Indefinite;
    }

    fn resume(&mut self) {
        self.state = PauseState::Running;
    }

    const fn state(&self) -> PauseState {
        self.state
    }

    fn has_expired(&self, now: DateTime<Utc>) -> bool {
        matches!(self.state, PauseState::Until(until) if now >= until)
    }

    fn is_paused(&self) -> bool {
        self.state != PauseState::Running
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureEvent {
    Unchanged,
    Ignored {
        change_count: i64,
        reason: IgnoreReason,
    },
    DiscardedWhilePaused {
        change_count: i64,
        item_count: usize,
    },
    DiscardedForPrivacyChange {
        change_count: i64,
        item_count: usize,
    },
    Stored {
        change_count: i64,
        clip_ids: Vec<ClipId>,
    },
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Clipboard(#[from] ClipboardError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Domain(#[from] paste_domain::DomainError),
}
