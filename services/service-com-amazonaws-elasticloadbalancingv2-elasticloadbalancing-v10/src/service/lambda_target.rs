// SPDX-License-Identifier: CC0-1.0

//! A `forward` to a `lambda` TARGET GROUP: how a request a client sent to an Application
//! Load Balancer is served by INVOKING the function registered with the group, and what the
//! load balancer answers for a function answer that breaks its response contract.
//!
//! `TargetGroup$TargetType` gives the registration — "lambda - Register a single Lambda
//! function as a target" — and `TargetDescription$Id` gives what the registration holds:
//! "if the target type is lambda, specify the ARN of the Lambda function". So a `forward` to
//! such a group names ONE function, and the request is made against it THROUGH THIS
//! EMULATOR, over the in-process transport `OperationContext::callback` hands an operation:
//! no connection is opened, and what the client is answered with is what the function's own
//! run produced.
//!
//! # What the function is handed, and what it must return
//!
//! The load balancer hands the function ONE event describing the request it received — the
//! target group it was forwarded to, the method, the path, the query string, the headers and
//! the body — and takes the object the function returned as the response: its status code,
//! its headers and its body. Every value in the event is read off THIS request and this
//! service's own records: the body travels as text where its bytes are text and
//! base64-encoded with `isBase64Encoded` set where they are not, and the single-valued
//! members become the multi-valued ones for a group whose
//! `lambda.multi_value_headers.enabled` is on ([`multi_value_headers`]) — "indicates whether
//! the request and response headers that are exchanged between the load balancer and the
//! Lambda function include arrays of values or strings"
//! (`TargetGroupAttribute$Key`'s own catalog).
//!
//! # Who the invocation is made as, and what refuses it
//!
//! A load balancer invokes a function as its own service principal ([`SERVICE_PRINCIPAL`]),
//! so the grant that admits the invocation is the FUNCTION's resource-based policy — the
//! document `AddPermission` writes and `GetPolicy` reports — and it is read from Lambda
//! itself rather than kept a second time here. The decision is
//! [`::service_common::iam_authorization`], the tree's one evaluation of policy documents,
//! taken on `lambda:InvokeFunction` over the registered function with the TARGET GROUP's ARN
//! as the `aws:SourceArn` such a grant is narrowed by — which is how a permission added for
//! one target group admits the invocation for it and for no other.
//!
//! Nothing is refused on evidence this emulator does not have: a registration whose function
//! Lambda does not answer about at all, and an emulator with no Lambda mounted, leave the
//! call to be made. A function whose policy Lambda REPORTS as absent is an answer rather than
//! a silence: it grants nothing, so the call is not made.
//!
//! # What the load balancer answers instead
//!
//! The returned object is a CONTRACT, and the load balancer — not the client — is what
//! answers a function that breaks it: a run that errored, a payload that is not a JSON
//! object, one that names no readable status code, one whose body or headers are not of the
//! shape the contract gives them, and one past the response size a Lambda target may answer
//! with are each answered [`BAD_GATEWAY_STATUS`], and the payload is NEVER passed through.
//! An invocation the function's own policy admits no load balancer to make earns the same
//! status, with nothing run at all.
//! The invocation itself still happened — the function ran, with whatever effects its run
//! had — and nothing of the registration or of its health changes: a contract violation
//! costs that one request and no more.
//!
//! No body is invented for any of those statuses, exactly as the sibling statuses this plane
//! answers carry none ([`super::data_plane`]): the load balancer's own error page is not a
//! document this emulator holds.

use ::serde_json::{Map, Value};
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::OperationContext;

use super::data_plane::{DataPlaneRequest, Served};
use crate::state::TargetGroupRecord;

/// The status a load balancer answers when the function it forwarded the request to
/// returned no response the Lambda-target contract admits — including a run that produced no
/// response at all. The load balancer answers it ITSELF: the payload the function returned is
/// not passed through.
pub(super) const BAD_GATEWAY_STATUS: u16 = 502;

/// The status a request whose body is larger than one invocation of a Lambda target may
/// carry is answered with: the request never becomes an event, so nothing is invoked for it.
const PAYLOAD_TOO_LARGE_STATUS: u16 = 413;

