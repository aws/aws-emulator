// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONTENTS of the three static reference catalogs this
//! service publishes: the upgrade-compatibility matrix `GetCompatibleVersions` reports,
//! the per-instance-type storage limits `DescribeInstanceTypeLimits` reports, and the
//! order `DescribeReservedInstanceOfferings` publishes its offering catalog in.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the per-operation calls from the suites that own those operations
//! ([`super::compatible_versions`], [`super::instance_type_limits`],
//! [`super::reserved_instance_offerings`]); this suite adds no second copy of them.
//!
//! The rules under test, each over a SET of inputs rather than one recorded call:
//!
//! * an upgrade walks the minors of a release LINE and leaves that line only from its
//!   END: a source's targets are the later releases of its own line, plus — when it is
//!   the newest release of that line — every release of the line that immediately
//!   succeeds it, and nothing further. The Elasticsearch 7 releases and the OpenSearch 1
//!   releases are one line, OpenSearch 1.0 being the fork of Elasticsearch 7.10. The
//!   table is published from the oldest release an in-place upgrade is offered for;
//! * the storage limits are the REQUESTED instance type's: the sub-types published for
//!   it, and, for each of them, the lesser of what the sub-type publishes and what that
//!   instance's own EBS attachment carries — so the burstable class publishes its
//!   attachment's volume, IOPS and throughput ceilings at every one of its sizes, and
//!   publishes neither the provisioned-IOPS sub-type (its attachment provisions nothing)
//!   nor the magnetic one, which is a previous-generation family's. The published ceiling
//!   is also the one `CreateDomain` enforces;
//! * the offering catalog is published in the catalog's own order — by instance type,
//!   then term, then payment option — and a page of it is a page of that order.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::RESERVED_INSTANCE_PAYMENT_OPTION_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    published_instance_types, RESERVED_INSTANCE_TERMS,
};

use super::compatible_versions::{compatible, entries, target_versions};
use super::domain_lifecycle::{string_member, Emulator, SETTLE_WAIT};
use super::instance_type_limits::{
    describe_limits, limits_by_role, StorageRow, MAXIMUM_STANDARD_VOLUME_SIZE,
    MINIMUM_VOLUME_SIZE, STORAGE_ROWS,
};
use super::reserved_instance_offerings::{describe, whole_catalog};

/// The engine release the storage rows are asked on: one that offers every family they
/// name.
const STORAGE_VERSION: &str = "OpenSearch_2.19";

/// The IOPS floor a `gp3` volume is provisioned from, and the one a provisioned-IOPS
/// volume is: the sub-types' own figures, which no instance narrows.
const GP3_MINIMUM_IOPS: i64 = 3_000;
const IO1_MINIMUM_IOPS: i64 = 1_000;

/// The throughput floor, in MiB/s, a `gp3` volume is provisioned from.
const GP3_MINIMUM_THROUGHPUT: i64 = 125;

/// One row of the upgrade table: a source release and EXACTLY the versions it may be
/// upgraded to, written out here rather than computed, so the rule is stated
/// independently of the service's own derivation of it.
struct UpgradeRow {
    /// The release a domain runs.
    source: &'static str,
    /// The releases it may be upgraded to, oldest first.
    targets: &'static [&'static str],
}

