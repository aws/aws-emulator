// SPDX-License-Identifier: CC0-1.0

//! WHAT A REFUSAL SAYS, over the wire — the rules the `refusal_messages` suite beside this
//! one does not cover.
//!
//! The rule both suites assert: where this service enforces a rule of its OWN — the
//! capacity floor a dimension can be provisioned at, the schedule's time zone and window,
//! which metrics a scalable dimension is scaled on, the configuration a policy type owes,
//! the filter combination a read accepts, a pagination token, a target that is not
//! registered, the tag limits and the reserved tag namespace — the refusal carries the
//! SERVICE's own sentence: the fixed text for that rule, with a value from the request
//! interpolated where the text names one. It is never a Smithy constraint clause
//! (`Value '<v>' at '<member>' failed to satisfy constraint: Member must …`), which is what
//! the framework writes for a constraint the MODEL declares — and for one of those, the
//! clause echoes the member's value AS SUBMITTED, a list included.
//!
//! Every suite is a TABLE whose rows differ along what its rule generalises over (which
//! bound, which dimension, which operation, which policy type, how many entries), with the
//! accepted rows beside the refused ones so each is a rule rather than a blanket refusal.

use serde_json::{json, Value};

use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::MAX_TAGS_PER_RESOURCE;

use crate::emulator::{error_code, error_message, Emulator, ACCOUNT, REGION};

// The sentences this suite pins, spelled out as the recorded refusals state them rather
// than imported from the implementation: the TEXT is what each rule owes a caller, so an
// expectation that read the service's own constant could not catch a text regression.

/// A missing `Schedule` on the create path.
const SCHEDULE_MUST_BE_SET: &str = "Schedule must be set";
/// A scheduling window whose boundaries are in the wrong order.
const START_TIME_BEFORE_END_TIME: &str = "StartTime must be before EndTime";
/// A dimension filter with no resource to narrow.
const DIMENSION_WITHOUT_RESOURCE_ID: &str =
    "Scalable dimension cannot be provided without a resource ID.";
/// A `NextToken` this service did not issue.
const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";
/// A tag operation addressed to an ARN that resolves to no registered scalable target.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";
/// A write to the reserved tag namespace.
const SYSTEM_TAG_KEY_REFUSED: &str = "Caller is an end user and not allowed to mutate system tags";
/// A tag set over the per-resource maximum.
const TAG_SET_TOO_LARGE: &str = "The resultant tag set must not have more than 50 user tags.";

/// Assert that `operation` on `request` is refused with HTTP `status`, `code` and exactly
/// `message`, and that the message is the service's own sentence rather than a constraint
/// clause.
async fn refused_with_status(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: Value,
    status: u16,
    code: &str,
    message: &str,
) {
    let (got, body) = emulator.call(operation, request).await;
    assert_eq!(
        got, status,
        "case {case:?}: expected {operation} to refuse with HTTP {status}, got {got} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code}, got {body:?}"
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
    assert!(
        !actual.contains("failed to satisfy constraint"),
        "case {case:?}: expected the service's own sentence, not a constraint clause, got \
         {actual:?}"
    );
}

/// Assert that `operation` on `request` is refused 400 with `code` and exactly `message`,
/// and that the message is the service's own sentence rather than a constraint clause.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: Value,
    code: &str,
    message: &str,
) {
    refused_with_status(emulator, case, operation, request, 400, code, message).await;
}

/// Assert that `operation` on `request` is accepted.
async fn accepted(emulator: &Emulator, case: &str, operation: &str, request: Value) -> Value {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
    );
    body
}

/// A registration of `target` with the bounds given.
fn registration(target: &Value, min: i64, max: i64) -> Value {
    let mut request = target.clone();
    request["MinCapacity"] = json!(min);
    request["MaxCapacity"] = json!(max);
    request
}

// ---------------------------------------------------------------------------
// The floor a dimension can be provisioned at
// ---------------------------------------------------------------------------

/// One capacity window, and what the service answers it with.
struct FloorRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    min: i64,
    max: i64,
    /// The refusal, or `None` when the row is accepted.
    expected: Option<&'static str>,
}

