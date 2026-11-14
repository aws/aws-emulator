// SPDX-License-Identifier: CC0-1.0

//! The `AWS_PROXY` integration: how a method request is served by INVOKING the Lambda
//! function its `uri` names, in process, and answering what the handler returned.
//!
//! `IntegrationType` gives `AWS_PROXY` "for integrating the API method request with the
//! Lambda function-invoking action with the client request passed through as-is", and
//! `Integration$uri` gives the form that action is named in: "for integrations of the
//! `aws` type … `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/\
//! {service_api}`", which for the function-invoking action is the Lambda data-plane path
//! `path/{version}/functions/{function-arn}/invocations`. So the `uri` names ONE function,
//! and the call is made against it THROUGH THIS EMULATOR, over the in-process transport
//! `OperationContext::callback` hands a handler: no connection is opened, and what the
//! method request is answered with is what the function's own run produced.
//!
//! # What is passed through, and what comes back
//!
//! A proxy integration maps nothing: "the client request [is] passed through as-is". API
//! Gateway hands the function ONE event document describing the whole method request — the
//! resource template it matched and the path it arrived at, the verb, the headers and query
//! string in both their single-valued and multi-valued forms, the path parameters the
//! template bound, the stage's variables, the call's context, and the body — and takes the
//! function's returned document as the response: `statusCode`, `headers`,
//! `multiValueHeaders`, `body` and `isBase64Encoded`. Neither direction consults an
//! integration request template, an integration response or a method response: a proxy
//! integration has none to consult, which is what makes it a proxy.
//!
//! Every value in the event is read off THIS call and this api's own records. Nothing is
//! invented: a member whose value this emulator does not have is left out rather than
//! filled in, and the body travels exactly as it arrived — as text where the request's bytes
//! are text, and base64-encoded with `isBase64Encoded` set where they are not, which is the
//! encoding the member documents. A name the request REPEATS is reported in both forms: every
//! value it carries in the multi-valued member, and the LAST of them in the single-valued one
//! beside it — each under the spelling HTTP capitalises a field name in
//! ([`canonical_header_name`]).
//!
//! What comes back the other way is bounded by the same records: `RestApi$binaryMediaTypes`
//! is "the list of binary media types supported by the RestApi. By default, the RestApi
//! supports only UTF-8-encoded text payloads", so a `body` the function declared
//! base64-encoded is handed to the client as the BYTES it names only where the response's
//! media type is one the API declares binary, and as the base64 TEXT it is otherwise
//! ([`proxy_response`]).
//!
//! # Who the call is made as, and what refuses it
//!
//! API Gateway invokes a function as its own service principal ([`SERVICE_PRINCIPAL`]), so
//! the grant that admits the invocation is the FUNCTION's resource-based policy — the
//! document `AddPermission` writes and `GetPolicy` reports — and it is read from Lambda
//! itself rather than kept a second time here. The decision is
//! [`::service_common::iam_authorization`], the tree's one evaluation of policy documents,
//! taken on `lambda:InvokeFunction` over the function the `uri` names with the invoking
//! METHOD's ARN as the `aws:SourceArn` such a grant is narrowed by — which is how a
//! permission added for one api, stage or path admits a call to it and no other.
//!
//! A SIMULATED invocation (`TestInvokeMethod`) is decided by the same grant: it reaches no
//! deployed stage, and AWS names that fact in the `aws:SourceArn` it is judged against rather
//! than leaving it out — the method's ARN under the stage AWS reserves for a test invocation
//! — so a function that admits API Gateway nothing for the method is not invoked from either
//! side.
//!
//! Nothing is refused on evidence this emulator does not have: a `uri` whose function Lambda
//! does not answer about at all, and an emulator with no Lambda mounted, each leave the call
//! to be made, for the reason [`super::super::resource_policy`]'s own reading gives. What
//! Lambda REPORTS is an answer rather than a silence: a function it holds no record of — the
//! record itself, or the version or alias the `uri` qualifies it by — has no policy for a
//! grant to live in, and a function whose policy it reports as absent carries none, so
//! neither is invoked and both earn the method the ONE configuration error API Gateway words
//! a missing grant with ([`INVALID_PERMISSIONS_REASON`]) — an unresolvable qualifier is
//! reported as a permissions error, not as a not-found one.
//!
//! # How an outcome becomes the method response
//!
//! * the function RAN and returned a well-formed proxy response: that response IS the method
//!   response — its status, its headers and its body, unmapped;
//! * the function ran and returned something else, or raised (the `FunctionError` signal
//!   Lambda reports): API Gateway "returns a 502 Bad Gateway" for a proxy response not in
//!   the required format, with the `{"message":"Internal server error"}` body it words such
//!   a response with, and the reason is logged ([`BAD_GATEWAY_STATUS`]);
//! * the invocation could not be MADE at all — no Lambda answered it, the `uri` names no
//!   function (or no version or alias of one) Lambda holds, or the function's policy admits
//!   no call from this method: there
//!   is no function answer to pass through, so the call is answered the way every integration
//!   this stage cannot perform is, through the api's own `API_CONFIGURATION_ERROR` gateway
//!   response ([`super::API_CONFIGURATION_ERROR`], `500` with
//!   `{"message":"Internal server error"}`);
//! * the invocation was made and the function had not answered by the integration's own
//!   `timeoutInMillis` ([`waited_for`]): the wait is the api's stored number, so a request to
//!   a deployed stage is answered whatever the function does with its own time — through the
//!   api's `INTEGRATION_TIMEOUT` gateway response ([`super::INTEGRATION_TIMEOUT`], `504`).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::base64::Engine as _;
use ::serde_json::{json, Map, Value};
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::OperationContext;

use crate::ops::put_integration::LAMBDA_PROXY_INTEGRATION_TYPE;
use crate::ops::types::GetIntegrationOutput;

use super::{IntegratedResponse, MethodRequest, UnservedIntegration, API_CONFIGURATION_ERROR,
            INTEGRATION_TIMEOUT, JSON_CONTENT_TYPE};

/// The vendor field of every AWS integration ARN, and the resource prefix of its PATH
/// form — read from the operation that VALIDATES an integration's `uri`
/// ([`crate::ops::put_integration`]), so the call that stores an ARN and this plane, which
/// performs it, agree on what one is.
use crate::ops::put_integration::{APIGATEWAY_VENDOR, PATH_FORM_PREFIX};

/// The subdomain field that names AWS Lambda, which is the service whose function-invoking
/// action this module performs.
const LAMBDA_SUBDOMAIN: &str = "lambda";

/// The segment of that path that the function's own name or ARN follows.
const FUNCTIONS_SEGMENT: &str = "/functions/";

/// The last segment of that path: Lambda's own synchronous-invocation resource.
const INVOCATIONS_SUFFIX: &str = "/invocations";

/// The service principal API Gateway invokes a function as, as a resource policy names one:
/// "for Amazon Web Services services, the principal is a domain-style identifier that the
/// service defines" (`AddPermission$Principal`).
const SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The IAM action an invocation of a function performs, which is not the operation's own
/// name: `Invoke` "requires permission for the `lambda:InvokeFunction` action".
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The condition key a grant narrowed to ONE invoking resource names the source by, which
/// for an api's method is that method's `execute-api` ARN.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key a grant narrowed to the account that OWNS the invoking api names it by.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The status API Gateway answers a proxy integration whose function did not return a
/// well-formed proxy response with: "If the response is not in this format, API Gateway
/// returns a 502 Bad Gateway error response."
const BAD_GATEWAY_STATUS: u16 = 502;

