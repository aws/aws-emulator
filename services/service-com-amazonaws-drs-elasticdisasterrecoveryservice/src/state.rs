// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-drs-elasticdisasterrecoveryservice service.

use std::collections::{BTreeMap, HashMap};

pub mod ec2_subnets;
pub mod launch_actions;
pub mod launch_configuration_templates;
pub mod recovery_plans;
pub mod tags;

use crate::ops::types::{
    CreateReplicationConfigurationTemplateOutput, DisconnectSourceServerOutput,
    IdentificationHints, NetworkInterface, PITPolicyRule, ResourceNotFoundException,
    SourceProperties, StagingArea, UpdateReplicationConfigurationTemplateOutput,
    ValidationException,
};

/// Per-service state for com-amazonaws-drs-elasticdisasterrecoveryservice. A handler reaches it through [`service_state`].
///
/// The emulated account is onboarded to this service for as long as the emulator is
/// up, so there is no initialization flag here and no operation refuses a request as
/// an uninitialized account: `InitializeService` has nothing left to switch on (see
/// `crate::ops::initialize_service`).
#[derive(Debug, Default)]
pub struct ComAmazonawsDrsElasticdisasterrecoveryserviceState {
    /// Replication configuration templates, keyed by `replicationConfigurationTemplateID`.
    ///
    /// A [`BTreeMap`] rather than a `HashMap` because both `Describe*` operations
    /// are `@paginated`: a page boundary is only meaningful over a stable order.
    /// The ids this service mints ascend with creation (see [`Self::next_id`]), so
    /// the map's key order IS creation order.
    pub replication_configuration_templates:
        BTreeMap<String, ReplicationConfigurationTemplateRecord>,
    /// Recovery Plans, keyed by `recoveryPlanID`.
    ///
    /// A [`BTreeMap`] for the same reason as the templates above: `ListRecoveryPlans`
    /// is `@paginated`, and the ids this service mints ascend with creation (see
    /// [`Self::next_id`]), so the map's key order IS creation order.
    pub recovery_plans: BTreeMap<String, recovery_plans::RecoveryPlanRecord>,
    /// Launch Configuration Templates, keyed by `launchConfigurationTemplateID`.
    ///
    /// A [`BTreeMap`] for the same reason as the templates above:
    /// `DescribeLaunchConfigurationTemplates` is `@paginated`, and the ids this service
    /// mints ascend with creation (see [`Self::next_id`]), so the map's key order IS
    /// creation order.
    pub launch_configuration_templates:
        BTreeMap<String, launch_configuration_templates::LaunchConfigurationTemplateRecord>,
    /// The launch actions each resource carries, keyed by the `resourceId` they hang
    /// off (a Launch Configuration Template id or a Source Server id) and, within one
    /// resource, by the `actionId` the caller chose.
    ///
    /// A launch action is not a resource of its own: it belongs to the resource it was
    /// put on, so a resource with no action at all simply has no entry here (see
    /// `crate::state::launch_actions`).
    pub launch_actions: BTreeMap<String, launch_actions::LaunchActions>,
    /// Source servers, keyed by `sourceServerID`. Registered by the replication
    /// agent, which this emulator does not model yet, so the map is empty — but
    /// `DescribeSourceServers` reads it rather than returning a canned empty list.
    pub source_servers: BTreeMap<String, SourceServerRecord>,
    /// Mints the suffix of every id this service hands out (see [`Self::next_id`]).
    pub id_counter: u64,
}