/// RULE: a capacity bound under the floor its dimension can be provisioned at is refused
/// with the end that was too low plus that floor — the message is the only place a caller
/// learns the floor, since `ResourceCapacity` carries no `@range`. (The INVERTED pair is a
/// different rule, pinned by `refusal_messages`.)
///
/// Rows differ along which bound is too low and along the floor itself: one unit for a
/// provisioned-throughput dimension of either service that publishes one, zero for a count
/// — each asserted AT its floor, which is accepted, and under it, which is not.
#[tokio::test]
async fn a_capacity_under_its_dimensions_floor_names_the_floor() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("bounds", 1, 1).await;

    let rows: &[FloorRow] = &[
        FloorRow {
            case: "a DynamoDB table's read capacity under its one-unit floor",
            namespace: "dynamodb",
            resource_id: "table/bounds",
            dimension: "dynamodb:table:ReadCapacityUnits",
            min: 0,
            max: 10,
            expected: Some("Minimum capacity cannot be less than 1"),
        },
        FloorRow {
            case: "a DynamoDB table's write maximum under the same floor",
            namespace: "dynamodb",
            resource_id: "table/bounds",
            dimension: "dynamodb:table:WriteCapacityUnits",
            min: 1,
            max: 0,
            expected: Some("Maximum capacity cannot be less than 1"),
        },
        FloorRow {
            case: "a Keyspaces table's write capacity under the same floor",
            namespace: "cassandra",
            resource_id: "keyspace/events/table/clicks",
            dimension: "cassandra:table:WriteCapacityUnits",
            min: 0,
            max: 10,
            expected: Some("Minimum capacity cannot be less than 1"),
        },
        FloorRow {
            case: "a negative minimum on a dimension whose floor is zero",
            namespace: "ecs",
            resource_id: "service/prod/web",
            dimension: "ecs:service:DesiredCount",
            min: -1,
            max: 10,
            expected: Some("Minimum capacity cannot be less than 0"),
        },
        FloorRow {
            case: "a negative maximum on a dimension whose floor is zero",
            namespace: "ecs",
            resource_id: "service/prod/web",
            dimension: "ecs:service:DesiredCount",
            min: 0,
            max: -3,
            expected: Some("Maximum capacity cannot be less than 0"),
        },
        FloorRow {
            case: "a DynamoDB window AT the one-unit floor",
            namespace: "dynamodb",
            resource_id: "table/bounds",
            dimension: "dynamodb:table:ReadCapacityUnits",
            min: 1,
            max: 1,
            expected: None,
        },
        FloorRow {
            case: "scaling a count all the way in, which a zero floor allows",
            namespace: "ecs",
            resource_id: "service/prod/web",
            dimension: "ecs:service:DesiredCount",
            min: 0,
            max: 10,
            expected: None,
        },
    ];

    for row in rows {
        let target = json!({"ServiceNamespace": row.namespace, "ResourceId": row.resource_id,
                            "ScalableDimension": row.dimension});
        let request = registration(&target, row.min, row.max);
        match row.expected {
            Some(message) => {
                refused(
                    &emulator,
                    row.case,
                    "RegisterScalableTarget",
                    request,
                    "ValidationException",
                    message,
                )
                .await;
            }
            None => {
                // An accepted row's resource has to exist, because a registration resolves
                // it; a refused one is refused on its window before anything is resolved.
                emulator
                    .given_resource(row.namespace, row.resource_id, row.min)
                    .await;
                accepted(&emulator, row.case, "RegisterScalableTarget", request).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A scheduled action's time zone and window
// ---------------------------------------------------------------------------

/// RULE: an unresolvable `Timezone` is refused with the fixed phrase plus the name it could
/// not resolve; a missing `Schedule` on the create path and a window whose boundaries are
/// in the wrong order each with their own fixed sentence. (The schedule GRAMMAR is pinned
/// by `refusal_messages`.)
///
/// The time-zone rows differ along how the name fails to be a canonical IANA zone (the
/// wrong case, an abbreviation, a zone that does not exist), with two canonical names
/// accepted beside them.
#[tokio::test]
async fn a_scheduled_actions_zone_and_window_refusals_are_fixed_sentences() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let action = |name: &str, patch: Value| {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!(name);
        for (member, value) in patch.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        request
    };

    // (case, Timezone, refused)
    let zones: &[(&str, &str, bool)] = &[
        ("a canonical name in the wrong case", "europe/london", true),
        ("an abbreviation, not a canonical name", "PST", true),
        ("a zone that does not exist", "Mars/Olympus", true),
        ("a canonical name", "Europe/London", false),
        ("the default zone, named explicitly", "UTC", false),
    ];
    for (index, (case, timezone, is_refused)) in zones.iter().enumerate() {
        let request = action(
            &format!("zone-{index}"),
            json!({"Schedule": "cron(0 9 * * ? *)", "Timezone": timezone}),
        );
        if *is_refused {
            refused(
                &emulator,
                case,
                "PutScheduledAction",
                request,
                "ValidationException",
                &format!("Invalid schedule timezone: {timezone}"),
            )
            .await;
        } else {
            accepted(&emulator, case, "PutScheduledAction", request).await;
        }
    }

    refused(
        &emulator,
        "no Schedule on the create path",
        "PutScheduledAction",
        action("unscheduled", json!({})),
        "ValidationException",
        SCHEDULE_MUST_BE_SET,
    )
    .await;
    // Two windows in the wrong order, a day apart and a second apart: the rule is the
    // ORDER of the pair, not the distance between them.
    for (case, start, end) in [
        ("a window a day the wrong way round", 1_900_086_400, 1_900_000_000),
        ("a window one second the wrong way round", 1_900_000_001, 1_900_000_000),
    ] {
        refused(
            &emulator,
            case,
            "PutScheduledAction",
            action(
                "inverted-window",
                json!({"Schedule": "rate(1 hour)", "StartTime": start, "EndTime": end}),
            ),
            "ValidationException",
            START_TIME_BEFORE_END_TIME,
        )
        .await;
    }
    // The boundaries in order are a window the action can run in.
    accepted(
        &emulator,
        "a window in order",
        "PutScheduledAction",
        action(
            "window",
            json!({"Schedule": "rate(1 hour)",
                   "StartTime": 1_900_000_000, "EndTime": 1_900_086_400}),
        ),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Which metrics a scalable dimension is scaled on
// ---------------------------------------------------------------------------

/// The predefined metric types AWS publishes for a scalable dimension, in the order the
/// "Predefined metrics for target tracking scaling policies" table of the Application Auto
/// Scaling User Guide lists them (which is the order of the model's own `MetricType` enum).
///
/// Written out by hand rather than read from the emulator's catalog, so the assertion is
/// against the PUBLISHED set; the rows are dimensions the issue did not cite beside the one
/// it did.
const PUBLISHED_METRICS: &[(&str, &str)] = &[
    (
        "cassandra:table:ReadCapacityUnits",
        "CassandraReadCapacityUtilization",
    ),
    (
        "dynamodb:table:WriteCapacityUnits",
        "DynamoDBWriteCapacityUtilization",
    ),
    (
        "lambda:function:ProvisionedConcurrency",
        "LambdaProvisionedConcurrencyUtilization",
    ),
    (
        "ecs:service:DesiredCount",
        "ALBRequestCountPerTarget, ECSServiceAverageCPUUtilization, \
         ECSServiceAverageMemoryUtilization, ECSServiceAverageCPUUtilizationHighResolution, \
         ECSServiceAverageMemoryUtilizationHighResolution",
    ),
];

/// RULE: a predefined metric type a scalable dimension is not scaled on is refused with a
/// sentence that ENUMERATES the types that dimension does support — which is what makes the
/// refusal recoverable — built from the request and the catalog.
#[tokio::test]
async fn a_metric_a_dimension_is_not_scaled_on_is_refused_with_the_set_it_supports() {
    for (dimension, supported) in PUBLISHED_METRICS {
        let emulator = Emulator::empty();
        let target = emulator.given_target(dimension, 1, 10).await;
        // A metric of ANOTHER service's resources, which this dimension does not publish:
        // the refusal is the same whichever foreign metric is named.
        let foreign = if dimension.starts_with("ecs:") {
            "DynamoDBReadCapacityUtilization"
        } else {
            "ECSServiceAverageCPUUtilization"
        };
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 50.0,
            "PredefinedMetricSpecification": {"PredefinedMetricType": foreign},
        });
        refused(
            &emulator,
            &format!("{foreign} on {dimension}"),
            "PutScalingPolicy",
            request,
            "ValidationException",
            &format!(
                "Scalable dimension {dimension} only supports the following predefined metric \
                 types: {supported}"
            ),
        )
        .await;
    }
}

/// RULE: a customized metric on a dimension that is scaled only from its own
/// consumed-capacity utilisation is refused with that dimension's own sentence.
///
/// Rows differ along the dimension: the two services whose capacity IS a provisioned
/// throughput refuse one, and a dimension scaled from a signal of the caller's choosing
/// accepts one.
#[tokio::test]
async fn a_customized_metric_is_refused_where_the_dimension_does_not_support_one() {
    let customized = json!({
        "TargetValue": 50.0,
        "CustomizedMetricSpecification": {
            "MetricName": "QueueDepth",
            "Namespace": "Custom/Orders",
            "Statistic": "Average",
        },
    });
    for (case, dimension, supported) in [
        (
            "a DynamoDB table's write capacity",
            "dynamodb:table:WriteCapacityUnits",
            false,
        ),
        (
            "a DynamoDB index's read capacity",
            "dynamodb:index:ReadCapacityUnits",
            false,
        ),
        (
            "a Keyspaces table's read capacity",
            "cassandra:table:ReadCapacityUnits",
            false,
        ),
        (
            "an ECS service's task count",
            "ecs:service:DesiredCount",
            true,
        ),
    ] {
        let emulator = Emulator::empty();
        let target = emulator.given_target(dimension, 1, 10).await;
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = customized.clone();
        if supported {
            accepted(&emulator, case, "PutScalingPolicy", request).await;
        } else {
            refused(
                &emulator,
                case,
                "PutScalingPolicy",
                request,
                "ValidationException",
                &format!(
                    "Scalable dimension {dimension} does not support \
                     CustomizedMetricSpecification."
                ),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// A policy's configuration
// ---------------------------------------------------------------------------

/// RULE: a policy whose type owes a configuration it did not carry is refused with the
/// sentence naming that TYPE, and a second target-tracking policy on a metric one of the
/// target's policies already tracks with one fixed sentence naming neither policy.
///
/// Rows differ along the policy type, so the sentence is built from the request rather than
/// written per type.
#[tokio::test]
async fn a_policy_configuration_refusal_names_the_policy_type() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    for policy_type in ["TargetTrackingScaling", "StepScaling", "PredictiveScaling"] {
        let mut request = target.clone();
        request["PolicyName"] = json!(format!("{policy_type}-no-config"));
        request["PolicyType"] = json!(policy_type);
        refused(
            &emulator,
            &format!("{policy_type} with no configuration"),
            "PutScalingPolicy",
            request,
            "ValidationException",
            &format!("{policy_type} policy configuration must be specified"),
        )
        .await;
    }

    // Two policies, one metric: the first lands, the second is the conflict.
    let tracking = json!({
        "TargetValue": 50.0,
        "PredefinedMetricSpecification": {
            "PredefinedMetricType": "DynamoDBReadCapacityUtilization",
        },
    });
    let mut first = target.clone();
    first["PolicyName"] = json!("first");
    first["PolicyType"] = json!("TargetTrackingScaling");
    first["TargetTrackingScalingPolicyConfiguration"] = tracking.clone();
    accepted(&emulator, "the first policy", "PutScalingPolicy", first).await;

    let mut second = target.clone();
    second["PolicyName"] = json!("second");
    second["PolicyType"] = json!("TargetTrackingScaling");
    second["TargetTrackingScalingPolicyConfiguration"] = tracking;
    refused(
        &emulator,
        "a second policy on the same metric",
        "PutScalingPolicy",
        second,
        "ValidationException",
        "Only one TargetTrackingScaling policy for a given metric specification is allowed.",
    )
    .await;
}

// ---------------------------------------------------------------------------
// What a read refuses
// ---------------------------------------------------------------------------

/// RULE: a dimension filter with no resource to narrow is refused with one fixed sentence,
/// and a `NextToken` this service did not issue with another — the same two sentences on
/// every read that enforces them.
///
/// Rows differ along the operation, which is what makes each a rule of the service rather
/// than of one handler.
#[tokio::test]
async fn a_reads_refusals_are_the_same_sentence_on_every_read() {
    let emulator = Emulator::empty();
    let namespace = json!({"ServiceNamespace": "dynamodb"});
    let dimension = "dynamodb:table:WriteCapacityUnits";

    // The reads whose filter is a single `ResourceId`, which a dimension filter needs.
    for operation in ["DescribeScheduledActions", "DescribeScalingActivities"] {
        let mut request = namespace.clone();
        request["ScalableDimension"] = json!(dimension);
        refused(
            &emulator,
            &format!("a dimension filter with no resource id on {operation}"),
            operation,
            request,
            "ValidationException",
            DIMENSION_WITHOUT_RESOURCE_ID,
        )
        .await;

        // With the resource id the filter names a resource, and the read answers.
        let mut narrowed = namespace.clone();
        narrowed["ScalableDimension"] = json!(dimension);
        narrowed["ResourceId"] = json!("table/any");
        accepted(
            &emulator,
            &format!("a dimension filter beside a resource id on {operation}"),
            operation,
            narrowed,
        )
        .await;
    }

    // Every paginated read refuses a token it did not issue, with the same sentence.
    for operation in [
        "DescribeScalableTargets",
        "DescribeScalingPolicies",
        "DescribeScheduledActions",
        "DescribeScalingActivities",
    ] {
        let mut request = namespace.clone();
        request["NextToken"] = json!("bogus");
        refused(
            &emulator,
            &format!("a token {operation} did not issue"),
            operation,
            request,
            "InvalidNextTokenException",
            INVALID_NEXT_TOKEN,
        )
        .await;
    }
}

/// One list-valued member whose `@length` the framework enforces, and how its entries are
/// spelled.
struct ListMember {
    operation: &'static str,
    member: &'static str,
    /// The dotted path the constraint clause names the member by.
    path: &'static str,
    entry: fn(usize) -> String,
}

/// RULE: a violation of a constraint the MODEL declares stays in the framework's
/// constraint envelope — and the clause echoes the member's value AS SUBMITTED, so a
/// list-valued member is echoed as its elements rather than as a count of them.
///
/// Rows differ along the operation and the member (`DescribeScalableTargets.ResourceIds`
/// and `DescribeScheduledActions.ScheduledActionNames`, both `@length` 0..50) and along how
/// far past the bound the request is; each bound is also asserted AT its published maximum,
/// which is accepted.
#[tokio::test]
async fn a_list_too_long_is_refused_with_the_list_the_caller_sent() {
    let emulator = Emulator::empty();

    let members: &[ListMember] = &[
        ListMember {
            operation: "DescribeScalableTargets",
            member: "ResourceIds",
            path: "resourceIds",
            entry: |index| format!("table/t-{index}"),
        },
        ListMember {
            operation: "DescribeScheduledActions",
            member: "ScheduledActionNames",
            path: "scheduledActionNames",
            entry: |index| format!("action-{index}"),
        },
    ];

    for row in members {
        let ListMember {
            operation,
            member,
            path,
            entry,
        } = row;
        for count in [51_usize, 60] {
            let entries: Vec<String> = (0..count).map(*entry).collect();
            let mut request = json!({"ServiceNamespace": "dynamodb"});
            request[*member] = json!(entries);
            let case = format!("{count} entries in {operation}.{member}");
            let expected = format!(
                "1 validation error detected: Value '[{}]' at '{path}' failed to satisfy \
                 constraint: Member must have length less than or equal to 50",
                entries.join(", ")
            );
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the clause to echo the list as submitted \
                 ({expected:?}), got {actual:?}"
            );
        }

        // AT the published bound the request is accepted, so what is refused is the bound
        // and not the member.
        let entries: Vec<String> = (0..50).map(*entry).collect();
        let mut request = json!({"ServiceNamespace": "dynamodb"});
        request[*member] = json!(entries);
        accepted(
            &emulator,
            &format!("50 entries in {operation}.{member}"),
            operation,
            request,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// A target that is not registered
// ---------------------------------------------------------------------------

/// RULE: an operation addressed at a scalable target that does not exist is refused with
/// the service's own sentence for that operation, built from the three identity members of
/// the request — `DeregisterScalableTarget` went looking for the registration in order to
/// remove it, so it reports that none was FOUND.
///
/// Rows differ along the identity (three namespaces, three resource-id forms) and along
/// why the target is absent: never registered, or deregistered already.
#[tokio::test]
async fn deregistering_a_target_that_is_not_registered_names_the_identity() {
    let emulator = Emulator::empty();

    let identities: &[(&str, &str, &str, &str)] = &[
        (
            "a DynamoDB table that was never registered",
            "dynamodb",
            "table/never",
            "dynamodb:table:WriteCapacityUnits",
        ),
        (
            "an ECS service that was never registered",
            "ecs",
            "service/prod/web",
            "ecs:service:DesiredCount",
        ),
        (
            "a Lambda alias that was never registered",
            "lambda",
            "function:ingest:green",
            "lambda:function:ProvisionedConcurrency",
        ),
    ];
    for (case, namespace, resource_id, dimension) in identities {
        refused(
            &emulator,
            case,
            "DeregisterScalableTarget",
            json!({"ServiceNamespace": namespace, "ResourceId": resource_id,
                   "ScalableDimension": dimension}),
            "ObjectNotFoundException",
            &format!(
                "No scalable target found for service namespace: {namespace}, resource ID: \
                 {resource_id}, scalable dimension: {dimension}"
            ),
        )
        .await;
    }

    // And a target that WAS registered: the first deregistration succeeds, the second is
    // refused with the same sentence.
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 4).await;
    accepted(
        &emulator,
        "the first deregistration",
        "DeregisterScalableTarget",
        target.clone(),
    )
    .await;
    refused(
        &emulator,
        "a target that was deregistered already",
        "DeregisterScalableTarget",
        target,
        "ObjectNotFoundException",
        "No scalable target found for service namespace: ecs, resource ID: \
         service/prod-cluster/web-app, scalable dimension: ecs:service:DesiredCount",
    )
    .await;
}

/// RULE: the sentence an absent scalable target is reported with follows what the operation
/// was DOING with the registration — an operation that cannot proceed without one ("You
/// cannot create a scaling policy until you have registered the resource as a scalable
/// target", and the same for a scheduled action) reports that no scalable target is
/// REGISTERED for the triple, while the operation that looked the registration up in order
/// to remove it reports that none was FOUND. The three identity members are the request's in
/// every row.
#[tokio::test]
async fn the_absent_target_sentence_follows_what_the_operation_wanted_the_registration_for() {
    let emulator = Emulator::empty();
    let namespace = "dynamodb";
    let resource_id = "table/absent";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let identity = json!({"ServiceNamespace": namespace, "ResourceId": resource_id,
                          "ScalableDimension": dimension});

    let mut policy = identity.clone();
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"},
    });
    let mut action = identity.clone();
    action["ScheduledActionName"] = json!("nightly");
    action["Schedule"] = json!("rate(5 minutes)");

    for (case, operation, request, verb) in [
        (
            "a policy on an absent target",
            "PutScalingPolicy",
            policy,
            "registered",
        ),
        (
            "a scheduled action on an absent target",
            "PutScheduledAction",
            action,
            "registered",
        ),
        (
            "deregistering an absent target",
            "DeregisterScalableTarget",
            identity.clone(),
            "found",
        ),
    ] {
        refused(
            &emulator,
            case,
            operation,
            request,
            "ObjectNotFoundException",
            &format!(
                "No scalable target {verb} for service namespace: {namespace}, resource ID: \
                 {resource_id}, scalable dimension: {dimension}"
            ),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Tagging
// ---------------------------------------------------------------------------

/// `count` distinct tags.
fn generated_tags(count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index:03}"), json!("v"));
    }
    Value::Object(tags)
}

/// RULE: the tagging refusals each carry their own sentence — an ARN that resolves to no
/// registered scalable target is reported absent by the READ and refuses the WRITE, a tag set
/// over the per-resource maximum states that maximum, and a key in the reserved namespace is
/// refused as a system tag — so a client can tell them apart by text as well as by code.
///
/// Rows differ along the operation (all three tag operations), along how the ARN fails
/// to resolve, along the batch size either side of the maximum, and along how the reserved
/// prefix appears.
#[tokio::test]
async fn the_tagging_refusals_are_distinct_sentences() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let registered = accepted(
        &emulator,
        "the resident target",
        "RegisterScalableTarget",
        registration(&target, 1, 10),
    )
    .await;
    let arn = registered["ScalableTargetARN"]
        .as_str()
        .expect("a ScalableTargetARN")
        .to_string();

    // Every ARN below satisfies the modelled `@pattern`, so each reaches the handler.
    let unresolvable: &[(&str, String)] = &[
        (
            "an ARN of a target that was never registered",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
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
    for (case, unknown) in unresolvable {
        // Every operation of the family reports the resource as absent for an ARN that
        // resolves to none, at the same client-error status.
        for (operation, request) in [
            (
                "TagResource",
                json!({"ResourceARN": unknown, "Tags": {"team": "search"}}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": unknown, "TagKeys": ["team"]}),
            ),
            ("ListTagsForResource", json!({"ResourceARN": unknown})),
        ] {
            refused_with_status(
                &emulator,
                &format!("{operation} on {case}"),
                operation,
                request,
                400,
                "ResourceNotFoundException",
                TARGET_NOT_FOUND,
            )
            .await;
        }
    }

    // One batch past the per-resource maximum: the sentence states the maximum, and the
    // maximum itself is accepted.
    for count in [MAX_TAGS_PER_RESOURCE + 1, MAX_TAGS_PER_RESOURCE + 10] {
        refused(
            &emulator,
            &format!("a batch of {count} tags"),
            "TagResource",
            json!({"ResourceARN": arn, "Tags": generated_tags(count)}),
            "ValidationException",
            TAG_SET_TOO_LARGE,
        )
        .await;
    }
    accepted(
        &emulator,
        &format!("a batch of {MAX_TAGS_PER_RESOURCE} tags"),
        "TagResource",
        json!({"ResourceARN": arn, "Tags": generated_tags(MAX_TAGS_PER_RESOURCE)}),
    )
    .await;

    // A key in the reserved namespace, however it appears in the batch.
    let reserved: &[(&str, Value)] = &[
        ("a reserved key alone", json!({"aws:cost-center": "1234"})),
        (
            "the reserved prefix in upper case",
            json!({"AWS:Service": "application-autoscaling"}),
        ),
        (
            "a reserved key beside legal ones",
            json!({"team": "search", "aws:created-by": "cfn"}),
        ),
    ];
    for (case, tags) in reserved {
        refused(
            &emulator,
            &format!("TagResource with {case}"),
            "TagResource",
            json!({"ResourceARN": arn, "Tags": tags}),
            "ValidationException",
            SYSTEM_TAG_KEY_REFUSED,
        )
        .await;
    }
    refused(
        &emulator,
        "UntagResource with a reserved key",
        "UntagResource",
        json!({"ResourceARN": arn, "TagKeys": ["aws:cost-center"]}),
        "ValidationException",
        SYSTEM_TAG_KEY_REFUSED,
    )
    .await;
}
