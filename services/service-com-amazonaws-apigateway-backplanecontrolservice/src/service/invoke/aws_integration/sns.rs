// SPDX-License-Identifier: CC0-1.0

//! The `AWS` integration whose `uri` names AMAZON SNS: how a method request becomes one
//! topic action, made in process.
//!
//! `Integration$uri` gives the form that action is named in —
//! `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/{service_api}` — and
//! Amazon SNS's own api is a QUERY api (`awsQuery`,
//! `smithy_query(query="service_info",model_path="services/
//! service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
//! com-amazonaws-sns-amazonsimplenotificationservice.json")`): a request names its action in
//! an `Action` field and that action's inputs in the other fields of an
//! `application/x-www-form-urlencoded` body, which is why an integration to a topic carries a
//! request template that renders that form. Those fields are read from the three places an
//! `AWS` integration puts them — the `uri`'s own `action/{Action}&{name}={value}` pairs, the
//! query string the integration maps, and the rendered request template — through the one
//! reading of that wire form this module holds ([`super::query_form`]).
//!
//! WHICH call they name is this module's own:
//!
//! * the ACTION is the `action` form's own name, else the `Action` field the integration
//!   request carries;
//! * the TOPIC is the `TopicArn` field of that request, else its `TargetArn`
//!   (`PublishInput$TopicArn` is "the topic you want to publish to", and "if you don't
//!   specify a value for the TopicArn parameter, you must specify a value for the
//!   PhoneNumber or TargetArn parameters"), which is the resource the call is authorized on;
//! * the `path` form addresses the service's own REST path, and this api publishes NONE —
//!   every action of it is reached at the root — so that form names the root and nothing
//!   under it, and a `uri` naming a path under it is a configuration this stage cannot serve.
//!
//! The call is then made as that api's own wire form carries it: the fields this request
//! named, written back out as a form body beside the `Action` and the api `Version`, posted
//! to the service's root THROUGH THE EMULATOR this service runs within, over the in-process
//! transport `OperationContext::callback` hands a handler ([`super::answered_in_process`]).
//! Nothing is invented and no connection is opened, and the fields travel exactly as the
//! integration request spelled them — so an attribute an integration sets on a publish
//! (`MessageAttributes.entry.{n}.Name`, the form the `MessageAttributeMap` member's entries
//! take on this wire) reaches the topic as that request named it, and the subscribers of that
//! topic are delivered to by the topic's own service. What the method response carries is
//! what Amazon SNS answered in process — its status, its media type and its bytes, refusals
//! included — for the integration response to map, so neither the document of a success nor
//! the envelope of a refusal is written here.
//!
//! Which ACTIONS are performed is therefore not a list this module keeps: an action the `uri`
//! or the integration request names is the call that is made, and an action that api does not
//! publish is refused BY IT, in its own words, rather than by a second reading of its
//! surface here.
//!
//! Who the call is made as — the role `Integration$credentials` names — and what a role that
//! may not make it earns is the one rule the parent module states for every `AWS`
//! integration, and this module only names the action it is authorized as
//! ([`TopicCall::iam_action`]), the resource it is authorized on ([`TopicCall::topic_arn`])
//! and the refusal ([`access_denied`]).

use std::collections::BTreeMap;

use ::smithy_server_runtime::OperationContext;

use crate::ops::types::GetIntegrationOutput;

use super::super::{MethodRequest, UnservedIntegration};
use super::query_form;
use super::{
    named_service, substituted, unanswered, Answered, ACCESS_DENIED_STATUS, ACTION_FORM_PREFIX,
    APIGATEWAY_VENDOR, PATH_FORM_PREFIX,
};

/// The subdomain field that names Amazon SNS, from the form `Integration$uri` documents.
const SNS_SUBDOMAIN: &str = "sns";

/// What Amazon SNS is called in the sentence an integration that reached no answer carries.
const SNS_SERVICE_NAME: &str = "Amazon SNS";

/// The version of the api a query request names itself against, which every `awsQuery`
/// request carries: this service's model version.
const SNS_API_VERSION: &str = "2010-03-31";

