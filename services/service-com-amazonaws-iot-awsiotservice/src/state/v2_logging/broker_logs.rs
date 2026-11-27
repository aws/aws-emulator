// SPDX-License-Identifier: CC0-1.0

//! What the MESSAGE BROKER's session events are LOGGED as, under the account's V2 logging
//! options: which events are logged at all, what each entry says, and where it is delivered.
//!
//! V2 logging is not a report a caller asks for — it is a DELIVERY. `SetV2LoggingOptions`
//! configures it ("the ARN of the role that allows IoT to write to Cloudwatch logs", "the
//! default logging level", "if true all logs are disabled") and from then on the broker's own
//! activity is written into CloudWatch Logs, where an operator debugging a device, or a control
//! asserting that logging is on, reads it. A device's whole interaction with this emulator
//! happens on the broker's socket ([`::service_common::iot_mqtt_broker::wire`]) and not on any
//! request of this service's, so the events arrive here through the sink this module installs on
//! the broker's store ([`::service_common::iot_mqtt_broker::session_logs`]) — the same shape of
//! hand-off the rules engine is reached by ([`crate::state::rules_engine::broker_publications`]).
//!
//! What this module decides, per event:
//!
//! * WHETHER it is logged. An account with `disableAllLogs` set logs nothing at all. Otherwise
//!   the level that applies is the most specific one configured — an `eventConfigurations` entry
//!   for the event's own type, else the logging level set for a log target this session matches
//!   ([`crate::state::v2_logging_levels`], which "override account-level logging"), else
//!   `defaultLogLevel` — and the event is logged only when that level admits an event of its
//!   SEVERITY: a packet the broker served is high-level flow information (`INFO`) and one it
//!   refused is an error (`ERROR`), so a level of `ERROR` carries the refusals alone and
//!   `DISABLED` carries nothing.
//! * WHAT the entry says: the fields AWS publishes for a CloudWatch Logs entry of its message
//!   broker — the instant, the level, a trace id, the account, the outcome, the event type, the
//!   protocol, the client id, the topic and the socket the client connected from — each read off
//!   the event the broker reported.
//! * WHERE it goes: the service-owned group [`V2_LOG_GROUP`], unless the account's event
//!   configuration for that event type names a `logDestination`, which "overrides default Log
//!   Group for the specified event type".
//!
//! The `principalId` an entry names is the account's own answer and not the handshake's: the
//! broker's connections are terminated by the CORE's TLS server, which requests no client
//! certificate (`vendor/server/src/tls_certificate_authority.rs`), so no certificate reaches a
//! session here and the certificate a session is the account's device by is resolved from the
//! registry, the certificate store and the attachment relation instead
//! ([`session_principal`]) — and left out entirely where those records identify none. The
//! payload of a publication is not reported at all: an entry names the topic, which is what the
//! published entry format carries.
//!
//! LOCK ORDER: the options and the levels are read under ONE guard of this service's store,
//! which is RELEASED before the delivery — the write is a call into CloudWatch Logs, and no
//! guard of this service's is held across it.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::Arc;

use ::serde_json::{json, Map, Value};
use ::service_common::iot_mqtt_broker::session_logs::{SessionEvent, SessionLogSink};
use ::smithy_server_runtime::{EmbeddedEmulator, OperationContext};

use super::V2LoggingOptions;
use crate::state::v2_logging_levels::{LoggedTarget, LOG_TARGET_TYPE_DEFAULT};

/// The CloudWatch Logs log group AWS IoT delivers its V2 log entries into: the SERVICE-OWNED
/// group of the account, which exists because logging was turned on and not because a caller
/// created it.
///
/// It is the group the IoT troubleshooting documentation names and the only one an operator can
/// read broker activity in, so it is published under this name whatever the role's ARN says.
pub const V2_LOG_GROUP: &str = "AWSIotLogsV2";

/// The transport an entry reports as the protocol its event was spoken on; see
/// [`::service_common::iot_mqtt_broker::session_logs::MQTT_PROTOCOL`].
const MQTT_PROTOCOL: &str = ::service_common::iot_mqtt_broker::session_logs::MQTT_PROTOCOL;

/// The `LogLevel` values in order of what each one ADMITS, least verbose first — the order AWS
/// IoT's own logging documentation states them in: `DISABLED` logs nothing, `ERROR` logs the
/// errors, `WARN` adds the warnings, `INFO` adds the high-level flow and `DEBUG` adds the rest.
/// So a level admits an event whose own severity is at or below it in this list.
///
/// The values themselves are `LogLevel`'s (`smithy_query(query="shape",shape="LogLevel")`:
/// `DEBUG`, `INFO`, `ERROR`, `WARN`, `DISABLED`); the boundary validator refuses anything else
/// before a handler records it.
const LEVELS_BY_VERBOSITY: [&str; 5] = [DISABLED_LEVEL, ERROR_LEVEL, "WARN", INFO_LEVEL, "DEBUG"];

