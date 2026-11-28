// SPDX-License-Identifier: CC0-1.0

//! The batch run-state read, end to end over HTTP.
//!
//! The rule under test: a batch state read answers every requested campaign id ON ITS
//! OWN — an id the store holds goes to `successfulRequests` carrying the run state the
//! last start/pause/stop left it in, an id it does not hold goes to `failedRequests`
//! carrying the `ResourceNotFound` failure code, and the whole call is refused (HTTP 400
//! `ValidationException`) only when the request breaks `campaignIds`' own modelled bounds —
//! its `@length` minimum of one entry and its maximum of 500. Reporting state changes
//! nothing, so the same batch asked twice, and the single-id read beside it, answer alike.
//!
//! Each rule is asserted over a table whose rows differ along what it generalises over —
//! how many ids are asked for, how many of them resolve, which run state each resolved
//! campaign is in, and what shape an unresolved id takes — rather than along the one
//! request that exposed the defect.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ops::types::{
    CAMPAIGN_STATE_VALUES, GET_CAMPAIGN_STATE_BATCH_FAILURE_CODE_VALUES,
};

use super::common::{call, router, Harness};

/// The region every case here signs for.
const REGION: &str = "us-west-2";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "batch-instance";

/// The failure code a requested id that resolves to no campaign is reported with. It is
/// one of the two values `GetCampaignStateBatchFailureCode` carries
/// (`smithy_query(query="shape", shape="GetCampaignStateBatchFailureCode")`:
/// `ResourceNotFound | UnknownError`), written out here so this suite pins the wire value
/// rather than reading it back out of the implementation.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// The largest batch `campaignIds` admits: the `@length` maximum the model this
/// operation's input is declared from carries on that member, whose minimum is one entry
/// (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`). It is the
/// maximum the SERVICE enforces rather than the one the member's documentation prints — the
/// recorded corpus shows a batch of five hundred ids answered. Written out here rather than
/// read back out of the implementation, so this suite pins it; the sizes around it are
/// `input_constraint_refusals.rs`'s own table.
const MAX_BATCH: usize = 500;

/// The sentence a request refused for ONE broken input constraint is answered with: the
/// envelope around a clause naming the value half the class states, the member it sits
/// at, and the requirement it broke. Written out here rather than read back out of the
/// implementation, so this suite pins the wording.
fn envelope(value_at_member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: {value_at_member} failed to satisfy constraint: \
         {requirement}"
    )
}

/// A minimal `CreateCampaign` body: every member the model marks required, and nothing
/// else.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// Create one campaign and hand back the id the service minted for it.
async fn a_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"create campaign {name}\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create campaign {name}\": no id in {}", created.body))
        .to_string()
}

/// Drive the run-state transitions `path_suffixes` names on `id`, so the campaign ends in
/// the state those transitions leave it in.
async fn drive(router: &Harness, id: &str, path_suffixes: &[&str]) {
    for suffix in path_suffixes {
        let moved = call(
            router,
            REGION,
            "POST",
            &format!("/campaigns/{id}/{suffix}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            moved.status, 200,
            "case \"move {id} with {suffix}\": expected HTTP 200, got {} {}",
            moved.status, moved.body
        );
    }
}

/// One batch read.
async fn batch(router: &Harness, ids: &[String]) -> super::common::Answer {
    call(
        router,
        REGION,
        "POST",
        "/campaigns-state",
        &json!({"campaignIds": ids}),
    )
    .await
}

/// One id past [`MAX_BATCH`], as the refusal row below sends them: well-formed under
/// `CampaignId` and naming no campaign.
fn oversize_ids() -> Vec<String> {
    (0..=MAX_BATCH).map(|index| format!("id-{index}")).collect()
}

/// The single-id run-state read, for the same campaign.
async fn single(router: &Harness, id: &str) -> super::common::Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/campaigns/{id}/state"),
        &Value::Null,
    )
    .await
}

