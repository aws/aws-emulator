// SPDX-License-Identifier: CC0-1.0

//! Where a MicroVM image version's code comes from: the object Amazon S3 holds at the
//! address `codeArtifact$uri` named, which is what says whether the version's build can
//! produce an image at all.
//!
//! `CodeArtifact$uri` is "The URI of the code artifact in Amazon S3", and
//! `CreateMicrovmImage` documents the build it starts as asynchronous — "the image
//! transitions from CREATING to CREATED on success, or CREATE_FAILED on failure". A
//! build that cannot FETCH its code artifact produces nothing, so resolving that address
//! is what decides the branch; this emulator holds an Amazon S3 of its own, reached the
//! way any emulated service reaches another, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS: no
//! socket, no outbound request, no host resolution. An object a caller staged with
//! `PutObject` is therefore the artifact a build fetches.
//!
//! ## What grounds a settled failure
//!
//! The two ways a resolution produces no artifact are kept apart, because they are
//! different claims:
//!
//! * Amazon S3 ANSWERED that it serves no such bucket or object, or that the read is not
//!   permitted — a fact about the `uri` the version carries, so the build settles on the
//!   FAILURE branch with the reason for that answer
//!   ([`CodeArtifactVerdict::Unfetchable`]);
//! * nothing answered the read at all — this emulator was assembled with no object store
//!   to ask — so NOTHING about the artifact is established
//!   ([`CodeArtifactVerdict::Unestablished`]), and a failure that was never established
//!   is not reported as one.
//!
//! The answer is taken when the build is SCHEDULED and settled on the record when the
//! clock makes the build due
//! ([`super::ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`]), so a
//! caller reading straight after the create still observes the build in flight and no
//! handler sleeps. The object's BYTES are what a build produces its image from, so a
//! served artifact is read into this process under the bound
//! [`super::guest_program::MAX_ARTIFACT_BYTES`] and the guest program it holds is kept on
//! the version ([`super::guest_program`]): RUNNING that guest is what a MicroVM's
//! published endpoint serves (`super::guest_runtime`).

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The `stateReason` a build settles with when Amazon S3 answers that it holds no such
/// BUCKET: the whole fixed sentence AWS words this failure class as, which has no
/// variable parts.
pub const BUCKET_ABSENT_REASON: &str = "The S3 bucket does not exist.";

/// The `stateReason` a build settles with when the bucket is there and the OBJECT is
/// not — the same sentence as [`BUCKET_ABSENT_REASON`] one level down, for the other
/// half of an object address.
pub const OBJECT_ABSENT_REASON: &str = "The S3 object does not exist.";

/// The `stateReason` a build settles with when Amazon S3 answers that the read of the
/// artifact is not permitted: the artifact may well be there, and what the build
/// establishes is that it may not fetch it.
pub const ACCESS_DENIED_REASON: &str = "Access to the S3 object was denied.";

/// What a resolution of a version's code artifact established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeArtifactVerdict {
    /// The object store served the artifact, so the build has code to build: the bytes it
    /// served, which are what the guest's program is read out of
    /// ([`super::guest_program`]).
    Fetched(Vec<u8>),
    /// The store ANSWERED that the artifact cannot be fetched; the clause is the reason
    /// the version and every build of it settle with.
    Unfetchable(&'static str),
    /// Nothing established whether the artifact is there: no object store answered.
    Unestablished,
}

impl CodeArtifactVerdict {
    /// The `stateReason` the build this verdict was taken for settles with, and `None`
    /// for a build that settles successfully.
    ///
    /// An [`Self::Unestablished`] resolution reports no failure: a build is refuted only
    /// by an answer from the store that owns the artifact, never by this emulator failing
    /// to ask it.
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            Self::Unfetchable(reason) => Some((*reason).to_string()),
            Self::Fetched(_) | Self::Unestablished => None,
        }
    }

    /// The GUEST the served artifact holds, as the program a MicroVM run from this
    /// version executes — and `None` for a verdict that served no bytes, or bytes holding
    /// no program this emulator's engine can run.
    ///
    /// `uri` is the `codeArtifact$uri` the artifact was fetched from: the OBJECT KEY it
    /// addresses is what names the single module of an artifact that is not an archive,
    /// so that is what the read is given.
    pub fn guest(&self, uri: &str) -> Option<::std::sync::Arc<super::guest_program::GuestProgram>> {
        match self {
            Self::Fetched(bytes) => {
                let key = super::s3_object_uri(uri).map_or(uri, |(_, key)| key);
                super::guest_program::read(bytes, key).map(::std::sync::Arc::new)
            }
            Self::Unfetchable(_) | Self::Unestablished => None,
        }
    }
}

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the
/// artifact a version named, each mapped to the reason the build settles with. Every
/// other answer is this emulator failing to ask its own object store, which establishes
/// nothing about the artifact.
const UNFETCHABLE_ARTIFACT_CODES: &[(&str, &str)] = &[
    ("NoSuchBucket", BUCKET_ABSENT_REASON),
    ("NoSuchKey", OBJECT_ABSENT_REASON),
    ("NoSuchVersion", OBJECT_ABSENT_REASON),
    ("NotFound", OBJECT_ABSENT_REASON),
    ("AccessDenied", ACCESS_DENIED_REASON),
    ("AllAccessDisabled", ACCESS_DENIED_REASON),
];

