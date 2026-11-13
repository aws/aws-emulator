// SPDX-License-Identifier: CC0-1.0

use super::*;

/// Fill in every `Cluster` member this service DERIVES from the cluster's own
/// configuration, leaving everything a request set exactly as it stands.
///
/// One function, called by every operation that writes a cluster record
/// (`CreateCluster`, `ModifyCluster`, `RestoreFromClusterSnapshot`), so the
/// create path, the modify path and the read path cannot report different member
/// sets for one cluster. It is idempotent: re-running it on a stored record
/// recomputes the derived members from what that record now says, which is what
/// makes it safe to call after a modify changed the node count, the engine
/// version or the maintenance window.
///
/// A read NEVER calls this: `DescribeClusters` reports the stored record.
pub fn populate_derived_members(
    cluster: &mut Cluster,
    cluster_type: &str,
    now: ::aws_smithy_types::DateTime,
) {
    let identifier = cluster.cluster_identifier.clone().unwrap_or_default();

    // Where the cluster is: a cluster launched into a subnet group is in the VPC
    // that group's subnets belong to, and that group's record carries which VPC
    // that is — resolved against the service that OWNS subnets when the group
    // was created ([`subnet_group_record`]). So the operation that launched the
    // cluster sets `VpcId` off that record and nothing is derived here: an id
    // this service invented would name no VPC the account has. A cluster no
    // subnet group placed in a VPC reports none ("The identifier of the VPC the
    // cluster is in, IF the cluster is in a VPC").
    cluster
        .cluster_subnet_group_name
        .get_or_insert_with(|| DEFAULT_CLUSTER_SUBNET_GROUP.to_string());

    // "This parameter is returned only if the cluster is in a VPC": a cluster
    // in one that named no security group is in that VPC's default group, so
    // the list is never empty — and a cluster in no VPC reports no VPC security
    // group at all.
    if let Some(vpc_id) = cluster.vpc_id.clone() {
        if cluster
            .vpc_security_groups
            .as_ref()
            .is_none_or(|groups| groups.is_empty())
        {
            cluster.vpc_security_groups = Some(vec![VpcSecurityGroupMembership {
                vpc_security_group_id: Some(default_vpc_security_group_id(&vpc_id)),
                status: Some(SECURITY_GROUP_STATUS_ACTIVE.to_string()),
            }]);
        }
    }

    // An encrypted cluster always reports the key its data is encrypted under.
    if cluster.encrypted == Some(true) && cluster.kms_key_id.is_none() {
        cluster.kms_key_id = Some(AWS_OWNED_KMS_KEY.to_string());
    }

    // The connection endpoint is NOT derived here: it is decided once, when the
    // cluster is allocated ([`cluster_endpoint`]), and reported only while the
    // cluster is serving ([`StoredCluster::publish_endpoint`]).

    cluster.cluster_public_key = Some(cluster_public_key(&identifier));
    cluster.cluster_nodes = Some(cluster_nodes(
        &identifier,
        cluster_type,
        // Clamped at the call site as well as inside `cluster_nodes`: the
        // topology is one record per node, so the size of this work is bounded
        // here whatever the record carries.
        cluster
            .number_of_nodes
            .unwrap_or(1)
            .clamp(1, MAX_NODES_PER_CLUSTER),
    ));
    cluster.cluster_revision_number = Some(cluster_revision_number(
        &identifier,
        cluster.cluster_version.as_deref().unwrap_or(""),
    ));
    cluster.aqua_configuration = Some(retired_aqua_configuration());
    cluster.cluster_availability_status = Some(
        availability_status(cluster.cluster_status.as_deref().unwrap_or_default()).to_string(),
    );

    // "The secondary compute unit of a cluster, if Multi-AZ deployment is turned
    // on" (`smithy_query(query="shape", shape="Cluster",
    // member="MultiAZSecondary")`): a cluster deployed in two Availability Zones
    // HAS a second compute unit, so one whose `MultiAZ` says so reports where
    // that unit is and which nodes it runs, and one deployed in a single zone
    // reports none. Derived here, beside the primary unit's own nodes, so the
    // create, the modify, the restore and the failover paths cannot report
    // different member sets for one cluster.
    //
    // WHERE the unit is, once decided, is kept: the zone a cluster's second
    // compute unit sits in is a fact of its deployment, so a record that already
    // carries one (a cluster whose primary has been failed over into the other
    // zone, which swaps the two) reports that zone, and only a record that
    // carries none — or one whose stored zone is now the PRIMARY's — chooses a
    // zone for the unit.
    cluster.multi_az_secondary = (cluster.multi_az.as_deref() == Some(MULTI_AZ_ENABLED)).then(
        || {
            let primary = cluster.availability_zone.clone().unwrap_or_default();
            let placed = cluster
                .multi_az_secondary
                .as_ref()
                .and_then(|secondary| secondary.availability_zone.clone())
                .filter(|zone| *zone != primary)
                .unwrap_or_else(|| secondary_availability_zone(&identifier, &primary));
            SecondaryClusterInfo {
                availability_zone: Some(placed),
                cluster_nodes: Some(secondary_cluster_nodes(
                    &identifier,
                    cluster_type,
                    cluster
                        .number_of_nodes
                        .unwrap_or(1)
                        .clamp(1, MAX_NODES_PER_CLUSTER),
                )),
            }
        },
    );

    // "Describes a group of DeferredMaintenanceWindow objects": a cluster whose
    // maintenance nobody deferred reports an EMPTY group rather than none.
    cluster.deferred_maintenance_windows.get_or_insert_default();
    // "A change to one or more the attributes was requested and is in progress
    // or will be applied": a cluster with nothing pending reports the structure
    // with no member set.
    cluster.pending_modified_values.get_or_insert_default();
    if let Some(window) = cluster.preferred_maintenance_window.as_deref() {
        cluster.next_maintenance_window_start_time = next_maintenance_window_start(window, now);
    }
}

