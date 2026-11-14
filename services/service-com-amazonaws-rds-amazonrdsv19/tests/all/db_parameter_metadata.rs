// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the METADATA `DescribeDBParameters` publishes for a DB (instance)
//! parameter group, and for the domain a modify refuses a value against.
//!
//! RULE: a DB parameter group reports, for every parameter its family publishes, that
//! parameter's whole declared record — the value the engine runs with, its `Source`, its
//! `ApplyType`, its `DataType`, its `AllowedValues` and its `IsModifiable` — so a client can
//! read what a parameter accepts before it writes one, and that same published domain is what
//! a write is refused against. One family has ONE published catalogue, whichever operation is
//! asked, so the group's parameter report and `DescribeEngineDefaultParameters` cannot
//! disagree about what a parameter admits.
//!
//! The sibling suites own the neighbouring rules: `db_parameters` the source filter, the
//! apply method, pagination and the value rule over the catalogue, and
//! `engine_default_parameters` the family-level report of the same catalogue. What is pinned
//! HERE is that a group just created already publishes the declared domain of each of its
//! family's parameters, and that a value outside it is refused in the wording Amazon RDS
//! refuses one with.
//!
//! Each rule is asserted over a TABLE spanning what it generalises over — the engine
//! vocabulary, the family (several per engine, including ones no issue cited), the data type
//! (boolean, integer, float, string), the apply type, whether the engine lets a caller modify
//! the parameter, and the form of the published domain (an enumeration, a positive range, a
//! range with a negative bound, a single admitted value).

use service_com_amazonaws_rds_amazonrdsv19::state::{
    family_parameters, published_db_parameters, ClusterParameterSpec, APPLY_METHOD_PENDING_REBOOT,
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, text};
use crate::engine_default_parameters::{reported, Reported};

/// The error code Amazon RDS answers a parameter value it cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// Create a DB parameter group of `family` and answer the name the store keys it under.
async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    family: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// A group name one row can be created under: the documented name rule admits letters,
/// digits and hyphens only, so a family's dots become hyphens, and the row's own position
/// keeps two rows of one family from naming one group.
fn group_name(prefix: &str, row: usize, family: &str) -> String {
    format!("{prefix}-{row}-{}", family.replace(['.', '_'], "-"))
}

/// Every `<Parameter>` `DescribeDBParameters` reports for `group` with NO `Source`, which is
/// what that member documents as "all parameter types returned", paginated to the end.
async fn every_parameter_of(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> Vec<Reported> {
    let mut found: Vec<Reported> = Vec::new();
    let mut marker: Option<String> = None;
    // A page is at most 100 records, so the walk is bounded by the longest catalogue this
    // service publishes; the loop is capped so a marker that never clears cannot hang.
    for _ in 0..8 {
        let mut params: Vec<(&str, String)> =
            vec![("DBParameterGroupName", group.to_string())];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let body = expect_ok(case, call(router, "DescribeDBParameters", &params).await);
        found.extend(reported(&body));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return found;
        }
    }
    panic!("case {case:?}: the parameter report of {group} never finished paging");
}

/// The entry `reported` carries for the parameter `name`.
fn entry_for<'a>(case: &str, parameters: &'a [Reported], name: &str) -> &'a Reported {
    parameters
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the report to carry {name}, got {:?}",
                parameters.iter().map(|entry| &entry.name).collect::<Vec<_>>()
            )
        })
}

/// What the declared record of one parameter must look like on the wire, as the assertions
/// below read it: `(ApplyType, DataType, AllowedValues, ParameterValue, IsModifiable,
/// Source)`.
type Declared = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The declared record of `entry`, as reported.
fn declared(entry: &Reported) -> Declared {
    (
        entry.apply_type.clone(),
        entry.data_type.clone(),
        entry.allowed_values.clone(),
        entry.value.clone(),
        entry.is_modifiable.clone(),
        entry.source.clone(),
    )
}

