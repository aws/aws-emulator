// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmanager-networkmanager service.
//!
//! The global-network / site / link lifecycle is the core resource tree of Network
//! Manager: a global network owns sites, a site owns links, and a global network owns
//! devices (each optionally assigned to one of its sites). A device and a link that sit at
//! the same site can be tied together by a LINK ASSOCIATION, which has no id of its own
//! and is therefore keyed by that pair. A device can likewise be tied to a CUSTOMER
//! GATEWAY of Amazon VPC — and, when the request names one, to a link of that device — by a
//! CUSTOMER GATEWAY ASSOCIATION, which has no id either and is keyed by the global network
//! plus the gateway's ARN. Two devices of one global network can instead be
//! tied together by a CONNECTION, which unlike that association IS a resource of its own,
//! with an id, an ARN and tags. A global network also owns its
//! CORE network — the Cloud WAN managed network
//! whose segments and edges are a function of the core network policy document it holds.
//! A core network edge can PEER with a resource of another service (a transit gateway),
//! and a core network in turn owns its ATTACHMENTS: each joins one resource to the edge
//! locations that policy configures — one of them for every kind but a Direct Connect
//! gateway attachment, which spans several — and a Connect attachment is carried over
//! another attachment (its transport) rather than over a resource of its own, and a
//! transit gateway route table attachment over a PEERING — the core network's own
//! connection to a transit gateway, which is a resource of its own with an id, an ARN and
//! tags, and which fixes the core network and the edge location the attachment over it
//! takes. A Connect
//! attachment in turn carries CONNECT PEERS, each a BGP peering between one edge of the
//! core network and one appliance outside AWS, and each a resource of its own with an id,
//! an ARN and tags. A Connect peer can in turn be ASSOCIATED with one device of the global
//! network that owns its core network, optionally over one of that device's links: like a
//! link association that tie has no id of its own and is keyed by the peer it ties. The
//! policy's
//! `attachment-policies` rules decide what becomes of each attachment created on the core
//! network — which segment or network function group it joins, and whether its attachment
//! request has to be ACCEPTED first, which is what holds it in
//! `PENDING_ATTACHMENT_ACCEPTANCE` until `AcceptAttachment` grants that acceptance. A
//! Connect attachment in turn owns the CONNECT PEERS built on it: each is one BGP peering
//! between that attachment's edge of the core network and an appliance.
//! Beside that tree a global network holds the telemetry-bearing connections registered
//! with it (a Connect peer of either kind, a VPN connection), whose session health is what
//! `GetNetworkTelemetry` reports.
//! A global network also holds the TRANSIT GATEWAYS registered with it: each registration
//! is the tie between the global network and one transit gateway of Amazon VPC, with no id
//! of its own, so it is keyed by that pair like a customer gateway association. A CONNECT
//! PEER of such a gateway can in turn be tied to one device of that global network, and
//! optionally to one of that device's links, by a TRANSIT GATEWAY CONNECT PEER ASSOCIATION,
//! which has no id either and is keyed by the global network plus the peer's own ARN; that
//! tie is what registers the peering with the global network's telemetry.
//! A global network also holds the ROUTE ANALYSES started on it: each is a Route Analyzer
//! job that analysed the routing path between two attachments against what the global
//! network held when it was started, and holds the paths it found for `GetRouteAnalysis`
//! to read back.
//! Any resource of a global network's network-resource inventory can in turn carry
//! METADATA — the key/value map `UpdateNetworkResourceMetadata` stores for it and
//! `GetNetworkResources` reports beside its definition — held under that resource's own ARN.
//! Everything
//! the implemented operations answer is computed from the records below — no handler
//! echoes its input back without storing it.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// The two shapes a core network change event is reported through, and the two a change of
// a change set is reported through, imported on their own so this slice's shape list below
// stays exactly as the rest of the service shares it.
use crate::ops::types::{CoreNetworkChange, CoreNetworkChangeValues};
use crate::ops::types::{CoreNetworkChangeEvent, CoreNetworkChangeEventValues};
// The shapes one change reports a segment's service insertion actions through, imported
// beside them for the same reason.
use crate::ops::types::{NetworkFunctionGroup, ServiceInsertionAction, Via};
use crate::ops::types::{
    AWSLocation, AccountStatus, Attachment, AttachmentRoutingPolicyAssociationSummary, Bandwidth,
    ConnectAttachment, ConnectAttachmentOptions,
    ConnectPeer, ConnectPeerAssociation, ConnectPeerBgpConfiguration, ConnectPeerConfiguration,
    ConnectPeerSummary,
    ConflictException,
    Connection, ConnectionHealth, CoreNetwork, CoreNetworkEdge, CoreNetworkNetworkFunctionGroup,
    CustomerGatewayAssociation,
    CoreNetworkPolicy, CoreNetworkPolicyError, CoreNetworkPolicyException, CoreNetworkPolicyVersion,
    CoreNetworkSegment,
    Device, DirectConnectGatewayAttachment, GlobalNetwork, Link, LinkAssociation, Location,
    NetworkResource, NetworkTelemetry, OrganizationStatus, Peering, Relationship,
    ResourceNotFoundException, RouteAnalysis, RouteAnalysisCompletion,
    RouteAnalysisEndpointOptions, RouteAnalysisEndpointOptionsSpecification, RouteAnalysisPath,
    ServiceInsertionSegments, Site, SiteToSiteVpnAttachment, Tag,
    TransitGatewayConnectPeerAssociation, TransitGatewayPeering,
    TransitGatewayRegistration, TransitGatewayRegistrationStateReason,
    TransitGatewayRouteTableAttachment, ValidationException,
    ValidationExceptionField, VpcAttachment, VpcOptions,
};

// The state is one store with one record per resource; these modules group the records
// and the computations by concern, and re-export everything, so every path a handler
// already uses keeps resolving through `crate::state`.
mod attachments;
mod limits;
mod policy;
mod records;
mod store;

pub use attachments::*;
pub use limits::*;
pub use policy::*;
pub use records::*;

/// Members of `GlobalNetworkState` / `SiteState` / `LinkState` this slice uses. All three
/// enums carry the same four values (`smithy_query shape=GlobalNetworkState,SiteState,LinkState`:
/// AVAILABLE, DELETING, PENDING, UPDATING), mirrored in
/// `ops::types::GLOBAL_NETWORK_STATE_VALUES` / `SITE_STATE_VALUES` / `LINK_STATE_VALUES`.
pub const STATE_PENDING: &str = "PENDING";
/// See [`STATE_PENDING`].
pub const STATE_AVAILABLE: &str = "AVAILABLE";
/// See [`STATE_PENDING`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_PENDING`]. The state a resource of one of those kinds is in between the
/// update that reconfigures it and the instant that reconfiguration is applied.
pub const STATE_UPDATING: &str = "UPDATING";

/// The state a resource whose CREATION is still in flight is in, where the resources
/// above wait in [`STATE_PENDING`]: a value of `CoreNetworkState`, `PeeringState`,
/// `ConnectPeerState` and `AttachmentState` (`smithy_query
/// shape=CoreNetworkState,PeeringState,ConnectPeerState,AttachmentState`, mirrored in
/// `ops::types::CORE_NETWORK_STATE_VALUES` and the three beside it), which carry CREATING
/// where `GlobalNetworkState` and its two siblings carry PENDING; AVAILABLE and DELETING
/// they share with them.
///
/// A peering, a Connect peer and an attachment are each answered in it by the call that
/// creates them and wait in it for the transition the clock owes. A CORE NETWORK is not:
/// its create answers it already [`STATE_AVAILABLE`] ([`CoreNetworkRecord::state_at`]), so
/// nothing of this service reports a core network in this state.
pub const STATE_CREATING: &str = "CREATING";

/// The state an ATTACHMENT waits in while the `attachment-policies` rule of the core
/// network policy that claimed it asks for the attachment request to be accepted. One of
/// `AttachmentState`'s own values (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`).
///
/// Unlike [`STATE_CREATING`] this is NOT a wait on the clock: no span is owed on it, and
/// only an acceptance (`AcceptAttachment`, [`AttachmentRecord::accept_at`]) moves an
/// attachment out of it — after which the ordinary `CREATING` -> `AVAILABLE` transition
/// the clock owes begins.
pub const STATE_PENDING_ATTACHMENT_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

/// The TERMINAL state of an attachment whose attachment request was turned down
/// (`RejectAttachment`, [`AttachmentRecord::reject_at`]). One of `AttachmentState`'s own
/// values (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`).
///
/// Like [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] this is not a wait on the clock, and unlike
/// it nothing moves an attachment out of it: the request was never granted, so the
/// attachment is never created and no transition is ever owed on it. The record stays
/// stored and readable in this state until `DeleteAttachment` removes it.
pub const STATE_REJECTED: &str = "REJECTED";

/// The ARN namespace the model declares for this service (`arnNamespace: networkmanager`).
const ARN_NAMESPACE: &str = "networkmanager";

/// The resource kinds this slice mints: both the id prefix (`global-network-<hex>`,
/// `site-<hex>`, `link-<hex>`) and the ARN resource type (`global-network/<id>`,
/// `site/<id>`, `link/<id>`).
pub const GLOBAL_NETWORK_KIND: &str = "global-network";
/// See [`GLOBAL_NETWORK_KIND`].
pub const SITE_KIND: &str = "site";
/// See [`GLOBAL_NETWORK_KIND`].
pub const LINK_KIND: &str = "link";
/// See [`GLOBAL_NETWORK_KIND`].
pub const DEVICE_KIND: &str = "device";
/// See [`GLOBAL_NETWORK_KIND`]. A connection hangs off the global network and ties two of
/// its devices together ("Creates a connection between two devices", `smithy_query
/// query="doc" shape="CreateConnection"`), so one name serves the id prefix
/// (`connection-<hex>`), the ARN resource type (`connection/<id>`) and the `connection`
/// entry of [`NETWORK_RESOURCE_TYPES`].
pub const CONNECTION_KIND: &str = "connection";
/// The resource kind a link ASSOCIATION reports in a refusal that names it.
///
/// Unlike the kinds above this one is not an ARN resource type and no id is minted for
/// it: a link association has no ARN and no id of its own — "Describes the association
/// between a device and a link" (`smithy_query shape=LinkAssociation`) is the
/// device/link pair plus its state — so the pair is its identity
/// ([`link_association_key`]) and this name is only how a not-found refusal spells the
/// resource type.
pub const LINK_ASSOCIATION_KIND: &str = "link-association";
/// The resource kind a CUSTOMER GATEWAY association reports in a refusal that names it.
///
/// Like [`LINK_ASSOCIATION_KIND`] this is not an ARN resource type and no id is minted for
/// it: "Describes the association between a customer gateway, a device, and a link"
/// (`smithy_query shape=CustomerGatewayAssociation`) carries no id, so the global network
/// plus the customer gateway ARN is its identity
/// ([`customer_gateway_association_key`]) and this name is only how a not-found refusal
/// spells the resource type.
pub const CUSTOMER_GATEWAY_ASSOCIATION_KIND: &str = "customer-gateway-association";
/// The resource kind a TRANSIT GATEWAY REGISTRATION reports in a refusal that names it.
///
/// Like [`CUSTOMER_GATEWAY_ASSOCIATION_KIND`] this is not an ARN resource type and no id is
/// minted for it: "Describes the registration of a transit gateway to a global network"
/// (`smithy_query shape=TransitGatewayRegistration`) carries no id, so the global network
/// plus the transit gateway's ARN is its identity
/// ([`transit_gateway_registration_key`]) and this name is only how a not-found refusal
/// spells the resource type.
pub const TRANSIT_GATEWAY_REGISTRATION_KIND: &str = "transit-gateway-registration";
/// The resource kind a TRANSIT GATEWAY CONNECT PEER association reports in a refusal that
/// names it.
///
/// Like [`TRANSIT_GATEWAY_REGISTRATION_KIND`] this is not an ARN resource type and no id is
/// minted for it: "Describes a transit gateway Connect peer association" (`smithy_query
/// shape=TransitGatewayConnectPeerAssociation`) carries no id, so the global network plus
/// the peer's own ARN is its identity
/// ([`transit_gateway_connect_peer_association_key`]) and this name is only how a
/// not-found refusal spells the resource type.
pub const TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND: &str =
    "transit-gateway-connect-peer-association";
/// The resource kind a CORE NETWORK CONNECT PEER association reports in a refusal that
/// names it.
///
/// Like [`TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND`] this is not an ARN resource type
/// and no id is minted for it: "Describes a core network Connect peer association"
/// (`smithy_query shape=ConnectPeerAssociation`) carries no id of its own, so the PEER is
/// its identity ([`ConnectPeerAssociationRecord`]) and this name is only how a not-found
/// refusal spells the resource type of the association, as against the
/// [`CONNECT_PEER_KIND`] of the peer itself.
pub const CONNECT_PEER_ASSOCIATION_KIND: &str = "connect-peer-association";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains this one's id itself:
/// `CoreNetworkId` carries `pattern ^core-network-([0-9a-f]{8,17})$`
/// (`smithy_query shape=CoreNetworkId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const CORE_NETWORK_KIND: &str = "core-network";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains this one's id itself too:
/// `PeeringId` carries `pattern ^peering-([0-9a-f]{8,17})$` (`smithy_query
/// shape=PeeringId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy. It is also the
/// `peering` entry of [`NETWORK_RESOURCE_TYPES`].
pub const PEERING_KIND: &str = "peering";
/// The resource kind a refusal names when the resource addressed is ONE policy of a core
/// network rather than the core network itself. A policy version is not a resource this
/// service mints an id or an ARN for — the model identifies it by its owning
/// `CoreNetworkId` plus an `Integer` `PolicyVersionId` (`smithy_query
/// operation=GetCoreNetworkPolicy,DeleteCoreNetworkPolicyVersion`) — so the pair travels
/// in the refusal's `ResourceId` ([`policy_version_resource_id`]) and this kind in its
/// `ResourceType`.
pub const CORE_NETWORK_POLICY_KIND: &str = "core-network-policy";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains an attachment's id too:
/// `AttachmentId` carries `pattern ^attachment-([0-9a-f]{8,17})$` (`smithy_query
/// shape=AttachmentId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy. It is also the
/// `attachment` entry of [`NETWORK_RESOURCE_TYPES`], so one name serves the id prefix,
/// the ARN resource type and the network-resource inventory.
pub const ATTACHMENT_KIND: &str = "attachment";
/// The resource kind a refusal names when the resource addressed is one PREFIX LIST
/// ASSOCIATION of a core network.
///
/// Like [`LINK_ASSOCIATION_KIND`] this is not an ARN resource type and no id is minted for
/// it: the modelled `PrefixListAssociation` is the core network / prefix list pair plus the
/// alias, with no id, ARN or state of its own (`smithy_query shape=PrefixListAssociation`),
/// so the pair travels in a refusal's `ResourceId` ([`prefix_list_association_key`]) and
/// this kind in its `ResourceType`.
pub const PREFIX_LIST_ASSOCIATION_KIND: &str = "prefix-list-association";
/// See [`GLOBAL_NETWORK_KIND`]. A ROUTE ANALYSIS is a job of its global network rather
/// than a resource with an ARN — the modelled `RouteAnalysis` carries only a
/// `RouteAnalysisId` (`smithy_query shape=RouteAnalysis`) — so this name serves the id
/// prefix (`route-analysis-<hex>`) and the `ResourceType` a not-found refusal reports.
pub const ROUTE_ANALYSIS_KIND: &str = "route-analysis";

/// The one value `PeeringType` enumerates (`smithy_query shape=PeeringType`, mirrored in
/// `ops::types::PEERING_TYPE_VALUES`): "The type of peering. This will be
/// `TRANSIT_GATEWAY`" (`smithy_query query="doc" shape="Peering" member="PeeringType"`).
pub const PEERING_TYPE_TRANSIT_GATEWAY: &str = "TRANSIT_GATEWAY";

/// The id prefix of the peering attachment a transit gateway peering leaves on the
/// transit gateway side: `TransitGatewayPeeringAttachmentId` carries `pattern
/// ^tgw-attach-([0-9a-f]{8,17})$` (`smithy_query shape=TransitGatewayPeering`), which the
/// 17 hex digits of [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
///
/// Unlike the kinds above this one is not an ARN resource type of this service: the
/// attachment it names belongs to the peered transit gateway, an Amazon VPC resource.
pub const TRANSIT_GATEWAY_PEERING_ATTACHMENT_KIND: &str = "tgw-attach";

/// See [`GLOBAL_NETWORK_KIND`]. A Connect peer hangs off a core network CONNECT attachment
/// — "Creates a core network Connect peer for a specified core network connect attachment
/// between a core network and an appliance" (`smithy_query query="doc"
/// shape="CreateConnectPeer"`) — and is a resource of its own, with an id the model
/// constrains (`ConnectPeerId` carries `pattern ^connect-peer-([0-9a-f]{8,17})$`,
/// `smithy_query shape=ConnectPeerId`), an ARN and tags. The name is the same string
/// [`CONNECT_PEER_TYPE`] holds, so one name serves the id prefix (`connect-peer-<hex>`),
/// the ARN resource type (`connect-peer/<id>`) and the `connect-peer` entry of
/// [`NETWORK_RESOURCE_TYPES`].
pub const CONNECT_PEER_KIND: &str = CONNECT_PEER_TYPE;

/// The state an attachment is in while a CHANGE to one that already exists is in flight,
/// as against the [`STATE_CREATING`] of one whose creation is.
///
/// `AttachmentState` carries both (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`), so an accepted `UpdateVpcAttachment` reports
/// `UPDATING` until the transition the clock owes falls due and [`STATE_AVAILABLE`] after
/// it. See [`AttachmentRecord::state_at`].
pub const ATTACHMENT_STATE_UPDATING: &str = "UPDATING";

/// The `AttachmentType`s this service mints, the five the model enumerates
/// (CONNECT, DIRECT_CONNECT_GATEWAY, SITE_TO_SITE_VPN, TRANSIT_GATEWAY_ROUTE_TABLE, VPC:
/// `smithy_query shape=AttachmentType`, mirrored in `ops::types::ATTACHMENT_TYPE_VALUES`).
pub const ATTACHMENT_TYPE_VPC: &str = "VPC";
/// See [`ATTACHMENT_TYPE_VPC`].
pub const ATTACHMENT_TYPE_CONNECT: &str = "CONNECT";
/// See [`ATTACHMENT_TYPE_VPC`].
pub const ATTACHMENT_TYPE_SITE_TO_SITE_VPN: &str = "SITE_TO_SITE_VPN";
/// See [`ATTACHMENT_TYPE_VPC`]. The one kind that spans SEVERAL edge locations of its
/// core network rather than sitting at one (see
/// [`AttachmentDetail::DirectConnectGateway`]).
pub const ATTACHMENT_TYPE_DIRECT_CONNECT_GATEWAY: &str = "DIRECT_CONNECT_GATEWAY";
/// See [`ATTACHMENT_TYPE_VPC`]. The one kind created over a PEERING of its core network
/// rather than over a resource reached directly (see
/// [`AttachmentDetail::TransitGatewayRouteTable`]).
pub const ATTACHMENT_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE: &str = "TRANSIT_GATEWAY_ROUTE_TABLE";

/// The tunnel protocols a Connect attachment's options may ask for, the two the model
/// enumerates (GRE, NO_ENCAP: `smithy_query shape=TunnelProtocol`, mirrored in
/// `ops::types::TUNNEL_PROTOCOL_VALUES`), and therefore the two a Connect peer can inherit
/// from the attachment it is carried over.
///
/// They are named here because the protocol decides which of a Connect peer create's
/// members apply at all ([`check_protocol_scoped_member`]).
pub const TUNNEL_PROTOCOL_GRE: &str = "GRE";
/// See [`TUNNEL_PROTOCOL_GRE`].
pub const TUNNEL_PROTOCOL_NO_ENCAP: &str = "NO_ENCAP";

/// Network Manager is a global (partition-scoped) service, so its ARNs carry an empty
/// region segment: `arn:aws:networkmanager::<account>:global-network/<id>`.
const ARN_REGION: &str = "";

/// The `ServiceCode` every modelled `ServiceQuotaExceededException` this service raises
/// carries: the model's own `arnNamespace`/signing name for Network Manager.
pub const SERVICE_CODE: &str = ARN_NAMESPACE;

