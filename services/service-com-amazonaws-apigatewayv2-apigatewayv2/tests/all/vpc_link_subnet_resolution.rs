// SPDX-License-Identifier: CC0-1.0

//! A VPC link's `SubnetIds` are resolved against the service that OWNS subnets.
//!
//! `CreateVpcLink$SubnetIds` is "a list of subnet IDs to include in the VPC link", and a
//! VPC link puts one elastic network interface in each of those subnets. So the rule under
//! test is:
//!
//! * every subnet a create names has to be one the ACCOUNT has, in the Region the request
//!   was signed for, as Amazon EC2 reports it at the time of the call — a create naming one
//!   EC2 holds no subnet for is refused 400 `BadRequestException`, with the sentence EC2
//!   answered wrapped the way API Gateway reports another service's refusal, and persists no
//!   link;
//! * the interfaces of one link live in one VPC, so the subnets a create names have to
//!   agree on their VPC — a list spanning two of them is refused the same way;
//! * a create whose subnets do resolve, in one VPC, is answered 201 and the link it minted
//!   reports those subnets when it is read back.
//!
//! Every subnet id a row names was MINTED by EC2 itself, through
//! `ec2:CreateVpc`/`ec2:CreateSubnet` driven over the AWS SDK on the in-process callback
//! this service's own resolution reaches EC2 through, or read back from the account's
//! default network — so nothing here passes off an id of its own as one the account holds.
//! The two services run on one shared engine, exactly as the router mounts them.
//!
//! The rows differ along what the rule generalises over: how many subnets a request
//! carries, which VPC each belongs to, where in the list the unusable one sits, whether the
//! named subnet was never created or was created and deleted again, whether it is one of
//! the account's default subnets or one a client minted, and whether one subnet is named
//! more than once.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_SUBNET_IDS_RESOLVED;
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::api_lifecycle::{as_str, call_in_region, error_code};

/// The Region every request below is signed for, and the Region the EC2 client asks in.
const REGION: &str = "eu-west-2";

/// The collection URI `CreateVpcLink` is bound to (`POST /v2/vpclinks`), and the one
/// `GetVpcLinks` reads.
const VPC_LINKS: &str = "/v2/vpclinks";

/// A form-valid subnet id no account ever minted: the id form `SubnetIdList` members take
/// (`subnet-` and 17 hexadecimal characters), naming nothing. The recorded create that
/// filed the issue named exactly this shape of id.
const UNKNOWN_SUBNET: &str = "subnet-0000000000bad0bad";

/// Both services on one shared engine — this one, and the Amazon EC2 service that owns
/// subnets — plus an EC2 client per Region over the router's own in-process callback: the
/// very seam the resolution under test reaches EC2 through.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine both services were built from, so a case can assert on the store itself —
    /// what a refused create must NOT have written. `GetVpcLinks` is still a scaffold stub
    /// (its own open issue owns it), so the account's links are counted here rather than
    /// read over the wire.
    engine: Arc<engine::InMemoryEngine>,
    ec2: ::aws_sdk_ec2::Client,
}

/// An EC2 client for `region` over `callback`, the in-process seam this emulator routes on:
/// no socket is opened and the endpoint URL is only the address that transport routes by.
fn ec2_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these tests; the requests still carry the SigV4
    // credential scope, which is what routes them and names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let callback = EmbeddedEmulator::from_router(router.clone());
    Emulator {
        ec2: ec2_client(&callback, REGION),
        engine,
        router,
    }
}

/// One VPC with one subnet per entry of `subnet_cidrs`, created through EC2's own API in
/// the Region `ec2` asks in; the ids are the ones EC2 MINTED.
async fn network(
    ec2: &::aws_sdk_ec2::Client,
    case: &str,
    vpc_cidr: &str,
    subnet_cidrs: &[&str],
) -> Vec<String> {
    let created = ec2
        .create_vpc()
        .cidr_block(vpc_cidr)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 must create {vpc_cidr}: {error:?}"));
    let vpc_id = created
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted"))
        .to_string();

    let mut subnets = Vec::with_capacity(subnet_cidrs.len());
    for cidr in subnet_cidrs {
        let created = ec2
            .create_subnet()
            .vpc_id(&vpc_id)
            .cidr_block(*cidr)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: EC2 must create the subnet {cidr}: {error:?}")
            });
        subnets.push(
            created
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: CreateSubnet must report the subnet it minted")
                })
                .to_string(),
        );
    }
    subnets
}

