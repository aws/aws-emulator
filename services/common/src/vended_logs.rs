// SPDX-License-Identifier: CC0-1.0

//! The WRITE a vended-log delivery makes into a destination that is not a log group: the
//! OBJECT an Amazon S3 bucket receives, and the PUT RECORD a Firehose delivery stream does.
//!
//! A vended-log delivery destination is "an Amazon Web Services resource that represents an
//! Amazon Web Services service that logs can be sent to", and `PutDeliveryDestination` names
//! the three kinds that receive log records: "a log group in CloudWatch Logs, an Amazon S3
//! bucket, or a delivery stream in Firehose". The log-group write is
//! [`crate::cloudwatch_logs::delivered`]; the other two are here, so a service DELIVERING its
//! vended logs puts a record in any of the three through one path per kind rather than one per
//! service.
//!
//! Each kind is READ differently by its consumer, which is what each write below has to
//! satisfy: a bucket is read by listing it and fetching its objects, so a record delivered
//! there has to BE the log file a reader fetches — gzipped, one record per line, as the sibling
//! vended-log layouts in this tree write theirs ([`crate::trail_log_files`]) — while a delivery
//! stream is read through whatever destination the stream itself delivers to, so a record
//! delivered there is one put record, which that stream then buffers and delivers on.
//!
//! Both services are reached the way any emulated service reaches another: the AWS SDK driven
//! over the emulator's own in-process transport, through the one construction of each client
//! this crate holds ([`crate::s3_client`], [`crate::firehose_client`]). The transport is what
//! makes the write in-process: no socket is opened, no name is resolved, and nothing outside
//! this process is reachable from it. The ADDRESS each client is built with comes from the
//! Region alone — no part of a caller's request is a destination.
//!
//! Delivery is BEST EFFORT, as every vended-logs write is: the caller that produced the record
//! has already been answered, so a destination the account has since deleted, or a write the
//! owning service refuses, loses the record and is reported in tracing rather than failing
//! anything.
//!
//! The copies that predate this module — the delivered objects of
//! `service-com-amazonaws-wafv2-awswaf-20190729`'s logging destinations
//! (`src/service/logs/delivered_objects.rs`) among them — are to be MIGRATED onto it, each in
//! the change that owns its service. Nothing new should frame or write one by hand.

use std::io::Write;

/// The media type of the object a delivered log file is: a gzip stream.
const GZIP_MEDIA_TYPE: &str = "application/x-gzip";

/// The separator between the records of one delivery: each record is delivered as its own
/// line, so a consumer reads one record at a time.
///
/// It is what a delivered object is framed with, and what terminates a record put on a
/// delivery stream: "Firehose buffers records before delivering them to the destination. To
/// disambiguate the data blobs at the destination, a common solution is to use delimiters in
/// the data, such as a newline (\n)" (`PutRecord`).
pub const RECORD_SEPARATOR: &str = "\n";

/// Write `records` into the object `key` of `bucket`, as the gzipped newline-delimited log file
/// a reader of a bucket of vended logs fetches.
///
/// `region` is where the object store is addressed; `true` once the object is in the bucket,
/// `false` when it could not be written. Call it with NO state guard held — it awaits.
pub async fn delivered_object(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    records: &[String],
) -> bool {
    delivered_object_over(ctx.callback(), region, bucket, key, records).await
}

/// [`delivered_object`] for a deliverer that holds the in-process transport ITSELF rather
/// than a request's context — a data plane served on a core-owned socket, which has no
/// `OperationContext` at all — as [`crate::cloudwatch_logs::delivered_over`] is for
/// [`crate::cloudwatch_logs::delivered`].
///
/// The same write, so a record delivered from a socket and one delivered from an operation
/// cannot land differently.
pub async fn delivered_object_over(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    bucket: &str,
    key: &str,
    records: &[String],
) -> bool {
    let Some(body) = gzipped(records) else {
        ::tracing::warn!(
            bucket = %bucket,
            key = %key,
            "these vended-log records could not be framed, so no object was delivered"
        );
        return false;
    };
    let client = crate::s3_client::s3_client(callback, region);
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
            "these vended-log records could not be delivered to the destination bucket"
        );
        return false;
    }
    true
}

