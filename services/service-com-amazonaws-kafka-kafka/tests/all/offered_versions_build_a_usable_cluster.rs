// SPDX-License-Identifier: CC0-1.0

//! What the published Apache Kafka version catalog PROMISES a new provisioned
//! cluster, driven end to end over the same `MultiServiceRouter` the emulator
//! serves from (the shared [`crate::kafka_harness`]).
//!
//! RULE: a version `ListKafkaVersions` publishes with `Status=ACTIVE` is one
//! Amazon MSK still builds a NEW provisioned cluster on, so a well-formed
//! `CreateCluster`/`CreateClusterV2` naming it is ACCEPTED for a broker category
//! that is offered it, and the cluster it mints settles to `ACTIVE` and publishes
//! the bootstrap addresses of the brokers it was asked for. The "<category>
//! instance types are not supported for Kafka version <version>. Valid values:
//! [<versions>]" refusal is therefore a statement about the broker CATEGORY — it
//! is raised for an offered version only where the OTHER category does build on
//! it — and never a statement that the service does not offer a version its own
//! catalog publishes as offered. A version the catalog publishes only as
//! deprecated is built by neither category, and the valid-value list that refusal
//! carries names only versions the catalog publishes as offered, so a client
//! reading it is steered inside the same catalog it enumerated.
//!
//! The suites assert that over TABLES whose rows differ along what the rule
//! generalises over: every version the catalog publishes and the status it is
//! published under, the broker category, the instance FAMILY and SIZE inside that
//! category, and which of the two provisioned creates submitted the request.
//! Every version string is read OFF THE WIRE from the catalog the service itself
//! publishes — none is written out here — so the suite pins the agreement between
//! the listing and the create gate rather than one run's version names.

use serde_json::{json, Value};

use crate::kafka_harness::{
    poll_until_active, unsupported_broker_version_message, v2_provisioned_body, Emulator, Reply,
};

/// The `KafkaVersionStatus` value the catalog publishes a version it still offers
/// under, as the model's enum spells it.
const OFFERED: &str = "ACTIVE";

/// The instance types of the STANDARD broker category this suite submits — more
/// than one family and more than one size, so the category is read from the type
/// rather than from one spelling of it. The standard families are the `kafka.*`
/// ones (`BrokerNodeGroupInfo$InstanceType`).
const STANDARD_INSTANCE_TYPES: &[&str] = &["kafka.t3.small", "kafka.m5.large", "kafka.m7g.2xlarge"];

/// The instance types of the EXPRESS broker category: the `express.*` family, in
/// two sizes.
const EXPRESS_INSTANCE_TYPES: &[&str] = &["express.m7g.large", "express.m7g.16xlarge"];

/// The broker categories, as the version refusal words each, with the instance
/// types of that category above.
const CATEGORIES: &[(&str, &[&str])] = &[
    ("Standard", STANDARD_INSTANCE_TYPES),
    ("Express", EXPRESS_INSTANCE_TYPES),
];

/// A value no catalog publishes, submitted to read a category's whole valid-value
/// list out of its own refusal.
const NO_SUCH_VERSION: &str = "0.0.0-no-such-version";

/// A `CreateCluster` body asking for `version` on brokers of `instance_type`: the
/// minimal provisioned v1 body, written out here rather than taken from the
/// harness's row type because the version is one this suite read off the wire.
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

/// Every ROW `ListKafkaVersions` publishes — a version together with the status it
/// is published under — in published order, walking the listing's pages as a
/// client does.
async fn published_rows(emulator: &Emulator) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut query = String::new();
    loop {
        let reply = emulator
            .call("GET", &format!("/v1/kafka-versions{query}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "ListKafkaVersions must succeed, got {} {}",
            reply.status, reply.body
        );
        for entry in reply
            .at("/kafkaVersions")
            .as_array()
            .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
        {
            let version = entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("a published row without a version: {entry}"))
                .to_string();
            let status = entry["status"]
                .as_str()
                .unwrap_or_else(|| panic!("a published row without a status: {entry}"))
                .to_string();
            rows.push((version, status));
        }
        match reply.at("/nextToken").as_str() {
            Some(token) => query = format!("?nextToken={token}"),
            None => break,
        }
    }
    assert!(
        !rows.is_empty(),
        "the catalog must publish at least one version row"
    );
    rows
}

