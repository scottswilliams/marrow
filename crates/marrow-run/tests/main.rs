#[macro_use]
mod support;
mod evolution_apply_support;

#[path = "cases/commit_id_conformance.rs"]
mod commit_id_conformance;
#[path = "cases/contract_composite_partial_key.rs"]
mod contract_composite_partial_key;
#[path = "cases/contract_post_delete_index_counts.rs"]
mod contract_post_delete_index_counts;
#[path = "cases/contract_read_only_closure.rs"]
mod contract_read_only_closure;
#[path = "cases/contract_saved_data.rs"]
mod contract_saved_data;
#[path = "cases/contract_store_isolation.rs"]
mod contract_store_isolation;
#[path = "cases/contract_transactions.rs"]
mod contract_transactions;
#[path = "cases/contract_txn_rollback_retry.rs"]
mod contract_txn_rollback_retry;
#[path = "cases/entry_args.rs"]
mod entry_args;
#[path = "cases/eval_asserts_groups.rs"]
mod eval_asserts_groups;
#[path = "cases/eval_basics.rs"]
mod eval_basics;
#[path = "cases/eval_count.rs"]
mod eval_count;
#[path = "cases/eval_debugger_hook.rs"]
mod eval_debugger_hook;
#[path = "cases/eval_delete_transactions.rs"]
mod eval_delete_transactions;
#[path = "cases/eval_enum_dispatch.rs"]
mod eval_enum_dispatch;
#[path = "cases/eval_error_model.rs"]
mod eval_error_model;
#[path = "cases/eval_group_write_faults.rs"]
mod eval_group_write_faults;
#[path = "cases/eval_host_effects.rs"]
mod eval_host_effects;
#[path = "cases/eval_identity_refs.rs"]
mod eval_identity_refs;
#[path = "cases/eval_index_identity.rs"]
mod eval_index_identity;
#[path = "cases/eval_index_iteration.rs"]
mod eval_index_iteration;
#[path = "cases/eval_keyed_leaves.rs"]
mod eval_keyed_leaves;
#[path = "cases/eval_layer_enumeration.rs"]
mod eval_layer_enumeration;
#[path = "cases/eval_maintenance.rs"]
mod eval_maintenance;
#[path = "cases/eval_module_dispatch.rs"]
mod eval_module_dispatch;
#[path = "cases/eval_optional_chains_nextid.rs"]
mod eval_optional_chains_nextid;
#[path = "cases/eval_ordered_navigation.rs"]
mod eval_ordered_navigation;
#[path = "cases/eval_read_only_expression.rs"]
mod eval_read_only_expression;
#[path = "cases/eval_reference_sample.rs"]
mod eval_reference_sample;
#[path = "cases/eval_resource_identity.rs"]
mod eval_resource_identity;
#[path = "cases/eval_resources.rs"]
mod eval_resources;
#[path = "cases/eval_run_fault_codes.rs"]
mod eval_run_fault_codes;
#[path = "cases/eval_saved_path_lowering.rs"]
mod eval_saved_path_lowering;
#[path = "cases/eval_saved_reads.rs"]
mod eval_saved_reads;
#[path = "cases/eval_saved_root_streaming.rs"]
mod eval_saved_root_streaming;
#[path = "cases/eval_saved_writes.rs"]
mod eval_saved_writes;
#[path = "cases/eval_std_builtins.rs"]
mod eval_std_builtins;
#[path = "cases/eval_strings_dispatch.rs"]
mod eval_strings_dispatch;
#[path = "cases/eval_temporal_conversions.rs"]
mod eval_temporal_conversions;
#[path = "cases/eval_traversal_guards.rs"]
mod eval_traversal_guards;
#[path = "cases/eval_values.rs"]
mod eval_values;
#[path = "cases/eval_values_entries.rs"]
mod eval_values_entries;
#[path = "cases/eval_vars_return_values.rs"]
mod eval_vars_return_values;
#[path = "cases/eval_whole_resources.rs"]
mod eval_whole_resources;
#[path = "cases/evolution_apply_catalog_publish.rs"]
mod evolution_apply_catalog_publish;
#[path = "cases/evolution_apply_defaults.rs"]
mod evolution_apply_defaults;
#[path = "cases/evolution_apply_drift.rs"]
mod evolution_apply_drift;
#[path = "cases/evolution_apply_fence.rs"]
mod evolution_apply_fence;
#[path = "cases/evolution_apply_indexes.rs"]
mod evolution_apply_indexes;
#[path = "cases/evolution_apply_lifecycle.rs"]
mod evolution_apply_lifecycle;
#[path = "cases/evolution_apply_retire.rs"]
mod evolution_apply_retire;
#[path = "cases/evolution_apply_transform_faults.rs"]
mod evolution_apply_transform_faults;
#[path = "cases/evolution_apply_transforms.rs"]
mod evolution_apply_transforms;
#[path = "cases/evolution_auto_apply.rs"]
mod evolution_auto_apply;
#[path = "cases/project_session.rs"]
mod project_session;
#[path = "cases/scenario_db_edge_cases.rs"]
mod scenario_db_edge_cases;
#[path = "cases/scenario_evolve_identity_and_fence.rs"]
mod scenario_evolve_identity_and_fence;
#[path = "cases/scenario_lang_db_seams.rs"]
mod scenario_lang_db_seams;
#[path = "cases/surface_read.rs"]
mod surface_read;