/// The object's bytes: the records as newline-delimited text, gzipped — or `None` when the
/// framing itself failed.
///
/// A delivered log file holds ONE record per line, each terminated, and the file IS a gzip
/// stream rather than a transport encoding, so a reader fetches the object and decompresses it.
/// Writing into a `Vec` cannot fail for want of space, so a failure here would be the encoder's
/// own; it answers `None` rather than a truncated stream, because no object at all is more
/// honest than one a reader cannot decompress.
#[must_use]
pub fn gzipped(records: &[String]) -> Option<Vec<u8>> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    for record in records {
        encoder.write_all(record.as_bytes()).ok()?;
        encoder.write_all(RECORD_SEPARATOR.as_bytes()).ok()?;
    }
    encoder.finish().ok()
}

/// Put `records` on the Firehose delivery stream `delivery_stream`, which that stream then
/// delivers to its own destination under its own buffering.
///
/// The records travel as ONE put record, each terminated by [`RECORD_SEPARATOR`], so a consumer
/// of the stream's destination reads them one per line whatever the stream's buffer put in one
/// object. `region` is where Firehose is addressed; `true` once the stream has taken them,
/// `false` when it would not. Call it with NO state guard held — it awaits.
pub async fn delivered_to_stream(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery_stream: &str,
    records: &[String],
) -> bool {
    delivered_to_stream_over(ctx.callback(), region, delivery_stream, records).await
}

/// [`delivered_to_stream`] for a deliverer that holds the in-process transport ITSELF rather
/// than a request's context, as [`delivered_object_over`] is for [`delivered_object`].
pub async fn delivered_to_stream_over(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    delivery_stream: &str,
    records: &[String],
) -> bool {
    let mut data = String::new();
    for record in records {
        data.push_str(record);
        data.push_str(RECORD_SEPARATOR);
    }
    let built = ::aws_sdk_firehose::types::Record::builder()
        .data(::aws_sdk_firehose::primitives::Blob::new(data.into_bytes()))
        .build();
    let record = match built {
        Ok(record) => record,
        Err(error) => {
            ::tracing::warn!(
                delivery_stream = %delivery_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "these vended-log records could not be framed for the destination delivery stream"
            );
            return false;
        }
    };
    let client = crate::firehose_client::firehose_client(callback, region);
    if let Err(error) = client
        .put_record()
        .delivery_stream_name(delivery_stream)
        .record(record)
        .send()
        .await
    {
        ::tracing::warn!(
            delivery_stream = %delivery_stream,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "these vended-log records could not be delivered to the destination delivery stream"
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    /// RULE: the object a delivery writes is a gzip stream of its records, one terminated line
    /// each — whatever the records say and however many there are.
    #[test]
    fn a_delivered_object_is_the_gzipped_records_one_line_each() {
        for (case, records) in [
            ("no record at all", Vec::new()),
            (
                "one record",
                vec![r#"{"x-edge-request-id":"one"}"#.to_string()],
            ),
            (
                "several records",
                vec![
                    r#"{"x-edge-request-id":"one"}"#.to_string(),
                    r#"{"x-edge-request-id":"two"}"#.to_string(),
                    "a plain, delimited record".to_string(),
                ],
            ),
        ] {
            let body = gzipped(&records)
                .unwrap_or_else(|| panic!("case {case}: the object must have a body"));
            assert_eq!(
                body.get(..2),
                Some([0x1f, 0x8b].as_slice()),
                "case {case}: expected a gzip stream, got {:?}",
                body.get(..2)
            );
            let mut decoded = String::new();
            ::flate2::read::GzDecoder::new(body.as_slice())
                .read_to_string(&mut decoded)
                .unwrap_or_else(|error| panic!("case {case}: not a gzip stream: {error}"));
            let lines: Vec<&str> = decoded.lines().collect();
            assert_eq!(
                lines, records,
                "case {case}: expected one line per record, got {decoded:?}"
            );
        }
    }
}