/// `Cluster$MultiAZ` for a cluster deployed in TWO Availability Zones, and for
/// one deployed in a single zone.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="MultiAZ")` — "A
/// boolean value that, if true, indicates that the cluster is deployed in two
/// Availability Zones" — reported as the `Enabled` / `Disabled` pair that
/// member's own documentation speaks in rather than as `true` / `false`.
pub const MULTI_AZ_ENABLED: &str = "Enabled";
/// See [`MULTI_AZ_ENABLED`].
pub const MULTI_AZ_DISABLED: &str = "Disabled";

/// What `Cluster$MultiAZ` reports for a request whose `MultiAZ` asked for
/// `enabled`.
///
/// ONE rule for every path that sets the member (`CreateCluster`,
/// `ModifyCluster`, `RestoreFromClusterSnapshot`), so no two of them can report
/// the same deployment differently — and so the derived
/// `Cluster$MultiAZSecondary` beside it is decided by one spelling (see
/// [`populate_derived_members`]).
pub fn multi_az_value(enabled: bool) -> String {
    if enabled {
        MULTI_AZ_ENABLED
    } else {
        MULTI_AZ_DISABLED
    }
    .to_string()
}

/// Why a `FailoverPrimaryCompute` cannot be served as the cluster stands.
///
/// Recorded Amazon Redshift wording for this operation's `InvalidClusterStateFault`
/// (awsQuery code `InvalidClusterState`): fixed text with no variable part — the
/// refusal names neither the cluster nor which precondition it broke. It covers
/// every state a failover cannot be done from, which is what the operation has to
/// say: a cluster deployed in a single Availability Zone has no second compute
/// unit for its primary to move to (`FailoverPrimaryCompute` "fails over the
/// primary compute unit of the specified MULTI-AZ cluster"), and one the service
/// is already working on cannot take the failover as well.
pub const FAILOVER_NOT_POSSIBLE_NOW: &str =
    "You can't failover primary compute of your cluster now. Try again later.";

/// Why a `PauseCluster` cannot be served as the cluster stands.
///
/// Recorded Amazon Redshift wording for this operation's
/// `InvalidClusterStateFault` (awsQuery code `InvalidClusterState`): fixed text
/// with no variable part — the refusal names neither the cluster nor the state
/// it is in, only the state a pause is legal from. Like
/// [`FAILOVER_NOT_POSSIBLE_NOW`] it covers every state the work cannot start
/// from: a cluster still being provisioned, one the service is already working
/// on, and one whose compute is already suspended.
pub const PAUSE_REQUIRES_ACTIVE_CLUSTER: &str = "You can only pause an ACTIVE Cluster";

