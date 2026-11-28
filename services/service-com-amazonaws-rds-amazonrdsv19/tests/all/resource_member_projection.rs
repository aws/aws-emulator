// SPDX-License-Identifier: CC0-1.0

//! The members a resource-describing response of this front door projects from the
//! resource it returns.
//!
//! The rule under test: an operation that answers with a resource projects EVERY modelled
//! member that resource's own control plane reports, computed from this service's state and
//! from the facts that state resolves (the region's VPC, its Availability Zones, the hosted
//! zone its endpoints sit in, the engine's own defaults) — rather than leaving the member
//! out — and the create, the read and the modify of one resource project the SAME record,
//! so none of them reports less than another.
//!
//! Each test drives the rule over a TABLE spanning what it generalises over rather than one
//! request: the three control planes this one `rds` front door serves (`src/service.rs`),
//! whether the request names a value for the member or leaves the control plane to supply
//! it, whether the resource has dependants (a cluster with and without a DB instance in
//! it), and every operation that answers with the same projection.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, wait_for_settle,
    REGION,
};

/// The instance class a cluster member is created with: one the orderable-options report
/// offers for a cluster of every engine this front door serves.
const CLUSTER_MEMBER_CLASS: &str = "db.r5.large";

/// `CreateDBCluster` with `DBClusterIdentifier` and `Engine` plus `extra`.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut parameters: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", identifier),
        ("Engine", engine),
    ];
    parameters.extend_from_slice(extra);
    call(router, "CreateDBCluster", &parameters).await
}

// ---------------------------------------------------------------------------
// The DBCluster projection
// ---------------------------------------------------------------------------

/// One row of the cluster table.
struct ClusterCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The parameters the create carries beyond the two required members.
    extra: &'static [(&'static str, &'static str)],
    /// The `MasterUsername` the cluster must report: the one the request named, or the one
    /// its control plane assigns a cluster that cannot be given one, or none.
    master_username: Option<&'static str>,
    /// The `AutoMinorVersionUpgrade` the cluster must report.
    auto_minor_version_upgrade: &'static str,
    /// The security group ids `VpcSecurityGroups` must report, in order; empty means the
    /// VPC's default group, which the assertion resolves for itself.
    security_groups: &'static [&'static str],
}

/// The clusters the projection is checked over. The rows differ along what the rule
/// generalises over: which control plane owns the engine (and so which defaults the cluster
/// takes and which DNS domain its endpoints sit in), whether the request names a master
/// username, whether it names security groups, and whether it names
/// `AutoMinorVersionUpgrade` itself.
const CLUSTER_CASES: &[ClusterCase] = &[
    ClusterCase {
        case: "a Neptune cluster, nothing beyond the two required members",
        identifier: "emu-proj-neptune",
        engine: "neptune",
        extra: &[],
        // `CreateDBClusterMessage$MasterUsername` is "Not supported by Neptune", so the
        // control plane assigns the cluster's master user.
        master_username: Some("admin"),
        // The graph control plane publishes no `AutoMinorVersionUpgrade` on its own
        // `CreateDBCluster` and reports a cluster it created as not taking minor engine
        // upgrades automatically, so the default of THAT engine is off
        // (`state::cluster_auto_minor_version_upgrade_default`).
        auto_minor_version_upgrade: "false",
        security_groups: &[],
    },
    ClusterCase {
        case: "a Neptune cluster that names its own security groups",
        identifier: "emu-proj-neptune-sg",
        engine: "neptune",
        extra: &[
            ("VpcSecurityGroupIds.VpcSecurityGroupId.1", "sg-0a1b2c3d4e5f60711"),
            ("VpcSecurityGroupIds.VpcSecurityGroupId.2", "sg-0a1b2c3d4e5f60722"),
        ],
        master_username: Some("admin"),
        auto_minor_version_upgrade: "false",
        security_groups: &["sg-0a1b2c3d4e5f60711", "sg-0a1b2c3d4e5f60722"],
    },
    ClusterCase {
        case: "a DocumentDB cluster, whose request names its master user",
        identifier: "emu-proj-docdb",
        engine: "docdb",
        extra: &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "pw1-Secret"),
        ],
        master_username: Some("emuadmin"),
        auto_minor_version_upgrade: "true",
        security_groups: &[],
    },
    ClusterCase {
        case: "an Aurora MySQL cluster, which takes this model's own defaults",
        identifier: "emu-proj-aurora",
        engine: "aurora-mysql",
        extra: &[],
        master_username: None,
        // "By default, minor engine upgrades are applied automatically."
        auto_minor_version_upgrade: "true",
        security_groups: &[],
    },
    ClusterCase {
        case: "an Aurora MySQL cluster whose request turns the upgrade setting off",
        identifier: "emu-proj-aurora-off",
        engine: "aurora-mysql",
        extra: &[("AutoMinorVersionUpgrade", "false")],
        master_username: None,
        auto_minor_version_upgrade: "false",
        security_groups: &[],
    },
];

