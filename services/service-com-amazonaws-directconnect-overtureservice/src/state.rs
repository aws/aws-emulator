// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-directconnect-overtureservice service.
//!
//! The connection + private virtual interface + Direct Connect gateway slice of
//! Direct Connect lives here: a
//! handler mints a record, stores it, and every later response is rendered FROM the
//! stored record (see the [`connection_output!`] / [`virtual_interface_output!`]
//! macros), so a create followed by a describe answers the same bytes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{BGPPeer, RouteFilterPrefix, Tag};

/// The gateways the ACCOUNT holds, asked of the service that owns them.
pub mod ec2_network;
/// The associations between a Direct Connect gateway and those gateways.
pub mod gateway_associations;
/// The resiliency groups the ACCOUNT holds.
pub mod resiliency_groups;
/// The location catalog's stored record and the rendering of it.
mod location_catalog;
/// The locations the catalog publishes, one row per published `Location`.
mod published_locations;
pub use location_catalog::*;
pub use published_locations::*;

/// This service's ARN namespace, from its model (`awsService.arnNamespace:
/// directconnect`) — the vendor field of every ARN it publishes, and the one the
/// patterns of `ResiliencyGroupArn` and `ResourceArn` carry.
pub const DIRECT_CONNECT_ARN_NAMESPACE: &str = "directconnect";

/// The resource path a Direct Connect GATEWAY's ARN carries before the gateway's own
/// id.
///
/// The resource type AWS Resource Groups Tagging and IAM name a Direct Connect gateway
/// under is `dx-gateway`, which is the segment
/// `arn:<partition>:directconnect::<account>:dx-gateway/<id>` carries. A gateway "is
/// global and visible in any Amazon Web Services Region after it is created", so such an
/// ARN carries no Region at all. The gateway shape this service answers with declares no
/// ARN member, so this path is never PUBLISHED: it is only parsed out of an ARN a client
/// built itself and named a gateway by, on the tag read ([`tags_of_resource`]) and on the
/// tag write ([`tags_of_resource_mut`]) alike, both of which resolve it through the one
/// [`tagged_resource`].
pub const DIRECT_CONNECT_GATEWAY_ARN_PATH: &str = "dx-gateway";

/// The resource paths a CONNECTION's and a VIRTUAL INTERFACE's ARN carry before the
/// resource's own id (`arn:<partition>:directconnect:<region>:<account>:dxcon/<id>`
/// and `…:dxvif/<id>`), which are the id prefixes those resources are minted under.
/// Either resource is landed at one Direct Connect location, so its ARN carries that
/// resource's own Region.
pub const CONNECTION_ARN_PATH: &str = "dxcon";
pub const VIRTUAL_INTERFACE_ARN_PATH: &str = "dxvif";

/// Which kind of this service's resources an ARN names, and the identifier and Region
/// it names it under.
///
/// The ONE resolution of an ARN to a tagged resource of this service, so every caller
/// that reads OR writes tags by ARN resolves the same four kinds — a connection, a
/// virtual interface, a Direct Connect gateway and a resiliency group — and none of them
/// resolves one kind differently.
///
/// The ARN is PARSED once ([`::service_common::arn::parse`], the one reading of an ARN
/// this crate has) and its resource segment split into the resource PATH and the
/// identifier, so a value that is not an ARN of this service resolves to nothing, the
/// path alone says which store to look in, and the record is found by its own key
/// rather than by rebuilding every candidate ARN. The Region the ARN carries is held to
/// what the resource's own ARN carries: none for a gateway or a group (both are global),
/// and the resource's own Region for a connection or a virtual interface.
enum TaggedResource<'a> {
    Connection { id: &'a str, region: &'a str },
    VirtualInterface { id: &'a str, region: &'a str },
    Gateway { id: &'a str },
    ResiliencyGroup { id: &'a str },
}

/// What `resource_arn` names, or `None` where it is not an ARN of a resource kind this
/// service tags. See [`TaggedResource`].
fn tagged_resource(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let arn = ::service_common::arn::parse(resource_arn)?;
    if arn.vendor != DIRECT_CONNECT_ARN_NAMESPACE {
        return None;
    }
    let (path, id) = arn.resource.split_once('/')?;
    match path {
        CONNECTION_ARN_PATH => Some(TaggedResource::Connection {
            id,
            region: arn.region,
        }),
        VIRTUAL_INTERFACE_ARN_PATH => Some(TaggedResource::VirtualInterface {
            id,
            region: arn.region,
        }),
        DIRECT_CONNECT_GATEWAY_ARN_PATH if arn.region.is_empty() => {
            Some(TaggedResource::Gateway { id })
        }
        resiliency_groups::RESILIENCY_GROUP_ARN_PATH if arn.region.is_empty() => {
            Some(TaggedResource::ResiliencyGroup { id })
        }
        _ => None,
    }
}

/// The tags this account holds on the Direct Connect resource `resource_arn`, or
/// `None` where it holds no resource under that ARN.
pub fn tags_of_resource(
    store: &ComAmazonawsDirectconnectOvertureserviceState,
    resource_arn: &str,
) -> Option<Vec<Tag>> {
    let tags = match tagged_resource(resource_arn)? {
        TaggedResource::Connection { id, region } => store
            .connections
            .get(id)
            .filter(|connection| connection.region == region)
            .map(|connection| &connection.tags),
        TaggedResource::VirtualInterface { id, region } => store
            .virtual_interfaces
            .get(id)
            .filter(|interface| interface.region == region)
            .map(|interface| &interface.tags),
        TaggedResource::Gateway { id } => store.gateways.get(id).map(|gateway| &gateway.tags),
        TaggedResource::ResiliencyGroup { id } => {
            store.resiliency_groups.get(id).map(|group| &group.tags)
        }
    };
    tags.cloned()
}

/// The tags this account holds on the Direct Connect resource `resource_arn`, to be
/// CHANGED in the record that holds them, or `None` where it holds no resource under
/// that ARN.
///
/// The write side of [`tags_of_resource`], resolving the ARN the same way, so a tag a
/// caller writes by an ARN is read back by that same ARN.
pub fn tags_of_resource_mut<'a>(
    store: &'a mut ComAmazonawsDirectconnectOvertureserviceState,
    resource_arn: &str,
) -> Option<&'a mut Vec<Tag>> {
    match tagged_resource(resource_arn)? {
        TaggedResource::Connection { id, region } => store
            .connections
            .get_mut(id)
            .filter(|connection| connection.region == region)
            .map(|connection| &mut connection.tags),
        TaggedResource::VirtualInterface { id, region } => store
            .virtual_interfaces
            .get_mut(id)
            .filter(|interface| interface.region == region)
            .map(|interface| &mut interface.tags),
        TaggedResource::Gateway { id } => {
            store.gateways.get_mut(id).map(|gateway| &mut gateway.tags)
        }
        TaggedResource::ResiliencyGroup { id } => store
            .resiliency_groups
            .get_mut(id)
            .map(|group| &mut group.tags),
    }
}

