// SPDX-License-Identifier: CC0-1.0

//! The EC2 subnets a PCS create places a resource in, and the scheduler version it takes.
//!
//! The rules under test:
//!
//! 1. A subnet is EC2's resource, not PCS's. "The subnets and security groups for the
//!    cluster must already exist before you use this API action" (`CreateCluster`'s own
//!    documentation), so a create that names subnets resolves EVERY id against EC2 before
//!    it mints anything: if any of them resolves to no subnet of the account, the create is
//!    refused with the service's fixed sentence and persists nothing. The same holds for
//!    `CreateComputeNodeGroup`, whose `subnetIds` is "the list of subnet IDs where the
//!    compute node group launches instances".
//! 2. A refusal is grounded in what EC2 ANSWERED, never assumed from silence: an emulator
//!    with no EC2 to ask serves the create.
//! 3. `scheduler.version` is a plain `String` in the model, so a value that LABELS a
//!    scheduler release is carried on with whatever release it names — the networking
//!    checks above are what then decides the request.
//!
//! The rows span what the rules generalise over: how many subnets a request names, whether
//! each one resolves, how it came to exist (created through EC2, a default subnet of the
//! Region nobody created, one that was created and then deleted), and which operation names
//! them. Nothing here reaches into either store: a subnet exists only because EC2 answered
//! for it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::cluster_lifecycle::{call, emulator, wait_past_the_owed_transition};

/// The region every case signs for, matching the scope `crate::cluster_lifecycle` signs
/// with: the Region a request is signed for is the one its subnets are resolved in.
const REGION: &str = "us-east-1";

/// What PCS answers a create whose subnets it could not resolve. The whole sentence is
/// fixed text: it names no subnet id and no count.
const SUBNETS_ABSENT: &str = "AWS PCS couldn't find 1 or more of the subnets you provided.";

/// The CIDR of the VPC these cases carve their subnets out of.
const VPC_CIDR: &str = "10.42.0.0/16";

/// PCS mounted beside the OWNER of the resources it resolves — Amazon EC2, which holds the
/// account's subnets — on ONE shared engine, plus an EC2 client over the router's own
/// in-process callback: the very seam the handler's resolution reaches EC2 through.
fn emulator_with_ec2() -> (MultiServiceRouter, ::aws_sdk_ec2::Client) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
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
    (router, ec2)
}

/// A VPC of `cidr`, as EC2 minted it.
async fn create_vpc(ec2: &::aws_sdk_ec2::Client, cidr: &str) -> String {
    ec2.create_vpc()
        .cidr_block(cidr)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must create the VPC {cidr}: {error:?}"))
        .vpc()
        .and_then(|vpc| vpc.vpc_id().map(str::to_string))
        .unwrap_or_else(|| panic!("setup: EC2 must report a VpcId for {cidr}"))
}

/// A subnet of `cidr` in `vpc`, as EC2 minted it.
async fn create_subnet(ec2: &::aws_sdk_ec2::Client, vpc: &str, cidr: &str) -> String {
    ec2.create_subnet()
        .vpc_id(vpc)
        .cidr_block(cidr)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must create the subnet {cidr}: {error:?}"))
        .subnet()
        .and_then(|subnet| subnet.subnet_id().map(str::to_string))
        .unwrap_or_else(|| panic!("setup: EC2 must report a SubnetId for {cidr}"))
}

/// One default subnet of [`REGION`], which the account holds without anybody creating it.
async fn a_default_subnet(ec2: &::aws_sdk_ec2::Client) -> String {
    ec2.describe_subnets()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name("default-for-az")
                .values("true")
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must report the default subnets: {error:?}"))
        .subnets()
        .iter()
        .find_map(|subnet| subnet.subnet_id().map(str::to_string))
        .unwrap_or_else(|| panic!("setup: expected {REGION} to hold a default subnet"))
}

/// A `CreateCluster` request over `subnets`, on the scheduler `version` given.
fn create_cluster(name: &str, version: &str, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "scheduler": {"type": "SLURM", "version": version},
        "size": "SMALL",
        "networking": {"subnetIds": subnets},
    })
}

