// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//!
//! Removes the tags the request names from the notification rule [`crate::state`] holds
//! for the ARN: a key the rule carries leaves it, a key it does not carry is nothing to
//! remove, and the keys the request does not name are left alone (`TagResource` is what
//! adds one). The map `DescribeNotificationRule` and `ListTagsForResource` then report is
//! what is left.
//!
//! A call that removed a key MODIFIED the rule, so it also moves the instant the rule
//! reports as the one it was most recently updated at; a call that removed nothing
//! modified nothing and moves nothing.
//!
//! The engine is held on `self` (constructor-injected); `ctx` carries request metadata
//! only — the request's SigV4 scope is the region whose rules this endpoint addresses.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json")
//! ```
//!
//! Removes the association between one or more provided tags and a notification rule.
//!
//! ## Input
//!
//! * `Arn` (NotificationRuleArn, pattern
//!   ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$)
//!   **required** — The Amazon Resource Name (ARN) of the notification rule from which to
//!   remove the tags.
//! * `TagKeys` (TagKeys) **required** — The key names of the tags to remove.
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
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models the
// shape (`@httpError 403`, "you do not have sufficient permissions") and it is the answer
// the resource-based authorization check gives an `Arn` that names no notification rule,
// which every operation addressing a rule by ARN applies before it looks one up.
#[smithy_macros::operation_error(
    "com.amazonaws.codestarnotifications#UntagResource",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConcurrentModificationException(ConcurrentModificationException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
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

/// HTTP 403 `AccessDeniedException`: the caller is not authorized for the resource the
/// request named, stated in this service's one text for the class.
fn not_authorized(arn: &str) -> UntagResourceError {
    UntagResourceErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::not_authorized_for(arn)),
    })
    .into()
}

/// HTTP 404 `ResourceNotFoundException`: no resource of this account and region matches
/// the ARN the request named. The TAGGING paths state it over the whole submitted ARN,
/// which is how they address the resource.
fn not_found(arn: &str) -> UntagResourceError {
    UntagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(crate::state::no_resource_with_arn(arn)),
    })
    .into()
}

/// HTTP 400 `LimitExceededException`: the request names more tag keys than one rule holds
/// at all — the same bound the tagging path applies, in this service's one text for it.
fn over_tag_limit() -> UntagResourceError {
    UntagResourceErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(crate::state::over_tag_limit()),
    })
    .into()
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
        use crate::state as st;

        let region = st::request_region(ctx.parts());

        // Resource authorization comes FIRST, before any lookup: an `Arn` outside the
        // notification-rule form names no resource of this service, so nothing can
        // authorize the caller for it. A well-formed ARN no rule holds is the 404 below.
        if !st::is_notification_rule_arn(&input.arn) {
            return Err(not_authorized(&input.arn));
        }

        // The bound precedes the work: a rule holds at most `MAX_TAGS_PER_RULE` tags, so a
        // request naming more keys than that names keys no rule can hold, and it is
        // refused before anything is walked.
        if input.tag_keys.len() > st::MAX_TAGS_PER_RULE {
            return Err(over_tag_limit());
        }

        let Some(state) = st::service_state(&*self.engine) else {
            // This service's store is the shared `InMemoryEngine`. Mounted on any other
            // engine there is no notification-rule store to untag, which is a "not
            // written for this engine" condition, not a client error.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "UntagResource (no state store on this engine)",
            ));
        };
        let mut store = state.lock();

        let Some(rule) = store.rule_in_region_mut(&region, &input.arn) else {
            return Err(not_found(&input.arn));
        };

        // Removal is by KEY and nothing else: a key the rule carries leaves it with its
        // value, a key it does not carry leaves the map as it was (so the same request
        // repeated reaches the same state), and no other key is touched.
        let mut removed = false;
        for key in &input.tag_keys {
            removed |= rule.tags.remove(key).is_some();
        }
        // Removing a key MODIFIES the rule, so the instant it reports as the one it was
        // most recently updated at moves to this write — the same step every other
        // mutator of a stored rule in this crate takes. A call that removed nothing
        // modified nothing, so that instant stays where it was.
        if removed {
            rule.last_modified_timestamp = st::now_timestamp();
        }

        Ok(UntagResourceOutput {})
    }
}
