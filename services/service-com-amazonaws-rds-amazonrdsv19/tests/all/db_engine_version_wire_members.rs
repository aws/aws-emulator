// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the member set an engine-version entry carries: every capability
//! member the `DBEngineVersion` shape declares is answered on every entry, each value is
//! the VERSION's own, and the set does not vary with what the request asked for.
//!
//! The rule under test is stated over a TABLE of every engine this service publishes
//! versions for, and over every version of each. The pagination half asserts the invariant a
//! `Marker` owes a caller rather than one recorded token: a cut that leaves entries
//! unserved reports a token, the token resumes the report exactly where the page stopped,
//! and the page that serves the tail reports none.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`).
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`).

use smithy_server_runtime::MultiServiceRouter;

use crate::custom_db_engine_versions::{described, KMS_KEY_ID};
use crate::db_engine_versions::{versions, walk_report, Version};
use crate::db_instances::{call, emulator, expect_ok, optional_text, text};

/// The operation under test.
const ACTION: &str = "DescribeDBEngineVersions";

/// Every engine this service publishes catalogue versions for — the engines of its own
/// control plane and the two it reports for another (Amazon DocumentDB and Amazon Neptune).
///
/// Source: the `Engine` member's own published value list
/// (`smithy_query(query="doc", shape="DescribeDBMajorEngineVersionsRequest",
/// member="Engine")`), which is where this catalogue's engine names come from.
const ENGINES: &[&str] = &[
    "aurora-mysql",
    "aurora-postgresql",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
    "db2-ae",
    "db2-ce",
    "db2-se",
    "docdb",
    "mariadb",
    "mysql",
    "neptune",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-dev-ee",
    "sqlserver-ee",
    "sqlserver-ex",
    "sqlserver-se",
    "sqlserver-web",
];

/// The boolean capability members the `DBEngineVersion` shape declares and this report
/// answers on every entry, whatever its engine: each is a fact about the VERSION, so a
/// version that does not offer the capability reports it false rather than leaving it out.
///
/// Source: the `Supports*` members of `smithy_query(query="shape", shape="DBEngineVersion")`.
const EVERY_ENTRY: [&str; 9] = [
    "SupportsLogExportsToCloudwatchLogs",
    "SupportsReadReplica",
    "SupportsParallelQuery",
    "SupportsGlobalDatabases",
    "SupportsBabelfish",
    "SupportsLimitlessDatabase",
    "SupportsCertificateRotationWithoutRestart",
    "SupportsLocalWriteForwarding",
    "SupportsIntegrations",
];

/// One boolean member of an entry, which must be reported and must be a boolean.
fn flag(case: &str, xml: &str, member: &str) -> bool {
    let value = optional_text(xml, member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be reported, got {xml}"));
    match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: {member} must be a boolean, got {other:?} in {xml}"),
    }
}

/// Whether `engine_version` is a release OF the major version line `major_engine_version`:
/// the line's own name, a dot, and the rest of the release, which is how every engine this
/// service publishes names a build of a family.
fn release_is_in_line(engine_version: &str, major_engine_version: &str) -> bool {
    engine_version
        .strip_prefix(major_engine_version)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Every version of `engine`, deprecated ones included, walked page by page.
async fn all_versions_of(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    walk_report(
        router,
        engine,
        100,
        &[
            ("Engine", engine.to_string()),
            ("IncludeAll", "true".to_string()),
        ],
    )
    .await
}

#[tokio::test]
async fn every_entry_carries_the_whole_member_set_its_engine_reports() {
    let router = emulator();

    let mut reported = 0usize;
    for &engine in ENGINES {
        let all = all_versions_of(&router, engine).await;
        assert!(
            !all.is_empty(),
            "case {engine:?}: this service publishes versions for every engine in the \
             table, got none"
        );

        for version in &all {
            reported += 1;
            let case = format!("{engine} {}", version.engine_version);

            // `MajorEngineVersion` names the family this release belongs to, so the release
            // is that family's own name, a dot, and the rest of the release.
            assert!(
                release_is_in_line(&version.engine_version, &version.major_engine_version),
                "case {case:?}: MajorEngineVersion {:?} must name the family \
                 EngineVersion is a release of, got {}",
                version.major_engine_version,
                version.scalars
            );

            // Reading each of these is the whole assertion: `flag` refuses an absent
            // member and a value that is not a boolean.
            for member in EVERY_ENTRY {
                let _ = flag(&case, &version.scalars, member);
            }
        }
    }
    assert!(
        reported > ENGINES.len(),
        "case \"every engine\": the report must carry several versions per engine, got \
         {reported} across {} engines",
        ENGINES.len()
    );
}

#[tokio::test]
async fn the_member_set_does_not_vary_with_what_the_request_asked_for() {
    let router = emulator();

    // One version, read four ways: narrowed by engine, narrowed by engine and version,
    // narrowed by parameter group family, and with the character-set and time-zone lists
    // asked for. The member set an entry carries is the entry's, not the request's.
    let anchor = all_versions_of(&router, "aurora-postgresql")
        .await
        .into_iter()
        .next_back()
        .expect("aurora-postgresql publishes versions");
    let narrowings: [(&str, Vec<(&str, String)>); 4] = [
        ("engine only", vec![("Engine", "aurora-postgresql".to_string())]),
        (
            "engine and version",
            vec![
                ("Engine", "aurora-postgresql".to_string()),
                ("EngineVersion", anchor.engine_version.clone()),
            ],
        ),
        (
            "parameter group family",
            vec![(
                "DBParameterGroupFamily",
                anchor.parameter_group_family.clone(),
            )],
        ),
        (
            "lists asked for",
            vec![
                ("Engine", "aurora-postgresql".to_string()),
                ("EngineVersion", anchor.engine_version.clone()),
                ("ListSupportedCharacterSets", "true".to_string()),
                ("ListSupportedTimezones", "true".to_string()),
            ],
        ),
    ];

    for (case, params) in narrowings {
        let body = expect_ok(case, call(&router, ACTION, &params).await);
        let entry = versions(&body)
            .into_iter()
            .find(|version| version.engine_version == anchor.engine_version)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the report must carry {:?}, got {body}",
                    anchor.engine_version
                )
            });
        assert_eq!(
            text(&entry.scalars, "MajorEngineVersion"),
            text(&anchor.scalars, "MajorEngineVersion"),
            "case {case:?}: MajorEngineVersion must not depend on how the request \
             narrowed the report, got {}",
            entry.scalars
        );
        for member in EVERY_ENTRY {
            assert_eq!(
                flag(case, &entry.scalars, member),
                flag(case, &anchor.scalars, member),
                "case {case:?}: {member} must not depend on how the request narrowed the \
                 report, got {}",
                entry.scalars
            );
        }
    }
}

