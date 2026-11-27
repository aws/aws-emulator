// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-globalaccelerator-globalaccelerator-v20180706 service.
//!
//! The accelerator control plane this emulator serves: accelerators, their
//! listeners, those listeners' endpoint groups, their attributes, and the
//! bookkeeping the create paths need (idempotency tokens and the static-IP
//! pool). Every handler computes its response from these records; nothing is
//! canned.
//!
//! Shared helpers live here rather than in a new top-level module because
//! `src/lib.rs` is generated and declares the module list.

/// WHICH host an accelerator ANNOUNCES as the one a client reaches it at, which is a
/// property of the address this emulator is served at rather than of a stored record.
mod announced_dns;

pub use announced_dns::announced_dns_host;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hasher;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Accelerator, AcceleratorAttributes, CustomRoutingAccelerator,
    CustomRoutingAcceleratorAttributes, CustomRoutingDestinationConfiguration,
    CustomRoutingDestinationDescription,
    CustomRoutingEndpointDescription, CustomRoutingEndpointGroup, CustomRoutingListener,
    EndpointConfiguration, EndpointDescription, EndpointGroup, IpAddressDetail, IpSet, Listener,
    PortOverride, PortRange, Tag,
};

/// `AcceleratorStatus` values, from the model enum of the same name
/// (`ACCELERATOR_STATUS_VALUES` in `ops::types`).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_IN_PROGRESS`].
pub const STATUS_DEPLOYED: &str = "DEPLOYED";

/// `IpAddressType` values, from the model enum of the same name
/// (`IP_ADDRESS_TYPE_VALUES` in `ops::types`).
pub const IP_ADDRESS_TYPE_IPV4: &str = "IPV4";
/// See [`IP_ADDRESS_TYPE_IPV4`].
pub const IP_ADDRESS_TYPE_DUAL_STACK: &str = "DUAL_STACK";

/// `IpAddressFamily` values, from the model enum of the same name
/// (`IP_ADDRESS_FAMILY_VALUES` in `ops::types`).
pub const IP_FAMILY_IPV4: &str = "IPv4";
/// See [`IP_FAMILY_IPV4`].
pub const IP_FAMILY_IPV6: &str = "IPv6";

/// `ClientAffinity`'s documented default: "The default value is `NONE`"
/// (`Listener$ClientAffinity` doc).
pub const CLIENT_AFFINITY_NONE: &str = "NONE";

/// `HealthCheckProtocol`'s documented default: "The default value is TCP"
/// (`EndpointGroup$HealthCheckProtocol` doc). One of `HEALTH_CHECK_PROTOCOL_VALUES`
/// in `ops::types`.
pub const HEALTH_CHECK_PROTOCOL_TCP: &str = "TCP";
/// See [`HEALTH_CHECK_PROTOCOL_TCP`]: the two protocols that health-check a path.
pub const HEALTH_CHECK_PROTOCOL_HTTP: &str = "HTTP";
/// See [`HEALTH_CHECK_PROTOCOL_HTTP`].
pub const HEALTH_CHECK_PROTOCOL_HTTPS: &str = "HTTPS";

/// The `HealthState` of an endpoint this emulator has not health-checked, from the
/// model enum of the same name (`HEALTH_STATE_VALUES` in `ops::types`).
pub const HEALTH_STATE_INITIAL: &str = "INITIAL";

/// `TrafficDialPercentage`'s documented default: "The default value is 100"
/// (`CreateEndpointGroup$TrafficDialPercentage` doc).
pub const DEFAULT_TRAFFIC_DIAL_PERCENTAGE: f32 = 100.0;

/// `ThresholdCount`'s documented default: "The default value is 3"
/// (`CreateEndpointGroup$ThresholdCount` doc).
pub const DEFAULT_THRESHOLD_COUNT: i32 = 3;

/// `HealthCheckIntervalSeconds`'s documented default: "The default value is 30"
/// (`CreateEndpointGroup$HealthCheckIntervalSeconds` doc).
pub const DEFAULT_HEALTH_CHECK_INTERVAL_SECONDS: i32 = 30;

/// The only two health-check intervals the service accepts: "The time—10 seconds or
/// 30 seconds—between each health check for an endpoint"
/// (`CreateEndpointGroup$HealthCheckIntervalSeconds` doc). The model's `@range`
/// (10..30) admits every value in between, so the pair is enforced in the handler.
pub const SUPPORTED_HEALTH_CHECK_INTERVALS: [i32; 2] = [10, 30];

/// `HealthCheckPath`'s documented default: "The default value is slash (/)"
/// (`CreateEndpointGroup$HealthCheckPath` doc).
pub const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The weight an endpoint carries when the caller names none: "By default, all
/// endpoints have a weight of 128" (Endpoint weights, Global Accelerator Developer
/// Guide — the page `EndpointConfiguration$Weight` links to).
pub const DEFAULT_ENDPOINT_WEIGHT: i32 = 128;

/// The documented maximum length of an accelerator name: "The name can have a
/// maximum of 64 characters" (`CreateAccelerator$Name` doc).
pub const ACCELERATOR_NAME_MAX_LEN: usize = 64;

/// The shortest accelerator name the service accepts: a name is what identifies the
/// accelerator to its owner, so an accelerator named by no character at all names
/// nothing. The lower bound of the length limit this service states a name outside with
/// ([`accelerator_name_length_message`]).
pub const ACCELERATOR_NAME_MIN_LEN: usize = 1;

/// Accelerators per Amazon Web Services account: the service's documented default
/// quota ("Accelerators per account: 20", AWS Global Accelerator service quotas).
/// A create past it is refused with the modelled `LimitExceededException` — which is
/// also what bounds this store's resident size, so a caller cannot grow it without
/// limit.
///
/// Applied PER accelerator kind, because a standard accelerator and a custom routing
/// one are distinct resources created by distinct operations: neither create's quota is
/// consumed by the other kind's resources.
pub const MAX_ACCELERATORS_PER_ACCOUNT: usize = 20;

/// The kinds of accelerator this service holds — standard and custom routing — each
/// bounded by [`MAX_ACCELERATORS_PER_ACCOUNT`]. Only [`MAX_IDEMPOTENCY_RECORDS`] needs
/// the count, to bound the tokens BOTH kinds' creates can leave behind.
pub const ACCELERATOR_KINDS: usize = 2;

/// Listeners per accelerator: the service's documented default quota ("Listeners per
/// accelerator: 10", AWS Global Accelerator service quotas). Refused the same way.
pub const MAX_LISTENERS_PER_ACCELERATOR: usize = 10;

/// Endpoint groups one accelerator can hold, across every one of its listeners: the
/// service's documented default quota ("Endpoint groups per accelerator: 10", AWS
/// Global Accelerator service quotas). A create past it is refused with the modelled
/// `LimitExceededException`, which is also what bounds this store: `EndpointGroupRegion`
/// is a free string, so without this cap a caller could add endpoint groups without
/// limit.
pub const MAX_ENDPOINT_GROUPS_PER_ACCELERATOR: usize = 10;

/// Endpoints one endpoint group can hold: the model's own `@length` on
/// `EndpointConfigurations` (0..10), which is also the service's documented quota
/// ("Endpoints per endpoint group: 10", AWS Global Accelerator service quotas). The
/// boundary validator refuses a longer list before a handler runs; it is re-checked in
/// [`endpoints_from_configurations`] so neither the work that helper does nor the vector
/// the store holds is proportional to a length the caller chose.
pub const MAX_ENDPOINTS_PER_ENDPOINT_GROUP: usize = 10;

/// Port overrides one endpoint group can hold: the model's own `@length` on
/// `PortOverrides` (0..10). Re-checked in [`port_overrides_to_pairs`] for the reason
/// [`MAX_ENDPOINTS_PER_ENDPOINT_GROUP`] gives.
pub const MAX_PORT_OVERRIDES_PER_ENDPOINT_GROUP: usize = 10;

/// Port ranges one listener can hold: the model's own `@length` maximum on `PortRanges`
/// (1..10), which every operation that takes such a list declares.
///
/// The service enforces it as a LIMIT rather than as a malformed argument — the error
/// class and the sentence are [`port_range_limit_for_accelerator_message`] and
/// [`port_range_limit_for_listener_message`], applied by
/// [`crate::service::listener_port_range_limit`] — which is also what bounds the vector of
/// ranges the store holds for a listener.
pub const MAX_PORT_RANGES_PER_LISTENER: usize = 10;

/// How many hexadecimal characters follow the `i-` of an EC2 instance ID: the eight of
/// the original form, or the seventeen of the long form every Region mints today (Amazon
/// EC2 resource IDs). The two lengths an `EndpointId` is matched against in
/// [`endpoint_id_names_ec2_instance`].
pub const EC2_INSTANCE_ID_HEX_LENGTHS: [usize; 2] = [8, 17];

/// Destination configurations one CUSTOM ROUTING endpoint group can hold: the model's own
/// `@length` on `CustomRoutingDestinationConfigurations` (1..100). Re-checked in
/// [`destinations_from_configurations`] for the reason
/// [`MAX_ENDPOINTS_PER_ENDPOINT_GROUP`] gives.
pub const MAX_DESTINATIONS_PER_ENDPOINT_GROUP: usize = 100;

/// The most idempotency records the store can hold: one per resource that can exist.
/// [`ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State::prune_idempotency`]
/// keeps the invariant that every record points at a LIVE resource, so the map is
/// bounded by the quotas above — the accelerators of both kinds the account may hold,
/// and the listeners and endpoint groups each of them may hold — rather than by how
/// many tokens a caller has ever sent.
pub const MAX_IDEMPOTENCY_RECORDS: usize = ACCELERATOR_KINDS
    * MAX_ACCELERATORS_PER_ACCOUNT
    * (1 + MAX_LISTENERS_PER_ACCELERATOR + MAX_ENDPOINT_GROUPS_PER_ACCELERATOR);

/// The largest page any list returns, and the page it returns when the caller names
/// no `MaxResults`: the model's own `MaxResults` upper bound (range 1..100). A request
/// without the member therefore cannot amplify into a response proportional to the
/// whole store.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// How long after the call that starts a deployment an accelerator stops being
/// reported `IN_PROGRESS` and becomes `DEPLOYED`.
///
/// CHOSEN, not documented: the real service publishes no settling time. It is
/// deliberately non-zero so a caller acting straight after `CreateAccelerator`
/// or `UpdateAccelerator` still observes the transient status, and it is stored
/// as a due instant on the record rather than slept for.
pub const DEPLOYMENT_SETTLE: Duration = Duration::from_millis(500);

