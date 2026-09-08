//! Read-only menu snapshot. Actual mutations must still recheck permissions.
use super::*;

pub struct ClipActionItem {
    pub clip: ClipItem,
    pub boards: Vec<PinboardId>,
    pub writable: bool,
    pub move_restricted: bool,
}

pub struct ClipActionBoard {
    pub id: PinboardId,
    pub name: String,
    pub writable: bool,
}

pub struct ClipActionContext {
    pub clips: Vec<ClipActionItem>,
    pub boards: Vec<ClipActionBoard>,
}

fn writable(result: Result<(), StorageError>) -> Result<bool, StorageError> {
    match result {
        Ok(()) => Ok(true),
        Err(StorageError::ReadOnlyPinboardShare) => Ok(false),
        Err(error) => Err(error),
    }
}

impl SqliteStore {
    pub fn clip_action_context(&self, ids: &[ClipId]) -> Result<ClipActionContext, StorageError> {
        if ids.is_empty()
            || ids.len() > 200
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        {
            return Err(StorageError::InvalidPinboardPlacement);
        }
        let connection = self.lock()?;
        // One lock and read transaction: no writes, enqueues or payload exports.
        let tx = connection.unchecked_transaction()?;
        let mut clips = Vec::with_capacity(ids.len());
        for id in ids {
            let key = id.to_string();
            ensure_exists(&tx, "clips", &key)?;
            let mut statement = tx.prepare(
                "SELECT pinboard_id FROM pinboard_items WHERE clip_id = ?1 ORDER BY pinboard_id",
            )?;
            let boards = statement
                .query_map([&key], |row| row.get::<_, String>(0))?
                .map(|row| {
                    PinboardId::from_str(&row?).map_err(|error| corrupt("pinboard id", error))
                })
                .collect::<Result<Vec<_>, _>>()?;
            clips.push(ClipActionItem {
                clip: load_clip(&tx, &key)?,
                boards,
                writable: writable(ensure_clip_share_writable(&tx, &key))?,
                move_restricted: share_materialize::isolated_origin(&tx, &key)?.is_some(),
            });
        }
        let boards = {
            let mut statement =
                tx.prepare("SELECT id, name FROM pinboards ORDER BY sort_order, created_at_ms")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .map(|row| {
                    let (id, name) = row?;
                    let id =
                        PinboardId::from_str(&id).map_err(|error| corrupt("pinboard id", error))?;
                    Ok(ClipActionBoard {
                        id,
                        name,
                        writable: writable(ensure_pinboard_share_writable(&tx, id))?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?
        };
        tx.commit()?;
        Ok(ClipActionContext { clips, boards })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seeded() -> (SqliteStore, ClipId, PinboardId) {
        let store = SqliteStore::open_in_memory().expect("store");
        let device = store.get_or_create_device("Synthetic").expect("device");
        let board = store.create_pinboard("Work", "#ff9500").expect("board");
        let clip = store
            .create_textual_item_in_pinboard(
                ContentKind::Text,
                &"Synthetic🦀".repeat(100),
                SourceApplication::unknown(),
                device,
                Some(board.id),
            )
            .expect("clip");
        (store, clip.id, board.id)
    }
    #[test]
    fn menu_context_preserves_order_membership_and_does_not_write() {
        let (store, id, board) = seeded();
        let before: i64 = store
            .lock()
            .expect("lock")
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .expect("count");
        let snapshot = store.clip_action_context(&[id]).expect("context");
        assert_eq!(snapshot.clips[0].clip.id, id);
        assert_eq!(
            snapshot.clips[0].clip.searchable_text,
            "Synthetic🦀".repeat(100)
        );
        let history = store
            .list_history(SearchPage::new(200, 0))
            .expect("history");
        assert_eq!(
            history[0].searchable_text,
            snapshot.clips[0].clip.searchable_text
        );
        assert_eq!(snapshot.clips[0].boards, vec![board]);
        assert!(snapshot.clips[0].writable);
        assert!(!snapshot.clips[0].move_restricted);
        assert_eq!(snapshot.boards[0].name, "Work");
        let after: i64 = store
            .lock()
            .expect("lock")
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .expect("count");
        assert_eq!(before, after);
    }
    #[test]
    fn menu_context_rejects_empty_duplicate_missing_and_deleted_items() {
        let (store, id, _) = seeded();
        assert!(store.clip_action_context(&[]).is_err());
        assert!(store.clip_action_context(&[id, id]).is_err());
        assert!(store.clip_action_context(&[id, ClipId::new()]).is_err());
        store.delete_clip(id, Utc::now()).expect("delete");
        assert!(store.clip_action_context(&[id]).is_err());
    }
    #[test]
    fn permission_denial_is_disabled_but_storage_failure_is_not_hidden() {
        assert!(!writable(Err(StorageError::ReadOnlyPinboardShare)).expect("disabled"));
        assert!(writable(Err(StorageError::NotFound)).is_err());
    }

    #[test]
    fn actual_shared_permission_changes_are_reflected_without_writes() {
        let (store, id, local) = seeded();
        let shared = PinboardId::new();
        let register = |permission| {
            store
                .register_accepted_pinboard_share(
                    shared,
                    "Synthetic Shared",
                    "#34c759",
                    &format!("PasteShare_{shared}"),
                    "test_owner",
                    "zone_share",
                    "https://www.icloud.com/share/synthetic",
                    permission,
                )
                .expect("synthetic registration only")
        };
        register(PinboardSharePermission::ReadWrite);
        store.pin_clip(shared, id).expect("move");
        register(PinboardSharePermission::ReadOnly);
        let before = store.pending_sync_change_count().expect("count");
        let context = store.clip_action_context(&[id]).expect("snapshot");
        assert!(!context.clips[0].writable);
        assert_eq!(context.clips[0].boards, vec![shared]);
        assert!(
            !context
                .boards
                .iter()
                .find(|board| board.id == shared)
                .expect("shared")
                .writable
        );
        assert!(
            context
                .boards
                .iter()
                .find(|board| board.id == local)
                .expect("local")
                .writable
        );
        assert!(store.pin_clip(local, id).is_err());
        assert_eq!(before, store.pending_sync_change_count().expect("count"));
    }
}
