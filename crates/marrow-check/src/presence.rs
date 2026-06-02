mod calls;
mod direct;
mod effects;
mod keys;
mod proofs;
mod scope;
mod target;
mod util;
mod walk;
mod writes;

pub(crate) use direct::direct_effects_for_block;
pub(crate) use target::read_target;

pub(crate) fn check_presence(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    walk::check_presence(program, diagnostics);
}