/// The most bytes the request body forwarded to a Lambda target may carry, and the most the
/// function's own response may: the documented 1 MB request and response limits of a Lambda
/// target. Measured over the whole payload each way, which is what the limit is on — the
/// response's headers and body travel in one returned object.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The ARN namespace a Lambda function's ARN names it under, which is where the Region the
/// invocation is made in is read from.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// The service principal a load balancer invokes a function as, as a resource policy names
/// one: "for Amazon Web Services services, the principal is a domain-style identifier that
/// the service defines" (`AddPermission$Principal`). It is the `cloudTrailEventSource` /
/// service identifier of Elastic Load Balancing, the one this service's own events carry.
const SERVICE_PRINCIPAL: &str = "elasticloadbalancing.amazonaws.com";

/// The IAM action an invocation of a function performs, which is not the operation's own
/// name: `Invoke` "requires permission for the `lambda:InvokeFunction` action".
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The condition key a grant narrowed to ONE invoking resource names the source by, which
/// for a load balancer's invocation is the TARGET GROUP the function is registered with.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key a grant narrowed to the account that owns the invoking load balancer
/// names it by.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The attribute that decides whether the headers and the query string travel as ARRAYS:
/// `TargetGroupAttribute$Key`'s "`lambda.multi_value_headers.enabled` - Indicates whether the
/// request and response headers that are exchanged between the load balancer and the Lambda
/// function include arrays of values or strings. … supported only if the load balancer is an
/// Application Load Balancer and the target is a Lambda function".
const MULTI_VALUE_HEADERS_ATTRIBUTE: &str = "lambda.multi_value_headers.enabled";

/// The members of the event the function is handed, and of the object it returns.
const TARGET_GROUP_ARN_MEMBER: &str = "targetGroupArn";
const STATUS_CODE_MEMBER: &str = "statusCode";
const HEADERS_MEMBER: &str = "headers";
const MULTI_VALUE_HEADERS_MEMBER: &str = "multiValueHeaders";
const BODY_MEMBER: &str = "body";
const BASE64_MEMBER: &str = "isBase64Encoded";

/// The headers the load balancer itself reports to the target about the hop it took: the
/// protocol and the port of the LISTENER the request arrived on, which is what this service
/// holds about it. The client's own address is not among them — a request served in-process
/// carries no connection address (see [`super::data_plane`]), so nothing is reported for one.
const FORWARDED_PROTO_HEADER: &str = "x-forwarded-proto";
const FORWARDED_PORT_HEADER: &str = "x-forwarded-port";

/// The response header a load balancer's own cookie travels in — the target-group stickiness
/// pin ([`super::target_group_selection`]) — beside whatever the function's own response set.
const SET_COOKIE_HEADER: &str = "set-cookie";

/// Which function a `forward` to a `lambda` target group invokes, and what the event reports
/// about the hop — read out of the store under its own guard, so the invocation is made with
/// no lock of this service's held.
#[derive(Debug)]
pub(super) struct Forwarded {
    /// The ARN of the function the chosen target registers.
    pub(super) function_arn: String,
    /// The target group that registration belongs to, which the event names as the resource
    /// the load balancer forwarded the request to.
    pub(super) target_group_arn: String,
    /// Whether that group exchanges ARRAYS of header and query-string values with the
    /// function ([`multi_value_headers`]).
    pub(super) multi_value_headers: bool,
    /// The protocol of the listener the request arrived on, when it carries one.
    pub(super) protocol: Option<String>,
    /// The port of that listener, when it carries one.
    pub(super) port: Option<i32>,
    /// The `Set-Cookie` field the answer carries, where the forward action pins the client
    /// to the target group this request was distributed to
    /// ([`super::target_group_selection`]). The load balancer's own, so it accompanies the
    /// response the FUNCTION produced and no status the load balancer answered instead.
    pub(super) set_cookie: Option<String>,
}

