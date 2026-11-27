// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the NETWORK a Studio domain is placed in: how `CreateDomain`
//! resolves the VPC, the subnets and the security groups a request names against the service
//! that owns them, and what the domain settles in when they do not agree.
//!
//! SageMaker is mounted BESIDE Amazon EC2 on one shared engine, the way the assembled
//! emulator mounts them, because a VPC, a subnet and a security group are EC2's resources:
//! every identifier a case below names is one EC2 minted through its own API, reached over
//! the same in-process callback the handler resolves a placement through.
//!
//! The suite asserts a RULE over a TABLE of placements rather than one recorded call: a
//! domain is provisioned only when every part of its placement that EC2 states a VPC for
//! belongs to the ONE VPC the domain uses for communication, the VPC `VpcId` declares. The
//! rows differ along which part disagrees (a subnet, a group named in each of the three
//! settings members, two subnets of different VPCs), how many parts the request names, and
//! whether EC2 holds any record for them. A request that names no VPC or no subnets at all
//! resolves no network for the domain and is refused before any domain exists, which is
//! `studio_domain_resolved_settings`' own table.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::inference_deployment_lifecycle::str_field;

/// The Region every case signs for; the network is resolved in the Region the caller
/// addressed, so the setup mints its VPCs there too.
const REGION: &str = "us-east-1";

/// An execution role inside `RoleArn`'s modelled pattern, for the `DefaultUserSettings`
/// every create must carry.
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/service-role/StudioRole";

/// The statuses of `DomainStatus` this suite expects a create to settle in
/// (`smithy_query(query="shape",shape="com.amazonaws.sagemaker#DomainStatus")` lists
/// `Deleting | Failed | InService | Pending | Updating | Update_Failed | Delete_Failed`).
const PENDING: &str = "Pending";
const IN_SERVICE: &str = "InService";
const FAILED: &str = "Failed";

