// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for the router.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/boot_serve.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
//
// This first entry is out of alphabetical order on purpose: both the appended region
// at the end of this file and the span of the sorted list beside a suite's own name
// are where concurrently landing suites add their line, so a declaration placed there
// collides with every one of them. The head of the list is the one region nothing
// else appends to.
#[path = "all/cloudformation_fifo_resource_generated_names.rs"]
mod cloudformation_fifo_resource_generated_names;
// Declared here, in this same head region, for the reason the entry above records: whether a
// flat-rate CloudFront subscription may be sold over a distribution is decided by the web ACL
// association CloudFront holds and by a web ACL only WAFv2 mints, so no single service crate's
// test target can set the association and then ask Pricing Plan Manager for the subscription.
#[path = "all/pricing_plan_subscription_resource_eligibility.rs"]
mod pricing_plan_subscription_resource_eligibility;
// Declared here, in this same head region, for the reason the entry above records: the work
// item an alarm's Systems Manager OpsItem action opens has to be read where a client reads it
// — through OpsCenter's own query — so neither CloudWatch's nor Systems Manager's own test
// target can drive the transition and then read what it opened.
#[path = "all/cloudwatch_alarm_ssm_ops_item_action.rs"]
mod cloudwatch_alarm_ssm_ops_item_action;
// Declared here, in this same head region, for the reason the entry above records: what a
// stack-deployed metric-stream pipeline exports before and after an update rewrites its filter
// spans CloudFormation, IAM, CloudWatch, Firehose and Amazon S3 at once, so no single service
// crate's test target can deploy the pipeline and then read the objects it delivered.
#[path = "all/cloudformation_metric_stream_filter_update.rs"]
mod cloudformation_metric_stream_filter_update;
// Lambda handler run as the work of a Step Functions task writes with the SDK the managed
// runtime supplies it spans Step Functions, IAM, Lambda, Amazon S3 and DynamoDB at once, so no
// single service crate's test target can run the chain and then read each write back.
#[path = "all/sfn_lambda_task_handler_writes_objects.rs"]
mod sfn_lambda_task_handler_writes_objects;
// Declared here, in this same head region, for the reason the entry above records: a stack
// that declares a private container registry and the task definition running an image out of
// it is only deployed if CloudFormation, Amazon ECR and Amazon ECS all agree, so no single
// crate's test target can read both halves back.
#[path = "all/cloudformation_registry_backed_task_definition.rs"]
mod cloudformation_registry_backed_task_definition;
// Declared in this same head region, for the reason the entry above records: the
// delivery-status record a stack-deployed SNS feedback role leaves for a delivery spans
// CloudFormation, IAM, Amazon SNS, Amazon SQS and CloudWatch Logs at once, so no single
// service crate's test target can deploy the stack and then read the log.
#[path = "all/cloudformation_sns_delivery_status_logging.rs"]
mod cloudformation_sns_delivery_status_logging;
// Declared in this same head region, for the reason the entry above records: an EventBridge
// replay's redelivery has to be observed where a client observes it — on the SQS queue the
// destination bus's rule targets — so neither crate's own test target can mount both sides of
// it.
#[path = "all/events_replay_redelivers_the_archived_window.rs"]
mod events_replay_redelivers_the_archived_window;
// Declared here, in this same head region, for the reason the entry above records: the
// identifiers a stack publishes for the Amazon Cognito user pool, app client, resource server
// and identity provider it declares are the ones that service minted or is addressed by, which
// spans CloudFormation and Amazon Cognito user pools at once, so neither crate's own test
// target can ask it.
#[path = "all/cloudformation_provisions_cognito_user_pools.rs"]
mod cloudformation_provisions_cognito_user_pools;
// object a WorkMail Message Flow update installs has to be written through Amazon S3's own
// API and read back through WorkMail Message Flow's, so neither crate's own test target can
// mount both sides of it.
#[path = "all/workmailmessageflow_resolves_an_s3_object.rs"]
mod workmailmessageflow_resolves_an_s3_object;
// client is answered by an endpoint serving a model SageMaker trained spans IAM, Amazon S3,
// the SageMaker control plane and the SageMaker runtime at once, so neither SageMaker
// crate's own test target can train a model and then invoke what it deployed.
#[path = "all/sagemaker_endpoint_serves_a_trained_model.rs"]
mod sagemaker_endpoint_serves_a_trained_model;
// configuration set's Kinesis Data Firehose event destination does with a message's events
// spans SESv2, IAM, Firehose and Amazon S3 at once, so no single service's test target can
// follow a published event to the object it lands in.
#[path = "all/ses_event_publication_reaches_firehose_s3.rs"]
mod ses_event_publication_reaches_firehose_s3;
// client of a CloudFormation-deployed HTTP api's `$default` Lambda route is served, and the
// CORS decoration that answer carries, spans CloudFormation, API Gateway v2, AWS Lambda and
// IAM at once, so no single service's test target can ask it.
#[path = "all/cloudformation_http_api_cors_lambda_default_route.rs"]
mod cloudformation_http_api_cors_lambda_default_route;
// Declared here, in this same head region, for the reason the entry above records: a
// guardrail the Amazon Bedrock control plane creates and the Bedrock Runtime data plane
// applies spans those two crates at once, so neither crate's own test target can see
// both sides of it.
#[path = "all/bedrock_guardrail_applied_across_planes.rs"]
mod bedrock_guardrail_applied_across_planes;
// StackSet status-change events a stack set's lifecycle publishes onto the default event
// bus, which span CloudFormation, EventBridge, Amazon SQS and Systems Manager at once.
#[path = "all/cloudformation_stackset_status_change_events.rs"]
mod cloudformation_stackset_status_change_events;
// `Condition`-gated `AWS::IAM::Policy` means for the stack's own function's read of the
// stack's own secret, which spans CloudFormation, IAM, Secrets Manager, Lambda and STS at
// once, so no single service's test target can ask it.
#[path = "all/cloudformation_condition_gated_secrets_policy.rs"]
mod cloudformation_condition_gated_secrets_policy;
// Declared here, in this same head region, for the reason the entries above record: the
// HANDLES a stack publishes for the Step Functions and Lambda resources it provisions, and
// what a declaration Step Functions refuses leaves behind, which span CloudFormation, Step
// Functions, Lambda, Amazon SQS and IAM at once.
#[path = "all/cloudformation_sfn_lambda_published_handles.rs"]
mod cloudformation_sfn_lambda_published_handles;
// metric-stream export pipeline a stack deploys, which only moves data if CloudFormation,
// IAM, CloudWatch, Firehose and Amazon S3 all agree, so no single crate's test target can
// follow a published datapoint to the object it lands in.
#[path = "all/cloudformation_metric_stream_exports_to_s3.rs"]
mod cloudformation_metric_stream_exports_to_s3;
// Declared here, in this same head region, for the reason the entry above records: what
// LAMBDA makes of an access point EFS minted, which neither crate's own test target can
// ask, since the ARN has to cross from one service to the other.
#[path = "all/efs_access_point_mounted_by_lambda.rs"]
mod efs_access_point_mounted_by_lambda;
// Declared here, in this same head region, for the reason the entry above records: what an
// EventBridge rule hands a Lambda target and which function qualifier the target ARN
// addresses, which spans EventBridge and Lambda at once.
#[path = "all/events_lambda_target_qualified_arn_envelope.rs"]
mod events_lambda_target_qualified_arn_envelope;
// Declared here, in this same head region, for the reason the entry above records: the two
// services sharing the `mgh` signing name are two crates, so neither crate's own test
// target can mount both and ask which of them claims a request.
#[path = "all/migrationhub_signing_name_routing.rs"]
mod migrationhub_signing_name_routing;
// services sharing the `agent-registry` signing name are two crates, so neither crate's own
// test target can mount both and ask which of them claims a request.
#[path = "all/agent_registry_signing_name_routing.rs"]
mod agent_registry_signing_name_routing;
// account's verified sending identity, which one SES surface records and the other
// discovers.
#[path = "all/ses_v1_discovers_a_verified_sending_identity.rs"]
mod ses_v1_discovers_a_verified_sending_identity;
// secret a stack declares, which has to exist in AWS Secrets Manager under the ARN the
// stack publishes for it.
#[path = "all/cloudformation_provisions_secrets_manager_secrets.rs"]
mod cloudformation_provisions_secrets_manager_secrets;
// RUNNING fault-injection experiment makes of the targeted role's calls to the service whose
// API it names, which spans four services.
#[path = "all/fis_injected_api_fault.rs"]
mod fis_injected_api_fault;
// deployed `AWS_IAM` method makes of the credential its request presents, on a router that
// verifies request signatures.
#[path = "all/apigateway_iam_authenticated_invoke.rs"]
mod apigateway_iam_authenticated_invoke;
// Declared here, in this same head region, for the reason the entry above records.
#[path = "all/cloudformation_rollback_triggers_revert_the_operation.rs"]
mod cloudformation_rollback_triggers_revert_the_operation;
// Declared in this span of the sorted list, beside the other `cloudformation…` suites,
// rather than at the head of this file or in its appended tail: both of those are where
// concurrently landing suites put their line, so a declaration there loses a squash merge to
// every one of them, while these neighbouring lines are stable.
#[path = "all/cloudformation_role_trusted_by_the_s3_service.rs"]
mod cloudformation_role_trusted_by_the_s3_service;
// Declared here, in this same head region, for the reason the entry above records: this
// suite holds the committed `Cargo.lock` to the workspace's manifests, so it belongs to no
// service's span of the sorted list.
#[path = "all/workspace_lockfile_covers_every_manifest.rs"]
mod workspace_lockfile_covers_every_manifest;
// Declared here, in this same head region, for the reason the entries above record: what a
// bucket's Lambda notification configuration DOES, which spans S3, Lambda, IAM and
// CloudFormation at once.
#[path = "all/s3_notification_invokes_its_lambda_target.rs"]
mod s3_notification_invokes_its_lambda_target;
// Declared here, in this same head region, beside the suite above it shares its wiring with:
// what the handler a bucket's notification runs DOES — the signed index request it sends to a
// deployed search domain — which spans S3, Lambda, IAM, STS, Amazon OpenSearch Service and
#[path = "all/s3_notification_indexes_into_its_opensearch_domain.rs"]
mod s3_notification_indexes_into_its_opensearch_domain;
// Declared here, in this same head region, beside the two suites above it shares its wiring
// with: what a STACK-DEPLOYED bucket notification does — the copy the handler writes back into
// the bucket the event came from — which spans CloudFormation, S3, Lambda and IAM at once.
#[path = "all/s3_deployed_notification_copies_within_its_bucket.rs"]
mod s3_deployed_notification_copies_within_its_bucket;
#[path = "all/cloudformation_asset_bucket_publishing_role.rs"]
mod cloudformation_asset_bucket_publishing_role;
// client SEES of an EventBridge rule's and an SNS subscription's deliveries to their Lambda and
// SNS targets, which spans EventBridge, SNS, Lambda, CloudWatch Logs, SQS, IAM and
#[path = "all/events_sns_lambda_delivery_observed_in_logs.rs"]
mod events_sns_lambda_delivery_observed_in_logs;
// appended tail and the sorted `c…` span (where every cloudformation suite lands) are both
// regions concurrently landing suites add their line to.
#[path = "all/cloudformation_sfn_reference_starts_execution.rs"]
mod cloudformation_sfn_reference_starts_execution;
// Declared here, in this same head region, for the reason the entries above record: a typed,
// nested PutEvents detail crossing an EventBridge rule's queue target and the Lambda event
// source mapping that consumes that queue, which spans EventBridge, Amazon SQS, IAM,
// CloudFormation, Lambda and DynamoDB at once.
#[path = "all/events_queue_target_nested_detail_reaches_its_lambda_consumer.rs"]
mod events_queue_target_nested_detail_reaches_its_lambda_consumer;
// appended tail and the sorted `r…` span are both regions concurrently landing suites add
// their line to.
#[path = "all/rds_s3_snapshot_export_flow.rs"]
mod rds_s3_snapshot_export_flow;
// appended tail and the sorted `d…` span are both regions concurrently landing suites add
#[path = "all/directory_data_plane_authorization.rs"]
mod directory_data_plane_authorization;
// Declared here, beside the suite above it shares its two services with: which DOMAIN the
// data plane answers for, which is the control plane's record of the name it was created
// with.
#[path = "all/directory_realm_follows_its_create.rs"]
mod directory_realm_follows_its_create;
// Declared here, in this same head region, for the reason the entries above record.
#[path = "all/directory_data_access_cycle.rs"]
mod directory_data_access_cycle;
#[path = "all/cloudformation_lambda_ssm_sourced_update.rs"]
mod cloudformation_lambda_ssm_sourced_update;
// appended tail and the sorted `k…` span are both regions concurrently landing suites add
#[path = "all/kinesisvideo_discovered_data_plane.rs"]
mod kinesisvideo_discovered_data_plane;
#[path = "all/lambda_sqs_event_source_batching_window.rs"]
mod lambda_sqs_event_source_batching_window;
#[path = "all/codebuild_service_role_trust.rs"]
mod codebuild_service_role_trust;
// Declared here, beside the suite above whose rule it shares: the role an app block
// builder runs as is resolved against the ACCOUNT's IAM records, so the suite spans the
// two services and belongs to neither crate's own test target.
#[path = "all/appstream_app_block_builder_role_trust.rs"]
mod appstream_app_block_builder_role_trust;
#[path = "all/codebuild_artifacts_override_bucket.rs"]
mod codebuild_artifacts_override_bucket;
#[path = "all/cloudformation_ec2_cdk_vpc_estate.rs"]
mod cloudformation_ec2_cdk_vpc_estate;
#[path = "all/greengrass_deployment_iot_job.rs"]
mod greengrass_deployment_iot_job;
#[path = "all/greengrass_deployment_target_resolution.rs"]
mod greengrass_deployment_target_resolution;
#[path = "all/cloudformation_sam_function_writes_its_table.rs"]
mod cloudformation_sam_function_writes_its_table;
// What S3's two address forms — path-style and virtual-hosted — mean to the router, which is
// the rule every cross-service suite that stages an S3 object addresses its setup calls by.
#[path = "all/s3_address_forms.rs"]
mod s3_address_forms;
#[path = "all/sns_lambda_subscription_pipeline.rs"]
mod sns_lambda_subscription_pipeline;
#[path = "all/cloudformation_refuses_s3vectors_vector_buckets.rs"]
mod cloudformation_refuses_s3vectors_vector_buckets;
#[path = "all/cloudfront_oac_kms_s3_origin_edge.rs"]
mod cloudfront_oac_kms_s3_origin_edge;
// appended tail and the sorted `a…` span (where the other apigateway suites land) are both
#[path = "all/apigateway_cognito_user_pools_authorizer.rs"]
mod apigateway_cognito_user_pools_authorizer;
#[path = "all/apigateway_iam_authorized_invoke.rs"]
mod apigateway_iam_authorized_invoke;
// appended tail and the sorted `e…` span are both regions concurrently landing suites add
#[path = "all/ebs_start_snapshot_resolves_its_kms_key.rs"]
mod ebs_start_snapshot_resolves_its_kms_key;
// appended tail and the sorted `s…` span (where every sqs suite lands) are both regions
// concurrently landing suites add their line to.
#[path = "all/sqs_queue_policy_governs_cross_service_delivery.rs"]
mod sqs_queue_policy_governs_cross_service_delivery;
// Declared here, at the head, for the reason the entry above records: the appended tail and
// the sorted `a…` span (beside the other application* suites) are both regions concurrently
// landing suites add their line to.
#[path = "all/applicationinsights_resource_group_admission.rs"]
mod applicationinsights_resource_group_admission;
// Declared at the head for the reason the entry above records: the appended tail and the
// sorted `s…` span (where the other sagemaker suites land) are both regions concurrently
#[path = "all/sagemaker_create_admission.rs"]
mod sagemaker_create_admission;
// Declared in this same head region, for the reason the entry above records: the appended
// tail and the sorted `s…` span (where the other sagemaker suites land) are both regions
#[path = "all/sagemaker_execution_role_admission.rs"]
mod sagemaker_execution_role_admission;
// Declared in this same head region, for the reason the entries above record: the appended
#[path = "all/sagemaker_pipeline_role_resolution.rs"]
mod sagemaker_pipeline_role_resolution;
// the sorted `g…` span (beside the other greengrass/glue suites) are both regions
#[path = "all/greengrass_signing_name_routing.rs"]
mod greengrass_signing_name_routing;
// the sorted `c…` span (where every cloudformation suite lands) are both regions
#[path = "all/cloudformation_deployed_domain_index_lifecycle.rs"]
mod cloudformation_deployed_domain_index_lifecycle;
#[path = "all/cloudformation_domain_access_policy_revision.rs"]
mod cloudformation_domain_access_policy_revision;
// the sorted `g…` span (beside the other glue suites) are both regions concurrently landing
// suites add their line to.
#[path = "all/glue_studio_notebook_catalog_through_the_router.rs"]
mod glue_studio_notebook_catalog_through_the_router;
// the sorted `e…` span (where every elbv2 suite lands) are both regions concurrently landing
#[path = "all/elbv2_dualstack_and_security_group_classes.rs"]
mod elbv2_dualstack_and_security_group_classes;
#[path = "all/cloudformation_provisions_codeartifact_resources.rs"]
mod cloudformation_provisions_codeartifact_resources;
// tail and the sorted `c…` span (where every cloudformation suite lands) are both regions
#[path = "all/cloudformation_provisions_glue_catalog.rs"]
mod cloudformation_provisions_glue_catalog;
// tail and the sorted `s…` span (where every ssm suite would land) are both regions
#[path = "all/ssm_securestring_ciphertext_decrypts_in_kms.rs"]
mod ssm_securestring_ciphertext_decrypts_in_kms;
// the sorted `s…` span (where every ses suite lands) are both regions concurrently landing
#[path = "all/ses_raw_send_under_a_shared_domain_identity.rs"]
mod ses_raw_send_under_a_shared_domain_identity;
// Declared here, at the head, for the reason the entries below record: the appended tail
// and the sorted `c…` span (where every codepipeline and cloudformation suite would land)
// are both regions concurrently landing suites add their line to.
#[path = "all/codepipeline_execution_controls.rs"]
mod codepipeline_execution_controls;
// tail and the sorted `c…` span (where every codepipeline suite would land) are both regions
#[path = "all/codepipeline_management_events.rs"]
mod codepipeline_management_events;
// and the sorted `i…` span are both regions concurrently landing suites add their line to.
#[path = "all/iam_management_events.rs"]
mod iam_management_events;
// tail and the sorted `s…` span are both regions concurrently landing suites add their line
// to. `sts_management_events` pins that an STS call — a read-only one included — is in
// CloudTrail event history, and that no credential material reaches a record.
#[path = "all/sts_management_events.rs"]
mod sts_management_events;
// and the sorted `a…` span are both regions concurrently landing suites add their line to.
#[path = "all/assembled_router_refusal_identities.rs"]
mod assembled_router_refusal_identities;
#[path = "all/cloudformation_apigateway_custom_domains.rs"]
mod cloudformation_apigateway_custom_domains;
#[path = "all/cloudformation_provisions_guardduty_ip_set.rs"]
mod cloudformation_provisions_guardduty_ip_set;
// Declared in this same head region, for the reason the entries above record.
#[path = "all/cloudformation_provisions_wafv2_web_acl_associations.rs"]
mod cloudformation_provisions_wafv2_web_acl_associations;
// Declared here, at the head, for the reason the entry below records: the appended tail and
// the sorted `k…` span are both regions concurrently landing suites add their line to.
#[path = "all/kinesisvideo_signing_name_routing.rs"]
mod kinesisvideo_signing_name_routing;
// the sorted `k…` span (beside the other kafka/kinesis suites) are both regions concurrently
#[path = "all/kafka_placement_subnets_resolve_in_ec2.rs"]
mod kafka_placement_subnets_resolve_in_ec2;
// the sorted `b…` span (where every batch suite would land) are both regions concurrently
#[path = "all/batch_job_state_change_is_scoped_to_its_own_job_queue.rs"]
mod batch_job_state_change_is_scoped_to_its_own_job_queue;
// Declared beside the suite above it shares its three services with: the same publication,
// on a router that verifies request signatures.
#[path = "all/batch_job_state_change_on_an_authenticating_router.rs"]
mod batch_job_state_change_on_an_authenticating_router;
// the sorted `t…` span are both regions concurrently landing suites add their line to.
#[path = "all/tagged_hosted_zone_selects_a_sending_domain.rs"]
mod tagged_hosted_zone_selects_a_sending_domain;
// the sorted `c…` span (where every codeartifact and cloudformation suite would land) are
// both regions concurrently landing suites add their line to.
#[path = "all/codeartifact_package_version_events.rs"]
mod codeartifact_package_version_events;
#[path = "all/cloudformation_opensearch_domain_access_policies.rs"]
mod cloudformation_opensearch_domain_access_policies;
#[path = "all/cloudformation_opensearch_domain_deployments.rs"]
mod cloudformation_opensearch_domain_deployments;
#[path = "all/cloudformation_opensearch_vpc_endpoint_access.rs"]
mod cloudformation_opensearch_vpc_endpoint_access;
#[path = "all/cloudformation_search_indexing_pipeline_outputs.rs"]
mod cloudformation_search_indexing_pipeline_outputs;
#[path = "all/cloudformation_ec2_ebs_instance_estate.rs"]
mod cloudformation_ec2_ebs_instance_estate;
// the sorted `k…` span (beside the other kinesis suites) are both regions concurrently
#[path = "all/kinesisanalytics_signing_name_routing.rs"]
mod kinesisanalytics_signing_name_routing;
// the sorted `s…` span (where the other s3 suites land) are both regions concurrently
#[path = "all/s3control_tags_a_general_purpose_bucket.rs"]
mod s3control_tags_a_general_purpose_bucket;
#[path = "all/cloudformation_provisions_kinesis_streams.rs"]
mod cloudformation_provisions_kinesis_streams;
// the sorted `e…` span (where the other ecr/eks suites land) are both regions concurrently
#[path = "all/ecr_public_action_scoped_iam_identity.rs"]
mod ecr_public_action_scoped_iam_identity;
// the sorted `a…` span (where the other apigateway suites land) are both regions
#[path = "all/apigateway_private_vpce_invoke.rs"]
mod apigateway_private_vpce_invoke;
#[path = "all/apigateway_s3_service_integration.rs"]
mod apigateway_s3_service_integration;
// the sorted `b…` span (where the other bedrock suites land) are both regions concurrently
#[path = "all/bedrock_model_customization_pass_role.rs"]
mod bedrock_model_customization_pass_role;
#[path = "all/bedrock_model_invocation_logging_pass_role.rs"]
mod bedrock_model_invocation_logging_pass_role;
#[path = "all/cloudformation_firehose_stream_stabilisation.rs"]
mod cloudformation_firehose_stream_stabilisation;
// Declared beside the suite above rather than in the head region: both are regions
// concurrently landing suites add their line to, and this one pins the other half of that
// suite's rule — a stack operation is not reported complete while a resource it DELETED is
// still held by the service that owned it.
#[path = "all/cloudformation_resource_removal_stabilisation.rs"]
mod cloudformation_resource_removal_stabilisation;
#[path = "all/cloudformation_provisions_opensearch_domains.rs"]
mod cloudformation_provisions_opensearch_domains;
#[path = "all/cloudformation_provisions_glue_jobs.rs"]
mod cloudformation_provisions_glue_jobs;
#[path = "all/cloudformation_glue_job_redeploy_across_regions.rs"]
mod cloudformation_glue_job_redeploy_across_regions;
#[path = "all/cloudformation_provisions_backup_resources.rs"]
mod cloudformation_provisions_backup_resources;
#[path = "all/cloudformation_provisions_route53_health_checks.rs"]
mod cloudformation_provisions_route53_health_checks;
#[path = "all/cloudformation_provisions_kms_aliases.rs"]
mod cloudformation_provisions_kms_aliases;
// Declared beside the entry above, at the head, for the same reason: the appended tail and
// the sorted `c…` span (where the other cloudwatch/cloudformation suites land) are both
#[path = "all/cloudwatch_alarm_action_reaches_its_subscribed_queue.rs"]
mod cloudwatch_alarm_action_reaches_its_subscribed_queue;
#[path = "all/cloudwatch_breaching_alarm_fires_through_the_whole_router.rs"]
mod cloudwatch_breaching_alarm_fires_through_the_whole_router;
// Declared beside the entry above, at the head, for the same reason.
#[path = "all/cloudwatch_composite_alarm_rule_decides_its_sns_action.rs"]
mod cloudwatch_composite_alarm_rule_decides_its_sns_action;
// the sorted `c…` span (where the other cloudwatch suites land) are both regions concurrently
#[path = "all/contributor_insights_log_events.rs"]
mod contributor_insights_log_events;
// the sorted `c…` span (where every cloudformation suite lands) are both regions concurrently
#[path = "all/cloudformation_records_per_resource_stack_events.rs"]
mod cloudformation_records_per_resource_stack_events;
// the sorted `i…` span (where the other iam suites land) are both regions concurrently
#[path = "all/iam_holds_the_principal_sts_reports.rs"]
mod iam_holds_the_principal_sts_reports;
// the sorted `s…` span are both regions concurrently landing suites add their line to.
#[path = "all/sts_does_not_disclose_an_absent_role.rs"]
mod sts_does_not_disclose_an_absent_role;
// Declared here, at the head, for the reason the entry above records: the appended tail
// and the sorted `s…` span are both regions concurrent suites add their line to.
#[path = "all/signer_revocation_status_across_services.rs"]
mod signer_revocation_status_across_services;
// the sorted `e…` span (where the other ecs/elbv2 suites land) are both regions concurrently
#[path = "all/ecs_awsvpc_configuration_over_an_ec2_network.rs"]
mod ecs_awsvpc_configuration_over_an_ec2_network;
// the sorted `l…` span are both regions concurrently landing suites add their line to.
#[path = "all/log_alarm_scheduled_query.rs"]
mod log_alarm_scheduled_query;
// the sorted `c…` span (where every cloudformation suite lands) are both regions concurrent
#[path = "all/cloudformation_provisions_codebuild_report_groups.rs"]
mod cloudformation_provisions_codebuild_report_groups;
#[path = "all/cloudformation_provisions_codecommit_repositories.rs"]
mod cloudformation_provisions_codecommit_repositories;
// and the sorted `c…` span (where every cloudformation suite lands) are both regions
// concurrent suites add their line to.
#[path = "all/cloudformation_provisions_transfer_servers.rs"]
mod cloudformation_provisions_transfer_servers;
// Declared beside the entry above, at the head, for the same reason: the appended tail
#[path = "all/cloudformation_provisions_batch_resources.rs"]
mod cloudformation_provisions_batch_resources;
// and the sorted `s…` span are both regions concurrent suites land in.
#[path = "all/ses_family_signing_name_routing.rs"]
mod ses_family_signing_name_routing;
// Declared beside its sibling suite, at the head, for the reason the entries above record:
// the appended tail and the sorted `s…` span are both regions concurrently landing suites
// add their line to.
#[path = "all/ses_event_destination_setup_across_services.rs"]
mod ses_event_destination_setup_across_services;
// Declared beside its two sibling suites, at the head, for the reason the entries above
// record: the appended tail and the sorted `s…` span are both regions concurrently landing
#[path = "all/ses_firehose_event_destination_setup.rs"]
mod ses_firehose_event_destination_setup;
// Declared beside its sibling ses suites, at the head, for the reason the entries above
#[path = "all/ses_v1_receipt_rule_sets_through_the_router.rs"]
mod ses_v1_receipt_rule_sets_through_the_router;
#[path = "all/ses_identities_shared_across_v1_and_v2.rs"]
mod ses_identities_shared_across_v1_and_v2;
#[path = "all/ses_easy_dkim_shared_across_surfaces.rs"]
mod ses_easy_dkim_shared_across_surfaces;
#[path = "all/ses_identity_policies_shared_across_surfaces.rs"]
mod ses_identity_policies_shared_across_surfaces;
// Declared beside the suites above, at the head, for the same reason: the appended tail and
#[path = "all/cloudformation_provisions_ses_configuration_sets.rs"]
mod cloudformation_provisions_ses_configuration_sets;
// the appended tail and the sorted `c…` span (where every cloudformation suite lands) are
#[path = "all/cloudformation_provisions_ses_receipt_rule_sets.rs"]
mod cloudformation_provisions_ses_receipt_rule_sets;
// At the head for the same reason, and moved here out of the `c…` span of the sorted
// list: that span is where the cloudformation/cloudtrail suites land, so a line beside
// them lost repeated squash merges to branches that touched it in the same window.
#[path = "all/chime_signing_name_routing.rs"]
mod chime_signing_name_routing;
// Out of order for the same reason, and placed beside it rather than in the sorted span
// of its own name or in the appended tail, both of which concurrent suites land into.
#[path = "all/cognito_vended_session_reads_sse_kms_object.rs"]
mod cognito_vended_session_reads_sse_kms_object;
// Declared beside the suite above, at the head, for the same reason it is here.
#[path = "all/cognito_vended_session_is_audited.rs"]
mod cognito_vended_session_is_audited;
// Declared beside its two sibling cognito suites, at the head, for the same reason they are
// here: the appended tail and the sorted `c…` span are both regions concurrent suites land in.
#[path = "all/cognito_vended_session_is_an_assumed_role_caller.rs"]
mod cognito_vended_session_is_an_assumed_role_caller;
// Declared beside its sibling cognito suites, at the head, for the same reason they are
#[path = "all/cognito_post_confirmation_lambda_trigger.rs"]
mod cognito_post_confirmation_lambda_trigger;
#[path = "all/cognito_group_role_mapping.rs"]
mod cognito_group_role_mapping;
#[path = "all/cognito_sms_caller_role_trust.rs"]
mod cognito_sms_caller_role_trust;
// here: whether a session already vended survives the teardown of the pool that vended it
// spans Amazon Cognito Identity, IAM, STS, Amazon S3 and the emulator's own credential gate.
#[path = "all/cognito_vended_session_outlives_pool_deletion.rs"]
mod cognito_vended_session_outlives_pool_deletion;
#[path = "all/pi_resource_metadata_reflects_rds_configuration.rs"]
mod pi_resource_metadata_reflects_rds_configuration;
// At the head for the reason the entries above record: the appended tail and the sorted
// `e…` span (where the other events suites land) are both regions concurrent suites add
#[path = "all/events_fan_out_to_each_matched_target.rs"]
mod events_fan_out_to_each_matched_target;
// Out of order for the same reason as the two entries above it.
#[path = "all/redshift_cluster_iam_role_poll_lifecycle.rs"]
mod redshift_cluster_iam_role_poll_lifecycle;
// At the head for the same reason: the `c…` span of the sorted list is where the
// cloudformation suites land, and the appended tail is where every concurrent suite does.
#[path = "all/cloudformation_updates_iam_roles.rs"]
mod cloudformation_updates_iam_roles;
// At the head for the same reason as the entry above: the `c…` span of the sorted list is
// where the cloudformation suites land, and the appended tail is where every concurrent
// suite does.
#[path = "all/cloudformation_apigatewayv2_api_children.rs"]
mod cloudformation_apigatewayv2_api_children;
#[path = "all/cloudformation_apigatewayv2_resources.rs"]
mod cloudformation_apigatewayv2_resources;
#[path = "all/amplifybackend_app_registration.rs"]
mod amplifybackend_app_registration;
#[path = "all/apigateway_signing_name_routing.rs"]
mod apigateway_signing_name_routing;
#[path = "all/appconfig_signing_name_routing.rs"]
mod appconfig_signing_name_routing;
#[path = "all/aws_marketplace_signing_name_routing.rs"]
mod aws_marketplace_signing_name_routing;
// These sit in the SORTED block rather than being appended below, deliberately: the
// appended region at the end of this file is where every concurrent suite lands, so a
// line added there conflicts with every other suite in the same window. Here the
// neighbouring lines are stable.
#[path = "all/bedrock_agentcore_execution_role.rs"]
mod bedrock_agentcore_execution_role;
#[path = "all/bedrock_agentcore_gateway_lambda_target.rs"]
mod bedrock_agentcore_gateway_lambda_target;
#[path = "all/bedrock_agentcore_gateway_mcp_endpoint.rs"]
mod bedrock_agentcore_gateway_mcp_endpoint;
#[path = "all/bedrock_agentcore_signing_name_routing.rs"]
mod bedrock_agentcore_signing_name_routing;
#[path = "all/bedrock_agent_runtime_session_authorization.rs"]
mod bedrock_agent_runtime_session_authorization;
#[path = "all/bedrock_signing_name_routing.rs"]
mod bedrock_signing_name_routing;
#[path = "all/billingconductor_payer_account.rs"]
mod billingconductor_payer_account;
#[path = "all/boot_serve.rs"]
mod boot_serve;
// Declared in the sorted list rather than appended at the end of this file: the tail is
// the contended region every other new router suite lands in, and a declaration here
// merges cleanly beside them.
#[path = "all/cfn_provisions_declared_resources.rs"]
mod cfn_provisions_declared_resources;
// Declared HERE, in the sorted list, for the same reason.
#[path = "all/chime_family_signing_name_routing.rs"]
mod chime_family_signing_name_routing;
// Declared in this quiet span of the sorted list, for the reason the notes below give.
#[path = "all/cloudformation_canceled_delete_keeps_exported_resources.rs"]
mod cloudformation_canceled_delete_keeps_exported_resources;
#[path = "all/cloudformation_cdk_bucket_notifications_custom_resource.rs"]
mod cloudformation_cdk_bucket_notifications_custom_resource;
// Declared in this span of the sorted list rather than appended at the end of this file,
// for the reason the notes here give: the tail is the region every concurrently landing
// suite appends to.
#[path = "all/cloudformation_provisions_acm_acme_endpoints.rs"]
mod cloudformation_provisions_acm_acme_endpoints;
// Declared in this same quiet span, for the reason the note above gives.
#[path = "all/cloudformation_provisions_acm_certificates.rs"]
mod cloudformation_provisions_acm_certificates;
#[path = "all/cloudformation_activates_acmpca_certificate_authorities.rs"]
mod cloudformation_activates_acmpca_certificate_authorities;
#[path = "all/cloudformation_admits_acmpca_resource_types.rs"]
mod cloudformation_admits_acmpca_resource_types;
#[path = "all/cloudformation_provisions_acmpca_certificate_authorities.rs"]
mod cloudformation_provisions_acmpca_certificate_authorities;
#[path = "all/cloudformation_provisions_codedeploy_resources.rs"]
mod cloudformation_provisions_codedeploy_resources;
// Declared in this same quiet span of the sorted list, for the reason the note above
// gives: the tail of this file and the `c…` span beside the other cloudformation suites
// are both regions concurrently landing suites put their line in.
#[path = "all/cloudformation_provisions_eks_access_entries.rs"]
mod cloudformation_provisions_eks_access_entries;
// Declared in this same quiet span of the sorted list, for the reason the note above gives:
// a stack that declares an `AWS::EKS::Cluster` or an `AWS::EKS::PodIdentityAssociation`
// really creates it in Amazon EKS, holds the cluster's creation open until that service
// reports it ACTIVE, and reports what it minted in the stack's own outputs.
#[path = "all/cloudformation_provisions_eks_clusters.rs"]
mod cloudformation_provisions_eks_clusters;
#[path = "all/cloudformation_provisions_cognito_user_pool_groups.rs"]
mod cloudformation_provisions_cognito_user_pool_groups;
// the tail of this file is where every concurrently landing router suite appends.
#[path = "all/cloudformation_provisions_cloudwatch_dashboards.rs"]
mod cloudformation_provisions_cloudwatch_dashboards;
#[path = "all/cloudformation_provisions_sagemaker_model_package_group.rs"]
mod cloudformation_provisions_sagemaker_model_package_group;
#[path = "all/cloudformation_provisions_sagemaker_notebook_estate.rs"]
mod cloudformation_provisions_sagemaker_notebook_estate;
// for the reason the notes above give: the tail is where every concurrently landing router
// suite appends, so a line added there conflicts with all of them.
#[path = "all/cloudformation_provisions_scheduler_schedules.rs"]
mod cloudformation_provisions_scheduler_schedules;
#[path = "all/cloudformation_provisions_stack_resources.rs"]
mod cloudformation_provisions_stack_resources;
#[path = "all/cloudfront_api_call_events.rs"]
mod cloudfront_api_call_events;
// Declared HERE, in the sorted list beside the other CloudFront suites, rather than
// appended at the end of this file: the tail is where every suite landing in the same
// window appends, so a line added there conflicts with all of them.
#[path = "all/cloudfront_apigateway_origin_edge.rs"]
mod cloudfront_apigateway_origin_edge;
#[path = "all/cloudfront_edge_cache_refresh.rs"]
mod cloudfront_edge_cache_refresh;
#[path = "all/cloudfront_key_value_store_handshake.rs"]
mod cloudfront_key_value_store_handshake;
// Declared HERE, in the sorted list beside the other CloudFront suite, rather than appended
// at the end of this file: the tail is where every suite landing in the same window appends,
// so a line added there conflicts with all of them.
#[path = "all/cloudfront_oac_s3_origin_edge.rs"]
mod cloudfront_oac_s3_origin_edge;
// Declared HERE, in the sorted list beside the other CloudFront suites, rather than appended
#[path = "all/cloudfront_oai_s3_origin_edge.rs"]
mod cloudfront_oai_s3_origin_edge;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every suite landing in the same window appends, so it is the contended
// region, while this span of the list is stable.
#[path = "all/cloudhsm_signing_name_routing.rs"]
mod cloudhsm_signing_name_routing;
// Declared here, in the SORTED block rather than appended at the end: the tail is where
// every suite landing in the same window appends, so it is the contended region, while
// this span of the list is stable.
#[path = "all/cloudtrail_api_call_events_by_region.rs"]
mod cloudtrail_api_call_events_by_region;
#[path = "all/cloudtrail_channel_target_routing.rs"]
mod cloudtrail_channel_target_routing;
// In the SORTED block, not appended at the end, for the reason stated above: the end of
// this file is where every concurrent suite lands.
#[path = "all/cloudtrail_channel_routing.rs"]
mod cloudtrail_channel_routing;
// region, while this span of the sorted list is untouched by those appends.
#[path = "all/cloudtrail_import_failures_routing.rs"]
mod cloudtrail_import_failures_routing;
// Declared here, in the SORTED block beside the other CloudTrail suites rather than
// appended at the end: the tail is where every suite landing in the same window appends.
#[path = "all/cloudtrail_read_only_api_calls.rs"]
mod cloudtrail_read_only_api_calls;
// tail is where every suite landing in the same window appends, so a line added there
// conflicts with every other one; this span of the sorted list is untouched by those
// appends.
#[path = "all/config_compliance_change_event.rs"]
mod config_compliance_change_event;
#[path = "all/connect_signing_name_routing.rs"]
mod connect_signing_name_routing;
// Declared HERE, in sorted position beside the other DynamoDB suite rather than appended
// at the end of this file: the tail is where every suite landing in the same window
// appends, so it is the contended region, while these neighbouring lines are stable.
#[path = "all/ddb_stream_lambda_esm_provisioning.rs"]
mod ddb_stream_lambda_esm_provisioning;
#[path = "all/dynamodb_data_plane_authorization.rs"]
mod dynamodb_data_plane_authorization;
#[path = "all/dynamodb_session_policy_authorization.rs"]
mod dynamodb_session_policy_authorization;
#[path = "all/dynamodb_streams_routing.rs"]
mod dynamodb_streams_routing;
// tail is where every landing in the same window appends, so it is the contended region,
// while this span of the sorted list is untouched by those appends.
#[path = "all/ecr_repository_policy_principal.rs"]
mod ecr_repository_policy_principal;
#[path = "all/ecs_service_target_group_association.rs"]
mod ecs_service_target_group_association;
// Declared HERE, in the sorted list, for the reason the note above gives: the tail of this
// file is the region every concurrent landing appends to.
#[path = "all/ecs_service_target_group_health_check_port.rs"]
mod ecs_service_target_group_health_check_port;
// Declared HERE, in the sorted list, for the reason the note above gives.
#[path = "all/ecs_service_target_group_security_groups.rs"]
mod ecs_service_target_group_security_groups;
// Declared HERE, beside the sibling ECS/ELBv2 suites, for the reason the notes above give:
// the tail of this file is the region every concurrent landing appends to.
#[path = "all/ecs_task_registration_precedes_its_health_verdict.rs"]
mod ecs_task_registration_precedes_its_health_verdict;
// Declared HERE, in the sorted list, rather than appended at the end of this file like
// the most recent suites: the tail is where every landing in the same window appends, so
// it is the contended region now, while this span of the sorted list is untouched by
// those appends.
#[path = "all/elb_classic_and_v2_signing_name_routing.rs"]
mod elb_classic_and_v2_signing_name_routing;
// Declared in this span of the sorted list for the reason the note above gives: the tail
// of this file is where every concurrent router suite appends, so a line added there
// conflicts with every other landing in the same window.
#[path = "all/elbv2_network_resolution.rs"]
mod elbv2_network_resolution;
// Declared beside the suite it is the sibling of, in this span of the sorted list, for the
// same reason: the tail of this file is where every concurrently landing router suite
// appends its line.
#[path = "all/elbv2_ip_target_refusal_classes.rs"]
mod elbv2_ip_target_refusal_classes;
// Declared beside its sibling suites, in this span of the sorted list rather than appended
// at the tail of this file, for the reason the notes above give.
#[path = "all/elbv2_subnet_and_elastic_ip_resolution.rs"]
mod elbv2_subnet_and_elastic_ip_resolution;
// Declared beside the sibling suite above, in this span of the sorted list rather than
// appended at the tail of this file, for the reason the notes above give.
#[path = "all/elbv2_subnet_vpc_membership.rs"]
mod elbv2_subnet_vpc_membership;
// Declared beside its sibling ELBv2 suites, in this span of the sorted list rather than
#[path = "all/elbv2_certificate_in_use_by_acm.rs"]
mod elbv2_certificate_in_use_by_acm;
#[path = "all/elbv2_load_balancer_security_groups.rs"]
mod elbv2_load_balancer_security_groups;
#[path = "all/elbv2_zonal_shift_managed_resource.rs"]
mod elbv2_zonal_shift_managed_resource;
// Declared beside the Classic/ELBv2 suites above, in this span of the sorted list rather
// than in the appended tail, for the same reason: the tail is where every concurrently
// landing router suite puts its line.
#[path = "all/elb_classic_zone_membership_follows_its_subnets.rs"]
mod elb_classic_zone_membership_follows_its_subnets;
// Declared beside its sibling Classic suite above, in this span of the sorted list rather
// than in the appended tail, for the same reason the notes above give.
#[path = "all/elb_classic_network_resources_resolve_in_ec2.rs"]
mod elb_classic_network_resources_resolve_in_ec2;
// Declared beside its sibling Classic suites above, in this span of the sorted list rather
#[path = "all/elb_classic_zone_enable_turns_on_the_vpc.rs"]
mod elb_classic_zone_enable_turns_on_the_vpc;
// Alphabetically between the `elbv2_` and `es_` suites, rather than appended at the end
// of this file, which is where every new suite lands and therefore where two changes
// collide.
#[path = "all/emr_job_flow_role_instance_profile.rs"]
mod emr_job_flow_role_instance_profile;
// Declared HERE too, in the sorted list rather than appended at the tail, for the reason
// the note above gives: the appended region at the end of this file is where every
// concurrent router suite lands, so a line added there conflicts with every other suite
// in the same window — this fix lost four serialized squash merges to exactly that. These
#[path = "all/es_opensearch_and_elasticsearch_signing_name_routing.rs"]
mod es_opensearch_and_elasticsearch_signing_name_routing;
#[path = "all/events_signing_name_routing.rs"]
mod events_signing_name_routing;
#[path = "all/events_store_ownership.rs"]
mod events_store_ownership;
#[path = "all/events_to_sfn_to_dynamodb_chain.rs"]
mod events_to_sfn_to_dynamodb_chain;
// In the sorted list rather than appended at the end of the file: every new suite appends
// there, so that is the contended region now and the sorted position this file's header
// asks for is the quiet one.
#[path = "all/firehose_destination_role.rs"]
mod firehose_destination_role;
// Declared HERE, beside the trust suite it is the sibling of, rather than appended at the
// end of this file: the tail is where every landing in the same window appends, so it is
// the contended region, while this span of the sorted list is untouched by those appends.
#[path = "all/fis_execution_role_permissions.rs"]
mod fis_execution_role_permissions;
#[path = "all/fis_execution_role_trust.rs"]
mod fis_execution_role_trust;
#[path = "all/forecast_signing_name_routing.rs"]
mod forecast_signing_name_routing;
#[path = "all/glue_jobs_are_regional_while_their_role_is_global.rs"]
mod glue_jobs_are_regional_while_their_role_is_global;
// Declared beside the other Glue suite rather than at the head or the tail of this file:
// both of those are where every concurrently landing suite puts its line.
#[path = "all/sfn_glue_run_a_job_sync.rs"]
mod sfn_glue_run_a_job_sync;
// Declared beside the other Step Functions suite, for the reason the lines below give:
// the head and the tail of this file are where every concurrently landing suite puts its
// line, so a declaration there conflicts with all of them.
#[path = "all/sfn_task_target_authorization.rs"]
mod sfn_task_target_authorization;
// Declared beside the other Step Functions suites, for the reason the lines above give.
#[path = "all/sfn_lambda_task_invocation.rs"]
mod sfn_lambda_task_invocation;
#[path = "all/sfn_integration_error_names.rs"]
mod sfn_integration_error_names;
#[path = "all/sfn_sns_publish_task.rs"]
mod sfn_sns_publish_task;
#[path = "all/sfn_parallel_branch_cancellation.rs"]
mod sfn_parallel_branch_cancellation;
#[path = "all/cloudformation_sfn_fifo_queue_stack.rs"]
mod cloudformation_sfn_fifo_queue_stack;
#[path = "all/sfn_management_events.rs"]
mod sfn_management_events;
// Declared HERE, in the sorted list at its own name, rather than at the head or the tail
// of this file: both of those are where every concurrently landing suite puts its line, so
// a declaration there conflicts with all of them, while these neighbouring lines are
// stable.
#[path = "all/guardduty_runtime_coverage.rs"]
mod guardduty_runtime_coverage;
// tail is where every concurrently landing router suite appends, so a line added there
// conflicts with all of them, while these neighbouring lines are stable.
#[path = "all/sagemaker_feature_store_control_plane_handoff.rs"]
mod sagemaker_feature_store_control_plane_handoff;
// Declared beside the hand-off suite whose harness it drives, rather than appended at the
// end of this file, for the same reason the line above gives.
#[path = "all/sagemaker_feature_store_group_deletion.rs"]
mod sagemaker_feature_store_group_deletion;
// end of this file, for the same reason the two lines above give.
#[path = "all/sagemaker_feature_store_default_ttl.rs"]
mod sagemaker_feature_store_default_ttl;
// Declared beside the feature-store hand-off suite it is the counterpart of, rather than
// appended at the end of this file, for the same reason the line above gives: the tail is
// where every concurrently landing router suite appends its line.
#[path = "all/sagemaker_agentic_rft_job_runtime_handoff.rs"]
mod sagemaker_agentic_rft_job_runtime_handoff;
#[path = "all/iam_and_signer_signing_name_routing.rs"]
mod iam_and_signer_signing_name_routing;
#[path = "all/iam_role_id_agrees_with_sts.rs"]
mod iam_role_id_agrees_with_sts;
// Declared beside the role-side suite it is the counterpart of, in the sorted list rather
// than appended at the end of this file: the tail is where every concurrently landing
// router suite puts its line, so a declaration there conflicts with all of them, while
// these neighbouring lines are stable.
#[path = "all/iam_access_key_signs_as_its_user.rs"]
mod iam_access_key_signs_as_its_user;
#[path = "all/iam_authorizes_an_assumed_role_session.rs"]
mod iam_authorizes_an_assumed_role_session;
#[path = "all/iam_session_policy_scopes_an_assumed_role.rs"]
mod iam_session_policy_scopes_an_assumed_role;
#[path = "all/iam_session_refusal_audit_documents.rs"]
mod iam_session_refusal_audit_documents;
#[path = "all/iam_signing_name_routing.rs"]
mod iam_signing_name_routing;
#[path = "all/iot_command_deprecation_is_observable_on_the_data_plane.rs"]
mod iot_command_deprecation_is_observable_on_the_data_plane;
#[path = "all/iot_command_execution_lifecycle.rs"]
mod iot_command_execution_lifecycle;
// Declared HERE, beside the command-execution suite it is the sibling of, rather than
// window appends, so it is the contended region, while these neighbouring lines are
#[path = "all/iot_command_executions_are_listed_across_planes.rs"]
mod iot_command_executions_are_listed_across_planes;
#[path = "all/iot_job_execution_reaches_the_device.rs"]
mod iot_job_execution_reaches_the_device;
#[path = "all/iot_jobs_data_endpoint_discovery.rs"]
mod iot_jobs_data_endpoint_discovery;
#[path = "all/ivs_signing_name_routing.rs"]
mod ivs_signing_name_routing;
// Declared here, between two unrelated spans, rather than at the head of this file, in its
// appended tail or in the `a…` span of its own name: all three are where concurrently
// landing suites put their line, so a declaration in any of them loses a squash merge to
// every one of them, while this boundary's neighbouring lines are stable.
#[path = "all/apigatewayv2_cognito_jwt_authorizer.rs"]
mod apigatewayv2_cognito_jwt_authorizer;
#[path = "all/keyspaces_signing_name_routing.rs"]
mod keyspaces_signing_name_routing;
#[path = "all/keyspaces_streams_routing.rs"]
mod keyspaces_streams_routing;
#[path = "all/lambda_bundled_wire_operations.rs"]
mod lambda_bundled_wire_operations;
// Declared in this span of the sorted list, beside the other `lambda…` suites, rather than
// at the head of this file or in its appended tail: both of those are where concurrently
// landing suites put their line, so a declaration there loses a squash merge to every one
// of them, while these neighbouring lines are stable.
#[path = "all/lambda_handler_invalidates_its_distribution.rs"]
mod lambda_handler_invalidates_its_distribution;
#[path = "all/lambda_signing_name_routing.rs"]
mod lambda_signing_name_routing;
#[path = "all/lambda_execution_role_trust.rs"]
mod lambda_execution_role_trust;
// Declared in this span of the sorted list, for the reason the line above states.
#[path = "all/lambda_capacity_provider_operator_role.rs"]
mod lambda_capacity_provider_operator_role;
#[path = "all/lambda_execution_role_session_identity.rs"]
mod lambda_execution_role_session_identity;
// Declared in this span of the sorted list, for the reason the lines above state.
#[path = "all/lambda_execution_role_search_domain_grant.rs"]
mod lambda_execution_role_search_domain_grant;
// Declared in this span of the sorted list, beside the other `l…` suites, rather than at
// the head of this file or in its appended tail: both of those are where concurrently
#[path = "all/lakeformation_catalog_resource_resolution.rs"]
mod lakeformation_catalog_resource_resolution;
// tail is where every concurrently landing router suite appends its line, so a
// declaration placed there conflicts with every one of them. (The build script requires
// every suite under `tests/all/` to be declared in this file, so it cannot live anywhere
// else.) These neighbouring lines are stable.
#[path = "all/lex_runtime_v2_sessions_through_the_router.rs"]
mod lex_runtime_v2_sessions_through_the_router;
#[path = "all/marketplace_signing_name_routing.rs"]
mod marketplace_signing_name_routing;
// Out of alphabetical order on purpose: the `c` span of this list, its end, and the
// appended region at the bottom of the file are where concurrently landing suites insert
// their own line, and a line next to theirs cannot be merged with them. This `n`/`o` gap
// is the quietest span in the file.
#[path = "all/chime_app_instance_admin_authorization.rs"]
mod chime_app_instance_admin_authorization;
// Declared in this same quiet `n`/`o` gap, beside the other cross-service Chime suite and
// for the reason the entry above records.
#[path = "all/chime_identity_name_projection.rs"]
mod chime_identity_name_projection;
#[path = "all/mediaconvert_input_probe.rs"]
mod mediaconvert_input_probe;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail, the head
// region or the `c…` span: all three are where concurrently landing suites put their line,
// and a line next to theirs cannot be merged with them.
#[path = "all/compute_optimizer_automation_denial_principal_agrees_with_sts.rs"]
mod compute_optimizer_automation_denial_principal_agrees_with_sts;
// Declared HERE, in this quiet span of the sorted list, rather than appended at the end of
// this file or beside the other `cloudformation_*` suites in the `c` span: both of those are
// where every concurrently landing suite puts its line, so a declaration there conflicts
// with all of them.
#[path = "all/cloudformation_ec2_network_identifiers.rs"]
mod cloudformation_ec2_network_identifiers;
// Declared HERE, beside the sibling suite above in the same quiet span, for the reason that
// entry records: the appended tail at the end of this file and the `c…` span beside the other
// `cloudformation_*` suites are both where every concurrently landing suite puts its line.
#[path = "all/cloudformation_ec2_network_stack_routes.rs"]
mod cloudformation_ec2_network_stack_routes;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail or in the
// `c…` span beside the other `cloudformation_provisions_*` suites: both of those are where
// every concurrently landing suite puts its line.
#[path = "all/cloudformation_provisions_wafv2_entities.rs"]
mod cloudformation_provisions_wafv2_entities;
#[path = "all/cloudformation_provisions_codepipeline_pipelines.rs"]
mod cloudformation_provisions_codepipeline_pipelines;
// Declared HERE, in this quiet `n`/`o` gap rather than in the appended tail or beside the
// other `eks_*` suite: both of those are where concurrently landing suites put their line.
// `eks_wire` is this test crate's shared EKS request harness, not a suite of its own.
#[path = "all/eks_wire.rs"]
mod eks_wire;
#[path = "all/eks_cluster_security_group.rs"]
mod eks_cluster_security_group;
// Declared HERE, beside the sibling suite above in the same quiet `n`/`o` gap, for the
// reason that entry records: the appended tail at the end of this file and the `e…` span
// beside the other `eks_*` suites are both regions every concurrently landing router suite
// puts its line in.
#[path = "all/eks_wrapped_peer_refusals.rs"]
mod eks_wrapped_peer_refusals;
// Declared HERE, beside the sibling suites above in the same quiet `n`/`o` gap, for the
// reason their entries record: the appended tail at the end of this file and the `e…` span
#[path = "all/eks_bootstrap_cluster_creator_access_entry.rs"]
mod eks_bootstrap_cluster_creator_access_entry;
#[path = "all/eks_capability_role_trust.rs"]
mod eks_capability_role_trust;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail at the end
// of this file or beside the sibling `logs_emf_*` suite in it: both of those are where every
// concurrently landing router suite puts its line.
#[path = "all/logs_emf_disabled_errors.rs"]
mod logs_emf_disabled_errors;
// Declared HERE, beside the sibling suite above in the same quiet `n`/`o` gap, for the reason
// that entry records: the appended tail at the end of this file and the head of the list are
// both regions every concurrently landing router suite puts its line in.
#[path = "all/logs_emf_extraction_policy_scope.rs"]
mod logs_emf_extraction_policy_scope;
// Declared HERE, beside the two sibling suites above in the same quiet `n`/`o` gap, for the
// reason their entries record: the appended tail at the end of this file and the head of the
// list are both regions every concurrently landing router suite puts its line in.
#[path = "all/logs_emf_declared_projections.rs"]
mod logs_emf_declared_projections;
// Declared HERE, beside the sibling `logs_*` suites in the same quiet `n`/`o` gap, for the
// reason those entries record: the appended tail at the end of this file and the head of the
#[path = "all/logs_export_task_destination_bucket.rs"]
mod logs_export_task_destination_bucket;
#[path = "all/logs_subscription_delivery_role.rs"]
mod logs_subscription_delivery_role;
#[path = "all/partnercentral_channel_target_routing.rs"]
mod partnercentral_channel_target_routing;
#[path = "all/personalize_signing_name_routing.rs"]
mod personalize_signing_name_routing;
// Declared HERE, in this quiet span of the sorted list, for the reason the entries around it
// record: the appended tail at the end of this file and the head of the list are both regions
// every concurrently landing router suite puts its line in.
#[path = "all/pcs_compute_node_group_instance_profile.rs"]
mod pcs_compute_node_group_instance_profile;
// Declared HERE, beside the sibling Personalize suite, for the reason the entries around it
#[path = "all/personalize_ingestion_targets.rs"]
mod personalize_ingestion_targets;
// Declared HERE, in this quiet span of the sorted list, for the reason the entries around
// it record: the appended tail at the end of this file and the head of the list are both
// regions every concurrently landing router suite puts its line in.
#[path = "all/payment_cryptography_signing_name_routing.rs"]
mod payment_cryptography_signing_name_routing;
// Declared HERE, in this same quiet span of the sorted list, for the reason the entries
// around it record: the appended tail at the end of this file and the HEAD of the list are
// both regions every concurrently landing router suite puts its line in, so a declaration in
// either collides with all of them.
#[path = "all/pinpoint_sms_voice_signing_name_routing.rs"]
mod pinpoint_sms_voice_signing_name_routing;
// both regions every concurrently landing router suite puts its line in. This one pins the
// FORM each service names a request by, which only an assembled router can show.
#[path = "all/per_service_response_request_id_form.rs"]
mod per_service_response_request_id_form;
// this file or beside the other `cloudformation_provisions_*` suites in the `c` span: both
// of those are where every concurrently landing suite puts its line, so a declaration there
// conflicts with all of them.
#[path = "all/cloudformation_provisions_redshift_clusters.rs"]
mod cloudformation_provisions_redshift_clusters;
#[path = "all/cloudformation_provisions_route53resolver_rules.rs"]
mod cloudformation_provisions_route53resolver_rules;
// Declared HERE, beside the sibling Route 53 Resolver suite in the same quiet span, for the
// reason that entry records: the appended tail at the end of this file and the `c` span are
#[path = "all/cloudformation_provisions_route53resolver_firewall.rs"]
mod cloudformation_provisions_route53resolver_firewall;
// Declared HERE, beside the sibling CloudFormation provisioning suites in the same quiet
// span, for the reason those entries record: the appended tail at the end of this file and
// the `c` span are both regions every concurrently landing router suite puts its line in.
#[path = "all/cloudformation_provisions_xray_sampling_rules.rs"]
mod cloudformation_provisions_xray_sampling_rules;
// `query_wire` and `rds_query_front_door` are declared HERE, in the sorted list,
// rather than appended at the end: the end of this file is where every new router
// suite lands, so a line added there collides with every other suite landing in the
// same window. Their own files are ordinary `tests/all/<name>.rs` suites.
#[path = "all/query_wire.rs"]
mod query_wire;
#[path = "all/rds_docdb_neptune_signing_name_routing.rs"]
mod rds_docdb_neptune_signing_name_routing;
// Declared beside its `rds_*` siblings, in sorted position: this is the quiet span the
// comment above names, and not the tail every concurrently landing suite appends to.
#[path = "all/rds_management_events.rs"]
mod rds_management_events;
// Declared beside its sibling management-event suite, in the same quiet span and for the
// same reason: the head of this file and its appended tail are both where concurrently
// landing suites add their line. This one pins that a CloudFormation call — accepted and
// refused alike — is in CloudTrail event history.
#[path = "all/cloudformation_management_events.rs"]
mod cloudformation_management_events;
#[path = "all/rds_query_front_door.rs"]
mod rds_query_front_door;
#[path = "all/rds_shared_query_front_door_routing.rs"]
mod rds_shared_query_front_door_routing;
#[path = "all/rds_shared_query_surface_routing.rs"]
mod rds_shared_query_surface_routing;
#[path = "all/rds_shared_signing_name_routing.rs"]
mod rds_shared_signing_name_routing;
#[path = "all/rds_signing_name_routing.rs"]
mod rds_signing_name_routing;
#[path = "all/resiliencehub_resource_mapping_existence.rs"]
mod resiliencehub_resource_mapping_existence;
#[path = "all/resiliencehub_signing_name_routing.rs"]
mod resiliencehub_signing_name_routing;
// Declared HERE, in the sorted list at its own name, rather than appended at the end of
// this file: the tail is where every concurrently landing router suite appends its line, so
// a declaration there conflicts with every one of them, while these neighbouring lines are
#[path = "all/route53_health_check_status_metric.rs"]
mod route53_health_check_status_metric;
// Declared beside the suite above for the same reason its own comment gives: this is a
// quiet span of the sorted list, while the tail of this file and the `c` span are where
#[path = "all/cloudformation_provisions_route53_hosted_zones.rs"]
mod cloudformation_provisions_route53_hosted_zones;
// Declared beside the suite above for the reason its own comment gives: a quiet span of the
// sorted list, rather than the tail of this file where every landing suite appends.
#[path = "all/cloudformation_provisions_route53_policies.rs"]
mod cloudformation_provisions_route53_policies;
#[path = "all/cloudformation_route53_record_set_contract.rs"]
mod cloudformation_route53_record_set_contract;
#[path = "all/s3files_create_file_system_bucket.rs"]
mod s3files_create_file_system_bucket;
#[path = "all/scheduler_execution_role.rs"]
mod scheduler_execution_role;
#[path = "all/servicecatalog_signing_name_routing.rs"]
mod servicecatalog_signing_name_routing;
// Declared HERE, in sorted position in this quiet span, rather than beside the sibling SES
// suites at the head of this file or in its appended tail: both of those are where every
// concurrently landing suite puts its line, so a declaration there loses the squash merge
// to all of them, while these neighbouring lines are stable.
#[path = "all/ses_family_pre_dispatch_refusals.rs"]
mod ses_family_pre_dispatch_refusals;
// Declared HERE, beside the sibling SES suite above in this quiet span, rather than with the
// other SES suites at the head of this file or in its appended tail: both of those are where
#[path = "all/sesv2_sns_event_destination_topic_policy.rs"]
mod sesv2_sns_event_destination_topic_policy;
// Declared HERE, beside the sibling SESv2 suite above in this quiet span, for the same
// reason: the head of this file and its appended tail are where every concurrently landing
// suite puts its line, while these neighbouring lines are stable.
#[path = "all/ses_send_events_reach_a_subscribed_queue.rs"]
mod ses_send_events_reach_a_subscribed_queue;
#[path = "all/ses_bulk_send_entry_headers_override_the_template.rs"]
mod ses_bulk_send_entry_headers_override_the_template;
#[path = "all/shared_signing_name_routing.rs"]
mod shared_signing_name_routing;
#[path = "all/ssm_contacts_replication_set.rs"]
mod ssm_contacts_replication_set;
#[path = "all/sts_source_identity_is_audited.rs"]
mod sts_source_identity_is_audited;
#[path = "all/sts_web_identity_federation.rs"]
mod sts_web_identity_federation;
#[path = "all/transcribe_signing_name_routing.rs"]
mod transcribe_signing_name_routing;
#[path = "all/transfer_user_with_iam_role.rs"]
mod transfer_user_with_iam_role;
// Declared HERE, in this quiet span rather than at the head of this file, in its appended
// tail or in the `b…` span of its own name: all three are where concurrently landing
// suites put their line, so a declaration in any of them loses a squash merge to every
// one of them, while these neighbouring lines are stable.
#[path = "all/batch_job_state_change_reaches_matching_rules.rs"]
mod batch_job_state_change_reaches_matching_rules;
// Declared HERE, in sorted position in this quiet span, rather than at the head of this
// file or in its appended tail: both of those are where every concurrently landing suite
// puts its line, so a declaration there conflicts with all of them.
#[path = "all/unsigned_entry_points_reach_their_service.rs"]
mod unsigned_entry_points_reach_their_service;
#[path = "all/wisdom_qconnect_signing_name_routing.rs"]
mod wisdom_qconnect_signing_name_routing;
// Declared last, out of the sorted order above, on purpose: every new router suite adds
// a line to this file, and a line inserted in the middle of the list conflicts with
// every other suite landing in the same window. Appending keeps this change out of that
// contended region. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/sagemaker_signing_name_routing.rs"]
mod sagemaker_signing_name_routing;
#[path = "all/billing_access_denied_principal_agrees_with_sts.rs"]
mod billing_access_denied_principal_agrees_with_sts;
#[path = "all/bedrock_modelled_route_claims.rs"]
mod bedrock_modelled_route_claims;
#[path = "all/connectcampaigns_signing_name_routing.rs"]
mod connectcampaigns_signing_name_routing;
#[path = "all/pi_data_source_ownership.rs"]
mod pi_data_source_ownership;
#[path = "all/lex_signing_name_routing.rs"]
mod lex_signing_name_routing;
#[path = "all/sagemaker_trial_component_metric_suggestions.rs"]
mod sagemaker_trial_component_metric_suggestions;
#[path = "all/lex_models_v2_runtime_v2_cross_service.rs"]
mod lex_models_v2_runtime_v2_cross_service;
#[path = "all/wisdom_signing_name_routing.rs"]
mod wisdom_signing_name_routing;
#[path = "all/cloudtrail_sdk_target_routing.rs"]
mod cloudtrail_sdk_target_routing;
#[path = "all/eks_cluster_inputs_resolve.rs"]
mod eks_cluster_inputs_resolve;
// Declared here, beside the other `eks_*` suites: the service CIDR a cluster is created
// with is judged against the VPC its subnets sit in, which only an engine carrying both
// EKS and EC2 can show.
#[path = "all/eks_service_cidr_vpc_overlap.rs"]
mod eks_service_cidr_vpc_overlap;
#[path = "all/cloudformation_provisions_ecr_public_repository.rs"]
mod cloudformation_provisions_ecr_public_repository;
#[path = "all/cloudformation_provisions_elastic_beanstalk_resources.rs"]
mod cloudformation_provisions_elastic_beanstalk_resources;
#[path = "all/cloudformation_provisions_sqs_queue_policies.rs"]
mod cloudformation_provisions_sqs_queue_policies;
#[path = "all/cloudformation_provisions_sqs_queues.rs"]
mod cloudformation_provisions_sqs_queues;
#[path = "all/cloudformation_imports_an_existing_queue.rs"]
mod cloudformation_imports_an_existing_queue;
#[path = "all/opensearchserverless_cloudformation_resources.rs"]
mod opensearchserverless_cloudformation_resources;
#[path = "all/cloudformation_provisions_container_platform_resources.rs"]
mod cloudformation_provisions_container_platform_resources;
#[path = "all/kinesis_video_signing_name_routing.rs"]
mod kinesis_video_signing_name_routing;
#[path = "all/apigatewayv2_websocket_deploy_then_connections.rs"]
mod apigatewayv2_websocket_deploy_then_connections;
#[path = "all/cloudformation_provisions_fis_experiment_templates.rs"]
mod cloudformation_provisions_fis_experiment_templates;
#[path = "all/cloudformation_provisions_guardduty_detector.rs"]
mod cloudformation_provisions_guardduty_detector;
#[path = "all/cloudformation_provisions_sns_topics.rs"]
mod cloudformation_provisions_sns_topics;
#[path = "all/cloudformation_provisions_sns_subscriptions.rs"]
mod cloudformation_provisions_sns_subscriptions;
#[path = "all/cloudformation_kms_encrypted_sns_sqs_pipeline.rs"]
mod cloudformation_kms_encrypted_sns_sqs_pipeline;
#[path = "all/cloudformation_sns_topic_policy_admits_s3_notifications.rs"]
mod cloudformation_sns_topic_policy_admits_s3_notifications;
#[path = "all/sns_lambda_subscription_permission.rs"]
mod sns_lambda_subscription_permission;
#[path = "all/sns_lambda_subscription_stack_invokes_its_function.rs"]
mod sns_lambda_subscription_stack_invokes_its_function;
#[path = "all/sns_lambda_subscription_permission_boundary.rs"]
mod sns_lambda_subscription_permission_boundary;
#[path = "all/sns_lambda_subscription_stack_delivery.rs"]
mod sns_lambda_subscription_stack_delivery;
#[path = "all/cloudformation_provisions_waf_regional_ip_sets.rs"]
mod cloudformation_provisions_waf_regional_ip_sets;
#[path = "all/cloudformation_provisions_grafana_workspaces.rs"]
mod cloudformation_provisions_grafana_workspaces;
#[path = "all/mediapackagev2_published_data_plane_urls.rs"]
mod mediapackagev2_published_data_plane_urls;
#[path = "all/pi_resource_features.rs"]
mod pi_resource_features;
#[path = "all/cloudformation_provisions_cloudwatch_metric_streams.rs"]
mod cloudformation_provisions_cloudwatch_metric_streams;
#[path = "all/cloudformation_provisions_event_rules_and_ssm_parameters.rs"]
mod cloudformation_provisions_event_rules_and_ssm_parameters;
#[path = "all/cloudformation_provisions_sfn_activities.rs"]
mod cloudformation_provisions_sfn_activities;
#[path = "all/cloudformation_provisions_sfn_state_machines.rs"]
mod cloudformation_provisions_sfn_state_machines;
#[path = "all/cloudformation_provisions_s3vectors_resources.rs"]
mod cloudformation_provisions_s3vectors_resources;
#[path = "all/cloudformation_provisions_s3tables_table_buckets.rs"]
mod cloudformation_provisions_s3tables_table_buckets;
#[path = "all/cloudformation_stamps_s3vectors_stack_provenance.rs"]
mod cloudformation_stamps_s3vectors_stack_provenance;
#[path = "all/cloudformation_stack_event_notifications.rs"]
mod cloudformation_stack_event_notifications;
#[path = "all/cloudformation_provisions_msk_clusters.rs"]
mod cloudformation_provisions_msk_clusters;
#[path = "all/logs_emf_metric_extraction.rs"]
mod logs_emf_metric_extraction;
#[path = "all/logs_metric_filter_extraction.rs"]
mod logs_metric_filter_extraction;
#[path = "all/cloudformation_provisions_kms_keys.rs"]
mod cloudformation_provisions_kms_keys;
#[path = "all/cloudformation_provisions_mq_brokers.rs"]
mod cloudformation_provisions_mq_brokers;
#[path = "all/cloudformation_refuses_mq_brokers.rs"]
mod cloudformation_refuses_mq_brokers;
#[path = "all/cloudformation_provisions_agentcore_workload_identities.rs"]
mod cloudformation_provisions_agentcore_workload_identities;
#[path = "all/events_lambda_target_dead_letter_capture.rs"]
mod events_lambda_target_dead_letter_capture;
#[path = "all/events_lambda_target_resource_policy.rs"]
mod events_lambda_target_resource_policy;
#[path = "all/events_lambda_target_runs_the_handler.rs"]
mod events_lambda_target_runs_the_handler;
// Declared here, beside the other suites of this pipeline rather than in the contended tail:
// the services an events-invoked handler records its work in are mounted by this router.
#[path = "all/events_lambda_target_handler_records_the_invoking_rule.rs"]
mod events_lambda_target_handler_records_the_invoking_rule;
#[path = "all/cloudformation_provisions_lambda_functions.rs"]
mod cloudformation_provisions_lambda_functions;
#[path = "all/cloudformation_updates_lambda_functions.rs"]
mod cloudformation_updates_lambda_functions;
#[path = "all/cloudformation_provisions_lambda_event_source_mappings.rs"]
mod cloudformation_provisions_lambda_event_source_mappings;
#[path = "all/cloudformation_lambda_versions_and_aliases.rs"]
mod cloudformation_lambda_versions_and_aliases;
// Declared here, in the sorted block, for the reason the entries above give: the appended
// tail of this file is the contended region every other new suite lands in.
#[path = "all/cloudformation_sam_http_api_definitions.rs"]
mod cloudformation_sam_http_api_definitions;
#[path = "all/opensearch_vpc_endpoint_vpc_membership.rs"]
mod opensearch_vpc_endpoint_vpc_membership;
// Declared here, in this quiet span rather than in the appended tail every concurrently
// landing router suite adds its line to.
#[path = "all/apigatewayv2_vpc_link_subnets_resolve_in_ec2.rs"]
mod apigatewayv2_vpc_link_subnets_resolve_in_ec2;
// Declared here, in this quiet span, for the reason the entry above gives: the IAM role an
// API Gateway account may write its logs under.
#[path = "all/apigateway_account_cloudwatch_role.rs"]
mod apigateway_account_cloudwatch_role;
// Declared here, in this quiet span rather than in the contended tail: the Directory
// Service control plane and its data plane over one engine.
#[path = "all/ds_data_plane_access_and_realm.rs"]
mod ds_data_plane_access_and_realm;
// Declared here, in this quiet span rather than in the contended tail: a log group's
// subscription filter, the Firehose stream it forwards to and the object that stream
// delivers.
#[path = "all/logs_subscription_filter_to_firehose_s3.rs"]
mod logs_subscription_filter_to_firehose_s3;
#[path = "all/zz_probe_whole_emulator.rs"]
mod zz_probe_whole_emulator;
#[path = "all/cloudformation_provisions_cognito_identity_pools.rs"]
mod cloudformation_provisions_cognito_identity_pools;
// Declared here, beside the suite above it shares its two services with: what a subscribed
// directory FORWARDS of the writes that plane makes.
#[path = "all/ds_log_subscription_forwarding.rs"]
mod ds_log_subscription_forwarding;
// Declared beside it, for the same reason: the Availability Zones of the subnets a directory
// create places its servers in, which only Amazon EC2 can answer.
#[path = "all/ds_placement_zones.rs"]
mod ds_placement_zones;
#[path = "all/sns_lambda_cdk_subscription_delivery.rs"]
mod sns_lambda_cdk_subscription_delivery;
// Declared here, in the appended tail: the whole ingest path of a deployed `AWS_PROXY` stage,
// and the log group the invocation that path makes writes its run to.
#[path = "all/apigateway_proxy_lambda_ddb_pipeline.rs"]
mod apigateway_proxy_lambda_ddb_pipeline;
// Declared here, in the appended tail: what a DICOM import job leaves in its data store,
// read from the objects Amazon S3 holds under the role IAM holds.
#[path = "all/medical_imaging_dicom_import.rs"]
mod medical_imaging_dicom_import;
// Declared here, in the appended tail: what REPEATED requests to one deployed stage are each
// answered with, and the one stream record each new item they write carries.
#[path = "all/apigateway_repeated_stage_requests.rs"]
mod apigateway_repeated_stage_requests;
// The suites that stood here — `apigateway_iam_authenticated_invoke`,
// `cloudformation_rollback_triggers_revert_the_operation`,
// `cloudformation_wafv2_protection_stack`, `lambda_sqs_event_source_batching_window` and
// `cloudformation_asset_bucket_publishing_role` — are each ALREADY declared above, in the
// sorted head. Two concurrent changes appended the same entry, which is a redefinition and
// stops this target from compiling; the duplicate lines are removed here and the first
// declaration of each stands.
// Declared here, in the appended tail: WAFv2 protection a stack provisions. It stood twice in
// this tail and once in the sorted head across concurrent changes; one declaration of each
// suite is what this target compiles, so the entry lives here, once.
#[path = "all/cloudformation_wafv2_protection_stack.rs"]
mod cloudformation_wafv2_protection_stack;
// Several suites that stood here are each ALREADY declared above, in the sorted head: two
// concurrent changes appended the same entry, which is a redefinition and stops this target
// from compiling, so the duplicate lines are removed and the first declaration of each
// stands. Add a NEW suite in the sorted span its name belongs to, and check it is not
// already declared before appending it here.
// The five suites that stood here — `apigateway_iam_authenticated_invoke`,
// `cloudformation_asset_bucket_publishing_role` — are each ALREADY declared above (the first
// four in the sorted head, the fifth at line 34, and the wafv2 one at line 1006). Two
// from compiling; the duplicate lines are removed here and the first declaration of each
// stands.
// reason those entries record: the appended tail at the end of this file and the `e…` span
#[path = "all/eks_control_plane_logs.rs"]
mod eks_control_plane_logs;
// Declared in this same head region, for the reason the note above records: the NEGATIVE
// half of a cluster's control plane logging — a type nobody enabled exporting nothing —
// is observable only with EKS and CloudWatch Logs on one engine.
#[path = "all/eks_control_plane_log_suppression.rs"]
mod eks_control_plane_log_suppression;
// Declared in this same head region, for the reason the note above records.
#[path = "all/bedrock_agentcore_data_plane_parent_resources.rs"]
mod bedrock_agentcore_data_plane_parent_resources;
// Declared HERE, in sorted position beside the other CloudFront suites, rather than appended
// at the end of this file: the tail is where every suite landing in the same window appends.
#[path = "all/cloudfront_edge_metrics_drive_the_alarm_chain.rs"]
mod cloudfront_edge_metrics_drive_the_alarm_chain;
// Declared beside the suite above, whose wiring it reuses: this one grades the SAME chain over
// 4xx shares that do not breach the alarm's threshold, which needs the edge, CloudWatch, SNS
// and SQS mounted together just as the firing leg does.
#[path = "all/cloudfront_edge_4xx_share_decides_the_alarm.rs"]
mod cloudfront_edge_4xx_share_decides_the_alarm;
// Nothing below this point re-declares a suite: several landing in the same window each
// appended their own entry to this tail, so the same suite stood here two and three times
// over, which is a redefinition (`E0428`) that stops this target from compiling. The
// duplicate lines are gone and ONE declaration of each suite stands —
// `all_rs_declares_each_suite_once` below is what keeps it that way, so add a NEW suite in
// the sorted span its name belongs to and check it is not declared already before appending
// it here.
// Declared here, in the appended tail: what a stack declaring the `AWS::Config::*` resources
// really creates in AWS Config, which no crate-local suite can see both sides of.
#[path = "all/cloudformation_provisions_config_recording.rs"]
mod cloudformation_provisions_config_recording;
// Declared here, in the appended tail, for the reason the entries above record: the tail and
// the sorted `c…` span (where every cloudformation and codepipeline suite would land) are
#[path = "all/cloudformation_provisions_codebuild_projects.rs"]
mod cloudformation_provisions_codebuild_projects;
#[path = "all/ecr_push_triggers_its_pipeline.rs"]
mod ecr_push_triggers_its_pipeline;
#[path = "all/codepipeline_build_action_invokes_codebuild.rs"]
mod codepipeline_build_action_invokes_codebuild;
// `cloudformation_lambda_ssm_sourced_update`, `cloudfront_edge_cache_refresh`,
// `cloudformation_imports_an_existing_queue` and `logs_subscription_filter_to_firehose_s3`
// each stood here too and are ALREADY declared above (lines 57, 431, 889 and 984): two
// concurrent changes appended the same entry, which is a redefinition and stops this
// target from compiling, so the duplicate lines are removed here and the first declaration
// of each stands — the same resolution the note further up records for five others.
#[path = "all/zz_opensearch_fgac_probe.rs"]
mod zz_opensearch_fgac_probe;
// Declared because the build enforces that every file under `tests/all/` is: the file this
// names is EMPTY (see its own note) and compiles to nothing.
#[path = "all/zz_diag.rs"]
mod zz_diag;
// Declared here, in this same head region, for the reason the entries above record: this
// suite holds THIS list to one declaration per suite, so it belongs to no service's span of
// the sorted list either.
#[path = "all/all_rs_declares_each_suite_once.rs"]
mod all_rs_declares_each_suite_once;
// `cloudformation_wafv2_protection_stack` and
// `bedrock_agentcore_data_plane_parent_resources` are NOT re-declared here: each is already
// declared in the sorted head, and a second `mod` for one file is a redefinition that stops
// this target from compiling. Several other suites stood here the same way —
// `apigateway_iam_authenticated_invoke`, `lambda_sqs_event_source_batching_window`,
// `cloudformation_asset_bucket_publishing_role` — and each stands in that head too, so the
// lines that were appended here a second time are gone and ONE declaration of each remains.
#[path = "all/cloudformation_rest_api_serves_its_stage.rs"]
mod cloudformation_rest_api_serves_its_stage;
// Declared HERE, beside the sibling SESv2 suites above in this quiet span, for the same
// reason as they are.
#[path = "all/ses_send_event_reports_what_answered.rs"]
mod ses_send_event_reports_what_answered;
// `cloudformation_provisions_config_recording` is NOT re-declared here: it is already declared
// earlier in this same tail, and a second `mod` for the same file is a redefinition that stops
// this target from compiling.
// `cloudformation_wafv2_protection_stack` is NOT re-declared here: it is already declared in
// the sorted head, and a second `mod` for the same file is a redefinition that stops this
// target from compiling.
// `bedrock_agentcore_data_plane_parent_resources` is NOT re-declared here either: it stands
// in the sorted head, and one `mod` per file is what this target compiles with.
// Declared here, in the appended tail, for the reason the entries above record: the sorted
// `e…` span is a region concurrently landing suites add their line to.
// declaration of each stands. `bedrock_agentcore_data_plane_parent_resources`, which stood
// just below, was appended the same way and is declared in that head too.
// `cloudformation_wafv2_protection_stack`, `lambda_sqs_event_source_batching_window`,
// `cloudformation_asset_bucket_publishing_role` and
// `bedrock_agentcore_data_plane_parent_resources` — are each ALREADY declared above, in the
// Several suites that stood here are each ALREADY declared above: two concurrent changes
// appended the same entry, which is a redefinition and stops this target from compiling, so
// the duplicate lines are removed and the first declaration of each stands.
// appended tail and the sorted `c…`/`s…` spans are both regions concurrently landing suites
#[path = "all/ses_configuration_sets_shared_across_v1_and_v2.rs"]
mod ses_configuration_sets_shared_across_v1_and_v2;
#[path = "all/cloudformation_deployed_configuration_set_reaches_the_email_api.rs"]
mod cloudformation_deployed_configuration_set_reaches_the_email_api;
// Three EMPTY files, declared because the layout check in `vendor/build-support` REFUSES an
// undeclared file under `tests/all/` (rule 2 of `check_test_layout`) and this fixer's tool set
// can write a file but not remove one. `all/rds_shared_signing_name_routing.rs` is the same
// case and is declared below — its own header saying it is not declared is stale. They held
// the scratch probes used while tracing the configuration-set issue; the two suites above
// assert what those probes established, and the next change to touch this directory should
// delete the files and these six lines.
#[path = "all/sesv2_probe.rs"]
mod sesv2_probe;
#[path = "all/sesv2_probe2.rs"]
mod sesv2_probe2;
#[path = "all/sesv2_probe3.rs"]
mod sesv2_probe3;
// Declared here, in this same head region, for the reason the note above records: the sorted
// `s…` span every SNS suite lands in is a region concurrently landing suites append to.
#[path = "all/sns_lambda_invoke_permission_revocation.rs"]
mod sns_lambda_invoke_permission_revocation;
// Declared here, in the appended tail: the log group a stack DECLARES, read back by the name
// the stack published, and the records the function pointed at it leaves in it.
#[path = "all/cloudformation_declared_log_group_takes_its_function_logs.rs"]
mod cloudformation_declared_log_group_takes_its_function_logs;
// Declared here, in the appended tail rather than in the sorted `c` span, for the reason
// the notes above record: both the tail and that span are where concurrently landing suites
// put their lines, and a line in the middle of the span conflicts with all of them.
#[path = "all/cloudformation_provisions_redshift_subnet_groups.rs"]
mod cloudformation_provisions_redshift_subnet_groups;
// Declared beside its sibling cognito suites, at the head, for the same reason they are here.
#[path = "all/cognito_authorizer_issuer_endpoint.rs"]
mod cognito_authorizer_issuer_endpoint;
#[path = "all/emr_studio_network_resolution.rs"]
mod emr_studio_network_resolution;
// Declared here, in this quiet span rather than in the contended tail: the log group a stack
// declares, with the retention it declares, in CloudWatch Logs.
#[path = "all/cloudformation_provisions_logs_log_groups.rs"]
mod cloudformation_provisions_logs_log_groups;
// EVERY suite in this file is declared EXACTLY ONCE: a second `mod` for one file is a
// redefinition (and `clippy::duplicate_mod`), which stops this target compiling. Two
// regions here are documented as where a concurrently landing suite adds its line, so the
// same suite is sometimes appended to both; a line removed in this file is therefore always
// a duplicate of one that stands elsewhere in it, never a suite dropped.
// appended tail and the sorted `c…` span are both regions concurrently landing suites add
// Declared in the appended tail, which is where every new suite inserts its lines: what a
// stack publishes for the `Fn::GetAtt` attributes of an `AWS::EC2::Instance` it launched —
// the values Amazon EC2 itself reports for that instance.
#[path = "all/cloudformation_ec2_instance_rollback.rs"]
mod cloudformation_ec2_instance_rollback;
// Declared in the appended tail, which is where every new suite inserts its lines: the API
// Gateway resources that govern ACCESS to a REST API a stack created — an api key, an
// authorizer, a usage plan and the association that puts a key in a plan.
#[path = "all/cloudformation_apigateway_access_control.rs"]
mod cloudformation_apigateway_access_control;
// Several suites that stood in this tail were each ALREADY declared above, in the sorted head
// or earlier in the tail, where their own entry records why they were placed there: two
// concurrent changes appending one entry is a redefinition that stops this target from
// compiling, so the duplicate lines are gone and ONE declaration of each stands.
// Declared in the appended tail, which is where every new suite inserts its lines: the
// knowledge base a stack declares, which has to exist in Amazon Bedrock Agent under the
// identifier that service minted for it — the one the stack publishes.
#[path = "all/cloudformation_provisions_a_bedrock_knowledge_base.rs"]
mod cloudformation_provisions_a_bedrock_knowledge_base;
// Declared here, in the appended tail: what an invocation PRINTS, through the metric filter on
// its log group, the alarm over that metric, the SNS action and the queues subscribed to it.
#[path = "all/lambda_log_metric_filter_drives_the_alarm_chain.rs"]
mod lambda_log_metric_filter_drives_the_alarm_chain;
#[path = "all/bedrock_catalogue_is_invocable.rs"]
mod bedrock_catalogue_is_invocable;
#[path = "all/bedrock_logging_destination_region.rs"]
mod bedrock_logging_destination_region;
// The ONE Amazon VPC seeding fixture the suites whose requests name a subnet literally
// share, so no suite writes a second copy of those records.
#[path = "all/ec2_network.rs"]
mod ec2_network;
#[path = "all/cloudformation_provisions_bedrock_guardrails.rs"]
mod cloudformation_provisions_bedrock_guardrails;
// Declared here, beside the sibling suites it shares its rule with: the whole deployed
// pipeline, from an `AWS::Events::Rule`/`AWS::Lambda::Permission` template to the permitted
// target's own work product and the un-permitted sibling's dead letter.
#[path = "all/cloudformation_events_rule_lambda_target_permission_boundary.rs"]
mod cloudformation_events_rule_lambda_target_permission_boundary;
// Three suites that stood here — `events_sns_lambda_delivery_observed_in_logs`,
// `agent_registry_signing_name_routing` and `cloudformation_domain_access_policy_revision` —
// are each ALREADY declared in the sorted head above, where their own entry records why they
// were placed there: two `mod` items for one name is a redefinition that stops this target
// compiling, so one declaration of each stands and the duplicates are gone.
// Declared in the appended tail: a deployed method whose integration is an `AWS` call on a
// QUEUE, served by making that call — across API Gateway, Amazon SQS and IAM on one engine.
#[path = "all/apigateway_sqs_service_integration.rs"]
mod apigateway_sqs_service_integration;
// load balancer PUBLISHES for the target groups it routes to, and what it holds in the
// account's EC2 inventory, which spans three services.
#[path = "all/elbv2_host_counts_and_interfaces.rs"]
mod elbv2_host_counts_and_interfaces;
// Declared in the appended tail, which is where every new suite inserts its lines: the whole
// deploy -> fire -> deliver -> stack-delete round trip of a stack-deployed
// `AWS::Scheduler::Schedule`.
// deploy -> read-back -> delete round trip of a stack-declared `AWS::Pipes::Pipe`, and the
// refusal of one whose declaration omits a property its type's schema requires.
#[path = "all/cloudformation_provisions_eventbridge_pipes.rs"]
mod cloudformation_provisions_eventbridge_pipes;
#[path = "all/cloudformation_deployed_schedule_invokes_its_target.rs"]
mod cloudformation_deployed_schedule_invokes_its_target;
// Declared in the appended tail, which is where every new suite inserts its lines: an
// EventBridge rule whose target is a Firehose delivery stream, and the object that stream's
// destination holds once the event reached it.
#[path = "all/events_rule_firehose_target_delivery.rs"]
mod events_rule_firehose_target_delivery;
// Declared here, beside the suite whose wire harness it reuses: the DOMAIN half of the
// one-VPC rule.
#[path = "all/opensearch_domain_vpc_placement.rs"]
mod opensearch_domain_vpc_placement;
// Declared in the appended tail, beside the suite above: the UPDATE half of that same
// flow — a stack that re-declares its `AWS::Scheduler::Schedule` re-arms it, so the next
// occurrence reaches the target the new declaration names and the one it dropped is
// handed nothing more.
#[path = "all/cloudformation_stack_update_redirects_a_schedule.rs"]
mod cloudformation_stack_update_redirects_a_schedule;
// Declared in the appended tail: an S3 ACCESS POINT POLICY that denies a Lambda function's
// execution role, enforced on the data path that function's own reads take — the access
// point created and its policy put through S3 Control's own API, both reads made by the
// running handler, and the same key still served by bucket name.
#[path = "all/s3_access_point_policy_denial.rs"]
mod s3_access_point_policy_denial;
// Declared here, in this same head region, for the reason the entry above records: the tags
// a bucket a stack created carries, which CloudFormation stamps and Amazon S3 reports.
#[path = "all/cloudformation_stamps_s3_bucket_stack_provenance.rs"]
mod cloudformation_stamps_s3_bucket_stack_provenance;
// outputs of a stack whose deployment COMPLETED — the create and the revision that swaps
// which role its deployed domain allows — each carrying the value the owning service holds.
#[path = "all/cloudformation_updated_stack_outputs.rs"]
mod cloudformation_updated_stack_outputs;
// CodePipeline execution publishes onto the default event bus, and which rule's SQS target
// receives it, spans four crates at once (CodePipeline, Amazon S3, EventBridge, Amazon SQS).
#[path = "all/codepipeline_execution_state_changes_reach_a_queue_target.rs"]
mod codepipeline_execution_state_changes_reach_a_queue_target;
// Declared beside the other Step Functions suites, for the reason the lines above give:
// the execution role the calls of a run-a-job task are made as, which spans Step
// Functions, IAM and Athena.
#[path = "all/sfn_athena_run_a_job_sync.rs"]
mod sfn_athena_run_a_job_sync;
// Declared in the appended tail, which is where every new suite inserts its lines: the chain a
// client's own PutLogEvents drives, from the log group's metric filters through the metric and
// the alarm to the queue subscribed to the alarm's topic.
#[path = "all/logs_metric_filter_drives_the_alarm_notification_chain.rs"]
mod logs_metric_filter_drives_the_alarm_notification_chain;
// Declared here, in this same head region, for the reason the entry above records: which
// copies of the in-process SQL engine a build of this workspace may contain is a property
// of the committed lock and of every service manifest at once, so no single service
// crate's test target can ask it.
#[path = "all/sql_engine_single_locked_copy.rs"]
mod sql_engine_single_locked_copy;
// CloudFormation-deployed pipe DOES with the records on its source, which spans
// CloudFormation, EventBridge Pipes, IAM and Amazon SQS at once.
#[path = "all/cloudformation_deployed_pipe_delivery.rs"]
mod cloudformation_deployed_pipe_delivery;
// client of a stack-deployed state machine gets — the ARN the stack publishes, the failure an
// execution role this account's IAM cannot answer for produces, and the abort a stop records —
// which spans CloudFormation, Step Functions and IAM at once.
#[path = "all/cloudformation_sfn_absent_role_and_stop.rs"]
mod cloudformation_sfn_absent_role_and_stop;
// deploy -> read-back -> delete round trip of a stack-declared `AWS::Braket::SpendingLimit`,
// and the refusals Amazon Braket's own control plane answers for one it will not create.
#[path = "all/cloudformation_provisions_braket_spending_limits.rs"]
mod cloudformation_provisions_braket_spending_limits;
// Declared in the appended tail, which is where every new suite inserts its lines: the TOPIC
// destination of a bucket's notification configuration, from the grant S3 reads before it
// records the container to an object event travelling through the topic onto a subscribed
// queue — S3, SNS and SQS at once.
#[path = "all/s3_object_events_fan_out_through_sns.rs"]
mod s3_object_events_fan_out_through_sns;
// RECORDED load-balanced Fargate service coming up spans ECS, Elastic Load Balancing v2 and
// the account's Amazon EC2 records at once (the subnet its interface is created in and the
// route its image pull takes), so no single service crate's test target can ask it; the table
// over the counts the rule generalises over is the owning ECS crate's own suite.
#[path = "all/ecs_fargate_service_converges_to_its_desired_count.rs"]
mod ecs_fargate_service_converges_to_its_desired_count;
// Declared in the appended tail, which is where every new suite inserts its lines: what the
// audit record of a call a service ACCEPTED and then failed carries, which spans the service
// that writes the record (IAM, CloudTrail) and the `cloudtrail:LookupEvents` that reports it
// in one Region and not another, so no single service crate's test target can ask it.
#[path = "all/audited_calls_that_fail_after_acceptance.rs"]
mod audited_calls_that_fail_after_acceptance;
// Amazon SNS notifications an Auto Scaling group's notification configuration publishes
// span Auto Scaling, Amazon EC2, SNS and Amazon SQS at once, and the publish is addressed
// the way a client's own `sns`-scoped call is, so no single crate's test target can ask
// which mounted model claims it.
#[path = "all/autoscaling_notification_reaches_its_subscribed_queue.rs"]
mod autoscaling_notification_reaches_its_subscribed_queue;
// Declared here, in this same head region, beside the suite above it shares its readers with:
// this one holds the same committed `Cargo.lock` to recording each member's own dependency
// edges and every registry package's checksum, so it too belongs to no service's span of the
// sorted list.
#[path = "all/workspace_lockfile_records_each_member_dependency.rs"]
mod workspace_lockfile_records_each_member_dependency;
// Declared beside the sibling suite whose SES v1 sender it reuses, at the head, for the
// reason the entries above record.
#[path = "all/ses_v1_receipt_rule_lambda_invoke_grant.rs"]
mod ses_v1_receipt_rule_lambda_invoke_grant;
// Declared beside its sibling GuardDuty suite for the reason the lines above give.
#[path = "all/guardduty_malware_protection_plan_role.rs"]
mod guardduty_malware_protection_plan_role;
#[path = "all/lakeformation_grant_principal_resolution.rs"]
mod lakeformation_grant_principal_resolution;
// deploy -> read-back -> update -> delete round trip of a stack-declared
// `AWS::MWAA::Environment`, and the refusals Amazon MWAA's own control plane answers for one
// it will not create — CloudFormation, Amazon MWAA and CloudWatch Logs at once.
#[path = "all/cloudformation_provisions_mwaa_environments.rs"]
mod cloudformation_provisions_mwaa_environments;
// execution's status changes reaching an EventBridge rule's SQS target on a router that
// VERIFIES request signatures — the axis the owning crate's own table cannot cover, since it
// runs with authentication off and so cannot see whether the publication the clock makes with
// no request in flight is accepted.
#[path = "all/sfn_execution_status_change_on_an_authenticating_router.rs"]
mod sfn_execution_status_change_on_an_authenticating_router;
// Declared in the appended tail, which is where every new suite inserts its lines: WHICH
// reviewer released a change is a fact about the caller of one call, so pinning it spans the
// roles IAM writes, the sessions STS mints, the decisions CodePipeline serves and the records
// CloudTrail reports at once — no single service crate's test target can ask it.
#[path = "all/codepipeline_approval_reviewer_attribution.rs"]
mod codepipeline_approval_reviewer_attribution;
// Declared in the appended tail, which is where every new suite inserts its lines: the cache
// cluster an `AWS::ElastiCache::CacheCluster` declares, created for real in ElastiCache and
// read back through that service's own describe — CloudFormation and ElastiCache at once, so
// no single service crate's test target can ask it.
#[path = "all/cloudformation_provisions_elasticache_cache_clusters.rs"]
mod cloudformation_provisions_elasticache_cache_clusters;
// Declared HERE, beside the neighbouring entry of this same quiet span, for the reason it
// records: the appended tail at the end of this file and the HEAD of the sorted list are
#[path = "all/polly_synthesis_task_side_effects.rs"]
mod polly_synthesis_task_side_effects;
// RUNNING function's own DynamoDB item call is authorized against spans IAM (the role and its
// document), STS (the session the call is signed with), Lambda (the handler and the SDK it is
// given) and DynamoDB (the decision) at once, so no single service crate's test target can
// ask it.
#[path = "all/lambda_in_function_dynamodb_authorization.rs"]
mod lambda_in_function_dynamodb_authorization;
// Declared here, in this same head region, for the reason the entry above records: a backup
// job's own state change, routed by content to an EventBridge rule's Lambda target whose run
// is read in CloudWatch Logs, spans AWS Backup, EventBridge, Lambda, CloudWatch Logs and IAM
// at once, so no single service's test target can ask it.
#[path = "all/backup_job_state_change_invokes_its_lambda_target.rs"]
mod backup_job_state_change_invokes_its_lambda_target;
// Declared in the appended tail, which is where every new suite inserts its lines: how a
// Declared in the appended tail, which is where every new suite inserts its lines: the WHOLE
// life of an event-driven object pipeline — the conversion a bucket notification drives and
// the teardown that follows — spans Amazon S3, AWS Lambda, IAM and CloudFormation at once, so
#[path = "all/s3_notification_pipeline_lifecycle.rs"]
mod s3_notification_pipeline_lifecycle;
// S3 -> SNS -> Lambda chain, from an object write through the bucket's topic configuration onto
// the function subscribed to that topic, which spans S3, SNS, Lambda, Amazon SQS, IAM and
// CloudFormation at once — no single service crate's test target can see both hops.
#[path = "all/s3_sns_lambda_chain_invoke_permission.rs"]
mod s3_sns_lambda_chain_invoke_permission;
// stack-declared secret rotation DOES — the four-step rotation protocol a CloudFormation
// `AWS::SecretsManager::RotationSchedule` puts in force — spans CloudFormation, AWS Secrets
// Manager, AWS Lambda, IAM and STS at once, so no single service crate's test target can ask
#[path = "all/cloudformation_rotation_schedule_rotates_its_secret.rs"]
mod cloudformation_rotation_schedule_rotates_its_secret;
// ROLLED-BACK stack leaves behind in Amazon S3 — nothing — which spans CloudFormation (the
// create and its rollback), Amazon S3 (the refusal and the bucket read) and Amazon SQS (the
// destination that decides the refusal) at once.
#[path = "all/cloudformation_rollback_deletes_the_buckets_it_created.rs"]
mod cloudformation_rollback_deletes_the_buckets_it_created;
// Declared in the appended tail: the IDENTITY-BASED authorization of a bucket's own lifecycle
// — the permissions a create's headers ask for and the one a delete asks for — which spans
// IAM (the documents), STS (the session) and Amazon S3 (the decision) at once, so no single
// service crate's test target can see both sides.
#[path = "all/s3_bucket_lifecycle_identity_authorization.rs"]
mod s3_bucket_lifecycle_identity_authorization;
// stack deploys for a property written as a `{{resolve:secretsmanager:…}}` dynamic reference
// spans CloudFormation (which resolves it), AWS Secrets Manager (which holds the secret) and
// the service that owns the configured resource at once, so no single crate's test target can
#[path = "all/cloudformation_secretsmanager_dynamic_references.rs"]
mod cloudformation_secretsmanager_dynamic_references;
// reference RabbitMQ-consumer estate deployed from scratch in one stack spans CloudFormation,
// AWS Secrets Manager, Amazon MQ, IAM and AWS Lambda at once, so no single service crate's
// test target can deploy it.
#[path = "all/scratch_estate.rs"]
mod scratch_estate;
// completed stack PUBLISHES for the Amazon Redshift resources it declared — an `OutputValue`
// on every output, carrying the endpoint, port and parameter-group name Amazon Redshift
// itself reports — which spans CloudFormation (the outputs) and Amazon Redshift (the cluster
// and its parameter group) at once.
#[path = "all/cloudformation_redshift_stack_outputs.rs"]
mod cloudformation_redshift_stack_outputs;
#[path = "all/logs_delivery_source_cloudfront_resource.rs"]
mod logs_delivery_source_cloudfront_resource;
#[path = "all/logs_subscription_filter_role_trust.rs"]
mod logs_subscription_filter_role_trust;
// Declared in the appended tail, which is where every new suite inserts its lines: what a log
// group's subscription filter whose destination is a LAMBDA FUNCTION really hands that
// function, and the lines the run it caused left in the function's own log group — CloudWatch
// Logs, AWS Lambda, IAM and CloudFormation at once, so no single service crate's test target
// can see both ends of it.
#[path = "all/logs_subscription_filter_to_lambda.rs"]
mod logs_subscription_filter_to_lambda;
// Declared in the appended tail, which is where every new suite inserts its lines: a CI
// topology DEPLOYED BY TEMPLATE and then run — the pipeline, the build project and the two
// service roles a stack provisions, driven to the terminal execution status the build decided
// — spans CloudFormation, CodePipeline, CodeBuild, IAM and Amazon S3 at once, so no single
// service crate's test target can see both ends of it.
#[path = "all/cloudformation_ci_topology_runs_its_pipeline.rs"]
mod cloudformation_ci_topology_runs_its_pipeline;
// deploy -> read-back -> update -> delete round trip of the Amplify UI Builder entities a stack
// declares, which spans CloudFormation (the template and the calls), AWS Amplify (the app they
// belong to) and Amplify UI Builder (the component, the form and the theme) at once, so no
// single service crate's test target can see it.
#[path = "all/cloudformation_provisions_amplifyuibuilder_entities.rs"]
mod cloudformation_provisions_amplifyuibuilder_entities;
// Declared in this quiet span of the sorted list, beside the other custom-resource suite,
// rather than appended at the end of this file: the tail is the contended region every
// concurrently landing suite appends to.
#[path = "all/cloudformation_sns_custom_resource_protocol.rs"]
mod cloudformation_sns_custom_resource_protocol;
// Declared in the appended tail, which is where every new suite inserts its lines: the role
// an EKS Pod Identity association binds to a service account is resolved against IAM, so
// EKS and IAM have to be on one engine for the rule to be observable at all.
#[path = "all/eks_pod_identity_role_trust.rs"]
mod eks_pod_identity_role_trust;
// Declared in the same tail: a managed node group's capacity is an Auto Scaling group and a
// launch template, and whether a Fargate profile's subnets are private is a fact about the
// EC2 network — EKS, Auto Scaling and EC2 at once.
#[path = "all/eks_nodegroup_capacity.rs"]
mod eks_nodegroup_capacity;
// Declared in the appended tail, which is where every new suite inserts its lines: which
// QUALIFIER of a function a `lambda`-protocol SNS subscription invokes, and whose grant admits
// it, spans SNS, AWS Lambda, Amazon SQS, IAM and CloudFormation at once, so no single service
// crate's test target can see both the grant and the run it admitted.
#[path = "all/sns_lambda_qualified_endpoint_delivery.rs"]
mod sns_lambda_qualified_endpoint_delivery;
// Declared in the appended tail, beside the suites above: the Resource Groups group a
// CloudFormation stack declares spans CloudFormation (the deployment and its outputs) and
// Resource Groups (the group, its query and its tags) at once, so neither crate's own test
// target can see both ends of it.
#[path = "all/cloudformation_provisions_resource_groups.rs"]
mod cloudformation_provisions_resource_groups;
// "an EventBridge rule runs a Fargate task" estate deployed in one stack spans
// CloudFormation, Amazon S3, CloudWatch Logs, Amazon EC2, IAM, Amazon ECS and Amazon
// EventBridge at once, so no single service crate's test target can deploy it.
#[path = "all/cloudformation_eventbridge_scheduled_fargate_estate.rs"]
mod cloudformation_eventbridge_scheduled_fargate_estate;
#[path = "all/cognito_vended_session_leading_keys_scoped_query.rs"]
mod cognito_vended_session_leading_keys_scoped_query;
// here: what a bucket policy decides about a read made with credentials an identity pool
// vended spans Amazon Cognito Identity, IAM, STS and Amazon S3 at once.
#[path = "all/cognito_vended_session_bucket_policy.rs"]
mod cognito_vended_session_bucket_policy;
// resource whose create was refused PART-WAY leaves behind spans CloudFormation (which
// deploys and disposes), Amazon S3 (which refuses the follow-up call and then answers the
// read) and the service owning the destination that decides the refusal, so no single
#[path = "all/cloudformation_half_made_resource_is_not_left_behind.rs"]
mod cloudformation_half_made_resource_is_not_left_behind;
// Declared in the appended tail: the EVENT path of an Amazon EMR cluster's lifecycle spans
// Amazon EMR (which announces each state its cluster enters), EventBridge (which routes the
// envelope by its content) and Amazon SQS (which holds what a rule's target received), so no
// single service crate's test target can see both ends of it.
#[path = "all/emr_cluster_state_change_events.rs"]
mod emr_cluster_state_change_events;
// Declared beside the suite above: the other half of this fix's evidence — an S3 Batch
// Operations job settling on the assembled router with request authentication ON, where the
// job's own run reaches Amazon S3 and IAM over the emulator's in-process callback, so no
// single service crate's test target can see it. Named like `zz_opensearch_fgac_probe.rs`,
// the sibling suite that drives a whole-router probe the same way.
#[path = "all/zz_batch_job_probe.rs"]
mod zz_batch_job_probe;
// EMPTY, and declared only because the build script requires every file under `tests/all/`
// to be declared: `zz_probe.rs` held a throwaway probe for the pre-existing stack overflow
// in `all/s3_notification_invokes_its_lambda_target.rs` and carries no test now. The file
// stays because this fixer's tool set can write a file but not remove one; the next change
// to touch this directory should delete both it and this declaration.
#[path = "all/zz_probe.rs"]
mod zz_probe;
// partitions of a table a running function may address under a conditioned execution-role
// grant spans IAM (the clause), STS (the session the function's call is signed with), Lambda
// (the handler and the SDK it is given) and DynamoDB (the decision) at once, so no single
// service crate's test target can ask it.
#[path = "all/lambda_in_function_dynamodb_partition_conditions.rs"]
mod lambda_in_function_dynamodb_partition_conditions;
// Declared in the appended tail, which is where every new suite inserts its lines: that a
// function a completed stack declared really exists in AWS Lambda, and that the records of its
// runs land in the one group it logs to — CloudFormation, IAM, AWS Lambda and CloudWatch Logs
// at once, so no single service crate's test target can ask it.
#[path = "all/cloudformation_lambda_function_log_destination.rs"]
mod cloudformation_lambda_function_log_destination;
// Declared in this same head region, for the reason the note above records: the WORKFLOW half
// of a cluster's control plane logging — the update reaching `Successful` before the logs are
// read — is observable only with EKS and CloudWatch Logs on one engine.
#[path = "all/eks_logging_update_reaches_cloudwatch_logs.rs"]
mod eks_logging_update_reaches_cloudwatch_logs;
// Declared in the appended tail, which is where every new suite inserts its lines: that ONE
// set of the account's email templates stands behind both Amazon SES APIs, and that a
// continuation token either API's template listing issued continues that one list on the
// other, spans the classic SES surface and the SESv2 surface at once, so no single service
#[path = "all/ses_templates_shared_across_v1_and_v2.rs"]
mod ses_templates_shared_across_v1_and_v2;
// Declared in the appended tail, which is where every new suite inserts its lines: the EBS
// direct-API write loop driven THROUGH a refused block write and then observed as completed
// spans the EBS direct APIs (which mint, refuse, write and seal) and Amazon EC2 (the only
// read of a snapshot's state, and the delete), so no single service crate's test target owns
// both ends of it.
#[path = "all/ebs_refused_block_write_then_ec2_snapshot_completion.rs"]
mod ebs_refused_block_write_then_ec2_snapshot_completion;
// RUNNING function's own Amazon SQS message-plane call is authorized against spans
// CloudFormation (which deploys the queue, the role and the relay together), IAM (the role and
// its document), STS (the session the call is signed with), AWS Lambda (the handler and the SDK
// it is given) and Amazon SQS (the decision) at once, so no single service crate's test target
// can ask it.
#[path = "all/lambda_in_function_sqs_authorization.rs"]
mod lambda_in_function_sqs_authorization;
// Declared in the appended tail, which is where every new suite inserts its lines: that the
// network an OpenSearch Serverless managed VPC endpoint is placed in is resolved against
// Amazon EC2, which owns the VPC and the subnets, so no endpoint is stored in a network that
// does not exist — two services on one engine, which neither crate's own test target can ask.
#[path = "all/aoss_vpc_endpoint_placement_resolves_in_ec2.rs"]
mod aoss_vpc_endpoint_placement_resolves_in_ec2;
// Declared in the appended tail, which is where every new suite inserts its lines: the NETWORK
// ORIGIN of an S3 access point, enforced on the data path — the access point created with its
// VpcConfiguration through S3 Control's own API and then addressed on S3's — spans S3 Control
// (which records the VPC it is bound to) and Amazon S3 (which decides the request), so no
#[path = "all/s3_access_point_vpc_network_origin.rs"]
mod s3_access_point_vpc_network_origin;
// Declared in this same head region, for the reason the entry above records: a backup job's
// state change reaching the queue ONE template's rule, topic, subscription and policies wire
// up spans CloudFormation, AWS Backup, Amazon DynamoDB, IAM, EventBridge, SNS and Amazon SQS
// at once, so no single service crate's test target can deploy it.
#[path = "all/cfn_deployed_backup_pipeline_delivers_job_events.rs"]
mod cfn_deployed_backup_pipeline_delivers_job_events;
// Declared in the appended tail, which is where every new suite inserts its lines: a custom
// domain and the BASE-PATH MAPPING of it that a stack provisions, read back through the API
// Gateway control plane that owns them — CloudFormation makes the calls and API Gateway
// serves them, so no single service crate's test target can see both ends of it.
#[path = "all/cloudformation_apigateway_base_path_mappings.rs"]
mod cloudformation_apigateway_base_path_mappings;
// Declared in the appended tail, for the same reason: a PRIVATE integration estate — the
// Network Load Balancer a VPC link reaches, the Application Load Balancer behind it and the
// REST API stage in front of them — spans Elastic Load Balancing v2 and API Gateway at once.
#[path = "all/apigateway_vpc_link_private_integration.rs"]
mod apigateway_vpc_link_private_integration;
// Declared in this same head region, for the reason the entry above records: whether a
// session an identity pool vends is a credential a client can SIGN a DynamoDB call with
// spans the request authenticator, Cognito Identity, IAM, STS and DynamoDB at once, so no
// single service crate's test target can drive it.
#[path = "all/cognito_signed_session_leading_keys_partition.rs"]
mod cognito_signed_session_leading_keys_partition;
// RUNNING function's own Secrets Manager read follows the document in force on its execution
// role AT THE INSTANT OF THE CALL — served, refused once the grant is withdrawn, served again
// once it is put back — spans IAM (the role and the document a PutRolePolicy rewrites), STS
// (the session the call is signed with), CloudFormation (which archives the handler into a
// package), AWS Lambda (the handler and the SDK it is given) and AWS Secrets Manager (the
// decision and the value) at once, so no single service crate's test target can ask it.
#[path = "all/lambda_in_function_secretsmanager_grant_revocation.rs"]
mod lambda_in_function_secretsmanager_grant_revocation;
// Declared in the appended tail, which is where every new suite inserts its lines: a
// CloudFormation-deployed REST API gated by a TOKEN Lambda authorizer whose invocation is
// admitted by the role its authorizerCredentials names spans CloudFormation (which deploys the
// eight resources), IAM and STS (the role, its documents and the assume-role reading), AWS
// Lambda (the authorizer function and its resource policy) and API Gateway (the api, the
// authorizer, the method and the stage that serves them), so no single service crate's test
// target can deploy it.
#[path = "all/cloudformation_rest_api_token_authorizer_credentials.rs"]
mod cloudformation_rest_api_token_authorizer_credentials;
// Declared in the appended tail, which is where every new suite inserts its lines: whether
// Amazon MSK answers its OWN refusal (a name the Region holds, a role of another account, a
// value it does not accept) before it resolves a create's placement spans Amazon EC2 — which
// owns the subnets — and Amazon MSK at once, so no single service crate's test target can
// see both ends of it.
#[path = "all/kafka_msk_own_refusals_precede_the_subnet_lookup.rs"]
mod kafka_msk_own_refusals_precede_the_subnet_lookup;
// `cloudformation_monitored_alb_search_estate` and `cloudformation_domain_endpoint_outputs`
// are declared once, in the sorted span above.
#[path = "all/cloudfront_edge_generated_headers.rs"]
mod cloudfront_edge_generated_headers;
// Declared beside those two, in the same quiet span and for the same reason. This one pins
// that a REFUSED Amazon EC2 call is in CloudTrail event history, with the fault class, the
// code and the message its caller saw and no response elements.
#[path = "all/ec2_management_events.rs"]
mod ec2_management_events;
// deployed REST stage whose method integrates an Amazon DynamoDB ACTION answers, and what the
// stage's response cache then holds for it, spans IAM, DynamoDB and API Gateway at once, so
// no single crate's test target can make the call and then read the item it wrote.
#[path = "all/apigateway_dynamodb_service_integration.rs"]
mod apigateway_dynamodb_service_integration;
// EventBridge at once, so no single service crate's test target can deploy it. Declared
// once, in the head region above.
// it. Declared once, in the head region above.
// crate's test target can ask it. Declared once, beside the suites above.
// Declared here, in this same head region, for the reason the entry above records: subscribing a
// Lambda function to a log group a Step Functions state machine vends its runs into spans
// CloudWatch Logs, AWS Lambda and Step Functions at once, so no single crate's own test target
// can wire it and then read the group back.
#[path = "all/logs_vended_group_lambda_subscriber_permission.rs"]
mod logs_vended_group_lambda_subscriber_permission;
// Declared in the appended tail, which is where every new suite inserts its lines: a deployed
// method whose integration is an `AWS` call on an SNS TOPIC, served by making that call —
// asked of the router that mounts every service at once, which is where the hop between the
// stage, the topic and the topic's queue subscriber is observable end to end.
#[path = "all/apigateway_sns_service_integration.rs"]
mod apigateway_sns_service_integration;
// `cloudformation_provisions_cloudfront_resources` is declared ONCE, in the head region
// above: two `mod` items for one name is a redefinition that compiles neither.
// (`cloudformation_provisions_cloudfront_resources` is declared once, in the sorted span
// above: two `mod` items for one name is a redefinition that compiles neither.)
// Both suites below are declared ONCE, in the sorted span above beside the other
// CloudFormation suites: the pair was appended here as well while two branches were landing
// at once, which declared each of them a second time and compiled neither.
// are declared ONCE, in the head region above beside the other `cloudformation_…` suites.
// Two branches each re-declared the pair in this tail, which compiles neither: a suite is
// declared in exactly one place.
// are declared ONCE each, in the head region above: a module declared twice in this file
// compiles the suite twice, which the workspace's lints refuse.
// are declared once, in the span above: two branches each re-added the pair here after both
// had removed it, which declares each of them twice and compiles nowhere.
// Declared in the appended tail, which is where every new suite inserts its lines. Both
// suites exist under tests/all/ and had lost their declaration when two branches removed
// the same duplicated pair, which compiles them nowhere.
#[path = "all/cloudformation_monitored_alb_search_estate.rs"]
mod cloudformation_monitored_alb_search_estate;
#[path = "all/cloudformation_domain_endpoint_outputs.rs"]
mod cloudformation_domain_endpoint_outputs;
// The stack CLOCK the suites above drive instead of waiting for it: one helper, because
// `settle` is global over the CloudFormation store, so every suite that asserts a settled
// stack pulls the due instants into the past the same way.
#[path = "all/cloudformation_stack_clock.rs"]
mod cloudformation_stack_clock;
// client that dials the address an Application Load Balancer PUBLISHES is answered with when
// the listener forwards to a `lambda` target group spans CloudFormation (which archives the
// handler into a deployment package), IAM and STS (the execution role and its session), Amazon
// SQS (the queue the handler reports each invocation to), AWS Lambda (which runs it) and ELBv2
// (which opens the address and makes the invocation), so no single service crate's test target
// can ask whether the address the assembled emulator publishes answers at all.
#[path = "all/elbv2_lambda_target_published_address.rs"]
mod elbv2_lambda_target_published_address;
// (`cloudformation_monitored_alb_search_estate` and `cloudformation_domain_endpoint_outputs`
// are declared further up this tail already: a module may be declared only once.)
// (`apigateway_sns_service_integration` is declared once, further up this tail: a module may
// be declared only once, and two branches landing at once appended it here as well.)
// are declared once, further up this tail: a module may be declared only once, and two
// branches landing at once each re-added the pair here.)
// are declared in the head region above: a module may be declared only once.)
// be declared only once, and two landings each appended it here.)
// `cloudformation_provisions_cloudfront_resources` is declared once, in the sorted span
// (`cloudformation_monitored_alb_search_estate`, `cloudformation_domain_endpoint_outputs`,
// `cloudformation_stack_clock` and `elbv2_lambda_target_published_address` are each declared
// once, a few lines above in this same tail: two landings each appended the group here, and a
// module declared twice loads the same file twice, which compiles neither.)
// (`apigateway_sns_service_integration` is declared ONCE, a few lines above in this same
// tail: it was appended here as well while two branches were landing at once, which
// declared it a second time and compiled neither.)
// (`apigateway_sns_service_integration` is declared ONCE, a few lines up in this same
// appended tail: two branches landing in the same window appended a second declaration of it
// here, which is a redefinition that compiled neither, so the duplicate is removed and the
// first declaration stands.)
// (`cloudformation_provisions_cloudfront_resources`,
// `cloudformation_monitored_alb_search_estate`, `cloudformation_domain_endpoint_outputs`,
// `cloudformation_stack_clock` and `elbv2_lambda_target_published_address` are each
// declared ONCE above: a module may be declared only once.)
// Appended here, which is where a new suite inserts its lines: a Lambda function's own
// aoss-signed write to the endpoint an OpenSearch Serverless collection publishes spans
// Lambda, OpenSearch Serverless, IAM, STS and CloudFormation at once, so no single service
// crate's test target can drive it.
#[path = "all/lambda_signed_write_to_an_aoss_collection_endpoint.rs"]
mod lambda_signed_write_to_an_aoss_collection_endpoint;
// (`apigateway_cognito_authorizer_app_client_audience` is declared in the head region of
// this file already: two branches landing in the same window each declared it, and a module
// may be declared only once.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared once, in the head region
// above: a file loaded as a module twice compiles neither copy.)
// above, and `cloudformation_metric_stream_estate_is_drainable` once further down this tail:
// (The API Gateway/Cognito authorizer suite is declared ONCE, in the head region above:
// a module may be declared only once, and two branches landing at once appended it here
// as well.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared in the head region above:
// a module may be declared only once.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared once in the head region
// above, and `cloudformation_metric_stream_estate_is_drainable` once in the tail below: a
// module may be declared only once, and a second declaration compiles neither.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared ONCE, in the sorted block
// at the head of this file: it was appended here as well while two branches were landing at
// once, which declared it a second time and compiled neither.)
// (The suite that was declared a second time here, and the metric-stream one beside it, are
// each declared ONCE now — the first in the sorted span at the head of this file, the second
// further down this tail: two branches landing at once appended a second declaration of each
// here, which compiled neither.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared in the head region
// above: a module may be declared only once.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared ONCE, in the sorted head
// region above beside the other `apigateway_…` suites: two `mod` items for one name is a
// redefinition that compiles neither.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared once, in the sorted head
// region above: a module may be declared only once, and two branches landing at once
// appended it here as well.)
// (The suite over which app client's token a COGNITO_USER_POOLS authorizer admits is declared
// ONCE, in the head region above: a module may be declared only once.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared at line 30, in the sorted
// block above: a module may be declared only once, and two branches landing at once declared
// it twice.)
// `apigateway_cognito_authorizer_app_client_audience` is declared ONCE, in the head region
// above: two landings each appended their own copy here, and a module declared twice loads
// the same file twice, which compiles neither.
#[path = "all/cloudformation_metric_stream_estate_is_drainable.rs"]
mod cloudformation_metric_stream_estate_is_drainable;
// (`cloudtrail_read_only_api_calls` is declared ONCE, in the sorted block above beside the
// other CloudTrail suites, and `zz_probe` ONCE further up this tail: a module declared twice
// loads the same file twice, which compiles neither.)
// Declared at the end of the appended tail, which is where every new suite inserts its lines:
// whether a CloudWatch alarm over the health series an Amazon Redshift cluster publishes about
// itself evaluates spans both crates at once, so neither crate's own test target can create the
// cluster and then read the alarm's state.
#[path = "all/redshift_health_alarms_evaluate.rs"]
mod redshift_health_alarms_evaluate;
// (`cloudwatch_alarm_ssm_ops_item_action` is declared once, in the head region above: a
// module may be declared only once, and this tail re-declared it.)
// (`cloudwatch_alarm_ssm_ops_item_action` is declared in the head region above: a module may
// be declared only once.)
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region of this
// file: a module may be declared only once.)
// (`zz_probe` is declared ONCE, further up this tail: a module may be declared only once.)
// (`cloudformation_metric_stream_filter_update` is declared once, in the head region above:
// module may be declared only once.)
// a module may be declared only once, and two branches landing at once appended it here as
// well.)
// two branches landing in the same window each declared it, and a module may be declared
// only once.)
// (`zz_probe` is declared further up this tail already: two branches landing in the same
// window each appended it, and a module may be declared only once.)
// (`cloudwatch_alarm_ssm_ops_item_action` and the suites beside it
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the sorted head region
// the same file twice, which compiles neither.)
// (`zz_probe` is declared ONCE, further up this tail, for the reason recorded there.)
// (`zz_probe` is declared once, further up this tail: a module may be declared only once.)
// (`zz_probe` is declared once further up this tail: a module may be declared only once.)
// (`zz_probe` is declared ONCE, further up this tail: it was appended here as well while two
// branches were landing at once, which declared it a second time and compiled neither.)
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region above
// beside the other `cloudformation_…` suites: it was appended here as well while two
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the sorted head above:
// two branches landing in the same window appended a second declaration of it here, which is a
// redefinition that compiled neither, so the duplicate is removed and the first stands.)
// (The metric-stream filter-update suite is declared ONCE, in the head region above: it was
// appended here as well while two branches were landing at once.)
// (`zz_probe` is declared ONCE, further up this tail.)
// (`cloudformation_metric_stream_filter_update` is declared in the head region above: a module
// (`zz_probe` is declared further up this tail already: a module may be declared only once.)
// (`zz_probe` is declared once, above.)
// (`zz_probe` is declared ONCE, a few lines above: a module may be declared only once.)
// (`zz_probe` is declared once, further up this tail: a module may be declared only once,
// and two branches landing at once appended it here as well.)
// (`zz_probe` is declared in the head region above: a module may be declared only once, and
// two branches landing at once appended it twice.)
// `zz_probe` is declared ONCE, in the span above: two landings each appended their own copy
// here, and a module declared twice loads the same file twice, which compiles neither.
// Declared in the appended tail, which is where every new suite inserts its lines: a vector
// knowledge base built on an index created through the collection's OWN published endpoint
// spans OpenSearch Serverless (which serves that endpoint's OpenSearch REST data plane), the
// Bedrock Agent build-time plane (the knowledge base, the custom data source and the
// ingestion) and the Bedrock Agent runtime plane (the retrieval) at once, so no single
// service crate's test target can create the index and then read the document back out.
#[path = "all/bedrock_knowledge_base_over_an_aoss_collection.rs"]
mod bedrock_knowledge_base_over_an_aoss_collection;
// Declared in the appended tail: whether an UpdateStack that changes an AWS::DynamoDB::Table
// in place is reported complete only once DynamoDB reports that table ACTIVE again spans
// CloudFormation (which makes the calls) and DynamoDB (which decides the table's status), so
// neither crate's own test target can observe both ends of it.
#[path = "all/cloudformation_dynamodb_table_in_place_update.rs"]
mod cloudformation_dynamodb_table_in_place_update;
// Declared in the appended tail, which is where every new suite inserts its lines: the HTTPS
// front door, its TLS terminator and its per-connection accept loop are the router's own, not
// any one service's, and the suite drives the BINARY this build produces over its data port,
// which only this target links.
#[path = "all/https_front_door_concurrent_pooled_producer.rs"]
mod https_front_door_concurrent_pooled_producer;
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, further up this
// tail: a module may be declared only once.)
// Declared in the appended tail, which is where every new suite inserts its lines: whether a
// `@connections` call is authorized spans API Gateway V2 (which holds the api, the stage and
// the deployment the addressing resolves), IAM (the role and its document), the token service
// (the session the call signs with) and the API Gateway Management API (which decides and
// words the refusal), so no single service crate's test target can drive it.
#[path = "all/apigatewaymanagementapi_manage_connections_authorization.rs"]
mod apigatewaymanagementapi_manage_connections_authorization;
// (`sdb_legacy_query_api_on_the_assembled_router`,
// `apigateway_cognito_authorizer_app_client_audience`,
// `lambda_sqs_event_source_visibility_hold`, `codebuild_s3_build_output` and
// `cloudformation_scripted_queue_deploy` are each declared ONCE, in the spans above: two
// landings each appended their own copy here, and a module declared twice loads the same file
// twice, which compiles neither.)
// The CloudFormation search-estate and domain-endpoint-output suites are declared ONCE, in
// the span above: two landings each appended their own copy here, and a module declared
// twice loads the same file twice, which does not compile.
// (`apigateway_sns_service_integration`, `lambda_signed_write_to_an_aoss_collection_endpoint`,
// `cloudformation_metric_stream_estate_is_drainable` and `redshift_health_alarms_evaluate` are
// each declared ONCE, further up this file: two landings each appended their own copy here, and
// a module declared twice loads the same file twice, which compiles neither.)
// Declared here, in this same head region, for the reason the entry above records: whether
// the one service that claims the `sdb` signing name serves BOTH of that endpoint's APIs is
// only observable against the whole service set, where that key is resolved.
#[path = "all/sdb_legacy_query_api_on_the_assembled_router.rs"]
mod sdb_legacy_query_api_on_the_assembled_router;
// Declared here, in this same head region, for the reason the entry above records: which app
// client's token a COGNITO_USER_POOLS authorizer admits spans Amazon Cognito user pools (which
// mints the tokens) and API Gateway's execute-api data plane (which reads them), so neither
// crate's own test target can mint a pool token and then present it to a deployed method.
#[path = "all/apigateway_cognito_authorizer_app_client_audience.rs"]
mod apigateway_cognito_authorizer_app_client_audience;
// Declared here, beside the suite whose helpers it shares: the reservation a Lambda event
// source mapping holds on an Amazon SQS record is observable only as what an INDEPENDENT
// consumer of that queue is handed, so the suite spans the two services and belongs to
// neither crate's own test target.
#[path = "all/lambda_sqs_event_source_visibility_hold.rs"]
mod lambda_sqs_event_source_visibility_hold;
// What a build PUTS in, and READS out of, the account's object store: the artifact objects
// it uploads, the cache it carries between builds, and the source archive it runs on.
#[path = "all/codebuild_s3_build_output.rs"]
mod codebuild_s3_build_output;
// Declared beside the queue-provisioning suite: this one drives the whole scripted
// deploy-wait-use-delete workflow over the ASSEMBLED emulator, which is what a waiter polls.
#[path = "all/cloudformation_scripted_queue_deploy.rs"]
mod cloudformation_scripted_queue_deploy;
// region above: a module may be declared only once.)
// `cloudformation_provisions_cloudfront_resources` is declared once, just below: two `mod`
// items for one name is a redefinition that compiles neither.
// Declared in the appended tail: whether the address a stack's DomainEndpoint output hands
// its consumers is the one Amazon OpenSearch Service published for the domain — and answers
// for it — spans CloudFormation (which resolves the output) and that service (which publishes
// the address and serves the domain's data plane), so neither crate's own test target can
// observe both ends of it.
#[path = "all/cloudformation_domain_endpoint_is_dialled_by_consumers.rs"]
mod cloudformation_domain_endpoint_is_dialled_by_consumers;
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, further up this file: a
// (`https_front_door_concurrent_pooled_producer` is declared once, further up this tail: a
// module may be declared only once, and two branches landing at once appended it here as
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, further up this
// file: a file loaded as a module twice compiles neither copy.)
// envelope an identity-based authorization refusal travels in is read by comparing IAM's
// (`https_front_door_concurrent_pooled_producer` is declared once, further up this tail:
// (`cloudformation_metric_stream_estate_is_drainable` is declared further up this file
// already: two branches landing in the same window each appended it, and a module may be
// declared only once.)
// (The HTTPS front-door suite is declared ONCE, in the span above: it was appended here as
// well while two branches were landing at once.)
// (The metric-stream estate suite and the other CloudFormation suites of this window
// (`https_front_door_concurrent_pooled_producer` is declared further up this tail already: a
// (`cloudformation_metric_stream_estate_is_drainable` is declared further up this tail
// already: a module may be declared only once.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, above.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, in the span above:
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, in the head region
// above: a module may be declared only once, and two branches landing at once appended it
// here as well.)
// (The suite over whether a stack-deployed metric-stream estate is drainable is declared
// ONCE, in the sorted span above: a module may be declared only once.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared in the head region above:
// a module may be declared only once, and two branches landing at once appended it twice.)
// `cloudformation_metric_stream_estate_is_drainable` is declared ONCE, further up this tail:
// two landings each appended their own copy, and a module declared twice compiles neither.
// session an identity pool vends may DO is decided by IAM's record of the mapped role and
// answered by Amazon S3 and the Cognito Identity control plane, so no single service crate's
// test target can vend the session and then be refused by what it reaches.
#[path = "all/cognito_vended_session_is_bounded_by_its_role_policy.rs"]
mod cognito_vended_session_is_bounded_by_its_role_policy;
// Declared in this tail: which role a `Rules` role mapping resolves from a signed-in user's
// own attribute claims spans cognito-idp (the custom attribute and the sign-in),
// cognito-identity (the mapping and the vend), IAM (the roles) and STS (the session's caller
// identity), so no single service crate's test target can sign in and then read the session
// the mapping vended.
#[path = "all/cognito_rules_role_mapping.rs"]
mod cognito_rules_role_mapping;
// (`lambda_signed_write_to_an_aoss_collection_endpoint` is declared once, further up this
// tail: a module may be declared only once, and two landings each appended a copy here.)
// (`cloudformation_domain_endpoint_is_dialled_by_consumers` is declared ONCE, further up this
// (`sdb_legacy_query_api_on_the_assembled_router` and `codebuild_s3_build_output` are
// declared ONCE each, further up this tail: a module may be declared only once.)
// (`lambda_signed_write_to_an_aoss_collection_endpoint` and
// `cloudformation_domain_endpoint_is_dialled_by_consumers` are declared ONCE each, further
// up this tail: a module may be declared only once.)
// awsQuery answer with Secrets Manager's awsJson1_1 one for ONE principal, so neither
// crate's own test target can ask it.
#[path = "all/iam_secretsmanager_denial_envelope_by_protocol.rs"]
mod iam_secretsmanager_denial_envelope_by_protocol;
// (That suite is declared ONCE, in the sorted span at the head of this file: a module may be
// declared only once, and two branches landing at once declared it twice.)
// (`zz_probe` is declared ONCE, in the span above: a module may be declared only once.)
// Declared here beside the other CloudFormation suites, ONCE: the pair below was appended to
// the tail as well while two branches were landing at once, which declared each of them a
// second time and compiled neither (`mod` is a definition, not an include). The API Gateway
// suite above it was appended twice the same way; its declaration in the sorted head region is
// the one that stands.
// Both suites above are declared ONCE, in the sorted span beside the other CloudFormation
// suites: the pair was appended here as well while two branches were landing at once, which
// declared each of them a second time and compiled neither.
// EMPTY, and declared only because the build script requires every file under `tests/all/` to
// be declared: the suite that pins what an `UpdateStack` does to the `AWS::S3::Bucket` a stack
// already created runs in the CloudFormation crate's own test target, which mounts Amazon S3
// beside CloudFormation on one engine. See the file's own header.
#[path = "all/cloudformation_s3_bucket_update_and_rename.rs"]
mod cloudformation_s3_bucket_update_and_rename;
// Declared in the appended tail, which is where every new suite inserts its lines: a Lambda
// durable execution's status changes reaching the SQS target of a default-bus rule spans
// CloudFormation and IAM (the role and the function), AWS Lambda (the execution and its
// publication), EventBridge (which routes the envelope) and Amazon SQS (which holds what the
// target received), on a router that authenticates every request — so no single service
// crate's test target can see both ends of it.
#[path = "all/lambda_durable_execution_status_events_reach_sqs.rs"]
mod lambda_durable_execution_status_events_reach_sqs;
// (`cloudformation_stack_clock` is declared once, in the span above: a module declared twice
// in this file compiles its suite twice.)
// Two suites that lost their declaration when two branches removed the same duplicated pair
// (`cloudformation_monitored_alb_search_estate` and `cloudformation_domain_endpoint_outputs`)
// are declared once each, in the sorted span beside the other CloudFormation suites: a module
// may be declared only once, so a second declaration here would compile them twice.
// (`elbv2_lambda_target_published_address` is declared once above, for the same reason.)
// `cloudformation_provisions_cloudfront_resources` is declared once, in the sorted span above:
// two `mod` items for one name is a redefinition that compiles neither.
// suites exist under tests/all/ and are declared once each in the sorted span above.
// (`cloudformation_stack_clock` and `elbv2_lambda_target_published_address` are each declared
// once in the spans above: a module declared twice in this file compiles its suite twice.)
// are declared once each, further up this tail, for the same reason.)
// above, for the reason the note above records.)
// (`zz_probe` is declared once, in the span above.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, in the span above.)
// above: two branches landing at once appended it here as well, which declared it twice and
// compiled neither.)
// (`zz_probe` is declared once, further up this tail: two branches landing at once appended
// it here as well, which declared it twice and compiled neither.)
// (`apigateway_cognito_authorizer_app_client_audience` is declared ONCE, in the head region
// (`zz_probe` is declared ONCE, further up this tail: two `mod` items for one name is a
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, further up this tail:
// two `mod` items for one name is a redefinition that compiles neither.)
// Declared here, in this same head region, for the reason the entry above records: the chain
// from a table's write traffic through the AWS/DynamoDB consumption series, the alarm over that
// series and its SNS action to a subscribed queue spans DynamoDB, CloudWatch, SNS and SQS at
// once, so no single service crate's test target can drive it end to end.
#[path = "all/dynamodb_write_capacity_metrics_drive_the_alarm_chain.rs"]
mod dynamodb_write_capacity_metrics_drive_the_alarm_chain;
// above: two branches each added it here as well, and a module declared twice compiles
// neither.)
// (`zz_probe` is declared ONCE, further up this tail: two branches each added it here as well,
// and a module declared twice compiles neither.)
// two branches each added it here as well, and a module declared twice compiles neither.)
// Declared in this tail: whether a stack-deployed metric-stream estate can be drained spans
// CloudFormation and the services the pipeline is built out of, so no single service crate's
// test target can deploy it and then drain it.
// stays because a fixer's tool set can write a file but not remove one; the next change to
// touch this directory should delete both it and this declaration. Declared ONCE: two
// landings each appended their own copy, and a module declared twice loads the same file
// twice, which compiles neither.
// (The suite that drives the Amazon Cognito user-pool authorizer is declared ONCE, in the
// head region above: a module may be declared only once, so the duplicate declaration that
// was appended here is gone.)
// Declared here, in this appended tail: whether a stack-deployed metric-stream estate can be
// drained spans CloudFormation and the services the estate is built from, so no single
// service crate's test target can deploy it and then drain it.
// Declared here, in this appended tail: whether a SESv2 send's email events reach the Lambda
// target of a rule on the bus its event destination names spans SESv2, EventBridge, Lambda,
// CloudFormation, IAM and Amazon SQS, so no single service crate's test target can wire it.
#[path = "all/ses_event_destination_reaches_an_eventbridge_rule_target.rs"]
mod ses_event_destination_reaches_an_eventbridge_rule_target;
// (The Amazon Cognito authorizer suite is declared ONCE, in the head region above: a module
// may be declared only once, and two branches landing at once appended it here as well.)
// Declared ONCE, in the head region above beside the other API Gateway suites: it was
// appended here as well while two branches were landing at once, which declared it a second
// time and compiled neither.
// at the top of this file: the entry was appended here as well while two branches were
// landing at once, which declared it a second time and compiled neither.)
// (`zz_probe` is declared ONCE, further up this tail: the entry was appended here as well
// while two branches were landing at once, which declared it a second time and compiled
// the entry was appended here as well while two branches were landing at once, which declared
// it a second time and compiled neither.)
// a module may be declared only once, and it was appended here as well while two branches
// were landing at once, which compiled it nowhere.)
// Declared HERE, beside the sibling suite it shares its wiring helpers with, for the same
#[path = "all/ses_delivery_event_advances_the_subscribed_queue.rs"]
mod ses_delivery_event_advances_the_subscribed_queue;
// (Two suites that were appended here as well while branches were landing at once — the
// Cognito authorizer audience suite and the drainable metric-stream estate suite — are
// declared ONCE in the regions above, which is where they stay: a module may be declared
// only once, and declaring it twice compiles neither.)
// (`zz_probe` is declared once, further up this tail.)
// above beside the other `apigateway_…` suites: a module may be declared only once.)
// (Declared ONCE, further up this tail: the same declaration was appended here as well while
// two branches were landing at once, which declared the module a second time and compiled
// of the sorted list above: a module may be declared only once.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, in the head region
// of the appended tail above: a module may be declared only once.)
// (Several suites once declared a second time in this tail — the `cloudformation_…` pair
// beside `cloudformation_provisions_cloudfront_resources`, `cloudformation_monitored_alb_
// search_estate` and `cloudformation_domain_endpoint_outputs` among them — are declared ONCE,
// in the sorted span above: a module may be declared only once, and a second `mod` item for
// one name compiles neither. The sentence halves earlier dedup passes left here are gone with
// this note.)
// at the top of this file: a module may be declared only once, and two branches landing in the
// same window declared it a second time here, which compiled neither.)
// (`zz_probe` and `cloudformation_metric_stream_estate_is_drainable` are declared ONCE each, a
// few lines up in this same appended tail: a module may be declared only once, and two
// branches landing in the same window appended a second declaration of each here, which
// (The API Gateway/Cognito authorizer-audience suite and the metric-stream estate suite are
// declared ONCE each, in the sorted span above: both were appended here as well while two
// branches were landing at once, which declared each a second time and compiled neither.)
// a module may be declared only once. Declaring it again here compiled nothing and broke this
// target's build, which is what the duplicate below was removed for too.)
// at the top of this file: two branches landing at once had appended a second declaration
// of it here.)
// (`zz_probe` is declared once, in the head region above: a module may be declared only
// once, and two branches landing at once had appended a second declaration of it here.)
// above: two branches landing at once had appended a second declaration of it here.)
// (The suite that reads which app client's token a COGNITO_USER_POOLS authorizer admits is
// declared in the head region at the top of this file already: a module may be declared only
// once.)
// (`zz_probe.rs` is declared further up this tail already: a module may be declared only
// once. This was a second declaration of it, which compiled neither. The note it carried
// stands unchanged at that one declaration: the file is EMPTY, is declared only because the
// build script requires every file under `tests/all/` to be declared, and held a throwaway
// probe for the pre-existing stack overflow in
// `all/s3_notification_invokes_its_lambda_target.rs` — neither the probe nor that standing
// abort is touched here.)
// (The declaration itself is the one in the sorted span at the top of this file: a module may
// be declared only once, and two branches landing at once declared it twice.)
// (`zz_probe` is declared ONCE, in the head region above: a module may be declared only
// once, and two branches landing at once appended a second declaration of it here.)
// above: a module may be declared only once, and this tail declared it a second time while
// two branches were landing at once.)
// Declared beside the provisioning suite above: this one pins that the stack's DELETION is
// reported complete only once Elastic Beanstalk has let the application go, which only an
// engine carrying both services can show.
#[path = "all/cloudformation_elastic_beanstalk_removal_stabilisation.rs"]
mod cloudformation_elastic_beanstalk_removal_stabilisation;
// at the top of this file: two `mod` items for one name is a redefinition that compiles
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, in the span above: a
// Every suite the comment fragments this tail accumulated name —
// `cloudformation_provisions_cloudfront_resources`,
// `cloudformation_stack_clock`, `elbv2_lambda_target_published_address` and
// `apigateway_cognito_authorizer_app_client_audience` — is declared exactly ONCE, in the
// sorted span above: two `mod` items for one name compile neither copy.
// (`zz_probe` and `cloudformation_metric_stream_estate_is_drainable` are declared once each,
// further up this tail: a module may be declared only once.)
// stack-declared request validator really refuses an invalid call to the deployed stage before
// the integration runs spans CloudFormation, API Gateway, AWS Lambda, IAM and CloudWatch Logs
// at once, so no single crate's own test target can deploy it and then prove the function was
// never reached.
#[path = "all/cloudformation_rest_api_request_validation.rs"]
mod cloudformation_rest_api_request_validation;
// `cloudformation_provisions_cloudfront_resources` is declared ONCE, in the sorted span
// are declared earlier in this tail already: a module may be declared only once.)
// (`lambda_signed_write_to_an_aoss_collection_endpoint` is declared ONCE, further down this
// tail: two landings each appended their own copy, and a module declared twice loads the same
// file twice, which compiles neither.)
// (`cloudtrail_read_only_api_calls` is declared ONCE, in the sorted CloudTrail block above:
// the tail is where every suite landing in the same window appends, and a module declared
// twice loads the same file twice, which compiles neither.)
// (`cloudwatch_alarm_ssm_ops_item_action` is declared in the head region above: the work
// target can drive the transition and then read what it opened.)
// crate's test target can drive it. (Declared ONCE: two landings each appended their own copy
// of this line, and a module declared twice loads the same file twice, which compiles
// declared ONCE above: two `mod` items for one name is a redefinition that compiles
// neither, which is what two branches each appending the same lines here produced.)
// (`zz_probe` is declared once, just above: a module may be declared only once.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, above: a module may
// `cloudformation_provisions_cloudfront_resources` is declared once, in the head region
// (`cloudformation_metric_stream_filter_update` is declared in the head region above too: the
// suite was appended here as well while two branches were landing at once, which declared it a
// second time and compiled neither.)
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the sorted span at the
// head of this file: it was appended here as well while two branches were landing at once,
// which declared it a second time and compiled neither.)
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, in the span above: it was
// time and compiled neither.)
// Declared beside the suite above, in this quiet span of the sorted list rather than at the
// tail of this file where every concurrently landing router suite appends its line: this one
// is the health-check PROBE of an endpoint the emulator itself serves.
#[path = "all/route53_health_check_string_match.rs"]
mod route53_health_check_string_match;
// (`cloudtrail_read_only_api_calls` is declared in the sorted block beside the other
// CloudTrail suites already: a module may be declared only once, and a second `mod` item for
// it compiles neither.)
// (`lambda_signed_write_to_an_aoss_collection_endpoint` is declared once, in the span above:
// two landings each appended their own copy here, and a file loaded as a module twice does
// not compile.)
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the sorted block at the
// and a module declared twice loads the same file twice, which compiles neither.)
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, further up this tail: it
// was appended here as well while two branches were landing at once, and a module declared
// Declared ONCE, in the head region above beside the other API Gateway suite: the pair of
// declarations this tail carried was appended while two branches were landing at once, which
// declared the module a second time and compiled it nowhere.
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region above:
// Both the probe module above and the metric-stream estate suite are declared ONCE, further
// up: the second declaration of each was appended here while two branches were landing at
// once, which compiled neither.
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, in the span above: a
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, further up this tail: a
// above beside the other `cloudformation_…` suites: two `mod` items for one name is a
// redefinition that compiles neither.
// are declared once, in the span above: a module may be declared only once.)
// (`apigateway_sns_service_integration` is declared ONCE, in the sorted head region above:
// (`cloudwatch_alarm_ssm_ops_item_action` is declared ONCE, in the head region above: the
// work item an alarm's Systems Manager OpsItem action opens has to be read where a client
// reads it — through OpsCenter's own query — so neither CloudWatch's nor Systems Manager's
// own test target can drive the transition and then read what it opened.)
// (`codebuild_s3_build_output` is declared ONCE, in the span above: a module may be declared
// (`lambda_signed_write_to_an_aoss_collection_endpoint` is declared ONCE, further up this
// (`cloudformation_domain_endpoint_is_dialled_by_consumers` is declared ONCE, in the span
// (`apigateway_sns_service_integration` is declared ONCE, further up this tail: two branches
// landing at once each appended their own copy here, and a module declared twice loads the
// same file twice, which compiles neither.)
// head of this file: two branches landing at once each appended their own copy here, and a
// (`cloudformation_provisions_cloudfront_resources` is declared in the head region above: a
// module may be declared only once, and two `mod` items for one file compile the suite twice.)
// (`apigateway_sns_service_integration`, `cloudformation_monitored_alb_search_estate` and
// `cloudformation_domain_endpoint_outputs` are each declared ONCE in the spans above: a
// be declared only once, and two `mod` items for one file compile the suite twice.)
// head of this file: a module may be declared only once.)
// (`zz_probe` is declared ONCE, in the span above: a module may be declared only once, and
// `zz_probe.rs` is the empty file the build script requires a declaration for.)
// tail: a module may be declared only once.))
// (`cloudformation_provisions_cloudfront_resources` is declared once, in the span above:
// Declared HERE, once: `cloudformation_provisions_cloudfront_resources` is the suite over the
// CloudFront resources a stack provisions, and the sorted span above no longer declares it —
// two branches each removed the copy they saw, leaving it compiled nowhere.
// are declared once, in the span above: a module may be declared only once in this file, and
// two branches each appended the pair here.)
// tail: two `mod` items for one name is a redefinition that compiles neither.)
// (`cloudwatch_alarm_ssm_ops_item_action` is declared in the head region above too, for the
// reason this span records: a module may be declared only once in this file.)
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, in the span above: two
// `mod` items for one name is a redefinition that compiles neither.)
// `cloudformation_domain_endpoint_is_dialled_by_consumers` are declared ONCE each, in the
// spans above: two `mod` items for one name is a redefinition that compiles neither.)
// Declared in this appended tail, which is where every new suite inserts its lines: what a
// stack-deployed `RedrivePolicy` does to a message the deployed consumer never processes spans
// CloudFormation (which provisions the queue pair, the function and the mapping), AWS Lambda
// (which polls and invokes), Amazon SQS (which counts the deliveries and performs the move)
// and IAM (which answers for the execution role), so no single service crate's test target can
// deploy the pipeline and then watch the dead-letter queue.
#[path = "all/cloudformation_sqs_redrive_policy_dead_letters_a_failing_consumer.rs"]
mod cloudformation_sqs_redrive_policy_dead_letters_a_failing_consumer;
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region above: a
// (`apigateway_sns_service_integration` is declared ONCE, a few lines up in this same appended
// tail: two branches landing at once appended a second declaration of it here, and a module
// declared twice compiles neither.)
// top of this file: two branches landing at once appended a second declaration of it here, and
// a module declared twice compiles neither.)
// Declared in this appended tail: what a stack-deployed `AWS::Glue::Crawler` DOES when it is
// started spans CloudFormation, IAM, AWS Glue and Amazon S3 at once, so no single service
// crate's test target can deploy it, crawl with it and read the catalog it wrote.
#[path = "all/cloudformation_deployed_crawler_crawls_its_s3_target.rs"]
mod cloudformation_deployed_crawler_crawls_its_s3_target;
// of the suites this span once declared are declared ONCE, in the head region above:
// loading one file as a module twice compiles nowhere.
// (`apigateway_sns_service_integration` is declared once, further up this tail: a deployed
// method whose integration is an `AWS` call on an SNS TOPIC is asked of the router that
// mounts every service at once, and a module may be declared only once.)
// (`cloudwatch_alarm_ssm_ops_item_action` is declared ONCE, in the head region above, for
// the reason that entry records: the work item an alarm's Systems Manager OpsItem action
// opens has to be read where a client reads it — through OpsCenter's own query — so neither
// CloudWatch's nor Systems Manager's own test target can drive the transition and then read
// what it opened.)
// (`codebuild_s3_build_output` is declared once, further up this tail: what a build PUTS in,
// and READS out of, the account's object store is asserted there, and a module may be
// `cloudformation_domain_endpoint_is_dialled_by_consumers` are each declared once, further
// region of this file: it was appended here as well while two branches were landing at once,
// at the top of this file: it was appended here as well while two branches were landing at
// once, and a module declared twice loads the same file twice, which compiles neither.)
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, a few lines up this tail:
// it was appended here as well while two branches were landing at once, and a module declared
// (`cloudformation_metric_stream_estate_is_drainable` is declared in the head region above
// and `zz_probe` once further up this tail: two branches landing at once appended a second
// declaration of each here, which compiled neither. A module may be declared only once.)
// Declared here, in this appended tail: what an Amazon SQS event source mapping hands its
// function, and what it does with a batch the function did not take, spans Amazon SQS, IAM,
// CloudFormation and Lambda at once, so no single service crate's test target can send the
// message and then read the record the handler was given.
#[path = "all/lambda_sqs_event_source_failed_batch_keeps_its_message.rs"]
mod lambda_sqs_event_source_failed_batch_keeps_its_message;
// Declared here, in this appended tail: that ONE published event reaches a Lambda, an Amazon
// SQS and a Step Functions target of the same rule carrying one envelope spans EventBridge and
// each of those three services, so no single service crate's test target can publish it and
// then read what each target was handed.
#[path = "all/events_one_envelope_reaches_every_target_kind.rs"]
mod events_one_envelope_reaches_every_target_kind;
// are declared once, further up this tail: a module may be declared only once, so the second
// copy of the pair this span used to carry is gone.)
// a module may be declared only once, and two branches landing at once each appended a copy
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, further up this tail:
// (`cloudformation_domain_endpoint_is_dialled_by_consumers` is declared once, a few lines
// above: a module may be declared only once, and two branches landing at once each appended
// a copy of it here.)
// (`apigateway_cognito_authorizer_app_client_audience` and
// `cloudformation_metric_stream_estate_is_drainable` are declared in the head region above and
// in this tail respectively: a module may be declared only once, and the pair was appended
// here as well while two branches were landing at once, which compiled neither.)
// Declared in this tail: which stage refuses a request signed with a temporary credential,
// and which document its refusal names, spans IAM (the role, the user and their documents),
// STS (the mint and the scope it recorded) and Amazon S3 (which resolves the credential and
// decides the call), so no single service crate's test target can mint the session and then
// read the answer the deciding service gives.
#[path = "all/sts_session_credential_refusals.rs"]
mod sts_session_credential_refusals;
// `cloudformation_provisions_cloudfront_resources` is declared once, in the sorted span at
// the head of this file: two `mod` items for one name is a redefinition that compiles
// neither.
// (`apigateway_cognito_authorizer_app_client_audience` is declared once, in the sorted span
// at the head of this file: the entry was appended here as well while two branches were
// (`cloudformation_metric_stream_estate_is_drainable` and `redshift_health_alarms_evaluate`
// are declared ONCE each, further up this file: a module may be declared only once, and two
// branches landing at once appended each of them here as well.)
// The drift-detection status-change events a stack's drift detection publishes onto the
// default event bus, which span CloudFormation, EventBridge, Amazon SQS and Systems Manager
// at once — Amazon SQS and Systems Manager both as a target and as the service that owns a
// resource changed outside the stack.
#[path = "all/cloudformation_drift_detection_status_change_events.rs"]
mod cloudformation_drift_detection_status_change_events;
// be declared only once, and two branches landing at once each re-added it here.)
// (`cloudformation_metric_stream_filter_update` is declared once, in the head region above: a
// module may be declared only once, and two branches landing at once each re-added it here.)
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region
// above: a module may be declared only once. Its subject — a stack-deployed metric-stream
// pipeline's exports before and after an update rewrites its filter — spans
// CloudFormation, IAM, CloudWatch, Firehose and Amazon S3 at once, so no single service
// crate's test target can deploy the pipeline and then read the objects it delivered.)
// (`cloudformation_provisions_cloudfront_resources`, `cloudformation_monitored_alb_search_estate`,
// `cloudformation_domain_endpoint_outputs`, `cloudformation_stack_clock` and
// `elbv2_lambda_target_published_address` are each declared ONCE, in a span above this one:
// two `mod` items for one name is a redefinition that compiles neither, and branches landing
// at once each re-appended the same lines here.)
// above: a module may be declared only once.
// `cloudwatch_alarm_ssm_ops_item_action` is declared ONCE, in the head region above: the
// own test target can drive the transition and then read what it opened.
// `zz_probe` is declared ONCE, in the span above: a module may be declared only once.
// `cloudformation_metric_stream_estate_is_drainable` is declared ONCE, in the span above: a
// module may be declared only once.
// (`cloudformation_provisions_cloudfront_resources`, `cloudformation_monitored_alb_search_estate`
// and `cloudformation_domain_endpoint_outputs` are each declared ONCE, in a span above this
// one: a module declared twice in this file loads the same suite twice, which compiles
// `cloudformation_metric_stream_filter_update` is declared ONCE, in the head region above:
// two landings each appended their own copy here, and a module declared twice loads the same
// file twice, which compiles neither.
// (`cloudformation_monitored_alb_search_estate`, `cloudformation_domain_endpoint_outputs` and
// `cloudwatch_alarm_ssm_ops_item_action` are each declared ONCE in this file — the first two
// further down this tail, the third in the head region above — because two branches appended
// each of them here while both were landing, and a module may be declared only once.)
// (`cloudformation_metric_stream_filter_update` is declared once, in the head region above.)
// (`apigateway_sns_service_integration` is declared once, further up this tail.)
// (`cloudformation_metric_stream_filter_update` and `zz_probe` are each declared once above,
// for the same reason.)
// (`cloudformation_metric_stream_filter_update` is declared once in the head region above: a
// (`https_front_door_concurrent_pooled_producer` is declared once further up this tail: a
// (The Cognito authorizer suite is declared ONCE, in the head region above beside the other
// cross-service suites: it was appended here as well while two branches were landing at once,
// (The empty `zz_probe` is declared ONCE, further up this tail: it was appended here as well
// (`cloudformation_provisions_cloudfront_resources` is declared ONCE, in the sorted span
// above: two `mod` items for one name is a redefinition that compiles neither, and two
// branches each appended it here.)
// (The Amazon Cognito authorizer suite is declared ONCE, in the head region above: it was
// (`zz_probe` is declared ONCE, in the span above: it was appended here as well while two
// (`apigateway_cognito_authorizer_app_client_audience` is declared ONCE, further down this
// tail: a module may be declared only once, and several landings appended a note here.)
// (`zz_probe` and `cloudwatch_alarm_ssm_ops_item_action` are each declared ONCE above: a
// module may be declared only once, and several landings appended a note here.)
// (`https_front_door_concurrent_pooled_producer` and
// `cloudformation_metric_stream_estate_is_drainable` are each declared ONCE above: a module
// may be declared only once, and several landings appended a note here.)
// (`apigateway_cognito_authorizer_app_client_audience`, `zz_probe` and
// An object write on an EventBridge-enabled bucket drives a matched rule's permitted Lambda
// target, whose CreateInvalidation becomes CloudFront's own invalidation and refreshes the
// distribution's edge: Amazon S3, EventBridge, AWS Lambda, IAM, CloudFormation and CloudFront
// have to agree for the chain to hold, so no single crate's test target can drive it.
#[path = "all/s3_object_created_invalidates_the_distribution_edge.rs"]
mod s3_object_created_invalidates_the_distribution_edge;
// ONCE, further down this tail: a module may be declared only once, and two branches landing
// in the same window each appended their own copy here.)
// Declared here, beside the other CloudFormation suites: whether a stack-deployed
// metric-stream estate is drainable spans CloudFormation, IAM, CloudWatch, Firehose and
// Amazon S3 at once, so no single service crate's test target can deploy it and then drain it.
// (`cloudtrail_read_only_api_calls` is declared ONCE, in the sorted block at the head of
// this file beside the other CloudTrail suites: a module may be declared only once.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, a few lines above in
// this same tail: a module may be declared only once.)
// (The HTTPS front-door suite is declared ONCE, further up this tail: a module may be
// declared only once, and two branches landing in the same window each appended their own
// copy here.)
// above: two branches landing in the same window declared it here as well, which compiled
// two branches landing in the same window declared it here as well, which compiled neither.)
// (`zz_probe` is declared ONCE, above: two branches landing in the same window declared it
// here as well, which compiled neither.)
// branches landing in the same window declared it here as well, which compiled neither.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, above: two branches
// landing in the same window declared it here as well, which compiled neither.)
#[path = "all/sfn_relayed_dynamodb_request_id.rs"]
mod sfn_relayed_dynamodb_request_id;
#[path = "all/cloudformation_provisions_cloudfront_resources.rs"]
mod cloudformation_provisions_cloudfront_resources;
// (`cloudtrail_read_only_api_calls` is declared ONCE, in the SORTED block beside the other
// CloudTrail suites above: two landings each appended their own copy, and a module declared
// (`cloudwatch_alarm_ssm_ops_item_action` is declared in the head region above as well, for
// tail: two landings each appended their own copy here, and a module declared twice loads
// (The suite about which app client's token a COGNITO_USER_POOLS authorizer admits is
// declared ONCE, in the sorted span at the head of this file: the line was appended here as
// well while two branches were landing at once, which declared it a second time and compiled
// head of this file: the line was appended here as well while two branches were landing at
// (`zz_probe` is declared ONCE, further up this tail: the line was appended here as well
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, further up this tail: the
// line was appended here as well while two branches were landing at once, which declared it a
// (The metric-stream estate suite is declared ONCE, in the head region above, for the same
// reason the pair of CloudFormation suites above records.)
// `cloudformation_provisions_cloudfront_resources` is declared ONCE, in the sorted span at
// are declared ONCE each, just above in this tail: a module declared twice in this file
// Declared ONCE, a few lines above in this same appended tail: two branches landing in the
// same window each appended a declaration of it here, which is a redefinition that compiled
// neither, so the duplicate is removed and the first declaration stands.
// `cloudtrail_read_only_api_calls` is declared ONCE, in the sorted block beside the other
// CloudTrail suites: the tail is where every suite landing in the same window appends, so two
// landings each appended their own copy and a module declared twice compiles neither.
// `cloudwatch_alarm_ssm_ops_item_action` is declared ONCE, in the sorted span at the head of
// this file, for the reason recorded there: the work item an alarm's Systems Manager OpsItem
// action opens has to be read where a client reads it — through OpsCenter's own query — so
// neither CloudWatch's nor Systems Manager's own test target can drive the transition and
// then read what it opened.
// `cloudformation_metric_stream_filter_update` is declared ONCE, in the sorted span at the
// head of this file: two landings each appended their own copy here, and a module declared
// twice compiles neither.
// `codebuild_s3_build_output` is declared ONCE, further up this tail: two landings each
// appended their own copy, and a module declared twice compiles neither.
// `lambda_signed_write_to_an_aoss_collection_endpoint` is declared ONCE, in the span above:
// `cloudformation_domain_endpoint_is_dialled_by_consumers` is declared ONCE, further up this
// tail: two landings each appended their own copy, and a module declared twice compiles
// other CloudTrail suites: a module may be declared only once.)
// (`cloudwatch_alarm_ssm_ops_item_action` is declared ONCE, in the head region above: a
// above, for the same reason.
// Declared ONCE each, higher up this file, because branches landing in the same window kept
// appending their own copy of these lines to this tail and two `mod` items for one name is a
// redefinition that compiles neither: `cloudformation_provisions_cloudfront_resources`,
// `cloudformation_stack_clock`, `elbv2_lambda_target_published_address`,
// `cloudformation_metric_stream_estate_is_drainable` and `zz_probe`.
// (`cloudformation_metric_stream_filter_update` is declared ONCE, a few lines above this one:
// two landings each appended a declaration of it to this tail, and a module may be declared
// (`https_front_door_concurrent_pooled_producer` is declared ONCE, a few lines above this
// one: two landings each appended a declaration of it to this tail, and a module may be
// (`apigatewaymanagementapi_manage_connections_authorization` is declared ONCE, further up
// this tail, for the same reason.)
// it was appended here as well while two branches were landing at once, and a module may be
// Declared here, ONCE: whether a stack-deployed metric-stream estate drains was appended both
// here and further down this tail in the same window, which declared it twice and compiled it
// nowhere.
// (`cloudformation_metric_stream_filter_update` is declared in the head region above: it was
// appended here as well while two branches were landing at once, and a module may be declared
// (`https_front_door_concurrent_pooled_producer` is declared once, further up this tail: it
// was appended here as well while two branches were landing at once, and a module may be
// `cloudformation_provisions_cloudfront_resources` is declared ONCE, in the sorted span above
// beside the other `cloudformation_…` suites: two `mod` items for one name is a redefinition
// that compiles neither.
// (`cloudformation_metric_stream_filter_update` and `zz_probe` are declared once each — the
// first in the head region above, the second further up this tail: a module may be declared
// (`cloudformation_metric_stream_estate_is_drainable` is declared once, in the span above: a
// `cloudformation_scripted_queue_deploy` are each declared ONCE, in the sorted spans above: a
// module may be declared only once, and this tail carried a second declaration of each.)
// (`codebuild_s3_build_output`, `lambda_signed_write_to_an_aoss_collection_endpoint` and
// `cloudformation_domain_endpoint_is_dialled_by_consumers` are each declared once, in the
// spans above: a module may be declared only once.)
// (`apigatewaymanagementapi_manage_connections_authorization` is declared once, further up
// this tail: a module may be declared only once.)
// conditioned identity policy decided against the tags of the session that signed the call
// spans IAM (the role, its policy and its tags) and STS (the mint that attaches the tags) at
// once, so neither crate's own test target can tag a session and then decide about it.
#[path = "all/iam_session_tag_authorization.rs"]
mod iam_session_tag_authorization;
// `cloudformation_provisions_cloudfront_resources`, `cloudformation_monitored_alb_search_estate`,
// `elbv2_lambda_target_published_address` are each declared ONCE, in the spans above: two `mod`
// items for one name is a redefinition that compiles neither, which is what two branches
// appending the same lines to this tail at once produced.
// (`cloudtrail_read_only_api_calls` and `zz_probe` are declared ONCE each, in the regions
// above: two landings each appended their own copy to this tail, and a module declared twice
// `cloudwatch_alarm_ssm_ops_item_action` is declared in the head region above, for the reason
// recorded there.
// Declared in this tail: what a stack-deployed metric-stream pipeline exports before and after
// an update rewrites its filter spans CloudFormation, IAM, CloudWatch, Firehose and Amazon S3
// at once, so no single service crate's test target can deploy the pipeline and then read the
// objects it delivered.
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region above.)
// (`apigateway_cognito_authorizer_app_client_audience`, `cloudtrail_read_only_api_calls`,
// `cloudformation_metric_stream_estate_is_drainable`, `redshift_health_alarms_evaluate`,
// `zz_probe`, `cloudformation_metric_stream_filter_update`,
// `sdb_legacy_query_api_on_the_assembled_router`, `lambda_sqs_event_source_visibility_hold`,
// `codebuild_s3_build_output` and `cloudformation_domain_endpoint_is_dialled_by_consumers` are
// each declared ONCE, in the regions above: two landings each appended their own copy to this
// tail, and a module declared twice loads the same file twice, which compiles neither.)
// (`lambda_sqs_event_source_visibility_hold`, `codebuild_s3_build_output` and
// `cloudformation_scripted_queue_deploy` are each declared ONCE, in the sorted span at the head
// of this file: two landings each appended their own copy here, and a module declared twice
// `lambda_signed_write_to_an_aoss_collection_endpoint` and
// `cloudformation_domain_endpoint_is_dialled_by_consumers` are declared ONCE each, further up
// this tail, for the reasons recorded there.
// above: a module may be declared only once, and two branches landing at once appended a
// second declaration of it here.)
// (`zz_probe` is declared ONCE, further up this tail: a module may be declared only once,
// and two branches landing at once appended a second declaration of it here.)
// (`cloudformation_metric_stream_estate_is_drainable` is declared ONCE, in the sorted span
// (`cloudformation_metric_stream_filter_update` is declared ONCE, in the head region at the
// top of this file: it was appended here as well while two branches were landing at once,
