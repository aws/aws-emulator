// SPDX-License-Identifier: CC0-1.0

//! The two version READ paths answer from ONE catalog: `ListKafkaVersions`
//! publishes it and `GetCompatibleKafkaVersions` derives its matrix from the same
//! list, so neither can offer a version the other does not know.
//!
//! Driven end to end over the same `MultiServiceRouter` the emulator serves from,
//! and asserted from the WIRE alone: the expectations below are computed from what
//! `ListKafkaVersions` reported, not from the service's own catalog constant, so a
//! matrix derived from a second, drifting table would fail here.
//!
//! The invariants are checked EXHAUSTIVELY over every source the matrix reports —
//! the input space is the catalog itself, which is small enough to walk, and that
//! is stronger than sampling it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::kafka_harness::{
    kafka_version_line as line, kafka_version_release as release, ClusterRequest, Emulator, Reply,
    OTHER_LINE,
};

/// A `KafkaVersionStatus` value: a version the service still offers.
const ACTIVE: &str = "ACTIVE";

/// `ListKafkaVersions`, which the model binds to `GET /v1/kafka-versions`.
async fn list_versions(emulator: &Emulator) -> Reply {
    emulator.call("GET", "/v1/kafka-versions", None).await
}

/// `GetCompatibleKafkaVersions` with no `ClusterArn` — the member is optional, and
/// omitting it asks for the whole matrix.
async fn matrix(emulator: &Emulator) -> Reply {
    emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await
}

/// The catalog as the wire reports it: the statuses published for each version,
/// in published order. A version the catalog publishes under more than one row
/// carries one status per row, and the same row is never published twice.
fn published(reply: &Reply) -> BTreeMap<String, Vec<String>> {
    let mut catalog: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
    {
        let version = entry["version"]
            .as_str()
            .unwrap_or_else(|| panic!("an entry without a version: {entry}"))
            .to_string();
        let status = entry["status"]
            .as_str()
            .unwrap_or_else(|| panic!("an entry without a status: {entry}"))
            .to_string();
        let rows = catalog.entry(version.clone()).or_default();
        assert!(
            !rows.contains(&status),
            "case version={version:?}: the catalog publishes a row once, got {}",
            reply.body
        );
        rows.push(status);
    }
    catalog
}

/// Whether the catalog still OFFERS `version`: whether it publishes any
/// [`ACTIVE`] row for it, which is what makes a version an upgrade target.
fn offered(catalog: &BTreeMap<String, Vec<String>>, version: &str) -> bool {
    catalog
        .get(version)
        .is_some_and(|statuses| statuses.iter().any(|status| status == ACTIVE))
}

/// The release at which the catalog offers the KRaft migration: the newest
/// release it publishes an OFFERED ZooKeeper-mode version of, which is the last
/// release published in both metadata modes.
fn migration_release(catalog: &BTreeMap<String, Vec<String>>) -> Vec<u32> {
    catalog
        .keys()
        .filter(|version| offered(catalog, version) && line(version).is_empty())
        .map(|version| release(version.as_str()))
        .max()
        .unwrap_or_default()
}

/// The matrix as `(sourceVersion, targetVersions)` pairs, in the order reported.
fn rows(reply: &Reply) -> Vec<(String, Vec<String>)> {
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

/// RULE: both read paths answer from one catalog — every source the matrix reports
/// is a version the listing publishes, every target is one the listing reports as
/// still offered, and the matrix covers every published entry.
#[tokio::test]
async fn the_matrix_is_derived_from_the_published_catalog() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);
    let matrix = rows(&matrix(&emulator).await);

    assert!(
        !catalog.is_empty() && !matrix.is_empty(),
        "both read paths must answer something, got {} catalog entries and {} matrix rows",
        catalog.len(),
        matrix.len()
    );
    for (source, targets) in &matrix {
        assert!(
            catalog.contains_key(source),
            "case source={source:?}: a matrix source must be a published version, got {:?}",
            catalog.keys().collect::<Vec<_>>()
        );
        for target in targets {
            assert!(
                offered(&catalog, target),
                "case source={source:?}: target {target:?} must be a version the catalog still \
                 offers, got {:?}",
                catalog.get(target)
            );
        }
    }
    // Every published version is a source the matrix answers for, so a client
    // choosing an upgrade finds the row for whatever its cluster runs — and
    // answers for it ONCE, however many rows the catalog publishes it under: the
    // matrix answers about versions, not about catalog rows.
    let sources: BTreeSet<&String> = matrix.iter().map(|(source, _)| source).collect();
    for version in catalog.keys() {
        assert!(
            sources.contains(version),
            "case version={version:?}: the matrix must answer for every published version, \
             got {sources:?}"
        );
    }
    assert_eq!(
        sources.len(),
        matrix.len(),
        "every published version must be a source exactly once, got {matrix:?}"
    );
}

