// SPDX-License-Identifier: CC0-1.0

//! A build's OUTPUT: which files of its working tree its artifacts are, where those go in
//! the account's object store, and what the build then reports about them.
//!
//! # The rule this module implements
//!
//! A build that produced output PUTS it at the destination it was started with, and the
//! `UPLOAD_ARTIFACTS` phase is that work: the buildspec's `artifacts` section says WHICH
//! files of the working tree the artifact is made of, and the destination's own members say
//! where the objects go —
//!
//! * `location` is "the name of the output bucket" (with any prefix written behind the
//!   bucket name kept as the first part of the key);
//! * `path` is "the path to the output artifact";
//! * `namespaceType` of `BUILD_ID` "include[s] the build ID in the location of the build
//!   output artifact", and `NONE` does not;
//! * `name` is "the name of the output artifact object", and "if you set the name to be a
//!   forward slash … the artifact is stored in the root of the output bucket";
//! * `packaging` of `ZIP` means "a ZIP file that contains the build output" — ONE object —
//!   while `NONE`, the default, means "a folder that contains the build output", so each
//!   file is its own object under that folder, at the path the build produced it under.
//!
//! AWS states the composition itself: "if path is set to MyArtifacts, namespaceType is set
//! to BUILD_ID, and name is set to MyArtifact.zip, then the output artifact is stored in
//! MyArtifacts/<build-ID>/MyArtifact.zip".
//!
//! An upload the build could not make is not reported as one it made: the phase ends in the
//! client error CodeBuild reports for it — the buildspec declaring no artifact file at all,
//! the paths it declared matching nothing the build produced, or the store not taking an
//! object — and the build's own `artifacts` then report no location, because nothing of what
//! the build declared arrived whole.
//!
//! What the build then REPORTS for its artifacts is read off the upload: `location` is the
//! S3 ARN of what was put there, and the checksums are stated only for the packaging AWS
//! documents them for ("This value is available only if the build project's packaging value
//! is set to ZIP").

use ::smithy_server_runtime::EmbeddedEmulator;

use super::source_archive;
use super::{ArtifactDeclaration, ARTIFACTS_TYPE_S3};
use crate::ops::types::{BuildArtifacts, PhaseContext, ProjectArtifacts};

/// The `ArtifactNamespace` value that puts the build's id in the artifact's location:
/// "BUILD_ID: Include the build ID in the location of the build output artifact."
const NAMESPACE_TYPE_BUILD_ID: &str = "BUILD_ID";

/// The `ArtifactPackaging` value that puts the whole output in ONE object: "ZIP: CodeBuild
/// creates in the output bucket a ZIP file that contains the build output."
const PACKAGING_ZIP: &str = "ZIP";

/// The `name` that names no folder of its own: "if you set the name to be a forward slash
/// ("/"), then the artifact is stored in the root of the output bucket".
const NAME_AT_THE_ROOT: &str = "/";

/// The `PhaseContext$statusCode` CodeBuild reports when the fault is the build's own
/// declaration rather than a command's — which is how it reports an artifact upload that
/// found nothing to upload.
const CLIENT_ERROR: &str = "CLIENT_ERROR";

/// What CodeBuild states when a build whose artifacts have a destination runs a buildspec
/// that names no artifact file to put there.
pub const NO_ARTIFACT_FILES_MESSAGE: &str = "No artifact files specified";

/// What CodeBuild states when the artifact paths a buildspec names match nothing to
/// upload at the build's destination.
pub const NO_MATCHING_ARTIFACT_PATHS_MESSAGE: &str = "No matching artifact paths found";

/// What is stated when the object store did not take an artifact object: the store's own
/// answer, around the key it was asked to put.
fn not_uploaded_message(key: &str) -> String {
    format!("The artifact {key} could not be uploaded to the output bucket")
}

/// Where ONE of a build's artifact destinations puts what it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDestination {
    /// The output bucket the destination names.
    pub bucket: String,
    /// The key the artifact is stored under: the folder the files go in for the packaging
    /// that produces a folder, and the object itself for the one that produces a ZIP file.
    pub key: String,
    /// Whether the whole output goes into ONE ZIP object.
    pub zipped: bool,
}

impl ArtifactDestination {
    /// The S3 ARN of what the build put here, which is what the build reports as the
    /// `location` of its artifacts.
    #[must_use]
    pub fn location(&self) -> String {
        format!("arn:aws:s3:::{}/{}", self.bucket, self.key)
    }
}

