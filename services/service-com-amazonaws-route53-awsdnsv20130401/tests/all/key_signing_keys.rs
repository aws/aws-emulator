// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateKeySigningKey`: the key-signing key (KSK) a
//! hosted zone is signed with.
//!
//! The rule under test: a KSK is a public/private key pair whose private half
//! stays in Key Management Service, so a create persists ONE key on the named
//! zone under the name it was given, in the initial status it was asked for, and
//! publishes the DNSSEC material of the customer managed key the request named —
//! the base64 public key, the key tag, the delegation signer digest and the DS
//! and DNSKEY record text, all derived from that key's own public half and the
//! zone's apex name — while an ARN that names no usable sign-and-verify
//! `ECC_NIST_P256` key of the account is refused rather than signed with. The
//! persisted key is what `GetDNSSEC` reports and what
//! `EnableHostedZoneDNSSEC` signs the zone over.
//!
//! One create is one row of that class, so the tables below span it: zones
//! differing in label count and spelling, KSK names differing in the characters
//! `KeySigningKey$Name` allows and in length, both initial statuses, one and two
//! keys per zone, one key published in two zones, and every ARN and input the
//! operation refuses — each refusal checked to have persisted nothing.
//!
//! The public half comes from the KMS emulator's own key store, so this suite
//! mounts both services on ONE engine and makes each customer managed key
//! through `CreateKey` — the operation a client makes one with — then reads the
//! same key's public half back through `GetPublicKey`, so what the KSK publishes
//! is checked against what KMS itself hands out rather than against a constant.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_KEY_SIGNING_KEYS_PER_ZONE;

use crate::dnssec_status::{enable_dnssec, get_dnssec};
use crate::hosted_zone_records::{call, create_zone_ok, Reply, ENDPOINT};

/// The Region the customer managed keys below are made in. A KMS key is
/// regional, and its ARN is what carries the Region to Route 53.
const REGION: &str = "us-east-1";

/// The key spec a KSK's customer managed key must have
/// (`KeySigningKey$KmsArn`: "Key spec ECC_NIST_P256").
const SIGNING_KEY_SPEC: &str = "ECC_NIST_P256";

/// The key usage it must have (`KeySigningKey$KmsArn`: "Key usage Sign and
/// verify"), as `CreateKeyRequest$KeyUsage` spells it.
const SIGNING_KEY_USAGE: &str = "SIGN_VERIFY";

/// How many octets the DNSKEY key material of an ECDSA P-256 key takes: the two
/// 32-octet coordinates of its public point (RFC 6605 § 4).
const COORDINATE_PAIR_OCTETS: usize = 64;

/// A router with Route 53 and KMS mounted on ONE shared engine — the assembly the
/// rule needs, since the public half a KSK publishes is KMS's own state.
///
/// `pub(crate)` so the sibling suite over the rest of a key's lifecycle drives
/// the same assembly rather than growing a second copy of it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_store().0
}

/// That same assembly, PLUS the engine handle.
///
/// A test that has to put a key into a status no implemented operation reaches
/// needs the store as well; everything else takes [`emulator`] above.
pub(crate) fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder credential; signature
    // verification is a front-door setting, not part of what this suite is about.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Call one KMS operation and return its status and JSON body. KMS speaks
