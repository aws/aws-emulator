// SPDX-License-Identifier: CC0-1.0

//! The audit-log delivery a cluster is configured with, and the Amazon S3 bucket
//! that delivery writes to as the service which OWNS the bucket reports it.
//!
//! A bucket is not a resource of this service: it is Amazon S3's. `EnableLogging`
//! takes "the name of an existing S3 bucket where the log files are to be stored"
//! and requires that "the cluster must have read bucket and put object
//! permissions" on it (`smithy_query(query="doc", shape="EnableLoggingMessage",
//! member="BucketName")`), so both facts are read where they live — reached the
//! way any emulated service reaches another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
//! no other service's crate and nothing leaves the process.
//!
//! This service keeps only the bucket NAME it was given: it places nothing in the
//! bucket and reports nothing it read off it. So a hop that could not be made at
//! all (no Amazon S3 in this process, an answer that could not be read) refuses
//! nothing — the gating reading `services/common/src/network.rs` states for a
//! service that keeps nothing of the resource it asks about — and only S3's own
//! answer grounds a refusal.

use crate::ops::types::{DescribeLoggingStatusOutput, DisableLoggingOutput, EnableLoggingOutput};

/// `LogDestinationType` for delivery to an Amazon S3 bucket, which is the
/// destination a request that names none asks for: it is the destination
/// `BucketName` and `S3KeyPrefix` describe, and the only one the two members
/// belong to.
///
/// Source: `smithy_query(query="shape", shape="LogDestinationType")` — the enum's
/// values are `s3`, `cloudwatch` and `s3table`.
pub const LOG_DESTINATION_S3: &str = "s3";
/// `LogDestinationType` for delivery to Amazon CloudWatch Logs. See
/// [`LOG_DESTINATION_S3`].
pub const LOG_DESTINATION_CLOUDWATCH: &str = "cloudwatch";
/// `LogDestinationType` for publishing system tables to an S3 table bucket. See
/// [`LOG_DESTINATION_S3`].
pub const LOG_DESTINATION_S3_TABLE: &str = "s3table";

/// What a cluster's audit logging is configured as: exactly the members a
/// `LoggingStatus` reports, held as this service's own record.
///
/// A cluster nobody enabled logging on carries the DEFAULT of this record, which
/// is logging disabled and nothing else reported — "LoggingEnabled: true if
/// logging is on, false if logging is off" is the whole of what such a cluster
/// has to say.
///
/// Nothing here is a delivery REPORT: `LastSuccessfulDeliveryTime`,
/// `LastFailureTime` and `LastFailureMessage` are facts about log files that were
/// actually written, and this control plane writes none, so a cluster reports
/// none of the three rather than a time nothing delivered at.
#[derive(Debug, Clone, Default)]
pub struct ClusterLogging {
    /// `LoggingStatus$LoggingEnabled`.
    pub logging_enabled: bool,
    /// `LoggingStatus$BucketName` — the bucket an S3 delivery writes to.
    pub bucket_name: Option<String>,
    /// `LoggingStatus$S3KeyPrefix` — "the prefix applied to the log file names".
    pub s3_key_prefix: Option<String>,
    /// `LoggingStatus$LogDestinationType`.
    pub log_destination_type: Option<String>,
    /// `LoggingStatus$LogExports` — the log types being exported, in the order
    /// the request listed them.
    pub log_exports: Vec<String>,
}

impl ClusterLogging {
    /// The log types this configuration exports, or `None` where it names none:
    /// an empty list is not a list of log types.
    fn reported_log_exports(&self) -> Option<Vec<String>> {
        (!self.log_exports.is_empty()).then(|| self.log_exports.clone())
    }

    /// This record as the `LoggingStatus` `EnableLogging` answers with.
    ///
    /// The two builders below fill the SAME modelled shape (`LoggingStatus`),
    /// which the generated types project once per operation that answers it, so
    /// each operation's answer is built from this one record and the two cannot
    /// report a cluster's logging differently.
    pub fn enable_output(&self) -> EnableLoggingOutput {
        EnableLoggingOutput {
            logging_enabled: Some(self.logging_enabled),
            bucket_name: self.bucket_name.clone(),
            s3_key_prefix: self.s3_key_prefix.clone(),
            last_successful_delivery_time: None,
            last_failure_time: None,
            last_failure_message: None,
            log_destination_type: self.log_destination_type.clone(),
            log_exports: self.reported_log_exports(),
            s3_tables: None,
        }
    }

