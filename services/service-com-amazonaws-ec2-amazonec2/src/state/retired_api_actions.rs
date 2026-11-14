// SPDX-License-Identifier: CC0-1.0

//! The refusal EC2 answers a call to an API ACTION IT NO LONGER SERVES with.
//!
//! Some actions of this API are retired: the model's own documentation for them says the
//! action "is no longer supported", and the live service serves no call to one — it refuses
//! every call with HTTP 400 [`UNSUPPORTED_OPERATION`] and one fixed sentence,
//! `<Action> API is not supported.`, which is what the recorded response to a retired
//! action's call carries. The sentence names the ACTION and nothing of the request, and that
//! is the rule rather than an accident: nothing a caller could change about the request
//! would make the action servable, so there is no parameter, resource or account state for
//! the refusal to name.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches [`api_not_supported`] as `crate::state::<item>`
//! like every other refusal of this module.
//!
//! A retired action is NOT the same as a feature no endpoint offers (an accelerator that
//! reached end of life, `crate::ops::describe_elastic_gpus`): that one is refused with the
//! same code and a different sentence, about the functionality rather than about the action.
//! Both are distinguished from the scaffold's HTTP 501, which says nobody has written the
//! operation yet — a claim that is false for an action the real service does not serve
//! either.

use super::*;

/// Refuse a call to an API action EC2 no longer serves: HTTP 400 [`UNSUPPORTED_OPERATION`]
/// with EC2's sentence for a retired action.
///
/// `action` is the operation's own modelled name — a handler passes `Self::NAME`, the name
/// the model gives the operation the router resolved — never a value read out of a request,
/// so the sentence can only ever name an action of this API.
///
/// The ONE place that wording lives, so every retired action of this service is refused the
/// same way: `CancelBundleTask` and `BundleInstance`, the two the model documents as no
/// longer supported, both answer through here.
pub fn api_not_supported<E>(action: &str) -> Scaffolded<E> {
    unsupported_operation(format!("{action} API is not supported."))
}
