// SPDX-License-Identifier: CC0-1.0

//! Refusals whose whole `Message` is the service's OWN fixed text for the condition.
//!
//! RULE under test: where this service states a refusal in a sentence of its own, the
//! answer is that sentence — not a rebuilt description of the rule — and it is FIXED for
//! the condition: the same text for every request that meets it, carrying nothing the
//! caller sent (no member name, no value, no identifier), so a client matching or
//! logging the service's refusal recognises it.
//!
//! Six conditions, each asserted over a TABLE whose rows differ along what the
//! condition generalises over rather than along the one request a report cited:
//!
//! 1. a server configuration that hosts its endpoint in a VPC and names no
//!    `EndpointDetails` — the rows span both `EndpointType` values that host in a VPC,
//!    crossed with the other members the create carries, and with the operation (create
//!    and update);
//! 2. a connector transfer that names no file to transfer — the rows span both connector
//!    kinds and both ways of naming nothing;
//! 3. host key material the service cannot read — the rows span the FORMS of unreadable
//!    material, including both published bounds of `HostKeyBody`'s `len 0..4096`;
//! 4. a resource reference the store cannot resolve — the rows span how the reference
//!    came not to be held (its own delete, the delete of the resource it was written on,
//!    an identifier nothing ever minted) and the operation that is asked for it, readers
//!    and mutations alike, over the agreement and the server kinds;
//! 5. a write of a web app's customization that lost the claim — the rows span the
//!    operation that lost, how many writers overlapped, and whether the web app held a
//!    customization already;
//! 6. a connector `Url` that names no endpoint the service could reach — the rows span
//!    WHY the value names none (a host under each top-level domain the DNS never
//!    delegates, a value that is no absolute URL, one carrying no authority, a host no
//!    DNS name can take the shape of), the scheme and protocol configuration it is sent
//!    under, and the operation that writes the connector, and are bounded by the
//!    endpoints that DO resolve and are still written.
//!
//! Conditions 4 and 5 are where the refusal CLASS matters as much as the wording: both
//! answers are in the asking operation's modelled error set, so a client that branches on
//! the code it models (to conclude "already deleted", or to retry a lost write) classifies
//! what it is given, and the read and the mutations of one vanished reference cannot
//! disagree about whether it is gone.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{
    custom_directories, drain_pages, generated_host_key, member_of, Answer, Emulator, ROLE,
    SSH_KEY, UNMINTED_SERVER_ID,
};
use crate::web_app_customization_write_guard::{create_web_app, futures_of};

