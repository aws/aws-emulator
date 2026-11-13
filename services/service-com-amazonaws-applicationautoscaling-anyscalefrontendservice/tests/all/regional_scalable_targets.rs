// SPDX-License-Identifier: CC0-1.0

//! Two rules about what a scalable target may be and where it can be seen.
//!
//! * A scalable target, and everything that hangs off it — its scaling policies, its
//!   scheduled actions and its scaling activities — is a REGIONAL resource: it lives in the
//!   Region the registering request was addressed to, the ARN this service mints for it
//!   names that Region, and the same `(ServiceNamespace, ResourceId, ScalableDimension)`
//!   triple in another Region is a different resource. So another Region's endpoint reports
//!   none of it, cannot address it, and the capacity window is enforced on the resource of
//!   the Region the target was registered in.
//! * A `ResourceId` has to name a resource the capacity its dimension scales exists ON:
//!   `lambda:function:ProvisionedConcurrency` scales provisioned concurrency, which is
//!   allocated to a published version or an alias and never to the unpublished `$LATEST`
//!   version, so a resource id naming that version — or naming no qualifier at all — names
//!   no capacity to scale and is refused with nothing registered.
//!
//! Each rule is asserted over a table: several Region pairs for the first, and the span of
//! qualifier forms for the second.

use serde_json::json;

use super::emulator::{error_code, error_message, Emulator};

/// The dimension the Region rows scale: a DynamoDB table's provisioned write capacity, so
/// what the registration enforces is readable in the table store of ONE Region.
const WRITE_CAPACITY: &str = "dynamodb:table:WriteCapacityUnits";

/// The table name both Regions of a row hold, so what tells the two targets apart is the
/// Region alone: a DynamoDB table name is unique per Region, not globally.
const TABLE: &str = "table/catalog";

/// One row of the Region table: a Region a target is registered in, and another Region whose
/// endpoint must see nothing of it.
struct RegionPair {
    case: &'static str,
    home: &'static str,
    other: &'static str,
}

/// The Region pairs.
///
/// Region names are opaque to the rule, so the rows differ in the shape of the pair rather
/// than in anything the service reads: two Regions of one geography, two on opposite sides
/// of the world, and a pair whose HOME is not the Region an unscoped request would fall back
/// to — which is what would hide a target that is stored under a default rather than under
/// the Region it was registered in.
const REGION_PAIRS: &[RegionPair] = &[
    RegionPair {
        case: "two Regions of one geography",
        home: "us-east-1",
        other: "us-west-2",
    },
    RegionPair {
        case: "two Regions on opposite sides of the world",
        home: "eu-central-1",
        other: "ap-southeast-2",
    },
    RegionPair {
        case: "a home Region that is not the fallback one",
        home: "sa-east-1",
        other: "us-east-1",
    },
];

