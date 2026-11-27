// SPDX-License-Identifier: CC0-1.0

//! What the Apache Kafka version catalogue PUBLISHES versus what an operation
//! ADMITS, driven end to end over the same `MultiServiceRouter` the emulator
//! serves from (the shared [`crate::kafka_harness`]).
//!
//! The RULE under test, in two halves:
//!
//! 1. `ListKafkaVersions` serves the service's catalogue as a sequence of ROWS —
//!    one per row the service publishes, each carrying a version string and that
//!    row's `KafkaVersionStatus` — in the catalogue's own published order, so a
//!    walk of the pages reproduces it row for row: every variant spelling the
//!    service publishes a row for (the `.tiered` storage line, the `.kraft`
//!    metadata-mode lines, the `link` line) and a version's second, other-status
//!    row included.
//! 2. Which versions an operation TAKES for a NEW RESOURCE is a SEPARATE and
//!    narrower fact: a provisioned cluster's versions are its broker category's
//!    offering, which the refusal of a version outside it advertises, so a row the
//!    catalogue publishes is published and nothing more — it never widens that
//!    gate — and no offering ever names a version the catalogue does not publish.
//!    A CONFIGURATION is not such a resource: it declares which versions a
//!    `server.properties` body may be APPLIED to, so it may name every version the
//!    catalogue publishes, while the valid-value list its refusal advertises stays
//!    the service's own narrower sentence.
//!
//! Each suite asserts that over a TABLE whose rows differ along what the rule
//! generalises over: the FORM of the published version string (a three-component
//! release, a four-component one, the `.x` patch name, the `.tiered` and `.kraft`
//! and `link` variant lines), how many rows the catalogue publishes for the
//! version, and — for the second half — the combinations of published against
//! admitted (published and taken everywhere, published and admitted by a
//! configuration only, published and built on by nothing, not published at all).
//! The expected rows are hand-written from the catalogue the service publishes
//! (the Amazon MSK Developer Guide's "Supported Apache Kafka versions" list) and
//! every expectation is read off the WIRE, so neither half is asserted against
//! the helper that computes it.

use serde_json::Value;

use crate::kafka_harness::{
    pct, unsupported_broker_version_message, v2_provisioned_body, ConfigurationRequest, Emulator,
    Reply,
};

/// A broker instance type of the Standard category, which is what the cluster
/// rows below ask for.
const STANDARD_INSTANCE_TYPE: &str = "kafka.m5.large";

/// The broker category `STANDARD_INSTANCE_TYPE` belongs to, as MSK words it in
/// the refusal that lists a category's offering.
const STANDARD_CATEGORY: &str = "Standard";

