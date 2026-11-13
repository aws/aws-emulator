// SPDX-License-Identifier: CC0-1.0

//! What a WEBSOCKET stage LOGS about one call of a connection: the access-log record its own
//! `AccessLogSettings` ask for.
//!
//! `Stage$AccessLogSettings` carries the `DestinationArn` ("the ARN of the CloudWatch Logs
//! log group to receive access logs") and the `Format` ("a single line format of the access
//! logs of data, as specified by selected `$context` variables"), and a WebSocket api's stage
//! is configured with exactly those two members — so a stage carrying both leaves ONE record
//! per ROUTE EXECUTION: the connection request, each message a client sends, and the close.
//! That is what makes a connect/disconnect pair auditable and a refused handshake tellable
//! from a session that ran.
//!
//! Everything a record says is read off the call itself. The `$context` values are the ones
//! the SAME call's event reported ([`super::event::template_context_of`]) — one set of values
//! per call, whether it is handed to a function as an event, read by a mapping template or
//! written into the log — with the values only the ANSWER can carry added here: the status
//! the gateway answered the call with, how many bytes it sent back, how long it took and the
//! sentence a refusal was worded in. A variable the call has no value for renders as the
//! placeholder `service_common` writes an unresolved field as, so a record is always a
//! complete line.
//!
//! The rendering and the write are `service_common`'s, so this plane, the HTTP plane
//! ([`crate::service::invoke::access_logs`]) and API Gateway's REST plane cannot render one
//! format three ways. The one thing a socket needs of its own is the TRANSPORT of the write:
//! a call carried on the api's own socket has no request and no `OperationContext`, so the
//! delivery is made over the in-process handle that socket was opened with
//! (`api_gateway_access_logs::delivered_over`).
//!
//! What ONE record can cost is bounded by what it carries: the `$context` of the call and the
//! stage's own `Format`, never a byte of the client's payload — a frame's body reaches the
//! route's integration and no record. The records a connection leaves are one per route
//! execution, which is strictly fewer than that connection's calls already leave in the same
//! CloudWatch Logs store through their own integrations' invocation logs, and they accumulate
//! there exactly as the HTTP plane's records do: retention of what that store holds is that
//! service's, not this plane's.

use std::time::{Duration, Instant};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::event::{self, Call, Connection};
// The lowest status that IS an error, which is what `$context.error.message` is about: the
// one reading of that this service has, shared with the HTTP plane's records.
use crate::service::invoke::access_logs::FIRST_ERROR_STATUS;
use crate::state::ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore;

/// ONE call of a connection, as the gateway answered it — the half of a record that only the
/// answer can carry.
#[derive(Debug, Clone)]
pub struct Served {
    /// Which of the connection's three calls it was, which decides the `eventType` the
    /// record reports and what else the call carries.
    pub call: Call,
    /// The `RouteKey` of the route that served it, empty for a call no route of the api
    /// served at all.
    pub route_key: String,
    /// The id of the call: the one the event the integration was handed reported, so the
    /// record and that invocation name ONE call.
    pub request_id: String,
    /// The status the GATEWAY answered the call with — the handshake's own status for a
    /// connection request, the status a message's route produced, the status the close's
    /// route produced — or `None` for a call nothing answered with a status at all.
    pub status: Option<u16>,
    /// How many bytes the gateway sent back to the client for this call, where it sent any.
    pub response_length: Option<usize>,
    /// How long the gateway took to serve the call.
    pub latency: Duration,
    /// The sentence a REFUSED call was answered with, where the gateway worded one.
    pub error_message: Option<String>,
}

/// Deliver the access-log record the stage this connection was established on asks for.
///
/// Called with the call already ANSWERED, because a record is about what the gateway did:
/// its status, the bytes it sent back and the time it took are not known before then.
///
/// Nothing here can fail the call — the client has already been answered — so a stage
/// carrying no `AccessLogSettings`, a destination that names no log group and a write
/// CloudWatch Logs refuses each leave no record and nothing else.
pub async fn delivered(
    state: &::engine::ServiceState<ServiceStore>,
    callback: &EmbeddedEmulator,
    connection: &Connection,
    served: &Served,
) {
    let Some(planned) = planned(state, connection, served) else {
        return;
    };
    ::service_common::api_gateway_access_logs::delivered_over(
        callback,
        &connection.region,
        &planned.log_group,
        &planned.log_stream,
        std::slice::from_ref(&planned.record),
    )
    .await;
}

/// One record to deliver: where it goes and what it says.
#[derive(Debug)]
struct Planned {
    log_group: String,
    log_stream: String,
    record: String,
}

