// SPDX-License-Identifier: CC0-1.0

//! What a failed request states about a call the OWNING service refused, for a resource type
//! whose handler carries its CLIENT's rendering of that refusal.
//!
//! The rule every case here holds the service to: **an error the owning service raised that a
//! resource type's handler maps to no verdict of its own is reported the way THAT type's
//! handler reports one. A handler that carries its client's rendering states the exception
//! that client raised, so the terminal `ProgressEvent`'s `StatusMessage` is the owning
//! service's OWN diagnostic of the refused call — the very sentence that service's own API
//! answers the same call with — followed by that client's rendering of the call: the service
//! it called, the status it answered, the id of the call and the attempts made at it. A
//! handler that states a sentence of its own states the CALL it was making instead and relays
//! nothing that service said. Either way the `ErrorCode` is the catch-all
//! `GeneralServiceException` and the refused work leaves no resource behind, in either
//! store.**
//!
//! The rows differ along what the rule generalises over: which rule of the owning service
//! refused the call (a FIFO name without its suffix, that suffix on a queue that is not FIFO,
//! a name outside the charset, a name past the length that service admits, an attribute no
//! queue of that kind has, a dead-letter target that is not there), and which resource
//! operation the handler was carrying out — a create and an update call different APIs of
//! that service. No case writes the expected sentence down: each reads it from the answer the
//! owning service's OWN API gives the same call, so what is pinned is that the diagnosis
//! travelled through rather than any wording this suite chose.
//!
//! `downstream_refusal_reporting` holds the other half of the rule — the sentence a handler
//! that relays nothing states — and `refusal_wording` holds the same client rendering on a
//! relayed not-found.

use serde_json::{json, Value};

use super::emulator::{code, message, progress, text, Emulator, STATUS_FAILED};
use super::refusal_wording::{relayed_call, RELAYED_SERVICE};
use super::sqs_queue_read_through::{queue_url, QUEUE};

/// The handler code for an error the OWNING service raised that the type's handler maps to no
/// verdict of its own.
const DOWNSTREAM: &str = "GeneralServiceException";

/// The name the Amazon SQS client the `AWS::SQS::Queue` handler reaches that service through
/// carries, which its rendering of an exception names the service by.
const SQS_CLIENT: &str = "Sqs";

/// A type whose handler states a sentence of its OWN about the call it was making rather than
/// relaying the exception its client raised.
const PARAMETER: &str = "AWS::SSM::Parameter";

/// The sentence such a handler states, naming the owning service's API operation it was
/// calling.
fn call_sentence(operation: &str) -> String {
    format!("Error occurred during operation '{operation}'.")
}

/// Hold one terminal event to the relayed diagnostic the rule states, against the answer the
/// OWNING service's own API gave the same call (`downstream`, that API's `(status, body)`
/// pair), and answer the id of the downstream call the diagnostic named.
fn assert_relayed(case: &str, event: &Value, downstream: &(u16, Value)) -> String {
    assert_eq!(
        text(event, "OperationStatus"),
        STATUS_FAILED,
        "case {case:?}: a call the owning service refuses must fail the request, got {event:?}"
    );
    assert_eq!(
        text(event, "ErrorCode"),
        DOWNSTREAM,
        "case {case:?}: expected the {DOWNSTREAM} handler error code, got {event:?}"
    );
    let (downstream_status, refused) = downstream;
    let sentence = message(refused);
    assert!(
        !sentence.is_empty(),
        "case {case:?}: the owning service must state a diagnostic of its own, got {refused:?}"
    );
    let reported = text(event, "StatusMessage");
    let call = relayed_call(&reported, case);
    assert_eq!(
        call.sentence, sentence,
        "case {case:?}: expected the owning service's own diagnosis {sentence:?}, got {:?}",
        call.sentence
    );
    assert_eq!(
        call.service, SQS_CLIENT,
        "case {case:?}: expected the diagnostic to name {SQS_CLIENT:?}, got {:?}",
        call.service
    );
    assert_eq!(
        call.status, *downstream_status,
        "case {case:?}: expected the status the owning service answered \
         ({downstream_status}), got {}",
        call.status
    );
    assert!(
        call.attempts > 0,
        "case {case:?}: expected the attempts the client made at the call, got {}",
        call.attempts
    );
    call.request_id
}

/// Each relayed diagnostic reports the call ITS OWN handler invocation made, so no two of
/// them name the same downstream request.
fn assert_each_call_named_once(ids: &[String]) {
    let mut named: Vec<&String> = ids.iter().collect();
    named.sort();
    named.dedup();
    assert_eq!(
        named.len(),
        ids.len(),
        "each relayed diagnostic must name its own downstream call, got {ids:?}"
    );
}

