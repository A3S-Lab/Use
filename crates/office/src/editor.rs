use std::path::Path;

use a3s_use_core::{UseError, UseResult};

use crate::discovery::office_error;
use crate::semantic::NativeOfficeDocument;
use crate::{
    template_merge, DocumentKind, NativeOfficePackage, NativeOfficeReplayArtifact,
    NativeOfficeTemplateMergeResult,
};

mod hyperlink;
mod image;
mod part;
mod presentation;
mod raw;
mod spreadsheet;
mod types;
mod word;

pub(crate) use image::inspect_image;

#[cfg(test)]
mod part_tests;

pub use part::{NativeCreatedPart, NativeOfficePartType};
pub use raw::NativeRawXmlPart;
pub use types::{
    NativeBatchResult, NativeCreatedImage, NativeOfficeHorizontalAlignment, NativeOfficeHyperlink,
    NativeOfficeHyperlinkTarget, NativeOfficeImage, NativeOfficeImageFormat,
    NativeOfficeImageMetadata, NativeOfficeInsertPosition, NativeOfficeMutation,
    NativeOfficeRgbColor, NativeOfficeSwapResult, NativeOfficeTextFormat, SpreadsheetCellValue,
};

/// Loss-preserving OOXML editor with transactional in-memory batches.
#[derive(Debug, Clone)]
pub struct NativeOfficeEditor {
    package: NativeOfficePackage,
}

impl NativeOfficeEditor {
    pub async fn create(path: impl AsRef<Path>) -> UseResult<Self> {
        Self::from_package(NativeOfficePackage::create(path).await?)
    }

    pub async fn open(path: impl AsRef<Path>) -> UseResult<Self> {
        let package = NativeOfficePackage::open(path).await?;
        NativeOfficeDocument::from_package(package.clone())?;
        Ok(Self { package })
    }

    pub fn from_package(package: NativeOfficePackage) -> UseResult<Self> {
        NativeOfficeDocument::from_package(package.clone())?;
        Ok(Self { package })
    }

    pub fn package(&self) -> &NativeOfficePackage {
        &self.package
    }

    /// Safely parses and returns an existing OOXML XML part.
    pub fn raw_xml_part(&self, part: &str) -> UseResult<NativeRawXmlPart> {
        raw::inspect(&self.package, part)
    }

    pub fn snapshot(&self) -> UseResult<NativeOfficeDocument> {
        NativeOfficeDocument::from_package(self.package.clone())
    }

    pub fn is_dirty(&self) -> bool {
        self.package.is_dirty()
    }

