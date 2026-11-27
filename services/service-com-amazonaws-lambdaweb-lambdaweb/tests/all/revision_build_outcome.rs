// SPDX-License-Identifier: CC0-1.0

//! What the OUTCOME of a revision's build is, driven over the same router the emulator
//! boots with Amazon S3 mounted beside this service on one engine — exactly as the
//! assembled router mounts them.
//!
//! RULE: the asynchronous build `CreateWebFunction` / `CreateWebFunctionRevision` starts
//! fetches the deployment package the revision's `buildConfig.codeConfig.s3Object` names
//! and settles the revision `Active` ("ready to serve traffic") only when that object is
//! an archive the build can read; when the object store answers that it will not serve the
//! object, or serves bytes that are no zip archive, the build FAILS — the revision settles
//! `Failed` ("the revision build failed") carrying the build's own reason. An endpoint
//! whose `autoDeploymentMode` is `LatestRevision` then serves the newest revision that
//! BUILT, so a revision that failed never takes traffic — and that routing is settled
//! where the endpoint's mode is WRITTEN, by its create as much as by its update, so a
//! write reports the routing every later reader of the same endpoint reports.
//!
//! Each test states that rule over a SET of inputs: the rows differ in WHAT the object is
//! (an archive, an archive with several entries, bytes that are no archive, no bytes at
//! all, a truncated archive, an object the store does not hold), so the recorded case — an
//! object whose body is `b"not a zip archive"` — is one row of the class.

use std::io::Write as _;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::code_package::NOT_A_ZIP_ARCHIVE_REASON;
use service_com_amazonaws_lambdaweb_lambdaweb::state::PROVISIONING_DELAY;

use super::web_function_lifecycle::{build_config, percent_encoded, role_arn, Emulator, Reply};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// The bucket the packages below are staged in. Shared with the sibling suite that
/// needs revisions whose builds settle differently.
pub(crate) const BUCKET: &str = "lambda-web-packages";

/// How long a test waits before expecting a build to have settled: the service's own span
/// plus slack for a loaded test machine. An endpoint settles one span later again, so a
/// wait that covers the endpoint covers the revision too.
fn settled() -> Duration {
    PROVISIONING_DELAY + PROVISIONING_DELAY + Duration::from_millis(400)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a build
/// fetches its package through, so an object staged here is the object the build reads.
fn s3(emulator: &Emulator) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(emulator.router().clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create the bucket every package below is staged in.
pub(crate) async fn create_bucket(emulator: &Emulator) {
    // No `CreateBucketConfiguration`: `us-east-1` is the one Region a create must NOT
    // state a location constraint for, which Amazon S3 refuses as
    // `InvalidLocationConstraint`.
    let created = s3(emulator).create_bucket().bucket(BUCKET).send().await;
    assert!(
        created.is_ok(),
        "staging {BUCKET}: the bucket must be created, got {created:?}"
    );
}

/// Stage one object through Amazon S3's OWN operations: what a build fetches is what that
/// service really holds.
pub(crate) async fn stage_object(emulator: &Emulator, key: &str, body: Vec<u8>) {
    let put = s3(emulator)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "staging s3://{BUCKET}/{key}: the object must be stored, got {put:?}"
    );
}

/// A .zip archive carrying `entries`, written with the maintained zip writer — a real
/// archive, not bytes that happen to pass.
pub(crate) fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, contents) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("the entry starts");
        writer.write_all(contents).expect("the entry is written");
    }
    writer
        .finish()
        .expect("the archive is finished")
        .into_inner()
}

/// `PUT /web-functions` with an initial revision built from the object `key`.
async fn create_function(emulator: &Emulator, name: &str, key: &str) -> Reply {
    emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
            })),
        )
        .await
}

/// `POST /web-functions/{name}/revisions` built from the object `key`.
async fn create_revision(emulator: &Emulator, name: &str, key: &str) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/web-functions/{name}/revisions"),
            REGION,
            Some(json!({
                "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                "serviceConfig": { "executionRoleArn": role_arn(name) },
            })),
        )
        .await
}

/// `GET /web-functions/{name}/revisions/{revisionId}`.
async fn get_revision(emulator: &Emulator, name: &str, revision_id: &str) -> Value {
    let read = emulator
        .send(
            "GET",
            &format!("/web-functions/{name}/revisions/{revision_id}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "GetWebFunctionRevision {name}/{revision_id}: expected 200, got {} {:?}",
        read.status, read.body
    );
    read.body
}

/// A member of a JSON object as a string, or the empty string when it is absent.
fn text(object: &Value, name: &str) -> String {
    object
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row: the case, the object key, the bytes staged under it (`None` stages nothing at
/// all), and the `stateReason` the build settles `Failed` with (`None` for a build that
/// settles `Active`).
struct Row {
    case: &'static str,
    key: &'static str,
    staged: Option<Vec<u8>>,
    failure: Option<&'static str>,
}

/// The rows every test below drives: the class of deployment package, spanning what the
/// rule generalises over — whether there is an object at all, and whether its bytes are an
/// archive a build can read.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an archive with one entry",
            key: "one-entry.zip",
            staged: Some(archive(&[("index.mjs", b"export const handler = () => 1;")])),
            failure: None,
        },
        Row {
            case: "an archive with several entries, one of them nested",
            key: "several-entries.zip",
            staged: Some(archive(&[
                ("index.mjs", b"export const handler = () => 1;"),
                ("src/app.mjs", b"export const app = 1;"),
                ("package.json", b"{}"),
            ])),
            failure: None,
        },
        Row {
            case: "an archive with an empty entry",
            key: "empty-entry.zip",
            staged: Some(archive(&[("index.mjs", b"")])),
            failure: None,
        },
        Row {
            // The recorded case: an object whose body is plain text.
            case: "bytes that are no archive",
            key: "code-b.zip",
            staged: Some(b"not a zip archive".to_vec()),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "an object with no bytes at all",
            key: "empty.zip",
            staged: Some(Vec::new()),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "an archive whose central directory was cut off",
            key: "truncated.zip",
            staged: Some({
                let mut built = archive(&[("index.mjs", b"export const handler = () => 1;")]);
                built.truncate(built.len() / 2);
                built
            }),
            failure: Some(NOT_A_ZIP_ARCHIVE_REASON),
        },
        Row {
            case: "a key the bucket holds nothing under",
            key: "absent.zip",
            staged: None,
            failure: Some(
                "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: \
                 The specified key does not exist.",
            ),
        },
    ]
}

