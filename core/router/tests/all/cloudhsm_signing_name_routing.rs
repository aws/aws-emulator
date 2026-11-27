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
//! Both speak `awsJson1_1`, so a request of that wire names its service out of band in
//! `X-Amz-Target` (`BaldrApiService.<Operation>` for V2,
//! `CloudHsmFrontendService.<Operation>` for Classic). The V2 model also declares Smithy
//! RPC v2 CBOR, whose requests carry no `X-Amz-Target` at all and name the service in the
//! path instead (`POST /service/BaldrApiService/operation/<Operation>`). The service a
//! request's own RPC wire form NAMES is therefore the discriminator, read by the one shared
//! rule each crate's `can_handle` applies to its own shape name, so the split is decided
//! the same way on both sides and in both of the wires this endpoint serves. This suite
//! pins it in the production router across both services' own traffic, both routing tiers
//! (credential scope and `X-Amz-Target`), either endpoint host and either wire.
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

/// Which wire a case's request arrives on. Both services declare the first; only CloudHSM
/// V2 declares the second.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wire {
    /// `POST /`, the operation in `X-Amz-Target`, the body as JSON.
    AwsJson,
    /// Smithy RPC v2 CBOR: the service and the operation in the path, the body as CBOR,
    /// `smithy-protocol: rpc-v2-cbor`, and no `X-Amz-Target` at all.
    RpcV2Cbor,
}

/// What must happen to a request both services could claim on their signing name: it is
/// served by the service the case names, with this status, and a body carrying this
/// marker — a string ONLY that service's model can produce, so it identifies which crate
/// answered.
struct Case {
    name: &'static str,
    host: &'static str,
    /// The service and operation the request names, `<Service>.<Operation>`, as the
    /// `X-Amz-Target` of an awsJson request spells it. A CBOR case addresses the same two
    /// names in its path.
    target: &'static str,
    wire: Wire,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
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
        wire: Wire::AwsJson,
        signed: false,
        body: "{}",
        who: "CloudHSM Classic",
        status: 200,
        marker: "HsmList",
    },
    Case {
        // The V2 model declares RPC v2 CBOR beside awsJson1_1, and such a request carries
        // no `X-Amz-Target`: it names the service in its PATH. The signing name is all the
        // credential scope carries, so the claim has to read that form — a write first.
        name: "V2 CreateCluster over the RPC v2 CBOR wire",
        host: "cloudhsmv2.us-east-1.amazonaws.com",
        target: "BaldrApiService.CreateCluster",
        wire: Wire::RpcV2Cbor,
        signed: true,
        body: CREATE_CLUSTER_BODY,
        who: "CloudHSM V2",
        status: 200,
        marker: "SubnetMapping",
    },
    Case {
        // …and a READ over the same wire, which reaches the store every case above wrote
        // into: one service, one store, either wire.
        name: "V2 DescribeClusters over the RPC v2 CBOR wire",
        host: "cloudhsmv2.us-east-1.amazonaws.com",
        target: "BaldrApiService.DescribeClusters",
        wire: Wire::RpcV2Cbor,
        signed: true,
        body: "{}",
        who: "CloudHSM V2",
        status: 200,
        marker: "Clusters",
    },
    Case {
        // The same wire on CLASSIC's endpoint host: the host does not decide, the service
        // the path names does.
        name: "V2 DescribeClusters over RPC v2 CBOR on the Classic endpoint host",
        host: "cloudhsm.us-east-1.amazonaws.com",
        target: "BaldrApiService.DescribeClusters",
        wire: Wire::RpcV2Cbor,
        signed: true,
        body: "{}",
        who: "CloudHSM V2",
        status: 200,
        marker: "Clusters",
    },
    Case {
        // A CBOR path naming CLASSIC is Classic's own traffic and is claimed by Classic —
        // not served by the peer that does speak that wire. Classic's model declares only
        // awsJson1_1, so Classic refuses it as the request it is for that wire: one
        // carrying no `X-Amz-Target` to name an operation with. What matters here is that
        // exactly one service answered, and that it was this one.
        name: "a CBOR path naming Classic is claimed by Classic, which does not speak it",
        host: "cloudhsm.us-east-1.amazonaws.com",
        target: "CloudHsmFrontendService.ListHsms",
        wire: Wire::RpcV2Cbor,
        signed: true,
        body: "{}",
        who: "CloudHSM Classic",
        status: 400,
        marker: "missing required header `x-amz-target`",
    },
];