/// Why an operation that needs a RUNNING cluster refuses one that is not
/// available, naming the cluster and the state it is in.
///
/// `smithy_query(query="shape", shape="InvalidClusterStateFault")` — "The
/// specified cluster is not in the `available` state" — which is the fixed part
/// of the sentence; the cluster the request named and the state it is actually in
/// are the variable parts a caller needs to decide whether to wait. One function
/// for every operation whose precondition is just that, so two of them cannot
/// word it differently; an operation whose recorded refusal says more than this
/// (the restart, which names the action) keeps its own sentence.
pub fn invalid_cluster_state_message(cluster_identifier: &str, status: &str) -> String {
    format!("Cluster {cluster_identifier} is in state {status}; it must be {STATUS_AVAILABLE}.")
}

/// The IAM role associations a cluster reports for `arns`, each at `apply_status`.
///
/// One rule for every path that puts roles on a cluster (`CreateCluster`,
/// `RestoreFromClusterSnapshot`, `ModifyClusterIamRoles`), so the create path and
/// the modify path cannot report an association differently. The ARNs are stored
/// exactly as the request carried them: an IAM role is a resource of another
/// service, so this service does not resolve one.
pub fn cluster_iam_roles<I>(arns: I, apply_status: &str) -> Vec<ClusterIamRole>
where
    I: IntoIterator<Item = String>,
{
    arns.into_iter()
        .map(|arn| ClusterIamRole {
            iam_role_arn: Some(arn),
            apply_status: Some(apply_status.to_string()),
        })
        .collect()
}

/// Associate `add` and disassociate `remove` on a cluster's IAM role list.
///
/// The list is a SET keyed by role ARN, in association order: associating an ARN
/// the cluster already carries leaves one entry, and disassociating an ARN it
/// does not carry changes nothing. Disassociations are applied first, so an ARN
/// named by both lists ends up associated.
///
/// Neither change is complete when this returns: a new association starts at
/// [`IAM_ROLE_APPLY_STATUS_ADDING`] and a disassociation at
/// [`IAM_ROLE_APPLY_STATUS_REMOVING`], and
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] completes
/// both once the cluster's span has passed.
pub fn apply_iam_role_changes(roles: &mut Vec<ClusterIamRole>, add: &[String], remove: &[String]) {
    let position = |roles: &[ClusterIamRole], arn: &str| {
        roles
            .iter()
            .position(|role| role.iam_role_arn.as_deref() == Some(arn))
    };
    for arn in remove {
        if let Some(index) = position(roles, arn) {
            roles[index].apply_status = Some(IAM_ROLE_APPLY_STATUS_REMOVING.to_string());
        }
    }
    for arn in add {
        match position(roles, arn) {
            // Already associated: re-associating it is not a second entry, and
            // an association already in use stays in use. One this same call
            // asked to disassociate goes back to being established.
            Some(index) => {
                if roles[index].apply_status.as_deref() != Some(IAM_ROLE_APPLY_STATUS_IN_SYNC) {
                    roles[index].apply_status = Some(IAM_ROLE_APPLY_STATUS_ADDING.to_string());
                }
            }
            None => roles.extend(cluster_iam_roles(
                [arn.clone()],
                IAM_ROLE_APPLY_STATUS_ADDING,
            )),
        }
    }
}

/// The parameter-group membership a cluster reports for the group it joined.
///
/// `smithy_query(query="doc", shape="ClusterParameterGroupStatus")` — "Describes
/// the status of a parameter group": the group's name and how far its values have
/// been applied. A cluster joins ONE group, the one the request named or the
/// service default ([`cluster_default_parameter_group`]).
pub fn cluster_parameter_group_status(requested: Option<String>) -> Vec<ClusterParameterGroupStatus> {
    vec![ClusterParameterGroupStatus {
        parameter_group_name: Some(requested.unwrap_or_else(cluster_default_parameter_group)),
        parameter_apply_status: Some(PARAMETER_APPLY_STATUS_IN_SYNC.to_string()),
        cluster_parameter_status_list: None,
    }]
}

