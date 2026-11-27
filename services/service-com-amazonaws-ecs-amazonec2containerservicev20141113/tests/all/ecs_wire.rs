// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One `POST /` with `X-Amz-Target: AmazonEC2ContainerServiceV20141113.<Operation>` and
//! an `awsJson1_1` body, dispatched by the multi-service router. Nothing here reaches
//! into the state struct, so a response that is not computed from persisted state cannot
//! pass.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in these suites signs for. Deliberately NOT the emulator's
/// default, so an ARN that carries it proves the region came from the request's SigV4
/// credential scope.
pub(crate) const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// What Amazon ECS says when the identity document a registration presents and the
/// signature presented with it are not the pair that vouches for one EC2 instance: the PAIR
/// is what is reported, so the sentence names neither of the two values. Held here because
/// more than one suite registers a container instance and asserts this refusal.
pub(crate) const IDENTITY_PAIR_NOT_VALID: &str =
    "The identity document and identity document signature were not valid.";

/// Long enough for every clock-owed transition the start of an `awsvpc` task takes to
/// fall due: Amazon ECS creates and attaches the interface such a task is allocated, one
/// `state::INTERFACE_STEP_SPAN` per documented step of
/// `state::INTERFACE_ATTACH_LIFECYCLE`, and only then is the task `PENDING` with its own
/// start owed one `state::TRANSITION_SPAN` later. With margin for a loaded test machine.
pub(crate) const AWSVPC_START: std::time::Duration = std::time::Duration::from_millis(
    (service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::INTERFACE_ATTACH_LIFECYCLE
        .len() as u64
        - 1)
        * service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::INTERFACE_STEP_SPAN
            .as_millis() as u64
        + service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
            .as_millis() as u64
        + 400,
);

/// The ONE shared wait of these suites: look again at what `read` answers until `settled`
/// holds of it, and answer the value it settled as.
///
/// Every clock-owed transition of this service is one the CLOCK materialises, so a suite
/// waits for the CONDITION rather than for a span: this is that loop, held here so a suite
/// does not carry a deadline and an interval of its own. [`POLL_BUDGET`] is generous — a
/// rollout is several spans, and a poll stops the instant its condition holds — so the
/// deadline only turns a failure into a failure rather than a hang.
pub(crate) async fn poll_until<Read, Reading>(
    case: &str,
    what: &str,
    mut read: Read,
    settled: impl Fn(&Value) -> bool,
) -> Value
where
    Read: FnMut() -> Reading,
    Reading: std::future::Future<Output = Value>,
{
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        let answered = read().await;
        if settled(&answered) {
            return answered;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {what}, got {answered}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// How long [`poll_until`] keeps looking before it gives up.
pub(crate) const POLL_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// How often it looks again.
pub(crate) const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// One service deployment, as `DescribeServiceDeployments` reports it.
pub(crate) async fn describe_service_deployment(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [arn] }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the rollout {arn} back, got {body}"))
}

/// That same rollout once it reports `wanted`, looked for until it does: where a rollout is
/// left is the CLOCK's, so a case about WHERE waits for the status rather than for a span.
pub(crate) async fn poll_deployment_status(
    router: &MultiServiceRouter,
    arn: &str,
    wanted: &str,
    case: &str,
) -> Value {
    poll_until(
        case,
        &format!("the rollout to reach {wanted:?}"),
        || describe_service_deployment(router, arn, case),
        |deployment| deployment["status"] == json!(wanted),
    )
    .await
}

/// A router with this service mounted on a fresh engine beside Amazon EC2 Auto Scaling,
/// and the one Amazon VPC subnet every `awsvpc` placement of these suites names
/// ([`AWSVPC_SUBNET`]) in the account's records.
///
/// Auto Scaling is mounted because a capacity provider binds onto an Auto Scaling group of
/// the caller's and a reference naming a group the account does not hold is refused, so a
/// suite that creates such a provider creates its group first, through that service's OWN
/// API ([`create_auto_scaling_group`]).
pub(crate) fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(engine),
    );
    router
}

