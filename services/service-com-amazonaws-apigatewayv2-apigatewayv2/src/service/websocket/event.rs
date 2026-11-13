// SPDX-License-Identifier: CC0-1.0

//! The EVENT a WEBSOCKET api hands a Lambda function: the proxy event of each of the three
//! calls a connection makes, and the authorizer event the connection request's `REQUEST`
//! authorizer is called with.
//!
//! A WebSocket api has no `PayloadFormatVersion` to choose a format with — "Required for
//! HTTP APIs" (`Integration$PayloadFormatVersion`) — because its routes have ONE event
//! shape, the one a connection's three calls differ only inside:
//!
//! * the CONNECTION REQUEST, `requestContext.eventType` `CONNECT` on the `$connect` route,
//!   which is the only call that carries the HANDSHAKE — its headers and its query string,
//!   the two things a client can only say while connecting;
//! * each MESSAGE, `MESSAGE` on the route the api's `RouteSelectionExpression` selected,
//!   carrying the frame as the event's `body` and nothing of the handshake: a frame has no
//!   headers and no query string of its own;
//! * the CLOSE, `DISCONNECT` on the `$disconnect` route, which reports how the connection
//!   ended.
//!
//! Every member the event AWS documents for a call STATES is stated, empty where the
//! connection has nothing for it (the stage's variables, the handshake's query string), so a
//! handler that reads one finds what it finds at AWS rather than nothing at all.
//!
//! Everything in an event comes from the connection the handshake established, from the
//! frame the client sent and from the api's own stored records. Nothing is invented: the
//! `connectionId` is the one this connection was minted, which is the id the `@connections`
//! management API addresses it by, and the same connection reports the same `connectedAt`
//! for every call it makes. The two ids a call has — `requestId` (`extendedRequestId`) and,
//! for a message, `messageId` — are minted per call, the way the gateway mints them.
//!
//! The `requestContext.authorizer` an authorized connection carries is the SAME member the
//! HTTP data plane's `1.0` event carries ([`crate::service::invoke::proxy_event`]), so a
//! handler reads an authorizer's context the one way whichever plane invoked it.
//!
//! The HEADER NAMES an event reports are the ones the CLIENT SENT, with their own spelling:
//! a WebSocket event's `headers` is the map API Gateway documents with `Host`,
//! `Sec-WebSocket-Key` and `X-Forwarded-For` written as the handshake wrote them, so a
//! handler reads a header under the name it was sent under. Nothing here looks a header up
//! by that spelling, though — a field name is case-insensitive (RFC 9110 §5.1), so every
//! reading below goes through [`field`].

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::serde_json::{json, Map, Value};

use crate::service::invoke::proxy_event::{self, Invocation};

/// The `requestContext.eventType` of each of a connection's three calls: "CONNECT",
/// "MESSAGE" and "DISCONNECT" are the values API Gateway reports for the `$connect` route,
/// a message route and the `$disconnect` route.
const CONNECT_EVENT_TYPE: &str = "CONNECT";
const MESSAGE_EVENT_TYPE: &str = "MESSAGE";
const DISCONNECT_EVENT_TYPE: &str = "DISCONNECT";

/// The `requestContext.messageDirection` of every call a CLIENT makes: inbound. (The other
/// direction is a payload pushed to the connection, which is not a route's call at all.)
const INBOUND_DIRECTION: &str = "IN";

/// The close status a client that closed WITHOUT one is reported under: RFC 6455 §7.4.1
/// reserves 1005 for "no status code was actually present", which is what `$disconnect`
/// reports for such a close.
const NO_CLOSE_STATUS: u16 = 1005;

/// The `type` an authorizer event names its own kind by, for the one authorizer type a
/// WEBSOCKET api admits: "REQUEST … for a Lambda function using incoming request
/// parameters" (`Authorizer$AuthorizerType`).
const REQUEST_AUTHORIZER_TYPE: &str = "REQUEST";

/// The address a connection on a core-owned loopback listener is attributed to; see
/// [`super::transport`].
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// The header a client names the host it addressed in, which an event reports as the
/// `domainName` the connection was made to.
const HOST_HEADER: &str = "host";

/// The header a client names itself in, which the event reports under `identity`.
const USER_AGENT_HEADER: &str = "user-agent";

