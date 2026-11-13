// SPDX-License-Identifier: CC0-1.0

//! What a caller-supplied CloudWatch Logs LOG GROUP NAME has to be.
//!
//! Several emulated services take a log group name as an input member and then WRITE
//! into that group — an access-log subscription, a delivery destination, a job's log
//! configuration. The value is therefore an instruction to create or write a log group,
//! so it has to be a name CloudWatch Logs can actually hold: the empty string, a name
//! past CloudWatch's ceiling, and a name carrying a character outside the published set
//! each name no group anything could ever be written to.
//!
//! CloudWatch Logs publishes one rule for every such name — 1 to
//! [`MAX_LOG_GROUP_NAME_CHARS`] characters, each of them a letter, a digit or one of the
//! characters [`log_group_name_requirement`] lists — and the services that bind such a
//! member typically declare it as a bare string with no `@length` or `@pattern`, so the
//! framework's constraint walk cannot refuse any of it. The rule lives here, once, so no
//! two services disagree about which names CloudWatch Logs holds; each service keeps only
//! the envelope its own API refuses with (the status, the error shape and which member the
//! refusal is keyed by).
//!
//! Beside the NAME, this module holds the two things a service that WRITES into a group
//! needs: the reading of a log group's own ARN, which is how a caller names the destination
//! it is pointing a service at ([`log_group_of_arn`]), and the vended-logs write itself
//! ([`delivered`]) — the one path a service delivering records on the account's behalf puts
//! them in the group through.
//!
//! `service-com-amazonaws-mediapackage-mediapackage` still carries its own copy of this
//! reading (`state::is_log_group_name`), inlined before this module existed and pinned by
//! that service's own refusal suite; it is to be MIGRATED onto this one rather than copied
//! again, exactly as the claim-rule copies this crate lists are. Nothing new should restate
//! the rule — call this module.

/// The shortest log group name CloudWatch Logs holds — so the empty string names no group.
pub const MIN_LOG_GROUP_NAME_CHARS: usize = 1;

/// The longest log group name CloudWatch Logs holds; see [`MIN_LOG_GROUP_NAME_CHARS`].
///
/// This is the ceiling of the service that WOULD hold the group, not any one emulated
/// service's generic string bound: a longer name is one CloudWatch Logs would never
/// accept, whatever memory it would occupy in a store here.
pub const MAX_LOG_GROUP_NAME_CHARS: usize = 512;

/// The non-alphanumeric characters a log group name may carry, as a refusal names them.
const LOG_GROUP_PUNCTUATION_CHARACTERS: &str = ". - _ / #";

/// Whether `c` is a character a CloudWatch Logs log group name may carry.
///
/// CloudWatch Logs' published set: letters, digits and the characters
/// [`LOG_GROUP_PUNCTUATION_CHARACTERS`] names. The letters-and-digits half is ASCII-ONLY
/// — unlike AWS's tagging restrictions, which say "Unicode letters, digits" — because the
/// log-group rule publishes its set as `a-z`, `A-Z` and `0-9`, so an accented or
/// non-Latin name is not one CloudWatch Logs holds.
///
/// Private: a caller judges a whole NAME ([`is_log_group_name`]), which is the unit the
/// rule is about; nothing outside this module needs the per-character half.
fn is_log_group_name_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '#')
}

/// Whether `value` is a name CloudWatch Logs can hold: between
/// [`MIN_LOG_GROUP_NAME_CHARS`] and [`MAX_LOG_GROUP_NAME_CHARS`] characters, every one of
/// them a letter, a digit or one of the characters
/// [`log_group_name_requirement`] lists.
///
/// The length is counted in CHARACTERS, not bytes, so a name is judged by what the caller
/// wrote rather than by how it happened to encode.
pub fn is_log_group_name(value: &str) -> bool {
    let length = value.chars().count();
    (MIN_LOG_GROUP_NAME_CHARS..=MAX_LOG_GROUP_NAME_CHARS).contains(&length)
        && value.chars().all(is_log_group_name_character)
}