/// Base of the static-IPv4 pool: `198.18.0.0`, the first address of
/// `198.18.0.0/15`, the RFC 2544 benchmarking range. Non-routable, so an
/// emulated accelerator never hands a caller an address that belongs to
/// somebody.
const IPV4_POOL_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);
/// `198.18.0.0/15` holds 2^17 addresses; allocation wraps inside it.
const IPV4_POOL_LEN: u32 = 1 << 17;

/// Base of the static-IPv6 pool: `2001:db8::`, the RFC 3849 documentation
/// prefix. Non-routable, for the same reason as [`IPV4_POOL_BASE`].
const IPV6_POOL_BASE: u128 = 0x2001_0db8_u128 << 96;

/// One static address of an accelerator, as persisted: the address and the network
/// zone it sits in.
///
/// The two are stored TOGETHER rather than as parallel lists so a write path that
/// replaces an address cannot leave a zone behind that belongs to another address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredStaticAddress {
    /// `IpAddressDetail$IpAddress` — also the entry `IpSet$IpAddresses` carries.
    pub address: String,
    /// `IpAddressDetail$NetworkZone`: "The network zone that the specified IP address
    /// is located on." See [`network_zone`] for where the label comes from.
    pub network_zone: String,
}

/// The network zone the `index`-th address of an IP address set sits in.
///
/// "A network zone services the static IP addresses for your accelerator from a unique
/// IP subnet. ... Global Accelerator assigns the static IP addresses for an accelerator
/// from two network zones" (Global Accelerator Developer Guide, Network zones), so each
/// of a set's (at most two, per `IpSet$IpAddresses`) addresses sits in its OWN zone: the
/// zones of one set are distinct, and there is exactly one per allocated address.
///
/// The LABEL is environment-specific — AWS's own differ per partition and Region — so
/// this emulator numbers its zones by the address's position in the set, which keeps the
/// label short (`NetworkZone` is `@length(0..24)`), stable for the life of the record and
/// distinct within a set.
pub fn network_zone(index: usize) -> String {
    format!("nz{}", index + 1)
}

/// One of an accelerator's IP address sets, as persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIpSet {
    /// An `IpAddressFamily` value: [`IP_FAMILY_IPV4`] or [`IP_FAMILY_IPV6`].
    pub ip_address_family: String,
    /// The addresses in this set, each with its network zone; an IP address set holds
    /// at most two (`IpSet$IpAddresses`, `@length(0..2)`).
    pub addresses: Vec<StoredStaticAddress>,
}

impl StoredIpSet {
    /// A set of `addresses` of one family, each placed in its own network zone by
    /// [`network_zone`].
    pub fn new(ip_address_family: &str, addresses: Vec<String>) -> Self {
        Self {
            ip_address_family: ip_address_family.to_string(),
            addresses: addresses
                .into_iter()
                .enumerate()
                .map(|(index, address)| StoredStaticAddress {
                    address,
                    network_zone: network_zone(index),
                })
                .collect(),
        }
    }

    /// Just the addresses of this set, in order — the `IpSet$IpAddresses` projection.
    pub fn ip_addresses(&self) -> Vec<String> {
        self.addresses
            .iter()
            .map(|entry| entry.address.clone())
            .collect()
    }

    /// This set as the modelled `IpSet`.
    ///
    /// `IpFamily` carries the same value as `IpAddressFamily`: the model marks it
    /// `@deprecated` with "IpFamily has been replaced by IpAddressFamily", i.e.
    /// it is an alias, not a second datum. `IpAddressDetails` is "The array of IP
    /// addresses in the IP address set, with detailed information about the IP
    /// addresses", so it carries one entry per address of this set, in the same order
    /// as `IpAddresses`, each naming that address and the zone it was allocated in.
    fn to_wire(&self) -> IpSet {
        IpSet {
            ip_family: Some(self.ip_address_family.clone()),
            ip_addresses: Some(self.ip_addresses()),
            ip_address_family: Some(self.ip_address_family.clone()),
            ip_address_details: Some(
                self.addresses
                    .iter()
                    .map(|entry| IpAddressDetail {
                        ip_address: Some(entry.address.clone()),
                        network_zone: Some(entry.network_zone.clone()),
                    })
                    .collect(),
            ),
        }
    }
}

/// An accelerator, as persisted.
///
/// The same record serves both kinds of accelerator the service has — a standard one
/// and a custom routing one — because they carry the same attributes; which kind a
/// record is, is the map it lives in
/// ([`GlobalAcceleratorState::accelerators`] or
/// [`GlobalAcceleratorState::custom_routing_accelerators`]), since each kind is served
/// by its OWN operations: a custom routing accelerator is not described, listed,
/// updated or deleted through the standard ones, and vice versa.
#[derive(Debug, Clone)]
pub struct StoredAccelerator {
    /// `AcceleratorArn` — also this record's key in [`GlobalAcceleratorState::accelerators`].
    pub arn: String,
    pub name: String,
    /// An `IpAddressType` value: [`IP_ADDRESS_TYPE_IPV4`] or [`IP_ADDRESS_TYPE_DUAL_STACK`].
    pub ip_address_type: String,
    pub ip_sets: Vec<StoredIpSet>,
    /// Whether this accelerator announces a `DualStackDnsName` as well as a `DnsName` —
    /// true only for a dual-stack one. WHICH host both announce is the address this
    /// emulator is reached at, read per response rather than stored (see
    /// [`announced_dns_host`]).
    pub dual_stack_dns: bool,
    pub enabled: bool,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The instant at which the deployment started by the most recent change
    /// becomes visible as [`STATUS_DEPLOYED`]. Stored, never slept for.
    pub deployed_at: SystemTime,
    /// `Tags`, as key/value pairs in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// The ARNs of this accelerator's listeners, in creation order.
    pub listener_arns: Vec<String>,
}

impl StoredAccelerator {
    /// The deployment status at `now`: `IN_PROGRESS` until the stored due
    /// instant passes, `DEPLOYED` from then on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.deployed_at {
            STATUS_IN_PROGRESS
        } else {
            STATUS_DEPLOYED
        }
    }

    /// This record as the modelled `CustomRoutingAccelerator`.
    ///
    /// The same record, projected through the shape the custom-routing operations
    /// answer with: it carries no `DualStackDnsName` and no `Events`, and its
    /// `Status` is a `CustomRoutingAcceleratorStatus`, whose values are the same
    /// `DEPLOYED`/`IN_PROGRESS` pair [`Self::status`] reports.
    ///
    /// `announced_host` is the host a client reaches this accelerator at
    /// ([`announced_dns_host`]).
    pub fn to_custom_routing_wire(
        &self,
        now: SystemTime,
        announced_host: &str,
    ) -> CustomRoutingAccelerator {
        CustomRoutingAccelerator {
            accelerator_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            enabled: Some(self.enabled),
            ip_sets: Some(self.ip_sets.iter().map(StoredIpSet::to_wire).collect()),
            dns_name: Some(announced_host.to_string()),
            status: Some(self.status(now).to_string()),
            created_time: Some(to_date_time(self.created_time)),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
        }
    }

    /// This record as the modelled `Accelerator`.
    ///
    /// `Events` is left unset: it is the accelerator's change history, which this
    /// slice does not record.
    ///
    /// Both DNS members carry `announced_host`, the host a client reaches this accelerator
    /// at ([`announced_dns_host`]): one process answers for both address families, so a
    /// dual-stack accelerator is reached where every other one is.
    pub fn to_wire(&self, now: SystemTime, announced_host: &str) -> Accelerator {
        Accelerator {
            accelerator_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            enabled: Some(self.enabled),
            ip_sets: Some(self.ip_sets.iter().map(StoredIpSet::to_wire).collect()),
            dns_name: Some(announced_host.to_string()),
            status: Some(self.status(now).to_string()),
            created_time: Some(to_date_time(self.created_time)),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            dual_stack_dns_name: self.dual_stack_dns.then(|| announced_host.to_string()),
            events: None,
        }
    }
}

/// A listener, as persisted.
#[derive(Debug, Clone)]
pub struct StoredListener {
    /// `ListenerArn` — also this record's key in [`GlobalAcceleratorState::listeners`].
    pub arn: String,
    /// The ARN of the accelerator this listener belongs to.
    pub accelerator_arn: String,
    /// `PortRanges`, as inclusive `(FromPort, ToPort)` pairs.
    pub port_ranges: Vec<(i32, i32)>,
    /// A `Protocol` value: `TCP` or `UDP`.
    pub protocol: String,
    /// A `ClientAffinity` value: `NONE` or `SOURCE_IP`.
    pub client_affinity: String,
    /// The ARNs of this listener's endpoint groups, in creation order.
    pub endpoint_group_arns: Vec<String>,
}

impl StoredListener {
    /// The port a health check goes to when the caller names no `HealthCheckPort`:
    /// "The default port is the port for the listener that this endpoint group is
    /// associated with. If the listener port is a list, Global Accelerator uses the
    /// first specified port in the list of ports." (`EndpointGroup$HealthCheckPort`.)
    pub fn default_health_check_port(&self) -> Option<i32> {
        self.port_ranges.first().map(|&(from, _)| from)
    }

    /// This record as the modelled `Listener`.
    pub fn to_wire(&self) -> Listener {
        Listener {
            listener_arn: Some(self.arn.clone()),
            port_ranges: Some(
                self.port_ranges
                    .iter()
                    .map(|&(from, to)| PortRange {
                        from_port: Some(from),
                        to_port: Some(to),
                    })
                    .collect(),
            ),
            protocol: Some(self.protocol.clone()),
            client_affinity: Some(self.client_affinity.clone()),
        }
    }
}

/// A custom routing accelerator's listener, as persisted.
///
/// Held apart from [`StoredListener`] for the reason [`StoredAccelerator`] gives about
/// the two accelerator kinds: a `CustomRoutingListener` carries only an ARN and port
/// ranges — it has no `Protocol` and no `ClientAffinity`, because the protocol and the
/// routing of a custom routing listener are properties of its endpoints — and it is
/// reached only through the custom routing operations.
#[derive(Debug, Clone)]
pub struct StoredCustomRoutingListener {
    /// `ListenerArn` — also this record's key in
    /// [`GlobalAcceleratorState::custom_routing_listeners`].
    pub arn: String,
    /// The ARN of the custom routing accelerator this listener belongs to.
    pub accelerator_arn: String,
    /// `PortRanges`, as inclusive `(FromPort, ToPort)` pairs.
    pub port_ranges: Vec<(i32, i32)>,
    /// The ARNs of this listener's custom routing endpoint groups, in creation order.
    pub endpoint_group_arns: Vec<String>,
}