/// The sentence that response carries, which is the one API Gateway words every
/// server-side failure of a call it could not pass on with.
const BAD_GATEWAY_MESSAGE: &str = "Internal server error";

/// The member the function's returned document carries the response status in.
const STATUS_CODE_MEMBER: &str = "statusCode";

/// The member it carries single-valued response headers in.
const HEADERS_MEMBER: &str = "headers";

/// The member it carries multi-valued response headers in.
const MULTI_VALUE_HEADERS_MEMBER: &str = "multiValueHeaders";

/// The member it carries the response body in, always as text.
const BODY_MEMBER: &str = "body";

/// The member that says the body is base64-encoded, in both directions.
const BASE64_MEMBER: &str = "isBase64Encoded";

// The response header whose value is the method response's media type rather than one of
// its headers: the parent module's one reading of the name, so the two sides of this plane
// compare it one way.
use super::CONTENT_TYPE_HEADER;

/// The `$context` member path an authorizer's values are published under, which the event
/// carries as `requestContext.authorizer`.
const AUTHORIZER_MEMBER: &str = super::custom_authorizer::AUTHORIZER_MEMBER;

/// The member of it a user-pool authorizer's token claims are published under, which the
/// event carries as `requestContext.authorizer.claims`.
const CLAIMS_MEMBER: &str = "claims";

/// The request header a proxy integration reads the caller's address off where the request
/// reached the emulator through a forwarder; a request carrying none names no address, so
/// the event carries none either. The ONE spelling, shared with the call's own `$context`.
const FORWARDED_FOR_HEADER: &str = super::FORWARDED_FOR_HEADER;

/// The request header the caller's user agent travels in; see [`FORWARDED_FOR_HEADER`].
const USER_AGENT_HEADER: &str = super::USER_AGENT_HEADER;

/// The request headers the event does NOT carry over from the caller's request: the two the
/// caller's own SigV4 CREDENTIAL travels in.
///
/// The same withholding the pass-through path makes, for the same reason
/// ([`super::HEADERS_NOT_PASSED_THROUGH`]): a credential the client presented to this
/// endpoint authenticates nothing at the backend the integration names, and handing it to
/// code the api's developer deployed would make every invoke a way to collect the caller's
/// credential. What API Gateway's own integration credential is is
/// `Integration$credentials` — a record of the integration — and the invocation here is made
/// under the transport's, never the caller's. Every OTHER header the caller sent travels in
/// the event, which is what "the client request passed through as-is" means.
const HEADERS_WITHHELD_FROM_THE_EVENT: &[&str] = &["authorization", "x-amz-security-token"];

/// The most bytes one function answer may carry into a method response: API Gateway's
/// published payload quota is 10 MB, so an answer past it is one this stage cannot pass on
/// rather than one buffered without bound.
const MAX_PROXY_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

/// How long this call waits for the function's answer: the integration's own
/// `timeoutInMillis`, "custom timeout between 50 and 29,000 milliseconds"
/// (`Integration$timeoutInMillis`), so the wait is a stored number of this api's and never
/// one a request chose. A stored value outside the range `PutIntegration` accepts is read as
/// the DEFAULT that member documents, which is what a record carrying none means.
///
/// A backend that has not answered by then has not answered the CALL: API Gateway stops
/// waiting and answers through the api's own `INTEGRATION_TIMEOUT` gateway response, which
/// is what keeps a request to a deployed stage bounded whatever the function does with its
/// own time.
fn waited_for(integration: &GetIntegrationOutput) -> Duration {
    let stored = integration.timeout_in_millis;
    let millis = if (crate::ops::put_integration::MIN_TIMEOUT_IN_MILLIS
        ..=crate::ops::put_integration::MAX_TIMEOUT_IN_MILLIS)
        .contains(&stored)
    {
        stored
    } else {
        crate::ops::put_integration::DEFAULT_TIMEOUT_IN_MILLIS
    };
    Duration::from_millis(u64::try_from(millis).unwrap_or_default())
}

/// What a PROXY integration's event carries about the call beyond what a mapping template
/// reads off it.
///
/// A proxy integration passes "the client request … through as-is", so the event names parts
/// of the request a template has no reference for: every value a repeated query parameter or
/// header carries, the verb as the CLIENT sent it (rather than the `ANY` catch-all it may
/// have matched), the `Resource` record the path resolved to, and the endpoint it arrived on.
/// Built by whatever is serving the call — the data plane off the wire, `TestInvokeMethod`
/// out of the members a caller simulates a request with — so one function sees one event
/// shape whichever side drives it.
#[derive(Debug, Default, Clone)]
pub struct ProxyInputs {
    /// The id of the `Resource` whose path the request matched.
    pub resource_id: String,
    /// The verb the CLIENT sent, which is what the event reports as the request's method.
    pub http_method: String,
    /// The path the request carried WITHIN the stage, leading `/` included.
    pub request_path: String,
    /// The endpoint host the request was addressed on, or empty where it named none.
    pub domain_name: String,
    /// Every value the request carries under each query-string name, in the order sent.
    pub multi_value_querystring: BTreeMap<String, Vec<String>>,
    /// Every value the request carries under each header name, in the order sent.
    pub multi_value_header: BTreeMap<String, Vec<String>>,
    /// The ARN of the METHOD this call addresses — the `aws:SourceArn` an `AddPermission`
    /// grant to API Gateway is narrowed by — or `None` for a call that reached no stage,
    /// which names no method for a grant to be judged against.
    pub source_arn: Option<String>,
    /// The media types the API declares BINARY — `RestApi$binaryMediaTypes`, "the list of
    /// binary media types supported by the RestApi. By default, the RestApi supports only
    /// UTF-8-encoded text payloads" — which is what decides whether a base64 body the
    /// function returned is decoded to the bytes it names ([`proxy_response`]).
    pub binary_media_types: Vec<String>,
    /// Whether the STAGE this call arrived at is tracing — `Stage$tracingEnabled`,
    /// "specifies whether active tracing with X-ray is enabled for the Stage" — which is
    /// the sampling decision the call this stage makes onward carries where the caller took
    /// none ([`super::trace_context`]). `false` for a call that reached no stage.
    pub tracing_enabled: bool,
}

/// Perform this call through the method's `AWS_PROXY` integration, or `None` when the
/// integration is not a Lambda proxy — in which case the caller performs it from the api's
/// own records ([`super::perform_integration`]).
///
/// `region` is the api's own Region, which is the endpoint the invocation is made at when the
/// integration ARN names none of its own.
pub(super) async fn performed(
    ctx: &OperationContext<'_>,
    region: &str,
    method: &crate::state::MethodEntry,
    call: &MethodRequest,
) -> Option<Result<IntegratedResponse, UnservedIntegration>> {
    let integration = method.integration.as_ref()?;
    if integration.r#type.as_deref() != Some(LAMBDA_PROXY_INTEGRATION_TYPE) {
        return None;
    }
    let uri = integration.uri.as_deref().unwrap_or_default();
    let Some((addressed_region, function)) = invoked_function(uri) else {
        return Some(Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the AWS_PROXY integration's uri does not name a Lambda function's \
                     invocation path, so there is no function to invoke"
                .to_string(),
        }));
    };
    let at = if addressed_region.is_empty() {
        region.to_string()
    } else {
        addressed_region.to_string()
    };
    Some(served(ctx, &at, function, call, waited_for(integration), integration.credentials.as_deref()).await)
}