/// The `LimitCode` of the two quotas this slice enforces. The model types `LimitCode` as
/// a free-form string and enumerates no values (`smithy_query
/// shape=ServiceQuotaExceededException`), so each code names its own quota.
pub const LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT: &str = "global-networks-per-account";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_SITES_PER_GLOBAL_NETWORK: &str = "sites-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_LINKS_PER_SITE: &str = "links-per-site";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_DEVICES_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_DEVICES_PER_GLOBAL_NETWORK: &str = "devices-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CORE_NETWORKS_PER_GLOBAL_NETWORK: &str = "core-networks-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CONNECTIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CONNECTIONS_PER_GLOBAL_NETWORK: &str = "connections-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`] bounds.
pub const LIMIT_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK: &str =
    "prefix-list-associations-per-core-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK: &str =
    "customer-gateway-associations-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK: &str =
    "transit-gateway-connect-peer-associations-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota [`MAX_TAGS_PER_RESOURCE`] bounds.
pub const LIMIT_TAGS_PER_RESOURCE: &str = "tags-per-resource";

/// The message every `ResourceNotFoundException` this service raises carries.
///
/// The model declares the member required but states no text. Two independent recordings
/// of different resources through different operations — `UpdateGlobalNetwork` on a
/// deleted global-network id and `TagResource` on the ARN of a deleted site — both answer
/// the identical string, so the message is a fixed, resource-INDEPENDENT constant: the
/// identifying detail travels in the shape's required `ResourceId` and `ResourceType`,
/// never in the prose. Every not-found refusal of this service is built by
/// [`resource_not_found`], so no handler can reintroduce per-request text.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The modelled `ResourceNotFoundException` (HTTP 404) for the resource `kind` and `id`
/// name: the constant [`RESOURCE_NOT_FOUND_MESSAGE`] plus the identifying detail in
/// `ResourceId` / `ResourceType`.
///
/// `kind` is one of the ARN resource types this service mints ([`GLOBAL_NETWORK_KIND`],
/// [`SITE_KIND`], [`LINK_KIND`]), which is also the `ResourceType` AWS reports.
///
/// `id` is the identifying detail the operation reports — which is NOT one kind of value
/// across this service's surface: it is the value the request addressed the resource by,
/// except where the recordings show AWS reporting the diagnostic of the lookup that failed
/// instead, which is what `ops/get_core_network.rs` passes [`FIELD_CORE_NETWORK_ID`] (the
/// NAME of the member that addressed the core network) and
/// `ops/execute_core_network_change_set.rs` passes its own fixed prose for.
pub fn resource_not_found(kind: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: id.to_string(),
        resource_type: kind.to_string(),
        context: None,
    }
}

/// The field names this service reports in `ValidationException$Fields[].Name` for the
/// reference a request carries — the parent a create hangs its new resource off, or the
/// resource a reporting operation is addressed through.
///
/// Network Manager names the offending REQUEST MEMBER in its own lowerCamelCase field
/// naming, not in the wire (PascalCase) spelling: a recording of `CreateSite` under an
/// unknown global network answers `globalNetworkId` and one of `CreateLink` with an
/// unknown site answers `siteId`. Both are `<kind>Id` for the kind the member references.
pub const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`FIELD_GLOBAL_NETWORK_ID`].
pub const FIELD_SITE_ID: &str = "siteId";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the references and values an attachment or peering create carries.
pub const FIELD_CORE_NETWORK_ID: &str = "coreNetworkId";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_TRANSPORT_ATTACHMENT_ID: &str = "transportAttachmentId";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_EDGE_LOCATION: &str = "edgeLocation";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_VPC_ARN: &str = "vpcArn";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_SUBNET_ARNS: &str = "subnetArns";
/// See [`FIELD_CORE_NETWORK_ID`]: the two lists an update of a VPC attachment carries.
pub const FIELD_ADD_SUBNET_ARNS: &str = "addSubnetArns";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_REMOVE_SUBNET_ARNS: &str = "removeSubnetArns";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_VPN_CONNECTION_ARN: &str = "vpnConnectionArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the ARN of the customer gateway a
/// global network's customer gateway association names.
pub const FIELD_CUSTOMER_GATEWAY_ARN: &str = "customerGatewayArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the ARN of the managed prefix list a
/// core network's prefix-list association names.
pub const FIELD_PREFIX_LIST_ARN: &str = "prefixListArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the several edge locations a Direct
/// Connect gateway attachment spans.
pub const FIELD_EDGE_LOCATIONS: &str = "edgeLocations";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the device and link references a connection carries.
pub const FIELD_DEVICE_ID: &str = "deviceId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_CONNECTED_DEVICE_ID: &str = "connectedDeviceId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_LINK_ID: &str = "linkId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_CONNECTED_LINK_ID: &str = "connectedLinkId";
/// See [`FIELD_CORE_NETWORK_ID`]: the same lowerCamelCase spelling of the request member,
/// for the references and values a peering and a transit gateway route table attachment
/// carry.
pub const FIELD_PEERING_ID: &str = "peeringId";
/// See [`FIELD_PEERING_ID`].
pub const FIELD_TRANSIT_GATEWAY_ARN: &str = "transitGatewayArn";
/// See [`FIELD_PEERING_ID`].
pub const FIELD_TRANSIT_GATEWAY_ROUTE_TABLE_ARN: &str = "transitGatewayRouteTableArn";
/// See [`FIELD_PEERING_ID`]: the member carrying the ARN of the transit gateway Connect
/// peer a global network's Connect peer association names.
pub const FIELD_TRANSIT_GATEWAY_CONNECT_PEER_ARN: &str = "transitGatewayConnectPeerArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the references and values a Connect peer create carries
/// — the Connect attachment the peer is carried over, the inside IP addresses it peers
/// over, and the subnet it reaches its appliance through.
pub const FIELD_CONNECT_ATTACHMENT_ID: &str = "connectAttachmentId";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`].
pub const FIELD_INSIDE_CIDR_BLOCKS: &str = "insideCidrBlocks";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`].
pub const FIELD_SUBNET_ARN: &str = "subnetArn";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`]: the two ends of the tunnel a Connect peer's BGP
/// session runs over — the appliance's address and the core network's own.
pub const FIELD_PEER_ADDRESS: &str = "peerAddress";
/// See [`FIELD_PEER_ADDRESS`].
pub const FIELD_CORE_NETWORK_ADDRESS: &str = "coreNetworkAddress";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the version id a put of a core network policy asserts the history ends at.
pub const FIELD_LATEST_VERSION_ID: &str = "latestVersionId";
/// See [`FIELD_CORE_NETWORK_ID`]: the segment a listing of one segment's routing
/// information addresses, and the next-hop filter map that listing narrows itself with.
pub const FIELD_SEGMENT_NAME: &str = "segmentName";
/// See [`FIELD_SEGMENT_NAME`].
pub const FIELD_NEXT_HOP_FILTERS: &str = "nextHopFilters";
/// See [`FIELD_SEGMENT_NAME`]: the four lists a listing of routing information matches a
/// route's BGP attributes against.
pub const FIELD_LOCAL_PREFERENCE_MATCHES: &str = "localPreferenceMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_MED_MATCHES: &str = "medMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_EXACT_AS_PATH_MATCHES: &str = "exactAsPathMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_COMMUNITY_MATCHES: &str = "communityMatches";
/// See [`FIELD_CORE_NETWORK_ID`]: the member naming the ROUTE TABLE a read of a global
/// network's routes is addressed at, and the network function group one of its forms names.
pub const FIELD_ROUTE_TABLE_IDENTIFIER: &str = "routeTableIdentifier";
/// See [`FIELD_ROUTE_TABLE_IDENTIFIER`].
pub const FIELD_NETWORK_FUNCTION_GROUP_NAME: &str = "networkFunctionGroupName";
/// See [`FIELD_ROUTE_TABLE_IDENTIFIER`]: the five lists a read of a route table matches a
/// route's prefix and prefix list against.
pub const FIELD_EXACT_CIDR_MATCHES: &str = "exactCidrMatches";
/// See [`FIELD_EXACT_CIDR_MATCHES`].
pub const FIELD_LONGEST_PREFIX_MATCHES: &str = "longestPrefixMatches";
/// See [`FIELD_EXACT_CIDR_MATCHES`].
pub const FIELD_SUBNET_OF_MATCHES: &str = "subnetOfMatches";
/// See [`FIELD_EXACT_CIDR_MATCHES`].
pub const FIELD_SUPERNET_OF_MATCHES: &str = "supernetOfMatches";
/// See [`FIELD_EXACT_CIDR_MATCHES`].
pub const FIELD_PREFIX_LIST_IDS: &str = "prefixListIds";
/// See [`FIELD_EXACT_CIDR_MATCHES`]: the filter map that read narrows itself with by what a
/// route's destinations carry, and the two enum lists it narrows itself with by what a route
/// itself carries.
pub const FIELD_DESTINATION_FILTERS: &str = "destinationFilters";
/// See [`FIELD_DESTINATION_FILTERS`].
pub const FIELD_STATES: &str = "states";
/// See [`FIELD_DESTINATION_FILTERS`].
pub const FIELD_TYPES: &str = "types";

/// How a resource kind of this service reads in prose: the ARN resource type
/// ([`GLOBAL_NETWORK_KIND`] and friends) with its hyphens as spaces and a leading
/// capital, so `global-network` reads "Global network" and `site` reads "Site".
fn kind_label(kind: &str) -> String {
    let spaced = kind.replace('-', " ");
    let mut characters = spaced.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => spaced,
    }
}

/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
///
/// Every id shape of this service is modelled `len 0..50` over `^[\s\S]*$`
/// (`smithy_query query="shape" shape="SiteId"`), so the empty string satisfies the
/// constraint walk and reaches the handler; an ARN-shaped reference member reaches it the
/// same way when its own pattern spells the empty string as an alternative of its own
/// (`^arn:…$|^$`, `smithy_query query="shape" shape="SubnetArn"`). On a member that is
/// OPTIONAL the empty string
/// is the ABSENCE of the reference — the spelling the service documents for giving no
/// value ("To remove information for any of the parameters, specify an empty string",
/// `smithy_query query="doc" shape="UpdateDevice"`) — and not a reference to a resource
/// whose id is the empty string, which nothing can ever be stored under. So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is: that keeps every stored record's references either naming a resource of the
/// same global network or naming nothing at all.
///
/// A REQUIRED reference is not passed through here: it must name a resource, so the empty
/// string stays the unresolvable reference its own refusal reports.
///
/// A listing that SELECTS on an optional reference member reads it through here too, so one
/// member has one reading whether a request writes it or filters by it: the empty string
/// names no resource, so it narrows the listing by nothing and the listing answers exactly
/// what the omitted member asks for. Two kinds of member are NOT read through here: a
/// DESCRIPTIVE one — a link's type or provider, a Region, a state — is not a reference and
/// its writers store the empty string verbatim, so a record can carry it and a filter naming
/// it keeps matching it literally; and one whose modelled shape does not admit the empty
/// string at all (an id pattern, a minimum length) never reaches a handler empty, because the
/// constraint walk refuses it first.
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}

/// What an UPDATE writes for an OPTIONAL reference member: the outer `Option` says whether
/// the request spoke about the member at all, the inner one what the member names.
///
/// `None` is the member the request does not carry, which leaves the stored value where it
/// is. `Some(inner)` is a member the request supplies, and `inner` is what it names under
/// the one reading this service gives these members ([`optional_reference`]): `Some(id)` for
/// an id to resolve, `None` for the empty string, which names no resource and is therefore
/// written as none rather than stored verbatim. So an update that empties a reference leaves
/// the record in the state a create with the same spelling leaves it in, and no stored
/// reference ever names an id nothing can be stored under.
pub fn requested_reference(supplied: Option<&str>) -> Option<Option<&str>> {
    supplied.map(|id| optional_reference(Some(id)))
}

/// The modelled `ValidationException` (HTTP 400) an operation answers when a reference
/// its input carries — the parent a new resource must hang off, or the resource a
/// reporting operation is addressed through — names no stored resource.
///
/// An operation that STORES A NEW RECORD — a resource of its own, a job, or a tie that has
/// no id (an association, a registration) — validates the references its input carries
/// before anything is stored, so a reference that resolves to nothing is a FIELD failure on
/// the member that carried it rather than a missing
/// resource: `Reason` is `FieldValidationFailed` and `Fields` carries one entry naming
/// that member (see [`FIELD_GLOBAL_NETWORK_ID`]). The prose is the one the recordings
/// show — "<Kind> not found." for the kind the member references, from [`kind_label`] —
/// and is repeated as the field's own message, exactly as recorded.
///
/// It is EVERY reference the input carries that is judged this way, not only the parent the
/// record hangs off: the device and link a record is tied to are references of the request
/// too, and a recorded `AssociateLink` naming a link that was deleted answers this same 400
/// with "Link not found.", exactly as the recorded `CreateConnection` refusals of an
/// unresolvable `deviceId` / `linkId` do.
///
/// This is the crate's SINGLE construction for an unresolvable reference refused as a
/// validation failure, so one service reports one field name and one message for the same
/// member whichever operation refused it. Which errors the operation MODELS does not decide
/// the class: the recorded refusals of a create and of an association under a global network
/// the store does not hold are both this 400 on operations that model the 404 as well. An
/// operation addressed AT a resource that already exists — a read, an update, a delete, a
/// disassociation — answers the absent resource with that 404 instead (see
/// [`resource_not_found`]), and so does the resource the request is addressed at even where
/// its id travels in the body.
///
/// `member` is the field name the refusal reports; `kind` is the resource kind the
/// member references ([`GLOBAL_NETWORK_KIND`], [`SITE_KIND`], [`CORE_NETWORK_KIND`]).
pub fn unknown_reference(member: &str, kind: &str) -> ValidationException {
    let message = format!("{} not found.", kind_label(kind));
    ValidationException {
        message: message.clone(),
        reason: Some("FieldValidationFailed".to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// How a resource kind of this service reads where AWS names it as the SUBJECT of a
/// refusal: every word of the ARN resource type capitalised, so `global-network` reads
/// "Global Network" and `core-network` reads "Core Network".
fn kind_title(kind: &str) -> String {
    kind.split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// The `Reason` a refusal carries when the request's own fields all parse and satisfy
/// their constraints, yet the request is invalid against stored state — the modelled
/// catch-all of `ValidationExceptionReason` (UnknownOperation, CannotParse,
/// FieldValidationFailed, Other: `smithy_query shape=ValidationExceptionReason`).
pub const VALIDATION_REASON_OTHER: &str = "Other";

/// The modelled `ValidationException` (HTTP 400) a DELETE answers while the resource it
/// names still owns dependent resources the caller has to remove first, or `None` when
/// nothing blocks the delete (every count is zero).
///
/// Network Manager documents the emptying order each delete requires ("You must first
/// delete all global network objects (devices, links, and sites), deregister all transit
/// gateways, and delete any core networks", `smithy_query query="doc"
/// shape="DeleteGlobalNetwork"`; "The site cannot be associated with any device or link",
/// `shape="DeleteSite"`) and refuses the delete until it holds. That refusal is a
/// VALIDATION failure of the request against the stored resource tree — HTTP 400,
/// `ValidationException` — and NOT a `ConflictException` (409): a dependency refusal
/// stands until the caller deletes the children, so an SDK waiter or retry loop must see a
/// terminal 400, while 409 is this service's concurrent-state conflict (an idempotency
/// token reused for a different request, a resource being modified) which a retry can
/// clear. Recorded behaviour confirms it for the core-network dependent: the delete of a
/// global network that still owns one answers HTTP 400 with code `ValidationException`,
/// and succeeds once the core network is gone.
///
/// The prose names the blocking kinds the same way the recorded message does — the parent
/// in [`kind_title`], the dependents as their ARN resource types read as words, which the
/// recorded refusal of a global network that still owns a core network spells exactly
/// ("Global Network cannot be deleted due to an existing core network associated with it.")
/// — and the identifying detail stays out of it: no `Fields` entry, because no input field
/// is at fault, and `Reason` is [`VALIDATION_REASON_OTHER`].
///
/// This is the refusal of a delete addressed at a CONTAINER: the global network itself, a
/// core network, an attachment, a peering, a transit gateway registration. A delete
/// addressed at one of the global network's own OBJECTS answers the other recorded template
/// instead ([`object_dependents_block_delete`]).
///
/// `parent_kind` is the kind being deleted ([`GLOBAL_NETWORK_KIND`], [`CORE_NETWORK_KIND`],
/// …) and `dependents` pairs each child kind with how many of it the parent still holds, in
/// the order the message should list them; a zero count never blocks and is never named.
pub fn dependents_block_delete(
    parent_kind: &str,
    dependents: &[(&str, usize)],
) -> Option<ValidationException> {
    let blocking: Vec<String> = dependents
        .iter()
        .filter(|(_, held)| *held > 0)
        .map(|(kind, _)| kind.replace('-', " "))
        .collect();
    let listed = match blocking.split_last() {
        None => return None,
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    };
    Some(ValidationException {
        message: format!(
            "{} cannot be deleted due to an existing {listed} associated with it.",
            kind_title(parent_kind)
        ),
        reason: Some(VALIDATION_REASON_OTHER.to_string()),
        fields: None,
    })
}

/// The same refusal for a delete addressed at one of a global network's OWN OBJECTS — a
/// device, a link or a site, the three kinds `DeleteGlobalNetwork`'s documentation groups as
/// "all global network objects (devices, links, and sites)" — which carries the other
/// recorded template, or `None` when nothing blocks the delete (every count is zero).
///
/// Here the prose names the OBJECT and not the dependent: its kind in [`kind_title`], the id
/// the request addressed it with, and then the fixed statement that an existing association
/// blocks the delete — which is what the recorded refusals of a site that still holds a
/// device ("Site with ID <SiteId> cannot be deleted due to existing association.") and of a
/// link that is still associated carry. The envelope is the one
/// [`dependents_block_delete`] builds: HTTP 400 `ValidationException`, no `Fields` entry
/// because no input field is at fault, and [`VALIDATION_REASON_OTHER`].
///
/// `object_kind` is the kind being deleted ([`DEVICE_KIND`], [`LINK_KIND`], [`SITE_KIND`]),
/// `object_id` the id that object was addressed by, and `dependents` pairs each referring
/// kind with how many records of it still name the object — the kinds are what the counts
/// belong to at the call site; this template names none of them.
pub fn object_dependents_block_delete(
    object_kind: &str,
    object_id: &str,
    dependents: &[(&str, usize)],
) -> Option<ValidationException> {
    if dependents.iter().all(|(_, held)| *held == 0) {
        return None;
    }
    Some(ValidationException {
        message: format!(
            "{} with ID {object_id} cannot be deleted due to existing association.",
            kind_title(object_kind)
        ),
        reason: Some(VALIDATION_REASON_OTHER.to_string()),
        fields: None,
    })
}

/// Every value a `ResourceType` member of this service's network-resource operations
/// accepts, in lexicographic order so a paginated listing of them is stable.
///
/// The set is the enumeration in the member's own documentation (`smithy_query
/// query="doc" shape="GetNetworkResourceCountsRequest" member="ResourceType"`): three
/// Direct Connect types (`dxcon`, `dx-gateway`, `dx-vif`), eight Network Manager types
/// (`attachment`, `connect-peer`, `connection`, `core-network`, `device`, `link`,
/// `peering`, `site`) and six Amazon VPC types (`customer-gateway`, `transit-gateway`,
/// `transit-gateway-attachment`, `transit-gateway-connect-peer`,
/// `transit-gateway-route-table`, `vpn-connection`). The modelled `ConstrainedString`
/// carries only a length and a permissive pattern, so a value outside this set is refused
/// here rather than by the schema.
pub const NETWORK_RESOURCE_TYPES: [&str; 17] = [
    ATTACHMENT_KIND,
    "connect-peer",
    "connection",
    "core-network",
    CUSTOMER_GATEWAY_KIND,
    "device",
    "dx-gateway",
    "dx-vif",
    "dxcon",
    "link",
    PEERING_KIND,
    "site",
    TRANSIT_GATEWAY_TYPE,
    TRANSIT_GATEWAY_ATTACHMENT_TYPE,
    "transit-gateway-connect-peer",
    TRANSIT_GATEWAY_ROUTE_TABLE_TYPE,
    "vpn-connection",
];

/// The network-resource type of a TRANSIT GATEWAY, and of one of its route tables: Amazon
/// VPC resources, which is also the resource type their ARNs carry
/// (`arn:<partition>:ec2:<region>:<account>:transit-gateway/tgw-<hex>`) and what
/// [`ec2_resource_region`] is asked for when a request names one.
pub const TRANSIT_GATEWAY_TYPE: &str = "transit-gateway";
/// See [`TRANSIT_GATEWAY_TYPE`].
pub const TRANSIT_GATEWAY_ROUTE_TABLE_TYPE: &str = "transit-gateway-route-table";

/// The network-resource type of an attachment on a TRANSIT GATEWAY registered with a
/// global network — an Amazon VPC resource, unlike the Cloud WAN attachments this service
/// mints itself ([`ATTACHMENT_KIND`]).
///
/// One of [`NETWORK_RESOURCE_TYPES`], and the only kind a ROUTE ANALYSIS can run a routing
/// path through: the Route Analyzer analyses the path between two transit gateway
/// attachments ([`RouteAnalysisEndpointOptions`]), so resolving an endpoint means finding
/// the global network's network resource of this type with the ARN the endpoint names (see
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::analyze_route_path`]).
pub const TRANSIT_GATEWAY_ATTACHMENT_TYPE: &str = "transit-gateway-attachment";

/// Whether `resource_type` is one of the values [`NETWORK_RESOURCE_TYPES`] lists.
pub fn is_network_resource_type(resource_type: &str) -> bool {
    NETWORK_RESOURCE_TYPES.contains(&resource_type)
}

/// The three resource types TELEMETRY is reported for, each also one of
/// [`NETWORK_RESOURCE_TYPES`].
///
/// Telemetry covers a far narrower set than the network-resource inventory does: only the
/// CONNECTIONS whose session the service measures. The set is the enumeration in the
/// member's own documentation (`smithy_query query="doc"
/// shape="GetNetworkTelemetryRequest" member="ResourceType"`: `connect-peer`,
/// `transit-gateway-connect-peer`, `vpn-connection`) — a Cloud WAN Connect peer, a transit
/// gateway Connect peer, and a Site-to-Site VPN connection. The modelled
/// `ConstrainedString` carries only a length and a permissive pattern, so a value outside
/// this set is refused by the handler rather than by the schema.
///
/// [`CONNECT_PEER_TYPE`] doubles as the id prefix and the ARN resource type of the Cloud
/// WAN Connect PEER this slice mints, the way [`GLOBAL_NETWORK_KIND`] does for a global
/// network: the model constrains that id itself with `pattern
/// ^connect-peer-([0-9a-f]{8,17})$` (`smithy_query shape=ConnectPeerId`), which the 17 hex
/// digits of [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const CONNECT_PEER_TYPE: &str = "connect-peer";
/// See [`CONNECT_PEER_TYPE`].
pub const TRANSIT_GATEWAY_CONNECT_PEER_TYPE: &str = "transit-gateway-connect-peer";
/// See [`CONNECT_PEER_TYPE`].
pub const VPN_CONNECTION_TYPE: &str = "vpn-connection";

/// The telemetry resource types ([`CONNECT_PEER_TYPE`] and friends) in lexicographic
/// order, so anything that walks them is stable.
pub const TELEMETRY_RESOURCE_TYPES: [&str; 3] = [
    CONNECT_PEER_TYPE,
    TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
    VPN_CONNECTION_TYPE,
];

/// Whether `resource_type` is one of the values [`TELEMETRY_RESOURCE_TYPES`] lists.
pub fn is_telemetry_resource_type(resource_type: &str) -> bool {
    TELEMETRY_RESOURCE_TYPES.contains(&resource_type)
}

/// The input member every operation that selects by resource type names in a refusal of the
/// value it carried: `ResourceType`, as the model spells the member.
pub const FIELD_RESOURCE_TYPE: &str = "ResourceType";

/// The modelled `ValidationException` (HTTP 400) for a `ResourceType` outside the set the
/// operation reports on.
///
/// The prose is the recorded one: the fixed statement that the member's value is invalid,
/// naming ONLY the member. The offending value is not echoed back and the set of accepted
/// values is not enumerated — a caller reads the accepted set from the member's own
/// documentation, not from a refusal — so the message is the same whichever value and
/// whichever of the two sets ([`NETWORK_RESOURCE_TYPES`], [`TELEMETRY_RESOURCE_TYPES`])
/// failed it.
///
/// The ONE construction of that refusal for this service, reached through the two checks
/// below: an operation's own predicate decides which set applies, but the refusal — its
/// `Reason`, its `Message` and the single `Fields` entry [`field_rejection`] gives the
/// member — is built here, so one rule on one member cannot answer two different bodies
/// depending on which operation refused it.
fn unsupported_resource_type() -> ValidationException {
    field_rejection(
        FIELD_RESOURCE_TYPE,
        REASON_FIELD_VALIDATION_FAILED,
        format!("{FIELD_RESOURCE_TYPE} is invalid"),
    )
}

/// Check one `ResourceType` member against the network-resource inventory's own set.
///
/// Answers `Ok(())` when the member is absent (no filter: every type is reported) or carries
/// one of [`NETWORK_RESOURCE_TYPES`], otherwise the modelled `ValidationException`
/// (HTTP 400) naming the member — a value outside that set names no resource kind this
/// service holds, so it is refused on the member rather than answered with an empty page or
/// a zero count. Shared by every operation that reports out of that inventory, so the set,
/// the predicate and the refusal cannot drift apart between them.
pub fn check_network_resource_type(resource_type: Option<&str>) -> Result<(), ValidationException> {
    match resource_type {
        Some(resource_type) if !is_network_resource_type(resource_type) => {
            Err(unsupported_resource_type())
        }
        _ => Ok(()),
    }
}

/// The sibling of [`check_network_resource_type`] for the narrower set TELEMETRY is reported
/// for ([`TELEMETRY_RESOURCE_TYPES`]): the same rule on the same member over the set that
/// operation covers, refused through the same construction.
pub fn check_telemetry_resource_type(
    resource_type: Option<&str>,
) -> Result<(), ValidationException> {
    match resource_type {
        Some(resource_type) if !is_telemetry_resource_type(resource_type) => {
            Err(unsupported_resource_type())
        }
        _ => Ok(()),
    }
}

/// The two values `ConnectionHealth$Type` carries (`smithy_query shape=ConnectionType`,
/// mirrored in `ops::types::CONNECTION_TYPE_VALUES`: BGP, IPSEC) — the PROTOCOL whose
/// session the reported health belongs to, which is a function of the resource kind: a
/// Connect peer of either kind peers over BGP (its configuration is a list of
/// `BgpConfigurations`, `smithy_query shape=ConnectPeerConfiguration`), while a
/// Site-to-Site VPN connection is an IPsec tunnel.
pub const CONNECTION_TYPE_BGP: &str = "BGP";
/// See [`CONNECTION_TYPE_BGP`].
pub const CONNECTION_TYPE_IPSEC: &str = "IPSEC";

/// The two values `ConnectionHealth$Status` carries (`smithy_query shape=ConnectionStatus`,
/// mirrored in `ops::types::CONNECTION_STATUS_VALUES`: DOWN, UP).
pub const CONNECTION_STATUS_UP: &str = "UP";
/// See [`CONNECTION_STATUS_UP`].
pub const CONNECTION_STATUS_DOWN: &str = "DOWN";

/// The tag-key prefix AWS reserves for its own tags.
///
/// "Do not use the `aws:` prefix in your tag names or values because it is reserved for
/// AWS use. You can't edit or delete tag names or values with this prefix"
/// (AWS tagging documentation), so a request that asks to SET or REMOVE such a key is
/// refused and nothing is stored. The comparison is case-insensitive, as AWS's own
/// reserved-prefix check is.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The two IP address families, spelled the way this service's documentation spells them
/// ("must be the same IP address family (IPv4 or IPv6)", `smithy_query query="doc"
/// shape="CreateConnectPeer"`). The model declares no enumeration of them, so these are the
/// words a refusal names a family by.
pub const IP_FAMILY_V4: &str = "IPv4";
/// See [`IP_FAMILY_V4`].
pub const IP_FAMILY_V6: &str = "IPv6";

/// Which IP address family `address` is in, or `None` when the value is no IP address at
/// all.
///
/// Every address member of this service targets `IPAddress`, whose modelled constraints are
/// a length and the permissive `^[\s\S]*$` (`smithy_query shape=IPAddress`), so the
/// pre-handler constraint walk cannot tell an address from any other string and what a
/// value IS has to be read here. The reading is the textual form of an IPv4 or an IPv6
/// address and nothing else: no port, no prefix length, no surrounding space.
pub fn ip_address_family(address: &str) -> Option<&'static str> {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Some(IP_FAMILY_V4),
        Ok(IpAddr::V6(_)) => Some(IP_FAMILY_V6),
        Err(_) => None,
    }
}

/// Check the PAIR of tunnel addresses a Connect peer create carries: each has to be an IP
/// address, and the two ends of one peering have to be in the same address family — "The
/// peer address and transit gateway address must be the same IP address family (IPv4 or
/// IPv6)" (`smithy_query query="doc" shape="CreateConnectPeer"`), the core network's end
/// being the transit gateway address that sentence names (`ConnectPeerConfiguration`
/// reports it as `CoreNetworkAddress`, "The IP address of a core network").
///
/// A BGP session runs between two addresses of ONE family, so a pair that straddles the two
/// describes a peering that cannot exist and is refused before anything is stored. The value
/// that is at fault is a MEMBER's, so the refusal is a field failure
/// ([`REASON_FIELD_VALIDATION_FAILED`]) naming the member that carried it; a mismatch is
/// blamed on the core network's end, since the appliance's end is the required member every
/// peering has and the core network's end is the one that has to agree with it.
///
/// `core_network_address` is `None` for a request naming no core network address at all,
/// which carries no second family and so can never straddle one.
pub fn check_tunnel_addresses(
    peer_member: &str,
    peer_address: &str,
    core_member: &str,
    core_network_address: Option<&str>,
) -> Result<(), ValidationException> {
    let not_an_address = |member: &str, value: &str| {
        field_rejection(
            member,
            REASON_FIELD_VALIDATION_FAILED,
            format!("{value} is not an {IP_FAMILY_V4} or {IP_FAMILY_V6} address"),
        )
    };
    let peer_family =
        ip_address_family(peer_address).ok_or_else(|| not_an_address(peer_member, peer_address))?;
    let Some(core_address) = core_network_address else {
        return Ok(());
    };
    let core_family = ip_address_family(core_address)
        .ok_or_else(|| not_an_address(core_member, core_address))?;
    if core_family != peer_family {
        return Err(field_rejection(
            core_member,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "core network address {core_address} is {core_family} and peer address \
                 {peer_address} is {peer_family}; the peer address and the core network \
                 address must be the same IP address family ({IP_FAMILY_V4} or \
                 {IP_FAMILY_V6})"
            ),
        ));
    }
    Ok(())
}

/// Check a request member the model scopes to ONE tunnel protocol against the protocol the
/// peering actually runs.
///
/// A Connect peer takes its protocol from the attachment it is carried over, and the model
/// scopes two of the create's members to a single protocol each: "This only applies only
/// when the protocol is `GRE`" on `CoreNetworkAddress` and "...when the protocol is
/// NO_ENCAP" on `SubnetArn` (`smithy_query query="doc" shape="CreateConnectPeerRequest"
/// member="CoreNetworkAddress"`, `member="SubnetArn"`). A member supplied against the OTHER
/// protocol describes configuration that peering has no place for — a tunnel address on a
/// peering that builds no tunnel, a subnet on one that reaches its appliance through a
/// tunnel — so it is refused with the modelled `ValidationException` (HTTP 400) naming the
/// member that carried it and nothing is stored. That is what keeps a stored peer from
/// contradicting the `Protocol` its own configuration reports.
///
/// The refusal is against STORED state rather than against the value — the member parses
/// and satisfies its constraints, and only the attachment's protocol makes it inapplicable
/// — so it carries [`VALIDATION_REASON_OTHER`], and it is judged after the member's own
/// value has been.
///
/// `supplied` says whether the request named the member at all (a member whose spelling
/// names nothing, like the empty `SubnetArn`, is not supplied); `applies_to` is the protocol
/// the model scopes it to; `protocol` is the attachment's, which is `None` where its options
/// named none — then no stated protocol excludes anything and the member stands.
pub fn check_protocol_scoped_member(
    member: &str,
    supplied: bool,
    applies_to: &str,
    connect_attachment_id: &str,
    protocol: Option<&str>,
) -> Result<(), ValidationException> {
    match protocol {
        Some(running) if supplied && running != applies_to => Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "{member} only applies when the protocol is {applies_to}, but Connect \
                 attachment {connect_attachment_id} carries protocol {running}"
            ),
        )),
        _ => Ok(()),
    }
}


