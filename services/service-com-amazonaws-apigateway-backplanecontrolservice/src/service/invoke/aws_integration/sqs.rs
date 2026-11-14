// SPDX-License-Identifier: CC0-1.0

//! The `AWS` integration whose `uri` names AMAZON SQS: how a method request becomes one
//! queue action, made in process.
//!
//! `Integration$uri` gives the form that action is named in —
//! `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/{service_api}` — and
//! Amazon SQS's own api is a QUERY api: a request names its action in an `Action` field and
//! that action's inputs in the other fields of an `application/x-www-form-urlencoded` body,
//! which is why an integration to a queue carries a request template that renders that form.
//! So the action and its parameters are read from the integration request the way the
//! service's own wire form carries them, over the three places this integration puts them —
//! the `uri`'s own pairs, the query string the integration maps, and the rendered request
//! template — which is the one reading [`super::query_form`] states for every service of that
//! protocol this module calls.
//!
//! WHICH call those fields name is this module's own: the
//! `path/{account-id}/{queue-name}` form of the `uri` names the QUEUE
//! rather than the action, and the action is then the `Action` field of the integration
//! request; the `action` form names the action, and the queue is then the `QueueUrl` field.
//!
//! The call is made THROUGH THE EMULATOR this service runs within, over the in-process
//! transport `OperationContext::callback` hands a handler: nothing is invented and no
//! connection is opened. What the queue service answered in process is then written in the
//! document its api answers that action with — the `{Action}Response` envelope an
//! `awsQueryCompatible` service wraps a query request's answer in, whose result member
//! carries the members the model declares for that action's output and whose
//! `ResponseMetadata` names the request — and a refusal in that envelope's error form, whose
//! `Error` member carries the fault side, the code and the sentence. The code is the LEGACY
//! query code the answer itself reported (the `x-amzn-query-error` header that service
//! completes its envelope with) rather than one chosen here.
//!
//! Who the call is made as — the role `Integration$credentials` names — and what a role that
//! may not make it earns is the one rule the parent module states for every `AWS`
//! integration, and this module only words the refusal ([`access_denied`]).

use std::collections::BTreeMap;

use ::aws_sdk_sqs::types::MessageAttributeValue;
use ::smithy_server_runtime::OperationContext;

use crate::ops::types::GetIntegrationOutput;

use super::super::{MethodRequest, UnservedIntegration};
use super::query_form;
use super::{
    bounded, named_service, refusal_of, substituted, unanswered, Answered, ACCESS_DENIED_CODE,
    ACCESS_DENIED_STATUS, ACTION_FORM_PREFIX, APIGATEWAY_VENDOR, PATH_FORM_PREFIX,
};

/// The subdomain field that names Amazon SQS, from the form `Integration$uri` documents.
const SQS_SUBDOMAIN: &str = "sqs";

/// What Amazon SQS is called in the sentence an integration that reached no answer carries.
const SQS_SERVICE_NAME: &str = "Amazon SQS";

/// The media type the document an answer carries is JSON under.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The `Type` of an error document whose fault is the CALLER's.
const SENDER_FAULT: &str = "Sender";

/// The `Type` of one whose fault is the SERVICE's.
const RECEIVER_FAULT: &str = "Receiver";

/// The lowest status whose fault is the service's own.
const FIRST_SERVER_FAULT_STATUS: u16 = 500;

/// The generic code AWS reports for a failure whose class an answer named none for.
const GENERIC_FAILURE_CODE: &str = "InternalFailure";

/// The field that names the action, which is how the query api carries one.
const ACTION_PARAMETER: &str = "Action";

/// The field that names the queue, which every action of the query api takes
/// (`QueueUrl`, required on each of them per `smithy_query(query="operation")`).
const QUEUE_URL_PARAMETER: &str = "QueueUrl";

/// `SendMessage$MessageBody`, the one input a send takes beside the queue.
const MESSAGE_BODY_PARAMETER: &str = "MessageBody";

/// `SendMessage$DelaySeconds`.
const DELAY_SECONDS_PARAMETER: &str = "DelaySeconds";

/// `SendMessage$MessageGroupId`.
const MESSAGE_GROUP_ID_PARAMETER: &str = "MessageGroupId";

/// `SendMessage$MessageDeduplicationId`.
const MESSAGE_DEDUPLICATION_ID_PARAMETER: &str = "MessageDeduplicationId";

/// `SendMessage$MessageAttributes`, whose `xmlName` is `MessageAttribute` and which is
/// flattened, so the wire form of one entry is `MessageAttribute.{n}.Name` with its value's
/// members under `MessageAttribute.{n}.Value.` (`smithy_query(query="operation",
/// operation="SendMessage")`).
const MESSAGE_ATTRIBUTE_PARAMETER: &str = "MessageAttribute";

/// `DeleteMessage$ReceiptHandle`.
const RECEIPT_HANDLE_PARAMETER: &str = "ReceiptHandle";

/// `ReceiveMessage$MaxNumberOfMessages`.
const MAX_MESSAGES_PARAMETER: &str = "MaxNumberOfMessages";

/// `ReceiveMessage$VisibilityTimeout`.
const VISIBILITY_TIMEOUT_PARAMETER: &str = "VisibilityTimeout";

