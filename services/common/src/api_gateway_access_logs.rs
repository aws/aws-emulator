// SPDX-License-Identifier: CC0-1.0

//! The ACCESS LOG one API Gateway stage writes for one request it served: the `$context`
//! format rendered into a record, and the write of that record into the CloudWatch Logs
//! destination the stage names.
//!
//! Both API Gateway planes configure this the same way — a stage carries
//! `AccessLogSettings` with a `destinationArn` ("the Amazon Resource Name of the CloudWatch
//! Logs log group or Kinesis Data Firehose delivery stream to receive access logs") and a
//! `format` ("a single line format of the access logs of data, as specified by selected
//! `$context` variables") — and both render the same variable language against the request
//! they served. So the rendering and the write live here, once, and each plane keeps only
//! what it alone knows: which `$context` values that plane's records produce for a call, and
//! when a stage is configured to log at all.
//!
//! # What a format renders to
//!
//! A format is literal text carrying `$context.<member>` references. Rendering one is a
//! LOOKUP of values the plane read off the request it served and its own records — nothing
//! here evaluates anything the format says — and a reference the plane has no value for
//! renders as [`UNRESOLVED_VALUE`], the single character AWS writes an access-log field it
//! has no value for as. A record is therefore always a complete line: a format naming a
//! variable this emulator does not produce still identifies the request it is about.
//!
//! # Where a record is written
//!
//! `destinationArn` names a log group, and a record is one log event in it, written through
//! the in-process transport into the CloudWatch Logs this emulator mounts: the group is
//! created if the account does not hold it yet (API Gateway provisions the destination it
//! was pointed at), then the stream, then the event. A destination of another kind — a
//! Kinesis Data Firehose delivery stream — names no log group, so [`destination_log_group`]
//! resolves none and the caller reports the delivery as one it does not make rather than
//! writing the record somewhere the client did not ask for.
//!
//! Delivery is BEST EFFORT, as it is for every vended-logs destination: a group the account
//! has since deleted, or a write CloudWatch Logs refuses, loses the record and is reported
//! in tracing rather than failing the request the stage already answered.

use std::collections::BTreeMap;

/// The prefix of every reference an access log format carries: `$context.` then the member
/// path, as `AccessLogSettings$format` documents the language ("selected `$context`
/// variables").
const CONTEXT_PREFIX: &str = "$context.";

/// What a reference the plane has no value for renders as.
///
/// AWS writes an access-log field it has no value for as a single hyphen — the Common Log
/// Format's own placeholder, which the `clf` format API Gateway publishes is written in — so
/// a line keeps its field count whatever the call did or did not carry.
pub const UNRESOLVED_VALUE: &str = "-";

/// The name of the log group an `accessLogSettings` `destinationArn` names, or `None` when
/// it names anything else.
///
/// The reading is this crate's one reading of a log group's ARN
/// ([`crate::cloudwatch_logs::log_group_of_arn`]): both forms name the same group, and a
/// destination of another kind (a Kinesis Data Firehose delivery stream) names no group at
/// all.
#[must_use]
pub fn destination_log_group(destination_arn: &str) -> Option<&str> {
    crate::cloudwatch_logs::log_group_of_arn(destination_arn)
}

/// `format` rendered against `values`, which are keyed by the member path a reference writes
/// after `$context.` (`requestId`, `status`, `identity.sourceIp`).
///
/// One pass over the format: every character is either copied or part of a reference, and a
/// reference is one map lookup. A member path ends where a name ends — the characters a
/// `$context` member path carries are letters, digits, `_` and the `.` that separates its
/// segments — so `$context.status,` renders the status and keeps the comma, and a `$` that
/// opens no reference is literal text.
#[must_use]
pub fn rendered(format: &str, values: &BTreeMap<String, String>) -> String {
    let mut record = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(at) = rest.find(CONTEXT_PREFIX) {
        record.push_str(&rest[..at]);
        let after = &rest[at + CONTEXT_PREFIX.len()..];
        let width = after
            .find(|c: char| !is_member_path_character(c))
            .unwrap_or(after.len());
        // A member path may not END in a separator: `$context.status.` names the status and
        // then a literal full stop, as a sentence in a format does.
        let member = after[..width].trim_end_matches('.');
        if member.is_empty() {
            // `$context.` followed by nothing a member path may carry names no member, so
            // the text stays as it was written.
            record.push_str(CONTEXT_PREFIX);
            rest = after;
            continue;
        }
        record.push_str(
            values
                .get(member)
                .map(String::as_str)
                .unwrap_or(UNRESOLVED_VALUE),
        );
        rest = &after[member.len()..];
    }
    record.push_str(rest);
    record
}

