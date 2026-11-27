// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through:
//! `awsJson1_1`, `POST /` with `X-Amz-Target: TrentService.<Operation>`, dispatched
//! by the same multi-service router the emulator boots.
//!
//! It holds ONLY the plumbing — build a router, call an operation, read a member.
//! Every assertion lives in the suite that makes it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::key_arn;
use service_com_amazonaws_kms_trentservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for. Keys are regional, so a case that is about
/// regional scoping names its own.
pub(crate) const REGION: &str = "us-east-1";

/// The access-key id every call below is signed with unless a case names another: one
/// neither STS nor IAM holds a record of, so it resolves to the account's own ambient
/// identity — the position an AWS account root credential is in, which no identity-based
/// policy governs.
pub(crate) const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// A router with only this service mounted, over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// [`router`], together with the engine it was assembled over, for a case that has
/// to seed a record no implemented operation creates yet.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// [`router`], with the IAM and STS services mounted beside this one over the SAME engine
/// — the assembly a case about the identity-based half of an authorization decision needs,
/// since the principal a request speaks for and the documents in force on it are those two
/// services' own state.
pub(crate) fn router_with_identity_services() -> MultiServiceRouter {
    let (mut router, engine) = router_with_engine();
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// Call one operation in `region` as [`ACCOUNT_KEY`] and return its HTTP status and JSON
/// body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_as(router, region, ACCOUNT_KEY, operation, body).await
}

/// [`call`], signed with `access_key_id` in the SigV4 credential scope — which is where a
/// service reads the caller's access key id from, so this is what makes a call speak for
/// one principal rather than another.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    region: &str,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let response = respond(router, region, access_key_id, operation, body).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// [`call_as`], answering the response as the CLIENT receives it — status, headers and
/// body — for a case about what a response reports beside its modelled members.
pub(crate) async fn respond(
    router: &MultiServiceRouter,
    region: &str,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> http::Response<bytes::Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{region}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    router.dispatch(request).await
}

/// Call one operation in [`REGION`], requiring success.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok_in(router, REGION, operation, body).await
}

/// Call one operation in [`REGION`] as `access_key_id`, requiring success.
pub(crate) async fn ok_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call_as(router, REGION, access_key_id, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body} as {access_key_id}: expected HTTP 200, got {status} with \
         {payload}"
    );
    payload
}

/// Call one operation in `region`, requiring success.
pub(crate) async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// Call one operation in [`REGION`], requiring a failure, and report its status and
/// wire error code.
pub(crate) async fn failure(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    failure_in(router, REGION, operation, body).await
}

/// [`failure`], in `region`.
pub(crate) async fn failure_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    let (status, payload) = call(router, region, operation, body).await;
    assert_ne!(
        status, 200,
        "case {operation} {body}: expected a failure, got HTTP 200 with {payload}"
    );
    (status, error_code(&payload))
}

/// The text `GetKeyPolicy` (and `ReplicateKey`'s `ReplicaPolicy`) reports for the key
/// policy document `submitted`: KMS renders an attached policy into its own layout
/// rather than echoing the caller's bytes, so a suite that attaches a document and reads
/// it back compares against this. WHAT that layout is — the two-space indentation, the
/// ` : ` separator, the inline arrays — is pinned against text written out by hand in
/// [`super::read_projection_fidelity`]; here it is only the reference point a round trip
/// is measured from.
pub(crate) fn reported_policy(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}

/// The wire error code (`__type`) of an error body, without its namespace.
pub(crate) fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error body carries, which is a separate member from the `__type`
/// [`error_code`] reports — and is absent on the refusal classes AWS words without one.
pub(crate) fn error_message(payload: &Value) -> String {
    payload["message"]
        .as_str()
        .or_else(|| payload["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Call an operation that must fail, and report its status, wire error code and message.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, payload) = call(router, region, operation, body).await;
    assert_ne!(
        status, 200,
        "case {operation} {body}: expected a failure, got HTTP 200 with {payload}"
    );
    (status, error_code(&payload), error_message(&payload))
}

/// Call an operation that must fail as `access_key_id`, and report its status, wire error
/// code and message.
pub(crate) async fn refused_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, payload) = call_as(router, REGION, access_key_id, operation, body).await;
    assert_ne!(
        status, 200,
        "case {operation} {body} as {access_key_id}: expected a failure, got HTTP 200 with \
         {payload}"
    );
    (status, error_code(&payload), error_message(&payload))
}

