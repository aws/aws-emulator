// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HEAD of the engine-default parameter catalog
//! `DescribeEngineDefaultParameters` publishes for a MySQL-compatible DB parameter group
//! family.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so every assertion reads the XML the emulator itself produced.
//!
//! The rule under test: the report is the named family's engine's own published default
//! parameter set, in ascending `ParameterName` order, each row carrying that parameter's own
//! published record — so the FIRST page is the first parameters of that set rather than a
//! sample taken from the middle of it, and `EngineDefaults.Marker` is a position in that
//! same order, from which the next page continues the sequence.
//!
//! A family is one RELEASE LINE of an engine, and it publishes the parameters that line
//! carries: a parameter the engine introduced in a later line is not among the earlier
//! family's defaults, so the head of the 8.0-line families opens with the parameter that
//! line added and the 5.7-line ones open at the next published name.
//!
//! The expected rows below are LITERALS, written from the engines' own published parameter
//! data (the MySQL manual's "Server System Variables", which states each variable's apply
//! time, data type, domain, default and the release it was introduced in, and the Amazon RDS
//! User Guide's MySQL parameter list, which states which of them the managed engine lets a
//! caller modify), so a drift in this service's catalog fails this test instead of moving
//! with it. The table spans the families that share the MySQL vocabulary — MySQL itself,
//! MariaDB and Aurora MySQL, and two families of one engine — and the named-row table below
//! pins entries no issue cited.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, PARAMETER_SOURCE_ENGINE_DEFAULT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, optional_text};
use crate::engine_default_parameters::{reported, Reported};

/// The operation under test.
const ACTION: &str = "DescribeEngineDefaultParameters";

/// The smallest page `MaxRecords` admits ("Constraints: Minimum 20, maximum 100"), which is
/// the window the head below is read through.
const PAGE: usize = 20;

/// The families that share the MySQL parameter vocabulary, each with the release line it
/// names: three lines of the engine itself, the Aurora MySQL engine that runs the same
/// vocabulary, and the MariaDB engine that runs it under its own release numbering. A family
/// publishes the parameters of the line it names, so the rows of one line publish the same
/// head.
const MYSQL_FAMILIES: [(&str, Line); 6] = [
    ("mysql5.7", Line::Mysql5_7),
    ("mysql8.0", Line::Mysql8_0),
    ("mysql8.4", Line::Mysql8_4),
    ("aurora-mysql5.7", Line::Mysql5_7),
    ("aurora-mysql8.0", Line::Mysql8_0),
    // MariaDB runs this vocabulary and numbers its OWN releases, so a MySQL release bound
    // says nothing about it and it carries every entry of the table.
    ("mariadb10.11", Line::Mariadb),
];

/// The release line a family names, which is what decides the head it publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// A family of MySQL's 5.7 line: it carries none of the parameters the engine introduced
    /// in 8.0, and the one the engine dropped later is still in it.
    Mysql5_7,
    /// A family of MySQL's 8.0 line: it carries the 8.0 additions AND the parameter the
    /// engine only dropped in a later line.
    Mysql8_0,
    /// A family of MySQL's 8.4 line: the additions, without the parameter that line dropped.
    Mysql8_4,
    /// A family of an engine that shares this vocabulary while numbering its own releases:
    /// no MySQL release bound applies to it, so it carries every entry.
    Mariadb,
}

/// The parameters the engine introduced in its 8.0 release line that fall inside the first
/// page, each with the release that introduced it: the rows an 8.0-line family publishes and
/// a 5.7-line family does not.
const INTRODUCED_IN_8_0: [(&str, &str); 3] = [
    ("activate_all_roles_on_login", "8.0.2"),
    ("binlog_expire_logs_seconds", "8.0.1"),
    ("binlog_row_metadata", "8.0.1"),
];

/// The parameters the engine DROPPED in its 8.4 release line, each with the release that
/// dropped it: the rows every earlier family publishes and an 8.4-line family does not, the
/// mirror of [`INTRODUCED_IN_8_0`].
const REMOVED_IN_8_4: [(&str, &str); 1] = [("big_tables", "8.4.0")];

/// The first page of the MySQL vocabulary's published defaults for a family of the 8.0 line,
/// in the ascending `ParameterName` order the report is paginated in: every entry of the
/// table's head, since that line is past the additions and before the removal.
const HEAD_8_0: [&str; PAGE] = [
    "activate_all_roles_on_login",
    "allow-suspicious-udfs",
    "auto_generate_certs",
    "auto_increment_increment",
    "auto_increment_offset",
    "autocommit",
    "automatic_sp_privileges",
    "avoid_temporal_upgrade",
    "back_log",
    "basedir",
    "big_tables",
    "binlog_cache_size",
    "binlog_checksum",
    "binlog_direct_non_transactional_updates",
    "binlog_error_action",
    "binlog_expire_logs_seconds",
    "binlog_format",
    "binlog_group_commit_sync_delay",
    "binlog_group_commit_sync_no_delay_count",
    "binlog_max_flush_queue_time",
];

