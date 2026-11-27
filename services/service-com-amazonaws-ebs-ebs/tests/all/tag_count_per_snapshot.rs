// SPDX-License-Identifier: CC0-1.0

//! How many `Tags` one `StartSnapshot` request may carry.
//!
//! RULE: a bound this service's own documentation states for a request member but the
//! model carries no constraint for is enforced by the handler, with the modelled
//! `ValidationException` and the `ValidationExceptionReason` that names the member's
//! class. `Tags` is the member here: it documents "You can specify up to 50 tags per
//! snapshot"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshotResponse",member="Tags")`)
//! while `com.amazonaws.ebs#Tags` is a bare `list` of `Tag` with no `@length`
//! (`smithy_query(query="shape",shape="com.amazonaws.ebs#Tags")`), so nothing at the
//! request boundary bounds the list and the ceiling is the handler's to apply — the same
//! shape as `VolumeSize`, whose documented maximum the model also omits. The reason is the
//! one the model declares for a tag the service will not take, `INVALID_TAG`
//! (`smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationExceptionReason")`).
//!
//! The rows differ along what the rule generalises over — how many tags the request
//! carries (none at all, an empty list, one, two, one short of the ceiling, exactly it, one
//! past it, and far past it) and which other members travel beside them — and every
//! accepted row is checked twice: in the body the call answers and in the record the store
//! kept, so a list the service reports is a list it really persisted.
//!
//! Both halves are asserted, because a bound only means something with its controls: a
//! request at the ceiling is accepted with every tag it sent, and a request past it is
//! refused without minting anything.

use serde_json::json;
use service_com_amazonaws_ebs_ebs::state::{service_state, MAX_TAGS_PER_SNAPSHOT};

use crate::declared_block_inputs::assert_validation;
use crate::snapshot_block_loop::{member, router_with_engine, start_snapshot};

/// A parent id of one of this service's two widths (`^snap-[0-9a-f]+$`, 17 hex digits)
/// that no call minted, for the row showing the tag count is judged before the parent is
/// resolved.
const UNHELD_PARENT_ID: &str = "snap-0123456789abcdef0";

/// A KMS key ARN of the modelled form (`KmsKeyArn`'s pattern
/// `^arn:aws[a-z\-]*:kms:.*:[0-9]{12}:key/`), for the row that carries a key reference
/// beside an over-long tag list.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// The sentence this service words for a request carrying more tags than one snapshot may
/// hold: fixed text stating the ceiling, echoing nothing of the request. Restated here
/// independently of the service so the assertion is against the wording and not against
/// whatever the handler happens to produce.
fn too_many_tags_refusal() -> String {
    format!("You can specify up to {MAX_TAGS_PER_SNAPSHOT} tags per snapshot")
}

/// `count` distinct tags, each key inside the modelled `TagKey` `@length` 0..127 and its
/// pattern `^[\S\s]+$`, each value inside `TagValue`'s 0..255 and `^[\S\s]*$`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#Tag")`), so every row is refused
/// or accepted for its COUNT and never for a tag the boundary would reject.
fn tags(count: usize) -> Vec<serde_json::Value> {
    (0..count)
        .map(|n| json!({ "Key": format!("key-{n:05}"), "Value": format!("value-{n:05}") }))
        .collect()
}

/// How many snapshots and how many `ClientToken` records this service's store holds.
fn stored_counts(engine: &engine::InMemoryEngine, case: &str) -> (usize, usize) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    (store.snapshots.len(), store.client_tokens.len())
}

/// The tags the store kept for `snapshot_id`, as the `(Key, Value)` pairs they arrived as.
fn stored_tags(
    engine: &engine::InMemoryEngine,
    case: &str,
    snapshot_id: &str,
) -> Vec<serde_json::Value> {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    let snapshot = store
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("case {case:?}: {snapshot_id} must be stored"));
    snapshot
        .tags
        .iter()
        .map(|tag| json!({ "Key": tag.key, "Value": tag.value }))
        .collect()
}