/// `ReceiveMessage$WaitTimeSeconds`.
const WAIT_TIME_PARAMETER: &str = "WaitTimeSeconds";

/// The `xmlName` of both `ReceiveMessage$AttributeNames` and
/// `ReceiveMessage$MessageSystemAttributeNames`, and of `GetQueueAttributes$AttributeNames`:
/// the flattened list a request names system attributes in.
const ATTRIBUTE_NAME_PARAMETER: &str = "AttributeName";

/// The `xmlName` of `ReceiveMessage$MessageAttributeNames`.
const MESSAGE_ATTRIBUTE_NAME_PARAMETER: &str = "MessageAttributeName";

/// The code the query api refuses a request that names no value for a required parameter
/// with, and the sentence it words that refusal with.
const MISSING_PARAMETER_CODE: &str = "MissingParameter";

/// The code it refuses a parameter whose value the action does not admit with.
const INVALID_PARAMETER_VALUE_CODE: &str = "InvalidParameterValue";

/// The status both of those refusals carry: a request the caller got wrong.
const BAD_REQUEST_STATUS: u16 = 400;

/// The response header an `awsQueryCompatible` service reports the LEGACY query code of the
/// class it threw in, as `{code};{fault}` — which is the code a client of the query api
/// reads, and so the one the refusal this integration passes on carries.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// One Amazon SQS action an `AWS` integration's request names.
///
/// The set is the queue's MESSAGE plane plus the attribute read — the actions an api that
/// proxies a queue is made of — and an action outside it is one this emulator does not
/// perform rather than one answered with something close to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueAction {
    /// `SendMessage`.
    Send,
    /// `ReceiveMessage`.
    Receive,
    /// `DeleteMessage`.
    Delete,
    /// `GetQueueAttributes`.
    Attributes,
}

impl QueueAction {
    /// The action the `Action` field names, by the operation's own name.
    fn of_name(name: &str) -> Option<Self> {
        match name {
            "SendMessage" => Some(Self::Send),
            "ReceiveMessage" => Some(Self::Receive),
            "DeleteMessage" => Some(Self::Delete),
            "GetQueueAttributes" => Some(Self::Attributes),
            _ => None,
        }
    }

    /// The operation's name, which the `<{Action}Response>` envelope is named by.
    fn name(self) -> &'static str {
        match self {
            Self::Send => "SendMessage",
            Self::Receive => "ReceiveMessage",
            Self::Delete => "DeleteMessage",
            Self::Attributes => "GetQueueAttributes",
        }
    }

    /// The IAM action a call of it is authorized by: the service's `arnNamespace` and the
    /// operation's own name, which is how Amazon SQS names every one of its actions.
    fn iam_action(self) -> &'static str {
        match self {
            Self::Send => "sqs:SendMessage",
            Self::Receive => "sqs:ReceiveMessage",
            Self::Delete => "sqs:DeleteMessage",
            Self::Attributes => "sqs:GetQueueAttributes",
        }
    }
}

/// What an `AWS` integration naming Amazon SQS addresses, as its `uri` and the integration
/// request resolve against this call: the queue, the Region of the endpoint the call is made
/// at, the action, and the parameters that action's inputs are read from.
#[derive(Debug)]
pub(super) struct QueueCall {
    /// The partition the integration ARN names, which the queue's own ARN carries.
    partition: String,
    /// The Region the integration ARN names, or the api's own when it names none.
    region: String,
    /// The account the queue belongs to, as the queue's url and ARN name one.
    account: String,
    /// The queue's own name.
    queue: String,
    action: QueueAction,
    /// The fields of the integration request, by name: every parameter of the query form
    /// this call carries, whichever of the three places it came from.
    parameters: BTreeMap<String, String>,
}

impl QueueCall {
    /// The Region of the service endpoint the call is made at.
    pub(super) fn region(&self) -> &str {
        &self.region
    }

    /// The IAM action a call of it is authorized by.
    pub(super) fn iam_action(&self) -> &'static str {
        self.action.iam_action()
    }

    /// The ARN of the queue this call addresses, which is the resource an identity-based
    /// policy grants the action on: an Amazon SQS queue ARN names the queue by its own name
    /// (`arn:{partition}:sqs:{region}:{account}:{name}`).
    pub(super) fn queue_arn(&self) -> String {
        format!(
            "arn:{partition}:sqs:{region}:{account}:{queue}",
            partition = self.partition,
            region = self.region,
            account = self.account,
            queue = self.queue
        )
    }

    /// The url the call addresses the queue by, which is `{endpoint}/{account}/{name}` —
    /// the form `CreateQueue` hands back and `GetQueueUrl` reports.
    pub(super) fn queue_url(&self) -> String {
        format!(
            "https://sqs.{region}.amazonaws.com/{account}/{queue}",
            region = self.region,
            account = self.account,
            queue = self.queue
        )
    }
}

/// The refusal Amazon SQS answers a call the caller may not make with: HTTP 403 with the
/// `AccessDenied` code and the sentence the query api words it with, which names the queue
/// the call addressed.
pub(super) fn access_denied(queue: &QueueCall) -> Answered {
    let url = queue.queue_url();
    error_answer(
        ACCESS_DENIED_STATUS,
        ACCESS_DENIED_CODE,
        &format!("Access to the resource {url} is denied."),
    )
}

