// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi service.
//!
//! The records here are the ONLY place a Route 53 Global Resolver, a DNS View or
//! a DNS Firewall domain list exists: every handler in `src/ops/` mints into this
//! store or reads out of it, so
//! a create is observable by the next get/list and a delete by the next reader.
//!
//! Two things this slice deliberately does NOT emulate, so a reader does not
//! mistake a modelled error for reachable behaviour: throttling
//! (`ThrottlingException`), and the evaluation of stored IAM documents — the one
//! authorization decision this service takes is the one it can take from a request
//! alone, on the resource ARN a caller supplies (`crate::service`'s resource-ARN
//! gate). The emulator verifies request signatures in the framework, before any
//! handler runs.

/// Which resource of this service a tag-family `resourceArn` addresses, and the
/// refusals that reading settles.
mod tagging;
pub use tagging::*;

/// The core network endpoint resource: the record, and the reading of a request's DNS IP
/// address entries.
mod core_network_endpoints;
pub use core_network_endpoints::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConflictException, CreateDNSViewInput, CreateFirewallDomainListInput, CreateGlobalResolverInput,
    InternalServerException, ResourceNotFoundException, ServiceQuotaExceededException,
    ValidationException, ValidationExceptionField,
};

/// The Region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Route 53 Global Resolver's own documentation
/// (`CreateGlobalResolver`) says "you must specify the US East (Ohio) Region to
/// create, update, or otherwise work with Route 53 Global Resolver resources".
pub const DEFAULT_REGION: &str = "us-east-2";

/// The ARN service namespace for every resource this service mints
/// (`OPERATIONS.md`: "ARN namespace: `route53globalresolver`").
pub const ARN_SERVICE: &str = "route53globalresolver";

/// The ARN resource prefix of a Global Resolver.
pub const GLOBAL_RESOLVER_RESOURCE: &str = "global-resolver";

/// The ARN resource prefix of a DNS View.
pub const DNS_VIEW_RESOURCE: &str = "dns-view";

/// The ARN resource prefix of a DNS Firewall domain list.
pub const FIREWALL_DOMAIN_LIST_RESOURCE: &str = "firewall-domain-list";

/// How this service's own refusal messages spell the Global Resolver resource
/// kind. The ARN prefix of the same resource is a DIFFERENT spelling
/// ([`GLOBAL_RESOLVER_RESOURCE`]) and never reaches a message: a client reads the
/// kind out of a refusal, so the refusal states it the way the API does.
pub const GLOBAL_RESOLVER_KIND: &str = "GlobalResolver";

/// How this service's own refusal messages spell the DNS View resource kind.
pub const DNS_VIEW_KIND: &str = "DnsView";

/// How this service's own refusal messages spell the DNS Firewall domain list
/// resource kind.
pub const FIREWALL_DOMAIN_LIST_KIND: &str = "FirewallDomainList";

/// The error code this service leads the message of a not-found refusal with when
/// an operation resolved a resource id and the store held no such resource.
///
/// Route 53 Global Resolver words its refusal bodies itself: a message opens with
/// a bracketed `[GR-ERRnnnnn]` code, which is the documented way a client tells
/// one refusal class of this service from another. The code belongs to the
/// service, not to the model, so it is stated here once for every path that
/// resolves an id.
const NOT_FOUND_BY_ID_CODE: &str = "GR-ERR00309";

/// The error code the DNS View LIST path leads its own not-found message with.
///
/// Resolving the parent resolver of a page is a distinct refusal from resolving a
/// resource by its id, and this service gives it its own code and its own wording
/// — so a client branching on the code can tell a list whose parent is gone from
/// a read of a resource that never existed.
const LISTED_PARENT_NOT_FOUND_CODE: &str = "GR-ERR02300";

/// The leading characters of every Global Resolver id this service mints, and so
/// of every `ListGlobalResolvers` page token.
pub const RESOLVER_ID_PREFIX: &str = "gr-";

/// The leading characters of every DNS View id this service mints, and so of
/// every `ListDNSViews` page token.
pub const DNS_VIEW_ID_PREFIX: &str = "dnsv-";

