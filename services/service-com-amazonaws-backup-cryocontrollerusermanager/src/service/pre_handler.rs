// SPDX-License-Identifier: CC0-1.0

//! The refusals AWS Backup decides AHEAD of the framework's deserialize-and-validate
//! boundary, for the operations whose recorded refusal of a bad request is not the one that
//! boundary can give.
//!
//! # Why a gate rather than a reworded refusal
//!
//! The framework's centralized walk is deserialize -> validate -> `handle` -> serialize: an
//! absent required member and a value outside a modelled constraint are both refused BEFORE
//! a handler runs, so no handler can answer them.
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is the one
//! seam over that refusal and it can only choose the error CODE and the message text: the
//! status stays with the protocol (400), the body member is the protocol's own lowercase
//! `message`, and the seam is handed the violations alone — with no operation and no caller
//! to decide anything else from. Three of this service's refusals are outside what that
//! leaves:
//!
//! * a `StartBackupJob` that names no role to perform the backup under is an AUTHORIZATION
//!   refusal, answered on 403 with the service's own `Message` member;
//! * a malformed `ReportPlanName` is answered with this service's own parameter-value shape,
//!   carrying the sentence that states the grammar in its `Message` member;
//! * a `StartScanJob` refused at the boundary is answered by the REQUEST VALIDATOR in front
//!   of that operation, in its envelope and under its error type, rather than by one of this
//!   service's own parameter errors.
//!
//! So each of those operations is served behind a [`PreHandlerGate`], wired through
//! [`Service::override_operation`](smithy_server_runtime::Service::override_operation) ahead
//! of the generated routing table. The gate reads the request exactly as the walk does — the
//! protocol's own `deserialize_input`, the operation's own input shape, then the schema-driven
//! constraint walk — and asks that operation's own decision what to answer. A request the
//! decision passes on is served by the one generated operation in the routing table, which
//! runs the whole walk over the same bytes and owns every modelled error, so the gate can
//! only REFUSE and never serve.

use std::marker::PhantomData;
use std::time::Instant;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::Violation;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, ServerHttpProtocol, Service,
};

/// The wire error code (`__type` / `x-amzn-errortype`) an authorization refusal of this
/// service carries: the `AccessDeniedException` shape AWS Backup answers a call it does not
/// grant with, whose model lists no authorization error of its own, and the same name a
/// handler-returned one carries (`crate::scaffolded::Scaffolded::AccessDenied`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The status that refusal is served on, as that same channel serves it.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// The wire error code a refusal by AWS's request validator carries, as the framework's own
/// boundary names it on this protocol (`crate::scaffolded::Scaffolded::ValidationException`).
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status every refusal of a bad request this service answers is served on: the
/// `@httpError` every one of its client-input error shapes declares.
pub const REFUSAL_STATUS: u16 = 400;

/// The answer an operation gives a request refused at the boundary: the error shape to
/// serialize, with the wire error code and the status it is served on.
///
/// Serialized through this service's own protocol, exactly as a handler-returned error is,
/// so the body member names, the `__type` and the `x-amzn-errortype` header a client reads
/// are the ones every other refusal of this service carries.
pub struct Answer {
    /// The error payload, as its own shape serializes.
    pub body: Box<dyn SerializableStruct>,
    /// That shape's schema, which drives the body codec.
    pub schema: &'static Schema,
    /// The wire error code (`__type` / `x-amzn-errortype`).
    pub code: &'static str,
    /// The HTTP status the refusal is served on.
    pub status: u16,
}

/// How one operation answers the violations that refuse a request before its handler runs:
/// `None` to let the request through to the generated operation, which then refuses it the
/// way the framework does.
///
/// The violations are the shared collecting walk's — every `@required` member the request
/// omitted and every value outside a declared constraint, with each value echo restored to
/// the bytes the caller sent — so a decision reads what it needs about the request off them
/// and nothing re-reads the body.
pub type Decision = fn(&[Violation]) -> Option<Answer>;

/// One operation served behind its own pre-boundary decision (see the module docs).
///
/// Holds no operation and no store: only the decision to ask. Everything else about the
/// gated operation — its routing key, its input shape and that shape's schema — is read off
/// the operation type itself, so none of it can drift from the model.
pub struct PreHandlerGate<O> {
    /// The operation's own answer to the violations that refuse a request.
    decide: Decision,
    /// The operation this gate stands in front of. `fn() -> O` rather than `O`, so the gate
    /// is `Send + Sync` whatever the operation holds.
    operation: PhantomData<fn() -> O>,
}

