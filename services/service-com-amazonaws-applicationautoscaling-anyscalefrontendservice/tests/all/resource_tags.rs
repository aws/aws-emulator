// SPDX-License-Identifier: CC0-1.0

//! The tagging surface of a scalable target — `TagResource`, `ListTagsForResource` and
//! `UntagResource` — end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` through the harness in
//! `scalable_target_lifecycle.rs`, so what is asserted is what a signing AWS client
//! would see: routing, deserialization, constraint validation, the handler and
//! serialization included.
//!
//! The rule under test is that a scalable target carries ONE tag set, addressed by the
//! ARN `RegisterScalableTarget` minted for it: tagging merges the request's keys into
//! that set (a key already present takes the new value, a key the request does not name
//! keeps the value it had), untagging removes exactly the keys named, and
//! `ListTagsForResource` reports the set without changing it. Every suite below is a
//! TABLE whose rows differ along what that rule generalises over — which keys the batch
//! names, whether they are already present, the value, the count, the target's identity
//! — and the cases the issue cited (a nonexistent resource, a batch of too many tags)
//! are rows among them.

use serde_json::{json, Map, Value};

use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::{
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX,
};

use crate::emulator::{error_code, Emulator, ACCOUNT, REGION};

/// `(ServiceNamespace, ResourceId, ScalableDimension)` triples the suites register, so
/// the rule is exercised against more than one target identity. The namespace/dimension
/// pairs come from the model's own `ServiceNamespace` and `ScalableDimension` value sets
/// (rendered into `src/ops/types/shared_types.rs` as `SERVICE_NAMESPACE_VALUES` /
/// `SCALABLE_DIMENSION_VALUES`).
const IDENTITIES: &[(&str, &str, &str)] = &[
    (
        "dynamodb",
        "table/tagged-table",
        "dynamodb:table:ReadCapacityUnits",
    ),
    ("ecs", "service/cluster/svc", "ecs:service:DesiredCount"),
    (
        "lambda",
        "function:tagged-fn:provisioned",
        "lambda:function:ProvisionedConcurrency",
    ),
    (
        "cassandra",
        "keyspace/ks/table/tbl",
        "cassandra:table:WriteCapacityUnits",
    ),
];

/// The `TagMap` a `(key, value)` table row asks for.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// `count` distinct tags, for the suites that exercise the per-resource maximum.
fn generated_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|i| (format!("key-{i:03}"), json!(format!("value-{i}"))))
            .collect::<Map<String, Value>>(),
    )
}

/// Register the `index`th identity (wrapping) with a distinct resource id, and return
/// the ARN the service minted for it.
async fn register(emulator: &Emulator, index: usize, case: &str) -> String {
    let (namespace, resource_id, dimension) = IDENTITIES[index % IDENTITIES.len()];
    let resource_id = format!("{resource_id}-{index}");
    // A scalable target resolves the resource it scales in the service that owns it, so
    // the DynamoDB and ECS rows get their resource created first.
    if let Some(table) = resource_id.strip_prefix("table/") {
        emulator.create_provisioned_table(table, 1, 1).await;
    }
    if let Some((cluster, service)) = resource_id
        .strip_prefix("service/")
        .and_then(|rest| rest.split_once('/'))
    {
        emulator.given_ecs_service(cluster, service, 1).await;
    }
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
    body["ScalableTargetARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ScalableTargetARN, got {body:?}"))
        .to_string()
}

/// The tag set `ListTagsForResource` reports for `arn`, having also checked that reading
/// it twice answers identically — a report must not change what the next reader sees.
async fn tags_of(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
    );
    let tags = body["Tags"].clone();
    assert!(
        tags.is_object(),
        "case {case:?}: expected a Tags object, got {body:?}"
    );

    let (again_status, again) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        (again_status, &again["Tags"]),
        (200, &tags),
        "case {case:?}: expected a second ListTagsForResource to answer identically, got \
         {again_status} {again:?}"
    );
    tags
}

