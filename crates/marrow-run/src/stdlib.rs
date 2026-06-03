//! Builtins shared by direct calls and `std::` modules.

mod args;
mod assertions;
mod conversion;
mod count;
mod error_constructor;
mod index_lookup;
mod math;
mod output;

#[cfg(test)]
mod tests;

pub(crate) use args::{
    eval_bytes_arg, eval_date_arg, eval_decimal_arg, eval_duration_arg, eval_instant_arg, eval_text,
};
pub(crate) use assertions::eval_assert;
pub(crate) use conversion::{eval_bytes_conversion, eval_conversion};
pub(crate) use count::{eval_count, eval_exists};
pub(crate) use error_constructor::eval_error_constructor;
pub(crate) use index_lookup::{
    check_key_collection, is_iterable_index_branch, unique_index_lookup, unique_index_lookup_values,
};
pub(crate) use math::{int_modulo, int_remainder};
pub(crate) use output::eval_output;
