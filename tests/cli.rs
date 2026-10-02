#[test]
fn core_check_verifies_pending_visibility_and_exit_persistence() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_quick-panel"))
        .arg("--core-check")
        .output()
        .expect("run core check");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("structured verification report");
    assert_eq!(report["pending_visible"], true);
    assert_eq!(report["disk_empty_before_flush"], true);
    assert_eq!(report["exit_restored"], true);
}
