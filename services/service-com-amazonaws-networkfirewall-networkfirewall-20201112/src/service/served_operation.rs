// SPDX-License-Identifier: CC0-1.0

//! WHICH of this service's operations is serving the request, for the one refusal rule that is
//! a fact about the OPERATION rather than about the constraint.
//!
//! The framework refuses a request that breaks a constraint the model declares BEFORE any
//! handler runs, and asks the service for the words through `Service::override_refusal`. That
//! seam is handed the violations ALONE: one member path, one constraint class and the value
//! that sat there, with no request and no operation. For most of this service's refusal
//! wording that is enough, because the rules are properties of the constraint. One is not:
//! whether a RESOURCE-NAME member carries a reference to a resource that already exists, or
//! the name of the one the request is creating, is a fact about the operation — the same member
//! name is both, on different operations ([`super::validation`]'s rule 0).
//!
//! None of the plumbing that closes that gap is this service's: `service_common::restated_refusal`
//! is the fleet's one home for it, and [`SERVED_OPERATION_LAYER`] is its wrapper, handed back
//! from `Service::override_operation`. It serves the modelled operation the generated routing
//! table names — so the framework keeps the whole deserialize → validate → handle → serialize
//! walk and every status, code and word it answers with — with the served operation in scope
//! for the run, and then hands what it rendered to this service's own restatement.
//!
//! This file states only the two facts that are THIS service's: which operations name a NEW
//! resource in their resource-name member ([`CREATE_OPERATIONS`]), and that the response it
//! answers with reports a modelled error's text in the member this model declares
//! ([`super::error_message_member`]).

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::restated_refusal::{served_operation, RestatedRefusalOperation};

/// The operations whose resource-NAME member names the resource being CREATED, rather than
/// one that already exists.
///
/// The model is what says so, and says it the same way for each: on a create the resource's
/// name member is `@required` — `CreateFirewall$FirewallName`,
/// `CreateFirewallPolicy$FirewallPolicyName`, `CreateRuleGroup$RuleGroupName` and
/// `CreateTLSInspectionConfiguration$TLSInspectionConfigurationName`
/// (`smithy_query(query="operation",
/// operation="CreateFirewall,CreateFirewallPolicy,CreateRuleGroup,CreateTLSInspectionConfiguration")`)
/// — while every operation that acts on a resource that already exists takes the name beside
/// its ARN as an OPTIONAL pair it resolves itself (`DescribeFirewall$FirewallName` and
/// `$FirewallArn`, and so on for each resource). A required name is content the request
/// supplies; an optional one is half of a reference.
///
/// Sorted, which [`names_a_new_resource`] relies on for `binary_search` and
/// `the_create_operations_are_sorted` pins.
const CREATE_OPERATIONS: &[&str] = &[
    "CreateFirewall",
    "CreateFirewallPolicy",
    "CreateRuleGroup",
    "CreateTLSInspectionConfiguration",
];

/// Whether `operation` names a resource it is CREATING in its resource-name member.
fn names_a_new_resource(operation: &str) -> bool {
    CREATE_OPERATIONS.binary_search(&operation).is_ok()
}

/// Whether the operation being served acts on a resource that ALREADY EXISTS, so a
/// resource-name member of its input carries a reference rather than a new resource's name.
///
/// `false` outside any served operation — a refusal the router raised before an operation was
/// reached — where no operation's wording applies and the framework's own stands.
pub(crate) fn serves_an_existing_resource() -> bool {
    served_operation().is_some_and(|served| !names_a_new_resource(&served))
}

/// `resp` as this service answers it, for a request the wrapper below served: a modelled error
/// body whose text is in the member this model declares it in.
///
/// The words the refusal seam chose are already in the body it rendered, so nothing is noted
/// and nothing is restated here — only the member the text travels in is this service's to
/// state.
fn with_this_services_envelope(
    _req: &RtRequest,
    resp: RtResponse,
    _noted: Option<String>,
) -> RtResponse {
    super::error_message_member::with_modelled_message_member(resp)
}

/// The layer every modelled operation of this service is served through: the fleet's wrapper,
/// which puts the served operation in scope for the whole run (read back by
/// [`serves_an_existing_resource`]) and answers what the run rendered through
/// [`with_this_services_envelope`].
///
/// A `static` so the reference `Service::override_operation` hands back outlives the call, and
/// one value for the whole process because the wrapper holds nothing per service but that one
/// function.
pub(crate) static SERVED_OPERATION_LAYER: RestatedRefusalOperation =
    RestatedRefusalOperation::new(with_this_services_envelope);

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a resource-name member names a NEW resource on a create and a reference
    /// everywhere else.
    ///
    /// The rows span the four creates and, for the other side, a read, a delete, an update
    /// and an associate of each resource kind — plus an operation that names no resource at
    /// all.
    #[test]
    fn only_a_create_names_a_new_resource() {
        // (the operation, whether its resource-name member names a new resource)
        let rows: &[(&str, bool)] = &[
            ("CreateFirewall", true),
            ("CreateFirewallPolicy", true),
            ("CreateRuleGroup", true),
            ("CreateTLSInspectionConfiguration", true),
            ("DescribeFirewall", false),
            ("DeleteFirewall", false),
            ("UpdateFirewallDescription", false),
            ("AssociateFirewallPolicy", false),
            ("DescribeFirewallPolicy", false),
            ("UpdateFirewallPolicy", false),
            ("DescribeRuleGroup", false),
            ("DeleteRuleGroup", false),
            ("DescribeTLSInspectionConfiguration", false),
            ("ListFirewalls", false),
            ("CreateVpcEndpointAssociation", false),
        ];

        for (operation, creates) in rows {
            assert_eq!(
                names_a_new_resource(operation),
                *creates,
                "case {operation:?}: expected names_a_new_resource = {creates}"
            );
        }
    }

    /// [`CREATE_OPERATIONS`] is searched by `binary_search`, which a list out of order
    /// answers wrongly for.
    #[test]
    fn the_create_operations_are_sorted() {
        let mut sorted = CREATE_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            CREATE_OPERATIONS, &sorted[..],
            "expected the operations sorted, got {CREATE_OPERATIONS:?}"
        );
    }

    /// Outside a served operation nothing of an operation's wording applies, so the
    /// reference reading of a resource-name member is off.
    #[tokio::test]
    async fn no_served_operation_reads_as_no_existing_resource() {
        assert!(
            !serves_an_existing_resource(),
            "expected no existing resource outside a served operation"
        );
    }
}
