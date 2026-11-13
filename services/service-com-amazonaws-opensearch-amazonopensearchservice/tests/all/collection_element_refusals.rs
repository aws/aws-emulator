// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service STATES a request whose input breaks a
//! constraint the model declares on the ELEMENTS of a collection member.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a renderer returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test, which holds for every collection member of every operation
//! rather than for the one a report cited: a value inside a collection that breaks the
//! constraints of the collection's ELEMENT shape is ONE validation error, reported
//! against the collection MEMBER (no element position and no `member` step), stating
//! the element shape's WHOLE constraint set — the constraints the value satisfied as
//! much as the ones it broke — and rendering the offending elements as the collection
//! they came from. A violation on a member that is not a collection element keeps the
//! framework's own clause, one per violation.
//!
//! So the assertions span a SET of inputs: a value too long for its element shape, one
//! too short, two offending elements in one list, an offending element beside a valid
//! one, a list every element satisfies (not refused at all), the same rule on a second
//! operation over a different element shape, and a scalar member's own refusal for
//! contrast.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::PACKAGE_SCOPE_OPERATION_ENUM_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::PACKAGE_NOT_FOUND;
use service_common::validation_message::hashed_enum_order;

use super::constraint_refusal_messages::value_set;
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// `UpdatePackageScope`: `POST /2021-01-01/packages/updateScope`.
async fn update_scope(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/updateScope"), Some(body))
        .await
}

/// `DescribeVpcEndpoints`: `POST /2021-01-01/opensearch/vpcEndpoints/describe`.
async fn describe_endpoints(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints/describe"),
            Some(body),
        )
        .await
}

/// A `PackageID` of the form the model's pattern admits, so a refusal under test is
/// about the member the case is exercising and nothing else.
const WELL_FORMED_PACKAGE_ID: &str = "pkg-0123456789abcdef";

/// The whole constraint set `PackageUser` declares, as one clause states it.
///
/// From the model: `PackageUser` is `@length(min: 6, max: 12)` with
/// `@pattern("^[0-9]{12}$|^GLOBAL$")`
/// (`smithy_query(query="shape",shape="PackageUser")`). The expression is QUOTED as the
/// model declares it, anchors included: its two BRANCHES each carry the whole-value pair,
/// so the pair at the ends of that text is not a wrapping this service's refusals drop —
/// the expression was a whole-value expression before any wrapping.
const PACKAGE_USER_SET: &str = "Member must satisfy constraint: [Member must have length less \
                                than or equal to 12, Member must have length greater than or \
                                equal to 6, Member must satisfy regular expression pattern: \
                                ^[0-9]{12}$|^GLOBAL$]";

/// The whole constraint set `VpcEndpointId` declares, as one clause states it — a
/// SECOND element shape, on a second operation, to pin that the rule is the collection
/// rule rather than one member's special case.
///
/// From the model: `VpcEndpointId` is `@length(min: 5, max: 256)` with
/// `@pattern("^aos-[a-zA-Z0-9]*$")`
/// (`smithy_query(query="shape",shape="VpcEndpoint")`) — an expression the API publishes
/// WITH the pair (the `Pattern` its API reference documents for a VPC endpoint id is
/// `^aos-[a-zA-Z0-9]*$`), so the clause quotes the model's text as written.
const VPC_ENDPOINT_ID_SET: &str = "Member must satisfy constraint: [Member must have length \
                                   less than or equal to 256, Member must have length greater \
                                   than or equal to 5, Member must satisfy regular expression \
                                   pattern: ^aos-[a-zA-Z0-9]*$]";

/// The `message` of a refusal, with the case named on failure.
fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected a refusal message, got {:?}", answer.body))
        .to_owned()
}

