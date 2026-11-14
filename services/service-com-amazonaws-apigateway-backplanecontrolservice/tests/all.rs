// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
//
// Declared here, ahead of the sorted list, ON PURPOSE: every apigateway fix that lands
// inserts its suite into that list or appends after it, so both of those regions collide
// with whatever else lands in the same window — this fix lost several serialized squash
// merges to exactly that, as the `update_deployment` note at the end of this file records
// for another. Fold it into the sorted list whenever the list is next touched for another
// reason; the suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/documentation_part_creation.rs"]
mod documentation_part_creation;
// Declared here, at the head, for the reason the entry above records.
#[path = "all/stage_lambda_proxy_integration.rs"]
mod stage_lambda_proxy_integration;
#[path = "all/stage_custom_authorizer.rs"]
mod stage_custom_authorizer;
#[path = "all/passed_through_integration.rs"]
mod passed_through_integration;
#[path = "all/stage_cors_preflight.rs"]
mod stage_cors_preflight;
#[path = "all/proxy_binary_media_types.rs"]
mod proxy_binary_media_types;
#[path = "all/http_proxy_function_url.rs"]
mod http_proxy_function_url;
#[path = "all/proxy_event_repeated_names.rs"]
mod proxy_event_repeated_names;
#[path = "all/test_invoke_method_lambda_permission.rs"]
mod test_invoke_method_lambda_permission;
// Declared here, at the head, for the reason the entry at the top of this file records: what
// a simulated invocation of a method whose integration `uri` names a function, version or
// alias that resolves to nothing is answered — and logged — with spans API Gateway, AWS
// Lambda and Amazon CloudWatch Logs at once.
#[path = "all/test_invoke_method_unresolvable_function.rs"]
mod test_invoke_method_unresolvable_function;
#[path = "all/stage_access_logging.rs"]
mod stage_access_logging;
#[path = "all/stage_proxy_run_logs.rs"]
mod stage_proxy_run_logs;
#[path = "all/stage_trace_context.rs"]
mod stage_trace_context;
// Declared here, at the head, for the reason the entry above the proxy suites records: what a
// body-validating request validator does to a request whose integration names a real Lambda
// backend spans API Gateway, AWS Lambda and Amazon DynamoDB at once.
#[path = "all/validated_proxy_stage_spares_its_backend.rs"]
mod validated_proxy_stage_spares_its_backend;
// Declared here, at the head, for the reason the entry above the proxy suites records.
#[path = "all/stage_trace_sampling_decision.rs"]
mod stage_trace_sampling_decision;
#[path = "all/stage_web_acl_edge.rs"]
mod stage_web_acl_edge;
// What that same web ACL LOGS about the requests it decided: the record its
// `LoggingConfiguration` delivers to a CloudWatch Logs log group, with the declared fields
// redacted.
#[path = "all/stage_web_acl_logging.rs"]
mod stage_web_acl_logging;
// Which DESTINATION those records reach: both published forms of a log-group ARN name the
// same group, the count of records tracks the requests inspected, and a destination the
// prefix rule refuses leaves the binding, the records and the stage alone.
#[path = "all/stage_web_acl_logging_destination_binding.rs"]
mod stage_web_acl_logging_destination_binding;
#[path = "all/stage_web_acl_association_report.rs"]
mod stage_web_acl_association_report;
#[path = "all/aws_service_integration.rs"]
mod aws_service_integration;
#[path = "all/account_settings.rs"]
mod account_settings;
// The account's CloudWatch role resolved against IAM: which `cloudwatchRoleArn` an
// `UpdateAccount` patch stores, and which one names a role API Gateway could never assume.
#[path = "all/account_logging_role.rs"]
mod account_logging_role;
// That the same question ANSWERS: the account-role patch reaches a terminal status line on the
// first call, leaves a refused account exactly as it was, and the role it accepts is the one the
// stage's execution log is then delivered under.
#[path = "all/account_role_answered_within_the_request.rs"]
mod account_role_answered_within_the_request;
#[path = "all/api_key_collection.rs"]
mod api_key_collection;
#[path = "all/api_key_collection_members.rs"]
mod api_key_collection_members;
#[path = "all/api_key_import.rs"]
mod api_key_import;
#[path = "all/api_key_lifecycle.rs"]
mod api_key_lifecycle;
#[path = "all/authorizer_lifecycle.rs"]
mod authorizer_lifecycle;
#[path = "all/delete_authorizer.rs"]
mod delete_authorizer;
#[path = "all/delete_integration.rs"]
mod delete_integration;
#[path = "all/delete_lifecycle.rs"]
mod delete_lifecycle;
#[path = "all/delete_method_response.rs"]
mod delete_method_response;
#[path = "all/delete_stage.rs"]
mod delete_stage;
#[path = "all/documentation_part_import_bulk.rs"]
mod documentation_part_import_bulk;
#[path = "all/documentation_part_import_document.rs"]
mod documentation_part_import_document;
#[path = "all/documentation_parts.rs"]
mod documentation_parts;
#[path = "all/documentation_version_collection.rs"]
mod documentation_version_collection;
#[path = "all/documentation_versions.rs"]
mod documentation_versions;
#[path = "all/domain_name_access_association_reject.rs"]
mod domain_name_access_association_reject;
#[path = "all/domain_name_collection.rs"]
mod domain_name_collection;
#[path = "all/domain_name_create_bounds.rs"]
mod domain_name_create_bounds;
#[path = "all/domain_name_lifecycle.rs"]
mod domain_name_lifecycle;
#[path = "all/dynamodb_service_integration.rs"]
mod dynamodb_service_integration;
#[path = "all/epoch_second_timestamps.rs"]
mod epoch_second_timestamps;
#[path = "all/execute_api_data_plane.rs"]
mod execute_api_data_plane;
#[path = "all/flush_stage_authorizers_cache.rs"]
mod flush_stage_authorizers_cache;
#[path = "all/flush_stage_cache.rs"]
mod flush_stage_cache;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
#[path = "all/gateway_response.rs"]
mod gateway_response;
#[path = "all/gateway_response_collection.rs"]
mod gateway_response_collection;
#[path = "all/gateway_response_scope.rs"]
mod gateway_response_scope;
#[path = "all/integration_response.rs"]
mod integration_response;
#[path = "all/method_integration.rs"]
mod method_integration;
#[path = "all/method_response.rs"]
mod method_response;
#[path = "all/patch_updates.rs"]
mod patch_updates;
// A REST API's own resource policy on the `execute-api` data plane: which requests to a
// deployed stage the document admits, and the `403` the `execute-api:Invoke` authorization
// sentence refuses the rest with — the gate a PRIVATE api pinned to one of its interface VPC
// endpoints is reached through.
#[path = "all/private_api_resource_policy.rs"]
mod private_api_resource_policy;
#[path = "all/query_value_coercion.rs"]
mod query_value_coercion;
// The front door a REGIONAL custom domain name publishes as its `regionalDomainName`: the
// address itself, the certificate it presents, the mutual-TLS truststore it enforces and the
// stage its base path mappings route to.
#[path = "all/regional_custom_domain_front_door.rs"]
mod regional_custom_domain_front_door;
#[path = "all/request_validator_lifecycle.rs"]
mod request_validator_lifecycle;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/rest_api_defaults.rs"]
mod rest_api_defaults;
#[path = "all/rest_api_identifier_refusals.rs"]
mod rest_api_identifier_refusals;
#[path = "all/rest_api_import.rs"]
mod rest_api_import;
#[path = "all/rest_api_lifecycle.rs"]
mod rest_api_lifecycle;
#[path = "all/sdk_types.rs"]
mod sdk_types;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/sns_service_integration.rs"]
mod sns_service_integration;
#[path = "all/stage_collection.rs"]
mod stage_collection;
// The access log format a stage may carry: which `accessLogSettings.format` identifies the
// request the line is about, and what the refusal of one that identifies none states.
#[path = "all/stage_access_log_format.rs"]
mod stage_access_log_format;
#[path = "all/stage_export.rs"]
mod stage_export;
#[path = "all/stage_invoke.rs"]
mod stage_invoke;
#[path = "all/stage_lifecycle.rs"]
mod stage_lifecycle;
// The client SDK a deployed stage is generated into: which languages a generator is written
// for, how a generator is configured, and what the generated project is made of.
#[path = "all/stage_sdk.rs"]
mod stage_sdk;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_reads_by_arn.rs"]
mod tag_reads_by_arn;
#[path = "all/update_integration.rs"]
mod update_integration;
#[path = "all/update_method_response.rs"]
mod update_method_response;
#[path = "all/update_stage.rs"]
mod update_stage;
#[path = "all/usage_plan_lifecycle.rs"]
mod usage_plan_lifecycle;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
#[path = "all/vpc_links.rs"]
mod vpc_links;
// Declared after the sorted list on purpose: every apigateway fix that lands adds its
// suite inside that list, so a line inserted there collides with every other suite
// landing in the same window — this fix lost three serialized squash merges to exactly
// that. Appending keeps this change out of the contended region; the suite itself is an
// ordinary `tests/all/<name>.rs` file. Fold it into the sorted list whenever the list is
// next touched for another reason.
#[path = "all/update_deployment.rs"]
mod update_deployment;
#[path = "all/delete_documentation_part.rs"]
mod delete_documentation_part;
#[path = "all/update_documentation_part.rs"]
mod update_documentation_part;
#[path = "all/update_stage_setting_spellings.rs"]
mod update_stage_setting_spellings;
#[path = "all/usage_plan_key_lifecycle.rs"]
mod usage_plan_key_lifecycle;
#[path = "all/deployment_collection.rs"]
mod deployment_collection;
#[path = "all/authorizer_collection.rs"]
mod authorizer_collection;
#[path = "all/update_stage_readers.rs"]
mod update_stage_readers;
#[path = "all/execution_logging_prerequisites.rs"]
mod execution_logging_prerequisites;
#[path = "all/usage_plan_creation_bounds.rs"]
mod usage_plan_creation_bounds;
#[path = "all/control_plane_deletes.rs"]
mod control_plane_deletes;
#[path = "all/delete_deployment.rs"]
mod delete_deployment;
#[path = "all/rest_api_create_from_import.rs"]
mod rest_api_create_from_import;
#[path = "all/usage_plan_collection.rs"]
mod usage_plan_collection;
#[path = "all/domain_name_access_associations.rs"]
mod domain_name_access_associations;
#[path = "all/stage_invoke_endpoint.rs"]
mod stage_invoke_endpoint;
#[path = "all/delete_integration_response.rs"]
mod delete_integration_response;
#[path = "all/update_authorizer.rs"]
mod update_authorizer;
#[path = "all/update_gateway_response.rs"]
mod update_gateway_response;
#[path = "all/stage_invoke_shared_stage_names.rs"]
mod stage_invoke_shared_stage_names;
#[path = "all/request_validator_collection.rs"]
mod request_validator_collection;
#[path = "all/api_key_gate.rs"]
mod api_key_gate;
#[path = "all/update_request_validator.rs"]
mod update_request_validator;
#[path = "all/update_stage_variables.rs"]
mod update_stage_variables;
#[path = "all/update_vpc_link.rs"]
mod update_vpc_link;
#[path = "all/update_documentation_version.rs"]
mod update_documentation_version;
#[path = "all/refusal_message_constants.rs"]
mod refusal_message_constants;
#[path = "all/delete_documentation_version.rs"]
mod delete_documentation_version;
#[path = "all/delete_usage_plan.rs"]
mod delete_usage_plan;
#[path = "all/update_integration_response.rs"]
mod update_integration_response;
#[path = "all/rest_api_import_security_warning.rs"]
mod rest_api_import_security_warning;
#[path = "all/request_validator_create_api_refusal.rs"]
mod request_validator_create_api_refusal;
#[path = "all/delete_domain_name.rs"]
mod delete_domain_name;
#[path = "all/stage_export_documentation.rs"]
mod stage_export_documentation;
#[path = "all/update_domain_name.rs"]
mod update_domain_name;
#[path = "all/test_invoke_method.rs"]
mod test_invoke_method;
#[path = "all/usage_plan_key_collection.rs"]
mod usage_plan_key_collection;
#[path = "all/stage_invoke_request_id.rs"]
mod stage_invoke_request_id;
#[path = "all/update_api_key_patch_path_refusal.rs"]
mod update_api_key_patch_path_refusal;
#[path = "all/delete_request_validator.rs"]
mod delete_request_validator;
#[path = "all/invalid_security_policy.rs"]
mod invalid_security_policy;
#[path = "all/base_path_mapping.rs"]
mod base_path_mapping;
#[path = "all/delete_gateway_response.rs"]
mod delete_gateway_response;
#[path = "all/update_usage_plan.rs"]
mod update_usage_plan;
#[path = "all/base_path_mapping_private_domain.rs"]
mod base_path_mapping_private_domain;
#[path = "all/test_invoke_method_no_integration.rs"]
mod test_invoke_method_no_integration;
#[path = "all/models.rs"]
mod models;
#[path = "all/authorizer_in_use.rs"]
mod authorizer_in_use;
#[path = "all/gateway_response_parameter_mappings.rs"]
mod gateway_response_parameter_mappings;
#[path = "all/documentation_part_import_atomicity.rs"]
mod documentation_part_import_atomicity;
#[path = "all/delete_model.rs"]
mod delete_model;
#[path = "all/delete_base_path_mapping.rs"]
mod delete_base_path_mapping;
#[path = "all/update_base_path_mapping.rs"]
mod update_base_path_mapping;
#[path = "all/update_model.rs"]
mod update_model;
#[path = "all/stage_request_validation.rs"]
mod stage_request_validation;
#[path = "all/model_in_use.rs"]
mod model_in_use;
#[path = "all/documentation_part_import_warnings.rs"]
mod documentation_part_import_warnings;
#[path = "all/model_collection.rs"]
mod model_collection;
#[path = "all/delete_domain_name_access_association.rs"]
mod delete_domain_name_access_association;
#[path = "all/base_path_mapping_collection.rs"]
mod base_path_mapping_collection;
#[path = "all/test_invoke_method_data_plane_parity.rs"]
mod test_invoke_method_data_plane_parity;
#[path = "all/request_validator_precedes_the_integration.rs"]
mod request_validator_precedes_the_integration;
#[path = "all/model_template.rs"]
mod model_template;
// Appended rather than folded into the sorted list above, for the reason the note on
// `update_deployment` gives: the sorted region is contended by every apigateway fix that
// lands in the same window.
#[path = "all/constraint_refusal_class.rs"]
mod constraint_refusal_class;
#[path = "all/rest_api_import_gateway_responses.rs"]
mod rest_api_import_gateway_responses;
#[path = "all/vpc_link_provisioning.rs"]
mod vpc_link_provisioning;
#[path = "all/vpc_link_records.rs"]
mod vpc_link_records;
// Appended for the reason the note above gives.
#[path = "all/stage_arn_names_the_creation_region.rs"]
mod stage_arn_names_the_creation_region;
#[path = "all/vpc_link_collection.rs"]
mod vpc_link_collection;
#[path = "all/rest_api_import_integrations.rs"]
mod rest_api_import_integrations;
#[path = "all/stage_refusal_error_class.rs"]
mod stage_refusal_error_class;
#[path = "all/stage_variable_integration_target.rs"]
mod stage_variable_integration_target;
#[path = "all/patch_path_refusal_templates.rs"]
mod patch_path_refusal_templates;
#[path = "all/usage_plan_state_and_refusals.rs"]
mod usage_plan_state_and_refusals;
#[path = "all/api_key_import_enablement.rs"]
mod api_key_import_enablement;
// Appended for the reason the note above gives: the STAGE RESPONSE CACHE of the
// `execute-api` data plane — what a method whose settings enable caching is answered with,
// until the entry the cluster holds expires, is flushed or is reloaded.
#[path = "all/stage_response_cache.rs"]
mod stage_response_cache;
// Appended for the reason the note above gives: the ORDER an `apiStages` entry's two
// identifiers are resolved in.
#[path = "all/usage_plan_api_stage_resolution.rs"]
mod usage_plan_api_stage_resolution;
// Appended for the reason the note above gives: WHICH record, condition and contract this
// control plane's 400 and 404 refusals name, and the sentences it names them in.
#[path = "all/refusal_sentence_targets.rs"]
mod refusal_sentence_targets;
// Appended for the reason the note above gives: what an exported definition says about
// CONTENT — the media types an operation negotiates, the API's TLS security policy, and
// the byte spelling of the YAML serialization.
#[path = "all/stage_export_content_types.rs"]
mod stage_export_content_types;
// Appended for the reason the note above gives: the MAPPING TEMPLATES a deployed stage
// applies — what an integration's request template and an integration response's response
// template are RENDERED against, and which of them this emulator does not render.
#[path = "all/stage_mapping_templates.rs"]
mod stage_mapping_templates;
// Appended for the reason the note above gives: WHOSE the two resources are is what
// decides `RejectDomainNameAccessAssociation` once both of its ARNs resolve.
#[path = "all/domain_name_access_association_reject_accounts.rs"]
mod domain_name_access_association_reject_accounts;
// Appended for the reason the note above gives: the ACCOUNT-level CloudWatch Logs role a
// stage configuration that turns execution logging on is refused without.
#[path = "all/stage_execution_logging_account_role.rs"]
mod stage_execution_logging_account_role;
// Appended for the reason the note above gives: the `position`/`limit` PAGE ENVELOPE every
// paginated collection of this service answers in, and the cursor the documentation-part
// collection is walked by.
#[path = "all/collection_page_envelope.rs"]
mod collection_page_envelope;
// ITSELF — the version the downloaded file is named for, and the containers each OpenAPI
// dialect always writes.
#[path = "all/stage_export_version_and_containers.rs"]
mod stage_export_version_and_containers;
// Appended for the reason the note above gives: a stage's CANARY RELEASE — whose
// deployment a canary serves, what promoting it makes the stage's — and the grammar a
// stage's `patchOperations` are read by.
#[path = "all/stage_canary_and_patch_grammar.rs"]
mod stage_canary_and_patch_grammar;
// Appended for the reason the note above gives: what a deployed stage answers a SIGNED
// invoke with, on an emulator that verifies request signatures.
#[path = "all/stage_invoke_signed.rs"]
mod stage_invoke_signed;
// Appended for the reason the note above gives: what this control plane ADMITS on the
// members whose value names something outside the request (a certificate, a URL path, a
// metering window).
#[path = "all/control_plane_input_admission.rs"]
mod control_plane_input_admission;
// Appended for the reason the note above gives: the records a base path mapping holds a
// REFERENCE to, and the deletes that reference refuses.
#[path = "all/base_path_mapping_references.rs"]
mod base_path_mapping_references;
// Appended for the reason the note above gives: the sentences a SETTING's own illegal
// value is refused with, and the one a domain name ARN this service cannot read earns.
#[path = "all/setting_refusal_sentences.rs"]
mod setting_refusal_sentences;
// Appended for the reason the note above gives: the SENTENCE and the error CLASS each
// refusal of this control plane carries.
#[path = "all/refusal_contract_sentences.rs"]
mod refusal_contract_sentences;
// Appended for the reason the note above gives: what removing ONE usage plan key changes —
// the plan stops metering that API key, at the gate as well as in the reads — and what a
// contended removal of one association resolves to.
#[path = "all/delete_usage_plan_key.rs"]
mod delete_usage_plan_key;
// Appended for the reason the note above gives: what a deployed stage answers a request
// signed with credentials whose principal is allowed no `execute-api:Invoke`.
#[path = "all/stage_invoke_identity_policy.rs"]
mod stage_invoke_identity_policy;
// Appended for the reason the note above gives: WHICH record a call addressed by a stage
// or a resource of a REST API reports when that address resolves to nothing.
#[path = "all/nested_record_refusals.rs"]
mod nested_record_refusals;
// Appended for the reason the note above gives: how an integration whose own HTTP method
// carries no value is refused.
#[path = "all/integration_http_method_enumeration.rs"]
mod integration_http_method_enumeration;
// Appended for the reason the note above gives: the `Model` records every REST API is
// CREATED with, over every create path.
#[path = "all/default_models.rs"]
mod default_models;
// Appended for the reason the note above gives: the `DeleteRequestValidator` a `Method` of
// the same API blocks while it still names the validator.
#[path = "all/request_validator_in_use.rs"]
mod request_validator_in_use;
// Appended for the reason the note above gives: the ORDER one `patchOperations` entry is
// judged in — the path before the op — and the sentence each verdict is worded with.
#[path = "all/patch_entry_precedence.rs"]
mod patch_entry_precedence;
// Appended for the reason the note above gives: what an `Integration` and an
// `IntegrationResponse` REPORT in every reading of them — the transfer mode, the
// connection type an imported definition declares, and the empty mapping template.
#[path = "all/integration_representation.rs"]
mod integration_representation;
// Appended for the reason the note above gives: the WIDTH and alphabet of the identifier
// each kind of record this control plane mints is addressed by.
#[path = "all/minted_identifier_widths.rs"]
mod minted_identifier_widths;
// Appended for the reason the note above gives: a deployed method whose integration is
// `AWS_PROXY`, served by INVOKING the Lambda function its `uri` names — across API Gateway,
// Lambda and DynamoDB on one engine.
#[path = "all/lambda_proxy_integration.rs"]
mod lambda_proxy_integration;
// Appended for the reason the note above gives: the identifier form an access association's
// `accessAssociationSource` must carry for the one source type the model declares.
#[path = "all/domain_name_access_association_source.rs"]
mod domain_name_access_association_source;
// Appended for the reason the note above gives: which method of one resource serves a
// request when the resource carries both an explicit verb and the `ANY` catch-all.
#[path = "all/stage_verb_precedence.rs"]
mod stage_verb_precedence;
// Appended for the reason the note above gives: an `AWS_PROXY` integration whose `uri`
// carries a QUALIFIER — an alias or a published version — and the code that qualifier
// resolves to.
#[path = "all/proxy_integration_qualified_function.rs"]
mod proxy_integration_qualified_function;
// Appended for the reason the note above gives: what every REPRESENTATION of a gateway
// response carries, the ORDER its collection is reported in, and the removal of a response
// parameter the response does not carry.
#[path = "all/gateway_response_representation.rs"]
mod gateway_response_representation;
// Appended for the reason the note above gives: the VPC endpoint an access association's
// `accessAssociationSource` must name, resolved against the service that owns endpoints.
#[path = "all/domain_name_access_association_endpoint.rs"]
mod domain_name_access_association_endpoint;
// Appended for the reason the note above gives: the USAGE DATA a usage plan reports — the
// daily log of used and remaining quota each of its API keys earns, and the interval and
// page a read of it covers.
#[path = "all/usage_plan_usage_report.rs"]
mod usage_plan_usage_report;
// Appended for the reason the note above gives: the CLIENT CERTIFICATES of the account —
// the material `GenerateClientCertificate` issues, the one member a patch of one changes,
// and the stage reference a delete of one is refused while it stands.
#[path = "all/client_certificates.rs"]
mod client_certificates;
// Appended for the reason the note above gives: the two `$util` conversions a mapping
// template may apply — the `application/x-www-form-urlencoded` encode and decode — and the
// `$util` methods this emulator still resolves nothing for.
#[path = "all/stage_template_url_conversions.rs"]
mod stage_template_url_conversions;
// Appended for the reason the note above gives: the QUOTA a usage plan enforces at a
// deployed stage — the budget of requests each of its API keys may send in a metering
// period, and the `QUOTA_EXCEEDED` refusal every request past it earns.
#[path = "all/usage_plan_quota_enforcement.rs"]
mod usage_plan_quota_enforcement;
// Appended for the reason the note above gives: WHICH refusal a `COGNITO_USER_POOLS`-gated
// method answers a token it does not admit with — the `401` of a request that authenticated
// nobody, the `401` of an expired token, and the `403` of an authorizer that denies the call.
#[path = "all/stage_cognito_token_refusal_class.rs"]
mod stage_cognito_token_refusal_class;
// Appended for the reason the note above gives: what SIMULATING one stored `Authorizer`
// reports — the decision its execution produced, and the status a client would have received.
#[path = "all/test_invoke_authorizer.rs"]
mod test_invoke_authorizer;
// Appended for the reason the note above gives: what the EDGE does to a request and to the
// answer independently of the integration — the payload it admits, and the content coding
// the api's `minimumCompressionSize` has it serve the answer under.
#[path = "all/stage_edge_admission.rs"]
mod stage_edge_admission;
// Appended for the reason the note above gives: what the edge SENDS for a refused request —
// the headers a gateway response's `responseParameters` write on it, the exception class it
// names, and the exact bytes of its default body.
#[path = "all/gateway_response_wire_rendering.rs"]
mod gateway_response_wire_rendering;
// Appended for the reason the note above gives: the method's own REQUEST VALIDATOR applied
// to a SIMULATED invocation, asserted beside the deployed stage's answer for the same
// records.
#[path = "all/test_invoke_method_request_validator.rs"]
mod test_invoke_method_request_validator;
// Appended for the reason the note above gives: the exception CLASS a deployed stage names on
// the `500` it answers when it cannot serve the call at all.
#[path = "all/stage_internal_error_class.rs"]
mod stage_internal_error_class;
// Appended for the reason the note above gives: `TestInvokeAuthorizer` against a
// `COGNITO_USER_POOLS` authorizer — the `claims` member a token it ACCEPTED fills and every
// other answer omits, beside the status a call that earned no decision reports.
#[path = "all/test_invoke_authorizer_cognito_claims.rs"]
mod test_invoke_authorizer_cognito_claims;
// Appended for the reason the note above gives: the GRANT of a remaining quota — what
// `UpdateUsage` writes into the metering period of one `(usage plan, API key)` pair, and what
// the data plane then admits against it.
#[path = "all/usage_plan_quota_extension.rs"]
mod usage_plan_quota_extension;
// Appended for the reason the note above gives: the whole ACCOUNT-role sequence a client
// scripts around a stage's execution logging, driven over the `AWS_PROXY` data plane — every
// call of it answers inside the request, and each outcome it drives is observable.
#[path = "all/stage_logging_role_sequence.rs"]
mod stage_logging_role_sequence;
// Appended for the reason the note above gives: the invoke ADDRESS the artifacts of a
// deployed stage advertise — the export's `host`/`basePath` and `servers[].url` and the
// endpoint a generated SDK is bound to, each asserted to be an address this emulator serves
// the stage on.
#[path = "all/stage_export_invoke_address.rs"]
mod stage_export_invoke_address;
// Appended for the reason the note above gives: the sentences a THROTTLE limit and a TAG
// outside its contract are refused with — one fixed sentence per setting class, carrying
// neither the patch path nor the rejected value.
#[path = "all/throttle_and_tag_contract_sentences.rs"]
mod throttle_and_tag_contract_sentences;
// Appended for the reason the note above gives: the CONTAINERS an exported OpenAPI 3.0
// document carries in every serialization — its Components Object and each Response
// Object's `content` map.
#[path = "all/stage_export_oas30_containers.rs"]
mod stage_export_oas30_containers;
// Appended for the reason the note above gives: the KEY a per-method throttle override of a
// usage plan's API stage association is held under, and the pointer spellings that address
// that one entry.
#[path = "all/usage_plan_method_throttle_keys.rs"]
mod usage_plan_method_throttle_keys;
// Appended for the reason the note above gives: what a `CUSTOM`-authorized method answers,
// whether its BACKEND runs, and the access-log record the stage leaves — the three halves of
// one request, across API Gateway, AWS Lambda and CloudWatch Logs on one engine.
#[path = "all/stage_authorizer_deny_blocks_the_backend.rs"]
mod stage_authorizer_deny_blocks_the_backend;
// Appended for the reason the note above gives: WHAT an `execute-api` authorization refusal
// reports — the method ARN of the path the client addressed, and the clause naming the
// document class that refused the call.
#[path = "all/stage_invoke_denial_sentence.rs"]
mod stage_invoke_denial_sentence;
// Appended for the reason the note above gives: the BOUNDARIES of a `position`/`limit`
// collection — the answer a collection that holds nothing makes, and the page a limited
// read of the documentation-part collection is cut at.
#[path = "all/collection_page_boundaries.rs"]
mod collection_page_boundaries;
// Appended for the reason the note above gives: WHERE a minted identifier comes from — that
// each one is drawn for the record it names rather than from the count of records created
// before it, and that the record keeps it.
#[path = "all/minted_identifier_independence.rs"]
mod minted_identifier_independence;
