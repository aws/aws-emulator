// SPDX-License-Identifier: CC0-1.0

//! Client VPN endpoints, the target networks associated with them, and the routes of their
//! route tables.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`: the stores, their
//! id forms, their refusals and the clock-owed `associating` -> `associated` and
//! `creating` -> `active` moves are one subject, and keeping them in their own file leaves
//! `state.rs` readable — and leaves this fix out of the end of that file, which is where
//! every concurrently landing EC2 fix inserts. Everything here is re-exported by
//! `crate::state`, so a handler reaches it as
//! `crate::state::require_known_client_vpn_endpoint` exactly as it reaches the helpers that
//! live in the parent module.
//!
//! The stores themselves are fields of [`ComAmazonawsEc2Amazonec2State`]
//! (`client_vpn_endpoints`, `client_vpn_target_networks`, `client_vpn_routes`), because the
//! state struct is one struct; it is only the code that reads and writes them that lives
//! here.

use super::*;

/// The id prefix EC2 mints a Client VPN endpoint with: `cvpn-endpoint-`, as the AWS Client
/// VPN documentation's own ids read (for example `cvpn-endpoint-0ac3a1abbccba1234`). The
/// model puts no pattern on `ClientVpnEndpointId`
/// (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain `String`), so the
/// prefix is the one thing that says which resource an id names.
///
/// The prefix is what an id of this kind READS as; it is not a constraint this surface
/// enforces, because Client VPN answers one refusal
/// ([`invalid_client_vpn_endpoint_id_not_found`]) for every value it cannot resolve — see
/// [`require_client_vpn_endpoint`]. So nothing here refuses an id for its FORM, and the
/// prefix serves the mint and the id spaces it keeps apart.
pub const CLIENT_VPN_ENDPOINT_ID_PREFIX: &str = "cvpn-endpoint";

/// The id prefix EC2 mints a target network association with: `cvpn-assoc-`, as the AWS
/// documentation's own association ids read (for example `cvpn-assoc-0c9a06dc85b3e9bc1`).
pub const CLIENT_VPN_TARGET_NETWORK_ASSOCIATION_ID_PREFIX: &str = "cvpn-assoc";

/// `InvalidClientVpnEndpointId.NotFound` (HTTP 400): the request named a Client VPN
/// endpoint id this account holds no endpoint for.
pub const INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND: &str = "InvalidClientVpnEndpointId.NotFound";

/// "associating" — the state a target network association starts in
/// (`AssociationStatusCode`), which `AssociateClientVpnTargetNetwork` answers with.
pub const ASSOCIATION_STATUS_ASSOCIATING: &str = "associating";

/// "associated" — the state the clock owes an `associating` target network association
/// ([`TRANSITION_DELAY`] later, through
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const ASSOCIATION_STATUS_ASSOCIATED: &str = "associated";

/// "pending-associate" — "The Client VPN endpoint has been created but no target networks
/// have been associated" (`ClientVpnEndpointStatusCode`), where an endpoint sits until one
/// is.
pub const CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE: &str = "pending-associate";

/// "available" — "The Client VPN endpoint has been created and a target network has been
/// associated" (`ClientVpnEndpointStatusCode`), which an endpoint reaches when a target
/// network IS associated with it
/// ([`ComAmazonawsEc2Amazonec2State::mark_client_vpn_endpoint_associated`]).
pub const CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE: &str = "available";

/// `InvalidClientVpnDuplicateAssociation` (HTTP 400): the target network the request names is
/// already associated with that Client VPN endpoint, so associating it would make a second
/// association of one network.
pub const INVALID_CLIENT_VPN_DUPLICATE_ASSOCIATION: &str = "InvalidClientVpnDuplicateAssociation";

/// `InvalidClientVpnSubnetId.DuplicateAz` (HTTP 400): the endpoint already has a target
/// network in that Availability Zone, and "you can associate only one subnet in each
/// Availability Zone" (`AssociateClientVpnTargetNetwork`).
pub const INVALID_CLIENT_VPN_SUBNET_ID_DUPLICATE_AZ: &str = "InvalidClientVpnSubnetId.DuplicateAz";

/// Refuse a target network an endpoint already has: HTTP 400
/// [`INVALID_CLIENT_VPN_DUPLICATE_ASSOCIATION`], naming the network and the endpoint.
///
/// `target` is the network as the association reports it — the subnet of a VPC-based
/// endpoint, the Availability Zone of a Transit Gateway-based one — so it is a value this
/// store resolved rather than the request's own bytes.
pub fn client_vpn_duplicate_association<E>(target: &str, endpoint_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_DUPLICATE_ASSOCIATION,
        400,
        format!("The target network {target} is already associated with Client VPN endpoint {endpoint_id}"),
    )
}

/// Refuse a subnet in an Availability Zone the endpoint already has a target network in:
/// HTTP 400 [`INVALID_CLIENT_VPN_SUBNET_ID_DUPLICATE_AZ`], naming the zone and the subnet
/// that holds it.
pub fn client_vpn_duplicate_availability_zone<E>(
    subnet_id: &str,
    zone: &str,
    associated_subnet_id: &str,
    endpoint_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_SUBNET_ID_DUPLICATE_AZ,
        400,
        format!(
            "The subnet {subnet_id} is in the Availability Zone {zone}, where the subnet \
             {associated_subnet_id} is already associated with Client VPN endpoint \
             {endpoint_id}"
        ),
    )
}

/// The most characters of a refused `ClientVpnEndpointId` its message repeats back.
///
/// The message names the endpoint the request could not resolve, and that is the request's own
/// bytes — `ClientVpnEndpointId` is a bare string the model puts no `@length` on, and this
/// surface refuses a value of any form by LOOKUP rather than by its form — so the echo is
/// bounded here the way [`MAX_ECHOED_COIP_POOL_ID_CHARS`] bounds a refused pool id's. Far
/// above the longest endpoint id this service mints ([`CLIENT_VPN_ENDPOINT_ID_PREFIX`] plus 17
/// digits), so a real id is always named in full.
pub const MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS: usize = 128;

