use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root should be two levels above tools/gardener manifest")
        .to_path_buf()
}

#[test]
fn rejects_binary_file_arguments() {
    let repo_root = repo_root();
    let root_script = repo_root.join("scripts").join("check-binary-blobs.sh");
    let temp = tempfile::tempdir().expect("tempdir");

    let text_file = temp.path().join("notes.txt");
    let binary_file = temp.path().join("blob.bin");

    fs::write(&text_file, b"this is text content\n").expect("write text fixture");
    fs::write(&binary_file, [0x7f_u8, 0x45, 0x4c, 0x46].as_ref()).expect("write binary fixture");

    let result = Command::new("bash")
        .arg(&root_script)
        .arg(&text_file)
        .arg(&binary_file)
        .output()
        .expect("run binary blob checker");

    assert!(
        !result.status.success(),
        "binary fixture should fail the checker"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("error: blocked artifact(s)"));
}

#[test]
fn allows_text_file_arguments() {
    let repo_root = repo_root();
    let root_script = repo_root.join("scripts").join("check-binary-blobs.sh");
    let temp = tempfile::tempdir().expect("tempdir");

    let text_file = temp.path().join("notes.txt");
    fs::write(&text_file, b"this is text content\n").expect("write text fixture");

    let result = Command::new("bash")
        .arg(&root_script)
        .arg(&text_file)
        .output()
        .expect("run binary blob checker");

    assert!(result.status.success(), "text-only input should pass");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("no staged binary blobs"));
}

#[test]
fn rejects_runtime_artifacts_by_name() {
    let repo_root = repo_root();
    let root_script = repo_root.join("scripts").join("check-binary-blobs.sh");
    let temp = tempfile::tempdir().expect("tempdir");

    let default_profraw = temp.path().join("default_9876543210_0_12345.profraw");
    let startup_diagnostics_dir = temp.path().join("startup-diagnostics");
    let startup_diag = startup_diagnostics_dir.join("test-startup-failure.md");

    fs::write(&default_profraw, b"not-a-real-binary payload\n").expect("write profiling artifact");
    fs::create_dir_all(&startup_diagnostics_dir).expect("startup diagnostics fixture dir");
    fs::write(&startup_diag, b"# Startup diagnostics").expect("write startup diagnostics artifact");

    let result = Command::new("bash")
        .arg(&root_script)
        .arg(&default_profraw)
        .arg(&startup_diag)
        .output()
        .expect("run binary blob checker");

    assert!(
        !result.status.success(),
        "runtime artifact fixtures should fail the checker"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("blocked artifact(s)"));
    assert!(stderr.contains("default_9876543210_0_12345.profraw"));
    assert!(stderr.contains("test-startup-failure.md"));
}
