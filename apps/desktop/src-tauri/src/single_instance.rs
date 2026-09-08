//! Normal desktop startup only. No database, clipboard, argument forwarding or
//! network endpoint. The file lease, not socket existence, decides ownership.
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
            net::UnixDatagram,
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

const SHOW: &[u8] = b"PSR1";
const RETRY: Duration = Duration::from_millis(25);
const WAIT: Duration = Duration::from_secs(2);
// Keep the lease until process termination, including after the event loop
// returns: background services must not outlive the process's exclusive lease.
static INSTANCE: Mutex<Option<Primary>> = Mutex::new(None);

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("pasters-single-instance")
        .setup(|app, _| {
            if INSTANCE
                .lock()
                .map_err(|_| io::Error::other("startup lease lock poisoned"))?
                .is_some()
            {
                return Err(io::Error::other("normal instance plugin already initialized").into());
            }
            let entry = enter(&app.path().app_data_dir()?, Path::new("/tmp"), WAIT)?;
            match entry {
                Entry::Primary(primary) => {
                    *INSTANCE
                        .lock()
                        .map_err(|_| io::Error::other("startup lease lock poisoned"))? =
                        Some(primary);
                }
                // No windows, hotkeys, database or capture services were created
                // by this process. A queued show request is not a visible-frame ACK.
                Entry::Forwarded => std::process::exit(0),
            }
            Ok(())
        })
        .on_event(|_, event| {
            if matches!(event, tauri::RunEvent::Exit)
                && let Ok(mut primary) = INSTANCE.lock()
                && let Some(primary) = primary.as_mut()
            {
                primary.stop_listener();
            }
        })
        .build()
}

/// Called at the end of application setup, when DesktopState and windows exist.
/// Earlier duplicate launches remain queued in the socket, not lost to a callback
/// that runs before the window is available.
pub fn start_listener(app: tauri::AppHandle) -> io::Result<()> {
    let queued = Arc::new(AtomicBool::new(false));
    INSTANCE
        .lock()
        .map_err(|_| io::Error::other("startup lease lock poisoned"))?
        .as_mut()
        .ok_or_else(|| io::Error::other("missing normal startup lease"))?
        .start_listener(move || {
            if queued.swap(true, Ordering::AcqRel) {
                return;
            }
            let pending = Arc::clone(&queued);
            let handle = app.clone();
            if app
                .run_on_main_thread(move || {
                    pending.store(false, Ordering::Release);
                    if let Some(window) = handle.get_webview_window("main")
                        && let Err(error) = crate::window::show_main_window(&window)
                    {
                        eprintln!("CopyRail could not show the existing window: {error}");
                    }
                })
                .is_err()
            {
                queued.store(false, Ordering::Release);
            }
        })
}

enum Entry {
    Primary(Primary),
    Forwarded,
}

struct Primary {
    // Never unlink this file: a new inode would let two processes hold a lease.
    _lease: File,
    socket: UnixDatagram,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
    stopped: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

fn uid() -> u32 {
    // SAFETY: geteuid takes no pointers and has no preconditions.
    unsafe { libc::geteuid() }
}

fn private_directory(path: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe instance directory",
        ));
    }
    Ok(())
}

fn lease_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe instance lease file",
        ));
    }
    Ok(file)
}

