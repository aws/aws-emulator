// SPDX-License-Identifier: CC0-1.0

//! What Lambda does with an asynchronous event once its delivery to the function is OVER:
//! the invocation RECORD it hands to the configured destination, the event it writes to the
//! configured dead-letter queue, and what each of those deliveries meters.
//!
//! The queue and the attempts themselves are [`super::async_invocation`]'s; this module is
//! the step after the LAST attempt, and it is the only thing a client of an asynchronous
//! invocation can observe about it at all — the accepting call was answered before the run,
//! so a caller that configured a destination or a dead-letter queue learns what became of
//! its event there and nowhere else.
//!
//! Two resources say where, and they are independent of one another, so an event is
//! delivered to each one that is configured:
//!
//! * `PutFunctionEventInvokeConfig`'s `DestinationConfig` — "a destination for events after
//!   they have been sent to a function for processing", whose `OnSuccess` is "the destination
//!   configuration for successful invocations" and whose `OnFailure` is "the destination
//!   configuration for failed invocations". What is delivered there is an INVOCATION RECORD
//!   ([`invocation_record`]): the context of the request, the event itself, and the context
//!   and payload of the response — so a failure-handling pipeline reads what was invoked,
//!   with what, how many attempts it had and what the function answered.
//! * `FunctionConfiguration`'s `DeadLetterConfig$TargetArn`, "The Amazon Resource Name (ARN)
//!   of an Amazon SQS queue or Amazon SNS topic". What is written there is the EVENT
//!   PAYLOAD, exactly as the caller supplied it, with the request id and the error the
//!   attempt reported recorded beside it as message attributes — so an operator can replay
//!   the dropped event.
//!
//! WHEN each is used follows from the outcome of the last attempt, and nothing else: a
//! handler that returned is a success, and a handler that errored on the last attempt the
//! event had — or an event whose function could not be run at all — is a failure. A dead
//! letter is only ever a failure's, because the queue exists for the events Lambda gave up
//! on.
//!
//! ## What a delivery meters
//!
//! Each kind of failure has its own series of Lambda's own metrics
//! ([`super::metrics`]), and each is published only when the delivery it is about FAILED:
//!
//! * `DestinationDeliveryFailures` for an invocation record the destination did not accept;
//! * `DeadLetterErrors` for an event the dead-letter target did not accept.
//!
//! So a destination that took the record, and a queue that took the event, meter nothing at
//! all — which is what makes a datapoint on either series mean a delivery really was lost.
//! Beside them, two series are about the EVENT rather than about a delivery of it:
//! `AsyncEventsDropped` for an event the function did not process, published here, and
//! `AsyncEventAge` — the span from the instant the event was accepted to the instant the
//! function was invoked with it — published per delivery ATTEMPT where the attempt is made
//! ([`super::async_invocation::deliver`]), so a retried event meters the longer wait its
//! retry really had.
//!
//! ## How a delivery is made
//!
//! Through the SAME seam the handler's own SDK calls go through
//! ([`super::execution::AwsAccess`]), which dispatches into this emulator over the in-process
//! callback and signs as the session of the function's own EXECUTION ROLE — which is the role
//! Lambda delivers a record as, so a destination the role may not write is a destination the
//! record does not reach, and the failure is metered.
//!
//! A destination is a caller-chosen value, and nothing of it is DIALLED: it is an ARN, and
//! the only things read out of it are the service it belongs to and the Region it names, which
//! decide which service OF THIS EMULATOR is asked and in which Region. No host name is built
//! from it, no name is resolved and no socket is opened, so a destination this emulator does
//! not serve itself is a delivery that was not made rather than a connection to an address a
//! caller chose.
//!
//! The destinations reached are the ones that seam binds a request for: an Amazon SQS queue,
//! an Amazon SNS topic, an Amazon EventBridge event bus and a Lambda function — which is
//! every kind `DeadLetterConfig$TargetArn` ("an Amazon SQS queue or Amazon SNS topic") names,
//! and every kind `DestinationConfig` names but the Amazon S3 bucket. A destination of any
//! other service is reported as a delivery that could not be made, naming the destination,
//! rather than counted as delivered — and metered as nothing, because no delivery was
//! attempted for it to have failed ([`Undeliverable`]).
//!
//! ## What bounds a destination that is itself a function
//!
//! A Lambda-function destination is another asynchronous invocation, which completes and
//! delivers to ITS destination in turn, so the chain is bounded here rather than left to grow:
//!
//! * a destination naming the FUNCTION this completion is of is withheld ([`deliver`]), which
//!   is the one configuration whose chain the completion path itself sustains;
//! * every hop beyond that carries this invocation's own tracing header
//!   ([`super::execution::AwsAccess`]), so it is a HOP OF THE SAME REQUEST CHAIN and Lambda's
//!   recursive-loop guardrail counts it: past
//!   [`super::recursion::MAX_REQUEST_CHAIN_INVOCATIONS`] the chain is stopped for a function
//!   whose configuration is `Terminate`, which is the value a function carries before anyone
//!   sets one;
//! * the delivery runs on the attempt's own task, under the permit that bounds how many
//!   asynchronous deliveries this emulator makes at once
//!   ([`super::MAX_IN_FLIGHT_DELIVERIES`]), and each function keeps at most
//!   [`super::MAX_QUEUED_ASYNC_INVOCATIONS`] queued events, so neither the tasks nor the
//!   store grow with the length of a chain.

