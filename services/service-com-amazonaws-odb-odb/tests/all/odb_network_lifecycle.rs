// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ODB network lifecycle: CreateOdbNetwork, GetOdbNetwork,
//! ListOdbNetworks, UpdateOdbNetwork and DeleteOdbNetwork.
//!
//! The last three cases cover the rest of this service's modelled surface rather than
//! the ODB network: every operation the service's model binds is dispatched to its own
//! handler and its input is checked against the model's constraints, and an operation
//! nobody has implemented answers the scaffold 501 rather than an unknown-operation
//! refusal.
//!
//! Every case drives the real wire path — an `awsJson1_0` `POST /` with
//! `X-Amz-Target: Odb.<Operation>`, dispatched through the same [`MultiServiceRouter`]
//! the emulator binary mounts — so routing, deserialization, modelled-constraint
//! validation, the handler and serialization are all exercised, and what a write stored
//! is read back through a SEPARATE HTTP call. Nothing reaches into the store directly,
//! so a response that is not computed from persisted state fails here.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::Arc;

use ipnet::Ipv4Net;
use serde_json::{json, Value};
use service_com_amazonaws_odb_odb::state::{
    AWS_REGION_PATTERN, MAX_CROSS_REGION_RESTORE_SOURCES_PER_ODB_NETWORK,
    MAX_PEERED_CIDRS_PER_ODB_NETWORK, OCI_RESERVED_CIDRS, RESOURCE_DISPLAY_NAME_PATTERN,
    STATUS_AVAILABLE, STATUS_PROVISIONING, TRANSITION_SPAN,
};
use service_com_amazonaws_odb_odb::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every ARN below is expected to carry: the one the `Authorization` header's
/// credential scope names.
const REGION: &str = "us-west-2";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator, one shared store — exactly what the production router builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these cases; the routing key still comes from
    // the `Authorization` header below.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://odb.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("Odb.{operation}"))
        // The router keys on the SigV4 credential scope's service field (`odb`, this
        // service's signing name) and the region its ARNs carry.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/odb/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(bytes)
            )
        })
    };
    (status, parsed)
}

/// Call one operation, requiring HTTP 200.
async fn call_ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to succeed with HTTP 200, got {status} and {parsed}"
    );
    parsed
}

/// The awsJson error code an error body carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call was refused with `expected_code` and `expected_status`.
fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
}

/// The member at a dotted path, or `Value::Null` when any step is absent.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for step in path.split('.') {
        match current.get(step) {
            Some(next) => current = next,
            None => return &Value::Null,
        }
    }
    current
}

/// A required string member.
fn string_at(value: &Value, path: &str, case: &str) -> String {
    at(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {path} is absent or not a string in {value}"))
        .to_string()
}

/// Create an ODB network and return the `odbNetworkId` it minted.
async fn create(router: &MultiServiceRouter, request: &Value, case: &str) -> String {
    let body = call_ok(router, "CreateOdbNetwork", request, case).await;
    string_at(&body, "odbNetworkId", case)
}

/// Read an ODB network back and return the `odbNetwork` structure.
async fn get_network(router: &MultiServiceRouter, id_or_arn: &str, case: &str) -> Value {
    let body = call_ok(
        router,
        "GetOdbNetwork",
        &json!({ "odbNetworkId": id_or_arn }),
        case,
    )
    .await;
    at(&body, "odbNetwork").clone()
}

/// A create request carrying only the members every ODB network needs.
fn minimal_request(display_name: &str, client_subnet_cidr: &str) -> Value {
    json!({
        "displayName": display_name,
        "clientSubnetCidr": client_subnet_cidr,
        "availabilityZone": "us-west-2a",
    })
}

/// The members `CreateOdbNetwork` persists verbatim and `GetOdbNetwork` reports back
/// from the stored record. A member the request leaves out must be absent from the
/// read, not invented.
const ROUND_TRIP_MEMBERS: &[&str] = &[
    "displayName",
    "availabilityZone",
    "availabilityZoneId",
    "clientSubnetCidr",
    "backupSubnetCidr",
    "customDomainName",
    "defaultDnsPrefix",
];

/// The `managedServices` sub-member each `*Access` request member determines, and the
/// policy-document member carried alongside it.
const ACCESS_MEMBERS: &[(&str, &str, Option<&str>)] = &[
    ("s3Access", "managedServices.s3Access.status", Some("s3PolicyDocument")),
    ("stsAccess", "managedServices.stsAccess.status", Some("stsPolicyDocument")),
    ("kmsAccess", "managedServices.kmsAccess.status", Some("kmsPolicyDocument")),
    ("zeroEtlAccess", "managedServices.zeroEtlAccess.status", None),
];

/// A timestamp member, asserted to be a real clock reading. Epoch seconds is what the
/// awsJson default rendering puts on the wire; an RFC-3339 string is accepted too, since
/// what is asserted here is that the value is a creation instant and not a placeholder.
fn assert_real_timestamp(network: &Value, member: &str, case: &str) {
    let value = at(network, member);
    match value {
        Value::Number(seconds) => {
            let seconds = seconds.as_f64().expect("a JSON number is an f64");
            // 2023-11-14T22:13:20Z — any emulator run is later than this.
            assert!(
                seconds > 1_700_000_000.0,
                "case {case:?}: {member} must be a real clock reading, got {seconds}"
            );
        }
        Value::String(rendered) => {
            assert!(
                rendered.starts_with("20"),
                "case {case:?}: {member} must be a real clock reading, got {rendered}"
            );
        }
        other => panic!("case {case:?}: {member} is absent or not a timestamp: {other}"),
    }
}

/// Every create case: the request, and the network read back through `GetOdbNetwork`,
/// must agree member for member.
///
/// The rows differ along what the rule generalises over: which of the two mutually
/// exclusive availability-zone members is given, how many optional members the request
/// carries, the CIDR prefix length, the access flags, and the number of cross-Region
/// restore sources and tags.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal, availabilityZone",
            minimal_request("my_odb_net", "10.0.1.0/24"),
        ),
        (
            "minimal, availabilityZoneId instead of availabilityZone",
            json!({
                "displayName": "AzIdOnly",
                "clientSubnetCidr": "10.1.0.0/16",
                "availabilityZoneId": "usw2-az3",
            }),
        ),
        (
            "every optional member, all four access flags enabled",
            json!({
                "displayName": "_full-network",
                "clientSubnetCidr": "10.2.0.0/22",
                "backupSubnetCidr": "10.3.0.0/22",
                "availabilityZone": "us-west-2b",
                "availabilityZoneId": "usw2-az1",
                "customDomainName": "example.internal",
                "defaultDnsPrefix": "odbdns",
                "s3Access": "ENABLED",
                "stsAccess": "ENABLED",
                "kmsAccess": "ENABLED",
                "zeroEtlAccess": "ENABLED",
                "s3PolicyDocument": "{\"Version\":\"2012-10-17\"}",
                "stsPolicyDocument": "{\"Version\":\"2012-10-18\"}",
                "kmsPolicyDocument": "{\"Version\":\"2012-10-19\"}",
                "crossRegionS3RestoreSourcesToEnable": ["us-east-1", "eu-west-1"],
                "tags": { "team": "dba", "env": "test" },
            }),
        ),
        (
            "one access flag enabled, the rest explicitly disabled, smallest subnet",
            json!({
                "displayName": "mixed_access",
                "clientSubnetCidr": "10.4.0.0/28",
                "availabilityZone": "us-west-2c",
                "s3Access": "ENABLED",
                "stsAccess": "DISABLED",
                "kmsAccess": "DISABLED",
                "zeroEtlAccess": "DISABLED",
                "s3PolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        (
            "client token carried, one cross-Region restore source",
            json!({
                "displayName": "tokened-net",
                "clientSubnetCidr": "172.16.8.0/21",
                "availabilityZone": "us-west-2d",
                "clientToken": "create-token-1",
                "crossRegionS3RestoreSourcesToEnable": ["ap-south-1"],
            }),
        ),
    ]
}

