// SPDX-License-Identifier: CC0-1.0

//! The role a VPC interface is created UNDER: a fact this service resolves rather than
//! holds.
//!
//! A flow's VPC interface is an elastic network interface MediaConnect creates in the
//! CALLER's account, and `VpcInterfaceRequest$RoleArn` is "the ARN of the role that you
//! created when you set up MediaConnect as a trusted service" — the stored
//! `VpcInterface$RoleArn` says it plainly: "a role Arn MediaConnect can assume to create
//! ENIs in your account". So a request that adds a VPC interface to a flow is a request for
//! this service to ACT UNDER that role, and the service takes the role on before it creates
//! anything: a role it cannot assume is a request that could never be carried out, so the
//! whole request is refused with the modelled `BadRequestException` (HTTP 400) and the flow
//! is left exactly as it was.
//!
//! The question is the tree's shared one ([`::service_common::iam_role::role_is_unassumable`]):
//! this account's IAM holds no role under the name the ARN carries, or the trust policy the
//! role it does hold carries does not admit this service's principal. Every refusal built
//! here is therefore grounded in what IAM ANSWERED — a value naming another account's role,
//! an answer this emulator could not read, and an emulator with no IAM mounted at all
//! establish nothing, and nothing is what a request is refused on then.
//!
//! NOTHING here reaches the network: IAM is asked over the framework's in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which hands the request to the
//! IAM service registered in this same emulator, so no socket is opened and no address from
//! the request is contacted. The Region label only selects which of this emulator's own
//! state answers.

/// The service principal MediaConnect assumes a caller-supplied role as — the entity a
/// role's trust policy has to admit for this service to create network interfaces under it.
///
/// It is the trusted entity the refusal below tells a caller to add, and the principal the
/// shared trust-policy reading is asked about, so the one value serves both.
pub const SERVICE_PRINCIPAL: &str = "mediaconnect.amazonaws.com";

/// MediaConnect's `BadRequestException` message for a request naming a role it cannot assume:
/// the semantic-refusal envelope ([`super::bad_request_message`]), the assume that failed,
/// and what the caller has to change.
///
/// `role_arn` is the value the REQUEST carried, echoed as the caller sent it, because that
/// is the role the caller has to go and fix; everything around it is fixed text for this
/// error class, with the trusted entity named from [`SERVICE_PRINCIPAL`].
#[must_use]
pub fn unassumable_role_message(role_arn: &str) -> String {
    super::bad_request_message(&format!(
        "Unable to assume role: {role_arn} Add {SERVICE_PRINCIPAL} as a trusted entity and \
         try again."
    ))
}

/// Why the VPC interfaces `requests` cannot be created — the whole
/// `BadRequestException$Message` the request is refused with — or `None` when every entry
/// names a role this service may act under.
///
/// Both CreateFlow (`VpcInterfaces`) and AddFlowVpcInterfaces add VPC interfaces to a flow,
/// so both resolve every entry's role HERE, in request order, and neither can store a VPC
/// interface the other would refuse. The FIRST entry whose role cannot be assumed refuses the
/// whole request, so a batch with one such entry adds none of its entries.
///
/// The list is CALLER-SIZED and this asks IAM once per entry, so the fan-out is bounded: each
/// operation refuses a request naming more entries than it may store before it calls here
/// (CreateFlow at [`super::MAX_FLOW_COLLECTION_ENTRIES`], AddFlowVpcInterfaces at
/// [`super::MAX_FLOW_VPC_INTERFACES`]), and the same ceiling the shared placement resolution
/// applies ([`super::MAX_RESOLVED_PLACEMENTS`]) bounds the walk here too, so no caller of it
/// can drive an unbounded number of reads.
///
/// `region` selects which of this emulator's own state answers, and is read as a Region
/// LABEL (`service_common::arn::region_label`) because it reaches the address the SDK names
/// the peer by: a value outside that grammar names no Region of this account, so there is
/// nothing to ask and nothing to refuse on.
pub async fn unassumable_vpc_interface_role(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    requests: &[crate::ops::types::VpcInterfaceRequest],
) -> Option<String> {
    let region = ::service_common::arn::region_label(region)?;
    for request in requests.iter().take(super::MAX_RESOLVED_PLACEMENTS) {
        if ::service_common::iam_role::role_is_unassumable(
            ctx,
            region,
            &request.role_arn,
            SERVICE_PRINCIPAL,
        )
        .await
        {
            return Some(unassumable_role_message(&request.role_arn));
        }
    }
    None
}
