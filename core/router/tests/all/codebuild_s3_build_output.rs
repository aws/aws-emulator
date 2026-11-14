// SPDX-License-Identifier: CC0-1.0

//! What a CodeBuild build PUTS in — and READS out of — the account's object store, across
//! the one emulator.
//!
//! RULE: a build's input and its output live in Amazon S3, so a build whose artifacts name
//! a destination there uploads the files its buildspec's `artifacts` section declares to
//! that destination — the objects composed from the destination's `path`, `namespaceType`,
//! `name` and `packaging`, which is AWS's own composition ("if path is set to MyArtifacts,
//! namespaceType is set to BUILD_ID, and name is set to MyArtifact.zip, then the output
//! artifact is stored in MyArtifacts/<build-ID>/MyArtifact.zip") — and reports the S3 ARN
//! of what it put there; a build whose buildspec declares no artifact file, or whose
//! declared paths match nothing it produced, uploads nothing and ends FAILED with the
//! client error CodeBuild states for it; a build that FAILED on one of its own commands
//! still uploads what it did produce; a project whose cache is held in the store carries
//! its declared cached paths from one build into the next, until `InvalidateProjectCache`
//! resets it; and a build whose source is an object of the store runs on the files that
//! archive holds.
//!
//! This suite needs more than one service on one engine — CodeBuild reaches S3 (and IAM,
//! for the project's service role) through the in-process callback, so a single-service
//! router has no store to read or write — which is why it lives here rather than in the
//! CodeBuild crate, exactly as `codebuild_artifacts_override_bucket.rs` does for the
//! bucket a start resolves. The CodeBuild crate's own suite pins what the build's commands
//! do (`buildspec_control_flow.rs`, `buildspec_file_tree.rs`), which needs no store.
//!
//! The rows span what the composition rule generalises over: which of the destination's
//! parts are stated, the packaging, the buildspec's `base-directory` and `discard-paths`,
//! and the two ways a declared artifact names nothing to upload.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// The region every request below is signed for.
const REGION: &str = "eu-central-1";

/// How long a poll waits for the transition the clock owes a build. Far above the span the
/// service itself allots a build, because what is being waited for is the CONDITION (the
/// build reporting itself complete), never an elapsed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll pauses before asking again.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// The emulator for this suite: CodeBuild, S3 and IAM on ONE engine, exactly as the router
/// mounts them, with request authentication off so each call carries a credential scope
/// (which names the Region and the service) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Dispatch one request addressed to `signing_name`, the way a signed SDK call is.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Bytes,
) -> (u16, Vec<u8>) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = router
        .dispatch(request.body(body).expect("a well-formed request"))
        .await;
    (response.status().as_u16(), response.into_body().to_vec())
}

/// One awsJson1_1 call to CodeBuild, answered with the HTTP status and the JSON body.
async fn codebuild(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let (status, bytes) = send(
        router,
        "codebuild",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1".to_string()),
            ("x-amz-target", format!("CodeBuild_20161006.{operation}")),
        ],
        Bytes::from(serde_json::to_vec(&body).expect("the test body serializes")),
    )
    .await;
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `PUT /<bucket>` — S3's `CreateBucket`, naming the Region this suite is signed for.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let (status, body) = send(
        router,
        "s3",
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml".to_string())],
        Bytes::from(format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        )),
    )
    .await;
    assert_eq!(
        status,
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {status} / {}",
        String::from_utf8_lossy(&body)
    );
}

/// The keys the bucket holds, as its own `ListObjectsV2` reports them.
async fn object_keys(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{bucket}?list-type=2"),
        &[],
        Bytes::new(),
    )
    .await;
    let body = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 200, "ListObjectsV2({bucket:?}): got {status} / {body}");
    elements(&body, "Key")
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

/// Create an IAM role CodeBuild can assume and hand back its ARN: a project's `serviceRole`
/// is resolved against the account's IAM, so the role has to exist and trust the service
/// before a project can be stored at all.
async fn codebuild_role(router: &MultiServiceRouter, name: &str) -> String {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "codebuild.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let form = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(name),
        form_encode(&document)
    );
    let (status, body) = send(
        router,
        "iam",
        "POST",
        "/",
        &[(
            "content-type",
            "application/x-www-form-urlencoded".to_string(),
        )],
        Bytes::from(form),
    )
    .await;
    let body = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "setup: CreateRole({name:?}) must succeed, got {status} / {body}"
    );
    element(&body, "Arn")
}