/// Assert `answer` is 400 `InvalidRequestException` whose `Message` is exactly
/// `expected` and carries none of `absent` — the values the request sent.
fn assert_fixed_refusal(case: &str, answer: &Answer, expected: &str, absent: &[&str]) {
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "InvalidRequestException"),
        "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
        answer.status,
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the service's fixed text {expected:?}, got {message:?}"
    );
    for value in absent {
        assert!(
            !value.is_empty() && !message.contains(value),
            "case {case:?}: expected no request data in Message, got {message:?} carrying \
             {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. A VPC-hosted endpoint with no settings
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const ENDPOINT_DETAILS_REQUIRED: &str = "EndpointDetails required for VPC";

/// RULE: a server whose endpoint would be hosted in a VPC with no `EndpointDetails` is
/// refused with the service's fixed text, and nothing is created or changed.
///
/// The rows differ along the `EndpointType` that hosts the endpoint in a VPC (both
/// values of the modelled `[PUBLIC, VPC, VPC_ENDPOINT]` set that do), the other members
/// the request carries, and the operation that resolves the configuration — a create and
/// an update of a stored server, which the same requirement holds for.
#[tokio::test]
async fn a_vpc_hosted_endpoint_with_no_settings_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();
    // A stored server the update rows address, hosted on a PUBLIC endpoint.
    let server_id = emulator.create_server(json!({})).await;

    // (case, the members beside EndpointType)
    let shapes: Vec<(&str, Value)> = vec![
        ("nothing else", json!({})),
        (
            "a logging role and a banner",
            json!({
                "LoggingRole": ROLE,
                "PreAuthenticationLoginBanner": "restricted",
            }),
        ),
        (
            "an EFS domain",
            json!({"Domain": "EFS"}),
        ),
    ];

    for endpoint_type in ["VPC", "VPC_ENDPOINT"] {
        for (shape, extra) in &shapes {
            for op in ["CreateServer", "UpdateServer"] {
                let case = format!("{op} with EndpointType {endpoint_type} and {shape}");
                let mut request = json!({"EndpointType": endpoint_type});
                let object = request.as_object_mut().expect("the request is an object");
                for (member, value) in extra.as_object().expect("the row is an object") {
                    object.insert(member.clone(), value.clone());
                }
                if op == "UpdateServer" {
                    object.insert("ServerId".to_string(), json!(&server_id));
                }
                let answer = emulator.call(op, request).await;
                assert_fixed_refusal(
                    &case,
                    &answer,
                    ENDPOINT_DETAILS_REQUIRED,
                    &[server_id.as_str(), ROLE],
                );
                assert!(
                    member_of(&answer.body, "ServerId").is_null(),
                    "case {case:?}: expected the refusal to answer no ServerId, got {}",
                    answer.body
                );
            }
        }
    }

    // The requirement is about the VPC-hosted configuration alone: the stored server is
    // untouched and a PUBLIC create still succeeds.
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        member_of(described.get("Server"), "EndpointType").as_str(),
        Some("PUBLIC"),
        "case \"the stored server after the refusals\": expected it untouched, got {}",
        described.body
    );
    let public = emulator
        .call("CreateServer", json!({"EndpointType": "PUBLIC"}))
        .await;
    assert_eq!(
        public.status, 200,
        "case \"a PUBLIC create\": expected 200, got {} with body {}",
        public.status, public.body
    );
}

// ---------------------------------------------------------------------------
// 2. A transfer that names no file
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const EMPTY_SEND_FILE_LIST: &str = "Empty send file list";

/// RULE: a `StartFileTransfer` that names no file to transfer is refused with the
/// service's fixed text, whichever connector it was addressed at, and accepts nothing.
///
/// The rows differ along the connector kind (AS2 and SFTP, the two a connector can
/// configure) and along HOW the request names nothing — no path member at all, and the
/// directory members a transfer may carry without naming a file.
#[tokio::test]
async fn a_transfer_that_names_no_file_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();

    let as2 = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": "p-0123456789abcdef0",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                },
            }),
        )
        .await;
    let sftp = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner"},
            }),
        )
        .await;
    let connectors: Vec<(&str, String)> = vec![
        ("an AS2 connector", connector_id("AS2", &as2)),
        ("an SFTP connector", connector_id("SFTP", &sftp)),
    ];

    let shapes: Vec<(&str, Value)> = vec![
        ("no path member at all", json!({})),
        (
            "a remote directory but no file",
            json!({"RemoteDirectoryPath": "/inbox"}),
        ),
        (
            "a local directory but no file",
            json!({"LocalDirectoryPath": "/bucket/landing"}),
        ),
    ];

    for (kind, connector_id) in &connectors {
        for (shape, extra) in &shapes {
            let case = format!("{kind} with {shape}");
            let mut request = json!({"ConnectorId": connector_id});
            let object = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the row is an object") {
                object.insert(member.clone(), value.clone());
            }
            let answer = emulator.call("StartFileTransfer", request).await;
            assert_fixed_refusal(
                &case,
                &answer,
                EMPTY_SEND_FILE_LIST,
                &[connector_id.as_str()],
            );
            assert!(
                member_of(&answer.body, "TransferId").is_null(),
                "case {case:?}: expected the refusal to accept nothing, got {}",
                answer.body
            );
        }
    }
}