/// The pending admin-password change a cluster reports while it is being applied.
///
/// Source: `smithy_query(query="doc", shape="PendingModifiedValues",
/// member="MasterUserPassword")` — "The pending or in-progress change of the
/// admin user password for the cluster". The member is `@sensitive`, so what is
/// reported is the mask [`REDACTED_MASTER_USER_PASSWORD`], never the password.
pub fn pending_master_user_password(password: Option<&str>) -> PendingModifiedValues {
    PendingModifiedValues {
        master_user_password: password.map(|_| REDACTED_MASTER_USER_PASSWORD.to_string()),
        ..Default::default()
    }
}

/// The account's DEFAULT cluster subnet group, over the subnets of the Region's
/// default VPC as the service that owns them reports them (see
/// [`default_vpc_subnets`]).
///
/// Amazon Redshift creates every provisioned cluster in a VPC, and an account
/// that names no subnet group gets the default one (see
/// [`DEFAULT_CLUSTER_SUBNET_GROUP`]), so this record is what a cluster that named
/// no group is launched into — and what a `DescribeClusterSubnetGroups` naming
/// `default` reports. Built by the same builder
/// `CreateClusterSubnetGroup` builds a caller's group with, so the two records
/// cannot disagree about a group's shape or about where its subnets are.
pub fn default_subnet_group(subnets: &[ResolvedSubnet]) -> ClusterSubnetGroup {
    subnet_group_record(
        DEFAULT_CLUSTER_SUBNET_GROUP,
        "Default cluster subnet group",
        subnets,
        Vec::new(),
    )
}

/// The record a cluster subnet group is STORED and reported as: its name, its
/// description, the VPC its subnets belong to, one [`Subnet`] per subnet it was
/// made of and the tags it carries.
///
/// ONE builder for every path that writes such a record — the create, the
/// account's ambient default group — so the create path and the read path cannot
/// report different members for one group, and the network it reports is in
/// every case the one EC2 answered with: `VpcId` is the VPC that CONTAINS those
/// subnets ("The VPC ID of the cluster subnet group"), read off their own
/// records, and never derived from the group's name.
///
/// `SubnetGroupStatus` is [`SUBNET_GROUP_STATUS_COMPLETE`]: every subnet of the
/// list resolved, which is what makes a group complete rather than `Incomplete`
/// or `Invalid`.
pub fn subnet_group_record(
    name: &str,
    description: &str,
    subnets: &[ResolvedSubnet],
    tags: Vec<crate::ops::types::Tag>,
) -> ClusterSubnetGroup {
    ClusterSubnetGroup {
        cluster_subnet_group_name: Some(name.to_string()),
        description: Some(description.to_string()),
        // Every subnet of one group belongs to one VPC (`InvalidSubnet` is what
        // a list spanning two earns), so the group's VPC is the VPC of its
        // subnets — taken from the first resolved record, not from the name.
        vpc_id: subnets.first().map(|subnet| subnet.vpc_id.clone()),
        subnet_group_status: Some(SUBNET_GROUP_STATUS_COMPLETE.to_string()),
        subnets: Some(subnets.iter().map(subnet_record).collect()),
        tags: Some(tags),
        supported_cluster_ip_address_types: Some(
            SUPPORTED_CLUSTER_IP_ADDRESS_TYPES
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
        ),
    }
}

/// The subnet record a cluster subnet group reports for one of its subnets.
///
/// `Subnet` is exactly `{SubnetIdentifier, SubnetAvailabilityZone, SubnetStatus}`
/// (`smithy_query(query="shape", shape="Subnet")`), and the zone is the one EC2
/// reports that subnet lies in — so a client cross-checking the group against
/// `ec2:DescribeSubnets` sees one answer. The zone carries its NAME alone on this
/// path: the platforms a zone supports are not a fact about a subnet, and a
/// subnet group does not report them.
pub fn subnet_record(subnet: &ResolvedSubnet) -> Subnet {
    Subnet {
        subnet_identifier: Some(subnet.subnet_id.clone()),
        subnet_availability_zone: Some(AvailabilityZone {
            name: Some(subnet.availability_zone.clone()),
            supported_platforms: None,
        }),
        subnet_status: Some(SUBNET_STATUS_ACTIVE.to_string()),
    }
}