/// `awsJson1_1` over `POST /`, selected by the `x-amz-target` header.
async fn kms_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the KMS request body serializes"),
        ))
        .expect("the KMS request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Make one KMS key of `key_spec` under `key_usage` and return its key ARN.
pub(crate) async fn create_kms_key(
    router: &MultiServiceRouter,
    case: &str,
    key_spec: &str,
    key_usage: &str,
) -> String {
    let (status, body) = kms_call(
        router,
        "CreateKey",
        &json!({ "KeySpec": key_spec, "KeyUsage": key_usage }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the {key_spec} {key_usage} key must succeed, got {status} \
         with body {body}"
    );
    body["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateKey must report a key ARN, got {body}"))
        .to_string()
}

/// A customer managed key configured as a KSK's key must be.
pub(crate) async fn create_signing_kms_key(router: &MultiServiceRouter, case: &str) -> String {
    create_kms_key(router, case, SIGNING_KEY_SPEC, SIGNING_KEY_USAGE).await
}

/// The DNSKEY key material of the KMS key `kms_arn`, as KMS itself publishes it:
/// the trailing coordinate pair of the X.509 `SubjectPublicKeyInfo`
/// `GetPublicKey` answers with, which is what a DNSKEY carries for this algorithm
/// (RFC 6605 § 4).
async fn kms_public_key_material(router: &MultiServiceRouter, case: &str, kms_arn: &str) -> Vec<u8> {
    let (status, body) = kms_call(router, "GetPublicKey", &json!({ "KeyId": kms_arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPublicKey for {kms_arn} must succeed, got {status} with body {body}"
    );
    let encoded = body["PublicKey"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetPublicKey must report a public key: {body}"));
    let der = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the public key is base64: {error}"));
    assert!(
        der.len() > COORDINATE_PAIR_OCTETS,
        "case {case:?}: a P-256 SubjectPublicKeyInfo is longer than its coordinate pair, got {} \
         octets",
        der.len()
    );
    der[der.len() - COORDINATE_PAIR_OCTETS..].to_vec()
}

/// A `CreateKeySigningKey` body.
fn create_ksk_body(
    caller_reference: &str,
    hosted_zone_id: &str,
    kms_arn: &str,
    name: &str,
    status: &str,
) -> String {
    format!(
        "<CreateKeySigningKeyRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <HostedZoneId>{hosted_zone_id}</HostedZoneId>\
         <KeyManagementServiceArn>{kms_arn}</KeyManagementServiceArn>\
         <Name>{name}</Name><Status>{status}</Status>\
         </CreateKeySigningKeyRequest>"
    )
}

/// Create one key-signing key and return the reply.
pub(crate) async fn create_ksk(
    router: &MultiServiceRouter,
    hosted_zone_id: &str,
    name: &str,
    status: &str,
    kms_arn: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/keysigningkey",
        &create_ksk_body(
            &format!("ksk-{hosted_zone_id}-{name}"),
            hosted_zone_id,
            kms_arn,
            name,
            status,
        ),
    )
    .await
}

/// Create one key-signing key, requiring the call to be served.
pub(crate) async fn create_ksk_ok(
    router: &MultiServiceRouter,
    case: &str,
    hosted_zone_id: &str,
    name: &str,
    status: &str,
    kms_arn: &str,
) -> Reply {
    let reply = create_ksk(router, hosted_zone_id, name, status, kms_arn).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected 201 (501 = still scaffolded), got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// Where a create answers the key it made: `CreateKeySigningKeyResponse` carries
/// it as one `KeySigningKey` element.
const CREATED_KEY: &str = "KeySigningKey";

/// Where a DNSSEC read reports a key: `GetDNSSECResponse$KeySigningKeys` is a
/// list, so each key travels as one `member` of it.
pub(crate) const REPORTED_KEY: &str = "member";

/// The text of one element of a reply, or a panic naming the case.
pub(crate) fn member<'a>(case: &str, reply: &'a Reply, path: &str) -> &'a str {
    reply.first(path).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry {path}, got body {}",
            reply.body
        )
    })
}

/// The text of one member of the key a create answered.
fn created<'a>(case: &str, reply: &'a Reply, name: &str) -> &'a str {
    member(case, reply, &format!("{CREATED_KEY}/{name}"))
}

/// The text of one member of the first key a DNSSEC read reported.
fn reported<'a>(case: &str, reply: &'a Reply, name: &str) -> &'a str {
    member(case, reply, &format!("{REPORTED_KEY}/{name}"))
}

/// Every member of a key-signing key whose value the create and the DNSSEC read
/// must agree on, which is what makes the create's answer the key it PERSISTED.
const PERSISTED_MEMBERS: &[&str] = &[
    "Name",
    "Status",
    "KmsArn",
    "Flag",
    "SigningAlgorithmMnemonic",
    "SigningAlgorithmType",
    "DigestAlgorithmMnemonic",
    "DigestAlgorithmType",
    "KeyTag",
    "DigestValue",
    "PublicKey",
    "DSRecord",
    "DNSKEYRecord",
    "CreatedDate",
    "LastModifiedDate",
];