#[tokio::test]
async fn a_refused_create_relays_the_owning_services_own_diagnosis() {
    let emulator = Emulator::with_sqs();

    // (case, the desired state Cloud Control API takes, the SAME call on the owning service's
    // own API — the answer to which is the diagnosis the handler has to relay).
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a FIFO queue whose name lacks the suffix",
            json!({ "QueueName": "relayed-fifo", "FifoQueue": true }),
            json!({ "QueueName": "relayed-fifo", "Attributes": { "FifoQueue": "true" } }),
        ),
        (
            "that suffix on a queue that is not FIFO",
            json!({ "QueueName": "relayed-standard.fifo", "FifoQueue": false }),
            json!({ "QueueName": "relayed-standard.fifo" }),
        ),
        (
            "a name carrying a character the charset excludes",
            json!({ "QueueName": "relayed queue!" }),
            json!({ "QueueName": "relayed queue!" }),
        ),
        (
            "a FIFO name that is the suffix and nothing else",
            json!({ "QueueName": ".fifo", "FifoQueue": true }),
            json!({ "QueueName": ".fifo", "Attributes": { "FifoQueue": "true" } }),
        ),
    ];

    // Started together, then waited for: the creates are independent, so one span covers them
    // all.
    let mut accepted = Vec::new();
    for (case, desired, _) in &rows {
        let (status, body) = emulator.create(QUEUE, desired, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a schema-valid model must be accepted, got {status} with body \
             {body:?}"
        );
        accepted.push(text(&progress(&body), "RequestToken"));
    }

    let mut named_calls = Vec::new();
    for ((case, desired, native), request_token) in rows.iter().zip(&accepted) {
        let settled = emulator.terminal(request_token).await;
        // What the OWNING service answers the same call, asked of its own API: the refusal
        // the handler relays, and the status the rendering of the call reports.
        let downstream = emulator.sqs("CreateQueue", native.clone()).await;
        named_calls.push(assert_relayed(case, &settled, &downstream));

        // Nothing was created: a refused create leaves no resource behind.
        let (status, body) = emulator
            .get(QUEUE, &queue_url(&text(desired, "QueueName")))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must leave no resource, got {status} with body \
             {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
    }
    assert_each_call_named_once(&named_calls);
}

#[tokio::test]
async fn a_relayed_create_refusal_leaves_the_owning_service_holding_no_queue() {
    let emulator = Emulator::with_sqs();
    let name = "relayed-rolled-back";

    // The positive control first: the same model with a name the owning service admits is
    // created, and that service resolves it — so what the refused create below leaves behind
    // is the difference between the two, rather than a type that cannot be created at all.
    let admitted = format!("{name}.fifo");
    emulator
        .provision(QUEUE, &json!({ "QueueName": &admitted, "FifoQueue": true }))
        .await;
    let resolved = emulator
        .sqs_ok("GetQueueUrl", json!({ "QueueName": &admitted }))
        .await;
    assert_eq!(
        text(&resolved, "QueueUrl"),
        queue_url(&admitted),
        "the owning service must resolve the queue the create made, got {resolved:?}"
    );

    let (status, body) = emulator
        .create(QUEUE, &json!({ "QueueName": name, "FifoQueue": true }), None)
        .await;
    assert_eq!(
        status, 200,
        "a schema-valid model must be accepted, got {status} with body {body:?}"
    );
    let settled = emulator
        .terminal(&text(&progress(&body), "RequestToken"))
        .await;
    let downstream = emulator
        .sqs(
            "CreateQueue",
            json!({ "QueueName": name, "Attributes": { "FifoQueue": "true" } }),
        )
        .await;
    assert_relayed("a FIFO queue whose name lacks the suffix", &settled, &downstream);

    // Neither API holds a queue of that name afterwards: Cloud Control API refuses the
    // identifier the create would have assigned, and the owning service resolves no such
    // queue in its own terms.
    let (status, refused) = emulator.get(QUEUE, &queue_url(name)).await;
    assert_eq!(
        status, 400,
        "a refused create must leave no resource, got {status} with body {refused:?}"
    );
    assert!(
        code(&refused).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {:?}",
        code(&refused)
    );
    let (status, absent) = emulator
        .sqs("GetQueueUrl", json!({ "QueueName": name }))
        .await;
    assert_eq!(
        status, 400,
        "the owning service must resolve no such queue, got {status} with body {absent:?}"
    );
    assert!(
        code(&absent).contains("QueueDoesNotExist"),
        "expected the queue-does-not-exist refusal, got {:?}",
        code(&absent)
    );
    assert_eq!(
        message(&absent),
        "The specified queue does not exist.",
        "unexpected wording from the owning service, got {absent:?}"
    );
}

