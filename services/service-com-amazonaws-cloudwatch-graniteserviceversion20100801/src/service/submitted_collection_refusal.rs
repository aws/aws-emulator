// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a COLLECTION the model does not admit — one carrying more entries
//! than the maximum its own member declares, and one whose ELEMENTS broke a constraint of their
//! own shape — on the operations whose model declares none of this service's parameter faults.
//!
//! Which CLASS a boundary refusal carries is a property of the OPERATION — the rule the rest of
//! [`super`] words its refusals by. An operation whose model declares this service's parameter
//! faults answers one of THEM, naming the collection and the cap it broke
//! (`super::collection_cap_refusal`); an operation that declares none keeps the class of the
//! request-validation layer in front of the service
//! (`super::query_compatible_errors::VALIDATION`) — the class the framework's own constraint
//! walk already answers, and the one a handler-side precondition of this service is refused
//! under (`crate::scaffolded::Scaffolded::validation_error`) — so a client branches on one
//! class whichever layer refused its request.
//!
//! What this layer adds to that class is the VALUE half of the clause. The framework's walk
//! length-checks a collection AT THE MEMBER and has no rendering for a list or a map, so it
//! stores its own summary of one (`<n> items`): text no caller sent and none can repair
//! against, where AWS quotes the collection the request carried. That collection is read back
//! out of the request by the one shared collector that renders a submitted value
//! (`service_common::validation::refusing_violations_with_mapped_echoes`, which owns every bound
//! on what one clause may quote) and restored into the clause by the one shared rule for it
//! (`service_common::validation_message::quoting_the_submitted_collection`). Only that half is
//! this layer's: the envelope, the clauses, the requirements and the member paths are the
//! shared renderer's and the framework's, so this and the walk behind it cannot drift.
//!
//! The other difference is WHERE a constraint one of a collection's ELEMENTS broke is stated.
//! The framework checks an element's own shape per element and reports it AT that element
//! (`<collection>.<position>.member`) with the one constraint that failed, where AWS reports it
//! at the COLLECTION member, quotes the collection that member held and names the element
//! shape's WHOLE set — so two offending elements, and two constraints of one element, are ONE
//! validation error about one member. Both halves are the shared renderer's
//! (`service_common::validation_message::aws_validation_refusal_over_elements_stating`, which
//! owns the grouping, and `element_set_requirement`, which owns the sentence around the set);
//! what is this service's is the SET a clause enumerates for an enumerated element shape, which
//! is the set it publishes for that type (`super::published_enum_values`).
//!
//! The clause needs the REQUEST, and `Service::override_refusal` is handed the violations alone
//! — so the wording lives in a wrapper round the operation, the one place the request is still
//! in hand. It holds for every collection of every operation it is mounted on, at any depth:
//! the alarm names a mute rule targets is the recorded case, and the actions an alarm writer
//! takes, the names an alarm read or a state switch lists and the dimensions a metric's alarms
//! are looked up by are the same rule.

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{Violation, ViolationKind};
use service_common::validation::SubmittedEcho;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service,
};
use std::collections::HashMap;

/// One modelled operation, served so that a collection over its modelled maximum is refused in
/// the class and the words AWS refuses one with ([the module docs](self)).
///
/// Nothing else is re-implemented: a request this layer has no wording for is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize
/// and every modelled error, and the violations it reads come from the framework's OWN
/// deserializer and constraint walk over the same input schema, so this and the walk behind it
/// cannot find a different set. The cost is DELIBERATE: such a request is read twice, because
/// the framework hands no already-built input to an operation.
pub struct StatingTheSubmittedCollection<O> {
    operation: O,
}

