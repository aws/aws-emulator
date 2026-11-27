// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CLUSTER-level binary-log parameters of the MySQL-compatible
//! cluster engines, the enhanced binary log's own set among them.
//!
//! RULE: a DB cluster parameter group carries the cluster-level parameters the engine release
//! its family names publishes, so `ModifyDBClusterParameterGroup` naming one of them with a
//! value inside that parameter's published domain is accepted (HTTP 200, echoing the group
//! name) and the group reports the value and the apply method the request asked for; only a
//! name no level of the family's engine publishes is refused `InvalidParameterValue` with
//! "Could not find parameter with name: <name>", and such a refusal stores nothing.
//!
//! The rows are DERIVED from the published catalog rather than hand-picked wherever they can
//! be — every value each parameter's own `AllowedValues` publishes, on two families of the
//! engine — so the table spans the domain the rule generalises over (the parameter, its value,
//! the number of settings one request carries, and the family's release line) instead of the
//! one request a recording made. The near-miss names are written out, because a name the
//! catalog does not carry cannot be derived from it.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    family_parameters, parameter_not_found_message, published_cluster_parameters,
    ClusterParameterSpec, APPLY_METHOD_PENDING_REBOOT, APPLY_TYPE_STATIC, DATA_TYPE_BOOLEAN,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, text};
use crate::engine_default_parameters::{reported, Reported};

/// The error code the AWS Query protocol answers a `ParameterValue` or a `ParameterName` this
/// front door cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The Aurora MySQL family whose release line publishes the enhanced binary log.
const FAMILY: &str = "aurora-mysql8.0";

/// A family of the SAME engine from an earlier release line, which the enhanced binary log's
/// parameters were introduced after: it publishes none of them.
const EARLIER_FAMILY: &str = "aurora-mysql5.7";

/// The parameters the ENHANCED binary log is configured through, which is the set a request
/// naming them has to be accepted for.
///
/// Source: the cluster-level rows of the published Aurora MySQL parameter list that the
/// service's own catalog carries
/// (`describe_engine_default_cluster_parameters::MYSQL_CLUSTER_PARAMETERS`) — the names only,
/// so every column each one is asserted against below is read from that catalog rather than
/// repeated here.
const ENHANCED_BINLOG_PARAMETERS: [&str; 3] = [
    "aurora_enhanced_binlog",
    "binlog_backup",
    "binlog_replication_globaldb",
];

/// The whole set of binary-log settings a caller configures before it can replicate out of a
/// cluster of this engine — the enhanced log's three, plus the three the log's own format is
/// set by — each with a value its published domain carries. ONE request carries all six, so
/// the rule is asserted on a multi-parameter modify as well as on the single-parameter ones.
const BINLOG_PREREQUISITES: [(&str, &str); 6] = [
    ("aurora_enhanced_binlog", "1"),
    ("binlog_backup", "0"),
    ("binlog_format", "ROW"),
    ("binlog_replication_globaldb", "0"),
    ("binlog_row_image", "full"),
    ("binlog_row_metadata", "FULL"),
];

/// Names the catalogs publish NO parameter of: near misses of the three above (a plural, a
/// split word, a prefix), so the refusal is asserted against names shaped like the ones the
/// family really has rather than against obvious nonsense.
const UNPUBLISHED_NAMES: [&str; 4] = [
    "binlog_backups",
    "binlog_replication_global_db",
    "aurora_enhanced_binlogs",
    "enhanced_binlog",
];

/// Create a DB cluster parameter group of `family` and answer the name the store keys it
/// under (every group name "is stored as a lowercase string").
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
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "an enhanced binary log row"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// `ModifyDBClusterParameterGroup` on `group` with `settings`, every one of them asked for at
/// `apply_method`.
async fn modify(
    router: &MultiServiceRouter,
    group: &str,
    settings: &[(&str, &str)],
    apply_method: &str,
) -> (u16, String) {
    let mut params: Vec<(String, String)> =
        vec![("DBClusterParameterGroupName".to_string(), group.to_string())];
    for (index, (name, value)) in settings.iter().enumerate() {
        let position = index + 1;
        params.push((
            format!("Parameters.Parameter.{position}.ParameterName"),
            (*name).to_string(),
        ));
        params.push((
            format!("Parameters.Parameter.{position}.ParameterValue"),
            (*value).to_string(),
        ));
        params.push((
            format!("Parameters.Parameter.{position}.ApplyMethod"),
            apply_method.to_string(),
        ));
    }
    call(router, "ModifyDBClusterParameterGroup", &params).await
}