/// The awsQuery API version an Amazon EC2 Auto Scaling request body carries.
const AUTO_SCALING_API_VERSION: &str = "2011-01-01";

/// Create one Auto Scaling group named `name` in the account, in [`SIGNED_REGION`], through
/// Amazon EC2 Auto Scaling's OWN API — never by writing into its store.
///
/// `AutoScalingGroupProvider$autoScalingGroupArn` names a group of the caller's, and
/// `CreateCapacityProvider` resolves it against that service's records, so every provider a
/// suite creates over a group needs the group to exist first.
pub(crate) async fn create_auto_scaling_group(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) {
    create_auto_scaling_group_in(router, SIGNED_REGION, name, case).await;
}

/// The same, in `region` — for a case about a group of ANOTHER Region, which is a group of
/// the account that this Region's capacity cannot be bound onto.
///
/// The smallest group that API accepts: a name, a zone of the Region to place instances in,
/// a launch template to launch them from, and a size range that holds none — so nothing is
/// launched and no transition is owed.
pub(crate) async fn create_auto_scaling_group_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    case: &str,
) {
    let zone = format!("{region}a");
    let (status, payload) = auto_scaling_in(
        router,
        region,
        &[
            ("Action", "CreateAutoScalingGroup"),
            ("Version", AUTO_SCALING_API_VERSION),
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", CAPACITY_LAUNCH_TEMPLATE),
            ("MinSize", "0"),
            ("MaxSize", "0"),
            ("AvailabilityZones.member.1", zone.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Auto Scaling CreateAutoScalingGroup to answer 200 for \
         {name} in {region}, got {status} with {payload}"
    );
}

/// One `awsQuery` call to Amazon EC2 Auto Scaling, in `region`, returning the HTTP status and
/// the XML body it answered — the ONE place these suites reach that service's wire, so a suite
/// that creates a group, reads one back or waits for one to move spells no request of its own.
///
/// Every parameter value is percent-encoded here, so a caller passes values as written.
pub(crate) async fn auto_scaling_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let body = params
        .iter()
        .map(|(key, value)| {
            // An `application/x-www-form-urlencoded` value carries only RFC 3986's unreserved
            // set literally; everything else is percent-encoded.
            let value =
                percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC);
            format!("{key}={value}")
        })
        .collect::<Vec<String>>()
        .join("&");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://autoscaling.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{region}\
                 /autoscaling/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The same, in [`SIGNED_REGION`] and asserting the 200 every accepted call answers.
pub(crate) async fn auto_scaling(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
    case: &str,
) -> String {
    let mut body = vec![("Action", action), ("Version", AUTO_SCALING_API_VERSION)];
    body.extend_from_slice(params);
    let (status, payload) = auto_scaling_in(router, SIGNED_REGION, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Auto Scaling {action} to answer 200, got {status} with \
         {payload}"
    );
    payload
}

/// The ARN Amazon EC2 Auto Scaling reports for the group it holds under `name`, read through
/// that service's OWN API.
///
/// `AutoScalingGroupProvider$autoScalingGroupArn` is reported as the ARN that service minted
/// for the group, whichever of the two forms a create named it by, so a suite asserting a
/// provider's binding reads the group's identity off the service that owns the group rather
/// than composing one of its own.
pub(crate) async fn auto_scaling_group_arn(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let payload = auto_scaling(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
        case,
    )
    .await;
    let (_, after) = payload
        .split_once("<AutoScalingGroupARN>")
        .unwrap_or_else(|| panic!("case {case:?}: no AutoScalingGroupARN for {name} in {payload}"));
    let (arn, _) = after
        .split_once("</AutoScalingGroupARN>")
        .unwrap_or_else(|| panic!("case {case:?}: unterminated AutoScalingGroupARN in {payload}"));
    arn.to_string()
}

/// The subnet id every `awsvpc` placement below names
/// ([`awsvpc_network_configuration`]), one of the ids the harness puts in the account's
/// Amazon VPC records ([`seed_awsvpc_subnets`]).
pub(crate) const AWSVPC_SUBNET: &str = "subnet-0123456789abcdef0";

/// The fixed subnet ids the suites of this binary name beside [`AWSVPC_SUBNET`]: a second
/// one for a configuration that names two, and the ones the load-balancer suites place
/// their services in.
const AWSVPC_SUBNETS: [&str; 5] = [
    AWSVPC_SUBNET,
    "subnet-0123456789abcdef1",
    "subnet-0abc1234ecs0lb",
    "subnet-0abc1234ecs0lb0",
    "subnet-0abc1234ecs0lb1",
];

/// How far the `subnet-<17 hexadecimal digits>` family the interface suites draw distinct
/// ids from runs ([`seed_awsvpc_subnets`] holds every id up to it).
const AWSVPC_SUBNET_FAMILY_END: u64 = 0x200;

/// Put the subnets these suites place into in the account's Amazon VPC records, in
/// [`SIGNED_REGION`].
///
/// A placement into the `awsvpc` network mode is resolved against those records — the
/// interface the task is given is created IN the subnet it names, so a placement naming a
/// subnet the account does not hold is refused. The records are written into the Amazon EC2
/// service's own store, where Amazon VPC's records live, because these suites mount only
/// Amazon ECS (and, for some, one other service): there is no Amazon EC2 endpoint in their
/// router to create them through, and the ids have to be the fixed ones the requests and
/// their assertions name, which `CreateSubnet` — which mints its own — cannot give. A suite
/// that drives the Amazon EC2 API itself builds its network through that API instead
/// ([`crate::image_pull_placement`]).
///
/// It also puts the launch template the Auto Scaling groups of these suites launch from
/// ([`seed_capacity_launch_template`]) in those same records, for the same reason: the
/// records live in another service's store that no router here exposes an endpoint for.
pub(crate) fn seed_awsvpc_subnets(engine: &engine::InMemoryEngine) {
    let ec2 = service_com_amazonaws_ec2_amazonec2::state::service_state(engine)
        .expect("the shared engine carries the Amazon EC2 store");
    let mut store = ec2.lock();
    let family = (0..=AWSVPC_SUBNET_FAMILY_END).map(|value| format!("subnet-{value:017x}"));
    for subnet_id in AWSVPC_SUBNETS.iter().map(|id| (*id).to_string()).chain(family) {
        store.subnets.insert(
            subnet_id.clone(),
            service_com_amazonaws_ec2_amazonec2::ops::types::Subnet {
                subnet_id: Some(subnet_id),
                availability_zone: Some(format!("{SIGNED_REGION}a")),
                state: Some("available".to_string()),
                ..Default::default()
            },
        );
    }
    drop(store);
    seed_capacity_launch_template(engine);
}

/// The launch template the Auto Scaling groups these suites create launch from
/// ([`create_auto_scaling_group_in`]).
pub(crate) const CAPACITY_LAUNCH_TEMPLATE: &str = "ecs-capacity";

/// Put that launch template in the account's own Amazon EC2 records, through Amazon EC2's own
/// writer.
///
/// An Auto Scaling group launches from a launch template THE ACCOUNT HOLDS — Amazon EC2 Auto
/// Scaling resolves the specification when the group is created, and refuses a request naming
/// a template Amazon EC2 does not have — so the group these suites create beneath a capacity
/// provider needs its template written first. The template states the image its instances run,
/// because a group is also refused a version no instance could be launched from: "An AMI ID is
/// required to launch an instance"
/// (`smithy_query(query="doc",shape="RunInstancesRequest",member="ImageId")`), and the instance
/// type they run as, because a group that is not configured as a mixed instances group takes
/// that type from the version it launches from.
pub(crate) fn seed_capacity_launch_template(engine: &engine::InMemoryEngine) {
    let ec2 = service_com_amazonaws_ec2_amazonec2::state::service_state(engine)
        .expect("the shared engine carries the Amazon EC2 store");
    let mut store = ec2.lock();
    store.insert_launch_template(
        service_com_amazonaws_ec2_amazonec2::state::LaunchTemplateSpec {
            name: CAPACITY_LAUNCH_TEMPLATE.to_string(),
            created_by: engine::emulator_arn("iam", "", "root"),
            create_time: std::time::SystemTime::now(),
            tags: Vec::new(),
            operator: None,
            version_description: None,
            data:
                service_com_amazonaws_ec2_amazonec2::ops::types::RequestLaunchTemplateData {
                    image_id: Some(CAPACITY_IMAGE_ID.to_string()),
                    instance_type: Some(CAPACITY_INSTANCE_TYPE.to_string()),
                    ..Default::default()
                },
        },
    );
}

/// The image the instances of those groups run, in the printed form Amazon EC2 issues an AMI id
/// in: the groups these suites create hold no instance of their own, so what matters is that the
/// template states an image at all.
const CAPACITY_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type those instances run as: the groups these suites create hold no instance
/// of their own, so what matters is that the template states a type at all.
const CAPACITY_INSTANCE_TYPE: &str = "t3.micro";

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One call, returning the HTTP status and the parsed body — for a suite whose table
/// spans requests that are ANSWERED and requests that are REFUSED, where which of the
/// two a row earns is itself what the row is about.
pub(crate) async fn answered(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    call(router, operation, body).await
}

/// A call that must be refused with HTTP 400 and the named modelled error code,
/// returning its error body so a caller can assert the message too.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected a {expected_code} from {operation}, got {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400, returning the modelled error code (the
/// short shape name the `__type` carries) and the `message` the refusal reports — the
/// two things a client keys on.
pub(crate) async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let code = parsed["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string();
    (code, message)
}

/// A required string member of a response body.
pub(crate) fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// An integer member whose modelled default is `0`, so an absent member reads as zero.
pub(crate) fn count(body: &Value, member: &str, case: &str) -> i64 {
    match &body[member] {
        Value::Null => 0,
        value => value
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}")),
    }
}

