// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's project -> build slice, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! Every assertion here is about state: what a write persisted is what a later read
//! reports, and nothing is recomputed per call.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{Build, Project};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StartBuildToken,
    StoredBuild, StoredProject,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so an ARN assertion proves the region came from the request.
///
/// `pub(crate)` here and below: the suites in this test crate drive the service the
/// same way, so they share this harness rather than each growing a copy of it.
pub(crate) const REGION: &str = "us-west-2";

/// The access key id every request below is signed with. `Build$initiator` reports the
/// calling identity, which is this.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDTEST";

/// Long enough for a build's clock-owed transition out of `IN_PROGRESS` to have fallen
/// due (the service settles it 750 ms after StartBuild).
pub(crate) const PAST_THE_SETTLE_SPAN: Duration = Duration::from_millis(1_100);

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// As [`emulator`], over an engine the caller already holds — so a test can seed or
/// inspect the service's state through the very store the handlers write.
pub(crate) fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and decoded body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// A minimal valid `CreateProject` request for `name`, with `extra`'s members merged in.
pub(crate) fn create_project_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
    });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    body
}

/// Create `name`, asserting it succeeded, and hand back the stored project.
pub(crate) async fn create_project(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let (status, body) = call(router, "CreateProject", create_project_body(name, extra)).await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {body}"
    );
    body["project"].clone()
}

/// `CreateProject` members that make a project BATCH-ENABLED, for a suite whose subject
/// is a batch build rather than the batch-enablement rule itself.
///
/// `Project$buildBatchConfig` "defines the batch build options for the project", and a
/// project that carries none cannot be batch built at all — so every suite that starts a
/// batch build creates its project with one. The suite that owns that rule is
/// `start_build_batch_validation`.
pub(crate) fn batch_enabled() -> Value {
    json!({
        "buildBatchConfig": {
            "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
            "timeoutInMins": 60,
        }
    })
}

/// `extra` with `more`'s members merged in — for a row that needs its own project
/// configuration AND one of the fragments above.
pub(crate) fn merged(mut extra: Value, more: Value) -> Value {
    if let (Value::Object(target), Value::Object(members)) = (&mut extra, more) {
        target.extend(members);
    }
    extra
}

/// Start a build of `project_name`, asserting it succeeded, and hand back the build.
pub(crate) async fn start_build(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "StartBuild", body.clone()).await;
    assert_eq!(
        status, 200,
        "StartBuild {body}: expected 200, got {status} with {response}"
    );
    response["build"].clone()
}