/// Refuse a request that named a Client VPN endpoint id this account holds no endpoint for:
/// HTTP 400 [`INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND`], worded as the Client VPN surface
/// words it — `Endpoint <id> does not exist`, naming the id the request sent, of which the
/// first [`MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS`] characters are named. That is not
/// the `The <kind> ID '<id>' does not exist` wording [`id_not_found`] writes for the
/// resources whose refusal reads that way, so this one is built here rather than through it.
pub fn invalid_client_vpn_endpoint_id_not_found<E>(id: &str) -> Scaffolded<E> {
    let echoed: String = id
        .chars()
        .take(MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS)
        .collect();
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND,
        400,
        format!("Endpoint {echoed} does not exist"),
    )
}

/// The Client VPN endpoint `id` names, or the one refusal this surface answers for a value it
/// holds no endpoint for: HTTP 400 [`INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND`].
///
/// Resolution by LOOKUP alone, with no id-form arm in front of it: the model puts no pattern
/// on the member (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain
/// `String`), so no id form is a declared constraint of it, and AWS's EC2 error catalogue
/// documents `InvalidClientVpnEndpointId.NotFound` for an endpoint it cannot resolve and no
/// `.Malformed` counterpart to tell "not an id of this kind" from "no such endpoint" with.
/// So a value of any form that this account holds no endpoint for is answered `Endpoint <id>
/// does not exist`.
///
/// That is what EVERY operation taking a `ClientVpnEndpointId` resolves it through, the ones
/// that report what hangs off an endpoint as much as the ones that mutate it:
/// [`require_known_client_vpn_endpoint`] is this lookup for a caller that needs no endpoint
/// back. One id, one refusal, whichever operation was called.
pub fn require_client_vpn_endpoint<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<&'store crate::ops::types::ClientVpnEndpoint, Scaffolded<E>> {
    store
        .client_vpn_endpoints
        .get(id)
        .ok_or_else(|| invalid_client_vpn_endpoint_id_not_found(id))
}

/// Resolve a `ClientVpnEndpointId` for an operation that only needs to know the endpoint
/// EXISTS — the operations that REPORT what hangs off one — by the same lookup
/// [`require_client_vpn_endpoint`] does, so an id the account holds no endpoint for is
/// refused the one way this surface refuses it whatever form the value takes.
///
/// It is the reading half of that function and nothing more: the two are named apart only so
/// a reporting operation does not have to borrow an endpoint it never looks at.
pub fn require_known_client_vpn_endpoint<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_client_vpn_endpoint::<E>(store, id).map(|_| ())
}

// ---------------------------------------------------------------------------
// What CREATING a Client VPN endpoint is held to
// ---------------------------------------------------------------------------

/// The `ResourceType` a Client VPN endpoint is tagged under
/// (`smithy_query(query="shape",shape="ResourceType")` lists `client-vpn-endpoint`), which is
/// the only type a `TagSpecification` of this resource's own create may name.
pub const RESOURCE_TYPE_CLIENT_VPN_ENDPOINT: &str = "client-vpn-endpoint";

/// The prefix lengths a Client VPN endpoint's client CIDR range may carry: "Client CIDR range
/// must have a size of at least /22 and must not be greater than /12"
/// (`CreateClientVpnEndpointRequest$ClientCidrBlock`). A size of "at least /22" is a prefix no
/// LONGER than 22 (the range's end), and "not greater than /12" is a prefix no SHORTER than 12
/// (its start), so the published sentence is this one range.
pub const CLIENT_VPN_CLIENT_CIDR_PREFIX_LENGTHS: ::std::ops::RangeInclusive<u32> = 12..=22;

/// "openvpn" — the one `VpnProtocol` the model declares
/// (`smithy_query(query="shape",shape="VpnProtocol")`), which is what every Client VPN
/// endpoint reports as `ClientVpnEndpoint$VpnProtocol`.
pub const CLIENT_VPN_PROTOCOL_OPENVPN: &str = "openvpn";

/// The transport protocol an endpoint created without one speaks: "Default value: udp"
/// (`CreateClientVpnEndpointRequest$TransportProtocol`).
pub const CLIENT_VPN_DEFAULT_TRANSPORT_PROTOCOL: &str = "udp";

/// The ports a Client VPN endpoint may be reached on: "Valid Values: 443 | 1194"
/// (`CreateClientVpnEndpointRequest$VpnPort`).
pub const CLIENT_VPN_PORTS: [i32; 2] = [443, 1194];

/// The port an endpoint created without one listens on: "Default Value: 443"
/// (`CreateClientVpnEndpointRequest$VpnPort`).
pub const CLIENT_VPN_DEFAULT_PORT: i32 = 443;

/// The session durations a Client VPN endpoint may be given: "Valid values: 8 | 10 | 12 | 24"
/// (`CreateClientVpnEndpointRequest$SessionTimeoutHours`).
pub const CLIENT_VPN_SESSION_TIMEOUT_HOURS: [i32; 4] = [8, 10, 12, 24];

/// The session duration an endpoint created without one runs on: "Default value: 24"
/// (`CreateClientVpnEndpointRequest$SessionTimeoutHours`).
pub const CLIENT_VPN_DEFAULT_SESSION_TIMEOUT_HOURS: i32 = 24;

/// How many DNS servers one endpoint may be given: "A Client VPN endpoint can have up to two
/// DNS servers" (`CreateClientVpnEndpointRequest$DnsServers`).
pub const MAX_CLIENT_VPN_DNS_SERVERS: usize = 2;

/// The IP address type an endpoint created without one carries: "ipv4 (default)"
/// (`CreateClientVpnEndpointRequest$EndpointIpAddressType` and `$TrafficIpAddressType`, which
/// publish the same default).
pub const CLIENT_VPN_DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// The `ClientVpnAuthenticationType` whose option names a client root certificate chain
/// (`smithy_query(query="shape",shape="ClientVpnAuthenticationType")`).
pub const CLIENT_VPN_AUTHENTICATION_CERTIFICATE: &str = "certificate-authentication";

