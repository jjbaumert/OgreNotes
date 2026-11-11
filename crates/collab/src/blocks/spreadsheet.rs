// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Embedded workbooks keep cells in independent XML attribute registers. The
//! shared decoder owns cell encodings and limits; this module validates the
//! surrounding XML shape and produces bounded, computed export tables.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use ogrenotes_spreadsheet::embedded::{
    MAX_CELL_CHARS, MAX_SHEETS, MAX_STORED_CELLS, SheetData, decode_sheet, engine_for_sheet,
    validate_workbook,
};
use ogrenotes_spreadsheet::parser::col_to_letters;
use yrs::{
    ReadTxn,
    types::xml::{Xml, XmlFragment, XmlOut},
};

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct SpreadsheetBlock;
pub static SPREADSHEET: SpreadsheetBlock = SpreadsheetBlock;

fn invalid(
    node_type: NodeType,
    field: impl Into<Cow<'static, str>>,
    reason: impl Into<String>,
) -> BlockValidationError {
    BlockValidationError {
        node_type,
        field: field.into(),
        reason: reason.into(),
    }
}

impl LiveAppBlock for SpreadsheetBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Spreadsheet, NodeType::SpreadsheetSheet]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        match node_type {
            NodeType::Spreadsheet => {
                if attrs.get("version").map(String::as_str) != Some("1") {
                    return Err(invalid(
                        node_type,
                        "version",
                        "unsupported spreadsheet version",
                    ));
                }
                let id = attrs.get("blockId").map(String::as_str).unwrap_or("");
                if id.is_empty() || id.chars().count() > 128 {
                    return Err(invalid(
                        node_type,
                        "blockId",
                        "workbook identity must contain 1..128 characters",
                    ));
                }
                if attrs
                    .get("title")
                    .is_some_and(|value| value.chars().count() > 128)
                {
                    return Err(invalid(node_type, "title", "title exceeds 128 characters"));
                }
                if let Some(key) = attrs
                    .keys()
                    .find(|key| !matches!(key.as_str(), "blockId" | "version" | "title"))
                {
                    return Err(invalid(
                        node_type,
                        key.to_string(),
                        "unknown workbook attribute",
                    ));
                }
            }
            NodeType::SpreadsheetSheet => {
                decode_sheet(attrs)
                    .map_err(|error| invalid(node_type, error.field, error.reason))?;
            }
            _ => return Err(invalid(node_type, "node_type", "not a spreadsheet node")),
        }
        Ok(attrs.clone())
    }
}

/// Validate both node shapes, including workbook-wide limits that cannot be
/// expressed by the attribute-only live-app trait. Used by the write gate and
/// by exports, which remain bounded even when validation rollout is in Log.
pub(crate) fn validate_shape<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
) -> Result<(), BlockValidationError> {
    match NodeType::from_tag(el.tag()) {
        Some(NodeType::Spreadsheet) => read_workbook(txn, el).map(|_| ()),
        Some(NodeType::SpreadsheetSheet) if !matches!(el.parent(), Some(XmlOut::Element(parent)) if parent.tag().as_ref() == NodeType::Spreadsheet.tag_name()) => {
            Err(invalid(
                NodeType::SpreadsheetSheet,
                "parent",
                "sheet must belong to a workbook",
            ))
        }
        Some(NodeType::SpreadsheetSheet) if el.len(txn) != 0 => Err(invalid(
            NodeType::SpreadsheetSheet,
            "children",
            "sheet must have no children",
        )),
        _ => Ok(()),
    }
}

fn read_attrs<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    node_type: NodeType,
) -> Result<HashMap<String, String>, BlockValidationError> {
    let mut attrs = HashMap::new();
    for (key, value) in el.attributes(txn) {
        if attrs.len() >= 4 + 2 * MAX_STORED_CELLS
            || key.len() > 128
            || value.chars().take(MAX_CELL_CHARS + 1).count() > MAX_CELL_CHARS
        {
            return Err(invalid(
                node_type,
                "attributes",
                "spreadsheet attributes exceed their bounds",
            ));
        }
        attrs.insert(key.to_string(), value.to_string());
    }
    Ok(attrs)
}

fn read_workbook<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
) -> Result<Vec<SheetData>, BlockValidationError> {
    let workbook_attrs = read_attrs(txn, el, NodeType::Spreadsheet)?;
    SPREADSHEET.validate_attrs(NodeType::Spreadsheet, &workbook_attrs)?;
    let count = el.len(txn) as usize;
    if count == 0 || count > MAX_SHEETS {
        return Err(invalid(
            NodeType::Spreadsheet,
            "children",
            format!("workbook must contain 1..{MAX_SHEETS} sheets"),
        ));
    }
    let mut sheets = Vec::with_capacity(count);
    for index in 0..count {
        let Some(XmlOut::Element(sheet)) = el.get(txn, index as u32) else {
            return Err(invalid(
                NodeType::Spreadsheet,
                "children",
                "workbook may contain only sheet elements",
            ));
        };
        if sheet.tag().as_ref() != NodeType::SpreadsheetSheet.tag_name() || sheet.len(txn) != 0 {
            return Err(invalid(
                NodeType::Spreadsheet,
                "children",
                "workbook may contain only attribute-only sheet elements",
            ));
        }
        let attrs = read_attrs(txn, &sheet, NodeType::SpreadsheetSheet)?;
        let data = decode_sheet(&attrs)
            .map_err(|error| invalid(NodeType::SpreadsheetSheet, error.field, error.reason))?;
        sheets.push(data);
    }
    validate_workbook(&sheets)
        .map_err(|error| invalid(NodeType::Spreadsheet, error.field, error.reason))?;
    Ok(sheets)
}