/// A stored replication configuration template: every member
/// `CreateReplicationConfigurationTemplate` accepts, plus the two the service
/// assigns (`id` and `arn`).
#[derive(Debug, Clone, Default)]
pub struct ReplicationConfigurationTemplateRecord {
    pub id: String,
    pub arn: String,
    pub staging_area_subnet_id: String,
    pub associate_default_security_group: Option<bool>,
    pub replication_servers_security_groups_ids: Vec<String>,
    pub replication_server_instance_type: Option<String>,
    pub use_dedicated_replication_server: Option<bool>,
    pub default_large_staging_disk_type: Option<String>,
    pub ebs_encryption: String,
    pub ebs_encryption_key_arn: Option<String>,
    pub bandwidth_throttling: i64,
    pub data_plane_routing: Option<String>,
    pub create_public_ip: Option<bool>,
    pub staging_area_tags: HashMap<String, String>,
    /// A template always carries a tag set, so a create that named none stores an
    /// empty one: the service reports this member whether or not a client sent it.
    pub tags: HashMap<String, String>,
    pub pit_policy: Vec<PITPolicyRule>,
    /// A template always carries this setting — a create that named none stores the
    /// service's default, [`AUTO_REPLICATE_NEW_DISKS_DEFAULT`] — so it too is always
    /// reported.
    pub auto_replicate_new_disks: bool,
    pub internet_protocol: Option<String>,
}

/// A stored source server. It keeps the members this slice can compute plus the
/// ones `DescribeSourceServers`' filters match on; the rest of the agent-reported
/// state (lifecycle, replication progress, disks) belongs to the operations that
/// will register a server.
#[derive(Debug, Clone, Default)]
pub struct SourceServerRecord {
    pub source_server_id: String,
    pub arn: String,
    pub tags: HashMap<String, String>,
    /// What the `hardwareId` filter matches on, together with
    /// [`Self::network_interfaces`]: the EC2 instance id or the VMware UUID.
    pub identification_hints: Option<IdentificationHints>,
    /// Also matched by the `hardwareId` filter, through each interface's MAC address.
    pub network_interfaces: Vec<NetworkInterface>,
    /// What the `stagingAccountIDs` filter matches on.
    pub staging_account_id: Option<String>,
}

impl ReplicationConfigurationTemplateRecord {
    /// The stored record as the modelled `ReplicationConfigurationTemplate`
    /// structure — the output shape of `CreateReplicationConfigurationTemplate`
    /// and the item shape of `DescribeReplicationConfigurationTemplates`.
    ///
    /// The ONE place a stored template becomes a wire template, so a created, a
    /// described and an updated template cannot disagree about any member.
    pub fn to_wire(&self) -> CreateReplicationConfigurationTemplateOutput {
        CreateReplicationConfigurationTemplateOutput {
            replication_configuration_template_id: self.id.clone(),
            arn: Some(self.arn.clone()),
            staging_area_subnet_id: Some(self.staging_area_subnet_id.clone()),
            associate_default_security_group: self.associate_default_security_group,
            replication_servers_security_groups_i_ds: Some(
                self.replication_servers_security_groups_ids.clone(),
            ),
            replication_server_instance_type: self.replication_server_instance_type.clone(),
            use_dedicated_replication_server: self.use_dedicated_replication_server,
            default_large_staging_disk_type: self.default_large_staging_disk_type.clone(),
            ebs_encryption: Some(self.ebs_encryption.clone()),
            ebs_encryption_key_arn: self.ebs_encryption_key_arn.clone(),
            bandwidth_throttling: self.bandwidth_throttling,
            data_plane_routing: self.data_plane_routing.clone(),
            create_public_ip: self.create_public_ip,
            staging_area_tags: Some(self.staging_area_tags.clone()),
            tags: Some(self.tags.clone()),
            pit_policy: Some(self.pit_policy.clone()),
            auto_replicate_new_disks: Some(self.auto_replicate_new_disks),
            internet_protocol: self.internet_protocol.clone(),
        }
    }