/// RULE, the accepting half: a request carrying at most the documented ceiling of tags is
/// accepted, reports exactly the tags it sent, in the order it sent them, and the store
/// keeps that same list — while a request carrying no tags at all, or an empty list,
/// reports no `Tags` member, since the member is optional with no `@default`.
#[tokio::test]
async fn a_tag_list_up_to_the_documented_ceiling_is_accepted_and_persisted() {
    // Rows differ along the number of tags — none, an empty list, one, two, one short of
    // the ceiling, and exactly the ceiling — and along which other members travel with
    // them.
    let cases: Vec<(String, Option<usize>, serde_json::Value)> = vec![
        ("no Tags member".to_string(), None, json!({ "VolumeSize": 1 })),
        (
            "an empty Tags list".to_string(),
            Some(0),
            json!({ "VolumeSize": 1 }),
        ),
        ("one tag".to_string(), Some(1), json!({ "VolumeSize": 1 })),
        (
            "two tags beside a description".to_string(),
            Some(2),
            json!({ "VolumeSize": 2, "Description": "two tags" }),
        ),
        (
            format!("{} tags, one short of the ceiling", MAX_TAGS_PER_SNAPSHOT - 1),
            Some(MAX_TAGS_PER_SNAPSHOT - 1),
            json!({ "VolumeSize": 1 }),
        ),
        (
            format!("{MAX_TAGS_PER_SNAPSHOT} tags, exactly the ceiling"),
            Some(MAX_TAGS_PER_SNAPSHOT),
            json!({ "VolumeSize": 1 }),
        ),
        (
            format!("{MAX_TAGS_PER_SNAPSHOT} tags beside a key and a timeout"),
            Some(MAX_TAGS_PER_SNAPSHOT),
            json!({ "VolumeSize": 4, "KmsKeyArn": KMS_KEY_ARN, "Timeout": 30 }),
        ),
    ];

    for (case, count, request) in &cases {
        let (router, engine) = router_with_engine();
        let mut body = request.clone();
        let sent = count.map(tags);
        if let Some(sent) = sent.clone() {
            body["Tags"] = json!(sent);
        }

        let reply = start_snapshot(&router, &body).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: a list inside the ceiling must be accepted 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let snapshot_id = member(&reply, "SnapshotId");
        assert!(
            snapshot_id.starts_with("snap-"),
            "case {case:?}: expected a minted snapshot id, got {snapshot_id:?}"
        );

        // A list the request carried is reported whole and in order; no list, or an empty
        // one, is reported as no member at all.
        let reported = reply.json().get("Tags").cloned();
        let expected = sent
            .clone()
            .filter(|sent| !sent.is_empty())
            .map(serde_json::Value::from);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected Tags {expected:?}, got {reported:?}"
        );

        // And the record behind that answer carries the same list, so the response is the
        // store's and not the request's.
        let kept = stored_tags(&engine, case, &snapshot_id);
        let expected_kept = sent.unwrap_or_default();
        assert_eq!(
            kept, expected_kept,
            "case {case:?}: expected the store to keep {expected_kept:?}, got {kept:?}"
        );
    }
}

