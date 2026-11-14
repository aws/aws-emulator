// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDBMajorEngineVersions`, the major-version report.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (`crate::db_instances::emulator`), each
//! request is a real awsQuery form POST carrying the SigV4 credential scope the router
//! keys on, and every assertion reads the XML the emulator answered with.
//!
//! The rules under test are stated over TABLES and over the service's OWN engine-version
//! report rather than over one recorded request:
//!
//! 1. The report is the distinct `(Engine, MajorEngineVersion)` pairs of the engine
//!    version catalogue `DescribeDBEngineVersions` publishes, each once, in that
//!    catalogue's order — so the expectation is READ from that report rather than
//!    hand-listed, and every engine it carries is a row.
//! 2. `Engine` and `MajorEngineVersion` narrow that report and refuse nothing: the table
//!    spans an engine alone, a major version alone, both together, a name in another
//!    case, and three values that select nothing (an engine whose versions only an
//!    account mints, an engine of another control plane, and a major version no entry
//!    carries).
//! 3. A support lifecycle is published only for the open-source engines, with Amazon RDS
//!    Extended Support only for the four that are offered it; a window covers whole UTC
//!    days — it opens at the first instant of its start day and closes at the LAST instant
//!    of its end day — standard support opens before it closes, and Extended Support opens
//!    at the first instant of the day after the day standard support closes on and runs the
//!    three years AWS offers it for, except for the families whose published window AWS has
//!    moved out, which are named and asserted as literal windows.
//! 4. `MaxRecords` holds its published bounds, the `Marker` walks every entry exactly
//!    once, and a `Marker` no prior page issued is refused.
//! 5. The report is a READ: two identical calls answer identically.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::walk_every_version;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text,
};

/// The operation under test.
const ACTION: &str = "DescribeDBMajorEngineVersions";

/// The engines a support lifecycle is published for: `SupportedEngineLifecycle` "only
/// returns information for the open source engines Amazon RDS for MariaDB, Amazon RDS for
/// MySQL, Amazon RDS for PostgreSQL, Aurora MySQL, and Aurora PostgreSQL"
/// (`smithy_query(query="doc",shape="SupportedEngineLifecycle")`).
const OPEN_SOURCE_ENGINES: [&str; 5] = [
    "aurora-mysql",
    "aurora-postgresql",
    "mariadb",
    "mysql",
    "postgres",
];

/// The one open-source engine offered no Amazon RDS Extended Support:
/// `LifecycleSupportName` states that "for Amazon RDS for MariaDB, this parameter only
/// returns the value `open-source-rds-standard-support`".
const STANDARD_SUPPORT_ONLY_ENGINE: &str = "mariadb";

/// The engines this one `rds` front door serves on behalf of ANOTHER control plane, whose
/// models declare no `DescribeDBMajorEngineVersions` at all — so this report carries none
/// of their families even though the engine-version report carries their versions.
const OTHER_CONTROL_PLANE_ENGINES: [&str; 2] = ["docdb", "neptune"];

/// The two values `LifecycleSupportName` publishes
/// (`smithy_query(query="shape",shape="LifecycleSupportName")`).
const STANDARD_SUPPORT: &str = "open-source-rds-standard-support";
const EXTENDED_SUPPORT: &str = "open-source-rds-extended-support";

/// Seconds in one day, which the published calendar is written in.
const DAY: i64 = 24 * 60 * 60;

/// The days in the three years Amazon RDS Extended Support runs for past the end of
/// standard support: three calendar years are 1095 days, or 1096 when a 29 February falls
/// inside them. Every family's window is that long, except the ones
/// [`EXTENDED_PAST_THREE_YEARS`] names, whose published end date AWS has moved out.
const EXTENDED_SUPPORT_DAYS: std::ops::RangeInclusive<i64> = 1095..=1096;

/// The families whose published Extended Support window runs LONGER than the three years the
/// offering's floor is, as `(Engine, MajorEngineVersion)`. Each one's two windows are
/// asserted as literals in [`PUBLISHED_WINDOWS`], so the exception is pinned rather than
/// merely tolerated.
const EXTENDED_PAST_THREE_YEARS: [(&str, &str); 1] = [("aurora-mysql", "5.7")];

/// The time of day the last instant of a published support day falls at: a window closes
/// when the whole of its published end day is over, so its end bound is that day's last
/// instant rather than the midnight that opens it.
const END_OF_DAY: &str = "23:59:59.999Z";

/// One reported `<SupportedEngineLifecycle>`.
#[derive(Debug, Clone)]
struct Lifecycle {
    name: String,
    start: String,
    end: String,
}

