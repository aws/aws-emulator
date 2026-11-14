// SPDX-License-Identifier: CC0-1.0

//! What a stage LOGS about a request it served: the stage's own access-log record, and the
//! execution log its method settings turn on.
//!
//! Both are CloudWatch Logs deliveries API Gateway makes on the account's behalf, so both
//! need the same two things to be configured, and neither happens without them:
//!
//! * the ACCOUNT's CloudWatch role. `Account$cloudwatchRoleArn` is "the ARN of an Amazon
//!   CloudWatch role for the current Account", which is the role API Gateway assumes to
//!   write into the account's log groups: an account that has never had one set gives API
//!   Gateway nothing to write as, so a stage of it serves its traffic and delivers no
//!   record at all. That is the whole difference between a stage whose logging is wired and
//!   one whose account step was skipped, and it is read here rather than at the control
//!   plane, which accepts a logging configuration whatever the account carries;
//! * the STAGE's own configuration. `Stage$accessLogSettings` carries the
//!   `destinationArn` a record is written to and the `format` it is rendered by, and
//!   `Stage$methodSettings` carries the `loggingLevel`/`dataTrace` that turn the EXECUTION
//!   log on for a method. A stage carrying neither logs nothing; each is independent of
//!   the other, so a stage may carry one without the other.
//!
//! # The access log
//!
//! One record per request the stage served — including a request the api itself refused,
//! which is a request the stage served an answer to — rendered from the stage's own format
//! against this call's `$context` values ([`super::call_context`], the same values a mapping
//! template of the call reads, with what the ANSWER came to added beside them). The
//! rendering and the write are `service_common`'s: the HTTP api plane of API Gateway
//! configures and renders access logging the same way, so one format cannot be rendered two
//! ways.
//!
//! Each stage writes into the destination IT names, so two stages of one api pointed at two
//! groups never cross: the destination is read off the stage the request reached.
//!
//! # The execution log
//!
//! `MethodSetting$loggingLevel` is "specifies the logging level for this method, which
//! affects the log entries pushed to Amazon CloudWatch Logs" with "valid values are `OFF`,
//! `ERROR`, and `INFO`", and `MethodSetting$dataTrace` is "specifies whether data trace
//! logging is enabled for this method, which affects the log entries pushed to Amazon
//! CloudWatch Logs". So the setting in force for the method the request reached decides:
//!
//! * `INFO` — the trace of the request is written;
//! * `ERROR` — only a request the stage answered with an error has a trace written, which
//!   is what logging errors only means;
//! * `OFF`, or no setting at all — nothing is written.
//!
//! `dataTrace` adds the request's and the response's own payloads to that trace, which is
//! the data the setting exists to gate: turned off, the trace names the sizes and not the
//! bytes.
//!
//! The group is the one API Gateway publishes for a stage's execution log,
//! `API-Gateway-Execution-Logs_{rest-api-id}/{stage-name}` ([`execution_log_group`]), and
//! the trace is worded exactly as `TestInvokeMethod` words the log of a simulated
//! invocation: one vocabulary, so a developer reading a stage's execution log and one
//! reading a test invocation's see the same steps.
//!
//! The trace is the steps the stage TOOK, so a request it could not serve through the
//! method's integration — a configuration this api cannot be performed through, a grant the
//! backend does not carry — carries the step that stopped it: the configuration error that
//! was found and the gateway response the request was then answered through ([`Unperformed`]).
//! A developer reads that line to tell a backend that answered badly from one that was never
//! reached at all, and a status alone does not say which.

use std::collections::BTreeMap;
use std::time::Duration;

use super::{RtRequest, RtResponse};
use smithy_server_runtime::OperationContext;

/// What serving ONE request left for the logs the stage delivers once it has been answered.
///
/// Filled by the serving path as it decides the request and read here, because neither half
/// of it can be recovered from the response: the values an AUTHORIZATION published are not in
/// the body, and a refusal's own gateway response type is not either.
#[derive(Debug, Default)]
pub(super) struct Recorded {
    /// What this request's own AUTHORIZATION published about the caller — a custom
    /// authorizer's `principalId` and context values, a user pool's claims — each under the
    /// `$context` member path a reference reads it by, which a record's format may name as
    /// `$context.authorizer.<member>`; empty for a request no authorizer decided.
    pub(super) authorized: BTreeMap<String, String>,
    /// The configuration error that stopped the method's integration, for a request the
    /// stage could not serve through it; `None` for every request whose integration was
    /// performed, cached or never reached.
    pub(super) unperformed: Option<Unperformed>,
}

