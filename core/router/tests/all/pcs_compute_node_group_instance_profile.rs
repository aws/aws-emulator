// SPDX-License-Identifier: CC0-1.0

//! PCS resolves a compute node group's IAM instance profile against IAM before it resolves
//! the cluster the request names.
//!
//! The rule under test: `CreateComputeNodeGroup$iamInstanceProfileArn` is "the Amazon
//! Resource Name (ARN) of the IAM instance profile used to pass an IAM role when launching
//! EC2 instances" (that member's own documentation), so the create resolves what it
//! REFERENCES in the service that owns instance profiles before it resolves the resource of
//! its own the request names:
//!
//! * a reference IAM holds no instance profile for is HTTP 400 `ValidationException`,
//!   whether or not the cluster exists — so a request naming a profile that does not exist
//!   and a cluster that does not exist is answered about the profile, never 404 about the
//!   cluster. The refusal carries IAM's own answer through, with the tail AWS appends to a
//!   failure of a call a service made on the caller's behalf (the service that answered,
//!   its status, the id of that call and the attempts made), and a value that names no
//!   instance profile of this account at all is refused in PCS's own words, since nobody
//!   was asked about it;
//! * a reference IAM does hold is passed on, and the create then resolves its cluster: a
//!   cluster that does not exist is HTTP 404 `ResourceNotFoundException` reporting the
//!   `clusterIdentifier` supplied and the kind of identifier that did not resolve, and a
//!   cluster that does gets the compute node group, which reads back through
//!   `GetComputeNodeGroup` carrying the profile it was created with.
//!
//! The read and delete paths of a compute node group reference no instance profile and keep
//! resolving their cluster first, which the PCS crate's own suites cover.
//!
//! This suite needs TWO services on one engine — PCS reaches IAM through the in-process
//! callback, so a single-service router has no IAM to ask — which is why it lives here
//! rather than in either service's own crate. PCS also resolves the SUBNETS a create places
//! a resource in against Amazon EC2, which owns them, so the VPC and subnet below are minted
//! by EC2's own operations on the same engine: every subnet id in an accepted request is one
//! EC2 handed out rather than one this suite made up. The rows span how the profile is
//! referenced (a profile IAM holds, the same profile under a path, a profile that was never
//! created, one held by ANOTHER account) against whether the cluster exists, and the two
//! `ClusterIdentifier` forms.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::query_wire::{element, form_encode};

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The CIDR of the VPC the subnet below is carved out of, and the subnet's own.
const VPC_CIDR: &str = "10.44.0.0/16";
const SUBNET_CIDR: &str = "10.44.1.0/24";

/// How long the cluster is given to reach `ACTIVE` before the wait is called a failure.
///
/// PCS reports a new cluster `CREATING` and the clock owes it `ACTIVE` a span later, so what
/// is waited for takes well under a second; this deadline is generous slack over that span so
/// that a loaded host cannot decide the outcome, and all it is for is that a move that never
/// comes fails instead of hanging. Nothing here asserts on elapsed time.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How long to wait before LOOKING AGAIN while that condition has not yet held. Not a bound
/// on anything: the bound is [`ACTIVE_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// An instance profile this account holds, under the default path. PCS documents that the
/// role an instance profile carries "must start with AWSPCS or must have the path
/// /aws-pcs/" (`CreateComputeNodeGroup$iamInstanceProfileArn`), which is the name used
/// here.
const HELD_PROFILE: &str = "AWSPCS-NodeRole";

/// An instance profile this account holds under a PATH, named by its ARN below: the
/// member's `@pattern` admits `instance-profile/<path>/<name>`, and IAM holds the profile
/// under the name alone.
const PATHED_PROFILE: &str = "AWSPCS-PathedRole";

/// The path [`PATHED_PROFILE`] is created under.
const PROFILE_PATH: &str = "/aws-pcs/";

/// An instance profile name this account holds nothing under: IAM answers that it cannot
/// be found, and PCS carries that answer through.
const ABSENT_PROFILE: &str = "AWSPCS-NeverCreated";

/// The cluster this account holds, ACTIVE by the time the rows run.
const HELD_CLUSTER: &str = "pcs-profile-cluster";

/// A cluster identifier of the generated form that names no cluster.
const ABSENT_CLUSTER_ID: &str = "pcs_22222222222222222222";