/// The leading characters of every firewall domain list id this service mints,
/// and so of every `ListFirewallDomainLists` page token.
pub const FIREWALL_DOMAIN_LIST_ID_PREFIX: &str = "fdl-";

/// How long a freshly minted resource stays in its non-terminal `CREATING` status
/// before the clock owes it the terminal one (`OPERATIONAL`).
///
/// AWS does not document a provisioning time, so this span is chosen rather than
/// cited: it is long enough that a caller acting straight after the create still
/// observes `CREATING`, and short enough that a client script does not wait. The
/// due instant is stored on the record ([`GlobalResolver::operational_at`]) and
/// materialized by whichever call runs after it falls due — nothing ever sleeps.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The number of global anycast addresses a Global Resolver advertises per
/// address family.
pub const ANYCAST_ADDRESS_COUNT: u64 = 2;

/// First address of the synthesized IPv4 anycast span: `198.18.0.0/15`, the
/// RFC 2544 benchmarking range, so an emulated resolver never advertises an
/// address that routes on the public internet.
const ANYCAST_IPV4_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// Addresses in `198.18.0.0/15`.
const ANYCAST_IPV4_SPAN: u64 = 1 << 17;

/// First address of the synthesized IPv6 anycast span: the RFC 3849
/// documentation prefix `2001:db8::/32`, for the same reason as the IPv4 base.
const ANYCAST_IPV6_BASE: u128 = 0x2001_0db8 << 96;

/// Addresses used out of `2001:db8::/32` (its first `/112`).
const ANYCAST_IPV6_SPAN: u64 = 1 << 16;

/// The most Global Resolvers this emulator holds at once.
///
/// A bound on how far the store can grow under repeated creates: a create past it
/// is refused with the modelled `ServiceQuotaExceededException` (HTTP 402) rather
/// than accepted, so an unauthenticated caller cannot grow the process's memory
/// without limit. AWS publishes its own quota and it is not this number; this is
/// the emulator's own ceiling, chosen well above what a client script needs.
/// Deleting a resolver frees its slot (and the idempotency record that named it),
/// so the store's size is bounded by the live resources and nothing else.
pub const MAX_GLOBAL_RESOLVERS: usize = 100;

/// The most DNS Views one Global Resolver holds at once, for the same reason and
/// with the same refusal.
pub const MAX_DNS_VIEWS_PER_RESOLVER: usize = 100;

/// The most firewall domain lists one Global Resolver holds at once, for the same
/// reason and with the same refusal.
pub const MAX_FIREWALL_DOMAIN_LISTS_PER_RESOLVER: usize = 100;

/// The most domain specifications one firewall domain list holds at once, for the
/// same reason and with the same refusal (`ServiceQuotaExceededException`, HTTP
/// 402). This is the emulator's own ceiling, not AWS's published quota, and it is
/// at least the per-request maximum ([`MAX_DOMAINS_PER_UPDATE`]) so a list that is
/// empty can take a whole request.
pub const MAX_DOMAINS_PER_FIREWALL_DOMAIN_LIST: usize = 1000;

/// The most domain specifications ONE `UpdateFirewallDomains` request may carry.
/// Documented by the member itself (`UpdateFirewallDomainsInput$domains`: "You can
/// add up to 1000 domains per request"), so a longer list is the modelled 400.
pub const MAX_DOMAINS_PER_UPDATE: usize = 1000;

/// The token a quota refusal about the domains of a firewall domain list names
/// them by. The domains of a list are not a resource with an ARN of their own, so
/// this is a message token only.
pub const FIREWALL_DOMAIN_RESOURCE: &str = "firewall-domain";

/// The most Regions one `CreateGlobalResolver` request may name, so a single
/// record cannot be grown without limit either. AWS has well under this many
/// Regions, and a list past it is refused with the modelled `ValidationException`.
pub const MAX_REGIONS_PER_RESOLVER: usize = 64;

/// The most items any `@paginated` list in this service returns in one page,
/// including when `maxResults` is absent or larger.
///
/// The model declares no `@range` on `maxResults`, so this is the service's own
/// ceiling: one call can never return the whole store in a single response body,
/// and a caller that wants the rest follows `nextToken`.
pub const MAX_PAGE_SIZE: i32 = 50;