/// The record the FAMILY publishes for `spec`: the catalogue's own columns, except for a
/// parameter whose published record moved between engine versions, which is the record that
/// family's version carries.
///
/// The one such parameter in the catalogues below is PostgreSQL's `rds.force_ssl`: from
/// PostgreSQL 15 the engine applies a change to it without a restart (`dynamic`) and RDS for
/// PostgreSQL publishes its default as 1, where the versions before it publish `static` and 0
/// (the Amazon RDS User Guide's RDS for PostgreSQL parameter table). The rule is written out
/// HERE as literals rather than read from the crate, so a drift in it fails this test; the
/// rows of `postgres_rds_namespace_parameters` beside it pin the same values family by family.
fn published_record(family: &str, spec: &ClusterParameterSpec) -> (&'static str, &'static str) {
    // Only a PostgreSQL-compatible family: the SQL Server catalogue carries a parameter of
    // this name too, and that engine's record does not move.
    let postgres = family
        .strip_prefix("postgres")
        .or_else(|| family.strip_prefix("aurora-postgresql"));
    let Some(version) = postgres.filter(|_| spec.name == "rds.force_ssl") else {
        return (spec.apply_type, spec.default_value);
    };
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok());
    match major {
        // The default that moved is RDS for PostgreSQL's own; another PostgreSQL-compatible
        // engine keeps the catalogue's.
        Some(major) if major >= 15 => (
            APPLY_TYPE_DYNAMIC,
            if family.starts_with("postgres") {
                "1"
            } else {
                spec.default_value
            },
        ),
        Some(_) => (APPLY_TYPE_STATIC, spec.default_value),
        None => (spec.apply_type, spec.default_value),
    }
}

/// The declared record `spec` publishes FOR `family`, as an engine default is reported.
///
/// An EMPTY column is one the engine publishes nothing for, and the matching member is then
/// left out rather than reported empty, which is how the service projects it: `ParameterValue`
/// for a parameter the engine autosizes (MySQL's `back_log`) or whose value the managed
/// service owns (`basedir`), and `AllowedValues` for one the engine bounds by nothing.
fn published(family: &str, spec: &ClusterParameterSpec) -> Declared {
    let (apply_type, default_value) = published_record(family, spec);
    (
        Some(apply_type.to_string()),
        Some(spec.data_type.to_string()),
        (!spec.allowed_values.is_empty()).then(|| spec.allowed_values.to_string()),
        (!default_value.is_empty()).then(|| default_value.to_string()),
        Some(spec.is_modifiable.to_string()),
        Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()),
    )
}

// ---------------------------------------------------------------------------
// The rule, over the published catalogue of every family in the table
// ---------------------------------------------------------------------------

/// One family per engine vocabulary this front door publishes a DB-parameter catalogue for,
/// plus two more families of two of those engines — a family is one major version of an
/// engine and the parameters belong to the ENGINE, so a second family of the same engine must
/// publish the same records.
///
/// `postgres17` is the family the recorded case used; the rest are families and engines no
/// issue cited.
const FAMILIES: [&str; 9] = [
    "postgres17",
    "postgres16",
    "aurora-postgresql16",
    "mysql8.0",
    "mysql5.7",
    "mariadb10.11",
    "oracle-ee-19",
    "sqlserver-ee-15.0",
    "neptune1.3",
];