/// RULE: the initial revision of a create settles `Active` exactly when its code object is
/// an archive the build can read, and `Failed` carrying the build's reason otherwise.
#[tokio::test]
async fn the_initial_revisions_build_settles_on_what_its_code_object_is() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    let rows = rows();
    for row in &rows {
        if let Some(bytes) = &row.staged {
            stage_object(&emulator, row.key, bytes.clone()).await;
        }
    }
    // Every function is created first and read after ONE wait, so the suite waits out the
    // provisioning span once rather than per row.
    let mut created = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("build-outcome-{index}");
        let reply = create_function(&emulator, &name, row.key).await;
        assert_eq!(
            reply.status, 202,
            "case {:?}: CreateWebFunction expected 202, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let revision = reply
            .field("revision")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create reports its revision", row.case));
        assert_eq!(
            text(&revision, "state"),
            "Pending",
            "case {:?}: a just-created revision is still building, got {revision:?}",
            row.case
        );
        created.push((name, text(&revision, "revisionId")));
    }

    tokio::time::sleep(settled()).await;

    for (row, (name, revision_id)) in rows.iter().zip(&created) {
        let revision = get_revision(&emulator, name, revision_id).await;
        let expected_state = if row.failure.is_some() {
            "Failed"
        } else {
            "Active"
        };
        assert_eq!(
            text(&revision, "state"),
            expected_state,
            "case {:?}: expected the build to settle {expected_state}, got {revision:?}",
            row.case
        );
        if let Some(reason) = row.failure {
            assert_eq!(
                text(&revision, "stateReason"),
                reason,
                "case {:?}: expected the stateReason {reason:?}, got {revision:?}",
                row.case
            );
        }
    }
}

/// RULE, over the other create path: a revision added to an existing function settles on
/// exactly the same reading of its own code object.
#[tokio::test]
async fn a_later_revisions_build_settles_on_what_its_code_object_is() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    let rows = rows();
    for row in &rows {
        if let Some(bytes) = &row.staged {
            stage_object(&emulator, row.key, bytes.clone()).await;
        }
    }
    let name = "build-outcome-revisions";
    let created_function = create_function(&emulator, name, "one-entry.zip").await;
    assert_eq!(
        created_function.status, 202,
        "the function is created, got {}",
        created_function.status
    );

    let mut created = Vec::new();
    for row in &rows {
        let reply = create_revision(&emulator, name, row.key).await;
        assert_eq!(
            reply.status, 202,
            "case {:?}: CreateWebFunctionRevision expected 202, got {} {:?}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            text(&reply.body, "state"),
            "Pending",
            "case {:?}: a just-created revision is still building, got {:?}",
            row.case,
            reply.body
        );
        created.push(text(&reply.body, "revisionId"));
    }

    tokio::time::sleep(settled()).await;

    for (row, revision_id) in rows.iter().zip(&created) {
        let revision = get_revision(&emulator, name, revision_id).await;
        let expected_state = if row.failure.is_some() {
            "Failed"
        } else {
            "Active"
        };
        assert_eq!(
            text(&revision, "state"),
            expected_state,
            "case {:?}: expected the build to settle {expected_state}, got {revision:?}",
            row.case
        );
        if let Some(reason) = row.failure {
            assert_eq!(
                text(&revision, "stateReason"),
                reason,
                "case {:?}: expected the stateReason {reason:?}, got {revision:?}",
                row.case
            );
        }
    }
}

/// RULE: an endpoint whose `autoDeploymentMode` is `LatestRevision` serves the newest
/// revision that BUILT — so it switches onto a revision whose build succeeded and never
/// onto one that failed, while a `Disabled` endpoint keeps serving what it was given
/// whatever is created after it.
#[tokio::test]
async fn an_auto_deploying_endpoint_switches_onto_a_built_revision_and_never_onto_a_failed_one() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "deploy-a.zip",
        archive(&[("index.mjs", b"export const handler = () => 'a';")]),
    )
    .await;
    stage_object(
        &emulator,
        "deploy-c.zip",
        archive(&[("index.mjs", b"export const handler = () => 'c';")]),
    )
    .await;
    stage_object(&emulator, "deploy-b.zip", b"not a zip archive".to_vec()).await;

    // (case, the endpoint's autoDeploymentMode, whether it follows the newest built
    // revision)
    let modes: [(&str, &str, bool); 2] = [
        ("auto-deploying", "LatestRevision", true),
        ("fixed", "Disabled", false),
    ];
    for (case, mode, follows) in modes {
        let name = format!("auto-deploy-{case}");
        let created = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({
                    "functionName": name,
                    "revisionConfig": {
                        "buildConfig": build_config("nodejs24.x", BUCKET, "deploy-a.zip", None),
                        "serviceConfig": { "executionRoleArn": role_arn(&name) },
                    },
                    "endpointConfig": {
                        "endpointName": "ep",
                        "endpointType": "HomeRegion",
                        "authType": "ApplicationManaged",
                        "autoDeploymentMode": mode,
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} {:?}",
            created.status, created.body
        );
        let revision_a = text(
            created.field("revision").unwrap_or(&Value::Null),
            "revisionId",
        );

        // A revision whose code is not an archive: its build fails, so nothing may route
        // to it.
        let failed = create_revision(&emulator, &name, "deploy-b.zip").await;
        let revision_b = text(&failed.body, "revisionId");
        tokio::time::sleep(settled()).await;
        let endpoint = get_endpoint(&emulator, &name, "ep").await;
        assert_eq!(
            served_revisions(&endpoint),
            vec![revision_a.clone()],
            "case {case:?}: a revision whose build failed must take no traffic, got \
             {endpoint:?}"
        );
        assert_eq!(
            text(&get_revision(&emulator, &name, &revision_b).await, "state"),
            "Failed",
            "case {case:?}: revision B's build must have failed"
        );

        // A revision that builds: an auto-deploying endpoint follows it, a fixed one does
        // not.
        let built = create_revision(&emulator, &name, "deploy-c.zip").await;
        let revision_c = text(&built.body, "revisionId");
        tokio::time::sleep(settled()).await;
        let endpoint = get_endpoint(&emulator, &name, "ep").await;
        let expected = if follows {
            vec![revision_c.clone()]
        } else {
            vec![revision_a.clone()]
        };
        assert_eq!(
            served_revisions(&endpoint),
            expected,
            "case {case:?}: expected the endpoint to serve {expected:?}, got {endpoint:?}"
        );
    }
}

