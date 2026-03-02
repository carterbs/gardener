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
    assert!(stderr.contains("error: blocked binary blob(s)"));
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