/// Whether `c` is a character a `$context` member path carries: a name's own characters and
/// the `.` that separates its segments (`identity.sourceIp`,
/// `authorizer.claims.sub`).
fn is_member_path_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// Write one access-log record into `log_group`, in the stream `log_stream`.
///
/// The one vended-logs write this crate holds
/// ([`crate::cloudwatch_logs::delivered`]): the group and the stream are created
/// first — API Gateway provisions the destination it was pointed at — and each record is one
/// log event stamped with the instant it is delivered.
///
/// `true` once the records are in the group, `false` when they could not be written, which
/// never fails the request the stage already answered. Call it with NO state guard held — it
/// awaits.
pub async fn delivered(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
    log_stream: &str,
    records: &[String],
) -> bool {
    crate::cloudwatch_logs::delivered(ctx, region, log_group, log_stream, records).await
}

/// [`delivered`] for a plane served on a core-owned SOCKET, which holds the in-process
/// transport itself and no request context (an API Gateway WebSocket api's own endpoint).
///
/// The same write ([`crate::cloudwatch_logs::delivered_over`]), so a stage's record of a
/// request it answered through the front door and its record of a call carried on a socket
/// land in the destination alike.
pub async fn delivered_over(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    log_group: &str,
    log_stream: &str,
    records: &[String],
) -> bool {
    crate::cloudwatch_logs::delivered_over(callback, region, log_group, log_stream, records).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values a rendering below resolves against.
    fn values() -> BTreeMap<String, String> {
        [
            ("requestId", "a1b2c3"),
            ("httpMethod", "GET"),
            ("status", "200"),
            ("identity.sourceIp", "127.0.0.1"),
            ("authorizer.claims.sub", "user-7"),
        ]
        .into_iter()
        .map(|(member, value)| (member.to_string(), value.to_string()))
        .collect()
    }

    #[test]
    fn a_reference_renders_its_value_and_an_unknown_one_renders_the_placeholder() {
        for (case, format, expected) in [
            ("one reference", "$context.requestId", "a1b2c3"),
            (
                "the clf shape, with a member this plane has no value for",
                "$context.identity.sourceIp - $context.identity.user [$context.requestTime]",
                "127.0.0.1 - - [-]",
            ),
            (
                "a json shape",
                r#"{"id":"$context.requestId","status":"$context.status"}"#,
                r#"{"id":"a1b2c3","status":"200"}"#,
            ),
            (
                "a nested member path",
                "$context.authorizer.claims.sub",
                "user-7",
            ),
            (
                "a reference followed by punctuation the format owns",
                "$context.httpMethod, $context.status.",
                "GET, 200.",
            ),
            ("no reference at all", "a literal line", "a literal line"),
            ("a dollar that opens no reference", "$5 and $input", "$5 and $input"),
            ("a prefix with no member after it", "$context. ", "$context. "),
        ] {
            let got = rendered(format, &values());
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?} from {format:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn a_log_group_destination_resolves_and_another_kind_resolves_none() {
        for (case, arn, expected) in [
            (
                "a log group",
                "arn:aws:logs:us-east-1:000000000000:log-group:/apigw/access",
                Some("/apigw/access"),
            ),
            (
                "a log group with everything under it",
                "arn:aws:logs:us-east-1:000000000000:log-group:/apigw/access:*",
                Some("/apigw/access"),
            ),
            (
                "a delivery stream",
                "arn:aws:firehose:us-east-1:000000000000:deliverystream/amazon-apigateway-logs",
                None,
            ),
            ("not an arn at all", "/apigw/access", None),
            (
                "a log group whose name no group may carry",
                "arn:aws:logs:us-east-1:000000000000:log-group:has a space",
                None,
            ),
        ] {
            let got = destination_log_group(arn);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?} for {arn:?}, got {got:?}"
            );
        }
    }
}