    pub fn set_text(&mut self, path: impl Into<String>, text: impl Into<String>) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetText {
            path: path.into(),
            text: text.into(),
        }])?;
        Ok(())
    }

    /// Applies typed rich-text properties without changing document content.
    pub fn set_text_format(
        &mut self,
        path: impl Into<String>,
        format: NativeOfficeTextFormat,
    ) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetTextFormat {
            path: path.into(),
            format,
        }])?;
        Ok(())
    }

    /// Creates or replaces a typed hyperlink on a supported semantic owner.
    pub fn set_hyperlink(
        &mut self,
        path: impl Into<String>,
        hyperlink: NativeOfficeHyperlink,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::SetHyperlink {
            path: path.into(),
            hyperlink,
        })
    }

    /// Sets one Presentation table-grid column width in English Metric Units.
    pub fn set_table_column_width(
        &mut self,
        path: impl Into<String>,
        width_emu: u64,
    ) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetTableColumnWidth {
            path: path.into(),
            width_emu,
        }])?;
        Ok(())
    }

    pub fn set_cell_value(
        &mut self,
        path: impl Into<String>,
        value: SpreadsheetCellValue,
    ) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetCellValue {
            path: path.into(),
            value,
        }])?;
        Ok(())
    }

    pub fn add_paragraph(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddParagraph {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office paragraph mutation returned no path.",
            )
        })
    }

    pub fn add_slide(
        &mut self,
        parent: impl Into<String>,
        title: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddSlide {
            parent: parent.into(),
            title: title.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office slide mutation returned no path.",
            )
        })
    }

    pub fn add_table(
        &mut self,
        parent: impl Into<String>,
        rows: usize,
        columns: usize,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTable {
            parent: parent.into(),
            rows,
            columns,
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table mutation returned no path.",
            )
        })
    }

    pub fn add_table_row(
        &mut self,
        parent: impl Into<String>,
        columns: Option<usize>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTableRow {
            parent: parent.into(),
            columns,
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table-row mutation returned no path.",
            )
        })
    }

    /// Inserts one Presentation table column at a zero-based grid position.
    ///
    /// Omitting `index` appends the column. Every row receives one new cell so
    /// the DrawingML table grid remains rectangular.
    pub fn add_table_column(
        &mut self,
        parent: impl Into<String>,
        index: Option<usize>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTableColumn {
            parent: parent.into(),
            index,
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table-column mutation returned no path.",
            )
        })
    }

    pub fn add_table_cell(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTableCell {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table-cell mutation returned no path.",
            )
        })
    }

    pub fn add_shape(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddShape {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office shape mutation returned no path.",
            )
        })
    }

    /// Embeds a validated PNG, JPEG, or GIF as a native DrawingML picture.
    pub fn add_image(
        &mut self,
        parent: impl Into<String>,
        image: NativeOfficeImage,
    ) -> UseResult<NativeCreatedImage> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddImage {
            parent: parent.into(),
            image,
        }])?;
        result.created_images.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office image mutation returned no creation receipt.",
            )
        })
    }

    /// Creates a known XML part with its content type and owner relationship.
    pub fn add_part(
        &mut self,
        parent: impl Into<String>,
        part_type: NativeOfficePartType,
    ) -> UseResult<NativeCreatedPart> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddPart {
            parent: parent.into(),
            part_type,
        }])?;
        result.created_parts.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office part mutation returned no creation receipt.",
            )
        })
    }

    pub fn remove(&mut self, path: impl Into<String>) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::Remove { path: path.into() }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office remove mutation returned no path.",
            )
        })
    }

    pub fn add_worksheet(&mut self, name: impl Into<String>) -> UseResult<String> {
        let result =
            self.apply_batch(&[NativeOfficeMutation::AddWorksheet { name: name.into() }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office worksheet mutation returned no path.",
            )
        })
    }

    pub fn insert_rows(
        &mut self,
        sheet: impl Into<String>,
        start: u32,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::InsertRows {
            sheet: sheet.into(),
            start,
            count,
        })
    }

    pub fn delete_rows(
        &mut self,
        sheet: impl Into<String>,
        start: u32,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::DeleteRows {
            sheet: sheet.into(),
            start,
            count,
        })
    }

    pub fn insert_columns(
        &mut self,
        sheet: impl Into<String>,
        start: impl Into<String>,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::InsertColumns {
            sheet: sheet.into(),
            start: start.into(),
            count,
        })
    }

    pub fn delete_columns(
        &mut self,
        sheet: impl Into<String>,
        start: impl Into<String>,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::DeleteColumns {
            sheet: sheet.into(),
            start: start.into(),
            count,
        })
    }

    pub fn rename_worksheet(
        &mut self,
        path: impl Into<String>,
        name: impl Into<String>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::RenameWorksheet {
            path: path.into(),
            name: name.into(),
        })
    }

    pub fn move_worksheet(
        &mut self,
        path: impl Into<String>,
        position: usize,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::MoveWorksheet {
            path: path.into(),
            position,
        })
    }

    pub fn copy_worksheet(
        &mut self,
        path: impl Into<String>,
        name: impl Into<String>,
        position: Option<usize>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::CopyWorksheet {
            path: path.into(),
            name: name.into(),
            position,
        })
    }

    pub fn move_node(
        &mut self,
        path: impl Into<String>,
        target_parent: Option<String>,
        position: Option<NativeOfficeInsertPosition>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::Move {
            path: path.into(),
            target_parent,
            position,
        })
    }

    pub fn copy_node(
        &mut self,
        path: impl Into<String>,
        target_parent: Option<String>,
        position: Option<NativeOfficeInsertPosition>,
        name: Option<String>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::Copy {
            path: path.into(),
            target_parent,
            position,
            name,
        })
    }

    pub fn swap_nodes(
        &mut self,
        path: impl Into<String>,
        with: impl Into<String>,
    ) -> UseResult<NativeOfficeSwapResult> {
        let result = self.apply_batch(&[NativeOfficeMutation::Swap {
            path: path.into(),
            with: with.into(),
        }])?;
        result.swaps.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office swap mutation returned no swap receipt.",
            )
        })
    }

    /// Replaces an existing, non-OPC-metadata XML part transactionally.
    pub fn replace_xml_part(
        &mut self,
        part: impl Into<String>,
        xml: impl Into<String>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::ReplaceXmlPart {
            part: part.into(),
            xml: xml.into(),
        })
    }

    fn single_path(&mut self, mutation: NativeOfficeMutation) -> UseResult<String> {
        let result = self.apply_batch(&[mutation])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office mutation returned no path.",
            )
        })
    }

    pub fn apply_batch(
        &mut self,
        mutations: &[NativeOfficeMutation],
    ) -> UseResult<NativeBatchResult> {
        if mutations.is_empty() {
            return Ok(NativeBatchResult {
                applied: 0,
                paths: Vec::new(),
                swaps: Vec::new(),
                created_parts: Vec::new(),
                created_images: Vec::new(),
            });
        }
        let original = self.package.clone();
        let mut paths = Vec::with_capacity(mutations.len());
        let mut swaps = Vec::new();
        let mut created_parts = Vec::new();
        let mut created_images = Vec::new();
        for mutation in mutations {
            let mut created_part = None;
            let mut created_image = None;
            let mut swap = None;
            let result = match mutation {
                NativeOfficeMutation::SetText { path, text } => {
                    set_text(&mut self.package, path, text).map(|()| path.clone())
                }
                NativeOfficeMutation::SetTextFormat { path, format } => format
                    .validate()
                    .and_then(|()| match self.package.kind() {
                        DocumentKind::Word => {
                            word::set_text_format(&mut self.package, path, format)
                        }
                        DocumentKind::Spreadsheet => {
                            spreadsheet::set_text_format(&mut self.package, path, format)
                        }
                        DocumentKind::Presentation => {
                            presentation::set_text_format(&mut self.package, path, format)
                        }
                    })
                    .map(|()| path.clone()),
                NativeOfficeMutation::SetHyperlink { path, hyperlink } => hyperlink
                    .validate()
                    .and_then(|()| hyperlink::set(&mut self.package, path, hyperlink)),
                NativeOfficeMutation::SetTableColumnWidth { path, width_emu } => {
                    presentation::set_table_column_width(&mut self.package, path, *width_emu)
                        .map(|()| path.clone())
                }
                NativeOfficeMutation::SetCellValue { path, value } => {
                    spreadsheet::set_cell_value(&mut self.package, path, value)
                        .map(|()| path.clone())
                }
                NativeOfficeMutation::AddParagraph { parent, text } => {
                    word::add_paragraph(&mut self.package, parent, text)
                }
                NativeOfficeMutation::AddTable {
                    parent,
                    rows,
                    columns,
                } => match self.package.kind() {
                    DocumentKind::Presentation => {
                        presentation::add_table(&mut self.package, parent, *rows, *columns)
                    }
                    _ => word::add_table(&mut self.package, parent, *rows, *columns),
                },
                NativeOfficeMutation::AddTableRow { parent, columns } => {
                    match self.package.kind() {
                        DocumentKind::Presentation => {
                            presentation::add_table_row(&mut self.package, parent, *columns)
                        }
                        _ => word::add_table_row(&mut self.package, parent, *columns),
                    }
                }
                NativeOfficeMutation::AddTableColumn {
                    parent,
                    index,
                    text,
                } => presentation::add_table_column(&mut self.package, parent, *index, text),
                NativeOfficeMutation::AddTableCell { parent, text } => match self.package.kind() {
                    DocumentKind::Presentation => {
                        presentation::add_table_cell(&mut self.package, parent, text)
                    }
                    _ => word::add_table_cell(&mut self.package, parent, text),
                },
                NativeOfficeMutation::AddSlide { parent, title } => {
                    presentation::add_slide(&mut self.package, parent, title)
                }
                NativeOfficeMutation::AddShape { parent, text } => {
                    presentation::add_shape(&mut self.package, parent, text)
                }
                NativeOfficeMutation::AddImage { parent, image } => {
                    image::add(&mut self.package, parent, image).map(|created| {
                        let path = created.path.clone();
                        created_image = Some(created);
                        path
                    })
                }
                NativeOfficeMutation::AddPart { parent, part_type } => {
                    part::add(&mut self.package, parent, *part_type).map(|created| {
                        let path = created.path.clone();
                        created_part = Some(created);
                        path
                    })
                }
                NativeOfficeMutation::AddWorksheet { name } => {
                    spreadsheet::add_worksheet(&mut self.package, name)
                }
                NativeOfficeMutation::InsertRows {
                    sheet,
                    start,
                    count,
                } => spreadsheet::insert_rows(&mut self.package, sheet, *start, *count),
                NativeOfficeMutation::DeleteRows {
                    sheet,
                    start,
                    count,
                } => spreadsheet::delete_rows(&mut self.package, sheet, *start, *count),
                NativeOfficeMutation::InsertColumns {
                    sheet,
                    start,
                    count,
                } => spreadsheet::insert_columns(&mut self.package, sheet, start, *count),
                NativeOfficeMutation::DeleteColumns {
                    sheet,
                    start,
                    count,
                } => spreadsheet::delete_columns(&mut self.package, sheet, start, *count),
                NativeOfficeMutation::RenameWorksheet { path, name } => {
                    spreadsheet::rename_worksheet(&mut self.package, path, name)
                }
                NativeOfficeMutation::MoveWorksheet { path, position } => {
                    spreadsheet::move_worksheet(&mut self.package, path, *position)
                }
                NativeOfficeMutation::CopyWorksheet {
                    path,
                    name,
                    position,
                } => spreadsheet::copy_worksheet(&mut self.package, path, name, *position),
                NativeOfficeMutation::Move {
                    path,
                    target_parent,
                    position,
                } => move_node(
                    &mut self.package,
                    path,
                    target_parent.as_deref(),
                    position.as_ref(),
                ),
                NativeOfficeMutation::Copy {
                    path,
                    target_parent,
                    position,
                    name,
                } => copy_node(
                    &mut self.package,
                    path,
                    target_parent.as_deref(),
                    position.as_ref(),
                    name.as_deref(),
                ),
                NativeOfficeMutation::Swap { path, with } => {
                    swap_nodes(&mut self.package, path, with).map(|receipt| {
                        let primary = receipt.first.clone();
                        swap = Some(receipt);
                        primary
                    })
                }
                NativeOfficeMutation::ReplaceXmlPart { part, xml } => {
                    raw::replace(&mut self.package, part, xml)
                }
                NativeOfficeMutation::Remove { path } => {
                    remove_node(&mut self.package, path).map(|()| path.clone())
                }
            };
            match result {
                Ok(path) => {
                    paths.push(path);
                    if let Some(created) = created_part {
                        created_parts.push(created);
                    }
                    if let Some(created) = created_image {
                        created_images.push(created);
                    }
                    if let Some(receipt) = swap {
                        swaps.push(receipt);
                    }
                }
                Err(error) => {
                    self.package = original;
                    return Err(error);
                }
            }
        }
        if let Err(error) = NativeOfficeDocument::from_package(self.package.clone()) {
            self.package = original;
            return Err(editor_error(
                "use.office.batch_validation_failed",
                format!("Native Office batch failed post-mutation validation: {error}"),
            ));
        }
        Ok(NativeBatchResult {
            applied: paths.len(),
            paths,
            swaps,
            created_parts,
            created_images,
        })
    }

    /// Applies a dump-produced replay artifact with blank-base and final-state
    /// fingerprint checks around the existing atomic mutation boundary.
    pub fn apply_replay(
        &mut self,
        artifact: &NativeOfficeReplayArtifact,
    ) -> UseResult<NativeBatchResult> {
        artifact.validate()?;
        if artifact.document_kind != self.package.kind() {
            return Err(editor_error(
                "use.office.replay_kind_mismatch",
                format!(
                    "Native Office replay targets {:?}, but the document is {:?}.",
                    artifact.document_kind,
                    self.package.kind()
                ),
            ));
        }

        let observed_base = self.package.content_sha256();
        if observed_base != artifact.base_sha256 {
            return Err(editor_error(
                "use.office.replay_base_mismatch",
                "Native Office replay requires the exact blank package recorded by the artifact.",
            )
            .with_detail("expectedSha256", artifact.base_sha256.clone())
            .with_detail("observedSha256", observed_base));
        }

        let original = self.package.clone();
        let result = self.apply_batch(&artifact.mutations)?;
        let observed_result = self.package.content_sha256();
        if observed_result != artifact.result_sha256 {
            self.package = original;
            return Err(editor_error(
                "use.office.replay_result_mismatch",
                "Native Office replay did not produce the package fingerprint recorded by the artifact; all mutations were rolled back.",
            )
            .with_detail("expectedSha256", artifact.result_sha256.clone())
            .with_detail("observedSha256", observed_result));
        }
        Ok(result)
    }

    /// Replaces `{{key}}` placeholders across the document's native OOXML text
    /// surfaces. The in-memory package is restored if any part cannot be
    /// edited losslessly or if the resulting package fails semantic validation.
    pub fn merge_template(
        &mut self,
        data: &serde_json::Value,
    ) -> UseResult<NativeOfficeTemplateMergeResult> {
        let original = self.package.clone();
        let result = match template_merge::merge(&mut self.package, data) {
            Ok(result) => result,
            Err(error) => {
                self.package = original;
                return Err(error);
            }
        };
        if let Err(error) = NativeOfficeDocument::from_package(self.package.clone()) {
            self.package = original;
            return Err(editor_error(
                "use.office.template_validation_failed",
                format!("Native Office template merge failed post-mutation validation: {error}"),
            ));
        }
        Ok(result)
    }

    pub async fn save(&mut self) -> UseResult<()> {
        self.package.save().await
    }

    pub async fn save_as(&mut self, path: impl AsRef<Path>) -> UseResult<()> {
        self.package.save_as(path).await
    }

    pub async fn save_as_new(&mut self, path: impl AsRef<Path>) -> UseResult<()> {
        self.package.save_as_new(path).await
    }
}

