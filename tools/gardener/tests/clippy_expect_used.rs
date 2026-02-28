use std::path::Path;

#[test]
fn workspace_lints_configure_expected_clippy_rules() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let root_manifest =
        std::fs::read_to_string(workspace_root.join("Cargo.toml")).expect("root Cargo.toml");

    assert!(
        root_manifest.contains("[workspace.lints.clippy]"),
        "root Cargo.toml must have [workspace.lints.clippy] section"
    );
    assert!(
        root_manifest.contains("unwrap_used = \"deny\""),
        "root Cargo.toml must deny unwrap_used"
    );
    assert!(
        root_manifest.contains("expect_used = \"warn\""),
        "root Cargo.toml must warn on expect_used"
    );

    let member_manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("member Cargo.toml");

    assert!(
        member_manifest.contains("[lints]") && member_manifest.contains("workspace = true"),
        "member Cargo.toml must inherit workspace lints"
    );
}