/// The record this call leaves behind, read under ONE shared read of the store, which is
/// RELEASED before the delivery is made: a write into another service is a call, and no lock
/// of this service's is held while another service answers.
///
/// `None` when nothing is logged at all: a connection whose api or stage the store no longer
/// holds, a stage carrying no `AccessLogSettings` or no `Format`, and a destination that
/// names no log group (a Kinesis Data Firehose delivery stream).
fn planned(
    state: &::engine::ServiceState<ServiceStore>,
    connection: &Connection,
    served: &Served,
) -> Option<Planned> {
    let store = state.lock();
    let api = store.apis.get(&connection.api_id)?;
    let stage = api.stages.get(&connection.stage_name)?;
    let settings = stage.access_log_settings.as_ref()?;
    let destination = settings.destination_arn.as_deref()?;
    let format = settings.format.as_deref()?;
    let log_group = ::service_common::api_gateway_access_logs::destination_log_group(destination)?;
    Some(Planned {
        log_group: log_group.to_string(),
        log_stream: crate::service::invoke::access_logs::log_stream_name(
            &api.api_id,
            &stage.stage_name,
        ),
        record: ::service_common::api_gateway_access_logs::rendered(
            format,
            &context_of(connection, served),
        ),
    })
}

/// The `$context` values of one call, keyed by the member path an access-log format writes
/// after `$context.`.
///
/// The call's own values are the ones its EVENT reported, so a handler's
/// `requestContext.requestId` and the record's `$context.requestId` are the same id; what is
/// added here is what only the answer carries.
fn context_of(
    connection: &Connection,
    served: &Served,
) -> std::collections::BTreeMap<String, String> {
    let mut context = event::template_context_of(
        connection,
        &served.route_key,
        &served.call,
        &served.request_id,
    );
    // A call no route served carries no route key at all, rather than an empty field.
    if served.route_key.is_empty() {
        context.remove("routeKey");
    }
    // The account the api belongs to, which the event's own context does not report and an
    // access-log format may name.
    context.insert(
        "accountId".to_string(),
        ::engine::emulator_account_id().to_string(),
    );
    context.insert(
        "responseLatency".to_string(),
        served.latency.as_millis().to_string(),
    );
    if let Some(length) = served.response_length {
        context.insert("responseLength".to_string(), length.to_string());
    }
    if let Some(status) = served.status {
        context.insert("status".to_string(), status.to_string());
        // `$context.error.message` is the message a REFUSED call was answered with, so it is
        // stated for a status that IS an error and left unset otherwise, where there is no
        // error to name. `$context.error.messageString` is that same text quoted.
        if status >= FIRST_ERROR_STATUS {
            if let Some(message) = served.error_message.as_deref() {
                context.insert(
                    "error.messageString".to_string(),
                    ::serde_json::Value::String(message.to_string()).to_string(),
                );
                context.insert("error.message".to_string(), message.to_string());
            }
        }
    }
    context
}

/// How long the gateway took to serve a call that started at `started`.
///
/// Measured on the MONOTONIC clock, which is the one a span is measured on: a step of the
/// wall clock cannot make a record report a latency the call did not take.
#[must_use]
pub fn latency_since(started: Instant) -> Duration {
    started.elapsed()
}

#[cfg(test)]
mod tests {
    //! What a record SAYS, over the inputs the rule generalises over: the three calls, a
    //! status that is an error and one that is not, and a call no route served. The delivery
    //! itself is driven end to end against CloudWatch Logs in
    //! `tests/all/websocket_access_logging.rs`.

    use super::*;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    /// A connection of one api, as the handshake established it.
    fn connection() -> Connection {
        Connection {
            api_id: "a1b2c3d4e5".to_string(),
            stage_name: "prod".to_string(),
            connection_id: "L0SM9cOFyjUCIhA=".to_string(),
            region: "us-east-1".to_string(),
            connected_at: SystemTime::UNIX_EPOCH + Duration::from_millis(1_793_000_000_000),
            headers: [("Host".to_string(), "127.0.0.1:34567".to_string())]
                .into_iter()
                .collect(),
            query_parameters: BTreeMap::new(),
            stage_variables: BTreeMap::new(),
            authorizer_context: None,
            authorizer_principal: None,
        }
    }

