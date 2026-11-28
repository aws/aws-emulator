// SPDX-License-Identifier: CC0-1.0

//! The ONE seam the files of a stream session are exported through: where the archive
//! lands, what it holds, and the record the export leaves on the stream session.
//!
//! "Export the files that your application modifies or generates in a stream session ...
//! You must provide an Amazon Simple Storage Service (Amazon S3) bucket to store the
//! files in ... Amazon GameLift Streams produces a compressed folder that contains all of
//! the files and directories that were modified or created by the application during the
//! stream session" (`ExportStreamSessionFiles`). So an export is an object WRITE, and it
//! is performed where objects live: Amazon S3 owns buckets and objects, so this module
//! hands the archive to it rather than recording here a claim the object store would not
//! answer for. "To verify the status of the exported files, use GetStreamSession. To
//! delete the files, delete the object in the S3 bucket."
//!
//! Amazon S3 is reached the way the rest of this crate reaches another emulated service
//! ([`super::content`]): the AWS SDK driven over the operation callback's in-process
//! transport ([`smithy_server_runtime::OperationContext::callback`]), through the ONE
//! construction of that client this tree keeps ([`service_common::s3_client`]) — so no
//! socket is opened, no host name is resolved and nothing leaves this process. The only
//! caller-supplied values that reach the client are the bucket NAME, the object KEY and
//! the signing Region, and the Region is narrowed to a Region LABEL
//! ([`service_common::arn::region_label`]) before it can reach the endpoint URL's
//! authority.
//!
//! What the archive HOLDS is the folders the service documents: "Amazon GameLift Streams
//! collects the following generated and modified files. Find them in the corresponding
//! folders in the .zip archive. application/: The folder where your application or game is
//! stored. profile/: The user profile folder. temp/: The system temp folder." The archive
//! carries those three folders and no file: every file in them is written by a RUNNING
//! application — the files it modified in its own folder, the user profile it wrote, the
//! temporary files it left — and no application runs in this emulator, so nothing was
//! collected from any of them. A file invented for them would be output nothing produced.

use std::time::SystemTime;

use crate::ops::types::ExportFilesMetadata;

/// "SUCCEEDED: The exported files are successfully stored in an S3 bucket"
/// (`ListStreamSessions`'s `ExportFilesStatus`), one of the model's own
/// `ExportFilesStatus` values.
pub const EXPORT_SUCCEEDED: &str = "SUCCEEDED";

/// "FAILED: The exported files couldn't be stored in an S3 bucket."
pub const EXPORT_FAILED: &str = "FAILED";

/// "PENDING: Amazon GameLift Streams is processing the request ... uploading the exported
/// files to the S3 bucket is in progress."
pub const EXPORT_PENDING: &str = "PENDING";

/// The folders the service collects files into, in the order it documents them:
/// "application/: The folder where your application or game is stored. profile/: The user
/// profile folder. temp/: The system temp folder."
pub const COLLECTED_FOLDERS: &[&str] = &["application/", "profile/", "temp/"];

/// The extension of a destination that names the archive OBJECT itself, and the uppercase
/// spelling the model's `OutputUri` pattern admits beside it
/// (`^s3://.*(/|\.zip|\.ZIP)$`).
const ARCHIVE_EXTENSIONS: [&str; 2] = [".zip", ".ZIP"];

/// The export of one stream session's files, as the session's own record of it.
///
/// This is what `GetStreamSession` reports as `ExportFilesMetadata`, so the status it
/// carries is the status of the UPLOAD and lives here rather than on the session: a
/// stream session's own status is about the stream, and an export outlives the stream it
/// was taken from.
#[derive(Debug, Clone)]
pub struct FileExport {
    /// An `ExportFilesStatus` value.
    pub status: String,
    /// "A short description of the reason the export is in FAILED status"
    /// (`ExportFilesMetadata$StatusReason`), and absent for every other status.
    pub status_reason: Option<String>,
    /// The S3 URI of the single `.zip` object the files were written to — the
    /// destination the request named, with the generated archive name when the request
    /// named a folder.
    pub output_uri: String,
    /// When the clock owes this export's completion, while it is still `PENDING`.
    pub due_at: Option<SystemTime>,
}

