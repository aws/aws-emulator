// SPDX-License-Identifier: CC0-1.0

//! The templated EVENTBRIDGE target: which `Target$Arn` names one, and the constraint
//! the parameters such a target carries have to satisfy when a schedule is WRITTEN.
//!
//! `Target$EventBridgeParameters` is "the templated target type for the EventBridge
//! `PutEvents` API operation", and that parameter block is where the published event's
//! `Source` and `DetailType` come from — both REQUIRED members of it
//! (`EventBridgeParameters$Source`, `$DetailType`). A target may state them in
//! `Target$EventBridgePutEventsV2Parameters` instead, which declares the same two members
//! for the `PutEvents` of the event bus v2 API, so what identifies the published event is
//! read from whichever block states it ([`event_identity`]) and every rule below holds for
//! both.
//!
//! `Source` carries a `@pattern` the request boundary leaves UNAPPLIED: it is written
//! with look-around, which the regex engine that boundary compiles an expression with
//! cannot express, and an expression that does not compile is treated there as matching
//! every value. A constraint the model declares is the service's requirement however it
//! is applied, so the MODEL's own text is applied here instead, by a look-around-capable
//! engine ([`source_matches`]), and worded in the same clause every constraint refusal of
//! this service is stated in ([`crate::service::validation`]).
//!
//! What names an event bus is stated ONCE here ([`names_event_bus`]) and read by the
//! write path and by the dispatch half alike ([`super::invocation`]), so the kind a
//! schedule is accepted as is the kind its occurrence is performed as.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

use crate::ops::types::Target;

/// The ARN namespace of Amazon EventBridge, which is the `arnNamespace` of the service
/// whose `PutEvents` carries this kind of invocation.
pub(crate) const EVENTS_ARN_NAMESPACE: &str = "events";

/// The resource TYPE an Amazon EventBridge ARN carries when it names an EVENT BUS.
///
/// That service's ARNs name several kinds of resource (a rule, an archive, an API
/// destination), and `PutEvents` publishes onto a BUS alone, so the type is what decides
/// whether such an ARN names a templated EventBridge target.
const EVENT_BUS_RESOURCE_TYPE: &str = "event-bus";

/// The modelled `@pattern` of `EventBridgeParameters$Source`
/// (`smithy_query(query="shape", shape="com.amazonaws.scheduler#EventBridgeParameters")`).
///
/// The request boundary leaves it UNAPPLIED: the expression is written with look-around —
/// the leading `(?=[/\.\-_A-Za-z0-9]+)` and the negative `(?!aws\.)` — which the regex
/// engine the boundary compiles a pattern with cannot express, and an expression that does
/// not compile is treated there as matching every value. So the constraint is the model's
/// and the check is [`source_matches`], which matches this text as written rather than
/// restating any part of it.
const SOURCE_PATTERN: &str = r"^(?=[/\.\-_A-Za-z0-9]+)((?!aws\.).*)|(\$(\.[\w_-]+(\[(\d+|\*)\])*)*)$";

/// The step budget one match may spend.
///
/// The engine below BACKTRACKS, so a match has no linear-time guarantee in general and
/// the work one write may cost is bounded here rather than left to the shape of the
/// expression. Both inputs are already tame — the expression is a model constant, never
/// request text, and the value is a member the boundary has already held to its own
/// `@length` — so this is the backstop: a match that exhausts it is answered, never spun
/// on.
const BACKTRACK_LIMIT: usize = 1_000_000;

/// [`SOURCE_PATTERN`] compiled ONCE, by an engine whose syntax HAS look-around
/// (`fancy_regex`, as the sibling service crates that apply an unappliable `@pattern`
/// do), or `None` for an expression even that engine cannot compile.
static SOURCE_EXPRESSION: ::std::sync::LazyLock<Option<::fancy_regex::Regex>> =
    ::std::sync::LazyLock::new(|| {
        ::fancy_regex::RegexBuilder::new(SOURCE_PATTERN)
            .backtrack_limit(BACKTRACK_LIMIT)
            .build()
            .ok()
    });

