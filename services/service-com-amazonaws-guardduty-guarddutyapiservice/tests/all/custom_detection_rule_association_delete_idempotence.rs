// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomDetectionRuleAssociation` is `@idempotent`: the RESPONSE, not only the
//! store, is the same on every replay.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test: an `@idempotent` delete answers the success its model declares
//! — HTTP 200 with the empty response body
//! (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`) — for EVERY
//! well-formed pair of URI labels, however many times it is sent and whether or not a
//! resource is behind them, because the post-condition the request asks for already
//! holds once nothing resolves. Idempotence is a property of this operation, not of the
//! URI: the non-idempotent create and the operations that REPORT or CHANGE a resource
//! (`Get`, `Update`) still answer the modelled `ResourceNotFoundException` for the same
//! absent pair.
//!
//! The `custom_detection_rule_association_delete` suite asserts what a delete removes;
//! this one asserts what a delete ANSWERS when there is nothing left to remove, over a
//! table generated across the label space the two members' `@length`/`@pattern` admit
//! rather than over one recorded pair.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call_bytes_in, call_in, error_code, error_shape, new_router, published_rule_ids, Router,
};

/// `CreateCustomDetectionRuleAssociation` and `ListCustomDetectionRuleAssociations` are
/// both bound at `/custom-detection-rule/association`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,ListCustomDetectionRuleAssociations")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The read, the update and the delete of an association are all bound at
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// The modelled error shape the read, the update and the create answer an absent
/// resource with
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,CreateCustomDetectionRuleAssociation")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so one page of
/// this size holds every association a case makes.
const MAX_RESULTS_MAX: i64 = 100;

/// How many times a case replays the same delete. More than two, so "the second call
/// is special" could not pass this suite.
const REPLAYS: usize = 4;

