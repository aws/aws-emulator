// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeServerlessV2PlatformVersions`, the report of the Amazon
//! DocumentDB Serverless platform versions this service publishes, and for the cluster
//! member that names the version a cluster runs on.
//!
//! THE RULE: the report answers out of one published catalogue
//! (`state::SERVERLESS_V2_PLATFORM_VERSIONS`) and every input member of the operation is a
//! FILTER over it — `Engine` and `ServerlessV2PlatformVersion` select the rows that match,
//! `IncludeAll` adds the rows no longer in use ("if you don't specify a value, the response
//! includes only active platform versions"), `DefaultOnly` narrows to the row the catalogue
//! designates the engine's default ("the version used for new DB clusters") — so a value
//! that names nothing the catalogue carries selects no record instead of refusing the call,
//! while the `Filters` member the model states "isn't currently supported" IS refused. The
//! same catalogue answers `DBCluster$ServerlessV2PlatformVersion`, so the version a
//! Serverless cluster reports is the row this report marks as the default.
//!
//! The rules are asserted over TABLES spanning what they generalise over: every published
//! row (active and retired, default and not), each documented filter alone and crossed with
//! `IncludeAll`, the case of a value, both published bounds of `MaxRecords` and one step
//! past each.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production router uses, with the wire harness this target shares
//! (`tests/all/cluster_lifecycle.rs`).

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{count, error_code, opt_text, text, texts, Docdb};

/// The `<Code>` a request this service refuses outside its modelled errors carries.
const VALIDATION_CODE: &str = "ValidationException";

/// The operation under test.
const ACTION: &str = "DescribeServerlessV2PlatformVersions";

/// The versions the catalogue publishes, in its own order.
fn published() -> Vec<&'static state::ServerlessPlatformVersion> {
    state::serverless_v2_platform_versions().collect()
}

/// The versions the catalogue publishes as still in use — what a request that says nothing
/// about `IncludeAll` reports.
fn active() -> Vec<&'static str> {
    published()
        .into_iter()
        .filter(|platform| platform.in_use)
        .map(|platform| platform.version)
        .collect()
}

/// Every published version, whatever its status.
fn every_version() -> Vec<&'static str> {
    published()
        .into_iter()
        .map(|platform| platform.version)
        .collect()
}

/// The version the catalogue designates the engine's default.
fn default_version() -> &'static str {
    state::default_serverless_v2_platform_version()
        .map(|platform| platform.version)
        .expect("the published catalogue designates a default platform version")
}

/// A published version that is NO LONGER in use, for the rows that exercise `IncludeAll`.
fn retired_version() -> &'static str {
    published()
        .into_iter()
        .find(|platform| !platform.in_use)
        .map(|platform| platform.version)
        .expect("the published catalogue carries a version no longer in use")
}

#[tokio::test]
async fn the_report_answers_every_member_out_of_the_published_catalogue() {
    // Every record's members are the catalogue row's own: the version, the description the
    // product's name and that version make, the engine, the status that follows from whether
    // the version is still in use, and the designation.
    let docdb = Docdb::new();
    let listed = docdb
        .ok("the whole report", ACTION, &[("IncludeAll", "true")])
        .await;

    let versions = texts(&listed, "ServerlessV2PlatformVersion");
    assert_eq!(
        versions,
        every_version(),
        "case \"the whole report\": expected the published catalogue in its own order, \
         got {listed}"
    );
    for platform in published() {
        let case = platform.version;
        let block = version_block(&listed, platform.version);
        assert_eq!(
            text(&block, "Engine"),
            "docdb",
            "case {case:?}: every record names this engine, got {block}"
        );
        assert_eq!(
            text(&block, "ServerlessV2PlatformVersionDescription"),
            state::serverless_platform_version_description(platform.version),
            "case {case:?}: expected the published description, got {block}"
        );
        let expected_status = match platform.in_use {
            true => "enabled",
            false => "disabled",
        };
        assert_eq!(
            text(&block, "Status"),
            expected_status,
            "case {case:?}: a version in use reports enabled and a retired one disabled, \
             got {block}"
        );
        assert_eq!(
            text(&block, "IsDefault"),
            platform.is_default.to_string(),
            "case {case:?}: expected IsDefault={}, got {block}",
            platform.is_default
        );
    }
    // Exactly one row is the engine's default, and it is one the report publishes as in use:
    // "the version used for new DB clusters" cannot be a version no longer in use.
    let defaults: Vec<String> = texts(&listed, "IsDefault")
        .into_iter()
        .filter(|value| value == "true")
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "case \"the whole report\": exactly one published version is the engine's default, \
         got {listed}"
    );
    assert!(
        active().contains(&default_version()),
        "case \"the whole report\": the default version must be one still in use, got {:?} \
         among {:?}",
        default_version(),
        active()
    );
}

