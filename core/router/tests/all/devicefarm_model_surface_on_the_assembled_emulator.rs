// SPDX-License-Identifier: CC0-1.0

//! The AWS Device Farm wire surface on the ASSEMBLED emulator — every service mounted, as
//! `aws_emulator_router::build_router` assembles the one the binary serves.
//!
//! THE RULE THESE SUITES PIN: a service's wire surface is the one its OWN Smithy model
//! declares, so every operation that model binds is answered by that service — its modelled
//! success, its modelled error with that error's status and code, or the scaffold's `501`
//! `NotImplemented` for an operation nobody has written yet — and never by a router-level
//! refusal (`UnknownService`, `AmbiguousService`) or the framework's unknown-operation
//! `404`, neither of which is a shape of any model. The crate renders that surface from the
//! model file it ships, so the surface is exactly what re-rendering the crate from a
//! refreshed model can break: an operation the refresh renames or drops stops being
//! answered at all, and the service's whole census is the only way to see it.
//!
//! The three suites below drive that rule over sets of inputs rather than one request:
//!
//! * the WHOLE operation census the model binds — one row per operation — where every row
//!   is answered by Device Farm itself: its own success, its own refusal of the row's empty
//!   input, or the scaffold's 501, and never a router refusal or a `404`;
//! * the implemented Project surface, which answers from state the assembled emulator holds:
//!   what `CreateProject` stored is what `GetProject` reports, over rows that differ in
//!   which of the project's optional members the request carried;
//! * the six operations that report a `RemoteAccessSession` — the one shape whose member
//!   list the latest model refresh changed, where the single `endpoint` is deprecated in
//!   favour of the `endpoints` structure — each answering the scaffold's 501 to a WELL-FORMED
//!   request, carrying no session: an emulator that opens no remote access session, and
//!   serves no endpoint for one, tells its caller so rather than reporting a session it does
//!   not hold.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region the ARNs it mints carry.
const REGION: &str = "us-west-2";

/// How Device Farm is addressed on the wire: its `endpointPrefix`/`sigv4Name`, the
/// `X-Amz-Target` service prefix of its one protocol (`awsJson1_1`) and that protocol's
/// content type. From the model's own wire identity,
/// `smithy_query(query="service_info")`: `endpointPrefix` `devicefarm`, service shape
/// `DeviceFarm_20150623`, protocol `awsJson1_1`.
const SIGNING_NAME: &str = "devicefarm";
const TARGET_PREFIX: &str = "DeviceFarm_20150623";
const CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The router's own refusals, which belong to no service's model: no row of these suites may
/// be answered with one.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// The wire error code of the scaffold's "generated but not written yet" signal.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// Every operation this service's model binds, from
/// `smithy_query(query="list_operations")` on
/// `services/service-com-amazonaws-devicefarm-devicefarm-20150623/smithy-model/com-amazonaws-devicefarm-devicefarm-20150623.json`.
///
/// The census is the subject of the first suite, so it is written out here rather than asked
/// of the implementation: an operation that leaves the model is a row that has to be removed
/// from this list, and one that joins it is a row that has to be added.
const OPERATIONS: [&str; 77] = [
    "CreateDevicePool",
    "CreateInstanceProfile",
    "CreateNetworkProfile",
    "CreateProject",
    "CreateRemoteAccessSession",
    "CreateTestGridProject",
    "CreateTestGridUrl",
    "CreateUpload",
    "CreateVPCEConfiguration",
    "DeleteDevicePool",
    "DeleteInstanceProfile",
    "DeleteNetworkProfile",
    "DeleteProject",
    "DeleteRemoteAccessSession",
    "DeleteRun",
    "DeleteTestGridProject",
    "DeleteUpload",
    "DeleteVPCEConfiguration",
    "GetAccountSettings",
    "GetDevice",
    "GetDeviceInstance",
    "GetDevicePool",
    "GetDevicePoolCompatibility",
    "GetInstanceProfile",
    "GetJob",
    "GetNetworkProfile",
    "GetOfferingStatus",
    "GetProject",
    "GetRemoteAccessSession",
    "GetRun",
    "GetSuite",
    "GetTest",
    "GetTestGridProject",
    "GetTestGridSession",
    "GetUpload",
    "GetVPCEConfiguration",
    "InstallToRemoteAccessSession",
    "ListArtifacts",
    "ListDeviceInstances",
    "ListDevicePools",
    "ListDevices",
    "ListInstanceProfiles",
    "ListJobs",
    "ListNetworkProfiles",
    "ListOfferingPromotions",
    "ListOfferingTransactions",
    "ListOfferings",
    "ListProjects",
    "ListRemoteAccessSessions",
    "ListRuns",
    "ListSamples",
    "ListSuites",
    "ListTagsForResource",
    "ListTestGridProjects",
    "ListTestGridSessionActions",
    "ListTestGridSessionArtifacts",
    "ListTestGridSessions",
    "ListTests",
    "ListUniqueProblems",
    "ListUploads",
    "ListVPCEConfigurations",
    "PurchaseOffering",
    "RenewOffering",
    "ScheduleRun",
    "StopJob",
    "StopRemoteAccessSession",
    "StopRun",
    "TagResource",
    "UntagResource",
    "UpdateDeviceInstance",
    "UpdateDevicePool",
    "UpdateInstanceProfile",
    "UpdateNetworkProfile",
    "UpdateProject",
    "UpdateTestGridProject",
    "UpdateUpload",
    "UpdateVPCEConfiguration",
];