#[tokio::test]
async fn an_element_of_a_collection_is_refused_as_one_error_about_the_collection_member() {
    let emulator = Emulator::new();

    // (case, the request body, the whole message the refusal must state). Each row
    // differs along what the rule generalises over: which of the element shape's
    // constraints the value breaks, how many elements offend, whether a valid element
    // sits beside them, and which operation's collection member carries them.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a value too long for the element shape, and outside its pattern",
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "ADD",
                "PackageUserList": ["not-an-account-id"],
            }),
            format!(
                "1 validation error detected: Value '[not-an-account-id]' at 'packageUserList' \
                 failed to satisfy constraint: {PACKAGE_USER_SET}"
            ),
        ),
        (
            "a value too short for the element shape",
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "ADD",
                "PackageUserList": ["abc"],
            }),
            format!(
                "1 validation error detected: Value '[abc]' at 'packageUserList' failed to \
                 satisfy constraint: {PACKAGE_USER_SET}"
            ),
        ),
        (
            "two offending elements in one list",
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "REMOVE",
                "PackageUserList": ["abc", "0123456789abcdef"],
            }),
            format!(
                "1 validation error detected: Value '[abc, 0123456789abcdef]' at \
                 'packageUserList' failed to satisfy constraint: {PACKAGE_USER_SET}"
            ),
        ),
        (
            "an offending element beside a valid one",
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "ADD",
                "PackageUserList": ["123456789012", "nope"],
            }),
            format!(
                "1 validation error detected: Value '[nope]' at 'packageUserList' failed to \
                 satisfy constraint: {PACKAGE_USER_SET}"
            ),
        ),
        (
            "another operation's collection, over another element shape",
            json!({ "VpcEndpointIds": ["bad"] }),
            format!(
                "1 validation error detected: Value '[bad]' at 'vpcEndpointIds' failed to \
                 satisfy constraint: {VPC_ENDPOINT_ID_SET}"
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let answer = if body.get("VpcEndpointIds").is_some() {
            describe_endpoints(&emulator, body).await
        } else {
            update_scope(&emulator, body).await
        };
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let stated = message(&answer, case);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_collection_every_element_satisfies_is_not_refused() {
    let emulator = Emulator::new();

    // Each row is a `PackageUserList` every element of which satisfies the element
    // shape, spanning what the shape admits: an account id, the `GLOBAL` literal, both
    // together, and the empty list.
    let rows: Vec<(&str, Value)> = vec![
        ("one account id", json!(["123456789012"])),
        ("the GLOBAL literal", json!(["GLOBAL"])),
        ("both forms together", json!(["123456789012", "GLOBAL"])),
        ("no element at all", json!([])),
    ];

    for (case, package_user_list) in rows {
        let answer = update_scope(
            &emulator,
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "ADD",
                "PackageUserList": package_user_list,
            }),
        )
        .await;
        // Nothing in this account holds the well-formed id the rows carry, so the only
        // refusal left for them is the one a `PackageID` that resolves to no package
        // gets: a list every element of which satisfies its shape states no constraint
        // violation of its own.
        let stated = message(&answer, case);
        assert_eq!(
            stated, PACKAGE_NOT_FOUND,
            "case {case:?}: a list every element satisfies must not be refused as invalid \
             input, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_member_that_is_not_a_collection_element_keeps_its_own_clause() {
    let emulator = Emulator::new();

    // (case, the body, the whole message). A scalar member states the value it held at
    // its own path with the one requirement it broke, and two offending members are two
    // validation errors — neither is folded into a collection's clause.
    //
    // The enumerated row's value set is the member's whole declared set
    // (`PackageScopeOperationEnum`, `smithy_query(query="shape",
    // shape="PackageScopeOperationEnum")`) listed by the rule this service states one by
    // — the order its front door walks its own hash table over those values, which is the
    // sibling suite's subject.
    let operations = value_set(&hashed_enum_order(PACKAGE_SCOPE_OPERATION_ENUM_VALUES));
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a scalar outside its pattern",
            json!({
                "PackageID": "not-a-package-id",
                "Operation": "ADD",
                "PackageUserList": ["GLOBAL"],
            }),
            "1 validation error detected: Value 'not-a-package-id' at 'packageID' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \
             ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$"
                .to_string(),
        ),
        (
            "a scalar outside its enum",
            json!({
                "PackageID": WELL_FORMED_PACKAGE_ID,
                "Operation": "NOT_AN_OPERATION",
                "PackageUserList": ["GLOBAL"],
            }),
            format!(
                "1 validation error detected: Value 'NOT_AN_OPERATION' at 'operation' failed \
                 to satisfy constraint: {operations}"
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let answer = update_scope(&emulator, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        let stated = message(&answer, case);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}