#[tokio::test]
async fn create_persists_the_request_and_get_reports_it_from_the_record() {
    let router = emulator();
    let mut ids = BTreeSet::new();

    for (case, request) in create_cases() {
        let id = create(&router, &request, case).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: CreateOdbNetwork reused the id {id}"
        );

        let network = get_network(&router, &id, case).await;

        assert_eq!(
            string_at(&network, "odbNetworkId", case),
            id,
            "case {case:?}: the read must report the id the create minted, got {network}"
        );
        assert_eq!(
            string_at(&network, "odbNetworkArn", case),
            format!("arn:aws:odb:{REGION}:{ACCOUNT_ID}:odb-network/{id}"),
            "case {case:?}: unexpected odbNetworkArn in {network}"
        );

        for member in ROUND_TRIP_MEMBERS {
            match request.get(member) {
                Some(expected) => assert_eq!(
                    at(&network, member),
                    expected,
                    "case {case:?}: member {member} must be reported as stored, got {network}"
                ),
                None => assert_eq!(
                    at(&network, member),
                    &Value::Null,
                    "case {case:?}: member {member} was not in the request, so the read must \
                     not invent one: {network}"
                ),
            }
        }

        for (request_member, status_path, document_member) in ACCESS_MEMBERS {
            let expected = request
                .get(request_member)
                .and_then(Value::as_str)
                // AWS leaves an access configuration the request does not name disabled.
                .unwrap_or("DISABLED");
            assert_eq!(
                at(&network, status_path).as_str().unwrap_or("<absent>"),
                expected,
                "case {case:?}: {status_path} must follow the stored {request_member}: {network}"
            );
            if let Some(document_member) = document_member {
                let path = format!(
                    "{}.{document_member}",
                    status_path.trim_end_matches(".status")
                );
                assert_eq!(
                    at(&network, &path),
                    request.get(document_member).unwrap_or(&Value::Null),
                    "case {case:?}: {path} must report the stored {document_member}: {network}"
                );
            }
        }

        let cross_region = request
            .get("crossRegionS3RestoreSourcesToEnable")
            .and_then(Value::as_array)
            .map(|regions| {
                regions
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let reported: Vec<String> = at(&network, "managedServices.crossRegionS3RestoreSourcesAccess")
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("region"))
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, cross_region,
            "case {case:?}: the cross-Region restore sources must be the stored ones: {network}"
        );

        assert_eq!(
            at(&network, "peeredCidrs"),
            &json!([]),
            "case {case:?}: a new ODB network is peered with nothing: {network}"
        );
        assert_real_timestamp(&network, "createdAt", case);

        let status = string_at(&network, "status", case);
        assert!(
            status == STATUS_PROVISIONING || status == STATUS_AVAILABLE,
            "case {case:?}: a created network is provisioning or available, got {status}"
        );
        let expected_progress = if status == STATUS_AVAILABLE { 100.0 } else { 0.0 };
        assert_eq!(
            at(&network, "percentProgress").as_f64(),
            Some(expected_progress),
            "case {case:?}: percentProgress must follow the status {status}: {network}"
        );
    }
}

#[tokio::test]
async fn reading_a_network_twice_reports_the_same_record() {
    let router = emulator();
    let case = "read twice";
    let id = create(&router, &minimal_request("stable_net", "10.5.0.0/24"), case).await;

    let first = get_network(&router, &id, case).await;
    let second = get_network(&router, &id, case).await;

    assert_eq!(
        at(&first, "createdAt"),
        at(&second, "createdAt"),
        "a read must not change what a later caller sees: {first} then {second}"
    );
    assert_eq!(
        at(&first, "odbNetworkArn"),
        at(&second, "odbNetworkArn"),
        "a read must not change what a later caller sees: {first} then {second}"
    );
}