/// Whether `group` exchanges ARRAYS of header and query-string values with its function:
/// what [`MULTI_VALUE_HEADERS_ATTRIBUTE`] holds for it — a caller's own write, else the value
/// the attribute is carried at — read through the ONE catalog of target group attributes, so
/// what `DescribeTargetGroupAttributes` reports and what the data plane does cannot drift.
pub(super) fn multi_value_headers(group: &TargetGroupRecord) -> bool {
    let Some(spec) = crate::state::target_group_attribute(MULTI_VALUE_HEADERS_ATTRIBUTE) else {
        return false;
    };
    if !spec.applies_to(group) {
        return false;
    }
    group
        .attributes
        .get(MULTI_VALUE_HEADERS_ATTRIBUTE)
        .cloned()
        .or_else(|| spec.default_for(group))
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

/// Serve one request by invoking the function `forwarded` names, and answer what the run
/// produced — or the load balancer's own [`BAD_GATEWAY_STATUS`] when it produced nothing the
/// contract admits.
///
/// Every answer records what the load balancer OBSERVED while producing it, which is what
/// its own access log carries for the request ([`super::access_logs`]): the `forward` it
/// performed, the target group it went to, the status the FUNCTION answered where it answered
/// one, how long the invocation took, and — for an answer the load balancer generated itself
/// — the class of the failure it generated it for.
pub(super) async fn served(
    ctx: &OperationContext<'_>,
    forwarded: &Forwarded,
    request: &DataPlaneRequest,
    body: &[u8],
) -> Served {
    let mut served = invoked(ctx, forwarded, request, body).await;
    served
        .observed
        .actions
        .push(crate::state::FORWARD_ACTION_TYPE.to_string());
    served.observed.target_group_arn = Some(forwarded.target_group_arn.clone());
    served
}

/// The load balancer's OWN answer for one class of Lambda-target failure: the status it
/// generates, and the reason its access log records beside it
/// ([`super::access_logs::reason`]). No payload of the function's is passed on with it.
fn generated(status: u16, reason: &str) -> Served {
    let mut served = Served::status(status);
    served.observed.error_reason = Some(reason.to_string());
    served
}

/// The invocation itself, and what the run produced.
async fn invoked(
    ctx: &OperationContext<'_>,
    forwarded: &Forwarded,
    request: &DataPlaneRequest,
    body: &[u8],
) -> Served {
    // The event the function is invoked with CARRIES the request's body, so the quota on one
    // invocation's payload bounds the request: a body past it is refused before an event is
    // built from it, and nothing is invoked.
    if body.len() > MAX_PAYLOAD_BYTES {
        ::tracing::info!(
            target_group = %forwarded.target_group_arn,
            bytes = body.len(),
            "the request's body is larger than one invocation of a Lambda target carries, so \
             the function is not invoked"
        );
        return generated(
            PAYLOAD_TOO_LARGE_STATUS,
            super::access_logs::reason::LAMBDA_REQUEST_TOO_LARGE,
        );
    }
    // The Region the invocation is made in is the function's own, which its ARN names — the
    // ARN a registration of a `lambda` target group is written as.
    let Some(region) =
        ::service_common::arn::region_of(&forwarded.function_arn, LAMBDA_ARN_NAMESPACE)
    else {
        ::tracing::warn!(
            target = %forwarded.function_arn,
            "the registered target is not a Lambda function ARN, so no function was invoked"
        );
        return Served::status(BAD_GATEWAY_STATUS);
    };
    let payload = match ::serde_json::to_vec(&event(forwarded, request, body)) {
        Ok(payload) => payload,
        Err(error) => {
            ::tracing::warn!(
                error = %error,
                "the forwarded request could not be written as an event, so no function was \
                 invoked"
            );
            return Served::status(BAD_GATEWAY_STATUS);
        }
    };
    let client = lambda_client(ctx, region);
    // The grant that admits the invocation is the FUNCTION's own, narrowed to the target
    // group this request was forwarded to. An invocation it does not admit is never made,
    // and the load balancer answers the request itself.
    if !invocation_admitted(&client, &forwarded.function_arn, &forwarded.target_group_arn).await {
        ::tracing::info!(
            function = %forwarded.function_arn,
            target_group = %forwarded.target_group_arn,
            "the registered function allows {SERVICE_PRINCIPAL} no {INVOKE_FUNCTION_ACTION} \
             for this target group, so no invocation was made"
        );
        return generated(
            BAD_GATEWAY_STATUS,
            super::access_logs::reason::LAMBDA_ACCESS_DENIED,
        );
    }
    // How long the TARGET took, which the access-log record reports for an invocation that
    // was made at all.
    let started = ::std::time::Instant::now();
    let answered = client
        .invoke()
        .function_name(&forwarded.function_arn)
        // The client is answered WITH the function's response, so the invocation is the
        // synchronous one: "RequestResponse (default) – Invoke the function synchronously"
        // (`Invoke$InvocationType`).
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(payload))
        .send()
        .await;
    let answered = match answered {
        Ok(answered) => answered,
        Err(error) => {
            // No invocation was made at all — the target could not be reached — which is the
            // same `502` a function answer the contract refuses earns, because either way
            // the load balancer has no usable response to pass on.
            ::tracing::warn!(
                function = %forwarded.function_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Lambda target of the forward did not answer the invocation"
            );
            return Served::status(BAD_GATEWAY_STATUS);
        }
    };
    // The function RAN, however its run ended, so every answer below carries the time it
    // took.
    let elapsed = started.elapsed();
    let with_elapsed = |mut served: Served| {
        served.observed.target_elapsed = Some(elapsed);
        served
    };
    // "FunctionError: if present, indicates that an error occurred during function
    // execution" — a run that errored returned no response object, and the load balancer
    // answers the client itself rather than passing the error document on.
    if let Some(signal) = answered.function_error() {
        ::tracing::info!(
            function = %forwarded.function_arn,
            function_error = %signal,
            "the Lambda target's run ended in a function error, so the load balancer answers \
             the request itself"
        );
        return with_elapsed(generated(
            BAD_GATEWAY_STATUS,
            super::access_logs::reason::LAMBDA_UNHANDLED,
        ));
    }
    let returned = answered
        .payload()
        .map(|payload| payload.as_ref().to_vec())
        .unwrap_or_default();
    if returned.len() > MAX_PAYLOAD_BYTES {
        ::tracing::info!(
            function = %forwarded.function_arn,
            bytes = returned.len(),
            "the Lambda target's response is larger than a Lambda target may answer with, so \
             the load balancer answers the request itself"
        );
        return with_elapsed(generated(
            BAD_GATEWAY_STATUS,
            super::access_logs::reason::LAMBDA_RESPONSE_TOO_LARGE,
        ));
    }
    match mapped_response(&returned, forwarded.multi_value_headers) {
        Ok(mut served) => {
            // The target answered, so the pin the forward action mints goes back with its
            // response — the one answer "requests from a client should be routed to the same
            // target group" can be made true of.
            if let Some(field) = forwarded.set_cookie.clone() {
                served.headers.push((SET_COOKIE_HEADER.to_string(), field));
            }
            // The status the client is answered with is the FUNCTION's own, which is what the
            // record reports as the target's.
            served.observed.target_status = Some(served.status);
            with_elapsed(served)
        }
        Err(reason) => {
            ::tracing::info!(
                function = %forwarded.function_arn,
                detail = %reason,
                "the Lambda target returned no response the load balancer's contract admits, \
                 so the load balancer answers the request itself"
            );
            with_elapsed(generated(
                BAD_GATEWAY_STATUS,
                super::access_logs::reason::LAMBDA_INVALID_RESPONSE,
            ))
        }
    }
}