/// The same page for a family of the 5.7 line: the same published order with the parameters
/// the engine had not introduced yet left out and the one it later dropped still in, so it
/// reaches two names further.
const HEAD_5_7: [&str; PAGE] = [
    "allow-suspicious-udfs",
    "auto_generate_certs",
    "auto_increment_increment",
    "auto_increment_offset",
    "autocommit",
    "automatic_sp_privileges",
    "avoid_temporal_upgrade",
    "back_log",
    "basedir",
    "big_tables",
    "binlog_cache_size",
    "binlog_checksum",
    "binlog_direct_non_transactional_updates",
    "binlog_error_action",
    "binlog_format",
    "binlog_group_commit_sync_delay",
    "binlog_group_commit_sync_no_delay_count",
    "binlog_max_flush_queue_time",
    "binlog_order_commits",
    "binlog_row_image",
];

/// The head a family of `line` publishes.
///
/// The 8.4 line publishes the 8.0 line's head with the parameter it DROPPED taken out, which
/// pulls the next name onto the page; a family whose engine numbers its own releases carries
/// every entry, which is the 8.0 head.
fn head(line: Line) -> Vec<&'static str> {
    match line {
        Line::Mysql5_7 => HEAD_5_7.to_vec(),
        Line::Mysql8_0 | Line::Mariadb => HEAD_8_0.to_vec(),
        Line::Mysql8_4 => {
            let (dropped, _) = REMOVED_IN_8_4[0];
            let mut page: Vec<&str> = HEAD_8_0
                .into_iter()
                .filter(|name| *name != dropped)
                .collect();
            page.push(AFTER_HEAD_8_0);
            page
        }
    }
}

/// The parameter that follows the 8.0 line's head, which is also the name the 8.4 line's
/// removal pulls onto its own page.
const AFTER_HEAD_8_0: &str = "binlog_order_commits";

/// The parameter that follows the head of `line`, so the page after the first page's
/// `Marker` is asserted to continue the one ordered sequence.
fn after_head(line: Line) -> &'static str {
    match line {
        Line::Mysql5_7 => "binlog_rows_query_log_events",
        Line::Mysql8_0 | Line::Mariadb => AFTER_HEAD_8_0,
        Line::Mysql8_4 => "binlog_row_image",
    }
}

/// The parameters of the first page the engine introduced INSIDE the releases these families
/// run, each with the release that introduced it: the only rows of the page that report a
/// `MinimumEngineVersion` at all, since "the earliest engine version to which the parameter
/// can apply" is only a fact worth reporting where the engine did not always carry the
/// parameter. The 8.0-line rows are in [`INTRODUCED_IN_8_0`], which a 5.7-line family does
/// not carry at all.
const INTRODUCED_IN: [(&str, &str); 3] = [
    ("auto_generate_certs", "5.7.5"),
    ("binlog_group_commit_sync_delay", "5.7.5"),
    ("binlog_group_commit_sync_no_delay_count", "5.7.5"),
];

/// One named row of the published record table: the parameter, its apply type, data type,
/// published domain (`None` where the engine bounds the value by nothing and publishes none),
/// default value (`None` where the engine publishes none) and whether a caller may modify it.
type Declared = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    bool,
);

/// Named rows the issue that filed this suite did not cite, spanning the record forms the
/// catalog carries: a parameter the managed engine does not let a caller modify, an
/// enumerated domain and a numeric range, a parameter the engine autosizes and so publishes
/// no default for, and both apply types.
const DECLARED: [Declared; 7] = [
    (
        "allow-suspicious-udfs",
        APPLY_TYPE_STATIC,
        "boolean",
        Some("0,1"),
        Some("0"),
        false,
    ),
    (
        "auto_increment_increment",
        APPLY_TYPE_DYNAMIC,
        "integer",
        Some("1-65535"),
        Some("1"),
        true,
    ),
    (
        "back_log",
        APPLY_TYPE_STATIC,
        "integer",
        Some("1-65535"),
        None,
        true,
    ),
    (
        // The installation directory the managed engine owns: it publishes neither a domain
        // nor a value for it, and does not let a caller modify it.
        "basedir",
        APPLY_TYPE_STATIC,
        "string",
        None,
        None,
        false,
    ),
    (
        "binlog_checksum",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("NONE,CRC32"),
        Some("CRC32"),
        true,
    ),
    (
        "binlog_format",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("ROW,STATEMENT,MIXED"),
        Some("MIXED"),
        true,
    ),
    (
        "binlog_row_image",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("full,minimal,noblob"),
        Some("full"),
        true,
    ),
];

/// One page of a family's defaults: the parameters and the `Marker` the answer carried.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    marker: Option<&str>,
) -> (Vec<Reported>, Option<String>) {
    let mut parameters = vec![
        ("DBParameterGroupFamily", family.to_string()),
        ("MaxRecords", PAGE.to_string()),
    ];
    if let Some(marker) = marker {
        parameters.push(("Marker", marker.to_string()));
    }
    let body = expect_ok(case, call(router, ACTION, &parameters).await);
    (reported(&body), optional_text(&body, "Marker"))
}

