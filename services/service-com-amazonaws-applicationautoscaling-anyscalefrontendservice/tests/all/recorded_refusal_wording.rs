// SPDX-License-Identifier: CC0-1.0

//! The refusals whose WORDING is the recorded one, and the one refusal of this service that
//! is a modelled-constraint message.
//!
//! Three rules, each asserted over a table rather than over the one request the issue cited:
//!
//! * a value outside a constraint the MODEL declares is refused with AWS's standard
//!   constraint-violation message — the clause that names the member path, echoes the value
//!   the caller submitted and quotes the expression or bound it had to satisfy — which is what
//!   a `ScheduledActionName` breaking its `@pattern` carries;
//! * a (`ServiceNamespace`, `ResourceId`, `ScalableDimension`) triple whose three members do
//!   not describe one resource between them is refused with the fixed sentence for an
//!   unsupported combination, which interpolates none of the three; a resource that cannot
//!   carry the target at all is refused with the "Validation failed for scalable target"
//!   envelope and the reason it cannot; and a predefined metric whose series cannot be
//!   resolved without a resource label says which metric type needs one;
//! * an operation addressed at something no scalable target holds reports it as absent: the
//!   `ObjectNotFoundException` template naming the triple for an identity nothing registered,
//!   and the one naming the TARGET's own identifier for a scheduled action a registered target
//!   does not hold; and, for an ARN, the `ResourceNotFoundException` body at the client-error
//!   status on the tag READ, where each tag MUTATION refuses the write it was asked to make
//!   with the recorded `ValidationException`.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, scalable_target_id, Emulator, ACCOUNT, REGION};

/// The `@pattern` the model declares on `ScheduledActionName`, as the MODEL declares it
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScheduledActionName")`).
/// Spelled out here rather than read from the service, so the expectation is against the
/// published constraint.
const NAME_PATTERN: &str =
    r"^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$";

/// The expression a refusal QUOTES for [`NAME_PATTERN`]: the declared one without the `^` and
/// `$` the model wraps it in to turn Smithy's find into a whole-value match, which is the
/// expression the service publishes. Computed from the declared text rather than spelled out a
/// second time; the `aws_worded_client_errors` suite asserts the same rendering over every
/// member of this service that declares a `@pattern`.
fn published_name_pattern() -> String {
    NAME_PATTERN[1..NAME_PATTERN.len() - 1].to_string()
}

/// The published bounds of `ScheduledActionName`: `@length(min: 1, max: 256)`, same source.
const MAX_NAME_CHARS: usize = 256;

/// Names the pattern forbids, one per alternative of it: a forbidden character anywhere, a
/// leading or trailing space, and a control character from either range.
const ILLEGAL_NAMES: &[(&str, &str)] = &[
    ("a slash", "scale-up/weekday"),
    ("a colon", "scale:up"),
    ("a pipe", "scale|up"),
    ("two forbidden characters", "scale:up/now"),
    ("a leading space", " scale-up"),
    ("a trailing space", "scale-up "),
    ("a C0 control character", "scale\u{1}up"),
    ("a C1 control character", "scale\u{85}up"),
];