/// Whether `source` satisfies [`SOURCE_PATTERN`], read as the FIND a Smithy `@pattern`
/// is — the same reading the framework's own check applies — over the MODEL's own text.
///
/// An expression even this engine cannot compile, and a match that exhausts its budget,
/// are both treated as satisfied: a value is never refused because of an expression
/// nothing could evaluate, which is exactly what the boundary does with a pattern it
/// cannot compile.
fn source_matches(source: &str) -> bool {
    SOURCE_EXPRESSION
        .as_ref()
        .is_none_or(|compiled| compiled.is_match(source).unwrap_or(true))
}

/// The member path a clause names the source of `Target$EventBridgeParameters` at: the
/// walk spells a path as the dotted lowerCamel member names from the root of the request,
/// so this is the one that member would have been reported at had the expression
/// compiled.
const SOURCE_PATH: &str = "target.eventBridgeParameters.source";

/// The same path for the source of `Target$EventBridgePutEventsV2Parameters`, the other
/// parameter block of this kind a target states.
const PUT_EVENTS_V2_SOURCE_PATH: &str = "target.eventBridgePutEventsV2Parameters.source";

/// The refusal a `Source` outside [`SOURCE_PATTERN`] is stated in: the constraint clause
/// the member would have been refused with had the expression compiled at the boundary,
/// quoting the value as submitted, the path of the member carrying it and the model's
/// expression as written — the rendering a
/// clause states it in (without the whole-value anchors the model writes around it) is
/// owned by the one place that words every clause of this service.
fn source_pattern_violation(source: &str, path: &str) -> String {
    crate::service::validation::self_applied(&Violation {
        path: path.to_string(),
        kind: ViolationKind::Pattern(SOURCE_PATTERN.to_string()),
        value: Some(source.to_string()),
        sensitive: false,
    })
}

/// What a templated EventBridge target states about the event it publishes: the `Source`
/// and `DetailType` its parameters carry, read from whichever parameter block of this
/// kind the target states.
///
/// Both blocks a schedule may state them in — `Target$EventBridgeParameters` and
/// `Target$EventBridgePutEventsV2Parameters` — declare the same two REQUIRED members for
/// the same `PutEvents` call, so what identifies the published event is read here once
/// and the dispatch half ([`super::invocation`]) carries it without caring which block
/// stated it.
pub(crate) struct EventIdentity<'a> {
    /// `EventBridgeParameters$Source` / `EventBridgePutEventsV2Parameters$Source`.
    pub(crate) source: &'a str,
    /// `EventBridgeParameters$DetailType` / `EventBridgePutEventsV2Parameters$DetailType`.
    pub(crate) detail_type: &'a str,
}

/// The event identity `target` states, or `None` for a target stating no parameter block
/// that carries one.
pub(crate) fn event_identity(target: &Target) -> Option<EventIdentity<'_>> {
    if let Some(parameters) = target.event_bridge_parameters.as_ref() {
        return Some(EventIdentity {
            source: &parameters.source,
            detail_type: &parameters.detail_type,
        });
    }
    target
        .event_bridge_put_events_v2_parameters
        .as_ref()
        .map(|parameters| EventIdentity {
            source: &parameters.source,
            detail_type: &parameters.detail_type,
        })
}

