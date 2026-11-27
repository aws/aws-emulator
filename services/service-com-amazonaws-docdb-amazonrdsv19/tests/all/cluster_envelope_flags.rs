// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members Amazon DocumentDB's cluster and instance envelopes
//! carry whatever the request that made the resource asked for: on the cluster the
//! Serverless platform version it runs on, on the instance the attribute set at the end of
//! this file.
//!
//! The rule: a cluster reports a `ServerlessV2PlatformVersion` exactly where it HAS
//! Serverless capacity configured — "the version of the Amazon DocumentDB Serverless
//! platform used by the cluster" — and that version is the one the published catalogue
//! designates the engine's default, "the version used for new DB clusters"
//! (`state::default_serverless_v2_platform_version`, the same catalogue
//! `DescribeServerlessV2PlatformVersions` reports from). A cluster with no Serverless
//! capacity runs on no Serverless platform and reports none. It is projected out of the
//! cluster record by the one projection every path shares
//! (`crate::state::cluster_to_shape`).
//!
//! The rule generalises over the cluster's other inputs (none of them touches the platform
//! version) and over the operation whose envelope carries the cluster, so the table below
//! spans BOTH: each row is a differently shaped `CreateDBCluster`, and every row is read
//! through the create, describe, modify and delete envelopes in turn.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production router uses, with the wire harness this target shares
//! (`tests/all/cluster_lifecycle.rs`).

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{
    count, let_the_clock_catch_up, opt_text, text, texts, Docdb, REGION,
};

/// The platform version a Serverless cluster of this engine runs on: the row the published
/// catalogue designates the engine's default, read from the catalogue itself so this
/// assertion and `DescribeServerlessV2PlatformVersions` cannot name different versions.
fn published_platform_version() -> &'static str {
    state::default_serverless_v2_platform_version()
        .map(|platform| platform.version)
        .expect("the published catalogue designates a default platform version")
}

/// Assert that `xml` reports `ServerlessV2PlatformVersion` exactly where the cluster was
/// created with Serverless capacity, and nothing where it was not — asserted on the raw
/// XML, so a member the serializer omits fails rather than reading as a default.
fn assert_platform_version(case: &str, xml: &str, serverless: bool) {
    let actual = opt_text(xml, "ServerlessV2PlatformVersion");
    let expected = serverless.then(published_platform_version);
    assert_eq!(
        actual.as_deref(),
        expected,
        "case {case:?}: the cluster envelope must report \
         ServerlessV2PlatformVersion={expected:?}, got {actual:?} in {xml}"
    );
}

/// One row of the table: a case name and the extra parameters its `CreateDBCluster` sends
/// beyond the identifier and the engine.
struct CreateCase {
    label: &'static str,
    identifier: &'static str,
    params: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    /// Whether this row's create configures Serverless capacity, which is the one input the
    /// platform version follows.
    fn configures_serverless(&self) -> bool {
        self.params
            .iter()
            .any(|(name, _)| name.starts_with("ServerlessV2ScalingConfiguration"))
    }
}

/// The table: clusters differing in the inputs `CreateDBCluster` takes — credentials,
/// port, retention, encryption, protection, logs exports, storage type, scaling, network
/// type, tags and availability zones. Only the scaling configuration is an input to the
/// member under test, which is exactly the rule.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the bare minimum",
            identifier: "flags-minimal",
            params: vec![],
        },
        CreateCase {
            label: "with master credentials",
            identifier: "flags-credentials",
            params: vec![
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        },
        CreateCase {
            label: "encrypted, with a key and a long retention",
            identifier: "flags-encrypted",
            params: vec![
                ("StorageEncrypted", "true"),
                ("KmsKeyId", "alias/aws/rds"),
                ("BackupRetentionPeriod", "35"),
            ],
        },
        CreateCase {
            label: "protected from deletion, on a chosen port",
            identifier: "flags-protected",
            params: vec![("DeletionProtection", "true"), ("Port", "27018")],
        },
        CreateCase {
            label: "exporting logs, copying tags, on the iopt1 storage type",
            identifier: "flags-logs",
            params: vec![
                ("EnableCloudwatchLogsExports.member.1", "audit"),
                ("CopyTagsToSnapshot", "true"),
                ("StorageType", "iopt1"),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "tests"),
            ],
        },
        CreateCase {
            label: "serverless, in two availability zones, on a chosen network type",
            identifier: "flags-serverless",
            params: vec![
                ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
                ("ServerlessV2ScalingConfiguration.MaxCapacity", "8"),
                ("AvailabilityZones.AvailabilityZone.1", "us-east-1a"),
                ("AvailabilityZones.AvailabilityZone.2", "us-east-1b"),
                ("NetworkType", "IPV4"),
            ],
        },
        CreateCase {
            label: "serverless at a wider capacity range, encrypted, with credentials",
            identifier: "flags-serverless-wide",
            params: vec![
                ("ServerlessV2ScalingConfiguration.MinCapacity", "2"),
                ("ServerlessV2ScalingConfiguration.MaxCapacity", "32.5"),
                ("StorageEncrypted", "true"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        },
    ]
}

