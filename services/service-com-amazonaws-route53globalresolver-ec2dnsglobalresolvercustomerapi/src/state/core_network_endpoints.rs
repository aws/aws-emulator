// SPDX-License-Identifier: CC0-1.0

//! The core network endpoint of a Route 53 Global Resolver, as this emulator holds it.
//!
//! A core network endpoint attaches one Global Resolver to an Amazon Web Services Cloud WAN
//! core network: it carries the DNS IP addresses the core network's resources send queries
//! to, each with its DNS label and the Region scope the address applies in
//! (`smithy_query(query="shape",shape="CoreNetworkEndpointDnsIpItem")`). The record and the
//! reading of a request's IP entries live here, so the five operations of the resource —
//! create, get, list, update and delete — cannot disagree about either.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::SystemTime;

use crate::ops::types::{
    CoreNetworkEndpointDnsIpInputItem, CoreNetworkEndpointDnsIpItem, ResourceNotFoundException,
    UpdateIpAddressEntry, ValidationException, ValidationExceptionField,
};

use super::CrResourceStatus;

/// The ARN resource prefix of a core network endpoint.
pub const CORE_NETWORK_ENDPOINT_RESOURCE: &str = "core-network-endpoint";

/// How this service's own refusal messages spell the core network endpoint resource
/// kind — the API's spelling, as [`super::GLOBAL_RESOLVER_KIND`] is for a resolver.
pub const CORE_NETWORK_ENDPOINT_KIND: &str = "CoreNetworkEndpoint";

/// The leading characters of every core network endpoint id this service mints, and so
/// of every `ListCoreNetworkEndpoints` page token.
pub const CORE_NETWORK_ENDPOINT_ID_PREFIX: &str = "cnep-";

/// The most core network endpoints one Global Resolver holds at once.
///
/// The emulator's own ceiling, for the reason [`super::MAX_DNS_VIEWS_PER_RESOLVER`] states:
/// a create past it is the modelled `ServiceQuotaExceededException` (HTTP 402) rather than
/// unbounded growth of the store.
pub const MAX_CORE_NETWORK_ENDPOINTS_PER_RESOLVER: usize = 100;

/// The most DNS IP addresses one core network endpoint holds at once, for the same reason
/// and with the same refusal. `CoreNetworkEndpointDnsIpInputItems` declares no `@length`,
/// so the bound is this service's own.
pub const MAX_DNS_IPS_PER_CORE_NETWORK_ENDPOINT: usize = 50;

/// The token a quota refusal about the DNS IP addresses of an endpoint names them by. The
/// addresses of an endpoint are not a resource with an ARN of their own, so this is a
/// message token only.
pub const CORE_NETWORK_ENDPOINT_DNS_IP_RESOURCE: &str = "core-network-endpoint-dns-ip";

/// The member a refusal about a request's IP address entries attributes itself to.
const IP_ADDRESSES_MEMBER: &str = "ipAddresses";

/// The member a refusal about an update's IP address changes attributes itself to.
const UPDATE_IP_ADDRESSES_MEMBER: &str = "updateIpAddresses";

/// `RegionScopeType` — whether a DNS IP address of an endpoint applies in every Region or
/// only in the ones the entry names (`smithy_query(query="shape",shape="RegionScopeType")`:
/// ALL | REGIONAL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionScope {
    /// `ALL` — the address applies in every Region the resolver operates in.
    All,
    /// `REGIONAL` — the address applies only in the Regions the entry lists.
    Regional,
}

impl RegionScope {
    /// The value this scope serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Regional => "REGIONAL",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ALL" => Some(Self::All),
            "REGIONAL" => Some(Self::Regional),
            _ => None,
        }
    }
}

