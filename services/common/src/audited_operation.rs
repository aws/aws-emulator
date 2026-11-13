// SPDX-License-Identifier: CC0-1.0

//! Serving an operation AUDITED: one wrapper that records each call a service serves
//! through it as a management event, so CloudTrail's `LookupEvents` reports it.
//!
//! # Why a wrapper and not per-handler code
//!
//! Every control-plane API call of an account is a management event, which makes the
//! recording a property of SERVING a call rather than of any one operation: an operation
//! that records its own call records the same six facts as its neighbour, and an operation
//! nobody remembered to edit records nothing at all. So the recording lives here, once,
//! and a service serves its modelled operations through it
//! ([`AuditedTable`], handed back from `Service::override_operation`).
//!
//! The wrapper sits at the TYPED seam ([`Operation`]), not at the wire: it is handed the
//! deserialized input before the handler runs and the handler's own `Result` after, which
//! is what lets one rule hold for every operation —
//!
//! * `eventName` is the operation's modelled name ([`Operation::NAME`]);
//! * `requestParameters` / `responseElements` are the built input and output rendered
//!   against their own schemas ([`crate::management_events::recorded_shape`]), so a
//!   member whose value the serving API withholds is redacted or left out rather than
//!   stored — and a call STOPPED AT AUTHORIZATION records neither, because the parameters
//!   report what a call that was authorized and dispatched carried
//!   ([`crate::management_events::recorded_request_parameters`]);
//! * `errorCode` / `errorMessage` come from the error channel the caller was answered
//!   through ([`crate::management_events::recorded_failure`]), so a failed call records
//!   the fault SHAPE's name;
//! * `userIdentity` is the credential the request was signed with, and — for a call made
//!   with a session the account's session directory holds — the session's own principal,
//!   the role that issued it and the source identity it carries
//!   ([`AuditedService::recorded_session`]);
//! * `requestID` is the id the CALLER was answered under
//!   ([`crate::query_error_envelope::named_request`]), so a client holding a response can
//!   find the record of that very call rather than guessing which of two records of one
//!   operation is its own. That id exists only once the response has been rendered, which
//!   is why the record built in the handler is HELD until the run that renders it finishes.
//!
//! Nothing else about the request changes: the wrapper runs the framework's own
//! deserialize -> validate -> handle -> serialize sequence for the operation it wraps
//! ([`smithy_server_runtime::run_operation_http`]), forwards the operation's modelled auth
//! schemes, and leaves every status, code and message to the framework.
//!
//! # The second reader of the same record
//!
//! A management event has two readers, so the wrapper has two sinks for one record: the
//! log CloudTrail's `LookupEvents` reads, and — while a trail of the Region is logging
//! calls of this kind — the account's default EventBridge bus, which CloudTrail publishes
//! each logged call onto as `AWS API Call via CloudTrail`
//! ([`crate::api_call_events`]). The record is written once and the event published from
//! that same record, so a rule matching the fan-out sees exactly what a lookup reports.
//!
//! # The calls of a service's DATA plane
//!
//! CloudTrail classes an account's API calls in two, and the two are delivered to different
//! readers: a management event is the control-plane call this wrapper records above, while a
//! call on the CONTENTS of one resource is a DATA event — never event history, never the
//! `AWS API Call via CloudTrail` channel, and delivered to a trail's bucket only while one
//! of that trail's event selectors admits it ([`crate::data_events`]). Which class a call
//! falls in is CloudTrail's own and is stated per operation where the table is built
//! ([`AuditedTable::serves_data_event`]), beside that operation's read/write kind: the
//! record itself is built from the typed call by the same code either way, so the two
//! classes cannot report one call's parameters, identity or outcome differently.
//!
//! # What is NOT recorded
//!
//! A call the scaffold refused as not implemented ([`NOT_IMPLEMENTED_SHAPE_NAME`]) was
//! never served, so no event is recorded for it: event history is the record of the calls
//! an account's APIs actually served, and an emulator gap is not one of them.
//!
//! Neither is a request the FRAMEWORK refused before any handler ran — a body that does
//! not deserialize, a member outside a modelled constraint, a pre-dispatch
//! authentication refusal. Those are answered without the typed call this wrapper hangs
//! off ever happening, so the input and the error channel a record is built from do not
//! exist; a reader should expect event history here to hold the calls that REACHED an
//! operation. Recording them too needs a seam that can name the operation and render its
//! fault from the framework's own pre-dispatch failure, which this wrapper is not.
//!
//! Recording is BEST EFFORT, as every other recorder in this tree: a service whose log is
//! unreachable (a unit test over a bare engine) still serves the call, because failing a
//! caller because the audit log is unreachable would be a worse answer than serving it
//! unrecorded.

