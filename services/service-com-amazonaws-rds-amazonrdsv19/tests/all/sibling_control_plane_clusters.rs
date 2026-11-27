// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB clusters this front door creates for ANOTHER control
//! plane's engine, and for the two refusals the cluster lifecycle turns on.
//!
//! One awsQuery front door serves three control planes: this model's wire identity
//! (`endpointPrefix` / `sigv4Name` `rds`, `awsQuery`, version `2014-10-31`) is published
//! identically by the Amazon DocumentDB and Amazon Neptune models, so a client compiled
//! against either of those APIs signs for `rds` and is answered HERE (`src/service.rs`).
//! A cluster of such an engine is still a resource of THAT service, so what it reports is
//! that service's published values and not this model's.
//!
//! The rules pinned here, each over a SET of inputs rather than one request:
//!
//! * a cluster created without a `Port` listens on the port its ENGINE's own control plane
//!   publishes as the default, and the endpoints of the cluster and of its member instances
//!   sit in that control plane's DNS domain — spanned over the two sibling engines, two
//!   Aurora engines and a Multi-AZ DB cluster engine, with a request that names its own
//!   port as the control;
//! * a cluster whose storage size is not fixed reports `AllocatedStorage` as the one
//!   gibibyte the member is expressed in, while a Multi-AZ DB cluster reports the size its
//!   request allocated — same table;
//! * a cluster takes the `StorageType` value list its engine's own control plane publishes,
//!   reports a non-default type and does not report the default one — spanned over both
//!   sibling engines, every value each of them publishes, the request that names none, and
//!   a value belonging to another cluster type;
//! * a delete that names neither of its two final-snapshot members is refused
//!   `InvalidParameterCombination`, asking for one of them — spanned over the engines and
//!   over the snapshot-member combinations;
//! * a `MasterUserPassword` outside its documented length or character set is refused
//!   `InvalidParameterValue`, naming the member and the constraint it broke — spanned over
//!   both bounds, the values just inside them and the forbidden characters, on the create
//!   and on the modify.

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// The DNS domain the Amazon DocumentDB control plane's endpoints sit in.
const DOCDB_DOMAIN: &str = "docdb.amazonaws.com";

/// The DNS domain the Amazon Neptune control plane's endpoints sit in.
const NEPTUNE_DOMAIN: &str = "neptune.amazonaws.com";

/// The DNS domain this front door's own endpoints sit in.
const RDS_DOMAIN: &str = "rds.amazonaws.com";

/// The instance class the orderable-options report offers for a cluster member of either
/// sibling control plane's engine.
const CLUSTER_MEMBER_CLASS: &str = "db.r5.large";

/// `CreateDBCluster` with `DBClusterIdentifier` and `Engine` plus `extra`.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&'static str, &str)],
) -> (u16, String) {
    let mut parameters: Params = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
    ];
    parameters.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBCluster", &parameters).await
}

// ---------------------------------------------------------------------------
// The defaults a cluster takes from its engine's own control plane
// ---------------------------------------------------------------------------

/// One row of the engine table.
struct EngineCase {
    case: &'static str,
    engine: &'static str,
    /// The parameters this row adds beyond the two required members.
    extra: &'static [(&'static str, &'static str)],
    /// The port the cluster must report, as the text the wire carries — for a cluster whose
    /// engine's wire this emulator does NOT serve, which is the only one that publishes its
    /// configured port.
    expected_port: &'static str,
    /// Whether this emulator serves the engine's own wire for such a cluster, so the
    /// cluster publishes the address of its own engine listener rather than a DNS name and
    /// the configured port.
    serves_the_engines_wire: bool,
    /// Whether the cluster publishes an address of THIS emulator's for its two endpoint
    /// members: true for every cluster whose data plane this emulator answers on a listener of
    /// its own, including the one whose wire is the DOCUMENT protocol — whose endpoint members
    /// carry no port of their own, so such a cluster publishes the host its plane is answered
    /// at beside the port its ENGINE is configured to listen on, which is the pair a client
    /// composes.
    publishes_an_own_host: bool,
    /// The DNS domain both cluster endpoints must sit in, for a cluster that publishes names.
    expected_domain: &'static str,
    /// The `AllocatedStorage` the cluster must report.
    expected_allocated_storage: &'static str,
}

/// The four settings a Multi-AZ DB cluster is required to be created with
/// (`AllocatedStorage`, `DBClusterInstanceClass`, `Iops` and `StorageType`, each
/// documented "This setting is required to create a Multi-AZ DB cluster"), with an `Iops`
/// inside the documented ratio of the allocated storage.
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "400"),
    ("DBClusterInstanceClass", "db.m5d.large"),
    ("Iops", "3000"),
    ("StorageType", "gp3"),
];