#[tokio::test]
async fn every_cluster_envelope_reports_the_platform_version_the_create_configured() {
    let docdb = Docdb::new();

    for case in create_cases() {
        let serverless = case.configures_serverless();
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", case.identifier), ("Engine", "docdb")];
        params.extend(case.params.iter().copied());

        // 1. The create envelope.
        let created = docdb.ok(case.label, "CreateDBCluster", &params).await;
        assert_platform_version(&format!("{} / create", case.label), &created, serverless);

        let_the_clock_catch_up();

        // 2. The describe envelope, which reads the record back out of state.
        let described = docdb
            .ok(
                case.label,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", case.identifier)],
            )
            .await;
        assert_platform_version(&format!("{} / describe", case.label), &described, serverless);

        // 3. The modify envelope — and a modify that changes something else must leave the
        //    platform version alone.
        let modified = docdb
            .ok(
                case.label,
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", case.identifier),
                    ("BackupRetentionPeriod", "7"),
                    ("DeletionProtection", "false"),
                ],
            )
            .await;
        assert_eq!(
            text(&modified, "BackupRetentionPeriod"),
            "7",
            "case {:?}: the modify must really have been applied, got {modified}",
            case.label
        );
        assert_platform_version(&format!("{} / modify", case.label), &modified, serverless);

        // 4. The delete envelope, the last path that reports a cluster.
        let deleted = docdb
            .ok(
                case.label,
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", case.identifier),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await;
        assert_eq!(
            text(&deleted, "Status"),
            "deleting",
            "case {:?}: the delete must report the deleting status, got {deleted}",
            case.label
        );
        assert_platform_version(&format!("{} / delete", case.label), &deleted, serverless);
    }
}

#[tokio::test]
async fn a_multi_cluster_page_reports_the_platform_version_of_every_cluster_it_lists() {
    let docdb = Docdb::new();
    // Two Serverless clusters and one provisioned one, so the page carries the member on
    // exactly the clusters that run on a Serverless platform.
    let serverless = ["page-one", "page-two"];
    let provisioned = ["page-three"];
    for identifier in serverless {
        docdb
            .ok(
                identifier,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", "docdb"),
                    ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
                    ("ServerlessV2ScalingConfiguration.MaxCapacity", "8"),
                ],
            )
            .await;
    }
    for identifier in provisioned {
        docdb
            .ok(
                identifier,
                "CreateDBCluster",
                &[("DBClusterIdentifier", identifier), ("Engine", "docdb")],
            )
            .await;
    }
    let_the_clock_catch_up();

    let page = docdb.ok("whole page", "DescribeDBClusters", &[]).await;
    assert_eq!(
        count(&page, "DBCluster"),
        serverless.len() + provisioned.len(),
        "case \"whole page\": expected every cluster, got {page}"
    );
    assert_eq!(
        count(&page, "ServerlessV2PlatformVersion"),
        serverless.len(),
        "case \"whole page\": only the Serverless clusters report a platform version, \
         got {page}"
    );
    let expected = published_platform_version();
    for (index, value) in texts(&page, "ServerlessV2PlatformVersion")
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            value, expected,
            "case \"whole page\" (cluster {index}): expected the published platform version \
             {expected:?}, got {value:?} in {page}"
        );
    }
}