use std::cell::RefCell;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationError, OperationRoutingTable, PreDispatchProtocol, ServerHttpProtocol, Service,
    ServiceIdentity,
};

use crate::management_events::{
    recorded_failure, recorded_request_parameters, recorded_shape, RecordedSession, ServedApiCall,
    WithheldFromRecord,
};
use crate::query_error_envelope::named_request;

thread_local! {
    /// The record of the call this thread is serving, built as the last thing the handler's
    /// own run does and taken by [`RecordedCall`] once the response that answers it exists —
    /// which is the only moment the id the caller was answered under is known.
    ///
    /// # The invariant this rests on
    ///
    /// The two sides of it are ONE poll apart, which is what makes a thread-local the right
    /// home: the framework's sequence serializes the handler's result and returns with
    /// nothing awaited after `Operation::handle`
    /// ([`smithy_server_runtime::run_operation_http`]), and the layer that takes the record
    /// is the one that awaited that sequence — so nothing else can run on this thread
    /// between the record being put here and `taken_record` taking it. An operation served
    /// behind one more wrapper ([`AuditedTable::serve_behind`]) is part of that invariant:
    /// such a wrapper must not await anything AFTER the framework sequence it drives, or the
    /// task may resume on another thread and leave the record behind.
    ///
    /// Nothing is left to be quietly wrong if that ever stops holding. The record is TAKEN
    /// rather than read, the run about to serve a call clears the slot first
    /// ([`forget_any_unanswered`]), and a record found where the slot should be empty is
    /// DISCARDED rather than written: a record may never name a request no caller was
    /// answered under, and may never be attributed to another call. Empty means nothing was
    /// recorded for the call — an operation the scaffold refused, or a request the framework
    /// answered before any handler ran.
    static PENDING_RECORD: RefCell<Option<ServedApiCall>> = const { RefCell::new(None) };
}

/// The record this thread is holding, taken out of the slot so it is read once.
fn taken_record() -> Option<ServedApiCall> {
    PENDING_RECORD.with(|slot| slot.borrow_mut().take())
}

/// Drop whatever record this thread may still hold.
///
/// There is one only if the invariant in [`PENDING_RECORD`] stopped holding, which no
/// composition in this tree does. It is DISCARDED rather than written, and that is the whole
/// of the choice: the id the caller of that call was answered under is no longer reachable,
/// and a record naming an id no response carried is the very defect a record's `requestID`
/// exists to prevent — so the alternatives are a record that lies and no record, and no
/// record is the honest one.
fn forget_any_unanswered() {
    drop(taken_record());
}

/// Hold `call` until the response that answers it exists.
///
/// The slot is empty by then — the run serving this call cleared it — so a record found in
/// it is dropped by [`forget_any_unanswered`] rather than overwritten where it sits.
fn hold_record(call: ServedApiCall) {
    forget_any_unanswered();
    PENDING_RECORD.with(|slot| *slot.borrow_mut() = Some(call));
}

/// The wire error code of the scaffold's "not implemented" signal, which every generated
/// service's error channel renders a stub with (`crate::scaffolded` in each service
/// crate). A call it refused reached no implementation, so it is not an event.
pub const NOT_IMPLEMENTED_SHAPE_NAME: &str = "NotImplemented";