/// `CRResourceStatus`, the lifecycle status of a Global Resolver. The wire values
/// are the model's (`CRResourceStatus`: CREATING | DELETING | OPERATIONAL |
/// UPDATING); only the ones this slice can produce are modelled here, and the
/// others arrive with the operations that move a resolver into them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrResourceStatus {
    /// Being provisioned: the status a create answers with.
    Creating,
    /// Ready to serve queries.
    Operational,
    /// Being changed: the status an update of the resource's contents answers
    /// with, until the clock owes it the terminal one again.
    Updating,
    /// Being removed: the status a delete answers with.
    Deleting,
}

impl CrResourceStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Operational => "OPERATIONAL",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
        }
    }
}

/// `ProfileResourceStatus`, the lifecycle status of a DNS View. As with
/// [`CrResourceStatus`], only the values this slice produces are modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileResourceStatus {
    /// Being provisioned: the status a create answers with.
    Creating,
    /// Ready to serve queries.
    Operational,
}

impl ProfileResourceStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Operational => "OPERATIONAL",
        }
    }
}

/// `GlobalResolverIpAddressType` — which address families a resolver advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalResolverIpAddressType {
    /// IPv4 only; the model documents this as the default.
    Ipv4,
    /// IPv4 and IPv6.
    DualStack,
}

impl GlobalResolverIpAddressType {
    /// The value this address type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPV4",
            Self::DualStack => "DUAL_STACK",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "IPV4" => Some(Self::Ipv4),
            "DUAL_STACK" => Some(Self::DualStack),
            _ => None,
        }
    }
}

/// `PublicIpStatus` — the public IP connectivity of a resolver: whether DNS clients
/// can reach it over the internet, and whether its global anycast public addresses
/// stay reserved for it (`smithy_query(query="shape",shape="PublicIpStatus")`:
/// ENABLED_RESERVED | DISABLED_RESERVED | DISABLED_RELEASED).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicIpStatus {
    /// `ENABLED_RESERVED` — reachable over the internet, addresses reserved.
    EnabledReserved,
    /// `DISABLED_RESERVED` — not reachable over the internet, addresses still reserved.
    DisabledReserved,
    /// `DISABLED_RELEASED` — not reachable over the internet, addresses released.
    DisabledReleased,
}

impl PublicIpStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnabledReserved => "ENABLED_RESERVED",
            Self::DisabledReserved => "DISABLED_RESERVED",
            Self::DisabledReleased => "DISABLED_RELEASED",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ENABLED_RESERVED" => Some(Self::EnabledReserved),
            "DISABLED_RESERVED" => Some(Self::DisabledReserved),
            "DISABLED_RELEASED" => Some(Self::DisabledReleased),
            _ => None,
        }
    }
}

/// The public IP connectivity a resolver carries when the create request names none.
///
/// The model declares no `@default` for the member, so this is the service's own
/// reading of what a resolver created without one has: every create answers with the
/// global anycast IPv4 addresses the resolver advertises, which `ipv4Addresses`
/// documents as addresses "DNS clients can send queries to ... from anywhere on the
/// internet" — public connectivity enabled, with those addresses reserved for it.
pub const DEFAULT_PUBLIC_IP_STATUS: PublicIpStatus = PublicIpStatus::EnabledReserved;

/// A `DISABLED`/`ENABLED` toggle. `DnsSecValidationType`, `EdnsClientSubnetType`
/// and `FirewallRulesFailOpenType` all declare exactly these two values and all
/// default to `DISABLED`, so one type serves the three of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// `DISABLED`, the `@default` every one of the three members declares.
    Disabled,
    /// `ENABLED`.
    Enabled,
}