///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`]: every `StartRouteAnalysis` records
/// one more analysis a later `GetRouteAnalysis` can read back, and neither the model nor
/// the published quotas state a number, so this bound exists only so that repeated calls
/// cannot grow the store without end. `StartRouteAnalysis` models no quota error
/// (`smithy_query query="operation" operation="StartRouteAnalysis"`), so a start past this
/// cap is refused with its modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] and records nothing.
pub const MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK: usize = 100;

/// How long after the creating call a `PENDING` resource settles to `AVAILABLE`.
///
/// AWS documents no span for this transition, so the value is CHOSEN; the emulator
/// contract fixes a chosen settle span inside 250–750 ms and forbids zero, so a caller
/// that reads straight after the create still observes the transient `PENDING` state.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The `PolicyVersionId` the first version of a core network's policy takes — the one a
/// create's document becomes ([`CoreNetworkRecord::add_policy_version`] mints from
/// [`CoreNetworkRecord::policy_version_counter`], which starts at zero).
pub const FIRST_POLICY_VERSION: i32 = 1;

/// How many policy versions one core network may hold.
///
/// A CHOSEN bound: a policy version is immutable ("Creates a new, immutable version of a
/// core network policy", `smithy_query query="doc" shape="PutCoreNetworkPolicy"`), so a
/// put never replaces one and the history would otherwise grow without end, and neither
/// the model nor the published quotas state a number. `PutCoreNetworkPolicy` models no
/// quota error at all (`smithy_query operation=PutCoreNetworkPolicy`), so a put past this
/// cap is refused with its modelled `ValidationException` (HTTP 400,
/// [`VALIDATION_REASON_OTHER`]) and stores nothing; deleting a version frees room for
/// another put, though never its version id ([`CoreNetworkRecord::policy_version_counter`]).
pub const MAX_POLICY_VERSIONS_PER_CORE_NETWORK: usize = 100;

/// The members of `CoreNetworkPolicyAlias` (`smithy_query shape=CoreNetworkPolicyAlias`:
/// LIVE, LATEST — mirrored in `ops::types::CORE_NETWORK_POLICY_ALIAS_VALUES`): "Whether a
/// core network policy is the current LIVE policy or the most recently submitted policy".
pub const POLICY_ALIAS_LIVE: &str = "LIVE";
/// See [`POLICY_ALIAS_LIVE`].
pub const POLICY_ALIAS_LATEST: &str = "LATEST";

/// The members of `ChangeSetState` a stored policy version reports (`smithy_query
/// shape=ChangeSetState`: PENDING_GENERATION, FAILED_GENERATION, READY_TO_EXECUTE,
/// EXECUTING, EXECUTION_SUCCEEDED, OUT_OF_DATE — mirrored in
/// `ops::types::CHANGE_SET_STATE_VALUES`).
///
/// A version whose change set has not been executed waits in
/// [`CHANGE_SET_PENDING_GENERATION`] until the span the clock owes elapses and then reads
/// [`CHANGE_SET_READY_TO_EXECUTE`]; one whose document is being put in force reads
/// [`CHANGE_SET_EXECUTING`] and then [`CHANGE_SET_EXECUTION_SUCCEEDED`]
/// ([`CoreNetworkPolicyRecord::change_set_state_at`]). An unexecuted version whose change
/// set was generated against a document that is no longer in force — because another
/// version has been put in force since — reads [`CHANGE_SET_OUT_OF_DATE`] instead and can
/// no longer be executed ([`CoreNetworkRecord::execute_policy_version`]), since the
/// differences it describes are no longer the ones executing it would apply.
/// `FAILED_GENERATION` is unreachable here because a document that does not pass
/// [`validate_policy_document`] is refused synchronously with `CoreNetworkPolicyException`
/// and never stored.
pub const CHANGE_SET_PENDING_GENERATION: &str = "PENDING_GENERATION";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_READY_TO_EXECUTE: &str = "READY_TO_EXECUTE";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_EXECUTING: &str = "EXECUTING";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_EXECUTION_SUCCEEDED: &str = "EXECUTION_SUCCEEDED";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_OUT_OF_DATE: &str = "OUT_OF_DATE";

/// The members of `ChangeStatus` one change of a stored policy version's change set
/// reports (`smithy_query shape=ChangeStatus`: NOT_STARTED, IN_PROGRESS, COMPLETE, FAILED
/// — mirrored in `ops::types::CHANGE_STATUS_VALUES`).
///
/// A change waits in [`CHANGE_STATUS_NOT_STARTED`] while its version's change set has not
/// been executed, reads [`CHANGE_STATUS_IN_PROGRESS`] while that execution is under way
/// and [`CHANGE_STATUS_COMPLETE`] once it has succeeded — the same clock the version's own
/// `ChangeSetState` follows ([`CoreNetworkPolicyRecord::change_set_state_at`]). `FAILED` is
/// unreachable here because a document that does not pass [`validate_policy_document`] is
/// refused synchronously and never stored, so no stored change set can fail.
pub const CHANGE_STATUS_NOT_STARTED: &str = "NOT_STARTED";
/// See [`CHANGE_STATUS_NOT_STARTED`].
pub const CHANGE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`CHANGE_STATUS_NOT_STARTED`].
pub const CHANGE_STATUS_COMPLETE: &str = "COMPLETE";

/// The members of `ChangeAction` one change reports (`smithy_query shape=ChangeAction`:
/// ADD, MODIFY, REMOVE — mirrored in `ops::types::CHANGE_ACTION_VALUES`): what the change
/// does to the object it names, which follows from which of the two compared policies
/// configures that object ([`policy_changes`]).
pub const CHANGE_ACTION_ADD: &str = "ADD";
/// See [`CHANGE_ACTION_ADD`].
pub const CHANGE_ACTION_MODIFY: &str = "MODIFY";
/// See [`CHANGE_ACTION_ADD`].
pub const CHANGE_ACTION_REMOVE: &str = "REMOVE";

/// The members of `ChangeType` the difference between two policy documents produces
/// (`smithy_query shape=ChangeType`, mirrored in `ops::types::CHANGE_TYPE_VALUES`): the
/// kinds of object a policy document configures and [`policy_topology`] therefore derives
/// — an edge, a segment at one edge location, a network function group at one edge
/// location, one static route a segment holds at one edge location, and the
/// `attachment-policies` block as a whole.
///
/// The members this list leaves out are the ones no difference between two STORED documents
/// can be seen in here. `ATTACHMENT_MAPPING`, the route-propagation kind and the
/// routing-policy kinds describe objects that are not part of a document at all. The
/// remaining configuration kinds (`CORE_NETWORK_CONFIGURATION`, `SEGMENTS_CONFIGURATION`,
/// `SEGMENT_ACTIONS_CONFIGURATION`) name blocks a document does carry, but every object
/// those blocks configure is reported under the kind that names that object — the edge, the
/// segment with what the `segment-actions` acting on it declare, the group, the static route
/// — so reporting the block as well would report one difference twice.
/// `attachment-policies` is the one block reported as a block, because it configures no
/// object of its own ([`policy_changes`]).
pub const CHANGE_TYPE_CORE_NETWORK_EDGE: &str = "CORE_NETWORK_EDGE";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_CORE_NETWORK_SEGMENT: &str = "CORE_NETWORK_SEGMENT";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_NETWORK_FUNCTION_GROUP: &str = "NETWORK_FUNCTION_GROUP";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`]: the kind of ONE static route a `create-route`
/// entry of `segment-actions` declares for a segment, which is reached through the
/// attachment its destination names.
pub const CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC: &str = "ATTACHMENT_ROUTE_STATIC";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_ATTACHMENT_POLICIES_CONFIGURATION: &str =
    "ATTACHMENT_POLICIES_CONFIGURATION";

/// How this service names ONE policy version of a core network in the required
/// `ResourceId` of a modelled refusal: `<CoreNetworkId>/<PolicyVersionId>`.
///
/// A version id is an `Integer` that is unique only inside its core network, so neither
/// half identifies the addressed policy on its own.
pub fn policy_version_resource_id(core_network_id: &str, version_id: i32) -> String {
    format!("{core_network_id}/{version_id}")
}

/// How this service names the policy a request addressed by ALIAS rather than by version
/// id: `<CoreNetworkId>/<Alias>`.
///
/// A request that selects a policy by alias names no version id, and a core network may
/// hold no version for that alias to resolve to, so this keeps a refusal's `ResourceId`
/// reading `<CoreNetworkId>/<selector>` whichever selector the request used, beside the
/// same `ResourceType` ([`CORE_NETWORK_POLICY_KIND`]).
pub fn policy_alias_resource_id(core_network_id: &str, alias: &str) -> String {
    format!("{core_network_id}/{alias}")
}

/// How a paginated listing of a core network's policy versions keys ONE version.
///
/// A version id is an `Integer` (`smithy_query shape=CoreNetworkPolicyVersion
/// member=PolicyVersionId`) while [`paginate`] resumes a page after a STRING key, so the
/// id is zero-padded to the widest a positive one can be: that keeps the lexicographic
/// order the page token is compared in the numeric order of the history, so version 10
/// never sorts before version 2.
pub fn policy_version_page_key(version_id: i32) -> String {
    format!("{version_id:010}")
}

/// A stored association between a core network and a managed prefix list: the tie
/// `CreateCoreNetworkPrefixListAssociation` makes "for routing control" (`smithy_query
/// query="doc" shape="CreateCoreNetworkPrefixListAssociation"`).
///
/// Like [`LinkAssociationRecord`] this is NOT a resource of its own: the modelled
/// `PrefixListAssociation` is the core network id, the prefix list ARN and the alias
/// (`smithy_query shape=PrefixListAssociation`), with no id, ARN or state member, so the
/// core network / prefix list pair is its identity and the store keys it by
/// [`prefix_list_association_key`]. Carrying no state member, it has nothing that is a
/// function of the clock and no transition is owed on it.
#[derive(Debug, Clone)]
pub struct PrefixListAssociationRecord {
    /// The core network the association was made on: the one whose prefix-list
    /// associations list it and whose delete removes it.
    pub core_network_id: String,
    /// The ARN of the associated prefix list, exactly as accepted. A managed prefix list is
    /// an Amazon VPC resource, so this service mints nothing for it and stores no record of
    /// the list itself — [`parse_prefix_list_arn`] judges only the form of the ARN.
    pub prefix_list_arn: String,
    /// The alias the create carried: "An optional alias for the prefix list association"
    /// (`smithy_query query="doc" shape="CreateCoreNetworkPrefixListAssociationRequest"
    /// member="PrefixListAlias"`). A required member whose modelled length starts at zero,
    /// so an empty alias is one a caller may ask for and is stored as asked.
    pub prefix_list_alias: String,
    /// The `@idempotencyToken` the create carried, when it carried a non-empty one, so a
    /// retry of the same request answers this association instead of storing a second one.
    pub client_token: Option<String>,
}

/// The key the association store holds a core network / prefix list pair under: the core
/// network id, then the ARN of the prefix list, so the map orders associations by core
/// network and then by prefix list and a pagination token can name an offset into it.
///
/// A core network id is `core-network-<17 hex>`
/// ([`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`], which the modelled
/// `CoreNetworkId` pattern constrains) and carries no `/`, so no two pairs can produce the
/// same key. The same string identifies the association in a not-found refusal, where no
/// single resource id would.
pub fn prefix_list_association_key(core_network_id: &str, prefix_list_arn: &str) -> String {
    format!("{core_network_id}/{prefix_list_arn}")
}

/// How many subnet ARNs one VPC attachment may hold.
///
/// Cloud WAN attaches a VPC through its subnets, one per Availability Zone, and no Region
/// offers more than a handful, so — like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`] — this bound
/// is CHOSEN: the model carries no number and neither the create nor the update of a VPC
/// attachment models a quota error (`smithy_query query="operation"
/// operation="CreateVpcAttachment,UpdateVpcAttachment"`), so its only purpose is that no
/// caller can grow one attachment's subnet list without end. A request that would push the
/// list past it is refused with the modelled `ValidationException` naming the member that
/// asked, and stores nothing.
pub const MAX_SUBNETS_PER_VPC_ATTACHMENT: usize = 50;

/// Refuse a subnet list that would leave a VPC attachment holding more than `limit`
/// subnets.
///
/// `length` is how many the attachment would be left holding, so this is called on the
/// list the attachment WOULD hold (a create's `SubnetArns`, the list an update composes) —
/// never on a removal, which cannot grow it. `limit` is
/// [`MAX_SUBNETS_PER_VPC_ATTACHMENT`], except where an attachment already holds more than
/// that: an update may then still change it, but never grow it (see
/// `UpdateVpcAttachment`), so the limit applied is the one the message reports.
pub fn check_subnet_count(
    member: &str,
    length: usize,
    limit: usize,
) -> Result<(), ValidationException> {
    if length > limit {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!("a VPC attachment holds at most {limit} subnets, but the request would leave {length}"),
        ));
    }
    Ok(())
}

/// Check a subnet-ARN list a request carries for a VPC attachment.
///
/// The LENGTH is judged FIRST, before any walk of the list: `SubnetArnList` carries no
/// modelled `@length` (`smithy_query shape=SubnetArnList` constrains the member string
/// only), so a caller chooses how long it is, and no per-entry work is done on a list that
/// is already too long to be accepted. `limit` is [`MAX_SUBNETS_PER_VPC_ATTACHMENT`],
/// except where an attachment already holds more than that: an update may then still
/// change it, but never grow it (see `UpdateVpcAttachment`).
///
/// Then every entry must name a subnet the VPC could hold — one in the VPC's own Region,
/// which is the attachment's edge location — and no entry may repeat, since a subnet is
/// attached once or not at all. The modelled `SubnetArn` admits the EMPTY string as well
/// (`^arn:…:subnet\/subnet-[0-9a-f]{8,17}$|^$`, `smithy_query shape=SubnetArn`), which
/// names no subnet and so names no Region; it is refused by the same check. Membership is
/// tested through a set built once, so a list at the bound costs O(n log n).
///
/// What the attachment would be LEFT holding is the separate rule
/// [`check_subnet_count`]: this one judges the list as the request carried it.
///
/// Shared by `CreateVpcAttachment` and `UpdateVpcAttachment`, so ONE function owns the
/// rule and the two cannot judge a subnet list differently; each passes the member of its
/// own request that carried the list.
pub fn check_subnet_list(
    member: &str,
    edge_location: &str,
    requested: &[String],
    limit: usize,
) -> Result<(), ValidationException> {
    if requested.len() > limit {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "at most {limit} subnet ARNs may be named, but {} were",
                requested.len()
            ),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for subnet_arn in requested {
        if arn_region(subnet_arn) != Some(edge_location) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!(
                    "subnet {subnet_arn} is not in {edge_location}, the Region of the VPC it must belong to"
                ),
            ));
        }
        if !seen.insert(subnet_arn.as_str()) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!("subnet {subnet_arn} is named more than once"),
            ));
        }
    }
    Ok(())
}

/// How many edge locations one Direct Connect gateway attachment may span.
///
/// CHOSEN, like [`MAX_SUBNETS_PER_VPC_ATTACHMENT`] and for the same reason: the model
/// carries no number — `ExternalRegionCodeList` constrains its member string only
/// (`smithy_query shape=ExternalRegionCodeList`) — and neither the create nor the update
/// of this attachment kind models a quota error (`smithy_query query="operation"
/// operation="CreateDirectConnectGatewayAttachment,UpdateDirectConnectGatewayAttachment"`),
/// so its only purpose is that no caller can grow one attachment's list without end. The
/// number is generous against what a list can usefully name: an attachment spans EDGES of
/// its core network, a core network configures at most one edge per Region, and no entry
/// may repeat ([`check_edge_location_list`]), so a longer list could name no further edge.
pub const MAX_EDGE_LOCATIONS_PER_ATTACHMENT: usize = 50;

/// Check ONE edge location a record is about to be placed at against `topology`, the policy
/// topology of the core network it hangs off (`None` where that core network holds none,
/// which configures no edge at all).
///
/// A record that sits at a core network EDGE sits at an edge the policy IN FORCE configures
/// ([`PolicyTopology::has_edge_location`]), and which edges those are moves with the
/// document in force: a version put in force after a resource was created can configure no
/// edge where an earlier one did. So the location is judged HERE, at the moment of the
/// call, whether the request named it or the handler took it from a resource created
/// earlier — otherwise the stored record would claim an edge the same core network reports
/// none of. `member` is the request member that asked: the one carrying the location, or
/// the reference the handler read it from.
///
/// ONE function owns that rule for every create and update of this service that places a
/// record at a single edge, and [`check_edge_location_list`] below applies it per entry of
/// a list, so no two of them can judge a location differently.
pub fn check_edge_location(
    member: &str,
    core_network_id: &str,
    topology: Option<&PolicyTopology>,
    location: &str,
) -> Result<(), ValidationException> {
    if topology.is_some_and(|topology| topology.has_edge_location(location)) {
        Ok(())
    } else {
        Err(no_such_edge(member, core_network_id, location))
    }
}