/// The event one forwarded request becomes.
///
/// `requestContext.elb.targetGroupArn` names the target group the request was forwarded to,
/// which is the one resource of this service the function is told about; everything else is
/// the request as the client sent it. The headers are the ones the rules were evaluated
/// against — lower-cased, which is how a header name reaches a Lambda target — plus what
/// the load balancer itself reports about the hop ([`FORWARDED_PROTO_HEADER`]).
fn event(forwarded: &Forwarded, request: &DataPlaneRequest, body: &[u8]) -> Value {
    let mut event = Map::new();
    event.insert(
        "requestContext".to_string(),
        ::serde_json::json!({
            "elb": { TARGET_GROUP_ARN_MEMBER: forwarded.target_group_arn.clone() }
        }),
    );
    event.insert("httpMethod".to_string(), Value::from(request.method.clone()));
    event.insert("path".to_string(), Value::from(request.path.clone()));
    let query = query_pairs(&request.query);
    let headers = forwarded_headers(forwarded, request);
    if forwarded.multi_value_headers {
        event.insert(
            "multiValueQueryStringParameters".to_string(),
            multi_valued(&query),
        );
        event.insert(MULTI_VALUE_HEADERS_MEMBER.to_string(), multi_valued(&headers));
    } else {
        event.insert("queryStringParameters".to_string(), single_valued(&query));
        event.insert(HEADERS_MEMBER.to_string(), single_valued(&headers));
    }
    // The body travels as the text it is where its bytes are text, and base64-encoded with
    // the flag set where they are not — the one encoding a JSON document carries bytes in.
    match ::std::str::from_utf8(body) {
        Ok(text) => {
            event.insert(BODY_MEMBER.to_string(), Value::from(text));
            event.insert(BASE64_MEMBER.to_string(), Value::from(false));
        }
        Err(_) => {
            event.insert(
                BODY_MEMBER.to_string(),
                Value::from(::aws_smithy_types::base64::encode(body)),
            );
            event.insert(BASE64_MEMBER.to_string(), Value::from(true));
        }
    }
    Value::Object(event)
}

