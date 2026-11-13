// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//! IMPLEMENTED. PARSES `resourceArn` first — a value that is not a resource ARN of
//! this service is a `BadRequestException` (400), before any lookup — and then
//! resolves it to the resource that carries that ARN and MERGES the supplied tags
//! into that resource's own tag map, so a key already present is overwritten and
//! every other key is kept. `ListTagsForResource` then reports the merged map. A
//! well-formed ARN no resource in the store owns is a `NotFoundException` (404).
//!
//! Every resource this service mints an ARN for is addressable here, through the one
//! resolution the whole tag plane shares
//! ([`crate::state::tagged_resource_for_arn`]): the tags land on that resource's own
//! record, so they are the tags its own read reports.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `TagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Associates the specified tags to an Amazon EKS resource with the specified resourceArn.
//! If existing tags on a resource are not specified in the request parameters, they aren't
//! changed. When a resource is deleted, the tags associated with that resource are also
//! deleted. Tags that you create for Amazon EKS resources don't propagate to any other
//! resources associated with the cluster. For example, if you tag a cluster with this
//! operation, that tag doesn't automatically propagate to the subnets and nodes associated
//! with the cluster.
//!
//! ## Input
//!
//! * `resourceArn` (String) **required** — The Amazon Resource Name (ARN) of the resource to
//!   add tags to.
//! * `tags` (TagMap, len 1..50) **required** — Metadata that assists with categorization and
//!   organization. Each tag consists of a key and an optional value. You define both. Tags
//!   don't propagate to any other cluster or Amazon Web Services resources.
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
use crate::state::{is_eks_resource_arn, MAX_TAGS_PER_RESOURCE};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `ServerException` is widened in with `extra_errors`: it is a shape the SERVICE
// models (most other EKS operations list it) that `TagResource` itself does not, and
// it is the only 500-status error channel this operation can answer with when the
// shared store cannot be reached. `BadRequestException` / `NotFoundException` would
// both blame the caller for a server fault.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#TagResource",
    extra_errors = ["ServerException"]
)]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    BadRequestException(BadRequestException),
    NotFoundException(NotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::tag_resource::TagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TagResourceError = crate::scaffolded::Scaffolded<TagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TagResourceErrorModeled> for TagResourceError {
    fn from(modeled: TagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TagResource<Eng>
{
    type Input = TagResourceInput;
    type Output = TagResourceOutput;
    type Error = TagResourceError;

    const NAME: &'static str = "TagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // THE INPUT FIRST: `resourceArn` names "the resource to add tags to", so a
        // value that is not a resource ARN of this service names nothing this API can
        // address — a syntax failure on the input, which is what
        // `BadRequestException` ("the request contains a semantic error") reports.
        // Nothing is looked up for it: "you sent garbage" and "the resource is gone"
        // are different answers to a caller, and only the second is a `NotFound`.
        if !is_eks_resource_arn(&input.resource_arn) {
            return Err(
                TagResourceErrorModeled::BadRequestException(crate::state::invalid_resource_arn())
                    .into(),
            );
        }

        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TagResourceError::from(TagResourceErrorModeled::ServerException(ServerException {
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // The tag plane states a resource it did not resolve in ONE fixed sentence,
        // shared with the read and the delete path
        // ([`crate::state::tag_resource_not_found`]): it names nothing from the
        // request.
        let not_found = || {
            TagResourceError::from(TagResourceErrorModeled::NotFoundException(
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

        // The 50-tag ceiling applies to the resource's ACCUMULATED map, not just to
        // one request's `TagMap` (which the model already bounds at 1..50, refused
        // before dispatch by `crate::service::tag_contract`). Count the merge before
        // applying it, so repeated calls with fresh keys are refused at the limit
        // instead of growing the record without bound. Nothing is written on the
        // refusal path, and the refusal is the SAME sentence the modelled bound
        // earns: one ceiling, one wording, and it echoes nothing the request carried.
        let merged_size = tags
            .keys()
            .chain(input.tags.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if merged_size > MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(crate::state::tag_ceiling_message(MAX_TAGS_PER_RESOURCE)),
                },
            )
            .into());
        }

        // MERGE, not replace: a key the caller supplies wins, every other stored key
        // survives.
        for (key, value) in input.tags {
            tags.insert(key, value);
        }

        Ok(TagResourceOutput {})
    }
}
