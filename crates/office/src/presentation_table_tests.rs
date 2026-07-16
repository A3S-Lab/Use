use crate::{NativeOfficeDocument, NativeOfficeEditor, NativeOfficeMutation, OfficeNodeType};

const ROW_HEIGHT_EMU: u64 = 370_840;
const TRANSITIONAL_DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const STRICT_DRAWING: &str = "http://purl.oclc.org/ooxml/drawingml/main";
const TRANSITIONAL_PRESENTATION: &str =
    "http://schemas.openxmlformats.org/presentationml/2006/main";
const STRICT_PRESENTATION: &str = "http://purl.oclc.org/ooxml/presentationml/main";

#[tokio::test]
async fn native_editor_adds_edits_and_removes_presentation_tables() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tables.pptx");
    let mut editor = NativeOfficeEditor::create(&path).await.unwrap();
    editor.add_slide("/", "Table slide").unwrap();
    let slide_part = slide_part(&editor);
    let extended_shape_tree =
        part_text(&editor, &slide_part).replacen("</p:spTree>", "<p:extLst/></p:spTree>", 1);
    editor
        .replace_xml_part(&slide_part, extended_shape_tree)
        .unwrap();

    assert_eq!(
        editor.add_table("/slide[1]", 0, 2).unwrap_err().code,
        "use.office.presentation_table_dimensions_invalid"
    );

    let result = editor
        .apply_batch(&[
            NativeOfficeMutation::AddTable {
                parent: "/slide[1]".into(),
                rows: 2,
                columns: 2,
            },
            NativeOfficeMutation::SetText {
                path: "/slide[1]/table[1]/tr[1]/tc[1]".into(),
                text: "Name".into(),
            },
            NativeOfficeMutation::SetText {
                path: "/slide[1]/table[1]/tr[1]/tc[2]".into(),
                text: "Value".into(),
            },
        ])
        .unwrap();
    assert_eq!(result.paths[0], "/slide[1]/table[1]");

    let table = editor
        .snapshot()
        .unwrap()
        .get("/slide[1]/table[1]", 3)
        .unwrap();
    assert_eq!(table.node_type, OfficeNodeType::Table);
    assert_eq!(table.child_count, 2);
    assert_eq!(table.children[0].child_count, 2);
    assert_eq!(table.children[0].children[0].text, "Name");
    assert_eq!(table.children[0].children[1].text, "Value");
    assert_eq!(table.format["name"], "Table 1");
    assert!(part_text(&editor, &slide_part).contains("</p:graphicFrame><p:extLst/>"));

    let extended_table =
        part_text(&editor, &slide_part).replacen("</a:tbl>", "<a:extLst/></a:tbl>", 1);
    editor
        .replace_xml_part(&slide_part, extended_table)
        .unwrap();

    let row = editor.add_table_row("/slide[1]/table[1]", Some(2)).unwrap();
    assert_eq!(row, "/slide[1]/table[1]/tr[3]");
    assert_eq!(
        editor.add_table_cell(&row, "overflow").unwrap_err().code,
        "use.office.presentation_table_cell_grid_full"
    );
    editor
        .set_text("/slide[1]/table[1]/tr[3]/tc[1]", "Blank cell updated")
        .unwrap();
    assert_eq!(
        editor
            .snapshot()
            .unwrap()
            .get("/slide[1]/table[1]/tr[3]/tc[1]", 1)
            .unwrap()
            .text,
        "Blank cell updated"
    );

    let slide_xml = part_text(&editor, &slide_part);
    assert!(slide_xml.contains("<p:graphicFrame"));
    assert!(slide_xml.contains("uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\""));
    assert_eq!(slide_xml.matches("<a:gridCol").count(), 2);
    assert!(slide_xml.contains(&format!("cy=\"{}\"", ROW_HEIGHT_EMU * 3)));
    assert!(slide_xml.contains("</a:tr><a:extLst/></a:tbl>"));

    let underfilled = add_extension_to_last_row(remove_last_table_cell(slide_xml));
    editor.replace_xml_part(&slide_part, underfilled).unwrap();
    let cell = editor.add_table_cell(&row, "Restored").unwrap();
    assert_eq!(cell, "/slide[1]/table[1]/tr[3]/tc[2]");
    assert_eq!(
        editor.snapshot().unwrap().get(&cell, 1).unwrap().text,
        "Restored"
    );
    assert!(part_text(&editor, &slide_part).contains("</a:tc><a:extLst/></a:tr>"));

    let overflow = duplicate_last_table_cell(part_text(&editor, &slide_part));
    editor.replace_xml_part(&slide_part, overflow).unwrap();
    editor.remove("/slide[1]/table[1]/tr[3]/tc[3]").unwrap();
    assert_eq!(
        editor
            .remove("/slide[1]/table[1]/tr[3]/tc[2]")
            .unwrap_err()
            .code,
        "use.office.presentation_table_cell_grid_invalid"
    );

    editor.remove(&row).unwrap();
    assert!(part_text(&editor, &slide_part).contains(&format!("cy=\"{}\"", ROW_HEIGHT_EMU * 2)));
    editor.remove("/slide[1]/table[1]/tr[2]").unwrap();
    assert_eq!(
        editor.remove("/slide[1]/table[1]/tr[1]").unwrap_err().code,
        "use.office.presentation_last_table_row"
    );
    editor.remove("/slide[1]/table[1]").unwrap();
    assert_eq!(editor.snapshot().unwrap().statistics().table_count, 0);

    editor.save().await.unwrap();
    NativeOfficeDocument::open(&path).await.unwrap();
}

