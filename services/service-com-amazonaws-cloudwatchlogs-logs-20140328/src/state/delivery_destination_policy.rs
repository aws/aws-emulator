// SPDX-License-Identifier: CC0-1.0

//! What makes a document a DELIVERY DESTINATION POLICY — the cross-account grant
//! `PutDeliveryDestinationPolicy` assigns to one of this account's delivery destinations — and
//! the form that document is stored and reported in.

use super::*;

/// Whether `document` is a delivery destination resource policy.
///
/// The operation "creates and assigns an IAM policy that grants permissions to CloudWatch Logs
/// to deliver logs cross-account", so the document is first an IAM JSON policy in its
/// RESOURCE-based form — it states who the permission is granted to — which is the one grammar
/// [`service_common::iam_policy`] states for every service of this emulator that takes such a
/// document.
///
/// Beyond that grammar the document has to be THIS grant rather than some other resource's
/// policy, which is what the operation's own documentation states it is: a policy whose
/// "contents must include two statements. One statement enables general logs delivery, and the
/// other allows delivery to the chosen destination", assigned "in the destination account" so
/// that another account may "deliver logs cross-account to a specified destination in this
/// account". So each of its statements is a grant of log delivery:
///
/// * it names the RESOURCE it grants access to, and every resource it names is one of the
///   vended-log delivery resources ([`names_a_delivery_resource`]) — the destination being
///   assigned to, its siblings, the delivery sources paired with them, or the deliveries
///   themselves. A statement about something else (a log group, a bucket, a stream) belongs to
///   THAT resource's policy, where it would be effective, rather than to a delivery
///   destination, where it grants nothing.
/// * every ACTION it names is one of the vended-log delivery actions
///   ([`names_a_delivery_action`]): those are the operations that act on those resources, so an
///   action outside them can never be exercised against the destination.
/// * it grants them to an ACCOUNT ([`grants_to_an_account`]), because what this policy enables
///   is another account's calls against this account's destination.
///
/// What a statement SAYS beyond that is not judged: which of the delivery actions a grant
/// covers, which account it names and what it conditions them on govern a cross-account
/// delivery nothing in this store performs.
pub fn is_delivery_destination_policy(document: &str) -> bool {
    if ::service_common::iam_policy::resource_policy_defect(document).is_some() {
        return false;
    }
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    let statements = policy_statements(&parsed);
    !statements.is_empty() && statements.iter().all(grants_log_delivery)
}

/// Whether one statement is a grant of log delivery: the three clauses above, read off the
/// statement's own elements.
fn grants_log_delivery(statement: &::serde_json::Value) -> bool {
    let resources = statement_element_values(statement, RESOURCE_ELEMENTS);
    let actions = statement_element_values(statement, ACTION_ELEMENTS);
    !resources.is_empty()
        && resources
            .iter()
            .map(String::as_str)
            .all(names_a_delivery_resource)
        && actions.iter().map(String::as_str).all(names_a_delivery_action)
        && grants_to_an_account(statement)
}

/// The resource types of this service a delivery destination policy grants access to, as the
/// ARN of each spells its type ([`delivery_source_arn`], [`delivery_destination_arn`],
/// [`delivery_arn`]).
pub const DELIVERY_POLICY_RESOURCE_TYPES: [&str; 3] =
    ["delivery-source", "delivery-destination", "delivery"];

/// Whether a `Resource` value names a resource of the vended-log delivery surface: the
/// wildcard that names every resource there is, or an ARN of this service naming a delivery
/// source, a delivery destination or a delivery — whose resource id the value may wildcard,
/// because a grant that "enables general logs delivery" covers every resource of a type rather
/// than one.
pub fn names_a_delivery_resource(value: &str) -> bool {
    if value == ::service_common::iam_policy::ANY_RESOURCE {
        return true;
    }
    arn_service_and_resource(value).is_some_and(|(service, resource)| {
        service == CLOUDWATCH_LOGS_VENDOR
            && DELIVERY_POLICY_RESOURCE_TYPES
                .iter()
                // The separator is part of the comparison: `delivery` is a prefix of
                // `delivery-source`, so a type matches only where the resource id follows it.
                .any(|resource_type| {
                    resource
                        .strip_prefix(resource_type)
                        .is_some_and(|rest| rest.starts_with(':'))
                })
    })
}

/// The operations of this service that act on a vended-log delivery resource — a delivery, a
/// delivery source or a delivery destination — and so the actions a delivery destination
/// policy can grant. Each is named as the operation is, which is how an IAM action of a service
/// is spelled.
///
/// Source: the operations of this service whose input or output is one of those resources
/// (`smithy_query(query="list_operations")`), which is also the set this crate implements one
/// file each for.
pub const DELIVERY_POLICY_ACTIONS: [&str; 17] = [
    "CreateDelivery",
    "DeleteDelivery",
    "DeleteDeliveryDestination",
    "DeleteDeliveryDestinationPolicy",
    "DeleteDeliverySource",
    "DescribeConfigurationTemplates",
    "DescribeDeliveries",
    "DescribeDeliveryDestinations",
    "DescribeDeliverySources",
    "GetDelivery",
    "GetDeliveryDestination",
    "GetDeliveryDestinationPolicy",
    "GetDeliverySource",
    "PutDeliveryDestination",
    "PutDeliveryDestinationPolicy",
    "PutDeliverySource",
    "UpdateDeliveryConfiguration",
];

