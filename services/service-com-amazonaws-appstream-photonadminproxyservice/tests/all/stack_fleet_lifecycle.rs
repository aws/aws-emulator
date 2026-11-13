// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's stack/fleet lifecycle over the wire.
//!
//! Every case drives the real awsJson1_1 front door — `POST /` with
//! `X-Amz-Target: PhotonAdminProxyService.<Operation>` — through the same
//! [`MultiServiceRouter`] the production router uses, so what these tests exercise is
//! the dispatch → deserialize → handle → serialize walk, not a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a Create persists the resource and reports it back from the stored record, so a
//!   later Describe answers byte-identically;
//! * a second Create of a name that exists answers `ResourceAlreadyExistsException`;
//! * an association requires both endpoints to exist, and blocks deletion of either
//!   until it is removed;
//! * a Describe reports only stored records, refuses an unknown name with
//!   `ResourceNotFoundException`, and pages every stored record exactly once.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulator ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: a shared engine, one mounted AppStream service, no signature checking
/// (the tests assert service behaviour, not SigV4).
///
/// Shared with the other suites in this target (`tests/all/update_stack.rs`), so every
/// AppStream test drives the same front door.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_appstream_photonadminproxyservice::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// Send one operation and return `(status, body)`.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("PhotonAdminProxyService.{operation}"),
            )
            // The credential scope is what routes the request to `appstream` and what
            // names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{}/appstream/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000",
                    self.region
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Send an operation that must succeed, returning its body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} {payload}"
        );
        payload
    }

    /// Fetch one URL this emulator handed out, the way a browser fetches it: a plain
    /// `GET`, nothing signed beyond what the URL itself carries, and no redirect followed.
    ///
    /// Returns `(status, Location header, body)`. Shared with the suites that follow a
    /// handed-out URL (`tests/all/streaming_session_resolution.rs`).
    pub(crate) async fn fetch(&self, url: &str) -> (u16, Option<String>, Vec<u8>) {
        // Whatever host the URL names, the request is dispatched to this router: what is
        // under test is the path and query the emulator signed, not name resolution.
        let target = match url.split_once("://") {
            Some((_, rest)) => match rest.split_once('/') {
                Some((_, path_and_query)) => format!("/{path_and_query}"),
                None => "/".to_string(),
            },
            None => url.to_string(),
        };
        let request = http::Request::builder()
            .method("GET")
            .uri(target)
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        (status, location, response.into_body().to_vec())
    }
}

/// The wire error code of a failure body (`__type`, possibly namespace-qualified).
pub(crate) fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert an operation failed with `status` and the wire error code `code`.
pub(crate) async fn assert_error(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: Value,
    status: u16,
    code: &str,
) {
    let (actual_status, payload) = emulator.call(operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: {operation} must answer HTTP {status}, got {actual_status} {payload}"
    );
    assert_eq!(
        error_code(&payload),
        code,
        "case {case:?}: {operation} must answer {code}, got {payload}"
    );
}

// ---------------------------------------------------------------------------
// CreateStack / DescribeStacks
// ---------------------------------------------------------------------------