    /// This record as the `LoggingStatus` `DescribeLoggingStatus` answers with.
    /// See [`Self::enable_output`].
    pub fn describe_output(&self) -> DescribeLoggingStatusOutput {
        DescribeLoggingStatusOutput {
            logging_enabled: Some(self.logging_enabled),
            bucket_name: self.bucket_name.clone(),
            s3_key_prefix: self.s3_key_prefix.clone(),
            last_successful_delivery_time: None,
            last_failure_time: None,
            last_failure_message: None,
            log_destination_type: self.log_destination_type.clone(),
            log_exports: self.reported_log_exports(),
            s3_tables: None,
        }
    }

    /// This record as the `LoggingStatus` `DisableLogging` answers with. See
    /// [`Self::enable_output`].
    pub fn disable_output(&self) -> DisableLoggingOutput {
        DisableLoggingOutput {
            logging_enabled: Some(self.logging_enabled),
            bucket_name: self.bucket_name.clone(),
            s3_key_prefix: self.s3_key_prefix.clone(),
            last_successful_delivery_time: None,
            last_failure_time: None,
            last_failure_message: None,
            log_destination_type: self.log_destination_type.clone(),
            log_exports: self.reported_log_exports(),
            s3_tables: None,
        }
    }
}

/// Why a `BucketName` outside Amazon S3's own naming rules is refused.
///
/// `smithy_query(query="shape", shape="InvalidS3BucketNameFault")` — "The S3
/// bucket name is invalid. For more information about naming rules, go to Bucket
/// Restrictions and Limitations in the Amazon Simple Storage Service (S3)
/// Developer Guide." The fault's own documented sentence is the fixed text: the
/// rules it points at are the whole of what is wrong, and the name the request
/// carried is already the caller's.
pub const INVALID_S3_BUCKET_NAME: &str =
    "The S3 bucket name is invalid. For more information about naming rules, go to Bucket \
     Restrictions and Limitations in the Amazon Simple Storage Service (S3) Developer Guide.";

/// Why a `BucketName` Amazon S3 holds no bucket under is refused, naming the
/// bucket the request asked for.
///
/// Recorded Amazon Redshift wording for `BucketNotFoundFault` (whose own doc is
/// "Could not find the specified S3 bucket"): the fixed text is `Could not find
/// bucket with name ` followed by the name the request named — the caller has to
/// recognise which bucket it asked for.
pub fn bucket_not_found_message(bucket: &str) -> String {
    format!("Could not find bucket with name {bucket}")
}

/// Why a bucket whose policy admits no log delivery is refused, naming the
/// bucket.
///
/// Recorded Amazon Redshift wording for `InsufficientS3BucketPolicyFault` (whose
/// own doc is "The cluster does not have read bucket or put object permissions on
/// the S3 bucket specified when enabling logging"): the fixed text is `Cannot read
/// ACLs of bucket ` + the name the request named + `. Please ensure that your IAM
/// permissions are set up correctly.` — the service says which bucket it could not
/// read, because that is what the caller has to go and fix.
///
/// The fault's own documented sentence is also the RULE the refusal follows: the
/// bucket's policy has to admit ONE of the two permissions it names (see
/// [`resolve_log_bucket`]).
pub fn insufficient_bucket_policy_message(bucket: &str) -> String {
    format!(
        "Cannot read ACLs of bucket {bucket}. Please ensure that your IAM permissions are set \
         up correctly."
    )
}

/// Why a request asking for S3 delivery without naming a bucket is refused: a
/// request that names no bucket names no bucket Amazon S3 holds, which is what
/// [`INVALID_S3_BUCKET_NAME`] — the fault's own documented sentence — reports.
/// One sentence for every value the member cannot be honoured with, so a client
/// matches on one rather than on two.
pub const MISSING_S3_BUCKET_NAME: &str = INVALID_S3_BUCKET_NAME;

/// What asking Amazon S3 about the bucket an `EnableLogging` names answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogBucketResolution {
    /// S3 holds the bucket and its policy admits log delivery to it.
    Admitted,
    /// S3 answered, and it holds no bucket under that name.
    NotFound,
    /// S3 holds the bucket, and its policy admits neither reading the bucket nor
    /// putting an object in it for this service.
    PolicyInsufficient,
    /// S3 could not be ASKED at all (it is not mounted in this process, or the
    /// answer could not be read), so nothing is known about the bucket. This
    /// service keeps nothing of the bucket but the name it was given, so silence
    /// refuses nothing.
    Unknown,
}

