// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudhsmv2-baldrapiservice service.
//!
//! This module owns the cluster/HSM records the implemented operations read and
//! write, the id minting that satisfies the modelled id patterns, and the single
//! place where a clock-owed status transition is materialised ([`settle`]).
//!
//! The network a cluster is placed in is NOT held here: a subnet is Amazon EC2's
//! resource, so [`ec2_subnets`] resolves what a request names against the service
//! that owns subnets and this module stores only what that answer said.
//!
//! [`settle`]: ComAmazonawsCloudhsmv2BaldrapiserviceState::settle

mod ec2_subnets;

pub use ec2_subnets::{
    common_vpc, resolve_requested_subnets, subnet_mapping_of, zone_named_twice, SubnetRecord,
};

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{BackupRetentionPolicy, Certificates, Cluster, Hsm, Tag};

/// The only value `BackupPolicy` models (`smithy_query(query="shape",shape="BackupPolicy")`
/// lists exactly `DEFAULT`), so every cluster reports it.
pub const BACKUP_POLICY_DEFAULT: &str = "DEFAULT";

/// The only value `BackupRetentionType` models
/// (`smithy_query(query="shape",shape="BackupRetentionPolicy")`).
pub const BACKUP_RETENTION_TYPE_DAYS: &str = "DAYS";

/// The retention window a cluster gets when the request names no
/// `BackupRetentionPolicy`: CloudHSM retains cluster backups for 90 days by
/// default.
pub const DEFAULT_BACKUP_RETENTION_DAYS: &str = "90";

/// The bounds the `BackupRetentionPolicy$Value` doc publishes for the `DAYS`
/// type: "Use a value between 7 - 379."
pub const BACKUP_RETENTION_DAYS_MIN: u32 = 7;
/// Upper bound of the published `BackupRetentionPolicy$Value` range.
pub const BACKUP_RETENTION_DAYS_MAX: u32 = 379;

/// The HSM types a new cluster can be created with.
///
/// The `HsmType` member doc publishes two ("The allowed values are hsm1.medium and
/// hsm2m.medium"), but `hsm1.medium` is retired: CloudHSM no longer provisions a cluster
/// of that type, so `CreateCluster` refuses one naming it instead of serving it. Only a
/// type in this catalog is accepted.
pub const HSM_TYPES: &[&str] = &["hsm2m.medium"];

/// The `NetworkType` a cluster gets when the request names none: the
/// `Cluster$NetworkType` doc says "The cluster's NetworkType can be IPv4 (the
/// default) or DUALSTACK".
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The `NetworkType` whose HSMs also carry an IPv6 ENI address.
pub const NETWORK_TYPE_DUALSTACK: &str = "DUALSTACK";

/// The `ClusterMode` a cluster gets when the request names none. CloudHSM
/// clusters are FIPS-mode unless the caller asks for `NON_FIPS`.
pub const DEFAULT_CLUSTER_MODE: &str = "FIPS";

/// The region used to name a cluster's Availability Zones when the request
/// carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a status transition this emulator owes
/// becomes due (`CREATE_IN_PROGRESS` -> `UNINITIALIZED`, `CREATE_IN_PROGRESS` ->
/// `ACTIVE`, `DELETE_IN_PROGRESS` -> gone). A chosen span, not an AWS-documented
/// duration: it only has to be non-zero, so a caller that acts straight after a
/// create or delete still observes the transient state.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(400);

/// The most clusters this emulator's store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and `CreateCluster` is the only thing that adds one, so the store needs a
/// maximum entry count: this is the emulator's OWN ceiling, well above AWS's own
/// per-region cluster quota, not a modelled limit.
pub const MAX_CLUSTERS: usize = 100;

/// The most HSMs one cluster holds — again the emulator's own ceiling, at AWS's
/// published maximum number of HSMs in a CloudHSM cluster. Without it a caller
/// that supplies its own `IpAddress` grows a cluster's HSM list without limit.
pub const MAX_HSMS_PER_CLUSTER: usize = 28;