/// The headers the event reports: the request's own, and the protocol and port of the
/// listener it arrived on, which the load balancer reports itself. A value the load balancer
/// reports REPLACES one the client sent under the same name — what is behind the load
/// balancer reads these to learn the hop the load balancer made, not what the client claimed.
fn forwarded_headers(
    forwarded: &Forwarded,
    request: &DataPlaneRequest,
) -> Vec<(String, String)> {
    let mut headers = request.headers.clone();
    let mut reported = |name: &str, value: String| {
        headers.retain(|(held, _)| held != name);
        headers.push((name.to_string(), value));
    };
    if let Some(protocol) = forwarded.protocol.as_deref() {
        reported(FORWARDED_PROTO_HEADER, protocol.to_ascii_lowercase());
    }
    if let Some(port) = forwarded.port {
        reported(FORWARDED_PORT_HEADER, port.to_string());
    }
    headers
}

/// The query string as the pairs it carries, in the order it carries them: `key=value`
/// pairs separated by `&`, each exactly as the client wrote it — a parameter carrying no `=`
/// has the empty value, and nothing is decoded, because what reaches the target is what
/// arrived.
///
/// The SAME reading a `query-string` rule condition is evaluated against
/// ([`super::data_plane`]), which splits the request's query on `&` and each pair on its
/// first `=` and compares the halves as written: one reading of a query string, so what a
/// rule matched on and what the target is handed cannot disagree.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            let (key, value) = raw.split_once('=').unwrap_or((raw, ""));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// `pairs` as the single-valued member: one entry per NAME, carrying the last value the
/// request gave it — the value a reader of the single-valued member sees for a name the
/// request repeated.
fn single_valued(pairs: &[(String, String)]) -> Value {
    let mut object = Map::new();
    for (name, value) in pairs {
        object.insert(name.clone(), Value::from(value.clone()));
    }
    Value::Object(object)
}

/// `pairs` as the multi-valued member: one entry per name, carrying EVERY value the request
/// gave it, in order.
fn multi_valued(pairs: &[(String, String)]) -> Value {
    let mut object: Map<String, Value> = Map::new();
    for (name, value) in pairs {
        match object.get_mut(name).and_then(Value::as_array_mut) {
            Some(values) => values.push(Value::from(value.clone())),
            None => {
                object.insert(name.clone(), Value::Array(vec![Value::from(value.clone())]));
            }
        }
    }
    Value::Object(object)
}

/// The response the object a function returned describes, or the reason it describes none.
///
/// Every refusal here is one the LOAD BALANCER answers: the object is the contract between
/// the two, so a payload that is not one is not passed through to the client.
fn mapped_response(payload: &[u8], multi_value_headers: bool) -> Result<Served, String> {
    let Ok(Value::Object(returned)) = ::serde_json::from_slice::<Value>(payload) else {
        return Err("the response is not a JSON object".to_string());
    };
    let status = status_of(&returned)?;
    let mut headers = headers_of(&returned)?;
    // A header the object names more than once is sent more than once, which is how several
    // cookies travel; the multi-valued member is what carries them, and a group that does not
    // exchange arrays with its function has none to read.
    if multi_value_headers {
        headers.extend(multi_value_headers_of(&returned)?);
    }
    Ok(Served {
        status,
        content_type: None,
        location: None,
        headers,
        body: body_of(&returned)?,
        ..Served::default()
    })
}

