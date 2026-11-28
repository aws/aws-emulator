// SPDX-License-Identifier: CC0-1.0

//! The references a cluster launch names, resolved AFTER the launch is accepted, end to end
//! over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, and reads the cluster back through `DescribeCluster` and its capacity through
//! `ListInstances`; what Amazon EC2 holds is read in Amazon EC2 itself. Nothing reaches into
//! either service's state struct.
//!
//! THE RULE: `RunJobFlow` answers a `JobFlowId` without resolving the resources the launch
//! names in other services — the Amazon VPC subnet its instances are launched into
//! (`JobFlowInstancesConfig$Ec2SubnetId`), the IAM role Amazon EMR acts under
//! (`RunJobFlow$ServiceRole`) and the permission that role needs for each Amazon EC2 resource
//! Amazon EMR creates for the cluster. Those are resolved once the launch has been accepted,
//! and one that resolves to nothing settles the CLUSTER `TERMINATED_WITH_ERRORS` with
//! `Status.StateChangeReason.Code` `VALIDATION_ERROR` (`ClusterStateChangeReasonCode`) and the
//! descriptive sentence for what could not be resolved, having launched no Amazon EC2 instance
//! and created no placement group. The same launch with its references resolvable brings the
//! cluster up and creates those resources.
//!
//! The rows span what the rule generalises over: WHICH reference is unresolvable (the subnet,
//! the role, the permission behind the role), the FORM the launch names it in (a subnet id of
//! the right shape that no subnet carries, a value that is no subnet id at all, a role named by
//! its friendly name, a role named by its ARN) and — since the grant a launch needs is the grant
//! for what it ASKED Amazon EMR to create — whether the placement group was asked for by a
//! `PlacementGroupConfigs` entry or applied by Amazon EMR itself to a multi-primary topology.
//! Three positive controls launch the same clusters with every reference resolvable.
//!
//! Amazon EC2 and IAM are mounted BESIDE Amazon EMR on one engine, exactly as the assembled
//! router mounts them, and every resource a row names is created through the service that OWNS
//! it: Amazon EC2's `CreateVpc`/`CreateSubnet` and IAM's `CreateRole`/`AttachRolePolicy`/
//! `CreateInstanceProfile`/`AddRoleToInstanceProfile`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every call below is addressed in.
const REGION: &str = "us-west-2";

/// The zone the subnet every resolvable launch is placed in lives in.
const ZONE: &str = "us-west-2a";

/// The release every launch names: one of this service's catalog, later than the release the
/// multi-primary topology came with.
const RELEASE_LABEL: &str = "emr-7.9.0";

/// The instance type every collection below asks for.
const INSTANCE_TYPE: &str = "m5.xlarge";

/// The EC2 instance profile every launch names as its `JobFlowRole`, which `RunJobFlow`
/// resolves against IAM while it answers the call.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";

/// The service role a launch whose references all resolve names.
pub(crate) const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// A service role carrying the EMR grants but NOT the placement-group one.
const SERVICE_ROLE_WITHOUT_PLACEMENT_GROUPS: &str = "EMR_NoPlacementGroupRole";

/// A role name no `CreateRole` of this suite ever writes, so IAM holds nothing under it.
pub(crate) const ABSENT_SERVICE_ROLE: &str = "EMR_NeverCreatedRole";

/// The AWS-managed policies a cluster's service role carries: the EMR service role's own
/// policy, and the separate one that grants the placement-group actions.
const EMR_SERVICE_ROLE_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceRole";
const EMR_PLACEMENT_GROUP_POLICY: &str =
    "arn:aws:iam::aws:policy/AmazonElasticMapReducePlacementGroupPolicy";

/// The states a cluster never leaves (`com.amazonaws.emr#ClusterState`).
pub(crate) const TERMINAL_STATES: [&str; 2] = ["TERMINATED", "TERMINATED_WITH_ERRORS"];