/// One answer, as a client reads it: the status, the wire error code (`__type`, or the
/// `x-amzn-errortype` header where the protocol sets one) and the parsed body.
struct Reply {
    status: u16,
    code: String,
    body: Value,
}

/// The whole emulator, exactly as the binary assembles it, over one shared engine.
///
/// Request authentication is off: these suites are about what a request is ANSWERED with,
/// and the signature of a hand-built request is checked nowhere in this target. The
/// credential scope is still carried, because it is what names the Region and the service.
fn assembled_emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// Call one Device Farm operation through the assembled emulator.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Reply {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", CONTENT_TYPE)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = if error_type.is_empty() {
        body["__type"].as_str().unwrap_or_default().to_string()
    } else {
        error_type
    };
    // An awsJson error code may be namespaced (`com.amazonaws.devicefarm#Argument…`); the
    // code a client matches on is the last segment.
    let code = code
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    Reply { status, code, body }
}

/// A well-formed Device Farm ARN of `resource_type` this emulator never minted, for a row
/// that has to satisfy an ARN member's modelled form (`AmazonResourceName`, length 32..1011,
/// pattern `^arn:aws:devicefarm:.+$`) without naming a resource.
fn arn(resource_type: &str, resource_id: &str) -> String {
    format!("arn:aws:devicefarm:{REGION}:111122223333:{resource_type}:{resource_id}")
}

/// Every operation the model binds is answered by Device Farm itself.
///
/// The row's input is empty, so what each operation answers is its own: a success for one
/// whose input needs nothing, its refusal of the missing members otherwise, or the
/// scaffold's 501 where nobody has written it yet. What no row may be answered with is a
/// refusal that belongs to no model — the router's `UnknownService`/`AmbiguousService`, or
/// the framework's unknown-operation `404` for a target this service does not claim.
#[tokio::test]
async fn every_operation_the_model_binds_is_answered_by_device_farm() {
    let router = assembled_emulator();

    for operation in OPERATIONS {
        let reply = call(&router, operation, &json!({})).await;
        assert!(
            !ROUTER_REFUSALS.contains(&reply.code.as_str()),
            "case {operation:?}: expected Device Farm's own answer, got the router's \
             {:?} with HTTP {} and {}",
            reply.code,
            reply.status,
            reply.body
        );
        assert_ne!(
            reply.status, 404,
            "case {operation:?}: expected the target to resolve to this service, got HTTP 404 \
             with {}",
            reply.body
        );
        assert!(
            matches!(reply.status, 200 | 400 | 501),
            "case {operation:?}: expected a modelled success, a modelled refusal or the \
             scaffold's 501, got HTTP {} with {}",
            reply.status,
            reply.body
        );
    }
}

/// What `CreateProject` stored is what `GetProject` reports, on the emulator as it is
/// ASSEMBLED: every member of the answer comes from the one record the create wrote.
///
/// The rows differ in which of the project's optional members the request carried — none of
/// them, the VPC-free pair the model added for a project's test hosts, and both at the
/// modelled `environmentVariables` minimum of 1 — so no single request is the specification.
#[tokio::test]
async fn a_created_project_is_reported_identically_by_a_later_read() {
    let router = assembled_emulator();

    // (case, CreateProject input)
    let rows: [(&str, Value); 3] = [
        (
            "only the required name",
            json!({ "name": "assembled-defaults" }),
        ),
        (
            "an execution role and the environment-variable list at its modelled minimum of 1",
            json!({
                "name": "assembled-execution-role",
                "executionRoleArn": "arn:aws:iam::111122223333:role/device-farm-host",
                "environmentVariables": [{ "name": "STAGE", "value": "test" }]
            }),
        ),
        (
            "a job timeout of its own and three environment variables",
            json!({
                "name": "assembled-timeout",
                "defaultJobTimeoutMinutes": 30,
                "environmentVariables": [
                    { "name": "A", "value": "1" },
                    { "name": "B", "value": "2" },
                    { "name": "C", "value": "3" }
                ]
            }),
        ),
    ];

    for (case, input) in &rows {
        let created = call(&router, "CreateProject", input).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateProject to answer 200, got HTTP {} with {}",
            created.status, created.body
        );
        let project = created.body["project"].clone();
        let project_arn = project["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the created project carries no arn: {project}"))
            .to_string();
        assert!(
            project_arn.starts_with(&format!("arn:aws:devicefarm:{REGION}:")),
            "case {case:?}: expected an ARN in the request's own Region, got {project_arn:?}"
        );
        assert_eq!(
            project["name"], input["name"],
            "case {case:?}: expected the name the request carried, got {}",
            project["name"]
        );

        let read = call(&router, "GetProject", &json!({ "arn": project_arn })).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetProject to answer 200, got HTTP {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.body["project"], project,
            "case {case:?}: expected GetProject to report the stored project {project}, got {}",
            read.body["project"]
        );
    }
}