/// The level that logs NOTHING — see [`LEVELS_BY_VERBOSITY`].
pub const DISABLED_LEVEL: &str = "DISABLED";

/// The severity of an event the broker REFUSED: an operation that failed is an error, which is
/// what makes the refusals the one thing an `ERROR` level still carries.
pub const ERROR_LEVEL: &str = "ERROR";

/// The severity of an event the broker SERVED: high-level information about the flow of a
/// session, which is what `INFO` is for.
pub const INFO_LEVEL: &str = "INFO";

/// The status an entry reports for an event the broker served, and for one it refused.
const SUCCESS_STATUS: &str = "Success";
/// See [`SUCCESS_STATUS`].
const FAILURE_STATUS: &str = "Failure";

/// How verbose `level` is: its place in [`LEVELS_BY_VERBOSITY`], and `None` for a value that is
/// no `LogLevel` at all.
fn verbosity(level: &str) -> Option<usize> {
    LEVELS_BY_VERBOSITY
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(level))
}

/// The severity of one event: `ERROR` for a packet the broker refused, `INFO` for one it served.
#[must_use]
pub fn severity_of(served: bool) -> &'static str {
    if served {
        INFO_LEVEL
    } else {
        ERROR_LEVEL
    }
}

/// Whether an account logging at `configured` logs an event of severity `severity`.
///
/// A level admits its own severity and every more severe one ([`LEVELS_BY_VERBOSITY`]), so
/// `DEBUG` admits everything and `DISABLED` admits nothing. A `configured` value that is no
/// `LogLevel` admits nothing either: nothing was configured that could be read as a level.
#[must_use]
pub fn admits(configured: &str, severity: &str) -> bool {
    match (verbosity(configured), verbosity(severity)) {
        (Some(configured), Some(severity)) => configured > 0 && configured >= severity,
        _ => false,
    }
}

/// Whether the event configuration of `configured_type` is one that configures `event_type`.
///
/// `LogEventConfiguration$eventType` is "the type of event to log. These include event types
/// like Connect, Publish, and Disconnect", while the broker tells the two DIRECTIONS of a
/// publication apart (`Publish-In`, `Publish-Out`) — so a configuration naming the event's own
/// type configures it, and one naming the FAMILY before the direction configures both of its
/// directions. Compared case-insensitively, because the value is a caller-supplied string.
fn configures(configured_type: &str, event_type: &str) -> bool {
    if configured_type.eq_ignore_ascii_case(event_type) {
        return true;
    }
    event_type
        .split_once('-')
        .is_some_and(|(family, _direction)| configured_type.eq_ignore_ascii_case(family))
}

/// The LOG TARGETS one session matches, most specific first: the client id it connected under
/// and the address it connected from, then the account's own default target.
///
/// `LogTargetType` declares five types (`smithy_query(query="shape",shape="LogTargetType")`).
/// Three of them are resolved here, because a session is exactly what they name: `CLIENT_ID`,
/// `SOURCE_IP` and the account's own `DEFAULT`. `PRINCIPAL_ID` and `THING_GROUP` name facts a
/// connection to this broker does not carry (see this module's own note on the principal), so
/// a level set for one of those two decides nothing about a session's events.
fn session_targets(client_id: &str, source_ip: Option<&str>) -> Vec<LoggedTarget> {
    let mut targets = vec![LoggedTarget {
        target_type: "CLIENT_ID".to_string(),
        target_name: Some(client_id.to_string()),
    }];
    if let Some(address) = source_ip {
        targets.push(LoggedTarget {
            target_type: "SOURCE_IP".to_string(),
            target_name: Some(address.to_string()),
        });
    }
    targets.push(LoggedTarget {
        target_type: LOG_TARGET_TYPE_DEFAULT.to_string(),
        target_name: None,
    });
    targets
}

/// What the account's V2 logging configuration says about ONE event: the level it is logged at,
/// and the log group it is delivered into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedEntry {
    /// The level the entry reports as its own `logLevel`: the severity of the event, which the
    /// configured level admitted.
    pub level: String,
    /// The log group it goes into: [`V2_LOG_GROUP`], or the `logDestination` the account's
    /// event configuration for this event type names.
    pub log_group: String,
    /// Whether the group is the SERVICE-OWNED one, which IoT publishes itself — as opposed to a
    /// destination the account pointed this event type at, which IoT writes into and does not
    /// create.
    pub service_owned_group: bool,
}

