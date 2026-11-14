// SPDX-License-Identifier: CC0-1.0

//! The OBJECT a web ACL's log record is delivered as when its logging destination names an
//! Amazon Simple Storage Service bucket.
//!
//! A logging destination is "an Amazon CloudWatch Logs log group, an Amazon Simple Storage
//! Service (Amazon S3) bucket, or an Amazon Kinesis Data Firehose"
//! (`PutLoggingConfiguration`), and each kind is READ differently: a log group is read with
//! `GetLogEvents`, while a bucket is read by listing it and fetching the objects. So a
//! record delivered into a bucket has to be the object a reader of that bucket fetches —
//! WAF's published log-file form, gzipped newline-delimited JSON under WAF's published key
//! layout — or the destination a client pointed the web ACL at holds nothing it can read.
//!
//! Amazon S3 is reached the way any emulated service reaches another: the AWS SDK over the
//! emulator's own in-process transport, through the operation callback, with the one
//! construction of that client this tree holds ([`::service_common::s3_client`]). Delivery is
//! BEST EFFORT, as every vended-logs delivery is — a bucket the account has since deleted, or
//! a write the object store refuses, loses the record and is traced rather than failing a
//! request that has already been answered.

use ::std::io::Write;

/// The first segment of every delivered log-file key, which is the prefix an audit pipeline
/// reads a bucket of vended logs under.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";

/// The segment naming WHOSE logs a prefix holds, which is also the middle field of a log
/// file's own name (lower-cased there, as WAF writes it).
const LOG_FILE_SERVICE_SEGMENT: &str = "WAFLogs";

/// [`LOG_FILE_SERVICE_SEGMENT`] as it appears in a log file's NAME.
const LOG_FILE_NAME_SERVICE_FIELD: &str = "waflogs";

/// What a delivered log file's name ends with: WAF's log records, gzipped.
const LOG_FILE_SUFFIX: &str = ".log.gz";

/// The media type of the object a log file is: a gzip stream.
const GZIP_MEDIA_TYPE: &str = "application/x-gzip";

/// How many characters of the unique field a log file's name ends with — the field that
/// tells two files of the same minute apart.
const UNIQUE_FIELD_CHARS: usize = 8;

/// Write `records` into the object `key` of `bucket`, one line of JSON each.
///
/// `region` is where the object store is addressed; `true` once the object is in the
/// bucket, `false` when it could not be written. Call it with NO state guard held — it
/// awaits.
pub(super) async fn delivered(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    records: &[String],
) -> bool {
    let Some(body) = gzipped(records) else {
        ::tracing::warn!(
            bucket = %bucket,
            key = %key,
            "this web acl's log records could not be framed, so no object was delivered"
        );
        return false;
    };
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    if let Err(error) = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(GZIP_MEDIA_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
    {
        ::tracing::warn!(
            bucket = %bucket,
            key = %key,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "these log records could not be delivered to the destination bucket"
        );
        return false;
    }
    true
}

/// The object's bytes: the records as newline-delimited JSON, gzipped — or `None` when the
/// framing itself failed.
///
/// A delivered log file holds ONE JSON record per line, each terminated, and the file is a
/// gzip stream: that is what the object IS rather than a transport encoding, so a reader
/// fetches the object and decompresses it. Writing into a `Vec` cannot fail for want of
/// space, so a failure here would be the encoder's own; it answers `None` rather than a
/// truncated stream, because no object at all is more honest than one a reader cannot
/// decompress.
fn gzipped(records: &[String]) -> Option<Vec<u8>> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    for record in records {
        encoder.write_all(record.as_bytes()).ok()?;
        encoder.write_all(b"\n").ok()?;
    }
    encoder.finish().ok()
}

/// The prefix every log file of one web ACL is delivered under:
/// `AWSLogs/<account-id>/WAFLogs/<region>/<web-acl-name>/`, with a trailing separator.
///
/// The one place this layout is written, for both readers of it: the delivery that writes the
/// keys ([`log_file_key`]) and the suites that assert what a destination bucket holds.
pub(super) fn log_file_prefix(account_id: &str, region: &str, web_acl_name: &str) -> String {
    format!(
        "{LOG_FILE_ROOT_SEGMENT}/{account_id}/{LOG_FILE_SERVICE_SEGMENT}/{region}/\
         {web_acl_name}/"
    )
}

/// The key one delivered log file takes: [`log_file_prefix`], then the MINUTE the records
/// belong to as a path, then the file's own name —
/// `<account-id>_waflogs_<region>_<web-acl-name>_<YYYYMMDDTHHmmZ>_<unique>.log.gz`.
///
/// `at_millis` is the instant the records were produced at, read as UTC calendar fields
/// (the maintained calendar crate, never by slicing a rendered stamp); `unique` is what
/// tells two files of the same minute apart ([`unique_field`]).
pub(super) fn log_file_key(
    account_id: &str,
    region: &str,
    web_acl_name: &str,
    at_millis: i64,
    unique: &str,
) -> String {
    let at = ::chrono::DateTime::<::chrono::Utc>::from_timestamp_millis(at_millis)
        .unwrap_or(::chrono::DateTime::<::chrono::Utc>::UNIX_EPOCH)
        .naive_utc();
    let minute_path = at.format("%Y/%m/%d/%H/%M");
    let minute = at.format("%Y%m%dT%H%MZ");
    let prefix = log_file_prefix(account_id, region, web_acl_name);
    format!(
        "{prefix}{minute_path}/\
         {account_id}_{LOG_FILE_NAME_SERVICE_FIELD}_{region}_{web_acl_name}_{minute}_{unique}\
         {LOG_FILE_SUFFIX}"
    )
}