pub(crate) struct DisplaySheet {
    pub name: String,
    /// First row contains column headings, followed by display-value rows.
    pub rows: Vec<Vec<String>>,
    pub note: Option<String>,
}

/// Cap materialized export cells and displayed text across the whole workbook.
/// A sparse sheet whose bounding box exceeds the budget becomes an address/value
/// table, preserving distant cells without allocating the empty rectangle.
const MAX_EXPORT_CELLS: usize = 10_000;
const MAX_EXPORT_TEXT_BYTES: usize = 1024 * 1024;

fn bounded_value(value: String, remaining: &mut usize) -> String {
    let mut end = value.len().min(*remaining);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    *remaining -= end;
    value[..end].to_string()
}

pub(crate) fn display_workbook<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
) -> Result<Vec<DisplaySheet>, BlockValidationError> {
    let sheets = read_workbook(txn, el)?;
    let mut remaining_cells = MAX_EXPORT_CELLS;
    let mut remaining_text = MAX_EXPORT_TEXT_BYTES;
    let mut result = Vec::with_capacity(sheets.len());
    for sheet in &sheets {
        let engine = engine_for_sheet(sheet, &sheets);
        let mut occupied: BTreeSet<(usize, usize)> = sheet
            .cells
            .keys()
            .chain(sheet.formats.keys())
            .map(|&(col, row)| (row, col))
            .collect();
        occupied.extend(engine.iter_spill_fill_cells().map(|(col, row)| (row, col)));
        let rows = occupied.iter().map(|(row, _)| row + 1).max().unwrap_or(1);
        let cols = occupied.iter().map(|(_, col)| col + 1).max().unwrap_or(1);
        let rectangle = (rows + 1).saturating_mul(cols + 1);
        let mut output = DisplaySheet {
            name: sheet.name.clone(),
            rows: Vec::new(),
            note: None,
        };
        if rectangle <= remaining_cells && remaining_text > 0 {
            let mut headings = vec![String::new()];
            headings.extend((0..cols).map(col_to_letters));
            output.rows.push(headings);
            for row in 0..rows {
                let mut cells = vec![(row + 1).to_string()];
                cells.extend(
                    (0..cols).map(|col| {
                        bounded_value(engine.get_display((col, row)), &mut remaining_text)
                    }),
                );
                output.rows.push(cells);
            }
            remaining_cells -= rectangle;
        } else if remaining_cells >= 4 && remaining_text > 0 {
            output.note = Some("Sparse sheet shown by cell address.".into());
            output.rows.push(vec!["Cell".into(), "Value".into()]);
            remaining_cells -= 2;
            let count = occupied.len().min(remaining_cells / 2);
            for &(row, col) in occupied.iter().take(count) {
                output.rows.push(vec![
                    format!("{}{}", col_to_letters(col), row + 1),
                    bounded_value(engine.get_display((col, row)), &mut remaining_text),
                ]);
                remaining_cells -= 2;
            }
            if count < occupied.len() {
                output.note = Some("Sparse sheet shown by cell address. Export truncated at the workbook display limit.".into());
            }
        } else {
            output.note = Some("Export truncated at the workbook display limit.".into());
        }
        if remaining_text == 0 {
            output.note = Some("Export truncated at the workbook display limit.".into());
        }
        result.push(output);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_values_are_independent_registers_and_use_shared_validation() {
        let attrs = HashMap::from([
            ("blockId".into(), "sheet1".into()),
            ("sheetName".into(), "Sheet1".into()),
            ("gridRows".into(), "12".into()),
            ("gridCols".into(), "6".into()),
            ("cell:0:0".into(), "12".into()),
            ("cell:0:1".into(), "=A1*2".into()),
            ("format:0:0".into(), "currency".into()),
        ]);
        assert_eq!(
            SPREADSHEET
                .validate_attrs(NodeType::SpreadsheetSheet, &attrs)
                .unwrap(),
            attrs
        );
        let mut bad = attrs;
        bad.insert("cell:1000:0".into(), "outside".into());
        assert!(
            SPREADSHEET
                .validate_attrs(NodeType::SpreadsheetSheet, &bad)
                .is_err()
        );
    }

    #[test]
    fn workbook_version_and_unknown_attributes_are_rejected() {
        let mut attrs = HashMap::from([
            ("blockId".into(), "workbook".into()),
            ("version".into(), "1".into()),
        ]);
        assert!(
            SPREADSHEET
                .validate_attrs(NodeType::Spreadsheet, &attrs)
                .is_ok()
        );
        attrs.insert("cells".into(), "{}".into());
        assert!(
            SPREADSHEET
                .validate_attrs(NodeType::Spreadsheet, &attrs)
                .is_err()
        );
        attrs.remove("cells");
        attrs.insert("version".into(), "2".into());
        assert!(
            SPREADSHEET
                .validate_attrs(NodeType::Spreadsheet, &attrs)
                .is_err()
        );
    }
}