/// What the build of a version whose `codeArtifact$uri` is `uri` can establish about its
/// artifact, read through the emulator's own Amazon S3.
///
/// A `uri` that is not an S3 object URI addresses no object at all, so nothing is asked
/// of the store: such a request is refused on its own shape
/// ([`super::CODE_ARTIFACT_URI_REQUIREMENT`]) before a build is ever scheduled.
pub async fn resolve(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    uri: &str,
) -> CodeArtifactVerdict {
    let Some((bucket, key)) = super::s3_object_uri(uri) else {
        return CodeArtifactVerdict::Unestablished;
    };
    let region = super::request_region(ctx);
    artifact_verdict(&s3_client(ctx, &region), bucket, key).await
}

/// Ask `client` whether it serves the object `key` of `bucket`, and read what it serves.
///
/// The read is `GetObject`, the operation a build fetches its artifact with, because its
/// refusals are what tell the two halves of an object address apart: a HEAD of an absent
/// object carries no body at all, so a 404 to one says nothing about WHICH of the bucket
/// and the key is missing. The bytes it answers ARE the guest's program, so they are read
/// into this process — bounded by [`super::guest_program::MAX_ARTIFACT_BYTES`], which is
/// what the read holds at most whatever the object's own length says.
async fn artifact_verdict(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> CodeArtifactVerdict {
    let read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        // RANGED to the bound, so a larger artifact is read up to it rather than
        // buffered whole: what this process holds is bounded by the range it asked for
        // and not by the length the object declares.
        .range(format!(
            "bytes=0-{}",
            super::guest_program::MAX_ARTIFACT_BYTES
        ));
    let error = match read.send().await {
        Ok(served) => {
            let mut held = match served.body.collect().await {
                Ok(body) => body.into_bytes().to_vec(),
                Err(error) => {
                    // The store served the object and its bytes did not arrive, which
                    // establishes nothing about the artifact.
                    ::tracing::warn!(
                        bucket = ?bucket,
                        key = ?key,
                        error = %error,
                        "a MicroVM image code artifact did not arrive in full"
                    );
                    return CodeArtifactVerdict::Unestablished;
                }
            };
            // The range asked for one byte past the bound, so a longer answer is an
            // artifact larger than this emulator reads: it is held to the bound, and the
            // operator is told, because the guest program is then read from a truncated
            // artifact and the MicroVM will have nothing to run.
            let bound = usize::try_from(super::guest_program::MAX_ARTIFACT_BYTES)
                .unwrap_or(usize::MAX);
            if held.len() > bound {
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    bound,
                    "a MicroVM image code artifact is larger than this emulator reads, so \
                     its guest program is read from the first {bound} bytes of it"
                );
                held.truncate(bound);
            }
            return CodeArtifactVerdict::Fetched(held);
        }
        Err(error) => error,
    };
    let code = error.code().unwrap_or_default();
    if let Some((_, reason)) = UNFETCHABLE_ARTIFACT_CODES
        .iter()
        .find(|(answered, _)| *answered == code)
    {
        // The store itself says it serves no such artifact, or that this read is not
        // permitted: the settled failure is grounded in its answer.
        return CodeArtifactVerdict::Unfetchable(reason);
    }
    // Any other answer is this emulator failing to ask its own object store rather than
    // the store reporting an unfetchable artifact — and it is worth a warning, because a
    // deployment with no Amazon S3 mounted to ask is one where no build's artifact is
    // ever established. The caller's own values are rendered in their escaped `Debug`
    // form, so a bucket or key carrying a newline cannot forge a log line.
    ::tracing::warn!(
        bucket = ?bucket,
        key = ?key,
        code = ?error.code(),
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "a MicroVM image code artifact could not be resolved against the emulator's Amazon S3"
    );
    CodeArtifactVerdict::Unestablished
}

/// The Amazon S3 client a code artifact is resolved with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below
/// is an ADDRESS the in-process router reads rather than a host any resolver looks up:
/// this client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or
/// key a caller names. The one caller-supplied value that reaches its authority is the
/// signing region, which [`super::request_region`] takes from the request's own
/// credential scope and otherwise answers the default region with.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which
/// would need a sleep implementation this client is not given. The same goes for caching
/// the identity: the credentials the callback hands over are used directly, and they
/// only sign this operation's in-process calls.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: only an answer from the object store settles a build on the failure branch,
    /// and the reason it settles with is the reason for that answer.
    #[test]
    fn only_an_answered_refusal_is_a_failure_reason() {
        let rows: &[(&str, CodeArtifactVerdict, Option<&str>)] = &[
            (
                "the artifact is served",
                CodeArtifactVerdict::Fetched(b"// the guest".to_vec()),
                None,
            ),
            (
                "no such bucket",
                CodeArtifactVerdict::Unfetchable(BUCKET_ABSENT_REASON),
                Some(BUCKET_ABSENT_REASON),
            ),
            (
                "no such object",
                CodeArtifactVerdict::Unfetchable(OBJECT_ABSENT_REASON),
                Some(OBJECT_ABSENT_REASON),
            ),
            (
                "the read is not permitted",
                CodeArtifactVerdict::Unfetchable(ACCESS_DENIED_REASON),
                Some(ACCESS_DENIED_REASON),
            ),
            (
                "nothing answered the read",
                CodeArtifactVerdict::Unestablished,
                None,
            ),
        ];
        for (case, verdict, expected) in rows {
            let actual = verdict.failure_reason();
            let expected = expected.map(::std::string::ToString::to_string);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