/// One ESTABLISHED (or, during the handshake, offered) connection, as every event of it
/// reports it.
///
/// It is read out of the api's own records and the opening handshake ONCE, when the
/// connection is admitted, and then carried for the connection's whole life: that is what
/// makes every call of one connection report the same `connectionId`, the same
/// `connectedAt` and the same authorizer context.
#[derive(Debug, Clone)]
pub struct Connection {
    /// The api this socket serves.
    pub api_id: String,
    /// The stage the handshake addressed.
    pub stage_name: String,
    /// The id this connection was minted, which the `@connections` management API
    /// addresses it by.
    pub connection_id: String,
    /// The Region the api is reached in.
    pub region: String,
    /// When the connection was established.
    pub connected_at: SystemTime,
    /// The opening handshake's headers, under the names the client sent them with.
    pub headers: BTreeMap<String, String>,
    /// The opening handshake's query parameters, a repeated name comma-joined.
    pub query_parameters: BTreeMap<String, String>,
    /// The serving stage's variables.
    pub stage_variables: BTreeMap<String, String>,
    /// The `context` the connection request's authorizer returned, carried into every later
    /// call's event — which is the only way a route's handler reads what the authorizer
    /// decided.
    pub authorizer_context: Option<Value>,
    /// The principal a policy-document authorizer named.
    pub authorizer_principal: Option<String>,
}

impl Connection {
    /// The host the client addressed, which the event reports as `domainName`: the
    /// handshake's own `Host`, and the api's `execute-api` host for a client that sent none.
    pub fn domain_name(&self) -> String {
        field(&self.headers, HOST_HEADER)
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| crate::state::execute_api_host(&self.api_id, &self.region))
    }

    /// What the client named itself as, which the event reports under `identity`.
    pub fn user_agent(&self) -> Option<&str> {
        field(&self.headers, USER_AGENT_HEADER)
    }

    /// What the gateway knows about one call of this connection, as the shared
    /// `requestContext.authorizer` rendering reads it.
    fn invocation(&self, request_id: &str) -> Invocation {
        Invocation {
            account_id: ::engine::emulator_account_id().to_string(),
            request_id: request_id.to_string(),
            received: SystemTime::now(),
            authorizer_context: self.authorizer_context.clone(),
            authorizer_principal: self.authorizer_principal.clone(),
            admitted_jwt: None,
        }
    }
}

/// The value `headers` carries for the field `name`, compared case-insensitively the way a
/// field name is (RFC 9110 §5.1) — because the map is keyed by the spelling the client used.
pub fn field<'h>(headers: &'h BTreeMap<String, String>, name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Which call of a connection an event is about.
#[derive(Debug, Clone)]
pub enum Call {
    /// The connection request: the opening handshake itself.
    Connect,
    /// One message the client sent, as the frame's bytes read as text.
    Message(String),
    /// The close, with the status and reason the client's close frame carried.
    Disconnect {
        status_code: Option<u16>,
        reason: Option<String>,
    },
}

impl Call {
    /// The `eventType` this call is reported under.
    fn event_type(&self) -> &'static str {
        match self {
            Call::Connect => CONNECT_EVENT_TYPE,
            Call::Message(_) => MESSAGE_EVENT_TYPE,
            Call::Disconnect { .. } => DISCONNECT_EVENT_TYPE,
        }
    }

    /// Whether this call carries the handshake, which is what decides whether the event
    /// reports headers and a query string: only the connection request and the close report
    /// the request the connection was opened with.
    fn carries_the_handshake(&self) -> bool {
        matches!(self, Call::Connect | Call::Disconnect { .. })
    }
}

/// The event the route `route_key` of this connection is invoked with for `call`.
///
/// The call's own id is minted here, the way the gateway mints one per call. A caller that
/// has to REPORT that id beside the event — the access-log record of the same call — states
/// it instead ([`event_of`]), so the record and the event the integration observed name the
/// same call.
pub fn event(connection: &Connection, route_key: &str, call: &Call) -> Value {
    event_of(connection, route_key, call, &proxy_event::mint_request_id())
}

