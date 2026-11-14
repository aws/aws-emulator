// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves — accepted and REFUSED alike — is a
//! management event of `ec2.amazonaws.com`, recorded in the account's event history where
//! CloudTrail's `LookupEvents` reads it, and published onto the account's default
//! EventBridge bus while a trail of the Region is logging calls of its kind.
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call rather than of any one operation — so it is not written
//! per handler. It is written in the one place every response of this service is finished
//! (`super::FinishedEnvelope`), which is this service's single front door: the Region
//! gate, the authorization gate, the fault injector and every modelled operation answer
//! through it, so no call can be served by a path that records nothing.
//!
//! # Where each field of a record comes from
//!
//! * `eventSource` is [`EVENT_SOURCE`], `awsRegion` the Region the request was signed for
//!   (`crate::state::request_region`) and `recipientAccountId` the account the emulator
//!   serves.
//! * `eventName` is the operation's modelled name, taken from the request's own routing
//!   key and only when the routing table SERVES that key — so it is a modelled
//!   operation's name and never a value a body carried.
//! * `readOnly` is whether that operation REPORTS state ([`reports_state`]).
//! * `requestParameters` is the parameter list the ec2Query request carried, read back as
//!   the paths its keys name and spelled as a CloudTrail document spells a modelled member;
//!   it is the WIRE form of the call's parameters — every value is the text the request
//!   sent, where a document rendered from a built input would carry the member's modelled
//!   type — and a call this service's permission check stopped before the operation acted
//!   on it records none at all
//!   (`service_common::management_events::recorded_request_parameters`).
//! * `responseElements` is what a MUTATING call answered with, rendered from the output's
//!   own schema (`service_common::management_events::recorded_shape`); a reporting call
//!   and a refused call carry none, there being no response to record for the second.
//! * `errorCode` is the fault CLASS of the answer plus the EC2 code the caller saw
//!   ([`audited_error_code`]), `errorMessage` the message it saw, character for
//!   character — the two taken from the error channel the caller was answered through
//!   rather than read back off the rendered XML, so nothing about the record depends on
//!   the envelope's escaping.
//! * `requestID` is the id the CALLER was answered under, read out of the very envelope
//!   the caller gets ([`answered_request_id`]), so a client holding a response finds the
//!   record of that call rather than guessing which record of an operation is its own.
//! * `userIdentity` is the credential the request was signed with
//!   (`service_common::management_events::recorded_caller`) and, for a call made with a
//!   session the account's directory holds, that session's own principal.
//!
//! # What is NOT recorded
//!
//! A call the scaffold refused as not implemented
//! (`service_common::audited_operation::NOT_IMPLEMENTED_SHAPE_NAME`) was never served, so
//! no event is recorded for it: event history is the record of the calls an account's APIs
//! actually served, and an emulator gap is not one of them. Neither is a request the
//! FRAMEWORK refused before any handler ran — a body that does not deserialize, a member
//! outside a modelled constraint — nor a request naming no operation of this service: the
//! typed call such a request is recorded from never happened, which is the same line the
//! shared per-operation recorder draws (`service_common::audited_operation`).
//!
//! Recording is BEST EFFORT, as every other recorder in this tree: a service whose log is
//! unreachable (a unit test over a bare engine) still serves the call, because failing a
//! caller because the audit log is unreachable would be a worse answer than serving it
//! unrecorded.

use std::cell::RefCell;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use smithy_server_runtime::{OperationContext, Service};

