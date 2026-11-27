// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` over the wire: the tags a scalable target carries, read back out
//! of what `RegisterScalableTarget` persisted.
//!
//! The rule under test: for the ARN of a registered scalable target the operation answers
//! exactly the tag set persisted on that target — no more, no fewer, and unchanged by the
//! read — and for an ARN no registered target carries it answers by the ENDPOINT that ARN
//! names: `ResourceNotFoundException` where the ARN names this account and Region,
//! and `ValidationException` where it names another.
//!
//! The suite is a table, not a single call: its rows differ along what the rule
//! generalises over — how many tags the target carries (zero, one, several) and the form
//! and size of the keys and values, including both published bounds of the model's own
//! `TagKey` (length 1..128) and `TagValue` (length 0..256). Those bounds come from this
//! service's model
//! (`smithy_query(query="shape", shape="TagMap,TagKey,TagValue", model_path=".../smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json")`).
//! No row goes PAST those bounds: a tag key or value outside them is a bad REQUEST, which
//! the framework's boundary validator (and, for the rules AWS enforces beyond the model —
//! an `aws:`-prefixed key, say — `RegisterScalableTarget` and `TagResource`) answers, and
//! not something this read decides. This read reports what is persisted.
//!
//! The harness is the one `scalable_target_lifecycle` drives, so these cases take exactly
//! the same path a signing awsJson1_1 client takes.

use serde_json::{json, Value};

use crate::emulator::{error_code, Emulator, ACCOUNT, REGION};

/// The namespace/dimension every row registers under: the tag rule is independent of the
/// scaled resource, so one legal pair is enough and each row varies only its tags.
const NAMESPACE: &str = "dynamodb";
/// See [`NAMESPACE`].
const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";

/// The DynamoDB tables this suite's rows register a scalable target on: a target resolves
/// the resource it scales in the service that owns it, so each table exists first.
const TABLES: &[&str] = &[
    "untagged",
    "empty-tags",
    "one-tag",
    "many-tags",
    "shortest-key",
    "longest-key",
    "empty-value",
    "longest-value",
    "unicode",
    "resident",
    "deregistered",
    "upserted",
];

/// One row of the tag catalog: the tags a target is registered with (`None` omits the
/// member entirely), and therefore the tag set `ListTagsForResource` must answer with.
struct TagRow {
    case: &'static str,
    resource_id: &'static str,
    /// A function rather than a literal so a row can build a value at one of the
    /// model's published length bounds.
    tags: fn() -> Option<Value>,
}

/// A one-entry tag map, built from owned strings so a row can sit exactly on one of the
/// model's published length bounds.
fn one_tag(key: String, value: String) -> Value {
    Value::Object(
        std::iter::once((key, Value::String(value)))
            .collect::<serde_json::Map<String, Value>>(),
    )
}

const TAG_CATALOG: &[TagRow] = &[
    TagRow {
        case: "no Tags member at all",
        resource_id: "table/untagged",
        tags: || None,
    },
    TagRow {
        case: "an explicitly empty Tags map",
        resource_id: "table/empty-tags",
        tags: || Some(json!({})),
    },
    TagRow {
        // The case the recorded unit cited: a single create-time tag.
        case: "one tag",
        resource_id: "table/one-tag",
        tags: || Some(json!({"team": "search"})),
    },
    TagRow {
        case: "several tags",
        resource_id: "table/many-tags",
        tags: || {
            Some(json!({
                "zone": "c",
                "team": "search",
                "env": "staging",
                "cost-center": "4711",
                "owner": "platform",
            }))
        },
    },
    TagRow {
        case: "a tag key at the 1-character minimum",
        resource_id: "table/shortest-key",
        tags: || Some(json!({"k": "v"})),
    },
    TagRow {
        case: "a tag key at the 128-character maximum",
        resource_id: "table/longest-key",
        tags: || Some(one_tag("k".repeat(128), "at the key bound".to_string())),
    },
    TagRow {
        case: "a tag value at the 0-character minimum",
        resource_id: "table/empty-value",
        tags: || Some(json!({"empty-value": ""})),
    },
    TagRow {
        case: "a tag value at the 256-character maximum",
        resource_id: "table/longest-value",
        tags: || Some(one_tag("long-value".to_string(), "v".repeat(256))),
    },
    TagRow {
        case: "non-ASCII keys and values",
        resource_id: "table/unicode",
        tags: || Some(json!({"équipe-🔎": "recherche-日本", "版": "一"})),
    },
];