/// The names of a page, in the order they were reported.
fn names(rows: &[Reported]) -> Vec<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

/// The first page of every MySQL-vocabulary family is the first parameters of the release
/// line that family names, in ascending name order.
#[tokio::test]
async fn the_first_page_is_the_engines_first_published_defaults() {
    let router = emulator();

    for (family, line) in MYSQL_FAMILIES {
        let case = format!("family {family}");
        let (rows, marker) = page(&router, &case, family, None).await;
        assert_eq!(
            names(&rows),
            head(line),
            "case {case:?}: expected the first published defaults of the {line:?} release \
             line, got {:?}",
            names(&rows)
        );
        assert!(
            marker.is_some(),
            "case {case:?}: parameters are left after the first page, so a Marker is owed, \
             got {marker:?}"
        );

        // The page continues from the marker in the SAME order: the row after the page is
        // the next published parameter, and the two pages together stay strictly ascending.
        let (next, _) = page(&router, &case, family, marker.as_deref()).await;
        assert_eq!(
            names(&next).first().copied(),
            Some(after_head(line)),
            "case {case:?}: the page after the Marker continues the published order, got {:?}",
            names(&next)
        );
        let walked: Vec<&str> = names(&rows).into_iter().chain(names(&next)).collect();
        let mut ordered = walked.clone();
        ordered.sort_unstable();
        ordered.dedup();
        assert_eq!(
            walked, ordered,
            "case {case:?}: the walk over both pages must be ascending and report no \
             parameter twice, got {walked:?}"
        );

        // Every row of the page is the engine's own default, and only the parameters the
        // engine introduced inside the releases this family's line runs report the release
        // that introduced them.
        for row in &rows {
            let expected = INTRODUCED_IN
                .iter()
                .chain(INTRODUCED_IN_8_0.iter())
                .find(|(name, _)| *name == row.name)
                .map(|(_, version)| (*version).to_string());
            assert_eq!(
                row.minimum_engine_version, expected,
                "case {case:?}: {} must report MinimumEngineVersion {expected:?}, got {:?}",
                row.name, row.minimum_engine_version
            );
            assert_eq!(
                row.source.as_deref(),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                "case {case:?}: {} is an engine default, got Source {:?}",
                row.name,
                row.source
            );
        }

        // A parameter the engine introduced in the 8.0 line is published by a family of
        // that line and by no earlier one, and one it DROPPED there by every family but that
        // line's: the family's own release decides both, and a family whose engine numbers
        // its own releases carries them all.
        let walked_names = walked.clone();
        for (introduced, _) in INTRODUCED_IN_8_0 {
            let carried = walked_names.contains(&introduced);
            assert_eq!(
                carried,
                line != Line::Mysql5_7,
                "case {case:?}: {introduced} arrived in the engine's 8.0 release line, so \
                 only a family of that line or a later one publishes it, got {walked_names:?}"
            );
        }
        for (removed, release) in REMOVED_IN_8_4 {
            let carried = walked_names.contains(&removed);
            assert_eq!(
                carried,
                line != Line::Mysql8_4,
                "case {case:?}: {removed} was dropped from the engine at {release}, so only \
                 a family outside that line publishes it, got {walked_names:?}"
            );
        }
    }
}

/// Each named parameter reports the whole record its engine publishes for it, and a
/// parameter the engine autosizes reports no `ParameterValue` at all rather than an empty
/// one.
#[tokio::test]
async fn each_published_parameter_reports_its_own_declared_record() {
    let router = emulator();

    for (family, _) in MYSQL_FAMILIES {
        let case = format!("family {family}");
        // The named rows fall inside the published head, which the 8.0 line's own
        // additions push over one page boundary, so both pages of it are read.
        let (first, marker) = page(&router, &case, family, None).await;
        let (second, _) = page(&router, &case, family, marker.as_deref()).await;
        let rows: Vec<Reported> = first.into_iter().chain(second).collect();
        for (name, apply_type, data_type, allowed_values, value, is_modifiable) in DECLARED {
            let row = rows
                .iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("case {case:?}: expected {name} in the published \
                                           head"));
            let found = (
                row.apply_type.as_deref(),
                row.data_type.as_deref(),
                row.allowed_values.as_deref(),
                row.value.as_deref(),
                row.is_modifiable.as_deref(),
            );
            let expected = (
                Some(apply_type),
                Some(data_type),
                allowed_values,
                value,
                Some(if is_modifiable { "true" } else { "false" }),
            );
            assert_eq!(
                found, expected,
                "case {case:?}: {name} must report the record its engine publishes, got \
                 {found:?}"
            );
            assert!(
                row.description.as_ref().is_some_and(|text| !text.is_empty()),
                "case {case:?}: {name} must report its Description, got {:?}",
                row.description
            );
        }
    }
}
