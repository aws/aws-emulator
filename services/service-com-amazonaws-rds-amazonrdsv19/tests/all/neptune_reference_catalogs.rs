// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the reference catalogs this one front door publishes on behalf of
//! the graph engine's control plane, and for how the two reports that RESOLVE a published
//! name word their refusals.
//!
//! This front door serves three control planes (`src/service.rs`): Amazon RDS, Amazon
//! DocumentDB and Amazon Neptune publish one wire identity, so a Neptune client signs `rds`
//! and its catalog reads land here. The rules these tables pin:
//!
//! 1. a `DBParameterGroupFamily` is published reference data — the families the
//!    engine-version catalogue reports plus the ones this model's own member value list
//!    publishes — so a value outside that list is refused with the service's one sentence,
//!    whatever shape the value has;
//! 2. each parameter LEVEL of a family publishes its own catalog, with that engine's own
//!    catalog text, in its own order;
//! 3. a parameter a GROUP carries is described by the record its family publishes for it,
//!    at whichever level that family publishes it — a parameter group's family is not
//!    scoped to the engine of the control plane the group was created through;
//! 4. the engine-version report publishes the graph engine's default release, its
//!    exportable log types and its serverless capacity range;
//! 5. the two reports that resolve an `Engine` word an unrecognised value the way the real
//!    service words it for THAT operation.
//!
//! Every assertion spans a table rather than one request: families of several engines and
//! several shapes of non-family, both parameter levels, groups of four families, every
//! published major-version family of the graph engine, and five values that name no engine.
//!
//! The harness (`emulator`, `call`, `expect_ok`, `expect_error` and the element readers) is
//! the [`crate::db_instances`] suite's, and the group create/modify pair is
//! [`crate::cluster_parameter_groups`]'s, reused rather than copied.

use service_com_amazonaws_rds_amazonrdsv19::state::PARAMETER_SOURCE_ENGINE_DEFAULT;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_parameter_groups::{create_group, modify};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
};

/// The Query API code this front door answers a value its member does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The graph engine of the Amazon Neptune control plane this door serves.
const GRAPH_ENGINE: &str = "neptune";

/// The report of the parameters a DB parameter group FAMILY publishes.
const INSTANCE_DEFAULTS: &str = "DescribeEngineDefaultParameters";

/// The report of the parameters a DB CLUSTER parameter group family publishes.
const CLUSTER_DEFAULTS: &str = "DescribeEngineDefaultClusterParameters";

// ---------------------------------------------------------------------------
// Rule 1: what is a published family
// ---------------------------------------------------------------------------

/// How the service refuses a `DBParameterGroupFamily` it does not publish: fixed text
/// around the value the request carried, which is what both engine-default reports answer.
fn not_a_family(family: &str) -> String {
    format!("ParameterGroupFamily {family} is not a valid parameter group family")
}

/// One family row: (case, the value, whether the instance-level report serves it, whether
/// the cluster-level report serves it, whether the instance-level report publishes any
/// parameter for it).
///
/// The rows differ along what the rule generalises over: which engine the family names,
/// whether that engine has a cluster form, where the family is published (the
/// engine-version catalogue, this model's member value list, or nowhere), the SHAPE of the
/// value (engine plus version, engine alone, version alone, a hyphenated engine, a name no
/// engine carries) and its case. The last column separates "served" from "publishes a
/// catalog": a family of an engine this emulator holds no catalog for is served and reports
/// an empty list, which is a different answer from a refusal.
const FAMILIES: &[(&str, &str, bool, bool, bool)] = &[
    ("a published family of the graph engine", "neptune1.4", true, true, true),
    (
        "the graph engine's first family, whose name carries no dot",
        "neptune1",
        true,
        true,
        true,
    ),
    (
        "a published family of a relational cluster engine",
        "aurora-postgresql16",
        true,
        true,
        true,
    ),
    ("a published family of an instance-only engine", "mariadb10.11", true, false, true),
    // The member's own value list publishes families this emulator's engine-version
    // catalogue samples no release of; `postgres` has a cluster form, so both levels of
    // that family are served.
    ("a family only the member's value list publishes", "postgres12", true, true, true),
    // A family of an engine this emulator publishes no parameter catalog for: served, and
    // honestly empty rather than filled with invented parameters.
    (
        "a versionless family the member's value list publishes",
        "db2-ae",
        true,
        false,
        false,
    ),
    ("a published family written in another case", "NEPTUNE1.4", true, true, true),
    // Everything below names no family the service publishes.
    ("an engine followed by a release it never had", "neptune9", false, false, false),
    ("the graph engine's name with no version at all", "neptune", false, false, false),
    ("another engine's name with no version", "mysql", false, false, false),
    ("a version with no engine", "8.0", false, false, false),
    ("a name no served engine carries", "not-an-engine1.0", false, false, false),
    ("another service's parameter group family", "memcached1.6", false, false, false),
    ("the empty string", "", false, false, false),
];

