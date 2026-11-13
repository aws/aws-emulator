// SPDX-License-Identifier: CC0-1.0

//! The members a cluster record reports that are COMPUTED rather than stored:
//! its ARNs, its endpoint address, its Availability Zones, its nodes, its keys
//! and the next maintenance window, plus the digest helpers each is derived
//! with.

use super::*;

/// The region this request was signed for, from its SigV4 credential scope.
///
/// Falls back to `us-east-1`, the region a generated emulator presents when a
/// request carries no scope to read one from.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// `n` lowercase hex characters of the SHA-256 of `seed`.
///
/// Used only to derive the stable, content-addressed parts of an endpoint
/// hostname and a namespace id: the same cluster always renders the same value,
/// so nothing here is a per-run value.
pub(super) fn short_digest(seed: &str, n: usize) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    hex::encode(digest).chars().take(n).collect()
}

/// The SHA-256 of `seed` rendered in the 8-4-4-4-12 lowercase-hex form AWS
/// identifies a reserved node offering and a namespace by.
///
/// Content-addressed from the resource's own identity, so the same resource
/// always renders the same value and nothing here is a per-run value.
pub(super) fn content_uuid(seed: &str) -> String {
    let hexed = short_digest(seed, 32);
    format!(
        "{}-{}-{}-{}-{}",
        &hexed[0..8],
        &hexed[8..12],
        &hexed[12..16],
        &hexed[16..20],
        &hexed[20..32]
    )
}

/// The first byte of the SHA-256 of `seed`, for picking a stable default out of
/// a documented set.
pub(super) fn digest_byte(seed: &str) -> u8 {
    Sha256::digest(seed.as_bytes())[0]
}

/// The DNS address of a cluster's connection endpoint, in Redshift's
/// `<cluster>.<id>.<region>.redshift.amazonaws.com` form.
///
/// Derived from the cluster identifier and region, so renaming a cluster moves
/// its endpoint exactly as the real hostname does.
pub fn endpoint_address(cluster_identifier: &str, region: &str) -> String {
    let token = short_digest(&format!("endpoint/{cluster_identifier}/{region}"), 12);
    format!("{cluster_identifier}.{token}.{region}.redshift.amazonaws.com")
}

/// The namespace ARN of a cluster: `arn:aws:redshift:<region>:<account>:namespace:<uuid>`.
///
/// Computed ONCE, when the cluster is created, and then persisted on the stored
/// cluster — the namespace identifies the cluster's data, so it must survive a
/// rename.
pub fn namespace_arn(cluster_identifier: &str, region: &str) -> String {
    let hexed = short_digest(&format!("namespace/{cluster_identifier}/{region}"), 32);
    let uuid = format!(
        "{}-{}-{}-{}-{}",
        &hexed[0..8],
        &hexed[8..12],
        &hexed[12..16],
        &hexed[16..20],
        &hexed[20..32]
    );
    engine::emulator_arn("redshift", region, &format!("namespace:{uuid}"))
}

/// The ARN of a snapshot: `arn:aws:redshift:<region>:<account>:snapshot:<cluster>/<snapshot>`.
pub fn snapshot_arn(cluster_identifier: &str, snapshot_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("snapshot:{cluster_identifier}/{snapshot_identifier}"),
    )
}

/// The ARN of a cluster: `arn:aws:redshift:<region>:<account>:cluster:<cluster>`.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Constructing an Amazon
/// Redshift Amazon Resource Name (ARN)" (the page
/// `smithy_query(query="doc", shape="TaggedResource", member="ResourceType")`
/// links to) — `arn:aws:redshift:region:account-id:cluster:cluster-name` — which
/// is also the example on `smithy_query(query="doc",
/// shape="CreateTagsMessage", member="ResourceName")`:
/// "arn:aws:redshift:us-east-2:123456789:cluster:t1".
pub fn cluster_arn(cluster_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_CLUSTER}:{cluster_identifier}"),
    )
}

/// The ARN of a cluster parameter group:
/// `arn:aws:redshift:<region>:<account>:parametergroup:<name>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:parametergroup:parameter-group-name`.
pub fn parameter_group_arn(parameter_group_name: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_PARAMETER_GROUP}:{parameter_group_name}"),
    )
}

