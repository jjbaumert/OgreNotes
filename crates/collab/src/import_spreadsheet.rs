// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Spreadsheet import: XLSX and CSV → yrs document.
//!
//! Counterpart to the spreadsheet export paths in [`crate::export`]. Each
//! importer builds the same Table → TableRow → TableCell → Paragraph → text
//! node structure the editor schema expects. Kept alongside the other import
//! modules (`import`, `import_docx`, `import_pdf`) rather than in `export` so
//! that all import formats live in one predictable place.

use yrs::{Doc, Transact};

use crate::schema::NodeType;
#[cfg(feature = "xlsx")]
use crate::export::ATTR_SHEET_NAME;
#[cfg(feature = "xlsx")]
use yrs::Xml;

/// Import an XLSX file into a yrs document.
/// Creates one Table node per worksheet with TableRow/TableCell children.
#[cfg(feature = "xlsx")]
pub fn from_xlsx(bytes: &[u8]) -> Result<Doc, String> {
    use calamine::{Reader, Xlsx, Data};
    use std::io::Cursor;
    use yrs::{WriteTxn, types::xml::{XmlElementPrelim, XmlTextPrelim, XmlFragment}};

    let cursor = Cursor::new(bytes);
    let mut workbook: Xlsx<_> = Xlsx::new(cursor)
        .map_err(|e| format!("Failed to open XLSX: {e}"))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();

    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        for sheet_name in &sheet_names {
            let Ok(range) = workbook.worksheet_range(sheet_name) else { continue };

            let pos = fragment.len(&txn);
            let table = fragment.insert(
                &mut txn,
                pos,
                XmlElementPrelim::empty(NodeType::Table.tag_name()),
            );
            table.insert_attribute(&mut txn, ATTR_SHEET_NAME, sheet_name.clone());

            for row_data in range.rows() {
                let row_pos = table.len(&txn);
                let row = table.insert(
                    &mut txn,
                    row_pos,
                    XmlElementPrelim::empty(NodeType::TableRow.tag_name()),
                );
                for cell_data in row_data {
                    let cell_pos = row.len(&txn);
                    let cell = row.insert(
                        &mut txn,
                        cell_pos,
                        XmlElementPrelim::empty(NodeType::TableCell.tag_name()),
                    );
                    let text = match cell_data {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => f.to_string(),
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => if *b { "TRUE".to_string() } else { "FALSE".to_string() },
                        Data::Error(e) => format!("{e:?}"),
                        _ => String::new(),
                    };
                    if !text.is_empty() {
                        let p = cell.insert(
                            &mut txn,
                            0,
                            XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
                        );
                        p.insert(&mut txn, 0, XmlTextPrelim::new(&text));
                    }
                }
            }
        }
    }

    Ok(doc)
}

/// Import a CSV string into a yrs document.
/// Creates one Table node with TableRow/TableCell children.
/// Handles RFC 4180 quoting: fields containing commas, newlines, or quotes
/// are wrapped in double quotes, with internal quotes escaped as "".
pub fn from_csv(csv_text: &str) -> Doc {
    use yrs::{WriteTxn, types::xml::{XmlElementPrelim, XmlTextPrelim, XmlFragment}};

    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        let table = fragment.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::Table.tag_name()),
        );

        for record in split_csv_records(csv_text) {
            if record.trim().is_empty() { continue; }
            let fields = parse_csv_line(&record);
            let row_pos = table.len(&txn);
            let row = table.insert(
                &mut txn,
                row_pos,
                XmlElementPrelim::empty(NodeType::TableRow.tag_name()),
            );
            for val in &fields {
                let cell_pos = row.len(&txn);
                let cell = row.insert(
                    &mut txn,
                    cell_pos,
                    XmlElementPrelim::empty(NodeType::TableCell.tag_name()),
                );
                if !val.is_empty() {
                    let p = cell.insert(
                        &mut txn,
                        0,
                        XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
                    );
                    p.insert(&mut txn, 0, XmlTextPrelim::new(val));
                }
            }
        }
    }

    doc
}

/// Split CSV text into logical records, treating a newline inside a quoted
/// field as part of the field rather than a record boundary (RFC 4180 §2).
/// This must run before field parsing: splitting on raw `\n` (as `str::lines`
/// does) would tear a quoted multi-line field across two records.
///
/// Quote state toggles on each `"`. An escaped `""` toggles twice (net no
/// change), and since the two quotes are adjacent no newline can fall between
/// them, so a plain toggle is sufficient for boundary detection — the
/// field-level handling of `""` and trimming stays in `parse_csv_line`.
/// `\r\n` and `\n` are both record terminators; a lone `\r` is treated as
/// ordinary content, matching `str::lines`.
fn split_csv_records(text: &str) -> Vec<String> {
    let mut records = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push('"');
            }
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    records.push(std::mem::take(&mut current));
                } else {
                    current.push('\r');
                }
            }
            '\n' if !in_quotes => {
                records.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    // Trailing record with no terminating newline.
    if !current.is_empty() {
        records.push(current);
    }
    records
}