/// The HTTP status that signal renders: a 501, deliberately not a retryable 5xx. Both it
/// and the name above must match before a failure is read as the scaffold's, so a modelled
/// error a service's own model happens to name `NotImplemented` — answered with that
/// model's own status by a handler that DID serve the call — is still recorded.
pub const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// What the serving SERVICE contributes to each record: the facts that are the same for
/// every call it serves, plus the per-call ones it alone can resolve.
///
/// Implemented once per service (on a value holding that service's engine handle), so the
/// wrapper names no engine type and no service state.
pub trait AuditedService: Send + Sync + 'static {
    /// Record one served call in the account's management-event log, or drop it when this
    /// engine carries no log.
    ///
    /// `answered_as` is the request id the CALLER was answered under, which the record
    /// carries as its `requestID`.
    ///
    /// Answers what the account's CloudTrail DELIVERS for the call, when a trail of the
    /// Region is logging one of its kind — which [`RecordedCall`] publishes onto the default
    /// event bus and writes into each logging trail's bucket. `None` is a call nothing is
    /// delivered for: no trail logs it, or this engine carries no log at all.
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<crate::api_call_events::DeliveredApiCall>;

    /// The `eventSource` every record of this service carries: its endpoint prefix in
    /// CloudTrail's `<prefix>.amazonaws.com` form.
    fn event_source(&self) -> &'static str;

    /// The Region the call was served in, read off the request the way this service reads
    /// it everywhere else.
    fn region(&self, ctx: &OperationContext<'_>) -> String;

    /// The account the call was served for.
    fn account_id(&self) -> String;

    /// The resources the call acted on, from the parameters it carried — the pairs
    /// CloudTrail reports in an event's `Resources`, which a lookup by `ResourceName` or
    /// `ResourceType` filters on. The default is none: a service that can name the
    /// resource a call addressed overrides this.
    ///
    /// `operation` is the modelled name of the operation that served the call, because
    /// which resource a parameter names can be the OPERATION's fact rather than the
    /// member's: one member name (`Name`) addresses a different kind of resource on each
    /// of the creates that carry it.
    fn resources(
        &self,
        operation: &str,
        request_parameters: &serde_json::Value,
        region: &str,
    ) -> Vec<crate::management_events::AuditedResource> {
        let _ = (operation, request_parameters, region);
        Vec::new()
    }

    /// The members this service's model declares `@sensitive` whose records carry the
    /// redaction literal in place of the value. A value held at one of them is replaced by
    /// that literal and never stored.
    fn sensitive_members(&self) -> &'static [&'static str];

    /// The SESSION the credential a call was signed with belongs to, read off the
    /// account's shared session directory
    /// ([`crate::management_events::SessionAuditDirectory`]) — which is what makes a call
    /// made with temporary credentials recorded as the session it was made by, with the
    /// role that issued the session and the source identity the session carries.
    ///
    /// The default is `None`: the directory is a store in the shared engine, and a service
    /// reaches engine state through its own accessor, so a service that records calls
    /// answers this with that accessor (and one whose engine carries no directory records
    /// no session context, exactly as it records nothing when the log is unreachable).
    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        let _ = access_key_id;
        None
    }

    /// Whether a failure answered with this error identity is the refusal this service's
    /// PERMISSION CHECK answers BEFORE the operation acts on the request — the one failure
    /// whose record carries no `requestParameters` and no `responseElements`, because the
    /// request reached no operation to be dispatched with them
    /// ([`crate::management_events::recorded_request_parameters`]).
    ///
    /// The default is NONE, which is the honest answer for a service that decides
    /// permission while serving a call: a refusal the OPERATION reached — a resource that is
    /// not there, a trust or resource policy that does not admit the request, a quota, a
    /// credential the operation checked — is a call that WAS dispatched, so its record
    /// carries what it was dispatched with. A service states this only for the refusal its
    /// own pre-dispatch gate answers, because WHERE a refusal is decided is a fact about
    /// that service and not about the status: two services answer the same 403 for refusals
    /// decided in different places.
    ///
    /// `operation` is the modelled name of the operation the call was made to, because
    /// which refusals a service decides before dispatch can be the OPERATION's fact rather
    /// than the error code's: one code covers both a gate's decision and a refusal a
    /// particular handler reaches of its own. `shape_name` and `http_status` are the error's
    /// own, exactly as the record reports them
    /// ([`crate::management_events::recorded_failure`]).
    fn stopped_at_authorization(
        &self,
        operation: &str,
        shape_name: &str,
        http_status: u16,
    ) -> bool {
        let _ = (operation, shape_name, http_status);
        false
    }

    /// The members this service's records leave OUT entirely: no key for them at all,
    /// rather than a key carrying the redaction literal.
    ///
    /// Which of the two withholdings a service's records use is the SERVING API's, not
    /// this wrapper's: the default is none, so a service states only the members its own
    /// records omit (see [`WithheldFromRecord`]).
    fn omitted_members(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Whether a call only REPORTED state, as CloudTrail stamps an event's `readOnly` — the
/// one fact about a call that the operation's own semantics decide, so it is stated per
/// operation where the table is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// The call reports state and changes none of it; `readOnly` is true and, as
    /// CloudTrail does for a read, the record carries no `responseElements`.
    Reporting,
    /// The call changes the account's resources; `readOnly` is false and the record
    /// carries what the response answered with.
    Mutating,
}

