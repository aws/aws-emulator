// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the warm-storage capability of the instance type a domain's HOT
//! tier runs on: which `ClusterConfig` the service provisions UltraWarm on.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`];
//! this suite adds no second copy of it.
//!
//! The rule under test: warm storage is offered per INSTANCE TYPE — the capability
//! `InstanceTypeDetails$WarmEnabled` publishes — so a `ClusterConfig` that enables the
//! warm tier on a hot-tier instance type that does not offer it describes a topology the
//! service does not provision: the request is refused with the modelled
//! `ValidationException` naming that instance type, no domain is created or changed, and
//! the same cluster on an instance type that does offer it is served.
//!
//! The rows differ along what the rule generalises over: the instance family (every
//! burstable one, at several sizes, against general-purpose, compute-, memory- and
//! storage-optimized ones), whether the call is a create or an update, and whether the
//! rest of the warm chain is satisfied.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    error_code, settled_domain, Answer, Emulator, RECORDED_NOT_FOUND_STATUS,
};

/// The engine release every create below names and every catalog read is taken for, so
/// the capability the catalog publishes and the rule the create applies are read for ONE
/// release rather than two.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The refusal's fixed text around the instance type it names.
fn unavailable_message(instance_type: &str) -> String {
    format!(
        "Warm storage is not available for the {instance_type} instance type. Select a \
         different instance type."
    )
}

/// The hot-tier instance types warm storage is NOT offered for: the BURSTABLE families,
/// at the sizes the published catalog offers them in
/// (`smithy_query(query="shape",shape="OpenSearchPartitionInstanceType")` lists them, and
/// `ListInstanceTypeDetails` reports `WarmEnabled` false for each — which is the same
/// capability this rule reads).
const WITHOUT_WARM_STORAGE: [&str; 6] = [
    "t2.small.search",
    "t2.medium.search",
    "t3.small.search",
    "t3.medium.search",
    "t3.large.search",
    "t4g.small.search",
];

/// Hot-tier instance types warm storage IS offered for, one per family group the catalog
/// publishes: general purpose, compute optimized, memory optimized, storage optimized and
/// OpenSearch optimized.
const WITH_WARM_STORAGE: [&str; 5] = [
    "m6g.large.search",
    "c5.xlarge.search",
    "r5.large.search",
    "i3.large.search",
    "or1.medium.search",
];

/// A `ClusterConfig` with the warm tier enabled on `instance_type`, with the rest of the
/// warm chain satisfied — dedicated master nodes, which the warm tier rests on — so the
/// only thing a refusal can be about is the instance type.
fn warm_cluster(instance_type: &str) -> Value {
    json!({
        "InstanceType": instance_type,
        "InstanceCount": 2,
        "DedicatedMasterEnabled": true,
        "DedicatedMasterType": "c5.large.search",
        "DedicatedMasterCount": 3,
        "WarmEnabled": true,
        "WarmType": "ultrawarm1.medium.search",
        "WarmCount": 2,
    })
}