/// How long a poll gives the clock to settle a domain: far longer than the span the state
/// gives a domain's provisioning, so a loaded host still settles well inside it. Only the
/// loop's CONDITION decides when it stops.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often a poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// SageMaker and Amazon EC2 on ONE shared engine, plus an EC2 client over the router's own
/// in-process callback — the very seam `CreateDomain` resolves a placement through.
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

    /// One VPC of the account with `subnets` subnets and `groups` security groups in it, all
    /// minted by EC2's own operations, so every identifier below is one EC2 handed out.
    ///
    /// `label` keeps one network's CIDR block and group names apart from another's, because
    /// two security groups of one VPC cannot share a name.
    async fn network(&self, label: u8, subnets: usize, groups: usize) -> Network {
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
        let mut subnet_ids = Vec::new();
        for index in 0..subnets {
            subnet_ids.push(
                self.ec2
                    .create_subnet()
                    .vpc_id(&vpc_id)
                    .cidr_block(format!("10.{label}.{}.0/24", index + 1))
                    .availability_zone(format!("{REGION}a"))
                    .send()
                    .await
                    .expect("setup: EC2 must create the subnet")
                    .subnet()
                    .and_then(|subnet| subnet.subnet_id())
                    .expect("setup: EC2 must report the subnet's id")
                    .to_string(),
            );
        }
        let mut group_ids = Vec::new();
        for index in 0..groups {
            group_ids.push(
                self.ec2
                    .create_security_group()
                    .vpc_id(&vpc_id)
                    .group_name(format!("studio-{label}-{index}"))
                    .description("a group a Studio domain is placed in")
                    .send()
                    .await
                    .expect("setup: EC2 must create the security group")
                    .group_id()
                    .expect("setup: EC2 must report the group's id")
                    .to_string(),
            );
        }
        Network {
            vpc_id,
            subnet_ids,
            group_ids,
        }
    }

    /// Poll `DescribeDomain` until the domain leaves [`PENDING`], and answer the terminal
    /// status with the `FailureReason` reported beside it.
    ///
    /// Waits for the CONDITION, never for a fixed time: the deadline is generous and only
    /// the status the reader reports ends the loop.
    async fn settled(&self, case: &str, domain_id: &str) -> (String, Option<String>) {
        let request = json!({ "DomainId": domain_id });
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = self.ok(case, "DescribeDomain", &request).await;
            let status = str_field(&described, "Status");
            if status != PENDING {
                let reason = described
                    .get("FailureReason")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                return (status, reason);
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the domain must leave {PENDING} inside {SETTLE_DEADLINE:?}, \
                 last read {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The status `ListDomains` reports for `domain_id`, so the listing is held to the same
    /// terminal status the reader reports.
    async fn listed_status(&self, case: &str, domain_id: &str) -> String {
        let listed = self
            .ok(case, "ListDomains", &json!({ "MaxResults": 50 }))
            .await;
        let domains = listed
            .get("Domains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a Domains list in {listed}"))
            .clone();
        let found = domains
            .iter()
            .find(|details| str_field(details, "DomainId") == domain_id)
            .unwrap_or_else(|| panic!("case {case:?}: {domain_id} must be listed in {listed}"));
        str_field(found, "Status")
    }
}

/// One VPC of the account as EC2 minted it.
struct Network {
    vpc_id: String,
    subnet_ids: Vec<String>,
    group_ids: Vec<String>,
}

/// A `CreateDomain` request naming `name`, the placement members given, and nothing else.
fn create_domain(
    name: &str,
    vpc_id: Option<&str>,
    subnet_ids: &[String],
    user_groups: &[String],
    domain_groups: &[String],
    space_groups: &[String],
) -> Value {
    let mut user_settings = json!({ "ExecutionRole": EXECUTION_ROLE });
    if !user_groups.is_empty() {
        user_settings["SecurityGroups"] = json!(user_groups);
    }
    let mut request = json!({
        "DomainName": name,
        "AuthMode": "IAM",
        "DefaultUserSettings": user_settings,
    });
    if let Some(vpc_id) = vpc_id {
        request["VpcId"] = json!(vpc_id);
    }
    if !subnet_ids.is_empty() {
        request["SubnetIds"] = json!(subnet_ids);
    }
    if !domain_groups.is_empty() {
        request["DomainSettings"] = json!({ "SecurityGroupIds": domain_groups });
    }
    if !space_groups.is_empty() {
        request["DefaultSpaceSettings"] = json!({
            "ExecutionRole": EXECUTION_ROLE,
            "SecurityGroups": space_groups,
        });
    }
    request
}

/// The `FailureReason` a domain carries for a subnet of another VPC than its own. A literal
/// on purpose: this suite is what pins the wording, so it does not ask the implementation
/// what it is. Both identifiers come from the request.
fn subnet_reason(subnet_id: &str, vpc_id: &str) -> String {
    format!("The subnet '{subnet_id}' does not exist in the VPC '{vpc_id}'.")
}

/// The same for a security group of another VPC than the domain's.
fn group_reason(group_id: &str, vpc_id: &str) -> String {
    format!("The security group '{group_id}' does not belong to the VPC '{vpc_id}'.")
}

/// One row of the placement table: the request to create a domain with, and what the clock
/// must settle it in.
struct Row {
    case: &'static str,
    request: Value,
    /// The terminal `Status` the domain must reach.
    status: &'static str,
    /// The `FailureReason` reported beside it, or `None` where the domain came up.
    reason: Option<String>,
}

/// RULE: every part of a domain's placement that EC2 states a VPC for must belong to the one
/// VPC the domain uses for communication, the VPC `VpcId` declares; a domain whose placement
/// disagrees is still CREATED and settles in `Failed` with the `FailureReason` naming the
/// disagreeing part, while a self-consistent one reaches `InService` with no reason at all.
#[tokio::test]
async fn a_domain_settles_by_whether_its_placement_names_one_vpc() {
    let wired = Wired::new();
    // Two networks of the account, each minted through EC2's own API: the domain's own, and
    // another whose subnets and groups a mismatched placement reaches into.
    let own = wired.network(10, 3, 2).await;
    let other = wired.network(20, 1, 1).await;
    let no_ids: Vec<String> = Vec::new();
    // Identifiers inside the modelled patterns that EC2 holds NO record for, so it states no
    // membership for them at all.
    let unheld_vpc = "vpc-0f1e2d3c4b5a69780".to_string();
    let unheld_subnet = "subnet-0f1e2d3c4b5a69781".to_string();

    let rows = vec![
        Row {
            case: "one subnet and one group of the declared VPC",
            request: create_domain(
                "one-of-each",
                Some(&own.vpc_id),
                &own.subnet_ids[..1],
                &own.group_ids[..1],
                &no_ids,
                &no_ids,
            ),
            status: IN_SERVICE,
            reason: None,
        },
        Row {
            case: "every subnet and group of the declared VPC, across all three members",
            request: create_domain(
                "all-of-the-vpc",
                Some(&own.vpc_id),
                &own.subnet_ids,
                &own.group_ids[..1],
                &own.group_ids[1..],
                &own.group_ids,
            ),
            status: IN_SERVICE,
            reason: None,
        },
        Row {
            case: "two subnets of the declared VPC",
            request: create_domain(
                "two-subnets",
                Some(&own.vpc_id),
                &own.subnet_ids[..2],
                &own.group_ids[..1],
                &no_ids,
                &no_ids,
            ),
            status: IN_SERVICE,
            reason: None,
        },
        Row {
            case: "identifiers EC2 holds no record for",
            request: create_domain(
                "unheld-network",
                Some(&unheld_vpc),
                std::slice::from_ref(&unheld_subnet),
                &no_ids,
                &no_ids,
                &no_ids,
            ),
            status: IN_SERVICE,
            reason: None,
        },
        Row {
            case: "a subnet of another VPC than the declared one",
            request: create_domain(
                "cross-vpc-subnet",
                Some(&own.vpc_id),
                &other.subnet_ids,
                &own.group_ids[..1],
                &no_ids,
                &no_ids,
            ),
            status: FAILED,
            reason: Some(subnet_reason(&other.subnet_ids[0], &own.vpc_id)),
        },
        Row {
            case: "a second subnet of another VPC, named after a resolving one",
            request: create_domain(
                "cross-vpc-second-subnet",
                Some(&own.vpc_id),
                &[own.subnet_ids[0].clone(), other.subnet_ids[0].clone()],
                &no_ids,
                &no_ids,
                &no_ids,
            ),
            status: FAILED,
            reason: Some(subnet_reason(&other.subnet_ids[0], &own.vpc_id)),
        },
        Row {
            case: "a declared VPC the account holds, named with another VPC's subnet only",
            request: create_domain(
                "declared-vpc-other-subnet",
                Some(&other.vpc_id),
                &own.subnet_ids[..1],
                &no_ids,
                &no_ids,
                &no_ids,
            ),
            status: FAILED,
            reason: Some(subnet_reason(&own.subnet_ids[0], &other.vpc_id)),
        },
        Row {
            case: "a default user settings group of another VPC",
            request: create_domain(
                "cross-vpc-user-group",
                Some(&own.vpc_id),
                &own.subnet_ids[..1],
                &other.group_ids,
                &no_ids,
                &no_ids,
            ),
            status: FAILED,
            reason: Some(group_reason(&other.group_ids[0], &own.vpc_id)),
        },
        Row {
            case: "a domain settings group of another VPC",
            request: create_domain(
                "cross-vpc-domain-group",
                Some(&own.vpc_id),
                &own.subnet_ids[..1],
                &own.group_ids[..1],
                &other.group_ids,
                &no_ids,
            ),
            status: FAILED,
            reason: Some(group_reason(&other.group_ids[0], &own.vpc_id)),
        },
        Row {
            case: "a default space settings group of another VPC",
            request: create_domain(
                "cross-vpc-space-group",
                Some(&own.vpc_id),
                &own.subnet_ids[..1],
                &no_ids,
                &no_ids,
                &other.group_ids,
            ),
            status: FAILED,
            reason: Some(group_reason(&other.group_ids[0], &own.vpc_id)),
        },
    ];

    // Every create is served first, so the whole table waits out ONE provisioning span rather
    // than one each; the create resolved what its placement comes to before it answered, so
    // the read that immediately follows it still reports the transient status and the poll
    // below weighs the terminal one the clock owed.
    let mut created: Vec<(&Row, String)> = Vec::with_capacity(rows.len());
    for row in &rows {
        let answered = wired.ok(row.case, "CreateDomain", &row.request).await;
        let domain_id = str_field(&answered, "DomainId");
        let described = wired
            .ok(row.case, "DescribeDomain", &json!({ "DomainId": domain_id }))
            .await;
        let status = str_field(&described, "Status");
        assert!(
            status == PENDING,
            "case {:?}: the read that follows the create reports the domain still \
             provisioning ({PENDING:?}), got {status:?}",
            row.case
        );
        created.push((row, domain_id));
    }

    for (row, domain_id) in created {
        let (status, reason) = wired.settled(row.case, &domain_id).await;
        assert!(
            status == row.status,
            "case {:?}: expected the domain to settle in {:?}, got {status:?} (reason \
             {reason:?})",
            row.case,
            row.status
        );
        assert!(
            reason == row.reason,
            "case {:?}: expected FailureReason {:?}, got {reason:?}",
            row.case,
            row.reason
        );
        // The listing reports what the reader does, so a client that branches on the
        // terminal status reads the same fact either way.
        let listed = wired.listed_status(row.case, &domain_id).await;
        assert!(
            listed == row.status,
            "case {:?}: ListDomains must report {:?}, got {listed:?}",
            row.case,
            row.status
        );
        // A settled status is terminal: a later read neither moves it nor drops its reason.
        let again = wired
            .ok(
                row.case,
                "DescribeDomain",
                &json!({ "DomainId": domain_id }),
            )
            .await;
        let again_status = str_field(&again, "Status");
        let again_reason = again
            .get("FailureReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert!(
            again_status == status && again_reason == reason,
            "case {:?}: a read must not change what a later caller sees, got \
             {again_status:?} with {again_reason:?} after {status:?} with {reason:?}",
            row.case
        );
    }
}
