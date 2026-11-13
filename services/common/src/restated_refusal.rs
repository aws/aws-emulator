// SPDX-License-Identifier: CC0-1.0

//! The ONE plumbing for a service that answers a refusal the FRAMEWORK rendered in an
//! envelope only the REQUEST and the whole rendered response can decide.
//!
//! # Why a service needs this
//!
//! The framework refuses a request before any handler runs — an absent `@required` member, a
//! value outside a declared bound — and asks the service for the words through
//! `Service::override_refusal`. That seam is handed the violations ALONE: not the request,
//! not the operation, and it can return only an error code and one message. So a service
//! whose recorded wire shows the envelope of such a refusal depending on WHICH operation was
//! addressed — the body member its text travels in, the shape its body names, whether the
//! rejected value is quoted back — cannot state that there, and the framework's own renderer
//! (`vendor/smithy-server-runtime/src/pre_dispatch.rs`, one envelope for every service) is
//! agent-immutable.
//!
//! The seam that CAN see both is an operation wrapper handed back from
//! `Service::override_operation`: it serves the modelled operation the routing table names,
//! so the framework keeps the whole deserialize → validate → handle → serialize walk and
//! every status and code it answers with, and then sees the request beside the response.
//!
//! This module carries the two halves of that, so no service inlines them again:
//!
//! * [`note_refusal`] / the scope [`RestatedRefusalOperation`] opens — the words the refusal
//!   seam chose for the request being served, recorded as that seam runs and read back once
//!   the response exists. Scoped around ONE operation's run, so what is noted belongs to that
//!   request and to no other, and nothing is carried between requests.
//! * [`RestatedRefusalOperation`] — the wrapper itself, which delegates to the routed
//!   operation and hands the response, the request and whatever was noted to the service's own
//!   restatement ([`Restatement`]). What that restatement DOES is the service's: the body
//!   rewriting a JSON service needs for it is [`crate::error_detail_member`].
//!
//! Nothing else about a response is touched here: the status, the headers and every body byte
//! stay with the renderer that owns them unless the service's own restatement changes them,
//! and a response the router renders outside the walk — an unroutable request, a
//! transport-level failure, the authentication refusal — never passes through this wrapper at
//! all.
//!
//! # Services still carrying their own copy
//!
//! This plumbing predates the module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it:
//! `service-com-amazonaws-evs-amazonelasticvmwareservice` (`src/service/validation_envelope.rs`,
//! which notes the whole modelled shape rather than its text) and
//! `service-com-amazonaws-kinesisvideo-kinesisvideo-20170930`
//! (`src/service/error_message_member.rs`, whose wrapper needs no note because both of its
//! envelopes carry the same words). Those copies are to be MIGRATED onto this module; they
//! are left in place only because this change does not own those services' issues, and each is
//! covered by its own suite. Nothing new should inline the plumbing again — call this module.

use std::cell::RefCell;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Service,
};

tokio::task_local! {
    /// The words a service chose for the framework-level refusal of the request this task is
    /// serving, recorded by its `Service::override_refusal` and read back by
    /// [`RestatedRefusalOperation::run`].
    ///
    /// Empty means nothing was noted: either the request was not refused at all, or it was
    /// refused for something the service does not word itself.
    static NOTED_REFUSAL: RefCell<Option<String>>;
}

/// Record the words a service states for the framework-level refusal of the request being
/// served, so its restatement can report them once the response exists.
///
/// A refusal raised outside an operation's run — the authentication refusal the router renders
/// before the service is asked anything — is recorded nowhere and changes nothing.
pub fn note_refusal(words: &str) {
    let _ = NOTED_REFUSAL.try_with(|slot| {
        *slot.borrow_mut() = Some(words.to_string());
    });
}

/// The words noted for this request, taken out of the scope so they are read once.
fn noted_refusal() -> Option<String> {
    NOTED_REFUSAL
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten()
}

/// How a service answers one served request: the request it was serving, the response the
/// framework rendered, and the words [`note_refusal`] recorded for it, if any.
///
/// A plain function pointer rather than a closure, so the wrapper a service holds in a field
/// is `Copy` and names no lifetime.
pub type Restatement = fn(&RtRequest, RtResponse, Option<String>) -> RtResponse;