/// Every `Source` the target states, each beside the path of the member that carries it.
fn stated_sources(target: &Target) -> [Option<(&str, &'static str)>; 2] {
    [
        target
            .event_bridge_parameters
            .as_ref()
            .map(|parameters| (parameters.source.as_str(), SOURCE_PATH)),
        target
            .event_bridge_put_events_v2_parameters
            .as_ref()
            .map(|parameters| (parameters.source.as_str(), PUT_EVENTS_V2_SOURCE_PATH)),
    ]
}

/// Whether `parsed` names an EVENT BUS, which is the resource a templated EventBridge
/// target addresses.
pub(crate) fn names_event_bus(parsed: &::service_common::arn::Arn<'_>) -> bool {
    parsed.vendor == EVENTS_ARN_NAMESPACE && parsed.resource_type() == EVENT_BUS_RESOURCE_TYPE
}

/// Refuse a target whose `Source` is outside the expression that member declares.
///
/// A `Source` of a templated EventBridge target is constrained by a `@pattern` the
/// request boundary cannot apply ([`SOURCE_PATTERN`]), and a constraint the model declares
/// is the service's requirement however it is applied: a value outside the expression —
/// every value in the reserved namespace among them — is refused when the schedule is
/// WRITTEN, with the clause that member would have been refused with at the boundary.
/// It holds for EVERY parameter block that carries such a member
/// ([`stated_sources`]), each refused at its own member path.
///
/// `None` for a target stating no parameters that carry a source, and for one whose every
/// `Source` satisfies the expression; every other member of a target is constrained by the
/// model alone, so the boundary has already answered for it.
pub(crate) fn event_bridge_target_violation(target: &Target) -> Option<String> {
    stated_sources(target)
        .into_iter()
        .flatten()
        .find(|(source, _)| !source_matches(source))
        .map(|(source, path)| source_pattern_violation(source, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{
        EventBridgeParameters, EventBridgePutEventsV2Parameters,
        EventBridgePutRawEventsParameters, SqsParameters,
    };

    /// A target naming `arn`, with the parameters and input a row states.
    fn target(arn: &str, parameters: bool, input: Option<&str>) -> Target {
        Target {
            arn: arn.to_string(),
            role_arn: "arn:aws:iam::000000000000:role/scheduled".to_string(),
            input: input.map(ToString::to_string),
            event_bridge_parameters: parameters.then(|| EventBridgeParameters {
                detail_type: "Nightly Roll-Up".to_string(),
                source: "orders.nightly".to_string(),
            }),
            ..Target::default()
        }
    }

    /// The same target with `source` as the value of the one member under test.
    fn sourced(arn: &str, source: &str) -> Target {
        let stated = target(arn, true, None);
        Target {
            event_bridge_parameters: Some(EventBridgeParameters {
                detail_type: "Nightly Roll-Up".to_string(),
                source: source.to_string(),
            }),
            ..stated
        }
    }

    /// The same target stating its source in the v2 `PutEvents` block instead, which
    /// declares the same two members for the same call.
    fn v2_sourced(arn: &str, source: &str) -> Target {
        Target {
            event_bridge_parameters: None,
            event_bridge_put_events_v2_parameters: Some(EventBridgePutEventsV2Parameters {
                detail_type: "Nightly Roll-Up".to_string(),
                source: source.to_string(),
            }),
            ..target(arn, false, None)
        }
    }

    /// An event bus is named by an `events` ARN whose resource type is `event-bus`, and
    /// by nothing else: the rows differ along the vendor and along the resource type,
    /// including the other kinds of resource that service's own ARNs name.
    #[test]
    fn an_event_bus_is_named_by_the_resource_type_of_an_eventbridge_arn() {
        // (case, the ARN, whether it names an event bus)
        let rows: [(&str, &str, bool); 7] = [
            (
                "a bus of the account's own",
                "arn:aws:events:eu-west-2:000000000000:event-bus/orders",
                true,
            ),
            (
                "the default bus",
                "arn:aws:events:us-east-1:000000000000:event-bus/default",
                true,
            ),
            (
                "a partner bus, whose name carries separators",
                "arn:aws:events:us-east-1:000000000000:event-bus/aws.partner/example.com/1/stream",
                true,
            ),
            (
                "a rule of the same service",
                "arn:aws:events:eu-west-2:000000000000:rule/orders/nightly",
                false,
            ),
            (
                "an API destination of the same service",
                "arn:aws:events:eu-west-2:000000000000:api-destination/orders/1",
                false,
            ),
            (
                "a queue",
                "arn:aws:sqs:eu-west-2:000000000000:orders",
                false,
            ),
            (
                "a universal target, which names a call rather than a resource",
                "arn:aws:scheduler:::aws-sdk:sqs:sendMessage",
                false,
            ),
        ];

        for (case, arn, expected) in rows {
            let parsed =
                ::service_common::arn::parse(arn).unwrap_or_else(|| panic!("case {case:?}: {arn}"));
            assert_eq!(
                names_event_bus(&parsed),
                expected,
                "case {case:?}: expected names_event_bus={expected} for {arn}"
            );
        }
    }

    /// What the write refuses, over a SET of targets rather than one: the rows differ
    /// along the kind the ARN names, along WHICH parameter block states the source, along
    /// whether such parameters are stated at all, and along the value of the member whose
    /// constraint the boundary leaves unapplied.
    #[test]
    fn a_source_outside_the_expression_is_refused_whatever_the_target_names() {
        const BUS: &str = "arn:aws:events:eu-west-2:000000000000:event-bus/orders";
        const QUEUE: &str = "arn:aws:sqs:eu-west-2:000000000000:orders";
        // (case, the target, whether it is refused)
        let rows: [(&str, Target, bool); 8] = [
            (
                "a bus target whose source satisfies the expression",
                target(BUS, true, Some(r#"{"probe":"a"}"#)),
                false,
            ),
            (
                "a bus target whose source claims the reserved namespace",
                sourced(BUS, "aws.scheduler"),
                true,
            ),
            (
                "a bus target whose source opens outside the character class",
                sourced(BUS, " orders"),
                true,
            ),
            (
                // The constraint is the MEMBER's, so it holds wherever the parameters are
                // stated rather than only beside a bus ARN.
                "a queue target whose source claims the reserved namespace",
                sourced(QUEUE, "aws.sqs"),
                true,
            ),
            (
                // The same member in the OTHER block that declares it: the rule is the
                // member's, so it holds in whichever block a target states it.
                "a bus target whose v2 source claims the reserved namespace",
                v2_sourced(BUS, "aws.scheduler"),
                true,
            ),
            (
                "a bus target whose v2 source satisfies the expression",
                v2_sourced(BUS, "orders.nightly"),
                false,
            ),
            (
                "a queue target stating none of this kind's parameters",
                target(QUEUE, false, Some("a line of plain text, not JSON")),
                false,
            ),
            (
                "a bus target stating none of this kind's parameters",
                target(BUS, false, None),
                false,
            ),
        ];

        for (case, target, refused) in rows {
            let violation = event_bridge_target_violation(&target);
            assert_eq!(
                violation.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {violation:?}"
            );
        }
    }

    /// The `@pattern` `Source` declares, over a SET of values rather than one: the rows
    /// walk both branches of the expression — the opening character class and the reserved
    /// namespace its look-ahead excludes, and the reference path the other branch admits —
    /// including the near misses of that namespace, which it does NOT exclude.
    #[test]
    fn a_source_must_satisfy_the_expression_its_member_declares() {
        // (case, the value, whether the expression admits it)
        let rows: [(&str, &str, bool); 13] = [
            ("a custom namespace", "orders.nightly", true),
            ("a single word", "orders", true),
            ("a path-like namespace", "/orders/nightly", true),
            ("a value opening with a separator", "-orders", true),
            ("a digit", "1", true),
            ("the reserved namespace", "aws.scheduler", false),
            ("the reserved namespace alone", "aws.", false),
            // The look-ahead is anchored at the start: the same letters without the dot,
            // and the namespace INSIDE the value, are not the reserved prefix.
            ("the reserved letters without the dot", "aws", true),
            ("a word opening with the reserved letters", "awsome.orders", true),
            ("the reserved namespace inside the value", "my.aws.orders", true),
            ("a value opening with a character outside the class", " orders", false),
            // The other branch: a reference path, which the input transformer's own
            // vocabulary states and which may stand where a source does.
            ("a reference path", "$.detail.source[0]", true),
            // `[\w_-]` is Unicode-aware in the syntax the model's expression is written
            // against, so a step carrying a letter beyond ASCII is one the expression
            // admits — the engine applies that text as written rather than a narrower
            // reading of it.
            ("a reference path whose step is not ASCII", "$.caf\u{e9}", true),
        ];

        for (case, value, admitted) in rows {
            assert_eq!(
                source_matches(value),
                admitted,
                "case {case:?}: expected the expression to admit={admitted} for {value:?}"
            );
        }
    }

    /// A refused source is stated in the clause every constraint refusal of this service
    /// carries: the value as submitted, the path of the MEMBER that carried it, and the
    /// expression without the whole-value anchors the model writes it with.
    ///
    /// The rows differ along which parameter block states the source, so each is reported
    /// at its own member path rather than at one of them for both.
    #[test]
    fn a_refused_source_is_stated_as_a_constraint_clause() {
        const BUS: &str = "arn:aws:events:eu-west-2:000000000000:event-bus/orders";
        const RESERVED: &str = "aws.scheduler";
        // (case, the target, the path the clause names)
        let rows: [(&str, Target, &str); 2] = [
            (
                "a source of the block modelled first",
                sourced(BUS, RESERVED),
                SOURCE_PATH,
            ),
            (
                "a source of the v2 PutEvents block",
                v2_sourced(BUS, RESERVED),
                PUT_EVENTS_V2_SOURCE_PATH,
            ),
        ];

        for (case, target, path) in rows {
            let expected = format!(
                "1 validation error detected: Value '{RESERVED}' at '{path}' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: {}",
                ::service_common::validation::unanchored(SOURCE_PATTERN)
            );
            let violation = event_bridge_target_violation(&target);
            assert_eq!(
                violation.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {violation:?}"
            );
        }
    }

    /// What identifies the published event is read from whichever block states it, and a
    /// target stating no such block states no identity at all.
    #[test]
    fn the_event_identity_is_read_from_whichever_block_states_it() {
        /// One row: the case, the target, and the source and detail type it states.
        type IdentityRow = (&'static str, Target, Option<(&'static str, &'static str)>);
        const BUS: &str = "arn:aws:events:eu-west-2:000000000000:event-bus/orders";
        let rows: [IdentityRow; 4] = [
            (
                "the block modelled first",
                sourced(BUS, "orders.nightly"),
                Some(("orders.nightly", "Nightly Roll-Up")),
            ),
            (
                "the v2 PutEvents block",
                v2_sourced(BUS, "returns.intake"),
                Some(("returns.intake", "Nightly Roll-Up")),
            ),
            (
                "the raw-events block, which carries neither",
                Target {
                    event_bridge_put_raw_events_parameters: Some(
                        EventBridgePutRawEventsParameters {
                            metadata: Some(
                                [("tenant".to_string(), "acme".to_string())]
                                    .into_iter()
                                    .collect(),
                            ),
                        },
                    ),
                    ..target(BUS, false, None)
                },
                None,
            ),
            (
                "no parameters of this kind at all",
                target(BUS, false, None),
                None,
            ),
        ];

        for (case, target, expected) in rows {
            let stated = event_identity(&target)
                .map(|identity| (identity.source.to_string(), identity.detail_type.to_string()));
            let stated = stated
                .as_ref()
                .map(|(source, detail_type)| (source.as_str(), detail_type.as_str()));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the identity {expected:?}, got {stated:?}"
            );
        }
    }

    /// Another kind's parameters are no business of this rule: a queue target's own
    /// `SqsParameters` are accepted beside its ARN.
    #[test]
    fn another_kinds_parameters_are_left_alone() {
        let queue = Target {
            sqs_parameters: Some(SqsParameters {
                message_group_id: Some("orders".to_string()),
            }),
            ..target("arn:aws:sqs:eu-west-2:000000000000:orders.fifo", false, None)
        };
        let violation = event_bridge_target_violation(&queue);
        assert!(
            violation.is_none(),
            "case a queue target stating its own parameters: expected it accepted, got \
             {violation:?}"
        );
    }
}
