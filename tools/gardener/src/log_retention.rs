use crate::errors::GardenerError;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns true if the file should never be deleted by budget enforcement.
///
/// The budget enforcer runs on the entire `~/.gardener/` directory. We must
/// never touch the backlog database or its SQLite sidecar files — those are
/// not logs and their loss is catastrophic and silent.
fn is_protected(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    // Protect backlog database and all SQLite sidecar/backup files.
    name.contains(".sqlite")
}

/// Rotate `log_path` if it exceeds `max_bytes`. Keeps at most `keep` numbered
/// rotations alongside the active file:
///
///   otel-logs.jsonl      ← always the current (writable) log
///   otel-logs.1.jsonl    ← most recent rotation
///   otel-logs.2.jsonl
///   otel-logs.{keep}.jsonl  ← oldest, deleted on next rotation
///
/// Returns `true` if a rotation actually occurred.
pub fn rotate_log_if_needed(
    log_path: &Path,
    max_bytes: u64,
    keep: u32,
) -> Result<bool, GardenerError> {
    let size = match fs::metadata(log_path) {
        Ok(m) => m.len(),
        Err(_) => return Ok(false), // file doesn't exist yet
    };

    if size <= max_bytes {
        return Ok(false);
    }

    // Drop the oldest rotation if we're already at the limit.
    let oldest = rotated_path(log_path, keep);
    if oldest.exists() {
        fs::remove_file(&oldest).map_err(|e| GardenerError::Io(e.to_string()))?;
    }

    // Shift rotations up: log.2 → log.3, log.1 → log.2, …
    for i in (1..keep).rev() {
        let from = rotated_path(log_path, i);
        let to = rotated_path(log_path, i + 1);
        if from.exists() {
            fs::rename(&from, &to).map_err(|e| GardenerError::Io(e.to_string()))?;
        }
    }

    // Rotate the current log into slot 1.
    fs::rename(log_path, rotated_path(log_path, 1))
        .map_err(|e| GardenerError::Io(e.to_string()))?;

    Ok(true)
}

/// Build the path for rotation slot `n`.
///
/// `otel-logs.jsonl` → `otel-logs.1.jsonl` (n=1), `otel-logs.2.jsonl` (n=2), …
fn rotated_path(log_path: &Path, n: u32) -> PathBuf {
    let stem = log_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("otel-logs");
    let ext = log_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jsonl");
    let parent = log_path.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.{n}.{ext}"))
}

