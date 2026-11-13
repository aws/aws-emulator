// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process Amazon S3 client an emulated service WRITES an
//! object through, so no two services configure that hop differently — the sibling of
//! [`crate::eventbridge_client`] and [`crate::kinesis_client`] for the account's object
//! store.
//!
//! A service that delivers something into a bucket a client named — the log files of a
//! trail that is logging ([`crate::trail_log_files`]) among them — reaches Amazon S3 the way
//! any emulated service reaches another: the AWS SDK driven over the emulator's own
//! in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]). The transport is what makes the
//! write in-process: no socket is opened, no name is resolved, and nothing outside this
//! process is reachable from it. The endpoint URL below is addressing for that transport,
//! not a host this process connects to, and it is built from the Region alone — no part of a
//! caller's request is a destination.
//!
//! The copies that predate this module — one per service that writes to a bucket — are to be
//! MIGRATED onto it, each in the change that owns its service. Nothing new should build the
//! client by hand.

use smithy_server_runtime::EmbeddedEmulator;

/// The Amazon S3 client a service writes an object through, addressed to `region`.
///
/// Path-style addressing is what the emulated object store routes on (its operations are
/// bound under a `{Bucket}` path label). The transport is in-process, so there is nothing to
/// retry, time out or watch for a stalled stream — each of those would need a sleep
/// implementation this client is not given — and the credentials the callback hands over are
/// used directly rather than cached. Those credentials are the emulator's own: the caller's
/// `Authorization` header is never forwarded.
#[must_use]
pub fn s3_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_s3::Client {
    client_signing_with(callback, region, callback.aws_credentials_provider())
}

/// The same client, signing every call it makes as `credentials` rather than with the
/// emulator's own.
///
/// What a service uses when the read or write it makes is one ANOTHER principal makes
/// through it — a job run fetching its script as the job's execution role, say — so the
/// object store authorizes it against the policies in force on that principal and against
/// the bucket's own policy, exactly as it authorizes any other signed request. The
/// credentials are a session an emulated service minted through the token service; nothing
/// here constructs an access key id or a secret.
#[must_use]
pub fn s3_client_signed_as(
    callback: &EmbeddedEmulator,
    region: &str,
    credentials: ::aws_sdk_s3::config::Credentials,
) -> ::aws_sdk_s3::Client {
    client_signing_with(
        callback,
        region,
        ::aws_sdk_s3::config::SharedCredentialsProvider::new(credentials),
    )
}

/// The ONE builder both constructions above share, so the hop is configured one way whoever
/// the call is signed as.
fn client_signing_with(
    callback: &EmbeddedEmulator,
    region: &str,
    credentials: impl ::aws_sdk_s3::config::ProvideCredentials + 'static,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(credentials)
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
