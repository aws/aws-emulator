// SPDX-License-Identifier: CC0-1.0

//! STANDARD LOGGING to Amazon S3: whether the bucket a `DistributionConfig` names can
//! receive the distribution's access logs, and the grant that lets CloudFront write them.
//!
//! ## The rule this module implements
//!
//! CloudFront delivers a standard access log by PUTting an object into the bucket the
//! configuration names, as the log-delivery account rather than as the caller — so the
//! bucket has to be one whose access control list can carry a grant to that account. A
//! bucket whose Object Ownership setting is the bucket-owner-enforced one has ACLs
//! DISABLED: "requests to set ACLs or update ACLs fail", so no such grant can ever exist on
//! it and no log could ever be delivered. A configuration that enables logging to such a
//! bucket is therefore refused where it is submitted (`InvalidArgument`, HTTP 400), and one
//! accepted against a bucket whose ACLs are enabled leaves the log-delivery account holding
//! `FULL_CONTROL` on that bucket — which is what a client reading the bucket's ACL sees,
//! and what makes the delivery possible at all.
//!
//! Both halves are the SAME question about the target bucket, asked once
//! ([`resolve_access_logs_target`]) and answered by Amazon S3 itself rather than decided
//! here: this emulator holds an object store of its own, reached the way any emulated
//! service reaches another — the AWS SDK over the emulator's in-process transport, through
//! the operation callback ([`smithy_server_runtime::OperationContext::callback`]), which
//! opens no socket, resolves no name and reaches nothing outside this process
//! ([`crate::state::ca_certificates_bundle`] reaches it the same way).
//!
//! ## What a read establishes, and what it does not
//!
//! The refusal and the grant both follow from what the object store ANSWERED about the
//! bucket's Object Ownership setting:
//!
//! * it reports the bucket-owner-enforced setting, so ACLs are disabled and the
//!   configuration is refused ([`AccessLogsTarget::AclsDisabled`]);
//! * it reports another setting, or answers that the bucket has no ownership controls
//!   associated with it at all — which is the state a withdrawn `OwnershipControls` leaves a
//!   bucket in, and one whose ACLs affect permissions again — so the configuration is
//!   accepted and the log-delivery grant is applied
//!   ([`AccessLogsTarget::AclsEnabled`]);
//! * nothing answered about that bucket — no object store is mounted to ask, the store does
//!   not hold the bucket, or the read was refused — so NOTHING is established about it
//!   ([`AccessLogsTarget::Unestablished`]). A configuration is then neither refused (the
//!   caller is not told something no read found out) nor followed by a grant on a bucket
//!   nothing confirmed.
//!
//! The grant is applied only once the configuration has been STORED, so a create or update
//! that is refused for any other reason leaves the target bucket's ACL exactly as it was.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::ops::types::DistributionConfig;

/// The canonical user id of the account CloudFront delivers standard access logs as.
///
/// AWS publishes it for exactly this purpose — standard logging "uses the following
/// canonical ID for the `awslogsdelivery` account" — and it is the id a client finds in the
/// log bucket's ACL once logging is enabled, so it is what the grant below names the
/// grantee by.
pub const LOG_DELIVERY_CANONICAL_USER_ID: &str =
    "c4c1ede66af53448b93c283ce9448c4ba468c9432aa01d700d3878632f77d2d0";

/// The Object Ownership setting that DISABLES a bucket's ACLs, and so the one setting a
/// standard-logging target cannot carry (`ObjectOwnership`'s own value list, as Amazon S3
/// publishes it).
const BUCKET_OWNER_ENFORCED: &str = "BucketOwnerEnforced";

/// The error code Amazon S3 answers a read of the ownership controls of a bucket that has
/// none associated with it: the state a withdrawn `OwnershipControls` leaves a bucket in.
const OWNERSHIP_CONTROLS_NOT_FOUND: &str = "OwnershipControlsNotFoundError";

