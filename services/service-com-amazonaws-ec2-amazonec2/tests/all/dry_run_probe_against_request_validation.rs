// SPDX-License-Identifier: CC0-1.0

//! WHERE a `DryRun` probe is answered among an operation's own checks, driven over the wire
//! the service speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rule under test: `DryRun` "checks whether you have the required permissions for the
//! action, WITHOUT ACTUALLY MAKING THE REQUEST, and provides an error response. If you have
//! the required permissions, the error response is DryRunOperation" (the member's own doc, on
//! every input shape that carries it). So the probe is answered at the point where carrying
//! the request out would BEGIN:
//!
//! * AFTER the refusals that read the REQUEST — a value the operation will not accept, a
//!   combination of members that cannot be given together, and a resource the request NAMES
//!   that the account holds none of (an AMI, an IAM instance profile, a placement group, an
//!   endpoint service, a subnet, an instance);
//! * BEFORE the refusals that come out of reading two of the resources it named AGAINST EACH
//!   OTHER, or out of performing the work — a security group against the subnet it would sit
//!   in, a private-DNS request against the named service's published DNS name and against the
//!   VPC's own DNS attributes, the filters a describe would select with, and the write itself.
//!
//! Each table below spans that line from both sides: rows whose refusal the probe WINS over
//! (412 `DryRunOperation`) and rows that win over the probe (the request's own refusal), with
//! the same request made again WITHOUT the flag to show the real call still answers what it
//! answered. No row is the specification: each is one input along what the rule generalises
//! over — which member is at fault, whether the resource it names exists, and which of the
//! two sides of the line the refusal falls on.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2::state::MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for: the account's default VPC and its default
/// subnets are seeded for it, so a launch that names no network of its own is placed.
const REGION: &str = "us-east-1";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The access key id the IAM calls below are presented under: the account's own published
/// credential.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The AMI every launch here names: which image an instance comes from has nothing to do with
/// where the probe is answered.
const AMI: &str = "ami-0123456789abcdef0";

/// EC2's answer to a probe the caller may make, and the status it carries it on.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// The whole sentence that answer carries — fixed text, naming nothing of the request.
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The status every refusal of a REQUEST below carries.
const REFUSAL_STATUS: u16 = 400;

/// A well-formed security group id the account holds no group under.
const UNHELD_GROUP: &str = "sg-0000000000000ffff";

/// A well-formed, long-form instance id this account was never issued.
const UNISSUED_INSTANCE: &str = "i-0000000000000ffff";

/// What the SAME request answers when it carries no flag at all.
#[derive(Debug, Clone, Copy)]
enum WithoutTheFlag {
    /// The call is served (HTTP 200).
    Served,
    /// The call is refused with this code.
    Refused(&'static str),
}

/// One row: the case, the request's members beside the flag, what a PROBE is answered, and
/// what the same request answers without the flag.
struct Row {
    case: &'static str,
    params: Vec<Param>,
    probe_code: &'static str,
    without_the_flag: WithoutTheFlag,
}

/// The emulator the rows drive: EC2 beside IAM on ONE shared engine, as the emulator mounts
/// them, so an instance-profile reference is resolved against the records IAM really holds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One EC2 `action` call carrying `params`.
async fn ec2_call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One EC2 `action` call that must be served, as its body.
async fn served(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = ec2_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must be served, got {status} with body {body}"
    );
    body
}

/// Run every row of a table against `action`: first as a probe, then as the same request
/// without the flag.
async fn assert_probe_position(router: &MultiServiceRouter, action: &str, rows: Vec<Row>) {
    for Row {
        case,
        params,
        probe_code,
        without_the_flag,
    } in rows
    {
        let mut probe = params.clone();
        probe.push(param("DryRun", "true"));
        let (status, body) = ec2_call(router, action, &probe).await;
        let expected_status = if probe_code == DRY_RUN_CODE {
            DRY_RUN_STATUS
        } else {
            REFUSAL_STATUS
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from a probe, got {status} with \
             body {body}"
        );
        let answered = error_code(&body);
        assert_eq!(
            answered, probe_code,
            "case {case:?}: expected {probe_code} from a probe, got {answered:?} in {body}"
        );
        if probe_code == DRY_RUN_CODE {
            let message = error_message(&body);
            assert_eq!(
                message, DRY_RUN_MESSAGE,
                "case {case:?}: expected EC2's own probe sentence, got {message:?}"
            );
        }

        let (status, body) = ec2_call(router, action, &params).await;
        match without_the_flag {
            WithoutTheFlag::Served => assert_eq!(
                status, 200,
                "case {case:?}: without the flag the request must be served, got {status} \
                 with body {body}"
            ),
            WithoutTheFlag::Refused(code) => {
                let answered = error_code(&body);
                assert_eq!(
                    (status, answered.as_str()),
                    (REFUSAL_STATUS, code),
                    "case {case:?}: without the flag the request must be refused {code}, got \
                     {status} with body {body}"
                );
            }
        }
    }
}

