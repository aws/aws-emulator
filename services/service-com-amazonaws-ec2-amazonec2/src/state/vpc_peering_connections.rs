// SPDX-License-Identifier: CC0-1.0

//! The VPC peering connection store: what one peering is stored as, the state the clock
//! owes a fresh request, when an unaccepted request expires, the store's bounds and the
//! error channel the peering operations share.
//!
//! A peering connection is a resource BETWEEN two VPCs — "Requests a VPC peering
//! connection between two VPCs: a requester VPC that you own and an accepter VPC with
//! which to create the connection" (`CreateVpcPeeringConnection`) — so the record holds
//! the facts OF the peering (which two VPCs, whose accounts, which Regions, its status and
//! its tags) and nothing that belongs to a VPC. The address space of each side is read off
//! the VPC the store holds, at the moment it is reported
//! ([`ComAmazonawsEc2Amazonec2State::vpc_peering_connection_view`]), rather than copied
//! onto this record: a VPC that takes another CIDR block afterwards cannot then be
//! described two ways.
//!
//! What the peering operations share lives here — the id mint, the status words, the
//! `initiating-request` -> `pending-acceptance` move the clock owes a fresh request, the
//! expiry of a request nobody answered, the `rejected` state a refusal records
//! ([`ComAmazonawsEc2Amazonec2State::reject_vpc_peering_connection`]), the
//! `deleting` -> `deleted` move a deletion is owed
//! ([`ComAmazonawsEc2Amazonec2State::delete_vpc_peering_connection`]), the ceilings and the
//! refusals — so no two of them can report one connection differently.

use std::time::{Duration, Instant, SystemTime};

use super::{
    ComAmazonawsEc2Amazonec2State, PendingStateMove, cidr_association_holds_its_block,
    ipv4_cidr_blocks_overlap, system_time_to_date_time, tag_list_bytes,
};
use crate::ops::types::{
    CidrBlock, Ipv6CidrBlock, Tag, Vpc, VpcPeeringConnection, VpcPeeringConnectionStateReason,
    VpcPeeringConnectionVpcInfo,
};
use crate::scaffolded::Scaffolded;

/// The id prefix every VPC peering connection id carries (`pcx-…`, the form the EC2
/// console and CLI report a peering connection under), minted through
/// [`ComAmazonawsEc2Amazonec2State::next_id`] like every other EC2 resource id.
pub const VPC_PEERING_CONNECTION_ID_PREFIX: &str = "pcx";

/// A peering request EC2 has only just taken: the `VpcPeeringConnectionStateReasonCode`
/// enum's `initiating-request` member
/// (`smithy_query(query="shape",shape="VpcPeeringConnectionStateReasonCode")`), which is
/// what `CreateVpcPeeringConnection` answers with until [`super::TRANSITION_DELAY`] has
/// passed.
pub const VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST: &str = "initiating-request";

/// A peering request waiting for the owner of the accepter VPC: the same enum's
/// `pending-acceptance` member — "The owner of the accepter VPC must accept the peering
/// request to activate the peering connection" (`CreateVpcPeeringConnection`), and the
/// state the clock owes a request answered [`VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST`].
pub const VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE: &str = "pending-acceptance";

/// A peering connection being set up after an acceptance: the same enum's `provisioning`
/// member.
pub const VPC_PEERING_CONNECTION_STATE_PROVISIONING: &str = "provisioning";

/// A peering connection that carries traffic: the same enum's `active` member — the one
/// state in which BOTH sides' address space is reported ("CIDR block information is only
/// returned when describing an active VPC peering connection",
/// `VpcPeeringConnection$AccepterVpcInfo`).
pub const VPC_PEERING_CONNECTION_STATE_ACTIVE: &str = "active";

/// A peering request that could not be set up: the same enum's `failed` member — "If you
/// create a VPC peering connection request between VPCs with overlapping CIDR blocks, the
/// VPC peering connection has a status of failed" (`CreateVpcPeeringConnection`).
pub const VPC_PEERING_CONNECTION_STATE_FAILED: &str = "failed";

/// A peering request nobody answered in time: the same enum's `expired` member — "The VPC
/// peering connection request expires after 7 days, after which it cannot be accepted or
/// rejected" (`CreateVpcPeeringConnection`).
pub const VPC_PEERING_CONNECTION_STATE_EXPIRED: &str = "expired";

/// A peering request the owner of the accepter VPC refused: the same enum's `rejected`
/// member — "Rejects a VPC peering connection request" (`RejectVpcPeeringConnection`), the
/// state [`ComAmazonawsEc2Amazonec2State::reject_vpc_peering_connection`] writes.
pub const VPC_PEERING_CONNECTION_STATE_REJECTED: &str = "rejected";

/// A connection a delete has been asked for and is still on its way out: the same enum's
/// `deleting` member, the transient state
/// [`ComAmazonawsEc2Amazonec2State::delete_vpc_peering_connection`] writes.
pub const VPC_PEERING_CONNECTION_STATE_DELETING: &str = "deleting";

/// A connection whose deletion has finished: the same enum's `deleted` member, which the
/// clock owes a [`VPC_PEERING_CONNECTION_STATE_DELETING`] connection one
/// [`super::TRANSITION_DELAY`] later.
pub const VPC_PEERING_CONNECTION_STATE_DELETED: &str = "deleted";

/// How long an unaccepted peering request lives.
///
/// "The VPC peering connection request expires after 7 days, after which it cannot be
/// accepted or rejected" (`CreateVpcPeeringConnection`): a span the service documents, so
/// it keeps the value AWS gives it rather than the span this crate compresses a status move
/// into ([`super::TRANSITION_DELAY`]).
pub const VPC_PEERING_CONNECTION_REQUEST_LIFETIME: Duration =
    Duration::from_secs(7 * 24 * 60 * 60);

