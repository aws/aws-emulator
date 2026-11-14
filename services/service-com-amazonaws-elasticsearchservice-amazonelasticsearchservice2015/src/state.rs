// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015 service.
//!
//! The service's primary resource is the Elasticsearch **domain**. One
//! [`Domain`] record is the single source of truth for every answer the domain
//! operations give: `CreateElasticsearchDomain` inserts it,
//! `UpdateElasticsearchDomainConfig` merges into it,
//! `DescribeElasticsearchDomain` / `DescribeElasticsearchDomains` /
//! `DescribeElasticsearchDomainConfig` / `ListDomainNames` read it, and
//! `DeleteElasticsearchDomain` removes it.
//!
//! Every per-option `*Status` wrapper the model declares is BUILT FROM the
//! stored [`OptionRecord`] (its creation date, its update date, the config
//! version it last changed at, and the instant it finishes provisioning), never
//! fabricated per response — so two reads of an unchanged domain agree and a
//! read never has to write.
//!
//! A domain's tag set lives on that same record: `AddTags` merges into it,
//! `ListTags` reads it and `RemoveTags` drops the keys it names, all addressed
//! by the ARN the create reported.
//!
//! A [`Package`] carries its own content history — one [`PackageVersionRecord`]
//! per revision, appended by `CreatePackage` and `UpdatePackage` — and a
//! [`DomainPackage`] records one package installed on one domain, which
//! `AssociatePackage` writes and the two list operations read. Beside the
//! packages an account creates, the service publishes a catalogue of plugin
//! packages every account sees ([`PUBLISHED_PACKAGES`]); the package reads
//! answer from the union of the two ([`ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State::visible_packages`]).
//!
//! The other resource this store holds is the outbound
//! [`CrossClusterSearchConnection`]: `CreateOutboundCrossClusterSearchConnection`
//! mints one and `DescribeOutboundCrossClusterSearchConnections` reports it. Its
//! status is COMPUTED from the stored validation outcome and the instant that
//! outcome falls due, for the same reason: reporting it must not write.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

// The service's modelled shapes. They are the stored representation as well as
// the wire one: a second, parallel set of structs here would let a describe and
// an update drift apart.
#[allow(unused_imports)]
use crate::ops::types::*;

/// The input constraints this surface applies ITSELF — the ones the boundary
/// walk in front of a handler cannot reach — and how a refusal of one is worded.
mod request_constraints;
pub use request_constraints::*;

/// The indexes and documents a domain's own cluster holds, which its REST data
/// plane ([`crate::service::domain_rest`]) reads and writes.
mod search_indexes;
pub use search_indexes::*;

/// The region an ARN or endpoint uses when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The config version `CreateElasticsearchDomain` stamps on every option it
/// stores; `UpdateElasticsearchDomainConfig` bumps it from there.
pub const FIRST_CONFIG_VERSION: i32 = 1;

/// How many domains one account may hold in one region before
/// `CreateElasticsearchDomain` answers the modelled `LimitExceededException`
/// ("an exception for trying to create more than allowed resources").
///
/// 100 is the service's documented default quota for domains per account per
/// Region. Enforcing it keeps the store's entry count bounded by a stated
/// service limit rather than by how many creates a caller issues, and makes the
/// modelled limit error reachable for callers that test their own
/// limit handling.
pub const MAX_DOMAINS_PER_REGION: usize = 100;

/// A hard ceiling on the WHOLE store, across every region.
///
/// [`MAX_DOMAINS_PER_REGION`] alone does not bound it: the region is read from
/// the request's own credential scope, so a caller that varies that field would
/// get a fresh per-region allowance each time. This cap dominates the single
/// growth site instead — at most `MAX_DOMAINS_TOTAL` records can exist however
/// many regions are addressed, and a region partition exists only while it holds
/// a record, so the number of partitions is bounded by the same number. It sits
/// well above the per-region quota, so no single region's allowance is clipped
/// by it.
pub const MAX_DOMAINS_TOTAL: usize = 1_000;

/// The largest access-policy document a domain may carry, in bytes.
///
/// AWS documents a maximum domain access policy size of 100 KB. It is also the
/// bound on the largest caller-supplied payload one stored record retains, since
/// `PolicyDocument` itself declares no length constraint for the boundary
/// validator to enforce.
pub const MAX_ACCESS_POLICY_BYTES: usize = 100 * 1024;

/// `Some(reason)` when an access-policy document is larger than the documented
/// maximum. The reason names the member and its constraint.
pub fn access_policy_refusal(policy: &str) -> Option<String> {
    (policy.len() > MAX_ACCESS_POLICY_BYTES).then(|| {
        format!(
            "AccessPolicies document of {len} bytes failed to satisfy constraint: \
             an access policy must not be larger than {max} bytes",
            len = policy.len(),
            max = MAX_ACCESS_POLICY_BYTES
        )
    })
}

/// The longest snapshot suspension `AutomatedSnapshotPauseOptions` accepts, in
/// seconds.
///
/// Straight from that member's own documentation: "Maximum suspension duration:
/// 3 days."
pub const MAX_SNAPSHOT_PAUSE_SECONDS: f64 = 3.0 * 24.0 * 60.0 * 60.0;

/// The key one domain is stored under: the region it was created in and its
/// name.
pub fn domain_key(region: &str, domain_name: &str) -> (String, String) {
    (region.to_string(), domain_name.to_string())
}

/// `Some(reason)` when a requested snapshot-suspension window cannot hold:
/// either it ends before it starts, or it is longer than the documented
/// three-day maximum. The reason names the member and its constraint, so the
/// caller can fix the request from the message alone.
///
/// A half-specified window (a start with no end, or an end with no start) is
/// stored as given: the model makes both members optional and documents no rule
/// for one without the other, so there is no bound to apply and inventing a
/// refusal here would refuse a request the service accepts.
pub fn snapshot_pause_refusal(options: &AutomatedSnapshotPauseRequestOptions) -> Option<String> {
    let start = options.start_time.as_ref()?.as_secs_f64();
    let end = options.end_time.as_ref()?.as_secs_f64();
    if end < start {
        return Some(
            "AutomatedSnapshotPauseOptions.EndTime must not be earlier than \
             AutomatedSnapshotPauseOptions.StartTime"
                .to_string(),
        );
    }
    let days = (end - start) / (24.0 * 60.0 * 60.0);
    (end - start > MAX_SNAPSHOT_PAUSE_SECONDS).then(|| {
        format!(
            "AutomatedSnapshotPauseOptions suspension window of {days:.2} days failed to satisfy \
             constraint: maximum suspension duration is 3 days"
        )
    })
}

/// How long a domain and the options a call just changed stay `Processing`
/// before the clock makes them `Active`.
///
/// The emulator CHOOSES this span: real provisioning takes minutes, which no
/// caller of an emulator can wait for. It is deliberately non-zero, so a client
/// that polls immediately after a create or an update still observes
/// `DomainStatus.Processing == true` and `OptionStatus.State == "Processing"`
/// (both modelled states) before the transition lands. Nothing sleeps for it:
/// the due instant is stored and compared against the clock on each read.
const PROVISIONING_SPAN: std::time::Duration = std::time::Duration::from_millis(500);

/// Convert a stored instant into the model's timestamp type.
fn datetime_of(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region to build this request's ARNs and endpoints from: the SigV4
/// credential scope's region, else [`DEFAULT_REGION`].
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `DomainId` — the account the domain lives in and its name, the shape AWS
/// reports (`<account id>/<domain name>`).
pub fn domain_id(domain_name: &str) -> String {
    format!("{}/{}", ::engine::emulator_account_id(), domain_name)
}

/// The domain's ARN: `arn:aws:es:<region>:<account>:domain/<name>` (the model's
/// own `es` ARN namespace, and the `CreateVpcEndpoint` `DomainArn` pattern).
pub fn domain_arn(region: &str, domain_name: &str) -> String {
    ::engine::emulator_arn("es", region, &format!("domain/{domain_name}"))
}

/// The domain's search endpoint:
/// `search-<name>-<suffix>.<region>.es.amazonaws.com`.
///
/// AWS's suffix is an opaque, per-domain token. Here it is the leading 26 hex
/// digits of a SHA-256 over the domain name and the store's monotonic sequence
/// number, so it is a valid DNS label, unique per created domain, and stable for
/// the life of that domain (every read returns the stored string).
///
/// This is the NAME AWS gives that endpoint, and the data plane serves it for a
/// client that resolves AWS's own hosts (the host-style claim of
/// [`crate::service::domain_rest`]). It is NOT what the control plane publishes:
/// a client of this emulator resolves this host nowhere, so the address it is
/// handed is the one it can reach ([`published_domain_address`]).
pub fn domain_endpoint(domain_name: &str, region: &str, seq: u64) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{domain_name}/{seq}").as_bytes());
    let hex = hex::encode(digest);
    format!(
        "search-{domain_name}-{suffix}.{region}.es.amazonaws.com",
        suffix = &hex[..26]
    )
}