/// What `options` and `levels` say about the event `event_type` of the session `client_id`
/// connected from `source_ip`, which the broker `served` or refused — and `None` for an event
/// that is not logged at all.
///
/// The decision, in the order AWS states the overrides in: an account with `disableAllLogs` set
/// logs nothing; otherwise the level that applies is the account's event configuration for this
/// event type, else the level set for the most specific log target this session matches
/// ([`session_targets`]), else `defaultLogLevel`; and the event is logged only when that level
/// admits an event of its own severity ([`admits`]). An account whose options state no level at
/// all logs nothing, exactly as one whose level is `DISABLED` does — a set carries the WHOLE
/// options document, so an account that logs at a level is one whose last set stated it.
#[must_use]
pub fn logged(
    options: &V2LoggingOptions,
    levels: &::std::collections::BTreeMap<LoggedTarget, String>,
    event_type: &str,
    client_id: &str,
    source_ip: Option<&str>,
    served: bool,
) -> Option<LoggedEntry> {
    if options.disable_all_logs {
        return None;
    }
    // The configuration that is about this event: the one naming the event's own type, and
    // otherwise the one naming the FAMILY of it ([`configures`]) — the most specific one the
    // account configured, so an account that configured both is read by the exact one.
    let configuration = options
        .event_configurations
        .iter()
        .find(|(configured_type, _)| configured_type.eq_ignore_ascii_case(event_type))
        .or_else(|| {
            options
                .event_configurations
                .iter()
                .find(|(configured_type, _)| configures(configured_type, event_type))
        })
        .map(|(_, configuration)| configuration);
    let level = configuration
        .and_then(|configuration| configuration.log_level.clone())
        .or_else(|| {
            session_targets(client_id, source_ip)
                .iter()
                .find_map(|target| levels.get(target).cloned())
        })
        .or_else(|| options.default_log_level.clone())
        .unwrap_or_else(|| DISABLED_LEVEL.to_string());
    let severity = severity_of(served);
    if !admits(&level, severity) {
        return None;
    }
    let destination = configuration.and_then(|configuration| configuration.log_destination.clone());
    Some(LoggedEntry {
        level: severity.to_string(),
        service_owned_group: destination.is_none(),
        log_group: destination.unwrap_or_else(|| V2_LOG_GROUP.to_string()),
    })
}

/// The log STREAM one entry is written in: the UTC day of the event.
///
/// CHOSEN: AWS publishes the group it delivers V2 log entries into and the fields of an entry,
/// but not how it names the streams inside that group. A day is what keeps the stream count
/// bounded by time rather than by a client — a stream per client id would let a device choose
/// how many streams the group holds — while leaving every entry of one day in one place for a
/// reader of the group. A reader looking for a session's entries filters the GROUP
/// (`FilterLogEvents`), which spans every stream in it.
#[must_use]
pub fn log_stream_of(at_millis: i64) -> String {
    let at = ::chrono::DateTime::from_timestamp_millis(at_millis).unwrap_or_default();
    at.format("%Y/%m/%d").to_string()
}

/// ONE log entry, as it is written into the group: the JSON document AWS IoT publishes for a
/// CloudWatch Logs entry of its message broker.
///
/// Every value is read off the event the broker reported, off the account's own records or off
/// this emulator's identity — the instant, the level the entry was admitted at, a trace id
/// minted for this entry, the account, the outcome, the event type, the protocol, the client id,
/// the certificate principal the session is the account's device by, the topic the event names
/// and the socket the client connected from. A field nothing is known for is LEFT OUT rather
/// than written empty, exactly as the broker's own presence events leave one out.
#[must_use]
pub fn log_entry(event: &SessionEvent, level: &str, principal_id: Option<&str>) -> String {
    let mut entry = json!({
        "timestamp": entry_timestamp(event.at_millis),
        "logLevel": level,
        // The id one entry is identified by, drawn fresh per entry: it identifies the ENTRY and
        // is never derived from the session or from the event.
        "traceId": ::uuid::Uuid::new_v4().to_string(),
        "accountId": ::engine::emulator_account_id(),
        "status": if event.served { SUCCESS_STATUS } else { FAILURE_STATUS },
        "eventType": event.event_type,
        "protocol": MQTT_PROTOCOL,
        "clientId": event.client_id,
    });
    let Some(members) = entry.as_object_mut() else {
        return entry.to_string();
    };
    if let Some(topic) = event.topic.as_deref() {
        members.insert("topicName".to_string(), json!(topic));
    }
    if let Some(principal_id) = principal_id {
        members.insert("principalId".to_string(), json!(principal_id));
    }
    if let Some(address) = event.source_ip.as_deref() {
        members.insert("sourceIp".to_string(), json!(address));
    }
    if event.source_port > 0 {
        members.insert("sourcePort".to_string(), json!(event.source_port));
    }
    if let Some(reason) = event.reason.as_deref() {
        members.insert("reason".to_string(), json!(reason));
    }
    Value::Object(Map::clone(members)).to_string()
}

