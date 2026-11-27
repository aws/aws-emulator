// SPDX-License-Identifier: CC0-1.0

//! The valid-value list a PROVISIONED create's unsupported-version refusal
//! ADVERTISES, driven end to end over the same `MultiServiceRouter` the emulator
//! serves from (the shared [`crate::kafka_harness`]).
//!
//! RULE: when a provisioned `CreateCluster`/`CreateClusterV2` names a Kafka version
//! the service will not build a new cluster on, the refusal's bracketed list is the
//! versions Amazon MSK OFFERS A NEW CLUSTER of the broker category the request's
//! `BrokerNodeGroupInfo$InstanceType` names — MSK's own recorded offering for that
//! category — and not a projection of the version catalogue's `ACTIVE` rows, which
//! is a wider set serving the clusters that already run those versions. The list is
//! therefore the SAME whatever value was refused and whatever size of that
//! category's family the request asked for, every version it names is one the
//! create is admitted on, and a version the catalogue publishes as `ACTIVE` but
//! this offering omits is still admitted while never being advertised.
//!
//! The suites assert that over TABLES whose rows differ along what the rule
//! generalises over: the broker category and the instance FAMILY and SIZE inside
//! it, the FORM of the refused version, which create submitted it, and — for the
//! second suite — every version `ListKafkaVersions` publishes as offered, split by
//! whether the offering advertises it. The advertised sets are written out here
//! from the sentence the service answers such a create with, so they are pinned by
//! this suite rather than by the code under test, while the catalogue's own
//! `ACTIVE` rows are read off the wire, so the two cannot agree by construction.

use crate::kafka_harness::{
    unsupported_broker_version_message, v2_provisioned_body, ClusterRequest, Emulator, Reply,
};

/// The versions Amazon MSK advertises for a NEW cluster of the STANDARD broker
/// category (the `kafka.*` instance families), in the order it names them in.
///
/// Source: the refusal MSK answers a provisioned create naming a version it does
/// not offer with — "Standard instance types are not supported for Kafka version
/// <v>. Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft,
/// 4.1.x.kraft]". The model enumerates no concrete versions, so this offering is
/// the service's own published fact.
const STANDARD_ADVERTISED: &[&str] = &[
    "3.8.x",
    "3.8.x.kraft",
    "3.9.x",
    "3.9.x.kraft",
    "4.0.x.kraft",
    "4.1.x.kraft",
];

/// The same for the EXPRESS broker category (the `express.*` family), which is
/// offered the newest KRaft line as well — the one version the two categories'
/// offerings differ by, and the only one of the published lines MSK builds an
/// Express cluster on and no Standard one.
const EXPRESS_ADVERTISED: &[&str] = &[
    "3.8.x",
    "3.8.x.kraft",
    "3.9.x",
    "3.9.x.kraft",
    "4.0.x.kraft",
    "4.1.x.kraft",
    "4.2.x.kraft",
];

/// The broker categories, as the refusal words each, with the instance types of
/// that category this suite submits — more than one family and more than one size,
/// so the category is read from the type rather than from one spelling of it.
const CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Standard",
        &["kafka.m5.large", "kafka.m7g.2xlarge", "kafka.t3.small"],
    ),
    ("Express", &["express.m7g.large", "express.m7g.16xlarge"]),
];

/// The versions this suite reads a category's advertised list out of, spanning the
/// FORMS a value outside the offering comes in: a release newer than every
/// published one, an older-looking release, a two-component version, a variant
/// spelling of a published release, a `.x` minor the catalogue does not publish,
/// a case-shifted spelling of one it does, and a value that is no version at all.
const REFUSED_VERSIONS: &[&str] = &[
    "9.9.9",
    "0.0.1",
    "3.7",
    "3.6.0.kraft",
    "3.5.x",
    "3.9.X",
    "not-a-version",
];

/// The versions a category's refusal advertises as valid.
fn advertised(category: &str) -> &'static [&'static str] {
    match category {
        "Express" => EXPRESS_ADVERTISED,
        _ => STANDARD_ADVERTISED,
    }
}

/// The versions the EXPRESS category is advertised and the STANDARD one is not:
/// the lines MSK offers the Express brokers alone, which is the one difference
/// between what the two categories build a new cluster on.
fn express_only() -> Vec<&'static str> {
    EXPRESS_ADVERTISED
        .iter()
        .filter(|version| !STANDARD_ADVERTISED.contains(*version))
        .copied()
        .collect()
}

/// A cluster name built from the parts of a case: a name carries letters, digits
/// and hyphens, so every other character becomes one.
fn cluster_name(parts: &[&str]) -> String {
    parts
        .join("-")
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character,
            false => '-',
        })
        .collect()
}

/// Assert `reply` is the version refusal of `case` carrying `expected`.
fn assert_version_refusal(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case:?}: expected a 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case:?}: expected the sentence {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("kafkaVersion"),
        "case {case:?}: the refusal must name kafkaVersion as the parameter at fault, got {}",
        reply.body
    );
}