/// The segment that names THIS service's data plane behind the reserved path
/// namespace every data plane of this emulator is addressed under.
///
/// The `es` endpoint prefix this model publishes, with the API version that
/// separates it from the modern Amazon OpenSearch Service model — which shares
/// the `es` signing name, endpoint prefix and ARN namespace and serves a data
/// plane of its own. The two segments are therefore disjoint, so a data-plane
/// request has exactly one claimant whichever of the two APIs created the domain
/// it names.
pub const DATA_PLANE_SEGMENT: &str = "es-2015-01-01";

/// The address `ElasticsearchDomainStatus$Endpoint` PUBLISHES for a domain's
/// data plane: the endpoint THIS emulator answers on, with the reserved path
/// that names this service's data plane and the domain behind it
/// (`<host>:<port>/_aws/es-2015-01-01/<domain-name>`).
///
/// The member is "the Elasticsearch domain endpoint that you use to submit index
/// and search requests", so a client builds its requests on the value it is
/// handed and sends them there. The host [`domain_endpoint`] spells is the name
/// AWS gives that endpoint and it resolves NOWHERE for a client of this
/// emulator: handing it out announces an address nothing answers at, so the data
/// plane the control plane says it provisioned could not be reached at all. What
/// a client can reach is the one endpoint this process serves its whole surface
/// at, which is why the domain is named in the PATH — the addressing
/// `service_common::data_plane_addressing` states for every data plane of this
/// emulator and [`crate::service::domain_rest`] resolves.
///
/// `emulator_endpoint` is that endpoint's `host[:port]`
/// ([`emulator_endpoint_authority`]).
pub fn published_domain_address(emulator_endpoint: &str, domain_name: &str) -> String {
    format!(
        "{emulator_endpoint}/{}/{DATA_PLANE_SEGMENT}/{domain_name}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The `host[:port]` of the endpoint this emulator answers on, as every URL a
/// handler hands out names: `OperationContext::endpoint`, "the base URL a later
/// request addresses… Callers send normally signed requests to it", without its
/// scheme — because what a domain publishes is an address a client appends a
/// path to, not a URL.
///
/// It is the data port's own published base, so it does not depend on which host
/// the request that read it carried: a domain created through an in-process call
/// of another service publishes the same address as one a client created itself.
///
/// The reading itself is the shared one every plane of this emulator publishes
/// its address by (`service_common::data_plane_addressing`), so this service
/// cannot drift from it.
pub fn emulator_endpoint_authority(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx)
}

/// The engine the domain runs, derived from `ElasticsearchVersion`.
///
/// `ElasticsearchVersionString`'s pattern admits three forms — `X.Y`,
/// `OpenSearch_X.Y` and `OS_X.Y` — and `EngineType` has exactly two values, so
/// the prefixed forms are OpenSearch and a bare `X.Y` is Elasticsearch. A domain
/// created without a version is an Elasticsearch domain, this service's own
/// engine.
pub fn engine_type_of(version: Option<&str>) -> &'static str {
    match version {
        Some(v) if v.starts_with("OpenSearch_") || v.starts_with("OS_") => "OpenSearch",
        _ => "Elasticsearch",
    }
}

/// Every engine version a domain of this service can run, in upgrade order:
/// the Elasticsearch versions the service still offers as an upgrade source,
/// then the OpenSearch versions a domain can be moved onto.
///
/// The model constrains only the FORM of a version string
/// (`ElasticsearchVersionString`'s pattern — `X.Y`, `OpenSearch_X.Y`, `OS_X.Y`),
/// never which versions exist, so the supported set is named here ONCE and
/// every version answer is derived from it rather than from a per-operation
/// list. It is the inventory the service CURRENTLY offers: the Elasticsearch
/// 6.8/7.x line and the OpenSearch 1.x/2.x/3.x line, the engines a domain can
/// be created on or upgraded to. A version retired from that inventory is no
/// longer a source or a target of an upgrade, which is why none is listed here;
/// a domain still running one is answered by the same rule
/// ([`compatible_target_versions`]) from its place in this order.
pub const SUPPORTED_ENGINE_VERSIONS: [&str; 22] = [
    "6.8",
    "7.1",
    "7.4",
    "7.7",
    "7.8",
    "7.9",
    "7.10",
    "OpenSearch_1.0",
    "OpenSearch_1.1",
    "OpenSearch_1.2",
    "OpenSearch_1.3",
    "OpenSearch_2.3",
    "OpenSearch_2.5",
    "OpenSearch_2.7",
    "OpenSearch_2.9",
    "OpenSearch_2.11",
    "OpenSearch_2.13",
    "OpenSearch_2.15",
    "OpenSearch_2.17",
    "OpenSearch_2.19",
    "OpenSearch_3.0",
    "OpenSearch_3.1",
];

/// One version's place in the upgrade order: its lineage (Elasticsearch before
/// OpenSearch — a domain moves from the one to the other and never back), then
/// its major and its minor as numbers, so `7.10` sorts after `7.9` rather than
/// before it.
///
/// `None` for a string outside the three forms the model's pattern admits.
pub fn version_order(version: &str) -> Option<(u8, u32, u32)> {
    let (lineage, number) = match version.split_once('_') {
        Some(("OpenSearch" | "OS", number)) => (1u8, number),
        Some(_) => return None,
        None => (0u8, version),
    };
    let (major, minor) = number.split_once('.')?;
    Some((lineage, major.parse().ok()?, minor.parse().ok()?))
}

/// The supported set in upgrade order, each entry with its order key.
fn ordered_versions() -> Vec<((u8, u32, u32), &'static str)> {
    let mut catalog: Vec<((u8, u32, u32), &'static str)> = SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter_map(|version| version_order(version).map(|order| (order, *version)))
        .collect();
    catalog.sort_unstable();
    catalog
}

/// The versions ONE in-place upgrade can move a domain running `version` onto,
/// in upgrade order.
///
/// The service's upgrade paths step through the supported set: from any version
/// a domain reaches every LATER supported version of its own major, and a major
/// boundary is crossed only from the last supported version of a major, into
/// the versions of the major that follows it — so the last supported 2.x
/// OpenSearch version reaches the 3.x ones. The one boundary that is open from
/// EVERY version of a lineage is the migration out of Elasticsearch: a domain
/// on any Elasticsearch version reaches the first OpenSearch major the service
/// offers, which is how a current Elasticsearch domain is moved onto OpenSearch
/// without first stepping to the last Elasticsearch version. A version the
/// supported set does not carry is placed in the same order and answered by the
/// same rule; a string outside the modelled forms reaches nothing.
pub fn compatible_target_versions(version: &str) -> Vec<String> {
    let Some(source) = version_order(version) else {
        return Vec::new();
    };
    let catalog = ordered_versions();
    let source_major = (source.0, source.1);

    // The majors the supported set carries, in order, so "the major after this
    // one" is a single step along that list.
    let mut majors: Vec<(u8, u32)> = catalog
        .iter()
        .map(|((lineage, major, _), _)| (*lineage, *major))
        .collect();
    majors.dedup();

    // The last supported version of the source's own major: only a domain
    // sitting at or past it may cross into the next major.
    let last_of_source_major = catalog
        .iter()
        .filter(|((lineage, major, _), _)| (*lineage, *major) == source_major)
        .map(|(order, _)| *order)
        .next_back();
    let next_major = match last_of_source_major {
        Some(last) if source >= last => majors
            .iter()
            .position(|major| *major == source_major)
            .and_then(|at| majors.get(at + 1))
            .copied(),
        Some(_) => None,
        // A major the supported set carries no version of at all: the boundary
        // to cross is into the first supported major after it.
        None => majors
            .iter()
            .find(|(lineage, major)| (*lineage, *major) > source_major)
            .copied(),
    };

    // The lineage boundary: the first OpenSearch major the set carries is
    // reachable from EVERY Elasticsearch version, not only from the last one of
    // its major.
    let first_opensearch_major = (source.0 == 0)
        .then(|| majors.iter().find(|(lineage, _)| *lineage == 1).copied())
        .flatten();

    catalog
        .into_iter()
        .filter(|(order, _)| {
            let major = (order.0, order.1);
            *order > source
                && (major == source_major
                    || Some(major) == next_major
                    || Some(major) == first_opensearch_major)
        })
        .map(|(_, version)| version.to_string())
        .collect()
}

/// One source version's `CompatibleVersionsMap`: the version a domain is on and
/// the versions one upgrade reaches from it.
pub fn compatible_versions_map(version: &str) -> CompatibleVersionsMap {
    CompatibleVersionsMap {
        source_version: Some(version.to_string()),
        target_versions: Some(compatible_target_versions(version)),
    }
}

/// Every supported version an upgrade can move a domain OFF, with the versions
/// it reaches — what `GetCompatibleElasticsearchVersions` answers when the
/// request names no domain. A version nothing is reachable from (the newest one)
/// carries no entry, since it is the source of no upgrade.
pub fn compatible_versions_catalog() -> Vec<CompatibleVersionsMap> {
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .map(|version| compatible_versions_map(version))
        .filter(|map| {
            map.target_versions
                .as_ref()
                .is_some_and(|targets| !targets.is_empty())
        })
        .collect()
}

/// One configuration option as stored: its value plus the provenance the
/// model's `OptionStatus` reports.
#[derive(Debug, Clone)]
pub struct OptionRecord<T> {
    /// The option's current value.
    pub value: T,
    /// When this option was first set on the domain.
    pub creation_date: SystemTime,
    /// When it last changed.
    pub update_date: SystemTime,
    /// The domain's config version at that change.
    pub update_version: i32,
    /// When the clock makes this option `Active`.
    pub active_at: SystemTime,
}

impl<T> OptionRecord<T> {
    /// Record an option set for the first time, at config version `version`.
    pub fn new(value: T, now: SystemTime, version: i32) -> Self {
        Self {
            value,
            creation_date: now,
            update_date: now,
            update_version: version,
            active_at: now + PROVISIONING_SPAN,
        }
    }

    /// Replace the option's value, which re-starts its provisioning span and
    /// stamps it with the new config version.
    pub fn replace(&mut self, value: T, now: SystemTime, version: i32) {
        self.value = value;
        self.update_date = now;
        self.update_version = version;
        self.active_at = now + PROVISIONING_SPAN;
    }

    /// Whether the clock has not yet made this option active.
    pub fn processing(&self, now: SystemTime) -> bool {
        now < self.active_at
    }

    /// The option's modelled state — one of `OPTION_STATE_VALUES`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        if self.processing(now) {
            "Processing"
        } else {
            "Active"
        }
    }

    /// The `OptionStatus` wrapper for this option, built from what is stored.
    pub fn status(&self, now: SystemTime) -> OptionStatus {
        OptionStatus {
            creation_date: datetime_of(self.creation_date),
            update_date: datetime_of(self.update_date),
            update_version: self.update_version,
            state: self.state(now).to_string(),
            pending_deletion: Some(false),
        }
    }
}