/// `GET /web-functions/{name}/endpoints/{endpointName}`.
async fn get_endpoint(emulator: &Emulator, name: &str, endpoint_name: &str) -> Value {
    let read = emulator
        .send(
            "GET",
            &format!("/web-functions/{name}/endpoints/{endpoint_name}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "GetWebFunctionEndpoint {name}/{endpoint_name}: expected 200, got {} {:?}",
        read.status, read.body
    );
    read.body
}

/// The revision ids an endpoint's `revisionWeights` route traffic to.
fn served_revisions(endpoint: &Value) -> Vec<String> {
    endpoint
        .get("revisionWeights")
        .and_then(Value::as_array)
        .map(|weights| {
            weights
                .iter()
                .map(|weight| text(weight, "revisionId"))
                .collect()
        })
        .unwrap_or_default()
}

/// `PUT /web-functions` with an initial revision built from `key` and one endpoint, in
/// the `auto_deployment_mode` the row asks for.
async fn create_function_with_endpoint(
    emulator: &Emulator,
    name: &str,
    key: &str,
    auto_deployment_mode: &str,
) -> Reply {
    emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
                "endpointConfig": {
                    "endpointName": ENDPOINT_NAME,
                    "endpointType": "HomeRegion",
                    "authType": "ApplicationManaged",
                    "autoDeploymentMode": auto_deployment_mode,
                },
            })),
        )
        .await
}

/// The endpoint every function below carries.
const ENDPOINT_NAME: &str = "ep";

/// `PATCH /web-functions/{name}/endpoints/{endpointName}` —
/// `UpdateWebFunctionEndpoint` (model: `smithy_query(query="operation",
/// operation="UpdateWebFunctionEndpoint")`).
async fn update_endpoint(emulator: &Emulator, name: &str, body: Value) -> Reply {
    emulator
        .send(
            "PATCH",
            &format!("/web-functions/{name}/endpoints/{ENDPOINT_NAME}"),
            REGION,
            Some(body),
        )
        .await
}

/// The body that asks for the auto-deployment mode under which "the endpoint
/// automatically serves the newest revision" (`AutoDeploymentMode`).
fn switch_to_latest_revision() -> Value {
    json!({ "autoDeploymentMode": "LatestRevision" })
}

/// Poll `GetWebFunctionRevision` until the revision's build has SETTLED — `Active` or
/// `Failed` — and report the state it settled in.
///
/// The end of a build is owed to the CLOCK, so the test waits for the CONDITION: it
/// looks again until it holds, or until a deadline far longer than the service's own
/// span has passed. Nothing here is decided by how fast the host runs.
async fn poll_until_settled(emulator: &Emulator, name: &str, revision_id: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let revision = get_revision(emulator, name, revision_id).await;
        let state = text(&revision, "state");
        if state != "Pending" {
            return state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the revision {name}/{revision_id} never settled, last read {revision:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: an endpoint in the `LatestRevision` auto-deployment mode serves the function's
/// newest revision, and only a revision the service reports `Active` is "ready to serve
/// traffic" (`RevisionState`) — so a request that switches an endpoint into that mode
/// while the newest revision is not `Active` conflicts with the current state of the
/// resource: it is refused `409 ConflictException`, the endpoint keeps serving what it
/// was serving, and any other change to it is still accepted.
#[tokio::test]
async fn switching_an_endpoint_onto_a_newest_revision_that_did_not_build_is_refused() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "switch-first.zip",
        archive(&[("index.mjs", b"export const handler = () => 'first';")]),
    )
    .await;
    stage_object(
        &emulator,
        "switch-newer.zip",
        archive(&[("index.mjs", b"export const handler = () => 'newer';")]),
    )
    .await;
    stage_object(&emulator, "switch-broken.zip", b"not a zip archive".to_vec()).await;

    // The rows span what the rule generalises over: the state the function's NEWEST
    // revision settled in, and whether there is an older revision behind it.
    // (case, the key the initial revision is built from, the key of a revision created
    // after it, the state the newest revision settles in, whether the switch is
    // accepted)
    let rows: [(&str, &str, Option<&str>, &str, bool); 4] = [
        (
            // The recorded case: the newest revision's build failed while an older
            // revision is serving traffic.
            "the newest revision's build failed, an older one serves",
            "switch-first.zip",
            Some("switch-broken.zip"),
            "Failed",
            false,
        ),
        (
            "the only revision's build failed",
            "switch-broken.zip",
            None,
            "Failed",
            false,
        ),
        (
            "the newest revision built",
            "switch-first.zip",
            Some("switch-newer.zip"),
            "Active",
            true,
        ),
        (
            "the only revision built",
            "switch-first.zip",
            None,
            "Active",
            true,
        ),
    ];

    for (index, (case, initial_key, later_key, settles, accepted)) in rows.into_iter().enumerate() {
        let name = format!("latest-revision-switch-{index}");
        let created = create_function_with_endpoint(&emulator, &name, initial_key, "Disabled").await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        let initial_revision = text(
            created.field("revision").unwrap_or(&Value::Null),
            "revisionId",
        );
        let newest_revision = match later_key {
            Some(key) => {
                let later = create_revision(&emulator, &name, key).await;
                assert_eq!(
                    later.status, 202,
                    "case {case:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
                    later.status, later.body
                );
                text(&later.body, "revisionId")
            }
            None => initial_revision.clone(),
        };
        // Every build has settled before the switch is attempted, so the switch's
        // outcome follows from the state the newest revision settled in.
        let state = poll_until_settled(&emulator, &name, &newest_revision).await;
        assert_eq!(
            state, settles,
            "case {case:?}: expected the newest revision's build to settle {settles}, got {state}"
        );
        let before = get_endpoint(&emulator, &name, ENDPOINT_NAME).await;

        let switched = update_endpoint(&emulator, &name, switch_to_latest_revision()).await;
        if accepted {
            assert_eq!(
                switched.status, 202,
                "case {case:?}: UpdateWebFunctionEndpoint expected 202, got {} with {:?}",
                switched.status, switched.body
            );
            assert_eq!(
                text(&switched.body, "autoDeploymentMode"),
                "LatestRevision",
                "case {case:?}: the update reports the stored mode, got {:?}",
                switched.body
            );
            // In that mode the endpoint serves the newest revision, and the read-back
            // reports the same record.
            let expected = vec![newest_revision.clone()];
            assert_eq!(
                served_revisions(&switched.body),
                expected,
                "case {case:?}: expected the endpoint to serve {expected:?}, got {:?}",
                switched.body
            );
            let read = get_endpoint(&emulator, &name, ENDPOINT_NAME).await;
            assert_eq!(
                served_revisions(&read),
                expected,
                "case {case:?}: expected the stored endpoint to serve {expected:?}, got {read:?}"
            );
            continue;
        }

        assert_eq!(
            switched.status, 409,
            "case {case:?}: expected 409, got {} with {:?}",
            switched.status, switched.body
        );
        assert_eq!(
            switched.error(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            switched.body
        );
        assert!(
            text(&switched.body, "message").contains(&newest_revision),
            "case {case:?}: the refusal names the revision it conflicts with, got {:?}",
            switched.body
        );
        let after = get_endpoint(&emulator, &name, ENDPOINT_NAME).await;
        assert_eq!(
            text(&after, "autoDeploymentMode"),
            "Disabled",
            "case {case:?}: a refused switch leaves the mode where it was, got {after:?}"
        );
        assert_eq!(
            served_revisions(&after),
            served_revisions(&before),
            "case {case:?}: a refused switch leaves the traffic where it was, got {after:?}"
        );

        // The endpoint is still updatable: it is the switch that conflicts, not the
        // endpoint.
        let described = update_endpoint(
            &emulator,
            &name,
            json!({ "description": "still updatable" }),
        )
        .await;
        assert_eq!(
            described.status, 202,
            "case {case:?}: another change to the same endpoint expected 202, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            text(&described.body, "autoDeploymentMode"),
            "Disabled",
            "case {case:?}: a change that names no mode keeps the stored one, got {:?}",
            described.body
        );
    }
}

/// RULE, at the instant the newest revision is still BUILDING: an endpoint is never
/// left serving a revision that has not built, whichever of the clock and the caller
/// got there first — the switch is either refused with the modelled conflict, in which
/// case it is accepted once that build has settled `Active`, or accepted straight away
/// onto a revision that had already built.
#[tokio::test]
async fn a_switch_onto_a_revision_still_building_waits_for_the_build() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "building-first.zip",
        archive(&[("index.mjs", b"export const handler = () => 'first';")]),
    )
    .await;
    stage_object(
        &emulator,
        "building-newer.zip",
        archive(&[("index.mjs", b"export const handler = () => 'newer';")]),
    )
    .await;

    let name = "latest-revision-switch-while-building";
    let created =
        create_function_with_endpoint(&emulator, name, "building-first.zip", "Disabled").await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    let first_revision = text(
        created.field("revision").unwrap_or(&Value::Null),
        "revisionId",
    );
    assert_eq!(
        poll_until_settled(&emulator, name, &first_revision).await,
        "Active",
        "the first revision's build must succeed"
    );

    // The switch is attempted with no wait at all after the revision was created, so
    // the newest revision may or may not have built yet.
    let later = create_revision(&emulator, name, "building-newer.zip").await;
    let newer_revision = text(&later.body, "revisionId");
    let switched = update_endpoint(&emulator, name, switch_to_latest_revision()).await;

    match switched.status {
        202 => {
            // Accepted, so the newest revision had built: the endpoint serves it.
            assert_eq!(
                served_revisions(&switched.body),
                vec![newer_revision.clone()],
                "an accepted switch serves the newest revision, got {:?}",
                switched.body
            );
            assert_eq!(
                poll_until_settled(&emulator, name, &newer_revision).await,
                "Active",
                "an accepted switch's revision must have built"
            );
        }
        409 => {
            assert_eq!(
                switched.error(),
                "ConflictException",
                "a refused switch is the modelled conflict, got {:?}",
                switched.body
            );
            let during = get_endpoint(&emulator, name, ENDPOINT_NAME).await;
            assert_eq!(
                served_revisions(&during),
                vec![first_revision.clone()],
                "a refused switch leaves the traffic where it was, got {during:?}"
            );
            // Once the build has settled `Active`, the same switch is accepted.
            assert_eq!(
                poll_until_settled(&emulator, name, &newer_revision).await,
                "Active",
                "the newest revision's build must succeed"
            );
            let again = update_endpoint(&emulator, name, switch_to_latest_revision()).await;
            assert_eq!(
                again.status, 202,
                "the switch must be accepted once the revision is Active, got {} with {:?}",
                again.status, again.body
            );
            assert_eq!(
                served_revisions(&again.body),
                vec![newer_revision.clone()],
                "the accepted switch serves the newest revision, got {:?}",
                again.body
            );
        }
        other => panic!("expected 202 or 409 from the switch, got {other} with {:?}", switched.body),
    }
}