/// The `ResourceType` a `CreateVpcPeeringConnection` tag specification may name: the
/// peering connection it creates (the modelled `ResourceType` enum's
/// `vpc-peering-connection` member).
pub const RESOURCE_TYPE_VPC_PEERING_CONNECTION: &str = "vpc-peering-connection";

/// The largest number of VPC peering connections this in-memory store holds.
///
/// A capacity bound in the same style as [`super::MAX_VPCS`], not an AWS quota (AWS's
/// peering-connections-per-VPC quota is adjustable): each connection is a record in this
/// process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_vpc_peering_connection_capacity`] is the predicate
/// a writer asks first; a create past it refuses with
/// [`VPC_PEERING_CONNECTION_LIMIT_EXCEEDED`].
pub const MAX_VPC_PEERING_CONNECTIONS: usize = 1_000;

/// The error code a create refuses with once the store is full, in the form EC2 names every
/// quota refusal with (`VpcLimitExceeded`, `SubnetLimitExceeded`): `<Resource>LimitExceeded`.
/// The ceiling it reports is this store's ([`MAX_VPC_PEERING_CONNECTIONS`]), not a published
/// AWS quota.
pub const VPC_PEERING_CONNECTION_LIMIT_EXCEEDED: &str = "VpcPeeringConnectionLimitExceeded";

/// EC2's error code for a request that named a peering connection id this account holds no
/// connection for (EC2 API error codes, "InvalidVpcPeeringConnectionID.NotFound").
pub const INVALID_VPC_PEERING_CONNECTION_ID_NOT_FOUND: &str =
    "InvalidVpcPeeringConnectionID.NotFound";

/// EC2's error code for a value that is no peering connection id at all.
///
/// The `.Malformed` half of the pair above: EC2 answers it BEFORE it looks an id up, so a
/// caller can tell "not an id of this kind" from "no such resource"
/// ([`super::require_well_formed_id`], the one place that ordering lives). The same half for
/// a `VpcId` is [`super::INVALID_VPC_ID_MALFORMED`], which lives beside its own not-found
/// code.
pub const INVALID_VPC_PEERING_CONNECTION_ID_MALFORMED: &str =
    "InvalidVpcPeeringConnectionID.Malformed";

/// EC2's error code for a second request to peer two VPCs that are already peered (EC2 API
/// error codes, "VpcPeeringConnectionAlreadyExists").
pub const VPC_PEERING_CONNECTION_ALREADY_EXISTS: &str = "VpcPeeringConnectionAlreadyExists";

/// EC2's error code for a peering request the connection's own state does not admit (EC2 API
/// error codes, "InvalidStateTransition": the VPC peering connection is not in the correct
/// state for the request).
///
/// The peering family's own name for the class every other resource of this crate refuses
/// with [`super::INCORRECT_STATE`]: EC2 words the peering lifecycle's refusals with this code
/// instead.
pub const INVALID_STATE_TRANSITION: &str = "InvalidStateTransition";

/// The `VpcPeeringConnectionStateReason$Message` of a request EC2 has just taken, which
/// names the account the request goes TO — the owner of the accepter VPC
/// (`CreateVpcPeeringConnectionRequest$PeerOwnerId`), read off the request or defaulted to
/// the caller's own account by the create.
pub fn vpc_peering_initiating_request_message(accepter_owner_id: &str) -> String {
    format!("Initiating Request to {accepter_owner_id}")
}

/// The `VpcPeeringConnectionStateReason$Message` of a request that is waiting for the
/// accepter's owner, naming that same account: "The owner of the accepter VPC must accept
/// the peering request to activate the peering connection" (`CreateVpcPeeringConnection`).
pub fn vpc_peering_pending_acceptance_message(accepter_owner_id: &str) -> String {
    format!("Pending Acceptance by {accepter_owner_id}")
}

/// The `VpcPeeringConnectionStateReason$Message` of a request that could not be set up.
///
/// The sentence the Amazon VPC Peering Guide's troubleshooting of a failed peering gives
/// for one: the request named a VPC, an account or an address range that cannot be peered.
/// It carries no value of its own, so the one cause this emulator can establish — two
/// overlapping CIDR blocks — is reported in the same words AWS reports every such failure
/// in.
pub const VPC_PEERING_CONNECTION_FAILED_MESSAGE: &str =
    "Failed due to incorrect VPC-ID, Account ID, or overlapping CIDR range";

/// The `VpcPeeringConnectionStateReason$Message` of a connection being set up after its
/// owner accepted it: the state's own word, which is what EC2 reports while a peering is
/// [`VPC_PEERING_CONNECTION_STATE_PROVISIONING`]. It names no account, the request having
/// been answered.
pub const VPC_PEERING_CONNECTION_PROVISIONING_MESSAGE: &str = "Provisioning";

/// The `VpcPeeringConnectionStateReason$Message` of a connection that carries traffic: the
/// state's own word, which is what EC2 reports for a peering that reached
/// [`VPC_PEERING_CONNECTION_STATE_ACTIVE`].
pub const VPC_PEERING_CONNECTION_ACTIVE_MESSAGE: &str = "Active";

/// The `VpcPeeringConnectionStateReason$Message` of a request the accepter's owner refused:
/// the connection carries no traffic and never will, the request having been answered no. It
/// names no account, as the states a request reaches after it stops waiting do not.
pub const VPC_PEERING_CONNECTION_REJECTED_MESSAGE: &str = "Inactive";