/// Store `value` into `slot` when the request supplied one, leaving the stored
/// value untouched when it did not. Answers whether anything was written, which
/// is what decides that a config version bump is owed.
pub fn apply_option<T>(
    slot: &mut Option<OptionRecord<T>>,
    value: Option<T>,
    now: SystemTime,
    version: i32,
) -> bool {
    let Some(value) = value else {
        return false;
    };
    match slot {
        Some(record) => record.replace(value, now, version),
        None => *slot = Some(OptionRecord::new(value, now, version)),
    }
    true
}

/// The subnets and security groups a VPC domain was created with, as the
/// `VPCDerivedInfo` the read operations report.
///
/// `VpcId` and `AvailabilityZones` are left unset: both are derived from the
/// subnets by EC2, which this emulator holds no state for, and the model makes
/// them optional.
pub fn vpc_derived_info(options: VPCOptions) -> VPCDerivedInfo {
    VPCDerivedInfo {
        vpc_id: None,
        subnet_ids: options.subnet_ids,
        availability_zones: None,
        security_group_ids: options.security_group_ids,
    }
}

/// `AdvancedSecurityOptionsInput` as the `AdvancedSecurityOptions` the read
/// operations report.
///
/// `MasterUserOptions` is deliberately dropped rather than stored: it carries
/// `MasterUserPassword`, and the output shape has no member for it, so keeping
/// it would only risk disclosing a credential the caller supplied.
pub fn advanced_security_options(input: AdvancedSecurityOptionsInput) -> AdvancedSecurityOptions {
    AdvancedSecurityOptions {
        enabled: input.enabled,
        internal_user_database_enabled: input.internal_user_database_enabled,
        saml_options: input.saml_options.map(saml_options_output),
        anonymous_auth_disable_date: None,
        anonymous_auth_enabled: input.anonymous_auth_enabled,
    }
}

/// `SAMLOptionsInput` as the `SAMLOptionsOutput` the read operations report.
/// `MasterUserName` and `MasterBackendRole` have no output member, so they are
/// not carried over.
fn saml_options_output(input: SAMLOptionsInput) -> SAMLOptionsOutput {
    SAMLOptionsOutput {
        enabled: input.enabled,
        idp: input.idp,
        subject_key: input.subject_key,
        roles_key: input.roles_key,
        session_timeout_minutes: input.session_timeout_minutes,
    }
}

/// `AutoTuneOptionsInput` (what create takes) as the `AutoTuneOptions` both the
/// store and `UpdateElasticsearchDomainConfig` use. `RollbackOnDisable` has no
/// create-time input member, so it stays unset until an update sets it.
pub fn auto_tune_options(input: AutoTuneOptionsInput) -> AutoTuneOptions {
    AutoTuneOptions {
        desired_state: input.desired_state,
        rollback_on_disable: None,
        maintenance_schedules: input.maintenance_schedules,
    }
}

/// Every configuration option of one domain, each with its own provenance: the
/// merge target for `UpdateElasticsearchDomainConfig` and the source for both
/// `Describe*` operations.
#[derive(Debug, Clone)]
pub struct DomainConfigRecord {
    pub elasticsearch_version: Option<OptionRecord<String>>,
    /// Not optional: `ElasticsearchDomainStatus.ElasticsearchClusterConfig` is
    /// `@required`, so a domain always has one.
    pub cluster_config: OptionRecord<ElasticsearchClusterConfig>,
    pub ebs_options: Option<OptionRecord<EBSOptions>>,
    pub access_policies: Option<OptionRecord<String>>,
    pub snapshot_options: Option<OptionRecord<SnapshotOptions>>,
    pub vpc_options: Option<OptionRecord<VPCDerivedInfo>>,
    pub cognito_options: Option<OptionRecord<CognitoOptions>>,
    pub encryption_at_rest_options: Option<OptionRecord<EncryptionAtRestOptions>>,
    pub node_to_node_encryption_options: Option<OptionRecord<NodeToNodeEncryptionOptions>>,
    /// Ordered so a response's option map is deterministic across reads.
    pub advanced_options: Option<OptionRecord<BTreeMap<String, String>>>,
    pub log_publishing_options: Option<OptionRecord<BTreeMap<String, LogPublishingOption>>>,
    pub domain_endpoint_options: Option<OptionRecord<DomainEndpointOptions>>,
    pub advanced_security_options: Option<OptionRecord<AdvancedSecurityOptions>>,
    pub auto_tune_options: Option<OptionRecord<AutoTuneOptions>>,
    pub deployment_strategy_options: Option<OptionRecord<DeploymentStrategyOptions>>,
    pub automated_snapshot_pause_options:
        Option<OptionRecord<AutomatedSnapshotPauseRequestOptions>>,
    pub use_case: Option<OptionRecord<String>>,
    pub engine_mode: Option<OptionRecord<String>>,
}

impl DomainConfigRecord {
    /// A config record holding only the cluster config, which every domain has.
    /// The caller stores the options the create request supplied with
    /// [`apply_option`].
    pub fn new(cluster_config: ElasticsearchClusterConfig, now: SystemTime) -> Self {
        Self {
            elasticsearch_version: None,
            cluster_config: OptionRecord::new(cluster_config, now, FIRST_CONFIG_VERSION),
            ebs_options: None,
            access_policies: None,
            snapshot_options: None,
            vpc_options: None,
            cognito_options: None,
            encryption_at_rest_options: None,
            node_to_node_encryption_options: None,
            advanced_options: None,
            log_publishing_options: None,
            domain_endpoint_options: None,
            advanced_security_options: None,
            auto_tune_options: None,
            deployment_strategy_options: None,
            automated_snapshot_pause_options: None,
            use_case: None,
            engine_mode: None,
        }
    }
}

/// One Elasticsearch domain, as stored.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub domain_name: String,
    pub arn: String,
    /// One of `ENGINE_TYPE_VALUES`, derived at create time from the version.
    pub engine_type: String,
    pub created: bool,
    pub deleted: bool,
    pub upgrade_processing: bool,
    pub endpoint: Option<String>,
    /// The `host[:port]` of the emulator endpoint this domain was created
    /// through ([`emulator_endpoint_authority`]) — the address its data plane is
    /// PUBLISHED at ([`Domain::published_endpoint`]).
    ///
    /// Recorded once, at the create, and never on a read: what a domain
    /// publishes cannot depend on which request read it, so two readers hand a
    /// client the same address.
    pub published_at: String,
    /// The indexes this domain's cluster holds, keyed by index name. Written and
    /// read by the domain's own REST data plane
    /// ([`crate::service::domain_rest`]); deleting the domain releases them.
    pub indexes: BTreeMap<String, SearchIndex>,
    pub config: DomainConfigRecord,
    /// `CreateElasticsearchDomain.TagList`, as given.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// Bumped by every `UpdateElasticsearchDomainConfig` that changes an option.
    pub config_version: i32,
    pub last_updated: SystemTime,
    /// When the clock finishes the domain's current change.
    pub active_at: SystemTime,
}