#[tokio::test]
async fn an_instance_keeps_its_own_documented_minor_version_upgrade_default() {
    // `CreateDBInstanceMessage$AutoMinorVersionUpgrade` documents "Default: false" and is a
    // real input, so an instance reports the flag it was created with and nothing else
    // chooses it for it.
    let docdb = Docdb::new();
    docdb
        .ok(
            "host cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "flags-host"), ("Engine", "docdb")],
        )
        .await;
    for (case, identifier, sent, expected) in [
        ("no input", "flags-member-default", None, "false"),
        (
            "explicit false",
            "flags-member-off",
            Some("false"),
            "false",
        ),
        ("explicit true", "flags-member-on", Some("true"), "true"),
    ] {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBInstanceIdentifier", identifier),
            ("DBInstanceClass", "db.r5.large"),
            ("Engine", "docdb"),
            ("DBClusterIdentifier", "flags-host"),
        ];
        if let Some(sent) = sent {
            params.push(("AutoMinorVersionUpgrade", sent));
        }
        let created = docdb.ok(case, "CreateDBInstance", &params).await;
        assert_eq!(
            opt_text(&created, "AutoMinorVersionUpgrade").as_deref(),
            Some(expected),
            "case {case:?}: an instance reports the flag it was created with, got {created}"
        );
    }

    let_the_clock_catch_up();
    let cluster = docdb
        .ok(
            "host cluster describe",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "flags-host")],
        )
        .await;
    // The host cluster was created with no Serverless capacity, so it runs on no Serverless
    // platform however many members it gained.
    assert_platform_version("host cluster describe", &cluster, false);
}

// ---------------------------------------------------------------------------
// The INSTANCE envelope: the attribute set every Amazon DocumentDB instance carries
// ---------------------------------------------------------------------------

/// The members an instance envelope carries with the same value whatever the instance is,
/// and the value each one has for this engine.
///
/// Sources: `DBInstance$PubliclyAccessible` states "Amazon DocumentDB does not currently
/// support public endpoints. The value of `PubliclyAccessible` is always `false`" (cited on
/// the constant in `src/state.rs` that projects it); `DBInstance$PendingModifiedValues` is
/// "specifies that changes to the instance are pending", one subelement per pending change.
const INSTANCE_ATTRIBUTES: [(&str, &str); 2] = [
    ("PubliclyAccessible", "false"),
    // Reported on every instance, one subelement per pending change: an instance with
    // nothing pending carries the element with nothing inside it.
    ("PendingModifiedValues", ""),
];

/// One row of the instance table: what its cluster was created with, what the instance
/// asked for, and the members whose value follows from those.
struct InstanceCase {
    label: &'static str,
    cluster: &'static str,
    instance: &'static str,
    /// The `MasterUsername` the cluster was created with, when it was created with one.
    master_username: Option<&'static str>,
    /// The `StorageType` the cluster named, when it named one.
    storage_type: Option<&'static str>,
    /// The `PromotionTier` the instance asked for, when it asked for one.
    promotion_tier: Option<&'static str>,
    /// The promotion tier the instance must report; `CreateDBInstanceMessage$PromotionTier`
    /// documents "Default: 1 Valid values: 0-15" for one that asked for none.
    expected_promotion_tier: &'static str,
}

/// The table: instances differing along what the rule generalises over — whether their
/// cluster carries a master user, which of the engine's storage types manages their
/// storage, and which promotion tier they were created at, including both published bounds
/// of that range.
fn instance_cases() -> [InstanceCase; 4] {
    [
        InstanceCase {
            label: "a cluster with a master user, on the default storage type",
            cluster: "inst-attrs-default",
            instance: "inst-attrs-default-1",
            master_username: Some("probeadmin"),
            storage_type: None,
            promotion_tier: Some("1"),
            expected_promotion_tier: "1",
        },
        InstanceCase {
            label: "an instance that asks for no promotion tier",
            cluster: "inst-attrs-notier",
            instance: "inst-attrs-notier-1",
            master_username: Some("dbadmin"),
            storage_type: Some("standard"),
            promotion_tier: None,
            expected_promotion_tier: "1",
        },
        InstanceCase {
            label: "a cluster on the iopt1 storage type, at the top of the tier range",
            cluster: "inst-attrs-iopt1",
            instance: "inst-attrs-iopt1-1",
            master_username: Some("ioadmin"),
            storage_type: Some("iopt1"),
            promotion_tier: Some("15"),
            expected_promotion_tier: "15",
        },
        InstanceCase {
            label: "a cluster created without a master user, at the bottom of the range",
            cluster: "inst-attrs-nouser",
            instance: "inst-attrs-nouser-1",
            master_username: None,
            storage_type: None,
            promotion_tier: Some("0"),
            expected_promotion_tier: "0",
        },
    ]
}