/// What `integration`'s `uri` addresses when it names an Amazon SQS queue action:
///
/// * `None` when the `uri` names no Amazon SQS call, which leaves it to another reader;
/// * `Some(Err(…))` when it DOES name Amazon SQS and this call cannot be placed against it,
///   which is a configuration this stage cannot serve — or an integration request no
///   template of it maps, which is the one refusal a method request earns here;
/// * `Some(Ok(call))` for the call to make.
pub(super) fn addressed_queue(
    integration: &GetIntegrationOutput,
    default_region: &str,
    call: &MethodRequest,
) -> Option<Result<QueueCall, UnservedIntegration>> {
    let uri = integration.uri.as_deref()?;
    let arn = ::service_common::arn::parse(uri)?;
    if arn.vendor != APIGATEWAY_VENDOR || named_service(arn.account) != SQS_SUBDOMAIN {
        return None;
    }
    Some(resolved_queue(integration, &arn, default_region, call))
}

/// The call the integration ARN `arn` and the integration request together name.
fn resolved_queue(
    integration: &GetIntegrationOutput,
    arn: &::service_common::arn::Arn<'_>,
    default_region: &str,
    call: &MethodRequest,
) -> Result<QueueCall, UnservedIntegration> {
    let region = match arn.region.is_empty() {
        true => default_region,
        false => arn.region,
    };
    let resource =
        substituted(arn.resource, integration, &call.inputs).map_err(|reason| unanswered(&reason))?;
    let named_queue = match resource.strip_prefix(PATH_FORM_PREFIX) {
        Some(path) => Some(addressed_path(path).ok_or_else(|| {
            unanswered(
                "the integration's uri addresses the Amazon SQS path form but names no \
                 {account-id}/{queue-name} under it",
            )
        })?),
        None => None,
    };
    let (named_action, fixed) = match resource.strip_prefix(ACTION_FORM_PREFIX) {
        Some(named) => match named.split_once('&') {
            Some((name, pairs)) => (Some(name.to_string()), pairs),
            None => (Some(named.to_string()), ""),
        },
        None => (None, ""),
    };
    if named_queue.is_none() && named_action.is_none() {
        return Err(unanswered(
            "the integration's uri addresses neither the path nor the action form of an \
             Amazon SQS call",
        ));
    }
    // The three places an `AWS` integration carries the fields of a query api's request, in
    // the order a later one overrides an earlier ([`query_form`]).
    let parameters = query_form::carried_parameters(integration, call, fixed)?;

    let action_name = named_action
        .or_else(|| parameters.get(ACTION_PARAMETER).cloned())
        .ok_or_else(|| {
            unanswered(
                "the integration's uri names no Amazon SQS action and its integration request \
                 carries no Action field, so it names no call",
            )
        })?;
    let action = QueueAction::of_name(&action_name).ok_or_else(|| {
        unanswered(&format!(
            "the integration request names the Amazon SQS action {action_name}, which this \
             emulator does not perform"
        ))
    })?;
    let (account, queue) = match named_queue {
        Some(addressed) => addressed,
        None => {
            let url = parameters.get(QUEUE_URL_PARAMETER).ok_or_else(|| {
                unanswered(
                    "the integration's uri names no queue and its integration request carries \
                     no QueueUrl field, so the call addresses no queue",
                )
            })?;
            split_queue_path(url).ok_or_else(|| {
                unanswered(
                    "the integration request's QueueUrl names no {account-id}/{queue-name}, so \
                     the call addresses no queue",
                )
            })?
        }
    };
    if account.is_empty() || queue.is_empty() {
        return Err(unanswered(
            "the integration's uri names an empty account or queue name, so it addresses no \
             queue",
        ));
    }
    Ok(QueueCall {
        partition: arn.partition.to_string(),
        region: region.to_string(),
        account,
        queue,
        action,
        parameters,
    })
}

/// The queue the `path` form of an integration `uri` names: `{account-id}/{queue-name}`,
/// which is Amazon SQS's own REST path for a queue, or a name alone — which is a path
/// override that names only the queue, and so one of THIS account, the account whose queues
/// this emulator holds.
fn addressed_path(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.contains('/') {
        true => split_queue_path(trimmed),
        false => Some((::engine::emulator_account_id().to_string(), trimmed.to_string())),
    }
}

/// The `{account-id}` and `{queue-name}` a queue url ends in, which is how a `QueueUrl`
/// field names a queue.
fn split_queue_path(path: &str) -> Option<(String, String)> {
    let (before, queue) = path.trim_end_matches('/').rsplit_once('/')?;
    let account = match before.rsplit_once('/') {
        Some((_, account)) => account,
        None => before,
    };
    (!queue.is_empty() && !account.is_empty())
        .then(|| (account.to_string(), queue.to_string()))
}

