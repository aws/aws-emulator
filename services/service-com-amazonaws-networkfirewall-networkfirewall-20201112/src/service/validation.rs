// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the boundary validator refused before any handler ran.
//!
//! The framework checks a request against the constraints the model declares and hands the
//! service the refusal STRUCTURALLY — one violation per offending position, carrying the
//! member path it sits at, the constraint class it broke and the value it held — and leaves
//! the words to the service, because no model records them. [`refused`] is those words,
//! reached from [`crate::service`]'s `override_refusal`, the one seam that sees a refusal
//! taken before dispatch.
//!
//! Six rules distinguish this service's wording from the framework's default. One is keyed
//! to the OPERATION as well as to what was refused — the rule-group type, which is a
//! parameter of its own only on an operation that writes a rule group — and rules 0, 1 and 2
//! are keyed to nothing but the member and the constraint, so each of those holds for every
//! operation of this service; rules 3 and 5 are keyed to the SHAPE a member targets, since
//! what a requirement states about a shape is a property of that shape:
//!
//! 0. a value in a member that carries one of this service's RESOURCE IDENTIFIERS
//!    ([`REFERENCE_ARN_MEMBERS`], [`RESOURCE_NAME_MEMBERS`]), refused because it is outside
//!    the form or the length a `ResourceArn` or a `ResourceName` declares, is not a
//!    validation clause at all: it cannot be one of this service's ARNs, or a resource name
//!    at all. For an ARN that is the same defect the resolution path refuses once a value
//!    reaches a handler ([`crate::state::ARN_INVALID_FORMAT_MESSAGE`]); a NAME outside that
//!    form never reaches a handler at all, because the boundary holds every binding of it to
//!    the one `ResourceName` shape, so this seam is the only place that half is stated
//!    ([`crate::state::NAME_INVALID_FORMAT_MESSAGE`]). Either way it leaves as the modelled
//!    client error with that constant — never as an error shape this service's model does not
//!    declare. This one is keyed to member names alone, because what a value CAN be is a fact
//!    about the member's shape and not about the call it arrived on: the model says which
//!    members carry a resource's name or ARN;
//! 1. a clause names the offending MEMBER PATH and the CONSTRAINT it broke, and NOTHING of
//!    the value the caller sent — not the submitted bytes, and not the `null` the framework
//!    writes where a member was absent. The requirement states what the request must
//!    satisfy, which the one value that missed it does not help with, and an error body that
//!    quotes request content back turns a request into its own echo;
//! 2. a constraint the model declares on the ELEMENTS of a collection member is stated at
//!    that MEMBER — no element position and no `member` step — with every requirement the
//!    element shape declares nested inside `Member must satisfy constraint: [ … ]`. The whole
//!    set, not just the constraint that happened to fail: a caller is told what each element
//!    of that member has to satisfy. A constraint the COLLECTION itself broke (its own
//!    `@length`, its `@uniqueItems`), and one on a member inside a structure element, keep
//!    their own path and their own requirement;
//! 3. a requirement that quotes a `@pattern` quotes the expression AWS publishes for that
//!    member's SHAPE, which is the model's trait string except for the shapes whose published
//!    expression drops the anchor that OPENS it ([`UNANCHORED_AT_THE_OPEN_PATTERNS`]), and one
//!    that enumerates an enumerated type's value set states the set the SERVICE states —
//!    which for the rule-group type is wider than the public model's, and for the
//!    generated-rules type is the model's values in another order ([`SERVICE_VALUE_SETS`]).
//! 4. a RULE-GROUP TYPE outside the values the public API accepts, on an operation that
//!    WRITES a rule group ([`crate::service::served_operation::writes_a_rule_group`]), is no
//!    validation clause: the type is that operation's own parameter — it must match the
//!    document the same request carries — so the request is refused as the invalid rule-group
//!    type it is ([`crate::state::INVALID_RULE_GROUP_TYPE_MESSAGE`]) with the same modelled
//!    client error rule 0 answers with. Being no constraint of that input, the type is also
//!    not among the clauses a request that breaks a constraint the operation DOES declare is
//!    refused with: such a request never reaches the operation, so it is answered with that
//!    constraint's clause alone. On every operation that takes the same member to pick which
//!    rule groups to act on, a refused value keeps the clause rule 3 states it in.
//! 5. a member the request OMITTED, whose target is a NUMBER, is stated as the requirement the
//!    ZERO a numeric slot defaults to breaks — the `@range` bound the model declares on that
//!    member, below which that zero sits ([`NUMBER_VALUED_SHAPES`]) — because the boundary
//!    reads such a member into a slot that already holds zero, so an omitted number and a
//!    submitted zero are one value to it. A member whose own range ADMITS zero, and one whose
//!    target is no number at all (a string, a list, a structure — each read into a slot that
//!    holds nothing), break no bound by being absent and keep the absent-member clause.
//!
//! Rules 1 and 2 are AWS's own wording rather than this service's, and both are implemented
//! once for the fleet in `service_common::validation_message`, which this module DELEGATES to
//! rather than rewording: [`service_common::validation_message::aws_validation_refusal_over_elements_withholding`]
//! is the entry point that takes both halves (withhold every value, group over a collection's
//! elements), and the envelope, the member paths, the element sets and every requirement
//! sentence stay the framework's and that module's. So does the `@pattern` rendering rule 3
//! names: `service_common::validation` carries one function per dialect the fleet's
//! recordings show, and this service states WHICH of them it is recorded in. Rules 0 and 4, the
//! value sets of rule 3, the zero-default reading of rule 5, and that choice are the only
//! wording this service owns, because what each states is a fact about THIS service.
//!
//! One limit of that shared rendering, which it states too: an element set is read off the
//! operation input schemas codegen derives from the model, and a member schema carries its own
//! traits and not the members of the shape it targets — so only a TOP-LEVEL collection member
//! resolves, and a collection nested inside a structure keeps the element path and the one
//! requirement its element broke.

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{path, RangeTrait, Violation, ViolationKind};
use ::smithy_server_runtime::{Refusal, Refused};

