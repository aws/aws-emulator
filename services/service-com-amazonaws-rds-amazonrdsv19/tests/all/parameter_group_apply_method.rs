// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the APPLY METHOD a parameter group's own parameter listing reports,
//! at both levels of parameter group this front door serves.
//!
//! RULE: a parameter group's listing is about the GROUP, and every parameter of a group
//! carries an apply method — `Parameter$ApplyMethod` "indicates when to apply parameter
//! updates". A parameter whose value the group SETS carries the method the request asked for
//! (or the one its apply type implies when it asked for none); a parameter the group sets
//! nothing for carries `pending-reboot`, the method a change to it would take while none is
//! waiting to be applied. The FAMILY-level engine-default report
//! (`DescribeEngineDefaultParameters`, `DescribeEngineDefaultClusterParameters`) is about the
//! engine and about no group at all, so it reports no apply method: the two projections of
//! one published record differ in exactly that member.
//!
//! The rule is asserted over a TABLE spanning what it generalises over: both levels of
//! parameter group, every engine vocabulary this front door publishes a catalog for, several
//! families per engine, both apply types, and both halves of a group's listing (the
//! parameters it sets and the ones it does not).

use service_com_amazonaws_rds_amazonrdsv19::state::{
    family_parameters, published_cluster_parameters, published_db_parameters,
    ClusterParameterSpec,
    APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT, APPLY_TYPE_DYNAMIC,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, optional_text, text};
use crate::engine_default_parameters::{reported, Reported};

/// A group name one row can be created under: the documented name rule admits letters,
/// digits and hyphens only, so a family's dots and underscores become hyphens, and the row's
/// own position keeps two rows of one family from naming one group.
fn group_name(prefix: &str, row: usize, family: &str) -> String {
    format!("{prefix}-{row}-{}", family.replace(['.', '_'], "-"))
}

/// The DB (instance) parameter group families under test: one per engine vocabulary this
/// front door publishes a DB-parameter catalog for, plus a second family of two of those
/// engines — the parameters belong to the ENGINE, so a second family of it must report the
/// same way.
const DB_FAMILIES: [&str; 7] = [
    "mysql5.7",
    "mysql8.0",
    "mariadb10.11",
    "postgres16",
    "postgres17",
    "oracle-ee-19",
    "sqlserver-ee-15.0",
];

/// The DB CLUSTER parameter group families under test: one per cluster engine vocabulary,
/// including the two sibling control planes this front door also serves and a second family
/// of one engine.
const CLUSTER_FAMILIES: [&str; 5] = [
    "aurora-mysql5.7",
    "aurora-mysql8.0",
    "aurora-postgresql16",
    "docdb5.0",
    "neptune1.3",
];

/// Create a DB parameter group of `family` and answer the name the store keys it under.
async fn create_db_group(
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
                ("Description", "an apply-method row"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// Create a DB cluster parameter group of `family` and answer the name the store keys it
/// under.
async fn create_cluster_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    family: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "an apply-method row"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// Every `<Parameter>` `action` reports for `group`, paginated to the end: `action` is the
/// describe of one LEVEL of parameter group and `key` its group-name member, so one reader
/// serves both levels.
async fn listing(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    key: &str,
    group: &str,
    source: Option<&str>,
) -> Vec<Reported> {
    let mut found: Vec<Reported> = Vec::new();
    let mut marker: Option<String> = None;
    // A page carries at most 100 records and the longest catalog this service publishes is
    // well inside eight pages; the loop is bounded so a marker that never clears cannot hang.
    for _ in 0..8 {
        let mut params: Vec<(&str, String)> = vec![(key, group.to_string())];
        if let Some(source) = source {
            params.push(("Source", source.to_string()));
        }
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let body = expect_ok(case, call(router, action, &params).await);
        found.extend(reported(&body));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return found;
        }
    }
    panic!("case {case:?}: the parameter listing of {group} never finished paging");
}

/// The entry `listed` carries for the parameter `name`.
fn entry_for<'a>(case: &str, listed: &'a [Reported], name: &str) -> &'a Reported {
    listed
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the listing to carry {name}, got {:?}",
                listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
            )
        })
}