/// Assert the members of one `DBCluster` envelope this row pins, whichever operation
/// answered with it.
fn assert_cluster_members(case: &str, label: &str, cluster: &str, row: &ClusterCase) {
    let case = format!("{case} ({label})");

    let master_username = optional_text(cluster, "MasterUsername");
    assert_eq!(
        master_username.as_deref(),
        row.master_username,
        "case {case:?}: expected MasterUsername {:?}, got {master_username:?}",
        row.master_username
    );

    let auto_minor = text(cluster, "AutoMinorVersionUpgrade");
    assert_eq!(
        auto_minor, row.auto_minor_version_upgrade,
        "case {case:?}: expected AutoMinorVersionUpgrade {:?}, got {auto_minor:?}",
        row.auto_minor_version_upgrade
    );

    // One hosted zone per region and control-plane domain, reported for every cluster of
    // that engine.
    let expected_zone = state::hosted_zone_id(REGION, state::endpoint_domain(row.engine));
    let hosted_zone = optional_text(cluster, "HostedZoneId");
    assert_eq!(
        hosted_zone.as_deref(),
        Some(expected_zone.as_str()),
        "case {case:?}: expected HostedZoneId {expected_zone:?}, got {hosted_zone:?}"
    );

    // The security groups the cluster belongs to: the ones the request named, or its VPC's
    // default group where it named none — each in the status an association in place has.
    let default_group = state::default_security_group_id(REGION);
    let expected_groups: Vec<String> = if row.security_groups.is_empty() {
        vec![default_group]
    } else {
        row.security_groups
            .iter()
            .map(|id| (*id).to_string())
            .collect()
    };
    let memberships = subtrees(cluster, "VpcSecurityGroupMembership");
    assert_eq!(
        memberships.len(),
        expected_groups.len(),
        "case {case:?}: expected {} security-group membership(s), got {memberships:?}",
        expected_groups.len()
    );
    for (membership, expected) in memberships.iter().zip(&expected_groups) {
        let reported = text(membership, "VpcSecurityGroupId");
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected VpcSecurityGroupId {expected:?}, got {reported:?}"
        );
        let status = text(membership, "Status");
        assert_eq!(
            status, "active",
            "case {case:?}: expected the membership status \"active\", got {status:?}"
        );
    }

    // The members the recorded cluster envelope of this front door carries on every cluster
    // it reports, whatever the engine: the activity-stream status of a cluster no
    // `StartActivityStream` started a stream on.
    let member = "ActivityStreamStatus";
    let expected = "stopped";
    {
        let reported = optional_text(cluster, member);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected {member} {expected:?} for engine {:?}, got {reported:?}",
            row.engine
        );
    }

    // The two express-configuration settings: a cluster this operation created is
    // VPC-based and has no internet access gateway in front of it.
    for (member, expected) in [
        ("VPCNetworkingEnabled", "true"),
        ("InternetAccessGatewayEnabled", "false"),
    ] {
        let reported = optional_text(cluster, member);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected {member} {expected:?}, got {reported:?}"
        );
    }

    // The default parameter group is the one of the engine-version family the cluster was
    // actually created with, so the two cannot name different families.
    let engine_version = text(cluster, "EngineVersion");
    let parameter_group = text(cluster, "DBClusterParameterGroup");
    let family = parameter_group
        .strip_prefix("default.")
        .unwrap_or_default()
        .to_string();
    assert!(
        !family.is_empty() && engine_version.starts_with(&format!("{}.", family_version(&family))),
        "case {case:?}: expected DBClusterParameterGroup to name the family of \
         EngineVersion {engine_version:?}, got {parameter_group:?}"
    );
}

/// The engine-version part of a DB parameter group family (`neptune1.4` -> `1.4`,
/// `aurora-mysql8.0` -> `8.0`): the family is the engine name followed by that version, so
/// the version is what is left once the leading letters and hyphens are dropped.
fn family_version(family: &str) -> String {
    family
        .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '-')
        .to_string()
}