impl FileExport {
    /// The export whose archive the object store has taken, with its completion owed at
    /// `due_at`: the upload is under way, which is what `PENDING` states.
    #[must_use]
    pub fn uploading(output_uri: String, due_at: SystemTime) -> Self {
        Self {
            status: EXPORT_PENDING.to_string(),
            status_reason: None,
            output_uri,
            due_at: Some(due_at),
        }
    }

    /// The export the object store would not take the archive for: there is no exported
    /// file, so the record says so and nothing is owed.
    #[must_use]
    pub fn failed(output_uri: String, status_reason: String) -> Self {
        Self {
            status: EXPORT_FAILED.to_string(),
            status_reason: Some(status_reason),
            output_uri,
            due_at: None,
        }
    }

    /// This export as the `ExportFilesMetadata` every read of the stream session reports.
    #[must_use]
    pub fn metadata(&self) -> ExportFilesMetadata {
        ExportFilesMetadata {
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            output_uri: Some(self.output_uri.clone()),
        }
    }

    /// Advance the transition the clock owed at `now`: an upload that is due has
    /// finished, and its files are stored in the bucket the caller named.
    pub(super) fn advance(&mut self, now: SystemTime) {
        let Some(due) = self.due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.due_at = None;
        self.status = EXPORT_SUCCEEDED.to_string();
    }
}

/// Where an export lands: the bucket and the object key of the one archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub bucket: String,
    pub key: String,
}

impl Destination {
    /// This destination as the S3 URI the export reports back
    /// (`ExportFilesMetadata$OutputUri`).
    #[must_use]
    pub fn uri(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.key)
    }
}

/// The destination the `OutputUri` of a request names, given `archive_name` for the case
/// where the value names a folder rather than an object.
///
/// "If you provide an S3 URI called `s3://amzn-s3-demo-destination-bucket/
/// MyGame_Session1.zip`, then Amazon GameLift Streams will save the files at that
/// location. If you provide an S3 URI called `s3://amzn-s3-demo-destination-bucket/
/// MyGameSessions_ExportedFiles/`, then Amazon GameLift Streams will save the files at
/// `s3://amzn-s3-demo-destination-bucket/MyGameSessions_ExportedFiles/
/// YYYYMMDD-HHMMSS-appId-sg-Id-sessionId.zip`" (`OutputUri`). So a value ending in the
/// archive extension IS the object, and one ending in the separator is a prefix the
/// generated name is appended to.
///
/// The URI itself is read by the tree's ONE reading of an `s3://` location
/// ([`service_common::s3_uri::bucket_and_prefix`], which answers the bucket and the path
/// under it for exactly this question — the PLACE a write goes), so a caller-supplied value
/// is not split by hand here and the bucket it yields is one Amazon S3 holds a bucket
/// under. `None` when the value names no such location, which the model's own `OutputUri`
/// pattern already refuses most of before a handler runs.
#[must_use]
pub fn destination(output_uri: &str, archive_name: &str) -> Option<Destination> {
    let (bucket, path) = ::service_common::s3_uri::bucket_and_prefix(output_uri)?;
    let names_the_object = ARCHIVE_EXTENSIONS
        .iter()
        .any(|extension| path.ends_with(extension));
    let key = match names_the_object {
        true => path,
        false => format!("{path}{archive_name}"),
    };
    (!key.is_empty()).then_some(Destination { bucket, key })
}

/// The name Amazon GameLift Streams generates for an archive when the destination names a
/// folder: "Amazon GameLift Streams generates a ZIP file name based on the stream session
/// metadata", of the documented form
/// `YYYYMMDD-HHMMSS-appId-sg-Id-sessionId.zip` (`OutputUri`).
///
/// Every part comes from the export itself — the instant it was asked for, and the ids of
/// the application, the stream group and the stream session it is an export of — so two
/// exports of two sessions never name one object.
#[must_use]
pub fn archive_name(
    at: SystemTime,
    application_id: &str,
    stream_group_id: &str,
    session_id: &str,
) -> String {
    let stamp = ::chrono::DateTime::<::chrono::Utc>::from(at).format("%Y%m%d-%H%M%S");
    format!("{stamp}-{application_id}-{stream_group_id}-{session_id}.zip")
}

