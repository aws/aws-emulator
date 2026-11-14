// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instances an EMR cluster runs on, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`. Nothing reaches into either service's state struct: an instance is read back
//! through `ListInstances` and then resolved in Amazon EC2 through `DescribeInstances`, so
//! these tests are evidence that the listing is computed from persisted state AND that what
//! it names really exists in the service that owns instances.
//!
//! THE RULE: a cluster's capacity is Amazon EC2's resource, so Amazon EMR launches it there
//! and `ListInstances` reports one record per instance Amazon EC2 minted for the cluster's
//! instance groups and instance fleets — each naming the collection it belongs to, the
//! Amazon EC2 instance it is, its type and market, and the lifecycle state the clock has
//! reached (`PROVISIONING` then `RUNNING`, and `TERMINATED` once the cluster has terminated)
//! — narrowed by the request's collection, collection-type and instance-state filters and
//! paged by its `Marker`. A role the cluster places is launched into an Amazon EC2 placement
//! group of that cluster, and every Amazon EC2 resource Amazon EMR creates for a cluster
//! carries that cluster's id as its service-managed tag.
//!
//! Amazon EC2 is mounted BESIDE Amazon EMR on one engine, exactly as the assembled router
//! mounts them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    LIST_INSTANCES_PAGE_SIZE, MAX_INSTANCES_PER_GROUP,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The zone the subnet every cluster here is placed in lives in.
const ZONE: &str = "us-west-2a";

/// The roles a launch names.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// The release every launch here names.
const RELEASE_LABEL: &str = "emr-7.9.0";

/// How long a wait for a clock-owed move looks for it before giving up. Generous: it bounds
/// a condition that is already due in well under a second, so a loaded host cannot fail it.
const WAIT_DEADLINE: Duration = Duration::from_secs(90);

/// One look inside that deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The service-managed tag naming the cluster an Amazon EC2 resource was created for, and
/// the one naming the role an instance fills (Amazon EMR Management Guide, "Tagging Amazon
/// EMR clusters").
const CLUSTER_ID_TAG: &str = "aws:elasticmapreduce:job-flow-id";
const INSTANCE_ROLE_TAG: &str = "aws:elasticmapreduce:instance-group-role";

/// Amazon EMR and Amazon EC2 on one engine, plus an EC2 client over the router's own
/// in-process callback — the seam this suite resolves what EMR launched through.
struct Emulator {
    router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let callback = EmbeddedEmulator::from_router(router.clone());
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Emulator {
        router,
        ec2: ::aws_sdk_ec2::Client::from_conf(config),
    }
}

/// One `awsJson1_1` Amazon EMR call, which must be served.
async fn emr(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, payload) = emr_call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One Amazon EMR call, answering the status and the payload whatever they are.
async fn emr_call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticmapreduce.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
                 elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The subnet of one VPC Amazon EC2 itself minted, so every cluster here is placed in a
/// subnet the owning service holds.
async fn subnet(emulator: &Emulator) -> String {
    let vpc = emulator
        .ec2
        .create_vpc()
        .cidr_block("10.0.0.0/16")
        .send()
        .await
        .expect("EC2 must create the VPC");
    let vpc_id = vpc
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc reports the VPC it minted")
        .to_string();
    let created = emulator
        .ec2
        .create_subnet()
        .vpc_id(&vpc_id)
        .cidr_block("10.0.1.0/24")
        .availability_zone(ZONE)
        .send()
        .await
        .expect("EC2 must create the subnet");
    created
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .expect("CreateSubnet reports the subnet it minted")
        .to_string()
}