/// The sentence this service words the refusal to auto-deploy a revision that is not
/// `Active` with: its fixed text, with the two variable parts taken from the store — the
/// latest revision's id and the state word that revision is in.
///
/// The model fixes no message text for `ConflictException` (`smithy_query(query="operation",
/// operation="UpdateWebFunctionEndpoint")` — httpError 409, documented for the class only),
/// so the wording is the service's recorded behaviour and belongs here in full.
fn latest_revision_not_deployable(revision_id: &str, state: &str) -> String {
    format!(
        "The latest revision {revision_id} is in {state} state and cannot be deployed. Only \
         revisions in Active state can be deployed. Create a new revision."
    )
}

/// The `message` member of an error body.
fn message_of(reply: &Reply) -> String {
    text(&reply.body, "message")
}

/// RULE: the `409 ConflictException` that refuses switching an endpoint into the
/// auto-deployment mode while the function's newest revision is not `Active` states which
/// revision cannot be deployed and the state it is in, the rule that only `Active`
/// revisions can be deployed, and the remedy (create a new revision) — the revision id and
/// the state word come from the store, and the refusal names no other resource.
///
/// The rows span what the rule generalises over: WHICH non-`Active` state the newest
/// revision is in (`Failed`, `Pending` — the only non-`Active` members of `RevisionState`),
/// how the build got there, and whether an older revision is serving behind it.
#[tokio::test]
async fn the_latest_revision_switch_conflict_words_the_refusal_as_the_service_does() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "message-good.zip",
        archive(&[("index.mjs", b"export const handler = () => 'good';")]),
    )
    .await;
    stage_object(&emulator, "message-broken.zip", b"not a zip archive".to_vec()).await;
    stage_object(
        &emulator,
        "message-truncated.zip",
        {
            let mut built = archive(&[("index.mjs", b"export const handler = () => 1;")]);
            built.truncate(built.len() / 2);
            built
        },
    )
    .await;
    // Staged NOWHERE: "message-absent.zip" is the key the bucket holds nothing under, so a
    // revision built from it fails for a different reason and still refuses the same way.

    // (case, the key the initial revision builds from, the key of a revision created after
    // it)
    let rows: [(&str, &str, Option<&str>); 4] = [
        (
            // The recorded case: the newest revision's build failed while an older
            // revision serves traffic.
            "the newest revision's build failed, an older one serves",
            "message-good.zip",
            Some("message-broken.zip"),
        ),
        (
            "the only revision's build failed",
            "message-broken.zip",
            None,
        ),
        (
            "the newest revision's build failed on a truncated archive",
            "message-good.zip",
            Some("message-truncated.zip"),
        ),
        (
            "the newest revision's build failed on an object the store does not hold",
            "message-good.zip",
            Some("message-absent.zip"),
        ),
    ];

    for (index, (case, initial_key, later_key)) in rows.into_iter().enumerate() {
        let name = format!("conflict-message-{index}");
        let created = create_function_with_endpoint(&emulator, &name, initial_key, "Disabled").await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        let initial_revision = text(
            created.field("revision").unwrap_or(&Value::Null),
            "revisionId",
        );
        let newest_revision = match later_key {
            Some(key) => {
                let later = create_revision(&emulator, &name, key).await;
                assert_eq!(
                    later.status, 202,
                    "case {case:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
                    later.status, later.body
                );
                text(&later.body, "revisionId")
            }
            None => initial_revision.clone(),
        };
        let settled_state = poll_until_settled(&emulator, &name, &newest_revision).await;
        assert_eq!(
            settled_state, "Failed",
            "case {case:?}: expected the newest revision's build to fail, got {settled_state}"
        );

        let switched = update_endpoint(&emulator, &name, switch_to_latest_revision()).await;
        assert_eq!(
            switched.status, 409,
            "case {case:?}: expected 409, got {} with {:?}",
            switched.status, switched.body
        );
        assert_eq!(
            switched.error(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            switched.body
        );
        let expected = latest_revision_not_deployable(&newest_revision, "Failed");
        let actual = message_of(&switched);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&name),
            "case {case:?}: the refusal names no resource but the revision, got {actual:?}"
        );
    }
}