#[tokio::test]
async fn a_scalable_target_is_only_in_the_region_it_was_registered_in() {
    for row in REGION_PAIRS {
        let case = row.case;
        let emulator = Emulator::empty();

        // The same table name in both Regions, each at one write unit.
        for region in [row.home, row.other] {
            let (status, body) = emulator
                .call_dynamodb_in(
                    region,
                    "CreateTable",
                    json!({
                        "TableName": "catalog",
                        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                        "BillingMode": "PROVISIONED",
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": 1, "WriteCapacityUnits": 1,
                        },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateTable in {region} to answer 200, got {status} \
                 {body:?}"
            );
        }

        // Registered in the home Region only, with a floor above what the table holds.
        let target = json!({
            "ServiceNamespace": "dynamodb",
            "ResourceId": TABLE,
            "ScalableDimension": WRITE_CAPACITY,
        });
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(3);
        registration["MaxCapacity"] = json!(9);
        let (status, body) = emulator
            .call_in(row.home, "RegisterScalableTarget", registration)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget in {} to answer 200, got {status} \
             {body:?}",
            row.home
        );
        let arn = body["ScalableTargetARN"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.contains(&format!(":{}:", row.home)),
            "case {case:?}: expected the minted ARN to name the Region it was registered in, \
             got {arn:?}"
        );

        // The home Region reports it, with the window it was registered with.
        let home_targets = scalable_targets(&emulator, row.home, &target, case).await;
        assert_eq!(
            home_targets.len(),
            1,
            "case {case:?}: expected {} to report the registered target, got {home_targets:?}",
            row.home
        );
        assert_eq!(
            (
                home_targets[0]["MinCapacity"].as_i64(),
                home_targets[0]["MaxCapacity"].as_i64()
            ),
            (Some(3), Some(9)),
            "case {case:?}: expected the registered window, got {:?}",
            home_targets[0]
        );

        // The other Region reports nothing for the same triple.
        let other_targets = scalable_targets(&emulator, row.other, &target, case).await;
        assert!(
            other_targets.is_empty(),
            "case {case:?}: expected {} to hold no scalable target for {TABLE}, got \
             {other_targets:?}",
            row.other
        );

        // The floor moved the table of the home Region, and only that one.
        assert_eq!(
            write_capacity(&emulator, row.home, case).await,
            3,
            "case {case:?}: expected the floor to raise the write capacity of {}'s table",
            row.home
        );
        assert_eq!(
            write_capacity(&emulator, row.other, case).await,
            1,
            "case {case:?}: expected {}'s table of the same name to be left alone",
            row.other
        );

        // A policy and a scheduled action of the home Region's target are the home Region's.
        let mut policy = target.clone();
        policy["PolicyName"] = json!("tracker");
        policy["PolicyType"] = json!("TargetTrackingScaling");
        policy["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 50.0,
            "PredefinedMetricSpecification": {
                "PredefinedMetricType": "DynamoDBWriteCapacityUtilization",
            },
        });
        let (status, body) = emulator
            .call_in(row.home, "PutScalingPolicy", policy.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy in {} to answer 200, got {status} {body:?}",
            row.home
        );
        let mut action = target.clone();
        action["ScheduledActionName"] = json!("evening");
        action["Schedule"] = json!("cron(0 18 * * ? *)");
        action["ScalableTargetAction"] = json!({"MinCapacity": 4});
        let (status, body) = emulator
            .call_in(row.home, "PutScheduledAction", action.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScheduledAction in {} to answer 200, got {status} \
             {body:?}",
            row.home
        );
        for (operation, member) in [
            ("DescribeScalingPolicies", "ScalingPolicies"),
            ("DescribeScheduledActions", "ScheduledActions"),
            ("DescribeScalingActivities", "ScalingActivities"),
        ] {
            let home = listed(&emulator, row.home, operation, member, &target).await;
            assert!(
                !home.is_empty(),
                "case {case:?}: expected {operation} in {} to report what the target carries, \
                 got {home:?}",
                row.home
            );
            let other = listed(&emulator, row.other, operation, member, &target).await;
            assert!(
                other.is_empty(),
                "case {case:?}: expected {operation} in {} to report nothing, got {other:?}",
                row.other
            );
        }

        // Every write addressed at the other Region's endpoint is about a target that is not
        // registered there, and the refusal names the triple it looked for.
        let (status, body) = emulator
            .call_in(row.other, "PutScalingPolicy", policy)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutScalingPolicy in {} to be refused, got {status} {body:?}",
            row.other
        );
        assert_eq!(
            error_code(&body),
            "ObjectNotFoundException",
            "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "No scalable target registered for service namespace: dynamodb, resource ID: \
                 {TABLE}, scalable dimension: {WRITE_CAPACITY}"
            ),
            "case {case:?}: unexpected refusal message, got {body:?}"
        );
        let (status, body) = emulator
            .call_in(row.other, "PutScheduledAction", action)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutScheduledAction in {} to be refused, got {status} \
             {body:?}",
            row.other
        );
        let (status, body) = emulator
            .call_in(row.other, "DeregisterScalableTarget", target.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DeregisterScalableTarget in {} to be refused, got {status} \
             {body:?}",
            row.other
        );
        assert_eq!(
            error_code(&body),
            "ObjectNotFoundException",
            "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
        );

        // And the target's own ARN names nothing in the other Region either.
        let (status, body) = emulator
            .call_in(
                row.other,
                "ListTagsForResource",
                json!({"ResourceARN": arn}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ListTagsForResource in {} to answer 400, got {status} \
             {body:?}",
            row.other
        );
        let (status, body) = emulator
            .call_in(
                row.other,
                "TagResource",
                json!({"ResourceARN": arn, "Tags": {"team": "search"}}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected TagResource in {} to be refused, got {status} {body:?}",
            row.other
        );

        // After all of that the home Region's target is still registered, untouched.
        let home_targets = scalable_targets(&emulator, row.home, &target, case).await;
        assert_eq!(
            home_targets.len(),
            1,
            "case {case:?}: expected {} to still hold its target, got {home_targets:?}",
            row.home
        );
        let (status, body) = emulator
            .call_in(
                row.home,
                "TagResource",
                json!({"ResourceARN": arn, "Tags": {"team": "search"}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource in {} to answer 200, got {status} {body:?}",
            row.home
        );

        // A page token is a walk through ONE Region's targets, so the other Region's
        // endpoint did not issue it: a second target makes the home Region's first page
        // carry a token, and presenting it elsewhere is refused instead of resuming inside
        // that Region's own keys.
        let mut second = target.clone();
        second["ScalableDimension"] = json!("dynamodb:table:ReadCapacityUnits");
        second["MinCapacity"] = json!(1);
        second["MaxCapacity"] = json!(9);
        let (status, body) = emulator
            .call_in(row.home, "RegisterScalableTarget", second)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second registration to answer 200, got {status} {body:?}"
        );
        let paged = json!({"ServiceNamespace": "dynamodb", "MaxResults": 1});
        let (status, body) = emulator
            .call_in(row.home, "DescribeScalableTargets", paged.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first page to answer 200, got {status} {body:?}"
        );
        let token = body["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a NextToken for a second page, got {body:?}")
            })
            .to_string();
        let mut foreign = paged;
        foreign["NextToken"] = json!(token);
        let (status, body) = emulator
            .call_in(row.other, "DescribeScalableTargets", foreign)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected another Region to refuse the token, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextTokenException",
            "case {case:?}: expected an InvalidNextTokenException, got {body:?}"
        );
    }
}

