// SPDX-License-Identifier: CC0-1.0

//! What a CLIENT reads out of the Apache Kafka version catalog: `ListKafkaVersions`
//! folded to a version -> status map, which is the ordinary read of that page —
//! for each published row, map the version to the status, so a version published
//! more than once is read from its LAST row.
//!
//! RULE: the status a client folds out of the published catalog for a version is
//! the status the service HONOURS for that version. A version that folds to
//! `ACTIVE` is one the service still offers — the upgrade matrix may offer it as a
//! target, and the versions its unsupported-version refusal reports as valid are
//! drawn from that set — and a version that folds to `DEPRECATED` is neither. The
//! versions a CREATE may name are the narrower set that refusal lists, since MSK
//! retires a version for new clusters while still offering it to the clusters
//! already on it, and the refusal's list is exactly what a create is admitted on.
//!
//! Every reading here comes off the WIRE: the fold is computed from the published
//! page and the offered set from the refusal the service renders, so neither side
//! is read from the service's own constants and the two cannot agree by
//! construction. Driven end to end over the same `MultiServiceRouter` the emulator
//! serves from (the shared harness in `tests/all/kafka_harness.rs`).

use serde_json::json;

use crate::kafka_harness::{Emulator, Reply};

/// The two `KafkaVersionStatus` values the model declares (`KafkaVersionStatus` =
/// `ACTIVE | DEPRECATED`).
const ACTIVE: &str = "ACTIVE";
const DEPRECATED: &str = "DEPRECATED";

/// A version string no catalog publishes, which is what makes a create report the
/// versions that ARE offered.
const UNCATALOGUED: &str = "9.9.9";

