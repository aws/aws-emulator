// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeServerlessV2PlatformVersions`, the report of the Neptune
//! Serverless platform versions this service publishes, and for the cluster member that
//! names the version a cluster runs on.
//!
//! THE RULE: the report answers out of one published catalogue
//! (`state::SERVERLESS_V2_PLATFORM_VERSIONS`) and every input member of the operation is a
//! FILTER over it — `Engine` and `ServerlessV2PlatformVersion` select the rows that match,
//! `IncludeAll` adds the rows no longer in use ("whether to also include platform versions
//! which are no longer in use"), `DefaultOnly` narrows to the row the catalogue designates
//! the engine's default ("the version used for new DB clusters") — so a value that names
//! nothing the catalogue carries selects no record instead of refusing the call, while the
//! `Filters` member the model states "isn't currently supported" IS refused. The same
//! catalogue answers `DBCluster$ServerlessV2PlatformVersion`, so the version a Serverless
//! cluster reports is the row this report marks as the default.
//!
//! The rules are asserted over TABLES spanning what they generalise over: every published
//! row (active and retired, default and not), each documented filter alone and crossed with
//! `IncludeAll`, the case of a value, both published bounds of `MaxRecords` and one step
//! past each, and a page size that makes the catalogue span several pages.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the wire harness this test target shares
//! (`db_cluster_lifecycle::Neptune`), so each assertion reads the XML the emulator itself
//! answered with. The bodies are read with `roxmltree` rather than by hand.

use service_com_amazonaws_neptune_amazonrdsv19::state;

use crate::db_cluster_lifecycle::{Neptune, Reply};

/// The operation under test.
const ACTION: &str = "DescribeServerlessV2PlatformVersions";

/// The `<Code>` a request this service refuses outside its modelled errors carries.
const VALIDATION_CODE: &str = "ValidationException";

/// The engine this view of the shared front door serves.
const ENGINE: &str = "neptune";

/// One record of the report, as the response carries it.
#[derive(Debug, PartialEq, Eq)]
struct Record {
    version: String,
    description: String,
    engine: String,
    status: String,
    is_default: String,
}

/// Every record of the report, in the order the body lists them.
///
/// The list is the awsQuery default (`<ServerlessV2PlatformVersions><member>…`), so the
/// records are the children of the list element rather than elements named for the shape.
fn records(reply: &Reply) -> Vec<Record> {
    let document =
        roxmltree::Document::parse(&reply.body).expect("the service answers a well-formed body");
    let Some(list) = document
        .descendants()
        .find(|node| node.has_tag_name("ServerlessV2PlatformVersions"))
    else {
        return Vec::new();
    };
    list.children()
        .filter(|node| node.is_element())
        .map(|info| {
            let member = |name: &str| {
                info.children()
                    .find(|node| node.has_tag_name(name))
                    .map(|node| node.text().unwrap_or_default().to_string())
                    .unwrap_or_default()
            };
            Record {
                version: member("ServerlessV2PlatformVersion"),
                description: member("ServerlessV2PlatformVersionDescription"),
                engine: member("Engine"),
                status: member("Status"),
                is_default: member("IsDefault"),
            }
        })
        .collect()
}

/// The versions `reply` reports, in order.
fn reported_versions(reply: &Reply) -> Vec<String> {
    records(reply)
        .into_iter()
        .map(|record| record.version)
        .collect()
}

/// The text of the first element named `name` in `reply`, or `None` when the body carries
/// no such element — which is how an ABSENT member is told from a reported one.
///
/// An element with no text at all reads as absent too: no member this suite reads is ever
/// reported as the empty string, so the two cases would say the same thing.
fn optional_text(reply: &Reply, name: &str) -> Option<String> {
    let document =
        roxmltree::Document::parse(&reply.body).expect("the service answers a well-formed body");
    document
        .descendants()
        .find(|node| node.has_tag_name(name))
        .map(|node| node.text().unwrap_or_default().to_string())
        .filter(|text| !text.is_empty())
}

/// The versions the catalogue publishes, in its own order.
fn published() -> Vec<&'static state::ServerlessPlatformVersion> {
    state::serverless_v2_platform_versions().collect()
}

/// Every published version, whatever its status.
fn every_version() -> Vec<String> {
    published()
        .into_iter()
        .map(|platform| platform.version.to_string())
        .collect()
}

/// The versions the catalogue publishes as still in use — what a request that says nothing
/// about `IncludeAll` reports.
fn active() -> Vec<String> {
    published()
        .into_iter()
        .filter(|platform| platform.in_use)
        .map(|platform| platform.version.to_string())
        .collect()
}

/// The version the catalogue designates the engine's default.
fn default_version() -> String {
    state::default_serverless_v2_platform_version()
        .map(|platform| platform.version.to_string())
        .expect("the published catalogue designates a default platform version")
}

/// A published version that is NO LONGER in use, for the rows that exercise `IncludeAll`.
fn retired_version() -> String {
    published()
        .into_iter()
        .find(|platform| !platform.in_use)
        .map(|platform| platform.version.to_string())
        .expect("the published catalogue carries a version no longer in use")
}

