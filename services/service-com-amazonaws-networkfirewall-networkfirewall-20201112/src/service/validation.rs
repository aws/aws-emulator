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
//! Four rules distinguish this service's wording from the framework's default. The first is
//! keyed to MEMBERS — the references this service resolves itself, one half of which is a
//! reference only on an operation acting on a resource that already exists — and the next two
//! are keyed to nothing at all, so each of those holds for every constrained member of every
//! operation of this service; the last is keyed to the SHAPE a member targets, since what a
//! requirement states about a shape is a property of that shape:
//!
//! 0. a value in a member that carries a RESOURCE REFERENCE this service resolves itself
//!    ([`REFERENCE_ARN_MEMBERS`], [`REFERENCE_NAME_MEMBERS`]), refused because it is outside
//!    the form or the length a `ResourceArn` or a `ResourceName` declares, is not a
//!    validation clause at all: it cannot be one of this service's ARNs, or a resource name
//!    at all. For an ARN that is the same defect the resolution path refuses once a value
//!    reaches a handler ([`crate::state::ARN_INVALID_FORMAT_MESSAGE`]); a NAME outside that
//!    form never reaches a handler at all, because the boundary holds every binding of it to
//!    the one `ResourceName` shape, so this seam is the only place that half is stated
//!    ([`crate::state::NAME_INVALID_FORMAT_MESSAGE`]). Either way it leaves as the modelled
//!    client error with that constant — never as an error shape this service's model does not
//!    declare. This one is keyed to member names because a reference is a member-level fact:
//!    the model says which members name a resource;
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
//!
//! Rules 1 and 2 are AWS's own wording rather than this service's, and both are implemented
//! once for the fleet in `service_common::validation_message`, which this module DELEGATES to
//! rather than rewording: [`service_common::validation_message::aws_validation_refusal_over_elements_withholding`]
//! is the entry point that takes both halves (withhold every value, group over a collection's
//! elements), and the envelope, the member paths, the element sets and every requirement
//! sentence stay the framework's and that module's. So does the `@pattern` rendering rule 3
//! names: `service_common::validation` carries one function per dialect the fleet's
//! recordings show, and this service states WHICH of them it is recorded in. Rule 0, the
//! value sets of rule 3, and that choice are the only wording this service owns, because what
//! each states is a fact about THIS service.
//!
//! One limit of that shared rendering, which it states too: an element set is read off the
//! operation input schemas codegen derives from the model, and a member schema carries its own
//! traits and not the members of the shape it targets — so only a TOP-LEVEL collection member
//! resolves, and a collection nested inside a structure keeps the element path and the one
//! requirement its element broke.

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
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
/// Unlike an ARN, a resource name is a reference only on an operation acting on a resource
/// that ALREADY EXISTS: on a create the same member name carries the name of the resource
/// being created, which is content the request supplies and which the recorded refusals state
/// as a validation clause. That is the one fact about the OPERATION this wording needs, and
/// [`crate::service::served_operation::serves_an_existing_resource`] is where it comes from.
const REFERENCE_NAME_MEMBERS: [&str; 4] = [
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
///   model: `smithy_query(query="shape",shape="RuleGroupType")` declares the three values a
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
    (
        &["STATELESS", "STATEFUL", "STATEFUL_DOMAIN"],
        &[
            "FIREWALL",
            "STATELESS",
            "IPS",
            "STATEFUL",
            "IPS_DOMAIN",
            "STATEFUL_DOMAIN",
        ],
    ),
    (
        &["ALLOWLIST", "DENYLIST", "REJECTLIST", "ALERTLIST"],
        &["ALLOWLIST", "DENYLIST", "ALERTLIST", "REJECTLIST"],
    ),
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
        types::ATTACH_RULE_GROUPS_TO_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::CREATE_FIREWALL_INPUT_SCHEMA,
        types::CREATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::CREATE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_PROXY_INPUT_SCHEMA,
        types::CREATE_PROXY_RULES_INPUT_SCHEMA,
        types::CREATE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::CREATE_RULE_GROUP_INPUT_SCHEMA,
        types::CREATE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_FIREWALL_INPUT_SCHEMA,
        types::DELETE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DELETE_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::DELETE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_PROXY_INPUT_SCHEMA,
        types::DELETE_PROXY_RULES_INPUT_SCHEMA,
        types::DELETE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DELETE_RULE_GROUP_INPUT_SCHEMA,
        types::DELETE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_FLOW_OPERATION_INPUT_SCHEMA,
        types::DESCRIBE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_RULE_INPUT_SCHEMA,
        types::DESCRIBE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_SUMMARY_INPUT_SCHEMA,
        types::DESCRIBE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DETACH_RULE_GROUPS_FROM_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DISASSOCIATE_AVAILABILITY_ZONES_INPUT_SCHEMA,
        types::DISASSOCIATE_SUBNETS_INPUT_SCHEMA,
        types::GET_ANALYSIS_REPORT_RESULTS_INPUT_SCHEMA,
        types::LIST_ANALYSIS_REPORTS_INPUT_SCHEMA,
        types::LIST_CONTAINER_ASSOCIATIONS_INPUT_SCHEMA,
        types::LIST_FIREWALLS_INPUT_SCHEMA,
        types::LIST_FIREWALL_POLICIES_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATIONS_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATION_RESULTS_INPUT_SCHEMA,
        types::LIST_PROXIES_INPUT_SCHEMA,
        types::LIST_PROXY_CONFIGURATIONS_INPUT_SCHEMA,
        types::LIST_PROXY_RULE_GROUPS_INPUT_SCHEMA,
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
        types::UPDATE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_PROXY_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_GROUP_PRIORITIES_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_PRIORITIES_INPUT_SCHEMA,
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
/// with. Everything else takes the shared rendering, whose error code travels with the
/// message and names the one every operation of this service declares for a refused input.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if let Some(refused) = malformed_reference(violations) {
        return Some(refused);
    }
    // Rule 3 is applied to the violations BEFORE the clauses are rendered, so the clause
    // itself, its member path and the words around the value set stay the framework's.
    let stated: Vec<Violation> = violations.iter().map(in_service_value_set).collect();
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

/// The error shape rule 0 answers with: the client error every operation of this service
/// declares for a request whose reference is bad, which is the shape its handlers return for
/// the same defect (`smithy_query(query="shape",shape="InvalidRequestException")`).
const REFERENCE_REFUSAL_CODE: &str = "InvalidRequestException";

/// Rule 0: the malformed-reference refusal when EVERY violation is a reference member whose
/// value cannot be a resource ARN or a resource name, and `None` otherwise.
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
        } else if !names_a_reference(path) {
            return None;
        }
    }
    let message = if arn {
        crate::state::ARN_INVALID_FORMAT_MESSAGE
    } else {
        crate::state::NAME_INVALID_FORMAT_MESSAGE
    };
    Some(Refused {
        code: REFERENCE_REFUSAL_CODE.to_owned(),
        message: message.to_owned(),
    })
}

/// Whether the member at `path` carries a resource NAME that is a reference on the operation
/// being served: one of [`REFERENCE_NAME_MEMBERS`], on an operation acting on a resource that
/// already exists rather than on the create that names a new one.
fn names_a_reference(path: &str) -> bool {
    REFERENCE_NAME_MEMBERS.contains(&path)
        && crate::service::served_operation::serves_an_existing_resource()
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
                &["STATELESS", "STATEFUL", "STATEFUL_DOMAIN"],
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
}
