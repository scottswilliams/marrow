use std::collections::{HashMap, HashSet};

use marrow_check::CheckedArg as ExecArg;
use marrow_store::value::{SavedValue, ScalarType, decode_value};
use marrow_store::{Decimal, DecimalParseError};
use marrow_syntax::SourceSpan;

use crate::collection::absent_read;
use crate::env::Env;
use crate::error::{RuntimeError, std_arity, type_error};
use crate::expr::eval_int;
use crate::stdlib::{eval_string_sequence, eval_text};
use crate::value::Value;

const MAX_BYTES: usize = 1_048_576;
const MAX_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 256;
const MAX_CELL_BYTES: usize = 65_536;

#[derive(Clone, Copy)]
enum CsvScalarOp {
    String,
    Int,
    Decimal,
    Bool,
}

impl CsvScalarOp {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "int" => Some(Self::Int),
            "decimal" => Some(Self::Decimal),
            "bool" => Some(Self::Bool),
            _ => None,
        }
    }
}

pub(crate) fn eval_csv(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "row" => {
            let [cells] = args else {
                return Err(std_arity("csv", op, span));
            };
            Ok(Value::Str(format_row(&eval_string_sequence(
                cells, env, span,
            )?)))
        }
        "rowCount" => {
            let [text] = args else {
                return Err(std_arity("csv", op, span));
            };
            let table = CsvTable::parse(&eval_text(text, env, span)?, span)?;
            Ok(Value::Int(table.rows.len() as i64))
        }
        "hasColumn" => {
            let [text, column] = args else {
                return Err(std_arity("csv", op, span));
            };
            let table = CsvTable::parse(&eval_text(text, env, span)?, span)?;
            let column = eval_text(column, env, span)?;
            Ok(Value::Bool(table.columns.contains_key(&column)))
        }
        _ => {
            let Some(cell_op) = CsvScalarOp::from_name(op) else {
                return Err(crate::error::unsupported(&format!("std::csv::{op}"), span));
            };
            let [text, row, column] = args else {
                return Err(std_arity("csv", op, span));
            };
            let table = CsvTable::parse(&eval_text(text, env, span)?, span)?;
            let row = row_index(row, env, span)?;
            let column = eval_text(column, env, span)?;
            let Some(cell) = table.cell(row, &column) else {
                return Err(absent_read("CSV cell is absent".into(), span));
            };
            if cell.is_empty() {
                return Err(absent_read("CSV cell is empty".into(), span));
            }
            parse_cell(cell_op, cell, span)
        }
    }
}

struct CsvTable {
    columns: HashMap<String, usize>,
    rows: Vec<Vec<String>>,
}

impl CsvTable {
    fn parse(text: &str, span: SourceSpan) -> Result<Self, RuntimeError> {
        if text.is_empty() || text.len() > MAX_BYTES {
            return Err(type_error("CSV text is empty or too large", span));
        }
        let records = parse_records(text, span)?;
        let Some((header, rows)) = records.split_first() else {
            return Err(type_error("CSV requires a header row", span));
        };
        if header.is_empty() || header.len() > MAX_COLUMNS {
            return Err(type_error("CSV header has an invalid column count", span));
        }
        let mut seen = HashSet::new();
        let mut columns = HashMap::new();
        for (index, name) in header.iter().enumerate() {
            if name.is_empty() || !seen.insert(name.clone()) {
                return Err(type_error("CSV headers must be non-empty and unique", span));
            }
            columns.insert(name.clone(), index);
        }
        if rows.len() > MAX_ROWS {
            return Err(type_error("CSV has too many rows", span));
        }
        for row in rows {
            if row.len() != header.len() {
                return Err(type_error("CSV rows must match the header width", span));
            }
        }
        Ok(Self {
            columns,
            rows: rows.to_vec(),
        })
    }

    fn cell(&self, row: usize, column: &str) -> Option<&str> {
        let column = *self.columns.get(column)?;
        self.rows.get(row)?.get(column).map(String::as_str)
    }
}