/// The ARN of a cluster subnet group:
/// `arn:aws:redshift:<region>:<account>:subnetgroup:<name>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:subnetgroup:subnet-group-name`.
pub fn subnet_group_arn(subnet_group_name: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_SUBNET_GROUP}:{subnet_group_name}"),
    )
}

/// The ARN of an HSM configuration:
/// `arn:aws:redshift:<region>:<account>:hsmconfiguration:<identifier>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:hsmconfiguration:hsm-configuration-name`.
pub fn hsm_configuration_arn(hsm_configuration_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_HSM_CONFIGURATION}:{hsm_configuration_identifier}"),
    )
}

/// The ARN of the Secrets Manager secret holding a cluster's admin credentials,
/// used only when `ManageMasterPassword` is on.
pub fn master_password_secret_arn(cluster_identifier: &str, region: &str) -> String {
    let token = short_digest(&format!("secret/{cluster_identifier}/{region}"), 6);
    engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:redshift!{cluster_identifier}-{token}"),
    )
}

/// The connection endpoint a cluster serves on: its DNS address, the port it
/// listens on and the VPC interface endpoint it is reached through.
///
/// The ONE rule for the whole `Endpoint` structure, so the create, the modify
/// and the restore paths cannot build one differently. The endpoint is a
/// property of the cluster's own configuration — its identifier, its Region, the
/// port it was asked for, and the VPC and Availability Zone its subnet group put
/// it in — so it is DECIDED when the cluster is allocated and
/// [`StoredCluster::endpoint`] holds it from then on, whether or not the cluster
/// is serving yet (see [`StoredCluster::publish_endpoint`]).
///
/// `vpc_id` is the VPC the cluster's subnet group placed it in, as that group's
/// stored record reports it — `None` where no subnet group placed the cluster in
/// a VPC at all. An interface endpoint lives IN a VPC, so a cluster in none is
/// reached at its address alone and reports no `VpcEndpoints`
/// (`smithy_query(query="doc", shape="Endpoint", member="VpcEndpoints")`).
pub fn cluster_endpoint(
    cluster_identifier: &str,
    region: &str,
    port: i32,
    vpc_id: Option<&str>,
    availability_zone: &str,
) -> Endpoint {
    Endpoint {
        address: Some(endpoint_address(cluster_identifier, region)),
        port: Some(port),
        vpc_endpoints: vpc_id
            .map(|vpc_id| cluster_vpc_endpoints(cluster_identifier, vpc_id, availability_zone)),
    }
}

/// The Availability Zones of one Region, in name order.
///
/// A Region's zones are `<region>` followed by consecutive letters from `a`, as
/// many as that Region publishes ([`region_zone_count`]). Derived from the
/// Region alone, so the same Region always reports the same zones.
pub fn region_availability_zones(region: &str) -> Vec<String> {
    (0..region_zone_count(region))
        .map(|index| format!("{region}{}", ZONE_LETTERS[index]))
        .collect()
}

/// The letters a zone name may end in, in order; a Region uses the first
/// [`region_zone_count`] of them.
pub(super) const ZONE_LETTERS: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

/// How many Availability Zones AWS publishes for one Region.
///
/// Source: the AWS global-infrastructure reference "Regions, Availability Zones,
/// and Local Zones" (also `aws ec2 describe-availability-zones` per Region): the
/// count differs BY REGION, so a caller choosing a zone from a Region's list
/// gets that Region's zones and not a fixed three. [`REGION_ZONE_COUNTS`] names
/// every Region that publishes more than [`DEFAULT_ZONE_COUNT`]; any other
/// Region publishes that default, which is the smallest count AWS publishes for
/// a Region.
pub fn region_zone_count(region: &str) -> usize {
    REGION_ZONE_COUNTS
        .iter()
        .find(|(named, _)| *named == region)
        .map_or(DEFAULT_ZONE_COUNT, |(_, count)| *count)
}

/// The number of Availability Zones a Region publishes unless
/// [`REGION_ZONE_COUNTS`] names it.
pub const DEFAULT_ZONE_COUNT: usize = 3;