impl StoredCustomRoutingListener {
    /// This record as the modelled `CustomRoutingListener`.
    pub fn to_wire(&self) -> CustomRoutingListener {
        CustomRoutingListener {
            listener_arn: Some(self.arn.clone()),
            port_ranges: Some(
                self.port_ranges
                    .iter()
                    .map(|&(from, to)| PortRange {
                        from_port: Some(from),
                        to_port: Some(to),
                    })
                    .collect(),
            ),
        }
    }
}

/// One destination configuration of a custom routing endpoint group, as persisted.
///
/// "Sets the port range and protocol for all endpoints (virtual private cloud subnets)
/// in a custom routing endpoint group to accept client traffic on"
/// (`CreateCustomRoutingEndpointGroup$DestinationConfigurations`), so the two ends of the
/// range and the protocols are held TOGETHER: a write that replaces a range cannot leave
/// behind the protocols of another one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCustomRoutingDestination {
    /// `FromPort`, the first port of the range, inclusive.
    pub from_port: i32,
    /// `ToPort`, the last port of the range, inclusive.
    pub to_port: i32,
    /// `Protocols`, in the order the configuration named them. Each is one of
    /// `CUSTOM_ROUTING_PROTOCOL_VALUES` in `ops::types`; the boundary validator refuses
    /// any other value before a handler runs.
    pub protocols: Vec<String>,
}

impl StoredCustomRoutingDestination {
    /// This record as the modelled `CustomRoutingDestinationDescription`.
    pub fn to_wire(&self) -> CustomRoutingDestinationDescription {
        CustomRoutingDestinationDescription {
            from_port: Some(self.from_port),
            to_port: Some(self.to_port),
            protocols: Some(self.protocols.clone()),
        }
    }
}

/// A custom routing accelerator's endpoint group, as persisted.
///
/// Held apart from [`StoredEndpointGroup`] for the reason [`StoredAccelerator`] gives
/// about the two accelerator kinds: a `CustomRoutingEndpointGroup` carries destination
/// configurations and subnet endpoints instead of health checks, traffic dial and port
/// overrides, and it is reached only through the custom routing operations.
#[derive(Debug, Clone)]
pub struct StoredCustomRoutingEndpointGroup {
    /// `EndpointGroupArn` — also this record's key in
    /// [`GlobalAcceleratorState::custom_routing_endpoint_groups`].
    pub arn: String,
    /// The ARN of the custom routing listener this endpoint group belongs to.
    pub listener_arn: String,
    /// `EndpointGroupRegion`: "An Amazon Web Services Region can have only one endpoint
    /// group for a specific listener" (`CustomRoutingEndpointGroup` doc).
    pub endpoint_group_region: String,
    /// The group's destination configurations, in the order they were configured.
    pub destinations: Vec<StoredCustomRoutingDestination>,
    /// The `EndpointId` of each endpoint (virtual private cloud subnet) in the group, in
    /// the order it was added. A create adds none: endpoints join through
    /// `AddCustomRoutingEndpoints`.
    pub endpoint_ids: Vec<String>,
}

impl StoredCustomRoutingEndpointGroup {
    /// This record as the modelled `CustomRoutingEndpointGroup`.
    pub fn to_wire(&self) -> CustomRoutingEndpointGroup {
        CustomRoutingEndpointGroup {
            endpoint_group_arn: Some(self.arn.clone()),
            endpoint_group_region: Some(self.endpoint_group_region.clone()),
            destination_descriptions: Some(
                self.destinations
                    .iter()
                    .map(StoredCustomRoutingDestination::to_wire)
                    .collect(),
            ),
            endpoint_descriptions: Some(
                self.endpoint_ids
                    .iter()
                    .map(|endpoint_id| CustomRoutingEndpointDescription {
                        endpoint_id: Some(endpoint_id.clone()),
                    })
                    .collect(),
            ),
        }
    }
}

/// One endpoint of an endpoint group, as persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEndpoint {
    /// `EndpointId` — the resource the endpoint names.
    pub endpoint_id: String,
    /// `Weight`, defaulted to [`DEFAULT_ENDPOINT_WEIGHT`] when the caller names none.
    pub weight: i32,
    /// `ClientIPPreservationEnabled`, only when the caller set it: the documented
    /// default depends on the endpoint's own type ("true for Application Load
    /// Balancer endpoints"), which this emulator cannot know, so an unset value stays
    /// unset rather than being invented.
    pub client_ip_preservation_enabled: Option<bool>,
}

impl StoredEndpoint {
    /// This endpoint as the modelled `EndpointDescription`.
    ///
    /// `HealthState` is always [`HEALTH_STATE_INITIAL`], the state AWS reports before
    /// an endpoint's first health checks complete: this emulator runs no health checks
    /// against the named resource, so reporting `HEALTHY` or `UNHEALTHY` would report
    /// the outcome of a check nothing performed. `HealthReason` is left unset — the
    /// model's own `EndpointDescription$HealthReason` doc says it "Returns a null
    /// result".
    pub fn to_wire(&self) -> EndpointDescription {
        EndpointDescription {
            endpoint_id: Some(self.endpoint_id.clone()),
            weight: Some(self.weight),
            health_state: Some(HEALTH_STATE_INITIAL.to_string()),
            health_reason: None,
            client_ip_preservation_enabled: self.client_ip_preservation_enabled,
        }
    }
}

/// An endpoint group, as persisted.
#[derive(Debug, Clone)]
pub struct StoredEndpointGroup {
    /// `EndpointGroupArn` — also this record's key in the state's `endpoint_groups`
    /// map.
    pub arn: String,
    /// The ARN of the listener this endpoint group belongs to.
    pub listener_arn: String,
    /// `EndpointGroupRegion`: "A listener can have only one endpoint group in a
    /// specific Region" (`CreateEndpointGroup$EndpointGroupRegion`).
    pub endpoint_group_region: String,
    /// The group's endpoints, in the order they were configured.
    pub endpoints: Vec<StoredEndpoint>,
    pub traffic_dial_percentage: f32,
    /// Absent only when the listener it defaulted from names no port at all.
    pub health_check_port: Option<i32>,
    /// A `HealthCheckProtocol` value: `TCP`, `HTTP` or `HTTPS`.
    pub health_check_protocol: String,
    /// `HealthCheckPath`, which only an HTTP/HTTPS health check has.
    pub health_check_path: Option<String>,
    /// One of [`SUPPORTED_HEALTH_CHECK_INTERVALS`].
    pub health_check_interval_seconds: i32,
    pub threshold_count: i32,
    /// `PortOverrides`, as `(ListenerPort, EndpointPort)` pairs.
    pub port_overrides: Vec<(i32, i32)>,
}

impl StoredEndpointGroup {
    /// This record as the modelled `EndpointGroup`.
    pub fn to_wire(&self) -> EndpointGroup {
        EndpointGroup {
            endpoint_group_arn: Some(self.arn.clone()),
            endpoint_group_region: Some(self.endpoint_group_region.clone()),
            endpoint_descriptions: Some(
                self.endpoints.iter().map(StoredEndpoint::to_wire).collect(),
            ),
            traffic_dial_percentage: Some(self.traffic_dial_percentage),
            health_check_port: self.health_check_port,
            health_check_protocol: Some(self.health_check_protocol.clone()),
            health_check_path: self.health_check_path.clone(),
            health_check_interval_seconds: Some(self.health_check_interval_seconds),
            threshold_count: Some(self.threshold_count),
            port_overrides: Some(
                self.port_overrides
                    .iter()
                    .map(|&(listener_port, endpoint_port)| PortOverride {
                        listener_port: Some(listener_port),
                        endpoint_port: Some(endpoint_port),
                    })
                    .collect(),
            ),
        }
    }
}

/// An accelerator's flow-log attributes, as persisted. Created with defaults
/// alongside the accelerator, so `Describe`/`UpdateAcceleratorAttributes` can be
/// implemented without a state change.
#[derive(Debug, Clone, Default)]
pub struct StoredAcceleratorAttributes {
    pub flow_logs_enabled: bool,
    pub flow_logs_s3_bucket: Option<String>,
    pub flow_logs_s3_prefix: Option<String>,
}

impl StoredAcceleratorAttributes {
    /// This record as the modelled `AcceleratorAttributes`.
    ///
    /// `FlowLogsEnabled` is always reported — it is a boolean the record always
    /// holds ("The default value is false", `AcceleratorAttributes$FlowLogsEnabled`)
    /// — while the bucket and the prefix are reported only once set, since an
    /// accelerator that never enabled flow logs has neither.
    pub fn to_wire(&self) -> AcceleratorAttributes {
        AcceleratorAttributes {
            flow_logs_enabled: Some(self.flow_logs_enabled),
            flow_logs_s3_bucket: self.flow_logs_s3_bucket.clone(),
            flow_logs_s3_prefix: self.flow_logs_s3_prefix.clone(),
        }
    }

    /// This record as the modelled `CustomRoutingAcceleratorAttributes`, which carries
    /// the same three flow-log members as `AcceleratorAttributes` and reports each of
    /// them for the reasons [`Self::to_wire`] gives: one record per accelerator, named
    /// by whichever API family that accelerator belongs to.
    pub fn to_custom_routing_wire(&self) -> CustomRoutingAcceleratorAttributes {
        CustomRoutingAcceleratorAttributes {
            flow_logs_enabled: Some(self.flow_logs_enabled),
            flow_logs_s3_bucket: self.flow_logs_s3_bucket.clone(),
            flow_logs_s3_prefix: self.flow_logs_s3_prefix.clone(),
        }
    }

    /// Apply the members an attributes update carries to this record.
    ///
    /// Every member but the accelerator's ARN is optional on an attributes update, so an
    /// absent member leaves the stored value alone rather than resetting it: the update
    /// changes what it names and nothing else. Callers apply it to a COPY, so a
    /// configuration [`validate_flow_logs`] refuses never reaches the store.
    pub fn apply_update(
        &mut self,
        flow_logs_enabled: Option<bool>,
        flow_logs_s3_bucket: Option<String>,
        flow_logs_s3_prefix: Option<String>,
    ) {
        if let Some(enabled) = flow_logs_enabled {
            self.flow_logs_enabled = enabled;
        }
        if let Some(bucket) = flow_logs_s3_bucket {
            self.flow_logs_s3_bucket = Some(bucket);
        }
        if let Some(prefix) = flow_logs_s3_prefix {
            self.flow_logs_s3_prefix = Some(prefix);
        }
    }
}