/// The two parts of an `AWS_PROXY` `uri` that decide the invocation: the Region of the
/// Lambda endpoint it names (empty where it names none) and the function it addresses.
///
/// `None` for a value that is not the function-invoking action's path at all, which is an
/// integration this call cannot be performed through rather than an address guessed at.
///
/// `pub(super)` because an `Authorizer$authorizerUri` names its function in the same form,
/// so one reading serves both ([`super::custom_authorizer`]).
pub(super) fn invoked_function(uri: &str) -> Option<(&str, &str)> {
    // Six fields, because the SIXTH — the resource — carries the function's own ARN, whose
    // colons must stay where they are.
    let mut fields = uri.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != APIGATEWAY_VENDOR {
        return None;
    }
    let region = fields.next()?;
    if !fields.next()?.eq_ignore_ascii_case(LAMBDA_SUBDOMAIN) {
        return None;
    }
    let path = fields.next()?.strip_prefix(PATH_FORM_PREFIX)?;
    let invoked = path.strip_suffix(INVOCATIONS_SUFFIX)?;
    let (_api_version, function) = invoked.split_once(FUNCTIONS_SEGMENT)?;
    (!function.is_empty()).then_some((region, function))
}

/// The method response this call earns: the function's policy decides whether the
/// invocation is made, the run decides what it answered, and that answer IS the response.
///
/// `within` is how long the integration waits for that answer ([`waited_for`]): the wait is
/// bounded here rather than left to the function's own runtime, so the caller is answered
/// whatever the run does with its time.
async fn served(
    ctx: &OperationContext<'_>,
    region: &str,
    function: &str,
    call: &MethodRequest,
    within: Duration,
    credentials: Option<&str>,
) -> Result<IntegratedResponse, UnservedIntegration> {
    // The stage's own work begins here. The trace context it propagates onto the invocation
    // was decided when the method request was built — ONCE for the request, so an
    // authorizer's function and this integration's belong to one trace
    // ([`super::trace_context`]).
    let started = SystemTime::now();
    let propagated = &call.trace;
    let outcome = invoked(
        ctx,
        region,
        function,
        call,
        within,
        propagated.header.as_deref(),
        credentials,
    )
    .await;
    // What the stage recorded about the request it served, submitted once the outcome is
    // known — off the answer, so a submission that could not be made never changes what the
    // client is served. A stage that is not tracing, and a request the sampling decision
    // says is not recorded, record nothing at all.
    let context = &call.inputs.context;
    let served = super::trace_context::Served {
        api_id: context.get("apiId").map(String::as_str).unwrap_or_default(),
        stage: context.get("stage").map(String::as_str).unwrap_or_default(),
        function_name: function,
        status: match &outcome {
            Ok(response) => Some(response.status),
            // An integration this stage could not perform is answered through a gateway
            // response of the api's own, whose status this layer does not resolve: the
            // segment records the FAULT rather than a status nothing answered with.
            Err(_unserved) => None,
        },
        started,
        ended: SystemTime::now(),
    };
    if let Some(document) = super::trace_context::stage_segment(propagated, &call.proxy, &served) {
        ::service_common::xray_segments::report_segments(ctx, region, vec![document]).await;
    }
    outcome
}

/// The invocation itself: the function's policy decides whether it is made, the run decides
/// what it answered, and that answer IS the response.
///
/// `trace` is the tracing header the invocation carries, where this request names a chain.
async fn invoked(
    ctx: &OperationContext<'_>,
    region: &str,
    function: &str,
    call: &MethodRequest,
    within: Duration,
    trace: Option<&str>,
    credentials: Option<&str>,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let client = lambda_client(ctx, region);
    if let Some(reason) = unpermitted(ctx, region, &client, function, call, credentials).await {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason,
        });
    }
    let event = proxy_event(call);
    let payload = ::serde_json::to_vec(&event).map_err(|_| UnservedIntegration {
        refusal: API_CONFIGURATION_ERROR,
        reason: "the proxy integration's event could not be written, so no invocation was \
                 made"
            .to_string(),
    })?;
    bounded(payload.len())?;
    // The X-Ray trace context travels on the `X-Amzn-Trace-Id` header of the invocation,
    // which is where the next hop reads its own trace context off.
    let trace = trace.map(str::to_string);
    let invocation = client
        .invoke()
        .function_name(function)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(payload))
        .customize()
        .mutate_request(move |request| {
            if let Some(trace) = trace.clone() {
                // `try_insert`, not the panicking `insert`: the value is the CALLER's own
                // header text where this stage records nothing and passes it on as written,
                // so a byte the wire cannot carry in a header value must cost the request its
                // trace context rather than its response.
                if let Err(refused) = request
                    .headers_mut()
                    .try_insert(super::trace_context::TRACE_HEADER, trace)
                {
                    ::tracing::warn!(
                        error = %refused,
                        "the tracing header this request named cannot travel on the wire, so \
                         the invocation carries none"
                    );
                }
            }
        })
        .send();
    let answer = match ::tokio::time::timeout(within, invocation).await {
        Ok(answer) => answer,
        // The integration's own timeout fell due with no answer, so the backend did not
        // answer this call: the request is answered through the api's `INTEGRATION_TIMEOUT`
        // gateway response rather than held open for a run that is still going.
        Err(_elapsed) => {
            ::tracing::warn!(
                waited_ms = within.as_millis(),
                "the proxy integration's function did not answer within the integration's \
                 timeout"
            );
            return Err(UnservedIntegration {
                refusal: INTEGRATION_TIMEOUT,
                reason: format!(
                    "the AWS_PROXY integration's function did not answer within the \
                     integration's timeoutInMillis of {}, so the call has no function \
                     answer to pass through",
                    within.as_millis()
                ),
            });
        }
    };
    let answered = match answer {
        Ok(answered) => answered,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the proxy integration's function was not invoked"
            );
            return Err(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: "the AWS_PROXY integration's function did not answer the \
                         invocation, so there is no function answer to pass through"
                    .to_string(),
            });
        }
    };
    // "FunctionError: if present, indicates that an error occurred during function
    // execution" — a run that errored returned no proxy response, which API Gateway answers
    // the same way it answers one it cannot read.
    if let Some(signal) = answered.function_error() {
        ::tracing::warn!(
            function_error = %signal,
            "the proxy integration's function errored, so it returned no proxy response"
        );
        return Ok(bad_gateway());
    }
    let returned = answered
        .payload()
        .map(|payload| payload.as_ref().to_vec())
        .unwrap_or_default();
    bounded(returned.len())?;
    match proxy_response(&returned, &call.proxy.binary_media_types) {
        Ok(response) => Ok(response),
        Err(reason) => {
            ::tracing::warn!(
                reason = %reason,
                "the proxy integration's function returned no well-formed proxy response"
            );
            Ok(bad_gateway())
        }
    }
}