/// The upgrade rows: the oldest release the table is published from, a release in the
/// middle of its line, a release at the END of its line (where the path leaves it, both
/// within one engine and across the engine crossing), and a release at the end of the
/// newest line.
const UPGRADE_ROWS: [UpgradeRow; 7] = [
    UpgradeRow {
        // The floor of the published table: an in-place upgrade is offered from here on.
        source: "Elasticsearch_5.1",
        targets: &[
            "Elasticsearch_5.3",
            "Elasticsearch_5.5",
            "Elasticsearch_5.6",
        ],
    },
    UpgradeRow {
        // The end of the Elasticsearch 5 line: the whole of the next line is reachable.
        source: "Elasticsearch_5.6",
        targets: &[
            "Elasticsearch_6.0",
            "Elasticsearch_6.2",
            "Elasticsearch_6.3",
            "Elasticsearch_6.4",
            "Elasticsearch_6.5",
            "Elasticsearch_6.7",
            "Elasticsearch_6.8",
        ],
    },
    UpgradeRow {
        // The end of the Elasticsearch 6 line: the next line is the Elasticsearch 7
        // releases together with the OpenSearch 1 releases that continue them, and
        // nothing after those.
        source: "Elasticsearch_6.8",
        targets: &[
            "Elasticsearch_7.1",
            "Elasticsearch_7.4",
            "Elasticsearch_7.7",
            "Elasticsearch_7.8",
            "Elasticsearch_7.9",
            "Elasticsearch_7.10",
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    UpgradeRow {
        // In the middle of that same line, which runs on into the OpenSearch 1 releases.
        source: "Elasticsearch_7.9",
        targets: &[
            "Elasticsearch_7.10",
            "OpenSearch_1.0",
            "OpenSearch_1.1",
            "OpenSearch_1.2",
            "OpenSearch_1.3",
        ],
    },
    UpgradeRow {
        // The end of it: only the next line is left.
        source: "OpenSearch_1.3",
        targets: &[
            "OpenSearch_2.3",
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
            "OpenSearch_2.19",
        ],
    },
    UpgradeRow {
        // In the middle of the OpenSearch 2 line: it reaches the end of that line and
        // stops there, with no release of the line after it.
        source: "OpenSearch_2.3",
        targets: &[
            "OpenSearch_2.5",
            "OpenSearch_2.7",
            "OpenSearch_2.9",
            "OpenSearch_2.11",
            "OpenSearch_2.13",
            "OpenSearch_2.15",
            "OpenSearch_2.17",
            "OpenSearch_2.19",
        ],
    },
    UpgradeRow {
        // The end of that line: the whole of the one after it.
        source: "OpenSearch_2.19",
        targets: &[
            "OpenSearch_3.1",
            "OpenSearch_3.3",
            "OpenSearch_3.5",
            "OpenSearch_3.7",
        ],
    },
];

/// The storage rows this suite holds the published limit blocks to: the ONE table, owned
/// by [`super::instance_type_limits`], spanning the burstable class at several sizes and
/// in two families, current-generation families at four sizes, and a previous-generation
/// family — the one that publishes the magnetic sub-type as well.
fn storage_row(instance_type: &str) -> &'static StorageRow {
    STORAGE_ROWS
        .iter()
        .find(|row| row.instance_type == instance_type)
        .unwrap_or_else(|| panic!("the storage table carries a row for {instance_type}"))
}

/// The limits one storage type publishes, in the order it publishes them.
fn published_limits(storage_type: &Value, case: &str) -> Vec<(String, i64)> {
    storage_type["StorageTypeLimits"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a StorageTypeLimits list, got {storage_type:?}")
        })
        .iter()
        .map(|limit| {
            let name = limit["LimitName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a LimitName, got {limit:?}"))
                .to_owned();
            let value = limit["LimitValues"][0]
                .as_str()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_else(|| {
                    panic!("case {case}: expected one numeric LimitValue, got {limit:?}")
                });
            (name, value)
        })
        .collect()
}

/// This suite's own statement of the limits one sub-type publishes on one row: the
/// volume-size pair every sub-type carries, the IOPS pair the provisioned ones carry, and
/// the throughput pair only `gp3` carries — each pair ceiling before floor.
fn expected_limits(row: &StorageRow, sub_type: &str) -> Vec<(String, i64)> {
    let maximum_volume_size = if sub_type == "standard" {
        row.maximum_volume_size.min(MAXIMUM_STANDARD_VOLUME_SIZE)
    } else {
        row.maximum_volume_size
    };
    let mut limits = vec![
        ("MaximumVolumeSize".to_owned(), maximum_volume_size),
        ("MinimumVolumeSize".to_owned(), MINIMUM_VOLUME_SIZE),
    ];
    match sub_type {
        "gp3" => {
            limits.push(("MaximumIops".to_owned(), row.maximum_iops));
            limits.push(("MinimumIops".to_owned(), GP3_MINIMUM_IOPS));
            limits.push(("MaximumThroughput".to_owned(), row.maximum_throughput));
            limits.push(("MinimumThroughput".to_owned(), GP3_MINIMUM_THROUGHPUT));
        }
        "io1" => {
            limits.push(("MaximumIops".to_owned(), row.maximum_iops));
            limits.push(("MinimumIops".to_owned(), IO1_MINIMUM_IOPS));
        }
        _ => {}
    }
    limits
}