/// One DNS IP address of a core network endpoint, as this emulator holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreNetworkEndpointDnsIp {
    /// `ipv4Address`.
    pub ipv4_address: Option<String>,
    /// `ipv6Address`.
    pub ipv6_address: Option<String>,
    /// `dnsLabel`.
    pub dns_label: String,
    /// `regionScope`.
    pub region_scope: RegionScope,
    /// `regions` — the Regions the address applies in; empty unless the scope is
    /// `REGIONAL`, which is the scope that lists them.
    pub regions: Vec<String>,
}

impl CoreNetworkEndpointDnsIp {
    /// Whether `ipv4` / `ipv6` name this address: an update entry identifies the address
    /// it acts on by the address itself, so either family matching is this address.
    fn names(&self, ipv4: Option<&str>, ipv6: Option<&str>) -> bool {
        let matches = |held: &Option<String>, named: Option<&str>| {
            named.is_some_and(|named| held.as_deref() == Some(named))
        };
        matches(&self.ipv4_address, ipv4) || matches(&self.ipv6_address, ipv6)
    }
}

/// A core network endpoint, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct CoreNetworkEndpoint {
    /// `id`.
    pub id: String,
    /// `arn`.
    pub arn: String,
    /// `coreNetworkArn` — the Cloud WAN core network this endpoint connects.
    pub core_network_arn: String,
    /// `globalResolverId` — the parent resolver.
    pub global_resolver_id: String,
    /// `name`.
    pub name: Option<String>,
    /// `clientToken` — the caller's, or the one this service minted for it. The output
    /// shapes carry no `clientToken`, so this is held for the idempotent replay alone.
    pub client_token: String,
    /// `ipAddresses`, in the order the request named them.
    pub ip_addresses: Vec<CoreNetworkEndpointDnsIp>,
    /// `status`.
    pub status: CrResourceStatus,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `updatedAt`.
    pub updated_at: SystemTime,
    /// When the clock owes this endpoint its terminal status.
    pub operational_at: SystemTime,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
}

impl CoreNetworkEndpoint {
    /// The core network endpoint counterpart of [`super::GlobalResolver::settle`]: an
    /// endpoint being provisioned or changed reports its non-terminal status until the
    /// instant stored on the record falls due.
    pub(super) fn settle(&mut self, now: SystemTime) {
        let pending = matches!(
            self.status,
            CrResourceStatus::Creating | CrResourceStatus::Updating
        );
        if pending && now >= self.operational_at {
            self.status = CrResourceStatus::Operational;
            self.updated_at = self.operational_at;
        }
    }

    /// Whether this record is the one `CreateCoreNetworkEndpoint` would mint for `input`
    /// — the comparison an idempotent replay makes before answering with it, so a
    /// `clientToken` reused with DIFFERENT parameters conflicts instead of quietly
    /// answering with the first call's endpoint.
    pub fn matches_create(
        &self,
        core_network_arn: &str,
        global_resolver_id: &str,
        name: Option<&str>,
        ip_addresses: &[CoreNetworkEndpointDnsIp],
        tags: &BTreeMap<String, String>,
    ) -> bool {
        self.core_network_arn == core_network_arn
            && self.global_resolver_id == global_resolver_id
            && self.name.as_deref() == name
            && self.ip_addresses == ip_addresses
            && &self.tags == tags
    }

    /// `ipAddresses` as the wire carries it.
    ///
    /// `dnsName` is the fully qualified name of the label within the parent resolver's own
    /// `dnsName` ([`super::dns_name`]): AWS publishes no format for it, so it is derived
    /// from the two values that identify it — the label and the resolver — rather than
    /// echoed from the request.
    pub fn ip_address_items(&self) -> Vec<CoreNetworkEndpointDnsIpItem> {
        let suffix = super::dns_name(&self.global_resolver_id);
        self.ip_addresses
            .iter()
            .map(|address| CoreNetworkEndpointDnsIpItem {
                ipv4_address: address.ipv4_address.clone(),
                ipv6_address: address.ipv6_address.clone(),
                dns_label: address.dns_label.clone(),
                dns_name: Some(format!("{}.{suffix}", address.dns_label)),
                region_scope: address.region_scope.as_wire().to_string(),
                regions: (!address.regions.is_empty()).then(|| address.regions.clone()),
            })
            .collect()
    }

