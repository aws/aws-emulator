// SPDX-License-Identifier: CC0-1.0

//! The modelled `@pattern`s of this model the framework's boundary cannot APPLY, applied
//! before any handler of this service runs, and stated in the refusal this service words.
//!
//! # The rule
//!
//! A request outside the constraints the model declares is refused before any handler runs,
//! over ONE counted envelope carrying one clause per constraint the request broke — the
//! value the caller submitted, the member path it sat at and the requirement it failed. That
//! is one refusal however many constraints the request broke, and a clause is stated for
//! EVERY one of them: a caller that fixes only the constraint a refusal reported, when its
//! value broke two, is refused again for a reason it was never told.
//!
//! Two halves of that envelope the framework's own walk cannot state, and this layer is
//! where both are stated:
//!
//! * a `@pattern` carrying LOOK-AROUND, which the boundary validator declares and treats as
//!   satisfied because its engine cannot compile it. Three members of this model carry one
//!   (the two namespace names whose expression reserves the `arn:` prefix, and the service
//!   name whose expression bounds its own length with a look-ahead), so a name AWS refuses
//!   as invalid input otherwise reaches a handler — and a name breaking BOTH that expression
//!   and its `@length` maximum is reported with the length clause alone.
//! * the ORDER two clauses about one member are stated in: AWS states that member's pattern
//!   requirement before its length requirement, where the walk emits the length clause
//!   first.
//!
//! Neither the members nor the expressions are named here: which members carry one is read
//! off each operation's own input schema, and the whole collection, the check and the order
//! are the shared home for this rule ([`::service_common::unapplied_patterns`]). What is
//! this service's own is only what a service chooses — the error CODE such a refusal travels
//! under, the words ([`super::enum_value_sets`], the one home for them) and how a rendered
//! response is answered ([`super::error_message_member`]).
//!
//! # Why a wrapper
//!
//! [`smithy_server_runtime::Service::override_refusal`] is handed the violations ALONE — no
//! request and no operation — so neither half can be stated there: the expression has to be
//! applied to the REQUEST, and the value a long clause quotes is already elided by the time
//! the violations reach that seam. A per-operation wrapper has both, which is why this is
//! one ([`ApplyingEveryModelledPattern`], served from [`super::submitted_collections`]
//! around the layer that quotes a submitted collection).
//!
//! The cost of the pass is the DELIBERATE one every per-operation refusal layer in this tree
//! pays: a request whose operation declares such an expression is read twice, here and in
//! the operation's own sequence, because the framework hands no already-built input to an
//! operation. An operation whose input declares none pays nothing — the schema is read
//! first, and a request it has nothing to say about is handed straight on.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::service_common::front_door::worded_rejection;
use ::service_common::submitted_collections::AnsweredAs;
use ::service_common::unapplied_patterns::{stated_together, unapplied_violations};
use ::service_common::validation::refusing_violations;
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, Service,
};
use std::marker::PhantomData;

/// One modelled operation of this service, served so that every constraint its request
/// breaks is stated — the ones the boundary validator applies and the ones it declares and
/// cannot.
///
/// It makes no routing decision and holds no state of its own: `served` is the same
/// operation the generated routing table names for this key (wrapped in the layers this
/// service already serves it through), built from the SAME shared engine, and a request this
/// wrapper has nothing to say about is handed to it untouched — so the framework still owns
/// the whole deserialize -> validate -> handle -> serialize sequence and every status, shape
/// and message it answers with.
pub(super) struct ApplyingEveryModelledPattern<Op> {
    /// The operation as this service otherwise serves it.
    served: Box<dyn ErasedOperation>,
    /// The error code a constraint refusal of this service carries.
    refusal_code: &'static str,
    /// How this service answers a response one of its layers rendered.
    answered_as: AnsweredAs,
    /// The operation this wrapper stands in front of, carried as a TYPE alone: the check
    /// needs its input shape to read the request into and its input schema to read the
    /// declared expressions off, and nothing else.
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ApplyingEveryModelledPattern<Op> {
    /// Serve `served` behind this check, under `refusal_code`.
    pub(super) fn new(
        served: Box<dyn ErasedOperation>,
        refusal_code: &'static str,
        answered_as: AnsweredAs,
    ) -> Self {
        Self {
            served,
            refusal_code,
            answered_as,
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ApplyingEveryModelledPattern<Op> {
    /// How this service refuses `req` when it breaks an expression the boundary validator
    /// will not apply, or `None` when it breaks none — which leaves the request to the
    /// layers behind this one, whose refusals are unchanged.
    ///
    /// The protocol is read, used and dropped inside this one function, so nothing borrowed
    /// from the service definition is held across the caller's await.
    fn refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        // The codec the request is SERVED in, resolved the way the framework's own dispatch
        // resolves it — per REQUEST, because this model declares more than one protocol and
        // each request is served in the one it names: a layer that read the request with a
        // different codec from the operation behind it would find a different violation set,
        // or none at all.
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().wire_protocol(req).protocol());
        // Read first, and for most requests the whole of this layer's work: an operation
        // whose input declares no such expression, and a request whose values satisfy the
        // ones it does, is handed straight on.
        let unapplied = unapplied_violations::<Op>(protocol, req);
        if unapplied.is_empty() {
            return None;
        }
        // The constraints the boundary DOES apply, from the same shared collecting walk the
        // framework's own refusal travels through — with each clause's value restored to the
        // bytes the caller sent, which is what an AWS constraint message echoes.
        let walked = refusing_violations::<Op>(protocol, req);
        let violations = stated_together(Op::INPUT_SCHEMA, walked, unapplied);
        // `stated_message` answers `None` for an empty violation set alone, which the guard
        // above has already left to the operation.
        let message = super::enum_value_sets::stated_message(&violations)?;
        Some(worded_rejection(
            svc,
            req,
            violations,
            Refused {
                code: self.refusal_code.to_string(),
                message,
            },
        ))
    }
}

impl<Op> ErasedOperation for ApplyingEveryModelledPattern<Op>
where
    Op: Operation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is rendered
            // through taken and dropped — BEFORE the await below: `&dyn
            // ErasedServiceDefinition` is not `Sync`, so holding one across that await would
            // make this future non-`Send`.
            if let Some(refused) = self.refusal(svc, req) {
                return (self.answered_as)(refused);
            }
            // The layers behind this one answer as this service answers, so the response is
            // passed through exactly as they wrote it.
            self.served.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.served.auth_schemes()
    }
}
