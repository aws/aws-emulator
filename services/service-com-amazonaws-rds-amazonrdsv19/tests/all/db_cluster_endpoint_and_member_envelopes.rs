// SPDX-License-Identifier: CC0-1.0

//! What a `DBCluster` envelope publishes about WHERE the cluster is reached, WHO its members
//! are and WHICH secret holds its master user password.
//!
//! Three rules are under test, each asserted over a table of inputs rather than on one
//! recorded call.
//!
//! 1. An endpoint member of a DB cluster is a HOST. The model gives a cluster one
//!    `DBCluster$Port` ("the port that the database engine is listening on") beside its two
//!    endpoint members, so neither `Endpoint` nor `ReaderEndpoint` carries a port of its own
//!    and neither is ever written as a `host:port` authority; a client composes the address
//!    it dials from an endpoint and that one port. A cluster whose data plane this emulator
//!    SERVES publishes, for both members, the ADDRESS that plane is answered at; every other
//!    cluster publishes the documented DNS names the CLUSTER mints from its identifier,
//!    `<identifier>.cluster-<label>` and `<identifier>.cluster-ro-<label>`, under the endpoint
//!    domain of the control plane that owns its engine.
//!
//! 2. `DBClusterMember$PromotionTier` is "the order in which an Aurora Replica is promoted
//!    to the primary instance": a property of the MEMBER instance — the value its request
//!    named, else the documented default 1 — so the cluster reports one for every member,
//!    the member's own `DBInstance$PromotionTier` reports the same value, and
//!    `ModifyDBInstance` moves both.
//!
//! 3. The secret RDS manages for a cluster's master user password is named the way AWS names
//!    a secret of its own: the reserved prefix no caller can mint, the kind of resource the
//!    credentials belong to, and a UUID of the secret's own — `rds!cluster-<uuid>` — which is
//!    the name its `MasterUserSecret$SecretArn` carries.
//!
//! The rows differ along what the rules generalise over: the engines whose wire this
//! emulator serves and one whose wire it does not (which is what decides the FORM of a
//! published endpoint), both cluster types, every operation that returns a cluster envelope
//! — the create, the modify, the read and the delete — a request that names no failover
//! order beside each published bound of the member's `PromotionTier` and a value in between,
//! and two clusters whose managed secrets must not share a name.

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{call, emulator, expect_ok, optional_text, subtrees, text};
use crate::lifecycle_poll_loops::poll_until;
use crate::managed_master_user_password::{
    emulator as emulator_with_secrets, master_user_secret, the_resource,
};

/// The status a resource reports once the transition this service owes it has come due.
const AVAILABLE: &str = "available";

/// Poll a report until the DB CLUSTER `identifier` is available, and answer with the
/// envelope it settled on.
///
/// The CONDITION is waited for, never a span: the poll loop is the one this test target
/// shares ([`poll_until`]), whose deadline is generous beside the span a transition this
/// service owes becomes due after.
async fn polled_available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let params = [("DBClusterIdentifier", identifier)];
    let (body, _polls) = poll_until(
        case,
        "the cluster",
        || call(router, "DescribeDBClusters", &params),
        |status, body| {
            status == 200
                && subtrees(body, "DBCluster")
                    .first()
                    .is_some_and(|cluster| text(cluster, "Status") == AVAILABLE)
        },
    )
    .await;
    the_cluster(case, &body)
}

/// [`polled_available_cluster`] for a DB INSTANCE, which reports its status in a member of
/// its own.
async fn polled_available_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let params = [("DBInstanceIdentifier", identifier)];
    let (body, _polls) = poll_until(
        case,
        "the DB instance",
        || call(router, "DescribeDBInstances", &params),
        |status, body| {
            status == 200
                && subtrees(body, "DBInstance")
                    .first()
                    .is_some_and(|instance| text(instance, "DBInstanceStatus") == AVAILABLE)
        },
    )
    .await;
    the_resource(case, &body, "DBInstance")
}

/// One cluster the endpoint rules are asserted on.
struct EndpointCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// Whether this emulator serves the engine's native wire, which is what decides the form
    /// of the addresses the cluster publishes.
    served: bool,
    /// The members this row adds beyond the required ones.
    extra: &'static [(&'static str, &'static str)],
}