/// Make the call `queue` addresses against Amazon SQS, in process, and report what it
/// answered in the document that api answers the action with.
///
/// `Err` only where there is no answer to report at all — the call did not reach the
/// service, or its answer is larger than one method response may carry. Every answer the
/// service DID give, refusals included, is reported as [`Answered`] and mapped.
pub(super) async fn answered(
    ctx: &OperationContext<'_>,
    queue: &QueueCall,
    _call: &MethodRequest,
) -> Result<Answered, UnservedIntegration> {
    let client = sqs_client(ctx, &queue.region);
    let url = queue.queue_url();
    match queue.action {
        QueueAction::Send => {
            let Some(message) = queue.parameters.get(MESSAGE_BODY_PARAMETER) else {
                return Ok(missing_parameter(MESSAGE_BODY_PARAMETER));
            };
            let attributes = match message_attributes(&queue.parameters) {
                Ok(attributes) => attributes,
                Err(absent) => return Ok(missing_parameter(&absent)),
            };
            let mut send = client
                .send_message()
                .queue_url(&url)
                .message_body(message)
                .set_message_group_id(queue.parameters.get(MESSAGE_GROUP_ID_PARAMETER).cloned())
                .set_message_deduplication_id(
                    queue
                        .parameters
                        .get(MESSAGE_DEDUPLICATION_ID_PARAMETER)
                        .cloned(),
                );
            match integer(&queue.parameters, DELAY_SECONDS_PARAMETER) {
                Ok(delay) => send = send.set_delay_seconds(delay),
                Err(invalid) => return Ok(invalid),
            }
            for (name, value) in attributes {
                send = send.message_attributes(name, value);
            }
            match send.send().await {
                Ok(answer) => answered_document(
                    queue.action,
                    Some(members(&[
                        ("MD5OfMessageAttributes", answer.md5_of_message_attributes()),
                        ("MD5OfMessageBody", answer.md5_of_message_body()),
                        (
                            "MD5OfMessageSystemAttributes",
                            answer.md5_of_message_system_attributes(),
                        ),
                        ("MessageId", answer.message_id()),
                        ("SequenceNumber", answer.sequence_number()),
                    ])),
                ),
                Err(error) => refused(&error),
            }
        }
        QueueAction::Receive => {
            // Each numeric input is read on its own, so a value the integration request
            // names for one of them cannot reach another's setter.
            let messages = match integer(&queue.parameters, MAX_MESSAGES_PARAMETER) {
                Ok(read) => read,
                Err(invalid) => return Ok(invalid),
            };
            let visibility = match integer(&queue.parameters, VISIBILITY_TIMEOUT_PARAMETER) {
                Ok(read) => read,
                Err(invalid) => return Ok(invalid),
            };
            let wait = match integer(&queue.parameters, WAIT_TIME_PARAMETER) {
                Ok(read) => read,
                Err(invalid) => return Ok(invalid),
            };
            let mut receive = client
                .receive_message()
                .queue_url(&url)
                .set_max_number_of_messages(messages)
                .set_visibility_timeout(visibility)
                .set_wait_time_seconds(wait);
            // `AttributeNames` is the spelling the model marks "replaced by
            // MessageSystemAttributeNames", so both spellings name the same list and are
            // read onto the member that is not deprecated.
            for name in list_parameter(&queue.parameters, ATTRIBUTE_NAME_PARAMETER) {
                receive = receive.message_system_attribute_names(
                    ::aws_sdk_sqs::types::MessageSystemAttributeName::from(name.as_str()),
                );
            }
            for name in list_parameter(&queue.parameters, MESSAGE_ATTRIBUTE_NAME_PARAMETER) {
                receive = receive.message_attribute_names(name);
            }
            match receive.send().await {
                Ok(answer) => answered_document(
                    queue.action,
                    Some(::serde_json::json!({
                        "Messages": answer
                            .messages()
                            .iter()
                            .map(received_message)
                            .collect::<Vec<::serde_json::Value>>(),
                    })),
                ),
                Err(error) => refused(&error),
            }
        }
        QueueAction::Delete => {
            let Some(handle) = queue.parameters.get(RECEIPT_HANDLE_PARAMETER) else {
                return Ok(missing_parameter(RECEIPT_HANDLE_PARAMETER));
            };
            match client
                .delete_message()
                .queue_url(&url)
                .receipt_handle(handle)
                .send()
                .await
            {
                // `DeleteMessage` answers no output shape, so its document carries the
                // response metadata alone.
                Ok(_) => answered_document(queue.action, None),
                Err(error) => refused(&error),
            }
        }
        QueueAction::Attributes => {
            let mut get = client.get_queue_attributes().queue_url(&url);
            for name in list_parameter(&queue.parameters, ATTRIBUTE_NAME_PARAMETER) {
                get = get.attribute_names(::aws_sdk_sqs::types::QueueAttributeName::from(
                    name.as_str(),
                ));
            }
            match get.send().await {
                Ok(answer) => {
                    // The attributes are the output's own map, ordered by name so one
                    // stored queue answers one document however the map was walked.
                    let ordered: ::serde_json::Map<String, ::serde_json::Value> = answer
                        .attributes()
                        .into_iter()
                        .flatten()
                        .map(|(name, value)| {
                            (name.as_str().to_string(), ::serde_json::json!(value))
                        })
                        .collect();
                    answered_document(
                        queue.action,
                        Some(::serde_json::json!({ "Attributes": ordered })),
                    )
                }
                Err(error) => refused(&error),
            }
        }
    }
}

