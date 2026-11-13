// SPDX-License-Identifier: CC0-1.0

//! The stored instance — this service's primary resource — with the hardware,
//! addresses, metadata options and lifecycle state the service assigns it.

use super::*;

/// One stored instance: every member `GetInstance` reports, plus the launch
/// script `CreateInstances` accepted and the clock-owed transition its lifecycle
/// is waiting on.
#[derive(Debug, Clone)]
pub struct StoredInstance {
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub blueprint_id: String,
    pub bundle_id: String,
    pub add_ons: Vec<AddOn>,
    /// No static IP is attached: `AllocateStaticIp` / `AttachStaticIp` are not
    /// part of this slice, so nothing can have attached one.
    pub is_static_ip: bool,
    pub private_ip_address: Option<String>,
    pub public_ip_address: Option<String>,
    pub ipv6_addresses: Vec<String>,
    pub ip_address_type: String,
    pub bundle: BundleSpec,
    pub ssh_key_name: String,
    /// The launch script the request carried, kept so the operations that serve
    /// it can report it; no member of `Instance` exposes it.
    pub user_data: Option<String>,
    /// The inbound rules this instance's firewall holds, in a stable total order
    /// (see [`StoredInstance::sort_ports`]), so every read reports the same
    /// sequence. Created with the platform's defaults, then whatever
    /// `OpenInstancePublicPorts`, `CloseInstancePublicPorts` and
    /// `PutInstancePublicPorts` have made of them.
    pub ports: Vec<StoredPortRule>,
    pub state: Lifecycle,
    /// The state the clock owes this instance, while one is outstanding.
    pub due: Option<DueTransition>,
    /// The GUI (Amazon DCV) session access `StartGUISession` and
    /// `CreateGUISessionAccessDetails` created for this virtual computer, while
    /// one exists. A session runs ON the instance, so it lives on the instance's
    /// own record: deleting the instance takes its session with it.
    pub gui_session: Option<StoredGuiSession>,
    /// How this instance's metadata service is configured — what
    /// `UpdateInstanceMetadataOptions` sets and `Instance$metadataOptions`
    /// reports.
    pub metadata_options: StoredMetadataOptions,
    /// The host keys the service has RECORDED for this instance — the known
    /// host keys the Lightsail browser-based SSH and RDP clients authenticate
    /// it by, each carrying "the time that the SSH host key or RDP certificate
    /// was recorded by Lightsail" (`HostKeyAttributes$witnessedAt`).
    ///
    /// They live on the instance's own record, as its GUI session access does:
    /// a recorded host key is the service's record of THAT instance, so
    /// deleting the instance takes its recorded host keys with it.
    ///
    /// An instance's record starts empty — nothing is recorded until the
    /// browser-based access path witnesses the instance's key — and
    /// `DeleteKnownHostKeys` is what empties it again, which is how a caller
    /// recovers from a host key mismatch.
    pub known_host_keys: Vec<HostKeyAttributes>,
    /// The temporary access `GetInstanceAccessDetails` has granted FOR this
    /// instance: the live temporary SSH key pair, and the Windows Administrator
    /// password the service holds for it.
    ///
    /// On the instance's own record, as its GUI session access and its recorded
    /// host keys are: access is access to THAT instance, so deleting the
    /// instance takes every grant made for it with it.
    pub access: StoredInstanceAccess,
}

/// The state of an instance's metadata options while the service is still
/// putting a new selection in place: "`pending` - The metadata options are being
/// updated. The instance is not yet ready to process metadata traffic with the
/// new selection" (the modelled `InstanceMetadataOptions$state`).
pub const INSTANCE_METADATA_STATE_PENDING: &str = "pending";
/// The state once the selection is in place: "`applied` - The metadata options
/// have been successfully applied to the instance" (same member).
pub const INSTANCE_METADATA_STATE_APPLIED: &str = "applied";

/// The `HttpTokens` value a metadata request needs no signed token under, and
/// the value this setting takes when a request names none:
/// "If the parameter is not specified in the request, the default state is
/// `optional`" (the modelled `UpdateInstanceMetadataOptionsRequest$httpTokens`).
pub const HTTP_TOKENS_OPTIONAL: &str = "optional";