    /// `availableRegions` — the Regions this endpoint is available in, computed from the
    /// addresses it holds: an `ALL`-scoped address is available wherever the parent
    /// resolver operates, and a `REGIONAL` one only in the Regions it names. Ordered and
    /// deduplicated, so two reads of one endpoint report one list.
    pub fn available_regions(&self, resolver_regions: &[String]) -> Vec<String> {
        let mut regions: Vec<String> = self
            .ip_addresses
            .iter()
            .flat_map(|address| match address.region_scope {
                RegionScope::All => resolver_regions.to_vec(),
                RegionScope::Regional => address.regions.clone(),
            })
            .collect();
        regions.sort();
        regions.dedup();
        regions
    }

    /// Whether this endpoint satisfies every filter the request named: a key selects the
    /// endpoints whose value for that member is one of the key's values, and a key with no
    /// value selects every endpoint, so several keys narrow the page while several values
    /// of one key widen it.
    pub fn matches_filters(&self, filters: Option<&HashMap<String, Vec<String>>>) -> bool {
        let Some(filters) = filters else {
            return true;
        };
        filters.iter().all(|(key, values)| {
            let held = match key.as_str() {
                "arn" => Some(self.arn.clone()),
                "coreNetworkArn" => Some(self.core_network_arn.clone()),
                "globalResolverId" => Some(self.global_resolver_id.clone()),
                "id" => Some(self.id.clone()),
                "name" => self.name.clone(),
                "status" => Some(self.status.as_wire().to_string()),
                _ => None,
            };
            values.is_empty() || held.is_some_and(|held| values.contains(&held))
        })
    }
}

/// A core network endpoint id for sequence number `sequence`, under the constraint
/// [`super::resolver_id`] states.
pub fn core_network_endpoint_id(sequence: u64) -> String {
    format!("{CORE_NETWORK_ENDPOINT_ID_PREFIX}{sequence:012x}")
}

/// The modelled 404 for a core network endpoint id that names nothing in the store, in the
/// wording every path of this service that resolves an id by its own id answers with
/// ([`super::not_found`]).
pub fn core_network_endpoint_not_found(id: &str) -> ResourceNotFoundException {
    super::not_found(CORE_NETWORK_ENDPOINT_KIND, id)
}

/// The modelled 409 for an `UpdateCoreNetworkEndpoint` entry whose change conflicts with
/// the endpoint's current state — an address or a DNS label the endpoint already holds —
/// naming the entry that asked, the value it named and the endpoint the request would have
/// changed.
pub fn conflicting_dns_ip(
    id: &str,
    index: usize,
    what: &str,
    value: &str,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "{UPDATE_IP_ADDRESSES_MEMBER}[{index}]: this core network endpoint already holds \
             {what} {value}"
        ),
        resource_id: Some(id.to_string()),
        resource_type: CORE_NETWORK_ENDPOINT_KIND.to_string(),
    }
}

/// The members a `ListCoreNetworkEndpoints` filter may name, each the one it is spelled
/// with in the item the list reports
/// (`smithy_query(query="shape",shape="CoreNetworkEndpointsItem")`); the list carries only
/// the members whose value is a single string, because a filter selects by equality.
///
/// `Filters` is an open map of strings to strings in the model, so which keys this service
/// answers is its own to state: a key outside this list names nothing of the listed item,
/// which the list refuses rather than serving as though no filter had been asked for.
pub const CORE_NETWORK_ENDPOINT_FILTER_KEYS: &[&str] = &[
    "arn",
    "coreNetworkArn",
    "globalResolverId",
    "id",
    "name",
    "status",
];