/// The ARN members that carry a RESOURCE REFERENCE this service resolves itself, as a
/// violation path spells a top-level member (lowerCamel).
///
/// Rule 0 is keyed to these by member NAME, because an ARN either is one of this service's
/// resource ARNs or is not, whatever operation it arrived on. So the set holds only the ARN
/// members whose every binding IS such a reference — `FirewallArn` and `RuleGroupArn`.
/// `FirewallPolicyArn` is deliberately absent: it is half of `DescribeFirewallPolicy`'s pair,
/// but on `CreateFirewall` the same member name is a required reference to another resource
/// whose refusal for a malformed value is recorded as a validation clause
/// (`tests/all/validation_refusal_wording.rs`). Every other ARN-valued member — a
/// `ResourceArn` naming a resource to tag or share, a reference inside a submitted document —
/// keeps the refusal its own operation already answers.
const REFERENCE_ARN_MEMBERS: [&str; 2] = ["firewallArn", "ruleGroupArn"];

/// The resource-NAME members that carry such a reference, as a violation path spells a
/// top-level member (lowerCamel) — the other half of each resource's identity pair.
///
/// One per resource this service stores: `FirewallName`, `FirewallPolicyName`,
/// `RuleGroupName` and `TLSInspectionConfigurationName`, each targeting `ResourceName`
/// (`smithy_query(query="shape",shape="ResourceName")`: `@length` 1..128, `@pattern`
/// `^[a-zA-Z0-9-]+$`).
///
/// Keyed by member NAME on every operation, like the ARN half: a value either satisfies the
/// form a `ResourceName` declares or it names no resource of this service at all, and which
/// of those it is does not depend on whether the call identifies a resource that already
/// exists or names the one it is creating — the recorded refusal of a CREATE whose resource
/// name is outside that form is the same modelled client error carrying the same constant.
/// What a create takes BESIDE the name keeps its own refusal: the required ARN reference to
/// another resource is recorded as a validation clause, which is why `firewallPolicyArn` is
/// absent from the ARN half above.
const RESOURCE_NAME_MEMBERS: [&str; 4] = [
    "firewallName",
    "firewallPolicyName",
    "ruleGroupName",
    "tLSInspectionConfigurationName",
];