/// RULE, for the other non-`Active` state a revision can be in: a newest revision that is
/// still `Pending` refuses the same switch with the same sentence, carrying `Pending` as
/// the state it reports that revision in.
#[tokio::test]
async fn the_latest_revision_switch_conflict_reports_a_revision_that_is_still_building() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "pending-first.zip",
        archive(&[("index.mjs", b"export const handler = () => 'first';")]),
    )
    .await;
    stage_object(
        &emulator,
        "pending-newer.zip",
        archive(&[("index.mjs", b"export const handler = () => 'newer';")]),
    )
    .await;

    let name = "conflict-message-pending";
    let created =
        create_function_with_endpoint(&emulator, name, "pending-first.zip", "Disabled").await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    let first_revision = text(
        created.field("revision").unwrap_or(&Value::Null),
        "revisionId",
    );
    assert_eq!(
        poll_until_settled(&emulator, name, &first_revision).await,
        "Active",
        "the first revision's build must succeed"
    );

    // The switch is attempted with no wait after the create, and the newest revision's
    // state is read on both sides of it. A revision's build state only ever moves AWAY from
    // `Pending`, so reading `Pending` on both sides means the refusal was decided while it
    // was `Pending`; a build that settled first is reported as that settled state instead.
    let later = create_revision(&emulator, name, "pending-newer.zip").await;
    let newer_revision = text(&later.body, "revisionId");
    let before = text(&get_revision(&emulator, name, &newer_revision).await, "state");
    let switched = update_endpoint(&emulator, name, switch_to_latest_revision()).await;
    let after = text(&get_revision(&emulator, name, &newer_revision).await, "state");

    if before == "Pending" && after == "Pending" {
        assert_eq!(
            switched.status, 409,
            "a switch onto a revision that is still building is refused, got {} with {:?}",
            switched.status, switched.body
        );
        assert_eq!(
            switched.error(),
            "ConflictException",
            "the refusal is the modelled conflict, got {:?}",
            switched.body
        );
        let expected = latest_revision_not_deployable(&newer_revision, "Pending");
        let actual = message_of(&switched);
        assert_eq!(
            actual, expected,
            "a still-building newest revision: expected the message {expected:?}, got {actual:?}"
        );
    } else {
        // The build settled before the switch was decided: either it was accepted onto an
        // `Active` revision, or refused naming the state it settled in.
        assert!(
            switched.status == 202 || switched.status == 409,
            "expected 202 or 409 from the switch, got {} with {:?}",
            switched.status, switched.body
        );
        if switched.status == 409 {
            let expected = latest_revision_not_deployable(&newer_revision, &after);
            let actual = message_of(&switched);
            assert_eq!(
                actual, expected,
                "a newest revision in {after}: expected the message {expected:?}, got {actual:?}"
            );
        }
    }

    // Once the build has settled `Active`, the same switch is accepted: the refusal is the
    // revision's state, not the endpoint.
    assert_eq!(
        poll_until_settled(&emulator, name, &newer_revision).await,
        "Active",
        "the newest revision's build must succeed"
    );
    let again = update_endpoint(&emulator, name, switch_to_latest_revision()).await;
    assert_eq!(
        again.status, 202,
        "the switch must be accepted once the revision is Active, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        text(&again.body, "autoDeploymentMode"),
        "LatestRevision",
        "the accepted switch stores the mode, got {:?}",
        again.body
    );
}

/// `DELETE /web-functions/{name}/revisions/{revisionId}` — `DeleteWebFunctionRevision`
/// (model: `smithy_query(query="operation", operation="DeleteWebFunctionRevision")`),
/// addressing the revision by `identifier` in either form the member documents,
/// percent-encoded as one URI segment.
async fn delete_revision(emulator: &Emulator, function: &str, identifier: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!(
                "/web-functions/{}/revisions/{}",
                percent_encoded(function),
                percent_encoded(identifier)
            ),
            REGION,
            None,
        )
        .await
}

/// `GET /web-functions/{name}/revisions/{revisionId}` as the wire answered it, whatever
/// the status — the read-back a delete is proved by.
async fn read_revision(emulator: &Emulator, function: &str, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/web-functions/{}/revisions/{}",
                percent_encoded(function),
                percent_encoded(identifier)
            ),
            REGION,
            None,
        )
        .await
}