/// How long a cluster stays readable in the terminal `DELETED` state before the
/// store reclaims it, so repeated create/delete cycles do not grow the store
/// monotonically. The emulator's own retention window, not an AWS-documented one;
/// it only has to outlast the `DELETE_IN_PROGRESS` span so a caller can observe
/// the terminal state.
pub const DELETED_CLUSTER_RETENTION: Duration = Duration::from_secs(2);

/// Cluster states this service reports. Spelled exactly as `ClusterState` models
/// them (`smithy_query(query="shape",shape="ClusterState")`).
pub const CLUSTER_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// A cluster that finished creating and is waiting for `InitializeCluster`.
pub const CLUSTER_UNINITIALIZED: &str = "UNINITIALIZED";
/// A cluster `DeleteCluster` has accepted.
pub const CLUSTER_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
/// A deleted cluster.
pub const CLUSTER_DELETED: &str = "DELETED";

/// HSM states this service reports, spelled as `HsmState` models them.
pub const HSM_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// An HSM that finished creating.
pub const HSM_ACTIVE: &str = "ACTIVE";
/// An HSM `DeleteHsm` has accepted.
pub const HSM_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// The alphabet the modelled cloudhsm id patterns accept — `[2-7a-zA-Z]` for
/// `^cluster-[2-7a-zA-Z]{11,16}$`, `^hsm-[2-7a-zA-Z]{11,16}$` and
/// `^backup-[2-7a-zA-Z]{11,16}$`.
const ID_ALPHABET: &[u8] = b"234567abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// How many alphabet characters a minted `cluster-`/`hsm-` id carries. Inside the
/// modelled 11..16 window.
const ID_BODY_LEN: usize = 11;

/// The direction tokens an Availability Zone name may carry, per the
/// `ExternalAz` pattern `^[a-z]{2}(-(gov))?-(east|west|north|south|central){1,2}-\d[a-z]$`.
const AZ_DIRECTIONS: &[&str] = &["east", "west", "north", "south", "central"];

/// What the clock owes a record once [`Pending::due`] passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owed {
    /// Move the record to this state.
    ToState(&'static str),
    /// Drop the record (a deleted HSM leaves its cluster's `Hsms` list).
    Removal,
}

/// A transition this emulator owes a record, and the instant it comes due. The
/// instant is stored, never slept on.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    /// When the transition is due.
    pub due: SystemTime,
    /// What is owed.
    pub owed: Owed,
}

impl Pending {
    /// A transition due [`TRANSITION_DELAY`] after `now`.
    pub fn after(now: SystemTime, owed: Owed) -> Self {
        Self {
            due: now + TRANSITION_DELAY,
            owed,
        }
    }
}

/// One HSM inside a cluster.
#[derive(Debug, Clone)]
pub struct HsmRecord {
    /// The HSM's id, matching `^hsm-[2-7a-zA-Z]{11,16}$`.
    pub hsm_id: String,
    /// The cluster that contains it.
    pub cluster_id: String,
    /// The AZ it was created in.
    pub availability_zone: String,
    /// The cluster subnet that maps to that AZ.
    pub subnet_id: String,
    /// The HSM's elastic network interface id.
    pub eni_id: String,
    /// The ENI's IPv4 address.
    pub eni_ip: String,
    /// The ENI's IPv6 address, for a `DUALSTACK` cluster only.
    pub eni_ip_v6: Option<String>,
    /// The cluster's HSM type, which every HSM in it shares.
    pub hsm_type: String,
    /// The HSM's state, one of `HsmState`.
    pub state: String,
    /// A description of that state.
    pub state_message: Option<String>,
    /// The transition the clock owes this HSM, if any.
    pub pending: Option<Pending>,
}

