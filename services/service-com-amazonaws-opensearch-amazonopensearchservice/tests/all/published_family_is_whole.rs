// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a family of the published instance-type catalog being published
//! WHOLE: every surface that answers about an instance type reads the one catalog, so a
//! family it carries is reported by the listing, has limits published for the roles an
//! instance of it serves, is named by the accepted-value set a refusal enumerates, and
//! carries a domain a caller provisions on it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the listing readers from [`super::instance_type_catalog`] and the limit readers from
//! [`super::instance_type_limits`], the two suites that own those operations; this one
//! adds no second copy of any of them.
//!
//! What it adds to its siblings ([`super::current_generation_instance_types`] tables the
//! listing of the current generations, [`super::published_instance_type_value_set`] the
//! membership of the stated value set): that the catalog's answer about a family is the
//! SAME answer on every surface, and that a type the listing publishes is a type a domain
//! can actually run. A generation the service offers as two families — the EBS-backed and
//! the NVMe-backed half of a memory-optimized generation — is published as both halves, so
//! each half is a row.
//!
//! Source of the rows: the instance types the service's own accepted-value list reports
//! (the set its refusal of an unknown `instanceType` enumerates), sampled one size per
//! family, with each row's storage kind the one its family's data sits on.

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, settled_domain, Emulator};
use super::instance_type_catalog::{details, instance_types, list, list_type};
use super::instance_type_limits::{describe_limits, limits_by_role};

/// The roles a general-purpose, compute-optimized or memory-optimized family serves.
const DATA_MASTER_COORDINATOR: &[&str] = &["data", "master", "coordinator"];

/// The roles a storage-optimized family serves: a dedicated coordinator node holds none
/// of the domain's data, so it is not provisioned on one.
const DATA_AND_MASTER: &[&str] = &["data", "master"];

/// The role a family published for the DATA TIER alone serves: its instances hold the
/// domain's data and are never master-eligible.
const DATA_ONLY: &[&str] = &["data"];

/// `StorageTypeName`'s `instance`: "built-in storage available for the instance", which
/// has no sub-type and no volume limits, because there is no volume to size.
const STORAGE_INSTANCE: &str = "instance";

/// `StorageTypeName`'s `ebs`: Elastic Block Store storage attached to the instance, the
/// one storage type that carries a sub-type and volume limits.
const STORAGE_EBS: &str = "ebs";

/// One row: an instance type, the roles an instance of its family serves, the storage its
/// data sits on, a release that offers the family and the release before its
/// prerequisite.
struct Row {
    /// The instance type every surface below is asked about.
    instance_type: &'static str,
    /// The roles the catalog publishes for its family.
    roles: &'static [&'static str],
    /// `StorageTypeName` of the storage its data sits on.
    storage: &'static str,
    /// A release the family is offered on.
    offered_on: &'static str,
    /// The release before the family's published prerequisite.
    before: &'static str,
}

/// The table every case below is made over: both halves of the memory-optimized generation
/// the service publishes as a pair, two further classes of the same generation, the warm
/// tier and the OpenSearch-optimized family, so no row's rule rests on one family, one
/// storage kind, one role set or one engine line.
const ROWS: [Row; 6] = [
    // The memory-optimized Graviton2 generation: the EBS-backed half and the NVMe-backed
    // one, which serve the same roles and differ only in where the data sits.
    Row {
        instance_type: "r6g.xlarge.search",
        roles: DATA_MASTER_COORDINATOR,
        storage: STORAGE_EBS,
        offered_on: "Elasticsearch_7.9",
        before: "Elasticsearch_7.8",
    },
    Row {
        instance_type: "r6gd.2xlarge.search",
        roles: DATA_MASTER_COORDINATOR,
        storage: STORAGE_INSTANCE,
        offered_on: "Elasticsearch_7.9",
        before: "Elasticsearch_7.8",
    },
    // The general-purpose and compute-optimized classes of that same generation.
    Row {
        instance_type: "m6g.4xlarge.search",
        roles: DATA_MASTER_COORDINATOR,
        storage: STORAGE_EBS,
        offered_on: "Elasticsearch_7.9",
        before: "Elasticsearch_7.8",
    },
    Row {
        instance_type: "c6g.8xlarge.search",
        roles: DATA_MASTER_COORDINATOR,
        storage: STORAGE_EBS,
        offered_on: "Elasticsearch_7.9",
        before: "Elasticsearch_7.8",
    },
    // The burstable Graviton family, which serves no dedicated coordinator node.
    Row {
        instance_type: "t4g.medium.search",
        roles: DATA_AND_MASTER,
        storage: STORAGE_EBS,
        offered_on: "Elasticsearch_7.9",
        before: "Elasticsearch_7.8",
    },
    // A family gated on an OpenSearch release rather than an Elasticsearch one.
    Row {
        instance_type: "or1.2xlarge.search",
        roles: DATA_ONLY,
        storage: STORAGE_EBS,
        offered_on: "OpenSearch_2.11",
        before: "OpenSearch_2.9",
    },
];