/// A flow-log configuration, validated as a whole.
///
/// The rule is the model's own doc for `UpdateAcceleratorAttributes$FlowLogsEnabled`:
/// "If the value is true, `FlowLogsS3Bucket` and `FlowLogsS3Prefix` must be
/// specified" — flow logs name a destination, so neither member may be missing or
/// empty while they are on. Disabled flow logs constrain neither member. Checked over
/// the configuration the update would LEAVE BEHIND rather than over the request's
/// members, so a call that only flips the flag is judged against the bucket and prefix
/// already stored. Returns the message to put on an `InvalidArgumentException`: the
/// fixed sentence the service states for the member that names nothing, with no part of
/// the request interpolated into it.
pub fn validate_flow_logs(attributes: &StoredAcceleratorAttributes) -> Result<(), String> {
    if !attributes.flow_logs_enabled {
        return Ok(());
    }
    for (message, value) in [
        (FLOW_LOGS_BUCKET_EMPTY_MESSAGE, &attributes.flow_logs_s3_bucket),
        (FLOW_LOGS_PREFIX_EMPTY_MESSAGE, &attributes.flow_logs_s3_prefix),
    ] {
        if value.as_deref().unwrap_or_default().is_empty() {
            return Err(message.to_string());
        }
    }
    Ok(())
}

/// The refusal for flow logs turned on with no bucket to write them to
/// (`AcceleratorAttributes$FlowLogsS3Bucket`). Fixed text: the service names the
/// destination that is missing, never the request's own members or values.
pub const FLOW_LOGS_BUCKET_EMPTY_MESSAGE: &str = "Flow logs s3 bucket should not be empty.";

/// The refusal for flow logs turned on with no prefix under the bucket
/// (`AcceleratorAttributes$FlowLogsS3Prefix`), which the service words for the prefix as
/// [`FLOW_LOGS_BUCKET_EMPTY_MESSAGE`] is worded for the bucket — the same sentence with the
/// part of the destination it names in place, both being the one destination the attributes
/// must state. The bucket's sentence is the one observed from the service; this one follows
/// its wording for the other half of the same destination, so an observation of the prefix
/// refusal is the authority over it.
pub const FLOW_LOGS_PREFIX_EMPTY_MESSAGE: &str = "Flow logs s3 prefix should not be empty.";

/// Per-service state for com-amazonaws-globalaccelerator-globalaccelerator-v20180706. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State {
    /// Accelerators by `AcceleratorArn`. `BTreeMap` so list pagination has a
    /// stable total order to resume in.
    pub accelerators: BTreeMap<String, StoredAccelerator>,
    /// Custom routing accelerators by `AcceleratorArn`, held apart from the standard
    /// ones for the reason [`StoredAccelerator`] gives: each kind is reached only
    /// through its own operations.
    pub custom_routing_accelerators: BTreeMap<String, StoredAccelerator>,
    /// Listeners by `ListenerArn`; the parent's `listener_arns` holds the
    /// per-accelerator order.
    pub listeners: BTreeMap<String, StoredListener>,
    /// Custom routing listeners by `ListenerArn`, held apart from the standard ones for
    /// the reason [`StoredCustomRoutingListener`] gives: each kind is reached only
    /// through its own operations. The parent custom routing accelerator's
    /// `listener_arns` holds the per-accelerator order.
    pub custom_routing_listeners: BTreeMap<String, StoredCustomRoutingListener>,
    /// Endpoint groups by `EndpointGroupArn`; the parent listener's
    /// `endpoint_group_arns` holds the per-listener order.
    pub endpoint_groups: BTreeMap<String, StoredEndpointGroup>,
    /// Custom routing endpoint groups by `EndpointGroupArn`, held apart from the standard
    /// ones for the reason [`StoredCustomRoutingEndpointGroup`] gives: each kind is
    /// reached only through its own operations. The parent custom routing listener's
    /// `endpoint_group_arns` holds the per-listener order.
    pub custom_routing_endpoint_groups: BTreeMap<String, StoredCustomRoutingEndpointGroup>,
    /// Flow-log attributes by `AcceleratorArn`, for an accelerator of either kind:
    /// `AcceleratorAttributes` and `CustomRoutingAcceleratorAttributes` carry the same
    /// three flow-log members, and an `AcceleratorArn` identifies one accelerator
    /// whichever kind it is.
    pub accelerator_attributes: BTreeMap<String, StoredAcceleratorAttributes>,
    /// `<kind>:<scope>:<IdempotencyToken>` -> the ARN that token already minted, so a
    /// repeated create returns the same resource instead of a second one. The scope is
    /// what the token identifies a create request WITHIN: an accelerator's token is
    /// account-scoped (`accelerator:<token>`), a listener's is scoped to its parent
    /// accelerator (`listener:<AcceleratorArn>:<token>`) and an endpoint group's to its
    /// parent listener (`endpoint-group:<ListenerArn>:<token>`), so one token cannot
    /// replay a resource that hangs off a different parent.
    pub idempotency: HashMap<String, String>,
    /// Next offset into the static-IPv4 pool, so allocated addresses are unique.
    pub next_ipv4: u32,
    /// Next offset into the static-IPv6 pool.
    pub next_ipv6: u32,
    /// Next sequence number for [`Self::mint_uuid`], so every identifier this store
    /// mints is built from a distinct input.
    pub next_id: u64,
}

/// Short alias for this service's state struct, whose generated name is long.
pub type GlobalAcceleratorState = ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State;

impl ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State {
    /// Allocate the next static IPv4 address from the pool.
    pub fn allocate_ipv4(&mut self) -> String {
        let offset = self.next_ipv4 % IPV4_POOL_LEN;
        self.next_ipv4 = self.next_ipv4.wrapping_add(1);
        Ipv4Addr::from(IPV4_POOL_BASE + offset).to_string()
    }

    /// Allocate the next static IPv6 address from the pool.
    pub fn allocate_ipv6(&mut self) -> String {
        let offset = u128::from(self.next_ipv6);
        self.next_ipv6 = self.next_ipv6.wrapping_add(1);
        Ipv6Addr::from(IPV6_POOL_BASE + offset).to_string()
    }

    /// Mint a fresh identifier in the `8-4-4-4-12` UUID text form AWS puts in an
    /// accelerator ARN.
    ///
    /// Built from this store's own monotonic counter mixed with the current instant
    /// through the standard library's hasher, so each call yields a distinct, opaque
    /// value carrying the version-4 nibble and the RFC 4122 variant bits — a client
    /// that parses an accelerator id out of its ARN sees a well-formed UUID, and the
    /// id needs no identifier source beyond the store and the clock.
    pub fn mint_uuid(&mut self) -> String {
        let sequence = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        format_uuid(mix(sequence, nanos, 0), mix(sequence, nanos, 1))
    }

    /// How many endpoint groups an accelerator holds across every one of its
    /// listeners — what [`MAX_ENDPOINT_GROUPS_PER_ACCELERATOR`] bounds.
    pub fn endpoint_group_count(&self, accelerator_arn: &str) -> usize {
        self.accelerators
            .get(accelerator_arn)
            .map_or(0, |accelerator| {
                accelerator
                    .listener_arns
                    .iter()
                    .filter_map(|listener_arn| self.listeners.get(listener_arn))
                    .map(|listener| listener.endpoint_group_arns.len())
                    .sum()
            })
    }

    /// How many endpoint groups a CUSTOM ROUTING accelerator holds across every one of its
    /// listeners — [`Self::endpoint_group_count`] for the other family, bounded by the
    /// same [`MAX_ENDPOINT_GROUPS_PER_ACCELERATOR`].
    pub fn custom_routing_endpoint_group_count(&self, accelerator_arn: &str) -> usize {
        self.custom_routing_accelerators
            .get(accelerator_arn)
            .map_or(0, |accelerator| {
                accelerator
                    .listener_arns
                    .iter()
                    .filter_map(|listener_arn| self.custom_routing_listeners.get(listener_arn))
                    .map(|listener| listener.endpoint_group_arns.len())
                    .sum()
            })
    }

    /// The cross-family refusal a STANDARD accelerator lookup of `accelerator_arn` owes,
    /// or `None` when the ARN names no accelerator of the custom routing family either
    /// — in which case the calling operation states its own absent-resource wording.
    ///
    /// The one place a family-scoped accelerator lookup asks "does the other family hold
    /// this ARN?", so every standard operation that resolves an `AcceleratorArn` — a
    /// read, an update, a delete, a list of children, a create's parent check — tells a
    /// caller that used the wrong API family the same thing.
    pub fn custom_routing_accelerator_crossing(&self, accelerator_arn: &str) -> Option<String> {
        self.custom_routing_accelerators
            .contains_key(accelerator_arn)
            .then(|| custom_routing_api_required_message(accelerator_arn))
    }

    /// The cross-family refusal a CUSTOM ROUTING accelerator lookup of `accelerator_arn`
    /// owes — [`Self::custom_routing_accelerator_crossing`] in the other direction.
    pub fn standard_accelerator_crossing(&self, accelerator_arn: &str) -> Option<String> {
        self.accelerators
            .contains_key(accelerator_arn)
            .then(|| standard_resource_not_supported_message(accelerator_arn))
    }

    /// What a CUSTOM ROUTING accelerator lookup that found nothing states: the family
    /// crossing when the standard family holds that ARN
    /// ([`Self::standard_accelerator_crossing`]), else this service's absent-resource
    /// wording ([`accelerator_not_found_message`]).
    ///
    /// One place for the pair so every custom routing operation that resolves an
    /// `AcceleratorArn` tells a caller that used the wrong API family the same thing.
    pub fn custom_routing_accelerator_not_found_message(&self, accelerator_arn: &str) -> String {
        self.standard_accelerator_crossing(accelerator_arn)
            .unwrap_or_else(|| accelerator_not_found_message(accelerator_arn))
    }

    /// The cross-family refusal a STANDARD listener lookup of `listener_arn` owes — the
    /// rule [`Self::custom_routing_accelerator_crossing`] states, one level down.
    pub fn custom_routing_listener_crossing(&self, listener_arn: &str) -> Option<String> {
        self.custom_routing_listeners
            .contains_key(listener_arn)
            .then(|| custom_routing_api_required_message(listener_arn))
    }

    /// The cross-family refusal a CUSTOM ROUTING listener lookup of `listener_arn` owes —
    /// [`Self::custom_routing_listener_crossing`] in the other direction.
    pub fn standard_listener_crossing(&self, listener_arn: &str) -> Option<String> {
        self.listeners
            .contains_key(listener_arn)
            .then(|| standard_resource_not_supported_message(listener_arn))
    }

