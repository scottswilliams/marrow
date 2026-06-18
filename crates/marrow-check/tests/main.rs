mod support;
mod support_binding;
mod support_conversion;
mod support_discharge;
mod support_enum;

#[path = "cases/analysis_api.rs"]
mod analysis_api;
#[path = "cases/binding_enum_annotations.rs"]
mod binding_enum_annotations;
#[path = "cases/binding_enum_member_literals.rs"]
mod binding_enum_member_literals;
#[path = "cases/binding_match_arm_dispatch.rs"]
mod binding_match_arm_dispatch;
#[path = "cases/binding_name_resolution.rs"]
mod binding_name_resolution;
#[path = "cases/binding_rename_safety.rs"]
mod binding_rename_safety;
#[path = "cases/binding_resource_constructors.rs"]
mod binding_resource_constructors;
#[path = "cases/catalog_presence_binding.rs"]
mod catalog_presence_binding;
#[path = "cases/catalog_presence_enum.rs"]
mod catalog_presence_enum;
#[path = "cases/catalog_presence_evolve.rs"]
mod catalog_presence_evolve;
#[path = "cases/catalog_presence_identity.rs"]
mod catalog_presence_identity;
#[path = "cases/catalog_presence_narrowing.rs"]
mod catalog_presence_narrowing;
#[path = "cases/catalog_presence_provider.rs"]
mod catalog_presence_provider;
#[path = "cases/checked_program_artifact.rs"]
mod checked_program_artifact;
#[path = "cases/checked_program_error_construct.rs"]
mod checked_program_error_construct;
#[path = "cases/checked_program_error_value.rs"]
mod checked_program_error_value;
#[path = "cases/checked_program_facts.rs"]
mod checked_program_facts;
#[path = "cases/checked_program_identity.rs"]
mod checked_program_identity;
#[path = "cases/checked_program_keys.rs"]
mod checked_program_keys;
#[path = "cases/checked_program_navigation.rs"]
mod checked_program_navigation;
#[path = "cases/checked_program_operators.rs"]
mod checked_program_operators;
#[path = "cases/checked_program_stdlib.rs"]
mod checked_program_stdlib;
#[path = "cases/commit_amplification.rs"]
mod commit_amplification;
#[path = "cases/cross_module_map_enum_identity.rs"]
mod cross_module_map_enum_identity;
#[path = "cases/discharge_defaults.rs"]
mod discharge_defaults;
#[path = "cases/discharge_digest.rs"]
mod discharge_digest;
#[path = "cases/discharge_enum.rs"]
mod discharge_enum;
#[path = "cases/discharge_index.rs"]
mod discharge_index;
#[path = "cases/discharge_keyed_layer_shape.rs"]
mod discharge_keyed_layer_shape;
#[path = "cases/discharge_leaf_reshape.rs"]
mod discharge_leaf_reshape;
#[path = "cases/discharge_required_leaf_presence.rs"]
mod discharge_required_leaf_presence;
#[path = "cases/discharge_retype.rs"]
mod discharge_retype;
#[path = "cases/discharge_store_key.rs"]
mod discharge_store_key;
#[path = "cases/discharge_structural_backstop.rs"]
mod discharge_structural_backstop;
#[path = "cases/discharge_transform.rs"]
mod discharge_transform;
#[path = "cases/effect_footprints.rs"]
mod effect_footprints;
#[path = "cases/enum_member_id_stability.rs"]
mod enum_member_id_stability;
#[path = "cases/error_cascade_isolation.rs"]
mod error_cascade_isolation;
#[path = "cases/language_reference_docs.rs"]
mod language_reference_docs;
#[path = "cases/lossy_round_trip.rs"]
mod lossy_round_trip;
#[path = "cases/optional_chain_enum_typing.rs"]
mod optional_chain_enum_typing;
#[path = "cases/project_analysis_overlay_snapshot.rs"]
mod project_analysis_overlay_snapshot;
#[path = "cases/project_analysis_pipeline.rs"]
mod project_analysis_pipeline;
#[path = "cases/project_analysis_test_resolution.rs"]
mod project_analysis_test_resolution;
#[path = "cases/project_calls.rs"]
mod project_calls;
#[path = "cases/project_control_flow.rs"]
mod project_control_flow;
#[path = "cases/project_cross_module.rs"]
mod project_cross_module;
#[path = "cases/project_enum_hierarchy.rs"]
mod project_enum_hierarchy;
#[path = "cases/project_enum_member_and_match.rs"]
mod project_enum_member_and_match;
#[path = "cases/project_enum_nominal_identity.rs"]
mod project_enum_nominal_identity;
#[path = "cases/project_enum_saved_fields.rs"]
mod project_enum_saved_fields;
#[path = "cases/project_modules.rs"]
mod project_modules;
#[path = "cases/project_nested_scripts.rs"]
mod project_nested_scripts;
#[path = "cases/project_schema.rs"]
mod project_schema;
#[path = "cases/project_statements.rs"]
mod project_statements;
#[path = "cases/project_surfaces.rs"]
mod project_surfaces;
#[path = "cases/project_type_flow_builtins_and_conversions.rs"]
mod project_type_flow_builtins_and_conversions;
#[path = "cases/project_type_flow_calls.rs"]
mod project_type_flow_calls;
#[path = "cases/project_type_flow_saved_reads.rs"]
mod project_type_flow_saved_reads;
#[path = "cases/project_values.rs"]
mod project_values;
#[path = "cases/ranges.rs"]
mod ranges;
#[path = "cases/required_field_assignment.rs"]
mod required_field_assignment;
#[path = "cases/resource_store_contract.rs"]
mod resource_store_contract;
#[path = "cases/saved_place_owner_architecture.rs"]
mod saved_place_owner_architecture;
#[path = "cases/v01_fixtures.rs"]
mod v01_fixtures;