/// How this service STATES a refusal of a tag call whose `resourceArn` names no resource
/// it holds.
///
/// Every client fault of this service shares the one error shape and the one 400
/// status, so the sentence is the whole discriminator — and there are two faults to tell
/// apart, since `ResourceArn` carries no `@pattern` and so a value that is not an ARN at
/// all reaches the call as readily as an ARN naming nothing. Both sentences are stated
/// here beside this service's other refusal wording, so the tag write and the tag removal
/// word each alike:
///
/// * a value that is not an ARN is refused about its FORM, naming the value the request
///   carried. RECORDED behaviour: the FIXED text is "Invalid ARN format: ".
/// * an ARN that parses but resolves to no resource of this account is refused about the
///   resource: the ARN the request submitted, then fixed text.
pub fn resource_arn_not_found_message(resource_arn: &str) -> String {
    match ::service_common::arn::parse(resource_arn) {
        None => format!("Invalid ARN format: {resource_arn}"),
        Some(_) => format!("Resource {resource_arn} does not exist"),
    }
}

/// How a tag REMOVAL states a refusal of a key list longer than the tag set a resource
/// can hold: the count the request named, then the fixed sentence naming the ceiling.
///
/// `TagKeyList` carries no `@length`
/// (`smithy_query(query="operation",operation="UntagResource")`) and `UntagResource`
/// models no quota error, so the bound is enforced as the one client fault this
/// operation has — and it is a bound on the REQUEST, since a resource holds at most
/// [`MAX_TAGS_PER_RESOURCE`] tags and a longer list therefore names keys no resource of
/// this service can carry. Stated here beside the bound it reports, so the sentence and
/// the number cannot drift apart.
pub fn too_many_tag_keys_message(named: usize) -> String {
    format!(
        "{named} tag keys were specified; a Direct Connect resource holds at most \
         {MAX_TAGS_PER_RESOURCE} tags"
    )
}

/// The Region a request names in its SigV4 credential scope when it carries no
/// `Authorization` header at all. Matches the Region the emulator's own generated
/// shared config names (`emulator_server`'s `[default] region = us-east-1`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a state change this service owes the
/// clock falls due.
///
/// CHOSEN, not documented: AWS takes minutes to tear a connection or a virtual
/// interface down, which no test can wait for. It is deliberately non-zero so a
/// caller that reads straight back still observes the transient `deleting` /
/// `pending` state the model documents, and the instant is stored rather than
/// slept on, so no handler blocks.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The MTUs `NewPrivateVirtualInterface$mtu` documents: "The supported values are
/// 1500 and 8500. The default value is 1500."
pub const SUPPORTED_VIRTUAL_INTERFACE_MTUS: &[i32] = &[1500, 8500];

/// The default MTU `NewPrivateVirtualInterface$mtu` documents.
pub const DEFAULT_VIRTUAL_INTERFACE_MTU: i32 = 1500;

/// The largest `asn` `NewPrivateVirtualInterface$asn` documents ("The valid values
/// are 1-2147483646").
pub const MAX_ASN: i64 = 2_147_483_646;

/// The largest `asnLong` `NewPrivateVirtualInterface$asnLong` documents ("The valid
/// range is from 1 to 4294967294").
pub const MAX_ASN_LONG: i64 = 4_294_967_294;

/// The largest value the legacy 2/4-byte `asn` member can carry;
/// `NewPrivateVirtualInterface$asnLong` documents that a larger `asnLong` is
/// reported back as `0` in `asn` "since 4-byte ASN values exceed the maximum
/// supported value of 2,147,483,647".
pub const MAX_LEGACY_ASN: i64 = 2_147_483_647;

/// The `amazonSideAsn` a `CreateDirectConnectGateway` request that names none
/// gets: `CreateDirectConnectGatewayRequest$amazonSideAsn` documents "The default
/// is 64512."
pub const DEFAULT_AMAZON_SIDE_ASN: i64 = 64_512;