#[tokio::test]
async fn only_a_published_parameter_group_family_is_served() {
    let router = emulator();

    for (case, family, instance_serves, cluster_serves, instance_publishes) in FAMILIES {
        for (action, serves) in [
            (INSTANCE_DEFAULTS, *instance_serves),
            (CLUSTER_DEFAULTS, *cluster_serves),
        ] {
            let case = format!("{action} with DBParameterGroupFamily {family:?} ({case})");
            let response = call(&router, action, &[("DBParameterGroupFamily", *family)]).await;
            if serves {
                let body = expect_ok(&case, response);
                assert_eq!(
                    text(&body, "DBParameterGroupFamily"),
                    *family,
                    "case {case:?}: the report echoes the family the request named, got {body}"
                );
                if action == INSTANCE_DEFAULTS {
                    assert_eq!(
                        !read_parameters(&body).is_empty(),
                        *instance_publishes,
                        "case {case:?}: expected published parameters == \
                         {instance_publishes}, got {body}"
                    );
                }
                continue;
            }
            let body = expect_error(&case, response, 400, INVALID_PARAMETER_VALUE);
            let message = text(&body, "Message");
            let expected = not_a_family(family);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the refusal {expected:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: each level publishes its own catalog, in its own order
// ---------------------------------------------------------------------------

/// One published parameter: (name, `ApplyType`, `DataType`, `AllowedValues`, the engine's
/// default value, the catalog description).
type Published = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// The cluster parameters the graph engine publishes, in the order its catalog carries.
///
/// Source: the "DB cluster parameter group parameters" section of the Amazon Neptune User
/// Guide, which is the catalog `src/ops/describe_engine_default_cluster_parameters.rs`
/// publishes. `neptune_streams_expiry_days` is an entry the issue cited nothing about.
const GRAPH_CLUSTER_PARAMETERS: &[Published] = &[
    (
        "neptune_enable_audit_log",
        "static",
        "boolean",
        "0,1",
        "0",
        "Toggle audit logging for Neptune",
    ),
    (
        "neptune_enable_slow_query_log",
        "dynamic",
        "string",
        "disabled,info,debug",
        "disabled",
        "Enable Slow query logs for Neptune",
    ),
    (
        "neptune_streams",
        "static",
        "boolean",
        "0,1",
        "0",
        "Enable or disable Neptune Streams feature",
    ),
    (
        "neptune_streams_expiry_days",
        "dynamic",
        "integer",
        "1-90",
        "7",
        "The number of days the cluster keeps the change-log records of its graph.",
    ),
];

/// The parameters the graph engine publishes at the DB-parameter level — its query timeout,
/// whose description carries the terminating period the engine's catalog writes.
const GRAPH_INSTANCE_PARAMETERS: &[Published] = &[(
    "neptune_query_timeout",
    "static",
    "integer",
    "10-2147483647",
    "120000",
    "Graph query timeout (ms).",
)];

/// The members of one reported parameter, in the order [`reported`] reads them:
/// `(ParameterName, ApplyType, DataType, AllowedValues, ParameterValue, Description,
/// Source)`.
type ReportedParameter = (String, String, String, String, String, String, String);

/// The parameters one report publishes, in the order the report serves them, or `None`
/// when that report does not serve this family at all (a cluster-level report of a family
/// whose engine has no cluster form).
async fn reported_if_served(
    router: &MultiServiceRouter,
    action: &str,
    family: &str,
) -> Option<Vec<ReportedParameter>> {
    let (status, body) = call(router, action, &[("DBParameterGroupFamily", family)]).await;
    (status == 200).then(|| read_parameters(&body))
}

/// The parameters one report publishes, in the order the report serves them.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    family: &str,
) -> Vec<ReportedParameter> {
    let body = expect_ok(
        case,
        call(router, action, &[("DBParameterGroupFamily", family)]).await,
    );
    read_parameters(&body)
}

/// Every `<Parameter>` of a report body, in the order it carries them. A member the
/// catalog publishes nothing for reads as the empty string, which is how the tables below
/// write "this parameter reports no such member".
fn read_parameters(body: &str) -> Vec<ReportedParameter> {
    subtrees(body, "Parameter")
        .into_iter()
        .map(|parameter| {
            let member = |name: &str| optional_text(&parameter, name).unwrap_or_default();
            (
                member("ParameterName"),
                member("ApplyType"),
                member("DataType"),
                member("AllowedValues"),
                member("ParameterValue"),
                member("Description"),
                member("Source"),
            )
        })
        .collect()
}

#[tokio::test]
async fn each_parameter_level_publishes_its_own_catalog() {
    let router = emulator();

    // Every published family of the graph engine reports the same catalog at each level:
    // the parameters belong to the ENGINE, and which releases a family has is the
    // engine-version report's question.
    for family in ["neptune1", "neptune1.2", "neptune1.3", "neptune1.4"] {
        for (action, published) in [
            (CLUSTER_DEFAULTS, GRAPH_CLUSTER_PARAMETERS),
            (INSTANCE_DEFAULTS, GRAPH_INSTANCE_PARAMETERS),
        ] {
            let case = format!("{action} for {family}");
            let answered = reported(&router, &case, action, family).await;
            let expected: Vec<ReportedParameter> = published
                .iter()
                .map(|(name, apply, data, allowed, value, description)| {
                    (
                        (*name).to_string(),
                        (*apply).to_string(),
                        (*data).to_string(),
                        (*allowed).to_string(),
                        (*value).to_string(),
                        (*description).to_string(),
                        PARAMETER_SOURCE_ENGINE_DEFAULT.to_string(),
                    )
                })
                .collect();
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the engine's published catalog in its own order, \
                 got {answered:?}"
            );
        }
    }

    // The two levels publish DIFFERENT parameters: a parameter of one is not reported by
    // the other, which is what makes the level a caller asks about meaningful.
    for (cluster, _, _, _, _, _) in GRAPH_CLUSTER_PARAMETERS {
        assert!(
            !GRAPH_INSTANCE_PARAMETERS
                .iter()
                .any(|(instance, ..)| instance == cluster),
            "case {cluster:?}: the two levels of one family publish disjoint parameters"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a group's parameters are described by its family's catalog
// ---------------------------------------------------------------------------

/// One group row: (case, the family the group is created with, the parameter set on it, the
/// value, the `ApplyType` that family publishes, the `DataType` it publishes, whether the
/// family publishes the parameter at all).
///
/// The rows differ along what the rule generalises over: whether the family's engine has a
/// cluster form at all, which LEVEL of that family publishes the parameter, which engine the
/// family names — and one parameter no catalog of the family publishes, which carries no
/// metadata rather than invented metadata.
const GROUPS: &[(&str, &str, &str, &str, &str, &str, bool)] = &[
    (
        "an instance-only engine's family, carrying that family's DB parameter",
        "mariadb10.11",
        "character_set_server",
        "utf8mb4",
        "dynamic",
        "string",
        true,
    ),
    (
        "a relational cluster engine's family, carrying a cluster parameter",
        "aurora-mysql8.0",
        "binlog_format",
        "ROW",
        "static",
        "string",
        true,
    ),
    (
        "the graph engine's family, carrying one of its cluster parameters",
        "neptune1.4",
        "neptune_enable_audit_log",
        "1",
        "static",
        "boolean",
        true,
    ),
    (
        "the graph engine's family, carrying the parameter its other level publishes",
        "neptune1.4",
        "neptune_query_timeout",
        "60000",
        "static",
        "integer",
        true,
    ),
    (
        "a family that publishes the parameter at neither level",
        "mariadb10.11",
        // A system variable the engine HAS that neither catalog of its family publishes a
        // record for: carried, and reported without the published columns.
        "innodb_old_blocks_time",
        "1",
        "",
        "",
        false,
    ),
];

#[tokio::test]
async fn a_group_describes_its_parameters_from_its_familys_catalog() {
    let router = emulator();

    for (index, (case, family, parameter, value, apply_type, data_type, published)) in
        GROUPS.iter().enumerate()
    {
        let group = create_group(
            &router,
            case,
            &format!("emu-family-catalog-{index}"),
            family,
        )
        .await;
        expect_ok(case, modify(&router, &group, parameter, value, None).await);

        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Source", "user"),
                ],
            )
            .await,
        );
        let answered = subtrees(&body, "Parameter");
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: expected the one parameter the group carries, got {body}"
        );
        let answered = &answered[0];
        assert_eq!(
            (
                text(answered, "ParameterName"),
                text(answered, "ParameterValue")
            ),
            ((*parameter).to_string(), (*value).to_string()),
            "case {case:?}: expected the stored name and value, got {answered}"
        );
        if !*published {
            for member in ["ApplyType", "DataType", "AllowedValues", "Description", "IsModifiable"] {
                let found = optional_text(answered, member);
                assert!(
                    found.is_none(),
                    "case {case:?}: a parameter no catalog of {family} publishes must carry no \
                     {member}, got {found:?}"
                );
            }
            continue;
        }
        assert_eq!(
            (
                text(answered, "ApplyType"),
                text(answered, "DataType"),
                text(answered, "IsModifiable")
            ),
            (
                (*apply_type).to_string(),
                (*data_type).to_string(),
                "true".to_string()
            ),
            "case {case:?}: expected the metadata {family} publishes for {parameter}, got \
             {answered}"
        );
        for member in ["AllowedValues", "Description"] {
            let found = text(answered, member);
            assert!(
                !found.is_empty(),
                "case {case:?}: expected {family} to publish a {member} for {parameter}, got \
                 {answered}"
            );
        }

        // The record is the FAMILY's own: the engine-default report of the level that
        // publishes the parameter reports the same apply type, data type, allowed values
        // and description for it as the group does, so a client that read the family's
        // catalog before setting the parameter and one that reads the group afterwards see
        // one record. A family whose engine publishes the name at BOTH levels is described
        // by its CLUSTER record here, because that is the level this group belongs to.
        let mut published_by_a_report = false;
        for action in [CLUSTER_DEFAULTS, INSTANCE_DEFAULTS] {
            let case = format!("{case} ({action})");
            let Some(found) = reported_if_served(&router, action, family)
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|entry| entry.0 == *parameter)
            else {
                continue;
            };
            published_by_a_report = true;
            assert_eq!(
                (found.1, found.2, found.3, found.5),
                (
                    text(answered, "ApplyType"),
                    text(answered, "DataType"),
                    text(answered, "AllowedValues"),
                    text(answered, "Description")
                ),
                "case {case:?}: the group and the engine-default report must publish one \
                 record for {parameter}"
            );
            break;
        }
        assert!(
            published_by_a_report,
            "case {case:?}: a parameter the group describes is one a level of {family} \
             publishes"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the engine-version report publishes the graph engine's default release
// ---------------------------------------------------------------------------

/// The log types the graph engine can export to CloudWatch Logs — one per log the engine
/// writes, each switched on by a cluster parameter its catalog publishes.
const GRAPH_LOG_TYPES: [&str; 2] = ["audit", "slowquery"];

/// The serverless capacity range the graph engine's releases publish, as
/// `ServerlessV2FeaturesSupport` reports it: a floor that does not pause, and the ceiling.
const GRAPH_SERVERLESS_CAPACITY: (&str, &str) = ("0.5", "128.0");

/// One `DefaultOnly` request: (case, the request's own parameters, the major version it
/// narrows to when it narrows to one).
type DefaultOnlyCase = (String, Vec<(&'static str, String)>, Option<&'static str>);

#[tokio::test]
async fn the_graph_engine_publishes_a_default_release() {
    let router = emulator();

    // `DefaultOnly` naming only the engine reports the ENGINE's default release; naming a
    // major-version family as well reports that family's own default. Every published
    // family is a row, so the rule is not read off one of them.
    let mut cases: Vec<DefaultOnlyCase> = vec![(
        "the engine's own default".to_string(),
        vec![
            ("Engine", GRAPH_ENGINE.to_string()),
            ("DefaultOnly", "true".to_string()),
        ],
        None,
    )];
    for major in ["1.1", "1.2", "1.3", "1.4"] {
        cases.push((
            format!("the default of the {major} family"),
            vec![
                ("Engine", GRAPH_ENGINE.to_string()),
                ("EngineVersion", major.to_string()),
                ("DefaultOnly", "true".to_string()),
            ],
            Some(major),
        ));
    }

    for (case, params, major) in cases {
        let body = expect_ok(
            &case,
            call(&router, "DescribeDBEngineVersions", &params).await,
        );
        let versions = subtrees(&body, "DBEngineVersion");
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: expected exactly one default release, got {body}"
        );
        let version = &versions[0];
        let engine_version = text(version, "EngineVersion");
        let major_engine_version = text(version, "MajorEngineVersion");
        assert_eq!(
            text(version, "Engine"),
            GRAPH_ENGINE,
            "case {case:?}: expected the engine the request named, got {version}"
        );
        if let Some(major) = major {
            assert_eq!(
                major_engine_version, major,
                "case {case:?}: expected the default of the {major} family, got {version}"
            );
        }
        // The release's own numbering: its major version prefixes it, and the rest of the
        // string is the build within that family.
        assert!(
            engine_version
                .strip_prefix(&major_engine_version)
                .is_some_and(|rest| rest.starts_with('.')),
            "case {case:?}: expected a release of the {major_engine_version} family, got \
             {version}"
        );
        assert_eq!(
            text(version, "Status"),
            "available",
            "case {case:?}: a default release is one a caller can order, got {version}"
        );
        assert_eq!(
            texts(version, "member")
                .into_iter()
                .filter(|value| GRAPH_LOG_TYPES.contains(&value.as_str()))
                .collect::<Vec<String>>(),
            GRAPH_LOG_TYPES.to_vec(),
            "case {case:?}: expected the engine's exportable log types, got {version}"
        );
        assert_eq!(
            text(version, "SupportsLogExportsToCloudwatchLogs"),
            "true",
            "case {case:?}: an engine with exportable logs exports them, got {version}"
        );
        // The parameter group family of a release is the engine name plus its major
        // version from the second family on, and `neptune1` for the first.
        let family = text(version, "DBParameterGroupFamily");
        let expected_family = if major_engine_version == "1.1" {
            "neptune1".to_string()
        } else {
            format!("{GRAPH_ENGINE}{major_engine_version}")
        };
        assert_eq!(
            family, expected_family,
            "case {case:?}: expected the release's own family, got {version}"
        );
        // A release that offers the engine's serverless form publishes its capacity range,
        // and one from before it publishes none rather than a range its clusters cannot be
        // configured in.
        let capacity = subtrees(version, "ServerlessV2FeaturesSupport");
        if major_engine_version == "1.1" {
            assert!(
                capacity.is_empty(),
                "case {case:?}: a release from before the serverless form publishes no \
                 capacity range, got {version}"
            );
        } else {
            assert_eq!(capacity.len(), 1, "case {case:?}: got {version}");
            assert_eq!(
                (
                    text(&capacity[0], "MinCapacity"),
                    text(&capacity[0], "MaxCapacity")
                ),
                (
                    GRAPH_SERVERLESS_CAPACITY.0.to_string(),
                    GRAPH_SERVERLESS_CAPACITY.1.to_string()
                ),
                "case {case:?}: expected the published capacity range, got {version}"
            );
        }
        // The family the release belongs to is one the parameter reports serve, so the two
        // catalogs agree about which families exist.
        expect_ok(
            &format!("{case}: the release's family is served"),
            call(&router, CLUSTER_DEFAULTS, &[("DBParameterGroupFamily", family.as_str())]).await,
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: each report words its own invalid-engine refusal
// ---------------------------------------------------------------------------

/// The values spanning the ways an `Engine` can name no engine this door publishes: a graph
/// query language, another one, the engine's PARAMETER GROUP family, a name with the
/// engine's name inside it, and the empty string.
const NOT_AN_ENGINE: [&str; 5] = ["gremlin", "sparql", "neptune1.4", "neptune-graph", ""];

#[tokio::test]
async fn each_report_words_its_invalid_engine_refusal_its_own_way() {
    let router = emulator();

    for value in NOT_AN_ENGINE {
        // The orderable-option report answers ONE fixed sentence, which carries nothing
        // from the request...
        let case = format!("DescribeOrderableDBInstanceOptions with Engine {value:?}");
        let body = expect_error(
            &case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[("Engine", value)],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, "Invalid DB engine",
            "case {case:?}: expected the report's own fixed sentence, got {message:?}"
        );
        assert!(
            value.is_empty() || !message.contains(value),
            "case {case:?}: the sentence names no value, got {message:?}"
        );

        // ...while the engine-version report names the value it did not recognise.
        let case = format!("DescribeDBEngineVersions with Engine {value:?}");
        let body = expect_error(
            &case,
            call(&router, "DescribeDBEngineVersions", &[("Engine", value)]).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        let expected = format!("Unrecognized engine name: {value}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The positive control: the graph engine itself is served by both reports.
    for action in [
        "DescribeDBEngineVersions",
        "DescribeOrderableDBInstanceOptions",
    ] {
        let case = format!("{action} for the graph engine");
        let body = expect_ok(
            &case,
            call(&router, action, &[("Engine", GRAPH_ENGINE)]).await,
        );
        assert!(
            body.contains(GRAPH_ENGINE),
            "case {case:?}: expected the engine's own catalog, got {body}"
        );
    }
}
