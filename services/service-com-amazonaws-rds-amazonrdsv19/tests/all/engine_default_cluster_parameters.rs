// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeEngineDefaultClusterParameters`: the CLUSTER parameter
//! defaults of an engine, which belong to a DB cluster parameter group FAMILY rather than to
//! any group a caller created.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and every
//! assertion reads the XML the emulator answered with.
//!
//! The rules under test, each asserted over a TABLE spanning the class it generalises over
//! rather than over one request:
//!
//! 1. A DB cluster parameter group family is an engine that HAS a cluster form followed by
//!    that engine's version, and the report is that engine's published cluster parameters,
//!    each sourced `engine-default`, in parameter-name order. The table is DERIVED rather
//!    than hand-picked: it is every `DBParameterGroupFamily` the engine-version catalogue
//!    publishes, for every engine, read back through `DescribeDBEngineVersions` — the cluster
//!    engines' families served, every other engine's refused.
//! 2. What each cluster engine publishes, over one named entry per engine vocabulary — the
//!    MySQL-compatible cluster engines, the PostgreSQL-compatible ones, Amazon DocumentDB and
//!    Amazon Neptune — including entries and families no issue cited.
//! 3. The cluster report is a DIFFERENT catalog from the per-instance one
//!    (`DescribeEngineDefaultParameters`) for every engine that has both levels, and the same
//!    one for Amazon DocumentDB, which keeps every parameter in a cluster parameter group.
//! 4. A value that is not a cluster family — a family of an engine with no cluster form, an
//!    engine name with no version, a version with no engine, a name no served engine carries
//!    — is refused with a 400 rather than answered with an empty list.
//! 5. `Filters`: "This parameter isn't currently supported", so a request carrying one is
//!    refused rather than served as though the filter had been applied.
//! 6. `MaxRecords`: both published bounds of the documented window are served and one step
//!    past each is refused, and a `Marker` no prior page issued is refused.

use std::collections::BTreeSet;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, DB_CLUSTER_ENGINES, DB_INSTANCE_ENGINES,
    MAX_RECORDS_RANGE, PARAMETER_SOURCE_ENGINE_DEFAULT, SIBLING_CONTROL_PLANE_ENGINES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, text};
use crate::engine_default_parameters::{reported, Reported, DATA_TYPES};

/// The operation under test.
const ACTION: &str = "DescribeEngineDefaultClusterParameters";

/// The per-instance engine-default report, which this one must not be a copy of.
const INSTANCE_ACTION: &str = "DescribeEngineDefaultParameters";

/// The error code Amazon RDS answers a parameter value it cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The operation with `params`, asserted to answer 200: the family it echoed, the parameters
/// it reported and the `Marker` it carried.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, String)],
) -> (Option<String>, Vec<Reported>, Option<String>) {
    let body = expect_ok(case, call(router, ACTION, params).await);
    (
        optional_text(&body, "DBParameterGroupFamily"),
        reported(&body),
        optional_text(&body, "Marker"),
    )
}

/// The cluster defaults of one family, asserted to answer 200.
async fn cluster_defaults_of(router: &MultiServiceRouter, case: &str, family: &str) -> Vec<Reported> {
    describe(
        router,
        case,
        &[("DBParameterGroupFamily", family.to_string())],
    )
    .await
    .1
}

/// The reported parameter names, in the order they were reported.
fn names(parameters: &[Reported]) -> Vec<String> {
    parameters.iter().map(|p| p.name.clone()).collect()
}

/// Whether `engine` has a cluster form, which is what makes its families cluster families.
fn has_cluster_form(engine: &str) -> bool {
    DB_CLUSTER_ENGINES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(engine))
}

/// Every `(engine, family)` pair the engine-version catalogue publishes, read back through
/// the operation that publishes it.
async fn published_families(router: &MultiServiceRouter) -> BTreeSet<(String, String)> {
    let mut families: BTreeSet<(String, String)> = BTreeSet::new();
    for engine in DB_INSTANCE_ENGINES
        .iter()
        .chain(SIBLING_CONTROL_PLANE_ENGINES.iter())
    {
        for version in all_versions_of(router, engine).await {
            families.insert((engine.to_string(), version.parameter_group_family));
        }
    }
    families
}

// ---------------------------------------------------------------------------
// Rule 1: every published family of a cluster engine reports that engine's cluster defaults
// ---------------------------------------------------------------------------