/// The `ClientVpnAuthenticationType` whose option names an Active Directory.
pub const CLIENT_VPN_AUTHENTICATION_DIRECTORY_SERVICE: &str = "directory-service-authentication";

/// The `ClientVpnAuthenticationType` whose option names a SAML identity provider.
pub const CLIENT_VPN_AUTHENTICATION_FEDERATED: &str = "federated-authentication";

/// "applied" — the `ClientVpnEndpointAttributeStatusCode` an attribute this store has already
/// written carries (`smithy_query(query="shape",shape="ClientVpnEndpointAttributeStatusCode")`
/// declares `applying | applied`). The configuration is in force the moment the call that
/// carried it returns: nothing here propagates an attribute to anywhere else, so there is no
/// `applying` phase to report for one.
pub const CLIENT_VPN_ATTRIBUTE_STATUS_APPLIED: &str = "applied";

/// Refuse a client CIDR range of a size Client VPN does not serve: HTTP 400
/// `InvalidParameterValue`, in the sentence `ClientCidrBlock` documents its rule with —
/// "Client CIDR range must have a size of at least /22 and must not be greater than /12" —
/// with the two prefix lengths taken from [`CLIENT_VPN_CLIENT_CIDR_PREFIX_LENGTHS`] rather
/// than written into the sentence twice.
///
/// A bare sentence carrying no part of the request, as the refusals of this surface read
/// ([`client_vpn_certificate_not_found`] is the one a recorded response pins): what was wrong
/// with the value is the SIZE the rule names, which the sentence states.
pub fn client_vpn_client_cidr_size_refusal<E>() -> Scaffolded<E> {
    // The SMALLEST range the rule admits is the one with the LONGEST prefix (the range's end),
    // and the largest is the one with the shortest (its start).
    invalid_parameter_value(format!(
        "Client CIDR range must have a size of at least /{longest_prefix} and must not be \
         greater than /{shortest_prefix}",
        longest_prefix = CLIENT_VPN_CLIENT_CIDR_PREFIX_LENGTHS.end(),
        shortest_prefix = CLIENT_VPN_CLIENT_CIDR_PREFIX_LENGTHS.start(),
    ))
}

/// Refuse a certificate ARN the account holds no ACM certificate for: HTTP 400
/// `InvalidParameterValue`, in the wording this surface answers.
///
/// The message names no part of the request, for the reason
/// [`invalid_certificate_arn_malformed`] gives for its own: what was wrong with the value is
/// that it names no certificate at all, whatever it says.
pub fn client_vpn_certificate_not_found<E>() -> Scaffolded<E> {
    invalid_parameter_value("Certificate not found")
}

/// Refuse a request that carried none of a parameter this surface requires: HTTP 400
/// `MissingParameter`, naming the member in the sentence EC2 answers for one of this
/// operation's `@required` members that a request left out — `The request must include the
/// <Member> parameter. Add the required parameter and retry the request.`
///
/// The member is named as the MODEL spells it, which is how the sentence a recorded refusal of
/// this operation pins names it, and the sentence carries nothing else of the request: what was
/// wrong is that the parameter was not there.
pub fn missing_required_parameter<E>(member: &str) -> Scaffolded<E> {
    missing_parameter(format!(
        "The request must include the {member} parameter. Add the required parameter and retry \
         the request."
    ))
}

/// The DNS name clients reach the Client VPN endpoint `endpoint_id` at, in `region`.
///
/// "The DNS name to be used by clients when establishing their VPN session"
/// (`CreateClientVpnEndpointResult$DnsName`): one name per endpoint, derived from the endpoint's
/// own id and the Region it was created in — the form the AWS Client VPN documentation's own
/// names take (`cvpn-endpoint-0123456789abcdef0.prod.clientvpn.us-east-2.amazonaws.com`) — so
/// the create and every later describe of one endpoint report the same name.
pub fn client_vpn_endpoint_dns_name(endpoint_id: &str, region: &str) -> String {
    format!("{endpoint_id}.prod.clientvpn.{region}.amazonaws.com")
}

/// One target network association: the modelled [`crate::ops::types::TargetNetwork`] every
/// operation that reports one answers with, plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// of this module holds one: a member a writer sets round-trips through
/// `DescribeClientVpnTargetNetworks` without a field-by-field copy that can silently drop
/// one.
#[derive(Debug, Clone)]
pub struct ClientVpnTargetNetworkRecord {
    /// The association as the model shapes it, carrying the status reached so far.
    pub view: crate::ops::types::TargetNetwork,
    /// The `AssociationStatusCode` the clock owes this association while it is still
    /// transient (`associating` -> `associated`); `None` once it has been applied.
    pub pending: Option<PendingStateMove>,
}

/// What one target network association is written with: the endpoint it associates a
/// network with, the network itself — the subnet of a VPC-based endpoint, or the
/// Availability Zone of a Transit Gateway-based one — and the security groups applied to
/// the association.
///
/// Every member the service itself owns — the association id and the status it starts in —
/// is derived by [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_target_network`] rather
/// than taken from a caller, so two writers cannot report one association differently.
#[derive(Debug, Clone, Default)]
pub struct ClientVpnTargetNetworkSpec {
    /// "The ID of the Client VPN endpoint with which the target network is associated"
    /// (`TargetNetwork$ClientVpnEndpointId`).
    pub client_vpn_endpoint_id: String,
    /// "The ID of the subnet specified as the target network"
    /// (`TargetNetwork$TargetNetworkId`); `None` for the zone association of a Transit
    /// Gateway-based endpoint, which names no subnet.
    pub target_network_id: Option<String>,
    /// "The ID of the VPC in which the target network (subnet) is located"
    /// (`TargetNetwork$VpcId`); `None` for that same zone association.
    pub vpc_id: Option<String>,
    /// "The IDs of the security groups applied to the target network association"
    /// (`TargetNetwork$SecurityGroups`).
    pub security_groups: Vec<String>,
    /// "The Availability Zone names for the target network association, if the Client VPN
    /// endpoint uses a Transit Gateway" (`TargetNetwork$AvailabilityZones`) — empty for the
    /// subnet association of a VPC-based endpoint.
    pub availability_zones: Vec<String>,
    /// "The Availability Zone IDs for the target network association, if the Client VPN
    /// endpoint uses a Transit Gateway" (`TargetNetwork$AvailabilityZoneIds`), the same
    /// zones as [`Self::availability_zones`] in their other spelling.
    pub availability_zone_ids: Vec<String>,
    /// The IPv4 range of the VPC the target network is in, which the association adds a
    /// route to the endpoint's route table for ([`CLIENT_VPN_ROUTE_ORIGIN_ASSOCIATE`]);
    /// `None` leaves the route table untouched.
    pub vpc_cidr_block: Option<String>,
}

