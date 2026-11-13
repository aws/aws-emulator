// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HTTP STATUS a refusal of this service leaves with.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness — `POST /` with
//! `X-Amz-Target: PhotonAdminProxyService.<Operation>` — so what is asserted is the status
//! line and the body a client actually parses, not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a refusal for a resource this account holds nothing under leaves with HTTP 400 and
//!   the wire code `ResourceNotFoundException`, whatever operation reports it, whichever
//!   resource family it is about, however the request addresses that resource and whatever
//!   form the name takes — the status does not vary with any of them, and the sentence the
//!   body carries is AWS's for that refusal class;
//! * every other refusal class keeps the status its own shape states, so restating the
//!   not-found status moves nothing else: a name already taken, a resource still in use and
//!   a value outside the model's constraints are each answered as they were, and an
//!   operation that is still a scaffold still answers HTTP 501.
//!
//! The 400 is the status AppStream serves these refusals over. `ResourceNotFoundException`
//! is the one error shape of this model whose `@httpError` does not state it
//! (`smithy_query(query="shape", shape="ResourceNotFoundException")` reports 404, while
//! every other error shape of the service reports 400), which is why the first rule is
//! worth a suite of its own.

use serde_json::{json, Value};

use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every ARN below carries: the single account this emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region every request below is signed for, and so the region its ARNs carry.
const REGION: &str = "us-east-1";

/// The status this service answers a refused request with — the one status every error
/// shape of this model declares but one, and the one AppStream serves them all over.
const REFUSAL_STATUS: u16 = 400;

/// The status a scaffolded operation answers with, which is NOT a refusal of the request:
/// nobody has written that operation yet.
const SCAFFOLD_STATUS: u16 = 501;

/// The resources these tests leave stored, so a row can make exactly one side of a
/// two-sided request the missing one.
const STORED_STACK: &str = "stored-stack";
const STORED_FLEET: &str = "stored-fleet";

/// Names spanning the class: the rows differ along length, character classes and case. The
/// last is the longest name a create admits — `Name` matches
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
/// (`smithy_query(query="operation", operation="CreateStack")`), so 101 characters.
fn names() -> Vec<String> {
    vec![
        "absent".to_string(),
        "Mixed.Case_name-9".to_string(),
        "0".to_string(),
        format!("z{}", "a".repeat(100)),
    ]
}

/// The ARN this service stamps on a resource of `resource_type` named `name`; the
/// `resource-type` segments are the ones the crate's ARN helpers use (`stack`, `fleet`,
/// `application`, `app-block-builder`).
fn arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:{resource_type}/{name}")
}

/// AWS's sentence for a stack that has no custom branding theme. A theme has no name of
/// its own, so the sentence names the stack it brands; the two spaces before `does not
/// exist` are AWS's own, as the sibling suite (`tests/all/stack_theme.rs`) states.
fn theme_not_found_sentence(stack: &str) -> String {
    format!("The theme for stack {stack} for account {ACCOUNT}  does not exist.")
}

/// One not-found refusal: the operation that reports it, how the request addresses the
/// resource, and the sentence AWS words that refusal with for the name the request asked
/// for.
struct NotFoundRow {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
    message: fn(&str) -> String,
}