/// What a read of the bucket a `DistributionConfig`'s `Logging` names established about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessLogsTarget {
    /// The configuration enables no logging, names no bucket, or names a host that is no
    /// Amazon S3 bucket endpoint at all — so there is nothing to refuse and nothing to
    /// grant.
    NotEnabled,
    /// Logging is enabled against a bucket the object store reports ACLs ENABLED on, held
    /// in the Region its endpoint host addresses.
    AclsEnabled { bucket: String, region: String },
    /// Logging is enabled against a bucket the object store reports ACLs DISABLED on, which
    /// no access log could be delivered to. The value carried is the one the request's
    /// `Logging.Bucket` named, which is what the refusal reports back.
    AclsDisabled { specified: String },
    /// Logging is enabled against a bucket nothing answered about: no object store is
    /// mounted to ask, it does not hold the bucket, or it refused the read.
    Unestablished,
}

impl AccessLogsTarget {
    /// The sentence a submitted configuration is REFUSED with, or `None` for a target
    /// nothing established against the rule.
    ///
    /// Only the bucket-owner-enforced answer is a refusal: it is the one answer that says
    /// the grant standard logging needs can never exist on that bucket.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        match self {
            Self::AclsDisabled { specified } => Some(acl_access_not_enabled_message(specified)),
            _ => None,
        }
    }

    /// Leave the log-delivery account holding `FULL_CONTROL` on the target bucket, through
    /// Amazon S3's own ACL calls.
    ///
    /// The bucket's CURRENT ACL is read first and the grant is ADDED to it, because
    /// `PutBucketAcl` replaces a bucket's access control list in full: the grants the bucket
    /// already carried — the owner's own among them — are kept, so enabling logging takes
    /// nothing away from anyone. A bucket that already carries the grant is left exactly as
    /// it is, so enabling logging twice writes nothing the second time.
    ///
    /// Nothing is reported back: the distribution's configuration is already stored by the
    /// time this runs, and the object store's own answer about its bucket is not a verdict
    /// on the call that stored it. A store that refused the change is WARNED about, because a
    /// deployment in which no log bucket can be granted to is worth noticing.
    pub async fn grant_log_delivery(&self, ctx: &::smithy_server_runtime::OperationContext<'_>) {
        let Self::AclsEnabled { bucket, region } = self else {
            return;
        };
        let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
        let current = match client.get_bucket_acl().bucket(bucket).send().await {
            Ok(current) => current,
            Err(error) => {
                ::tracing::warn!(
                    bucket = %bucket,
                    error = ?error,
                    "the emulator's Amazon S3 did not report the access control list of a \
                     standard-logging bucket"
                );
                return;
            }
        };
        if current.grants().iter().any(holds_log_delivery) {
            return;
        }
        let Ok(grantee) = ::aws_sdk_s3::types::Grantee::builder()
            .r#type(::aws_sdk_s3::types::Type::CanonicalUser)
            .id(LOG_DELIVERY_CANONICAL_USER_ID)
            .build()
        else {
            return;
        };
        let mut grants = current.grants().to_vec();
        grants.push(
            ::aws_sdk_s3::types::Grant::builder()
                .grantee(grantee)
                .permission(::aws_sdk_s3::types::Permission::FullControl)
                .build(),
        );
        let mut policy = ::aws_sdk_s3::types::AccessControlPolicy::builder();
        // The owner the store itself reports for the bucket: an ACL cannot transfer a
        // bucket, so the policy names the owner the read answered with rather than one
        // composed here.
        if let Some(owner) = current.owner() {
            policy = policy.owner(owner.clone());
        }
        let policy = policy.set_grants(Some(grants)).build();
        if let Err(error) = client
            .put_bucket_acl()
            .bucket(bucket)
            .access_control_policy(policy)
            .send()
            .await
        {
            ::tracing::warn!(
                bucket = %bucket,
                error = ?error,
                "the emulator's Amazon S3 refused the log-delivery grant on a \
                 standard-logging bucket"
            );
        }
    }
}

/// Whether one grant of a bucket's ACL is the log-delivery account's `FULL_CONTROL`.
///
/// Judged on the canonical user id alone ([`LOG_DELIVERY_CANONICAL_USER_ID`]), which is what
/// identifies the grantee; a display name is a label the store may or may not resolve for
/// one.
fn holds_log_delivery(grant: &::aws_sdk_s3::types::Grant) -> bool {
    grant.permission() == Some(&::aws_sdk_s3::types::Permission::FullControl)
        && grant
            .grantee()
            .and_then(::aws_sdk_s3::types::Grantee::id)
            .is_some_and(|id| id == LOG_DELIVERY_CANONICAL_USER_ID)
}