/// The caller-chosen bytes one stored target network association holds: the ids of the
/// resources it joins and the security groups applied to it, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. Everything else on the
/// record — its own id and its status — is derived here, so it is fixed-size.
///
/// The ONE definition of that charge, the way
/// [`local_gateway_vif_group_association_payload_bytes`] is for its own store: the insert
/// charges exactly it, and a removal path must give back the same number.
pub fn client_vpn_target_network_payload_bytes(view: &crate::ops::types::TargetNetwork) -> usize {
    let security_group_bytes = view
        .security_groups
        .iter()
        .flatten()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    let zone_bytes = view
        .availability_zones
        .iter()
        .flatten()
        .chain(view.availability_zone_ids.iter().flatten())
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.client_vpn_endpoint_id.as_deref().map_or(0, str::len))
        .saturating_add(view.target_network_id.as_deref().map_or(0, str::len))
        .saturating_add(view.vpc_id.as_deref().map_or(0, str::len))
        .saturating_add(security_group_bytes)
        .saturating_add(zone_bytes)
}

/// The caller-chosen bytes one stored Client VPN endpoint holds: its description, the client
/// CIDR block it assigns addresses from, the VPC and security groups it is attached to, the
/// server certificate it presents and its tags, on top of that same fixed overhead.
pub fn client_vpn_endpoint_payload_bytes(view: &crate::ops::types::ClientVpnEndpoint) -> usize {
    let security_group_bytes = view
        .security_group_ids
        .iter()
        .flatten()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(view.client_cidr_block.as_deref().map_or(0, str::len))
        .saturating_add(view.vpc_id.as_deref().map_or(0, str::len))
        .saturating_add(view.server_certificate_arn.as_deref().map_or(0, str::len))
        .saturating_add(security_group_bytes)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Client VPN endpoint fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks this BEFORE inserting, because nothing here reclaims a stored endpoint.
    pub fn has_client_vpn_endpoint_capacity(&self) -> bool {
        self.client_vpn_endpoints.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another target network association fits inside that same ceiling.
    pub fn has_client_vpn_target_network_capacity(&self) -> bool {
        self.client_vpn_target_networks.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a Client VPN endpoint and answer the `ClientVpnEndpointId` it was given — or
    /// `None` when one of the store's bounds would be crossed and NOTHING was stored.
    ///
    /// The seam `CreateClientVpnEndpoint` calls.
    ///
    /// The id is minted here rather than taken from `view`, exactly as
    /// [`Self::insert_ipam_internet_registry_association`] mints an association's, so no
    /// path can store an endpoint under an id this service could not have issued. An
    /// endpoint is stored with no target network associated, which is what
    /// `pending-associate` means ("The Client VPN endpoint has been created but no target
    /// networks have been associated" — `ClientVpnEndpointStatusCode`), so a `view` that
    /// names no status gets that one.
    pub fn insert_client_vpn_endpoint(
        &mut self,
        mut view: crate::ops::types::ClientVpnEndpoint,
    ) -> Option<String> {
        let charge = client_vpn_endpoint_payload_bytes(&view);
        if !self.has_client_vpn_endpoint_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let endpoint_id = self.next_id(CLIENT_VPN_ENDPOINT_ID_PREFIX);
        view.client_vpn_endpoint_id = Some(endpoint_id.clone());
        if view.status.is_none() {
            view.status = Some(crate::ops::types::ClientVpnEndpointStatus {
                code: Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE.to_string()),
                message: None,
            });
        }
        self.add_stored_bytes(charge);
        self.client_vpn_endpoints.insert(endpoint_id.clone(), view);
        Some(endpoint_id)
    }

    /// Associate one target network with a Client VPN endpoint, and answer the association
    /// as its writer reports it — or `None` when one of the store's bounds would be crossed
    /// and NOTHING was stored.
    ///
    /// The seam `AssociateClientVpnTargetNetwork` calls.
    ///
    /// The association starts in `associating`, the state `AssociateClientVpnTargetNetwork`
    /// answers with, and the clock owes it `associated` one [`TRANSITION_DELAY`] later
    /// through [`Self::materialize_due_transitions`]: the span is never zero, so a caller
    /// describing the association straight after the call that made it still sees
    /// `associating`.
    ///
    /// A `spec` naming the VPC's range also adds the route to that range the association
    /// brings with it — "`associate` indicates that the route was automatically added when
    /// the target network was associated with the Client VPN endpoint"
    /// (`ClientVpnRoute$Origin`) — unless the endpoint's route table already holds a route
    /// for that destination through this subnet. Both records clear the store's bounds
    /// TOGETHER, before either is written, so an endpoint is never left with an association
    /// whose route is missing.
    pub fn insert_client_vpn_target_network(
        &mut self,
        spec: ClientVpnTargetNetworkSpec,
    ) -> Option<crate::ops::types::TargetNetwork> {
        // An empty list is no list: a subnet association names no zone and a zone
        // association names no subnet, and neither reports the other's member as an empty
        // set.
        let listed = |values: Vec<String>| (!values.is_empty()).then_some(values);
        let view = crate::ops::types::TargetNetwork {
            association_id: None,
            vpc_id: spec.vpc_id,
            target_network_id: spec.target_network_id.clone(),
            client_vpn_endpoint_id: Some(spec.client_vpn_endpoint_id.clone()),
            status: Some(crate::ops::types::AssociationStatus {
                code: Some(ASSOCIATION_STATUS_ASSOCIATING.to_string()),
                message: None,
            }),
            security_groups: Some(spec.security_groups),
            // "The Availability Zone names for the target network association, if the
            // Client VPN endpoint uses a Transit Gateway"
            // (`TargetNetwork$AvailabilityZones`) — nothing for an endpoint that does not.
            availability_zones: listed(spec.availability_zones),
            availability_zone_ids: listed(spec.availability_zone_ids),
        };
        // The route the association itself adds, and the key it would be held under; `None`
        // when the request's endpoint has no VPC range to route, or when its route table
        // already holds that path.
        let associated_route = spec
            .vpc_cidr_block
            .map(|destination_cidr| {
                client_vpn_route_record(ClientVpnRouteSpec {
                    client_vpn_endpoint_id: spec.client_vpn_endpoint_id,
                    destination_cidr,
                    target_subnet: spec.target_network_id,
                    transit_gateway_attachment_id: None,
                    description: None,
                    origin: CLIENT_VPN_ROUTE_ORIGIN_ASSOCIATE,
                })
            })
            .filter(|(key, _)| !self.client_vpn_routes.contains_key(key));
        // Both records' bounds, asked before the first write: a store at a ceiling stores
        // NEITHER, so no endpoint is left holding half of what one association writes.
        let charge = client_vpn_target_network_payload_bytes(&view).saturating_add(
            associated_route
                .as_ref()
                .map_or(0, |(_, route)| client_vpn_route_payload_bytes(route)),
        );
        if !self.has_client_vpn_target_network_capacity()
            || (associated_route.is_some() && !self.has_client_vpn_route_capacity())
            || self.would_exceed_stored_bytes(charge)
        {
            return None;
        }
        let association_id = self.next_id(CLIENT_VPN_TARGET_NETWORK_ASSOCIATION_ID_PREFIX);
        let view = crate::ops::types::TargetNetwork {
            association_id: Some(association_id.clone()),
            ..view
        };
        self.add_stored_bytes(client_vpn_target_network_payload_bytes(&view));
        let stored = view.clone();
        self.client_vpn_target_networks.insert(
            association_id,
            ClientVpnTargetNetworkRecord {
                view,
                pending: Some(PendingStateMove::starting_now(
                    ASSOCIATION_STATUS_ASSOCIATED,
                )),
            },
        );
        if let Some((key, route)) = associated_route {
            self.store_client_vpn_route(key, route);
        }
        Some(stored)
    }

    /// Move a Client VPN endpoint that was waiting for a target network to `available`.
    ///
    /// "`pending-associate`: The Client VPN endpoint has been created but no target networks
    /// have been associated" and "`available`: The Client VPN endpoint has been created and
    /// a target network has been associated" (`ClientVpnEndpointStatusCode`), so the move
    /// belongs to the operation that associates one — which is why no reporting path
    /// materialises it. Every other status an endpoint can hold is left exactly as it is:
    /// only the endpoint that was WAITING for a target network reaches `available` by
    /// getting one.
    pub fn mark_client_vpn_endpoint_associated(&mut self, endpoint_id: &str) {
        let Some(endpoint) = self.client_vpn_endpoints.get_mut(endpoint_id) else {
            return;
        };
        let waiting = endpoint
            .status
            .as_ref()
            .and_then(|status| status.code.as_deref())
            == Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE);
        if waiting {
            endpoint.status = Some(crate::ops::types::ClientVpnEndpointStatus {
                code: Some(CLIENT_VPN_ENDPOINT_STATUS_AVAILABLE.to_string()),
                message: None,
            });
        }
    }

    /// Record `vpc_id` as the VPC of a Client VPN endpoint that was created without one.
    ///
    /// "If you specified a VPC when you created the Client VPN endpoint or if you have
    /// previous subnet associations, the specified subnet must be in the same VPC"
    /// (`AssociateClientVpnTargetNetwork`): the first subnet associated is what pins an
    /// endpoint created without a `VpcId` to a VPC, so that is where the endpoint's own
    /// `VpcId` comes from. An endpoint that already names one keeps it, so this cannot move
    /// an endpoint from one VPC to another.
    ///
    /// The id is a value this store minted, charged against the byte budget like every other
    /// write onto a stored record, and it is written at most once per endpoint.
    pub fn pin_client_vpn_endpoint_vpc(&mut self, endpoint_id: &str, vpc_id: &str) {
        let Some(endpoint) = self.client_vpn_endpoints.get_mut(endpoint_id) else {
            return;
        };
        if endpoint
            .vpc_id
            .as_deref()
            .is_some_and(|held| !held.is_empty())
        {
            return;
        }
        endpoint.vpc_id = Some(vpc_id.to_string());
        self.add_stored_bytes(vpc_id.len());
    }

    /// The VPC every target network already associated with `endpoint_id` is in, or `None`
    /// when it has no subnet association yet.
    ///
    /// The "previous subnet associations" half of the same sentence
    /// [`Self::pin_client_vpn_endpoint_vpc`] quotes: they are all in one VPC, so the first
    /// one found answers for all of them.
    pub fn client_vpn_endpoint_associated_vpc(&self, endpoint_id: &str) -> Option<String> {
        self.client_vpn_target_networks.values().find_map(|record| {
            let view = &record.view;
            if view.client_vpn_endpoint_id.as_deref() != Some(endpoint_id) {
                return None;
            }
            view.vpc_id.clone()
        })
    }

    /// The subnet already associated with `endpoint_id` that sits in the Availability Zone
    /// `zone_name`, or `None` when none does.
    ///
    /// "You can associate only one subnet in each Availability Zone"
    /// (`AssociateClientVpnTargetNetwork`), asked of the zone each associated subnet is
    /// stored in — so an association whose subnet this store does not hold answers for no
    /// zone rather than for a guessed one.
    pub fn client_vpn_endpoint_subnet_in_zone(
        &self,
        endpoint_id: &str,
        zone_name: &str,
    ) -> Option<String> {
        self.client_vpn_target_networks.values().find_map(|record| {
            let view = &record.view;
            if view.client_vpn_endpoint_id.as_deref() != Some(endpoint_id) {
                return None;
            }
            let subnet_id = view.target_network_id.as_deref()?;
            let subnet = self.subnets.get(subnet_id)?;
            (subnet.availability_zone.as_deref() == Some(zone_name))
                .then(|| subnet_id.to_string())
        })
    }

    /// Is the Availability Zone `zone_name` already a target network of `endpoint_id`?
    ///
    /// The question [`Self::client_vpn_endpoint_has_target_network`] asks of a subnet, for
    /// the zone association of a Transit Gateway-based endpoint — which names a zone where a
    /// VPC-based endpoint's association names a subnet.
    pub fn client_vpn_endpoint_has_target_zone(&self, endpoint_id: &str, zone_name: &str) -> bool {
        self.client_vpn_target_networks.values().any(|record| {
            record.view.client_vpn_endpoint_id.as_deref() == Some(endpoint_id)
                && record
                    .view
                    .availability_zones
                    .iter()
                    .flatten()
                    .any(|zone| zone == zone_name)
        })
    }

    /// Apply the `associating` -> `associated` move every target network association that
    /// has reached its due instant by `now` owes.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// calls: the move was owed at its instant whether or not anybody asked, so applying it
    /// on a read changes nothing a later caller would not have seen anyway. The endpoint's
    /// own `pending-associate` -> `available` move is NOT here: it belongs to the operation
    /// that associates the network, which is what decides an endpoint "has been created and
    /// a target network has been associated" (`ClientVpnEndpointStatusCode`).
    pub(crate) fn materialize_due_client_vpn_target_networks(&mut self, now: Instant) {
        for record in self.client_vpn_target_networks.values_mut() {
            let due = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string);
            if let Some(code) = due {
                record.view.status = Some(crate::ops::types::AssociationStatus {
                    code: Some(code),
                    message: None,
                });
                record.pending = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The routes of a Client VPN endpoint's route table
// ---------------------------------------------------------------------------

/// "creating" — the state a route starts in (`ClientVpnRouteStatusCode`), which
/// `CreateClientVpnRoute` answers with.
pub const CLIENT_VPN_ROUTE_STATUS_CREATING: &str = "creating";

/// "active" — the state the clock owes a `creating` route ([`TRANSITION_DELAY`] later,
/// through [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const CLIENT_VPN_ROUTE_STATUS_ACTIVE: &str = "active";

/// "add-route" — the `Origin` of a route a caller added: "`add-route` indicates that the
/// route was manually added using the CreateClientVpnRoute action"
/// (`ClientVpnRoute$Origin`).
pub const CLIENT_VPN_ROUTE_ORIGIN_ADD_ROUTE: &str = "add-route";

/// "associate" — the other `Origin` that member takes: "`associate` indicates that the route
/// was automatically added when the target network was associated with the Client VPN
/// endpoint" (`ClientVpnRoute$Origin`), which is the operation that associates a target
/// network's to write, not this one's.
pub const CLIENT_VPN_ROUTE_ORIGIN_ASSOCIATE: &str = "associate";

/// "Nat" — the `Type` ("The route type", `ClientVpnRoute$Type`) EC2 reports for every route
/// of a Client VPN endpoint, as the AWS CLI reference's own `describe-client-vpn-routes`
/// example output reports both the route added with a target network and the route added by
/// `CreateClientVpnRoute`.
pub const CLIENT_VPN_ROUTE_TYPE_NAT: &str = "Nat";

/// The `TargetVpcSubnetId` that names the endpoint's own client network rather than a
/// subnet: "if you're adding a route for the local network, specify `local`"
/// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`). It is not a subnet id, so it is not
/// resolved against the endpoint's target networks.
pub const CLIENT_VPN_ROUTE_TARGET_LOCAL: &str = "local";

/// `InvalidClientVpnDuplicateRoute` (HTTP 400): the endpoint's route table already holds a
/// route for that destination through that target, so adding this one would make two.
pub const INVALID_CLIENT_VPN_DUPLICATE_ROUTE: &str = "InvalidClientVpnDuplicateRoute";

/// `InvalidClientVpnActiveAssociationNotFound` (HTTP 400): the subnet a route would send
/// traffic through is not a target network of the endpoint, so there is no path for the
/// route to be programmed into.
pub const INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND: &str =
    "InvalidClientVpnActiveAssociationNotFound";

/// What separates a route's destination from its target in the position a page token names
/// ([`ClientVpnRouteKey::cursor`]): a character that orders BELOW every character a CIDR
/// block or a subnet id carries, so comparing two cursors as strings orders the pairs they
/// name rather than running the two halves together.
pub const CLIENT_VPN_ROUTE_CURSOR_SEPARATOR: char = ' ';

/// Refuse a route an endpoint's route table already holds: HTTP 400
/// [`INVALID_CLIENT_VPN_DUPLICATE_ROUTE`], naming the destination and the endpoint the
/// request sent.
pub fn client_vpn_duplicate_route<E>(destination_cidr: &str, endpoint_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_DUPLICATE_ROUTE,
        400,
        format!("Route {destination_cidr} already exists in Client VPN endpoint {endpoint_id}"),
    )
}

/// The id prefix EC2 mints a subnet with — `subnet-`, the prefix
/// [`ComAmazonawsEc2Amazonec2State::insert_subnet`] mints a subnet id under. A
/// `TargetVpcSubnetId` is read against it, because a value that is not a subnet id at all is
/// refused before anything is looked up.
pub const SUBNET_ID_PREFIX: &str = "subnet";

/// `InvalidSubnetID.Malformed` (HTTP 400): the value given for a subnet is not a subnet id.
///
/// The same `Invalid<Kind>.Malformed` question [`require_well_formed_id`] asks of every
/// resource id of this crate, in the wording the Client VPN surface answers it with — `The
/// subnet ID '<id>' is malformed`, naming the member's own resource kind rather than the
/// generic [`invalid_id_message`].
pub const INVALID_SUBNET_ID_MALFORMED: &str = "InvalidSubnetID.Malformed";

/// The most characters of a refused value for a subnet its message repeats back.
///
/// The message names the subnet the request could not resolve, and that is the request's own
/// bytes — `SubnetId` is a bare string the model puts no `@length` on
/// (`smithy_query(query="shape",shape="SubnetId")`) — so the echo is bounded here the way
/// [`MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS`] bounds a refused endpoint id's. Far above the
/// longest subnet id EC2 issues ([`SUBNET_ID_PREFIX`] plus 17 digits), so a real id is always
/// named in full.
pub const MAX_ECHOED_SUBNET_ID_CHARS: usize = 128;

/// Refuse a value for a subnet that is not a subnet of the account: HTTP 400
/// [`INVALID_SUBNET_ID_MALFORMED`], naming the value the request sent, of which the first
/// [`MAX_ECHOED_SUBNET_ID_CHARS`] characters are named.
pub fn invalid_subnet_id_malformed<E>(subnet_id: &str) -> Scaffolded<E> {
    let echoed: String = subnet_id.chars().take(MAX_ECHOED_SUBNET_ID_CHARS).collect();
    Scaffolded::service_error(
        INVALID_SUBNET_ID_MALFORMED,
        400,
        format!("The subnet ID '{echoed}' is malformed"),
    )
}

/// Refuse a route through a subnet that is not a target network of the endpoint: HTTP 400
/// [`INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND`], naming the subnet and the endpoint.
pub fn client_vpn_active_association_not_found<E>(
    subnet_id: &str,
    endpoint_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND,
        400,
        format!(
            "There is no active association for subnet {subnet_id} in Client VPN endpoint \
             {endpoint_id}"
        ),
    )
}

/// What names ONE route of a Client VPN endpoint's route table: the endpoint it belongs to,
/// the destination it routes, and the target that traffic leaves through.
///
/// Those three are exactly what `DeleteClientVpnRoute` names a route by
/// (`ClientVpnEndpointId` + `DestinationCidrBlock` + `TargetVpcSubnetId`), and
/// `ClientVpnRoute` carries no id member of its own, so the store is keyed by them rather
/// than by a minted id. `target` is the empty string for a route of a Transit Gateway-based
/// endpoint, where "this parameter is not required"
/// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`) because traffic leaves through the
/// endpoint's attachment instead.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientVpnRouteKey {
    /// "The ID of the Client VPN endpoint with which the route is associated"
    /// (`ClientVpnRoute$ClientVpnEndpointId`).
    pub client_vpn_endpoint_id: String,
    /// "The IPv4 address range, in CIDR notation, of the route destination"
    /// (`ClientVpnRoute$DestinationCidr`), in the canonical form this service stores every
    /// CIDR member in.
    pub destination_cidr: String,
    /// "The ID of the subnet through which traffic is routed"
    /// (`ClientVpnRoute$TargetSubnet`), [`CLIENT_VPN_ROUTE_TARGET_LOCAL`] for a route of the
    /// local network, or empty for a route that leaves through a Transit Gateway attachment.
    pub target: String,
}

