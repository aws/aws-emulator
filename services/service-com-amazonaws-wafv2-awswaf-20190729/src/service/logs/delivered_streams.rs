// SPDX-License-Identifier: CC0-1.0

//! The RECORD a web ACL's log record is delivered as when its logging destination names an
//! Amazon Data Firehose delivery stream.
//!
//! The third destination kind `PutLoggingConfiguration` publishes, beside a CloudWatch Logs
//! log group and an Amazon S3 bucket ([`super::delivered_objects`]): "you can use an Amazon
//! CloudWatch Logs log group, an Amazon Simple Storage Service (Amazon S3) bucket, or an
//! Amazon Kinesis Data Firehose". A stream is READ by what it was created to deliver into,
//! so the record is handed to the stream itself — one data record per log record, the
//! document followed by a newline, so what the stream writes on is newline-delimited JSON —
//! and the stream's own delivery into its configured destination is the stream's concern.
//!
//! Amazon Data Firehose is reached the way any emulated service reaches another: the AWS SDK
//! over the emulator's own in-process transport, through the operation callback, which opens
//! no socket and reaches nothing outside this process. Delivery is BEST EFFORT, as every
//! vended-logs delivery is: a stream the account has since deleted loses the record and is
//! traced rather than failing a request that has already been answered.

/// The service namespace an Amazon Data Firehose ARN carries, which is also the host its
/// in-process endpoint is addressed by.
pub(super) const FIREHOSE_VENDOR: &str = "firehose";

/// The resource type a delivery stream's ARN names
/// (`arn:<partition>:firehose:<region>:<account>:deliverystream/<name>`).
pub(super) const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// Hand `records` to the delivery stream `stream`, one data record each.
///
/// `region` is the Region the stream's own ARN names, which is where the stream lives.
/// `true` once the stream has taken them, `false` when it could not. Call it with NO state
/// guard held — it awaits.
pub(super) async fn delivered(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    stream: &str,
    records: &[String],
) -> bool {
    let batch: Vec<::aws_sdk_firehose::types::Record> = records
        .iter()
        .filter_map(|record| {
            ::aws_sdk_firehose::types::Record::builder()
                // A record a consumer reads line by line, as the destination the stream
                // writes into holds it.
                .data(::aws_smithy_types::Blob::new(format!("{record}\n")))
                .build()
                .ok()
        })
        .collect();
    if batch.is_empty() {
        return true;
    }
    let client = firehose_client(ctx, region);
    if let Err(error) = client
        .put_record_batch()
        .delivery_stream_name(stream)
        .set_records(Some(batch))
        .send()
        .await
    {
        ::tracing::warn!(
            delivery_stream = %stream,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "these log records could not be delivered to the destination delivery stream"
        );
        return false;
    }
    true
}

/// Amazon Data Firehose's client for `region`, over the in-process callback and under the
/// same client policy every other in-process hop in this tree is built with: nothing to
/// retry, time out or watch for a stalled stream, because the transport is in process, and
/// the credentials the callback hands over are the emulator's own.
fn firehose_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{FIREHOSE_VENDOR}.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}