/// What the bucket a configuration's `Logging` names establishes about standard logging to
/// it.
///
/// A configuration whose `Logging` is absent, whose `Enabled` is false or whose `Bucket` is
/// empty enables no logging at all — "if you don't want to enable logging when you create a
/// distribution ... specify `false` for `Enabled`, and specify empty `Bucket` and `Prefix`
/// elements" — so nothing is asked of the object store for one.
pub async fn resolve_access_logs_target(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    config: &DistributionConfig,
) -> AccessLogsTarget {
    let Some(logging) = config.logging.as_ref() else {
        return AccessLogsTarget::NotEnabled;
    };
    if logging.enabled != Some(true) {
        return AccessLogsTarget::NotEnabled;
    }
    let Some(domain_name) = logging.bucket.as_deref().map(str::trim).filter(|bucket| !bucket.is_empty())
    else {
        return AccessLogsTarget::NotEnabled;
    };
    // "The Amazon S3 bucket to store the access logs in, for example,
    // `amzn-s3-demo-bucket.s3.amazonaws.com`" (`LoggingConfig$Bucket`): the member carries
    // the bucket's ENDPOINT HOST, so the bucket it names and the Region that host addresses
    // are read by the one reading of an Amazon S3 endpoint host this crate has
    // (`crate::service::origin_access`, which reads an S3 origin's host the same way).
    let Some((bucket, region)) = crate::service::origin_access::s3_endpoint_bucket(domain_name)
    else {
        return AccessLogsTarget::NotEnabled;
    };
    let client = ::service_common::s3_client::s3_client(ctx.callback(), &region);
    match client
        .get_bucket_ownership_controls()
        .bucket(&bucket)
        .send()
        .await
    {
        Ok(answered) => {
            // "The rules the container names are what the bucket's effective setting
            // becomes: the LAST one wins", which is how the object store itself reads a
            // container of rules.
            let effective = answered
                .ownership_controls()
                .map(|controls| controls.rules())
                .and_then(<[_]>::last)
                .map(|rule| rule.object_ownership().as_str().to_string());
            match effective.as_deref() {
                Some(BUCKET_OWNER_ENFORCED) => AccessLogsTarget::AclsDisabled {
                    specified: domain_name.to_string(),
                },
                Some(_) => AccessLogsTarget::AclsEnabled { bucket, region },
                // A container carrying no rule settles nothing about the bucket.
                None => AccessLogsTarget::Unestablished,
            }
        }
        // The bucket has no ownership controls associated with it, which is the state a
        // withdrawn container leaves it in — and a bucket without controls is one whose
        // ACLs affect permissions.
        Err(error) if error.code() == Some(OWNERSHIP_CONTROLS_NOT_FOUND) => {
            AccessLogsTarget::AclsEnabled { bucket, region }
        }
        Err(error) => {
            ::tracing::debug!(
                bucket = %bucket,
                error = ?error,
                "nothing was established about a standard-logging bucket's Object Ownership \
                 setting"
            );
            AccessLogsTarget::Unestablished
        }
    }
}