    /// The cross-family refusal a STANDARD endpoint-group lookup of `endpoint_group_arn`
    /// owes — the rule [`Self::custom_routing_accelerator_crossing`] states, two levels
    /// down.
    pub fn custom_routing_endpoint_group_crossing(
        &self,
        endpoint_group_arn: &str,
    ) -> Option<String> {
        self.custom_routing_endpoint_groups
            .contains_key(endpoint_group_arn)
            .then(|| custom_routing_api_required_message(endpoint_group_arn))
    }

    /// The cross-family refusal a CUSTOM ROUTING endpoint-group lookup of
    /// `endpoint_group_arn` owes — [`Self::custom_routing_endpoint_group_crossing`] in the
    /// other direction.
    pub fn standard_endpoint_group_crossing(&self, endpoint_group_arn: &str) -> Option<String> {
        self.endpoint_groups
            .contains_key(endpoint_group_arn)
            .then(|| standard_resource_not_supported_message(endpoint_group_arn))
    }

    /// What a STANDARD endpoint-group lookup that found nothing states: the family
    /// crossing when the custom routing family holds that ARN
    /// ([`Self::custom_routing_endpoint_group_crossing`]), else this service's
    /// absent-resource wording ([`endpoint_group_not_found_message`]).
    ///
    /// One place for the pair so every standard operation that resolves an
    /// `EndpointGroupArn` — a read, an update, a delete, an endpoint add or remove — tells
    /// a caller that used the wrong API family the same thing.
    pub fn standard_endpoint_group_not_found_message(&self, endpoint_group_arn: &str) -> String {
        self.custom_routing_endpoint_group_crossing(endpoint_group_arn)
            .unwrap_or_else(|| endpoint_group_not_found_message(endpoint_group_arn))
    }

    /// What a CUSTOM ROUTING endpoint-group lookup that found nothing states —
    /// [`Self::standard_endpoint_group_not_found_message`] in the other direction.
    pub fn custom_routing_endpoint_group_not_found_message(
        &self,
        endpoint_group_arn: &str,
    ) -> String {
        self.standard_endpoint_group_crossing(endpoint_group_arn)
            .unwrap_or_else(|| endpoint_group_not_found_message(endpoint_group_arn))
    }

    /// Drop every idempotency record whose resource is gone.
    ///
    /// This is the release path for [`Self::idempotency`]: a record is only useful
    /// while the resource its token minted still exists, so after this call the map
    /// holds at most one entry per live resource and cannot exceed
    /// [`MAX_IDEMPOTENCY_RECORDS`]. Called on the create paths (before a new record is
    /// added) and on the delete paths (where a record's resource disappears), so a
    /// caller looping create/delete with fresh tokens reclaims the memory each time.
    pub fn prune_idempotency(&mut self) {
        let Self {
            accelerators,
            custom_routing_accelerators,
            listeners,
            custom_routing_listeners,
            endpoint_groups,
            custom_routing_endpoint_groups,
            idempotency,
            ..
        } = self;
        idempotency.retain(|_, minted| {
            accelerators.contains_key(minted)
                || custom_routing_accelerators.contains_key(minted)
                || listeners.contains_key(minted)
                || custom_routing_listeners.contains_key(minted)
                || endpoint_groups.contains_key(minted)
                || custom_routing_endpoint_groups.contains_key(minted)
        });
        debug_assert!(idempotency.len() <= MAX_IDEMPOTENCY_RECORDS);
    }
}

/// One 64-bit lane of a minted identifier: the standard library's hasher over the
/// sequence number, the instant and the lane index.
fn mix(sequence: u64, nanos: u128, lane: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_u64(sequence);
    hasher.write_u128(nanos);
    hasher.write_u64(lane);
    hasher.finish()
}

/// Render 128 bits as UUID text, with the version-4 nibble and the RFC 4122 variant
/// bits set so the result is a well-formed UUID.
fn format_uuid(high: u64, low: u64) -> String {
    let time_low = (high >> 32) as u32;
    let time_mid = ((high >> 16) & 0xffff) as u16;
    let time_hi_and_version = 0x4000_u16 | (high & 0x0fff) as u16;
    let clock_seq = 0x8000_u16 | ((low >> 48) & 0x3fff) as u16;
    let node = low & 0x0000_ffff_ffff_ffff;
    format!("{time_low:08x}-{time_mid:04x}-{time_hi_and_version:04x}-{clock_seq:04x}-{node:012x}")
}

/// The index of the IPv4 set in an accelerator's `IpSets`, if it has one.
pub fn ipv4_set_index(ip_sets: &[StoredIpSet]) -> Option<usize> {
    ip_sets
        .iter()
        .position(|set| set.ip_address_family == IP_FAMILY_IPV4)
}

/// An `IdempotencyToken` key for the accelerator creates.
pub fn accelerator_idempotency_key(token: &str) -> String {
    format!("accelerator:{token}")
}

/// An `IdempotencyToken` key for `CreateCustomRoutingAccelerator`.
///
/// Account-scoped like [`accelerator_idempotency_key`], but under its OWN kind: the
/// token identifies one create request, and the same token sent to the standard create
/// is a different request against a different resource kind, so neither may replay the
/// other's accelerator.
pub fn custom_routing_accelerator_idempotency_key(token: &str) -> String {
    format!("custom-routing-accelerator:{token}")
}

/// An `IdempotencyToken` key for the listener creates.
///
/// Scoped by the PARENT accelerator: a token identifies one create request, and the
/// same token sent against a different accelerator is a different request, so it must
/// not replay a listener that hangs off another accelerator.
pub fn listener_idempotency_key(accelerator_arn: &str, token: &str) -> String {
    format!("listener:{accelerator_arn}:{token}")
}

/// An `IdempotencyToken` key for `CreateCustomRoutingListener`.
///
/// Scoped by the PARENT accelerator like [`listener_idempotency_key`], but under its OWN
/// kind, for the reason [`custom_routing_accelerator_idempotency_key`] gives: the same
/// token sent to the standard listener create is a different request against a different
/// resource kind, so neither may replay the other's listener.
pub fn custom_routing_listener_idempotency_key(accelerator_arn: &str, token: &str) -> String {
    format!("custom-routing-listener:{accelerator_arn}:{token}")
}

/// An `IdempotencyToken` key for `CreateEndpointGroup`.
///
/// Scoped by the PARENT listener, for the reason [`listener_idempotency_key`] gives:
/// the same token sent against a different listener is a different create request.
pub fn endpoint_group_idempotency_key(listener_arn: &str, token: &str) -> String {
    format!("endpoint-group:{listener_arn}:{token}")
}

/// An `IdempotencyToken` key for `CreateCustomRoutingEndpointGroup`.
///
/// Scoped by the PARENT listener like [`endpoint_group_idempotency_key`], but under its
/// OWN kind, for the reason [`custom_routing_accelerator_idempotency_key`] gives: the same
/// token sent to the standard endpoint group create is a different request against a
/// different resource kind, so neither may replay the other's endpoint group.
pub fn custom_routing_endpoint_group_idempotency_key(listener_arn: &str, token: &str) -> String {
    format!("custom-routing-endpoint-group:{listener_arn}:{token}")
}

/// Mint an accelerator ARN: the account-scoped ARN of one accelerator, over the record's
/// own freshly minted id (see
/// [`ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State::mint_uuid`]), so it is
/// unique and stable for the life of the record.
///
/// It mints no DNS name: what an accelerator announces as its `DnsName` is the host this
/// emulator is reached at, which is a property of the process rather than of the record
/// ([`announced_dns_host`]).
pub fn mint_accelerator(uuid: &str) -> String {
    ::engine::emulator_arn("globalaccelerator", "", &format!("accelerator/{uuid}"))
}

/// Hex digits in the id of a listener ARN's final segment, as the service's own ARNs
/// carry it (`.../accelerator/<uuid>/listener/<8 hex>`).
pub const LISTENER_ID_HEX_LEN: usize = 8;

/// Hex digits in the id of an endpoint-group ARN's final segment, as the service's own
/// ARNs carry it (`.../listener/<8 hex>/endpoint-group/<12 hex>`): an endpoint group's
/// id is WIDER than its listener's, so the two cannot share one width.
pub const ENDPOINT_GROUP_ID_HEX_LEN: usize = 12;

/// The lowercase-hex id of an ARN's final segment, `digits` wide, taken from a freshly
/// minted UUID (see
/// [`ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State::mint_uuid`]): the
/// UUID's own 32 hex digits are opaque and unique, so a prefix of them is too.
fn arn_segment_id(uuid: &str, digits: usize) -> String {
    uuid.chars().filter(|c| *c != '-').take(digits).collect()
}

/// Mint a listener ARN under its parent accelerator, the form
/// `<accelerator arn>/listener/<id>`.
pub fn mint_listener_arn(accelerator_arn: &str, uuid: &str) -> String {
    let id = arn_segment_id(uuid, LISTENER_ID_HEX_LEN);
    format!("{accelerator_arn}/listener/{id}")
}

/// Mint an endpoint group ARN under its parent listener, the form
/// `<listener arn>/endpoint-group/<id>` — the shape the service's own ARNs have
/// (`.../accelerator/<id>/listener/<id>/endpoint-group/<id>`), whose endpoint-group id
/// is [`ENDPOINT_GROUP_ID_HEX_LEN`] hex digits wide.
pub fn mint_endpoint_group_arn(listener_arn: &str, uuid: &str) -> String {
    let id = arn_segment_id(uuid, ENDPOINT_GROUP_ID_HEX_LEN);
    format!("{listener_arn}/endpoint-group/{id}")
}

/// Which punctuation an accelerator name may carry — the ONE way the two accelerator
/// kinds' documented name rules differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamePunctuation {
    /// `CreateAccelerator$Name`: "alphanumeric characters, periods (.), or hyphens (-)".
    PeriodsAndHyphens,
    /// `CreateCustomRoutingAccelerator$Name`: "only alphanumeric characters or
    /// hyphens (-)".
    HyphensOnly,
}

/// The sentence this service states a name outside the accelerator name-length limit
/// with.
///
/// Recorded wire behaviour on `CreateAccelerator`: a name the limit does not admit is
/// refused with ONE fixed sentence that names the two bounds of that limit
/// ([`ACCELERATOR_NAME_MIN_LEN`] and [`ACCELERATOR_NAME_MAX_LEN`]) and takes nothing from
/// the request — neither the name submitted nor its length. It is the wording for the
/// CLASS (a name of an accelerator is outside the length the service accepts), so it is
/// stated for either bound and for every leg that names an accelerator, rather than for
/// the one bound a recording happened to break.
pub fn accelerator_name_length_message() -> String {
    format!(
        "Accelerator Name should be between {ACCELERATOR_NAME_MIN_LEN} - \
         {ACCELERATOR_NAME_MAX_LEN} characters in size."
    )
}

