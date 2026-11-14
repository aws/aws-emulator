// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a paginated report resumes: the position its `Marker` names.
//!
//! RULE: a `Marker` is an opaque token naming the LAST record the page that issued it
//! served, and a request carrying one is answered with the records BEYOND that position in
//! the report's own order ("the response includes only records beyond the marker",
//! `DescribeDBParametersMessage$Marker`, and the same sentence on every `Marker` this front
//! door serves). So the token is PLACED in that order rather than required to still BE a
//! record of it: a page that was served before a record was removed still resumes at the
//! records after it. Only a token this service's codec could not have produced at all names
//! no position, and that is the one refusal — the modelled errors of these reports carry no
//! other refusal for a well-formed token.
//!
//! The rule is asserted over TABLES spanning what it generalises over:
//!
//! * the report: a group's own stored parameters, the account's stored parameter groups and
//!   a family's published engine defaults — one report of records a caller wrote, one of
//!   records this service provides and one of published reference data, so the rule is not
//!   read as a property of one source;
//! * the page size: both bounds `MaxRecords` publishes ("Minimum 20, maximum 100");
//! * where in the report the token points: its first record, its second, the middle, the
//!   one before last, the last, a key BETWEEN two records, and a key past every record;
//! * what happened to the record the token names: still there, or dropped between the two
//!   calls;
//! * the shape of a token that names no position (punctuation, a word, one character,
//!   encoded bytes that are no key).
//!
//! The second rule here is the one the same projection carries for a member AWS reports only
//! where it has a value: a parameter's `MinimumEngineVersion` is reported where its family's
//! published catalog names the version the parameter was introduced in, and is ABSENT — not
//! empty — where it names none.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    encode_marker, invalid_marker_message, invalid_parameter_marker_message,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, texts};
use crate::engine_default_parameters::reported;

/// The error code Amazon RDS answers a `Marker` it cannot place with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The bounds `MaxRecords` publishes: "Constraints: Minimum 20, maximum 100."
const PAGE_SIZES: [usize; 2] = [20, 100];

/// How many parameters the stored-parameter report below holds: more than the smallest page,
/// so that report pages.
const STORED_PARAMETERS: usize = 25;

/// How many groups the stored-group report below holds beyond the ones RDS provides itself.
const STORED_GROUPS: usize = 25;

/// One report under test: how to ask for it, and which element each entry's key is read from.
struct Report {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    key_element: &'static str,
    /// Whether this report is one of the PARAMETER reports, whose invalid-marker refusal
    /// echoes the token the request carried instead of answering the fixed sentence.
    echoes_the_marker: bool,
}

/// One call of `report`, at `page_size`, resuming from `marker` when there is one: the keys
/// it served and the `Marker` it answered with.
async fn page_of(
    router: &MultiServiceRouter,
    case: &str,
    report: &Report,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut params = report.params.clone();
    params.push(("MaxRecords".to_string(), page_size.to_string()));
    if let Some(marker) = marker {
        params.push(("Marker".to_string(), marker.to_string()));
    }
    let body = expect_ok(case, call(router, report.action, &params).await);
    (
        texts(&body, report.key_element),
        optional_text(&body, "Marker"),
    )
}

/// Every page of `report` at `page_size`, in the order they were served.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    report: &Report,
    page_size: usize,
) -> Vec<(Vec<String>, Option<String>)> {
    let mut pages: Vec<(Vec<String>, Option<String>)> = Vec::new();
    let mut marker: Option<String> = None;
    // The longest report under test is far inside this many pages at the smallest page size;
    // the cap is what keeps a marker that never clears from hanging the suite.
    for _ in 0..16 {
        let page = page_of(router, case, report, page_size, marker.as_deref()).await;
        marker = page.1.clone();
        pages.push(page);
        if marker.is_none() {
            return pages;
        }
    }
    panic!("case {case:?}: the report never stopped handing out a Marker");
}