/// The `HttpEndpoint` value an instance's metadata endpoint answers on, which is
/// how Lightsail creates an instance: the endpoint serves, and
/// `UpdateInstanceMetadataOptions` is what turns it off.
pub const HTTP_ENDPOINT_ENABLED: &str = "enabled";

/// The `HttpProtocolIpv6` values the metadata service's IPv6 endpoint takes.
pub const HTTP_PROTOCOL_IPV6_ENABLED: &str = "enabled";
/// See [`HTTP_PROTOCOL_IPV6_ENABLED`].
pub const HTTP_PROTOCOL_IPV6_DISABLED: &str = "disabled";

/// The one Region whose instances have an IPv6 endpoint on their metadata
/// service at all, from that setting's own documented availability note
/// (`smithy_query(query="doc",shape="UpdateInstanceMetadataOptionsRequest",member="httpProtocolIpv6")`):
/// "This parameter is available only for instances in the Europe (Stockholm)
/// Amazon Web Services Region (eu-north-1)."
pub const IPV6_METADATA_ENDPOINT_REGION_NAME: &str = "eu-north-1";

/// The refusal a request that sets `httpProtocolIpv6` on an instance outside
/// [`IPV6_METADATA_ENDPOINT_REGION_NAME`] is answered with, as that member's own
/// documentation states the restriction.
pub const IPV6_METADATA_ENDPOINT_REGION_ONLY_MESSAGE: &str =
    "The httpProtocolIpv6 parameter is available only for instances in the Europe (Stockholm) \
     Amazon Web Services Region (eu-north-1).";

/// Whether an instance in `region_name` has the metadata service's IPv6 endpoint
/// to enable or disable.
///
/// The crate's ONE owner of that setting's documented Region restriction, asked
/// both by the operation that sets the member and by the projection that reports
/// it, so the restriction is decided in a single place.
pub fn region_has_ipv6_metadata_endpoint(region_name: &str) -> bool {
    region_name == IPV6_METADATA_ENDPOINT_REGION_NAME
}

/// The refusal for a request that sets `httpProtocolIpv6` on an instance in
/// `region_name`, and `None` when that Region's instances have the endpoint the
/// member configures.
pub fn ipv6_metadata_region_refusal(region_name: &str) -> Option<String> {
    (!region_has_ipv6_metadata_endpoint(region_name))
        .then(|| IPV6_METADATA_ENDPOINT_REGION_ONLY_MESSAGE.to_string())
}

/// The hop limit an instance's metadata service is created with: a metadata
/// response reaches the instance itself and goes no further, which is the one
/// hop a request from the instance needs.
pub const DEFAULT_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 1;

/// The smallest hop limit the metadata service accepts.
///
/// The hop limit is the IP time-to-live of the metadata service's PUT response,
/// so a response has to be allowed at least the ONE hop that reaches the
/// requesting instance; a limit of zero would let no response arrive at all.
pub const MIN_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 1;

/// The largest hop limit the metadata service accepts — the ceiling the instance
/// metadata service publishes for this setting, "Possible values: Integers from
/// 1 to 64" (the EC2 instance metadata service's own
/// `HttpPutResponseHopLimit`, which a Lightsail instance's metadata service is).
pub const MAX_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 64;

/// One instance's metadata service configuration: the values
/// `UpdateInstanceMetadataOptions` sets, and the instant the clock owes the
/// instance a selection that is still being put in place.
#[derive(Debug, Clone)]
pub struct StoredMetadataOptions {
    pub http_tokens: String,
    pub http_endpoint: String,
    pub http_put_response_hop_limit: i32,
    /// What a caller has set the IPv6 metadata endpoint to, and `None` while no
    /// caller has set it: an instance nobody has configured reaches that
    /// endpoint exactly when it has IPv6 addressing at all, so the value
    /// reported then follows the instance's own address type.
    pub http_protocol_ipv6: Option<String>,
    /// The instant the selection now being put in place is applied, while one
    /// is: until then the options report `pending`, and from then on `applied`.
    pub applied_at_ms: Option<i64>,
}