/// The ASN ranges `CreateDirectConnectGatewayRequest$amazonSideAsn` documents:
/// "The ASN must be in the private range of 64,512 to 65,534 or 4,200,000,000 to
/// 4,294,967,294." Each entry is an inclusive `(low, high)` pair.
pub const PRIVATE_AMAZON_SIDE_ASN_RANGES: [(i64, i64); 2] =
    [(64_512, 65_534), (4_200_000_000, 4_294_967_294)];

/// Whether `asn` lies in one of the private ranges
/// [`PRIVATE_AMAZON_SIDE_ASN_RANGES`] holds.
pub fn amazon_side_asn_is_private(asn: i64) -> bool {
    PRIVATE_AMAZON_SIDE_ASN_RANGES
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&asn))
}

/// How this service STATES a refusal of a listing of associations that selects by nothing.
///
/// `DescribeDirectConnectGatewayAssociations` publishes its listing in forms that each name at
/// least one of the Direct Connect gateway, the gateway associated with it, or the association
/// itself ("You must specify one of the following: A Direct Connect gateway … A virtual private
/// gateway … A transit gateway …"), so a request carrying none of those members asks for no
/// listing at all. FIXED text: nothing of the request varies it. Stated here beside this
/// service's other refusal wording.
pub const ASSOCIATION_SELECTOR_REQUIRED_MESSAGE: &str =
    "Either Direct Connect Gateway ID or Associated Gateway ID or Association ID must be set";

/// How this service STATES a refusal of an Amazon-side ASN outside the ranges
/// [`PRIVATE_AMAZON_SIDE_ASN_RANGES`] holds.
///
/// `amazonSideAsn` carries no `@range` in the model
/// (`smithy_query(query="operation",operation="CreateDirectConnectGateway")`), so the
/// bound is the documented one and the sentence reporting it is this service's own: the
/// VALUE the request submitted, then fixed text. The member is not named and the ranges
/// are not restated — a client branches on this sentence to tell a bad ASN from any
/// other client fault, since every one of them shares the one error shape and the one
/// 400 status. Stated here once, so every caller that refuses such a value words it
/// alike.
pub fn amazon_side_asn_not_valid_message(asn: i64) -> String {
    format!("{asn} is not a valid value for Amazon Side ASN")
}

/// How this service STATES a refusal naming a Direct Connect gateway that no account
/// holds.
///
/// Every operation that resolves a `directConnectGatewayId` and finds no gateway under it
/// words the refusal from here, so a client matching this sentence to decide "already
/// deleted, treat as success" matches it on each of them. `DirectConnectClientException`
/// carries nothing a client can branch on but the sentence, and the model states none, so
/// this is RECORDED behaviour: the FIXED text is "Direct Connect Gateway for owner ",
/// " and with id " and " does not exist", and the VARIABLE parts are the owner account the
/// lookup was made for and the id the request carried — the account being the
/// distinguishing detail when a gateway exists but is owned elsewhere.
pub fn direct_connect_gateway_not_found_message(owner_account: &str, gateway_id: &str) -> String {
    format!(
        "Direct Connect Gateway for owner {owner_account} and with id {gateway_id} does not exist"
    )
}

/// The largest page a `maxResults` of `MaxResultSetSize` can ask for: "If
/// MaxResults is given a value larger than 100, only 100 results are returned."
pub const MAX_RESULT_SET_SIZE: i32 = 100;

/// The longest `virtualInterfaceName`
/// `NewPrivateVirtualInterface$virtualInterfaceName` documents: "The name has a
/// maximum of 100 characters."
pub const MAX_VIRTUAL_INTERFACE_NAME_LEN: usize = 100;

/// Whether `name` satisfies what
/// `NewPrivateVirtualInterface$virtualInterfaceName` documents: at most
/// [`MAX_VIRTUAL_INTERFACE_NAME_LEN`] characters, and "the following are valid
/// characters: a-z, 0-9 and a hyphen (-)".
pub fn virtual_interface_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_VIRTUAL_INTERFACE_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The number of records this emulator keeps in each caller-driven store.
///
/// An EMULATOR memory ceiling, not an AWS quota: every successful create inserts
/// a record holding caller-supplied strings, so without a ceiling one caller
/// could grow the process without bound. At the ceiling a create first reclaims
/// the records the clock has already torn down
/// ([`ComAmazonawsDirectconnectOvertureserviceState::reclaim_deleted`]) and, if
/// that frees nothing, is refused with a modelled error naming the ceiling.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The number of tags AWS allows on one Direct Connect resource (the "Tags per
/// Direct Connect resource" service quota). A request carrying more is refused
/// with the modelled `TooManyTagsException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many resources ONE tag read may name.
///
/// An EMULATOR bound, not an AWS quota: `ResourceArnList` carries no `@length`
/// (`smithy_query(query="operation",operation="DescribeTags")`), so the list a request
/// names is caller-sized and both the work that read does and the tag set it renders grow
/// with it — the product of the batch and [`MAX_TAGS_PER_RESOURCE`]. Bounded here, beside
/// the other bounds of this store, and enforced before any element is resolved, so one
/// request cannot make the read cost — or answer — what that request chose. The ceiling is
/// the one AWS publishes for the same input shape on the service whose own `DescribeTags`
/// takes an ARN list ("you can specify up to 20 resources in a single call").
pub const MAX_RESOURCES_PER_CALL: usize = 20;

