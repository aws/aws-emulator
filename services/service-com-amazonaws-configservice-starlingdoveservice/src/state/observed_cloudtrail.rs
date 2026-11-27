// SPDX-License-Identifier: CC0-1.0

//! What a pass OBSERVES in AWS CloudTrail: the trails this account holds, and whether each
//! one is LOGGING.
//!
//! An account-wide Config managed rule about the account's audit trail is evaluated against
//! what the service that OWNS trails reports (`super::account_rule`), so the trails are read
//! through CloudTrail's own API over the in-process callback, exactly as the IAM and Amazon
//! EC2 observations beside it are: this crate names no other service crate, and a trail
//! CloudTrail does not report is not observed at all rather than observed as something else.
//!
//! Whether a trail is logging is the trail's own STATUS rather than part of its
//! configuration (`GetTrailStatus`), so one observation of a trail is two reads of the same
//! service — and a read that FAILED leaves the account's trails unobserved rather than
//! observed as a trail that is not logging.

/// One trail CloudTrail reports for this account, as an account-wide rule reads it.
///
/// Every member comes from a CloudTrail read named on it; nothing else is observed, because
/// nothing else is read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedTrail {
    /// Whether this trail "is currently logging Amazon Web Services API calls"
    /// (`GetTrailStatusResponse$IsLogging`), which is what makes a trail ENABLED.
    pub is_logging: bool,
    /// "Name of the Amazon S3 bucket into which CloudTrail delivers your trail files"
    /// (`Trail$S3BucketName`), absent for a trail CloudTrail reported without one.
    pub s3_bucket_name: Option<String>,
    /// "Specifies the ARN of the Amazon SNS topic that CloudTrail uses to send notifications
    /// when log files are delivered" (`Trail$SnsTopicARN`).
    pub sns_topic_arn: Option<String>,
    /// "Specifies an Amazon Resource Name (ARN), a unique identifier that represents the log
    /// group to which CloudTrail logs will be delivered"
    /// (`Trail$CloudWatchLogsLogGroupArn`).
    pub cloud_watch_logs_log_group_arn: Option<String>,
}

/// How many trails ONE observation reads the status of.
///
/// A HOST bound of the same kind as the one on the pages of IAM roles a pass reads
/// ([`super::observed_iam`]): each trail's logging state is one more in-process read, so an
/// account holding trail after trail cannot turn one observation into an unbounded run of
/// calls. Far more than the five trails per Region CloudTrail's own quota allows, so no
/// account this emulator can hold is observed in part.
const MAX_OBSERVED_TRAILS: usize = 50;

/// The trails this account holds, as CloudTrail reports them, or `None` when CloudTrail
/// could not be read — which is NOT the same as an account that holds no trail.
///
/// `DescribeTrails` answers the trails of this Region together with the shadow trails of the
/// multi-Region trails that also log here, which is what an account-wide rule about the
/// account's audit trail is evaluated against; the logging state of each is its own
/// `GetTrailStatus`.
pub async fn observed_trails(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> Option<Vec<ObservedTrail>> {
    let client = cloudtrail_client(callback, region);
    let described = match client.describe_trails().send().await {
        Ok(described) => described,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Config could not read this account's CloudTrail trails"
            );
            return None;
        }
    };

    let mut observed = Vec::new();
    for trail in described.trail_list().iter().take(MAX_OBSERVED_TRAILS) {
        // A trail is addressed by its ARN where CloudTrail reported one, and by its name
        // otherwise; a trail it reported with neither cannot be read back at all.
        let Some(addressed) = trail
            .trail_arn
            .as_deref()
            .or(trail.name.as_deref())
            .filter(|addressed| !addressed.is_empty())
        else {
            continue;
        };
        let status = match client.get_trail_status().name(addressed).send().await {
            Ok(status) => status,
            Err(error) => {
                // Nothing is known about THIS trail's logging, so nothing is concluded
                // about the account: the whole observation is answered as an unread one.
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "Config could not read the logging status of one of this account's \
                     CloudTrail trails"
                );
                return None;
            }
        };
        observed.push(ObservedTrail {
            is_logging: status.is_logging.unwrap_or(false),
            s3_bucket_name: trail.s3_bucket_name.clone(),
            sns_topic_arn: trail.sns_topic_arn.clone(),
            cloud_watch_logs_log_group_arn: trail.cloud_watch_logs_log_group_arn.clone(),
        });
    }
    Some(observed)
}

/// An AWS CloudTrail client over the in-process callback, built the way this crate's other
/// observation clients are ([`super::observed_ec2`]): the callback's own HTTP client, so the
/// endpoint below is only the address that transport routes on IN PROCESS — no socket is
/// opened and nothing leaves this process — and the callback's credentials, which sign this
/// in-process hop.
fn cloudtrail_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_cloudtrail::Client {
    let config = ::aws_sdk_cloudtrail::Config::builder()
        .behavior_version(::aws_sdk_cloudtrail::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudtrail::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://cloudtrail.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_cloudtrail::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudtrail::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudtrail::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudtrail::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudtrail::Client::from_conf(config)
}