/// A list member of a response body, absent reading as empty.
pub(crate) fn list(body: &Value, member: &str, case: &str) -> Vec<Value> {
    match &body[member] {
        Value::Null => Vec::new(),
        Value::Array(items) => items.clone(),
        other => panic!("case {case:?}: expected a list {member}, got {other}"),
    }
}

/// Assert that `document` carries NO value for `member` — absent, or present as `null`.
///
/// Read as "absence", not as "no value of the type I expected": a member present with a value
/// of another JSON type would satisfy a typed read of `None`, so a wrongly typed `stopCode` or
/// `startedAt` would slip through such a check. The ONE reader of absence for this binary's
/// suites, so a suite asserting that a member was not reported shares this check rather than
/// keeping a copy of it.
pub(crate) fn assert_absent(document: &Value, member: &str, case: &str) {
    assert!(
        document.get(member).is_none_or(Value::is_null),
        "case {case:?}: expected no {member} at all, got {document}"
    );
}

/// One container of a described task, by name — the read every suite that asserts on a named
/// container of a task does, shared for the same reason.
pub(crate) fn container_of(task: &Value, name: &str, case: &str) -> Value {
    list(task, "containers", case)
        .into_iter()
        .find(|container| container["name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: expected the container {name}, got {task}"))
}