/// Every version `ListKafkaVersions` publishes an `ACTIVE` row for, each ONCE in
/// published order, read off the wire: the catalogue's own offered set, which is
/// what the advertised list must NOT be a projection of.
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
        // A version the catalogue publishes on more than one row is offered when
        // any of its rows says so.
        if entry["status"].as_str() == Some("ACTIVE") && !offered.contains(&version) {
            offered.push(version);
        }
    }
    assert!(
        !offered.is_empty(),
        "the catalogue must publish at least one offered version, got {}",
        reply.body
    );
    offered
}

/// RULE: the refusal advertises its category's new-cluster offering, named in that
/// one order — the same list whatever value was refused, whatever family and size
/// of that category the request asked for, and whichever of the two creates
/// submitted it.
#[tokio::test]
async fn the_refusal_advertises_its_categorys_new_cluster_offering() {
    let emulator = Emulator::new();

    for (category, instance_types) in CATEGORIES {
        for instance_type in *instance_types {
            for version in REFUSED_VERSIONS {
                let expected =
                    unsupported_broker_version_message(category, version, advertised(category));

                let case = format!("CreateClusterV2 {version:?} on {instance_type}");
                let name = cluster_name(&["v2", instance_type, version]);
                let reply = emulator
                    .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                    .await;
                assert_version_refusal(&reply, &case, &expected);
            }
        }
    }

    // The v1 create submits the same broker configuration, so it answers the same
    // sentence; its rows name the version as a literal, which is what that body
    // carries.
    let v1_rows: &[(&str, &str, &str)] = &[
        ("Standard", "kafka.m5.large", "9.9.9"),
        ("Standard", "kafka.t3.small", "3.5.x"),
        ("Express", "express.m7g.large", "9.9.9"),
        ("Express", "express.m7g.16xlarge", "not-a-version"),
    ];
    for (category, instance_type, version) in v1_rows {
        let case = format!("CreateCluster {version:?} on {instance_type}");
        let reply = emulator
            .create(&ClusterRequest {
                kafka_version: version,
                instance_type,
                ..ClusterRequest::named(&cluster_name(&["v1", instance_type, version]))
            })
            .await;
        assert_version_refusal(
            &reply,
            &case,
            &unsupported_broker_version_message(category, version, advertised(category)),
        );
    }

    // Not one of those refusals registered a cluster.
    let listed = emulator.list_v2("").await;
    assert_eq!(
        listed
            .at("/clusterInfoList")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a refused create must register no cluster, got {}",
        listed.body
    );
}

/// RULE: the advertised offering is NOT the catalogue's `ACTIVE` rows. Every
/// version it names is one a create of that category is admitted on and builds the
/// cluster it reports back, while a version the catalogue publishes as offered and
/// the offering omits is still admitted — and never advertised — unless it is a
/// line the other broker category is offered alone, which that category's create
/// refuses with this same sentence.
#[tokio::test]
async fn the_advertised_offering_is_narrower_than_the_catalogues_offered_rows() {
    let emulator = Emulator::new();
    let offered = offered_versions(&emulator).await;
    let express_only = express_only();

    for (category, instance_types) in CATEGORIES {
        let instance_type = instance_types[0];
        let advertised = advertised(category);

        // Everything advertised is a version the catalogue really publishes as
        // offered: the list never steers a client at a value no reader of this
        // service knows.
        for version in advertised {
            assert!(
                offered.iter().any(|name| name == version),
                "case {category}/{version}: an advertised version must be published as offered, \
                 got the offered set {offered:?}"
            );
        }

        // And the two sets really differ, so the rule below is not vacuous: the
        // catalogue goes on publishing versions as offered that this category is
        // admitted on and never advertised.
        let offered_unadvertised: Vec<&String> = offered
            .iter()
            .filter(|version| !advertised.contains(&version.as_str()))
            .filter(|version| !express_only.contains(&version.as_str()))
            .collect();
        assert!(
            !offered_unadvertised.is_empty(),
            "case {category}: the catalogue must publish an offered version the advertised list \
             omits, got {offered:?} against {advertised:?}"
        );

        for version in &offered {
            // What the catalogue publishes decides ADMISSION, narrowed for the
            // standard brokers by the lines the Express ones are offered alone;
            // what the refusal advertises is MSK's narrower steer.
            let admitted = *category == "Express" || !express_only.contains(&version.as_str());
            let case = format!("{category}/{version}");
            let name = cluster_name(&["offered", instance_type, version]);
            let reply = emulator
                .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                .await;
            match admitted {
                true => {
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}: an offered version must build a cluster, got {} {}",
                        reply.status, reply.body
                    );
                    // Round trip: the cluster runs the version it was asked for, as
                    // this service's own reader reports it.
                    let arn = reply
                        .at("/clusterArn")
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: the create minted no clusterArn: {}", reply.body)
                        })
                        .to_string();
                    let described = emulator.describe_v2(&arn).await;
                    assert_eq!(
                        described
                            .at("/clusterInfo/provisioned/currentBrokerSoftwareInfo/kafkaVersion")
                            .as_str(),
                        Some(version.as_str()),
                        "case {case:?}: the cluster must report the version it was built on, got {}",
                        described.body
                    );
                }
                false => assert_version_refusal(
                    &reply,
                    &case,
                    &unsupported_broker_version_message(category, version, advertised),
                ),
            }
        }
    }
}
