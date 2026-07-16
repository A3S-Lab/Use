use crate::{NativeOfficeDocument, NativeOfficeEditor};

#[tokio::test]
async fn structural_edits_rewrite_tables_comments_drawings_vml_and_chart_formulas() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("related.xlsx");
    let editor = NativeOfficeEditor::create(&path).await.unwrap();
    let mut package = editor.package().clone();

    let parts = [
        (
            "xl/tables/table1.xml",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml",
            br#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Table1" displayName="Table1" ref="A1:B3"><autoFilter ref="A1:B3"/><tableColumns count="2"><tableColumn id="1" name="A"/><tableColumn id="2" name="B"/></tableColumns></table>"#.as_slice(),
        ),
        (
            "xl/comments1.xml",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml",
            br#"<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>A3S</author></authors><commentList><comment ref="B2" authorId="0"><text><t>note</t></text></comment></commentList></comments>"#.as_slice(),
        ),
        (
            "xl/drawings/vmlDrawing1.vml",
            "application/vnd.openxmlformats-officedocument.vmlDrawing",
            br#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="note"><x:ClientData ObjectType="Note"><x:Row>1</x:Row><x:Column>1</x:Column></x:ClientData></v:shape></xml>"#.as_slice(),
        ),
        (
            "xl/drawings/drawing1.xml",
            "application/vnd.openxmlformats-officedocument.drawing+xml",
            br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:row>1</xdr:row></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:row>2</xdr:row></xdr:to><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#.as_slice(),
        ),
        (
            "xl/charts/chart1.xml",
            "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
            br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:ser><c:val><c:numRef><c:f>'Sheet1'!$A$1:$B$3</c:f><c:numCache><c:ptCount val="1"/><c:pt idx="0"><c:v>1</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#.as_slice(),
        ),
    ];
    for (part, content_type, bytes) in parts {
        package.set_part(part, bytes.to_vec()).unwrap();
        crate::opc_edit::add_content_type_override(&mut package, part, content_type).unwrap();
    }
    for (relationship_type, target) in [
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/table",
            "../tables/table1.xml",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments",
            "../comments1.xml",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing",
            "../drawings/vmlDrawing1.vml",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing",
            "../drawings/drawing1.xml",
        ),
    ] {
        crate::opc_edit::add_relationship(
            &mut package,
            "xl/worksheets/_rels/sheet1.xml.rels",
            relationship_type,
            target,
        )
        .unwrap();
    }
    crate::opc_edit::add_relationship(
        &mut package,
        "xl/drawings/_rels/drawing1.xml.rels",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart",
        "../charts/chart1.xml",
    )
    .unwrap();

    let mut editor = NativeOfficeEditor::from_package(package).unwrap();
    editor.insert_rows("/Sheet1", 2, 1).unwrap();

    let table = part_text(&editor, "xl/tables/table1.xml");
    assert!(table.contains("ref=\"A1:B4\""));
    let comments = part_text(&editor, "xl/comments1.xml");
    assert!(comments.contains("ref=\"B3\""));
    let vml = part_text(&editor, "xl/drawings/vmlDrawing1.vml");
    assert!(vml.contains("<x:Row>2</x:Row>"));
    let drawing = part_text(&editor, "xl/drawings/drawing1.xml");
    assert!(drawing.contains("<xdr:row>2</xdr:row>"));
    assert!(drawing.contains("<xdr:row>3</xdr:row>"));
    let chart = part_text(&editor, "xl/charts/chart1.xml");
    assert!(chart.contains("&apos;Sheet1&apos;!$A$1:$B$4"), "{chart}");
    assert!(!chart.contains("numCache"));

    editor.insert_columns("/Sheet1", "B", 1).unwrap();
    let expanded_table = part_text(&editor, "xl/tables/table1.xml");
    assert!(expanded_table.contains("ref=\"A1:C4\""));
    assert!(expanded_table.contains("tableColumns count=\"3\""));
    assert!(expanded_table.contains("name=\"Column3\""));
    assert!(part_text(&editor, "xl/comments1.xml").contains("ref=\"C3\""));
    assert!(part_text(&editor, "xl/drawings/vmlDrawing1.vml").contains("<x:Column>2</x:Column>"));

    editor.delete_columns("/Sheet1", "B", 2).unwrap();
    let table = part_text(&editor, "xl/tables/table1.xml");
    assert!(table.contains("ref=\"A1:A4\""));
    assert!(table.contains("tableColumns count=\"1\""));
    assert!(!table.contains("name=\"B\""));
    assert!(!part_text(&editor, "xl/comments1.xml").contains("<comment "));
    assert!(!part_text(&editor, "xl/drawings/vmlDrawing1.vml").contains("<v:shape"));
    assert!(part_text(&editor, "xl/charts/chart1.xml").contains("&apos;Sheet1&apos;!$A$1:$A$4"));
    NativeOfficeDocument::from_package(editor.package().clone()).unwrap();
}

fn part_text(editor: &NativeOfficeEditor, part: &str) -> String {
    String::from_utf8(editor.package().part(part).unwrap().to_vec()).unwrap()
}