impl ClientVpnRouteKey {
    /// This route's position in the order its endpoint's routes are reported and paged in:
    /// the destination, then the target.
    ///
    /// A single string so a page token can name a position with one cursor
    /// ([`CLIENT_VPN_ROUTE_CURSOR_SEPARATOR`] says why the halves cannot run together), and
    /// the reporting path both SORTS and RESUMES by it, so the paging order and the cursor
    /// are the same order by construction.
    pub fn cursor(&self) -> String {
        format!(
            "{destination}{CLIENT_VPN_ROUTE_CURSOR_SEPARATOR}{target}",
            destination = self.destination_cidr,
            target = self.target,
        )
    }
}

/// One route of a Client VPN endpoint's route table: the modelled
/// [`crate::ops::types::ClientVpnRoute`] every operation that reports one answers with, plus
/// the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// of this module holds one: a member a writer sets round-trips through
/// `DescribeClientVpnRoutes` without a field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct ClientVpnRouteRecord {
    /// The route as the model shapes it, carrying the status reached so far.
    pub view: crate::ops::types::ClientVpnRoute,
    /// The `ClientVpnRouteStatusCode` the clock owes this route while it is still transient
    /// (`creating` -> `active`); `None` once it has been applied.
    pub pending: Option<PendingStateMove>,
}