/// The report, or a panic naming the case when the service did not serve it.
async fn report(neptune: &Neptune, case: &str, params: &[(&str, &str)]) -> Reply {
    let reply = neptune.call(ACTION, params).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the report to be served, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

#[tokio::test]
async fn the_report_answers_every_member_out_of_the_published_catalogue() {
    // Every record's members are the catalogue row's own: the version, the description the
    // product's name and that version make, the engine, the status that follows from whether
    // the version is still in use, and the designation.
    let neptune = Neptune::new();
    let listed = report(&neptune, "the whole report", &[("IncludeAll", "true")]).await;

    let expected: Vec<Record> = published()
        .into_iter()
        .map(|platform| Record {
            version: platform.version.to_string(),
            description: state::serverless_platform_version_description(platform.version),
            engine: ENGINE.to_string(),
            status: match platform.in_use {
                true => state::PLATFORM_VERSION_ENABLED,
                false => state::PLATFORM_VERSION_DISABLED,
            }
            .to_string(),
            is_default: platform.is_default.to_string(),
        })
        .collect();
    let actual = records(&listed);
    assert_eq!(
        actual, expected,
        "case \"the whole report\": expected the published catalogue in its own order, got {}",
        listed.body
    );

    // Exactly one row is the engine's default, and it is one the report publishes as in use:
    // "the version used for new DB clusters" cannot be a version no longer in use.
    let defaults: Vec<&Record> = actual
        .iter()
        .filter(|record| record.is_default == "true")
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "case \"the whole report\": exactly one published version is the engine's default, \
         got {defaults:?}"
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
    expected: Vec<String>,
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
            expected: vec![default.clone()],
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
            expected: vec![default.clone()],
        },
        FilterCase {
            label: "the engine this view serves",
            params: vec![("Engine", ENGINE.to_string())],
            expected: active(),
        },
        FilterCase {
            label: "the engine named in another case",
            params: vec![("Engine", "Neptune".to_string())],
            expected: active(),
        },
        FilterCase {
            label: "another engine of the shared front door",
            params: vec![("Engine", "docdb".to_string())],
            expected: Vec::new(),
        },
        FilterCase {
            label: "the default version by name",
            params: vec![("ServerlessV2PlatformVersion", default.clone())],
            expected: vec![default.clone()],
        },
        FilterCase {
            label: "a retired version by name, which no active report carries",
            params: vec![("ServerlessV2PlatformVersion", retired.clone())],
            expected: Vec::new(),
        },
        FilterCase {
            label: "a retired version by name, with IncludeAll",
            params: vec![
                ("ServerlessV2PlatformVersion", retired.clone()),
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
                ("Engine", ENGINE.to_string()),
                ("ServerlessV2PlatformVersion", default.clone()),
            ],
            expected: vec![default.clone()],
        },
        FilterCase {
            label: "another engine and a version this one publishes",
            params: vec![
                ("Engine", "aurora-postgresql".to_string()),
                ("ServerlessV2PlatformVersion", default),
            ],
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_documented_filter_selects_out_of_the_catalogue() {
    let neptune = Neptune::new();
    for case in filter_cases() {
        let params: Vec<(&str, &str)> = case
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let listed = report(&neptune, case.label, &params).await;
        let versions = reported_versions(&listed);
        assert_eq!(
            versions, case.expected,
            "case {:?}: expected {:?}, got {}",
            case.label, case.expected, listed.body
        );
        // A selection that carries nothing is an EMPTY report, not a refusal and not a
        // report of everything.
        assert_eq!(
            records(&listed).len(),
            case.expected.len(),
            "case {:?}: expected {} record(s), got {}",
            case.label,
            case.expected.len(),
            listed.body
        );
    }
}

#[tokio::test]
async fn the_catalogue_fits_one_page_at_every_documented_bound() {
    // `MaxRecords`: "Default: 20", "Constraints: Minimum 1, maximum 200". Both published
    // bounds of the RANGE are accepted and one step past each is refused; the catalogue is
    // smaller than the default page, so the default page is the last one and carries no
    // marker.
    let neptune = Neptune::new();
    for (case, max_records) in [
        ("no MaxRecords, so the default page", None),
        ("the largest page the member documents", Some("200")),
        (
            "a page larger than the catalogue but inside the range",
            Some("20"),
        ),
    ] {
        let mut params: Vec<(&str, &str)> = vec![("IncludeAll", "true")];
        if let Some(value) = max_records {
            params.push(("MaxRecords", value));
        }
        let listed = report(&neptune, case, &params).await;
        assert_eq!(
            reported_versions(&listed),
            every_version(),
            "case {case:?}: the whole catalogue fits one page, got {}",
            listed.body
        );
        assert_eq!(
            optional_text(&listed, "Marker"),
            None,
            "case {case:?}: a last page carries no marker, got {}",
            listed.body
        );
    }

    for (case, max_records) in [
        ("one below the smallest page", "0"),
        ("one above the largest page", "201"),
        ("far above the largest page", "1000"),
    ] {
        let refused = neptune.call(ACTION, &[("MaxRecords", max_records)]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, VALIDATION_CODE),
            "case {case:?}: a MaxRecords outside the documented range is refused, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }
}

#[tokio::test]
async fn the_marker_walks_the_catalogue_one_published_page_at_a_time() {
    // The smallest page the member documents is 1 ("Constraints: Minimum 1"), so it makes
    // the catalogue span as many pages as it has rows: each page carries its own records and
    // the marker the next one resumes at, and the last page carries none. Nothing is stored
    // for a page, so the same walk repeated reads the same rows.
    let neptune = Neptune::new();
    let page_size = state::PLATFORM_VERSION_MAX_RECORDS_BOUNDS.0.to_string();
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for step in 0..=every_version().len() {
        let mut params: Vec<(&str, &str)> =
            vec![("IncludeAll", "true"), ("MaxRecords", page_size.as_str())];
        if let Some(from) = marker.as_deref() {
            params.push(("Marker", from));
        }
        let case = format!("page {step} of the walk");
        let listed = report(&neptune, &case, &params).await;
        let versions = reported_versions(&listed);
        assert!(
            versions.len() <= 1,
            "case {case:?}: a page of one record carries at most one, got {versions:?}"
        );
        walked.extend(versions);
        marker = optional_text(&listed, "Marker");
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        marker, None,
        "the walk ends on a page with no marker, got {marker:?}"
    );
    assert_eq!(
        walked,
        every_version(),
        "the walked pages are the whole catalogue in its published order, got {walked:?}"
    );
}

#[tokio::test]
async fn a_filters_member_is_refused_rather_than_silently_dropped() {
    // `Filters`: "This parameter isn't currently supported" — so this report supports no
    // filter name at all, and a request carrying one is refused in the sentence this service
    // answers an unsupported filter name with, naming the name the request carried.
    let neptune = Neptune::new();
    for (case, name, params) in [
        (
            "a filter name another report of this API publishes",
            "engine",
            vec![
                ("Filters.Filter.1.Name", "engine"),
                ("Filters.Filter.1.Values.Value.1", ENGINE),
            ],
        ),
        (
            "a filter beside a supported member",
            "platform-version",
            vec![
                ("Engine", ENGINE),
                ("Filters.Filter.1.Name", "platform-version"),
                ("Filters.Filter.1.Values.Value.1", "3"),
            ],
        ),
        (
            "a filter name nothing publishes",
            "not-a-filter-at-all",
            vec![
                ("Filters.Filter.1.Name", "not-a-filter-at-all"),
                ("Filters.Filter.1.Values.Value.1", "anything"),
            ],
        ),
    ] {
        let refused = neptune.call(ACTION, &params).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, VALIDATION_CODE),
            "case {case:?}: an unsupported filter is refused, got {} with {}",
            refused.status,
            refused.body
        );
        let expected = format!("Unrecognized filter name: {name}");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_serverless_cluster_runs_on_the_version_the_report_calls_the_default() {
    // The cross-report rule: `DBCluster$ServerlessV2PlatformVersion` is "the serverless
    // platform version used by the Neptune Serverless DB cluster", and the platform a NEW
    // cluster is placed on is the one this report designates the default. A cluster with no
    // Serverless capacity runs on no Serverless platform and reports none. Read off the wire
    // from both reports, so neither can be answered from a catalogue of its own.
    let neptune = Neptune::new();
    let reported_default = report(&neptune, "the default row", &[("DefaultOnly", "true")]).await;
    let reported_default = optional_text(&reported_default, "ServerlessV2PlatformVersion")
        .expect("the report publishes a default platform version");

    for (case, identifier, scaling) in [
        (
            "a cluster at the lowest capacity range",
            "emu-platform-serverless-low",
            Some(("1", "128")),
        ),
        (
            "a cluster at a half-step capacity range",
            "emu-platform-serverless-half",
            Some(("2.5", "40.5")),
        ),
        (
            "a cluster whose capacity range is a single value",
            "emu-platform-serverless-fixed",
            Some(("8.5", "8.5")),
        ),
        (
            "a provisioned cluster",
            "emu-platform-provisioned",
            None,
        ),
    ] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", identifier), ("Engine", ENGINE)];
        if let Some((min, max)) = scaling {
            params.push(("ServerlessV2ScalingConfiguration.MinCapacity", min));
            params.push(("ServerlessV2ScalingConfiguration.MaxCapacity", max));
        }
        let created = neptune.call("CreateDBCluster", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} with body {}",
            created.status, created.body
        );
        let expected = scaling.map(|_| reported_default.clone());
        assert_eq!(
            optional_text(&created, "ServerlessV2PlatformVersion"),
            expected,
            "case {case:?}: the create must report {expected:?}, got {}",
            created.body
        );

        // ROUND TRIP: the describe reads the record back out of state through the same
        // projection, so it reports exactly what the create announced.
        let described = neptune
            .call("DescribeDBClusters", &[("DBClusterIdentifier", identifier)])
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected the describe to be served, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            optional_text(&described, "ServerlessV2PlatformVersion"),
            expected,
            "case {case:?}: the describe must report {expected:?}, got {}",
            described.body
        );
    }
}