/// Create a project named `name` whose source carries `buildspec` inline and whose other
/// members are the ones `extra` states.
async fn create_project(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    buildspec: &str,
    extra: Value,
) {
    let mut body = json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": buildspec},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": role_arn,
    });
    if let (Value::Object(members), Value::Object(stated)) = (&mut body, &extra) {
        for (name, value) in stated {
            members.insert(name.clone(), value.clone());
        }
    }
    let (status, answer) = codebuild(router, "CreateProject", body).await;
    assert_eq!(
        status, 200,
        "setup: CreateProject({name:?}) must succeed, got {status} / {answer}"
    );
}

/// Start a build of `project` and POLL `BatchGetBuilds` until the build reports itself
/// complete, answering the settled record.
///
/// What is waited for is the build's own `buildComplete`, so the loop looks again until that
/// holds or [`SETTLE_DEADLINE`] — far above the span the service allots a build — passes.
async fn settled_build(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, started) = codebuild(router, "StartBuild", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuild({project:?}): expected 200, got {status} / {started}"
    );
    let id = started["build"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuild({project:?}) must mint a build, got {started}"))
        .to_string();
    settled(router, &id).await
}

/// Poll `BatchGetBuilds` for `id` until the build reports itself complete.
async fn settled(router: &MultiServiceRouter, id: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let (status, read) = codebuild(router, "BatchGetBuilds", json!({"ids": [id]})).await;
        assert_eq!(
            status, 200,
            "BatchGetBuilds({id:?}): expected 200, got {status} / {read}"
        );
        last = read["builds"][0].clone();
        if last["buildComplete"] == json!(true) {
            return last;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
    panic!("build {id} never reported itself complete; last read {last}")
}

/// The context message of the phase `phase_type`, or an empty string.
fn phase_message(build: &Value, phase_type: &str) -> String {
    build["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|phase| phase["phaseType"] == json!(phase_type))
        .and_then(|phase| phase["contexts"][0]["message"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The marker each row's build writes into its artifact, so the bytes at the destination can
/// be compared with what the build produced.
const MARKER: &str = "sim2real-artifact-bytes";

/// A buildspec that writes the marker into one file at the build's root and one under a
/// directory, and declares `artifacts` as `declaration` states.
fn buildspec_with(declaration: &str) -> String {
    format!(
        "version: 0.2\nphases:\n  build:\n    commands:\n      \
         - \"mkdir -p dist\"\n      \
         - \"printf '%s' '{MARKER}' > out.txt\"\n      \
         - \"printf '%s' '{MARKER}' > dist/app.txt\"\n{declaration}"
    )
}

/// One row of the composition table.
struct Row {
    case: &'static str,
    /// The `artifacts` the project is created with, beside its bucket.
    artifacts: Value,
    /// The buildspec's own `artifacts` section, as the YAML behind the phases.
    declaration: &'static str,
    /// The build's terminal status.
    status: &'static str,
    /// The keys the bucket must hold, in order.
    keys: Vec<&'static str>,
    /// The phase context the build reports for its upload, for a row that uploads nothing.
    upload_message: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a path and a name, one declared file",
            artifacts: json!({"path": "out", "name": "build", "packaging": "NONE"}),
            declaration: "artifacts:\n  files:\n    - out.txt\n",
            status: "SUCCEEDED",
            keys: vec!["out/build/out.txt"],
            upload_message: "",
        },
        Row {
            case: "the bucket alone, with no path, namespace or name",
            artifacts: json!({}),
            declaration: "artifacts:\n  files:\n    - out.txt\n",
            status: "SUCCEEDED",
            keys: vec!["out.txt"],
            upload_message: "",
        },
        Row {
            case: "every file of the tree",
            artifacts: json!({"path": "out", "name": "build"}),
            declaration: "artifacts:\n  files:\n    - '**/*'\n",
            status: "SUCCEEDED",
            keys: vec!["out/build/dist/app.txt", "out/build/out.txt"],
            upload_message: "",
        },
        Row {
            case: "under a base directory, which is itself not part of the artifact",
            artifacts: json!({"path": "out", "name": "build"}),
            declaration: "artifacts:\n  base-directory: dist\n  files:\n    - '**/*'\n",
            status: "SUCCEEDED",
            keys: vec!["out/build/app.txt"],
            upload_message: "",
        },
        Row {
            case: "with the paths discarded",
            artifacts: json!({"path": "out", "name": "build"}),
            declaration: "artifacts:\n  discard-paths: yes\n  files:\n    - dist/app.txt\n",
            status: "SUCCEEDED",
            keys: vec!["out/build/app.txt"],
            upload_message: "",
        },
        Row {
            case: "a name that is the bucket's own root",
            artifacts: json!({"name": "/"}),
            declaration: "artifacts:\n  files:\n    - dist/app.txt\n",
            status: "SUCCEEDED",
            keys: vec!["dist/app.txt"],
            upload_message: "",
        },
        Row {
            case: "declared paths that match nothing the build produced",
            artifacts: json!({"path": "out", "name": "build"}),
            declaration: "artifacts:\n  files:\n    - never-written.txt\n",
            status: "FAILED",
            keys: vec![],
            upload_message: "No matching artifact paths found",
        },
        Row {
            case: "no artifacts section at all",
            artifacts: json!({"path": "out", "name": "build"}),
            declaration: "",
            status: "FAILED",
            keys: vec![],
            upload_message: "No artifact files specified",
        },
    ]
}

/// RULE: a build's declared artifact lands at the destination its artifacts compose, with
/// the bytes the build produced, and a build that declared nothing to upload uploads
/// nothing and says why.
#[tokio::test]
async fn a_builds_declared_artifact_lands_where_its_destination_composes() {
    let router = emulator();
    let role = codebuild_role(&router, "codebuild-output-role").await;

    for (index, row) in rows().into_iter().enumerate() {
        // One bucket per row, so the keys asserted below are this row's whole output.
        let bucket = format!("codebuild-output-{index}");
        create_bucket(&router, &bucket).await;
        let project = format!("codebuild-output-proj-{index}");
        let mut artifacts = row.artifacts.clone();
        if let Value::Object(members) = &mut artifacts {
            members.insert("type".to_string(), json!("S3"));
            members.insert("location".to_string(), json!(bucket.clone()));
        }
        create_project(
            &router,
            &project,
            &role,
            &buildspec_with(row.declaration),
            json!({"artifacts": artifacts}),
        )
        .await;

        let build = settled_build(&router, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(row.status),
            "case {:?}: expected buildStatus {}, got {build}",
            row.case,
            row.status
        );
        let keys = object_keys(&router, &bucket).await;
        assert_eq!(
            keys, row.keys,
            "case {:?}: expected the keys {:?}, got {keys:?}",
            row.case, row.keys
        );
        for key in &keys {
            let bytes = object_bytes(&router, &bucket, key).await;
            let read = bytes.map(|bytes| String::from_utf8_lossy(&bytes).to_string());
            assert_eq!(
                read.as_deref(),
                Some(MARKER),
                "case {:?}: the object {key} must hold the bytes the build produced, got \
                 {read:?}",
                row.case
            );
        }
        let reported = phase_message(&build, "UPLOAD_ARTIFACTS");
        assert_eq!(
            reported, row.upload_message,
            "case {:?}: expected the upload context {:?}, got {reported:?}",
            row.case, row.upload_message
        );
        if row.keys.is_empty() {
            continue;
        }
        // A build that uploaded reports WHERE: the S3 ARN of the artifact it put there.
        let location = build["artifacts"]["location"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            location.starts_with(&format!("arn:aws:s3:::{bucket}")),
            "case {:?}: expected the artifact's own S3 ARN, got {location:?}",
            row.case
        );
    }
}

/// RULE: artifacts whose packaging is `ZIP` are ONE object — the archive of the declared
/// files — and the checksums AWS documents for that packaging are reported from its bytes.
#[tokio::test]
async fn zipped_artifacts_land_as_one_object_with_its_checksums() {
    let router = emulator();
    let role = codebuild_role(&router, "codebuild-zip-role").await;
    let bucket = "codebuild-zip-output";
    create_bucket(&router, bucket).await;
    create_project(
        &router,
        "codebuild-zip-proj",
        &role,
        &buildspec_with("artifacts:\n  files:\n    - '**/*'\n"),
        json!({"artifacts": {
            "type": "S3",
            "location": bucket,
            "path": "out",
            "name": "build.zip",
            "packaging": "ZIP",
        }}),
    )
    .await;

    let build = settled_build(&router, "codebuild-zip-proj").await;
    assert_eq!(
        build["buildStatus"],
        json!("SUCCEEDED"),
        "a zipped upload must succeed, got {build}"
    );
    let keys = object_keys(&router, bucket).await;
    assert_eq!(
        keys,
        ["out/build.zip"],
        "expected the one archive the packaging names, got {keys:?}"
    );
    let bytes = object_bytes(&router, bucket, "out/build.zip")
        .await
        .expect("the archive is served");
    assert_eq!(
        bytes.get(0..2),
        Some(b"PK".as_slice()),
        "expected an archive at the destination, got {} bytes starting {:?}",
        bytes.len(),
        bytes.get(0..4)
    );
    for member in ["md5sum", "sha256sum"] {
        let stated = build["artifacts"][member].as_str().unwrap_or_default();
        assert!(
            !stated.is_empty(),
            "expected the {member} of the archive to be reported, got {build}"
        );
    }
}

/// RULE: a build that FAILED on one of its own commands still uploads what it produced —
/// the output of the phase's `finally` block included.
#[tokio::test]
async fn a_failed_builds_output_is_still_uploaded() {
    let router = emulator();
    let role = codebuild_role(&router, "codebuild-failed-role").await;
    let bucket = "codebuild-failed-output";
    create_bucket(&router, bucket).await;
    let buildspec = "version: 0.2\nphases:\n  build:\n    commands:\n      \
         - \"echo pre-marker >> out.txt\"\n      - \"exit 7\"\n    finally:\n      \
         - \"echo finally-marker >> out.txt\"\nartifacts:\n  files:\n    - out.txt\n";
    create_project(
        &router,
        "codebuild-failed-proj",
        &role,
        buildspec,
        json!({"artifacts": {"type": "S3", "location": bucket, "name": "build"}}),
    )
    .await;

    let build = settled_build(&router, "codebuild-failed-proj").await;
    assert_eq!(
        build["buildStatus"],
        json!("FAILED"),
        "a command exiting non-zero fails the build, got {build}"
    );
    assert_eq!(
        phase_message(&build, "BUILD"),
        "Error while executing command: exit 7. Reason: exit status 7",
        "expected the command and the status it exited with, got {build}"
    );
    let keys = object_keys(&router, bucket).await;
    assert_eq!(
        keys,
        ["build/out.txt"],
        "a FAILED build still uploads what it produced, got {keys:?}"
    );
    let bytes = object_bytes(&router, bucket, "build/out.txt")
        .await
        .expect("the artifact is served");
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        "pre-marker\nfinally-marker\n",
        "expected both markers in the order they were written, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

/// RULE: a project whose cache is held in the object store carries its declared cached
/// paths from one build into the NEXT one, and a build started after
/// `InvalidateProjectCache` restores nothing that was cached before the reset.
#[tokio::test]
async fn a_projects_cached_path_carries_into_its_next_build() {
    let router = emulator();
    let role = codebuild_role(&router, "codebuild-cache-role").await;
    let bucket = "codebuild-cache-store";
    create_bucket(&router, bucket).await;
    let project = "codebuild-cache-proj";
    // The commands ask the tree whether the cached file is there: a build that finds it
    // reports a hit and the value it holds, and one that does not writes it.
    let buildspec = "version: 0.2\nenv:\n  exported-variables:\n    - STATE\n    - VALUE\n\
         phases:\n  build:\n    commands:\n      \
         - \"mkdir -p carried\"\n      \
         - \"if [ -f carried/token ]; then export STATE=hit; export VALUE=\\\"$(cat \
         carried/token)\\\"; else printf '%s' first-build > carried/token; export \
         STATE=miss; export VALUE=first-build; fi\"\ncache:\n  paths:\n    - 'carried/**/*'\n";
    create_project(
        &router,
        project,
        &role,
        buildspec,
        json!({"cache": {"type": "S3", "location": format!("{bucket}/caches")}}),
    )
    .await;

    // (case, the state the build must report)
    let first = settled_build(&router, project).await;
    assert_eq!(
        (
            first["buildStatus"].clone(),
            exported(&first, "STATE"),
            exported(&first, "VALUE")
        ),
        (json!("SUCCEEDED"), "miss".to_string(), "first-build".to_string()),
        "case \"the first build\": expected a miss that writes the cached path, got {first}"
    );
    assert!(
        !object_keys(&router, bucket).await.is_empty(),
        "case \"the first build\": the cache it saved must be an object of the store"
    );

    let second = settled_build(&router, project).await;
    assert_eq!(
        (
            second["buildStatus"].clone(),
            exported(&second, "STATE"),
            exported(&second, "VALUE")
        ),
        (json!("SUCCEEDED"), "hit".to_string(), "first-build".to_string()),
        "case \"the next build\": expected the cached file restored, got {second}"
    );

    let (status, reset) = codebuild(
        &router,
        "InvalidateProjectCache",
        json!({"projectName": project}),
    )
    .await;
    assert_eq!(
        status, 200,
        "InvalidateProjectCache: expected 200, got {status} / {reset}"
    );
    let after_reset = settled_build(&router, project).await;
    assert_eq!(
        exported(&after_reset, "STATE"),
        "miss",
        "case \"a build after the cache was reset\": expected nothing restored, got \
         {after_reset}"
    );
}

/// RULE: a build whose source is an object of the store runs on the files that ARCHIVE
/// holds, and a source the store serves nothing for leaves the build nothing to run.
#[tokio::test]
async fn a_build_runs_on_the_source_archive_its_project_names() {
    let router = emulator();
    let role = codebuild_role(&router, "codebuild-source-role").await;
    let bucket = "codebuild-source-store";
    create_bucket(&router, bucket).await;

    // The archive is written by a build of its own — artifacts packaged as ZIP are exactly
    // the form a source object takes — so the suite stages it without writing one by hand.
    create_project(
        &router,
        "codebuild-source-writer",
        &role,
        "version: 0.2\nphases:\n  build:\n    commands:\n      \
         - \"printf '%s' sim2real-source-payload > payload.txt\"\nartifacts:\n  files:\n    \
         - payload.txt\n",
        json!({"artifacts": {
            "type": "S3",
            "location": bucket,
            "name": "source.zip",
            "packaging": "ZIP",
        }}),
    )
    .await;
    let wrote = settled_build(&router, "codebuild-source-writer").await;
    assert_eq!(
        wrote["buildStatus"],
        json!("SUCCEEDED"),
        "setup: the build that stages the source archive must succeed, got {wrote}"
    );

    // (case, the source location the project names, the build's status, the value its
    // commands read out of the source tree)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "the archive the store holds",
            format!("{bucket}/source.zip"),
            "SUCCEEDED",
            "sim2real-source-payload",
        ),
        (
            "an object the store holds nothing at",
            format!("{bucket}/never-staged.zip"),
            "FAILED",
            "",
        ),
    ];
    for (index, (case, location, status, expected)) in rows.into_iter().enumerate() {
        let project = format!("codebuild-source-reader-{index}");
        let (created, answer) = codebuild(
            &router,
            "CreateProject",
            json!({
                "name": project,
                "source": {
                    "type": "S3",
                    "location": location,
                    "buildspec": "version: 0.2\nenv:\n  exported-variables:\n    - PAYLOAD\n\
                         phases:\n  build:\n    commands:\n      \
                         - \"export PAYLOAD=\\\"$(cat payload.txt)\\\"\"\n",
                },
                "artifacts": {"type": "NO_ARTIFACTS"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
                "serviceRole": role,
            }),
        )
        .await;
        assert_eq!(
            created, 200,
            "case {case:?}: CreateProject must succeed, got {created} / {answer}"
        );
        let build = settled_build(&router, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(status),
            "case {case:?}: expected buildStatus {status}, got {build}"
        );
        let payload = exported(&build, "PAYLOAD");
        assert_eq!(
            payload, expected,
            "case {case:?}: expected the payload {expected:?} out of the source, got \
             {payload:?}"
        );
        if status == "FAILED" {
            // A source the store served nothing for is reported by the phase that fetches
            // it, naming the object it was asked for.
            let reported = phase_message(&build, "DOWNLOAD_SOURCE");
            assert!(
                reported.contains("never-staged.zip"),
                "case {case:?}: expected the source object to be named, got {reported:?}"
            );
        }
    }
}

/// The value a settled build reports for the exported variable `name`, or an empty string.
fn exported(build: &Value, name: &str) -> String {
    build["exportedEnvironmentVariables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|variable| variable["name"] == json!(name))
        .and_then(|variable| variable["value"].as_str().map(str::to_string))
        .unwrap_or_default()
}