/// What one route is written with: the endpoint whose route table it joins, the destination
/// it routes, what traffic for that destination leaves through, the description the
/// request gave it, and what added it.
///
/// Every member the service itself owns — the route's type and the status it starts in — is
/// derived by [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_route`] rather than taken
/// from a caller, so two writers cannot report one route differently.
#[derive(Debug, Clone)]
pub struct ClientVpnRouteSpec {
    /// The endpoint whose route table takes the route.
    pub client_vpn_endpoint_id: String,
    /// The destination range, in the canonical form this service stores a CIDR member in.
    pub destination_cidr: String,
    /// The subnet traffic is routed through, or [`CLIENT_VPN_ROUTE_TARGET_LOCAL`]; `None`
    /// for a Transit Gateway-based endpoint, whose routes leave through its attachment.
    pub target_subnet: Option<String>,
    /// "The ID of the Transit Gateway attachment, if the route targets a Transit Gateway"
    /// (`ClientVpnRoute$TransitGatewayAttachmentId`) — read off the endpoint's own stored
    /// configuration, never from the request.
    pub transit_gateway_attachment_id: Option<String>,
    /// "A brief description of the route" (`CreateClientVpnRouteRequest$Description`).
    pub description: Option<String>,
    /// What added the route: [`CLIENT_VPN_ROUTE_ORIGIN_ADD_ROUTE`] for one a caller added,
    /// [`CLIENT_VPN_ROUTE_ORIGIN_ASSOCIATE`] for one a target network association brought
    /// with it (`ClientVpnRoute$Origin`). A `&'static str` so the origin is always one of
    /// the two the model declares.
    pub origin: &'static str,
}