/// A cluster launched with `instances` in `subnet_id`, answering its id.
async fn launch(emulator: &Emulator, name: &str, subnet_id: &str, instances: Value) -> String {
    let mut instances = instances;
    instances["Ec2SubnetId"] = json!(subnet_id);
    instances["KeepJobFlowAliveWhenNoSteps"] = json!(true);
    let request = json!({
        "Name": name,
        "ReleaseLabel": RELEASE_LABEL,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": instances,
    });
    emr(&emulator.router, "RunJobFlow", request).await["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// Every instance `ListInstances` reports for `cluster_id` under `filters`, following the
/// `Marker` to its end — which is what collects the whole listing.
async fn list_instances(
    emulator: &Emulator,
    case: &str,
    cluster_id: &str,
    filters: Value,
) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "ClusterId": cluster_id });
        for (member, value) in filters.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        if let Some(token) = marker.as_ref() {
            request["Marker"] = json!(token);
        }
        let out = emr(&emulator.router, "ListInstances", request).await;
        let page = out["Instances"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: ListInstances must answer Instances, got {out}")
        });
        all.extend(page);
        match out["Marker"].as_str() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    all
}

/// The instances of `cluster_id` once every one of them has reached `state`, looked for
/// until the deadline passes rather than waited out.
async fn instances_in_state(
    emulator: &Emulator,
    case: &str,
    cluster_id: &str,
    expected: usize,
    state: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + WAIT_DEADLINE;
    let last = loop {
        let listed = list_instances(emulator, case, cluster_id, json!({})).await;
        let settled = listed.len() == expected
            && listed
                .iter()
                .all(|instance| instance["Status"]["State"].as_str() == Some(state));
        if settled || Instant::now() >= deadline {
            break listed;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        last.len(),
        expected,
        "case {case:?}: expected {expected} instances, got {last:?}"
    );
    for instance in &last {
        assert_eq!(
            instance["Status"]["State"].as_str(),
            Some(state),
            "case {case:?}: every instance must reach {state:?}, got {instance}"
        );
    }
    last
}

/// The instance groups of `cluster_id`, as `(group id, role)`.
async fn groups(emulator: &Emulator, cluster_id: &str) -> BTreeMap<String, String> {
    emr(
        &emulator.router,
        "ListInstanceGroups",
        json!({ "ClusterId": cluster_id }),
    )
    .await["InstanceGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|group| {
            (
                group["Id"].as_str().unwrap_or_default().to_string(),
                group["InstanceGroupType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// What Amazon EC2 reports for the instance `ec2_instance_id`: its state name, the subnet it
/// is in, its type, its placement group and its tags.
struct Ec2Instance {
    subnet_id: Option<String>,
    instance_type: Option<String>,
    placement_group: Option<String>,
    tags: BTreeMap<String, String>,
}

/// The instances Amazon EC2 holds under `ec2_instance_ids`, keyed by id — so an id EC2 does
/// not hold is simply absent, which is what makes a missing entry a failed assertion rather
/// than a panic.
async fn described_in_ec2(
    emulator: &Emulator,
    ec2_instance_ids: &[String],
) -> BTreeMap<String, Ec2Instance> {
    let answer = emulator
        .ec2
        .describe_instances()
        .set_instance_ids(Some(ec2_instance_ids.to_vec()))
        .send()
        .await
        .expect("EC2 must describe the instances EMR launched");
    let mut held = BTreeMap::new();
    for reservation in answer.reservations() {
        for instance in reservation.instances() {
            let Some(id) = instance.instance_id() else {
                continue;
            };
            held.insert(
                id.to_string(),
                Ec2Instance {
                    subnet_id: instance.subnet_id().map(str::to_string),
                    instance_type: instance.instance_type().map(|kind| kind.as_str().to_string()),
                    placement_group: instance
                        .placement()
                        .and_then(::aws_sdk_ec2::types::Placement::group_name)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string),
                    tags: instance
                        .tags()
                        .iter()
                        .filter_map(|tag| Some((tag.key()?.to_string(), tag.value()?.to_string())))
                        .collect(),
                },
            );
        }
    }
    held
}

/// The `Ec2InstanceId` of every reported instance.
fn ec2_ids(instances: &[Value]) -> Vec<String> {
    instances
        .iter()
        .map(|instance| {
            instance["Ec2InstanceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the collection table: what `Instances` asks for, how many instances that is,
/// and the `(role, instance type, market)` each of them must report.
struct Case {
    label: &'static str,
    instances: Value,
    expected: Vec<(&'static str, &'static str, &'static str)>,
}

/// THE RULE over the collection forms a cluster states its capacity in.
///
/// The rows span what the rule generalises over: the uniform `MasterInstanceType` /
/// `SlaveInstanceType` / `InstanceCount` form, instance groups of two roles and two sizes, a
/// group on the Spot market, and an instance fleet whose capacity is stated in weighted
/// units — so the count and the reported facts are read off each collection rather than off
/// one shape of request.
#[tokio::test]
async fn list_instances_reports_one_ec2_instance_per_instance_the_cluster_asked_for() {
    let cases = vec![
        Case {
            label: "the uniform form, one primary",
            instances: json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            expected: vec![("MASTER", "m5.xlarge", "ON_DEMAND")],
        },
        Case {
            label: "the uniform form, a primary and two core nodes",
            instances: json!({
                "InstanceCount": 3,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.large",
            }),
            expected: vec![
                ("MASTER", "m5.xlarge", "ON_DEMAND"),
                ("CORE", "m5.large", "ON_DEMAND"),
                ("CORE", "m5.large", "ON_DEMAND"),
            ],
        },
        Case {
            label: "instance groups of three roles",
            instances: json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                    { "InstanceRole": "CORE", "InstanceType": "m5.2xlarge", "InstanceCount": 2 },
                    { "InstanceRole": "TASK", "InstanceType": "m5.large", "InstanceCount": 1 },
                ],
            }),
            expected: vec![
                ("MASTER", "m5.xlarge", "ON_DEMAND"),
                ("CORE", "m5.2xlarge", "ON_DEMAND"),
                ("CORE", "m5.2xlarge", "ON_DEMAND"),
                ("TASK", "m5.large", "ON_DEMAND"),
            ],
        },
        Case {
            label: "a task group on the Spot market",
            instances: json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                    {
                        "InstanceRole": "TASK",
                        "InstanceType": "m5.large",
                        "InstanceCount": 2,
                        "Market": "SPOT",
                        "BidPrice": "0.10",
                    },
                ],
            }),
            expected: vec![
                ("MASTER", "m5.xlarge", "ON_DEMAND"),
                ("TASK", "m5.large", "SPOT"),
                ("TASK", "m5.large", "SPOT"),
            ],
        },
        Case {
            label: "an instance fleet whose weighted units need two instances",
            instances: json!({
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 4,
                    "InstanceTypeConfigs": [
                        { "InstanceType": "m5.xlarge", "WeightedCapacity": 2 },
                    ],
                }],
            }),
            expected: vec![
                ("MASTER", "m5.xlarge", "ON_DEMAND"),
                ("MASTER", "m5.xlarge", "ON_DEMAND"),
            ],
        },
    ];

    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;
    for case in &cases {
        let label = case.label;
        let cluster_id = launch(&emulator, label, &subnet_id, case.instances.clone()).await;

        // Reported straight after the launch, the instances are still coming up: the span
        // the clock owes them is never zero, so a caller that lists at once sees the
        // transient state.
        let launched = list_instances(&emulator, label, &cluster_id, json!({})).await;
        assert_eq!(
            launched.len(),
            case.expected.len(),
            "case {label:?}: expected {} instances, got {launched:?}",
            case.expected.len()
        );
        for instance in &launched {
            assert_eq!(
                instance["Status"]["State"].as_str(),
                Some("PROVISIONING"),
                "case {label:?}: a just-launched instance is PROVISIONING, got {instance}"
            );
        }

        let settled =
            instances_in_state(&emulator, label, &cluster_id, case.expected.len(), "RUNNING").await;
        let collections = groups(&emulator, &cluster_id).await;

        // What each instance reports: the collection it belongs to, its type and its market.
        let mut reported: Vec<(String, String, String)> = Vec::new();
        for instance in &settled {
            let role = match instance["InstanceGroupId"].as_str() {
                Some(group_id) => {
                    let role = collections.get(group_id).cloned().unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: {group_id} is no instance group of the cluster, \
                             which holds {collections:?}"
                        )
                    });
                    assert!(
                        instance["InstanceFleetId"].is_null(),
                        "case {label:?}: an instance of a group names no fleet, got {instance}"
                    );
                    role
                }
                None => {
                    let fleet_id = instance["InstanceFleetId"].as_str().unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: an instance names its group or its fleet, \
                             got {instance}"
                        )
                    });
                    assert!(
                        fleet_id.starts_with("if-"),
                        "case {label:?}: {fleet_id:?} is no instance fleet id"
                    );
                    "MASTER".to_string()
                }
            };
            let id = instance["Id"].as_str().unwrap_or_default();
            assert!(
                id.starts_with("ci-") && id.len() > 3,
                "case {label:?}: {id:?} is no Amazon EMR instance id"
            );
            assert!(
                !instance["PrivateIpAddress"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "case {label:?}: an instance reports the private address EC2 gave it, \
                 got {instance}"
            );
            reported.push((
                role,
                instance["InstanceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                instance["Market"].as_str().unwrap_or_default().to_string(),
            ));
        }
        reported.sort();
        let mut expected: Vec<(String, String, String)> = case
            .expected
            .iter()
            .map(|(role, kind, market)| {
                (role.to_string(), kind.to_string(), market.to_string())
            })
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected these (role, type, market) instances, got {settled:?}"
        );

        // BACKED: every instance the cluster reports is one Amazon EC2 holds, in the subnet
        // the launch placed the cluster in and of the type the collection asked for, carrying
        // the cluster and the role as its service-managed tags.
        let ids = ec2_ids(&settled);
        let ids_are_unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            ids_are_unique.len(),
            ids.len(),
            "case {label:?}: two instances must not name one EC2 instance, got {ids:?}"
        );
        let held = described_in_ec2(&emulator, &ids).await;
        for (instance, id) in settled.iter().zip(&ids) {
            let ec2 = held.get(id).unwrap_or_else(|| {
                panic!("case {label:?}: EC2 holds no instance {id:?}, only {:?}", held.keys())
            });
            assert_eq!(
                ec2.subnet_id.as_deref(),
                Some(subnet_id.as_str()),
                "case {label:?}: {id} must run in the cluster's subnet"
            );
            assert_eq!(
                ec2.instance_type.as_deref(),
                instance["InstanceType"].as_str(),
                "case {label:?}: {id} must be of the type the cluster reports"
            );
            assert_eq!(
                ec2.tags.get(CLUSTER_ID_TAG).map(String::as_str),
                Some(cluster_id.as_str()),
                "case {label:?}: {id} must carry the cluster's own tag, got {:?}",
                ec2.tags
            );
            assert!(
                ec2.tags.contains_key(INSTANCE_ROLE_TAG),
                "case {label:?}: {id} must carry its role tag, got {:?}",
                ec2.tags
            );
        }
    }
}