/// The `VpcPeeringConnectionStateReason$Message` that goes with `code`, or `None` for a
/// state EC2 reports no message for — "A message that provides more information about the
/// status, IF APPLICABLE" (`VpcPeeringConnectionStateReason$Message`).
///
/// The ONE mapping from a peering's status to the sentence reported beside it, so the create
/// that mints a request, the clock that moves it on
/// ([`ComAmazonawsEc2Amazonec2State::materialize_due_vpc_peering_connections`]) and the
/// acceptance that answers it ([`ComAmazonawsEc2Amazonec2State::accept_vpc_peering_connection`])
/// cannot word one state two ways. The two states a request passes through while it waits
/// name the account it waits for; the states it reaches afterwards do not — except a finished
/// deletion, whose sentence names the account that deleted it and is therefore written by the
/// delete rather than mapped from the state ([`vpc_peering_deleted_message`]).
pub fn vpc_peering_connection_state_message(
    code: &str,
    accepter_owner_id: &str,
) -> Option<String> {
    match code {
        VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST => {
            Some(vpc_peering_initiating_request_message(accepter_owner_id))
        }
        VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE => {
            Some(vpc_peering_pending_acceptance_message(accepter_owner_id))
        }
        VPC_PEERING_CONNECTION_STATE_PROVISIONING => {
            Some(VPC_PEERING_CONNECTION_PROVISIONING_MESSAGE.to_string())
        }
        VPC_PEERING_CONNECTION_STATE_ACTIVE => {
            Some(VPC_PEERING_CONNECTION_ACTIVE_MESSAGE.to_string())
        }
        VPC_PEERING_CONNECTION_STATE_FAILED => {
            Some(VPC_PEERING_CONNECTION_FAILED_MESSAGE.to_string())
        }
        VPC_PEERING_CONNECTION_STATE_REJECTED => {
            Some(VPC_PEERING_CONNECTION_REJECTED_MESSAGE.to_string())
        }
        _ => None,
    }
}

/// Whether a connection in this state is still waiting for the accepter's owner, so the
/// clock may expire it and its owner may still answer it.
///
/// `initiating-request` and `pending-acceptance` are the two states a request passes
/// through before anybody answers it; every other state is one the request has been
/// answered in or has already left.
///
/// This is also the question an acceptance asks of the request it is given: a request that
/// is still waiting is what an acceptance RECORDS — "To accept a request, the VPC peering
/// connection must be in the pending-acceptance state" (`AcceptVpcPeeringConnection`) says
/// which requests are still open, and a request on its way in is on its way to exactly that
/// state, the real service leaving `initiating-request` inside the call that mints it, so an
/// acceptance issued straight after a create is served rather than refused. An acceptance of
/// a connection that is no longer waiting writes nothing, and whether it is answered or
/// refused is a fact of whether that connection is still a peering at all
/// ([`vpc_peering_connection_holds_its_pair`]).
pub fn vpc_peering_connection_awaits_acceptance(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST)
            | Some(VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE)
    )
}

/// The `VpcPeeringConnectionStateReason$Message` of a connection a delete has acted on,
/// naming the account that deleted it — either side's owner may delete an active connection,
/// and the requester's may delete a request still waiting to be answered
/// (`DeleteVpcPeeringConnection`), so the message says which one did.
pub fn vpc_peering_deleted_message(deleted_by_owner_id: &str) -> String {
    format!("Deleted by {deleted_by_owner_id}")
}

/// Whether a connection in this state still PEERS its two VPCs, so a second request to
/// peer the same pair is a duplicate ([`vpc_peering_connection_already_exists`]) rather
/// than a new request, and an acceptance of it is one the service can still answer.
///
/// A request on its way in, one waiting to be answered, one being set up and an active
/// connection all hold the pair; a `failed`, `expired`, `rejected`, `deleting` or `deleted`
/// one does not, so the same two VPCs may be peered again after one of those.
pub fn vpc_peering_connection_holds_its_pair(code: Option<&str>) -> bool {
    vpc_peering_connection_awaits_acceptance(code)
        || matches!(
            code,
            Some(VPC_PEERING_CONNECTION_STATE_PROVISIONING)
                | Some(VPC_PEERING_CONNECTION_STATE_ACTIVE)
        )
}

/// Whether a delete may act on a connection in this state.
///
/// "Either the owner of the requester VPC or the owner of the accepter VPC can delete the VPC
/// peering connection if it's in the active state. The owner of the requester VPC can delete a
/// VPC peering connection in the pending-acceptance state. You cannot delete a VPC peering
/// connection that's in the failed or rejected state" (`DeleteVpcPeeringConnection`): what a
/// delete acts on is a connection that still peers its two VPCs
/// ([`vpc_peering_connection_holds_its_pair`]) — a request on its way in, one waiting to be
/// answered, one being set up, and an active connection. Every other state is one the
/// connection stopped peering in (`failed`, `rejected`, `expired`) or one a delete has already
/// put it in (`deleting`, `deleted`), and there is nothing left for a delete to take.
pub fn vpc_peering_connection_is_deletable(code: Option<&str>) -> bool {
    vpc_peering_connection_holds_its_pair(code)
}

/// The caller-chosen bytes a stored peering connection holds: its tags. Everything else on
/// the record — its own id, the two VPC ids, the two accounts, the two Regions and its
/// status — is derived by this crate or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`super::carrier_gateway_payload_bytes`] is
/// for a carrier gateway: `CreateVpcPeeringConnection` asks it against the store's budget
/// before the write and [`ComAmazonawsEc2Amazonec2State::insert_vpc_peering_connection`]
/// charges exactly it.
pub fn vpc_peering_connection_payload_bytes(tags: &[Tag]) -> usize {
    tag_list_bytes(tags)
}