/// Assert `reported` is an epoch-seconds instant encoded no finer than the MILLISECOND: the
/// milliseconds it names are whole, so nothing below a millisecond reaches the wire.
///
/// The ONE reader of that resolution for this binary's suites, beside the wire they read it
/// off: the rule holds of every instant this API reports, so a suite asserting it for the
/// members of its own records shares this check rather than keeping a copy that could come
/// to disagree with it.
pub(crate) fn assert_millisecond_resolution(case: &str, member: &str, reported: &Value) {
    let Some(seconds) = reported.as_f64() else {
        panic!("case {case:?}: expected an epoch instant for {member}, got {reported}");
    };
    let millis = seconds * 1_000.0;
    assert!(
        (millis - millis.round()).abs() < 1e-3,
        "case {case:?}: expected {member} at no finer than millisecond resolution, got \
         {reported}"
    );
}

/// An EC2 instance identity document, as the agent reads it from
/// `http://169.254.169.254/latest/dynamic/instance-identity/document/`. It carries the
/// `accountId` and the `instanceId` a registration must supply.
pub(crate) fn identity_document(instance_id: &str) -> String {
    json!({
        "accountId": ACCOUNT,
        "instanceId": instance_id,
        "region": SIGNED_REGION,
        "instanceType": "m5.large",
        "availabilityZone": format!("{SIGNED_REGION}a"),
    })
    .to_string()
}