/// A cluster identifier of the name form that names no cluster.
const ABSENT_CLUSTER_NAME: &str = "no-such-cluster";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// An EC2 trust policy, so the role each profile below carries is one an instance may
/// assume. Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an
/// `awsQuery` form body as written.
fn ec2_trust_policy() -> String {
    "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}"
        .to_string()
}

/// The ARN of an instance profile `account` holds under `path`.
fn instance_profile_arn(account: &str, path: &str, name: &str) -> String {
    format!("arn:aws:iam::{account}:instance-profile{path}{name}")
}

/// Send a request and return the status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    target: Option<&str>,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form body. Every
/// value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    send(
        router,
        runtime,
        "iam",
        None,
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Drive one PCS operation: `POST /` with `X-Amz-Target` and an awsJson1_0 body.
fn pcs(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        runtime,
        "pcs",
        Some(&format!("AWSParallelComputingService.{operation}")),
        "application/x-amz-json-1.0",
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The awsJson1_0 error code, without the namespace the envelope may prefix.
fn error_code(payload: &serde_json::Value) -> String {
    payload
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &serde_json::Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Drive one ec2Query action that must succeed, answered with its XML body.
fn ec2(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (key, value) in parameters {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, text) = send(
        router,
        runtime,
        "ec2",
        None,
        "application/x-www-form-urlencoded",
        body,
    );
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// A `CreateComputeNodeGroup` body carrying every `@required` member, placing the group in
/// `subnet` — an id EC2 minted.
fn create_body(cluster: &str, name: &str, profile_arn: &str, subnet: &str) -> serde_json::Value {
    serde_json::json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupName": name,
        "subnetIds": [subnet],
        "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
        "iamInstanceProfileArn": profile_arn,
        "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
        "instanceConfigs": [{"instanceType": "c6i.large"}],
    })
}

/// What a row expects of its create.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The profile resolves and the cluster exists, so the group is created.
    Created,
    /// IAM answered that it holds no instance profile under that name: 400
    /// `ValidationException` carrying IAM's own sentence about the name it was asked
    /// about, plus the tail of the call behind it.
    ProfileAbsentInIam(&'static str),
    /// The value names no instance profile of this account: 400 `ValidationException` in
    /// PCS's own words, since no call was made about it.
    ProfileNotOfThisAccount,
    /// The profile resolves and the cluster does not exist: 404
    /// `ResourceNotFoundException` reporting the identifier supplied.
    ClusterNotFound(&'static str),
}

/// One row of the table.
struct Case {
    case: &'static str,
    cluster: String,
    group: &'static str,
    profile_arn: String,
    expected: Expected,
}

fn cases(account: &str, cluster_id: &str) -> Vec<Case> {
    let held = instance_profile_arn(account, "/", HELD_PROFILE);
    let pathed = instance_profile_arn(account, PROFILE_PATH, PATHED_PROFILE);
    let absent = instance_profile_arn(account, "/", ABSENT_PROFILE);
    // Another account's instance profile: not one this emulator's IAM can be asked about,
    // so it names no profile of this account at all.
    let foreign = instance_profile_arn("123456789012", "/", HELD_PROFILE);
    vec![
        Case {
            case: "a profile IAM holds, on a cluster that exists",
            cluster: cluster_id.to_string(),
            group: "held-profile",
            profile_arn: held.clone(),
            expected: Expected::Created,
        },
        Case {
            case: "a profile IAM holds under a path, on the same cluster by name",
            cluster: HELD_CLUSTER.to_string(),
            group: "pathed-profile",
            profile_arn: pathed,
            expected: Expected::Created,
        },
        Case {
            // The recorded case: both references are unresolvable, and the profile is
            // resolved first, so the refusal is about the profile.
            case: "a profile that was never created, on a cluster that does not exist",
            cluster: ABSENT_CLUSTER_ID.to_string(),
            group: "absent-both",
            profile_arn: absent.clone(),
            expected: Expected::ProfileAbsentInIam(ABSENT_PROFILE),
        },
        Case {
            case: "a profile that was never created, on a cluster that exists",
            cluster: cluster_id.to_string(),
            group: "absent-profile",
            profile_arn: absent,
            expected: Expected::ProfileAbsentInIam(ABSENT_PROFILE),
        },
        Case {
            case: "another account's instance profile, on a cluster that exists",
            cluster: cluster_id.to_string(),
            group: "foreign-profile",
            profile_arn: foreign,
            expected: Expected::ProfileNotOfThisAccount,
        },
        Case {
            case: "a profile IAM holds, on a cluster id that names no cluster",
            cluster: ABSENT_CLUSTER_ID.to_string(),
            group: "absent-cluster-id",
            profile_arn: held.clone(),
            expected: Expected::ClusterNotFound(ABSENT_CLUSTER_ID),
        },
        Case {
            case: "a profile IAM holds, on a cluster name that names no cluster",
            cluster: ABSENT_CLUSTER_NAME.to_string(),
            group: "absent-cluster-name",
            profile_arn: held,
            expected: Expected::ClusterNotFound(ABSENT_CLUSTER_NAME),
        },
    ]
}

/// Wait until `cluster` reports `ACTIVE`, looking again until it does or
/// [`ACTIVE_DEADLINE`] passes.
///
/// The CALLER waits for the condition, as a caller of the real service would; the service
/// never sleeps for it, and nothing between the reads nudges it.
fn wait_until_active(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let (status, body) = pcs(
            router,
            runtime,
            "GetCluster",
            serde_json::json!({"clusterIdentifier": cluster}),
        );
        assert_eq!(
            status, 200,
            "GetCluster {cluster} must answer 200 while it provisions, got {status} {body}"
        );
        if body["cluster"]["status"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the clock owed {cluster} its ACTIVE status and it never came; last read {body}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Every compute node group `ListComputeNodeGroups` reports for `cluster`, by name.
fn group_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) -> Vec<String> {
    let (status, body) = pcs(
        router,
        runtime,
        "ListComputeNodeGroups",
        serde_json::json!({"clusterIdentifier": cluster, "maxResults": 100}),
    );
    assert_eq!(
        status, 200,
        "ListComputeNodeGroups must answer 200, got {status} {body}"
    );
    body["computeNodeGroups"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_compute_node_group_is_created_only_for_an_instance_profile_iam_holds() {
    std::thread::Builder::new()
        .name("pcs-instance-profile".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let account = engine::emulator_account_id();

            // Setup: two instance profiles, each built the way AWS builds one —
            // CreateRole, CreateInstanceProfile, AddRoleToInstanceProfile.
            let policy = ec2_trust_policy();
            for (profile, path) in [(HELD_PROFILE, "/"), (PATHED_PROFILE, PROFILE_PATH)] {
                for (action, parameters) in [
                    (
                        "CreateRole",
                        vec![
                            ("RoleName", profile),
                            ("AssumeRolePolicyDocument", policy.as_str()),
                        ],
                    ),
                    (
                        "CreateInstanceProfile",
                        vec![("InstanceProfileName", profile), ("Path", path)],
                    ),
                    (
                        "AddRoleToInstanceProfile",
                        vec![("InstanceProfileName", profile), ("RoleName", profile)],
                    ),
                ] {
                    let (status, body) = iam(&router, &runtime, action, &parameters);
                    assert_eq!(
                        status, 200,
                        "setup: IAM {action} for {profile}: got {status} {body}"
                    );
                }
                // Round trip: the profile is readable through IAM's own reader, under the
                // path it was created with.
                let (status, body) = iam(
                    &router,
                    &runtime,
                    "GetInstanceProfile",
                    &[("InstanceProfileName", profile)],
                );
                assert_eq!(
                    status, 200,
                    "GetInstanceProfile {profile} must answer 200, got {status} {body}"
                );
                assert!(
                    body.contains(&format!(
                        "<Arn>{}</Arn>",
                        instance_profile_arn(account, path, profile)
                    )),
                    "GetInstanceProfile {profile} must report the ARN it is held under, got \
                     {body}"
                );
            }

            // Setup: the network the cluster and its groups are placed in, minted by EC2
            // itself — PCS resolves what a create names against the service that owns
            // subnets, so an id this suite made up would name no subnet of the account.
            let vpc = element(
                &ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", VPC_CIDR)]),
                "vpcId",
            );
            let subnet = element(
                &ec2(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[("VpcId", vpc.as_str()), ("CidrBlock", SUBNET_CIDR)],
                ),
                "subnetId",
            );

            // Setup: one cluster, which the clock makes ACTIVE a span after its create.
            let (status, created) = pcs(
                &router,
                &runtime,
                "CreateCluster",
                serde_json::json!({
                    "clusterName": HELD_CLUSTER,
                    "scheduler": {"type": "SLURM", "version": "24.11"},
                    "size": "SMALL",
                    "networking": {"subnetIds": [subnet.clone()]},
                }),
            );
            assert_eq!(status, 200, "setup: CreateCluster: got {status} {created}");
            let cluster_id = created["cluster"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !cluster_id.is_empty(),
                "setup: CreateCluster must mint an id, got {created}"
            );
            // The CALLER waits for the cluster to report ACTIVE, as a caller of the real
            // service would; the service never sleeps for it.
            wait_until_active(&router, &runtime, &cluster_id);

            for case in cases(account, &cluster_id) {
                let (status, body) = pcs(
                    &router,
                    &runtime,
                    "CreateComputeNodeGroup",
                    create_body(&case.cluster, case.group, &case.profile_arn, &subnet),
                );
                match &case.expected {
                    Expected::Created => {
                        assert_eq!(
                            status, 200,
                            "case {:?}: CreateComputeNodeGroup must succeed, got {status} \
                             {body}",
                            case.case
                        );
                        let group_id = body["computeNodeGroup"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        assert!(
                            !group_id.is_empty(),
                            "case {:?}: the create must mint an id, got {body}",
                            case.case
                        );
                        // Round trip: the group reads back through PCS's own reader,
                        // carrying the profile the create referenced.
                        let (status, read) = pcs(
                            &router,
                            &runtime,
                            "GetComputeNodeGroup",
                            serde_json::json!({
                                "clusterIdentifier": cluster_id.clone(),
                                "computeNodeGroupIdentifier": group_id,
                            }),
                        );
                        assert_eq!(
                            status, 200,
                            "case {:?}: GetComputeNodeGroup must answer 200, got {status} \
                             {read}",
                            case.case
                        );
                        assert_eq!(
                            read["computeNodeGroup"]["iamInstanceProfileArn"]
                                .as_str()
                                .unwrap_or_default(),
                            case.profile_arn,
                            "case {:?}: the stored group must report the profile it was \
                             created with, got {read}",
                            case.case
                        );
                        assert!(
                            group_names(&router, &runtime, &cluster_id)
                                .iter()
                                .any(|name| name == case.group),
                            "case {:?}: the group must be listed, got {body}",
                            case.case
                        );
                    }
                    Expected::ProfileAbsentInIam(profile) => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "ValidationException"),
                            "case {:?}: a profile IAM holds nothing for must be refused 400 \
                             ValidationException, got {status} {body}",
                            case.case
                        );
                        let message = error_message(&body);
                        let opening = format!(
                            "Instance Profile {profile} cannot be found. (Service: Iam, \
                             Status Code: 404, Request ID: "
                        );
                        let closing = ") (SDK Attempt Count: 1)";
                        let request_id = message
                            .strip_prefix(opening.as_str())
                            .and_then(|tail| tail.strip_suffix(closing));
                        assert!(
                            request_id.is_some_and(|id| !id.is_empty()),
                            "case {:?}: expected IAM's own answer carried through as \
                             {opening:?}<request id>{closing:?}, got {body}",
                            case.case
                        );
                    }
                    Expected::ProfileNotOfThisAccount => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "ValidationException"),
                            "case {:?}: a value naming no instance profile of this account \
                             must be refused 400 ValidationException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            format!(
                                "The instance profile {} isn't an IAM instance profile of \
                                 this account.",
                                case.profile_arn
                            ),
                            "case {:?}: expected PCS's own sentence naming the value, got \
                             {body}",
                            case.case
                        );
                    }
                    Expected::ClusterNotFound(identifier) => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (404, "ResourceNotFoundException"),
                            "case {:?}: a resolvable profile on an absent cluster must be \
                             refused 404 ResourceNotFoundException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            "AWS PCS couldn't find the cluster you specified.",
                            "case {:?}: expected the service's own sentence, got {body}",
                            case.case
                        );
                        assert_eq!(
                            (
                                body["resourceId"].as_str(),
                                body["resourceType"].as_str()
                            ),
                            (Some(*identifier), Some("Cluster ID")),
                            "case {:?}: expected the identifier and its kind in the \
                             modelled members, got {body}",
                            case.case
                        );
                    }
                }
                if case.expected != Expected::Created {
                    assert!(
                        !group_names(&router, &runtime, &cluster_id)
                            .iter()
                            .any(|name| name == case.group),
                        "case {:?}: a refused create must write no compute node group",
                        case.case
                    );
                }
            }
        })
        .expect("spawn the instance-profile thread")
        .join()
        .expect("every instance profile reference must be resolved against IAM");
}