/// Every Region that publishes more Availability Zones than
/// [`DEFAULT_ZONE_COUNT`], with the number it publishes.
///
/// Source: as [`region_zone_count`] — the AWS Regions and Availability Zones
/// reference. Kept to the Regions whose count differs from the default, so a
/// Region added to AWS later still reports a well-formed zone list.
pub const REGION_ZONE_COUNTS: [(&str, usize); 4] = [
    ("us-east-1", 6),
    ("us-west-2", 4),
    ("ap-northeast-1", 4),
    ("ap-northeast-2", 4),
];

/// Whether `value` has the form of an Amazon Web Services Region code, which is
/// what makes it the name of a Region at all.
///
/// A Region code is an AREA code ([`REGION_AREA_CODES`]), then — in the
/// partitions that have one — a partition qualifier, then the LOCATION within
/// the area ([`REGION_LOCATIONS`]), then the Region's number, counting from 1:
/// `us-east-1`, `eu-west-3`, `ap-southeast-4`, `us-gov-west-1`, `cn-north-1`,
/// `us-iso-east-1`.
///
/// Source: the Amazon Web Services General Reference's Region code list (the
/// "Regions and Endpoints" reference that
/// `smithy_query(query="doc", shape="EnableSnapshotCopyMessage",
/// member="DestinationRegion")` points at for "Must be the name of a valid
/// Amazon Web Services Region").
///
/// Decided from the form and those two published vocabularies rather than from a
/// closed list of today's Region codes, for the same reason
/// [`region_zone_count`] derives a Region's zones from its name: AWS adds
/// Regions, and one added after this emulator was built is still a Region. A
/// value outside the form names no Region at all.
pub fn names_a_region(value: &str) -> bool {
    let segments: ::std::vec::Vec<&str> = value.split('-').collect();
    let [area, qualifiers @ .., location, number] = segments.as_slice() else {
        return false;
    };
    REGION_AREA_CODES.contains(area)
        // At most one qualifier, and a qualifier is a partition name: letters.
        && qualifiers.len() <= 1
        && qualifiers.iter().all(|qualifier| {
            !qualifier.is_empty() && qualifier.bytes().all(|byte| byte.is_ascii_lowercase())
        })
        && REGION_LOCATIONS.contains(location)
        // The Region number is a decimal count from 1, so neither an empty
        // segment, a signed one, a zero nor a zero-padded one is one.
        && !number.starts_with('0')
        && number.parse::<u32>().is_ok_and(|ordinal| ordinal >= 1)
}

/// The area codes an Amazon Web Services Region code starts with.
///
/// Source: as [`names_a_region`] — the published Region code list, reduced to
/// the distinct area codes it uses.
pub const REGION_AREA_CODES: [&str; 10] = [
    "af", "ap", "ca", "cn", "eu", "il", "me", "mx", "sa", "us",
];

/// The location words an Amazon Web Services Region code names inside its area.
///
/// Source: as [`REGION_AREA_CODES`].
pub const REGION_LOCATIONS: [&str; 9] = [
    "central",
    "east",
    "north",
    "northeast",
    "northwest",
    "south",
    "southeast",
    "southwest",
    "west",
];

/// The only platform a cluster can be launched on today.
///
/// Source: `smithy_query(query="shape", shape="SupportedPlatform")` — "A list of
/// supported platforms for orderable clusters". EC2-Classic is retired, so every
/// zone a cluster is orderable in supports exactly the VPC platform.
pub const PLATFORM_VPC: &str = "VPC";

/// The platforms a cluster placed in an Availability Zone can run on.
pub fn zone_supported_platforms() -> Vec<SupportedPlatform> {
    [PLATFORM_VPC]
        .into_iter()
        .map(|name| SupportedPlatform {
            name: Some(name.to_string()),
        })
        .collect()
}

/// The platform a cluster ran on OUTSIDE a VPC, before EC2-Classic was retired.
///
/// Named here only so the capabilities that existed for that platform alone can
/// be decided from the published platform list rather than from a flag: EC2's
/// `supported-platforms` account attribute reports `EC2` for EC2-Classic and
/// `VPC` for the VPC platform, and [`zone_supported_platforms`] publishes only
/// the latter.
pub const PLATFORM_EC2_CLASSIC: &str = "EC2";

