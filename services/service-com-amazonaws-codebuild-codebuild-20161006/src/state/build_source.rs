// SPDX-License-Identifier: CC0-1.0

//! A build's INPUT: the source archive the build downloads before its first command runs.
//!
//! # The rule this module implements
//!
//! A build whose source is an object of the account's object store downloads that object
//! and unpacks it into its working tree, so the buildspec's commands run with the
//! repository's files present: `ProjectSource$location` for the `S3` source type is "the
//! path to the ZIP file that contains the source code", and
//! `StartBuildInput$sourceVersion` is, "for Amazon S3: the version ID of the object that
//! represents the build input ZIP file to use", so a build that names one downloads THAT
//! version however the object has changed since — which is what makes a retry of a pinned
//! build run on the same bytes as the build it retries.
//!
//! A source the store does not serve is a build with nothing to run: the phase that
//! fetches the source fails and reports what the store answered
//! ([`super::BuildSeed::source_failure`]), and no command of the buildspec runs. Nothing
//! reports a download that did not happen.
//!
//! The object is read through the ONE shared read of an S3 object
//! ([`::service_common::s3_object_read`]), over the emulator's in-process transport, and
//! unpacked by the ONE archive reading this crate has ([`super::source_archive`]).

use ::smithy_server_runtime::EmbeddedEmulator;

use super::source_archive;
use crate::ops::types::{PhaseContext, ProjectSource};

/// The `SourceType` whose source is an object of the account's object store: "S3: The
/// source code is in an Amazon S3 bucket" (`ProjectSource$type`).
pub const SOURCE_TYPE_S3: &str = "S3";

/// The `PhaseContext$statusCode` CodeBuild reports when the fault is the build's own
/// declaration or the account's own state rather than a command's — which is how it
/// reports a source it was not served.
pub const CLIENT_ERROR: &str = "CLIENT_ERROR";

/// How many bytes of a source archive a build reads. The archive is caller-supplied, so
/// the ceiling is this emulator's: the working tree the entries are unpacked into is
/// bounded in its own right ([`super::build_shell`]), and this bounds what is buffered to
/// read them out of.
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// Where one build's S3 source object is: the bucket and the key
/// `ProjectSource$location` names, as `<bucket>/<key>` — `None` for a source that is not
/// an object of the store, or one that names no object.
pub fn source_object(source: &ProjectSource) -> Option<(String, String)> {
    if source.r#type != SOURCE_TYPE_S3 {
        return None;
    }
    let location = source.location.as_deref()?.trim();
    let (bucket, key) = location.split_once('/')?;
    let (bucket, key) = (bucket.trim(), key.trim().trim_start_matches('/'));
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// What a build's S3 source contributes to its working tree: the files the archive holds,
/// or why the build could not be prepared from it.
#[derive(Debug, Clone)]
pub enum SourceDownload {
    /// There is no object to download: the build's source is not one.
    NotAnObject,
    /// The store served the object and it unpacked into these files.
    Unpacked(Vec<(String, String)>),
    /// The build has nothing to run, and this is what the phase that fetches its source
    /// reports.
    Refused(PhaseContext),
}

/// Download and unpack `source` at `version`, through this emulator's own object store.
pub async fn download(
    callback: &EmbeddedEmulator,
    region: &str,
    source: &ProjectSource,
    version: Option<&str>,
) -> SourceDownload {
    let Some((bucket, key)) = source_object(source) else {
        return SourceDownload::NotAnObject;
    };
    let read = ::service_common::s3_object_read::read_object(
        callback,
        region,
        &bucket,
        &key,
        version,
        MAX_SOURCE_BYTES,
    )
    .await;
    let address = object_address(&bucket, &key);
    match read {
        ::service_common::s3_object_read::ObjectRead::Served(bytes) => {
            match source_archive::unpacked(&bytes) {
                Some(files) => SourceDownload::Unpacked(files),
                // The object is there and it is not an archive, so there is no source
                // tree to run on.
                None => SourceDownload::Refused(client_error(&format!(
                    "{NOT_AN_ARCHIVE_MESSAGE} {FOR_PRIMARY_SOURCE} {address}"
                ))),
            }
        }
        ::service_common::s3_object_read::ObjectRead::Absent => SourceDownload::Refused(
            client_error(&format!("{NO_SUCH_KEY_MESSAGE} {FOR_PRIMARY_SOURCE} {address}")),
        ),
        ::service_common::s3_object_read::ObjectRead::Denied => SourceDownload::Refused(
            client_error(&format!("{ACCESS_DENIED_MESSAGE} {FOR_PRIMARY_SOURCE} {address}")),
        ),
        ::service_common::s3_object_read::ObjectRead::Unreadable
        | ::service_common::s3_object_read::ObjectRead::Unanswered => {
            SourceDownload::Refused(client_error(&format!(
                "{UNREAD_MESSAGE} {FOR_PRIMARY_SOURCE} {address}"
            )))
        }
    }
}

/// What CodeBuild states about the source a download was for: the source it was the
/// primary one of, and the object that source named.
const FOR_PRIMARY_SOURCE: &str = "for primary source and source version";

/// The object store's own sentence for an object it holds nothing at.
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// The object store's own sentence for a read it does not permit.
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// What is stated about an object the store served that is not the ZIP file a build's
/// source is documented to be.
const NOT_AN_ARCHIVE_MESSAGE: &str = "The source archive could not be unzipped";

/// What is stated about an object the store neither served nor answered about, so nothing
/// is known about it.
const UNREAD_MESSAGE: &str = "The source could not be downloaded";

/// The address a build's source object is reported by: the S3 ARN of the object, which is
/// the form CodeBuild reports a source location in.
fn object_address(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// A phase context carrying CodeBuild's client-error classification and `message`.
pub fn client_error(message: &str) -> PhaseContext {
    PhaseContext {
        status_code: Some(CLIENT_ERROR.to_string()),
        message: Some(message.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a build's source object is the bucket and key its `location` names, and only
    /// for the source type whose source IS an object of the store.
    ///
    /// The rows differ along what the rule generalises over: the source type, how many
    /// path segments the key has, and the forms that name no object.
    #[test]
    fn a_source_object_is_what_the_location_names() {
        /// One row: the case, the source type, its location, and the object it names.
        type Row = (
            &'static str,
            &'static str,
            Option<&'static str>,
            Option<(&'static str, &'static str)>,
        );
        let rows: Vec<Row> = vec![
            (
                "a bucket and a key",
                SOURCE_TYPE_S3,
                Some("source-bucket/input.zip"),
                Some(("source-bucket", "input.zip")),
            ),
            (
                "a key under several segments",
                SOURCE_TYPE_S3,
                Some("source-bucket/team/project/input.zip"),
                Some(("source-bucket", "team/project/input.zip")),
            ),
            (
                "a location with no key at all",
                SOURCE_TYPE_S3,
                Some("source-bucket"),
                None,
            ),
            ("no location", SOURCE_TYPE_S3, None, None),
            (
                "a source that is not an object",
                "NO_SOURCE",
                Some("source-bucket/input.zip"),
                None,
            ),
            (
                "a source of another kind that names a path",
                "GITHUB",
                Some("github.com/owner/repository"),
                None,
            ),
        ];
        for (case, kind, location, expected) in rows {
            let source = ProjectSource {
                r#type: kind.to_string(),
                location: location.map(str::to_string),
                ..ProjectSource::default()
            };
            let object = source_object(&source);
            let read = object
                .as_ref()
                .map(|(bucket, key)| (bucket.as_str(), key.as_str()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
