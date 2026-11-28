// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Associates tags with a notification rule: the supplied `Tags` are MERGED into the
//! rule [`crate::state`] holds for the ARN (a key the rule already carries takes the new
//! value), and the resulting map is what the operation answers with — the same map
//! `DescribeNotificationRule` then reports.
//!
//! A call that leaves the rule holding a different map MODIFIED the rule, so it also moves
//! the instant the rule reports as the one it was most recently updated at; a call that
//! leaves the map exactly as it was modified nothing and moves nothing.
//!
//! The engine is held on `self` (constructor-injected); `ctx` carries request metadata
//! only — the request's SigV4 scope is the region whose rules this endpoint addresses.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Associates a set of provided tags with a notification rule.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule to tag.
//! * `Tags` (Tags) **required** — The list of tags to associate with the resource. Tag key
//!   names cannot start with "aws".
//!
//! ## Output
//!
//! * `Tags` (Tags) — The list of tags associated with the resource.
//!
//! ## Errors
//!
//! * `ConcurrentModificationException` (HTTP 400) — CodeStar Notifications can't complete the
//!   request because the resource is being modified by another process. Wait a few minutes
//!   and try again.
//! * `LimitExceededException` (HTTP 400) — One of the CodeStar Notifications limits has been
//!   exceeded. Limits apply to accounts, notification rules, notifications, resources, and
//!   targets. For more information, see Limits.
//! * `ResourceNotFoundException` (HTTP 404) — CodeStar Notifications can't find a resource
//!   that matches the provided ARN.
//! * `ValidationException` (HTTP 400) — One or more parameter values are not valid.
//! * `AccessDeniedException` (HTTP 403) — widened in through `extra_errors` (see below):
//!   the answer the resource-based authorization check gives an `Arn` that names no
//!   notification rule of this service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#TagResource",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConcurrentModificationException(ConcurrentModificationException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
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

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> TagResourceError {
    TagResourceErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// HTTP 400 `ValidationException`, the modelled shape — "One or more parameter values
/// are not valid."
fn invalid(message: impl Into<String>) -> TagResourceError {
    TagResourceErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// HTTP 404 `ResourceNotFoundException`: no resource of this account and region matches
/// the ARN the request named. The TAGGING paths state it over the whole submitted ARN,
/// which is how they address the resource.
fn not_found(arn: &str) -> TagResourceError {
    TagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(crate::state::no_resource_with_arn(arn)),
    })
    .into()
}

/// HTTP 400 `LimitExceededException`: the rule would hold more tags than this service
/// keeps for one — the bound that stops a caller growing a stored rule without limit.
fn over_tag_limit() -> TagResourceError {
    TagResourceErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(crate::state::over_tag_limit()),
    })
    .into()
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
        use crate::state as st;

        let region = st::request_region(ctx.parts());

        // Resource authorization comes FIRST, before any lookup: an `Arn` outside the
        // notification-rule form names no resource of this service, so nothing can
        // authorize the caller for it.
        if !st::is_notification_rule_arn(&input.arn) {
            return Err(not_authorized(&input.arn));
        }

        // The bound precedes the work: a request naming more tags than one rule may hold
        // can never succeed, so it is refused before anything is built from it.
        if input.tags.len() > st::MAX_TAGS_PER_RULE {
            return Err(over_tag_limit());
        }
        // The documented key rule ("Tag key names cannot start with \"aws\"") is the one
        // the create path applies to the same map, so it is applied — and worded — by the
        // same helper.
        let supplied = st::rule_tags_from(Some(&input.tags)).map_err(invalid)?;

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to tag, which is a "not written
            // for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "TagResource (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        let Some(rule) = store.rule_in_region_mut(&region, &input.arn) else {
            return Err(not_found(&input.arn));
        };

        // Tagging MERGES: a key the rule already carries takes the supplied value, and
        // the keys the request does not name are left alone (`UntagResource` is what
        // removes one). The result is bounded too — the supplied keys may be new ones on
        // top of the keys the rule already holds.
        let mut merged = rule.tags.clone();
        merged.extend(
            supplied
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        if merged.len() > st::MAX_TAGS_PER_RULE {
            return Err(over_tag_limit());
        }
        // Tagging MODIFIES the rule, so a call that leaves it holding a different map
        // moves the instant the rule reports as the one it was most recently updated at —
        // the same step every other mutator of a stored rule in this crate takes. A call
        // that leaves the map exactly as it was modified nothing, so it leaves that
        // instant alone too, which is what keeps a repeated request at the same state.
        if merged != rule.tags {
            rule.tags = merged;
            rule.last_modified_timestamp = st::now_timestamp();
        }

        Ok(TagResourceOutput {
            // The resulting map, read back off the rule that now holds it rather than
            // echoed from the request.
            tags: Some(
                rule.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        })
    }
}