/// Whether this service offers the platform a cluster security group governs.
///
/// A cluster security group is EC2-Classic's access-control mechanism: it
/// "control[s] access to non-VPC clusters"
/// (`smithy_query(query="doc", shape="CreateClusterSecurityGroup")`), and a
/// cluster inside a VPC is governed by VPC security groups
/// (`Cluster$VpcSecurityGroups`) instead. So the question "can this account hold
/// a cluster security group at all?" is the question "does any zone it can place
/// a cluster in offer the EC2-Classic platform?", answered from the one
/// published platform list rather than from a second, independently maintained
/// fact.
pub fn cluster_security_groups_available() -> bool {
    zone_supported_platforms()
        .iter()
        .any(|platform| platform.name.as_deref() == Some(PLATFORM_EC2_CLASSIC))
}

/// Why any call in the cluster-security-group family is refused.
///
/// The text says the capability is gone and names the one that replaced it,
/// because that — not any value the request carried — is what the caller has to
/// act on: no name, description, rule or tag list could make such a call
/// succeed. Only the status (400) and the error code (`InvalidParameterValue`)
/// are wire requirements; this sentence is the service's own standing notice,
/// identical for every request and every operation of the family, which is why
/// it lives here once rather than being spelled at a call site.
pub const CLUSTER_SECURITY_GROUPS_UNAVAILABLE: &str =
    "Amazon Redshift has discontinued cluster security groups. For security group \
     configuration, associate VPC security groups when creating or modifying your cluster.";

/// One Availability Zone as this service reports it: its name AND the platforms
/// a cluster in it can run on.
///
/// `AvailabilityZone` is `{Name, SupportedPlatforms}`
/// (`smithy_query(query="shape", shape="AvailabilityZone")`), and a client
/// branching on platform support reads the second member — so every
/// `AvailabilityZone` this service emits is built here rather than from the name
/// alone.
pub fn availability_zone(name: &str) -> AvailabilityZone {
    AvailabilityZone {
        name: Some(name.to_string()),
        supported_platforms: Some(zone_supported_platforms()),
    }
}

/// The Availability Zones of one Region as modelled records, in name order:
/// [`region_availability_zones`] through [`availability_zone`].
pub fn region_availability_zone_records(region: &str) -> Vec<AvailabilityZone> {
    region_availability_zones(region)
        .iter()
        .map(|name| availability_zone(name))
        .collect()
}

/// The Availability Zone Redshift picks when the request names none.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AvailabilityZone")` — "Default: A random, system-chosen Availability
/// Zone in the region that is specified by the endpoint." The emulator's choice
/// is derived from the cluster identifier rather than drawn per run, so it is
/// stable across reads.
pub fn default_availability_zone(cluster_identifier: &str, region: &str) -> String {
    let zones = region_availability_zones(region);
    let index = digest_byte(&format!("az/{cluster_identifier}")) as usize % zones.len();
    zones[index].clone()
}

/// Another Availability Zone of the same Region as `primary`, which is where a
/// Multi-AZ cluster's SECOND compute unit is.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="MultiAZ")` — "a
/// boolean value that, if true, indicates that the cluster is deployed in TWO
/// Availability Zones" — so the secondary zone is never the primary one. A
/// zone's name is its Region's code followed by the zone letter (see
/// [`region_availability_zones`]), so the Region a zone belongs to is its name
/// without that letter, and the choice among that Region's remaining zones is
/// derived from the cluster identifier rather than drawn per run: the same
/// cluster always reports the same secondary zone.
pub fn secondary_availability_zone(cluster_identifier: &str, primary: &str) -> String {
    let region = match primary.char_indices().next_back() {
        Some((index, last)) if ZONE_LETTERS.contains(&last) => &primary[..index],
        _ => primary,
    };
    let zones: Vec<String> = region_availability_zones(region)
        .into_iter()
        .filter(|zone| zone != primary)
        .collect();
    // A Region publishes at least [`DEFAULT_ZONE_COUNT`] zones, so removing the
    // primary one always leaves at least one to place the secondary unit in.
    let Some(count) = ::std::num::NonZeroUsize::new(zones.len()) else {
        return primary.to_string();
    };
    let index = digest_byte(&format!("multi-az-secondary/{cluster_identifier}")) as usize
        % count.get();
    zones[index].clone()
}