impl Toggle {
    /// The value this toggle serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::Enabled => "ENABLED",
        }
    }

    /// Resolve a wire value: an ABSENT member (the zero the deserializer leaves
    /// for an enum member it cannot materialize a `@default` for) resolves to the
    /// declared default, a declared value to itself, and anything else to `None`
    /// for the caller to refuse.
    pub fn resolve(value: &str) -> Option<Self> {
        match value {
            "" | "DISABLED" => Some(Self::Disabled),
            "ENABLED" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// A Route 53 Global Resolver, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct GlobalResolver {
    /// `id` — unique, matches `^[-.a-zA-Z0-9]+$`.
    pub id: String,
    /// `arn`.
    pub arn: String,
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: Option<String>,
    /// `clientToken` — the caller's, or the one this service minted for it.
    pub client_token: String,
    /// `dnsName`.
    pub dns_name: String,
    /// `regions`.
    pub regions: Vec<String>,
    /// `observabilityRegion`.
    pub observability_region: Option<String>,
    /// `ipAddressType`.
    pub ip_address_type: GlobalResolverIpAddressType,
    /// `publicIpStatus`.
    pub public_ip_status: PublicIpStatus,
    /// `ipv4Addresses`.
    pub ipv4_addresses: Vec<String>,
    /// `ipv6Addresses` — empty unless `ipAddressType` is `DUAL_STACK`.
    pub ipv6_addresses: Vec<String>,
    /// `status`.
    pub status: CrResourceStatus,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `updatedAt`.
    pub updated_at: SystemTime,
    /// When the clock owes this resolver its terminal status.
    pub operational_at: SystemTime,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// The DNS Views created under this resolver, in creation order.
    pub dns_view_ids: Vec<String>,
}

impl GlobalResolver {
    /// Materialize the status change the clock has already made due. Called by
    /// every handler before it reads or writes, so a reporting operation reports
    /// what a caller would see at this instant without changing anything a later
    /// caller would not already have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == CrResourceStatus::Creating && now >= self.operational_at {
            self.status = CrResourceStatus::Operational;
            self.updated_at = self.operational_at;
        }
    }

    /// `ipv6Addresses` as the wire carries it: present only for a `DUAL_STACK`
    /// resolver, which is the only kind that has any.
    pub fn ipv6_addresses_for_output(&self) -> Option<Vec<String>> {
        if self.ipv6_addresses.is_empty() {
            None
        } else {
            Some(self.ipv6_addresses.clone())
        }
    }

    /// Whether this record is the one `CreateGlobalResolver` would mint for
    /// `input` — the comparison an idempotent replay makes before answering with
    /// it, so a `clientToken` reused with DIFFERENT parameters conflicts instead of
    /// quietly answering with the first call's resolver.
    pub fn matches_create(
        &self,
        input: &CreateGlobalResolverInput,
        ip_address_type: GlobalResolverIpAddressType,
        public_ip_status: PublicIpStatus,
    ) -> bool {
        self.name == input.name
            && self.description == input.description
            && self.regions == input.regions
            && self.observability_region == input.observability_region
            && self.ip_address_type == ip_address_type
            && self.public_ip_status == public_ip_status
            && self.tags == tags_of(input.tags.as_ref())
    }
}

/// A DNS View under a Global Resolver, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct DnsView {
    /// `id`.
    pub id: String,
    /// `arn`.
    pub arn: String,
    /// `globalResolverId` — the parent resolver.
    pub global_resolver_id: String,
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: Option<String>,
    /// `clientToken` — the caller's, or the one this service minted for it.
    pub client_token: String,
    /// `dnssecValidation`.
    pub dnssec_validation: Toggle,
    /// `ednsClientSubnet`.
    pub edns_client_subnet: Toggle,
    /// `firewallRulesFailOpen`.
    pub firewall_rules_fail_open: Toggle,
    /// `status`.
    pub status: ProfileResourceStatus,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `updatedAt`.
    pub updated_at: SystemTime,
    /// When the clock owes this view its terminal status.
    pub operational_at: SystemTime,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
}

impl DnsView {
    /// The DNS View counterpart of [`GlobalResolver::settle`].
    fn settle(&mut self, now: SystemTime) {
        if self.status == ProfileResourceStatus::Creating && now >= self.operational_at {
            self.status = ProfileResourceStatus::Operational;
            self.updated_at = self.operational_at;
        }
    }