/// The signature of an EC2 instance identity document, as the agent reads it from
/// `http://169.254.169.254/latest/dynamic/instance-identity/signature/`: base64 text,
/// which a registration presents BESIDE the document — the document alone identifies no
/// machine. 350 characters, because the signature of a document under a 2048-bit key is
/// 256 bytes and base64 spells those in 344. Nothing here signs anything: what a
/// signature is OF cannot be checked without the key that produced it.
pub(crate) fn identity_document_signature() -> String {
    "ZXhhbXBsZQ".repeat(35)
}

/// Register one container instance into `cluster` through the agent's own operation and
/// return its ARN.
pub(crate) async fn register_instance(
    router: &MultiServiceRouter,
    cluster: &str,
    instance_id: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterContainerInstance",
        json!({
            "cluster": cluster,
            "instanceIdentityDocument": identity_document(instance_id),
            "instanceIdentityDocumentSignature": identity_document_signature(),
        }),
        case,
    )
    .await;
    string(&body["containerInstance"], "containerInstanceArn", case)
}

/// The principal every caller of this emulator authenticates as, and so the one a "who
/// created this" member names: the identity `sts:GetCallerIdentity` publishes
/// (`arn:aws:iam::<account>:user/emulator`).
pub(crate) fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// An integer member that must REACH the wire, as `Some(value)`, and `None` for a member
/// the response left out — so an assertion on a member whose expected value is its
/// modelled default cannot pass against a projection that never wrote it.
pub(crate) fn present_count(body: &Value, member: &str, case: &str) -> Option<i64> {
    match &body[member] {
        Value::Null => None,
        value => Some(
            value
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}")),
        ),
    }
}

/// The `networkConfiguration` every launch of an `awsvpc` task definition has to carry:
/// an `awsvpc` task gets its own elastic network interface, so `CreateService`,
/// `UpdateService` and `RunTask` must say which subnets and security groups it takes
/// (`AwsVpcConfiguration$subnets`/`$securityGroups`). The subnet is [`AWSVPC_SUBNET`], which
/// the harness holds in the account's Amazon VPC records: a placement is resolved against
/// them, so a subnet nothing holds is refused.
pub(crate) fn awsvpc_network_configuration() -> Value {
    json!({
        "awsvpcConfiguration": {
            "subnets": [AWSVPC_SUBNET],
            "securityGroups": ["sg-0123456789abcdef0"],
        }
    })
}

/// Write every member of `extra` onto `target`, which is the object a row adds them to —
/// so a table's row states only the members it changes. Shared by every suite whose rows
/// are built that way.
pub(crate) fn with_members(target: &mut Value, extra: Value) {
    if let Value::Object(members) = extra {
        for (member, value) in members {
            target[member] = value;
        }
    }
}

/// Create a cluster and return its ARN.
pub(crate) async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let body = ok(router, "CreateCluster", json!({ "clusterName": name }), case).await;
    string(&body["cluster"], "clusterArn", case)
}

/// One cluster as `DescribeClusters` reports it, with the attachments a capacity-provider
/// association leaves on it (`DescribeClusters$include`: the `ATTACHMENTS` field).
pub(crate) async fn describe_cluster_attachments(
    router: &MultiServiceRouter,
    cluster: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": ["ATTACHMENTS"] }),
        case,
    )
    .await;
    list(&body, "clusters", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the cluster {cluster} back, got {body}"))
}

/// Associate exactly `providers` with `cluster`, and wait for the attachments update the
/// association owes to land. A cluster whose attachments are mid-update refuses the next
/// cluster mutation, so a suite that associates and then associates again waits for the
/// CONDITION rather than for a span ([`poll_until`]).
pub(crate) async fn associate_capacity_providers(
    router: &MultiServiceRouter,
    cluster: &str,
    providers: &[&str],
    case: &str,
) {
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": providers,
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
    poll_until(
        case,
        "the capacity-provider attachments update to land",
        || describe_cluster_attachments(router, cluster, case),
        |reported| reported["attachmentsStatus"] == json!("UPDATE_COMPLETE"),
    )
    .await;
}

