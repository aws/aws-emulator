// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for WHERE a SageMaker HyperPod node is launched: the
//! `NodeDetails.Placement` `DescribeClusterNode` reports for it, and the
//! `AvailabilityZones` a `BatchAddClusterNodes` entry carries for a node it added.
//!
//! SageMaker is mounted BESIDE Amazon EC2 on one shared engine, the way the assembled
//! emulator mounts them, because a subnet and its Availability Zone are EC2's: every subnet
//! a case below names is one EC2 minted through its own API, and the zone names and zone IDs
//! the assertions expect are read back from EC2 rather than written here.
//!
//! The suites assert a RULE over a TABLE of clusters rather than one recorded call: a node is
//! launched into a subnet of the VPC configuration its instance group runs under — the
//! group's own `OverrideVpcConfig` when it declares one, else the cluster's `VpcConfig` — so
//! the zone every reader reports for that node is the zone that subnet lives in, and a group
//! whose configuration names no subnet this account holds is still placed, in a zone of the
//! Region. The rows differ along how many subnets the configuration names, which zones they
//! live in, how many instances the group declares, whether the group overrides the cluster's
//! configuration, and whether EC2 holds the named subnet at all.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::inference_deployment_lifecycle::{error_code, str_field, ACCOUNT};

/// The Region every case signs for; a placement is resolved in the Region the caller
/// addressed, so the setup mints its subnets there too.
const REGION: &str = "us-east-1";

/// SageMaker and Amazon EC2 on ONE shared engine, plus an EC2 client over the router's own
/// in-process callback — the very seam a cluster's placement is resolved through.
struct Wired {
    router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
}

impl Wired {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the Region resolved is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            engine,
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        let ec2 = ::aws_sdk_ec2::Client::from_conf(
            ::aws_sdk_ec2::Config::builder()
                .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
                .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        Self { router, ec2 }
    }

    /// One SageMaker call over its awsJson1_1 wire, signed for [`REGION`].
    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call that must be served.
    async fn ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, answered) = self.call(target, body).await;
        assert!(
            status == 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {answered}"
        );
        answered
    }

    /// One VPC of the account with one subnet per entry of `zones`, all minted by EC2's own
    /// operations, so every identifier below is one EC2 handed out.
    ///
    /// `label` keeps one network's CIDR block apart from another's.
    async fn network(&self, label: u8, zones: &[&str]) -> Vec<Subnet> {
        let vpc_id = self
            .ec2
            .create_vpc()
            .cidr_block(format!("10.{label}.0.0/16"))
            .send()
            .await
            .expect("setup: EC2 must create the VPC")
            .vpc()
            .and_then(|vpc| vpc.vpc_id())
            .expect("setup: EC2 must report the VPC's id")
            .to_string();
        let mut subnets = Vec::new();
        for (index, zone) in zones.iter().enumerate() {
            let created = self
                .ec2
                .create_subnet()
                .vpc_id(&vpc_id)
                .cidr_block(format!("10.{label}.{}.0/24", index + 1))
                .availability_zone(*zone)
                .send()
                .await
                .expect("setup: EC2 must create the subnet")
                .subnet()
                .cloned()
                .expect("setup: EC2 must report the subnet");
            subnets.push(Subnet {
                id: created
                    .subnet_id()
                    .expect("setup: EC2 must report the subnet's id")
                    .to_string(),
                zone: (*zone).to_string(),
            });
        }
        subnets
    }

    /// The zones of the Region as EC2 lists them: each zone NAME with the zone ID EC2 pairs
    /// with it, in EC2's own order. That pairing is the owning service's, so every
    /// expectation below is read from EC2 instead of being derived here.
    async fn region_zones(&self) -> Vec<(String, String)> {
        self.ec2
            .describe_availability_zones()
            .send()
            .await
            .expect("setup: EC2 must report the Region's zones")
            .availability_zones()
            .iter()
            .filter_map(|zone| Some((zone.zone_name()?.to_string(), zone.zone_id()?.to_string())))
            .collect()
    }

    /// Every node of `cluster`, as `NodeLogicalId` -> `Placement`, read through
    /// `ListClusterNodes` and then `DescribeClusterNode` — the per-node reader whose
    /// `Placement` this suite is about.
    async fn placements(&self, case: &str, cluster: &str) -> BTreeMap<String, Option<Value>> {
        let listed = self
            .ok(
                case,
                "ListClusterNodes",
                &json!({ "ClusterName": cluster, "IncludeNodeLogicalIds": true }),
            )
            .await;
        let summaries = listed
            .get("ClusterNodeSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut placements = BTreeMap::new();
        for summary in &summaries {
            let logical_id = str_field(summary, "NodeLogicalId");
            let placement = self.placement_of(case, cluster, &logical_id).await;
            placements.insert(logical_id, placement);
        }
        placements
    }

    /// The `Placement` `DescribeClusterNode` reports for one node.
    async fn placement_of(&self, case: &str, cluster: &str, logical_id: &str) -> Option<Value> {
        let described = self
            .ok(
                case,
                "DescribeClusterNode",
                &json!({ "ClusterName": cluster, "NodeLogicalId": logical_id }),
            )
            .await;
        described
            .get("NodeDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected NodeDetails in {described}"))
            .get("Placement")
            .cloned()
    }
}

