// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DRS launch action lifecycle: `PutLaunchAction`,
//! `ListLaunchActions` and `DeleteLaunchAction`.
//!
//! Every test drives the service the way the router does — one buffered `http::Request`
//! per call through [`MultiServiceRouter::dispatch`] — so what is asserted is the wire
//! behaviour (status, error code, JSON body), not a handler's Rust return value. The
//! harness (the router, `call`, `items`, `Answer`) is the one
//! `replication_configuration_template_lifecycle` builds.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A put persists every member the request supplied onto the resource it named, keyed
//!    by the action id it named, reports the STORED action — including the `type` of the
//!    SSM document its action code names, which no member of the request supplies — and a
//!    following list reads the same action back.
//! 2. A put of an action id the resource already carries REPLACES that action: the
//!    resource carries one action under that id, with the values the later put described.
//! 3. The `@paginated` list walks each of the resource's actions exactly once, in the
//!    order they run (`order` ascending, then `actionId`), for any `maxResults`; the
//!    `actionIds` filter selects among them; and no read changes what a later caller sees.
//! 4. A delete makes the action stop being reported; the same action id again, and an
//!    action id the resource never carried, is the modelled `ResourceNotFoundException`.
//! 5. A `resourceId` naming a resource this service does not hold is that same modelled
//!    refusal for ALL THREE operations, naming the kind of resource the id addresses, and
//!    a refused put writes nothing.