fn set_text(package: &mut NativeOfficePackage, path: &str, text: &str) -> UseResult<()> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::set_text(package, path, text),
        DocumentKind::Spreadsheet => spreadsheet::set_text(package, path, text),
        DocumentKind::Presentation => presentation::set_text(package, path, text),
    }
}

fn remove_node(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    validate_mutation_path(path)?;
    let node_type = NativeOfficeDocument::from_package(package.clone())?
        .get(path, 0)?
        .node_type;
    if node_type == crate::OfficeNodeType::Picture {
        return image::remove(package, path);
    }
    if node_type == crate::OfficeNodeType::Hyperlink {
        return hyperlink::remove(package, path);
    }
    match package.kind() {
        DocumentKind::Word => word::remove(package, path),
        DocumentKind::Spreadsheet => spreadsheet::remove(package, path),
        DocumentKind::Presentation => presentation::remove(package, path),
    }
}

fn move_node(
    package: &mut NativeOfficePackage,
    path: &str,
    target_parent: Option<&str>,
    position: Option<&NativeOfficeInsertPosition>,
) -> UseResult<String> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::move_node(package, path, target_parent, position),
        DocumentKind::Spreadsheet => spreadsheet::move_node(package, path, target_parent, position),
        DocumentKind::Presentation => {
            presentation::move_node(package, path, target_parent, position)
        }
    }
}

