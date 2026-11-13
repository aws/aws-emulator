// SPDX-License-Identifier: CC0-1.0

//! AWS CloudHSM Classic and AWS CloudHSM V2 share one SigV4 signing name — each must
//! still be claimed by exactly one service in the assembled router.
//!
//! Both services declare `aws.auth#sigv4` name `cloudhsm`
//! (`smithy_query(query="service_info")` on either model; their endpoint prefixes are
//! `cloudhsm` and `cloudhsmv2`), and the router resolves a request's routing key from the
//! SigV4 credential scope FIRST, so both see the same key. A service that claims on the
//! signing name alone therefore claims its peer's traffic too, the router finds two
//! claimants and rejects the request rather than serving it from an arbitrary one — which
//! made the WHOLE CloudHSM surface unreachable, every call answering a server fault
//! instead of its modelled 200 or 4xx.
//!
//! Both speak `awsJson1_1`, so every request names its service out of band in
//! `X-Amz-Target` (`BaldrApiService.<Operation>` for V2,
//! `CloudHsmFrontendService.<Operation>` for Classic) — that prefix is the discriminator
//! each crate's `can_handle` reads through the one shared reader, so the split is decided
//! the same way on both sides. This suite pins it in the production router across both
//! services' own traffic, both routing tiers (credential scope and `X-Amz-Target`) and
//! either endpoint host.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.
//!
//! The V2 create below is placed in a subnet EC2 minted on the same engine, because
//! CloudHSM V2 resolves `SubnetIds` against the account's EC2 inventory: an id this suite
//! made up names no subnet and would be refused for a reason that has nothing to do with
//! routing.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

use crate::query_wire::{element, form_encode};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `cloudhsm` signing name, which is what BOTH
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudhsm/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request both services could claim on their signing name: it is
/// served by the service the case names, with this status, and a body carrying this
/// marker — a string ONLY that service's model can produce, so it identifies which crate
/// answered.
struct Case {
    name: &'static str,
    host: &'static str,
    target: &'static str,
    /// Whether to sign for `cloudhsm`. A case with `false` carries no credential scope,
    /// so the router falls through to the `X-Amz-Target` routing tier.
    signed: bool,
    body: &'static str,
    who: &'static str,
    status: u16,
    marker: &'static str,
}

/// A `CreateCluster` input: `HsmType` is the type a cluster can be created with and
/// `SubnetIds` names one subnet. `{subnet}` is replaced with the id EC2 minted for this
/// run (see the module header), so the create is placed in a subnet the account has.
const CREATE_CLUSTER_BODY: &str = r#"{"HsmType":"hsm2m.medium","SubnetIds":["{subnet}"]}"#;

/// The placeholder [`CREATE_CLUSTER_BODY`] carries for that id.
const SUBNET_PLACEHOLDER: &str = "{subnet}";

/// The EC2 API version an ec2Query body names.
const EC2_API_VERSION: &str = "2016-11-15";

/// A Classic `CreateHsm` input, each member matching its modelled pattern: `SubnetId` is
/// `^subnet-[0-9a-f]{8}$`, `IamRoleArn` an IAM role ARN, and `SubscriptionType` the one
/// modelled value.
const CREATE_HSM_BODY: &str = r#"{"SubnetId":"subnet-0a1b2c3d",
    "IamRoleArn":"arn:aws:iam::000000000000:role/CloudHsmRole",
    "SshKey":"ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ== routing-check",
    "SubscriptionType":"PRODUCTION"}"#;