/// The settings a Multi-AZ DB cluster is required to be created with, with an `Iops` inside
/// the documented ratio of the allocated storage.
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "400"),
    ("DBClusterInstanceClass", "db.m5d.large"),
    ("Iops", "3000"),
    ("StorageType", "gp3"),
];

const ENDPOINT_CASES: &[EndpointCase] = &[
    EndpointCase {
        case: "an Aurora MySQL cluster",
        identifier: "emu-envelope-mysql",
        engine: "aurora-mysql",
        served: true,
        extra: &[],
    },
    EndpointCase {
        case: "an Aurora PostgreSQL cluster",
        identifier: "emu-envelope-pg",
        engine: "aurora-postgresql",
        served: true,
        extra: &[],
    },
    EndpointCase {
        case: "a Multi-AZ DB cluster, the other cluster type",
        identifier: "emu-envelope-multiaz",
        engine: "postgres",
        served: true,
        extra: MULTI_AZ_SETTINGS,
    },
    EndpointCase {
        case: "a cluster whose wire this emulator serves no data plane for",
        identifier: "emu-envelope-neptune",
        engine: "neptune",
        // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
        // emulator is reached at — and these requests are addressed to this service's own AWS
        // host, which is no address of this process's.
        served: false,
        extra: &[],
    },
    EndpointCase {
        case: "a cluster whose data plane is the document wire",
        identifier: "emu-envelope-docdb",
        engine: "docdb",
        served: true,
        extra: &[],
    },
];

/// Assert the two endpoint members and the port of one cluster envelope, whichever
/// operation answered with it.
fn the_published_address(case: &str, row: &EndpointCase, which: &str, cluster: &str) {
    let endpoint = text(cluster, "Endpoint");
    let reader = text(cluster, "ReaderEndpoint");
    let port = text(cluster, "Port");
    assert!(
        port.parse::<u16>().is_ok_and(|port| port != 0),
        "case {case:?} ({which}): expected the port the cluster's engine is reached on, got \
         {port:?}"
    );
    // Neither endpoint member carries a port: the one above is the pair's.
    for (member, value) in [("Endpoint", &endpoint), ("ReaderEndpoint", &reader)] {
        assert!(
            !value.is_empty() && !value.contains(':'),
            "case {case:?} ({which}): expected {member} to be a host, got {value:?}"
        );
        assert!(
            !value.ends_with(&port),
            "case {case:?} ({which}): expected {member} to leave the port to Port, got \
             {value:?}"
        );
    }
    if row.served {
        // A cluster whose engine wire this emulator serves publishes, for both members, the
        // address that wire is answered on.
        crate::db_clusters::assert_served_endpoint_pair(
            &format!("{case} ({which})"),
            &endpoint,
            &reader,
        );
        return;
    }
    assert!(
        endpoint.starts_with(&format!("{}.cluster-", row.identifier)),
        "case {case:?} ({which}): expected the DNS form of the writer endpoint, got \
         {endpoint:?}"
    );
    assert!(
        reader.starts_with(&format!("{}.cluster-ro-", row.identifier)),
        "case {case:?} ({which}): expected the DNS form of the reader endpoint, got {reader:?}"
    );
}

#[tokio::test]
async fn every_db_cluster_envelope_publishes_a_host_for_each_endpoint_member() {
    let router = emulator();
    for row in ENDPOINT_CASES {
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
        ];
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let created = the_cluster(
            row.case,
            &expect_ok(row.case, call(&router, "CreateDBCluster", &params).await),
        );
        the_published_address(row.case, row, "CreateDBCluster", &created);

        // The read path projects the stored cluster, so it reports what the create
        // published. The cluster is read once it is available, which is also what a
        // modification needs.
        let described = polled_available_cluster(&router, row.case, row.identifier).await;
        the_published_address(row.case, row, "DescribeDBClusters", &described);
        for member in ["Endpoint", "ReaderEndpoint", "Port"] {
            assert_eq!(
                text(&described, member),
                text(&created, member),
                "case {:?}: the {member} the create published must be the one the describe \
                 reports",
                row.case
            );
        }

        // A modification answers with the same envelope, and so does the delete, which
        // reports the cluster as it stood when the delete was accepted.
        let modified = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("BackupRetentionPeriod", "7"),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            ),
        );
        the_published_address(row.case, row, "ModifyDBCluster", &modified);
        polled_available_cluster(&router, row.case, row.identifier).await;
        let deleted = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            ),
        );
        the_published_address(row.case, row, "DeleteDBCluster", &deleted);
    }
}