/// The IAM action that READS a bucket, and the one that PUTS an object in it —
/// the two permissions `InsufficientS3BucketPolicyFault` names, and the two the
/// Amazon Redshift Cluster Management Guide's own audit-logging bucket policy
/// grants the log-delivery identity.
const READ_BUCKET_ACTION: &str = "s3:GetBucketAcl";
const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The service principal Amazon Redshift delivers a cluster's audit logs as, and
/// the IAM path of the log-delivery USER the guide's older policy names instead —
/// the two identities a log-delivery bucket policy grants to.
const LOG_DELIVERY_SERVICE_PRINCIPAL: &str = "redshift.amazonaws.com";
const LOG_DELIVERY_USER_PATH: &str = "user/logs";

/// The condition keys the guide's own audit-logging bucket policy scopes its
/// grant by: the account the delivery is made for and the ARN of the cluster the
/// logs are of.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The path audit logs are delivered under inside the bucket, below any
/// `S3KeyPrefix` the request names.
///
/// Source: the Amazon Redshift Cluster Management Guide's "Bucket structure" for
/// audit logging — `AWSLogs/<AccountID>/redshift/<Region>/<year>/<month>/<day>/…`
/// — which is the object the `s3:PutObject` grant has to cover, and so the
/// resource that permission is judged against.
fn delivery_path(account_id: &str, region: &str, key_prefix: Option<&str>) -> String {
    let prefix = key_prefix
        .map(|prefix| format!("{}/", prefix.trim_end_matches('/')))
        .unwrap_or_default();
    format!("{prefix}AWSLogs/{account_id}/redshift/{region}/")
}

/// Amazon S3's ARN vendor, which the endpoint of a hop to it is addressed under,
/// and which a bucket's own ARN names: `arn:aws:s3:::<bucket>` IS the bucket, and
/// an object of it is `arn:aws:s3:::<bucket>/<key>`.
const S3_ARN_NAMESPACE: &str = "s3";

/// Whether the bucket `bucket` is one this service may deliver the audit logs of
/// the cluster `cluster_identifier` to, as Amazon S3 reports it.
///
/// Call it with NO store guard held — it awaits. Two questions, each put with
/// that service's own read: whether it holds the bucket at all
/// (`s3:ListBuckets`, narrowed to the name, since "all bucket names in the
/// response begin with the specified bucket name prefix"), and what the bucket's
/// policy says (`s3:GetBucketPolicy`). The policy is decided by the one IAM
/// evaluator every service in this workspace decides a resource policy with
/// ([`policy_admits_log_delivery`]).
pub async fn resolve_log_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_identifier: &str,
    bucket: &str,
    key_prefix: Option<&str>,
) -> LogBucketResolution {
    let Some(client) = s3(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so Amazon S3 cannot be asked");
        return LogBucketResolution::Unknown;
    };
    match client.list_buckets().prefix(bucket).send().await {
        Ok(answer) => {
            if !answer
                .buckets()
                .iter()
                .any(|held| held.name() == Some(bucket))
            {
                return LogBucketResolution::NotFound;
            }
        }
        Err(error) => {
            unresolved(bucket, &error);
            return LogBucketResolution::Unknown;
        }
    }
    match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(answer) => match answer.policy() {
            Some(policy)
                if policy_admits_log_delivery(
                    policy,
                    region,
                    cluster_identifier,
                    bucket,
                    key_prefix,
                ) =>
            {
                LogBucketResolution::Admitted
            }
            // A bucket whose policy allows the delivery neither permission is one
            // this service cannot write to, which is what the fault reports.
            _ => LogBucketResolution::PolicyInsufficient,
        },
        // S3 answers `NoSuchBucketPolicy` for a bucket that carries no policy at
        // all, which is the same answer for this question: nothing grants the
        // delivery.
        Err(error) if carries_no_policy(&error) => LogBucketResolution::PolicyInsufficient,
        Err(error) => {
            unresolved(bucket, &error);
            LogBucketResolution::Unknown
        }
    }
}