#[tokio::test]
async fn provisioning_settles_to_available_after_the_transition_span() {
    let router = emulator();
    let case = "provisioning transition";
    let created = call_ok(
        &router,
        "CreateOdbNetwork",
        &minimal_request("settling_net", "10.6.0.0/24"),
        case,
    )
    .await;
    assert_eq!(
        at(&created, "status").as_str(),
        Some(STATUS_PROVISIONING),
        "case {case:?}: a create answers with the transient status it stored: {created}"
    );

    let id = string_at(&created, "odbNetworkId", case);
    let immediately = get_network(&router, &id, case).await;
    assert_eq!(
        at(&immediately, "status").as_str(),
        Some(STATUS_PROVISIONING),
        "case {case:?}: a read inside the transition span still sees PROVISIONING: {immediately}"
    );

    tokio::time::sleep(TRANSITION_SPAN + std::time::Duration::from_millis(150)).await;

    let settled = get_network(&router, &id, case).await;
    assert_eq!(
        at(&settled, "status").as_str(),
        Some(STATUS_AVAILABLE),
        "case {case:?}: past the transition span the clock owes AVAILABLE: {settled}"
    );
    assert_eq!(
        at(&settled, "percentProgress").as_f64(),
        Some(100.0),
        "case {case:?}: an available network has finished provisioning: {settled}"
    );
}

#[tokio::test]
async fn create_with_a_repeated_client_token_returns_the_same_network() {
    let router = emulator();
    let case = "idempotent create";
    let request = json!({
        "displayName": "idempotent_net",
        "clientSubnetCidr": "10.7.0.0/24",
        "availabilityZone": "us-west-2a",
        "clientToken": "tok-12345678",
    });

    let first = create(&router, &request, case).await;
    let second = create(&router, &request, case).await;
    assert_eq!(
        first, second,
        "case {case:?}: the same clientToken must name the same ODB network"
    );

    let listed = list_all_ids(&router, None, case).await;
    assert_eq!(
        listed,
        BTreeSet::from([first.clone()]),
        "case {case:?}: an idempotent replay must not create a second network"
    );

    // A DIFFERENT token is a different request, even with identical members.
    let mut other = request.clone();
    other["clientToken"] = json!("tok-87654321");
    let third = create(&router, &other, case).await;
    assert_ne!(
        third, first,
        "case {case:?}: a fresh clientToken must mint a fresh ODB network"
    );
}

/// Walk `ListOdbNetworks` to exhaustion and return every id it reported, asserting the
/// `@paginated` contract: a page that is not the last carries `nextToken` and is full.
async fn list_all_ids(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    case: &str,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let body = call_ok(router, "ListOdbNetworks", &request, case).await;
        let page = at(&body, "odbNetworks")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: odbNetworks is required: {body}"))
            .clone();
        for summary in &page {
            let id = string_at(summary, "odbNetworkId", case);
            assert!(
                ids.insert(id.clone()),
                "case {case:?}: ListOdbNetworks reported {id} twice"
            );
        }
        next_token = at(&body, "nextToken").as_str().map(ToString::to_string);
        if let (Some(max_results), Some(_)) = (max_results, next_token.as_deref()) {
            assert_eq!(
                page.len(),
                max_results as usize,
                "case {case:?}: a page carrying nextToken must be full: {body}"
            );
        }
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: ListOdbNetworks did not terminate after {pages} pages"
        );
        if next_token.is_none() {
            return ids;
        }
    }
}

#[tokio::test]
async fn list_pages_over_every_stored_network() {
    let router = emulator();
    let case = "pagination";
    let mut expected = BTreeSet::new();
    for index in 0..5 {
        let id = create(
            &router,
            &minimal_request(
                &format!("paged_net_{index}"),
                &format!("10.{}.0.0/24", 10 + index),
            ),
            case,
        )
        .await;
        expected.insert(id);
    }

    // The page size the rule generalises over: below, at and above the number of
    // stored networks, plus an absent `maxResults`.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(9), None] {
        let listed = list_all_ids(&router, max_results, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?} with maxResults={max_results:?}: every stored network must be \
             reported exactly once across the pages"
        );
    }

    // A summary carries the same record a Get does.
    let body = call_ok(&router, "ListOdbNetworks", &json!({}), case).await;
    let summaries = at(&body, "odbNetworks").as_array().cloned().unwrap_or_default();
    for summary in summaries {
        let id = string_at(&summary, "odbNetworkId", case);
        let network = get_network(&router, &id, case).await;
        for member in ["displayName", "clientSubnetCidr", "odbNetworkArn", "status"] {
            assert_eq!(
                at(&summary, member),
                at(&network, member),
                "case {case:?}: the summary of {id} must report the same {member} as the Get"
            );
        }
    }
}

/// One row of the update table: the case name, the `UpdateOdbNetwork` members to send,
/// and the dotted paths of the read-back network they must change (or leave alone).
type UpdateRow = (&'static str, Value, Vec<(&'static str, Value)>);