/// `(campaignId, state)` of every entry in `successfulRequests`, in the order served.
fn successful(answer: &super::common::Answer) -> Vec<(String, String)> {
    answer.body["successfulRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["campaignId"].as_str().unwrap_or_default().to_string(),
                entry["state"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `(campaignId, failureCode)` of every entry in `failedRequests`, in the order served.
fn failed(answer: &super::common::Answer) -> Vec<(String, String)> {
    answer.body["failedRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["campaignId"].as_str().unwrap_or_default().to_string(),
                entry["failureCode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Campaign ids no campaign carries, spanning `CampaignId` (len 0..256, pattern `^[\S]*$`
/// from `smithy_query(query="shape", shape="CampaignId")`): the uuid form, one character,
/// punctuation the pattern admits, the longest id the bound allows, and the 32-hex form
/// this emulator mints (with digits that name no campaign).
fn unknown_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "the uuid form",
            "9f1c7b22-10ae-4c53-bb07-6e4f0c9d8a11".to_string(),
        ),
        ("one character", "q".to_string()),
        ("punctuation the pattern admits", "a:b-D9.e_f".to_string()),
        ("256 characters (the length maximum)", "u".repeat(256)),
        (
            "the minted id form",
            "fedcba9876543210fedcba9876543210".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_batch_read_reports_the_state_each_resolved_id_is_in() {
    let router = router();

    // One campaign per run state an operation of this service can leave a campaign in,
    // as `(the transitions that get there, the state they leave it in)`. `Failed` is in
    // `CampaignState` but no operation of this slice moves a campaign into it, so no row
    // claims it.
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("a freshly created campaign", vec![], "Initialized"),
        ("a started campaign", vec!["start"], "Running"),
        ("a paused campaign", vec!["start", "pause"], "Paused"),
        ("a stopped campaign", vec!["start", "stop"], "Stopped"),
        (
            "a campaign stopped from paused",
            vec!["start", "pause", "stop"],
            "Stopped",
        ),
    ];

    let mut expected: Vec<(String, String)> = Vec::new();
    for (case, transitions, state) in &rows {
        let id = a_campaign(&router, case).await;
        drive(&router, &id, transitions).await;
        assert!(
            CAMPAIGN_STATE_VALUES.contains(state),
            "case {case:?}: {state:?} must be a value the model's CampaignState carries"
        );
        expected.push((id, (*state).to_string()));
    }

    // Asked one id at a time, and then all of them at once: the batch answers exactly
    // what the single-id read answers for each, in the order the ids were asked for.
    for (id, state) in &expected {
        let one = batch(&router, std::slice::from_ref(id)).await;
        assert_eq!(
            (one.status, successful(&one)),
            (200, vec![(id.clone(), state.clone())]),
            "case \"one id at a time: {state}\": expected the stored state, got {} {}",
            one.status,
            one.body
        );
        assert!(
            failed(&one).is_empty(),
            "case \"one id at a time: {state}\": a resolved id must not be reported as \
             failed, got {}",
            one.body
        );
        let direct = single(&router, id).await;
        assert_eq!(
            (direct.status, direct.body["state"].as_str()),
            (200, Some(state.as_str())),
            "case \"the single-id read beside the batch: {state}\": expected the same \
             state, got {} {}",
            direct.status,
            direct.body
        );
    }

    let ids: Vec<String> = expected.iter().map(|(id, _)| id.clone()).collect();
    let all = batch(&router, &ids).await;
    assert_eq!(
        (all.status, successful(&all)),
        (200, expected.clone()),
        "case \"every id in one batch\": expected each id with its own state, in request \
         order, got {} {}",
        all.status,
        all.body
    );
    assert_eq!(
        failed(&all),
        Vec::<(String, String)>::new(),
        "case \"every id in one batch\": nothing failed, so failedRequests is empty, got {}",
        all.body
    );

    // Reporting state changes nothing: the same batch asked again answers the same.
    let again = batch(&router, &ids).await;
    assert_eq!(
        successful(&again),
        expected,
        "case \"the same batch asked twice\": a read must not change what a later caller \
         sees, got {}",
        again.body
    );
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_fails_on_its_own_inside_the_batch() {
    let router = router();
    let known = a_campaign(&router, "known").await;

    // Every row is one unresolved id shape asked beside the one stored campaign: the
    // known id succeeds and the unknown one fails, in the same answer.
    for (case, unknown) in unknown_ids() {
        let answer = batch(&router, &[known.clone(), unknown.clone()]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an unresolved id is a per-id failure, not a refusal of the \
             batch, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            successful(&answer),
            vec![(known.clone(), "Initialized".to_string())],
            "case {case:?}: expected the stored campaign to be served beside the failure, \
             got {}",
            answer.body
        );
        assert_eq!(
            failed(&answer),
            vec![(unknown.clone(), RESOURCE_NOT_FOUND.to_string())],
            "case {case:?}: expected the unresolved id reported with the not-found failure \
             code, got {}",
            answer.body
        );
    }

    // The failure code this service answers with is one the model's enum carries.
    assert!(
        GET_CAMPAIGN_STATE_BATCH_FAILURE_CODE_VALUES.contains(&RESOURCE_NOT_FOUND),
        "case \"the failure code\": {RESOURCE_NOT_FOUND:?} must be a value the model's \
         GetCampaignStateBatchFailureCode carries, which holds \
         {GET_CAMPAIGN_STATE_BATCH_FAILURE_CODE_VALUES:?}"
    );

    // A campaign that has been deleted is as unresolved as an id that never existed.
    let deleted = a_campaign(&router, "to-be-deleted").await;
    let dropped = call(
        &router,
        REGION,
        "DELETE",
        &format!("/campaigns/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "case \"delete a campaign\": expected HTTP 200, got {} {}",
        dropped.status, dropped.body
    );
    let after = batch(&router, std::slice::from_ref(&deleted)).await;
    assert_eq!(
        (after.status, failed(&after)),
        (200, vec![(deleted.clone(), RESOURCE_NOT_FOUND.to_string())]),
        "case \"a deleted campaign\": expected the not-found failure code, got {} {}",
        after.status,
        after.body
    );
}

#[tokio::test]
async fn every_requested_id_is_answered_once_in_the_order_it_was_asked_for() {
    let router = router();
    let first = a_campaign(&router, "first").await;
    let second = a_campaign(&router, "second").await;
    drive(&router, &second, &["start"]).await;
    let absent = "no-such-campaign".to_string();

    // Rows differ along the COUNT and the ORDER of the ids asked for, including the same
    // id twice (each occurrence is its own requested entry) and the largest batch the
    // modelled maximum admits.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one resolved id", vec![first.clone()]),
        ("one unresolved id", vec![absent.clone()]),
        (
            "unresolved first, resolved second",
            vec![absent.clone(), first.clone()],
        ),
        (
            "resolved, unresolved, resolved",
            vec![first.clone(), absent.clone(), second.clone()],
        ),
        (
            "the same resolved id twice",
            vec![first.clone(), first.clone()],
        ),
        (
            "the same unresolved id twice",
            vec![absent.clone(), absent.clone()],
        ),
        (
            "the largest batch the modelled maximum admits",
            (0..MAX_BATCH)
                .map(|index| match index % 3 {
                    0 => first.clone(),
                    1 => second.clone(),
                    _ => format!("absent-{index}"),
                })
                .collect(),
        ),
    ];

    for (case, ids) in rows {
        let answer = batch(&router, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let served = successful(&answer);
        let refused = failed(&answer);
        assert_eq!(
            served.len() + refused.len(),
            ids.len(),
            "case {case:?}: expected one answer per requested id, got {} successful and {} \
             failed for {} ids: {}",
            served.len(),
            refused.len(),
            ids.len(),
            answer.body
        );
        // Each list keeps the request's own order, restricted to the ids that landed in
        // it.
        let expected_served: Vec<String> = ids
            .iter()
            .filter(|id| **id == first || **id == second)
            .cloned()
            .collect();
        let expected_refused: Vec<String> = ids
            .iter()
            .filter(|id| **id != first && **id != second)
            .cloned()
            .collect();
        assert_eq!(
            served.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            expected_served,
            "case {case:?}: expected the resolved ids in request order, got {}",
            answer.body
        );
        assert_eq!(
            refused.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            expected_refused,
            "case {case:?}: expected the unresolved ids in request order, got {}",
            answer.body
        );
        for (id, state) in &served {
            let expected_state = if *id == second { "Running" } else { "Initialized" };
            assert_eq!(
                state, expected_state,
                "case {case:?}: expected {id} to be reported {expected_state}, got {state}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_outside_the_bounds_of_campaign_ids_is_refused_as_a_bad_request() {
    let router = router();
    let known = a_campaign(&router, "known").await;

    // `campaignIds` is required and carries a `@length` of one to 500 entries, so the three
    // ways a batch can break the contract are refused as bad requests rather than answered
    // with an empty pair of lists: no list at all, an empty list, and one id past the
    // modelled maximum. Each row carries the sentence that class is refused with — the one
    // envelope, with the value half each class states.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "no campaignIds member at all",
            json!({}),
            envelope("Value null at 'campaignIds'", "Member must not be null"),
        ),
        (
            "the empty list",
            json!({"campaignIds": []}),
            envelope(
                "Value '[]' at 'campaignIds'",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            // The oversize clause states the list the caller submitted as the value of the
            // member that broke the maximum: a list has a rendering of its own, which is
            // the half a scalar member's clause drops.
            "one id past the modelled maximum",
            json!({"campaignIds": oversize_ids()}),
            envelope(
                &format!("Value '[{}]' at 'campaignIds'", oversize_ids().join(", ")),
                &format!("Member must have length less than or equal to {MAX_BATCH}"),
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let refused = call(&router, REGION, "POST", "/campaigns-state", &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a request outside campaignIds' modelled \
             bounds, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} on HTTP {}",
            refused.error_code,
            refused.status
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {case:?}: expected this service's constraint sentence, got {}",
            refused.body
        );
        assert!(
            refused.body.get("successfulRequests").is_none()
                && refused.body.get("failedRequests").is_none(),
            "case {case:?}: a refused batch answers no state at all, got {}",
            refused.body
        );
    }

    // And the refusals changed nothing: the stored campaign still reads back.
    let answer = batch(&router, std::slice::from_ref(&known)).await;
    assert_eq!(
        (answer.status, successful(&answer)),
        (200, vec![(known.clone(), "Initialized".to_string())]),
        "case \"the stored campaign after every refusal\": expected it to read back, got \
         {} {}",
        answer.status,
        answer.body
    );
}

#[tokio::test]
async fn an_empty_list_is_refused_as_the_empty_list_it_is_on_every_list_member() {
    let router = router();
    let known = a_campaign(&router, "known").await;

    // Every member of this service modelled as a list with a length MINIMUM, so an empty
    // list breaks it: `campaignIds` here (`@length(min 1, max 500)`, from the overlay model
    // that operation's input is declared from) and `dialRequests` on PutDialRequestBatch
    // (`@length(min 1, max 25)`, from
    // `smithy_query(query="operation", operation="PutDialRequestBatch")`).
    // The rule is the service's, not this operation's: whichever list member a request
    // empties, the clause states that value as the empty list it is.
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "GetCampaignStateBatch campaignIds",
            "POST",
            "/campaigns-state".to_string(),
            "campaignIds",
        ),
        (
            "PutDialRequestBatch dialRequests",
            "PUT",
            format!("/campaigns/{known}/dial-requests"),
            "dialRequests",
        ),
    ];

    for (case, method, path, member) in rows {
        let refused = call(&router, REGION, method, &path, &json!({member: []})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for an empty list, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            envelope(
                &format!("Value '[]' at '{member}'"),
                "Member must have length greater than or equal to 1",
            ),
            "case {case:?}: expected the empty list to be stated as the list it is, got {}",
            refused.body
        );
    }
}