/// [`event`] with the call's own `requestId` stated rather than minted.
pub fn event_of(
    connection: &Connection,
    route_key: &str,
    call: &Call,
    request_id: &str,
) -> Value {
    let mut event = Map::new();
    if call.carries_the_handshake() {
        event.insert(
            "headers".to_string(),
            proxy_event::single_valued(&connection.headers),
        );
        event.insert(
            "multiValueHeaders".to_string(),
            proxy_event::multi_valued(&connection.headers),
        );
    }
    if matches!(call, Call::Connect) {
        event.insert(
            "queryStringParameters".to_string(),
            proxy_event::optional_map(&connection.query_parameters),
        );
        event.insert(
            "multiValueQueryStringParameters".to_string(),
            proxy_event::optional_multi_map(&connection.query_parameters),
        );
    }
    if call.carries_the_handshake() {
        // Both of these are STATED, empty when the stage carries no variables, because the
        // event API Gateway documents for a connection request states them: a handler that
        // reads `event["stageVariables"]` finds the map it has at AWS rather than nothing.
        event.insert(
            "stageVariables".to_string(),
            json!(connection.stage_variables),
        );
    }
    event.insert(
        "requestContext".to_string(),
        request_context(connection, route_key, call, request_id),
    );
    if let Call::Message(body) = call {
        event.insert("body".to_string(), json!(body));
    }
    // A frame reaches this data plane as text (the bytes of a binary frame are read as the
    // text they spell), so what the event carries is never a base64-encoded payload.
    event.insert("isBase64Encoded".to_string(), json!(false));
    Value::Object(event)
}

/// The `$context` values of one call, keyed by the member path a MAPPING TEMPLATE writes
/// them (`routeKey`, `connectionId`, `identity.sourceIp`, `authorizer.<key>`, …).
///
/// The same set the event's own `requestContext` reports — "`$context` … contains the
/// contextual information of your request" is one set of values per call, whether the call
/// is handed to a function as an event or read by a template — so it is FLATTENED out of
/// [`request_context`] rather than listed a second time: a template and a handler cannot
/// disagree about what the gateway knows about a call.
pub fn template_context(
    connection: &Connection,
    route_key: &str,
    call: &Call,
) -> BTreeMap<String, String> {
    template_context_of(connection, route_key, call, &proxy_event::mint_request_id())
}

/// [`template_context`] with the call's own `requestId` stated rather than minted, for a
/// reader that has to name the same call the event named — the stage's access-log record of
/// it ([`super::access_logs`]).
pub fn template_context_of(
    connection: &Connection,
    route_key: &str,
    call: &Call,
    request_id: &str,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    flattened(
        "",
        &request_context(connection, route_key, call, request_id),
        MAX_CONTEXT_DEPTH,
        &mut values,
    );
    values
}

/// How deep the walk below descends, which is how deep a `$context` member path goes: the
/// members the gateway reports are values, beside the two MAPS one level inside the context
/// (`identity`, and the `authorizer` an authorized connection carries), and an authorizer's
/// own context carries no object or array as a value — so nothing deeper is a member path a
/// template can name. Bounded rather than open because that authorizer context is a value a
/// caller's own function returned.
const MAX_CONTEXT_DEPTH: usize = 3;

/// `value` written into `values` under the member path a template names it by: a nested
/// object's members are reached through the dotted path their nesting gives them
/// (`identity.sourceIp`), and a member the call has no value for at all (`null`) is left out
/// rather than written as the empty string — a template reference to it then resolves to
/// nothing, the way a reference to a member the gateway does not report does.
fn flattened(path: &str, value: &Value, depth: usize, values: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(members) => {
            let Some(deeper) = depth.checked_sub(1) else {
                return;
            };
            for (name, member) in members {
                let nested = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                flattened(&nested, member, deeper, values);
            }
        }
        Value::Null => {}
        // A string's own characters, and every other scalar as the text its JSON spells —
        // which is what a template renders a number or a boolean of the context as.
        Value::String(text) => {
            values.insert(path.to_string(), text.clone());
        }
        other => {
            values.insert(path.to_string(), other.to_string());
        }
    }
}