#[tokio::test]
async fn a_refused_patch_relays_the_owning_services_own_diagnosis() {
    let emulator = Emulator::with_sqs();
    let identifier = emulator
        .provision(QUEUE, &json!({ "QueueName": "relayed-patched" }))
        .await;
    let before = emulator.properties(QUEUE, &identifier).await;

    // The same rule on the UPDATE path, whose handler calls the owning service's attribute
    // write. The rows differ along which rule of that service refuses the write: an attribute
    // no queue of this kind has, and a dead-letter target that service holds no queue for.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "an attribute that applies to no queue of this kind",
            json!([{ "op": "add", "path": "/ContentBasedDeduplication", "value": true }]),
            json!({ "ContentBasedDeduplication": "true" }),
        ),
        (
            "a dead-letter target the owning service holds no queue for",
            json!([{
                "op": "add",
                "path": "/RedrivePolicy",
                "value": {
                    "deadLetterTargetArn": "arn:aws:sqs:us-east-1:000000000000:relayed-absent",
                    "maxReceiveCount": 5,
                },
            }]),
            json!({
                "RedrivePolicy": "{\"deadLetterTargetArn\":\
                                   \"arn:aws:sqs:us-east-1:000000000000:relayed-absent\",\
                                   \"maxReceiveCount\":5}",
            }),
        ),
    ];

    let mut named_calls = Vec::new();
    for (case, patch, attributes) in &rows {
        let (status, body) = emulator.update(QUEUE, &identifier, patch, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the patch must be accepted, got {status} with body {body:?}"
        );
        let settled = emulator
            .terminal(&text(&progress(&body), "RequestToken"))
            .await;
        let downstream = emulator
            .sqs(
                "SetQueueAttributes",
                json!({ "QueueUrl": &identifier, "Attributes": attributes }),
            )
            .await;
        named_calls.push(assert_relayed(case, &settled, &downstream));

        let after = emulator.properties(QUEUE, &identifier).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused patch must leave the queue as it was, got {after:?}"
        );
    }
    assert_each_call_named_once(&named_calls);
}

#[tokio::test]
async fn a_handler_that_states_its_own_sentence_relays_nothing() {
    let emulator = Emulator::with_ssm();
    // The control for the rule above: this type's handler states its own sentence about the
    // call it was making, so the same class of failure carries no diagnosis of the owning
    // service's and none of the groups a relayed exception is rendered with — the rows
    // differing along which rule of that service refused the write. Every property of this
    // type is a member of that write under the same name, so each row's model is the call
    // its handler makes.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a name in the hierarchy the owning service keeps for itself",
            json!({ "Name": "/aws/relayed", "Type": "String", "Value": "v" }),
        ),
        (
            "a value the expression the write itself states does not admit",
            json!({
                "Name": "/relayed/pattern",
                "Type": "String",
                "Value": "not-a-number",
                "AllowedPattern": "^[0-9]+$",
            }),
        ),
    ];
    let mut accepted = Vec::new();
    for (case, desired) in &rows {
        let (status, body) = emulator.create(PARAMETER, desired, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a schema-valid model must be accepted, got {status} with body \
             {body:?}"
        );
        accepted.push(text(&progress(&body), "RequestToken"));
    }
    for ((case, desired), request_token) in rows.iter().zip(&accepted) {
        let settled = emulator.terminal(request_token).await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: a call the owning service refuses must fail the request, got \
             {settled:?}"
        );
        assert_eq!(
            text(&settled, "ErrorCode"),
            DOWNSTREAM,
            "case {case:?}: expected the {DOWNSTREAM} handler error code, got {settled:?}"
        );
        let reported = text(&settled, "StatusMessage");
        assert_eq!(
            reported,
            call_sentence("PutParameter"),
            "case {case:?}: expected the sentence naming the call, got {reported:?}"
        );
        assert!(
            !reported.contains(RELAYED_SERVICE),
            "case {case:?}: a handler that relays nothing must render no call, got {reported:?}"
        );
        // And the owning service's own API, asked the same thing, does state a diagnosis of
        // its own — so what the control shows is that this handler drops it rather than that
        // there was none to carry.
        let (status, refused) = emulator.ssm("PutParameter", desired.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the owning service must refuse the same call, got {status} with \
             body {refused:?}"
        );
        assert!(
            !message(&refused).is_empty() && !reported.contains(&message(&refused)),
            "case {case:?}: the sentence must carry none of the owning service's wording \
             ({:?}), got {reported:?}",
            message(&refused)
        );
    }
}