/// The requirement a refused log group name failed: the member's WHOLE accepted domain, in
/// one sentence, so a name refused for its length and one refused for a character it
/// carries are answered with the same rule.
///
/// Built here rather than written at each call site, so the sentence and the bounds it
/// states cannot disagree and a suite asserting it reads the rule's own words.
pub fn log_group_name_requirement() -> String {
    format!(
        "must be {MIN_LOG_GROUP_NAME_CHARS} to {MAX_LOG_GROUP_NAME_CHARS} characters of \
         letters, numbers and `{LOG_GROUP_PUNCTUATION_CHARACTERS}`"
    )
}

/// The log group name prefix CloudWatch Logs keeps for groups of its own: "Log group names
/// can't start with the string aws/" (`CreateLogGroup`'s own documentation), so a name in it
/// is not a caller's to create.
pub const RESERVED_LOG_GROUP_NAME_PREFIX: &str = "aws/";

/// Whether `value` is a name in [`RESERVED_LOG_GROUP_NAME_PREFIX`], which CloudWatch Logs
/// creates no group under however well-formed the rest of it is.
pub fn is_reserved_log_group_name(value: &str) -> bool {
    value.starts_with(RESERVED_LOG_GROUP_NAME_PREFIX)
}

/// What CloudWatch Logs states for a create whose name is in that prefix: the member and the
/// string it may not start with.
///
/// The sentence lives here, once, so the service that owns log groups and any other service
/// that creates one on a caller's behalf cannot word the same refusal differently.
pub fn reserved_log_group_name_message() -> String {
    format!("logGroupName must not start with the string '{RESERVED_LOG_GROUP_NAME_PREFIX}'")
}

/// The suffix the ARN of a log group carries when it names the group TOGETHER with
/// everything under it, which is the form `DescribeLogGroups` reports a group's `arn` in.
const EVERYTHING_UNDER_THE_GROUP: &str = ":*";

/// The resource type a log group's ARN names
/// (`arn:<partition>:logs:<region>:<account>:log-group:<name>`).
const LOG_GROUP_RESOURCE_TYPE: &str = "log-group";

/// The NAME of the log group `arn` names, or `None` when it names anything else.
///
/// Several services take the ARN of a log group as the destination they are to write into
/// — an access-log setting, a vended-logs destination, a web ACL's logging configuration —
/// and both forms of that ARN name the same group: the group on its own, and the group
/// with everything under it ([`EVERYTHING_UNDER_THE_GROUP`], which is how a client reads
/// an ARN off `DescribeLogGroups`). What is left has to be a name CloudWatch Logs can hold
/// ([`is_log_group_name`]), so a destination of another kind (an Amazon S3 bucket, a
/// Firehose delivery stream) resolves no group at all and its caller reports the delivery
/// as one it does not make rather than writing somewhere nobody named.
#[must_use]
pub fn log_group_of_arn(arn: &str) -> Option<&str> {
    let resource = crate::arn::resource_id_of_type(arn, LOG_GROUP_RESOURCE_TYPE)?;
    let name = resource
        .strip_suffix(EVERYTHING_UNDER_THE_GROUP)
        .unwrap_or(resource);
    is_log_group_name(name).then_some(name)
}

/// Write one log event per entry of `records` into `log_group`, in the stream `log_stream`.
///
/// The VENDED-LOGS write: a service delivering records on the account's behalf — an API
/// Gateway stage's access log, the record a WAFv2 web ACL logs about a request it decided —
/// writes them here, once, so no two of them deliver differently. The group and the stream
/// are created first, because a service was POINTED at that destination and provisions it,
/// and a group or stream already there is the one it has been writing to; each record is one
/// log event stamped with the instant it is delivered, which is what keeps it inside
/// CloudWatch Logs' ingestion window.
///
/// The write goes over the in-process transport into the CloudWatch Logs this emulator
/// mounts, so a delivered record and `GetLogEvents` / `FilterLogEvents` cannot disagree
/// about it.
///
/// Delivery is BEST EFFORT, as it is for every vended-logs destination: a group the account
/// has since deleted, or a write CloudWatch Logs refuses, loses the record and is reported
/// in tracing rather than failing the request the caller has already answered. `true` once
/// the records are in the group, `false` when they could not be written. Call it with NO
/// state guard held — it awaits.
pub async fn delivered(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
    log_stream: &str,
    records: &[String],
) -> bool {
    delivered_over(ctx.callback(), region, log_group, log_stream, records).await
}

