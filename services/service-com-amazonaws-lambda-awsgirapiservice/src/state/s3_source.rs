// SPDX-License-Identifier: CC0-1.0

//! Reading the DEPLOYMENT PACKAGE a .zip package's Amazon S3 location names, out of Amazon
//! S3 itself.
//!
//! "For a .zip file archive, the code property specifies the location of the .zip file"
//! (`CreateFunction`), and a layer's content is "an Amazon S3 location" the same way
//! (`LayerVersionContentInput`): such a package is a REFERENCE into Amazon S3, and Lambda
//! FOLLOWS it when the package is stored, exactly as it follows a container image's
//! reference into the registry ([`super::image_source`]). Two things come out of following
//! it, and both are settled by the call that WRITES the package rather than by an
//! invocation:
//!
//! 1. whether Amazon S3 hands the object over at all — an object it answers that it will
//!    not serve is refused, and nothing is stored, so a customer learns that the package
//!    was never uploaded from the refusal instead of from a function that can never start;
//! 2. WHICH bytes the package is — the object's own, which is what `CodeSize` and
//!    `CodeSha256` are measured from ([`super::package_code_identity`]) and what an
//!    invocation of the function runs.
//!
//! The object is read by ASKING AMAZON S3 for it, over the in-process callback into this
//! same emulator ([`smithy_server_runtime::OperationContext::callback`]): the read is a
//! `GetObject` signed with this emulator's own credentials and dispatched in process, so the
//! bytes a function is frozen to are the ones `PutObject` wrote and nothing here opens a
//! socket or resolves a host. It is made with no store guard held, because it is a call out
//! of the handler.
//!
//! Nothing is refused on evidence this emulator does not have, and there are two such cases:
//!
//! * an answer that identified no Amazon S3 refusal at all — no Amazon S3 mounted to ask, a
//!   hop that did not complete, a body that could not be read — shows nothing about the
//!   object;
//! * an answer that this store holds NO SUCH BUCKET ([`UNHELD_BUCKET_CODE`]). A bucket is
//!   named by a bare name in a namespace that spans every account ("General purpose buckets
//!   exist in a global namespace", `CreateBucket`), so the name a package carries may well
//!   name a bucket of another account that granted this one access — and an artifact bucket
//!   is routinely provisioned outside the client that creates the function. A bucket this
//!   emulator's Amazon S3 holds no record of is therefore evidence about THIS STORE and not
//!   about the object, the same reading [`super::execution_role_is_unassumable`] takes of an
//!   execution role this account's IAM holds nothing under.
//!
//! What Amazon S3 answers about a bucket it DOES hold is the authority on that bucket's
//! objects, so a key it has nothing under, and a read it will not serve, are refusals this
//! service reports as its own. So is an object at or past the ceiling a deployment package
//! may unzip to at all: an archive is never larger than its own contents, so an object as
//! long as that ceiling is over it whatever it holds — the refusal states that a package has
//! to be SMALLER than the ceiling — and it is refused on the length Amazon S3 reported rather
//! than materialised to be measured.

/// The error code Amazon S3 answers for a bucket it holds no record of ("The specified
/// bucket does not exist"), which is the one refusal that is evidence about this store
/// rather than about the object (see the module docs).
pub const UNHELD_BUCKET_CODE: &str = "NoSuchBucket";

/// What following a .zip package's Amazon S3 location established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Package {
    /// Amazon S3 handed the object over: the deployment package's own bytes.
    Read(Vec<u8>),
    /// Amazon S3 ANSWERED, about a bucket it holds, that it will not hand the object over:
    /// the error code it classified that refusal under and the message it worded it with.
    Refused {
        /// Amazon S3's own error code (`NoSuchKey`, `AccessDenied`, …).
        code: String,
        /// The sentence Amazon S3 answered with, or its code where it worded none.
        message: String,
    },
    /// The object is larger than a deployment package may be at all, so it is no package
    /// Lambda would store whatever it holds (see [`read_s3_package`]).
    TooLarge,
    /// Nothing of the kind can be shown: no Amazon S3 to ask, an answer this emulator could
    /// not read, or a bucket this store holds no record of.
    Unknown,
}