/// Check an `EdgeLocations` list a request carries for a Direct Connect gateway attachment
/// against `topology`, the policy topology of the core network the attachment belongs to
/// (`None` where that core network holds none, which configures no edge at all).
///
/// The LENGTH is judged FIRST, before anything walks the list, so no per-entry work — and
/// no retention — is spent on a list already too long to be accepted
/// ([`MAX_EDGE_LOCATIONS_PER_ATTACHMENT`]). Then the list must name at least one location,
/// since an attachment joins its resource to at least one edge; every location it names
/// must be one the policy in force configures an edge for, which is
/// [`check_edge_location`] per entry; and no location may repeat, since an attachment
/// spans an edge once or not at all. Membership is tested through a set built once, so a
/// list at the bound costs O(n log n).
///
/// Shared by `CreateDirectConnectGatewayAttachment` and
/// `UpdateDirectConnectGatewayAttachment` — the create's list and the list an update
/// OVERWRITES it with are the same thing — so ONE function owns the rule and the two
/// cannot judge a list differently. Every refusal names [`FIELD_EDGE_LOCATIONS`], the
/// member both requests carry it in.
pub fn check_edge_location_list(
    core_network_id: &str,
    topology: Option<&PolicyTopology>,
    requested: &[String],
) -> Result<(), ValidationException> {
    if requested.len() > MAX_EDGE_LOCATIONS_PER_ATTACHMENT {
        return Err(field_rejection(
            FIELD_EDGE_LOCATIONS,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "at most {MAX_EDGE_LOCATIONS_PER_ATTACHMENT} edge locations may be named, but {} were",
                requested.len()
            ),
        ));
    }
    if requested.is_empty() {
        return Err(field_rejection(
            FIELD_EDGE_LOCATIONS,
            REASON_FIELD_VALIDATION_FAILED,
            "a Direct Connect gateway attachment must name at least one edge location"
                .to_string(),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for location in requested {
        check_edge_location(FIELD_EDGE_LOCATIONS, core_network_id, topology, location)?;
        if !seen.insert(location.as_str()) {
            return Err(field_rejection(
                FIELD_EDGE_LOCATIONS,
                VALIDATION_REASON_OTHER,
                format!("edge location {location} is named more than once"),
            ));
        }
    }
    Ok(())
}

/// A telemetry-bearing network resource of one global network: a connection registered
/// with the global network whose session health `GetNetworkTelemetry` reports.///
/// Its `resource_type` is one of [`TELEMETRY_RESOURCE_TYPES`] — nothing else has a session
/// to measure — and its identity is its ARN, which is an ARN of the service that OWNS the
/// connection (Amazon VPC for a transit gateway Connect peer or a VPN connection, this
/// service for a Cloud WAN Connect peer), not one this service mints. So the ARN is stored
/// and `ResourceId` and `AccountId` are DERIVED from it ([`arn_resource_id`],
/// [`arn_account_id`]) rather than kept a second time.
///
/// The Region is stored, because it cannot be derived: a Cloud WAN Connect peer's ARN
/// carries an EMPTY region segment (this service is partition-scoped, see `ARN_REGION`)
/// while its telemetry still reports the edge location the peer lives at.
///
/// Like every other record in this slice, the health this record reports is a function of
/// the clock rather than a field ([`Self::health_at`]), so reading telemetry never writes.
#[derive(Debug, Clone)]
pub struct TelemetryResourceRecord {
    /// The global network the connection is registered with: the one whose
    /// `GetNetworkTelemetry` reports it.
    pub global_network_id: String,
    /// One of [`TELEMETRY_RESOURCE_TYPES`].
    pub resource_type: String,
    /// The ARN of the connection itself, which is also this record's identity.
    pub resource_arn: String,
    /// The Amazon Web Services Region the connection lives in.
    pub aws_region: String,
    /// The core network the connection belongs to, for a Cloud WAN Connect peer; `None`
    /// for a connection reached through a registered gateway instead.
    pub core_network_id: Option<String>,
    /// The ARN of the gateway the connection was registered through (a transit gateway or
    /// a customer gateway), for a connection on the gateway side; `None` for a Cloud WAN
    /// Connect peer.
    pub registered_gateway_arn: Option<String>,
    /// The address of the connection's far end, when the registration recorded one.
    pub address: Option<String>,
    /// The instant the connection was registered with the global network: the instant the
    /// `DOWN` health this record starts out with took effect.
    pub registered_at: SystemTime,
    /// The instant the session the registration started comes up — the transition the
    /// clock owes, [`SETTLE_DELAY`] after the registering call — and so the instant the
    /// `UP` health takes effect.
    pub connected_at: SystemTime,
}

impl TelemetryResourceRecord {
    /// The protocol whose session this resource's health belongs to: IPsec for a VPN
    /// connection, BGP for a Connect peer of either kind. See [`CONNECTION_TYPE_BGP`].
    pub fn connection_type(&self) -> &'static str {
        match self.resource_type.as_str() {
            VPN_CONNECTION_TYPE => CONNECTION_TYPE_IPSEC,
            // `connect-peer` and `transit-gateway-connect-peer`: both peer over BGP.
            _ => CONNECTION_TYPE_BGP,
        }
    }

    /// Whether the session is up as of `now`: the registration starts it, and it comes up
    /// at [`Self::connected_at`] — the transition the clock owes, never a stored flag, so
    /// a caller that reads straight after the registration still sees it `DOWN`.
    pub fn is_up_at(&self, now: SystemTime) -> bool {
        now >= self.connected_at
    }

    /// The modelled `ConnectionHealth` for this record as of `now`.
    ///
    /// `Timestamp` is "The time the status was last updated"
    /// (`smithy_query query="doc" shape="ConnectionHealth" member="Timestamp"`), which is
    /// the instant the status being reported took effect: the registration while the
    /// session is still down, the instant it came up afterwards. Both are stored instants,
    /// so repeated reads of one record report the identical health.
    pub fn health_at(&self, now: SystemTime) -> ConnectionHealth {
        let up = self.is_up_at(now);
        let status = if up {
            CONNECTION_STATUS_UP
        } else {
            CONNECTION_STATUS_DOWN
        };
        let since = if up { self.connected_at } else { self.registered_at };
        ConnectionHealth {
            r#type: Some(self.connection_type().to_string()),
            status: Some(status.to_string()),
            timestamp: Some(epoch_seconds(since)),
        }
    }

    /// The modelled `NetworkTelemetry` for this record as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> NetworkTelemetry {
        NetworkTelemetry {
            registered_gateway_arn: self.registered_gateway_arn.clone(),
            core_network_id: self.core_network_id.clone(),
            aws_region: Some(self.aws_region.clone()),
            account_id: arn_account_id(&self.resource_arn).map(str::to_string),
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(arn_resource_id(&self.resource_arn).to_string()),
            resource_arn: Some(self.resource_arn.clone()),
            address: self.address.clone(),
            health: Some(self.health_at(now)),
        }
    }
}

/// A stored Connect peer association: the tie between one core network Connect peer and
/// one DEVICE of a global network, optionally over one of that device's links.
///
/// "Associates a core network Connect peer with a device and optionally, with a link."
/// (`smithy_query query="doc" shape="AssociateConnectPeer"`) — like a link association the
/// tie carries no id and no ARN of its own (the modelled `ConnectPeerAssociation` is the
/// peer, the global network, the device, the link and a state, `smithy_query
/// shape=ConnectPeerAssociation`), so the PEER is its identity: one peer reaches its
/// appliance over one device, and the store keys the association by `ConnectPeerId`. That
/// is also what `DisassociateConnectPeer` addresses, which names the peer and nothing else
/// (`smithy_query query="operation" operation="DisassociateConnectPeer"`).
///
/// Like the records above, its state is a function of the clock rather than a field, so
/// reading one never writes.
#[derive(Debug, Clone)]
pub struct ConnectPeerAssociationRecord {
    pub connect_peer_id: String,
    pub global_network_id: String,
    pub device_id: String,
    /// The link the device reaches this peering over, or `None` where the association
    /// named none — the link is optional on the request that makes the association.
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl ConnectPeerAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `ConnectPeerAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=ConnectPeerAssociationState`, mirrored in
    /// `ops::types::CONNECT_PEER_ASSOCIATION_STATE_VALUES`); the first three are spelled
    /// exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this
    /// slice reuses those constants, as [`LinkAssociationRecord::state_at`] does.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `ConnectPeerAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> ConnectPeerAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `ConnectPeerAssociation` for this record, forcing `state` — used by
    /// `DisassociateConnectPeer`, which reports the association it just removed as
    /// `DELETING`, exactly as [`TransitGatewayConnectPeerAssociationRecord::to_shape_with_state`]
    /// serves its own disassociate.
    pub fn to_shape_with_state(&self, state: &str) -> ConnectPeerAssociation {
        ConnectPeerAssociation {
            connect_peer_id: Some(self.connect_peer_id.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The two `Action` values `StartOrganizationServiceAccessUpdate` accepts, in the order
/// its documentation lists them.
///
/// The model types `Action` as a free-form string of length 0..50 and enumerates nothing
/// (`smithy_query shape=Action`); the catalog is the member's own documentation — "The
/// action to take for the update request. This can be either ENABLE or DISABLE"
/// (`smithy_query query="doc" shape="StartOrganizationServiceAccessUpdateRequest"
/// member="Action"`) — so a value outside it is refused by the handler rather than by the
/// schema.
pub const SERVICE_ACCESS_ACTIONS: [&str; 2] = ["ENABLE", "DISABLE"];

/// The input member a refused `StartOrganizationServiceAccessUpdate` names in
/// `ValidationException$Fields[].Name`, and the one a listing names for a token it did not
/// mint.
pub const FIELD_ACTION: &str = "Action";
/// See [`FIELD_ACTION`].
pub const FIELD_NEXT_TOKEN: &str = "NextToken";

/// The two values `OrganizationAwsServiceAccessStatus` carries: "The status of the
/// organization's AWS service access. This will be ENABLED or DISABLED" (`smithy_query
/// query="doc" shape="OrganizationStatus" member="OrganizationAwsServiceAccessStatus"`).
/// The modelled string enumerates nothing, so the pair lives here.
pub const SERVICE_ACCESS_ENABLED: &str = "ENABLED";
/// See [`SERVICE_ACCESS_ENABLED`].
pub const SERVICE_ACCESS_DISABLED: &str = "DISABLED";

/// The two values `SLRDeploymentStatus` carries: "The status of the SLR deployment for the
/// account. This will be either SUCCEEDED or IN_PROGRESS" (`smithy_query query="doc"
/// shape="OrganizationStatus" member="SLRDeploymentStatus"`). A deployment is IN_PROGRESS
/// until the span [`SETTLE_DELAY`] the clock owes has passed, then SUCCEEDED.
pub const SLR_DEPLOYMENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`SLR_DEPLOYMENT_IN_PROGRESS`].
pub const SLR_DEPLOYMENT_SUCCEEDED: &str = "SUCCEEDED";

/// The id prefix of an Amazon Web Services Organization. The model constrains the id
/// itself: `OrganizationId` carries `pattern ^o-([0-9a-f]{8,17})$` (`smithy_query
/// shape=OrganizationId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const ORGANIZATION_KIND: &str = "o";

/// This account's Network Manager service-access record for its Amazon Web Services
/// Organization: whether the service has organization-wide access, and the state of the
/// service-linked-role (SLR) deployment that enabling it started.
///
/// There is at most ONE such record, because this emulator presents exactly one account
/// ([`::engine::emulator_account_id`]) and an account belongs to at most one organization.
/// `SLRDeploymentStatus` is not a stored field: it is a function of the clock
/// ([`OrganizationServiceAccessRecord::slr_deployment_status_at`]), so reading one never
/// writes.
#[derive(Debug, Clone)]
pub struct OrganizationServiceAccessRecord {
    /// The minted `o-<hex>` id of the organization the record belongs to. Minted once and
    /// kept across later enable/disable updates: the organization does not change when
    /// service access to it does.
    pub organization_id: String,
    /// [`SERVICE_ACCESS_ENABLED`] or [`SERVICE_ACCESS_DISABLED`].
    pub service_access_status: String,
    /// The accounts the SLR deployment in effect covers, in account-id order.
    ///
    /// The organization's membership is not knowable from inside this service — the
    /// Organizations emulator's own `ListAccounts` is a scaffold stub, and this emulator
    /// presents exactly one account — so an enabled organization's deployment covers that
    /// single (management) account. EMPTY while service access is disabled: no SLR is
    /// deployed then, so there is no deployment status to report for the organization or
    /// for any account in it.
    pub deployed_accounts: Vec<String>,
    /// The instant the SLR deployment the enabling call started becomes due, i.e. when
    /// `IN_PROGRESS` becomes `SUCCEEDED`.
    pub deployed_at: SystemTime,
}

impl OrganizationServiceAccessRecord {
    /// The SLR deployment status of every account this deployment covers, as of `now`, or
    /// `None` while no SLR is deployed at all (see [`Self::deployed_accounts`]).
    ///
    /// One due instant covers the whole deployment, so this is also the organization-wide
    /// aggregate: `IN_PROGRESS` until it passes, `SUCCEEDED` after.
    pub fn slr_deployment_status_at(&self, now: SystemTime) -> Option<&'static str> {
        if self.deployed_accounts.is_empty() {
            return None;
        }
        Some(if now >= self.deployed_at {
            SLR_DEPLOYMENT_SUCCEEDED
        } else {
            SLR_DEPLOYMENT_IN_PROGRESS
        })
    }

    /// One modelled `AccountStatus` per account the deployment covers, keyed by account id
    /// so [`paginate`] can page over them in a stable order.
    pub fn account_statuses_at(&self, now: SystemTime) -> Vec<(String, AccountStatus)> {
        let status = self.slr_deployment_status_at(now);
        self.deployed_accounts
            .iter()
            .map(|account_id| {
                (
                    account_id.clone(),
                    AccountStatus {
                        account_id: Some(account_id.clone()),
                        slr_deployment_status: status.map(str::to_string),
                    },
                )
            })
            .collect()
    }

    /// The modelled `OrganizationStatus` for this record as of `now`, carrying
    /// `account_status_list` — the page of accounts the caller asked for, which is why the
    /// list is passed in rather than computed here.
    pub fn to_shape(
        &self,
        now: SystemTime,
        account_status_list: Vec<AccountStatus>,
    ) -> OrganizationStatus {
        OrganizationStatus {
            organization_id: Some(self.organization_id.clone()),
            organization_aws_service_access_status: Some(self.service_access_status.clone()),
            slr_deployment_status: self.slr_deployment_status_at(now).map(str::to_string),
            account_status_list: Some(account_status_list),
        }
    }
}

/// The members of `RouteAnalysisStatus` a route analysis of this service reports
/// (`smithy_query shape=RouteAnalysisStatus`: RUNNING, COMPLETED, FAILED — mirrored in
/// `ops::types::ROUTE_ANALYSIS_STATUS_VALUES`).
///
/// An analysis starts `RUNNING` and reaches `COMPLETED` when the transition the clock owes
/// falls due ([`RouteAnalysisRecord::status_at`]). `FAILED` is the analysis itself failing
/// rather than finding no route — a path that does not reach the destination COMPLETES,
/// carrying [`ROUTE_ANALYSIS_NOT_CONNECTED`] — so nothing here reports it.
pub const ROUTE_ANALYSIS_RUNNING: &str = "RUNNING";
/// See [`ROUTE_ANALYSIS_RUNNING`].
pub const ROUTE_ANALYSIS_COMPLETED: &str = "COMPLETED";

/// The members of `RouteAnalysisCompletionResultCode` (`smithy_query
/// shape=RouteAnalysisCompletionResultCode`: CONNECTED, NOT_CONNECTED — mirrored in
/// `ops::types::ROUTE_ANALYSIS_COMPLETION_RESULT_CODE_VALUES`): whether the analysed path
/// reached its destination. A `NOT_CONNECTED` path also carries the reason code for where
/// it stopped ("The reason code. Available only if a connection is not found.",
/// `smithy_query query="doc" shape="RouteAnalysisCompletion" member="ReasonCode"`).
pub const ROUTE_ANALYSIS_CONNECTED: &str = "CONNECTED";
/// See [`ROUTE_ANALYSIS_CONNECTED`].
pub const ROUTE_ANALYSIS_NOT_CONNECTED: &str = "NOT_CONNECTED";

/// The members of `RouteAnalysisCompletionReasonCode` a path of this service completes
/// with, each documented on the member itself (`smithy_query query="doc"
/// shape="RouteAnalysisCompletion" member="ReasonCode"`, mirrored in
/// `ops::types::ROUTE_ANALYSIS_COMPLETION_REASON_CODE_VALUES`):
///
/// * this one — the request named no attachment ARN for the endpoint the path runs
///   TOWARDS, so there is no destination to analyse a path to;
/// * [`REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND`] — "Cannot find an attachment": the
///   ARN an endpoint named is not an attachment the global network holds;
/// * [`REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY`] — "Found an attachment,
///   but not to the correct transit gateway": the ARN names an attachment of the global
///   network that is not one on a transit gateway;
/// * [`REASON_ROUTE_NOT_FOUND`] — "Cannot find a route table with the destination CIDR
///   block": both endpoints are attachments on transit gateways of the global network, and
///   no route table of them carries the destination.
pub const REASON_NO_DESTINATION_ARN_PROVIDED: &str = "NO_DESTINATION_ARN_PROVIDED";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND: &str =
    "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY: &str =
    "TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_ROUTE_NOT_FOUND: &str = "ROUTE_NOT_FOUND";

/// The request members a route analysis's two endpoints come from, which is how a reason
/// context names the endpoint whose attachment ARN a path could not get past
/// ([`RouteEndpoint::reason_context`]).
pub const ENDPOINT_SOURCE: &str = "Source";
/// See [`ENDPOINT_SOURCE`].
pub const ENDPOINT_DESTINATION: &str = "Destination";

/// The reason-context key naming the `TransitGatewayAttachmentArn` of the endpoint
/// [`ENDPOINT_SOURCE`] / [`ENDPOINT_DESTINATION`] names.
///
/// `ReasonContext` is "Additional information about the path. Available only if a
/// connection is not found." (`smithy_query query="doc" shape="RouteAnalysisCompletion"
/// member="ReasonContext"`) — an unconstrained map — so the key names the request member
/// whose value the analysis could not resolve, and the value is that ARN.
pub fn reason_context_key(endpoint_member: &str) -> String {
    format!("{endpoint_member}.TransitGatewayAttachmentArn")
}

/// What the global network's network-resource inventory holds for the
/// `TransitGatewayAttachmentArn` one endpoint of a route analysis names.
///
/// A routing path runs between two attachments on transit gateways registered with the
/// global network, so this is what decides whether the path can start at an endpoint at
/// all, and the reason code it completes with when it cannot.
#[derive(Debug, Clone)]
enum RouteEndpointResolution {
    /// The endpoint named no attachment ARN at all.
    Unspecified,
    /// The ARN names no network resource of this global network.
    Unknown,
    /// The ARN names an attachment on a transit gateway registered with this global
    /// network ([`TRANSIT_GATEWAY_ATTACHMENT_TYPE`]), carrying the ARN of the gateway it
    /// was registered through where the inventory records one.
    TransitGatewayAttachment { transit_gateway_arn: Option<String> },
    /// The ARN names a network resource of this global network of some OTHER type,
    /// carrying that `NetworkResource$ResourceType`.
    OtherResource { resource_type: String },
}

/// One endpoint of a route analysis: the request member it came from, what it named, and
/// what the global network knows about the attachment ARN it named.
#[derive(Debug, Clone)]
struct RouteEndpoint {
    /// [`ENDPOINT_SOURCE`] or [`ENDPOINT_DESTINATION`].
    member: &'static str,
    /// The `TransitGatewayAttachmentArn` the endpoint named, if any.
    attachment_arn: Option<String>,
    /// The `IpAddress` the endpoint named, if any. The analysis carries it through to the
    /// endpoint options it reports; no route table of this global network holds a CIDR to
    /// match it against (see [`REASON_ROUTE_NOT_FOUND`]).
    ip_address: Option<String>,
    /// See [`RouteEndpointResolution`].
    resolution: RouteEndpointResolution,
}

impl RouteEndpoint {
    /// The modelled endpoint options this endpoint reports: the ARN and the IP address the
    /// request named, plus the transit gateway the attachment belongs to — which is
    /// RESOLVED from the inventory, not supplied by the caller, and is therefore absent
    /// for an endpoint the global network holds no such attachment for.
    fn options(&self) -> RouteAnalysisEndpointOptions {
        RouteAnalysisEndpointOptions {
            transit_gateway_attachment_arn: self.attachment_arn.clone(),
            transit_gateway_arn: match &self.resolution {
                RouteEndpointResolution::TransitGatewayAttachment {
                    transit_gateway_arn,
                } => transit_gateway_arn.clone(),
                _ => None,
            },
            ip_address: self.ip_address.clone(),
        }
    }

    /// The reason code a path stops with at this endpoint, or `None` for one a path can run
    /// through — an attachment on a transit gateway of this global network.
    fn reason_code(&self) -> Option<&'static str> {
        match &self.resolution {
            RouteEndpointResolution::TransitGatewayAttachment { .. } => None,
            // An attachment of the global network that is not one on a transit gateway: a
            // Cloud WAN attachment ([`ATTACHMENT_KIND`]) is an attachment, so the reason is
            // the "found an attachment, but not to the correct transit gateway" one rather
            // than "cannot find an attachment".
            RouteEndpointResolution::OtherResource { resource_type }
                if resource_type == ATTACHMENT_KIND =>
            {
                Some(REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY)
            }
            _ => Some(REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND),
        }
    }

    /// The reason context a path carries when it stopped at this endpoint: the request
    /// member and the ARN the analysis could not resolve, or `None` for an endpoint that
    /// named no ARN at all — there is nothing further to report about it.
    fn reason_context(&self) -> Option<std::collections::HashMap<String, String>> {
        let arn = self.attachment_arn.clone()?;
        Some(std::collections::HashMap::from([(
            reason_context_key(self.member),
            arn,
        )]))
    }
}

/// A stored route analysis: the Route Analyzer job one `StartRouteAnalysis` records on its
/// global network, which `GetRouteAnalysis` reads back.
///
/// The paths are computed ONCE, by the call that starts the analysis, out of what the
/// global network held at that instant — an analysis is a snapshot of the routing it
/// found, so a resource created afterwards cannot change the answer a completed analysis
/// reports. What the CLOCK owes is the `RUNNING` -> `COMPLETED` transition
/// ([`Self::completed_at`]), so `Status` is a function of the clock rather than a stored
/// field and reading an analysis never writes.
#[derive(Debug, Clone)]
pub struct RouteAnalysisRecord {
    /// The minted `route-analysis-<hex>` id, this record's identity.
    pub id: String,
    /// The global network the analysis was started on: the only one it can be read back
    /// through.
    pub global_network_id: String,
    /// "The time that the analysis started" (`smithy_query query="doc"
    /// shape="RouteAnalysis" member="StartTimestamp"`).
    pub started_at: SystemTime,
    /// The instant the `RUNNING` -> `COMPLETED` transition the clock owes falls due:
    /// [`SETTLE_DELAY`] after the call that started the analysis, so a caller that reads
    /// straight afterwards still sees it `RUNNING`.
    pub completed_at: SystemTime,
    /// The source as the analysis ran it: the ARN and IP address the request named, plus
    /// the transit gateway resolved for that attachment.
    pub source: RouteAnalysisEndpointOptions,
    /// The destination as the analysis ran it; see [`Self::source`].
    pub destination: RouteAnalysisEndpointOptions,
    pub include_return_path: bool,
    pub use_middleboxes: bool,
    /// The forward path, as the analysis completed it.
    pub forward_path: RouteAnalysisPath,
    /// The return path, or `None` where none was analysed: "The return path is not
    /// analyzed if the forward path analysis does not succeed" (`smithy_query query="doc"
    /// shape="RouteAnalysis" member="IncludeReturnPath"`), and none is analysed at all
    /// unless the request asked for one.
    pub return_path: Option<RouteAnalysisPath>,
}

impl RouteAnalysisRecord {
    /// This analysis's status as of `now`: [`ROUTE_ANALYSIS_RUNNING`] until the transition
    /// the clock owes falls due, [`ROUTE_ANALYSIS_COMPLETED`] after it.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.completed_at {
            ROUTE_ANALYSIS_COMPLETED
        } else {
            ROUTE_ANALYSIS_RUNNING
        }
    }

    /// The modelled `RouteAnalysis` for this record as of `now`.
    ///
    /// The paths are "The status of the analysis at completion" (`smithy_query query="doc"
    /// shape="RouteAnalysisPath" member="CompletionStatus"`), so they are reported only
    /// once the analysis has completed; while it is still `RUNNING` there is no completion
    /// to report. Both are already stored, so no read computes them a second time.
    pub fn to_shape(&self, now: SystemTime) -> RouteAnalysis {
        let completed = self.status_at(now) == ROUTE_ANALYSIS_COMPLETED;
        RouteAnalysis {
            global_network_id: Some(self.global_network_id.clone()),
            // "The ID of the AWS account that created the route analysis": this emulator
            // presents exactly one account, which is the one every call runs as.
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            route_analysis_id: Some(self.id.clone()),
            start_timestamp: Some(epoch_seconds(self.started_at)),
            status: Some(self.status_at(now).to_string()),
            source: Some(self.source.clone()),
            destination: Some(self.destination.clone()),
            include_return_path: self.include_return_path,
            use_middleboxes: self.use_middleboxes,
            forward_path: completed.then(|| self.forward_path.clone()),
            return_path: completed.then(|| self.return_path.clone()).flatten(),
        }
    }
}