/// Stack create requests spanning the class: `Name` is the only required member, and
/// every other row adds a different kind of configuration — a scalar, a list of
/// structures, a map, a nested structure — so the assertions below hold for the whole
/// input shape rather than for one recorded body.
///
/// Member names, their constraints and the enum values come from
/// `smithy_query(query="operation", operation="CreateStack")` and the
/// `*_VALUES` constants in `src/ops/types/shared_types.rs`
/// (`PERMISSION_VALUES`, `ACTION_VALUES`, `STORAGE_CONNECTOR_TYPE_VALUES`,
/// `ACCESS_ENDPOINT_TYPE_VALUES`, `PREFERRED_PROTOCOL_VALUES`).
fn stack_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("name only", json!({ "Name": "minimal" })),
        (
            "description and display name",
            json!({ "Name": "described", "Description": "a demo stack", "DisplayName": "Demo" }),
        ),
        (
            // `Name`'s pattern allows digits, `_`, `.` and `-` after the first
            // character: ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$
            "punctuated name",
            json!({ "Name": "9stack_v1.2-beta", "Description": "" }),
        ),
        (
            "urls",
            json!({
                "Name": "redirecting",
                "RedirectURL": "https://example.invalid/after",
                "FeedbackURL": "https://example.invalid/feedback",
            }),
        ),
        (
            "two storage connectors",
            json!({
                "Name": "storage",
                "StorageConnectors": [
                    { "ConnectorType": "HOMEFOLDERS" },
                    { "ConnectorType": "GOOGLE_DRIVE", "Domains": ["example.invalid"] },
                ],
            }),
        ),
        (
            "user settings and persistent application settings",
            json!({
                "Name": "settings",
                "UserSettings": [
                    { "Action": "FILE_UPLOAD", "Permission": "DISABLED" },
                    { "Action": "CLIPBOARD_COPY_TO_LOCAL_DEVICE", "Permission": "ENABLED" },
                    { "Action": "PRINTING_TO_LOCAL_DEVICE", "Permission": "ENABLED" },
                ],
                "ApplicationSettings": { "Enabled": true, "SettingsGroup": "group-a" },
            }),
        ),
        (
            "access endpoints, embed hosts and streaming experience",
            json!({
                "Name": "endpoints",
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-00000000" }],
                "EmbedHostDomains": ["a.example.invalid", "b.example.invalid"],
                "StreamingExperienceSettings": { "PreferredProtocol": "UDP" },
            }),
        ),
        (
            "tags",
            json!({ "Name": "tagged", "Tags": { "Environment": "Test", "Owner": "" } }),
        ),
    ]
}

