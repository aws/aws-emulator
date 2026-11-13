// SPDX-License-Identifier: CC0-1.0

//! What a stage LOGS about a request it served: the access-log record its own
//! `AccessLogSettings` ask for.
//!
//! `Stage$AccessLogSettings` carries the `DestinationArn` ("the ARN of the CloudWatch Logs
//! log group to receive access logs") and the `Format` ("a single line format of the access
//! logs of data, as specified by selected `$context` variables. The format must include at
//! least `$context.requestId`"), so a stage carrying both writes ONE record per request it
//! served — including a request the api itself refused, which is a request the stage served
//! an answer to — into the group that ARN names.
//!
//! The rendering and the write are `service_common`'s: the REST plane of API Gateway
//! configures and renders access logging the same way, so one format cannot be rendered two
//! ways. What this module owns is the one thing that plane cannot know — which `$context`
//! values an HTTP api's records produce for a call ([`context_of`]).
//!
//! An HTTP api needs no account-level CloudWatch role for this: that role is the REST
//! plane's own account setting (`apigateway`'s `Account$cloudwatchRoleArn`), and an HTTP
//! api's stage delivers through the service-linked role AWS creates for it, so the stage's
//! own configuration is the whole condition here.
//!
//! The record names the request by the id the RESPONSE reported ([`super::serve`] mints it
//! and the invocation's own event carries it), which is what lets a client join the answer
//! it holds to the record of it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::{RtRequest, RtResponse};
use smithy_server_runtime::OperationContext;

/// The lowest status that IS an error, which is what `$context.error.message` is about: the
/// 4xx and 5xx classes RFC 9110 defines as the client-error and server-error classes.
///
/// Shared with the WEBSOCKET plane's own records
/// ([`crate::service::websocket::access_logs`]): one reading of which statuses carry an error
/// to name, for both planes' records.
pub(crate) const FIRST_ERROR_STATUS: u16 = 400;

/// The request header the caller's user agent travels in, which `$context.identity.userAgent`
/// reports.
const USER_AGENT_HEADER: &str = "user-agent";

/// The request header the address the request came from is recorded in where it reached the
/// emulator through a forwarder.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The address a request that no forwarder recorded an address for came from: this emulator
/// is reached in process or over the loopback interface, so that is where such a request
/// arrived from.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// Deliver the access-log record the stage this request reached asks for.
///
/// Called from the ONE seam that serves this data plane with the response in hand: a record
/// is about a request that was ANSWERED, so its status, its size and the latency of
/// answering it are part of the record and cannot be known before then.
///
/// Nothing here can fail the request: the stage has already answered, so a delivery that
/// could not be made is traced by `service_common` and nothing else.
pub(super) async fn delivered<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    request_id: &str,
    response: &RtResponse,
    latency: Duration,
) {
    let Some(planned) = planned(engine, req, request_id, response, latency) else {
        return;
    };
    ::service_common::api_gateway_access_logs::delivered(
        ctx,
        &planned.region,
        &planned.log_group,
        &planned.log_stream,
        std::slice::from_ref(&planned.record),
    )
    .await;
}

/// One record to deliver: where it goes and what it says.
#[derive(Debug)]
struct Planned {
    /// The Region the api's own endpoint names, which is where its log groups live.
    region: String,
    log_group: String,
    log_stream: String,
    record: String,
}

/// The record this request leaves behind, read under ONE shared read of the store, which is
/// RELEASED before the delivery is made: a write into another service is a call, and no lock
/// of this service's is held while another service answers.
///
/// `None` when nothing is logged at all: a request that reached no stage of an api this
/// service holds, a stage carrying no `AccessLogSettings`, and a destination that names no
/// log group (a Kinesis Data Firehose delivery stream).
fn planned<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    request_id: &str,
    response: &RtResponse,
    latency: Duration,
) -> Option<Planned> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    let resolved = super::resolve(&store, &super::Addressed::of_request(req))?;
    let api = resolved.api;
    let (stage, route_path) = super::stage_of(api, resolved.invoke_path)?;
    let settings = stage.access_log_settings.as_ref()?;
    let destination = settings.destination_arn.as_deref()?;
    let format = settings.format.as_deref()?;
    let log_group = ::service_common::api_gateway_access_logs::destination_log_group(destination)?;
    // WHICH route the request matched, where it matched one: `$context.routeKey` is the key
    // of the route that served it, and a request that matched none carries none.
    let route_key = stage
        .deployment_id
        .as_deref()
        .and_then(|deployment_id| api.deployments.get(deployment_id))
        .and_then(|deployment| {
            super::matching_route(
                &deployment.configuration.routes,
                req.method(),
                &route_path,
            )
        })
        .map(|route| route.route_key.clone());
    let context = context_of(
        api,
        &stage.stage_name,
        route_key.as_deref(),
        req,
        request_id,
        response,
        latency,
    );
    Some(Planned {
        region: super::api_region(api),
        log_group: log_group.to_string(),
        log_stream: log_stream_name(&api.api_id, &stage.stage_name),
        record: ::service_common::api_gateway_access_logs::rendered(format, &context),
    })
}