    /// The same wire template, in the distinct Rust type the generator emits for
    /// `UpdateReplicationConfigurationTemplate`'s output. Both types render the one
    /// `com.amazonaws.drs#ReplicationConfigurationTemplate` shape, so this is a
    /// field-for-field re-wrap of [`Self::to_wire`] rather than a second mapping.
    pub fn to_update_wire(&self) -> UpdateReplicationConfigurationTemplateOutput {
        let wire = self.to_wire();
        UpdateReplicationConfigurationTemplateOutput {
            replication_configuration_template_id: wire.replication_configuration_template_id,
            arn: wire.arn,
            staging_area_subnet_id: wire.staging_area_subnet_id,
            associate_default_security_group: wire.associate_default_security_group,
            replication_servers_security_groups_i_ds: wire
                .replication_servers_security_groups_i_ds,
            replication_server_instance_type: wire.replication_server_instance_type,
            use_dedicated_replication_server: wire.use_dedicated_replication_server,
            default_large_staging_disk_type: wire.default_large_staging_disk_type,
            ebs_encryption: wire.ebs_encryption,
            ebs_encryption_key_arn: wire.ebs_encryption_key_arn,
            bandwidth_throttling: wire.bandwidth_throttling,
            data_plane_routing: wire.data_plane_routing,
            create_public_ip: wire.create_public_ip,
            staging_area_tags: wire.staging_area_tags,
            tags: wire.tags,
            pit_policy: wire.pit_policy,
            auto_replicate_new_disks: wire.auto_replicate_new_disks,
            internet_protocol: wire.internet_protocol,
        }
    }
}

/// The wire shape of a source server: the modelled
/// `com.amazonaws.drs#SourceServer` structure, which the generator renders under
/// the name of the first operation that returns it (`DisconnectSourceServer`).
/// The alias says what the type IS, so a reader of `DescribeSourceServers` does
/// not take its items for the result of a disconnect.
pub type SourceServerWire = DisconnectSourceServerOutput;

impl SourceServerRecord {
    /// The stored source server as the modelled `SourceServer` structure — the item
    /// shape of `DescribeSourceServers`.
    pub fn to_wire(&self) -> SourceServerWire {
        DisconnectSourceServerOutput {
            source_server_id: Some(self.source_server_id.clone()),
            arn: Some(self.arn.clone()),
            tags: Some(self.tags.clone()),
            source_properties: Some(SourceProperties {
                identification_hints: self.identification_hints.clone(),
                network_interfaces: Some(self.network_interfaces.clone()),
                ..SourceProperties::default()
            }),
            staging_area: self.staging_account_id.clone().map(|id| StagingArea {
                staging_account_id: Some(id),
                ..StagingArea::default()
            }),
            ..DisconnectSourceServerOutput::default()
        }
    }

    /// The hardware identifiers the `hardwareId` filter of `DescribeSourceServers`
    /// matches a server by: its EC2 instance id, its VMware UUID, and the MAC
    /// address of each of its network interfaces.
    pub fn hardware_ids(&self) -> Vec<String> {
        let hints = self.identification_hints.iter().flat_map(|hints| {
            [hints.aws_instance_id.clone(), hints.vm_ware_uuid.clone()]
                .into_iter()
                .flatten()
        });
        hints
            .chain(
                self.network_interfaces
                    .iter()
                    .filter_map(|nic| nic.mac_address.clone()),
            )
            .collect()
    }
}

/// The characters a DRS resource id suffix is drawn from — exactly the class the
/// model's id patterns allow (`^rct-[0-9a-zA-Z]{17}$`), listed in ASCII order.
///
/// ASCII order matters: a fixed-width rendering over an ASCII-ordered alphabet
/// sorts the same way as the number it renders, which is what makes a store keyed
/// by these ids iterate in creation order (see
/// [`ComAmazonawsDrsElasticdisasterrecoveryserviceState::next_id`]).
const ID_ALPHABET: [u8; 62] = *b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Characters after the `<prefix>-`, per the model's 21-character
/// `ReplicationConfigurationTemplateID` (`rct-` + 17).
const ID_SUFFIX_LEN: usize = 17;

/// The region used when a request carries no SigV4 credential scope to read one
/// from — the default `emulator_arn` documents for generated emulators.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page either `Describe*` operation serves, and the page size it uses
/// when the request names no `maxResults`.
///
/// `maxResults` is modelled `@range(min: 1)` with NO maximum, so the model leaves
/// the upper bound to the service: this is the emulator's OWN cap, not a claim
/// about an AWS page size. It bounds one response regardless of how large the
/// store is — anything past it is reached through `nextToken`, which is what the
/// `@paginated` contract is for.
pub const PAGE_LIMIT: i32 = 100;

