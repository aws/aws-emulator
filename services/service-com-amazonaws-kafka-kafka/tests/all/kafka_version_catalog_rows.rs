// SPDX-License-Identifier: CC0-1.0

//! The Apache Kafka version catalog as the service PUBLISHES it, driven end to
//! end over the same `MultiServiceRouter` the emulator serves from (the shared
//! harness in `tests/all/kafka_harness.rs`).
//!
//! Three rules, each asserted from the WIRE alone — the expectations are computed
//! from what `ListKafkaVersions` reported, never from the service's own catalog
//! constant, so a second, drifting table would fail here (the version-spelling
//! helpers are the shared harness's, so no suite disagrees with its neighbour
//! about what `3.9.x.kraft` names):
//!
//! * the listing serves one entry per published ROW, so a page resumes after a
//!   row — checked at EVERY page boundary, which is exhaustive over the
//!   boundaries a walk can stop at;
//! * a version the catalog publishes an `ACTIVE` row for is a version the
//!   service still offers, on every surface that answers about a version: the
//!   upgrade matrix's targets and the valid-value list a create refusal carries;
//! * the crossing from ZooKeeper mode to the KRaft line is offered at ONE
//!   release, the final offered ZooKeeper-mode release — checked over every
//!   ZooKeeper-mode version the catalog publishes.

use crate::kafka_harness::{
    kafka_version_line as line, kafka_version_release as release, pct, ClusterRequest, Emulator,
    Reply, KRAFT_LINE,
};

/// A `KafkaVersionStatus` value: a version the service still offers.
const ACTIVE: &str = "ACTIVE";

/// One `ListKafkaVersions` call. `query` is the raw query string, `""` for none.
async fn list_versions(emulator: &Emulator, query: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/kafka-versions{query}"), None)
        .await
}

/// The published rows a reply reports, as `(version, status)` in reported order.
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
                    .unwrap_or_else(|| panic!("a row without a version: {entry}"))
                    .to_string(),
                entry["status"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a row without a status: {entry}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The whole published catalog, read in one page.
async fn published_rows(emulator: &Emulator) -> Vec<(String, String)> {
    rows(&list_versions(emulator, "").await)
}

/// The versions the catalog publishes, each ONCE, in published order.
fn distinct_versions(rows: &[(String, String)]) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();
    for (version, _) in rows {
        if !versions.contains(version) {
            versions.push(version.clone());
        }
    }
    versions
}

/// Whether the catalog publishes any `ACTIVE` row for `version`.
fn offered(rows: &[(String, String)], version: &str) -> bool {
    rows.iter()
        .any(|(name, status)| name == version && status == ACTIVE)
}

/// The `(sourceVersion, targetVersions)` rows of the whole upgrade matrix —
/// `GetCompatibleKafkaVersions` with no `ClusterArn`, the member being optional.
async fn matrix(emulator: &Emulator) -> Vec<(String, Vec<String>)> {
    let reply = emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 for a matrix request, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/compatibleKafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            let source = entry["sourceVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without a sourceVersion: {entry}"))
                .to_string();
            let targets = entry["targetVersions"]
                .as_array()
                .unwrap_or_else(|| panic!("an entry without targetVersions: {entry}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("a non-string target version: {entry}"))
                        .to_string()
                })
                .collect();
            (source, targets)
        })
        .collect()
}

/// RULE: the listing serves one entry per published ROW, and `NextToken` names
/// the row a page stopped at — so resuming from ANY boundary reports exactly the
/// rows that follow it, and the pages of such a walk partition the catalog
/// exactly once.
#[tokio::test]
async fn a_page_resumes_after_a_row_at_every_boundary() {
    let emulator = Emulator::new();
    let whole = published_rows(&emulator).await;
    assert!(
        whole.len() > 1,
        "the catalog must publish more than one row to page, got {whole:?}"
    );
    // Exhaustive over the boundaries: every position a page can stop at.
    for boundary in 1..=whole.len() {
        let first = list_versions(&emulator, &format!("?maxResults={boundary}")).await;
        assert_eq!(
            first.status, 200,
            "case boundary={boundary}: expected 200, got {} {}",
            first.status, first.body
        );
        assert_eq!(
            rows(&first),
            whole[..boundary].to_vec(),
            "case boundary={boundary}: the first page must be the catalog's first {boundary} rows"
        );
        let token = first.body.get("nextToken").and_then(|value| value.as_str());
        if boundary == whole.len() {
            assert!(
                token.is_none(),
                "case boundary={boundary}: a page that reaches the end must close the list, \
                 got {token:?}"
            );
            continue;
        }
        let token = token.unwrap_or_else(|| {
            panic!("case boundary={boundary}: a page with rows left must carry a nextToken")
        });
        let rest = list_versions(&emulator, &format!("?nextToken={}", pct(token))).await;
        assert_eq!(
            rows(&rest),
            whole[boundary..].to_vec(),
            "case boundary={boundary}: resuming must report exactly the rows after it, got {}",
            rest.body
        );
    }
}