/// `Ok` while `bytes` is within what one proxy payload may carry
/// ([`MAX_PROXY_PAYLOAD_BYTES`]).
fn bounded(bytes: usize) -> Result<(), UnservedIntegration> {
    if bytes > MAX_PROXY_PAYLOAD_BYTES {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the proxy integration's payload of {bytes} bytes is larger than the \
                 {MAX_PROXY_PAYLOAD_BYTES} bytes one invocation carries"
            ),
        });
    }
    Ok(())
}

/// The response API Gateway answers a proxy integration whose function returned no
/// well-formed proxy response with.
fn bad_gateway() -> IntegratedResponse {
    IntegratedResponse {
        status: BAD_GATEWAY_STATUS,
        content_type: JSON_CONTENT_TYPE.to_string(),
        body: json!({ "message": BAD_GATEWAY_MESSAGE }).to_string().into_bytes(),
        headers: BTreeMap::new(),
    }
}

/// Why this invocation is not admitted, or `None` when it is — and when nothing about it
/// could be shown, which refuses nothing.
///
/// WHICH records decide it is what `Integration$credentials` chooses: an integration naming a
/// ROLE has the call made under that role, so the role's own permissions admit it
/// ([`role_unpermitted`]); one naming none has it made as this service's own principal, so the
/// FUNCTION's resource-based policy admits it. The phrase a refusal carries is the one AWS
/// states in the execution log of a method whose integration's function admits it nothing
/// ([`INVALID_PERMISSIONS_REASON`]).
async fn unpermitted(
    ctx: &OperationContext<'_>,
    region: &str,
    client: &::aws_sdk_lambda::Client,
    function: &str,
    call: &MethodRequest,
    credentials: Option<&str>,
) -> Option<String> {
    if let Some(role_arn) = stated_role(credentials) {
        let function_arn = resolved_function_arn(client, function).await;
        return role_unpermitted(ctx, region, role_arn, &function_arn).await;
    }
    // A call that names no method at all leaves a grant's `aws:SourceArn` nothing to be
    // judged against, so there is no decision to take about it.
    let source_arn = call.proxy.source_arn.as_deref()?;
    if invocation_admitted(client, function, source_arn).await {
        return None;
    }
    Some(INVALID_PERMISSIONS_REASON.to_string())
}

/// What API Gateway states in the execution log of a method whose integration's function
/// grants it no invocation: "Invalid permissions on Lambda function", the sentence it words
/// that configuration error with before answering the method's own `500`.
const INVALID_PERMISSIONS_REASON: &str = "Invalid permissions on Lambda function";

/// The ROLE an api's record names for API Gateway to make a call under, out of the value that
/// record carries, or `None` where it names none.
///
/// `Integration$credentials` is "the credentials required for the integration … To specify an
/// IAM role, specify the role's Amazon Resource Name (ARN)", and
/// `Authorizer$authorizerCredentials` is "the required credentials as an IAM role for API
/// Gateway to invoke the authorizer. To specify an IAM role, specify the role's ARN. To use
/// resource-based permissions on the Lambda function, specify null" — so the two records word
/// one choice, and an EMPTY value, a missing one and the spelling that asks for the caller's
/// own identity to be passed through each name no role to assume.
pub(super) fn stated_role(credentials: Option<&str>) -> Option<&str> {
    credentials
        .map(str::trim)
        .filter(|role| !role.is_empty() && *role != CALLER_CREDENTIALS)
}

/// The `credentials` spelling that asks for the CALLER's own identity to be passed through
/// rather than for a role to be assumed, as `Integration$credentials` documents it.
///
/// The same reading [`super::aws_integration`] takes of that member; stated once here, because
/// the value is the same member of the same record whichever integration type carries it.
const CALLER_CREDENTIALS: &str = "arn:aws:iam::*:user/*";

/// Why the ROLE `role_arn` does not admit an invocation of `function` made under it, or `None`
/// when it does — and when nothing about the role could be shown, which refuses nothing.
///
/// Where an api's record names a role ([`stated_role`]), that role is WHO the call is made as,
/// so the permissions in force on it are what admit the invocation and the function's own
/// resource-based policy is not consulted — which is exactly what
/// `Authorizer$authorizerCredentials` offers as the alternative to "resource-based permissions
/// on the Lambda function". Two things have to hold, and each is asked of IAM itself over the
/// in-process hop (`service_common::iam_role`, the one route this tree has to either answer):
/// the role must let this service ASSUME it, and it must allow `lambda:InvokeFunction` on the
/// function. Neither reading refuses on silence: a role this emulator's IAM holds nothing
/// under, and an answer it could not read, leave the call to be made.
///
/// `pub(super)` because an authorizer's function is invoked under the role ITS record names,
/// by the same rule ([`super::custom_authorizer`]).
pub(super) async fn role_unpermitted(
    ctx: &OperationContext<'_>,
    region: &str,
    role_arn: &str,
    function_arn: &str,
) -> Option<String> {
    if ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
        .await
    {
        return Some(format!(
            "the role {role_arn} the api names for this call does not let {SERVICE_PRINCIPAL} \
             assume it, so no invocation was made under it"
        ));
    }
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        INVOKE_FUNCTION_ACTION,
        function_arn,
    )
    .await
    {
        return Some(format!(
            "the role {role_arn} the api names for this call is allowed no \
             {INVOKE_FUNCTION_ACTION} on {function_arn}, so no invocation was made under it"
        ));
    }
    None
}

/// The ARN Lambda reports for `function`, whatever spelling a record addressed it by, or the
/// value itself where Lambda answers nothing about it — which is a function this module judges
/// nothing about.
///
/// `pub(super)` for the reason [`role_unpermitted`] gives.
pub(super) async fn resolved_function_arn(
    client: &::aws_sdk_lambda::Client,
    function: &str,
) -> String {
    client
        .get_function_configuration()
        .function_name(function)
        .send()
        .await
        .ok()
        .and_then(|reported| reported.function_arn)
        .unwrap_or_else(|| function.to_string())
}

/// Whether the resource policy of `function` admits an invocation API Gateway makes for
/// `source_arn` — the ARN of the resource of this api the grant's `aws:SourceArn` narrows a
/// permission to, which is the invoked METHOD for an integration and the AUTHORIZER itself
/// for an authorizer's function.
///
/// `true` also when nothing about the function could be shown, which refuses nothing: a
/// `uri` whose function Lambda does not answer about at all, and an emulator with no Lambda
/// mounted, are not evidence of a missing grant. A function Lambda REPORTS as absent — the
/// record itself, or the version or alias the spelling qualifies it by — is an answer rather
/// than a silence: there is no function for a grant to be attached to, so it grants nothing.
///
/// `pub(super)` because an authorizer's function is admitted by the same grant, read the
/// same way ([`super::custom_authorizer`]).
pub(super) async fn invocation_admitted(
    client: &::aws_sdk_lambda::Client,
    function: &str,
    source_arn: &str,
) -> bool {
    // The RESOURCE a grant is taken on is the function's own ARN, whatever spelling the
    // integration's `uri` addressed it by, so it is resolved from Lambda rather than assumed
    // to be what the `uri` carried. A function Lambda answers nothing about is one this
    // module judges nothing about: the invocation below is what then decides the call.
    let resolved = match client
        .get_function_configuration()
        .function_name(function)
        .send()
        .await
    {
        Ok(reported) => reported.function_arn.unwrap_or_else(|| function.to_string()),
        Err(error) => {
            // Lambda REPORTING that it holds no such function is an answer, not a silence:
            // the spelling — with whatever version or alias it qualifies the function by —
            // resolves to no record at all, so there is nothing for a resource policy to be
            // attached to and no grant that could admit this invocation.
            if error.as_service_error().is_some_and(
                ::aws_sdk_lambda::operation::get_function_configuration::GetFunctionConfigurationError
                    ::is_resource_not_found_exception,
            ) {
                ::tracing::warn!(
                    "the invoked function resolves to no record Lambda holds, so no grant \
                     admits the invocation"
                );
                return false;
            }
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the invoked function was not judged: nothing answered for its \
                 configuration"
            );
            return true;
        }
    };
    let answer = client.get_policy().function_name(&resolved).send().await;
    let document = match answer {
        Ok(reported) => reported.policy.unwrap_or_default(),
        Err(error) => {
            if !refused_by_lambda(&error) {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the invoked function was not judged: nothing answered for its \
                     resource policy"
                );
                return true;
            }
            // A function Lambda reports no resource policy for carries none, which grants
            // nothing.
            String::new()
        }
    };
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let context = [
        (SOURCE_ARN_KEY.to_string(), source_arn.to_string()),
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let request = iam::Request::new(INVOKE_FUNCTION_ACTION, &resolved, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY]);
    iam::resource_decision(&document, &request) == Decision::Allowed
}

