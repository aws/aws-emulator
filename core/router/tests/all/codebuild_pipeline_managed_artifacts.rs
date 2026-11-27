// SPDX-License-Identifier: CC0-1.0

//! WHOSE destination a build's output artifacts go to, and what that decides about a
//! buildspec that declares no `artifacts` section — across the one emulator.
//!
//! RULE: a build puts its output at the destination it was started with, and two artifacts
//! types name one. A destination the CALLER configured for a bucket of its own
//! (`ProjectArtifacts$type` `S3`, whose `location` is "the name of the output bucket") is one
//! the build has to fill: a buildspec that declares no artifact file has nothing to put
//! there, which is the client error CodeBuild reports, and the build ends `FAILED`. A
//! destination CODEPIPELINE manages — the object a pipeline assigned its Build action's
//! output artifact, handed to CodeBuild when the build is started — always gets its object,
//! because that action's output artifact exists whatever the project builds: a buildspec that
//! declares no `artifacts` section at all leaves the EMPTY archive there and the build ends
//! `SUCCEEDED`. A buildspec that DOES declare a section names files it meant to produce, so
//! paths matching nothing it produced are the client error they are at either destination;
//! and artifacts that name no object at all (a project declaring the type outside any
//! pipeline) have nothing to put anywhere, which is no error either.
//!
//! This suite needs CodeBuild, Amazon S3 and IAM on ONE engine — what the build put at the
//! destination has to be readable where it put it — so it lives here and shares the harness
//! `codebuild_s3_build_output.rs` built for exactly those three services.
//!
//! The rows span what the rule generalises over: which artifacts type the build runs with,
//! whether that destination names an object, whether the buildspec declares an `artifacts`
//! section, and whether what it declares is among what the build produced.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::codebuild_s3_build_output::{
    codebuild, codebuild_role, create_bucket, emulator, phase_message, send, settled,
};

/// The marker the one file every build below produces carries, so the bytes at the
/// destination are told apart from anything else the store holds.
const MARKER: &str = "pipeline-managed-artifact";

/// The signature a ZIP archive's entry begins with ("local file header signature", ZIP
/// APPNOTE 4.3.7): an archive carrying no entry has none of them.
const ZIP_ENTRY_SIGNATURE: &[u8] = b"PK\x03\x04";

/// The signature every ZIP archive ENDS with ("end of central directory record", ZIP APPNOTE
/// 4.3.16), which is the whole of an archive that carries no entry.
const ZIP_END_SIGNATURE: &[u8] = b"PK\x05\x06";

/// The `PhaseContext$message` CodeBuild states for a build whose destination got nothing
/// because its buildspec named no artifact file at all.
const NO_ARTIFACT_FILES: &str = "No artifact files specified";

/// The `PhaseContext$message` CodeBuild states for a build whose declared artifact paths
/// matched nothing it produced.
const NO_MATCHING_PATHS: &str = "No matching artifact paths found";

/// The phase that PUTS a build's output where its artifacts say.
const UPLOAD_PHASE: &str = "UPLOAD_ARTIFACTS";

/// The `name` every destination below gives its object, which is the artifact name a
/// pipeline's Build action occupies.
const ARTIFACT_NAME: &str = "BuildOutput";

/// Which destination a row's build runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// The object a PIPELINE assigned its Build action's output artifact, as one archive at
    /// exactly that key.
    PipelineManaged,
    /// The same object, named as a destination a CALLER configured for a bucket of its own.
    CallerConfigured,
    /// Artifacts of the type CodePipeline manages that name no object — a project declaring
    /// the type, built outside any pipeline.
    NoObject,
}

impl Destination {
    /// The `artifacts` member `CreateProject` is given for this destination: the key is
    /// composed under `path`, so each row's object is its own.
    fn artifacts(self, bucket: &str, path: &str) -> Value {
        let kind = match self {
            Self::PipelineManaged | Self::NoObject => "CODEPIPELINE",
            Self::CallerConfigured => "S3",
        };
        if self == Self::NoObject {
            return json!({"type": kind});
        }
        json!({
            "type": kind,
            "location": bucket,
            "path": path,
            "name": ARTIFACT_NAME,
            "namespaceType": "NONE",
            "packaging": "ZIP",
        })
    }
}