/// The `ConnectorId` a create answered.
fn connector_id(kind: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "the {kind} connector must be created, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("the {kind} create answered no ConnectorId: {}", answer.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// 3. Host key material the service cannot read
// ---------------------------------------------------------------------------

/// The text the service states for that condition. `pub(crate)` because another suite
/// pins the same sentence on refusals of its own rows ([`crate::host_key_listing`]), and
/// one text the service states belongs in one place.
pub(crate) const INVALID_HOST_KEY_FORMAT: &str = "Invalid host key format";

/// RULE: an `ImportHostKey` whose `HostKeyBody` the service cannot read as a private key
/// is refused with the service's fixed text — which names neither the member nor any
/// expected encoding — and imports nothing.
///
/// The rows differ along the FORM of the unreadable material: the empty body and a body
/// at the modelled maximum (both published bounds of `HostKeyBody`'s `len 0..4096`),
/// prose, a PUBLIC key instead of a private one, a PEM envelope with no key inside, and
/// a truncated OpenSSH private key.
#[tokio::test]
async fn host_key_material_the_service_cannot_read_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let (material, _fingerprint) = generated_host_key();
    let truncated: String = material.chars().take(40).collect();

    let unreadable: Vec<(&str, String)> = vec![
        ("the empty body the modelled minimum admits", String::new()),
        ("prose", "this is not a private key".to_string()),
        ("a public key instead of a private one", SSH_KEY.to_string()),
        (
            "a PEM envelope with no key inside",
            "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n".to_string(),
        ),
        ("a truncated OpenSSH private key", truncated),
        (
            "a body at the modelled maximum length",
            "k".repeat(4096),
        ),
    ];

    for (case, body) in &unreadable {
        let answer = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": body}),
            )
            .await;
        assert_fixed_refusal(case, &answer, INVALID_HOST_KEY_FORMAT, &[server_id.as_str()]);
        assert!(
            member_of(&answer.body, "HostKeyId").is_null(),
            "case {case:?}: expected the refusal to mint no HostKeyId, got {}",
            answer.body
        );
    }

    // The server is untouched: readable material still imports.
    let imported = emulator
        .call(
            "ImportHostKey",
            json!({"ServerId": &server_id, "HostKeyBody": &material}),
        )
        .await;
    assert_eq!(
        imported.status, 200,
        "case \"readable material after the refusals\": expected 200, got {} with body {}",
        imported.status, imported.body
    );
}

// ---------------------------------------------------------------------------
// 4. A resource reference the store cannot resolve
// ---------------------------------------------------------------------------

/// The text the service states for a `ServerId` nothing in the store answers to.
const UNKNOWN_SERVER: &str = "Unknown server";

/// The text the service states for a `(ServerId, AgreementId)` pair the store does not
/// hold — the pair the model makes `AgreementResource`'s identifiers
/// (`smithy_query(query="operation",operation="DeleteAgreement")` takes both, each
/// `@required`) — whichever half of it is the one that resolves to nothing.
const UNKNOWN_AGREEMENT: &str = "Unknown agreement";

/// `AgreementId`s of the modelled form (`len 19..19`, pattern `^a-([0-9a-f]{17})$`) that
/// no `CreateAgreement` ever minted: the low end of the id space and the high end.
const NEVER_MINTED_AGREEMENTS: [&str; 2] = ["a-00000000000000000", "a-fffffffffffffffff"];

/// A `ServerId` of the modelled form (`len 19..19`, pattern `^s-([0-9a-f]{17})$`) that no
/// `CreateServer` minted, beside the harness's own [`UNMINTED_SERVER_ID`]: the high end of
/// the id space.
const NEVER_MINTED_SERVER: &str = "s-fffffffffffffffff";

