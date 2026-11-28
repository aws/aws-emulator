// SPDX-License-Identifier: CC0-1.0

//! Serving `ListCoreNetworkEndpoints` off its own query-string binding.
//!
//! ## The rule this module implements
//!
//! A `@httpQueryParams` member whose value is a LIST cannot be read by the vendored HTTP
//! binding, which reads a map of strings and collects every query parameter into it: the
//! request is refused 400 before any handler runs as soon as it carries one query
//! parameter — `max_results` and `next_token` included. That gap is filed as
//! `issues/things-that-do-not-work/20261022T170500Z-gap-smithy-schema-server-httpqueryparams-cannot-read-a-map-of-value-lists.md`
//! and the binding is read-only from a service crate, so this one operation binds its own
//! input ([`crate::ops::list_core_network_endpoints::input_from_query`]) and is served
//! through this gate.
//!
//! The gate drives the SAME sequence the centralized walk does — bind the input, call the
//! handler, serialize the output or the modelled error through this service's protocol —
//! and nothing else: the behaviour stays the operation's own `handle`. The model declares
//! no constraint on any member of this input (`maxResults` is a plain `Integer`,
//! `nextToken` a plain string and `filters` an unbounded map), so there is no constraint
//! walk to run between the two: what a request can still get wrong — a page smaller than
//! one item, a token this service never issued, a filter naming no member — the handler
//! refuses itself.
//!
//! Delete this module, the binding it serves and the seam in [`crate::service`] that
//! reaches it once the framework reads a list-valued `@httpQueryParams` map.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationError, Service,
};

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype` and in the body's `__type`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// `ValidationException`'s modelled status (`@httpError(400)` on
/// `com.amazonaws.route53globalresolver#ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The operation this gate fronts.
type Listing<Eng> = crate::ops::list_core_network_endpoints::ListCoreNetworkEndpoints<Eng>;

/// Serves `ListCoreNetworkEndpoints` off the input binding in
/// [`crate::ops::list_core_network_endpoints::input_from_query`].
pub(crate) struct ListCoreNetworkEndpointsFromQuery<Eng> {
    operation: Listing<Eng>,
}

impl<Eng> ListCoreNetworkEndpointsFromQuery<Eng> {
    /// Built from the SAME shared engine the generated entry threads into every operation,
    /// so this gate and the rest of the service read one store.
    pub(crate) fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: Listing::new(engine),
        }
    }

    /// The routing key the protocol resolves for the operation this gate fronts — the
    /// operation's own modelled name, so this file writes no literal route.
    pub(crate) fn routing_key() -> &'static str
    where
        Eng: Send + Sync + 'static,
    {
        <Listing<Eng> as Operation>::NAME
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ListCoreNetworkEndpointsFromQuery<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let query = req.uri().split_once('?').map_or("", |(_, query)| query);
            let input = match crate::ops::list_core_network_endpoints::input_from_query(query) {
                Ok(input) => input,
                // A member the request carries that cannot become its modelled type:
                // stated in the same modelled `ValidationException` as a value outside a
                // declared constraint, since both are the caller's to fix.
                Err(refused) => {
                    return protocol
                        .serialize_error(
                            &refused,
                            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                            VALIDATION_EXCEPTION_CODE,
                            VALIDATION_EXCEPTION_STATUS,
                        )
                        .unwrap_or_else(|error| {
                            svc.definition().framework_error_response(error, req)
                        })
                }
            };
            let response = match self.operation.handle(input, ctx).await {
                Ok(output) => protocol.serialize_output(
                    &output,
                    <Listing<Eng> as Operation>::OUTPUT_SCHEMA,
                    <Listing<Eng> as Operation>::NAME,
                    <Listing<Eng> as Operation>::SUCCESS_STATUS,
                ),
                Err(error) => protocol.serialize_error(
                    error.as_serializable(),
                    error.variant_schema(),
                    error.shape_name(),
                    error.http_status(),
                ),
            };
            response.unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
        })
    }
}