/// Every implemented operation that refuses a resource this account holds nothing under,
/// spanning the request forms and the resource families this service names resources in.
///
/// The rows differ along: the KIND of operation (a read, an update, a delete, a create,
/// and both sides of a two-sided association), the RESOURCE FAMILY (stack, fleet,
/// application, image, app block builder, a stack's branding theme, and a resource reached
/// only by ARN), and HOW the request addresses the resource (a single `Name`, a list of
/// `Names` — alone and beside a name that IS stored — a list of `Arns`, a `ResourceArn`,
/// and a member naming one side of a pair). The resource words the sentences use are the
/// crate's (`stack`, `fleet`, `application`, `image`, `appblockbuilder`), which the
/// sibling suites pin against AWS's wording.
fn not_found_rows() -> Vec<NotFoundRow> {
    vec![
        NotFoundRow {
            case: "a read of a name that names no stack",
            operation: "DescribeStacks",
            request: |name| json!({ "Names": [name] }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a read of a stored stack beside one that names no stack",
            operation: "DescribeStacks",
            request: |name| json!({ "Names": [STORED_STACK, name] }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "an update of a name that names no stack",
            operation: "UpdateStack",
            request: |name| json!({ "Name": name, "Description": "an update nobody applies" }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a delete of a name that names no stack",
            operation: "DeleteStack",
            request: |name| json!({ "Name": name }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a read of a name that names no fleet",
            operation: "DescribeFleets",
            request: |name| json!({ "Names": [name] }),
            message: |name| not_found_sentence("fleet", name),
        },
        NotFoundRow {
            case: "a delete of a name that names no fleet",
            operation: "DeleteFleet",
            request: |name| json!({ "Name": name }),
            message: |name| not_found_sentence("fleet", name),
        },
        NotFoundRow {
            case: "a start of a name that names no fleet",
            operation: "StartFleet",
            request: |name| json!({ "Name": name }),
            message: |name| not_found_sentence("fleet", name),
        },
        NotFoundRow {
            case: "an association whose fleet side names no fleet",
            operation: "AssociateFleet",
            request: |name| json!({ "FleetName": name, "StackName": STORED_STACK }),
            message: |name| not_found_sentence("fleet", name),
        },
        NotFoundRow {
            case: "a disassociation whose stack side names no stack",
            operation: "DisassociateFleet",
            request: |name| json!({ "FleetName": STORED_FLEET, "StackName": name }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a streaming URL asked for on a stack that does not exist",
            operation: "CreateStreamingURL",
            request: |name| {
                json!({ "StackName": name, "FleetName": STORED_FLEET, "UserId": "streaming-user" })
            },
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a branding read on a stack that does not exist",
            operation: "DescribeThemeForStack",
            request: |name| json!({ "StackName": name }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a branding delete where there is no theme to delete",
            operation: "DeleteThemeForStack",
            request: |name| json!({ "StackName": name }),
            message: theme_not_found_sentence,
        },
        NotFoundRow {
            case: "a read of a name that names no app block builder",
            operation: "DescribeAppBlockBuilders",
            request: |name| json!({ "Names": [name] }),
            message: |name| not_found_sentence("appblockbuilder", name),
        },
        NotFoundRow {
            case: "a read of a name that names no image",
            operation: "DescribeImages",
            request: |name| json!({ "Names": [name] }),
            message: |name| not_found_sentence("image", name),
        },
        NotFoundRow {
            case: "a read of an ARN that names no application",
            operation: "DescribeApplications",
            request: |name| json!({ "Arns": [arn("application", name)] }),
            message: |name| not_found_sentence("application", name),
        },
        NotFoundRow {
            case: "an update of a name that names no application",
            operation: "UpdateApplication",
            request: |name| json!({ "Name": name, "Description": "an update nobody applies" }),
            message: |name| not_found_sentence("application", name),
        },
        NotFoundRow {
            case: "a delete of a name that names no application",
            operation: "DeleteApplication",
            request: |name| json!({ "Name": name }),
            message: |name| not_found_sentence("application", name),
        },
        NotFoundRow {
            case: "a tag read on an ARN that names no stack",
            operation: "ListTagsForResource",
            request: |name| json!({ "ResourceArn": arn("stack", name) }),
            message: |name| not_found_sentence("stack", name),
        },
        NotFoundRow {
            case: "a tag write on an ARN that names no fleet",
            operation: "TagResource",
            request: |name| {
                json!({ "ResourceArn": arn("fleet", name), "Tags": { "Environment": "Test" } })
            },
            message: |name| not_found_sentence("fleet", name),
        },
        NotFoundRow {
            case: "a tag removal on an ARN that names no app block builder",
            operation: "UntagResource",
            request: |name| {
                json!({ "ResourceArn": arn("app-block-builder", name), "TagKeys": ["Environment"] })
            },
            message: |name| not_found_sentence("appblockbuilder", name),
        },
    ]
}

/// Store one stack and one fleet, so a two-sided request can have exactly one missing
/// side and a row can read a stored name beside an absent one.
async fn emulator_with_stored_pair() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": STORED_STACK }))
        .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({ "Name": STORED_FLEET, "InstanceType": "stream.standard.small" }),
        )
        .await;
    emulator
}

#[tokio::test]
async fn a_not_found_refusal_leaves_with_the_services_refusal_status() {
    let emulator = emulator_with_stored_pair().await;

    for row in not_found_rows() {
        for name in names() {
            let case = format!("{} — {} {name:?}", row.operation, row.case);
            let message = refusal(
                &emulator,
                &case,
                row.operation,
                (row.request)(&name),
                REFUSAL_STATUS,
                "ResourceNotFoundException",
            )
            .await;

            let expected = (row.message)(&name);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_other_refusal_class_keeps_the_status_it_already_answered_with() {
    let emulator = emulator_with_stored_pair().await;
    // The stored fleet is associated with the stored stack, so the stack is in use and a
    // delete of it is refused for a reason that is not "not found".
    emulator
        .ok(
            "setup",
            "AssociateFleet",
            json!({ "FleetName": STORED_FLEET, "StackName": STORED_STACK }),
        )
        .await;

    // Rows: the refusal classes this service answers beside the not-found one, each with
    // the status its own shape states — and the scaffold channel, whose 501 is not a
    // refusal of the request at all.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a create of a name that is already taken",
            "CreateStack",
            json!({ "Name": STORED_STACK }),
            REFUSAL_STATUS,
            "ResourceAlreadyExistsException",
        ),
        (
            "a create of a fleet name that is already taken",
            "CreateFleet",
            json!({ "Name": STORED_FLEET, "InstanceType": "stream.standard.small" }),
            REFUSAL_STATUS,
            "ResourceAlreadyExistsException",
        ),
        (
            "a delete of a stack that still has a fleet associated with it",
            "DeleteStack",
            json!({ "Name": STORED_STACK }),
            REFUSAL_STATUS,
            "ResourceInUseException",
        ),
        (
            "a delete of a fleet that is still associated with a stack",
            "DeleteFleet",
            json!({ "Name": STORED_FLEET }),
            REFUSAL_STATUS,
            "ResourceInUseException",
        ),
        (
            "a create whose name is outside the model's pattern",
            "CreateStack",
            json!({ "Name": "-leading-hyphen" }),
            REFUSAL_STATUS,
            "ValidationException",
        ),
        (
            "a create whose access endpoint type is outside the model's enum",
            "CreateStack",
            json!({
                "Name": "refused-endpoint-type",
                "AccessEndpoints": [{ "EndpointType": "API", "VpceId": "vpce-0a1b2c3d4e5f60718" }],
            }),
            REFUSAL_STATUS,
            "ValidationException",
        ),
        (
            // `StopFleet` is still a scaffold (`grep -rl '^//! SCAFFOLDED, NOT
            // IMPLEMENTED:' src/ops/`), so it answers the scaffold signal even for a fleet
            // this account does not hold: the status mapping restates what a MODELLED
            // refusal leaves with and nothing else. Implementing `StopFleet` is what moves
            // this row to the not-found rule above.
            "a call to an operation nobody has written yet",
            "StopFleet",
            json!({ "Name": "absent" }),
            SCAFFOLD_STATUS,
            "NotImplemented",
        ),
    ];

    for (case, operation, request, status, code) in rows {
        assert_error(&emulator, case, operation, request, status, code).await;
    }
}