/// Assert `answer` is the service's not-found for `resource`, reported as
/// `resource_type`, whose whole `Message` is the fixed `expected` text.
///
/// The status and the code are the service's single answer to a resource reference it
/// cannot resolve: HTTP 400 on the recorded wire (which
/// `ResourceNotFoundException`'s modelled `@httpError` does not describe) and the
/// `ResourceNotFoundException` every operation of these planes models — so a refusal from
/// outside the asking operation's modelled error set fails here.
fn assert_fixed_not_found(
    case: &str,
    op: &str,
    answer: &Answer,
    resource_type: &str,
    resource: &str,
    expected: &str,
) {
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: expected {op} to answer 400 ResourceNotFoundException, got {} with \
         body {}",
        answer.status,
        answer.body
    );
    for (member, want) in [("ResourceType", resource_type), ("Resource", resource)] {
        assert_eq!(
            member_of(&answer.body, member).as_str(),
            Some(want),
            "case {case:?}: expected {op} to report {member} {want:?}, got {}",
            answer.body
        );
    }
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {op} to answer the service's fixed text {expected:?}, got \
         {message:?}"
    );
    assert!(
        !message.contains(resource),
        "case {case:?}: expected {op}'s refusal to carry no identifier, got {message:?}"
    );
}

/// Create one agreement on `server_id` between `parties`, carrying `members` beyond the
/// server, the two profiles and the access role, and hand back its `AgreementId`.
async fn create_agreement(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    parties: (&str, &str),
    members: &Value,
) -> String {
    let mut request = json!({
        "ServerId": server_id,
        "LocalProfileId": parties.0,
        "PartnerProfileId": parties.1,
        "AccessRole": ROLE,
    });
    let object = request.as_object_mut().expect("the request is an object");
    for (member, value) in members.as_object().expect("the members are an object") {
        object.insert(member.clone(), value.clone());
    }
    emulator.create_agreement(case, request).await
}

/// The two AS2 profiles every agreement in this module is written between.
async fn parties(emulator: &Emulator) -> (String, String) {
    (
        emulator
            .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
            .await,
        emulator
            .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
            .await,
    )
}

