// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//! IMPLEMENTED. PARSES `resourceArn` first — a value that is not a resource ARN of
//! this service is a `BadRequestException` (400), before any lookup — and then returns
//! the tag map STORED on the resource it names — create-time `tags` plus every later
//! `TagResource` merge — never an echo of the request. A well-formed ARN no resource
//! in the store owns is a `NotFoundException` (404).
//!
//! Every resource this service mints an ARN for is addressable here, through the one
//! resolution the whole tag plane shares
//! ([`crate::state::tagged_resource_for_arn`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListTagsForResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! List the tags for an Amazon EKS resource.
//!
//! ## Input
//!
//! * `resourceArn` (String) **required** — The Amazon Resource Name (ARN) that identifies the
//!   resource to list tags for.
//!
//! ## Output
//!
//! * `tags` (TagMap, len 1..50) — The tags for the resource.
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

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `ServerException` is widened in with `extra_errors`: it is a shape the SERVICE
// models (most other EKS operations list it) that `ListTagsForResource` itself does
// not, and it is the only 500-status error channel this operation can answer with
// when the shared store cannot be reached. `BadRequestException` /
// `NotFoundException` would both blame the caller for a server fault.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListTagsForResource",
    extra_errors = ["ServerException"]
)]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    BadRequestException(BadRequestException),
    NotFoundException(NotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_tags_for_resource::ListTagsForResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTagsForResourceError = crate::scaffolded::Scaffolded<ListTagsForResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTagsForResourceErrorModeled> for ListTagsForResourceError {
    fn from(modeled: ListTagsForResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTagsForResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTagsForResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTagsForResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTagsForResource<Eng>
{
    type Input = ListTagsForResourceInput;
    type Output = ListTagsForResourceOutput;
    type Error = ListTagsForResourceError;

    const NAME: &'static str = "ListTagsForResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_FOR_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // THE INPUT FIRST: `resourceArn` "identifies the resource to list tags for",
        // so a value that is not a resource ARN of this service names nothing this API
        // can address — a syntax failure on the input, which is what
        // `BadRequestException` ("the request contains a semantic error") reports. The
        // read path answers it exactly as `TagResource` does, before any lookup: a
        // malformed ARN is not an absent resource.
        if !crate::state::is_eks_resource_arn(&input.resource_arn) {
            return Err(ListTagsForResourceErrorModeled::BadRequestException(
                crate::state::invalid_resource_arn(),
            )
            .into());
        }

        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListTagsForResourceError::from(ListTagsForResourceErrorModeled::ServerException(
                ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // The tag plane states a resource it did not resolve in ONE fixed sentence,
        // shared with the write and the delete path
        // ([`crate::state::tag_resource_not_found`]): it names nothing from the
        // request.
        let not_found = || {
            ListTagsForResourceError::from(ListTagsForResourceErrorModeled::NotFoundException(
                crate::state::tag_resource_not_found(),
            ))
        };

        let guard = store.lock();
        // The ARN itself names the region and the resource, so the resolution
        // addresses any resource this service minted that ARN for — in any region,
        // without the request's own credential scope
        // ([`crate::state::tagged_resource_for_arn`]).
        let Some(resource) = guard.tagged_resource_for_arn(&input.resource_arn) else {
            return Err(not_found());
        };
        let Some(tags) = guard.tags_for(&resource) else {
            return Err(not_found());
        };

        Ok(ListTagsForResourceOutput {
            tags: Some(
                tags.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        })
    }
}