/// RULE: `DeleteWebFunctionRevision` removes the named revision of the named function —
/// so a reader of this service no longer holds it — and is refused `409
/// ConflictException` while an endpoint of that function is serving it ("You cannot
/// delete a revision that is currently serving traffic on an endpoint"). It is
/// `@idempotent` and states an END STATE, so a revision the function does not carry
/// settles as the modelled `204`, while a request addressed at a function this store does
/// not hold reports that FUNCTION absent with `404 ResourceNotFoundException`.
#[tokio::test]
async fn deleting_a_revision_removes_it_unless_an_endpoint_is_serving_it() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "delete-good.zip",
        archive(&[("index.mjs", b"export const handler = () => 'good';")]),
    )
    .await;
    stage_object(&emulator, "delete-broken.zip", b"not a zip archive".to_vec()).await;

    // Each row drives one function: an endpoint serving the INITIAL revision, and a
    // second revision created behind it. The rows span what the rule generalises over:
    // which revision is named, which FORM names it (the id or the revision ARN), whether
    // an endpoint serves it, and whether it is stored at all.
    // (case, which revision the delete names, whether it is named by its ARN, the status
    // expected)
    let rows: [(&str, Target, bool, u16); 6] = [
        ("the newest revision, by its id", Target::Newest, false, 204),
        ("the newest revision, by its ARN", Target::Newest, true, 204),
        (
            "the newest revision, deleted twice",
            Target::NewestTwice,
            false,
            204,
        ),
        (
            "the revision the endpoint serves",
            Target::Served,
            false,
            409,
        ),
        (
            "a revision id nothing is stored under",
            Target::Absent,
            false,
            204,
        ),
        (
            "a revision of a function nothing is stored under",
            Target::AbsentFunction,
            false,
            404,
        ),
    ];

    for (index, (case, target, by_arn, expected_status)) in rows.into_iter().enumerate() {
        let name = format!("delete-revision-{index}");
        let created =
            create_function_with_endpoint(&emulator, &name, "delete-good.zip", "Disabled").await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} with {:?}",
            created.status, created.body
        );
        let served_revision = text(
            created.field("revision").unwrap_or(&Value::Null),
            "revisionId",
        );
        let later = create_revision(&emulator, &name, "delete-broken.zip").await;
        assert_eq!(
            later.status, 202,
            "case {case:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
            later.status, later.body
        );
        let newest_revision = text(&later.body, "revisionId");
        let newest_arn = text(&later.body, "revisionArn");

        let (function_named, identifier) = match target {
            Target::Newest | Target::NewestTwice => (
                name.clone(),
                if by_arn {
                    newest_arn.clone()
                } else {
                    newest_revision.clone()
                },
            ),
            Target::Served => (name.clone(), served_revision.clone()),
            Target::Absent => (
                name.clone(),
                "00000000-0000-0000-0000-00000000dead".to_string(),
            ),
            Target::AbsentFunction => (format!("{name}-absent"), newest_revision.clone()),
        };

        if matches!(target, Target::NewestTwice) {
            let first = delete_revision(&emulator, &function_named, &identifier).await;
            assert_eq!(
                first.status, 204,
                "case {case:?}: the first delete expected 204, got {} with {:?}",
                first.status, first.body
            );
        }

        let deleted = delete_revision(&emulator, &function_named, &identifier).await;
        assert_eq!(
            deleted.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} with {:?}",
            deleted.status, deleted.body
        );

        match target {
            Target::Newest | Target::NewestTwice => {
                // The revision the delete named is gone, and the one the endpoint serves
                // is untouched: the store, not the answer, is what the delete is proved
                // by.
                let gone = read_revision(&emulator, &name, &newest_revision).await;
                assert_eq!(
                    gone.status, 404,
                    "case {case:?}: a deleted revision is no longer readable, got {} with {:?}",
                    gone.status, gone.body
                );
                assert_eq!(
                    gone.error(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    gone.body
                );
                let expected_message =
                    format!("The revision {newest_revision} of web function {name} doesn't exist.");
                let actual = text(&gone.body, "message");
                assert_eq!(
                    actual, expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {actual:?}"
                );
                let kept = read_revision(&emulator, &name, &served_revision).await;
                assert_eq!(
                    kept.status, 200,
                    "case {case:?}: the other revisions are untouched, got {} with {:?}",
                    kept.status, kept.body
                );
            }
            Target::Absent => {
                // The end state already held, so the success removed nothing: both
                // stored revisions are still readable.
                for still_there in [&served_revision, &newest_revision] {
                    let kept = read_revision(&emulator, &name, still_there).await;
                    assert_eq!(
                        kept.status, 200,
                        "case {case:?}: the delete of an absent revision removes nothing, {} \
                         answered {} with {:?}",
                        still_there, kept.status, kept.body
                    );
                }
            }
            Target::Served => {
                assert_eq!(
                    deleted.error(),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {:?}",
                    deleted.body
                );
                let expected_message = format!(
                    "The revision {served_revision} is serving traffic on endpoint \
                     {ENDPOINT_NAME} and cannot be deleted. Shift the endpoint's traffic to \
                     another revision and try again."
                );
                let actual = text(&deleted.body, "message");
                assert_eq!(
                    actual, expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {actual:?}"
                );
                let kept = read_revision(&emulator, &name, &served_revision).await;
                assert_eq!(
                    kept.status, 200,
                    "case {case:?}: a refused delete removes nothing, got {} with {:?}",
                    kept.status, kept.body
                );
            }
            Target::AbsentFunction => {
                assert_eq!(
                    deleted.error(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    deleted.body
                );
                let expected_message = format!("The function {function_named} does not exist.");
                let actual = text(&deleted.body, "message");
                assert_eq!(
                    actual, expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {actual:?}"
                );
                let kept = read_revision(&emulator, &name, &newest_revision).await;
                assert_eq!(
                    kept.status, 200,
                    "case {case:?}: a delete of another function's revision removes nothing, got \
                     {} with {:?}",
                    kept.status, kept.body
                );
            }
        }
    }
}

/// Which revision one row of the delete table names.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The function's newest revision, which no endpoint serves.
    Newest,
    /// The newest revision, deleted once already.
    NewestTwice,
    /// The revision the function's endpoint serves.
    Served,
    /// A revision id the function carries nothing under.
    Absent,
    /// A revision of a function this store holds nothing under.
    AbsentFunction,
}