use std::time::{Duration, SystemTime};

use super::*;

/// How long one delivery of a record or a dead letter is given.
///
/// A bound on an in-process hop made off the response path, exactly as the delivery of a
/// run's own log is bounded ([`super::execution::log_delivery`]): the caller was answered
/// before the run, so a destination that does not answer cannot hold this task open.
const DELIVERY_CEILING: Duration = Duration::from_secs(5);

/// The `version` member of the invocation record, as AWS documents the schema of it.
const RECORD_SCHEMA_VERSION: &str = "1.0";

/// The `requestContext.condition` of a record for an event whose handler RETURNED:
/// "Success", the condition the documented success record carries.
const CONDITION_SUCCESS: &str = "Success";

/// The `requestContext.condition` of a record for an event Lambda gave up on: the event had
/// every delivery attempt it was going to get and the last one did not succeed.
const CONDITION_RETRIES_EXHAUSTED: &str = "RetriesExhausted";

/// The `requestContext.condition` of a record for an event whose RETENTION window was spent
/// before the function could be run for it: "The maximum age of a request that Lambda sends
/// to a function for processing" (`MaximumEventAgeInSeconds`) is the window, and an event
/// held for the whole of it is one Lambda stops trying to run
/// ([`super::async_delivery::next_throttled_attempt_in`]).
const CONDITION_EVENT_AGE_EXCEEDED: &str = "EventAgeExceeded";

/// The status an invocation of a function that RAN answers with: "A success response (202
/// Accepted) indicates that the request is queued for invocation" is the accepting call's,
/// and the run itself is reported on the `200` an invocation that executed carries — which
/// is the status the documented record's `responseContext` holds whether the handler
/// returned or raised, because the failure is reported in `functionError` beside it.
const INVOKED_STATUS: i64 = 200;

/// The message attribute a dead letter records the invocation's own request id in, whose
/// value is text.
const REQUEST_ID_ATTRIBUTE: &str = "RequestID";

/// The message attribute a dead letter records the status of the invocation that failed in,
/// whose value is a NUMBER: it is the status an invocation that executed answers
/// ([`INVOKED_STATUS`]) rather than a word, so it carries the numeric data type.
const ERROR_CODE_ATTRIBUTE: &str = "ErrorCode";

/// The message attribute a dead letter records the failure's own message in, whose value is
/// text.
const ERROR_MESSAGE_ATTRIBUTE: &str = "ErrorMessage";

/// How much of that message a dead letter carries: AWS records the error message of a
/// dead-lettered event truncated to the first kilobyte of it, so a handler that raised with a
/// long message does not put an unbounded value on the queue's message attributes.
const MAX_ERROR_MESSAGE_BYTES: usize = 1_024;

/// The `DataType` a textual message attribute carries (`MessageAttributeValue$DataType`).
const STRING_ATTRIBUTE_TYPE: &str = "String";

/// The `DataType` a numeric one carries, the other of the two that attribute's documented
/// types name for a value that is not bytes: "Number" (`MessageAttributeValue$DataType`).
const NUMBER_ATTRIBUTE_TYPE: &str = "Number";

/// One message attribute a dead letter records beside the event: its name, the `DataType`
/// its value IS, and the value.
///
/// The type travels WITH the value because it is a property of that value and not of the
/// delivery: the status of the failed invocation is a number and the request id and the
/// message are text, so each is recorded as what it is rather than as one type chosen for
/// all of them.
struct Attribute {
    /// The name the attribute is recorded under.
    name: &'static str,
    /// The `DataType` of its value.
    data_type: &'static str,
    /// The value itself.
    value: String,
}

/// What the LAST delivery attempt of one accepted asynchronous event came to, and everything
/// the deliveries that follow it are built from.
pub struct Completion<'run> {
    /// The invocation the attempt was made for, which carries the event, the request id, the
    /// version that ran and the SDK seam a delivery is made through.
    pub invocation: &'run Invocation,
    /// The qualifier the event was ADDRESSED to — `$LATEST` or an alias — which is the
    /// resource an asynchronous-invocation configuration is held against.
    pub qualifier: &'run str,
    /// How many delivery attempts RAN the handler.
    pub attempts: u32,
    /// What the last of them came to: the run, or why the function could not be run.
    pub run: &'run Result<super::execution::Invoked, super::execution::Unrunnable>,
    /// WHAT ended the event, which is the condition the record reports ([`Ending`]).
    pub ending: Ending,
}