impl HsmRecord {
    /// This record as the modelled `Hsm` shape.
    pub fn to_shape(&self) -> Hsm {
        Hsm {
            availability_zone: Some(self.availability_zone.clone()),
            cluster_id: Some(self.cluster_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            eni_id: Some(self.eni_id.clone()),
            eni_ip: Some(self.eni_ip.clone()),
            eni_ip_v6: self.eni_ip_v6.clone(),
            hsm_id: self.hsm_id.clone(),
            hsm_type: Some(self.hsm_type.clone()),
            state: Some(self.state.clone()),
            state_message: self.state_message.clone(),
        }
    }

    /// Whether this HSM is on its way out, so it can no longer be selected for
    /// deletion.
    pub fn is_deleting(&self) -> bool {
        self.state == HSM_DELETE_IN_PROGRESS
    }
}

/// One CloudHSM cluster.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    /// The cluster's id, matching `^cluster-[2-7a-zA-Z]{11,16}$`.
    pub cluster_id: String,
    /// The HSM type every HSM in the cluster has.
    pub hsm_type: String,
    /// The cluster's state, one of `ClusterState`.
    pub state: String,
    /// A description of that state.
    pub state_message: Option<String>,
    /// When `CreateCluster` minted it.
    pub create_timestamp: SystemTime,
    /// AZ -> subnet, one subnet per AZ.
    pub subnet_mapping: HashMap<String, String>,
    /// The VPC the subnets live in.
    pub vpc_id: String,
    /// The cluster's security group.
    pub security_group: String,
    /// The cluster's backup policy.
    pub backup_policy: String,
    /// How long backups are retained.
    pub backup_retention_policy: BackupRetentionPolicy,
    /// `FIPS` or `NON_FIPS`.
    pub mode: String,
    /// `IPV4` or `DUALSTACK`.
    pub network_type: String,
    /// The backup the cluster was restored from, when it was.
    pub source_backup_id: Option<String>,
    /// The PRECO password, which only `InitializeCluster` can produce.
    pub pre_co_password: Option<String>,
    /// The cluster's certificates, which only an initialized cluster has. A cluster
    /// without them still REPORTS the member, as the empty structure
    /// ([`ClusterRecord::to_shape`]).
    pub certificates: Option<Certificates>,
    /// The HSMs in the cluster.
    pub hsms: Vec<HsmRecord>,
    /// The cluster's tags.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this cluster, if any.
    pub pending: Option<Pending>,
    /// When the store reclaims this deleted cluster. Set by `DeleteCluster`; until
    /// it passes the cluster stays readable in its terminal state.
    pub reclaim_at: Option<SystemTime>,
}

impl ClusterRecord {
    /// This record as the modelled `Cluster` shape, HSMs included.
    pub fn to_shape(&self) -> Cluster {
        Cluster {
            backup_policy: Some(self.backup_policy.clone()),
            backup_retention_policy: Some(self.backup_retention_policy.clone()),
            cluster_id: Some(self.cluster_id.clone()),
            create_timestamp: Some(::aws_smithy_types::DateTime::from(self.create_timestamp)),
            hsms: Some(self.hsms.iter().map(HsmRecord::to_shape).collect()),
            hsm_type: Some(self.hsm_type.clone()),
            // Only a cluster whose HSM type was modified can be rolled back, and
            // `ModifyCluster` is not implemented yet.
            hsm_type_rollback_expiration: None,
            pre_co_password: self.pre_co_password.clone(),
            security_group: Some(self.security_group.clone()),
            source_backup_id: self.source_backup_id.clone(),
            state: Some(self.state.clone()),
            state_message: self.state_message.clone(),
            subnet_mapping: Some(self.subnet_mapping.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            network_type: Some(self.network_type.clone()),
            // `Certificates` is reported for every cluster, whatever it holds: a cluster
            // that has not been initialized has none of them yet, which is the empty
            // structure rather than an absent member, so the usual "is the CSR available
            // yet?" read finds no CSR instead of finding no `Certificates` at all.
            certificates: Some(self.certificates.clone().unwrap_or_default()),
            // `TagList` is modelled `len 1..50`, so an untagged cluster reports no
            // list rather than an empty one.
            tag_list: (!self.tags.is_empty()).then(|| self.tags.clone()),
            mode: Some(self.mode.clone()),
        }
    }

    /// Whether the cluster has been deleted or is being deleted, so it is no
    /// longer a resource another operation can act on.
    pub fn is_gone(&self) -> bool {
        self.state == CLUSTER_DELETE_IN_PROGRESS || self.state == CLUSTER_DELETED
    }

    /// The position of `az` among this cluster's mapped Availability Zones, used
    /// to keep each AZ's synthesized addresses in their own /24.
    pub fn az_index(&self, az: &str) -> u8 {
        let mut zones: Vec<&String> = self.subnet_mapping.keys().collect();
        zones.sort();
        zones
            .iter()
            .position(|zone| zone.as_str() == az)
            .unwrap_or(0)
            .min(u8::MAX as usize) as u8
    }

    /// The lowest host number in this AZ's synthesized /24 that no HSM in the
    /// cluster holds, or `None` when every address in it is taken.
    ///
    /// The emulator models no VPC CIDRs, so an HSM the caller gave no address
    /// gets one from `10.0.<az>.0/24` (RFC 1918), skipping `.1`-`.3`, which AWS
    /// reserves in every subnet.
    pub fn free_eni_host(&self, az_index: u8) -> Option<u8> {
        (4u8..=254).find(|host| {
            let ip = format!("10.0.{az_index}.{host}");
            !self.hsms.iter().any(|hsm| hsm.eni_ip == ip)
        })
    }

    /// Whether any HSM in this cluster already holds `ip`.
    pub fn eni_ip_in_use(&self, ip: &str) -> bool {
        self.hsms.iter().any(|hsm| hsm.eni_ip == ip)
    }
}

/// Per-service state for com-amazonaws-cloudhsmv2-baldrapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudhsmv2BaldrapiserviceState {
    /// Every cluster this emulator holds, keyed by cluster id.
    pub clusters: HashMap<String, ClusterRecord>,
    /// Bumped once per minted id, so two ids minted in the same instant differ.
    pub id_counter: u64,
}