/// RULE: a function's NEWEST revision is the newest one the store still holds, so
/// deleting the newest revision moves "the newest revision" back onto the one created
/// before it — which is what the auto-deployment switch then acts on: refused while the
/// newest revision is not `Active`, accepted once the newest revision the store holds is.
#[tokio::test]
async fn deleting_the_newest_revision_moves_the_latest_revision_back() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "latest-good.zip",
        archive(&[("index.mjs", b"export const handler = () => 'good';")]),
    )
    .await;
    stage_object(&emulator, "latest-broken.zip", b"not a zip archive".to_vec()).await;

    let name = "delete-revision-latest";
    let created =
        create_function_with_endpoint(&emulator, name, "latest-good.zip", "Disabled").await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    let first_revision = text(
        created.field("revision").unwrap_or(&Value::Null),
        "revisionId",
    );
    assert_eq!(
        poll_until_settled(&emulator, name, &first_revision).await,
        "Active",
        "the first revision's build must succeed"
    );

    // A newer revision whose build fails: while it is the newest, the switch into the
    // auto-deployment mode is refused.
    let later = create_revision(&emulator, name, "latest-broken.zip").await;
    let failed_revision = text(&later.body, "revisionId");
    assert_eq!(
        poll_until_settled(&emulator, name, &failed_revision).await,
        "Failed",
        "the newer revision's build must fail"
    );
    let refused = update_endpoint(&emulator, name, switch_to_latest_revision()).await;
    assert_eq!(
        refused.status, 409,
        "the switch onto a revision that did not build is refused, got {} with {:?}",
        refused.status, refused.body
    );

    // Deleting it moves the function's newest revision back onto the one that BUILT, so
    // the same switch is now a change the service can make.
    let deleted = delete_revision(&emulator, name, &failed_revision).await;
    assert_eq!(
        deleted.status, 204,
        "deleting a revision no endpoint serves expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    let accepted = update_endpoint(&emulator, name, switch_to_latest_revision()).await;
    assert_eq!(
        accepted.status, 202,
        "the switch must be accepted once the newest revision is Active, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        text(&accepted.body, "autoDeploymentMode"),
        "LatestRevision",
        "the accepted switch stores the mode, got {:?}",
        accepted.body
    );
    assert_eq!(
        served_revisions(&accepted.body),
        vec![first_revision.clone()],
        "the endpoint serves the newest revision the store holds, got {:?}",
        accepted.body
    );
}

/// `PUT /web-functions/{name}/endpoints` — `CreateWebFunctionEndpoint` (model:
/// `smithy_query(query="operation", operation="CreateWebFunctionEndpoint")`).
async fn create_endpoint(emulator: &Emulator, name: &str, body: Value) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/web-functions/{name}/endpoints"),
            REGION,
            Some(body),
        )
        .await
}

/// The `revisionWeights` member of an endpoint object — the whole list, revision ids and
/// weights, which is what a reader of the endpoint is served.
fn weights_of(endpoint: &Value) -> Value {
    endpoint
        .get("revisionWeights")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A function with an initial revision built from `BUILT_PACKAGE` and one more revision
/// per entry of `later` (the object key it builds from, and the state its build has to
/// settle in), every build settled. Reports the revision ids in creation order, oldest
/// first.
async fn function_with_settled_revisions(
    emulator: &Emulator,
    name: &str,
    later: &[(&str, &str)],
) -> Vec<String> {
    let created = create_function(emulator, name, BUILT_PACKAGE).await;
    assert_eq!(
        created.status, 202,
        "{name}: CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    let mut revisions = vec![text(
        created.field("revision").unwrap_or(&Value::Null),
        "revisionId",
    )];
    for (key, _) in later {
        let reply = create_revision(emulator, name, key).await;
        assert_eq!(
            reply.status, 202,
            "{name}: CreateWebFunctionRevision expected 202, got {} with {:?}",
            reply.status, reply.body
        );
        revisions.push(text(&reply.body, "revisionId"));
    }
    let settles = std::iter::once("Active").chain(later.iter().map(|(_, state)| *state));
    for (revision_id, expected) in revisions.iter().zip(settles) {
        let settled = poll_until_settled(emulator, name, revision_id).await;
        assert_eq!(
            settled, expected,
            "{name}/{revision_id}: expected the build to settle {expected}, got {settled}"
        );
    }
    revisions
}

/// The package every revision that BUILDS below is built from.
const BUILT_PACKAGE: &str = "create-mode-built.zip";

/// The package a revision whose build FAILS below is built from: bytes that are no
/// archive.
const BROKEN_PACKAGE: &str = "create-mode-broken.zip";

/// One row of the create table: the mode the request asks for, the weights it names and
/// the routing the stored endpoint has to serve. Each is built from the function's
/// revision ids, oldest first.
struct CreateRow {
    case: &'static str,
    /// The `autoDeploymentMode` the request carries, or `None` when it names none — the
    /// documented default is `Disabled`.
    mode: Option<&'static str>,
    /// The `revisionWeights` the request carries, or `None` when it names none.
    requested: fn(&[String]) -> Option<Value>,
    /// The `revisionWeights` the endpoint has to serve.
    expected: fn(&[String]) -> Value,
}

/// The request body one row sends under `endpoint_name`.
fn create_endpoint_body(row: &CreateRow, endpoint_name: &str, revisions: &[String]) -> Value {
    let mut body = json!({
        "endpointName": endpoint_name,
        "endpointType": "HomeRegion",
        "authType": "IamAuth",
    });
    if let Some(mode) = row.mode {
        body["autoDeploymentMode"] = json!(mode);
    }
    if let Some(weights) = (row.requested)(revisions) {
        body["revisionWeights"] = weights;
    }
    body
}

