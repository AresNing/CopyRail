use std::sync::{Arc, Mutex};

use paste_storage::SqliteStore;

#[cfg(feature = "cloudkit")]
use std::{
    sync::mpsc::{self, Sender},
    thread,
    time::Duration,
};

#[cfg(feature = "cloudkit")]
use paste_cloudkit::{
    CloudDownloadBatch, CloudKitAccountStatus, CloudKitClient, CloudKitConfiguration,
    CloudKitError, CloudShareRole, CloudUploadPlan,
};

#[cfg(feature = "cloudkit")]
const CLOUDKIT_CONTAINER_IDENTIFIER: &str = "iCloud.io.pasters.desktop";
#[cfg(feature = "cloudkit")]
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(feature = "cloudkit")]
const ACCOUNT_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(feature = "cloudkit")]
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(feature = "cloudkit")]
const MAX_BATCHES_PER_CYCLE: usize = 8;

#[derive(Clone, Debug, Default)]
pub struct CloudSyncRuntimeStatus {
    pub transport_configured: bool,
    pub syncing: bool,
    pub last_success_at_ms: Option<i64>,
    pub blocked_reason: Option<String>,
}

pub struct CloudSyncService {
    status: Arc<Mutex<CloudSyncRuntimeStatus>>,
    #[cfg(feature = "cloudkit")]
    sender: Option<Sender<WorkerMessage>>,
}

impl CloudSyncService {
    pub fn isolated() -> Self {
        Self {
            status: Arc::new(Mutex::new(CloudSyncRuntimeStatus {
                blocked_reason: Some("隔离验证不创建 CloudKit 客户端或同步线程。".into()),
                ..Default::default()
            })),
            #[cfg(feature = "cloudkit")]
            sender: None,
        }
    }
    pub fn spawn(store: Arc<SqliteStore>) -> Self {
        #[cfg(feature = "cloudkit")]
        {
            Self::spawn_enabled(store)
        }
        #[cfg(not(feature = "cloudkit"))]
        {
            let _ = store;
            Self {
                status: Arc::new(Mutex::new(CloudSyncRuntimeStatus {
                    blocked_reason: Some(
                        "当前构建未包含 CloudKit 传输；本地队列不会被标记为已发送。".into(),
                    ),
                    ..CloudSyncRuntimeStatus::default()
                })),
            }
        }
    }

    #[cfg(feature = "cloudkit")]
    fn spawn_enabled(store: Arc<SqliteStore>) -> Self {
        let configuration = CloudKitConfiguration::new(CLOUDKIT_CONTAINER_IDENTIFIER)
            .expect("built-in CloudKit container identifier must be valid");
        let client = CloudKitClient::new(configuration);
        let entitled = client.has_required_entitlements();
        let status = Arc::new(Mutex::new(CloudSyncRuntimeStatus {
            transport_configured: entitled,
            blocked_reason: (!entitled).then(|| {
                "当前签名未获得 CopyRail CloudKit 容器 entitlement；不会连接 iCloud。".into()
            }),
            ..CloudSyncRuntimeStatus::default()
        }));
        let (sender, receiver) = mpsc::channel();
        let worker_status = Arc::clone(&status);
        thread::Builder::new()
            .name("pasters-cloudkit-sync".into())
            .spawn(move || {
                let mut shared_cursor = None;
                loop {
                    match receiver.recv_timeout(IDLE_POLL_INTERVAL) {
                        Ok(WorkerMessage::Stop) => break,
                        Ok(WorkerMessage::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    let enabled = match store.cloud_sync_enabled() {
                        Ok(enabled) => enabled,
                        Err(error) => {
                            set_blocked(&worker_status, format!("读取同步设置失败：{error}"));
                            continue;
                        }
                    };
                    if !enabled {
                        set_idle(&worker_status);
                        continue;
                    }
                    if !entitled {
                        set_blocked(
                            &worker_status,
                            "当前签名未获得 CopyRail CloudKit 容器 entitlement；本地队列会保留。"
                                .into(),
                        );
                        continue;
                    }
                    set_syncing(&worker_status);
                    if let Err(error) = run_sync_cycle(&store, &client, &mut shared_cursor) {
                        set_blocked(&worker_status, user_facing_error(&error));
                    } else {
                        set_succeeded(&worker_status);
                    }
                }
            })
            .expect("CloudKit worker thread must be spawnable");
        Self {
            status,
            sender: Some(sender),
        }
    }

    pub fn wake(&self) {
        #[cfg(feature = "cloudkit")]
        {
            if let Some(sender) = &self.sender {
                let _ = sender.send(WorkerMessage::Wake);
            }
        }
    }

    pub fn status(&self) -> CloudSyncRuntimeStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| CloudSyncRuntimeStatus {
                blocked_reason: Some("同步状态暂时不可用。".into()),
                ..CloudSyncRuntimeStatus::default()
            })
    }
}