/// Refuse a request that named a peering connection id this account holds no connection
/// for: HTTP 400 [`INVALID_VPC_PEERING_CONNECTION_ID_NOT_FOUND`], with the message EC2
/// builds from the id the request named.
pub fn invalid_vpc_peering_connection_id_not_found<E>(
    vpc_peering_connection_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPC_PEERING_CONNECTION_ID_NOT_FOUND,
        400,
        format!("The vpcPeeringConnection ID '{vpc_peering_connection_id}' does not exist"),
    )
}

/// Refuse a create that would take the store past [`MAX_VPC_PEERING_CONNECTIONS`]: HTTP 400
/// [`VPC_PEERING_CONNECTION_LIMIT_EXCEEDED`].
pub fn vpc_peering_connection_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(VPC_PEERING_CONNECTION_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a request the state of the peering connection it names does not admit: HTTP 400
/// [`INVALID_STATE_TRANSITION`], naming the connection and the state it IS in, so the caller
/// can tell a connection that has already left the lifecycle from one it may still act on.
pub fn vpc_peering_connection_invalid_state_transition<E>(
    vpc_peering_connection_id: &str,
    state: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_STATE_TRANSITION,
        400,
        format!("Invalid state transition for {vpc_peering_connection_id}, state: {state}"),
    )
}

/// Refuse a request to peer two VPCs a connection of this account already peers: HTTP 400
/// [`VPC_PEERING_CONNECTION_ALREADY_EXISTS`], naming the connection that holds the pair so
/// the caller can act on it instead of creating a second one.
pub fn vpc_peering_connection_already_exists<E>(
    vpc_peering_connection_id: &str,
    vpc_id: &str,
    peer_vpc_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_PEERING_CONNECTION_ALREADY_EXISTS,
        400,
        format!(
            "A VPC peering connection between {vpc_id} and {peer_vpc_id} already exists: \
             {vpc_peering_connection_id}"
        ),
    )
}

/// Refuse an acceptance made in a Region other than the accepter VPC's: HTTP 400
/// `OperationNotPermitted`, naming the Region the request was made in and the one it has to
/// be made in.
///
/// "For an inter-Region VPC peering connection request, you must accept the VPC peering
/// connection in the Region of the accepter VPC" (`AcceptVpcPeeringConnection`): the
/// accepter's Region is the one the connection itself records for that side
/// (`VpcPeeringConnectionVpcInfo$Region`), and the request's is the one it was signed for.
pub fn vpc_peering_connection_wrong_region<E>(
    request_region: &str,
    vpc_peering_connection_id: &str,
    accepter_region: &str,
) -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "Incorrect region ({request_region}) specified for this request. VPC peering \
         connection {vpc_peering_connection_id} must be accepted in region {accepter_region}"
    ))
}

/// Refuse an acceptance by an account that does not own the accepter VPC: HTTP 400
/// `OperationNotPermitted`, naming the account that asked and the account that has to.
///
/// "To accept a request, … you must be the owner of the peer VPC"
/// (`AcceptVpcPeeringConnection`): the accepter's owner is the account the connection itself
/// records for that side (`VpcPeeringConnectionVpcInfo$OwnerId`), which a cross-account
/// request names ([`VpcPeeringConnectionSpec::peer_owner_id`]). This emulator serves one
/// account ([`::engine::emulator_account_id`]), so a request whose accepter belongs to
/// another is one no caller here can answer, and it is refused rather than accepted on that
/// account's behalf — worded alongside the Region refusal above, the two together being the
/// standing the operation documents for accepting.
pub fn vpc_peering_connection_wrong_account<E>(
    caller_account_id: &str,
    vpc_peering_connection_id: &str,
    accepter_owner_id: &str,
) -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "Incorrect account ({caller_account_id}) specified for this request. VPC peering \
         connection {vpc_peering_connection_id} must be accepted by account \
         {accepter_owner_id}"
    ))
}

/// What one peering connection is created with, beyond everything the service itself
/// derives ([`ComAmazonawsEc2Amazonec2State::insert_vpc_peering_connection`] mints the id,
/// fills in the requester's account and settles the status the pair's address space earns).
#[derive(Debug, Clone)]
pub struct VpcPeeringConnectionSpec {
    /// "The ID of the requester VPC" (`CreateVpcPeeringConnectionRequest$VpcId`), a VPC of
    /// this account.
    pub vpc_id: String,
    /// "The ID of the VPC with which you are creating the VPC peering connection"
    /// (`$PeerVpcId`).
    pub peer_vpc_id: String,
    /// "The Amazon Web Services account ID of the owner of the accepter VPC"
    /// (`$PeerOwnerId`), which the create defaults to this account as that member
    /// documents.
    pub peer_owner_id: String,
    /// The Region the request was signed for, which is where the requester VPC is.
    pub region: String,
    /// "The Region code for the accepter VPC" (`$PeerRegion`), which the create defaults to
    /// the Region the request was made in as that member documents.
    pub peer_region: String,
    /// "The tags to assign to the peering connection" (`$TagSpecifications`).
    pub tags: Vec<Tag>,
}

/// One stored VPC peering connection: the modelled [`VpcPeeringConnection`] shape beside
/// the move the clock owes it, which that shape carries no member for.
#[derive(Debug, Clone)]
pub struct VpcPeeringConnectionRecord {
    /// The connection as it is reported, except for the per-side address space
    /// [`ComAmazonawsEc2Amazonec2State::vpc_peering_connection_view`] reads off the VPC
    /// store (see the module docs).
    pub view: VpcPeeringConnection,
    /// The status move the clock owes it: `initiating-request` -> `pending-acceptance` for
    /// a request this store has just taken. A connection in a state nothing is owed from
    /// carries none.
    pub pending: Option<PendingStateMove>,
}