/// `NetworkInterface.<index>` form fields for one entry of a launch's `NetworkInterfaces`.
fn launch_interface(index: usize, fields: &[(&str, &str)]) -> Vec<Param> {
    fields
        .iter()
        .map(|(name, value)| param(&format!("NetworkInterface.{index}.{name}"), value))
        .collect()
}

/// The members every launch below carries beside the ones its row is about.
fn launch(extra: Vec<Param>) -> Vec<Param> {
    let mut params = vec![
        param("ImageId", AMI),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    params.extend(extra);
    params
}

/// Create the instance profile `name` in IAM, so EC2 resolves a reference to it against a
/// record the owning service really holds.
async fn create_instance_profile(case: &str, router: &MultiServiceRouter, name: &str) {
    let (status, body) = super::ec2_query::query_call_as(
        router,
        REGION,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        "CreateInstanceProfile",
        &[("InstanceProfileName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: IAM must hold the profile {name:?}, got {status}: {body}"
    );
}

/// RULE, on the launch path: a probe is answered after the launch's own members have been read
/// and the resources it names resolved, and before any two of those resources are read against
/// each other.
#[tokio::test]
async fn a_launch_probe_is_answered_after_the_request_and_before_the_launch_is_assembled() {    let (router, engine) = emulator();
    let instances = || engine.state::<Ec2State>().lock().instances.len();
    let before = instances();

    // The resource the rows that name a HELD one refer to, created through the service that
    // owns it.
    create_instance_profile("the held profile", &router, "web-tier").await;

    let rows = vec![
        // Nothing wrong with the request at all: the probe is the whole answer.
        Row {
            case: "a launch that would have succeeded",
            params: launch(Vec::new()),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Served,
        },
        // The resource the launch names is there, so the probe is still the answer — the row
        // below differs only in that the account holds none of it.
        Row {
            case: "a launch naming an instance profile the account holds",
            params: launch(vec![param("IamInstanceProfile.Name", "web-tier")]),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Served,
        },
        // READING the launch's own parameters is what the probe comes AFTER: a combination of
        // members that cannot be given together is answered to a probe exactly as it is to
        // the real call.
        Row {
            case: "network interfaces beside an instance-level security group",
            params: launch(
                launch_interface(1, &[("DeviceIndex", "0")])
                    .into_iter()
                    .chain(indexed("SecurityGroupId", &[UNHELD_GROUP]))
                    .collect(),
            ),
            probe_code: "InvalidParameterCombination",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterCombination"),
        },
        Row {
            case: "network interfaces beside an instance-level private address",
            params: launch(
                launch_interface(1, &[("DeviceIndex", "0")])
                    .into_iter()
                    .chain([param("PrivateIpAddress", "10.0.0.17")])
                    .collect(),
            ),
            probe_code: "InvalidParameterCombination",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterCombination"),
        },
        Row {
            case: "two network interfaces at one device index",
            params: launch(
                launch_interface(1, &[("DeviceIndex", "0")])
                    .into_iter()
                    .chain(launch_interface(2, &[("DeviceIndex", "0")]))
                    .collect(),
            ),
            probe_code: "InvalidParameterValue",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "network interfaces naming no primary interface",
            params: launch(launch_interface(1, &[("DeviceIndex", "1")])),
            probe_code: "InvalidParameterCombination",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterCombination"),
        },
        // READING TWO OF THE NAMED RESOURCES AGAINST EACH OTHER is what the probe comes
        // BEFORE: which groups may be applied in the subnet the launch lands in is resolved
        // for the real call alone, so the probe wins over that refusal.
        Row {
            case: "a security group read against the subnet the launch lands in",
            params: launch(indexed("SecurityGroupId", &[UNHELD_GROUP])),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidGroup.NotFound"),
        },
        // The rest of the request's own reading: a member the launch got wrong, and a
        // resource it names that the account holds none of.
        Row {
            case: "an instance profile the account's IAM records hold none of",
            params: launch(vec![param("IamInstanceProfile.Name", "never-created")]),
            probe_code: "InvalidParameterValue",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "an image id that is no AMI id at all",
            params: vec![
                param("ImageId", "ami-not-an-id"),
                param("MinCount", "1"),
                param("MaxCount", "1"),
            ],
            probe_code: "InvalidAMIID.Malformed",
            without_the_flag: WithoutTheFlag::Refused("InvalidAMIID.Malformed"),
        },
        Row {
            case: "a Systems Manager parameter that resolves to no image of the account",
            params: vec![
                param("ImageId", "resolve:ssm:/aws/service/never-published"),
                param("MinCount", "1"),
                param("MaxCount", "1"),
            ],
            probe_code: "SsmInvalidParameter",
            without_the_flag: WithoutTheFlag::Refused("SsmInvalidParameter"),
        },
        Row {
            case: "a placement group the account holds none of",
            params: launch(vec![param("Placement.GroupName", "never-created")]),
            probe_code: "InvalidPlacementGroup.Unknown",
            without_the_flag: WithoutTheFlag::Refused("InvalidPlacementGroup.Unknown"),
        },
        Row {
            case: "an instance count below the smallest a launch may ask for",
            params: vec![
                param("ImageId", AMI),
                param("MinCount", "0"),
                param("MaxCount", "1"),
            ],
            probe_code: "InvalidParameterValue",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
    ];
    let served_rows = rows
        .iter()
        .filter(|row| matches!(row.without_the_flag, WithoutTheFlag::Served))
        .count();

    assert_probe_position(&router, "RunInstances", rows).await;

    // Every probe performed nothing: the only instances the store holds are the ones the
    // rows whose real call was served launched.
    let after = instances();
    assert_eq!(
        after,
        before + served_rows,
        "the probes must launch nothing: the store held {before} instances, {served_rows} \
         real launches were served, and it now holds {after}"
    );
}

/// RULE, on a reporting path: resolving the filters a describe selects with is part of
/// PERFORMING it, so a probe is answered before them — while the request's own members, and
/// the instances it names, are read first.
#[tokio::test]
async fn a_describe_probe_is_answered_before_its_filters_are_resolved() {
    let (router, _engine) = emulator();
    // One instance to describe, so the rows are about the probe rather than about an empty
    // account.
    served("the described instance", &router, "RunInstances", &launch(Vec::new())).await;

    let rows = vec![
        Row {
            case: "a describe that would have succeeded",
            params: Vec::new(),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Served,
        },
        // Filter names EC2 does not document, each refused to the real call and none of them
        // read for a probe. The rows differ along the SHAPE of the undocumented name: a word
        // of its own, a near-miss of a documented one, a tag selector with no key, and an
        // entry that names no filter at all.
        Row {
            case: "a filter name of its own",
            params: filters(&[("bogus-filter", &["anything"])]),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "a near-miss of a documented filter name",
            params: filters(&[("instance-ids", &["i-0123456789abcdef0"])]),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "a tag selector naming no key",
            params: filters(&[("tag:", &["anything"])]),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "an entry that names no filter at all",
            params: vec![param("Filter.1.Value.1", "anything")],
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        // The request's own members, read before the probe is answered.
        Row {
            case: "a long-form instance id this account was never issued",
            params: indexed("InstanceId", &[UNISSUED_INSTANCE]),
            probe_code: "InvalidInstanceID.Malformed",
            without_the_flag: WithoutTheFlag::Refused("InvalidInstanceID.Malformed"),
        },
        Row {
            case: "a page size below the smallest the operation pages with",
            params: vec![param("MaxResults", "1")],
            probe_code: "InvalidParameterValue",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "an id list beside a page size",
            params: indexed("InstanceId", &["i-0123456789abcdef0"])
                .into_iter()
                .chain([param("MaxResults", "5")])
                .collect(),
            probe_code: "InvalidParameterCombination",
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterCombination"),
        },
    ];
    assert_probe_position(&router, "DescribeInstances", rows).await;
}

/// RULE, on a create that names two resources: reading them AGAINST EACH OTHER is part of
/// carrying the create out, so a probe is answered before that and after each of them has been
/// resolved on its own.
#[tokio::test]
async fn a_create_probe_is_answered_before_the_named_resources_are_read_against_each_other() {
    let (router, engine) = emulator();
    let endpoints = || {
        engine
            .state::<Ec2State>()
            .lock()
            .instance_connect_endpoints
            .len()
    };

    // One network for the endpoint, and a second whose security group belongs to another
    // network — built through the API, so the endpoint is created against resources this
    // emulator really stores.
    let body = served(
        "the endpoint's VPC",
        &router,
        "CreateVpc",
        &[param("CidrBlock", "10.60.0.0/16")],
    )
    .await;
    let vpc_id = element(&body, "vpcId").expect("the create reports the VPC it minted");
    let body = served(
        "the endpoint's subnet",
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.60.1.0/24"),
            param("AvailabilityZone", "us-east-1a"),
        ],
    )
    .await;
    let subnet_id = element(&body, "subnetId").expect("the create reports the subnet it minted");
    let body = served(
        "another network",
        &router,
        "CreateVpc",
        &[param("CidrBlock", "10.61.0.0/16")],
    )
    .await;
    let other_vpc = element(&body, "vpcId").expect("the create reports the VPC it minted");
    let body = served(
        "a group of another network",
        &router,
        "CreateSecurityGroup",
        &[
            param("VpcId", &other_vpc),
            param("GroupName", "elsewhere"),
            param("GroupDescription", "a group of another network"),
        ],
    )
    .await;
    let foreign_group = element(&body, "groupId").expect("the create reports the group it minted");

    let rows = vec![
        Row {
            case: "an endpoint that would have been created",
            params: vec![param("SubnetId", &subnet_id)],
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Served,
        },
        Row {
            case: "a security group of another network",
            params: vec![param("SubnetId", &subnet_id)]
                .into_iter()
                .chain(indexed("SecurityGroupId", &[foreign_group.as_str()]))
                .collect(),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameterValue"),
        },
        Row {
            case: "a security group the account holds none of",
            params: vec![param("SubnetId", &subnet_id)]
                .into_iter()
                .chain(indexed("SecurityGroupId", &[UNHELD_GROUP]))
                .collect(),
            probe_code: "InvalidGroup.NotFound",
            without_the_flag: WithoutTheFlag::Refused("InvalidGroup.NotFound"),
        },
        Row {
            case: "a subnet the account holds none of",
            params: vec![param("SubnetId", "subnet-0000000000000ffff")],
            probe_code: "InvalidSubnetID.NotFound",
            without_the_flag: WithoutTheFlag::Refused("InvalidSubnetID.NotFound"),
        },
    ];
    let served_rows = rows
        .iter()
        .filter(|row| matches!(row.without_the_flag, WithoutTheFlag::Served))
        .count();
    let before = endpoints();
    assert_probe_position(&router, "CreateInstanceConnectEndpoint", rows).await;
    let after = endpoints();
    assert_eq!(
        after,
        before + served_rows,
        "the probes must create nothing: the store held {before} endpoints, {served_rows} \
         real creates were served, and it now holds {after}"
    );
}

/// RULE, on the same line for another create: an interface endpoint's private hosted zone is
/// judged against the VPC's own DNS attributes, which is two resources read against each
/// other — so a probe is answered before it, while a service name that names no endpoint
/// service is answered to the probe itself.
#[tokio::test]
async fn an_endpoint_probe_is_answered_before_the_vpcs_dns_attributes_are_read() {
    let (router, engine) = emulator();
    let stored = || engine.state::<Ec2State>().lock().vpc_endpoints.len();

    // "enableDnsHostnames is true for a default VPC and false for a VPC you create" (Amazon
    // VPC User Guide, "DNS attributes in your VPC"), so a VPC created here holds no private
    // hosted zone — which is exactly what the rows below ask for.
    let body = served(
        "the endpoint's VPC",
        &router,
        "CreateVpc",
        &[param("CidrBlock", "10.62.0.0/16")],
    )
    .await;
    let vpc_id = element(&body, "vpcId").expect("the create reports the VPC it minted");
    let body = served(
        "the endpoint's subnet",
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.62.1.0/24"),
            param("AvailabilityZone", "us-east-1a"),
        ],
    )
    .await;
    let subnet_id = element(&body, "subnetId").expect("the create reports the subnet it minted");
    let interface_endpoint = |service_name: &str, private_dns: &str| -> Vec<Param> {
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", service_name),
            param("VpcEndpointType", "Interface"),
            param("PrivateDnsEnabled", private_dns),
        ];
        params.extend(indexed("SubnetId", &[subnet_id.as_str()]));
        params
    };

    let rows = vec![
        Row {
            case: "an endpoint that would have been created",
            params: interface_endpoint("com.amazonaws.us-east-1.logs", "false"),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Served,
        },
        // Private DNS in a VPC whose attributes do not admit a private hosted zone: the real
        // call is refused for it and the probe is not held to it. The rows differ in which
        // service's zone was asked for.
        Row {
            case: "private DNS for a logs endpoint",
            params: interface_endpoint("com.amazonaws.us-east-1.logs", "true"),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameter"),
        },
        Row {
            case: "private DNS for an SSM endpoint",
            params: interface_endpoint("com.amazonaws.us-east-1.ssm", "true"),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameter"),
        },
        Row {
            case: "private DNS for a service that publishes no DNS name of its own",
            params: interface_endpoint("com.amazonaws.us-east-1.iot.data", "true"),
            probe_code: DRY_RUN_CODE,
            without_the_flag: WithoutTheFlag::Refused("InvalidParameter"),
        },
        // The request's own members, read before the probe is answered.
        Row {
            case: "a service name no endpoint service of the Region publishes",
            params: interface_endpoint("com.amazonaws.us-east-1.not-a-real-service", "false"),
            probe_code: "InvalidServiceName",
            without_the_flag: WithoutTheFlag::Refused("InvalidServiceName"),
        },
    ];
    let served_rows = rows
        .iter()
        .filter(|row| matches!(row.without_the_flag, WithoutTheFlag::Served))
        .count();
    let before = stored();
    assert_probe_position(&router, "CreateVpcEndpoint", rows).await;
    let after = stored();
    assert_eq!(
        after,
        before + served_rows,
        "the probes must create nothing: the store held {before} endpoints, {served_rows} \
         real creates were served, and it now holds {after}"
    );
}

/// The SENTENCE the refusal this change adds carries: EC2's own `iamInstanceProfile.name`
/// wording, whose only variable part is the name the REQUEST sent — so the rows differ in that
/// name, and each answer must name the row's own, with or without the flag.
#[tokio::test]
async fn an_unheld_instance_profile_is_refused_in_ec2s_own_wording() {
    let (router, engine) = emulator();
    // Rows: names differing in length and in which of the characters IAM's
    // `instanceProfileNameType` pattern admits they carry, so a sentence that echoed anything
    // but the request's own value would show up as the wrong name.
    for name in [
        "never-created",
        "n",
        "app+role=1,.@-_x",
        "sim2real-probe-ip-PowerUserRole",
        &"p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS),
    ] {
        let expected = format!(
            "Value ({name}) for parameter iamInstanceProfile.name is invalid. \
             Invalid IAM Instance Profile name"
        );
        let params = launch(vec![param("IamInstanceProfile.Name", name)]);
        for (case, mut sent) in [
            (format!("a probe naming {name:?}"), params.clone()),
            (format!("a launch naming {name:?}"), params.clone()),
        ] {
            if case.starts_with("a probe") {
                sent.push(param("DryRun", "true"));
            }
            let (status, body) = ec2_call(&router, "RunInstances", &sent).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
            );
            let answered = error_message(&body);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
    // Nothing was launched: every row of this table is a refusal.
    let instances = engine.state::<Ec2State>().lock().instances.len();
    assert_eq!(
        instances, 0,
        "a refused launch must store no instance, got {instances}"
    );
}