impl ComAmazonawsCloudhsmv2BaldrapiserviceState {
    /// Materialise every status transition the clock has already made due.
    ///
    /// Called at the top of every handler, including the reporting ones: the
    /// change was owed at `now` whether or not anybody called, so applying it
    /// here is what makes a read and a later read agree.
    pub fn settle(&mut self, now: SystemTime) {
        // A deleted cluster is readable for its retention window and then reclaimed,
        // so create/delete cycles do not grow the store monotonically.
        self.clusters
            .retain(|_, cluster| cluster.reclaim_at.is_none_or(|at| now < at));

        for cluster in self.clusters.values_mut() {
            if let Some(pending) = cluster.pending {
                if now >= pending.due {
                    cluster.pending = None;
                    if let Owed::ToState(next) = pending.owed {
                        cluster.state = next.to_string();
                        cluster.state_message = None;
                    }
                }
            }

            cluster.hsms.retain(|hsm| {
                !hsm.pending
                    .is_some_and(|pending| now >= pending.due && pending.owed == Owed::Removal)
            });

            for hsm in &mut cluster.hsms {
                if let Some(pending) = hsm.pending {
                    if now >= pending.due {
                        hsm.pending = None;
                        if let Owed::ToState(next) = pending.owed {
                            hsm.state = next.to_string();
                            hsm.state_message = None;
                        }
                    }
                }
            }
        }
    }

    /// The clusters this emulator holds, oldest first and ties broken by id, so
    /// paging over them is stable.
    pub fn ordered_clusters(&self) -> Vec<&ClusterRecord> {
        let mut clusters: Vec<&ClusterRecord> = self.clusters.values().collect();
        clusters.sort_by(|a, b| {
            a.create_timestamp
                .cmp(&b.create_timestamp)
                .then_with(|| a.cluster_id.cmp(&b.cluster_id))
        });
        clusters
    }

    /// The cluster `cluster_id` names, if this emulator holds it.
    pub fn cluster(&self, cluster_id: &str) -> Option<&ClusterRecord> {
        self.clusters.get(cluster_id)
    }

    /// 32 bytes derived from `domain`, a per-state counter and the wall clock, so
    /// every minted id is distinct from every other.
    fn digest(&mut self, domain: &str) -> [u8; 32] {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        hasher.update(self.id_counter.to_be_bytes());
        hasher.update(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes(),
        );
        hasher.finalize().into()
    }

    /// A fresh cluster id matching `^cluster-[2-7a-zA-Z]{11,16}$`.
    pub fn mint_cluster_id(&mut self) -> String {
        loop {
            let body = encode_in_id_alphabet(&self.digest("cluster"), ID_BODY_LEN);
            let id = format!("cluster-{body}");
            if !self.clusters.contains_key(&id) {
                return id;
            }
        }
    }