#[tokio::test]
async fn presentation_table_mutations_are_atomic_and_enforce_the_table_grid() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("atomic-tables.pptx");
    let mut editor = NativeOfficeEditor::create(&path).await.unwrap();
    editor.add_slide("/", "Atomic").unwrap();
    let before = editor.package().content_sha256();

    let error = editor
        .apply_batch(&[
            NativeOfficeMutation::AddTable {
                parent: "/slide[1]".into(),
                rows: 2,
                columns: 2,
            },
            NativeOfficeMutation::SetText {
                path: "/slide[1]/table[1]/tr[99]/tc[1]".into(),
                text: "missing".into(),
            },
        ])
        .unwrap_err();
    assert_eq!(error.code, "use.office.node_not_found");
    assert_eq!(editor.package().content_sha256(), before);
    assert_eq!(editor.snapshot().unwrap().statistics().table_count, 0);

    assert_eq!(
        editor.add_table("/slide[1]", 5_001, 1).unwrap_err().code,
        "use.office.presentation_table_limit"
    );

    editor.add_table("/slide[1]", 1, 2).unwrap();
    assert_eq!(
        editor
            .add_table_row("/slide[1]/table[1]", Some(3))
            .unwrap_err()
            .code,
        "use.office.presentation_table_row_grid_mismatch"
    );

    editor.add_table_row("/slide[1]/table[1]", Some(2)).unwrap();
    let slide_part = slide_part(&editor);
    let merged = part_text(&editor, &slide_part).replacen("<a:tc>", "<a:tc gridSpan=\"2\">", 1);
    editor.replace_xml_part(&slide_part, merged).unwrap();
    let before = editor.package().content_sha256();
    assert_eq!(
        editor.remove("/slide[1]/table[1]/tr[1]").unwrap_err().code,
        "use.office.presentation_table_merge_unsupported"
    );
    assert_eq!(editor.package().content_sha256(), before);
}

#[tokio::test]
async fn presentation_tables_follow_the_strict_slide_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("strict-tables.pptx");
    let mut editor = NativeOfficeEditor::create(&path).await.unwrap();
    editor.add_slide("/", "Strict").unwrap();
    let slide_part = slide_part(&editor);
    let mut package = editor.package().clone();
    for part_name in ["ppt/presentation.xml", slide_part.as_str()] {
        let source =
            String::from_utf8(package.xml_part(part_name).unwrap().raw().to_vec()).unwrap();
        let strict = source
            .replace(TRANSITIONAL_PRESENTATION, STRICT_PRESENTATION)
            .replace(TRANSITIONAL_DRAWING, STRICT_DRAWING);
        package.set_part(part_name, strict.into_bytes()).unwrap();
    }
    let mut editor = NativeOfficeEditor::from_package(package).unwrap();

    editor.add_table("/slide[1]", 1, 1).unwrap();

    let slide_xml = part_text(&editor, &slide_part);
    assert!(slide_xml.contains(&format!("<p:graphicFrame xmlns:a=\"{STRICT_DRAWING}\"")));
    assert!(!slide_xml.contains(&format!(
        "<p:graphicFrame xmlns:a=\"{TRANSITIONAL_DRAWING}\""
    )));
}

fn slide_part(editor: &NativeOfficeEditor) -> String {
    editor
        .snapshot()
        .unwrap()
        .get("/slide[1]", 0)
        .unwrap()
        .format["part"]
        .clone()
}

fn part_text(editor: &NativeOfficeEditor, part: &str) -> String {
    String::from_utf8(editor.package().xml_part(part).unwrap().raw().to_vec()).unwrap()
}

fn remove_last_table_cell(mut xml: String) -> String {
    let row_start = xml.rfind("<a:tr").unwrap();
    let cell_start = row_start + xml[row_start..].rfind("<a:tc>").unwrap();
    let cell_end = cell_start + xml[cell_start..].find("</a:tc>").unwrap() + "</a:tc>".len();
    xml.replace_range(cell_start..cell_end, "");
    xml
}

fn duplicate_last_table_cell(mut xml: String) -> String {
    let row_start = xml.rfind("<a:tr").unwrap();
    let cell_start = row_start + xml[row_start..].rfind("<a:tc>").unwrap();
    let cell_end = cell_start + xml[cell_start..].find("</a:tc>").unwrap() + "</a:tc>".len();
    let fragment = xml[cell_start..cell_end].to_string();
    xml.insert_str(cell_end, &fragment);
    xml
}

fn add_extension_to_last_row(mut xml: String) -> String {
    let row_end = xml.rfind("</a:tr>").unwrap();
    xml.insert_str(row_end, "<a:extLst/>");
    xml
}