/// The modelled 400 for a filter key outside [`CORE_NETWORK_ENDPOINT_FILTER_KEYS`], or
/// `None` when every key the request named is one of them.
pub fn unknown_filter(
    filters: Option<&HashMap<String, Vec<String>>>,
) -> Option<ValidationException> {
    let named = filters?;
    // Ordered, so a request naming two unknown keys is refused for the same one every time.
    let mut unknown: Vec<&String> = named
        .keys()
        .filter(|key| !CORE_NETWORK_ENDPOINT_FILTER_KEYS.contains(&key.as_str()))
        .collect();
    unknown.sort();
    let key = unknown.first()?;
    Some(super::field_validation(
        "filters",
        format!(
            "{key:?} is not a member a core network endpoint can be filtered by; must be one of {}",
            CORE_NETWORK_ENDPOINT_FILTER_KEYS.join(", ")
        ),
    ))
}

/// The modelled 400 for an entry of a request's IP address list, naming the entry by its
/// position in the list the request carried it in.
fn entry_validation(member: &str, index: usize, message: impl Into<String>) -> ValidationException {
    let name = format!("{member}[{index}]");
    let message = message.into();
    ValidationException {
        message: format!("{name}: {message}"),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![ValidationExceptionField { name, message }]),
    }
}

/// The address an entry names, read off its two address members.
fn named_address<'a>(
    ipv4: Option<&'a str>,
    ipv6: Option<&'a str>,
) -> Option<(Option<&'a str>, Option<&'a str>)> {
    let ipv4 = ipv4.filter(|value| !value.is_empty());
    let ipv6 = ipv6.filter(|value| !value.is_empty());
    (ipv4.is_some() || ipv6.is_some()).then_some((ipv4, ipv6))
}

/// The Regions an entry's scope allows it to name, or the modelled 400.
///
/// `regions` "are listed in the `regions` field" when the scope is `REGIONAL`
/// (`CoreNetworkEndpointDnsIpItem$regionScope`), so a `REGIONAL` entry naming none has no
/// Region to apply in, and an `ALL` one needs none. Each entry must be a Region AWS could
/// name: `Regions` is a plain list of strings, so the grammar is the service's to enforce
/// ([`super::is_region_token`]).
fn scoped_regions(
    member: &str,
    index: usize,
    scope: RegionScope,
    regions: Option<&Vec<String>>,
) -> Result<Vec<String>, ValidationException> {
    let regions: Vec<String> = regions.cloned().unwrap_or_default();
    for region in &regions {
        if !super::is_region_token(region) {
            return Err(entry_validation(
                member,
                index,
                format!("{region:?} is not an Amazon Web Services Region"),
            ));
        }
    }
    match scope {
        RegionScope::Regional if regions.is_empty() => Err(entry_validation(
            member,
            index,
            "must name at least one Region when regionScope is REGIONAL",
        )),
        RegionScope::All if !regions.is_empty() => Err(entry_validation(
            member,
            index,
            "must name no Region when regionScope is ALL",
        )),
        _ => Ok(regions),
    }
}

/// The scope an entry's `regionScope` names, or the modelled 400 naming the catalog.
///
/// `RegionScopeType` is an enum SHAPE, whose membership the boundary validator does not
/// check, so this is the service's to enforce.
fn scope_of(
    member: &str,
    index: usize,
    value: Option<&str>,
) -> Result<RegionScope, ValidationException> {
    let value = value.unwrap_or_default();
    RegionScope::from_wire(value).ok_or_else(|| {
        entry_validation(
            member,
            index,
            format!(
                "must be one of {}",
                crate::ops::types::REGION_SCOPE_TYPE_VALUES.join(", ")
            ),
        )
    })
}

/// Why a `CreateCoreNetworkEndpoint` request's IP address list was refused.
#[derive(Debug)]
pub enum DnsIpListRefusal {
    /// The list is not one this service can store: the modelled 400.
    Invalid(ValidationException),
    /// The list is longer than the number of addresses this emulator keeps for one
    /// endpoint: the modelled 402.
    TooMany,
}

