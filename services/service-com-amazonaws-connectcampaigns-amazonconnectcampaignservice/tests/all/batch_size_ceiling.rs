// SPDX-License-Identifier: CC0-1.0

//! HOW MANY ids one batch run-state read may name, end to end over HTTP.
//!
//! RULE under test: **the ceiling on a batch of this service is the one the SERVICE
//! enforces, which is the `@length` maximum the model its operation's input is declared
//! from carries for that member — every batch up to it is ANSWERED, one entry per id asked
//! for, and only a batch past it is refused HTTP 400 `ValidationException` in the one
//! envelope this service states a constraint in, whose clause names the submitted list, the
//! member it sits at and the maximum it broke. The ceiling is per MEMBER: a member whose
//! recorded ceiling is wider than the one its own documentation prints does not widen any
//! other member's.**
//!
//! Which ceiling that is, for `GetCampaignStateBatch$campaignIds`, is recorded behaviour:
//! a batch of five hundred ids is answered — every id the store holds no campaign under
//! coming back in `failedRequests` — and only a larger batch refused, naming five hundred
//! as the maximum. The member's own documentation prints twenty-five, so the published
//! snapshot is not the bound this service applies and the operation's input is declared
//! from the overlay model beside it (`smithy-model/LOCAL-ADDITIONS.md`); the suites that
//! assert the bound read it from there rather than restating a number.
//!
//! The rows span what the rule generalises over: the SIZE of the batch (one id, the size
//! the member's documentation prints, one past that, a hundred, the ceiling itself, one
//! past the ceiling and far past it), how many of its ids RESOLVE (none, one, every
//! other one), and WHICH batch member is asked (the state read's `campaignIds` and the
//! dial batch's `dialRequests`, which keeps its own, narrower ceiling).

use serde_json::{json, Value};

use super::common::{call, router, Answer, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "ceiling-instance";

/// The ids one batch state read may name, as the overlay model that operation's input is
/// declared from carries that member's `@length` maximum. Written out here rather than read
/// back out of the implementation, so this suite pins it.
const CAMPAIGN_IDS_CEILING: usize = 500;

/// The ids one dial batch may name, as the published snapshot carries THAT member's
/// `@length` maximum — the narrower ceiling the batch state read's does not widen.
const DIAL_REQUESTS_CEILING: usize = 25;

/// The size the batch state read's own documentation prints for its member, which is not
/// the ceiling this service applies: a batch of this many ids, and of one more, are both
/// answered.
const DOCUMENTED_SIZE: usize = 25;

/// A minimal `CreateCampaign` body: every member the model marks required, and nothing
/// else.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"agentlessDialerConfig": {}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// One campaign of [`INSTANCE`], handed back as its id.
async fn a_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "setup \"create campaign {name}\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup \"create campaign {name}\": no id in {}", created.body))
        .to_string()
}

/// Ask for the run state of `ids` in one call.
async fn batch(router: &Harness, ids: &[String]) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/campaigns-state",
        &json!({"campaignIds": ids}),
    )
    .await
}

/// `count` ids no campaign is held under, each well formed under `CampaignId` (len 0..256,
/// pattern `^[\S]*$`).
fn absent_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("absent-{index}")).collect()
}