#[tokio::test]
async fn every_cluster_envelope_projects_the_members_its_control_plane_reports() {
    let router = emulator();

    for row in CLUSTER_CASES {
        let created = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                create_cluster(&router, row.identifier, row.engine, row.extra).await,
            ),
        );
        assert_cluster_members(row.case, "CreateDBCluster", &created, row);

        // A cluster still being created has taken no backup yet, so there is no point in
        // time it can be restored to.
        for member in ["EarliestRestorableTime", "LatestRestorableTime"] {
            let reported = optional_text(&created, member);
            assert_eq!(
                reported, None,
                "case {:?}: a cluster that is still being created reports no {member}, got \
                 {reported:?}",
                row.case
            );
        }

        // The same record, read back once the clock has made the cluster available.
        wait_for_settle();
        let read_back = describe_one(&router, row.case, row.identifier).await;
        assert_cluster_members(row.case, "DescribeDBClusters", &read_back, row);

        // ... and now it reports the window its automated backup covers: at most
        // `BackupRetentionPeriod` days back, and never before the cluster existed.
        let created_at = text(&read_back, "ClusterCreateTime");
        let earliest = text(&read_back, "EarliestRestorableTime");
        let latest = text(&read_back, "LatestRestorableTime");
        assert!(
            earliest >= created_at && latest >= earliest,
            "case {:?}: expected a restore window inside the cluster's lifetime, got \
             create {created_at:?}, earliest {earliest:?}, latest {latest:?}",
            row.case
        );

        // And a modification that needs no downtime answers with the same projection, the
        // cluster still available.
        let modified = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("CopyTagsToSnapshot", "true"),
                    ],
                )
                .await,
            ),
        );
        assert_cluster_members(row.case, "ModifyDBCluster", &modified, row);
        let status = text(&modified, "Status");
        assert_eq!(
            status, "available",
            "case {:?}: a modification that needs no downtime leaves the cluster available, \
             got {status:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn two_control_planes_endpoints_sit_in_two_hosted_zones() {
    let router = emulator();

    // One hosted zone per region AND per control-plane domain: two clusters of one engine
    // report the same id, and clusters of engines served under different domains do not.
    let mut zones: Vec<(&str, String)> = Vec::new();
    for (index, engine) in ["neptune", "neptune", "docdb", "aurora-mysql"]
        .iter()
        .enumerate()
    {
        let identifier = format!("emu-proj-zone-{index}");
        let created = the_cluster(
            engine,
            &expect_ok(engine, create_cluster(&router, &identifier, engine, &[]).await),
        );
        zones.push((engine, text(&created, "HostedZoneId")));
    }
    assert_eq!(
        zones[0].1, zones[1].1,
        "two clusters of one engine in one region sit in one hosted zone, got {zones:?}"
    );
    for other in [2usize, 3] {
        assert_ne!(
            zones[0].1, zones[other].1,
            "a cluster served under another control plane's domain sits in another hosted \
             zone, got {zones:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The DBClusterSnapshot projection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_snapshot_projects_the_cluster_it_was_taken_from() {
    let router = emulator();

    // (case, engine, whether the source cluster has a DB instance in it, the LicenseModel
    // the engine publishes). The rows span the control planes and both sides of the rule
    // the snapshot's storage and port follow.
    let rows: [(&str, &str, bool, Option<&str>); 4] = [
        ("a Neptune cluster with no instance", "neptune", false, Some("neptune")),
        ("a Neptune cluster with a member instance", "neptune", true, Some("neptune")),
        (
            "an Aurora MySQL cluster with a member instance",
            "aurora-mysql",
            true,
            // An Aurora engine is the licensed product itself, so the cluster's records
            // report the engine's own name; see `state::cluster_license_model`.
            Some("aurora-mysql"),
        ),
        ("a DocumentDB cluster with no instance", "docdb", false, None),
    ];

    for (index, (case, engine, with_instance, license_model)) in rows.iter().enumerate() {
        let cluster = format!("emu-proj-snap-{index}");
        let snapshot = format!("emu-proj-snap-{index}-1");
        let extra: Vec<(&str, &str)> = if *engine == "docdb" {
            vec![
                ("MasterUsername", "emuadmin"),
                ("MasterUserPassword", "pw1-Secret"),
            ]
        } else {
            Vec::new()
        };
        expect_ok(case, create_cluster(&router, &cluster, engine, &extra).await);
        if *with_instance {
            let member = format!("{cluster}-m0");
            expect_ok(
                case,
                call(
                    &router,
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", member.as_str()),
                        ("Engine", engine),
                        ("DBInstanceClass", CLUSTER_MEMBER_CLASS),
                        ("DBClusterIdentifier", cluster.as_str()),
                    ],
                )
                .await,
            );
        }
        wait_for_settle();
        let source = describe_one(&router, case, &cluster).await;
        let taken = expect_ok(
            case,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", cluster.as_str()),
                    ("DBClusterSnapshotIdentifier", snapshot.as_str()),
                ],
            )
            .await,
        );
        // The snapshot read back once the clock has finished taking it: a finished snapshot
        // is where its own size is known.
        wait_for_settle();
        let read_back = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", snapshot.as_str())],
            )
            .await,
        );

        // The members the snapshot copies off the cluster it was taken from, read from the
        // cluster's own envelope rather than restated here.
        let mut expected: Vec<(&str, Option<String>)> = vec![
            ("MasterUsername", optional_text(&source, "MasterUsername")),
            ("VpcId", Some(state::region_vpc_id(REGION))),
            (
                "BackupRetentionPeriod",
                Some(text(&source, "BackupRetentionPeriod")),
            ),
            (
                "PreferredBackupWindow",
                Some(text(&source, "PreferredBackupWindow")),
            ),
            (
                "IAMDatabaseAuthenticationEnabled",
                Some(text(&source, "IAMDatabaseAuthenticationEnabled")),
            ),
            ("LicenseModel", license_model.map(str::to_string)),
        ];
        // The storage the snapshot holds and the port it was taken on are facts about the
        // cluster's DB instances: a cluster with none reports zero for both. The storage is
        // only known once the snapshot has been taken, so the CREATE still reports the
        // cluster's own and only the finished snapshot reports what it holds.
        expected.push((
            "Port",
            Some(if *with_instance {
                text(&source, "Port")
            } else {
                "0".to_string()
            }),
        ));
        let source_storage = text(&source, "AllocatedStorage");
        let held_storage = if *with_instance {
            source_storage.clone()
        } else {
            "0".to_string()
        };

        for (envelope, xml, storage) in [
            ("CreateDBClusterSnapshot", &taken, &source_storage),
            ("DescribeDBClusterSnapshots", &read_back, &held_storage),
        ] {
            for (member, value) in &expected {
                let reported = optional_text(xml, member);
                assert_eq!(
                    reported.as_deref(),
                    value.as_deref(),
                    "case {case:?} ({envelope}): expected {member} {value:?}, got \
                     {reported:?} in {xml}"
                );
            }
            let reported = optional_text(xml, "AllocatedStorage");
            assert_eq!(
                reported.as_deref(),
                Some(storage.as_str()),
                "case {case:?} ({envelope}): expected AllocatedStorage {storage:?}, got \
                 {reported:?} in {xml}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The DBSubnetGroup projection
// ---------------------------------------------------------------------------

/// `CreateDBSubnetGroup` naming `subnets`.
async fn create_subnet_group(
    router: &MultiServiceRouter,
    name: &str,
    subnets: &[&str],
) -> (u16, String) {
    let mut parameters: Vec<(String, String)> = vec![
        ("DBSubnetGroupName".to_string(), name.to_string()),
        (
            "DBSubnetGroupDescription".to_string(),
            "resource projection".to_string(),
        ),
    ];
    for (index, subnet) in subnets.iter().enumerate() {
        parameters.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            (*subnet).to_string(),
        ));
    }
    call(router, "CreateDBSubnetGroup", &parameters).await
}