/// The header the RPC v2 CBOR specification requires on every request and response of that
/// wire, and the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that wire carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// `value` as the CBOR item carrying the same members, every container declaring its length
/// up front: RFC 8949 major type 0 (unsigned), 3 (text), 4 (array), 5 (map) and the two
/// simple values, which is every item this suite's inputs are built from.
///
/// Written here rather than with a CBOR crate because the router's test manifest is
/// re-rendered whenever a service is added and so carries no dependency of this suite's own.
fn cbor_bytes(value: &serde_json::Value) -> Vec<u8> {
    fn header(out: &mut Vec<u8>, major: u8, argument: u64) {
        let high = major << 5;
        match argument {
            small if small < 24 => out.push(high | small as u8),
            byte if byte <= u64::from(u8::MAX) => {
                out.push(high | 24);
                out.push(byte as u8);
            }
            short if short <= u64::from(u16::MAX) => {
                out.push(high | 25);
                out.extend_from_slice(&(short as u16).to_be_bytes());
            }
            word => {
                out.push(high | 27);
                out.extend_from_slice(&word.to_be_bytes());
            }
        }
    }

    fn write(out: &mut Vec<u8>, value: &serde_json::Value) {
        match value {
            serde_json::Value::Null => out.push(0xf6),
            serde_json::Value::Bool(flag) => out.push(if *flag { 0xf5 } else { 0xf4 }),
            serde_json::Value::Number(number) => header(
                out,
                0,
                number
                    .as_u64()
                    .unwrap_or_else(|| panic!("this suite's inputs carry whole numbers only")),
            ),
            serde_json::Value::String(text) => {
                header(out, 3, text.len() as u64);
                out.extend_from_slice(text.as_bytes());
            }
            serde_json::Value::Array(elements) => {
                header(out, 4, elements.len() as u64);
                for element in elements {
                    write(out, element);
                }
            }
            serde_json::Value::Object(members) => {
                header(out, 5, members.len() as u64);
                for (name, held) in members {
                    header(out, 3, name.len() as u64);
                    out.extend_from_slice(name.as_bytes());
                    write(out, held);
                }
            }
        }
    }

    let mut out = Vec::new();
    write(&mut out, value);
    out
}

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
                    // The two wires address the SAME service and operation, each in its
                    // own place: `X-Amz-Target` on one, the path on the other, which
                    // carries no target header at all.
                    let (service, operation) = case
                        .target
                        .split_once('.')
                        .expect("a target names a service and an operation");
                    let mut builder = http::Request::builder()
                        .method("POST")
                        .header("host", case.host);
                    builder = match case.wire {
                        Wire::AwsJson => builder
                            .uri(format!("https://{}/", case.host))
                            .header("content-type", "application/x-amz-json-1.1")
                            .header("x-amz-target", case.target),
                        Wire::RpcV2Cbor => builder
                            .uri(format!(
                                "https://{}/service/{service}/operation/{operation}",
                                case.host
                            ))
                            .header("content-type", CBOR_MEDIA_TYPE)
                            .header("accept", CBOR_MEDIA_TYPE)
                            .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1),
                    };
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    let payload = match case.wire {
                        Wire::AwsJson => body.into_bytes(),
                        Wire::RpcV2Cbor => cbor_bytes(
                            &serde_json::from_str(&body).expect("the case's input is JSON"),
                        ),
                    };
                    let request = builder
                        .body(bytes::Bytes::from(payload))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    // The marker is read out of the bytes as they arrived: a CBOR map
                    // writes its member names as text, so the member that identifies the
                    // answering service is as readable here as in a JSON body.
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