/// How many key-signing keys `GetDNSSEC` reports for a zone.
pub(crate) async fn ksk_count(router: &MultiServiceRouter, zone_id: &str) -> usize {
    get_dnssec(router, zone_id)
        .await
        .all(&format!("{REPORTED_KEY}/Name"))
        .len()
}

/// Assert the `ChangeInfo` a create answered is a change a caller can poll.
///
/// `pub(crate)` so the sibling suite over the rest of a key's lifecycle checks
/// the change ITS operation answers by the same rule.
pub(crate) fn assert_polls_a_change(case: &str, reply: &Reply) {
    let id = member(case, reply, "ChangeInfo/Id");
    let bare = id.strip_prefix("/change/").unwrap_or_else(|| {
        panic!("case {case:?}: a change id is reported behind /change/, got {id:?}")
    });
    assert!(
        bare.starts_with('C') && bare.len() == 15,
        "case {case:?}: a minted change id is C plus 14 characters, got {bare:?}"
    );
    let status = member(case, reply, "ChangeInfo/Status");
    // The modelled `ChangeStatus` values
    // (smithy_query(query="shape",shape="ChangeStatus")).
    assert!(
        status == "PENDING" || status == "INSYNC",
        "case {case:?}: a change is PENDING or INSYNC, got {status:?}"
    );
}

/// Assert the constants every KSK this service creates publishes, which are the
/// model's own: `KeySigningKey$Flag` ("for key-signing key (KSK), this value is
/// always 257") and the algorithms RFC 8624 § 3.1 and § 3.3 name for a NIST P-256
/// key pair published with a SHA-256 delegation signer.
fn assert_published_algorithms(case: &str, reply: &Reply) {
    for (name, expected) in [
        ("Flag", "257"),
        ("SigningAlgorithmMnemonic", "ECDSAP256SHA256"),
        ("SigningAlgorithmType", "13"),
        ("DigestAlgorithmMnemonic", "SHA-256"),
        ("DigestAlgorithmType", "2"),
    ] {
        let actual = created(case, reply, name);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {name} {expected:?}, got {actual:?}"
        );
    }
}

/// Assert the DS and DNSKEY record text a reply carries renders the very values
/// reported beside it: the DS record is "key tag, algorithm, digest type, digest"
/// (RFC 4034 § 5.3) and the DNSKEY record is "flags, protocol, algorithm, key"
/// (RFC 4034 § 2.2), so neither can drift from the structure.
fn assert_records_render_the_members(case: &str, reply: &Reply) {
    let key_tag = created(case, reply, "KeyTag");
    let digest = created(case, reply, "DigestValue");
    let public_key = created(case, reply, "PublicKey");
    let ds_record = created(case, reply, "DSRecord");
    let dnskey_record = created(case, reply, "DNSKEYRecord");
    assert_eq!(
        ds_record,
        format!("{key_tag} 13 2 {digest}"),
        "case {case:?}: the DS record must render the key tag, algorithm, digest type and \
         digest reported beside it, got {ds_record:?}"
    );
    assert_eq!(
        dnskey_record,
        format!("257 3 13 {public_key}"),
        "case {case:?}: the DNSKEY record must render the flags, protocol, algorithm and key \
         reported beside it, got {dnskey_record:?}"
    );
    assert_eq!(
        digest.len(),
        64,
        "case {case:?}: a SHA-256 digest is 32 octets, so 64 hexadecimal characters, got \
         {digest:?}"
    );
    assert!(
        digest
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_uppercase()),
        "case {case:?}: a DS record's digest is written in upper-case hexadecimal, got {digest:?}"
    );
    let key_tag: u32 = key_tag
        .parse()
        .unwrap_or_else(|error| panic!("case {case:?}: the key tag is an integer: {error}"));
    // `SigningKeyTag` is `range 0..65536`.
    assert!(
        key_tag <= 65_536,
        "case {case:?}: the key tag is inside its modelled range, got {key_tag}"
    );
}

/// One row of the create table: the zone to publish in, the KSK's name and its
/// initial status.
struct CreateRow {
    zone_name: &'static str,
    ksk_name: &'static str,
    status: &'static str,
}