/// A group just created reports, for every parameter its family publishes, the whole declared
/// record of that parameter — under the engine's own value and `Source` `engine-default`,
/// because the group has set none of its own.
#[tokio::test]
async fn a_new_groups_parameter_report_publishes_each_parameters_declared_record() {
    let router = emulator();

    for (row, family) in FAMILIES.into_iter().enumerate() {
        let catalog = family_parameters(
            family,
            published_db_parameters(family)
                .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalogue")),
        );
        let group = create_group(
            &router,
            family,
            &group_name("declared", row, family),
            family,
        )
        .await;
        let parameters = every_parameter_of(&router, family, &group).await;

        // Every published parameter is reported, and nothing else is: the group has set no
        // parameter of its own, so the report is exactly the family's catalogue.
        let reported_names: Vec<&str> = parameters
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let published_names: Vec<&str> = catalog.iter().map(|spec| spec.name).collect();
        assert_eq!(
            reported_names, published_names,
            "case {family:?}: a fresh group must report its family's whole published \
             catalogue, in name order"
        );

        for spec in &catalog {
            let case = format!("{family} {}", spec.name);
            let entry = entry_for(&case, &parameters, spec.name);
            assert_eq!(
                declared(entry),
                published(family, spec),
                "case {case:?}: expected the declared record the family publishes, got \
                 {entry:?}"
            );
            assert!(
                entry.description.as_ref().is_some_and(|text| !text.is_empty()),
                "case {case:?}: a published parameter reports its Description, got {:?}",
                entry.description
            );
            // "ApplyMethod: indicates when to apply parameter updates" — and in a GROUP's
            // own listing every parameter carries one: a parameter nobody has changed is
            // reported under the method a change to it would take while none is waiting,
            // `pending-reboot`. The FAMILY-level report below is about the engine and about
            // no group, so it carries none.
            assert_eq!(
                entry.apply_method.as_deref(),
                Some(APPLY_METHOD_PENDING_REBOOT),
                "case {case:?}: a group reports an engine default under the apply method a \
                 change to it would take, got {:?}",
                entry.apply_method
            );
        }

        // One family, one catalogue: what the group reports for a parameter is what the
        // family-level report publishes for the same parameter.
        let family_level = expect_ok(
            family,
            call(
                &router,
                "DescribeEngineDefaultParameters",
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        );
        for entry in reported(&family_level) {
            let case = format!("{family} {} at family level", entry.name);
            let group_level = entry_for(&case, &parameters, &entry.name);
            assert_eq!(
                declared(group_level),
                declared(&entry),
                "case {case:?}: the group's report and the family's must agree about the \
                 parameter's declared record"
            );
        }
    }
}

/// Named rows of the same rule: the parameter the recorded case read, and parameters of other
/// engines spanning the data types, the apply types, the published-domain forms and a
/// parameter the engine does not let a caller modify.
///
/// The expected values are the engines' own published records (the `ClusterParameterSpec`
/// tables of `crate::ops::describe_engine_default_parameters`, sourced from each engine's
/// configuration documentation), written here as LITERALS so a drift in the catalogue fails
/// this test rather than moving with it.
#[tokio::test]
async fn a_named_parameter_of_each_engine_reports_its_published_domain() {
    let router = emulator();

    // (case, family, parameter, apply type, data type, allowed values, default, modifiable)
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        bool,
    );
    let rows: [Row; 8] = [
        (
            "the PostgreSQL planner GUC the recorded case read",
            "postgres17",
            "from_collapse_limit",
            APPLY_TYPE_DYNAMIC,
            "integer",
            "1-2147483647",
            "8",
            true,
        ),
        (
            "its sibling GUC, in another PostgreSQL family",
            "postgres16",
            "join_collapse_limit",
            APPLY_TYPE_DYNAMIC,
            "integer",
            "1-2147483647",
            "8",
            true,
        ),
        (
            "a PostgreSQL parameter whose domain is a fraction",
            "aurora-postgresql16",
            "cursor_tuple_fraction",
            APPLY_TYPE_DYNAMIC,
            "float",
            "0-1",
            "0.1",
            true,
        ),
        (
            "a PostgreSQL parameter the engine applies on reboot, with a computed default",
            "postgres17",
            "max_connections",
            APPLY_TYPE_STATIC,
            "integer",
            "6-8388607",
            "LEAST({DBInstanceClassMemory/9531392},5000)",
            true,
        ),
        (
            "a PostgreSQL parameter whose range admits a negative value",
            "postgres17",
            "log_min_duration_statement",
            APPLY_TYPE_DYNAMIC,
            "integer",
            "-1-2147483647",
            "-1",
            true,
        ),
        (
            "a MySQL parameter whose domain is an enumeration",
            "mysql8.0",
            "innodb_flush_log_at_trx_commit",
            APPLY_TYPE_DYNAMIC,
            "integer",
            "0,1,2",
            "1",
            true,
        ),
        (
            "a MariaDB group, which shares the MySQL vocabulary",
            "mariadb10.11",
            "max_allowed_packet",
            APPLY_TYPE_DYNAMIC,
            "integer",
            "1024-1073741824",
            "67108864",
            true,
        ),
        (
            "an Oracle parameter the engine fixes, so a caller cannot modify it",
            "oracle-ee-19",
            "db_block_size",
            APPLY_TYPE_STATIC,
            "integer",
            "8192",
            "8192",
            false,
        ),
    ];

    for (row, (case, family, name, apply_type, data_type, allowed_values, default_value, modifiable)) in
        rows.into_iter().enumerate()
    {
        let group = create_group(&router, case, &group_name("named", row, family), family).await;
        let parameters = every_parameter_of(&router, case, &group).await;
        let entry = entry_for(case, &parameters, name);
        assert_eq!(
            declared(entry),
            (
                Some(apply_type.to_string()),
                Some(data_type.to_string()),
                Some(allowed_values.to_string()),
                Some(default_value.to_string()),
                Some(modifiable.to_string()),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()),
            ),
            "case {case:?}: {name} of {family} reported {entry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The published domain is what a write is refused against
// ---------------------------------------------------------------------------

/// `ModifyDBParameterGroup` on `group` with one setting.
async fn modify(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
    value: &str,
) -> (u16, String) {
    call(
        router,
        "ModifyDBParameterGroup",
        &[
            ("DBParameterGroupName", group),
            ("Parameters.Parameter.1.ParameterName", name),
            ("Parameters.Parameter.1.ParameterValue", value),
        ],
    )
    .await
}

/// A value outside a parameter's published domain is refused with `InvalidParameterValue`,
/// whose message names the submitted value, that parameter's published domain and the
/// parameter — and the group is left reporting the engine's default for it, since a refused
/// write changes nothing.
///
/// The rows span the forms a published domain takes and the ways a value falls outside one:
/// below a range's low bound, above its high bound, outside an enumeration, and a value that
/// is no number at all. Each row's admitted counterpart is written first, so the refusal is
/// asserted against a parameter the group really carries a value for. The two forms of
/// domain carry the two wordings Amazon RDS gives this refusal, one each.
#[tokio::test]
async fn a_value_outside_a_parameters_published_domain_is_refused_naming_that_domain() {
    let router = emulator();

    // (case, family, parameter, published domain, an admitted value, a refused value)
    let rows: [(&str, &str, &str, &str, &str, &str); 6] = [
        (
            "below the low bound of a PostgreSQL range",
            "postgres17",
            "from_collapse_limit",
            "1-2147483647",
            "1",
            "0",
        ),
        (
            "above the high bound of the same range",
            "postgres17",
            "from_collapse_limit",
            "1-2147483647",
            "8",
            "2147483648",
        ),
        (
            "a value that is no number at all",
            "postgres16",
            "join_collapse_limit",
            "1-2147483647",
            "16",
            "eight",
        ),
        (
            "outside a fractional range",
            "aurora-postgresql16",
            "cursor_tuple_fraction",
            "0-1",
            "0.5",
            "2",
        ),
        (
            "outside a MySQL enumeration",
            "mysql8.0",
            "innodb_flush_log_at_trx_commit",
            "0,1,2",
            "2",
            "3",
        ),
        (
            "outside a MySQL byte range",
            "mariadb10.11",
            "max_allowed_packet",
            "1024-1073741824",
            "1024",
            "1023",
        ),
    ];

    for (row, (case, family, name, domain, admitted, refused)) in rows.into_iter().enumerate() {
        let group =
            create_group(&router, case, &group_name("domain", row, family), family).await;

        // The admitted value is written and read back as the caller's own, so the group under
        // test carries a value this refusal must not change.
        expect_ok(case, modify(&router, &group, name, admitted).await);
        let carried = every_parameter_of(&router, case, &group).await;
        let entry = entry_for(case, &carried, name);
        assert_eq!(
            (entry.value.as_deref(), entry.source.as_deref()),
            (Some(admitted), Some(PARAMETER_SOURCE_USER)),
            "case {case:?}: expected {name} to carry the admitted value as the caller's, got \
             {entry:?}"
        );

        let answered = expect_error(
            case,
            modify(&router, &group, name, refused).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&answered, "Message");
        // Amazon RDS words the refusal by the FORM of the domain the value fell outside: a
        // numeric `<low>-<high>` range is "outside of range", while a comma-separated value
        // list is an invalid parameter value whose allowed values are listed.
        let expected = if domain.contains(',') {
            format!("Invalid parameter value: {refused} for: {name} allowed values are: {domain}")
        } else {
            format!("Value: {refused} is outside of range: {domain} for parameter: {name}")
        };
        assert_eq!(
            message, expected,
            "case {case:?}: the refusal must name the value, the published domain and the \
             parameter, got {message:?}"
        );

        // The refused write changed nothing: the group still reports the value it carried.
        let after = every_parameter_of(&router, case, &group).await;
        let entry = entry_for(case, &after, name);
        assert_eq!(
            (entry.value.as_deref(), entry.allowed_values.as_deref()),
            (Some(admitted), Some(domain)),
            "case {case:?}: a refused write must leave {name} as it was, got {entry:?}"
        );
    }
}