/// Why a `SubnetIds` list whose subnets do not all belong to one VPC is refused.
///
/// `smithy_query(query="shape", shape="InvalidSubnet")` — "The requested subnet
/// is not valid, or not all of the subnets are in the same VPC." The fault's own
/// documented sentence is the fixed text of the refusal: a cluster subnet group
/// has ONE `VpcId`, so there is nothing about the request left to say.
pub const SUBNETS_NOT_IN_ONE_VPC: &str =
    "The requested subnet is not valid, or not all of the subnets are in the same VPC.";

/// Why a `SubnetIds` list naming no subnet at all is refused, under the same
/// fault: "You must provide a list of one or more subnets in your existing Amazon
/// Virtual Private Cloud (Amazon VPC) when creating Amazon Redshift subnet group"
/// (`smithy_query(query="doc", operation="CreateClusterSubnetGroup")`) — a group
/// with no subnet covers no Availability Zone, so there is nothing to launch a
/// cluster into. One sentence for the create and the modify, which take the same
/// list.
pub const SUBNET_LIST_IS_EMPTY: &str = "SubnetIds must name at least one subnet of your VPC.";

/// Why a cluster subnet group something was placed in cannot be deleted.
///
/// `smithy_query(query="shape", shape="InvalidClusterSubnetGroupStateFault")` —
/// "The cluster subnet group cannot be deleted because it is in use." The
/// fault's own documented sentence is the fixed text of the refusal.
pub const SUBNET_GROUP_IN_USE: &str =
    "The cluster subnet group cannot be deleted because it is in use.";

/// Why a `SubnetIds` list longer than one group may hold is refused, naming the
/// allowance it exceeds.
///
/// `smithy_query(query="shape", shape="ClusterSubnetQuotaExceededFault")` — "The
/// request would result in user exceeding the allowed number of subnets in a
/// cluster subnet groups." One sentence for the create and the modify, which hold
/// the same list to the same allowance ([`SUBNETS_PER_SUBNET_GROUP_QUOTA`]).
pub fn subnet_quota_exceeded_message() -> String {
    format!(
        "The request would exceed the allowed number of subnets in a cluster subnet group ({SUBNETS_PER_SUBNET_GROUP_QUOTA})."
    )
}

/// Which sentence a `ClusterSubnetGroupNotFoundFault` is worded with.
///
/// The same split as [`ClusterParameterGroupNotFoundWording`]: the fault's status
/// (400) and awsQuery code (`ClusterSubnetGroupNotFoundFault`) are what the model
/// fixes, while the fixed text around the name the request named is an attribute
/// of the OPERATION. Recorded refusals show both sentences below in use, so
/// naming each one here keeps the choice at the handler that makes it and no
/// operation can quietly inherit another one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubnetGroupNotFoundWording {
    /// `Cluster Subnet group '<name>' not found.` — what the group LISTING,
    /// `DescribeClusterSubnetGroups`, answers when its `ClusterSubnetGroupName`
    /// filter names no group (capital `S` on `Subnet`).
    NotFound,
    /// `Cluster subnet group '<name>' does not exist.` — what every operation
    /// that DEREFERENCES a group answers: the delete whose target is the group
    /// itself, and the operations configured from one (`CreateCluster`,
    /// `RestoreFromClusterSnapshot`, `CreateEndpointAccess`).
    DoesNotExist,
}

/// How an operation that resolves a `ClusterSubnetGroupName` to nothing says so,
/// quoting the name as the REQUEST spelled it — the caller has to recognise what
/// it asked for.
///
/// Recorded Amazon Redshift wording for `ClusterSubnetGroupNotFoundFault` ("The
/// cluster subnet group name does not refer to an existing cluster subnet
/// group"), per [`SubnetGroupNotFoundWording`]. One function for every path that
/// raises the fault, so two operations that word it the same way cannot drift
/// apart.
pub fn subnet_group_not_found_message(
    wording: SubnetGroupNotFoundWording,
    requested: &str,
) -> String {
    match wording {
        SubnetGroupNotFoundWording::NotFound => {
            format!("Cluster Subnet group '{requested}' not found.")
        }
        SubnetGroupNotFoundWording::DoesNotExist => {
            format!("Cluster subnet group '{requested}' does not exist.")
        }
    }
}