/// Enumerated value sets this service's validator states itself, each keyed by the set the
/// public model declares for the same type.
///
/// Two types need one, and for two different reasons — which is why the table restates the
/// whole set rather than editing one:
///
/// * the rule-group type is one enumeration service-side and a subset of it in the public
///   model: `smithy_query(query="shape",shape="RuleGroupType")` declares the values a
///   caller may name on a create, while a refusal naming the type enumerates every value the
///   service's own enumeration carries, in its order;
/// * the generated-rules type of a domain-list rule group carries the model's own four values
///   (`smithy_query(query="shape",shape="GeneratedRulesType")`) in the order the SERVICE
///   states them, which is not the model's declaration order: the alert behaviour is
///   enumerated before the reject one.
///
/// So the model is not the source for either requirement's text, and the sets below are what
/// the service states.
///
/// Keyed by the WHOLE modelled set, in the model's order, so only the types whose sets are
/// restated here are affected and every other enumerated member of this service keeps the
/// values its model declares.
const SERVICE_VALUE_SETS: &[(&[&str], &[&str])] = &[
    (MODELLED_RULE_GROUP_TYPES, SERVICE_RULE_GROUP_TYPES),
    (
        &["ALLOWLIST", "DENYLIST", "REJECTLIST", "ALERTLIST"],
        &["ALLOWLIST", "DENYLIST", "ALERTLIST", "REJECTLIST"],
    ),
];

/// The rule-group type values the PUBLIC model declares
/// (`smithy_query(query="shape",shape="RuleGroupType")`), in its declaration order —
/// which is how a violation about that type is recognised among this service's other
/// enumerated members (see [`SERVICE_VALUE_SETS`], keyed by the whole modelled set).
const MODELLED_RULE_GROUP_TYPES: &[&str] = &[
    "STATELESS",
    "STATEFUL",
    "STATEFUL_DOMAIN",
    "FIREWALL",
    "IPS",
];

/// The rule-group type values the SERVICE's own enumeration carries, in the order it
/// states them: the five the public model declares plus the one it does not.
const SERVICE_RULE_GROUP_TYPES: &[&str] = &[
    "FIREWALL",
    "STATELESS",
    "IPS",
    "STATEFUL",
    "IPS_DOMAIN",
    "STATEFUL_DOMAIN",
];

/// Every operation INPUT shape of this service, as the schemas codegen derives from the
/// Smithy model.
///
/// What they are read for: rule 2 states the whole constraint set an element shape declares,
/// and that set is a property of the model rather than of one request — a refusal carries
/// only the constraints a value actually broke. Codegen attaches a collection member's
/// element constraints to that member's schema, which is where the shared rendering reads
/// them, and where the walk read them to refuse the request.
///
/// ALL of them, not the ones one refusal happens to name: the rule holds for every
/// constrained collection of every operation. `grep -h 'super::types::.*_INPUT_SCHEMA'
/// src/ops/*.rs` names exactly these, one per operation file; one missing from this list is
/// not a wrong clause, only a clause stating the constraint that failed rather than the
/// element shape's whole set.
const INPUT_SCHEMAS: &[&Schema] = {
    use crate::ops::types;

    &[
        types::ACCEPT_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::ASSOCIATE_AVAILABILITY_ZONES_INPUT_SCHEMA,
        types::ASSOCIATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::ASSOCIATE_SUBNETS_INPUT_SCHEMA,
        types::CREATE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::CREATE_FIREWALL_INPUT_SCHEMA,
        types::CREATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::CREATE_FIREWALL_POLICY_V2_INPUT_SCHEMA,
        types::CREATE_RULE_GROUP_INPUT_SCHEMA,
        types::CREATE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_FIREWALL_INPUT_SCHEMA,
        types::DELETE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DELETE_FIREWALL_POLICY_V2_INPUT_SCHEMA,
        types::DELETE_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::DELETE_PROXY_INPUT_SCHEMA,
        types::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DELETE_RULE_GROUP_INPUT_SCHEMA,
        types::DELETE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_POLICY_V2_INPUT_SCHEMA,
        types::DESCRIBE_FLOW_OPERATION_INPUT_SCHEMA,
        types::DESCRIBE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_INPUT_SCHEMA,
        types::DESCRIBE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_SUMMARY_INPUT_SCHEMA,
        types::DESCRIBE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DISASSOCIATE_AVAILABILITY_ZONES_INPUT_SCHEMA,
        types::DISASSOCIATE_SUBNETS_INPUT_SCHEMA,
        types::GET_ANALYSIS_REPORT_RESULTS_INPUT_SCHEMA,
        types::LIST_ANALYSIS_REPORTS_INPUT_SCHEMA,
        types::LIST_CONTAINER_ASSOCIATIONS_INPUT_SCHEMA,
        types::LIST_CONTEXT_STORES_INPUT_SCHEMA,
        types::LIST_FIREWALLS_INPUT_SCHEMA,
        types::LIST_FIREWALL_POLICIES_INPUT_SCHEMA,
        types::LIST_FIREWALL_POLICIES_V2_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATIONS_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATION_RESULTS_INPUT_SCHEMA,
        types::LIST_RESOURCES_IN_CONTEXT_STORE_INPUT_SCHEMA,
        types::LIST_RULE_GROUPS_INPUT_SCHEMA,
        types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        types::LIST_TLS_INSPECTION_CONFIGURATIONS_INPUT_SCHEMA,
        types::LIST_VPC_ENDPOINT_ASSOCIATIONS_INPUT_SCHEMA,
        types::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        types::REJECT_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::START_ANALYSIS_REPORT_INPUT_SCHEMA,
        types::START_FLOW_CAPTURE_INPUT_SCHEMA,
        types::START_FLOW_FLUSH_INPUT_SCHEMA,
        types::TAG_RESOURCE_INPUT_SCHEMA,
        types::UNTAG_RESOURCE_INPUT_SCHEMA,
        types::UPDATE_AVAILABILITY_ZONE_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_ANALYSIS_SETTINGS_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_DELETE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_DESCRIPTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_ENCRYPTION_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_POLICY_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_POLICY_V2_INPUT_SCHEMA,
        types::UPDATE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_PROXY_SETTINGS_INPUT_SCHEMA,
        types::UPDATE_RULE_GROUP_INPUT_SCHEMA,
        types::UPDATE_SUBNET_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
    ]
};