/// Assert one `DBSubnetGroup` envelope reports the VPC its subnets belong to, the network
/// protocols they carry, and each subnet with the Availability Zone it lies in.
fn assert_subnet_group(case: &str, label: &str, group: &str, subnets: &[&str]) {
    let case = format!("{case} ({label})");

    let vpc_id = optional_text(group, "VpcId");
    assert_eq!(
        vpc_id.as_deref(),
        Some(state::region_vpc_id(REGION).as_str()),
        "case {case:?}: expected the group's VpcId, got {vpc_id:?} in {group}"
    );

    // "Valid values: IPV4 | DUAL" — the subnets this service resolves carry no IPv6 CIDR.
    let network_types = texts(group, "member");
    assert_eq!(
        network_types,
        state::SUBNET_GROUP_NETWORK_TYPES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected SupportedNetworkTypes, got {network_types:?} in {group}"
    );

    let reported = subtrees(group, "Subnet");
    assert_eq!(
        reported.len(),
        subnets.len(),
        "case {case:?}: expected {} subnet(s), got {reported:?}",
        subnets.len()
    );
    for (subnet, expected) in reported.iter().zip(subnets) {
        let identifier = text(subnet, "SubnetIdentifier");
        assert_eq!(
            identifier, *expected,
            "case {case:?}: expected SubnetIdentifier {expected:?}, got {identifier:?}"
        );
        // The zone a subnet lies in is the fact of the service that OWNS subnets, and this
        // harness mounts only Amazon RDS: the group reports NO zone for a subnet nobody
        // answered for, rather than one of its own choosing. What it reports when the owner
        // of subnets IS mounted beside it is the `db_subnet_group_ec2_zones` suite's rows.
        let zone = subtrees(subnet, "SubnetAvailabilityZone");
        assert!(
            zone.is_empty(),
            "case {case:?}: no zone was ever resolved for {expected:?}, so none may be \
             reported, got {zone:?}"
        );
        // A subnet that is not on an Outpost reports the element EMPTY, which says exactly
        // that, rather than omitting it.
        let outpost = subtrees(subnet, "SubnetOutpost");
        assert_eq!(
            outpost.len(),
            1,
            "case {case:?}: expected a SubnetOutpost element, got {subnet}"
        );
        assert_eq!(
            optional_text(subnet, "SubnetOutpost").unwrap_or_default(),
            String::new(),
            "case {case:?}: expected an empty SubnetOutpost, got {subnet}"
        );
        let status = text(subnet, "SubnetStatus");
        assert_eq!(
            status, "Active",
            "case {case:?}: expected SubnetStatus \"Active\", got {status:?}"
        );
    }
}

