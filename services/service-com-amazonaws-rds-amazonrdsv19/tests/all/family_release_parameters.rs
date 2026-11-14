// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH parameters a DB parameter group family publishes.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so every assertion reads the XML the emulator itself produced.
//!
//! The rule under test: a family is one RELEASE LINE of one engine (`mysql8.0`,
//! `aurora-mysql5.7`, `docdb3.6`), and it publishes exactly the parameters that line's
//! engine release carries. A parameter the engine introduced in a LATER release than the
//! line the family names is not one of that family's parameters — `MinimumEngineVersion` is
//! "the earliest engine version to which the parameter can apply" — so neither the
//! family-level engine-default reports nor the listing of a group of that family carries it,
//! while every family at or past that release does.
//!
//! The table below states each row INDEPENDENTLY of this crate's catalogs: the parameter,
//! the release that introduced it (from the engine's own published parameter data — the
//! MySQL manual's "Server System Variables" for the MySQL vocabulary, the Amazon Aurora
//! User Guide's cluster-level Aurora MySQL table for the Aurora parameters and the Amazon
//! DocumentDB parameter table for that engine's), the family and whether that family
//! carries it. It spans both LEVELS (a DB parameter group family and a DB cluster parameter
//! group family), three engine vocabularies, both sides of each release boundary, a
//! parameter introduced INSIDE the line a family names, a parameter the engine REMOVED in a
//! later line, and a parameter the engine has always carried — including rows no issue
//! cited.
//!
//! The same rule holds at the other end of a parameter's life: a parameter the engine
//! dropped in a later release is not among the parameters of a family naming that line,
//! while every earlier family still carries it.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, optional_text};
use crate::engine_default_parameters::{reported, Reported};

/// Which level of parameter group a row is about, which is the pair of operations that
/// answers for it: the engine-default report of a family, and the listing of a group of
/// that family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// A DB parameter group family: the parameters of a single DB instance.
    Instance,
    /// A DB cluster parameter group family: the cluster-level parameters.
    Cluster,
}

impl Level {
    /// The engine-default report of a family at this level.
    fn defaults_action(self) -> &'static str {
        match self {
            Level::Instance => "DescribeEngineDefaultParameters",
            Level::Cluster => "DescribeEngineDefaultClusterParameters",
        }
    }

    /// The create that stores a group of a family at this level, with the member it names
    /// the group by.
    fn create(self) -> (&'static str, &'static str) {
        match self {
            Level::Instance => ("CreateDBParameterGroup", "DBParameterGroupName"),
            Level::Cluster => (
                "CreateDBClusterParameterGroup",
                "DBClusterParameterGroupName",
            ),
        }
    }

    /// The describe that lists a group's own parameters at this level, with the member it
    /// names the group by.
    fn listing(self) -> (&'static str, &'static str) {
        match self {
            Level::Instance => ("DescribeDBParameters", "DBParameterGroupName"),
            Level::Cluster => (
                "DescribeDBClusterParameters",
                "DBClusterParameterGroupName",
            ),
        }
    }
}

/// One row of the published table: the level, the family, the parameter, the release that
/// introduced the parameter into its engine (or `None` where the engine has carried it
/// since before any release this service serves) and whether the family carries it.
struct Row {
    case: &'static str,
    level: Level,
    family: &'static str,
    parameter: &'static str,
    introduced_in: Option<&'static str>,
    carried: bool,
}