/// Whether `policy` grants the audit-log delivery either permission
/// `InsufficientS3BucketPolicyFault` names: READING the bucket
/// ([`READ_BUCKET_ACTION`] on the bucket itself), or PUTTING an object into the
/// path the logs are delivered under ([`PUT_OBJECT_ACTION`] on
/// [`delivery_path`]).
///
/// Either one is enough, because either one is what the fault reports the absence
/// of — "does not have read bucket OR put object permissions" — so a policy that
/// allows one of them is a policy that admits the delivery, while a bucket whose
/// policy allows neither (or which carries none at all) is one nothing could
/// deliver into.
///
/// The decision is the shared IAM evaluator's
/// ([`service_common::iam_authorization::resource_decision`]), the one every
/// service in this workspace decides a resource policy with — so `Principal`,
/// `Resource`, `Condition` and an explicit `Deny` all count here exactly as they
/// count everywhere else. What this function supplies is who is asking and in
/// what context:
///
/// * the PRINCIPALS are every spelling a log-delivery bucket policy names the
///   delivery by: the service principal the guide's current policy grants to, the
///   log-delivery user its older policy grants to, and this account's own
///   spellings ([`service_common::iam_authorization::account_principals`]), since
///   the delivery is made for this account;
/// * the CONTEXT is what the guide's own policy conditions that grant on: the
///   account the logs are delivered for, and the ARN of the cluster they are the
///   logs of.
fn policy_admits_log_delivery(
    policy: &str,
    region: &str,
    cluster_identifier: &str,
    bucket: &str,
    key_prefix: Option<&str>,
) -> bool {
    use ::service_common::iam_authorization::{
        account_principals, resource_decision, Decision, Request,
    };

    let account_id = ::engine::emulator_account_id().to_string();
    let cluster_arn = super::cluster_arn(cluster_identifier, region);
    let mut principals = vec![
        LOG_DELIVERY_SERVICE_PRINCIPAL.to_string(),
        format!("arn:aws:iam::{account_id}:{LOG_DELIVERY_USER_PATH}"),
    ];
    principals.extend(account_principals(&cluster_arn));
    let context = [
        (SOURCE_ACCOUNT_KEY.to_string(), account_id.clone()),
        (SOURCE_ARN_KEY.to_string(), cluster_arn.clone()),
    ];

    let bucket_arn = format!("arn:aws:{S3_ARN_NAMESPACE}:::{bucket}");
    let object_arn = format!(
        "{bucket_arn}/{}",
        delivery_path(&account_id, region, key_prefix)
    );
    [
        (READ_BUCKET_ACTION, bucket_arn.as_str()),
        (PUT_OBJECT_ACTION, object_arn.as_str()),
    ]
    .iter()
    .any(|(action, resource)| {
        let request = Request::new(action, resource, &principals).with_context(&context);
        resource_decision(policy, &request) == Decision::Allowed
    })
}

/// Whether this refusal of the policy read is Amazon S3 saying the bucket carries
/// NO policy, rather than a failure to ask it at all.
fn carries_no_policy<E: ::aws_sdk_s3::error::ProvideErrorMetadata>(error: &E) -> bool {
    ::aws_sdk_s3::error::ProvideErrorMetadata::code(error) == Some(NO_SUCH_BUCKET_POLICY)
}

/// The error code Amazon S3 answers a policy read of a bucket that carries no
/// policy with, which its own documentation names: `NoSuchBucketPolicy`.
const NO_SUCH_BUCKET_POLICY: &str = "NoSuchBucketPolicy";

/// Trace a bucket this emulator could not ask Amazon S3 about, which is a
/// deployment with no such service mounted rather than a bucket reported missing.
///
/// The caller's own bucket name is rendered in its escaped `Debug` form, so a
/// value carrying a newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(bucket: &str, error: &E) {
    ::tracing::warn!(
        bucket = ?bucket,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "the audit-log bucket a cluster's logging names could not be resolved against Amazon S3"
    );
}

/// An AWS SDK client for Amazon S3 over the in-process callback into this same
/// emulator, under the same client policy as the EC2 client beside it
/// ([`super::ec2_subnets`]): the transport is in process, so there is nothing to
/// retry, time out or watch for a stalled stream.
///
/// Path style, because the emulator serves one endpoint for every service: the
/// bucket travels in the path rather than in a per-bucket host the router could
/// not resolve. The Region is read as a Region LABEL
/// (`service_common::arn::region_label`) before it reaches the endpoint
/// authority, so no caller-supplied text names the host this hop addresses.
fn s3(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_s3::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{S3_ARN_NAMESPACE}.{region}.amazonaws.com"))
        .force_path_style(true)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_s3::Client::from_conf(config))
}