/// For every `DBParameterGroupFamily` this front door publishes an engine version for, the
/// report carries that engine's cluster parameters with the metadata the API models, sourced
/// `engine-default` and in parameter-name order — and a family of an engine that runs only as
/// a single DB instance is refused, because it publishes no cluster parameters at all.
#[tokio::test]
async fn every_published_cluster_family_reports_its_engines_cluster_defaults() {
    let router = emulator();

    let families = published_families(&router).await;
    assert!(
        families.len() >= 20,
        "case {:?}: the engine-version catalogue must publish families for every engine, got \
         {} pairs: {families:?}",
        "the derived table",
        families.len()
    );
    assert!(
        families
            .iter()
            .filter(|(engine, _)| has_cluster_form(engine))
            .count()
            >= DB_CLUSTER_ENGINES.len(),
        "case {:?}: every cluster engine must contribute at least one family, got {families:?}",
        "the derived table"
    );

    for (engine, family) in &families {
        let case = format!("{engine} family {family}");
        if !has_cluster_form(engine) {
            expect_error(
                &case,
                call(&router, ACTION, &[("DBParameterGroupFamily", family)]).await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            continue;
        }

        let (echoed, parameters, marker) = describe(
            &router,
            &case,
            &[("DBParameterGroupFamily", family.clone())],
        )
        .await;
        assert_eq!(
            echoed.as_deref(),
            Some(family.as_str()),
            "case {case:?}: expected the requested family echoed, got {echoed:?}"
        );
        assert_eq!(
            marker, None,
            "case {case:?}: the published catalog fits one page, so no Marker is owed, got \
             {marker:?}"
        );
        assert!(
            !parameters.is_empty(),
            "case {case:?}: a cluster engine publishes cluster parameters, got none"
        );

        let reported_names = names(&parameters);
        let mut ordered = reported_names.clone();
        ordered.sort();
        ordered.dedup();
        assert_eq!(
            reported_names, ordered,
            "case {case:?}: expected ascending, unique parameter names, got {reported_names:?}"
        );

        for parameter in &parameters {
            let name = &parameter.name;
            assert_eq!(
                parameter.source.as_deref(),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                "case {case:?}: {name} is the engine's default, so expected Source \
                 {PARAMETER_SOURCE_ENGINE_DEFAULT}, got {:?}",
                parameter.source
            );
            assert!(
                parameter.value.is_some(),
                "case {case:?}: {name} must report the value the cluster runs with, got {:?}",
                parameter.value
            );
            assert!(
                parameter.description.as_ref().is_some_and(|d| !d.is_empty()),
                "case {case:?}: {name} must report a Description, got {:?}",
                parameter.description
            );
            assert!(
                matches!(
                    parameter.apply_type.as_deref(),
                    Some(APPLY_TYPE_STATIC) | Some(APPLY_TYPE_DYNAMIC)
                ),
                "case {case:?}: {name} must report one of the two apply types, got {:?}",
                parameter.apply_type
            );
            assert!(
                parameter
                    .data_type
                    .as_deref()
                    .is_some_and(|data_type| DATA_TYPES.contains(&data_type)),
                "case {case:?}: {name} must report one of {DATA_TYPES:?}, got {:?}",
                parameter.data_type
            );
            assert!(
                parameter
                    .allowed_values
                    .as_ref()
                    .is_some_and(|values| !values.is_empty()),
                "case {case:?}: {name} must report the values it admits, got {:?}",
                parameter.allowed_values
            );
            assert!(
                matches!(parameter.is_modifiable.as_deref(), Some("true") | Some("false")),
                "case {case:?}: {name} must report IsModifiable, got {:?}",
                parameter.is_modifiable
            );
            // `ApplyMethod` is "when to apply parameter updates" — a property of a change a
            // caller asked for, and nobody asked for an engine default.
            assert_eq!(
                parameter.apply_method, None,
                "case {case:?}: {name} is an engine default, so it carries no ApplyMethod, got \
                 {:?}",
                parameter.apply_method
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: what each cluster engine vocabulary publishes
// ---------------------------------------------------------------------------

/// One expected catalog entry: (case, family, parameter name, apply type, data type, allowed
/// values, default value, modifiable).
type Published = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    bool,
);

/// One entry per cluster engine vocabulary this front door serves, each read from the
/// engine's own published cluster parameter list. The rows span the engines rather than the
/// one family any recorded request used, and include a parameter that takes effect only after
/// a reboot, one that does not, an engine whose cluster form is a Multi-AZ DB cluster rather
/// than an Aurora one, and a parameter whose default is on rather than off.
const PUBLISHED: [Published; 9] = [
    (
        "Aurora MySQL",
        "aurora-mysql8.0",
        "binlog_format",
        APPLY_TYPE_STATIC,
        "string",
        "ROW,STATEMENT,MIXED,OFF",
        "OFF",
        true,
    ),
    (
        "Aurora MySQL, an earlier family of the same engine",
        "aurora-mysql5.7",
        "require_secure_transport",
        APPLY_TYPE_DYNAMIC,
        "string",
        "ON,OFF",
        "OFF",
        true,
    ),
    (
        "RDS for MySQL in a Multi-AZ DB cluster runs the same engine",
        "mysql8.0",
        "server_audit_logging",
        APPLY_TYPE_DYNAMIC,
        "boolean",
        "0,1",
        "0",
        true,
    ),
    (
        "Aurora PostgreSQL",
        "aurora-postgresql15",
        "rds.logical_replication",
        APPLY_TYPE_STATIC,
        "boolean",
        "0,1",
        "0",
        true,
    ),
    (
        // Both halves of this parameter's published record moved at PostgreSQL 15 — the
        // engine applies a change to it without a restart from that release, and RDS for
        // PostgreSQL publishes its default as 1 — so a family of that engine past the bound
        // reports dynamic and 1; Aurora PostgreSQL keeps its own published default.
        "RDS for PostgreSQL in a Multi-AZ DB cluster runs the same engine",
        "postgres16",
        "rds.force_ssl",
        APPLY_TYPE_DYNAMIC,
        "boolean",
        "0,1",
        "1",
        true,
    ),
    (
        "Amazon DocumentDB",
        "docdb5.0",
        "tls",
        APPLY_TYPE_STATIC,
        "string",
        "disabled,enabled,fips-140-3,tls1.2+,tls1.3+",
        "enabled",
        true,
    ),
    (
        "Amazon DocumentDB, an earlier family of the same engine",
        "docdb3.6",
        "ttl_monitor",
        APPLY_TYPE_DYNAMIC,
        "string",
        "enabled,disabled",
        "enabled",
        true,
    ),
    (
        "Amazon Neptune",
        "neptune1.2",
        "neptune_streams",
        APPLY_TYPE_STATIC,
        "boolean",
        "0,1",
        "0",
        true,
    ),
    (
        "Amazon Neptune, a parameter whose domain is an enumeration",
        "neptune1.3",
        "neptune_enable_slow_query_log",
        APPLY_TYPE_DYNAMIC,
        "string",
        "disabled,info,debug",
        "disabled",
        true,
    ),
];

/// Each cluster engine's published parameters carry that engine's own metadata, and two
/// families of one engine publish the same catalog.
#[tokio::test]
async fn a_cluster_family_reports_its_engines_published_cluster_parameters() {
    let router = emulator();

    for (case, family, name, apply_type, data_type, allowed_values, default_value, modifiable) in
        PUBLISHED
    {
        let parameters = cluster_defaults_of(&router, case, family).await;
        let Some(found) = parameters.iter().find(|p| p.name == name) else {
            panic!(
                "case {case:?}: expected {family} to publish {name}, got {:?}",
                names(&parameters)
            );
        };
        assert_eq!(
            (
                found.apply_type.as_deref(),
                found.data_type.as_deref(),
                found.allowed_values.as_deref(),
                found.value.as_deref(),
                found.is_modifiable.as_deref(),
            ),
            (
                Some(apply_type),
                Some(data_type),
                Some(allowed_values),
                Some(default_value),
                Some(if modifiable { "true" } else { "false" }),
            ),
            "case {case:?}: {name} of {family} reported {found:?}"
        );
    }

    // Two families of ONE engine publish that engine's cluster parameters, each the ones ITS
    // OWN release line carries: the earlier line's list is a subset of the later line's.
    // Two different engines' catalogs differ outright.
    let aurora_mysql = cluster_defaults_of(&router, "aurora-mysql8.0", "aurora-mysql8.0").await;
    let aurora_mysql_5_7 = cluster_defaults_of(&router, "aurora-mysql5.7", "aurora-mysql5.7").await;
    let aurora_postgresql =
        cluster_defaults_of(&router, "aurora-postgresql15", "aurora-postgresql15").await;
    let later = names(&aurora_mysql);
    let earlier = names(&aurora_mysql_5_7);
    assert!(
        earlier.iter().all(|name| later.contains(name)) && earlier.len() < later.len(),
        "case {:?}: the earlier release line carries the later line's parameters except the \
         ones the engine introduced in it, got {earlier:?} against {later:?}",
        "aurora-mysql5.7 against aurora-mysql8.0"
    );
    assert_ne!(
        later,
        names(&aurora_postgresql),
        "case {:?}: two engines publish different catalogs",
        "aurora-mysql8.0 against aurora-postgresql15"
    );

    // A cluster parameter of a family whose engine publishes it reports the earliest engine
    // version it can apply at INSIDE that family: the later of the version it was introduced
    // in and the family's own first release, which for the 5.0 family is 5.0.0.
    let docdb = cluster_defaults_of(&router, "docdb5.0", "docdb5.0").await;
    let tls = docdb
        .iter()
        .find(|p| p.name == "tls")
        .expect("docdb5.0 publishes tls");
    assert_eq!(
        tls.minimum_engine_version.as_deref(),
        Some("5.0.0"),
        "case {:?}: expected the earliest engine version of the group's family that supports \
         tls, got {:?}",
        "docdb5.0 tls",
        tls.minimum_engine_version
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the cluster catalog is not the per-instance one
// ---------------------------------------------------------------------------

/// A cluster parameter applies to every instance of a DB cluster at once, so for an engine
/// that has both levels the cluster report is a different catalog from the per-instance one —
/// and for Amazon DocumentDB, which keeps every parameter in a DB cluster parameter group, it
/// is the same one.
#[tokio::test]
async fn the_cluster_catalog_differs_from_the_per_instance_catalog() {
    let router = emulator();

    // (case, family, the two reports carry the same catalog)
    let rows: [(&str, &str, bool); 5] = [
        ("Aurora MySQL has both levels", "aurora-mysql8.0", false),
        ("RDS for MySQL has both levels", "mysql8.0", false),
        ("Aurora PostgreSQL has both levels", "aurora-postgresql15", false),
        ("Amazon Neptune has both levels", "neptune1.2", false),
        // DocumentDB publishes one catalog, and it is the cluster one.
        ("Amazon DocumentDB has one level", "docdb5.0", true),
    ];

    for (case, family, same) in rows {
        let cluster = names(&cluster_defaults_of(&router, case, family).await);
        let instance = names(&reported(&expect_ok(
            case,
            call(
                &router,
                INSTANCE_ACTION,
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        )));
        assert!(
            !cluster.is_empty() && !instance.is_empty(),
            "case {case:?}: both reports must publish parameters, got {cluster:?} and \
             {instance:?}"
        );
        assert_eq!(
            cluster == instance,
            same,
            "case {case:?}: expected the two catalogs to be the same == {same}, got \
             {cluster:?} and {instance:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what is a cluster family at all
// ---------------------------------------------------------------------------

/// A `DBParameterGroupFamily` this operation serves is one this front door PUBLISHES whose
/// engine HAS a cluster form; anything else — a published family of an instance-only engine,
/// a family naming a release its engine never had, an engine name with no version, a version
/// with no engine, a name no served engine carries — is refused with a 400
/// `InvalidParameterValue` rather than answered with an empty parameter list.
#[tokio::test]
async fn a_value_that_is_not_a_cluster_family_is_refused() {
    let router = emulator();

    // (case, family, served)
    let rows: [(&str, &str, bool); 13] = [
        ("an Aurora engine and its version", "aurora-postgresql16", true),
        ("an engine whose cluster form is Multi-AZ", "postgres15", true),
        ("a sibling control plane's engine", "docdb4.0", true),
        ("the other sibling control plane's engine", "neptune1.3", true),
        (
            "a release this emulator publishes no engine version of",
            "aurora-mysql5.6",
            false,
        ),
        ("an engine that runs only as a DB instance", "oracle-ee-19", false),
        ("another instance-only engine", "sqlserver-ee-15.0", false),
        ("an instance-only engine of the MySQL family", "mariadb10.11", false),
        (
            "an instance-only family the member's value list writes without a version",
            "db2-ae",
            false,
        ),
        ("a cluster engine name with no version", "aurora-mysql", false),
        ("a version with no engine", "8.0", false),
        ("a name no served engine carries", "not-an-engine1.0", false),
        ("another service's parameter group family", "memcached1.6", false),
    ];

    for (case, family, served) in rows {
        let response = call(&router, ACTION, &[("DBParameterGroupFamily", family)]).await;
        if served {
            let body = expect_ok(case, response);
            assert!(
                !reported(&body).is_empty(),
                "case {case:?}: expected {family} to publish cluster parameters, got {body}"
            );
            continue;
        }
        expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
    }
}

// ---------------------------------------------------------------------------
// Rule 5: Filters is not supported
// ---------------------------------------------------------------------------

/// `Filters` is documented "This parameter isn't currently supported", so a request carrying
/// one is refused rather than served as though the filter had been applied — whatever the
/// filter names.
#[tokio::test]
async fn a_request_carrying_filters_is_refused() {
    let router = emulator();
    let family = "aurora-mysql8.0";

    // (case, filter name, filter value)
    let rows: [(&str, &str, &str); 3] = [
        ("a filter naming a parameter", "parameter-name", "binlog_format"),
        ("a filter naming a source", "parameter-source", "engine-default"),
        ("a filter name no RDS operation supports", "not-a-filter", "anything"),
    ];

    for (case, name, value) in rows {
        expect_error(
            case,
            call(
                &router,
                ACTION,
                &[
                    ("DBParameterGroupFamily", family),
                    ("Filters.Filter.1.Name", name),
                    ("Filters.Filter.1.Values.Value.1", value),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
    }

    // The same request without the filter is served, so the refusal is the filter's and not
    // the family's.
    assert!(
        !cluster_defaults_of(&router, "the same request with no filter", family)
            .await
            .is_empty(),
        "case {:?}: expected {family} to publish cluster parameters",
        "the same request with no filter"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the pagination window
// ---------------------------------------------------------------------------

/// `MaxRecords` is documented "Default: 100 Constraints: Minimum 20, maximum 100": both
/// published bounds are served and one step past each is refused, and a `Marker` no prior page
/// issued is refused rather than resynchronised to the first page.
///
/// There is no multi-page row: the smallest page the member admits is 20 records and no
/// engine's published cluster catalog reaches that, so this operation owes a `Marker` for no
/// request it can be sent — which is itself asserted here and in the suite above. Add a paging
/// row once a catalog grows past the documented minimum.
#[tokio::test]
async fn the_documented_max_records_window_is_served_and_its_bounds_are_refused() {
    let router = emulator();
    let family = "docdb5.0";
    // The documented window, as LITERALS from the member's own sentence ("Default: 100
    // Constraints: Minimum 20, maximum 100") rather than from the constant the handler reads,
    // so a drift in that constant fails this test instead of moving with it.
    let low = 20;
    let high = 100;
    assert_eq!(
        (*MAX_RECORDS_RANGE.start(), *MAX_RECORDS_RANGE.end()),
        (low, high),
        "case {:?}: the window this service enforces must be the documented one",
        "the documented window"
    );

    // (case, MaxRecords, served)
    let rows: [(&str, i32, bool); 5] = [
        ("the published minimum", low, true),
        ("the published maximum", high, true),
        ("inside the window", (low + high) / 2, true),
        ("one below the minimum", low - 1, false),
        ("one above the maximum", high + 1, false),
    ];

    let whole_catalog = cluster_defaults_of(&router, "the whole catalog", family)
        .await
        .len();
    assert!(
        whole_catalog > 0 && whole_catalog <= low as usize,
        "case {:?}: this family's catalog must fit the smallest page, got {whole_catalog} \
         parameters",
        "the page-size rows"
    );

    for (case, max_records, served) in rows {
        let response = call(
            &router,
            ACTION,
            &[
                ("DBParameterGroupFamily", family.to_string()),
                ("MaxRecords", max_records.to_string()),
            ],
        )
        .await;
        if served {
            let body = expect_ok(case, response);
            assert_eq!(
                reported(&body).len(),
                whole_catalog,
                "case {case:?}: every published parameter fits this page, got {body}"
            );
            assert_eq!(
                optional_text(&body, "Marker"),
                None,
                "case {case:?}: nothing is left to page, so no Marker is owed, got {body}"
            );
            continue;
        }
        expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
    }

    // The refusal is built from the request: it names the value rejected, the member it was
    // sent for and the bounds it must lie between.
    for (case, max_records) in [
        ("one below the minimum", low - 1),
        ("one above the maximum", high + 1),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                ACTION,
                &[
                    ("DBParameterGroupFamily", family.to_string()),
                    ("MaxRecords", max_records.to_string()),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        let expected = format!(
            "Invalid value {max_records} for MaxRecords. Must be between {low} and {high}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A `Marker` this operation never issued names no record of the result set.
    expect_error(
        "a Marker no prior page issued",
        call(
            &router,
            ACTION,
            &[
                ("DBParameterGroupFamily", family),
                ("Marker", "not-a-marker-this-service-minted"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
}