/// Rows differing along what a create can differ in: the zone's label count and
/// spelling, the characters and the length `KeySigningKey$Name` allows ("Name can
/// include numbers, letters, and underscores (_)", length 3..128 — so the
/// shortest name allowed is a row of its own), and both initial statuses
/// `CreateKeySigningKeyRequest$Status` accepts.
const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        zone_name: "example.com",
        ksk_name: "ksk",
        status: "ACTIVE",
    },
    CreateRow {
        zone_name: "a.b.c.deep.example.net",
        ksk_name: "Key_Signing_Key_2024",
        status: "INACTIVE",
    },
    CreateRow {
        zone_name: "trailing.example.org.",
        ksk_name: "K99",
        status: "ACTIVE",
    },
    CreateRow {
        zone_name: "MiXeD.Example.Com",
        ksk_name: "_____",
        status: "INACTIVE",
    },
];

#[tokio::test]
async fn a_created_key_publishes_the_material_of_its_customer_managed_key() {
    for row in CREATE_ROWS {
        let case = format!("{} / {} / {}", row.zone_name, row.ksk_name, row.status);
        let router = emulator();
        let zone_id = create_zone_ok(&router, row.zone_name, &format!("ref-{}", row.ksk_name)).await;
        let kms_arn = create_signing_kms_key(&router, &case).await;

        let reply = create_ksk_ok(&router, &case, &zone_id, row.ksk_name, row.status, &kms_arn)
            .await;

        // The create answers the key it persisted, and a change to poll.
        assert_eq!(
            created(&case, &reply, "Name"),
            row.ksk_name,
            "case {case:?}: the created key carries the name the request asked for"
        );
        assert_eq!(
            created(&case, &reply, "Status"),
            row.status,
            "case {case:?}: the created key carries the initial status the request asked for"
        );
        assert_eq!(
            created(&case, &reply, "KmsArn"),
            kms_arn,
            "case {case:?}: the created key names the customer managed key behind it"
        );
        assert_polls_a_change(&case, &reply);
        assert_published_algorithms(&case, &reply);
        assert_records_render_the_members(&case, &reply);

        // The published key is the PUBLIC half of that KMS key, as KMS itself
        // hands it out — not a value minted beside it.
        let expected_key = aws_smithy_types::base64::encode(
            &kms_public_key_material(&router, &case, &kms_arn).await[..],
        );
        let published = created(&case, &reply, "PublicKey");
        assert_eq!(
            published, expected_key,
            "case {case:?}: the published key must be the KMS key's own public half, got \
             {published:?}"
        );

        // `Location` is the path the operations that read this key back are bound
        // to, so the URL the create hands out leads to the key it created.
        let location = reply.location.as_deref().unwrap_or("");
        assert_eq!(
            location,
            format!(
                "https://{ENDPOINT}/2013-04-01/keysigningkey/{zone_id}/{}",
                row.ksk_name
            ),
            "case {case:?}: expected the key's own URL, got {location:?}"
        );

        // Read back through the implemented DNSSEC report: one key, with the
        // values the create answered, and a zone that is NOT signing yet —
        // creating a key does not start signing.
        let read = get_dnssec(&router, &zone_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetDNSSEC must be served, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.all(&format!("{REPORTED_KEY}/Name")),
            vec![row.ksk_name],
            "case {case:?}: the zone must report exactly the key that was created"
        );
        for name in PERSISTED_MEMBERS {
            let answered = created(&case, &reply, name);
            let read_back = reported(&case, &read, name);
            assert_eq!(
                answered, read_back,
                "case {case:?}: the DNSSEC read must report the created key's {name}, got \
                 {read_back:?} after {answered:?}"
            );
        }
        assert_eq!(
            member(&case, &read, "Status/ServeSignature"),
            "NOT_SIGNING",
            "case {case:?}: creating a key-signing key does not start signing the zone"
        );
    }
}