/// A subnet of the account's DEFAULT network in [`REGION`], read back from EC2 — the
/// network EC2 materialises for every Region without anyone creating one.
///
/// Selected by the flag EC2 reports for exactly those subnets (`DefaultForAz`, "indicates
/// whether this is the default subnet for the Availability Zone"), so a subnet a case
/// created cannot be mistaken for one of them.
async fn default_subnet(ec2: &::aws_sdk_ec2::Client) -> String {
    let described = ec2
        .describe_subnets()
        .send()
        .await
        .expect("EC2 must report the account's subnets");
    let mut reported: Vec<String> = described
        .subnets()
        .iter()
        .filter(|subnet| subnet.default_for_az() == Some(true))
        .filter(|subnet| {
            subnet
                .availability_zone()
                .is_some_and(|zone| zone.starts_with(REGION))
        })
        .filter_map(|subnet| subnet.subnet_id().map(str::to_string))
        .collect();
    reported.sort();
    reported
        .into_iter()
        .next()
        .expect("the account must have a default subnet in this Region")
}

/// How many VPC links the account holds.
fn link_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .vpc_links
        .len()
}

/// One `CreateVpcLink` naming `subnet_ids`.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    subnet_ids: &[String],
) -> (u16, Value) {
    call_in_region(
        router,
        REGION,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": name, "subnetIds": subnet_ids })),
    )
    .await
}

/// Assert the refusal of a create naming `subnet_id`: the sentence EC2 states for an id it
/// holds no subnet for, wrapped the way API Gateway reports a refusal another AWS service
/// answered it — the service, the status, the error code and the id of the refused request.
///
/// The REQUEST ID is that refused request's own, one value per call, so what is asserted is
/// the fixed text around it.
fn assert_relayed_not_found(case: &str, body: &Value, subnet_id: &str) {
    let message = as_str(body, "message");
    let fixed = format!(
        "The subnet ID '{subnet_id}' does not exist (Service: AmazonEC2; Status Code: 400; \
         Error Code: InvalidSubnetID.NotFound; Request ID: "
    );
    assert!(
        message.starts_with(&fixed) && message.ends_with("; Proxy: null)"),
        "case {case:?}: expected {fixed:?} then the refused request's id and \
         \"; Proxy: null)\", got {message:?}"
    );
}

#[tokio::test]
async fn a_create_naming_subnets_of_one_vpc_the_account_has_is_answered_with_the_link() {
    let emulator = emulator();
    let one_vpc = network(&emulator.ec2, "one VPC", "10.11.0.0/16", &["10.11.1.0/24", "10.11.2.0/24"]).await;
    let default = default_subnet(&emulator.ec2).await;

    // (case, the subnets the create names, the `SubnetIds` the stored link must report)
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one subnet a client created",
            vec![one_vpc[0].clone()],
            vec![one_vpc[0].clone()],
        ),
        (
            "two subnets of that same VPC",
            one_vpc.clone(),
            one_vpc.clone(),
        ),
        (
            "the same subnet named twice: one subnet, named twice",
            vec![one_vpc[1].clone(), one_vpc[1].clone()],
            vec![one_vpc[1].clone(), one_vpc[1].clone()],
        ),
        (
            "a subnet of the account's default network, which nobody created",
            vec![default.clone()],
            vec![default.clone()],
        ),
    ];

    for (index, (case, subnet_ids, expected)) in rows.into_iter().enumerate() {
        let (status, body) = create(&emulator.router, &format!("link-{index}"), &subnet_ids).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateVpcLink, got {status} with {body:?}"
        );
        let vpc_link_id = as_str(&body, "vpcLinkId");
        assert!(
            !vpc_link_id.is_empty(),
            "case {case:?}: expected a minted VpcLinkId, got {body:?}"
        );

        // The round trip: the stored link reports the subnets the create named.
        let (status, read_back) = call_in_region(
            &emulator.router,
            REGION,
            "GET",
            &format!("{VPC_LINKS}/{vpc_link_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetVpcLink, got {status} with {read_back:?}"
        );
        assert_eq!(
            read_back.get("subnetIds"),
            Some(&json!(expected)),
            "case {case:?}: expected the stored link to report {expected:?}, got {read_back:?}"
        );
    }
}