/// Create an IAM user through IAM's OWN operations — with `policy` as its inline policy
/// when one is given — and answer the access-key id of a long-term key minted for it,
/// beside the user's ARN.
///
/// A principal made this way is one the IAM store really holds, so the identity-based
/// documents this service reads for it are the ones IAM wrote.
pub(crate) async fn iam_user(
    router: &MultiServiceRouter,
    user_name: &str,
    policy: Option<&str>,
) -> (String, String) {
    let body = iam_query(router, "CreateUser", &[("UserName", user_name.to_string())]).await;
    let arn = xml_element(&body, "Arn")
        .unwrap_or_else(|| panic!("CreateUser must answer the user Arn, got {body}"));
    if let Some(policy) = policy {
        iam_query(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    let body = iam_query(router, "CreateAccessKey", &[("UserName", user_name.to_string())]).await;
    let access_key_id = xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"));
    (access_key_id, arn)
}

/// One awsQuery call to IAM as [`ACCOUNT_KEY`], requiring success, with its response body.
async fn iam_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(status, 200, "iam:{action} must succeed, got {status} {body}");
    body
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Wrap `material` under the public wrapping key `GetParametersForImport` answered, with
/// the wrapping algorithm the parameters were asked for — what a caller importing key
/// material does before calling `ImportKeyMaterial`.
///
/// The two `RSAES_OAEP_*` algorithms encrypt the material directly under the RSA public
/// key; the two `RSA_AES_KEY_WRAP_*` ones encrypt a freshly generated 256-bit AES key
/// under it and wrap the material with that AES key, then concatenate the two — which is
/// the only form long material (an RSA private key) fits in.
pub(crate) fn wrap(
    case: &str,
    public_key: &[u8],
    wrapping_algorithm: &str,
    material: &[u8],
) -> Vec<u8> {
    use aws_lc_rs::key_wrap::KeyWrapPadded;

    let (algorithm, aes_key_wrap) = match wrapping_algorithm {
        "RSAES_OAEP_SHA_256" => (&aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256, false),
        "RSAES_OAEP_SHA_1" => (&aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1, false),
        "RSA_AES_KEY_WRAP_SHA_256" => (&aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256, true),
        "RSA_AES_KEY_WRAP_SHA_1" => (&aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1, true),
        other => panic!("case {case}: {other} is not a wrapping algorithm this test uses"),
    };
    let oaep = oaep_wrapping_key(case, public_key);

    // What the RSA public key encrypts: the material itself, or the ephemeral AES key
    // that wrapped it.
    let mut aes_wrapped = Vec::new();
    let mut aes_key = Vec::new();
    if aes_key_wrap {
        aes_key = service_com_amazonaws_kms_trentservice::state::random_bytes(32)
            .unwrap_or_else(|| panic!("case {case}: the platform produces entropy"));
        let kek = aws_lc_rs::key_wrap::AesKek::new(&aws_lc_rs::key_wrap::AES_256, &aes_key)
            .unwrap_or_else(|error| panic!("case {case}: the AES key wraps: {error}"));
        // The padded key wrap grows the material by at most one block plus the 8-byte
        // integrity check value.
        let mut wrapped = vec![0u8; material.len() + 16];
        let len = kek
            .wrap_with_padding(material, &mut wrapped)
            .unwrap_or_else(|error| panic!("case {case}: the material wraps: {error}"))
            .len();
        wrapped.truncate(len);
        aes_wrapped = wrapped;
    }
    let sealed: &[u8] = if aes_key_wrap { &aes_key } else { material };
    let mut wrapped = vec![0u8; oaep.key_size_bytes()];
    let len = oaep
        .encrypt(algorithm, sealed, &mut wrapped, None)
        .unwrap_or_else(|error| panic!("case {case}: the material wraps: {error}"))
        .len();
    wrapped.truncate(len);
    wrapped.extend_from_slice(&aes_wrapped);
    wrapped
}

/// A blob as awsJson1_1 carries it.
pub(crate) fn b64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

/// The bytes behind a blob member.
pub(crate) fn unb64(case: &str, payload: &Value, member: &str) -> Vec<u8> {
    let encoded = payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected {member} in {payload}"));
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case}: {member} is not base64: {error}"))
}

/// The RSA wrapping key an X.509 `SubjectPublicKeyInfo` DER public key carries, which
/// is what a caller does with the `PublicKey` member before wrapping key material.
pub(crate) fn oaep_wrapping_key(
    case: &str,
    public_key: &[u8],
) -> aws_lc_rs::rsa::OaepPublicEncryptingKey {
    let parsed = aws_lc_rs::rsa::PublicEncryptingKey::from_der(public_key)
        .unwrap_or_else(|error| panic!("case {case}: the public key is DER: {error}"));
    aws_lc_rs::rsa::OaepPublicEncryptingKey::new(parsed)
        .unwrap_or_else(|error| panic!("case {case}: the public key wraps: {error}"))
}

/// The ENCAPSULATION key of the ML-KEM decapsulation key the DER PKCS#8 `PrivateKeyInfo`
/// `private` carries, as a DER-encoded X.509 `SubjectPublicKeyInfo` naming
/// `object_identifier`.
///
/// The public half is read out of the private key as FIPS 203 §7.1 lays a decapsulation
/// key out — `dk = dk_PKE ‖ ek ‖ H(ek) ‖ z`, whose section lengths follow from the
/// parameter set's own `dk` length (`dk` is `768k + 96` bytes and `ek` is `384k + 32`) —
/// and then CONFIRMED through `aws-lc-rs`, the library that implements the mechanism:
/// both halves are re-imported there and a secret encapsulated under the public half is
/// recovered by the private one. Bytes that are not a key pair of this parameter set
/// cannot be imported, or do not round trip, and the case fails.
pub(crate) fn ml_kem_public_key_spki(
    case: &str,
    algorithm: &'static aws_lc_rs::kem::Algorithm<aws_lc_rs::kem::AlgorithmId>,
    object_identifier: &str,
    private: &[u8],
) -> Vec<u8> {
    use pkcs8::der::Decode as _;
    use spki::der::Encode as _;

    let info = pkcs8::PrivateKeyInfoRef::from_der(private)
        .unwrap_or_else(|error| panic!("case {case}: not a PKCS#8 PrivateKeyInfo: {error}"));
    assert_eq!(
        info.algorithm.oid.to_string(),
        object_identifier,
        "case {case}: the private key must name the parameter set's algorithm identifier, \
         got {}",
        info.algorithm.oid
    );
    // `privateKey` carries the `expandedKey` alternative of `ML-KEM-PrivateKey`: the
    // decapsulation key inside an OCTET STRING
    // (draft-ietf-lamps-kyber-certificates).
    let expanded: &pkcs8::der::asn1::OctetStringRef =
        pkcs8::der::Decode::from_der(info.private_key.as_bytes()).unwrap_or_else(|error| {
            panic!("case {case}: the privateKey must carry an ML-KEM private key: {error}")
        });
    let decapsulation_key_bytes = expanded.as_bytes();
    let decapsulation_key = aws_lc_rs::kem::DecapsulationKey::new(algorithm, decapsulation_key_bytes)
        .unwrap_or_else(|error| {
            panic!(
                "case {case}: not a decapsulation key of this parameter set ({} bytes): {error}",
                decapsulation_key_bytes.len()
            )
        });

    // The parameter set's rank, from the one length that states it.
    let rank = (decapsulation_key_bytes.len().saturating_sub(96)) / 768;
    let decapsulation_key_pke_len = 384 * rank;
    let encapsulation_key_len = 384 * rank + 32;
    assert_eq!(
        decapsulation_key_bytes.len(),
        768 * rank + 96,
        "case {case}: a decapsulation key is 768k + 96 bytes, got {}",
        decapsulation_key_bytes.len()
    );
    let encapsulation_key_bytes = &decapsulation_key_bytes
        [decapsulation_key_pke_len..decapsulation_key_pke_len + encapsulation_key_len];

    // The two halves belong to one pair: a secret established under the public half is
    // the one the private half recovers.
    let encapsulation_key = aws_lc_rs::kem::EncapsulationKey::new(algorithm, encapsulation_key_bytes)
        .unwrap_or_else(|error| {
            panic!("case {case}: not an encapsulation key of this parameter set: {error}")
        });
    let (ciphertext, established) = encapsulation_key
        .encapsulate()
        .unwrap_or_else(|error| panic!("case {case}: {error}"));
    let recovered = decapsulation_key
        .decapsulate(aws_lc_rs::kem::Ciphertext::from(ciphertext.as_ref()))
        .unwrap_or_else(|error| panic!("case {case}: {error}"));
    assert_eq!(
        recovered.as_ref(),
        established.as_ref(),
        "case {case}: the public half read out of the private key must establish the secret \
         that private key recovers"
    );

    let subject_public_key_info = spki::SubjectPublicKeyInfoOwned {
        algorithm: spki::AlgorithmIdentifierOwned {
            oid: spki::ObjectIdentifier::new(object_identifier)
                .unwrap_or_else(|error| panic!("case {case}: {error}")),
            parameters: None,
        },
        subject_public_key: spki::der::asn1::BitString::from_bytes(encapsulation_key_bytes)
            .unwrap_or_else(|error| panic!("case {case}: {error}")),
    };
    subject_public_key_info
        .to_der()
        .unwrap_or_else(|error| panic!("case {case}: {error}"))
}

/// Create a key and return its `KeyMetadata`.
pub(crate) async fn create_key(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateKey", request).await["KeyMetadata"].clone()
}

/// The `KeyMetadata` `DescribeKey` reports for an identifier.
pub(crate) async fn describe(router: &MultiServiceRouter, identifier: &str) -> Value {
    describe_in(router, REGION, identifier).await
}

/// [`describe`], through the endpoint of `region` — which is a different KEY for a
/// multi-Region key set, since every Region holds its own member of the set.
pub(crate) async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Value {
    ok_in(router, region, "DescribeKey", &json!({ "KeyId": identifier })).await["KeyMetadata"]
        .clone()
}

/// Replicate the multi-Region key `key_id` into each of `regions`, which is what
/// leaves a replica key of the set at each of those Regions' endpoints, and wait for
/// each replica to leave the transient `Creating` state a replicate starts it in — so a
/// case about what the SET does next reads the state the replicate left rather than one
/// it was still owed. What a replicate itself answers is asserted by
/// `tests/all/replicate_key.rs`.
pub(crate) async fn replicate_into(
    router: &MultiServiceRouter,
    case: &str,
    key_id: &str,
    regions: &[&str],
) {
    for region in regions {
        let answer = ok_in(
            router,
            REGION,
            "ReplicateKey",
            &json!({"KeyId": key_id, "ReplicaRegion": region}),
        )
        .await;
        let metadata = answer["ReplicaKeyMetadata"].clone();
        assert_eq!(
            metadata["Arn"],
            json!(key_arn(region, key_id)),
            "case {case}: the replica in {region} is named by the key ARN of its own Region, \
             got {metadata}"
        );
        settle_key_state(router, region, case, key_id, "Creating").await;
    }
}

/// The `MultiRegionConfiguration` of the multi-Region key set `key_id` as the endpoint
/// of `region` reports it: which member of the set that endpoint serves, the Region
/// holding the set's primary key, and the Regions holding a replica key, sorted. Every
/// member it names is checked to be named by the key ARN of its OWN Region, which is
/// how a set of one key id spans Regions.
pub(crate) async fn topology_at(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    key_id: &str,
) -> (String, String, Vec<String>) {
    let metadata = describe_in(router, region, key_id).await;
    let configuration = metadata["MultiRegionConfiguration"].clone();
    let key_type = text(case, &configuration, "MultiRegionKeyType");
    let primary = configuration["PrimaryKey"].clone();
    let primary_region = text(case, &primary, "Region");
    assert_eq!(
        primary["Arn"],
        json!(key_arn(&primary_region, key_id)),
        "case {case}: the primary key is named by the key ARN of its own Region, got {primary}"
    );
    let mut replicas: Vec<String> = configuration["ReplicaKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ReplicaKeys in {configuration}"))
        .iter()
        .map(|replica| {
            let replica_region = text(case, replica, "Region");
            assert_eq!(
                replica["Arn"],
                json!(key_arn(&replica_region, key_id)),
                "case {case}: a replica key is named by the key ARN of its own Region, \
                 got {replica}"
            );
            replica_region
        })
        .collect();
    replicas.sort();
    (key_type, primary_region, replicas)
}