impl CallKind {
    /// The event's `readOnly`.
    fn read_only(self) -> bool {
        matches!(self, Self::Reporting)
    }
}

/// Which CLASS of CloudTrail event a served call is, which is what decides WHERE its record
/// goes — the one difference between the two rows an audited table can hold.
///
/// It is not a property of the wrapper: an account's API calls are classed by CloudTrail
/// itself, and the two classes are delivered to different readers
/// ([`crate::management_events`], [`crate::data_events`]), so the class is stated per
/// operation where the table is built, beside that operation's read/write kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordedAs {
    /// A MANAGEMENT event: the control-plane call of a service, recorded in event history
    /// where `LookupEvents` reports it, published onto the default event bus, and written
    /// into every logging trail's bucket.
    Management,
    /// A DATA event: a call on the CONTENTS of one resource, written into the bucket of each
    /// trail whose event selectors admit it and nowhere else — never event history, and
    /// never the `AWS API Call via CloudTrail` channel.
    Data,
}

/// One modelled operation served audited.
///
/// Built by [`AuditedTable::serve`]; there is no reason to name this type elsewhere.
pub struct AuditedOperation<O, S> {
    /// The operation that serves the call.
    operation: O,
    /// The service that records it.
    service: Arc<S>,
    /// What this operation's own semantics make of the call.
    kind: CallKind,
    /// Which class of event CloudTrail records this operation's calls as, and so which
    /// reader the record the handler holds is held for.
    recorded_as: RecordedAs,
    /// The protocol envelope the framework renders a pre-dispatch failure of this
    /// operation in — the same value the generated shim passes.
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<O: Operation, S: AuditedService> Operation for AuditedOperation<O, S> {
    type Input = O::Input;
    type Output = O::Output;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static aws_smithy_schema::Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static aws_smithy_schema::Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is rendered BEFORE the handler runs, because the handler consumes it;
        // the document is what the request carried, which is what a record reports.
        let request_parameters = recorded_shape(&input, O::INPUT_SCHEMA, self.withheld());
        let outcome = self.operation.handle(input, ctx).await;
        if let Some(call) = self.served_call(&request_parameters, ctx, &outcome) {
            // Held, not written: the record names the request by the id the caller is
            // answered under, and the response that states it does not exist yet. This is
            // the LAST thing this handler does, with nothing awaited after it, which is
            // what `PENDING_RECORD` relies on — and what the data-event slot relies on in
            // exactly the same way.
            match self.recorded_as {
                RecordedAs::Management => hold_record(call),
                RecordedAs::Data => {
                    crate::data_events::hold_data_event(call, ::std::time::SystemTime::now())
                }
            }
        }
        outcome
    }
}

