// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeEngineDefaultParameters`: the engine's own parameter
//! defaults, which belong to a DB parameter group FAMILY rather than to any group a caller
//! created.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with.
//!
//! The rules under test, each asserted over a TABLE spanning the class it generalises over
//! rather than over one request:
//!
//! 1. A family is an engine this front door serves followed by that engine's version, and
//!    the report is THAT engine's published parameters, each sourced `engine-default`, in
//!    parameter-name order. The table is DERIVED rather than hand-picked: it is every
//!    `DBParameterGroupFamily` the engine-version catalogue publishes, for every engine,
//!    read back through `DescribeDBEngineVersions`.
//! 2. What each engine publishes, over one named entry per engine vocabulary — the
//!    MySQL-compatible engines (MySQL, MariaDB, Aurora MySQL), the PostgreSQL-compatible
//!    ones, Oracle, Microsoft SQL Server (RDS Custom included), Amazon DocumentDB and
//!    Amazon Neptune — including entries and families no issue cited.
//! 3. A value that is not a family at all — an engine name with no version, a version with
//!    no engine, a name no served engine carries — is refused with a 400 rather than
//!    answered with an empty list.
//! 4. `Filters`: "The only supported filter is parameter-name", so a `parameter-name`
//!    filter selects exactly the parameters it names, repeated filters intersect, and any
//!    other filter name is refused.
//! 5. `MaxRecords`: both published bounds of the documented window are served and one step
//!    past each is refused, and a `Marker` no prior page issued is refused.

use std::collections::BTreeSet;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, DB_INSTANCE_ENGINES, MAX_RECORDS_RANGE,
    PARAMETER_SOURCE_ENGINE_DEFAULT, SIBLING_CONTROL_PLANE_ENGINES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text,
};

/// The operation under test.
const ACTION: &str = "DescribeEngineDefaultParameters";

/// The error code Amazon RDS answers a parameter value it cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The `DataType` values this service's catalogs publish — the four the engines' own
/// parameter lists use.
///
/// `pub(crate)` for the same reason as [`Reported`]: the cluster-parameter suite asserts the
/// same published set.
pub(crate) const DATA_TYPES: [&str; 4] = ["boolean", "float", "integer", "string"];

/// The parameters whose value the engine does not PUBLISH, and which therefore report no
/// `ParameterValue` at all: MySQL's `back_log` is autosized from the permitted connection
/// count ("Default Value: -1 (autosized)"), and `basedir` is the installation directory the
/// managed service owns and reports unset. Every other published parameter must report the
/// value its engine runs with; these are the NAMED exceptions, not a licence to omit one.
const AUTOSIZED: [&str; 2] = ["back_log", "basedir"];

/// The parameters the engine bounds by NOTHING it publishes, and which therefore report no
/// `AllowedValues` at all: `basedir` is a path the managed service owns, so there is no
/// enumeration and no range for the report to carry. Every other published parameter must
/// report the values it admits; this is the NAMED exception.
const UNBOUNDED: [&str; 1] = ["basedir"];

/// One reported `<Parameter>`.
///
/// `pub(crate)` with the reader below because the sibling suite for
/// `DescribeEngineDefaultClusterParameters` reads the same `<Parameter>` element out of its
/// own responses: one projection of that element, not two.
#[derive(Debug, Clone)]
pub(crate) struct Reported {
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) apply_type: Option<String>,
    pub(crate) data_type: Option<String>,
    pub(crate) allowed_values: Option<String>,
    pub(crate) is_modifiable: Option<String>,
    pub(crate) minimum_engine_version: Option<String>,
    pub(crate) apply_method: Option<String>,
}

/// Every `<Parameter>` in a response body, in the order it was reported.
pub(crate) fn reported(xml: &str) -> Vec<Reported> {
    subtrees(xml, "Parameter")
        .into_iter()
        .map(|entry| Reported {
            name: text(&entry, "ParameterName"),
            value: optional_text(&entry, "ParameterValue"),
            description: optional_text(&entry, "Description"),
            source: optional_text(&entry, "Source"),
            apply_type: optional_text(&entry, "ApplyType"),
            data_type: optional_text(&entry, "DataType"),
            allowed_values: optional_text(&entry, "AllowedValues"),
            is_modifiable: optional_text(&entry, "IsModifiable"),
            minimum_engine_version: optional_text(&entry, "MinimumEngineVersion"),
            apply_method: optional_text(&entry, "ApplyMethod"),
        })
        .collect()
}