/// Tag `arn` with `tags`, asserting the call succeeded.
async fn tag(emulator: &Emulator, arn: &str, tags: Value, case: &str) {
    let (status, body) = emulator
        .call("TagResource", json!({"ResourceARN": arn, "Tags": tags}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource to answer 200, got {status} {body:?}"
    );
}

/// A row of the merge table: what is already on the target, the batch the request
/// carries, and the tag set the merge must leave behind.
struct MergeRow {
    case: &'static str,
    stored: &'static [(&'static str, &'static str)],
    request: &'static [(&'static str, &'static str)],
    expected: &'static [(&'static str, &'static str)],
}

/// Rows differ along what the merge rule generalises over: whether the key is already
/// present, the value (including the empty value `TagValue len 0..256` allows), the
/// casing of the key ("both case-sensitive strings"), the size of the batch, and the
/// punctuation a tag key may carry.
const MERGE_TABLE: &[MergeRow] = &[
    MergeRow {
        case: "a new key is added beside the stored ones",
        stored: &[("team", "search")],
        request: &[("env", "prod")],
        expected: &[("env", "prod"), ("team", "search")],
    },
    MergeRow {
        case: "an existing key takes the request's value",
        stored: &[("env", "dev"), ("team", "search")],
        request: &[("env", "prod")],
        expected: &[("env", "prod"), ("team", "search")],
    },
    MergeRow {
        case: "the empty value is a value",
        stored: &[],
        request: &[("env", "")],
        expected: &[("env", "")],
    },
    MergeRow {
        case: "keys differing only in case are distinct tags",
        stored: &[("Env", "upper")],
        request: &[("env", "lower")],
        expected: &[("Env", "upper"), ("env", "lower")],
    },
    MergeRow {
        case: "a batch of several keys lands whole",
        stored: &[("keep", "me")],
        request: &[("a", "1"), ("b", "2"), ("c", "3")],
        expected: &[("a", "1"), ("b", "2"), ("c", "3"), ("keep", "me")],
    },
    MergeRow {
        case: "an empty batch leaves the tag set alone",
        stored: &[("team", "search")],
        request: &[],
        expected: &[("team", "search")],
    },
    MergeRow {
        case: "a key carrying the punctuation tag keys allow",
        stored: &[],
        request: &[("cost-center/team_1+a=b.c@d", "42")],
        expected: &[("cost-center/team_1+a=b.c@d", "42")],
    },
    MergeRow {
        case: "a tag whose value is not ASCII",
        stored: &[("project", "search")],
        request: &[("project", "検索-π")],
        expected: &[("project", "検索-π")],
    },
];

#[tokio::test]
async fn tagging_merges_the_batch_into_the_stored_tag_set() {
    let emulator = Emulator::empty();

    for (index, row) in MERGE_TABLE.iter().enumerate() {
        let case = row.case;
        let arn = register(&emulator, index, case).await;

        if !row.stored.is_empty() {
            tag(&emulator, &arn, tag_map(row.stored), case).await;
        }
        tag(&emulator, &arn, tag_map(row.request), case).await;

        let actual = tags_of(&emulator, &arn, case).await;
        assert_eq!(
            actual,
            tag_map(row.expected),
            "case {case:?}: expected the merged tag set {:?}, got {actual:?}",
            tag_map(row.expected)
        );
    }
}

#[tokio::test]
async fn tags_set_when_the_target_is_registered_are_reported_and_then_mergeable() {
    let emulator = Emulator::with_tables(&["created-with-tags"]).await;
    let case = "create-time tags round-trip";

    // `RegisterScalableTarget` takes create-time `Tags`; the same tag set must be what
    // the reader reports and what a later `TagResource` merges into.
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/created-with-tags",
                "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 4,
                "Tags": {"team": "search", "env": "dev"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
    let arn = body["ScalableTargetARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ScalableTargetARN, got {body:?}"))
        .to_string();

    let created = tags_of(&emulator, &arn, case).await;
    assert_eq!(
        created,
        tag_map(&[("env", "dev"), ("team", "search")]),
        "case {case:?}: expected the create-time tags, got {created:?}"
    );

    tag(&emulator, &arn, json!({"env": "prod"}), case).await;
    let merged = tags_of(&emulator, &arn, case).await;
    assert_eq!(
        merged,
        tag_map(&[("env", "prod"), ("team", "search")]),
        "case {case:?}: expected TagResource to edit the create-time tag, got {merged:?}"
    );

    // Untagging the key that was set at creation leaves the rest of the set.
    let (untag_status, untag_body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["team"]}),
        )
        .await;
    assert_eq!(
        untag_status, 200,
        "case {case:?}: expected UntagResource to answer 200, got {untag_status} {untag_body:?}"
    );
    let left = tags_of(&emulator, &arn, case).await;
    assert_eq!(
        left,
        tag_map(&[("env", "prod")]),
        "case {case:?}: expected only the untagged key to be gone, got {left:?}"
    );
}

