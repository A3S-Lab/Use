#![cfg(feature = "office")]

use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

fn run(provider: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(binary())
        .args(args)
        .env("A3S_OFFICECLI_EXECUTABLE", provider)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_spreadsheet_cli_edits_ranges_structure_and_sheet_order_without_officecli() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("structure.xlsx");
    let provider = temp.path().join("must-not-be-invoked");
    let document = document.to_str().unwrap();

    run(
        &provider,
        &["office", "native", "create", document, "--json"],
    );
    run(
        &provider,
        &[
            "office", "native", "add", document, "/", "--type", "sheet", "--name", "Data", "--json",
        ],
    );
    let range = run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document,
            "/Sheet1/B2:A1",
            "--number",
            "1",
            "--json",
        ],
    );
    assert_eq!(range["data"]["node"]["format"]["normalizedRef"], "A1:B2");
    assert_eq!(range["data"]["node"]["childCount"], 4);

    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document,
            "/Data/A1",
            "--formula",
            "Sheet1!B2",
            "--json",
        ],
    );
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document,
            "/Sheet1/C1",
            "--formula",
            "Data!A1",
            "--json",
        ],
    );

    let inserted = run(
        &provider,
        &[
            "office",
            "native",
            "insert-rows",
            document,
            "/Sheet1",
            "2",
            "--count",
            "2",
            "--json",
        ],
    );
    assert_eq!(inserted["data"]["operation"], "insert-rows");
    assert_eq!(inserted["data"]["path"], "/Sheet1/row[2:3]");

    let shifted = run(
        &provider,
        &["office", "native", "get", document, "/Data/A1", "--json"],
    );
    assert_eq!(shifted["data"]["node"]["format"]["formula"], "Sheet1!B4");

    let deleted = run(
        &provider,
        &[
            "office",
            "native",
            "delete-columns",
            document,
            "/Sheet1",
            "A",
            "--json",
        ],
    );
    assert_eq!(deleted["data"]["operation"], "delete-columns");
    assert_eq!(deleted["data"]["path"], "/Sheet1/col[1:1]");

    let renamed = run(
        &provider,
        &[
            "office",
            "native",
            "rename-sheet",
            document,
            "/Data",
            "Q1 Data",
            "--json",
        ],
    );
    assert_eq!(renamed["data"]["path"], "/Q1 Data");
    let renamed_formula = run(
        &provider,
        &["office", "native", "get", document, "/Sheet1/B1", "--json"],
    );
    assert_eq!(
        renamed_formula["data"]["node"]["format"]["formula"],
        "'Q1 Data'!A1"
    );

    run(
        &provider,
        &[
            "office",
            "native",
            "move-sheet",
            document,
            "/Q1 Data",
            "1",
            "--json",
        ],
    );
    let root = run(
        &provider,
        &[
            "office", "native", "get", document, "/", "--depth", "1", "--json",
        ],
    );
    assert_eq!(root["data"]["node"]["children"][0]["path"], "/Q1 Data");
    assert_eq!(root["data"]["node"]["children"][1]["path"], "/Sheet1");

    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document,
            "/Sheet1/D1",
            "--formula",
            "'Sheet1'!B1",
            "--json",
        ],
    );
    let copied = run(
        &provider,
        &[
            "office",
            "native",
            "copy-sheet",
            document,
            "/Sheet1",
            "Copy",
            "--position",
            "2",
            "--json",
        ],
    );
    assert_eq!(copied["data"]["operation"], "copy-sheet");
    assert_eq!(copied["data"]["path"], "/Copy");
    let copied_formula = run(
        &provider,
        &["office", "native", "get", document, "/Copy/D1", "--json"],
    );
    assert_eq!(
        copied_formula["data"]["node"]["format"]["formula"],
        "'Copy'!B1"
    );
}

#[test]
fn native_spreadsheet_structural_mutations_are_available_in_atomic_batches() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("batch.xlsx");
    let mutations = temp.path().join("mutations.json");
    let provider = temp.path().join("must-not-be-invoked");
    let document_text = document.to_str().unwrap();

    run(
        &provider,
        &["office", "native", "create", document_text, "--json"],
    );
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document_text,
            "/Sheet1/A1",
            "--text",
            "move",
            "--json",
        ],
    );
    std::fs::write(
        &mutations,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "mutations": [
                {
                    "operation": "insert-rows",
                    "sheet": "/Sheet1",
                    "start": 1,
                    "count": 2
                },
                {
                    "operation": "insert-columns",
                    "sheet": "/Sheet1",
                    "start": "A",
                    "count": 1
                },
                {
                    "operation": "copy-worksheet",
                    "path": "/Sheet1",
                    "name": "Clone",
                    "position": 2
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let batch = run(
        &provider,
        &[
            "office",
            "native",
            "batch",
            document_text,
            "--input",
            mutations.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(batch["data"]["result"]["applied"], 3);
    assert_eq!(batch["data"]["atomic"], true);

    let moved = run(
        &provider,
        &[
            "office",
            "native",
            "get",
            document_text,
            "/Sheet1/B3",
            "--json",
        ],
    );
    assert_eq!(moved["data"]["node"]["text"], "move");
    let cloned = run(
        &provider,
        &[
            "office",
            "native",
            "get",
            document_text,
            "/Clone/B3",
            "--json",
        ],
    );
    assert_eq!(cloned["data"]["node"]["text"], "move");
}