/// The XML namespace Amazon SNS answers its documents under (`xmlNamespace`).
const SNS_XML_NAMESPACE: &str = "http://sns.amazonaws.com/doc/2010-03-31/";

/// The media type a query api's request body carries.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The media type an `awsQuery` answer is XML under.
const XML_CONTENT_TYPE: &str = "text/xml";

/// The field that names the action, which is how a query api carries one.
const ACTION_PARAMETER: &str = "Action";

/// The field that names the api version.
const VERSION_PARAMETER: &str = "Version";

/// `PublishInput$TopicArn`, the field a topic action names its topic in — and the member
/// every other action of a topic names it in too.
const TOPIC_ARN_PARAMETER: &str = "TopicArn";

/// `PublishInput$TargetArn`, the destination a publish names when it names no topic.
const TARGET_ARN_PARAMETER: &str = "TargetArn";

/// The namespace Amazon SNS publishes its actions under: `SNS:<ApiName>` ("Actions defined
/// by Amazon Simple Notification Service", IAM Service Authorization Reference), which is the
/// ONE spelling this tree holds for them — the same one this emulator's SNS service words its
/// own refusals with.
const SNS_ACTION_PREFIX: &str = "SNS";

/// The resource an access request names when the call addresses no single topic — a publish
/// to a phone number, say — which only a grant on every resource covers.
const ANY_RESOURCE: &str = "*";

/// The code Amazon SNS refuses an unauthorized call under: `AuthorizationErrorException`
/// "indicates that the user has been denied access to the requested resource", whose
/// `awsQueryError` code is `AuthorizationError` and whose status is 403
/// (`smithy_query(query="shape",shape="AuthorizationErrorException",model_path="services/
/// service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
/// com-amazonaws-sns-amazonsimplenotificationservice.json")`).
const AUTHORIZATION_ERROR_CODE: &str = "AuthorizationError";

/// The `Type` of an `awsQuery` error document whose fault is the CALLER's, which a refusal
/// of a call the caller may not make is.
const SENDER_FAULT: &str = "Sender";

/// What an `AWS` integration naming Amazon SNS addresses, as its `uri` and the integration
/// request resolve against this call: the Region of the endpoint the call is made at, the
/// action, the topic it addresses, and the fields that action's inputs travel in.
#[derive(Debug)]
pub(super) struct TopicCall {
    /// The Region the integration ARN names, or the api's own when it names none.
    region: String,
    /// The action the call names, by the operation's own name.
    action: String,
    /// The topic the integration request addresses, empty for a request that names none.
    topic: String,
    /// The fields of the integration request, by name: every parameter of the query form
    /// this call carries, whichever of the three places it came from.
    parameters: BTreeMap<String, String>,
}

impl TopicCall {
    /// The Region of the service endpoint the call is made at.
    pub(super) fn region(&self) -> &str {
        &self.region
    }

    /// The IAM action a call of it is authorized by: `SNS:{Action}`, the action the request
    /// names under the namespace that service publishes its actions in.
    pub(super) fn iam_action(&self) -> String {
        format!("{SNS_ACTION_PREFIX}:{}", self.action)
    }

    /// The ARN of the topic this call addresses, which is the resource an identity-based
    /// policy grants the action on — the value the integration request named, because a topic
    /// is named by its own ARN on this wire.
    ///
    /// A request that names no topic addresses no single resource of its own, so the access
    /// request is over every resource ([`ANY_RESOURCE`]).
    pub(super) fn topic_arn(&self) -> String {
        match self.topic.is_empty() {
            true => ANY_RESOURCE.to_string(),
            false => self.topic.clone(),
        }
    }
}