/// An integration the stage could not perform, as the execution log records it.
#[derive(Debug)]
pub(super) struct Unperformed {
    /// Why it could not be performed, in one sentence: `UnservedIntegration`'s own reason,
    /// which states the configuration and never a backend answer.
    reason: String,
    /// The `GatewayResponseType` the request was answered through, which is what names the
    /// CLASS of the configuration error to a developer reading the trace.
    response_type: &'static str,
}

impl Recorded {
    /// Record that `unserved` is what stopped this request's integration.
    pub(super) fn stopped_by(&mut self, unserved: &super::UnservedIntegration) {
        self.unperformed = Some(Unperformed {
            reason: unserved.reason.clone(),
            response_type: unserved.refusal.response_type(),
        });
    }
}

/// The prefix API Gateway publishes every stage's execution log group under:
/// `API-Gateway-Execution-Logs_{rest-api-id}/{stage-name}`.
const EXECUTION_LOG_GROUP_PREFIX: &str = "API-Gateway-Execution-Logs_";

/// The `loggingLevel` that writes the whole trace of a request.
const LOGGING_LEVEL_INFO: &str = "INFO";

/// The `loggingLevel` that writes the trace of a request the stage answered with an error.
const LOGGING_LEVEL_ERROR: &str = "ERROR";

/// The lowest status that IS an error, which is what [`LOGGING_LEVEL_ERROR`] writes a trace
/// for: the 4xx and 5xx classes RFC 9110 defines as the client-error and server-error
/// classes.
const FIRST_ERROR_STATUS: u16 = 400;

/// The execution log group of the stage `stage` of the api `api_id`.
fn execution_log_group(api_id: &str, stage: &str) -> String {
    format!("{EXECUTION_LOG_GROUP_PREFIX}{api_id}/{stage}")
}

/// The stream a stage's records of one UTC day are written into: that day, the stage and the
/// api, so the records of one stage of one api accumulate in one stream per day and two
/// stages pointed at ONE destination still write two streams.
///
/// The day is taken from the one timestamp rendering this service has — the date half of the
/// `date-time` form — rather than from a second calendar here.
fn log_stream_name(api_id: &str, stage: &str) -> String {
    let stamp = crate::state::now()
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let day = stamp.split('T').next().unwrap_or_default().to_string();
    format!("{day}/[{stage}]{api_id}")
}

/// What one served request leaves in CloudWatch Logs, read out of the store in ONE pass: the
/// Region each delivery is made in, and the group, stream and records of each of the two
/// logs this stage writes.
#[derive(Debug, Default)]
struct Planned {
    /// The Region the api's own ARN names, which is where its log groups live.
    region: String,
    /// The stage's access-log destination and the record rendered for this request, when
    /// the stage carries `accessLogSettings` naming a log group and a format.
    access: Option<(String, String)>,
    /// The stage's execution log group and the trace of this request, when the method
    /// settings in force turn execution logging on.
    execution: Option<(String, Vec<String>)>,
    /// The stream both deliveries are written into ([`log_stream_name`]).
    stream: String,
}

impl Planned {
    /// Whether this request leaves anything at all, which is what decides whether the
    /// deliveries below are made.
    fn is_empty(&self) -> bool {
        self.access.is_none() && self.execution.is_none()
    }
}