/// The rows differ along which service owns the request, the routing tier its key comes
/// from (credential scope vs `X-Amz-Target`), the endpoint host, and whether the
/// operation reads or writes.
const CASES: &[Case] = &[
    Case {
        // The minimal repro the issue reports: a V2 `CreateCluster` that never reached a
        // handler because two services claimed `cloudhsm`.
        name: "V2 CreateCluster on the V2 endpoint host",
        host: "cloudhsmv2.us-east-1.amazonaws.com",
        target: "BaldrApiService.CreateCluster",
        signed: true,
        body: CREATE_CLUSTER_BODY,
        who: "CloudHSM V2",
        status: 200,
        marker: "SubnetMapping",
    },
    Case {
        // The same operation on a loopback endpoint: the host does not decide, the
        // target does.
        name: "V2 CreateCluster on a loopback endpoint",
        host: "127.0.0.1:9297",
        target: "BaldrApiService.CreateCluster",
        signed: true,
        body: CREATE_CLUSTER_BODY,
        who: "CloudHSM V2",
        status: 200,
        marker: "SubnetMapping",
    },
    Case {
        // …and even on CLASSIC's endpoint host, for the same reason.
        name: "V2 CreateCluster on the Classic endpoint host",
        host: "cloudhsm.us-east-1.amazonaws.com",
        target: "BaldrApiService.CreateCluster",
        signed: true,
        body: CREATE_CLUSTER_BODY,
        who: "CloudHSM V2",
        status: 200,
        marker: "SubnetMapping",
    },
    Case {
        // A V2 READ, which must reach the same store the writes above wrote into.
        name: "V2 DescribeClusters",
        host: "cloudhsmv2.us-east-1.amazonaws.com",
        target: "BaldrApiService.DescribeClusters",
        signed: true,
        body: "{}",
        who: "CloudHSM V2",
        status: 200,
        marker: "Clusters",
    },
    Case {
        // An UNSIGNED V2 request: with no credential scope the router's next tier is
        // `X-Amz-Target`, whose prefix resolves to this service's own key, so the claim
        // must still split the same way.
        name: "V2 DescribeClusters routed by X-Amz-Target alone",
        host: "127.0.0.1:9297",
        target: "BaldrApiService.DescribeClusters",
        signed: false,
        body: "{}",
        who: "CloudHSM V2",
        status: 200,
        marker: "Clusters",
    },
    Case {
        // Classic's own traffic, under the same signing name: it must still be served by
        // the Classic crate.
        name: "Classic ListHsms on the Classic endpoint host",
        host: "cloudhsm.us-east-1.amazonaws.com",
        target: "CloudHsmFrontendService.ListHsms",
        signed: true,
        body: "{}",
        who: "CloudHSM Classic",
        status: 200,
        marker: "HsmList",
    },
    Case {
        name: "Classic ListHsms on the V2 endpoint host",
        host: "cloudhsmv2.us-east-1.amazonaws.com",
        target: "CloudHsmFrontendService.ListHsms",
        signed: true,
        body: "{}",
        who: "CloudHSM Classic",
        status: 200,
        marker: "HsmList",
    },
    Case {
        // A Classic WRITE, so dispatch is shown reaching the handler that writes and not
        // only the readers. `HsmArn` is an output member of this operation alone.
        name: "Classic CreateHsm",
        host: "cloudhsm.us-east-1.amazonaws.com",
        target: "CloudHsmFrontendService.CreateHsm",
        signed: true,
        body: CREATE_HSM_BODY,
        who: "CloudHSM Classic",
        status: 200,
        marker: "HsmArn",
    },
    Case {
        // The unsigned Classic request, for the same reason as its V2 counterpart.
        name: "Classic ListHsms routed by X-Amz-Target alone",
        host: "127.0.0.1:9297",
        target: "CloudHsmFrontendService.ListHsms",
        signed: false,
        body: "{}",
        who: "CloudHSM Classic",
        status: 200,
        marker: "HsmList",
    },
];

/// One subnet of the account, as EC2 itself mints it: a VPC of this suite's own, and one
/// subnet in it. Driven over EC2's own ec2Query wire on the same router, so the id is one
/// EC2 handed out rather than one this suite made up.
async fn ec2_subnet(router: &smithy_server_runtime::MultiServiceRouter) -> String {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.90.0.0/16")]).await,
        "vpcId",
    );
    element(
        &ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", "10.90.1.0/24"),
                ("AvailabilityZone", "us-east-1a"),
            ],
        )
        .await,
        "subnetId",
    )
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(
    router: &smithy_server_runtime::MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "ec2.us-east-1.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/ec2/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef",
        )
        .body(bytes::Bytes::from(body))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body().as_ref()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

#[test]
fn cloudhsm_classic_and_v2_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("cloudhsm-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these cases are about which service claims a request.
                router.set_request_authentication(false);

                // The subnet the V2 create is placed in, minted by EC2 itself on this
                // same engine — the only kind of id CloudHSM V2 can resolve.
                let subnet = ec2_subnet(&router).await;

                for case in CASES {
                    let body = case.body.replace(SUBNET_PLACEHOLDER, &subnet);
                    let mut builder = http::Request::builder()
                        .method("POST")
                        .uri(format!("https://{}/", case.host))
                        .header("host", case.host)
                        .header("content-type", "application/x-amz-json-1.1")
                        .header("x-amz-target", case.target);
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    let request = builder
                        .body(bytes::Bytes::from(body))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert_eq!(
                        status, case.status,
                        "case {:?}: expected HTTP {} from {}, got {status} {body}",
                        case.name, case.status, case.who
                    );
                    assert!(
                        body.contains(case.marker),
                        "case {:?}: expected a body from {} carrying {:?}, got {status} {body}",
                        case.name,
                        case.who,
                        case.marker
                    );
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
