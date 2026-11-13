// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's TAGGING operations: `TagResource`, and the
//! `ListTagsForResource` that reads its writes back.
//!
//! The rule these tables assert: `TagResource` MERGES the requested tags into the ones
//! the named resource already holds, per key — a key the resource does not hold is
//! created, a key it does hold has its value replaced, and a key the request does not
//! name is left alone — and the resource must exist (`ResourceNotFoundException`, 404,
//! for an ARN no stored resource carries) and must not end up holding more than
//! [`state::MAX_TAGS_PER_RESOURCE`] tags (`TooManyTagsException`, 400, writing nothing).
//!
//! The rows span the class the rule generalises over: how many tags a call carries (none
//! through the per-resource maximum and one past it), whether each key is new or already
//! held, the form of the key and the value (shortest, longest, empty value, non-ASCII),
//! and how many calls the tags arrive in. Where a row names a published bound it names
//! the modelled member it comes from. The merge invariant is also asserted over EVERY
//! ordered pair of a set of candidate calls, so the whole sequence class is covered
//! deterministically rather than sampled.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON
//! envelope and the HTTP status are all exercised.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{
    encode_label, error_code, expected_arn, text, Harness, REGION,
};

/// `TagKey` is `length(min: 1, max: 128)` (`smithy_query(query="shape",shape="Tags")`).
const MAX_TAG_KEY: usize = 128;

/// `TagValue` is `length(min: 0, max: 256)` (same query).
const MAX_TAG_VALUE: usize = 256;

/// Source of a distinct team name per created team, so one test's rows never collide on
/// the `ConflictException` a duplicate name earns.
static TEAM_SEQ: AtomicUsize = AtomicUsize::new(0);

/// A `(key, value)` tag pair, from `&str` literals.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `n` distinct tags, `offset` apart, so a row can ask for a tag set of any size.
fn tags(offset: usize, n: usize) -> Vec<(String, String)> {
    (offset..offset + n)
        .map(|i| (format!("key-{i:03}"), format!("value-{i:03}")))
        .collect()
}

