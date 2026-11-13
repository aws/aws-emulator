// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the boundary refusal of a request whose input breaks a
//! constraint the model declares.
//!
//! [`crate::service`] owns the seams — the per-operation wrapper that has the REQUEST, and
//! `Service::override_refusal` for a refusal that reaches the centralized walk anyway — and
//! this module owns the wording, so the two paths cannot word one constraint differently.
//! WHAT is enforced is always the model's: every violation comes from the framework's own
//! walk, which reads the `@length`, `@range`, `@pattern`, `@enum` and `@uniqueItems` traits
//! codegen captured off this service's model. The envelope, the member paths and every
//! requirement sentence are the fleet's one home for them
//! ([`::service_common::validation_message`]), so a clause this service states cannot drift
//! from another service's for the same constraint.
//!
//! Three things about such a report are this API's own:
//!
//! * a violation on an ELEMENT of a collection member is stated against the COLLECTION
//!   MEMBER — no element position and no `member` step in the path — with the ELEMENT
//!   SHAPE'S WHOLE constraint set nested inside `Member must satisfy constraint: [ … ]`,
//!   and the collection itself quoted as the value. The set is the element shape's, not
//!   just the constraint that happened to fail: the clause tells a caller what every
//!   element of that member has to satisfy, so one request that breaks two of an element's
//!   constraints is ONE validation error about one member rather than one error per
//!   constraint at an indexed path. It is read off the MODEL — the input schemas
//!   [`input_schemas`] hands over — because the refusal carries only the constraints that
//!   FAILED. A violation on the collection member ITSELF (its own `@length`, its
//!   `@uniqueItems`) is not nested, and neither is one on a member INSIDE a structure
//!   element, which keeps its own full path;
//! * the value such a clause quotes, which is the collection the REQUEST carried at that
//!   member — read from the request by the fleet's own collecting walk
//!   (`::service_common::validation::submitted_lists`, reached through the per-operation
//!   seam) and never rebuilt from the violations, which carry only the elements that
//!   offended. A list that walk offers no rendering for leaves the framework's own echo in
//!   place rather than a value the caller never sent, and so does a refusal reaching the
//!   request-less seam;
//! * a `@pattern` requirement quotes the expression the service PUBLISHES for the member's
//!   shape ([`crate::state::quoted_pattern`], the one home for that choice, which the
//!   handler-stated clauses reach too).
//!
//! Everything else keeps the framework's own clause, verbatim.

use std::collections::{BTreeSet, HashMap};

use ::aws_smithy_schema::Schema;
use ::service_common::validation::list_of_element;
use ::smithy_server_runtime::{Refusal, Refused, Violation};

/// The members of `violations` whose clauses state NO submitted value: the ones the request
/// broke SEVERAL constraints of.
///
/// This front door echoes the value a member submitted while it has ONE thing to say about
/// that member, and states the member and its requirements alone once it has several: what
/// distinguishes the clauses of such a report is the requirement each names, and the value
/// is the same in all of them. A member with one violation keeps the echo, whatever it
/// carried and wherever it was bound — a URI label, a query parameter, a body member nested
/// in a structure.
fn members_stated_without_their_value(violations: &[Violation]) -> BTreeSet<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut several: BTreeSet<String> = BTreeSet::new();
    for violation in violations {
        if !seen.insert(violation.path.as_str()) {
            several.insert(violation.path.clone());
        }
    }
    several
}

/// This service's wording for the violations the framework's walk found, or `None` when
/// there is nothing to word.
///
/// `lists` yields every list of scalar elements the REQUEST carried, keyed by the member
/// path the walk reports that list at and already rendered
/// (`::service_common::validation::submitted_lists`) — the value a clause stated at a
/// collection member needs and no violation carries. It is a closure because it is only
/// read for a violation found on a collection ELEMENT, so a refusal that has none never
/// walks the request again, and the request-less seam passes one that yields nothing.
pub(super) fn refusal(
    violations: &[Violation],
    lists: impl FnOnce() -> HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let lists = if violations
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        lists()
    } else {
        HashMap::new()
    };
    let without_value = members_stated_without_their_value(violations);
    ::service_common::validation_message::aws_validation_refusal_over_elements_quoting_withholding(
        &in_hashed_value_set_order(violations),
        &input_schemas(),
        crate::state::quoted_pattern,
        |collection| lists.get(collection).cloned(),
        |path| without_value.contains(path),
    )
}

