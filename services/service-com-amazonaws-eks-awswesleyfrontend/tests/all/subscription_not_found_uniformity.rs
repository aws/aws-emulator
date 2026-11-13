// SPDX-License-Identifier: CC0-1.0

//! One rule, over the WHOLE EKS Anywhere subscription surface addressed by `id`:
//! every operation that resolves a single subscription from its `@httpLabel` `id` —
//! `DescribeEksAnywhereSubscription` (GET), `UpdateEksAnywhereSubscription` (POST) and
//! `DeleteEksAnywhereSubscription` (DELETE) on `/eks-anywhere-subscriptions/{id}` —
//! refuses an id the signing Region carries no subscription for with HTTP 404
//! `ResourceNotFoundException` in the subscription surface's own FIXED sentence: the
//! same bytes for every one of those operations and every id form, with nothing from
//! the request interpolated into it and no resource named in the payload.
//!
//! The three operations are the ones the model binds to that URI with `id` as an
//! `@httpLabel` (smithy_query(query="digest") over this crate's
//! `smithy-model/com-amazonaws-eks-awswesleyfrontend.json`); each of them models
//! `ResourceNotFoundException` at HTTP 404, and none of them models a 501.
//!
//! What the suites beside this one assert is one operation at a time
//! (`refusal_selection` spans the read, `eks_anywhere_subscription_mutations` the two
//! mutations); what is asserted HERE is that the three agree — a client that branches
//! on the refusal text cannot be given one sentence by the read and another by a
//! mutation, and no cell of the table may fall into the scaffold's 501 channel.
//!
//! Nothing here waits for a span: the positive control looks again until the
//! subscription has settled, so what is asserted is the state a call reaches and never
//! how fast the host gets there.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one in
//! `tests/all/wire.rs`.

use std::time::{Duration, Instant};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    RESOURCE_NOT_FOUND_MESSAGE, SUBSCRIPTION_NOT_FOUND_MESSAGE, SUBSCRIPTION_STATUS_ACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// Another Region of the same account: a subscription belongs to the Region it was
/// created in, so this one carries none of the subscriptions created below.
const OTHER_REGION: &str = "eu-central-1";

/// How long the positive control gives a clock-owed subscription move before it calls
/// it a failure. Generous on purpose: what is asserted is the state a call reaches,
/// never how fast the host gets there.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How often the poll loop looks again. See [`POLL_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The three operations under test, each as the `(method, body)` a client sends to
/// `/eks-anywhere-subscriptions/{id}`.
///
/// `UpdateEksAnywhereSubscription` carries the renewal flag its input models as
/// required; the read and the delete carry no payload at all.
const ADDRESSED_OPERATIONS: &[(&str, &str, &str)] = &[
    ("DescribeEksAnywhereSubscription", "GET", ""),
    (
        "UpdateEksAnywhereSubscription",
        "POST",
        r#"{"autoRenew":true}"#,
    ),
    ("DeleteEksAnywhereSubscription", "DELETE", ""),
];

/// One call to `/eks-anywhere-subscriptions/{id}`, with `id` percent-encoded the way
/// an AWS SDK encodes a non-greedy `@httpLabel`.
async fn addressed(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    id: &str,
    body: &str,
) -> Answer {
    call(
        router,
        region,
        method,
        &format!("/eks-anywhere-subscriptions/{}", encode_label(id)),
        body,
    )
    .await
}

/// A subscription request for `name`, bought for `months`.
fn subscription_request(name: &str, months: i32) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "term": { "duration": months, "unit": "MONTHS" },
        "licenseQuantity": 1,
        "autoRenew": false,
    })
}

/// A created subscription of [`REGION`], with the id the create minted.
async fn created(router: &MultiServiceRouter, case: &str, name: &str, months: i32) -> String {
    let answer = call(
        router,
        REGION,
        "POST",
        "/eks-anywhere-subscriptions",
        &subscription_request(name, months).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: setup CreateEksAnywhereSubscription expected 200, got {answer:?}"
    );
    let id = answer.body["subscription"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !id.is_empty(),
        "case {case:?}: setup must report the id the create minted, got {answer:?}"
    );
    id
}