/// How a tag read STATES a refusal of a batch past [`MAX_RESOURCES_PER_CALL`]: the count
/// the request named, then the fixed sentence naming the ceiling. Stated here beside the
/// bound it reports, so the sentence and the number cannot drift apart.
pub fn too_many_resources_message(named: usize) -> String {
    format!(
        "{named} resources were specified; you can specify up to {MAX_RESOURCES_PER_CALL} \
         resources in a single call"
    )
}

/// The first tag key that appears more than once in `tags`, which the modelled
/// `DuplicateTagKeysException` ("A tag key was specified more than once") refuses.
///
/// `BTreeSet` membership rather than a linear scan, so the cost is O(n log n) in
/// the caller-chosen tag count whatever order a caller's checks run in.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for tag in tags {
        if !seen.insert(tag.key.as_str()) {
            return Some(tag.key.as_str());
        }
    }
    None
}

/// How this service STATES a refusal of a tag set naming one key twice, which the modelled
/// `DuplicateTagKeysException` ("A tag key was specified more than once") refuses.
///
/// The model states no sentence, so this is RECORDED behaviour: the FIXED text is "Tag "
/// and " is specified more than once.", and the VARIABLE part is the key the request
/// repeated. Stated here once, so every write that refuses such a set words it alike.
pub fn duplicate_tag_key_message(key: &str) -> String {
    format!("Tag {key} is specified more than once.")
}

/// How this service STATES a refusal of a tag set past [`MAX_TAGS_PER_RESOURCE`], which the
/// modelled `TooManyTagsException` refuses.
///
/// The sentence is the one the model's own `TooManyTagsException` documents ("you have
/// reached the limit on the number of tags that can be assigned"); it is FIXED — nothing
/// of the request varies it, and the limit is not restated in it. Stated here once, so
/// every write of this service that refuses such a set words it alike.
pub const TOO_MANY_TAGS_MESSAGE: &str =
    "You have reached the limit on the number of tags that can be assigned.";