/// The unique field one log file's name ends with: [`UNIQUE_FIELD_CHARS`] hex characters
/// drawn fresh, so two files delivered in the same minute for the same web ACL are two
/// objects and not one overwriting the other.
///
/// Drawn from the same entropy source this service's minted ids come from, never derived
/// from the request or from the records: the field identifies the FILE and nothing about its
/// contents.
pub(super) fn unique_field() -> String {
    ::hex::encode(crate::state::fresh_entropy())
        .chars()
        .take(UNIQUE_FIELD_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use ::std::io::Read;

    use super::*;

    /// An instant with a known UTC calendar rendering: 2026-10-28T19:50:03Z.
    fn at_millis() -> i64 {
        ::chrono::DateTime::parse_from_rfc3339("2026-10-28T19:50:03Z")
            .expect("the fixture instant is a well-formed timestamp")
            .timestamp_millis()
    }

    /// RULE: the key is the published layout — the log-file prefix down to the web ACL, the
    /// MINUTE the records belong to as a path, and a file name carrying the account, WAF,
    /// the Region, the web ACL and that minute.
    #[test]
    fn a_log_file_key_carries_the_published_layout() {
        // (case, account, region, web acl name, the key)
        let rows = [
            (
                "a REGIONAL web acl",
                "000000000000",
                "us-west-2",
                "guard",
                "AWSLogs/000000000000/WAFLogs/us-west-2/guard/2026/10/28/19/50/\
                 000000000000_waflogs_us-west-2_guard_20261028T1950Z_abcdef01.log.gz",
            ),
            (
                "another account, Region and name",
                "123456789012",
                "eu-central-1",
                "edge-acl",
                "AWSLogs/123456789012/WAFLogs/eu-central-1/edge-acl/2026/10/28/19/50/\
                 123456789012_waflogs_eu-central-1_edge-acl_20261028T1950Z_abcdef01.log.gz",
            ),
            (
                "a CLOUDFRONT-scoped web acl, whose files land under the distribution \
                 Region segment",
                "000000000000",
                "cloudfront",
                "global-acl",
                "AWSLogs/000000000000/WAFLogs/cloudfront/global-acl/2026/10/28/19/50/\
                 000000000000_waflogs_cloudfront_global-acl_20261028T1950Z_abcdef01.log.gz",
            ),
        ];
        for (case, account, region, name, expected) in rows {
            let actual = log_file_key(account, region, name, at_millis(), "abcdef01");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                actual.starts_with(&log_file_prefix(account, region, name)),
                "case {case:?}: expected the key under the log-file prefix, got {actual:?}"
            );
        }
    }

    /// RULE: the object is a gzip stream of the records, one JSON document per line.
    #[test]
    fn a_delivered_object_is_gzipped_newline_delimited_json() {
        for count in [0_usize, 1, 3] {
            let records: Vec<String> = (0..count)
                .map(|index| ::serde_json::json!({ "action": "COUNT", "n": index }).to_string())
                .collect();
            let body = gzipped(&records)
                .unwrap_or_else(|| panic!("case {count} records: the object must have a body"));
            let mut decoded = String::new();
            ::flate2::read::GzDecoder::new(body.as_slice())
                .read_to_string(&mut decoded)
                .unwrap_or_else(|error| panic!("case {count} records: not a gzip stream: {error}"));
            let lines: Vec<&str> = decoded.lines().collect();
            assert_eq!(
                lines.len(),
                count,
                "case {count} records: expected one line each, got {decoded:?}"
            );
            for (index, line) in lines.iter().enumerate() {
                let document: ::serde_json::Value = ::serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("case {count} records: line {index} is not \
                                                    JSON: {error}"));
                assert_eq!(
                    document["n"],
                    ::serde_json::json!(index),
                    "case {count} records: expected line {index} to be that record, got {line:?}"
                );
            }
        }
    }

    /// RULE: a unique field is drawn fresh, so two files of one minute do not collide.
    #[test]
    fn unique_fields_are_drawn_fresh() {
        let first = unique_field();
        let second = unique_field();
        assert_eq!(
            (first.len(), second.len()),
            (UNIQUE_FIELD_CHARS, UNIQUE_FIELD_CHARS),
            "case two fields: expected {UNIQUE_FIELD_CHARS} characters each, got {first:?} and \
             {second:?}"
        );
        assert_ne!(
            first, second,
            "case two fields: expected distinct fields, got {first:?} twice"
        );
    }
}