/// The 30-minute weekly maintenance window Redshift assigns when the request
/// names none, in the `ddd:hh24:mi-ddd:hh24:mi` form Redshift reports.
///
/// Derived from the cluster identifier, so it is stable across reads.
pub fn default_maintenance_window(cluster_identifier: &str) -> String {
    let days = MAINTENANCE_WINDOW_DAYS;
    let seed = digest_byte(&format!("window/{cluster_identifier}"));
    let day = days[usize::from(seed) % days.len()];
    // 0..=22, so the window's end hour is always a valid same-day hour.
    let hour = (usize::from(seed) / days.len()) % 23;
    format!("{day}:{hour:02}:30-{day}:{:02}:00", hour + 1)
}

/// The cluster subnet group a cluster joins when the request names none.
///
/// Source: `smithy_query(query="doc", operation="CreateCluster")` — "To create a
/// cluster in Virtual Private Cloud (VPC), you must provide a cluster subnet
/// group name. The cluster subnet group identifies the subnets of your VPC that
/// Amazon Redshift uses when creating the cluster." Amazon Redshift creates every
/// provisioned cluster in a VPC and an account that names no subnet group gets
/// its DEFAULT one, which Redshift names `default` — so a cluster's
/// `ClusterSubnetGroupName` and, with it, its `VpcId` are always reported.
pub const DEFAULT_CLUSTER_SUBNET_GROUP: &str = "default";

/// The `VpcSecurityGroupMembership$Status` of a security group a cluster is in.
pub(super) const SECURITY_GROUP_STATUS_ACTIVE: &str = "active";

/// The VPC security group a cluster joins when the request names none: the
/// default security group of the VPC its subnet group is in.
///
/// Derived from the VPC id, so every cluster of one VPC reports the same default
/// group, exactly as the VPC's own default group is shared.
pub fn default_vpc_security_group_id(vpc_id: &str) -> String {
    format!("sg-{}", short_digest(&format!("sg/{vpc_id}"), 17))
}

/// The `KmsKeyId` of a cluster encrypted with the key Amazon Web Services owns
/// rather than a customer managed key.
///
/// `smithy_query(query="doc", shape="Cluster", member="KmsKeyId")` — "The Key
/// Management Service (KMS) key ID of the encryption key used to encrypt data in
/// the cluster": an encrypted cluster always has one, and a cluster whose request
/// named no `KmsKeyId` is encrypted under the service-owned key, which Redshift
/// reports by this id rather than by an ARN.
pub const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// The `KmsKeyId` a snapshot copy grant carries when its create named none.
///
/// Source: `smithy_query(query="doc", shape="CreateSnapshotCopyGrantMessage",
/// member="KmsKeyId")` — "If no key is specified, the default key is used." The
/// default key of a Redshift account is the Amazon Web Services managed key for
/// this service, which callers name by its alias, so that alias is the key such
/// a grant permits Redshift to use. Resolved once at CREATE time and stored on
/// the grant, so every later read of it reports the same key.
pub const DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY: &str = "alias/aws/redshift";

/// What a `PendingModifiedValues$MasterUserPassword` carries while an admin
/// password change is still being applied: the member is `@sensitive`, so the
/// pending change is reported as a MASK and the password itself never reaches the
/// wire.
pub const REDACTED_MASTER_USER_PASSWORD: &str = "****";

/// `ClusterNode$NodeRole` of the one node of a single-node cluster, which is its
/// own leader and its own compute node.
pub const NODE_ROLE_SHARED: &str = "SHARED";
/// `ClusterNode$NodeRole` of the node that runs the query planner of a
/// multi-node cluster.
pub const NODE_ROLE_LEADER: &str = "LEADER";