/// RULE: a version the catalog publishes an `ACTIVE` row for is one the service
/// still OFFERS, and one it publishes a `DEPRECATED` row for is not — on every
/// surface that answers about the version rather than about the catalog's rows:
/// which sources the upgrade matrix publishes a ladder for, which versions that
/// matrix offers as targets (narrower still: the software the service ships to a
/// new cluster), and the valid-value list a create refusal carries, which names
/// each version it ships ONCE in published order.
#[tokio::test]
async fn a_version_with_an_active_row_is_the_offered_version_set() {
    let emulator = Emulator::new();
    let catalog = published_rows(&emulator).await;

    // The table spans the shapes a published version can take: an `ACTIVE` row
    // and a `DEPRECATED` row, across the version spellings the catalog uses.
    // Hand-written from the published catalog (the Amazon MSK Developer Guide's
    // "Supported Apache Kafka versions" list and the rows the service publishes),
    // and including versions this suite's rules were not derived from (`2.3.1`,
    // `3.1.1`, `3.6.0.1`).
    let expected_offered: &[(&str, bool)] = &[
        ("2.3.1", false),
        ("3.1.1", false),
        ("3.6.0", true),
        ("3.6.0.1", false),
        ("3.8.x.kraft", true),
        ("3.9.x", true),
        ("4.2.x.kraft", true),
    ];
    for (version, is_offered) in expected_offered {
        assert_eq!(
            offered(&catalog, version),
            *is_offered,
            "case version={version:?}: expected offered={is_offered}, got the published rows {:?}",
            catalog
                .iter()
                .filter(|(name, _)| name == version)
                .collect::<Vec<_>>()
        );
    }

    // The create refusal's valid-value list is drawn from that same set, narrowed:
    // the versions MSK ADVERTISES for a NEW cluster are ones it still offers, each
    // named once — a narrower set than the offered one, since a version stays
    // published, stays an upgrade SOURCE and stays admitted after the service has
    // stopped steering a cluster that does not exist yet onto it.
    let advertised = |reply: &Reply| -> Vec<String> {
        let message = reply.error_message().to_string();
        let listed = message
            .rsplit_once("Valid values: [")
            .and_then(|(_, tail)| tail.strip_suffix(']'))
            .unwrap_or_else(|| panic!("the refusal must carry a valid-value list, got {message:?}"));
        listed.split(", ").map(str::to_string).collect()
    };
    let refusal = emulator
        .create(&ClusterRequest {
            kafka_version: "9.9.9",
            ..ClusterRequest::named("offered-set")
        })
        .await;
    assert_eq!(
        (refusal.status, refusal.error_code()),
        (400, "BadRequestException"),
        "a create naming an uncatalogued version must be refused, got {} {}",
        refusal.status,
        refusal.body
    );
    let listed: Vec<String> = advertised(&refusal);
    // The lines the EXPRESS brokers alone are offered, read off that category's own
    // refusal: they are the one part of the offered set the service does not install
    // on a running cluster.
    let express_refusal = emulator
        .create(&ClusterRequest {
            kafka_version: "9.9.9",
            instance_type: "express.m7g.large",
            ..ClusterRequest::named("express-offered-set")
        })
        .await;
    assert_eq!(
        (express_refusal.status, express_refusal.error_code()),
        (400, "BadRequestException"),
        "an Express create naming an uncatalogued version must be refused, got {} {}",
        express_refusal.status,
        express_refusal.body
    );
    let express_only: Vec<String> = advertised(&express_refusal)
        .into_iter()
        .filter(|version| !listed.contains(version))
        .collect();

    // The matrix offers a version as a target exactly when the service still
    // SHIPS it to a cluster — every version the catalog publishes as offered,
    // less the lines offered the Express brokers alone, which is the software an
    // in-place update installs — and it offers it from every published version an
    // upgrade to it can start from: an older release of its own build line that the
    // catalog still publishes as offered. A version with no `ACTIVE` row, and one
    // offered the Express brokers alone, is no source's target.
    let matrix = matrix(&emulator).await;
    let versions = distinct_versions(&catalog);
    // Everything a refusal advertises is software the service ships, so the steer it
    // gives a client can never name a version no upgrade could land.
    for version in &listed {
        assert!(
            offered(&catalog, version) && !express_only.contains(version),
            "case version={version:?}: an advertised version must be one the service ships, \
             got the published rows {catalog:?}"
        );
    }
    for version in &versions {
        let is_target = matrix
            .iter()
            .any(|(_, targets)| targets.iter().any(|target| target == version));
        if !offered(&catalog, version) {
            assert!(
                !is_target,
                "case version={version:?}: a version with no ACTIVE row must be no source's \
                 target, got the matrix {matrix:?}"
            );
            continue;
        }
        if express_only.contains(version) {
            assert!(
                !is_target,
                "case version={version:?}: a line offered the Express brokers alone must be no \
                 source's target, got the matrix {matrix:?}"
            );
            continue;
        }
        let has_older_offered_on_line = versions.iter().any(|other| {
            line(other) == line(version)
                && release(other) < release(version)
                && offered(&catalog, other)
        });
        assert!(
            is_target || !has_older_offered_on_line,
            "case version={version:?}: a shipped version with an older offered version on its \
             own line must be that source's upgrade target, got the matrix {matrix:?}"
        );
    }
    let offered_versions: Vec<String> = distinct_versions(&catalog)
        .into_iter()
        .filter(|version| offered(&catalog, version))
        .collect();
    assert!(
        !listed.is_empty(),
        "the refusal must name the versions a create may use, got {listed:?}"
    );
    for (index, version) in listed.iter().enumerate() {
        assert!(
            offered_versions.iter().any(|offered| offered == version),
            "case version={version:?}: the refusal may only name a version with an ACTIVE row, \
             got {listed:?}"
        );
        assert!(
            !listed[..index].contains(version),
            "case version={version:?}: the refusal must name each version once, got {listed:?}"
        );
    }
}