impl Default for StoredMetadataOptions {
    /// The configuration Lightsail creates an instance's metadata service with:
    /// the endpoint serving, no signed token required, and the one hop that
    /// reaches the instance. Nothing is in flight, so it is `applied` already.
    fn default() -> Self {
        Self {
            http_tokens: HTTP_TOKENS_OPTIONAL.to_string(),
            http_endpoint: HTTP_ENDPOINT_ENABLED.to_string(),
            http_put_response_hop_limit: DEFAULT_HTTP_PUT_RESPONSE_HOP_LIMIT,
            http_protocol_ipv6: None,
            applied_at_ms: None,
        }
    }
}

impl StoredMetadataOptions {
    /// This configuration as the modelled `InstanceMetadataOptions`, for an
    /// instance whose own addressing is `ip_address_type` and which lives in
    /// `region_name`.
    ///
    /// `state` is read from what is outstanding rather than stored twice: a
    /// selection the clock still owes the instance is `pending`, and one it has
    /// nothing left to do for is `applied`.
    ///
    /// `httpProtocolIpv6` is the metadata service's IPv6 endpoint, which only an
    /// instance in [`IPV6_METADATA_ENDPOINT_REGION_NAME`] has at all, so an
    /// instance anywhere else reports it disabled whatever its own addressing:
    /// there is no such endpoint there to reach.
    pub fn view(&self, ip_address_type: &str, region_name: &str) -> InstanceMetadataOptions {
        InstanceMetadataOptions {
            state: Some(
                if self.applied_at_ms.is_some() {
                    INSTANCE_METADATA_STATE_PENDING
                } else {
                    INSTANCE_METADATA_STATE_APPLIED
                }
                .to_string(),
            ),
            http_tokens: Some(self.http_tokens.clone()),
            http_endpoint: Some(self.http_endpoint.clone()),
            http_put_response_hop_limit: Some(self.http_put_response_hop_limit),
            http_protocol_ipv6: Some(if !region_has_ipv6_metadata_endpoint(region_name) {
                HTTP_PROTOCOL_IPV6_DISABLED.to_string()
            } else {
                self.http_protocol_ipv6.clone().unwrap_or_else(|| {
                    if ip_address_type_has_ipv6(ip_address_type) {
                        HTTP_PROTOCOL_IPV6_ENABLED.to_string()
                    } else {
                        HTTP_PROTOCOL_IPV6_DISABLED.to_string()
                    }
                })
            }),
        }
    }
}

/// The device path an instance's system disk is exposed at.
///
/// A Lightsail instance is an EC2 instance whose root volume is the first device
/// of the machine, which is `/dev/xvda` on the Xen-style device naming an EC2
/// Linux instance uses. It is reported as the system disk's `path`, and it is
/// therefore a path no block storage disk can be attached at.
pub const SYSTEM_DISK_PATH: &str = "/dev/xvda";

impl StoredInstance {
    /// This record as the modelled `Instance`.
    ///
    /// `attached_disks` are the block storage disks the store has attached to
    /// this instance, which `hardware.disks` reports after the system disk. They
    /// are passed in because an attachment lives on the DISK record (the disk
    /// names the instance it is attached to), so only the store can resolve
    /// them — see [`ComAmazonawsLightsailLightsail20161128State::instance_view`].
    ///
    /// `next_auto_snapshot_time_of_day` is when the next automatic copy of this
    /// instance is due, which lives on the SCHEDULE its add-on runs on rather
    /// than on this record, so it too is passed in by the store
    /// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`]).
    pub fn view(
        &self,
        attached_disks: Vec<crate::ops::types::Disk>,
        next_auto_snapshot_time_of_day: Option<String>,
    ) -> Instance {
        Instance {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_INSTANCE.to_string()),
            tags: Some(self.tags.clone()),
            blueprint_id: Some(self.blueprint_id.clone()),
            blueprint_name: Some(blueprint_display_name(&self.blueprint_id)),
            bundle_id: Some(self.bundle_id.clone()),
            add_ons: Some(add_ons_view(&self.add_ons, next_auto_snapshot_time_of_day)),
            is_static_ip: Some(self.is_static_ip),
            private_ip_address: self.private_ip_address.clone(),
            public_ip_address: self.public_ip_address.clone(),
            ipv6_addresses: Some(self.ipv6_addresses.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            hardware: Some(InstanceHardware {
                cpu_count: Some(self.bundle.cpu_count),
                ram_size_in_gb: Some(self.bundle.ram_size_in_gb),
                // The bundle's system disk first, then every block storage disk
                // `AttachDisk` has attached to this instance: an instance's
                // hardware is the volumes it actually has, so a disk attached to
                // it is reported here for as long as the attachment stands.
                disks: Some(
                    std::iter::once(self.system_disk())
                        .chain(attached_disks)
                        .collect(),
                ),
            }),
            networking: Some(InstanceNetworking {
                monthly_transfer: Some(MonthlyTransfer {
                    gb_per_month_allocated: Some(self.bundle.transfer_gb_per_month),
                }),
                ports: Some(self.ports.iter().map(StoredPortRule::port_info_view).collect()),
            }),
            state: Some(InstanceState {
                code: Some(self.state.code),
                name: Some(self.state.name.to_string()),
            }),
            username: blueprint_username(&self.blueprint_id).map(str::to_string),
            ssh_key_name: Some(self.ssh_key_name.clone()),
            metadata_options: Some(
                self.metadata_options
                    .view(&self.ip_address_type, &self.location.region_name),
            ),
        }
    }

