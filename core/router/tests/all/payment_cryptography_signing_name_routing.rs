// SPDX-License-Identifier: CC0-1.0

//! Payment Cryptography's control plane and data plane share one SigV4 signing name —
//! each must still be claimed by exactly one service in the assembled router.
//!
//! Both models declare `aws.auth#sigv4` name `payment-cryptography`
//! (`smithy_query(query="service_info")` on either model), while they are two distinct
//! wire services: the control plane is `awsJson1_0` at
//! `controlplane.payment-cryptography`, the data plane `restJson1` at
//! `dataplane.payment-cryptography`. The router resolves a request's routing key from
//! the SigV4 credential scope FIRST, so both services see the same key. A service that
//! claims on the signing name alone therefore claims its peer's traffic too, the router
//! finds two claimants and rejects the request rather than serving it from an arbitrary
//! one — which made the WHOLE Payment Cryptography surface unreachable, every call
//! answering an `AmbiguousService` fault instead of its modelled 200, 4xx or 501.
//!
//! The discriminator is each service's own modelled wire surface, read through the one
//! shared claim rule: the control plane claims a request that NAMES it in
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Operation>` and arrives on the root
//! path its protocol posts to, and the data plane claims the URI path space its
//! `restJson1` operations bind. This suite pins the split in the production router
//! across both services' own traffic, either endpoint host, a loopback endpoint, and
//! both a read and a write. Each case's marker is a string only the claiming service
//! can produce: a member of its own output, its own 501 for an operation it has not
//! implemented, or — for an operation it serves — its own modelled refusal naming what
//! the request asked for.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `payment-cryptography` signing name, which is
/// what BOTH services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/payment-cryptography/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request both services could claim on their signing name: it is
/// served by the service the case names, with this status, and a body carrying this
/// marker — a string ONLY that service's model can produce, so it identifies which
/// crate answered.
struct Case {
    name: &'static str,
    host: &'static str,
    /// The `X-Amz-Target` an `awsJson1_0` control-plane request names its operation in;
    /// empty for a `restJson1` data-plane request, which sends none.
    target: &'static str,
    method: &'static str,
    uri_path: &'static str,
    content_type: &'static str,
    body: &'static str,
    who: &'static str,
    status: u16,
    marker: &'static str,
}

/// A `CreateKey` input: the attributes describe one symmetric key and `Exportable` is
/// the other required member.
const CREATE_KEY_BODY: &str = r#"{"KeyAttributes":{
    "KeyUsage":"TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
    "KeyClass":"SYMMETRIC_KEY",
    "KeyAlgorithm":"AES_128",
    "KeyModesOfUse":{"Encrypt":true,"Decrypt":true}},
    "Exportable":true}"#;

/// A `GetParametersForExport` input: both required members, with the one signing key
/// algorithm that member's documentation allows.
const EXPORT_PARAMETERS_BODY: &str =
    r#"{"KeyMaterialType":"TR34_KEY_BLOCK","SigningKeyAlgorithm":"RSA_2048"}"#;

/// A `GenerateMac` input, each member matching its modelled shape: the key identifier
/// is the `alias/<name>` form `KeyArnOrKeyAliasType` admits, the message data is hex,
/// and the attributes name one `MacAlgorithm`.
const GENERATE_MAC_BODY: &str = r#"{"KeyIdentifier":"alias/routing-check",
    "MessageData":"31323334","GenerationAttributes":{"Algorithm":"ISO9797_ALGORITHM1"}}"#;

/// A `VerifyMac` input: the same members plus the MAC to verify, which is hex too.
const VERIFY_MAC_BODY: &str = r#"{"KeyIdentifier":"alias/routing-check",
    "MessageData":"31323334","Mac":"0123456789abcdef",
    "VerificationAttributes":{"Algorithm":"ISO9797_ALGORITHM1"}}"#;

/// The content type an `awsJson1_0` request carries.
const JSON_1_0: &str = "application/x-amz-json-1.0";
/// The content type a `restJson1` request carries.
const REST_JSON: &str = "application/json";

/// The data plane's answer to the alias above, which no key is bound to: the
/// `ResourceId` of its modelled `ResourceNotFoundException` is the identifier the
/// request named, so a body carrying this came from the data-plane crate serving the
/// operation — the service whose key resolution produced it.
const ROUTING_CHECK_RESOURCE_ID: &str = r#""ResourceId":"alias/routing-check""#;

/// The rows differ along which service owns the request, the endpoint host it was
/// addressed at, the operation's protocol and path, and whether the operation reads or
/// writes.
const CASES: &[Case] = &[
    Case {
        // The minimal repro the issue reports: a control-plane `CreateKey` that never
        // reached a handler because two services claimed `payment-cryptography`.
        name: "CreateKey on the control-plane endpoint host",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.CreateKey",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: CREATE_KEY_BODY,
        who: "the control plane",
        status: 200,
        marker: "KeyCheckValue",
    },
    Case {
        // The same operation on a loopback endpoint: the host does not decide, the
        // target does.
        name: "CreateKey on a loopback endpoint",
        host: "127.0.0.1:9297",
        target: "PaymentCryptographyControlPlane.CreateKey",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: CREATE_KEY_BODY,
        who: "the control plane",
        status: 200,
        marker: "KeyCheckValue",
    },
    Case {
        // …and even on the DATA plane's endpoint host, for the same reason.
        name: "CreateKey on the data-plane endpoint host",
        host: "dataplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.CreateKey",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: CREATE_KEY_BODY,
        who: "the control plane",
        status: 200,
        marker: "KeyCheckValue",
    },
    Case {
        // A control-plane READ, which must reach the same store the writes above wrote
        // into.
        name: "ListKeys",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.ListKeys",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: "{}",
        who: "the control plane",
        status: 200,
        marker: "Keys",
    },
    Case {
        // The account-level read the issue names as a second unreachable operation.
        name: "GetDefaultKeyReplicationRegions",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.GetDefaultKeyReplicationRegions",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: "{}",
        who: "the control plane",
        status: 200,
        marker: "EnabledReplicationRegions",
    },
    Case {
        // …and the third, which mints an export token.
        name: "GetParametersForExport",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.GetParametersForExport",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: EXPORT_PARAMETERS_BODY,
        who: "the control plane",
        status: 200,
        marker: "ExportToken",
    },
    Case {
        // A control-plane refusal: an operation the model declares but this emulator
        // has not implemented answers its own 501, which only the control plane's
        // routing table can resolve.
        name: "a control-plane operation that is still a stub",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "PaymentCryptographyControlPlane.DeleteResourcePolicy",
        method: "POST",
        uri_path: "/",
        content_type: JSON_1_0,
        body: r#"{"ResourceArn":"arn:aws:payment-cryptography:us-east-1:000000000000:key/abcdefghij123456"}"#,
        who: "the control plane",
        status: 501,
        marker: "operation DeleteResourcePolicy is scaffolded",
    },
    Case {
        // The data plane's own traffic, under the same signing name and with no
        // `X-Amz-Target` at all: it must be claimed by the data-plane crate, which
        // SERVES this operation — so what identifies the claimant is the data plane's
        // own modelled answer for a `KeyIdentifier` that names no key: its
        // `ResourceNotFoundException`, whose `ResourceId` echoes the identifier this
        // body asked for. The control plane binds no such path and could not produce
        // it.
        name: "GenerateMac on the data-plane endpoint host",
        host: "dataplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "",
        method: "POST",
        uri_path: "/mac/generate",
        content_type: REST_JSON,
        body: GENERATE_MAC_BODY,
        who: "the data plane",
        status: 404,
        marker: ROUTING_CHECK_RESOURCE_ID,
    },
    Case {
        // The same path space on the CONTROL plane's endpoint host: the path decides,
        // not the host.
        name: "VerifyMac on the control-plane endpoint host",
        host: "controlplane.payment-cryptography.us-east-1.amazonaws.com",
        target: "",
        method: "POST",
        uri_path: "/mac/verify",
        content_type: REST_JSON,
        body: VERIFY_MAC_BODY,
        who: "the data plane",
        status: 404,
        marker: ROUTING_CHECK_RESOURCE_ID,
    },
    Case {
        // A second data-plane path space, on a loopback endpoint.
        name: "GeneratePinData on a loopback endpoint",
        host: "127.0.0.1:9297",
        target: "",
        method: "POST",
        uri_path: "/pindata/generate",
        content_type: REST_JSON,
        body: r#"{"GenerationKeyIdentifier":"alias/routing-check",
            "EncryptionKeyIdentifier":"alias/routing-check",
            "GenerationAttributes":{"VisaPin":{"PinVerificationKeyIndex":1}},
            "PinBlockFormat":"ISO_FORMAT_0","PrimaryAccountNumber":"171234567890123"}"#,
        who: "the data plane",
        status: 501,
        marker: "operation GeneratePinData is scaffolded",
    },
];

#[test]
fn payment_cryptography_control_and_data_planes_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("payment-cryptography-routing".to_string())
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

                for case in CASES {
                    let mut builder = http::Request::builder()
                        .method(case.method)
                        .uri(format!("https://{}{}", case.host, case.uri_path))
                        .header("host", case.host)
                        .header("content-type", case.content_type)
                        .header("authorization", AUTHORIZATION);
                    if !case.target.is_empty() {
                        builder = builder.header("x-amz-target", case.target);
                    }
                    let request = builder
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
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