/// The versions the catalog publishes as OFFERED, each once in published order: a
/// version is offered when any row the catalog publishes it under says so, which
/// is the reading a client reducing the listing to one status per version makes.
fn offered_versions(rows: &[(String, String)]) -> Vec<String> {
    let mut offered: Vec<String> = Vec::new();
    for (version, status) in rows {
        if status == OFFERED && !offered.contains(version) {
            offered.push(version.clone());
        }
    }
    offered
}

/// The versions the catalog publishes NO offered row for, each once in published
/// order: the ones it has stopped offering altogether.
fn retired_versions(rows: &[(String, String)]) -> Vec<String> {
    let offered = offered_versions(rows);
    let mut retired: Vec<String> = Vec::new();
    for (version, _) in rows {
        if !offered.contains(version) && !retired.contains(version) {
            retired.push(version.clone());
        }
    }
    retired
}

/// Assert `reply` is the version refusal of `case` carrying `expected`.
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

/// The valid-value list a category's refusal advertises, parsed out of the
/// sentence the service answers a create naming no published version with.
async fn advertised_versions(emulator: &Emulator, case: &str, instance_type: &str) -> Vec<String> {
    let name = cluster_name(&["advertised", instance_type]);
    let reply = emulator
        .create_cluster_v2(v2_provisioned_body(&name, NO_SUCH_VERSION, instance_type))
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

/// The ARN a create minted.
fn minted_arn(reply: &Reply, case: &str) -> String {
    reply
        .at("/clusterArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the create minted no clusterArn, got {}", reply.body))
        .to_string()
}

/// One admitted create: what it was asked for, and the cluster it minted.
struct Built {
    case: String,
    version: String,
    instance_type: &'static str,
    arn: String,
}