/// Deliver what the stage this request reached logs about it.
///
/// Called from the ONE funnel every invoke of this data plane leaves through, with the
/// response the stage answered in hand: an access-log record is about a request that was
/// ANSWERED, so its status, its size and the latency of answering it are part of the record
/// and cannot be known before then.
///
/// `latency` is how long the whole invoke took, which is what `$context.responseLatency`
/// ("the response latency in ms") reports.
///
/// `recorded` is what serving this request left for its logs: what its own AUTHORIZATION
/// published about the caller, and the configuration error that stopped its integration where
/// one did ([`Recorded`]).
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
    recorded: &Recorded,
) {
    let planned = planned(engine, req, request_id, response, latency, recorded);
    let Some(planned) = planned.filter(|planned| !planned.is_empty()) else {
        return;
    };
    if let Some((group, record)) = &planned.access {
        ::service_common::api_gateway_access_logs::delivered(
            ctx,
            &planned.region,
            group,
            &planned.stream,
            std::slice::from_ref(record),
        )
        .await;
    }
    if let Some((group, trace)) = &planned.execution {
        ::service_common::api_gateway_access_logs::delivered(
            ctx,
            &planned.region,
            group,
            &planned.stream,
            trace,
        )
        .await;
    }
}

/// What this request leaves behind, read under ONE shared read of the store, which is
/// RELEASED before any delivery is made: a write into another service is a call, and no lock
/// of this service's is held while another service answers.
///
/// `None` when nothing is logged at all: an account with no CloudWatch role for API Gateway
/// to write as, a request that reached no stage of an api this service holds, or a stage
/// carrying neither logging configuration.
fn planned<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    request_id: &str,
    response: &RtResponse,
    latency: Duration,
    recorded: &Recorded,
) -> Option<Planned> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    // The account's CloudWatch role is what API Gateway writes as; see the module docs.
    store.account.cloudwatch_role_arn.as_ref()?;
    let resolved = super::resolve(&store, &super::Addressed::of_request(req))?;
    let api = resolved.api;
    let (stage_name, resource_path) = super::split_stage(resolved.invoke_path)?;
    let stage = api.stages.get(stage_name)?;
    // WHICH deployed method the request reached, where it reached one: the access log names
    // the resource path of the matched method and the verb it was served under, and the
    // execution log is gated by the settings in force for exactly that method. A request
    // that matched none — a path this deployment does not carry — is still a request the
    // stage answered, so it is logged under the path and verb the CLIENT addressed.
    let deployment = api.deployments.get(&stage.deployment_id);
    let matched = deployment.and_then(|deployment| {
        let path = super::match_resource_path(deployment, &resource_path)?;
        let (verb, _) = super::match_method(deployment, path, req.method())?;
        Some((path, verb))
    });
    let (matched_path, verb) = matched.unwrap_or((resource_path.as_str(), req.method()));
    let status = response.status().as_u16();
    let mut context = super::call_context(
        &api.id,
        stage_name,
        verb,
        matched_path,
        req,
        request_id,
        ::std::time::SystemTime::now(),
    );
    // What the request's own AUTHORIZATION published, under the same `$context` member paths
    // the call's mapping templates read it by: `$context.authorizer.principalId` is "the
    // principal user identification associated with the token sent by the client and returned
    // from an API Gateway Lambda authorizer function", which is an access-log variable of its
    // own, and a user pool's claims are read as `$context.authorizer.claims.<name>`. A
    // request no authorizer decided publishes nothing, so such a reference stays unresolved.
    context.extend(
        recorded
            .authorized
            .iter()
            .map(|(member, value)| (member.clone(), value.clone())),
    );
    answered_context(&mut context, response, latency);

    let access = stage
        .access_log_settings
        .as_ref()
        .and_then(|settings| {
            let destination = settings.destination_arn.as_deref()?;
            let format = settings.format.as_deref()?;
            let group =
                ::service_common::api_gateway_access_logs::destination_log_group(destination)?;
            Some((
                group.to_string(),
                ::service_common::api_gateway_access_logs::rendered(format, &context),
            ))
        });

    let execution = super::response_cache::method_settings_in_force(stage, matched_path, verb)
        .filter(|settings| traces(settings.logging_level.as_deref(), status))
        .map(|settings| {
            (
                execution_log_group(&api.id, stage_name),
                trace(
                    req,
                    response,
                    request_id,
                    verb,
                    matched_path,
                    deployment
                        .and_then(|deployment| {
                            deployment
                                .resources
                                .values()
                                .find(|resource| resource.path == matched_path)
                        })
                        .and_then(|resource| resource.methods.get(verb))
                        .and_then(|method| method.integration.as_ref())
                        .and_then(|integration| integration.r#type.as_deref()),
                    recorded.unperformed.as_ref(),
                    settings.data_trace_enabled,
                ),
            )
        });

    Some(Planned {
        region: api.arn_region().to_string(),
        access,
        execution,
        stream: log_stream_name(&api.id, stage_name),
    })
}

