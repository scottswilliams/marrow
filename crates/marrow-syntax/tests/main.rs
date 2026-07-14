mod common;

#[path = "cases/format.rs"]
mod format;
#[path = "cases/fuzz.rs"]
mod fuzz;
#[path = "cases/lexer.rs"]
mod lexer;
#[path = "cases/parse_call_arg_order_consistency.rs"]
mod parse_call_arg_order_consistency;
#[path = "cases/parse_control_flow.rs"]
mod parse_control_flow;
#[path = "cases/parse_declarations.rs"]
mod parse_declarations;
#[path = "cases/parse_enums.rs"]
mod parse_enums;
#[path = "cases/parse_evolve.rs"]
mod parse_evolve;
#[path = "cases/parse_expressions.rs"]
mod parse_expressions;
#[path = "cases/parse_interpolation_edges.rs"]
mod parse_interpolation_edges;
#[path = "cases/parse_paths_calls.rs"]
mod parse_paths_calls;
#[path = "cases/parse_resource_member_order.rs"]
mod parse_resource_member_order;
#[path = "cases/parse_resources_storage.rs"]
mod parse_resources_storage;
#[path = "cases/parse_statements.rs"]
mod parse_statements;
#[path = "cases/parse_type_expr.rs"]
mod parse_type_expr;
#[path = "cases/parse_types_params.rs"]
mod parse_types_params;
#[path = "cases/roundtrip.rs"]
mod roundtrip;
#[path = "cases/total_parser_architecture.rs"]
mod total_parser_architecture;
#[path = "cases/type_expr_architecture.rs"]
mod type_expr_architecture;