#[tokio::test]
async fn two_keys_in_one_zone_publish_their_own_customer_managed_keys() {
    let case = "two keys in one zone";
    let router = emulator();
    let zone_id = create_zone_ok(&router, "example.com", "ref-two-ksks").await;
    let first_arn = create_signing_kms_key(&router, case).await;
    let second_arn = create_signing_kms_key(&router, case).await;
    assert_ne!(
        first_arn, second_arn,
        "case {case:?}: two CreateKey calls make two keys"
    );

    let first = create_ksk_ok(&router, case, &zone_id, "first", "ACTIVE", &first_arn).await;
    let second = create_ksk_ok(&router, case, &zone_id, "second", "INACTIVE", &second_arn).await;

    assert_ne!(
        created(case, &first, "PublicKey"),
        created(case, &second, "PublicKey"),
        "case {case:?}: two keys over two customer managed keys publish two public keys"
    );
    assert_ne!(
        created(case, &first, "DSRecord"),
        created(case, &second, "DSRecord"),
        "case {case:?}: two different published keys give two different DS records"
    );

    // Both are held on the zone, in `Name` order, and the first key's values did
    // not move when the second was added.
    let read = get_dnssec(&router, &zone_id).await;
    assert_eq!(
        read.all(&format!("{REPORTED_KEY}/Name")),
        vec!["first", "second"],
        "case {case:?}: the zone reports both keys, got body {}",
        read.body
    );
    assert_eq!(
        read.all(&format!("{REPORTED_KEY}/PublicKey")),
        vec![
            created(case, &first, "PublicKey"),
            created(case, &second, "PublicKey"),
        ],
        "case {case:?}: each reported key keeps its own published key"
    );
}

#[tokio::test]
async fn one_customer_managed_key_published_in_two_zones_digests_each_zone() {
    let case = "one key, two zones";
    let router = emulator();
    let first_zone = create_zone_ok(&router, "example.com", "ref-digest-1").await;
    let second_zone = create_zone_ok(&router, "example.net", "ref-digest-2").await;
    let kms_arn = create_signing_kms_key(&router, case).await;

    let first = create_ksk_ok(&router, case, &first_zone, "shared", "ACTIVE", &kms_arn).await;
    let second = create_ksk_ok(&router, case, &second_zone, "shared", "ACTIVE", &kms_arn).await;

    // The published key is the same key pair's public half ...
    assert_eq!(
        created(case, &first, "PublicKey"),
        created(case, &second, "PublicKey"),
        "case {case:?}: one customer managed key has one public half"
    );
    assert_eq!(
        created(case, &first, "KeyTag"),
        created(case, &second, "KeyTag"),
        "case {case:?}: the key tag is a checksum over the published key alone"
    );
    // ... but the delegation signer digest covers the DNSKEY's OWNER NAME as well
    // (RFC 4034 § 5.1.4), so each zone's DS record is its own.
    assert_ne!(
        created(case, &first, "DigestValue"),
        created(case, &second, "DigestValue"),
        "case {case:?}: the digest covers the zone's own name, so two zones digest differently"
    );
}

/// One row of the refusal table: what is wrong with the request, the modelled
/// error code it answers and the HTTP status that error carries.
struct RefusalRow {
    case: &'static str,
    ksk_name: &'static str,
    status: &'static str,
    /// How the row's `KeyManagementServiceArn` is chosen.
    kms_key: KmsKeyRow,
    /// Whether the request names the zone the row created, or an id no zone
    /// carries.
    zone: ZoneRow,
    code: &'static str,
    http_status: u16,
}