    /// Open `rule` on this instance's firewall — or refuse, when that would take
    /// the firewall past [`MAX_PORT_RULES`].
    ///
    /// A rule is identified by its port range and protocol, so opening one that
    /// is already open REPLACES the addresses allowed through it rather than
    /// holding the same ports twice: "Opens ports for a specific Amazon
    /// Lightsail instance, and specifies the IP addresses allowed to connect to
    /// the instance through the ports, and the protocol."
    pub fn open_port(&mut self, rule: StoredPortRule) -> Result<(), String> {
        match self
            .ports
            .iter_mut()
            .find(|stored| stored.is_same_rule(&rule))
        {
            Some(stored) => *stored = rule,
            None => {
                if self.ports.len() >= MAX_PORT_RULES {
                    return Err(format!(
                        "the firewall of instance '{}' already holds {MAX_PORT_RULES} rules, \
                         the most this emulator keeps",
                        self.name
                    ));
                }
                self.ports.push(rule);
            }
        }
        self.sort_ports();
        Ok(())
    }

    /// Close the rule `from_port`/`to_port`/`protocol` names, answering whether
    /// the firewall held it.
    ///
    /// Closing a port that is not open leaves the firewall exactly as it is: the
    /// request's goal — that those ports are not open — already holds.
    pub fn close_port(&mut self, from_port: i32, to_port: i32, protocol: &str) -> bool {
        let before = self.ports.len();
        self.ports
            .retain(|stored| stored.key() != (from_port, to_port, protocol));
        self.ports.len() != before
    }

    /// Replace this instance's whole firewall with `rules` — or refuse, when the
    /// RULES they collapse to are past [`MAX_PORT_RULES`] (entries naming one
    /// rule are one rule, so the bound is on what the firewall would hold).
    ///
    /// This is `PutInstancePublicPorts`' own semantics: "This action also closes
    /// all currently open ports that are not included in the request." Two
    /// entries that name the same ports and protocol are the same rule, so the
    /// last of them is what the firewall holds.
    ///
    /// PRECONDITION: `rules` is already bounded by the caller — the handler
    /// refuses a request carrying more than [`MAX_PORT_INFOS_PER_REQUEST`]
    /// entries before building any of them — so the collapse below allocates a
    /// bounded amount however large a request was sent.
    pub fn replace_ports(&mut self, rules: Vec<StoredPortRule>) -> Result<(), String> {
        debug_assert!(
            rules.len() <= MAX_PORT_INFOS_PER_REQUEST,
            "replace_ports was handed {} rules, past the {MAX_PORT_INFOS_PER_REQUEST} \
             entries one request may carry",
            rules.len()
        );
        // Two entries that name the same ports and protocol are the same rule,
        // so they are collapsed BEFORE the bound is applied: the guard measures
        // what the firewall would actually hold, not how many entries the
        // request wrote it with. The collapse is keyed, not scanned, so the work
        // is linear in the entries however many there are.
        let mut collapsed: HashMap<(i32, i32, String), StoredPortRule> = HashMap::new();
        for rule in rules {
            collapsed.insert(
                (rule.from_port, rule.to_port, rule.protocol.clone()),
                rule,
            );
        }
        if collapsed.len() > MAX_PORT_RULES {
            return Err(format!(
                "portInfos names {} rules, more than the {MAX_PORT_RULES} this emulator \
                 keeps on one firewall",
                collapsed.len()
            ));
        }
        self.ports = collapsed.into_values().collect();
        self.sort_ports();
        Ok(())
    }