/// The `Tags` map member of a request body.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The tags a response reports, or `None` when it carries no `Tags` member at all.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    body.get("Tags").and_then(Value::as_object).map(|map| {
        map.iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    })
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag_resource(harness: &Harness, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({ "Tags": tag_map(pairs) })),
        )
        .await
}

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`.
async fn list_tags(harness: &Harness, arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/tags/{}", encode_label(arn)), None)
        .await
}

/// The tags `ListTagsForResource` reports for `arn`, asserting the read succeeded.
async fn tags_of(harness: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(harness, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource returned {status} with {body:?}"
    );
    reported_tags(&body).unwrap_or_else(|| {
        panic!("case {case:?}: ListTagsForResource reported no Tags member: {body:?}")
    })
}

/// A create body the modelled constraints accept, optionally tagged.
fn create_body(name: &str, initial: &[(String, String)]) -> Value {
    let mut body = json!({
        "Name": name,
        "Description": "tagging",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn":
                format!(
                    "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                    engine::emulator_account_id()
                ),
        }],
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy1/$DEFAULT"
        }],
    });
    if !initial.is_empty() {
        body["Tags"] = tag_map(initial);
    }
    body
}

/// Create a team carrying `initial` tags and return its ARN.
async fn create_team(harness: &Harness, initial: &[(String, String)], case: &str) -> String {
    let name = format!("tagged-{}", TEAM_SEQ.fetch_add(1, Ordering::Relaxed));
    let (status, body) = harness.create(&create_body(&name, initial)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApprovalTeam returned {status} with {body:?}"
    );
    let arn = text(&body, "Arn");
    assert!(!arn.is_empty(), "case {case:?}: create minted no Arn: {body:?}");
    arn
}

/// A team ARN in `account` that names no stored team.
fn team_arn(account: &str, name: &str) -> String {
    format!("arn:aws:mpa:{REGION}:{account}:approval-team/{name}")
}

/// One row of the merge table.
struct MergeCase {
    case: &'static str,
    /// Tags the team is created with.
    initial: Vec<(String, String)>,
    /// Tags each successive `TagResource` call carries, in order.
    calls: Vec<Vec<(String, String)>>,
    /// What the resource must hold once every call has been made.
    expected: Vec<(String, String)>,
}

/// The merge table. Rows differ along tag count, key novelty (new key vs. a key already
/// held), key and value form, and the number of calls the tags arrive in.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            case: "one tag onto an untagged resource",
            initial: Vec::new(),
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![tag("env", "prod")],
        },
        MergeCase {
            case: "an empty Tags map changes nothing",
            initial: vec![tag("env", "prod")],
            calls: vec![Vec::new()],
            expected: vec![tag("env", "prod")],
        },
        MergeCase {
            case: "a new key leaves the keys already held alone",
            initial: vec![tag("owner", "platform")],
            calls: vec![vec![tag("env", "test")]],
            expected: vec![tag("env", "test"), tag("owner", "platform")],
        },
        MergeCase {
            case: "a key already held has its value replaced, not duplicated",
            initial: vec![tag("env", "test"), tag("owner", "platform")],
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![tag("env", "prod"), tag("owner", "platform")],
        },
        MergeCase {
            // `TagValue` is `length(min: 0, max: 256)`: the empty string is a value, so
            // setting a key to it is a tag, never a removal.
            case: "an empty value is a tag, and replaces a non-empty one",
            initial: vec![tag("env", "prod")],
            calls: vec![vec![tag("env", "")]],
            expected: vec![tag("env", "")],
        },
        MergeCase {
            // `TagKey` is `length(min: 1, max: 128)`, `TagValue` `length(max: 256)`:
            // both bounds, and the shortest key there is.
            case: "the shortest and the longest key and value the model allows",
            initial: Vec::new(),
            calls: vec![vec![
                tag("k", ""),
                ("K".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE)),
            ]],
            expected: vec![
                ("K".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE)),
                tag("k", ""),
            ],
        },
        MergeCase {
            // Neither member carries a modelled pattern, so a key and a value are any
            // string inside their length bounds.
            case: "non-ASCII key and value",
            initial: Vec::new(),
            calls: vec![vec![
                tag("プロジェクト", "本番"),
                tag("cost:center", "a/b c=d"),
            ]],
            expected: vec![tag("cost:center", "a/b c=d"), tag("プロジェクト", "本番")],
        },
        MergeCase {
            case: "three calls merge cumulatively, the last write winning per key",
            initial: vec![tag("a", "1")],
            calls: vec![
                vec![tag("b", "2")],
                vec![tag("a", "9"), tag("c", "3")],
                vec![tag("c", "4")],
            ],
            expected: vec![tag("a", "9"), tag("b", "2"), tag("c", "4")],
        },
        MergeCase {
            case: "the same call twice is the same resource (idempotent)",
            initial: Vec::new(),
            calls: vec![
                vec![tag("env", "prod"), tag("owner", "platform")],
                vec![tag("env", "prod"), tag("owner", "platform")],
            ],
            expected: vec![tag("env", "prod"), tag("owner", "platform")],
        },
        MergeCase {
            // Exactly the per-resource maximum, in one call.
            case: "a call that fills the resource to the maximum",
            initial: Vec::new(),
            calls: vec![tags(0, state::MAX_TAGS_PER_RESOURCE)],
            expected: tags(0, state::MAX_TAGS_PER_RESOURCE),
        },
        MergeCase {
            // The maximum reached across a created set and two later calls.
            case: "created tags plus two calls reaching the maximum",
            initial: tags(0, 20),
            calls: vec![tags(20, 20), tags(40, 10)],
            expected: tags(0, state::MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// The merge rule, over the table: what `ListTagsForResource` reports after a sequence of
/// `TagResource` calls is the per-key merge of those calls into the tags the resource was
/// created with.
#[tokio::test]
async fn tag_resource_merges_per_key_and_lists_back_what_it_wrote() {
    let harness = Harness::new();

    for MergeCase {
        case,
        initial,
        calls,
        expected,
    } in merge_cases()
    {
        let arn = create_team(&harness, &initial, case).await;

        // What the resource was created with is what it holds before any tagging call.
        let before = tags_of(&harness, &arn, case).await;
        let initial_map: BTreeMap<String, String> = initial.iter().cloned().collect();
        assert_eq!(
            before, initial_map,
            "case {case:?}: before tagging the resource reports {before:?}"
        );

        // The merge is tracked alongside the service, so every intermediate state is
        // asserted too, not only the last one.
        let mut model = initial_map;
        for (n, pairs) in calls.iter().enumerate() {
            let (status, body) = tag_resource(&harness, &arn, pairs).await;
            assert_eq!(
                status, 200,
                "case {case:?}: call {n} returned {status} with {body:?}"
            );
            for (key, value) in pairs {
                model.insert(key.clone(), value.clone());
            }
            let listed = tags_of(&harness, &arn, case).await;
            assert_eq!(
                listed, model,
                "case {case:?}: after call {n} the resource reports {listed:?}"
            );
        }

        let listed = tags_of(&harness, &arn, case).await;
        let expected_map: BTreeMap<String, String> = expected.iter().cloned().collect();
        assert_eq!(
            listed, expected_map,
            "case {case:?}: expected {expected_map:?}, got {listed:?}"
        );
        assert!(
            listed.len() <= state::MAX_TAGS_PER_RESOURCE,
            "case {case:?}: the resource holds {} tags, past the maximum of {}",
            listed.len(),
            state::MAX_TAGS_PER_RESOURCE
        );

        // Tagging is not a version of the team: the tags are the only thing that moved.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {case:?}: the team reads back as {team:?}");
        assert_eq!(
            text(&team, "VersionId"),
            "1",
            "case {case:?}: tagging bumped the team's VersionId: {team:?}"
        );
    }
}

/// One row of the limit table.
struct LimitCase {
    case: &'static str,
    /// Tags the team is created with.
    initial: Vec<(String, String)>,
    /// Tags the `TagResource` call carries.
    call: Vec<(String, String)>,
    /// Whether the call is accepted.
    accepted: bool,
}

/// The limit table. Rows differ along how many tags the resource already holds, how many
/// the call carries and how many of those are keys the resource already holds — the one
/// thing the count keys on, since a key already held is an update, not an addition.
fn limit_cases() -> Vec<LimitCase> {
    let max = state::MAX_TAGS_PER_RESOURCE;
    vec![
        LimitCase {
            case: "the maximum in a single call on an untagged resource",
            initial: Vec::new(),
            call: tags(0, max),
            accepted: true,
        },
        LimitCase {
            case: "one past the maximum in a single call",
            initial: Vec::new(),
            call: tags(0, max + 1),
            accepted: false,
        },
        LimitCase {
            case: "far past the maximum in a single call",
            initial: Vec::new(),
            call: tags(0, max * 2),
            accepted: false,
        },
        LimitCase {
            case: "one new key on a resource already at the maximum",
            initial: tags(0, max),
            call: vec![tag("one-more", "x")],
            accepted: false,
        },
        LimitCase {
            case: "a value update on a resource already at the maximum",
            initial: tags(0, max),
            call: vec![(format!("key-{:03}", 0), "updated".to_string())],
            accepted: true,
        },
        LimitCase {
            case: "every key re-set on a resource already at the maximum",
            initial: tags(0, max),
            call: tags(0, max),
            accepted: true,
        },
        LimitCase {
            case: "a batch that exactly fills the resource",
            initial: tags(0, max - 5),
            call: tags(max - 5, 5),
            accepted: true,
        },
        LimitCase {
            case: "a batch that would take the resource one past the maximum",
            initial: tags(0, max - 5),
            call: tags(max - 5, 6),
            accepted: false,
        },
        LimitCase {
            case: "a batch of updates and additions that fits",
            initial: tags(0, max - 5),
            call: {
                let mut pairs = tags(0, 5);
                pairs.extend(tags(max - 5, 5));
                pairs
            },
            accepted: true,
        },
    ]
}

/// The limit rule, over the table: a resource holds at most
/// [`state::MAX_TAGS_PER_RESOURCE`] tags, counting each key once, and a call that would
/// take it past that earns `TooManyTagsException` (400) and writes nothing.
#[tokio::test]
async fn a_request_past_the_per_resource_tag_maximum_is_refused_and_writes_nothing() {
    let harness = Harness::new();

    for LimitCase {
        case,
        initial,
        call,
        accepted,
    } in limit_cases()
    {
        let arn = create_team(&harness, &initial, case).await;
        let before = tags_of(&harness, &arn, case).await;

        let (status, body) = tag_resource(&harness, &arn, &call).await;
        let mut expected: BTreeMap<String, String> = before.clone();
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be accepted, got {status} with {body:?}"
            );
            for (key, value) in &call {
                expected.insert(key.clone(), value.clone());
            }
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "TooManyTagsException",
                "case {case:?}: wrong error code in {body:?}"
            );
            // `ResourceName` is built from the request: the ARN that was asked for.
            assert_eq!(
                text(&body, "ResourceName"),
                arn,
                "case {case:?}: wrong ResourceName in {body:?}"
            );
            assert!(
                !text(&body, "Message").is_empty(),
                "case {case:?}: TooManyTagsException carries no Message: {body:?}"
            );
        }

        let after = tags_of(&harness, &arn, case).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the resource to hold {expected:?}, it holds {after:?}"
        );
        assert!(
            after.len() <= state::MAX_TAGS_PER_RESOURCE,
            "case {case:?}: the resource holds {} tags, past the maximum of {}",
            after.len(),
            state::MAX_TAGS_PER_RESOURCE
        );
    }
}

/// `CreateApprovalTeam` holds the same ceiling: it is the other path that writes tags, so
/// no team can be stored already past the maximum. It does not declare
/// `TooManyTagsException`, so the refusal is the `ValidationException` (400) it does.
#[tokio::test]
async fn create_refuses_more_tags_than_a_resource_may_hold() {
    let harness = Harness::new();
    let max = state::MAX_TAGS_PER_RESOURCE;

    // (case, tag count, accepted)
    let rows: [(&str, usize, bool); 4] = [
        ("no tags", 0, true),
        ("one tag", 1, true),
        ("exactly the maximum", max, true),
        ("one past the maximum", max + 1, false),
    ];

    for (case, count, accepted) in rows {
        let name = format!("create-tagged-{}", TEAM_SEQ.fetch_add(1, Ordering::Relaxed));
        let (status, body) = harness.create(&create_body(&name, &tags(0, count))).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: create returned {status} with {body:?}"
            );
            let arn = text(&body, "Arn");
            let listed = tags_of(&harness, &arn, case).await;
            assert_eq!(
                listed.len(),
                count,
                "case {case:?}: the created team reports {listed:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
        // A refused create wrote nothing, so there is no resource to list tags for.
        let (status, listed) = list_tags(&harness, &expected_arn(&name)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create still recorded a team: {listed:?}"
        );
    }
}

/// The existence rule, over the table: a tagging call on an ARN no stored resource
/// carries is `ResourceNotFoundException` (404) — decided before the tag count is, so a
/// resource that does not exist is never reported as over its tag limit instead.
#[tokio::test]
async fn tagging_an_arn_no_resource_carries_is_not_found() {
    let harness = Harness::new();
    let own = engine::emulator_account_id();

    // A real team of the caller's, so a row can ask for its name under another resource
    // type and in another region.
    let live = create_team(&harness, &[tag("env", "prod")], "a live team").await;

    // (case, ARN)
    let rows: Vec<(&str, String)> = vec![
        ("a team name never created", team_arn(own, "never-created")),
        (
            "a team ARN whose region holds no such team",
            format!("arn:aws:mpa:eu-west-1:{own}:approval-team/{}", "tagged-0"),
        ),
        (
            "an identity-source ARN under which no identity source was created",
            format!("arn:aws:mpa:{REGION}:{own}:identity-source/ssoins-0001"),
        ),
        (
            "a session ARN, a resource type this service does not store",
            format!("arn:aws:mpa:{REGION}:{own}:session/session-0001"),
        ),
        (
            "an ARN that is the live team's with one character changed",
            format!("{live}x"),
        ),
    ];

    for (case, arn) in rows {
        // The request's own size decides nothing: one tag and one past the per-resource
        // maximum answer the same way, because existence is decided first.
        let requests: [(&str, Vec<(String, String)>); 3] = [
            ("one tag", vec![tag("env", "prod")]),
            ("no tags", Vec::new()),
            (
                "one past the maximum",
                tags(0, state::MAX_TAGS_PER_RESOURCE + 1),
            ),
        ];
        for (shape, pairs) in requests {
            let (status, body) = tag_resource(&harness, &arn, &pairs).await;
            assert_eq!(
                status, 404,
                "case {case:?} / {shape}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} / {shape}: wrong error code in {body:?}"
            );
        }

        let (status, body) = list_tags(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource returned {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // None of those refusals touched the resource that does exist.
    let listed = tags_of(&harness, &live, "a live team").await;
    assert_eq!(
        listed,
        BTreeMap::from([tag("env", "prod")]),
        "a refused call changed a live resource's tags: {listed:?}"
    );
}

/// A resource that exists and carries no tags is a 200 with an EMPTY `Tags` map, which is
/// a different answer from the 404 an absent resource earns.
#[tokio::test]
async fn an_untagged_resource_lists_an_empty_tag_map() {
    let harness = Harness::new();
    let arn = create_team(&harness, &[], "untagged").await;

    let (status, body) = list_tags(&harness, &arn).await;
    assert_eq!(status, 200, "ListTagsForResource returned {status} with {body:?}");
    let listed = reported_tags(&body)
        .unwrap_or_else(|| panic!("an existing resource reported no Tags member: {body:?}"));
    assert!(
        listed.is_empty(),
        "an untagged resource reported {listed:?}"
    );
}

/// The account boundary, over the tagging operations: an ARN outside the caller's own
/// account is `AccessDeniedException` (403), decided before the resource is resolved — so
/// the answer does not depend on whether anything is stored under it. The denial's wording
/// is the shared rule tested in `authorization_gate.rs`; here the rows are the accounts.
#[tokio::test]
async fn a_tagging_call_on_another_accounts_arn_is_denied() {
    let harness = Harness::new();
    let live = create_team(&harness, &[tag("env", "prod")], "cross-account").await;
    let live_name = live.rsplit('/').next().expect("the team name").to_string();

    // (case, ARN)
    let rows: Vec<(&str, String)> = vec![
        ("another account", team_arn("999999999999", "probe")),
        (
            "another account, one digit from the caller's",
            team_arn("000000000001", "probe"),
        ),
        (
            "another account, a team name the caller does own",
            team_arn("999999999999", &live_name),
        ),
        (
            "an account-less ARN",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P1/$DEFAULT".to_string(),
        ),
    ];

    for (case, arn) in rows {
        for (action, (status, body)) in [
            (
                "TagResource",
                tag_resource(&harness, &arn, &[tag("env", "prod")]).await,
            ),
            ("ListTagsForResource", list_tags(&harness, &arn).await),
        ] {
            assert_eq!(
                status, 403,
                "case {case:?} / {action}: expected HTTP 403, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?} / {action}: wrong error code in {body:?}"
            );
            assert!(
                text(&body, "Message").contains(&format!("mpa:{action}")),
                "case {case:?} / {action}: the denial does not name the action: {body:?}"
            );
        }
    }

    // A denied call is a denial: the caller's own resource is untouched.
    let listed = tags_of(&harness, &live, "cross-account").await;
    assert_eq!(
        listed,
        BTreeMap::from([tag("env", "prod")]),
        "a denied call changed the caller's own tags: {listed:?}"
    );
}

/// The candidate `TagResource` calls the sequence enumeration below draws from.
///
/// They share a small key pool on purpose, so a sequence really does re-set keys an
/// earlier call already wrote — the upsert half of the invariant — and they span the
/// forms the rule generalises over: no tags at all, one tag, several, the shortest and
/// the longest key and value the model allows (`TagKey` `length(1..128)`, `TagValue`
/// `length(0..256)`), an empty value and non-ASCII text.
fn candidate_calls() -> Vec<Vec<(String, String)>> {
    vec![
        Vec::new(),
        vec![tag("key-0", "")],
        vec![tag("key-0", "a"), tag("key-1", "value with spaces")],
        vec![
            tag("key-1", "本番"),
            ("key-2".to_string(), "v".repeat(MAX_TAG_VALUE)),
        ],
        vec![
            tag("k", "a"),
            ("K".repeat(MAX_TAG_KEY), "本番".to_string()),
            tag("プロジェクト", ""),
        ],
        vec![
            tag("key-0", "a"),
            tag("key-1", "a"),
            tag("key-2", "a"),
            tag("key-3", "a"),
        ],
    ]
}

/// The merge invariant over EVERY ordered pair of candidate calls, on a resource created
/// with a third candidate set: whatever the sequence, what `ListTagsForResource` reports
/// is the per-key merge of every call into the tags the resource was created with — the
/// last write to a key wins, a key no call named keeps its value, the stored count is the
/// number of DISTINCT keys named, and re-sending a call changes nothing.
///
/// The sequences are enumerated rather than sampled, so the whole class is covered and
/// every run exercises exactly the same cases; nothing is written to disk.
#[tokio::test]
async fn tagging_is_a_per_key_upsert_whatever_the_call_sequence() {
    let harness = Harness::new();
    let candidates = candidate_calls();

    for (i, first) in candidates.iter().enumerate() {
        for (j, second) in candidates.iter().enumerate() {
            // The created tag set rotates through the same candidates, so the sequences
            // start from every one of them too.
            let initial = &candidates[(i + j) % candidates.len()];
            let case = format!("created {initial:?}, then {first:?}, then {second:?}");
            let arn = create_team(&harness, initial, &case).await;

            let mut model: BTreeMap<String, String> = initial.iter().cloned().collect();
            for (n, pairs) in [first, second].into_iter().enumerate() {
                let (status, body) = tag_resource(&harness, &arn, pairs).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: call {n} returned {status} with {body:?}"
                );
                for (key, value) in pairs {
                    model.insert(key.clone(), value.clone());
                }
                let listed = tags_of(&harness, &arn, &case).await;
                assert_eq!(
                    listed, model,
                    "case {case:?}: after call {n} the resource reports {listed:?}"
                );
            }

            // Re-sending the last call changes nothing: the merge is idempotent.
            let (status, body) = tag_resource(&harness, &arn, second).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the replayed call returned {status} with {body:?}"
            );
            let listed = tags_of(&harness, &arn, &case).await;
            assert_eq!(
                listed, model,
                "case {case:?}: the replayed call changed the resource to {listed:?}"
            );
            assert_eq!(
                listed.len(),
                model.len(),
                "case {case:?}: the resource holds {} tags for {} distinct keys",
                listed.len(),
                model.len()
            );
        }
    }
}

/// [`state::merged_tag_count`] counts each key once, over a table of existing/incoming
/// key sets: it is the size of the UNION of the two key sets, which is what makes an
/// update to a key already held free against the per-resource maximum.
#[test]
fn the_merged_tag_count_is_the_size_of_the_key_union() {
    // (case, existing keys, incoming keys, expected count)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>, usize)> = vec![
        ("nothing held, nothing incoming", vec![], vec![], 0),
        ("nothing held, two incoming", vec![], vec!["a", "b"], 2),
        ("two held, nothing incoming", vec!["a", "b"], vec![], 2),
        ("two held, both re-set", vec!["a", "b"], vec!["a", "b"], 2),
        ("two held, one re-set and one new", vec!["a", "b"], vec!["b", "c"], 3),
        ("incoming repeats a key", vec!["a"], vec!["b", "b"], 2),
        (
            "at the maximum, every key re-set",
            (0..state::MAX_TAGS_PER_RESOURCE).map(|_| "x").collect(),
            vec!["x"],
            1,
        ),
    ];

    for (case, existing, incoming, expected) in rows {
        let held: BTreeMap<String, String> = existing
            .iter()
            .map(|key| ((*key).to_string(), "v".to_string()))
            .collect();
        let count = state::merged_tag_count(&held, incoming.iter().copied());
        assert_eq!(
            count, expected,
            "case {case:?}: expected {expected}, got {count}"
        );
    }
}