/// One `ListKafkaVersions` call; `query` is the whole query string including its
/// `?`, or empty.
async fn list_versions(emulator: &Emulator, query: &str) -> Reply {
    let reply = emulator
        .call("GET", &format!("/v1/kafka-versions{query}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"ListKafkaVersions{query}\": expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// The `(version, status)` rows a page reports, in reported order.
fn rows(reply: &Reply) -> Vec<(String, String)> {
    reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            (
                entry["version"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a catalogue row without a version: {entry}"))
                    .to_string(),
                entry["status"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a catalogue row without a status: {entry}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The whole published catalogue, walked at `page_size` rows per page.
async fn walked(emulator: &Emulator, page_size: usize) -> Vec<(String, String)> {
    /// A generous ceiling on the number of pages a walk may take, so a listing
    /// that never closes fails here instead of looping forever.
    const PAGE_CEILING: usize = 1024;

    let mut seen: Vec<(String, String)> = Vec::new();
    let mut query = format!("?maxResults={page_size}");
    for _ in 0..PAGE_CEILING {
        let page = list_versions(emulator, &query).await;
        seen.extend(rows(&page));
        match page.body.get("nextToken").and_then(Value::as_str) {
            Some(token) => query = format!("?maxResults={page_size}&nextToken={}", pct(token)),
            None => return seen,
        }
    }
    panic!("case maxResults={page_size}: the walk never closed, got {seen:?}");
}

/// `CreateConfiguration` (`POST /v1/configurations`) naming `versions`.
async fn create_configuration(emulator: &Emulator, name: &str, versions: &[&str]) -> Reply {
    let request = ConfigurationRequest {
        kafka_versions: versions.iter().map(|entry| (*entry).to_string()).collect(),
        ..ConfigurationRequest::named(name)
    };
    emulator
        .call("POST", "/v1/configurations", Some(request.to_json()))
        .await
}

/// The versions a refusal advertises as valid, in the order it named them.
fn advertised(message: &str, case: &str) -> Vec<String> {
    let (_, tail) = message.split_once("Valid values: [").unwrap_or_else(|| {
        panic!("case {case}: the refusal must carry a valid-value list, got {message:?}")
    });
    tail.strip_suffix(']')
        .unwrap_or_else(|| panic!("case {case}: the valid-value list must be closed, got {message:?}"))
        .split(", ")
        .map(str::to_string)
        .collect()
}

/// The versions a configuration may name, read off the wire from the refusal that
/// advertises them.
async fn configuration_offering(emulator: &Emulator) -> Vec<String> {
    let probe = create_configuration(emulator, "offering-probe", &["0.0.1-does-not-exist"]).await;
    assert_eq!(
        (probe.status, probe.error_code()),
        (400, "BadRequestException"),
        "case \"the configuration offering\": expected a refusal to read it from, got {} {}",
        probe.status,
        probe.body
    );
    advertised(probe.error_message(), "the configuration offering")
}

/// The versions a NEW Standard-broker cluster may name, read off the wire from
/// the refusal that advertises them.
async fn standard_cluster_offering(emulator: &Emulator) -> Vec<String> {
    let probe = emulator
        .create_cluster_v2(v2_provisioned_body(
            "cluster-offering-probe",
            "0.0.1-does-not-exist",
            STANDARD_INSTANCE_TYPE,
        ))
        .await;
    assert_eq!(
        (probe.status, probe.error_code()),
        (400, "BadRequestException"),
        "case \"the Standard offering\": expected a refusal to read it from, got {} {}",
        probe.status,
        probe.body
    );
    advertised(probe.error_message(), "the Standard offering")
}

/// RULE (first half): the listing serves the catalogue's published ROWS — the
/// variant lines and a version's second lifecycle row included — and a walk of
/// the pages at any page size reproduces that sequence row for row.
#[tokio::test]
async fn the_listing_serves_every_published_row_including_the_variant_lines() {
    let emulator = Emulator::new();
    let published = rows(&list_versions(&emulator, "").await);

    // Rows hand-written from the catalogue Amazon MSK publishes (the Developer
    // Guide's "Supported Apache Kafka versions" list, and the rows the service
    // itself publishes), spanning the FORMS a published version string takes and
    // the number of rows a version is published on. Each row is the status
    // SEQUENCE published for that version, in published order.
    let expected: &[(&str, &[&str])] = &[
        // A plain three-component release, and a four-component one.
        ("1.1.1", &["DEPRECATED"]),
        ("2.4.1.1", &["DEPRECATED"]),
        // The `.tiered` storage line.
        ("2.8.2.tiered", &["DEPRECATED"]),
        // An offered release, and the `.x` patch name.
        ("3.6.0", &["ACTIVE"]),
        ("3.7.x", &["ACTIVE"]),
        // The `.kraft` metadata-mode line.
        ("3.8.x.kraft", &["ACTIVE"]),
        // The `link` line of that same release, which the service publishes
        // deprecated.
        ("3.8.link", &["DEPRECATED"]),
        // Versions the catalogue publishes on TWO rows: both lifecycle statuses,
        // the offering row last.
        ("3.9.x", &["DEPRECATED", "ACTIVE"]),
        ("4.2.x.kraft", &["DEPRECATED", "ACTIVE"]),
    ];
    for (version, statuses) in expected {
        let served: Vec<&str> = published
            .iter()
            .filter(|(name, _)| name == version)
            .map(|(_, status)| status.as_str())
            .collect();
        assert_eq!(
            served, *statuses,
            "case version={version:?}: expected the published rows {statuses:?}, got {served:?}"
        );
    }

    // A version the catalogue does not publish is served by no row, whatever
    // spelling it is asked for under.
    for absent in ["3.8.x.link", "3.9.link", "2.8.2", "9.9.9", "3.8.LINK", ""] {
        assert!(
            !published.iter().any(|(name, _)| name == absent),
            "case version={absent:?}: must not be published, got {published:?}"
        );
    }

    // The pages of a walk reproduce that same sequence, so a row's POSITION is
    // the catalogue's and not the page's: one row at a time, a size that divides
    // the catalogue unevenly, and one page for the lot.
    for page_size in [1, 7, published.len()] {
        let seen = walked(&emulator, page_size).await;
        assert_eq!(
            seen, published,
            "case maxResults={page_size}: the walk must reproduce the published rows"
        );
    }
}

/// RULE (second half): being published is not being built on. For each row of the
/// table, the catalogue publishes the version exactly when the row says so, a
/// configuration may name it exactly when the catalogue publishes it, and a new
/// Standard-broker cluster takes it exactly when the catalogue publishes it as
/// offered — each refusal naming the version verbatim beside the versions that
/// operation ADVERTISES as valid, which for a cluster is MSK's narrower
/// new-cluster offering, and storing nothing.
#[tokio::test]
async fn a_published_row_is_not_a_version_an_operation_admits() {
    let emulator = Emulator::new();
    let published: Vec<String> = rows(&list_versions(&emulator, "").await)
        .into_iter()
        .map(|(version, _)| version)
        .collect();
    let for_configurations = configuration_offering(&emulator).await;
    let for_clusters = standard_cluster_offering(&emulator).await;

    // Rows: the combinations of published against admitted, plus a second
    // published-and-built-on-by-nothing spelling, so the rule is checked on more
    // than the one row the catalogue gained. The last column is whether a NEW
    // standard-broker cluster may be built on the version, which is the catalogue's
    // own `ACTIVE` verdict — a wider set than the one the refusal ADVERTISES, since
    // MSK goes on offering a version to a cluster that already runs it after it has
    // stopped steering new clusters onto it.
    let table: &[(&str, &str, bool, bool)] = &[
        // Published and taken by both operations.
        ("offered-everywhere", "3.9.x", true, true),
        // Published as offered, so a new cluster may still be built on it, though
        // the refusal's valid-value list no longer advertises it.
        ("offered-not-advertised-for-a-new-cluster", "3.6.0", true, true),
        // Published deprecated: a configuration may still name it, because
        // clusters are still running it, but no new cluster is built on it.
        ("deprecated-but-named-by-a-configuration", "1.1.1", true, false),
        ("deprecated-variant-line", "2.8.2.tiered", true, false),
        // Published, built on by nothing, and named by no advertised list: the
        // `link` line. A configuration may still name it — the catalogue publishes
        // it, so a client can only have learnt it from this service.
        ("published-only", "3.8.link", true, false),
        // Not published at all.
        ("never-published", "9.9.9", false, false),
        ("never-published-variant", "3.8.x.link", false, false),
    ];

    // Everything the refusal advertises is something a create is admitted on: the
    // retry it invites must be served.
    for version in &for_clusters {
        let probe = emulator
            .create_cluster_v2(v2_provisioned_body(
                &format!("advertised-{}", version.replace('.', "-")),
                version,
                STANDARD_INSTANCE_TYPE,
            ))
            .await;
        assert_eq!(
            probe.status, 200,
            "case \"advertised {version}\": an advertised version must build a cluster, got {} {}",
            probe.status, probe.body
        );
    }

    for (case, version, is_published, builds_a_cluster) in table {
        assert_eq!(
            published.iter().any(|name| name == version),
            *is_published,
            "case {case} ({version}): expected published={is_published}, got {published:?}"
        );
        // An offering never names a version the catalogue does not publish.
        for (what, offering) in [
            ("a configuration", &for_configurations),
            ("a new Standard cluster", &for_clusters),
        ] {
            if offering.iter().any(|name| name == version) {
                assert!(
                    *is_published,
                    "case {case} ({version}): {what} must not be offered a version the \
                     catalogue does not publish, got {offering:?}"
                );
            }
        }

        // CreateConfiguration admits exactly the versions the catalogue
        // publishes: a configuration is declared for the versions clusters run.
        let takes_configuration = *is_published;
        let name = format!("cfg-{case}");
        let reply = create_configuration(&emulator, &name, &[version]).await;
        if takes_configuration {
            assert_eq!(
                (reply.status, reply.error_code()),
                (200, ""),
                "case {case} ({version}): a published version must be accepted, got {} {}",
                reply.status,
                reply.body
            );
            let arn = reply.body["arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case} ({version}): the create returned no arn: {}", reply.body)
                })
                .to_string();
            let described = emulator.describe_configuration(&arn).await;
            assert_eq!(
                described.at("/kafkaVersions"),
                &serde_json::json!([version]),
                "case {case} ({version}): the stored configuration must report it, got {}",
                described.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {case} ({version}): a version the catalogue does not publish must be \
                 refused, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some("kafkaVersions"),
                "case {case} ({version}): the refusal must name the member, got {}",
                reply.body
            );
            let expected = format!(
                "Unsupported KafkaVersion [{version}]. Valid values: [{}]",
                for_configurations.join(", ")
            );
            assert_eq!(
                reply.error_message(),
                expected,
                "case {case} ({version}): expected {expected:?}, got {:?}",
                reply.error_message()
            );
            assert!(
                !emulator.listed_configuration_names().await.contains(&name),
                "case {case} ({version}): a refused create must store nothing"
            );
        }

        // CreateClusterV2 admits every version the catalogue publishes as offered,
        // and refuses every other one with its category's advertised list.
        let takes_cluster = *builds_a_cluster;
        let cluster = emulator
            .create_cluster_v2(v2_provisioned_body(
                &format!("cluster-{case}"),
                version,
                STANDARD_INSTANCE_TYPE,
            ))
            .await;
        if takes_cluster {
            assert_eq!(
                (cluster.status, cluster.error_code()),
                (200, ""),
                "case {case} ({version}): an offered version must build a cluster, got {} {}",
                cluster.status,
                cluster.body
            );
        } else {
            assert_eq!(
                (cluster.status, cluster.error_code()),
                (400, "BadRequestException"),
                "case {case} ({version}): a version outside the category's offering must be \
                 refused, got {} {}",
                cluster.status,
                cluster.body
            );
            let expected =
                unsupported_broker_version_message(STANDARD_CATEGORY, version, &for_clusters);
            assert_eq!(
                cluster.error_message(),
                expected,
                "case {case} ({version}): expected {expected:?}, got {:?}",
                cluster.error_message()
            );
            assert!(
                !emulator
                    .list_v2("")
                    .await
                    .at("/clusterInfoList")
                    .as_array()
                    .map(|entries| entries.iter().any(|entry| {
                        entry["clusterName"].as_str() == Some(format!("cluster-{case}").as_str())
                    }))
                    .unwrap_or_default(),
                "case {case} ({version}): a refused create must mint no cluster"
            );
        }
    }

    // Not vacuous: the table really covers both directions, and the two
    // offerings really differ from the published catalogue and from each other.
    assert!(
        published.len() > for_configurations.len(),
        "the catalogue must publish more rows than a configuration may name, got {published:?} \
         and {for_configurations:?}"
    );
    assert!(
        for_configurations.len() > for_clusters.len(),
        "a configuration's offering must be wider than a new cluster's, got \
         {for_configurations:?} and {for_clusters:?}"
    );
}

/// RULE: the published compatibility matrix is a table of the SAME published rows
/// — so the variant-line row the catalogue publishes is a source there too, and
/// since the service does not offer it, it is terminal and no source may be
/// upgraded onto it.
#[tokio::test]
async fn the_matrix_publishes_the_variant_line_row_as_a_terminal_source() {
    let emulator = Emulator::new();
    let published = rows(&list_versions(&emulator, "").await);
    let reply = emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 for a matrix request, got {} {}",
        reply.status, reply.body
    );
    let entries: Vec<(String, Vec<String>)> = reply.body["compatibleKafkaVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            (
                entry["sourceVersion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("an entry without a sourceVersion: {entry}"))
                    .to_string(),
                entry["targetVersions"]
                    .as_array()
                    .unwrap_or_else(|| panic!("an entry without targetVersions: {entry}"))
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .unwrap_or_else(|| panic!("a non-string target version: {entry}"))
                            .to_string()
                    })
                    .collect(),
            )
        })
        .collect();

    // One source row per published row, in published order.
    let sources: Vec<String> = entries.iter().map(|(source, _)| source.clone()).collect();
    let published_versions: Vec<String> = published
        .iter()
        .map(|(version, _)| version.clone())
        .collect();
    assert_eq!(
        sources, published_versions,
        "the matrix must publish one source per published row, in published order"
    );

    // Rows: the published versions whose line has no upgrade off it — the `link`
    // line, which is on no metadata-mode line at all, and the newest KRaft
    // version, which nothing newer is published beside.
    for terminal in ["3.8.link", "4.2.x.kraft"] {
        let targets: Vec<&Vec<String>> = entries
            .iter()
            .filter(|(source, _)| source == terminal)
            .map(|(_, targets)| targets)
            .collect();
        assert!(
            !targets.is_empty(),
            "case source={terminal:?}: the matrix must publish a row for it, got {sources:?}"
        );
        for offered in targets {
            assert!(
                offered.is_empty(),
                "case source={terminal:?}: a terminal source offers no target, got {offered:?}"
            );
        }
    }

    // And a version the service does not offer is nobody's upgrade target.
    for (source, targets) in &entries {
        for unoffered in ["3.8.link", "1.1.1", "2.8.2.tiered"] {
            assert!(
                !targets.iter().any(|target| target == unoffered),
                "case source={source:?}: {unoffered:?} is not offered, so it must be no \
                 target, got {targets:?}"
            );
        }
    }
}