/// What ended an accepted asynchronous event, as `requestContext.condition` words it.
///
/// The two endings this emulator can reach, and the two AWS words for an event it gave up
/// on: the attempts the configuration allowed are spent, or the event was held for as long as
/// it is retained without a run ever being made for it
/// ([`super::async_delivery::event_retention`]). A handler that RETURNED ends neither way and
/// is the success condition instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The event had every delivery attempt it was allowed.
    AttemptsSpent,
    /// Its retention window was spent before the function could be run for it.
    AgeExceeded,
}

impl Completion<'_> {
    /// Whether the event FAILED: a handler that errored on this attempt, or a function that
    /// could not be run for the event at all. A handler that returned did not fail.
    fn failed(&self) -> bool {
        match self.run {
            Ok(invoked) => invoked.function_error.is_some(),
            Err(_why) => true,
        }
    }

    /// The `requestContext.condition` of this completion's record.
    fn condition(&self) -> &'static str {
        if !self.failed() {
            return CONDITION_SUCCESS;
        }
        match self.ending {
            Ending::AttemptsSpent => CONDITION_RETRIES_EXHAUSTED,
            Ending::AgeExceeded => CONDITION_EVENT_AGE_EXCEEDED,
        }
    }

    /// Whether `destination` names the FUNCTION this completion is of, under any qualifier.
    ///
    /// Such a destination is the one configuration whose delivery would start the very chain
    /// it ends — a function that is its own destination queues an event for itself for every
    /// event it completes — so it is reported as a delivery that was not made (see
    /// [`deliver`]).
    fn names_its_own_function(&self, destination: &str) -> bool {
        match (
            addressed_function(destination),
            addressed_function(&self.invocation.invoked_function_arn),
        ) {
            (Some(named), Some(mine)) => named == mine,
            _ => false,
        }
    }
}

/// The FUNCTION a Lambda ARN names, without the qualifier beside it: the `function:<name>`
/// head of its resource field, or `None` for an ARN of another service.
///
/// Which function an ARN names is what decides whether two ARNs are the same function, and a
/// qualifier is a version OF that function rather than another one.
fn addressed_function(arn: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == "lambda").then(|| {
        parsed
            .resource
            .split(':')
            .take(2)
            .collect::<Vec<_>>()
            .join(":")
    })
}

/// Make the deliveries the completion of one asynchronous event owes, and meter them.
///
/// Reads the two resources that decide them out of the store under ONE guard — the
/// destination the qualifier's asynchronous-invocation configuration names, and the
/// dead-letter target the resolved version's configuration names — and then releases it:
/// every delivery below is an in-process hop with no guard held.
pub async fn deliver_completion(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    completion: &Completion<'_>,
) {
    let function = &completion.invocation.function_name;
    let failed = completion.failed();
    let (destination, dead_letter) = {
        let store = state.lock();
        let Some(record) = store.functions.get(function) else {
            return;
        };
        let configured = record
            .event_invoke_configs
            .get(completion.qualifier)
            .and_then(|config| {
                if failed {
                    config.on_failure.clone()
                } else {
                    config.on_success.clone()
                }
            });
        // The dead-letter target is the RESOLVED version's own configuration's, because a
        // published version froze the configuration it was published with.
        let dead_letter = failed
            .then(|| {
                record
                    .config_of(&completion.invocation.executed_version)
                    .unwrap_or(&record.config)
                    .dead_letter_config
                    .as_ref()
                    .and_then(|config| config.target_arn.clone())
            })
            .flatten();
        (configured, dead_letter)
    };
    // An event the function did not process is one Lambda DROPPED, which is metered whatever
    // became of the deliveries below (`super::metrics`). The AGE of the event is metered per
    // delivery ATTEMPT instead, where the attempt is made
    // (`super::async_invocation::deliver`).
    if failed {
        if let Some(metrics) = completion.invocation.metrics.as_ref() {
            metrics.publish_async_event_dropped();
        }
    }
    if let Some(destination) = destination.as_deref() {
        let delivered = deliver(
            completion,
            destination,
            &invocation_record(completion).to_string(),
        )
        .await;
        report(completion, destination, "invocation record", &delivered);
        // A delivery LAMBDA WOULD HAVE MADE and this emulator did not is one
        // `DestinationDeliveryFailures` datapoint. A destination of a service this seam binds
        // no request for is not that: no delivery was attempted, so metering one would put a
        // failure on the series for work the service never did (see [`Undeliverable`]).
        if matches!(delivered, Err(Undeliverable::Refused(_))) {
            if let Some(metrics) = completion.invocation.metrics.as_ref() {
                metrics.publish_destination_delivery_failure();
            }
        }
    }
    if let Some(target) = dead_letter.as_deref() {
        let delivered = deliver_dead_letter(completion, target).await;
        report(completion, target, "dead letter", &delivered);
        if matches!(delivered, Err(Undeliverable::Refused(_))) {
            if let Some(metrics) = completion.invocation.metrics.as_ref() {
                metrics.publish_dead_letter_error();
            }
        }
    }
}