fn parse_records(text: &str, span: SourceSpan) -> Result<Vec<Vec<String>>, RuntimeError> {
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    let mut quoted = false;
    let mut field_start = true;

    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                _ => field.push(ch),
            }
            continue;
        }
        match ch {
            '"' if field_start => {
                in_quotes = true;
                quoted = true;
                field_start = false;
            }
            '"' => return Err(type_error("CSV quote appears outside a quoted field", span)),
            ',' => finish_field(&mut row, &mut field, &mut quoted, &mut field_start, span)?,
            '\n' => {
                finish_field(&mut row, &mut field, &mut quoted, &mut field_start, span)?;
                records.push(std::mem::take(&mut row));
            }
            '\r' => {
                if chars.next() != Some('\n') {
                    return Err(type_error("CSV uses CR without LF", span));
                }
                finish_field(&mut row, &mut field, &mut quoted, &mut field_start, span)?;
                records.push(std::mem::take(&mut row));
            }
            _ => {
                if quoted {
                    return Err(type_error("CSV quoted field has trailing text", span));
                }
                field.push(ch);
                field_start = false;
            }
        }
    }
    if in_quotes {
        return Err(type_error("CSV quoted field is unterminated", span));
    }
    if !field_start || quoted || !row.is_empty() {
        finish_field(&mut row, &mut field, &mut quoted, &mut field_start, span)?;
        records.push(row);
    }
    Ok(records)
}

fn finish_field(
    row: &mut Vec<String>,
    field: &mut String,
    quoted: &mut bool,
    field_start: &mut bool,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if row.len() >= MAX_COLUMNS || field.len() > MAX_CELL_BYTES {
        return Err(type_error("CSV row or cell is too large", span));
    }
    row.push(std::mem::take(field));
    *quoted = false;
    *field_start = true;
    Ok(())
}

/// Render one RFC 4180 record that the reader parses back to the original cells: a
/// cell is quoted only when it contains a comma, quote, CR, or LF, and internal
/// quotes double. No trailing newline is emitted.
fn format_row(cells: &[String]) -> String {
    cells
        .iter()
        .map(|cell| quote_cell(cell))
        .collect::<Vec<_>>()
        .join(",")
}

fn quote_cell(cell: &str) -> String {
    if cell.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_owned()
    }
}

fn row_index(arg: &ExecArg, env: &mut Env<'_>, span: SourceSpan) -> Result<usize, RuntimeError> {
    usize::try_from(eval_int(&arg.value, env)?)
        .map_err(|_| type_error("CSV row index must be non-negative", span))
}

fn parse_cell(op: CsvScalarOp, cell: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    match op {
        CsvScalarOp::String => Ok(Value::Str(cell.to_string())),
        // An integer cell must be the one canonical spelling, the same rule the
        // `int()` builtin and `std::json::int` enforce: `-0`, leading zeros, and a
        // `+` sign are rejected rather than silently normalized.
        CsvScalarOp::Int => match decode_value(cell.as_bytes(), ScalarType::Int) {
            Some(SavedValue::Int(value)) => Ok(Value::Int(value)),
            _ => Err(type_error("CSV cell is not an int", span)),
        },
        // A CSV cell is external data, so a non-canonical spelling such as `9.50`
        // canonicalizes to its one stored value rather than being rejected as a
        // Marrow source literal would be.
        // A value outside the decimal scalar envelope is a scalar-reader fence, like
        // the integer reader rejecting a cell past the int envelope, not the
        // decimal-arithmetic overflow fault.
        CsvScalarOp::Decimal => match Decimal::parse_relaxed(cell) {
            Ok(decimal) => Ok(Value::Decimal(decimal)),
            Err(DecimalParseError::Overflow) => {
                Err(type_error("CSV cell is outside the decimal envelope", span))
            }
            Err(DecimalParseError::Malformed) => Err(type_error("CSV cell is not a decimal", span)),
        },
        CsvScalarOp::Bool => match cell {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(type_error("CSV cell is not a bool", span)),
        },
    }
}