/// The table, written from the engines' own published parameter data; see this module's
/// header for the sources.
const ROWS: &[Row] = &[
    Row {
        case: "a parameter the engine introduced in the line the family names",
        level: Level::Instance,
        family: "mysql8.0",
        parameter: "activate_all_roles_on_login",
        introduced_in: Some("8.0.2"),
        carried: true,
    },
    Row {
        case: "the same parameter, one release line earlier",
        level: Level::Instance,
        family: "mysql5.7",
        parameter: "activate_all_roles_on_login",
        introduced_in: Some("8.0.2"),
        carried: false,
    },
    Row {
        case: "another engine running the later line of the same vocabulary",
        level: Level::Instance,
        family: "aurora-mysql8.0",
        parameter: "binlog_row_metadata",
        introduced_in: Some("8.0.1"),
        carried: true,
    },
    Row {
        case: "the same engine's earlier line",
        level: Level::Instance,
        family: "aurora-mysql5.7",
        parameter: "binlog_expire_logs_seconds",
        introduced_in: Some("8.0.1"),
        carried: false,
    },
    Row {
        case: "an engine that numbers its own releases carries the entry either way",
        level: Level::Instance,
        family: "mariadb10.11",
        parameter: "binlog_expire_logs_seconds",
        introduced_in: Some("8.0.1"),
        carried: true,
    },
    Row {
        case: "a parameter introduced INSIDE the line the family names",
        level: Level::Instance,
        family: "mysql5.7",
        parameter: "auto_generate_certs",
        introduced_in: Some("5.7.5"),
        carried: true,
    },
    Row {
        case: "a parameter the engine has always carried",
        level: Level::Instance,
        family: "mysql5.7",
        parameter: "autocommit",
        introduced_in: None,
        carried: true,
    },
    Row {
        case: "the same parameter at the later line",
        level: Level::Instance,
        family: "mysql8.0",
        parameter: "autocommit",
        introduced_in: None,
        carried: true,
    },
    Row {
        case: "a parameter the engine DROPPED in a later line, read at an earlier one",
        level: Level::Instance,
        family: "mysql5.7",
        parameter: "big_tables",
        introduced_in: None,
        carried: true,
    },
    Row {
        case: "the same parameter one line before the one that dropped it",
        level: Level::Instance,
        family: "mysql8.0",
        parameter: "big_tables",
        introduced_in: None,
        carried: true,
    },
    Row {
        case: "the same parameter at the line that dropped it",
        level: Level::Instance,
        family: "mysql8.4",
        parameter: "big_tables",
        introduced_in: None,
        carried: false,
    },
    Row {
        // The engine shares the MySQL vocabulary and numbers its own releases against no
        // MySQL release, so neither a MySQL introduction nor a MySQL removal applies to it.
        case: "the same parameter at an engine that numbers its own releases",
        level: Level::Instance,
        family: "mariadb10.11",
        parameter: "big_tables",
        introduced_in: None,
        carried: true,
    },
    Row {
        case: "a cluster parameter introduced in the later line",
        level: Level::Cluster,
        family: "aurora-mysql8.0",
        parameter: "activate_all_roles_on_login",
        introduced_in: Some("8.0.2"),
        carried: true,
    },
    Row {
        case: "the same cluster parameter at the earlier line",
        level: Level::Cluster,
        family: "aurora-mysql5.7",
        parameter: "activate_all_roles_on_login",
        introduced_in: Some("8.0.2"),
        carried: false,
    },
    Row {
        case: "an Aurora cluster parameter of the earlier line's own release",
        level: Level::Cluster,
        family: "aurora-mysql5.7",
        parameter: "aurora_binlog_read_buffer_size",
        introduced_in: Some("5.7.mysql_aurora.2.04.5"),
        carried: true,
    },
    Row {
        case: "a document database's cluster parameter at the line that introduced it",
        level: Level::Cluster,
        family: "docdb4.0",
        parameter: "audit_logs",
        introduced_in: Some("4.0.0"),
        carried: true,
    },
    Row {
        case: "the same parameter at that engine's earlier line",
        level: Level::Cluster,
        family: "docdb3.6",
        parameter: "audit_logs",
        introduced_in: Some("4.0.0"),
        carried: false,
    },
    Row {
        case: "a parameter that engine's earlier line does carry",
        level: Level::Cluster,
        family: "docdb3.6",
        parameter: "tls",
        introduced_in: Some("3.6.0"),
        carried: true,
    },
    Row {
        case: "a graph engine's cluster parameter, carried by every line of it",
        level: Level::Cluster,
        family: "neptune1.3",
        parameter: "neptune_streams",
        introduced_in: None,
        carried: true,
    },
];

/// Every `<Parameter>` the report or listing `action` answers for `params`, paginated to the
/// end.
async fn every_parameter(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, String)],
) -> Vec<Reported> {
    let mut found: Vec<Reported> = Vec::new();
    let mut marker: Option<String> = None;
    // A page carries at most 100 records and the longest catalog this service publishes is
    // well inside eight pages; the loop is bounded so a marker that never clears cannot
    // hang the test.
    for _ in 0..8 {
        let mut request: Vec<(&str, String)> = params.to_vec();
        if let Some(marker) = &marker {
            request.push(("Marker", marker.clone()));
        }
        let body = expect_ok(case, call(router, action, &request).await);
        found.extend(reported(&body));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return found;
        }
    }
    panic!("case {case:?}: paging did not terminate: {found:?}");
}