/// When a change begun at `now` becomes `Active`. Stored, never slept on.
pub fn active_deadline(now: SystemTime) -> SystemTime {
    now + PROVISIONING_SPAN
}

impl Domain {
    /// A newly created domain. Its id, ARN and endpoint are derived ONCE, here,
    /// and stored: every later read answers with the stored strings.
    ///
    /// `published_at` is the emulator endpoint the create was served at, which
    /// is the address this domain's data plane is published at from now on.
    pub fn create(
        domain_name: String,
        region: &str,
        seq: u64,
        published_at: String,
        config: DomainConfigRecord,
        tags: Vec<Tag>,
        now: SystemTime,
    ) -> Self {
        let engine_type = engine_type_of(
            config
                .elasticsearch_version
                .as_ref()
                .map(|record| record.value.as_str()),
        );
        Self {
            domain_id: domain_id(&domain_name),
            arn: domain_arn(region, &domain_name),
            endpoint: Some(domain_endpoint(&domain_name, region, seq)),
            published_at,
            indexes: BTreeMap::new(),
            engine_type: engine_type.to_string(),
            domain_name,
            created: true,
            deleted: false,
            upgrade_processing: false,
            config,
            tags,
            created_at: now,
            config_version: FIRST_CONFIG_VERSION,
            last_updated: now,
            active_at: active_deadline(now),
        }
    }

    /// Record that a config change has begun: the domain moves to `version` and
    /// starts a fresh provisioning span.
    pub fn begin_change(&mut self, version: i32, now: SystemTime) {
        self.config_version = version;
        self.last_updated = now;
        self.active_at = active_deadline(now);
    }

    /// Whether the domain's last change is still provisioning.
    pub fn processing(&self, now: SystemTime) -> bool {
        now < self.active_at
    }

    /// The address the control plane PUBLISHES for this domain's data plane —
    /// `ElasticsearchDomainStatus$Endpoint`, "the Elasticsearch domain endpoint
    /// that you use to submit index and search requests".
    ///
    /// It is the address a client of this emulator can REACH the domain at
    /// ([`published_domain_address`]), because a client builds its index and
    /// search requests on the value it is handed: the host AWS names this
    /// endpoint by ([`Self::endpoint`]) resolves nowhere here, so publishing it
    /// would announce a data plane nothing answers at. A record that never
    /// recorded an emulator endpoint falls back to that host rather than to a
    /// path naming no host at all.
    pub fn published_endpoint(&self) -> Option<String> {
        let endpoint = self.endpoint.as_ref()?;
        if self.published_at.is_empty() {
            return Some(endpoint.clone());
        }
        Some(published_domain_address(
            &self.published_at,
            &self.domain_name,
        ))
    }