/// The sentence Lambda refuses a package it will not store with.
///
/// For an object Amazon S3 would not hand over, fixed text around the two parts AMAZON S3
/// itself worded — the code it classified the refusal under and the message it answered —
/// because that refusal is what Lambda reports: the read it made of the deployment package
/// was not served, and S3 said why. For an object past the package ceiling, the ceiling
/// itself ([`super::CODE_SIZE_UNZIPPED_LIMIT`]), which is the limit the package was measured
/// against.
#[must_use]
pub fn s3_package_message(package: &S3Package) -> Option<String> {
    match package {
        S3Package::Refused { code, message } => Some(format!(
            "Error occurred while GetObject. S3 Error Code: {code}. S3 Error Message: {message}"
        )),
        S3Package::TooLarge => Some(super::unzipped_limit_message()),
        S3Package::Read(_) | S3Package::Unknown => None,
    }
}

/// Follow the Amazon S3 location `bucket`/`key` (optionally one `version` of it) names, for
/// a package being written in `region`.
///
/// A READ of Amazon S3: it decides and reads, and changes nothing there.
pub async fn read_s3_package(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    version: Option<&str>,
) -> S3Package {
    let read = client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version.map(::std::string::ToString::to_string))
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            // What AMAZON S3 answered, when it answered at all: a hop that produced no
            // coded refusal of its own establishes nothing about the object.
            let Some(refusal) = error.as_service_error().and_then(|service| {
                let code = ::aws_sdk_s3::error::ProvideErrorMetadata::code(service)?;
                let message = ::aws_sdk_s3::error::ProvideErrorMetadata::message(service);
                Some(S3Package::Refused {
                    code: code.to_string(),
                    message: message.unwrap_or(code).to_string(),
                })
            }) else {
                return S3Package::Unknown;
            };
            return match &refusal {
                S3Package::Refused { code, .. } if code == UNHELD_BUCKET_CODE => S3Package::Unknown,
                _ => refusal,
            };
        }
    };
    // What the answer SAYS it carries, judged before any of it is held: an archive is never
    // larger than its own contents, so an object Amazon S3 reports as long as a deployment
    // package may unzip to at all, or longer (`AccountLimit$CodeSizeUnzipped`, "250 MB
    // (unzipped)" — [`super::CODE_SIZE_UNZIPPED_LIMIT`]), is at or past that ceiling whatever
    // it holds, and is refused on the length S3 reported rather than materialised to measure.
    // The bound is the one the refusal states: a package has to be SMALLER than the ceiling,
    // so the ceiling itself is over it.
    if is_over_the_package_ceiling(read.content_length()) {
        return S3Package::TooLarge;
    }
    // The object's bytes, read whole out of the in-process answer. A body that stopped
    // part-way through is an answer this emulator could not read, which shows nothing about
    // the object.
    match read.body.collect().await {
        Ok(body) => S3Package::Read(body.to_vec()),
        Err(_) => S3Package::Unknown,
    }
}

/// Whether an object Amazon S3 reports `length` bytes long is at or past the ceiling a
/// deployment package may unzip to ([`super::CODE_SIZE_UNZIPPED_LIMIT`]).
///
/// The bound is the one the refusal states: a package has to be SMALLER than the ceiling, so
/// the ceiling itself is over it. An answer that reported no length at all says nothing about
/// how long the object is.
fn is_over_the_package_ceiling(length: Option<i64>) -> bool {
    length.is_some_and(|length| length >= super::CODE_SIZE_UNZIPPED_LIMIT)
}

/// The bytes the Amazon S3 location `bucket`/`key` names, for the three writes that store a
/// package named that way.
///
/// `Err` is the message the write is refused with ([`s3_package_message`], and the expanded
/// ceiling for an object whose CONTENTS are past it), and `Ok(None)` is an object nothing
/// could be shown about, which refuses nothing and holds no bytes.
pub async fn s3_package_bytes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    version: Option<&str>,
) -> Result<Option<Vec<u8>>, String> {
    let package = read_s3_package(ctx, region, bucket, key, version).await;
    if let Some(message) = s3_package_message(&package) {
        return Err(message);
    }
    match package {
        // The length Amazon S3 reported is only half of the ceiling: an archive whose
        // stored bytes are well under it can still declare CONTENTS past it, and the
        // package is held to what it expands to, so the object this read handed over is
        // measured the same way an inline upload is (`super::archive`).
        S3Package::Read(bytes) if super::is_over_the_unzipped_limit(Some(&bytes)) => {
            Err(super::unzipped_limit_message())
        }
        S3Package::Read(bytes) => Ok(Some(bytes)),
        S3Package::Refused { .. } | S3Package::TooLarge | S3Package::Unknown => Ok(None),
    }
}