/// `ListInstanceTypeDetails` for one engine version, which is where the service PUBLISHES
/// the warm-storage capability of each instance type — the data the create rule reads.
async fn published_warm_capability(emulator: &Emulator, instance_type: &str) -> bool {
    let answer: Answer = emulator
        .call(
            "GET",
            &format!(
                "/2021-01-01/opensearch/instanceTypeDetails/{ENGINE_VERSION}?maxResults=100"
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case the published catalog: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let mut entries: Vec<Value> = answer.body["InstanceTypeDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut token = answer.body["NextToken"].as_str().map(str::to_owned);
    while let Some(cursor) = token {
        let page = emulator
            .call(
                "GET",
                &format!(
                    "/2021-01-01/opensearch/instanceTypeDetails/{ENGINE_VERSION}\
                     ?maxResults=100&nextToken={cursor}"
                ),
                None,
            )
            .await;
        entries.extend(
            page.body["InstanceTypeDetails"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = page.body["NextToken"].as_str().map(str::to_owned);
    }
    let entry = entries
        .iter()
        .find(|entry| entry["InstanceType"].as_str() == Some(instance_type))
        .unwrap_or_else(|| {
            panic!("case {instance_type:?}: the published catalog must carry this instance type")
        });
    entry["WarmEnabled"].as_bool().unwrap_or_else(|| {
        panic!("case {instance_type:?}: expected a WarmEnabled flag, got {entry:?}")
    })
}

#[tokio::test]
async fn a_warm_tier_on_an_instance_type_that_offers_none_is_refused() {
    let emulator = Emulator::new();

    for (index, instance_type) in WITHOUT_WARM_STORAGE.iter().enumerate() {
        let name = format!("warm-refused-{index}");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": ENGINE_VERSION,
                "ClusterConfig": warm_cluster(instance_type),
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {instance_type:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {instance_type:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let expected = unavailable_message(instance_type);
        let stated = answer.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            stated, expected,
            "case {instance_type:?}: expected {expected:?}, got {stated:?}"
        );

        // The refused create provisioned nothing.
        let read = emulator.describe(&name).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {instance_type:?}: a refused create must provision no domain, got {} with {:?}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_warm_tier_on_an_instance_type_that_offers_it_is_served() {
    let emulator = Emulator::new();

    for (index, instance_type) in WITH_WARM_STORAGE.iter().enumerate() {
        let name = format!("warm-served-{index}");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": ENGINE_VERSION,
                "ClusterConfig": warm_cluster(instance_type),
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {instance_type:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // The cluster was stored as asked for, read back through the operation that
        // reports a domain's configuration.
        let config = emulator.describe_config(&name).await;
        let cluster = &config.body["DomainConfig"]["ClusterConfig"]["Options"];
        assert_eq!(
            cluster["WarmEnabled"],
            json!(true),
            "case {instance_type:?}: expected the warm tier the create asked for, got {:?}",
            config.body
        );
        assert_eq!(
            cluster["InstanceType"],
            json!(instance_type),
            "case {instance_type:?}: expected the hot tier the create asked for, got {:?}",
            config.body
        );
    }
}

#[tokio::test]
async fn the_rule_reads_the_capability_the_published_catalog_reports() {
    let emulator = Emulator::new();

    // Every row's create agrees with the capability `ListInstanceTypeDetails` publishes
    // for that instance type: the two read the same answer, so a client can tell from the
    // catalog which clusters the create will take. One instance type the issue's own case
    // did not cite is included on each side.
    for (index, instance_type) in WITHOUT_WARM_STORAGE
        .iter()
        .chain(WITH_WARM_STORAGE.iter())
        .enumerate()
    {
        let published = published_warm_capability(&emulator, instance_type).await;
        let name = format!("warm-catalog-{index}");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": ENGINE_VERSION,
                "ClusterConfig": warm_cluster(instance_type),
            }))
            .await;
        assert_eq!(
            answer.status == 200,
            published,
            "case {instance_type:?}: the create must agree with the published WarmEnabled \
             ({published}), got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn an_update_that_would_leave_the_warm_tier_on_such_an_instance_type_is_refused() {
    let emulator = Emulator::new();
    let name = "warm-update-probe";

    // A domain on a burstable instance type with no warm tier: the create is served,
    // because the rule is about the warm tier rather than the instance type.
    let created = emulator
        .create(json!({
            "DomainName": name,
            "EngineVersion": ENGINE_VERSION,
            "ClusterConfig": {
                "InstanceType": "t3.medium.search",
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case the burstable domain with no warm tier: expected 200, got {} with {:?}",
        created.status, created.body
    );
    // The provisioning the clock owed the new domain falls due on its own, and an update
    // is served once it is no longer in flight: the case waits for THAT condition, through
    // the harness's own poll for a settled domain.
    settled_domain(&emulator, name).await;

    // Switching the warm tier ON over that hot tier is refused, and the domain keeps the
    // configuration it had. The change carries the whole `ClusterConfig` the domain would
    // be left with, which is what the rule is applied to.
    let answer = emulator
        .update_config(
            name,
            json!({
                "ClusterConfig": {
                    "InstanceType": "t3.medium.search",
                    "InstanceCount": 2,
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": "c5.large.search",
                    "DedicatedMasterCount": 3,
                    "WarmEnabled": true,
                    "WarmType": "ultrawarm1.medium.search",
                    "WarmCount": 2,
                },
            }),
        )
        .await;
    assert_eq!(
        (answer.status, error_code(&answer).as_str()),
        (400, "ValidationException"),
        "case the update that would enable the warm tier: expected the modelled refusal, got {} \
         with {:?}",
        answer.status,
        answer.body
    );
    let expected = unavailable_message("t3.medium.search");
    assert_eq!(
        answer.body["message"].as_str().unwrap_or_default(),
        expected,
        "case the update that would enable the warm tier: expected {expected:?}, got {:?}",
        answer.body
    );
    let config = emulator.describe_config(name).await;
    let cluster = &config.body["DomainConfig"]["ClusterConfig"]["Options"];
    assert_ne!(
        cluster["WarmEnabled"],
        json!(true),
        "case the refused update: the domain must keep the cluster it had, got {:?}",
        config.body
    );
    assert_eq!(
        cluster["InstanceType"],
        json!("t3.medium.search"),
        "case the refused update: the domain must keep the hot tier it had, got {:?}",
        config.body
    );

    // ... while the same update that ALSO moves the hot tier onto an instance type warm
    // storage is offered for is served, so what refused the row above is the pairing.
    let served = emulator
        .update_config(
            name,
            json!({
                "ClusterConfig": {
                    "InstanceType": "m6g.large.search",
                    "InstanceCount": 2,
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": "c5.large.search",
                    "DedicatedMasterCount": 3,
                    "WarmEnabled": true,
                    "WarmType": "ultrawarm1.medium.search",
                    "WarmCount": 2,
                },
            }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case the update onto an instance type that offers warm storage: expected 200, got {} \
         with {:?}",
        served.status, served.body
    );
}