/// [`delivered`] for a deliverer that holds the in-process transport ITSELF rather than a
/// request's context: a data plane served on a core-owned socket (an API Gateway WebSocket
/// api's own endpoint) has no `OperationContext` at all, and the handle is the one piece the
/// write needs.
///
/// The same write, so a record delivered from a socket and one delivered from an operation
/// cannot land differently.
pub async fn delivered_over(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    log_group: &str,
    log_stream: &str,
    records: &[String],
) -> bool {
    if records.is_empty() {
        return true;
    }
    let client = logs_client(callback, region);
    match client
        .create_log_group()
        .log_group_name(log_group)
        .send()
        .await
    {
        Ok(_) => {}
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                log_group = %log_group,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the vended-logs destination could not be created, so these records are not \
                 delivered"
            );
            return false;
        }
    }
    match client
        .create_log_stream()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .send()
        .await
    {
        Ok(_) => {}
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                log_group = %log_group,
                log_stream = %log_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the vended-logs stream could not be created, so these records are not \
                 delivered"
            );
            return false;
        }
    }
    let at = epoch_millis(::std::time::SystemTime::now());
    let events: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent> = records
        .iter()
        .filter_map(|record| {
            ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
                .timestamp(at)
                .message(record)
                .build()
                .ok()
        })
        .collect();
    let count = events.len();
    if let Err(error) = client
        .put_log_events()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .set_log_events(Some(events))
        .send()
        .await
    {
        ::tracing::warn!(
            log_group = %log_group,
            log_stream = %log_stream,
            records = count,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "these records could not be delivered to CloudWatch Logs"
        );
        return false;
    }
    true
}

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no network client is configured here. `region` is the Region the
/// delivering service's own records name, so a record lands where the destination's ARN says
/// it does.
fn logs_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// `at` in milliseconds since the Unix epoch, which is the stamp a log event carries.
///
/// An instant before the epoch, which no log event can carry, clamps to the epoch rather
/// than wrapping.
fn epoch_millis(at: ::std::time::SystemTime) -> i64 {
    at.duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_group_arn_resolves_in_both_published_forms_and_nothing_else_does() {
        for (case, arn, expected) in [
            (
                "a log group",
                "arn:aws:logs:us-east-1:000000000000:log-group:/vended/one",
                Some("/vended/one"),
            ),
            (
                "a log group with everything under it",
                "arn:aws:logs:us-east-1:000000000000:log-group:aws-waf-logs-guard:*",
                Some("aws-waf-logs-guard"),
            ),
            (
                "a bucket, which names no log group",
                "arn:aws:s3:::aws-waf-logs-guard",
                None,
            ),
            ("not an arn at all", "aws-waf-logs-guard", None),
            (
                "a log group whose name no group may carry",
                "arn:aws:logs:us-east-1:000000000000:log-group:has a space",
                None,
            ),
        ] {
            let got = log_group_of_arn(arn);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?} for {arn:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn the_published_domain_is_admitted_and_nothing_else_is() {
        for name in [
            "a",
            "7",
            "Z",
            "_",
            "-",
            ".",
            "/",
            "#",
            "/aws/vendedlogs/states/one",
            "UPPER/lower/MiXeD-0.9_#",
        ] {
            assert!(
                is_log_group_name(name),
                "case {name:?}: CloudWatch Logs holds this name, got rejected"
            );
        }
        for name in [
            "",
            "has a space",
            "group!",
            "group$",
            "group*",
            "group:name",
            "grüße",
            "日本語",
        ] {
            assert!(
                !is_log_group_name(name),
                "case {name:?}: CloudWatch Logs holds no such group, got accepted"
            );
        }
    }

    #[test]
    fn the_length_bound_holds_at_each_published_edge() {
        for (length, acceptable) in [
            (MIN_LOG_GROUP_NAME_CHARS - 1, false),
            (MIN_LOG_GROUP_NAME_CHARS, true),
            (MIN_LOG_GROUP_NAME_CHARS + 1, true),
            (MAX_LOG_GROUP_NAME_CHARS - 1, true),
            (MAX_LOG_GROUP_NAME_CHARS, true),
            (MAX_LOG_GROUP_NAME_CHARS + 1, false),
        ] {
            let name = "g".repeat(length);
            assert_eq!(
                is_log_group_name(&name),
                acceptable,
                "case {length} characters: expected acceptable={acceptable}, got {}",
                is_log_group_name(&name)
            );
        }
    }
}