/// THE RULE's bound: a group asks for at most the instances Amazon EMR provisions for one
/// (`crate::state::MAX_INSTANCES_PER_GROUP`), whichever form states the capacity, and a
/// launch past it is refused before anything is stored or asked of Amazon EC2.
///
/// The rows span the two forms that state a count — the uniform `InstanceCount` and an
/// explicit `InstanceGroups` entry — one past the ceiling each. The ceiling itself is not
/// exercised from the serving side: a cluster AT it would be 65,536 Amazon EC2 instances, so
/// what is asserted there is that a count well inside it is served (every other test here
/// does exactly that).
#[tokio::test]
async fn a_launch_past_the_instance_group_ceiling_is_refused_and_launches_nothing() {
    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;
    let past = MAX_INSTANCES_PER_GROUP as i64 + 1;
    let rows: Vec<(&str, Value)> = vec![
        (
            "the uniform form, whose implied core group is one past the ceiling",
            json!({
                // The uniform form's primary takes one instance of the total, so the group
                // it implies for the rest is one past the ceiling at this count.
                "InstanceCount": past + 1,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
                "Ec2SubnetId": subnet_id.clone(),
            }),
        ),
        (
            "an instance group one past the ceiling",
            json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                    {
                        "InstanceRole": "CORE",
                        "InstanceType": "m5.xlarge",
                        "InstanceCount": past,
                    },
                ],
                "Ec2SubnetId": subnet_id.clone(),
            }),
        ),
    ];

    for (label, instances) in &rows {
        let (status, payload) = emr_call(
            &emulator.router,
            "RunJobFlow",
            json!({
                "Name": label,
                "ReleaseLabel": RELEASE_LABEL,
                "JobFlowRole": INSTANCE_PROFILE,
                "ServiceRole": SERVICE_ROLE,
                "Instances": instances.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert!(
            payload["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("ValidationException"),
            "case {label:?}: expected ValidationException, got {payload}"
        );
        assert!(
            payload["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&MAX_INSTANCES_PER_GROUP.to_string()),
            "case {label:?}: the refusal must name the ceiling, got {payload}"
        );
    }

    // Nothing was launched for a refused request: the account holds no cluster, so Amazon EC2
    // holds no instance of one.
    let clusters = emr(&emulator.router, "ListClusters", json!({})).await;
    assert_eq!(
        clusters["Clusters"].as_array().map(Vec::len),
        Some(0),
        "a refused launch stores no cluster, got {clusters}"
    );
    let held = emulator
        .ec2
        .describe_instances()
        .send()
        .await
        .expect("EC2 must answer a describe of every instance");
    let launched: usize = held
        .reservations()
        .iter()
        .map(|reservation| reservation.instances().len())
        .sum();
    assert_eq!(
        launched, 0,
        "a refused launch asks Amazon EC2 for no instance, got {launched}"
    );
}

/// THE RULE's pagination: a cluster holding more instances than one page reports them over
/// several pages, each at most `LIST_INSTANCES_PAGE_SIZE`, and the `Marker` resumes the
/// listing exactly where the page stopped — so every instance appears once and none is lost
/// at a boundary.
#[tokio::test]
async fn the_listing_pages_a_cluster_holding_more_instances_than_one_page() {
    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;
    let case = "one instance past a whole page";
    // One past the page size, so there are exactly two pages and the second holds one row:
    // the smallest cluster that proves the boundary.
    let total = LIST_INSTANCES_PAGE_SIZE + 1;
    let cluster_id = launch(
        &emulator,
        case,
        &subnet_id,
        json!({
            "InstanceGroups": [
                { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                {
                    "InstanceRole": "CORE",
                    "InstanceType": "m5.xlarge",
                    "InstanceCount": total - 1,
                },
            ],
        }),
    )
    .await;

    // The pages, taken one at a time so the sizes and the resume position are asserted rather
    // than collected away by the harness helper.
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "ClusterId": cluster_id.clone() });
        if let Some(token) = marker.as_ref() {
            request["Marker"] = json!(token);
        }
        let out = emr(&emulator.router, "ListInstances", request).await;
        let page: Vec<String> = out["Instances"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|instance| instance["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            page.len() <= LIST_INSTANCES_PAGE_SIZE,
            "case {case:?}: a page holds at most {LIST_INSTANCES_PAGE_SIZE} instances, got {}",
            page.len()
        );
        match out["Marker"].as_str() {
            // The marker names the first instance the NEXT page reports, so it is the row the
            // page stopped before.
            Some(token) => {
                marker = Some(token.to_string());
                assert!(
                    !page.contains(&token.to_string()),
                    "case {case:?}: the marker must name a row this page did not report, \
                     got {token} in {page:?}"
                );
                pages.push(page);
            }
            None => {
                pages.push(page);
                break;
            }
        }
    }

    assert_eq!(
        pages.len(),
        2,
        "case {case:?}: {total} instances must take two pages, got {pages:?}"
    );
    assert_eq!(
        pages.first().map(Vec::len),
        Some(LIST_INSTANCES_PAGE_SIZE),
        "case {case:?}: the first page is full, got {pages:?}"
    );
    let walked: Vec<String> = pages.concat();
    let unique: BTreeSet<&String> = walked.iter().collect();
    assert_eq!(
        (walked.len(), unique.len()),
        (total, total),
        "case {case:?}: every instance appears exactly once over the pages, got {walked:?}"
    );
}

/// THE RULE for the filters and the paging: each filter the request carries narrows the
/// listing, and the `Marker` resumes it where the page stopped.
///
/// The rows span the filters the operation takes: a collection by id, a collection by role
/// (one role and two), and a state (the one the instances are in and one they are not).
#[tokio::test]
async fn list_instances_narrows_the_listing_by_every_filter_the_request_carries() {
    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;
    let case = "a cluster of three groups";
    let cluster_id = launch(
        &emulator,
        case,
        &subnet_id,
        json!({
            "InstanceGroups": [
                { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                { "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 2 },
                { "InstanceRole": "TASK", "InstanceType": "m5.large", "InstanceCount": 1 },
            ],
        }),
    )
    .await;
    let all = instances_in_state(&emulator, case, &cluster_id, 4, "RUNNING").await;
    let collections = groups(&emulator, &cluster_id).await;
    let group_of = |role: &str| -> String {
        collections
            .iter()
            .find(|(_, held)| held.as_str() == role)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("the cluster holds a {role} group, got {collections:?}"))
    };

    let rows: Vec<(&str, Value, usize)> = vec![
        ("no filter at all", json!({}), 4),
        (
            "one group by id",
            json!({ "InstanceGroupId": group_of("CORE") }),
            2,
        ),
        (
            "one group by role",
            json!({ "InstanceGroupTypes": ["MASTER"] }),
            1,
        ),
        (
            "two groups by role",
            json!({ "InstanceGroupTypes": ["CORE", "TASK"] }),
            3,
        ),
        (
            "a role no group of the cluster fills",
            json!({ "InstanceFleetType": "CORE" }),
            0,
        ),
        (
            "the state the instances are in",
            json!({ "InstanceStates": ["RUNNING"] }),
            4,
        ),
        (
            "a state none of them is in",
            json!({ "InstanceStates": ["TERMINATED"] }),
            0,
        ),
        (
            "a state filter listing both",
            json!({ "InstanceStates": ["RUNNING", "TERMINATED"] }),
            4,
        ),
        (
            "a group by role and a state together",
            json!({ "InstanceGroupTypes": ["TASK"], "InstanceStates": ["RUNNING"] }),
            1,
        ),
        (
            "a group by role and a state nothing is in",
            json!({ "InstanceGroupTypes": ["TASK"], "InstanceStates": ["PROVISIONING"] }),
            0,
        ),
    ];

    for (label, filters, expected) in &rows {
        let selected = list_instances(&emulator, label, &cluster_id, filters.clone()).await;
        assert_eq!(
            selected.len(),
            *expected,
            "case {label:?}: expected {expected} instances, got {selected:?}"
        );
        // Every selected instance is one of the cluster's own, named by the same id the
        // unfiltered listing reports it under.
        let known: BTreeSet<&str> = all
            .iter()
            .map(|instance| instance["Id"].as_str().unwrap_or_default())
            .collect();
        for instance in &selected {
            let id = instance["Id"].as_str().unwrap_or_default();
            assert!(
                known.contains(id),
                "case {label:?}: {id:?} is not one of the cluster's instances {known:?}"
            );
        }
    }

    // A marker no instance of the cluster carries is refused with the modelled 400, and a
    // cluster nothing was launched under is refused the same way.
    for (label, request) in [
        (
            "a marker of no instance",
            json!({ "ClusterId": cluster_id.clone(), "Marker": "ci-NOTAMARKER" }),
        ),
        (
            "a cluster the account does not hold",
            json!({ "ClusterId": "j-ZZZZZZZZZZZZZ" }),
        ),
    ] {
        let (status, payload) = emr_call(&emulator.router, "ListInstances", request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert!(
            payload["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {payload}"
        );
    }
}

/// THE RULE for a cluster with more than one primary node: its primaries are launched into
/// ONE Amazon EC2 placement group of the cluster, which carries the cluster's own tag and is
/// released when the cluster is terminated — and every instance of a terminated cluster is
/// `TERMINATED`.
#[tokio::test]
async fn a_highly_available_cluster_places_its_primaries_in_one_group_and_releases_it() {
    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;
    let case = "three primaries and two core nodes";
    let cluster_id = launch(
        &emulator,
        case,
        &subnet_id,
        json!({
            "InstanceGroups": [
                { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 3 },
                { "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 2 },
            ],
        }),
    )
    .await;
    let settled = instances_in_state(&emulator, case, &cluster_id, 5, "RUNNING").await;
    let collections = groups(&emulator, &cluster_id).await;
    let ids = ec2_ids(&settled);
    let held = described_in_ec2(&emulator, &ids).await;

    let mut primary_groups: BTreeSet<String> = BTreeSet::new();
    for (instance, id) in settled.iter().zip(&ids) {
        let role = instance["InstanceGroupId"]
            .as_str()
            .and_then(|group_id| collections.get(group_id).cloned())
            .unwrap_or_default();
        let ec2 = held
            .get(id)
            .unwrap_or_else(|| panic!("case {case:?}: EC2 holds no instance {id:?}"));
        if role == "MASTER" {
            let group = ec2.placement_group.clone().unwrap_or_else(|| {
                panic!("case {case:?}: a primary must be in a placement group, got none for {id}")
            });
            primary_groups.insert(group);
        } else {
            assert!(
                ec2.placement_group.is_none(),
                "case {case:?}: only the placed role is in a group, got {:?} for {id}",
                ec2.placement_group
            );
        }
    }
    assert_eq!(
        primary_groups.len(),
        1,
        "case {case:?}: the primaries must share ONE placement group, got {primary_groups:?}"
    );
    let group_name = primary_groups
        .iter()
        .next()
        .expect("the primaries share one group")
        .clone();

    // The group is a resource Amazon EMR created for this cluster, so it carries the
    // cluster's own tag.
    let described = emulator
        .ec2
        .describe_placement_groups()
        .group_names(&group_name)
        .send()
        .await
        .expect("EC2 must describe the placement group EMR created");
    let tags: BTreeMap<String, String> = described
        .placement_groups()
        .iter()
        .find(|group| group.group_name() == Some(group_name.as_str()))
        .map(|group| {
            group
                .tags()
                .iter()
                .filter_map(|tag| Some((tag.key()?.to_string(), tag.value()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        tags.get(CLUSTER_ID_TAG).map(String::as_str),
        Some(cluster_id.as_str()),
        "case {case:?}: the placement group must carry the cluster's tag, got {tags:?}"
    );

    // A highly available cluster is locked against a shutdown it did not mean, so the lock
    // is cleared before the cluster is terminated.
    emr(
        &emulator.router,
        "SetTerminationProtection",
        json!({ "JobFlowIds": [cluster_id.clone()], "TerminationProtected": false }),
    )
    .await;
    emr(
        &emulator.router,
        "TerminateJobFlows",
        json!({ "JobFlowIds": [cluster_id.clone()] }),
    )
    .await;

    // Every instance of the cluster ends with it, and the group Amazon EMR created for it is
    // released — looked for until the deadline rather than waited out.
    let ended = instances_in_state(&emulator, case, &cluster_id, 5, "TERMINATED").await;
    for instance in &ended {
        assert!(
            instance["Status"]["Timeline"]["EndDateTime"].is_number(),
            "case {case:?}: a terminated instance carries its end instant, got {instance}"
        );
        assert_eq!(
            instance["Status"]["StateChangeReason"]["Code"].as_str(),
            Some("CLUSTER_TERMINATED"),
            "case {case:?}: the reason is the cluster's own termination, got {instance}"
        );
    }
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        let answer = emulator
            .ec2
            .describe_placement_groups()
            .group_names(&group_name)
            .send()
            .await;
        let gone = match &answer {
            Ok(output) => !output
                .placement_groups()
                .iter()
                .any(|group| group.group_name() == Some(group_name.as_str())),
            Err(_) => true,
        };
        if gone {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the placement group {group_name} must be released with the cluster"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