/// The status the returned object names, refused when it is not a number inside the range a
/// response status can take (RFC 9110 §15: three digits, 100 through 599) — the member every
/// response has to carry.
fn status_of(returned: &Map<String, Value>) -> Result<u16, String> {
    let named = returned.get(STATUS_CODE_MEMBER).cloned().unwrap_or_default();
    let status = match &named {
        Value::Number(number) => number.as_i64(),
        // A function that states the status as a string states one too: `"200"` is the status
        // 200, which is what a handler written in a dynamically typed runtime produces when
        // it formats the value.
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    let status = status.ok_or_else(|| {
        format!("the response names no readable {STATUS_CODE_MEMBER}: {named}")
    })?;
    u16::try_from(status)
        .ok()
        .filter(|status| (100..=599).contains(status))
        .ok_or_else(|| format!("the response names the status {status}, which is no HTTP response status"))
}

/// The headers the returned object names, by lower-cased name. A member that is not an
/// object of header values is a response the contract does not admit, and so is a value that
/// is not text: a header field is text.
fn headers_of(returned: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
    let Some(named) = returned.get(HEADERS_MEMBER) else {
        return Ok(Vec::new());
    };
    if named.is_null() {
        return Ok(Vec::new());
    }
    let Some(object) = named.as_object() else {
        return Err(format!("the response's {HEADERS_MEMBER} is not an object"));
    };
    object
        .iter()
        .map(|(name, value)| {
            header_value(value)
                .map(|value| (name.to_ascii_lowercase(), value))
                .ok_or_else(|| {
                    format!("the response's {HEADERS_MEMBER} names {name:?} as no header value")
                })
        })
        .collect()
}

/// The headers the returned object names under [`MULTI_VALUE_HEADERS_MEMBER`], each value as
/// its own header field.
fn multi_value_headers_of(returned: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
    let Some(named) = returned.get(MULTI_VALUE_HEADERS_MEMBER) else {
        return Ok(Vec::new());
    };
    if named.is_null() {
        return Ok(Vec::new());
    }
    let Some(object) = named.as_object() else {
        return Err(format!(
            "the response's {MULTI_VALUE_HEADERS_MEMBER} is not an object"
        ));
    };
    let mut headers = Vec::new();
    for (name, values) in object {
        let Some(values) = values.as_array() else {
            return Err(format!(
                "the response's {MULTI_VALUE_HEADERS_MEMBER} names {name:?} as no list of \
                 header values"
            ));
        };
        for value in values {
            let value = header_value(value).ok_or_else(|| {
                format!(
                    "the response's {MULTI_VALUE_HEADERS_MEMBER} names {name:?} as no list of \
                     header values"
                )
            })?;
            headers.push((name.to_ascii_lowercase(), value));
        }
    }
    Ok(headers)
}

/// One header value as it is sent: text as itself, and a number or a boolean as its own
/// rendering, which is what a handler setting one means.
fn header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The body the returned object names: the text as it is, or the bytes it decodes to when
/// the function marked it base64-encoded. An object naming no body has an empty one — the
/// body is the one member a Lambda target's response may leave out — and a body that is not
/// text, or a flag that is not a boolean, is a response the contract does not admit.
fn body_of(returned: &Map<String, Value>) -> Result<Vec<u8>, String> {
    let encoded = match returned.get(BASE64_MEMBER) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(other) => {
            return Err(format!("the response's {BASE64_MEMBER} is not a boolean: {other}"));
        }
    };
    let body = match returned.get(BODY_MEMBER) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(text)) => text.clone(),
        Some(other) => {
            return Err(format!("the response's {BODY_MEMBER} is not text: {other}"));
        }
    };
    if !encoded {
        return Ok(body.into_bytes());
    }
    ::aws_smithy_types::base64::decode(body.trim()).map_err(|error| {
        format!("the response marks its {BODY_MEMBER} base64-encoded and it is not base64: {error}")
    })
}