/// What Amazon SNS answers a call the integration's role may not make: HTTP 403 with the
/// `AuthorizationError` code, in the `ErrorResponse` envelope that api answers a query
/// request's failure in, carrying the sentence IAM words an unauthorized call with — the
/// principal the call would have been made as, the action it asked for, the topic it
/// addressed and the clause closing an implicit deny, through the tree's one formatter for
/// that sentence ([`::service_common::iam_authorization::not_authorized_by_identity_policy`]).
///
/// `principal` is the role the call would have been made under.
pub(super) fn access_denied(call: &TopicCall, principal: &str) -> Answered {
    let action = call.iam_action();
    let message = ::service_common::iam_authorization::not_authorized_by_identity_policy(
        principal,
        &action,
        &call.topic_arn(),
        &::service_common::iam_authorization::no_identity_policy_allows(&action),
    );
    // Every value written into the document is escaped for XML by the maintained coder, so a
    // role ARN or a topic ARN a caller configured cannot end an element: an IAM path admits
    // every printable character, `<` and `&` among them.
    let element = |name: &str, value: &str| {
        format!(
            "<{name}>{}</{name}>",
            ::quick_xml::escape::escape(value)
        )
    };
    let body = format!(
        "<ErrorResponse xmlns=\"{SNS_XML_NAMESPACE}\"><Error>{fault}{code}{sentence}</Error>\
         {request}</ErrorResponse>",
        fault = element("Type", SENDER_FAULT),
        code = element("Code", AUTHORIZATION_ERROR_CODE),
        sentence = element("Message", &message),
        request = element(
            "RequestId",
            &::service_common::query_error_envelope::new_request_id()
        ),
    );
    Answered {
        status: ACCESS_DENIED_STATUS,
        content_type: Some(XML_CONTENT_TYPE.to_string()),
        body: body.into_bytes(),
    }
}

/// What `integration`'s `uri` addresses when it names an Amazon SNS topic action:
///
/// * `None` when the `uri` names no Amazon SNS call, which leaves it to another reader;
/// * `Some(Err(…))` when it DOES name Amazon SNS and this call cannot be placed against it,
///   which is a configuration this stage cannot serve — or an integration request no
///   template of it maps, which is the one refusal a method request earns here;
/// * `Some(Ok(call))` for the call to make.
pub(super) fn addressed_topic(
    integration: &GetIntegrationOutput,
    default_region: &str,
    call: &MethodRequest,
) -> Option<Result<TopicCall, UnservedIntegration>> {
    let uri = integration.uri.as_deref()?;
    let arn = ::service_common::arn::parse(uri)?;
    if arn.vendor != APIGATEWAY_VENDOR || named_service(arn.account) != SNS_SUBDOMAIN {
        return None;
    }
    Some(resolved_topic(integration, &arn, default_region, call))
}

/// The call the integration ARN `arn` and the integration request together name.
fn resolved_topic(
    integration: &GetIntegrationOutput,
    arn: &::service_common::arn::Arn<'_>,
    default_region: &str,
    call: &MethodRequest,
) -> Result<TopicCall, UnservedIntegration> {
    let region = match arn.region.is_empty() {
        true => default_region,
        false => arn.region,
    };
    let resource =
        substituted(arn.resource, integration, &call.inputs).map_err(|reason| unanswered(&reason))?;
    // The PATH form names the service's own REST path, and this api publishes none: every
    // action of it is reached at the root, so the form addresses that root and nothing under
    // it.
    let named_root = match resource.strip_prefix(PATH_FORM_PREFIX) {
        Some(path) => {
            if !path.trim_matches('/').is_empty() {
                return Err(unanswered(&format!(
                    "the integration's uri addresses the Amazon SNS path form at {path:?}, and \
                     that api publishes no resource path: every action of it is reached at the \
                     service's root"
                )));
            }
            true
        }
        None => false,
    };
    let (named_action, fixed) = match resource.strip_prefix(ACTION_FORM_PREFIX) {
        Some(named) => match named.split_once('&') {
            Some((name, pairs)) => (Some(name.to_string()), pairs),
            None => (Some(named.to_string()), ""),
        },
        None => (None, ""),
    };
    if !named_root && named_action.is_none() {
        return Err(unanswered(
            "the integration's uri addresses neither the path nor the action form of an \
             Amazon SNS call",
        ));
    }
    // The three places an `AWS` integration carries the fields of a query api's request, in
    // the order a later one overrides an earlier ([`query_form`]).
    let parameters = query_form::carried_parameters(integration, call, fixed)?;

    let action = named_action
        .or_else(|| parameters.get(ACTION_PARAMETER).cloned())
        .ok_or_else(|| {
            unanswered(
                "the integration's uri names no Amazon SNS action and its integration request \
                 carries no Action field, so it names no call",
            )
        })?;
    // The action travels in the request as the api's own wire form names one, so a value
    // that is no action name at all places no call rather than being sent as a field.
    if action.is_empty() || !action.chars().all(char::is_alphanumeric) {
        return Err(unanswered(&format!(
            "the integration request names {action:?}, which is no Amazon SNS action"
        )));
    }
    let topic = parameters
        .get(TOPIC_ARN_PARAMETER)
        .or_else(|| parameters.get(TARGET_ARN_PARAMETER))
        .map(|named| named.trim().to_string())
        .unwrap_or_default();
    Ok(TopicCall {
        region: region.to_string(),
        action,
        topic,
        parameters,
    })
}