/// One row of the qualifier table: the `ResourceId` a `lambda` registration carries, and
/// whether the capacity it names exists — with, for a row it does not, the sentence the
/// registration is refused with.
struct QualifierRow {
    case: &'static str,
    resource_id: &'static str,
    accepted: bool,
    /// The refusal's message, for a row that is refused.
    refusal: &'static str,
}

/// The qualifier forms.
///
/// Source: the model's own `ResourceId` documentation for the `function` resource type —
/// "the unique identifier is the function name with a function version or alias name suffix
/// that is not `$LATEST`" — so an alias and a published version name a capacity, and the
/// unpublished version names none. The rows the issue did not cite are the published
/// version, the second function's alias and the id whose qualifier is empty.
///
/// Which refusal a row gets follows what it got wrong: an id of the documented FORM whose
/// qualifier can carry no provisioned concurrency is refused for that qualifier, and an id
/// that is not of the form at all is the triple that describes no one resource.
const QUALIFIER_ROWS: &[QualifierRow] = &[
    QualifierRow {
        case: "an alias",
        resource_id: "function:ingest:prod",
        accepted: true,
        refusal: "",
    },
    QualifierRow {
        case: "a published version",
        resource_id: "function:ingest:3",
        accepted: true,
        refusal: "",
    },
    QualifierRow {
        case: "an alias of another function",
        resource_id: "function:web-api:blue",
        accepted: true,
        refusal: "",
    },
    QualifierRow {
        case: "the unpublished version",
        resource_id: "function:ingest:$LATEST",
        accepted: false,
        refusal: UNSUPPORTED_QUALIFIER,
    },
    QualifierRow {
        case: "the unpublished version of another function",
        resource_id: "function:web-api:$LATEST",
        accepted: false,
        refusal: UNSUPPORTED_QUALIFIER,
    },
    QualifierRow {
        case: "an unqualified function",
        resource_id: "function:ingest",
        accepted: false,
        refusal: UNSUPPORTED_TRIPLE,
    },
    QualifierRow {
        case: "a qualifier that is empty",
        resource_id: "function:ingest:",
        accepted: false,
        refusal: UNSUPPORTED_TRIPLE,
    },
];

