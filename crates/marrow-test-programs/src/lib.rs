//! Compiled-program fixtures: a project captured through the production owner and built
//! into image bytes or a verified image. Reached only through `[dev-dependencies]`, by the
//! crates whose tests need a compiled program; every other test builds on
//! `marrow-test-support`, which carries none of the compiler.

pub mod program;
