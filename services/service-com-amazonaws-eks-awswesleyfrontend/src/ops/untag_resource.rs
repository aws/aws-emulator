// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//! IMPLEMENTED. PARSES `resourceArn` first — a value that is not a resource ARN of
//! this service is a `BadRequestException` (400), before any lookup — and then
//! resolves it to the resource that carries that ARN and REMOVES the named keys
//! from that
//! resource's own tag map, leaving every key the request did not name in place.
//! `ListTagsForResource` then reports what is left. A key the resource does not
//! carry is removed vacuously, so the call is idempotent: deleting the same keys
//! twice answers 200 both times. A well-formed ARN no resource in the store owns is
//! a `NotFoundException` (404).
//!
//! Every resource this service mints an ARN for is addressable here, through the one
//! resolution the whole tag plane shares
//! ([`crate::state::tagged_resource_for_arn`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UntagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes specified tags from an Amazon EKS resource.
//!
//! ## Input
//!
//! * `resourceArn` (String) **required** — The Amazon Resource Name (ARN) of the resource to
//!   delete tags from.
//! * `tagKeys` (TagKeyList, len 1..50) **required** — The keys of the tags to remove.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — This exception is thrown if the request contains a
//!   semantic error. The precise meaning will depend on the API, and will be documented in
//!   the error message.
//! * `NotFoundException` (HTTP 404) — A service resource associated with the request could
//!   not be found. Clients should not retry such requests.

#[allow(unused_imports)]
use super::types::*;
use crate::state::is_eks_resource_arn;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `ServerException` is widened in with `extra_errors`: it is a shape the SERVICE
// models (most other EKS operations list it) that `UntagResource` itself does not,
// and it is the only 500-status error channel this operation can answer with when
// the shared store cannot be reached. `BadRequestException` / `NotFoundException`
// would both blame the caller for a server fault.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#UntagResource",
    extra_errors = ["ServerException"]
)]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    BadRequestException(BadRequestException),
    NotFoundException(NotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::untag_resource::UntagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UntagResourceError = crate::scaffolded::Scaffolded<UntagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UntagResourceErrorModeled> for UntagResourceError {
    fn from(modeled: UntagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UntagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UntagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UntagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UntagResource<Eng>
{
    type Input = UntagResourceInput;
    type Output = UntagResourceOutput;
    type Error = UntagResourceError;

    const NAME: &'static str = "UntagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // THE INPUT FIRST: `resourceArn` names "the resource to delete tags from", so
        // a value that is not a resource ARN of this service names nothing this API
        // can address — a syntax failure on the input, which is what
        // `BadRequestException` ("the request contains a semantic error") reports. The
        // delete path answers it exactly as `TagResource` and `ListTagsForResource`
        // do, before any lookup: a malformed ARN is not an absent resource.
        if !is_eks_resource_arn(&input.resource_arn) {
            return Err(UntagResourceErrorModeled::BadRequestException(
                crate::state::invalid_resource_arn(),
            )
            .into());
        }

        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UntagResourceError::from(UntagResourceErrorModeled::ServerException(ServerException {
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // The tag plane states a resource it did not resolve in ONE fixed sentence,
        // shared with the read and the write path
        // ([`crate::state::tag_resource_not_found`]): it names nothing from the
        // request.
        let not_found = || {
            UntagResourceError::from(UntagResourceErrorModeled::NotFoundException(
                crate::state::tag_resource_not_found(),
            ))
        };

        let mut guard = store.lock();
        // The ARN itself names the region and the resource, so the resolution
        // addresses any resource this service minted that ARN for — in any region,
        // without the request's own credential scope
        // ([`crate::state::tagged_resource_for_arn`]).
        let Some(resource) = guard.tagged_resource_for_arn(&input.resource_arn) else {
            return Err(not_found());
        };
        let Some(tags) = guard.tags_for_mut(&resource) else {
            return Err(not_found());
        };

        // REMOVE the named keys and nothing else: a key the request does not name
        // survives, and a key the resource does not carry is removed vacuously — the
        // request asks for an ABSENCE that already holds, so repeating the call
        // answers the same 200 rather than a not-found for the tag.
        for tag_key in &input.tag_keys {
            tags.remove(tag_key);
        }

        Ok(UntagResourceOutput {})
    }
}