/// Refuse a name the real service refuses.
///
/// One rule for both accelerator kinds: between [`ACCELERATOR_NAME_MIN_LEN`] and
/// [`ACCELERATOR_NAME_MAX_LEN`] characters, only alphanumerics and the punctuation
/// `punctuation` admits, and never that punctuation at either end. Returns the message to
/// put on an `InvalidArgumentException`: the length limit is stated with the service's own
/// fixed sentence ([`accelerator_name_length_message`]), and the character rule with the
/// rule the name broke.
fn validate_name(name: &str, punctuation: NamePunctuation) -> Result<(), String> {
    let (allowed, edge_names) = match punctuation {
        NamePunctuation::PeriodsAndHyphens => (
            "alphanumeric characters, periods (.), or hyphens (-)",
            "a hyphen or period",
        ),
        NamePunctuation::HyphensOnly => ("alphanumeric characters or hyphens (-)", "a hyphen"),
    };
    let bad = |reason: &str| {
        Err(format!(
            "Name {reason}: a name can have a maximum of {ACCELERATOR_NAME_MAX_LEN} characters, \
             must contain only {allowed}, and must not begin or end with {edge_names}"
        ))
    };
    let is_edge = |c: char| match punctuation {
        NamePunctuation::PeriodsAndHyphens => c == '-' || c == '.',
        NamePunctuation::HyphensOnly => c == '-',
    };
    if !(ACCELERATOR_NAME_MIN_LEN..=ACCELERATOR_NAME_MAX_LEN).contains(&name.chars().count()) {
        return Err(accelerator_name_length_message());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || is_edge(c))
    {
        return bad("contains an unsupported character");
    }
    if name.starts_with(is_edge) || name.ends_with(is_edge) {
        return bad(&format!("begins or ends with {edge_names}"));
    }
    Ok(())
}

/// Refuse a standard accelerator's name the real service refuses.
///
/// The rule is the model's own doc for `CreateAccelerator$Name` /
/// `Accelerator$Name`: "The name can have a maximum of 64 characters, must
/// contain only alphanumeric characters, periods (.), or hyphens (-), and must
/// not begin or end with a hyphen or period."
pub fn validate_accelerator_name(name: &str) -> Result<(), String> {
    validate_name(name, NamePunctuation::PeriodsAndHyphens)
}

/// Refuse a custom routing accelerator's name the real service refuses.
///
/// The rule is the model's own doc for `CreateCustomRoutingAccelerator$Name`: "The name
/// can have a maximum of 64 characters, must contain only alphanumeric characters or
/// hyphens (-), and must not begin or end with a hyphen." A period, which a standard
/// accelerator's name may carry, is NOT in this set.
pub fn validate_custom_routing_accelerator_name(name: &str) -> Result<(), String> {
    validate_name(name, NamePunctuation::HyphensOnly)
}

/// The `IpAddressType` a custom routing accelerator ends up with.
///
/// "The IP address type that an accelerator supports. For a custom routing accelerator,
/// the value must be IPV4" (`CreateCustomRoutingAccelerator$IpAddressType`), so the
/// other value the enum offers is refused rather than stored, and a request that names
/// no type gets [`IP_ADDRESS_TYPE_IPV4`] — the only type this kind supports. Returns
/// the message for an `InvalidArgumentException`.
pub fn resolve_custom_routing_ip_address_type(supplied: Option<String>) -> Result<String, String> {
    match supplied {
        None => Ok(IP_ADDRESS_TYPE_IPV4.to_string()),
        Some(value) if value == IP_ADDRESS_TYPE_IPV4 => Ok(value),
        Some(value) => Err(format!(
            "IpAddressType {value} is not supported: for a custom routing accelerator, the IP \
             address type must be {IP_ADDRESS_TYPE_IPV4}"
        )),
    }
}

/// A single `PortRange`, validated.
///
/// A port range must carry both ends and run forwards: `FromPort` and `ToPort`
/// are the inclusive bounds of one range (`PortRange` doc), so a range missing an
/// end, or with `FromPort > ToPort`, names no ports at all. The model's own
/// `PortNumber` range (1..65535) is refused by the boundary validator before a
/// handler runs; it is re-checked here so the rule holds wherever this helper is
/// called from.
pub fn validate_port_range(range: &PortRange) -> Result<(i32, i32), String> {
    let (Some(from), Some(to)) = (range.from_port, range.to_port) else {
        return Err("a PortRange must carry both FromPort and ToPort".to_string());
    };
    validate_port_bounds(from, to)
}

/// The inclusive bounds of one port range, validated.
///
/// The rule [`validate_port_range`] states, over a range whose two ends the model makes
/// REQUIRED (`CustomRoutingDestinationConfiguration$FromPort` / `$ToPort`) rather than
/// optional, so both callers refuse the same ranges with the same words.
pub fn validate_port_bounds(from: i32, to: i32) -> Result<(i32, i32), String> {
    if !(1..=65535).contains(&from) || !(1..=65535).contains(&to) {
        return Err(format!(
            "PortRange {from}-{to} is outside the supported port range 1-65535"
        ));
    }
    if from > to {
        // The sentence the service states this violation with: the requirement on the
        // two members, with no part taken from the request (recorded wire behaviour on
        // `CreateListener`).
        return Err("FromPort should be less than or equal to ToPort".to_string());
    }
    Ok((from, to))
}

/// A caller-supplied `HealthCheckIntervalSeconds`, validated.
///
/// The rule is the model's own doc for `HealthCheckIntervalSeconds`: "The time—10
/// seconds or 30 seconds—between each health check for an endpoint", so the only
/// accepted values are [`SUPPORTED_HEALTH_CHECK_INTERVALS`]. The `@range` on the shape
/// (10..30) is the enclosing interval, not the set, so every value strictly between the
/// two reaches a handler and is refused here. Returns the message to put on an
/// `InvalidArgumentException`.
pub fn validate_health_check_interval(seconds: i32) -> Result<(), String> {
    if SUPPORTED_HEALTH_CHECK_INTERVALS.contains(&seconds) {
        return Ok(());
    }
    Err(unsupported_health_check_interval_message(
        &seconds.to_string(),
    ))
}

/// The sentence this service states an unsupported `HealthCheckIntervalSeconds` with.
///
/// Recorded wire behaviour on `UpdateEndpointGroup`: the fixed text
/// `Invalid HealthCheck Interval Seconds: ` + ` expected values are: ` over the rejected
/// value as the request carried it and the set of intervals the service supports
/// ([`SUPPORTED_HEALTH_CHECK_INTERVALS`], joined the way the sentence names alternatives).
/// It is the wording for the CLASS — the value is not one of the intervals this service
/// health-checks at — so it is stated for every value outside that set, whether or not the
/// value is also outside the enclosing `@range` the member declares.
pub fn unsupported_health_check_interval_message(submitted: &str) -> String {
    let supported = SUPPORTED_HEALTH_CHECK_INTERVALS
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(" or ");
    format!("Invalid HealthCheck Interval Seconds: {submitted} expected values are: {supported}")
}

/// The health-check path an endpoint group ends up with.
///
/// A path only belongs to an HTTP/S health check — "If the protocol is HTTP/S, then
/// this specifies the path that is the destination for health check targets. The
/// default value is slash (/)" (`CreateEndpointGroup$HealthCheckPath`) — so a group
/// the caller gave no path gets [`DEFAULT_HEALTH_CHECK_PATH`] under `HTTP`/`HTTPS` and
/// no path at all under `TCP`, which pings a port rather than a path. A path the caller
/// did supply is kept whatever the protocol.
pub fn resolve_health_check_path(supplied: Option<String>, protocol: &str) -> Option<String> {
    match supplied {
        Some(path) => Some(path),
        None => (protocol == HEALTH_CHECK_PROTOCOL_HTTP || protocol == HEALTH_CHECK_PROTOCOL_HTTPS)
            .then(|| DEFAULT_HEALTH_CHECK_PATH.to_string()),
    }
}

/// A `PortOverrides` list as the `(ListenerPort, EndpointPort)` pairs the store holds.
///
/// A port override maps one listener port to one endpoint port (`PortOverride` doc), so
/// an entry missing either end maps nothing, a port outside the model's `PortNumber`
/// range (1..65535) is not a port, and the same listener port named twice would map one
/// arriving port to two endpoint ports. The list is refused past
/// [`MAX_PORT_OVERRIDES_PER_ENDPOINT_GROUP`] BEFORE it is walked, so neither the work
/// here nor the stored vector is proportional to a length the caller chose. Returns the
/// message for an `InvalidArgumentException`.
pub fn port_overrides_to_pairs(overrides: &[PortOverride]) -> Result<Vec<(i32, i32)>, String> {
    if overrides.len() > MAX_PORT_OVERRIDES_PER_ENDPOINT_GROUP {
        return Err(format!(
            "PortOverrides holds {} entries; an endpoint group has at most \
             {MAX_PORT_OVERRIDES_PER_ENDPOINT_GROUP} port overrides",
            overrides.len()
        ));
    }
    let mut seen_listener_ports: HashSet<i32> = HashSet::with_capacity(overrides.len());
    let mut pairs: Vec<(i32, i32)> = Vec::with_capacity(overrides.len());
    for entry in overrides {
        let (Some(listener_port), Some(endpoint_port)) = (entry.listener_port, entry.endpoint_port)
        else {
            return Err(
                "a PortOverride must carry both ListenerPort and EndpointPort".to_string()
            );
        };
        for (member, port) in [
            ("ListenerPort", listener_port),
            ("EndpointPort", endpoint_port),
        ] {
            if !(1..=65535).contains(&port) {
                return Err(format!(
                    "PortOverride {member} {port} is outside the supported port range 1-65535"
                ));
            }
        }
        if !seen_listener_ports.insert(listener_port) {
            return Err(format!(
                "PortOverrides names ListenerPort {listener_port} more than once; a listener \
                 port maps to one endpoint port"
            ));
        }
        pairs.push((listener_port, endpoint_port));
    }
    Ok(pairs)
}

