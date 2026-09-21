//! The command-line surface: project scaffolding, check, test, image, doctor, argument
//! and resource limits, published ids, and the Cargo-graph boundary.

pub mod common;

#[path = "cli/argument_bound.rs"]
mod argument_bound;
#[path = "cli/check_command.rs"]
mod check_command;
#[path = "cli/cli_project.rs"]
mod cli_project;
#[path = "cli/doctor_command.rs"]
mod doctor_command;
#[path = "cli/ids_publication.rs"]
mod ids_publication;
#[path = "cli/image_command.rs"]
mod image_command;
#[path = "cli/lsp_not_in_cli_graph.rs"]
mod lsp_not_in_cli_graph;
#[path = "cli/resource_limit_cli.rs"]
mod resource_limit_cli;
#[path = "cli/run_command.rs"]
mod run_command;
#[path = "cli/shared_product_cli.rs"]
mod shared_product_cli;
#[path = "cli/test_command.rs"]
mod test_command;