/// Why one delivery was not made.
///
/// The two are metered apart, because only one of them is a delivery that FAILED: Lambda
/// counts `DestinationDeliveryFailures` and `DeadLetterErrors` for a write it attempted and
/// did not get accepted, so a destination of a service this seam binds no request for — which
/// is a gap in this emulator and not a failure of the destination — is logged and counted
/// nowhere.
#[derive(Debug)]
enum Undeliverable {
    /// The delivery was attempted or deliberately withheld, and did not land; the string is
    /// why.
    Refused(NotDelivered),
    /// This emulator binds no request to the destination's service at all, so nothing was
    /// attempted; the string is why.
    Unsupported(NotDelivered),
}

impl Undeliverable {
    /// The sentence this outcome is logged with.
    fn detail(&self) -> &str {
        match self {
            Self::Refused(detail) | Self::Unsupported(detail) => detail,
        }
    }
}

/// Why one delivery was not made, as the sentence logged for it.
type NotDelivered = String;

/// Deliver `body` to the destination `destination` names, through the seam the invocation
/// carries, and report whether the destination ACCEPTED it.
///
/// Which call is made follows from the SERVICE the destination's ARN names, because a
/// destination is a resource Lambda writes to with that service's own write: a message to a
/// queue, an event to a bus, an asynchronous invocation of a function.
///
/// One destination is WITHHELD: a Lambda function that is the function this completion is of.
/// Delivering to it would queue another event for the same function for every event it
/// completes, which is the one configuration whose chain the completion path itself sustains,
/// so it is refused here rather than entered — the chain of destination invocations to any
/// OTHER function is bounded as the module header states.
async fn deliver(
    completion: &Completion<'_>,
    destination: &str,
    body: &str,
) -> Result<(), Undeliverable> {
    let Some(vendor) = ::service_common::arn::vendor(destination) else {
        return Err(Undeliverable::Refused(format!(
            "{destination} is no resource of any service"
        )));
    };
    if vendor == "lambda" && completion.names_its_own_function(destination) {
        return Err(Undeliverable::Refused(format!(
            "{destination} is the function this invocation ran, and a function that is its own \
             destination would queue an event for itself for every event it completes"
        )));
    }
    match vendor {
        "sqs" => send_to_queue(completion, destination, body, &[]).await,
        "sns" => publish_to_topic(completion, destination, body, &[]).await,
        "events" => put_to_bus(completion, destination, body).await,
        "lambda" => invoke_function(completion, destination, body).await,
        other => Err(Undeliverable::Unsupported(format!(
            "this emulator binds no request to {other} for a destination, so the record was \
             not delivered to {destination}"
        ))),
    }
}

/// Write the event itself to the dead-letter target `target` names.
///
/// The body is the event payload EXACTLY as the caller supplied it, which is what makes the
/// dropped event replayable, with the invocation's own request id and the failure the last
/// attempt reported recorded beside it as message attributes.
async fn deliver_dead_letter(
    completion: &Completion<'_>,
    target: &str,
) -> Result<(), Undeliverable> {
    let body = String::from_utf8_lossy(&completion.invocation.payload).into_owned();
    let mut attributes: Vec<Attribute> = vec![Attribute {
        name: REQUEST_ID_ATTRIBUTE,
        data_type: STRING_ATTRIBUTE_TYPE,
        value: completion.invocation.request_id.clone(),
    }];
    if let Ok(invoked) = completion.run {
        if let Some(function_error) = invoked.function_error.as_deref() {
            attributes.push(Attribute {
                name: ERROR_CODE_ATTRIBUTE,
                data_type: NUMBER_ATTRIBUTE_TYPE,
                value: INVOKED_STATUS.to_string(),
            });
            attributes.push(Attribute {
                name: ERROR_MESSAGE_ATTRIBUTE,
                data_type: STRING_ATTRIBUTE_TYPE,
                value: first_kilobyte(
                    &failure_message(invoked).unwrap_or_else(|| function_error.to_string()),
                ),
            });
        }
    }
    let Some(vendor) = ::service_common::arn::vendor(target) else {
        return Err(Undeliverable::Refused(format!(
            "{target} is no resource of any service"
        )));
    };
    match vendor {
        "sqs" => send_to_queue(completion, target, &body, &attributes).await,
        "sns" => publish_to_topic(completion, target, &body, &attributes).await,
        other => Err(Undeliverable::Unsupported(format!(
            "this emulator binds no request to {other} for a dead-letter target, so the event \
             was not written to {target}"
        ))),
    }
}