/// Wait until the key `identifier` names at the endpoint of `region` has left the
/// TRANSIENT key state `transient_state`, and report the `KeyMetadata` it settled
/// with.
///
/// Two of this service's key states are ones only the clock ends, each with its own
/// span: a replica key is `Creating` for `state::REPLICA_CREATING_SPAN` after the
/// replicate that made it, and the two keys a primary-Region move goes through are
/// `Updating` for `state::PRIMARY_REGION_UPDATE_SPAN` after the call that started it
/// ("even after this operation completes, the process of updating the primary Region
/// might still be in progress for a few more seconds"). The deadline is that state's
/// own span over, so a case waiting out the shorter one is not given the longer one's
/// patience. A case that is about what such a change LEFT waits for it here; what the
/// service answers WHILE it is in flight is asserted by the suites about those
/// operations.
pub(crate) async fn settle_key_state(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    identifier: &str,
    transient_state: &str,
) -> Value {
    let span = match transient_state {
        "Creating" => service_com_amazonaws_kms_trentservice::state::REPLICA_CREATING_SPAN,
        "Updating" => service_com_amazonaws_kms_trentservice::state::PRIMARY_REGION_UPDATE_SPAN,
        other => panic!("case {case}: {other} is not a key state only the clock ends"),
    };
    let deadline = std::time::Instant::now() + span * 20;
    loop {
        let metadata = describe_in(router, region, identifier).await;
        if metadata["KeyState"] != json!(transient_state) {
            return metadata;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: {identifier} must leave the transient {transient_state} key state, \
             got {metadata}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A principal ARN in the emulator's own account, which is the form KMS requires of a
/// grantee or retiring principal.
pub(crate) fn principal(resource: &str) -> String {
    format!(
        "arn:aws:iam::{account}:{resource}",
        account = engine::emulator_account_id()
    )
}

/// A member of a metadata object, as a string.
pub(crate) fn text(case: &str, value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {value}"))
        .to_string()
}

/// Whether `id` has the shape of a KMS key id: a version-4 UUID (8-4-4-4-12
/// lowercase hex with the version nibble `4`) for a single-Region key, and `mrk-`
/// followed by 32 lowercase hex characters for a multi-Region one.
pub(crate) fn is_key_id(id: &str, multi_region: bool) -> bool {
    if multi_region {
        return id
            .strip_prefix("mrk-")
            .is_some_and(|hex| hex.len() == 32 && is_lower_hex(hex));
    }
    let groups: Vec<&str> = id.split('-').collect();
    let lengths = [8usize, 4, 4, 4, 12];
    groups.len() == lengths.len()
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, len)| group.len() == len && is_lower_hex(group))
        && groups[2].starts_with('4')
}

/// Whether every character is a lowercase hexadecimal digit.
pub(crate) fn is_lower_hex(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Whether `haystack` contains `needle` as a contiguous run — how a test checks
/// that a ciphertext does not simply carry its plaintext.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| window == needle)
}

/// The `ValidationException` envelope the framework renders a SINGLE constraint violation
/// in, with `clause` as that violation — the counted prefix a constraint refusal carries,
/// written once for every suite that asserts one.
pub(crate) fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// A symmetric encryption key in the `Enabled` state.
pub(crate) async fn enabled_key(router: &MultiServiceRouter) -> Value {
    create_key(router, &json!({"Description": "round trip"})).await
}

/// The alias ARN of `alias` (`alias/<name>`) as the endpoint of `region` names it:
/// `arn:aws:kms:<region>:<account>:alias/<name>`.
pub(crate) fn alias_arn(region: &str, alias: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{account}:{alias}",
        account = engine::emulator_account_id()
    )
}