/// The published rows, as `(version, status)` in published order.
async fn published_rows(emulator: &Emulator) -> Vec<(String, String)> {
    let reply = emulator.call("GET", "/v1/kafka-versions", None).await;
    assert_eq!(
        reply.status, 200,
        "ListKafkaVersions must succeed, got {} {}",
        reply.status, reply.body
    );
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

/// The page folded the way a client folds it: one status per version, the LAST row
/// published for that version, in the order the versions are first published.
fn folded(rows: &[(String, String)]) -> Vec<(String, String)> {
    let mut folded: Vec<(String, String)> = Vec::new();
    for (version, status) in rows {
        match folded.iter_mut().find(|(name, _)| name == version) {
            Some(entry) => entry.1 = status.clone(),
            None => folded.push((version.clone(), status.clone())),
        }
    }
    folded
}

/// The status the fold reports for `version`, or `None` when the catalog publishes
/// no row for it.
fn folded_status(folded: &[(String, String)], version: &str) -> Option<String> {
    folded
        .iter()
        .find(|(name, _)| name == version)
        .map(|(_, status)| status.clone())
}

/// The versions the SERVICE ADVERTISES to a create of standard brokers: the
/// valid-value list its refusal of a version it does not publish carries, in the
/// order that message names them. A narrower set than the versions it still admits
/// — see the fold rule below.
async fn offered_versions(emulator: &Emulator) -> Vec<String> {
    let refusal = create_provisioned(emulator, "catalog-fold-refusal", UNCATALOGUED).await;
    assert_eq!(
        (refusal.status, refusal.error_code()),
        (400, "BadRequestException"),
        "a create naming a version the catalog does not publish must be refused, got {} {}",
        refusal.status,
        refusal.body
    );
    let message = refusal.error_message().to_string();
    let listed = message
        .rsplit_once("Valid values: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("the refusal must carry a valid-value list, got {message:?}"));
    listed.split(", ").map(str::to_string).collect()
}

/// The versions the service advertises to a create of EXPRESS brokers, read the
/// same way off that category's own refusal — so what one category is offered and
/// the other is not comes off the wire rather than out of a constant.
async fn express_offered_versions(emulator: &Emulator) -> Vec<String> {
    let refusal = emulator
        .call(
            "POST",
            "/v1/clusters",
            Some(json!({
                "clusterName": "catalog-fold-express-refusal",
                "kafkaVersion": UNCATALOGUED,
                "numberOfBrokerNodes": 3,
                "brokerNodeGroupInfo": {
                    "instanceType": "express.m7g.large",
                    "clientSubnets": [
                        "subnet-aaaaaaaaaaaaaaaaa",
                        "subnet-bbbbbbbbbbbbbbbbb",
                        "subnet-ccccccccccccccccc",
                    ],
                },
            })),
        )
        .await;
    assert_eq!(
        (refusal.status, refusal.error_code()),
        (400, "BadRequestException"),
        "an Express create naming a version the catalog does not publish must be refused, got \
         {} {}",
        refusal.status,
        refusal.body
    );
    let message = refusal.error_message().to_string();
    let listed = message
        .rsplit_once("Valid values: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("the refusal must carry a valid-value list, got {message:?}"));
    listed.split(", ").map(str::to_string).collect()
}

/// `CreateCluster` for a provisioned cluster on `version`, built here rather than
/// through the harness's own request so the version can be one read off the wire.
async fn create_provisioned(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/v1/clusters",
            Some(json!({
                "clusterName": name,
                "kafkaVersion": version,
                "numberOfBrokerNodes": 3,
                "brokerNodeGroupInfo": {
                    "instanceType": "kafka.m5.large",
                    "clientSubnets": ["subnet-a", "subnet-b", "subnet-c"],
                },
            })),
        )
        .await
}

/// Every version the upgrade matrix offers as a TARGET, each once.
async fn matrix_targets(emulator: &Emulator) -> Vec<String> {
    let matrix = emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await;
    assert_eq!(
        matrix.status, 200,
        "GetCompatibleKafkaVersions must succeed, got {} {}",
        matrix.status, matrix.body
    );
    let mut targets: Vec<String> = Vec::new();
    for entry in matrix
        .at("/compatibleKafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", matrix.body))
    {
        let Some(offered) = entry["targetVersions"].as_array() else {
            continue;
        };
        for target in offered {
            let Some(target) = target.as_str() else {
                panic!("a target that is not a string: {entry}");
            };
            if !targets.iter().any(|seen| seen == target) {
                targets.push(target.to_string());
            }
        }
    }
    targets
}

/// A cluster name out of a version string: the name pattern admits no dot.
fn cluster_name(version: &str) -> String {
    format!("fold-{}", version.replace('.', "-"))
}

/// RULE: the fold is the status the service honours, version for version.
///
/// The table of folds is hand-written from the catalog Amazon MSK publishes (the
/// Developer Guide's "Supported Apache Kafka versions" list, and the rows the
/// service itself publishes), and spans the multiplicities a published version
/// comes in — one offered row, one deprecated row, two rows — across the version
/// spellings the catalog uses, including versions the report this rule came from
/// does not cite (`3.6.0`, `3.7.x.kraft`, `2.3.1`, `2.8.2.tiered`).
#[tokio::test]
async fn the_status_a_client_folds_out_of_the_catalog_is_the_status_the_service_honours() {
    let emulator = Emulator::new();
    let rows = published_rows(&emulator).await;
    let folded = folded(&rows);

    // The fold itself, over the multiplicities a version is published in.
    let expected: &[(&str, &str)] = &[
        ("2.3.1", DEPRECATED),
        ("2.8.2.tiered", DEPRECATED),
        ("3.6.0", ACTIVE),
        ("3.6.0.1", DEPRECATED),
        ("3.7.x.kraft", ACTIVE),
        ("3.9.x", ACTIVE),
        ("4.2.x.kraft", ACTIVE),
    ];
    for (version, status) in expected {
        let actual = folded_status(&folded, version);
        assert_eq!(
            actual.as_deref(),
            Some(*status),
            "case version={version:?}: expected the fold to read {status:?}, got {actual:?} \
             out of the rows {rows:?}"
        );
    }

    // The fold is not vacuous: the catalog really does publish a version on more
    // than one row, so the LAST row is what decides the fold.
    assert!(
        rows.len() > folded.len(),
        "a version published on two rows means more rows than folded versions, got {rows:?}"
    );

    // The versions the service names as the ones a CREATE may use are drawn from
    // the versions the fold reads as ACTIVE, each named once and in published
    // order. It is a NARROWER set: a version stays offered — and stays an upgrade
    // target for the clusters already on it — after the service has stopped taking
    // it for a cluster that does not exist yet.
    let offered = offered_versions(&emulator).await;
    let active: Vec<String> = folded
        .iter()
        .filter(|(_, status)| status == ACTIVE)
        .map(|(version, _)| version.clone())
        .collect();
    assert!(
        !offered.is_empty(),
        "the refusal must name the versions a create may use, got {offered:?}"
    );
    let mut published_at = 0_usize;
    for version in &offered {
        let step = active[published_at..]
            .iter()
            .position(|active| active == version)
            .unwrap_or_else(|| {
                panic!(
                    "case version={version:?}: expected a version the fold reads as {ACTIVE:?}, \
                     named once and in published order, got {offered:?} against {active:?}"
                )
            });
        published_at += step + 1;
    }

    // Every upgrade target the matrix offers folds to ACTIVE: a client is never
    // steered onto a version the catalog reads as deprecated.
    for target in matrix_targets(&emulator).await {
        let status = folded_status(&folded, &target);
        assert_eq!(
            status.as_deref(),
            Some(ACTIVE),
            "case target={target:?}: a version the matrix offers must fold to {ACTIVE:?}, got \
             {status:?}"
        );
    }

    // And the whole point of the read: a create of standard brokers naming a version
    // the fold reads as ACTIVE is accepted — whether or not the refusal's narrower
    // steer advertises it — and the cluster it mints reports that version back,
    // while a line the catalog offers the EXPRESS brokers alone is refused on
    // standard ones.
    let express_only: Vec<String> = express_offered_versions(&emulator)
        .await
        .into_iter()
        .filter(|version| !offered.contains(version))
        .collect();
    for version in &active {
        let created = create_provisioned(&emulator, &cluster_name(version), version).await;
        if express_only.contains(version) {
            assert_eq!(
                (created.status, created.error_code()),
                (400, "BadRequestException"),
                "case version={version:?}: a line offered the Express brokers alone must be \
                 refused on standard ones, got {} {}",
                created.status,
                created.body
            );
            continue;
        }
        assert_eq!(
            created.status, 200,
            "case version={version:?}: a create naming a version the catalog publishes as \
             offered must be accepted, got {} {}",
            created.status, created.body
        );
        let arn = created
            .at("/clusterArn")
            .as_str()
            .unwrap_or_else(|| panic!("case version={version:?}: expected a clusterArn, got {}", created.body))
            .to_string();
        let described = emulator.describe_v2(&arn).await;
        assert_eq!(
            described.status, 200,
            "case version={version:?}: DescribeClusterV2 must answer, got {} {}",
            described.status, described.body
        );
        let reported = described
            .at("/clusterInfo/provisioned/currentBrokerSoftwareInfo/kafkaVersion")
            .as_str()
            .map(str::to_string);
        assert_eq!(
            reported.as_deref(),
            Some(version.as_str()),
            "case version={version:?}: the cluster must report the version it was created on, \
             got {reported:?}"
        );
    }

    // Reading and creating changed nothing about the catalog a later caller sees.
    let again = published_rows(&emulator).await;
    assert_eq!(
        again, rows,
        "the published catalog must be unchanged by reads and creates, got {again:?}"
    );
}

/// RULE, the other half of the same read: a version the fold reads as `DEPRECATED`
/// is offered for nothing — it is named among neither the valid values the refusal
/// reports nor the targets the matrix offers, and a create naming it is refused —
/// while the service still KNOWS it, so the matrix goes on publishing it as an
/// upgrade SOURCE, which is how a cluster already running it is moved off it.
///
/// The rows differ along the spelling of the deprecated version (a three-component
/// release, a four-component one, the storage-variant line), and name versions the
/// report this rule came from does not cite.
#[tokio::test]
async fn a_version_the_fold_reads_as_deprecated_is_offered_for_nothing() {
    let emulator = Emulator::new();
    let rows = published_rows(&emulator).await;
    let folded = folded(&rows);
    let offered = offered_versions(&emulator).await;
    let targets = matrix_targets(&emulator).await;

    let deprecated: Vec<&(String, String)> = folded
        .iter()
        .filter(|(_, status)| status == DEPRECATED)
        .collect();
    assert!(
        !deprecated.is_empty(),
        "this rule needs a version the fold reads as {DEPRECATED:?}, got {folded:?}"
    );
    for (version, _) in &deprecated {
        assert!(
            !offered.iter().any(|offered| offered == version),
            "case version={version:?}: a deprecated version must not be named among the offered \
             versions, got {offered:?}"
        );
        assert!(
            !targets.iter().any(|target| target == version),
            "case version={version:?}: a deprecated version must not be offered as an upgrade \
             target, got {targets:?}"
        );
    }

    // The service still knows such a version: the matrix publishes it as an upgrade
    // SOURCE, which is what the version-update surface moves a cluster off, while a
    // create naming it is refused.
    let sources = matrix_sources(&emulator).await;
    for version in ["2.3.1", "2.4.1.1", "2.8.2.tiered"] {
        let status = folded_status(&folded, version);
        assert_eq!(
            status.as_deref(),
            Some(DEPRECATED),
            "case version={version:?}: this row needs a deprecated version, got {status:?}"
        );
        assert!(
            sources.iter().any(|source| source == version),
            "case version={version:?}: the matrix must still publish it as an upgrade source, \
             got {sources:?}"
        );
        let created = create_provisioned(&emulator, &cluster_name(version), version).await;
        assert_eq!(
            (created.status, created.error_code()),
            (400, "BadRequestException"),
            "case version={version:?}: a create naming a version MSK no longer builds a new \
             cluster on must be refused, got {} {}",
            created.status,
            created.body
        );
    }
}

/// Every version the upgrade matrix publishes as a SOURCE, in published order.
async fn matrix_sources(emulator: &Emulator) -> Vec<String> {
    let matrix = emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await;
    assert_eq!(
        matrix.status, 200,
        "GetCompatibleKafkaVersions must succeed, got {} {}",
        matrix.status, matrix.body
    );
    matrix
        .at("/compatibleKafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", matrix.body))
        .iter()
        .map(|entry| {
            entry["sourceVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without a sourceVersion: {entry}"))
                .to_string()
        })
        .collect()
}

/// RULE: every row the listing publishes carries one of the two statuses the model
/// declares, so the fold reads a `KafkaVersionStatus` for every version and both
/// statuses are really published.
#[tokio::test]
async fn every_published_row_carries_a_status_the_model_declares() {
    let emulator = Emulator::new();
    let rows = published_rows(&emulator).await;
    let unexpected: Vec<&(String, String)> = rows
        .iter()
        .filter(|(_, status)| status != ACTIVE && status != DEPRECATED)
        .collect();
    assert!(
        unexpected.is_empty(),
        "every row must carry a status the model declares, got {unexpected:?}"
    );
    let folded = folded(&rows);
    let statuses: Vec<&String> = folded.iter().map(|(_, status)| status).collect();
    assert!(
        statuses.iter().any(|status| *status == ACTIVE)
            && statuses.iter().any(|status| *status == DEPRECATED),
        "the fold must read both statuses the model declares, got {folded:?}"
    );
}