/// Which customer managed key — or which ARN that names none — a refusal row
/// sends.
enum KmsKeyRow {
    /// A key configured as `KeySigningKey$KmsArn` requires.
    Signing,
    /// A key of another spec or usage, or a disabled one: each is a key Route 53
    /// cannot sign a zone with.
    Other {
        key_spec: &'static str,
        key_usage: &'static str,
        disabled: bool,
    },
    /// Text that is not the ARN of a key of this account's KMS store.
    Literal(&'static str),
}

/// Which hosted zone a refusal row addresses.
enum ZoneRow {
    /// The zone the row created.
    Created,
    /// An id no zone in the store carries.
    Missing,
}

/// The rows: every input and every ARN `CreateKeySigningKey` refuses, with the
/// modelled error each one answers
/// (`smithy_query(query="operation",operation="CreateKeySigningKey")`).
const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "a name carrying a character outside letters, digits and underscore",
        ksk_name: "ksk-one",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Signing,
        zone: ZoneRow::Created,
        code: "InvalidKeySigningKeyName",
        http_status: 400,
    },
    RefusalRow {
        case: "a name carrying a dot",
        ksk_name: "ksk.one",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Signing,
        zone: ZoneRow::Created,
        code: "InvalidKeySigningKeyName",
        http_status: 400,
    },
    RefusalRow {
        case: "a status outside ACTIVE and INACTIVE",
        ksk_name: "ksk",
        status: "DELETING",
        kms_key: KmsKeyRow::Signing,
        zone: ZoneRow::Created,
        code: "InvalidKeySigningKeyStatus",
        http_status: 400,
    },
    RefusalRow {
        case: "a status in the wrong case",
        ksk_name: "ksk",
        status: "active",
        kms_key: KmsKeyRow::Signing,
        zone: ZoneRow::Created,
        code: "InvalidKeySigningKeyStatus",
        http_status: 400,
    },
    RefusalRow {
        case: "an id no hosted zone carries",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Signing,
        zone: ZoneRow::Missing,
        code: "NoSuchHostedZone",
        http_status: 404,
    },
    RefusalRow {
        case: "text that is not an ARN",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Literal("not-an-arn"),
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "an ARN of another service",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Literal("arn:aws:s3:::example-bucket"),
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "a KMS key ARN no key of the account carries",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Literal(
            "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000",
        ),
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "a symmetric encryption key",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Other {
            key_spec: "SYMMETRIC_DEFAULT",
            key_usage: "ENCRYPT_DECRYPT",
            disabled: false,
        },
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "a P-256 key whose usage is key agreement rather than sign and verify",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Other {
            key_spec: "ECC_NIST_P256",
            key_usage: "KEY_AGREEMENT",
            disabled: false,
        },
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "a sign-and-verify key on another curve",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Other {
            key_spec: "ECC_NIST_P384",
            key_usage: "SIGN_VERIFY",
            disabled: false,
        },
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
    RefusalRow {
        case: "a correctly configured key that is disabled",
        ksk_name: "ksk",
        status: "ACTIVE",
        kms_key: KmsKeyRow::Other {
            key_spec: "ECC_NIST_P256",
            key_usage: "SIGN_VERIFY",
            disabled: true,
        },
        zone: ZoneRow::Created,
        code: "InvalidKMSArn",
        http_status: 400,
    },
];

#[tokio::test]
async fn every_refused_create_persists_nothing() {
    for row in REFUSAL_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-refusal").await;
        let kms_arn = match &row.kms_key {
            KmsKeyRow::Signing => create_signing_kms_key(&router, case).await,
            KmsKeyRow::Other {
                key_spec,
                key_usage,
                disabled,
            } => {
                let arn = create_kms_key(&router, case, key_spec, key_usage).await;
                if *disabled {
                    let (status, body) =
                        kms_call(&router, "DisableKey", &json!({ "KeyId": arn })).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: disabling the key must succeed, got {status} with body \
                         {body}"
                    );
                }
                arn
            }
            KmsKeyRow::Literal(arn) => (*arn).to_string(),
        };
        let addressed = match row.zone {
            ZoneRow::Created => zone_id.clone(),
            // A well-formed zone id (`ResourceId` is at most 32 characters) that
            // no zone in this store carries.
            ZoneRow::Missing => "Z00000000000000MISSING".to_string(),
        };

        let reply = create_ksk(&router, &addressed, row.ksk_name, row.status, &kms_arn).await;

        assert_eq!(
            reply.status, row.http_status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            row.http_status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {case:?}: expected the {} refusal, got body {}",
            row.code,
            reply.body
        );
        assert!(
            !reply
                .first("Error/Message")
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "case {case:?}: a refusal must say what was wrong, got body {}",
            reply.body
        );
        assert_eq!(
            ksk_count(&router, &zone_id).await,
            0,
            "case {case:?}: a refused create must leave the zone with no key-signing key"
        );
    }
}

