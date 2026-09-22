//! What a reported diagnostic is: the one bounded collector and its ceilings, the file
//! identity and full byte span every row keeps, the ordered artifact a corpus reports,
//! and the rule that a source-level problem is a typed diagnostic rather than an abort.

#[path = "common/ids.rs"]
mod ids;
use marrow_test_support::project as project_capture;

#[path = "diagnostics/collector.rs"]
mod collector;
#[path = "diagnostics/identity_golden.rs"]
mod identity_golden;
#[path = "diagnostics/lowering_not_panics.rs"]
mod lowering_not_panics;