/// How long a suite waits for the transition the clock owes a started build, looking again
/// until the build reports itself complete. Generous on purpose: the wait ends on the
/// CONDITION, and the deadline is only there so a build that never settles fails the test
/// instead of hanging it.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long [`settled_build`] waits before looking again.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// Start a build of `project`, with `extra`'s members merged into the `StartBuild`
/// request, look again until the transition the clock owes it has fallen due, and hand back
/// the settled record `BatchGetBuilds` reports.
///
/// Shared by every suite whose subject is what a BUILD answered, so each of them drives
/// one build the same way.
pub(crate) async fn settled_build(
    router: &MultiServiceRouter,
    project: &str,
    extra: Value,
) -> Value {
    let build = start_build(router, merged(json!({"projectName": project}), extra)).await;
    let id = build["id"].as_str().expect("a build id").to_string();
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
        assert_eq!(
            status, 200,
            "BatchGetBuilds {id}: expected 200, got {status} with {read}"
        );
        let settled = read["builds"][0].clone();
        if settled["buildComplete"] == json!(true) {
            return settled;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "build {id}: expected the build to complete, last read {settled}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// The `(phaseType, phaseStatus)` pairs a build reports, in the order it reached them.
pub(crate) fn phase_pairs(build: &Value) -> Vec<(String, Option<String>)> {
    build["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|phase| {
            (
                phase["phaseType"].as_str().unwrap_or_default().to_string(),
                phase["phaseStatus"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The one phase of `build` whose `phaseType` is `phase_type`, when it reports one.
pub(crate) fn phase_of(build: &Value, phase_type: &str) -> Option<Value> {
    build["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|phase| phase["phaseType"] == json!(phase_type))
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
pub(crate) fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The `ids` member of a list answer, as strings — the shape every `List*` operation of
/// this service that reports ids uses, so the suites covering them share one reader.
///
/// A non-string entry is a defect rather than something to paper over, so it panics
/// instead of mapping to a placeholder.
pub(crate) fn listed_ids(body: &Value) -> Vec<String> {
    match body["ids"].as_array() {
        None => Vec::new(),
        Some(ids) => ids
            .iter()
            .map(|id| {
                id.as_str()
                    .unwrap_or_else(|| panic!("every entry of ids must be a string, got {id}"))
                    .to_string()
            })
            .collect(),
    }
}

pub(crate) fn expected_arn(resource: &str) -> String {
    format!(
        "arn:aws:codebuild:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A project source of `source_type` that states everything CodeBuild requires of one:
/// its buildspec inline, and — for every type that HAS input source code of its own —
/// the `location` that code lives at (`ProjectSource$location`: "If you specify
/// CODEPIPELINE for the Type property, don't specify this property. For all of the other
/// types, you must specify Location"; `NO_SOURCE` has no source code to locate).
///
/// Shared by every suite that creates a project per source type, so the rule is encoded
/// once: a suite whose subject is something else is then never decided by this one.
pub(crate) fn source_of(source_type: &str) -> Value {
    let mut source = json!({"type": source_type, "buildspec": "version: 0.2"});
    let needs_location =
        source_type != state::NO_SOURCE_TYPE && source_type != state::CODEPIPELINE_SOURCE_TYPE;
    if let (Value::Object(members), true) = (&mut source, needs_location) {
        members.insert(
            "location".to_string(),
            json!("https://example.test/repo/source"),
        );
    }
    source
}

/// The whole slice, in the order a client drives it: every read reports what the
/// preceding write persisted.
#[tokio::test]
async fn the_project_to_build_path_is_served_from_stored_state() {
    let router = emulator();

    // CreateProject stores the project and returns the stored record.
    let project = create_project(&router, "demo-proj", json!({"description": "a demo"})).await;
    assert_eq!(
        project["name"], "demo-proj",
        "CreateProject: expected name demo-proj, got {project}"
    );
    assert_eq!(
        project["arn"],
        expected_arn("project/demo-proj").as_str(),
        "CreateProject: unexpected arn in {project}"
    );
    assert_eq!(
        project["timeoutInMinutes"], 60,
        "CreateProject: absent timeoutInMinutes must default to 60, got {project}"
    );
    assert!(
        project["created"].is_number() && project["lastModified"].is_number(),
        "CreateProject: created/lastModified must be timestamps, got {project}"
    );
    assert_eq!(
        project["projectVisibility"], "PRIVATE",
        "CreateProject: a new project's builds are PRIVATE, got {project}"
    );

    // A second CreateProject for the same name is refused.
    let (status, body) = call(
        &router,
        "CreateProject",
        create_project_body("demo-proj", json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "duplicate CreateProject: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceAlreadyExistsException"),
        "duplicate CreateProject: expected ResourceAlreadyExistsException, got {body}"
    );

    // BatchGetProjects reports exactly what was stored, and nothing it does not hold.
    let (status, fetched) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["demo-proj", "nope"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetProjects: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["projects"],
        json!([project]),
        "BatchGetProjects must report the stored project verbatim, got {fetched}"
    );
    assert_eq!(
        fetched["projectsNotFound"],
        json!(["nope"]),
        "BatchGetProjects: unknown names belong in projectsNotFound, got {fetched}"
    );

    // StartBuild mints a build from the project plus this request's overrides.
    let build = start_build(
        &router,
        json!({
            "projectName": "demo-proj",
            "environmentVariablesOverride": [{"name": "FOO", "value": "bar", "type": "PLAINTEXT"}],
            "timeoutInMinutesOverride": 15,
        }),
    )
    .await;
    let build_id = build["id"].as_str().expect("the build carries an id").to_string();
    let minted = build_id
        .strip_prefix("demo-proj:")
        .unwrap_or_else(|| panic!("StartBuild: id must be <projectName>:<uuid>, got {build_id:?}"));
    assert_eq!(
        (
            minted.len(),
            minted.match_indices('-').map(|(at, _)| at).collect::<Vec<_>>(),
            minted.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        ),
        (36, vec![8, 13, 18, 23], true),
        "StartBuild: the minted id must be an 8-4-4-4-12 hex UUID, got {minted:?}"
    );
    assert_eq!(
        build["arn"],
        expected_arn(&format!("build/{build_id}")).as_str(),
        "StartBuild: unexpected arn in {build}"
    );
    assert_eq!(
        build["buildNumber"], 1,
        "StartBuild: a project's first build is number 1, got {build}"
    );
    assert_eq!(
        build["buildStatus"], "IN_PROGRESS",
        "StartBuild: a fresh build is IN_PROGRESS, got {build}"
    );
    assert_eq!(
        build["buildComplete"], false,
        "StartBuild: a fresh build is not complete, got {build}"
    );
    assert_eq!(
        build["projectName"], "demo-proj",
        "StartBuild: unexpected projectName in {build}"
    );
    assert_eq!(
        build["initiator"], ACCESS_KEY_ID,
        "StartBuild: initiator is the calling identity, got {build}"
    );
    assert!(
        build["startTime"].is_number(),
        "StartBuild: startTime must be set, got {build}"
    );
    // Resolved from the project, with the overrides applied and the flags AWS
    // documents a default for reported at the value the build RUNS WITH — the
    // project keeps what it was configured with, so `insecureSsl` is only on the
    // build.
    assert_eq!(
        build["source"]["type"], project["source"]["type"],
        "StartBuild: source comes from the project, got {build}"
    );
    assert_eq!(
        build["source"]["buildspec"], project["source"]["buildspec"],
        "StartBuild: source comes from the project, got {build}"
    );
    assert_eq!(
        build["source"]["insecureSsl"],
        json!(false),
        "StartBuild: a source that does not enable insecureSsl runs with it off, \
         got {build}"
    );
    assert_eq!(
        build["serviceRole"], project["serviceRole"],
        "StartBuild: serviceRole comes from the project, got {build}"
    );
    assert_eq!(
        build["timeoutInMinutes"], 15,
        "StartBuild: timeoutInMinutesOverride must win, got {build}"
    );
    assert_eq!(
        build["environment"]["environmentVariables"],
        json!([{"name": "FOO", "value": "bar", "type": "PLAINTEXT"}]),
        "StartBuild: environmentVariablesOverride must be merged in, got {build}"
    );

    // A second build of the same project takes the next number.
    let second = start_build(&router, json!({"projectName": "demo-proj"})).await;
    assert_eq!(
        second["buildNumber"], 2,
        "StartBuild: buildNumber increments by 1 per project, got {second}"
    );
    let second_id = second["id"].as_str().expect("an id").to_string();

    // An unknown project cannot be built.
    let (status, body) = call(&router, "StartBuild", json!({"projectName": "missing-proj"})).await;
    assert_eq!(
        status, 400,
        "StartBuild on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "StartBuild on an unknown project: expected ResourceNotFoundException, got {body}"
    );

    // BatchGetBuilds reports the persisted builds and splits out the ids it does not hold.
    let bogus = "demo-proj:00000000-0000-0000-0000-000000000000";
    let (status, fetched) = call(
        &router,
        "BatchGetBuilds",
        json!({"ids": [build_id.clone(), bogus]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds: expected 200, got {status} with {fetched}"
    );
    for member in ["id", "arn", "buildNumber", "projectName", "startTime", "source"] {
        assert_eq!(
            fetched["builds"][0][member], build[member],
            "BatchGetBuilds: {member} must match what StartBuild persisted, got {fetched}"
        );
    }
    assert_eq!(
        fetched["buildsNotFound"],
        json!([bogus]),
        "BatchGetBuilds: unknown ids belong in buildsNotFound, got {fetched}"
    );

    // ListBuildsForProject pages the stored ids, newest first by default.
    let (status, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "demo-proj"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListBuildsForProject: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["ids"],
        json!([second_id.clone(), build_id.clone()]),
        "ListBuildsForProject: default order is descending by build number, got {listed}"
    );

    // ListProjects reports the stored project names.
    let (status, projects) = call(&router, "ListProjects", json!({})).await;
    assert_eq!(
        status, 200,
        "ListProjects: expected 200, got {status} with {projects}"
    );
    assert_eq!(
        projects["projects"],
        json!(["demo-proj"]),
        "ListProjects: expected the one stored name, got {projects}"
    );

    // StopBuild mutates the stored build, and the mutation is what a later read sees.
    let (status, stopped) = call(&router, "StopBuild", json!({"id": build_id.clone()})).await;
    assert_eq!(
        status, 200,
        "StopBuild: expected 200, got {status} with {stopped}"
    );
    assert_eq!(
        stopped["build"]["buildStatus"], "STOPPED",
        "StopBuild: expected STOPPED, got {stopped}"
    );
    assert_eq!(
        stopped["build"]["buildComplete"], true,
        "StopBuild: a stopped build is complete, got {stopped}"
    );
    assert!(
        stopped["build"]["endTime"].is_number(),
        "StopBuild: endTime must be set, got {stopped}"
    );
    let (_, after_stop) = call(&router, "BatchGetBuilds", json!({"ids": [build_id.clone()]})).await;
    assert_eq!(
        after_stop["builds"][0]["buildStatus"], "STOPPED",
        "BatchGetBuilds after StopBuild must still report STOPPED, got {after_stop}"
    );

    // An unknown build cannot be stopped.
    let (status, body) = call(&router, "StopBuild", json!({"id": bogus})).await;
    assert_eq!(
        status, 400,
        "StopBuild on an unknown id: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "StopBuild on an unknown id: expected ResourceNotFoundException, got {body}"
    );

    // DeleteProject removes the project; its builds survive, as AWS documents.
    let (status, deleted) = call(&router, "DeleteProject", json!({"name": "demo-proj"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject: expected 200, got {status} with {deleted}"
    );
    let (status, gone) = call(&router, "BatchGetProjects", json!({"names": ["demo-proj"]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects after delete: expected 200, got {status} with {gone}"
    );
    assert_eq!(
        gone["projects"],
        json!([]),
        "BatchGetProjects after delete: no project may be reported, got {gone}"
    );
    assert_eq!(
        gone["projectsNotFound"],
        json!(["demo-proj"]),
        "BatchGetProjects after delete: the name belongs in projectsNotFound, got {gone}"
    );
    let (_, surviving) = call(&router, "BatchGetBuilds", json!({"ids": [build_id]})).await;
    assert_eq!(
        surviving["builds"][0]["projectName"], "demo-proj",
        "deleting a project must not delete its builds, got {surviving}"
    );
    // Deleting again is a success: the operation models no not-found error.
    let (status, again) = call(&router, "DeleteProject", json!({"name": "demo-proj"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject on an absent project: expected 200, got {status} with {again}"
    );
}

/// RULE: `CreateProject` persists every member the request carried and reports the
/// stored record, defaulting only `timeoutInMinutes` (AWS: "The default is 60
/// minutes"). The table varies the optional members the issue's example omits.
#[tokio::test]
async fn create_project_persists_each_optional_member_it_was_given() {
    let router = emulator();

    // (case, the extra members sent, the member to read back, its expected value)
    let cases: Vec<(&str, Value, &str, Value)> = vec![
        ("absent timeout", json!({}), "timeoutInMinutes", json!(60)),
        (
            "minimum timeout",
            json!({"timeoutInMinutes": 5}),
            "timeoutInMinutes",
            json!(5),
        ),
        (
            "maximum timeout",
            json!({"timeoutInMinutes": 2160}),
            "timeoutInMinutes",
            json!(2160),
        ),
        (
            "queued timeout",
            json!({"queuedTimeoutInMinutes": 480}),
            "queuedTimeoutInMinutes",
            json!(480),
        ),
        (
            "description",
            json!({"description": "release pipeline"}),
            "description",
            json!("release pipeline"),
        ),
        (
            "tags",
            json!({"tags": [{"key": "team", "value": "platform"}]}),
            "tags",
            json!([{"key": "team", "value": "platform"}]),
        ),
        (
            "encryption key",
            json!({"encryptionKey": "alias/aws/s3"}),
            "encryptionKey",
            json!("alias/aws/s3"),
        ),
        (
            "badge enabled",
            json!({"badgeEnabled": true}),
            "badge",
            json!({"badgeEnabled": true}),
        ),
        (
            "badge not enabled",
            json!({"badgeEnabled": false}),
            "badge",
            json!({"badgeEnabled": false}),
        ),
        (
            "cache",
            json!({"cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}}),
            "cache",
            json!({"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}),
        ),
        (
            "concurrent build limit",
            json!({"concurrentBuildLimit": 3}),
            "concurrentBuildLimit",
            json!(3),
        ),
    ];

    for (index, (case, extra, member, expected)) in cases.into_iter().enumerate() {
        let name = format!("case-{index}");
        let created = create_project(&router, &name, extra).await;
        assert_eq!(
            created[member], expected,
            "{case}: expected {member} = {expected}, got {created}"
        );
        // And the read path reports the same record, not a recomputed one.
        let (_, fetched) = call(&router, "BatchGetProjects", json!({"names": [name]})).await;
        assert_eq!(
            fetched["projects"][0], created,
            "{case}: BatchGetProjects must report the stored project, got {fetched}"
        );
    }
}

/// RULE: a request violating a constraint the model declares is refused with HTTP 400
/// `InvalidInputException`, naming the member and the constraint it failed; one that
/// satisfies every constraint is accepted. The table walks `ProjectName`'s published
/// bounds (`@length(min: 2, max: 150)`, `@pattern
/// ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`) and one case past each.
#[tokio::test]
async fn a_project_name_outside_its_declared_constraints_is_refused() {
    let router = emulator();

    let at_min_length = "ab";
    let at_max_length = "a".repeat(150);
    let past_max_length = "a".repeat(151);

    let accepted: Vec<(&str, String)> = vec![
        ("at the minimum length", at_min_length.to_string()),
        ("at the maximum length", at_max_length),
        ("hyphen and underscore", "A0-_z".to_string()),
    ];
    for (case, name) in accepted {
        let (status, body) = call(
            &router,
            "CreateProject",
            create_project_body(&name, json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case} ({} chars): expected 200, got {status} with {body}",
            name.len()
        );
    }

    let refused: Vec<(&str, String)> = vec![
        ("one character, below the minimum length", "a".to_string()),
        ("past the maximum length", past_max_length),
        ("leading hyphen", "-abc".to_string()),
        ("a character outside the pattern", "has space".to_string()),
        ("a dot, outside the pattern", "has.dot".to_string()),
    ];
    for (case, name) in refused {
        let (status, body) = call(
            &router,
            "CreateProject",
            create_project_body(&name, json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "{case}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains("name"),
            "{case}: the refusal must name the member, got {body}"
        );
    }

    // A required member left out is the same class of refusal.
    let (status, body) = call(&router, "CreateProject", json!({"name": "no-source"})).await;
    assert_eq!(
        status, 400,
        "absent required members: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_type(&body),
        "InvalidInputException",
        "absent required members: expected InvalidInputException, got {body}"
    );
}

/// RULE: `buildNumber` is 1 for a project's first build and increments by 1 per build,
/// per project independently; `ListBuildsForProject` orders the stored ids by build
/// number in the requested direction (descending when unspecified).
#[tokio::test]
async fn build_numbers_and_list_order_come_from_the_stored_builds() {
    let router = emulator();

    // (project, how many builds to start)
    let cases: Vec<(&str, usize)> = vec![("one-build", 1), ("three-builds", 3), ("five-builds", 5)];
    for (project, count) in &cases {
        create_project(&router, project, json!({})).await;
        let mut ids_in_start_order = Vec::new();
        for expected_number in 1..=*count {
            let build = start_build(&router, json!({"projectName": project})).await;
            assert_eq!(
                build["buildNumber"],
                json!(expected_number),
                "{project}: build {expected_number} must carry that buildNumber, got {build}"
            );
            ids_in_start_order.push(build["id"].as_str().expect("an id").to_string());
        }

        let mut newest_first = ids_in_start_order.clone();
        newest_first.reverse();
        for (case, sort_order, expected) in [
            ("unspecified sortOrder", json!(null), &newest_first),
            ("DESCENDING", json!("DESCENDING"), &newest_first),
            ("ASCENDING", json!("ASCENDING"), &ids_in_start_order),
        ] {
            let mut request = json!({"projectName": project});
            if !sort_order.is_null() {
                request["sortOrder"] = sort_order;
            }
            let (status, listed) = call(&router, "ListBuildsForProject", request).await;
            assert_eq!(
                status, 200,
                "{project} / {case}: expected 200, got {status} with {listed}"
            );
            assert_eq!(
                listed["ids"],
                json!(expected),
                "{project} / {case}: unexpected order, got {listed}"
            );
        }
    }

    // Numbering is per project, not global: the last project started at 1 too.
    let (_, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "five-builds", "sortOrder": "ASCENDING"}),
    )
    .await;
    let first_id = listed["ids"][0].as_str().expect("an id").to_string();
    let (_, fetched) = call(&router, "BatchGetBuilds", json!({"ids": [first_id]})).await;
    assert_eq!(
        fetched["builds"][0]["buildNumber"], 1,
        "each project numbers its own builds from 1, got {fetched}"
    );

    // An unknown project has no build list.
    let (status, body) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "never-created"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "ListBuildsForProject on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "ListBuildsForProject on an unknown project: expected ResourceNotFoundException, got {body}"
    );
}

/// RULE: a paginated list hands back at most one page of the stored entries plus a
/// `nextToken` that resumes after them, and a token naming nothing is refused. The
/// model bounds both lists at 100 entries, so that is the page size.
#[tokio::test]
async fn a_paginated_list_pages_over_the_stored_entries() {
    let router = emulator();

    // 105 projects: one full page, then the remainder.
    let total = 105;
    for index in 0..total {
        create_project(&router, &format!("proj-{index:03}"), json!({})).await;
    }

    let (status, first) = call(&router, "ListProjects", json!({})).await;
    assert_eq!(
        status, 200,
        "ListProjects page 1: expected 200, got {status} with {first}"
    );
    let page_one = first["projects"].as_array().expect("a page of names").clone();
    assert_eq!(
        page_one.len(),
        100,
        "ListProjects page 1: expected 100 names, got {}",
        page_one.len()
    );
    assert_eq!(
        page_one[0], "proj-000",
        "ListProjects page 1: name order is ascending by default, got {first}"
    );
    let next_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListProjects page 1 must carry a nextToken, got {first}"))
        .to_string();

    let (status, second) = call(&router, "ListProjects", json!({"nextToken": next_token})).await;
    assert_eq!(
        status, 200,
        "ListProjects page 2: expected 200, got {status} with {second}"
    );
    let page_two = second["projects"].as_array().expect("a page of names").clone();
    assert_eq!(
        page_two.len(),
        total - 100,
        "ListProjects page 2: expected {} names, got {}",
        total - 100,
        page_two.len()
    );
    assert_eq!(
        page_two[0], "proj-100",
        "ListProjects page 2 must resume after page 1, got {second}"
    );
    assert!(
        second["nextToken"].is_null(),
        "ListProjects page 2 is the last page, got {second}"
    );

    // Descending reverses the whole stored ordering, not just the page.
    let (_, descending) = call(&router, "ListProjects", json!({"sortOrder": "DESCENDING"})).await;
    assert_eq!(
        descending["projects"][0], "proj-104",
        "ListProjects DESCENDING must start at the last name, got {descending}"
    );

    // A token naming no stored entry is a refusable request, not an empty page.
    let (status, body) = call(&router, "ListProjects", json!({"nextToken": "not-a-token"})).await;
    assert_eq!(
        status, 400,
        "ListProjects with a bogus nextToken: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "ListProjects with a bogus nextToken: expected InvalidInputException, got {body}"
    );
}

/// RULE: a `StartBuild` idempotency token in force resolves to the build the first
/// request with it started (AWS: valid for 5 minutes); the same token with a changed
/// parameter is a parameter mismatch, not a second build.
#[tokio::test]
async fn an_idempotency_token_resolves_to_the_build_it_started() {
    let router = emulator();
    create_project(&router, "idem-proj", json!({})).await;

    let request = json!({"projectName": "idem-proj", "idempotencyToken": "token-1"});
    let first = start_build(&router, request.clone()).await;
    let replay = start_build(&router, request).await;
    assert_eq!(
        replay["id"], first["id"],
        "a replayed token must resolve to the same build, got {replay}"
    );
    assert_eq!(
        replay["buildNumber"], first["buildNumber"],
        "a replayed token must not consume a build number, got {replay}"
    );

    // A different token is a different build.
    let other = start_build(
        &router,
        json!({"projectName": "idem-proj", "idempotencyToken": "token-2"}),
    )
    .await;
    assert_eq!(
        other["buildNumber"], 2,
        "a new token starts the next build, got {other}"
    );

    // The same token with a changed parameter is refused.
    let (status, body) = call(
        &router,
        "StartBuild",
        json!({
            "projectName": "idem-proj",
            "idempotencyToken": "token-1",
            "buildspecOverride": "version: 0.2\nphases: {}",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a token reused with changed parameters: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "a token reused with changed parameters: expected InvalidInputException, got {body}"
    );

    // Only two builds exist.
    let (_, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "idem-proj"}),
    )
    .await;
    assert_eq!(
        listed["ids"].as_array().map(Vec::len),
        Some(2),
        "the replay and the refusal must not have started a build, got {listed}"
    );

    // A mismatch is detected in EVERY member, including the credential-bearing ones:
    // the fingerprint is a hash of the whole request, and the generated input's
    // `Debug` is the plain derive, so nothing is elided out of the comparison.
    let credentialed = |credential: &str| {
        json!({
            "projectName": "idem-proj",
            "idempotencyToken": "token-3",
            "registryCredentialOverride": {
                "credential": credential,
                "credentialProvider": "SECRETS_MANAGER",
            },
            "imagePullCredentialsTypeOverride": "SERVICE_ROLE",
            "sourceAuthOverride": {"type": "SECRETS_MANAGER", "resource": "secret-one"},
        })
    };
    let first_credentialed = start_build(&router, credentialed("arn:aws:secretsmanager:::one")).await;
    let (status, body) = call(&router, "StartBuild", credentialed("arn:aws:secretsmanager:::two")).await;
    assert_eq!(
        status, 400,
        "a token reused with a different credential: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "a token reused with a different credential: expected InvalidInputException, got {body}"
    );
    let replay = start_build(&router, credentialed("arn:aws:secretsmanager:::one")).await;
    assert_eq!(
        replay["id"], first_credentialed["id"],
        "the unchanged credentialed request must still replay to the same build, got {replay}"
    );
}

/// RULE: `environmentVariablesOverride` overrides, for this build only, the variables
/// the project already defines — replacing one of the same name in place and appending
/// the rest — and the count the model leaves unbounded is capped by this service, which
/// refuses a request past the ceiling with `InvalidInputException`.
#[tokio::test]
async fn environment_variable_overrides_are_merged_by_name_and_bounded() {
    let router = emulator();
    create_project(
        &router,
        "env-proj",
        json!({
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
                "environmentVariables": [
                    {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                    {"name": "REPLACE", "value": "project", "type": "PLAINTEXT"},
                ],
            },
        }),
    )
    .await;

    // (case, the override list, the resolved list the build must carry)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no override keeps the project's variables",
            json!(null),
            json!([
                {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "project", "type": "PLAINTEXT"},
            ]),
        ),
        (
            "an override replaces the project variable of the same name, in place",
            json!([{"name": "REPLACE", "value": "build", "type": "PLAINTEXT"}]),
            json!([
                {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "build", "type": "PLAINTEXT"},
            ]),
        ),
        (
            "a new name is appended after the project's variables",
            json!([{"name": "EXTRA", "value": "build", "type": "PLAINTEXT"}]),
            json!([
                {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "project", "type": "PLAINTEXT"},
                {"name": "EXTRA", "value": "build", "type": "PLAINTEXT"},
            ]),
        ),
        (
            "several overrides keep the order each came from",
            json!([
                {"name": "EXTRA", "value": "one", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "two", "type": "PLAINTEXT"},
                {"name": "ANOTHER", "value": "three", "type": "PLAINTEXT"},
            ]),
            json!([
                {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "two", "type": "PLAINTEXT"},
                {"name": "EXTRA", "value": "one", "type": "PLAINTEXT"},
                {"name": "ANOTHER", "value": "three", "type": "PLAINTEXT"},
            ]),
        ),
        (
            "a repeated name in the override list resolves to the last one",
            json!([
                {"name": "REPLACE", "value": "first", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "last", "type": "PLAINTEXT"},
            ]),
            json!([
                {"name": "KEEP", "value": "project", "type": "PLAINTEXT"},
                {"name": "REPLACE", "value": "last", "type": "PLAINTEXT"},
            ]),
        ),
    ];

    for (case, overrides, expected) in cases {
        let mut request = json!({"projectName": "env-proj"});
        if !overrides.is_null() {
            request["environmentVariablesOverride"] = overrides;
        }
        let build = start_build(&router, request).await;
        assert_eq!(
            build["environment"]["environmentVariables"], expected,
            "{case}: unexpected resolved variables, got {build}"
        );
    }

    // At the ceiling the request is served; one past it is refused.
    let variables = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"name": format!("VAR_{index}"), "value": "x", "type": "PLAINTEXT"}))
            .collect()
    };
    let at_ceiling = start_build(
        &router,
        json!({
            "projectName": "env-proj",
            "environmentVariablesOverride": variables(state::MAX_ENVIRONMENT_VARIABLES),
        }),
    )
    .await;
    assert_eq!(
        at_ceiling["environment"]["environmentVariables"]
            .as_array()
            .map(Vec::len),
        // The project's own two variables survive beside the overrides.
        Some(state::MAX_ENVIRONMENT_VARIABLES + 2),
        "at the ceiling every override must be merged, got {at_ceiling}"
    );

    let (status, body) = call(
        &router,
        "StartBuild",
        json!({
            "projectName": "env-proj",
            "environmentVariablesOverride": variables(state::MAX_ENVIRONMENT_VARIABLES + 1),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "one override past the ceiling: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "one override past the ceiling: expected InvalidInputException, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("environmentVariablesOverride"),
        "the refusal must name the member, got {body}"
    );
}

/// RULE: `ListProjects` orders the stored names by the requested `sortBy` criterion in
/// the requested direction. The instants are seeded through the store, so the three
/// criteria disagree with each other and a wrong key or a reversed comparison shows.
#[tokio::test]
async fn list_projects_orders_by_each_sort_by_criterion() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let handle = engine.state::<CodeBuildState>();
        let mut guard = handle.lock();
        // name, created, lastModified — deliberately three different orderings.
        for (name, created, last_modified) in [
            ("alpha", 300, 100),
            ("bravo", 100, 300),
            ("charlie", 200, 200),
        ] {
            guard.projects.insert(
                name.to_string(),
                StoredProject {
                    project: Project {
                        name: Some(name.to_string()),
                        // The ARN `CreateProject` would have stored it with: it is what
                        // records the account that owns the project, and the account's
                        // own listing reports the projects it owns.
                        arn: Some(state::project_arn(REGION, name)),
                        created: Some(state::datetime(created)),
                        last_modified: Some(state::datetime(last_modified)),
                        ..Project::default()
                    },
                    created_millis: created,
                    last_modified_millis: last_modified,
                    cache_invalidated_millis: None,
                },
            );
        }
    }
    let router = emulator_on(engine);

    // (case, sortBy, sortOrder, the names in order)
    let cases: Vec<(&str, Value, Value, Vec<&str>)> = vec![
        (
            "no sortBy lists by name ascending",
            json!(null),
            json!(null),
            vec!["alpha", "bravo", "charlie"],
        ),
        (
            "NAME ascending",
            json!("NAME"),
            json!("ASCENDING"),
            vec!["alpha", "bravo", "charlie"],
        ),
        (
            "NAME descending",
            json!("NAME"),
            json!("DESCENDING"),
            vec!["charlie", "bravo", "alpha"],
        ),
        (
            "CREATED_TIME ascending",
            json!("CREATED_TIME"),
            json!("ASCENDING"),
            vec!["bravo", "charlie", "alpha"],
        ),
        (
            "CREATED_TIME descending",
            json!("CREATED_TIME"),
            json!("DESCENDING"),
            vec!["alpha", "charlie", "bravo"],
        ),
        (
            "LAST_MODIFIED_TIME ascending",
            json!("LAST_MODIFIED_TIME"),
            json!("ASCENDING"),
            vec!["alpha", "charlie", "bravo"],
        ),
        (
            "LAST_MODIFIED_TIME descending",
            json!("LAST_MODIFIED_TIME"),
            json!("DESCENDING"),
            vec!["bravo", "charlie", "alpha"],
        ),
    ];

    for (case, sort_by, sort_order, expected) in cases {
        let mut request = json!({});
        if !sort_by.is_null() {
            request["sortBy"] = sort_by;
        }
        if !sort_order.is_null() {
            request["sortOrder"] = sort_order;
        }
        let (status, listed) = call(&router, "ListProjects", request).await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed["projects"],
            json!(expected),
            "{case}: unexpected order, got {listed}"
        );
    }
}

/// RULE: the transition out of `IN_PROGRESS` is owed by the CLOCK, so it is
/// materialised from the stored instants when the build is read — once, and identically
/// on every later read. What the build ENDS in is what running its buildspec answered
/// (the suite that owns that rule is `buildspec_execution`); this project declares no
/// command, so the build runs nothing that could fail.
#[tokio::test]
async fn a_build_settles_once_from_stored_state_and_never_changes_again() {
    let router = emulator();
    create_project(&router, "settle-proj", json!({})).await;
    let build = start_build(&router, json!({"projectName": "settle-proj"})).await;
    let id = build["id"].as_str().expect("an id").to_string();

    // Before the span elapses the caller still sees the transient state.
    let (_, early) = call(&router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
    assert_eq!(
        early["builds"][0]["buildStatus"], "IN_PROGRESS",
        "a build read straight after StartBuild is still IN_PROGRESS, got {early}"
    );

    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let (_, settled) = call(&router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
    let record = settled["builds"][0].clone();
    assert_eq!(
        record["buildStatus"], "SUCCEEDED",
        "a build whose every command succeeded ends SUCCEEDED, got {record}"
    );
    assert_eq!(
        record["buildComplete"], true,
        "a settled build is complete, got {record}"
    );
    assert_eq!(
        record["currentPhase"], "COMPLETED",
        "a settled build sits in COMPLETED, got {record}"
    );
    assert!(
        record["endTime"].is_number(),
        "a settled build carries an endTime, got {record}"
    );
    let phases = record["phases"].as_array().expect("phases").clone();
    assert!(
        phases
            .iter()
            .all(|phase| phase["phaseStatus"] != "FAULT" && phase["phaseStatus"] != "FAILED"),
        "no phase of this build failed, got {record}"
    );
    assert_eq!(
        phases.last().map(|phase| phase["phaseType"].clone()),
        Some(json!("COMPLETED")),
        "a settled build's last phase is the COMPLETED marker, got {record}"
    );

    // Read again: identical, because it was computed once from the stored instants.
    let (_, reread) = call(&router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
    assert_eq!(
        reread["builds"][0], record,
        "a settled build must never be re-fabricated per call, got {reread}"
    );

    // And stopping a build the clock already finished keeps its outcome.
    let (status, stopped) = call(&router, "StopBuild", json!({"id": id})).await;
    assert_eq!(
        status, 200,
        "StopBuild on a finished build: expected 200, got {status} with {stopped}"
    );
    assert_eq!(
        stopped["build"], record,
        "StopBuild must not rewrite a build that already finished, got {stopped}"
    );
}

/// RULE: every store this service feeds per request is bounded at its growth site —
/// the build store by eviction, the idempotency store by its documented lifetime plus
/// a ceiling, and the project store by the account quota the operation models.
#[tokio::test]
async fn each_per_request_store_is_bounded_at_its_growth_site() {
    let ceiling = i64::try_from(state::MAX_BUILDS).expect("the ceiling fits in i64");

    // Builds: one past the ceiling, and the oldest COMPLETED build is gone.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let handle = engine.state::<CodeBuildState>();
        let mut guard = handle.lock();
        for index in 0..=ceiling {
            let id = format!("bounded:{index:08}");
            guard.insert_build(
                id.clone(),
                StoredBuild {
                    build: Build {
                        id: Some(id),
                        project_name: Some("bounded".to_string()),
                        build_complete: true,
                        ..Build::default()
                    },
                    start_millis: index,
                    settle_due_millis: index,
                },
            );
        }
        assert_eq!(
            guard.builds.len(),
            state::MAX_BUILDS,
            "the build store must stay at its ceiling, got {} entries",
            guard.builds.len()
        );
        assert!(
            !guard.builds.contains_key("bounded:00000000"),
            "the oldest completed build must be the one evicted"
        );
        assert!(
            guard.builds.contains_key(&format!("bounded:{ceiling:08}")),
            "the newest build must survive the eviction round"
        );
        assert_eq!(
            guard.builds_by_project["bounded"].len(),
            state::MAX_BUILDS,
            "evicting a build must also drop its id from the project's list, got {} entries",
            guard.builds_by_project["bounded"].len()
        );

        // Idempotency entries: an entry past its documented lifetime is retired.
        let now = state::now_millis();
        guard.record_start_build_token(
            ("bounded".to_string(), "stale".to_string()),
            StartBuildToken {
                build_id: "bounded:00000001".to_string(),
                request_fingerprint: state::request_fingerprint("stale"),
                issued_millis: now - state::IDEMPOTENCY_TOKEN_TTL_MILLIS - 1,
            },
            now - state::IDEMPOTENCY_TOKEN_TTL_MILLIS - 1,
        );
        guard.record_start_build_token(
            ("bounded".to_string(), "fresh".to_string()),
            StartBuildToken {
                build_id: "bounded:00000002".to_string(),
                request_fingerprint: state::request_fingerprint("fresh"),
                issued_millis: now,
            },
            now,
        );
        assert_eq!(
            guard
                .start_build_tokens
                .keys()
                .map(|(_, token)| token.as_str())
                .collect::<Vec<_>>(),
            vec!["fresh"],
            "an expired idempotency entry must be dropped, not merely ignored"
        );
        // A fingerprint is fixed-size however large the request was.
        let short = state::request_fingerprint("x");
        let long = state::request_fingerprint(&"x".repeat(1_000_000));
        assert_eq!(
            (short.len(), long.len()),
            (64, 64),
            "a fingerprint must be a fixed-size digest, got {} and {}",
            short.len(),
            long.len()
        );
    }

    // Projects: the account quota the operation models, answered rather than grown past.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let handle = engine.state::<CodeBuildState>();
        let mut guard = handle.lock();
        for index in 0..state::MAX_PROJECTS {
            let name = format!("full-{index:06}");
            guard.projects.insert(
                name.clone(),
                StoredProject {
                    project: Project {
                        name: Some(name),
                        ..Project::default()
                    },
                    created_millis: 0,
                    last_modified_millis: 0,
                    cache_invalidated_millis: None,
                },
            );
        }
    }
    let router = emulator_on(Arc::clone(&engine));
    let (status, body) = call(
        &router,
        "CreateProject",
        create_project_body("one-too-many", json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "CreateProject at the account quota: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("AccountLimitExceededException"),
        "CreateProject at the account quota: expected AccountLimitExceededException, got {body}"
    );
    assert_eq!(
        engine.state::<CodeBuildState>().lock().projects.len(),
        state::MAX_PROJECTS,
        "a refused CreateProject must not have grown the store"
    );
}
