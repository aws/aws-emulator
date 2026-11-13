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
#[path = "all/argument_validation.rs"]
mod argument_validation;
#[path = "all/buffering_hints.rs"]
mod buffering_hints;
#[path = "all/buffering_hints_destination_members.rs"]
mod buffering_hints_destination_members;
#[path = "all/clock_owed_transition_span.rs"]
mod clock_owed_transition_span;
#[path = "all/cloudwatch_logs_pipeline_delivery.rs"]
mod cloudwatch_logs_pipeline_delivery;
#[path = "all/cloudwatch_logs_source_gate.rs"]
mod cloudwatch_logs_source_gate;
#[path = "all/constraint_refusal_precedes_state.rs"]
mod constraint_refusal_precedes_state;
#[path = "all/control_plane_refusal_templates.rs"]
mod control_plane_refusal_templates;
#[path = "all/custom_time_zone_prefix.rs"]
mod custom_time_zone_prefix;
#[path = "all/delivered_object_content_encoding.rs"]
mod delivered_object_content_encoding;
#[path = "all/delivered_object_encryption.rs"]
mod delivered_object_encryption;
#[path = "all/delivered_object_name.rs"]
mod delivered_object_name;
#[path = "all/delivery_role_refusal.rs"]
mod delivery_role_refusal;
#[path = "all/delivery_stream_encryption.rs"]
mod delivery_stream_encryption;
#[path = "all/delivery_stream_lifecycle.rs"]
mod delivery_stream_lifecycle;
#[path = "all/delivery_stream_tag_updates.rs"]
mod delivery_stream_tag_updates;
#[path = "all/delivery_stream_tags.rs"]
mod delivery_stream_tags;
#[path = "all/description_defaults.rs"]
mod description_defaults;
#[path = "all/destination_argument_rules.rs"]
mod destination_argument_rules;
#[path = "all/destination_arity_refusal.rs"]
mod destination_arity_refusal;
#[path = "all/destination_bucket_preflight.rs"]
mod destination_bucket_preflight;
#[path = "all/destination_bucket_resolution.rs"]
mod destination_bucket_resolution;
#[path = "all/destination_configuration_echo.rs"]
mod destination_configuration_echo;
#[path = "all/destination_delivery.rs"]
mod destination_delivery;
#[path = "all/destination_delivery_errors.rs"]
mod destination_delivery_errors;
#[path = "all/destination_update.rs"]
mod destination_update;
#[path = "all/direct_put_source_gate.rs"]
mod direct_put_source_gate;
#[path = "all/encryption_change_not_found_message.rs"]
mod encryption_change_not_found_message;
#[path = "all/encryption_conflict_messages.rs"]
mod encryption_conflict_messages;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/error_output_prefix_resolution.rs"]
mod error_output_prefix_resolution;
#[path = "all/format_conversion_delivery.rs"]
mod format_conversion_delivery;
#[path = "all/http_endpoint_delivery.rs"]
mod http_endpoint_delivery;
#[path = "all/http_endpoint_lambda_error_output.rs"]
mod http_endpoint_lambda_error_output;
#[path = "all/kinesis_source_ingestion.rs"]
mod kinesis_source_ingestion;
#[path = "all/kms_key_resolution.rs"]
mod kms_key_resolution;
#[path = "all/legacy_s3_destination_mirror.rs"]
mod legacy_s3_destination_mirror;
#[path = "all/nested_schema_conversion.rs"]
mod nested_schema_conversion;
#[path = "all/not_found_message_name_space.rs"]
mod not_found_message_name_space;
#[path = "all/opensearch_destination_delivery.rs"]
mod opensearch_destination_delivery;
#[path = "all/opensearch_fgac_destination_delivery.rs"]
mod opensearch_fgac_destination_delivery;
#[path = "all/parquet_serializer_parameters.rs"]
mod parquet_serializer_parameters;
#[path = "all/processor_parameters.rs"]
mod processor_parameters;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/source_fed_stream_delivers_no_direct_put.rs"]
mod source_fed_stream_delivers_no_direct_put;
#[path = "all/splunk_destination_delivery.rs"]
mod splunk_destination_delivery;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_encryption.rs"]
mod stream_encryption;
#[path = "all/stream_not_found_message.rs"]
mod stream_not_found_message;
#[path = "all/tag_key_pattern.rs"]
mod tag_key_pattern;
