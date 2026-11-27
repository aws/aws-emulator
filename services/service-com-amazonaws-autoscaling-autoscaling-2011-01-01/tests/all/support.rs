// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the engine and router every
//! test drives the service through, the awsQuery POST, and the two readers over
//! an answered XML body. A suite keeps only what is its own.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state as ec2;
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7 as elb_service;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as elbv2_service;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam_service;

/// The region every suite signs for, so the ARNs the service stamps are checked
/// against the region the request names rather than a default.
pub const REGION: &str = "us-west-2";

/// Percent-encode one form value: RFC 3986's unreserved set (the alphanumerics
/// plus `-`, `.`, `_` and `~`) is all that may stay literal in an
/// `application/x-www-form-urlencoded` body.
///
/// THE ONE COPY in this crate's tests: every suite posts through [`query`] below,
/// or builds its own request on this encoder, so it is defined here and nowhere
/// else. Written out rather than taken from `percent_encoding` for one reason — a
/// dev-dependency rewrites `Cargo.lock`, the file nearly every other change in this
/// repository also rewrites, and that lock conflict is what repeatedly blocked this
/// fix from being merged. It is RFC 3986's unreserved set, exercised by every
/// request these suites make.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn new_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// Every launch template name the suites in this crate create their groups from.
///
/// A group launches from a launch template THE ACCOUNT HOLDS — Amazon EC2 Auto Scaling
/// resolves the specification when the group is configured, and refuses a request naming a
/// template Amazon EC2 does not have — so the harness puts these templates in the account's
/// own Amazon EC2 store before any suite creates a group. A suite that creates its own
/// template over Amazon EC2's wire (through [`create_launch_template`]) names it itself and
/// is not listed here.
const SUITE_LAUNCH_TEMPLATES: &[&str] = &[
    "lt-1",
    "lt-activities",
    "lt-cancel",
    "lt-cancel-absent",
    "lt-refresh",
    "lt-refresh-next",
    "lt-rollback",
    "lt-rollback-next",
    "lt-wording",
    "tags-template",
];

/// The AMI the launch templates of this harness state.
///
/// "An AMI ID is required to launch an instance"
/// (`smithy_query(query="doc",shape="RunInstancesRequest",member="ImageId")`), and Amazon
/// EC2 Auto Scaling reads the launch parameters of the version a group points at when the group
/// is configured: a template stating no image names nothing its instances could run, so every
/// template a suite here creates a group from states this one.
pub const SEEDED_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type the launch templates of this harness state.
///
/// An instance of a group is launched as one instance TYPE, and a group that is not configured
/// as a mixed instances group takes it from the launch-template version it launches from, so
/// every template a suite here creates a group from states this one. Any type is a valid
/// `InstanceType` string; which types exist is not what these suites are about.
pub const SEEDED_INSTANCE_TYPE: &str = "t3.micro";

/// The launch parameters a template this harness seeds carries: the image its instances run
/// and the instance type they run as, and nothing else.
///
/// THE ONE COPY for every suite here that writes a launch-template version through Amazon EC2's
/// own writer, so a seeded version is one a group can be configured against. A suite that cares
/// what an instance is launched WITH adds the parameters it means to this.
pub fn seeded_launch_template_data() -> ec2_service::ops::types::RequestLaunchTemplateData {
    ec2_service::ops::types::RequestLaunchTemplateData {
        image_id: Some(SEEDED_IMAGE_ID.to_string()),
        instance_type: Some(SEEDED_INSTANCE_TYPE.to_string()),
        ..Default::default()
    }
}

/// Put the launch templates `names` in the account's own Amazon EC2 store, through EC2's
/// own writer, so a group created from one launches from a template the account holds.
///
/// THE ONE COPY for every suite here that creates a group: the launch templates an Auto
/// Scaling request names are Amazon EC2's resources, so they are written through EC2's own
/// store rather than invented here. Each carries only the image a launch requires
/// ([`seeded_launch_template_data`]): a suite that cares what an instance is launched WITH
/// creates its template over EC2's wire with the parameters it means.
pub fn seed_launch_templates(engine: &Arc<engine::InMemoryEngine>, names: &[&str]) {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    for name in names {
        store.insert_launch_template(ec2::LaunchTemplateSpec {
            name: (*name).to_string(),
            created_by: engine::emulator_arn("iam", "", "root"),
            create_time: std::time::SystemTime::now(),
            tags: Vec::new(),
            operator: None,
            version_description: None,
            data: seeded_launch_template_data(),
        });
    }
}