/// RULE: the matrix is a per-source upgrade GRAPH, not one blanket list: each
/// source's targets are the versions the catalog permits an upgrade to FROM it —
/// still offered, on that source's own build line and strictly newer, or, at the
/// one release the catalog offers the crossing at (its final offered
/// ZooKeeper-mode release), the KRaft version of that release — so a terminal
/// version reports an EMPTY list and two sources on different lines are offered
/// different things.
#[tokio::test]
async fn each_source_is_offered_only_the_upgrades_its_own_line_permits() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);
    let matrix = rows(&matrix(&emulator).await);
    let crossing = migration_release(&catalog);

    for (source, targets) in &matrix {
        // No self-upgrade, and no duplicate: a version is offered once or not at
        // all.
        let distinct: BTreeSet<&String> = targets.iter().collect();
        assert_eq!(
            distinct.len(),
            targets.len(),
            "case source={source:?}: a target must be offered once, got {targets:?}"
        );
        assert!(
            !targets.contains(source),
            "case source={source:?}: a version is not an upgrade of itself, got {targets:?}"
        );
        for target in targets {
            let same_line = line(target) == line(source) && line(source) != OTHER_LINE;
            let kraft_migration = line(source).is_empty()
                && line(target) == "kraft"
                && release(target) == release(source)
                && release(source) == crossing;
            assert!(
                same_line || kraft_migration,
                "case source={source:?}: target {target:?} is on another build line, on a \
                 line with no upgrade path, or at a release the crossing to KRaft is not \
                 offered at, so MSK does not offer it, got {targets:?}"
            );
            if same_line {
                assert!(
                    release(target) > release(source),
                    "case source={source:?}: target {target:?} is not a newer release, \
                     got {targets:?}"
                );
            }
        }
        // And every version the catalog permits is offered: the row is the whole
        // subset, not a sample of it.
        let permitted: BTreeSet<&String> = catalog
            .keys()
            .filter(|version| offered(&catalog, version))
            .filter(|version| {
                if line(version) == line(source) {
                    line(source) != OTHER_LINE && release(version) > release(source)
                } else {
                    line(source).is_empty()
                        && line(version) == "kraft"
                        && release(version) == release(source)
                        && release(source) == crossing
                }
            })
            .collect();
        assert_eq!(
            distinct, permitted,
            "case source={source:?}: expected exactly the permitted upgrades {permitted:?}, \
             got {targets:?}"
        );
    }

    // The graph really is per source: some source has nothing left to upgrade to,
    // and the rows do not all carry one shared list.
    assert!(
        matrix.iter().any(|(_, targets)| targets.is_empty()),
        "expected a terminal source with no upgrade target, got {matrix:?}"
    );
    let shapes: BTreeSet<&Vec<String>> = matrix.iter().map(|(_, targets)| targets).collect();
    assert!(
        shapes.len() > 1,
        "expected the matrix to offer different targets per source, got {matrix:?}"
    );
}