impl<O: Operation, S: AuditedService> AuditedOperation<O, S> {
    /// What this service's records do not carry the values of, as the one walk over an
    /// input or an output takes it.
    fn withheld(&self) -> WithheldFromRecord<'static> {
        WithheldFromRecord {
            redacted: self.service.sensitive_members(),
            omitted: self.service.omitted_members(),
        }
    }

    /// The record of the call just served, or `None` when it was refused as not
    /// implemented (so nothing was served at all).
    fn served_call(
        &self,
        request_parameters: &serde_json::Value,
        ctx: &OperationContext<'_>,
        outcome: &Result<O::Output, O::Error>,
    ) -> Option<ServedApiCall> {
        if outcome.as_ref().err().is_some_and(|error| {
            error.shape_name() == NOT_IMPLEMENTED_SHAPE_NAME
                && error.http_status() == NOT_IMPLEMENTED_STATUS
        }) {
            return None;
        }
        let region = self.service.region(ctx);
        // "responseElements" is what the call ANSWERED with, which a reporting call has
        // nothing of: CloudTrail records a read with a null `responseElements`. Neither does
        // a DATA event carry one: what such a call answers IS the contents of the resource
        // it acted on, and CloudTrail logs the call on those contents rather than the data
        // itself — the same reason the payload a data call carried is withheld.
        let response_elements = match (self.recorded_as, self.kind, outcome.as_ref().ok()) {
            (RecordedAs::Management, CallKind::Mutating, Some(output)) => {
                Some(recorded_shape(output, O::OUTPUT_SCHEMA, self.withheld()))
            }
            _ => None,
        };
        let (error_code, error_message) = recorded_failure(outcome);
        // WHERE the refusal was decided, which the serving service answers: a call its
        // permission check stopped reached no operation, so there is nothing it was
        // dispatched with to record.
        let stopped_at_authorization = outcome.as_ref().err().is_some_and(|error| {
            self.service
                .stopped_at_authorization(O::NAME, error.shape_name(), error.http_status())
        });
        let caller = crate::management_events::recorded_caller(ctx.parts());
        // The session the credential belongs to, when the call was made with one: a session
        // knows its own principal type, id and name, so those come from it rather than from
        // the access-key prefix the request carried.
        let session = caller
            .access_key_id
            .as_deref()
            .and_then(|access_key_id| self.service.recorded_session(access_key_id));
        Some(ServedApiCall {
            event_name: O::NAME.to_string(),
            event_source: self.service.event_source().to_string(),
            read_only: self.kind.read_only(),
            access_key_id: caller.access_key_id,
            user_name: session
                .as_ref()
                .map_or(caller.user_name, |session| session.user_name.clone()),
            principal_type: session.as_ref().map_or_else(
                || caller.principal_type.to_string(),
                |session| session.principal_type.clone(),
            ),
            // The unique id of the IAM entity a LONG-TERM credential speaks for is IAM's to
            // mint, and a serving service resolves none, so such a record leaves it out
            // rather than naming a principal nothing resolves. A session's own principal id
            // is the session's, and the directory holds it.
            principal_id: session.as_ref().map(|session| session.principal_id.clone()),
            // A call signed with an AWS credential was authenticated by this account, not
            // by an external provider.
            identity_provider: None,
            session,
            account_id: self.service.account_id(),
            user_agent: crate::management_events::recorded_user_agent(ctx.parts()),
            resources: self.service.resources(O::NAME, request_parameters, &region),
            aws_region: region,
            // The parameters the call was DISPATCHED with, and none for a call the
            // permission check stopped before it was dispatched at all (see
            // [`recorded_request_parameters`]).
            request_parameters: recorded_request_parameters(
                request_parameters.clone(),
                stopped_at_authorization,
            ),
            response_elements,
            error_code,
            error_message,
        })
    }
}

impl<O, S> ErasedOperation for AuditedOperation<O, S>
where
    O: Operation + ErasedOperation,
    S: AuditedService,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation: the handler it calls is the wrapped one, so nothing about the
            // wire changes and the record is built inside the typed call. The record the
            // handler holds is written by [`RecordedCall`], which is what the table serves
            // this operation behind.
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, self.pre_dispatch)
                .await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: wrapping must not change which
        // credentials sign a call.
        self.operation.auth_schemes()
    }
}

/// One call served and then RECORDED: whatever the table serves for an operation, with the
/// record its handler held written once the response that answers the call exists.
///
/// It is the OUTERMOST layer of every row of an [`AuditedTable`], and the record is written
/// here rather than inside [`AuditedOperation`] for one reason: an operation may be served
/// behind one more wrapper ([`AuditedTable::serve_behind`]), and such a wrapper drives the
/// framework's sequence itself — the audited operation's own `run` is then never called,
/// while this one always is. So the record is written exactly once per served call, after
/// the response has been rendered by every layer that renders it, which is the only moment
/// the id the caller is answered under is known.
pub struct RecordedCall<W, S> {
    /// What serves the call: the audited operation, or a wrapper around it.
    inner: W,
    /// The service that records it.
    service: Arc<S>,
}

