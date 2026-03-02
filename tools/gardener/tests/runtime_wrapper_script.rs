use assert_cmd::Command;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn make_executable(path: &PathBuf) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("set permissions");
}

#[test]
fn brad_gardener_fails_outside_worktree() {
    let temp = TempDir::new().expect("tempdir");
    let mut cmd = Command::new("bash");
    cmd.arg(workspace_root().join("scripts/brad-gardener"));
    cmd.current_dir(&temp);
    cmd.env_remove("GIT_DIR");
    cmd.env_remove("GIT_WORK_TREE");
    cmd.env_remove("GIT_INDEX_FILE");

    let output = cmd.assert().failure();
    let stderr =
        String::from_utf8(output.get_output().stderr.clone()).expect("stderr should be utf8");

    assert!(stderr.contains("must be run from a git worktree"));
}

#[test]
fn brad_gardener_delegates_to_cargo_from_worktree() {
    let temp = TempDir::new().expect("tempdir");
    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin");

    let log_path = temp.path().join("cargo-args.txt");
    let cargo_stub_path = fake_bin.join("cargo");
    fs::write(
        &cargo_stub_path,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            log_path.display()
        ),
    )
    .expect("write cargo stub");
    make_executable(&cargo_stub_path);

    let mut cmd = Command::new("bash");
    cmd.arg(workspace_root().join("scripts/brad-gardener"));
    cmd.arg("--help");
    cmd.current_dir(workspace_root());
    cmd.env_remove("GIT_DIR");
    cmd.env_remove("GIT_WORK_TREE");
    cmd.env_remove("GIT_INDEX_FILE");
    let path = env::var("PATH").unwrap_or_else(|_| String::new());
    cmd.env("PATH", format!("{}:{}", fake_bin.display(), path));

    cmd.assert().success();

    let captured = fs::read_to_string(&log_path).expect("read cargo args log");
    let expected = ["run", "--quiet", "-p", "gardener", "--", "--help"];
    let actual: Vec<_> = captured.lines().collect();

    assert_eq!(actual, expected);
}

#[test]
fn brad_gardener_script_path_resolution_uses_portable_readlink() {
    let script_text = fs::read_to_string(workspace_root().join("scripts").join("brad-gardener"))
        .expect("read brad-gardener script");
    assert!(
        !script_text.contains("readlink -f"),
        "script should not use GNU-only readlink -f"
    );
    assert!(
        script_text.contains("while [[ -h \"$source_path\" ]]"),
        "script should resolve symlink script paths"
    );
    assert!(
        script_text.contains("cargo run --quiet -p gardener -- \"$@\""),
        "script should invoke gardener cargo wrapper with positional args"
    );
}

#[test]
fn brad_gardener_rejects_wrong_worktree_context() {
    let temp = TempDir::new().expect("tempdir");
    StdCommand::new("git")
        .arg("init")
        .current_dir(&temp)
        .output()
        .expect("init git repo");

    let mut cmd = Command::new("bash");
    cmd.arg(workspace_root().join("scripts/brad-gardener"));
    cmd.current_dir(&temp);
    cmd.env_remove("GIT_DIR");
    cmd.env_remove("GIT_WORK_TREE");
    cmd.env_remove("GIT_INDEX_FILE");

    let output = cmd.assert().failure();
    let stderr =
        String::from_utf8(output.get_output().stderr.clone()).expect("stderr should be utf8");

    assert!(
        stderr.contains("must be run from its own git worktree")
            || stderr.contains("must be run from a git worktree")
    );
}