/// The states a cluster that came up reports (`com.amazonaws.emr#ClusterState`).
pub(crate) const READY_STATES: [&str; 2] = ["RUNNING", "WAITING"];

/// The modelled `ClusterStateChangeReasonCode` an unresolvable reference settles the cluster
/// with (`smithy_query(query="shape", shape="ClusterStateChangeReasonCode")`), spelled out
/// here rather than imported from the service crate, so a wrong constant in the implementation
/// cannot make this suite pass.
pub(crate) const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The service-managed tag naming the cluster an Amazon EC2 resource was created for (Amazon
/// EMR Management Guide, "Tagging Amazon EMR clusters").
const CLUSTER_ID_TAG: &str = "aws:elasticmapreduce:job-flow-id";

/// How long a wait for a clock-owed settlement looks for it before giving up. Generous: the
/// settlement is due in a few seconds, so a loaded host delays this suite rather than failing
/// it.
const WAIT_DEADLINE: Duration = Duration::from_secs(120);

/// One look inside that deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Amazon EMR beside the services that own the references a launch names, all on one engine,
/// plus the clients this suite creates and reads those resources through.
///
/// `pub(crate)` with its members and its fixtures below, so the probe that drives this same
/// workflow at a CLIENT's own polling cadence ([`crate::zzz_probe_tmp`]) mounts the three
/// services and builds the same identities through this one harness rather than its own copy.
pub(crate) struct Harness {
    pub(crate) router: MultiServiceRouter,
    pub(crate) ec2: ::aws_sdk_ec2::Client,
    pub(crate) iam: ::aws_sdk_iam::Client,
}

pub(crate) fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
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
    let iam = ::aws_sdk_iam::Client::from_conf(
        ::aws_sdk_iam::Config::builder()
            .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
            .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
            )
            .build(),
    );
    Harness { router, ec2, iam }
}