/// One reported `<DBMajorEngineVersion>`.
#[derive(Debug, Clone)]
struct Family {
    engine: String,
    major_engine_version: String,
    lifecycles: Vec<Lifecycle>,
}

impl Family {
    /// The `(Engine, MajorEngineVersion)` pair this entry names.
    fn pair(&self) -> (String, String) {
        (self.engine.clone(), self.major_engine_version.clone())
    }
}

/// Every `<DBMajorEngineVersion>` in a response body, in the order it was reported.
///
/// A lifecycle window carries no `<Engine>` or `<MajorEngineVersion>` of its own, so the
/// two member reads over the whole entry are unambiguous.
fn reported(xml: &str) -> Vec<Family> {
    subtrees(xml, "DBMajorEngineVersion")
        .into_iter()
        .map(|entry| Family {
            engine: text(&entry, "Engine"),
            major_engine_version: text(&entry, "MajorEngineVersion"),
            lifecycles: subtrees(&entry, "SupportedEngineLifecycle")
                .into_iter()
                .map(|window| Lifecycle {
                    name: text(&window, "LifecycleSupportName"),
                    start: text(&window, "LifecycleSupportStartDate"),
                    end: text(&window, "LifecycleSupportEndDate"),
                })
                .collect(),
        })
        .collect()
}

/// One `DescribeDBMajorEngineVersions` call: the families it reported and the `Marker` it
/// carried.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, String)],
) -> (Vec<Family>, Option<String>) {
    let body = expect_ok(case, call(router, ACTION, params).await);
    (reported(&body), optional_text(&body, "Marker"))
}

/// The whole report, walked one page of `page_size` at a time: a page the report issued a
/// `Marker` for carries entries, no page exceeds `MaxRecords`, and the walk terminates.
async fn walk(router: &MultiServiceRouter, case: &str, page_size: usize) -> Vec<Family> {
    let mut seen: Vec<Family> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params: Vec<(&str, String)> = vec![("MaxRecords", page_size.to_string())];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let (page, next) = describe(router, case, &params).await;
        assert!(
            !page.is_empty(),
            "case {case:?}: a page the report issued a Marker for must carry entries"
        );
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page must hold at most MaxRecords={page_size} entries, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages < 500,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    seen
}

/// The `(Engine, MajorEngineVersion)` pairs the ENGINE-VERSION report publishes, deduped
/// in its own order and without the engines of another control plane: the expectation this
/// report owes, read from the service rather than hand-listed.
async fn catalogued_families(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for version in walk_every_version(router, "every engine version", 20).await {
        if OTHER_CONTROL_PLANE_ENGINES.contains(&version.engine.as_str()) {
            continue;
        }
        let pair = (version.engine.clone(), version.major_engine_version.clone());
        if !found.contains(&pair) {
            found.push(pair);
        }
    }
    found
}

/// A reported timestamp as epoch seconds, which is what the window arithmetic below is
/// stated in.
fn instant(case: &str, member: &str, value: &str) -> i64 {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| {
            panic!("case {case:?}: {member} must be an ISO-8601 instant, got {value:?}")
        })
        .secs()
}

#[tokio::test]
async fn the_report_is_the_catalogues_major_version_families() {
    let router = emulator();

    let expected = catalogued_families(&router).await;
    assert!(
        expected.len() > 20,
        "case \"every major engine version\": the catalogue must publish more families \
         than one page holds, so paging is exercised, got {}",
        expected.len()
    );

    let seen: Vec<(String, String)> = walk(&router, "every major engine version", 20)
        .await
        .iter()
        .map(Family::pair)
        .collect();
    assert_eq!(
        seen, expected,
        "case \"every major engine version\": the report must carry each \
         (Engine, MajorEngineVersion) pair of the engine-version catalogue exactly once, \
         in that catalogue's order"
    );

    // The engines of another control plane carry versions in the engine-version report and
    // no family here; asserted directly so the exclusion is not read out of `expected`
    // alone.
    for engine in OTHER_CONTROL_PLANE_ENGINES {
        assert!(
            !seen.iter().any(|(reported, _)| reported == engine),
            "case {engine:?}: an engine of another control plane must report no major \
             engine version, got {seen:?}"
        );
    }
}

/// One narrowing request: the members it carries, and whether it must select anything.
struct Narrowing {
    case: &'static str,
    engine: Option<&'static str>,
    major_engine_version: Option<&'static str>,
    /// Whether this request must select at least one family. A row that selects none says
    /// so here, which is what keeps an empty answer from passing a row meant to narrow.
    selects: bool,
}