/// Where `artifacts` put the output of the build `build_id` — `None` for artifacts that
/// name no destination of their own, which is every type but the one that stores build
/// output in a bucket, and any of that type that names no bucket.
pub fn destination_of(artifacts: &ProjectArtifacts, build_id: &str) -> Option<ArtifactDestination> {
    if artifacts.r#type != ARTIFACTS_TYPE_S3 {
        return None;
    }
    let location = artifacts.location.as_deref()?.trim();
    let (bucket, prefix) = match location.split_once('/') {
        Some((bucket, prefix)) => (bucket.trim(), prefix.trim()),
        None => (location, ""),
    };
    if bucket.is_empty() {
        return None;
    }
    let namespace = artifacts
        .namespace_type
        .as_deref()
        .filter(|namespace| *namespace == NAMESPACE_TYPE_BUILD_ID)
        .map(|_| build_id)
        .unwrap_or_default();
    let name = artifacts
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != NAME_AT_THE_ROOT)
        .unwrap_or_default();
    Some(ArtifactDestination {
        bucket: bucket.to_string(),
        key: joined(&[prefix, artifacts.path.as_deref().unwrap_or_default(), namespace, name]),
        zipped: artifacts
            .packaging
            .as_deref()
            .is_some_and(|packaging| packaging == PACKAGING_ZIP),
    })
}

/// The parts of a key, joined with the one separator a key has, with every empty part left
/// out — so a destination that names no path, no namespace and no name is the bucket's own
/// root rather than a key of empty segments.
fn joined(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim().trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join("/")
}

/// The files `declaration` names out of `produced`, each with the path it is STORED under
/// at the destination.
///
/// The patterns are matched under the declaration's `base-directory` when it names one —
/// "CodeBuild … determine[s] which files and subdirectories to include" from there, and
/// the directory itself is not part of the artifact, so the stored path is relative to it —
/// and `discard-paths` stores every matched file at the root of the artifact under its own
/// file name instead. A pattern that names a DIRECTORY of the tree takes every file under
/// it, which is what `**/*` does for the whole tree.
#[must_use]
pub fn declared_files(
    declaration: &ArtifactDeclaration,
    produced: &[(String, String)],
) -> Vec<(String, String)> {
    let mut matchers = ::globset::GlobSetBuilder::new();
    let mut directories: Vec<String> = Vec::new();
    for pattern in &declaration.files {
        let pattern = pattern.trim().trim_start_matches("./");
        if pattern.is_empty() {
            continue;
        }
        if let Ok(glob) = ::globset::GlobBuilder::new(pattern)
            // `*` is one path segment's worth and `**` crosses segments, which is what
            // makes `**/*` the whole tree and `dist/*` one directory of it.
            .literal_separator(true)
            .build()
        {
            matchers.add(glob);
        }
        directories.push(pattern.trim_end_matches('/').to_string());
    }
    let Ok(matchers) = matchers.build() else {
        return Vec::new();
    };
    let base = declaration.base_directory.as_deref().unwrap_or_default();
    let mut selected: Vec<(String, String)> = Vec::new();
    for (path, contents) in produced {
        // Under the base directory, the path the patterns are matched against — and the
        // path the file is stored under — is relative to it.
        let relative = if base.is_empty() {
            path.as_str()
        } else {
            match path.strip_prefix(&format!("{base}/")) {
                Some(relative) => relative,
                None => continue,
            }
        };
        let named = matchers.is_match(relative)
            || directories
                .iter()
                .any(|directory| relative.starts_with(&format!("{directory}/")));
        if !named {
            continue;
        }
        let stored = if declaration.discard_paths {
            relative.rsplit_once('/').map_or(relative, |(_, name)| name)
        } else {
            relative
        };
        selected.push((stored.to_string(), contents.clone()));
    }
    selected
}

/// What putting a build's output at its destination answered.
#[derive(Debug, Clone)]
pub enum ArtifactUpload {
    /// The objects are at the destination, and this is what the build reports about them.
    Uploaded(BuildArtifacts),
    /// Nothing was put anywhere, and this is what the `UPLOAD_ARTIFACTS` phase reports.
    Refused(PhaseContext),
}