/// The `@pattern`s whose PUBLISHED expression is not the trait string the model carries: the
/// ones AWS states WITHOUT the anchor that opens them.
///
/// The anchoring is a property of the SHAPE rather than of a member or an operation, and the
/// recorded refusals of this service differ on it: a refused `nextToken` states
/// `com.amazonaws.networkfirewall#PaginationToken`'s `^[0-9A-Za-z:\/+=]+$` as
/// `[0-9A-Za-z:\/+=]+$`, while a refused `vpcIds` element states
/// `com.amazonaws.networkfirewall#VpcId`'s `^vpc-[0-9a-f]+$` as the model spells it, anchors
/// and all. So the entries are keyed on the EXPRESSION — what is published belongs to the
/// shape, and so holds for every member targeting it, in any operation and inside the
/// constraint set stated at a collection of them — and every expression not named here is
/// stated as the model declares it.
///
/// The CHECK always runs the model's pattern as written; only this rendering drops the anchor.
const UNANCHORED_AT_THE_OPEN_PATTERNS: &[&str] = &[
    // `PaginationToken` (`@length` 1..4096), reached by the `nextToken` member of every
    // paginated operation of this service.
    r"^[0-9A-Za-z:\/+=]+$",
];

/// The expression a requirement states for the `@pattern` the model spells `pattern`: the
/// published form where AWS publishes one without its opening anchor
/// ([`UNANCHORED_AT_THE_OPEN_PATTERNS`]), and otherwise the model's own expression.
///
/// Both the keying and the DROP are `service-common`'s, so this service states only its own
/// rows and nothing here spells out what an anchor is.
fn stated_pattern(pattern: &str) -> String {
    ::service_common::validation::unanchored_at_the_open_where_published(
        pattern,
        UNANCHORED_AT_THE_OPEN_PATTERNS,
    )
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with its own code and status, and nothing about it is this service's to word.
///
/// Rule 0 is answered first and on its own: a refused resource REFERENCE is not a validation
/// clause this service states, it is the modelled refusal its handlers answer the same defect
/// with, and rule 4 is the same for a refused rule-group type on an operation that writes one.
/// Everything else takes the shared rendering, whose error code travels with the
/// message and names the one every operation of this service declares for a refused input.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if let Some(refused) = malformed_reference(violations) {
        return Some(refused);
    }
    // Rule 4 is answered next, and over the violations rather than over the rendering: on an
    // operation that WRITES a rule group, the type is the operation's own parameter, so it is
    // no constraint of that operation's input at all — the value reaches the operation, which
    // refuses it. It is therefore dropped from the set the clauses are rendered from, and the
    // modelled refusal is answered only when the boundary refused NOTHING ELSE: a request that
    // also breaks a constraint the operation does declare never reaches the operation, so what
    // it is refused with is that constraint's clause and the type is not mentioned.
    let (invalid_type, declared): (Vec<Violation>, Vec<Violation>) =
        if crate::service::served_operation::writes_a_rule_group() {
            violations.iter().cloned().partition(names_a_rule_group_type)
        } else {
            (Vec::new(), violations.to_vec())
        };
    if !invalid_type.is_empty() && declared.is_empty() {
        return Some(Refused {
            code: MODELLED_REFUSAL_CODE.to_owned(),
            message: crate::state::INVALID_RULE_GROUP_TYPE_MESSAGE.to_owned(),
        });
    }
    // Rules 3 and 5 are applied to the violations BEFORE the clauses are rendered, so the
    // clause itself, its member path and the words around the requirement stay the framework's.
    let stated: Vec<Violation> = declared
        .iter()
        .map(in_service_value_set)
        .map(|violation| as_the_zero_default_breaks(&violation))
        .collect();
    service_common::validation_message::aws_validation_refusal_over_elements_withholding(
        &Refusal::Invalid(stated),
        INPUT_SCHEMAS,
        // This service quotes a `@pattern` requirement as AWS PUBLISHES the expression for
        // that shape ([`stated_pattern`]).
        stated_pattern,
        // Rule 1: no clause of this service's refusals states a value, whichever member it
        // is about, so the decision is the same for every member path.
        |_| true,
    )
}