/// The `requestContext` of one call: what the gateway reports about the connection and
/// about this call of it.
fn request_context(
    connection: &Connection,
    route_key: &str,
    call: &Call,
    request_id: &str,
) -> Value {
    let now = SystemTime::now();
    let mut context = Map::new();
    context.insert("routeKey".to_string(), json!(route_key));
    context.insert("eventType".to_string(), json!(call.event_type()));
    if let Call::Disconnect {
        status_code,
        reason,
    } = call
    {
        context.insert(
            "disconnectStatusCode".to_string(),
            json!(status_code.unwrap_or(NO_CLOSE_STATUS)),
        );
        if let Some(reason) = reason.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            context.insert("disconnectReason".to_string(), json!(reason));
        }
    }
    context.insert("extendedRequestId".to_string(), json!(request_id));
    context.insert(
        "requestTime".to_string(),
        json!(proxy_event::common_log_time(now)),
    );
    context.insert("messageDirection".to_string(), json!(INBOUND_DIRECTION));
    context.insert("stage".to_string(), json!(connection.stage_name));
    context.insert(
        "connectedAt".to_string(),
        json!(proxy_event::epoch_millis(connection.connected_at)),
    );
    context.insert(
        "requestTimeEpoch".to_string(),
        json!(proxy_event::epoch_millis(now)),
    );
    context.insert(
        "identity".to_string(),
        json!({
            "sourceIp": LOOPBACK_SOURCE_IP,
            "userAgent": connection.user_agent(),
        }),
    );
    context.insert("requestId".to_string(), json!(request_id));
    context.insert("domainName".to_string(), json!(connection.domain_name()));
    context.insert("connectionId".to_string(), json!(connection.connection_id));
    context.insert("apiId".to_string(), json!(connection.api_id));
    if let Some(authorizer) = proxy_event::authorizer_1_0(&connection.invocation(request_id)) {
        context.insert("authorizer".to_string(), authorizer);
    }
    if matches!(call, Call::Message(_)) {
        // One message of a connection, identified apart from the call that carried it:
        // `messageId` is what a handler correlates a frame by, and only a message has one.
        context.insert("messageId".to_string(), json!(proxy_event::mint_request_id()));
    }
    Value::Object(context)
}

/// The event the connection request's `REQUEST` authorizer is called with: the connection
/// request's own event, plus the two members an authorizer event adds — the `type` of
/// authorizer deciding, and the ARN of the route it is deciding about.
///
/// It carries no `body` member of its own because a connection request has no body at all,
/// and it is the event of the ROUTE the authorizer fronts, so a function that reads
/// `requestContext.connectionId` sees the connection it is admitting.
pub fn authorizer_event(connection: &Connection, route_key: &str, route_arn: &str) -> Value {
    let mut event = event(connection, route_key, &Call::Connect);
    if let Value::Object(members) = &mut event {
        members.insert("type".to_string(), json!(REQUEST_AUTHORIZER_TYPE));
        members.insert("methodArn".to_string(), json!(route_arn));
        // An authorizer event reports no body at all, so the flag that describes one is not
        // on it either — the same shape the HTTP plane's authorizer event has.
        members.remove("isBase64Encoded");
    }
    event
}

#[cfg(test)]
mod tests {
    //! What each call's event reports, over the inputs the rule generalises over: the three
    //! calls, a connection with and without a handshake to report, and an authorizer
    //! decision carried into a later call. The events reaching a real function are driven
    //! end to end in `tests/all/websocket_lambda_routes.rs`.

    use super::*;