/// The path `from` -> `to`, as the analysis completes it.
///
/// A routing path runs from one attachment on a transit gateway of the global network to
/// another, so the completion is decided by what the global network holds for the two
/// endpoints ([`RouteEndpoint::reason_code`]), in the order the path depends on them:
///
/// 1. the endpoint the path runs TOWARDS must name an attachment ARN at all, or there is
///    no destination to analyse towards ([`REASON_NO_DESTINATION_ARN_PROVIDED`]);
/// 2. the endpoint it starts FROM must be an attachment on a transit gateway of this
///    global network, then the one it runs towards must be too;
/// 3. with both of them attachments on transit gateways of this global network, the path
///    needs a route table of those gateways carrying the destination. A transit gateway's
///    route tables are Amazon VPC's, learned through the gateway's registration with the
///    global network, and the inventory holds none, so no route table carries it
///    ([`REASON_ROUTE_NOT_FOUND`]).
///
/// The path carries no component: nothing was traversed, since the analysis stopped at an
/// endpoint or at the missing route table. `UseMiddleboxes` asks for "the location of
/// middlebox appliances in the route analysis" (`smithy_query query="doc"
/// shape="RouteAnalysis" member="UseMiddleboxes"`), which is a property of the components
/// of a path, so it changes nothing about a path that has none.
fn analyze_route_path(from: &RouteEndpoint, to: &RouteEndpoint) -> RouteAnalysisPath {
    let (reason_code, reason_context) = if to.attachment_arn.is_none() {
        (REASON_NO_DESTINATION_ARN_PROVIDED, None)
    } else if let Some(reason) = from.reason_code() {
        (reason, from.reason_context())
    } else if let Some(reason) = to.reason_code() {
        (reason, to.reason_context())
    } else {
        (REASON_ROUTE_NOT_FOUND, to.reason_context())
    };
    RouteAnalysisPath {
        completion_status: Some(RouteAnalysisCompletion {
            result_code: Some(ROUTE_ANALYSIS_NOT_CONNECTED.to_string()),
            reason_code: Some(reason_code.to_string()),
            reason_context,
        }),
        path: Some(Vec::new()),
    }
}

/// Whether `path` reached its destination.
fn route_path_connected(path: &RouteAnalysisPath) -> bool {
    path.completion_status
        .as_ref()
        .and_then(|completion| completion.result_code.as_deref())
        == Some(ROUTE_ANALYSIS_CONNECTED)
}

/// The key the telemetry store holds one registration under: the global network, then the
/// resource ARN, so the map groups a global network's connections into one contiguous run
/// ordered by ARN and a pagination token can name an offset into it.
///
/// The pair is the identity: one connection is registered with one global network, and the
/// same ARN may not be registered with it twice.
pub fn telemetry_key(global_network_id: &str, resource_arn: &str) -> String {
    format!("{global_network_id}/{resource_arn}")
}

/// The id of the resource an ARN names: the text after the last `/` of its resource
/// segment, or the whole segment for an ARN that carries no `/` at all.
///
/// Unlike [`parse_resource_arn`] this judges nothing about the ARN and accepts one of ANY
/// service, because a telemetry resource is owned by whichever service runs the connection
/// (`arn:aws:ec2:<region>:<account>:vpn-connection/<id>` for a VPN connection,
/// `arn:aws:networkmanager::<account>:connect-peer/<id>` for a Cloud WAN Connect peer).
pub fn arn_resource_id(arn: &str) -> &str {
    let resource = arn.rsplit_once(':').map_or(arn, |(_, tail)| tail);
    resource.rsplit_once('/').map_or(resource, |(_, id)| id)
}

/// The TYPE of the resource an ARN names: the text of its resource segment ahead of the
/// first `/`, or `None` for an ARN whose resource segment carries no `/` at all and so
/// names no type of its own.
///
/// The companion of [`arn_resource_id`], and judges as little as it does: the ARN may
/// belong to any service, because the resource an attachment attaches is owned by
/// whichever service runs it (`arn:aws:ec2:<region>:<account>:vpc/<id>` for a VPC,
/// `arn:aws:networkmanager::<account>:attachment/<id>` for the transport attachment a
/// Connect attachment is carried over). So a next hop reports the kind and the id of the
/// resource it reaches as that resource's own ARN spells them, rather than a second copy
/// kept beside it.
pub fn arn_resource_kind(arn: &str) -> Option<&str> {
    let resource = arn.rsplit_once(':').map_or(arn, |(_, tail)| tail);
    let (kind, id) = resource.split_once('/')?;
    (!kind.is_empty() && !id.is_empty()).then_some(kind)
}

/// The account id an ARN carries (its fifth colon-separated field), or `None` when the
/// string has no such field or carries it empty.
///
/// This is the account that OWNS the resource, which is what a telemetry entry reports in
/// `AccountId` — not the account this emulator runs as, since a connection may be owned by
/// another account of the organization.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let account = arn.split(':').nth(4)?;
    (!account.is_empty()).then_some(account)
}

/// The Region an ARN carries (its fourth colon-separated field), or `None` when the string
/// has no such field or carries it empty — which is what every ARN of this global service
/// carries, and what an ARN of a global resource of another service carries too.
///
/// The companion of [`arn_account_id`], for the same reason: the Region a resource of
/// ANOTHER service lives in is spelled by its own ARN, so a reader comparing Regions reads
/// it from there rather than from a second copy. A VPC attachment's edge location is not a
/// request member either: it is the Region the VPC is in, which only the VPC's ARN names,
/// and the same holds for a site-to-site VPN attachment and the VPN connection it attaches.
/// The modelled patterns for `VpcArn`, `SubnetArn` and `VpnConnectionArn` all allow an
/// EMPTY Region segment
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`, `smithy_query
/// shape=VpcArn`), so an ARN that names no Region answers `None` here and is refused by the
/// caller rather than stored with an edge location no edge has.
pub fn arn_region(arn: &str) -> Option<&str> {
    let region = arn.split(':').nth(3)?;
    (!region.is_empty()).then_some(region)
}

/// Per-service state for com-amazonaws-networkmanager-networkmanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Global networks by `GlobalNetworkId`. A `BTreeMap`, so listing order is stable
    /// and a pagination token can name a key offset into it.
    pub global_networks: BTreeMap<String, GlobalNetworkRecord>,
    /// Sites by `SiteId`, each carrying its owning `GlobalNetworkId`.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Links by `LinkId`, each carrying the `SiteId` and `GlobalNetworkId` it belongs to.
    pub links: BTreeMap<String, LinkRecord>,
    /// Devices by `DeviceId`, each carrying its owning `GlobalNetworkId` and, when the
    /// create named one, the `SiteId` it sits at.
    pub devices: BTreeMap<String, DeviceRecord>,
    /// Link associations by [`link_association_key`], each tying one stored device to one
    /// stored link of the same site.
    ///
    /// No quota of its own bounds this map: an association exists only for a device and a
    /// link that are both stored and sit at the same site, so the per-global-network
    /// device quota and the per-site link quota already bound how many pairs can exist.
    pub link_associations: BTreeMap<String, LinkAssociationRecord>,
    /// Customer gateway associations by [`customer_gateway_association_key`], each tying a
    /// customer gateway of Amazon VPC to one stored device, and optionally one stored link,
    /// of the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] — unlike the link
    /// associations above, the subject is a gateway of another service rather than a stored
    /// resource, so no existing quota bounds how many distinct ARNs a caller can name — and
    /// only ever holding a pair once.
    pub customer_gateway_associations: BTreeMap<String, CustomerGatewayAssociationRecord>,
    /// Transit gateway registrations by [`transit_gateway_registration_key`], each tying a
    /// transit gateway of Amazon VPC to the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_TRANSIT_GATEWAY_REGISTRATIONS_PER_GLOBAL_NETWORK`], for the reason the customer
    /// gateway associations above are: the subject is a gateway of another service rather
    /// than a stored resource, so no existing quota bounds how many distinct ARNs a caller
    /// can name — and only ever holding a pair once.
    pub transit_gateway_registrations: BTreeMap<String, TransitGatewayRegistrationRecord>,
    /// Transit gateway Connect peer associations by
    /// [`transit_gateway_connect_peer_association_key`], each tying a Connect peer of a
    /// transit gateway of Amazon VPC to one stored device, and optionally one stored link,
    /// of the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK`], for the reason
    /// the customer gateway associations above are: the subject is a resource of another
    /// service rather than a stored one, so no existing quota bounds how many distinct ARNs
    /// a caller can name — and only ever holding a pair once.
    pub transit_gateway_connect_peer_associations:
        BTreeMap<String, TransitGatewayConnectPeerAssociationRecord>,
    /// Connections by `ConnectionId`, each carrying the `GlobalNetworkId` that owns it and
    /// the two devices of that global network it ties together.
    pub connections: BTreeMap<String, ConnectionRecord>,
    /// Core networks by `CoreNetworkId`, each carrying its owning `GlobalNetworkId`.
    pub core_networks: BTreeMap<String, CoreNetworkRecord>,
    /// Peerings by `PeeringId`, each carrying the `CoreNetworkId` it joins to a resource
    /// of another service ([`PeeringRecord`]). A `BTreeMap`, so the store has one stable
    /// order like every other map here.
    ///
    /// Bounded by [`MAX_PEERINGS_PER_CORE_NETWORK`] per core network, so repeated creates
    /// cannot grow it without end.
    pub peerings: BTreeMap<String, PeeringRecord>,
    /// Attachments by `AttachmentId`, of every kind, each carrying the `CoreNetworkId` it
    /// belongs to. A `BTreeMap`, so listing order is stable and a pagination token can
    /// name a key offset into it.
    ///
    /// One map for every attachment kind: the kinds share the modelled `Attachment`
    /// envelope and differ only in [`AttachmentDetail`], and the quota, the
    /// network-resource inventory and the core-network delete refusal all count
    /// attachments without caring which kind each is.
    pub attachments: BTreeMap<String, AttachmentRecord>,
    /// Prefix list associations by [`prefix_list_association_key`], each tying one stored
    /// core network to one managed prefix list of Amazon VPC. A `BTreeMap`, so listing
    /// order is stable and a pagination token can name a key offset into it.
    ///
    /// Bounded per core network by [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`],
    /// and only ever holding a pair once, so no caller can grow it without end.
    pub prefix_list_associations: BTreeMap<String, PrefixListAssociationRecord>,
    /// Connect peers by `ConnectPeerId`, each carrying the Connect attachment it is carried
    /// over and that attachment's core network and edge location. A `BTreeMap`, so listing
    /// order is stable and a pagination token can name a key offset into it.
    ///
    /// Bounded by [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] per attachment, on top of the
    /// per-core-network attachment quota, so no caller can grow the map without end.
    pub connect_peers: BTreeMap<String, ConnectPeerRecord>,
    /// Connect peer associations by `ConnectPeerId` — the tie each peer has to one device
    /// of a global network ([`ConnectPeerAssociationRecord`]). A `BTreeMap`, so listing
    /// order is the stable peer-id order and a pagination token can name a key offset
    /// into it.
    ///
    /// No quota of its own bounds this map: it holds at most one entry per stored Connect
    /// peer, which [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] and the per-core-network
    /// attachment quota already bound.
    pub connect_peer_associations: BTreeMap<String, ConnectPeerAssociationRecord>,
    /// Resource policies by the ARN of the resource they are attached to — the one store
    /// `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` share, holding
    /// the document exactly as it was accepted so a reader reports what was put.
    ///
    /// Keyed by ARN rather than hung off a resource record: none of the three operations
    /// models a `ResourceNotFoundException` (`smithy_query query="operation"
    /// operation="GetResourcePolicy,PutResourcePolicy,DeleteResourcePolicy"`), unlike every
    /// other reader of a resource in this service, so the policy store does not assert
    /// that the ARN names a resource this account holds — only that it names a resource
    /// type a policy may be attached to ([`parse_policy_attachable_arn`]), which is a
    /// resource of ANOTHER service. The store is bounded instead by its own quota:
    /// at most [`MAX_RESOURCE_POLICIES`] ARNs hold a policy, each document at most
    /// [`MAX_RESOURCE_POLICY_CHARS`] characters, so no caller can grow it without end.
    pub resource_policies: BTreeMap<String, String>,
    /// Telemetry-bearing connections by [`telemetry_key`]: the registrations
    /// `GetNetworkTelemetry` reports the session health of.
    ///
    /// A connection is registered here by the operation that ties it to the global network:
    /// `AssociateConnectPeer` for a core network Connect peer and
    /// `AssociateTransitGatewayConnectPeer` for a transit gateway one, and the matching
    /// disassociate removes it again. Registering a transit gateway adds nothing here:
    /// the connections a registered gateway carries are VPN connections of Amazon VPC, of
    /// which this service holds no inventory, so an emulator nobody has registered a
    /// connection with reports the empty telemetry it really holds rather than a
    /// placeholder. No quota of its own bounds the map: a
    /// registration exists only for a stored global network, which the per-account global
    /// network quota bounds, and only once per resource ARN.
    pub telemetry_resources: BTreeMap<String, TelemetryResourceRecord>,
    /// Route analyses by `RouteAnalysisId`, each carrying the global network it was
    /// started on and the paths the analysis found there ([`RouteAnalysisRecord`]). A
    /// `BTreeMap`, so the store has one stable order like every other map here.
    ///
    /// Bounded by [`MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK`] per global network, so
    /// repeated `StartRouteAnalysis` calls cannot grow it without end: an analysis is
    /// addressed only through the global network it was started on, so deleting that global
    /// network takes its analyses with it ([`Self::untie_global_network`]) and the bound
    /// keeps holding across repeated create/delete rounds.
    pub route_analyses: BTreeMap<String, RouteAnalysisRecord>,
    /// The metadata one network resource of a global network's inventory carries, by the
    /// ARN of that resource — the one store `UpdateNetworkResourceMetadata` writes and
    /// [`Self::network_resources_of`] reports in `NetworkResource$Metadata`.
    ///
    /// Keyed by ARN alone rather than by the global network and the ARN together: a network
    /// resource of this inventory belongs to exactly one global network, so its ARN already
    /// identifies the pair the operation addresses, which the write path checks before it
    /// stores anything. An entry holds the WHOLE metadata of its resource, so a later write
    /// replaces it rather than growing it, and a resource nobody has written metadata for
    /// has no entry at all — which is what makes `Metadata` absent rather than empty for it.
    ///
    /// Bounded by the resources that exist: at most one entry per stored resource, which the
    /// per-global-network and per-core-network quotas bound, and the write path drops the
    /// entry of a resource that has since been deleted.
    pub network_resource_metadata: BTreeMap<String, BTreeMap<String, String>>,
    /// This account's organization service-access record, or `None` while no
    /// `StartOrganizationServiceAccessUpdate` has ever been called: this service then
    /// knows of no organization, so `ListOrganizationServiceAccessStatus` reports no
    /// `OrganizationStatus` at all. One record, not a map — see
    /// [`OrganizationServiceAccessRecord`].
    pub organization_service_access: Option<OrganizationServiceAccessRecord>,
    /// Monotonic source for generated resource-id suffixes, so ids are unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

/// The ARN -> stored-tag-list dispatch of this service, stated ONCE: every taggable kind
/// paired with the map whose records carry that kind's tags.
///
/// Both halves of the dispatch are generated from this one mapping — the reader
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::tags_of`] behind
/// `ListTagsForResource` (and behind the "does this ARN still name a stored resource"
/// probe of [`ComAmazonawsNetworkmanagerNetworkmanagerState::put_network_resource_metadata`])
/// and the writer [`ComAmazonawsNetworkmanagerNetworkmanagerState::tags_of_mut`] behind
/// `TagResource` / `UntagResource` — so the two cannot answer different records for the
/// same ARN: a kind is added, and the identity re-check of a scoped ARN changed, in one
/// place for both.
macro_rules! taggable_records {
    ($($kind:ident => $records:ident),+ $(,)?) => {
        impl ComAmazonawsNetworkmanagerNetworkmanagerState {
            /// The stored tag list of the resource `arn` names, or `None` when this service
            /// holds no resource under that ARN.
            ///
            /// Tags live on the resource record itself, so every reader of a resource
            /// (`DescribeGlobalNetworks`, `GetSites`, `GetLinks`, `GetDevices`,
            /// `ListTagsForResource`) answers from the one stored list. The record's own ARN
            /// is re-checked against `arn`, so an ARN carrying a stored id under the wrong
            /// parent names no resource.
            pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
                let parts = parse_resource_arn(arn)?;
                match parts.kind {
                    $(
                        $kind => self
                            .$records
                            .get(parts.id)
                            .filter(|record| record.arn == arn)
                            .map(|record| &record.tags),
                    )+
                    _ => None,
                }
            }

            /// The stored tag list of the resource `arn` names, for writing: the SAME
            /// mapping [`Self::tags_of`] reads through, so a write and a read resolve the
            /// same record for the same ARN.
            pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
                let parts = parse_resource_arn(arn)?;
                match parts.kind {
                    $(
                        $kind => self
                            .$records
                            .get_mut(parts.id)
                            .filter(|record| record.arn == arn)
                            .map(|record| &mut record.tags),
                    )+
                    _ => None,
                }
            }
        }
    };
}

taggable_records!(
    GLOBAL_NETWORK_KIND => global_networks,
    SITE_KIND => sites,
    LINK_KIND => links,
    DEVICE_KIND => devices,
    CONNECTION_KIND => connections,
    CORE_NETWORK_KIND => core_networks,
    ATTACHMENT_KIND => attachments,
    PEERING_KIND => peerings,
    CONNECT_PEER_KIND => connect_peers,
);

/// The key one network resource is listed under: its resource TYPE, then its id.
///
/// A network-resource listing spans several kinds at once, and the ids of two kinds are
/// drawn from different prefixes, so the type leads the key: the listing is then ordered
/// by type and then by id, every key is unique, and a page token (which names a key
/// offset, see [`paginate`]) can resume inside it.
pub fn network_resource_key(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type}/{resource_id}")
}

/// The key one relationship is listed under: the ARN it leads FROM, then the ARN it names.
///
/// A relationship has no identity beyond the ordered pair of ARNs it joins, so the pair is
/// its key: the listing is then ordered by the resource a relationship leads from and then
/// by the one it reaches, every key is unique, a pair reached from two stored ties is held
/// once, and a page token (which names a key offset, see [`paginate`]) can resume inside
/// it. The separator is one no ARN carries, so no two pairs can produce the same key.
pub fn relationship_key(from: &str, to: &str) -> String {
    format!("{from}|{to}")
}

/// How many entries the metadata map of one network resource may carry.
///
/// CHOSEN, like [`MAX_RESOURCE_POLICIES`], and for the same reason: an entry of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resource_metadata`] holds the
/// map a request carried, so without a bound one request could grow this emulator's store
/// without end. AWS publishes no per-resource limit for this map, and the model bounds only
/// each key and each value inside it (a `ConstrainedString` of 0 to 256 characters,
/// `smithy_query shape=NetworkResourceMetadataMap`), so the number here is the emulator's
/// own: the same one as this service's per-resource key/value quota for tags
/// ([`MAX_TAGS_PER_RESOURCE`]), the closest limit AWS does publish.
///
/// A map carrying more entries is refused with `UpdateNetworkResourceMetadata`'s modelled
/// `ValidationException` (HTTP 400) naming [`FIELD_METADATA`], and stores nothing.
pub const MAX_METADATA_ENTRIES: usize = MAX_TAGS_PER_RESOURCE;

/// Check a requested metadata map of `given` entries against [`MAX_METADATA_ENTRIES`].
pub fn check_metadata_entries(given: usize) -> Result<(), ValidationException> {
    if given > MAX_METADATA_ENTRIES {
        return Err(field_rejection(
            FIELD_METADATA,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "a network resource's metadata may carry at most {MAX_METADATA_ENTRIES} entries, but {given} were given"
            ),
        ));
    }
    Ok(())
}

