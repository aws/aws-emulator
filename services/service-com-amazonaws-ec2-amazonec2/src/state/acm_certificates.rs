// SPDX-License-Identifier: CC0-1.0

//! The ACM certificates the ACCOUNT holds, as the EC2 operations that take a certificate ARN
//! resolve one.
//!
//! A certificate is ACM's resource: `acm:RequestCertificate` and `acm:ImportCertificate` are
//! what create one, and `acm:DescribeCertificate` is what reports it. So an EC2 operation that
//! takes a certificate ARN keeps no copy of the certificate and no second list of which ones
//! exist — it ASKS the service that owns them, over the emulator's in-process callback, and a
//! certificate that service does not hold cannot be presented by a resource this one creates.
//!
//! NOTHING here opens a socket or leaves the host: the transport is
//! `ctx.callback().http_client()`, the emulator's in-process channel into its own services,
//! and the endpoint is only the address that channel resolves ACM by. The credentials are the
//! emulator's own, used to sign this read; they are never returned or logged.
//!
//! READ ONLY: nothing here writes to ACM, and nothing here caches its answer — two reads of
//! one ARN ask that service twice, so a certificate imported between them is seen.

use ::smithy_server_runtime::OperationContext;

/// The endpoint prefix the in-process channel resolves ACM by, which is also the service
/// segment of every certificate ARN this crate reads
/// ([`super::acm_certificate_arn_is_well_formed`]).
const ENDPOINT_PREFIX: &str = "acm";

/// Whether the account holds NO ACM certificate under `certificate_arn`.
///
/// True only when the owning service establishes the absence itself — it answers that it holds
/// no certificate under the ARN (`ResourceNotFoundException`), or that the value is not a
/// certificate ARN it could hold one under (`InvalidArnException`) — or when the value is not
/// an ACM certificate ARN at all, which this crate reads one way for every operation that
/// takes one and needs no call to settle. A dependency that did not answer establishes
/// nothing, so its silence reads as "held" rather than refusing a request for want of another
/// service; the same reading `Connector for SCEP` gives a private CA it could not resolve.
///
/// The ARN is the whole key: it carries the partition, the Region and the account the
/// certificate belongs to, so a value naming another account's or another Region's certificate
/// resolves to nothing — the same answer as an ARN naming nothing at all.
pub async fn acm_certificate_is_absent(
    ctx: &OperationContext<'_>,
    region: &str,
    certificate_arn: &str,
) -> bool {
    if !super::acm_certificate_arn_is_well_formed(certificate_arn) {
        return true;
    }
    let answer = client(ctx, region)
        .describe_certificate()
        .certificate_arn(certificate_arn)
        .send()
        .await;
    let error = match answer {
        Ok(_) => return false,
        Err(error) => error,
    };
    match error.as_service_error() {
        Some(service_error) => {
            service_error.is_resource_not_found_exception()
                || service_error.is_invalid_arn_exception()
        }
        None => {
            ::tracing::warn!(
                certificate = %certificate_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the certificate manager did not answer whether the certificate exists"
            );
            false
        }
    }
}

/// The ACM client this service reads that service through: the AWS SDK driven over the
/// emulator's own in-process transport, signing in the emulator's name — EC2 reads the
/// certificate a request named with its own service principal.
fn client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_acm::Client {
    let config = ::aws_sdk_acm::Config::builder()
        .behavior_version(::aws_sdk_acm::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_acm::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{ENDPOINT_PREFIX}.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_acm::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_acm::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_acm::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_acm::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_acm::Client::from_conf(config)
}
