// SPDX-License-Identifier: CC0-1.0

//! The two halves of a constraint refusal of this service that the framework's walk cannot
//! state, because neither can be recovered from the violations alone: the COLLECTION a
//! clause about a collection member is about, and the clause a constraint broken by a
//! collection's ELEMENTS is stated at.
//!
//! # The rule
//!
//! A value outside a constraint the model declares is refused before any handler runs, over
//! one clause per violation, each naming the value that was rejected, the member path it sat
//! at and the requirement it failed. The value is the only place the request's own input is
//! reported back, so a client comparing it against what it sent has to see its own input.
//!
//! * A `@length` on a list or a map is checked ON THE MEMBER, before the walk descends into
//!   the elements, so the only rendering the framework has for the rejected value is a
//!   SUMMARY of its size — `<n> items` for a list, `<n> entries` for a map
//!   (`vendor/smithy-schema-server/src/constraints.rs`) — which is not a value any caller
//!   sent. AWS states the collection itself there: a list as `[<element>, …]`.
//! * A constraint the ELEMENTS broke is reported by the framework once per offending
//!   element, at `<collection>.<position>.member`, naming only the constraint that element
//!   broke; AWS states ONE clause at `<collection>`, quoting the whole list and listing every
//!   requirement the element shape declares. So one list breaking an element constraint is
//!   ONE validation error about one member, however many of its elements broke it.
//!
//! # Why a wrapper
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is handed
//! the violations alone and never the request they were found in, so neither value can be
//! recovered there. [`QuotingSubmittedValues`] is where they are: a per-operation wrapper,
//! which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the violations and the
//! submitted collections come from the tree's shared collecting walk
//! (`service_common::validation::refusing_violations_with_mapped_echoes`, which runs the
//! protocol's own deserializer and the framework's own `constraints::validate` and owns every
//! bound on what one clause may quote), the restoring rule, the grouping, the element sets
//! and the envelope are `service_common::validation_message`'s, each clause is the
//! framework's own `Violation::message`, and the status and the wire envelope come from the
//! framework's refusal channel. A refusal with no such clause is left to that channel
//! untouched, which answers it with the same code through
//! [`super::EmulatedService::override_refusal`].

use std::collections::HashMap;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::validation::refusing_violations_with_mapped_echoes;
use service_common::validation_message::{
    aws_validation_refusal_over_elements_quoting, quoting_the_submitted_collection,
};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service, Violation,
};

/// The `@pattern` rendering this service's refusals use: the expression WITHOUT the
/// whole-value anchors a model adds to express the match AWS's own pattern makes
/// implicitly (`service_common::validation::unanchored`, the one home for that rendering).
/// The CHECK always ran the model's pattern as written; this is the quoting alone.
fn as_this_service_quotes_it(pattern: &str) -> String {
    ::service_common::validation::unanchored(pattern)
}

/// The message a constraint refusal of `Op` carries, with both halves above stated, or
/// `None` when there is nothing to state.
///
/// `quoted_collection` answers, for a member path a clause is stated at, the collection that
/// member held — the one value no violation carries. `None` from it states the clause without
/// that value, which is the fail-safe direction.
fn stated_message<Op: Operation>(
    violations: Vec<Violation>,
    quoted_collection: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    aws_validation_refusal_over_elements_quoting(
        &Refusal::Invalid(violations),
        &[Op::INPUT_SCHEMA],
        as_this_service_quotes_it,
        quoted_collection,
    )
    .map(|refused| refused.message)
}

/// One modelled operation, served so that a constraint refusal states the collection the
/// request carried.
///
/// It makes no routing decision and holds no state of its own: the operation it wraps is the
/// one the generated routing table names for the same key, built from the SAME shared engine,
/// and a request this wrapper does not refuse is handed to it untouched — so the framework
/// still owns the whole deserialize -> validate -> handle -> serialize sequence and every
/// status, shape and message it answers with.
///
/// The cost is DELIBERATE: the request's body is read twice, here and in the operation's own
/// sequence, because the framework hands no already-built input to an operation. Which
/// operations pay it is this service's own choice ([`super::stated_error_bodies`]).
pub(super) struct QuotingSubmittedValues<Op> {
    operation: Op,
}

impl<Op> QuotingSubmittedValues<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub(super) fn new(operation: Op) -> Self {
        Self { operation }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedValues<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is
            // rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across that
            // await would make this future non-`Send`.
            if let Some(refused) = quoting_refusal::<Op>(svc, req) {
                return super::error_message_member::with_modelled_message_member(refused);
            }
            super::error_message_member::with_modelled_message_member(
                self.operation.run(svc, req, ctx).await,
            )
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The refusal `req` earns from `Op`'s own constraints, with the collection a clause about a
/// collection quotes — or `None` for a request those constraints do not refuse at all, which
/// is every request the operation goes on to serve.
fn quoting_refusal<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let (refusing, submitted_lists): (Vec<(Violation, _)>, HashMap<String, String>) =
        refusing_violations_with_mapped_echoes::<Op>(svc.definition().protocol(), req);
    if refusing.is_empty() {
        return None;
    }
    // A clause about a collection's own `@length` quotes the collection from the ECHO the
    // collecting walk recorded for that member; one stated AT a collection for a constraint
    // its elements broke quotes the whole list, which the same walk recorded under the member
    // path the clause is stated at.
    let violations: Vec<Violation> = refusing
        .into_iter()
        .map(|(violation, echo)| quoting_the_submitted_collection(violation, echo))
        .collect();
    let reported = violations.clone();
    let message = stated_message::<Op>(violations, |member| submitted_lists.get(member).cloned())?;
    Some(svc.definition().refusal_response(
        ::aws_smithy_schema_server::ProtocolError::Invalid(reported),
        Some(Refused {
            code: super::stated_error_bodies::CLOUD_HSM_INVALID_REQUEST.to_string(),
            message,
        }),
        req,
    ))
}