/// The stored metadata of one network resource as the modelled `NetworkResourceMetadataMap`
/// is carried on the wire.
///
/// The store keeps the map ordered by key, like every other map of this state, while the
/// modelled shape is a plain map whose order the protocol does not fix, so this is the one
/// place the two representations meet.
pub fn metadata_shape(
    stored: &BTreeMap<String, String>,
) -> ::std::collections::HashMap<String, String> {
    stored
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The members every network resource of this service carries, whatever its kind: the
/// account that holds it, its type, id and ARN, its tags, and the instant the answer read
/// its definition. The caller fills `Definition` (and, where the kind has one,
/// `CoreNetworkId` and `AwsRegion`) from the record itself.
///
/// `AwsRegion` defaults to absent here and is filled in by the caller for every kind whose
/// own reader names the single Region the resource lives in — the edge location of an
/// attachment, a peering or a Connect peer, all typed `ExternalRegionCode` exactly as
/// `NetworkResource$AwsRegion` is (`smithy_query shape=NetworkResource,Attachment`). A kind
/// whose reader names no single Region keeps the default: the resource is held under the
/// global network itself rather than at one edge (a site, a link, a device, a connection),
/// or it spans several Regions at once and reports them as a LIST (a core network's
/// `Edges`, a Direct Connect gateway attachment's `EdgeLocations`), which a single-valued
/// member cannot carry.
///
/// One member is deliberately left absent, because nothing in the store holds it:
///
/// * `RegisteredGatewayArn` — only a resource reached THROUGH a registered gateway carries
///   one, and every resource of this inventory is one this service holds in its own right.
///   Registering a transit gateway with a global network records that registration alone:
///   the resources reached through the gateway belong to Amazon VPC, of which this service
///   holds no inventory. Associating a customer gateway with a device does not register one
///   either: it ties a gateway the global network already holds to a device of it.
///
/// `Metadata` defaults to absent here too and is filled in by
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resources_of`] for a resource
/// whose ARN the metadata store holds an entry for, whatever its kind.
fn network_resource(
    resource_type: &str,
    resource_id: &str,
    resource_arn: &str,
    tags: &[Tag],
    now: SystemTime,
) -> NetworkResource {
    NetworkResource {
        registered_gateway_arn: None,
        core_network_id: None,
        aws_region: None,
        account_id: Some(::engine::emulator_account_id().to_string()),
        resource_type: Some(resource_type.to_string()),
        resource_id: Some(resource_id.to_string()),
        resource_arn: Some(resource_arn.to_string()),
        definition: None,
        // "The time that the resource definition was retrieved"
        // (`smithy_query shape=NetworkResource`): the definition below is computed from
        // the record as this call reads it, so the instant is this call's.
        definition_timestamp: Some(epoch_seconds(now)),
        tags: Some(tags.to_vec()),
        metadata: None,
    }
}

/// The JSON a `NetworkResource$Definition` carries for one stored resource: the resource's
/// own Describe output, rendered by THIS SERVICE'S OWN restJson1 codec from the very shape
/// that resource's reader answers.
///
/// "Information about the resource, in JSON format. Network Manager gets this information
/// by describing the resource using its Describe API call"
/// (`smithy_query shape=NetworkResource`), so a definition is exactly what `GetSites` /
/// `GetLinks` / `GetDevices` / `GetCoreNetwork` / `GetVpcAttachment` /
/// `GetConnectAttachment` put on the wire for that resource. Rendering through the codec
/// (`JsonCodec::rest_json`, the one the protocol itself serialises responses with) rather
/// than mapping member by member is what keeps the two readers of one resource in step:
/// the member spelling, the `@jsonName` overrides and the wire form of every timestamp are
/// the protocol's, so a model change cannot drift the definition away from the reader.
///
/// The resource's tags appear here as well as in the enclosing `NetworkResource$Tags`,
/// because the definition is the Describe output as it stands and both come from the one
/// stored tag list.
///
/// A value the codec cannot render answers `None`, so the member is absent rather than
/// carrying a definition this service made up.
fn resource_definition(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<String> {
    use ::aws_smithy_schema::codec::Codec;
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored resource, so its network-resource definition is absent"
        );
        return None;
    }
    Some(serializer.into_string())
}

/// The resource an ARN of this service names: the ARN resource type (which is also the
/// resource-id prefix this service mints, see [`GLOBAL_NETWORK_KIND`]), the id, and —
/// for a kind that exists only inside a parent — the id of that parent.
#[derive(Debug, Clone, Copy)]
pub struct ArnParts<'a> {
    /// The ARN resource type, e.g. `global-network`.
    pub kind: &'a str,
    /// The resource id inside that type: the LAST path segment of the ARN.
    pub id: &'a str,
    /// The id the ARN names ahead of `id`, for a resource whose name carries its owner
    /// (see [`ComAmazonawsNetworkmanagerNetworkmanagerState::scoped_resource_arn`]);
    /// `None` for an ARN whose resource path is just `<kind>/<id>`.
    pub scope: Option<&'a str>,
}

/// Split a Network Manager resource ARN into its kind and id.
///
/// Every ARN this service mints reads
/// `arn:<partition>:networkmanager::<account>:<kind>/<id>`, or
/// `arn:<partition>:networkmanager::<account>:<kind>/<owner-id>/<id>` for a kind that
/// exists only inside an owning resource (see
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::scoped_resource_arn`]). The resource's
/// own id is the LAST path segment either way, which is the id a lookup keys on and the
/// `ResourceId` a refusal reports. Answers `None` when the string is not an ARN of this
/// service in one of those forms at all — the caller refuses that as a
/// `ValidationException` on the ARN member, which is a different answer from a well-formed
/// ARN naming a resource that does not exist.
///
/// Only the form is judged here, not which account, partition or region the ARN carries,
/// nor whether the owner it names really owns the resource: an ARN of this service that
/// names another account's resource, carries a region this global service does not use, or
/// names the wrong owner, parses and then matches no stored record, so the caller answers
/// it as NOT FOUND rather than as a malformed request.
pub fn parse_resource_arn(arn: &str) -> Option<ArnParts<'_>> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // Partition and account are always present in an ARN; the region segment of this
    // global service is empty, so it is the only one allowed to be.
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    let _region = fields.next()?;
    if fields.next()?.is_empty() {
        return None;
    }
    let (kind, path) = fields.next()?.split_once('/')?;
    // The resource's own id is the last path segment; anything ahead of it names the
    // resource that owns it.
    let (scope, id) = match path.split_once('/') {
        Some((scope, id)) => (Some(scope), id),
        None => (None, path),
    };
    if kind.is_empty() || id.is_empty() || scope.is_some_and(str::is_empty) || id.contains('/') {
        return None;
    }
    Some(ArnParts { kind, id, scope })
}

/// The `Reason` a `ValidationException` of this service carries when one input MEMBER is
/// at fault — whatever is wrong with the value it carried — and the one it carries when a
/// DOCUMENT this service has to read was not readable as the format it is written in (a
/// policy document that is no JSON: [`validate_resource_policy_document`]). A member whose
/// value this service rejects is the first of the two, not the second, however the value
/// failed. Both are values the modelled enumeration lists
/// (`ops::types::VALIDATION_EXCEPTION_REASON_VALUES`: `CannotParse`,
/// `FieldValidationFailed`, `Other`, `UnknownOperation`).
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
pub const REASON_CANNOT_PARSE: &str = "CannotParse";

/// The input member the resource-policy operations name in a refusal: the ARN of the
/// resource whose policy is addressed, and the policy document itself.
pub const FIELD_RESOURCE_ARN: &str = "ResourceArn";
/// See [`FIELD_RESOURCE_ARN`].
pub const FIELD_POLICY_DOCUMENT: &str = "PolicyDocument";
/// The input member every operation that stores tags names in a refusal of them: the
/// `Tags` list itself, as [`validate_tags`] and [`tag_problem_rejection`] both report it.
pub const FIELD_TAGS: &str = "Tags";
/// See [`FIELD_RESOURCE_ARN`]: the member carrying the resource metadata an update of one
/// network resource's metadata stores, as [`check_metadata_entries`] reports it.
pub const FIELD_METADATA: &str = "Metadata";

/// The modelled `ValidationException` (HTTP 400) for a refusal of the single input member
/// `member`, carrying `reason` and one `Fields` entry explaining `message`.
pub fn field_rejection(member: &str, reason: &str, message: String) -> ValidationException {
    ValidationException {
        message: message.clone(),
        reason: Some(reason.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// How many ids ONE id-list filter of a reporting operation may name.
///
/// The bound is the one the member's OWN documentation publishes, so it reaches exactly
/// the id-list filters whose documentation carries the sentence that publishes it: "The IDs
/// of one or more global networks. The maximum is 10." on
/// `DescribeGlobalNetworksRequest$GlobalNetworkIds`, and the same sentence on
/// `GetSitesRequest$SiteIds` ("One or more site IDs. The maximum is 10."),
/// `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds`,
/// `GetTransitGatewayRegistrationsRequest$TransitGatewayArns` and
/// `GetCustomerGatewayAssociationsRequest$CustomerGatewayArns` ("One or more customer
/// gateway Amazon Resource Names (ARNs). The maximum is 10.": `smithy_query query="doc"
/// shape="<Request>" member="<Member>"`). The modelled list shapes do NOT express it as
/// `@length` — `smithy_query
/// shape=GlobalNetworkIdList,SiteIdList,LinkIdList,DeviceIdList,CustomerGatewayArnList`
/// carries a length on the member STRING only — so the pre-handler constraint walk
/// cannot enforce it and [`check_filter_ids`] does, exactly as the device and link
/// descriptor maxima ([`MAX_DEVICE_DESCRIPTOR_CHARS`]) are enforced in handler code.
///
/// The id-list filters whose documentation publishes NO maximum do not take this bound and
/// get no such refusal invented for them — `GetConnectionsRequest$ConnectionIds` ("One or
/// more connection IDs."), `GetConnectPeerAssociationsRequest$ConnectPeerIds` and
/// `GetTransitGatewayConnectPeerAssociationsRequest$TransitGatewayConnectPeerArns`: each of
/// those listings serves a filter of any length: each selects by ONE pass over the records
/// it holds, testing them against the ids the request named, so what the selection costs is
/// bounded by what the store holds rather than by the length of the caller's list.
pub const MAX_FILTER_IDS: usize = 10;

/// Check one id-list filter against [`MAX_FILTER_IDS`].
///
/// Answers `Ok(())` when the filter names at most that many ids (an absent filter is an
/// empty one, which always passes), otherwise the modelled `ValidationException`
/// (HTTP 400) naming `member` — the input member that carried the list — so a request past
/// the documented maximum is refused before any listing is computed. Shared by every
/// reporting operation whose id-list filter member PUBLISHES that maximum (see
/// [`MAX_FILTER_IDS`] for which do and which do not), so one rule reaches all of them.
pub fn check_filter_ids(member: &str, ids: &[String]) -> Result<(), ValidationException> {
    check_filter_list(member, ids.len(), "name", "ids")
}

/// Check one VALUE-match filter of a reporting operation against [`MAX_FILTER_IDS`].
///
/// The sibling of [`check_filter_ids`] for a filter that selects on a VALUE rather than on
/// a resource id: the match lists of a listing of routing information, and each value list
/// of its next-hop filter map. Those lists are modelled as `ConstrainedStringList` /
/// `FilterValues`, which constrain the member STRING only and carry no length of their own
/// (`smithy_query shape=ConstrainedStringList,FilterValues`), so — exactly as for the id
/// lists above — the pre-handler constraint walk cannot bound them and the handler does.
///
/// The bound MATTERS beyond tidiness: selecting a listing compares every entry it holds
/// against every value a filter carries, so the work is the product of the two, and the
/// count is what caps the caller's factor of it — a value-match filter selects by COMPARING
/// values rather than by looking an id up, so there is no keyed lookup to serve it with.
/// It is the same number this service allows a documented id-list filter — but unlike that
/// one it is CHOSEN rather than documented: no match list of this service carries a
/// published maximum, so a request real AWS would plausibly accept is refused here past it.
pub fn check_filter_values(member: &str, values: &[String]) -> Result<(), ValidationException> {
    check_filter_list(member, values.len(), "carry", "values")
}

/// Whether the value-match filter `values` holds the value `held`.
///
/// The ONE membership test a value-match filter of this service is: a filter selects a
/// record when the attribute it names carries one of the filter's values, so every handler
/// that narrows a listing by such a filter asks this rather than spelling the scan again.
/// The list is bounded by [`check_filter_values`], which is what keeps the scan cheap.
pub fn filter_holds(values: &[String], held: &str) -> bool {
    values.iter().any(|value| value == held)
}

/// The shared body of the two above: ONE statement of the bound and of the refusal a list
/// past it carries, so an id-list filter and a value-match filter cannot drift apart in
/// either. `verb` and `things` are how the refusal reads about the list at fault ("name …
/// ids", "carry … values"); `given` is how long that list was.
fn check_filter_list(
    member: &str,
    given: usize,
    verb: &str,
    things: &str,
) -> Result<(), ValidationException> {
    if given > MAX_FILTER_IDS {
        return Err(field_rejection(
            member,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "{member} may {verb} at most {MAX_FILTER_IDS} {things}, but {given} were given"
            ),
        ));
    }
    Ok(())
}

/// Check one ARN-LIST filter of a reporting operation: every value it carries must read as
/// an ARN of the resource type the member documents.
///
/// `well_formed` is the walk that judges that form — the SAME one the operations that WRITE
/// a tie to such a resource judge their own ARN member with — and `refusal` builds the field
/// failure those operations answer for a string that is no such ARN, named here on the
/// filter member that carried it. A value that passes `well_formed` and names nothing stored
/// is NOT an error: it selects nothing, which is what a filter does.
///
/// One statement of that distinction for every ARN-list filter of this service, beside the
/// one [`check_filter_ids`] makes about the LENGTH of such a filter, so a malformed ARN
/// cannot be refused by the operation that writes a tie and quietly served by the listing
/// that reports it.
pub fn check_filter_arns(
    member: &str,
    arns: &[String],
    well_formed: impl Fn(&str) -> bool,
    refusal: impl FnOnce(&str) -> ValidationException,
) -> Result<(), ValidationException> {
    if arns.iter().all(|arn| well_formed(arn.as_str())) {
        return Ok(());
    }
    Err(refusal(member))
}

/// The modelled `ValidationException` (HTTP 400) an operation answers when the string in
/// its ARN member `member` is not an ARN of this service at all.
///
/// A string that does not parse as `arn:<partition>:networkmanager::<account>:<type>/<id>`
/// (see [`parse_resource_arn`]) names no resource of this service, which is a FIELD
/// failure on the member that carried it — a different answer from a well-formed ARN that
/// names nothing stored.
///
/// The wording is the one the tagging operations of this service already answer for the
/// same input class (`ops/list_tags_for_resource.rs`, `ops/tag_resource.rs`,
/// `ops/untag_resource.rs`), so one malformed ARN reads the same whichever operation it
/// arrived on.
pub fn malformed_resource_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a networkmanager resource ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>".to_string(),
        }]),
    }
}

/// Whether every character of `text` is a lowercase hex digit — the alphabet every id the
/// cross-service ARN patterns of this model spell out is drawn from (`[0-9a-f]` in
/// `DirectConnectGatewayArn`'s and `VpcArn`'s patterns).
fn is_lowercase_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// What the Region segment of a cross-service ARN may carry, which is the one thing the
/// modelled patterns of the two forms below disagree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnRegion {
    /// The segment must be EMPTY, as `DirectConnectGatewayArn`'s pattern has it
    /// (`:directconnect::`).
    Absent,
    /// The segment may carry any Region, or none, as the EC2 patterns have it
    /// (`:ec2:[^:]{0,63}:`).
    Any,
}

/// Split the ARN of a resource of ANOTHER service into its kind and id, or `None` when the
/// string is not one in the form asked for.
///
/// Every cross-service ARN this model types spells one skeleton —
/// `arn:<partition>:<namespace>:<region>:<account>:<kind>/<id>`, with the partition 1 to 63
/// characters and the Region and account at most 63
/// (`^arn:[^:]{1,63}:directconnect::[^:]{0,63}:dx-gateway\/…$`, `smithy_query
/// shape=DirectConnectGatewayArn`; `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/…$`,
/// `smithy_query shape=VpcArn`) — so ONE walk serves every such form and two of them cannot
/// drift apart. `region` is what that form's pattern says about the Region segment, `kind`
/// the resource type it names and `is_id` the shape of the id inside it.
///
/// Which account, partition or Region the ARN carries is not judged beyond those widths,
/// exactly as [`parse_resource_arn`] does not judge them for an ARN of this service: the
/// resource belongs to another service, so whether it EXISTS cannot be known here.
fn parse_foreign_arn<'a>(
    arn: &'a str,
    namespace: &str,
    region: ArnRegion,
    kind: &str,
    is_id: fn(&str) -> bool,
) -> Option<ArnParts<'a>> {
    /// The widest partition, Region and account those patterns admit.
    const MAX_SEGMENT: usize = 63;

    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if partition.is_empty() || partition.len() > MAX_SEGMENT {
        return None;
    }
    if fields.next()? != namespace {
        return None;
    }
    let found_region = fields.next()?;
    if found_region.len() > MAX_SEGMENT || (region == ArnRegion::Absent && !found_region.is_empty())
    {
        return None;
    }
    if fields.next()?.len() > MAX_SEGMENT {
        return None;
    }
    let (found_kind, id) = fields.next()?.split_once('/')?;
    if found_kind != kind || !is_id(id) {
        return None;
    }
    // A resource of another service is identified account-wide here: none of the forms this
    // walk serves names an owning resource ahead of the id, so the resource path is just
    // `<kind>/<id>` and the scope [`ArnParts`] carries is `None`.
    Some(ArnParts {
        kind: found_kind,
        id,
        scope: None,
    })
}

/// The ARN namespace of the resources of ANOTHER service this service's Cloud WAN
/// operations take as INPUT: a transit gateway, one of its route tables and a managed
/// prefix list are all Amazon VPC (EC2) resources, so their ARNs read
/// `arn:<partition>:ec2:<region>:<account>:<type>/<id>` — the form the members' own
/// documentation gives as an example ("arn:aws:ec2:us-west-2:123456789012:transit-gateway-route-table/tgw-rtb-9876543210123456",
/// `smithy_query query="doc" shape="CreateTransitGatewayRouteTableAttachmentRequest"
/// member="TransitGatewayRouteTableArn"`). This service mints nothing for them.
const EC2_NAMESPACE: &str = "ec2";
/// The ARN resource type of a managed prefix list, and the prefix of its id — the `pl-` of
/// `prefix-list/pl-0123456789abcdef1`.
pub const PREFIX_LIST_KIND: &str = "prefix-list";
/// See [`PREFIX_LIST_KIND`].
const PREFIX_LIST_ID_PREFIX: &str = "pl-";

/// Whether `id` reads as an EC2 resource id of the type `prefix` names: that prefix plus
/// the 8 to 17 lowercase hex digits every EC2 resource id the model DOES type carries
/// (`vpc-[0-9a-f]{8,17}` in `VpcArn`'s pattern, `vpn-[0-9a-f]{8,17}` in
/// `VpnConnectionArn`'s).
fn is_ec2_hex_id(id: &str, prefix: &str) -> bool {
    /// The width an EC2 resource id's hex suffix may have, from those patterns.
    const WIDTHS: ::std::ops::RangeInclusive<usize> = 8..=17;

    match id.strip_prefix(prefix) {
        Some(digits) => WIDTHS.contains(&digits.len()) && is_lowercase_hex(digits),
        None => false,
    }
}

/// Whether `id` reads as a managed prefix list id (see [`is_ec2_hex_id`]).
fn is_prefix_list_id(id: &str) -> bool {
    is_ec2_hex_id(id, PREFIX_LIST_ID_PREFIX)
}

/// Split a managed prefix list ARN into its kind and id, or `None` when the string is not
/// one at all — the caller refuses that as a `ValidationException` on the ARN member
/// ([`malformed_prefix_list_arn`]).
///
/// `PrefixListArn` is the one cross-service ARN this model leaves free-form (len 0..500,
/// pattern `^[\s\S]*$`: `smithy_query shape=PrefixListArn`), so the form is judged here
/// rather than by the pre-handler constraint walk. Every EC2 ARN the model DOES type spells
/// the skeleton [`parse_foreign_arn`] walks — `VpcArn` carries
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` (`smithy_query
/// shape=VpcArn`) — and a prefix list ARN is that skeleton with the `prefix-list` resource
/// type and a `pl-` id ([`is_prefix_list_id`]).
///
/// Whether the prefix list EXISTS is not judged: it belongs to Amazon VPC, which this
/// service holds no record of, so a well-formed ARN is stored as the association's subject.
pub fn parse_prefix_list_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        EC2_NAMESPACE,
        ArnRegion::Any,
        PREFIX_LIST_KIND,
        is_prefix_list_id,
    )
}

/// The modelled `ValidationException` (HTTP 400) a prefix-list association operation
/// answers when the string in its member `member` is not a managed prefix list ARN at all
/// (see [`parse_prefix_list_arn`]).
///
/// Like [`malformed_resource_arn`]: a string that names no prefix list is a FIELD failure
/// on the member that carried it, a different answer from a well-formed ARN that names no
/// association this core network holds.
pub fn malformed_prefix_list_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a managed prefix list ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{EC2_NAMESPACE}:<region>:<account>:{PREFIX_LIST_KIND}/{PREFIX_LIST_ID_PREFIX}<8 to 17 hex digits>"
            ),
        }]),
    }
}

/// The ARN resource type of a CUSTOMER GATEWAY, and the prefix of its id — the `cgw-` of
/// `customer-gateway/cgw-0123456789abcdef1`. An Amazon VPC (EC2) resource, like the managed
/// prefix list above, and one of [`NETWORK_RESOURCE_TYPES`].
pub const CUSTOMER_GATEWAY_KIND: &str = "customer-gateway";
/// See [`CUSTOMER_GATEWAY_KIND`].
const CUSTOMER_GATEWAY_ID_PREFIX: &str = "cgw-";

/// Whether `id` reads as a customer gateway id (see [`is_ec2_hex_id`]).
fn is_customer_gateway_id(id: &str) -> bool {
    is_ec2_hex_id(id, CUSTOMER_GATEWAY_ID_PREFIX)
}

/// Split a customer gateway ARN into its kind and id, or `None` when the string is not one
/// at all — the caller refuses that as a `ValidationException` on the ARN member
/// ([`malformed_customer_gateway_arn`]).
///
/// `CustomerGatewayArn` is modelled free-form (len 0..500, pattern `^[\s\S]*$`:
/// `smithy_query shape=CustomerGatewayArn`), like `PrefixListArn`, so the form is judged
/// here rather than by the pre-handler constraint walk. A customer gateway is an Amazon VPC
/// resource, so its ARN is the EC2 skeleton [`parse_foreign_arn`] walks — the one
/// `VpnConnectionArn` spells out, `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
/// (`smithy_query shape=VpnConnectionArn`) — with the `customer-gateway` resource type and
/// a `cgw-` id.
///
/// Whether the gateway EXISTS is not judged: it belongs to Amazon VPC, which this service
/// holds no record of, so a well-formed ARN is stored as the association's subject.
pub fn parse_customer_gateway_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        EC2_NAMESPACE,
        ArnRegion::Any,
        CUSTOMER_GATEWAY_KIND,
        is_customer_gateway_id,
    )
}

/// The modelled `ValidationException` (HTTP 400) a customer gateway association operation
/// answers when the string in its member `member` is not a customer gateway ARN at all
/// (see [`parse_customer_gateway_arn`]).
///
/// Like [`malformed_prefix_list_arn`]: a string that names no customer gateway is a FIELD
/// failure on the member that carried it, a different answer from a well-formed ARN that
/// names no association this global network holds.
pub fn malformed_customer_gateway_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a customer gateway ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{EC2_NAMESPACE}:<region>:<account>:{CUSTOMER_GATEWAY_KIND}/{CUSTOMER_GATEWAY_ID_PREFIX}<8 to 17 hex digits>"
            ),
        }]),
    }
}