/// Whether an `Action` value names an action a delivery destination policy can grant: the
/// wildcard that names every action there is, or an action of THIS service
/// ([`DELIVERY_POLICY_ACTIONS`]) — written out, or as a pattern the IAM policy language admits
/// there, which covers one of them.
pub fn names_a_delivery_action(value: &str) -> bool {
    if value == ::service_common::iam_policy::ANY_ACTION {
        return true;
    }
    // An action is `<service>:<action>`; another service's actions cannot be exercised against
    // a resource of this one, whatever they are.
    let Some(action) = value.strip_prefix(&format!("{CLOUDWATCH_LOGS_VENDOR}:")) else {
        return false;
    };
    // The document's value is the PATTERN, read with the policy language's own wildcards and
    // compared without regard to case, as IAM compares an action name.
    DELIVERY_POLICY_ACTIONS.iter().any(|operation| {
        ::service_common::iam_authorization::wildcard_match(action, operation, false)
    })
}

/// Whether a statement grants its actions to an ACCOUNT: the key an IAM policy names an
/// account principal under.
///
/// A policy that shares one of this account's resources with ANOTHER ACCOUNT — a delivery
/// destination policy, which lets that account create a delivery to this account's
/// destination, and a logical destination's access policy, which lets it register a
/// subscription filter against this account's destination — grants to an account: named by its
/// id, by its root ARN or by the ARN of one of its roles or users, or the wildcard that names
/// every principal. A grant to a SERVICE principal alone is a service's permission on a
/// resource, which is not what either policy does.
pub fn grants_to_an_account(statement: &::serde_json::Value) -> bool {
    PRINCIPAL_ELEMENTS
        .into_iter()
        .filter_map(|element| statement.get(element))
        .any(|principal| match principal {
            ::serde_json::Value::Object(members) => members
                .get(ACCOUNT_PRINCIPAL)
                .is_some_and(|named| !policy_element_values(named).is_empty()),
            named => !policy_element_values(named).is_empty(),
        })
}

/// What the service STORES for a delivery destination policy, and so what every later read of
/// it reports: the document in its CANONICAL form.
///
/// A service whose API takes an IAM policy as a string does not keep the caller's bytes: it
/// reads the document and renders it again, so two clients that submit the same policy written
/// differently read the same text back. The rendering is the tree's one rendering of a policy
/// document ([`service_common::iam_policy_rendering`] — no whitespace between the tokens, and
/// an element the policy language types as "String or Array of strings" written as that string
/// where the caller submitted a one-element array), walked with the one spelling this service's
/// reads add: every ACCOUNT principal named by the account's id alone written as that account's
/// ROOT ARN ([`account_root_arn`]), the other spelling of the same principal.
///
/// Member order and every string literal survive, so what the policy SAYS stays the caller's.
/// The rendering is a fixed point: re-rendering a rendered document returns it unchanged, so a
/// client that puts back what it read sees no drift.
pub fn canonical_delivery_destination_policy(document: &str) -> String {
    ::service_common::iam_policy_rendering::rendered_rewriting(
        document,
        &::service_common::iam_policy_rendering::ElementValues {
            element: ACCOUNT_PRINCIPAL,
            written_as: &account_root_arn,
        },
    )
}

/// The root ARN of the account `value` names by its id alone, or `None` for a value that is
/// not an account id — an ARN, a wildcard, anything else, which the read reports as it was
/// written.
///
/// An account id is [`ACCOUNT_ID_DIGITS`] digits, which is the one form the account's own
/// identifier takes. The PARTITION is the one every ARN of this emulator carries, read off an
/// ARN it builds rather than written here, so an account is named in the same partition as the
/// resources the policy governs.
fn account_root_arn(value: &str) -> Option<String> {
    if value.len() != ACCOUNT_ID_DIGITS || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let partition = ::service_common::arn::parse(&delivery_destination_arn("", ""))
        .map(|arn| arn.partition.to_string())?;
    Some(format!("arn:{partition}:iam::{value}:root"))
}

/// How many digits an Amazon Web Services account id has.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The key an IAM policy names an ACCOUNT principal under — "the account, user, role, or
/// federated or anonymous user" a resource-based policy grants to, as opposed to the `Service`,
/// `Federated` and `CanonicalUser` keys that name the other kinds.
const ACCOUNT_PRINCIPAL: &str = "AWS";

/// The policy elements that name the principal a statement grants to, the resources it covers
/// and the actions it allows, each with its complement — the IAM policy language states each
/// either way round, and a statement is read the same whichever way it is written.
const PRINCIPAL_ELEMENTS: [&str; 2] = ["Principal", "NotPrincipal"];
const RESOURCE_ELEMENTS: [&str; 2] = ["Resource", "NotResource"];
const ACTION_ELEMENTS: [&str; 2] = ["Action", "NotAction"];

/// The statements a policy document declares: the entries of its `Statement` list, or the one
/// statement written as the object itself rather than as a list — the two spellings the IAM
/// policy language admits.
///
/// `pub(super)` so the OTHER surface of this service that reads a submitted policy document —
/// the destination access policy ([`super::is_access_policy`]) — walks the same statements
/// this one does rather than carrying a second reading of that element.
pub(super) fn policy_statements(document: &::serde_json::Value) -> Vec<::serde_json::Value> {
    match document.get("Statement") {
        Some(::serde_json::Value::Array(entries)) => entries.clone(),
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
        _ => Vec::new(),
    }
}

/// Every value one statement names in `elements`, in the order they are written.
fn statement_element_values(statement: &::serde_json::Value, elements: [&str; 2]) -> Vec<String> {
    elements
        .into_iter()
        .filter_map(|element| statement.get(element))
        .flat_map(policy_element_values)
        .collect()
}

/// One policy element's values: an element is written as one string or as a list of them, and
/// anything else names no value.
fn policy_element_values(element: &::serde_json::Value) -> Vec<String> {
    match element {
        ::serde_json::Value::String(value) => vec![value.clone()],
        ::serde_json::Value::Array(entries) => {
            entries.iter().flat_map(policy_element_values).collect()
        }
        _ => Vec::new(),
    }
}