/// Whether a method whose `loggingLevel` is `level` writes a trace of a request answered
/// with `status`; see the module docs.
fn traces(level: Option<&str>, status: u16) -> bool {
    match level {
        Some(LOGGING_LEVEL_INFO) => true,
        Some(LOGGING_LEVEL_ERROR) => status >= FIRST_ERROR_STATUS,
        _ => false,
    }
}

/// The `$context` values of the ANSWER, added beside the values of the call: what the stage
/// answered with, how large that answer was and how long answering took.
///
/// `$context.error.message` is the message a REFUSED call was answered with — every refusal
/// this data plane renders is a gateway response whose body carries a `message` member — so
/// it is read off the answer for a status that IS an error and left unset otherwise, where
/// there is no error to name. `$context.error.messageString` is "the quoted value of
/// `$context.error.message`", so it is that same text quoted.
fn answered_context(
    context: &mut BTreeMap<String, String>,
    response: &RtResponse,
    latency: Duration,
) {
    let status = response.status().as_u16();
    let body = response.body().bytes().unwrap_or_default();
    context.insert("status".to_string(), status.to_string());
    context.insert("responseLength".to_string(), body.len().to_string());
    context.insert(
        "responseLatency".to_string(),
        latency.as_millis().to_string(),
    );
    if status < FIRST_ERROR_STATUS {
        return;
    }
    let message = ::serde_json::from_slice::<::serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(|document| document.get("message"))
        .and_then(|message| message.as_str().map(str::to_string));
    if let Some(message) = message {
        context.insert(
            "error.messageString".to_string(),
            ::serde_json::Value::String(message.clone()).to_string(),
        );
        context.insert("error.message".to_string(), message);
    }
}

/// The execution log of one request: the steps the stage took to answer it, in the order it
/// took them, worded as `TestInvokeMethod` words the log of a simulated invocation.
///
/// One log EVENT per step, because that is how CloudWatch Logs holds a multi-line trace: a
/// client filtering the group for one step finds the event that step is. Every one of them
/// NAMES the request it is about, in the form API Gateway prefixes a record of a stage's
/// execution log with — the request's own id in parentheses ([`about_request`]) — because one
/// group holds the records of every request the stage served and a client reading it has
/// nothing else to separate two traces by.
///
/// `data_trace` adds the payloads of the call to the trace; without it the trace names their
/// sizes, exactly as a simulated invocation's log does ([`payload_line`]).
///
/// `unperformed` is the configuration error that stopped the integration, for a request the
/// stage could not serve through it: it is recorded where the integration's own step belongs —
/// after the type that was to answer and before what the caller was answered with — in the
/// two sentences a simulated invocation's log words that outcome with.
#[allow(clippy::too_many_arguments)]
fn trace(
    req: &RtRequest,
    response: &RtResponse,
    request_id: &str,
    verb: &str,
    resource_path: &str,
    integration_type: Option<&str>,
    unperformed: Option<&Unperformed>,
    data_trace: bool,
) -> Vec<String> {
    let request_body = req.body().bytes().unwrap_or_default();
    let response_body = response.body().bytes().unwrap_or_default();
    let mut lines = vec![
        format!("Execution log for request {request_id}"),
        format!("Starting execution for request: {request_id}"),
        format!("HTTP Method: {verb}, Resource Path: {resource_path}"),
        format!(
            "Method request headers: [{}]",
            header_names(req).join(", ")
        ),
        payload_line(
            "Method request body before transformations",
            request_body,
            data_trace,
        ),
        format!(
            "Integration type: {}",
            integration_type.unwrap_or("none")
        ),
    ];
    if let Some(unperformed) = unperformed {
        lines.push(format!(
            "Execution failed due to configuration error: {}",
            unperformed.reason
        ));
        lines.push(format!(
            "Answered through this API's {} gateway response",
            unperformed.response_type
        ));
    }
    lines.push(payload_line(
        "Method response body after transformations",
        response_body,
        data_trace,
    ));
    lines.push(format!(
        "Method completed with status: {}",
        response.status().as_u16()
    ));
    lines
        .into_iter()
        .map(|line| about_request(request_id, &line))
        .collect()
}