/// Whether `error` is LAMBDA refusing the call rather than the emulator failing to carry it.
///
/// A failure the emulator itself produced — no service of it serves the endpoint, or more
/// than one claims it — is not Lambda answering, so it grounds no refusal: `service_common`
/// owns the one reading of that refusal.
fn refused_by_lambda<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    let ::aws_smithy_runtime_api::client::result::SdkError::ServiceError(service) = error else {
        return false;
    };
    let body = service.raw().body().bytes().unwrap_or_default();
    !::service_common::in_process_refusal::is_transport_refusal(body)
}

/// The Lambda client the invocation is made with.
///
/// The transport is the emulator's own, in process, so the call is served by the Lambda this
/// emulator mounts and no connection is opened; the credentials are the ones the callback
/// hands over, which sign this call and are never reported or logged.
///
/// `pub(super)` because an authorizer's function is invoked over the same transport
/// ([`super::custom_authorizer`]).
pub(super) fn lambda_client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// The event document the function is handed for this call.
///
/// Every member is read off the call and this api's own records. A map the call has no
/// entries for is `null` rather than empty, which is the value API Gateway sends for a
/// request carrying no query string, no path parameter and no stage variable.
fn proxy_event(call: &MethodRequest) -> Value {
    let mut event = described(call, event_headers(&call.proxy.multi_value_header));
    let (body, base64_encoded) = event_body(call.inputs.payload.as_deref());
    event.insert(BODY_MEMBER.to_string(), body);
    event.insert(BASE64_MEMBER.to_string(), json!(base64_encoded));
    Value::Object(event)
}

/// The event a `REQUEST` authorizer is handed: the same description of the method request,
/// under the `type` that authorizer type names and beside the ARN of the method the call
/// addresses.
///
/// It carries no body: a `REQUEST` authorizer "receives the caller's identity in a request
/// context" (`AuthorizerType`), and that context is the request's parameters, headers and
/// the stage's own values rather than its payload — so the one event builder serves both,
/// and the authorizer cannot see a member a proxy integration does not report either.
///
/// It DOES carry every header the request sent, including the ones an integration's event
/// withholds ([`HEADERS_WITHHELD_FROM_THE_EVENT`]): the authorizer is the api's own, its
/// `identitySource` is "the identity source for which authorization is requested" and the
/// `Authorization` header is the source such an authorizer is usually declared with, so
/// withholding it would withhold the very value the authorizer is asked about.
pub(super) fn request_authorizer_event(call: &MethodRequest, method_arn: &str) -> Value {
    let mut event = described(call, canonical_headers(&call.proxy.multi_value_header));
    event.insert("type".to_string(), json!("REQUEST"));
    event.insert("methodArn".to_string(), json!(method_arn));
    Value::Object(event)
}

/// What both events say about the method request: the resource template it matched, the path
/// it arrived at, the verb, the `headers` given, the query string in both its forms, the
/// path parameters, the stage's variables and the call's context.
fn described(call: &MethodRequest, headers: BTreeMap<String, Vec<String>>) -> Map<String, Value> {
    let inputs = &call.inputs;
    let proxy = &call.proxy;
    let context = &inputs.context;
    let stage = context.get("stage").cloned().unwrap_or_default();
    let mut event = Map::new();
    event.insert(
        "resource".to_string(),
        json!(context.get("resourcePath").cloned().unwrap_or_default()),
    );
    event.insert("path".to_string(), json!(proxy.request_path));
    event.insert("httpMethod".to_string(), json!(proxy.http_method));
    event.insert("headers".to_string(), last_valued(&headers));
    event.insert(
        MULTI_VALUE_HEADERS_MEMBER.to_string(),
        multi_valued(&headers),
    );
    event.insert(
        "queryStringParameters".to_string(),
        or_null(last_valued(&proxy.multi_value_querystring)),
    );
    event.insert(
        "multiValueQueryStringParameters".to_string(),
        or_null(multi_valued(&proxy.multi_value_querystring)),
    );
    event.insert(
        "pathParameters".to_string(),
        or_null(single_valued(&inputs.path)),
    );
    event.insert(
        "stageVariables".to_string(),
        or_null(single_valued(&inputs.stage_variables)),
    );
    event.insert(
        "requestContext".to_string(),
        request_context(call, &stage),
    );
    event
}