/// A value the parameter `spec` admits, read from the domain it publishes: the first value of
/// an enumeration, the low bound of a range, and the single admitted value otherwise.
fn an_admitted_value(spec: &ClusterParameterSpec) -> Option<String> {
    if spec.allowed_values.is_empty() {
        return None;
    }
    if spec.allowed_values.contains(',') {
        return spec
            .allowed_values
            .split(',')
            .next()
            .map(|value| value.trim().to_string());
    }
    match spec.range() {
        Some((low, _)) if matches!(spec.data_type, "integer" | "float") => {
            Some(low.trim().to_string())
        }
        _ => Some(spec.allowed_values.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Rule 1: a group reports an apply method for every parameter of its family
// ---------------------------------------------------------------------------

/// Every row of a DB parameter group's listing carries an apply method: `pending-reboot`
/// while the group sets no value of its own, and the method the caller asked for once it
/// does — while the family-level engine-default report of the SAME catalog carries none.
#[tokio::test]
async fn a_db_parameter_groups_listing_reports_an_apply_method_for_every_parameter() {
    let router = emulator();

    for (row, family) in DB_FAMILIES.into_iter().enumerate() {
        let catalog = family_parameters(
            family,
            published_db_parameters(family)
                .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog")),
        );
        let group = create_db_group(
            &router,
            family,
            &group_name("apply-method-pg", row, family),
            family,
        )
        .await;

        // 1. A group that has set nothing reports every parameter of its family under the
        //    method a change to it would take while none is waiting.
        let listed = listing(
            &router,
            family,
            "DescribeDBParameters",
            "DBParameterGroupName",
            &group,
            None,
        )
        .await;
        assert_eq!(
            listed.len(),
            catalog.len(),
            "case {family:?}: expected one row per published parameter, got {:?}",
            listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
        );
        for entry in &listed {
            let case = format!("{family} {}", entry.name);
            assert_eq!(
                (entry.source.as_deref(), entry.apply_method.as_deref()),
                (
                    Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                    Some(APPLY_METHOD_PENDING_REBOOT)
                ),
                "case {case:?}: an unchanged parameter is the engine's default and waits for \
                 the reboot, got {entry:?}"
            );
        }

        // 2. The FAMILY's own engine-default report is about the engine and about no group,
        //    so the same published records carry no apply method there.
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
            assert_eq!(
                entry.apply_method, None,
                "case {case:?}: the engine's own report carries no ApplyMethod, got {entry:?}"
            );
        }

        // 3. Once the group sets one parameter, that row carries the method the request asked
        //    for and its untouched siblings still carry the reboot.
        let Some(spec) = catalog
            .iter()
            .find(|spec| spec.is_modifiable && spec.apply_type == APPLY_TYPE_DYNAMIC)
        else {
            continue;
        };
        let Some(value) = an_admitted_value(spec) else {
            continue;
        };
        let case = format!("{family} {} set immediately", spec.name);
        expect_ok(
            &case,
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", group.clone()),
                    ("Parameters.Parameter.1.ParameterName", spec.name.to_string()),
                    ("Parameters.Parameter.1.ParameterValue", value.clone()),
                    (
                        "Parameters.Parameter.1.ApplyMethod",
                        APPLY_METHOD_IMMEDIATE.to_string(),
                    ),
                ],
            )
            .await,
        );
        let listed = listing(
            &router,
            &case,
            "DescribeDBParameters",
            "DBParameterGroupName",
            &group,
            None,
        )
        .await;
        let changed = entry_for(&case, &listed, spec.name);
        assert_eq!(
            (changed.source.as_deref(), changed.apply_method.as_deref()),
            (Some(PARAMETER_SOURCE_USER), Some(APPLY_METHOD_IMMEDIATE)),
            "case {case:?}: the parameter the caller set carries the method it asked for, got \
             {changed:?}"
        );
        for entry in listed.iter().filter(|entry| entry.name != spec.name) {
            let case = format!("{case}, sibling {}", entry.name);
            assert_eq!(
                entry.apply_method.as_deref(),
                Some(APPLY_METHOD_PENDING_REBOOT),
                "case {case:?}: a parameter nobody changed still waits for the reboot, got \
                 {entry:?}"
            );
        }
    }
}