/// The clusters the table creates. The rows differ along what the rules generalise over:
/// the control plane that owns the engine, the cluster TYPE (Aurora-style versus Multi-AZ
/// DB cluster), whether the request names its own port, and whether the engine's storage
/// size is fixed.
const ENGINE_CASES: &[EngineCase] = &[
    EngineCase {
        case: "a DocumentDB cluster, nothing beyond the two required members",
        engine: "docdb",
        extra: &[],
        // The port Amazon DocumentDB publishes as its default.
        expected_port: "27017",
        serves_the_engines_wire: false,
        publishes_an_own_host: true,
        expected_domain: DOCDB_DOMAIN,
        expected_allocated_storage: "1",
    },
    EngineCase {
        case: "a DocumentDB cluster that names its own port",
        engine: "docdb",
        extra: &[("Port", "27018")],
        // The request's port wins: the default only fills a port nobody asked for.
        expected_port: "27018",
        serves_the_engines_wire: false,
        publishes_an_own_host: true,
        expected_domain: DOCDB_DOMAIN,
        expected_allocated_storage: "1",
    },
    EngineCase {
        case: "a Neptune cluster",
        engine: "neptune",
        extra: &[],
        // `CreateDBClusterMessage$Port` of the Neptune model: "Default: 8182".
        expected_port: "8182",
        serves_the_engines_wire: false,
        publishes_an_own_host: false,
        expected_domain: NEPTUNE_DOMAIN,
        expected_allocated_storage: "1",
    },
    EngineCase {
        case: "an Aurora MySQL cluster",
        engine: "aurora-mysql",
        extra: &[],
        // "RDS for MySQL and Aurora MySQL - 3306".
        expected_port: "3306",
        serves_the_engines_wire: true,
        publishes_an_own_host: true,
        expected_domain: RDS_DOMAIN,
        expected_allocated_storage: "1",
    },
    EngineCase {
        case: "an Aurora PostgreSQL cluster",
        engine: "aurora-postgresql",
        extra: &[],
        // "RDS for PostgreSQL and Aurora PostgreSQL - 5432".
        expected_port: "5432",
        serves_the_engines_wire: true,
        publishes_an_own_host: true,
        expected_domain: RDS_DOMAIN,
        expected_allocated_storage: "1",
    },
    EngineCase {
        case: "a Multi-AZ DB cluster, whose storage size IS fixed",
        engine: "postgres",
        extra: MULTI_AZ_SETTINGS,
        expected_port: "5432",
        serves_the_engines_wire: true,
        publishes_an_own_host: true,
        expected_domain: RDS_DOMAIN,
        // The size the request allocated to each DB instance in the cluster.
        expected_allocated_storage: "400",
    },
];

/// Assert one cluster envelope reports this row's port, endpoints and allocated storage.
fn assert_engine_defaults(case: &str, label: &str, cluster: &str, row: &EngineCase) {
    let case = format!("{case} ({label})");
    let port = text(cluster, "Port");
    if row.serves_the_engines_wire {
        // The cluster publishes where a client DIALS it: the port its own engine listener
        // answers on, which the core assigns, not the engine's configured default.
        assert!(
            port.parse::<u16>().is_ok_and(|port| port != 0),
            "case {case:?}: expected the port the cluster's engine listener answers on, got \
             {port:?} in {cluster}"
        );
    } else {
        assert_eq!(
            port, row.expected_port,
            "case {case:?}: expected <Port> {:?}, got {port:?} in {cluster}",
            row.expected_port
        );
    }
    let allocated = optional_text(cluster, "AllocatedStorage");
    assert_eq!(
        allocated.as_deref(),
        Some(row.expected_allocated_storage),
        "case {case:?}: expected <AllocatedStorage> {:?}, got {allocated:?} in {cluster}",
        row.expected_allocated_storage
    );
    let identifier = text(cluster, "DBClusterIdentifier");
    // What a cluster's two endpoint members carry: the address this emulator answers its data
    // plane at for a cluster whose data plane it SERVES, and the pair of names the cluster
    // mints from its own identifier under the endpoint domain of the control plane that owns
    // its engine for a cluster it serves no data plane for.
    if row.publishes_an_own_host {
        crate::db_clusters::assert_served_endpoint_pair(
            &case,
            &text(cluster, "Endpoint"),
            &text(cluster, "ReaderEndpoint"),
        );
    } else {
        crate::db_clusters::assert_endpoint_name_pair(
            &case,
            &identifier,
            REGION,
            row.expected_domain,
            &text(cluster, "Endpoint"),
            &text(cluster, "ReaderEndpoint"),
        );
    }
}