/// Delete the oldest files in `dir` (by mtime) until total size is under
/// `budget_bytes`. Never deletes SQLite database files.
///
/// This is a safety-net function; prefer `rotate_log_if_needed` for structured
/// rotation of known log files.
pub fn enforce_total_budget(dir: &Path, budget_bytes: u64) -> Result<Vec<PathBuf>, GardenerError> {
    let mut files = fs::read_dir(dir)
        .map_err(|e| GardenerError::Io(e.to_string()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !is_protected(path))
        .collect::<Vec<_>>();

    files.sort_by(|a, b| {
        let ma = fs::metadata(a).ok().and_then(|m| m.modified().ok());
        let mb = fs::metadata(b).ok().and_then(|m| m.modified().ok());
        ma.cmp(&mb)
    });

    let mut total = files
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
        .sum::<u64>();

    let mut deleted = Vec::new();
    for path in files {
        if total <= budget_bytes {
            break;
        }
        let len = fs::metadata(&path)
            .map_err(|e| GardenerError::Io(e.to_string()))?
            .len();
        fs::remove_file(&path).map_err(|e| GardenerError::Io(e.to_string()))?;
        total = total.saturating_sub(len);
        deleted.push(path);
    }

    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── enforce_total_budget ──────────────────────────────────────────────────

    #[test]
    fn prunes_oldest_files_until_budget_is_met() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.log"), vec![0u8; 40]).expect("a");
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(dir.path().join("b.log"), vec![0u8; 40]).expect("b");

        let deleted = enforce_total_budget(dir.path(), 50).expect("pruned");
        assert_eq!(deleted.len(), 1);
        assert!(deleted[0].ends_with("a.log"));
    }

    #[test]
    fn never_deletes_sqlite_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("backlog.sqlite"), vec![0u8; 60]).expect("db");
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(dir.path().join("otel-logs.jsonl"), vec![0u8; 60]).expect("log");

        let deleted = enforce_total_budget(dir.path(), 50).expect("pruned");
        assert!(
            deleted
                .iter()
                .all(|p| !p.to_string_lossy().contains(".sqlite")),
            "budget enforcer deleted a sqlite file: {deleted:?}"
        );
        assert!(
            dir.path().join("backlog.sqlite").exists(),
            "backlog.sqlite was deleted"
        );
    }

    #[test]
    fn never_deletes_sqlite_sidecar_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in &[
            "backlog.sqlite",
            "backlog.sqlite-wal",
            "backlog.sqlite-shm",
            "backlog.sqlite.bak",
        ] {
            fs::write(dir.path().join(name), vec![0u8; 30]).expect("write");
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(dir.path().join("otel-logs.jsonl"), vec![0u8; 30]).expect("log");

        enforce_total_budget(dir.path(), 50).expect("pruned");
        for name in &[
            "backlog.sqlite",
            "backlog.sqlite-wal",
            "backlog.sqlite-shm",
            "backlog.sqlite.bak",
        ] {
            assert!(
                dir.path().join(name).exists(),
                "{name} was deleted by budget enforcer"
            );
        }
    }

    // ── rotate_log_if_needed ──────────────────────────────────────────────────

    #[test]
    fn no_rotation_when_under_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("otel-logs.jsonl");
        fs::write(&log, vec![0u8; 100]).expect("write");

        let rotated = rotate_log_if_needed(&log, 200, 3).expect("rotate");
        assert!(!rotated);
        assert!(log.exists(), "log should still exist");
    }

    #[test]
    fn rotates_when_over_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("otel-logs.jsonl");
        fs::write(&log, vec![b'x'; 200]).expect("write");

        let rotated = rotate_log_if_needed(&log, 100, 3).expect("rotate");
        assert!(rotated);
        assert!(!log.exists(), "current log should have been renamed away");
        assert!(
            dir.path().join("otel-logs.1.jsonl").exists(),
            "rotation slot 1 should exist"
        );
    }

    #[test]
    fn shifts_existing_rotations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("otel-logs.jsonl");
        fs::write(dir.path().join("otel-logs.1.jsonl"), b"old-1").expect("r1");
        fs::write(dir.path().join("otel-logs.2.jsonl"), b"old-2").expect("r2");
        fs::write(&log, vec![b'x'; 200]).expect("write");

        rotate_log_if_needed(&log, 100, 3).expect("rotate");

        assert_eq!(
            fs::read(dir.path().join("otel-logs.1.jsonl")).expect("read slot 1"),
            vec![b'x'; 200],
            "slot 1 should be the just-rotated log"
        );
        assert_eq!(
            fs::read(dir.path().join("otel-logs.2.jsonl")).expect("read slot 2"),
            b"old-1",
            "slot 2 should be the previous slot 1"
        );
        assert_eq!(
            fs::read(dir.path().join("otel-logs.3.jsonl")).expect("read slot 3"),
            b"old-2",
            "slot 3 should be the previous slot 2"
        );
    }

    #[test]
    fn drops_oldest_rotation_at_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("otel-logs.jsonl");
        // All three rotation slots are already full.
        fs::write(dir.path().join("otel-logs.1.jsonl"), b"r1").expect("r1");
        fs::write(dir.path().join("otel-logs.2.jsonl"), b"r2").expect("r2");
        fs::write(dir.path().join("otel-logs.3.jsonl"), b"r3-oldest").expect("r3");
        fs::write(&log, vec![b'x'; 200]).expect("write");

        rotate_log_if_needed(&log, 100, 3).expect("rotate");

        assert!(
            !dir.path().join("otel-logs.3.jsonl").exists()
                || fs::read(dir.path().join("otel-logs.3.jsonl")).expect("read slot 3")
                    != b"r3-oldest",
            "oldest rotation should have been evicted or overwritten"
        );
    }

    #[test]
    fn rotated_path_follows_naming_convention() {
        let log = Path::new("/home/user/.gardener/otel-logs.jsonl");
        assert_eq!(
            rotated_path(log, 1),
            PathBuf::from("/home/user/.gardener/otel-logs.1.jsonl")
        );
        assert_eq!(
            rotated_path(log, 3),
            PathBuf::from("/home/user/.gardener/otel-logs.3.jsonl")
        );
    }
}