impl<O: Operation> PreHandlerGate<O> {
    /// Gate `O`, answering as `decide` says.
    pub const fn new(decide: Decision) -> Self {
        Self {
            decide,
            operation: PhantomData,
        }
    }

    /// What this operation answers `req`, or `None` for a request nothing refuses before the
    /// handler and one whose refusal the framework's own wording covers.
    ///
    /// The violations are collected by the tree's one collecting half of a pre-handler
    /// refusal ([`::service_common::validation::refusing_violations`]), which drives the
    /// framework's own machinery — the protocol's deserializer and `constraints::validate`
    /// over the built input — so this gate cannot find a different set from the walk that
    /// follows it. A read stops at the FIRST required member it finds absent, so where more
    /// than one may be missing the shared re-reading walk is asked for the rest
    /// ([`::service_common::unstated_members`]), which drives the operation's own generated
    /// deserializer and decides nothing for itself about which members are required; it is
    /// asked only where it could add anything, and is bounded in passes and in positions.
    async fn answer_for(&self, protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<Answer> {
        let violations = ::service_common::validation::refusing_violations::<O>(protocol, req);
        if violations.is_empty() {
            return None;
        }
        let violations = if ::service_common::unstated_members::worth_walking(&violations) {
            let unstated = ::service_common::unstated_members::unstated_members::<O>(protocol, req)
                .await;
            ::service_common::unstated_members::stating_every_absence(
                violations,
                &unstated.omitted,
            )
        } else {
            violations
        };
        (self.decide)(&violations)
    }

    /// Serialize `answer` in this service's own protocol envelope, and record the refused
    /// request as the operation row the framework walk would have recorded for it.
    fn refuse(
        &self,
        protocol: &dyn ServerHttpProtocol,
        ctx: &OperationContext<'_>,
        answer: Answer,
        started: Instant,
    ) -> RtResponse {
        let response = match protocol.serialize_error(
            &*answer.body,
            answer.schema,
            answer.code,
            answer.status,
        ) {
            Ok(response) => response,
            // A refusal never fails OPEN: an error shape that will not serialize is still a
            // refusal, answered in this protocol's own envelope rather than by letting the
            // request through to the operation.
            Err(error) => {
                tracing::warn!(
                    operation = O::NAME,
                    %error,
                    "the refusal's error shape would not serialize; it falls back to a \
                     hand-built response"
                );
                unserialized_refusal(answer.code, answer.status)
            }
        };
        record_refusal(
            ctx,
            O::NAME,
            answer.code,
            answer.status,
            started.elapsed(),
        );
        response
    }
}

impl<O: Operation> ErasedOperation for PreHandlerGate<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = Instant::now();
            let protocol = svc.definition().protocol();
            if let Some(answer) = self.answer_for(protocol, req).await {
                return self.refuse(protocol, ctx, answer, started);
            }
            // Nothing to refuse here: the request is served by the generated operation,
            // which owns deserialize -> validate -> handle -> serialize and every modelled
            // error, over the same bytes.
            match svc.definition().routing_table().get(O::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this gate is only reachable under the key the
                // generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The gate is not an operation of its own: the request is signed (or not) as the
        // MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this gate stands in front of.
        None
    }
}

/// The clause AWS's request validator states one violation with, inside its envelope
/// ([`validation_envelope`]).
///
/// The validator states the member and the requirement, and NOT the value it refused: an
/// absent member has none to state, and a value outside an enumerated type's set is not one
/// it reports back. The recorded listing of this front door's refusals shows both — `Value
/// at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: …` and
/// `Value at 'malwareScanner' failed to satisfy constraint: Member must not be null`, where
/// the framework writes `Value 'PARTIAL_SCAN' at …` and `Value null at …`.
///
/// Nothing is spelled here: the clause stays the framework's and the value half is dropped
/// off it by the tree's one reading of that drop
/// ([`::service_common::validation_message::valueless_clause`]), and an enumerated type's
/// set is restated by the tree's one reading of the order a hash-table front door lists one
/// in ([`::service_common::validation_message::in_hashed_enum_order`]) — so the member path,
/// the requirement's wording and the set's order cannot drift from any other service's.
/// A class the drop does not apply to keeps the framework's clause whole.
pub fn validation_clause(violation: &Violation) -> String {
    let stated = ::service_common::validation_message::in_hashed_enum_order(violation);
    ::service_common::validation_message::valueless_clause(&stated)
        .unwrap_or_else(|| stated.message())
}