/// Assert an operation answered the empty 200 AWS documents for it ("this operation
/// does not return a response"), so a success carries no members at all.
pub(crate) fn assert_empty(case: &str, operation: &str, answer: &Value) {
    let empty = answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty());
    assert!(
        empty,
        "case {case}: {operation} must answer an empty 200 body, got {answer}"
    );
}

/// Create an alias for `target` in [`REGION`], requiring the empty 200.
pub(crate) async fn create_alias(
    router: &MultiServiceRouter,
    case: &str,
    alias: &str,
    target: &str,
) {
    create_alias_in(router, REGION, case, alias, target).await;
}

/// [`create_alias`], in `region`.
pub(crate) async fn create_alias_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    alias: &str,
    target: &str,
) {
    let answer = ok_in(
        router,
        region,
        "CreateAlias",
        &json!({"AliasName": alias, "TargetKeyId": target}),
    )
    .await;
    assert_empty(case, "CreateAlias", &answer);
}

/// Rotate the key `identifier` names on demand and wait for the rotation to COMPLETE,
/// which is what makes its material the one the key encrypts under.
///
/// "There might be a slight delay between initiating on-demand key rotation and the
/// rotation's completion" — an on-demand rotation is in progress for
/// `state::ON_DEMAND_ROTATION_SPAN` after the call that starts it, during which
/// `GetKeyRotationStatus` reports its `OnDemandRotationStartDate` and the key goes on
/// encrypting under the material it had. A case that is about what the rotation LEFT
/// waits for it here; the in-progress report itself is asserted by
/// `tests/all/refusal_and_rotation_contracts.rs`.
pub(crate) async fn rotate_on_demand(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> Value {
    rotate_on_demand_in(router, REGION, case, identifier).await
}

/// [`rotate_on_demand`], through the endpoint of `region`.
pub(crate) async fn rotate_on_demand_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    identifier: &str,
) -> Value {
    let answer = ok_in(
        router,
        region,
        "RotateKeyOnDemand",
        &json!({"KeyId": identifier}),
    )
    .await;
    settle_rotation_in(router, region, case, identifier).await;
    answer
}