/// How many replication configuration templates the store holds before
/// `CreateReplicationConfigurationTemplate` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any account's
/// AWS quota (Service Quotas is not modelled here). It is set far above any
/// workflow an emulator serves, and it is what stops a caller growing the state
/// without end.
pub const REPLICATION_CONFIGURATION_TEMPLATE_QUOTA: usize = 1_000;

/// The `serviceCode` a quota refusal reports: this service's ARN namespace and
/// SigV4 signing name.
pub const SERVICE_CODE: &str = "drs";

/// The `reason` this service reports when a request's fields do not hold up.
///
/// The model's `ValidationExceptionReason` lists camelCase values (see
/// `VALIDATION_EXCEPTION_REASON_VALUES` in `crate::ops::types`), but DRS answers this
/// member in SCREAMING_SNAKE_CASE, so that is what a client branching on the reason
/// reads off the wire.
pub const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The modelled `ValidationException` (HTTP 400) for a request this service refuses on
/// one of its fields, carrying `message` and that reason.
///
/// The ONE place such a refusal is built, so every operation reports the same reason
/// for the same class of failure.
pub fn field_validation_failed(message: String) -> ValidationException {
    ValidationException {
        message: Some(message),
        code: None,
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: None,
    }
}

/// Whether a replication configuration template replicates newly added disks when the
/// create that minted it named no `autoReplicateNewDisks`: the service's own default
/// for that setting, which the created template then reports and every later read
/// reports with it.
pub const AUTO_REPLICATE_NEW_DISKS_DEFAULT: bool = true;

/// How this service names the replication-configuration-template resource when it
/// reports one in an error: the `resourceType` of a not-found refusal, and the
/// subject of its message.
pub const REPLICATION_CONFIGURATION_TEMPLATE_RESOURCE_TYPE: &str =
    "Replication Configuration Template";

/// The modelled `ResourceNotFoundException` for a replication configuration template
/// this store does not hold.
///
/// The ONE place that refusal is built, so every id-keyed template operation reports
/// the same `resourceType`, the same `resourceId` (the id the request named, verbatim)
/// and the same message: the service's name for the resource type, then the id it was
/// asked for, then that it was not found.
pub fn replication_configuration_template_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{REPLICATION_CONFIGURATION_TEMPLATE_RESOURCE_TYPE} with Id: '{id}' was not found"
        )),
        code: None,
        resource_id: Some(id.to_string()),
        resource_type: Some(REPLICATION_CONFIGURATION_TEMPLATE_RESOURCE_TYPE.to_string()),
    }
}

impl ComAmazonawsDrsElasticdisasterrecoveryserviceState {
    /// Mint a fresh id of the form `<prefix>-<17 base-62 characters>`.
    ///
    /// The suffix is the base-62 rendering of `BASE + counter * STRIDE`. That map
    /// is strictly increasing in the counter, so no two calls can collide AND a
    /// later id sorts after an earlier one — the order a store keyed by these ids
    /// pages in. `BASE` keeps the rendering a full 17 characters wide, so every
    /// character is significant (never a run of filler zeroes) and the widths
    /// cannot differ, which is what lets the comparison stay numeric.
    pub fn next_id(&mut self, prefix: &str) -> String {
        /// Keeps the rendering a full 17 characters wide.
        const BASE: u128 = 62u128.pow(16);
        /// Spaces the ids out, so consecutive calls do not hand out ids differing
        /// in one character. Uniqueness does not rest on this: it follows from
        /// `BASE + counter * STRIDE` being strictly increasing within the width the
        /// `debug_assert` below guards.
        const STRIDE: u128 = 1_000_000_007;

        let mut value = BASE + u128::from(self.id_counter) * STRIDE;
        debug_assert!(
            value < 62u128.pow(17),
            "the id counter has run past the 17-character width the model allows"
        );
        self.id_counter += 1;

        let mut suffix = ['0'; ID_SUFFIX_LEN];
        for slot in suffix.iter_mut().rev() {
            *slot = char::from(ID_ALPHABET[(value % 62) as usize]);
            value /= 62;
        }
        let suffix: String = suffix.iter().collect();
        format!("{prefix}-{suffix}")
    }
}

