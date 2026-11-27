// SPDX-License-Identifier: CC0-1.0

//! The Direct Connect gateway and resiliency group lifecycles, end to end over the
//! wire.
//!
//! Both are the REGION-LESS resources of this service — a gateway "is global and
//! visible in any Amazon Web Services Region after it is created" and a group's ARN
//! carries no Region either — which is why their lifecycles are pinned together here.
//!
//! Every assertion goes through `POST /` with `X-Amz-Target: OvertureService.<Op>`
//! (awsJson1_1) — and, for the rule that the two protocols this model declares are two
//! ENCODINGS of one API over one store, through `rpcv2Cbor` as well — the same paths a
//! client takes, and every expectation is about what
//! the SERVICE STORED: a `CreateDirectConnectGateway` is proved by reading the
//! gateway back through `DescribeDirectConnectGateways`, and a
//! `CreateResiliencyGroup` by reading the group back through `GetResiliencyGroup`,
//! `ListResiliencyGroups` and `DescribeTags`.
//!
//! The tables span the class each rule generalises over (ASN range and bound, tag
//! count, name shape, page size, filter), and the ASN tables are BUILT from the
//! service's own `PRIVATE_AMAZON_SIDE_ASN_RANGES` — the ranges
//! `CreateDirectConnectGatewayRequest$amazonSideAsn` documents — so every published
//! bound and one value past it is covered rather than a chosen few.
//!
//! Two of the rules pinned here are about what a REFUSAL is: a `directConnectGatewayId`
//! that selects no gateway is an empty page and not a client fault, and an
//! `amazonSideAsn` outside the documented private ranges is refused with the one
//! sentence this service states for it, the rejected value and nothing else of the
//! request in it.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_directconnect_overtureservice::ops::types::{
    CreateDirectConnectGatewayInput, CreateDirectConnectGatewayOutput,
    DescribeDirectConnectGatewaysInput, DescribeDirectConnectGatewaysOutput,
    CREATE_DIRECT_CONNECT_GATEWAY_INPUT_SCHEMA, DESCRIBE_DIRECT_CONNECT_GATEWAYS_INPUT_SCHEMA,
};
use service_com_amazonaws_directconnect_overtureservice::state::gateway_associations::MAX_ALLOWED_PREFIXES_PER_ASSOCIATION;
use service_com_amazonaws_directconnect_overtureservice::state::resiliency_groups::{
    ResiliencyGroup, MAX_RESILIENCY_GROUP_NAME_LEN, RESILIENCY_GROUP_ID_DIGITS,
    RESILIENCY_GROUP_ID_PREFIX, RESILIENCY_GROUP_STATE_AVAILABLE,
    RESILIENCY_GROUP_STATE_DELETED, RESILIENCY_GROUP_STATE_DELETING,
    RESILIENCY_GROUP_STATE_PENDING, RESILIENCY_GROUP_TYPE_MANAGED,
};
use service_com_amazonaws_directconnect_overtureservice::state::{
    too_many_resources_message, too_many_tag_keys_message,
    ComAmazonawsDirectconnectOvertureserviceState, DEFAULT_AMAZON_SIDE_ASN, MAX_RECORDS_PER_STORE,
    MAX_RESOURCES_PER_CALL, MAX_RESULT_SET_SIZE, MAX_TAGS_PER_RESOURCE,
    PRIVATE_AMAZON_SIDE_ASN_RANGES,
};

use crate::connection_and_virtual_interface_lifecycle::{connection, error_code, Emulator};

/// The group lengths of a UUID (`8-4-4-4-12`), the form AWS reports a
/// `directConnectGatewayId` in.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// One row of the create table: case, name, `amazonSideAsn`, tags.
type GatewayRow<'a> = (&'a str, &'a str, Option<i64>, &'a [(&'a str, &'a str)]);

/// One row of the projection table: case, name, tags.
type ProjectionRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Every member the model declares on `DirectConnectGateway`, in declaration order
/// (`smithy_query(query="shape", shape="DirectConnectGateway")`) — the WHOLE set a
/// response may carry for a gateway, and the catalog the projection assertions are
/// derived from rather than transcribing one expected body.
const MODELLED_GATEWAY_MEMBERS: [&str; 8] = [
    "directConnectGatewayId",
    "directConnectGatewayName",
    "amazonSideAsn",
    "ownerAccount",
    "directConnectGatewayState",
    "stateChangeError",
    "totalPrefixPoolAllocations",
    "tags",
];

/// The member of `DirectConnectGateway` a gateway no state advance has failed for carries
/// nothing in: `stateChangeError` is "the error message if the state of an object failed
/// to advance", so it is absent from a healthy record rather than rendered empty.
const GATEWAY_MEMBER_ONLY_A_FAILED_ADVANCE_FILLS: &str = "stateChangeError";

/// The members a gateway record this service answers with is expected to carry: the
/// modelled set ([`MODELLED_GATEWAY_MEMBERS`]) less the one only a failed state advance
/// fills, sorted so the comparison is about the SET.
fn reported_gateway_members() -> Vec<String> {
    let mut members: Vec<String> = MODELLED_GATEWAY_MEMBERS
        .iter()
        .filter(|member| **member != GATEWAY_MEMBER_ONLY_A_FAILED_ADVANCE_FILLS)
        .map(|member| (*member).to_string())
        .collect();
    members.sort();
    members
}

/// The members `record` actually carries, sorted.
fn rendered_members(record: &Value) -> Vec<String> {
    let mut members: Vec<String> = record
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default();
    members.sort();
    members
}

