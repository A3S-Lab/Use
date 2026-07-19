use std::path::PathBuf;

use a3s_use_ocr::{OcrProviderKind, OcrRequest};

#[test]
fn public_contract_names_only_pp_ocr_v6() {
    assert_eq!(
        serde_json::to_value(OcrProviderKind::PpOcrV6).unwrap(),
        serde_json::json!("pp-ocr-v6")
    );

    let request = OcrRequest {
        path: PathBuf::from("scan.png"),
    };
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        serde_json::json!({ "path": "scan.png" })
    );
}