/// An Amazon S3 client over the in-process callback into this same emulator, signing as the
/// emulator's own credentials — the read Lambda itself makes of a deployment package, not
/// one a running handler makes (`super::execution::aws_s3` is that one).
///
/// Path-style addressing is what the emulated Amazon S3 service routes on (its operations are
/// bound under a `{Bucket}` path label). The transport is in process, so everything a network
/// client needs and this one cannot have — retries, timeouts, identity caching and
/// stalled-stream protection, each of which needs a sleep implementation the callback
/// transport is not given — is switched off.
fn client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::{is_over_the_package_ceiling, s3_package_message, S3Package};

    /// The ceiling a deployment package may unzip to, as AWS publishes it ("250 MB
    /// (unzipped)"): a package has to be smaller than it, so the ceiling itself is over it
    /// and one byte under it is not.
    #[test]
    fn the_package_ceiling_admits_everything_under_it_and_nothing_at_it() {
        let ceiling = crate::state::CODE_SIZE_UNZIPPED_LIMIT;
        // (case, the length Amazon S3 reported, whether it is over the ceiling)
        let rows: [(&str, Option<i64>, bool); 6] = [
            ("an empty object", Some(0), false),
            ("an ordinary package", Some(4_096), false),
            ("one byte under the ceiling", Some(ceiling - 1), false),
            ("the ceiling itself", Some(ceiling), true),
            ("one byte over the ceiling", Some(ceiling + 1), true),
            ("an answer that reported no length", None, false),
        ];
        for (case, length, expected) in rows {
            let over = is_over_the_package_ceiling(length);
            assert_eq!(
                over, expected,
                "case {case:?}: expected over-the-ceiling {expected}, got {over}"
            );
        }
    }

    /// The refusal is fixed text around the code and the message AMAZON S3 answered, and a
    /// resolution that refuses nothing words nothing.
    #[test]
    fn the_refusal_reports_the_code_and_message_amazon_s3_answered() {
        // (case, what following the reference established, the sentence it is refused with)
        let rows: [(&str, S3Package, Option<&str>); 6] = [
            (
                "a key the bucket holds nothing under",
                S3Package::Refused {
                    code: "NoSuchKey".to_string(),
                    message: "The specified key does not exist.".to_string(),
                },
                Some(
                    "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error \
                     Message: The specified key does not exist.",
                ),
            ),
            (
                "a read the bucket will not serve",
                S3Package::Refused {
                    code: "AccessDenied".to_string(),
                    message: "Access Denied".to_string(),
                },
                Some(
                    "Error occurred while GetObject. S3 Error Code: AccessDenied. S3 Error \
                     Message: Access Denied",
                ),
            ),
            (
                "a refusal Amazon S3 worded nothing for, reported as what it did identify",
                S3Package::Refused {
                    code: "NoSuchVersion".to_string(),
                    message: "NoSuchVersion".to_string(),
                },
                Some(
                    "Error occurred while GetObject. S3 Error Code: NoSuchVersion. S3 Error \
                     Message: NoSuchVersion",
                ),
            ),
            (
                "an object past the ceiling a deployment package may unzip to",
                S3Package::TooLarge,
                Some("Unzipped size must be smaller than 262144000 bytes"),
            ),
            (
                "an object Amazon S3 handed over",
                S3Package::Read(b"PK\x03\x04".to_vec()),
                None,
            ),
            ("nothing that can be shown", S3Package::Unknown, None),
        ];
        for (case, package, expected) in rows {
            let worded = s3_package_message(&package);
            assert_eq!(
                worded.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {worded:?}"
            );
        }
    }
}