/// The ARN namespace and resource type of the resource a resource policy of this service
/// is attached to: a Direct Connect gateway.
///
/// A resource policy is attached to a resource that can CARRY one, and no resource Network
/// Manager itself owns can: a recording of `PutResourcePolicy` against a core network — the
/// central resource of this service — is refused for every well-formed policy document.
/// The model agrees that the subject of these three operations is not a resource of this
/// service: `TagResource` and `ListTagsForResource` carry the very same `ResourceArn`
/// member and model `ResourceNotFoundException` (HTTP 404) for an ARN naming nothing,
/// while `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` model no
/// not-found failure at all (`smithy_query query="operation"
/// operation="PutResourcePolicy,TagResource"`), so they never resolve the ARN against this
/// service's resources.
///
/// The one resource of ANOTHER service this API surface joins to a core network — across
/// accounts, so it needs a grant from its own owner — is the Direct Connect gateway
/// `CreateDirectConnectGatewayAttachment` takes, and its ARN is the one cross-service ARN
/// the model types: `DirectConnectGatewayArn` carries
/// `pattern ^arn:[^:]{1,63}:directconnect::[^:]{0,63}:dx-gateway\/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}$`
/// (`smithy_query shape=DirectConnectGatewayArn`). That form is what
/// [`parse_direct_connect_gateway_arn`] accepts, and an ARN of any other resource type is
/// refused by `PutResourcePolicy` with [`resource_not_policy_attachable`].
const DIRECT_CONNECT_NAMESPACE: &str = "directconnect";
/// See [`DIRECT_CONNECT_NAMESPACE`].
pub const DIRECT_CONNECT_GATEWAY_KIND: &str = "dx-gateway";

/// Whether `id` reads as a Direct Connect gateway id: the five lowercase-hex groups of
/// 8-4-4-4-12 digits the modelled `DirectConnectGatewayArn` pattern spells out (see
/// [`DIRECT_CONNECT_NAMESPACE`]).
fn is_direct_connect_gateway_id(id: &str) -> bool {
    /// The widths of the groups, in order, from that pattern.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = id.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !is_lowercase_hex(group) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Split a Direct Connect gateway ARN into its kind and id, or `None` when the string is
/// not one in the form the model types (see [`DIRECT_CONNECT_NAMESPACE`]).
///
/// The region segment must be EMPTY and the account segment may be, exactly as that
/// pattern has them; which account owns the gateway is not judged here, since the point of
/// a resource policy is that the gateway belongs to another account.
pub fn parse_direct_connect_gateway_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        DIRECT_CONNECT_NAMESPACE,
        ArnRegion::Absent,
        DIRECT_CONNECT_GATEWAY_KIND,
        is_direct_connect_gateway_id,
    )
}

/// The resource an ARN names when that resource may CARRY a resource policy, or `None`
/// when it may not — see [`DIRECT_CONNECT_NAMESPACE`] for which resource types may.
///
/// `PutResourcePolicy` attaches a policy only to such a resource; every other ARN, this
/// service's own resources included, is refused with [`resource_not_policy_attachable`].
/// This is the ONE place that class is decided, so a second attachable resource type —
/// should one be recorded as carrying a policy — is added here as another arm and reaches
/// all three operations at once.
pub fn parse_policy_attachable_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_direct_connect_gateway_arn(arn)
}

/// The resource an ARN names when the resource-policy operations ADDRESS it at all: one of
/// this service's own resources (which carries no policy, so it is reported as holding
/// none) or a policy-attachable resource of another service.
///
/// `GetResourcePolicy` and `DeleteResourcePolicy` answer for either — a resource that
/// cannot carry a policy simply holds none — so only a string that names neither is
/// refused, with [`malformed_policy_resource_arn`].
pub fn parse_policy_resource_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_resource_arn(arn).or_else(|| parse_policy_attachable_arn(arn))
}

/// The modelled `ValidationException` (HTTP 400) the resource-policy operations answer when
/// the string in `member` names no resource they address at all (see
/// [`parse_policy_resource_arn`]).
///
/// Like [`malformed_resource_arn`], but naming BOTH forms these three operations read: an
/// ARN of this service, and the policy-attachable form of another service's resource — a
/// caller whose gateway ARN is slightly wrong is told the form it missed rather than the
/// one it was not reaching for.
pub fn malformed_policy_resource_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not an ARN of a resource a resource policy is read or written for"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{ARN_NAMESPACE}::<account>:<type>/<id>, or arn:<partition>:{DIRECT_CONNECT_NAMESPACE}::<account>:{DIRECT_CONNECT_GATEWAY_KIND}/<id> for a resource that can carry a policy"
            ),
        }]),
    }
}

/// The message a refusal of a resource that cannot carry a resource policy carries.
///
/// A fixed, request-INDEPENDENT constant, like [`RESOURCE_NOT_FOUND_MESSAGE`] and for the
/// same reason: a recording of `PutResourcePolicy` against a resource of this service
/// answers the identical string for every well-formed policy document put at it — the
/// document's principal, its actions and whether the resource already holds a policy of
/// its own change nothing — so the wording carries no per-request detail. The offending
/// member travels in `Fields` instead, where [`resource_not_policy_attachable`] names it.
///
/// What this refusal PINS is the HTTP 400, the `ValidationException` code and the
/// `FieldValidationFailed` reason; the wording is this service's own text, so changing it
/// is not a behaviour change.
pub const INVALID_RESOURCE_POLICY_MESSAGE: &str = "Invalid policy.";

/// The modelled `ValidationException` (HTTP 400) `PutResourcePolicy` answers when the ARN
/// in `member` names a well-formed resource that cannot carry a resource policy — this
/// service's own resources among them (see [`DIRECT_CONNECT_NAMESPACE`]).
///
/// A FIELD failure on the member that carried the ARN, like [`malformed_resource_arn`]:
/// nothing is stored, so the resource keeps reporting no policy afterwards.
pub fn resource_not_policy_attachable(member: &str) -> ValidationException {
    ValidationException {
        message: INVALID_RESOURCE_POLICY_MESSAGE.to_string(),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{DIRECT_CONNECT_NAMESPACE}::<account>:{DIRECT_CONNECT_GATEWAY_KIND}/<id>"
            ),
        }]),
    }
}

/// How many resource policies this account may hold here, and how long one policy
/// document may be, in characters.
///
/// Both are CHOSEN, like [`MAX_GLOBAL_NETWORKS`], and for the same reason: the
/// `resource_policies` store is keyed by the ARN a request carries, so without a bound a
/// caller could grow this emulator's store without end. AWS publishes an account-level
/// resource-policy quota (adjustable per account) and a per-policy size limit for its
/// resource-based policies, and the model carries neither number — the document is an
/// opaque `SynthesizedJsonResourcePolicyDocument` of length 0..10000000 — so the numbers
/// here are the emulator's own: 20,480 characters is the size AWS's resource-based policy
/// limits are commonly stated at, well inside the modelled maximum.
///
/// Asking to store a policy for one more ARN than [`MAX_RESOURCE_POLICIES`] is refused
/// with `PutResourcePolicy`'s modelled `ServiceQuotaExceededException` (HTTP 402) and
/// stores nothing; replacing the policy of an ARN already held never trips it. A document
/// longer than [`MAX_RESOURCE_POLICY_CHARS`] is refused with the same operation's
/// `ValidationException` (HTTP 400), naming [`FIELD_POLICY_DOCUMENT`].
pub const MAX_RESOURCE_POLICIES: usize = 100;
/// See [`MAX_RESOURCE_POLICIES`].
pub const MAX_RESOURCE_POLICY_CHARS: usize = 20_480;

/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota [`MAX_RESOURCE_POLICIES`] bounds.
pub const LIMIT_RESOURCE_POLICIES_PER_ACCOUNT: &str = "resource-policies-per-account";

/// The message a refusal past the [`MAX_RESOURCE_POLICIES`] quota carries: `resulting` is
/// how many ARNs would hold a policy if the request were accepted.
pub fn too_many_resource_policies_message(resulting: usize) -> String {
    format!(
        "an account may hold resource policies for at most {MAX_RESOURCE_POLICIES} resources, but the request would leave {resulting}"
    )
}

/// The members of a resource policy document this service judges.
///
/// A resource policy is an IAM JSON policy document, so its grammar is IAM's (AWS IAM
/// User Guide, "IAM JSON policy element reference"): the document is a JSON object whose
/// `Statement` is one statement object or a list of them, and every statement names its
/// `Effect`, which is either `Allow` or `Deny`. The model types the document as an opaque
/// string carrying `mediaType: application/json` (`smithy_query
/// shape=SynthesizedJsonResourcePolicyDocument`), so these names come from that grammar
/// rather than from the model.
const POLICY_STATEMENT: &str = "Statement";
/// See [`POLICY_STATEMENT`].
const POLICY_EFFECT: &str = "Effect";
/// See [`POLICY_STATEMENT`]: the two values IAM's `Effect` element allows.
pub const POLICY_EFFECT_VALUES: [&str; 2] = ["Allow", "Deny"];

/// Check the document a `PutResourcePolicy` asks to attach to a resource.
///
/// The document's SIZE is judged first, against [`MAX_RESOURCE_POLICY_CHARS`], so an
/// over-long document is refused before it is parsed. Then its SHAPE, against the IAM
/// policy grammar ([`POLICY_STATEMENT`]): it must parse as JSON, its root must be an
/// object, it must carry at least one statement, and every statement must be an object
/// naming an `Effect` of `Allow` or `Deny`. Whom the statements grant what is the deeper
/// semantics of a policy and is not interpreted here — this service only stores and
/// reports the document it accepted.
///
/// A document that fails any of these is refused with a `ValidationException` (HTTP 400)
/// naming [`FIELD_POLICY_DOCUMENT`] and nothing is stored, so a rejected request leaves
/// the resource's policy exactly as it was.
pub fn validate_resource_policy_document(document: &str) -> Result<(), ValidationException> {
    let reject = |reason: &str, message: String| {
        field_rejection(FIELD_POLICY_DOCUMENT, reason, message)
    };

    let length = document.chars().count();
    if length > MAX_RESOURCE_POLICY_CHARS {
        return Err(reject(
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "a resource policy document must be at most {MAX_RESOURCE_POLICY_CHARS} characters, but {length} were given"
            ),
        ));
    }

    let parsed: ::serde_json::Value = ::serde_json::from_str(document).map_err(|problem| {
        reject(
            REASON_CANNOT_PARSE,
            format!("the resource policy document is not valid JSON: {problem}"),
        )
    })?;
    let root = parsed.as_object().ok_or_else(|| {
        reject(
            REASON_FIELD_VALIDATION_FAILED,
            "the resource policy document must be a JSON object".to_string(),
        )
    })?;

    // IAM allows `Statement` to be a single statement object as well as a list of them.
    let statements: Vec<&::serde_json::Value> = match root.get(POLICY_STATEMENT) {
        Some(::serde_json::Value::Array(entries)) => entries.iter().collect(),
        Some(single @ ::serde_json::Value::Object(_)) => vec![single],
        Some(_) => {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("{POLICY_STATEMENT} must be a statement object or a list of them"),
            ));
        }
        None => {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("the resource policy document must carry a {POLICY_STATEMENT}"),
            ));
        }
    };
    if statements.is_empty() {
        return Err(reject(
            REASON_FIELD_VALIDATION_FAILED,
            format!("{POLICY_STATEMENT} must carry at least one statement"),
        ));
    }

    for (index, statement) in statements.iter().enumerate() {
        let entry = statement.as_object().ok_or_else(|| {
            reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("{POLICY_STATEMENT}[{index}] must be a JSON object"),
            )
        })?;
        let effect = entry
            .get(POLICY_EFFECT)
            .and_then(::serde_json::Value::as_str);
        if !effect.is_some_and(|effect| POLICY_EFFECT_VALUES.contains(&effect)) {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!(
                    "{POLICY_STATEMENT}[{index}].{POLICY_EFFECT} must be one of {}",
                    POLICY_EFFECT_VALUES.join(", ")
                ),
            ));
        }
    }

    Ok(())
}

/// Why a requested tag set or key list was refused: the input member to blame and the
/// reason, which the caller reports as a `ValidationException` (HTTP 400).
#[derive(Debug, Clone)]
pub struct TagRejection {
    /// The input member the refusal names, e.g. `Tags`.
    pub member: &'static str,
    /// Why that member is not acceptable.
    pub message: String,
}

impl TagRejection {
    fn new(member: &'static str, message: String) -> Self {
        Self { member, message }
    }

    /// The modelled `ValidationException` this rejection reports. Every caller lifts the
    /// same exception into its own operation error enum, so the refusal reads the same
    /// whichever operation the tags arrived on.
    pub fn to_exception(&self) -> ValidationException {
        ValidationException {
            message: self.message.clone(),
            reason: Some("FieldValidationFailed".to_string()),
            fields: Some(vec![ValidationExceptionField {
                name: self.member.to_string(),
                message: self.message.clone(),
            }]),
        }
    }
}

/// Why the `Tags` a request asks to store were refused.
#[derive(Debug, Clone)]
pub enum TagProblem {
    /// A tag breaks a rule every tag obeys; the caller answers [`TagRejection::to_exception`].
    Invalid(TagRejection),
    /// The request carries more distinct keys than [`MAX_TAGS_PER_RESOURCE`] (this many),
    /// so the caller answers its modelled `ServiceQuotaExceededException`.
    TooMany(usize),
}

/// The checks every operation that STORES a `Tags` member runs before storing anything:
/// the per-tag rules of [`validate_tags`], then the per-resource tag quota.
///
/// Shared by the creates that carry tags, so one rule change reaches all of them; the
/// caller only lifts the problem into its own error enum.
pub fn check_requested_tags(tags: Option<&Vec<Tag>>) -> Result<(), TagProblem> {
    let tags = tags.map_or(&[][..], Vec::as_slice);
    validate_tags(tags).map_err(TagProblem::Invalid)?;
    let count = tag_count_after_merge(&[], tags);
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(TagProblem::TooMany(count));
    }
    Ok(())
}

/// The modelled `ValidationException` (HTTP 400) a [`TagProblem`] becomes on an operation
/// that models NO `ServiceQuotaExceededException`.
///
/// The attachment creates are such operations (`smithy_query query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment"` models AccessDenied, Conflict,
/// InternalServer, ResourceNotFound, Throttling and Validation only), so a request past
/// the per-resource tag quota is refused through the one error class they do model,
/// carrying [`VALIDATION_REASON_OTHER`] — the `Tags` member satisfies every modelled
/// constraint, it is the resulting tag COUNT that the request cannot have.
pub fn tag_problem_rejection(problem: TagProblem) -> ValidationException {
    match problem {
        TagProblem::Invalid(rejection) => rejection.to_exception(),
        TagProblem::TooMany(resulting) => field_rejection(
            FIELD_TAGS,
            VALIDATION_REASON_OTHER,
            too_many_tags_message(resulting),
        ),
    }
}

/// The message a refusal past the per-resource tag quota carries, shared by every
/// operation that stores tags: `resulting` is how many tags the resource would carry if
/// the request were accepted.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags, but the request would leave {resulting}"
    )
}

/// The modelled `ValidationException` (HTTP 400) a create answers when the core network it
/// must hang off configures no edge in the Region the resource it names lives in.
///
/// A peering and an attachment both sit at a core network EDGE, and a core network's edges
/// are the ones its policy document configures
/// ([`PolicyTopology::has_edge_location`]), so a request naming a Region that document
/// does not configure is refused rather than attached to an edge that does not exist. The
/// `Reason` is [`VALIDATION_REASON_OTHER`]: the member satisfies every modelled
/// constraint, it is the stored policy that configures no such edge.
pub fn no_such_edge(
    member: &str,
    core_network_id: &str,
    edge_location: &str,
) -> ValidationException {
    field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!("core network {core_network_id} configures no edge location in {edge_location}"),
    )
}

/// The modelled `ValidationException` (HTTP 400) a request answers when the core network it
/// addresses declares no segment of the name it carries.
///
/// The sibling of [`no_such_edge`] for the other half of a core network's topology: a
/// segment of a core network is one its policy document declares
/// ([`PolicyTopology::has_segment`]), so a request naming any other segment addresses
/// nothing. The `Reason` is [`VALIDATION_REASON_OTHER`] for the same reason: the member
/// satisfies every modelled constraint, it is the stored policy that declares no such
/// segment.
pub fn no_such_segment(
    member: &str,
    core_network_id: &str,
    segment_name: &str,
) -> ValidationException {
    field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!("core network {core_network_id} declares no segment {segment_name}"),
    )
}

/// The modelled `ValidationException` (HTTP 400) a request answers when the core network it
/// addresses declares no network function group of the name it carries.
///
/// The sibling of [`no_such_segment`] for the other kind of group a policy document declares
/// ([`PolicyTopology::has_network_function_group`]): a network function group of a core
/// network is one its `network-function-groups` declares, so a request naming any other
/// group addresses nothing. The `Reason` is [`VALIDATION_REASON_OTHER`] for the same reason:
/// the member satisfies every modelled constraint, it is the stored policy that declares no
/// such group.
pub fn no_such_network_function_group(
    member: &str,
    core_network_id: &str,
    group_name: &str,
) -> ValidationException {
    field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!("core network {core_network_id} declares no network function group {group_name}"),
    )
}

/// Check a tag set a request asks to APPLY (`TagResource`, and the `Tags` of a create).
///
/// Refuses an empty or over-long key, an over-long value, and any key carrying the
/// AWS-reserved prefix. A key repeated inside one request is NOT an error: tag keys are
/// unique per resource, so the last value for a key wins — through [`merge_tags`] on the
/// `TagResource` path and through [`tags_to_store`] for the `Tags` of a create. Every
/// entry is checked, including one a repeat later replaces, so a request carrying a tag
/// that breaks a rule is refused whether or not that entry would survive the merge.
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagRejection> {
    const MEMBER: &str = FIELD_TAGS;
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        check_tag_key(MEMBER, key)?;
        let value = tag.value.as_deref().unwrap_or_default();
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            return Err(TagRejection::new(
                MEMBER,
                format!(
                    "the value of tag key {key:?} must be at most {MAX_TAG_VALUE_CHARS} characters, but is {length}"
                ),
            ));
        }
    }
    Ok(())
}

/// Check a key list a request asks to REMOVE (`UntagResource`). A key the resource does
/// not carry is not an error — removal is idempotent — but a key AWS reserves cannot be
/// removed any more than it can be set.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), TagRejection> {
    const MEMBER: &str = "TagKeys";
    for key in keys {
        check_tag_key(MEMBER, key)?;
    }
    Ok(())
}

/// The rules every tag key obeys, whichever member carries it.
fn check_tag_key(member: &'static str, key: &str) -> Result<(), TagRejection> {
    if key.is_empty() {
        return Err(TagRejection::new(
            member,
            "a tag key must not be empty".to_string(),
        ));
    }
    let length = key.chars().count();
    if length > MAX_TAG_KEY_CHARS {
        return Err(TagRejection::new(
            member,
            format!("a tag key must be at most {MAX_TAG_KEY_CHARS} characters, but {length} were given"),
        ));
    }
    // The recorded refusal is fixed text naming the reserved prefix and nothing else: the
    // offending key is not echoed back.
    if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Err(TagRejection::new(
            member,
            format!("Tag cannot start with {RESERVED_TAG_PREFIX}"),
        ));
    }
    Ok(())
}

/// How many tags the resource would carry once `requested` is merged into `existing`,
/// for the [`MAX_TAGS_PER_RESOURCE`] quota: a key already held is replaced, not added.
pub fn tag_count_after_merge(existing: &[Tag], requested: &[Tag]) -> usize {
    let mut keys: BTreeSet<&str> = existing.iter().filter_map(|tag| tag.key.as_deref()).collect();
    keys.extend(requested.iter().filter_map(|tag| tag.key.as_deref()));
    keys.len()
}

/// Merge `requested` into the resource's stored tags.
///
/// Tag keys are unique per resource, so a key already held keeps its position and takes
/// the requested value and a new key is appended — which is what makes a repeated
/// `TagResource` overwrite rather than duplicate.
pub fn merge_tags(existing: &mut Vec<Tag>, requested: Vec<Tag>) {
    for tag in requested {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => existing.push(tag),
        }
    }
}

/// The tag list a resource is STORED with for the `Tags` a create carried.
///
/// A resource's tags are a key-to-value map, so a key one request names more than once is
/// stored ONCE: the first occurrence keeps its position and the last value given for that
/// key wins — the very merge [`merge_tags`] performs for a `TagResource`, and the one
/// [`tag_count_after_merge`] already grades the create's quota by. Without it a create
/// could store two entries for one key, which no later write could repair.
pub fn tags_to_store(requested: Option<Vec<Tag>>) -> Vec<Tag> {
    let mut stored = Vec::new();
    merge_tags(&mut stored, requested.unwrap_or_default());
    stored
}

/// Remove every tag whose key `keys` names. A key the resource does not carry is ignored,
/// so removing twice answers the same as removing once.
pub fn remove_tags(existing: &mut Vec<Tag>, keys: &[String]) {
    existing.retain(|held| {
        held.key
            .as_deref()
            .is_none_or(|key| !keys.iter().any(|wanted| wanted == key))
    });
}

/// The keys of a core network policy document this service reads.
///
/// A core network policy is a JSON document whose schema AWS publishes (Cloud WAN "Core
/// network policy" documentation): a `version`, a `core-network-configuration` carrying
/// `asn-ranges` and `edge-locations`, a `segments` list and a `network-function-groups`
/// list. The model types the document as an opaque string (`CoreNetworkPolicyDocument`,
/// len 0..10000000, `smithy_query shape=CoreNetworkPolicyDocument`), so these names come
/// from that published schema rather than from the model.
const POLICY_VERSION: &str = "version";
/// See [`POLICY_VERSION`].
const POLICY_CONFIGURATION: &str = "core-network-configuration";
/// See [`POLICY_VERSION`].
const POLICY_ASN_RANGES: &str = "asn-ranges";
/// See [`POLICY_VERSION`].
const POLICY_EDGE_LOCATIONS: &str = "edge-locations";
/// See [`POLICY_VERSION`].
const POLICY_LOCATION: &str = "location";
/// See [`POLICY_VERSION`].
const POLICY_ASN: &str = "asn";
/// See [`POLICY_VERSION`].
const POLICY_INSIDE_CIDR_BLOCKS: &str = "inside-cidr-blocks";
/// See [`POLICY_VERSION`].
const POLICY_SEGMENTS: &str = "segments";
/// See [`POLICY_VERSION`].
const POLICY_NETWORK_FUNCTION_GROUPS: &str = "network-function-groups";
/// See [`POLICY_VERSION`].
const POLICY_NAME: &str = "name";
/// See [`POLICY_VERSION`]: the keys of an `attachment-policies[]` rule, the part of the
/// document that decides what happens to an attachment created on the core network. A rule
/// carries a `rule-number`, the `conditions` an attachment is matched against (combined by
/// its `condition-logic`) and the `action` taken on the attachment it claims.
const POLICY_ATTACHMENT_POLICIES: &str = "attachment-policies";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_RULE_NUMBER: &str = "rule-number";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_CONDITION_LOGIC: &str = "condition-logic";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_CONDITIONS: &str = "conditions";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_ACTION: &str = "action";
/// See [`POLICY_ATTACHMENT_POLICIES`]: the keys of one `conditions[]` entry.
const POLICY_TYPE: &str = "type";
/// See [`POLICY_TYPE`].
const POLICY_OPERATOR: &str = "operator";
/// See [`POLICY_TYPE`].
const POLICY_KEY: &str = "key";
/// See [`POLICY_TYPE`].
const POLICY_VALUE: &str = "value";
/// See [`POLICY_ATTACHMENT_POLICIES`]: the keys of a rule's `action`.
const POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE: &str = "require-attachment-acceptance";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_ASSOCIATION_METHOD: &str = "association-method";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_SEGMENT: &str = "segment";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_TAG_VALUE_OF_KEY: &str = "tag-value-of-key";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_ADD_TO_NETWORK_FUNCTION_GROUP: &str = "add-to-network-function-group";