/// `Publish` the body to the Amazon SNS topic the ARN `topic` names, with `attributes` as the
/// message's own attributes.
///
/// Through the same seam as every other delivery here: Amazon SNS's own protocol is the
/// legacy `awsQuery`, which that seam binds a request for with that service's own client
/// ([`super::execution::aws_api`]), so the publish is signed as the function's execution role
/// like the rest and the topic reached is one of this emulator's own.
///
/// The attributes travel as the published message's `MessageAttributes`, which is the only
/// place a topic can carry them: "Message attributes for Publish action" (`Publish`), and a
/// subscriber receives them as its own endpoint's attributes. A dead letter's request id and
/// error are recorded this way whether the target is a queue or a topic, so the same
/// bookkeeping reaches the operator whichever kind of target the configuration names.
async fn publish_to_topic(
    completion: &Completion<'_>,
    topic: &str,
    body: &str,
    attributes: &[Attribute],
) -> Result<(), Undeliverable> {
    let parsed = ::service_common::arn::parse(topic).ok_or_else(|| {
        Undeliverable::Refused(format!("{topic} is no topic ARN"))
    })?;
    let mut input = ::serde_json::json!({ "TopicArn": topic, "Message": body });
    if !attributes.is_empty() {
        input["MessageAttributes"] = ::serde_json::Value::Object(stated_attributes(attributes));
    }
    called(completion, "sns", "Publish", parsed.region, &input.to_string()).await
}

/// `attributes` as the `MessageAttributes` map both Amazon SQS's `SendMessage` and Amazon
/// SNS's `Publish` take: each name bound to its value's `DataType` and `StringValue`.
///
/// One rendering for both, because the member is the same shape on either call
/// (`MessageAttributeValue`): the `DataType` of each attribute is the one that attribute
/// carries ([`Attribute`]), so a value that is a number is recorded as a number whichever
/// kind of target the configuration names.
fn stated_attributes(attributes: &[Attribute]) -> ::serde_json::Map<String, ::serde_json::Value> {
    attributes
        .iter()
        .map(|attribute| {
            (
                attribute.name.to_string(),
                ::serde_json::json!({
                    "DataType": attribute.data_type,
                    "StringValue": attribute.value,
                }),
            )
        })
        .collect()
}