/// The `requestContext` of the event: what this api, this stage and this request are, each
/// read off the records that resolved the call.
fn request_context(call: &MethodRequest, stage: &str) -> Value {
    let context = &call.inputs.context;
    let proxy = &call.proxy;
    let now = SystemTime::now();
    let mut reported = Map::new();
    reported.insert(
        "apiId".to_string(),
        json!(context.get("apiId").cloned().unwrap_or_default()),
    );
    reported.insert("stage".to_string(), json!(stage));
    reported.insert("resourceId".to_string(), json!(proxy.resource_id));
    reported.insert(
        "resourcePath".to_string(),
        json!(context.get("resourcePath").cloned().unwrap_or_default()),
    );
    reported.insert("httpMethod".to_string(), json!(proxy.http_method));
    // The path API Gateway reports is the one the api's own endpoint carries: the stage and
    // then the resource path, whichever addressing the client reached this emulator by.
    reported.insert(
        "path".to_string(),
        json!(format!("/{stage}{}", proxy.request_path)),
    );
    reported.insert(
        "accountId".to_string(),
        json!(::engine::emulator_account_id()),
    );
    if !proxy.domain_name.is_empty() {
        reported.insert("domainName".to_string(), json!(proxy.domain_name));
        if let Some((prefix, _)) = proxy.domain_name.split_once('.') {
            reported.insert("domainPrefix".to_string(), json!(prefix));
        }
    }
    // The request id is the one THIS call is answered under: a response of this data plane
    // names the request it answers in its own header, and the event the function is handed
    // names the same request, so a client correlating what it holds with what the function
    // logged reads one value. A call that carries none (a simulated invocation, which
    // reaches no wire) is reported under a freshly minted one, as it was before.
    let request_id = context
        .get("requestId")
        .cloned()
        .unwrap_or_else(::service_common::query_error_envelope::new_request_id);
    reported.insert("requestId".to_string(), json!(request_id.clone()));
    reported.insert(
        "extendedRequestId".to_string(),
        json!(context
            .get("extendedRequestId")
            .cloned()
            .unwrap_or(request_id)),
    );
    match context.get("requestTime").cloned() {
        Some(stated) => {
            reported.insert("requestTime".to_string(), json!(stated));
        }
        None => {
            if let Some(clf) = common_log_time(now) {
                reported.insert("requestTime".to_string(), json!(clf));
            }
        }
    }
    reported.insert(
        "requestTimeEpoch".to_string(),
        context
            .get("requestTimeEpoch")
            .and_then(|stated| stated.parse::<i64>().ok())
            .map_or_else(|| json!(epoch_millis(now)), |millis| json!(millis)),
    );
    reported.insert("identity".to_string(), identity(call));
    // What the method's authorizer published for this call, under the member paths a proxy
    // event carries them in: a user pool's `claims` as the object they are, and a custom
    // authorizer's own values beside it. A method whose authorization publishes nothing adds
    // nothing.
    let mut authorizer = Map::new();
    let mut claims = Map::new();
    for (member, value) in context {
        let Some(published) = member.strip_prefix(&format!("{AUTHORIZER_MEMBER}.")) else {
            continue;
        };
        match published.strip_prefix(&format!("{CLAIMS_MEMBER}.")) {
            Some(claim) => {
                claims.insert(claim.to_string(), json!(value));
            }
            None => {
                authorizer.insert(published.to_string(), json!(value));
            }
        }
    }
    if !claims.is_empty() {
        authorizer.insert(CLAIMS_MEMBER.to_string(), Value::Object(claims));
    }
    if !authorizer.is_empty() {
        reported.insert(AUTHORIZER_MEMBER.to_string(), Value::Object(authorizer));
    }
    Value::Object(reported)
}

/// The `identity` of the event: what the REQUEST says about who made it, and nothing else.
///
/// Only the two values a request off the wire carries are reported — the address the request
/// came from and the user agent it declared — because every other member of this object
/// names a principal API Gateway authenticated, and a method is authenticated by its own
/// `authorizationType` rather than by this object.
///
/// `sourceIp` is "the source IP address of the TCP connection making the request": the
/// FIRST hop a forwarder recorded where the request reached the emulator through one, and
/// otherwise the loopback address, which is the interface this emulator is reached over when
/// nothing forwarded the request. The member is always reported, because every request
/// arrived from somewhere.
fn identity(call: &MethodRequest) -> Value {
    let mut reported = Map::new();
    reported.insert(
        "sourceIp".to_string(),
        json!(super::source_address(
            header_of(call, FORWARDED_FOR_HEADER).as_deref()
        )),
    );
    if let Some(agent) = header_of(call, USER_AGENT_HEADER) {
        reported.insert("userAgent".to_string(), json!(agent));
    }
    Value::Object(reported)
}

/// The value the call carries for the request header `name`, looked up case-insensitively
/// because a header name is.
fn header_of(call: &MethodRequest, name: &str) -> Option<String> {
    call.inputs
        .header
        .iter()
        .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// The event's `body` and `isBase64Encoded`: the request's own bytes as TEXT where they are
/// text, and base64-encoded where they are not — "the body is base64-encoded" is exactly
/// what that member says.
///
/// A call carrying no payload carries no body, which the event reports as `null` rather than
/// as the empty string: API Gateway sends `null` for a request with no body at all.
fn event_body(payload: Option<&[u8]>) -> (Value, bool) {
    let Some(payload) = payload.filter(|bytes| !bytes.is_empty()) else {
        return (Value::Null, false);
    };
    match ::std::str::from_utf8(payload) {
        Ok(text) => (json!(text), false),
        Err(_) => (
            json!(::base64::engine::general_purpose::STANDARD.encode(payload)),
            true,
        ),
    }
}

/// One single-valued map of the call as a JSON object.
fn single_valued(values: &BTreeMap<String, String>) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect(),
    )
}

/// The SINGLE-VALUED view of a multi-valued map of the call, as the event's `headers` and
/// `queryStringParameters` carry it: the LAST value each name carries.
///
/// A proxy event reports a repeated name twice — every value it carries in the multi-valued
/// member beside this one, and one value here — so this half has to choose, and API Gateway
/// reports the last of them: `?a=1&a=2` is `{"a": "2"}` beside `{"a": ["1", "2"]}`. A name
/// whose values are somehow empty is left out rather than reported as a null.
fn last_valued(values: &BTreeMap<String, Vec<String>>) -> Value {
    Value::Object(
        values
            .iter()
            .filter_map(|(name, carried)| {
                carried
                    .iter()
                    .next_back()
                    .map(|last| (name.clone(), json!(last)))
            })
            .collect(),
    )
}

