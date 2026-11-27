// SPDX-License-Identifier: CC0-1.0

//! How this service states the VALUE half of a clause about a COLLECTION member whose
//! modelled `@length` a request broke.
//!
//! The framework length-checks a list or a map AT THE MEMBER, where the only rendering it
//! has for one is its own size summary (`<n> items`, `<n> entries`): text no caller sent,
//! where AWS quotes the collection the request carried. That collection is read back out of
//! the request by the one shared collector that renders a submitted value
//! (`service_common::validation::refusing_violations_with_echoes`, which owns every bound on
//! what one clause may quote) and restored into the clause by the one shared rule for it
//! (`service_common::validation_message::quoting_the_submitted_collection`). Only that half
//! is this layer's: the envelope, the clauses, the requirements and the member paths stay
//! [`super::stated_refusal`]'s, so which layer refused a request is not something a caller
//! can tell.
//!
//! The clause needs the REQUEST, and `Service::override_refusal` is handed the violations
//! alone — so the wording lives in a wrapper round the operation, the one place the request
//! is still in hand ([`StatingTheSubmittedCollection`]). It is mounted on each operation
//! whose input declares a collection member a caller can take outside its modelled bounds,
//! whatever that member is bound to: the batch state read's `campaignIds` and the dial
//! batch's `dialRequests` in the body, and an untag call's `tagKeys` in the query string.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema_server::ProtocolError;
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Service,
};
use ::std::marker::PhantomData;

/// One modelled operation, served so that a clause about a COLLECTION its request carried
/// quotes that collection ([the module docs](self)).
///
/// Nothing else is re-implemented: a request this layer has no wording for is handed to the
/// wrapper behind it, which serves the operation the service's routing table names and
/// announces the answer the way this service announces one. The violations it reads come
/// from the framework's OWN deserializer and constraint walk over the same input schema, so
/// this and the walk behind it cannot find a different set. The cost is DELIBERATE: such a
/// request is read twice, because the framework hands no already-built input to an operation.
pub struct StatingTheSubmittedCollection<O> {
    /// The media-type wrapper every routed request of this service is served through, which
    /// this one delegates to for every request it words no refusal for.
    announced: ::service_common::response_media_type::ResponseMediaTypeOperation,
    /// The operation whose input schema the request is read against. Held as a type alone:
    /// the walk below needs `O`'s schemas and nothing of the instance the routing table
    /// holds, which is the one that serves the request.
    operation: PhantomData<fn() -> O>,
}

impl<O: Operation> StatingTheSubmittedCollection<O> {
    /// Wrap the media-type wrapper this one delegates to.
    pub const fn new(
        announced: ::service_common::response_media_type::ResponseMediaTypeOperation,
    ) -> Self {
        Self {
            announced,
            operation: PhantomData,
        }
    }

    /// Whether `routing_key` names the operation this wrapper words refusals of — the
    /// operation's own modelled name, so the mounting below cannot name an operation this
    /// service does not declare.
    pub fn serves(&self, routing_key: &str) -> bool {
        routing_key == O::NAME
    }
}

impl<O> ErasedOperation for StatingTheSubmittedCollection<O>
where
    O: Operation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The definition is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`. The protocol is the one THIS REQUEST
            // is served in, so the request is read here exactly as the framework's own
            // sequence reads it.
            let refused = {
                let definition = svc.definition();
                let protocol = definition.wire_protocol(req).protocol();
                let refusing =
                    ::service_common::validation::refusing_violations_with_echoes::<O>(
                        protocol, req,
                    );
                super::stated_refusal_quoting_collections(&refusing).map(|(violations, stated)| {
                    definition.refusal_response(
                        ProtocolError::Invalid(violations),
                        Some(stated),
                        req,
                    )
                })
            };
            match refused {
                Some(response) => self.announced.announce(response),
                None => self.announced.run(svc, req, ctx).await,
            }
        })
    }

    /// The modelled auth schemes of the operation behind this wrapper: wrapping an
    /// operation must not change which schemes sign it. `None` is the wrapper's own
    /// deferral to the operation the routing key names, which is the one that serves the
    /// request.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.announced.auth_schemes()
    }
}

/// The protocol-resolved routing key of `req`, or `None` when it names no operation of
/// `definition` — the key [`StatingTheSubmittedCollection::serves`] is asked about.
pub fn routing_key<'r>(
    definition: &dyn ::smithy_server_runtime::ErasedServiceDefinition,
    req: &'r RtRequest,
) -> Option<&'r str> {
    definition.wire_protocol(req).protocol().routing_key(req).ok()
}
