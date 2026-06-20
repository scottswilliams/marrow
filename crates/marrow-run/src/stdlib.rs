//! Builtins shared by direct calls and `std::` modules.

mod args;
mod assertions;
mod conversion;
mod count;
mod error_constructor;
mod index_lookup;
mod math;
mod output;
mod temporal;

#[cfg(test)]
mod tests;

pub(crate) use args::{
    eval_bytes_arg, eval_date_arg, eval_decimal_arg, eval_duration_arg, eval_instant_arg,
    eval_string_sequence, eval_text,
};
pub(crate) use assertions::eval_assert;
pub(crate) use conversion::{ConversionKind, eval_conversion};
pub(crate) use count::{eval_count, eval_exists};
pub(crate) use error_constructor::eval_error_constructor;
pub(crate) use index_lookup::{
    UniqueIndexLookup, check_key_collection, exact_unique_index_lookup_value,
    read_exact_unique_index_lookup_if_present, read_exact_unique_index_lookup_value,
    read_unique_index_identity, unique_index_lookup,
};
pub(crate) use math::{int_modulo, int_remainder};
pub(crate) use output::eval_output;
pub(crate) use temporal::{parse_iso8601_duration_nanos, parse_rfc3339_instant_nanos};