/// `headers` without the names [`HEADERS_WITHHELD_FROM_THE_EVENT`] lists, compared
/// case-insensitively because a header name is.
fn withheld<V: Clone>(headers: &BTreeMap<String, V>) -> BTreeMap<String, V> {
    headers
        .iter()
        .filter(|(name, _)| {
            !HEADERS_WITHHELD_FROM_THE_EVENT
                .iter()
                .any(|withheld| name.eq_ignore_ascii_case(withheld))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The request's headers as the EVENT reports them: the ones it carries at all
/// ([`withheld`]), each under the HTTP-canonical spelling of its name.
///
/// The names are canonicalised because the wire layer a request arrives over normalises
/// every header name to lower case, while the event reports the spelling HTTP itself
/// capitalises a field name in — `User-Agent`, `X-Forwarded-For`, `Content-Type` — which is
/// the form a client writes and the form API Gateway hands the function. A function reading
/// the event by name therefore finds the header it would find on AWS.
fn event_headers(headers: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    canonical_headers(&withheld(headers))
}

/// `headers` under the HTTP-canonical spelling of each name; see [`event_headers`].
fn canonical_headers(headers: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    headers
        .iter()
        .map(|(name, values)| (canonical_header_name(name), values.clone()))
        .collect()
}

/// `name` in the capitalisation HTTP spells a field name with: every `-`-separated token's
/// first letter upper-cased and the rest lower-cased (`x-multi` -> `X-Multi`).
fn canonical_header_name(name: &str) -> String {
    name.split('-')
        .map(|token| {
            let mut glyphs = token.chars();
            match glyphs.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &glyphs.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join("-")
}

/// One multi-valued map of the call as a JSON object of arrays.
fn multi_valued(values: &BTreeMap<String, Vec<String>>) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, carried)| (name.clone(), json!(carried)))
            .collect(),
    )
}

/// `Null` for an empty object, which is the value API Gateway sends for a map the request
/// has no entries for.
fn or_null(object: Value) -> Value {
    match &object {
        Value::Object(entries) if entries.is_empty() => Value::Null,
        _ => object,
    }
}

// The instant `at` in the Common Log Format API Gateway reports `requestTime` in, and the
// same instant in the milliseconds `requestTimeEpoch` reports: the ONE rendering of each
// lives beside the call's own `$context`, which is built with them too
// (`super::call_context`), so the event and a mapping template cannot report one instant two
// ways.
use super::{common_log_time, epoch_millis};

/// The method response the function's returned document IS, or the reason it is not a
/// well-formed proxy response.
///
/// A proxy integration maps nothing, so every part of the response is read straight out of
/// the document: the status it names, the headers it declares in either member, and the body
/// it carries.
///
/// `binary_media_types` is the API's own `RestApi$binaryMediaTypes` — "the list of binary
/// media types supported by the RestApi. By default, the RestApi supports only UTF-8-encoded
/// text payloads" — and it is what decides what the BODY is. A document declaring
/// `isBase64Encoded` says its `body` is the base64 TEXT of some bytes; those bytes are what
/// the client receives only where the response's own media type is one the API declares
/// binary. An API that declares none supports only text payloads, so the base64 text IS the
/// payload and travels unchanged — which is why the decoding is decided here, after the media
/// type the headers declare is known, rather than off the member alone.
fn proxy_response(
    returned: &[u8],
    binary_media_types: &[String],
) -> Result<IntegratedResponse, String> {
    let Ok(Value::Object(document)) = ::serde_json::from_slice::<Value>(returned) else {
        return Err("the function returned no JSON object".to_string());
    };
    let status = status_code(document.get(STATUS_CODE_MEMBER))
        .ok_or_else(|| format!("the function's {STATUS_CODE_MEMBER} is not an HTTP status"))?;

    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in object_of(document.get(HEADERS_MEMBER), HEADERS_MEMBER)? {
        let text = header_value(value)
            .ok_or_else(|| format!("the function's {HEADERS_MEMBER}.{name} is not a value a \
                                    header carries"))?;
        headers.insert(name.clone(), vec![text]);
    }
    for (name, value) in object_of(
        document.get(MULTI_VALUE_HEADERS_MEMBER),
        MULTI_VALUE_HEADERS_MEMBER,
    )? {
        let Value::Array(carried) = value else {
            return Err(format!(
                "the function's {MULTI_VALUE_HEADERS_MEMBER}.{name} is not a list of values"
            ));
        };
        let mut values = Vec::with_capacity(carried.len());
        for one in carried {
            values.push(header_value(one).ok_or_else(|| {
                format!(
                    "the function's {MULTI_VALUE_HEADERS_MEMBER}.{name} carries a value no \
                     header carries"
                )
            })?);
        }
        // The multi-valued member is the complete set of values for the names it declares,
        // so it REPLACES what the single-valued member said about the same name.
        headers.insert(name.clone(), values);
    }

    // The media type is one of the headers the function declared, and `application/json` is
    // what API Gateway sends for a proxy response that declares none.
    let declared = headers
        .keys()
        .find(|name| name.eq_ignore_ascii_case(CONTENT_TYPE_HEADER))
        .cloned();
    let content_type = declared
        .and_then(|name| {
            headers
                .remove(&name)
                .and_then(|values| values.into_iter().next())
        })
        .unwrap_or_else(|| JSON_CONTENT_TYPE.to_string());

    let base64_encoded = match document.get(BASE64_MEMBER) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(encoded)) => *encoded,
        Some(_) => return Err(format!("the function's {BASE64_MEMBER} is not a boolean")),
    };
    let decoded = base64_encoded && declares_binary(&content_type, binary_media_types);
    let body = match document.get(BODY_MEMBER) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) if decoded => {
            ::base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|_| {
                    format!("the function's {BODY_MEMBER} is not the base64 its \
                             {BASE64_MEMBER} declares")
                })?
        }
        Some(Value::String(text)) => text.clone().into_bytes(),
        Some(_) => return Err(format!("the function's {BODY_MEMBER} is not a string")),
    };

    Ok(IntegratedResponse {
        status,
        content_type,
        body,
        headers,
    })
}

/// Whether `media` is one of the media types `declared` names as binary.
///
/// `RestApi$binaryMediaTypes` carries media types, and a media type's two tokens may each be
/// the wildcard `*` (`*/*` for every payload, `image/*` for a whole type), which is how an
/// api declares a class of payloads binary rather than listing every subtype. Compared
/// without the parameters a `Content-Type` may carry (`; charset=…`) and case-insensitively,
/// because a media type's type and subtype are.
fn declares_binary(media: &str, declared: &[String]) -> bool {
    let (media_type, media_subtype) = media_type_tokens(media);
    declared.iter().any(|binary| {
        let (declared_type, declared_subtype) = media_type_tokens(binary);
        let matches = |declared: &str, sent: &str| {
            declared == "*" || declared.eq_ignore_ascii_case(sent)
        };
        matches(declared_type, media_type) && matches(declared_subtype, media_subtype)
    })
}

/// The type and subtype of a media type, with any parameters dropped: `image/png;
/// charset=x` is `("image", "png")`. A value carrying no `/` at all has no subtype, which
/// no media type matches but the wildcard.
fn media_type_tokens(media: &str) -> (&str, &str) {
    let without_parameters = media.split(';').next().unwrap_or(media).trim();
    match without_parameters.split_once('/') {
        Some((media_type, subtype)) => (media_type.trim(), subtype.trim()),
        None => (without_parameters, ""),
    }
}

/// The status the function's `statusCode` names, which the member carries as a number or as
/// the text of one, and which has to be an HTTP status code.
fn status_code(carried: Option<&Value>) -> Option<u16> {
    let status = match carried? {
        Value::Number(number) => u16::try_from(number.as_u64()?).ok()?,
        Value::String(text) => text.trim().parse::<u16>().ok()?,
        _ => return None,
    };
    (100..=599).contains(&status).then_some(status)
}

/// The entries of an OBJECT member of the returned document, empty for one it does not
/// carry, or the reason the member is not an object at all.
fn object_of<'d>(
    carried: Option<&'d Value>,
    member: &str,
) -> Result<Vec<(&'d String, &'d Value)>, String> {
    match carried {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(entries)) => Ok(entries.iter().collect()),
        Some(_) => Err(format!("the function's {member} is not an object")),
    }
}