/// One received message as the answer carries it: the `Message` members the model declares,
/// each written only where the service answered a value for it.
fn received_message(message: &::aws_sdk_sqs::types::Message) -> ::serde_json::Value {
    let mut written = members(&[
        ("Body", message.body()),
        ("MD5OfBody", message.md5_of_body()),
        (
            "MD5OfMessageAttributes",
            message.md5_of_message_attributes(),
        ),
        ("MessageId", message.message_id()),
        ("ReceiptHandle", message.receipt_handle()),
    ]);
    let system: ::serde_json::Map<String, ::serde_json::Value> = message
        .attributes()
        .into_iter()
        .flatten()
        .map(|(name, value)| (name.as_str().to_string(), ::serde_json::json!(value)))
        .collect();
    if let Some(object) = written.as_object_mut() {
        object.insert("Attributes".to_string(), ::serde_json::json!(system));
        let attributes: ::serde_json::Map<String, ::serde_json::Value> = message
            .message_attributes()
            .into_iter()
            .flatten()
            .map(|(name, value)| (name.clone(), message_attribute(value)))
            .collect();
        object.insert(
            "MessageAttributes".to_string(),
            ::serde_json::json!(attributes),
        );
    }
    written
}

/// One message attribute's value as the answer carries it: the members
/// `MessageAttributeValue` declares, with a `Binary` member base64-encoded as the wire
/// carries every blob.
fn message_attribute(value: &MessageAttributeValue) -> ::serde_json::Value {
    let mut written = members(&[
        ("DataType", Some(value.data_type())),
        ("StringValue", value.string_value()),
    ]);
    if let Some(object) = written.as_object_mut() {
        let binary = value.binary_value().map(|blob| {
            ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, blob.as_ref())
        });
        object.insert("BinaryValue".to_string(), ::serde_json::json!(binary));
    }
    written
}

/// The message attributes the integration request names, by name, or the field whose value
/// the request did not carry — which the query api refuses the call for.
///
/// One entry is `MessageAttribute.{n}.Name` with its value's members under
/// `MessageAttribute.{n}.Value.`, numbered from 1; an index whose `Name` or whose
/// `Value.DataType` is absent is a required field the request did not carry, and
/// `MessageAttributeValue$DataType` is the member the model marks required.
fn message_attributes(
    parameters: &BTreeMap<String, String>,
) -> Result<Vec<(String, MessageAttributeValue)>, String> {
    let mut named = Vec::new();
    for index in entry_indices(parameters, MESSAGE_ATTRIBUTE_PARAMETER) {
        let member = |suffix: &str| {
            parameters
                .get(&format!("{MESSAGE_ATTRIBUTE_PARAMETER}.{index}.{suffix}"))
                .cloned()
        };
        let name = member("Name").ok_or_else(|| {
            format!("{MESSAGE_ATTRIBUTE_PARAMETER}.{index}.Name")
        })?;
        let data_type = member("Value.DataType").ok_or_else(|| {
            format!("{MESSAGE_ATTRIBUTE_PARAMETER}.{index}.Value.DataType")
        })?;
        let mut value = MessageAttributeValue::builder()
            .data_type(data_type)
            .set_string_value(member("Value.StringValue"));
        if let Some(binary) = member("Value.BinaryValue") {
            // The query form carries a blob base64-encoded, which is how the wire carries
            // every `Binary` member.
            let decoded = ::base64::Engine::decode(
                &::base64::engine::general_purpose::STANDARD,
                binary.as_bytes(),
            )
            .map_err(|_| format!("{MESSAGE_ATTRIBUTE_PARAMETER}.{index}.Value.BinaryValue"))?;
            value = value.binary_value(::aws_sdk_sqs::primitives::Blob::new(decoded));
        }
        let built = value
            .build()
            .map_err(|_| format!("{MESSAGE_ATTRIBUTE_PARAMETER}.{index}.Value.DataType"))?;
        named.push((name, built));
    }
    Ok(named)
}