/// How CloudFront words the refusal of a `DistributionConfig` whose standard logging names a
/// bucket with ACLs disabled.
///
/// The fixed text is the service's own sentence for the class; the only variable part is the
/// bucket, named by the value the request's own `LoggingConfig$Bucket` SPECIFIED — "the S3
/// bucket that you specified" — which is that bucket's endpoint host.
#[must_use]
pub fn acl_access_not_enabled_message(specified: &str) -> String {
    format!(
        "The S3 bucket that you specified for CloudFront logs does not enable ACL access: \
         {specified}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::LoggingConfig;

    /// A configuration whose `Logging` is `logging`.
    fn config_with(logging: Option<LoggingConfig>) -> DistributionConfig {
        DistributionConfig {
            caller_reference: "a-reference".to_string(),
            logging,
            ..DistributionConfig::default()
        }
    }

    /// RULE: only a configuration that ENABLES logging against a named bucket asks anything
    /// of the object store — so every other shape of `Logging` resolves without a read.
    #[test]
    fn a_configuration_that_enables_no_logging_names_no_target() {
        let rows: &[(&str, Option<LoggingConfig>)] = &[
            ("no Logging element at all", None),
            (
                "logging switched off",
                Some(LoggingConfig {
                    enabled: Some(false),
                    bucket: Some("logs.s3.amazonaws.com".to_string()),
                    ..LoggingConfig::default()
                }),
            ),
            (
                "logging on, with no bucket",
                Some(LoggingConfig {
                    enabled: Some(true),
                    bucket: Some(String::new()),
                    ..LoggingConfig::default()
                }),
            ),
            (
                "logging on, against a host that names no bucket",
                Some(LoggingConfig {
                    enabled: Some(true),
                    bucket: Some("s3.us-east-1.amazonaws.com".to_string()),
                    ..LoggingConfig::default()
                }),
            ),
        ];
        for (case, logging) in rows {
            let config = config_with(logging.clone());
            let enabled = config
                .logging
                .as_ref()
                .is_some_and(|logging| logging.enabled == Some(true));
            let bucket = config
                .logging
                .as_ref()
                .and_then(|logging| logging.bucket.clone())
                .filter(|bucket| !bucket.trim().is_empty())
                .and_then(|bucket| {
                    crate::service::origin_access::s3_endpoint_bucket(bucket.trim())
                });
            assert!(
                !enabled || bucket.is_none(),
                "case {case:?}: expected no standard-logging target, got {bucket:?}"
            );
        }
    }

    /// RULE: the bucket-owner-enforced answer is the one that refuses the configuration;
    /// every other outcome of the read refuses nothing.
    #[test]
    fn only_a_bucket_with_acls_disabled_is_refused() {
        let rows: &[(&str, AccessLogsTarget, bool)] = &[
            (
                "a bucket with ACLs disabled",
                AccessLogsTarget::AclsDisabled {
                    specified: "logs.s3.amazonaws.com".to_string(),
                },
                true,
            ),
            (
                "a bucket with ACLs enabled",
                AccessLogsTarget::AclsEnabled {
                    bucket: "logs".to_string(),
                    region: "us-east-1".to_string(),
                },
                false,
            ),
            ("a configuration enabling no logging", AccessLogsTarget::NotEnabled, false),
            ("a bucket nothing answered about", AccessLogsTarget::Unestablished, false),
        ];
        for (case, target, refused) in rows {
            let actual = target.refusal();
            assert_eq!(
                actual.is_some(),
                *refused,
                "case {case:?}: expected refused={refused}, got {actual:?}"
            );
        }
    }

    /// RULE: the refusal names the bucket the request itself specified.
    #[test]
    fn the_refusal_names_the_bucket_the_request_named() {
        for specified in [
            "logs-one.s3.amazonaws.com",
            "another.log.bucket.s3.us-west-2.amazonaws.com",
        ] {
            let message = acl_access_not_enabled_message(specified);
            assert!(
                message.ends_with(&format!("ACL access: {specified}")),
                "case {specified:?}: expected the message to name the bucket as specified, got \
                 {message:?}"
            );
        }
    }

    /// RULE: a grant is the log-delivery account's only when it names that canonical user
    /// AND carries `FULL_CONTROL`.
    #[test]
    fn the_log_delivery_grant_is_recognised_by_its_canonical_user_and_permission() {
        let grant = |id: &str, permission: ::aws_sdk_s3::types::Permission| {
            ::aws_sdk_s3::types::Grant::builder()
                .grantee(
                    ::aws_sdk_s3::types::Grantee::builder()
                        .r#type(::aws_sdk_s3::types::Type::CanonicalUser)
                        .id(id)
                        .build()
                        .expect("a canonical user grantee"),
                )
                .permission(permission)
                .build()
        };
        let rows: &[(&str, ::aws_sdk_s3::types::Grant, bool)] = &[
            (
                "the log-delivery account's full control",
                grant(
                    LOG_DELIVERY_CANONICAL_USER_ID,
                    ::aws_sdk_s3::types::Permission::FullControl,
                ),
                true,
            ),
            (
                "the log-delivery account, with less than full control",
                grant(
                    LOG_DELIVERY_CANONICAL_USER_ID,
                    ::aws_sdk_s3::types::Permission::Write,
                ),
                false,
            ),
            (
                "another account's full control",
                grant("00112233445566778899", ::aws_sdk_s3::types::Permission::FullControl),
                false,
            ),
        ];
        for (case, grant, expected) in rows {
            let actual = holds_log_delivery(grant);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }
}