/// Whether `id` is a server-minted gateway id: lowercase hex in UUID groups.
fn is_minted_gateway_id(id: &Value) -> bool {
    let Some(id) = id.as_str() else { return false };
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// The message an awsJson1_1 error body carries in `message`.
fn error_message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// A `CreateDirectConnectGateway` body.
fn create_gateway_body(
    name: &str,
    amazon_side_asn: Option<i64>,
    tags: &[(&str, &str)],
) -> Value {
    let mut body = Map::new();
    body.insert("directConnectGatewayName".to_string(), json!(name));
    if let Some(asn) = amazon_side_asn {
        body.insert("amazonSideAsn".to_string(), json!(asn));
    }
    if !tags.is_empty() {
        body.insert(
            "tags".to_string(),
            Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// Create one gateway and return its id, failing the test if it was refused.
async fn gateway(dx: &Emulator, name: &str) -> String {
    let (status, body) = dx
        .call(
            "CreateDirectConnectGateway",
            create_gateway_body(name, None, &[]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateDirectConnectGateway to succeed, got {status} with {body}"
    );
    body["directConnectGateway"]["directConnectGatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The gateways `DescribeDirectConnectGateways` answers for `body`, with the
/// response's `nextToken`.
async fn describe(dx: &Emulator, body: Value) -> (Vec<Value>, Value) {
    let (status, response) = dx.call("DescribeDirectConnectGateways", body).await;
    assert_eq!(
        status, 200,
        "expected DescribeDirectConnectGateways 200, got {status} with {response}"
    );
    (
        response["directConnectGateways"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        response["nextToken"].clone(),
    )
}

#[tokio::test]
async fn a_created_gateway_is_minted_stored_and_read_back_field_for_field() {
    let dx = Emulator::new();

    // (case, name, amazonSideAsn, tags). The ASNs span both private ranges the
    // service's PRIVATE_AMAZON_SIDE_ASN_RANGES names, including each bound.
    let rows: &[GatewayRow<'_>] = &[
        ("no amazonSideAsn, so the documented default", "dxgw-default", None, &[]),
        (
            "the low bound of the two-byte private range",
            "Gateway With Spaces",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[0].0),
            &[("env", "prod")],
        ),
        (
            "the high bound of the two-byte private range",
            "gw.with.dots",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[0].1),
            &[("env", "test"), ("team", "network")],
        ),
        (
            "the low bound of the four-byte private range",
            "x",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[1].0),
            &[],
        ),
        (
            "the high bound of the four-byte private range",
            "UPPER-and-lower_99",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[1].1),
            &[("only", "one-tag")],
        ),
        (
            "a value inside the four-byte private range",
            "mid-range-asn",
            Some(4_250_000_000),
            &[],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, name, amazon_side_asn, tags) in rows {
        let (status, response) = dx
            .call(
                "CreateDirectConnectGateway",
                create_gateway_body(name, *amazon_side_asn, tags),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        let created = response["directConnectGateway"].clone();
        assert!(
            is_minted_gateway_id(&created["directConnectGatewayId"]),
            "case {case:?}: expected a server-minted UUID id, got {:?}",
            created["directConnectGatewayId"]
        );
        assert_eq!(
            created["directConnectGatewayName"],
            json!(name),
            "case {case:?}: expected the requested name, got {:?}",
            created["directConnectGatewayName"]
        );
        assert_eq!(
            created["directConnectGatewayState"],
            json!("available"),
            "case {case:?}: a gateway is ready for use from the create on, got {:?}",
            created["directConnectGatewayState"]
        );
        assert_eq!(
            created["amazonSideAsn"],
            json!(amazon_side_asn.unwrap_or(DEFAULT_AMAZON_SIDE_ASN)),
            "case {case:?}: expected the requested ASN or the documented default, got {:?}",
            created["amazonSideAsn"]
        );
        assert_eq!(
            created["ownerAccount"],
            json!(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: expected the emulator account, got {:?}",
            created["ownerAccount"]
        );
        // A brand new gateway has no attachments, so no route prefixes are
        // allocated to it.
        assert_eq!(
            created["totalPrefixPoolAllocations"],
            json!(0),
            "case {case:?}: a gateway with no attachments allocates no prefixes, got {:?}",
            created["totalPrefixPoolAllocations"]
        );
        assert!(
            created["stateChangeError"].is_null(),
            "case {case:?}: no state advance has failed, got {:?}",
            created["stateChangeError"]
        );
        // The tag list is PRESENT whatever the create carried: the tags themselves
        // when it carried any, and an empty list when it carried none.
        assert_eq!(
            created["tags"],
            tag_list(tags),
            "case {case:?}: expected the tags the create carried, got {:?}",
            created["tags"]
        );

        // The stored record, read back: field for field what the create returned.
        let id = created["directConnectGatewayId"].as_str().unwrap_or_default();
        let (described, next_token) = describe(&dx, json!({"directConnectGatewayId": id})).await;
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: expected exactly the named gateway, got {described:?}"
        );
        assert!(
            next_token.is_null(),
            "case {case:?}: a single-item page is the last one, got {next_token:?}"
        );
        assert_eq!(
            described[0], created,
            "case {case:?}: the describe must render the stored record, got {:?}",
            described[0]
        );

        minted.push(id.to_string());
    }

    // Ids are unique, and a describe with no filter answers every stored one in
    // creation order.
    let (all, _) = describe(&dx, json!({})).await;
    let listed: Vec<String> = all
        .iter()
        .map(|gateway| {
            gateway["directConnectGatewayId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, minted,
        "expected every stored gateway in creation order, got {listed:?}"
    );
}

#[tokio::test]
async fn a_created_gateway_is_ready_from_the_create_on_and_no_read_moves_it() {
    // A create that allocates no network capacity leaves nothing to settle, so the
    // gateway is reported ready from the create response on and no later read has a
    // state to advance it to.
    let dx = Emulator::new();
    let id = gateway(&dx, "ready-from-the-create").await;

    let (described, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
    assert_eq!(
        described.len(),
        1,
        "case {id:?}: expected exactly the named gateway, got {described:?}"
    );
    assert_eq!(
        described[0]["directConnectGatewayState"],
        json!("available"),
        "case {id:?}: a gateway read straight back is ready for use, got {:?}",
        described[0]["directConnectGatewayState"]
    );
    assert_eq!(
        described[0]["directConnectGatewayId"],
        json!(&id),
        "case {id:?}: the read must answer the gateway the create minted, got {:?}",
        described[0]["directConnectGatewayId"]
    );

    // Reading it again answers the same record: a read changes nothing a later
    // caller sees.
    let (again, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
    assert_eq!(
        again[0], described[0],
        "case {id:?}: a second read must answer the same record, got {:?}",
        again[0]
    );
}

#[tokio::test]
async fn every_gateway_record_carries_exactly_the_modelled_member_set() {
    // RULE: a response carries exactly the members the model declares for the shape it
    // answers — never one the model does not declare — and ONE projection renders a
    // stored Direct Connect gateway, so create, describe, update and delete report that
    // same member set for the same gateway: its id, its name, its Amazon-side ASN, its
    // owning account, the state the record is actually in, the prefix-pool allocation
    // count and the tag list (present and empty when nothing tagged it).
    //
    // The rows differ in what the rule generalises over — the tag count (none, one,
    // several) and the name shape — since none of those changes the member set.
    let dx = Emulator::new();

    // (case, name, tags)
    let rows: &[ProjectionRow<'_>] = &[
        ("no tags at all", "projection-untagged", &[]),
        ("one tag", "Projection With Spaces", &[("env", "prod")]),
        (
            "several tags",
            "projection.tagged_9",
            &[("env", "test"), ("team", "network"), ("tier", "edge")],
        ),
    ];

    for (case, name, tags) in rows {
        let (status, response) = dx
            .call("CreateDirectConnectGateway", create_gateway_body(name, None, tags))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        let created = response["directConnectGateway"].clone();
        let id = created["directConnectGatewayId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_arn = direct_connect_arn("", "dx-gateway", &id);
        assert_eq!(
            rendered_members(&created),
            reported_gateway_members(),
            "case {case:?}: expected exactly the members the model declares for a gateway \
             (and no other), got {created}"
        );
        assert_eq!(
            (
                created["directConnectGatewayName"].clone(),
                created["ownerAccount"].clone(),
                created["tags"].clone(),
                created["directConnectGatewayState"].clone(),
                created["totalPrefixPoolAllocations"].clone(),
            ),
            (
                json!(name),
                json!(engine::EMULATOR_ACCOUNT_ID),
                tag_list(tags),
                json!("available"),
                json!(0),
            ),
            "case {case:?}: expected the create to report the gateway's name, owner, tag \
             list, state and prefix-pool count, got {created}"
        );

        // The read path renders the SAME projection, member for member.
        let (described, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
        assert_eq!(
            described,
            vec![created.clone()],
            "case {case:?}: the describe must render the same record as the create, got \
             {described:?}"
        );

        // A gateway record publishes no ARN of its own — the shape declares no member for
        // one — so the ARN a client BUILDS for it, in the form AWS names a Direct Connect
        // gateway under, is what this service resolves back to that same gateway, and a
        // client can tag or read tags by it.
        let (status, tagged) = dx
            .call("DescribeTags", json!({"resourceArns": [&expected_arn]}))
            .await;
        let expected_tags = json!([{"resourceArn": &expected_arn, "tags": tag_list(tags)}]);
        assert_eq!(
            (status, tagged["resourceTags"].clone()),
            (200, expected_tags),
            "case {case:?}: the ARN a client builds for this gateway must resolve to it, got \
             {status} with {tagged}"
        );

        // A rename changes the name and NOTHING else of the projection, and the read
        // path serves the new name.
        let renamed_to = format!("{name}-renamed");
        let (status, renamed) = dx
            .call(
                "UpdateDirectConnectGateway",
                json!({
                    "directConnectGatewayId": &id,
                    "newDirectConnectGatewayName": &renamed_to,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rename to succeed, got {status} with {renamed}"
        );
        let mut expected = created.clone();
        expected["directConnectGatewayName"] = json!(&renamed_to);
        assert_eq!(
            renamed["directConnectGateway"], expected,
            "case {case:?}: expected the rename to change only the name, got {renamed}"
        );
        let (described, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
        assert_eq!(
            described,
            vec![expected.clone()],
            "case {case:?}: the read path must serve the renamed record, got {described:?}"
        );

        // The delete answers the same projection, in the state the delete put the
        // gateway in.
        let (status, deleted) = dx
            .call(
                "DeleteDirectConnectGateway",
                json!({"directConnectGatewayId": &id}),
            )
            .await;
        let mut torn_down = expected;
        torn_down["directConnectGatewayState"] = json!("deleting");
        assert_eq!(
            (status, deleted["directConnectGateway"].clone()),
            (200, torn_down),
            "case {case:?}: expected the delete to answer the same record deleting, got \
             {status} with {deleted}"
        );
    }
}

#[tokio::test]
async fn an_amazon_side_asn_outside_the_documented_private_ranges_is_refused() {
    let dx = Emulator::new();

    // One value past each published bound of each documented private range, plus
    // values far outside them.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for (low, high) in PRIVATE_AMAZON_SIDE_ASN_RANGES {
        rows.push((format!("one below the low bound {low}"), json!(low - 1)));
        rows.push((format!("one above the high bound {high}"), json!(high + 1)));
    }
    rows.push(("a public two-byte ASN".to_string(), json!(7_224)));
    rows.push(("zero".to_string(), json!(0)));
    rows.push(("negative".to_string(), json!(-1)));

    for (case, asn) in &rows {
        let mut body = Map::new();
        body.insert("directConnectGatewayName".to_string(), json!("asn-check"));
        body.insert("amazonSideAsn".to_string(), asn.clone());
        let (status, response) = dx
            .call("CreateDirectConnectGateway", Value::Object(body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
        // Every client fault of this service shares one error shape and one 400
        // status, so the SENTENCE is the whole discriminator: fixed text, with the
        // rejected value — and nothing else of the request — in front of it. The
        // member is not named and the ranges are not restated.
        let expected = format!("{asn} is not a valid value for Amazon Side ASN");
        assert_eq!(
            error_message(&response),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&response)
        );
    }

    // A refused create stores nothing.
    let (stored, _) = describe(&dx, json!({})).await;
    assert!(
        stored.is_empty(),
        "a refused create must store no gateway, got {stored:?}"
    );
}

#[tokio::test]
async fn a_request_that_names_nothing_is_refused() {
    let dx = Emulator::new();

    // (case, body, the error code expected)
    let rows: &[(&str, Value, &str)] = &[
        (
            "an empty gateway name",
            json!({"directConnectGatewayName": ""}),
            "DirectConnectClientException",
        ),
        (
            "a gateway name of only whitespace",
            json!({"directConnectGatewayName": "   "}),
            "DirectConnectClientException",
        ),
        (
            "a pagination token naming nothing",
            json!({"nextToken": "00000000-0000-4000-8000-000000000000"}),
            "DirectConnectClientException",
        ),
    ];

    for (case, body, expected) in rows {
        let target = if body.get("directConnectGatewayName").is_some() {
            "CreateDirectConnectGateway"
        } else {
            "DescribeDirectConnectGateways"
        };
        let (status, response) = dx.call(target, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains(expected),
            "case {case:?}: expected {expected}, got {:?}",
            error_code(&response)
        );
    }
}

#[tokio::test]
async fn a_gateway_id_filter_that_selects_nothing_answers_an_empty_page() {
    let dx = Emulator::new();

    // Two gateways stored, so a filter that selects nothing is answering about a
    // store that HOLDS something.
    let stored = gateway(&dx, "selection-one").await;
    let other = gateway(&dx, "selection-two").await;

    // And one the store holds in the TERMINAL `deleted` state, which
    // `DirectConnectGatewayState` models as a state a gateway is reported in: such a
    // tombstone is still selected by its own id, so a client that deleted a gateway
    // can tell "it is gone" from "it was never there". The record is put in that
    // state directly through the shared engine — the store the handler reads — so
    // this row does not wait on the span `DeleteDirectConnectGateway` owes.
    let deleted = gateway(&dx, "selection-deleted").await;
    dx.engine()
        .state::<ComAmazonawsDirectconnectOvertureserviceState>()
        .lock()
        .gateways
        .get_mut(&deleted)
        .expect("the gateway just created is in the store")
        .direct_connect_gateway_state = "deleted".to_string();

    // (case, the directConnectGatewayId filtered by, the ids the page must hold).
    // `DirectConnectGatewayId` declares no constraint
    // (`smithy_query(query="shape",shape="DirectConnectGatewayId")`), so every row
    // below is a well-formed request; they differ along the FORM of the value — a
    // UUID naming no gateway, a value in another resource's id form, a value
    // carrying a space, the empty string — and along what the store holds for it: a
    // gateway that is deleted, and a gateway that is there.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "a UUID naming no gateway",
            "00000000-0000-4000-8000-000000000000".to_string(),
            vec![],
        ),
        (
            "a value in no gateway-id form at all",
            "dxgw-nothing-here".to_string(),
            vec![],
        ),
        ("a value carrying a space", "not a gateway".to_string(), vec![]),
        ("the empty string", String::new(), vec![]),
        (
            "the id of a deleted gateway",
            deleted.clone(),
            vec![deleted.clone()],
        ),
        (
            "the id of a stored gateway",
            stored.clone(),
            vec![stored.clone()],
        ),
    ];

    for (case, filter, expected) in &rows {
        for body in [
            json!({"directConnectGatewayId": filter}),
            // The same selection asked for one page at a time: a window over
            // nothing is still nothing, and never a refusal.
            json!({"directConnectGatewayId": filter, "maxResults": 1}),
        ] {
            let (status, response) = dx.call("DescribeDirectConnectGateways", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?} as {body}: expected 200, got {status} with {response}"
            );
            // The member is SERIALIZED even when it holds nothing: a client reads
            // an empty list as "absent", and an omitted member as "no answer".
            let Some(listed) = response["directConnectGateways"].as_array() else {
                panic!(
                    "case {case:?} as {body}: directConnectGateways must be present, got {response}"
                );
            };
            let ids: Vec<String> = listed
                .iter()
                .map(|gateway| {
                    gateway["directConnectGatewayId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                &ids, expected,
                "case {case:?} as {body}: expected {expected:?}, got {ids:?}"
            );
            assert!(
                response["nextToken"].is_null(),
                "case {case:?} as {body}: one page holds the whole selection, so no token, got {:?}",
                response["nextToken"]
            );
        }
    }

    // The filter NARROWS the listing rather than replacing it: unfiltered, every
    // gateway the store holds is still answered, the tombstone among them.
    let (all, _) = describe(&dx, json!({})).await;
    let listed: Vec<String> = all
        .iter()
        .map(|gateway| {
            gateway["directConnectGatewayId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed,
        vec![stored, other, deleted],
        "expected every gateway the store holds, got {listed:?}"
    );
}

#[tokio::test]
async fn gateways_page_with_max_results_and_next_token() {
    let dx = Emulator::new();
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(gateway(&dx, &format!("page-{n}")).await);
    }

    // (case, page size, expected page lengths)
    let rows: &[(&str, i64, &[usize])] = &[
        ("one at a time", 1, &[1, 1, 1, 1, 1]),
        ("two at a time", 2, &[2, 2, 1]),
        ("exactly the whole set", 5, &[5]),
        ("more than the whole set", 50, &[5]),
    ];

    for (case, page_size, expected_lengths) in rows {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        for (index, expected_length) in expected_lengths.iter().enumerate() {
            let mut body = Map::new();
            body.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                body.insert("nextToken".to_string(), json!(token));
            }
            let (page, next_token) = describe(&dx, Value::Object(body)).await;
            assert_eq!(
                page.len(),
                *expected_length,
                "case {case:?} page {index}: expected {expected_length} items, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|gateway| {
                gateway["directConnectGatewayId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            let is_last = index + 1 == expected_lengths.len();
            assert_eq!(
                next_token.is_null(),
                is_last,
                "case {case:?} page {index}: nextToken must be null only on the last page, got {next_token:?}"
            );
            token = next_token.as_str().map(ToString::to_string);
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must visit every stored gateway exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_max_results_above_the_documented_ceiling_is_capped() {
    let dx = Emulator::new();
    // One more than the documented ceiling, so a capped page is observable.
    let total = usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive") + 1;
    for n in 0..total {
        gateway(&dx, &format!("capped-{n}")).await;
    }

    // "If MaxResults is given a value larger than 100, only 100 results are
    // returned."
    for asked in [MAX_RESULT_SET_SIZE + 1, 250, 100_000] {
        let (page, next_token) = describe(&dx, json!({"maxResults": asked})).await;
        assert_eq!(
            page.len(),
            usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive"),
            "case {asked:?}: a page is capped at {MAX_RESULT_SET_SIZE}, got {}",
            page.len()
        );
        assert!(
            next_token.is_string(),
            "case {asked:?}: a capped page is not the last one, got {next_token:?}"
        );

        let (rest, rest_token) =
            describe(&dx, json!({"maxResults": asked, "nextToken": next_token})).await;
        assert_eq!(
            rest.len(),
            total - usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive"),
            "case {asked:?}: the next page holds what the cap left, got {}",
            rest.len()
        );
        assert!(
            rest_token.is_null(),
            "case {asked:?}: the last page carries no token, got {rest_token:?}"
        );
    }
}

#[tokio::test]
async fn a_private_virtual_interface_terminates_on_a_stored_gateway() {
    let dx = Emulator::new();
    let (status, connection) = dx
        .call(
            "CreateConnection",
            json!({"connectionName": "gw-host", "location": "EqDC2", "bandwidth": "1Gbps"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConnection to succeed, got {status} with {connection}"
    );
    let connection_id = connection["connectionId"].as_str().unwrap_or_default();

    let asn_rows = [
        PRIVATE_AMAZON_SIDE_ASN_RANGES[0].0,
        PRIVATE_AMAZON_SIDE_ASN_RANGES[1].1,
    ];
    for (index, amazon_side_asn) in asn_rows.into_iter().enumerate() {
        let (status, created) = dx
            .call(
                "CreateDirectConnectGateway",
                create_gateway_body(&format!("vif-gateway-{index}"), Some(amazon_side_asn), &[]),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {amazon_side_asn:?}: expected the gateway create to succeed, got {status} with {created}"
        );
        let gateway_id = created["directConnectGateway"]["directConnectGatewayId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, vif) = dx
            .call(
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": connection_id,
                    "newPrivateVirtualInterface": {
                        "virtualInterfaceName": format!("vif-{index}"),
                        "vlan": 200 + i64::try_from(index).unwrap_or(0),
                        "asn": 65_000,
                        "directConnectGatewayId": &gateway_id,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {amazon_side_asn:?}: expected the virtual interface create to succeed, got {status} with {vif}"
        );
        assert_eq!(
            vif["directConnectGatewayId"],
            json!(&gateway_id),
            "case {amazon_side_asn:?}: expected the gateway the caller named, got {:?}",
            vif["directConnectGatewayId"]
        );
        // The Amazon side of a peering that terminates on a Direct Connect
        // gateway is that gateway's ASN.
        assert_eq!(
            vif["amazonSideAsn"],
            json!(amazon_side_asn),
            "case {amazon_side_asn:?}: expected the gateway's ASN, got {:?}",
            vif["amazonSideAsn"]
        );
        assert!(
            vif["virtualGatewayId"].is_null(),
            "case {amazon_side_asn:?}: a gateway-terminated interface names no virtual private gateway, got {:?}",
            vif["virtualGatewayId"]
        );
    }

    // (case, the newPrivateVirtualInterface members under test)
    let refusals: &[(&str, Value)] = &[
        (
            "a Direct Connect gateway id naming nothing",
            json!({
                "virtualInterfaceName": "unknown-gw",
                "vlan": 301,
                "asn": 65_000,
                "directConnectGatewayId": "00000000-0000-4000-8000-000000000000",
            }),
        ),
        (
            "both gateway kinds at once",
            json!({
                "virtualInterfaceName": "both-gw",
                "vlan": 302,
                "asn": 65_000,
                "directConnectGatewayId": "00000000-0000-4000-8000-000000000000",
                "virtualGatewayId": "vgw-0123456789abcdef0",
            }),
        ),
        (
            "neither gateway kind",
            json!({
                "virtualInterfaceName": "no-gw",
                "vlan": 303,
                "asn": 65_000,
            }),
        ),
    ];

    for (case, requested) in refusals {
        let (status, response) = dx
            .call(
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": connection_id,
                    "newPrivateVirtualInterface": requested.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
    }
}

/// The associations `DescribeDirectConnectGatewayAssociations` answers for `body`.
async fn associations(dx: &Emulator, case: &str, body: Value) -> Vec<Value> {
    let (status, response) = dx
        .call("DescribeDirectConnectGatewayAssociations", body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the describe to be served, got {status} with {response}"
    );
    response["directConnectGatewayAssociations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rule: the gateway a Direct Connect gateway association is made on is a resource of
// the service that OWNS virtual private gateways and transit gateways, resolved there
// before anything is stored. So Direct Connect alone refuses a request that names no
// Direct Connect gateway, names no gateway to associate, names an identifier that is
// no gateway identifier at all, or names more prefixes than this emulator advertises
// per association — and with no owner of gateways on this engine to ask, every
// gateway identifier resolves to nothing, which is fail-closed: nothing is stored and
// no association is reported.
//
// The ACCEPTED side needs that owner beside this service on one engine, so it is
// driven end to end in
// `core/router/tests/all/direct_connect_gateway_association_network.rs`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_naming_no_gateway_the_account_holds_is_refused_and_stores_nothing() {
    let dx = Emulator::new();
    let gateway_id = gateway(&dx, "association-refusals").await;
    // One prefix past the ceiling this emulator advertises per association.
    let too_many: Vec<Value> = (0..=MAX_ALLOWED_PREFIXES_PER_ASSOCIATION)
        .map(|n| json!({"cidr": format!("10.{}.{}.0/24", n / 256, n % 256)}))
        .collect();

    // (case, the request body, the text the refusal must name)
    let rows: Vec<(String, Value, String)> = vec![
        (
            "a Direct Connect gateway id naming nothing, beside a gateway no owner \
             answers for: the gateway to associate is resolved first"
                .to_string(),
            json!({
                "directConnectGatewayId": "00000000-0000-4000-8000-000000000000",
                "gatewayId": "vgw-0123456789abcdef0",
            }),
            "vgw-0123456789abcdef0".to_string(),
        ),
        (
            "no gateway to associate at all".to_string(),
            json!({"directConnectGatewayId": &gateway_id}),
            "virtualGatewayId".to_string(),
        ),
        (
            "gatewayId and virtualGatewayId naming different gateways".to_string(),
            json!({
                "directConnectGatewayId": &gateway_id,
                "gatewayId": "vgw-0123456789abcdef0",
                "virtualGatewayId": "vgw-0f1e2d3c4b5a69780",
            }),
            "virtualGatewayId".to_string(),
        ),
        (
            "an identifier that is no gateway identifier".to_string(),
            json!({"directConnectGatewayId": &gateway_id, "gatewayId": "potato"}),
            "potato".to_string(),
        ),
        (
            "a virtual private gateway no owner answers for".to_string(),
            json!({"directConnectGatewayId": &gateway_id, "gatewayId": "vgw-0123456789abcdef0"}),
            "vgw-0123456789abcdef0".to_string(),
        ),
        (
            "a transit gateway no owner answers for".to_string(),
            json!({
                "directConnectGatewayId": &gateway_id,
                "gatewayId": "tgw-0f1e2d3c4b5a69780",
                "addAllowedPrefixesToDirectConnectGateway": [{"cidr": "10.0.0.0/16"}],
            }),
            "tgw-0f1e2d3c4b5a69780".to_string(),
        ),
        (
            "one prefix past the ceiling".to_string(),
            json!({
                "directConnectGatewayId": &gateway_id,
                "gatewayId": "tgw-0f1e2d3c4b5a69780",
                "addAllowedPrefixesToDirectConnectGateway": too_many,
            }),
            MAX_ALLOWED_PREFIXES_PER_ASSOCIATION.to_string(),
        ),
        (
            "prefixes named for a virtual private gateway, whose prefixes are its \
             attached VPCs' blocks"
                .to_string(),
            json!({
                "directConnectGatewayId": &gateway_id,
                "gatewayId": "vgw-0123456789abcdef0",
                "addAllowedPrefixesToDirectConnectGateway": [{"cidr": "10.0.0.0/16"}],
            }),
            "addAllowedPrefixesToDirectConnectGateway".to_string(),
        ),
    ];

    for (case, body, named) in &rows {
        let (status, response) = dx
            .call("CreateDirectConnectGatewayAssociation", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(named.as_str()),
            "case {case:?}: expected the refusal to name {named:?}, got {response}"
        );
        // A refused create stores nothing, so no association is reported for either
        // gateway the rows name.
        for selector in [
            json!({"directConnectGatewayId": &gateway_id}),
            json!({"directConnectGatewayId": "00000000-0000-4000-8000-000000000000"}),
        ] {
            let stored = associations(&dx, case, selector).await;
            assert!(
                stored.is_empty(),
                "case {case:?}: a refused create must store no association, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_that_names_no_stored_association_is_refused() {
    let dx = Emulator::new();
    let gateway_id = gateway(&dx, "delete-refusals").await;

    // (case, the request body)
    let rows: Vec<(&str, Value)> = vec![
        (
            "an associationId naming nothing",
            json!({"associationId": "00000000-0000-4000-8000-000000000000"}),
            ),
        (
            "a Direct Connect gateway and a gateway with no association between them",
            json!({
                "directConnectGatewayId": &gateway_id,
                "virtualGatewayId": "vgw-0123456789abcdef0",
            }),
        ),
        (
            "neither an associationId nor both gateway ids",
            json!({"directConnectGatewayId": &gateway_id}),
        ),
        ("nothing at all", json!({})),
    ];

    for (case, body) in &rows {
        let (status, response) = dx
            .call("DeleteDirectConnectGatewayAssociation", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
    }

    // What this listing does with a selector, and with none, is the next test's
    // subject: `a_listing_of_associations_that_selects_by_nothing_is_refused_whatever_else_it_carries`.
}

// ---------------------------------------------------------------------------
// Rule: an operation published only in forms that each name a selector refuses a
// request carrying none of them, with the sentence it states for that refusal — a
// windowing member is not a selector, so it never makes such a request answerable,
// and a selector that IS carried only filters, whatever its value names.
// ---------------------------------------------------------------------------

/// The sentence `DescribeDirectConnectGatewayAssociations` states for a request
/// that selects by nothing. FIXED text: no member, value or identifier of the
/// request appears in it.
const SELECTOR_REQUIRED: &str =
    "Either Direct Connect Gateway ID or Associated Gateway ID or Association ID must be set";

#[tokio::test]
async fn a_listing_of_associations_that_selects_by_nothing_is_refused_whatever_else_it_carries() {
    let dx = Emulator::new();
    let gateway_id = gateway(&dx, "selector-required").await;

    // (case, the request body). Every row carries NO selector and differs in which
    // windowing members it carries and what they hold — including each bound of
    // `maxResults` ("If MaxResults is given a value larger than 100, only 100
    // results are returned") and one value past it.
    let refused: Vec<(&str, Value)> = vec![
        ("no member at all", json!({})),
        ("the smallest page", json!({"maxResults": 1})),
        (
            "the largest page the member documents",
            json!({"maxResults": MAX_RESULT_SET_SIZE}),
        ),
        (
            "one past the largest page the member documents",
            json!({"maxResults": MAX_RESULT_SET_SIZE + 1}),
        ),
        ("a page size of zero", json!({"maxResults": 0})),
        (
            "a pagination token",
            json!({"nextToken": "00000000-0000-4000-8000-000000000000"}),
        ),
        (
            "a page size and a pagination token",
            json!({"maxResults": 7, "nextToken": "not-a-token"}),
        ),
    ];

    for (case, body) in &refused {
        let (status, response) = dx
            .call("DescribeDirectConnectGatewayAssociations", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
        assert_eq!(
            error_message(&response),
            SELECTOR_REQUIRED,
            "case {case:?}: expected the sentence this listing states, got {response}"
        );
    }

    // And a request that DOES carry one selector is served, under every window the
    // refused rows carried: a selector is a filter, so a value naming no association
    // is an empty page and not a client fault.
    let selected: Vec<(&str, Value)> = vec![
        (
            "the stored Direct Connect gateway",
            json!({"directConnectGatewayId": &gateway_id}),
        ),
        (
            "a Direct Connect gateway id naming nothing",
            json!({"directConnectGatewayId": "00000000-0000-4000-8000-000000000000"}),
        ),
        (
            "an associated virtual private gateway",
            json!({"associatedGatewayId": "vgw-0123456789abcdef0"}),
        ),
        (
            "an associated transit gateway",
            json!({"associatedGatewayId": "tgw-0f1e2d3c4b5a69780"}),
        ),
        (
            "the older spelling of the associated gateway",
            json!({"virtualGatewayId": "vgw-0123456789abcdef0"}),
        ),
        (
            "an association id",
            json!({"associationId": "00000000-0000-4000-8000-000000000000"}),
        ),
        (
            "a selector holding nothing at all",
            json!({"directConnectGatewayId": ""}),
        ),
        (
            "a selector holding only space",
            json!({"associationId": "   "}),
        ),
    ];

    for (selector_case, selector) in &selected {
        for window in [
            json!({}),
            json!({"maxResults": 1}),
            json!({"maxResults": MAX_RESULT_SET_SIZE + 1}),
        ] {
            let mut body = selector.as_object().cloned().unwrap_or_default();
            for (member, value) in window.as_object().into_iter().flatten() {
                body.insert(member.clone(), value.clone());
            }
            let case = format!("{selector_case} windowed by {window}");
            let page = associations(&dx, &case, Value::Object(body)).await;
            assert!(
                page.is_empty(),
                "case {case:?}: expected an empty page over a store holding no association, got \
                 {page:?}"
            );
        }
    }

    // A `nextToken` is not a selector either way: beside one it names a page of the
    // selected set, so a token naming nothing in that set is the caller's error.
    let (status, response) = dx
        .call(
            "DescribeDirectConnectGatewayAssociations",
            json!({
                "directConnectGatewayId": &gateway_id,
                "nextToken": "00000000-0000-4000-8000-000000000000",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a pagination token naming nothing in the selected set\": expected a refusal, got \
         {status} with {response}"
    );
    assert!(
        error_code(&response).contains("DirectConnectClientException"),
        "case \"a pagination token naming nothing in the selected set\": expected \
         DirectConnectClientException, got {:?}",
        error_code(&response)
    );
}

// ---------------------------------------------------------------------------
// The resiliency group: the other Region-less resource of this service
// ---------------------------------------------------------------------------

/// How long this half of the suite is willing to wait for a state the CLOCK makes due,
/// and how often it looks again.
///
/// Generous on purpose: what is waited FOR is the CONDITION — the state the service
/// reports for the resource — rather than a span of wall clock, so a loaded or a faster
/// host cannot decide the outcome, and the wait ends as soon as the state appears.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
/// See [`SETTLE_DEADLINE`].
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(20);

/// The ARN form every resource of this service publishes:
/// `arn:aws:directconnect:<region>:<account>:<path>/<id>`, where `region` is EMPTY for a
/// resource that is global (a Direct Connect gateway, a resiliency group) — which is what
/// `ResiliencyGroupArn`'s own pattern states
/// (`^arn:(aws|aws-cn|aws-us-gov):directconnect::[0-9]{12}:dx-resiliency-group/\
/// dxrg-[0-9a-z]{17}$`). Written out here rather than taken from the service, so the
/// FORM is what the test pins.
fn direct_connect_arn(region: &str, path: &str, id: &str) -> String {
    format!("arn:aws:directconnect:{region}:000000000000:{path}/{id}")
}

/// Read one resiliency group again until that service reports it in `state`, and answer the
/// record it reported there.
///
/// The ONE wait of the group half of this suite: a state the clock owes is due at an
/// INSTANT the service records, so what is waited for is that state appearing, under
/// [`SETTLE_DEADLINE`]. A wait that runs out names the last read, so a group that settled
/// somewhere else says where.
///
/// The pause between looks is `std::thread::sleep` rather than `tokio::time::sleep`: the
/// workspace enables tokio's `macros`/`rt` features only, so `tokio::time` is not
/// available here, and nothing else is pending on the test's runtime.
async fn group_in_state(dx: &Emulator, case: &str, group_id: &str, state: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, held) = get_group(dx, group_id).await;
        if status == 200 && held["state"] == json!(state) {
            return held;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the group {group_id} to reach {state}, last read {status} \
             with {held}"
        );
        std::thread::sleep(LOOK_AGAIN_AFTER);
    }
}

/// The same wait for one Direct Connect gateway, read through the listing its own id
/// narrows, and answered with the record that listing reported in `state`.
async fn gateway_in_state(dx: &Emulator, case: &str, gateway_id: &str, state: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (listed, _) = describe(dx, json!({"directConnectGatewayId": gateway_id})).await;
        let held = listed
            .iter()
            .find(|held| held["directConnectGatewayState"] == json!(state))
            .cloned();
        if let Some(held) = held {
            return held;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the gateway {gateway_id} to reach {state}, last read \
             {listed:?}"
        );
        std::thread::sleep(LOOK_AGAIN_AFTER);
    }
}

/// One row of the resiliency-group create table: case, name, `intendedResiliencyModel`,
/// tags.
type GroupRow<'a> = (&'a str, String, &'a str, &'a [(&'a str, &'a str)]);

/// One row of the tag-set refusal table: case, the tags the create carries, and the
/// modelled error code it must be refused with — `None` for a set the service accepts.
type TagSetRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, Option<&'a str>);

/// A `CreateResiliencyGroup` body.
fn create_group_body(
    name: &str,
    model: &str,
    tags: &[(&str, &str)],
    client_token: Option<&str>,
) -> Value {
    let mut body = Map::new();
    body.insert("resiliencyGroupName".to_string(), json!(name));
    body.insert("intendedResiliencyModel".to_string(), json!(model));
    if let Some(token) = client_token {
        body.insert("clientToken".to_string(), json!(token));
    }
    if !tags.is_empty() {
        body.insert(
            "tags".to_string(),
            Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// Create one resiliency group and return its id, failing the test if it was refused.
async fn group(dx: &Emulator, name: &str, model: &str) -> String {
    let (status, body) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body(name, model, &[], None),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateResiliencyGroup to succeed, got {status} with {body}"
    );
    body["resiliencyGroup"]["resiliencyGroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `GetResiliencyGroup`'s own record of one group.
async fn get_group(dx: &Emulator, group_id: &str) -> (u16, Value) {
    let (status, response) = dx
        .call(
            "GetResiliencyGroup",
            json!({"resiliencyGroupId": group_id}),
        )
        .await;
    (status, response["resiliencyGroup"].clone())
}

/// The groups `ListResiliencyGroups` answers for `body`, with the response's `nextToken`.
async fn list_groups(dx: &Emulator, body: Value) -> (Vec<Value>, Value) {
    let (status, response) = dx.call("ListResiliencyGroups", body.clone()).await;
    assert_eq!(
        status, 200,
        "expected ListResiliencyGroups 200 for {body}, got {status} with {response}"
    );
    (
        response["items"].as_array().cloned().unwrap_or_default(),
        response["nextToken"].clone(),
    )
}

/// Whether `id` is a server-minted resiliency group id: the `dxrg-` prefix and
/// [`RESILIENCY_GROUP_ID_DIGITS`] characters of the lowercase alphabet
/// `ResiliencyGroupArn`'s own pattern admits.
fn is_minted_group_id(id: &Value) -> bool {
    let Some(id) = id.as_str() else { return false };
    let Some(label) = id.strip_prefix(&format!("{RESILIENCY_GROUP_ID_PREFIX}-")) else {
        return false;
    };
    label.chars().count() == RESILIENCY_GROUP_ID_DIGITS
        && label
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// `tags` as the `TagList` a stored record reports them in.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

#[tokio::test]
async fn a_created_resiliency_group_is_minted_stored_and_read_back_field_for_field() {
    // RULE: the create MINTS the group's identity and stores the declaration, and every
    // later read renders the SAME stored record — so a create followed by a read answers
    // the same bytes, and the account's own listing reports the group too.
    //
    // The rows span what the rule generalises over: which of the three `ResiliencyModel`
    // values the group is created to meet (the whole set
    // `smithy_query(query="shape",shape="ResiliencyModel")` states), how many tags the
    // declaration carries (none, one, two) and how long the name is (one character, a
    // middling name, and the longest `ResiliencyGroupName`'s `@length 1..255` admits).
    let rows: Vec<GroupRow> = vec![
        (
            "the maximum-resiliency model, no tags",
            "a".to_string(),
            "maximum-resiliency",
            &[],
        ),
        (
            "the high-resiliency model, one tag",
            "group-high_1".to_string(),
            "high-resiliency",
            &[("tier", "high")],
        ),
        (
            "the basic-resiliency model, two tags, the longest name admitted",
            "b".repeat(MAX_RESILIENCY_GROUP_NAME_LEN),
            "basic-resiliency",
            &[("tier", "basic"), ("owner", "platform")],
        ),
    ];

    for (case, name, model, tags) in rows {
        let dx = Emulator::new();
        let (status, created) = dx
            .call(
                "CreateResiliencyGroup",
                create_group_body(&name, model, tags, None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {created}"
        );
        let held_group = created["resiliencyGroup"].clone();
        let group_id = held_group["resiliencyGroupId"].clone();
        assert!(
            is_minted_group_id(&group_id),
            "case {case:?}: expected a minted {RESILIENCY_GROUP_ID_PREFIX}- id, got {group_id:?}"
        );
        let expected_arn = direct_connect_arn(
            "",
            "dx-resiliency-group",
            group_id.as_str().unwrap_or_default(),
        );
        assert_eq!(
            (
                held_group["resiliencyGroupArn"].clone(),
                held_group["resiliencyGroupName"].clone(),
                held_group["resiliencyGroupType"].clone(),
                held_group["ownerAccount"].clone(),
                held_group["state"].clone(),
            ),
            (
                json!(expected_arn),
                json!(name),
                json!(RESILIENCY_GROUP_TYPE_MANAGED),
                json!(engine::emulator_account_id()),
                json!(RESILIENCY_GROUP_STATE_PENDING),
            ),
            "case {case:?}: expected the declared group minted pending, got {held_group}"
        );
        // `TagList` is `@length 1..`, so a group nobody tagged publishes no list at all
        // rather than an empty one.
        if tags.is_empty() {
            assert!(
                held_group["tags"].is_null(),
                "case {case:?}: a group with no tags publishes no list, got {:?}",
                held_group["tags"]
            );
        } else {
            assert_eq!(
                held_group["tags"],
                tag_list(tags),
                "case {case:?}: expected the declared tags, got {:?}",
                held_group["tags"]
            );
        }

        // The READ renders the same record, and the listing carries its summary — which
        // is the record LESS the tags, the one member `ResiliencyGroupSummary` does not
        // publish.
        let (status, held) = get_group(&dx, group_id.as_str().unwrap_or_default()).await;
        assert_eq!(
            (status, held.clone()),
            (200, held_group.clone()),
            "case {case:?}: expected the read to answer the stored group, got {status} with \
             {held}"
        );
        let (listed, token) = list_groups(&dx, json!({})).await;
        let mut summary = held_group.clone();
        if let Some(object) = summary.as_object_mut() {
            object.remove("tags");
        }
        assert_eq!(
            (listed, token),
            (vec![summary], Value::Null),
            "case {case:?}: expected the account's listing to report the group's summary"
        );

        // `pending` is where the create leaves the group; `available` is what the clock
        // owes from there, so the read is looked at again until that state appears.
        let settled = group_in_state(
            &dx,
            case,
            group_id.as_str().unwrap_or_default(),
            RESILIENCY_GROUP_STATE_AVAILABLE,
        )
        .await;
        assert_eq!(
            settled["resiliencyGroupId"],
            group_id,
            "case {case:?}: expected the settled group to be the one created, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_resiliency_group_create_is_refused_for_a_tag_set_no_resource_can_carry() {
    // RULE: the tag set a create carries is held to the two things AWS states about one —
    // at most `MAX_TAGS_PER_RESOURCE` tags per Direct Connect resource, and one value per
    // key — and the modelled exception for each is its own.
    //
    // The rows differ along which bound is crossed and by how much: the count AT the
    // ceiling (accepted), one past it, a key repeated twice, and a key repeated inside an
    // otherwise legal set.
    let at_the_ceiling: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();
    let past_the_ceiling: Vec<(String, String)> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();
    fn borrowed(tags: &[(String, String)]) -> Vec<(&str, &str)> {
        tags.iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    let rows: Vec<TagSetRow> = vec![
        (
            "a tag set exactly at the ceiling",
            borrowed(&at_the_ceiling),
            None,
        ),
        (
            "one tag past the ceiling",
            borrowed(&past_the_ceiling),
            Some("TooManyTagsException"),
        ),
        (
            "one key stated twice",
            vec![("tier", "basic"), ("tier", "high")],
            Some("DuplicateTagKeysException"),
        ),
        (
            "a repeated key inside an otherwise legal set",
            vec![("owner", "platform"), ("tier", "basic"), ("owner", "net")],
            Some("DuplicateTagKeysException"),
        ),
    ];

    for (case, tags, refused) in rows {
        let dx = Emulator::new();
        let (status, response) = dx
            .call(
                "CreateResiliencyGroup",
                create_group_body("tagged-group", "basic-resiliency", &tags, None),
            )
            .await;
        match refused {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the set to be accepted, got {status} with {response}"
                );
                let (held, _) = list_groups(&dx, json!({})).await;
                assert_eq!(
                    held.len(),
                    1,
                    "case {case:?}: expected the accepted group to be held, got {held:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&response)),
                    (400, expected.to_string()),
                    "case {case:?}: expected {expected}, got {status} with {response}"
                );
                let (held, _) = list_groups(&dx, json!({})).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: a refused create must store nothing, got {held:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_repeated_create_under_one_client_token_answers_the_group_it_already_minted() {
    // `clientToken` is "a unique, case-sensitive identifier that you provide to ensure the
    // idempotency of the request", so the same request repeated under it answers THAT
    // group rather than minting a second one — while a create carrying no token, or
    // another token, asks for a group of its own, the name being no identity.
    let dx = Emulator::new();
    let token = "11111111-2222-3333-4444-555555555555";
    let (status, first) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body("idempotent-group", "basic-resiliency", &[], Some(token)),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the first create to succeed, got {status} with {first}"
    );
    let (status, again) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body("idempotent-group", "basic-resiliency", &[], Some(token)),
        )
        .await;
    assert_eq!(
        (status, again.clone()),
        (200, first.clone()),
        "expected the repeat under one token to answer the same group, got {status} with {again}"
    );
    let (held, _) = list_groups(&dx, json!({})).await;
    assert_eq!(
        held.len(),
        1,
        "expected one group for one token, got {held:?}"
    );

    // Another token, and no token at all, each ask for a group of their own.
    for (case, carried) in [
        ("another token", Some("99999999-8888-7777-6666-555555555555")),
        ("no token at all", None),
    ] {
        let (status, other) = dx
            .call(
                "CreateResiliencyGroup",
                create_group_body("idempotent-group", "basic-resiliency", &[], carried),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a group of its own, got {status} with {other}"
        );
        assert_ne!(
            other["resiliencyGroup"]["resiliencyGroupId"],
            first["resiliencyGroup"]["resiliencyGroupId"],
            "case {case:?}: expected a second group, got the first one back"
        );
    }
    let (held, _) = list_groups(&dx, json!({})).await;
    assert_eq!(
        held.len(),
        3,
        "expected one group per distinct request, got {held:?}"
    );
}

#[tokio::test]
async fn a_resiliency_group_call_naming_an_id_the_account_holds_nothing_under_is_refused() {
    // RULE: `GetResiliencyGroup`, `UpdateResiliencyGroup` and `DeleteResiliencyGroup` each
    // RESOLVE their group rather than selecting one, so an id the account holds no group
    // under is a client fault — this service's one client exception, worded the way this
    // crate words every unresolved identifier.
    //
    // The rows differ along the operation and along the FORM of the id: a well-formed id
    // naming no group, and one in the uppercase spelling `ResiliencyGroupId`'s pattern
    // also admits.
    let dx = Emulator::new();
    let held = group(&dx, "resolved-group", "basic-resiliency").await;
    for unknown in ["dxrg-00000000000000000", "DXRG-00000000000000000"] {
        for operation in [
            "GetResiliencyGroup",
            "UpdateResiliencyGroup",
            "DeleteResiliencyGroup",
        ] {
            let mut body = Map::new();
            body.insert("resiliencyGroupId".to_string(), json!(unknown));
            if operation == "UpdateResiliencyGroup" {
                body.insert("resiliencyGroupName".to_string(), json!("renamed"));
            }
            let (status, response) = dx.call(operation, Value::Object(body)).await;
            assert_eq!(
                (status, error_code(&response), error_message(&response)),
                (
                    400,
                    "DirectConnectClientException".to_string(),
                    format!("Resiliency group ID {unknown} does not exist"),
                ),
                "case {operation} on {unknown:?}: expected the unresolved-id refusal, got \
                 {status} with {response}"
            );
        }
    }

    // A pagination token naming nothing in the listing IS a refusal too, while the group
    // the account does hold is still resolved.
    let (status, response) = dx
        .call(
            "ListResiliencyGroups",
            json!({"nextToken": "dxrg-00000000000000000"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&response)),
        (400, "DirectConnectClientException".to_string()),
        "case \"a pagination token naming nothing\": expected a refusal, got {status} with \
         {response}"
    );
    let (status, resolved) = get_group(&dx, &held).await;
    assert_eq!(
        (status, resolved["resiliencyGroupId"].clone()),
        (200, json!(held)),
        "expected the group the account holds to resolve, got {status} with {resolved}"
    );
}

#[tokio::test]
async fn deleting_a_resiliency_group_moves_it_through_deleting_to_a_readable_tombstone() {
    // RULE: `deleting` is where a torn-down group starts and `deleted` is the terminal
    // state the clock owes it, which `ResiliencyGroupState` models as a state a group is
    // REPORTED in — so the tombstone stays readable under its own id, a repeated delete is
    // the state it already asks for, and a group on its way out takes no rename.
    let dx = Emulator::new();
    let group_id = group(&dx, "torn-down-group", "basic-resiliency").await;

    let (status, deleted) = dx
        .call(
            "DeleteResiliencyGroup",
            json!({"resiliencyGroupId": &group_id}),
        )
        .await;
    assert_eq!(
        (status, deleted["resiliencyGroup"]["state"].clone()),
        (200, json!(RESILIENCY_GROUP_STATE_DELETING)),
        "expected the delete to answer the group deleting, got {status} with {deleted}"
    );

    // A group being torn down is not one a name can be changed on.
    let (status, refused) = dx
        .call(
            "UpdateResiliencyGroup",
            json!({"resiliencyGroupId": &group_id, "resiliencyGroupName": "renamed"}),
        )
        .await;
    assert_eq!(
        (status, error_message(&refused)),
        (
            400,
            format!(
                "Resiliency group {group_id} is in state {RESILIENCY_GROUP_STATE_DELETING}"
            )
        ),
        "expected a rename of a group being torn down to be refused, got {status} with {refused}"
    );

    // `deleted` is what the clock owes a `deleting` group, so the read is looked at again
    // until that state appears — and the tombstone is still READABLE there.
    let tombstone = group_in_state(
        &dx,
        "a torn-down group",
        &group_id,
        RESILIENCY_GROUP_STATE_DELETED,
    )
    .await;
    assert_eq!(
        tombstone["resiliencyGroupId"],
        json!(group_id),
        "expected the tombstone to stay readable under its own id, got {tombstone}"
    );
    let (listed, _) = list_groups(&dx, json!({})).await;
    assert_eq!(
        listed
            .iter()
            .map(|summary| summary["state"].clone())
            .collect::<Vec<Value>>(),
        vec![json!(RESILIENCY_GROUP_STATE_DELETED)],
        "expected the listing to report the tombstone too, got {listed:?}"
    );

    // The delete is idempotent: a group already gone is in the state the call asks for.
    let (status, again) = dx
        .call(
            "DeleteResiliencyGroup",
            json!({"resiliencyGroupId": &group_id}),
        )
        .await;
    assert_eq!(
        (status, again["resiliencyGroup"]["state"].clone()),
        (200, json!(RESILIENCY_GROUP_STATE_DELETED)),
        "expected a repeated delete to answer the state it already asks for, got {status} with \
         {again}"
    );
}

#[tokio::test]
async fn renaming_a_resiliency_group_keeps_the_identity_the_service_minted() {
    // `UpdateResiliencyGroup` takes the group's id and a new `resiliencyGroupName` and
    // nothing else, so a rename is a modification of the group rather than a new one: the
    // id, the ARN, the type and the tags all stay.
    let dx = Emulator::new();
    let (status, created) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body(
                "named-before",
                "high-resiliency",
                &[("tier", "high")],
                None,
            ),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the create to succeed, got {status} with {created}"
    );
    let before = created["resiliencyGroup"].clone();
    let group_id = before["resiliencyGroupId"].as_str().unwrap_or_default().to_string();

    let (status, renamed) = dx
        .call(
            "UpdateResiliencyGroup",
            json!({"resiliencyGroupId": &group_id, "resiliencyGroupName": "named-after"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the rename to succeed, got {status} with {renamed}"
    );
    let after = renamed["resiliencyGroup"].clone();
    assert_eq!(
        (
            after["resiliencyGroupId"].clone(),
            after["resiliencyGroupArn"].clone(),
            after["resiliencyGroupType"].clone(),
            after["tags"].clone(),
            after["resiliencyGroupName"].clone(),
        ),
        (
            before["resiliencyGroupId"].clone(),
            before["resiliencyGroupArn"].clone(),
            before["resiliencyGroupType"].clone(),
            before["tags"].clone(),
            json!("named-after"),
        ),
        "expected the rename to keep every other value, got {after}"
    );
    let (_, held) = get_group(&dx, &group_id).await;
    assert_eq!(
        held["resiliencyGroupName"],
        json!("named-after"),
        "expected the store to hold the new name, got {held}"
    );
}

#[tokio::test]
async fn resiliency_groups_page_with_max_results_and_next_token() {
    // `maxResults` windows the listing and `nextToken` resumes it just past the page's
    // last item, and "if MaxResults is given a value larger than 100, only 100 results
    // are returned" — so a page size past that ceiling is the ceiling.
    let dx = Emulator::new();
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(group(&dx, &format!("page-group-{n}"), "basic-resiliency").await);
    }

    let (page, token) = list_groups(&dx, json!({"maxResults": 2})).await;
    assert_eq!(
        page.iter()
            .map(|summary| summary["resiliencyGroupId"].as_str().unwrap_or_default())
            .collect::<Vec<&str>>(),
        ids[..2].iter().map(String::as_str).collect::<Vec<&str>>(),
        "expected the first two groups in creation order, got {page:?}"
    );
    assert_eq!(
        token,
        json!(ids[1]),
        "expected the token to be the key of the page's last item, got {token:?}"
    );

    let (rest, token) = list_groups(&dx, json!({"nextToken": &ids[1]})).await;
    assert_eq!(
        (
            rest.iter()
                .map(|summary| summary["resiliencyGroupId"].as_str().unwrap_or_default())
                .collect::<Vec<&str>>(),
            token
        ),
        (
            ids[2..].iter().map(String::as_str).collect::<Vec<&str>>(),
            Value::Null
        ),
        "expected the rest of the listing and no further token, got {rest:?}"
    );

    // A window wider than the result set, and one past the published ceiling, both answer
    // the whole set in one page.
    for body in [
        json!({"maxResults": ids.len()}),
        json!({"maxResults": MAX_RESULT_SET_SIZE + 1}),
    ] {
        let (whole, token) = list_groups(&dx, body.clone()).await;
        assert_eq!(
            (whole.len(), token),
            (ids.len(), Value::Null),
            "case {body}: expected the whole set in one page"
        );
    }
}

#[tokio::test]
async fn the_group_store_refuses_one_past_its_ceiling_and_reclaims_a_torn_down_one() {
    // The emulator's own memory ceiling (not an AWS quota): at
    // `MAX_RECORDS_PER_STORE` a create first reclaims what the clock has already torn
    // down and, if that frees nothing, is refused with a modelled error naming the
    // ceiling. The store is filled through the shared engine rather than through a
    // thousand wire calls, which is what the handler then reads.
    let dx = Emulator::new();
    {
        let state = dx
            .engine()
            .state::<ComAmazonawsDirectconnectOvertureserviceState>();
        let mut store = state.lock();
        for n in 0..MAX_RECORDS_PER_STORE {
            let group_id = format!("dxrg-{n:017}");
            store.resiliency_groups.insert(
                group_id.clone(),
                ResiliencyGroup {
                    resiliency_group_id: group_id.clone(),
                    resiliency_group_name: format!("seeded-{n}"),
                    intended_resiliency_model: "basic-resiliency".to_string(),
                    owner_account: engine::emulator_account_id().to_string(),
                    state: RESILIENCY_GROUP_STATE_AVAILABLE.to_string(),
                    tags: Vec::new(),
                    client_token: None,
                    due: None,
                },
            );
            store.resiliency_group_order.push(group_id);
        }
    }

    let (status, refused) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body("one-too-many", "basic-resiliency", &[], None),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "LimitExceededException".to_string()),
        "expected the ceiling to refuse a further group, got {status} with {refused}"
    );

    // A record the clock has already torn down is reclaimed, which frees the room.
    {
        let state = dx
            .engine()
            .state::<ComAmazonawsDirectconnectOvertureserviceState>();
        let mut store = state.lock();
        let first = store.resiliency_group_order[0].clone();
        if let Some(held) = store.resiliency_groups.get_mut(&first) {
            held.state = RESILIENCY_GROUP_STATE_DELETED.to_string();
        }
    }
    let (status, created) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body("room-reclaimed", "basic-resiliency", &[], None),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the reclaimed room to admit a further group, got {status} with {created}"
    );
}

#[tokio::test]
async fn deleting_a_gateway_moves_it_to_a_tombstone_and_is_refused_while_it_carries_traffic() {
    // RULE: "You must first delete all virtual interfaces that are attached to the Direct
    // Connect gateway and disassociate all virtual private gateways associated with the
    // Direct Connect gateway" — so a gateway something still terminates on is not
    // deletable, and one nothing does walks `deleting` to the terminal `deleted` the clock
    // owes it.
    let dx = Emulator::new();
    let gateway_id = gateway(&dx, "deletable-gateway").await;
    let attached_to = gateway(&dx, "attached-gateway").await;

    // A virtual interface terminating on the second gateway, created the way a client
    // creates one: over a connection of this account, naming that gateway.
    let (status, connection) = dx
        .call(
            "CreateConnection",
            json!({
                "connectionName": "gateway-delete-connection",
                "location": "EqDC2-EQ",
                "bandwidth": "1Gbps",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConnection to succeed, got {status} with {connection}"
    );
    let connection_id = connection["connectionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, interface) = dx
        .call(
            "CreatePrivateVirtualInterface",
            json!({
                "connectionId": &connection_id,
                "newPrivateVirtualInterface": {
                    "virtualInterfaceName": "gateway-delete-vif",
                    "vlan": 101,
                    "asn": 65000,
                    "directConnectGatewayId": &attached_to,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreatePrivateVirtualInterface to succeed, got {status} with {interface}"
    );

    let (status, refused) = dx
        .call(
            "DeleteDirectConnectGateway",
            json!({"directConnectGatewayId": &attached_to}),
        )
        .await;
    assert_eq!(
        (status, error_message(&refused)),
        (
            400,
            format!("Direct Connect gateway {attached_to} has virtual interfaces attached")
        ),
        "expected a gateway with an attachment to be undeletable, got {status} with {refused}"
    );

    // A gateway nothing terminates on is torn down: `deleting` first, then the terminal
    // `deleted` the clock owes it, which stays selectable by its own id.
    let (status, deleted) = dx
        .call(
            "DeleteDirectConnectGateway",
            json!({"directConnectGatewayId": &gateway_id}),
        )
        .await;
    assert_eq!(
        (
            status,
            deleted["directConnectGateway"]["directConnectGatewayState"].clone()
        ),
        (200, json!("deleting")),
        "expected the delete to answer the gateway deleting, got {status} with {deleted}"
    );
    // `deleted` is what the clock owes a `deleting` gateway, so the listing is looked at
    // again until it reports that state — and the tombstone is still SELECTED by its own
    // id there.
    let tombstone = gateway_in_state(&dx, "a torn-down gateway", &gateway_id, "deleted").await;
    assert_eq!(
        tombstone["directConnectGatewayId"],
        json!(gateway_id),
        "expected the tombstone to stay describable under its own id, got {tombstone}"
    );

    // An id naming no gateway at all is the unresolved-id refusal, and a repeated delete
    // answers the state it already asks for.
    let (status, unknown) = dx
        .call(
            "DeleteDirectConnectGateway",
            json!({"directConnectGatewayId": "00000000-0000-4000-8000-000000000000"}),
        )
        .await;
    // The sentence is written out rather than taken from the service: its wording is what
    // a client branches on, so the wording is what this pins (see the table below over
    // every operation that resolves a gateway id).
    assert_eq!(
        (status, error_message(&unknown)),
        (
            400,
            gateway_not_found("00000000-0000-4000-8000-000000000000")
        ),
        "expected the unresolved-id refusal, got {status} with {unknown}"
    );
    let (status, again) = dx
        .call(
            "DeleteDirectConnectGateway",
            json!({"directConnectGatewayId": &gateway_id}),
        )
        .await;
    assert_eq!(
        (
            status,
            again["directConnectGateway"]["directConnectGatewayState"].clone()
        ),
        (200, json!("deleted")),
        "expected a repeated delete to answer the state it already asks for, got {status} with \
         {again}"
    );
}

#[tokio::test]
async fn renaming_a_gateway_keeps_its_id_and_is_refused_once_it_is_torn_down() {
    // `UpdateDirectConnectGateway` "updates the name of a CURRENT Direct Connect gateway"
    // and takes nothing else, so a rename keeps the id that service minted and the ASN it
    // was created with — and a gateway being torn down is not a current one.
    let dx = Emulator::new();
    let (status, created) = dx
        .call(
            "CreateDirectConnectGateway",
            create_gateway_body("renamed-before", Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[0].0), &[]),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the create to succeed, got {status} with {created}"
    );
    let before = created["directConnectGateway"].clone();
    let gateway_id = before["directConnectGatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, renamed) = dx
        .call(
            "UpdateDirectConnectGateway",
            json!({
                "directConnectGatewayId": &gateway_id,
                "newDirectConnectGatewayName": "renamed-after",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the rename to succeed, got {status} with {renamed}"
    );
    let after = renamed["directConnectGateway"].clone();
    assert_eq!(
        (
            after["directConnectGatewayId"].clone(),
            after["amazonSideAsn"].clone(),
            after["ownerAccount"].clone(),
            after["directConnectGatewayName"].clone(),
        ),
        (
            before["directConnectGatewayId"].clone(),
            before["amazonSideAsn"].clone(),
            before["ownerAccount"].clone(),
            json!("renamed-after"),
        ),
        "expected the rename to keep every other value, got {after}"
    );
    let (listed, _) = describe(&dx, json!({"directConnectGatewayId": &gateway_id})).await;
    assert_eq!(
        listed
            .iter()
            .map(|held| held["directConnectGatewayName"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("renamed-after")],
        "expected the store to hold the new name, got {listed:?}"
    );

    // A name made only of whitespace names nothing, and a gateway on its way out takes no
    // name at all.
    let (status, empty) = dx
        .call(
            "UpdateDirectConnectGateway",
            json!({
                "directConnectGatewayId": &gateway_id,
                "newDirectConnectGatewayName": "   ",
            }),
        )
        .await;
    assert_eq!(
        (status, error_message(&empty)),
        (
            400,
            "newDirectConnectGatewayName must not be empty".to_string()
        ),
        "expected a whitespace-only name to be refused, got {status} with {empty}"
    );
    let (status, _) = dx
        .call(
            "DeleteDirectConnectGateway",
            json!({"directConnectGatewayId": &gateway_id}),
        )
        .await;
    assert_eq!(status, 200, "setup: expected the delete to succeed");
    let (status, torn_down) = dx
        .call(
            "UpdateDirectConnectGateway",
            json!({
                "directConnectGatewayId": &gateway_id,
                "newDirectConnectGatewayName": "renamed-again",
            }),
        )
        .await;
    assert_eq!(
        (status, error_message(&torn_down)),
        (
            400,
            format!("Direct Connect gateway {gateway_id} is in state deleting")
        ),
        "expected a rename of a gateway being torn down to be refused, got {status} with \
         {torn_down}"
    );
}

/// `count` tags a bulk write carries, keyed `bulk-<n>`.
fn bulk_tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("bulk-{index}"), "value": "x"}))
            .collect(),
    )
}

/// The keys [`bulk_tags`] writes, as a `tagKeys` list.
fn bulk_keys(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("bulk-{index}")))
            .collect(),
    )
}

/// The tags `DescribeTags` reports for the one resource `resource_arn` names.
async fn tags_of(dx: &Emulator, resource_arn: &str) -> Value {
    let (status, described) = dx
        .call("DescribeTags", json!({"resourceArns": [resource_arn]}))
        .await;
    assert_eq!(
        status, 200,
        "expected DescribeTags 200 for {resource_arn}, got {status} with {described}"
    );
    described["resourceTags"][0]["tags"].clone()
}

#[tokio::test]
async fn tags_are_written_merged_and_removed_by_the_arn_a_resource_publishes() {
    // RULE: `TagResource` writes the tags of the resource its ARN names — merging by
    // key, since "if a tag with the same key is already associated with the resource,
    // this action updates its value", and bounded by the number of tags one resource may
    // hold — and `UntagResource` removes the keys it names and keeps the rest. Both
    // reach the SAME stored tag list every read of that resource reports, and an ARN
    // this account holds no resource under names nothing to tag.
    //
    // The rows span the resource kinds the ARN resolution distinguishes: a Direct
    // Connect gateway and a resiliency group, whose ARNs carry no Region, and a
    // connection, whose ARN carries its own.
    let dx = Emulator::new();
    let gateway_id = gateway(&dx, "tag-write-gateway").await;
    let group_id = group(&dx, "tag-write-group", "basic-resiliency").await;
    let (status, connection) = dx
        .call(
            "CreateConnection",
            json!({
                "connectionName": "tag-write-connection",
                "location": "EqDC2-EQ",
                "bandwidth": "1Gbps",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConnection to succeed, got {status} with {connection}"
    );
    let connection_id = connection["connectionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rows: &[(&str, String)] = &[
        (
            "a Direct Connect gateway, whose ARN carries no Region",
            direct_connect_arn("", "dx-gateway", &gateway_id),
        ),
        (
            "a resiliency group, whose ARN carries no Region",
            direct_connect_arn("", "dx-resiliency-group", &group_id),
        ),
        (
            "a connection, whose ARN carries its own Region",
            direct_connect_arn("us-east-1", "dxcon", &connection_id),
        ),
    ];

    for (case, arn) in rows {
        // A write is read back by the same ARN it was made under.
        let (status, written) = dx
            .call(
                "TagResource",
                json!({"resourceArn": arn, "tags": tag_list(&[("env", "prod"), ("team", "network")])}),
            )
            .await;
        assert_eq!(
            (status, written.clone()),
            (200, json!({})),
            "case {case:?}: expected the tag write to be accepted, got {status} with {written}"
        );
        assert_eq!(
            tags_of(&dx, arn).await,
            tag_list(&[("env", "prod"), ("team", "network")]),
            "case {case:?}: expected the tags just written, got {:?}",
            tags_of(&dx, arn).await
        );

        // A second write MERGES: the value of a key already held is updated in place,
        // a key the resource does not hold is added, and a key the request does not
        // name is kept.
        let (status, merged) = dx
            .call(
                "TagResource",
                json!({"resourceArn": arn, "tags": tag_list(&[("env", "staging"), ("tier", "edge")])}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the merging write to be accepted, got {status} with {merged}"
        );
        assert_eq!(
            tags_of(&dx, arn).await,
            tag_list(&[("env", "staging"), ("team", "network"), ("tier", "edge")]),
            "case {case:?}: expected one entry per key, the named ones updated, got {:?}",
            tags_of(&dx, arn).await
        );

        // One key per call: a request naming a key twice does not say which value it
        // means, and the refused write leaves the resource as it was.
        let (status, duplicated) = dx
            .call(
                "TagResource",
                json!({"resourceArn": arn, "tags": tag_list(&[("env", "a"), ("env", "b")])}),
            )
            .await;
        assert_eq!(
            (status, error_code(&duplicated)),
            (400, "DuplicateTagKeysException".to_string()),
            "case {case:?}: expected a repeated key to be refused, got {status} with {duplicated}"
        );

        // The ceiling is on what the resource would HOLD: three keys are already on it,
        // so a write that takes it one past the ceiling is refused and writes nothing,
        // and one that lands exactly at the ceiling is accepted.
        let past = MAX_TAGS_PER_RESOURCE - 2;
        let (status, too_many) = dx
            .call(
                "TagResource",
                json!({"resourceArn": arn, "tags": bulk_tags(past)}),
            )
            .await;
        assert_eq!(
            (status, error_code(&too_many)),
            (400, "TooManyTagsException".to_string()),
            "case {case:?}: expected a write past the ceiling to be refused, got {status} with \
             {too_many}"
        );
        assert_eq!(
            tags_of(&dx, arn).await,
            tag_list(&[("env", "staging"), ("team", "network"), ("tier", "edge")]),
            "case {case:?}: a refused write must change nothing, got {:?}",
            tags_of(&dx, arn).await
        );
        let at_ceiling = MAX_TAGS_PER_RESOURCE - 3;
        let (status, filled) = dx
            .call(
                "TagResource",
                json!({"resourceArn": arn, "tags": bulk_tags(at_ceiling)}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a write up to the ceiling to be accepted, got {status} with \
             {filled}"
        );
        assert_eq!(
            tags_of(&dx, arn).await.as_array().map_or(0, Vec::len),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected the resource to hold the ceiling, got {:?}",
            tags_of(&dx, arn).await
        );

        // Removing keys: the named ones go, a key the resource does not carry removes
        // nothing, and the rest are kept.
        let mut keys = bulk_keys(at_ceiling);
        if let Some(list) = keys.as_array_mut() {
            list.push(json!("never-written"));
        }
        let (status, removed) = dx
            .call(
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": keys}),
            )
            .await;
        assert_eq!(
            (status, removed.clone()),
            (200, json!({})),
            "case {case:?}: expected the tag removal to be accepted, got {status} with {removed}"
        );
        assert_eq!(
            tags_of(&dx, arn).await,
            tag_list(&[("env", "staging"), ("team", "network"), ("tier", "edge")]),
            "case {case:?}: expected only the named keys removed, got {:?}",
            tags_of(&dx, arn).await
        );
    }

    // A single call cannot name more tags than one resource may hold, nor more keys to
    // remove than one resource may carry: every tag it names would have to end up
    // there, and every key it names would have to be there to remove.
    let gateway_arn = direct_connect_arn("", "dx-gateway", &gateway_id);
    let (status, oversized) = dx
        .call(
            "TagResource",
            json!({"resourceArn": &gateway_arn, "tags": bulk_tags(MAX_TAGS_PER_RESOURCE + 1)}),
        )
        .await;
    assert_eq!(
        (status, error_code(&oversized)),
        (400, "TooManyTagsException".to_string()),
        "expected a call naming more tags than a resource may hold to be refused, got {status} \
         with {oversized}"
    );
    for (case, count, accepted) in [
        ("a key list exactly at the ceiling", MAX_TAGS_PER_RESOURCE, true),
        ("one key past the ceiling", MAX_TAGS_PER_RESOURCE + 1, false),
    ] {
        let (status, response) = dx
            .call(
                "UntagResource",
                json!({"resourceArn": &gateway_arn, "tagKeys": bulk_keys(count)}),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the removal to be accepted, got {status} with {response}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&response),
                    error_message(&response)
                ),
                (
                    400,
                    "DirectConnectClientException".to_string(),
                    too_many_tag_keys_message(count),
                ),
                "case {case:?}: expected the key-count ceiling to refuse the removal, got \
                 {status} with {response}"
            );
        }
    }

    // The gateway's own record reports the tags the write reached, so the tag write and
    // every gateway read see one tag list.
    let (described, _) = describe(&dx, json!({"directConnectGatewayId": &gateway_id})).await;
    assert_eq!(
        described[0]["tags"],
        tag_list(&[("env", "staging"), ("team", "network"), ("tier", "edge")]),
        "expected the gateway record to report the tags the write reached, got {:?}",
        described[0]
    );

    // An ARN this account holds no resource under names nothing to tag or untag.
    let unknown = direct_connect_arn("", "dx-gateway", "00000000-0000-4000-8000-000000000000");
    for (case, target, body) in [
        (
            "a write naming no resource",
            "TagResource",
            json!({"resourceArn": &unknown, "tags": tag_list(&[("env", "prod")])}),
        ),
        (
            "a removal naming no resource",
            "UntagResource",
            json!({"resourceArn": &unknown, "tagKeys": ["env"]}),
        ),
    ] {
        let (status, refused) = dx.call(target, body).await;
        assert_eq!(
            (status, error_code(&refused), error_message(&refused)),
            (
                400,
                "DirectConnectClientException".to_string(),
                format!("Resource {unknown} does not exist"),
            ),
            "case {case:?}: expected the unresolved-ARN refusal, got {status} with {refused}"
        );
    }
}

#[tokio::test]
async fn describe_tags_reports_the_tags_of_each_kind_of_resource_by_its_arn() {
    // RULE: "Describes the tags associated with the SPECIFIED Direct Connect resources" —
    // one `ResourceTag` per ARN the request names that this account holds a resource
    // under, in the order the request named them, with the tags that resource carries; an
    // ARN naming no resource of this account contributes no element at all, there being no
    // tags to describe for it.
    //
    // The rows span the four kinds of resource this service tags — a connection and a
    // virtual interface, whose ARNs carry their Region, and a Direct Connect gateway and a
    // resiliency group, whose ARNs carry none — plus a resource nobody tagged and an ARN
    // naming nothing.
    let dx = Emulator::new();
    let (status, connection) = dx
        .call(
            "CreateConnection",
            json!({
                "connectionName": "tagged-connection",
                "location": "EqDC2-EQ",
                "bandwidth": "1Gbps",
                "tags": tag_list(&[("kind", "connection")]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConnection to succeed, got {status} with {connection}"
    );
    let connection_id = connection["connectionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, interface) = dx
        .call(
            "CreatePrivateVirtualInterface",
            json!({
                "connectionId": &connection_id,
                "newPrivateVirtualInterface": {
                    "virtualInterfaceName": "tagged-vif",
                    "vlan": 102,
                    "asn": 65000,
                    "virtualGatewayId": "vgw-0123456789abcdef0",
                    "tags": tag_list(&[("kind", "virtual-interface")]),
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreatePrivateVirtualInterface to succeed, got {status} with {interface}"
    );
    let interface_id = interface["virtualInterfaceId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, created) = dx
        .call(
            "CreateDirectConnectGateway",
            create_gateway_body("tagged-gateway", None, &[("kind", "gateway")]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateDirectConnectGateway to succeed, got {status} with {created}"
    );
    let gateway_id = created["directConnectGateway"]["directConnectGatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, created_group) = dx
        .call(
            "CreateResiliencyGroup",
            create_group_body("tagged-group", "basic-resiliency", &[("kind", "group")], None),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateResiliencyGroup to succeed, got {status} with {created_group}"
    );
    let group_id = created_group["resiliencyGroup"]["resiliencyGroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let untagged = group(&dx, "untagged-group", "basic-resiliency").await;

    let connection_arn = direct_connect_arn("us-east-1", "dxcon", &connection_id);
    let interface_arn = direct_connect_arn("us-east-1", "dxvif", &interface_id);
    let gateway_arn = direct_connect_arn("", "dx-gateway", &gateway_id);
    let group_arn = direct_connect_arn("", "dx-resiliency-group", &group_id);
    let untagged_arn = direct_connect_arn("", "dx-resiliency-group", &untagged);
    let unknown_arn = direct_connect_arn("", "dx-resiliency-group", "dxrg-00000000000000000");

    let (status, described) = dx
        .call(
            "DescribeTags",
            json!({"resourceArns": [
                &connection_arn,
                &interface_arn,
                &gateway_arn,
                &group_arn,
                &untagged_arn,
                &unknown_arn,
            ]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected DescribeTags 200, got {status} with {described}"
    );
    assert_eq!(
        described["resourceTags"],
        json!([
            {"resourceArn": &connection_arn, "tags": tag_list(&[("kind", "connection")])},
            {"resourceArn": &interface_arn, "tags": tag_list(&[("kind", "virtual-interface")])},
            {"resourceArn": &gateway_arn, "tags": tag_list(&[("kind", "gateway")])},
            {"resourceArn": &group_arn, "tags": tag_list(&[("kind", "group")])},
            {"resourceArn": &untagged_arn, "tags": tag_list(&[])},
        ]),
        "expected one element per ARN this account holds a resource under, in request order, \
         got {described}"
    );

    // ...and a read of only the ARN that names nothing answers the one way a listing can.
    let (status, nothing) = dx
        .call("DescribeTags", json!({"resourceArns": [&unknown_arn]}))
        .await;
    assert_eq!(
        (status, nothing["resourceTags"].clone()),
        (200, json!([])),
        "expected an empty list for an ARN naming no resource, got {status} with {nothing}"
    );

    // The caller-chosen BATCH is bounded: `ResourceArnList` carries no `@length`, so the
    // ceiling is this emulator's own and it is enforced before any element is resolved.
    // The rows sit AT it and one past it.
    for (case, count, accepted) in [
        ("a batch exactly at the ceiling", MAX_RESOURCES_PER_CALL, true),
        ("one resource past the ceiling", MAX_RESOURCES_PER_CALL + 1, false),
    ] {
        let batch: Vec<String> = (0..count).map(|_| group_arn.clone()).collect();
        let (status, response) = dx
            .call("DescribeTags", json!({"resourceArns": batch}))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the batch to be read, got {status} with {response}"
            );
            assert_eq!(
                response["resourceTags"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                count,
                "case {case:?}: expected one element per ARN named, got {response}"
            );
        } else {
            assert_eq!(
                (status, error_code(&response), error_message(&response)),
                (
                    400,
                    "DirectConnectClientException".to_string(),
                    too_many_resources_message(count),
                ),
                "case {case:?}: expected the batch ceiling to refuse the read, got {status} \
                 with {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: the resource a create is refused ABOUT is the first identifier it resolves,
// and this create resolves the gateway to associate — the account's own resource,
// held by the service that owns gateways — before the Direct Connect gateway the
// association would be made on. So a gateway the account does not hold is reported
// as the missing one, named by the kind its identifier says it is and by the account
// that would own it, whether or not the Direct Connect gateway exists either and
// whichever member of the pair named it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_is_refused_about_the_gateway_to_associate_before_the_direct_connect_gateway()
{
    let dx = Emulator::new();
    let stored_gateway = gateway(&dx, "resolution-order").await;
    // A Direct Connect gateway id the store holds no gateway under, so a row naming
    // it would be refused about the Direct Connect gateway if the order were the
    // other way round.
    let no_such_direct_connect_gateway = "00000000-0000-4000-8000-000000000000";
    // The account the gateway would belong to comes from the emulator, never from a
    // recorded response.
    let owner = engine::emulator_account_id();

    // (case, the request body, the gateway the refusal is about, the noun that
    // gateway's kind is named by)
    let rows: Vec<(String, Value, &str, &str)> = vec![
        (
            "a virtual private gateway as gatewayId, beside a Direct Connect gateway that \
             exists"
                .to_string(),
            json!({
                "directConnectGatewayId": &stored_gateway,
                "gatewayId": "vgw-0123456789abcdef0",
            }),
            "vgw-0123456789abcdef0",
            "VirtualGateway",
        ),
        (
            "a virtual private gateway as virtualGatewayId, beside a Direct Connect gateway \
             that does not exist"
                .to_string(),
            json!({
                "directConnectGatewayId": no_such_direct_connect_gateway,
                "virtualGatewayId": "vgw-0f1e2d3c4b5a69780",
            }),
            "vgw-0f1e2d3c4b5a69780",
            "VirtualGateway",
        ),
        (
            "both spellings naming the same virtual private gateway".to_string(),
            json!({
                "directConnectGatewayId": &stored_gateway,
                "gatewayId": "vgw-00112233445566778",
                "virtualGatewayId": "vgw-00112233445566778",
            }),
            "vgw-00112233445566778",
            "VirtualGateway",
        ),
        (
            "a virtual private gateway named with surrounding space".to_string(),
            json!({
                "directConnectGatewayId": &stored_gateway,
                "gatewayId": "  vgw-0aabbccddeeff0011  ",
            }),
            "vgw-0aabbccddeeff0011",
            "VirtualGateway",
        ),
        (
            "a transit gateway with the prefixes such an association advertises".to_string(),
            json!({
                "directConnectGatewayId": &stored_gateway,
                "gatewayId": "tgw-0f1e2d3c4b5a69780",
                "addAllowedPrefixesToDirectConnectGateway": [{"cidr": "10.0.0.0/16"}],
            }),
            "tgw-0f1e2d3c4b5a69780",
            "TransitGateway",
        ),
        (
            "a transit gateway with no prefixes and a Direct Connect gateway that does not \
             exist: the gateway to associate is still what is reported"
                .to_string(),
            json!({
                "directConnectGatewayId": no_such_direct_connect_gateway,
                "virtualGatewayId": "tgw-00112233445566778",
            }),
            "tgw-00112233445566778",
            "TransitGateway",
        ),
    ];

    for (case, body, named, noun) in &rows {
        let (status, response) = dx
            .call("CreateDirectConnectGatewayAssociation", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
        assert_eq!(
            error_message(&response),
            format!("The {noun} {named} for owner {owner} does not exist"),
            "case {case:?}: expected the refusal to be about the gateway to associate, got \
             {response}"
        );
        // A refused create stores nothing, for either Direct Connect gateway the rows
        // name.
        for selector in [
            json!({"directConnectGatewayId": &stored_gateway}),
            json!({"directConnectGatewayId": no_such_direct_connect_gateway}),
        ] {
            let page = associations(&dx, case, selector).await;
            assert!(
                page.is_empty(),
                "case {case:?}: a refused create must store no association, got {page:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// How an unresolved identifier is WORDED: the ARN of a tag call that names no resource of
// this account, and the id of a gateway no account holds.
//
// The rules pinned below:
//
// 1. A tag call whose `resourceArn` is not an ARN at all is refused about its FORM, while
//    one that parses but names no resource of this account is refused about the resource.
// 2. One wording for an unresolved gateway id, on every operation that resolves one.
// ---------------------------------------------------------------------------------------


/// The ARN resource path of each kind of Direct Connect resource these suites name, and
/// the Region such an ARN carries — none for a kind that is global. Written out rather
/// than taken from the service, so the FORM is what the test pins.
const GATEWAY_PATH: &str = "dx-gateway";
const RESILIENCY_GROUP_PATH: &str = "dx-resiliency-group";
const REGIONAL: &str = "us-east-1";
const GLOBAL: &str = "";

/// One row of the unresolved-gateway-id table: case, the operation, and the request body
/// built around the id it must be refused about.
type UnresolvedRow = (&'static str, &'static str, Value);

/// One row of the tag-wording table: case, the operation, and a request body built around
/// the tag set the row submits, so only that set differs between the cases.
type TagWordingRow = (
    &'static str,
    &'static str,
    Box<dyn Fn(&[(&str, &str)]) -> Value>,
);

/// `TagResource` with `tags` on `resource_arn`.
async fn tag_resource(dx: &Emulator, resource_arn: &str, tags: &[(&str, &str)]) -> (u16, Value) {
    dx.call(
        "TagResource",
        json!({"resourceArn": resource_arn, "tags": tag_list(tags)}),
    )
    .await
}

/// The ONE sentence this service states for a `directConnectGatewayId` no gateway exists
/// under, written out here: its wording is what a client branches on to tell this refusal
/// from any other client fault of the operation, so the wording is what the test pins.
fn gateway_not_found(gateway_id: &str) -> String {
    format!(
        "Direct Connect Gateway for owner {} and with id {gateway_id} does not exist",
        engine::EMULATOR_ACCOUNT_ID
    )
}

#[tokio::test]
async fn a_tag_call_is_refused_about_the_arns_form_or_about_the_resource_it_names() {
    let dx = Emulator::new();
    let (status, created) = dx
        .call(
            "CreateDirectConnectGateway",
            create_gateway_body("tag-refusal-gateway", None, &[("held", "already")]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateDirectConnectGateway to succeed, got {status} with {created}"
    );
    let gateway_id = created["directConnectGateway"]["directConnectGatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let gateway_arn = direct_connect_arn(GLOBAL, GATEWAY_PATH, &gateway_id);
    let held_before = tags_of(&dx, &gateway_arn).await;

    // (case, the value the write names, the sentence it is refused with) — a value that is
    // not an ARN at all is refused about its FORM, and an ARN that parses but names no
    // resource of this account about the RESOURCE: a gateway ARN whose id nobody holds, a
    // gateway ARN carrying a Region (a gateway is global, so its ARN carries none), an ARN
    // of another service, and an ARN of this service naming a resource path it does not
    // publish.
    let unresolved: &[(&str, String, String)] = &[
        (
            "a gateway ARN whose id nobody holds",
            direct_connect_arn(GLOBAL, GATEWAY_PATH, "00000000-0000-4000-8000-000000000000"),
            format!(
                "Resource {} does not exist",
                direct_connect_arn(GLOBAL, GATEWAY_PATH, "00000000-0000-4000-8000-000000000000")
            ),
        ),
        (
            "a gateway ARN carrying a Region",
            direct_connect_arn(REGIONAL, GATEWAY_PATH, &gateway_id),
            format!(
                "Resource {} does not exist",
                direct_connect_arn(REGIONAL, GATEWAY_PATH, &gateway_id)
            ),
        ),
        (
            "an ARN of another service",
            format!(
                "arn:aws:ec2:{REGIONAL}:{}:vpc/vpc-0123456789abcdef0",
                engine::EMULATOR_ACCOUNT_ID
            ),
            format!(
                "Resource arn:aws:ec2:{REGIONAL}:{}:vpc/vpc-0123456789abcdef0 does not exist",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "a resource path this service does not publish",
            direct_connect_arn(GLOBAL, "dx-not-a-resource", &gateway_id),
            format!(
                "Resource {} does not exist",
                direct_connect_arn(GLOBAL, "dx-not-a-resource", &gateway_id)
            ),
        ),
        (
            "a value that is no ARN at all",
            gateway_id.clone(),
            format!("Invalid ARN format: {gateway_id}"),
        ),
        (
            "a value that only starts out like an ARN",
            "arn:aws:directconnect".to_string(),
            "Invalid ARN format: arn:aws:directconnect".to_string(),
        ),
        (
            "an ARN whose resource field is empty",
            format!("arn:aws:directconnect::{}:", engine::EMULATOR_ACCOUNT_ID),
            format!(
                "Invalid ARN format: arn:aws:directconnect::{}:",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];
    for (case, resource_arn, refusal) in unresolved {
        let (status, body) = tag_resource(&dx, resource_arn, &[("new", "tag")]).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                400,
                "DirectConnectClientException".to_string(),
                refusal.clone()
            ),
            "case {case:?}: expected the unresolved-ARN refusal, got {status} with {body}"
        );
        // A removal resolves the ARN the same way, so it is refused with the same sentence.
        let (status, body) = dx
            .call(
                "UntagResource",
                json!({"resourceArn": resource_arn, "tagKeys": ["held"]}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                400,
                "DirectConnectClientException".to_string(),
                refusal.clone()
            ),
            "case {case:?}: expected the removal to be refused alike, got {status} with {body}"
        );
    }

    // None of those calls reached a resource: the gateway holds what it held, and a
    // resource no call named holds no tags at all.
    assert_eq!(
        tags_of(&dx, &gateway_arn).await,
        held_before,
        "a refused call must leave the resource it failed to name as it was"
    );
    let other = group(&dx, "tag-refusal-group", "basic-resiliency").await;
    assert_eq!(
        tags_of(&dx, &direct_connect_arn(GLOBAL, RESILIENCY_GROUP_PATH, &other)).await,
        tag_list(&[]),
        "a resource no call named must hold no tags"
    );
}

#[tokio::test]
async fn a_tag_set_one_resource_cannot_carry_is_refused_alike_on_every_write_of_it() {
    // RULE: a tag set naming one key twice, and one past the number of tags a resource may
    // hold, are refused with the ONE sentence this service states for each — the tag write
    // and every create that carries a tag set alike, since the sentence is all a client has
    // to tell the two faults apart.
    //
    // The rows are the writes that take a tag set: `TagResource` and each create of a
    // taggable resource this service mints, so no one of them can word either refusal
    // differently. Each row names the request body around a tag set, so only the tag set
    // differs between the duplicate-key and the ceiling case.
    let dx = Emulator::new();
    let connection_id = connection(&dx, "tag-wording-connection", "EqDC2-EQ", "1Gbps").await;
    let gateway_id = gateway(&dx, "tag-wording-gateway").await;
    let gateway_arn = direct_connect_arn(GLOBAL, GATEWAY_PATH, &gateway_id);

    // One key twice, and one key more than a resource may hold.
    let repeated: Vec<(&str, &str)> = vec![("twice", "first"), ("twice", "second")];
    let keys: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key-{index}"))
        .collect();
    let past_ceiling: Vec<(&str, &str)> = keys
        .iter()
        .map(|key| (key.as_str(), "one-too-many"))
        .collect();

    // (case, the operation, a request body built around `tags`)
    let writes: Vec<TagWordingRow> = vec![
        (
            "TagResource",
            "TagResource",
            Box::new({
                let arn = gateway_arn.clone();
                move |tags| json!({"resourceArn": &arn, "tags": tag_list(tags)})
            }),
        ),
        (
            "CreateDirectConnectGateway",
            "CreateDirectConnectGateway",
            Box::new(|tags| create_gateway_body("tag-wording-gw", None, tags)),
        ),
        (
            "CreateConnection",
            "CreateConnection",
            Box::new(|tags| {
                json!({
                    "connectionName": "tag-wording-conn",
                    "location": "EqDC2-EQ",
                    "bandwidth": "1Gbps",
                    "tags": tag_list(tags),
                })
            }),
        ),
        (
            "CreateResiliencyGroup",
            "CreateResiliencyGroup",
            Box::new(|tags| create_group_body("tag-wording-group", "basic-resiliency", tags, None)),
        ),
        (
            "CreatePrivateVirtualInterface",
            "CreatePrivateVirtualInterface",
            Box::new({
                let connection_id = connection_id.clone();
                move |tags| {
                    json!({
                        "connectionId": &connection_id,
                        "newPrivateVirtualInterface": {
                            "virtualInterfaceName": "tag-wording-vif",
                            "vlan": 303,
                            "asn": 65000,
                            "virtualGatewayId": "vgw-0123456789abcdef0",
                            "tags": tag_list(tags),
                        },
                    })
                }
            }),
        ),
    ];

    for (case, target, body_of) in &writes {
        // The error CODE is whatever each operation's model declares for the fault
        // (`CreateDirectConnectGateway` declares no tag-specific error, so it answers the
        // one client fault it has); what every one of them must share is the SENTENCE.
        let (status, refused) = dx.call(target, body_of(&repeated)).await;
        assert_eq!(
            (status, error_message(&refused)),
            (400, "Tag twice is specified more than once.".to_string()),
            "case {case:?}: expected the one duplicate-key sentence, got {status} with {refused}"
        );

        let (status, refused) = dx.call(target, body_of(&past_ceiling)).await;
        assert_eq!(
            (status, error_message(&refused)),
            (
                400,
                "You have reached the limit on the number of tags that can be assigned."
                    .to_string()
            ),
            "case {case:?}: expected the one tag-ceiling sentence, got {status} with {refused}"
        );
    }

    // Neither refusal stored anything: the gateway still holds no tags, and no resource of
    // the refused creates was minted.
    assert_eq!(
        tags_of(&dx, &gateway_arn).await,
        tag_list(&[]),
        "a refused write must leave the resource untagged"
    );
    let (listed, _) = describe(&dx, json!({})).await;
    let named: Vec<Value> = listed
        .iter()
        .map(|held| held["directConnectGatewayName"].clone())
        .collect();
    assert!(
        !named.contains(&json!("tag-wording-gw")),
        "a refused create must store no gateway, got {named:?}"
    );
}

#[tokio::test]
async fn a_gateway_id_no_gateway_exists_under_is_refused_with_the_one_stated_sentence() {
    let dx = Emulator::new();
    let connection_id = connection(
        &dx,
        "unresolved-gateway-connection",
        "EqDC2-EQ",
        "1Gbps",
    )
    .await;

    // Every operation of this service that resolves a `directConnectGatewayId`, against ids
    // of each form that names no stored gateway: a UUID nobody holds, an id of another
    // resource of this service, and a value that is no gateway id at all.
    let unknown_ids: &[(&str, &str)] = &[
        (
            "a UUID nobody holds",
            "00000000-0000-4000-8000-000000000000",
        ),
        ("another resource's id", "dxcon-00000000"),
        ("a value that is no gateway id", "not-a-gateway-id"),
    ];

    for (id_case, gateway_id) in unknown_ids {
        let rows: Vec<UnresolvedRow> = vec![
            (
                "DeleteDirectConnectGateway",
                "DeleteDirectConnectGateway",
                json!({"directConnectGatewayId": gateway_id}),
            ),
            (
                "UpdateDirectConnectGateway",
                "UpdateDirectConnectGateway",
                json!({
                    "directConnectGatewayId": gateway_id,
                    "newDirectConnectGatewayName": "renamed",
                }),
            ),
            (
                "CreatePrivateVirtualInterface, which terminates on the gateway",
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": &connection_id,
                    "newPrivateVirtualInterface": {
                        "virtualInterfaceName": "unresolved-gateway-vif",
                        "vlan": 222,
                        "asn": 65000,
                        "directConnectGatewayId": gateway_id,
                    },
                }),
            ),
        ];

        for (case, target, body) in rows {
            let (status, response) = dx.call(target, body).await;
            assert_eq!(
                (status, error_code(&response), error_message(&response)),
                (
                    400,
                    "DirectConnectClientException".to_string(),
                    gateway_not_found(gateway_id)
                ),
                "case {case:?} with {id_case:?}: expected the one stated refusal, got {status} \
                 with {response}"
            );
        }
    }

    // A gateway the account DOES hold is refused by none of them, so the refusal is about
    // the id resolving to nothing rather than about the request's shape.
    let held = gateway(&dx, "resolved-gateway").await;
    let (status, renamed) = dx
        .call(
            "UpdateDirectConnectGateway",
            json!({
                "directConnectGatewayId": &held,
                "newDirectConnectGatewayName": "resolved-gateway-renamed",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a gateway the account holds\": expected the rename to succeed, got {status} with \
         {renamed}"
    );
}

/// The Smithy SERVICE shape name of this service
/// (`smithy_query(query="service_info")`): the `{Service}` label of an `rpcv2Cbor` URI
/// (`/service/{Service}/operation/{Operation}`) and the prefix an awsJson1_1
/// `X-Amz-Target` carries.
const SERVICE_SHAPE: &str = "OvertureService";

/// The two headers an `rpcv2Cbor` exchange of this service is labelled with.
const CBOR_MEDIA_TYPE: &str = "application/cbor";
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";

/// The media type an `awsJson1_1` exchange of this service is labelled with.
const JSON_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// One emulator reached over EITHER wire protocol this model declares — `awsJson1_1` and
/// `rpcv2Cbor` (`smithy_query(query="service_info")`) — over ONE router and ONE engine.
///
/// Separate from the suite's shared [`Emulator`], which posts awsJson1_1 at `POST /` and
/// nothing else: what is pinned below is that the declared protocols are two ENCODINGS of
/// one API over one store, so the harness has to be able to send a request either way and
/// read the answer back in the form it was asked for.
struct EitherProtocol {
    router: smithy_server_runtime::MultiServiceRouter,
}

/// The credential scope of this service, which is how a request names the service that
/// claims it; the signature itself is not verified by the test router.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/us-east-1/directconnect/aws4_request, \
     SignedHeaders=host, Signature=0000"
        .to_string()
}

/// What one exchange answered: its status, the two labels the envelope carries, and the
/// body as it came off the wire.
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: bytes::Bytes,
}

impl EitherProtocol {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        Answer {
            status: response.status().as_u16(),
            content_type: header("content-type"),
            smithy_protocol: header("smithy-protocol"),
            body: bytes::Bytes::copy_from_slice(response.body()),
        }
    }

    /// One `rpcv2Cbor` request: the operation in the URI, the input encoded by the SAME
    /// codec the protocol specifies, and the labels that name the protocol.
    async fn over_cbor<I: aws_smithy_schema::serde::SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &aws_smithy_schema::Schema,
        input: &I,
    ) -> Answer {
        use aws_smithy_schema::codec::{Codec, FinishSerializer};
        use aws_smithy_schema::serde::ShapeSerializer;

        let codec = aws_smithy_schema_server::CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
            .header("content-type", CBOR_MEDIA_TYPE)
            .header("smithy-protocol", CBOR_PROTOCOL)
            .header("authorization", authorization())
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        self.dispatch(request).await
    }

    /// One `awsJson1_1` request: the operation in `X-Amz-Target` at `POST /`.
    async fn over_json(&self, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://directconnect.amazonaws.com/")
            .header("content-type", JSON_MEDIA_TYPE)
            .header("x-amz-target", format!("{SERVICE_SHAPE}.{operation}"))
            .header("authorization", authorization())
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed awsJson1_1 request");
        self.dispatch(request).await
    }
}

impl Answer {
    /// The body decoded as the CBOR an `rpcv2Cbor` response carries, through the generated
    /// schema of the shape the operation answers.
    fn cbor<T: aws_smithy_schema_server::DeserializableStruct>(&self, case: &str) -> T {
        use aws_smithy_schema::codec::Codec;

        let codec = aws_smithy_schema_server::CborCodec::default();
        let mut deserializer = codec.create_deserializer(&self.body);
        T::deserialize(&mut deserializer).unwrap_or_else(|error| {
            panic!("case {case:?}: the CBOR response does not deserialize: {error:?}")
        })
    }

    /// The body decoded as the JSON an `awsJson1_1` response carries.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The body as something an assertion can print, whichever encoding it is in.
    fn rendered(&self) -> String {
        match serde_json::from_slice::<Value>(&self.body) {
            Ok(body) => body.to_string(),
            Err(_) => format!("{:?}", self.body),
        }
    }
}

#[tokio::test]
async fn a_gateway_is_created_and_read_back_over_either_declared_protocol() {
    // RULE: the protocols a service declares are ENCODINGS of one API over one store, so
    // a request reaches the same handler and the same stored record whichever of them it
    // is sent in, and the answer comes back labelled with the protocol the request named.
    // This model declares `awsJson1_1` and `rpcv2Cbor`, so a gateway created over either
    // is one gateway, readable over both.
    //
    // The rows differ along which protocol WRITES and which READS, and along whether the
    // create names an Amazon-side ASN, since neither changes the record.
    //
    // (case, create over CBOR, amazonSideAsn)
    let rows: &[(&str, bool, Option<i64>)] = &[
        ("created over rpcv2Cbor, nothing optional set", true, None),
        ("created over rpcv2Cbor with its own ASN", true, Some(64_800)),
        ("created over awsJson1_1 with its own ASN", false, Some(64_801)),
    ];

    for (case, create_over_cbor, amazon_side_asn) in rows {
        let dx = EitherProtocol::new();
        let name = format!("either-protocol-{}", case.replace(' ', "-"));

        let (id, state, asn) = if *create_over_cbor {
            let answer = dx
                .over_cbor(
                    "CreateDirectConnectGateway",
                    CREATE_DIRECT_CONNECT_GATEWAY_INPUT_SCHEMA,
                    &CreateDirectConnectGatewayInput {
                        direct_connect_gateway_name: name.clone(),
                        amazon_side_asn: *amazon_side_asn,
                        tags: None,
                    },
                )
                .await;
            assert_eq!(
                (
                    answer.status,
                    answer.content_type.as_str(),
                    answer.smithy_protocol.as_str()
                ),
                (200, CBOR_MEDIA_TYPE, CBOR_PROTOCOL),
                "case {case:?}: expected the create to succeed in the rpcv2Cbor envelope, got \
                 {} with {} / {} and body {}",
                answer.status,
                answer.content_type,
                answer.smithy_protocol,
                answer.rendered()
            );
            let created: CreateDirectConnectGatewayOutput = answer.cbor(case);
            let gateway = created
                .direct_connect_gateway
                .unwrap_or_else(|| panic!("case {case:?}: the create answered no gateway"));
            (
                gateway.direct_connect_gateway_id.unwrap_or_default(),
                gateway.direct_connect_gateway_state.unwrap_or_default(),
                gateway.amazon_side_asn,
            )
        } else {
            let mut body = Map::new();
            body.insert("directConnectGatewayName".to_string(), json!(&name));
            if let Some(asn) = amazon_side_asn {
                body.insert("amazonSideAsn".to_string(), json!(asn));
            }
            let answer = dx
                .over_json("CreateDirectConnectGateway", Value::Object(body))
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the create to succeed, got {} with {}",
                answer.status,
                answer.rendered()
            );
            let created = answer.json()["directConnectGateway"].clone();
            (
                created["directConnectGatewayId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                created["directConnectGatewayState"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                created["amazonSideAsn"].as_i64(),
            )
        };

        let expected_asn = amazon_side_asn.unwrap_or(DEFAULT_AMAZON_SIDE_ASN);
        assert_eq!(
            (state.as_str(), asn),
            ("available", Some(expected_asn)),
            "case {case:?}: expected the create to report the gateway ready on its ASN, got \
             {state:?} and {asn:?}"
        );
        assert!(
            is_minted_gateway_id(&json!(&id)),
            "case {case:?}: expected a server-minted gateway id, got {id:?}"
        );

        // The OTHER protocol reads the same record back: one store, two encodings.
        let read_over_cbor = !*create_over_cbor;
        let (read_id, read_name, read_asn) = if read_over_cbor {
            let answer = dx
                .over_cbor(
                    "DescribeDirectConnectGateways",
                    DESCRIBE_DIRECT_CONNECT_GATEWAYS_INPUT_SCHEMA,
                    &DescribeDirectConnectGatewaysInput {
                        direct_connect_gateway_id: Some(id.clone()),
                        ..Default::default()
                    },
                )
                .await;
            assert_eq!(
                (
                    answer.status,
                    answer.content_type.as_str(),
                    answer.smithy_protocol.as_str()
                ),
                (200, CBOR_MEDIA_TYPE, CBOR_PROTOCOL),
                "case {case:?}: expected the read to succeed in the rpcv2Cbor envelope, got {} \
                 with {} / {} and body {}",
                answer.status,
                answer.content_type,
                answer.smithy_protocol,
                answer.rendered()
            );
            let described: DescribeDirectConnectGatewaysOutput = answer.cbor(case);
            let mut held = described.direct_connect_gateways.unwrap_or_default();
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected exactly the named gateway, got {held:?}"
            );
            let gateway = held.remove(0);
            (
                gateway.direct_connect_gateway_id.unwrap_or_default(),
                gateway.direct_connect_gateway_name.unwrap_or_default(),
                gateway.amazon_side_asn,
            )
        } else {
            let answer = dx
                .over_json(
                    "DescribeDirectConnectGateways",
                    json!({"directConnectGatewayId": &id}),
                )
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the read to succeed, got {} with {}",
                answer.status,
                answer.rendered()
            );
            let held = answer.json()["directConnectGateways"].clone();
            assert_eq!(
                held.as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected exactly the named gateway, got {held}"
            );
            (
                held[0]["directConnectGatewayId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                held[0]["directConnectGatewayName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                held[0]["amazonSideAsn"].as_i64(),
            )
        };

        assert_eq!(
            (read_id.as_str(), read_name.as_str(), read_asn),
            (id.as_str(), name.as_str(), Some(expected_asn)),
            "case {case:?}: expected the other protocol to read the same record back, got \
             {read_id:?}, {read_name:?} and {read_asn:?}"
        );
    }
}