pub fn router_with(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    seed_launch_templates(&engine, SUITE_LAUNCH_TEMPLATES);
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(engine));
    router
}

/// POST one awsQuery action and return its status and body.
pub async fn query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2011-01-01");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://autoscaling.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/autoscaling/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One `(key, value)` request parameter, for a suite whose values are built per case.
pub fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The API version an Elastic Load Balancing v2 client puts in its awsQuery body, read from
/// that service's own statement of it rather than written out here.
const ELBV2_VERSION: &str = elbv2_service::service::shared_endpoint::API_VERSION;

/// One emulator with the three services on ONE engine: Auto Scaling, Amazon EC2 and Elastic
/// Load Balancing v2.
///
/// THE ONE COPY for every suite here whose subject spans them — a group's instances are the
/// account's own Amazon EC2 instances, and the target groups it is attached to and the
/// targets they hold are Elastic Load Balancing v2's own records, so one engine is what
/// makes the three views one account.
pub fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_launch_templates(&engine, SUITE_LAUNCH_TEMPLATES);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(elbv2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// [`emulator`] with IAM mounted beside the three, on that same engine.
///
/// The one copy for a suite whose subject reaches an IAM record as well — the instance
/// profile a group's launch source names is IAM's resource, so a launch that resolves it
/// resolves it where it lives. The suites that mounted their own four-service router before
/// this existed are to be folded onto it by their own issues; nothing new should write
/// another one.
pub fn emulator_with_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator();
    router.register_service(iam_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// [`emulator`] with Elastic Load Balancing (Classic) mounted beside the three, on that same
/// engine.
///
/// THE ONE COPY for every suite here whose subject reaches the account's own Classic Load
/// Balancers: the load balancers a group's `LoadBalancerNames` names are that service's
/// records, so a suite that needs them to EXIST creates them where they live.
pub fn emulator_with_classic_elb() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator();
    router.register_service(elb_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// POST one awsQuery action to Elastic Load Balancing (Classic), signed for `region`, the way
/// a client does, and answer its status and body.
///
/// The API version is that service's own statement of the version a Classic client sends,
/// rather than a copy of it written out here.
pub async fn classic_elb_query(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!(
        "Action={action}&Version={}",
        elb_service::service::api_version::CLASSIC_API_VERSION
    );
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://elasticloadbalancing.{region}.amazonaws.com/"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}\
                 /elasticloadbalancing/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// Create one Classic Load Balancer named `name` in `region`, enabled for the Availability
/// Zone `zone` and carrying one HTTP listener on port 80, over Elastic Load Balancing's own
/// wire.
///
/// For the suites here whose groups name load balancers the account has to HOLD: a Classic
/// Load Balancer is identified within one Region, so the Region it is created in is stated by
/// the caller. A suite that cares what its listener forwards to, or what its health check
/// asks for, posts its own create instead.
pub async fn create_classic_load_balancer(
    router: &MultiServiceRouter,
    region: &str,
    zone: &str,
    name: &str,
) {
    let (status, body) = classic_elb_query(
        router,
        region,
        "CreateLoadBalancer",
        &[
            p("LoadBalancerName", name),
            p("AvailabilityZones.member.1", zone),
            p("Listeners.member.1.Protocol", "HTTP"),
            p("Listeners.member.1.LoadBalancerPort", "80"),
            p("Listeners.member.1.InstanceProtocol", "HTTP"),
            p("Listeners.member.1.InstancePort", "80"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLoadBalancer for {name:?} in {region:?} expected 200, got {status}: {body}"
    );
}

/// POST one awsQuery action to Elastic Load Balancing v2, the way a client does, and answer
/// its status and body.
///
/// THE ONE COPY for every suite here that reaches the account's own target groups: they are
/// that service's resources, and this is how a client asks it for them.
pub async fn elbv2_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={ELBV2_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://elasticloadbalancingv2.{REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The VPC a seeded subnet is in, read back through Amazon EC2's own `DescribeSubnets` —
/// the service that owns the subnet is what says which VPC it is in.
pub async fn vpc_of(router: &MultiServiceRouter, subnet: &SeededSubnet) -> String {
    let (status, body) = ec2_query(router, "DescribeSubnets", &[p("SubnetId.1", &subnet.id)]).await;
    assert_eq!(
        status, 200,
        "DescribeSubnets for {:?} expected 200, got {status}: {body}",
        subnet.id
    );
    let vpc_id = tag_value(&body, "vpcId");
    assert!(
        !vpc_id.is_empty(),
        "DescribeSubnets must report the subnet's VPC, got {body}"
    );
    vpc_id
}

/// Create one Elastic Load Balancing v2 target group over that service's own wire and answer
/// the ARN it minted for it.
///
/// A target group whose targets are INVOKED rather than connected to (`TargetType` `lambda`)
/// has no network identity, and Elastic Load Balancing refuses a create that states one for
/// it — `Protocol`, `Port` and `VpcId` each document "if the target is a Lambda function,
/// this parameter does not apply" — so such a row asks for none of the three.
pub async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    port: i32,
    target_type: &str,
    vpc_id: &str,
) -> String {
    let mut params = vec![p("Name", name), p("TargetType", target_type)];
    if target_type != "lambda" {
        params.push(p("Protocol", "HTTP"));
        params.push(p("Port", port.to_string()));
        params.push(p("VpcId", vpc_id));
    }
    let (status, body) = elbv2_query(router, "CreateTargetGroup", &params).await;
    assert_eq!(
        status, 200,
        "CreateTargetGroup for {name:?} expected 200, got {status}: {body}"
    );
    let arn = tag_value(&body, "TargetGroupArn");
    assert!(
        !arn.is_empty(),
        "CreateTargetGroup must report the target group's ARN, got {body}"
    );
    arn
}

/// Create one Amazon EC2 launch template over that service's own wire, so a group launches
/// from a template the account actually holds, and answer the `(id, latest version number)`
/// that service assigned it — values a suite naming the template by id or by version takes
/// from EC2 rather than writing out.
pub async fn create_launch_template(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", SEEDED_IMAGE_ID),
            p("LaunchTemplateData.InstanceType", SEEDED_INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    let latest_version = tag_value(&body, "latestVersionNumber");
    assert!(
        !id.is_empty() && !latest_version.is_empty(),
        "CreateLaunchTemplate for {name:?} must report the template's id and latest version \
         number, got {body}"
    );
    (id, latest_version)
}

/// Delete one Amazon EC2 launch template through the service that owns it, and assert a client
/// can no longer read it back THERE — the precondition every suite that drives a DANGLING
/// launch-template reference rests on, since a group's reference is resolved against that
/// service's own store.
///
/// THE ONE COPY for the suites here whose subject is a reference to a template the account no
/// longer holds: the template is Amazon EC2's resource, so it is removed where a client removes
/// it rather than out of that service's store by hand.
pub async fn delete_launch_template(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) =
        ec2_query(router, "DeleteLaunchTemplate", &[p("LaunchTemplateId", id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteLaunchTemplate for {id:?} expected 200, got {status}: {body}"
    );
    let (status, body) = ec2_query(
        router,
        "DescribeLaunchTemplates",
        &[p("LaunchTemplateId.1", id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: Amazon EC2 must no longer hold the deleted template {id:?}, got {status}: \
         {body}"
    );
}

/// Where a group places the instances it launches, for a suite whose cases span both ways of
/// stating it.
///
/// THE ONE COPY for the suites here that create one group per placement: a group states
/// either the Availability Zones it places instances in or the subnets it places them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// `AvailabilityZones`, naming the zone a seeded subnet is in.
    Zone,
    /// `VPCZoneIdentifier`, naming that subnet.
    Subnet,
}

/// The path IAM reserves for a service-linked role, followed by the service principal Amazon
/// EC2 Auto Scaling acts under.
///
/// SOURCE: a service-linked role is filed under `/aws-service-role/<service principal>/`, and
/// this service's principal is `autoscaling.amazonaws.com` — the principal IAM's own catalog
/// of published service-linked roles names for Amazon EC2 Auto Scaling.
pub const DEFAULT_SERVICE_LINKED_ROLE_PATH: &str = "/aws-service-role/autoscaling.amazonaws.com/";

/// The name of the role a group is bound to when its create names none.
///
/// SOURCE: `CreateAutoScalingGroupType$ServiceLinkedRoleARN` — "By default, Amazon EC2 Auto
/// Scaling uses a service-linked role named `AWSServiceRoleForAutoScaling`, which it creates
/// if it does not exist."
pub const DEFAULT_SERVICE_LINKED_ROLE_NAME: &str = "AWSServiceRoleForAutoScaling";

/// The ARN of that role: fixed text apart from the account id, which is the caller's own
/// account.
///
/// THE ONE COPY for every suite here that reads the role a group is bound to.
pub fn default_service_linked_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role{DEFAULT_SERVICE_LINKED_ROLE_PATH}{DEFAULT_SERVICE_LINKED_ROLE_NAME}",
        ::engine::emulator_account_id()
    )
}

/// The owned parameters of a case, as the `&[(&str, &str)]` [`query`] takes.
pub fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// The ec2Query API version a client sends in an Amazon EC2 request body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The awsQuery API version a client sends in an IAM request body.
const IAM_API_VERSION: &str = "2010-05-08";

/// POST one ec2Query action, signed for Amazon EC2 in [`REGION`], and answer its status
/// and body.
///
/// THE ONE COPY for every suite here that reaches the account's own Amazon EC2 resources
/// over the wire: the subnets, launch templates and instances an Auto Scaling group is
/// built from are EC2's, and this is how a client asks EC2 about them.
pub async fn ec2_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// POST one awsQuery action to IAM and answer its status and body.
///
/// IAM is a global service, so the request carries no Region in its endpoint host; the
/// credential scope names the one every AWS SDK signs an IAM request for.
pub async fn iam_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/iam/aws4_request, \
             SignedHeaders=host, Signature=abc",
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One subnet seeded into the account's own Amazon EC2 store: its id, and the zone EC2
/// holds it in.
#[derive(Debug, Clone)]
pub struct SeededSubnet {
    pub id: String,
    pub zone: String,
}

/// Seed `count` subnets into the account's own Amazon EC2 store, through EC2's own
/// writers, and answer them in the order they were created.
///
/// THE ONE COPY for every suite that needs a group placed by subnet: the subnets an Auto
/// Scaling request names are EC2's, so they are written through EC2's own store rather
/// than invented here.
///
/// The i-th subnet is in the i-th Availability Zone of [`REGION`], wrapping when more
/// subnets are asked for than the Region has zones — so asking for one more than the zone
/// count puts two subnets in one zone. The zone names are EC2's own catalog for the Region
/// (`ec2::availability_zones_in_region`), never written out here, and each block is the
/// n-th default-sized block of the VPC's own CIDR (`ec2::nth_subnet_cidr_block`).
pub fn seed_subnets(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<SeededSubnet> {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    let vpc_id = store.insert_vpc(
        ec2::DEFAULT_VPC_CIDR_BLOCK,
        false,
        ec2::DEFAULT_INSTANCE_TENANCY,
        ec2::VPC_STATE_AVAILABLE,
        REGION,
        Vec::new(),
    );
    let zones = ec2::availability_zones_in_region(REGION);
    assert!(
        !zones.is_empty(),
        "the Region {REGION} must have Availability Zones to seed subnets in"
    );
    (0..count)
        .map(|index| {
            let zone = zones[index % zones.len()].clone();
            let cidr_block = ec2::nth_subnet_cidr_block(
                ec2::DEFAULT_VPC_CIDR_BLOCK,
                ec2::DEFAULT_SUBNET_PREFIX_LENGTH,
                u32::try_from(index).expect("the seeded subnet count fits in a u32"),
            )
            .expect("a default-sized block inside the VPC's own block");
            let id = store.insert_subnet(ec2::SubnetSpec {
                vpc_id: vpc_id.clone(),
                cidr_block,
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id,
                region: REGION.to_string(),
                state: ec2::SUBNET_STATE_AVAILABLE.to_string(),
                default_for_az: false,
                map_public_ip_on_launch: true,
                tags: Vec::new(),
            });
            SeededSubnet {
                id,
                zone: zone.zone_name,
            }
        })
        .collect()
}

/// The text of every `<tag>…</tag>` in document order, whatever ATTRIBUTES the open tag
/// carries.
///
/// THE ONE COPY in this crate's tests, and attribute-tolerant because the envelope stamps
/// the document's namespace onto the FIRST element of a list (`<member xmlns="…">`): an
/// element is matched by its tag NAME plus whatever attributes follow it, never by the
/// literal `<tag>` text, so the first member of a list is read like every other one.
pub fn tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        // What follows the NAME decides: `>` closes the open tag and whitespace begins its
        // attributes, while anything else means the match was a longer tag name that merely
        // begins with this one — and a self-closing `<tag/>` holds no text to read.
        if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
            rest = after;
            continue;
        }
        let Some(open_end) = after.find('>') else { break };
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else { break };
        out.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    out
}

/// Decode the five XML entities the envelope escapes its text with, so an assertion
/// compares the value a client parses rather than its wire spelling.
///
/// THE ONE COPY in this crate's tests: every suite that reads an escaped `<Message>`
/// or `<Value>` back decodes it through here.
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The whole message the service refuses a request whose maximum group size is below its
/// minimum with: fixed text apart from the two bounds, and the MAXIMUM is named first.
///
/// THE ONE COPY for this crate's tests, written out here rather than taken from the
/// service's own builder — a test that re-used the implementation's literal would assert
/// nothing. Generic over how a suite spells the bounds, because one states them as the
/// integers it sends and another as the request parameters it builds.
pub fn max_below_min_message(
    max_size: impl ::std::fmt::Display,
    min_size: impl ::std::fmt::Display,
) -> String {
    format!("Max bound, {max_size}, must be greater than or equal to min bound, {min_size}")
}

/// The whole message the service refuses a request whose minimum group size is below zero
/// with: fixed text apart from the bound the request handed it.
///
/// THE ONE COPY for this crate's tests, written out here rather than taken from the
/// service's own builder — a test that re-used the implementation's literal would assert
/// nothing. Generic over how a suite spells the bound, as [`max_below_min_message`] is.
pub fn negative_min_size_message(min_size: impl ::std::fmt::Display) -> String {
    format!("Min bound, {min_size}, must be non-negative")
}

/// The text of the first `<tag>…</tag>`, or `""` when the member is absent.
pub fn tag_value(xml: &str, tag: &str) -> String {
    tag_values(xml, tag).first().cloned().unwrap_or_default()
}

/// The XML inside the first element named `tag`: `Some("")` for an element holding nothing,
/// however the wire spells it (`<Tag/>` or `<Tag></Tag>`), and `None` when the document does
/// not name the element at all.
///
/// THE ONE COPY for every suite here whose subject is the difference between an EMPTY member
/// and an ABSENT one — a client reads an empty sequence off the one and nothing at all off the
/// other — so the two are kept apart rather than collapsed into a string, as
/// [`element_body`] below collapses them. Attribute-tolerant, because the envelope stamps the
/// document's namespace onto the elements a modelled structure renders, and name-exact, so an
/// element whose name merely BEGINS with the tag is not that element.
pub fn element_or_absent<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    loop {
        let at = rest.find(&open)?;
        let after = &rest[at + open.len()..];
        let boundary = after.chars().next()?;
        if !(boundary == '>' || boundary == '/' || boundary.is_whitespace()) {
            // A longer element name that merely begins like this one.
            rest = after;
            continue;
        }
        let open_end = after.find('>')?;
        if after[..open_end].ends_with('/') {
            return Some("");
        }
        let inner = &after[open_end + 1..];
        let end = inner.find(&close)?;
        return Some(&inner[..end]);
    }
}

/// The `<member>` entries of the list member `tag`, failing the case when the answer does not
/// NAME that member at all.
///
/// THE ONE COPY for every suite here that reads a list member back: an absent member is
/// reported as itself rather than read as an empty list, because a client that iterates the
/// member can tell the two apart.
pub fn named_list(case: &str, xml: &str, tag: &str) -> Vec<String> {
    match element_or_absent(xml, tag) {
        Some(inner) => tag_values(inner, "member"),
        None => panic!(
            "case {case:?}: expected the described group to name {tag} as a list, got {xml}"
        ),
    }
}

/// The inner text of the first `<tag …>…</tag>` ELEMENT, whose open tag may carry attributes
/// — so a member of a structure is read inside the element it belongs to rather than as "the
/// first element of that name in the document".
///
/// THE ONE COPY for every suite here that has to scope a read to one element: the envelope
/// stamps an `xmlns` on the elements a modelled structure renders, and an element whose name
/// merely BEGINS with the tag (`<Activities>` against `<Activity`) is not that element, so the
/// character after the name is what tells the two apart.
pub fn element_body<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    loop {
        let start = rest.find(&open)?;
        let after = &rest[start + open.len()..];
        let boundary = after.chars().next()?;
        if boundary == '>' || boundary.is_whitespace() {
            let inner_start = after.find('>')? + 1;
            let end = after.find(&close)?;
            return Some(&after[inner_start..end]);
        }
        rest = after;
    }
}

/// Each `<member>` of the list element `list` in one answer, as its own block.
///
/// THE ONE COPY for every suite here that reads a LIST of structures back: scoped to that
/// list, so a member of one element is read inside the element it belongs to rather than as
/// "the first element of that name in the document". The serializer stamps the document's
/// namespace onto the first member of a list, so a block starts after its own tag's closing
/// bracket; an answer reporting no such list, or an empty one, has no blocks to read.
pub fn list_members<'a>(xml: &'a str, list: &str) -> Vec<&'a str> {
    let Some(open) = xml.find(&format!("<{list}")) else {
        return Vec::new();
    };
    let Some((_, body)) = xml[open..].split_once('>') else {
        return Vec::new();
    };
    let body = body
        .split(&format!("</{list}>"))
        .next()
        .unwrap_or_default();
    body.split("<member")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, inner)| inner.split("</member>").next())
        .collect()
}