/// The error shape rules 0 and 4 answer with: the client error every operation of this service
/// declares for a request whose reference or whose rule-group type is bad, which is the shape
/// its handlers return for the same defect
/// (`smithy_query(query="shape",shape="InvalidRequestException")`).
const MODELLED_REFUSAL_CODE: &str = "InvalidRequestException";

/// Rule 4: whether `violation` is about a RULE-GROUP TYPE.
///
/// Keyed on the whole value set the public model declares for that type — the same keying
/// [`SERVICE_VALUE_SETS`] uses, since what a type enumerates is a property of the type and not
/// of one member — and nothing else: WHICH operations read that member as the type they write
/// is the one fact about the operation this wording needs, and it comes from
/// [`crate::service::served_operation::writes_a_rule_group`].
fn names_a_rule_group_type(violation: &Violation) -> bool {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return false;
    };
    declared
        .iter()
        .map(String::as_str)
        .eq(MODELLED_RULE_GROUP_TYPES.iter().copied())
}

/// Rule 0: the malformed-identifier refusal when EVERY violation is a member carrying one of
/// this service's resource identifiers whose value cannot be a resource ARN or a resource
/// name, and `None` otherwise.
///
/// A value outside the member's declared form and one outside its declared length are the
/// same answer: both are bounds a `ResourceArn` or a `ResourceName` has to satisfy to be one
/// at all. The message is one constant per half of the pair, and the ARN's is the one stated
/// when both halves are refused together, because that is the order the resolution path
/// checks them in (`crate::state`'s one name-or-ARN rule reads the ARN first). Nothing of the
/// value reaches either message.
fn malformed_reference(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let mut arn = false;
    for violation in violations {
        if !matches!(
            violation.kind,
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
        ) {
            return None;
        }
        let path = violation.path.as_str();
        if REFERENCE_ARN_MEMBERS.contains(&path) {
            arn = true;
        } else if !RESOURCE_NAME_MEMBERS.contains(&path) {
            return None;
        }
    }
    let message = if arn {
        crate::state::ARN_INVALID_FORMAT_MESSAGE
    } else {
        crate::state::NAME_INVALID_FORMAT_MESSAGE
    };
    Some(Refused {
        code: MODELLED_REFUSAL_CODE.to_owned(),
        message: message.to_owned(),
    })
}

/// `violation` with an enumerated type's value set stated as the SERVICE enumerates it, and
/// any other violation returned unchanged.
///
/// Only the value set is replaced: the member path, the constraint class and the clause's
/// wording keep coming from the one place that writes them.
fn in_service_value_set(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::EnumValue(stated_value_set(declared)),
        ..violation.clone()
    }
}

/// The structures of this service's request documents that declare a NUMBER-valued member,
/// as the schemas codegen derives from the Smithy model.
///
/// What they are read for: rule 5 states what the zero a numeric slot defaults to breaks, and
/// the bound it breaks is a property of the MODEL rather than of one request — a refusal for an
/// absent member carries no bound at all. Reading each bound off the member's own schema is
/// what keeps the number in the clause the model's: nothing here spells one out.
///
/// The set is every structure a request can carry a number in whose member the model marks
/// `@required`, so the rule is stated wherever it holds rather than for the one member a
/// recording named (`smithy_query(query="closure",operation="CreateFirewallPolicy,CreateRuleGroup")`:
/// `StatelessRuleGroupReference$Priority` and `StatelessRule$Priority` target `Priority`,
/// `@range` 1..65535; `PortRange$FromPort` and `$ToPort` target `PortRangeBound`, `@range`
/// 0..65535 — whose own bound ADMITS zero, so an absent one keeps the absent-member clause).
/// Every other number a request of this service can carry sits in an OPTIONAL member — a Next
/// Generation policy's firewall-section entry priority (`FirewallRuleGroupEntry$Priority`), a
/// page size, a flow age, an idle timeout — and an
/// optional member is never reported absent, so no other structure of a request document
/// declares one of the member names above under a bound of its own.
const NUMBER_VALUED_SHAPES: &[&Schema] = {
    use crate::ops::types;

    &[
        types::STATELESS_RULE_GROUP_REFERENCE_SCHEMA,
        types::STATELESS_RULE_SCHEMA,
        types::PORT_RANGE_SCHEMA,
    ]
};