#[tokio::test]
async fn a_scheduled_action_name_outside_its_pattern_carries_the_modelled_constraint_message() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    for (case, name) in ILLEGAL_NAMES {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!(name);
        request["Schedule"] = json!("rate(5 minutes)");
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{name}' at 'scheduledActionName' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: {}",
            published_name_pattern()
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The other constraint the same member declares is its `@length`, and a violation of it
    // is stated in the same form — so the two cannot drift: one name at the published maximum
    // is accepted, and one character past it is refused with the length clause.
    let at_bound = "n".repeat(MAX_NAME_CHARS);
    let mut request = target.clone();
    request["ScheduledActionName"] = json!(at_bound);
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"a name at the published maximum\": expected 200, got {status} {body:?}"
    );

    let past_bound = "n".repeat(MAX_NAME_CHARS + 1);
    let mut request = target.clone();
    request["ScheduledActionName"] = json!(past_bound);
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 400,
        "case \"a name one past the published maximum\": expected HTTP 400, got {status} {body:?}"
    );
    let message = error_message(&body);
    let expected = format!(
        "1 validation error detected: Value '{past_bound}' at 'scheduledActionName' failed to \
         satisfy constraint: Member must have length less than or equal to {MAX_NAME_CHARS}"
    );
    assert_eq!(
        message, expected,
        "case \"a name one past the published maximum\": expected {expected:?}, got {message:?}"
    );

    // And a name the pattern allows is stored, so neither check refuses a legal name.
    let mut request = target.clone();
    request["ScheduledActionName"] = json!("scale up nightly.2_a");
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"a legal name\": expected 200, got {status} {body:?}"
    );
}

/// The sentence a triple whose three members do not describe one resource is refused with.
const UNSUPPORTED_TRIPLE: &str = "Unsupported service namespace, resource type or scalable \
                                  dimension";

/// One registration whose triple does not describe one resource, whatever the resource store
/// holds.
///
/// The rows pair a resource id of one resource type with a dimension of another, across
/// namespaces whose id forms differ in shape (a path, a table/index pair, a colon-qualified
/// name, an ARN) — including pairs the issue did not cite.
const MISMATCHED_TRIPLES: &[(&str, &str, &str, &str)] = &[
    (
        "a table id on an index dimension",
        "dynamodb",
        "table/orders",
        "dynamodb:index:ReadCapacityUnits",
    ),
    (
        "an index id on a table dimension",
        "dynamodb",
        "table/orders/index/by-customer",
        "dynamodb:table:WriteCapacityUnits",
    ),
    (
        "an ECS service id without its cluster",
        "ecs",
        "service/web-app",
        "ecs:service:DesiredCount",
    ),
    (
        "a Keyspaces table in the DynamoDB form",
        "cassandra",
        "table/clicks",
        "cassandra:table:ReadCapacityUnits",
    ),
    (
        "an RDS cluster written with a slash",
        "rds",
        "cluster/orders-db",
        "rds:cluster:ReadReplicaCount",
    ),
    (
        "a SageMaker variant without its endpoint",
        "sagemaker",
        "variant/blue",
        "sagemaker:variant:DesiredInstanceCount",
    ),
];

#[tokio::test]
async fn a_triple_that_describes_no_resource_is_refused_as_an_unsupported_combination() {
    for (case, namespace, resource_id, dimension) in MISMATCHED_TRIPLES {
        let emulator = Emulator::empty();
        // The rule is about the triple, not about what the resource store holds: the table
        // and the index both exist here, and the refusal is the same.
        emulator
            .create_table_with_index("orders", "by-customer", "customer", 2, 2)
            .await;

        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": namespace, "ResourceId": resource_id,
                       "ScalableDimension": dimension, "MinCapacity": 1, "MaxCapacity": 10}),
            )
            .await;
        assert_refused(case, status, &body, UNSUPPORTED_TRIPLE);
    }
}

#[tokio::test]
async fn a_table_that_provisions_no_throughput_is_refused_with_the_reason_it_cannot_scale() {
    /// The recorded refusal for a DynamoDB table in the billing mode that provisions no
    /// throughput at all: the envelope, and the reason that mode is not scalable.
    const NOT_SCALABLE: &str =
        "Validation failed for scalable target. Reason: PAY_PER_REQUEST table mode is not \
         scalable.";

    // Both capacity dimensions of the table: the reason is the table's billing mode, so it is
    // the same whichever property the dimension names.
    for dimension in [
        "dynamodb:table:ReadCapacityUnits",
        "dynamodb:table:WriteCapacityUnits",
    ] {
        let emulator = Emulator::empty();
        emulator.create_on_demand_table("on-demand").await;
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/on-demand",
                       "ScalableDimension": dimension, "MinCapacity": 1, "MaxCapacity": 10}),
            )
            .await;
        assert_refused(dimension, status, &body, NOT_SCALABLE);
    }
}