/// A row of the untag table: what is on the target, the keys the request names, and what
/// must be left.
struct UntagRow {
    case: &'static str,
    stored: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    expected: &'static [(&'static str, &'static str)],
}

/// Rows differ along what the removal rule generalises over: how many of the stored keys
/// are named, whether a named key is present at all, repetition inside the list, the
/// empty list, and the casing of the key.
const UNTAG_TABLE: &[UntagRow] = &[
    UntagRow {
        case: "only the key named is removed",
        stored: &[("a", "1"), ("b", "2"), ("c", "3")],
        remove: &["b"],
        expected: &[("a", "1"), ("c", "3")],
    },
    UntagRow {
        case: "a key the target does not carry changes nothing",
        stored: &[("a", "1")],
        remove: &["ghost"],
        expected: &[("a", "1")],
    },
    UntagRow {
        case: "every key can be removed",
        stored: &[("a", "1"), ("b", "2")],
        remove: &["a", "b"],
        expected: &[],
    },
    UntagRow {
        case: "an empty key list leaves the tag set alone",
        stored: &[("a", "1")],
        remove: &[],
        expected: &[("a", "1")],
    },
    UntagRow {
        case: "a key named twice is removed once",
        stored: &[("a", "1"), ("b", "2")],
        remove: &["a", "a"],
        expected: &[("b", "2")],
    },
    UntagRow {
        case: "removal is case-sensitive",
        stored: &[("Env", "upper")],
        remove: &["env"],
        expected: &[("Env", "upper")],
    },
];

#[tokio::test]
async fn untagging_removes_exactly_the_keys_named() {
    let emulator = Emulator::empty();

    for (index, row) in UNTAG_TABLE.iter().enumerate() {
        let case = row.case;
        let arn = register(&emulator, index, case).await;
        if !row.stored.is_empty() {
            tag(&emulator, &arn, tag_map(row.stored), case).await;
        }

        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": row.remove}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {body:?}"
        );

        let actual = tags_of(&emulator, &arn, case).await;
        assert_eq!(
            actual,
            tag_map(row.expected),
            "case {case:?}: expected the tag set {:?} to be left, got {actual:?}",
            tag_map(row.expected)
        );
    }
}