fn owns_lease(file: &File) -> io::Result<bool> {
    // SAFETY: file owns a live descriptor, and flock does not retain pointers.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

fn socket_path(data: &Path, socket_parent: &Path) -> io::Result<PathBuf> {
    // Short path for macOS sockaddr_un, stable across aliases/versioned bundles,
    // and separate for each user and canonical application data directory.
    let hash = blake3::hash(data.as_os_str().as_encoded_bytes()).to_hex();
    let root = socket_parent.join(format!("pasters-{}-{}", uid(), &hash[..32]));
    private_directory(&root)?;
    Ok(root.join("show.sock"))
}

fn enter(data: &Path, socket_parent: &Path, timeout: Duration) -> io::Result<Entry> {
    fs::create_dir_all(data)?;
    let data = data.canonicalize()?;
    let lease_path = data.join("desktop-instance.lock");
    let file = lease_file(&lease_path)?;
    let socket_path = socket_path(&data, socket_parent)?;
    let notifier = UnixDatagram::unbound()?;
    notifier.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        if owns_lease(&file)? {
            let opened = file.metadata()?;
            let current = fs::symlink_metadata(&lease_path)?;
            if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
                return Err(io::Error::other("instance lease changed during startup"));
            }
            // Only the lease owner may remove a stale socket. Never delete an
            // unexpected regular file, directory or symlink at this location.
            match fs::symlink_metadata(&socket_path) {
                Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == uid() => {
                    fs::remove_file(&socket_path)?
                }
                Ok(_) => return Err(io::Error::other("unexpected instance socket path")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let socket = UnixDatagram::bind(&socket_path)?;
            socket.set_read_timeout(Some(Duration::from_millis(125)))?;
            let metadata = fs::symlink_metadata(&socket_path)?;
            return Ok(Entry::Primary(Primary {
                _lease: file,
                socket,
                socket_path,
                socket_identity: (metadata.dev(), metadata.ino()),
                stopped: Arc::new(AtomicBool::new(false)),
                listener: None,
            }));
        }
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == uid() => {}
            Ok(_) => return Err(io::Error::other("unexpected instance socket path")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match notifier.send_to(SHOW, &socket_path) {
            Ok(count) if count == SHOW.len() => return Ok(Entry::Forwarded),
            Ok(_) => return Err(io::Error::other("incomplete instance notification")),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "CopyRail 已有启动进程，但暂时无法接收显示请求；没有启动第二个采集进程，请稍后重试。",
            ));
        }
        thread::sleep(RETRY.min(deadline.saturating_duration_since(Instant::now())));
    }
}