/// Make the call `topic` addresses against Amazon SNS, in process, and report what it
/// answered.
///
/// The request is the one that api takes: the fields this call named written back out as the
/// `application/x-www-form-urlencoded` body a query request carries, beside the `Action` it
/// names and the api `Version` every such request states, posted to the service's root. A
/// field the integration request already carries for either of those stands, because the
/// configuration names the call and this adds only what it left unstated.
///
/// `Err` only where there is no answer FROM THE SERVICE to report at all; every answer the
/// service DID give, refusals included, is reported as [`Answered`] and mapped.
pub(super) async fn answered(
    ctx: &OperationContext<'_>,
    topic: &TopicCall,
) -> Result<Answered, UnservedIntegration> {
    let mut fields = topic.parameters.clone();
    fields.insert(ACTION_PARAMETER.to_string(), topic.action.clone());
    fields
        .entry(VERSION_PARAMETER.to_string())
        .or_insert_with(|| SNS_API_VERSION.to_string());
    let host = format!("{SNS_SUBDOMAIN}.{}.amazonaws.com", topic.region);
    let built = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header(::http::header::HOST, &host)
        .header(::http::header::CONTENT_TYPE, FORM_CONTENT_TYPE)
        .body(query_form::form_body(&fields));
    let request = built.map_err(|error| {
        ::tracing::warn!(%error, "the integration's Amazon SNS call does not render an HTTP \
                                  request");
        unanswered(
            "the integration's uri does not render a request to Amazon SNS, so no call was made",
        )
    })?;
    super::answered_in_process(
        ctx,
        request,
        SNS_SUBDOMAIN,
        &topic.region,
        SNS_SERVICE_NAME,
        Some(XML_CONTENT_TYPE),
    )
    .await
}

#[cfg(test)]
mod tests {
    //! What an integration `uri` and an integration request together address, which is the
    //! one part of this module that needs neither a backend nor a role to decide.

    use super::*;

    use crate::service::mapping_template::TemplateInputs;

    /// The topic every row below addresses, by the ARN a topic is named by on this wire.
    const TOPIC: &str = "arn:aws:sns:us-east-1:000000000000:orders";

    /// An `AWS` integration to Amazon SNS: `uri`, the request template that renders the
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
                [(
                    "application/x-www-form-urlencoded".to_string(),
                    body.to_string(),
                )]
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

