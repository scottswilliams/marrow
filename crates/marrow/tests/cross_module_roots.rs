//! The project-wide durable-root pin: a durable root is visible to every module in
//! the project, not only the module that declares it.
//!
//! Module `bank` declares `store ^accounts` and `teller` both reads and writes it.
//! The E07 M1 flagship read this law as broken and was forced single-module; the M2
//! evidence check found the law already holds (one flat root gather, a single durable
//! registry, no module gate on root resolution) and that the flagship symptom was a
//! separate identity-admission diagnostic confound. This pin makes the law a
//! regression target directly, through the production capture -> compile -> verify ->
//! run path and through `marrow check`, so the confound cannot mislead again.

mod common;

use common::Project;
use marrow_vm::Value;

fn int(n: i64) -> Value {
    Value::Int(n)
}

/// The present arm of an optional (`int?`) return.
fn some_int(n: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(n)))))
}

/// A cross-module durable root round trips: `teller` (which does not declare the
/// root) writes `^accounts` through a read-modify-write, a later `teller` read
/// observes the committed value, and `bank` (the declaring module) reads back exactly
/// what `teller` wrote. Every effect crosses the module boundary against one durable
/// root, on the persistent ephemeral attachment.
#[test]
fn a_durable_root_is_read_and_written_across_modules() {
    let mut session = Project::from_fixture("cross_module_roots").session();

    // The declaring module opens the account at zero (Unit return).
    assert_eq!(session.call("openAccount", vec![int(1)]), None);

    // A module other than the declarer reads-modifies-writes the same root, twice.
    assert_eq!(
        session.call("deposit", vec![int(1), int(100)]),
        Some(int(100)),
        "teller writes ^accounts, a root it does not declare",
    );
    assert_eq!(
        session.call("deposit", vec![int(1), int(50)]),
        Some(int(150)),
        "teller's second deposit observes its own committed write",
    );

    // The non-declaring module reads the accumulated balance back.
    assert_eq!(
        session.call("balanceOf", vec![int(1)]),
        Some(int(150)),
        "teller reads the cross-module root it does not declare",
    );

    // The declaring module reads back exactly what the other module wrote.
    assert_eq!(
        session.call("ownerBalance", vec![int(1)]),
        some_int(150),
        "bank observes through its own root the value teller committed",
    );

    // An untouched key is absent from the declaring module's view.
    assert_eq!(
        session.call("ownerBalance", vec![int(2)]),
        Some(Value::Optional(None)),
        "an unopened account reads absent, not a default",
    );
}

/// `marrow check` describes each export's durable access demand in source spelling and
/// exits 0. Both modules appear: `teller`'s read and write of `^accounts` — a root it
/// does not declare — is the project-wide-roots law made visible in the demand report.
/// The bytes are frozen so a regression that regionalizes root visibility is
/// conspicuous.
#[test]
fn check_reports_cross_module_root_demand() {
    let output =
        Project::from_fixture("cross_module_roots").run_cli("cross-module-check", &["check"]);
    assert!(
        output.status.success(),
        "check must succeed on the clean cross-module project: {}",
        output.stderr_text(),
    );
    assert_eq!(output.stdout_text(), CROSS_MODULE_DEMAND_REPORT);
}

/// The frozen per-export demand report, one line per export in `module.item` order.
/// `teller.deposit` reading and writing `^accounts.balance` — and `teller.balanceOf`
/// reading it — pins that a root declared in `bank` is demandable from `teller`.
const CROSS_MODULE_DEMAND_REPORT: &str = "\
bank.openAccount reads ^accounts; writes ^accounts
bank.ownerBalance reads ^accounts.balance
teller.balanceOf reads ^accounts.balance
teller.deposit reads ^accounts.balance; writes ^accounts.balance
";