/// The nodes of a cluster, in the order Redshift reports them.
///
/// Source: `smithy_query(query="doc", shape="ClusterNode", member="NodeRole")` —
/// "Whether the node is a leader node or a compute node", and
/// `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterType")`
/// — "When cluster type is specified as single-node, the NumberOfNodes parameter
/// is not required". So a `single-node` cluster is ONE node that is both roles at
/// once ([`NODE_ROLE_SHARED`]), and a `multi-node` cluster is its leader node
/// ([`NODE_ROLE_LEADER`]) followed by its `NumberOfNodes` compute nodes, each
/// numbered from zero.
///
/// The addresses are derived from the cluster identifier and the node's position,
/// so a cluster always reports the same topology and no address is a per-run
/// value.
///
/// The loop is bounded by [`MAX_NODES_PER_CLUSTER`] whatever it is handed, so the
/// per-node work this does can never be sized by a request: a node count above
/// that bound is refused with the modelled
/// `NumberOfNodesPerClusterLimitExceededFault` before any cluster is written (see
/// [`exceeds_nodes_per_cluster_limit`]), and this clamp keeps the bound at the
/// allocation site for every other caller.
pub fn cluster_nodes(
    cluster_identifier: &str,
    cluster_type: &str,
    number_of_nodes: i32,
) -> Vec<ClusterNode> {
    let node = |role: String, index: i32| ClusterNode {
        node_role: Some(role),
        private_ip_address: Some(derived_ipv4(
            10,
            &format!("private/{cluster_identifier}/{index}"),
        )),
        public_ip_address: Some(derived_ipv4(
            54,
            &format!("public/{cluster_identifier}/{index}"),
        )),
    };
    if cluster_type == CLUSTER_TYPE_SINGLE_NODE || number_of_nodes <= 1 {
        return vec![node(NODE_ROLE_SHARED.to_string(), 0)];
    }
    let compute_nodes = number_of_nodes.clamp(2, MAX_NODES_PER_CLUSTER);
    let mut nodes = Vec::with_capacity(compute_nodes as usize + 1);
    nodes.push(node(NODE_ROLE_LEADER.to_string(), 0));
    for index in 0..compute_nodes {
        nodes.push(node(format!("COMPUTE-{index}"), index + 1));
    }
    nodes
}

/// The nodes of the SECONDARY compute unit of a Multi-AZ cluster, in the order
/// Redshift reports them.
///
/// Source: `smithy_query(query="shape", shape="SecondaryClusterInfo")` — "the
/// nodes in the secondary compute unit". A Multi-AZ cluster runs the same
/// topology twice, so the second unit has the same roles and the same count as
/// the primary one ([`cluster_nodes`]) with addresses of its own: the seed names
/// the unit, so the two units of one cluster never report the same node.
pub fn secondary_cluster_nodes(
    cluster_identifier: &str,
    cluster_type: &str,
    number_of_nodes: i32,
) -> Vec<ClusterNode> {
    cluster_nodes(
        &format!("{cluster_identifier}/secondary"),
        cluster_type,
        number_of_nodes,
    )
}

/// The largest `NumberOfNodes` one cluster may have.
///
/// Amazon Redshift bounds the compute nodes of a provisioned cluster by node
/// type, and 128 is the largest count any node type offers (the published
/// "Nodes per cluster" quota of the largest RA3 and DC2 node types), so it is the
/// bound this service enforces for every node type rather than a per-type table
/// it would have to invent. A request above it is refused with the operation's
/// own modelled `NumberOfNodesPerClusterLimitExceededFault` — "The operation
/// would exceed the number of nodes allowed for a cluster" — so no request can
/// size the work a handler does.
pub const MAX_NODES_PER_CLUSTER: i32 = 128;

/// Whether a request's `NumberOfNodes` is above [`MAX_NODES_PER_CLUSTER`], which
/// every operation that writes a cluster checks BEFORE it derives anything from
/// the count.
pub fn exceeds_nodes_per_cluster_limit(number_of_nodes: i32) -> bool {
    number_of_nodes > MAX_NODES_PER_CLUSTER
}

/// The refusal a `NumberOfNodes` above [`MAX_NODES_PER_CLUSTER`] carries.
pub fn nodes_per_cluster_limit_message(requested: i32) -> String {
    format!(
        "The operation would exceed the number of nodes allowed for a cluster ({MAX_NODES_PER_CLUSTER}): {requested} were requested."
    )
}