/// One `awsJson1_1` Amazon EMR call, which must be served.
pub(crate) async fn emr(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticmapreduce.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261114/{REGION}/\
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
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// The subnet of one VPC Amazon EC2 itself minted, so a launch meant to resolve is placed in a
/// subnet the owning service holds.
pub(crate) async fn subnet(harness: &Harness) -> String {
    let vpc = harness
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
    let created = harness
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

/// A trust policy admitting one service principal, the form `CreateRole` takes it in.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// One IAM role carrying `policies`, created the way a client creates one.
async fn role(harness: &Harness, name: &str, principal: &str, policies: &[&str]) {
    harness
        .iam
        .create_role()
        .role_name(name)
        .assume_role_policy_document(trust_policy(principal))
        .send()
        .await
        .unwrap_or_else(|error| panic!("IAM must create {name}: {error:?}"));
    for policy in policies {
        harness
            .iam
            .attach_role_policy()
            .role_name(name)
            .policy_arn(*policy)
            .send()
            .await
            .unwrap_or_else(|error| panic!("IAM must attach {policy} to {name}: {error:?}"));
    }
}

/// The instance profile every launch names, and the roles the service roles of this suite are:
/// each written in IAM itself, so what Amazon EMR resolves is a record a client created.
pub(crate) async fn identities(harness: &Harness) {
    role(
        harness,
        INSTANCE_PROFILE,
        "ec2.amazonaws.com",
        &["arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceforEC2Role"],
    )
    .await;
    harness
        .iam
        .create_instance_profile()
        .instance_profile_name(INSTANCE_PROFILE)
        .send()
        .await
        .expect("IAM must create the instance profile");
    harness
        .iam
        .add_role_to_instance_profile()
        .instance_profile_name(INSTANCE_PROFILE)
        .role_name(INSTANCE_PROFILE)
        .send()
        .await
        .expect("IAM must attach the role to the instance profile");
    role(
        harness,
        SERVICE_ROLE,
        "elasticmapreduce.amazonaws.com",
        &[EMR_SERVICE_ROLE_POLICY, EMR_PLACEMENT_GROUP_POLICY],
    )
    .await;
    role(
        harness,
        SERVICE_ROLE_WITHOUT_PLACEMENT_GROUPS,
        "elasticmapreduce.amazonaws.com",
        &[EMR_SERVICE_ROLE_POLICY],
    )
    .await;
}

/// The uniform single-primary capacity a launch states when the row is not about a topology.
fn single_primary() -> Value {
    json!({
        "InstanceCount": 2,
        "MasterInstanceType": INSTANCE_TYPE,
        "SlaveInstanceType": INSTANCE_TYPE,
        "KeepJobFlowAliveWhenNoSteps": true,
    })
}

/// The multi-primary capacity Amazon EMR spreads over a placement group: the three primary
/// nodes of that topology and the core group they coordinate on.
pub(crate) fn multi_primary() -> Value {
    json!({
        "KeepJobFlowAliveWhenNoSteps": true,
        "InstanceGroups": [
            {
                "Name": "primary",
                "InstanceRole": "MASTER",
                "InstanceType": INSTANCE_TYPE,
                "InstanceCount": 3,
            },
            {
                "Name": "core",
                "InstanceRole": "CORE",
                "InstanceType": INSTANCE_TYPE,
                "InstanceCount": 2,
            },
        ],
    })
}

/// One launch of this suite, and the cluster id Amazon EMR answered it with.
pub(crate) async fn launch(
    harness: &Harness,
    case: &str,
    subnet_id: &str,
    service_role: &str,
    instances: Value,
    placement_group_configs: Option<Value>,
) -> String {
    let mut instances = instances;
    instances["Ec2SubnetId"] = json!(subnet_id);
    let mut request = json!({
        "Name": case,
        "ReleaseLabel": RELEASE_LABEL,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": service_role,
        "Instances": instances,
    });
    if let Some(configs) = placement_group_configs {
        request["PlacementGroupConfigs"] = configs;
    }
    let answered = emr(&harness.router, "RunJobFlow", request).await;
    answered["JobFlowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: RunJobFlow must answer a JobFlowId, got {answered}"))
        .to_string()
}

/// The cluster `cluster_id`, read back through `DescribeCluster`.
pub(crate) async fn describe(harness: &Harness, cluster_id: &str) -> Value {
    emr(
        &harness.router,
        "DescribeCluster",
        json!({ "ClusterId": cluster_id }),
    )
    .await["Cluster"]
        .clone()
}

/// The cluster `cluster_id` once it has reached one of `states`, looked for until the deadline
/// passes rather than waited out.
async fn cluster_in(harness: &Harness, case: &str, cluster_id: &str, states: &[&str]) -> Value {
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        let cluster = describe(harness, cluster_id).await;
        let state = cluster["Status"]["State"].as_str().unwrap_or_default();
        if states.contains(&state) {
            return cluster;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the cluster must reach one of {states:?}, got {cluster}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every instance `ListInstances` reports for `cluster_id`.
pub(crate) async fn instances_of(harness: &Harness, cluster_id: &str) -> Vec<Value> {
    emr(
        &harness.router,
        "ListInstances",
        json!({ "ClusterId": cluster_id }),
    )
    .await["Instances"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The Amazon EC2 placement groups tagged as created for `cluster_id`, read in the service that
/// OWNS placement groups — so a group this service claims to have created is one EC2 holds.
async fn placement_groups_of(harness: &Harness, cluster_id: &str) -> Vec<String> {
    let answer = harness
        .ec2
        .describe_placement_groups()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name(format!("tag:{CLUSTER_ID_TAG}"))
                .values(cluster_id)
                .build(),
        )
        .send()
        .await
        .expect("EC2 must describe the placement groups of this account");
    answer
        .placement_groups()
        .iter()
        .filter_map(|group| group.group_name().map(str::to_string))
        .collect()
}

/// One row of the refusal table: how the launch is broken, and the sentence the cluster's
/// `VALIDATION_ERROR` reason must carry.
struct Unresolvable {
    case: &'static str,
    /// The subnet the launch names, or `None` for the subnet Amazon EC2 really holds.
    subnet_id: Option<&'static str>,
    /// The service role the launch names, or `None` for the one that resolves.
    service_role: Option<String>,
    /// The capacity the launch states.
    multi_primary: bool,
    /// Whether the launch asks for the primary placement group itself.
    configures_placement_group: bool,
    /// `Status.StateChangeReason.Message` the settled cluster must carry.
    message: String,
}

fn unresolvable_rows() -> Vec<Unresolvable> {
    let absent_role_arn = format!(
        "arn:aws:iam::{}:role/{ABSENT_SERVICE_ROLE}",
        engine::emulator_account_id()
    );
    vec![
        Unresolvable {
            case: "a subnet id of the right shape that no subnet of the account carries",
            subnet_id: Some("subnet-0000000000000000f"),
            service_role: None,
            multi_primary: false,
            configures_placement_group: false,
            message: "Invalid Ec2SubnetId: subnet-0000000000000000f.".to_string(),
        },
        Unresolvable {
            case: "a value that is no subnet id at all",
            subnet_id: Some("not-a-subnet"),
            service_role: None,
            multi_primary: false,
            configures_placement_group: false,
            message: "Invalid Ec2SubnetId: not-a-subnet.".to_string(),
        },
        Unresolvable {
            case: "a service role named by a friendly name IAM holds nothing under",
            subnet_id: None,
            service_role: Some(ABSENT_SERVICE_ROLE.to_string()),
            multi_primary: false,
            configures_placement_group: false,
            message: format!("Invalid ServiceRole: {ABSENT_SERVICE_ROLE}."),
        },
        Unresolvable {
            case: "a service role named by an ARN IAM holds nothing under",
            subnet_id: None,
            service_role: Some(absent_role_arn.clone()),
            multi_primary: false,
            configures_placement_group: false,
            message: format!("Invalid ServiceRole: {absent_role_arn}."),
        },
        Unresolvable {
            case: "a service role without the placement-group grant, the group configured",
            subnet_id: None,
            service_role: Some(SERVICE_ROLE_WITHOUT_PLACEMENT_GROUPS.to_string()),
            multi_primary: true,
            configures_placement_group: true,
            message: format!(
                "Service role {SERVICE_ROLE_WITHOUT_PLACEMENT_GROUPS} has insufficient EC2 \
                 permissions"
            ),
        },
    ]
}

/// The `PlacementGroupConfigs` a row that configures the primary placement group itself names:
/// "Amazon EC2 Placement Group strategy associated with instance role"
/// (`PlacementGroupConfig$PlacementStrategy`).
fn spread_primaries() -> Value {
    json!([{ "InstanceRole": "MASTER", "PlacementStrategy": "SPREAD" }])
}

#[tokio::test]
async fn a_launch_naming_an_unresolvable_reference_settles_the_cluster_validation_error() {
    let harness = harness();
    identities(&harness).await;
    let held_subnet = subnet(&harness).await;

    for row in unresolvable_rows() {
        let case = row.case;
        let cluster_id = launch(
            &harness,
            case,
            row.subnet_id.unwrap_or(held_subnet.as_str()),
            row.service_role.as_deref().unwrap_or(SERVICE_ROLE),
            if row.multi_primary {
                multi_primary()
            } else {
                single_primary()
            },
            row.configures_placement_group.then(spread_primaries),
        )
        .await;

        // The call itself was ACCEPTED: the cluster exists and is coming up, which is what
        // makes the settlement below asynchronous rather than a refusal of the request.
        let accepted = describe(&harness, &cluster_id).await;
        assert!(
            !TERMINAL_STATES.contains(&accepted["Status"]["State"].as_str().unwrap_or_default()),
            "case {case:?}: the launch is accepted and the cluster starts, got {accepted}"
        );

        let settled = cluster_in(&harness, case, &cluster_id, &["TERMINATED_WITH_ERRORS"]).await;
        assert_eq!(
            settled["Status"]["StateChangeReason"]["Code"].as_str(),
            Some(VALIDATION_ERROR),
            "case {case:?}: the settled cluster must carry the validation reason, got {settled}"
        );
        assert_eq!(
            settled["Status"]["StateChangeReason"]["Message"].as_str(),
            Some(row.message.as_str()),
            "case {case:?}: expected the sentence {:?}, got {settled}",
            row.message
        );

        // Nothing was provisioned for a cluster whose launch never got past its references.
        let reported = instances_of(&harness, &cluster_id).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: no instance is launched, got {reported:?}"
        );
        let groups = placement_groups_of(&harness, &cluster_id).await;
        assert!(
            groups.is_empty(),
            "case {case:?}: no placement group is created, got {groups:?}"
        );
    }
}

/// One row of the positive-control table: a launch whose every reference resolves, and the
/// Amazon EC2 resources it therefore really creates.
struct Resolvable {
    case: &'static str,
    /// The service role the launch names.
    service_role: &'static str,
    /// The capacity the launch states.
    instances: Value,
    /// The `PlacementGroupConfigs` the launch carries, where it asks for one itself.
    placement_group_configs: Option<Value>,
    /// How many instances the cluster must report once it is up.
    expected_instances: usize,
    /// How many Amazon EC2 placement groups must carry this cluster's id.
    expected_groups: usize,
}

#[tokio::test]
async fn a_launch_whose_references_all_resolve_brings_the_cluster_up() {
    let harness = harness();
    identities(&harness).await;
    let held_subnet = subnet(&harness).await;

    // The positive control of each refusal above: the same request with every reference
    // resolvable, so what the rows above assert is the reference and not the shape. The last row
    // is the OTHER side of the permission rule: the grant a launch needs is the grant for what
    // it ASKED Amazon EMR to create, so a multi-primary cluster whose primaries' placement group
    // Amazon EMR applies on its own comes up under a role that carries no placement-group grant.
    let rows = vec![
        Resolvable {
            case: "a held subnet and a role IAM holds, one primary node",
            service_role: SERVICE_ROLE,
            instances: single_primary(),
            placement_group_configs: None,
            expected_instances: 2,
            expected_groups: 0,
        },
        Resolvable {
            case: "a role carrying the placement-group grant, three primary nodes",
            service_role: SERVICE_ROLE,
            instances: multi_primary(),
            placement_group_configs: Some(spread_primaries()),
            expected_instances: 5,
            expected_groups: 1,
        },
        Resolvable {
            case: "a role without the placement-group grant, the group applied by the topology",
            service_role: SERVICE_ROLE_WITHOUT_PLACEMENT_GROUPS,
            instances: multi_primary(),
            placement_group_configs: None,
            expected_instances: 5,
            expected_groups: 1,
        },
    ];

    for row in rows {
        let case = row.case;
        let cluster_id = launch(
            &harness,
            case,
            held_subnet.as_str(),
            row.service_role,
            row.instances,
            row.placement_group_configs,
        )
        .await;

        let up = cluster_in(&harness, case, &cluster_id, &READY_STATES).await;
        assert!(
            up["Status"]["StateChangeReason"].is_null(),
            "case {case:?}: a cluster that came up carries no state-change reason, got {up}"
        );
        let reported = instances_of(&harness, &cluster_id).await;
        assert_eq!(
            reported.len(),
            row.expected_instances,
            "case {case:?}: expected {} instances, got {reported:?}",
            row.expected_instances
        );
        let groups = placement_groups_of(&harness, &cluster_id).await;
        assert_eq!(
            groups.len(),
            row.expected_groups,
            "case {case:?}: expected {} placement groups, got {groups:?}",
            row.expected_groups
        );
    }
}