#[tokio::test]
async fn create_stack_persists_the_record_a_later_describe_reports() {
    let emulator = Emulator::new();

    for (case, request) in stack_rows() {
        let name = request["Name"].as_str().expect("every row names a stack");

        let created = emulator.ok(case, "CreateStack", request.clone()).await;
        let stack = &created["Stack"];

        assert_eq!(
            stack["Name"].as_str(),
            Some(name),
            "case {case:?}: CreateStack must report the requested Name, got {stack}"
        );
        assert_eq!(
            stack["Arn"].as_str(),
            Some(format!("arn:aws:appstream:us-east-1:{ACCOUNT}:stack/{name}").as_str()),
            "case {case:?}: CreateStack must mint the stack ARN, got {stack}"
        );
        assert!(
            stack["CreatedTime"].is_number(),
            "case {case:?}: CreateStack must report a CreatedTime, got {stack}"
        );
        assert_eq!(
            stack["StackErrors"],
            json!([]),
            "case {case:?}: a freshly created stack has no errors, got {stack}"
        );
        for member in ["Description", "DisplayName", "RedirectURL", "FeedbackURL"] {
            if let Some(expected) = request.get(member) {
                assert_eq!(
                    &stack[member], expected,
                    "case {case:?}: CreateStack must store and report {member}, got {stack}"
                );
            }
        }
        if let Some(expected) = request.get("ApplicationSettings") {
            assert_eq!(
                stack["ApplicationSettings"]["Enabled"], expected["Enabled"],
                "case {case:?}: ApplicationSettings.Enabled must round-trip, got {stack}"
            );
            assert_eq!(
                stack["ApplicationSettings"]["SettingsGroup"], expected["SettingsGroup"],
                "case {case:?}: ApplicationSettings.SettingsGroup must round-trip, got {stack}"
            );
        }
        for member in [
            "StorageConnectors",
            "UserSettings",
            "AccessEndpoints",
            "EmbedHostDomains",
            "StreamingExperienceSettings",
        ] {
            if let Some(expected) = request.get(member) {
                assert_eq!(
                    &stack[member], expected,
                    "case {case:?}: CreateStack must store and report {member}, got {stack}"
                );
            }
        }

        // ROUND TRIP: the reader returns the PERSISTED record, not a re-synthesised one.
        let described = emulator
            .ok(case, "DescribeStacks", json!({ "Names": [name] }))
            .await;
        assert_eq!(
            described["Stacks"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: DescribeStacks(Names=[{name}]) must report exactly that stack, \
             got {described}"
        );
        assert_eq!(
            described["Stacks"][0], *stack,
            "case {case:?}: DescribeStacks must report the stored record unchanged, \
             got {described}"
        );
    }

    // Every row is still stored, and an unfiltered Describe reports all of them.
    let all = emulator.ok("all", "DescribeStacks", json!({})).await;
    let reported: BTreeSet<String> = all["Stacks"]
        .as_array()
        .expect("Stacks is a list")
        .iter()
        .map(|stack| stack["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: BTreeSet<String> = stack_rows()
        .iter()
        .map(|(_, request)| request["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "DescribeStacks with no Names must report every stored stack, got {all}"
    );
}

// ---------------------------------------------------------------------------
// CreateFleet / DescribeFleets
// ---------------------------------------------------------------------------

/// Fleet create requests spanning the class. The rows differ along the members that
/// drive the computed response: the `FleetType` (including omitted, which takes the
/// service default), the requested capacity (absent, zero, one, many) and the optional
/// timeouts/platform members. `FLEET_TYPE_VALUES` / `PLATFORM_TYPE_VALUES` /
/// `STREAM_VIEW_VALUES` in `src/ops/types/shared_types.rs` are the source of the enum
/// values; `expected_fleet_type` is the value the response must carry and
/// `expected_desired` the `ComputeCapacityStatus.Desired` it must compute.
struct FleetRow {
    case: &'static str,
    request: Value,
    expected_fleet_type: &'static str,
    expected_desired: i64,
}

fn fleet_rows() -> Vec<FleetRow> {
    vec![
        FleetRow {
            case: "required members only, FleetType omitted",
            request: json!({ "Name": "bare", "InstanceType": "stream.standard.small" }),
            // CreateStack/CreateFleet doc: the FleetType default is ON_DEMAND.
            expected_fleet_type: "ON_DEMAND",
            expected_desired: 0,
        },
        FleetRow {
            case: "on demand, one instance",
            request: json!({
                "Name": "on-demand-1",
                "InstanceType": "stream.standard.small",
                "FleetType": "ON_DEMAND",
                "ImageName": "base-image",
                "ComputeCapacity": { "DesiredInstances": 1 },
            }),
            expected_fleet_type: "ON_DEMAND",
            expected_desired: 1,
        },
        FleetRow {
            case: "always on, many instances, timeouts and platform",
            request: json!({
                "Name": "always.on_42",
                "InstanceType": "stream.graphics.g4dn.xlarge",
                "FleetType": "ALWAYS_ON",
                "ImageArn": format!("arn:aws:appstream:us-east-1:{ACCOUNT}:image/base"),
                "ComputeCapacity": { "DesiredInstances": 42 },
                "Description": "a big fleet",
                "DisplayName": "Big",
                "MaxUserDurationInSeconds": 57600,
                "DisconnectTimeoutInSeconds": 900,
                "IdleDisconnectTimeoutInSeconds": 600,
                "StreamView": "DESKTOP",
                "Platform": "WINDOWS_SERVER_2022",
                "EnableDefaultInternetAccess": true,
            }),
            expected_fleet_type: "ALWAYS_ON",
            expected_desired: 42,
        },
        FleetRow {
            case: "explicit zero instances",
            request: json!({
                "Name": "scaled-to-zero",
                "InstanceType": "stream.standard.medium",
                "FleetType": "ON_DEMAND",
                "ComputeCapacity": { "DesiredInstances": 0 },
                "VpcConfig": { "SubnetIds": ["subnet-1", "subnet-2"] },
            }),
            expected_fleet_type: "ON_DEMAND",
            expected_desired: 0,
        },
        FleetRow {
            case: "elastic, session based capacity",
            request: json!({
                "Name": "elastic-sessions",
                "InstanceType": "stream.standard.large",
                "FleetType": "ELASTIC",
                "MaxConcurrentSessions": 10,
                "ComputeCapacity": { "DesiredSessions": 10 },
                "Platform": "AMAZON_LINUX2",
            }),
            expected_fleet_type: "ELASTIC",
            expected_desired: 0,
        },
        FleetRow {
            case: "tags and usb filters",
            request: json!({
                "Name": "peripherals",
                "InstanceType": "stream.standard.small",
                "ComputeCapacity": { "DesiredInstances": 3 },
                "UsbDeviceFilterStrings": ["0x0000,0x0000,abc,def,1,2,0,1"],
                "Tags": { "Team": "streaming" },
            }),
            expected_fleet_type: "ON_DEMAND",
            expected_desired: 3,
        },
    ]
}

#[tokio::test]
async fn create_fleet_reports_a_stopped_fleet_with_the_requested_capacity() {
    let emulator = Emulator::new();

    for row in fleet_rows() {
        let case = row.case;
        let name = row.request["Name"].as_str().expect("every row names a fleet");

        let created = emulator.ok(case, "CreateFleet", row.request.clone()).await;
        let fleet = &created["Fleet"];

        assert_eq!(
            fleet["Name"].as_str(),
            Some(name),
            "case {case:?}: CreateFleet must report the requested Name, got {fleet}"
        );
        assert_eq!(
            fleet["Arn"].as_str(),
            Some(format!("arn:aws:appstream:us-east-1:{ACCOUNT}:fleet/{name}").as_str()),
            "case {case:?}: CreateFleet must mint the fleet ARN, got {fleet}"
        );
        assert_eq!(
            fleet["InstanceType"], row.request["InstanceType"],
            "case {case:?}: CreateFleet must store and report InstanceType, got {fleet}"
        );
        assert_eq!(
            fleet["FleetType"].as_str(),
            Some(row.expected_fleet_type),
            "case {case:?}: CreateFleet must report FleetType {}, got {fleet}",
            row.expected_fleet_type
        );
        assert_eq!(
            fleet["State"].as_str(),
            Some("STOPPED"),
            "case {case:?}: a freshly created fleet is STOPPED, got {fleet}"
        );
        assert_eq!(
            fleet["ComputeCapacityStatus"]["Desired"].as_i64(),
            Some(row.expected_desired),
            "case {case:?}: ComputeCapacityStatus.Desired must come from the requested \
             ComputeCapacity, got {fleet}"
        );
        for counter in ["Running", "InUse", "Available"] {
            assert_eq!(
                fleet["ComputeCapacityStatus"][counter].as_i64(),
                Some(0),
                "case {case:?}: a stopped fleet has no {counter} capacity, got {fleet}"
            );
        }
        assert!(
            fleet["CreatedTime"].is_number(),
            "case {case:?}: CreateFleet must report a CreatedTime, got {fleet}"
        );
        assert_eq!(
            fleet["FleetErrors"],
            json!([]),
            "case {case:?}: a freshly created fleet has no errors, got {fleet}"
        );
        for member in [
            "ImageName",
            "ImageArn",
            "Description",
            "DisplayName",
            "MaxUserDurationInSeconds",
            "DisconnectTimeoutInSeconds",
            "IdleDisconnectTimeoutInSeconds",
            "MaxConcurrentSessions",
            "StreamView",
            "Platform",
            "EnableDefaultInternetAccess",
            "VpcConfig",
            "UsbDeviceFilterStrings",
        ] {
            if let Some(expected) = row.request.get(member) {
                assert_eq!(
                    &fleet[member], expected,
                    "case {case:?}: CreateFleet must store and report {member}, got {fleet}"
                );
            }
        }

        // ROUND TRIP through the implemented reader.
        let described = emulator
            .ok(case, "DescribeFleets", json!({ "Names": [name] }))
            .await;
        assert_eq!(
            described["Fleets"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: DescribeFleets(Names=[{name}]) must report exactly that fleet, \
             got {described}"
        );
        assert_eq!(
            described["Fleets"][0], *fleet,
            "case {case:?}: DescribeFleets must report the stored record unchanged, \
             got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// Duplicate names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_create_of_an_existing_name_already_exists() {
    let emulator = Emulator::new();

    for (case, request) in stack_rows() {
        emulator.ok(case, "CreateStack", request.clone()).await;
        assert_error(
            &emulator,
            case,
            "CreateStack",
            request,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
    }

    for row in fleet_rows() {
        emulator.ok(row.case, "CreateFleet", row.request.clone()).await;
        assert_error(
            &emulator,
            row.case,
            "CreateFleet",
            row.request,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// AssociateFleet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn associate_fleet_requires_both_endpoints_to_exist() {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": "live-stack" }))
        .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({
                "Name": "live-fleet",
                "InstanceType": "stream.standard.small",
                "ComputeCapacity": { "DesiredInstances": 1 },
            }),
        )
        .await;

    // Rows: which side of the pair names a stored record.
    let rows: Vec<(&str, &str, &str, Option<u16>)> = vec![
        ("both exist", "live-fleet", "live-stack", None),
        ("fleet missing", "absent-fleet", "live-stack", Some(400)),
        ("stack missing", "live-fleet", "absent-stack", Some(400)),
        ("both missing", "absent-fleet", "absent-stack", Some(400)),
    ];

    for (case, fleet, stack, expected_failure) in rows {
        let request = json!({ "FleetName": fleet, "StackName": stack });
        match expected_failure {
            None => {
                emulator.ok(case, "AssociateFleet", request).await;
            }
            Some(status) => {
                assert_error(
                    &emulator,
                    case,
                    "AssociateFleet",
                    request,
                    status,
                    "ResourceNotFoundException",
                )
                .await;
            }
        }
    }

    // Associating the same pair again is idempotent: one Disassociate clears it.
    emulator
        .ok(
            "repeat",
            "AssociateFleet",
            json!({ "FleetName": "live-fleet", "StackName": "live-stack" }),
        )
        .await;
    emulator
        .ok(
            "repeat",
            "DisassociateFleet",
            json!({ "FleetName": "live-fleet", "StackName": "live-stack" }),
        )
        .await;
    emulator
        .ok("repeat", "DeleteFleet", json!({ "Name": "live-fleet" }))
        .await;
}

// ---------------------------------------------------------------------------
// Delete: in-use refusal, then removal from state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_blocks_both_deletes_until_it_is_removed() {
    // Rows: which end of the association the test tries to delete first. Both must be
    // refused while the edge exists and both must succeed once it is gone.
    for (case, first, second) in [
        ("fleet first", "DeleteFleet", "DeleteStack"),
        ("stack first", "DeleteStack", "DeleteFleet"),
    ] {
        let emulator = Emulator::new();
        emulator
            .ok(case, "CreateStack", json!({ "Name": "demo-stack" }))
            .await;
        emulator
            .ok(
                case,
                "CreateFleet",
                json!({
                    "Name": "demo-fleet",
                    "InstanceType": "stream.standard.small",
                    "ComputeCapacity": { "DesiredInstances": 1 },
                }),
            )
            .await;
        emulator
            .ok(
                case,
                "AssociateFleet",
                json!({ "FleetName": "demo-fleet", "StackName": "demo-stack" }),
            )
            .await;

        let name_of = |operation: &str| {
            if operation == "DeleteFleet" {
                "demo-fleet"
            } else {
                "demo-stack"
            }
        };

        for operation in [first, second] {
            assert_error(
                &emulator,
                case,
                operation,
                json!({ "Name": name_of(operation) }),
                400,
                "ResourceInUseException",
            )
            .await;
        }

        // Both resources are still there: a refused delete changed nothing.
        for (reader, member) in [
            ("DescribeFleets", "Fleets"),
            ("DescribeStacks", "Stacks"),
        ] {
            let listed = emulator.ok(case, reader, json!({})).await;
            assert_eq!(
                listed[member].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: a refused delete must leave the record in place, got {listed}"
            );
        }

        emulator
            .ok(
                case,
                "DisassociateFleet",
                json!({ "FleetName": "demo-fleet", "StackName": "demo-stack" }),
            )
            .await;

        for operation in [first, second] {
            emulator
                .ok(case, operation, json!({ "Name": name_of(operation) }))
                .await;
        }

        // The deletes mutated persisted state: the readers no longer know the names,
        // and a repeat delete is a not-found.
        assert_error(
            &emulator,
            case,
            "DescribeFleets",
            json!({ "Names": ["demo-fleet"] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_error(
            &emulator,
            case,
            "DescribeStacks",
            json!({ "Names": ["demo-stack"] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        for operation in [first, second] {
            assert_error(
                &emulator,
                case,
                operation,
                json!({ "Name": name_of(operation) }),
                400,
                "ResourceNotFoundException",
            )
            .await;
        }
        for (reader, member) in [
            ("DescribeFleets", "Fleets"),
            ("DescribeStacks", "Stacks"),
        ] {
            let listed = emulator.ok(case, reader, json!({})).await;
            assert_eq!(
                listed[member],
                json!([]),
                "case {case:?}: {reader} must report nothing once every record is deleted, \
                 got {listed}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Describe: unknown names, and pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_refuses_a_name_that_was_never_created() {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": "known-stack" }))
        .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({ "Name": "known-fleet", "InstanceType": "stream.standard.small" }),
        )
        .await;

    // Rows: the shape of the `Names` list — only unknown, unknown mixed with known,
    // and a known name alone (which must still succeed).
    let rows: Vec<(&str, Value, Value, Option<u16>)> = vec![
        (
            "single unknown",
            json!({ "Names": ["nope"] }),
            json!({ "Names": ["nope"] }),
            Some(400),
        ),
        (
            "known then unknown",
            json!({ "Names": ["known-stack", "nope"] }),
            json!({ "Names": ["known-fleet", "nope"] }),
            Some(400),
        ),
        (
            "unknown then known",
            json!({ "Names": ["nope", "known-stack"] }),
            json!({ "Names": ["nope", "known-fleet"] }),
            Some(400),
        ),
        (
            "all known",
            json!({ "Names": ["known-stack"] }),
            json!({ "Names": ["known-fleet"] }),
            None,
        ),
    ];

    for (case, stack_request, fleet_request, expected_failure) in rows {
        for (operation, request) in [
            ("DescribeStacks", stack_request),
            ("DescribeFleets", fleet_request),
        ] {
            match expected_failure {
                None => {
                    emulator.ok(case, operation, request).await;
                }
                Some(status) => {
                    assert_error(
                        &emulator,
                        case,
                        operation,
                        request,
                        status,
                        "ResourceNotFoundException",
                    )
                    .await;
                }
            }
        }
    }
}

#[tokio::test]
async fn following_next_token_reports_every_record_exactly_once() {
    let emulator = Emulator::new();

    // More records than one page holds, so at least three pages are walked. The names
    // are deliberately not in insertion order so the paging order cannot be an artifact
    // of the order they were created in.
    let total = 125_u32;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for index in (0..total).rev() {
        let name = format!("page-{index:03}");
        emulator
            .ok("setup", "CreateStack", json!({ "Name": name }))
            .await;
        emulator
            .ok(
                "setup",
                "CreateFleet",
                json!({ "Name": name, "InstanceType": "stream.standard.small" }),
            )
            .await;
        expected.insert(name);
    }

    for (operation, member, key) in [
        ("DescribeStacks", "Stacks", "Name"),
        ("DescribeFleets", "Fleets", "Name"),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_u32;
        loop {
            let request = match &next_token {
                Some(token) => json!({ "NextToken": token }),
                None => json!({}),
            };
            let page = emulator.ok(operation, operation, request).await;
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {operation:?}: paging did not terminate after {pages} pages"
            );
            for record in page[member].as_array().expect("the page carries a list") {
                seen.push(
                    record[key]
                        .as_str()
                        .expect("every record carries a name")
                        .to_string(),
                );
            }
            next_token = page["NextToken"].as_str().map(str::to_string);
            match &next_token {
                Some(token) => assert!(
                    !expected.contains(token),
                    "case {operation:?}: NextToken must be opaque, not a record name, \
                     got {token:?}"
                ),
                None => break,
            }
        }

        assert!(
            pages > 1,
            "case {operation:?}: {total} records must not fit in a single page"
        );
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "case {operation:?}: paging reported a record twice, got {} entries for {} names",
            seen.len(),
            unique.len()
        );
        assert_eq!(
            unique, expected,
            "case {operation:?}: paging must report every stored record exactly once"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {operation:?}: pages must be walked in a stable order"
        );
    }
}

#[tokio::test]
async fn describe_refuses_a_next_token_it_did_not_issue() {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": "known-stack" }))
        .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({ "Name": "known-fleet", "InstanceType": "stream.standard.small" }),
        )
        .await;

    // Rows: tokens a client could hand-craft. The cursor is an opaque encoding this
    // service mints, so none of these is a token it issued.
    for (case, token) in [
        ("a record name in the clear", "known-stack"),
        ("odd length", "abc"),
        ("outside the codec's alphabet", "zzzz"),
        ("decodes to bytes that are not text", "ff"),
    ] {
        for operation in ["DescribeStacks", "DescribeFleets"] {
            assert_error(
                &emulator,
                case,
                operation,
                json!({ "NextToken": token }),
                400,
                "ValidationException",
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Store ceilings
// ---------------------------------------------------------------------------

/// The emulator's memory ceiling, read from the crate so the test tracks the constant
/// rather than a copy of its value.
const CEILING: usize =
    service_com_amazonaws_appstream_photonadminproxyservice::state::MAX_RECORDS_PER_STORE;

#[tokio::test]
async fn each_store_refuses_a_record_past_its_ceiling() {
    // Row 1 — stacks: fill the store to the ceiling, then one more name is refused.
    {
        let emulator = Emulator::new();
        for index in 0..CEILING {
            emulator
                .ok(
                    "stacks",
                    "CreateStack",
                    json!({ "Name": format!("s{index:05}") }),
                )
                .await;
        }
        assert_error(
            &emulator,
            "stacks",
            "CreateStack",
            json!({ "Name": "one-too-many" }),
            400,
            "LimitExceededException",
        )
        .await;
        // A name already held is still an already-exists, not a limit failure: the
        // duplicate check runs first and inserts nothing.
        assert_error(
            &emulator,
            "stacks",
            "CreateStack",
            json!({ "Name": "s00000" }),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        // Deleting a record makes room again, so the ceiling is a bound, not a latch.
        emulator
            .ok("stacks", "DeleteStack", json!({ "Name": "s00000" }))
            .await;
        emulator
            .ok("stacks", "CreateStack", json!({ "Name": "one-too-many" }))
            .await;
    }

    // Row 2 — fleets: the same rule on the other create.
    {
        let emulator = Emulator::new();
        for index in 0..CEILING {
            emulator
                .ok(
                    "fleets",
                    "CreateFleet",
                    json!({
                        "Name": format!("f{index:05}"),
                        "InstanceType": "stream.standard.small",
                    }),
                )
                .await;
        }
        assert_error(
            &emulator,
            "fleets",
            "CreateFleet",
            json!({ "Name": "one-too-many", "InstanceType": "stream.standard.small" }),
            400,
            "LimitExceededException",
        )
        .await;
        emulator
            .ok("fleets", "DeleteFleet", json!({ "Name": "f00000" }))
            .await;
        emulator
            .ok(
                "fleets",
                "CreateFleet",
                json!({ "Name": "one-too-many", "InstanceType": "stream.standard.small" }),
            )
            .await;
    }

    // Row 3 — associations: the edge store has the same ceiling, and a repeat of an edge
    // it already holds inserts nothing, so it is never refused.
    {
        let emulator = Emulator::new();
        // A grid wide enough that the pairs outnumber the ceiling.
        let side = CEILING.isqrt() + 2;
        for index in 0..side {
            emulator
                .ok(
                    "associations",
                    "CreateStack",
                    json!({ "Name": format!("gs{index:05}") }),
                )
                .await;
            emulator
                .ok(
                    "associations",
                    "CreateFleet",
                    json!({
                        "Name": format!("gf{index:05}"),
                        "InstanceType": "stream.standard.small",
                    }),
                )
                .await;
        }

        let mut inserted = 0_usize;
        let mut refused: Option<(String, String)> = None;
        for fleet in 0..side {
            for stack in 0..side {
                let pair = (format!("gf{fleet:05}"), format!("gs{stack:05}"));
                let request = json!({ "FleetName": pair.0, "StackName": pair.1 });
                if inserted < CEILING {
                    emulator.ok("associations", "AssociateFleet", request).await;
                    inserted += 1;
                } else {
                    refused = Some(pair);
                    assert_error(
                        &emulator,
                        "associations",
                        "AssociateFleet",
                        request,
                        400,
                        "LimitExceededException",
                    )
                    .await;
                    break;
                }
            }
            if refused.is_some() {
                break;
            }
        }
        assert!(
            refused.is_some(),
            "case \"associations\": the grid must offer more pairs than the \
             ceiling of {CEILING}, but only {inserted} were inserted"
        );

        // A repeat of an edge the store already holds is still accepted at the ceiling.
        emulator
            .ok(
                "associations",
                "AssociateFleet",
                json!({ "FleetName": "gf00000", "StackName": "gs00000" }),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// ARNs carry the request's region
// ---------------------------------------------------------------------------

#[tokio::test]
async fn minted_arns_carry_the_requests_region() {
    // Rows: the region named by the request's SigV4 credential scope. Each is a real
    // AWS region token, and the ARN a create mints must carry whichever one signed it.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let stack = emulator
            .ok(region, "CreateStack", json!({ "Name": "regional" }))
            .await;
        assert_eq!(
            stack["Stack"]["Arn"].as_str(),
            Some(format!("arn:aws:appstream:{region}:{ACCOUNT}:stack/regional").as_str()),
            "case {region:?}: the stack ARN must carry the signing region, got {stack}"
        );

        let fleet = emulator
            .ok(
                region,
                "CreateFleet",
                json!({ "Name": "regional", "InstanceType": "stream.standard.small" }),
            )
            .await;
        assert_eq!(
            fleet["Fleet"]["Arn"].as_str(),
            Some(format!("arn:aws:appstream:{region}:{ACCOUNT}:fleet/regional").as_str()),
            "case {region:?}: the fleet ARN must carry the signing region, got {fleet}"
        );

        // The ARN is stable across reads: a Describe reports the one the create minted.
        let described = emulator
            .ok(region, "DescribeStacks", json!({ "Names": ["regional"] }))
            .await;
        assert_eq!(
            described["Stacks"][0]["Arn"], stack["Stack"]["Arn"],
            "case {region:?}: the stored ARN must not change between reads, got {described}"
        );
    }
}