/// One row: the case, the destination the build runs with, the buildspec's own `artifacts`
/// section, and what the build then comes to.
struct Row {
    case: &'static str,
    destination: Destination,
    /// The buildspec's `artifacts` section, as the YAML written behind its phases — empty
    /// for a buildspec that declares none at all.
    declaration: &'static str,
    /// The status the build settles at.
    status: &'static str,
    /// The `UPLOAD_ARTIFACTS` context message, empty for a build that put its output there.
    upload_message: &'static str,
    /// Whether the destination's object is in the store afterwards.
    stored: bool,
    /// The entry names that object's archive carries, in order.
    entries: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the destination a pipeline manages, and a buildspec declaring no artifacts \
                   section",
            destination: Destination::PipelineManaged,
            declaration: "",
            status: "SUCCEEDED",
            upload_message: "",
            stored: true,
            entries: &[],
        },
        Row {
            case: "the destination a pipeline manages, and a buildspec declaring the file it \
                   produced",
            destination: Destination::PipelineManaged,
            declaration: "artifacts:\n  files:\n    - out.txt\n",
            status: "SUCCEEDED",
            upload_message: "",
            stored: true,
            entries: &["out.txt"],
        },
        Row {
            case: "the destination a pipeline manages, and a buildspec declaring a file it \
                   never produced",
            destination: Destination::PipelineManaged,
            declaration: "artifacts:\n  files:\n    - never-written.txt\n",
            status: "FAILED",
            upload_message: NO_MATCHING_PATHS,
            stored: false,
            entries: &[],
        },
        Row {
            case: "a destination the caller configured, and a buildspec declaring no artifacts \
                   section",
            destination: Destination::CallerConfigured,
            declaration: "",
            status: "FAILED",
            upload_message: NO_ARTIFACT_FILES,
            stored: false,
            entries: &[],
        },
        Row {
            case: "a destination the caller configured, and a buildspec declaring the file it \
                   produced",
            destination: Destination::CallerConfigured,
            declaration: "artifacts:\n  files:\n    - out.txt\n",
            status: "SUCCEEDED",
            upload_message: "",
            stored: true,
            entries: &["out.txt"],
        },
        Row {
            case: "artifacts CodePipeline manages that name no object at all",
            destination: Destination::NoObject,
            declaration: "",
            status: "SUCCEEDED",
            upload_message: "",
            stored: false,
            entries: &[],
        },
    ]
}

/// RULE: see the module header.
#[tokio::test]
async fn a_pipeline_managed_destination_gets_its_object_whatever_the_buildspec_declares() {
    let router = emulator();
    let bucket = "codebuild-pipeline-managed";
    create_bucket(&router, bucket).await;
    let role = codebuild_role(&router, "codebuild-pipeline-managed-role").await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let project = format!("pipeline-managed-{index}");
        let key = format!("{project}/{ARTIFACT_NAME}");
        create_project(
            &router,
            &project,
            &role,
            &buildspec_with(row.declaration),
            row.destination.artifacts(bucket, &project),
        )
        .await;

        let (status, started) =
            codebuild(&router, "StartBuild", json!({"projectName": project})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartBuild must answer 200, got {status} / {started}"
        );
        let id = started["build"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start must mint a build, got {started}"))
            .to_string();
        let build = settled(&router, &id).await;

        assert_eq!(
            build["buildStatus"],
            json!(row.status),
            "case {case:?}: expected the build to settle {}, got {build}",
            row.status
        );
        let reported = phase_message(&build, UPLOAD_PHASE);
        assert_eq!(
            reported, row.upload_message,
            "case {case:?}: expected the upload phase to report {:?}, got {reported:?}",
            row.upload_message
        );

        let held = object_bytes(&router, bucket, &key).await;
        if !row.stored {
            assert!(
                held.is_none(),
                "case {case:?}: nothing may be stored at {key} for a build that put nothing \
                 there, got {} bytes",
                held.unwrap_or_default().len()
            );
            continue;
        }
        let bytes = held.unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold {bucket}/{key}, got {build}")
        });
        assert_eq!(
            build["artifacts"]["location"].as_str(),
            Some(format!("arn:aws:s3:::{bucket}/{key}").as_str()),
            "case {case:?}: the build must report the object it put, got {build}"
        );
        assert!(
            carries(&bytes, ZIP_END_SIGNATURE),
            "case {case:?}: expected an archive at {key}, got {} bytes",
            bytes.len()
        );
        for entry in row.entries {
            assert!(
                carries(&bytes, entry.as_bytes()),
                "case {case:?}: expected the archive at {key} to carry {entry:?}, got {} bytes",
                bytes.len()
            );
        }
        if row.entries.is_empty() {
            assert!(
                !carries(&bytes, ZIP_ENTRY_SIGNATURE),
                "case {case:?}: expected the EMPTY archive at {key}, got {} bytes",
                bytes.len()
            );
        }
    }
}

/// A buildspec that writes the marker into one file at the build's root and declares
/// `declaration` — which is the empty string for a buildspec that declares no `artifacts`
/// section at all.
fn buildspec_with(declaration: &str) -> String {
    format!(
        "version: 0.2\nphases:\n  build:\n    commands:\n      \
         - \"printf '%s' '{MARKER}' > out.txt\"\n{declaration}"
    )
}

/// Whether `bytes` carries `wanted` anywhere in it.
fn carries(bytes: &[u8], wanted: &[u8]) -> bool {
    bytes.windows(wanted.len()).any(|window| window == wanted)
}

/// Create a project named `name` whose source carries `buildspec` inline and whose output
/// artifacts are `artifacts`.
async fn create_project(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    buildspec: &str,
    artifacts: Value,
) {
    let (status, answer) = codebuild(
        router,
        "CreateProject",
        json!({
            "name": name,
            "source": {"type": "NO_SOURCE", "buildspec": buildspec},
            "artifacts": artifacts,
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
            },
            "serviceRole": role_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateProject({name:?}) must succeed, got {status} / {answer}"
    );
}

/// The bytes the object at `bucket`/`key` holds — `None` when the store holds none.
async fn object_bytes(router: &MultiServiceRouter, bucket: &str, key: &str) -> Option<Vec<u8>> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{bucket}/{key}"),
        &[],
        Bytes::new(),
    )
    .await;
    (status == 200).then_some(body)
}
