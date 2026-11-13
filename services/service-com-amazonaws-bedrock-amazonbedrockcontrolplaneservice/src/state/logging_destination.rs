// SPDX-License-Identifier: CC0-1.0

//! Whether the destinations a model invocation logging configuration names are destinations
//! Amazon Bedrock would deliver into — the Amazon S3 buckets it writes objects to, and the
//! CloudWatch Logs group it writes log events to under the role the same destination names.
//!
//! A destination is a resource of ANOTHER service that Amazon Bedrock itself writes the
//! invocation records into, so a configuration naming one it could not deliver into is
//! refused as input rather than installed — the modelled `ValidationException`
//! (`@httpError(400)`) — because installing it would leave an operator believing an audit
//! trail exists where no record is ever delivered. Every such question is put to the service
//! that OWNS the resource, over the in-process callback every cross-service call of an
//! emulated service uses (no socket, no outbound request, no host resolution; the caller's
//! value reaches it only as a resource name), and only that service's ANSWER refuses
//! anything: an emulator assembled without it to ask, and an answer that could not be read,
//! leave the configuration admitted, because a refusal has to be grounded — the same rule the
//! rest of this service's cross-service decisions follow ([`super::model_source`],
//! [`super::model_invocation_run`]).
//!
//! # The Amazon S3 destinations
//!
//! Delivery is REGIONAL — the records are written from the Region the configuration is
//! installed in — so a bucket that lives in another Region is not a destination this
//! configuration could ever be served from, whatever policy that bucket carries. Both
//! buckets a configuration can carry are judged: `loggingConfig.s3Config`, the object
//! destination, and `cloudWatchConfig.largeDataDeliveryS3Config`, the bucket the CloudWatch
//! Logs destination spills a large record into — the two `S3Config` members the modelled
//! `LoggingConfig` and `CloudWatchConfig` shapes of this service declare.
//!
//! WHERE a bucket lives is a question the object store ANSWERS
//! ([`super::model_invocation_run::s3_client`]). The answer is `GetBucketLocation`'s own,
//! whose `locationConstraint` is the bucket's Region and is absent for the one Region S3
//! reports no constraint for (`GetBucketLocationOutput$LocationConstraint`: "Buckets in
//! Region us-east-1 have a LocationConstraint of null").
//!
//! # The CloudWatch Logs destination
//!
//! The CloudWatch Logs destination names both the group the records are written INTO
//! (`logGroupName`) and the role they are written UNDER (`roleArn`), each `@required`
//! (`smithy_query(query="shape",shape="CloudWatchConfig")`), and Amazon Bedrock validates
//! the delivery permissions that pair describes before it enables the destination. So the
//! group has to be one CloudWatch Logs holds in this Region, and the role has to be one this
//! account's IAM resolves: a destination either half of which the owning service reports
//! absent is one whose permissions cannot be validated, and it is refused with the submitted
//! pair stated back ([`unvalidated_cloud_watch_destination_message`]) so the operator can see
//! which value to fix.
//!
//! What this asks of the role is that it RESOLVES. Two further questions about it are
//! deliberately not asked here. Whether a role that resolves may be handed to Amazon Bedrock
//! at all has a different modelled answer — the pass-role decision
//! ([`super::pass_role_denial`], the `AccessDeniedException` at `@httpError(403)`). And
//! whether the role's own identity policies grant the log-delivery actions is a question this
//! emulator's IAM cannot answer about a role provisioned OUT OF BAND: an empty policy listing
//! is evidence about this store rather than about the role, so refusing on it would make the
//! recorded success path unreachable for a client that created its delivery role elsewhere —
//! the same reading [`::service_common::iam_role::held_role_is_unassumable`] takes of a role
//! this account's IAM holds nothing under.

use crate::ops::types::LoggingConfig;

use super::model_invocation_run::s3_client;

/// What Amazon Bedrock answers for a CloudWatch Logs destination whose delivery permissions
/// it could not validate.
///
/// The sentence names the two values that pair describes and nothing else — the log group the
/// records would be written into and the role they would be written under — and both come
/// from the call being served rather than from one recorded run, so the operator sees the
/// very pair it submitted.
#[must_use]
pub fn unvalidated_cloud_watch_destination_message(log_group: &str, role_arn: &str) -> String {
    format!(
        "Failed to validate permissions for log group: {log_group}, with role: {role_arn}. \
         Verify the IAM role permissions are correct."
    )
}