/// Whether an `EndpointId` names an Amazon EC2 instance.
///
/// The member's own doc says how each endpoint kind is named: "If the endpoint is a
/// Network Load Balancer or Application Load Balancer, this is the Amazon Resource Name
/// (ARN) of the resource. If the endpoint is an Elastic IP address, this is the Elastic IP
/// address allocation ID. For Amazon EC2 instances, this is the EC2 instance ID"
/// (`EndpointConfiguration$EndpointId`). An instance ID is `i-` followed by hexadecimal
/// characters, one of [`EC2_INSTANCE_ID_HEX_LENGTHS`] of them, so an id of exactly that
/// form names an instance and an id of any other form names an endpoint of another kind.
pub fn endpoint_id_names_ec2_instance(endpoint_id: &str) -> bool {
    let Some(hex) = endpoint_id.strip_prefix("i-") else {
        return false;
    };
    EC2_INSTANCE_ID_HEX_LENGTHS.contains(&hex.len())
        && hex.chars().all(|digit| matches!(digit, '0'..='9' | 'a'..='f'))
}

/// The sentence this service states when a registration turns client IP address
/// preservation OFF for an endpoint that is an EC2 instance.
///
/// An EC2 instance endpoint is reached WITH the client's address preserved — client IP
/// address preservation "is supported ... for endpoints that are Application Load
/// Balancers, Amazon EC2 instances, and Network Load Balancers with security groups"
/// (`EndpointConfiguration$ClientIPPreservationEnabled`), and for an instance it cannot be
/// turned off — so the refusal is the class's wording, fixed text with no part taken from
/// the request (recorded wire behaviour on `UpdateEndpointGroup`).
pub const CLIENT_IP_PRESERVATION_NOT_OPTIONAL_MESSAGE: &str =
    "Client IP Preservation cannot be set to false for EC2 instances";

/// An `EndpointConfigurations` list as the endpoints the store holds.
///
/// An endpoint configuration identifies one resource to send traffic to ("A resource
/// must be valid and active when you add it as an endpoint", `EndpointConfiguration`
/// doc), so an entry naming no `EndpointId` names no resource and one resource cannot
/// be named twice in the same group — two weights for one endpoint. An absent `Weight`
/// takes [`DEFAULT_ENDPOINT_WEIGHT`]. An endpoint that is an EC2 instance keeps client IP
/// address preservation on, the rule
/// [`CLIENT_IP_PRESERVATION_NOT_OPTIONAL_MESSAGE`] states.
///
/// The list is refused past [`MAX_ENDPOINTS_PER_ENDPOINT_GROUP`] BEFORE it is walked,
/// and membership is tested through a set rather than by rescanning what has been
/// accepted: `EndpointId` is a free string, so neither the work this helper does nor the
/// vector the store ends up holding may be proportional to a length the caller chose.
/// Returns the message for an `InvalidArgumentException`.
pub fn endpoints_from_configurations(
    configurations: &[EndpointConfiguration],
) -> Result<Vec<StoredEndpoint>, String> {
    if configurations.len() > MAX_ENDPOINTS_PER_ENDPOINT_GROUP {
        return Err(format!(
            "EndpointConfigurations holds {} endpoints; an endpoint group holds at most \
             {MAX_ENDPOINTS_PER_ENDPOINT_GROUP}",
            configurations.len()
        ));
    }
    let mut seen_ids: HashSet<&str> = HashSet::with_capacity(configurations.len());
    let mut endpoints: Vec<StoredEndpoint> = Vec::with_capacity(configurations.len());
    for configuration in configurations {
        let Some(endpoint_id) = configuration
            .endpoint_id
            .as_deref()
            .filter(|id| !id.is_empty())
        else {
            return Err("an EndpointConfiguration must carry an EndpointId".to_string());
        };
        if !seen_ids.insert(endpoint_id) {
            return Err(format!(
                "EndpointConfigurations names endpoint {endpoint_id:?} more than once; an \
                 endpoint group holds each endpoint once"
            ));
        }
        // Client IP address preservation is not optional for an endpoint that is an EC2
        // instance — the rule [`CLIENT_IP_PRESERVATION_NOT_OPTIONAL_MESSAGE`] states —
        // while an endpoint of any other kind takes either value, so a configuration that
        // turns it off for an instance ID is refused here, on every leg that registers an
        // endpoint, before anything is stored.
        if configuration.client_ip_preservation_enabled == Some(false)
            && endpoint_id_names_ec2_instance(endpoint_id)
        {
            return Err(CLIENT_IP_PRESERVATION_NOT_OPTIONAL_MESSAGE.to_string());
        }
        endpoints.push(StoredEndpoint {
            endpoint_id: endpoint_id.to_string(),
            weight: configuration.weight.unwrap_or(DEFAULT_ENDPOINT_WEIGHT),
            client_ip_preservation_enabled: configuration.client_ip_preservation_enabled,
        });
    }
    Ok(endpoints)
}

/// A `CustomRoutingDestinationConfigurations` list as the destinations the store holds.
///
/// Each configuration "sets the port range and protocol for all endpoints (virtual private
/// cloud subnets) in a custom routing endpoint group to accept client traffic on"
/// (`CreateCustomRoutingEndpointGroup$DestinationConfigurations`), so a configuration whose
/// range names no port — the rule [`validate_port_bounds`] states — sets nothing. Returns
/// the message for an `InvalidPortRangeException`.
///
/// The list is refused past [`MAX_DESTINATIONS_PER_ENDPOINT_GROUP`] BEFORE it is walked, so
/// neither the work this helper does nor the vector the store ends up holding is
/// proportional to a length the caller chose.
pub fn destinations_from_configurations(
    configurations: &[CustomRoutingDestinationConfiguration],
) -> Result<Vec<StoredCustomRoutingDestination>, String> {
    if configurations.len() > MAX_DESTINATIONS_PER_ENDPOINT_GROUP {
        return Err(format!(
            "DestinationConfigurations holds {} entries; an endpoint group has at most \
             {MAX_DESTINATIONS_PER_ENDPOINT_GROUP}",
            configurations.len()
        ));
    }
    let mut destinations: Vec<StoredCustomRoutingDestination> =
        Vec::with_capacity(configurations.len());
    for configuration in configurations {
        let (from_port, to_port) =
            validate_port_bounds(configuration.from_port, configuration.to_port)?;
        destinations.push(StoredCustomRoutingDestination {
            from_port,
            to_port,
            protocols: configuration.protocols.clone(),
        });
    }
    Ok(destinations)
}

/// Validate a caller-supplied static IPv4 address (the BYOIP path of
/// `CreateAccelerator$IpAddresses`). Returns the message for an
/// `InvalidArgumentException`.
pub fn validate_static_ipv4(address: &str) -> Result<(), String> {
    if address.parse::<Ipv4Addr>().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "IpAddresses entry {address:?} is not a valid IPv4 address"
        ))
    }
}

/// The whole caller-supplied `IpAddresses` list, validated.
///
/// One accelerator has exactly two static IPv4 addresses, so a caller may name at
/// most two and may not name one twice; each entry must be an IPv4 literal. The
/// single place the rule lives, so the create and the update path cannot drift.
/// Returns the message for an `InvalidArgumentException`.
pub fn validate_static_ipv4_addresses(addresses: &[String]) -> Result<(), String> {
    if addresses.len() > 2 {
        return Err(format!(
            "IpAddresses holds {} addresses; an accelerator has at most two static IPv4 \
             addresses",
            addresses.len()
        ));
    }
    for (index, address) in addresses.iter().enumerate() {
        validate_static_ipv4(address)?;
        if addresses[..index].contains(address) {
            return Err(format!(
                "IpAddresses names {address:?} more than once; an accelerator's static \
                 IPv4 addresses are distinct"
            ));
        }
    }
    Ok(())
}

/// The message a create states when the account already holds as many accelerators of
/// that kind as [`MAX_ACCELERATORS_PER_ACCOUNT`] allows.
///
/// One place for the wording so both accelerator creates state the quota they refused
/// against alike, with the limit interpolated from the constant rather than written
/// into the sentence.
pub fn accelerator_quota_exceeded_message() -> String {
    format!("the account already holds the maximum of {MAX_ACCELERATORS_PER_ACCOUNT} accelerators")
}

/// The message a listener create states when its parent accelerator already holds the
/// documented per-accelerator quota.
///
/// One place for the wording so both listener creates state the quota they refused
/// against alike, with the limit interpolated from the constant rather than written into
/// the sentence — the same rule as [`accelerator_quota_exceeded_message`], one level
/// down.
pub fn listener_quota_exceeded_message() -> String {
    format!(
        "the accelerator already holds the maximum of {MAX_LISTENERS_PER_ACCELERATOR} listeners"
    )
}

/// The message for a refusal that names an accelerator the store does not hold.
///
/// A refusal identifies the resource the request named, so the caller — commonly a
/// cleanup loop over many accelerators — can attribute it: the ARN is interpolated
/// from the request rather than left out of a constant sentence. One place for the
/// wording so every operation that answers `AcceleratorNotFoundException` states it
/// alike.
pub fn accelerator_not_found_message(accelerator_arn: &str) -> String {
    format!("Accelerator {accelerator_arn} not found.")
}

/// The message a create states when the accelerator it was asked to attach the new
/// resource to does not exist.
///
/// The service words the parent lookup of a create as a sentence with no variable part:
/// recorded wire behaviour on `CreateListener`. The operation that is ASKED about one
/// accelerator (a describe, a delete, an update, a list of its children) states
/// [`accelerator_not_found_message`] instead, which names the ARN the request carried,
/// so each path keeps the wording it is observed with.
pub fn create_parent_accelerator_not_found_message() -> String {
    "Could not find accelerator.".to_string()
}

/// The message for a refusal that names a listener the store does not hold. The same
/// rule as [`accelerator_not_found_message`], one level down.
pub fn listener_not_found_message(listener_arn: &str) -> String {
    format!("Listener {listener_arn} not found.")
}

/// The sentence this service states the per-listener port-range limit
/// ([`MAX_PORT_RANGES_PER_LISTENER`]) with on the leg that CREATES a listener.
///
/// The limit belongs to a listener, and a create has no listener yet, so it is stated
/// against the accelerator the new listener would belong to: fixed text whose single
/// variable part is the `AcceleratorArn` the request carried (recorded wire behaviour on
/// `CreateListener`). One place for the wording, beside the listener-scoped form the
/// update leg states, so the two legs of one limit cannot drift apart.
pub fn port_range_limit_for_accelerator_message(accelerator_arn: &str) -> String {
    format!(
        "Limit Exceeded for the number of Port Ranges allowed in a Listener for Accelerator \
         {accelerator_arn}."
    )
}