/// The region this request is signed for, falling back to [`DEFAULT_REGION`].
///
/// A resource's ARN carries the region the caller addressed, so the ARN the
/// emulator reports is the one an SDK client configured for that region expects.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a replication configuration template:
/// `arn:aws:drs:<region>:<account>:replication-configuration-template/<id>`.
pub fn replication_configuration_template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "drs",
        region,
        &format!(
            "{}{id}",
            tags::REPLICATION_CONFIGURATION_TEMPLATE_ARN_PREFIX
        ),
    )
}

/// One page of a `@paginated` operation's `items`, plus the token that resumes
/// after it (absent on the last page).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The number of items one page serves.
///
/// `maxResults` is modelled `@range(min: 1)` with no maximum and the boundary
/// validator refused anything below 1 before a handler ran, so what is left is to
/// cap the caller's request — and to pick the page size for a request that names
/// none — at [`PAGE_LIMIT`]. A response is therefore bounded however large the
/// store is, and however large a number the caller asks for.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT).clamp(1, PAGE_LIMIT);
    usize::try_from(requested).expect("a page size in 1..=PAGE_LIMIT fits in usize")
}

/// Cut `all` (an id-keyed, ordered sequence of stored records) into the page
/// `max_results` and `next_token` name, rendering ONLY that page's records to the
/// wire shape through `to_item`.
///
/// Nothing outside the page is cloned, and the page itself is at most
/// [`PAGE_LIMIT`] items (see [`page_size`]), so neither the work done under the
/// state lock nor the size of the response grows with the store.
///
/// The token records the id the NEXT page starts at rather than an offset, so a
/// resource added or removed between pages cannot make a page skip an item. `kind`
/// namespaces the token so a token minted by one paginated operation is not
/// accepted by another. On a token that does not decode, or that names an id the
/// collection no longer holds, this answers `Err(message)` for the caller to
/// render as the operation's `ValidationException`.
pub fn paginate<'a, R: 'a, T>(
    kind: &str,
    all: impl IntoIterator<Item = (&'a str, &'a R)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    to_item: impl Fn(&R) -> T,
) -> Result<Page<T>, String> {
    let resume = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(kind, token)
                .ok_or_else(|| "nextToken is not a valid pagination token".to_string())?,
        ),
    };

    let mut rest = all.into_iter().peekable();
    if let Some(resume) = resume {
        // Walk up TO the item the token names, never past it: the token says where
        // the next page starts. An id the collection no longer holds is refused
        // rather than silently restarting the walk.
        loop {
            match rest.peek() {
                Some((id, _)) if *id == resume.as_str() => break,
                Some(_) => {
                    rest.next();
                }
                None => {
                    return Err("nextToken refers to a page that no longer exists".to_string());
                }
            }
        }
    }

    let limit = page_size(max_results);
    let mut items = Vec::new();
    while items.len() < limit {
        match rest.next() {
            Some((_, record)) => items.push(to_item(record)),
            None => break,
        }
    }
    // Peeking one past the page is what decides whether there IS another page:
    // a token is minted only when an item actually follows.
    let next_token = rest.peek().map(|(id, _)| encode_page_token(kind, id));

    Ok(Page { items, next_token })
}

/// Mint the opaque pagination token that resumes at `start_id`.
///
/// Encoded with `aws_smithy_types::base64` — the codec the wire protocols already
/// carry, and the one the sibling service crates use for their page tokens — so a
/// client sees an opaque string rather than an id it might try to construct.
fn encode_page_token(kind: &str, start_id: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{start_id}"))
}

/// Read back a token minted by [`encode_page_token`] for the same `kind`.
fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("{kind}:"))
        .map(std::string::ToString::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDrsElasticdisasterrecoveryserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDrsElasticdisasterrecoveryserviceState>)
}