    /// The DNS View counterpart of [`GlobalResolver::matches_create`], including
    /// the parent resolver: a token reused under a different resolver is a
    /// mismatch, not a replay.
    pub fn matches_create(
        &self,
        input: &CreateDNSViewInput,
        dnssec_validation: Toggle,
        edns_client_subnet: Toggle,
        firewall_rules_fail_open: Toggle,
    ) -> bool {
        self.global_resolver_id == input.global_resolver_id
            && self.name == input.name
            && self.description == input.description
            && self.dnssec_validation == dnssec_validation
            && self.edns_client_subnet == edns_client_subnet
            && self.firewall_rules_fail_open == firewall_rules_fail_open
            && self.tags == tags_of(input.tags.as_ref())
    }
}

/// A DNS Firewall domain list under a Global Resolver, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct FirewallDomainList {
    /// `id`.
    pub id: String,
    /// `arn`.
    pub arn: String,
    /// `globalResolverId` — the parent resolver.
    pub global_resolver_id: String,
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: Option<String>,
    /// `clientToken` — the caller's, or the one this service minted for it.
    pub client_token: String,
    /// `status`.
    pub status: CrResourceStatus,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `updatedAt`.
    pub updated_at: SystemTime,
    /// When the clock owes this list its terminal status.
    pub operational_at: SystemTime,
    /// The domain specifications this list holds, deduplicated and ordered;
    /// `domainCount` is their number. A list is created empty and
    /// `UpdateFirewallDomains` is what adds to, removes from or replaces the set.
    pub domains: BTreeSet<String>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
}

impl FirewallDomainList {
    /// The firewall domain list counterpart of [`GlobalResolver::settle`]. A list
    /// whose membership was just changed is settled the same way a freshly minted
    /// one is: both report a non-terminal status until the instant stored on the
    /// record falls due.
    fn settle(&mut self, now: SystemTime) {
        let pending = matches!(
            self.status,
            CrResourceStatus::Creating | CrResourceStatus::Updating
        );
        if pending && now >= self.operational_at {
            self.status = CrResourceStatus::Operational;
            self.updated_at = self.operational_at;
        }
    }

    /// `domainCount` — the number of domains this list holds, as the wire carries
    /// it.
    pub fn domain_count(&self) -> i32 {
        i32::try_from(self.domains.len()).unwrap_or(i32::MAX)
    }

    /// The firewall domain list counterpart of [`GlobalResolver::matches_create`],
    /// including the parent resolver: a token reused under a different resolver is
    /// a mismatch, not a replay.
    pub fn matches_create(&self, input: &CreateFirewallDomainListInput) -> bool {
        self.global_resolver_id == input.global_resolver_id
            && self.name == input.name
            && self.description == input.description
            && self.tags == tags_of(input.tags.as_ref())
    }
}

/// What an `UpdateFirewallDomains` request asks this service to do with the
/// domains it carries.
///
/// The model types that member as a plain `String`, so the values live in the
/// member's documentation rather than in an enum shape
/// (`UpdateFirewallDomainsInput$operation`: "The allowed values are ADD, REMOVE,
/// and REPLACE").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallDomainUpdate {
    /// Add the request's domains to the ones the list already holds.
    Add,
    /// Remove the request's domains from the list, ignoring any it does not hold.
    Remove,
    /// Make the request's domains the whole membership of the list.
    Replace,
}

/// The values [`FirewallDomainUpdate`] accepts, in the order the member's
/// documentation lists them — the catalog a refusal of any other value names.
pub const FIREWALL_DOMAIN_UPDATE_VALUES: &[&str] = &["ADD", "REMOVE", "REPLACE"];

impl FirewallDomainUpdate {
    /// Parse a wire value, or `None` when it is not one of
    /// [`FIREWALL_DOMAIN_UPDATE_VALUES`].
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ADD" => Some(Self::Add),
            "REMOVE" => Some(Self::Remove),
            "REPLACE" => Some(Self::Replace),
            _ => None,
        }
    }
}