/// One member instance the failover-order rule is asserted on: the `PromotionTier` its
/// create names, if any, and the order the cluster must then report for it.
struct PromotionCase {
    case: &'static str,
    identifier: &'static str,
    /// The value the create names, or `None` for a create that names none.
    requested: Option<&'static str>,
    /// What every envelope must report for the member.
    reported: &'static str,
}

/// The rows span what the rule generalises over: a create that names no order and takes the
/// documented default, each published bound of `PromotionTier` ("Valid Values: 0 - 15") and
/// one value in between.
const PROMOTION_CASES: &[PromotionCase] = &[
    PromotionCase {
        case: "a member created with no PromotionTier",
        identifier: "emu-tier-default",
        requested: None,
        reported: "1",
    },
    PromotionCase {
        case: "a member created in the first tier",
        identifier: "emu-tier-zero",
        requested: Some("0"),
        reported: "0",
    },
    PromotionCase {
        case: "a member created in a middle tier",
        identifier: "emu-tier-middle",
        requested: Some("7"),
        reported: "7",
    },
    PromotionCase {
        case: "a member created in the last tier",
        identifier: "emu-tier-last",
        requested: Some("15"),
        reported: "15",
    },
];

/// The `<DBClusterMember>` the cluster reports for `identifier`.
fn the_member(case: &str, cluster: &str, identifier: &str) -> String {
    subtrees(cluster, "DBClusterMember")
        .into_iter()
        .find(|member| text(member, "DBInstanceIdentifier") == identifier)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {identifier} among the cluster's members: {cluster}")
        })
}

/// `DescribeDBInstances` for one identifier, as the one `<DBInstance>` it reports.
async fn the_described_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    the_resource(case, &body, "DBInstance")
}

#[tokio::test]
async fn a_cluster_reports_the_failover_order_of_every_member() {
    let router = emulator();
    const CLUSTER: &str = "emu-tier-cluster";
    expect_ok(
        "the cluster the members join",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", CLUSTER),
                ("Engine", "aurora-postgresql"),
            ],
        )
        .await,
    );

    for row in PROMOTION_CASES {
        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", row.identifier.to_string()),
            ("Engine", "aurora-postgresql".to_string()),
            ("DBInstanceClass", "db.serverless".to_string()),
            ("DBClusterIdentifier", CLUSTER.to_string()),
        ];
        if let Some(requested) = row.requested {
            params.push(("PromotionTier", requested.to_string()));
        }
        let created = the_resource(
            row.case,
            &expect_ok(row.case, call(&router, "CreateDBInstance", &params).await),
            "DBInstance",
        );
        // The instance's own envelope reports the order it was created with.
        let reported = optional_text(&created, "PromotionTier");
        assert_eq!(
            reported.as_deref(),
            Some(row.reported),
            "case {:?}: expected the DB instance to report PromotionTier {:?}, got {reported:?}",
            row.case,
            row.reported
        );
    }

    // And the cluster reports the same order for each of them, beside the writer flag.
    let cluster = describe_one(&router, "the cluster's members", CLUSTER).await;
    for row in PROMOTION_CASES {
        let member = the_member(row.case, &cluster, row.identifier);
        let reported = optional_text(&member, "PromotionTier");
        assert_eq!(
            reported.as_deref(),
            Some(row.reported),
            "case {:?}: expected the cluster member to report PromotionTier {:?}, got \
             {reported:?}",
            row.case,
            row.reported
        );
        let described = the_described_instance(&router, row.case, row.identifier).await;
        assert_eq!(
            optional_text(&described, "PromotionTier").as_deref(),
            Some(row.reported),
            "case {:?}: the member and its own DB instance must report one order, got \
             {described}",
            row.case
        );
    }

    // `ModifyDBInstance` moves the order, and both envelopes follow it.
    let moved = PROMOTION_CASES[0].identifier;
    polled_available_instance(&router, "the member to modify", moved).await;
    expect_ok(
        "move the member's failover order",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", moved),
                ("PromotionTier", "9"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let described = the_described_instance(&router, "the modified member", moved).await;
    assert_eq!(
        optional_text(&described, "PromotionTier").as_deref(),
        Some("9"),
        "expected the modified member to report its new failover order, got {described}"
    );
    let cluster = describe_one(&router, "the cluster after the modification", CLUSTER).await;
    let member = the_member("the modified member", &cluster, moved);
    assert_eq!(
        optional_text(&member, "PromotionTier").as_deref(),
        Some("9"),
        "expected the cluster to report the member's new failover order, got {cluster}"
    );

    // An instance that belongs to NO cluster has no cluster to be promoted to primary in,
    // so it reports no order at all.
    expect_ok(
        "a standalone instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-tier-standalone"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t4g.micro"),
                ("AllocatedStorage", "20"),
            ],
        )
        .await,
    );
    let standalone =
        the_described_instance(&router, "a standalone instance", "emu-tier-standalone").await;
    assert_eq!(
        optional_text(&standalone, "PromotionTier"),
        None,
        "expected an instance that belongs to no cluster to report no PromotionTier, got \
         {standalone}"
    );
}