/// The sentence a (namespace, resource id, dimension) triple that does not describe one
/// resource is refused with. Entirely fixed text: what is unsupported is the combination.
const UNSUPPORTED_TRIPLE: &str =
    "Unsupported service namespace, resource type or scalable dimension";

/// The sentence a resource id whose QUALIFIER carries no provisioned concurrency is refused
/// with: the qualifier, and that it is not one Auto Scaling supports. It names neither the
/// function nor the dimension — the qualifier is the whole of what is unsupported.
const UNSUPPORTED_QUALIFIER: &str = "$LATEST is not a supported qualifier for Auto Scaling";

#[tokio::test]
async fn provisioned_concurrency_is_only_scalable_on_a_version_or_an_alias() {
    const DIMENSION: &str = "lambda:function:ProvisionedConcurrency";

    for row in QUALIFIER_ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = json!({
            "ServiceNamespace": "lambda",
            "ResourceId": row.resource_id,
            "ScalableDimension": DIMENSION,
        });
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(1);
        registration["MaxCapacity"] = json!(5);
        let (status, body) = emulator.call("RegisterScalableTarget", registration).await;

        let registered = scalable_targets(&emulator, super::emulator::REGION, &target, case).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} \
                 {body:?}"
            );
            assert_eq!(
                registered.len(),
                1,
                "case {case:?}: expected the registered target to be reported, got {registered:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            // Each refusal states the rule that was broken and nothing else: the qualifier
            // that carries no provisioned concurrency, or the triple that describes no one
            // resource.
            let message = error_message(&body);
            assert_eq!(
                message, row.refusal,
                "case {case:?}: expected the refusal {:?}, got {body:?}",
                row.refusal
            );
            assert!(
                registered.is_empty(),
                "case {case:?}: expected the refused registration to store nothing, got \
                 {registered:?}"
            );
        }
    }
}

/// The scalable targets `region`'s endpoint reports for the triple `target` names.
async fn scalable_targets(
    emulator: &Emulator,
    region: &str,
    target: &serde_json::Value,
    case: &str,
) -> Vec<serde_json::Value> {
    listed(emulator, region, "DescribeScalableTargets", "ScalableTargets", target)
        .await
        .into_iter()
        .inspect(|reported| {
            assert_eq!(
                reported["ResourceId"], target["ResourceId"],
                "case {case:?}: expected the report to be of the target asked about"
            );
        })
        .collect()
}

/// What `operation` reports in `region` for the target `target` names, under the response
/// member `member`.
async fn listed(
    emulator: &Emulator,
    region: &str,
    operation: &str,
    member: &str,
    target: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let mut request = json!({
        "ServiceNamespace": target["ServiceNamespace"],
        "ScalableDimension": target["ScalableDimension"],
    });
    // `DescribeScalableTargets` filters on a LIST of resource ids; its siblings on one.
    if operation == "DescribeScalableTargets" {
        request["ResourceIds"] = json!([target["ResourceId"]]);
    } else {
        request["ResourceId"] = target["ResourceId"].clone();
    }
    let (status, body) = emulator.call_in(region, operation, request).await;
    assert_eq!(
        status, 200,
        "expected {operation} in {region} to answer 200, got {status} {body:?}"
    );
    body[member].as_array().cloned().unwrap_or_default()
}

/// The provisioned write capacity `region`'s DynamoDB store holds for the table under test.
async fn write_capacity(emulator: &Emulator, region: &str, case: &str) -> i64 {
    let (status, body) = emulator
        .call_dynamodb_in(region, "DescribeTable", json!({"TableName": "catalog"}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeTable in {region} to answer 200, got {status} {body:?}"
    );
    body["Table"]["ProvisionedThroughput"]["WriteCapacityUnits"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a write capacity in {region}, got {body:?}")
        })
}