/// Apply `submitted` to the tag set `held` a resource already carries: "if a tag with the
/// same key is already associated with the resource, this action updates its value", and a
/// key the resource does not hold yet is added after the ones it does — so a repeated write
/// of the same tag set changes nothing.
///
/// The ONE merge of a submitted tag set into a stored one, so every tag write of this
/// service updates and appends alike.
pub fn merge_tags(held: &mut Vec<Tag>, submitted: Vec<Tag>) {
    for tag in submitted {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// A state change the clock owes a resource: at [`DueTransition::at`] the resource
/// moves to [`DueTransition::to`].
///
/// A reporting operation materialises it through
/// [`ComAmazonawsDirectconnectOvertureserviceState::settle_due_transitions`] — the
/// change was owed at that instant whether or not anybody called.
#[derive(Debug, Clone)]
pub struct DueTransition {
    /// The instant the move falls due.
    pub at: Instant,
    /// The state the resource moves to.
    pub to: String,
}

/// A stored Direct Connect connection — the `Connection` shape's data, owned by
/// this service rather than rebuilt per response.
#[derive(Debug, Clone)]
pub struct Connection {
    pub connection_id: String,
    pub connection_name: String,
    pub connection_state: String,
    pub location: String,
    pub bandwidth: String,
    pub region: String,
    pub owner_account: String,
    /// `0` for a port: a dedicated connection carries no VLAN tag of its own, and
    /// `Connection$vlan` is a required member with a `default` of `0`.
    pub vlan: i32,
    pub lag_id: Option<String>,
    pub provider_name: Option<String>,
    pub billing_mode: Option<String>,
    pub mac_sec_capable: bool,
    pub jumbo_frame_capable: bool,
    pub has_logical_redundancy: String,
    pub aws_device: String,
    pub aws_device_v2: String,
    pub aws_logical_device_id: String,
    /// Set by a call to `DescribeLoa`/`DescribeConnectionLoa`, which
    /// `Connection$loaIssueTime` defines as "the time of the most recent call to
    /// DescribeLoa for this connection". Both are still scaffolded, so no stored
    /// connection has one yet.
    pub loa_issue_time: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    pub due: Option<DueTransition>,
}

/// A stored Direct Connect gateway - the `DirectConnectGateway` shape's data.
///
/// A Direct Connect gateway "is global and visible in any Amazon Web Services
/// Region after it is created", so unlike a [`Connection`] it holds no Region of
/// its own.
#[derive(Debug, Clone)]
pub struct DirectConnectGateway {
    pub direct_connect_gateway_id: String,
    pub direct_connect_gateway_name: String,
    pub amazon_side_asn: i64,
    pub owner_account: String,
    pub direct_connect_gateway_state: String,
    /// `DirectConnectGateway$stateChangeError`: "the error message if the state
    /// of an object failed to advance". `None` while no advance has failed.
    pub state_change_error: Option<String>,
    pub tags: Vec<Tag>,
    pub due: Option<DueTransition>,
}

/// A stored private virtual interface — the `VirtualInterface` shape's data.
///
/// Membership of a connection is held ONCE, here, as [`Self::connection_id`]:
/// `DescribeVirtualInterfaces` filters on it rather than on a second list kept
/// beside the connection, so the two cannot disagree.
#[derive(Debug, Clone)]
pub struct VirtualInterface {
    pub virtual_interface_id: String,
    pub connection_id: String,
    pub owner_account: String,
    pub virtual_interface_name: String,
    pub virtual_interface_type: String,
    pub virtual_interface_state: String,
    pub vlan: i32,
    pub asn: i32,
    pub asn_long: Option<i64>,
    pub amazon_side_asn: Option<i64>,
    pub auth_key: Option<String>,
    pub amazon_address: Option<String>,
    pub customer_address: Option<String>,
    pub address_family: String,
    pub mtu: i32,
    pub jumbo_frame_capable: bool,
    pub virtual_gateway_id: Option<String>,
    pub direct_connect_gateway_id: Option<String>,
    pub route_filter_prefixes: Vec<RouteFilterPrefix>,
    pub bgp_peers: Vec<BGPPeer>,
    pub location: String,
    pub region: String,
    pub aws_device_v2: String,
    pub aws_logical_device_id: String,
    pub site_link_enabled: bool,
    pub tags: Vec<Tag>,
    pub due: Option<DueTransition>,
}

/// Per-service state for com-amazonaws-directconnect-overtureservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsDirectconnectOvertureserviceState {
    /// Stored connections, keyed by `connectionId`.
    ///
    /// A `BTreeMap` rather than a `HashMap`: minted ids are zero-padded and
    /// monotonic, so key order IS creation order and `DescribeConnections` answers
    /// a stable list instead of a per-process-random one.
    pub connections: BTreeMap<String, Connection>,
    /// Stored virtual interfaces, keyed by `virtualInterfaceId`. Ordered for the
    /// same reason as [`Self::connections`].
    pub virtual_interfaces: BTreeMap<String, VirtualInterface>,
    /// Stored Direct Connect gateways, keyed by `directConnectGatewayId`.
    ///
    /// A gateway id is a UUID rather than a monotonic suffix, so key order is NOT
    /// creation order here: [`Self::gateway_order`] carries that, and
    /// `DescribeDirectConnectGateways` answers through it.
    pub gateways: BTreeMap<String, DirectConnectGateway>,
    /// The keys of [`Self::gateways`] in creation order.
    pub gateway_order: Vec<String>,
    /// Monotonic source of the `dxcon-`/`dxvif-` suffixes, so every minted id is
    /// unique and reproducible within a run.
    pub id_counter: u64,
    /// The Direct Connect location catalog, keyed by `locationCode`.
    ///
    /// Seeded from [`built_in_location_catalog`] when this store is created, so
    /// `DescribeLocations` is a pure read: no call creates, changes or removes a
    /// location. Keyed for lookup by code; the ORDER the catalog is answered in is
    /// [`Self::location_order`], because the order AWS lists a Region's locations in
    /// is published catalog data and is not the order their codes sort in.
    pub locations: BTreeMap<String, LocationRecord>,
    /// The keys of [`Self::locations`] in the order the catalog publishes them.
    ///
    /// A location the catalog holds but this list does not name is still answered,
    /// after the published ones, so a record put in the store out of band is never
    /// dropped from a listing.
    pub location_order: Vec<String>,
    /// Stored associations between a Direct Connect gateway and a virtual private
    /// gateway or transit gateway, keyed by `associationId`.
    ///
    /// An association id is a UUID rather than a monotonic suffix, so key order is
    /// NOT creation order here: [`Self::association_order`] carries that, and
    /// `DescribeDirectConnectGatewayAssociations` answers through it.
    pub gateway_associations: BTreeMap<String, gateway_associations::GatewayAssociation>,
    /// The keys of [`Self::gateway_associations`] in creation order.
    pub association_order: Vec<String>,
    /// Stored resiliency groups, keyed by `resiliencyGroupId`.
    ///
    /// A group id's 17 characters are derived rather than a zero-padded counter, so
    /// key order is NOT creation order here: [`Self::resiliency_group_order`] carries
    /// that, and `ListResiliencyGroups` answers through it.
    pub resiliency_groups: BTreeMap<String, resiliency_groups::ResiliencyGroup>,
    /// The keys of [`Self::resiliency_groups`] in creation order.
    pub resiliency_group_order: Vec<String>,
}

impl Default for ComAmazonawsDirectconnectOvertureserviceState {
    /// A fresh store: no caller-created records, and the location catalog already
    /// seeded. Seeded HERE rather than on first read so a reporting operation never
    /// has to write: every caller sees the same catalog from the first request on.
    fn default() -> Self {
        let published = built_in_location_catalog();
        Self {
            connections: BTreeMap::new(),
            virtual_interfaces: BTreeMap::new(),
            gateways: BTreeMap::new(),
            gateway_order: Vec::new(),
            id_counter: 0,
            location_order: published
                .iter()
                .map(|location| location.location_code.clone())
                .collect(),
            locations: published
                .into_iter()
                .map(|location| (location.location_code.clone(), location))
                .collect(),
            gateway_associations: BTreeMap::new(),
            association_order: Vec::new(),
            resiliency_groups: BTreeMap::new(),
            resiliency_group_order: Vec::new(),
        }
    }
}

impl ComAmazonawsDirectconnectOvertureserviceState {
    /// Mint the next id for `prefix` (`"dxcon"`, `"dxvif"`), in AWS's
    /// `<prefix>-xxxxxxxx` 8-character form.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        format!("{prefix}-{:08x}", self.id_counter)
    }

    /// Mint the next `directConnectGatewayId`.
    ///
    /// A Direct Connect gateway id is not one of the `dxcon-`/`dxvif-` prefixed
    /// ids: AWS hands out a version-4 UUID (`DirectConnectGatewayId` is an opaque
    /// string), so the minted id has that shape.
    pub fn next_gateway_id(&mut self) -> String {
        self.next_uuid()
    }

    /// Mint the next opaque version-4 UUID this service hands a resource out under.
    ///
    /// The low 12 hex digits ARE the monotonic counter, so every minted id is unique
    /// within a run, and the rest is derived from it, so it is reproducible. ONE
    /// minter, so a gateway id and an association id can never collide.
    pub fn next_uuid(&mut self) -> String {
        self.id_counter += 1;
        let seed = fnv1a64(&self.id_counter.to_be_bytes());
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            u32::try_from(seed >> 32).unwrap_or(0),
            (seed >> 16) & 0xffff,
            seed & 0x0fff,
            0x8000 | ((seed >> 4) & 0x3fff),
            self.id_counter & 0xffff_ffff_ffff,
        )
    }

    /// Render a stored [`DirectConnectGateway`] into the generated
    /// `DirectConnectGateway` output shape: the ONE rendering of a stored gateway, so
    /// every operation — create, listing, rename and delete alike — answers the same
    /// field set.
    ///
    /// A method rather than a free function because
    /// `DirectConnectGateway$totalPrefixPoolAllocations` ("the total number of
    /// inbound route prefixes allocated to the attachments on the Direct Connect
    /// gateway") is a property of the attachments, so it is counted from the
    /// stored virtual interfaces rather than kept as a second copy on the gateway.
    pub fn gateway_output(
        &self,
        gateway: &DirectConnectGateway,
    ) -> crate::ops::types::DirectConnectGateway {
        crate::ops::types::DirectConnectGateway {
            direct_connect_gateway_id: Some(gateway.direct_connect_gateway_id.clone()),
            direct_connect_gateway_name: Some(gateway.direct_connect_gateway_name.clone()),
            amazon_side_asn: Some(gateway.amazon_side_asn),
            owner_account: Some(gateway.owner_account.clone()),
            direct_connect_gateway_state: Some(gateway.direct_connect_gateway_state.clone()),
            state_change_error: gateway.state_change_error.clone(),
            total_prefix_pool_allocations: Some(
                self.gateway_prefix_allocations(&gateway.direct_connect_gateway_id),
            ),
            // A gateway's tag list is always PRESENT, empty when nothing tagged it,
            // rather than absent: a client reading it never has to tell "no tags" from
            // "not reported".
            tags: Some(gateway.tags.clone()),
        }
    }

    /// The inbound route prefixes allocated to the attachments on `gateway_id`,
    /// combining both address families.
    fn gateway_prefix_allocations(&self, gateway_id: &str) -> i32 {
        let allocated: usize = self
            .virtual_interfaces
            .values()
            .filter(|interface| {
                interface.direct_connect_gateway_id.as_deref() == Some(gateway_id)
                    && interface.virtual_interface_state != "deleted"
            })
            .map(|interface| interface.route_filter_prefixes.len())
            .sum();
        i32::try_from(allocated).unwrap_or(i32::MAX)
    }

    /// The stored gateways in creation order, newest last.
    pub fn gateways_in_order(&self) -> Vec<&DirectConnectGateway> {
        self.gateway_order
            .iter()
            .filter_map(|id| self.gateways.get(id))
            .collect()
    }

    /// Apply every state change the clock has already made due.
    ///
    /// This is the ONE write a reporting operation is allowed: each move was owed
    /// at [`DueTransition::at`] whether or not anybody called, so materialising it
    /// on read cannot change what a later caller sees.
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for connection in self.connections.values_mut() {
            if let Some(due) = &connection.due {
                if now >= due.at {
                    connection.connection_state = due.to.clone();
                    connection.due = None;
                }
            }
        }
        for interface in self.virtual_interfaces.values_mut() {
            if let Some(due) = &interface.due {
                if now >= due.at {
                    interface.virtual_interface_state = due.to.clone();
                    interface.due = None;
                }
            }
        }
        for gateway in self.gateways.values_mut() {
            if let Some(due) = &gateway.due {
                if now >= due.at {
                    gateway.direct_connect_gateway_state = due.to.clone();
                    gateway.due = None;
                }
            }
        }
        self.settle_due_associations();
        self.settle_due_resiliency_groups();
    }

    /// Reclaim the records the clock has already torn down.
    ///
    /// A resource in the terminal `deleted` state can no longer forward traffic
    /// and no call can bring it back, so dropping it frees memory without
    /// changing what any later caller can do. Called only when a store is at
    /// [`MAX_RECORDS_PER_STORE`], so a `deleted` record stays readable until the
    /// store actually needs the room.
    pub fn reclaim_deleted(&mut self) {
        self.connections
            .retain(|_, connection| connection.connection_state != "deleted");
        self.virtual_interfaces
            .retain(|_, interface| interface.virtual_interface_state != "deleted");
        self.gateways
            .retain(|_, gateway| gateway.direct_connect_gateway_state != "deleted");
        self.gateway_order
            .retain(|id| self.gateways.contains_key(id));
        self.resiliency_groups
            .retain(|_, group| group.state != resiliency_groups::RESILIENCY_GROUP_STATE_DELETED);
        self.resiliency_group_order
            .retain(|id| self.resiliency_groups.contains_key(id));
    }

    /// Whether `len` leaves room for one more record under
    /// [`MAX_RECORDS_PER_STORE`].
    pub fn has_room(len: usize) -> bool {
        len < MAX_RECORDS_PER_STORE
    }

    /// The virtual interfaces attached to `connection_id`, in creation order.
    pub fn interfaces_of(&self, connection_id: &str) -> Vec<&VirtualInterface> {
        self.virtual_interfaces
            .values()
            .filter(|interface| interface.connection_id == connection_id)
            .collect()
    }

    /// The catalog locations in `region`, in the order the catalog publishes them.
    ///
    /// `DescribeLocations` "lists the Direct Connect locations in the current Amazon
    /// Web Services Region", and `Location$region` is "the Amazon Web Services Region
    /// for the location", so the Region the request is signed for is what selects
    /// them. A Region the catalog holds no location in selects none, which is an
    /// empty list, not an error. The ORDER is the published one
    /// ([`Self::location_order`]), with any location that list does not name after it
    /// in code order, so a listing is stable whatever a record's code sorts as.
    pub fn locations_in_region(&self, region: &str) -> Vec<&LocationRecord> {
        let published = self
            .location_order
            .iter()
            .filter_map(|code| self.locations.get(code));
        let unlisted = self
            .locations
            .values()
            .filter(|location| !self.location_order.contains(&location.location_code));
        published
            .chain(unlisted)
            .filter(|location| location.region == region)
            .collect()
    }

    /// Render a stored [`LocationRecord`] into the generated `Location` output shape:
    /// the ONE rendering of a catalog location, so every operation answers the same
    /// field set.
    ///
    /// A method rather than a free function so a renderer that has to reach the rest
    /// of the catalog keeps ONE call site. Every value the catalog PUBLISHES for the
    /// location is answered as stored; `availableMacSecPortSpeeds`
    /// ([`mac_sec_port_speeds`]) and each billing mode's port speeds
    /// ([`location_billing_modes`]) are narrowed from the location's own stored
    /// speeds, so no two of its lists can drift apart.
    pub fn location_output(&self, location: &LocationRecord) -> crate::ops::types::Location {
        // The usage-based mode on the location's own speeds, then the flat-rate modes
        // the catalog publishes there with the Regions each one includes.
        let billing_modes = location_billing_modes(location);

        crate::ops::types::Location {
            location_code: Some(location.location_code.clone()),
            location_name: Some(location.location_name.clone()),
            region: Some(location.region.clone()),
            available_port_speeds: Some(location.available_port_speeds.clone()),
            available_providers: Some(location.available_providers.clone()),
            available_mac_sec_port_speeds: Some(mac_sec_port_speeds(
                &location.available_port_speeds,
                location.offers_mac_sec,
            )),
            available_billing_modes: Some(billing_modes),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDirectconnectOvertureserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDirectconnectOvertureserviceState>)
}