/// One custom engine version a create stores, as the round-trip table names it.
struct Row {
    engine: &'static str,
    engine_version: &'static str,
    /// The major version whose family the CEV name belongs to.
    major_engine_version: &'static str,
    /// The rest of the create's parameters.
    extra: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_stored_custom_engine_version_carries_the_same_member_set() {
    let router = emulator();

    // A custom engine version is the one engine-version kind a request creates, so it is
    // the one that can be round-tripped: create it, then read it back through
    // `DescribeDBEngineVersions`. Both answers come from one projection, so both carry
    // the member set a catalogue row of the same engine carries.
    //
    // (engine, CEV name, the major version its name belongs to, the rest of the create).
    // The name forms are the ones `CreateCustomDBEngineVersion$EngineVersion` publishes,
    // and RDS Custom for SQL Server requires an AMI id.
    let rows: [Row; 2] = [
        Row {
            engine: "custom-sqlserver-se",
            engine_version: "15.00.4073.23.my_cev1",
            major_engine_version: "15.00",
            extra: &[
                ("KMSKeyId", KMS_KEY_ID),
                ("UseAwsProvidedLatestImage", "true"),
            ],
        },
        Row {
            engine: "custom-sqlserver-ee",
            engine_version: "15.00.4249.2.my_cev1",
            major_engine_version: "15.00",
            extra: &[
                ("KMSKeyId", KMS_KEY_ID),
                ("ImageId", "ami-0abcdef1234567890"),
            ],
        },
    ];

    for Row {
        engine,
        engine_version,
        major_engine_version,
        extra,
    } in rows
    {
        let case = format!("{engine} {engine_version}");
        let mut params = vec![
            ("Engine".to_string(), engine.to_string()),
            ("EngineVersion".to_string(), engine_version.to_string()),
        ];
        for (name, value) in extra {
            params.push(((*name).to_string(), (*value).to_string()));
        }
        let created = expect_ok(
            &case,
            call(&router, "CreateCustomDBEngineVersion", &params).await,
        );
        let described = described(&router, "us-west-2", &case, engine, engine_version, true).await;
        let read_back = described.first().unwrap_or_else(|| {
            panic!("case {case:?}: the created version must be reported, got {described:?}")
        });

        assert!(
            release_is_in_line(engine_version, major_engine_version),
            "case {case:?}: the table's CEV name must be a release of the family it names, \
             got {engine_version:?} and {major_engine_version:?}"
        );
        for projection in [&created, read_back] {
            assert_eq!(
                text(projection, "MajorEngineVersion"),
                major_engine_version,
                "case {case:?}: a custom engine version reports the family its own name \
                 belongs to, got {projection}"
            );
            // Reading each of these is half the assertion: `flag` refuses an absent member
            // and a value that is not a boolean, on the create's answer and on the
            // read-back alike.
            for member in EVERY_ENTRY {
                let _ = flag(&case, projection, member);
            }
        }
        // And both answers agree on every one of them: ONE projection serves the create and
        // the read.
        for member in EVERY_ENTRY {
            assert_eq!(
                flag(&case, &created, member),
                flag(&case, read_back, member),
                "case {case:?}: {member} must be reported the same way by the create and by \
                 the read-back, got {created} against {read_back}"
            );
        }
    }
}

/// One page of the whole report at `page_size`, resumed at `marker`: the entries and the
/// `Marker` the result carried.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<Version>, Option<String>) {
    let mut params = vec![
        ("MaxRecords".to_string(), page_size.to_string()),
        ("IncludeAll".to_string(), "true".to_string()),
    ];
    if let Some(marker) = marker {
        params.push(("Marker".to_string(), marker.to_string()));
    }
    let body = expect_ok(case, call(router, ACTION, &params).await);
    (versions(&body), optional_text(&body, "Marker"))
}