/// How a create refuses a `ClusterSubnetGroupName` the account already has a
/// group under, quoting the name as the REQUEST spelled it — the caller has to
/// recognise what it asked for, exactly as
/// [`subnet_group_not_found_message`] quotes it.
///
/// Recorded Amazon Redshift wording for `ClusterSubnetGroupAlreadyExistsFault`
/// (`smithy_query(query="shape", shape="ClusterSubnetGroupAlreadyExistsFault")`
/// — "A ClusterSubnetGroupName is already used by an existing cluster subnet
/// group"): the fixed text is `The Cluster subnet group '` + the name + `'
/// already exists.`
pub fn subnet_group_already_exists_message(requested: &str) -> String {
    format!("The Cluster subnet group '{requested}' already exists.")
}

/// Why the snapshot `snapshot`, named by `identifier`, cannot be deleted as it
/// stands — or `None` when every rule a delete enforces admits it.
///
/// ONE rule for both delete paths, the one-snapshot `DeleteClusterSnapshot` and
/// the per-entry `BatchDeleteClusterSnapshots`, so the two cannot drift on what
/// is deletable or on what they say about a record that is not. The three rules,
/// each from the one-snapshot operation's own documentation:
///
/// * "Deletes the specified MANUAL snapshot" — Amazon Redshift deletes a
///   snapshot it took on its own schedule itself, when that snapshot's retention
///   period ends, so a caller cannot delete one;
/// * "The snapshot must be in the available state" — which is also what
///   `InvalidClusterSnapshotStateFault` reports;
/// * "with no other users authorized to access the snapshot […] If other
///   accounts are authorized to access the snapshot, you must revoke all of the
///   authorizations before you can delete the snapshot."
///
/// The identifier is passed rather than read off the record, so a caller that
/// has already resolved which spelling to report (the stored one, or the one its
/// entry named) keeps it.
pub fn snapshot_delete_refusal(snapshot: &Snapshot, identifier: &str) -> Option<String> {
    let snapshot_type = snapshot.snapshot_type.as_deref().unwrap_or_default();
    if snapshot_type != SNAPSHOT_TYPE_MANUAL {
        return Some(format!(
            "Cluster snapshot {identifier} is of type {snapshot_type}, and only a \
             {SNAPSHOT_TYPE_MANUAL} snapshot can be deleted."
        ));
    }
    let status = snapshot.status.as_deref().unwrap_or_default();
    if status != STATUS_AVAILABLE {
        return Some(format!(
            "Cluster snapshot {identifier} is in the {status} state, not {STATUS_AVAILABLE}."
        ));
    }
    let shared_with = snapshot
        .accounts_with_restore_access
        .iter()
        .flatten()
        .count();
    if shared_with > 0 {
        return Some(format!(
            "Cluster snapshot {identifier} is authorized for restore by {shared_with} other \
             account(s); revoke every authorization before deleting it."
        ));
    }
    None
}