/// `violations` with every enumerated value set they report listed in the order this front
/// door states one.
///
/// The ORDER is the order a hash table over the admitted values is walked
/// ([`::service_common::validation_message::in_hashed_enum_order`], the fleet's one home for
/// it), which is the order a recorded refusal of this surface lists a set in — the model
/// declares `EngineType` `[OpenSearch, Elasticsearch]`
/// (`smithy_query(query="shape",shape="EngineType")`) and codegen hands the walk the values
/// sorted, so neither of those orders is what a clause states. MEMBERSHIP is untouched, and
/// so is every other part of the violation: only the order inside the bracketed set changes.
fn in_hashed_value_set_order(violations: &[Violation]) -> Refusal {
    Refusal::Invalid(
        violations
            .iter()
            .map(::service_common::validation_message::in_hashed_enum_order)
            .collect::<Vec<Violation>>(),
    )
}

/// Every INPUT shape of this service, as the generated schema statics.
///
/// All of them, not the ones one report happened to cite: an element constraint is a
/// property of an input shape, so leaving an operation out would silently word its
/// collection clauses with less than the model declares. Regenerate with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`, which names exactly these statics; an input
/// shape missing from this list is not a wrong clause, only a clause that states the
/// constraints that failed rather than the element shape's whole set.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::ACCEPT_INBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_INPUT_SCHEMA,
        t::ADD_TAGS_INPUT_SCHEMA,
        t::ASSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::AUTHORIZE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::CANCEL_DOMAIN_CONFIG_CHANGE_INPUT_SCHEMA,
        t::CANCEL_ELASTICSEARCH_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::CREATE_ELASTICSEARCH_DOMAIN_INPUT_SCHEMA,
        t::CREATE_OUTBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_INPUT_SCHEMA,
        t::CREATE_PACKAGE_INPUT_SCHEMA,
        t::CREATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DELETE_ELASTICSEARCH_DOMAIN_INPUT_SCHEMA,
        t::DELETE_INBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_INPUT_SCHEMA,
        t::DELETE_OUTBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_INPUT_SCHEMA,
        t::DELETE_PACKAGE_INPUT_SCHEMA,
        t::DELETE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_AUTO_TUNES_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_CHANGE_PROGRESS_INPUT_SCHEMA,
        t::DESCRIBE_ELASTICSEARCH_DOMAINS_INPUT_SCHEMA,
        t::DESCRIBE_ELASTICSEARCH_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::DESCRIBE_ELASTICSEARCH_DOMAIN_INPUT_SCHEMA,
        t::DESCRIBE_ELASTICSEARCH_INSTANCE_TYPE_LIMITS_INPUT_SCHEMA,
        t::DESCRIBE_INBOUND_CROSS_CLUSTER_SEARCH_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_OUTBOUND_CROSS_CLUSTER_SEARCH_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_PACKAGES_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_ELASTICSEARCH_INSTANCES_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_ELASTICSEARCH_INSTANCE_OFFERINGS_INPUT_SCHEMA,
        t::DESCRIBE_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::DISSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::GET_COMPATIBLE_ELASTICSEARCH_VERSIONS_INPUT_SCHEMA,
        t::GET_PACKAGE_VERSION_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_STATUS_INPUT_SCHEMA,
        t::LIST_DOMAINS_FOR_PACKAGE_INPUT_SCHEMA,
        t::LIST_DOMAIN_NAMES_INPUT_SCHEMA,
        t::LIST_ELASTICSEARCH_INSTANCE_TYPES_INPUT_SCHEMA,
        t::LIST_ELASTICSEARCH_VERSIONS_INPUT_SCHEMA,
        t::LIST_PACKAGES_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_TAGS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::PURCHASE_RESERVED_ELASTICSEARCH_INSTANCE_OFFERING_INPUT_SCHEMA,
        t::REJECT_INBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_INPUT_SCHEMA,
        t::REMOVE_TAGS_INPUT_SCHEMA,
        t::REVOKE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::START_ELASTICSEARCH_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::UPDATE_ELASTICSEARCH_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::UPDATE_PACKAGE_INPUT_SCHEMA,
        t::UPDATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::UPGRADE_ELASTICSEARCH_DOMAIN_INPUT_SCHEMA,
    ]
}