/// The data role's storage types of one answer, as the sub-type name and that entry.
async fn data_storage_types(
    emulator: &Emulator,
    engine_version: &str,
    instance_type: &str,
) -> Vec<(String, Value)> {
    let case = format!("{engine_version} {instance_type}");
    let map = limits_by_role(
        &describe_limits(emulator, engine_version, instance_type, "").await,
        &case,
    );
    map["data"]["StorageTypes"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected the data role to publish its storage types, got {:?}",
                map["data"]
            )
        })
        .iter()
        .map(|entry| {
            (
                entry["StorageSubTypeName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                entry.clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_upgrade_table_leaves_a_release_line_only_from_its_end() {
    let emulator = Emulator::new();

    // The whole published table, read once: every row below is looked up in it, so the
    // rule is asserted about the table a client walks.
    let table = entries(&compatible(&emulator, None).await, "published table");
    let rows: BTreeMap<String, Vec<String>> = table
        .iter()
        .map(|entry| {
            let source = string_member(entry, "SourceVersion", "published table");
            let targets = target_versions(entry, &source);
            (source, targets)
        })
        .collect();
    assert_eq!(
        rows.len(),
        table.len(),
        "case the published table: expected one row per source version, got {} rows for \
         {} entries",
        rows.len(),
        table.len()
    );

    for (index, row) in UPGRADE_ROWS.iter().enumerate() {
        let expected: Vec<String> = row.targets.iter().map(|v| (*v).to_string()).collect();
        let published = rows.get(row.source).unwrap_or_else(|| {
            panic!(
                "case {}: expected the published table to carry a row for it, got {:?}",
                row.source,
                rows.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            *published, expected,
            "case {}: expected exactly the releases ahead of it on its own line, plus the \
             next line only from the end of its own, got {published:?}",
            row.source
        );

        // The same row read the other way: a domain running that release reports its one
        // row, with the same targets.
        let name = format!("catalog-upgrade-{index}");
        let created = emulator
            .create(json!({ "DomainName": name, "EngineVersion": row.source }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected a domain on that release, got {} {:?}",
            row.source, created.status, created.body
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        let filtered = entries(&compatible(&emulator, Some(&name)).await, row.source);
        assert_eq!(
            filtered.len(),
            1,
            "case {}: naming a domain limits the answer to its one version, got \
             {filtered:?}",
            row.source
        );
        assert_eq!(
            string_member(&filtered[0], "SourceVersion", row.source),
            row.source,
            "case {}: expected the version the domain runs, got {:?}",
            row.source,
            filtered[0]
        );
        assert_eq!(
            target_versions(&filtered[0], row.source),
            expected,
            "case {}: expected the domain-scoped row to publish the same targets as the \
             table, got {:?}",
            row.source,
            filtered[0]
        );
    }

    // The table is published from its floor: the oldest release an in-place upgrade is
    // offered for leads it, and no row sits below that.
    let floor = UPGRADE_ROWS[0].source;
    assert_eq!(
        table
            .first()
            .map(|entry| string_member(entry, "SourceVersion", "first row")),
        Some(floor.to_owned()),
        "case the table's first row: expected the published floor, got {:?}",
        table.first()
    );
}

#[tokio::test]
async fn storage_limits_are_the_requested_instance_types_own() {
    let emulator = Emulator::new();

    for row in &STORAGE_ROWS {
        let case = format!("{STORAGE_VERSION} {}", row.instance_type);
        let storage = data_storage_types(&emulator, STORAGE_VERSION, row.instance_type).await;

        // Only the sub-types this instance type attaches are published, in the published
        // order: a burstable instance, whose attachment provisions nothing, publishes the
        // general-purpose pair alone, and the magnetic sub-type is a previous-generation
        // family's only.
        let sub_types: Vec<&str> = storage
            .iter()
            .map(|(sub_type, _)| sub_type.as_str())
            .collect();
        assert_eq!(
            sub_types, row.sub_types,
            "case {case}: expected the sub-types this instance type attaches, got \
             {sub_types:?}"
        );

        for (sub_type, entry) in &storage {
            let case = format!("{case} {sub_type}");
            assert_eq!(
                entry["StorageTypeName"],
                json!("ebs"),
                "case {case}: expected the ebs storage type, got {entry:?}"
            );
            assert_eq!(
                published_limits(entry, &case),
                expected_limits(row, sub_type),
                "case {case}: expected the limits published for this instance type, each \
                 pair ceiling before floor, got {entry:?}"
            );
        }

        // The limits are a function of the INSTANCE TYPE, so the same type publishes the
        // same ones on another release that offers it.
        let other_release = "OpenSearch_2.11";
        let elsewhere = data_storage_types(&emulator, other_release, row.instance_type).await;
        assert_eq!(
            elsewhere, storage,
            "case {case}: expected the same limits on {other_release}, got {elsewhere:?}"
        );
    }

    // A burstable instance's published ceiling really is lower than the sub-type's own,
    // and lower than what a class with a provisioned attachment publishes: the bound is
    // the instance's, not the volume's.
    let burstable_row = storage_row("t3.small.search");
    let burstable = burstable_row.maximum_volume_size;
    let provisioned = storage_row("m5.large.search").maximum_volume_size;
    assert!(
        burstable < provisioned,
        "case the burstable class: expected a lower published volume ceiling than a class \
         with a provisioned attachment, got {burstable} and {provisioned}"
    );

    // The published ceiling is the one the service ENFORCES: a volume sized at it is
    // accepted for that instance type and one past it is refused, so a client sizing from
    // this catalog builds a request the service takes.
    let instance_type = burstable_row.instance_type;
    for (index, (size, accepted)) in [(burstable, true), (burstable + 1, false)]
        .into_iter()
        .enumerate()
    {
        let name = format!("catalog-ebs-{index}");
        let case = format!("{instance_type} {size} GiB");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": STORAGE_VERSION,
                "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 1 },
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": size },
            }))
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected a size the catalog publishes to be accepted, got {} \
                 {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 409,
                "case {case}: expected a size past the published ceiling to be refused, \
                 got {} {:?}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_offering_catalog_is_published_in_catalog_order() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "catalog order").await;

    /// Where one offering sits in the published order: its instance type in the published
    /// instance-type order, then its term shortest first, then its payment option in the
    /// order the modelled enum declares them.
    fn key(offering: &Value) -> (usize, i64, usize) {
        let position = |values: &[&str], value: Option<&str>| {
            values
                .iter()
                .position(|candidate| Some(*candidate) == value)
                .unwrap_or(values.len())
        };
        (
            position(
                &published_instance_types(),
                offering["InstanceType"].as_str(),
            ),
            offering["Duration"].as_i64().unwrap_or_default(),
            position(
                RESERVED_INSTANCE_PAYMENT_OPTION_VALUES,
                offering["PaymentOption"].as_str(),
            ),
        )
    }

    // The order the whole catalog is published in, as the suite's own statement of it.
    let mut terms: Vec<i64> = RESERVED_INSTANCE_TERMS
        .iter()
        .map(|term| i64::from(*term))
        .collect();
    terms.sort_unstable();
    let mut expected: Vec<(usize, i64, usize)> = Vec::new();
    for instance_type in 0..published_instance_types().len() {
        for duration in &terms {
            for payment_option in 0..RESERVED_INSTANCE_PAYMENT_OPTION_VALUES.len() {
                expected.push((instance_type, *duration, payment_option));
            }
        }
    }

    let published: Vec<(usize, i64, usize)> = catalog.iter().map(key).collect();
    assert_eq!(
        published, expected,
        "case the published catalog: expected it published by instance type, then term, \
         then payment option, got {} offerings",
        published.len()
    );

    // A PAGE of it is a page of that order: over a table of page sizes, the first page is
    // the first offerings of the catalog and every page follows the one before it.
    for page_size in [1_usize, 5, 100] {
        let case = format!("page size {page_size}");
        let answer = describe(&emulator, &format!("maxResults={page_size}")).await;
        let page = answer.body["ReservedInstanceOfferings"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a page of offerings"));
        assert_eq!(
            page.len(),
            page_size.min(catalog.len()),
            "case {case}: expected a full page, got {} offerings",
            page.len()
        );
        assert_eq!(
            page,
            catalog[..page.len()].to_vec(),
            "case {case}: expected the first offerings of the published catalog, got \
             {page:?}"
        );
    }
}