/// RULE: a `(ServerId, AgreementId)` pair the store does not hold is refused, by EVERY
/// operation of the agreement plane — the reader and the mutations alike — with the
/// modelled `ResourceNotFoundException` carrying the service's fixed agreement text, so
/// the planes cannot disagree about whether the agreement is gone and none of them
/// answers outside the operation's modelled error set.
///
/// The rows differ along how the pair came not to be held: its own delete removed it, the
/// delete of the SERVER it was written on took it (with the agreement ACTIVE, and with it
/// INACTIVE and configured through `CustomDirectories`), the identifier was never minted,
/// and neither half of the pair was.
#[tokio::test]
async fn a_pair_the_store_does_not_hold_is_refused_with_one_fixed_text_on_every_plane() {
    let emulator = Emulator::new();
    let (local, partner) = parties(&emulator).await;
    let parties = (local.as_str(), partner.as_str());

    // (case, the pair every operation below is addressed at)
    let mut rows: Vec<(String, String, String)> = Vec::new();

    // Its own delete took it.
    {
        let case = "the agreement its own delete removed";
        let server_id = emulator.create_server(json!({})).await;
        let agreement_id = create_agreement(
            &emulator,
            case,
            &server_id,
            parties,
            &json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2"}),
        )
        .await;
        let deleted = emulator
            .call(
                "DeleteAgreement",
                json!({"ServerId": &server_id, "AgreementId": &agreement_id}),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgreement 200, got {} with body {}",
            deleted.status, deleted.body
        );
        rows.push((case.to_string(), server_id, agreement_id));
    }

    // The delete of the server it was written on took it. The server's delete is served
    // while the agreement still references it, whatever the agreement's own Status, so
    // the agreement is left addressed at a server the store no longer holds.
    for (case, members) in [
        (
            "an ACTIVE agreement the delete of its server took",
            json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2", "Status": "ACTIVE"}),
        ),
        (
            "an INACTIVE custom-directories agreement the delete of its server took",
            json!({
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/custom"),
                "Status": "INACTIVE",
            }),
        ),
    ] {
        let server_id = emulator.create_server(json!({})).await;
        let agreement_id =
            create_agreement(&emulator, case, &server_id, parties, &members).await;

        // The agreement really is held, in the configuration the row created it with,
        // before the server's delete is what takes it away.
        let read = emulator.describe_agreement(&server_id, &agreement_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the agreement to be readable before the delete, got {} \
             with body {}",
            read.status, read.body
        );
        assert_eq!(
            member_of(read.get("Agreement"), "Status").as_str(),
            member_of(&members, "Status").as_str(),
            "case {case:?}: expected the agreement to hold the Status it was created with, \
             got {}",
            read.body
        );

        let deleted = emulator
            .call("DeleteServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteServer to be served while the agreement \
             referenced the server, got {} with body {}",
            deleted.status, deleted.body
        );
        rows.push((case.to_string(), server_id, agreement_id));
    }

    // An identifier nothing minted, on a server that exists; and a pair neither half of
    // which was ever minted.
    let live_server = emulator.create_server(json!({})).await;
    for agreement_id in NEVER_MINTED_AGREEMENTS {
        rows.push((
            format!("the never-minted {agreement_id} on a server that exists"),
            live_server.clone(),
            agreement_id.to_string(),
        ));
    }
    for server_id in [UNMINTED_SERVER_ID, NEVER_MINTED_SERVER] {
        rows.push((
            format!("a pair on the never-minted {server_id}"),
            server_id.to_string(),
            NEVER_MINTED_AGREEMENTS[0].to_string(),
        ));
    }

    for (case, server_id, agreement_id) in &rows {
        for (op, request) in [
            (
                "DescribeAgreement",
                json!({"ServerId": server_id, "AgreementId": agreement_id}),
            ),
            (
                "DeleteAgreement",
                json!({"ServerId": server_id, "AgreementId": agreement_id}),
            ),
            (
                "UpdateAgreement",
                json!({
                    "ServerId": server_id,
                    "AgreementId": agreement_id,
                    "Status": "INACTIVE",
                }),
            ),
        ] {
            let answer = emulator.call(op, request).await;
            assert_fixed_not_found(case, op, &answer, "Agreement", agreement_id, UNKNOWN_AGREEMENT);
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.contains(server_id),
                "case {case:?}: expected {op}'s refusal to carry nothing the request sent, \
                 got {message:?} carrying {server_id:?}"
            );
        }
    }
}

/// RULE (the same condition, one kind up): a `ServerId` the store does not hold is
/// refused, by every operation of the server plane, with the modelled
/// `ResourceNotFoundException` carrying the service's fixed server text — so a server
/// whose delete was served reads as gone on every plane, as the agreements it took with it
/// do.
///
/// The rows differ along how the server came not to be held: its delete was served while
/// an ACTIVE agreement referenced it, its delete was served with nothing written on it,
/// and the identifier was never minted (both ends of the id space).
#[tokio::test]
async fn a_server_the_store_does_not_hold_is_refused_with_one_fixed_text_on_every_plane() {
    let emulator = Emulator::new();
    let (local, partner) = parties(&emulator).await;
    let parties = (local.as_str(), partner.as_str());

    // (case, the ServerId every operation below is addressed at)
    let mut rows: Vec<(String, String)> = Vec::new();

    for (case, with_agreement) in [
        ("a server whose delete was served while an ACTIVE agreement referenced it", true),
        ("a server whose delete was served with nothing written on it", false),
    ] {
        let server_id = emulator.create_server(json!({})).await;
        if with_agreement {
            create_agreement(
                &emulator,
                case,
                &server_id,
                parties,
                &json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2", "Status": "ACTIVE"}),
            )
            .await;
        }
        let deleted = emulator
            .call("DeleteServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteServer 200, got {} with body {}",
            deleted.status, deleted.body
        );
        rows.push((case.to_string(), server_id));
    }
    for server_id in [UNMINTED_SERVER_ID, NEVER_MINTED_SERVER] {
        rows.push((
            format!("the never-minted {server_id}"),
            server_id.to_string(),
        ));
    }

    for (case, server_id) in &rows {
        for op in ["DescribeServer", "DeleteServer", "StartServer", "StopServer"] {
            let answer = emulator.call(op, json!({"ServerId": server_id})).await;
            assert_fixed_not_found(case, op, &answer, "Server", server_id, UNKNOWN_SERVER);
        }
    }
}