/// RULE: the catalog is published metadata, so it carries the variant lines the
/// service offers and both support statuses — a client can see which versions are
/// on the way out, and can select a KRaft version at all.
#[tokio::test]
async fn the_catalog_carries_the_variant_lines_and_both_statuses() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);

    for status in [ACTIVE, "DEPRECATED"] {
        assert!(
            catalog
                .values()
                .any(|statuses| statuses.iter().any(|served| served == status)),
            "case status={status:?}: the catalog must publish it, got {catalog:?}"
        );
    }
    // Each build line the service publishes carries at least one entry, and the
    // KRaft line carries an offered one — a client that wants a KRaft cluster has
    // a version to ask for.
    let lines: BTreeSet<&str> = catalog.keys().map(|version| line(version)).collect();
    for expected in ["", "kraft"] {
        assert!(
            lines.contains(expected),
            "case line={expected:?}: the catalog must publish versions on it, got {lines:?}"
        );
    }
    assert!(
        catalog
            .keys()
            .any(|version| line(version) == "kraft" && offered(&catalog, version)),
        "the catalog must offer a KRaft version, got {catalog:?}"
    );
    // Every reported entry is one the published catalog holds, under a modelled
    // status.
    let listed = list_versions(&emulator).await;
    let entries = listed
        .at("/kafkaVersions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    for entry in &entries {
        let version = entry["version"].as_str().unwrap_or_default();
        assert!(
            catalog.contains_key(version),
            "case version={version:?}: every reported entry must be in the published catalog"
        );
        assert!(
            matches!(entry["status"].as_str(), Some(ACTIVE | "DEPRECATED")),
            "case version={version:?}: expected a modelled status, got {}",
            entry["status"]
        );
    }
    // Reading the catalog twice answers the same thing: it is published metadata,
    // not caller state.
    let again = published(&list_versions(&emulator).await);
    assert_eq!(
        again, catalog,
        "the catalog must not change because it was read"
    );
    assert_eq!(
        listed.at("/kafkaVersions"),
        &Value::Array(entries),
        "the listing must report the same entries on a second read"
    );
}

/// RULE: the LAST row the catalog publishes for a version carries the status the
/// service OFFERS that version under, so the catalog's final word about a version
/// agrees with the gate that accepts it — a client reducing the published rows to
/// one status per version (a version has one lifecycle status) reads the version
/// the way a create naming it is answered.
///
/// Checked EXHAUSTIVELY over every version the catalog publishes, against the
/// offered set read from the WIRE: the valid-value list a create refusal carries,
/// which is the versions the service offers for a new cluster.
#[tokio::test]
async fn the_last_published_row_is_the_status_the_service_offers() {
    let emulator = Emulator::new();
    let listed = list_versions(&emulator).await;
    let mut last_row: BTreeMap<String, String> = BTreeMap::new();
    for entry in listed
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", listed.body))
    {
        let version = entry["version"].as_str().unwrap_or_default().to_string();
        let status = entry["status"].as_str().unwrap_or_default().to_string();
        last_row.insert(version, status);
    }
    assert!(
        !last_row.is_empty(),
        "the catalog must publish versions, got {}",
        listed.body
    );

    // The offered set, as the service itself renders it: a create naming a version
    // the catalog does not publish is refused with the versions it DOES offer.
    let refusal = emulator
        .create(&ClusterRequest {
            kafka_version: "9.9.9",
            ..ClusterRequest::named("offered-set-from-the-wire")
        })
        .await;
    assert_eq!(
        (refusal.status, refusal.error_code()),
        (400, "BadRequestException"),
        "a create naming an uncatalogued version must be refused, got {} {}",
        refusal.status,
        refusal.body
    );
    let message = refusal.error_message().to_string();
    let offered: BTreeSet<&str> = message
        .rsplit_once("Valid values: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("the refusal must carry a valid-value list, got {message:?}"))
        .split(", ")
        .collect();

    for (version, status) in &last_row {
        assert_eq!(
            status == ACTIVE,
            offered.contains(version.as_str()),
            "case version={version:?}: the last published row says {status:?}, while the \
             offered set is {offered:?}"
        );
    }
    // Not vacuous: the catalog publishes both statuses as some version's last row.
    assert!(
        last_row.values().any(|status| status == ACTIVE)
            && last_row.values().any(|status| status != ACTIVE),
        "both lifecycle statuses must appear as a version's last row, got {last_row:?}"
    );
}