/// RULE, the refusing half: a request carrying more tags than the documented ceiling is
/// refused with the modelled `ValidationException`, the `INVALID_TAG` reason and this
/// service's fixed sentence for the ceiling — whatever else the request carried — and the
/// refused call mints no snapshot and records no `ClientToken`.
#[tokio::test]
async fn a_tag_list_past_the_documented_ceiling_is_refused_and_persists_nothing() {
    // Rows differ along how far past the ceiling the list reaches — one entry, two, twice
    // the ceiling, and far past it — and along which other members travel with it,
    // including ones that would be judged later: a key reference another service resolves,
    // and a parent this service does not hold.
    let cases: Vec<(String, usize, serde_json::Value)> = vec![
        (
            format!("{} tags, one past the ceiling", MAX_TAGS_PER_SNAPSHOT + 1),
            MAX_TAGS_PER_SNAPSHOT + 1,
            json!({ "VolumeSize": 1 }),
        ),
        (
            format!("{} tags, two past the ceiling", MAX_TAGS_PER_SNAPSHOT + 2),
            MAX_TAGS_PER_SNAPSHOT + 2,
            json!({ "VolumeSize": 1, "Description": "two past the ceiling" }),
        ),
        (
            format!("{} tags, twice the ceiling", MAX_TAGS_PER_SNAPSHOT * 2),
            MAX_TAGS_PER_SNAPSHOT * 2,
            json!({ "VolumeSize": 2, "Timeout": 30 }),
        ),
        (
            "5000 tags".to_string(),
            5_000,
            json!({ "VolumeSize": 64, "Encrypted": true }),
        ),
        (
            format!(
                "{} tags beside a key reference",
                MAX_TAGS_PER_SNAPSHOT + 1
            ),
            MAX_TAGS_PER_SNAPSHOT + 1,
            json!({ "VolumeSize": 1, "KmsKeyArn": KMS_KEY_ARN }),
        ),
        (
            format!(
                "{} tags beside a parent the service does not hold",
                MAX_TAGS_PER_SNAPSHOT + 1
            ),
            MAX_TAGS_PER_SNAPSHOT + 1,
            json!({ "VolumeSize": 1, "ParentSnapshotId": UNHELD_PARENT_ID }),
        ),
        (
            format!("{} tags beside a client token", MAX_TAGS_PER_SNAPSHOT + 1),
            MAX_TAGS_PER_SNAPSHOT + 1,
            json!({ "VolumeSize": 1, "ClientToken": "token-past-the-tag-ceiling" }),
        ),
    ];

    for (case, count, request) in &cases {
        let (router, engine) = router_with_engine();
        let mut body = request.clone();
        body["Tags"] = json!(tags(*count));

        let reply = start_snapshot(&router, &body).await;
        assert_validation(case, &reply, "INVALID_TAG", &too_many_tags_refusal());

        let (snapshots, tokens) = stored_counts(&engine, case);
        assert_eq!(
            (snapshots, tokens),
            (0, 0),
            "case {case:?}: a refused request must mint no snapshot and record no client \
             token, got {snapshots} snapshots and {tokens} tokens"
        );
    }
}

/// The controls the refusal above needs: each member a refused row carried beside its
/// over-long list is perfectly acceptable on its own, so the refusals are about the tag
/// COUNT and not about anything else those requests said.
#[tokio::test]
async fn the_same_requests_inside_the_ceiling_are_answered_by_the_rule_that_owns_them() {
    // The same rows as the refusing table, with a list the ceiling admits: each is then
    // answered by the gate that comes after the tag count — accepted, or refused for the
    // parent it names.
    let accepted: Vec<(String, serde_json::Value)> = vec![
        (
            "a key reference with a list inside the ceiling".to_string(),
            json!({ "VolumeSize": 1, "KmsKeyArn": KMS_KEY_ARN }),
        ),
        (
            "an encrypted snapshot with a list inside the ceiling".to_string(),
            json!({ "VolumeSize": 64, "Encrypted": true }),
        ),
        (
            "a client token with a list inside the ceiling".to_string(),
            json!({ "VolumeSize": 1, "ClientToken": "token-inside-the-tag-ceiling" }),
        ),
    ];

    for (case, request) in &accepted {
        let (router, engine) = router_with_engine();
        let mut body = request.clone();
        body["Tags"] = json!(tags(MAX_TAGS_PER_SNAPSHOT));
        let reply = start_snapshot(&router, &body).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: the same request inside the ceiling must be accepted 201, got {} \
             with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let (snapshots, _) = stored_counts(&engine, case);
        assert_eq!(
            snapshots, 1,
            "case {case:?}: the accepted request must mint exactly one snapshot, got \
             {snapshots}"
        );
    }

    // And the parent row: with a list the ceiling admits, the very same request is refused
    // for the snapshot it names instead, so the tag count really was judged first.
    let (router, engine) = router_with_engine();
    let reply = start_snapshot(
        &router,
        &json!({
            "VolumeSize": 1,
            "ParentSnapshotId": UNHELD_PARENT_ID,
            "Tags": tags(MAX_TAGS_PER_SNAPSHOT),
        }),
    )
    .await;
    let case = "a parent the service does not hold, with a list inside the ceiling";
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a parent nothing holds must be refused 404, got {} with body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let (snapshots, _) = stored_counts(&engine, case);
    assert_eq!(
        snapshots, 0,
        "case {case:?}: the refused request must mint no snapshot, got {snapshots}"
    );
}
