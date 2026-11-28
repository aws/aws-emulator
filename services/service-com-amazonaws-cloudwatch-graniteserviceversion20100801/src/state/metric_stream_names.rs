// SPDX-License-Identifier: CC0-1.0

//! How the two metric-stream switches refuse a `Names` entry the list's element shape does not
//! admit.
//!
//! `StartMetricStreams` and `StopMetricStreams` both take `Names`, a list whose MEMBER carries
//! the length bounds of a metric stream's name
//! (`smithy_query(query="operation",operation="StartMetricStreams")`). The constraint belongs to
//! the element, and the framework's constraint walk reports a broken element constraint AT THAT
//! ELEMENT (`names.<position>.member`) with the one bound the element broke, where AWS states it
//! at the LIST member, quotes the list the request carried and names the element shape's WHOLE
//! declared set. Both halves of that difference are already single-homed in the shared renderer
//! ([`::service_common::validation_message::aws_validation_refusal_over_elements_quoting`], which
//! reads the set off the operation's own input schema and groups the clause at the list member)
//! and in the shared collector of a submitted list
//! ([`::service_common::validation::submitted_lists`]) — so nothing of the envelope, the clause,
//! the requirement or the member path is spelled here.
//!
//! What IS here is the judgement: the refusal has to be worded AHEAD of the framework's
//! sequence, because that sequence words the clause its own way, so the element bounds are read
//! off the same schema the walk validates against and applied to the names the request carried.
//! The CLASS the refusal travels under is the request-validation class this service refuses an
//! unacceptable value in ([`crate::scaffolded::Scaffolded::validation_error`], whose legacy query
//! code is what a client branches on), rather than the parameter fault these operations declare
//! for a name no stream could carry: a length outside the member's own bounds is the
//! request-validation layer's finding, which is the class the recordings of both switches carry.

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::{Violation, ViolationKind};
use ::aws_smithy_schema::serde::SerializableStruct;
use ::smithy_server_runtime::Refusal;

/// The `Names` member, as a violation path spells one (lowerCamel, the walk's own spelling).
const NAMES_MEMBER: &str = "names";

/// The refusal `names` earns from the constraints the list's element shape declares in
/// `schema`, or `None` when every entry is one that shape admits.
///
/// `input` is the request the names came from — the one thing that can answer what the `Names`
/// member HELD, which the clause quotes and no violation carries.
#[must_use]
pub fn constraint_refusal(
    names: &[String],
    input: &dyn SerializableStruct,
    schema: &'static Schema,
) -> Option<String> {
    let declared =
        ::service_common::validation_message::element_constraint_set(&[schema], NAMES_MEMBER)?;
    let broken: Vec<Violation> = names
        .iter()
        .enumerate()
        .filter_map(|(position, name)| {
            let kind = declared
                .iter()
                .find(|kind| breaks(name, kind))
                .cloned()?;
            Some(Violation {
                // The position the walk reports an element at, counted as it counts them.
                path: format!("{NAMES_MEMBER}.{}.member", position + 1),
                kind,
                value: Some(name.clone()),
                sensitive: false,
            })
        })
        .collect();
    if broken.is_empty() {
        return None;
    }
    let lists = ::service_common::validation::submitted_lists(input, schema);
    ::service_common::validation_message::aws_validation_refusal_over_elements_quoting(
        &Refusal::Invalid(broken),
        &[schema],
        crate::service::published_pattern::quoted_pattern,
        |member| lists.get(member).cloned(),
    )
    .map(|refused| refused.message)
}

/// Whether `name` breaks the element constraint `kind` the model declares.
///
/// Only the bounds a `Names` entry carries are judged, each counted the way the constraint
/// counts it (`@length` counts CHARACTERS). A constraint of another kind is left to the
/// framework's own walk, which applies every one of them: this judgement exists to WORD the
/// refusal AWS words differently, never to replace the walk.
fn breaks(name: &str, kind: &ViolationKind) -> bool {
    let length = i64::try_from(name.chars().count()).unwrap_or(i64::MAX);
    match kind {
        ViolationKind::MaxLength(max) => length > *max,
        ViolationKind::MinLength(min) => length < *min,
        _ => false,
    }
}
