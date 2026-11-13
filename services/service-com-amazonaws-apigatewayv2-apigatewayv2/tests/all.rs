// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/absent_page_delete_idempotence.rs"]
mod absent_page_delete_idempotence;
#[path = "all/absent_portal_product_delete_idempotence.rs"]
mod absent_portal_product_delete_idempotence;
#[path = "all/access_log_settings_deletion.rs"]
mod access_log_settings_deletion;
#[path = "all/api_endpoint_scheme.rs"]
mod api_endpoint_scheme;
#[path = "all/api_import.rs"]
mod api_import;
#[path = "all/api_import_advisory_report.rs"]
mod api_import_advisory_report;
#[path = "all/api_import_integration_warnings.rs"]
mod api_import_integration_warnings;
#[path = "all/api_import_oas_version_refusal.rs"]
mod api_import_oas_version_refusal;
#[path = "all/api_lifecycle.rs"]
mod api_lifecycle;
#[path = "all/api_mapping_key_claim_race.rs"]
mod api_mapping_key_claim_race;
#[path = "all/api_mapping_lifecycle.rs"]
mod api_mapping_lifecycle;
#[path = "all/api_mapping_update.rs"]
mod api_mapping_update;
#[path = "all/api_reimport.rs"]
mod api_reimport;
#[path = "all/api_reimport_identity.rs"]
mod api_reimport_identity;
#[path = "all/api_update.rs"]
mod api_update;
#[path = "all/authorizer_deletion.rs"]
mod authorizer_deletion;
#[path = "all/authorizer_lifecycle.rs"]
mod authorizer_lifecycle;
#[path = "all/authorizer_update.rs"]
mod authorizer_update;
#[path = "all/authorizers_cache_reset.rs"]
mod authorizers_cache_reset;
#[path = "all/auto_deploy_republishes.rs"]
mod auto_deploy_republishes;
#[path = "all/child_resource_delete_is_applied.rs"]
mod child_resource_delete_is_applied;
#[path = "all/child_collection_pagination_token.rs"]
mod child_collection_pagination_token;
#[path = "all/cors_configuration.rs"]
mod cors_configuration;
#[path = "all/cors_protocol_refusal.rs"]
mod cors_protocol_refusal;
#[path = "all/constraint_refusal.rs"]
mod constraint_refusal;
#[path = "all/control_plane_request_validation.rs"]
mod control_plane_request_validation;
#[path = "all/create_api_modelled_outcomes.rs"]
mod create_api_modelled_outcomes;
#[path = "all/custom_domain_hosted_zone.rs"]
mod custom_domain_hosted_zone;
#[path = "all/date_time_schema_coverage.rs"]
mod date_time_schema_coverage;
#[path = "all/deployment_deletion.rs"]
mod deployment_deletion;
#[path = "all/deployment_lifecycle.rs"]
mod deployment_lifecycle;
#[path = "all/deployment_pagination.rs"]
mod deployment_pagination;
#[path = "all/deployment_stage_release.rs"]
mod deployment_stage_release;
#[path = "all/disabled_execute_api_endpoint.rs"]
mod disabled_execute_api_endpoint;
#[path = "all/domain_name_deletion.rs"]
mod domain_name_deletion;
#[path = "all/domain_name_deletion_identity.rs"]
mod domain_name_deletion_identity;
#[path = "all/domain_name_endpoint_required.rs"]
mod domain_name_endpoint_required;
#[path = "all/domain_name_lifecycle.rs"]
mod domain_name_lifecycle;
#[path = "all/domain_name_pagination.rs"]
mod domain_name_pagination;
#[path = "all/empty_answer_framing.rs"]
mod empty_answer_framing;
#[path = "all/http_api_access_logging.rs"]
mod http_api_access_logging;
#[path = "all/http_api_backend_dispatch.rs"]
mod http_api_backend_dispatch;
#[path = "all/http_api_cors_data_plane.rs"]
mod http_api_cors_data_plane;
#[path = "all/http_api_event_path.rs"]
mod http_api_event_path;
#[path = "all/http_api_integration_fault_status.rs"]
mod http_api_integration_fault_status;
#[path = "all/http_api_jwt_claims_reach_the_integration.rs"]
mod http_api_jwt_claims_reach_the_integration;
#[path = "all/http_api_jwt_data_plane.rs"]
mod http_api_jwt_data_plane;
#[path = "all/http_api_jwt_scope_gate.rs"]
mod http_api_jwt_scope_gate;
#[path = "all/http_api_payload_quota.rs"]
mod http_api_payload_quota;
#[path = "all/http_api_published_endpoint_reporters.rs"]
mod http_api_published_endpoint_reporters;
#[path = "all/http_api_published_invoke_address.rs"]
mod http_api_published_invoke_address;
#[path = "all/http_api_request_authorizer.rs"]
mod http_api_request_authorizer;
#[path = "all/http_api_route_precedence.rs"]
mod http_api_route_precedence;
#[path = "all/http_api_servable_stage.rs"]
mod http_api_servable_stage;
#[path = "all/http_api_service_action_integration.rs"]
mod http_api_service_action_integration;
#[path = "all/http_api_signed_invoke.rs"]
mod http_api_signed_invoke;
#[path = "all/http_api_step_functions_subtype.rs"]
mod http_api_step_functions_subtype;
#[path = "all/integration_deletion.rs"]
mod integration_deletion;
#[path = "all/integration_lifecycle.rs"]
mod integration_lifecycle;
#[path = "all/integration_pagination.rs"]
mod integration_pagination;
#[path = "all/integration_response_key_grammar.rs"]
mod integration_response_key_grammar;
#[path = "all/integration_response_lifecycle.rs"]
mod integration_response_lifecycle;
#[path = "all/integration_response_patch.rs"]
mod integration_response_patch;
#[path = "all/integration_update.rs"]
mod integration_update;
#[path = "all/integration_uri_length.rs"]
mod integration_uri_length;
#[path = "all/integration_validation.rs"]
mod integration_validation;
#[path = "all/iso8601_timestamp_members.rs"]
mod iso8601_timestamp_members;
#[path = "all/minted_id_shape.rs"]
mod minted_id_shape;
#[path = "all/model_lifecycle.rs"]
mod model_lifecycle;
#[path = "all/model_template.rs"]
mod model_template;
#[path = "all/mutating_success_status.rs"]
mod mutating_success_status;
#[path = "all/not_found_sentence_per_addressing_operation.rs"]
mod not_found_sentence_per_addressing_operation;
#[path = "all/portal_data_plane.rs"]
mod portal_data_plane;
#[path = "all/portal_deletion.rs"]
mod portal_deletion;
#[path = "all/portal_hosted_zone.rs"]
mod portal_hosted_zone;
#[path = "all/portal_identity_authorization.rs"]
mod portal_identity_authorization;
#[path = "all/portal_lifecycle.rs"]
mod portal_lifecycle;
#[path = "all/portal_pagination.rs"]
mod portal_pagination;
#[path = "all/portal_product_lifecycle.rs"]
mod portal_product_lifecycle;
#[path = "all/portal_product_listing_order.rs"]
mod portal_product_listing_order;
#[path = "all/portal_product_member_authorization.rs"]
mod portal_product_member_authorization;
#[path = "all/portal_product_pagination.rs"]
mod portal_product_pagination;
#[path = "all/portal_product_sharing_policy.rs"]
mod portal_product_sharing_policy;
#[path = "all/portal_product_update.rs"]
mod portal_product_update;
#[path = "all/portal_publication.rs"]
mod portal_publication;
#[path = "all/portal_publication_revocation.rs"]
mod portal_publication_revocation;
#[path = "all/portal_terminal_lifecycle.rs"]
mod portal_terminal_lifecycle;
#[path = "all/portal_update.rs"]
mod portal_update;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/product_page_deletion.rs"]
mod product_page_deletion;
#[path = "all/product_page_lifecycle.rs"]
mod product_page_lifecycle;
#[path = "all/product_page_not_found_sentence.rs"]
mod product_page_not_found_sentence;
#[path = "all/product_page_update.rs"]
mod product_page_update;
#[path = "all/product_rest_endpoint_page_deletion.rs"]
mod product_rest_endpoint_page_deletion;
#[path = "all/product_rest_endpoint_page_generation.rs"]
mod product_rest_endpoint_page_generation;
#[path = "all/product_rest_endpoint_page_lifecycle.rs"]
mod product_rest_endpoint_page_lifecycle;
#[path = "all/product_rest_endpoint_page_published_endpoint.rs"]
mod product_rest_endpoint_page_published_endpoint;
#[path = "all/product_rest_endpoint_page_update.rs"]
mod product_rest_endpoint_page_update;
#[path = "all/protocol_restricted_member_message.rs"]
mod protocol_restricted_member_message;
#[path = "all/protocol_type_gating.rs"]
mod protocol_type_gating;
#[path = "all/query_form.rs"]
mod query_form;
#[path = "all/quick_create_is_http_only.rs"]
mod quick_create_is_http_only;
#[path = "all/quick_create_servable_set.rs"]
mod quick_create_servable_set;
#[path = "all/refusal_class_fidelity.rs"]
mod refusal_class_fidelity;
#[path = "all/refusal_message_text.rs"]
mod refusal_message_text;
#[path = "all/refusal_resource_identity.rs"]
mod refusal_resource_identity;
#[path = "all/reset_authorizers_cache.rs"]
mod reset_authorizers_cache;
#[path = "all/route_deletion.rs"]
mod route_deletion;
#[path = "all/route_request_parameter.rs"]
mod route_request_parameter;
#[path = "all/route_response_lifecycle.rs"]
mod route_response_lifecycle;
#[path = "all/route_response_update.rs"]
mod route_response_update;
#[path = "all/route_selection_expression_validation.rs"]
mod route_selection_expression_validation;
#[path = "all/route_settings_deletion.rs"]
mod route_settings_deletion;
#[path = "all/route_target_forms.rs"]
mod route_target_forms;
#[path = "all/route_update.rs"]
mod route_update;
#[path = "all/route_validation.rs"]
mod route_validation;
#[path = "all/routing_rule_deletion.rs"]
mod routing_rule_deletion;
#[path = "all/routing_rule_lifecycle.rs"]
mod routing_rule_lifecycle;
#[path = "all/routing_rule_read_back.rs"]
mod routing_rule_read_back;
#[path = "all/routing_rule_update.rs"]
mod routing_rule_update;
#[path = "all/server_applied_defaults.rs"]
mod server_applied_defaults;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/stage_auto_deploy_presence.rs"]
mod stage_auto_deploy_presence;
#[path = "all/stage_deletion.rs"]
mod stage_deletion;
#[path = "all/stage_lifecycle.rs"]
mod stage_lifecycle;
#[path = "all/stage_pagination.rs"]
mod stage_pagination;
#[path = "all/stage_update.rs"]
mod stage_update;
#[path = "all/stage_variable_backend_selection.rs"]
mod stage_variable_backend_selection;
#[path = "all/stage_variable_invoke_url_value.rs"]
mod stage_variable_invoke_url_value;
#[path = "all/stage_variable_values.rs"]
mod stage_variable_values;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/vpc_link_deletion.rs"]
mod vpc_link_deletion;
#[path = "all/vpc_link_lifecycle.rs"]
mod vpc_link_lifecycle;
#[path = "all/vpc_link_listing.rs"]
mod vpc_link_listing;
#[path = "all/vpc_link_subnet_resolution.rs"]
mod vpc_link_subnet_resolution;
#[path = "all/vpc_link_update.rs"]
mod vpc_link_update;
#[path = "all/websocket_call_context.rs"]
mod websocket_call_context;
#[path = "all/websocket_connect_authorization.rs"]
mod websocket_connect_authorization;
#[path = "all/websocket_connect_backend.rs"]
mod websocket_connect_backend;
#[path = "all/websocket_endpoint_connections.rs"]
mod websocket_endpoint_connections;
#[path = "all/websocket_handshake_data_plane.rs"]
mod websocket_handshake_data_plane;
#[path = "all/websocket_lambda_routes.rs"]
mod websocket_lambda_routes;
#[path = "all/websocket_transport_data_plane.rs"]
mod websocket_transport_data_plane;