/// A create request's `tags` as a record holds them: ordered, and empty when the
/// member is absent.
fn tags_of(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// Per-service state for com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState {
    /// Every Global Resolver, keyed by id — a `BTreeMap`, so every list page is
    /// in a stable, resumable order.
    pub global_resolvers: BTreeMap<String, GlobalResolver>,
    /// Every DNS View, keyed by id, for the same reason.
    pub dns_views: BTreeMap<String, DnsView>,
    /// Every firewall domain list, keyed by id, for the same reason.
    pub firewall_domain_lists: BTreeMap<String, FirewallDomainList>,
    /// Every core network endpoint, keyed by id, for the same reason.
    pub core_network_endpoints: BTreeMap<String, CoreNetworkEndpoint>,
    /// `clientToken` -> resolver id, so an `@idempotent` create replayed with the
    /// same token answers with the resolver it already minted.
    pub resolver_idempotency: HashMap<String, String>,
    /// `clientToken` -> DNS View id, for the same reason. Kept separate from
    /// `resolver_idempotency` because the two creates are different operations: one
    /// token reused across both must not make a `CreateDNSView` answer with a
    /// resolver.
    pub dns_view_idempotency: HashMap<String, String>,
    /// `clientToken` -> firewall domain list id, for the same reason and kept
    /// separate for the same one.
    pub firewall_domain_list_idempotency: HashMap<String, String>,
    /// `clientToken` -> core network endpoint id, for the same reason and kept separate
    /// for the same one.
    pub core_network_endpoint_idempotency: HashMap<String, String>,
    /// Source of the per-resource sequence number behind every minted id, client
    /// token and anycast address. Monotonic, so no two resources in one store ever
    /// share an id or a client token. The anycast addresses are drawn from a fixed
    /// span (see [`anycast_ipv4_addresses`]), so they repeat only after the
    /// sequence has passed that span, far above the store's own ceiling.
    pub next_sequence: u64,
}