/// Parse a single CSV record respecting RFC 4180 quoting.
/// Fields wrapped in double quotes can contain commas, embedded newlines, and
/// escaped quotes ("").
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if in_quotes {
            if chars[i] == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    // Escaped quote ""
                    current.push('"');
                    i += 2;
                } else {
                    // End of quoted field
                    in_quotes = false;
                    i += 1;
                }
            } else {
                current.push(chars[i]);
                i += 1;
            }
        } else if chars[i] == '"' {
            in_quotes = true;
            i += 1;
        } else if chars[i] == ',' {
            fields.push(current.trim().to_string());
            current = String::new();
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }
    fields.push(current.trim().to_string());
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::to_csv;

    // ── Import tests ───────────────────────────────────────────

    #[test]
    fn csv_import_roundtrip() {
        let csv_text = "A,B,C\n1,2,3\n4,5,6";
        let doc = from_csv(csv_text);

        // Export back to CSV and verify content
        let exported = to_csv(&doc);
        assert!(exported.contains("A,B,C"), "got: {exported}");
        assert!(exported.contains("1,2,3"), "got: {exported}");
        assert!(exported.contains("4,5,6"), "got: {exported}");
    }

    #[test]
    fn csv_import_empty() {
        let doc = from_csv("");
        let exported = to_csv(&doc);
        assert!(exported.is_empty() || exported.trim().is_empty());
    }

    #[test]
    fn csv_import_quoted_fields() {
        // RFC 4180: fields with commas inside quotes
        let csv_text = "\"Smith, John\",42,\"New York, NY\"";
        let doc = from_csv(csv_text);
        let exported = to_csv(&doc);
        // Should have 3 fields, not 5
        let line = exported.lines().next().unwrap();
        // Count actual values (commas inside quotes are preserved)
        assert!(exported.contains("Smith, John"), "got: {exported}");
        assert!(exported.contains("42"), "got: {exported}");
        assert!(exported.contains("New York, NY"), "got: {exported}");
    }

    #[test]
    fn csv_import_escaped_quotes() {
        // RFC 4180: escaped quotes ("" inside quoted field)
        let csv_text = "\"say \"\"hello\"\"\",world";
        let doc = from_csv(csv_text);
        let exported = to_csv(&doc);
        // The exported CSV re-escapes quotes: say "hello" → "say ""hello"""
        assert!(exported.contains("say"), "got: {exported}");
        assert!(exported.contains("hello"), "got: {exported}");
        assert!(exported.contains("world"), "got: {exported}");
    }

    #[test]
    fn csv_import_quoted_field_with_embedded_newline() {
        // RFC 4180 §2: a newline inside a quoted field is part of the field,
        // not a record boundary. Regression: from_csv used to split on raw
        // `\n` before quote context, tearing this data row into two rows.
        use yrs::{ReadTxn, Transact, types::xml::{XmlFragment, XmlOut}};

        let csv_text = "name,note\n\"Smith\",\"line1\nline2\"\n";
        let doc = from_csv(csv_text);

        // The table must have exactly two rows (header + one data row), not
        // three — the embedded newline must not start a new row.
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").expect("content fragment");
        let table = (0..fragment.len(&txn))
            .filter_map(|i| match fragment.get(&txn, i) {
                Some(XmlOut::Element(el)) => Some(el),
                _ => None,
            })
            .find(|el| el.tag().as_ref() == "table")
            .expect("table element");
        let row_count = (0..table.len(&txn))
            .filter_map(|i| match table.get(&txn, i) {
                Some(XmlOut::Element(el)) => Some(el),
                _ => None,
            })
            .filter(|el| el.tag().as_ref() == "table_row")
            .count();
        assert_eq!(
            row_count, 2,
            "an embedded newline in a quoted field must not split the data row"
        );
        drop(txn);

        // And the field value retains the embedded newline through a
        // re-export.
        let exported = to_csv(&doc);
        assert!(
            exported.contains("line1\nline2"),
            "embedded newline must survive in the cell value, got: {exported:?}"
        );
    }

    #[test]
    fn csv_parse_line_simple() {
        let fields = parse_csv_line("a,b,c");
        assert_eq!(fields, vec!["a", "b", "c"]);
    }

    #[test]
    fn csv_parse_line_quoted_comma() {
        let fields = parse_csv_line("\"a,b\",c");
        assert_eq!(fields, vec!["a,b", "c"]);
    }

    #[test]
    fn csv_parse_line_escaped_quote() {
        let fields = parse_csv_line("\"a\"\"b\",c");
        assert_eq!(fields, vec!["a\"b", "c"]);
    }
}