/// The DNS IP addresses a `CreateCoreNetworkEndpoint` request asks for, or why the list
/// was refused.
///
/// `ipAddresses` is `@required` but declares no `@length`, so a list of any size reaches
/// the handler: an endpoint with no address receives no DNS query, and the upper bound is
/// this service's own ([`MAX_DNS_IPS_PER_CORE_NETWORK_ENDPOINT`]). BOTH bounds are checked
/// before a single entry is read, so the per-entry work is bounded by the ceiling whatever
/// the request carries.
///
/// Two entries naming one IP address, or one DNS label, describe no endpoint this service
/// could report, so the list is refused rather than silently deduplicated. The addresses
/// and labels already taken are held in sets, so reading a list costs one pass over it.
pub fn created_dns_ips(
    items: &[CoreNetworkEndpointDnsIpInputItem],
) -> Result<Vec<CoreNetworkEndpointDnsIp>, DnsIpListRefusal> {
    let invalid = DnsIpListRefusal::Invalid;
    if items.is_empty() {
        return Err(invalid(super::field_validation(
            IP_ADDRESSES_MEMBER,
            "must carry at least one IP address",
        )));
    }
    if items.len() > MAX_DNS_IPS_PER_CORE_NETWORK_ENDPOINT {
        return Err(DnsIpListRefusal::TooMany);
    }
    let mut addresses: Vec<CoreNetworkEndpointDnsIp> = Vec::with_capacity(items.len());
    let mut taken_addresses: HashSet<String> = HashSet::with_capacity(items.len());
    let mut taken_labels: HashSet<String> = HashSet::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some((ipv4, ipv6)) =
            named_address(item.ipv4_address.as_deref(), item.ipv6_address.as_deref())
        else {
            return Err(invalid(entry_validation(
                IP_ADDRESSES_MEMBER,
                index,
                "must carry ipv4Address, ipv6Address or both",
            )));
        };
        let scope =
            scope_of(IP_ADDRESSES_MEMBER, index, Some(item.region_scope.as_str())).map_err(invalid)?;
        let regions = scoped_regions(IP_ADDRESSES_MEMBER, index, scope, item.regions.as_ref())
            .map_err(invalid)?;
        let named: Vec<String> = [ipv4, ipv6]
            .into_iter()
            .flatten()
            .map(str::to_string)
            .collect();
        // Which of the two the entry repeats, and the value it repeated, so the refusal is
        // diagnosable from the message alone — the wording the update path's conflict
        // carries too.
        if let Some(repeated) = named
            .iter()
            .find(|address| taken_addresses.contains(*address))
        {
            return Err(invalid(entry_validation(
                IP_ADDRESSES_MEMBER,
                index,
                format!("names the IP address {repeated}, which another entry already names"),
            )));
        }
        if taken_labels.contains(&item.dns_label) {
            return Err(invalid(entry_validation(
                IP_ADDRESSES_MEMBER,
                index,
                format!(
                    "names the DNS label {}, which another entry already names",
                    item.dns_label
                ),
            )));
        }
        taken_addresses.extend(named);
        taken_labels.insert(item.dns_label.clone());
        addresses.push(CoreNetworkEndpointDnsIp {
            ipv4_address: ipv4.map(str::to_string),
            ipv6_address: ipv6.map(str::to_string),
            dns_label: item.dns_label.clone(),
            region_scope: scope,
            regions,
        });
    }
    Ok(addresses)
}

/// What one `UpdateCoreNetworkEndpoint` entry asks this service to do with an address.
///
/// `UpdateIpAddressOperation` is an enum shape, whose membership the boundary validator
/// does not check (`smithy_query(query="shape",shape="UpdateIpAddressOperation")`: ADD |
/// REMOVE | UPDATE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpAddressOperation {
    /// `ADD` — "Adds a new DNS IP address to the core network endpoint."
    Add,
    /// `REMOVE` — "Removes an existing DNS IP address from the core network endpoint."
    Remove,
    /// `UPDATE` — "Updates the configuration of an existing DNS IP address."
    Update,
}