/// A release every row is offered on, so one listing answers for the whole table.
const CURRENT_RELEASE: &str = "OpenSearch_2.19";

/// An instance type no family is published in, which is what makes a refusal state the
/// accepted-value set. Its form satisfies `InstanceTypeString`'s
/// `^.*\..*\.search$`, so what refuses it is the value set and not the pattern.
const OUTSIDE_THE_CATALOG: &str = "t9.nonexistent.search";

/// The sub-types every instance class with an attached volume publishes: the
/// general-purpose SSD ones. Source: `smithy_query(query="shape",shape="VolumeType")`,
/// whose values are `standard`, `gp2`, `io1`, `gp3`.
const GENERAL_PURPOSE_SUB_TYPES: [&str; 2] = ["gp2", "gp3"];

/// One entry's `InstanceRole`, as the strings it reports.
fn roles(entry: &Value, case: &str) -> Vec<String> {
    entry["InstanceRole"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an InstanceRole array, got {entry:?}"))
        .iter()
        .filter_map(|role| role.as_str().map(str::to_owned))
        .collect()
}

/// The roles a row expects, as owned strings a reported list compares against.
fn expected_roles(row: &Row) -> Vec<String> {
    row.roles.iter().map(|role| (*role).to_owned()).collect()
}

/// `InstanceRole`'s `coordinator`, which the LISTING publishes for a family whose
/// instances can be provisioned as a dedicated coordinator node.
const INSTANCE_ROLE_COORDINATOR: &str = "coordinator";

/// The roles the LIMIT catalog publishes a block for, out of the roles a family serves:
/// `LimitsByRole`'s own documentation names its roles `data`, `master` and `ultra_warm`,
/// so a family that can also take a dedicated coordinator node publishes no block under
/// that role — there is no instance count or storage to publish for it.
fn expected_limit_roles(row: &Row) -> Vec<String> {
    let mut roles: Vec<String> = expected_roles(row)
        .into_iter()
        .filter(|role| role != INSTANCE_ROLE_COORDINATOR)
        .collect();
    roles.sort();
    roles
}

/// RULE: the listing and the limit catalog answer the SAME thing about a family — the
/// roles an instance of it serves — and the limit catalog publishes one entry per role
/// the listing publishes that has limits to publish.
#[tokio::test]
async fn the_listing_and_the_limit_catalog_publish_one_answer_per_family() {
    let emulator = Emulator::new();
    let current = instance_types(&details(&list(&emulator, CURRENT_RELEASE, "").await, CURRENT_RELEASE));

    for row in &ROWS {
        let case = format!("{} on {}", row.instance_type, row.offered_on);

        // Published by the release that offers the family, and by a current release.
        assert!(
            current.contains(&row.instance_type.to_string()),
            "case {case}: expected a current release to publish the instance type, got {} \
             entries",
            current.len()
        );
        // The narrowed read is made the way the service requires it
        // ([`super::instance_type_catalog::list_type`]): naming an instance type asks for
        // that type's Availability Zone information, which the retrieve-AZ flag has to
        // enable.
        let filtered = details(
            &list_type(&emulator, row.offered_on, row.instance_type).await,
            &case,
        );
        assert_eq!(
            filtered.len(),
            1,
            "case {case}: the filter names one instance type, got {filtered:?}"
        );
        let published = roles(&filtered[0], &case);
        assert_eq!(
            published,
            expected_roles(row),
            "case {case}: expected the roles the family serves, got {published:?}"
        );

        // The limit catalog publishes limits for the roles that HAVE limits among the ones
        // the listing publishes, so the two surfaces cannot disagree about what an instance
        // of the family can be provisioned as.
        let limits = limits_by_role(
            &describe_limits(&emulator, row.offered_on, row.instance_type, "").await,
            &case,
        );
        let mut keys: Vec<String> = limits
            .as_object()
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        assert_eq!(
            keys,
            expected_limit_roles(row),
            "case {case}: expected limits for the roles the listing publishes, got {limits:?}"
        );
    }
}

/// RULE: the storage the limit catalog publishes for a family is the storage its data
/// sits on — built-in storage, with no sub-type and no volume limits, for a family whose
/// instances carry it, and one `ebs` entry per attachable volume sub-type otherwise.
#[tokio::test]
async fn the_limit_catalog_publishes_the_storage_the_family_carries() {
    let emulator = Emulator::new();

    for row in &ROWS {
        let case = format!("{} on {CURRENT_RELEASE}", row.instance_type);
        let limits = limits_by_role(
            &describe_limits(&emulator, CURRENT_RELEASE, row.instance_type, "").await,
            &case,
        );
        let storage_types = limits["data"]["StorageTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected the data role's StorageTypes, got {limits:?}"))
            .clone();
        assert!(
            !storage_types.is_empty(),
            "case {case}: expected the storage a data node of the family holds its data on"
        );
        for storage in &storage_types {
            assert_eq!(
                storage["StorageTypeName"].as_str(),
                Some(row.storage),
                "case {case}: expected the storage the family carries, got {storage:?}"
            );
        }

        if row.storage == STORAGE_INSTANCE {
            // Built-in storage is one entry with nothing to size: no sub-type and no
            // volume limits.
            assert_eq!(
                storage_types.len(),
                1,
                "case {case}: built-in storage is published once, got {storage_types:?}"
            );
            assert!(
                storage_types[0]["StorageSubTypeName"].is_null()
                    && storage_types[0]["StorageTypeLimits"].is_null(),
                "case {case}: expected built-in storage to publish no sub-type and no volume \
                 limits, got {:?}",
                storage_types[0]
            );
            continue;
        }

        // An attached volume publishes its sub-types, each with the volume-size floor
        // every sub-type shares.
        let sub_types: Vec<String> = storage_types
            .iter()
            .filter_map(|storage| storage["StorageSubTypeName"].as_str().map(str::to_owned))
            .collect();
        for general_purpose in GENERAL_PURPOSE_SUB_TYPES {
            assert!(
                sub_types.contains(&general_purpose.to_string()),
                "case {case}: expected the general-purpose sub-type {general_purpose} to be \
                 published, got {sub_types:?}"
            );
        }
        for storage in &storage_types {
            let limits = storage["StorageTypeLimits"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected an attached volume's limits, got {storage:?}")
                })
                .clone();
            assert!(
                limits
                    .iter()
                    .any(|limit| limit["LimitName"] == json!("MinimumVolumeSize")),
                "case {case}: expected the volume-size floor to be published, got {limits:?}"
            );
        }
    }
}