#[cfg(feature = "cloudkit")]
impl Drop for CloudSyncService {
    fn drop(&mut self) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(WorkerMessage::Stop);
        }
    }
}

#[cfg(feature = "cloudkit")]
enum WorkerMessage {
    Wake,
    Stop,
}

#[cfg(feature = "cloudkit")]
fn run_sync_cycle(
    store: &SqliteStore,
    client: &CloudKitClient,
    shared_cursor: &mut Option<uuid::Uuid>,
) -> Result<(), CloudSyncWorkerError> {
    match client.account_status(ACCOUNT_STATUS_TIMEOUT)? {
        CloudKitAccountStatus::Available => {}
        status => return Err(CloudSyncWorkerError::AccountUnavailable(status)),
    }

    // The scopes are independent: a rejected private page must not prevent
    // other registered shares from receiving their own valid pages.
    let private = run_private_cycle(store, client);
    let shared = pull_shared_pages(store, client, shared_cursor);
    private?;
    shared?;
    let pending = store.pending_shared_download_count()?;
    let has_active_shares = store
        .list_pinboard_shares()?
        .iter()
        .any(|share| share.state == paste_storage::PinboardShareState::Active);
    if pending > 0 || has_active_shares {
        return Err(CloudSyncWorkerError::SharedMaterializationPending(pending));
    }
    Ok(())
}

#[cfg(feature = "cloudkit")]
fn run_private_cycle(
    store: &SqliteStore,
    client: &CloudKitClient,
) -> Result<(), CloudSyncWorkerError> {
    pull_private_changes(store, client)?;
    for _ in 0..MAX_BATCHES_PER_CYCLE {
        if !store.cloud_sync_enabled()? {
            return Ok(());
        }
        let plan = prepare_fitting_batch(store)?;
        if plan.records.is_empty() {
            break;
        }
        let receipt = client.upload_private(&plan, UPLOAD_TIMEOUT)?;
        store.acknowledge_sync_changes(&receipt.acknowledge_operation_ids)?;
    }
    Ok(())
}

