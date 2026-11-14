// SPDX-License-Identifier: CC0-1.0

//! What a repeated `CreateInstance` `ClientToken` is judged by, end to end over the
//! `awsJson1_1` wire.
//!
//! The rule, from the `ClientToken` member's own documentation ("Passing the same value to
//! a later call to an operation requires that you also pass the same value for all other
//! parameters"): a reused `@idempotencyToken` is a REPLAY only when every other parameter
//! of the request is the one the token belongs to — `Name` as sent, and `Tags` as the
//! MULTISET of `(Key, Value)` pairs the request carried, so reordering them changes
//! nothing while any difference in the pairs themselves (one dropped, one added, a value or
//! a key changed) is a DIFFERENT request, which the reused token earns the modelled
//! `ConflictException` for. The parameters are those of the call that minted the instance,
//! not the state the instance has drifted to since, so tagging it afterwards cannot turn a
//! true retry into a conflict.
//!
//! `instance_create.rs` owns the create's own rules and the headline idempotency row;
//! this suite is the one that spans the MULTISET comparison, whose rows differ along what
//! the rule generalises over: the number of pairs (none, one, several, and the 75 the
//! model's `TagList` admits at its upper bound), their order, which pairs they are, and
//! whether `Name` and `Tags` are present at all. A list that names one KEY twice is refused
//! before the token is consulted at all, so no row carries one (`duplicate_tag_keys.rs`
//! owns that refusal). The `Tag` / `TagList` constraints the rows span are from
//! `smithy_query(query="shape",shape="Tag,TagList")`: `Key` `length 1..128`, `Value`
//! `length 0..256`, `TagList` `length 0..75`. The harness is the one
//! `ssoadmin_permission_sets.rs` owns, and every call goes through
//! `MultiServiceRouter::dispatch`.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, is_instance_arn};

/// One `CreateInstance` request of a row, minus the `ClientToken` every call of the row
/// shares: the `Name` it carries (`None` for no member at all) and the `Tags` it carries
/// (`None` for no member at all, `Some(vec![])` for the empty list the `TagList` lower
/// bound of 0 admits).
type Request = (Option<String>, Option<Vec<(String, String)>>);

/// One row: the request a token is established with, the requests that are the SAME
/// parameter set (so a replay), and the requests that are a DIFFERENT one (so a conflict).
struct Row {
    label: String,
    first: Request,
    replays: Vec<(String, Request)>,
    conflicts: Vec<(String, Request)>,
}

/// Owned `(Key, Value)` pairs from literals, as a `Tags` member.
fn tags(pairs: &[(&str, &str)]) -> Option<Vec<(String, String)>> {
    Some(
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    )
}

/// A request carrying `Name` and the given tag pairs.
fn request(name: &str, pairs: &[(&str, &str)]) -> Request {
    (Some(name.to_string()), tags(pairs))
}

/// The labelled variants of a row, built from literals.
fn variants(rows: &[(&str, Request)]) -> Vec<(String, Request)> {
    rows.iter()
        .map(|(label, body)| (label.to_string(), body.clone()))
        .collect()
}