/// The instant an entry reports, as AWS IoT's published entry format carries it: the UTC
/// calendar time to the millisecond (`2017-08-10 15:37:23.476`).
fn entry_timestamp(at_millis: i64) -> String {
    let at = ::chrono::DateTime::from_timestamp_millis(at_millis).unwrap_or_default();
    at.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// The policy action a CONNECTION is authorized by, as an IoT policy names it — the one the
/// `ActionType` `CONNECT` asks about ([`crate::state::iot_action_for_action_type`]).
const CONNECT_ACTION: &str = "iot:Connect";

/// The resource type segment a certificate principal is keyed under in the attachment relation
/// and on a thing's record, as [`crate::state::PolicyTarget::key`] and
/// [`crate::state::ThingPrincipal::key`] both spell it.
const CERTIFICATE_KEY_PREFIX: &str = "cert/";

/// How many certificate principals a session's principal is resolved against.
///
/// CHOSEN as a process bound, not an AWS quota: resolving a session's principal from the
/// account's policies evaluates the policies attached to each candidate certificate, and an
/// account's certificate store is one a CALLER grows — so without a ceiling the cost of logging
/// ONE broker event would be the product of that store and the policies on it. An account
/// holding more active certificates than this is not searched at all and its entries state no
/// principal, which is the same answer a search that could not single one out gives.
const MAX_RESOLVED_PRINCIPAL_CANDIDATES: usize = 64;

/// The PRINCIPAL one session of the broker holds, as the `principalId` of an entry about it:
/// the X.509 certificate the device that opened it is the account's device BY.
///
/// A connection to this broker cannot be asked which certificate it presented — the core's TLS
/// server terminates it without requesting a client certificate (see this module's own note) —
/// so the principal is resolved from the account's OWN records, which are what say which device
/// a client id belongs to, in the order AWS itself relates the two:
///
/// 1. the certificate attached to the THING the client id identifies. A thing's name is the
///    client id its device connects under ("The thing name... is also used as the default MQTT
///    client ID", `DescribeThing$defaultClientId`) and `AttachThingPrincipal` records "the
///    principal... X.509 certificates" of that thing, so a thing holding exactly one
///    certificate principal names the device's certificate;
/// 2. otherwise the certificate this account's POLICIES authorize to connect under that client
///    id: an IoT policy attached to a certificate is what lets it connect at all, and the
///    decision is the one `TestAuthorization` would give for `CONNECT` on the client id's own
///    `client/` ARN ([`crate::state::evaluate_authorization`]).
///
/// `None` wherever the account's records leave it ambiguous or say nothing — no such thing and
/// no certificate authorized, or more than one of either — because a principal nothing
/// identifies is one an entry must not state. A READ throughout: it decides from the registry,
/// the certificate store and the attachment relation and writes none of them, and the search it
/// makes is bounded by [`MAX_RESOLVED_PRINCIPAL_CANDIDATES`].
#[must_use]
pub fn session_principal(
    store: &crate::state::ComAmazonawsIotAwsiotserviceState,
    region: &str,
    client_id: &str,
) -> Option<String> {
    if let Some(thing) = store.things.get(client_id) {
        let mut attached = thing
            .principals
            .keys()
            .filter_map(|key| key.strip_prefix(CERTIFICATE_KEY_PREFIX));
        if let (Some(certificate_id), None) = (attached.next(), attached.next()) {
            return Some(certificate_id.to_string());
        }
    }
    // The candidates the account's policies are asked about: the certificates that are usable to
    // connect at all. Counted before any policy is read, so an account past the ceiling costs one
    // walk of the store's statuses rather than one policy evaluation per certificate.
    let candidates: Vec<&str> = store
        .certificates
        .values()
        .filter(|certificate| certificate.status == crate::state::CERTIFICATE_STATUS_ACTIVE)
        .map(|certificate| certificate.certificate_id.as_str())
        .take(MAX_RESOLVED_PRINCIPAL_CANDIDATES.saturating_add(1))
        .collect();
    if candidates.len() > MAX_RESOLVED_PRINCIPAL_CANDIDATES {
        return None;
    }
    // The question a connection is decided by, asked about the client id this session connected
    // under. The context is built over parts that name nothing — a session on the broker's wire
    // serves no request — exactly as the rules engine's own evaluation is.
    let parts = ::http::Request::new(()).into_parts().0;
    let connection = crate::state::SimulatedConnection::of(Some(client_id), &parts);
    let resource = ::emulator_server::emulator_arn("iot", region, &format!("client/{client_id}"));
    let mut authorized = candidates
        .into_iter()
        .filter(|certificate_id| {
            let scope =
                [crate::state::PolicyTarget::Certificate((*certificate_id).to_string()).key()];
            let policies = store.policies_in_effect(&scope, &[], &[]);
            !policies.is_empty()
                && crate::state::evaluate_authorization(
                    &policies,
                    Some(CONNECT_ACTION),
                    ::std::slice::from_ref(&resource),
                    &connection,
                )
                .decision
                    == crate::state::AUTH_DECISION_ALLOWED
        })
        .map(str::to_string);
    match (authorized.next(), authorized.next()) {
        (Some(certificate_id), None) => Some(certificate_id),
        _ => None,
    }
}

/// This service's V2 logging as the BROKER reaches it: everything one entry needs, and nothing
/// of any one request.
///
/// What it remembers is this EMULATOR and not a client: the shared engine the account's logging
/// options live in, the in-process transport the delivery rides, and the Region of the endpoint
/// whose address was published — the Region the account's CloudWatch Logs group is written in,
/// which a session arriving on a socket carries no credential scope to read. No caller, no
/// credential of a caller's and no payload is held.
pub struct BrokerSessionLogs<Eng> {
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    region: String,
}

/// Written by hand rather than derived: the engine a sink holds carries no `Debug`. What is
/// diagnosable about a sink is the Region it delivers in.
impl<Eng> ::std::fmt::Debug for BrokerSessionLogs<Eng> {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("BrokerSessionLogs")
            .field("region", &self.region)
            .finish()
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> SessionLogSink
    for BrokerSessionLogs<Eng>
{
    fn log<'sink>(
        &'sink self,
        event: SessionEvent,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'sink>> {
        Box::pin(async move {
            // What the account's options say about this event, read under ONE guard of this
            // service's store and decided with it released: the delivery below is a call into
            // CloudWatch Logs.
            let Some(state) = crate::state::service_state(&*self.engine) else {
                return;
            };
            let planned = {
                let held = state.lock();
                held.v2_logging_options
                    .as_ref()
                    .and_then(|options| {
                        logged(
                            options,
                            &held.v2_logging_levels,
                            &event.event_type,
                            &event.client_id,
                            event.source_ip.as_deref(),
                            event.served,
                        )
                    })
                    .map(|planned| {
                        // The certificate the session is this account's device by, resolved
                        // under the SAME guard as the options so one entry reports one state.
                        let principal =
                            session_principal(&held, &self.region, &event.client_id);
                        (planned, principal)
                    })
            };
            // An account that has set no options, one that disabled its logs and one whose
            // level does not admit this event are each an event that is not logged: nothing is
            // delivered and nothing is created.
            let Some((planned, principal)) = planned else {
                return;
            };
            let record = log_entry(&event, &planned.level, principal.as_deref());
            let stream = log_stream_of(event.at_millis);
            // The service-owned group is one IoT PUBLISHES, so the delivery creates it; a
            // destination the account pointed an event type at is a group of the account's, so
            // the delivery writes into the group CloudWatch Logs already holds and creates
            // none. Either way the write goes in through CloudWatch Logs' own ingest path, so a
            // delivered entry and `FilterLogEvents` cannot disagree about it.
            //
            // The entry is ingested at the instant of the EVENT it reports and not at the
            // instant this delivery is made: an entry belongs to the moment the broker served
            // the packet, so a reader narrowing the group by time (`FilterLogEvents`'
            // `startTime` / `endTime`) reads it in the period it belongs to however long the
            // delivery itself took.
            let events = [(event.at_millis, record)];
            let delivered = if planned.service_owned_group {
                ::service_common::cloudwatch_logs::delivered_stamped_over(
                    &self.callback,
                    &self.region,
                    &planned.log_group,
                    &stream,
                    &events,
                )
                .await
            } else {
                ::service_common::cloudwatch_logs::delivered_stamped_into_held_group_over(
                    &self.callback,
                    &self.region,
                    &planned.log_group,
                    &stream,
                    &events,
                )
                .await
            };
            if !delivered {
                // Delivery is best effort, as every vended-logs write is: the client's packet
                // has already been served, so a write CloudWatch Logs refused is traced rather
                // than reported to a device that cannot act on it.
                ::tracing::warn!(
                    log_group = %planned.log_group,
                    event_type = %event.event_type,
                    "this V2 logging entry could not be delivered to CloudWatch Logs"
                );
            }
        })
    }
}

/// Install this service's V2 logging as what the broker's session events are LOGGED by, for the
/// endpoint of `region`.
///
/// Called by the operation that PUBLISHES the broker's address, for the same reason the rules
/// engine is installed there ([`crate::state::rules_engine::broker_publications`]): from the
/// moment an address exists, a session opened on it is one the account's logging options decide
/// about. Idempotent — the first sink is kept
/// ([`::service_common::iot_mqtt_broker::MqttBrokerState::install_session_log_sink`]), and every
/// sink reads the same options out of the same store — and it changes no session. An engine that
/// vends no broker store installs nothing.
pub fn install_session_logging<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Arc<Eng>,
    ctx: &OperationContext<'_>,
    region: &str,
) {
    let Some(broker) = ::service_common::iot_mqtt_broker::broker_state(&**engine) else {
        return;
    };
    let sink = Arc::new(BrokerSessionLogs {
        engine: Arc::clone(engine),
        callback: ctx.callback().clone(),
        region: region.to_string(),
    });
    broker.lock().install_session_log_sink(sink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::v2_logging::LoggedEventConfiguration;
    use ::service_common::iot_mqtt_broker::session_logs::{
        Outcome, CONNECT_EVENT, PUBLISH_IN_EVENT, PUBLISH_OUT_EVENT,
    };
    use ::std::collections::BTreeMap;

    /// The account's options with `default_log_level` set to `level` and nothing else
    /// configured, which is what `SetV2LoggingOptions(roleArn, defaultLogLevel)` records.
    fn options_at(level: &str) -> V2LoggingOptions {
        V2LoggingOptions {
            role_arn: Some("arn:aws:iam::000000000000:role/iot-logging".to_string()),
            default_log_level: Some(level.to_string()),
            disable_all_logs: false,
            event_configurations: BTreeMap::new(),
        }
    }

    /// RULE: an event is logged only when the level that applies admits an event of its own
    /// severity — a served event is `INFO` and a refused one `ERROR` — so the published level
    /// vocabulary decides, from `DISABLED` (nothing) to `DEBUG` (everything).
    #[test]
    fn a_level_admits_the_events_its_own_verbosity_covers() {
        // Every value `LogLevel` declares, and one past the vocabulary, against both
        // severities a broker event has.
        let rows: &[(&str, bool, bool)] = &[
            ("DISABLED", false, false),
            ("ERROR", false, true),
            ("WARN", false, true),
            ("INFO", true, true),
            ("DEBUG", true, true),
            // Lower case is the same level: the value is compared as a level, not as bytes.
            ("debug", true, true),
            // A value that is no level at all configures no logging.
            ("VERBOSE", false, false),
            ("", false, false),
        ];
        for (level, served_logged, refused_logged) in rows {
            let levels = BTreeMap::new();
            let served = logged(
                &options_at(level),
                &levels,
                PUBLISH_IN_EVENT,
                "sensor-1",
                Some("127.0.0.1"),
                true,
            );
            let refused = logged(
                &options_at(level),
                &levels,
                PUBLISH_IN_EVENT,
                "sensor-1",
                Some("127.0.0.1"),
                false,
            );
            assert_eq!(
                served.is_some(),
                *served_logged,
                "case {level:?}: expected a served event logged {served_logged}, got {served:?}"
            );
            assert_eq!(
                refused.is_some(),
                *refused_logged,
                "case {level:?}: expected a refused event logged {refused_logged}, got \
                 {refused:?}"
            );
            assert_eq!(
                served.map(|entry| entry.level),
                served_logged.then(|| INFO_LEVEL.to_string()),
                "case {level:?}: a served event is logged at its own severity"
            );
            assert_eq!(
                refused.map(|entry| entry.level),
                refused_logged.then(|| ERROR_LEVEL.to_string()),
                "case {level:?}: a refused event is logged at its own severity"
            );
        }
    }

    /// RULE: `disableAllLogs` logs nothing, whatever level or override is configured.
    #[test]
    fn disabling_all_logs_logs_nothing() {
        let mut levels = BTreeMap::new();
        levels.insert(
            LoggedTarget {
                target_type: "CLIENT_ID".to_string(),
                target_name: Some("sensor-1".to_string()),
            },
            "DEBUG".to_string(),
        );
        // The rows span what the flag overrides: the default level, a per-target level and a
        // per-event-type configuration, each at the most verbose level there is.
        let mut with_event = options_at("DEBUG");
        with_event.event_configurations.insert(
            "Publish".to_string(),
            LoggedEventConfiguration {
                log_level: Some("DEBUG".to_string()),
                log_destination: None,
            },
        );
        let rows: &[(&str, V2LoggingOptions)] = &[
            ("the default level", options_at("DEBUG")),
            ("a per-event-type configuration", with_event),
        ];
        for (case, options) in rows {
            let mut disabled = options.clone();
            disabled.disable_all_logs = true;
            for event_type in [CONNECT_EVENT, PUBLISH_IN_EVENT, PUBLISH_OUT_EVENT] {
                for served in [true, false] {
                    let entry = logged(
                        &disabled,
                        &levels,
                        event_type,
                        "sensor-1",
                        Some("127.0.0.1"),
                        served,
                    );
                    assert!(
                        entry.is_none(),
                        "case {case:?} / {event_type} / served {served}: expected nothing \
                         logged, got {entry:?}"
                    );
                }
                // ... and with the flag clear the same configuration does log it, so the rows
                // above are not vacuous.
                let entry = logged(
                    options,
                    &levels,
                    event_type,
                    "sensor-1",
                    Some("127.0.0.1"),
                    true,
                );
                assert!(
                    entry.is_some(),
                    "case {case:?} / {event_type}: expected the same configuration to log it \
                     with disableAllLogs clear"
                );
            }
        }
    }

    /// RULE: the level that applies is the most specific one configured — the event type's own
    /// configuration, else the level of the most specific log target the session matches, else
    /// the account's default level.
    #[test]
    fn the_most_specific_configured_level_decides() {
        let mut options = options_at("ERROR");
        options.event_configurations.insert(
            "Connect".to_string(),
            LoggedEventConfiguration {
                log_level: Some("INFO".to_string()),
                log_destination: None,
            },
        );
        let mut levels = BTreeMap::new();
        levels.insert(
            LoggedTarget {
                target_type: LOG_TARGET_TYPE_DEFAULT.to_string(),
                target_name: None,
            },
            "DISABLED".to_string(),
        );
        levels.insert(
            LoggedTarget {
                target_type: "SOURCE_IP".to_string(),
                target_name: Some("10.0.0.7".to_string()),
            },
            "DEBUG".to_string(),
        );
        levels.insert(
            LoggedTarget {
                target_type: "CLIENT_ID".to_string(),
                target_name: Some("sensor-1".to_string()),
            },
            "INFO".to_string(),
        );

        // (case, event type, client id, source ip, whether a SERVED event is logged)
        let rows: &[(&str, &str, &str, Option<&str>, bool)] = &[
            (
                "the event type's own configuration beats every target",
                CONNECT_EVENT,
                "sensor-9",
                None,
                true,
            ),
            (
                "the client id's level beats the source address and the default target",
                PUBLISH_IN_EVENT,
                "sensor-1",
                Some("10.0.0.7"),
                true,
            ),
            (
                "the source address's level beats the default target",
                PUBLISH_IN_EVENT,
                "sensor-9",
                Some("10.0.0.7"),
                true,
            ),
            (
                "the default target beats the account's default level",
                PUBLISH_IN_EVENT,
                "sensor-9",
                Some("10.0.0.8"),
                false,
            ),
        ];
        for (case, event_type, client_id, source_ip, expected) in rows {
            let entry = logged(
                &options,
                &levels,
                event_type,
                client_id,
                *source_ip,
                true,
            );
            assert_eq!(
                entry.is_some(),
                *expected,
                "case {case:?}: expected a served event logged {expected}, got {entry:?}"
            );
        }

        // With no target level at all the account's own default level is what applies, and it
        // carries the refusals alone.
        let empty = BTreeMap::new();
        assert!(
            logged(
                &options,
                &empty,
                PUBLISH_IN_EVENT,
                "sensor-9",
                None,
                true
            )
            .is_none(),
            "case \"the account's default level\": expected a served event not logged at ERROR"
        );
        assert!(
            logged(
                &options,
                &empty,
                PUBLISH_IN_EVENT,
                "sensor-9",
                None,
                false
            )
            .is_some(),
            "case \"the account's default level\": expected a refused event logged at ERROR"
        );
    }

    /// RULE: `disableAllLogs` is what decides whether an account that states a level delivers
    /// at all: the same options with the switch set log nothing and with it clear log what the
    /// level admits, whatever that level is.
    #[test]
    fn the_switch_alone_decides_whether_a_configured_account_delivers() {
        // The rows span the levels the switch is toggled over: one that admits a session's whole
        // flow, one that admits its refusals alone, and one that admits nothing.
        let rows: &[(&str, bool, bool)] = &[
            ("DEBUG", true, true),
            ("INFO", true, true),
            ("ERROR", false, true),
            ("DISABLED", false, false),
        ];
        for (level, served_logged, refused_logged) in rows {
            let mut suspended_options = options_at(level);
            suspended_options.disable_all_logs = true;
            for served in [true, false] {
                let expected = if served {
                    *served_logged
                } else {
                    *refused_logged
                };
                let cleared = logged(
                    &options_at(level),
                    &BTreeMap::new(),
                    PUBLISH_IN_EVENT,
                    "sensor-1",
                    None,
                    served,
                );
                assert_eq!(
                    cleared.is_some(),
                    expected,
                    "case {level:?} / served {served}: expected the switch cleared to log it \
                     {expected}, got {cleared:?}"
                );
                let suspended = logged(
                    &suspended_options,
                    &BTreeMap::new(),
                    PUBLISH_IN_EVENT,
                    "sensor-1",
                    None,
                    served,
                );
                assert!(
                    suspended.is_none(),
                    "case {level:?} / served {served}: expected the switch set to log nothing, \
                     got {suspended:?}"
                );
            }
        }
    }

    /// RULE: an entry goes into the service-owned group, unless the event type's own
    /// configuration names a destination that overrides it — and that destination is a group of
    /// the account's, which the delivery does not create.
    #[test]
    fn an_event_configuration_overrides_the_group_for_its_event_type() {
        let mut options = options_at("DEBUG");
        options.event_configurations.insert(
            "Publish".to_string(),
            LoggedEventConfiguration {
                log_level: None,
                log_destination: Some("/account/iot/publishes".to_string()),
            },
        );
        // The rows span what the override covers: both directions of the configured family, an
        // event type it does not name, and a direction the account configured EXACTLY, which is
        // the more specific configuration of the two that are about it.
        options.event_configurations.insert(
            PUBLISH_OUT_EVENT.to_string(),
            LoggedEventConfiguration {
                log_level: None,
                log_destination: Some("/account/iot/deliveries".to_string()),
            },
        );
        let rows: &[(&str, &str, bool)] = &[
            (PUBLISH_IN_EVENT, "/account/iot/publishes", false),
            (PUBLISH_OUT_EVENT, "/account/iot/deliveries", false),
            (CONNECT_EVENT, V2_LOG_GROUP, true),
        ];
        for (event_type, group, service_owned) in rows {
            let entry = logged(
                &options,
                &BTreeMap::new(),
                event_type,
                "sensor-1",
                None,
                true,
            )
            .unwrap_or_else(|| panic!("case {event_type}: the event is logged at DEBUG"));
            assert_eq!(
                (entry.log_group.as_str(), entry.service_owned_group),
                (*group, *service_owned),
                "case {event_type}: expected the group {group:?} (service owned \
                 {service_owned}), got {entry:?}"
            );
        }
    }

    /// RULE: an entry reports the event's own facts in the published entry format, and leaves
    /// out every field the event carries nothing for.
    #[test]
    fn an_entry_reports_the_event_it_is_about() {
        let session = ::service_common::iot_mqtt_broker::MqttSession {
            client_id: "sensor-1".to_string(),
            source_ip: Some("127.0.0.1".to_string()),
            source_port: 51_234,
            ..::service_common::iot_mqtt_broker::MqttSession::default()
        };
        // 2026-11-29T16:06:47.123Z, as the entry's own timestamp field spells it.
        let at = 1_795_968_407_123;
        let served = SessionEvent::of_session(PUBLISH_IN_EVENT, &session, Outcome::Served, at)
            .about_topic("devices/sensor-1/telemetry");
        let entry: Value =
            ::serde_json::from_str(&log_entry(&served, INFO_LEVEL, Some("a1b2c3")))
                .expect("the entry is a JSON document");
        for (member, expected) in [
            ("timestamp", json!("2026-11-29 16:06:47.123")),
            ("logLevel", json!(INFO_LEVEL)),
            ("accountId", json!(::engine::emulator_account_id())),
            ("status", json!(SUCCESS_STATUS)),
            ("eventType", json!(PUBLISH_IN_EVENT)),
            ("protocol", json!(MQTT_PROTOCOL)),
            ("clientId", json!("sensor-1")),
            ("principalId", json!("a1b2c3")),
            ("topicName", json!("devices/sensor-1/telemetry")),
            ("sourceIp", json!("127.0.0.1")),
            ("sourcePort", json!(51_234)),
        ] {
            assert_eq!(
                entry.get(member),
                Some(&expected),
                "case \"a served publication\": expected {member} to be {expected}, got {entry}"
            );
        }
        assert!(
            entry
                .get("traceId")
                .and_then(Value::as_str)
                .is_some_and(|id| ::uuid::Uuid::parse_str(id).is_ok()),
            "case \"a served publication\": expected a trace id of its own, got {entry}"
        );
        assert!(
            entry.get("reason").is_none(),
            "case \"a served publication\": expected no reason on an event it served, got {entry}"
        );

        // A refused event reports the failure and the sentence that says why; an event of a
        // connection with no session record reports neither address nor port.
        let refused = SessionEvent::of_client(
            CONNECT_EVENT,
            "sensor-2",
            Outcome::Refused("the connection named no client id"),
            at,
        );
        let entry: Value = ::serde_json::from_str(&log_entry(&refused, ERROR_LEVEL, None))
            .expect("the entry is a JSON document");
        for (member, expected) in [
            ("logLevel", json!(ERROR_LEVEL)),
            ("status", json!(FAILURE_STATUS)),
            ("eventType", json!(CONNECT_EVENT)),
            ("clientId", json!("sensor-2")),
            ("reason", json!("the connection named no client id")),
        ] {
            assert_eq!(
                entry.get(member),
                Some(&expected),
                "case \"a refused connect\": expected {member} to be {expected}, got {entry}"
            );
        }
        for member in ["topicName", "sourceIp", "sourcePort", "principalId"] {
            assert!(
                entry.get(member).is_none(),
                "case \"a refused connect\": expected no {member}, got {entry}"
            );
        }
        assert_eq!(
            log_stream_of(at),
            "2026/11/29",
            "an entry is written in the stream of its own UTC day"
        );
    }
}