impl ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState {
    /// Materialize every status change the clock has made due, as of `now`.
    pub fn settle(&mut self, now: SystemTime) {
        for resolver in self.global_resolvers.values_mut() {
            resolver.settle(now);
        }
        for view in self.dns_views.values_mut() {
            view.settle(now);
        }
        for list in self.firewall_domain_lists.values_mut() {
            list.settle(now);
        }
        for endpoint in self.core_network_endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// The next sequence number: unique within this store for the life of the
    /// process.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }
}

/// A resolver id for sequence number `sequence`, matching `ResourceId`'s
/// `^[-.a-zA-Z0-9]+$` within its 1..64 length. Zero-padded, so id order is
/// creation order and a list page resumes deterministically.
pub fn resolver_id(sequence: u64) -> String {
    format!("{RESOLVER_ID_PREFIX}{sequence:012x}")
}

/// A DNS View id for sequence number `sequence`, under the same constraint.
pub fn dns_view_id(sequence: u64) -> String {
    format!("{DNS_VIEW_ID_PREFIX}{sequence:012x}")
}

/// A firewall domain list id for sequence number `sequence`, under the same
/// constraint.
pub fn firewall_domain_list_id(sequence: u64) -> String {
    format!("{FIREWALL_DOMAIN_LIST_ID_PREFIX}{sequence:012x}")
}

/// Whether `value` is a domain specification of the form the `Domains` member
/// declares (`@length` 1..256, `@pattern`
/// `^\*?[a-zA-Z0-9!"#$%&'()*+,./:;<=>?@\[\\\]^_`{|}~-]+$`): an optional leading
/// wildcard followed by at least one visible ASCII character and no whitespace.
///
/// The framework checks that pattern on every entry of a request's `domains`
/// before a handler runs; this is the same rule applied to the page cursor
/// `ListFirewallDomains` issues, which is one of the stored specifications, so a
/// `nextToken` of any other shape was never issued by this service.
pub fn is_domain_specification(value: &str) -> bool {
    let body = value.strip_prefix('*').unwrap_or(value);
    !body.is_empty()
        && value.len() <= 256
        && body.chars().all(|c| c.is_ascii_graphic())
}

/// Whether `token` is a page cursor this service could have issued for ids
/// carrying `prefix` — the form [`resolver_id`] and [`dns_view_id`] mint.
///
/// A cursor is one of this service's own ids, so a token of any other shape was
/// never issued by this service and the list refuses it rather than silently
/// serving the whole collection from the start.
pub fn is_page_token(prefix: &str, token: &str) -> bool {
    token
        .strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether `value` is an AWS Region token.
///
/// The grammar is the framework's own published one,
/// `[a-z]{2}(-gov|-iso[a-z]?)?-[a-z]+-\d` (`REGION_PATTERN_IMPLEMENTED` in
/// `vendor/emulator-telemetry/src/backend_contract.rs`, minus its `unknown` fold
/// token), checked segment by segment because this crate pulls in no regex engine.
/// A `regions` entry outside it is a Region AWS could not name, which the real
/// service refuses.
pub fn is_region_token(value: &str) -> bool {
    fn two_letters(segment: &str) -> bool {
        segment.len() == 2 && segment.chars().all(|c| c.is_ascii_lowercase())
    }
    fn letters(segment: &str) -> bool {
        !segment.is_empty() && segment.chars().all(|c| c.is_ascii_lowercase())
    }
    fn one_digit(segment: &str) -> bool {
        segment.len() == 1 && segment.chars().all(|c| c.is_ascii_digit())
    }
    /// `-gov` or `-iso` with an optional single letter (`-isob`, `-isoe`).
    fn partition(segment: &str) -> bool {
        segment == "gov"
            || segment.strip_prefix("iso").is_some_and(|rest| {
                rest.is_empty() || (rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase()))
            })
    }
    let segments: Vec<&str> = value.split('-').collect();
    match segments.as_slice() {
        [prefix, area, number] => two_letters(prefix) && letters(area) && one_digit(number),
        [prefix, part, area, number] => {
            two_letters(prefix) && partition(part) && letters(area) && one_digit(number)
        }
        _ => false,
    }
}

/// The `clientToken` this service mints when the caller omits the
/// `@idempotencyToken` member, which the model still requires in the output.
/// Derived from the sequence number, so two creates never share one.
pub fn minted_client_token(sequence: u64) -> String {
    format!("server-generated-{sequence:012x}")
}

/// The anycast IPv4 addresses of the resolver with this sequence number.
pub fn anycast_ipv4_addresses(sequence: u64) -> Vec<String> {
    (0..ANYCAST_ADDRESS_COUNT)
        .map(|slot| {
            let offset = (sequence * ANYCAST_ADDRESS_COUNT + slot) % ANYCAST_IPV4_SPAN;
            // `offset` is reduced modulo the span, which is far below u32::MAX.
            Ipv4Addr::from(ANYCAST_IPV4_BASE + offset as u32).to_string()
        })
        .collect()
}

/// The anycast IPv6 addresses of the resolver with this sequence number;
/// only a `DUAL_STACK` resolver has any.
pub fn anycast_ipv6_addresses(sequence: u64) -> Vec<String> {
    (0..ANYCAST_ADDRESS_COUNT)
        .map(|slot| {
            let offset = (sequence * ANYCAST_ADDRESS_COUNT + slot) % ANYCAST_IPV6_SPAN;
            Ipv6Addr::from(ANYCAST_IPV6_BASE + u128::from(offset)).to_string()
        })
        .collect()
}

/// The hostname DNS clients use for this resolver. AWS publishes no format for
/// `dnsName`, so it is synthesized from the id — the one value that is unique
/// per resolver — and stays within `Sni`'s 1..128 length.
pub fn dns_name(id: &str) -> String {
    format!("{id}.resolver.route53globalresolver.aws")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState>)
}

/// The Region an ARN this request mints belongs in: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope region that is not an ARN region segment (`[-.a-z0-9]{0,63}`, from
/// `ResourceArn`'s own pattern) is not used, so a crafted `Authorization` header
/// can never produce a stored `arn` that violates the output member's pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of one of this service's resources, through the one ARN builder the
/// framework vends (so the account segment is the canonical emulator account).
pub fn resource_arn(region: &str, resource_type: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{resource_type}/{id}"))
}

/// The modelled 500 for a request that could not reach this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the Route 53 Global Resolver state store is unavailable".to_string(),
        retry_after_seconds: None,
    }
}

