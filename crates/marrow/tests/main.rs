mod support;
mod support_data;
mod support_evolve;
mod support_surface;

#[path = "cases/backup_cli.rs"]
mod backup_cli;
#[path = "cases/broken_pipe_cli.rs"]
mod broken_pipe_cli;
#[path = "cases/check_cli.rs"]
mod check_cli;
#[path = "cases/check_client_cli.rs"]
mod check_client_cli;
#[path = "cases/check_footprints_cli.rs"]
mod check_footprints_cli;
#[path = "cases/check_project_cli.rs"]
mod check_project_cli;
#[path = "cases/check_read_only_cli.rs"]
mod check_read_only_cli;
#[path = "cases/conformance_corpus.rs"]
mod conformance_corpus;
#[path = "cases/data_cli_composite.rs"]
mod data_cli_composite;
#[path = "cases/data_cli_flags.rs"]
mod data_cli_flags;
#[path = "cases/data_cli_get.rs"]
mod data_cli_get;
#[path = "cases/data_cli_integrity.rs"]
mod data_cli_integrity;
#[path = "cases/data_cli_inventory.rs"]
mod data_cli_inventory;
#[path = "cases/data_cli_value_rendering.rs"]
mod data_cli_value_rendering;
#[path = "cases/data_orphan_cli.rs"]
mod data_orphan_cli;
#[path = "cases/doctor_cli.rs"]
mod doctor_cli;
#[path = "cases/dry_run_cli.rs"]
mod dry_run_cli;
#[path = "cases/evolve_cli_atomic_publish.rs"]
mod evolve_cli_atomic_publish;
#[path = "cases/evolve_cli_client_sync.rs"]
mod evolve_cli_client_sync;
#[path = "cases/evolve_cli_default_backfill.rs"]
mod evolve_cli_default_backfill;
#[path = "cases/evolve_cli_fresh_checkout_seed.rs"]
mod evolve_cli_fresh_checkout_seed;
#[path = "cases/evolve_cli_preview.rs"]
mod evolve_cli_preview;
#[path = "cases/evolve_cli_retire_rename.rs"]
mod evolve_cli_retire_rename;
#[path = "cases/evolve_cli_store_behind_lock.rs"]
mod evolve_cli_store_behind_lock;
#[path = "cases/evolve_unhappy_path_guide.rs"]
mod evolve_unhappy_path_guide;
#[path = "cases/fmt_cli.rs"]
mod fmt_cli;
#[path = "cases/format_matrix_cli.rs"]
mod format_matrix_cli;
#[path = "cases/init_cli.rs"]
mod init_cli;
#[path = "cases/nesting_limit_cli.rs"]
mod nesting_limit_cli;
#[path = "cases/run_auto_apply.rs"]
mod run_auto_apply;
#[path = "cases/run_cli_catalog.rs"]
mod run_cli_catalog;
#[path = "cases/run_cli_entry.rs"]
mod run_cli_entry;
#[path = "cases/run_cli_enum.rs"]
mod run_cli_enum;
#[path = "cases/run_cli_exec.rs"]
mod run_cli_exec;
#[path = "cases/run_cli_faults.rs"]
mod run_cli_faults;
#[path = "cases/run_cli_fence.rs"]
mod run_cli_fence;
#[path = "cases/run_cli_maintenance.rs"]
mod run_cli_maintenance;
#[path = "cases/scenario_evolve_write_run_cli.rs"]
mod scenario_evolve_write_run_cli;
#[path = "cases/store_open_robustness_cli.rs"]
mod store_open_robustness_cli;
#[path = "cases/surface_client_cli.rs"]
mod surface_client_cli;
#[path = "cases/surface_serve_cli.rs"]
mod surface_serve_cli;
#[path = "cases/test_cli.rs"]
mod test_cli;
#[path = "cases/tidy_prose_assertions.rs"]
mod tidy_prose_assertions;
#[path = "cases/trace_cli.rs"]
mod trace_cli;
#[path = "cases/usage_cli.rs"]
mod usage_cli;
#[path = "cases/v01_cli.rs"]
mod v01_cli;