    /// One row of the table below: the case, the integration whose `uri` and request
    /// template are read, and the `(action, topic, message)` the call it places addresses —
    /// `None` for a configuration this stage cannot serve.
    type Row = (
        &'static str,
        GetIntegrationOutput,
        Option<(&'static str, &'static str, &'static str)>,
    );

    #[test]
    fn the_uri_and_the_integration_request_decide_which_topic_call_is_made() {
        // Rows differ along what the rule generalises over: WHICH of the three places the
        // action and its parameters come from, the FORM the uri names the call in, and the
        // action named.
        let cases: Vec<Row> = vec![
            (
                "the action form with the topic and the message in the rendered body",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:action/Publish",
                    Some(&format!(
                        "TopicArn={}&Message=$input.body",
                        TOPIC.replace(':', "%3A")
                    )),
                    &[],
                ),
                Some(("Publish", TOPIC, "hello")),
            ),
            (
                "the path form, which names the service's root, with the action in the body",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:path//",
                    Some(&format!(
                        "Action=Publish&TopicArn={}&Message=$input.body",
                        TOPIC.replace(':', "%3A")
                    )),
                    &[],
                ),
                Some(("Publish", TOPIC, "hello")),
            ),
            (
                "the action form whose own pairs fix the topic",
                integration(
                    &format!(
                        "arn:aws:apigateway:us-east-1:sns:action/GetTopicAttributes&TopicArn={}",
                        TOPIC.replace(':', "%3A")
                    ),
                    None,
                    &[],
                ),
                Some(("GetTopicAttributes", TOPIC, "")),
            ),
            (
                "parameters mapped onto the integration request's query string",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:path/",
                    None,
                    &[
                        ("integration.request.querystring.Action", "'Publish'"),
                        (
                            "integration.request.querystring.TopicArn",
                            &format!("'{TOPIC}'"),
                        ),
                        (
                            "integration.request.querystring.Message",
                            "method.request.querystring.note",
                        ),
                    ],
                ),
                Some(("Publish", TOPIC, "from the query string")),
            ),
            (
                "the body overriding a parameter the uri fixes",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:action/Publish&TopicArn=unset",
                    Some(&format!(
                        "TopicArn={}&Message=$input.body",
                        TOPIC.replace(':', "%3A")
                    )),
                    &[],
                ),
                Some(("Publish", TOPIC, "hello")),
            ),
            (
                "a JSON body, which is the other wire form of the same inputs",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:action/Publish",
                    Some(&format!(
                        r#"{{"TopicArn":"{TOPIC}","Message":"$input.body"}}"#
                    )),
                    &[],
                ),
                Some(("Publish", TOPIC, "hello")),
            ),
            (
                "the uri carrying a subdomain in front of the service",
                integration(
                    "arn:aws:apigateway:us-east-1:topic.sns:action/Publish",
                    Some(&format!(
                        "TopicArn={}&Message=$input.body",
                        TOPIC.replace(':', "%3A")
                    )),
                    &[],
                ),
                Some(("Publish", TOPIC, "hello")),
            ),
            (
                "a publish naming a destination rather than a topic",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:action/Publish",
                    Some("TargetArn=arn%3Aaws%3Asns%3Aus-east-1%3A000000000000%3Aendpoint/APNS/app/1\
                          &Message=$input.body"),
                    &[],
                ),
                Some((
                    "Publish",
                    "arn:aws:sns:us-east-1:000000000000:endpoint/APNS/app/1",
                    "hello",
                )),
            ),
            (
                "a publish naming no destination at all, which addresses no single topic",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:action/Publish",
                    Some("PhoneNumber=%2B15555550100&Message=$input.body"),
                    &[],
                ),
                Some(("Publish", "", "hello")),
            ),
            (
                "the path form naming a path under the service's root, which it publishes none of",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:path/orders",
                    Some("Action=Publish&Message=$input.body"),
                    &[],
                ),
                None,
            ),
            (
                "no action named anywhere",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:path//",
                    Some(&format!("TopicArn={}", TOPIC.replace(':', "%3A"))),
                    &[],
                ),
                None,
            ),
            (
                "a value that is no action name",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:path//",
                    Some("Action=Publish%20Message"),
                    &[],
                ),
                None,
            ),
            (
                "a form that is neither path nor action",
                integration(
                    "arn:aws:apigateway:us-east-1:sns:topic/orders",
                    Some("Action=Publish"),
                    &[],
                ),
                None,
            ),
        ];
        for (case, integration, expected) in &cases {
            let read = addressed_topic(integration, "us-east-1", &request("hello"))
                .unwrap_or_else(|| panic!("case {case:?}: the uri names an Amazon SNS call"));
            match expected {
                Some((action, topic, message)) => {
                    let call = read.as_ref().unwrap_or_else(|error| {
                        panic!("case {case:?}: unplaced: {:?}", error.reason)
                    });
                    assert_eq!(
                        (
                            call.action.as_str(),
                            call.topic.as_str(),
                            call.parameters
                                .get("Message")
                                .map(String::as_str)
                                .unwrap_or_default()
                        ),
                        (*action, *topic, *message),
                        "case {case:?}: expected {action} on {topic:?} with message \
                         {message:?}, got {call:?}"
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
        // Rows differ along WHY the uri names no Amazon SNS call: another service's
        // subdomain, another vendor, and a value that is no ARN at all.
        let cases = [
            (
                "another service's path",
                "arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt",
            ),
            (
                "an ARN of another vendor",
                "arn:aws:sns:us-east-1:000000000000:orders",
            ),
            ("a formed URL rather than an ARN", "https://example.test/x"),
        ];
        for (case, uri) in &cases {
            let read = addressed_topic(&integration(uri, None, &[]), "us-east-1", &request("x"));
            assert!(
                read.is_none(),
                "case {case:?}: expected the uri to name none of these calls, got {read:?}"
            );
        }
    }

    /// A `TopicCall` of `action` on `topic`, as [`resolved_topic`] builds one.
    fn call(action: &str, topic: &str) -> TopicCall {
        TopicCall {
            region: "us-east-1".to_string(),
            action: action.to_string(),
            topic: topic.to_string(),
            parameters: BTreeMap::new(),
        }
    }

    #[test]
    fn the_call_is_authorized_as_its_action_on_the_topic_it_names() {
        // Rows differ along the action named and whether the request addresses a topic at
        // all.
        let rows: &[(&str, &str, &str, &str, &str)] = &[
            ("a publish", "Publish", TOPIC, "SNS:Publish", TOPIC),
            (
                "a read of the topic's attributes",
                "GetTopicAttributes",
                TOPIC,
                "SNS:GetTopicAttributes",
                TOPIC,
            ),
            (
                "a request that names no topic",
                "Publish",
                "",
                "SNS:Publish",
                ANY_RESOURCE,
            ),
        ];
        for (case, action, topic, expected_action, expected_resource) in rows {
            let call = call(action, topic);
            assert_eq!(
                (call.iam_action().as_str(), call.topic_arn().as_str()),
                (*expected_action, *expected_resource),
                "case {case:?}: expected {expected_action} on {expected_resource}, got {} on {}",
                call.iam_action(),
                call.topic_arn()
            );
        }
    }

    #[test]
    fn a_refusal_names_the_principal_the_action_and_the_topic() {
        // The principal carries a character XML gives meaning to, which the document has to
        // escape rather than emit: an IAM path admits every printable character.
        let principal = "arn:aws:iam::000000000000:role/a&b<c";
        let denied = access_denied(&call("Publish", TOPIC), principal);
        let body = String::from_utf8(denied.body.clone()).expect("the document is UTF-8");
        assert_eq!(
            denied.status, ACCESS_DENIED_STATUS,
            "case {:?}: expected the status this api refuses an unauthorized call with, got {}",
            "a withheld call", denied.status
        );
        assert!(
            body.contains(&format!("<Code>{AUTHORIZATION_ERROR_CODE}</Code>"))
                && body.contains(&format!("<Type>{SENDER_FAULT}</Type>")),
            "case {:?}: expected this api's own code and fault side, got {body:?}",
            "a withheld call"
        );
        assert!(
            body.contains("is not authorized to perform: SNS:Publish on resource:")
                && body.contains(TOPIC)
                && body.contains("because no identity-based policy allows the SNS:Publish action"),
            "case {:?}: expected the sentence naming the action and the topic, got {body:?}",
            "a withheld call"
        );
        assert!(
            body.contains("role/a&amp;b&lt;c") && !body.contains("role/a&b<c"),
            "case {:?}: expected the principal escaped for the document, got {body:?}",
            "a withheld call"
        );
    }
}