#[tokio::test]
async fn a_name_or_a_customer_managed_key_the_zone_already_uses_is_refused() {
    let case = "a repeated name or customer managed key";
    let router = emulator();
    let zone_id = create_zone_ok(&router, "example.com", "ref-duplicates").await;
    let first_arn = create_signing_kms_key(&router, case).await;
    let second_arn = create_signing_kms_key(&router, case).await;
    create_ksk_ok(&router, case, &zone_id, "held", "ACTIVE", &first_arn).await;

    // Both halves of `KeySigningKeyAlreadyExists`: the name is "unique for each
    // key-signing key in the same hosted zone" and the `KmsArn` "must be unique
    // for each key-signing key (KSK) in a single hosted zone".
    for (what, name, arn) in [
        ("the same name over another key", "held", second_arn.as_str()),
        ("another name over the same key", "other", first_arn.as_str()),
        ("the same name and key again", "held", first_arn.as_str()),
    ] {
        let reply = create_ksk(&router, &zone_id, name, "ACTIVE", arn).await;
        assert_eq!(
            reply.status, 409,
            "case {what:?}: expected HTTP 409, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "KeySigningKeyAlreadyExists",
            "case {what:?}: expected the KeySigningKeyAlreadyExists refusal, got body {}",
            reply.body
        );
        assert_eq!(
            ksk_count(&router, &zone_id).await,
            1,
            "case {what:?}: the zone keeps the one key it held"
        );
    }

    // Another zone may publish the same customer managed key: the uniqueness is
    // per hosted zone.
    let other_zone = create_zone_ok(&router, "example.net", "ref-duplicates-2").await;
    create_ksk_ok(&router, case, &other_zone, "held", "ACTIVE", &first_arn).await;
}

#[tokio::test]
async fn a_zone_holds_no_more_keys_than_the_quota() {
    let case = "the per-zone quota";
    let router = emulator();
    let zone_id = create_zone_ok(&router, "example.com", "ref-quota").await;

    // "You can only have two KSKs per hosted zone"
    // (`smithy_query(query="doc",shape="CreateKeySigningKey")`), so filling the
    // quota is two creates and the next one is refused.
    for index in 0..MAX_KEY_SIGNING_KEYS_PER_ZONE {
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(
            &router,
            case,
            &zone_id,
            &format!("ksk_{index}"),
            "ACTIVE",
            &arn,
        )
        .await;
    }
    assert_eq!(
        ksk_count(&router, &zone_id).await,
        MAX_KEY_SIGNING_KEYS_PER_ZONE,
        "case {case:?}: the zone holds the keys that were created"
    );

    let arn = create_signing_kms_key(&router, case).await;
    let reply = create_ksk(&router, &zone_id, "one_too_many", "ACTIVE", &arn).await;
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "TooManyKeySigningKeys",
        "case {case:?}: expected the TooManyKeySigningKeys refusal, got body {}",
        reply.body
    );
    assert_eq!(
        ksk_count(&router, &zone_id).await,
        MAX_KEY_SIGNING_KEYS_PER_ZONE,
        "case {case:?}: a refused create adds no key"
    );
}

#[tokio::test]
async fn only_an_active_created_key_lets_the_zone_be_signed() {
    // The create is what gives a zone a key to sign with, so the status it
    // persists is what the enable reads: a zone whose only key is INACTIVE has no
    // key "with ACTIVE status", and one with an ACTIVE key signs.
    for (case, status, expected_status, expected_signature) in [
        ("an INACTIVE key", "INACTIVE", 400, "NOT_SIGNING"),
        ("an ACTIVE key", "ACTIVE", 200, "SIGNING"),
    ] {
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-signable").await;
        let kms_arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, "signer", status, &kms_arn).await;

        let enabled = enable_dnssec(&router, &zone_id).await;
        assert_eq!(
            enabled.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from the enable, got {} with body {}",
            enabled.status, enabled.body
        );
        if expected_status != 200 {
            assert_eq!(
                enabled.error_code(),
                "KeySigningKeyWithActiveStatusNotFound",
                "case {case:?}: a zone with no ACTIVE key is refused for that reason, got body {}",
                enabled.body
            );
        }
        let read = get_dnssec(&router, &zone_id).await;
        let reported = member(case, &read, "Status/ServeSignature");
        assert_eq!(
            reported, expected_signature,
            "case {case:?}: expected the zone to report {expected_signature}, got {reported:?}"
        );
    }
}
