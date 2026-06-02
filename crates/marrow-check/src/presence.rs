mod calls;
mod effects;
mod keys;
mod scope;
mod target;
mod util;
mod walk;

pub(crate) use calls::append_call_args;
pub(crate) use keys::{SavedPathParts, saved_path_parts};
pub(crate) use scope::NameScope;
pub(crate) use target::read_target;

pub(crate) fn check_presence(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    walk::check_presence(program, diagnostics);
}
