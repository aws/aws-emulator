// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request addressed to something nothing registered, over the
//! wire.
//!
//! Three rules, each asserted over a TABLE rather than over the one request an issue cited:
//!
//! * an operation that depends on the existence of a scalable target resolves the
//!   (`ServiceNamespace`, `ResourceId`, `ScalableDimension`) triple as that target's identity
//!   and, when nothing is registered under it, reports the missing TARGET — the
//!   `ObjectNotFoundException` template naming those three members of the request — rather
//!   than the missing sub-resource it was going to touch or a constraint on one member. A
//!   dimension belonging to another namespace is one such triple: nothing could have
//!   registered it, so it names no target either.
//! * the ARN-addressed tag operations answer an ARN that resolves to no registered scalable
//!   target at the client-error status, each according to what it was asked to do: the READ
//!   reports the resource as absent (`ResourceNotFoundException`, the fixed sentence "Scalable
//!   Target not found" and the ARN asked about in `ResourceName`), while each MUTATION refuses
//!   the write it was asked to make (`ValidationException` and the fixed sentence "Access
//!   Denied", disclosing neither the absence nor the ARN) — one answer per operation,
//!   whichever way the ARN fails to resolve.
//! * an operation this service does not offer for the namespace addressed is refused
//!   outright, with `AccessDeniedException` and the fixed sentence naming the operation,
//!   whatever is or is not registered. Predictive scaling is offered in one namespace: the
//!   model documents "PredictiveScaling—Only supported for Amazon ECS" on
//!   `PutScalingPolicyRequest$PolicyType`
//!   (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#PutScalingPolicyRequest",member="PolicyType")`).

use serde_json::{json, Value};

use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::ops::types::SERVICE_NAMESPACE_VALUES;

use crate::emulator::{error_code, error_message, resource_id_for, Emulator, ACCOUNT, REGION};

/// The sentence the tag READ carries for an ARN that resolves to no registered scalable
/// target, spelled out here rather than read from the implementation.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The sentence a tag MUTATION carries for the same ARN: the write is refused, and the answer
/// carries nothing of the request.
const TAGGING_DENIED: &str = "Access Denied";

/// The status a refusal of this class answers with. Both rules below answer the client-error
/// status: a client, and any proxy in front of it, branches on the status CLASS before it
/// parses a body.
const CLIENT_ERROR: u16 = 400;

// ---------------------------------------------------------------------------
// A triple nothing registered a scalable target for
// ---------------------------------------------------------------------------

/// One triple no scalable target was registered under, and the operations aimed at it.
struct TripleRow {
    /// What makes this triple one nothing registered.
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
}

/// The `ObjectNotFoundException` sentence for a triple nothing registered, built from the
/// three members of the REQUEST: `verb` is what the operation was doing with the registration
/// — one that needs an existing registration to proceed is told none is registered, one that
/// went looking for the registration itself is told none was found.
fn missing_target(row: &TripleRow, verb: &str) -> String {
    format!(
        "No scalable target {verb} for service namespace: {}, resource ID: {}, scalable \
         dimension: {}",
        row.namespace, row.resource_id, row.dimension
    )
}

