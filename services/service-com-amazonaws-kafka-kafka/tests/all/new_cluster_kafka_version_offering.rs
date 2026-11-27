// SPDX-License-Identifier: CC0-1.0

//! Which Apache Kafka versions a PROVISIONED CREATE may name, and what the
//! refusal of any other version says.
//!
//! RULE: Amazon MSK builds a new provisioned cluster only on a version its
//! catalog still publishes as offered to the broker category the request's
//! `InstanceType` names, and the refusal of any other `KafkaVersion` is HTTP 400
//! `BadRequestException` naming `kafkaVersion` and ADVERTISING that category's
//! new-cluster offering — MSK's own narrower set, which it steers a retry onto and
//! every member of which a create is admitted on. The advertised set is therefore a
//! subset of the admitted one, not the same set: the catalog goes on publishing a
//! version as offered, and the create goes on taking it, after MSK has stopped
//! naming it to a cluster that does not exist yet.
//!
//! The suite asserts that over a TABLE: both creates (the v1 one and the v2
//! provisioned body), both broker categories, and every version
//! `ListKafkaVersions` publishes plus values that are no published version at all.
//! Nothing here is read from the service's own constants: the advertised set comes
//! off the refusal's own sentence and the catalog off the listing, so the two
//! halves cannot agree by construction.
//!
//! Driven end to end over the same `MultiServiceRouter` the emulator serves from
//! (the shared harness in `tests/all/kafka_harness.rs`).

use serde_json::{json, Value};

use crate::kafka_harness::{
    unsupported_broker_version_message, v2_provisioned_body, Emulator, Reply,
};

/// The broker categories a provisioned cluster's brokers come in, as the refusal
/// words each, with an instance type of that category. The standard families are
/// the `kafka.*` ones and the Express one is `express.*`
/// (`BrokerNodeGroupInfo$InstanceType`).
const CATEGORIES: &[(&str, &str)] = &[
    ("Standard", "kafka.m5.large"),
    ("Express", "express.m7g.large"),
];

/// Values that are no version this service publishes, spanning the FORMS such a
/// value comes in: a release newer than every published one, an older-looking
/// release, a two-component version, a variant spelling of a published release, a
/// `.x` minor the catalog does not carry, an upper-cased spelling of one it does,
/// and a value that is no version at all.
const UNPUBLISHED: &[&str] = &[
    "9.9.9",
    "0.0.1",
    "3.7",
    "3.6.0.kraft",
    "3.5.x",
    "3.9.X",
    "not-a-version",
];

/// The version the valid-value list is read out of: no catalog publishes it, so
/// every category's refusal of it reports that category's whole offering.
const NO_SUCH_VERSION: &str = "0.0.0-no-such-version";

/// A `CreateCluster` body asking for `version` on brokers of `instance_type`,
/// written out here rather than taken from the harness's row so the version can be
/// one read off the wire.
fn v1_body(name: &str, version: &str, instance_type: &str) -> Value {
    json!({
        "clusterName": name,
        "kafkaVersion": version,
        "numberOfBrokerNodes": 2,
        "brokerNodeGroupInfo": {
            "instanceType": instance_type,
            "clientSubnets": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
        },
    })
}

// The v2 provisioned body carrying that same configuration, and the sentence the
// refusal reads, come from the shared harness
// ([`crate::kafka_harness::v2_provisioned_body`],
// [`crate::kafka_harness::unsupported_broker_version_message`]), so this suite and
// the create-gate suite beside it cannot drift apart in either.

/// A cluster name built from the parts of a case: a name carries letters, digits
/// and hyphens, so every other character becomes one.
fn cluster_name(parts: &[&str]) -> String {
    let joined = parts.join("-");
    joined
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character,
            false => '-',
        })
        .collect()
}

/// Every version `ListKafkaVersions` publishes, each ONCE in published order, read
/// off the wire — so this suite's view of the catalog is the one a client gets.
async fn published_versions(emulator: &Emulator) -> Vec<String> {
    let reply = emulator.call("GET", "/v1/kafka-versions", None).await;
    assert_eq!(
        reply.status, 200,
        "ListKafkaVersions must succeed, got {} {}",
        reply.status, reply.body
    );
    let mut versions: Vec<String> = Vec::new();
    for entry in reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
    {
        let version = entry["version"]
            .as_str()
            .unwrap_or_else(|| panic!("a published row without a version: {entry}"))
            .to_string();
        if !versions.contains(&version) {
            versions.push(version);
        }
    }
    assert!(
        !versions.is_empty(),
        "the catalog must publish at least one version, got {}",
        reply.body
    );
    versions
}