/// Whether `definition`'s generated routing table can serve this request — the condition
/// under which a service claims it for [`RestatedRefusalOperation`].
///
/// A request the table cannot serve is declined rather than claimed, so the router answers it
/// as the unroutable request it is instead of handing it to a wrapper that would have to
/// guess.
#[must_use]
pub fn routes(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
    routed(definition, req).is_some()
}

/// The operation `definition`'s generated routing table names for this request, if any.
///
/// The ONE resolution here: [`routes`] and [`RestatedRefusalOperation::run`] are both
/// expressed through it, so the claim and the delegation target cannot drift apart.
fn routed<'s>(
    definition: &'s dyn ErasedServiceDefinition,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// Serves the modelled operation the service's routing table names for the request, exactly as
/// that table would, then hands what it answered to the service's own [`Restatement`].
///
/// It holds no operation of its own: the one it delegates to is the one the routing table
/// names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] — a wrapper held on the service cannot borrow from the entry it is
/// held beside, which is why the key is resolved here rather than carried in.
#[derive(Debug, Clone, Copy)]
pub struct RestatedRefusalOperation {
    restate: Restatement,
}

impl RestatedRefusalOperation {
    /// The wrapper that answers through `restate`.
    #[must_use]
    pub const fn new(restate: Restatement) -> Self {
        Self { restate }
    }
}

impl ErasedOperation for RestatedRefusalOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let Some(op) = routed(svc.definition(), req) else {
                // Unreachable through a `Service::override_operation` that claims a request
                // only when the routing table holds its key ([`routes`]); answered as the
                // framework answers an unroutable request rather than guessed at.
                return svc.definition().routing_error_response(req);
            };
            NOTED_REFUSAL
                .scope(RefCell::new(None), async move {
                    let resp = op.run(svc, req, ctx).await;
                    (self.restate)(req, resp, noted_refusal())
                })
                .await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is the
        // operation this wrapper delegates to, so its model-declared schemes decide and this
        // wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A response at 400 carrying `body`.
    fn response(body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"),
            SdkBody::from(body),
        )
    }

    /// Words noted inside one scope are the words that scope reads back, and the slot is
    /// emptied by the read, so nothing carries into the next request.
    #[tokio::test]
    async fn the_words_noted_for_a_request_are_read_back_once() {
        NOTED_REFUSAL
            .scope(RefCell::new(None), async {
                assert_eq!(noted_refusal(), None, "a fresh scope notes nothing");
                note_refusal("the service's words");
                assert_eq!(
                    noted_refusal().as_deref(),
                    Some("the service's words"),
                    "expected the noted words"
                );
                assert_eq!(noted_refusal(), None, "the words are read once");
            })
            .await;
    }

    /// Outside any scope — the authentication refusal the router renders before an operation
    /// is served — noting is a no-op rather than a panic.
    #[tokio::test]
    async fn noting_outside_a_served_request_changes_nothing() {
        note_refusal("nowhere to put these");
        assert_eq!(noted_refusal(), None, "expected nothing noted");
    }

    /// A restatement is handed the response its service rendered, and may answer with its
    /// own.
    #[tokio::test]
    async fn a_restatement_answers_for_the_response_it_was_handed() {
        fn restate(_req: &RtRequest, resp: RtResponse, noted: Option<String>) -> RtResponse {
            match noted {
                Some(words) => response(&words),
                None => resp,
            }
        }
        let wrapper = RestatedRefusalOperation::new(restate);
        let req = RtRequest::new(SdkBody::empty());
        let answered = (wrapper.restate)(&req, response("rendered"), Some("restated".to_string()));
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            b"restated",
            "expected the restatement's own body"
        );
        let kept = (wrapper.restate)(&req, response("rendered"), None);
        assert_eq!(
            kept.body().bytes().expect("bytes"),
            b"rendered",
            "expected the rendered body kept when nothing was noted"
        );
    }
}