/// Build the snapshot record a manual snapshot of `cluster` reports.
///
/// Every identifying attribute is COPIED off the stored cluster — node type,
/// node count, admin user, database, engine version, port, Availability Zone and
/// the cluster's own creation time — so a snapshot read back describes the
/// cluster as it was when the snapshot was asked for.
///
/// `Status` starts at `creating`: `smithy_query(query="doc", shape="Snapshot",
/// member="Status")` states "CreateClusterSnapshot and CopyClusterSnapshot
/// returns status as \"creating\"". [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]
/// moves it to `available` once [`PROVISIONING_SPAN`] has passed.
///
/// The manual retention period is applied through [`apply_manual_retention`],
/// the one rule for it, so a snapshot a create minted reports the same retention
/// bookkeeping a `ModifyClusterSnapshot` would leave for the same period.
pub fn snapshot_of(
    stored: &StoredCluster,
    snapshot_identifier: &str,
    created_at: ::aws_smithy_types::DateTime,
    manual_retention_period: i32,
    tags: Vec<crate::ops::types::Tag>,
    region: &str,
) -> Snapshot {
    let cluster = &stored.cluster;
    let cluster_identifier = cluster.cluster_identifier.clone().unwrap_or_default();
    let mut snapshot = Snapshot {
        snapshot_identifier: Some(snapshot_identifier.to_string()),
        cluster_identifier: Some(cluster_identifier.clone()),
        snapshot_create_time: Some(created_at),
        status: Some(STATUS_CREATING.to_string()),
        // The port the cluster serves on, taken from the endpoint the cluster
        // was allocated rather than from the one it PUBLISHES: a snapshot of a
        // cluster that is still coming up records the same port a snapshot of it
        // taken a moment later does.
        port: stored.endpoint.port,
        availability_zone: cluster.availability_zone.clone(),
        cluster_create_time: cluster.cluster_create_time,
        master_username: cluster.master_username.clone(),
        cluster_version: cluster.cluster_version.clone(),
        snapshot_type: Some(SNAPSHOT_TYPE_MANUAL.to_string()),
        node_type: cluster.node_type.clone(),
        number_of_nodes: cluster.number_of_nodes,
        db_name: cluster.db_name.clone(),
        vpc_id: cluster.vpc_id.clone(),
        encrypted: cluster.encrypted,
        kms_key_id: cluster.kms_key_id.clone(),
        encrypted_with_hsm: Some(cluster.hsm_status.is_some()),
        owner_account: Some(engine::emulator_account_id().to_string()),
        // `Snapshot$SourceRegion` names the region a snapshot was COPIED from, so
        // a snapshot taken in place leaves it unset.
        tags: Some(tags),
        enhanced_vpc_routing: cluster.enhanced_vpc_routing,
        maintenance_track_name: cluster.maintenance_track_name.clone(),
        master_password_secret_arn: cluster.master_password_secret_arn.clone(),
        master_password_secret_kms_key_id: cluster.master_password_secret_kms_key_id.clone(),
        snapshot_arn: Some(snapshot_arn(
            &cluster_identifier,
            snapshot_identifier,
            region,
        )),
        ..Default::default()
    };
    apply_manual_retention(&mut snapshot, manual_retention_period, created_at);
    snapshot
}

/// The database revision a cluster RUNS, as `DescribeClusterDbRevisions`
/// reports it.
///
/// Every value is read off the cluster's own stored record, so what this reports
/// and what a `DescribeClusters` reports for the same cluster cannot disagree:
///
/// * `ClusterIdentifier` is "the unique identifier of the cluster" the record
///   carries;
/// * `CurrentDatabaseRevision` is "a string representing the current cluster
///   version" (`smithy_query(query="shape", shape="ClusterDbRevision")`), which
///   is the revision the cluster's `ClusterRevisionNumber` already reports (see
///   [`cluster_revision_number`]) rather than a second number for the same
///   thing;
/// * `DatabaseRevisionReleaseDate` is "the date on which the database revision
///   was released". This service releases a cluster's database revision to it
///   when it provisions the cluster — the revision is minted per cluster and
///   engine version — so the instant is the cluster's own `ClusterCreateTime`,
///   taken off the record rather than chosen here;
/// * `RevisionTargets` is "a list of `RevisionTarget` objects, where each object
///   describes the database revision that a cluster can be updated to": this
///   service publishes no revision of a cluster's database later than the one
///   the cluster runs, so a provisioned cluster is on its latest revision and
///   the list is EMPTY rather than offering an update nothing could apply.
pub fn cluster_db_revision(cluster: &Cluster) -> ClusterDbRevision {
    ClusterDbRevision {
        cluster_identifier: cluster.cluster_identifier.clone(),
        current_database_revision: cluster.cluster_revision_number.clone(),
        database_revision_release_date: cluster.cluster_create_time,
        revision_targets: Some(Vec::new()),
    }
}

/// Why a request to move a cluster onto another database revision is refused
/// when the cluster runs the latest one this service publishes, naming the
/// cluster the request asked about.
///
/// The fixed text is what the model states the fault is
/// (`smithy_query(query="shape", shape="ClusterOnLatestRevisionFault")` —
/// "Cluster is already on the latest database revision"), with the cluster it is
/// about named in it; which revisions a cluster can be moved onto is
/// [`cluster_db_revision`]'s `RevisionTargets`.
pub fn cluster_on_latest_revision_message(cluster_identifier: &str) -> String {
    format!("Cluster {cluster_identifier} is already on the latest database revision.")
}