/// The names `ListClusters` reports.
async fn cluster_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, listed) = call(router, "ListClusters", &json!({"maxResults": 100})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    listed["clusters"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// An id that is shaped like a subnet id (`SubnetId`'s `@pattern` is
/// `^subnet-\w{8,17}$`, enforced at the boundary) but that the account can hold no subnet
/// under: EC2 issues resource ids whose label is lowercase HEX digits, so a label carrying a
/// letter outside that alphabet names no subnet EC2 could ever have minted. The `index`
/// keeps each row's unresolvable id its own.
fn unresolvable_subnet(index: usize) -> String {
    format!("subnet-zzzzz{index:03}")
}

#[tokio::test]
async fn create_cluster_resolves_every_subnet_it_is_given_against_ec2() {
    let (router, ec2) = emulator_with_ec2();
    let vpc = create_vpc(&ec2, VPC_CIDR).await;
    let held_one = create_subnet(&ec2, &vpc, "10.42.1.0/24").await;
    let held_two = create_subnet(&ec2, &vpc, "10.42.2.0/24").await;
    let by_default = a_default_subnet(&ec2).await;

    // (case, the subnets the create names, whether the placement resolves)
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        ("one subnet EC2 holds", vec![held_one.clone()], true),
        (
            "a default subnet nobody created",
            vec![by_default.clone()],
            true,
        ),
        (
            "two subnets EC2 holds",
            vec![held_one.clone(), held_two.clone()],
            true,
        ),
        (
            "one id EC2 holds no subnet for",
            vec![unresolvable_subnet(0)],
            false,
        ),
        (
            "one held subnet and one unresolvable id",
            vec![held_one.clone(), unresolvable_subnet(1)],
            false,
        ),
        (
            "an unresolvable id ahead of a held subnet",
            vec![unresolvable_subnet(2), held_two.clone()],
            false,
        ),
    ];

    let mut accepted = 0_usize;
    for (index, (case, subnets, resolves)) in rows.iter().enumerate() {
        let name = format!("placement-{index}");
        let (status, body) = call(
            &router,
            "CreateCluster",
            &create_cluster(&name, "24.11", subnets),
        )
        .await;
        if *resolves {
            accepted += 1;
            assert_eq!(status, 200, "case {case}: expected the create to be served, got {body}");
            // The create answers from the stored record, so the placement reads back.
            assert_eq!(
                body["cluster"]["networking"]["subnetIds"],
                json!(subnets),
                "case {case}: expected the subnets to persist, got {body}"
            );
            let (status, read) =
                call(&router, "GetCluster", &json!({"clusterIdentifier": &name})).await;
            assert_eq!(status, 200, "case {case}: GetCluster got {read}");
            assert_eq!(
                read["cluster"]["networking"]["subnetIds"],
                json!(subnets),
                "case {case}: expected the stored placement on the read back, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected the create to be refused, got {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case {case}: got {body}"
            );
            assert_eq!(
                body["message"],
                json!(SUBNETS_ABSENT),
                "case {case}: got {body}"
            );
            for id in subnets {
                assert!(
                    !body["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(id.as_str()),
                    "case {case}: the sentence must name no subnet id, got {body}"
                );
            }
        }
        let held = cluster_names(&router).await;
        assert_eq!(
            held.len(),
            accepted,
            "case {case}: expected only the served creates to have persisted, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_subnet_that_stops_existing_stops_resolving() {
    let (router, ec2) = emulator_with_ec2();
    let vpc = create_vpc(&ec2, VPC_CIDR).await;
    let subnet = create_subnet(&ec2, &vpc, "10.42.3.0/24").await;

    // Nothing is cached: while EC2 holds the subnet the create is served.
    let (status, served) = call(
        &router,
        "CreateCluster",
        &create_cluster("before-delete", "24.11", std::slice::from_ref(&subnet)),
    )
    .await;
    assert_eq!(status, 200, "case before the delete: got {served}");

    ec2.delete_subnet()
        .subnet_id(&subnet)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must delete the subnet: {error:?}"));

    let (status, refused) = call(
        &router,
        "CreateCluster",
        &create_cluster("after-delete", "24.11", std::slice::from_ref(&subnet)),
    )
    .await;
    assert_eq!(status, 400, "case after the delete: got {refused}");
    assert_eq!(
        refused["message"],
        json!(SUBNETS_ABSENT),
        "case after the delete: got {refused}"
    );
    let held = cluster_names(&router).await;
    assert_eq!(
        held,
        vec!["before-delete".to_string()],
        "case after the delete: expected no second cluster, got {held:?}"
    );
}

#[tokio::test]
async fn a_scheduler_release_outside_the_printed_list_reaches_the_networking_checks() {
    let (router, ec2) = emulator_with_ec2();
    let vpc = create_vpc(&ec2, VPC_CIDR).await;
    let held = create_subnet(&ec2, &vpc, "10.42.4.0/24").await;

    // Each release label is carried past the scheduler-version step, whether or not the
    // member's documentation prints it: what decides the request is its networking.
    for (index, version) in ["23.11", "24.11", "26.05", "25.05.1"].iter().enumerate() {
        let (status, refused) = call(
            &router,
            "CreateCluster",
            &create_cluster(
                &format!("version-{index}"),
                version,
                &[unresolvable_subnet(index)],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case version {version}: expected the networking refusal, got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!(SUBNETS_ABSENT),
            "case version {version}: expected the subnet sentence rather than a version \
             refusal, got {refused}"
        );

        // The same version on a subnet EC2 holds is served and persists.
        let name = format!("served-{index}");
        let (status, served) = call(
            &router,
            "CreateCluster",
            &create_cluster(&name, version, std::slice::from_ref(&held)),
        )
        .await;
        assert_eq!(status, 200, "case version {version}: got {served}");
        assert_eq!(
            served["cluster"]["scheduler"]["version"],
            json!(version),
            "case version {version}: expected the stored version, got {served}"
        );
    }
}

#[tokio::test]
async fn compute_node_group_subnets_are_resolved_the_same_way() {
    let (router, ec2) = emulator_with_ec2();
    let vpc = create_vpc(&ec2, VPC_CIDR).await;
    let held = create_subnet(&ec2, &vpc, "10.42.5.0/24").await;

    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("group-host", "24.11", std::slice::from_ref(&held)),
    )
    .await;
    assert_eq!(status, 200, "case create the cluster: got {created}");
    wait_past_the_owed_transition().await;

    let group = |name: &str, subnets: &[String]| {
        json!({
            "clusterIdentifier": "group-host",
            "computeNodeGroupName": name,
            "subnetIds": subnets,
            "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
            "iamInstanceProfileArn": format!(
                "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
            "instanceConfigs": [{"instanceType": "c6i.large"}],
        })
    };

    let (status, refused) = call(
        &router,
        "CreateComputeNodeGroup",
        &group("absent-subnet", &[unresolvable_subnet(7)]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an unresolvable group subnet: got {refused}"
    );
    assert_eq!(
        refused["message"],
        json!(SUBNETS_ABSENT),
        "case an unresolvable group subnet: got {refused}"
    );

    let (status, listed) = call(
        &router,
        "ListComputeNodeGroups",
        &json!({"clusterIdentifier": "group-host"}),
    )
    .await;
    assert_eq!(status, 200, "case the refused group: got {listed}");
    assert_eq!(
        listed["computeNodeGroups"].as_array().map(Vec::len),
        Some(0),
        "case the refused group: expected nothing persisted, got {listed}"
    );

    let (status, served) = call(
        &router,
        "CreateComputeNodeGroup",
        &group("held-subnet", std::slice::from_ref(&held)),
    )
    .await;
    assert_eq!(status, 200, "case a held group subnet: got {served}");
    assert_eq!(
        served["computeNodeGroup"]["subnetIds"],
        json!(vec![held.clone()]),
        "case a held group subnet: expected the subnets to persist, got {served}"
    );
}

#[tokio::test]
async fn an_emulator_with_no_ec2_to_ask_refuses_nothing() {
    // No EC2 is mounted here, so nothing is known about the account's subnets — and a
    // refusal is grounded in what EC2 answered, never assumed from silence.
    let router = emulator();
    let (status, served) = call(
        &router,
        "CreateCluster",
        &create_cluster("no-peer", "24.11", &[unresolvable_subnet(9)]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case no EC2 mounted: expected the create to be served, got {served}"
    );
    assert_eq!(
        served["cluster"]["networking"]["subnetIds"],
        json!(vec![unresolvable_subnet(9)]),
        "case no EC2 mounted: expected the subnets to persist, got {served}"
    );
}