impl IpAddressOperation {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ADD" => Some(Self::Add),
            "REMOVE" => Some(Self::Remove),
            "UPDATE" => Some(Self::Update),
            _ => None,
        }
    }
}

/// Why an `UpdateCoreNetworkEndpoint` request's IP address changes were refused.
#[derive(Debug)]
pub enum IpAddressUpdateRefusal {
    /// The request is not one this service can apply: the modelled 400.
    Invalid(ValidationException),
    /// The change asks for an address or a label the endpoint already holds: the modelled
    /// 409, naming the entry that asked, what it already holds and the value the REQUEST
    /// named it by.
    Conflict {
        /// The entry's position in the list the request carried it in.
        index: usize,
        /// What the endpoint already holds — an IP address, or a DNS label.
        what: &'static str,
        /// The value the request named it by.
        value: String,
    },
    /// The change would leave the endpoint holding more addresses than this emulator keeps
    /// for one endpoint: the modelled 402.
    TooMany,
}

/// The addresses an endpoint holds once `entries` are applied to `held`, or why the
/// request was refused.
///
/// `ADD` appends an address the endpoint does not hold yet, `REMOVE` drops one it does, and
/// `UPDATE` restates one it does — so an entry naming an address the endpoint does not hold
/// is refused rather than silently ignored, and an `ADD` of one it already holds conflicts
/// with the endpoint's current state. The result keeps at least one address, for the reason
/// [`created_dns_ips`] refuses an empty list.
pub fn updated_dns_ips(
    held: &[CoreNetworkEndpointDnsIp],
    entries: &[UpdateIpAddressEntry],
) -> Result<Vec<CoreNetworkEndpointDnsIp>, IpAddressUpdateRefusal> {
    let invalid = IpAddressUpdateRefusal::Invalid;
    // `UpdateIpAddressList` declares no `@length` either, so the number of changes one
    // request carries is bounded before any of them is read: no request may ask for more
    // changes than an endpoint can hold addresses.
    if entries.len() > MAX_DNS_IPS_PER_CORE_NETWORK_ENDPOINT {
        return Err(IpAddressUpdateRefusal::TooMany);
    }
    let mut addresses = held.to_vec();
    for (index, entry) in entries.iter().enumerate() {
        let operation = IpAddressOperation::from_wire(&entry.operation).ok_or_else(|| {
            invalid(entry_validation(
                UPDATE_IP_ADDRESSES_MEMBER,
                index,
                format!(
                    "must be one of {}",
                    crate::ops::types::UPDATE_IP_ADDRESS_OPERATION_VALUES.join(", ")
                ),
            ))
        })?;
        let Some((ipv4, ipv6)) = named_address(
            entry.ipv4_address.as_deref(),
            entry.ipv6_address.as_deref(),
        ) else {
            return Err(invalid(entry_validation(
                UPDATE_IP_ADDRESSES_MEMBER,
                index,
                "must carry ipv4Address, ipv6Address or both",
            )));
        };
        let position = addresses
            .iter()
            .position(|address| address.names(ipv4, ipv6));
        match operation {
            IpAddressOperation::Add => {
                if position.is_some() {
                    return Err(IpAddressUpdateRefusal::Conflict {
                        index,
                        what: "the IP address",
                        value: [ipv4, ipv6].into_iter().flatten().collect::<Vec<&str>>().join(", "),
                    });
                }
                let Some(dns_label) = entry.dns_label.clone().filter(|label| !label.is_empty())
                else {
                    return Err(invalid(entry_validation(
                        UPDATE_IP_ADDRESSES_MEMBER,
                        index,
                        "must carry dnsLabel when the operation is ADD",
                    )));
                };
                let scope = scope_of(
                    UPDATE_IP_ADDRESSES_MEMBER,
                    index,
                    entry.region_scope.as_deref(),
                )
                .map_err(invalid)?;
                let regions = scoped_regions(
                    UPDATE_IP_ADDRESSES_MEMBER,
                    index,
                    scope,
                    entry.regions.as_ref(),
                )
                .map_err(invalid)?;
                if addresses.iter().any(|held| held.dns_label == dns_label) {
                    return Err(IpAddressUpdateRefusal::Conflict {
                        index,
                        what: "the DNS label",
                        value: dns_label,
                    });
                }
                if addresses.len() >= MAX_DNS_IPS_PER_CORE_NETWORK_ENDPOINT {
                    return Err(IpAddressUpdateRefusal::TooMany);
                }
                addresses.push(CoreNetworkEndpointDnsIp {
                    ipv4_address: ipv4.map(str::to_string),
                    ipv6_address: ipv6.map(str::to_string),
                    dns_label,
                    region_scope: scope,
                    regions,
                });
            }
            IpAddressOperation::Remove => {
                let Some(position) = position else {
                    return Err(invalid(entry_validation(
                        UPDATE_IP_ADDRESSES_MEMBER,
                        index,
                        "names an IP address this endpoint does not hold",
                    )));
                };
                addresses.remove(position);
            }
            IpAddressOperation::Update => {
                let Some(position) = position else {
                    return Err(invalid(entry_validation(
                        UPDATE_IP_ADDRESSES_MEMBER,
                        index,
                        "names an IP address this endpoint does not hold",
                    )));
                };
                // An absent member of an `UPDATE` entry leaves that part of the address as
                // the endpoint already holds it: the entry states the change, not the whole
                // address.
                let held = addresses[position].clone();
                let scope = match entry.region_scope.as_deref().filter(|v| !v.is_empty()) {
                    None => held.region_scope,
                    Some(value) => {
                        scope_of(UPDATE_IP_ADDRESSES_MEMBER, index, Some(value)).map_err(invalid)?
                    }
                };
                let regions = match entry.regions.as_ref() {
                    None if scope == held.region_scope => held.regions.clone(),
                    regions => {
                        scoped_regions(UPDATE_IP_ADDRESSES_MEMBER, index, scope, regions)
                            .map_err(invalid)?
                    }
                };
                let dns_label = entry
                    .dns_label
                    .clone()
                    .filter(|label| !label.is_empty())
                    .unwrap_or(held.dns_label);
                if addresses
                    .iter()
                    .enumerate()
                    .any(|(other, address)| other != position && address.dns_label == dns_label)
                {
                    return Err(IpAddressUpdateRefusal::Conflict {
                        index,
                        what: "the DNS label",
                        value: dns_label,
                    });
                }
                let updated = CoreNetworkEndpointDnsIp {
                    ipv4_address: ipv4.map(str::to_string).or(held.ipv4_address),
                    ipv6_address: ipv6.map(str::to_string).or(held.ipv6_address),
                    dns_label,
                    region_scope: scope,
                    regions,
                };
                // No two addresses of one endpoint share an IP address — the state
                // [`created_dns_ips`] refuses to build — so an `UPDATE` that would give the
                // address it restates an address of either family that ANOTHER held address
                // already carries is refused too, rather than stored.
                if addresses.iter().enumerate().any(|(other, address)| {
                    other != position
                        && address.names(
                            updated.ipv4_address.as_deref(),
                            updated.ipv6_address.as_deref(),
                        )
                }) {
                    return Err(IpAddressUpdateRefusal::Conflict {
                        index,
                        what: "the IP address",
                        value: [
                            updated.ipv4_address.as_deref(),
                            updated.ipv6_address.as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<&str>>()
                        .join(", "),
                    });
                }
                addresses[position] = updated;
            }
        }
    }
    if addresses.is_empty() {
        return Err(invalid(super::field_validation(
            UPDATE_IP_ADDRESSES_MEMBER,
            "must leave the endpoint at least one IP address",
        )));
    }
    Ok(addresses)
}