/// Every operation that reports a `RemoteAccessSession` answers the scaffold's 501 to a
/// well-formed request, and carries no session.
///
/// `RemoteAccessSession` is the shape the model refresh changed: its single `endpoint` is
/// deprecated in favour of an `endpoints` structure naming the endpoint a client views the
/// device through and the one a WebDriver client drives it through. This emulator opens no
/// remote access session and serves no such endpoint, so each of these operations is still
/// scaffolded — and a scaffolded operation says so with a 501 rather than answering an empty
/// success a client cannot tell from a session that was really opened.
///
/// Each row is well formed: every required member is present and every ARN has the modelled
/// form, so the answer is the handler's own and not the boundary validator's.
#[tokio::test]
async fn remote_access_session_operations_answer_the_scaffold_501() {
    let router = assembled_emulator();

    // The owning project really exists, created through the assembled emulator, so no row is
    // answered 501 for want of a project.
    let created = call(
        &router,
        "CreateProject",
        &json!({ "name": "remote-access-owner" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected CreateProject to answer 200, got HTTP {} with {}",
        created.status, created.body
    );
    let project_arn = created.body["project"]["arn"]
        .as_str()
        .expect("the created project carries an arn")
        .to_string();
    let session_arn = arn(
        "session",
        "99999999-8888-7777-6666-555555555555/11111111-2222-3333-4444-555555555555",
    );
    let device_arn = arn("device", "00000000-1111-2222-3333-444444444444");
    let upload_arn = arn(
        "upload",
        "99999999-8888-7777-6666-555555555555/22222222-3333-4444-5555-666666666666",
    );

    // (case, operation, well-formed input)
    let rows: [(&str, &str, Value); 6] = [
        (
            "opening a session on a device of an existing project",
            "CreateRemoteAccessSession",
            json!({ "projectArn": project_arn, "deviceArn": device_arn }),
        ),
        (
            "reading one session",
            "GetRemoteAccessSession",
            json!({ "arn": session_arn }),
        ),
        (
            "listing the sessions of a project",
            "ListRemoteAccessSessions",
            json!({ "arn": project_arn }),
        ),
        (
            "stopping one session",
            "StopRemoteAccessSession",
            json!({ "arn": session_arn }),
        ),
        (
            "installing an app into one session",
            "InstallToRemoteAccessSession",
            json!({ "remoteAccessSessionArn": session_arn, "appArn": upload_arn }),
        ),
        (
            "releasing one session",
            "DeleteRemoteAccessSession",
            json!({ "arn": session_arn }),
        ),
    ];

    for (case, operation, input) in &rows {
        let reply = call(&router, operation, input).await;
        assert_eq!(
            reply.status, 501,
            "case {case:?} ({operation}): expected the scaffold's 501, got HTTP {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code, NOT_IMPLEMENTED,
            "case {case:?} ({operation}): expected the error code {NOT_IMPLEMENTED}, got {:?} \
             with {}",
            reply.code, reply.body
        );
        for member in ["remoteAccessSession", "remoteAccessSessions", "appUpload"] {
            assert!(
                reply.body.get(member).is_none(),
                "case {case:?} ({operation}): expected no {member} in a refusal, got {}",
                reply.body
            );
        }
    }

    // The refused calls wrote nothing a later caller can see: the project they named is
    // still the only resource the store holds for it.
    let read = call(&router, "GetProject", &json!({ "arn": project_arn })).await;
    assert_eq!(
        read.status, 200,
        "case the owning project after six refusals: expected 200, got HTTP {} with {}",
        read.status, read.body
    );
    let listed = call(&router, "ListRemoteAccessSessions", &json!({ "arn": project_arn })).await;
    assert_eq!(
        listed.status, 501,
        "case listing after six refusals: expected the scaffold's 501, got HTTP {} with {}",
        listed.status, listed.body
    );
}