/// The two clusters the managed-secret naming is asserted on, so the name is shown to be
/// the secret's own rather than a value two resources could share.
const MANAGED_SECRET_CLUSTERS: &[&str] = &["emu-secret-name-one", "emu-secret-name-two"];

#[tokio::test]
async fn a_managed_master_user_secret_is_named_the_way_rds_names_one() {
    let router = emulator_with_secrets();
    let mut minted: Vec<(String, String)> = Vec::new();
    for identifier in MANAGED_SECRET_CLUSTERS {
        let created = the_cluster(
            identifier,
            &expect_ok(
                identifier,
                call(
                    &router,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", *identifier),
                        ("Engine", "aurora-postgresql"),
                        ("MasterUsername", "clusteradmin"),
                        ("ManageMasterUserPassword", "true"),
                    ],
                )
                .await,
            ),
        );
        let secret = master_user_secret(identifier, &created).unwrap_or_else(|| {
            panic!("case {identifier:?}: expected the cluster to report MasterUserSecret")
        });
        let arn = text(&secret, "SecretArn");
        // The name segment of the secret's ARN: the reserved prefix a caller cannot mint,
        // the kind of resource the credentials belong to, and the secret's own UUID — never
        // the cluster's own resource id, which AWS does not write into the name.
        let (_, name) = arn.rsplit_once(":secret:").unwrap_or_else(|| {
            panic!("case {identifier:?}: expected a Secrets Manager ARN, got {arn:?}")
        });
        let uuid = name
            .strip_prefix("rds!cluster-")
            .and_then(|rest| rest.rsplit_once('-'))
            .map(|(uuid, _suffix)| uuid.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "case {identifier:?}: expected a secret named 'rds!cluster-<uuid>', got \
                     {name:?}"
                )
            });
        let groups: Vec<usize> = uuid.split('-').map(str::len).collect();
        assert!(
            groups == vec![8, 4, 4, 4, 12]
                && uuid.split('-').all(|group| group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
            "case {identifier:?}: expected the secret's name to carry a UUID, got {name:?}"
        );
        let resource_id = text(&created, "DbClusterResourceId");
        assert!(
            !name.contains(&resource_id),
            "case {identifier:?}: a managed secret's name carries an opaque id, not the \
             cluster's own {resource_id:?}, got {name:?}"
        );
        // The read path reports the ARN the create minted, unchanged.
        let described = describe_one(&router, identifier, identifier).await;
        let read_back = master_user_secret(identifier, &described).unwrap_or_else(|| {
            panic!("case {identifier:?}: expected the read to report MasterUserSecret")
        });
        assert_eq!(
            text(&read_back, "SecretArn"),
            arn,
            "case {identifier:?}: the read must report the secret the create minted"
        );
        minted.push(((*identifier).to_string(), uuid));
    }
    let [(left, left_uuid), (right, right_uuid)] = &minted[..] else {
        panic!("expected both clusters to report a managed secret, got {minted:?}");
    };
    assert_ne!(
        left_uuid, right_uuid,
        "clusters {left} and {right} were given the same managed-secret name {left_uuid:?}"
    );
}
