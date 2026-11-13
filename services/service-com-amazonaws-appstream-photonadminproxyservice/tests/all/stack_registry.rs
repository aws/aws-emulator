// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STACK REGISTRY: which stack names this service answers for,
//! and for how long.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness — `POST /` with
//! `X-Amz-Target: PhotonAdminProxyService.<Operation>` — so what is exercised is the
//! dispatch → deserialize → handle → serialize walk and not a handler called directly.
//!
//! What the record a create persists must CONTAIN is pinned by
//! `tests/all/stack_fleet_lifecycle.rs`; what this suite pins is which names the registry
//! resolves, as rules asserted over a TABLE of inputs rather than over one recorded
//! request:
//!
//! * a stack `CreateStack` accepted is addressable under the name it accepted for the
//!   rest of that stack's life: the by-name `DescribeStacks` resolves the stored record
//!   and the by-name `DeleteStack` removes it, whatever form the name takes and whatever
//!   configuration the create carried;
//! * only an accepted create adds a name to the registry and only an accepted delete
//!   removes one — a request this service REFUSES (a value outside the model's
//!   constraints, a name already taken, a name that names no stack) leaves the set of
//!   addressable names and every stored record exactly as they were;
//! * a delete answers an empty output and removes only the stack it names, so the stacks
//!   beside it stay addressable.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The resource word AWS's refusal sentences use for a stack.
const STACK_KIND: &str = "stack";

