//! Known-answer evidence that the integer-bound value built-ins reach the VM as the
//! exact domain edges: `maxInt` runs to `i64::MAX` and `minInt` to `i64::MIN` through
//! the whole production path (capture -> compile -> verify -> VM), including when a
//! bound is folded into a module constant. The image legitimately carries the constant;
//! no *source* spells the literal.

use marrow_verify::VerifiedImage;
use marrow_vm::{Value, run};

use crate::common::Project;

const SOURCE: &str = r#"module main

const CAP = maxInt

pub fn hi(): int {
    return maxInt
}

pub fn lo(): int {
    return minInt
}

pub fn cap(): int {
    return CAP
}

pub fn floorMinusOneOverflows(): int {
    return minInt - 1
}
"#;

fn run_named(image: &VerifiedImage, name: &str) -> Result<Option<Value>, String> {
    let function = image
        .exports()
        .iter()
        .map(|export| {
            image
                .function(export.function())
                .expect("verified function")
        })
        .find(|function| function.body().name() == name)
        .unwrap_or_else(|| panic!("export `{name}` present"));
    run(function, Vec::new()).map_err(|fault| fault.code().as_str().to_string())
}

#[test]
fn the_bounds_run_to_the_int_domain_edges() {
    let image = Project::single(SOURCE).image();
    assert_eq!(run_named(&image, "hi"), Ok(Some(Value::Int(i64::MAX))));
    assert_eq!(run_named(&image, "lo"), Ok(Some(Value::Int(i64::MIN))));
}

#[test]
fn a_bound_folded_into_a_constant_runs_to_its_value() {
    let image = Project::single(SOURCE).image();
    assert_eq!(run_named(&image, "cap"), Ok(Some(Value::Int(i64::MAX))));
}

#[test]
fn a_bound_is_an_ordinary_int_at_runtime() {
    // `minInt - 1` is exactly the checked-arithmetic underflow, so the bound behaves as
    // the ordinary i64 it is, not a sentinel with special arithmetic.
    let image = Project::single(SOURCE).image();
    assert_eq!(
        run_named(&image, "floorMinusOneOverflows"),
        Err("run.overflow".to_string())
    );
}