/// The same limit on the leg that UPDATES a listener that already exists.
///
/// Stated against the listener the request identified rather than against its accelerator
/// — the resource the call is about is the listener itself, so the sentence names it and
/// carries no accelerator at all — with the `ListenerArn` the request carried as its
/// single variable part (recorded wire behaviour on `UpdateListener`).
pub fn port_range_limit_for_listener_message(listener_arn: &str) -> String {
    format!("Limit Exceeded for the number of Port Ranges allowed in a Listener for {listener_arn}.")
}

/// The message a STANDARD operation refuses an ARN with that names a resource of the
/// CUSTOM ROUTING family.
///
/// Standard and custom routing resources are distinct resource types served by distinct
/// operations — the model gives each kind its own output shape (`Accelerator` vs
/// `CustomRoutingAccelerator`, `Listener` vs `CustomRoutingListener`) and its own
/// create, read, update, delete and list — so an ARN that names a custom routing
/// resource names a resource that EXISTS and is reached through another API family, not
/// an unknown one: the refusal says which family serves it and names the ARN the request
/// carried, so the caller can reissue the call. Recorded wire behaviour; the model states
/// no wording. The sentence is the FAMILY's, not one resource kind's, so it is stated for
/// a custom routing accelerator and a custom routing listener alike.
pub fn custom_routing_api_required_message(arn: &str) -> String {
    format!(
        "Custom Routing resources must use the Custom Routing APIs; got custom routing \
         resource: {arn}"
    )
}

/// The message a CUSTOM ROUTING operation refuses an ARN with that names a resource of
/// the STANDARD family — the same rule as [`custom_routing_api_required_message`] in the
/// other direction, in the wording recorded for it, which names the standard family
/// rather than the resource kind.
pub fn standard_resource_not_supported_message(arn: &str) -> String {
    format!(
        "Only Custom Routing resources are supported by this custom routing API; got \
         standard accelerator: {arn}"
    )
}

/// The message for a refusal that names an endpoint group the store does not hold. The
/// same rule as [`accelerator_not_found_message`], one level further down, in the
/// wording the service states this error class with: the resource kind as the model
/// spells the shape, the ARN the request carried, and "does not exist" (recorded wire
/// behaviour on `DescribeEndpointGroup`).
pub fn endpoint_group_not_found_message(endpoint_group_arn: &str) -> String {
    format!("EndpointGroup {endpoint_group_arn} does not exist.")
}

/// The message for a create that names a Region the listener's groups already cover.
///
/// "A listener can have only one endpoint group in a specific Region"
/// (`CreateEndpointGroup$EndpointGroupRegion`), and the service states that conflict with
/// one FIXED sentence — recorded wire behaviour on `CreateEndpointGroup` — naming neither
/// the Region, nor the listener, nor the group that already covers it. Every duplicate
/// therefore gets the same text, and no generated identifier leaks into it.
pub const ENDPOINT_GROUP_ALREADY_EXISTS_MESSAGE: &str = "Endpoint group already exists.";

/// The message an endpoint group create states when its accelerator already holds as many
/// endpoint groups as [`MAX_ENDPOINT_GROUPS_PER_ACCELERATOR`] allows.
///
/// One place for the wording so both endpoint group creates — the standard family's and
/// the custom routing family's — state the quota they refused against alike, with the
/// accelerator and the limit interpolated rather than written into the sentence; the same
/// rule [`accelerator_quota_exceeded_message`] states one level up.
pub fn endpoint_group_quota_exceeded_message(accelerator_arn: &str) -> String {
    format!(
        "Accelerator {accelerator_arn} already holds the maximum of \
         {MAX_ENDPOINT_GROUPS_PER_ACCELERATOR} endpoint groups."
    )
}

/// The message for an endpoint whose `AttachmentArn` names a cross-account attachment the
/// store does not hold.
///
/// An `AttachmentArn` names "the Amazon Resource Name (ARN) of the cross-account
/// attachment that specifies the endpoints (resources) that can be added to accelerators
/// and principals that have permission to add the endpoints"
/// (`EndpointConfiguration$AttachmentArn`), and the attachment operations are not
/// implemented in this emulator, so no stored attachment can grant that permission. The
/// refusal names the attachment the request claimed and the endpoint it was claimed for,
/// and is stated once for every leg that configures an endpoint — the create and the
/// update — so the two cannot drift apart.
pub fn attachment_grants_nothing_message(attachment_arn: &str, endpoint_id: &str) -> String {
    format!(
        "cross-account attachment {attachment_arn} does not exist, so it grants no permission \
         to add endpoint {endpoint_id}"
    )
}

/// The message an add states for an endpoint the group already holds.
///
/// A group holds each endpoint once — the rule [`endpoints_from_configurations`] states
/// inside one request — so naming a member again asks to change it rather than to add
/// it, which is what the service's sentence says: fixed text and the request's own
/// `EndpointId` as the only variable part (recorded wire behaviour on `AddEndpoints`),
/// naming neither the group nor the member's stored configuration.
pub fn cannot_update_existing_endpoint_message(endpoint_id: &str) -> String {
    format!("Cannot update an existing endpoint: {endpoint_id}.")
}

/// The message an add states when the endpoints it would leave behind exceed
/// [`MAX_ENDPOINTS_PER_ENDPOINT_GROUP`].
///
/// One place for the wording, with the group, the membership it already holds and the
/// quota interpolated rather than written into the sentence — the same rule
/// [`accelerator_quota_exceeded_message`] states one level up.
pub fn endpoint_quota_exceeded_message(
    endpoint_group_arn: &str,
    held: usize,
    added: usize,
) -> String {
    format!(
        "Endpoint group {endpoint_group_arn} already holds {held} of the maximum of \
         {MAX_ENDPOINTS_PER_ENDPOINT_GROUP} endpoints; adding {added} more would exceed it."
    )
}

/// The message for deleting an accelerator that is still enabled.
///
/// "Before you can delete an accelerator, you must disable it" (`DeleteAccelerator`
/// doc), so the refusal tells the caller which accelerator to disable, by the ARN the
/// request carried.
pub fn accelerator_not_disabled_message(accelerator_arn: &str) -> String {
    format!("Please disable accelerator {accelerator_arn} before deleting it.")
}

/// The message for a delete refused because a dependent resource still exists.
///
/// "You must remove all dependent resources from a resource before you can delete it"
/// (`AssociatedListenerFoundException` / `AssociatedEndpointGroupFoundException` docs).
/// The service states that whole class with ONE template — recorded wire behaviour on
/// `DeleteListener` — whose only variable parts are the kind of resource the request
/// asked to delete, the ARN it carried for it, and the kind of dependent that blocks it:
/// `Cannot delete <resource> <arn> because an associated <dependent> was found.`
fn associated_dependent_found_message(resource: &str, arn: &str, dependent: &str) -> String {
    format!("Cannot delete {resource} {arn} because an associated {dependent} was found.")
}

/// The message for deleting an accelerator that still has a listener — the class
/// template of [`associated_dependent_found_message`] at the accelerator level.
pub fn associated_listener_found_message(accelerator_arn: &str) -> String {
    associated_dependent_found_message("accelerator", accelerator_arn, "listener")
}

/// The message for deleting a listener that still has an endpoint group — the same
/// template one level down.
pub fn associated_endpoint_group_found_message(listener_arn: &str) -> String {
    associated_dependent_found_message("listener", listener_arn, "endpoint group")
}

/// One page of keys, plus the token that resumes immediately after it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The keys on this page, in the collection's order.
    pub keys: Vec<String>,
    /// The `NextToken` to resume with, absent on the last page.
    pub next_token: Option<String>,
}

/// The caller's `NextToken` did not decode, or named a position the collection no
/// longer holds — an `InvalidNextTokenException`.
#[derive(Debug, Clone, Copy)]
pub struct InvalidPageToken;

/// Encode a `NextToken` that resumes at `key`.
///
/// Hex rather than the bare key: `NextToken` is an opaque continuation token, so
/// a caller must not be able to read or hand-build a position out of it, and a
/// token that is not hex is recognisably not one this service issued.
pub fn encode_next_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a `NextToken` resumes at, or `None` when the token is not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The id at the end of an ARN: its last path segment, which is the id this service
/// minted for that resource (`.../accelerator/<id>/listener/<id>/endpoint-group/<id>`).
///
/// A continuation token names a position by this id rather than by the whole ARN, because
/// `NextToken` is a `GenericString` (`@length` 0..255) and the hex encoding is twice its
/// key's length: the ARN of a nested resource — an endpoint group's, which hangs off its
/// listener's, which hangs off its accelerator's — already exceeds half that bound, so a
/// token naming the whole ARN would be a token the caller cannot send back. Within one
/// collection the ids are distinct, so the shorter key names the same position.
fn arn_resource_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// The message for a `NextToken` this service did not issue.
///
/// "There isn't another item to return" (`InvalidNextTokenException` doc): a token that
/// does not decode, or that names a position the collection no longer holds, resumes
/// nothing. Stated once, beside [`paginate`], because every paginated list refuses the
/// same class and the three of them must not drift apart.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "NextToken is not a valid continuation token";

/// Cut one page out of `all`.
///
/// `max_results` truncates the page; the returned token names the first key of
/// the following page, so resuming with it continues exactly where this page
/// ended, and it is absent on the last page. A token that does not decode, or
/// that names a key the collection no longer holds, is refused.
///
/// The page is ALWAYS clamped to [`DEFAULT_MAX_RESULTS`], which is also the page a
/// caller that names no `MaxResults` gets, so one small request can never answer with
/// the whole store. A non-positive `max_results` is treated as absent: the model's own
/// `MaxResults` range (1..100) makes it unreachable from the wire, and a zero-length
/// page carrying a continuation token would be a page that never makes progress.
pub fn paginate(
    all: &[String],
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page, InvalidPageToken> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let id = decode_next_token(token).ok_or(InvalidPageToken)?;
            all.iter()
                .position(|candidate| arn_resource_id(candidate) == id)
                .ok_or(InvalidPageToken)?
        }
    };
    let limit = max_results
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(DEFAULT_MAX_RESULTS) as usize;
    let end = start.saturating_add(limit).min(all.len());
    Ok(Page {
        keys: all[start..end].to_vec(),
        next_token: (end < all.len()).then(|| encode_next_token(arn_resource_id(&all[end]))),
    })
}

/// A `Tags` list as the key/value pairs the store holds.
pub fn tags_to_pairs(tags: Option<Vec<Tag>>) -> Vec<(String, String)> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// A `SystemTime` as the modelled `Timestamp`, at millisecond resolution.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State>)
}