/// `AssociationId` is `len 1..64`, `^[a-zA-Z0-9_-]{1,64}$`
/// (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`): one value per
/// character class the pattern admits and per end of the length range, none of them a
/// value any create mints.
fn absent_association_ids() -> Vec<(String, String)> {
    vec![
        ("one lower-case character".to_string(), "a".to_string()),
        ("one upper-case character".to_string(), "Z".to_string()),
        ("one digit".to_string(), "7".to_string()),
        ("one underscore".to_string(), "_".to_string()),
        ("one hyphen".to_string(), "-".to_string()),
        (
            "every class the pattern admits".to_string(),
            "aZ_9-Qx".to_string(),
        ),
        (
            "the 32 hex characters a create mints, never minted".to_string(),
            "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        ),
        ("one below the length ceiling".to_string(), "b".repeat(63)),
        ("the length ceiling".to_string(), "c".repeat(64)),
        ("the length ceiling, all hyphens".to_string(), "-".repeat(64)),
    ]
}

/// `RuleId` is `len 1..100`, `^[a-z0-9]+(-[a-z0-9]+)*$`
/// (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`): one value per
/// form the pattern admits and per end of the length range. None is a rule the
/// catalogue publishes, so the pair cannot resolve however the ids are combined — a
/// published rule is covered separately, from [`published_rule_ids`].
fn unpublished_rule_ids() -> Vec<(String, String)> {
    vec![
        ("one lower-case character".to_string(), "a".to_string()),
        ("one digit".to_string(), "4".to_string()),
        (
            "two hyphen-joined segments".to_string(),
            "no-such".to_string(),
        ),
        (
            "four hyphen-joined segments".to_string(),
            "no-such-published-rule".to_string(),
        ),
        (
            "digits and letters in one segment".to_string(),
            "rule99".to_string(),
        ),
        ("the length ceiling".to_string(), "d".repeat(100)),
    ]
}

/// Enroll this Region's account in `rule_id` in `mode`, returning the minted
/// `AssociationId`.
async fn associate(router: &Router, rule_id: &str, mode: &str) -> String {
    let (status, body) = call_in(
        router,
        REGION,
        "POST",
        ASSOCIATIONS_PATH,
        Some(json!({"ruleId": rule_id, "mode": mode})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-association {rule_id}/{mode}: expected an associationId, got {body:?}"))
        .to_string()
}

/// The association URI the read, the update and the delete share.
fn association_path(rule_id: &str, association_id: &str) -> String {
    format!("{RULES_PATH}/{rule_id}/association/{association_id}")
}

/// Send the delete and hand back its status and the response body's RAW bytes, so a
/// replay can be compared to the first answer byte for byte.
async fn delete_raw(router: &Router, rule_id: &str, association_id: &str) -> (u16, String) {
    call_bytes_in(
        router,
        REGION,
        "DELETE",
        &association_path(rule_id, association_id),
        None,
    )
    .await
}

/// Assert one delete answered the success its model declares: HTTP 200 and a body that
/// carries no members, since `DeleteCustomDetectionRuleAssociationResponse` declares
/// none (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`).
fn assert_empty_success(case: &str, status: u16, raw: &str) {
    assert_eq!(
        status, 200,
        "case {case:?}: delete-association expected HTTP 200, got {status} with body {raw:?}"
    );
    let parsed: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw)
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {raw:?}"))
    };
    let members = parsed.as_object().map(|object| object.len()).unwrap_or(0);
    assert_eq!(
        members, 0,
        "case {case:?}: the response shape declares no members, got {raw:?}"
    );
}

/// Send the same delete [`REPLAYS`] times and assert every answer is the empty success
/// and is identical, byte for byte, to the first one.
async fn assert_delete_replays_identically(router: &Router, case: &str, rule_id: &str, id: &str) {
    let mut first: Option<(u16, String)> = None;
    for attempt in 1..=REPLAYS {
        let case = format!("{case}, delete attempt {attempt}");
        let (status, raw) = delete_raw(router, rule_id, id).await;
        assert_empty_success(&case, status, &raw);
        match &first {
            None => first = Some((status, raw)),
            Some((expected_status, expected_raw)) => assert_eq!(
                (status, raw.as_str()),
                (*expected_status, expected_raw.as_str()),
                "case {case:?}: an idempotent delete must answer what it answered first, {expected_status} {expected_raw:?}"
            ),
        }
    }
}

/// The `AssociationId`s this Region's endpoint reports, in page order.
async fn listed_ids(router: &Router, case: &str) -> Vec<String> {
    let (status, body) = call_in(
        router,
        REGION,
        "GET",
        &format!("{ASSOCIATIONS_PATH}?maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-associations expected 200, got {status} {body:?}"
    );
    body["ruleAssociations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ruleAssociations is @required, got {body:?}"))
        .iter()
        .map(|summary| {
            summary["associationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The whole read of an association, failing the case if the read did not succeed.
async fn read_association(router: &Router, case: &str, rule_id: &str, id: &str) -> Value {
    let (status, body) = call_in(
        router,
        REGION,
        "GET",
        &association_path(rule_id, id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-association expected 200, got {status} {body:?}"
    );
    body
}

/// Assert a response is the modelled refusal: the status, the shape in the body's
/// `__type`, in the `x-amzn-errortype` header and in the exception's own `type` member,
/// and the message GuardDuty words that refusal with.
fn assert_modelled_not_found(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        NOT_FOUND,
        "case {case:?}: expected {NOT_FOUND} in __type, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        NOT_FOUND,
        "case {case:?}: expected {NOT_FOUND} in x-amzn-errortype, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected message {expected_message:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete of a pair nothing resolves for is the empty success, however the two
// well-formed labels are spelled and however often it is sent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_pair_is_the_empty_success_however_the_labels_are_spelled() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let published = catalogue[0].clone();

    // One association this endpoint DOES hold, so every empty answer below can be
    // shown to have removed nothing.
    let held_rule = catalogue[1].clone();
    let held = associate(&router, &held_rule, MODES[0]).await;
    let before = read_association(&router, "the baseline", &held_rule, &held).await;
    let listed_before = listed_ids(&router, "the baseline").await;

    // The rule labels span the published catalogue entry and the forms the member's
    // pattern admits for a rule the catalogue does not carry; the association labels
    // span the pattern's character classes and both ends of its length range. No
    // combination names an association this endpoint holds.
    let mut rules = vec![("a published rule".to_string(), published)];
    rules.extend(
        unpublished_rule_ids()
            .into_iter()
            .map(|(form, rule_id)| (format!("an unpublished ruleId, {form}"), rule_id)),
    );

    for (rule_form, rule_id) in &rules {
        for (id_form, association_id) in absent_association_ids() {
            let case = format!("{rule_form} with an associationId of {id_form}");
            assert_delete_replays_identically(&router, &case, rule_id, &association_id).await;
        }
    }

    // Nothing above removed anything, and nothing above created anything either.
    let after = read_association(&router, "after the empty deletes", &held_rule, &held).await;
    assert_eq!(
        after, before,
        "a delete of a pair that resolves to nothing must change nothing, got {after:?}"
    );
    let listed_after = listed_ids(&router, "after the empty deletes").await;
    assert_eq!(
        listed_after, listed_before,
        "a delete of a pair that resolves to nothing must leave the listing alone, got {listed_after:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete that DID remove an association answers the same thing again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_delete_answers_what_the_first_delete_answered() {
    let catalogue = published_rule_ids(&new_router()).await;

    for rule_id in catalogue.iter().take(2) {
        for mode in MODES {
            // A store of its own per row: a rule holds at most one association, so a
            // row must not see what another row associated over the same rule.
            let router = new_router();
            let case = format!("{rule_id} in {mode}");
            let association_id = associate(&router, rule_id, mode).await;

            // The first delete removes the association, and every replay answers
            // exactly what it answered.
            assert_delete_replays_identically(&router, &case, rule_id, &association_id).await;

            // And the state the first delete left is the state the replays leave: the
            // read refuses, with the id the request carried after the fixed prefix,
            // and the listing no longer reports it.
            let (status, body) = call_in(
                &router,
                REGION,
                "GET",
                &association_path(rule_id, &association_id),
                None,
            )
            .await;
            assert_modelled_not_found(
                &format!("{case}: the read after the replayed deletes"),
                status,
                &body,
                &format!("Association not found: {association_id}"),
            );
            let listed = listed_ids(&router, &case).await;
            assert!(
                !listed.contains(&association_id),
                "case {case:?}: the list must not report a deleted association, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Idempotence belongs to the delete, not to the URI: the sibling operations keep
// refusing the same absent pair.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_sibling_operations_still_refuse_the_pair_the_delete_answers_emptily() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let published = catalogue[0].clone();

    // Rows differing in WHY the pair resolves to nothing: an id no create minted under
    // a published rule, and a well-formed rule the catalogue does not publish — plus,
    // below, an association that WAS there and has been deleted.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for (id_form, association_id) in absent_association_ids() {
        rows.push((
            format!("a published rule with an associationId of {id_form}"),
            published.clone(),
            association_id,
        ));
    }
    for (rule_form, rule_id) in unpublished_rule_ids() {
        rows.push((
            format!("an unpublished ruleId, {rule_form}"),
            rule_id,
            "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        ));
    }
    // The pair of an association this endpoint held and then deleted.
    let deleted_rule = catalogue[1].clone();
    let deleted = associate(&router, &deleted_rule, MODES[1]).await;
    let (status, raw) = delete_raw(&router, &deleted_rule, &deleted).await;
    assert_empty_success("the association this endpoint held", status, &raw);
    rows.push((
        "an association this endpoint held and deleted".to_string(),
        deleted_rule,
        deleted,
    ));

    for (case, rule_id, association_id) in &rows {
        let expected = format!("Association not found: {association_id}");
        let path = association_path(rule_id, association_id);

        // The read REPORTS a resource, so an absent one is the modelled refusal.
        let (status, body) = call_in(&router, REGION, "GET", &path, None).await;
        assert_modelled_not_found(&format!("{case}: the read"), status, &body, &expected);

        // The update CHANGES a resource, so an absent one is the same refusal — even
        // though it, too, is `@idempotent`
        // (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")`):
        // idempotence means a replay answers the same thing, not that an absent
        // resource is a success.
        for mode in MODES {
            let (status, body) = call_in(
                &router,
                REGION,
                "PUT",
                &path,
                Some(json!({"mode": mode})),
            )
            .await;
            assert_modelled_not_found(
                &format!("{case}: the update to {mode}"),
                status,
                &body,
                &expected,
            );
        }

        // And the delete of that very pair is still the empty success, so the rows
        // above are refused for the resource being absent rather than for anything the
        // refusals changed.
        assert_delete_replays_identically(&router, case, rule_id, association_id).await;
    }

    // The create validates the rule's existence first, so a rule the catalogue does not
    // publish is the modelled refusal there, worded for the rule rather than for the
    // association.
    for (rule_form, rule_id) in unpublished_rule_ids() {
        let case = format!("create against an unpublished ruleId, {rule_form}");
        let (status, body) = call_in(
            &router,
            REGION,
            "POST",
            ASSOCIATIONS_PATH,
            Some(json!({"ruleId": rule_id, "mode": MODES[0]})),
        )
        .await;
        assert_modelled_not_found(&case, status, &body, &format!("Rule not found: {rule_id}"));
    }
}