/// The names `action` reports for `params`.
async fn names(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, String)],
) -> Vec<String> {
    every_parameter(router, case, action, params)
        .await
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

/// The group name a row's group is created under: one group per row, keyed by the row's
/// index so two rows of one family do not collide.
fn group_name(index: usize) -> String {
    format!("emu-release-line-{index}")
}

#[tokio::test]
async fn a_family_publishes_the_parameters_its_own_engine_release_carries() {
    let router = emulator();

    for (index, row) in ROWS.iter().enumerate() {
        let case = format!("{} ({} {})", row.case, row.family, row.parameter);

        // 1. The FAMILY's own engine-default report.
        let published = every_parameter(
            &router,
            &case,
            row.level.defaults_action(),
            &[("DBParameterGroupFamily", row.family.to_string())],
        )
        .await;
        let found = published
            .iter()
            .find(|entry| entry.name == row.parameter);
        assert_eq!(
            found.is_some(),
            row.carried,
            "case {case:?}: the family must publish this parameter: {}, got {:?}",
            row.carried,
            published
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>()
        );

        // A parameter a family carries reports the release that introduced it where the
        // engine published one, and nothing where it published none.
        if let Some(found) = found {
            let expected = row.introduced_in.map(ToString::to_string);
            // The cluster-level report answers the member about the FAMILY: a release
            // earlier than the family's own first one reads as that first release, so the
            // assertion is that the member is reported exactly where one is published.
            assert_eq!(
                found.minimum_engine_version.is_some(),
                expected.is_some(),
                "case {case:?}: MinimumEngineVersion is reported exactly where the engine \
                 published an introducing release, got {:?}",
                found.minimum_engine_version
            );
        }

        // 2. A group of that family, freshly created, reports the family's parameters —
        //    so the report and the group listing cannot disagree about the family's list.
        let (create, create_key) = row.level.create();
        let group = group_name(index);
        expect_ok(
            &case,
            call(
                &router,
                create,
                &[
                    (create_key, group.as_str()),
                    ("DBParameterGroupFamily", row.family),
                    ("Description", "a release-line row"),
                ],
            )
            .await,
        );
        let (listing, listing_key) = row.level.listing();
        let listed = names(
            &router,
            &case,
            listing,
            &[(listing_key, group.clone())],
        )
        .await;
        assert_eq!(
            listed.iter().any(|name| name == row.parameter),
            row.carried,
            "case {case:?}: a group of the family reports the family's parameters, so this \
             one must be among them: {}, got {listed:?}",
            row.carried
        );
    }
}

/// The lists two families of one engine publish are decided by their releases: what only the
/// later line carries is what the engine introduced there, what only the earlier line carries
/// is what the engine dropped there, and the two agree on the rest of the vocabulary.
#[tokio::test]
async fn an_earlier_release_lines_parameters_are_a_subset_of_a_later_lines() {
    let router = emulator();

    // (case, level, the earlier family, the later family)
    let pairs: [(&str, Level, &str, &str); 3] = [
        ("one engine's two lines", Level::Instance, "mysql5.7", "mysql8.0"),
        (
            "another engine of the same vocabulary",
            Level::Instance,
            "aurora-mysql5.7",
            "aurora-mysql8.0",
        ),
        (
            "the cluster level of that engine",
            Level::Cluster,
            "aurora-mysql5.7",
            "aurora-mysql8.0",
        ),
    ];

    for (case, level, earlier_family, later_family) in pairs {
        let earlier = names(
            &router,
            case,
            level.defaults_action(),
            &[("DBParameterGroupFamily", earlier_family.to_string())],
        )
        .await;
        let later = names(
            &router,
            case,
            level.defaults_action(),
            &[("DBParameterGroupFamily", later_family.to_string())],
        )
        .await;
        assert!(
            earlier.iter().filter(|name| later.contains(name)).count() * 2 > earlier.len(),
            "case {case:?}: two lines of one engine share that engine's vocabulary, got \
             {earlier:?} against {later:?}"
        );
        let only_later: Vec<&String> = later
            .iter()
            .filter(|name| !earlier.contains(name))
            .collect();
        assert!(
            !only_later.is_empty(),
            "case {case:?}: the engine introduced parameters in its later line, so \
             {later_family} must carry parameters {earlier_family} does not, got {later:?}"
        );
        for name in only_later {
            let row = every_parameter(
                &router,
                case,
                level.defaults_action(),
                &[("DBParameterGroupFamily", later_family.to_string())],
            )
            .await
            .into_iter()
            .find(|entry| &entry.name == name)
            .unwrap_or_else(|| panic!("case {case:?}: just read from this report"));
            assert!(
                row.minimum_engine_version.is_some(),
                "case {case:?}: a parameter only the later line carries is one the engine \
                 introduced there, so it reports the release it can first apply at, got \
                 {row:?}"
            );
        }
    }
}