/// Every version `ListKafkaVersions` publishes as OFFERED, each once in published
/// order, read off the wire: the versions the catalog's own `ACTIVE` status says
/// MSK still offers.
async fn offered_versions(emulator: &Emulator) -> Vec<String> {
    let reply = emulator.call("GET", "/v1/kafka-versions", None).await;
    assert_eq!(
        reply.status, 200,
        "ListKafkaVersions must succeed, got {} {}",
        reply.status, reply.body
    );
    let mut offered: Vec<String> = Vec::new();
    for entry in reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
    {
        let version = entry["version"]
            .as_str()
            .unwrap_or_else(|| panic!("a published row without a version: {entry}"))
            .to_string();
        // A version the catalog publishes on more than one row is offered whenever
        // any of its rows says so, which is what the listing's reader does too.
        if entry["status"].as_str() == Some("ACTIVE") && !offered.contains(&version) {
            offered.push(version);
        }
    }
    assert!(
        !offered.is_empty(),
        "the catalog must publish at least one offered version, got {}",
        reply.body
    );
    offered
}

/// The versions the service itself advertises as valid for a new cluster of
/// `instance_type`'s category: the list its refusal of a value no catalog
/// publishes carries, parsed out of that sentence.
async fn advertised_versions(emulator: &Emulator, case: &str, instance_type: &str) -> Vec<String> {
    let name = cluster_name(&["advertised", instance_type]);
    let reply = emulator
        .call(
            "POST",
            "/v1/clusters",
            Some(v1_body(&name, NO_SUCH_VERSION, instance_type)),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: a create naming no published version must be refused, got {} {}",
        reply.status,
        reply.body
    );
    let message = reply.error_message().to_string();
    let listed = message
        .rsplit_once("Valid values: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| {
            panic!("case {case}: the refusal must carry a valid-value list, got {message:?}")
        });
    listed.split(", ").map(str::to_string).collect()
}