/// The EC2 instance id the shared capacity helper registers. One per cluster is enough:
/// a container instance is registered INTO a cluster, so the same instance id in another
/// cluster is another registration.
const CAPACITY_INSTANCE_ID: &str = "i-0c0ffee0c0ffee0c0";

/// Create a cluster and register ONE container instance into it, so a task placed with
/// a launch type whose capacity is registered to the cluster has somewhere to run —
/// `RunTask` refuses a cluster with none ("The EC2 launch type runs your tasks on Amazon
/// EC2 instances registered to your cluster"). Returns the cluster's ARN.
pub(crate) async fn cluster_with_capacity(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let arn = create_cluster(router, name, case).await;
    register_capacity(router, name, case).await;
    arn
}

/// Register ONE container instance into an EXISTING cluster, for a suite that created
/// the cluster itself. Returns the container instance's ARN.
pub(crate) async fn register_capacity(
    router: &MultiServiceRouter,
    cluster: &str,
    case: &str,
) -> String {
    register_instance(router, cluster, CAPACITY_INSTANCE_ID, case).await
}

/// Register one revision of `family` with a single container and return its ARN. The
/// suites that need a plain revision share this one; a suite that needs a revision of a
/// particular SHAPE (Fargate-compatible, a named container) builds its own.
pub(crate) async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// A router carrying this service AND the Cloud Map (service discovery) service over ONE
/// shared engine — the two stores every cross-service registry rule reads across, mounted
/// the way the production router mounts them. Holds [`AWSVPC_SUBNET`] too, so a placement
/// of these suites resolves the subnet it names, and Amazon EC2 Auto Scaling, so a capacity
/// provider these suites create has a group to bind onto
/// ([`create_auto_scaling_group`]).
pub(crate) fn emulator_with_cloud_map() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_servicediscovery_route53autonaming_v20170314::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One `awsJson1_1` call to the Cloud Map service, returning its parsed body. A Cloud Map
/// resource an ECS rule reads is created through CLOUD MAP's own API, never written into a
/// store by hand.
pub(crate) async fn cloud_map(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("Route53AutoNaming_v20170314.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /servicediscovery/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected Cloud Map {operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// The instances Cloud Map's own `DiscoverInstances` answers for one of its services, named
/// by the namespace and the service name it was created under — the read a client
/// discovering a service makes, so an instance that is not in Cloud Map's own store cannot
/// pass.
pub(crate) async fn cloud_map_discovered(
    router: &MultiServiceRouter,
    namespace_name: &str,
    service_name: &str,
    case: &str,
) -> Vec<Value> {
    let answer = cloud_map(
        router,
        "DiscoverInstances",
        json!({ "NamespaceName": namespace_name, "ServiceName": service_name }),
        case,
    )
    .await;
    list(&answer, "Instances", case)
}

/// `GetService`'s own `InstanceCount` for one Cloud Map service, read by the id that names
/// it — the last segment of its ARN when an ARN is what the caller holds.
pub(crate) async fn cloud_map_instance_count(
    router: &MultiServiceRouter,
    service_id_or_arn: &str,
    case: &str,
) -> i64 {
    let id = service_id_or_arn
        .rsplit('/')
        .next()
        .unwrap_or(service_id_or_arn);
    let read = cloud_map(router, "GetService", json!({ "Id": id }), case).await;
    count(&read["Service"], "InstanceCount", case)
}

/// A router carrying this service AND CloudWatch over ONE shared engine — the store the
/// `AWS/ECS` series are published into and the one the alarms a rollout is watched by live
/// in, mounted the way the production router mounts them.
///
/// The ONE such harness of these suites, so a suite that reads a published series and one
/// that moves an alarm reach CloudWatch the same way ([`cloudwatch`]).
pub(crate) fn emulator_with_cloudwatch() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One CloudWatch Logs call, the way a client makes it: `POST /` with
/// `X-Amz-Target: Logs_20140328.<operation>` and a credential scoped to `logs`, which is what
/// the router picks the service from. Answered with the HTTP status and the parsed body, because
/// a suite that reads a stream nothing has been delivered to yet is answered a refusal it treats
/// as "nothing yet" rather than as a failure.
///
/// The ONE such caller these suites share: a container's output travels into CloudWatch Logs'
/// own store, and every suite that reads it back reads it the same way.
pub(crate) async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /logs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One `x-amz-json-1.0` call to the CloudWatch service, the way a client makes it, returning
/// its parsed body. A metric or an alarm these suites read or move is read and moved through
/// CloudWatch's OWN API, never written into a store by hand.
pub(crate) async fn cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("GraniteServiceVersion20100801.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /monitoring/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "expected CloudWatch {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A router carrying this service AND the Amazon EC2 service over ONE shared engine — the
/// two stores the image-fetch rule reads across, mounted the way the production router
/// mounts them. Amazon EC2 Auto Scaling is mounted beside them, so a capacity provider
/// these suites create has a group to bind onto ([`create_auto_scaling_group`]), and Amazon
/// ECR, which SERVES the pull of an image reference naming this account's registry: the
/// repository and the image such a reference resolves to are created through that service's
/// own API ([`ecr`]).
pub(crate) fn emulator_with_ec2() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call to the Amazon ECR service, returning its parsed body. An image an
/// ECS pull resolves is pushed through ECR's OWN API, never written into a store by hand.
pub(crate) async fn ecr(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /ecr/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon ECR {operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// A router carrying this service AND the two stores the image PULL of a launched task is
/// decided across — Amazon ECR, which serves the pull, and IAM, which holds the policies in
/// force on the task's execution role — over ONE shared engine, mounted the way the
/// production router mounts them.
pub(crate) fn emulator_with_ecr_and_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// The IAM API version an awsQuery body carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// One awsQuery call to IAM that must succeed, returning its XML body. A role and the
/// policies in force on it are created through IAM's OWN API, never written into its store.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
    case: &str,
) -> String {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(
            &percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
                .to_string(),
        );
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /iam/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected IAM {action} to answer 200, got {status} with {text}"
    );
    text
}

/// The Amazon EC2 API version an `ec2Query` body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// One `ec2Query` call to the Amazon EC2 service, returning the XML body it answered.
///
/// EC2 speaks `ec2Query`: an `application/x-www-form-urlencoded` POST whose body carries
/// `Action=<Operation>`, answered with XML. Every network resource an ECS rule reads is
/// created through EC2's OWN API, never written into a store by hand.
pub(crate) async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
    case: &str,
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{SIGNED_REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /ec2/aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected EC2 {action} to answer 200, got {status} with {payload}"
    );
    payload
}

/// The same call with every parameter VALUE percent-encoded, for a request carrying a value
/// that is not made of unreserved characters — an instance's user data, which is base64 and
/// so may carry `+`, `/` and `=`.
///
/// [`ec2`] passes its values through as written, which is what lets a caller spell an
/// already-encoded one; a value this one is given is encoded here, so a caller must NOT
/// pre-encode it.
pub(crate) async fn ec2_form(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
    case: &str,
) -> String {
    let encoded: Vec<(&str, String)> = params
        .iter()
        .map(|(name, value)| {
            (
                *name,
                percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = encoded
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    ec2(router, action, &borrowed, case).await
}

/// The text of the FIRST `<name>` element of an `ec2Query` response — the minted id a
/// create answers with.
pub(crate) fn ec2_element(xml: &str, name: &str, case: &str) -> String {
    xml_element(xml, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {xml}"))
}

/// The text of the FIRST `<name>` element of an XML body — an `ec2Query` or `awsQuery`
/// response — and `None` for a body carrying none, such as the console of a guest that has
/// written nothing yet. [`ec2_element`] is the form for an element that must be there.
pub(crate) fn xml_element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}