#[tokio::test]
async fn update_mutates_the_stored_record() {
    let router = emulator();
    let case = "update";
    let id = create(
        &router,
        &json!({
            "displayName": "my_odb_net",
            "clientSubnetCidr": "10.20.0.0/24",
            "availabilityZone": "us-west-2a",
            "s3Access": "ENABLED",
            "crossRegionS3RestoreSourcesToEnable": ["us-east-1"],
        }),
        case,
    )
    .await;

    // Each row is applied in turn to the same network and then read back; the rows
    // differ along what is being changed (a scalar, a list addition, a list removal, a
    // flag, a policy document) and how many entries each list operation names.
    let rows: Vec<UpdateRow> = vec![
        (
            "rename",
            json!({ "displayName": "my_odb_net2" }),
            vec![("displayName", json!("my_odb_net2"))],
        ),
        (
            "add two peered CIDRs",
            json!({ "peeredCidrsToBeAdded": ["10.9.0.0/16", "192.168.4.0/24"] }),
            vec![("peeredCidrs", json!(["10.9.0.0/16", "192.168.4.0/24"]))],
        ),
        (
            "adding a peered CIDR again is not a duplicate",
            json!({ "peeredCidrsToBeAdded": ["10.9.0.0/16"] }),
            vec![("peeredCidrs", json!(["10.9.0.0/16", "192.168.4.0/24"]))],
        ),
        (
            "remove one peered CIDR",
            json!({ "peeredCidrsToBeRemoved": ["10.9.0.0/16"] }),
            vec![("peeredCidrs", json!(["192.168.4.0/24"]))],
        ),
        (
            "disable an access flag and replace a policy document",
            json!({
                "s3Access": "DISABLED",
                "s3PolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
            vec![
                ("managedServices.s3Access.status", json!("DISABLED")),
                (
                    "managedServices.s3Access.s3PolicyDocument",
                    json!("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
                ),
            ],
        ),
        (
            "enable two more access flags at once",
            json!({ "stsAccess": "ENABLED", "kmsAccess": "ENABLED" }),
            vec![
                ("managedServices.stsAccess.status", json!("ENABLED")),
                ("managedServices.kmsAccess.status", json!("ENABLED")),
                // Untouched by this row, so still what the previous row stored.
                ("managedServices.s3Access.status", json!("DISABLED")),
            ],
        ),
        (
            "disable a cross-Region restore source and enable another",
            json!({
                "crossRegionS3RestoreSourcesToDisable": ["us-east-1"],
                "crossRegionS3RestoreSourcesToEnable": ["eu-central-1"],
            }),
            vec![(
                "managedServices.crossRegionS3RestoreSourcesAccess",
                json!([{ "region": "eu-central-1", "status": "ENABLED" }]),
            )],
        ),
    ];

    for (row, mut update, expectations) in rows {
        update["odbNetworkId"] = json!(id);
        let answer = call_ok(&router, "UpdateOdbNetwork", &update, row).await;
        assert_eq!(
            string_at(&answer, "odbNetworkId", row),
            id,
            "case {row:?}: the update must answer for the network it changed: {answer}"
        );

        let network = get_network(&router, &id, row).await;
        for (path, expected) in expectations {
            assert_eq!(
                at(&network, path),
                &expected,
                "case {row:?}: expected {path} to be {expected}, got {network}"
            );
        }
    }
}

#[tokio::test]
async fn delete_removes_the_network_from_get_and_list() {
    let router = emulator();
    let case = "delete";
    let kept = create(&router, &minimal_request("kept_net", "10.30.0.0/24"), case).await;
    let doomed = create(&router, &minimal_request("doomed_net", "10.31.0.0/24"), case).await;

    call_ok(
        &router,
        "DeleteOdbNetwork",
        &json!({ "odbNetworkId": doomed, "deleteAssociatedResources": true }),
        case,
    )
    .await;

    assert_refused(
        call(
            &router,
            "GetOdbNetwork",
            &json!({ "odbNetworkId": doomed }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        case,
    );
    assert_eq!(
        list_all_ids(&router, None, case).await,
        BTreeSet::from([kept.clone()]),
        "case {case:?}: a deleted network must be absent from ListOdbNetworks"
    );
    // Deleting it again is a 404: the record is gone, not merely marked.
    assert_refused(
        call(
            &router,
            "DeleteOdbNetwork",
            &json!({ "odbNetworkId": doomed, "deleteAssociatedResources": false }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        case,
    );
}

#[tokio::test]
async fn an_arn_addresses_the_same_network_as_the_bare_id() {
    let router = emulator();
    let case = "ARN addressing";
    let id = create(&router, &minimal_request("arn_net", "10.40.0.0/24"), case).await;
    let arn = format!("arn:aws:odb:{REGION}:{ACCOUNT_ID}:odb-network/{id}");

    let by_arn = get_network(&router, &arn, case).await;
    assert_eq!(
        string_at(&by_arn, "odbNetworkId", case),
        id,
        "case {case:?}: the ARN must address the network the id does: {by_arn}"
    );

    call_ok(
        &router,
        "UpdateOdbNetwork",
        &json!({ "odbNetworkId": arn, "displayName": "arn_net_renamed" }),
        case,
    )
    .await;
    let network = get_network(&router, &id, case).await;
    assert_eq!(
        at(&network, "displayName"),
        &json!("arn_net_renamed"),
        "case {case:?}: an ARN-addressed update must change the same record: {network}"
    );

    call_ok(
        &router,
        "DeleteOdbNetwork",
        &json!({ "odbNetworkId": arn, "deleteAssociatedResources": false }),
        case,
    )
    .await;
    assert!(
        list_all_ids(&router, None, case).await.is_empty(),
        "case {case:?}: an ARN-addressed delete must remove the record"
    );
}

#[tokio::test]
async fn a_subnet_in_an_oci_reserved_range_is_refused() {
    let router = emulator();

    // Both published bounds of every reserved range, and the range itself. The catalog
    // is `OCI_RESERVED_CIDRS`, whose source is the `clientSubnetCidr` /
    // `backupSubnetCidr` member docs in this service's Smithy model.
    for reserved in OCI_RESERVED_CIDRS {
        let range: Ipv4Net = reserved
            .parse()
            .unwrap_or_else(|e| panic!("{reserved} is a CIDR range ({e})"));
        let lowest = format!("{}/24", range.network());
        let highest = format!(
            "{}/24",
            Ipv4Addr::from(u32::from(range.broadcast()) & !0xff)
        );
        for candidate in [(*reserved).to_string(), lowest, highest] {
            let case = format!("clientSubnetCidr {candidate} inside reserved {reserved}");
            assert_refused(
                call(
                    &router,
                    "CreateOdbNetwork",
                    &minimal_request("reserved_probe", &candidate),
                )
                .await,
                400,
                "ValidationException",
                &case,
            );
            let case = format!("backupSubnetCidr {candidate} inside reserved {reserved}");
            let mut request = minimal_request("reserved_probe", "10.50.0.0/24");
            request["backupSubnetCidr"] = json!(candidate);
            assert_refused(
                call(&router, "CreateOdbNetwork", &request).await,
                400,
                "ValidationException",
                &case,
            );
        }
    }

    // One step past a published bound, which is NOT reserved and must be accepted. The
    // upper neighbour of 100.106.0.0/16 is 100.107.0.0/16 and that of 224.0.0.0/4 is
    // 240.0.0.0/4 — both reserved themselves, so those two bounds appear here only from
    // below.
    let accepted = [
        "100.105.255.0/24", // just below 100.106.0.0/16
        "100.108.0.0/24",   // just above 100.107.0.0/16
        "169.253.255.0/24", // just below 169.254.0.0/16
        "169.255.0.0/24",   // just above 169.254.0.0/16
        "223.255.255.0/24", // just below 224.0.0.0/4
        "10.0.1.0/24",      // an ordinary private range
    ];
    for (index, candidate) in accepted.into_iter().enumerate() {
        let case = format!("clientSubnetCidr {candidate} is outside every reserved range");
        let id = create(
            &router,
            &minimal_request(&format!("accepted_{index}"), candidate),
            &case,
        )
        .await;
        let network = get_network(&router, &id, &case).await;
        assert_eq!(
            at(&network, "clientSubnetCidr"),
            &json!(candidate),
            "case {case:?}: the accepted CIDR must be the one stored: {network}"
        );
    }
}

#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    let router = emulator();
    let unknown = "abcdefabcdef";

    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "displayName starting with a hyphen",
            "CreateOdbNetwork",
            minimal_request("-bad--name", "10.60.0.0/24"),
            400,
            "ValidationException",
        ),
        (
            "displayName with two consecutive hyphens",
            "CreateOdbNetwork",
            minimal_request("bad--name", "10.60.0.0/24"),
            400,
            "ValidationException",
        ),
        (
            "displayName starting with a digit",
            "CreateOdbNetwork",
            minimal_request("9net", "10.60.0.0/24"),
            400,
            "ValidationException",
        ),
        (
            "displayName with a character outside the pattern",
            "CreateOdbNetwork",
            minimal_request("net.one", "10.60.0.0/24"),
            400,
            "ValidationException",
        ),
        (
            "displayName empty, below the modelled minimum length",
            "CreateOdbNetwork",
            minimal_request("", "10.60.0.0/24"),
            400,
            "ValidationException",
        ),
        (
            "displayName missing altogether",
            "CreateOdbNetwork",
            json!({ "clientSubnetCidr": "10.60.0.0/24", "availabilityZone": "us-west-2a" }),
            400,
            "ValidationException",
        ),
        (
            "clientSubnetCidr missing altogether",
            "CreateOdbNetwork",
            json!({ "displayName": "no_subnet", "availabilityZone": "us-west-2a" }),
            400,
            "ValidationException",
        ),
        (
            "neither availabilityZone nor availabilityZoneId",
            "CreateOdbNetwork",
            json!({ "displayName": "no_az", "clientSubnetCidr": "10.60.0.0/24" }),
            400,
            "ValidationException",
        ),
        (
            "clientSubnetCidr is not a CIDR range",
            "CreateOdbNetwork",
            minimal_request("bad_cidr", "10.60.0.0"),
            400,
            "ValidationException",
        ),
        (
            "clientSubnetCidr is an IPv6 range",
            "CreateOdbNetwork",
            minimal_request("v6_cidr", "2001:db8::/32"),
            400,
            "ValidationException",
        ),
        (
            "backupSubnetCidr overlapping the client subnet",
            "CreateOdbNetwork",
            json!({
                "displayName": "overlapping",
                "clientSubnetCidr": "10.61.0.0/16",
                "backupSubnetCidr": "10.61.4.0/24",
                "availabilityZone": "us-west-2a",
            }),
            400,
            "ValidationException",
        ),
        (
            "an access flag outside the Access enum",
            "CreateOdbNetwork",
            json!({
                "displayName": "bad_access",
                "clientSubnetCidr": "10.62.0.0/24",
                "availabilityZone": "us-west-2a",
                "s3Access": "enabled",
            }),
            400,
            "ValidationException",
        ),
        (
            "GetOdbNetwork on an id no record holds",
            "GetOdbNetwork",
            json!({ "odbNetworkId": unknown }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateOdbNetwork on an id no record holds",
            "UpdateOdbNetwork",
            json!({ "odbNetworkId": unknown, "displayName": "ghost" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteOdbNetwork on an id no record holds",
            "DeleteOdbNetwork",
            json!({ "odbNetworkId": unknown, "deleteAssociatedResources": true }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ListOdbNetworks with a token this service never issued",
            "ListOdbNetworks",
            json!({ "nextToken": "not-a-token" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, operation, request, status, code) in cases {
        assert_refused(
            call(&router, operation, &request).await,
            status,
            code,
            case,
        );
    }

    // None of the refusals above stored anything.
    assert!(
        list_all_ids(&router, None, "refusals store nothing")
            .await
            .is_empty(),
        "a refused request must not leave a record behind"
    );
}

#[tokio::test]
async fn update_refuses_a_peered_cidr_that_overlaps_the_networks_own_subnets() {
    let router = emulator();
    let case = "peered CIDR overlap";
    let id = create(
        &router,
        &json!({
            "displayName": "peering_net",
            "clientSubnetCidr": "10.70.0.0/16",
            "backupSubnetCidr": "10.71.0.0/16",
            "availabilityZone": "us-west-2a",
        }),
        case,
    )
    .await;

    // A peered CIDR is a connected VPC's range, which the model documents must not
    // overlap either subnet of the ODB network.
    for candidate in ["10.70.8.0/24", "10.71.0.0/16", "10.0.0.0/8", "nonsense"] {
        let case = format!("peeredCidrsToBeAdded {candidate}");
        assert_refused(
            call(
                &router,
                "UpdateOdbNetwork",
                &json!({ "odbNetworkId": id, "peeredCidrsToBeAdded": [candidate] }),
            )
            .await,
            400,
            "ValidationException",
            &case,
        );
        let network = get_network(&router, &id, &case).await;
        assert_eq!(
            at(&network, "peeredCidrs"),
            &json!([]),
            "case {case:?}: a refused update must store nothing: {network}"
        );
    }

    // And a range that overlaps neither subnet is accepted.
    call_ok(
        &router,
        "UpdateOdbNetwork",
        &json!({ "odbNetworkId": id, "peeredCidrsToBeAdded": ["10.80.0.0/16"] }),
        case,
    )
    .await;
    let network = get_network(&router, &id, case).await;
    assert_eq!(
        at(&network, "peeredCidrs"),
        &json!(["10.80.0.0/16"]),
        "case {case:?}: a non-overlapping peered CIDR must be stored: {network}"
    );
}

#[tokio::test]
async fn a_stored_peered_cidr_is_canonical_and_bounded_per_network() {
    let router = emulator();
    let case = "peered CIDR canonicalisation";
    let id = create(&router, &minimal_request("peer_bounds", "10.95.0.0/24"), case).await;

    // Two spellings of one range — the second carries host bits — are one entry, so a
    // caller cannot grow the stored list by respelling what it already added.
    for spelling in ["10.90.0.0/16", "10.90.0.1/16", "10.90.255.7/16"] {
        call_ok(
            &router,
            "UpdateOdbNetwork",
            &json!({ "odbNetworkId": id, "peeredCidrsToBeAdded": [spelling] }),
            spelling,
        )
        .await;
        let network = get_network(&router, &id, spelling).await;
        assert_eq!(
            at(&network, "peeredCidrs"),
            &json!(["10.90.0.0/16"]),
            "case {spelling:?}: a peered range is stored once, in canonical form: {network}"
        );
    }

    // A removal names the range in any spelling too.
    call_ok(
        &router,
        "UpdateOdbNetwork",
        &json!({ "odbNetworkId": id, "peeredCidrsToBeRemoved": ["10.90.4.9/16"] }),
        case,
    )
    .await;
    let network = get_network(&router, &id, case).await;
    assert_eq!(
        at(&network, "peeredCidrs"),
        &json!([]),
        "case {case:?}: a removal must match the stored canonical range: {network}"
    );

    // Filling the per-network bound is allowed; one range past it is refused and the
    // stored list is left exactly at the bound.
    let full: Vec<String> = (0..MAX_PEERED_CIDRS_PER_ODB_NETWORK)
        .map(|index| format!("172.{}.0.0/24", 20 + index))
        .collect();
    call_ok(
        &router,
        "UpdateOdbNetwork",
        &json!({ "odbNetworkId": id, "peeredCidrsToBeAdded": full }),
        "fill the peered CIDR bound",
    )
    .await;
    let network = get_network(&router, &id, case).await;
    assert_eq!(
        at(&network, "peeredCidrs").as_array().map(Vec::len),
        Some(MAX_PEERED_CIDRS_PER_ODB_NETWORK),
        "case {case:?}: every range up to the bound must be stored: {network}"
    );

    assert_refused(
        call(
            &router,
            "UpdateOdbNetwork",
            &json!({ "odbNetworkId": id, "peeredCidrsToBeAdded": ["192.0.2.0/24"] }),
        )
        .await,
        400,
        "ValidationException",
        "one peered CIDR past the per-network bound",
    );
    let network = get_network(&router, &id, case).await;
    assert_eq!(
        at(&network, "peeredCidrs").as_array().map(Vec::len),
        Some(MAX_PEERED_CIDRS_PER_ODB_NETWORK),
        "case {case:?}: a refused update must leave the stored list unchanged: {network}"
    );
}

#[tokio::test]
async fn a_cross_region_restore_source_must_be_a_region_name_and_is_bounded() {
    let router = emulator();
    let case = "restore sources";

    // Region names, in every published form: two-letter partition plus area and index,
    // a multi-word area, and a partition-qualified Region.
    let accepted = ["us-east-1", "ap-southeast-3", "us-gov-west-1", "us-iso-east-1"];
    let id = create(
        &router,
        &json!({
            "displayName": "restore_sources",
            "clientSubnetCidr": "10.96.0.0/24",
            "availabilityZone": "us-west-2a",
            "crossRegionS3RestoreSourcesToEnable": accepted,
        }),
        case,
    )
    .await;
    let network = get_network(&router, &id, case).await;
    let stored: Vec<String> = at(&network, "managedServices.crossRegionS3RestoreSourcesAccess")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("region"))
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored,
        accepted.map(ToString::to_string).to_vec(),
        "case {case:?}: every Region name given must be stored, in order: {network}"
    );

    // Anything that is not a Region name is refused rather than persisted — on create
    // and on update alike.
    for rejected in [
        "US-EAST-1",                 // Region names are lowercase
        "useast1",                   // no separators
        "us-east",                   // no index
        "../../etc/passwd",          // an arbitrary caller string
        &"r".repeat(600),            // an oversized caller string
    ] {
        let case = format!("crossRegionS3RestoreSourcesToEnable {rejected:?}");
        let mut request = minimal_request("bad_region", "10.97.0.0/24");
        request["crossRegionS3RestoreSourcesToEnable"] = json!([rejected]);
        assert_refused(
            call(&router, "CreateOdbNetwork", &request).await,
            400,
            "ValidationException",
            &case,
        );
        assert_refused(
            call(
                &router,
                "UpdateOdbNetwork",
                &json!({
                    "odbNetworkId": id,
                    "crossRegionS3RestoreSourcesToEnable": [rejected],
                }),
            )
            .await,
            400,
            "ValidationException",
            &case,
        );
        assert_eq!(
            at(&get_network(&router, &id, &case).await, "managedServices.crossRegionS3RestoreSourcesAccess")
                .as_array()
                .map(Vec::len),
            Some(accepted.len()),
            "case {case:?}: a refused update must store nothing new ({AWS_REGION_PATTERN})"
        );
    }

    // One Region past the per-network bound is refused.
    let too_many: Vec<String> = (0..=MAX_CROSS_REGION_RESTORE_SOURCES_PER_ODB_NETWORK)
        .map(|index| format!("eu-west-{index}"))
        .collect();
    assert_refused(
        call(
            &router,
            "UpdateOdbNetwork",
            &json!({
                "odbNetworkId": id,
                "crossRegionS3RestoreSourcesToEnable": too_many,
            }),
        )
        .await,
        400,
        "ValidationException",
        "one restore source past the per-network bound",
    );
}

#[tokio::test]
async fn the_display_name_pattern_accepts_every_form_the_model_allows() {
    let router = emulator();

    // The pattern the model declares, enforced here because the boundary validator's
    // regex engine cannot compile its negative lookahead.
    let accepted = [
        "a",                                  // the shortest allowed name
        "_leading_underscore",                 // the pattern's other allowed first char
        "Mixed_Case-Name",                     // every allowed character class
        "a-b-c",                               // single hyphens are fine
        &"n".repeat(255),                      // the modelled maximum length
    ];
    for (index, display_name) in accepted.into_iter().enumerate() {
        let case = format!("displayName {display_name:?} satisfies {RESOURCE_DISPLAY_NAME_PATTERN}");
        let request = minimal_request(display_name, &format!("10.{}.0.0/24", 100 + index));
        let id = create(&router, &request, &case).await;
        let network = get_network(&router, &id, &case).await;
        assert_eq!(
            at(&network, "displayName"),
            &json!(display_name),
            "case {case:?}: the stored displayName must be the one given: {network}"
        );
    }

    // One past the modelled maximum length.
    assert_refused(
        call(
            &router,
            "CreateOdbNetwork",
            &minimal_request(&"n".repeat(256), "10.200.0.0/24"),
        )
        .await,
        400,
        "ValidationException",
        "displayName one character past the modelled maximum",
    );
}

/// One well-formed request per operation whose handler is still a scaffold, including
/// every operation this service's model binds that no handler implements yet.
///
/// The rows differ along what the rule generalises over: the kind of operation (a
/// create, a paginated list, a read, an associate/disassociate carrying a list member,
/// four updates and a refresh), which members its input requires, and whether the
/// addressed resource is named by a bare id or by an id-or-ARN. Each body satisfies
/// every modelled constraint, so the only thing left to answer is the handler itself.
fn scaffolded_operation_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AssociateVirtualMachinesToCloudVmCluster",
            json!({ "cloudVmClusterId": "vmclus-1a", "dbServers": ["dbserver-one"] }),
        ),
        (
            "DisassociateVirtualMachinesFromCloudVmCluster",
            json!({
                "cloudVmClusterId": "vmclus-1a",
                "dbServers": ["dbserver-one", "dbserver-two"],
            }),
        ),
        (
            "ListAutonomousDatabaseRefreshableClones",
            json!({ "autonomousDatabaseId": "adb-000001", "maxResults": 10 }),
        ),
        (
            "StartAutonomousDatabaseRefresh",
            json!({ "autonomousDatabaseId": "adb-000001" }),
        ),
        (
            "UpdateCloudAutonomousVmCluster",
            json!({
                "cloudAutonomousVmClusterId": "avmc-000001",
                "cpuCoreCountPerNode": 4,
                "displayName": "my_avm_cluster",
                "licenseModel": "LICENSE_INCLUDED",
            }),
        ),
        (
            "UpdateCloudExadataInfrastructureStorageCapacity",
            json!({ "cloudExadataInfrastructureId": "exainfra-000001" }),
        ),
        (
            "UpdateCloudVmCluster",
            json!({
                "cloudVmClusterId": "vmclus-1a",
                "cpuCoreCount": 8,
                "dataStorageSizeInTBs": 4.0,
                "sshPublicKeys": ["ssh-rsa AAAA"],
            }),
        ),
        (
            "UpdateExascaleCloudExadataInfrastructure",
            json!({
                "cloudExadataInfrastructureId": "exainfra-000001",
                "totalStorageSizeInGBs": 1000,
            }),
        ),
        // The two operations whose input the model extended with
        // `cloudExadataInfrastructureId` (and, for the list, `vaultType`).
        (
            "CreateExascaleDbStorageVault",
            json!({
                "displayName": "my_vault",
                "highCapacityDatabaseStorageTotalSizeInGBs": 300,
                "cloudExadataInfrastructureId": "exainfra-000001",
            }),
        ),
        (
            "ListExascaleDbStorageVaults",
            json!({
                "cloudExadataInfrastructureId": "exainfra-000001",
                "vaultType": "EXASCALE_INFRASTRUCTURE",
            }),
        ),
        // A read the model has always bound, so the rule is not read off the
        // operations this change touched.
        (
            "GetExascaleDbStorageVault",
            json!({ "exascaleDbStorageVaultId": "exavault-000001" }),
        ),
    ]
}

#[tokio::test]
async fn every_modelled_operation_reaches_its_own_handler() {
    let router = emulator();

    for (operation, request) in scaffolded_operation_cases() {
        let case = format!("{operation} with a request that satisfies every constraint");
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 501,
            "case {case:?}: an operation no handler implements must answer HTTP 501, \
             got {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected the scaffold error code NotImplemented, got {body}"
        );
    }
}

#[tokio::test]
async fn a_modelled_constraint_is_enforced_before_any_handler_runs() {
    let router = emulator();

    // Each row breaks exactly one constraint the model declares — a required member
    // left out, a string below its minimum length or outside its pattern, a number
    // outside its range, a list below its minimum length, a value outside an enum's
    // value set — so the request must be refused with the modelled
    // `ValidationException` (HTTP 400) and never reach the handler's 501.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "cloudVmClusterId below the modelled minimum length",
            "AssociateVirtualMachinesToCloudVmCluster",
            json!({ "cloudVmClusterId": "vmclu", "dbServers": ["dbserver-one"] }),
        ),
        (
            "dbServers missing altogether",
            "AssociateVirtualMachinesToCloudVmCluster",
            json!({ "cloudVmClusterId": "vmclus-1a" }),
        ),
        (
            "dbServers below the modelled minimum list length",
            "DisassociateVirtualMachinesFromCloudVmCluster",
            json!({ "cloudVmClusterId": "vmclus-1a", "dbServers": [] }),
        ),
        (
            "maxResults below the modelled minimum",
            "ListAutonomousDatabaseRefreshableClones",
            json!({ "autonomousDatabaseId": "adb-000001", "maxResults": 0 }),
        ),
        (
            "autonomousDatabaseId missing altogether",
            "ListAutonomousDatabaseRefreshableClones",
            json!({ "maxResults": 10 }),
        ),
        (
            "autonomousDatabaseId below the modelled minimum length",
            "StartAutonomousDatabaseRefresh",
            json!({ "autonomousDatabaseId": "adb" }),
        ),
        (
            "displayName one character past the modelled maximum",
            "UpdateCloudAutonomousVmCluster",
            json!({ "cloudAutonomousVmClusterId": "avmc-000001", "displayName": "n".repeat(256) }),
        ),
        (
            "cpuCoreCountPerNode below the modelled minimum",
            "UpdateCloudAutonomousVmCluster",
            json!({ "cloudAutonomousVmClusterId": "avmc-000001", "cpuCoreCountPerNode": -1 }),
        ),
        (
            "licenseModel outside the modelled value set",
            "UpdateCloudAutonomousVmCluster",
            json!({ "cloudAutonomousVmClusterId": "avmc-000001", "licenseModel": "license_included" }),
        ),
        (
            "cloudExadataInfrastructureId missing altogether",
            "UpdateCloudExadataInfrastructureStorageCapacity",
            json!({}),
        ),
        (
            "dataStorageSizeInTBs below the modelled minimum",
            "UpdateCloudVmCluster",
            json!({ "cloudVmClusterId": "vmclus-1a", "dataStorageSizeInTBs": -1.0 }),
        ),
        (
            "totalStorageSizeInGBs missing altogether",
            "UpdateExascaleCloudExadataInfrastructure",
            json!({ "cloudExadataInfrastructureId": "exainfra-000001" }),
        ),
        (
            "cloudExadataInfrastructureId below the modelled minimum length",
            "CreateExascaleDbStorageVault",
            json!({
                "displayName": "my_vault",
                "highCapacityDatabaseStorageTotalSizeInGBs": 300,
                "cloudExadataInfrastructureId": "exa-1",
            }),
        ),
        (
            "cloudExadataInfrastructureId below the modelled minimum length",
            "ListExascaleDbStorageVaults",
            json!({ "cloudExadataInfrastructureId": "exa-1" }),
        ),
        (
            "vaultType outside the modelled value set",
            "ListExascaleDbStorageVaults",
            json!({ "vaultType": "exascale_infrastructure" }),
        ),
        (
            "exascaleDbStorageVaultId below the modelled minimum length",
            "GetExascaleDbStorageVault",
            json!({ "exascaleDbStorageVaultId": "exa-1" }),
        ),
    ];

    for (reason, operation, request) in cases {
        let case = format!("{operation}: {reason}");
        assert_refused(
            call(&router, operation, &request).await,
            400,
            "ValidationException",
            &case,
        );
    }
}

#[test]
fn the_exascale_storage_vault_operations_carry_the_modelled_not_found_error() {
    use service_com_amazonaws_odb_odb::ops::create_exascale_db_storage_vault::CreateExascaleDbStorageVaultErrorModeled;
    use service_com_amazonaws_odb_odb::ops::list_exascale_db_storage_vaults::ListExascaleDbStorageVaultsErrorModeled;
    use service_com_amazonaws_odb_odb::ops::types::ResourceNotFoundException;
    use smithy_server_runtime::OperationError;

    /// The payload the error channel carries; the wire code and status under test come
    /// from the model, not from these values.
    fn not_found() -> ResourceNotFoundException {
        ResourceNotFoundException {
            message: "the resource was not found".to_string(),
            resource_id: "exainfra-000001".to_string(),
            resource_type: "CLOUD_EXADATA_INFRASTRUCTURE".to_string(),
        }
    }

    let create = CreateExascaleDbStorageVaultErrorModeled::ResourceNotFoundException(not_found());
    let list = ListExascaleDbStorageVaultsErrorModeled::ResourceNotFoundException(not_found());

    for (case, status, code) in [
        (
            "CreateExascaleDbStorageVault",
            create.http_status(),
            create.shape_name(),
        ),
        (
            "ListExascaleDbStorageVaults",
            list.http_status(),
            list.shape_name(),
        ),
    ] {
        assert_eq!(
            status, 404,
            "case {case:?}: the modelled not-found error renders with the model's own \
             status, got {status}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected the modelled error code, got {code}"
        );
    }
}