/// Every `<Parameter>` the group `group` reports under `source`, paginated to the end.
async fn listing(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    source: &str,
) -> Vec<Reported> {
    let mut found: Vec<Reported> = Vec::new();
    let mut marker: Option<String> = None;
    // A page carries at most 100 records and the longest catalog this front door publishes is
    // well inside eight pages; the loop is bounded so a marker that never clears cannot hang.
    for _ in 0..8 {
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterParameterGroupName", group.to_string()),
            ("Source", source.to_string()),
        ];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let body = expect_ok(case, call(router, "DescribeDBClusterParameters", &params).await);
        found.extend(reported(&body));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return found;
        }
    }
    panic!("case {case:?}: the parameter listing of {group} never finished paging");
}

/// The entry `listed` carries for `name`, if any.
fn entry_for<'a>(listed: &'a [Reported], name: &str) -> Option<&'a Reported> {
    listed.iter().find(|entry| entry.name == name)
}

/// The cluster parameters `family` publishes that `names` names, read from the published
/// catalog of the family's own release line.
fn published(family: &str, names: &[&str]) -> Vec<&'static ClusterParameterSpec> {
    let catalog = published_cluster_parameters(family).unwrap_or_default();
    family_parameters(family, catalog)
        .into_iter()
        .filter(|spec| names.contains(&spec.name))
        .collect()
}

/// The engine defaults `DescribeEngineDefaultClusterParameters` reports for `family`.
async fn engine_defaults_of(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
) -> Vec<Reported> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeEngineDefaultClusterParameters",
            &[("DBParameterGroupFamily", family)],
        )
        .await,
    );
    reported(&body)
}

/// The family publishes each enhanced-binary-log parameter with the record its catalog
/// carries, and an earlier release line of the same engine publishes none of them.
#[tokio::test]
async fn the_family_publishes_the_enhanced_binlog_parameters() {
    let router = emulator();

    let specs = published(FAMILY, &ENHANCED_BINLOG_PARAMETERS);
    assert_eq!(
        specs.len(),
        ENHANCED_BINLOG_PARAMETERS.len(),
        "case {:?}: expected {FAMILY} to publish every one of {ENHANCED_BINLOG_PARAMETERS:?}, \
         got {:?}",
        "the published catalog",
        specs.iter().map(|spec| spec.name).collect::<Vec<_>>()
    );

    let defaults = engine_defaults_of(&router, "the 8.0 release line", FAMILY).await;
    for spec in &specs {
        let case = format!("{FAMILY} publishes {}", spec.name);
        let Some(found) = entry_for(&defaults, spec.name) else {
            panic!(
                "case {case:?}: expected the engine defaults to carry {}, got {:?}",
                spec.name,
                defaults.iter().map(|entry| &entry.name).collect::<Vec<_>>()
            );
        };
        // The switch of a log either runs or does not, and the engine reads the change only
        // when it starts, so each one is published as a boolean that takes a reboot.
        assert_eq!(
            (
                spec.apply_type,
                spec.data_type,
                spec.allowed_values,
                spec.is_modifiable
            ),
            (APPLY_TYPE_STATIC, DATA_TYPE_BOOLEAN, "0,1", true),
            "case {case:?}: expected the published record of a cluster-level switch, got {spec:?}"
        );
        assert_eq!(
            (
                found.apply_type.as_deref(),
                found.data_type.as_deref(),
                found.allowed_values.as_deref(),
                found.value.as_deref(),
                found.source.as_deref(),
                found.is_modifiable.as_deref(),
            ),
            (
                Some(spec.apply_type),
                Some(spec.data_type),
                Some(spec.allowed_values),
                Some(spec.default_value),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                Some("true"),
            ),
            "case {case:?}: the report must carry the published record, got {found:?}"
        );
        assert!(
            found
                .description
                .as_ref()
                .is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected a Description, got {:?}",
            found.description
        );
    }

    // The set arrived in the 8.0 release line, so the earlier line of the same engine carries
    // none of it — which is what `MinimumEngineVersion` states about each entry.
    let earlier = engine_defaults_of(&router, "the earlier release line", EARLIER_FAMILY).await;
    for name in ENHANCED_BINLOG_PARAMETERS {
        let case = format!("{EARLIER_FAMILY} does not publish {name}");
        assert!(
            entry_for(&earlier, name).is_none(),
            "case {case:?}: a release line earlier than the one that introduced {name} must \
             publish no record of it, got {:?}",
            entry_for(&earlier, name)
        );
    }
}