/// Rule 5: `violation` stated as the constraint the ZERO a numeric slot defaults to breaks,
/// and unchanged wherever that zero breaks nothing.
///
/// Only an ABSENT member is restated, and only when it is a number whose `@range` does not
/// admit zero: the member path, the constraint class and the clause's words all keep coming
/// from the one place that writes them, and so does the bound, read off the model
/// ([`NUMBER_VALUED_SHAPES`]).
fn as_the_zero_default_breaks(violation: &Violation) -> Violation {
    if !matches!(violation.kind, ViolationKind::MissingRequiredMember) {
        return violation.clone();
    }
    let Some(range) = declared_range(member_at(&violation.path)) else {
        return violation.clone();
    };
    // The value under test is the zero a numeric slot holds before anything is read into it,
    // so the bound it breaks is the minimum when zero sits below it and the maximum when zero
    // sits above it; a range that admits zero breaks nothing.
    let kind = match (range.min, range.max) {
        (Some(min), _) if ZERO < min => ViolationKind::MinValue(bound(min)),
        (_, Some(max)) if ZERO > max => ViolationKind::MaxValue(bound(max)),
        _ => return violation.clone(),
    };
    Violation {
        kind,
        ..violation.clone()
    }
}

/// The value a numeric member holds when the request carried none.
const ZERO: f64 = 0.0;