impl<W: ErasedOperation, S: AuditedService> ErasedOperation for RecordedCall<W, S> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Whatever this thread may still hold belongs to no call this run serves (see
            // `PENDING_RECORD`): cleared before the slot is used, so a record cannot be
            // named on another call's response.
            forget_any_unanswered();
            let resp = self.inner.run(svc, req, ctx).await;
            let pending = taken_record();
            // The record is written only now, because its `requestID` is the id the CALLER
            // is answered under, and that id is the rendered response's. A response the
            // renderer named no request in is named one here, which every layer outside
            // this one then reports unchanged, so the record and the caller can never hold
            // two ids for one call.
            let (resp, answered_as) = named_request(resp);
            if let Some(call) = pending {
                // The record, and — while the account's CloudTrail is logging calls of its
                // kind — the event that same record is fanned out onto the default event bus
                // as and the log file each logging trail receives it as. Every delivery is
                // made OUTSIDE the log's lock, after the response the caller gets has been
                // rendered, so it changes nothing about what this call answers.
                if let Some(delivered) = self.service.record(call, &answered_as) {
                    delivered.deliver(ctx.callback()).await;
                }
            }
            resp
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The served operation's modelled schemes: recording must not change which
        // credentials sign a call.
        self.inner.auth_schemes()
    }
}

/// The routing table a service serves its operations audited through: every operation
/// wrapped in [`AuditedOperation`] and served behind [`RecordedCall`], keyed by the routing
/// key the protocol resolves (the operation's own modelled name).
///
/// A service builds one in its `EmulatedService::new` and answers
/// `Service::override_operation` from it; [`serve`](Self::serve) takes one
/// already-constructed operation and files it under [`Operation::NAME`], so a wrapped
/// operation cannot land under a key the protocol never resolves.
pub struct AuditedTable<S> {
    table: OperationRoutingTable,
    service: Arc<S>,
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<S: AuditedService> AuditedTable<S> {
    /// An empty table recording into `service`, whose operations render a pre-dispatch
    /// failure in `pre_dispatch` — the service's own protocol envelope.
    pub fn new(service: Arc<S>, pre_dispatch: PreDispatchProtocol<'static>) -> Self {
        Self {
            table: OperationRoutingTable::new(),
            service,
            pre_dispatch,
        }
    }

    /// Serve `operation` audited, as a call of `kind`.
    pub fn serve<O: Operation + ErasedOperation>(
        &mut self,
        operation: O,
        kind: CallKind,
    ) -> &mut Self {
        let audited = self.audited(operation, kind, RecordedAs::Management);
        self.file(O::NAME, audited);
        self
    }

    /// Serve `operation` audited, as a call of `kind`, with the audited operation itself
    /// served behind ONE more wrapper — `wrap` is handed it, together with the envelope
    /// its pre-dispatch failures render in, and hands back what the table serves.
    ///
    /// The audited call stays INSIDE, which is the point of this variant over
    /// [`serve`](Self::serve) wrapped from the outside: a record reports the operation's
    /// OWN input and output, so a wrapper that changes how a response is rendered on the
    /// wire cannot change the document a lookup reports.
    ///
    /// `wrap`'s own `run` must not AWAIT anything after the framework sequence it drives:
    /// the record of the call is handed to [`RecordedCall`] across that resumption (see
    /// `PENDING_RECORD`), and a yield in between would leave it unwritten and reported.
    /// Rendering the response that sequence answered, which is what such a wrapper is for,
    /// awaits nothing.
    pub fn serve_behind<O, W>(
        &mut self,
        operation: O,
        kind: CallKind,
        wrap: impl FnOnce(AuditedOperation<O, S>, PreDispatchProtocol<'static>) -> W,
    ) -> &mut Self
    where
        O: Operation + ErasedOperation,
        W: ErasedOperation + 'static,
    {
        let audited = self.audited(operation, kind, RecordedAs::Management);
        let wrapped = wrap(audited, self.pre_dispatch);
        self.file(O::NAME, wrapped);
        self
    }

    /// `operation` as a call of `kind` of the event class `recorded_as`, recording into this
    /// table's service.
    fn audited<O: Operation + ErasedOperation>(
        &self,
        operation: O,
        kind: CallKind,
        recorded_as: RecordedAs,
    ) -> AuditedOperation<O, S> {
        AuditedOperation {
            operation,
            service: Arc::clone(&self.service),
            kind,
            recorded_as,
            pre_dispatch: self.pre_dispatch,
        }
    }

    /// File what serves one operation under `name`, behind the layer that WRITES the record
    /// its handler held ([`RecordedCall`]) — so every row of this table records its call,
    /// whether or not the operation is served behind a wrapper of its own.
    fn file<W: ErasedOperation + 'static>(&mut self, name: &'static str, inner: W) {
        self.table.add_operation(
            name,
            Box::new(RecordedCall {
                inner,
                service: Arc::clone(&self.service),
            }),
        );
    }

    /// Serve `operation` audited as a REPORTING call ([`CallKind::Reporting`]).
    pub fn reports<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.serve(operation, CallKind::Reporting)
    }