/// RULE: an operation that depends on the existence of a scalable target reports the missing
/// TARGET for a triple nothing registered — not the sub-resource it was going to touch, and
/// not a constraint violation on one of the three members.
///
/// Rows differ along WHY nothing is registered (an identity never registered, a resource id
/// of another form, a dimension that belongs to another namespace, a namespace whose
/// resources this emulator does not even hold) and along the operation (a create, two
/// deletes of sub-resources, and the deregistration of the target itself), with one
/// registered target standing beside them so each row is a rule and not a blanket refusal.
#[tokio::test]
async fn an_operation_that_needs_a_registered_target_reports_the_missing_target() {
    let rows: &[TripleRow] = &[
        TripleRow {
            case: "an identity nothing ever registered",
            namespace: "dynamodb",
            resource_id: "table/never-registered",
            dimension: "dynamodb:table:ReadCapacityUnits",
        },
        TripleRow {
            case: "a dimension of another namespace",
            namespace: "dynamodb",
            resource_id: "table/catalog",
            dimension: "ecs:service:DesiredCount",
        },
        TripleRow {
            case: "a dimension of another namespace, the other way round",
            namespace: "ecs",
            resource_id: "service/prod-cluster/web-app",
            dimension: "dynamodb:index:WriteCapacityUnits",
        },
        TripleRow {
            case: "a custom resource nobody registered",
            namespace: "custom-resource",
            resource_id: "https://example.com/scalableTargetDimensions/orphan",
            dimension: "custom-resource:ResourceType:Property",
        },
        TripleRow {
            case: "a namespace whose resources this emulator holds no store for",
            namespace: "elasticache",
            resource_id: "replication-group/sessions",
            dimension: "elasticache:replication-group:Replicas",
        },
    ];

    let emulator = Emulator::empty();
    // One registered target, so a refusal below means "not this triple" rather than "nothing
    // at all" — and so the registered identity is the control the rows are read against.
    let registered = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    for row in rows {
        // Every operation that depends on the target existing, each with the verb its own
        // refusal uses: the four that address the target or one of its sub-resources to
        // create or remove something, and the deregistration that looks the registration
        // itself up.
        let requests: &[(&str, Value, &str)] = &[
            (
                "PutScalingPolicy",
                json!({
                    "PolicyName": "tracker",
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "PolicyType": "StepScaling",
                    "StepScalingPolicyConfiguration": {
                        "AdjustmentType": "ChangeInCapacity",
                        "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}],
                    },
                }),
                "registered",
            ),
            (
                "PutScheduledAction",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "ScheduledActionName": "nightly",
                    "Schedule": "rate(1 hour)",
                }),
                "registered",
            ),
            (
                "DeleteScheduledAction",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "ScheduledActionName": "nightly",
                }),
                "registered",
            ),
            (
                "DeregisterScalableTarget",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                }),
                "found",
            ),
        ];

        for (operation, request, verb) in requests {
            let case = format!("{} on {operation}", row.case);
            let (status, body) = emulator.call(operation, request.clone()).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ObjectNotFoundException",
                "case {case:?}: expected an ObjectNotFoundException, got {code:?} in {body:?}"
            );
            let expected = missing_target(row, verb);
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }

    // The control: the registered identity still answers, so the refusals above are about
    // what is registered and not about the operations themselves.
    let (status, body) = emulator
        .call("DeregisterScalableTarget", registered.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the registered target\": expected DeregisterScalableTarget to answer 200, got \
         {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// An ARN that resolves to no registered scalable target
// ---------------------------------------------------------------------------

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
        ("ListTagsForResource", json!({"ResourceARN": resource_arn})),
    ]
}