/// The member a violation PATH is about: its last segment, which a path ends on whichever
/// collection or structure the member sits inside.
fn member_at(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// The `@range` the model declares on the member spelled `member` (lowerCamel, as a violation
/// path spells it), when every structure declaring that member declares the same one; `None`
/// for a member no [`NUMBER_VALUED_SHAPES`] entry declares, for one carrying no `@range`, and
/// for one whose shapes disagree — where no single bound is this service's to state.
fn declared_range(member: &str) -> Option<RangeTrait> {
    let mut found: Option<RangeTrait> = None;
    for shape in NUMBER_VALUED_SHAPES {
        for declared in shape.members() {
            if declared.member_name().map(path::member).as_deref() != Some(member) {
                continue;
            }
            let Some(range) = range_of(declared) else {
                continue;
            };
            match found {
                Some(carried) if carried != range => return None,
                _ => found = Some(range),
            }
        }
    }
    found
}

/// The `@range` carried on one member's schema, when the model declares one on it.
fn range_of(member: &Schema) -> Option<RangeTrait> {
    member
        .traits()?
        .get(&RangeTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<RangeTrait>()
        .copied()
}

/// A `@range` bound as a requirement states it — an integral bound with no trailing `.0` —
/// which is the rendering the framework gives the same bound when the walk reports the
/// violation itself.
fn bound(value: f64) -> String {
    if value.is_finite() && value.fract() == ZERO {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// The value set a requirement states for a member whose model declares `declared`: the
/// service's own set where [`SERVICE_VALUE_SETS`] restates it, and the modelled set
/// otherwise.
fn stated_value_set(declared: &[String]) -> Vec<String> {
    for (modelled, stated) in SERVICE_VALUE_SETS {
        if declared.len() == modelled.len()
            && declared
                .iter()
                .zip(modelled.iter())
                .all(|(declared, modelled)| declared == modelled)
        {
            return stated.iter().map(|value| (*value).to_owned()).collect();
        }
    }
    declared.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an enumerated value set is stated as the service enumerates it where it
    /// restates one, and as the model declares it everywhere else.
    ///
    /// The rows span: the restated set, a set of the same values in another order, a subset
    /// of it, a superset, an unrelated set of this model's and the empty set.
    #[test]
    fn a_value_set_is_restated_only_where_the_service_widens_it() {
        let widened = [
            "FIREWALL",
            "STATELESS",
            "IPS",
            "STATEFUL",
            "IPS_DOMAIN",
            "STATEFUL_DOMAIN",
        ];
        // (case, the set the model declares, the set a requirement states)
        let rows: [(&str, &[&str], &[&str]); 6] = [
            (
                "the rule-group type's modelled set",
                &["STATELESS", "STATEFUL", "STATEFUL_DOMAIN", "FIREWALL", "IPS"],
                &widened,
            ),
            (
                "the same values in another order",
                &["STATEFUL", "STATELESS", "STATEFUL_DOMAIN"],
                &["STATEFUL", "STATELESS", "STATEFUL_DOMAIN"],
            ),
            (
                "a subset of the restated set",
                &["STATELESS", "STATEFUL"],
                &["STATELESS", "STATEFUL"],
            ),
            ("the restated set itself", &widened, &widened),
            (
                "the stateful rule order, another enumeration of this model",
                &["DEFAULT_ACTION_ORDER", "STRICT_ORDER"],
                &["DEFAULT_ACTION_ORDER", "STRICT_ORDER"],
            ),
            ("no declared value at all", &[], &[]),
        ];
        for (case, declared, expected) in rows {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_owned()).collect();
            let stated = stated_value_set(&declared);
            assert_eq!(
                stated, expected,
                "case {case}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a `@pattern` is stated as AWS publishes the expression for that shape — the
    /// pagination token without the anchor that opens it, every other expression as the
    /// model declares it.
    ///
    /// The rows span the published expression itself, the two other expressions this
    /// service's recorded refusals state as declared, an expression carrying no anchor at
    /// all, and the empty expression.
    #[test]
    fn only_a_published_expression_drops_the_anchor_that_opens_it() {
        // (case, the model's expression, the expression a requirement states)
        let rows: [(&str, &str, &str); 5] = [
            (
                "the pagination token",
                r"^[0-9A-Za-z:\/+=]+$",
                r"[0-9A-Za-z:\/+=]+$",
            ),
            ("a VPC id", "^vpc-[0-9a-f]+$", "^vpc-[0-9a-f]+$"),
            ("a resource name", "^[a-zA-Z0-9-]+$", "^[a-zA-Z0-9-]+$"),
            ("an expression with no anchor", "arn:aws", "arn:aws"),
            ("no expression at all", "", ""),
        ];
        for (case, pattern, stated) in rows {
            let actual = stated_pattern(pattern);
            assert_eq!(
                actual, stated,
                "case {case}: expected {stated:?}, got {actual:?}"
            );
        }
    }

    /// RULE: an enumerated value set is stated in the ORDER the service states it, which for
    /// the generated-rules type of a domain-list rule group is not the model's declaration
    /// order.
    ///
    /// The rows span: the modelled set (reordered), the stated order arriving as declared
    /// (which is left alone, so a model that reordered its own declaration states the same
    /// set), a subset of it, and a set of the same size carrying one value the type does not
    /// declare.
    #[test]
    fn a_value_set_is_stated_in_the_order_the_service_states_it() {
        // (case, the set the model declares, the set a requirement states)
        let rows: [(&str, &[&str], &[&str]); 4] = [
            (
                "the generated-rules type's modelled set",
                &["ALLOWLIST", "DENYLIST", "REJECTLIST", "ALERTLIST"],
                &["ALLOWLIST", "DENYLIST", "ALERTLIST", "REJECTLIST"],
            ),
            (
                "the stated order itself",
                &["ALLOWLIST", "DENYLIST", "ALERTLIST", "REJECTLIST"],
                &["ALLOWLIST", "DENYLIST", "ALERTLIST", "REJECTLIST"],
            ),
            (
                "a subset of it",
                &["ALLOWLIST", "DENYLIST", "REJECTLIST"],
                &["ALLOWLIST", "DENYLIST", "REJECTLIST"],
            ),
            (
                "four values one of which the type does not declare",
                &["ALLOWLIST", "DENYLIST", "REJECTLIST", "DROPLIST"],
                &["ALLOWLIST", "DENYLIST", "REJECTLIST", "DROPLIST"],
            ),
        ];
        for (case, declared, expected) in rows {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_owned()).collect();
            let stated = stated_value_set(&declared);
            assert_eq!(
                stated, expected,
                "case {case}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: only an enumerated-value violation is touched; every other class reaches the
    /// shared rendering exactly as the walk reported it.
    #[test]
    fn no_other_violation_class_is_touched() {
        for kind in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxValue("100".to_owned()),
            ViolationKind::Pattern("^vpc-[0-9a-f]+$".to_owned()),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "vpcIds".to_owned(),
                kind: kind.clone(),
                value: Some("DMZVPC".to_owned()),
                sensitive: false,
            };
            let stated = in_service_value_set(&violation);
            assert_eq!(
                stated, violation,
                "case {kind:?}: expected the violation unchanged, got {stated:?}"
            );
        }
    }

    /// RULE: an ABSENT member is stated as the requirement the zero a numeric slot defaults
    /// to breaks, and left alone wherever that zero breaks nothing.
    ///
    /// The rows span: the two members whose target's range excludes zero, each at the path a
    /// different structure and element position gives it; a required number whose own range
    /// admits zero; a required member that is no number; and a member name no shape of this
    /// service declares. The bounds come from the model through [`declared_range`], so no row
    /// states one.
    #[test]
    fn an_absent_number_takes_the_requirement_its_zero_default_breaks() {
        // (case, the violation path, the class stated for it)
        let rows: &[(&str, &str, ViolationKind)] = &[
            (
                "a reference's priority inside a policy document",
                "firewallPolicy.statelessRuleGroupReferences.1.member.priority",
                ViolationKind::MinValue("1".to_owned()),
            ),
            (
                "a reference's priority at another element position",
                "firewallPolicy.statelessRuleGroupReferences.7.member.priority",
                ViolationKind::MinValue("1".to_owned()),
            ),
            (
                "a stateless rule's priority inside a rule-group document",
                "ruleGroup.rulesSource.statelessRulesAndCustomActions.statelessRules.1.member.\
                 priority",
                ViolationKind::MinValue("1".to_owned()),
            ),
            (
                "a port-range bound, whose range admits zero",
                "ruleGroup.rulesSource.statelessRulesAndCustomActions.statelessRules.1.member.\
                 ruleDefinition.matchAttributes.sourcePorts.1.member.fromPort",
                ViolationKind::MissingRequiredMember,
            ),
            (
                "a rule-group capacity, a number with no declared range",
                "capacity",
                ViolationKind::MissingRequiredMember,
            ),
            (
                "a required string",
                "firewallPolicyArn",
                ViolationKind::MissingRequiredMember,
            ),
            (
                "a member no shape of this service declares",
                "somethingElse",
                ViolationKind::MissingRequiredMember,
            ),
        ];
        for (case, path, expected) in rows {
            let violation = Violation {
                path: (*path).to_owned(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            };
            let stated = as_the_zero_default_breaks(&violation);
            assert_eq!(
                &stated.kind, expected,
                "case {case}: expected {expected:?}, got {:?}",
                stated.kind
            );
            assert_eq!(
                stated.path, violation.path,
                "case {case}: expected the member path kept, got {:?}",
                stated.path
            );
        }
    }

    /// RULE: only an ABSENT member is restated this way; a violation of any other class
    /// reaches the shared rendering exactly as the walk reported it, whatever member it names.
    #[test]
    fn no_other_violation_class_takes_the_zero_default_reading() {
        for kind in [
            ViolationKind::MinValue("1".to_owned()),
            ViolationKind::MaxValue("65535".to_owned()),
            ViolationKind::MinLength(1),
            ViolationKind::Pattern("^arn:aws".to_owned()),
            ViolationKind::EnumValue(vec!["STRICT_ORDER".to_owned()]),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "firewallPolicy.statelessRuleGroupReferences.1.member.priority".to_owned(),
                kind: kind.clone(),
                value: Some("0".to_owned()),
                sensitive: false,
            };
            let stated = as_the_zero_default_breaks(&violation);
            assert_eq!(
                stated, violation,
                "case {kind:?}: expected the violation unchanged, got {stated:?}"
            );
        }
    }

    /// The bound a requirement states is READ OFF the model, so the two members whose target
    /// is the priority shape carry the same one and a member of another numeric shape carries
    /// its own.
    #[test]
    fn a_declared_range_comes_from_the_members_own_schema() {
        let priority = declared_range("priority").expect("the priority members declare a range");
        assert_eq!(
            (priority.min, priority.max),
            (Some(1.0), Some(65535.0)),
            "case priority: expected the modelled range, got {priority:?}"
        );
        let from_port = declared_range("fromPort").expect("a port-range bound declares a range");
        assert_eq!(
            (from_port.min, from_port.max),
            (Some(0.0), Some(65535.0)),
            "case fromPort: expected the modelled range, got {from_port:?}"
        );
        assert!(
            declared_range("resourceArn").is_none(),
            "case resourceArn: expected no range on a member that is no number"
        );
    }
}