/// The `campaignId` of every entry of one of the answer's two lists, in the order served.
fn answered(answer: &Answer, member: &str) -> Vec<String> {
    answer.body[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["campaignId"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a campaignId, got {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_batch_up_to_the_ceiling_is_answered_one_entry_per_id() {
    let router = router();
    let held = a_campaign(&router, "ceiling-known").await;

    // Rows: the sizes a caller can send inside the ceiling — one id, the size the member's
    // documentation prints, one past that, a round hundred, one short of the ceiling and
    // the ceiling itself.
    for size in [
        1,
        DOCUMENTED_SIZE,
        DOCUMENTED_SIZE + 1,
        100,
        CAMPAIGN_IDS_CEILING - 1,
        CAMPAIGN_IDS_CEILING,
    ] {
        // Every other id of the batch is the stored campaign, so each row asserts the
        // split between the two lists rather than one list being the whole answer.
        let ids: Vec<String> = absent_ids(size)
            .into_iter()
            .enumerate()
            .map(|(index, absent)| {
                if index % 2 == 0 {
                    held.clone()
                } else {
                    absent
                }
            })
            .collect();
        let case = format!("a batch of {size} ids");
        let answer = batch(&router, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch up to the ceiling is answered, got {} {}",
            answer.status, answer.message()
        );
        let served = answered(&answer, "successfulRequests");
        let failed = answered(&answer, "failedRequests");
        assert_eq!(
            served.len() + failed.len(),
            ids.len(),
            "case {case:?}: expected one entry per requested id, got {} served and {} failed",
            served.len(),
            failed.len()
        );
        let expected_served: Vec<String> =
            ids.iter().filter(|id| **id == held).cloned().collect();
        let expected_failed: Vec<String> =
            ids.iter().filter(|id| **id != held).cloned().collect();
        assert_eq!(
            served, expected_served,
            "case {case:?}: expected the resolved ids in request order"
        );
        assert_eq!(
            failed, expected_failed,
            "case {case:?}: expected the unresolved ids in request order"
        );
    }
}

#[tokio::test]
async fn a_batch_past_the_ceiling_is_refused_naming_the_submitted_list_and_the_maximum() {
    let router = router();
    let held = a_campaign(&router, "ceiling-refused").await;

    // Rows: one id past the ceiling — the smallest batch that breaks it — and far past it,
    // which is the same refusal. A batch too long for one clause of the envelope to quote
    // states the member and the requirement alone, which the second row is: the shared
    // echo bound (`service_common::validation`) is what one clause may carry, and these
    // rows sit either side of it.
    let rows: Vec<(String, Vec<String>, bool)> = vec![
        (
            format!("one id past the ceiling ({})", CAMPAIGN_IDS_CEILING + 1),
            absent_ids(CAMPAIGN_IDS_CEILING + 1),
            true,
        ),
        (
            format!("ten times the ceiling ({})", 10 * CAMPAIGN_IDS_CEILING),
            absent_ids(10 * CAMPAIGN_IDS_CEILING),
            false,
        ),
    ];

    for (case, ids, quoted) in rows {
        let refused = batch(&router, &ids).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 past the ceiling, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        let subject = if quoted {
            format!("Value '[{}]' at 'campaignIds'", ids.join(", "))
        } else {
            "Value at 'campaignIds'".to_string()
        };
        assert_eq!(
            refused.message(),
            format!(
                "1 validation error detected: {subject} failed to satisfy constraint: Member \
                 must have length less than or equal to {CAMPAIGN_IDS_CEILING}"
            ),
            "case {case:?}: expected this service's constraint sentence naming the ceiling, \
             got a message of {} characters",
            refused.message().chars().count()
        );
        assert!(
            refused.body.get("successfulRequests").is_none()
                && refused.body.get("failedRequests").is_none(),
            "case {case:?}: a refused batch answers no state at all, got {}",
            refused.body
        );
    }

    // And nothing was spent on the refusals: the stored campaign still reads back.
    let after = batch(&router, std::slice::from_ref(&held)).await;
    assert_eq!(
        answered(&after, "successfulRequests"),
        vec![held],
        "case \"the stored campaign after every refusal\": expected it to read back, got {}",
        after.body
    );
}

#[tokio::test]
async fn the_dial_batch_keeps_its_own_narrower_ceiling() {
    let router = router();
    let held = a_campaign(&router, "ceiling-dial").await;
    let started = call(
        &router,
        REGION,
        "POST",
        &format!("/campaigns/{held}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "setup \"start the campaign\": expected HTTP 200, got {} {}",
        started.status, started.body
    );

    let dial_requests = |count: usize| -> Value {
        let requests: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "clientToken": format!("tok-{index}"),
                    "phoneNumber": "+15555550100",
                    "expirationTime": "2099-01-01T00:00:00Z",
                    "attributes": {"campaign": "nightly"},
                })
            })
            .collect();
        json!({"dialRequests": requests})
    };

    // Rows: the dial batch's own ceiling, which is answered, and one past it, which is
    // refused naming THAT member's maximum — the batch state read's wider ceiling is the
    // state read's alone.
    for (case, count, expected_status) in [
        ("the dial batch at its own ceiling", DIAL_REQUESTS_CEILING, 200),
        (
            "the dial batch one past its own ceiling",
            DIAL_REQUESTS_CEILING + 1,
            400,
        ),
    ] {
        let answer = call(
            &router,
            REGION,
            "PUT",
            &format!("/campaigns/{held}/dial-requests"),
            &dial_requests(count),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {count} dial requests, got {} {}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert!(
                answer.message().ends_with(&format!(
                    "Member must have length less than or equal to {DIAL_REQUESTS_CEILING}"
                )),
                "case {case:?}: expected this member's own maximum in the clause, got {}",
                answer.message()
            );
        }
    }
}