/// Why a `RevisionTarget` that names no revision the cluster can be moved onto
/// is refused, naming the target the request sent and the cluster it asked
/// about.
///
/// A target outside [`cluster_db_revision`]'s `RevisionTargets` is a parameter
/// value this service cannot honour, which is the `InvalidParameterValue` code
/// Amazon Redshift answers such a request with.
pub fn unknown_revision_target_message(cluster_identifier: &str, revision_target: &str) -> String {
    format!(
        "Invalid value for RevisionTarget: '{revision_target}'. Cluster {cluster_identifier} cannot be updated to that database revision."
    )
}

/// Resolve the `ClusterType` / `NumberOfNodes` pair a request asks for, or say
/// why the pair cannot hold.
///
/// The two members describe one another, so neither is applied alone. Source:
/// `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterType")`
/// — "Valid Values: multi-node | single-node" and "When cluster type is specified
/// as single-node, the NumberOfNodes parameter is not required. multi-node, the
/// NumberOfNodes parameter is required" — with `NumberOfNodes`'s own "Default: 1".
///
/// `current` is the stored pair when a cluster is being MODIFIED, and `None` when
/// it is being created; a create that names neither member takes the node-count
/// default and the cluster type that node count describes. Shared by
/// `CreateCluster` and `ModifyCluster` so the rule cannot drift between them.
///
/// The resolved count is BOUNDED on both ends: at least one node (two for a
/// multi-node cluster) and at most [`MAX_NODES_PER_CLUSTER`].
pub fn resolve_cluster_shape(
    requested_type: Option<&str>,
    requested_nodes: Option<i32>,
    current: Option<(&str, i32)>,
) -> Result<(String, i32), String> {
    let kind = match requested_type {
        Some(CLUSTER_TYPE_SINGLE_NODE) => CLUSTER_TYPE_SINGLE_NODE,
        Some(CLUSTER_TYPE_MULTI_NODE) => CLUSTER_TYPE_MULTI_NODE,
        Some(other) => {
            return Err(format!(
                "Invalid value for ClusterType: '{other}'. Valid Values: {}",
                ORDERABLE_CLUSTER_TYPES.join(" | ")
            ))
        }
        // No `ClusterType`: a modify keeps the stored shape, and a create lets
        // the node count (default 1) say which shape is being asked for.
        None => match current {
            Some((stored, _)) => stored,
            None if requested_nodes.unwrap_or(1) > 1 => CLUSTER_TYPE_MULTI_NODE,
            None => CLUSTER_TYPE_SINGLE_NODE,
        },
    };
    // A node count the request does not name: "When cluster type is specified as
    // single-node, the NumberOfNodes parameter is not required" — a single-node
    // cluster has exactly one node, whatever it had before. A cluster that STAYS
    // multi-node keeps the count it had; one that BECOMES multi-node must say how
    // many, since "multi-node, the NumberOfNodes parameter is required".
    let nodes = match (requested_nodes, kind, current) {
        (Some(nodes), _, _) => nodes,
        (None, CLUSTER_TYPE_SINGLE_NODE, _) => 1,
        (None, _, Some((stored_kind, stored_nodes))) if stored_kind == kind => stored_nodes,
        // "Default: 1": a request that names no count asks for the one-node
        // shape, which the cluster type then has to admit — so a multi-node
        // cluster with no count named is refused by the lower bound below, under
        // the same message a count of 1 is, rather than by a rule of its own.
        (None, _, _) => 1,
    };
    if kind == CLUSTER_TYPE_SINGLE_NODE && nodes != 1 {
        return Err(SINGLE_NODE_COUNT_REFUSAL.to_string());
    }
    if kind == CLUSTER_TYPE_MULTI_NODE && nodes < MIN_MULTI_NODE_NODES {
        return Err(MULTI_NODE_COUNT_REFUSAL.to_string());
    }
    // The UPPER bound belongs here too, beside the lower ones: this is the one
    // resolver every cluster write goes through, so no caller can carry a node
    // count past [`MAX_NODES_PER_CLUSTER`] into the per-node work a cluster's
    // topology costs. An operation that models
    // `NumberOfNodesPerClusterLimitExceededFault` refuses with that fault BEFORE
    // it gets here (see [`exceeds_nodes_per_cluster_limit`]); this is the floor
    // under every other caller.
    if exceeds_nodes_per_cluster_limit(nodes) {
        return Err(nodes_per_cluster_limit_message(nodes));
    }
    Ok((kind.to_string(), nodes))
}