/// The wire status this service's `ResourceNotFoundException` leaves with, stated once so
/// the rows below do not each encode it; the sibling suites assert the same status. The
/// shape's `@httpError` is 404
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`) while AppStream
/// serves this refusal over HTTP 400, which is the status the serializer answers with
/// (`crate::state::modelled_error_status` in the service crate).
const NOT_FOUND_STATUS: u16 = 400;

/// A name no row ever creates, so every operation must report it as missing.
const NEVER_CREATED: &str = "never-created-stack";

/// Stack creates spanning the class, keyed by the FORM of the name and by the
/// configuration the create carries: the rules below must hold for each of them alike.
///
/// The name forms span the `Name` member's pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
/// (`smithy_query(query="operation", operation="CreateStack")`): one character, digits
/// only, each of the three punctuation characters it admits after the first, mixed case,
/// and the longest name it admits (101 characters). The configurations span a create that
/// carries nothing but the name, one that carries the interface-endpoint configuration a
/// VPC-endpoint-scoped stack is made for, one that carries a nested structure and a list
/// of structures, and one that carries a tag map.
fn accepted_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("single-character name, no configuration", json!({ "Name": "a" })),
        (
            "digits-only name, tags",
            json!({ "Name": "0123456789", "Tags": { "Environment": "Test" } }),
        ),
        (
            // The configuration a stack scoped to an interface VPC endpoint is created
            // with: one STREAMING access endpoint. `AccessEndpointType`'s only value is
            // STREAMING (`smithy_query(query="shape", shape="AccessEndpointType")`).
            "dot-separated name, one streaming access endpoint",
            json!({
                "Name": "stack.v1.2",
                "AccessEndpoints": [
                    { "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f60718" },
                ],
            }),
        ),
        (
            "underscore-separated name, user and application settings",
            json!({
                "Name": "stack_v1_2",
                "UserSettings": [
                    { "Action": "FILE_DOWNLOAD", "Permission": "ENABLED" },
                    { "Action": "PRINTING_TO_LOCAL_DEVICE", "Permission": "DISABLED" },
                ],
                "ApplicationSettings": { "Enabled": true, "SettingsGroup": "group-a" },
            }),
        ),
        (
            "hyphenated mixed-case name, embedded hosts and streaming experience",
            json!({
                "Name": "Stack-V1-2",
                "EmbedHostDomains": ["a.example.invalid"],
                "StreamingExperienceSettings": { "PreferredProtocol": "TCP" },
            }),
        ),
        (
            "the longest name the pattern admits, no configuration",
            json!({ "Name": format!("z{}", "a".repeat(100)) }),
        ),
    ]
}

/// The name a row's create asks for.
fn row_name(request: &Value) -> String {
    request["Name"]
        .as_str()
        .expect("every row names a stack")
        .to_string()
}

/// One request this service must refuse, with the wire status and the error code the
/// refusal carries, and the name that must stay unknown once it has been refused — a
/// well-formed one, since `DescribeStacks.Names` is a plain `StringList` and reading back
/// a malformed name would assert the refusal of the READ rather than the absence of the
/// stack.
struct RefusedRow {
    case: String,
    operation: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    leaves_unknown: Option<&'static str>,
}

/// Requests spanning the refusal class, each paired with the name of a stack that IS
/// stored (`stored`) so a row can collide with it or name it beside a missing one.
///
/// The rows differ along WHY the request is refused: a member value outside an enum the
/// model declares, a list shorter and longer than the model's bounds, a name outside the
/// model's pattern and one character past its maximum length, a name already taken, and a
/// name that names no stack — read, updated and deleted. The constraints come from
/// `smithy_query(query="operation", operation="CreateStack")` and
/// `smithy_query(query="shape", shape="AccessEndpointList,AccessEndpointType")`:
/// `AccessEndpointList` is `@length min 1 max 4`, `AccessEndpointType` admits only
/// STREAMING, and `Name` matches `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`.
fn refused_rows(stored: &str) -> Vec<RefusedRow> {
    let endpoint = |vpce: &str| json!({ "EndpointType": "STREAMING", "VpceId": vpce });
    vec![
        RefusedRow {
            case: "an access endpoint type the enum does not declare".to_string(),
            operation: "CreateStack",
            request: json!({
                "Name": "refused-endpoint-type",
                "AccessEndpoints": [
                    { "EndpointType": "API", "VpceId": "vpce-0a1b2c3d4e5f60718" },
                ],
            }),
            status: 400,
            code: "ValidationException",
            leaves_unknown: Some("refused-endpoint-type"),
        },
        RefusedRow {
            case: "an access endpoint list below the modelled minimum".to_string(),
            operation: "CreateStack",
            request: json!({ "Name": "refused-empty-endpoints", "AccessEndpoints": [] }),
            status: 400,
            code: "ValidationException",
            leaves_unknown: Some("refused-empty-endpoints"),
        },
        RefusedRow {
            case: "an access endpoint list above the modelled maximum".to_string(),
            operation: "CreateStack",
            request: json!({
                "Name": "refused-five-endpoints",
                "AccessEndpoints": [
                    endpoint("vpce-0a1b2c3d4e5f60711"),
                    endpoint("vpce-0a1b2c3d4e5f60712"),
                    endpoint("vpce-0a1b2c3d4e5f60713"),
                    endpoint("vpce-0a1b2c3d4e5f60714"),
                    endpoint("vpce-0a1b2c3d4e5f60715"),
                ],
            }),
            status: 400,
            code: "ValidationException",
            leaves_unknown: Some("refused-five-endpoints"),
        },
        RefusedRow {
            case: "a name the pattern does not admit".to_string(),
            operation: "CreateStack",
            request: json!({ "Name": "-leading-hyphen" }),
            status: 400,
            code: "ValidationException",
            // The refused name is not well-formed, so there is no name to read back:
            // the registry-unchanged assertion below is what proves nothing was stored.
            leaves_unknown: None,
        },
        RefusedRow {
            case: "a name one character past the pattern's maximum".to_string(),
            operation: "CreateStack",
            request: json!({ "Name": format!("z{}", "a".repeat(101)) }),
            status: 400,
            code: "ValidationException",
            leaves_unknown: None,
        },
        RefusedRow {
            case: format!("a create of the name {stored:?}, which is already taken"),
            operation: "CreateStack",
            request: json!({ "Name": stored }),
            status: 400,
            code: "ResourceAlreadyExistsException",
            // The name names the stack that IS stored, so it must stay addressable, which
            // the registry-unchanged assertion covers.
            leaves_unknown: None,
        },
        RefusedRow {
            case: "a read of a name that names no stack".to_string(),
            operation: "DescribeStacks",
            request: json!({ "Names": [NEVER_CREATED] }),
            status: NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            leaves_unknown: Some(NEVER_CREATED),
        },
        RefusedRow {
            case: format!("a read of {stored:?} beside a name that names no stack"),
            operation: "DescribeStacks",
            request: json!({ "Names": [stored, NEVER_CREATED] }),
            status: NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            leaves_unknown: Some(NEVER_CREATED),
        },
        RefusedRow {
            case: "an update of a name that names no stack".to_string(),
            operation: "UpdateStack",
            request: json!({ "Name": NEVER_CREATED, "Description": "an update nobody applies" }),
            status: NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            leaves_unknown: Some(NEVER_CREATED),
        },
        RefusedRow {
            case: "a delete of a name that names no stack".to_string(),
            operation: "DeleteStack",
            request: json!({ "Name": NEVER_CREATED }),
            status: NOT_FOUND_STATUS,
            code: "ResourceNotFoundException",
            leaves_unknown: Some(NEVER_CREATED),
        },
    ]
}

/// Every stack the registry currently answers for, keyed by name, read through the
/// unfiltered `DescribeStacks`.
async fn stored_stacks(emulator: &Emulator, case: &str) -> BTreeMap<String, Value> {
    let all = emulator.ok(case, "DescribeStacks", json!({})).await;
    // One page holds every row this suite stores, so the comparisons below read the whole
    // registry rather than a truncated page: a token here means a row was added past
    // `state::DESCRIBE_PAGE_SIZE` and the page must be followed.
    assert_eq!(
        all.get("NextToken"),
        None,
        "case {case:?}: the unfiltered read must report every stored stack in one page, \
         got {all}"
    );
    all["Stacks"]
        .as_array()
        .expect("DescribeStacks reports a Stacks list")
        .iter()
        .map(|stack| {
            (
                stack["Name"]
                    .as_str()
                    .expect("every reported stack is named")
                    .to_string(),
                stack.clone(),
            )
        })
        .collect()
}

/// An emulator holding every [`accepted_rows`] stack, and the create response each row
/// answered with.
async fn emulator_with_every_row() -> (Emulator, BTreeMap<String, Value>) {
    let emulator = Emulator::new();
    let mut accepted = BTreeMap::new();
    for (case, request) in accepted_rows() {
        let created = emulator.ok(case, "CreateStack", request.clone()).await;
        accepted.insert(row_name(&request), created["Stack"].clone());
    }
    (emulator, accepted)
}

#[tokio::test]
async fn an_accepted_stack_is_addressable_by_the_name_the_create_accepted() {
    let (emulator, accepted) = emulator_with_every_row().await;

    // Cause 1 of the issue this suite pins: the by-name read must resolve the stack the
    // create accepted — every row, after every other row was created, so no name shadows
    // another — and must report the record the create persisted rather than a
    // re-synthesised one.
    let stored = stored_stacks(&emulator, "every row").await;
    for (case, request) in accepted_rows() {
        let name = row_name(&request);
        assert!(
            stored.contains_key(&name),
            "case {case:?}: the unfiltered read must report {name:?}, got {:?}",
            stored.keys().collect::<Vec<_>>()
        );

        let described = emulator
            .ok(case, "DescribeStacks", json!({ "Names": [&name] }))
            .await;
        assert_eq!(
            described["Stacks"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: DescribeStacks(Names=[{name}]) must report exactly that stack, \
             got {described}"
        );
        assert_eq!(
            described["Stacks"][0], accepted[&name],
            "case {case:?}: the by-name read must report the record CreateStack accepted, \
             got {described}"
        );
        assert_eq!(
            described["Stacks"][0], stored[&name],
            "case {case:?}: the by-name read and the unfiltered read must report the same \
             record for {name:?}, got {described}"
        );
    }
}

#[tokio::test]
async fn a_refused_request_leaves_the_registry_exactly_as_it_was() {
    let (emulator, _) = emulator_with_every_row().await;

    let stored = stored_stacks(&emulator, "before any refusal").await;
    let collides_with = row_name(&accepted_rows()[0].1);

    for row in refused_rows(&collides_with) {
        refusal(
            &emulator,
            &row.case,
            row.operation,
            row.request.clone(),
            row.status,
            row.code,
        )
        .await;

        // Nothing was added (a refused create stores no stack), nothing was removed (a
        // refused read, update or delete is not a teardown) and nothing was rewritten:
        // every accepted stack is still addressable, unchanged.
        let after = stored_stacks(&emulator, &row.case).await;
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            stored.keys().collect::<Vec<_>>(),
            "case {:?}: a refused {} must not change which stacks are addressable, got {:?}",
            row.case,
            row.operation,
            after.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            after, stored,
            "case {:?}: a refused {} must leave every stored record untouched",
            row.case, row.operation
        );

        // And a name the refused request named, which no accepted create stored, is
        // addressable by nobody.
        if let Some(unknown) = row.leaves_unknown {
            let message = refusal(
                &emulator,
                &row.case,
                "DescribeStacks",
                json!({ "Names": [unknown] }),
                NOT_FOUND_STATUS,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message,
                not_found_sentence(STACK_KIND, unknown),
                "case {:?}: the registry must still report {unknown:?} as unknown, got \
                 {message:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_delete_removes_only_the_stack_it_names() {
    let (emulator, accepted) = emulator_with_every_row().await;

    let names: Vec<String> = accepted_rows()
        .iter()
        .map(|(_, request)| row_name(request))
        .collect();

    for (position, name) in names.iter().enumerate() {
        let case = format!("delete {name:?}");
        // The stacks the deletes so far have not named: every one of them must still be
        // addressable once this delete lands.
        let survivors = &names[position + 1..];

        // Cause 2 of the issue this suite pins: the by-name delete resolves the stack the
        // create accepted, and answers the modelled empty output.
        let (status, payload) = emulator.call("DeleteStack", json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack must delete the stack the create accepted, \
             got {status} {payload}"
        );
        // `DeleteStackResult` declares no members, so the body is the empty structure —
        // or no body at all, which the harness reports as `null`.
        assert!(
            payload == json!({}) || payload.is_null(),
            "case {case:?}: DeleteStackResult carries no members, got {payload}"
        );

        // The delete mutated the registry: both the reader and a repeat delete now report
        // the name as missing, with AWS's sentence for that refusal.
        for (operation, request) in [
            ("DescribeStacks", json!({ "Names": [name] })),
            ("DeleteStack", json!({ "Name": name })),
        ] {
            let message = refusal(
                &emulator,
                &case,
                operation,
                request,
                NOT_FOUND_STATUS,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message,
                not_found_sentence(STACK_KIND, name),
                "case {case:?}: {operation} must report {name:?} as missing, got {message:?}"
            );
        }

        // Every stack the delete did not name is still addressable by its own name, and
        // still reports what its create accepted.
        let stored = stored_stacks(&emulator, &case).await;
        let mut expected: Vec<String> = survivors.to_vec();
        expected.sort();
        assert_eq!(
            stored.keys().cloned().collect::<Vec<_>>(),
            expected,
            "case {case:?}: a delete must remove only the stack it names, got {:?}",
            stored.keys().collect::<Vec<_>>()
        );
        for other in survivors {
            let described = emulator
                .ok(&case, "DescribeStacks", json!({ "Names": [other] }))
                .await;
            assert_eq!(
                described["Stacks"][0], accepted[other],
                "case {case:?}: {other:?} must still resolve to the record its create \
                 accepted, got {described}"
            );
        }
    }
}