// ---------------------------------------------------------------------------
// 5. A write of a web app's customization that lost the claim
// ---------------------------------------------------------------------------

/// The text the service states for an UPDATE of a web app's customization it could not
/// carry out because another write of the same object was being applied.
const UPDATE_CUSTOMIZATION_CONFLICT: &str = "Unable to update WebApp customization";

/// The text it states for a DELETE of that one object, on the same basis: the sentence is
/// the write's own, so the refusal describes the write that lost rather than some other
/// one.
const DELETE_CUSTOMIZATION_CONFLICT: &str = "Unable to delete WebApp customization";

/// RULE: of writes of ONE web app's customization that overlap, every loser is refused
/// with the modelled `ConflictException` (HTTP 409 for both writing operations,
/// `smithy_query(query="operation",operation="UpdateWebAppCustomization,DeleteWebAppCustomization")`)
/// whose whole `Message` is the service's fixed text for the OPERATION that lost —
/// carrying nothing the request submitted, and the same text however many writers
/// overlapped.
///
/// The rows differ along which operation lost, how many writers overlap, and whether the
/// web app held a customization already.
#[tokio::test]
async fn a_customization_write_that_lost_the_claim_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();

    // (case, the operation every racer calls, how many overlap, whether the web app holds
    // a customization when they arrive)
    let rows: [(&str, &str, usize, bool); 6] = [
        ("two overlapping deletes", "DeleteWebAppCustomization", 2, true),
        ("four overlapping deletes", "DeleteWebAppCustomization", 4, true),
        ("eight overlapping deletes", "DeleteWebAppCustomization", 8, true),
        (
            "eight overlapping deletes of a web app holding no customization",
            "DeleteWebAppCustomization",
            8,
            false,
        ),
        ("two overlapping updates", "UpdateWebAppCustomization", 2, true),
        ("eight overlapping updates", "UpdateWebAppCustomization", 8, true),
    ];

    for (case, op, racers, held) in rows {
        let web_app_id = create_web_app(&emulator).await;
        if held {
            let seeded = emulator
                .call(
                    "UpdateWebAppCustomization",
                    json!({"WebAppId": &web_app_id, "Title": "seeded"}),
                )
                .await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: expected the seeding update to be served, got {} with body {}",
                seeded.status, seeded.body
            );
        }

        // Every racer submits a value of its own where the operation takes one, so a
        // refusal that echoed the request would differ between them.
        let titles: Vec<String> = (0..racers).map(|index| format!("racer-{index}")).collect();
        let calls = titles.iter().map(|title| {
            let mut request = json!({"WebAppId": &web_app_id});
            if op == "UpdateWebAppCustomization" {
                request["Title"] = json!(title);
            }
            emulator.call(op, request)
        });
        let answers = futures_of(calls).await;

        let served = answers.iter().filter(|answer| answer.status == 200).count();
        assert_eq!(
            served,
            1,
            "case {case:?}: expected exactly one winning write, got the statuses {:?}",
            answers.iter().map(|answer| answer.status).collect::<Vec<_>>()
        );

        let expected = if op == "DeleteWebAppCustomization" {
            DELETE_CUSTOMIZATION_CONFLICT
        } else {
            UPDATE_CUSTOMIZATION_CONFLICT
        };
        let mut refused = 0;
        for (index, answer) in answers
            .iter()
            .enumerate()
            .filter(|(_, answer)| answer.status != 200)
        {
            assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (409, "ConflictException"),
                "case {case:?} (racer {index} lost): expected 409 ConflictException, got {} \
                 with body {}",
                answer.status,
                answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message, expected,
                "case {case:?} (racer {index} lost): expected the service's fixed text \
                 {expected:?}, got {message:?}"
            );
            for value in titles
                .iter()
                .map(String::as_str)
                .chain([web_app_id.as_str()])
            {
                assert!(
                    !message.contains(value),
                    "case {case:?} (racer {index} lost): expected no request data in Message, \
                     got {message:?} carrying {value:?}"
                );
            }
            refused += 1;
        }
        assert_eq!(
            refused,
            racers - 1,
            "case {case:?}: expected every writer but the winner to be refused"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. A connector Url that names no endpoint the service could reach
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const URL_NOT_VALID: &str = "The specified Url is not valid";

/// A connector `Url` that names no reachable endpoint, with the protocol configuration
/// the row sends it under — the exclusivity requirement is resolved before the `Url`, so
/// every row names exactly one configuration.
struct UnreachableUrl {
    case: &'static str,
    url: &'static str,
    config: fn() -> Value,
}

/// An AS2 configuration, for a row whose `Url` names an AS2 endpoint.
fn as2_config() -> Value {
    json!({"As2Config": {
        "LocalProfileId": "p-0123456789abcdef0",
        "PartnerProfileId": "p-00000000000000001",
        "MdnResponse": "NONE",
        "Compression": "DISABLED",
        "EncryptionAlgorithm": "NONE",
        "SigningAlgorithm": "NONE",
    }})
}

/// An SFTP configuration, for a row whose `Url` names an SFTP endpoint.
fn sftp_config() -> Value {
    json!({"SftpConfig": {"UserSecretId": "sftp-partner-credentials"}})
}

/// The `Url` values that name no endpoint the service could ever connect to.
///
/// The rows differ along WHY the value names none — a host under each top-level domain
/// the DNS never delegates (`.invalid`, `.test`, `.localhost`, `.local`), a URL carrying
/// no authority, and a host no DNS name can take the shape of — crossed with the scheme
/// and the protocol configuration the connector would carry.
///
/// A value that carries no scheme at all is NOT here: it earns the service's OTHER
/// sentence, for a `Url` that is nothing but path information, which
/// `crate::connector_configuration` asserts beside this one.
fn unreachable_urls() -> Vec<UnreachableUrl> {
    vec![
        UnreachableUrl {
            case: "an AS2 endpoint whose host is under the reserved .invalid",
            url: "https://as2.partner.invalid/as2",
            config: as2_config,
        },
        UnreachableUrl {
            case: "an AS2 endpoint deeper under .invalid, on a port",
            url: "http://as2.partner.example.invalid:8080/as2",
            config: as2_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint under .invalid",
            url: "sftp://files.partner.invalid",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint under the reserved .test",
            url: "sftp://files.partner.test:2222",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint under the reserved .localhost",
            url: "sftp://files.partner.localhost",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint on the local link",
            url: "sftp://files.partner.local",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "a URL that carries no authority at all",
            url: "mailto:partner@example.com",
            config: as2_config,
        },
        UnreachableUrl {
            case: "a single-label host, which the DNS root holds no address for",
            url: "sftp://partner",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "a host whose label ends in a hyphen",
            url: "sftp://partner-.example.com",
            config: sftp_config,
        },
    ]
}

/// The `Url` values that DO name an endpoint, as the control the rule is bounded by:
/// delegated host names under every scheme a connector speaks, the documentation domain
/// `example.com` (which is delegated and resolves, unlike `.invalid`), a host name under
/// a reserved label that is not the top-level one, and the literals that need no
/// resolution.
fn reachable_urls() -> Vec<UnreachableUrl> {
    vec![
        UnreachableUrl {
            case: "an https AS2 endpoint with a path",
            url: "https://partner.example.com/as2",
            config: as2_config,
        },
        UnreachableUrl {
            case: "an http AS2 endpoint on a port",
            url: "http://partner.example.com:8080/as2",
            config: as2_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint",
            url: "sftp://partner.example.com",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint whose host carries the reserved label below the \
                   top-level one",
            url: "sftp://invalid.partner.example.com",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint addressed by an IPv4 literal",
            url: "sftp://192.0.2.10:22",
            config: sftp_config,
        },
        UnreachableUrl {
            case: "an SFTP endpoint addressed by an IPv6 literal",
            url: "sftp://[2001:db8::1]:22",
            config: sftp_config,
        },
    ]
}

/// The request a create of `row` sends: the access role, the row's `Url`, and the row's
/// one protocol configuration.
fn create_request(row: &UnreachableUrl) -> Value {
    let mut request = json!({"AccessRole": ROLE, "Url": row.url});
    let object = request.as_object_mut().expect("the request is an object");
    for (member, value) in (row.config)()
        .as_object()
        .expect("a configuration is an object")
    {
        object.insert(member.clone(), value.clone());
    }
    request
}

/// RULE: a connector `Url` that names no endpoint the service could reach — a value that
/// is no absolute URL, or whose host can never be resolved — is refused with the
/// service's fixed text on every operation that writes a connector, and nothing is
/// written: the refused create mints no connector and leaks none into `ListConnectors`,
/// and the refused update leaves the stored connector exactly as it was.
///
/// The rows are [`unreachable_urls`], crossed with the two operations that write a
/// connector, and bounded by [`reachable_urls`], which the same operations must still
/// serve.
#[tokio::test]
async fn a_connector_url_that_names_no_reachable_endpoint_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();

    // The connector the update rows address, written with an endpoint that does resolve.
    let stored_url = "sftp://partner.example.com";
    let created = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": stored_url,
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            }),
        )
        .await;
    let stored_id = connector_id("SFTP", &created);

    for row in unreachable_urls() {
        // CreateConnector: nothing is minted for a connector the service would refuse.
        let case = format!("CreateConnector with {}", row.case);
        let answer = emulator
            .call("CreateConnector", create_request(&row))
            .await;
        assert_fixed_refusal(&case, &answer, URL_NOT_VALID, &[row.url, ROLE]);
        assert!(
            member_of(&answer.body, "ConnectorId").is_null(),
            "case {case:?}: expected the refusal to mint no ConnectorId, got {}",
            answer.body
        );

        // UpdateConnector: the same value is refused on the stored connector, which
        // keeps the endpoint it was created with.
        let case = format!("UpdateConnector with {}", row.case);
        let answer = emulator
            .call(
                "UpdateConnector",
                json!({"ConnectorId": &stored_id, "Url": row.url}),
            )
            .await;
        assert_fixed_refusal(&case, &answer, URL_NOT_VALID, &[row.url, ROLE]);
        let described = emulator
            .call("DescribeConnector", json!({"ConnectorId": &stored_id}))
            .await;
        assert_eq!(
            member_of(described.get("Connector"), "Url").as_str(),
            Some(stored_url),
            "case {case:?}: expected the stored connector to keep its endpoint, got {}",
            described.body
        );
    }

    // Every endpoint that does resolve is still written, and is the only kind the store
    // holds: the refused creates above added nothing to the listing.
    let mut expected_urls = vec![stored_url.to_string()];
    for row in reachable_urls() {
        let answer = emulator
            .call("CreateConnector", create_request(&row))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the create to be served, got {} with body {}",
            row.case, answer.status, answer.body
        );
        expected_urls.push(row.url.to_string());
    }

    let (_pages, listed) =
        drain_pages(&emulator, "ListConnectors", &json!({}), "Connectors", "Url", None).await;
    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    expected_urls.sort();
    assert_eq!(
        listed_sorted, expected_urls,
        "expected the store to hold only the connectors whose Url names a reachable \
         endpoint, got {listed:?}"
    );
}