fn copy_node(
    package: &mut NativeOfficePackage,
    path: &str,
    target_parent: Option<&str>,
    position: Option<&NativeOfficeInsertPosition>,
    name: Option<&str>,
) -> UseResult<String> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::copy_node(package, path, target_parent, position, name),
        DocumentKind::Spreadsheet => {
            spreadsheet::copy_node(package, path, target_parent, position, name)
        }
        DocumentKind::Presentation => {
            presentation::copy_node(package, path, target_parent, position, name)
        }
    }
}

fn swap_nodes(
    package: &mut NativeOfficePackage,
    path: &str,
    with: &str,
) -> UseResult<NativeOfficeSwapResult> {
    validate_mutation_path(path)?;
    validate_mutation_path(with)?;
    match package.kind() {
        DocumentKind::Word => word::swap_nodes(package, path, with),
        DocumentKind::Spreadsheet => spreadsheet::swap_nodes(package, path, with),
        DocumentKind::Presentation => presentation::swap_nodes(package, path, with),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSegment {
    name: String,
    position: Option<usize>,
}

fn parse_segments(path: &str) -> UseResult<Vec<PathSegment>> {
    validate_mutation_path(path)?;
    path.trim_start_matches('/')
        .split('/')
        .map(|segment| {
            if let Some((name, position)) = segment.split_once('[') {
                let position = position.strip_suffix(']').ok_or_else(|| {
                    editor_error(
                        "use.office.path_invalid",
                        format!("Office path segment '{segment}' is missing ']'."),
                    )
                })?;
                let position = position.parse::<usize>().map_err(|_| {
                    editor_error(
                        "use.office.path_invalid",
                        format!("Office mutation path segment '{segment}' is not numeric."),
                    )
                })?;
                if name.is_empty() || position == 0 {
                    return Err(editor_error(
                        "use.office.path_invalid",
                        "Office mutation paths use non-empty, one-based segments.",
                    ));
                }
                Ok(PathSegment {
                    name: name.to_ascii_lowercase(),
                    position: Some(position),
                })
            } else if segment.is_empty() {
                Err(editor_error(
                    "use.office.path_invalid",
                    "Office mutation paths cannot contain empty segments.",
                ))
            } else {
                Ok(PathSegment {
                    name: segment.to_ascii_lowercase(),
                    position: None,
                })
            }
        })
        .collect()
}

fn validate_mutation_path(path: &str) -> UseResult<()> {
    if path.is_empty()
        || path == "/"
        || !path.starts_with('/')
        || path.len() > 4_096
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(editor_error(
            "use.office.path_invalid",
            "Office mutation path must be absolute, bounded, and traversal-free.",
        ));
    }
    Ok(())
}

fn prefix(qualified_name: &str) -> Option<&str> {
    qualified_name.rsplit_once(':').map(|(prefix, _)| prefix)
}

fn qualified(prefix: Option<&str>, local_name: &str) -> String {
    prefix.map_or_else(
        || local_name.to_string(),
        |prefix| format!("{prefix}:{local_name}"),
    )
}

fn preserve_space_attribute(text: &str) -> &'static str {
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        " xml:space=\"preserve\""
    } else {
        ""
    }
}

fn escape_attribute(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn node_not_found(path: &str) -> UseError {
    editor_error(
        "use.office.node_not_found",
        format!("Office semantic path '{path}' does not exist."),
    )
    .with_detail("path", path)
}

fn editor_error(code: &str, message: impl Into<String>) -> UseError {
    office_error(code, message)
}
