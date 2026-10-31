//! Two writers on one design file (#5467): a design kept in a shared or synced folder can be open
//! on two computers at once, and the second Save would silently replace the first one's work.
//!
//! - **Lock files.** While the desktop app has a design open it keeps a small JSON file next to
//!   it, `.~<file>.lock` ([`lock_path`]): the computer, the user, the app version, the process
//!   and when it was opened and last refreshed. The app refreshes it every [`REFRESH_SECS`] and
//!   removes it on close, so a lock left by a crash or a lost connection goes stale
//!   ([`Lock::is_stale`]) and is ignored instead of blocking anyone. One written in this very
//!   boot of this computer, and among the same processes (the lock keeps which: [`Lock::boot`]),
//!   by a process that no longer runs (a crash) is ignored at once ([`Lock::is_dead`]), and the
//!   design's next opener takes it over; another computer's (even one of the same name), another
//!   sandbox's, or one where that can't be told waits to go stale. It is not a design file, so
//!   the file browser, Open and the recent lists never list it, and failing to write one (a
//!   read-only folder) never stops a design from opening.
//! - **File stamps.** A design remembers its file's size and modification time when it is opened
//!   or saved ([`FileStamp`]); a different stamp at the next Save means another program or
//!   computer wrote the file meanwhile.
//!
//! The browser build keeps neither: it has no shared folders, and no clock in `std`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How often an open design's lock is written again.
pub const REFRESH_SECS: u64 = 60;
/// A lock not refreshed for this long belongs to an app that is gone (three missed refreshes).
pub const STALE_SECS: u64 = 3 * REFRESH_SECS;
/// Largest lock file read (a real one is a few hundred bytes).
const MAX_LOCK_BYTES: u64 = 4096;

/// A file's size and modification time, to tell whether something else wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    /// Nanoseconds since the Unix epoch (`None` where the file system keeps no time).
    pub modified_ns: Option<u128>,
}

/// The stamp of the file at `path` now (`None`: it doesn't exist or can't be read; always `None`
/// in the browser). A symbolic link is followed, as a save follows it.
pub fn stamp(path: &str) -> Option<FileStamp> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let meta = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    let modified_ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos());
    Some(FileStamp { len: meta.len(), modified_ns })
}

/// The file at `path` is not the one stamped `known`: something else wrote it since. A file that
/// is gone counts as unchanged (saving puts it back and replaces nobody's work).
pub fn changed_since(path: &str, known: &FileStamp) -> bool {
    stamp(path).is_some_and(|now| now != *known)
}

/// The lock file of the design at `path`: `.~<file name>.lock` in the same folder.
pub fn lock_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "design".into());
    path.with_file_name(format!(".~{name}.lock"))
}

/// Seconds since the Unix epoch (0 in the browser, which keeps no lock files).
pub fn now_secs() -> u64 {
    if cfg!(target_arch = "wasm32") {
        return 0;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Who has a design open, as its lock file says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// The computer's name.
    pub host: String,
    pub user: String,
    /// The SolidCraft version that wrote it.
    pub app: String,
    pub pid: u32,
    /// When the design was opened, and when the lock was last written (Unix seconds).
    pub opened: u64,
    pub refreshed: u64,
    /// Which boot of which computer, among which processes, `pid` is of ([`boot_id`]); empty
    /// where that can't be told (and in a lock written before locks kept it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub boot: String,
}

impl Lock {
    /// This app's lock on a design opened at `opened`.
    pub fn mine(opened: u64) -> Lock {
        Lock {
            host: host_name(),
            user: user_name(),
            app: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            opened,
            refreshed: opened,
            boot: boot_id().into(),
        }
    }

    /// Not refreshed for [`STALE_SECS`] at `now`: the app that wrote it is gone. A lock dated
    /// further ahead than that (another computer's clock far off) is stale too, so a wrong clock
    /// can't keep a design locked.
    pub fn is_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.refreshed) > STALE_SECS || self.refreshed.saturating_sub(now) > STALE_SECS
    }

    /// Written by this process: its pid among the same processes of the same boot.
    pub fn is_mine(&self) -> bool {
        self.pid == std::process::id() && self.host == host_name() && self.boot == boot_id()
    }

    /// Written in this boot of this computer, among the processes this one sees, by another
    /// process that no longer runs: the app crashed, and the design is free. `false` where that
    /// can't be told (another computer, of whatever name; another sandbox, whose pids are its
    /// own; a lock or a system that doesn't say which boot; a system the app can't ask).
    pub fn is_dead(&self) -> bool {
        !self.boot.is_empty() && self.boot == boot_id() && self.host == host_name() && !self.is_mine() && process_gone(self.pid)
    }
}

/// Which boot of which computer this process runs in, and among which processes (a sandbox or
/// a container numbers its own), so a pid in a lock names this process or another one only when
/// they match; empty where that can't be told. Linux: the kernel's boot id (random for each
/// boot) and the pid namespace; macOS: the boot session's id (random for each boot, and one pid
/// namespace on a Mac).
fn boot_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s.len() <= 64);
            let space =
                std::fs::read_link("/proc/self/ns/pid").ok().map(|p| p.to_string_lossy().to_string()).filter(|s| !s.is_empty() && s.len() <= 64);
            boot.zip(space).map(|(b, n)| format!("{b} {n}")).unwrap_or_default()
        }
        #[cfg(target_vendor = "apple")]
        {
            // Asked once, of the system's own tool (there is no other safe way to read it).
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.bootsessionuuid"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty() && s.len() <= 64 && !s.contains(char::is_whitespace))
                .unwrap_or_default()
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            String::new()
        }
    })
}

/// No process `pid` runs among the ones this process sees (`false` where that can't be told).
/// A process of another user still runs (asking it refuses, which is not "gone"), even where
/// the system hides other users' processes.
fn process_gone(pid: u32) -> bool {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) else { return false };
        matches!(rustix::process::test_kill_process(pid), Err(rustix::io::Errno::SRCH))
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

/// [`host_name`] for a computer that has no name.
const NO_HOST: &str = "this computer";

/// The computer's name ("this computer" when it has none).
pub fn host_name() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    let name = gethostname::gethostname().to_string_lossy().trim().to_string();
    #[cfg(target_arch = "wasm32")]
    let name = String::new();
    if name.is_empty() { NO_HOST.into() } else { name }
}

fn user_name() -> String {
    ["USER", "USERNAME", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())).unwrap_or_default()
}

/// The lock next to the design at `path`, if there is a readable one.
pub fn read(path: &Path) -> Option<Lock> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let file = lock_path(path);
    if std::fs::metadata(&file).ok()?.len() > MAX_LOCK_BYTES {
        return None;
    }
    serde_json::from_slice(&std::fs::read(&file).ok()?).ok()
}

/// Another app's fresh lock on the design at `path` at `now`: it is open elsewhere. One left by
/// a process of this computer that is gone is not ([`Lock::is_dead`]).
pub fn held_elsewhere(path: &Path, now: u64) -> Option<Lock> {
    read(path).filter(|l| !l.is_mine() && !l.is_stale(now) && !l.is_dead())
}

/// Write `lock` next to the design at `path` (replacing whatever lock is there).
pub fn write(path: &Path, lock: &Lock) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(lock).map_err(std::io::Error::other)?;
    crate::safe_write::write_atomic(&lock_path(path), &bytes, false)
}

/// Remove this process's lock on the design at `path`; another app's lock is left alone.
pub fn remove(path: &Path) {
    if read(path).is_some_and(|l| l.is_mine()) {
        let _ = std::fs::remove_file(lock_path(path));
    }
}