use serde_json::{json, Map, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::{
    LAUNCH_ACTION_CATEGORY_VALUES, LAUNCH_ACTION_PARAMETER_TYPE_VALUES, LAUNCH_ACTION_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::replication_configuration_template_lifecycle::{call, drs, items, Answer};

/// The code the model declares for a resource this service does not hold
/// (`smithy_query(query="operation",operation="PutLaunchAction,ListLaunchActions,\
/// DeleteLaunchAction")` lists `ResourceNotFoundException`, HTTP 404).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The members of one launch action a list reports, i.e. every member of the modelled
/// `LaunchAction` shape (`smithy_query(query="shape",shape="LaunchAction")`). A put
/// reports these too, beside the `resourceId` the action hangs off.
const ACTION_MEMBERS: [&str; 11] = [
    "actionId",
    "actionCode",
    "type",
    "name",
    "active",
    "order",
    "actionVersion",
    "optional",
    "parameters",
    "description",
    "category",
];

/// A string member of a JSON object.
fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `actionId` of the modelled form
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`),
/// distinct per `seed`, so a test can name as many actions as it needs without a UUID
/// crate.
fn action_id(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-{seed:012x}")
}

/// A launch configuration template to hang actions off, created through the operation
/// that mints one, and its `lct-` id.
async fn resource(router: &MultiServiceRouter) -> String {
    let answer = call(router, "CreateLaunchConfigurationTemplate", &json!({})).await;
    assert_eq!(answer.status, 201, "create a template: got {answer:?}");
    let template = answer
        .member("launchConfigurationTemplate")
        .unwrap_or_else(|| panic!("expected a created template, got {answer:?}"));
    text(template, "launchConfigurationTemplateID")
}

/// A `PutLaunchAction` body: every member the operation requires, with `extra` merged
/// over them.
fn put_body(resource_id: &str, action_id: &str, extra: &Value) -> Value {
    let mut body = json!({
        "resourceId": resource_id,
        "actionId": action_id,
        "actionCode": "AWSEC2-CreateVssSnapshot",
        "actionVersion": "$DEFAULT",
        "name": "Create a snapshot",
        "description": "Takes a snapshot of the recovered instance",
        "category": LAUNCH_ACTION_CATEGORY_VALUES[0],
        "order": 2,
        "active": true,
        "optional": false,
    });
    let (Some(body_members), Some(extra_members)) = (body.as_object_mut(), extra.as_object()) else {
        panic!("both the body and the overlay are JSON objects");
    };
    for (member, value) in extra_members {
        body_members.insert(member.clone(), value.clone());
    }
    body
}

/// `PutLaunchAction` with `body`, asserting the modelled 200, and answer the action it
/// reported.
async fn put(router: &MultiServiceRouter, body: &Value) -> Value {
    let answer = call(router, "PutLaunchAction", body).await;
    assert_eq!(answer.status, 200, "put {body}: expected 200, got {answer:?}");
    answer.body
}

/// `ListLaunchActions` with `body`, asserting the modelled 200.
async fn list(router: &MultiServiceRouter, body: &Value) -> Answer {
    let answer = call(router, "ListLaunchActions", body).await;
    assert_eq!(
        answer.status, 200,
        "list {body}: expected 200, got {answer:?}"
    );
    answer
}

/// Every action a resource carries, walking every page of the list.
async fn all_actions(router: &MultiServiceRouter, resource_id: &str) -> Vec<Value> {
    let mut actions = Vec::new();
    let mut body = json!({"resourceId": resource_id});
    loop {
        let answer = list(router, &body).await;
        actions.extend(items(&answer).iter().cloned());
        match answer.member("nextToken").and_then(Value::as_str) {
            Some(token) => body = json!({"resourceId": resource_id, "nextToken": token}),
            None => return actions,
        }
    }
}

/// A parameter map of `count` entries, each carrying the parameter type at `type_index`
/// of the modelled set, built as values the model's `LaunchActionParameterName`
/// (`^([A-Za-z0-9])+$`) and `LaunchActionParameterValue` (`^[A-Za-z0-9.-]+$`) admit.
fn parameters(count: usize, type_index: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(
            format!("parameter{index}"),
            json!({
                "value": format!("value-{index}.0"),
                "type": LAUNCH_ACTION_PARAMETER_TYPE_VALUES[type_index
                    % LAUNCH_ACTION_PARAMETER_TYPE_VALUES.len()],
            }),
        );
    }
    Value::Object(map)
}

/// The puts rule 1 spans: the rows differ along which optional member they supply, which
/// value each required member carries, and how many parameters the action has. Every
/// modelled enumeration is covered at EVERY one of its values, from the value sets the
/// model generated (`crate::…::ops::types::{LAUNCH_ACTION_CATEGORY_VALUES,
/// LAUNCH_ACTION_PARAMETER_TYPE_VALUES}`), and each bound of a constrained member is a
/// row of its own: `order` at 2 and at 10000 (`range(min: 2, max: 10000)`), `name` at 1
/// and at 256 characters (`length(min: 1, max: 256)`), `description` at 0 and at 1024
/// (`length(min: 0, max: 1024)`), and `parameters` at 0 and at 20 entries
/// (`length(min: 0, max: 20)`).
///
/// Each row carries the `type` the action must report, written out as the literal a client
/// reads (one of the values the model's `LaunchActionType` lists) rather than recomputed
/// from the action code, so a wrong reading of the document name cannot satisfy the row
/// that pins it.
fn put_rows() -> Vec<(String, Value, &'static str)> {
    /// The two values the model's `LaunchActionType` lists, written out here as a client
    /// reads them off the wire.
    const AUTOMATION: &str = "SSM_AUTOMATION";
    const COMMAND: &str = "SSM_COMMAND";

    // Every row below leaves `actionCode` at the Automation runbook `put_body` supplies,
    // except the two that name a document of their own.
    let mut rows: Vec<(String, Value, &'static str)> = vec![
        (
            "only the required members".to_string(),
            json!({}),
            AUTOMATION,
        ),
        (
            "the least order the model admits".to_string(),
            json!({"order": 2}),
            AUTOMATION,
        ),
        (
            "the greatest order the model admits".to_string(),
            json!({"order": 10000}),
            AUTOMATION,
        ),
        (
            "an order in between".to_string(),
            json!({"order": 5_000}),
            AUTOMATION,
        ),
        (
            "the shortest name the model admits".to_string(),
            json!({"name": "A"}),
            AUTOMATION,
        ),
        (
            "the longest name the model admits".to_string(),
            json!({"name": "N".repeat(256)}),
            AUTOMATION,
        ),
        (
            "the empty description".to_string(),
            json!({"description": ""}),
            AUTOMATION,
        ),
        (
            "the longest description the model admits".to_string(),
            json!({"description": "d".repeat(1024)}),
            AUTOMATION,
        ),
        (
            "an inactive, optional action".to_string(),
            json!({"active": false, "optional": true}),
            AUTOMATION,
        ),
        (
            "an active, non-optional action".to_string(),
            json!({"active": true, "optional": false}),
            AUTOMATION,
        ),
        (
            "an inactive, non-optional action".to_string(),
            json!({"active": false, "optional": false}),
            AUTOMATION,
        ),
        (
            "no parameter at all".to_string(),
            json!({"parameters": parameters(0, 0)}),
            AUTOMATION,
        ),
        (
            "the greatest number of parameters the model admits".to_string(),
            json!({"parameters": parameters(20, 1)}),
            AUTOMATION,
        ),
        (
            "a pinned action version".to_string(),
            json!({"actionVersion": "7"}),
            AUTOMATION,
        ),
        (
            "the latest action version".to_string(),
            json!({"actionVersion": "$LATEST"}),
            AUTOMATION,
        ),
        (
            "a Run Command document".to_string(),
            json!({"actionCode": "AWS-RunShellScript"}),
            COMMAND,
        ),
        (
            "another Run Command document".to_string(),
            json!({"actionCode": "AWS-RunPowerShellScript"}),
            COMMAND,
        ),
        (
            "an Automation runbook of another namespace".to_string(),
            json!({"actionCode": "AWSDisasterRecovery-InstallDRAgentOnInstance"}),
            AUTOMATION,
        ),
        (
            "an Automation runbook of the AWS namespace".to_string(),
            json!({"actionCode": "AWS-CreateImage"}),
            AUTOMATION,
        ),
    ];
    for category in LAUNCH_ACTION_CATEGORY_VALUES {
        rows.push((
            format!("category {category}"),
            json!({"category": category}),
            AUTOMATION,
        ));
    }
    for (index, parameter_type) in LAUNCH_ACTION_PARAMETER_TYPE_VALUES.iter().enumerate() {
        rows.push((
            format!("one {parameter_type} parameter"),
            json!({"parameters": parameters(1, index)}),
            AUTOMATION,
        ));
    }
    rows
}

// ---------------------------------------------------------------------------
// Rule 1 — a put persists what it was given, and the list returns it
// ---------------------------------------------------------------------------

/// Every member a put supplies is stored and reported, the action's `type` is the type of
/// the SSM document its action code names, and a list of that action id reads the same
/// action back.
#[tokio::test]
async fn a_put_persists_every_supplied_member_and_a_list_returns_it() {
    let router = drs();
    let resource_id = resource(&router).await;

    for (index, (case, extra, expected_type)) in put_rows().into_iter().enumerate() {
        let id = action_id(u32::try_from(index).expect("a small row index") + 1);
        let body = put_body(&resource_id, &id, &extra);
        let action = put(&router, &body).await;

        // The put reports the resource the action hangs off, and every member the
        // request named with the value it named.
        assert_eq!(
            text(&action, "resourceId"),
            resource_id,
            "case {case:?}: the put must report the resource it wrote to, got {action}"
        );
        for (member, value) in body.as_object().expect("a JSON object body") {
            assert_eq!(
                action.get(member),
                Some(value),
                "case {case:?}: expected {member} to be {value}, got {action}"
            );
        }

        // `type` is the one member no part of the request supplies: the type of the SSM
        // document the action code names, pinned per row rather than recomputed here.
        let action_code = text(&body, "actionCode");
        assert_eq!(
            text(&action, "type"),
            expected_type,
            "case {case:?}: wrong type for the document {action_code:?}, got {action}"
        );
        assert!(
            LAUNCH_ACTION_TYPE_VALUES.contains(&text(&action, "type").as_str()),
            "case {case:?}: the type must be one the model lists, got {action}"
        );

        // The stored action, read back through the operation that reports it: every
        // member of the modelled action, as the put reported it.
        let answer = list(
            &router,
            &json!({"resourceId": resource_id, "filters": {"actionIds": [id]}}),
        )
        .await;
        let reported = items(&answer);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the action just put, got {answer:?}"
        );
        for member in ACTION_MEMBERS {
            assert_eq!(
                reported[0].get(member),
                action.get(member),
                "case {case:?}: the list and the put disagree about {member}: \
                 {:?} vs {:?}",
                reported[0].get(member),
                action.get(member)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a put of an action id the resource carries replaces it
// ---------------------------------------------------------------------------

/// A second put under the same action id replaces the action rather than adding one: the
/// resource still carries exactly one action under that id, carrying the later put's
/// values. The rows span the members a replacement can change.
#[tokio::test]
async fn a_put_of_a_held_action_id_replaces_that_action() {
    let router = drs();
    let resource_id = resource(&router).await;
    let id = action_id(1);

    let replacements = [
        ("a renamed action", json!({"name": "Renamed action"})),
        ("another order", json!({"order": 99})),
        (
            "another category",
            json!({"category": LAUNCH_ACTION_CATEGORY_VALUES[2]}),
        ),
        (
            "another document, of the other type",
            json!({"actionCode": "AWS-RunPowerShellScript"}),
        ),
        (
            "parameters where there were none",
            json!({"parameters": parameters(3, 0)}),
        ),
        ("deactivated", json!({"active": false, "optional": true})),
    ];

    // The action as first put: the rows below each replace it, and the resource must
    // carry the LATER action under the one id rather than both.
    let first = put(&router, &put_body(&resource_id, &id, &json!({}))).await;
    assert_eq!(
        all_actions(&router, &resource_id).await.len(),
        1,
        "the first put must leave the resource carrying exactly one action"
    );
    assert_eq!(
        text(&first, "actionId"),
        id,
        "the first put must report the action id it was given, got {first}"
    );

    for (case, extra) in replacements {
        let expected = put(&router, &put_body(&resource_id, &id, &extra)).await;
        let held = all_actions(&router, &resource_id).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: a put of a held action id must not add an action, got {held:?}"
        );
        for member in ACTION_MEMBERS {
            assert_eq!(
                held[0].get(member),
                expected.get(member),
                "case {case:?}: expected {member} to be {:?}, got {:?}",
                expected.get(member),
                held[0].get(member)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — the paginated list, its order and its filter
// ---------------------------------------------------------------------------

/// The list walks every action of the resource exactly once, in the order they run,
/// whatever `maxResults` asks for; the filter selects; and no read changes what a later
/// caller sees. Two actions are given the SAME order, so the tie-break is exercised too.
#[tokio::test]
async fn the_list_walks_every_action_once_in_run_order_for_any_page_size() {
    let router = drs();
    let resource_id = resource(&router).await;

    // Put them in an order that is NOT the order they run in, so a list that reported
    // insertion order would differ from the expected sequence below.
    let orders: [(u32, i64); 5] = [(1, 40), (2, 10), (3, 30), (4, 10), (5, 20)];
    for (seed, order) in orders {
        put(
            &router,
            &put_body(&resource_id, &action_id(seed), &json!({"order": order})),
        )
        .await;
    }
    // `order` ascending, then `actionId` for the two sharing an order.
    let mut expected: Vec<(i64, String)> = orders
        .iter()
        .map(|(seed, order)| (*order, action_id(*seed)))
        .collect();
    expected.sort();
    let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();

    for max_results in [1, 2, 3, 5, 1000] {
        let mut seen = Vec::new();
        let mut body = json!({"resourceId": resource_id, "maxResults": max_results});
        loop {
            let answer = list(&router, &body).await;
            let page = items(&answer);
            assert!(
                page.len() <= usize::try_from(max_results).expect("a positive page size"),
                "case maxResults={max_results}: a page carried {} items, got {answer:?}",
                page.len()
            );
            for item in page {
                seen.push(text(item, "actionId"));
            }
            match answer.member("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    body = json!({
                        "resourceId": resource_id,
                        "maxResults": max_results,
                        "nextToken": token,
                    });
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={max_results}: expected every action once, in run order"
        );
    }

    // The filter selects among the resource's actions, in the same run order; an id the
    // resource does not carry selects nothing rather than refusing the read.
    let filter_rows: Vec<(String, Vec<String>, Vec<String>)> = vec![
        (
            "no id at all".to_string(),
            Vec::new(),
            expected.clone(),
        ),
        (
            "one held id".to_string(),
            vec![action_id(3)],
            vec![action_id(3)],
        ),
        (
            "several held ids, named out of run order".to_string(),
            vec![action_id(1), action_id(2)],
            vec![action_id(2), action_id(1)],
        ),
        (
            "an id the resource does not carry".to_string(),
            vec![action_id(99)],
            Vec::new(),
        ),
        (
            "a held id beside one the resource does not carry".to_string(),
            vec![action_id(99), action_id(5)],
            vec![action_id(5)],
        ),
    ];
    for (case, ids, wanted) in filter_rows {
        let answer = list(
            &router,
            &json!({"resourceId": resource_id, "filters": {"actionIds": ids}}),
        )
        .await;
        let reported: Vec<String> = items(&answer)
            .iter()
            .map(|item| text(item, "actionId"))
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?}, got {answer:?}"
        );
    }

    // Every read above left the resource's actions as they were.
    let still: Vec<String> = all_actions(&router, &resource_id)
        .await
        .iter()
        .map(|action| text(action, "actionId"))
        .collect();
    assert_eq!(
        still, expected,
        "the reads must have changed nothing a later caller sees"
    );
}

/// A `nextToken` the service cannot resume from is refused as a bad request — HTTP 400,
/// error code `ValidationException` — rather than restarting the walk. The rows span the
/// ways a token can fail to name a page: text that is not a token at all, a token of
/// another paginated operation of this service, and a token naming an action the resource
/// has since given up.
#[tokio::test]
async fn a_next_token_the_service_cannot_resume_from_is_refused() {
    let router = drs();
    let resource_id = resource(&router).await;
    // A second template, so `DescribeLaunchConfigurationTemplates` has a page to follow
    // and therefore a token of its own to offer the rows below.
    let _other_resource = resource(&router).await;
    for seed in 1..=3 {
        put(
            &router,
            &put_body(&resource_id, &action_id(seed), &json!({})),
        )
        .await;
    }

    // A token of this operation, for a page that stops existing when its action does.
    let answer = list(
        &router,
        &json!({"resourceId": resource_id, "maxResults": 1}),
    )
    .await;
    let stale = answer
        .member("nextToken")
        .and_then(Value::as_str)
        .expect("a page of one of three actions is followed by another")
        .to_string();
    for seed in 1..=3 {
        let answer = call(
            &router,
            "DeleteLaunchAction",
            &json!({"resourceId": resource_id, "actionId": action_id(seed)}),
        )
        .await;
        assert_eq!(answer.status, 204, "delete: got {answer:?}");
    }
    // The resource must still exist for the read to be about the token rather than the
    // resource, so one action goes back on it.
    put(
        &router,
        &put_body(&resource_id, &action_id(7), &json!({})),
    )
    .await;

    // A token minted by another paginated operation of this service.
    let other = call(
        &router,
        "DescribeLaunchConfigurationTemplates",
        &json!({"maxResults": 1}),
    )
    .await;
    let foreign = other
        .member("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut rows = vec![
        ("text that is not a token", "not-a-token".to_string()),
        ("the empty token", String::new()),
        ("a token of a page that no longer exists", stale),
    ];
    if let Some(foreign) = foreign {
        rows.push(("a token of another paginated operation", foreign));
    }

    for (case, token) in rows {
        let answer = call(
            &router,
            "ListLaunchActions",
            &json!({"resourceId": resource_id, "nextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for {token:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 — a delete, and an action id the resource does not carry
// ---------------------------------------------------------------------------

/// A delete answers the modelled empty 204 and the action stops being reported; the same
/// id again, and an id the resource never carried, is the modelled
/// `ResourceNotFoundException` naming the action the request asked for.
#[tokio::test]
async fn a_delete_makes_the_action_stop_being_reported() {
    let router = drs();
    let resource_id = resource(&router).await;
    let keep = action_id(1);
    let gone = action_id(2);
    put(&router, &put_body(&resource_id, &keep, &json!({}))).await;
    put(&router, &put_body(&resource_id, &gone, &json!({}))).await;

    let answer = call(
        &router,
        "DeleteLaunchAction",
        &json!({"resourceId": resource_id, "actionId": gone}),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "deleting {gone}: expected 204, got {answer:?}"
    );

    let held: Vec<String> = all_actions(&router, &resource_id)
        .await
        .iter()
        .map(|action| text(action, "actionId"))
        .collect();
    assert_eq!(
        held,
        vec![keep.clone()],
        "the deleted action must stop being reported"
    );

    let never_carried = action_id(42);
    for (case, id) in [
        ("an id the resource has given up", gone.as_str()),
        ("an id the resource never carried", never_carried.as_str()),
    ] {
        let answer = call(
            &router,
            "DeleteLaunchAction",
            &json!({"resourceId": resource_id, "actionId": id}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {id}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE}, got {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "resourceId"),
            id,
            "case {case:?}: resourceId must be the id the request named, got {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "message"),
            format!("Launch Action with Id: '{id}' was not found"),
            "case {case:?}: wrong message in {answer:?}"
        );
    }

    // Deleting the last action leaves the resource reporting none, and the resource
    // itself still there.
    let answer = call(
        &router,
        "DeleteLaunchAction",
        &json!({"resourceId": resource_id, "actionId": keep}),
    )
    .await;
    assert_eq!(answer.status, 204, "deleting {keep}: got {answer:?}");
    assert!(
        all_actions(&router, &resource_id).await.is_empty(),
        "a resource whose last action was deleted reports none"
    );
}

// ---------------------------------------------------------------------------
// Rule 5 — a resourceId naming no resource this service holds
// ---------------------------------------------------------------------------

/// All three operations refuse a `resourceId` this service does not hold with the
/// modelled `ResourceNotFoundException`, naming the kind of resource the id addresses —
/// the model's `LaunchActionResourceId` admits a launch configuration template id and a
/// source server id — and a refused put writes nothing. The rows span the ids such a
/// request can name: a template id nothing minted, a template id the store has given up,
/// and a source server id (no agent registers a source server in this emulator, so none
/// is held).
#[tokio::test]
async fn a_resource_id_naming_no_resource_is_refused_by_every_launch_action_operation() {
    let router = drs();
    // One resource that IS held, carrying one action, so the refusals below can be shown
    // to write nothing.
    let held_resource = resource(&router).await;
    let held_action = action_id(1);
    let present = put(&router, &put_body(&held_resource, &held_action, &json!({}))).await;

    let given_up = resource(&router).await;
    let answer = call(
        &router,
        "DeleteLaunchConfigurationTemplate",
        &json!({"launchConfigurationTemplateID": given_up}),
    )
    .await;
    assert_eq!(answer.status, 204, "deleting {given_up}: got {answer:?}");

    let rows: [(&str, &str, &str); 3] = [
        (
            "a template id nothing minted",
            "lct-00000000000000000",
            "Launch Configuration Template",
        ),
        (
            "a template id the store has given up",
            given_up.as_str(),
            "Launch Configuration Template",
        ),
        (
            "a source server id, none being registered",
            "s-0123456789abcdefg",
            "Source Server",
        ),
    ];

    for (case, resource_id, resource_type) in rows {
        let calls = [
            (
                "PutLaunchAction",
                put_body(resource_id, &action_id(5), &json!({})),
            ),
            ("ListLaunchActions", json!({"resourceId": resource_id})),
            (
                "DeleteLaunchAction",
                json!({"resourceId": resource_id, "actionId": action_id(5)}),
            ),
        ];
        for (operation, body) in calls {
            let answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} / {operation}: expected 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some(NOT_FOUND_CODE),
                "case {case:?} / {operation}: expected {NOT_FOUND_CODE}, got {answer:?}"
            );
            assert_eq!(
                text(&answer.body, "resourceId"),
                resource_id,
                "case {case:?} / {operation}: resourceId must be the id the request \
                 named, got {answer:?}"
            );
            assert_eq!(
                text(&answer.body, "resourceType"),
                resource_type,
                "case {case:?} / {operation}: wrong resourceType in {answer:?}"
            );
            assert_eq!(
                text(&answer.body, "message"),
                format!("{resource_type} with Id: '{resource_id}' was not found"),
                "case {case:?} / {operation}: wrong message in {answer:?}"
            );
        }
    }

    // Nothing was written for any refused row: the one action put above is still the only
    // one the held resource carries, unchanged.
    let held = all_actions(&router, &held_resource).await;
    assert_eq!(
        held.len(),
        1,
        "a refused put must leave the store exactly as it was, got {held:?}"
    );
    for member in ACTION_MEMBERS {
        assert_eq!(
            held[0].get(member),
            present.get(member),
            "the held action's {member} changed: expected {:?}, got {:?}",
            present.get(member),
            held[0].get(member)
        );
    }
}