impl Primary {
    fn start_listener(&mut self, mut show: impl FnMut() + Send + 'static) -> io::Result<()> {
        if self.listener.is_some() {
            return Err(io::Error::other("instance listener already started"));
        }
        let socket = self.socket.try_clone()?;
        let stopped = Arc::clone(&self.stopped);
        self.listener = Some(
            thread::Builder::new()
                .name("pasters-instance".into())
                .spawn(move || {
                    let mut message = [0; 5];
                    while !stopped.load(Ordering::Acquire) {
                        match socket.recv(&mut message) {
                            Ok(count)
                                if &message[..count] == SHOW
                                    && !stopped.load(Ordering::Acquire) =>
                            {
                                show()
                            }
                            Ok(_) => {}
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    io::ErrorKind::WouldBlock
                                        | io::ErrorKind::TimedOut
                                        | io::ErrorKind::Interrupted
                                ) => {}
                            Err(error) => {
                                eprintln!("CopyRail instance listener stopped: {error}");
                                break;
                            }
                        }
                    }
                })?,
        );
        Ok(())
    }

    fn stop_listener(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        self.stop_listener();
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path)
            && metadata.file_type().is_socket()
            && (metadata.dev(), metadata.ino()) == self.socket_identity
        {
            let _ = fs::remove_file(&self.socket_path);
        }
        // Closing _lease releases ownership only after socket cleanup.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::fs::{PermissionsExt, symlink},
        process::{Child, Command, Stdio},
        sync::{Barrier, mpsc},
    };
    use tempfile::TempDir;

    struct Fixture {
        root: TempDir,
        data: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir_in("/tmp").expect("private short socket root");
            let data = root.path().join("data");
            Self { root, data }
        }
        fn enter(&self) -> io::Result<Entry> {
            enter(&self.data, self.root.path(), WAIT)
        }
        fn primary(&self) -> Primary {
            match self.enter().expect("start") {
                Entry::Primary(value) => value,
                Entry::Forwarded => panic!("expected owner"),
            }
        }
    }

    #[test]
    fn secondary_queues_before_listener_ready_without_replacing_the_lease() {
        let fixture = Fixture::new();
        let mut primary = fixture.primary();
        let inode = primary._lease.metadata().expect("lease metadata").ino();
        assert!(matches!(
            fixture.enter().expect("forward"),
            Entry::Forwarded
        ));
        let (tx, rx) = mpsc::channel();
        primary
            .start_listener(move || {
                tx.send(()).expect("test receiver");
            })
            .expect("listener");
        rx.recv_timeout(Duration::from_secs(2))
            .expect("queued show delivered");
        assert!(rx.try_recv().is_err());
        assert_eq!(
            fs::metadata(fixture.data.join("desktop-instance.lock"))
                .expect("lease exists")
                .ino(),
            inode
        );
        assert!(primary.start_listener(|| {}).is_err());
        drop(primary);
        assert_eq!(
            fs::metadata(fixture.data.join("desktop-instance.lock"))
                .expect("lease remains")
                .ino(),
            inode
        );
        assert!(matches!(
            fixture.enter().expect("reopen"),
            Entry::Primary(_)
        ));
    }

    #[test]
    fn simultaneous_starts_elect_exactly_one_owner() {
        let fixture = Fixture::new();
        let barrier = Arc::new(Barrier::new(8));
        let (tx, rx) = mpsc::channel();
        let workers = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let tx = tx.clone();
                let data = fixture.data.clone();
                let root = fixture.root.path().to_owned();
                thread::spawn(move || {
                    barrier.wait();
                    tx.send(enter(&data, &root, WAIT)).expect("send result");
                })
            })
            .collect::<Vec<_>>();
        drop(tx);
        let entries = rx
            .into_iter()
            .map(|result| result.expect("concurrent start"))
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("worker");
        }
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Primary(_)))
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Forwarded))
                .count(),
            7
        );
    }

    #[test]
    fn canonical_data_identity_ignores_aliases_but_separates_profiles() {
        let fixture = Fixture::new();
        let primary = fixture.primary();
        let alias = fixture.root.path().join("alias");
        symlink(&fixture.data, &alias).expect("data alias");
        assert!(matches!(
            enter(&alias, fixture.root.path(), WAIT).expect("alias notification"),
            Entry::Forwarded
        ));
        assert!(matches!(
            enter(
                &fixture.root.path().join("other"),
                fixture.root.path(),
                WAIT
            )
            .expect("other profile"),
            Entry::Primary(_)
        ));
        let sender = UnixDatagram::unbound().expect("sender");
        let (tx, rx) = mpsc::channel();
        let mut primary = primary;
        primary
            .start_listener(move || {
                tx.send(()).expect("receiver");
            })
            .expect("listener");
        rx.recv_timeout(WAIT).expect("alias queued message");
        sender
            .send_to(b"PSR1-extra", &primary.socket_path)
            .expect("invalid packet");
        sender
            .send_to(b"PSR0", &primary.socket_path)
            .expect("old packet");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        sender
            .send_to(SHOW, &primary.socket_path)
            .expect("valid packet");
        rx.recv_timeout(WAIT)
            .expect("only exact bounded protocol accepted");
    }

    #[test]
    fn unavailable_owner_fails_closed_with_a_bounded_wait() {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.data).expect("data");
        let lease = lease_file(&fixture.data.join("desktop-instance.lock")).expect("lease");
        assert!(owns_lease(&lease).expect("claim"));
        let before = Instant::now();
        let result = enter(
            &fixture.data,
            fixture.root.path(),
            Duration::from_millis(90),
        );
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::TimedOut));
        assert!(before.elapsed() < Duration::from_secs(2));
        let challenger =
            lease_file(&fixture.data.join("desktop-instance.lock")).expect("challenger");
        assert!(!owns_lease(&challenger).expect("still owned"));
        drop(lease);
        assert!(owns_lease(&challenger).expect("released"));
    }

    #[test]
    fn waiting_start_can_recover_when_the_previous_owner_exits_before_binding() {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.data).expect("data");
        let lease = lease_file(&fixture.data.join("desktop-instance.lock")).expect("lease");
        assert!(owns_lease(&lease).expect("claim"));
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(75));
            drop(lease);
        });
        assert!(matches!(
            fixture.enter().expect("recover startup"),
            Entry::Primary(_)
        ));
        worker.join().expect("previous owner");
    }

    #[test]
    fn unsafe_paths_are_rejected_without_deleting_or_overwriting_them() {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.data).expect("data");
        let target = fixture.root.path().join("do-not-touch");
        fs::write(&target, b"synthetic sentinel").expect("sentinel");
        let path = fixture.data.join("desktop-instance.lock");
        symlink(&target, &path).expect("symlink");
        assert!(fixture.enter().is_err());
        assert_eq!(fs::read(&target).expect("preserved"), b"synthetic sentinel");
        fs::remove_file(&path).expect("remove test symlink");
        fs::hard_link(&target, &path).expect("hard link");
        assert!(fixture.enter().is_err());
        fs::remove_file(&path).expect("remove test hard link");
        let socket = socket_path(
            &fixture.data.canonicalize().expect("data path"),
            fixture.root.path(),
        )
        .expect("socket path");
        fs::write(&socket, b"not a socket").expect("unexpected node");
        assert!(fixture.enter().is_err());
        assert_eq!(fs::read(&socket).expect("not deleted"), b"not a socket");
        fs::remove_file(&socket).expect("remove fixture file");
        fs::set_permissions(
            socket.parent().expect("socket root"),
            fs::Permissions::from_mode(0o777),
        )
        .expect("insecure directory");
        assert!(fixture.enter().is_err());
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn wait_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(Instant::now() < deadline, "probe marker was not produced");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn actual_second_process_forwards_and_crash_releases_the_lease() {
        let fixture = Fixture::new();
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "single_instance::tests::child_process_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env("PASTERS_INSTANCE_PROBE_ROOT", fixture.root.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("private probe child"),
        );
        wait_file(&fixture.root.path().join("ready"));
        assert!(child.0.try_wait().expect("child status").is_none());
        let lease_inode = fs::metadata(fixture.data.join("desktop-instance.lock"))
            .expect("lease")
            .ino();
        assert!(matches!(
            fixture.enter().expect("second process"),
            Entry::Forwarded
        ));
        wait_file(&fixture.root.path().join("notified"));
        child.0.kill().expect("terminate only owned probe");
        let status = child.0.wait().expect("reap crashed probe");
        assert!(!status.success());
        let primary = fixture.primary();
        assert_eq!(
            primary._lease.metadata().expect("same lease").ino(),
            lease_inode
        );
        assert!(matches!(
            fixture.enter().expect("post-crash notification"),
            Entry::Forwarded
        ));
        let mut bytes = [0; 5];
        let count = primary
            .socket
            .recv(&mut bytes)
            .expect("post-crash delivery");
        assert_eq!(&bytes[..count], SHOW);
    }

    #[test]
    #[ignore = "helper entrypoint, explicitly invoked by the cross-process test"]
    fn child_process_probe() {
        let root = PathBuf::from(
            std::env::var_os("PASTERS_INSTANCE_PROBE_ROOT").expect("private probe root"),
        );
        let Entry::Primary(mut primary) =
            enter(&root.join("data"), &root, WAIT).expect("child lease")
        else {
            panic!("child must own lease");
        };
        let notification = root.join("notified");
        primary
            .start_listener(move || {
                fs::write(&notification, b"show").expect("private marker");
            })
            .expect("child listener");
        fs::write(root.join("ready"), b"primary").expect("ready marker");
        std::io::stdout().flush().expect("flush");
        let mut byte = [0; 1];
        let _ = std::io::stdin().read(&mut byte);
    }
}