#[tokio::test]
async fn the_per_resource_tag_maximum_is_the_bound_on_the_whole_tag_set() {
    let emulator = Emulator::empty();

    // AT the bound: a batch of exactly the maximum lands, and every key is readable.
    let case = format!("a batch of {MAX_TAGS_PER_RESOURCE} tags (the bound)");
    let arn = register(&emulator, 0, &case).await;
    tag(&emulator, &arn, generated_tags(MAX_TAGS_PER_RESOURCE), &case).await;
    let at_bound = tags_of(&emulator, &arn, &case).await;
    assert_eq!(
        at_bound,
        generated_tags(MAX_TAGS_PER_RESOURCE),
        "case {case:?}: expected all {MAX_TAGS_PER_RESOURCE} tags to be stored, got {at_bound:?}"
    );

    // At the bound, EDITING a key is still accepted: the merge leaves the count alone.
    let case = format!("editing a key while holding {MAX_TAGS_PER_RESOURCE} tags");
    tag(&emulator, &arn, json!({"key-000": "edited"}), &case).await;
    let edited = tags_of(&emulator, &arn, &case).await;
    assert_eq!(
        edited["key-000"],
        json!("edited"),
        "case {case:?}: expected the edit to land, got {edited:?}"
    );
    assert_eq!(
        edited.as_object().map(Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "case {case:?}: expected the tag count to stay at {MAX_TAGS_PER_RESOURCE}, got {edited:?}"
    );

    // ONE PAST the bound by a merge: the request fits on no resource at all only when it
    // is itself oversized, so this one is refused as the modelled `TooManyTagsException`
    // and nothing from it lands.
    let case = format!("one key past the bound of {MAX_TAGS_PER_RESOURCE}");
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": arn, "Tags": {"one-too-many": "x"}}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refusal with HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyTagsException",
        "case {case:?}: expected TooManyTagsException, got {body:?}"
    );
    let unchanged = tags_of(&emulator, &arn, &case).await;
    assert_eq!(
        unchanged, edited,
        "case {case:?}: expected the refused batch to leave the tag set alone, got {unchanged:?}"
    );

    // ONE PAST the bound in a single request: refused as input, before any resource is
    // resolved, with the 400 `ValidationException` the recorded run shows.
    let case = format!("a single batch of {} tags", MAX_TAGS_PER_RESOURCE + 1);
    let fresh = register(&emulator, 1, &case).await;
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({
                "ResourceARN": fresh,
                "Tags": generated_tags(MAX_TAGS_PER_RESOURCE + 1),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refusal with HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let empty = tags_of(&emulator, &fresh, &case).await;
    assert_eq!(
        empty,
        json!({}),
        "case {case:?}: expected the refused batch to land no tag at all, got {empty:?}"
    );
}

#[tokio::test]
async fn the_modelled_key_and_value_lengths_are_enforced_at_their_bounds() {
    let emulator = Emulator::empty();
    // The bounds are the model's own: `TagMap` keys target `TagKey` (len 1..128) and
    // values target `TagValue` (len 0..256) — `smithy_query(query="shape",
    // shape="TagMap,TagKey,TagValue")`.
    const MAX_KEY_CHARS: usize = 128;
    const MAX_VALUE_CHARS: usize = 256;

    let arn = register(&emulator, 0, "tag length bounds").await;

    for (case, key, value, accepted) in [
        (
            "a key and a value at their maxima",
            "k".repeat(MAX_KEY_CHARS),
            "v".repeat(MAX_VALUE_CHARS),
            true,
        ),
        (
            "a key at its minimum of one character",
            "k".to_string(),
            String::new(),
            true,
        ),
        (
            "a key one character past its maximum",
            "k".repeat(MAX_KEY_CHARS + 1),
            "v".to_string(),
            false,
        ),
        (
            "a value one character past its maximum",
            "edge".to_string(),
            "v".repeat(MAX_VALUE_CHARS + 1),
            false,
        ),
        (
            "a key below its minimum of one character",
            String::new(),
            "v".to_string(),
            false,
        ),
    ] {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceARN": arn, "Tags": {key.clone(): value}}),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected TagResource to answer 200, got {status} {body:?}"
            );
            let stored = tags_of(&emulator, &arn, case).await;
            assert!(
                stored.get(&key).is_some(),
                "case {case:?}: expected the key to be stored, got {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal with HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            let stored = tags_of(&emulator, &arn, case).await;
            assert!(
                stored.get(&key).is_none(),
                "case {case:?}: expected the refused key not to be stored, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_reserved_tag_key_namespace_is_refused_atomically() {
    let emulator = Emulator::empty();

    // Rows differ along how the reserved prefix appears: alone, with the rest of a real
    // AWS-generated key, in the other case, and beside keys that are perfectly legal.
    // A key that merely STARTS with the three letters is not in the namespace.
    let reserved_batches: &[(&str, Value)] = &[
        (
            "a reserved key alone",
            json!({"aws:cloudformation:stack-name": "my-stack"}),
        ),
        (
            "the reserved prefix in upper case",
            json!({"AWS:Service": "application-autoscaling"}),
        ),
        (
            "a reserved key beside legal ones",
            json!({"team": "search", "aws:created-by": "cfn", "env": "prod"}),
        ),
    ];

    for (index, (case, batch)) in reserved_batches.iter().enumerate() {
        let arn = register(&emulator, index, case).await;
        let (status, body) = emulator
            .call("TagResource", json!({"ResourceARN": arn, "Tags": batch}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal with HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        // Atomically: not one key of the batch landed, legal ones included.
        let stored = tags_of(&emulator, &arn, case).await;
        assert_eq!(
            stored,
            json!({}),
            "case {case:?}: expected the refused batch to land no tag at all, got {stored:?}"
        );
    }

    // A key that is not in the reserved namespace, though it begins with the same
    // letters, is an ordinary tag key.
    let case = "a key beginning with the prefix's letters but not the prefix";
    let arn = register(&emulator, 3, case).await;
    let lookalike = RESERVED_TAG_KEY_PREFIX.trim_end_matches(':').to_string() + "ome";
    tag(&emulator, &arn, json!({lookalike.clone(): "yes"}), case).await;
    let stored = tags_of(&emulator, &arn, case).await;
    assert_eq!(
        stored,
        json!({lookalike.clone(): "yes"}),
        "case {case:?}: expected {lookalike:?} to be stored, got {stored:?}"
    );

    // Removing a reserved key is refused the same way, and removes nothing.
    let case = "untagging a reserved key";
    tag(&emulator, &arn, json!({"team": "search"}), case).await;
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["team", "aws:created-by"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refusal with HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let stored = tags_of(&emulator, &arn, case).await;
    assert_eq!(
        stored,
        json!({lookalike: "yes", "team": "search"}),
        "case {case:?}: expected the refused removal to remove nothing, got {stored:?}"
    );
}

#[tokio::test]
async fn a_resource_arn_naming_no_registered_target_is_refused() {
    let emulator = Emulator::empty();

    // A target that was registered and then deregistered: its ARN was real and is not
    // any more, so it names no scalable target either.
    let case = "the ARN of a deregistered target";
    let withdrawn = register(&emulator, 0, case).await;
    let (namespace, resource_id, dimension) = IDENTITIES[0];
    let (status, body) = emulator
        .call(
            "DeregisterScalableTarget",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": format!("{resource_id}-0"),
                "ScalableDimension": dimension,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeregisterScalableTarget to answer 200, got {status} {body:?}"
    );

    // The ARN of the target that WAS registered now resolves to no registered scalable
    // target, so every operation of the family answers alike at the client-error status: the
    // resource the ARN names is reported absent.
    for (operation, request) in [
        (
            "TagResource",
            json!({"ResourceARN": withdrawn, "Tags": {"a": "1"}}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": withdrawn, "TagKeys": ["a"]}),
        ),
        ("ListTagsForResource", json!({"ResourceARN": withdrawn})),
    ] {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse with HTTP 400, got {status} {body:?}"
        );
        let expected = expected_code(operation);
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {operation} to answer {expected}, got {body:?}"
        );
    }

    // Every ARN below satisfies the modelled `@pattern`
    // (^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$), so
    // each one reaches the handler rather than the boundary validator — and none of them is
    // one this service minted.
    let unresolvable: &[(&str, String)] = &[
        (
            "a well-formed ARN of a target that was never registered",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an ARN whose target id is not one this service mints",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/nonexistent"
            ),
        ),
        (
            "an ARN scoped to another region",
            format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an ARN scoped to another account",
            format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
    ];

    for (case, arn) in unresolvable {
        // None of these ARNs resolves to a registered scalable target, so each operation
        // answers the same way it answers the ARN of a deregistered target: there is no
        // scalable target to report tags of, and none to write tags on.
        for (operation, request) in [
            ("TagResource", json!({"ResourceARN": arn, "Tags": {"a": "1"}})),
            (
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": ["a"]}),
            ),
            ("ListTagsForResource", json!({"ResourceARN": arn})),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse with HTTP 400, got {status} {body:?}"
            );
            let expected = expected_code(operation);
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {operation} to answer {expected}, got {body:?}"
            );
        }
    }
}

/// The error code every operation of the tagging family answers an ARN that resolves to no
/// registered scalable target with: `ResourceARN` names the resource the call addresses, so an
/// ARN that identifies no registered target is the resource reported absent.
fn expected_code(_operation: &str) -> &'static str {
    "ResourceNotFoundException"
}

#[tokio::test]
async fn a_resource_arn_outside_the_modelled_pattern_is_refused_at_the_boundary() {
    let emulator = Emulator::empty();

    // Rows that break the modelled `@pattern`/`@length` on `ResourceARN`: a different
    // resource type, a different service, and the empty string (len min 1). Each is a
    // boundary refusal — 400 `ValidationException` — on all three operations.
    for case in [
        format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalingPolicy/p"),
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/t"),
        "not-an-arn".to_string(),
        String::new(),
    ] {
        for (operation, request) in [
            (
                "TagResource",
                json!({"ResourceARN": case, "Tags": {"a": "1"}}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": case, "TagKeys": ["a"]}),
            ),
            ("ListTagsForResource", json!({"ResourceARN": case})),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse with HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected {operation} to answer ValidationException, got {body:?}"
            );
        }
    }
}