/// One row of the filter table: the case, the request's parameters, and the versions the
/// report must carry, in order.
struct FilterCase {
    label: &'static str,
    params: Vec<(&'static str, String)>,
    expected: Vec<&'static str>,
}

/// The table: each documented filter on its own, crossed with `IncludeAll`, in both cases
/// of a value, and naming a value the catalogue does not carry.
fn filter_cases() -> Vec<FilterCase> {
    let default = default_version();
    let retired = retired_version();
    vec![
        FilterCase {
            label: "no filter at all",
            params: vec![],
            expected: active(),
        },
        FilterCase {
            label: "IncludeAll false, which is the default behaviour",
            params: vec![("IncludeAll", "false".to_string())],
            expected: active(),
        },
        FilterCase {
            label: "IncludeAll true",
            params: vec![("IncludeAll", "true".to_string())],
            expected: every_version(),
        },
        FilterCase {
            label: "DefaultOnly",
            params: vec![("DefaultOnly", "true".to_string())],
            expected: vec![default],
        },
        FilterCase {
            label: "DefaultOnly false keeps the whole active report",
            params: vec![("DefaultOnly", "false".to_string())],
            expected: active(),
        },
        FilterCase {
            label: "DefaultOnly crossed with IncludeAll",
            params: vec![
                ("DefaultOnly", "true".to_string()),
                ("IncludeAll", "true".to_string()),
            ],
            expected: vec![default],
        },
        FilterCase {
            label: "the engine this view serves",
            params: vec![("Engine", "docdb".to_string())],
            expected: active(),
        },
        FilterCase {
            label: "the engine named in another case",
            params: vec![("Engine", "DocDB".to_string())],
            expected: active(),
        },
        FilterCase {
            label: "another engine of the shared front door",
            params: vec![("Engine", "aurora-postgresql".to_string())],
            expected: Vec::new(),
        },
        FilterCase {
            label: "the default version by name",
            params: vec![("ServerlessV2PlatformVersion", default.to_string())],
            expected: vec![default],
        },
        FilterCase {
            label: "a retired version by name, which no active report carries",
            params: vec![("ServerlessV2PlatformVersion", retired.to_string())],
            expected: Vec::new(),
        },
        FilterCase {
            label: "a retired version by name, with IncludeAll",
            params: vec![
                ("ServerlessV2PlatformVersion", retired.to_string()),
                ("IncludeAll", "true".to_string()),
            ],
            expected: vec![retired],
        },
        FilterCase {
            label: "a version the catalogue does not publish",
            params: vec![
                ("ServerlessV2PlatformVersion", "4096".to_string()),
                ("IncludeAll", "true".to_string()),
            ],
            expected: Vec::new(),
        },
        FilterCase {
            label: "the engine and the default version together",
            params: vec![
                ("Engine", "docdb".to_string()),
                ("ServerlessV2PlatformVersion", default.to_string()),
            ],
            expected: vec![default],
        },
        FilterCase {
            label: "another engine and a version this one publishes",
            params: vec![
                ("Engine", "neptune".to_string()),
                ("ServerlessV2PlatformVersion", default.to_string()),
            ],
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_documented_filter_selects_out_of_the_catalogue() {
    let docdb = Docdb::new();
    for case in filter_cases() {
        let params: Vec<(&str, &str)> = case
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let listed = docdb.ok(case.label, ACTION, &params).await;
        let versions = texts(&listed, "ServerlessV2PlatformVersion");
        assert_eq!(
            versions,
            case.expected,
            "case {:?}: expected {:?}, got {listed}",
            case.label,
            case.expected
        );
        // A selection that carries nothing is an EMPTY report, not a refusal and not a
        // report of everything.
        assert_eq!(
            count(&listed, "ServerlessV2PlatformVersionInfo"),
            case.expected.len(),
            "case {:?}: expected {} record(s), got {listed}",
            case.label,
            case.expected.len()
        );
    }
}

#[tokio::test]
async fn the_whole_catalogue_fits_one_page_at_every_documented_bound() {
    // `MaxRecords`: "Default: 100 Constraints: Minimum 20, maximum 100." Both published
    // bounds are accepted and one step past each is refused; the catalogue is smaller than
    // the smallest page, so every page is the last one and carries no marker.
    let docdb = Docdb::new();
    for (case, max_records) in [
        ("no MaxRecords, so the default page", None),
        ("the smallest page the member documents", Some("20")),
        ("the largest page the member documents", Some("100")),
    ] {
        let mut params: Vec<(&str, &str)> = vec![("IncludeAll", "true")];
        if let Some(value) = max_records {
            params.push(("MaxRecords", value));
        }
        let listed = docdb.ok(case, ACTION, &params).await;
        assert_eq!(
            texts(&listed, "ServerlessV2PlatformVersion"),
            every_version(),
            "case {case:?}: the whole catalogue fits one page, got {listed}"
        );
        assert!(
            opt_text(&listed, "Marker").unwrap_or_default().is_empty(),
            "case {case:?}: a last page carries no marker, got {listed}"
        );
    }

    for (case, max_records) in [
        ("one below the smallest page", "19"),
        ("one above the largest page", "101"),
    ] {
        let refused = docdb.call(ACTION, &[("MaxRecords", max_records)]).await;
        assert_eq!(
            (refused.status, error_code(&refused.xml).as_str()),
            (400, VALIDATION_CODE),
            "case {case:?}: a MaxRecords outside the documented range is refused, got {} \
             with {}",
            refused.status,
            refused.xml
        );
    }
}

#[tokio::test]
async fn a_filters_member_is_refused_rather_than_silently_dropped() {
    // `Filters`: "This parameter isn't currently supported" — so a request carrying one is
    // refused rather than answered as though the filter had been applied.
    let docdb = Docdb::new();
    for (case, params) in [
        (
            "one filter",
            vec![
                ("Filters.Filter.1.Name", "engine"),
                ("Filters.Filter.1.Values.Value.1", "docdb"),
            ],
        ),
        (
            "a filter beside a supported member",
            vec![
                ("Engine", "docdb"),
                ("Filters.Filter.1.Name", "platform-version"),
                ("Filters.Filter.1.Values.Value.1", "3"),
            ],
        ),
    ] {
        let refused = docdb.call(ACTION, &params).await;
        assert_eq!(
            (refused.status, error_code(&refused.xml).as_str()),
            (400, VALIDATION_CODE),
            "case {case:?}: an unsupported filter is refused, got {} with {}",
            refused.status,
            refused.xml
        );
    }
}

#[tokio::test]
async fn a_serverless_cluster_runs_on_the_version_the_report_calls_the_default() {
    // The cross-report rule: `DBCluster$ServerlessV2PlatformVersion` is "the version of the
    // Amazon DocumentDB Serverless platform used by the cluster", and the platform a NEW
    // cluster is placed on is the one this report designates the default. A cluster with no
    // Serverless capacity runs on no Serverless platform and reports none. Read off the
    // wire from both reports, so neither can be answered from a catalogue of its own.
    let docdb = Docdb::new();
    let report = docdb
        .ok("the default row", ACTION, &[("DefaultOnly", "true")])
        .await;
    let reported_default = text(&report, "ServerlessV2PlatformVersion");

    for (case, identifier, scaling) in [
        (
            "a cluster at the lowest capacity range",
            "platform-serverless-low",
            Some(("0.5", "2")),
        ),
        (
            "a cluster at a half-step capacity range",
            "platform-serverless-half",
            Some(("8.5", "32.5")),
        ),
        (
            "a provisioned cluster",
            "platform-provisioned",
            None,
        ),
    ] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", identifier), ("Engine", "docdb")];
        if let Some((min, max)) = scaling {
            params.push(("ServerlessV2ScalingConfiguration.MinCapacity", min));
            params.push(("ServerlessV2ScalingConfiguration.MaxCapacity", max));
        }
        let created = docdb.ok(case, "CreateDBCluster", &params).await;
        let expected = scaling.map(|_| reported_default.clone());
        assert_eq!(
            opt_text(&created, "ServerlessV2PlatformVersion"),
            expected,
            "case {case:?}: the create must report {expected:?}, got {created}"
        );

        // ROUND TRIP: the describe reads the record back out of state through the same
        // projection, so it reports exactly what the create announced.
        let described = docdb
            .ok(
                case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier)],
            )
            .await;
        assert_eq!(
            opt_text(&described, "ServerlessV2PlatformVersion"),
            expected,
            "case {case:?}: the describe must report {expected:?}, got {described}"
        );
    }
}

/// The `<ServerlessV2PlatformVersionInfo>` element whose version is `version`, as its own
/// XML fragment.
///
/// The report carries one element per published version and the reader in
/// `cluster_lifecycle` reads a tag's text in document order, so a per-record assertion has
/// to narrow to the element first.
fn version_block(xml: &str, version: &str) -> String {
    let needle = format!("<ServerlessV2PlatformVersion>{version}</ServerlessV2PlatformVersion>");
    xml.split("<ServerlessV2PlatformVersionInfo>")
        .find(|block| block.contains(needle.as_str()))
        .map(|block| block.to_string())
        .unwrap_or_else(|| panic!("no record for platform version {version} in {xml}"))
}