#[tokio::test]
async fn a_predefined_metric_that_needs_a_resource_label_says_which_type_does() {
    /// The one predefined metric type whose series is counted per target group, so it cannot
    /// be resolved without the resource the label names.
    const METRIC_TYPE: &str = "ALBRequestCountPerTarget";

    let emulator = Emulator::empty();
    // Two dimensions that are scaled on this metric type, so the sentence is a rule about the
    // metric type rather than about one scalable dimension.
    for dimension in ["ecs:service:DesiredCount", "ec2:spot-fleet-request:TargetCapacity"] {
        let target = emulator.given_target(dimension, 1, 20).await;
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 1000.0,
            "PredefinedMetricSpecification": {"PredefinedMetricType": METRIC_TYPE},
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_refused(
            dimension,
            status,
            &body,
            &format!("Resource label should be specified for predefined metric type {METRIC_TYPE}"),
        );

        // With the label the same policy is accepted, so what is refused is the missing
        // label and not the metric type.
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 1000.0,
            "PredefinedMetricSpecification": {
                "PredefinedMetricType": METRIC_TYPE,
                "ResourceLabel": "app/web/73e2d6bc24d8a067/targetgroup/orders/dbf2bd4a9c7e0e4a",
            },
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected the policy with a resource label to be accepted, got \
             {status} {body:?}"
        );
    }
}

/// One operation addressed at an identity nothing registered, and the four members its
/// refusal names.
///
/// The rows span the operations that need a registration to proceed and the ways an identity
/// can miss one: no target at all, a target registered on another dimension, and a scheduled
/// action no target holds.
#[tokio::test]
async fn an_identity_nothing_registered_is_reported_as_not_registered() {
    const NAMESPACE: &str = "dynamodb";
    const RESOURCE_ID: &str = "table/catalog";
    const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";
    /// A dimension of the same resource that nothing registers.
    const OTHER_DIMENSION: &str = "dynamodb:table:WriteCapacityUnits";

    let emulator = Emulator::empty();
    let identity = json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": RESOURCE_ID,
        "ScalableDimension": DIMENSION,
    });

    // Nothing is registered yet: a policy and a scheduled action both report the triple.
    let mut policy = identity.clone();
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 50.0,
        "PredefinedMetricSpecification": {
            "PredefinedMetricType": "DynamoDBReadCapacityUtilization",
        },
    });
    let (status, body) = emulator.call("PutScalingPolicy", policy).await;
    assert_not_found(
        "a policy on an unregistered target",
        status,
        &body,
        &target_not_registered(NAMESPACE, RESOURCE_ID, DIMENSION),
    );

    let mut action = identity.clone();
    action["ScheduledActionName"] = json!("nightly");
    action["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", action).await;
    assert_not_found(
        "a scheduled action on an unregistered target",
        status,
        &body,
        &target_not_registered(NAMESPACE, RESOURCE_ID, DIMENSION),
    );

    // With the target registered, the OTHER dimension of the same resource is still an
    // identity nothing registered.
    let target = emulator.given_target(DIMENSION, 1, 10).await;
    let mut policy = target.clone();
    policy["ScalableDimension"] = json!(OTHER_DIMENSION);
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 50.0,
        "PredefinedMetricSpecification": {
            "PredefinedMetricType": "DynamoDBWriteCapacityUtilization",
        },
    });
    let (status, body) = emulator.call("PutScalingPolicy", policy).await;
    assert_not_found(
        "a policy on an unregistered dimension",
        status,
        &body,
        &target_not_registered(NAMESPACE, RESOURCE_ID, OTHER_DIMENSION),
    );

    // And a scheduled action the target DOES hold a registration for, but not that action, is
    // reported as the missing action: the TARGET's own identifier beside the action's name,
    // over a stored action's name misspelled as well as a name nothing ever stored.
    let mut stored = target.clone();
    stored["ScheduledActionName"] = json!("nightly");
    stored["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", stored).await;
    assert_eq!(
        status, 200,
        "case \"the stored action\": expected 200, got {status} {body:?}"
    );
    let target_id = scalable_target_id(&emulator.target_arn(&target).await);
    for (case, name) in [
        ("a name nothing stored", "weekly"),
        ("the stored name misspelled", "Nightly"),
    ] {
        let mut delete = target.clone();
        delete["ScheduledActionName"] = json!(name);
        let (status, body) = emulator.call("DeleteScheduledAction", delete).await;
        assert_not_found(
            case,
            status,
            &body,
            &format!(
                "No scheduled action found for service namespace: {NAMESPACE}, scalable target \
                 ID: {target_id}, scheduled action name: {name}"
            ),
        );
    }
}