/// Put the files `declared` names at `destination`, through this emulator's own object
/// store, and answer what the build reports about it.
///
/// `declared` is `None` for a build whose buildspec declares no `artifacts` section at
/// all, which is a different client error from one whose declared paths matched nothing.
pub async fn upload(
    callback: &EmbeddedEmulator,
    region: &str,
    destination: &ArtifactDestination,
    reported: &BuildArtifacts,
    declared: Option<&ArtifactDeclaration>,
    produced: &[(String, String)],
) -> ArtifactUpload {
    let Some(declaration) = declared else {
        return ArtifactUpload::Refused(client_error(NO_ARTIFACT_FILES_MESSAGE));
    };
    let files = declared_files(declaration, produced);
    if files.is_empty() {
        // Either the section named no path at all, or the paths it named matched nothing
        // the build produced: CodeBuild states the two separately.
        let message = if declaration.files.is_empty() {
            NO_ARTIFACT_FILES_MESSAGE
        } else {
            NO_MATCHING_ARTIFACT_PATHS_MESSAGE
        };
        return ArtifactUpload::Refused(client_error(message));
    }
    let client = ::service_common::s3_client::s3_client(callback, region);
    // The objects this upload is made of: one per file for the packaging that produces a
    // folder, and the single archive for the one that produces a ZIP file.
    let objects: Vec<(String, Vec<u8>)> = if destination.zipped {
        let Some(archive) = source_archive::packed(&files) else {
            return ArtifactUpload::Refused(client_error(&not_uploaded_message(&destination.key)));
        };
        vec![(destination.key.clone(), archive)]
    } else {
        files
            .iter()
            .map(|(path, contents)| {
                (
                    joined(&[&destination.key, path]),
                    contents.as_bytes().to_vec(),
                )
            })
            .collect()
    };
    let mut digested: Vec<u8> = Vec::new();
    for (key, bytes) in &objects {
        digested.extend_from_slice(bytes);
        let put = client
            .put_object()
            .bucket(&destination.bucket)
            .key(key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(bytes.clone()))
            .send()
            .await;
        if let Err(error) = put {
            ::tracing::warn!(
                bucket = %destination.bucket,
                %key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a build's artifact object was not taken by the emulator's Amazon S3"
            );
            return ArtifactUpload::Refused(client_error(&not_uploaded_message(key)));
        }
    }
    ArtifactUpload::Uploaded(BuildArtifacts {
        location: Some(destination.location()),
        // Stated only for the packaging AWS documents them for: a folder of objects has no
        // one artifact to hash, so neither checksum is reported for it.
        md5sum: destination.zipped.then(|| md5_of(&digested)),
        sha256sum: destination.zipped.then(|| sha256_of(&digested)),
        ..reported.clone()
    })
}

/// The MD5 hash of the artifact's bytes, as hexadecimal.
fn md5_of(bytes: &[u8]) -> String {
    use ::md5::Digest;
    ::hex::encode(::md5::Md5::digest(bytes))
}

/// The SHA-256 hash of the artifact's bytes, as hexadecimal.
fn sha256_of(bytes: &[u8]) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(bytes))
}