/// Whether the resource policy of `function` admits the invocation a load balancer makes for
/// `source_arn` — the ARN of the TARGET GROUP the request was forwarded to, which is the
/// resource a grant's `aws:SourceArn` narrows such a permission to.
///
/// `true` also when nothing about the function could be shown, which refuses nothing: a
/// registration whose function Lambda does not answer about at all, and an emulator with no
/// Lambda mounted, are not evidence of a missing grant. A function whose policy Lambda
/// REPORTS as absent is an answer rather than a silence: it grants nothing.
async fn invocation_admitted(
    client: &::aws_sdk_lambda::Client,
    function: &str,
    source_arn: &str,
) -> bool {
    // The RESOURCE a grant is taken on is the function's own ARN as Lambda reports it, so it
    // is resolved there rather than assumed to be what the registration carried. A function
    // Lambda answers nothing about is one this module judges nothing about: the invocation
    // below is what then decides the request.
    let resolved = match client
        .get_function_configuration()
        .function_name(function)
        .send()
        .await
    {
        Ok(reported) => reported.function_arn.unwrap_or_else(|| function.to_string()),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the registered function was not judged: nothing answered for its \
                 configuration"
            );
            return true;
        }
    };
    let document = match client.get_policy().function_name(&resolved).send().await {
        Ok(reported) => reported.policy.unwrap_or_default(),
        Err(error) => {
            if !refused_by_lambda(&error) {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the registered function was not judged: nothing answered for its \
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
fn lambda_client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The WAIT is bounded without one: the run this invocation makes is
        // bounded by the function's own `Timeout` ("the amount of time that Lambda allows a
        // function to run before stopping it"), which Lambda enforces, so the answer comes
        // back however long the handler tries to take.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    //! The two pure halves of the contract: the event one request becomes, and the response
    //! one returned object describes. What the plane SERVES is pinned end to end in
    //! `tests/all/alb_lambda_target.rs`.

    use super::*;

    /// One forwarded request, for a group that exchanges arrays or does not.
    fn forwarded(multi_value_headers: bool) -> Forwarded {
        Forwarded {
            function_arn: "arn:aws:lambda:us-west-2:000000000000:function:f".to_string(),
            target_group_arn: "arn:aws:elasticloadbalancing:us-west-2:000000000000:\
                               targetgroup/tg/0123456789abcdef"
                .to_string(),
            multi_value_headers,
            protocol: Some("HTTP".to_string()),
            port: Some(80),
            set_cookie: None,
        }
    }

    /// The request a row builds the event from.
    fn request(query: &str) -> DataPlaneRequest {
        DataPlaneRequest {
            method: "GET".to_string(),
            path: "/ok".to_string(),
            query: query.to_string(),
            host: "my-alb-1.us-west-2.elb.amazonaws.com".to_string(),
            headers: vec![(
                "host".to_string(),
                "my-alb-1.us-west-2.elb.amazonaws.com".to_string(),
            )],
            port: Some(80),
        }
    }

    /// The event names the target group, the request, and the hop the load balancer made —
    /// in the single-valued members, or in the multi-valued ones for a group whose attribute
    /// is on.
    #[test]
    fn the_event_carries_the_request_and_the_group_it_was_forwarded_to() {
        let rows: &[(&str, bool, &str, &str)] = &[
            ("single-valued, one value per name", false, "a=1&b=2", r#"{"a":"1","b":"2"}"#),
            ("single-valued, a repeated name keeps the last value", false, "a=1&a=2", r#"{"a":"2"}"#),
            ("multi-valued, one list per name", true, "a=1&b=2", r#"{"a":["1"],"b":["2"]}"#),
            ("multi-valued, a repeated name keeps every value", true, "a=1&a=2", r#"{"a":["1","2"]}"#),
            ("a parameter with no value", false, "flag", r#"{"flag":""}"#),
            ("no query string at all", false, "", "{}"),
        ];
        for (case, multi, query, expected) in rows {
            let forwarded = forwarded(*multi);
            let event = event(&forwarded, &request(query), b"");
            let member = if *multi {
                "multiValueQueryStringParameters"
            } else {
                "queryStringParameters"
            };
            let seen = event.get(member).cloned().unwrap_or_default().to_string();
            assert_eq!(
                seen, *expected,
                "case {case:?}: expected {member} {expected}, got {seen}"
            );
            let group = event
                .pointer("/requestContext/elb/targetGroupArn")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                group, forwarded.target_group_arn,
                "case {case:?}: the event names the target group it was forwarded to, got \
                 {group:?}"
            );
            let proto = event
                .get(if *multi { MULTI_VALUE_HEADERS_MEMBER } else { HEADERS_MEMBER })
                .and_then(|headers| headers.get(FORWARDED_PROTO_HEADER))
                .cloned()
                .unwrap_or_default()
                .to_string();
            let expected_proto = if *multi { r#"["http"]"# } else { r#""http""# };
            assert_eq!(
                proto, expected_proto,
                "case {case:?}: the event reports the listener's protocol, got {proto}"
            );
        }
    }

    /// A body whose bytes are not text travels base64-encoded, with the flag set.
    #[test]
    fn a_body_travels_as_text_or_as_base64() {
        let rows: &[(&str, &[u8], &str, bool)] = &[
            ("text", b"hello", "hello", false),
            ("empty", b"", "", false),
            ("bytes no text carries", &[0xff, 0xfe], "//4=", true),
        ];
        for (case, body, expected, encoded) in rows {
            let event = event(&forwarded(false), &request(""), body);
            let seen = event.get(BODY_MEMBER).and_then(Value::as_str).unwrap_or_default();
            let flag = event.get(BASE64_MEMBER).and_then(Value::as_bool);
            assert_eq!(seen, *expected, "case {case:?}: expected body {expected:?}, got {seen:?}");
            assert_eq!(
                flag,
                Some(*encoded),
                "case {case:?}: expected {BASE64_MEMBER} {encoded}, got {flag:?}"
            );
        }
    }

    /// One row of the response table: the case, the payload the function returned, and the
    /// status and body it must map to — `None` for a payload the contract refuses.
    type ResponseRow<'a> = (&'a str, &'a str, Option<(u16, &'a str)>);

    /// The response the returned object describes, and the objects the contract refuses.
    #[test]
    fn a_response_is_mapped_only_when_it_satisfies_the_contract() {
        // Rows span the contract: a well-formed response with and without a body, the status
        // written as text, every member the contract names of a wrong shape, and each class
        // of payload that is no response object at all.
        let rows: &[ResponseRow<'_>] = &[
            (
                "a status, headers and a body",
                r#"{"statusCode":200,"headers":{"Content-Type":"text/plain"},"body":"TARGET-OK"}"#,
                Some((200, "TARGET-OK")),
            ),
            ("no body, which the contract allows", r#"{"statusCode":204}"#, Some((204, ""))),
            ("the status as text", r#"{"statusCode":"418","body":"short"}"#, Some((418, "short"))),
            (
                "a base64-encoded body",
                r#"{"statusCode":200,"isBase64Encoded":true,"body":"aGk="}"#,
                Some((200, "hi")),
            ),
            ("no statusCode at all", r#"{"body":"TARGET-OK"}"#, None),
            ("a statusCode outside the HTTP range", r#"{"statusCode":99}"#, None),
            ("a statusCode that is no number", r#"{"statusCode":"two hundred"}"#, None),
            ("headers that are not an object", r#"{"statusCode":200,"headers":[1]}"#, None),
            (
                "a header value no field carries",
                r#"{"statusCode":200,"headers":{"x":{"a":1}}}"#,
                None,
            ),
            ("a body that is not text", r#"{"statusCode":200,"body":{"a":1}}"#, None),
            (
                "a body marked base64 that is not base64",
                r#"{"statusCode":200,"isBase64Encoded":true,"body":"not base64 at all"}"#,
                None,
            ),
            ("isBase64Encoded that is no boolean", r#"{"statusCode":200,"isBase64Encoded":"yes"}"#, None),
            ("a payload that is not JSON", "not json at all", None),
            ("a JSON value that is no object", r#"["statusCode", 200]"#, None),
            ("an empty payload", "", None),
        ];
        for (case, payload, expected) in rows {
            let mapped = mapped_response(payload.as_bytes(), false);
            match expected {
                Some((status, body)) => {
                    let served = mapped.as_ref().unwrap_or_else(|reason| {
                        panic!("case {case:?}: expected a mapped response, got {reason:?}")
                    });
                    let seen = String::from_utf8_lossy(&served.body).into_owned();
                    assert_eq!(
                        (served.status, seen.as_str()),
                        (*status, *body),
                        "case {case:?}: expected ({status}, {body:?}), got ({}, {seen:?})",
                        served.status
                    );
                }
                None => assert!(
                    mapped.is_err(),
                    "case {case:?}: expected the contract to refuse it, got {:?}",
                    mapped.map(|served| served.status)
                ),
            }
        }
    }

    /// The multi-valued response member is read only for a group that exchanges arrays, and
    /// each value it names becomes its own header field.
    #[test]
    fn multi_value_response_headers_are_read_for_a_group_that_exchanges_arrays() {
        let payload = r#"{"statusCode":200,"multiValueHeaders":{"Set-Cookie":["a=1","b=2"]}}"#;
        for (case, multi, expected) in [
            ("a group that exchanges arrays", true, 2_usize),
            ("a group that does not", false, 0),
        ] {
            let served = mapped_response(payload.as_bytes(), multi)
                .unwrap_or_else(|reason| panic!("case {case:?}: expected a response, got {reason:?}"));
            let cookies = served
                .headers
                .iter()
                .filter(|(name, _)| name == "set-cookie")
                .count();
            assert_eq!(
                cookies, expected,
                "case {case:?}: expected {expected} set-cookie fields, got {cookies}"
            );
        }
        let refused = mapped_response(
            br#"{"statusCode":200,"multiValueHeaders":{"Set-Cookie":"a=1"}}"#,
            true,
        );
        assert!(
            refused.is_err(),
            "case a multi-valued member that is no list: expected the contract to refuse it, \
             got {:?}",
            refused.map(|served| served.status)
        );
    }
}