/// The identity of each reported entry, as two pages are compared by.
fn keys(entries: &[Version]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect()
}

#[tokio::test]
async fn a_cut_that_leaves_entries_unserved_reports_the_marker_that_resumes_it() {
    let router = emulator();

    let whole = walk_report(
        &router,
        "whole report",
        100,
        &[("IncludeAll", "true".to_string())],
    )
    .await;
    assert!(
        whole.len() > 40,
        "case \"whole report\": the catalogue must be larger than one page, got {} entries",
        whole.len()
    );

    // Each published `MaxRecords` bound and one size between them: wherever the cut
    // falls, the page is the report's own page and the token names the entry the next
    // page starts at.
    for page_size in [20usize, 37, 100] {
        let case = format!("page size {page_size}");
        let (first, marker) = page(&router, &case, page_size, None).await;
        let served = page_size.min(whole.len());
        assert_eq!(
            keys(&first),
            keys(&whole[..served]),
            "case {case:?}: the first page must be the report's own first {served} entries"
        );
        if whole.len() <= page_size {
            assert_eq!(
                marker, None,
                "case {case:?}: a page that serves the whole report reports no \
                 continuation token"
            );
            continue;
        }
        let marker = marker.unwrap_or_else(|| {
            panic!(
                "case {case:?}: a cut that leaves {} entries unserved must report a \
                 continuation token",
                whole.len() - served
            )
        });
        let (second, _) = page(&router, &case, page_size, Some(&marker)).await;
        let end = (served + page_size).min(whole.len());
        assert_eq!(
            keys(&second),
            keys(&whole[served..end]),
            "case {case:?}: the token must resume the report where the first page \
             stopped, with entries {served}..{end}"
        );
    }

    // Walking to the end: every entry exactly once, and the page that serves the tail
    // reports no token at all.
    let mut seen: Vec<Version> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let (entries, next) = page(&router, "walk to the end", 100, marker.as_deref()).await;
        seen.extend(entries);
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
        assert!(
            seen.len() <= whole.len(),
            "case \"walk to the end\": paging must terminate at {} entries, got {}",
            whole.len(),
            seen.len()
        );
    }
    assert_eq!(
        keys(&seen),
        keys(&whole),
        "case \"walk to the end\": the walked pages must be the whole report, once each"
    );
}