/// The Region the request is signed for, from its SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/<name>/aws4_request`), else
/// [`DEFAULT_REGION`].
///
/// Direct Connect is a regional service, so `Connection$region` is a property of
/// the request's endpoint, not of the caller's body.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether a connection of `bandwidth` supports jumbo frames.
///
/// Jumbo frames (the 8500-byte MTU `NewPrivateVirtualInterface$mtu` documents) are
/// a property of a DEDICATED port, whose bandwidths AWS quotes in Gbps; a hosted
/// sub-gigabit connection is quoted in Mbps and carries the 1500-byte MTU only.
/// Keyed on the unit of the stored bandwidth string, so it holds for every
/// bandwidth AWS offers rather than for an enumerated few.
pub fn bandwidth_supports_jumbo_frames(bandwidth: &str) -> bool {
    bandwidth.trim().to_ascii_lowercase().ends_with("gbps")
}

/// The Direct Connect endpoint device id for a resource at `location`.
///
/// AWS reports these as `<LocationCode>-<opaque suffix>`. The suffix is derived
/// from `seed` so it is stable for the life of the resource (two reads of one
/// connection report the same device) and distinct between resources.
pub fn device_id(location: &str, seed: &str) -> String {
    format!("{location}-{}", base36(fnv1a64(seed.as_bytes()), 13))
}