/// The narrowing table: each member alone, both together, another case, and the values
/// that select nothing.
const NARROWINGS: &[Narrowing] = &[
    Narrowing {
        case: "every family of one engine",
        engine: Some("mysql"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "one family of one engine",
        engine: Some("postgres"),
        major_engine_version: Some("16"),
        selects: true,
    },
    Narrowing {
        case: "an engine offered standard support only",
        engine: Some("mariadb"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "an Oracle multi-tenant family",
        engine: Some("oracle-ee-cdb"),
        major_engine_version: Some("21"),
        selects: true,
    },
    Narrowing {
        case: "a SQL Server family, whose major version is a release",
        engine: Some("sqlserver-ee"),
        major_engine_version: Some("14.00"),
        selects: true,
    },
    Narrowing {
        case: "a Db2 edition",
        engine: Some("db2-ae"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "an Aurora engine, whose families are cluster engines",
        engine: Some("aurora-postgresql"),
        major_engine_version: Some("15"),
        selects: true,
    },
    Narrowing {
        case: "an engine named in another case",
        engine: Some("MySQL"),
        major_engine_version: Some("8.0"),
        selects: true,
    },
    Narrowing {
        case: "a major version alone, which several engines publish",
        engine: None,
        major_engine_version: Some("19"),
        selects: true,
    },
    Narrowing {
        case: "an engine whose versions only an account mints",
        engine: Some("custom-oracle-ee"),
        major_engine_version: None,
        selects: false,
    },
    Narrowing {
        case: "an engine of another control plane",
        engine: Some("docdb"),
        major_engine_version: None,
        selects: false,
    },
    Narrowing {
        case: "a major version no entry of that engine carries",
        engine: Some("mysql"),
        major_engine_version: Some("9.9"),
        selects: false,
    },
    Narrowing {
        case: "a value that is not an engine",
        engine: Some("not-an-engine"),
        major_engine_version: None,
        selects: false,
    },
];

#[tokio::test]
async fn engine_and_major_engine_version_narrow_the_report() {
    let router = emulator();
    let whole = walk(&router, "every major engine version", 100).await;

    for row in NARROWINGS {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(engine) = row.engine {
            params.push(("Engine", engine.to_string()));
        }
        if let Some(major) = row.major_engine_version {
            params.push(("MajorEngineVersion", major.to_string()));
        }
        // The expectation is the whole report under the same rule: the entries whose
        // members match the values the request named, case-insensitively, in the report's
        // own order.
        let expected: Vec<(String, String)> = whole
            .iter()
            .filter(|family| {
                row.engine
                    .is_none_or(|engine| family.engine.eq_ignore_ascii_case(engine))
                    && row.major_engine_version.is_none_or(|major| {
                        family.major_engine_version.eq_ignore_ascii_case(major)
                    })
            })
            .map(Family::pair)
            .collect();
        assert_eq!(
            !expected.is_empty(),
            row.selects,
            "case {:?}: the table says this request selects {}, but the whole report \
             matches {expected:?}",
            row.case,
            row.selects
        );

        let (page, marker) = describe(&router, row.case, &params).await;
        let seen: Vec<(String, String)> = page.iter().map(Family::pair).collect();
        assert_eq!(
            seen, expected,
            "case {:?}: the narrowed report must be the whole report's matching entries, \
             in its order",
            row.case
        );
        assert_eq!(
            marker, None,
            "case {:?}: a narrowed report that fits in one page must carry no Marker, got \
             {marker:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_open_source_family_reports_its_support_lifecycles() {
    let router = emulator();

    for family in walk(&router, "every major engine version", 100).await {
        let case = format!("{} {}", family.engine, family.major_engine_version);
        let names: Vec<&str> = family
            .lifecycles
            .iter()
            .map(|window| window.name.as_str())
            .collect();
        let open_source = OPEN_SOURCE_ENGINES.contains(&family.engine.as_str());
        let expected_names: Vec<&str> = if !open_source {
            Vec::new()
        } else if family.engine == STANDARD_SUPPORT_ONLY_ENGINE {
            vec![STANDARD_SUPPORT]
        } else {
            vec![STANDARD_SUPPORT, EXTENDED_SUPPORT]
        };
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the lifecycle names {expected_names:?}, got {names:?}"
        );

        for window in &family.lifecycles {
            let start = instant(&case, "LifecycleSupportStartDate", &window.start);
            let end = instant(&case, "LifecycleSupportEndDate", &window.end);
            // AWS publishes a support calendar in DAYS, so a window covers whole UTC days:
            // it opens at the first instant of its start day and closes at the last instant
            // of its end day, which is what keeps the day a window closes on inside it.
            assert_eq!(
                start % DAY,
                0,
                "case {case:?}: LifecycleSupportStartDate must open a UTC day, got {:?}",
                window.start
            );
            assert!(
                window.end.ends_with(END_OF_DAY),
                "case {case:?}: LifecycleSupportEndDate must be the last instant of its \
                 published day (ending {END_OF_DAY}), got {:?}",
                window.end
            );
            assert_eq!(
                (end + 1) % DAY,
                0,
                "case {case:?}: LifecycleSupportEndDate must close a UTC day, got {:?}",
                window.end
            );
            assert!(
                start < end,
                "case {case:?}: the {} window must open before it closes, got {:?} and {:?}",
                window.name,
                window.start,
                window.end
            );
        }

        if !open_source {
            continue;
        }
        let standard = family
            .lifecycles
            .iter()
            .find(|window| window.name == STANDARD_SUPPORT)
            .unwrap_or_else(|| panic!("case {case:?}: checked just above"));
        let standard_end = instant(&case, "LifecycleSupportEndDate", &standard.end);
        if let Some(extended) = family
            .lifecycles
            .iter()
            .find(|window| window.name == EXTENDED_SUPPORT)
        {
            let extended_start = instant(&case, "LifecycleSupportStartDate", &extended.start);
            assert_eq!(
                extended_start,
                standard_end + 1,
                "case {case:?}: Extended Support must open at the first instant of the day \
                 after the day standard support closes on, got {:?} against {:?}",
                extended.start,
                standard.end
            );
            let extended_end = instant(&case, "LifecycleSupportEndDate", &extended.end);
            let days = (extended_end - standard_end) / DAY;
            // A family whose published window AWS moved out is named in the exception table
            // and asserted as a literal window; every other family's runs the three years
            // the offering is published as.
            let moved_out = EXTENDED_PAST_THREE_YEARS.contains(&(
                family.engine.as_str(),
                family.major_engine_version.as_str(),
            ));
            if moved_out {
                assert!(
                    days > *EXTENDED_SUPPORT_DAYS.end(),
                    "case {case:?}: this family is named as one whose Extended Support window \
                     AWS moved out, so it must run past {EXTENDED_SUPPORT_DAYS:?} days, got \
                     {days} between {:?} and {:?}",
                    standard.end,
                    extended.end
                );
                assert!(
                    PUBLISHED_WINDOWS.iter().any(|(engine, major, _, _, _)| {
                        *engine == family.engine && *major == family.major_engine_version
                    }),
                    "case {case:?}: a family excepted from the three-year window must have \
                     its published windows asserted as literals"
                );
            } else {
                assert!(
                    EXTENDED_SUPPORT_DAYS.contains(&days),
                    "case {case:?}: Extended Support must run the three years past the end of \
                     standard support ({EXTENDED_SUPPORT_DAYS:?} days), got {days} between \
                     {:?} and {:?}",
                    standard.end,
                    extended.end
                );
            }
        }
    }
}

#[tokio::test]
async fn max_records_is_bounded_and_the_marker_resumes_the_report() {
    let router = emulator();

    // "The maximum number of records to include in the response … Minimum 20, maximum
    // 100" — each published bound, and one step past each. The bound is a MODELLED range
    // on this operation's `MaxRecords`, so a value past it is refused at the boundary,
    // before this handler runs — under the same `InvalidParameterValue` the reports that
    // carry the bound in prose answer it with.
    for (case, value, status) in [
        ("one below the published minimum", "19", 400),
        ("the published minimum", "20", 200),
        ("the published maximum", "100", 200),
        ("one above the published maximum", "101", 400),
    ] {
        let response = call(&router, ACTION, &[("MaxRecords", value.to_string())]).await;
        if status == 200 {
            expect_ok(case, response);
        } else {
            let body = expect_error(case, response, status, "InvalidParameterValue");
            assert!(
                text(&body, "Message").contains("maxRecords"),
                "case {case:?}: the refusal must name the member it refused, got {body}"
            );
        }
    }

    // A `Marker` no prior page issued is refused rather than resynchronised onto a page
    // the caller cannot place.
    expect_error(
        "a Marker no prior page issued",
        call(
            &router,
            ACTION,
            &[("Marker", "not-a-marker-this-report-issued".to_string())],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // Paging at the smallest page size the operation accepts visits every entry exactly
    // once, in the same order as the unpaged report.
    let (whole, marker) = describe(&router, "the default page", &[]).await;
    assert_eq!(
        marker, None,
        "case \"the default page\": the whole report must fit in the default page size, \
         got the Marker {marker:?}"
    );
    let expected: Vec<(String, String)> = whole.iter().map(Family::pair).collect();
    let paged: Vec<(String, String)> = walk(&router, "the paged report", 20)
        .await
        .iter()
        .map(Family::pair)
        .collect();
    assert_eq!(
        paged, expected,
        "case \"the paged report\": paging must visit every entry exactly once, in the \
         report's own order"
    );
}

/// One family's published support windows: the engine, its major version, the standard
/// window's two bounds, and the Extended Support window's two bounds where that engine is
/// offered it.
type PublishedWindows = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);

/// The windows this service publishes for four named families, written out as LITERALS from
/// the published major version release calendars, so a drift in the calendar fails this test
/// rather than moving with it.
///
/// The rows span the engines the lifecycles differ by: an Aurora family whose Extended
/// Support runs past the three-year floor, an RDS family at the floor, the one engine
/// offered no Extended Support at all, and a PostgreSQL family — including families no
/// issue cited. Every bound is the instant its published DAY bounds: the first instant of a
/// day a window opens on and the LAST instant of the day it closes on.
const PUBLISHED_WINDOWS: [PublishedWindows; 4] = [
    (
        "aurora-mysql",
        "5.7",
        "2018-02-06T00:00:00Z",
        "2024-10-31T23:59:59.999Z",
        Some(("2024-11-01T00:00:00Z", "2029-06-30T23:59:59.999Z")),
    ),
    (
        "mysql",
        "8.0",
        "2018-10-23T00:00:00Z",
        "2026-07-31T23:59:59.999Z",
        Some(("2026-08-01T00:00:00Z", "2029-07-31T23:59:59.999Z")),
    ),
    (
        "mariadb",
        "10.11",
        "2023-05-24T00:00:00Z",
        "2028-02-29T23:59:59.999Z",
        None,
    ),
    (
        "postgres",
        "17",
        "2024-11-14T00:00:00Z",
        "2029-11-30T23:59:59.999Z",
        Some(("2029-12-01T00:00:00Z", "2032-11-30T23:59:59.999Z")),
    ),
];

#[tokio::test]
async fn a_named_family_reports_its_published_support_windows() {
    let router = emulator();

    for (engine, major, standard_start, standard_end, extended) in PUBLISHED_WINDOWS {
        let case = format!("{engine} {major}");
        let (families, _) = describe(
            &router,
            &case,
            &[
                ("Engine", engine.to_string()),
                ("MajorEngineVersion", major.to_string()),
            ],
        )
        .await;
        let family = match families.as_slice() {
            [family] => family.clone(),
            other => panic!("case {case:?}: expected exactly this family, got {other:?}"),
        };
        let reported: Vec<(&str, &str, &str)> = family
            .lifecycles
            .iter()
            .map(|window| {
                (
                    window.name.as_str(),
                    window.start.as_str(),
                    window.end.as_str(),
                )
            })
            .collect();
        let mut expected: Vec<(&str, &str, &str)> =
            vec![(STANDARD_SUPPORT, standard_start, standard_end)];
        if let Some((start, end)) = extended {
            expected.push((EXTENDED_SUPPORT, start, end));
        }
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the published support windows, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_report_is_a_read_and_answers_the_same_twice() {
    let router = emulator();

    let params: Vec<(&str, String)> = vec![("Engine", "mysql".to_string())];
    let first = expect_ok("the first read", call(&router, ACTION, &params).await);
    let second = expect_ok("the second read", call(&router, ACTION, &params).await);
    assert_eq!(
        reported(&first)
            .iter()
            .map(Family::pair)
            .collect::<Vec<_>>(),
        reported(&second)
            .iter()
            .map(Family::pair)
            .collect::<Vec<_>>(),
        "case \"two reads\": a report must not change what a later caller sees"
    );
    assert_eq!(
        reported(&first)
            .iter()
            .flat_map(|family| family.lifecycles.iter().map(|window| window.end.clone()))
            .collect::<Vec<_>>(),
        reported(&second)
            .iter()
            .flat_map(|family| family.lifecycles.iter().map(|window| window.end.clone()))
            .collect::<Vec<_>>(),
        "case \"two reads\": a published support calendar must not move between reads"
    );
}