/// The IPv4 CIDR blocks `vpc` HOLDS, in the order it reports them.
///
/// Read off `Vpc$CidrBlockAssociationSet` through [`cidr_association_holds_its_block`], the
/// one reader of that question, so a block on its way out of the VPC is not reported as one
/// of its peered ranges.
///
/// The one definition of a VPC's own address space, so every reader of it agrees: the peered
/// ranges below, and the prefixes a transit gateway VPC attachment propagates
/// ([`ComAmazonawsEc2Amazonec2State::transit_gateway_route_set`]).
pub fn held_ipv4_cidr_blocks(vpc: &Vpc) -> Vec<String> {
    vpc.cidr_block_association_set
        .iter()
        .flatten()
        .filter(|association| {
            cidr_association_holds_its_block(
                association
                    .cidr_block_state
                    .as_ref()
                    .and_then(|block| block.state.as_deref()),
            )
        })
        .filter_map(|association| association.cidr_block.clone())
        .collect()
}

/// The IPv6 CIDR blocks `vpc` HOLDS, read off `Vpc$Ipv6CidrBlockAssociationSet` on the same
/// terms as [`held_ipv4_cidr_blocks`].
pub fn held_ipv6_cidr_blocks(vpc: &Vpc) -> Vec<String> {
    vpc.ipv6_cidr_block_association_set
        .iter()
        .flatten()
        .filter(|association| {
            cidr_association_holds_its_block(
                association
                    .ipv6_cidr_block_state
                    .as_ref()
                    .and_then(|block| block.state.as_deref()),
            )
        })
        .filter_map(|association| association.ipv6_cidr_block.clone())
        .collect()
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another peering connection fits inside the store's ceiling
    /// ([`MAX_VPC_PEERING_CONNECTIONS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses with
    /// [`vpc_peering_connection_limit_exceeded`] when it answers `false`.
    pub fn has_vpc_peering_connection_capacity(&self) -> bool {
        self.vpc_peering_connections.len() < MAX_VPC_PEERING_CONNECTIONS
    }

    /// The id of the connection that already peers `vpc_id` with `peer_vpc_id`, if this
    /// account holds one.
    ///
    /// A peering connection is between two VPCs rather than FROM one to the other, so the
    /// pair is matched in either direction: a request that names the two the other way round
    /// asks for the connection that already exists. Only a connection that still HOLDS its
    /// pair counts ([`vpc_peering_connection_holds_its_pair`]) — after a failure, a
    /// rejection, an expiry or a deletion the same two VPCs may be peered again.
    pub fn vpc_peering_connection_between(
        &self,
        vpc_id: &str,
        peer_vpc_id: &str,
    ) -> Option<&str> {
        self.vpc_peering_connections
            .iter()
            .filter(|(_id, record)| {
                vpc_peering_connection_holds_its_pair(
                    record
                        .view
                        .status
                        .as_ref()
                        .and_then(|status| status.code.as_deref()),
                )
            })
            .find(|(_id, record)| {
                let side = |info: &Option<VpcPeeringConnectionVpcInfo>| {
                    info.as_ref()
                        .and_then(|info| info.vpc_id.clone())
                        .unwrap_or_default()
                };
                let requester = side(&record.view.requester_vpc_info);
                let accepter = side(&record.view.accepter_vpc_info);
                (requester == vpc_id && accepter == peer_vpc_id)
                    || (requester == peer_vpc_id && accepter == vpc_id)
            })
            .map(|(id, _record)| id.as_str())
    }

    /// Store one peering connection as of `created_at`, and answer the
    /// `VpcPeeringConnectionId` it was minted with.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller,
    /// so two writers cannot report one connection differently: the id, the requester's
    /// account, the status the request starts in and the instant it expires at. The
    /// caller-chosen bytes the record retains ([`vpc_peering_connection_payload_bytes`]) are
    /// charged against the store's byte budget here, the way
    /// [`Self::insert_carrier_gateway`] charges a gateway's, so a writer cannot store them
    /// uncharged.
    ///
    /// The status is settled HERE because it is a fact of the two VPCs' address space rather
    /// than of the request: "If you create a VPC peering connection request between VPCs
    /// with overlapping CIDR blocks, the VPC peering connection has a status of failed"
    /// (`CreateVpcPeeringConnection`). A request this store can see both sides of — the peer
    /// VPC belongs to this account — is `failed` when any block of the one meets any block of
    /// the other, and a failed request expires at no instant, having nothing left to accept.
    /// Every other request starts `initiating-request`, with `pending-acceptance` owed to the
    /// clock one [`super::TRANSITION_DELAY`] later and an expiry
    /// [`VPC_PEERING_CONNECTION_REQUEST_LIFETIME`] after it was made.
    ///
    /// The two instants are read off two clocks on purpose, and `created_at` is only one of
    /// them: the EXPIRY is a wall-clock instant, dated from `created_at` so a caller (and a
    /// test) can date a request earlier than now, while the owed status MOVE is monotonic and
    /// always falls due one span after this insert — the request is taken when it is stored,
    /// whatever instant it is dated from, and a monotonic clock cannot be dated backwards.
    /// A request dated far enough back is therefore `expired` by the time anything reads it,
    /// which is the state that wins over the move it was owed.
    pub fn insert_vpc_peering_connection(
        &mut self,
        spec: VpcPeeringConnectionSpec,
        created_at: SystemTime,
    ) -> String {
        let vpc_peering_connection_id = self.next_id(VPC_PEERING_CONNECTION_ID_PREFIX);
        let requester_owner_id = ::engine::emulator_account_id().to_string();
        let overlapping = self.peered_cidr_blocks_overlap(&spec);
        let status_code = if overlapping {
            VPC_PEERING_CONNECTION_STATE_FAILED
        } else {
            VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST
        };
        let view = VpcPeeringConnection {
            vpc_peering_connection_id: Some(vpc_peering_connection_id.clone()),
            requester_vpc_info: Some(VpcPeeringConnectionVpcInfo {
                vpc_id: Some(spec.vpc_id),
                owner_id: Some(requester_owner_id),
                region: Some(spec.region),
                ..VpcPeeringConnectionVpcInfo::default()
            }),
            accepter_vpc_info: Some(VpcPeeringConnectionVpcInfo {
                vpc_id: Some(spec.peer_vpc_id),
                owner_id: Some(spec.peer_owner_id.clone()),
                region: Some(spec.peer_region),
                ..VpcPeeringConnectionVpcInfo::default()
            }),
            status: Some(VpcPeeringConnectionStateReason {
                code: Some(status_code.to_string()),
                // Through the ONE mapping from a status to the sentence reported beside it,
                // so the state a request is minted in cannot be worded one way here and
                // another by the clock or by an acceptance.
                message: vpc_peering_connection_state_message(
                    status_code,
                    &spec.peer_owner_id,
                ),
            }),
            // "The time that an unaccepted VPC peering connection will expire"
            // (`VpcPeeringConnection$ExpirationTime`): only a request that can still be
            // accepted has one.
            expiration_time: (!overlapping)
                .then(|| created_at.checked_add(VPC_PEERING_CONNECTION_REQUEST_LIFETIME))
                .flatten()
                .map(system_time_to_date_time),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(vpc_peering_connection_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
        ));
        let pending = (status_code == VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST).then(|| {
            PendingStateMove::starting_now(VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE)
        });
        self.vpc_peering_connections.insert(
            vpc_peering_connection_id.clone(),
            VpcPeeringConnectionRecord { view, pending },
        );
        vpc_peering_connection_id
    }

    /// Record the acceptance of the peering request `vpc_peering_connection_id` names, and
    /// answer the connection as it now stands — `None` when this store holds none.
    ///
    /// The ONE writer of an acceptance, as [`Self::insert_vpc_peering_connection`] is of a
    /// request. The request leaves the state it was waiting in for
    /// [`VPC_PEERING_CONNECTION_STATE_ACTIVE`], the state a peering that carries traffic is
    /// in, and keeps no expiry: "The time that an UNACCEPTED VPC peering connection will
    /// expire" (`VpcPeeringConnection$ExpirationTime`) is a fact of a request nobody has
    /// answered, and this one now has been.
    ///
    /// The peering is set up WITHIN the call that accepts it, so nothing is owed to the clock
    /// afterwards — the same reading [`super::VPC_PROVISIONING_DELAY`] gives a VPC that is
    /// provisioned within its own create: the acceptance's answer is where the real service
    /// reports what the connection became, and the read that follows it with nothing in
    /// between already reports the connection carrying traffic. (The `provisioning` member of
    /// the status catalog belongs to a peering whose two sides a Region or an account apart
    /// still have to be joined, which is work this store cannot do for another account's VPC.)
    ///
    /// Whether the connection is in a state that admits an acceptance is the CALLER's
    /// question ([`vpc_peering_connection_invalid_state_transition`] is how it refuses one
    /// that is not), because it is asked of the request alongside who may answer it and
    /// where.
    pub fn accept_vpc_peering_connection(
        &mut self,
        vpc_peering_connection_id: &str,
    ) -> Option<VpcPeeringConnection> {
        let record = self
            .vpc_peering_connections
            .get_mut(vpc_peering_connection_id)?;
        let accepter_owner_id = record
            .view
            .accepter_vpc_info
            .as_ref()
            .and_then(|info| info.owner_id.clone())
            .unwrap_or_default();
        record.view.status = Some(VpcPeeringConnectionStateReason {
            code: Some(VPC_PEERING_CONNECTION_STATE_ACTIVE.to_string()),
            message: vpc_peering_connection_state_message(
                VPC_PEERING_CONNECTION_STATE_ACTIVE,
                &accepter_owner_id,
            ),
        });
        record.view.expiration_time = None;
        // The move the request was owed while it waited goes with the answer: a connection
        // that has been accepted is not on its way to being waited on.
        record.pending = None;
        // Reported through the ONE projection of a stored connection, so the acceptance and
        // the describe that follows it cannot answer differently.
        let accepted = record.clone();
        Some(self.vpc_peering_connection_view(&accepted))
    }

    /// Record the refusal of the peering request `vpc_peering_connection_id` names, and
    /// answer whether this store held one to refuse.
    ///
    /// The ONE writer of a rejection, as [`Self::accept_vpc_peering_connection`] is of an
    /// acceptance: the request leaves the state it was waiting in for
    /// [`VPC_PEERING_CONNECTION_STATE_REJECTED`], which is terminal — a refused request is
    /// answered, so the clock owes it nothing further and it holds its pair no longer
    /// ([`vpc_peering_connection_holds_its_pair`]), the same two VPCs being free to be peered
    /// again.
    ///
    /// Two facts follow the refusal, both for the same reason a deletion's do
    /// ([`Self::delete_vpc_peering_connection`]): the connection will never carry traffic, so
    /// every route whose target is it READS `blackhole` — "the `blackhole` state indicates that
    /// the route's target isn't available" (`Route$State`), derived from the connection's state
    /// wherever the route is reported ([`Self::reported_route_table`]) — and it expires at no
    /// instant, "the time that an UNACCEPTED VPC peering connection will expire"
    /// (`VpcPeeringConnection$ExpirationTime`) being a fact of a request nobody has answered.
    ///
    /// Whether the connection is in a state that admits a refusal is the CALLER's question,
    /// the way it is for an acceptance: it is asked of the request alongside who may answer it
    /// and where ([`vpc_peering_connection_invalid_state_transition`] is how it refuses one
    /// that is not).
    pub fn reject_vpc_peering_connection(&mut self, vpc_peering_connection_id: &str) -> bool {
        let Some(record) = self
            .vpc_peering_connections
            .get_mut(vpc_peering_connection_id)
        else {
            return false;
        };
        record.view.status = Some(VpcPeeringConnectionStateReason {
            code: Some(VPC_PEERING_CONNECTION_STATE_REJECTED.to_string()),
            // Through the ONE mapping from a status to the sentence reported beside it, so a
            // refused request is worded the same however it is read back. The accepter's
            // account is passed for the shape of that mapping; this state names none.
            message: vpc_peering_connection_state_message(
                VPC_PEERING_CONNECTION_STATE_REJECTED,
                record
                    .view
                    .accepter_vpc_info
                    .as_ref()
                    .and_then(|info| info.owner_id.as_deref())
                    .unwrap_or_default(),
            ),
        });
        record.view.expiration_time = None;
        record.pending = None;
        true
    }

    /// Whether the two VPCs `spec` names have overlapping address space, as far as this
    /// store can see.
    ///
    /// The requester VPC is this account's, so the store holds it; the accepter's is only
    /// this account's when the request names no other owner, and an account's VPC this store
    /// does not hold has no blocks to compare. A pair this store cannot see both sides of
    /// therefore overlaps nothing, which is the same answer it gives for a pair whose blocks
    /// do not meet.
    fn peered_cidr_blocks_overlap(&self, spec: &VpcPeeringConnectionSpec) -> bool {
        if spec.peer_owner_id != ::engine::emulator_account_id() {
            return false;
        }
        let (Some(requester), Some(accepter)) = (
            self.vpcs.get(&spec.vpc_id),
            self.vpcs.get(&spec.peer_vpc_id),
        ) else {
            return false;
        };
        // Each side's blocks are read ONCE, before the comparison: the question is whether
        // any block of the one meets any block of the other, which costs the product of the
        // two counts in comparisons and should not cost it in allocations too.
        let requester_blocks = held_ipv4_cidr_blocks(requester);
        let accepter_blocks = held_ipv4_cidr_blocks(accepter);
        requester_blocks.iter().any(|left| {
            accepter_blocks
                .iter()
                .any(|right| ipv4_cidr_blocks_overlap(left, right))
        })
    }

    /// One peering connection as it is REPORTED: the stored record, with each side's
    /// address space read off the VPC this store holds for it.
    ///
    /// "Information about the accepter VPC. CIDR block information is only returned when
    /// describing an active VPC peering connection"
    /// (`VpcPeeringConnection$AccepterVpcInfo`): the requester VPC is the one this account
    /// owns, so its ranges are reported from the moment the request is made, while the
    /// accepter's are reported once the connection is
    /// [`VPC_PEERING_CONNECTION_STATE_ACTIVE`]. A side whose VPC this store does not hold —
    /// one in another account — is reported with the ids and the Region the peering itself
    /// records and no ranges, rather than ranges this emulator would have to invent.
    ///
    /// The ONE projection of a stored connection, so `CreateVpcPeeringConnection` and
    /// `DescribeVpcPeeringConnections` cannot report one differently.
    pub fn vpc_peering_connection_view(
        &self,
        record: &VpcPeeringConnectionRecord,
    ) -> VpcPeeringConnection {
        let mut view = record.view.clone();
        let active = view
            .status
            .as_ref()
            .and_then(|status| status.code.as_deref())
            == Some(VPC_PEERING_CONNECTION_STATE_ACTIVE);
        for (info, report_ranges) in [
            (view.requester_vpc_info.as_mut(), true),
            (view.accepter_vpc_info.as_mut(), active),
        ] {
            let Some(info) = info.filter(|_| report_ranges) else {
                continue;
            };
            let Some(vpc) = info
                .vpc_id
                .as_deref()
                .and_then(|vpc_id| self.vpcs.get(vpc_id))
            else {
                continue;
            };
            let ipv4 = held_ipv4_cidr_blocks(vpc);
            let ipv6 = held_ipv6_cidr_blocks(vpc);
            // "The IPv4 CIDR block for the VPC" is the VPC's primary block, and
            // "Information about the IPv4 CIDR blocks for the VPC" every block it holds
            // (`VpcPeeringConnectionVpcInfo$CidrBlock` / `$CidrBlockSet`). A VPC holding no
            // block of a family is reported with no set for it rather than an empty one.
            info.cidr_block = vpc.cidr_block.clone();
            info.cidr_block_set = (!ipv4.is_empty()).then(|| {
                ipv4.into_iter()
                    .map(|cidr_block| CidrBlock {
                        cidr_block: Some(cidr_block),
                    })
                    .collect()
            });
            info.ipv6_cidr_block_set = (!ipv6.is_empty()).then(|| {
                ipv6.into_iter()
                    .map(|ipv6_cidr_block| Ipv6CidrBlock {
                        ipv6_cidr_block: Some(ipv6_cidr_block),
                    })
                    .collect()
            });
        }
        view
    }

    /// Apply the status moves the clock owes this store's peering connections, as of the
    /// monotonic instant `now` and the wall clock `wall_clock`.
    ///
    /// Two moves, both owed at their instant whether or not anybody called — which is why a
    /// reporting operation may materialise them:
    ///
    /// * a request `CreateVpcPeeringConnection` answered
    ///   [`VPC_PEERING_CONNECTION_STATE_INITIATING_REQUEST`] is
    ///   [`VPC_PEERING_CONNECTION_STATE_PENDING_ACCEPTANCE`] once its due instant has
    ///   passed, the same contract as a `pending` VPC's move to `available`;
    /// * a request nobody answered by its own `ExpirationTime` expired at that instant:
    ///   "The VPC peering connection request expires after 7 days, after which it cannot be
    ///   accepted or rejected" (`CreateVpcPeeringConnection`).
    ///
    /// Wall-clock instants are compared as `(seconds, nanoseconds)`, the way
    /// [`Self::expire_due_image_usage_reports`] compares its own: the modelled timestamp
    /// type carries no `Ord`.
    pub fn materialize_due_vpc_peering_connections(
        &mut self,
        now: Instant,
        wall_clock: SystemTime,
    ) {
        fn key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
            (at.secs(), at.subsec_nanos())
        }

        let wall_clock = key(system_time_to_date_time(wall_clock));
        for record in self.vpc_peering_connections.values_mut() {
            // The accepter's account, which both of a request's own status messages name.
            let accepter_owner_id = record
                .view
                .accepter_vpc_info
                .as_ref()
                .and_then(|info| info.owner_id.clone())
                .unwrap_or_default();
            if let Some(due) = record
                .pending
                .as_ref()
                .and_then(|owed| owed.due_by(now))
                .map(str::to_string)
            {
                // "A message that provides more information about the status, if applicable"
                // (`VpcPeeringConnectionStateReason$Message`): a deletion that has FINISHED
                // keeps the sentence the delete wrote, which names the account that deleted
                // it; every other state takes the sentence that state carries, from the one
                // mapping of a peering's status to its message.
                let message = match due.as_str() {
                    VPC_PEERING_CONNECTION_STATE_DELETED => record
                        .view
                        .status
                        .as_ref()
                        .and_then(|status| status.message.clone()),
                    due => vpc_peering_connection_state_message(due, &accepter_owner_id),
                };
                record.view.status = Some(VpcPeeringConnectionStateReason {
                    code: Some(due.clone()),
                    message,
                });
                // The move is applied once; nothing is owed on this connection any more.
                record.pending = None;
            }
            let code = record
                .view
                .status
                .as_ref()
                .and_then(|status| status.code.as_deref());
            let expired = record
                .view
                .expiration_time
                .map(key)
                .is_some_and(|expires_at| wall_clock >= expires_at);
            if expired && vpc_peering_connection_awaits_acceptance(code) {
                record.view.status = Some(VpcPeeringConnectionStateReason {
                    code: Some(VPC_PEERING_CONNECTION_STATE_EXPIRED.to_string()),
                    // "A message that provides more information about the status, IF
                    // APPLICABLE" (`VpcPeeringConnectionStateReason$Message`): the message
                    // the request carried named the account that was to answer it, which an
                    // expired request is no longer waiting for — so the one mapping from a
                    // status to its sentence reports none for this state.
                    message: vpc_peering_connection_state_message(
                        VPC_PEERING_CONNECTION_STATE_EXPIRED,
                        &accepter_owner_id,
                    ),
                });
                // A request that has expired is owed nothing further, and it can no longer
                // be accepted, so the move it was waiting on goes with it.
                record.pending = None;
            }
        }
    }

    /// Delete the peering connection `vpc_peering_connection_id` on behalf of
    /// `deleted_by_owner_id`, and answer whether this store held one to delete.
    ///
    /// The ONE writer of a peering deletion, so nothing can report a deleted connection two
    /// ways. `VpcPeeringConnectionStateReasonCode` carries `deleting` beside `deleted`
    /// (`smithy_query(query="shape",shape="VpcPeeringConnectionStateReasonCode")`), so a
    /// connection a delete has been asked for is still a connection the API reports: the
    /// delete writes the transient [`VPC_PEERING_CONNECTION_STATE_DELETING`] and the clock
    /// owes it [`VPC_PEERING_CONNECTION_STATE_DELETED`] one [`super::TRANSITION_DELAY`] later
    /// ([`Self::materialize_due_vpc_peering_connections`]), rather than making the record
    /// vanish — the same contract as a NAT gateway's delete.
    ///
    /// Two facts follow the deletion, because the connection no longer carries traffic and no
    /// longer waits for anybody:
    ///
    /// * every route whose target is this connection READS `blackhole` — "the `blackhole`
    ///   state indicates that the route's target isn't available" (`Route$State`), derived from
    ///   the connection's state wherever the route is reported
    ///   ([`Self::reported_route_table`]);
    /// * it expires at no instant, having nothing left to accept
    ///   (`VpcPeeringConnection$ExpirationTime` is "the time that an UNACCEPTED VPC peering
    ///   connection will expire").
    ///
    /// The record's tags stay stored, so the bytes they were charged stay charged: the
    /// connection is still reported, tags and all, until something removes the record.
    pub fn delete_vpc_peering_connection(
        &mut self,
        vpc_peering_connection_id: &str,
        deleted_by_owner_id: &str,
    ) -> bool {
        let Some(record) = self
            .vpc_peering_connections
            .get_mut(vpc_peering_connection_id)
        else {
            return false;
        };
        record.view.status = Some(VpcPeeringConnectionStateReason {
            code: Some(VPC_PEERING_CONNECTION_STATE_DELETING.to_string()),
            message: Some(vpc_peering_deleted_message(deleted_by_owner_id)),
        });
        record.view.expiration_time = None;
        record.pending = Some(PendingStateMove::starting_now(
            VPC_PEERING_CONNECTION_STATE_DELETED,
        ));
        true
    }
}