/// The text one header value is, for the three JSON scalars a proxy response may declare
/// one as; `None` for a value no header carries.
fn header_value(carried: &Value) -> Option<String> {
    match carried {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long the integration waits for the function's answer, over the values a stored
    /// `timeoutInMillis` can carry: the rows differ along where the stored number sits
    /// relative to the range `Integration$timeoutInMillis` documents (each published bound,
    /// a value inside it, one past each bound, and the zero a record that was never given
    /// one would carry).
    #[test]
    fn the_wait_is_the_integrations_own_timeout() {
        let default = u64::try_from(crate::ops::put_integration::DEFAULT_TIMEOUT_IN_MILLIS)
            .expect("the documented default is a positive number of milliseconds");
        let rows: [(&str, i32, u64); 7] = [
            ("the documented default, which PutIntegration stores for a request naming none",
             crate::ops::put_integration::DEFAULT_TIMEOUT_IN_MILLIS, default),
            ("the lowest value the member admits", 50, 50),
            ("a value inside the range", 1_500, 1_500),
            ("the highest value the member admits", 29_000, 29_000),
            ("one millisecond below the lowest", 49, default),
            ("one millisecond above the highest", 29_001, default),
            ("a record carrying no value at all", 0, default),
        ];
        for (case, stored, expected) in rows {
            let integration = GetIntegrationOutput {
                timeout_in_millis: stored,
                ..GetIntegrationOutput::default()
            };
            let waited = waited_for(&integration);
            assert_eq!(
                waited,
                Duration::from_millis(expected),
                "case {case:?}: expected a {expected} ms wait for a stored {stored}, got \
                 {waited:?}"
            );
        }
    }

    /// One `uri` row: the case, the value, and the Region and function it names (`None` for
    /// a value that names no function-invoking action at all).
    struct UriRow {
        case: &'static str,
        uri: String,
        names: Option<(String, String)>,
    }

    /// The `uri` form the function-invoking action is named in, over the parts that decide
    /// the invocation — and the forms that name no function at all.
    #[test]
    fn an_aws_proxy_uri_names_one_function_and_the_region_it_is_invoked_in() {
        let function = "arn:aws:lambda:us-east-1:000000000000:function:writer";
        let qualified = format!("{function}:live");
        let rows = [
            UriRow {
                case: "the documented form, naming the function by ARN",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                     {function}/invocations"
                ),
                names: Some(("us-east-1".to_string(), function.to_string())),
            },
            UriRow {
                case: "an alias qualifier, which is part of the function the uri addresses",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                     {qualified}/invocations"
                ),
                names: Some(("us-east-1".to_string(), qualified.clone())),
            },
            UriRow {
                case: "the function named by name alone, at another Region's endpoint",
                uri: "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: Some(("eu-west-1".to_string(), "writer".to_string())),
            },
            UriRow {
                case: "no invocations segment",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/{function}"
                ),
                names: None,
            },
            UriRow {
                case: "the action form, which names no invocation path",
                uri: "arn:aws:apigateway:us-east-1:lambda:action/Invoke".to_string(),
                names: None,
            },
            UriRow {
                case: "another service's subdomain",
                uri: "arn:aws:apigateway:us-east-1:s3:path/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: None,
            },
            UriRow {
                case: "a uri that is no ARN at all",
                uri: "https://lambda.us-east-1.amazonaws.com/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: None,
            },
        ];
        for row in &rows {
            let read = invoked_function(&row.uri).map(|(region, function)| {
                (region.to_string(), function.to_string())
            });
            assert_eq!(
                read, row.names,
                "case {:?}: expected {:?}, got {read:?}",
                row.case, row.names
            );
        }
    }

    /// What a function's returned document maps to, over the shapes a proxy response takes
    /// and the ones that are not proxy responses at all.
    #[test]
    fn a_proxy_response_is_the_document_the_function_returned() {
        let no_binary_types: [String; 0] = [];
        let served = proxy_response(
            br#"{"statusCode":201,"headers":{"Content-Type":"text/plain","X-One":"1"},
                 "multiValueHeaders":{"Set-Cookie":["a=1","b=2"]},"body":"hello"}"#,
            &no_binary_types,
        )
        .expect("case \"a well-formed proxy response\": expected it to be read");
        assert_eq!(
            served.status, 201,
            "case \"a well-formed proxy response\": expected 201, got {}",
            served.status
        );
        assert_eq!(
            served.content_type, "text/plain",
            "case \"a well-formed proxy response\": expected the declared media type, got {:?}",
            served.content_type
        );
        assert_eq!(
            served.body, b"hello",
            "case \"a well-formed proxy response\": expected the declared body, got {:?}",
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.headers.get("Set-Cookie").map(Vec::as_slice),
            Some(["a=1".to_string(), "b=2".to_string()].as_slice()),
            "case \"a well-formed proxy response\": expected both cookie values, got {:?}",
            served.headers
        );

        let encoded = proxy_response(
            br#"{"statusCode":"200","body":"AAEC","isBase64Encoded":true}"#,
            &[JSON_CONTENT_TYPE.to_string()],
        )
        .expect("case \"a base64 body of a media type the api declares binary\": expected it \
                 to be read");
        assert_eq!(
            encoded.body,
            vec![0u8, 1, 2],
            "case \"a base64 body of a media type the api declares binary\": expected the \
             decoded bytes, got {:?}",
            encoded.body
        );
        assert_eq!(
            encoded.content_type, JSON_CONTENT_TYPE,
            "case \"a base64 body of a media type the api declares binary\": expected the \
             default media type, got {:?}",
            encoded.content_type
        );

        // The same document on an api that declares the media type NOT binary: the base64
        // text IS the payload, and a text that is not base64 at all is still that payload.
        for (case, declared) in [
            ("an api declaring no binary media type at all", &[][..]),
            (
                "an api declaring another media type binary",
                &["image/png".to_string()][..],
            ),
        ] {
            for (body, expected) in [("AAEC", "AAEC"), ("!", "!")] {
                let text = proxy_response(
                    format!(
                        r#"{{"statusCode":200,"body":"{body}","isBase64Encoded":true}}"#
                    )
                    .as_bytes(),
                    declared,
                )
                .unwrap_or_else(|reason| {
                    panic!("case {case:?} ({body:?}): expected it to be read, got {reason}")
                });
                assert_eq!(
                    text.body,
                    expected.as_bytes(),
                    "case {case:?} ({body:?}): expected the base64 text unchanged, got {:?}",
                    String::from_utf8_lossy(&text.body)
                );
            }
        }

        for (case, returned) in [
            ("no status at all", &br#"{"body":"hi"}"#[..]),
            ("a status outside the HTTP range", &br#"{"statusCode":99}"#[..]),
            ("a body that is not a string", &br#"{"statusCode":200,"body":{"a":1}}"#[..]),
            ("a base64 body that is not base64", &br#"{"statusCode":200,"body":"!","isBase64Encoded":true}"#[..]),
            ("headers that are not an object", &br#"{"statusCode":200,"headers":[]}"#[..]),
            ("no JSON object at all", &b"not json"[..]),
        ] {
            let read = proxy_response(returned, &[JSON_CONTENT_TYPE.to_string()]);
            assert!(
                read.is_err(),
                "case {case:?}: expected no proxy response to be read, got {:?}",
                read.map(|response| response.status)
            );
        }
    }

    /// Which media types an api's `binaryMediaTypes` declares binary: the wildcard forms a
    /// media type takes, the parameters a `Content-Type` carries, and the case neither is
    /// compared in.
    #[test]
    fn the_apis_binary_media_types_decide_which_payloads_are_binary() {
        // (case, the api's declared list, the response's media type, binary)
        let rows: &[(&str, &[&str], &str, bool)] = &[
            ("no type declared at all", &[], "image/png", false),
            ("the exact type", &["image/png"], "image/png", true),
            ("another type", &["image/png"], "image/jpeg", false),
            ("a type whose case differs", &["Image/PNG"], "image/png", true),
            (
                "a declared type against a media type carrying parameters",
                &["text/plain"],
                "text/plain; charset=utf-8",
                true,
            ),
            ("the subtype wildcard", &["image/*"], "image/png", true),
            ("the subtype wildcard of another type", &["image/*"], "text/plain", false),
            ("the whole wildcard", &["*/*"], "application/octet-stream", true),
            ("one entry of several", &["image/png", "application/pdf"], "application/pdf", true),
            ("none of several", &["image/png", "application/pdf"], "text/csv", false),
            ("a value that is no media type", &["image/png"], "octet-stream", false),
        ];
        for (case, declared, media, expected) in rows {
            let declared: Vec<String> = declared.iter().map(|one| (*one).to_string()).collect();
            let read = declares_binary(media, &declared);
            assert_eq!(
                read, *expected,
                "case {case:?} ({media:?} against {declared:?}): expected {expected}, got \
                 {read}"
            );
        }
    }
}
