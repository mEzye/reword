use fs2::FileExt;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const LOG_LIMIT: u64 = 1024 * 1024;

pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("REWORD_DATA_DIR") {
        return path.into();
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("Reword");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/Reword");
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("reword");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/reword");
    }
    std::env::temp_dir().join("Reword")
}

pub fn log_path(speech: bool) -> PathBuf {
    data_dir()
        .join("logs")
        .join(if speech { "speech.log" } else { "app.log" })
}

fn append(path: &Path, message: &str) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock_exclusive()?;
    let result = (|| {
        let mut start = message.len().saturating_sub(LOG_LIMIT as usize - 1);
        while !message.is_char_boundary(start) {
            start += 1;
        }
        let bytes = &message.as_bytes()[start..];
        if file.metadata()?.len() + bytes.len() as u64 + 1 > LOG_LIMIT {
            file.set_len(0)?;
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(bytes)?;
        if !bytes.ends_with(b"\n") {
            file.write_all(b"\n")?;
        }
        Ok(())
    })();
    let _ = FileExt::unlock(&file);
    result
}

pub fn log(speech: bool, message: &str) {
    let _ = append(&log_path(speech), message);
}

pub fn drain<R: Read + Send + 'static>(mut stream: R, speech: bool) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut bytes = [0u8; 4096];
        let mut pending = Vec::with_capacity(4100);
        while let Ok(n) = stream.read(&mut bytes) {
            if n == 0 {
                break;
            }
            pending.extend_from_slice(&bytes[..n]);
            loop {
                match std::str::from_utf8(&pending) {
                    Ok(text) => {
                        log(speech, text);
                        pending.clear();
                        break;
                    }
                    Err(error) => {
                        let valid = error.valid_up_to();
                        if valid > 0 {
                            log(speech, std::str::from_utf8(&pending[..valid]).unwrap());
                            pending.drain(..valid);
                        }
                        if let Some(invalid) = error.error_len() {
                            log(speech, "\u{fffd}");
                            pending.drain(..invalid);
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        if !pending.is_empty() {
            log(speech, &String::from_utf8_lossy(&pending));
        }
    })
}

pub fn tail(speech: bool) -> String {
    let path = log_path(speech);
    let Ok(mut file) = OpenOptions::new().read(true).open(path) else {
        return String::new();
    };
    if file.lock_shared().is_err() {
        return String::new();
    }
    let mut data = Vec::new();
    let _ = file.read_to_end(&mut data);
    let _ = FileExt::unlock(&file);
    String::from_utf8_lossy(&data[data.len().saturating_sub(700)..]).into_owned()
}

pub fn scope(config: &Path) -> io::Result<PathBuf> {
    let mut hash = DefaultHasher::new();
    fs::canonicalize(config)
        .unwrap_or_else(|_| config.to_path_buf())
        .hash(&mut hash);
    let path = data_dir()
        .join("runtime")
        .join(format!("{:016x}", hash.finish()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub struct Session {
    pub path: PathBuf,
    lease: Option<File>,
}

impl Session {
    pub fn start(config: &Path) -> io::Result<Self> {
        if let Some(path) = std::env::var_os("REWORD_RUNTIME_SESSION") {
            let path = PathBuf::from(path);
            let expected = scope(config)?;
            let gate = gate(&expected)?;
            if !valid_session(&path, &expected) {
                return Err(io::Error::other("Invalid runtime session"));
            }
            let result = Self::open(path);
            drop(gate);
            return result;
        }
        let scope = scope(config)?;
        let gate = gate(&scope)?;
        cleanup(&scope);
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
        let suffix: String = nonce.iter().map(|v| format!("{v:02x}")).collect();
        let path = scope.join(format!("run-{suffix}"));
        fs::create_dir(&path)?;
        let session = Self::open(path)?;
        drop(gate);
        // SAFETY: this is called before the process starts any worker threads.
        unsafe {
            std::env::set_var("REWORD_RUNTIME_SESSION", &session.path);
        }
        Ok(session)
    }
    fn open(path: PathBuf) -> io::Result<Self> {
        let lease_path = path.join("lease");
        if let Ok(meta) = fs::symlink_metadata(&lease_path) {
            if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                return Err(io::Error::other("Invalid runtime lease"));
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lease_path)?;
        file.lock_shared()?;
        Ok(Self {
            path,
            lease: Some(file),
        })
    }
}

fn valid_session(path: &Path, scope: &Path) -> bool {
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    let valid_name = name
        .strip_prefix("run-")
        .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()));
    let expected_parent =
        path.parent().and_then(|v| fs::canonicalize(v).ok()) == fs::canonicalize(scope).ok();
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !valid_name
        || !expected_parent
        || !meta.file_type().is_dir()
        || meta.file_type().is_symlink()
    {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(scope) = self.path.parent() else {
            return;
        };
        let Ok(_gate) = gate(scope) else { return };
        if !valid_session(&self.path, scope) {
            return;
        }
        drop(self.lease.take());
        remove_inactive_session(&self.path);
    }
}

fn gate(scope: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(scope.join("cleanup.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

fn cleanup(scope: &Path) {
    cleanup_older_than(scope, Duration::from_secs(30));
}

fn cleanup_older_than(scope: &Path, minimum_age: Duration) {
    let Ok(entries) = fs::read_dir(scope) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(suffix) = name.strip_prefix("run-") else {
            continue;
        };
        if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        if !valid_session(&path, scope) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta
            .modified()
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .unwrap_or(Duration::ZERO)
            < minimum_age
        {
            continue;
        }
        remove_inactive_session(&path);
    }
}

// The caller holds the scope gate. Other owners keep the lease shared.
fn remove_inactive_session(path: &Path) {
    let lease_path = path.join("lease");
    if fs::symlink_metadata(&lease_path)
        .map(|m| !m.file_type().is_file() || m.file_type().is_symlink())
        .unwrap_or(true)
    {
        return;
    }
    let Ok(lease) = OpenOptions::new().read(true).write(true).open(&lease_path) else {
        return;
    };
    if lease.try_lock_exclusive().is_err() {
        return;
    }
    // Only names created by Reword are removed; live processes hold shared leases.
    for item in fs::read_dir(&path).into_iter().flatten().flatten() {
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".renderer-ready-")
            || name.starts_with(".speech-ready-")
            || (name.starts_with(".speech-") && name.ends_with(".token"))
        {
            let _ = fs::remove_file(item.path());
        }
    }
    let only_lease = fs::read_dir(&path)
        .map(|items| items.flatten().all(|item| item.file_name() == "lease"))
        .unwrap_or(false);
    drop(lease);
    if only_lease {
        let _ = fs::remove_file(path.join("lease"));
        let _ = fs::remove_dir(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary() -> PathBuf {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!(
            "reword-runtime-test-{}",
            nonce.iter().map(|v| format!("{v:02x}")).collect::<String>()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn bounded_logs_keep_channels_separate() {
        let dir = temporary();
        let app = dir.join("app.log");
        let speech = dir.join("speech.log");
        append(&app, "app failure").unwrap();
        append(&speech, "model failure").unwrap();
        append(&app, &"я".repeat(LOG_LIMIT as usize)).unwrap();
        assert!(fs::metadata(&app).unwrap().len() <= LOG_LIMIT);
        assert!(std::str::from_utf8(&fs::read(&app).unwrap()).is_ok());
        assert_eq!(fs::read_to_string(&speech).unwrap(), "model failure\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn last_owner_removes_session_but_live_owner_protects_it() {
        let scope = temporary();
        let path = scope.join(format!("run-{}", "c".repeat(32)));
        fs::create_dir(&path).unwrap();
        let first = Session::open(path.clone()).unwrap();
        let second = Session::open(path.clone()).unwrap();
        fs::write(path.join(".speech-ready-test.json"), "ready").unwrap();
        drop(first);
        assert!(path.join(".speech-ready-test.json").exists());
        drop(second);
        assert!(!path.exists());
        fs::remove_dir_all(scope).unwrap();
    }

    #[test]
    fn cleanup_keeps_live_leases_and_unrelated_files() {
        let scope = temporary();
        let live = scope.join(format!("run-{}", "a".repeat(32)));
        let stale = scope.join(format!("run-{}", "b".repeat(32)));
        for path in [&live, &stale] {
            fs::create_dir(path).unwrap();
            File::create(path.join("lease")).unwrap();
            fs::write(path.join(".speech-ready-test.json"), "ready").unwrap();
            fs::write(path.join("unrelated"), "keep").unwrap();
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(live.join("lease"))
            .unwrap();
        lease.lock_shared().unwrap();
        cleanup_older_than(&scope, Duration::ZERO);
        assert!(live.join(".speech-ready-test.json").exists());
        assert!(!stale.join(".speech-ready-test.json").exists());
        assert!(stale.join("unrelated").exists());
        drop(lease);
        fs::remove_dir_all(scope).unwrap();
    }
}