    /// The domain's modelled processing status — one of
    /// `DOMAIN_PROCESSING_STATUS_TYPE_VALUES`.
    pub fn processing_status(&self, now: SystemTime) -> &'static str {
        if self.deleted {
            "Deleting"
        } else if self.processing(now) {
            if self.config_version <= FIRST_CONFIG_VERSION {
                "Creating"
            } else {
                "Modifying"
            }
        } else {
            "Active"
        }
    }

    /// This domain's entry in `ListDomainNames`.
    pub fn domain_info(&self) -> DomainInfo {
        DomainInfo {
            domain_name: Some(self.domain_name.clone()),
            engine_type: Some(self.engine_type.clone()),
        }
    }

    /// The domain's `ElasticsearchDomainStatus`, computed from what is stored.
    pub fn domain_status(&self, now: SystemTime) -> ElasticsearchDomainStatus {
        let config = &self.config;
        ElasticsearchDomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: self.arn.clone(),
            created: Some(self.created),
            deleted: Some(self.deleted),
            endpoint: self.published_endpoint(),
            // Only a VPC domain reports per-subnet endpoints, and those are EC2-derived.
            endpoints: None,
            processing: Some(self.processing(now)),
            upgrade_processing: Some(self.upgrade_processing),
            elasticsearch_version: config
                .elasticsearch_version
                .as_ref()
                .map(|record| record.value.clone()),
            elasticsearch_cluster_config: config.cluster_config.value.clone(),
            ebs_options: config.ebs_options.as_ref().map(|r| r.value.clone()),
            access_policies: config.access_policies.as_ref().map(|r| r.value.clone()),
            snapshot_options: config.snapshot_options.as_ref().map(|r| r.value.clone()),
            vpc_options: config.vpc_options.as_ref().map(|r| r.value.clone()),
            cognito_options: config.cognito_options.as_ref().map(|r| r.value.clone()),
            encryption_at_rest_options: config
                .encryption_at_rest_options
                .as_ref()
                .map(|r| r.value.clone()),
            node_to_node_encryption_options: config
                .node_to_node_encryption_options
                .as_ref()
                .map(|r| r.value.clone()),
            advanced_options: config
                .advanced_options
                .as_ref()
                .map(|r| r.value.clone().into_iter().collect()),
            log_publishing_options: config
                .log_publishing_options
                .as_ref()
                .map(|r| r.value.clone().into_iter().collect()),
            // Owned by Start/CancelElasticsearchServiceSoftwareUpdate, which are
            // still scaffolded: no software update has been requested on this domain.
            service_software_options: None,
            domain_endpoint_options: config
                .domain_endpoint_options
                .as_ref()
                .map(|r| r.value.clone()),
            advanced_security_options: config
                .advanced_security_options
                .as_ref()
                .map(|r| r.value.clone()),
            auto_tune_options: config.auto_tune_options.as_ref().map(|record| {
                AutoTuneOptionsOutput {
                    state: Some(
                        auto_tune_state(
                            record.value.desired_state.as_deref(),
                            record.processing(now),
                        )
                        .to_string(),
                    ),
                    error_message: None,
                }
            }),
            // Set by CancelDomainConfigChange / DescribeDomainChangeProgress, both
            // still scaffolded.
            change_progress_details: None,
            domain_processing_status: Some(self.processing_status(now).to_string()),
            modifying_properties: None,
            deployment_strategy_options: config
                .deployment_strategy_options
                .as_ref()
                .map(|r| r.value.clone()),
            automated_snapshot_pause_options: config
                .automated_snapshot_pause_options
                .as_ref()
                .map(|r| automated_snapshot_pause_options(r.value.clone(), now)),
            use_case: config.use_case.as_ref().map(|r| r.value.clone()),
            engine_mode: config.engine_mode.as_ref().map(|r| r.value.clone()),
        }
    }

    /// The domain's `ElasticsearchDomainConfig`: every stored option wrapped in
    /// its own status, built from that option's own provenance.
    pub fn domain_config(&self, now: SystemTime) -> ElasticsearchDomainConfig {
        let config = &self.config;
        ElasticsearchDomainConfig {
            elasticsearch_version: config.elasticsearch_version.as_ref().map(|r| {
                ElasticsearchVersionStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            elasticsearch_cluster_config: Some(ElasticsearchClusterConfigStatus {
                options: config.cluster_config.value.clone(),
                status: config.cluster_config.status(now),
            }),
            ebs_options: config.ebs_options.as_ref().map(|r| EBSOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            access_policies: config.access_policies.as_ref().map(|r| AccessPoliciesStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            snapshot_options: config.snapshot_options.as_ref().map(|r| SnapshotOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            vpc_options: config.vpc_options.as_ref().map(|r| VPCDerivedInfoStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            cognito_options: config.cognito_options.as_ref().map(|r| CognitoOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            encryption_at_rest_options: config.encryption_at_rest_options.as_ref().map(|r| {
                EncryptionAtRestOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            node_to_node_encryption_options: config.node_to_node_encryption_options.as_ref().map(
                |r| NodeToNodeEncryptionOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                },
            ),
            advanced_options: config.advanced_options.as_ref().map(|r| AdvancedOptionsStatus {
                options: r.value.clone().into_iter().collect(),
                status: r.status(now),
            }),
            log_publishing_options: config.log_publishing_options.as_ref().map(|r| {
                LogPublishingOptionsStatus {
                    options: Some(r.value.clone().into_iter().collect()),
                    status: Some(r.status(now)),
                }
            }),
            domain_endpoint_options: config.domain_endpoint_options.as_ref().map(|r| {
                DomainEndpointOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            advanced_security_options: config.advanced_security_options.as_ref().map(|r| {
                AdvancedSecurityOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            auto_tune_options: config.auto_tune_options.as_ref().map(|r| AutoTuneOptionsStatus {
                options: Some(r.value.clone()),
                status: Some(AutoTuneStatus {
                    creation_date: datetime_of(r.creation_date),
                    update_date: datetime_of(r.update_date),
                    update_version: r.update_version,
                    state: auto_tune_state(r.value.desired_state.as_deref(), r.processing(now))
                        .to_string(),
                    error_message: None,
                    pending_deletion: Some(false),
                }),
            }),
            change_progress_details: None,
            modifying_properties: None,
            deployment_strategy_options: config.deployment_strategy_options.as_ref().map(|r| {
                DeploymentStrategyOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            automated_snapshot_pause_options: config
                .automated_snapshot_pause_options
                .as_ref()
                .map(|r| AutomatedSnapshotPauseOptionsStatus {
                    options: automated_snapshot_pause_options(r.value.clone(), now),
                    status: r.status(now),
                }),
            use_case: config.use_case.as_ref().map(|r| UseCaseStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            engine_mode: config.engine_mode.as_ref().map(|r| EngineModeStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
        }
    }
}

/// The Auto-Tune state to report — one of `AUTO_TUNE_STATE_VALUES`.
///
/// While the option is still provisioning the state is the matching
/// `*_IN_PROGRESS` value; once the clock has made it active it is the state the
/// caller asked for.
fn auto_tune_state(desired_state: Option<&str>, processing: bool) -> &'static str {
    match (desired_state, processing) {
        (Some("ENABLED"), true) => "ENABLE_IN_PROGRESS",
        (Some("ENABLED"), false) => "ENABLED",
        (Some("DISABLED"), true) => "DISABLE_IN_PROGRESS",
        _ => "DISABLED",
    }
}

/// The automated-snapshot pause options as reported, with the `PauseState` the
/// stored window and the clock imply — one of `PAUSE_STATE_VALUES`.
fn automated_snapshot_pause_options(
    stored: AutomatedSnapshotPauseRequestOptions,
    now: SystemTime,
) -> AutomatedSnapshotPauseOptions {
    let now_secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let started = stored
        .start_time
        .as_ref()
        .is_none_or(|start| now_secs >= start.as_secs_f64());
    let ended = stored
        .end_time
        .as_ref()
        .is_some_and(|end| now_secs >= end.as_secs_f64());
    let state = if !stored.enabled {
        "Disabled"
    } else if !started {
        "Scheduled"
    } else if ended {
        "Completed"
    } else {
        "Active"
    };
    AutomatedSnapshotPauseOptions {
        enabled: stored.enabled,
        start_time: stored.start_time,
        end_time: stored.end_time,
        state: Some(state.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Cross-cluster search connections
// ---------------------------------------------------------------------------

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `VALIDATING`: "The outbound
/// connection request is being validated." The status a connection holds from the
/// instant the create returns until [`VALIDATION_SPAN`] has passed.
pub const CONNECTION_STATUS_VALIDATING: &str = "VALIDATING";

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `PENDING_ACCEPTANCE`:
/// "Outbound connection request is validated and is not yet accepted by destination
/// domain owner" — where a request that passed validation waits for that decision.
pub const CONNECTION_STATUS_PENDING_ACCEPTANCE: &str = "PENDING_ACCEPTANCE";

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `VALIDATION_FAILED`:
/// "Validation failed for the connection request" — where a request naming a domain
/// that does not resolve ends up, since no owner can decide on it.
pub const CONNECTION_STATUS_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// How long a connection request stays [`CONNECTION_STATUS_VALIDATING`] before the
/// clock settles it on the outcome the create computed.
///
/// The emulator CHOOSES this span: AWS validates a connection request
/// asynchronously, so the create's answer is never the validation's outcome. It is
/// deliberately non-zero, so a caller that reads the connection straight after
/// creating it still observes the modelled `VALIDATING` status. Nothing sleeps for
/// it: the due instant is stored and compared against the clock on each read.
pub const VALIDATION_SPAN: std::time::Duration = std::time::Duration::from_secs(3);

/// How many outbound cross-cluster search connections ONE source domain may hold
/// before the create answers the modelled `LimitExceededException` ("an exception
/// for trying to create more than allowed resources or sub-resources").
///
/// 20 is the service's documented limit on the cross-cluster search connections a
/// domain takes part in. It bounds what one domain can add to the store.
pub const MAX_CONNECTIONS_PER_DOMAIN: usize = 20;

/// A hard ceiling on every stored connection, across every source domain and
/// region.
///
/// [`MAX_CONNECTIONS_PER_DOMAIN`] alone does not bound the store: the source domain
/// is named by the request, so a caller varying it would get a fresh allowance each
/// time. This cap dominates the single growth site instead, and sits at the product
/// of the two documented quotas, so it never clips a caller that stays inside both.
pub const MAX_CONNECTIONS_TOTAL: usize = MAX_CONNECTIONS_PER_DOMAIN * MAX_DOMAINS_TOTAL;

/// The odd multiplier that scrambles a resource counter into its id.
///
/// Multiplication by an odd constant modulo 2^64 is a BIJECTION on `u64`, so two
/// counter values can never render the same id.
const ID_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

/// The id an outbound cross-cluster search connection is addressed by.
///
/// `CrossClusterSearchConnectionId` declares no constraint, so what matters is that
/// the id is opaque and that two connections never share one: it is the store's
/// monotonic connection counter scrambled through the bijection described on
/// [`ID_MULTIPLIER`], behind the prefix naming the resource it
/// identifies.
pub fn cross_cluster_connection_id(seq: u64) -> String {
    format!(
        "ccs-{token:016x}",
        token = seq.wrapping_mul(ID_MULTIPLIER)
    )
}

/// A `DomainInformation` reference with the owner and Region it is resolved against
/// filled in.
///
/// A reference naming no `OwnerId` is to a domain of the account making the call,
/// and one naming no `Region` is to a domain in the Region the call was signed for —
/// so the stored record, and every report of it, names the domain in full however
/// little the request said.
pub fn resolved_domain_information(info: DomainInformation, region: &str) -> DomainInformation {
    DomainInformation {
        owner_id: Some(
            info.owner_id
                .unwrap_or_else(|| ::engine::emulator_account_id().to_string()),
        ),
        domain_name: info.domain_name,
        region: Some(info.region.unwrap_or_else(|| region.to_string())),
    }
}

/// One outbound cross-cluster search connection, as stored.
///
/// The record is the single source of truth for every answer about the connection:
/// the create's response and every later read are both [`Self::to_outbound`] of it,
/// so the two cannot disagree. Where the connection is in its lifecycle is COMPUTED
/// from the stored validation outcome and its due instant rather than stored as a
/// mutable status, so no read has to write.
#[derive(Debug, Clone)]
pub struct CrossClusterSearchConnection {
    /// `CrossClusterSearchConnectionId`, unique per connection, so it IS the key the
    /// store holds the record under.
    pub connection_id: String,
    /// The Region the connection itself lives in: the one the create was signed for,
    /// which is the control plane that holds it.
    pub region: String,
    /// `ConnectionAlias`: the name the source domain's owner gave the connection.
    pub connection_alias: String,
    /// The source domain, as [`resolved_domain_information`] recorded it.
    pub source_domain: DomainInformation,
    /// The destination domain, as [`resolved_domain_information`] recorded it.
    pub destination_domain: DomainInformation,
    /// When the clock finishes validating the request.
    pub validated_at: SystemTime,
    /// The status the connection is in once [`Self::validated_at`] has passed: the
    /// outcome the create computed from the domains the request named.
    pub validated_status: String,
    /// `OutboundCrossClusterSearchConnectionStatus$Message` for that outcome, where
    /// there is something to say about it.
    pub validated_message: Option<String>,
}

impl CrossClusterSearchConnection {
    /// Where the connection is in its lifecycle at `now` — one of
    /// `OUTBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_STATUS_CODE_VALUES`. Still being
    /// validated until the clock has made the outcome due.
    pub fn status_code(&self, now: SystemTime) -> &str {
        if now < self.validated_at {
            CONNECTION_STATUS_VALIDATING
        } else {
            &self.validated_status
        }
    }

    /// The verbose information for that status, which a request still being
    /// validated has none of: the outcome's message is only the settled status's.
    pub fn status_message(&self, now: SystemTime) -> Option<String> {
        if now < self.validated_at {
            None
        } else {
            self.validated_message.clone()
        }
    }

    /// This record as the `OutboundCrossClusterSearchConnection` the describe
    /// reports, member for member the projection the create answers with.
    pub fn to_outbound(&self, now: SystemTime) -> OutboundCrossClusterSearchConnection {
        OutboundCrossClusterSearchConnection {
            source_domain_info: Some(self.source_domain.clone()),
            destination_domain_info: Some(self.destination_domain.clone()),
            cross_cluster_search_connection_id: Some(self.connection_id.clone()),
            connection_alias: Some(self.connection_alias.clone()),
            connection_status: Some(OutboundCrossClusterSearchConnectionStatus {
                status_code: Some(self.status_code(now).to_string()),
                message: self.status_message(now),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Packages
// ---------------------------------------------------------------------------

/// The plugin packages the service itself publishes, which every account sees
/// beside the ones it created.
mod package_catalog;
pub use package_catalog::*;

/// `PackageStatus`'s `COPYING`: the package's content is still being copied out
/// of the customer's S3 location. The status a package holds from the instant
/// `CreatePackage` returns until [`active_deadline`] has passed.
pub const PACKAGE_STATUS_COPYING: &str = "COPYING";

/// `PackageStatus`'s `AVAILABLE`: the copy finished and the package can be
/// associated with a domain.
pub const PACKAGE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The label the `ordinal`-th revision of a package's content carries.
///
/// `PackageVersion` is an unconstrained string, and the versions one package
/// goes through are a GENERATED SEQUENCE rather than anything the caller
/// supplies: a create mints the first, and every `UpdatePackage` the next
/// counting number behind the same `v` prefix. One function is the only place
/// that shape is written, so `AvailablePackageVersion`, the version history and
/// an association's `PackageVersion` cannot name a package's version three
/// different ways.
pub fn package_version_label(ordinal: usize) -> String {
    format!("v{ordinal}")
}

/// The most revisions one package may hold.
///
/// `UpdatePackage` is a caller-driven growth site on a record that is never
/// otherwise trimmed, and this is the emulator's own bound on it, answered with
/// the modelled `LimitExceededException` ("trying to create more than allowed
/// resources or sub-resources"). It sits far above the number of revisions a
/// dictionary goes through in any one workflow.
pub const MAX_PACKAGE_VERSIONS: usize = 100;

/// A hard ceiling on every stored package, across every region.
///
/// Packages are the second caller-driven growth site in this store, and
/// `CreatePackage`'s only bound on it: the region comes from the request's own
/// credential scope, so a per-region allowance alone would not bound the total.
/// It sits far above the number of packages any domain workflow associates, so
/// no caller that stays inside the service's own quotas is clipped by it.
pub const MAX_PACKAGES_TOTAL: usize = 1_000;

/// The key one package is stored under: the region it was created in and the id
/// the service minted for it.
pub fn package_key(region: &str, package_id: &str) -> (String, String) {
    (region.to_string(), package_id.to_string())
}

/// The id a package is addressed by.
///
/// `PackageID` declares no constraint, so what matters is that the id is opaque
/// and that two packages never share one: it is the store's monotonic package
/// counter scrambled through the bijection described on [`ID_MULTIPLIER`],
/// behind the prefix naming the resource it identifies.
pub fn package_id(seq: u64) -> String {
    format!("pkg-{token:016x}", token = seq.wrapping_mul(ID_MULTIPLIER))
}

/// One revision of a package's content, as stored.
///
/// The sequence of these IS the package's version history: `CreatePackage`
/// appends the first and each `UpdatePackage` the next, so the history a read
/// reports is the stored sequence rather than anything recomputed per response.
#[derive(Debug, Clone)]
pub struct PackageVersionRecord {
    /// This revision's label — see [`package_version_label`].
    pub version: String,
    /// `CommitMessage`, "a message associated with the version", where the
    /// update that minted it carried one. A create has no such input member, so
    /// a package's first version has none.
    pub commit_message: Option<String>,
    /// When this revision was minted.
    pub created_at: SystemTime,
}

impl PackageVersionRecord {
    /// This record as the `PackageVersionHistory` entry a history read reports.
    pub fn to_history(&self) -> PackageVersionHistory {
        PackageVersionHistory {
            package_version: Some(self.version.clone()),
            commit_message: self.commit_message.clone(),
            created_at: Some(datetime_of(self.created_at)),
        }
    }
}

/// One package, as stored.
///
/// The record is the single source of truth for every answer about the package:
/// the create's response and every later read are both [`Self::to_details`] of
/// it, so the two cannot disagree. The package's status is COMPUTED from the
/// instant its copy falls due rather than stored as a mutable status, so no read
/// has to write.
#[derive(Debug, Clone)]
pub struct Package {
    /// `PackageID`, unique per package, so it is half the key the store holds
    /// the record under.
    pub package_id: String,
    /// `PackageName`, the name the caller gave the package. Unique among the
    /// packages of one region, which is the scope a create checks it in.
    pub package_name: String,
    /// `PackageType` — one of `PACKAGE_TYPE_VALUES`.
    pub package_type: String,
    /// `PackageDescription`, where the create supplied one.
    pub package_description: Option<String>,
    /// The S3 location the content was imported from, as the request gave it.
    pub source: PackageSource,
    /// When the package was created.
    pub created_at: SystemTime,
    /// When the package last changed.
    pub last_updated_at: SystemTime,
    /// When the clock finishes copying the current content, i.e. when the
    /// package becomes [`PACKAGE_STATUS_AVAILABLE`].
    pub available_at: SystemTime,
    /// Every revision of the package's content, oldest first — the package's
    /// version history, and the source of the version its current content
    /// carries.
    pub versions: Vec<PackageVersionRecord>,
}

impl Package {
    /// A newly created package, whose content is still being copied.
    pub fn create(
        package_id: String,
        package_name: String,
        package_type: String,
        package_description: Option<String>,
        source: PackageSource,
        now: SystemTime,
    ) -> Self {
        Self {
            package_id,
            package_name,
            package_type,
            package_description,
            source,
            created_at: now,
            last_updated_at: now,
            available_at: active_deadline(now),
            versions: vec![PackageVersionRecord {
                version: package_version_label(1),
                commit_message: None,
                created_at: now,
            }],
        }
    }

    /// The label of the package's newest revision — the version its current
    /// copy is of, and the one `AvailablePackageVersion` names once that copy
    /// has landed.
    pub fn current_version(&self) -> &str {
        self.versions
            .last()
            .map(|revision| revision.version.as_str())
            .unwrap_or_default()
    }

    /// Take new content: the package's source becomes `source`, the clock starts
    /// copying it again, and the history grows by the next version.
    ///
    /// `PackageDescription` is replaced only when the update supplied one, since
    /// the member is optional and an update that omits it changes nothing about
    /// the description.
    pub fn revise(
        &mut self,
        source: PackageSource,
        package_description: Option<String>,
        commit_message: Option<String>,
        now: SystemTime,
    ) {
        self.source = source;
        if package_description.is_some() {
            self.package_description = package_description;
        }
        self.versions.push(PackageVersionRecord {
            version: package_version_label(self.versions.len() + 1),
            commit_message,
            created_at: now,
        });
        self.last_updated_at = now;
        self.available_at = active_deadline(now);
    }

    /// The package's version history, NEWEST FIRST: a history read reports the
    /// most recent revision first, so the newest version is the head of the
    /// first page.
    pub fn history_newest_first(&self) -> Vec<&PackageVersionRecord> {
        self.versions.iter().rev().collect()
    }

    /// Whether the clock has not yet finished copying the package's content.
    pub fn copying(&self, now: SystemTime) -> bool {
        now < self.available_at
    }

    /// The package's modelled state — one of `PACKAGE_STATUS_VALUES`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.copying(now) {
            PACKAGE_STATUS_COPYING
        } else {
            PACKAGE_STATUS_AVAILABLE
        }
    }

    /// This record as the `PackageDetails` every package operation reports.
    ///
    /// `AvailablePackageVersion` names the version a domain can associate, so a
    /// package whose copy has not landed yet has none to report; `ErrorDetails`
    /// is "additional information if the package is in an error state. Null
    /// otherwise" (its own documentation), and a stored package is in no error
    /// state.
    pub fn to_details(&self, now: SystemTime) -> PackageDetails {
        PackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(self.package_name.clone()),
            package_type: Some(self.package_type.clone()),
            package_description: self.package_description.clone(),
            package_status: Some(self.status(now).to_string()),
            created_at: Some(datetime_of(self.created_at)),
            last_updated_at: Some(datetime_of(self.last_updated_at)),
            available_package_version: (!self.copying(now))
                .then(|| self.current_version().to_string()),
            error_details: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Package/domain associations
// ---------------------------------------------------------------------------

/// `DomainPackageStatus`'s `ASSOCIATING`: the package is being installed on the
/// domain's nodes. The state an association holds from the instant
/// `AssociatePackage` returns until [`active_deadline`] has passed.
pub const DOMAIN_PACKAGE_STATUS_ASSOCIATING: &str = "ASSOCIATING";

/// `DOMAIN_PACKAGE_STATUS`'s `ACTIVE`: the package is installed and the domain
/// can reference it.
pub const DOMAIN_PACKAGE_STATUS_ACTIVE: &str = "ACTIVE";

/// How many packages one domain may hold associated with it.
///
/// AWS documents a quota of 100 packages per domain. Enforcing it keeps the
/// association count of one domain bounded by a stated service limit rather
/// than by how many associate calls a caller issues.
pub const MAX_PACKAGES_PER_DOMAIN: usize = 100;

/// A hard ceiling on every stored association, across every region.
///
/// [`MAX_PACKAGES_PER_DOMAIN`] alone does not bound the store: the region comes
/// from the request's own credential scope, so a caller varying it would get a
/// fresh allowance each time. This cap dominates the single growth site
/// instead, and sits well above the per-domain quota, so no domain's allowance
/// is clipped by it.
pub const MAX_ASSOCIATIONS_TOTAL: usize = 10_000;

/// The refusal `AssociatePackage` gives a domain whose service software is not
/// the latest one.
///
/// A domain still provisioning a change has not finished moving onto its
/// service software, so it cannot take a package yet. The sentence pair is the
/// whole message: nothing from the request or the store is interpolated into it.
pub const NOT_ON_LATEST_SOFTWARE_MESSAGE: &str =
    "The domain is not on the latest software version. Upgrade the domain to start using this \
     feature. ";

/// The key one association is stored under: the region, the package and the
/// domain.
///
/// Region first, so an account's regions stay apart; the package before the
/// domain, so one package's associations are contiguous and in domain-name
/// order, which is the order a list of them is reported in.
pub fn association_key(
    region: &str,
    package_id: &str,
    domain_name: &str,
) -> (String, String, String) {
    (
        region.to_string(),
        package_id.to_string(),
        domain_name.to_string(),
    )
}

/// `ReferencePath` — "the relative path on Amazon ES nodes, which can be used
/// as synonym_path when the package is synonym file" (its own documentation):
/// the package's content is laid down under the nodes' analyzer directory,
/// addressed by the package's own id.
pub fn package_reference_path(package_id: &str) -> String {
    format!("analyzers/{package_id}")
}

/// One package associated with one domain, as stored.
///
/// This record is the single source of truth for the association: the
/// associate's own `DomainPackageDetails` and every later `ListPackagesForDomain`
/// / `ListDomainsForPackage` entry are both [`Self::to_details`] of it. The
/// association's status is COMPUTED from the instant its installation falls
/// due, so reporting it never has to write.
#[derive(Debug, Clone)]
pub struct DomainPackage {
    /// The package being installed, which is half the key the store holds the
    /// record under.
    pub package_id: String,
    /// The domain it is installed on, the other half of that key.
    pub domain_name: String,
    /// The package version the domain picked up when the association was made.
    /// A later `UpdatePackage` mints a new version for the package without
    /// moving the domain onto it, so this is stored rather than read back off
    /// the package.
    pub package_version: String,
    /// When the association last changed — `LastUpdated`, "timestamp of the
    /// most-recent update to the association status".
    pub last_updated: SystemTime,
    /// When the clock finishes installing the package on the domain, i.e. when
    /// the association becomes [`DOMAIN_PACKAGE_STATUS_ACTIVE`].
    pub active_at: SystemTime,
}

impl DomainPackage {
    /// A freshly made association, still being installed.
    pub fn associate(package: &Package, domain_name: String, now: SystemTime) -> Self {
        Self {
            package_id: package.package_id.clone(),
            domain_name,
            package_version: package.current_version().to_string(),
            last_updated: now,
            active_at: active_deadline(now),
        }
    }

    /// Whether the clock has not yet finished installing the package.
    pub fn associating(&self, now: SystemTime) -> bool {
        now < self.active_at
    }

    /// The association's modelled state — one of `DOMAIN_PACKAGE_STATUS_VALUES`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.associating(now) {
            DOMAIN_PACKAGE_STATUS_ASSOCIATING
        } else {
            DOMAIN_PACKAGE_STATUS_ACTIVE
        }
    }

    /// This record as the `DomainPackageDetails` every association operation
    /// reports.
    ///
    /// The package's name and type come from the package record this
    /// association names, so a list of associations and a describe of the
    /// package cannot disagree about them. `ErrorDetails` is "additional
    /// information if the package is in an error state. Null otherwise" (its own
    /// documentation), and a stored association is in no error state.
    pub fn to_details(&self, package: &Package, now: SystemTime) -> DomainPackageDetails {
        DomainPackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(package.package_name.clone()),
            package_type: Some(package.package_type.clone()),
            last_updated: Some(datetime_of(self.last_updated)),
            domain_name: Some(self.domain_name.clone()),
            domain_package_status: Some(self.status(now).to_string()),
            package_version: Some(self.package_version.clone()),
            reference_path: Some(package_reference_path(&self.package_id)),
            error_details: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Domain tags
// ---------------------------------------------------------------------------

/// How many tags one domain may carry.
///
/// Straight from `AddTags`'s own documentation: "An Elasticsearch domain may
/// have up to 10 tags." It is also this store's bound on the tag set one domain
/// record retains, since neither `TagList` nor `AddTags` declares a length for
/// the boundary validator to enforce.
pub const MAX_TAGS_PER_DOMAIN: usize = 10;

/// `Some(reason)` when ONE call's tag list names more tags than a domain may
/// carry at all.
///
/// A domain holds at most [`MAX_TAGS_PER_DOMAIN`] keys, so a single list naming
/// more than that cannot describe a tag set the service would keep: it is the
/// allowance's own refusal, reached on the request's list alone and therefore
/// before any domain is resolved — which is what keeps the cost of one call
/// bounded by the allowance rather than by how long a list the request carried,
/// since neither `TagList` nor the operations that take one declare a length for
/// the boundary validator to enforce.
///
/// It bounds ONE list, which is the whole stored set on a create
/// (`CreateElasticsearchDomain`'s `TagList` becomes the record's tags) and the
/// submitted half on an `AddTags`, where the handler counts the set the MERGE
/// would leave behind against the same allowance before it lands. Both halves
/// are needed: this one alone would let repeated calls, each inside the
/// allowance, grow a stored record past it.
pub fn oversized_tag_list(count: usize) -> Option<String> {
    (count > MAX_TAGS_PER_DOMAIN).then(|| {
        format!(
            "TagList carries {count} tags, which is more than the \
             {MAX_TAGS_PER_DOMAIN} tags a domain may have"
        )
    })
}

/// Merge `added` into a domain's tag set.
///
/// A key the domain already carries takes the submitted value and a key it does
/// not is appended, so "tag keys must be unique for the Elasticsearch domain"
/// (`Tag$Key`'s own documentation) holds and the set keeps the order its keys
/// were first attached in.
///
/// The keys already held are indexed once, so merging costs one lookup per
/// submitted tag rather than a scan of the whole set per tag.
pub fn merge_tags(tags: &mut Vec<Tag>, added: Vec<Tag>) {
    let mut at: BTreeMap<String, usize> = tags
        .iter()
        .enumerate()
        .map(|(index, tag)| (tag.key.clone(), index))
        .collect();
    for tag in added {
        match at.get(&tag.key).copied() {
            Some(index) => {
                if let Some(held) = tags.get_mut(index) {
                    held.value = tag.value;
                }
            }
            None => {
                at.insert(tag.key.clone(), tags.len());
                tags.push(tag);
            }
        }
    }
}

/// The keys one removal names, indexed once.
///
/// A removal may name any number of keys — nothing documents a limit on
/// `TagKeys`, and naming a key the domain does not carry is not an error — so
/// the list is not refused for its length; it is turned into a set HERE, which
/// the caller does before it locks the store, so the only work done while
/// holding the store is one lookup per tag the domain actually carries.
pub fn tag_keys_to_drop(removed: &[String]) -> BTreeSet<&str> {
    removed.iter().map(String::as_str).collect()
}

/// Drop every tag whose key `dropped` holds, leaving every other key and value
/// as it was.
pub fn remove_tag_keys(tags: &mut Vec<Tag>, dropped: &BTreeSet<&str>) {
    tags.retain(|tag| !dropped.contains(tag.key.as_str()));
}

/// The refusal a tag operation gives an `ARN` that names no domain of this
/// account — one that is not an Elasticsearch domain ARN at all, or one whose
/// domain this store does not hold.
///
/// Wholly fixed text: nothing from the request is interpolated into it.
pub const INVALID_TAG_ARN_MESSAGE: &str = "Invalid ARN. Domain not found.";

/// Per-service state for com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State {
    /// Every live domain, keyed by `(region, domain name)` — the scope the name
    /// is unique in (`DomainName`'s own doc: "unique across the domains owned by
    /// an account within an AWS region"), so the same name in two regions is two
    /// domains. Ordered, so `ListDomainNames` is deterministic.
    pub domains: BTreeMap<(String, String), Domain>,
    /// Monotonic counter feeding generated endpoint suffixes, so each domain
    /// created in a run gets its own.
    pub next_id_seq: u64,
    /// Every outbound cross-cluster search connection, keyed by its
    /// `CrossClusterSearchConnectionId`. Ordered, so the describe reports the same
    /// order twice — which is what lets a pagination token name a place in the set.
    pub connections: BTreeMap<String, CrossClusterSearchConnection>,
    /// Monotonic counter feeding [`Self::mint_connection_id`].
    pub connection_seq: u64,
    /// Every package, keyed by `(region, package id)` — a package is held by the
    /// control plane of the Region its create was signed for. Ordered, so a
    /// describe reports the same order twice, which is what lets a pagination
    /// token name a place in the set.
    pub packages: BTreeMap<(String, String), Package>,
    /// Monotonic counter feeding [`Self::mint_package_id`].
    pub package_seq: u64,
    /// Every package/domain association, keyed by `(region, package id, domain
    /// name)` — see [`association_key`]. Ordered, so both list operations report
    /// the same order twice, which is what lets a pagination token name a place
    /// in the set.
    pub package_associations: BTreeMap<(String, String, String), DomainPackage>,
}

impl ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State {
    /// Hand out the next sequence number.
    pub fn next_seq(&mut self) -> u64 {
        self.next_id_seq += 1;
        self.next_id_seq
    }

    /// How many domains the account already holds in `region` — the count the
    /// per-region quota is checked against.
    pub fn domain_count(&self, region: &str) -> usize {
        self.domains
            .keys()
            .filter(|(each, _)| each == region)
            .count()
    }

    /// The domains of one region, in name order.
    pub fn domains_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Domain> {
        self.domains
            .iter()
            .filter(move |((each, _), _)| each == region)
            .map(|(_, domain)| domain)
    }

    /// Hand out the id of the next connection created, so two connections never
    /// share one. See [`cross_cluster_connection_id`].
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seq = self.connection_seq.wrapping_add(1);
        cross_cluster_connection_id(self.connection_seq)
    }

    /// Whether the domain a `DomainInformation` reference names is one this store
    /// holds — what a connection request is validated against.
    ///
    /// A reference is resolved against the registry this emulator serves, which
    /// holds one account's domains keyed by Region and name, so it is the Region and
    /// the NAME that resolve; the `OwnerId` a request names is recorded with the
    /// reference rather than resolved, since no other account's domains are here to
    /// resolve it against.
    pub fn holds_referenced_domain(&self, reference: &DomainInformation) -> bool {
        reference.region.as_ref().is_some_and(|region| {
            self.domains
                .contains_key(&domain_key(region, &reference.domain_name))
        })
    }

    /// The connections of one Region, in connection-id order: a connection lives in
    /// the Region its create was signed for, so that is the scope a describe reports.
    pub fn connections_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a CrossClusterSearchConnection> {
        self.connections
            .values()
            .filter(move |connection| connection.region == region)
    }

    /// How many connections one source domain already holds — the count the
    /// per-domain quota is checked against.
    pub fn connection_count_from(&self, region: &str, source_domain_name: &str) -> usize {
        self.connections_in(region)
            .filter(|connection| connection.source_domain.domain_name == source_domain_name)
            .count()
    }

    /// Hand out the id of the next package created, so two packages never share
    /// one. See [`package_id`].
    pub fn mint_package_id(&mut self) -> String {
        self.package_seq = self.package_seq.wrapping_add(1);
        package_id(self.package_seq)
    }

    /// The packages of one Region, in package-id order.
    pub fn packages_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Package> {
        self.packages
            .iter()
            .filter(move |((each, _), _)| each == region)
            .map(|(_, package)| package)
    }

    /// Every package VISIBLE in one Region, in package-id order: the catalog of
    /// plugin packages the service publishes to every account, plus the
    /// packages this account created in that Region.
    ///
    /// This is the set the package reads report and resolve an id against —
    /// `DescribePackages` describes "all packages available to Amazon ES", not
    /// only the ones a caller created. The published entries are computed from
    /// the read-only catalog table on each call rather than inserted into the
    /// store, so reporting them writes nothing, and the merged set is ordered
    /// by package id, so two calls report the same order and a pagination
    /// token names a place in it.
    pub fn visible_packages(&self, region: &str) -> Vec<Package> {
        let mut visible = published_packages();
        visible.extend(self.packages_in(region).cloned());
        visible.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        visible
    }

    /// The visible package of one Region carrying `package_id` — one this
    /// account created there, else one the service publishes.
    pub fn visible_package(&self, region: &str, package_id: &str) -> Option<Package> {
        self.packages
            .get(&package_key(region, package_id))
            .cloned()
            .or_else(|| published_package(package_id))
    }

    /// The package of `region` carrying `package_name`, if the store holds one:
    /// a package name is unique among the packages of one Region, so this is
    /// what a create checks before minting another.
    pub fn package_named<'a>(&'a self, region: &'a str, package_name: &str) -> Option<&'a Package> {
        self.packages_in(region)
            .find(|package| package.package_name == package_name)
    }

    /// The domain the ARN names, if this store holds one.
    ///
    /// The tag operations address a domain by the ARN a create reported, and
    /// that ARN names its own Region, so the lookup is NOT scoped to the
    /// Region the call was signed for.
    pub fn domain_by_arn(&self, arn: &str) -> Option<&Domain> {
        self.domains.values().find(|domain| domain.arn == arn)
    }

    /// The same domain, to write its tag set through.
    pub fn domain_by_arn_mut(&mut self, arn: &str) -> Option<&mut Domain> {
        self.domains.values_mut().find(|domain| domain.arn == arn)
    }

    /// The associations of one domain, in package-id order.
    pub fn packages_of_domain<'a>(
        &'a self,
        region: &'a str,
        domain_name: &'a str,
    ) -> impl Iterator<Item = &'a DomainPackage> {
        self.package_associations
            .iter()
            .filter(move |((each, _, _), association)| {
                each == region && association.domain_name == domain_name
            })
            .map(|(_, association)| association)
    }

    /// The associations of one package, in domain-name order.
    pub fn domains_of_package<'a>(
        &'a self,
        region: &'a str,
        package_id: &'a str,
    ) -> impl Iterator<Item = &'a DomainPackage> {
        self.package_associations
            .iter()
            .filter(move |((each, package, _), _)| each == region && package == package_id)
            .map(|(_, association)| association)
    }

    /// One association as the `DomainPackageDetails` every association
    /// operation reports, or `None` when the package it names is no longer
    /// visible in that Region.
    pub fn association_details(
        &self,
        region: &str,
        association: &DomainPackage,
        now: SystemTime,
    ) -> Option<DomainPackageDetails> {
        self.visible_package(region, &association.package_id)
            .map(|package| association.to_details(&package, now))
    }
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's
/// 501 rather than a 5xx because there is nothing to retry: no engine of this
/// process will ever serve the call. An operation whose model declares a server
/// error answers with THAT instead.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the Elasticsearch emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// The message a `NextToken` that names no place in the set being paginated is
/// refused with, wherever an operation refuses one: an operation that models
/// `InvalidPaginationTokenException` carries it in that shape, and one that
/// models only `ValidationException` carries the same text in that one.
pub fn invalid_pagination_token_message(token: &str) -> String {
    format!("Invalid pagination token: {token}")
}

/// `InvalidPaginationTokenException` for a `NextToken` that names no place in the
/// set being paginated.
pub fn invalid_pagination_token(token: &str) -> InvalidPaginationTokenException {
    InvalidPaginationTokenException {
        message: Some(invalid_pagination_token_message(token)),
    }
}

/// The largest page a paginated operation of this service answers with, and the
/// page size a request that asks for none gets: this model's `MaxResults` is
/// `@range(max: 100)` and its documentation gives 100 as the unspecified case.
pub const MAX_PAGE: usize = 100;

/// Where the page a `NextToken` names starts: the position, in the set being
/// paginated, of the item the token names — 0 when the request carries no token,
/// and `None` when the token names no item of THAT set (one this service never
/// minted, or one carried over to a request with different filters), which is
/// what each operation refuses in the shape its own model declares.
///
/// A token names a PLACE in the set rather than an offset, so paging resumes
/// where the last page ended even though a later create may have grown the set.
pub fn page_start<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    token: Option<&str>,
) -> Option<usize> {
    match token {
        None => Some(0),
        Some(token) => ids.into_iter().position(|id| id == token),
    }
}

/// Where a page of `max_results` items starting at `start` ends, in a set of
/// `len` items.
///
/// The model defaults `MaxResults` to 0, so a request that asks for no
/// particular page size gets the whole page rather than none; the member
/// declares no minimum, so a negative value also reaches here, and it names no
/// page size either, so it is answered the same way rather than refused.
pub fn page_end(start: usize, max_results: i32, len: usize) -> usize {
    let size = match usize::try_from(max_results) {
        Ok(0) | Err(_) => MAX_PAGE,
        Ok(size) => size.min(MAX_PAGE),
    };
    start.saturating_add(size).min(len)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State>)
}