/// The operation with `params`, asserted to answer 200: the family it echoed, the
/// parameters it reported and the `Marker` it carried.
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

/// The defaults of one family, asserted to answer 200.
async fn defaults_of(router: &MultiServiceRouter, case: &str, family: &str) -> Vec<Reported> {
    describe(
        router,
        case,
        &[("DBParameterGroupFamily", family.to_string())],
    )
    .await
    .1
}

// ---------------------------------------------------------------------------
// Rule 1: every published family reports its engine's defaults
// ---------------------------------------------------------------------------

/// For every `DBParameterGroupFamily` this front door publishes an engine version for, the
/// report carries that engine's parameters with the metadata the API models, sourced
/// `engine-default` and in parameter-name order.
///
/// The one documented exception is a row of the same table: the Db2 engines, which this
/// emulator publishes no parameter catalog for and so reports an empty list for rather than
/// invented parameter names and defaults.
#[tokio::test]
async fn every_published_family_reports_its_engines_defaults() {
    let router = emulator();

    // The table: every (engine, family) pair the engine-version catalogue publishes, read
    // back through the operation that publishes it.
    let mut families: BTreeSet<(String, String)> = BTreeSet::new();
    for engine in DB_INSTANCE_ENGINES
        .iter()
        .chain(SIBLING_CONTROL_PLANE_ENGINES.iter())
    {
        for version in all_versions_of(&router, engine).await {
            families.insert((engine.to_string(), version.parameter_group_family));
        }
    }
    assert!(
        families.len() >= 20,
        "case {:?}: the engine-version catalogue must publish families for every engine, \
         got {} pairs: {families:?}",
        "the derived table",
        families.len()
    );

    for (engine, family) in &families {
        let case = format!("{engine} family {family}");
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

        // The Db2 engines are the documented no-catalog case; every other engine publishes
        // parameters.
        let publishes = !engine.starts_with("db2");
        assert_eq!(
            !parameters.is_empty(),
            publishes,
            "case {case:?}: expected published parameters == {publishes}, got {} entries",
            parameters.len()
        );

        let names: Vec<String> = parameters.iter().map(|p| p.name.clone()).collect();
        let mut ordered = names.clone();
        ordered.sort();
        ordered.dedup();
        assert_eq!(
            names, ordered,
            "case {case:?}: expected ascending, unique parameter names, got {names:?}"
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
                parameter.value.is_some() || AUTOSIZED.contains(&name.as_str()),
                "case {case:?}: {name} must report the value the engine runs with, got \
                 {:?}",
                parameter.value
            );
            assert!(
                parameter.value.as_ref().is_none_or(|value| !value.is_empty()),
                "case {case:?}: {name} must never report an EMPTY value, got {:?}",
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
                    .is_some_and(|values| !values.is_empty())
                    || UNBOUNDED.contains(&name.as_str()),
                "case {case:?}: {name} must report the values it admits, got {:?}",
                parameter.allowed_values
            );
            assert!(
                matches!(parameter.is_modifiable.as_deref(), Some("true") | Some("false")),
                "case {case:?}: {name} must report IsModifiable, got {:?}",
                parameter.is_modifiable
            );
            // `ApplyMethod` is "when to apply parameter updates" — a property of a change
            // a caller asked for, and nobody asked for an engine default.
            assert_eq!(
                parameter.apply_method, None,
                "case {case:?}: {name} is an engine default, so it carries no ApplyMethod, \
                 got {:?}",
                parameter.apply_method
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: what each engine vocabulary publishes
// ---------------------------------------------------------------------------

/// One expected catalog entry: (case, family, parameter name, apply type, data type,
/// allowed values, default value, modifiable).
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

/// One entry per engine vocabulary this front door serves, each read from the engine's own
/// published parameter list. The rows span the engines rather than the one family any
/// recorded request used, and include a parameter the engine fixes (Oracle's block size),
/// a default the service computes from the instance class, and the RDS Custom edition of
/// an engine, which runs the same engine as its non-custom counterpart.
const PUBLISHED: [Published; 12] = [
    (
        "MySQL",
        "mysql8.0",
        "max_connections",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "1-100000",
        "{DBInstanceClassMemory/12582880}",
        true,
    ),
    (
        "MySQL, an earlier family of the same engine",
        "mysql5.7",
        "long_query_time",
        APPLY_TYPE_DYNAMIC,
        "float",
        "0-31536000",
        "10",
        true,
    ),
    (
        "MariaDB shares the MySQL vocabulary",
        "mariadb10.11",
        "slow_query_log",
        APPLY_TYPE_DYNAMIC,
        "boolean",
        "0,1",
        "0",
        true,
    ),
    (
        "Aurora MySQL shares the MySQL vocabulary",
        "aurora-mysql8.0",
        "innodb_lock_wait_timeout",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "1-1073741824",
        "50",
        true,
    ),
    (
        "PostgreSQL",
        "postgres16",
        "log_min_duration_statement",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "-1-2147483647",
        "-1",
        true,
    ),
    (
        "Aurora PostgreSQL shares the PostgreSQL vocabulary",
        "aurora-postgresql15",
        "shared_buffers",
        APPLY_TYPE_STATIC,
        "integer",
        "16-1073741823",
        "{DBInstanceClassMemory/32768}",
        true,
    ),
    (
        "Oracle",
        "oracle-ee-19",
        "open_cursors",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "1-65535",
        "300",
        true,
    ),
    (
        "Oracle, a parameter the engine fixes",
        "oracle-se2-cdb-21",
        "db_block_size",
        APPLY_TYPE_STATIC,
        "integer",
        "8192",
        "8192",
        false,
    ),
    (
        "Microsoft SQL Server",
        "sqlserver-ee-15.0",
        "cost threshold for parallelism",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "0-32767",
        "5",
        true,
    ),
    (
        "RDS Custom for SQL Server runs the same engine",
        "custom-sqlserver-ee-16.0",
        "clr enabled",
        APPLY_TYPE_DYNAMIC,
        "boolean",
        "0,1",
        "0",
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
        "Amazon Neptune",
        "neptune1.2",
        "neptune_query_timeout",
        APPLY_TYPE_STATIC,
        "integer",
        "10-2147483647",
        "120000",
        true,
    ),
];

/// Each engine's published parameters carry that engine's own metadata, and two families of
/// one engine publish the same catalog.
#[tokio::test]
async fn a_family_reports_its_engines_published_parameters() {
    let router = emulator();

    for (case, family, name, apply_type, data_type, allowed_values, default_value, modifiable) in
        PUBLISHED
    {
        let parameters = defaults_of(&router, case, family).await;
        let found = parameters.iter().find(|p| p.name == name);
        let Some(found) = found else {
            panic!(
                "case {case:?}: expected {family} to publish {name}, got {:?}",
                parameters.iter().map(|p| &p.name).collect::<Vec<_>>()
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

    // Two families of ONE engine publish that engine's parameters, each the ones ITS OWN
    // release line carries: what only the later line publishes is what the engine
    // introduced there, and the two lists agree on everything neither release boundary
    // touches. Two different engines' catalogs differ outright.
    let mysql_8 = defaults_of(&router, "mysql8.0", "mysql8.0").await;
    let mysql_5_7 = defaults_of(&router, "mysql5.7", "mysql5.7").await;
    let postgres = defaults_of(&router, "postgres16", "postgres16").await;
    let names = |parameters: &[Reported]| -> Vec<String> {
        parameters.iter().map(|p| p.name.clone()).collect()
    };
    let later = names(&mysql_8);
    let earlier = names(&mysql_5_7);
    let shared = earlier.iter().filter(|name| later.contains(name)).count();
    assert!(
        shared * 2 > earlier.len(),
        "case {:?}: two lines of one engine share that engine's vocabulary, got {earlier:?} \
         against {later:?}",
        "mysql5.7 against mysql8.0"
    );
    let only_later: Vec<&String> = later
        .iter()
        .filter(|name| !earlier.contains(name))
        .collect();
    assert!(
        !only_later.is_empty(),
        "case {:?}: the engine introduced parameters in its later release line, so the \
         earlier family must not publish all of them, got {later:?}",
        "mysql8.0 against mysql5.7"
    );
    for name in only_later {
        let row = mysql_8
            .iter()
            .find(|p| &p.name == name)
            .unwrap_or_else(|| panic!("case {name:?}: just read from this report"));
        assert!(
            row.minimum_engine_version
                .as_deref()
                .is_some_and(|version| version.starts_with("8.")),
            "case {name:?}: a parameter only the later family publishes is one the engine \
             introduced in that line, so it reports a MinimumEngineVersion inside it, got \
             {:?}",
            row.minimum_engine_version
        );
    }
    assert_ne!(
        later,
        names(&postgres),
        "case {:?}: two engines publish different catalogs",
        "mysql8.0 against postgres16"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: what is a family at all
// ---------------------------------------------------------------------------

/// A `DBParameterGroupFamily` names an engine this front door serves followed by that
/// engine's version — or, for the three Db2 editions the member's own value list writes
/// without one, the engine name alone; a value that is not one is refused with a 400
/// `InvalidParameterValue` rather than answered with an empty parameter list.
#[tokio::test]
async fn a_value_that_is_not_a_family_is_refused() {
    let router = emulator();

    // (case, family, served, publishes parameters)
    let rows: [(&str, &str, bool, bool); 11] = [
        ("an engine and its version", "mysql8.0", true, true),
        ("an engine whose name carries a hyphen", "oracle-ee-cdb-19", true, true),
        ("a sibling control plane's engine", "docdb4.0", true, true),
        (
            "a version this emulator publishes no engine version for",
            "postgres12",
            true,
            true,
        ),
        // The member's own value list writes the Db2 families without a version, and this
        // emulator publishes no Db2 parameter catalog, so the family is served and the
        // report is empty.
        ("a Db2 family, as the member's value list writes it", "db2-ae", true, false),
        ("a Db2 family carrying its version", "db2-se-11.5", true, false),
        ("the engine name with no version", "mysql", false, false),
        ("a version with no engine", "8.0", false, false),
        ("a name no served engine carries", "not-an-engine1.0", false, false),
        ("a lettered version", "mysqlX", false, false),
        ("another service's parameter group family", "memcached1.6", false, false),
    ];

    for (case, family, served, publishes) in rows {
        let response = call(&router, ACTION, &[("DBParameterGroupFamily", family)]).await;
        if served {
            let body = expect_ok(case, response);
            assert_eq!(
                !reported(&body).is_empty(),
                publishes,
                "case {case:?}: expected published parameters == {publishes} for {family}, \
                 got {body}"
            );
            continue;
        }
        expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the parameter-name filter
// ---------------------------------------------------------------------------

/// `Filters.Filter.<n>` as query parameters: one `parameter-name` filter per entry of
/// `wanted`, each carrying that entry's values.
fn filter_params(family: &str, wanted: &[&[&str]]) -> Vec<(String, String)> {
    let mut params = vec![(
        "DBParameterGroupFamily".to_string(),
        family.to_string(),
    )];
    for (index, values) in wanted.iter().enumerate() {
        params.push((
            format!("Filters.Filter.{}.Name", index + 1),
            "parameter-name".to_string(),
        ));
        for (position, value) in values.iter().enumerate() {
            params.push((
                format!("Filters.Filter.{}.Values.Value.{}", index + 1, position + 1),
                (*value).to_string(),
            ));
        }
    }
    params
}

/// One filter row: (case, one `parameter-name` filter per entry, the names expected back).
type FilterRow = (
    &'static str,
    &'static [&'static [&'static str]],
    &'static [&'static str],
);

/// The only supported filter is `parameter-name`: it selects exactly the parameters it
/// names, several filters intersect, and any other filter name is refused.
#[tokio::test]
async fn the_parameter_name_filter_selects_exactly_the_parameters_it_names() {
    let router = emulator();
    let family = "postgres16";

    let rows: [FilterRow; 6] = [
        ("no filter at all", &[], &[]),
        ("one name", &[&["work_mem"]], &["work_mem"]),
        (
            "two names in one filter, reported in catalog order",
            &[&["work_mem", "autovacuum"]],
            &["autovacuum", "work_mem"],
        ),
        (
            "a name this engine does not publish",
            &[&["neptune_query_timeout"]],
            &[],
        ),
        (
            "two filters sharing one name intersect",
            &[&["work_mem", "autovacuum"], &["autovacuum", "log_statement"]],
            &["autovacuum"],
        ),
        (
            "two filters naming disjoint sets intersect to nothing",
            &[&["work_mem"], &["autovacuum"]],
            &[],
        ),
    ];

    let unfiltered: Vec<String> = defaults_of(&router, "unfiltered", family)
        .await
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert!(
        unfiltered.len() >= 3,
        "case {:?}: the filter rows need a catalog of several parameters, got {unfiltered:?}",
        "unfiltered"
    );

    for (case, filters, expected) in rows {
        let params = filter_params(family, filters);
        let params: Vec<(&str, String)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let (_, parameters, _) = describe(&router, case, &params).await;
        let names: Vec<String> = parameters.iter().map(|p| p.name.clone()).collect();
        let expected: Vec<String> = if filters.is_empty() {
            unfiltered.clone()
        } else {
            expected.iter().map(|name| (*name).to_string()).collect()
        };
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }

    // A filter name this operation does not support is refused rather than ignored.
    expect_error(
        "a filter name the operation does not support",
        call(
            &router,
            ACTION,
            &[
                ("DBParameterGroupFamily", family),
                ("Filters.Filter.1.Name", "parameter-source"),
                ("Filters.Filter.1.Values.Value.1", "engine-default"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the pagination window
// ---------------------------------------------------------------------------

/// `MaxRecords` is documented "Default: 100 Constraints: Minimum 20, maximum 100": both
/// published bounds are served and one step past each is refused, and a `Marker` no prior
/// page issued is refused rather than resynchronised to the first page.
///
/// A page holds as many records as the request asked for while records are left, and the
/// answer carries a continuation `Marker` exactly while they are: the MySQL family's
/// published catalog is larger than the smallest page the member admits, so the paging rows
/// below walk it to its end.
#[tokio::test]
async fn the_documented_max_records_window_is_served_and_its_bounds_are_refused() {
    let router = emulator();
    let family = "mysql8.0";
    // The documented window, as LITERALS from the member's own sentence ("Default: 100
    // Constraints: Minimum 20, maximum 100") rather than from the constant the handler
    // reads, so a drift in that constant fails this test instead of moving with it.
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

    let whole_catalog = defaults_of(&router, "the whole catalog", family).await.len();
    assert!(
        whole_catalog > low as usize,
        "case {:?}: this family's catalog must be larger than the smallest page for the \
         paging rows below to walk it, got {whole_catalog} parameters",
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
            let page = (max_records as usize).min(whole_catalog);
            assert_eq!(
                reported(&body).len(),
                page,
                "case {case:?}: a page holds what the request asked for while records are \
                 left, got {body}"
            );
            // The marker is owed exactly while parameters are left to report.
            assert_eq!(
                optional_text(&body, "Marker").is_some(),
                page < whole_catalog,
                "case {case:?}: a Marker is owed exactly while records are left, got {body}"
            );
            // And resuming from it walks the REST of the catalog, page by page, so the walk
            // covers the whole published list and no record is served twice.
            let mut seen = page;
            let mut cursor = optional_text(&body, "Marker");
            while let Some(marker) = cursor {
                let rest = expect_ok(
                    case,
                    call(
                        &router,
                        ACTION,
                        &[
                            ("DBParameterGroupFamily", family.to_string()),
                            ("MaxRecords", max_records.to_string()),
                            ("Marker", marker),
                        ],
                    )
                    .await,
                );
                let rows = reported(&rest).len();
                assert!(
                    rows > 0 && rows <= page,
                    "case {case:?}: a page after a marker holds at least one record and no \
                     more than the request asked for, got {rows} in {rest}"
                );
                seen += rows;
                cursor = optional_text(&rest, "Marker");
            }
            assert_eq!(
                seen, whole_catalog,
                "case {case:?}: paging from the first marker to the last reports the whole \
                 published catalog exactly once, got {seen} of {whole_catalog}"
            );
            continue;
        }
        expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
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
