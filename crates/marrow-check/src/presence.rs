mod calls;
mod effects;
mod keys;
mod scope;
mod target;
mod util;
mod walk;

pub(crate) use target::read_target;

pub(crate) fn check_presence(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    walk::check_presence(program, diagnostics);
}
