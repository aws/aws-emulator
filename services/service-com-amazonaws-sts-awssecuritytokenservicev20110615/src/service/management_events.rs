// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `sts.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event — read-only calls
//! included — which makes the recording a property of SERVING a call and not of any one
//! operation, so it is not written per handler. The shared wrapper
//! ([`service_common::audited_operation`]) is handed the deserialized input before the
//! handler runs, the handler's own `Result` after, and the rendered response last; this
//! module is only what THIS service contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the members whose values its records LEAVE OUT ([`OMITTED_MEMBERS`]);
//! * the SESSION a call made with temporary credentials was made BY, read off the
//!   account's session directory (see [`SecurityTokenAudit::recorded_session`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the table
//! [`crate::service::EmulatedService::override_operation`] serves every modelled action
//! from, so no request can be served by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: sts`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "sts.amazonaws.com";

/// The Region a record reports for a call whose request carries no credential scope to
/// read one from — the Region a generated emulator defaults to, which is also the Region
/// a lookup made with a default-configured SDK reports its events from.
const DEFAULT_REGION: &str = "us-east-1";

/// The request and response members this service's records LEAVE OUT — no key for them in
/// `requestParameters` / `responseElements` at all, as against a key carrying CloudTrail's
/// redaction literal.
///
/// Every one of them is credential material: the secret half of a session this service
/// minted, the session token that authorises calls under it, and the three caller-supplied
/// bearer artefacts a federated assume is authenticated by. The model marks four of them
/// `@sensitive` (`smithy_query(query="shape", shape="Credentials")` reports
/// `SecretAccessKey` as `sensitive: true`, and the same holds for
/// `AssumeRoleWithWebIdentityRequest$WebIdentityToken`,
/// `AssumeRoleWithSAMLRequest$SAMLAssertion` and
/// `GetDelegatedAccessTokenRequest$TradeInToken`); `SessionToken` is here because a holder
/// of it can sign requests as the session, which is exactly what makes the secret one.
///
/// WHY omitted rather than redacted: the record of an STS call carries no trace of the
/// credential material the call minted or was given, not even a placeholder saying a value
/// was there — which is what lets a client prove the NEGATIVE, that a session's secrets
/// were never persisted into event history. The set is by member NAME, so it holds for
/// every operation that carries a member of that name.
pub const OMITTED_MEMBERS: &[&str] = &[
    "SecretAccessKey",
    "SessionToken",
    "WebIdentityToken",
    "SAMLAssertion",
    "TradeInToken",
];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads, and the sessions it names come from the directory in that same engine
/// — so it is generic over the engine type exactly as the service is.
pub struct SecurityTokenAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> SecurityTokenAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for SecurityTokenAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // A call to THIS service can be made with a session this service minted — a chained
        // assume, or a `GetCallerIdentity` under a role session — so its record names that
        // session exactly as another service's record of a session call does.
        crate::state::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the request was signed for: a caller reaching a regional endpoint of
        // this service is answered — and recorded — in that Region, which is the Region a
        // lookup made by the same client reports its events from.
        ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
            .map(|scope| scope.region)
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. A session this service mints is
    // no resource of the account, and the role a request names is IAM's resource rather
    // than one this service holds, so a record carries the call, its parameters and its
    // outcome and no resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        // Nothing is redacted to the placeholder literal: what this service withholds, it
        // withholds by OMISSION (see [`OMITTED_MEMBERS`]).
        &[]
    }

    fn omitted_members(&self) -> &'static [&'static str] {
        OMITTED_MEMBERS
    }
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type ([`service_common::management_events::ManagementEventLog`]) and not a
/// field of this service's own state: CloudTrail reports the management calls of every
/// service, and the service that served a call is the only one that knows it happened, so
/// recorder and reporter reach one log rather than two copies of it.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsQuery` (not the EC2 dialect), which is the same value the
/// generated per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

/// A call that only REPORTS state, as the table's rows name that kind.
const fn reports() -> CallKind {
    CallKind::Reporting
}

/// A call that CHANGES the account's resources.
const fn mutates() -> CallKind {
    CallKind::Mutating
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// Each operation is served behind one more wrapper, OUTSIDE the audited call: this
/// service states a refused request with the value as SUBMITTED and names the request in
/// its error envelope (see [`super::StatedPerOperation`]). That wrapper changes how a
/// response is RENDERED only, so the record the audited call writes still reports the
/// operation's own input and output.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve_behind(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                $kind(),
                |audited, _pre_dispatch| super::StatedPerOperation::new(audited),
            );
        )*
    }};
}

/// Every modelled operation of this service, each constructed from the shared engine and
/// served behind the audit recorder.
///
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the three that answer
/// a question about a credential or a message and change nothing — `GetCallerIdentity`,
/// `GetAccessKeyInfo` and `DecodeAuthorizationMessage` — and `mutates` for every operation
/// that MINTS a credential the account then holds, which is every assume, every token vend
/// and the delegated-access trade.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(SecurityTokenAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, QUERY_ENVELOPE);
    audited!(
        table, engine,
        assume_role::AssumeRole => mutates,
        assume_role_with_saml::AssumeRoleWithSAML => mutates,
        assume_role_with_web_identity::AssumeRoleWithWebIdentity => mutates,
        assume_root::AssumeRoot => mutates,
        decode_authorization_message::DecodeAuthorizationMessage => reports,
        get_access_key_info::GetAccessKeyInfo => reports,
        get_caller_identity::GetCallerIdentity => reports,
        get_delegated_access_token::GetDelegatedAccessToken => mutates,
        get_federation_token::GetFederationToken => mutates,
        get_scoped_credentials::GetScopedCredentials => mutates,
        get_session_token::GetSessionToken => mutates,
        get_web_identity_token::GetWebIdentityToken => mutates,
    );
    table.finish()
}
