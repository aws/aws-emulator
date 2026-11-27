// SPDX-License-Identifier: CC0-1.0

//! The upgrade ladder `GetCompatibleKafkaVersions` publishes under each source,
//! and the update gate that same ladder admits.
//!
//! RULE: every row of the published matrix carries the ladder of ITS OWN source —
//! the versions the service would install on a cluster running it: software it
//! still SHIPS to a new cluster (the offering a create's refusal advertises, which
//! is narrower than the versions the catalog publishes an `ACTIVE` row for), on
//! that source's own build line and naming a strictly newer release, plus the one
//! cross-line move MSK offers, the crossing to the `.kraft` line at the final
//! release it still offers in ZooKeeper mode. A ladder is published only OFF a
//! version the service still offers, so a source the catalog publishes as
//! deprecated is TERMINAL and carries an empty list, as is a source with nothing
//! shipped above it on its line. The same ladder is what
//! `UpdateClusterKafkaVersion` admits, so the matrix and the gate are one
//! derivation seen twice — never one blanket list repeated under every source.
//!
//! Asserted from the WIRE alone and over a TABLE whose rows differ along what the
//! rule generalises over: the source's build line (ZooKeeper, the `.tiered`
//! spelling, `.kraft`, the `link` line MSK publishes no path off), its lifecycle
//! status, whether the service still ships it, where it sits on its line (the
//! oldest release, the middle, the top) and whether its release is the one the
//! crossing to KRaft is offered at.
//!
//! Driven end to end over the same `MultiServiceRouter` the emulator serves from
//! (the shared harness in `tests/all/kafka_harness.rs`).

use std::collections::{BTreeMap, BTreeSet};

use crate::kafka_harness::{
    kafka_version_line as line, kafka_version_release as release, pct, poll_until_active,
    ClusterRequest, Emulator, Reply, KRAFT_LINE, OTHER_LINE,
};

/// A `KafkaVersionStatus` value: a version the service still offers.
const ACTIVE: &str = "ACTIVE";

/// A version no catalog publishes, so a create naming it is refused with the whole
/// valid-value list of the broker category it asked for.
const NO_SUCH_VERSION: &str = "0.0.0-no-such-version";

/// The whole published matrix: `GetCompatibleKafkaVersions` with no `ClusterArn`,
/// as `(sourceVersion, targetVersions)` rows in the order reported.
async fn matrix(emulator: &Emulator) -> Vec<(String, Vec<String>)> {
    let reply = emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await;
    assert_eq!(
        reply.status, 200,
        "the published matrix must succeed, got {} {}",
        reply.status, reply.body
    );
    rows(&reply)
}