/// A dotted-quad IPv4 address in the `first`.0.0.0/8 space, derived from `seed`.
///
/// The last octet is kept inside 1..=254, so the address is never a network or a
/// broadcast address of its /24.
pub(super) fn derived_ipv4(first: u8, seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    format!(
        "{first}.{}.{}.{}",
        digest[0],
        digest[1],
        1 + u32::from(digest[2]) % 254
    )
}

/// The SSH public key of a cluster, in the `ssh-rsa <key> Amazon-Redshift` form
/// Redshift reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="ClusterPublicKey")`
/// — "The public key for the cluster". The key material is generated per cluster
/// by the service; here it is DERIVED from the cluster identifier so the same
/// cluster always reports the same key and nothing is drawn per run. The body
/// opens with the fixed prefix every `ssh-rsa` key carries.
pub fn cluster_public_key(cluster_identifier: &str) -> String {
    let body = short_digest(&format!("public-key/{cluster_identifier}"), 48);
    format!("ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB{body}= Amazon-Redshift")
}

/// The database revision a cluster runs, as the six-digit number Redshift
/// reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="ClusterRevisionNumber")` — "The specific revision number of the
/// database in the cluster". Derived from the engine version and the cluster, so
/// two clusters of one version can report different revisions (the service
/// patches them independently) and a cluster's own revision is stable across
/// reads.
pub fn cluster_revision_number(cluster_identifier: &str, cluster_version: &str) -> String {
    let digest = Sha256::digest(format!("revision/{cluster_version}/{cluster_identifier}"));
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{}", 100_000 + value % 900_000)
}

/// The VPC interface endpoint a VPC-resident cluster is reached through, with the
/// network interface it has in the cluster's Availability Zone.
///
/// Source: `smithy_query(query="doc", shape="Endpoint", member="VpcEndpoints")`
/// and `smithy_query(query="doc", shape="VpcEndpoint")` — "The connection
/// endpoint for connecting to an Amazon Redshift cluster through the proxy",
/// whose `NetworkInterfaces` are "one or more network interfaces of the endpoint.
/// Also known as an interface endpoint." Every id is derived from the cluster and
/// its VPC, so the topology is stable across reads.
pub fn cluster_vpc_endpoints(
    cluster_identifier: &str,
    vpc_id: &str,
    availability_zone: &str,
) -> Vec<VpcEndpoint> {
    vec![vpc_interface_endpoint(
        cluster_identifier,
        vpc_id,
        availability_zone,
        None,
    )]
}

/// ONE VPC interface endpoint, as [`cluster_vpc_endpoints`] reports it, for the
/// resource `owner` names.
///
/// The same derivation serves a cluster's own endpoint and a Redshift-managed
/// VPC endpoint of that cluster (`EndpointAccess$VpcEndpoint`), because both are
/// the same kind of thing — "the connection endpoint for connecting to an Amazon
/// Redshift cluster through the proxy" — so `owner` is whichever resource the
/// interface belongs to and every id stays derived from it. `subnet_id` is the
/// subnet the interface was placed in where the caller KNOWS it (an endpoint
/// deployed into a subnet of its subnet group); `None` derives it from the VPC
/// and the zone, which is what a cluster whose subnets this service does not
/// enumerate per interface reports.
pub fn vpc_interface_endpoint(
    owner: &str,
    vpc_id: &str,
    availability_zone: &str,
    subnet_id: Option<&str>,
) -> VpcEndpoint {
    VpcEndpoint {
        vpc_endpoint_id: Some(format!(
            "vpce-{}",
            short_digest(&format!("vpce/{owner}/{vpc_id}"), 17)
        )),
        vpc_id: Some(vpc_id.to_string()),
        network_interfaces: Some(vec![NetworkInterface {
            network_interface_id: Some(format!(
                "eni-{}",
                short_digest(&format!("eni/{owner}/{availability_zone}"), 17)
            )),
            subnet_id: Some(subnet_id.map_or_else(
                || {
                    format!(
                        "subnet-{}",
                        short_digest(&format!("subnet/{vpc_id}/{availability_zone}"), 17)
                    )
                },
                str::to_string,
            )),
            private_ip_address: Some(derived_ipv4(
                10,
                &format!("eni-address/{owner}/{availability_zone}"),
            )),
            availability_zone: Some(availability_zone.to_string()),
            // The emulator presents IPv4 subnets only, so an endpoint interface
            // has no IPv6 address to report.
            ipv6_address: None,
        }]),
    }
}