/// The indices of the entries of a flattened member the query form numbers from 1
/// (`{name}.{n}.…`), in index order and each reported once.
fn entry_indices(parameters: &BTreeMap<String, String>, name: &str) -> Vec<u32> {
    let prefix = format!("{name}.");
    let mut indices: Vec<u32> = parameters
        .keys()
        .filter_map(|key| {
            let rest = key.strip_prefix(&prefix)?;
            let index = rest.split('.').next()?;
            index.parse::<u32>().ok()
        })
        .collect();
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// The values of a flattened LIST member, which the query form carries as `{name}.{n}`
/// members numbered from 1, in index order.
fn list_parameter(parameters: &BTreeMap<String, String>, name: &str) -> Vec<String> {
    let prefix = format!("{name}.");
    let mut numbered: Vec<(u32, &String)> = parameters
        .iter()
        .filter_map(|(key, value)| {
            let index = key.strip_prefix(&prefix)?.parse::<u32>().ok()?;
            Some((index, value))
        })
        .collect();
    numbered.sort_by_key(|(index, _)| *index);
    numbered
        .into_iter()
        .map(|(_, value)| value.clone())
        .collect()
}

/// The integer the field `name` names, `None` when the request carries none, and the query
/// api's own refusal for a value that is no integer at all — which is a value the action
/// does not admit rather than one silently dropped.
///
/// The sentence is the one this api words that class with — the value in parentheses, the
/// field it was named for, and the reason — and the reason is what the model states of every
/// one of these members: each targets `NullableInteger`
/// (`smithy_query(query="operation",operation="ReceiveMessage")`), so a value that is no
/// integer is one the action cannot take.
fn integer(parameters: &BTreeMap<String, String>, name: &str) -> Result<Option<i32>, Answered> {
    let Some(value) = parameters.get(name) else {
        return Ok(None);
    };
    match value.trim().parse::<i32>() {
        Ok(read) => Ok(Some(read)),
        Err(_) => Err(error_answer(
            BAD_REQUEST_STATUS,
            INVALID_PARAMETER_VALUE_CODE,
            &format!(
                "Value ({value}) for parameter {name} is invalid. Reason: Must be an integer."
            ),
        )),
    }
}

/// The refusal the query api answers a request that carries no value for a required field
/// with: `MissingParameter` and the sentence naming that field.
fn missing_parameter(name: &str) -> Answered {
    error_answer(
        BAD_REQUEST_STATUS,
        MISSING_PARAMETER_CODE,
        &format!("The request must contain the parameter {name}."),
    )
}

/// What Amazon SQS answered a call it refused, as the method response maps it: the error
/// document of the same envelope, carrying the code the answer ITSELF reported and the
/// sentence it reported beside it, under the status it refused the call with.
///
/// The code is the one the answer's `x-amzn-query-error` header names — the report an
/// `awsQueryCompatible` service completes its envelope with, which is the code a client of
/// the query form of this api reads — and the modelled class's own code where the answer
/// named none.
fn refused<E>(error: &::aws_smithy_runtime_api::client::result::SdkError<E, ::aws_smithy_runtime_api::client::orchestrator::HttpResponse>) -> Result<Answered, UnservedIntegration>
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
{
    let refusal = refusal_of(error, SQS_SERVICE_NAME)?;
    let reported = refusal
        .headers
        .get(QUERY_ERROR_HEADER)
        .and_then(|report| report.split(';').next())
        .filter(|code| !code.is_empty())
        .map(str::to_string);
    let code = reported
        .or_else(|| refusal.code.clone())
        // The generic code AWS reports for a failure whose class the answer named none for.
        .unwrap_or_else(|| GENERIC_FAILURE_CODE.to_string());
    let message = refusal.message.clone().unwrap_or_default();
    Ok(error_answer(refusal.status, &code, &message))
}

/// The error document this api answers a refusal with: the `Error` member carrying the
/// FAULT side, the code and the sentence, beside the `RequestId` that names the request —
/// the envelope an `awsQueryCompatible` service answers a query request's failure in, under
/// the status it refuses the call with.
///
/// The fault is the caller's for every status below [`FIRST_SERVER_FAULT_STATUS`] and the
/// service's at or above it, which is how that envelope's two sides are decided.
fn error_answer(status: u16, code: &str, message: &str) -> Answered {
    let fault = match status < FIRST_SERVER_FAULT_STATUS {
        true => SENDER_FAULT,
        false => RECEIVER_FAULT,
    };
    Answered {
        status,
        content_type: Some(JSON_CONTENT_TYPE.to_string()),
        body: ::serde_json::json!({
            "Error": {
                "Type": fault,
                "Code": code,
                "Message": message,
                "Detail": {},
            },
            "RequestId": ::service_common::query_error_envelope::new_request_id(),
        })
        .to_string()
        .into_bytes(),
    }
}

/// The document this api answers `action` with: the result member carrying `result`, where
/// the action declares an output shape, and the `ResponseMetadata` every such answer
/// carries, both under the `{Action}Response` member that wraps them.
fn answered_document(
    action: QueueAction,
    result: Option<::serde_json::Value>,
) -> Result<Answered, UnservedIntegration> {
    let name = action.name();
    let mut answer = ::serde_json::Map::new();
    if let Some(result) = result {
        answer.insert(format!("{name}Result"), result);
    }
    answer.insert(
        "ResponseMetadata".to_string(),
        ::serde_json::json!({
            "RequestId": ::service_common::query_error_envelope::new_request_id(),
        }),
    );
    let body = ::serde_json::json!({ format!("{name}Response"): answer })
        .to_string()
        .into_bytes();
    bounded(body.len())?;
    Ok(Answered {
        status: 200,
        content_type: Some(JSON_CONTENT_TYPE.to_string()),
        body,
    })
}

/// The members of a result: the value the service answered for each, and `null` for each it
/// answered none for — which is what an absent member is written as on this wire.
fn members(named: &[(&str, Option<&str>)]) -> ::serde_json::Value {
    ::serde_json::Value::Object(
        named
            .iter()
            .map(|(name, value)| ((*name).to_string(), ::serde_json::json!(value)))
            .collect(),
    )
}

/// The Amazon SQS client the integration request is made with.
///
/// The transport is the emulator's own, in process, so the call is served by the Amazon SQS
/// this emulator mounts and no connection is opened; the credentials are the ones the
/// callback hands over, which sign this call and are never reported or logged.
fn sqs_client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_sqs::Client {
    let config = ::aws_sdk_sqs::Config::builder()
        .behavior_version(::aws_sdk_sqs::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_sqs::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sqs.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_sqs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sqs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sqs::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sqs::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sqs::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    //! What an integration `uri` and an integration request together address, which is the
    //! one part of this module that needs neither a backend nor a role to decide.

    use super::*;

    use crate::service::mapping_template::TemplateInputs;

    /// The account every queue below belongs to, written as a queue url and a path form
    /// name one.
    const ACCOUNT: &str = "000000000000";

    /// An `AWS` integration to Amazon SQS: `uri`, the request template that renders the
    /// query form, and the parameters it maps.
    fn integration(
        uri: &str,
        template: Option<&str>,
        parameters: &[(&str, &str)],
    ) -> GetIntegrationOutput {
        GetIntegrationOutput {
            r#type: Some("AWS".to_string()),
            http_method: Some("POST".to_string()),
            uri: Some(uri.to_string()),
            request_templates: template.map(|body| {
                [("application/x-www-form-urlencoded".to_string(), body.to_string())]
                    .into_iter()
                    .collect()
            }),
            request_parameters: Some(
                parameters
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
            ..GetIntegrationOutput::default()
        }
    }

    /// A form-encoded method request carrying `payload`, with one query parameter the
    /// mapping rows name.
    fn request(payload: &str) -> MethodRequest {
        MethodRequest::simulated(
            Some("application/x-www-form-urlencoded"),
            None,
            TemplateInputs {
                payload: Some(payload.as_bytes().to_vec()),
                querystring: [("note".to_string(), "from the query string".to_string())]
                    .into_iter()
                    .collect(),
                ..TemplateInputs::default()
            },
            crate::service::invoke::lambda_proxy::ProxyInputs::default(),
        )
    }

    /// One row of the table below: the case name, the integration whose `uri` and request
    /// template are read, and the `(queue, action, message body)` the call it places
    /// addresses — `None` for a configuration this stage cannot serve.
    type Row = (
        &'static str,
        GetIntegrationOutput,
        Option<(&'static str, QueueAction, &'static str)>,
    );

    #[test]
    fn the_uri_and_the_integration_request_decide_which_queue_call_is_made() {
        // Rows differ along what the rule generalises over: WHICH of the three places the
        // action and its parameters come from, the FORM the uri names the call in, and the
        // action named.
        let cases: Vec<Row> = vec![
            (
                "the path form with the action in the rendered body",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:path/{ACCOUNT}/orders"),
                    Some("Action=SendMessage&MessageBody=$input.body"),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "the action form with the queue in the rendered body",
                integration(
                    "arn:aws:apigateway:us-east-1:sqs:action/SendMessage",
                    Some(&format!(
                        "QueueUrl=https%3A%2F%2Fsqs.us-east-1.amazonaws.com%2F{ACCOUNT}%2Forders\
                         &MessageBody=$input.body"
                    )),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "the action form whose own pairs fix the queue",
                integration(
                    &format!(
                        "arn:aws:apigateway:us-east-1:sqs:action/ReceiveMessage&QueueUrl=/{ACCOUNT}/orders"
                    ),
                    None,
                    &[],
                ),
                Some(("orders", QueueAction::Receive, "")),
            ),
            (
                "a parameter mapped onto the integration request's query string",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:path/{ACCOUNT}/orders"),
                    None,
                    &[
                        ("integration.request.querystring.Action", "'SendMessage'"),
                        (
                            "integration.request.querystring.MessageBody",
                            "method.request.querystring.note",
                        ),
                    ],
                ),
                Some(("orders", QueueAction::Send, "from the query string")),
            ),
            (
                "the body overriding a parameter the uri fixes",
                integration(
                    &format!(
                        "arn:aws:apigateway:us-east-1:sqs:action/SendMessage&QueueUrl=/{ACCOUNT}/orders"
                    ),
                    Some("MessageBody=$input.body"),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "the attribute read, named in the body",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:path/{ACCOUNT}/orders"),
                    Some("Action=GetQueueAttributes&AttributeName.1=All"),
                    &[],
                ),
                Some(("orders", QueueAction::Attributes, "")),
            ),
            (
                "the path form whose uri carries a subdomain in front of the service",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:queue.sqs:path/{ACCOUNT}/orders"),
                    Some("Action=SendMessage&MessageBody=$input.body"),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "a path override naming the queue alone, which is one of this account",
                integration(
                    "arn:aws:apigateway:us-east-1:sqs:path/orders",
                    Some("Action=SendMessage&MessageBody=$input.body"),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "a JSON body, which is the other wire form of the same inputs",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:action/SendMessage&QueueUrl=%2F{ACCOUNT}%2Forders"),
                    Some(r#"{"MessageBody":"$input.body"}"#),
                    &[],
                ),
                Some(("orders", QueueAction::Send, "hello")),
            ),
            (
                "an action this emulator does not perform",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:path/{ACCOUNT}/orders"),
                    Some("Action=PurgeQueue"),
                    &[],
                ),
                None,
            ),
            (
                "no action named anywhere",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:path/{ACCOUNT}/orders"),
                    None,
                    &[],
                ),
                None,
            ),
            (
                "the action form naming no queue",
                integration(
                    "arn:aws:apigateway:us-east-1:sqs:action/SendMessage",
                    Some("MessageBody=$input.body"),
                    &[],
                ),
                None,
            ),
            (
                "a form that is neither path nor action",
                integration(
                    &format!("arn:aws:apigateway:us-east-1:sqs:{ACCOUNT}/orders"),
                    Some("Action=SendMessage"),
                    &[],
                ),
                None,
            ),
        ];
        for (case, integration, expected) in &cases {
            let read = addressed_queue(integration, "us-east-1", &request("hello"))
                .unwrap_or_else(|| panic!("case {case:?}: the uri names an Amazon SQS call"));
            match expected {
                Some((queue, action, body)) => {
                    let call = read
                        .as_ref()
                        .unwrap_or_else(|error| panic!("case {case:?}: unplaced: {:?}", error.reason));
                    assert_eq!(
                        (
                            call.queue.as_str(),
                            call.action,
                            call.parameters
                                .get(MESSAGE_BODY_PARAMETER)
                                .map(String::as_str)
                                .unwrap_or_default()
                        ),
                        (*queue, *action, *body),
                        "case {case:?}: expected {queue} {action:?} with body {body:?}, got {call:?}"
                    );
                }
                None => assert!(
                    read.is_err(),
                    "case {case:?}: expected a configuration this stage cannot serve, got {read:?}"
                ),
            }
        }
    }

    #[test]
    fn an_integration_naming_another_service_is_not_one_of_these_calls() {
        // Rows differ along WHY the uri names no Amazon SQS call: another service's
        // subdomain, another vendor, and a value that is no ARN at all.
        let cases = [
            ("another service's path", "arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt"),
            ("an ARN of another vendor", "arn:aws:sqs:us-east-1:000000000000:orders"),
            ("a formed URL rather than an ARN", "https://example.test/x"),
        ];
        for (case, uri) in &cases {
            let read = addressed_queue(&integration(uri, None, &[]), "us-east-1", &request("x"));
            assert!(
                read.is_none(),
                "case {case:?}: expected the uri to name none of these calls, got {read:?}"
            );
        }
    }

    #[test]
    fn a_result_document_carries_the_members_the_service_answered() {
        // Rows differ along what the action answered: every member present, some absent,
        // and an action with no result member at all.
        let send = answered_document(
            QueueAction::Send,
            Some(members(&[
                ("MD5OfMessageBody", Some("d41d8cd98f00b204e9800998ecf8427e")),
                ("MD5OfMessageAttributes", None),
                ("MessageId", Some("m-1")),
            ])),
        )
        .expect("the document is within the payload bound");
        let read: ::serde_json::Value =
            ::serde_json::from_slice(&send.body).expect("the document is JSON");
        let result = &read["SendMessageResponse"]["SendMessageResult"];
        assert_eq!(
            result["MD5OfMessageBody"],
            ::serde_json::json!("d41d8cd98f00b204e9800998ecf8427e"),
            "case {:?}: expected the digest the service answered, got {read}",
            "a send answers its result members"
        );
        assert_eq!(
            result["MD5OfMessageAttributes"],
            ::serde_json::Value::Null,
            "case {:?}: expected a member the service answered none for to be null, got {read}",
            "a send answers its result members"
        );
        assert!(
            read["SendMessageResponse"]["ResponseMetadata"]["RequestId"].is_string(),
            "case {:?}: expected the answer to name the request, got {read}",
            "a send answers its result members"
        );
        let deleted = answered_document(QueueAction::Delete, None)
            .expect("the document is within the payload bound");
        let read: ::serde_json::Value =
            ::serde_json::from_slice(&deleted.body).expect("the document is JSON");
        assert!(
            read["DeleteMessageResponse"]["DeleteMessageResult"].is_null()
                && read["DeleteMessageResponse"]["ResponseMetadata"].is_object(),
            "case {:?}: expected the metadata alone, got {read}",
            "a delete answers no result member"
        );
    }

    #[test]
    fn a_refusal_names_the_queue_the_call_addressed() {
        let queue = QueueCall {
            partition: "aws".to_string(),
            region: "us-east-1".to_string(),
            account: ACCOUNT.to_string(),
            queue: "orders".to_string(),
            action: QueueAction::Send,
            parameters: BTreeMap::new(),
        };
        let denied = access_denied(&queue);
        let read: ::serde_json::Value =
            ::serde_json::from_slice(&denied.body).expect("the document is JSON");
        assert_eq!(
            denied.status, ACCESS_DENIED_STATUS,
            "case {:?}: expected the status the api refuses an unauthorized call with, got {}",
            "a withheld call", denied.status
        );
        assert_eq!(
            read["Error"]["Code"],
            ::serde_json::json!(ACCESS_DENIED_CODE),
            "case {:?}: expected the service's own code, got {read}",
            "a withheld call"
        );
        assert_eq!(
            read["Error"]["Type"],
            ::serde_json::json!(SENDER_FAULT),
            "case {:?}: expected the fault to be the caller's, got {read}",
            "a withheld call"
        );
        assert_eq!(
            read["Error"]["Message"],
            ::serde_json::json!(format!(
                "Access to the resource {} is denied.",
                queue.queue_url()
            )),
            "case {:?}: expected the sentence naming the queue, got {read}",
            "a withheld call"
        );
        assert_eq!(
            queue.queue_arn(),
            format!("arn:aws:sqs:us-east-1:{ACCOUNT}:orders"),
            "case {:?}: expected the queue's own ARN, got {}",
            "a withheld call",
            queue.queue_arn()
        );
    }
}