#[tokio::test]
async fn a_create_naming_a_subnet_the_account_does_not_have_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let held = network(&emulator.ec2, "held", "10.21.0.0/16", &["10.21.1.0/24"]).await;
    // A subnet that existed and was then deleted: EC2 stops reporting it at once, so it is
    // no longer one the account has.
    let deleted = network(&emulator.ec2, "deleted", "10.23.0.0/16", &["10.23.1.0/24"]).await;
    emulator
        .ec2
        .delete_subnet()
        .subnet_id(&deleted[0])
        .send()
        .await
        .expect("EC2 must delete the subnet it minted");

    // (case, the subnets the create names)
    let rows: Vec<(&str, Vec<String>)> = vec![
        (
            "one id of the right form naming no subnet at all",
            vec![UNKNOWN_SUBNET.to_string()],
        ),
        (
            "an unknown id AFTER a subnet the account has",
            vec![held[0].clone(), UNKNOWN_SUBNET.to_string()],
        ),
        (
            "an unknown id BEFORE a subnet the account has",
            vec![UNKNOWN_SUBNET.to_string(), held[0].clone()],
        ),
        (
            "a subnet the account created and then deleted",
            vec![deleted[0].clone()],
        ),
    ];

    for (index, (case, subnet_ids)) in rows.into_iter().enumerate() {
        let before = link_count(&emulator.engine);
        let (status, body) =
            create(&emulator.router, &format!("refused-{index}"), &subnet_ids).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateVpcLink, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body:?}"
        );
        let after = link_count(&emulator.engine);
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must persist no VPC link, held {before} then \
             {after}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_subnets_span_two_vpcs_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let first = network(
        &emulator.ec2,
        "first VPC",
        "10.31.0.0/16",
        &["10.31.1.0/24", "10.31.2.0/24"],
    )
    .await;
    let second = network(&emulator.ec2, "second VPC", "10.32.0.0/16", &["10.32.1.0/24"]).await;
    let default = default_subnet(&emulator.ec2).await;

    // (case, the subnets the create names) — each list is two or more subnets the account
    // HAS, so the only thing wrong with it is the VPC they do not agree on.
    let rows: Vec<(&str, Vec<String>)> = vec![
        (
            "one subnet from each of two VPCs a client created",
            vec![first[0].clone(), second[0].clone()],
        ),
        (
            "the disagreement in the last position of a longer list",
            vec![first[0].clone(), first[1].clone(), second[0].clone()],
        ),
        (
            "a client's VPC beside the account's default network",
            vec![first[0].clone(), default.clone()],
        ),
    ];

    for (index, (case, subnet_ids)) in rows.into_iter().enumerate() {
        let before = link_count(&emulator.engine);
        let (status, body) =
            create(&emulator.router, &format!("cross-vpc-{index}"), &subnet_ids).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateVpcLink, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body:?}"
        );
        assert_eq!(
            as_str(&body, "message"),
            "All subnets provided should be in the same VPC",
            "case {case:?}: expected the one-VPC sentence, got {body:?}"
        );
        let after = link_count(&emulator.engine);
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must persist no VPC link, held {before} then \
             {after}"
        );
    }
}

#[tokio::test]
async fn the_refusal_of_an_unknown_subnet_names_that_subnet() {
    let emulator = emulator();
    // The message is built from the id the REQUEST sent, so the template is the sentence
    // and the value is each call's own: two rows of the right id form, one per published id
    // length.
    for subnet_id in [UNKNOWN_SUBNET, "subnet-0bad0bad"] {
        let (status, body) = create(
            &emulator.router,
            "names-the-subnet",
            &[subnet_id.to_string()],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {subnet_id:?}: expected 400, got {status} with {body:?}"
        );
        assert_relayed_not_found(subnet_id, &body, subnet_id);
    }
}

#[tokio::test]
async fn a_request_naming_more_subnets_than_one_call_resolves_is_refused_at_the_bound() {
    let emulator = emulator();
    // `MAX_SUBNET_IDS_RESOLVED` is the server-side bound on the work one create costs, so
    // the rows are the bound itself and one past it: AT the bound the list is resolved (and
    // refused for the first identifier the account has no subnet for, which is the first
    // one), PAST it the list is refused for its length instead and nothing is resolved at
    // all. Every id is distinct and of the published form, so the only thing that differs
    // between the rows is how many there are.
    let ids = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("subnet-{index:017x}"))
            .collect()
    };

    let at_bound = ids(MAX_SUBNET_IDS_RESOLVED);
    let (status, body) = create(&emulator.router, "at-the-bound", &at_bound).await;
    assert_eq!(
        status, 400,
        "at the bound: expected 400, got {status} with {body:?}"
    );
    assert_relayed_not_found("at the bound", &body, &at_bound[0]);

    let past_bound = ids(MAX_SUBNET_IDS_RESOLVED + 1);
    let (status, body) = create(&emulator.router, "past-the-bound", &past_bound).await;
    assert_eq!(
        status, 400,
        "past the bound: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "past the bound: expected BadRequestException, got {body:?}"
    );
    let message = as_str(&body, "message");
    assert!(
        message.contains(&MAX_SUBNET_IDS_RESOLVED.to_string())
            && message.contains(&past_bound.len().to_string()),
        "past the bound: expected the refusal to name the bound and what was asked, got \
         {message:?}"
    );
    assert_eq!(
        link_count(&emulator.engine),
        0,
        "neither refused create may persist a VPC link"
    );
}