/// The `AquaConfiguration` every cluster reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="AquaConfiguration")`
/// — "This field is retired. Amazon Redshift automatically determines whether to
/// use AQUA (Advanced Query Accelerator)", and the identical sentence on
/// `CreateClusterMessage$AquaConfigurationStatus`: "This parameter is retired. It
/// does not set the AQUA configuration status." So the configuration status a
/// request asks for is IGNORED and the cluster reports the automatic
/// configuration (`auto`, of the `AquaConfigurationStatus` enum) with AQUA not in
/// use (`disabled`, of the `AquaStatus` enum).
pub fn retired_aqua_configuration() -> AquaConfiguration {
    AquaConfiguration {
        aqua_status: Some("disabled".to_string()),
        aqua_configuration_status: Some("auto".to_string()),
    }
}

/// What a `ModifyAquaConfiguration` call is answered with.
///
/// The operation is RETIRED — "this operation is retired. Calling this operation
/// does not change AQUA configuration. Amazon Redshift automatically determines
/// whether to use AQUA" (`smithy_query(query="doc",
/// operation="ModifyAquaConfiguration")`) — and the recorded Amazon Redshift
/// answer to a call of it is the service's internal-failure refusal, carrying
/// this sentence verbatim: there is no backend left behind the retired path, so
/// the refusal is decided before the request is looked at and names nothing from
/// it. The code (`InternalFailure`) and the status (500) are the common AWS
/// ones, which the model does not list for this operation (its faults are
/// `ClusterNotFoundFault`, `InvalidClusterStateFault` and
/// `UnsupportedOperationFault`), so both come from the recording.
pub const AQUA_CONFIGURATION_INTERNAL_FAILURE: &str =
    "An internal error has occurred. Please try your query again at a later time.";

/// The next UTC instant at which a cluster's weekly maintenance window opens,
/// strictly after `after`.
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="NextMaintenanceWindowStartTime")` — "The date and time in UTC when
/// system maintenance can begin" — computed from the cluster's own
/// `PreferredMaintenanceWindow`, whose documented form is
/// `ddd:hh24:mi-ddd:hh24:mi` (`smithy_query(query="doc",
/// shape="CreateClusterMessage", member="PreferredMaintenanceWindow")`). `None`
/// when the window is not in that form, so an unparsable window reports no next
/// start rather than a made-up one.
///
/// Every value is bounded before it is used and every instant is computed with
/// CHECKED arithmetic, so no window a request can carry — and no instant the
/// clock can reach — overflows or panics: an input that would is one this
/// reports no next start for.
pub fn next_maintenance_window_start(
    window: &str,
    after: ::aws_smithy_types::DateTime,
) -> Option<::aws_smithy_types::DateTime> {
    const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

    let start = window.split('-').next()?;
    let mut parts = start.split(':');
    let day = parts.next()?.to_ascii_lowercase();
    // Parsed UNSIGNED and bounded on both ends: `hh24` is 00..=23 and `mi` is
    // 00..=59, so a negative or out-of-range field is a window this reports
    // nothing for rather than a value the arithmetic below has to survive.
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let second_of_day = i64::from(hour) * 3600 + i64::from(minute) * 60;
    let target_day = MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|candidate| *candidate == day)? as i64;

    let after_secs = after.secs();
    // Epoch day 0 (1970-01-01) was a Thursday, index 3 of a Monday-first week.
    let day_index = after_secs.div_euclid(SECONDS_PER_DAY);
    let weekday = (day_index + 3).rem_euclid(7);
    let ahead = (target_day - weekday).rem_euclid(7);
    let mut start_secs = day_index
        .checked_add(ahead)?
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(second_of_day)?;
    if start_secs <= after_secs {
        start_secs = start_secs.checked_add(7 * SECONDS_PER_DAY)?;
    }
    Some(::aws_smithy_types::DateTime::from_secs(start_secs))
}