/// The causes of the scaling activities a group has recorded, newest first, unescaped.
///
/// THE ONE COPY for every suite here that reads WHY a group moved an instance: the cause is
/// what a client reads the reason off, so the read is defined once and each suite asserts
/// the sentence it is about.
pub async fn activity_causes(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeScalingActivities",
        &[("AutoScalingGroupName", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeScalingActivities for {group:?} expected 200, got {status}: {body}"
    );
    tag_values(&body, "Cause")
        .iter()
        .map(|cause| xml_unescape(cause))
        .collect()
}

/// POST one `awsJson` RPC call against a service mounted BESIDE Auto Scaling, the way a
/// client makes it: `POST /` with `X-Amz-Target: <target_prefix>.<operation>` and a
/// credential scoped to `signing_name`, which is what the router picks the service from.
///
/// THE ONE COPY for every suite here that drives another emulated service over the wire —
/// EventBridge, Amazon SQS — because a lifecycle step this service publishes is observed
/// where a client observes it: on the queue it was delivered to.
pub async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: ::serde_json::Value,
) -> (u16, ::serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.body().clone();
    let parsed = if payload.is_empty() {
        ::serde_json::json!({})
    } else {
        ::serde_json::from_slice(&payload).unwrap_or_else(|_| {
            ::serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}