/// One route as the model shapes it, beside the key it is held under: what
/// [`ClientVpnRouteSpec`] means, with every member the service owns derived.
///
/// The ONE place a route is built, so the route `CreateClientVpnRoute` adds and the route a
/// target network association brings with it cannot be shaped differently.
fn client_vpn_route_record(
    spec: ClientVpnRouteSpec,
) -> (ClientVpnRouteKey, crate::ops::types::ClientVpnRoute) {
    let key = ClientVpnRouteKey {
        client_vpn_endpoint_id: spec.client_vpn_endpoint_id.clone(),
        destination_cidr: spec.destination_cidr.clone(),
        target: spec.target_subnet.clone().unwrap_or_default(),
    };
    let view = crate::ops::types::ClientVpnRoute {
        client_vpn_endpoint_id: Some(spec.client_vpn_endpoint_id),
        destination_cidr: Some(spec.destination_cidr),
        target_subnet: spec.target_subnet,
        r#type: Some(CLIENT_VPN_ROUTE_TYPE_NAT.to_string()),
        origin: Some(spec.origin.to_string()),
        status: Some(crate::ops::types::ClientVpnRouteStatus {
            code: Some(CLIENT_VPN_ROUTE_STATUS_CREATING.to_string()),
            message: None,
        }),
        description: spec.description,
        transit_gateway_attachment_id: spec.transit_gateway_attachment_id,
    };
    (key, view)
}