/// The whole of `report`, in the order it serves it, read by walking its pages.
async fn whole(router: &MultiServiceRouter, case: &str, report: &Report) -> Vec<String> {
    walk(router, case, report, PAGE_SIZES[0])
        .await
        .into_iter()
        .flat_map(|(keys, _)| keys)
        .collect()
}

/// Create a DB parameter group of `family` and answer the name the store keys it under.
async fn create_group(router: &MultiServiceRouter, name: &str, family: &str) -> String {
    expect_ok(
        name,
        call(
            router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "a paged group"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// Write `settings` into `group`, in requests no longer than the documented maximum of 20
/// parameters per modify.
async fn set_parameters(router: &MultiServiceRouter, group: &str, settings: &[(String, String)]) {
    for chunk in settings.chunks(20) {
        let mut params: Vec<(String, String)> =
            vec![("DBParameterGroupName".to_string(), group.to_string())];
        for (index, (name, value)) in chunk.iter().enumerate() {
            let position = index + 1;
            params.push((
                format!("Parameters.Parameter.{position}.ParameterName"),
                name.clone(),
            ));
            params.push((
                format!("Parameters.Parameter.{position}.ParameterValue"),
                value.clone(),
            ));
        }
        expect_ok(
            "filling the group",
            call(router, "ModifyDBParameterGroup", &params).await,
        );
    }
}

/// Drop the settings `names` from `group`, which is what returns those parameters to the
/// engine's own default and takes them out of the group's own report.
async fn reset_parameters(router: &MultiServiceRouter, group: &str, names: &[&str]) {
    let mut params: Vec<(String, String)> =
        vec![("DBParameterGroupName".to_string(), group.to_string())];
    for (index, name) in names.iter().enumerate() {
        let position = index + 1;
        params.push((
            format!("Parameters.Parameter.{position}.ParameterName"),
            (*name).to_string(),
        ));
    }
    expect_ok(
        "resetting parameters",
        call(router, "ResetDBParameterGroup", &params).await,
    );
}

/// The names the stored-parameter report below holds, in the order that report serves them.
fn stored_parameter_names() -> Vec<String> {
    (0..STORED_PARAMETERS)
        .map(|index| format!("p_{index:03}"))
        .collect()
}

/// An emulator holding the three reports the tables below page, and those reports.
///
/// The parameter group is of a family this service publishes NO catalog for, so that
/// report holds exactly the parameters the caller wrote — which is what lets the expected
/// order be named rather than guessed.
async fn seeded() -> (MultiServiceRouter, Vec<Report>) {
    let router = emulator();
    let group = create_group(&router, "emu-marker-pg", "db2-se").await;
    let settings: Vec<(String, String)> = stored_parameter_names()
        .into_iter()
        .map(|name| (name, "1".to_string()))
        .collect();
    set_parameters(&router, &group, &settings).await;
    for index in 0..STORED_GROUPS {
        create_group(&router, &format!("emu-marker-group-{index:03}"), "mysql8.0").await;
    }

    let reports = vec![
        Report {
            case: "a group's own stored parameters",
            action: "DescribeDBParameters",
            params: vec![("DBParameterGroupName".to_string(), group.clone())],
            key_element: "ParameterName",
            echoes_the_marker: true,
        },
        Report {
            case: "the account's stored parameter groups",
            action: "DescribeDBParameterGroups",
            params: Vec::new(),
            key_element: "DBParameterGroupName",
            echoes_the_marker: false,
        },
        Report {
            case: "a family's published engine defaults",
            action: "DescribeEngineDefaultParameters",
            params: vec![("DBParameterGroupFamily".to_string(), "mysql8.0".to_string())],
            key_element: "ParameterName",
            echoes_the_marker: true,
        },
    ];
    (router, reports)
}

#[tokio::test]
async fn a_marker_names_the_last_record_its_own_page_served() {
    let (router, reports) = seeded().await;

    for report in &reports {
        let order = whole(&router, report.case, report).await;
        assert!(
            order.len() > PAGE_SIZES[0],
            "case {:?}: this table needs a report longer than one page of {}, got {}",
            report.case,
            PAGE_SIZES[0],
            order.len()
        );
        for page_size in PAGE_SIZES {
            let case = format!("{} paged at {page_size}", report.case);
            let pages = walk(&router, &case, report, page_size).await;
            let mut served: Vec<String> = Vec::new();
            for (index, (keys, marker)) in pages.iter().enumerate() {
                let last_page = index + 1 == pages.len();
                assert!(
                    keys.len() <= page_size,
                    "case {case:?}: page {index} served {} records, more than the page size, \
                     got {keys:?}",
                    keys.len()
                );
                let expected = if last_page {
                    None
                } else {
                    keys.last().map(|key| encode_marker(key))
                };
                assert_eq!(
                    *marker, expected,
                    "case {case:?}: page {index} must carry the token for the LAST record it \
                     served, got {marker:?}"
                );
                served.extend(keys.clone());
            }
            assert_eq!(
                served, order,
                "case {case:?}: expected every record once, in the report's own order, got \
                 {served:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_is_answered_with_the_records_beyond_the_token_it_carries() {
    let (router, reports) = seeded().await;
    let page_size = PAGE_SIZES[1];

    for report in &reports {
        let order = whole(&router, report.case, report).await;
        let last = order.len() - 1;
        // Where in the report the token points. The rows span both ends and the middle of it,
        // plus a key that is no record at all: `~` sorts after every character the keys of
        // these reports use, so a token of `~~~` lies past the whole report.
        let rows: Vec<(String, String, Vec<String>)> = vec![
            (
                "the first record".to_string(),
                order[0].clone(),
                order[1..].to_vec(),
            ),
            (
                "the second record".to_string(),
                order[1].clone(),
                order[2..].to_vec(),
            ),
            (
                "the middle record".to_string(),
                order[last / 2].clone(),
                order[last / 2 + 1..].to_vec(),
            ),
            (
                "the record before last".to_string(),
                order[last - 1].clone(),
                order[last..].to_vec(),
            ),
            ("the last record".to_string(), order[last].clone(), Vec::new()),
            ("a key past every record".to_string(), "~~~".to_string(), Vec::new()),
        ];

        for (position, key, beyond) in rows {
            let case = format!("{} resumed from {position}", report.case);
            let expected: Vec<String> = beyond.iter().take(page_size).cloned().collect();
            let (served, marker) =
                page_of(&router, &case, report, page_size, Some(&encode_marker(&key))).await;
            assert_eq!(
                served, expected,
                "case {case:?}: expected the records beyond it, got {served:?}"
            );
            let due = (expected.len() < beyond.len())
                .then(|| expected.last().map(|key| encode_marker(key)))
                .flatten();
            assert_eq!(
                marker, due,
                "case {case:?}: a Marker is due exactly while records are left, got {marker:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_for_a_key_between_two_records_resumes_at_the_one_after_it() {
    let (router, reports) = seeded().await;
    let report = &reports[0];
    let names = stored_parameter_names();
    let page_size = PAGE_SIZES[1];

    // This report's keys are `p_000`..`p_0NN`, so appending a character that sorts after a
    // digit names a key strictly between two of its records: no record holds it, and the
    // position it names is the one after the key it extends.
    for row in [0usize, STORED_PARAMETERS / 2, STORED_PARAMETERS - 1] {
        let key = format!("{}~", names[row]);
        let case = format!("a key between {} and the record after it", names[row]);
        let expected: Vec<String> = names[row + 1..].to_vec();
        let (served, _) =
            page_of(&router, &case, report, page_size, Some(&encode_marker(&key))).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected the records beyond that position, got {served:?}"
        );
    }
}

#[tokio::test]
async fn a_token_for_a_record_the_report_no_longer_holds_resumes_beyond_it() {
    let (router, reports) = seeded().await;
    let report = &reports[0];
    let names = stored_parameter_names();
    let page_size = PAGE_SIZES[0];

    let (first, marker) = page_of(&router, report.case, report, page_size, None).await;
    assert_eq!(
        first,
        names[..page_size].to_vec(),
        "case {:?}: expected the first page in the report's own order, got {first:?}",
        report.case
    );
    let marker = marker.unwrap_or_else(|| {
        panic!(
            "case {:?}: a page that leaves records behind hands out a Marker",
            report.case
        )
    });

    // What happened to the record the token names, between the two calls. Every row holds a
    // token the first page above issued; the rows differ in whether that record, and the ones
    // around it, are still in the report.
    let rows: [(&str, &[&str], usize); 3] = [
        ("still in the report", &[], page_size),
        ("dropped between the two calls", &["p_019"], page_size),
        (
            "dropped along with records before it",
            &["p_000", "p_005"],
            page_size,
        ),
    ];
    for (case, dropped, beyond_from) in rows {
        if !dropped.is_empty() {
            reset_parameters(&router, "emu-marker-pg", dropped).await;
        }
        let expected: Vec<String> = names[beyond_from..].to_vec();
        let (served, next) =
            page_of(&router, case, report, page_size, Some(&marker)).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected the records beyond the token's position, got {served:?}"
        );
        assert_eq!(
            next, None,
            "case {case:?}: the page that serves the tail owes no Marker, got {next:?}"
        );
    }
}

#[tokio::test]
async fn only_a_token_this_codec_could_not_have_produced_is_refused() {
    let (router, reports) = seeded().await;

    // The rows differ along the SHAPE of the token: punctuation, a word, one character, and
    // characters the codec reads whose bytes are no key of anything.
    for token in ["!!!!", "not-a-token", "m", "/////w=="] {
        for report in &reports {
            let case = format!("{} resumed from {token:?}", report.case);
            let mut params = report.params.clone();
            params.push(("Marker".to_string(), token.to_string()));
            let body = expect_error(
                &case,
                call(&router, report.action, &params).await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = optional_text(&body, "Message");
            let expected = if report.echoes_the_marker {
                invalid_parameter_marker_message(token)
            } else {
                invalid_marker_message()
            };
            assert_eq!(
                message,
                Some(expected.clone()),
                "case {case:?}: expected {expected:?} for a token it could not place, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_parameters_minimum_engine_version_is_reported_only_where_its_family_publishes_one() {
    let router = emulator();

    // `(case, family, parameter, value, the MinimumEngineVersion the report must carry)`. The
    // rows span a published parameter whose catalog entry names the version it appeared in, a
    // published parameter whose entry names none, and a parameter of a family this service
    // publishes no catalog for at all. The expected versions are the catalog's own
    // (`services/common/src/rds_cluster_parameters.rs`, the Amazon DocumentDB cluster
    // parameters), including `ttl_monitor`, which no issue cited.
    let rows: [(&str, &str, &str, &str, Option<&str>); 4] = [
        (
            "a published parameter whose entry names a version",
            "docdb5.0",
            "profiler",
            "enabled",
            Some("4.0.0"),
        ),
        (
            "another published parameter, of an earlier version",
            "docdb5.0",
            "ttl_monitor",
            "disabled",
            Some("3.6.0"),
        ),
        (
            "a published parameter whose entry names none",
            "mysql8.0",
            "autocommit",
            "0",
            None,
        ),
        (
            "a parameter of a family publishing no catalog",
            "db2-se",
            "unpublished_parameter",
            "7",
            None,
        ),
    ];

    for (row, (case, family, parameter, value, expected)) in rows.iter().enumerate() {
        let group = create_group(
            &router,
            &format!("emu-min-version-{row}"),
            family,
        )
        .await;
        set_parameters(
            &router,
            &group,
            &[((*parameter).to_string(), (*value).to_string())],
        )
        .await;
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameters",
                &[
                    ("DBParameterGroupName", group.as_str()),
                    ("Source", "user"),
                ],
            )
            .await,
        );
        let entry = reported(&body)
            .into_iter()
            .find(|entry| entry.name == *parameter)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the report to carry {parameter}, got {body}")
            });
        assert_eq!(
            entry.minimum_engine_version.as_deref(),
            *expected,
            "case {case:?}: expected MinimumEngineVersion {expected:?}, got {:?}",
            entry.minimum_engine_version
        );
    }
}