/// Register the row's target and answer the ARN `RegisterScalableTarget` minted for it.
async fn register(emulator: &Emulator, row: &TagRow) -> String {
    let mut request = json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": row.resource_id,
        "ScalableDimension": DIMENSION,
        "MinCapacity": 1,
        "MaxCapacity": 10,
    });
    if let Some(tags) = (row.tags)() {
        request["Tags"] = tags;
    }

    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}",
        row.case
    );
    body["ScalableTargetARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a ScalableTargetARN, got {body:?}",
                row.case
            )
        })
        .to_string()
}

#[tokio::test]
async fn list_tags_answers_exactly_the_tags_the_target_was_registered_with() {
    let emulator = Emulator::with_tables(TABLES).await;

    for row in TAG_CATALOG {
        let case = row.case;
        let arn = register(&emulator, row).await;
        // An omitted or empty `Tags` member leaves the target with zero tags, which the
        // operation reports as an empty map.
        let expected = (row.tags)().unwrap_or_else(|| json!({}));

        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: expected the persisted tags {expected:?}, got {body:?}"
        );

        // A read must not change what the next reader sees.
        let (status_again, body_again) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status_again, 200,
            "case {case:?}: expected a second ListTagsForResource to answer 200, \
             got {status_again} {body_again:?}"
        );
        assert_eq!(
            body_again, body,
            "case {case:?}: expected a second read to answer identically, got {body_again:?}"
        );

        // And the target itself is untouched by the tag read.
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [row.resource_id]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            described["ScalableTargets"][0]["ScalableTargetARN"],
            json!(arn),
            "case {case:?}: expected the target to still carry the ARN it was minted with, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn list_tags_refuses_an_arn_no_registered_target_carries() {
    let emulator = Emulator::with_tables(TABLES).await;

    // One registered target, so the store is not empty and the refusal means "not this ARN"
    // rather than "nothing at all".
    let resident = register(
        &emulator,
        &TagRow {
            case: "the resident target",
            resource_id: "table/resident",
            tags: || Some(json!({"keep": "me"})),
        },
    )
    .await;

    // A target that is registered and then deregistered: its ARN stops resolving.
    let removed = register(
        &emulator,
        &TagRow {
            case: "the target to deregister",
            resource_id: "table/deregistered",
            tags: || Some(json!({"gone": "soon"})),
        },
    )
    .await;
    let (status, body) = emulator
        .call(
            "DeregisterScalableTarget",
            json!({
                "ServiceNamespace": NAMESPACE,
                "ResourceId": "table/deregistered",
                "ScalableDimension": DIMENSION,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected DeregisterScalableTarget to answer 200, got {status} {body:?}"
    );

    // Every ARN below satisfies the modelled `ResourceARN` pattern, so each reaches the
    // handler; none of them names a registered target. The flag is whether the ARN names a
    // scalable-target IDENTITY of THIS endpoint — its account and Region, and an identifier of a
    // scalable target — which is what decides which refusal it earns.
    let unknown: Vec<(&str, String, bool)> = vec![
        (
            "an identifier this emulator never minted",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            true,
        ),
        (
            "an id in no form an identifier takes",
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/0123456789abcdef"),
            false,
        ),
        ("a deregistered target's ARN", removed, true),
        (
            "a resident target's ARN in another region",
            resident.replace(REGION, "eu-west-1"),
            false,
        ),
        (
            "a resident target's ARN under another account",
            resident.replace(ACCOUNT, "123456789012"),
            false,
        ),
    ];

    for (case, arn, served_here) in &unknown {
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        if *served_here {
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                body["ResourceName"],
                json!(arn),
                "case {case:?}: expected the refusal to name the ARN asked about, got {body:?}"
            );
        } else {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert!(
                body.get("ResourceName").is_none(),
                "case {case:?}: expected the refusal to name no resource, got {body:?}"
            );
        }
    }

    // The refusals changed nothing: the resident target still answers with its tags.
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident}))
        .await;
    assert_eq!(
        status, 200,
        "case \"resident after refusals\": expected HTTP 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({"keep": "me"}),
        "case \"resident after refusals\": expected the resident target's tags, got {body:?}"
    );

    // An ARN that is not a scalable-target ARN at all fails the modelled pattern, so it
    // is refused as a bad request rather than reported as a missing resource.
    let malformed: &[(&str, &str)] = &[
        (
            "a scaling-policy ARN",
            "arn:aws:autoscaling:us-east-1:000000000000:scalingPolicy:1234:resource/dynamodb/table/t:policyName/p",
        ),
        ("not an ARN at all", "table/resident"),
    ];
    for (case, arn) in malformed {
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// An upsert patch, and the capacity window it must leave behind.
struct UpsertRow {
    case: &'static str,
    patch: fn() -> Value,
    min: i64,
    max: i64,
}

/// Each row names only part of the registration, so the upsert path is exercised with a
/// different subset of members every time — the tag set must be the same afterwards for
/// all of them.
const UPSERT_CATALOG: &[UpsertRow] = &[
    UpsertRow {
        case: "a new maximum only",
        patch: || json!({"MaxCapacity": 42}),
        min: 1,
        max: 42,
    },
    UpsertRow {
        case: "a new minimum only",
        patch: || json!({"MinCapacity": 3}),
        min: 3,
        max: 42,
    },
    UpsertRow {
        case: "both bounds at once",
        patch: || json!({"MinCapacity": 5, "MaxCapacity": 50}),
        min: 5,
        max: 50,
    },
    UpsertRow {
        case: "a suspended state, no bounds",
        patch: || json!({"SuspendedState": {"DynamicScalingInSuspended": true}}),
        min: 5,
        max: 50,
    },
    UpsertRow {
        case: "a role, no bounds",
        patch: || json!({"RoleARN": "arn:aws:iam::000000000000:role/aas-upsert"}),
        min: 5,
        max: 50,
    },
];

#[tokio::test]
async fn the_tags_a_target_carries_outlive_every_upsert_of_it() {
    let emulator = Emulator::with_tables(TABLES).await;

    let created = TagRow {
        case: "the upserted target",
        resource_id: "table/upserted",
        tags: || Some(json!({"team": "search", "env": "prod"})),
    };
    let arn = register(&emulator, &created).await;
    let expected = (created.tags)().expect("the row carries tags");

    for row in UPSERT_CATALOG {
        let case = row.case;
        let mut request = json!({
            "ServiceNamespace": NAMESPACE,
            "ResourceId": created.resource_id,
            "ScalableDimension": DIMENSION,
        });
        for (member, value) in (row.patch)()
            .as_object()
            .expect("the patch is a JSON object")
        {
            request[member] = value.clone();
        }

        let (status, body) = emulator.call("RegisterScalableTarget", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["ScalableTargetARN"],
            json!(arn),
            "case {case:?}: expected the upsert to keep the ARN of the registered target, \
             got {body:?}"
        );

        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: expected the create-time tags {expected:?} to survive the upsert, \
             got {body:?}"
        );

        // The upsert the tags survived is the one the row asked for: the bounds it names
        // are what the target now carries.
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [created.resource_id]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {described:?}"
        );
        let target = &described["ScalableTargets"][0];
        assert_eq!(
            target["MinCapacity"],
            json!(row.min),
            "case {case:?}: expected MinCapacity {}, got {target:?}",
            row.min
        );
        assert_eq!(
            target["MaxCapacity"],
            json!(row.max),
            "case {case:?}: expected MaxCapacity {}, got {target:?}",
            row.max
        );
    }
}