/// RULE: every version the catalog publishes as offered builds a new provisioned
/// cluster for some broker category — the oldest published offered version
/// included — and that cluster settles to `ACTIVE` and publishes the bootstrap
/// addresses of the brokers it was built with. Where one category refuses an
/// offered version, the refusal is that category's alone and the other category
/// builds on it.
///
/// The rows differ along the version (every offered row of the catalog, read off
/// the wire), the create that submits it (the v1 body and the v2 provisioned one,
/// alternating) and the instance family and size the brokers run on. Every create
/// is submitted before any cluster is polled, so the suite waits for the
/// transitions the clock owes them concurrently and waits for the CONDITION, never
/// for a span.
#[tokio::test]
async fn every_offered_version_builds_a_cluster_that_settles_and_publishes_its_brokers() {
    let emulator = Emulator::new();
    let rows = published_rows(&emulator).await;
    let offered = offered_versions(&rows);
    assert!(
        offered.len() > 1,
        "the catalog must publish more than one offered version for this rule to span, got {rows:?}"
    );

    let mut built: Vec<Built> = Vec::new();
    for (index, version) in offered.iter().enumerate() {
        // The STANDARD brokers are asked first, over a rotating family and size and
        // alternating between the two creates, which submit the same broker
        // configuration and so answer by the same rule.
        let instance_type = STANDARD_INSTANCE_TYPES[index % STANDARD_INSTANCE_TYPES.len()];
        let by_v2 = index % 2 == 0;
        let submitted_by = match by_v2 {
            true => "CreateClusterV2",
            false => "CreateCluster",
        };
        let case = format!("{submitted_by} Standard/{version} on {instance_type}");
        let name = cluster_name(&["offered", "standard", version]);
        let reply = match by_v2 {
            true => {
                emulator
                    .create_cluster_v2(v2_provisioned_body(&name, version, instance_type))
                    .await
            }
            false => {
                emulator
                    .call(
                        "POST",
                        "/v1/clusters",
                        Some(v1_body(&name, version, instance_type)),
                    )
                    .await
            }
        };
        if reply.status == 200 {
            built.push(Built {
                case: case.clone(),
                version: version.clone(),
                instance_type,
                arn: minted_arn(&reply, &case),
            });
            continue;
        }
        // A category that does not build on an offered version says so in the
        // service's own sentence, advertising its own offering — and the version is
        // then one the OTHER category does build on, so the refusal is about the
        // brokers asked for and never about a version the catalog offers.
        let advertised = advertised_versions(&emulator, &case, instance_type).await;
        assert_version_refusal(
            &reply,
            &case,
            &unsupported_broker_version_message("Standard", version, &advertised),
        );

        let express_type = EXPRESS_INSTANCE_TYPES[index % EXPRESS_INSTANCE_TYPES.len()];
        let case = format!("CreateClusterV2 Express/{version} on {express_type}");
        let name = cluster_name(&["offered", "express", version]);
        let reply = emulator
            .create_cluster_v2(v2_provisioned_body(&name, version, express_type))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: an offered version the standard brokers are refused must build a \
             cluster of the other category, got {} {}",
            reply.status, reply.body
        );
        built.push(Built {
            case: case.clone(),
            version: version.clone(),
            instance_type: express_type,
            arn: minted_arn(&reply, &case),
        });
    }

    // The OLDEST offered row the catalog publishes is one the STANDARD brokers are
    // built on: the published offering is not a steer onto the newest lines alone.
    let oldest = offered
        .first()
        .expect("the catalog publishes an offered version");
    let case = format!("the oldest offered version {oldest}");
    assert!(
        built
            .iter()
            .any(|cluster| &cluster.version == oldest
                && cluster.instance_type.starts_with("kafka.")),
        "case {case}: the oldest offered version must build a standard-broker cluster, got the \
         built set {:?}",
        built
            .iter()
            .map(|cluster| (cluster.version.as_str(), cluster.instance_type))
            .collect::<Vec<(&str, &str)>>()
    );

    // Every admitted create really stood a cluster up: it reports the version and
    // the brokers it was asked for, settles out of `CREATING` on its own, and then
    // publishes the addresses a client bootstraps against.
    for cluster in &built {
        let case = &cluster.case;
        poll_until_active(&emulator, &cluster.arn, case).await;
        let described = emulator.describe_v2(&cluster.arn).await;
        assert_eq!(
            described
                .at("/clusterInfo/provisioned/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(cluster.version.as_str()),
            "case {case}: the cluster must report the version it was built on, got {}",
            described.body
        );
        assert_eq!(
            described
                .at("/clusterInfo/provisioned/brokerNodeGroupInfo/instanceType")
                .as_str(),
            Some(cluster.instance_type),
            "case {case}: the cluster must report the brokers it was built with, got {}",
            described.body
        );
        assert_eq!(
            described.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: a settled cluster must report ACTIVE, got {}",
            described.body
        );
        let address = emulator
            .published_bootstrap(&cluster.arn, "bootstrapBrokerString", case)
            .await;
        assert!(
            address.contains(':'),
            "case {case}: the cluster must publish a host:port bootstrap address, got \
             {address:?}"
        );
    }
}

/// RULE: a version the catalog publishes no offered row for builds no new cluster
/// for either broker category, and the refusal's valid-value list names only
/// versions the same catalog publishes as offered — each once — so a client
/// retrying with a version the sentence named stays inside the catalog it
/// enumerated.
///
/// The rows differ along the retired version (every version the catalog publishes
/// only as deprecated), the category and the instance family and size inside it.
#[tokio::test]
async fn a_retired_version_builds_nothing_and_the_refusal_steers_inside_the_catalog() {
    let emulator = Emulator::new();
    let rows = published_rows(&emulator).await;
    let offered = offered_versions(&rows);
    let retired = retired_versions(&rows);
    assert!(
        !retired.is_empty(),
        "the catalog must publish a version it has stopped offering, got {rows:?}"
    );

    for (category, instance_types) in CATEGORIES {
        for instance_type in *instance_types {
            let advertised = advertised_versions(&emulator, category, instance_type).await;
            assert!(
                !advertised.is_empty(),
                "case {category}/{instance_type}: the refusal must advertise the versions a \
                 create may name"
            );
            for (index, version) in advertised.iter().enumerate() {
                assert!(
                    offered.contains(version),
                    "case {category}/{instance_type}/{version}: an advertised version must be \
                     one the catalog publishes as {OFFERED}, got the offered set {offered:?}"
                );
                assert!(
                    !advertised[..index].contains(version),
                    "case {category}/{instance_type}/{version}: the list must name each version \
                     once, got {advertised:?}"
                );
            }

            for version in &retired {
                let case = format!("{category}/{instance_type}/{version}");
                let name = cluster_name(&["retired", instance_type, version]);
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