/// Why the CloudWatch Logs destination `config` names is not one this Region's model
/// invocation logging could deliver into, or `None` when it is — which is the ordinary case,
/// and the answer whenever nothing could be established, and the answer for a configuration
/// that names no CloudWatch Logs destination at all.
///
/// The two halves of the destination are asked about in the order the shape declares them, so
/// a destination wrong in both is refused on the group; either half ANSWERED absent is the
/// whole refusal, because the permissions that pair describes cannot be validated without
/// both of its resources. Call it with NO store of this service held: it awaits.
pub async fn unvalidated_cloud_watch_destination(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &LoggingConfig,
) -> Option<String> {
    let destination = config.cloud_watch_config.as_ref()?;
    let log_group = destination.log_group_name.as_str();
    let role_arn = destination.role_arn.as_str();
    let unvalidated =
        ::service_common::cloudwatch_logs::log_group_is_absent(ctx, region, log_group).await
            || ::service_common::iam_role::role_is_absent(ctx, region, role_arn).await;
    unvalidated.then(|| unvalidated_cloud_watch_destination_message(log_group, role_arn))
}

/// The Region a bucket lives in when `GetBucketLocation` reports no constraint for it:
/// "Buckets in Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), which is why that Region is not one of
/// the values `BucketLocationConstraint` carries.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// What Amazon Bedrock answers for a logging destination whose bucket it could not validate
/// as one it may deliver into — here, a bucket that lives outside the Region the
/// configuration is being installed in.
///
/// The sentence names the BUCKET the request named and nothing else: the fault is about
/// that destination, so the bucket is the one variable part and it comes from the call being
/// served rather than from one recorded run.
#[must_use]
pub fn cross_region_bucket_message(bucket: &str) -> String {
    format!(
        "Failed to validate permissions for bucket: {bucket}. Verify the S3 bucket policy \
         is correct."
    )
}

/// Why the Amazon S3 destinations `config` names are not destinations this Region's model
/// invocation logging can deliver into, or `None` when every bucket it names is one it can
/// — which is the ordinary case, and the answer whenever nothing could be established.
///
/// The destinations are asked about in the order the configuration states them, so a
/// configuration wrong in more than one is refused on the first one READ. A destination the
/// object store established nothing about is SKIPPED rather than ending the check, so the
/// destination beside it is still judged.
pub async fn cross_region_logging_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &LoggingConfig,
) -> Option<String> {
    let destinations = [
        config.s3_config.as_ref(),
        config
            .cloud_watch_config
            .as_ref()
            .and_then(|cloud_watch| cloud_watch.large_data_delivery_s3_config.as_ref()),
    ];
    for destination in destinations.into_iter().flatten() {
        let bucket = destination.bucket_name.as_str();
        let Some(bucket_region) = bucket_region(ctx, region, bucket).await else {
            continue;
        };
        if bucket_region != region {
            return Some(cross_region_bucket_message(bucket));
        }
    }
    None
}

/// The Region this emulator's own Amazon S3 says `bucket` lives in, or `None` when it
/// established none: it holds no such bucket, there was no object store to ask, or the
/// answer could not be read.
async fn bucket_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Option<String> {
    match s3_client(ctx, region)
        .get_bucket_location()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(answer) => Some(
            answer
                .location_constraint()
                .map(|constraint| constraint.as_str().to_string())
                .filter(|constraint| !constraint.is_empty())
                .unwrap_or_else(|| UNCONSTRAINED_BUCKET_REGION.to_string()),
        ),
        Err(error) => {
            // Not the object store reporting a bucket's Region, so it establishes nothing
            // about this destination — and it is worth a warning, because an emulator with
            // no object store mounted to ask is one where this pre-condition cannot be
            // decided. The caller's own value is rendered in its escaped `Debug` form, so a
            // bucket name carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the logging destination's bucket could not be located in this emulator's \
                 Amazon S3"
            );
            None
        }
    }
}