/// The first kilobyte of `message`, cut on a character boundary so the value stays the text
/// it is; see [`MAX_ERROR_MESSAGE_BYTES`].
fn first_kilobyte(message: &str) -> String {
    if message.len() <= MAX_ERROR_MESSAGE_BYTES {
        return message.to_string();
    }
    let mut end = MAX_ERROR_MESSAGE_BYTES;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// `SendMessage` the body to the queue the ARN `queue` names.
///
/// A queue is addressed by its `QueueUrl`, which is the url the Amazon SQS emulator of this
/// same engine publishes for the queue the ARN names
/// ([`::service_com_amazonaws_sqs_amazonsqs::state::queue_url`]) — the ONE spelling of it, so
/// the address written to is the address a client of that queue holds.
async fn send_to_queue(
    completion: &Completion<'_>,
    queue: &str,
    body: &str,
    attributes: &[Attribute],
) -> Result<(), Undeliverable> {
    let parsed = ::service_common::arn::parse(queue).ok_or_else(|| {
        Undeliverable::Refused(format!("{queue} is no queue ARN"))
    })?;
    let url = ::service_com_amazonaws_sqs_amazonsqs::state::queue_url(
        parsed.region,
        parsed.resource,
    );
    let mut input = ::serde_json::json!({ "QueueUrl": url, "MessageBody": body });
    if !attributes.is_empty() {
        input["MessageAttributes"] = ::serde_json::Value::Object(stated_attributes(attributes));
    }
    called(completion, "sqs", "SendMessage", parsed.region, &input.to_string()).await
}

/// `PutEvents` the record to the event bus the ARN `bus` names.
async fn put_to_bus(
    completion: &Completion<'_>,
    bus: &str,
    body: &str,
) -> Result<(), Undeliverable> {
    let parsed = ::service_common::arn::parse(bus).ok_or_else(|| {
        Undeliverable::Refused(format!("{bus} is no event bus ARN"))
    })?;
    let input = ::serde_json::json!({
        "Entries": [{
            "EventBusName": bus,
            "Source": EVENT_RECORD_SOURCE,
            "DetailType": EVENT_RECORD_DETAIL_TYPE,
            "Detail": body,
            "Resources": [completion.invocation.invoked_function_arn.clone()],
        }]
    });
    called(completion, "events", "PutEvents", parsed.region, &input.to_string()).await
}

/// The `Source` of the event an invocation record is published to a bus as: the service that
/// published it, as an AWS event names its publisher.
const EVENT_RECORD_SOURCE: &str = "lambda";

/// The `DetailType` of that event: what the detail IS.
const EVENT_RECORD_DETAIL_TYPE: &str = "Lambda Function Invocation Result";

/// `Invoke` the function the ARN `function` names with the record as its event.
///
/// ASYNCHRONOUSLY: a destination is delivered to off the completion of another invocation,
/// so nothing waits for the destination function's own handler — "Invoke the function
/// asynchronously […] The API response only includes a status code".
async fn invoke_function(
    completion: &Completion<'_>,
    function: &str,
    body: &str,
) -> Result<(), Undeliverable> {
    let parsed = ::service_common::arn::parse(function).ok_or_else(|| {
        Undeliverable::Refused(format!("{function} is no function ARN"))
    })?;
    let input = ::serde_json::json!({
        "FunctionName": function,
        "InvocationType": INVOCATION_TYPE_EVENT,
        "Payload": body,
    });
    called(completion, "lambda", "Invoke", parsed.region, &input.to_string()).await
}

/// The `InvocationType` a destination function is invoked under; see [`invoke_function`].
const INVOCATION_TYPE_EVENT: &str = "Event";

/// Make ONE call through the invocation's own SDK seam and read the destination's verdict
/// off it: a `2xx` is the destination having accepted the delivery, and everything else —
/// including a seam that could not make the call at all — is a delivery that was not made.
async fn called(
    completion: &Completion<'_>,
    service: &str,
    operation: &str,
    region: &str,
    body: &str,
) -> Result<(), Undeliverable> {
    let Some(aws) = completion.invocation.aws.as_ref() else {
        return Err(Undeliverable::Refused(
            "this emulator has no in-process seam to deliver through on this invocation"
                .to_string(),
        ));
    };
    let region = if region.is_empty() {
        &completion.invocation.region
    } else {
        region
    };
    match aws
        .call_within(service, operation, region, body, DELIVERY_CEILING)
        .await
    {
        Ok(answer) if (200..300).contains(&answer.status) => Ok(()),
        Ok(answer) => Err(Undeliverable::Refused(format!(
            "{service}:{operation} answered {} {}",
            answer.status,
            answer.error_type.as_deref().unwrap_or_default()
        ))),
        Err(refusal) => Err(Undeliverable::Refused(format!(
            "{service}:{operation} was not made: {refusal:?}"
        ))),
    }
}

/// Log what one delivery came to, naming what was delivered and where.
///
/// A delivery is never part of what a caller was told — the accepting call was answered
/// before the run — so a failure is reported here and metered, and nothing is retried.
fn report(
    completion: &Completion<'_>,
    destination: &str,
    what: &str,
    delivered: &Result<(), Undeliverable>,
) {
    if let Err(undeliverable) = delivered {
        ::tracing::warn!(
            function = %completion.invocation.function_name,
            request_id = %completion.invocation.request_id,
            destination = %destination,
            detail = %undeliverable.detail(),
            "the {what} of a completed asynchronous invocation did not reach its destination"
        );
    }
}

/// The INVOCATION RECORD one completed asynchronous event is delivered to its destination
/// as: the schema version, the instant the record was made, the context of the request, the
/// event itself, and the context and payload of the response.
///
/// Every value is the invocation's own: the request id and the function ARN it was addressed
/// by, how many attempts the event had, the version that ran, and what the handler answered.
/// A completion whose function never RAN carries no response at all — there was no run to
/// report a status, a version or a payload for — rather than a response the record would be
/// inventing.
fn invocation_record(completion: &Completion<'_>) -> ::serde_json::Value {
    let invocation = completion.invocation;
    let mut record = ::serde_json::json!({
        "version": RECORD_SCHEMA_VERSION,
        "timestamp": iso8601(SystemTime::now()),
        "requestContext": {
            "requestId": invocation.request_id,
            "functionArn": qualified_function_arn(completion),
            "condition": completion.condition(),
            "approximateInvokeCount": completion.attempts,
        },
        "requestPayload": document_or_text(&invocation.payload),
    });
    if let (Ok(invoked), Some(object)) = (completion.run, record.as_object_mut()) {
        let mut response = ::serde_json::json!({
            "statusCode": INVOKED_STATUS,
            "executedVersion": invocation.executed_version,
        });
        if let (Some(function_error), Some(context)) =
            (invoked.function_error.as_deref(), response.as_object_mut())
        {
            context.insert(
                "functionError".to_string(),
                ::serde_json::Value::String(function_error.to_string()),
            );
        }
        object.insert("responseContext".to_string(), response);
        object.insert(
            "responsePayload".to_string(),
            document_or_text(&invoked.payload),
        );
    }
    record
}

/// The `requestContext.functionArn` of an invocation record: the ARN the invocation was
/// addressed by, QUALIFIED by the qualifier the event was addressed to where that ARN carries
/// none of its own.
///
/// A record names the qualifier whose configuration delivered it — the documented record of
/// an unqualified asynchronous invocation carries the unpublished version
/// (`…:function:<name>:$LATEST`) — because an asynchronous-invocation configuration is held
/// per qualifier ([`super::EventInvokeConfigRecord`]), so the function ARN of the record says
/// which one's `DestinationConfig` this record was written for. An ARN the caller already
/// addressed WITH a qualifier is left exactly as it was: that qualifier is the invocation's.
fn qualified_function_arn(completion: &Completion<'_>) -> String {
    let arn = &completion.invocation.invoked_function_arn;
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return arn.clone();
    };
    let unqualified = parsed.vendor == "lambda" && parsed.resource.split(':').count() == 2;
    if unqualified && !completion.qualifier.is_empty() {
        return format!("{arn}:{}", completion.qualifier);
    }
    arn.clone()
}

