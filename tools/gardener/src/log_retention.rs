use crate::errors::GardenerError;
use std::fs;
use std::path::{Path, PathBuf};

pub fn enforce_total_budget(dir: &Path, budget_bytes: u64) -> Result<Vec<PathBuf>, GardenerError> {
    let mut files = fs::read_dir(dir)
        .map_err(|e| GardenerError::Io(e.to_string()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
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
    use super::enforce_total_budget;
    use std::fs;

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
}