#[tokio::test]
async fn a_cluster_reports_the_defaults_its_engines_control_plane_publishes() {
    let router = emulator();

    for (index, row) in ENGINE_CASES.iter().enumerate() {
        let identifier = format!("emu-sibling-{index}");
        let created = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                create_cluster(&router, &identifier, row.engine, row.extra).await,
            ),
        );
        assert_engine_defaults(row.case, "CreateDBCluster", &created, row);

        // The same cluster read back out of the store once the transition the clock owed
        // it has come due: what the create answered is what it persisted.
        wait_for_settle();
        let read_back = describe_one(&router, row.case, &identifier).await;
        assert_engine_defaults(row.case, "DescribeDBClusters", &read_back, row);
    }
}

#[tokio::test]
async fn a_cluster_members_endpoint_sits_in_the_same_domain_as_its_cluster() {
    let router = emulator();

    // The rule holds for the member INSTANCES of a cluster too: an instance of an engine
    // another control plane owns is reached under that control plane's domain — unless this
    // emulator SERVES that engine's wire, and then the member's endpoint is the address of
    // the listener it opened onto the cluster's own storage rather than a name nothing
    // resolves. An Aurora member and an Amazon DocumentDB member are both of that second
    // kind; an Amazon Neptune member is not, because its plane is an API rather than a wire
    // of its own.
    let rows: [(&str, &str, Option<&str>); 3] = [
        ("a DocumentDB cluster member", "docdb", None),
        ("a Neptune cluster member", "neptune", Some(NEPTUNE_DOMAIN)),
        ("an Aurora MySQL cluster member", "aurora-mysql", None),
    ];

    for (index, (case, engine, domain)) in rows.iter().enumerate() {
        let cluster = format!("emu-member-cluster-{index}");
        expect_ok(case, create_cluster(&router, &cluster, engine, &[]).await);
        let instance = format!("emu-member-instance-{index}");
        let body = expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => instance,
                    "Engine" => engine,
                    "DBInstanceClass" => CLUSTER_MEMBER_CLASS,
                    "DBClusterIdentifier" => cluster,
                ],
            )
            .await,
        );
        let address = text(&body, "Address");
        match domain {
            Some(domain) => {
                let suffix = format!(".{REGION}.{domain}");
                assert!(
                    address.starts_with(&instance) && address.ends_with(&suffix),
                    "case {case:?}: expected an endpoint of the form {instance}.<label>\
                     {suffix}, got {address:?}"
                );
            }
            None => assert!(
                address
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback()),
                "case {case:?}: expected an address of the interface the member's own engine \
                 listener is bound on, got {address:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The storage types a sibling control plane's cluster publishes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sibling_control_planes_cluster_takes_its_own_storage_type_list() {
    let router = emulator();

    // `com.amazonaws.docdb#CreateDBClusterMessage$StorageType`, whose Neptune counterpart
    // states the same: "Valid values for storage type - standard | iopt1 … Default value
    // is standard … the storage type is returned in the response [for iopt1]. The storage
    // type isn't returned when you set it to standard."
    //
    // (case, engine, the storage type the request names, the one the cluster reports —
    // `Some("!refused")` for a value the engine's list does not carry).
    let rows: [(&str, &str, Option<&str>, Option<&str>); 8] = [
        ("a DocumentDB cluster that names no storage type", "docdb", None, None),
        (
            "a DocumentDB cluster on the default storage type, which is not reported",
            "docdb",
            Some("standard"),
            None,
        ),
        (
            "a DocumentDB cluster on I/O-optimized storage, which is reported",
            "docdb",
            Some("iopt1"),
            Some("iopt1"),
        ),
        ("a Neptune cluster that names no storage type", "neptune", None, None),
        (
            "a Neptune cluster on the default storage type",
            "neptune",
            Some("standard"),
            None,
        ),
        (
            "a Neptune cluster on I/O-optimized storage",
            "neptune",
            Some("iopt1"),
            Some("iopt1"),
        ),
        (
            "an Aurora storage type on a DocumentDB cluster is refused",
            "docdb",
            Some("aurora-iopt1"),
            Some("!refused"),
        ),
        (
            "a Multi-AZ DB cluster storage type on a Neptune cluster is refused",
            "neptune",
            Some("gp3"),
            Some("!refused"),
        ),
    ];

    for (index, (case, engine, named, expected)) in rows.iter().enumerate() {
        let identifier = format!("emu-sibling-storage-{index}");
        let extra: Vec<(&str, &str)> = named
            .map(|value| vec![("StorageType", value)])
            .unwrap_or_default();
        let response = create_cluster(&router, &identifier, engine, &extra).await;
        if *expected == Some("!refused") {
            let body = expect_error(case, response, 400, "StorageTypeNotSupported");
            let message = text(&body, "Message");
            assert!(
                message.contains(named.unwrap_or_default()),
                "case {case:?}: the refusal must name the storage type it refused, got \
                 {message:?}"
            );
            continue;
        }
        let created = the_cluster(case, &expect_ok(case, response));
        wait_for_settle();
        let read_back = describe_one(&router, case, &identifier).await;
        for (label, xml) in [
            ("CreateDBCluster", &created),
            ("DescribeDBClusters", &read_back),
        ] {
            let reported = optional_text(xml, "StorageType");
            assert_eq!(
                reported.as_deref(),
                *expected,
                "case {case:?} ({label}): expected <StorageType> {expected:?}, got \
                 {reported:?} in {xml}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The two refusals of the cluster lifecycle
// ---------------------------------------------------------------------------

/// What a delete of a DB cluster that names neither of its two final-snapshot members
/// answers: exactly one of them has to be given, so the refusal asks for one of the two.
const FINAL_SNAPSHOT_REFUSAL: &str = "Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot";

/// One row of the delete table: the case, the cluster's engine, and the parameters that
/// engine's cluster type requires beyond the two required members.
type DeleteRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

#[tokio::test]
async fn a_delete_that_names_neither_snapshot_member_asks_for_one_of_them() {
    let router = emulator();

    // The engines the rule spans: both sibling control planes' and this model's own, on
    // both cluster types.
    let engines: [DeleteRow; 4] = [
        ("a DocumentDB cluster", "docdb", &[]),
        ("a Neptune cluster", "neptune", &[]),
        ("an Aurora cluster", "aurora-mysql", &[]),
        ("a Multi-AZ DB cluster", "postgres", MULTI_AZ_SETTINGS),
    ];

    for (index, (case, engine, extra)) in engines.iter().enumerate() {
        let identifier = format!("emu-sibling-delete-{index}");
        expect_ok(case, create_cluster(&router, &identifier, engine, extra).await);
        wait_for_settle();

        // Neither member: refused, with the wording this Query API answers that with.
        let body = expect_error(
            case,
            call(
                &router,
                "DeleteDBCluster",
                &params!["DBClusterIdentifier" => identifier],
            )
            .await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, FINAL_SNAPSHOT_REFUSAL,
            "case {case:?}: expected {FINAL_SNAPSHOT_REFUSAL:?}, got {message:?}"
        );

        // Both members: refused too — the documentation forbids that combination as well,
        // so the refusal is about the combination and not about one missing member.
        let body = expect_error(
            case,
            call(
                &router,
                "DeleteDBCluster",
                &params![
                    "DBClusterIdentifier" => identifier,
                    "SkipFinalSnapshot" => "true",
                    "FinalDBSnapshotIdentifier" => "emu-sibling-final",
                ],
            )
            .await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("FinalDBSnapshotIdentifier") && message.contains("SkipFinalSnapshot"),
            "case {case:?}: the refusal must name both members, got {message:?}"
        );

        // Exactly one of them: served, and the delete is accepted — it answers the cluster
        // as it FOUND it, and the cluster is on its way out from the next read on. So the
        // two refusals above are about the combination, not about deleting the cluster.
        let body = expect_ok(
            case,
            call(
                &router,
                "DeleteDBCluster",
                &params![
                    "DBClusterIdentifier" => identifier,
                    "SkipFinalSnapshot" => "true",
                ],
            )
            .await,
        );
        let status = text(&body, "Status");
        assert_eq!(
            status, "available",
            "case {case:?}: the delete that names one member is served, got {body}"
        );
        let during = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusters",
                &params!["DBClusterIdentifier" => identifier],
            )
            .await,
        );
        let during_status = text(&during, "Status");
        assert_eq!(
            during_status, "deleting",
            "case {case:?}: the next read reports the cluster on its way out, got {during}"
        );
    }
}

/// One row of the password table: the case, the password, and the refusal it must be
/// answered with, or `None` for a password the service accepts.
type PasswordRow = (&'static str, String, Option<String>);

/// The passwords the rule spans, for an engine whose `MasterUserPassword` may be up to
/// `longest` characters: each published bound of the member, the values just outside them,
/// and each character it forbids ("Can include any printable ASCII character except \"/\",
/// \"'\", \"\"\", or the \"@\" symbol").
///
/// The LENGTH bound is per engine — `MasterUserPassword` publishes one line per engine
/// ("MySQL: Must contain from 8 to 41 characters", "PostgreSQL: Must contain from 8 to 128
/// characters", Amazon DocumentDB's "Must contain from 8 to 100 characters") — and every
/// one of them has the same lower bound of 8, so the table takes the upper bound of the
/// engine under test.
fn password_rows(longest: usize) -> Vec<PasswordRow> {
    let too_short = "The parameter MasterUserPassword is not a valid password because it is \
                     shorter than 8 characters."
        .to_string();
    let too_long = format!(
        "The parameter MasterUserPassword is not a valid password because it is longer \
         than {longest} characters."
    );
    let forbidden = "The parameter MasterUserPassword is not a valid password. Only \
                     printable ASCII characters besides '/', '@', '\"', ' ' may be used."
        .to_string();
    vec![
        ("one character under the lower bound", "p".repeat(7), Some(too_short.clone())),
        ("the lower bound itself", "p".repeat(8), None),
        ("a password well inside both bounds", "pw1-Secret".to_string(), None),
        ("the upper bound itself", "p".repeat(longest), None),
        ("one character over the upper bound", "p".repeat(longest + 1), Some(too_long)),
        ("a single character", "p".to_string(), Some(too_short)),
        (
            "the forbidden at sign",
            "pw1-Secret@".to_string(),
            Some(forbidden.clone()),
        ),
        (
            "the forbidden forward slash",
            "pw1/Secret".to_string(),
            Some(forbidden.clone()),
        ),
        (
            "a character outside printable ASCII",
            "pw1-Sécret".to_string(),
            Some(forbidden),
        ),
    ]
}

/// How long a `MasterUserPassword` may be on an Amazon DocumentDB cluster, which is the
/// engine the table below drives: "Must contain from 8 to 100 characters".
const DOCDB_PASSWORD_MAX: usize = 100;

#[tokio::test]
async fn a_master_user_password_outside_its_constraints_names_the_constraint_it_broke() {
    let router = emulator();

    // The cluster the modifications below address: a DocumentDB cluster, because the
    // password rule is the same whichever control plane the caller compiled against.
    let identifier = "emu-sibling-password";
    expect_ok(
        "the cluster the modifications address",
        create_cluster(
            &router,
            identifier,
            "docdb",
            &[("MasterUsername", "emuadmin"), ("MasterUserPassword", "pw1-Secret")],
        )
        .await,
    );
    wait_for_settle();

    for (index, (case, password, refusal)) in
        password_rows(DOCDB_PASSWORD_MAX).into_iter().enumerate()
    {
        // The same value on the MODIFY and on a fresh CREATE: one rule, both paths.
        let created = format!("emu-sibling-password-{index}");
        let calls: [(&str, Params); 2] = [
            (
                "ModifyDBCluster",
                params![
                    "DBClusterIdentifier" => identifier,
                    "MasterUserPassword" => password,
                ]
                .to_vec(),
            ),
            (
                "CreateDBCluster",
                params![
                    "DBClusterIdentifier" => created,
                    "Engine" => "docdb",
                    "MasterUsername" => "emuadmin",
                    "MasterUserPassword" => password,
                ]
                .to_vec(),
            ),
        ];
        for (action, parameters) in calls {
            let case = format!("{case} ({action})");
            let response = call(&router, action, &parameters).await;
            match &refusal {
                Some(expected) => {
                    let body = expect_error(&case, response, 400, "InvalidParameterValue");
                    let message = text(&body, "Message");
                    assert_eq!(
                        message, *expected,
                        "case {case:?}: expected {expected:?}, got {message:?}"
                    );
                }
                None => {
                    let body = expect_ok(&case, response);
                    assert!(
                        !body.contains(password.as_str()),
                        "case {case:?}: the master user password must never be reported, got \
                         {body}"
                    );
                    // An accepted modification leaves the cluster `modifying` until the
                    // clock settles it, and a cluster in that state takes no further
                    // modification — so the next row waits the span out rather than
                    // reading a state fault as a refusal of its password.
                    wait_for_settle();
                }
            }
        }
    }
}