/// One subnet of the account: the identifier EC2 handed out and the zone it lives in.
struct Subnet {
    id: String,
    zone: String,
}

/// One instance group of a HyperPod cluster. `InstanceGroupName`, `InstanceCount`,
/// `InstanceType` and `ExecutionRole` are the members a create gives a group (smithy_query
/// shape=ClusterInstanceGroupSpecification).
fn instance_group(name: &str, instances: i64, override_subnets: &[String]) -> Value {
    let mut group = json!({
        "InstanceGroupName": name,
        "InstanceCount": instances,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    });
    if !override_subnets.is_empty() {
        group["OverrideVpcConfig"] = json!({
            "SecurityGroupIds": ["sg-0123456789abcdef0"],
            "Subnets": override_subnets,
        });
    }
    group
}

/// The `AvailabilityZone` and `AvailabilityZoneId` a `Placement` reports. A node with no
/// placement at all fails the case: a running node is somewhere.
fn reported(case: &str, logical_id: &str, placement: Option<&Value>) -> (String, Option<String>) {
    let placement = placement.unwrap_or_else(|| {
        panic!(
            "case {case:?}: node {logical_id:?} must report a Placement, the zone it was \
             launched in"
        )
    });
    (
        str_field(placement, "AvailabilityZone"),
        placement
            .get("AvailabilityZoneId")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// One row of the placement table.
struct Row {
    case: &'static str,
    /// The subnets the cluster's `VpcConfig` names; empty for a cluster that names no VPC
    /// configuration at all.
    subnets: Vec<String>,
    /// The subnets the group's own `OverrideVpcConfig` names; empty for a group that
    /// declares none.
    group_overrides: Vec<String>,
    instances: i64,
    /// The zones the group's nodes must be launched in — every one of them, since each row
    /// declares at least as many instances as zones.
    zones: Vec<String>,
}

/// RULE: every node of an instance group is launched into a subnet of the VPC configuration
/// that group runs under — its own `OverrideVpcConfig` when it declares subnets, else the
/// cluster's `VpcConfig` — so the zone and zone ID reported for the node are that subnet's
/// own; a group whose configuration names no subnet the account holds is still placed, in a
/// zone of the Region.
#[tokio::test]
async fn a_nodes_placement_is_the_zone_of_the_subnet_its_group_runs_under() {
    let wired = Wired::new();
    let region_zones = wired.region_zones().await;
    let zone_ids: BTreeMap<String, String> = region_zones.iter().cloned().collect();
    // Three networks of the account, each minted through EC2's own API: one spanning a
    // single zone, one spanning two, and one whose subnet a group overrides into.
    let one_zone = wired.network(10, &["us-east-1a"]).await;
    let two_zones = wired.network(20, &["us-east-1c", "us-east-1d"]).await;
    let overridden = wired.network(30, &["us-east-1b"]).await;
    // The FIRST zone the Region publishes, which is where a group with no resolvable subnet
    // is placed.
    let region_first = region_zones
        .first()
        .map(|(name, _)| name.clone())
        .expect("the Region publishes zones");
    // An identifier inside the modelled form that EC2 holds no subnet for.
    let unheld_subnet = "subnet-0f1e2d3c4b5a69781".to_string();

    let rows: Vec<Row> = vec![
        Row {
            case: "one subnet, one instance",
            subnets: vec![one_zone[0].id.clone()],
            group_overrides: Vec::new(),
            instances: 1,
            zones: vec![one_zone[0].zone.clone()],
        },
        Row {
            case: "one subnet outside the Region's first zone, two instances",
            subnets: vec![two_zones[0].id.clone()],
            group_overrides: Vec::new(),
            instances: 2,
            zones: vec![two_zones[0].zone.clone()],
        },
        Row {
            case: "two subnets in two zones, three instances",
            subnets: vec![two_zones[0].id.clone(), two_zones[1].id.clone()],
            group_overrides: Vec::new(),
            instances: 3,
            zones: vec![two_zones[0].zone.clone(), two_zones[1].zone.clone()],
        },
        Row {
            case: "a group overriding the cluster's configuration",
            subnets: vec![one_zone[0].id.clone()],
            group_overrides: vec![overridden[0].id.clone()],
            instances: 2,
            zones: vec![overridden[0].zone.clone()],
        },
        Row {
            case: "no VPC configuration at all",
            subnets: Vec::new(),
            group_overrides: Vec::new(),
            instances: 2,
            zones: vec![region_first.clone()],
        },
        Row {
            case: "a subnet EC2 holds no record for",
            subnets: vec![unheld_subnet.clone()],
            group_overrides: Vec::new(),
            instances: 1,
            zones: vec![region_first.clone()],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let cluster = format!("placement-{index}");
        let mut request = json!({
            "ClusterName": cluster,
            "InstanceGroups": [instance_group("controller", row.instances, &row.group_overrides)],
        });
        if !row.subnets.is_empty() {
            request["VpcConfig"] = json!({
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
                "Subnets": row.subnets,
            });
        }
        wired.ok(row.case, "CreateCluster", &request).await;

        let placements = wired.placements(row.case, &cluster).await;
        assert!(
            placements.len() == usize::try_from(row.instances).expect("a positive count"),
            "case {:?}: expected one node per declared instance ({}), got {}",
            row.case,
            row.instances,
            placements.len()
        );
        let expected: BTreeSet<String> = row.zones.iter().cloned().collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (logical_id, placement) in &placements {
            let (zone, zone_id) = reported(row.case, logical_id, placement.as_ref());
            assert!(
                expected.contains(&zone),
                "case {:?}: node {logical_id:?} must be launched in one of {:?}, got {zone:?}",
                row.case,
                row.zones
            );
            assert!(
                zone_id == zone_ids.get(&zone).cloned(),
                "case {:?}: node {logical_id:?} must report the zone ID the account's own \
                 network service pairs with {zone:?} ({:?}), got {zone_id:?}",
                row.case,
                zone_ids.get(&zone)
            );
            seen.insert(zone);
        }
        assert!(
            seen == expected,
            "case {:?}: a group placed across {:?} launches nodes in every one of them, got \
             {seen:?}",
            row.case,
            row.zones
        );

        // A read must not change what a later caller sees: the placement was resolved once,
        // when the cluster was created, so reading again reports the same zones.
        let again = wired.placements(row.case, &cluster).await;
        assert!(
            again == placements,
            "case {:?}: a second read must report the same nodes and placements, got \
             {again:?} after {placements:?}",
            row.case
        );
    }
}

/// RULE: a node `BatchAddClusterNodes` adds is placed the way its group's provisioning
/// places one — the zones of the subnets the group runs under, taken in turn — unless the
/// specification TARGETS zones, in which case the node is launched in the zone it named; the
/// entry the call answers carries that zone in `AvailabilityZones`, and the per-node reader
/// reports the same placement.
#[tokio::test]
async fn an_added_node_reports_the_zone_it_was_placed_in() {
    let wired = Wired::new();
    let zone_ids: BTreeMap<String, String> = wired.region_zones().await.into_iter().collect();
    let network = wired
        .network(40, &["us-east-1a", "us-east-1b", "us-east-1c"])
        .await;
    let cluster = "added-placement";
    wired
        .ok(
            "add-setup",
            "CreateCluster",
            &json!({
                "ClusterName": cluster,
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    "Subnets": [&network[0].id, &network[1].id],
                },
                "InstanceGroups": [instance_group("controller", 0, &[])],
            }),
        )
        .await;
    // The cluster's own provisioning is settled first: a group that declared no instance
    // holds no node, so every node below is one an addition placed.
    assert!(
        wired.placements("add-setup", cluster).await.is_empty(),
        "setup: a group declaring no instance holds no node"
    );

    // The zones the group itself runs in, from the subnets its cluster's configuration
    // names, and one the group does not run in.
    let group_zones = [network[0].zone.clone(), network[1].zone.clone()];
    let outside = network[2].zone.clone();
    // Rows differ along whether the specification targets a zone, and whether the zone it
    // targets is one the group's own subnets span.
    let rows: Vec<(&str, Option<String>, Vec<String>, bool)> = vec![
        (
            "no zone targeted",
            None,
            group_zones.to_vec(),
            // The group's own zones carry the pairing the owning service stated for them.
            true,
        ),
        (
            "a zone the group's subnets span",
            Some(group_zones[1].clone()),
            vec![group_zones[1].clone()],
            true,
        ),
        (
            "a zone outside the group's subnets",
            Some(outside.clone()),
            vec![outside.clone()],
            // A zone only the request named: the group holds no pairing for it, and the
            // pairing is not derived here, so the node reports the name alone.
            false,
        ),
    ];

    for (case, targeted, admissible, paired) in rows {
        let mut spec = json!({
            "InstanceGroupName": "controller",
            "IncrementTargetCountBy": 2,
        });
        if let Some(zone) = &targeted {
            spec["AvailabilityZones"] = json!([zone]);
        }
        let added = wired
            .ok(
                case,
                "BatchAddClusterNodes",
                &json!({ "ClusterName": cluster, "NodesToAdd": [spec] }),
            )
            .await;
        let successful = added
            .get("Successful")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            successful.len() == 2,
            "case {case:?}: expected one entry per added node, got {added}"
        );
        let mut answered_zones: BTreeSet<String> = BTreeSet::new();
        for entry in &successful {
            let zones: Vec<String> = entry
                .get("AvailabilityZones")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every added node reports its zones, got {entry}")
                })
                .iter()
                .filter_map(|zone| zone.as_str().map(str::to_string))
                .collect();
            assert!(
                zones.len() == 1 && admissible.contains(&zones[0]),
                "case {case:?}: the entry must report one of the zones {admissible:?}, got \
                 {zones:?}"
            );
            answered_zones.insert(zones[0].clone());

            // The per-node reader reports the same placement the entry did.
            let logical_id = str_field(entry, "NodeLogicalId");
            let placement = wired.placement_of(case, cluster, &logical_id).await;
            let (zone, zone_id) = reported(case, &logical_id, placement.as_ref());
            assert!(
                zone == zones[0],
                "case {case:?}: node {logical_id:?} must report the zone the add answered \
                 ({:?}), got {zone:?}",
                zones[0]
            );
            let expected_id = if paired { zone_ids.get(&zone).cloned() } else { None };
            assert!(
                zone_id == expected_id,
                "case {case:?}: node {logical_id:?} must report the zone ID {expected_id:?}, \
                 got {zone_id:?}"
            );
        }
        if targeted.is_none() {
            // Two nodes added to a group spanning two zones cover both: the group's zones
            // are taken in turn rather than every node stacked in the first.
            let expected: BTreeSet<String> = admissible.iter().cloned().collect();
            assert!(
                answered_zones == expected,
                "case {case:?}: an addition spreads over the group's zones {expected:?}, got \
                 {answered_zones:?}"
            );
        }
    }

    // A cluster this account does not hold is refused rather than placed.
    let (status, refused) = wired
        .call(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": "no-such-cluster",
                "NodesToAdd": [{ "InstanceGroupName": "controller", "IncrementTargetCountBy": 1 }],
            }),
        )
        .await;
    assert!(
        status == 400 && error_code(&refused) == "ResourceNotFound",
        "an add against a cluster the account does not hold is refused ResourceNotFound, got \
         {status} with {refused}"
    );
}