/// The modelled 404 for a resource id that names nothing in the store: this
/// service's own [`NOT_FOUND_BY_ID_CODE`], the kind the id was looked up as
/// (`kind`, one of [`GLOBAL_RESOLVER_KIND`] and its siblings) and the id the
/// REQUEST named, verbatim.
///
/// The shape also models the two members that state the same two values on their
/// own, `resourceType` (`@required`) and `resourceId`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so a client
/// branching on them does not have to read the sentence: both carry what the
/// message states.
pub fn not_found(kind: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("[{NOT_FOUND_BY_ID_CODE}] No {kind} found with the ID: {id}"),
        resource_id: Some(id.to_string()),
        resource_type: kind.to_string(),
    }
}

/// The modelled 404 for a Global Resolver id that names nothing in the store.
///
/// Every handler that resolves a `globalResolverId` to a resolver answers with
/// it, so one error class has one wording whichever operation reports it. The
/// DNS View list path is the exception the service itself makes: see
/// [`listed_global_resolver_not_found`].
pub fn global_resolver_not_found(id: &str) -> ResourceNotFoundException {
    not_found(GLOBAL_RESOLVER_KIND, id)
}

/// The modelled 404 the DNS View LIST path answers for a parent resolver the
/// store does not hold: its own [`LISTED_PARENT_NOT_FOUND_CODE`] and its own
/// wording, which states the id without a colon before it.
pub fn listed_global_resolver_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "[{LISTED_PARENT_NOT_FOUND_CODE}] No {GLOBAL_RESOLVER_KIND} found with ID {id}"
        ),
        resource_id: Some(id.to_string()),
        resource_type: GLOBAL_RESOLVER_KIND.to_string(),
    }
}

/// The modelled 409 for an `@idempotencyToken` replayed with parameters that do
/// not match the resource it already minted.
pub fn idempotency_conflict(resource_type: &str, resource_id: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "the clientToken of this request already created {resource_type} {resource_id}, \
             with different parameters"
        ),
        resource_id: Some(resource_id.to_string()),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled 402 for a create this store has no room for, naming the ceiling
/// it would pass.
pub fn quota_exceeded(
    resource_type: &str,
    resource_id: Option<&str>,
    limit: usize,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!("this emulator holds at most {limit} {resource_type} resources"),
        resource_id: resource_id.map(str::to_string),
        resource_type: resource_type.to_string(),
        service_code: Some(ARN_SERVICE.to_string()),
        quota_code: None,
    }
}

/// The modelled 400 for a member this service refuses, naming the member and the
/// rule it broke — the text is built from the request, so a caller can fix the
/// request from it alone.
pub fn field_validation(member: &str, message: impl Into<String>) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: format!("{member}: {message}"),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// One page of a `@paginated` list over id-ordered `(id, item)` pairs: at most
/// `max_results` items (never more than [`MAX_PAGE_SIZE`]) starting at
/// `next_token`, and the token that resumes after them.
///
/// An absent `max_results` asks for everything, which this service answers with
/// at most [`MAX_PAGE_SIZE`] items plus a resume token: no single call returns the
/// whole store. A `next_token` resumes at the first id at or after it, so a page
/// survives the deletion of the resource the previous page stopped at. The
/// returned token is `None` on the last page, and it is an id — the only cursor
/// this store has that stays valid across concurrent creates and deletes.
pub fn page<'a, T, I>(
    items: I,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<&'a T>, Option<String>)
where
    I: Iterator<Item = (&'a String, &'a T)>,
{
    let token = next_token.unwrap_or_default().to_string();
    let mut remaining = items.skip_while(move |(id, _)| id.as_str() < token.as_str());
    // A non-positive `maxResults` is refused by the handler before it gets here;
    // one above the ceiling is served at the ceiling.
    let limit = max_results.map_or(MAX_PAGE_SIZE, |value| value.clamp(1, MAX_PAGE_SIZE)) as usize;
    let page: Vec<&T> = remaining
        .by_ref()
        .take(limit)
        .map(|(_, item)| item)
        .collect();
    let next = remaining.next().map(|(id, _)| id.clone());
    (page, next)
}