/// RULE: the one move between build lines MSK offers is the crossing from
/// ZooKeeper mode to the KRaft line, and it is offered at ONE release — the final
/// release the catalog publishes an offered ZooKeeper-mode version of, since that
/// is the last release published in both metadata modes. A ZooKeeper-mode cluster
/// below that release is offered no `.kraft` target at all; at it, it is offered
/// the KRaft versions of that same release and no other.
#[tokio::test]
async fn the_crossing_to_kraft_is_offered_at_the_final_zookeeper_release() {
    let emulator = Emulator::new();
    let catalog = published_rows(&emulator).await;
    let versions = distinct_versions(&catalog);

    let crossing = versions
        .iter()
        .filter(|version| offered(&catalog, version) && line(version).is_empty())
        .map(|version| release(version))
        .max()
        .unwrap_or_else(|| panic!("the catalog must offer a ZooKeeper-mode version, got {catalog:?}"));

    let matrix = matrix(&emulator).await;
    // Exhaustive over every ZooKeeper-mode version the catalog publishes — the
    // whole input space of the rule, which is stronger than sampling it.
    let mut crossings = 0usize;
    for (source, targets) in &matrix {
        if !line(source).is_empty() {
            continue;
        }
        let kraft_targets: Vec<&String> = targets
            .iter()
            .filter(|target| line(target) == KRAFT_LINE)
            .collect();
        let expected: Vec<&String> = if release(source) == crossing {
            versions
                .iter()
                .filter(|version| {
                    offered(&catalog, version)
                        && line(version) == KRAFT_LINE
                        && release(version) == crossing
                })
                .collect()
        } else {
            Vec::new()
        };
        assert_eq!(
            kraft_targets, expected,
            "case source={source:?}: the crossing to KRaft is offered at release {crossing:?} \
             only, so expected {expected:?}, got the targets {targets:?}"
        );
        crossings += usize::from(!kraft_targets.is_empty());
    }
    assert!(
        crossings > 0,
        "the catalog must offer the crossing to KRaft somewhere, got the matrix {matrix:?}"
    );
    // The crossing stays inside its own release: no source is offered a KRaft
    // version of another release, and no KRaft source is offered a way back.
    for (source, targets) in &matrix {
        for target in targets {
            if line(source).is_empty() && line(target) == KRAFT_LINE {
                assert_eq!(
                    release(target),
                    release(source),
                    "case source={source:?}: the crossing must stay at one release, got \
                     target {target:?}"
                );
            }
            assert!(
                !(line(source) == KRAFT_LINE && line(target).is_empty()),
                "case source={source:?}: a KRaft cluster is offered no way back to ZooKeeper \
                 mode, got target {target:?}"
            );
        }
    }
}