/// Assert that `xml` carries the whole instance attribute set — the members fixed for this
/// engine ([`INSTANCE_ATTRIBUTES`]) and the ones that follow from the row. Asserted on the
/// raw XML, so a member the serializer omits fails rather than reading as a default.
fn assert_instance_attributes(case: &str, row: &InstanceCase, xml: &str) {
    for (member, expected) in INSTANCE_ATTRIBUTES {
        let actual = opt_text(xml, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: the instance envelope must carry {member}={expected:?}, \
             got {actual:?} in {xml}"
        );
    }
    for (member, expected) in [
        ("PromotionTier", Some(row.expected_promotion_tier)),
        // The master user belongs to the CLUSTER: this model's `DBInstance` declares no
        // `MasterUsername`, so the instance envelope reports none whatever its cluster was
        // created with.
        ("MasterUsername", None),
    ] {
        let actual = opt_text(xml, member);
        assert_eq!(
            actual.as_deref(),
            expected,
            "case {case:?}: the instance envelope must report {member}={expected:?}, \
             got {actual:?} in {xml}"
        );
    }
}

#[tokio::test]
async fn every_instance_envelope_carries_the_documentdb_instance_attribute_set() {
    let docdb = Docdb::new();

    for row in instance_cases() {
        let mut cluster_params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", row.cluster), ("Engine", "docdb")];
        if let Some(user) = row.master_username {
            cluster_params.push(("MasterUsername", user));
            cluster_params.push(("MasterUserPassword", "secret99"));
        }
        if let Some(storage) = row.storage_type {
            cluster_params.push(("StorageType", storage));
        }
        docdb
            .ok(row.label, "CreateDBCluster", &cluster_params)
            .await;

        let mut instance_params: Vec<(&str, &str)> = vec![
            ("DBInstanceIdentifier", row.instance),
            ("DBInstanceClass", "db.r5.large"),
            ("Engine", "docdb"),
            ("DBClusterIdentifier", row.cluster),
        ];
        if let Some(tier) = row.promotion_tier {
            instance_params.push(("PromotionTier", tier));
        }

        // 1. The create envelope.
        let created = docdb
            .ok(row.label, "CreateDBInstance", &instance_params)
            .await;
        assert_instance_attributes(&format!("{} / create", row.label), &row, &created);

        // 2. The describe envelope, which reads the record back out of state: the same
        //    projection, so the read reports exactly what the create announced. The
        //    attribute set is a property of the record and its cluster, so it does not wait
        //    on the status the clock owes (`cluster_lifecycle.rs` pins that transition).
        let described = docdb
            .ok(
                row.label,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", row.instance)],
            )
            .await;
        assert_instance_attributes(&format!("{} / describe", row.label), &row, &described);
    }

    // 3. The delete envelope, the last path that reports an instance. A delete is refused
    //    while the instance is still coming up, so the clock is let past ONCE here rather
    //    than once per row.
    let_the_clock_catch_up();
    for row in instance_cases() {
        let deleted = docdb
            .ok(
                row.label,
                "DeleteDBInstance",
                &[("DBInstanceIdentifier", row.instance)],
            )
            .await;
        assert_instance_attributes(&format!("{} / delete", row.label), &row, &deleted);
    }
}