/// The link-local `/30` AWS assigns to the n-th virtual interface when the caller
/// supplies no addresses: the peering addresses come from `169.254.0.0/16`, two
/// usable hosts per `/30`, the Amazon side on the first and the customer side on
/// the second.
///
/// Returns `(amazonAddress, customerAddress)`.
pub fn link_local_peering_addresses(index: u64) -> (String, String) {
    // 169.254.0.0/16 holds 16384 /30 blocks; wrap so a long-lived run keeps
    // producing a well-formed address rather than overflowing out of the range.
    let block = index % 16_384;
    let third = block / 64;
    let fourth = (block % 64) * 4;
    (
        format!("169.254.{third}.{}/30", fourth + 1),
        format!("169.254.{third}.{}/30", fourth + 2),
    )
}

/// One page of a list response, plus the token that fetches the next one.
#[derive(Debug)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// The token the caller passes as `nextToken` for the following page; `None`
    /// when this page is the last one, which is what
    /// `Connections$nextToken` documents ("This value is null when there are no
    /// more results to return").
    pub next_token: Option<String>,
}

/// Window an already-ordered result set by the `maxResults`/`nextToken` pair both
/// list operations model.
///
/// The token is the KEY of the last item on the previous page, so paging stays
/// stable when items are added or removed between calls. `None` means the token
/// names no item in this result set — a client error for the caller to answer.
pub fn paginate<T>(
    items: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    key: impl Fn(&T) -> String,
) -> Option<Page<T>> {
    let mut items = items;
    if let Some(token) = next_token {
        let position = items.iter().position(|item| key(item) == token)?;
        items.drain(..=position);
    }
    let limit = match max_results {
        // "If MaxResults is given a value larger than 100, only 100 results are
        // returned."
        Some(max) if max > 0 => {
            usize::try_from(max.min(MAX_RESULT_SET_SIZE)).unwrap_or(usize::MAX)
        }
        // `maxResults` absent, zero or negative asks for no window at all.
        _ => items.len(),
    };
    let next_token = if items.len() > limit {
        items.truncate(limit);
        items.last().map(key)
    } else {
        None
    };
    Some(Page { items, next_token })
}