/// RULE: every tag operation answers an ARN that resolves to no registered scalable target at
/// the client-error status, according to what it was asked to do — the READ reports the
/// resource as absent with the fixed sentence and the ARN asked about in `ResourceName`, each
/// MUTATION refuses the write with `ValidationException` and the fixed sentence that carries
/// nothing of the request.
///
/// Rows differ along HOW the ARN fails to resolve — an id this service never minted, an id of
/// another form, another account, another Region, and the ARN of a target that WAS registered
/// and has been deregistered — and along the operation, including both writes, so the two
/// writes do not word the condition differently from each other.
#[tokio::test]
async fn every_tag_operation_answers_an_unresolvable_arn_in_its_own_terms() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let registered = emulator.target_arn(&target).await;

    // While the target is registered all three answer it: the refusals below are about the
    // ARN resolving to no target, not about the request being unreadable.
    for (operation, request) in tag_requests(&registered) {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 200,
            "case \"a registered target\" on {operation}: expected 200, got {status} {body:?}"
        );
    }

    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected 200, got {status} {body:?}"
    );

    // Every ARN below satisfies the modelled `@pattern` on `ResourceARN`
    // (^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$), so each
    // reaches the handler rather than the boundary validator.
    let unresolvable: &[(&str, String)] = &[
        (
            "the ARN of a target that has been deregistered",
            registered.clone(),
        ),
        (
            "an id this service never minted",
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an id of another form",
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

    for (row, arn) in unresolvable {
        for (operation, request) in tag_requests(arn) {
            let case = format!("{row} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} {body:?}"
            );
            let code = error_code(&body);
            let message = error_message(&body);
            if operation == "ListTagsForResource" {
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case:?}: expected a ResourceNotFoundException, got {code:?} in {body:?}"
                );
                assert_eq!(
                    message, TARGET_NOT_FOUND,
                    "case {case:?}: expected the message {TARGET_NOT_FOUND:?}, got {message:?}"
                );
                assert_eq!(
                    body["ResourceName"],
                    json!(arn),
                    "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
                );
                continue;
            }
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected a ValidationException, got {code:?} in {body:?}"
            );
            assert_eq!(
                message, TAGGING_DENIED,
                "case {case:?}: expected the message {TAGGING_DENIED:?}, got {message:?}"
            );
            assert_eq!(
                body["ResourceName"],
                Value::Null,
                "case {case:?}: expected the refusal to carry nothing of the request, got \
                 {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// An operation this service does not offer for the namespace addressed
// ---------------------------------------------------------------------------

/// The one namespace predictive scaling is offered in, from the model's own `PolicyType`
/// documentation: "PredictiveScaling—Only supported for Amazon ECS".
const PREDICTIVE_SCALING_NAMESPACE: &str = "ecs";

/// RULE: an operation this service does not offer for the namespace addressed is refused
/// outright — `AccessDeniedException` with the fixed sentence naming the operation, at the
/// client-error status — ahead of every lookup, so the answer does not depend on what is
/// registered.
///
/// Rows are every modelled `ServiceNamespace` (the catalog comes from the model, rendered as
/// `SERVICE_NAMESPACE_VALUES`), each asked with nothing registered and then with a scalable
/// target of its own registered, so the refusal is shown not to be a report about state. The
/// one namespace that DOES offer predictive scaling stands beside them: it gets no such
/// refusal, but the answer of the forecast itself.
#[tokio::test]
async fn an_operation_a_namespace_does_not_offer_is_refused_outright() {
    /// The sentence the refusal states, with the operation as its only variable part.
    const NOT_SUPPORTED: &str = "GetPredictiveScalingForecast is not supported.";

    let emulator = Emulator::empty();
    assert!(
        SERVICE_NAMESPACE_VALUES.len() > 10,
        "case \"the model's namespace catalog\": expected every modelled ServiceNamespace, got \
         {SERVICE_NAMESPACE_VALUES:?}"
    );

    for namespace in SERVICE_NAMESPACE_VALUES {
        // A dimension of this namespace, and the resource id its resource type documents, so
        // the request is a well-formed one for that namespace.
        let dimension = dimension_of(namespace);
        let request = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id_for(dimension),
            "ScalableDimension": dimension,
            "PolicyName": "forecast-me",
            "StartTime": 1_767_225_600.0,
            "EndTime": 1_767_312_000.0,
        });

        // Asked twice: before anything is registered, and again with this namespace's own
        // target registered — the refusal is the same, because it is about the operation
        // being offered and not about state.
        for registered in [false, true] {
            if registered {
                emulator.given_target(dimension, 1, 10).await;
            }
            let case = format!(
                "{namespace} with {}",
                if registered {
                    "a registered target"
                } else {
                    "nothing registered"
                }
            );
            let (status, body) = emulator.call("GetPredictiveScalingForecast", request.clone()).await;
            let code = error_code(&body);
            if *namespace == PREDICTIVE_SCALING_NAMESPACE {
                assert_ne!(
                    code, "AccessDeniedException",
                    "case {case:?}: expected the namespace that offers predictive scaling not to \
                     be refused as unsupported, got {body:?}"
                );
                continue;
            }
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} {body:?}"
            );
            assert_eq!(
                code, "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {code:?} in {body:?}"
            );
            // The sentence, under the key this service's error bodies carry it under: every
            // error shape of this model names its message member `Message`, and a client
            // reading the body looks for it there.
            assert_eq!(
                body["Message"],
                json!(NOT_SUPPORTED),
                "case {case:?}: expected the message {NOT_SUPPORTED:?} under Message, got {body:?}"
            );
        }
    }
}

/// A `ScalableDimension` of `namespace`: the first one the model's own dimension catalog
/// names it in, so no row carries a hand-written pair.
fn dimension_of(namespace: &str) -> &'static str {
    use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::ops::types::SCALABLE_DIMENSION_VALUES;

    SCALABLE_DIMENSION_VALUES
        .iter()
        .find(|dimension| dimension.split(':').next() == Some(namespace))
        .unwrap_or_else(|| {
            panic!("case {namespace:?}: expected the model to name a dimension of this namespace")
        })
}