/// The same rule at the CLUSTER level, including the two sibling control planes' engines.
#[tokio::test]
async fn a_cluster_parameter_groups_listing_reports_an_apply_method_for_every_parameter() {
    let router = emulator();

    for (row, family) in CLUSTER_FAMILIES.into_iter().enumerate() {
        let catalog = family_parameters(
            family,
            published_cluster_parameters(family)
                .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog")),
        );
        let group = create_cluster_group(
            &router,
            family,
            &group_name("apply-method-cpg", row, family),
            family,
        )
        .await;

        for source in [None, Some(PARAMETER_SOURCE_ENGINE_DEFAULT)] {
            let case = format!("{family} under Source {source:?}");
            let listed = listing(
                &router,
                &case,
                "DescribeDBClusterParameters",
                "DBClusterParameterGroupName",
                &group,
                source,
            )
            .await;
            assert_eq!(
                listed.len(),
                catalog.len(),
                "case {case:?}: expected one row per published cluster parameter, got {:?}",
                listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
            );
            for entry in &listed {
                let case = format!("{case}, {}", entry.name);
                assert_eq!(
                    (entry.source.as_deref(), entry.apply_method.as_deref()),
                    (
                        Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                        Some(APPLY_METHOD_PENDING_REBOOT)
                    ),
                    "case {case:?}: an unchanged cluster parameter is the engine's default \
                     and waits for the reboot, got {entry:?}"
                );
            }
        }

        // The family's own cluster engine-default report carries no apply method.
        let family_level = expect_ok(
            family,
            call(
                &router,
                "DescribeEngineDefaultClusterParameters",
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        );
        for entry in reported(&family_level) {
            let case = format!("{family} {} at family level", entry.name);
            assert_eq!(
                entry.apply_method, None,
                "case {case:?}: the engine's own report carries no ApplyMethod, got {entry:?}"
            );
        }

        // A parameter the caller sets carries the method asked for; its siblings still wait.
        let Some(spec) = catalog
            .iter()
            .find(|spec| spec.is_modifiable && spec.apply_type == APPLY_TYPE_DYNAMIC)
        else {
            continue;
        };
        let Some(value) = an_admitted_value(spec) else {
            continue;
        };
        let case = format!("{family} {} set immediately", spec.name);
        expect_ok(
            &case,
            call(
                &router,
                "ModifyDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.clone()),
                    ("Parameters.Parameter.1.ParameterName", spec.name.to_string()),
                    ("Parameters.Parameter.1.ParameterValue", value.clone()),
                    (
                        "Parameters.Parameter.1.ApplyMethod",
                        APPLY_METHOD_IMMEDIATE.to_string(),
                    ),
                ],
            )
            .await,
        );
        let listed = listing(
            &router,
            &case,
            "DescribeDBClusterParameters",
            "DBClusterParameterGroupName",
            &group,
            None,
        )
        .await;
        let changed = entry_for(&case, &listed, spec.name);
        assert_eq!(
            (changed.source.as_deref(), changed.apply_method.as_deref()),
            (Some(PARAMETER_SOURCE_USER), Some(APPLY_METHOD_IMMEDIATE)),
            "case {case:?}: the cluster parameter the caller set carries the method it asked \
             for, got {changed:?}"
        );
        for entry in listed.iter().filter(|entry| entry.name != spec.name) {
            let case = format!("{case}, sibling {}", entry.name);
            assert_eq!(
                entry.apply_method.as_deref(),
                Some(APPLY_METHOD_PENDING_REBOOT),
                "case {case:?}: a cluster parameter nobody changed still waits for the \
                 reboot, got {entry:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the first cluster parameter each Aurora MySQL family publishes
// ---------------------------------------------------------------------------

/// One family's first published cluster parameter: the family, the parameter, its apply
/// type, data type, published domain, default value and a value a caller may set.
type FirstParameter = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// The first cluster parameter each Aurora MySQL family publishes.
///
/// The two families differ because a family publishes the parameters ITS OWN engine release
/// carries: `activate_all_roles_on_login` arrived in the engine's 8.0 release line, so the
/// 5.7-compatible family does not carry it and its listing begins at the Aurora parameter
/// that follows it in name order.
///
/// Each record is written out as LITERALS so a drift in the catalog fails this test rather
/// than moving with it.
const FIRST_CLUSTER_PARAMETERS: [FirstParameter; 2] = [
    (
        "aurora-mysql8.0",
        "activate_all_roles_on_login",
        APPLY_TYPE_DYNAMIC,
        "boolean",
        "0,1",
        "0",
        "1",
    ),
    (
        "aurora-mysql5.7",
        "aurora_binlog_read_buffer_size",
        APPLY_TYPE_DYNAMIC,
        "integer",
        "8192-536870912",
        "5242880",
        "8192",
    ),
];

/// A DB cluster parameter group of an Aurora MySQL family reports, as the first row of its
/// listing, the first cluster parameter that family publishes, with the whole published
/// record — and carries the caller's value for it once one is set.
#[tokio::test]
async fn an_aurora_mysql_cluster_group_publishes_the_engines_first_cluster_parameter() {
    let router = emulator();

    for (row, (family, first, apply_type, data_type, allowed_values, value, set_to)) in
        FIRST_CLUSTER_PARAMETERS.into_iter().enumerate()
    {
        let group = create_cluster_group(
            &router,
            family,
            &group_name("first-cpg", row, family),
            family,
        )
        .await;
        let listed = listing(
            &router,
            family,
            "DescribeDBClusterParameters",
            "DBClusterParameterGroupName",
            &group,
            None,
        )
        .await;
        assert_eq!(
            listed.first().map(|entry| entry.name.as_str()),
            Some(first),
            "case {family:?}: the listing is in name order, so it begins with {first}, got {:?}",
            listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
        );
        let entry = entry_for(family, &listed, first);
        assert_eq!(
            (
                entry.apply_type.as_deref(),
                entry.data_type.as_deref(),
                entry.allowed_values.as_deref(),
                entry.value.as_deref(),
                entry.is_modifiable.as_deref(),
                entry.source.as_deref(),
                entry.apply_method.as_deref(),
            ),
            (
                Some(apply_type),
                Some(data_type),
                Some(allowed_values),
                Some(value),
                Some("true"),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                Some(APPLY_METHOD_PENDING_REBOOT),
            ),
            "case {family:?}: expected the record the family publishes for {first}, got {entry:?}"
        );

        // It is a parameter of the group like any other: a modify stores it and the listing
        // reports the caller's value under `user`.
        expect_ok(
            family,
            call(
                &router,
                "ModifyDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.clone()),
                    ("Parameters.Parameter.1.ParameterName", first.to_string()),
                    ("Parameters.Parameter.1.ParameterValue", set_to.to_string()),
                ],
            )
            .await,
        );
        let own = expect_ok(
            family,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Source", PARAMETER_SOURCE_USER),
                ],
            )
            .await,
        );
        assert_eq!(
            (text(&own, "ParameterName"), text(&own, "ParameterValue")),
            (first.to_string(), set_to.to_string()),
            "case {family:?}: expected the value the caller set for {first}, got {own}"
        );
    }
}
