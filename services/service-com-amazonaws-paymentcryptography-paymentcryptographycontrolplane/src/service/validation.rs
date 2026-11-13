// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint violations the
//! framework's own boundary walk found become the sentence a 400 `ValidationException`
//! carries.
//!
//! Nothing here accepts or refuses anything: what is ENFORCED is always the model's, and
//! every violation handed to [`refusal`] comes from the framework's walk over this
//! operation's own input schema. This module only chooses the words, which no model records.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is `<N> validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `;
//! 2. a clause states the value as SUBMITTED, the member's dotted lowerCamel path and the
//!    requirement it failed, in the framework's own words;
//! 3. a reported `@pattern` is the expression without the `^` / `$` anchors a model writes
//!    to turn a find into a whole-value match — the check still runs the model's pattern as
//!    written, only the rendering drops them;
//! 4. a violation on the ELEMENTS of a list is stated once, at the LIST member, with the
//!    element shape's whole declared constraint set nested as
//!    `Member must satisfy constraint: [<requirement>]` and the list itself as the value.
//!
//! The envelope, the clause, the member paths, the anchor rendering and the element grouping
//! are all `service_common`'s, so this service cannot word them differently from any other;
//! what this file states is only WHICH of those renderings this service's wire shows.
//!
//! The status (400) and the error code are the ones the framework already answers a
//! constraint failure with on this `awsJson1_0` service: a chosen refusal replaces the code
//! and the message together, so the code is restated rather than changed.

use std::collections::HashMap;

use aws_smithy_schema::Schema;
use smithy_server_runtime::{Refusal, Refused, Violation};

/// This service's wording for `violations`, or `None` when there is nothing to word.
///
/// `input_schemas` are the operation input shapes the element constraint sets of rule 4 are
/// read off: codegen attaches a list member's element constraints to that member's schema,
/// and a refusal carries only the constraints that FAILED, so a set read off the violations
/// alone would be missing the ones the value satisfied.
///
/// `lists` is every list of scalar elements the request carried, keyed by the member path the
/// walk reports that list at and already rendered — the value rule 4 states at the list
/// member, which no violation carries and only the request can answer. An empty map leaves
/// such a clause with no value at all.
pub(super) fn refusal(
    violations: &[Violation],
    input_schemas: &[&'static Schema],
    lists: &HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    service_common::validation_message::aws_validation_refusal_over_elements_quoting(
        // The shared renderer takes the refusal structurally, as the framework hands one to
        // `Service::override_refusal`; both of this service's refusal paths reach it through
        // here, so neither can drift into a second envelope.
        &Refusal::Invalid(violations.to_vec()),
        input_schemas,
        service_common::validation::unanchored,
        |collection| lists.get(collection).cloned(),
    )
}