impl<O> StatingTheSubmittedCollection<O> {
    /// Wrap the operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatingTheSubmittedCollection<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`. It is the protocol THIS REQUEST is
            // served in (`super::request_protocol`), so a query-protocol call is read here
            // exactly as it is in the framework's own sequence.
            let refused = {
                let protocol = svc.definition().wire_protocol(req).protocol();
                let (refusing, lists) =
                    service_common::validation::refusing_violations_with_mapped_echoes::<O>(
                        protocol, req,
                    );
                collection_refusal(&refusing, &lists, O::INPUT_SCHEMA)
                    .and_then(|refused| super::refusal_response(protocol, &refused))
            };
            match refused {
                Some(response) => response,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The refusal of a request whose COLLECTION the model does not admit — one carrying more
/// entries than its member's modelled maximum, or one whose ELEMENTS broke a constraint of
/// their own shape — or `None` when nothing in `refusing` is one.
///
/// Every violation this service ENFORCES is stated, in the shared renderer's envelope, with the
/// collection each clause is about quoted as the request carried it (`lists`, the collections
/// that request held, keyed by the member path the walk reports each at): a request that breaks
/// a collection's maximum and something else is one refusal naming both, which is what the
/// envelope's clause count is for. A constraint this service does not enforce on the wire
/// (`super::is_unenforced_constraint`) is left out of the clauses, so a refusal never states a
/// requirement this service does not apply, and a request whose only violations are those is
/// left to the sequence behind this layer.
fn collection_refusal(
    refusing: &[(Violation, SubmittedEcho)],
    lists: &HashMap<String, String>,
    input_schema: &'static Schema,
) -> Option<Refused> {
    let enforced: Vec<&(Violation, SubmittedEcho)> = refusing
        .iter()
        .filter(|(violation, _)| !super::is_unenforced_constraint(violation))
        .collect();
    if !enforced.iter().any(|(violation, _)| {
        is_collection_maximum(violation) || is_a_stated_element(violation, input_schema)
    }) {
        return None;
    }
    let stated = Refusal::Invalid(
        enforced
            .into_iter()
            .map(|(violation, echo)| {
                service_common::validation_message::quoting_the_submitted_collection(
                    violation.clone(),
                    echo.clone(),
                )
            })
            .collect(),
    );
    // The whole envelope, the grouping and every clause are the shared renderer's; only the
    // CLASS, the quoted collection and the value set an element clause enumerates are this
    // layer's. The `@pattern` rendering is the one dialect this service states a pattern
    // requirement in, whichever layer refused the request (`super::unanchored_pattern_refusal`).
    let message = service_common::validation_message::aws_validation_refusal_over_elements_stating(
        &stated,
        super::published_pattern::quoted_pattern,
        |collection| lists.get(collection).cloned(),
        |collection| element_set_requirement(collection, input_schema),
    )?
    .message;
    Some(Refused {
        code: super::query_compatible_errors::VALIDATION.shape_name.to_string(),
        message,
    })
}

/// Whether `violation` is an ELEMENT of a collection this layer states the element shape's set
/// at ([`element_set_requirement`]).
fn is_a_stated_element(violation: &Violation, input_schema: &'static Schema) -> bool {
    service_common::validation::list_of_element(&violation.path)
        .is_some_and(|collection| element_set_requirement(collection, input_schema).is_some())
}

/// The requirement a clause states at the collection member `collection` for a constraint one
/// of its ELEMENTS broke, or `None` when this layer states none for that member.
///
/// The set is the one the ELEMENT SHAPE declares, read off the operation's own input schema by
/// the shared reading of it (`service_common::validation_message::element_constraint_set`) —
/// the whole set, not just the constraint that happened to fail, which is what lets a caller
/// repair the request — with an ENUMERATED element shape's values stated as this service
/// enumerates a value set (`super::published_enum_values`). Every sentence is the framework's
/// own, nested by the shared rule for it.
///
/// Keyed on the element shape being an ENUMERATED type and nothing else, so it holds for every
/// such collection of every operation mounted with the wrapper above: the kinds of alarm an
/// alarm read is narrowed by are the recorded case. A collection whose elements carry other
/// constraints keeps the framework's own per-element clause, which is the class and the clause
/// its own recordings carry (`super::empty_required_value_refusal` words the one such class
/// this service restates).
fn element_set_requirement(collection: &str, input_schema: &'static Schema) -> Option<String> {
    let declared =
        service_common::validation_message::element_constraint_set(&[input_schema], collection)?;
    if !declared
        .iter()
        .any(|kind| matches!(kind, ViolationKind::EnumValue(_)))
    {
        return None;
    }
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| match kind {
            ViolationKind::EnumValue(values) => service_common::validation_message::requirement(
                ViolationKind::EnumValue(super::published_enum_values(&values)),
            ),
            other => service_common::validation_message::requirement(other),
        })
        .collect();
    Some(service_common::validation_message::element_set_requirement(
        &requirements,
    ))
}

/// Whether `violation` is a COLLECTION carrying more entries than its member's modelled maximum.
///
/// Keyed on the CLASS of violation alone — a maximum length broken by a value the walk rendered
/// as a collection's SIZE, which is a rendering no caller can send for a scalar member
/// (`service_common::validation_message::submitted_collection`, the shared reading of that
/// summary) — so it holds for a list and for a map, at any depth of any operation mounted with
/// the wrapper above.
fn is_collection_maximum(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(_))
        && service_common::validation_message::submitted_collection(violation).is_some()
}