    /// A connection of one api, carrying `authorizer_context` as its authorizer's decision.
    fn connection(authorizer_context: Option<Value>) -> Connection {
        // A policy-document authorizer names a principal beside its context, so a row that
        // carries one carries both.
        let authorizer_principal = authorizer_context.as_ref().map(|_| "user".to_string());
        Connection {
            api_id: "abc1230000".to_string(),
            stage_name: "prod".to_string(),
            connection_id: "L0SM9cOFyjUCIhA=".to_string(),
            region: "eu-west-1".to_string(),
            connected_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000),
            headers: BTreeMap::from([
                ("host".to_string(), "sockets.example.test".to_string()),
                ("user-agent".to_string(), "suite".to_string()),
            ]),
            query_parameters: BTreeMap::from([("token".to_string(), "t0ken".to_string())]),
            stage_variables: BTreeMap::from([("tenant".to_string(), "acme".to_string())]),
            authorizer_context,
            authorizer_principal,
        }
    }

    #[test]
    fn each_call_of_a_connection_reports_its_own_kind_and_the_same_connection() {
        let connection = connection(None);
        // The rows span the three calls a connection makes: what each reports, and what
        // only one of them does.
        let cases: [(&str, &str, Call, &str, Option<&str>); 4] = [
            (
                "the connection request",
                "$connect",
                Call::Connect,
                "CONNECT",
                None,
            ),
            (
                "a message the route selection matched",
                "ping",
                Call::Message(r#"{"action":"ping"}"#.to_string()),
                "MESSAGE",
                Some(r#"{"action":"ping"}"#),
            ),
            (
                "a close carrying a status",
                "$disconnect",
                Call::Disconnect {
                    status_code: Some(1000),
                    reason: Some("done".to_string()),
                },
                "DISCONNECT",
                None,
            ),
            (
                "a close carrying none",
                "$disconnect",
                Call::Disconnect {
                    status_code: None,
                    reason: None,
                },
                "DISCONNECT",
                None,
            ),
        ];
        for (case, route_key, call, expected_type, expected_body) in cases {
            let is_connect = matches!(call, Call::Connect);
            let is_message = matches!(call, Call::Message(_));
            let closed_without_status = matches!(
                call,
                Call::Disconnect {
                    status_code: None,
                    ..
                }
            );
            let event = event(&connection, route_key, &call);
            let member = |pointer: &str| event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                member("/requestContext/eventType"),
                json!(expected_type),
                "case {case:?}: expected eventType {expected_type}, got {event}"
            );
            assert_eq!(
                member("/requestContext/routeKey"),
                json!(route_key),
                "case {case:?}: expected routeKey {route_key}, got {event}"
            );
            assert_eq!(
                member("/requestContext/connectionId"),
                json!("L0SM9cOFyjUCIhA="),
                "case {case:?}: every call reports the connection's own id, got {event}"
            );
            assert_eq!(
                member("/requestContext/connectedAt"),
                json!(1_700_000_i64),
                "case {case:?}: every call reports when the connection was made, got {event}"
            );
            assert_eq!(
                member("/body"),
                expected_body.map_or(Value::Null, |body| json!(body)),
                "case {case:?}: expected body {expected_body:?}, got {event}"
            );
            assert_eq!(
                member("/queryStringParameters/token").is_null(),
                !is_connect,
                "case {case:?}: only the connection request reports the handshake's query \
                 string, got {event}"
            );
            assert_eq!(
                member("/headers/host").is_null(),
                is_message,
                "case {case:?}: a message carries no headers of its own, got {event}"
            );
            assert_eq!(
                member("/requestContext/messageId").is_null(),
                !is_message,
                "case {case:?}: only a message has a messageId, got {event}"
            );
            if matches!(call, Call::Disconnect { .. }) {
                let expected = if closed_without_status { 1005 } else { 1000 };
                assert_eq!(
                    member("/requestContext/disconnectStatusCode"),
                    json!(expected),
                    "case {case:?}: expected disconnectStatusCode {expected}, got {event}"
                );
            }
            assert_eq!(
                member("/requestContext/domainName"),
                json!("sockets.example.test"),
                "case {case:?}: the event reports the host the client addressed, got {event}"
            );
            assert!(
                member("/requestContext/authorizer").is_null(),
                "case {case:?}: no authorizer decided this connection, got {event}"
            );
        }
    }

    #[test]
    fn an_authorizers_context_reaches_every_later_call_of_the_connection() {
        let connection = connection(Some(json!({ "tier": "gold" })));
        // The rows span the calls that follow the decision: the connection request itself
        // and each later call, all of which report what the authorizer returned.
        let cases: [(&str, &str, Call); 3] = [
            ("the connection request", "$connect", Call::Connect),
            (
                "a message route",
                "ping",
                Call::Message(r#"{"action":"ping"}"#.to_string()),
            ),
            (
                "the close",
                "$disconnect",
                Call::Disconnect {
                    status_code: None,
                    reason: None,
                },
            ),
        ];
        for (case, route_key, call) in cases {
            let event = event(&connection, route_key, &call);
            assert_eq!(
                event
                    .pointer("/requestContext/authorizer/tier")
                    .cloned()
                    .unwrap_or(Value::Null),
                json!("gold"),
                "case {case:?}: expected the authorizer's own context member, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/requestContext/authorizer/principalId")
                    .cloned()
                    .unwrap_or(Value::Null),
                json!("user"),
                "case {case:?}: expected the principal the authorizer named, got {event}"
            );
        }
    }

    #[test]
    fn the_template_context_is_the_same_set_the_events_request_context_reports() {
        // An authorized connection, so the `authorizer` map an authorizer returned is part of
        // the context a template reads.
        let connection = connection(Some(json!({ "tier": "gold" })));
        let call = Call::Message(r#"{"action":"ping"}"#.to_string());
        let context = template_context(&connection, "ping", &call);
        let event = event(&connection, "ping", &call);
        // The rows span what the flattening generalises over: a member the gateway reports as
        // text, one it reports as a number, a member of a nested map, a member of the map an
        // authorizer returned, and a path no member of the context has.
        let cases: [(&str, &str, Option<&str>); 6] = [
            ("the route that served the call", "routeKey", Some("ping")),
            ("the kind of call", "eventType", Some("MESSAGE")),
            ("the connection", "connectionId", Some("L0SM9cOFyjUCIhA=")),
            ("a nested member", "identity.sourceIp", Some("127.0.0.1")),
            ("a member the authorizer returned", "authorizer.tier", Some("gold")),
            ("a member no call has", "nothingHere", None),
        ];
        for (what, path, expected) in cases {
            assert_eq!(
                context.get(path).map(String::as_str),
                expected,
                "case {what:?}: expected {path} to be {expected:?}, got {context:?}"
            );
        }
        // A number is rendered as the text its JSON spells, and it is the SAME value the
        // event reports for that member — one context per call.
        let reported = event
            .pointer("/requestContext/connectedAt")
            .map(::serde_json::Value::to_string);
        assert_eq!(
            context.get("connectedAt").cloned(),
            reported,
            "case \"an instant of the connection\": expected the event's own value, got \
             {context:?}"
        );
        // Every member the event's `requestContext` reports is a path of the context, so the
        // two cannot disagree about WHICH members the gateway knows about (the ids and the
        // instants each call mints are its own, so what is compared is the set of paths).
        if let Some(Value::Object(members)) = event.pointer("/requestContext") {
            for (name, value) in members {
                let reported = match value {
                    Value::Object(_) => context.keys().any(|path| {
                        path.strip_prefix(name)
                            .is_some_and(|rest| rest.starts_with('.'))
                    }),
                    _ => context.contains_key(name),
                };
                assert!(
                    reported,
                    "case {name:?}: every member the event reports is a path of the context, \
                     got {context:?}"
                );
            }
        }
    }

    #[test]
    fn an_authorizer_context_nested_deeper_than_a_member_path_is_left_out() {
        // The value a caller's own authorizer function returned, nested past the one level
        // `$context.authorizer.<key>` names.
        let connection = connection(Some(json!({
            "flat": "value",
            "deep": { "deeper": { "deepest": "value" } }
        })));
        let context = template_context(&connection, "ping", &Call::Message("{}".to_string()));
        assert_eq!(
            context.get("authorizer.flat").map(String::as_str),
            Some("value"),
            "a member of the authorizer's context is a member path, got {context:?}"
        );
        assert!(
            context
                .keys()
                .all(|path| !path.starts_with("authorizer.deep.deeper")),
            "a value nested deeper than a member path is left out, got {context:?}"
        );
    }

    #[test]
    fn the_authorizer_event_names_the_route_it_decides_about() {
        let connection = connection(None);
        let arn = "arn:aws:execute-api:eu-west-1:000000000000:abc1230000/prod/$connect";
        let event = authorizer_event(&connection, "$connect", arn);
        for (member, pointer, expected) in [
            ("the authorizer kind", "/type", json!("REQUEST")),
            ("the route ARN", "/methodArn", json!(arn)),
            (
                "the connection being admitted",
                "/requestContext/connectionId",
                json!("L0SM9cOFyjUCIhA="),
            ),
            (
                "the identity source the handshake carried",
                "/queryStringParameters/token",
                json!("t0ken"),
            ),
        ] {
            let actual = event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {member:?}: expected {expected} at {pointer}, got {event}"
            );
        }
        assert!(
            event.pointer("/body").is_none(),
            "a connection request has no body, so its authorizer event carries none: {event}"
        );
    }
}