/// Wait until no on-demand rotation of the key `identifier` names is in progress.
pub(crate) async fn settle_rotation(router: &MultiServiceRouter, case: &str, identifier: &str) {
    settle_rotation_in(router, REGION, case, identifier).await;
}

/// [`settle_rotation`], through the endpoint of `region`.
pub(crate) async fn settle_rotation_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    identifier: &str,
) {
    let deadline = std::time::Instant::now()
        + service_com_amazonaws_kms_trentservice::state::ON_DEMAND_ROTATION_SPAN * 20;
    loop {
        let status = ok_in(
            router,
            region,
            "GetKeyRotationStatus",
            &json!({"KeyId": identifier}),
        )
        .await;
        if status["OnDemandRotationStartDate"].is_null() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the on-demand rotation of {identifier} must complete, got {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// The whole `GetKeyLastUsage` answer for the key `identifier` names, once KMS's usage
/// tracking has PRODUCED a record that names `operation` and is not the one `previous`
/// carries (pass `&Value::Null` when there was none).
///
/// The tracking takes a span to begin recording a key (`state::USAGE_TRACKING_LAG`), so a
/// read taken straight after the FIRST cryptographic operation on a key is answered the
/// empty record; a case that is about WHAT was recorded waits for it here, and the empty
/// answer a read gets in the meantime is asserted by the suite that is about that
/// (`tests/all/key_last_usage.rs`).
pub(crate) async fn tracked_last_usage(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    operation: &str,
    previous: &Value,
) -> Value {
    tracked_last_usage_in(router, REGION, case, identifier, operation, previous).await
}

/// [`tracked_last_usage`], through the endpoint of `region`.
pub(crate) async fn tracked_last_usage_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    identifier: &str,
    operation: &str,
    previous: &Value,
) -> Value {
    let deadline = std::time::Instant::now()
        + service_com_amazonaws_kms_trentservice::state::USAGE_TRACKING_LAG * 20;
    loop {
        let answer = ok_in(
            router,
            region,
            "GetKeyLastUsage",
            &json!({"KeyId": identifier}),
        )
        .await;
        let record = &answer["KeyLastUsage"];
        if record["Operation"] == json!(operation) && *record != previous["KeyLastUsage"] {
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the usage tracking must produce the record of the {operation} of \
             {identifier}, got {answer}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

