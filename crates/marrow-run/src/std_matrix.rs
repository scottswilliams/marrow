use marrow_check::CheckedArg as ExecArg;
use marrow_store::{Decimal, DecimalParseError};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, decimal_overflow, std_arity, type_error};
use crate::expr::eval_int;
use crate::stdlib::eval_text;
use crate::value::Value;

const MAX_BYTES: usize = 1_048_576;
const MAX_DIMENSION: usize = 64;
const MAX_CELLS: usize = 4_096;
const MAX_OPS: usize = 100_000;

pub(crate) fn eval_matrix(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "parse" => {
            let [text] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let matrix = ParsedMatrix::parse(&eval_text(text, env, span)?, span)?;
            Ok(Value::Str(matrix.render()))
        }
        "identity" => {
            let [size] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let size = dimension(eval_int(&size.value, env)?, span)?;
            let mut rows = vec![vec![Decimal::ZERO; size]; size];
            for (index, row) in rows.iter_mut().enumerate() {
                row[index] = Decimal::ONE;
            }
            Ok(Value::Str(ParsedMatrix { rows }.render()))
        }
        "rows" | "cols" => {
            let [text] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let matrix = ParsedMatrix::parse(&eval_text(text, env, span)?, span)?;
            let value = if op == "rows" {
                matrix.row_count()
            } else {
                matrix.col_count()
            };
            Ok(Value::Int(value as i64))
        }
        "get" => {
            let [text, row, col] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let matrix = ParsedMatrix::parse(&eval_text(text, env, span)?, span)?;
            let row = index(eval_int(&row.value, env)?, span)?;
            let col = index(eval_int(&col.value, env)?, span)?;
            let Some(value) = matrix.rows.get(row).and_then(|row| row.get(col)) else {
                return Err(type_error("matrix index is out of bounds", span));
            };
            Ok(Value::Decimal(*value))
        }
        "add" | "multiply" => {
            let [a, b] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let a = ParsedMatrix::parse(&eval_text(a, env, span)?, span)?;
            let b = ParsedMatrix::parse(&eval_text(b, env, span)?, span)?;
            let result = if op == "add" {
                a.add(&b, span)?
            } else {
                a.multiply(&b, span)?
            };
            Ok(Value::Str(result.render()))
        }
        "transpose" => {
            let [text] = args else {
                return Err(std_arity("matrix", op, span));
            };
            let matrix = ParsedMatrix::parse(&eval_text(text, env, span)?, span)?;
            Ok(Value::Str(matrix.transpose().render()))
        }
        _ => Err(crate::error::unsupported(
            &format!("std::matrix::{op}"),
            span,
        )),
    }
}

#[derive(Clone)]
struct ParsedMatrix {
    rows: Vec<Vec<Decimal>>,
}