/// The ladder the same reader publishes for ONE cluster:
/// `GET /v1/compatible-kafka-versions?clusterArn=<arn>`.
async fn ladder_for(emulator: &Emulator, arn: &str) -> Vec<String> {
    let reply = emulator
        .call(
            "GET",
            &format!("/v1/compatible-kafka-versions?clusterArn={}", pct(arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "the ladder of a stored cluster must succeed, got {} {}",
        reply.status, reply.body
    );
    let rows = rows(&reply);
    assert_eq!(
        rows.len(),
        1,
        "a cluster's ladder is one row, got {rows:?}"
    );
    rows[0].1.clone()
}

/// The `(sourceVersion, targetVersions)` rows of a reply.
fn rows(reply: &Reply) -> Vec<(String, Vec<String>)> {
    reply
        .at("/compatibleKafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            let source = entry["sourceVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("a row without a sourceVersion: {entry}"))
                .to_string();
            let targets = entry["targetVersions"]
                .as_array()
                .unwrap_or_else(|| panic!("a row without targetVersions: {entry}"))
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

/// The catalog as the wire publishes it: every version with the statuses its rows
/// carry, read from `ListKafkaVersions`.
async fn catalog(emulator: &Emulator) -> BTreeMap<String, Vec<String>> {
    let reply = emulator.call("GET", "/v1/kafka-versions", None).await;
    assert_eq!(
        reply.status, 200,
        "the version catalog must succeed, got {} {}",
        reply.status, reply.body
    );
    let mut published: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
    {
        let version = entry["version"]
            .as_str()
            .unwrap_or_else(|| panic!("a row without a version: {entry}"))
            .to_string();
        let status = entry["status"]
            .as_str()
            .unwrap_or_else(|| panic!("a row without a status: {entry}"))
            .to_string();
        published.entry(version).or_default().push(status);
    }
    assert!(
        !published.is_empty(),
        "the catalog must publish something, got {}",
        reply.body
    );
    published
}

/// Whether the catalog still OFFERS `version`: whether it publishes any [`ACTIVE`]
/// row for it, which is what makes a version an upgrade SOURCE.
fn offered(catalog: &BTreeMap<String, Vec<String>>, version: &str) -> bool {
    catalog
        .get(version)
        .is_some_and(|statuses| statuses.iter().any(|status| status == ACTIVE))
}

/// The versions the service still SHIPS to a cluster — the software an in-place
/// update installs and a create of the standard brokers is admitted on — read off
/// the wire: the versions the catalog publishes an [`ACTIVE`] row for, less the
/// lines it advertises to the EXPRESS brokers alone, since a line offered those
/// brokers only is not software installed on a running cluster.
///
/// Read from the published catalog rather than from the valid-value list a create's
/// refusal carries: that list is MSK's narrower STEER for a new cluster, a subset
/// of what it still ships, so reading it here would make an upgrade ladder follow
/// the advertisement instead of the catalog.
async fn shipped(emulator: &Emulator) -> BTreeSet<String> {
    let published = catalog(emulator).await;
    let express_only = express_only_versions(emulator).await;
    let shipped: BTreeSet<String> = published
        .iter()
        .filter(|(version, statuses)| {
            statuses.iter().any(|status| status == ACTIVE) && !express_only.contains(*version)
        })
        .map(|(version, _)| version.clone())
        .collect();
    assert!(
        !shipped.is_empty(),
        "the catalog must publish a version the service still ships, got {published:?}"
    );
    shipped
}

/// The lines MSK advertises to the EXPRESS brokers alone, read off the wire: the
/// versions that category's create refusal names as valid and the STANDARD one does
/// not.
async fn express_only_versions(emulator: &Emulator) -> BTreeSet<String> {
    let standard = advertised_for(emulator, "kafka.m5.large").await;
    advertised_for(emulator, "express.m7g.large")
        .await
        .into_iter()
        .filter(|version| !standard.contains(version))
        .collect()
}

/// The valid-value list a create of `instance_type`'s brokers is refused with, read
/// off that refusal's own sentence.
async fn advertised_for(emulator: &Emulator, instance_type: &'static str) -> BTreeSet<String> {
    let reply = emulator
        .create(&ClusterRequest {
            kafka_version: NO_SUCH_VERSION,
            instance_type,
            ..ClusterRequest::named("advertised-set")
        })
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "a create naming no published version must be refused, got {} {}",
        reply.status,
        reply.body
    );
    let message = reply.error_message().to_string();
    let listed = message
        .rsplit_once("Valid values: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("the refusal must advertise a valid-value list, got {message:?}"))
        .to_string();
    let advertised: BTreeSet<String> = listed.split(", ").map(str::to_string).collect();
    assert!(
        !advertised.is_empty(),
        "the refusal must name the versions a create may use, got {message:?}"
    );
    advertised
}

/// The ladder the published facts imply for `source`, computed here from the
/// catalog and the shipped set rather than from the service's own helper — so the
/// answer under test is never compared with itself.
fn implied_ladder(
    catalog: &BTreeMap<String, Vec<String>>,
    shipped: &BTreeSet<String>,
    source: &str,
) -> BTreeSet<String> {
    if !offered(catalog, source) {
        return BTreeSet::new();
    }
    // The release the crossing to the KRaft line is offered at: the newest the
    // catalog publishes an offered ZooKeeper-mode version of, that being the last
    // release published in both metadata modes.
    let crossing = catalog
        .keys()
        .filter(|version| offered(catalog, version) && line(version).is_empty())
        .map(|version| release(version.as_str()))
        .max()
        .unwrap_or_default();
    shipped
        .iter()
        .filter(|target| {
            if line(target) == line(source) {
                return line(source) != OTHER_LINE && release(target) > release(source);
            }
            line(source).is_empty()
                && line(target) == KRAFT_LINE
                && release(target) == release(source)
                && release(source) == crossing
        })
        .cloned()
        .collect()
}

/// RULE: each published row carries its own source's ladder — exactly the versions
/// the service ships that an upgrade from that source may land. Checked
/// EXHAUSTIVELY over every row the matrix publishes (the input space is the
/// catalog, which is small enough to walk) and then spelled out for a table of
/// named sources, so a ladder that drifts says which source it drifted for.
#[tokio::test]
async fn every_published_row_carries_its_own_sources_ladder() {
    let emulator = Emulator::new();
    let catalog = catalog(&emulator).await;
    let shipped = shipped(&emulator).await;
    let matrix = matrix(&emulator).await;

    for (source, targets) in &matrix {
        let implied = implied_ladder(&catalog, &shipped, source);
        let answered: BTreeSet<String> = targets.iter().cloned().collect();
        assert_eq!(
            answered, implied,
            "case source={source:?}: expected the ladder {implied:?}, got {targets:?}"
        );
        assert_eq!(
            answered.len(),
            targets.len(),
            "case source={source:?}: a target is offered once, got {targets:?}"
        );
    }

    // The named rows this rule is stated over, hand-written from what the service
    // publishes (every version here is one the catalog lists) and the versions it
    // ships, and including sources the rule was not derived from (`2.8.2.tiered`,
    // `3.6.0.1`, `3.7.x`).
    let expected: &[(&str, &[&str])] = &[
        // The oldest release the catalog publishes, long deprecated: terminal,
        // though the service ships much newer ZooKeeper-mode software.
        ("1.1.1", &[]),
        // The `.tiered` storage spelling of a deprecated release: on the
        // ZooKeeper line, and terminal for the same reason.
        ("2.8.2.tiered", &[]),
        // Still offered and still shipped: a ladder of the shipped ZooKeeper-mode
        // versions above it, which is every version the catalog publishes as
        // offered on that line above this release.
        ("3.6.0", &["3.7.x", "3.8.x", "3.9.x"]),
        ("3.6.0.1", &[]),
        ("3.7.x", &["3.8.x", "3.9.x"]),
        // A KRaft source climbs its own line only; there is no way back to
        // ZooKeeper mode.
        (
            "3.7.x.kraft",
            &["3.8.x.kraft", "3.9.x.kraft", "4.0.x.kraft", "4.1.x.kraft"],
        ),
        ("3.8.x", &["3.9.x"]),
        ("3.8.x.kraft", &["3.9.x.kraft", "4.0.x.kraft", "4.1.x.kraft"]),
        // The `link` line: a spelling MSK publishes a row for and no upgrade path
        // off.
        ("3.8.link", &[]),
        // The final release still offered in ZooKeeper mode: nothing newer on its
        // own line, so its one target is the crossing to KRaft.
        ("3.9.x", &["3.9.x.kraft"]),
        ("3.9.x.kraft", &["4.0.x.kraft", "4.1.x.kraft"]),
        ("4.0.x.kraft", &["4.1.x.kraft"]),
        // The top of the shipped ladder, and a published row above it the service
        // no longer ships: both terminal.
        ("4.1.x.kraft", &[]),
        ("4.2.x.kraft", &[]),
    ];
    for (source, ladder) in expected {
        assert!(
            catalog.contains_key(*source),
            "case source={source:?}: this row needs a version the catalog publishes, got {:?}",
            catalog.keys().collect::<Vec<_>>()
        );
        let answered: Vec<&Vec<String>> = matrix
            .iter()
            .filter(|(published, _)| published.as_str() == *source)
            .map(|(_, targets)| targets)
            .collect();
        assert!(
            !answered.is_empty(),
            "case source={source:?}: the matrix must publish a row for every published \
             version, got {matrix:?}"
        );
        let wanted: BTreeSet<String> = ladder.iter().map(|target| (*target).to_string()).collect();
        for targets in answered {
            let got: BTreeSet<String> = targets.iter().cloned().collect();
            assert_eq!(
                got, wanted,
                "case source={source:?}: expected the ladder {wanted:?}, got {targets:?}"
            );
        }
    }
}

/// RULE: the ladder is per SOURCE, not one list the matrix repeats. Two sources on
/// different build lines are offered disjoint sets, no source is its own target,
/// no deprecated source is offered anything, and the matrix publishes more than
/// one distinct ladder — a blanket list would fail all four.
#[tokio::test]
async fn the_matrix_is_a_per_source_graph_and_not_one_blanket_list() {
    let emulator = Emulator::new();
    let catalog = catalog(&emulator).await;
    let matrix = matrix(&emulator).await;

    let ladders: BTreeSet<Vec<String>> = matrix
        .iter()
        .map(|(_, targets)| targets.clone())
        .collect();
    assert!(
        ladders.len() > 2,
        "expected the matrix to publish several distinct ladders, got {ladders:?}"
    );
    assert!(
        matrix.iter().any(|(_, targets)| targets.is_empty()),
        "expected a terminal source carrying an empty ladder, got {matrix:?}"
    );
    assert!(
        matrix.iter().any(|(_, targets)| !targets.is_empty()),
        "expected a source carrying a ladder, got {matrix:?}"
    );

    for (source, targets) in &matrix {
        assert!(
            !targets.contains(source),
            "case source={source:?}: a version is no upgrade of itself, got {targets:?}"
        );
        if !offered(&catalog, source) {
            assert!(
                targets.is_empty(),
                "case source={source:?}: the service publishes no path off a version it has \
                 deprecated, got {targets:?}"
            );
        }
        for target in targets {
            let same_line = line(target) == line(source) && line(source) != OTHER_LINE;
            let crossing = line(source).is_empty()
                && line(target) == KRAFT_LINE
                && release(target) == release(source);
            assert!(
                same_line || crossing,
                "case source={source:?}: target {target:?} is on another build line, so the \
                 ladder is not this source's own, got {targets:?}"
            );
            if same_line {
                assert!(
                    release(target) > release(source),
                    "case source={source:?}: target {target:?} is no newer release, got \
                     {targets:?}"
                );
            }
        }
    }

    // Spelled out across the lines: what a KRaft source is offered and what a
    // ZooKeeper-mode source is offered have nothing in common but the crossing.
    for (zookeeper, kraft) in [("3.8.x", "3.8.x.kraft"), ("3.6.0", "3.7.x.kraft")] {
        let of = |version: &str| -> BTreeSet<String> {
            matrix
                .iter()
                .find(|(source, _)| source.as_str() == version)
                .map(|(_, targets)| targets.iter().cloned().collect())
                .unwrap_or_else(|| panic!("the matrix must publish a row for {version:?}"))
        };
        let (zookeeper_ladder, kraft_ladder) = (of(zookeeper), of(kraft));
        assert!(
            zookeeper_ladder
                .intersection(&kraft_ladder)
                .next()
                .is_none(),
            "case {zookeeper:?} vs {kraft:?}: sources on different build lines must be offered \
             different things, got {zookeeper_ladder:?} and {kraft_ladder:?}"
        );
    }
}

/// RULE: the published ladder IS the update gate — a cluster running a source is
/// offered exactly the targets an `UpdateClusterKafkaVersion` of that cluster is
/// admitted on, so a client that plans an upgrade off the matrix is never refused,
/// and a version the matrix withholds is never installed.
///
/// Rows: a ZooKeeper-mode source in the middle of its line, the source at the
/// release the crossing to KRaft is offered at, a KRaft source with a ladder, and
/// the top of the KRaft ladder, which is terminal.
#[tokio::test]
async fn the_published_ladder_is_the_update_gate() {
    let emulator = Emulator::new();
    let shipped = shipped(&emulator).await;

    // Each row names a source a create admits (the only way a cluster reaches the
    // registry) and ONE target to probe: a version the matrix publishes for it, or
    // — where the row's ladder is empty — a published version it withholds.
    let rows: &[(&str, &'static str, &str)] = &[
        ("mid-zookeeper-line", "3.8.x", "3.9.x"),
        ("the-kraft-crossing", "3.9.x", "3.9.x.kraft"),
        ("mid-kraft-line", "3.8.x.kraft", "4.1.x.kraft"),
        ("terminal-kraft-source", "4.1.x.kraft", "4.2.x.kraft"),
    ];

    for &(case, source, target) in rows {
        assert!(
            shipped.contains(source),
            "case {case}: this row needs a source a create admits, got the offering {shipped:?}"
        );
        let arn = emulator
            .create_ok(&ClusterRequest {
                kafka_version: source,
                ..ClusterRequest::named(&format!("gate-{case}"))
            })
            .await;
        let version = poll_until_active(&emulator, &arn, case).await;
        let ladder = ladder_for(&emulator, &arn).await;
        let published = ladder.iter().any(|offered| offered.as_str() == target);

        let reply = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/version", pct(&arn)),
                Some(serde_json::json!({
                    "currentVersion": version,
                    "targetKafkaVersion": target,
                })),
            )
            .await;
        match published {
            true => assert_eq!(
                reply.status, 200,
                "case {case}: the matrix publishes {target:?} for a cluster running {source:?}, \
                 so the update must be accepted, got {} {}",
                reply.status, reply.body
            ),
            false => {
                assert_eq!(
                    (reply.status, reply.error_code()),
                    (400, "BadRequestException"),
                    "case {case}: the matrix withholds {target:?} for a cluster running \
                     {source:?} (ladder {ladder:?}), so the update must be refused, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.at("/invalidParameter").as_str(),
                    Some("targetKafkaVersion"),
                    "case {case}: the refusal must name the member at fault, got {}",
                    reply.body
                );
            }
        }
    }
}

/// RULE: publishing the matrix is a READ — it answers the same table every time
/// and changes nothing a later caller sees, cluster records included.
#[tokio::test]
async fn publishing_the_matrix_changes_nothing() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_ok(&ClusterRequest::named("matrix-read"))
        .await;
    let before = emulator.describe(&arn).await;
    let first = matrix(&emulator).await;
    let second = matrix(&emulator).await;
    let after = emulator.describe(&arn).await;
    assert_eq!(
        first, second,
        "two reads of the matrix must answer the same table, got {first:?} then {second:?}"
    );
    assert_eq!(
        before.at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion"),
        after.at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion"),
        "reading the matrix must not move a cluster's Apache Kafka version, got {} then {}",
        before.body,
        after.body
    );
}
