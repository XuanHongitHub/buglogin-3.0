use std::{path::PathBuf, process::Command};

fn run_contract_test(pattern: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let test = root.join("tests/contracts/contract-drift.test.mjs");
    let output = Command::new("node")
        .arg("--test")
        .arg(format!("--test-name-pattern={pattern}"))
        .arg(test)
        .current_dir(root)
        .output()
        .expect("Node.js is required for contract compatibility tests");

    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rest_openapi_compatibility() {
    run_contract_test("REST");
}

#[test]
fn tauri_command_compatibility() {
    run_contract_test("Tauri");
}

#[test]
fn mcp_tool_compatibility() {
    run_contract_test("MCP");
}

#[test]
fn generated_client_compatibility() {
    run_contract_test("Generated");
}