/// See [`POLICY_VERSION`]: the keys of a `segment-actions[]` entry, the part of the document
/// that says what one segment's routes reach. Every entry names the `action` it takes and
/// the `segment` it acts on; a `share` action lists the segments that segment's routes are
/// shared with in `share-with`, and a service-insertion action names the network function
/// groups the segment's traffic goes through under `via.network-function-groups`.
const POLICY_SEGMENT_ACTIONS: &str = "segment-actions";
/// See [`POLICY_SEGMENT_ACTIONS`].
const POLICY_SHARE_WITH: &str = "share-with";
/// See [`POLICY_SEGMENT_ACTIONS`]: the key of the object form of `share-with`, which shares
/// with every other segment but the ones it lists.
const POLICY_EXCEPT: &str = "except";
/// See [`POLICY_SEGMENT_ACTIONS`].
const POLICY_VIA: &str = "via";
/// See [`POLICY_SEGMENT_ACTIONS`]: the keys of a `create-route` entry, which declares the
/// STATIC routes of the segment it acts on — the prefixes those routes carry, and the
/// attachments of the segment they are reached through (or the string
/// [`POLICY_DESTINATION_BLACKHOLE`], which reaches nothing).
const POLICY_DESTINATION_CIDR_BLOCKS: &str = "destination-cidr-blocks";
/// See [`POLICY_DESTINATION_CIDR_BLOCKS`].
const POLICY_DESTINATIONS: &str = "destinations";
/// See [`POLICY_DESTINATION_CIDR_BLOCKS`]: the `destinations` entry that names no
/// attachment, so the route it declares is reached through nothing.
pub const POLICY_DESTINATION_BLACKHOLE: &str = "blackhole";

/// The `segment-actions[].action` values this slice derives the reported topology from:
/// `share` decides which segments a segment's routes reach, and the two service-insertion
/// actions — the values the model enumerates as `SegmentActionServiceInsertion`, mirrored
/// in `ops::types::SEGMENT_ACTION_SERVICE_INSERTION_VALUES` — decide which segments a
/// network function group serves. Every other action of the published schema says nothing
/// about either and is passed over.
const POLICY_ACTION_SHARE: &str = "share";
/// See [`POLICY_ACTION_SHARE`].
const POLICY_ACTION_SEND_VIA: &str = "send-via";
/// See [`POLICY_ACTION_SHARE`].
const POLICY_ACTION_SEND_TO: &str = "send-to";
/// See [`POLICY_ACTION_SHARE`]: `create-route` declares the static routes a segment holds,
/// which is what the routing information of that segment reports beside what its BGP
/// peerings carry ([`PolicyTopology::static_routes_of`]).
const POLICY_ACTION_CREATE_ROUTE: &str = "create-route";

/// The `share-with` spelling that means every other segment of the core network.
const POLICY_SHARE_WITH_ALL: &str = "*";

/// The `condition-logic` value that makes a rule claim an attachment ANY one of its
/// conditions holds for; the other published value, `and`, is the default and is what every
/// other string (and an absent member) means here.
const POLICY_LOGIC_OR: &str = "or";

/// The `association-method` values a rule's action may carry: `constant` names the segment
/// in `segment`, `tag` takes the segment name from the value of the attachment's
/// `tag-value-of-key` tag (Cloud WAN "Core network policy" documentation).
const POLICY_ASSOCIATION_CONSTANT: &str = "constant";
/// See [`POLICY_ASSOCIATION_CONSTANT`].
const POLICY_ASSOCIATION_TAG: &str = "tag";

/// The `conditions[].type` values an attachment-policy rule matches an attachment against,
/// as the published policy schema lists them: `any` claims every attachment, and the rest
/// each compare one property of the attachment with the condition's `value` under its
/// `operator` — the account that owns it, the Region of its edge location, its
/// `AttachmentType`, the ARN of the resource it attaches, or one of its tags.
const CONDITION_ANY: &str = "any";
/// See [`CONDITION_ANY`].
const CONDITION_ACCOUNT_ID: &str = "account-id";
/// See [`CONDITION_ANY`].
const CONDITION_REGION: &str = "region";
/// See [`CONDITION_ANY`].
const CONDITION_ATTACHMENT_TYPE: &str = "attachment-type";
/// See [`CONDITION_ANY`].
const CONDITION_RESOURCE_ID: &str = "resource-id";
/// See [`CONDITION_ANY`].
const CONDITION_TAG_EXISTS: &str = "tag-exists";
/// See [`CONDITION_ANY`].
const CONDITION_TAG_VALUE: &str = "tag-value";

/// The `conditions[].operator` values the schema lists, each comparing the attachment's
/// property with the condition's `value`.
const OPERATOR_EQUALS: &str = "equals";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_NOT_EQUALS: &str = "not-equals";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_CONTAINS: &str = "contains";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_BEGINS_WITH: &str = "begins-with";

/// A stored record that remembers the idempotency token the request which produced it
/// carried, which is what lets [`replay_or_conflict`] recognise a retry of that request.
pub trait HasClientToken {
    /// The normalised token of the request that produced this record, or `None` where it
    /// carried none.
    fn client_token(&self) -> Option<&str>;
}

/// Every record of this service whose producing operation models an idempotency token:
/// each keeps it in a `client_token` field, so one implementation serves them all.
macro_rules! client_token_records {
    ($($record:ty),+ $(,)?) => {
        $(
            impl HasClientToken for $record {
                fn client_token(&self) -> Option<&str> {
                    self.client_token.as_deref()
                }
            }
        )+
    };
}

client_token_records!(
    AttachmentRecord,
    ConnectPeerRecord,
    CoreNetworkPolicyRecord,
    CoreNetworkRecord,
    PeeringRecord,
    PrefixListAssociationRecord,
);

/// The normalisation every idempotency token of this service goes through: an EMPTY token
/// is no token at all — the modelled member starts at length zero (`smithy_query
/// shape=ClientToken`) — so a request carrying one is idempotent against nothing and the
/// record it produces is stored under no token.
pub fn normalized_client_token<Token: AsRef<str>>(token: Option<Token>) -> Option<Token> {
    token.filter(|token| !token.as_ref().is_empty())
}

/// What the refusal of a reused idempotency token says about the record the token's first
/// request already produced: how that request is named, and the resource the refusal points
/// at.
#[derive(Debug)]
pub struct ClientTokenConflict {
    /// What the first request under the token DID, as the refusal names it ("created
    /// attachment attachment-0123…"): the only part of the message that varies by kind.
    pub effect: String,
    /// The id of the resource that request produced, for `ConflictException$ResourceId`.
    pub resource_id: String,
    /// That resource's type, for `ConflictException$ResourceType`.
    pub resource_type: String,
}

/// Resolve the idempotency token of a tokened write against the records a store already
/// holds, where each record remembers that token in the field [`HasClientToken`] names.
///
/// `Ok(Some(record))` is a retry of the request that produced `record`, which the caller
/// answers from that record instead of producing a second one; `Ok(None)` is a request that
/// nothing has been stored under yet, which the caller goes on to produce; `Err` is the same
/// token asking for something else, which is refused and stores nothing. `same_request` is
/// the per-kind comparison of a stored record against the request, and `conflict` names what
/// the stored record's own creation was.
pub fn replay_or_conflict<'store, Record>(
    token: Option<&str>,
    stored: impl IntoIterator<Item = &'store Record>,
    same_request: impl Fn(&Record) -> bool,
    conflict: impl FnOnce(&Record) -> ClientTokenConflict,
) -> Result<Option<&'store Record>, ConflictException>
where
    Record: HasClientToken + 'store,
{
    replay_or_conflict_by(
        token,
        stored,
        HasClientToken::client_token,
        same_request,
        conflict,
    )
}

/// [`replay_or_conflict`] where the token a stored record was produced under is reached by
/// `token_of` rather than by [`HasClientToken`].
///
/// The walk is the same one, and it is the only one: a record can remember the token of
/// MORE than one tokened operation — an attachment remembers the token of the create that
/// minted it and the token of the call that applied the label it currently carries — and
/// each of those tokens is resolved against the records holding it rather than against the
/// one field a record's [`HasClientToken`] names.
pub fn replay_or_conflict_by<'store, Record>(
    token: Option<&str>,
    stored: impl IntoIterator<Item = &'store Record>,
    token_of: impl Fn(&Record) -> Option<&str>,
    same_request: impl Fn(&Record) -> bool,
    conflict: impl FnOnce(&Record) -> ClientTokenConflict,
) -> Result<Option<&'store Record>, ConflictException> {
    let Some(token) = normalized_client_token(token) else {
        return Ok(None);
    };
    let Some(existing) = stored
        .into_iter()
        .find(|record| token_of(record) == Some(token))
    else {
        return Ok(None);
    };
    if same_request(existing) {
        return Ok(Some(existing));
    }
    let ClientTokenConflict {
        effect,
        resource_id,
        resource_type,
    } = conflict(existing);
    Err(ConflictException {
        message: format!("client token {token:?} already {effect}, with a different request"),
        resource_id,
        resource_type,
    })
}

/// Whether two tag sets hold the same keys with the same values, whatever order they
/// arrived in. Used by the idempotency check of a create: a retry that carries the same
/// `ClientToken` must carry the same tags to be the same request. `Tag` is a generated
/// shape with no `PartialEq`, so the comparison goes through the key/value pairs.
pub fn same_tags(left: &[Tag], right: &[Tag]) -> bool {
    let pairs = |tags: &[Tag]| -> BTreeMap<String, String> {
        tags.iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect()
    };
    pairs(left) == pairs(right)
}

/// Whether two `ConnectAttachmentOptions` ask for the same thing, for the idempotency
/// comparison a repeated `ClientToken` needs: the shape is generated and carries no
/// `PartialEq`, so the comparison goes through its members (`Protocol` is its only one:
/// `smithy_query shape=ConnectAttachmentOptions`).
pub fn same_connect_options(
    left: &ConnectAttachmentOptions,
    right: &ConnectAttachmentOptions,
) -> bool {
    left.protocol == right.protocol
}

/// Whether two `VpcOptions` ask for the same thing. See [`same_connect_options`]; an
/// absent `Options` asks for the modelled defaults, which is what an absent one on the
/// other side asks for too, so two absent options are the same request.
///
/// EVERY member of `VpcOptions` carries a modelled `@default` of `false` (`smithy_query
/// shape=VpcOptions`), so a set that leaves a member out asks for that member off, and a
/// set that is absent altogether asks for all four off. Each side is therefore read as the
/// four flags it asks for — the defaults where it says nothing — rather than as "present"
/// against "absent": an absent options set and one spelling the defaults out, the empty
/// object included, are the same request. Unlike [`same_location`], where every member is
/// optional with no default and an absent structure names no location at all, no spelling
/// of `VpcOptions` can leave a flag unasked-for.
pub fn same_vpc_options(left: Option<&VpcOptions>, right: Option<&VpcOptions>) -> bool {
    let members = |options: Option<&VpcOptions>| {
        options
            .map(|options| {
                (
                    options.ipv6_support,
                    options.appliance_mode_support,
                    options.dns_support,
                    options.security_group_referencing_support,
                )
            })
            .unwrap_or_default()
    };
    members(left) == members(right)
}

/// Whether two `Bandwidth` structures carry the same speeds. See [`same_connect_options`]:
/// the shape is generated and carries no `PartialEq`, so the comparison goes through its
/// members (`UploadSpeed` and `DownloadSpeed`: `smithy_query shape=Bandwidth`).
pub fn same_bandwidth(left: &Bandwidth, right: &Bandwidth) -> bool {
    (left.upload_speed, left.download_speed) == (right.upload_speed, right.download_speed)
}

/// Whether two `Location` structures carry the same members. See [`same_connect_options`];
/// a structure that is absent altogether differs from one carrying no member, because the
/// first names no location at all while the second names an empty one.
pub fn same_location(left: Option<&Location>, right: Option<&Location>) -> bool {
    let members = |location: Option<&Location>| {
        location.map(|location| {
            (
                location.address.clone(),
                location.latitude.clone(),
                location.longitude.clone(),
            )
        })
    };
    members(left) == members(right)
}

/// Whether two `AWSLocation` structures carry the same members. See [`same_location`].
pub fn same_aws_location(left: Option<&AWSLocation>, right: Option<&AWSLocation>) -> bool {
    let members = |location: Option<&AWSLocation>| {
        location.map(|location| (location.zone.clone(), location.subnet_arn.clone()))
    };
    members(left) == members(right)
}

/// Write the value an update SUPPLIED for one member over the value a record holds,
/// answering whether that CHANGED the record.
///
/// An absent member (`None`) is one the request left out, so nothing is written for it;
/// a supplied one replaces the stored value — the empty string included, which is how
/// this service's updates document removing the information a member held — and counts as
/// a change only where the stored value differs. Answering whether anything changed is
/// what lets every updater of this service apply one rule, the one
/// [`AttachmentRecord::set_edge_locations`] applies to an attachment: a request that asks
/// for the state the resource is already in leaves the service no change to apply, so it
/// writes nothing and owes no transition.
pub fn write_supplied<T: PartialEq>(held: &mut T, supplied: Option<T>) -> bool {
    write_supplied_by(held, supplied, |supplied, held| supplied == held)
}

/// [`write_supplied`] for a member whose type is a generated shape and therefore carries
/// no `PartialEq` (see [`same_connect_options`]): `same` is what compares the supplied
/// value against the stored one.
pub fn write_supplied_by<T>(
    held: &mut T,
    supplied: Option<T>,
    same: impl FnOnce(&T, &T) -> bool,
) -> bool {
    match supplied {
        Some(value) if !same(&value, held) => {
            *held = value;
            true
        }
        _ => false,
    }
}

/// The Region of an EC2 resource ARN that must name a resource of type `resource_type`
/// (`arn:<partition>:ec2:<region>:<account>:<resource_type>/<id>`), or `None` when the
/// string is not such an ARN or names no Region.
///
/// [`arn_region`] is enough where the MODEL constrains the ARN's form, but
/// `TransitGatewayArn` and `TransitGatewayRouteTableArn` carry only a length and the
/// permissive `^[\s\S]*$` (`smithy_query shape=TransitGatewayArn`), so for those the
/// handler is what has to establish the resource an ARN names before anything is stored
/// against it.
pub fn ec2_resource_region<'a>(arn: &'a str, resource_type: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != EC2_NAMESPACE {
        return None;
    }
    let region = fields.next()?;
    // The account segment is not judged: a peered transit gateway may belong to another
    // account of the organization.
    let _account = fields.next()?;
    let (kind, id) = fields.next()?.split_once('/')?;
    if kind != resource_type || id.is_empty() {
        return None;
    }
    (!region.is_empty()).then_some(region)
}

/// The modelled `ValidationException` (HTTP 400) an operation answers when the string in its
/// member `member` is not a transit gateway ARN at all (see [`ec2_resource_region`]).
///
/// Like [`malformed_customer_gateway_arn`]: a string that names no transit gateway — the
/// Region it is in included, which such an ARN has to carry — is a FIELD failure on the
/// member that carried it, a different answer from a well-formed ARN that names no
/// registration or peering this service holds. One statement of the wording the register,
/// the deregister, the peering create and the registration listing all answer, so one
/// malformed ARN reads the same whichever of them it arrived on.
pub fn malformed_transit_gateway_arn(member: &str) -> ValidationException {
    field_rejection(
        member,
        REASON_FIELD_VALIDATION_FAILED,
        "must name a transit gateway and the Region it is in".to_string(),
    )
}

/// [`malformed_transit_gateway_arn`] for the Connect peer of a transit gateway: the answer
/// the associate, the disassociate and the association listing give for a string that is no
/// such ARN at all.
pub fn malformed_transit_gateway_connect_peer_arn(member: &str) -> ValidationException {
    field_rejection(
        member,
        REASON_FIELD_VALIDATION_FAILED,
        "must name a transit gateway Connect peer and the Region it is in".to_string(),
    )
}

/// `SystemTime` as a Smithy timestamp (epoch seconds, sub-second part kept).
pub fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// The modelled error list of a stored resource whose every request this service took
/// succeeded: present and EMPTY.
///
/// Every shape of this service that reports the errors of a request taken on a resource
/// does it the same way — an optional list, unbounded or bounded 0..20, "describes the
/// error associated with the … request" (`smithy_query query="shape"
/// shape="Attachment,ConnectPeer,Peering,CoreNetworkPolicy"`: `LastModificationErrors` on
/// the first three, `PolicyErrors` on the last). A resource this service accepted and
/// stored has a known and complete record of those requests, and none of them failed, so
/// the list is reported EMPTY rather than absent: absent would leave a client unable to
/// tell "nothing failed" from "nothing was reported", and would make it tell one of these
/// resources apart from the others for no reason the model gives. One function so no
/// rendering of any of them can drift from the rest.
fn no_reported_errors<T>() -> Option<Vec<T>> {
    Some(Vec::new())
}

/// One page of a paginated listing.
pub struct Page<T> {
    /// The items on this page, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` when this is the last page.
    pub next_token: Option<String>,
}

/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";

/// What separates the listing a token belongs to from the key offset it names inside that
/// listing. A listing is named by its operation name, which carries no `:`, so the FIRST
/// `:` after the listing name ends it and everything beyond it is the key — however many
/// of these the key itself holds.
const PAGE_TOKEN_LISTING_SEPARATOR: char = ':';

/// Encode the continuation token that resumes `listing` AFTER `last_key`.
///
/// The token is opaque to the caller: hex of a prefixed key offset into the relevant
/// `BTreeMap`, which is why the listing map is ordered. Hex, so the token survives the
/// query string a `NextToken` travels back in (`@httpQuery("nextToken")`) unescaped,
/// whatever the key holds.
///
/// `listing` is the listing the token names a position IN — each paginated operation passes
/// its own operation name — because a key offset only means anything in the key space that
/// produced it: an attachment id names no position among site ids, and a zero-padded
/// version number names none among either. Carrying the listing in the token is what lets
/// [`decode_page_token`] tell a token of THIS listing from one of another.
fn encode_page_token(listing: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{PAGE_TOKEN_PREFIX}{listing}{PAGE_TOKEN_LISTING_SEPARATOR}{last_key}"
    ))
}

/// Decode a continuation token minted by [`encode_page_token`] FOR `listing`, or `None`
/// when the token is not one this service produced for that listing — the caller answers
/// that with a `ValidationException`.
///
/// Only the ONE spelling [`encode_page_token`] writes for a position names that position: a
/// token is accepted only when it is byte-identical to the token this listing would mint
/// for the key it decodes to. `hex::decode` is laxer than `hex::encode` — it reads
/// `A-F` as well as the `a-f` the minter emits — so without that check a re-cased copy of a
/// token, a string no response of this service ever carried, would resume a listing.
///
/// A token minted by a DIFFERENT listing is rejected here too, rather than resolved against
/// a key space it names no position in: the keys of one listing sort arbitrarily against
/// another's, so such a token would otherwise be answered with an empty page (when no key of
/// this listing sorts after it) or with this listing's first page again (when every key
/// does), both of them silent.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let last_key = text
        .strip_prefix(PAGE_TOKEN_PREFIX)?
        .strip_prefix(listing)?
        .strip_prefix(PAGE_TOKEN_LISTING_SEPARATOR)?;
    (encode_page_token(listing, last_key) == token).then(|| last_key.to_string())
}

/// Where the `next_token` a request carries makes a paginated listing START: the key offset
/// the token names, or `None` for the first page. Every paginated listing of this service
/// reads its `NextToken` through here, so one value has one reading whatever is listed.
///
/// An EMPTY token is NO token. The member is modelled `length 0..2048` (`smithy_query
/// query="shape" shape="NextToken"`) and travels in the query string, so `?nextToken=`
/// satisfies the schema and reaches the handler as the empty string — a value that names no
/// position, and whose only page can be the first one. That is the reading this service
/// gives every optional string member whose modelled length admits the empty string
/// ([`optional_reference`], [`normalized_client_token`]): empty is the ABSENCE of the value,
/// not a value nothing can resolve. The first page is also what a listing answers for the
/// omitted member, so the two spellings of "no token" agree.
///
/// Any other token [`decode_page_token`] cannot decode for `listing` gets the ONE refusal
/// every paginated listing of this service gives for a token it did not mint — whether no
/// listing of this service minted it or another one did.
///
/// A request carrying such a token was itself read perfectly well — it is the value of the
/// single input member `NextToken` that names no position in the listing, which is what the
/// refusal says, whether the token came from another listing of this service or from
/// nowhere this service minted — so the refusal
/// is the member failure [`REASON_FIELD_VALIDATION_FAILED`], naming [`FIELD_NEXT_TOKEN`]
/// in `Fields`, and not [`REASON_CANNOT_PARSE`], which this service answers only where the
/// TEXT it was handed could not be read at all (a resource policy document that is no
/// JSON: [`validate_resource_policy_document`]). Stated once here so no listing of this
/// service can answer another `Reason`, or another `Fields` entry, for the same input.
pub fn page_start_or_reject(
    listing: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, ValidationException> {
    let Some(token) = next_token.filter(|token| !token.is_empty()) else {
        return Ok(None);
    };
    decode_page_token(listing, token)
        .map(Some)
        .ok_or_else(|| ValidationException {
            message: "NextToken does not name a position in this listing".to_string(),
            reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
            fields: Some(vec![ValidationExceptionField {
                name: FIELD_NEXT_TOKEN.to_string(),
                message: "the token names no position in this listing".to_string(),
            }]),
        })
}

/// Take one page of `listing`'s `entries` (which must be sorted by key) starting after
/// `after`.
///
/// `max_results` is the caller's page size; absent means "every remaining item". A
/// `next_token` is emitted only when items remain after this page, so a client that
/// follows tokens sees every item exactly once.
///
/// `listing` is the same listing name the `after` key was decoded with
/// ([`page_start_or_reject`]): the token minted here names a position in THIS
/// listing's key space, so only this listing accepts it back.
pub fn paginate<T>(
    listing: &str,
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let start = match after {
        Some(key) => entries
            .iter()
            .position(|(candidate, _)| candidate.as_str() > key)
            .unwrap_or(entries.len()),
        None => 0,
    };
    let limit = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(usize::MAX)
        // A page is never empty while items remain: `MaxResults` is modelled `1..500`, so
        // 0 cannot arrive, and clamping keeps the token below from skipping an item.
        .max(1);
    let end = start.saturating_add(limit).min(entries.len());
    let next_token = if end < entries.len() {
        entries
            .get(end.saturating_sub(1))
            .map(|(key, _)| encode_page_token(listing, key))
    } else {
        None
    };
    let items = entries
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, item)| item)
        .collect();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmanagerNetworkmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmanagerNetworkmanagerState>)
}