/// `bytes` as the JSON document they are, and as the text they are when they are not a JSON
/// document at all — which is how a payload travels into a record either way.
fn document_or_text(bytes: &[u8]) -> ::serde_json::Value {
    let text = String::from_utf8_lossy(bytes);
    ::serde_json::from_str::<::serde_json::Value>(&text)
        .unwrap_or_else(|_| ::serde_json::Value::String(text.into_owned()))
}

/// The `errorMessage` of the error document a failed run answered with, when it answered one
/// that carries the member — the handler's OWN message, never a sentence written here.
fn failure_message(invoked: &super::execution::Invoked) -> Option<String> {
    let document = invoked.error_document()?;
    ::serde_json::from_str::<::serde_json::Value>(&document)
        .ok()?
        .get("errorMessage")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the record table: the case, what the attempt came to, what ended the
    /// event, and what the record has to say about it.
    struct Row {
        case: &'static str,
        run: Result<super::super::execution::Invoked, super::super::execution::Unrunnable>,
        ending: Ending,
        condition: &'static str,
        response: Option<(&'static str, Option<&'static str>)>,
    }

    /// A finished run carrying `function_error` and answering `payload`.
    fn invoked(
        payload: &str,
        function_error: Option<&str>,
    ) -> super::super::execution::Invoked {
        super::super::execution::Invoked {
            payload: payload.as_bytes().to_vec(),
            function_error: function_error.map(str::to_string),
            log: String::new(),
            log_events: Vec::new(),
            duration: Duration::from_millis(1),
            chunks: Vec::new(),
            stream_failure: None,
        }
    }

    /// The invocation the rows are built against: the one event, one request id and one
    /// version every record below reports.
    fn invocation() -> Invocation {
        Invocation {
            function_name: "probe".to_string(),
            executed_version: "$LATEST".to_string(),
            invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe"
                .to_string(),
            region: "us-west-2".to_string(),
            runtime: Some("python3.12".to_string()),
            handler: Some("app.handler".to_string()),
            code_revision: "11111111-aaaa-bbbb-cccc-222222222222".to_string(),
            variables: ::std::collections::BTreeMap::new(),
            memory_size: 128,
            architecture: "x86_64".to_string(),
            timeout: 3,
            zip_bytes: None,
            layer_zips: Vec::new(),
            payload: br#"{"fail":true,"seq":0}"#.to_vec(),
            client_context: None,
            tenant_id: None,
            request_id: "11111111-2222-3333-4444-555555555555".to_string(),
            trace: None,
            initialization_type: super::super::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            aws: None,
            logs: None,
            metrics: None,
        }
    }

    /// RULE: the record states the CONDITION the event ended in and reports a response only
    /// for an attempt whose function really ran. The rows span the outcomes one attempt can
    /// come to: a handler that returned, one that raised, one that raised another error, and
    /// each way a run can fail to start.
    #[test]
    fn a_record_reports_the_condition_and_only_a_response_that_happened() {
        let rows = vec![
            Row {
                case: "the handler returned",
                run: Ok(invoked(r#"{"ok":true}"#, None)),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_SUCCESS,
                response: Some((r#"{"ok":true}"#, None)),
            },
            Row {
                case: "the handler raised",
                run: Ok(invoked(r#"{"errorMessage":"boom"}"#, Some("Unhandled"))),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_RETRIES_EXHAUSTED,
                response: Some((r#"{"errorMessage":"boom"}"#, Some("Unhandled"))),
            },
            Row {
                case: "the handler ran out of memory",
                run: Ok(invoked(
                    r#"{"errorMessage":"out of memory"}"#,
                    Some("Unhandled.OutOfMemory"),
                )),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_RETRIES_EXHAUSTED,
                response: Some((
                    r#"{"errorMessage":"out of memory"}"#,
                    Some("Unhandled.OutOfMemory"),
                )),
            },
            Row {
                case: "no engine runs the function's package",
                run: Err(super::super::execution::Unrunnable::NoEngine {
                    runtime: "go1.x".to_string(),
                }),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_RETRIES_EXHAUSTED,
                response: None,
            },
            Row {
                case: "the configuration names no handler",
                run: Err(super::super::execution::Unrunnable::NoHandler),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_RETRIES_EXHAUSTED,
                response: None,
            },
            Row {
                case: "the package is not held",
                run: Err(super::super::execution::Unrunnable::PackageNotHeld),
                ending: Ending::AttemptsSpent,
                condition: CONDITION_RETRIES_EXHAUSTED,
                response: None,
            },
            // An event whose RETENTION window was spent before any capacity was free to run
            // the function for it: a different ending, so a different condition.
            Row {
                case: "no capacity was free inside the window the event is retained for",
                run: Err(super::super::execution::Unrunnable::Overloaded(
                    "the run envelope is full".to_string(),
                )),
                ending: Ending::AgeExceeded,
                condition: CONDITION_EVENT_AGE_EXCEEDED,
                response: None,
            },
        ];
        let invocation = invocation();
        for Row {
            case,
            run,
            ending,
            condition,
            response,
        } in rows
        {
            let completion = Completion {
                invocation: &invocation,
                qualifier: "$LATEST",
                attempts: 3,
                run: &run,
                ending,
            };
            let record = invocation_record(&completion);
            assert_eq!(
                record["requestContext"]["condition"].as_str(),
                Some(condition),
                "case {case:?}: expected the condition {condition:?}, got {record}"
            );
            assert_eq!(
                record["requestContext"]["requestId"].as_str(),
                Some(invocation.request_id.as_str()),
                "case {case:?}: expected the invocation's own request id, got {record}"
            );
            assert_eq!(
                record["requestContext"]["approximateInvokeCount"].as_u64(),
                Some(3),
                "case {case:?}: expected the attempts the event had, got {record}"
            );
            assert_eq!(
                record["requestPayload"],
                ::serde_json::json!({"fail": true, "seq": 0}),
                "case {case:?}: expected the event as the caller sent it, got {record}"
            );
            match response {
                Some((payload, function_error)) => {
                    assert_eq!(
                        record["responseContext"]["statusCode"].as_i64(),
                        Some(INVOKED_STATUS),
                        "case {case:?}: expected the invoked status, got {record}"
                    );
                    assert_eq!(
                        record["responseContext"]["functionError"].as_str(),
                        function_error,
                        "case {case:?}: expected the function error {function_error:?}, got \
                         {record}"
                    );
                    let expected = ::serde_json::from_str::<::serde_json::Value>(payload)
                        .expect("the row's payload is a document");
                    assert_eq!(
                        record["responsePayload"], expected,
                        "case {case:?}: expected the handler's own answer, got {record}"
                    );
                }
                None => {
                    assert!(
                        record.get("responseContext").is_none()
                            && record.get("responsePayload").is_none(),
                        "case {case:?}: a run that never happened reports no response, got \
                         {record}"
                    );
                }
            }
        }
    }

    /// RULE: a payload is carried into the record as the document it is, and as text when it
    /// is no document at all. The rows span the forms an event payload takes.
    #[test]
    fn a_payload_travels_as_the_document_or_the_text_it_is() {
        let rows: &[(&str, &[u8], ::serde_json::Value)] = &[
            (
                "an object",
                br#"{"seq":1}"#,
                ::serde_json::json!({"seq": 1}),
            ),
            ("an array", b"[1,2]", ::serde_json::json!([1, 2])),
            ("a number", b"7", ::serde_json::json!(7)),
            ("the empty payload", b"", ::serde_json::json!("")),
            (
                "text that is no document",
                b"not json",
                ::serde_json::json!("not json"),
            ),
        ];
        for (case, bytes, expected) in rows {
            let read = document_or_text(bytes);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected}, got {read}"
            );
        }
    }

    /// RULE: a destination that names the FUNCTION the completion is of is the one whose
    /// delivery the completion path itself would sustain, under any qualifier — and a
    /// destination of any other function, or of any other service, is not.
    ///
    /// The rows span what the reading generalises over: the same function unqualified, the
    /// same function at a version and at an alias, another function of the same account,
    /// another function whose name merely begins the same way, a resource of another service
    /// whose name matches, and a value that is no ARN.
    #[test]
    fn only_a_destination_naming_this_very_function_is_withheld() {
        let run: Result<super::super::execution::Invoked, super::super::execution::Unrunnable> =
            Ok(invoked(r#"{"ok":true}"#, None));
        let invocation = invocation();
        let completion = Completion {
            invocation: &invocation,
            qualifier: "$LATEST",
            attempts: 1,
            run: &run,
            ending: Ending::AttemptsSpent,
        };
        let rows: &[(&str, &str, bool)] = &[
            (
                "the function itself, unqualified",
                "arn:aws:lambda:us-west-2:000000000000:function:probe",
                true,
            ),
            (
                "the function itself at a published version",
                "arn:aws:lambda:us-west-2:000000000000:function:probe:7",
                true,
            ),
            (
                "the function itself at an alias",
                "arn:aws:lambda:us-west-2:000000000000:function:probe:live",
                true,
            ),
            (
                "another function",
                "arn:aws:lambda:us-west-2:000000000000:function:records",
                false,
            ),
            (
                "a function whose name merely begins the same way",
                "arn:aws:lambda:us-west-2:000000000000:function:probe-records",
                false,
            ),
            (
                "a queue of the same name",
                "arn:aws:sqs:us-west-2:000000000000:probe",
                false,
            ),
            ("a value that is no ARN at all", "probe", false),
        ];
        for (case, destination, withheld) in rows {
            let decided = completion.names_its_own_function(destination);
            assert_eq!(
                decided, *withheld,
                "case {case:?}: expected names_its_own_function({destination:?}) to be \
                 {withheld}, got {decided}"
            );
        }
    }
}
