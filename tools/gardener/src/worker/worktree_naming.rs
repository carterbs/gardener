use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn worktree_branch_for(task_id: &str) -> String {
    format!("gardener/{}", worktree_slug_for_task(task_id))
}

pub(crate) fn worktree_path_for(repo_root: &Path, task_id: &str) -> PathBuf {
    let base = env::var("HOME").map_or_else(
        |_| repo_root.to_path_buf(),
        |_home| PathBuf::from("/tmp/gardener-worktrees"),
    );
    base.join(worktree_slug_for_task(task_id))
}

/// Returns a git-safe slug derived from the task ID.
/// Replaces runs of non-alphanumeric characters with a single `-` and
/// truncates to 24 characters so branch names stay readable.
pub(crate) fn sanitize_for_branch(task_id: &str) -> String {
    let slug: String = task_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and strip leading/trailing ones.
    let collapsed = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    collapsed.chars().take(24).collect()
}

pub(crate) fn worktree_slug_for_task(task_id: &str) -> String {
    let base = sanitize_for_branch(task_id);
    let base = if base.is_empty() {
        "task".to_string()
    } else {
        base
    };
    let prefix = base
        .chars()
        .take(WORKTREE_TASK_SLUG_PREFIX_CHARS)
        .collect::<String>();
    let suffix = worktree_slug_suffix(task_id);
    format!("{prefix}-{suffix}")
}

fn worktree_slug_suffix(task_id: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    for &byte in task_id.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{:08x}", hash)
}

const WORKTREE_TASK_SLUG_PREFIX_CHARS: usize = 14;

#[cfg(test)]
mod tests {
    use super::{
        sanitize_for_branch, worktree_branch_for, worktree_path_for, worktree_slug_for_task,
        worktree_slug_suffix, WORKTREE_TASK_SLUG_PREFIX_CHARS,
    };

    #[test]
    fn sanitize_for_branch_strips_invalid_chars() {
        assert_eq!(
            sanitize_for_branch("manual:tui:GARD-03"),
            "manual-tui-GARD-03"
        );
        assert_eq!(sanitize_for_branch("simple"), "simple");
        assert_eq!(sanitize_for_branch("abc-123"), "abc-123");
        assert_eq!(sanitize_for_branch("foo bar"), "foo-bar");
        assert_eq!(sanitize_for_branch("foo..bar"), "foo-bar");
        assert_eq!(sanitize_for_branch("a/b/c"), "a-b-c");
        assert_eq!(sanitize_for_branch("a::b"), "a-b");
        let long = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(sanitize_for_branch(long).len(), 24);
    }

    #[test]
    fn worktree_names_are_git_safe_for_namespaced_task_ids() {
        let branch = worktree_branch_for("manual:tui:GARD-03");
        assert!(
            !branch.contains(':'),
            "branch name must not contain colon: {branch}"
        );
        assert_eq!(
            branch,
            format!("gardener/{}", worktree_slug_for_task("manual:tui:GARD-03"))
        );

        let path = worktree_path_for(std::path::Path::new("/repo"), "manual:tui:GARD-03");
        let dir_name = path
            .file_name()
            .expect("worktree path should have file name")
            .to_str()
            .expect("worktree path should be valid UTF-8");
        assert!(
            !dir_name.contains(':'),
            "path component must not contain colon: {dir_name}"
        );
    }

    #[test]
    fn worktree_slug_for_task_is_stable_and_collision_resistant() {
        let first = worktree_slug_for_task("manual:tui:GARD-01");
        let second = worktree_slug_for_task("manual:tui:GARD-11");
        assert_ne!(first, second);
        let first_suffix = first.rsplit('-').next().unwrap_or_default();
        assert_eq!(first_suffix, worktree_slug_suffix("manual:tui:GARD-01"));
        assert_eq!(first_suffix.len(), 16);
        assert_eq!(
            second.rsplit('-').next().unwrap_or_default(),
            worktree_slug_suffix("manual:tui:GARD-11")
        );
        assert!(first.len() <= WORKTREE_TASK_SLUG_PREFIX_CHARS + 1 + 16);
        let branch = worktree_branch_for("manual:tui:GARD-01");
        assert_eq!(branch.len(), "gardener/".len() + first.len());
    }
}