use service_common::audited_operation::{NOT_IMPLEMENTED_SHAPE_NAME, NOT_IMPLEMENTED_STATUS};
use service_common::management_events::{
    recorded_caller, recorded_request_parameters, recorded_shape, recorded_user_agent,
    ManagementEventLog, RecordedSession, ServedApiCall, SessionAuditDirectory,
    WithheldFromRecord,
};

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: ec2`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "ec2.amazonaws.com";

/// The fault class a record names a CLIENT-side failure under — a request the service
/// refused on the caller's own terms.
pub const CLIENT_FAULT_CLASS: &str = "Client";

/// The fault class a record names a SERVER-side failure under.
pub const SERVER_FAULT_CLASS: &str = "Server";

/// The separator between the fault class and the service's own error code in an audited
/// `errorCode` (`Client.InvalidInstanceID.NotFound`).
const FAULT_CLASS_SEPARATOR: &str = ".";

/// The lowest HTTP status a SERVER fault is answered on, which is what tells the two fault
/// classes apart: a 4xx is the caller's fault and a 5xx is the service's.
const FIRST_SERVER_FAULT_STATUS: u16 = 500;

/// The name prefixes of the operations that REPORT state: an EC2 operation named with one
/// of these reporting verbs answers what the account holds and changes none of it, which
/// is the event's `readOnly` and what a lookup can filter on.
///
/// A PREFIX rule rather than a row per operation, which is how this service already states
/// the other per-operation fact it reads off a name (`super::authorization`'s
/// account-wide-read test): this model binds some 800 operations, and a fact that holds for
/// every one of them by the verb it is named with is stated once.
pub const REPORTING_VERBS: &[&str] = &["Describe", "Get", "List", "Search"];

/// The operations that report state DESPITE being named with another verb — each one takes
/// the resource it is about and answers a document built from state, minting nothing and
/// changing nothing the account then holds.
///
/// Read off each operation's own signature
/// (`smithy_query(query="operation",operation="ConfirmProductInstance,ExportClientVpnClientConfiguration,ExportClientVpnClientCertificateRevocationList,ExportVerifiedAccessInstanceClientConfiguration,ValidateSecurityGroupQuotasForInterface")`):
/// every one answers values alone. The `Export*` operations that DO change something are
/// deliberately absent — `ExportImage` mints an export task the account then holds and
/// `ExportTransitGatewayRoutes` writes an object into a bucket.
pub const REPORTING_BESIDE_ITS_VERB: &[&str] = &[
    "ConfirmProductInstance",
    "ExportClientVpnClientCertificateRevocationList",
    "ExportClientVpnClientConfiguration",
    "ExportVerifiedAccessInstanceClientConfiguration",
    "ValidateSecurityGroupQuotasForInterface",
];

/// The members this service's records carry CloudTrail's redaction literal in place of the
/// value the call carried, by member NAME — so a value held at one of them reaches no
/// record whichever operation carried it.
///
/// The members this model marks `@sensitive` whose value a record could otherwise hold: a
/// generated key pair's private half and an instance's password data
/// (`smithy_query(query="shape",shape="SensitiveUserData,PasswordData")`), the credentials
/// an Apple silicon task is driven with (`SensitiveMacCredentials`), a VPN tunnel's
/// pre-shared key (`TunnelOption$PreSharedKey`, `VpnTunnelOptionsSpecification$PreSharedKey`),
/// a trust provider's OIDC client secret (`OidcOptions$ClientSecret`) and the user data a
/// launch carries. A member whose name a NON-sensitive member of this model shares is
/// deliberately not here: the set is by name, so redacting it would withhold a value the
/// serving API reports in the clear.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    "ClientSecret",
    "KeyMaterial",
    "MacCredentials",
    "PasswordData",
    "PreSharedKey",
    "UserData",
];

/// The parameters an ec2Query request carries that are the PROTOCOL's rather than the
/// call's, and so are no part of the call's recorded parameters.
const ENVELOPE_PARAMETERS: &[&str] = &["Action", "Version"];

/// The most form parameters one record reports. The same bound, for the same reason, as the
/// authorization gate's walk over the same parameter list
/// (`super::addressed_resources`): the list is sized by the caller and every served call
/// appends a record, so one record's parameter count is bounded here rather than by what
/// was sent.
const MAX_RECORDED_PARAMETERS: usize = 4_096;

/// The element an ec2Query SUCCESS envelope names the request it answers with — the
/// lowercase `requestId` the framework's ec2Query renderer writes as the last child of the
/// response (`vendor/smithy-schema-server/src/query.rs`), where this service's ERROR
/// envelope writes EC2's own `RequestID` (`super::ERROR_REQUEST_ID_ELEMENT`).
const SUCCESS_REQUEST_ID_ELEMENT: &str = "requestId";

/// Whether `operation` only REPORTS state, which is the event's `readOnly`.
///
/// Stated from the operation's own name (see [`REPORTING_VERBS`] and
/// [`REPORTING_BESIDE_ITS_VERB`]), so it holds for every operation this service serves
/// rather than for the ones someone remembered to classify.
#[must_use]
pub fn reports_state(operation: &str) -> bool {
    REPORTING_BESIDE_ITS_VERB.contains(&operation)
        || REPORTING_VERBS
            .iter()
            .any(|verb| operation.starts_with(verb))
}

/// The `errorCode` a record carries for a failure answered with `code` on `http_status`:
/// the fault CLASS of that answer, then the service's own code.
///
/// CloudTrail records an EC2 failure as `Client.<code>` / `Server.<code>` — the class of
/// the fault plus the code the SDK saw — so a client that read `InvalidInstanceID.NotFound`
/// off its own response finds the record of that call under the `Client.`-prefixed name.
/// The class comes from the status the caller was answered on, which is what makes it the
/// class of the fault rather than a second fact to keep in step.
#[must_use]
pub fn audited_error_code(code: &str, http_status: u16) -> String {
    let fault_class = if http_status >= FIRST_SERVER_FAULT_STATUS {
        SERVER_FAULT_CLASS
    } else {
        CLIENT_FAULT_CLASS
    };
    format!("{fault_class}{FAULT_CLASS_SEPARATOR}{code}")
}

/// What this service's dispatch learned about the OUTCOME of the call it is serving: the
/// half of a record that only the typed call knows, held until the response that answers
/// the caller exists.
#[derive(Debug)]
struct ServedOutcome {
    /// The code and message the caller was answered with, and the status they were
    /// answered on; `None` for a call that succeeded.
    failure: Option<ServedFailure>,
    /// What a MUTATING call answered with, as the record's `responseElements`.
    response_elements: Option<serde_json::Value>,
    /// Whether this service's PERMISSION CHECK stopped the call before the operation acted
    /// on the request at all.
    stopped_at_authorization: bool,
}

/// The failure the caller was answered with.
#[derive(Debug)]
struct ServedFailure {
    /// The service's own wire error code (`<Code>`).
    code: String,
    /// The message that went with it, exactly as the caller received it.
    message: Option<String>,
    /// The HTTP status it was answered on, which fixes the audited fault class.
    http_status: u16,
}

thread_local! {
    /// The outcome of the call this thread is serving, put here by the layer that KNOWS it
    /// — the protocol that rendered the handler's result, or the gate that refused the
    /// request — and taken by the front door once the response that answers the caller
    /// exists, which is the only moment the id that response carries is known.
    ///
    /// # The invariant this rests on
    ///
    /// The two sides of it are ONE poll apart: the framework's sequence serializes the
    /// handler's result and returns with nothing awaited after it
    /// (`smithy_server_runtime::run_operation_http`), and the layer that takes the outcome
    /// is the one that awaited that sequence — so nothing else can run on this thread
    /// between the outcome being put here and [`taken_outcome`] taking it. The same holds
    /// for a gate, which refuses a request without awaiting anything at all.
    ///
    /// Nothing is left to be quietly wrong if that ever stops holding. The outcome is
    /// TAKEN rather than read, the run about to serve a call clears the slot first
    /// ([`forget_any_unanswered`]), and an empty slot means nothing is recorded for the
    /// call — never a record assembled from another call's outcome.
    static PENDING_OUTCOME: RefCell<Option<ServedOutcome>> = const { RefCell::new(None) };
}

/// The outcome this thread is holding, taken out of the slot so it is read once.
fn taken_outcome() -> Option<ServedOutcome> {
    PENDING_OUTCOME.with(|slot| slot.borrow_mut().take())
}

/// Drop whatever outcome this thread may still hold.
///
/// There is one only if the invariant in [`PENDING_OUTCOME`] stopped holding. It is
/// DISCARDED rather than used: the response that outcome belongs to has already been sent,
/// so the alternatives are a record attributed to the wrong call and no record, and no
/// record is the honest one.
pub fn forget_any_unanswered() {
    drop(taken_outcome());
}

/// Hold `outcome` until the response that answers the call exists.
fn hold_outcome(outcome: ServedOutcome) {
    PENDING_OUTCOME.with(|slot| *slot.borrow_mut() = Some(outcome));
}

/// Hold the outcome of a call that SUCCEEDED, with what it answered as the record's
/// `responseElements`.
///
/// Called where this service renders a success (`Ec2QueryEnvelope::serialize_output`),
/// which is handed the built output and its schema — so the document a lookup reports is
/// the operation's own output rendered against its own schema, and not a reading of the XML
/// the envelope wrote.
pub fn hold_success(output: &dyn SerializableStruct, output_schema: &Schema) {
    hold_outcome(ServedOutcome {
        failure: None,
        response_elements: Some(recorded_shape(output, output_schema, withheld())),
        stopped_at_authorization: false,
    });
}

/// Hold the outcome of a call the operation's own error channel REFUSED, with the code and
/// message the caller is answered with.
///
/// Called where this service renders a failure
/// (`Ec2QueryEnvelope::serialize_error`), which is handed the error's own
/// serializable form, its schema, the wire code and the status — so the recorded message is
/// the one the caller received, before the envelope escapes it.
pub fn hold_failure(
    error: &dyn SerializableStruct,
    error_schema: &Schema,
    error_code: &str,
    http_status: u16,
) {
    let message = recorded_shape(error, error_schema, withheld())
        .get(MESSAGE_MEMBER)
        .and_then(|value| value.as_str().map(str::to_string));
    hold_outcome(ServedOutcome {
        failure: Some(ServedFailure {
            code: error_code.to_string(),
            message,
            http_status,
        }),
        response_elements: None,
        stopped_at_authorization: false,
    });
}

/// The member every error shape of this service states its reason in, as a recorded
/// document names it (`service_common::management_events::recorded_member_name` of the
/// modelled `message`).
const MESSAGE_MEMBER: &str = "message";

/// Hold the outcome of a call one of this service's GATES refused — the Region gate, the
/// authorization gate, a running fault-injection experiment — which answer the caller
/// without the typed call that [`hold_failure`] hangs off ever happening.
///
/// `stopped_at_authorization` is whether this service's PERMISSION CHECK is what refused
/// the call: such a call reached no operation, so its record carries no parameters at all
/// (`service_common::management_events::recorded_request_parameters`).
pub fn hold_refusal(code: &str, message: &str, http_status: u16, stopped_at_authorization: bool) {
    hold_outcome(ServedOutcome {
        failure: Some(ServedFailure {
            code: code.to_string(),
            message: Some(message.to_string()),
            http_status,
        }),
        response_elements: None,
        stopped_at_authorization,
    });
}

/// What this service's records do not carry the values of, as the one walk over an input or
/// an output takes it.
fn withheld() -> WithheldFromRecord<'static> {
    WithheldFromRecord {
        redacted: SENSITIVE_MEMBERS,
        omitted: &[],
    }
}

/// Record the call `response` answers in the account's event history, and deliver what the
/// account's CloudTrail delivers for it.
///
/// Nothing about the response changes: the record is built from the outcome the dispatch
/// held and from the request itself, and every delivery is made after the response the
/// caller gets has been rendered.
pub async fn record_served_call<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    response: &RtResponse,
) {
    let Some(outcome) = taken_outcome() else {
        // Nothing was served that this is the record of: a request the framework refused
        // before any handler ran, or one naming no operation of this service (see the
        // module docs).
        return;
    };
    // The operation's own name, and only when the routing table SERVES that key.
    let Some(operation) = svc
        .definition()
        .protocol()
        .routing_key(req)
        .ok()
        .filter(|key| svc.definition().routing_table().get(key).is_some())
    else {
        return;
    };
    if outcome.failure.as_ref().is_some_and(|failure| {
        failure.code == NOT_IMPLEMENTED_SHAPE_NAME && failure.http_status == NOT_IMPLEMENTED_STATUS
    }) {
        // The scaffold refused the call as not implemented, so nothing was served at all.
        return;
    }
    let Some(answered_as) = answered_request_id(response) else {
        // A record may never name a request no caller was answered under.
        ::tracing::warn!(
            operation,
            "the response named no request: the call was served unrecorded"
        );
        return;
    };
    let read_only = reports_state(operation);
    let call = served_api_call(engine, req, ctx, operation, read_only, &outcome);
    let Some(log) = management_event_log(engine) else {
        // Not silent: an unrecorded call leaves no trace in the log by definition, so it
        // leaves one here instead.
        ::tracing::warn!(
            operation,
            "the engine carries no management-event log: the call was served unrecorded"
        );
        return;
    };
    // The record, and the event the account's CloudTrail fans out for it while a trail of
    // the Region is logging calls of its kind. The guard is BOUND rather than used as a
    // temporary, so the lock is released before the delivery is awaited.
    let delivered = {
        let mut recording = log.lock();
        recording.record_delivered(call, ::std::time::SystemTime::now(), answered_as)
    };
    if let Some(delivered) = delivered {
        delivered.deliver(ctx.callback()).await;
    }
}

/// The record of one served call: what this service knows about it, beside the identifiers
/// and the instant the log itself stamps.
fn served_api_call<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    operation: &str,
    read_only: bool,
    outcome: &ServedOutcome,
) -> ServedApiCall {
    let region = crate::state::request_region(ctx.parts());
    let caller = recorded_caller(ctx.parts());
    // The session the credential belongs to, when the call was made with one: a session
    // knows its own principal type, id and name, so those come from it rather than from the
    // access-key prefix the request carried.
    let session = caller
        .access_key_id
        .as_deref()
        .and_then(|access_key_id| recorded_session(engine, access_key_id));
    let (error_code, error_message) = match &outcome.failure {
        None => (None, None),
        Some(failure) => (
            Some(audited_error_code(&failure.code, failure.http_status)),
            failure.message.clone(),
        ),
    };
    ServedApiCall {
        event_name: operation.to_string(),
        event_source: EVENT_SOURCE.to_string(),
        read_only,
        access_key_id: caller.access_key_id,
        user_name: session
            .as_ref()
            .map_or(caller.user_name, |session| session.user_name.clone()),
        principal_type: session.as_ref().map_or_else(
            || caller.principal_type.to_string(),
            |session| session.principal_type.clone(),
        ),
        // The unique id of the IAM entity a LONG-TERM credential speaks for is IAM's to
        // mint, and a serving service resolves none, so such a record leaves it out rather
        // than naming a principal nothing resolves. A session's own principal id is the
        // session's, and the directory holds it.
        principal_id: session.as_ref().map(|session| session.principal_id.clone()),
        // A call signed with an AWS credential was authenticated by this account, not by an
        // external provider.
        identity_provider: None,
        session,
        account_id: ::emulator_server::emulator_account_id().to_string(),
        user_agent: recorded_user_agent(ctx.parts()),
        // No resource pair: the entries CloudTrail reports in an event's `Resources` name a
        // resource by its CloudTrail type beside its ARN, and the reading this service has
        // of the ids a request carries answers the IAM resource type instead
        // ([`super::addressed_resources`]) — so the record carries the call, its parameters
        // and its outcome rather than a type nothing publishes.
        resources: Vec::new(),
        aws_region: region,
        // The parameters the call was DISPATCHED with, and none for a call the permission
        // check stopped before it was dispatched at all — which is why the parameters are
        // not even read for one.
        request_parameters: if outcome.stopped_at_authorization {
            recorded_request_parameters(serde_json::Value::Null, true)
        } else {
            recorded_request_parameters(recorded_parameters(req), false)
        },
        // "responseElements" is what the call ANSWERED with, which a reporting call has
        // nothing of: CloudTrail records a read with a null `responseElements`, and so is a
        // call that was refused — there was no response to record.
        response_elements: (!read_only)
            .then(|| outcome.response_elements.clone())
            .flatten(),
        error_code,
        error_message,
    }
}

/// The parameters the request carried, as the record's `requestParameters`.
///
/// A record names a member as the CloudTrail event document names it — the modelled name
/// with a lower-case first letter, which is what the shared
/// [`service_common::management_events::recorded_member_name`] spells and what the shared
/// recorder's own walk over a built input produces. An ec2Query request carries its
/// parameters as an `application/x-www-form-urlencoded` list of flattened keys
/// (`InstanceType`, `InstanceId.1`, `Filter.1.Name`), read through `form_urlencoded` — the
/// crate that decodes that media type, the same reader the authorization gate takes the
/// request's ids through (`super::addressed_resources`) — so each key is read back as the
/// path it names: a name segment is a member of a structure, an INDEX segment an element of
/// a list, and the document is built from the fields the call really carried.
///
/// The protocol's own parameters ([`ENVELOPE_PARAMETERS`]) are no part of the call and are
/// left out, and a value held at a member this service withholds ([`SENSITIVE_MEMBERS`])
/// carries the redaction literal instead — matched per segment, so a withheld member is
/// withheld wherever in a request it occurs.
fn recorded_parameters(req: &RtRequest) -> serde_json::Value {
    let mut document = serde_json::Value::Object(serde_json::Map::new());
    // The one walk this front door takes over a request's parameter list
    // (`super::request_parameters`), under the bound a RECORD takes rather than the one a gate's
    // decision takes.
    let fields = super::request_parameters::request_parameters(req, MAX_RECORDED_PARAMETERS);
    for (key, value) in fields {
        if ENVELOPE_PARAMETERS.contains(&key.as_ref()) {
            continue;
        }
        let segments: Vec<&str> = key
            .split(PARAMETER_PATH_SEPARATOR)
            .filter(|segment| *segment != WRAPPED_LIST_SEGMENT)
            .collect();
        if segments.is_empty() || segments.len() > MAX_PARAMETER_DEPTH {
            continue;
        }
        let withheld = segments.iter().any(|segment| is_withheld(segment));
        let recorded = if withheld {
            ::service_common::management_events::HIDDEN_SENSITIVE_VALUE.to_string()
        } else {
            value.into_owned()
        };
        put_parameter(
            &mut document,
            &segments,
            serde_json::Value::String(recorded),
        );
    }
    document
}

/// Whether a record withholds the value a parameter key's `segment` names
/// ([`SENSITIVE_MEMBERS`]).
///
/// Compared without regard to ASCII case: the set is the MODEL's spelling of each member,
/// an ec2Query key is that name with its first letter capitalised, and a client's spelling
/// of a KEY must not be what decides whether a secret reaches a record.
fn is_withheld(segment: &str) -> bool {
    SENSITIVE_MEMBERS
        .iter()
        .any(|member| member.eq_ignore_ascii_case(segment))
}

/// The separator between the segments of an ec2Query parameter key.
const PARAMETER_PATH_SEPARATOR: char = '.';

/// The segment a WRAPPED list writes between the member and the index of an element. It
/// positions a value within the member the segment before it named rather than naming a
/// member of its own, so a recorded path does not carry it.
const WRAPPED_LIST_SEGMENT: &str = "member";

/// How deep into a request's parameters one record goes — the same bound, for the same
/// reason, as the shared recorder's walk over a built input.
const MAX_PARAMETER_DEPTH: usize = 16;

/// Put `value` at the path `segments` names in `document`: a name segment is a member of a
/// structure, a segment of digits the (one-based) index of a list element.
///
/// A list grows only by the element the NEXT position names, so the document holds one
/// value per field the request sent and nothing a field did not fill: a request numbering
/// an element past the end of the list it is in states a position the fields before it
/// never filled, and that field is left out rather than reserving the positions between.
/// That is what keeps the document's size the size of the parameter list the request really
/// carried — already bounded by [`MAX_RECORDED_PARAMETERS`] — rather than a number a
/// caller wrote in a key.
///
/// A container already holding another kind of value is REPLACED rather than merged: a key
/// that names a path through a value another key stated as a leaf is the caller's own
/// inconsistency, and a record reports the last reading of it rather than both.
fn put_parameter(document: &mut serde_json::Value, segments: &[&str], value: serde_json::Value) {
    let Some((segment, rest)) = segments.split_first() else {
        *document = value;
        return;
    };
    match list_index(segment) {
        Some(index) => {
            if !document.is_array() {
                *document = serde_json::Value::Array(Vec::new());
            }
            let Some(elements) = document.as_array_mut() else {
                return;
            };
            if index > elements.len() {
                return;
            }
            if index == elements.len() {
                elements.push(serde_json::Value::Null);
            }
            // By position rather than by index expression: a record is never worth a panic
            // on a path a request reaches.
            if let Some(element) = elements.get_mut(index) {
                put_parameter(element, rest, value);
            }
        }
        None => {
            if !document.is_object() {
                *document = serde_json::Value::Object(serde_json::Map::new());
            }
            let Some(members) = document.as_object_mut() else {
                return;
            };
            let member = members
                .entry(::service_common::management_events::recorded_member_name(segment))
                .or_insert(serde_json::Value::Null);
            put_parameter(member, rest, value);
        }
    }
}

/// The zero-based position of the list element `segment` names, or `None` when it names a
/// member instead.
///
/// ec2Query numbers the elements of a list FROM ONE (`InstanceId.1`), so the index is one
/// less than the segment.
fn list_index(segment: &str) -> Option<usize> {
    let position = segment.parse::<usize>().ok()?;
    position.checked_sub(1)
}

/// The request id the CALLER is answered under, read out of the response envelope itself.
///
/// The one id a response carries, whichever envelope it is: this service's error envelope
/// names it in EC2's own `<RequestID>` (`super::ERROR_REQUEST_ID_ELEMENT`, written by
/// `super::stamp_envelope`) and the success envelope in the framework's lowercase
/// `<requestId>` ([`SUCCESS_REQUEST_ID_ELEMENT`]). Reading it from the rendered body is
/// what makes the record and the caller hold the SAME id rather than two ids for one call.
///
/// `None` for a response that names no request at all, for one whose named id does not have
/// the FORM of a request id (`service_common::query_error_envelope::is_request_id`, the
/// shared reading of that form) and for a streamed body, none of which is a record this can
/// name.
fn answered_request_id(response: &RtResponse) -> Option<String> {
    let body = response.body().bytes()?;
    let text = std::str::from_utf8(body).ok()?;
    last_element_text(text, super::ERROR_REQUEST_ID_ELEMENT)
        .or_else(|| last_element_text(text, SUCCESS_REQUEST_ID_ELEMENT))
        .filter(|stated| ::service_common::query_error_envelope::is_request_id(stated))
        .map(str::to_string)
}

/// The text of the LAST `<name>` element of `text`, or `None` when it holds none.
///
/// The LAST, because the id belongs to the envelope and is written as its final child: an
/// element of the same name nested in a reported resource comes before it. The reading is
/// textual, exactly as the WRITER of that element is (`super::with_error_request_id`) and as
/// the shared `awsQuery` envelope's own reading of its `<RequestId>` is
/// (`service_common::query_error_envelope`) — one element of this service's own rendering,
/// whose value is hexadecimal and dashes, rather than a parse of the document.
fn last_element_text<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let close = format!("</{name}>");
    let end = text.rfind(&close)?;
    let open = format!("<{name}>");
    let start = text[..end].rfind(&open)? + open.len();
    Some(&text[start..end])
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type (`service_common::management_events::ManagementEventLog`) and not a field
/// of this service's own state: CloudTrail reports the management calls of every service,
/// and the service that served a call is the only one that knows it happened, so recorder
/// and reporter reach one log rather than two copies of it.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The SESSION a request's credential belongs to, read off the account's shared session
/// directory — which is what lets a record of a call made with temporary credentials name
/// the session it was made by, the role that issued it and the source identity it carries.
///
/// A SHARED type for the same reason the log is: the service that MINTS a session and the
/// services that record the calls made WITH it are not the same service.
fn recorded_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
) -> Option<RecordedSession> {
    session_audit_directory(engine)
        .and_then(|directory| directory.lock().session(access_key_id).cloned())
}

/// The account's session directory in that same shared engine.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<SessionAuditDirectory>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an audited `errorCode` is the fault CLASS of the answer plus the service's own
    /// code — `Client.` for a refusal the caller caused, `Server.` for a failure the service
    /// did.
    #[test]
    fn an_audited_error_code_carries_the_fault_class_of_its_status() {
        // (the code the caller saw, the status it was answered on, the audited code)
        let rows: &[(&str, u16, &str)] = &[
            ("InvalidInstanceID.NotFound", 400, "Client.InvalidInstanceID.NotFound"),
            // 412 is the status EC2 answers an authorized dry run on: still the caller's
            // class, the class being read off the status rather than listed per code.
            ("DryRunOperation", 412, "Client.DryRunOperation"),
            ("UnauthorizedOperation", 403, "Client.UnauthorizedOperation"),
            ("InternalError", 500, "Server.InternalError"),
            ("RequestLimitExceeded", 503, "Server.RequestLimitExceeded"),
        ];
        for (code, status, expected) in rows {
            let actual = audited_error_code(code, *status);
            assert_eq!(
                &actual, expected,
                "case {code:?} on {status}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: `readOnly` is whether the operation reports state, decided by the reporting
    /// verb its name begins with and by the named operations that report despite another
    /// verb.
    #[test]
    fn the_reporting_operations_are_the_ones_named_with_a_reporting_verb() {
        // (the operation, whether it only reports state)
        let rows: &[(&str, bool)] = &[
            ("DescribeInstances", true),
            ("GetConsoleOutput", true),
            ("ListImagesInRecycleBin", true),
            ("SearchTransitGatewayRoutes", true),
            ("ConfirmProductInstance", true),
            ("ExportClientVpnClientConfiguration", true),
            ("RunInstances", false),
            ("TerminateInstances", false),
            ("CreateVpc", false),
            ("MonitorInstances", false),
            // An export that mints a task the account then holds is a write, where the
            // export of a client configuration above is a read.
            ("ExportImage", false),
            ("ExportTransitGatewayRoutes", false),
        ];
        for (operation, expected) in rows {
            let actual = reports_state(operation);
            assert_eq!(
                actual, *expected,
                "case {operation:?}: expected readOnly {expected}, got {actual}"
            );
        }
    }

    /// One row of the parameter-path table: the case, the fields the request carried, and the
    /// document they are recorded as.
    type ParameterCase<'a> = (&'a str, &'a [(&'a str, &'a str)], serde_json::Value);

    /// RULE: a parameter key is read back as the path it names, each member spelled as a
    /// CloudTrail event document spells one, and a position no field filled is left out.
    #[test]
    fn a_parameter_key_is_read_back_as_the_path_it_names() {
        // (the case, the fields the request carried, the document they are recorded as)
        let rows: &[ParameterCase<'_>] = &[
            (
                "a scalar member",
                &[("InstanceType", "t3.micro")],
                serde_json::json!({"instanceType": "t3.micro"}),
            ),
            (
                "an indexed list",
                &[("InstanceId.1", "i-0123abcd"), ("InstanceId.2", "i-4567ef01")],
                serde_json::json!({"instanceId": ["i-0123abcd", "i-4567ef01"]}),
            ),
            (
                "a wrapped list, whose wrapper segment names no member",
                &[("InstanceId.member.1", "i-0123abcd")],
                serde_json::json!({"instanceId": ["i-0123abcd"]}),
            ),
            (
                "a list of structures, each with a list of its own",
                &[
                    ("Filter.1.Name", "instance-state-name"),
                    ("Filter.1.Value.1", "running"),
                    ("Filter.1.Value.2", "pending"),
                ],
                serde_json::json!({
                    "filter": [{"name": "instance-state-name", "value": ["running", "pending"]}],
                }),
            ),
            (
                "an element numbered past the end of its list, which no field filled",
                &[("InstanceId.1", "i-0123abcd"), ("InstanceId.9999", "i-4567ef01")],
                serde_json::json!({"instanceId": ["i-0123abcd"]}),
            ),
            (
                "a member whose value this service withholds",
                &[("UserData", "a script nobody should read back")],
                serde_json::json!({
                    "userData": ::service_common::management_events::HIDDEN_SENSITIVE_VALUE,
                }),
            ),
        ];
        for (case, fields, expected) in rows {
            let mut document = serde_json::Value::Object(serde_json::Map::new());
            for (key, value) in *fields {
                let segments: Vec<&str> = key
                    .split(PARAMETER_PATH_SEPARATOR)
                    .filter(|segment| *segment != WRAPPED_LIST_SEGMENT)
                    .collect();
                let withheld = segments.iter().any(|segment| is_withheld(segment));
                let recorded = if withheld {
                    ::service_common::management_events::HIDDEN_SENSITIVE_VALUE
                } else {
                    value
                };
                put_parameter(
                    &mut document,
                    &segments,
                    serde_json::Value::String(recorded.to_string()),
                );
            }
            assert_eq!(
                &document, expected,
                "case {case:?}: expected {expected}, got {document}"
            );
        }
    }

    /// RULE: the id a record names the request by is the one the response carries — the
    /// envelope's own last `RequestID` / `requestId`, never one nested in a reported
    /// resource.
    #[test]
    fn the_envelopes_own_request_id_is_the_one_read_back() {
        let error = "<Response><Errors><Error><Code>InvalidInstanceID.NotFound</Code>\
                     <Message>The instance ID &apos;i-0123abcd&apos; does not exist</Message>\
                     </Error></Errors><RequestID>00000000-0000-0000-0000-000000000001\
                     </RequestID></Response>";
        assert_eq!(
            last_element_text(error, "RequestID"),
            Some("00000000-0000-0000-0000-000000000001"),
            "case \"an error envelope\": got {error}"
        );
        let success = "<DescribeInstancesResponse><reservationSet><item>\
                       <requestId>not-the-envelopes</requestId></item></reservationSet>\
                       <requestId>00000000-0000-0000-0000-000000000002</requestId>\
                       </DescribeInstancesResponse>";
        assert_eq!(
            last_element_text(success, "requestId"),
            Some("00000000-0000-0000-0000-000000000002"),
            "case \"a success envelope whose body nests the element\": got {success}"
        );
        assert_eq!(
            last_element_text("<DescribeInstancesResponse/>", "requestId"),
            None,
            "case \"an envelope naming no request\""
        );
    }
}