/// One record of a stage's execution log, naming the request it is about: API Gateway writes
/// each line of a request's trace prefixed with that request's own id in parentheses, which is
/// the id the response reported in its own header — so a client holding a response can find
/// exactly the records of the request it holds in a group that carries every request's.
fn about_request(request_id: &str, line: &str) -> String {
    format!("({request_id}) {line}")
}

/// Emulator resource guard, NOT an AWS limit: the most bytes of one payload a traced line
/// carries.
///
/// A payload's size is the CLIENT's choice, and a log event is itself bounded — CloudWatch
/// Logs takes at most one megabyte per event — so a payload past this is reported by its SIZE
/// (the line the trace writes when data tracing is off) rather than taking the whole trace
/// down with it. The figure is generous enough that no payload a method request carries in
/// practice is reported by its size instead of its bytes.
const MAX_TRACED_PAYLOAD_BYTES: usize = 8 * 1024;

/// One payload line of the trace: the BYTES of the payload where data tracing is on and the
/// payload is within [`MAX_TRACED_PAYLOAD_BYTES`], and its SIZE otherwise.
fn payload_line(what: &str, payload: &[u8], data_trace: bool) -> String {
    if data_trace && payload.len() <= MAX_TRACED_PAYLOAD_BYTES {
        return format!("{what}: {}", String::from_utf8_lossy(payload));
    }
    format!("{what}: {} byte(s)", payload.len())
}

/// The NAMES of the headers the request carries, in name order.
///
/// The names, not the values: a header is where a caller's authorization token or API key
/// travels, and an execution log is not where a caller's credential belongs — the same
/// reading `TestInvokeMethod`'s own log takes.
fn header_names(req: &RtRequest) -> Vec<String> {
    let mut names: Vec<String> = req
        .headers()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_execution_group_is_the_one_api_gateway_publishes_for_the_stage() {
        for (case, api, stage, expected) in [
            (
                "a stage of an api",
                "abc123",
                "prod",
                "API-Gateway-Execution-Logs_abc123/prod",
            ),
            (
                "another stage of the same api",
                "abc123",
                "dev",
                "API-Gateway-Execution-Logs_abc123/dev",
            ),
        ] {
            let got = execution_log_group(api, stage);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn the_logging_level_in_force_decides_which_requests_leave_a_trace() {
        for (case, level, status, expected) in [
            ("INFO logs a served request", Some("INFO"), 200, true),
            ("INFO logs a refused request", Some("INFO"), 403, true),
            ("ERROR logs no served request", Some("ERROR"), 200, false),
            ("ERROR logs a refused request", Some("ERROR"), 500, true),
            ("OFF logs nothing", Some("OFF"), 500, false),
            ("no level logs nothing", None, 500, false),
        ] {
            let got = traces(level, status);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected}, got {got}"
            );
        }
    }

    #[test]
    fn a_traced_payload_carries_its_bytes_within_the_guard_and_its_size_past_it() {
        let label = "Method request body before transformations";
        let at_the_bound = "x".repeat(MAX_TRACED_PAYLOAD_BYTES);
        let past_the_bound = "x".repeat(MAX_TRACED_PAYLOAD_BYTES + 1);
        for (case, payload, data_trace, expected) in [
            (
                "a small payload, traced",
                r#"{"id":"1"}"#,
                true,
                format!("{label}: {}", r#"{"id":"1"}"#),
            ),
            (
                "a small payload, not traced",
                r#"{"id":"1"}"#,
                false,
                format!("{label}: 10 byte(s)"),
            ),
            (
                "no payload at all, traced",
                "",
                true,
                format!("{label}: "),
            ),
            (
                "a payload at the guard, traced",
                at_the_bound.as_str(),
                true,
                format!("{label}: {at_the_bound}"),
            ),
            (
                "a payload past the guard, traced",
                past_the_bound.as_str(),
                true,
                format!("{label}: {} byte(s)", MAX_TRACED_PAYLOAD_BYTES + 1),
            ),
        ] {
            let got = payload_line(label, payload.as_bytes(), data_trace);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
        }
    }
}