/// RULE, on the CREATE path: an endpoint in the `LatestRevision` auto-deployment mode
/// serves the newest revision that BUILT with all of the traffic, and that routing is
/// settled where the endpoint's mode and weights are WRITTEN — so a create in that mode
/// stores, and reports, that revision whatever `revisionWeights` the request named, while
/// a create in the `Disabled` mode, where "revision routing stays fixed until you change
/// it", stores and reports exactly the weights it was given. Either way the create's own
/// answer is the routing every later reader of the same endpoint reports.
#[tokio::test]
async fn a_create_stores_the_routing_its_auto_deployment_mode_serves() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        BUILT_PACKAGE,
        archive(&[("index.mjs", b"export const handler = () => 'built';")]),
    )
    .await;

    let name = "create-auto-deployment-mode";
    // Two revisions, both ready to serve traffic: the newest is the one the mode follows.
    let revisions =
        function_with_settled_revisions(&emulator, name, &[(BUILT_PACKAGE, "Active")]).await;
    assert_ne!(
        revisions[0], revisions[1],
        "two revisions of one function must not share an id, got {revisions:?}"
    );

    // The rows span what the rule generalises over: the mode asked for, how many
    // revisions the requested weights name and with what split, and whether they name the
    // newest revision at all.
    let rows: [CreateRow; 7] = [
        CreateRow {
            // The recorded case: the mode on, one OLDER revision at full weight.
            case: "the mode on, the older revision at full weight",
            mode: Some("LatestRevision"),
            requested: |revisions| Some(json!([{ "revisionId": revisions[0], "weight": 100 }])),
            expected: |revisions| json!([{ "revisionId": revisions[1], "weight": 100 }]),
        },
        CreateRow {
            case: "the mode on, the traffic split evenly across both revisions",
            mode: Some("LatestRevision"),
            requested: |revisions| {
                Some(json!([
                    { "revisionId": revisions[0], "weight": 50 },
                    { "revisionId": revisions[1], "weight": 50 },
                ]))
            },
            expected: |revisions| json!([{ "revisionId": revisions[1], "weight": 100 }]),
        },
        CreateRow {
            case: "the mode on, a canary split that names the newest revision",
            mode: Some("LatestRevision"),
            requested: |revisions| {
                Some(json!([
                    { "revisionId": revisions[0], "weight": 99 },
                    { "revisionId": revisions[1], "weight": 1 },
                ]))
            },
            expected: |revisions| json!([{ "revisionId": revisions[1], "weight": 100 }]),
        },
        CreateRow {
            case: "the mode on, the newest revision already at full weight",
            mode: Some("LatestRevision"),
            requested: |revisions| Some(json!([{ "revisionId": revisions[1], "weight": 100 }])),
            expected: |revisions| json!([{ "revisionId": revisions[1], "weight": 100 }]),
        },
        CreateRow {
            case: "the mode on, no weights named at all",
            mode: Some("LatestRevision"),
            requested: |_| None,
            expected: |revisions| json!([{ "revisionId": revisions[1], "weight": 100 }]),
        },
        CreateRow {
            case: "the mode off, the older revision at full weight",
            mode: Some("Disabled"),
            requested: |revisions| Some(json!([{ "revisionId": revisions[0], "weight": 100 }])),
            expected: |revisions| json!([{ "revisionId": revisions[0], "weight": 100 }]),
        },
        CreateRow {
            case: "no mode named, an uneven split across both revisions",
            mode: None,
            requested: |revisions| {
                Some(json!([
                    { "revisionId": revisions[0], "weight": 70 },
                    { "revisionId": revisions[1], "weight": 30 },
                ]))
            },
            expected: |revisions| {
                json!([
                    { "revisionId": revisions[0], "weight": 70 },
                    { "revisionId": revisions[1], "weight": 30 },
                ])
            },
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let endpoint_name = format!("created-{index}");
        let created = create_endpoint(
            &emulator,
            name,
            create_endpoint_body(row, &endpoint_name, &revisions),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunctionEndpoint expected 202, got {} with {:?}",
            created.status, created.body
        );
        let expected_mode = row.mode.unwrap_or("Disabled");
        assert_eq!(
            text(&created.body, "autoDeploymentMode"),
            expected_mode,
            "case {case:?}: expected the mode {expected_mode:?}, got {:?}",
            created.body
        );
        let expected = (row.expected)(&revisions);
        assert_eq!(
            weights_of(&created.body),
            expected,
            "case {case:?}: expected the create to report {expected:?}, got {:?}",
            created.body
        );

        // The read-back, which is what the create's answer is held to: the next call to
        // this service must report the routing the create reported.
        let read = get_endpoint(&emulator, name, &endpoint_name).await;
        assert_eq!(
            weights_of(&read),
            expected,
            "case {case:?}: expected the stored endpoint to serve {expected:?}, got {read:?}"
        );
        assert_eq!(
            text(&read, "autoDeploymentMode"),
            expected_mode,
            "case {case:?}: the read reports another mode than the create did, got {read:?}"
        );

        // And a further read reports the same routing: nothing about the endpoint moves
        // because somebody looked at it.
        let again = get_endpoint(&emulator, name, &endpoint_name).await;
        assert_eq!(
            weights_of(&again),
            expected,
            "case {case:?}: the weights moved between two reads, got {again:?}"
        );
    }
}

/// RULE, where the newest revision is not one an endpoint can serve: only a revision the
/// service reports `Active` is "ready to serve traffic", so a create in the
/// `LatestRevision` mode stores the newest revision that BUILT — never one whose build
/// failed, even when the create's own weights named it.
#[tokio::test]
async fn a_create_in_the_auto_deployment_mode_never_routes_onto_a_failed_revision() {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        BUILT_PACKAGE,
        archive(&[("index.mjs", b"export const handler = () => 'built';")]),
    )
    .await;
    stage_object(&emulator, BROKEN_PACKAGE, b"not a zip archive".to_vec()).await;

    let name = "create-auto-deployment-mode-failed";
    // The newest revision's build fails, so the newest revision that BUILT is the older
    // one — the only revision an endpoint of this function can serve.
    let revisions =
        function_with_settled_revisions(&emulator, name, &[(BROKEN_PACKAGE, "Failed")]).await;

    // (case, the weights the create names, the routing the endpoint has to serve)
    let rows: [(&str, Value, Value); 3] = [
        (
            "the weights name the revision whose build failed",
            json!([{ "revisionId": revisions[1], "weight": 100 }]),
            json!([{ "revisionId": revisions[0], "weight": 100 }]),
        ),
        (
            "the weights split across the built and the failed revision",
            json!([
                { "revisionId": revisions[0], "weight": 80 },
                { "revisionId": revisions[1], "weight": 20 },
            ]),
            json!([{ "revisionId": revisions[0], "weight": 100 }]),
        ),
        (
            "the weights name only the revision that built",
            json!([{ "revisionId": revisions[0], "weight": 100 }]),
            json!([{ "revisionId": revisions[0], "weight": 100 }]),
        ),
    ];

    for (index, (case, requested, expected)) in rows.into_iter().enumerate() {
        let endpoint_name = format!("failed-newest-{index}");
        let created = create_endpoint(
            &emulator,
            name,
            json!({
                "endpointName": endpoint_name,
                "endpointType": "HomeRegion",
                "authType": "IamAuth",
                "autoDeploymentMode": "LatestRevision",
                "revisionWeights": requested,
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunctionEndpoint expected 202, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            weights_of(&created.body),
            expected,
            "case {case:?}: expected the create to report {expected:?}, got {:?}",
            created.body
        );
        let read = get_endpoint(&emulator, name, &endpoint_name).await;
        assert_eq!(
            weights_of(&read),
            expected,
            "case {case:?}: expected the stored endpoint to serve {expected:?}, got {read:?}"
        );
    }
}