/// The bytes of the archive an export delivers.
///
/// "Amazon GameLift Streams collects the following generated and modified files. Find them
/// in the corresponding folders in the .zip archive", so the archive carries the
/// [`COLLECTED_FOLDERS`] themselves — and no file, which is what was collected: every file
/// in those folders is written by a RUNNING application and no application runs in this
/// emulator, so a file invented for them would be output nothing produced.
///
/// `None` only when the archive could not be written at all, which leaves the caller to
/// report a failure rather than deliver a file that is not one.
#[must_use]
pub fn archive_bytes() -> Option<Vec<u8>> {
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    // A fixed entry timestamp: the archive's bytes then depend on what was collected and
    // on nothing about the machine that wrote it.
    let options =
        ::zip::write::SimpleFileOptions::default().last_modified_time(::zip::DateTime::default());
    for folder in COLLECTED_FOLDERS {
        archive.add_directory(*folder, options).ok()?;
    }
    Some(archive.finish().ok()?.into_inner())
}

/// What the object store answered when the archive was handed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The store took the archive: the object is at the destination, and a later read of
    /// the bucket finds it.
    Stored,
    /// The store would not take it — it holds no such bucket, it refused the write, or
    /// there is no object store beside this service to write into at all — so there is no
    /// exported file.
    Refused,
}

/// Hand `archive` to Amazon S3 as the object `destination` names, signing the in-process
/// hop in `region`.
///
/// Answers [`Delivery::Stored`] only when the store took the object, so an export is
/// reported as uploading exactly when there is an object to find.
pub async fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination: &Destination,
    archive: Vec<u8>,
) -> Delivery {
    // A Region that is not a Region LABEL would re-point the authority of the endpoint
    // URL below, so nothing is written for it.
    let Some(region) = ::service_common::arn::region_label(region) else {
        return Delivery::Refused;
    };
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    match client
        .put_object()
        .bucket(&destination.bucket)
        .key(&destination.key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(archive))
        .send()
        .await
    {
        Ok(_) => Delivery::Stored,
        Err(failure) => {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // bucket name carrying a newline cannot forge a log line.
            ::tracing::debug!(
                bucket = ?destination.bucket,
                key = ?destination.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "Amazon S3 did not take the exported files of a stream session"
            );
            Delivery::Refused
        }
    }
}