/// AWS's validation envelope around the clauses of `violations`, in the order a front door
/// that holds its findings by MEMBER states them
/// ([`::service_common::validation_message::in_hashed_member_path_order`]).
///
/// Nothing here is spelled: the envelope is the framework's, rendered by the tree's one
/// reading of it over clauses a caller stated
/// ([`::service_common::validation_message::envelope_over`]), and the order is the tree's one
/// reading of that findings table. Empty only for no violation at all, which no caller here
/// has.
pub fn validation_envelope(violations: &[Violation]) -> String {
    let clauses: Vec<String> =
        ::service_common::validation_message::in_hashed_member_path_order(violations)
            .into_iter()
            .map(validation_clause)
            .collect();
    ::service_common::validation_message::envelope_over(&clauses).unwrap_or_default()
}

/// The response a refusal falls back to when its error shape will not serialize: the same
/// status and the same wire error code, in a JSON envelope naming no message it could not
/// render.
fn unserialized_refusal(code: &'static str, status: u16) -> RtResponse {
    let body = ::serde_json::json!({ "__type": code }).to_string();
    let status = StatusCode::try_from(status).expect("a modelled status is a valid HTTP status");
    let mut response = RtResponse::new(status, SdkBody::from(body));
    let headers = response.headers_mut();
    headers.insert("content-type", "application/json");
    headers.insert("x-amzn-errortype", code);
    response
}

/// Record a refusal answered here as the operation row the framework walk would have
/// recorded, so a gated refusal is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as the
/// framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &'static str,
    error_code: &'static str,
    status: u16,
    latency: std::time::Duration,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle = emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        operation,
    );
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            status, error_code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

#[cfg(test)]
mod tests {
    use aws_smithy_schema_server::constraints::ViolationKind;

    use super::*;

    /// RULE: the clauses of one refusal are stated in the order this front door's own table
    /// of findings is walked, which is a function of the member paths alone — so it does not
    /// depend on the order the model happens to declare those members in.
    #[test]
    fn the_clauses_are_stated_in_one_order_whatever_order_they_arrive_in() {
        // The six members this service's scan-job start requires, as the model declares them
        // and permuted, so the only difference between the two cases is the arrival order.
        let declared = [
            "backupVaultName",
            "iamRoleArn",
            "malwareScanner",
            "recoveryPointArn",
            "scanMode",
            "scannerRoleArn",
        ];
        let mut permuted: Vec<&str> = declared.to_vec();
        permuted.reverse();
        let mut rendered: Vec<String> = Vec::new();
        for (case, paths) in [
            ("the members in the model's own order", declared.to_vec()),
            ("the same six reversed", permuted),
        ] {
            let violations: Vec<Violation> = paths
                .iter()
                .map(|path| Violation {
                    path: (*path).to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                })
                .collect();
            let envelope = validation_envelope(&violations);
            assert!(
                envelope.starts_with("6 validation errors detected: "),
                "case {case:?}: expected the validator's envelope over six clauses, got \
                 {envelope:?}"
            );
            for path in &paths {
                assert!(
                    envelope.contains(&format!("Value at '{path}' failed")),
                    "case {case:?}: expected a clause for {path:?}, got {envelope:?}"
                );
            }
            rendered.push(envelope);
        }
        assert_eq!(
            rendered[0], rendered[1],
            "case \"a permuted set of findings\": the order is the table's, not the arrival \
             order"
        );
    }

    /// RULE: a clause names the member and the requirement and NOT the value it refused —
    /// for an absent member, which has none, and for a value outside an enumerated type's
    /// set, which the validator does not report back.
    #[test]
    fn a_clause_states_the_requirement_and_not_the_submitted_value() {
        let enum_violation = Violation {
            path: "scanMode".to_string(),
            kind: ViolationKind::EnumValue(vec![
                "FULL_SCAN".to_string(),
                "INCREMENTAL_SCAN".to_string(),
            ]),
            value: Some("PARTIAL_SCAN".to_string()),
            sensitive: false,
        };
        let clause = validation_clause(&enum_violation);
        assert!(
            !clause.contains("PARTIAL_SCAN"),
            "case \"an out-of-set enum value\": the submitted value is not stated, got \
             {clause:?}"
        );
        assert!(
            clause.contains(
                "Value at 'scanMode' failed to satisfy constraint: Member must \
                 satisfy enum value set: ["
            ),
            "case \"an out-of-set enum value\": expected the validator's clause, got {clause:?}"
        );
        let envelope = validation_envelope(&[enum_violation]);
        assert!(
            envelope.starts_with("1 validation error detected: "),
            "case \"one violation\": expected the validator's envelope, got {envelope:?}"
        );

        let absent = Violation {
            path: "scanMode".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        assert_eq!(
            validation_clause(&absent),
            "Value at 'scanMode' failed to satisfy constraint: Member must not be null",
            "case \"an absent member\": the validator states the member and not a value"
        );
    }
}