    /// A fresh HSM id matching `^hsm-[2-7a-zA-Z]{11,16}$`, unique across every
    /// cluster this emulator holds.
    pub fn mint_hsm_id(&mut self) -> String {
        loop {
            let body = encode_in_id_alphabet(&self.digest("hsm"), ID_BODY_LEN);
            let id = format!("hsm-{body}");
            let taken = self
                .clusters
                .values()
                .any(|cluster| cluster.hsms.iter().any(|hsm| hsm.hsm_id == id));
            if !taken {
                return id;
            }
        }
    }

    /// A fresh ENI id matching `^eni-[0-9a-fA-F]{8,17}$`.
    pub fn mint_eni_id(&mut self) -> String {
        let digest = self.digest("eni");
        format!("eni-{}", hex_body(&digest, 17))
    }

    /// A fresh security-group id matching `^sg-[0-9a-fA-F]{8,17}$`.
    pub fn mint_security_group(&mut self) -> String {
        let digest = self.digest("sg");
        format!("sg-{}", hex_body(&digest, 17))
    }
}

/// `len` characters of `digest`, each byte folded into [`ID_ALPHABET`].
fn encode_in_id_alphabet(digest: &[u8], len: usize) -> String {
    digest
        .iter()
        .take(len)
        .map(|byte| char::from(ID_ALPHABET[usize::from(*byte) % ID_ALPHABET.len()]))
        .collect()
}

/// `len` lowercase hex characters of `digest`.
fn hex_body(digest: &[u8], len: usize) -> String {
    let mut encoded = hex::encode(digest);
    encoded.truncate(len);
    encoded
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudhsmv2BaldrapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudhsmv2BaldrapiserviceState>)
}

/// The region a request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope or one whose region cannot name an Availability Zone
/// the `ExternalAz` pattern accepts.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| region_names_modelled_azs(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `<region><letter>` can satisfy the modelled `ExternalAz` pattern
/// `^[a-z]{2}(-(gov))?-(east|west|north|south|central){1,2}-\d[a-z]$`.
pub fn region_names_modelled_azs(region: &str) -> bool {
    let mut fields = region.split('-').peekable();
    let Some(country) = fields.next() else {
        return false;
    };
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    if fields.peek().is_some_and(|field| *field == "gov") {
        fields.next();
    }
    let mut directions = 0;
    while fields
        .peek()
        .is_some_and(|field| AZ_DIRECTIONS.contains(field))
    {
        fields.next();
        directions += 1;
    }
    if !(1..=2).contains(&directions) {
        return false;
    }
    let Some(number) = fields.next() else {
        return false;
    };
    number.len() == 1 && number.as_bytes()[0].is_ascii_digit() && fields.next().is_none()
}

/// The IPv6 address an HSM's ENI carries in a `DUALSTACK` cluster.
///
/// The emulator models no VPC CIDRs, so the address is synthesized from the same
/// two values the IPv4 one is — the Availability Zone's position in the cluster's
/// subnet mapping and the ENI's host number — inside `2001:db8::/32`, the prefix
/// RFC 3849 reserves for documentation, so it can never name real infrastructure.
pub fn synthesized_eni_ip_v6(az_index: u8, eni_ip: &str) -> String {
    let host: u32 = eni_ip
        .rsplit('.')
        .next()
        .and_then(|octet| octet.parse().ok())
        .unwrap_or_else(|| u32::from(az_index));
    format!("2001:db8:0:{az_index:x}::{host:x}")
}

/// A `NextToken` that resumes a paged describe at `cluster_id`.
///
/// Hex-encoded rather than returned raw so a client cannot read a resource id
/// out of a pagination token, and so a token this emulator did not mint is
/// refused rather than silently treated as a cluster id. `NextToken` is modelled
/// `len 0..256`, which a hex-encoded cluster id stays well inside.
pub fn encode_next_token(cluster_id: &str) -> String {
    hex::encode(cluster_id)
}

/// The cluster id a `NextToken` resumes at, or `None` when the token was not
/// minted by [`encode_next_token`].
pub fn decode_next_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let id = String::from_utf8(bytes).ok()?;
    (!id.is_empty()).then_some(id)
}