/// Every cluster name `ListClustersV2` reports.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    emulator
        .list_v2("")
        .await
        .at("/clusterInfoList")
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| cluster["clusterName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `reply` is the version refusal carrying `expected`.
fn assert_version_refusal(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected a 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected the sentence {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("kafkaVersion"),
        "case {case}: the refusal must name kafkaVersion as the parameter at fault, got {}",
        reply.body
    );
}

/// RULE: a create is admitted on every version the catalog publishes as offered to
/// that broker category, every version its refusal ADVERTISES is one of those, and
/// every other version is refused in that one sentence, whether or not the catalog
/// publishes it.
///
/// The table spans every version `ListKafkaVersions` publishes (so the published
/// versions the advertised list does not name are covered too, not only the ones
/// the report this rule came from cited), the unpublished forms above, both broker
/// categories and both provisioned creates.
#[tokio::test]
async fn a_create_is_admitted_on_the_offered_versions_and_advertises_msks_own_list() {
    let emulator = Emulator::new();
    let published = published_versions(&emulator).await;
    let offered = offered_versions(&emulator).await;
    // Each category's advertised list, read off its own refusal before the walk, so
    // what one category offers and the other does not is read off the wire too.
    let standard_advertised = advertised_versions(&emulator, "Standard", "kafka.m5.large").await;
    let express_advertised = advertised_versions(&emulator, "Express", "express.m7g.large").await;
    let mut candidates: Vec<String> = published.clone();
    candidates.extend(UNPUBLISHED.iter().map(|version| (*version).to_string()));

    for (category, instance_type) in CATEGORIES {
        let advertised = advertised_versions(&emulator, category, instance_type).await;
        // The lines MSK offers the EXPRESS brokers alone, read off the wire: the
        // versions that category advertises and the STANDARD one does not. They are
        // the one difference between what the two categories admit.
        let express_only: Vec<String> = express_advertised
            .iter()
            .filter(|version| !standard_advertised.contains(*version))
            .cloned()
            .collect();
        assert!(
            !advertised.is_empty(),
            "case {category}: the refusal must name the versions a create may use"
        );
        // The advertised list offers only versions a reader of this service knows,
        // and names each once: a list carrying a version nothing publishes would
        // steer a client at a value no reader could explain.
        for (index, version) in advertised.iter().enumerate() {
            assert!(
                published.contains(version),
                "case {category}/{version}: an advertised version must be one \
                 ListKafkaVersions publishes, got {published:?}"
            );
            assert!(
                !advertised[..index].contains(version),
                "case {category}/{version}: the list must name each version once, got \
                 {advertised:?}"
            );
        }
        // It is really NARROWER than the published catalog, so the rule below is
        // not vacuous: MSK retires a version for new clusters while still
        // publishing it.
        assert!(
            advertised.len() < published.len(),
            "case {category}: the advertised set must be narrower than the published catalog, \
             got {advertised:?} against {published:?}"
        );

        let mut admitted_names: Vec<String> = Vec::new();
        for version in &candidates {
            // What the catalog publishes decides admission — narrowed, for the
            // standard brokers, by the lines MSK offers the Express ones alone —
            // while what the refusal advertises is MSK's narrower steer, so every
            // advertised version is admitted and no admitted version is refused.
            let admitted = offered.contains(version)
                && (*category == "Express" || !express_only.contains(version));
            assert!(
                !advertised.contains(version) || admitted,
                "case {category}/{version}: an advertised version must be one the create \
                 admits, got the offered set {offered:?}"
            );
            let sentence = unsupported_broker_version_message(category, version, &advertised);

            let name = cluster_name(&["v1", category, version]);
            let case = format!("CreateCluster {category}/{version}");
            let reply = emulator
                .call(
                    "POST",
                    "/v1/clusters",
                    Some(v1_body(&name, version, instance_type)),
                )
                .await;
            match admitted {
                true => {
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: an offered version must be admitted, got {} {}",
                        reply.status, reply.body
                    );
                    admitted_names.push(name);
                }
                false => assert_version_refusal(&reply, &case, &sentence),
            }

            // The v2 provisioned body submits the same configuration, so the same
            // rule holds for it.
            let name = cluster_name(&["v2", category, version]);
            let case = format!("CreateClusterV2 {category}/{version}");
            let reply = emulator
                .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                .await;
            match admitted {
                true => {
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: an offered version must be admitted, got {} {}",
                        reply.status, reply.body
                    );
                    // Round trip: the cluster the create minted runs the version it
                    // was asked for, as the service's own reader reports it.
                    let arn = reply
                        .at("/clusterArn")
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: the create minted no clusterArn"))
                        .to_string();
                    let described = emulator.describe_v2(&arn).await;
                    assert_eq!(
                        described
                            .at("/clusterInfo/provisioned/currentBrokerSoftwareInfo/kafkaVersion")
                            .as_str(),
                        Some(version.as_str()),
                        "case {case}: the cluster must report the version it was created on, \
                         got {}",
                        described.body
                    );
                    admitted_names.push(name);
                }
                false => assert_version_refusal(&reply, &case, &sentence),
            }
        }

        // Only the admitted creates left a cluster behind: a refused create
        // registers nothing.
        let listed = listed_names(&emulator).await;
        for name in &admitted_names {
            assert!(
                listed.contains(name),
                "case {category}: an admitted create must register {name:?}, got {listed:?}"
            );
        }
        let marker = format!("-{category}-");
        let strays: Vec<&String> = listed
            .iter()
            .filter(|name| name.contains(&marker) && !admitted_names.contains(name))
            .collect();
        assert!(
            strays.is_empty(),
            "case {category}: a refused create must register nothing, got {strays:?}"
        );
    }
}

/// RULE: the category the refusal names is read from `BrokerNodeGroupInfo`'s
/// `InstanceType` — the `kafka.*` families are the standard brokers and the
/// `express.*` family the Express ones — and the version it quotes is the
/// request's own, verbatim.
///
/// The rows differ along the family and the SIZE inside each category, including
/// sizes the table above does not use, and along the form of the rejected version.
#[tokio::test]
async fn the_refusal_names_the_category_of_the_instance_type_and_the_version_asked_for() {
    let emulator = Emulator::new();

    // Instance types the service offers (`kafka.m5.*`, `kafka.m7g.*`,
    // `kafka.t3.small`, `express.m7g.*`), so the category is read from the type
    // rather than from the one size the table above used.
    let rows: &[(&str, &str)] = &[
        ("Standard", "kafka.m5.4xlarge"),
        ("Standard", "kafka.m7g.2xlarge"),
        ("Standard", "kafka.t3.small"),
        ("Express", "express.m7g.4xlarge"),
        ("Express", "express.m7g.16xlarge"),
    ];

    for (category, instance_type) in rows {
        let advertised = advertised_versions(&emulator, instance_type, instance_type).await;
        for version in UNPUBLISHED {
            let case = format!("{instance_type}/{version}");
            let name = cluster_name(&["category", instance_type, version]);
            let reply = emulator
                .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                .await;
            assert_version_refusal(
                &reply,
                &case,
                &unsupported_broker_version_message(category, version, &advertised),
            );
        }
    }
}