#[tokio::test]
async fn every_subnet_group_envelope_projects_its_vpc_and_zones() {
    let router = emulator();

    // The rows differ along the number of subnets the group names and the ids themselves,
    // which is what the Availability Zone of each is resolved from.
    let rows: [(&str, &str, &[&str]); 3] = [
        (
            "two subnets",
            "emu-proj-subnets-2",
            &["subnet-0a1b2c3d4e5f60711", "subnet-0a1b2c3d4e5f60722"],
        ),
        (
            "three subnets",
            "emu-proj-subnets-3",
            &[
                "subnet-0123456789abcdef0",
                "subnet-0fedcba987654321f",
                "subnet-00112233445566778",
            ],
        ),
        (
            "two subnets of another shape",
            "emu-proj-subnets-short",
            &["subnet-1a2b3c4d", "subnet-5e6f7a8b"],
        ),
    ];

    for (case, name, subnets) in rows {
        let created = expect_ok(case, create_subnet_group(&router, name, subnets).await);
        assert_subnet_group(case, "CreateDBSubnetGroup", &created, subnets);

        let read_back = expect_ok(
            case,
            call(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", name)],
            )
            .await,
        );
        assert_subnet_group(case, "DescribeDBSubnetGroups", &read_back, subnets);

        // The modify answers with the same projection, over the subnets it leaves the group
        // holding — here the group's own list with one more subnet on the end.
        let mut modified_subnets = subnets.to_vec();
        modified_subnets.push("subnet-0aabbccddeeff0011");
        let mut parameters: Vec<(String, String)> =
            vec![("DBSubnetGroupName".to_string(), name.to_string())];
        for (index, subnet) in modified_subnets.iter().enumerate() {
            parameters.push((
                format!("SubnetIds.SubnetIdentifier.{}", index + 1),
                (*subnet).to_string(),
            ));
        }
        let modified = expect_ok(
            case,
            call(&router, "ModifyDBSubnetGroup", &parameters).await,
        );
        assert_subnet_group(case, "ModifyDBSubnetGroup", &modified, &modified_subnets);
    }

    // The same resolution the group's `Subnets` report is what the coverage refusal names:
    // a request that cannot cover the two Availability Zones a group needs is refused with
    // the coverage its own subnets add up to — which, with no owner of subnets mounted to
    // answer for them, is no zone at all.
    for (case, subnets) in [
        ("one subnet", vec!["subnet-0a1b2c3d4e5f60711"]),
        ("the same subnet twice", vec!["subnet-1a2b3c4d", "subnet-1a2b3c4d"]),
    ] {
        let refused = expect_error(
            case,
            create_subnet_group(&router, "emu-proj-subnets-thin", &subnets).await,
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        );
        let message = text(&refused, "Message");
        // The sentence pair is fixed; its one variable part is the coverage the request's
        // own subnets add up to, which here is empty because no zone was ever answered for
        // them.
        let expected = "The DB subnet group doesn't meet Availability Zone (AZ) coverage \
                        requirement. Current AZ coverage: . Add subnets to cover at least \
                        2 AZs."
            .to_string();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_subnet_group_name_already_taken_is_refused_as_the_api_words_it() {
    let router = emulator();

    // The sentence is about the name the REQUEST asked for, so it quotes that name in the
    // spelling the request carried, whichever case the group was created under ("stored as a
    // lowercase string" is what the group is keyed by, not what the refusal reports).
    let rows: [(&str, &str, &str); 2] = [
        ("the same name", "emu-proj-dup", "emu-proj-dup"),
        ("the same name in mixed case", "emu-proj-dup-2", "EMU-Proj-Dup-2"),
    ];

    for (case, created_as, taken_as) in rows {
        expect_ok(
            case,
            create_subnet_group(
                &router,
                created_as,
                &["subnet-0a1b2c3d4e5f60711", "subnet-0a1b2c3d4e5f60722"],
            )
            .await,
        );
        let refused = expect_error(
            case,
            create_subnet_group(
                &router,
                taken_as,
                &["subnet-0a1b2c3d4e5f60711", "subnet-0a1b2c3d4e5f60722"],
            )
            .await,
            400,
            "DBSubnetGroupAlreadyExists",
        );
        let message = text(&refused, "Message");
        let expected = format!("The DB subnet group '{taken_as}' already exists.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Parameter projection
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_parameter_group_reports_its_familys_catalog_with_its_metadata() {
    let router = emulator();

    // (case, family, a parameter of that family, its published apply type, data type,
    // allowed values). The rows span the control planes whose families this front door
    // serves and both apply types a catalog publishes.
    let rows: [(&str, &str, &str, &str, &str, &str); 4] = [
        (
            "a Neptune family",
            "neptune1.4",
            "neptune_enable_audit_log",
            "static",
            "boolean",
            "0,1",
        ),
        (
            "another version of the same Neptune family",
            "neptune1.2",
            "neptune_streams",
            "static",
            "boolean",
            "0,1",
        ),
        (
            "a DocumentDB family",
            "docdb5.0",
            "profiler",
            "dynamic",
            "string",
            "enabled,disabled",
        ),
        (
            "an Aurora MySQL family",
            "aurora-mysql8.0",
            "binlog_format",
            "static",
            "string",
            "ROW,STATEMENT,MIXED,OFF",
        ),
    ];

    for (index, (case, family, name, apply_type, data_type, allowed_values)) in
        rows.iter().enumerate()
    {
        let group = format!("emu-proj-cpg-{index}");
        expect_ok(
            case,
            call(
                &router,
                "CreateDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("DBParameterGroupFamily", family),
                    ("Description", "resource projection"),
                ],
            )
            .await,
        );
        // A group nobody has modified still reports its family's whole parameter list, each
        // parameter at the engine's own default.
        let reported = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[("DBClusterParameterGroupName", group.as_str())],
            )
            .await,
        );
        let parameters = subtrees(&reported, "Parameter");
        let found = parameters
            .iter()
            .find(|parameter| text(parameter, "ParameterName") == *name);
        let Some(found) = found else {
            panic!("case {case:?}: expected {family} to report {name}, got {reported}");
        };
        for (member, expected) in [
            ("ApplyType", *apply_type),
            ("DataType", *data_type),
            ("AllowedValues", *allowed_values),
            ("IsModifiable", "true"),
            ("Source", "engine-default"),
        ] {
            let value = optional_text(found, member);
            assert_eq!(
                value.as_deref(),
                Some(expected),
                "case {case:?}: expected {name}'s {member} {expected:?}, got {value:?} in \
                 {found}"
            );
        }
        let description = optional_text(found, "Description");
        assert!(
            description.is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected {name} to carry the engine's description, got {found}"
        );
        // Every cluster parameter carries the engine modes its family's clusters run in.
        let modes = texts(found, "member");
        assert!(
            modes.contains(&"provisioned".to_string()),
            "case {case:?}: expected {name}'s SupportedEngineModes to carry the mode its \
             clusters run in, got {modes:?}"
        );
    }
}