    /// One row: the call served, and what the record must then carry.
    struct Row {
        case: &'static str,
        served: Served,
        /// The `(member, value)` pairs the record's context must carry.
        expected: &'static [(&'static str, &'static str)],
        /// The members it must carry none of.
        absent: &'static [&'static str],
    }

    #[test]
    fn a_record_reports_the_call_and_what_the_gateway_answered_it_with() {
        let rows = vec![
            Row {
                case: "a completed connection request",
                served: Served {
                    call: Call::Connect,
                    route_key: "$connect".to_string(),
                    request_id: "req-1".to_string(),
                    status: Some(101),
                    response_length: None,
                    latency: Duration::from_millis(7),
                    error_message: None,
                },
                expected: &[
                    ("eventType", "CONNECT"),
                    ("routeKey", "$connect"),
                    ("requestId", "req-1"),
                    ("extendedRequestId", "req-1"),
                    ("status", "101"),
                    ("connectionId", "L0SM9cOFyjUCIhA="),
                    ("stage", "prod"),
                    ("apiId", "a1b2c3d4e5"),
                    ("domainName", "127.0.0.1:34567"),
                    ("responseLatency", "7"),
                ],
                absent: &["error.message", "responseLength", "messageId"],
            },
            Row {
                case: "a message served by a two-way route",
                served: Served {
                    call: Call::Message("{\"action\":\"ping\"}".to_string()),
                    route_key: "ping".to_string(),
                    request_id: "req-2".to_string(),
                    status: Some(200),
                    response_length: Some(12),
                    latency: Duration::from_millis(3),
                    error_message: None,
                },
                expected: &[
                    ("eventType", "MESSAGE"),
                    ("routeKey", "ping"),
                    ("status", "200"),
                    ("responseLength", "12"),
                    ("messageDirection", "IN"),
                ],
                absent: &["error.message"],
            },
            Row {
                case: "a message the api routed nowhere",
                served: Served {
                    call: Call::Message("{\"action\":\"nope\"}".to_string()),
                    route_key: String::new(),
                    request_id: "req-3".to_string(),
                    status: Some(403),
                    response_length: Some(64),
                    latency: Duration::from_millis(1),
                    error_message: Some("Forbidden".to_string()),
                },
                expected: &[
                    ("eventType", "MESSAGE"),
                    ("status", "403"),
                    ("error.message", "Forbidden"),
                    ("error.messageString", "\"Forbidden\""),
                ],
                absent: &["routeKey"],
            },
            Row {
                case: "the close",
                served: Served {
                    call: Call::Disconnect {
                        status_code: Some(1000),
                        reason: Some("done".to_string()),
                    },
                    route_key: "$disconnect".to_string(),
                    request_id: "req-4".to_string(),
                    status: None,
                    response_length: None,
                    latency: Duration::from_millis(2),
                    error_message: None,
                },
                expected: &[
                    ("eventType", "DISCONNECT"),
                    ("routeKey", "$disconnect"),
                    ("disconnectStatusCode", "1000"),
                    ("disconnectReason", "done"),
                ],
                absent: &["status", "error.message"],
            },
        ];
        for row in rows {
            let case = row.case;
            let context = context_of(&connection(), &row.served);
            for (member, value) in row.expected {
                assert_eq!(
                    context.get(*member).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected {member} to be {value:?}, got {:?}",
                    context.get(*member)
                );
            }
            for member in row.absent {
                assert!(
                    !context.contains_key(*member),
                    "case {case:?}: expected no {member}, got {:?}",
                    context.get(*member)
                );
            }
        }
    }

    #[test]
    fn a_format_renders_the_members_the_call_carries_and_the_placeholder_for_the_rest() {
        let served = Served {
            call: Call::Connect,
            route_key: "$connect".to_string(),
            request_id: "req-9".to_string(),
            status: Some(101),
            response_length: None,
            latency: Duration::from_millis(4),
            error_message: None,
        };
        let context = context_of(&connection(), &served);
        // The rows differ along the format: the JSON shape AWS publishes for a WebSocket
        // stage, a single reference, and a reference to a member no call of this plane
        // carries.
        let cases: &[(&str, &str, &str)] = &[
            (
                "the published json shape",
                r#"{"requestId":"$context.requestId","eventType":"$context.eventType","status":"$context.status"}"#,
                r#"{"requestId":"req-9","eventType":"CONNECT","status":"101"}"#,
            ),
            ("one reference", "$context.connectionId", "L0SM9cOFyjUCIhA="),
            ("a member no call carries", "$context.authorizer.error", "-"),
        ];
        for (case, format, expected) in cases {
            let record = ::service_common::api_gateway_access_logs::rendered(format, &context);
            assert_eq!(
                record, *expected,
                "case {case:?}: expected {expected:?}, got {record:?}"
            );
        }
    }
}