/// The rows the rule is asserted over. Each runs against an account of its own, since an
/// account holds at most one instance.
fn rows() -> Vec<Row> {
    let name = "Idempotent";
    let mut rows = vec![
        Row {
            label: "two distinct pairs".to_string(),
            first: request(name, &[("Owner", "platform"), ("Tier", "test")]),
            replays: variants(&[
                ("the request verbatim", request(name, &[("Owner", "platform"), ("Tier", "test")])),
                ("the pairs reversed", request(name, &[("Tier", "test"), ("Owner", "platform")])),
            ]),
            conflicts: variants(&[
                ("one pair dropped", request(name, &[("Owner", "platform")])),
                (
                    "a third pair added",
                    request(
                        name,
                        &[("Owner", "platform"), ("Tier", "test"), ("Env", "prod")],
                    ),
                ),
                (
                    "one value changed",
                    request(name, &[("Owner", "finance"), ("Tier", "test")]),
                ),
                (
                    "one value differing only in case",
                    request(name, &[("Owner", "Platform"), ("Tier", "test")]),
                ),
                (
                    "one key changed",
                    request(name, &[("owner", "platform"), ("Tier", "test")]),
                ),
                (
                    "every pair dropped, as the empty list",
                    (Some(name.to_string()), Some(Vec::new())),
                ),
                (
                    "the Tags member dropped",
                    (Some(name.to_string()), None),
                ),
                (
                    "another Name",
                    request("Different", &[("Owner", "platform"), ("Tier", "test")]),
                ),
                (
                    "the Name dropped",
                    (None, tags(&[("Owner", "platform"), ("Tier", "test")])),
                ),
            ]),
        },
        Row {
            label: "three pairs, with a Key at its 128-character bound and a Value at the \
                    empty lower bound"
                .to_string(),
            first: (
                Some(name.to_string()),
                Some(vec![
                    ("a".repeat(128), String::new()),
                    ("b".to_string(), "2".to_string()),
                    ("c".to_string(), "3".to_string()),
                ]),
            ),
            replays: variants(&[
                (
                    "the request verbatim",
                    (
                        Some(name.to_string()),
                        Some(vec![
                            ("a".repeat(128), String::new()),
                            ("b".to_string(), "2".to_string()),
                            ("c".to_string(), "3".to_string()),
                        ]),
                    ),
                ),
                (
                    "the pairs rotated by one",
                    (
                        Some(name.to_string()),
                        Some(vec![
                            ("b".to_string(), "2".to_string()),
                            ("c".to_string(), "3".to_string()),
                            ("a".repeat(128), String::new()),
                        ]),
                    ),
                ),
            ]),
            conflicts: variants(&[
                (
                    "one pair replaced by a fourth distinct pair",
                    (
                        Some(name.to_string()),
                        Some(vec![
                            ("a".repeat(128), String::new()),
                            ("b".to_string(), "2".to_string()),
                            ("d".to_string(), "4".to_string()),
                        ]),
                    ),
                ),
                (
                    "the bounded key dropped",
                    (
                        Some(name.to_string()),
                        Some(vec![
                            ("b".to_string(), "2".to_string()),
                            ("c".to_string(), "3".to_string()),
                        ]),
                    ),
                ),
                (
                    "the bounded key given a value",
                    (
                        Some(name.to_string()),
                        Some(vec![
                            ("a".repeat(128), "v".to_string()),
                            ("b".to_string(), "2".to_string()),
                            ("c".to_string(), "3".to_string()),
                        ]),
                    ),
                ),
            ]),
        },
        Row {
            label: "no optional members at all".to_string(),
            first: (None, None),
            replays: variants(&[
                ("the request verbatim", (None, None)),
                // No member and the empty list are the same parameter: no tags.
                ("an empty Tags list instead of no member", (None, Some(Vec::new()))),
            ]),
            conflicts: variants(&[
                ("a Name added", (Some("Named".to_string()), None)),
                ("one tag added", (None, tags(&[("Owner", "platform")]))),
            ]),
        },
        Row {
            label: "a pair whose key and value could be re-split".to_string(),
            first: request(name, &[("a", "bc")]),
            replays: variants(&[("the request verbatim", request(name, &[("a", "bc")]))]),
            conflicts: variants(&[
                ("the same text split differently", request(name, &[("ab", "c")])),
                ("the same text as one key", request(name, &[("abc", "")])),
            ]),
        },
    ];

    // The 75 pairs `TagList` admits at its upper bound, sent in descending key order.
    let many: Vec<(String, String)> = (0..75)
        .rev()
        .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
        .collect();
    let mut ascending = many.clone();
    ascending.sort();
    // One pair carrying another's VALUE: the same count and every KEY the first call sent,
    // yet a different parameter set.
    let mut one_revalued = many.clone();
    one_revalued[40] = (many[40].0.clone(), many[7].1.clone());
    let mut one_dropped = many.clone();
    one_dropped.pop();
    rows.push(Row {
        label: "75 pairs, the most TagList admits".to_string(),
        first: (Some(name.to_string()), Some(many.clone())),
        replays: variants(&[
            ("the request verbatim", (Some(name.to_string()), Some(many.clone()))),
            ("the pairs in ascending key order", (Some(name.to_string()), Some(ascending))),
        ]),
        conflicts: variants(&[
            (
                "one of the 75 pairs carrying another's value",
                (Some(name.to_string()), Some(one_revalued)),
            ),
            ("one of the 75 pairs dropped", (Some(name.to_string()), Some(one_dropped))),
        ]),
    });
    rows
}