#[tokio::test]
async fn a_tag_operation_on_an_arn_naming_no_target_answers_the_recorded_refusal() {
    /// The sentence every operation of the tagging family carries for an ARN that resolves to
    /// no registered scalable target.
    const TARGET_NOT_FOUND: &str = "Scalable Target not found";

    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": target["ServiceNamespace"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the registered target\": expected 200, got {status} {body:?}"
    );
    let registered = body["ScalableTargets"][0]["ScalableTargetARN"]
        .as_str()
        .expect("the registered target's ARN")
        .to_string();

    // While the target is registered all three tag operations answer it.
    for (operation, request) in tag_requests(&registered) {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 200,
            "case \"a registered target\" on {operation}: expected 200, got {status} {body:?}"
        );
    }

    // Once it is deregistered that ARN resolves to no registered scalable target, so all
    // three answer alike, at the client-error status: the RESOURCE is reported absent, with
    // the ARN asked about in `ResourceName`.
    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected 200, got {status} {body:?}"
    );
    for (operation, request) in tag_requests(&registered) {
        let case = "a deregistered target's ARN";
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on {operation}: expected a ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            message, TARGET_NOT_FOUND,
            "case {case:?} on {operation}: expected {TARGET_NOT_FOUND:?}, got {message:?}"
        );
        assert_eq!(
            body["ResourceName"],
            json!(registered),
            "case {case:?} on {operation}: expected the ARN asked about in ResourceName, got \
             {body:?}"
        );
    }

    // An ARN this service never minted resolves to no registered scalable target either, so
    // each operation gives the SAME answer it gives for the deregistered one. Over several
    // ways an ARN can be one it never minted.
    let never_minted: &[(&str, String)] = &[
        (
            "an id this service never minted",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an id of another shape",
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/nothing"),
        ),
        (
            "an ARN of another account",
            format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an ARN of another Region",
            format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
    ];
    for (case, arn) in never_minted {
        for (operation, request) in tag_requests(arn) {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected a ResourceNotFoundException, got {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, TARGET_NOT_FOUND,
                "case {case:?} on {operation}: expected {TARGET_NOT_FOUND:?}, got {message:?}"
            );
        }
    }
}

/// One request per tag operation, all three addressed to `resource_arn`.
fn tag_requests(resource_arn: &str) -> [(&'static str, Value); 3] {
    [
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": {"owner": "orders-team"}}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["owner"]}),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceARN": resource_arn}),
        ),
    ]
}

/// The `ObjectNotFoundException` template for a scalable target nothing registered.
fn target_not_registered(namespace: &str, resource_id: &str, dimension: &str) -> String {
    format!(
        "No scalable target registered for service namespace: {namespace}, resource ID: \
         {resource_id}, scalable dimension: {dimension}"
    )
}

/// Assert a 400 `ValidationException` carrying exactly `expected`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Assert an `ObjectNotFoundException` carrying exactly `expected`.
fn assert_not_found(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ObjectNotFoundException",
        "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}