impl ParsedMatrix {
    fn parse(text: &str, span: SourceSpan) -> Result<Self, RuntimeError> {
        if text.len() > MAX_BYTES {
            return Err(type_error("matrix text is too large", span));
        }
        let trimmed = text.trim();
        let Some(inner) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            return Err(type_error("matrix text must be bracketed", span));
        };
        if inner.trim().is_empty() {
            return Err(type_error("matrix must have at least one row", span));
        }
        let mut rows = Vec::new();
        let mut width = None;
        for row_text in inner.split(';') {
            if rows.len() == MAX_DIMENSION {
                return Err(type_error("matrix dimensions are too large", span));
            }
            let row = parse_row(row_text, span)?;
            match width {
                Some(width) if row.len() != width => {
                    return Err(type_error("matrix rows must be rectangular", span));
                }
                Some(_) => {}
                None => width = Some(row.len()),
            }
            check_shape(rows.len() + 1, row.len(), span)?;
            rows.push(row);
        }
        Ok(Self { rows })
    }

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn col_count(&self) -> usize {
        self.rows[0].len()
    }

    fn render(&self) -> String {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| value.to_text())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(";");
        format!("[{rows}]")
    }

    fn add(&self, other: &ParsedMatrix, span: SourceSpan) -> Result<ParsedMatrix, RuntimeError> {
        if self.row_count() != other.row_count() || self.col_count() != other.col_count() {
            return Err(type_error("matrix add requires matching dimensions", span));
        }
        let mut rows = Vec::with_capacity(self.row_count());
        for (left, right) in self.rows.iter().zip(&other.rows) {
            let mut row = Vec::with_capacity(left.len());
            for (a, b) in left.iter().zip(right) {
                row.push(a.checked_add(*b).ok_or_else(|| decimal_overflow(span))?);
            }
            rows.push(row);
        }
        Ok(ParsedMatrix { rows })
    }

    fn multiply(
        &self,
        other: &ParsedMatrix,
        span: SourceSpan,
    ) -> Result<ParsedMatrix, RuntimeError> {
        if self.col_count() != other.row_count() {
            return Err(type_error(
                "matrix multiply requires compatible dimensions",
                span,
            ));
        }
        let ops = self
            .row_count()
            .checked_mul(other.col_count())
            .and_then(|cells| cells.checked_mul(self.col_count()))
            .ok_or_else(|| type_error("matrix operation is too large", span))?;
        if ops > MAX_OPS {
            return Err(type_error("matrix operation is too large", span));
        }
        let mut rows = Vec::with_capacity(self.row_count());
        for row_index in 0..self.row_count() {
            let mut row = Vec::with_capacity(other.col_count());
            for col_index in 0..other.col_count() {
                let mut total = Decimal::ZERO;
                for offset in 0..self.col_count() {
                    let product = self.rows[row_index][offset]
                        .checked_mul(other.rows[offset][col_index])
                        .ok_or_else(|| decimal_overflow(span))?;
                    total = total
                        .checked_add(product)
                        .ok_or_else(|| decimal_overflow(span))?;
                }
                row.push(total);
            }
            rows.push(row);
        }
        Ok(ParsedMatrix { rows })
    }

    fn transpose(&self) -> ParsedMatrix {
        let mut rows = vec![Vec::with_capacity(self.row_count()); self.col_count()];
        for row in &self.rows {
            for (index, value) in row.iter().enumerate() {
                rows[index].push(*value);
            }
        }
        ParsedMatrix { rows }
    }
}

fn parse_row(row: &str, span: SourceSpan) -> Result<Vec<Decimal>, RuntimeError> {
    if row.trim().is_empty() {
        return Err(type_error("matrix rows must not be empty", span));
    }
    let mut values = Vec::new();
    for cell in row.split(',') {
        if values.len() == MAX_DIMENSION {
            return Err(type_error("matrix dimensions are too large", span));
        }
        values.push(parse_decimal(cell.trim(), span)?);
    }
    Ok(values)
}

fn parse_decimal(text: &str, span: SourceSpan) -> Result<Decimal, RuntimeError> {
    match Decimal::parse_relaxed(text) {
        Ok(decimal) => Ok(decimal),
        Err(DecimalParseError::Overflow) => Err(decimal_overflow(span)),
        Err(DecimalParseError::Malformed) => Err(type_error("matrix cell is not a decimal", span)),
    }
}

fn dimension(value: i64, span: SourceSpan) -> Result<usize, RuntimeError> {
    if value < 1 {
        return Err(type_error("matrix size must be positive", span));
    }
    let value =
        usize::try_from(value).map_err(|_| type_error("matrix dimensions are too large", span))?;
    check_shape(value, value, span)?;
    Ok(value)
}

fn index(value: i64, span: SourceSpan) -> Result<usize, RuntimeError> {
    usize::try_from(value).map_err(|_| type_error("matrix index must be non-negative", span))
}

fn check_shape(rows: usize, cols: usize, span: SourceSpan) -> Result<(), RuntimeError> {
    if rows > MAX_DIMENSION || cols > MAX_DIMENSION || rows.saturating_mul(cols) > MAX_CELLS {
        return Err(type_error("matrix dimensions are too large", span));
    }
    Ok(())
}