/// A `CreateInstance` body: `ClientToken` plus whichever optional members the request
/// carries.
fn body(client_token: &str, (name, pairs): &Request) -> Value {
    let mut body = json!({"ClientToken": client_token});
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    if let Some(pairs) = pairs {
        body["Tags"] = Value::Array(
            pairs
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    body
}

/// The `(Key, Value)` pairs `ListTagsForResource` reports for `resource_arn`.
async fn reported_tags(router: &MultiServiceRouter, resource_arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"ResourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200 for {resource_arn}, got {status} {body}"
    );
    body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A reused `ClientToken` replays only the request it belongs to: the same `Name` and the
/// same MULTISET of tag pairs, in any order.
#[tokio::test]
async fn a_reused_client_token_replays_only_the_same_name_and_tag_multiset() {
    for (index, row) in rows().into_iter().enumerate() {
        let (router, engine) = crate::instance_create::empty_account_router();
        let token = format!("c0ffee00-0000-4000-8000-00000000{index:04}");

        let (status, created) = call(&router, "CreateInstance", &body(&token, &row.first)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create must answer 200, got {status} {created}",
            row.label
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create minted no ARN: {created}", row.label))
            .to_string();
        assert!(
            is_instance_arn(&instance_arn),
            "case {:?}: expected a modelled InstanceArn, got {instance_arn:?}",
            row.label
        );
        // The create owes the instance an activation, and until it falls due the instance
        // serves only its own describe and delete, the tag report among the calls it
        // withholds (`instance_create_in_progress_reads.rs`). This suite's subject is the
        // tag MULTISET a token belongs to, so the owed instant is moved into the past —
        // the clock's business, not the wire's — before any report is asked for.
        crate::instance_create::hold_activation(
            &engine,
            std::time::SystemTime::now() - std::time::Duration::from_secs(60),
        );
        let after_create = reported_tags(&router, &instance_arn).await;

        for (why, replay) in &row.replays {
            let case = format!("{} / {why}", row.label);
            let (status, answer) = call(&router, "CreateInstance", &body(&token, replay)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a replay must answer 200, got {status} {answer}"
            );
            assert_eq!(
                answer["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {case:?}: a replay must answer the instance the token created \
                 {instance_arn:?}, got {answer}"
            );
            assert_eq!(
                reported_tags(&router, &instance_arn).await,
                after_create,
                "case {case:?}: a replay must not rewrite the instance's tags"
            );
        }

        for (why, conflict) in &row.conflicts {
            let case = format!("{} / {why}", row.label);
            let (status, refused) = call(&router, "CreateInstance", &body(&token, conflict)).await;
            assert_refused(
                &case,
                "the reused ClientToken carrying another request",
                "ConflictException",
                status,
                &refused,
            );
            assert_eq!(
                refused["Message"].as_str(),
                Some(state::CLIENT_TOKEN_MISMATCH_MESSAGE),
                "case {case:?}: the refusal must carry the parameter-mismatch sentence, \
                 got {refused}"
            );
            assert_eq!(
                reported_tags(&router, &instance_arn).await,
                after_create,
                "case {case:?}: a refused create must leave the instance's tags alone"
            );
        }

        // The conflicts minted nothing and changed nothing: the token still replays.
        let (status, still) = call(&router, "CreateInstance", &body(&token, &row.first)).await;
        assert_eq!(
            still["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {:?}: after the conflicts the token must still replay to {instance_arn:?}, \
             got {status} {still}",
            row.label
        );
    }
}

/// The comparison is against the PARAMETERS the token belongs to, not the tags the
/// instance carries now: a `TagResource` in between changes neither answer.
#[tokio::test]
async fn a_replay_is_judged_by_the_request_not_by_the_tags_the_instance_holds_now() {
    // The ENGINE behind the router too: the tagging call below is a write, and a write is
    // served only once the instance's creation has finished, so this row moves the
    // activation instant the create fixed into the past — the clock's business, not the
    // wire's, exactly as the shared `account_instance_router` does.
    let (router, engine) = crate::instance_create::empty_account_router();
    let token = "d1e2f3a4-b5c6-4071-8899-aabbccddeeff";
    let first = request("Idempotent", &[("Owner", "platform"), ("Tier", "test")]);

    let (status, created) = call(&router, "CreateInstance", &body(token, &first)).await;
    assert_eq!(
        status, 200,
        "the first create must answer 200, got {status} {created}"
    );
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create minted no ARN: {created}"))
        .to_string();
    crate::instance_create::hold_activation(
        &engine,
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );

    // Tag the instance after the fact: one key re-valued, one added. The instance now
    // carries a tag set NO `CreateInstance` request ever sent.
    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": instance_arn,
            "Tags": [{"Key": "Owner", "Value": "identity"}, {"Key": "Env", "Value": "prod"}],
        }),
    )
    .await;
    assert_eq!(status, 200, "TagResource expected 200, got {status} {tagged}");
    let now_held = reported_tags(&router, &instance_arn).await;
    assert_eq!(
        now_held,
        vec![
            ("Env".to_string(), "prod".to_string()),
            ("Owner".to_string(), "identity".to_string()),
            ("Tier".to_string(), "test".to_string()),
        ],
        "the instance must carry the merged tag set, got {now_held:?}"
    );

    // The original request is still a replay of the call the token belongs to.
    let (status, replayed) = call(&router, "CreateInstance", &body(token, &first)).await;
    assert_eq!(
        status, 200,
        "case \"the original request after a TagResource\": a replay must answer 200, got \
         {status} {replayed}"
    );
    assert_eq!(
        replayed["InstanceArn"].as_str(),
        Some(instance_arn.as_str()),
        "case \"the original request after a TagResource\": expected {instance_arn:?}, got \
         {replayed}"
    );

    // And a request carrying the instance's CURRENT tags is a different request from the
    // one the token belongs to, whatever the instance holds.
    let current: Request = (
        Some("Idempotent".to_string()),
        Some(now_held.clone()),
    );
    let (status, refused) = call(&router, "CreateInstance", &body(token, &current)).await;
    assert_refused(
        "the instance's current tags under the original token",
        "the reused ClientToken carrying another request",
        "ConflictException",
        status,
        &refused,
    );
    assert_eq!(
        reported_tags(&router, &instance_arn).await,
        now_held,
        "the refused create must leave the instance's tags alone"
    );
}