/// The caller-chosen bytes one stored route holds: the ids of the resources it joins, the
/// destination range and the description, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way
/// [`client_vpn_target_network_payload_bytes`] is for the store beside it: the insert charges
/// exactly it, and a removal path must give back the same number.
pub fn client_vpn_route_payload_bytes(view: &crate::ops::types::ClientVpnRoute) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.client_vpn_endpoint_id.as_deref().map_or(0, str::len))
        .saturating_add(view.destination_cidr.as_deref().map_or(0, str::len))
        .saturating_add(view.target_subnet.as_deref().map_or(0, str::len))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(
            view.transit_gateway_attachment_id
                .as_deref()
                .map_or(0, str::len),
        )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks this BEFORE inserting, because nothing here reclaims a stored route.
    pub fn has_client_vpn_route_capacity(&self) -> bool {
        self.client_vpn_routes.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The route `key` names, or `None` when that endpoint's route table holds none.
    pub fn client_vpn_route(&self, key: &ClientVpnRouteKey) -> Option<&ClientVpnRouteRecord> {
        self.client_vpn_routes.get(key)
    }

    /// Is `subnet_id` a target network of `endpoint_id`?
    ///
    /// The question `CreateClientVpnRoute` asks of its target subnet — "The specified subnet
    /// must be an existing target network of the Client VPN endpoint"
    /// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`) — answered against the association
    /// store, so the route path and `DescribeClientVpnTargetNetworks` read one set of
    /// associations.
    pub fn client_vpn_endpoint_has_target_network(
        &self,
        endpoint_id: &str,
        subnet_id: &str,
    ) -> bool {
        self.client_vpn_target_networks.values().any(|record| {
            record.view.client_vpn_endpoint_id.as_deref() == Some(endpoint_id)
                && record.view.target_network_id.as_deref() == Some(subnet_id)
        })
    }

    /// Add one route to a Client VPN endpoint's route table and answer the key it is held
    /// under beside the route as its writer reports it — or `None` when one of the store's
    /// bounds would be crossed and NOTHING was stored.
    ///
    /// The seam `CreateClientVpnRoute` calls. Every member the service owns is derived by
    /// [`client_vpn_route_record`]: the route `Type` EC2 reports for a Client VPN route and
    /// the `creating` status it starts in — which the clock owes `active` one
    /// [`TRANSITION_DELAY`] later through [`Self::materialize_due_transitions`]. The span is
    /// never zero, so a caller describing the route straight after the call that made it
    /// still sees `creating`.
    pub fn insert_client_vpn_route(
        &mut self,
        spec: ClientVpnRouteSpec,
    ) -> Option<(ClientVpnRouteKey, crate::ops::types::ClientVpnRoute)> {
        let (key, view) = client_vpn_route_record(spec);
        let charge = client_vpn_route_payload_bytes(&view);
        if !self.has_client_vpn_route_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let stored = view.clone();
        self.store_client_vpn_route(key.clone(), view);
        Some((key, stored))
    }

    /// Hold `view` under `key`, charged for and owed its `creating` -> `active` move.
    ///
    /// The one write of the route store, so the route a caller adds and the route a target
    /// network association brings with it are charged and aged the same way. It asks NO
    /// bound of its own: a caller clears the store's bounds first — which is what lets an
    /// association clear its own and its route's together.
    fn store_client_vpn_route(
        &mut self,
        key: ClientVpnRouteKey,
        view: crate::ops::types::ClientVpnRoute,
    ) {
        self.add_stored_bytes(client_vpn_route_payload_bytes(&view));
        self.client_vpn_routes.insert(
            key,
            ClientVpnRouteRecord {
                view,
                pending: Some(PendingStateMove::starting_now(
                    CLIENT_VPN_ROUTE_STATUS_ACTIVE,
                )),
            },
        );
    }

    /// Apply the `creating` -> `active` move every route that has reached its due instant by
    /// `now` owes.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// calls: the move was owed at its instant whether or not anybody asked, so applying it
    /// on a read changes nothing a later caller would not have seen anyway.
    pub(crate) fn materialize_due_client_vpn_routes(&mut self, now: Instant) {
        for record in self.client_vpn_routes.values_mut() {
            let due = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string);
            if let Some(code) = due {
                record.view.status = Some(crate::ops::types::ClientVpnRouteStatus {
                    code: Some(code),
                    message: None,
                });
                record.pending = None;
            }
        }
    }
}