/// FNV-1a over `bytes`. A stable, dependency-free mixer for deriving an opaque but
/// REPRODUCIBLE device suffix from a resource id; it is not used for anything
/// security-relevant.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// `value` in lowercase base36, left-padded to `len` digits — the alphabet AWS's
/// own device suffixes use.
fn base36(mut value: u64, len: usize) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = vec![b'0'; len];
    for slot in digits.iter_mut().rev() {
        *slot = ALPHABET[usize::try_from(value % 36).unwrap_or(0)];
        value /= 36;
    }
    String::from_utf8(digits).expect("the base36 alphabet is ASCII")
}

/// Render a stored [`Connection`] into one of the generated `Connection`-shaped
/// output structs (`CreateConnectionOutput`, `DeleteConnectionOutput`, the list
/// element `AllocateConnectionOnInterconnectOutput`).
///
/// A macro rather than a function because the generated types are distinct Rust
/// structs for one Smithy shape; this keeps ONE rendering of a stored connection,
/// so every operation answers the same field set.
#[macro_export]
macro_rules! connection_output {
    ($ty:ident, $conn:expr) => {{
        let conn: &$crate::state::Connection = $conn;
        $ty {
            owner_account: Some(conn.owner_account.clone()),
            connection_id: Some(conn.connection_id.clone()),
            connection_name: Some(conn.connection_name.clone()),
            connection_state: Some(conn.connection_state.clone()),
            region: Some(conn.region.clone()),
            location: Some(conn.location.clone()),
            bandwidth: Some(conn.bandwidth.clone()),
            vlan: conn.vlan,
            loa_issue_time: conn.loa_issue_time,
            lag_id: conn.lag_id.clone(),
            aws_device: Some(conn.aws_device.clone()),
            aws_device_v2: Some(conn.aws_device_v2.clone()),
            aws_logical_device_id: Some(conn.aws_logical_device_id.clone()),
            jumbo_frame_capable: Some(conn.jumbo_frame_capable),
            has_logical_redundancy: Some(conn.has_logical_redundancy.clone()),
            tags: (!conn.tags.is_empty()).then(|| conn.tags.clone()),
            provider_name: conn.provider_name.clone(),
            mac_sec_capable: Some(conn.mac_sec_capable),
            // MACsec encryption is reported only for a connection that supports
            // it, and a connection with no MAC Security key associated has its
            // port encryption down — `Connection$portEncryptionStatus` names
            // "Encryption Up, which means that there is an active Connection Key
            // Name, or Encryption Down".
            port_encryption_status: conn
                .mac_sec_capable
                .then(|| "Encryption Down".to_string()),
            encryption_mode: conn.mac_sec_capable.then(|| "no_encrypt".to_string()),
            billing_mode: conn.billing_mode.clone(),
            ..Default::default()
        }
    }};
}

/// Render a stored [`VirtualInterface`] into one of the generated
/// `VirtualInterface`-shaped output structs
/// (`CreatePrivateVirtualInterfaceOutput`, the list element
/// `AllocatePrivateVirtualInterfaceOutput`). See [`connection_output!`] for why
/// this is a macro.
#[macro_export]
macro_rules! virtual_interface_output {
    ($ty:ident, $vif:expr) => {{
        let vif: &$crate::state::VirtualInterface = $vif;
        $ty {
            owner_account: Some(vif.owner_account.clone()),
            virtual_interface_id: Some(vif.virtual_interface_id.clone()),
            location: Some(vif.location.clone()),
            connection_id: Some(vif.connection_id.clone()),
            virtual_interface_type: Some(vif.virtual_interface_type.clone()),
            virtual_interface_name: Some(vif.virtual_interface_name.clone()),
            vlan: vif.vlan,
            asn: vif.asn,
            asn_long: vif.asn_long,
            amazon_side_asn: vif.amazon_side_asn,
            auth_key: vif.auth_key.clone(),
            amazon_address: vif.amazon_address.clone(),
            customer_address: vif.customer_address.clone(),
            address_family: Some(vif.address_family.clone()),
            virtual_interface_state: Some(vif.virtual_interface_state.clone()),
            mtu: Some(vif.mtu),
            jumbo_frame_capable: Some(vif.jumbo_frame_capable),
            virtual_gateway_id: vif.virtual_gateway_id.clone(),
            direct_connect_gateway_id: vif.direct_connect_gateway_id.clone(),
            route_filter_prefixes: (!vif.route_filter_prefixes.is_empty())
                .then(|| vif.route_filter_prefixes.clone()),
            bgp_peers: (!vif.bgp_peers.is_empty()).then(|| vif.bgp_peers.clone()),
            region: Some(vif.region.clone()),
            aws_device_v2: Some(vif.aws_device_v2.clone()),
            aws_logical_device_id: Some(vif.aws_logical_device_id.clone()),
            tags: (!vif.tags.is_empty()).then(|| vif.tags.clone()),
            site_link_enabled: Some(vif.site_link_enabled),
            ..Default::default()
        }
    }};
}