/// The `StatusReason` a `FAILED` export carries: the destination the object store would
/// not take the archive into, which is what the caller has to correct.
#[must_use]
pub fn not_uploaded_reason(output_uri: &str) -> String {
    format!("The exported files could not be uploaded to {output_uri}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the destination table: a label, the `OutputUri` the request named, and
    /// the `(bucket, key)` it names, if any.
    type DestinationRow = (&'static str, &'static str, Option<(&'static str, String)>);

    /// RULE: an `OutputUri` ending in the archive extension names the object itself, and
    /// one ending in the separator names a prefix the generated name is appended to.
    #[test]
    fn a_destination_is_the_object_or_the_prefix_the_request_named() {
        let generated = "20260102-030405-a-1-sg-2-ss-3.zip";
        let rows: Vec<DestinationRow> = vec![
            (
                "an object named with the lowercase extension",
                "s3://exports/MyGame_Session1.zip",
                Some(("exports", "MyGame_Session1.zip".to_string())),
            ),
            (
                "an object named with the uppercase extension",
                "s3://exports/MyGame_Session1.ZIP",
                Some(("exports", "MyGame_Session1.ZIP".to_string())),
            ),
            (
                "an object under a multi-level prefix",
                "s3://exports/sessions/alpha/run.zip",
                Some(("exports", "sessions/alpha/run.zip".to_string())),
            ),
            (
                "a folder",
                "s3://exports/MyGameSessions_ExportedFiles/",
                Some((
                    "exports",
                    format!("MyGameSessions_ExportedFiles/{generated}"),
                )),
            ),
            (
                "the root of a bucket",
                "s3://exports/",
                Some(("exports", generated.to_string())),
            ),
            (
                "a bucket named with no key at all",
                "s3://exports",
                Some(("exports", generated.to_string())),
            ),
            ("no bucket", "s3:///exports.zip", None),
            ("no scheme", "exports/run.zip", None),
            // What the shared reading of an `s3://` location enforces beyond the shape:
            // the authority has to be a name Amazon S3 holds a bucket under, and nothing
            // but the one separator may stand between it and the key.
            ("a bucket name too short to exist", "s3://ex/run.zip", None),
            ("a bucket name with two adjacent dots", "s3://ex..ports/run.zip", None),
            ("an authority carrying a port", "s3://exports:9000/run.zip", None),
            (
                "an authority carrying user information",
                "s3://user@exports/run.zip",
                None,
            ),
        ];
        for (case, output_uri, expected) in rows {
            let actual = destination(output_uri, generated);
            let expected = expected.map(|(bucket, key)| Destination {
                bucket: bucket.to_string(),
                key,
            });
            assert_eq!(
                actual, expected,
                "case {case:?} ({output_uri}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: an upload is `PENDING` until the instant the clock owes its completion, and
    /// `SUCCEEDED` from that instant on; an export the store refused is `FAILED` with its
    /// reason and is owed nothing.
    #[test]
    fn an_upload_is_pending_until_the_clock_owes_its_completion() {
        let started = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_800_000_000);
        let span = ::std::time::Duration::from_millis(500);
        let uri = "s3://exports/run.zip";

        // The rows differ along where the instant read at falls relative to the due one:
        // before it, exactly on it, and well past it.
        let rows: [(&str, ::std::time::Duration, &str); 3] = [
            ("before the completion is due", span / 2, EXPORT_PENDING),
            ("exactly when it is due", span, EXPORT_SUCCEEDED),
            ("well past it", span * 100, EXPORT_SUCCEEDED),
        ];
        for (case, elapsed, expected) in rows {
            let mut export = FileExport::uploading(uri.to_string(), started + span);
            assert_eq!(
                export.status, EXPORT_PENDING,
                "case {case:?}: an upload starts PENDING, got {:?}",
                export.status
            );
            export.advance(started + elapsed);
            let metadata = export.metadata();
            assert_eq!(
                metadata.status.as_deref(),
                Some(expected),
                "case {case:?}: expected {expected}, got {metadata:?}"
            );
            assert_eq!(
                metadata.output_uri.as_deref(),
                Some(uri),
                "case {case:?}: the reported URI is the object the archive went to, got \
                 {metadata:?}"
            );
            assert_eq!(
                metadata.status_reason, None,
                "case {case:?}: only a FAILED export carries a StatusReason, got {metadata:?}"
            );
        }

        let mut refused = FileExport::failed(uri.to_string(), not_uploaded_reason(uri));
        refused.advance(started + span * 100);
        let metadata = refused.metadata();
        assert_eq!(
            (
                metadata.status.as_deref(),
                metadata.status_reason.is_some(),
                metadata.output_uri.as_deref()
            ),
            (Some(EXPORT_FAILED), true, Some(uri)),
            "a refused export stays FAILED with its reason, whatever the clock does, got \
             {metadata:?}"
        );
    }

    /// RULE: the archive an export delivers carries the folders the service collects files
    /// into, and no file — nothing ran to write one.
    #[test]
    fn the_archive_holds_the_collected_folders() {
        let bytes = archive_bytes().expect("the export archive is written");
        let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes))
            .expect("the export archive is readable");
        let mut entries: Vec<String> = (0..archive.len())
            .map(|position| {
                archive
                    .by_index(position)
                    .expect("every entry of the archive is readable")
                    .name()
                    .expect("every entry of the archive names a path")
                    .to_string()
            })
            .collect();
        entries.sort();
        let mut expected: Vec<String> = COLLECTED_FOLDERS
            .iter()
            .map(|folder| (*folder).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            entries, expected,
            "the archive carries the collected folders and nothing else, got {entries:?}"
        );
    }
}