/// A phase context carrying CodeBuild's client-error classification and `message`.
fn client_error(message: &str) -> PhaseContext {
    PhaseContext {
        status_code: Some(CLIENT_ERROR.to_string()),
        message: Some(message.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Artifacts of the type that stores build output in a bucket, as a row describes them.
    fn artifacts(
        location: &str,
        path: Option<&str>,
        namespace: Option<&str>,
        name: Option<&str>,
        packaging: Option<&str>,
    ) -> ProjectArtifacts {
        ProjectArtifacts {
            r#type: ARTIFACTS_TYPE_S3.to_string(),
            location: Some(location.to_string()),
            path: path.map(str::to_string),
            namespace_type: namespace.map(str::to_string),
            name: name.map(str::to_string),
            packaging: packaging.map(str::to_string),
            ..ProjectArtifacts::default()
        }
    }

    /// RULE: an artifact's key is the destination's `path`, namespace and `name` composed
    /// in that order under the bucket its `location` names, with the parts that name
    /// nothing left out.
    ///
    /// The rows differ along what the rule generalises over: which of the three parts the
    /// destination states, whether the location carries a prefix, which namespace type it
    /// asks for, and the name that names the bucket's own root.
    #[test]
    fn an_artifacts_key_is_its_destination_composed() {
        /// One row: the case, the destination's artifacts, and the bucket, key and
        /// packaging it resolves to.
        type Row = (&'static str, ProjectArtifacts, Option<(&'static str, &'static str, bool)>);
        let rows: Vec<Row> = vec![
            (
                "a path, a name and no namespace",
                artifacts("out-bucket", Some("out"), None, Some("build"), None),
                Some(("out-bucket", "out/build", false)),
            ),
            (
                "the bucket alone",
                artifacts("out-bucket", None, None, None, None),
                Some(("out-bucket", "", false)),
            ),
            (
                "a prefix written behind the bucket name",
                artifacts("out-bucket/team", Some("out"), None, Some("build"), None),
                Some(("out-bucket", "team/out/build", false)),
            ),
            (
                "the build id in the namespace",
                artifacts(
                    "out-bucket",
                    Some("MyArtifacts"),
                    Some(NAMESPACE_TYPE_BUILD_ID),
                    Some("MyArtifact.zip"),
                    Some(PACKAGING_ZIP),
                ),
                Some((
                    "out-bucket",
                    "MyArtifacts/proj:a-build-id/MyArtifact.zip",
                    true,
                )),
            ),
            (
                "a namespace type that includes nothing",
                artifacts("out-bucket", None, Some("NONE"), Some("build"), None),
                Some(("out-bucket", "build", false)),
            ),
            (
                "a name that is the bucket's own root",
                artifacts("out-bucket", None, None, Some(NAME_AT_THE_ROOT), None),
                Some(("out-bucket", "", false)),
            ),
            (
                "artifacts of a type that names no destination",
                ProjectArtifacts {
                    r#type: "NO_ARTIFACTS".to_string(),
                    location: Some("out-bucket".to_string()),
                    ..ProjectArtifacts::default()
                },
                None,
            ),
            (
                "artifacts bound for a bucket that name none",
                ProjectArtifacts {
                    r#type: ARTIFACTS_TYPE_S3.to_string(),
                    location: None,
                    ..ProjectArtifacts::default()
                },
                None,
            ),
        ];
        for (case, artifacts, expected) in rows {
            let destination = destination_of(&artifacts, "proj:a-build-id");
            let read = destination
                .as_ref()
                .map(|destination| {
                    (
                        destination.bucket.as_str(),
                        destination.key.as_str(),
                        destination.zipped,
                    )
                });
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the files an artifact is made of are the ones the declaration's patterns name,
    /// stored under the path they were produced at — relative to a `base-directory` when
    /// one is declared, and at the artifact's root when `discard-paths` asks for it.
    ///
    /// The rows differ along what the rule generalises over: an exact path, a pattern
    /// inside one directory, a pattern across directories, a base directory, discarded
    /// paths, a directory named on its own, and patterns that match nothing.
    #[test]
    fn an_artifact_is_made_of_the_files_its_declaration_names() {
        let produced: Vec<(String, String)> = [
            ("dist/app.js", "APP"),
            ("dist/inner/app.css", "CSS"),
            ("out.txt", "ROOT"),
        ]
        .into_iter()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect();

        /// One row: the case, the declaration, and the files it selects.
        type Row = (
            &'static str,
            ArtifactDeclaration,
            Vec<(&'static str, &'static str)>,
        );
        let rows: Vec<Row> = vec![
            (
                "one exact path",
                ArtifactDeclaration {
                    files: vec!["out.txt".to_string()],
                    ..ArtifactDeclaration::default()
                },
                vec![("out.txt", "ROOT")],
            ),
            (
                "one directory's files",
                ArtifactDeclaration {
                    files: vec!["dist/*".to_string()],
                    ..ArtifactDeclaration::default()
                },
                vec![("dist/app.js", "APP")],
            ),
            (
                "every file of the tree",
                ArtifactDeclaration {
                    files: vec!["**/*".to_string()],
                    ..ArtifactDeclaration::default()
                },
                vec![
                    ("dist/app.js", "APP"),
                    ("dist/inner/app.css", "CSS"),
                    ("out.txt", "ROOT"),
                ],
            ),
            (
                "a directory named on its own",
                ArtifactDeclaration {
                    files: vec!["dist".to_string()],
                    ..ArtifactDeclaration::default()
                },
                vec![("dist/app.js", "APP"), ("dist/inner/app.css", "CSS")],
            ),
            (
                "under a base directory",
                ArtifactDeclaration {
                    files: vec!["**/*".to_string()],
                    base_directory: Some("dist".to_string()),
                    ..ArtifactDeclaration::default()
                },
                vec![("app.js", "APP"), ("inner/app.css", "CSS")],
            ),
            (
                "with the paths discarded",
                ArtifactDeclaration {
                    files: vec!["**/*".to_string()],
                    discard_paths: true,
                    ..ArtifactDeclaration::default()
                },
                vec![("app.js", "APP"), ("app.css", "CSS"), ("out.txt", "ROOT")],
            ),
            (
                "a pattern that matches nothing produced",
                ArtifactDeclaration {
                    files: vec!["never-written.txt".to_string()],
                    ..ArtifactDeclaration::default()
                },
                vec![],
            ),
            (
                "no pattern at all",
                ArtifactDeclaration::default(),
                vec![],
            ),
        ];
        for (case, declaration, expected) in rows {
            let selected = declared_files(&declaration, &produced);
            let read: Vec<(&str, &str)> = selected
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.as_str()))
                .collect();
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