// ---------------------------------------------------------------------------
// RULE: an id the signing Region carries no subscription for is HTTP 404
// `ResourceNotFoundException` with the subscription surface's own fixed sentence —
// identically from the read and from both mutations, whatever form the id takes.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over in BOTH directions: the operation
/// addressed (all three the model binds to the id), and the form of the id and the
/// reason it resolves to nothing — a well-formed UUID nothing was minted for, a value
/// that is no id at all, a single character, an id longer than any this service mints,
/// a value carrying characters an SDK must percent-encode, a real id with a suffix, a
/// real id upper-cased, and a real id addressed in a Region that does not hold it.
#[tokio::test]
async fn every_subscription_operation_refuses_an_unknown_id_in_one_fixed_sentence() {
    let router = emulator();
    let real_id = created(&router, "the control subscription", "uniform-refusal", 12).await;

    let long_id = "a".repeat(512);
    let suffixed = format!("{real_id}-x");
    let upper = real_id.to_uppercase();
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a well-formed UUID nothing was minted for",
            REGION,
            "11111111-2222-4333-8444-555555555555",
        ),
        ("a value that is no id at all", REGION, "not-a-subscription"),
        ("a one-character id", REGION, "x"),
        (
            "an id longer than any this service mints",
            REGION,
            long_id.as_str(),
        ),
        (
            "an id carrying characters an SDK percent-encodes",
            REGION,
            "sub/../id with spaces?&=",
        ),
        ("the real id with a suffix", REGION, suffixed.as_str()),
        ("the real id upper-cased", REGION, upper.as_str()),
        (
            "the real id, addressed in another Region",
            OTHER_REGION,
            real_id.as_str(),
        ),
    ];

    for (case, region, id) in rows {
        let mut sentences: Vec<(&str, String)> = Vec::new();
        for (operation, method, body) in ADDRESSED_OPERATIONS {
            let answer = addressed(&router, region, method, id, body).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} with {case:?}: expected HTTP 404, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {operation} with {case:?}: expected ResourceNotFoundException, got {code:?}"
            );
            let message = answer.message();
            assert_eq!(
                message, SUBSCRIPTION_NOT_FOUND_MESSAGE,
                "case {operation} with {case:?}: expected the subscription surface's own \
                 sentence, got {message:?}"
            );
            // FIXED text for the class: no part of it comes from the request, so the id
            // just sent appears nowhere in it.
            assert!(
                !message.contains(id),
                "case {operation} with {case:?}: the sentence is fixed text and must not carry \
                 the id {id:?}, got {message:?}"
            );
            assert_ne!(
                message, RESOURCE_NOT_FOUND_MESSAGE,
                "case {operation} with {case:?}: a subscription is not a resource inside a \
                 cluster, so the generic sentence is wrong, got {message:?}"
            );
            // A refusal that resolved NO subscription names none of the exception's
            // identity members.
            for member in [
                "subscriptionId",
                "clusterName",
                "nodegroupName",
                "fargateProfileName",
                "addonName",
            ] {
                assert_eq!(
                    answer.body[member],
                    serde_json::Value::Null,
                    "case {operation} with {case:?}: expected {member} unset, got {answer:?}"
                );
            }
            sentences.push((operation, message));
        }
        // The three agree: one sentence for the surface, not one per operation.
        let first = sentences[0].1.clone();
        for (operation, message) in &sentences {
            assert_eq!(
                *message, first,
                "case {operation} with {case:?}: every operation of this surface answers the \
                 same sentence, expected {first:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: the rule above is about ABSENCE. Each of the three operations serves the
// subscription that IS there — none of them is the scaffold's 501 channel, and none of
// them answers the not-found refusal for an id its Region carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_subscription_operation_serves_an_id_its_region_carries() {
    let router = emulator();
    let id = created(&router, "the served subscription", "served-subscription", 36).await;

    // The read reports the subscription the create wrote.
    let read = addressed(&router, REGION, "GET", &id, "").await;
    assert_eq!(
        read.status, 200,
        "case DescribeEksAnywhereSubscription: expected 200, got {read:?}"
    );
    assert_eq!(
        read.body["subscription"]["id"],
        serde_json::json!(id),
        "case DescribeEksAnywhereSubscription: expected subscription {id:?}, got {read:?}"
    );

    // The update writes the renewal flag, and the read after it reports what was
    // written once the clock has settled the subscription.
    let updated = addressed(&router, REGION, "POST", &id, r#"{"autoRenew":true}"#).await;
    assert_eq!(
        updated.status, 200,
        "case UpdateEksAnywhereSubscription: expected 200, got {updated:?}"
    );
    assert_eq!(
        updated.body["subscription"]["autoRenew"],
        serde_json::json!(true),
        "case UpdateEksAnywhereSubscription: expected autoRenew true, got {updated:?}"
    );
    let deadline = Instant::now() + POLL_DEADLINE;
    let settled = loop {
        let answer = addressed(&router, REGION, "GET", &id, "").await;
        if answer.status == 200
            && answer.body["subscription"]["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE)
        {
            break answer;
        }
        assert!(
            Instant::now() < deadline,
            "case UpdateEksAnywhereSubscription: the subscription never reached \
             {SUBSCRIPTION_STATUS_ACTIVE}, last read {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        settled.body["subscription"]["autoRenew"],
        serde_json::json!(true),
        "case UpdateEksAnywhereSubscription: expected the read to report autoRenew true, \
         got {settled:?}"
    );

    // The delete of a LIVE subscription is refused for the state it is in — a modeled
    // 400 that names the subscription the service resolved, which is what proves the
    // operation resolved the id rather than answering the 501 channel or the not-found
    // refusal.
    let refused = addressed(&router, REGION, "DELETE", &id, "").await;
    assert_eq!(
        refused.status, 400,
        "case DeleteEksAnywhereSubscription: expected HTTP 400, got {refused:?}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "InvalidRequestException",
        "case DeleteEksAnywhereSubscription: expected InvalidRequestException, got {code:?}"
    );
    assert_eq!(
        refused.body["subscriptionId"],
        serde_json::json!(id),
        "case DeleteEksAnywhereSubscription: expected the refusal to name subscription {id:?}, \
         got {refused:?}"
    );
    assert_ne!(
        refused.message(),
        SUBSCRIPTION_NOT_FOUND_MESSAGE,
        "case DeleteEksAnywhereSubscription: an id this Region carries is not a not-found, \
         got {refused:?}"
    );
}