#[cfg(feature = "cloudkit")]
fn pull_shared_pages(
    store: &SqliteStore,
    client: &CloudKitClient,
    cursor: &mut Option<uuid::Uuid>,
) -> Result<(), CloudSyncWorkerError> {
    use paste_storage::PinboardShareRole;
    let shares = shared_download_order(store.list_pinboard_shares()?, *cursor);
    let mut first_error = None;
    // One page per board, at most eight boards per cycle. Advance the cursor on
    // failed attempts too, so one bad/large zone cannot starve other shares.
    for share in shares {
        if !store.cloud_sync_enabled()? {
            break;
        }
        *cursor = Some(share.id);
        let result = pull_one_shared_page_with(store, share, |share| {
            let owner = share
                .owner_name
                .as_deref()
                .ok_or(CloudKitError::InvalidShareMetadata)?;
            let role = match share.role {
                PinboardShareRole::Owner => CloudShareRole::Owner,
                PinboardShareRole::Participant => CloudShareRole::Participant,
            };
            client.fetch_pinboard_share_changes(
                &share.zone_name,
                owner,
                role,
                share.server_change_token.as_deref(),
                UPLOAD_TIMEOUT,
            )
        });
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(feature = "cloudkit")]
fn shared_download_order(
    mut shares: Vec<paste_storage::PinboardShare>,
    cursor: Option<uuid::Uuid>,
) -> Vec<paste_storage::PinboardShare> {
    shares.retain(|share| share.state == paste_storage::PinboardShareState::Active);
    shares.sort_by_key(|share| share.id);
    if let Some(cursor) = cursor {
        let start = shares.partition_point(|share| share.id <= cursor);
        if start < shares.len() {
            shares.rotate_left(start);
        }
    }
    shares.truncate(MAX_BATCHES_PER_CYCLE);
    shares
}

#[cfg(feature = "cloudkit")]
fn pull_one_shared_page_with(
    store: &SqliteStore,
    mut share: paste_storage::PinboardShare,
    mut fetch: impl FnMut(&paste_storage::PinboardShare) -> Result<CloudDownloadBatch, CloudKitError>,
) -> Result<(), CloudSyncWorkerError> {
    if !store.cloud_sync_enabled()? {
        return Ok(());
    }
    // Resume durable local work even when the next network request fails.
    store.materialize_pinboard_share(share.id, 250)?;
    let batch = match fetch(&share) {
        Ok(batch) => batch,
        Err(CloudKitError::ServerChangeTokenExpired) if share.server_change_token.is_some() => {
            store.reset_pinboard_share_download_checkpoint(&share)?;
            share.server_change_token = None;
            if !store.cloud_sync_enabled()? {
                return Ok(());
            }
            fetch(&share)?
        }
        Err(error) => return Err(error.into()),
    };
    // The download and projection are separate durable transactions. A failed
    // projection leaves its operation unconsumed for restart/retry.
    if !store.cloud_sync_enabled()? {
        return Ok(());
    }
    store.stage_pinboard_share_sync_page(
        &share,
        &batch.envelopes,
        &batch.blobs,
        &batch.server_change_token,
    )?;
    store.materialize_pinboard_share(share.id, 250)?;
    Ok(())
}

#[cfg(feature = "cloudkit")]
fn pull_private_changes(
    store: &SqliteStore,
    client: &CloudKitClient,
) -> Result<(), CloudSyncWorkerError> {
    let mut token = store.load_sync_token("private")?;
    let mut reset_expired_token = false;
    for _ in 0..MAX_BATCHES_PER_CYCLE {
        if !store.cloud_sync_enabled()? {
            break;
        }
        let batch = match client.fetch_private_changes(token.as_deref(), UPLOAD_TIMEOUT) {
            Ok(batch) => batch,
            Err(CloudKitError::ServerChangeTokenExpired)
                if token.is_some() && !reset_expired_token =>
            {
                store.clear_sync_token("private")?;
                token = None;
                reset_expired_token = true;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        store.apply_private_sync_page(
            &batch.envelopes,
            &batch.blobs,
            &batch.server_change_token,
        )?;
        token = Some(batch.server_change_token);
        if !batch.more_coming {
            break;
        }
    }
    Ok(())
}

#[cfg(feature = "cloudkit")]
fn prepare_fitting_batch(store: &SqliteStore) -> Result<CloudUploadPlan, CloudSyncWorkerError> {
    let mut limit = 48;
    loop {
        let batch = store.prepare_private_sync_batch(limit)?;
        match CloudUploadPlan::from_prepared(&batch) {
            Ok(plan) => return Ok(plan),
            Err(CloudKitError::TooManyRecords(_) | CloudKitError::BatchTooLarge) if limit > 1 => {
                limit /= 2;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(feature = "cloudkit")]
#[derive(Debug, thiserror::Error)]
enum CloudSyncWorkerError {
    #[error("{0} shared operations are durably downloaded but awaiting local materialization")]
    SharedMaterializationPending(usize),
    #[error("storage error: {0}")]
    Storage(#[from] paste_storage::StorageError),
    #[error("CloudKit error: {0}")]
    CloudKit(#[from] CloudKitError),
    #[error("iCloud account is unavailable: {0:?}")]
    AccountUnavailable(CloudKitAccountStatus),
}

#[cfg(feature = "cloudkit")]
fn user_facing_error(error: &CloudSyncWorkerError) -> String {
    match error {
        CloudSyncWorkerError::SharedMaterializationPending(count) => {
            format!(
                "共享内容已接入本地，另有 {count} 项下载待整合；共享上传调度及退出清理仍未完成，不能视为共享同步完成。"
            )
        }
        CloudSyncWorkerError::AccountUnavailable(CloudKitAccountStatus::NoAccount) => {
            "没有可用的 iCloud 账户；请先在系统设置中登录。".into()
        }
        CloudSyncWorkerError::AccountUnavailable(CloudKitAccountStatus::Restricted) => {
            "当前账户或设备限制了 iCloud 访问。".into()
        }
        CloudSyncWorkerError::AccountUnavailable(CloudKitAccountStatus::TemporarilyUnavailable) => {
            "iCloud 暂时不可用，队列会在本机保留并稍后重试。".into()
        }
        CloudSyncWorkerError::AccountUnavailable(CloudKitAccountStatus::CouldNotDetermine) => {
            "暂时无法确认 iCloud 账户状态，稍后会自动重试。".into()
        }
        CloudSyncWorkerError::AccountUnavailable(CloudKitAccountStatus::Available) => {
            "iCloud 账户状态异常，稍后会自动重试。".into()
        }
        CloudSyncWorkerError::CloudKit(CloudKitError::AccountStatusRequestFailed) => {
            "CloudKit 拒绝账户检查；请确认应用签名与 iCloud entitlement。".into()
        }
        CloudSyncWorkerError::CloudKit(CloudKitError::ZonePreparationFailed) => {
            "无法准备 CloudKit 私有数据库区域；请确认容器配置与 entitlement。".into()
        }
        CloudSyncWorkerError::CloudKit(CloudKitError::UploadFailed) => {
            "CloudKit 未确认本批上传，本地队列已保留并会重试。".into()
        }
        other => format!("同步暂时不可用：{other}"),
    }
}

#[cfg(feature = "cloudkit")]
fn update_status(
    status: &Arc<Mutex<CloudSyncRuntimeStatus>>,
    update: impl FnOnce(&mut CloudSyncRuntimeStatus),
) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}

#[cfg(feature = "cloudkit")]
fn set_syncing(status: &Arc<Mutex<CloudSyncRuntimeStatus>>) {
    update_status(status, |status| {
        status.syncing = true;
        status.blocked_reason = None;
    });
}

#[cfg(feature = "cloudkit")]
fn set_blocked(status: &Arc<Mutex<CloudSyncRuntimeStatus>>, reason: String) {
    update_status(status, |status| {
        status.syncing = false;
        status.blocked_reason = Some(reason);
    });
}

#[cfg(feature = "cloudkit")]
fn set_succeeded(status: &Arc<Mutex<CloudSyncRuntimeStatus>>) {
    update_status(status, |status| {
        status.syncing = false;
        status.last_success_at_ms = Some(chrono::Utc::now().timestamp_millis());
        status.blocked_reason = None;
    });
}

#[cfg(feature = "cloudkit")]
fn set_idle(status: &Arc<Mutex<CloudSyncRuntimeStatus>>) {
    update_status(status, |status| {
        status.syncing = false;
        status.blocked_reason = None;
    });
}

#[cfg(all(test, feature = "cloudkit"))]
mod shared_download_tests {
    use super::*;
    use paste_domain::PinboardId;
    use paste_storage::{PinboardShare, PinboardSharePermission, PinboardShareState};

    fn fixture() -> (SqliteStore, PinboardShare) {
        let store = SqliteStore::open_in_memory().expect("isolated store");
        store
            .set_cloud_sync_enabled(true)
            .expect("local setting only");
        let board = PinboardId::new();
        let share = store
            .register_accepted_pinboard_share(
                board,
                "Synthetic",
                "#34c759",
                &format!("PasteShare_{board}"),
                "synthetic_owner",
                "zone_share",
                "https://www.icloud.com/share/synthetic",
                PinboardSharePermission::ReadOnly,
            )
            .expect("synthetic registration");
        (store, share)
    }

    #[test]
    fn shared_worker_projects_received_content_and_drains_before_a_later_network_failure() {
        let (store, share) = fixture();
        let mut board = store.list_pinboards().expect("synthetic board")[0].clone();
        board.name = "Downloaded presentation".into();
        let device = paste_domain::DeviceId::new();
        let mut version = paste_sync::VersionVector::default();
        version.increment(device);
        let change = paste_sync::SyncChange::new(
            paste_sync::SyncEntity {
                kind: paste_sync::SyncEntityKind::Pinboard,
                id: board.id.to_string(),
            },
            paste_sync::SyncChangeKind::Save,
            paste_sync::HybridClock::new(device).tick(chrono::Utc::now().timestamp_millis()),
            version,
        );
        let envelope = paste_sync::SyncEnvelope::new(
            paste_sync::SyncScope::Shared,
            change,
            Some(paste_sync::SyncPayload::Pinboard(board)),
        )
        .expect("synthetic envelope");
        pull_one_shared_page_with(&store, share.clone(), |_| {
            Ok(CloudDownloadBatch {
                envelopes: vec![envelope.clone()],
                server_change_token: b"applied".to_vec(),
                ..Default::default()
            })
        })
        .expect("download and apply");
        assert_eq!(
            store.list_pinboards().expect("projected")[0].name,
            "Downloaded presentation"
        );
        assert_eq!(store.pending_shared_download_count().expect("consumed"), 0);
        let mut next = envelope;
        next.change.operation_id = uuid::Uuid::new_v4();
        next.change.version.increment(device);
        if let Some(paste_sync::SyncPayload::Pinboard(board)) = &mut next.payload {
            board.name = "Durable backlog".into();
        }
        let share = store.list_pinboard_shares().expect("current")[0].clone();
        store
            .stage_pinboard_share_sync_page(&share, &[next], &[], b"backlog")
            .expect("durable before crash");
        let share = store.list_pinboard_shares().expect("current")[0].clone();
        assert!(
            pull_one_shared_page_with(&store, share, |_| Err(CloudKitError::DownloadTimeout))
                .is_err()
        );
        assert_eq!(
            store.list_pinboards().expect("backlog applied offline")[0].name,
            "Durable backlog"
        );
        assert_eq!(store.pending_shared_download_count().expect("consumed"), 0);
    }

    #[test]
    fn expired_token_resets_once_and_replays_through_the_atomic_inbox_gate() {
        let (store, mut share) = fixture();
        store
            .stage_pinboard_share_sync_page(&share, &[], &[], b"expired")
            .expect("initial token");
        share.server_change_token = Some(b"expired".to_vec());
        let mut calls = 0;
        pull_one_shared_page_with(&store, share.clone(), |snapshot| {
            calls += 1;
            if calls == 1 {
                assert_eq!(
                    snapshot.server_change_token.as_deref(),
                    Some(b"expired".as_slice())
                );
                return Err(CloudKitError::ServerChangeTokenExpired);
            }
            assert!(snapshot.server_change_token.is_none());
            Ok(CloudDownloadBatch {
                server_change_token: b"replayed".to_vec(),
                more_coming: true,
                ..Default::default()
            })
        })
        .expect("simulated replay");
        assert_eq!(calls, 2);
        assert_eq!(
            store.list_pinboard_shares().expect("checkpoint")[0]
                .server_change_token
                .as_deref(),
            Some(b"replayed".as_slice())
        );
        share.server_change_token = Some(b"replayed".to_vec());
        calls = 0;
        assert!(
            pull_one_shared_page_with(&store, share, |_| {
                calls += 1;
                Err(CloudKitError::ServerChangeTokenExpired)
            })
            .is_err()
        );
        assert_eq!(calls, 2, "never loop indefinitely on expiry");
    }

    #[test]
    fn network_failure_or_late_response_never_moves_the_checkpoint_backward() {
        let (store, share) = fixture();
        assert!(
            pull_one_shared_page_with(&store, share.clone(), |_| Err(
                CloudKitError::DownloadTimeout
            ))
            .is_err()
        );
        assert!(
            store.list_pinboard_shares().expect("checkpoint")[0]
                .server_change_token
                .is_none()
        );
        let result = pull_one_shared_page_with(&store, share.clone(), |_| {
            store
                .stage_pinboard_share_sync_page(&share, &[], &[], b"newer")
                .expect("concurrent checkpoint");
            Ok(CloudDownloadBatch {
                server_change_token: b"late".to_vec(),
                ..Default::default()
            })
        });
        assert!(matches!(
            result,
            Err(CloudSyncWorkerError::Storage(
                paste_storage::StorageError::StaleSharedDownload
            ))
        ));
        assert_eq!(
            store.list_pinboard_shares().expect("checkpoint")[0]
                .server_change_token
                .as_deref(),
            Some(b"newer".as_slice())
        );
    }

    #[test]
    fn disabled_sync_never_fetches_and_disabling_in_flight_does_not_accept_a_page() {
        let (store, share) = fixture();
        store.set_cloud_sync_enabled(false).expect("disable");
        pull_one_shared_page_with(&store, share.clone(), |_| panic!("must not fetch"))
            .expect("disabled");
        store.set_cloud_sync_enabled(true).expect("enable");
        pull_one_shared_page_with(&store, share, |_| {
            store
                .set_cloud_sync_enabled(false)
                .expect("disable in flight");
            Ok(CloudDownloadBatch {
                server_change_token: b"not-accepted".to_vec(),
                ..Default::default()
            })
        })
        .expect("stop after completed request");
        assert!(
            store.list_pinboard_shares().expect("checkpoint")[0]
                .server_change_token
                .is_none()
        );
    }

    #[test]
    fn bounded_round_robin_skips_inactive_shares_and_eventually_visits_every_board() {
        let (_, base) = fixture();
        let shares = (1..=20)
            .map(|index| {
                let mut share = base.clone();
                share.id = uuid::Uuid::from_u128(index);
                if index <= 2 {
                    share.state = PinboardShareState::Preparing;
                }
                share
            })
            .collect::<Vec<_>>();
        let mut seen = std::collections::BTreeSet::new();
        let mut cursor = None;
        for _ in 0..3 {
            let batch = shared_download_order(shares.clone(), cursor);
            assert_eq!(batch.len(), MAX_BATCHES_PER_CYCLE);
            assert!(
                batch
                    .iter()
                    .all(|share| share.state == PinboardShareState::Active)
            );
            for share in batch {
                seen.insert(share.id);
                cursor = Some(share.id);
            }
        }
        assert_eq!(seen.len(), 18);
        assert!(shared_download_order(vec![], cursor).is_empty());
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::*;

    #[test]
    fn isolated_cloud_service_never_creates_a_worker() {
        let service = CloudSyncService::isolated();
        #[cfg(feature = "cloudkit")]
        assert!(service.sender.is_none());
        service.wake();
        let status = service.status();
        assert!(!status.transport_configured && !status.syncing);
        assert!(status.last_success_at_ms.is_none());
        assert!(status.blocked_reason.is_some());
    }
}