/// RULE: a type the catalog publishes is a type a domain RUNS — the create is served and
/// the provisioned domain reports the instance type it was asked for — and the release
/// before the family's prerequisite offers neither the listing entry nor the limits.
#[tokio::test]
async fn a_published_type_carries_a_domain_and_an_earlier_release_offers_none() {
    let emulator = Emulator::new();

    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("published-family-{index}");
        let case = format!("{} on {CURRENT_RELEASE}", row.instance_type);
        let created = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": CURRENT_RELEASE,
                "ClusterConfig": { "InstanceType": row.instance_type, "InstanceCount": 1 },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case CreateDomain({case}): expected the create to be served, got {} {:?}",
            created.status, created.body
        );

        // Read back through the operation that reports a domain, once the provisioning the
        // clock owes has fallen due.
        let status = settled_domain(&emulator, &name).await;
        assert_eq!(
            status["ClusterConfig"]["InstanceType"].as_str(),
            Some(row.instance_type),
            "case {case}: expected the domain to run the instance type the create named, got \
             {:?}",
            status["ClusterConfig"]
        );

        // A release before the family's prerequisite offers it nowhere: not in the
        // listing, and not as a pairing the limit catalog answers.
        let earlier = instance_types(&details(&list(&emulator, row.before, "").await, &case));
        assert!(
            !earlier.contains(&row.instance_type.to_string()),
            "case {} on {}: expected a release before the family's prerequisite not to offer \
             it, got {} entries",
            row.instance_type,
            row.before,
            earlier.len()
        );
        let refused = describe_limits(&emulator, row.before, row.instance_type, "").await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (409, "InvalidTypeException"),
            "case {} on {}: expected the modelled unsupported-type refusal, got {} {:?}",
            row.instance_type,
            row.before,
            refused.status,
            refused.body
        );
    }
}

/// RULE: the accepted-value set a refusal of an unknown instance type enumerates is the
/// catalog the service publishes, so every row of the table above is stated in it — the
/// message is the only place a client reads which values to choose from.
#[tokio::test]
async fn the_accepted_value_set_states_every_published_family() {
    let emulator = Emulator::new();

    let refused = describe_limits(&emulator, CURRENT_RELEASE, OUTSIDE_THE_CATALOG, "").await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case {OUTSIDE_THE_CATALOG}: expected the modelled constraint refusal, got {} {:?}",
        refused.status,
        refused.body
    );
    let stated = refused.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {OUTSIDE_THE_CATALOG}: expected a message, got {:?}", refused.body))
        .to_owned();
    let listed: Vec<&str> = stated
        .rsplit_once("enum value set: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("expected a bracketed value set, got {stated:?}"))
        .split(", ")
        .collect();

    for row in &ROWS {
        assert!(
            listed.contains(&row.instance_type),
            "case {}: expected the stated value set to name a type the catalog publishes, got \
             {} values",
            row.instance_type,
            listed.len()
        );
    }
}