/// The stream a stage's records of one UTC day are written into: that day, the stage and the
/// api, so the records of one stage of one api accumulate in one stream per day and two
/// stages pointed at ONE destination still write two streams.
///
/// The day is taken from the one timestamp rendering this service has — the date half of the
/// `date-time` form — rather than from a second calendar here.
///
/// Shared with the WEBSOCKET plane's own records ([`crate::service::websocket::access_logs`]):
/// API Gateway names the stream a stage's access log goes to the same way whichever plane the
/// stage belongs to.
pub(crate) fn log_stream_name(api_id: &str, stage: &str) -> String {
    let stamp = ::aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let day = stamp.split('T').next().unwrap_or_default().to_string();
    format!("{day}/[{stage}]{api_id}")
}

/// The `$context` values of this call, keyed by the member path an access log format writes
/// after `$context.`.
///
/// Only what the api's own records, the request and the ANSWER say about the call: which api
/// and stage served it, the route key it matched, the path it arrived at and the verb it
/// used, WHICH REQUEST it is and WHEN it arrived, the account the api belongs to, the
/// endpoint it was addressed on, the address it came from, and what it was answered with.
/// A member no value here names is one the format renders as the unresolved placeholder,
/// rather than a value invented for it.
fn context_of(
    api: &crate::state::Api,
    stage: &str,
    route_key: Option<&str>,
    req: &RtRequest,
    request_id: &str,
    response: &RtResponse,
    latency: Duration,
) -> BTreeMap<String, String> {
    let received = SystemTime::now();
    let status = response.status().as_u16();
    let body = response.body().bytes().unwrap_or_default();
    let domain_name = super::endpoint_host(api);
    let mut context: BTreeMap<String, String> = [
        ("accountId", ::engine::emulator_account_id().to_string()),
        ("apiId", api.api_id.clone()),
        ("domainName", domain_name),
        ("domainPrefix", api.api_id.clone()),
        ("httpMethod", req.method().to_string()),
        ("path", super::path_of(req.uri()).to_string()),
        ("requestId", request_id.to_string()),
        ("extendedRequestId", request_id.to_string()),
        (
            "requestTime",
            crate::service::invoke::proxy_event::common_log_time(received),
        ),
        ("requestTimeEpoch", epoch_millis(received).to_string()),
        ("stage", stage.to_string()),
        ("status", status.to_string()),
        ("responseLength", body.len().to_string()),
        ("responseLatency", latency.as_millis().to_string()),
        ("identity.sourceIp", source_ip(req)),
    ]
    .into_iter()
    .map(|(member, value)| (member.to_string(), value))
    .collect();
    if let Some(route_key) = route_key {
        context.insert("routeKey".to_string(), route_key.to_string());
    }
    if let Some(agent) = req.headers().get(USER_AGENT_HEADER) {
        context.insert("identity.userAgent".to_string(), agent.to_string());
    }
    // `$context.error.message` is the message a REFUSED call was answered with — every
    // refusal this data plane renders carries a `message` member in its body — so it is read
    // off the answer for a status that IS an error and left unset otherwise, where there is
    // no error to name. `$context.error.messageString` is that same text quoted.
    if status >= FIRST_ERROR_STATUS {
        if let Some(message) = ::serde_json::from_slice::<::serde_json::Value>(body)
            .ok()
            .as_ref()
            .and_then(|document| document.get("message"))
            .and_then(|message| message.as_str().map(str::to_string))
        {
            context.insert(
                "error.messageString".to_string(),
                ::serde_json::Value::String(message.clone()).to_string(),
            );
            context.insert("error.message".to_string(), message);
        }
    }
    context
}

/// The address this call came from, as `$context.identity.sourceIp` reports it: the FIRST
/// hop a forwarder recorded where the request reached the emulator through one, and the
/// loopback address otherwise — the same reading an invocation's own event takes.
fn source_ip(req: &RtRequest) -> String {
    req.headers()
        .get(FORWARDED_FOR_HEADER)
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or(LOOPBACK_SOURCE_IP)
        .to_string()
}

/// `at` in milliseconds since the epoch, which is the unit `requestTimeEpoch` reports.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}