/// Every value each of these parameters publishes is accepted on its own, and the group
/// reports the value and the apply method the request asked for.
#[tokio::test]
async fn every_published_value_of_an_enhanced_binlog_parameter_is_accepted() {
    let router = emulator();

    let specs = published(FAMILY, &ENHANCED_BINLOG_PARAMETERS);
    assert!(
        !specs.is_empty(),
        "case {:?}: expected {FAMILY} to publish the enhanced binary log's parameters",
        "the published catalog"
    );
    let group = create_group(&router, "one parameter at a time", "emu-enh-binlog-one", FAMILY).await;

    for spec in &specs {
        // The rows are the parameter's OWN published domain, so the table follows the catalog
        // rather than a value a recording happened to send.
        for value in spec.allowed_values.split(',') {
            let case = format!("{FAMILY} {} = {value:?}", spec.name);
            let body = expect_ok(
                &case,
                modify(
                    &router,
                    &group,
                    &[(spec.name, value)],
                    APPLY_METHOD_PENDING_REBOOT,
                )
                .await,
            );
            let echoed = text(&body, "DBClusterParameterGroupName");
            assert_eq!(
                echoed, group,
                "case {case:?}: expected the group name echoed, got {echoed:?}"
            );

            let listed = listing(&router, &case, &group, PARAMETER_SOURCE_USER).await;
            let Some(found) = entry_for(&listed, spec.name) else {
                panic!(
                    "case {case:?}: expected the group to report the value it was given, got \
                     {:?}",
                    listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
                );
            };
            assert_eq!(
                (
                    found.value.as_deref(),
                    found.apply_method.as_deref(),
                    found.source.as_deref()
                ),
                (
                    Some(value),
                    Some(APPLY_METHOD_PENDING_REBOOT),
                    Some(PARAMETER_SOURCE_USER)
                ),
                "case {case:?}: the group must report the value and the apply method asked \
                 for, got {found:?}"
            );
        }
    }
}

/// ONE request carrying the whole binary-log prerequisite set is accepted and every one of
/// its settings is recorded: the handler checks each entry before it writes any, so a set
/// that is accepted is accepted whole.
#[tokio::test]
async fn one_request_records_the_whole_binlog_prerequisite_set() {
    let router = emulator();

    for (index, family) in [FAMILY, "mysql8.0"].into_iter().enumerate() {
        let case = format!("the prerequisite set on {family}");
        let group = create_group(
            &router,
            &case,
            &format!("emu-enh-binlog-set-{index}"),
            family,
        )
        .await;

        let body = expect_ok(
            &case,
            modify(
                &router,
                &group,
                &BINLOG_PREREQUISITES,
                APPLY_METHOD_PENDING_REBOOT,
            )
            .await,
        );
        let echoed = text(&body, "DBClusterParameterGroupName");
        assert_eq!(
            echoed, group,
            "case {case:?}: expected the group name echoed, got {echoed:?}"
        );

        let listed = listing(&router, &case, &group, PARAMETER_SOURCE_USER).await;
        for (name, value) in BINLOG_PREREQUISITES {
            let Some(found) = entry_for(&listed, name) else {
                panic!(
                    "case {case:?}: expected {name} among the group's own parameters, got {:?}",
                    listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
                );
            };
            assert_eq!(
                (found.value.as_deref(), found.apply_method.as_deref()),
                (Some(value), Some(APPLY_METHOD_PENDING_REBOOT)),
                "case {case:?}: expected {name} recorded as the request asked, got {found:?}"
            );
        }
        assert_eq!(
            listed.len(),
            BINLOG_PREREQUISITES.len(),
            "case {case:?}: the group carries exactly what the request set, got {:?}",
            listed.iter().map(|entry| &entry.name).collect::<Vec<_>>()
        );
    }
}

/// A name no level of the family's engine publishes is still refused as unknown, and the
/// refusal leaves the group exactly as it was — so the catalog's gain is the published
/// parameters and not the loss of the membership rule.
#[tokio::test]
async fn a_name_the_family_publishes_no_parameter_for_is_refused() {
    let router = emulator();

    let group = create_group(&router, "an unpublished name", "emu-enh-binlog-unknown", FAMILY).await;
    // One setting the family really publishes is in force first, so each refusal is made
    // against a group that carries something to lose.
    expect_ok(
        "a published parameter first",
        modify(
            &router,
            &group,
            &[("aurora_enhanced_binlog", "1")],
            APPLY_METHOD_PENDING_REBOOT,
        )
        .await,
    );

    for name in UNPUBLISHED_NAMES {
        let case = format!("{FAMILY} {name}");
        let answered = expect_error(
            &case,
            modify(&router, &group, &[(name, "0")], APPLY_METHOD_PENDING_REBOOT).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&answered, "Message");
        let expected = parameter_not_found_message(name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        let listed = listing(&router, &case, &group, PARAMETER_SOURCE_USER).await;
        let carried: Vec<&String> = listed.iter().map(|entry| &entry.name).collect();
        assert_eq!(
            carried,
            vec!["aurora_enhanced_binlog"],
            "case {case:?}: a refused name must leave the group as it was, got {carried:?}"
        );
    }
}