    /// Keep the firewall in one total order — by port range, then protocol — so
    /// every read reports its rules in the same sequence however they were
    /// opened.
    pub(super) fn sort_ports(&mut self) {
        self.ports.sort_by(|left, right| {
            (left.from_port, left.to_port, left.protocol.as_str()).cmp(&(
                right.from_port,
                right.to_port,
                right.protocol.as_str(),
            ))
        });
    }

    /// The instance's system disk: the bundle's SSD, mounted at the root and
    /// attached for as long as the instance exists.
    ///
    /// The system disk is not a Lightsail `Disk` resource of its own — nothing
    /// created it and no name or ARN addresses it — so it reports only what it
    /// has: its size, its path, and the instance it is attached to.
    /// `attachmentState` is the discontinued member the modelled docs say is
    /// "still included in the response", returning `attached` for a system disk.
    pub(super) fn system_disk(&self) -> crate::ops::types::Disk {
        crate::ops::types::Disk {
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            size_in_gb: Some(self.bundle.disk_size_in_gb),
            is_system_disk: Some(true),
            path: Some(SYSTEM_DISK_PATH.to_string()),
            // `DiskState` enum: a disk an instance is using is `in-use`.
            state: Some("in-use".to_string()),
            attached_to: Some(self.name.clone()),
            is_attached: Some(true),
            attachment_state: Some("attached".to_string()),
            ..Default::default()
        }
    }

    /// Put `selection` in place on this instance's metadata service, answering
    /// the instant the clock owes the instance the applied options — `None` when
    /// they are applied already.
    ///
    /// A RUNNING instance is serving metadata traffic under the options it has,
    /// so a new selection is `pending` until the service has it in place: "When
    /// you modify the parameters on a running instance, the GetInstance or
    /// GetInstances API operation initially responds with a state of pending.
    /// After the parameter modifications are successfully applied, the state
    /// changes to applied" (the modelled `UpdateInstanceMetadataOptions`
    /// documentation). An instance that is not running is processing no metadata
    /// traffic at all, so there is nothing to put in place: the options it will
    /// serve under are already the new ones.
    ///
    /// The only two states a selection is ever put in place from are `running`
    /// and `stopped` — the operation that calls this refuses an instance in any
    /// other state — so the instance is serving metadata traffic or it is down,
    /// and never one the clock is still moving.
    pub fn apply_metadata_options(
        &mut self,
        selection: MetadataOptionsSelection,
        now_ms: i64,
    ) -> Option<i64> {
        // "If the parameter is not specified in the request, the default state is
        // `optional`" — the one member of this input whose absence is a value
        // rather than "the existing state is maintained".
        self.metadata_options.http_tokens = selection
            .http_tokens
            .unwrap_or_else(|| HTTP_TOKENS_OPTIONAL.to_string());
        if let Some(http_endpoint) = selection.http_endpoint {
            self.metadata_options.http_endpoint = http_endpoint;
        }
        if let Some(hop_limit) = selection.http_put_response_hop_limit {
            self.metadata_options.http_put_response_hop_limit = hop_limit;
        }
        if let Some(http_protocol_ipv6) = selection.http_protocol_ipv6 {
            self.metadata_options.http_protocol_ipv6 = Some(http_protocol_ipv6);
        }
        let applied_at_ms = (self.state == RUNNING)
            .then(|| now_ms.saturating_add(STATE_TRANSITION_MILLIS));
        self.metadata_options.applied_at_ms = applied_at_ms;
        applied_at_ms
    }
}

/// What one `UpdateInstanceMetadataOptions` request asks of an instance's
/// metadata service: a member it carries is what that setting becomes, and one
/// it leaves out is the caller naming no value for it.
#[derive(Debug, Clone, Default)]
pub struct MetadataOptionsSelection {
    pub http_tokens: Option<String>,
    pub http_endpoint: Option<String>,
    pub http_put_response_hop_limit: Option<i32>,
    pub http_protocol_ipv6: Option<String>,
}