#[tokio::test]
async fn a_multi_instance_page_carries_the_attribute_set_on_every_instance_it_lists() {
    let docdb = Docdb::new();
    docdb
        .ok(
            "host cluster",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "inst-page-host"),
                ("Engine", "docdb"),
                ("MasterUsername", "pageadmin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await;
    // The cluster's writer and two replicas: the attribute set is the same on every member,
    // whatever its place in the cluster.
    let identifiers = ["inst-page-1", "inst-page-2", "inst-page-3"];
    for identifier in identifiers {
        docdb
            .ok(
                identifier,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", "inst-page-host"),
                ],
            )
            .await;
    }

    let page = docdb.ok("whole page", "DescribeDBInstances", &[]).await;
    assert_eq!(
        count(&page, "DBInstance"),
        identifiers.len(),
        "case \"whole page\": expected every instance, got {page}"
    );
    for (member, expected) in INSTANCE_ATTRIBUTES {
        assert_eq!(
            count(&page, member),
            identifiers.len(),
            "case \"whole page\": every listed instance must carry one {member}, got {page}"
        );
        for (index, value) in texts(&page, member).into_iter().enumerate() {
            assert_eq!(
                value, expected,
                "case \"whole page\" (instance {index}): expected {member}={expected:?}, \
                 got {value:?} in {page}"
            );
        }
    }
    for member in ["MasterUsername", "StorageType"] {
        assert_eq!(
            count(&page, member),
            0,
            "case \"whole page\": this model's DBInstance declares no {member}, so no listed \
             instance may carry one, got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// The address an instance publishes
// ---------------------------------------------------------------------------

/// One row of the published-address table.
struct AddressCase {
    label: &'static str,
    /// The `Host` header the request carries — the address the client is talking to — or
    /// `None` for a request that addresses no host of its own.
    host: Option<&'static str>,
    /// The URL this emulator's data port is published at, or `None` for a router served on
    /// no port.
    served_base: Option<&'static str>,
    /// The `Port` the cluster is created with, when the row names one: the engine port the
    /// CLUSTER publishes, which an instance served on a socket of its own does not report,
    /// because what it publishes is the address and port that socket answers at.
    cluster_port: Option<&'static str>,
}

#[tokio::test]
async fn an_instance_publishes_the_address_its_own_socket_answers_on() {
    // RULE: an endpoint a service advertises is an address the service ANSWERS on, so an
    // instance whose data plane this emulator serves publishes the host and port of the
    // socket the core opened for it — never a name in Amazon DocumentDB's own DNS zone,
    // which resolves to nothing on the machine this emulator runs on.
    //
    // The rows span how the emulator is addressed and how it is served: the default data
    // port, an ephemeral one, loopback by name, an IPv6 literal whose own colons live
    // inside its brackets, an authority naming no port, the two ways a request can name no
    // address of this process, and a cluster that chose its engine port. None of them
    // changes the rule — what an instance publishes is its OWN socket, not the address the
    // control-plane request happened to arrive at.
    let cases = [
        AddressCase {
            label: "the front door on the emulator's default data port",
            host: Some("127.0.0.1:9297"),
            served_base: None,
            cluster_port: None,
        },
        AddressCase {
            label: "an ephemeral data port, on a cluster that chose its engine port",
            host: Some("127.0.0.1:44903"),
            served_base: None,
            cluster_port: Some("27018"),
        },
        AddressCase {
            label: "loopback by name",
            host: Some("localhost:41509"),
            served_base: None,
            cluster_port: None,
        },
        AddressCase {
            label: "an IPv6 loopback literal",
            host: Some("[::1]:38001"),
            served_base: None,
            cluster_port: None,
        },
        AddressCase {
            label: "an authority that names no port of its own",
            host: Some("127.0.0.1"),
            served_base: None,
            cluster_port: None,
        },
        AddressCase {
            label: "a request that named no host, on an emulator served on a port",
            host: None,
            served_base: Some("https://127.0.0.1:9297"),
            cluster_port: None,
        },
        AddressCase {
            label: "a request that addressed an AWS host, on an emulator served on a port",
            host: Some("docdb.us-east-1.amazonaws.com"),
            served_base: Some("https://127.0.0.1:35117"),
            cluster_port: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let docdb = match case.served_base {
            Some(base) => Docdb::served_at(base),
            None => Docdb::new(),
        };
        let cluster = format!("published-cluster-{index}");
        let instance = format!("published-instance-{index}");
        let mut cluster_params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", cluster.as_str()),
            ("Engine", "docdb"),
        ];
        if let Some(port) = case.cluster_port {
            cluster_params.push(("Port", port));
        }
        let created_cluster = call(&docdb, case.host, "CreateDBCluster", &cluster_params).await;
        assert_eq!(
            created_cluster.status, 200,
            "case {:?}: CreateDBCluster must succeed, got {} with {}",
            case.label, created_cluster.status, created_cluster.xml
        );
        let created = call(
            &docdb,
            case.host,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance.as_str()),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", cluster.as_str()),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateDBInstance must succeed, got {} with {}",
            case.label, created.status, created.xml
        );

        let described = call(
            &docdb,
            case.host,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", instance.as_str())],
        )
        .await;
        // ROUND TRIP: the describe reports the address the create published, because the
        // create resolved it once — from the socket it opened — and stored it on the record.
        let mut published: Vec<(String, String)> = Vec::new();
        for (path, xml) in [("create", &created.xml), ("describe", &described.xml)] {
            let address = text(xml, "Address");
            let port = text(xml, "Port");
            assert!(
                !address.ends_with(&format!(".{REGION}.docdb.amazonaws.com")),
                "case {:?} / {path}: the published address must be one this emulator answers \
                 on, not a name in Amazon DocumentDB's own DNS zone, got {address:?} in {xml}",
                case.label
            );
            assert!(
                !address.contains("://"),
                "case {:?} / {path}: Endpoint.Address is a host, not a URL, got {address:?}",
                case.label
            );
            // A host of this emulator's: the core binds its listeners on the operator-chosen
            // interface, loopback by default, so the published host is an address literal of
            // this machine rather than a name that has to be resolved.
            assert!(
                address
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok(),
                "case {:?} / {path}: expected the address of the interface the core bound, \
                 got {address:?} in {xml}",
                case.label
            );
            assert!(
                port.parse::<u16>().is_ok_and(|port| port > 0),
                "case {:?} / {path}: expected the port the core assigned the instance's \
                 socket, got {port:?} in {xml}",
                case.label
            );
            // The point of the pair: `<Address>:<Port>` is ONE address a client dials, so it
            // parses as a URL naming exactly the published port.
            let dialled = format!("https://{address}:{port}/")
                .parse::<http::Uri>()
                .unwrap_or_else(|error| {
                    panic!(
                        "case {:?} / {path}: https://{address}:{port}/ must be a dialable \
                         URL: {error}",
                        case.label
                    )
                });
            assert_eq!(
                dialled.port_u16().map(|found| found.to_string()).as_deref(),
                Some(port.as_str()),
                "case {:?} / {path}: expected the dialed URL to name port {port:?}, got {:?}",
                case.label,
                dialled.port_u16()
            );
            published.push((address, port));
        }
        assert_eq!(
            published.first(),
            published.last(),
            "case {:?}: the describe must report the address the create published, got \
             {published:?}",
            case.label
        );
        // The cluster's own `Port` member is the engine port its endpoints publish, and is
        // untouched by what the instance's socket answers on.
        if let Some(requested) = case.cluster_port {
            let cluster_port = text(&created_cluster.xml, "Port");
            assert_eq!(
                cluster_port, requested,
                "case {:?}: the cluster must still report the engine port it was created \
                 with, got {cluster_port:?} in {}",
                case.label, created_cluster.xml
            );
        }
    }
}

#[tokio::test]
async fn each_member_of_a_cluster_publishes_the_address_of_its_own_socket() {
    // The pair an instance publishes is ONE address a client dials: the host and port of the
    // socket this emulator answers that instance on. The cluster goes on reporting the engine
    // port it was created with (`CreateDBClusterMessage$Port`: "Default: 27017"), which the
    // rows vary — the member's own endpoint is its socket's either way, because the core owns
    // the port range and takes no port from this service.
    for (label, requested_port, expected_cluster_port) in [
        ("no Port member, so the engine default", None, "27017"),
        ("a Port member the cluster chose", Some("27018"), "27018"),
    ] {
        let docdb = Docdb::new();
        let cluster = format!("member-address-cluster-{expected_cluster_port}");
        let instance = format!("member-address-instance-{expected_cluster_port}");
        let mut cluster_params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", cluster.as_str()),
            ("Engine", "docdb"),
        ];
        if let Some(port) = requested_port {
            cluster_params.push(("Port", port));
        }
        let created_cluster = docdb.ok(label, "CreateDBCluster", &cluster_params).await;
        assert_eq!(
            text(&created_cluster, "Port"),
            expected_cluster_port,
            "case {label:?}: the cluster reports the engine port it was created with, got \
             {created_cluster}"
        );
        let created = docdb
            .ok(
                label,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", instance.as_str()),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", cluster.as_str()),
                ],
            )
            .await;
        let address = text(&created, "Address");
        let port = text(&created, "Port");
        assert!(
            address
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok(),
            "case {label:?}: expected the address its socket is bound on, got {address:?} in \
             {created}"
        );
        assert!(
            port.parse::<u16>().is_ok_and(|port| port > 0),
            "case {label:?}: expected the port the core assigned its socket, got {port:?} in \
             {created}"
        );
        // Two instances of one cluster are two addresses onto its storage, each served on a
        // socket of its own — so each publishes its own pair, which is what tells them apart.
        let second_identifier = format!("{instance}-2");
        let second = docdb
            .ok(
                label,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", second_identifier.as_str()),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", cluster.as_str()),
                ],
            )
            .await;
        assert_ne!(
            (text(&second, "Address"), text(&second, "Port")),
            (address, port),
            "case {label:?}: a second member must publish its own socket's address, got \
             {second}"
        );
    }
}

/// Drive one awsQuery call, with or without a `Host` header.
async fn call(
    docdb: &Docdb,
    host: Option<&str>,
    action: &str,
    params: &[(&str, &str)],
) -> crate::cluster_lifecycle::Reply {
    match host {
        Some(host) => docdb.call_addressed_to(host, action, params).await,
        None => docdb.call(action, params).await,
    }
}