    /// Serve `operation` audited as a MUTATING call ([`CallKind::Mutating`]).
    pub fn mutates<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.serve(operation, CallKind::Mutating)
    }

    /// Serve `operation` as a DATA event of the resource it acts on, as a call of `kind`:
    /// the record is built from the operation's own input and outcome exactly as every other
    /// row's is, and DELIVERED through `delivery` instead of being recorded in event history
    /// ([`crate::data_events`]).
    ///
    /// The row is filed behind [`crate::data_events::DeliveringDataEvents`] rather than
    /// behind [`RecordedCall`], which is the whole difference: a data event is not event
    /// history and is not published onto the default bus, so the only sink for it is the
    /// bucket of each trail whose event selectors admit it.
    ///
    /// The operation is still SERVED by this table, because CloudTrail's classification of a
    /// call says nothing about who serves it: a service with a data plane serves its data
    /// calls and its control-plane calls through one front door, and this is the row that
    /// keeps the class of each one stated in one place.
    pub fn serves_data_event<O, D>(
        &mut self,
        operation: O,
        kind: CallKind,
        delivery: &Arc<D>,
    ) -> &mut Self
    where
        O: Operation + ErasedOperation,
        D: crate::data_events::DataEventDelivery,
    {
        let audited = self.audited(operation, kind, RecordedAs::Data);
        self.table.add_operation(
            O::NAME,
            Box::new(crate::data_events::DeliveringDataEvents::new(
                audited,
                Arc::clone(delivery),
            )),
        );
        self
    }

    /// The table, for the service to hold and route from.
    #[must_use]
    pub fn finish(self) -> OperationRoutingTable {
        self.table
    }
}

/// A service's model-derived half, routing through its AUDITED operations.
///
/// Identity, the protocol and every framework-level error renderer come from the wrapped
/// definition untouched — nothing model-derived is restated here. The ONE thing this
/// wrapper answers differently is the routing table: it hands back the audited one, whose
/// keys are the same modelled operation names the generated table's are and whose
/// operations are the generated ones wrapped in the recorder. So every layer that resolves
/// an operation for a request — the router itself, and any envelope wrapper a service
/// serves its requests through — reaches the audited operation, and no request can be
/// served by a path that records nothing.
///
/// Generic over the definition rather than over an engine, so this crate names no service's
/// generated type; a service whose definition is itself wrapped (an error-envelope wrapper,
/// say) nests the two, with this one INSIDE, so the audited table is what the outer wrapper
/// routes from.
pub struct AuditedDefinition<D> {
    inner: D,
    audited: OperationRoutingTable,
}

impl<D: ErasedServiceDefinition> AuditedDefinition<D> {
    /// Wrap `inner`, routing through `audited` instead of `inner`'s own table.
    pub fn new(inner: D, audited: OperationRoutingTable) -> Self {
        Self { inner, audited }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for AuditedDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        &self.audited
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(
        &self,
        error: ::aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ::aws_smithy_schema_server::ProtocolError,
        chosen: Option<::smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        self.inner.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> ::smithy_server_runtime::WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}
