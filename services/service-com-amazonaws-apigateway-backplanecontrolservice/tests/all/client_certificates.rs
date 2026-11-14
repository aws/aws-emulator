// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an account's client certificates: `GenerateClientCertificate`,
//! `GetClientCertificate`, `GetClientCertificates`, `UpdateClientCertificate` and
//! `DeleteClientCertificate`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULES under test:
//!
//! * a generated client certificate is PERSISTED under an id the service mints, and every
//!   later reading of it — its own read, and the account's collection — reports that record
//!   member for member: the description the request set as it set it, nothing for the
//!   members it did not, and the certificate the SERVICE generated for it;
//! * the material is API Gateway's own: a real PEM-encoded X.509 certificate, distinct per
//!   record, valid over the record's own window, which runs from `createdDate` for
//!   [`CLIENT_CERTIFICATE_VALIDITY`];
//! * `description` is the one member a patch changes; a pointer naming a member the service
//!   itself assigns is the modelled `BadRequestException` (400) and leaves the record as it
//!   was;
//! * a `clientCertificateId` no stored certificate answers to is the modelled
//!   `NotFoundException` (404) on every operation that takes one, and a delete a stage's
//!   reference still stands in the way of is the modelled `BadRequestException` (400);
//! * a generate past the account's published certificate quota is the modelled
//!   `LimitExceededException` (429), and a request this service will not store is the
//!   modelled `BadRequestException` (400) — neither is a 501.
//!
//! The tables span what the rules generalise over rather than one recorded call: which
//! optional members a request carries, how long a description is and which characters it
//! uses, how many tags it sets, the forms an unknown id takes, which member a patch points
//! at, and the page sizes a collection is walked by.

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    CLIENT_CERTIFICATE_VALIDITY, MAX_CLIENT_CERTIFICATES, MAX_STRING_CHARS,
};

use crate::base_path_mapping::assert_refusal;
use crate::patch_updates::{op, patch, replace};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployed_stage;
use crate::tag_reads_by_arn::{apigateway_arn, get_tags};

/// `GenerateClientCertificate` with `request`, answering the whole response.
async fn generate(gateway: &Gateway, request: Value) -> Answer {
    gateway
        .call("POST", "/clientcertificates", Some(request))
        .await
}

/// `GetClientCertificate` for `client_certificate_id`.
async fn get_certificate(gateway: &Gateway, client_certificate_id: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/clientcertificates/{client_certificate_id}"),
            None,
        )
        .await
}

/// `GetClientCertificates`, with `query` appended to the collection URI (`""` for none).
async fn list_certificates(gateway: &Gateway, query: &str) -> Answer {
    gateway
        .call("GET", &format!("/clientcertificates{query}"), None)
        .await
}

/// `DeleteClientCertificate` for `client_certificate_id`.
async fn delete_certificate(gateway: &Gateway, client_certificate_id: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/clientcertificates/{client_certificate_id}"),
            None,
        )
        .await
}

/// `UpdateClientCertificate` for `client_certificate_id` with `operations`.
async fn patch_certificate(
    gateway: &Gateway,
    client_certificate_id: &str,
    operations: Vec<Value>,
) -> Answer {
    patch(
        gateway,
        &format!("/clientcertificates/{client_certificate_id}"),
        operations,
    )
    .await
}

/// A client certificate generated with `request`, asserting the modelled 201 and answering
/// the whole body.
async fn generated(gateway: &Gateway, case: &str, request: Value) -> Value {
    let answer = generate(gateway, request).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from GenerateClientCertificate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A bare client certificate, for a case whose rule is about something other than the
/// request: its id.
async fn bare_certificate(gateway: &Gateway, case: &str) -> String {
    let body = generated(gateway, case, json!({})).await;
    string_member(&body, "clientCertificateId", case)
}

/// A required epoch-seconds timestamp member of a body.
fn seconds(body: &Value, member: &str, case: &str) -> i64 {
    body[member].as_i64().unwrap_or_else(|| {
        panic!("case {case}: expected a whole-second epoch {member}, got {body:?}")
    })
}

/// Assert `pem` is a real PEM-encoded X.509 certificate: the armour an X.509 certificate is
/// published in, whose body is base64 that decodes to a DER SEQUENCE spanning every decoded
/// byte — which is what a client that parses the member gets, rather than text in the shape
/// of a certificate.
fn assert_pem_certificate(pem: &str, case: &str) {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let trimmed = pem.trim_end();
    let body = trimmed
        .strip_prefix(BEGIN)
        .and_then(|rest| rest.trim_end().strip_suffix(END))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a {BEGIN} … {END} document, got {pem:?}")
        });
    let encoded: String = body.split_whitespace().collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap_or_else(|error| {
            panic!("case {case}: expected a base64 certificate body, got {error} in {pem:?}")
        });
    // DER: a certificate is a SEQUENCE (tag 0x30) carrying a long-form length, and the
    // length it declares covers exactly the bytes that follow it.
    assert_eq!(
        der.first(),
        Some(&0x30u8),
        "case {case}: expected a DER SEQUENCE, got {:?} in {pem:?}",
        der.first()
    );
    let length_octets = usize::from(der.get(1).copied().unwrap_or(0) & 0x7f);
    assert!(
        der.get(1).copied().unwrap_or(0) & 0x80 != 0 && length_octets > 0,
        "case {case}: expected a long-form DER length, got {:?} in {pem:?}",
        der.get(1)
    );
    let declared = der
        .get(2..2 + length_octets)
        .unwrap_or_else(|| panic!("case {case}: truncated DER length in {pem:?}"))
        .iter()
        .fold(0usize, |total, byte| (total << 8) | usize::from(*byte));
    assert_eq!(
        declared,
        der.len() - 2 - length_octets,
        "case {case}: the DER SEQUENCE must span the whole certificate, got {declared} \
         of {} bytes",
        der.len() - 2 - length_octets
    );
}

/// Assert the `ClientCertificate` body `body` is a well-formed representation of a record
/// the service generated: a minted id, the material it issued, and the validity window that
/// material is stamped with.
fn assert_generated_representation(body: &Value, case: &str) {
    let id = string_member(body, "clientCertificateId", case);
    assert_eq!(
        id.chars().count(),
        10,
        "case {case}: a client certificate id is 10 characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "case {case}: a client certificate id is lowercase alphanumeric, got {id:?}"
    );
    assert_pem_certificate(&string_member(body, "pemEncodedCertificate", case), case);

    let created = seconds(body, "createdDate", case);
    let expires = seconds(body, "expirationDate", case);
    let validity = i64::try_from(CLIENT_CERTIFICATE_VALIDITY.as_secs()).expect("a span in seconds");
    assert_eq!(
        expires - created,
        validity,
        "case {case}: a generated certificate expires {validity} seconds after it was \
         created, got createdDate {created} and expirationDate {expires}"
    );
}

#[tokio::test]
async fn a_generated_client_certificate_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: whether the request describes the
    // certificate, how long that description is and which characters it uses, and how many
    // tags it sets.
    let rows: Vec<(&str, Value)> = vec![
        ("every member absent", json!({})),
        (
            "described",
            json!({ "description": "the backend's client certificate" }),
        ),
        ("an empty description", json!({ "description": "" })),
        (
            "a description at the longest this service stores",
            json!({ "description": "d".repeat(MAX_STRING_CHARS) }),
        ),
        (
            "a description outside ASCII",
            json!({ "description": "zertifikat für die rückwärtige schnittstelle ✅" }),
        ),
        ("tagged once", json!({ "tags": { "team": "api" } })),
        (
            "described and tagged twice",
            json!({
                "description": "described and tagged",
                "tags": { "team": "api", "env": "test" },
            }),
        ),
    ];

    let gateway = Gateway::new();
    let mut minted: Vec<String> = Vec::new();
    let mut materials: Vec<String> = Vec::new();
    for (case, request) in &rows {
        let created = generated(&gateway, case, request.clone()).await;
        assert_generated_representation(&created, case);

        let id = string_member(&created, "clientCertificateId", case);
        assert!(
            !minted.contains(&id),
            "case {case}: every certificate gets its own id, got {id:?} twice in {minted:?}"
        );
        minted.push(id.clone());

        // The material is this record's own: API Gateway generates a certificate per
        // record, so no two records share one.
        let material = string_member(&created, "pemEncodedCertificate", case);
        assert!(
            !materials.contains(&material),
            "case {case}: every certificate gets its own material, got the same PEM twice"
        );
        materials.push(material);

        // Every member the request set is reported as it set it, and the members it did
        // not set are absent — `description` and `tags` among them.
        for member in ["description", "tags"] {
            assert_eq!(
                created[member], request[member],
                "case {case}: expected {member} {:?}, got {:?}",
                request[member], created[member]
            );
        }

        // The READ reports the same record, member for member: one stored certificate, not
        // a body synthesised per call.
        let read = get_certificate(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetClientCertificate, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetClientCertificate must report the generated certificate, \
             got {:?} after {:?}",
            read.body, created
        );

        // And so does the account's collection, through the same body.
        let listed = list_certificates(&gateway, "").await;
        let items = page_items(&listed.body, case);
        assert!(
            items.contains(&created),
            "case {case}: the collection must report the generated certificate, got {items:?}"
        );
    }

    // Every certificate generated above is still readable: a generate does not displace
    // the certificates before it, and a read does not consume the one it reports.
    for id in &minted {
        let read = get_certificate(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetClientCertificate({id}) at the end: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "clientCertificateId", "the end"),
            *id,
            "case GetClientCertificate({id}): a certificate reports its own id, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_id_no_stored_certificate_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    // One stored certificate, so the refusals below are about the id and not about an
    // empty store.
    let stored = bare_certificate(&gateway, "the-stored-one").await;

    // Rows differ along the form of the id: an id of the shape this service mints, a
    // longer and a shorter one, the id in another case, and the stored id with its last
    // character stepped on — as close to a real id as a wrong id gets.
    let mut rows: Vec<(String, String)> = vec![
        ("an unminted id".to_string(), "zzzzz99999".to_string()),
        ("a short id".to_string(), "abc".to_string()),
        (
            "a long id".to_string(),
            "abcdefghij0123456789".to_string(),
        ),
        (
            "the stored id upper-cased".to_string(),
            stored.to_uppercase(),
        ),
    ];
    let mut near_miss: Vec<char> = stored.chars().collect();
    if let Some(last) = near_miss.pop() {
        near_miss.push(if last == 'a' { 'b' } else { 'a' });
    }
    rows.push((
        "the stored id with one character changed".to_string(),
        near_miss.into_iter().collect(),
    ));

    for (case, id) in &rows {
        assert_failure(
            &get_certificate(&gateway, id).await,
            404,
            "NotFoundException",
            &format!("GetClientCertificate / {case}"),
        );
        assert_failure(
            &patch_certificate(&gateway, id, vec![replace("/description", "patched")]).await,
            404,
            "NotFoundException",
            &format!("UpdateClientCertificate / {case}"),
        );
        assert_failure(
            &delete_certificate(&gateway, id).await,
            404,
            "NotFoundException",
            &format!("DeleteClientCertificate / {case}"),
        );
    }

    // The stored certificate is untouched by every refusal above.
    let read = get_certificate(&gateway, &stored).await;
    assert_eq!(
        read.status, 200,
        "case GetClientCertificate(the stored certificate): expected 200, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_patch_changes_the_description_and_nothing_the_service_assigned() {
    // Rows differ along the op and the value: a `replace` of a description the record has,
    // a `replace` of one it does not, an `add` that sets it, a `remove` that clears it, an
    // empty value, and an empty list that changes nothing.
    let rows: Vec<(&str, Value, Vec<Value>, Value)> = vec![
        (
            "replace a description the record carries",
            json!({ "description": "before" }),
            vec![replace("/description", "after")],
            json!("after"),
        ),
        (
            "replace a description the record does not carry",
            json!({}),
            vec![replace("/description", "set by the patch")],
            json!("set by the patch"),
        ),
        (
            "add sets the description",
            json!({}),
            vec![op("add", "/description", Some("added"))],
            json!("added"),
        ),
        (
            "remove clears the description",
            json!({ "description": "to be cleared" }),
            vec![op("remove", "/description", None)],
            Value::Null,
        ),
        (
            "an empty value is a description of no characters",
            json!({ "description": "before" }),
            vec![replace("/description", "")],
            json!(""),
        ),
        (
            "two ops on one member, the last one wins",
            json!({}),
            vec![replace("/description", "first"), replace("/description", "second")],
            json!("second"),
        ),
        (
            "an empty patch list changes nothing",
            json!({ "description": "unchanged" }),
            Vec::new(),
            json!("unchanged"),
        ),
    ];

    let gateway = Gateway::new();
    for (case, request, operations, expected) in &rows {
        let created = generated(&gateway, case, request.clone()).await;
        let id = string_member(&created, "clientCertificateId", case);

        let patched = patch_certificate(&gateway, &id, operations.clone()).await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateClientCertificate, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            patched.body["description"], *expected,
            "case {case}: expected description {expected:?}, got {:?}",
            patched.body["description"]
        );
        // Nothing the SERVICE assigned moved: the id, the material it generated and the
        // window that material is valid over are what they were.
        for member in [
            "clientCertificateId",
            "pemEncodedCertificate",
            "createdDate",
            "expirationDate",
        ] {
            assert_eq!(
                patched.body[member], created[member],
                "case {case}: a patch must leave {member} as {:?}, got {:?}",
                created[member], patched.body[member]
            );
        }

        // The patch is PERSISTED: the read reports what the update answered.
        let read = get_certificate(&gateway, &id).await;
        assert_eq!(
            read.body, patched.body,
            "case {case}: GetClientCertificate must report the patched certificate, \
             got {:?} after {:?}",
            read.body, patched.body
        );
    }
}

#[tokio::test]
async fn a_patch_of_a_member_the_service_assigns_is_the_modelled_bad_request() {
    let gateway = Gateway::new();
    let created = generated(
        &gateway,
        "the patched certificate",
        json!({ "description": "before any refusal", "tags": { "team": "api" } }),
    )
    .await;
    let id = string_member(&created, "clientCertificateId", "the patched certificate");

    // Rows differ along WHICH member the pointer names and HOW it is spelled: each of the
    // four members the service itself assigns, the tag collection the tag operations own, a
    // member of no resource, a pointer reaching into a single-valued member, a pointer that
    // is no JSON Pointer at all, and the ops no path of this resource takes.
    let rows: Vec<(&str, Vec<Value>)> = vec![
        (
            "replace the generated material",
            vec![replace(
                "/pemEncodedCertificate",
                "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----",
            )],
        ),
        (
            "replace the record's own id",
            vec![replace("/clientCertificateId", "zzzzz99999")],
        ),
        (
            "replace the creation date",
            vec![replace("/createdDate", "1700000000")],
        ),
        (
            "replace the expiration date",
            vec![replace("/expirationDate", "1700000000")],
        ),
        (
            "replace the tag collection",
            vec![replace("/tags", "{}")],
        ),
        (
            "a member of no resource",
            vec![replace("/notAMember", "value")],
        ),
        (
            "a pointer into a single-valued member",
            vec![replace("/description/0", "value")],
        ),
        ("no JSON Pointer at all", vec![replace("description", "value")]),
        (
            "an add of a path no collection of this resource publishes",
            vec![op("add", "/tags", Some("value"))],
        ),
        (
            "a remove of the generated material",
            vec![op("remove", "/pemEncodedCertificate", None)],
        ),
        (
            "an op no path of this resource takes",
            vec![op("copy", "/description", Some("value"))],
        ),
        (
            "a good op followed by a refused one",
            vec![
                replace("/description", "would have been applied"),
                replace("/clientCertificateId", "zzzzz99999"),
            ],
        ),
    ];

    for (case, operations) in &rows {
        assert_failure(
            &patch_certificate(&gateway, &id, operations.clone()).await,
            400,
            "BadRequestException",
            case,
        );
        // Every refusal left the record exactly as it was — including the row whose first
        // op would have been applied had the second one not been refused.
        let read = get_certificate(&gateway, &id).await;
        assert_eq!(
            read.body, created,
            "case {case}: the refused patch must leave the record as it was, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_certificate_a_stage_presents_cannot_be_deleted_until_the_stage_releases_it() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "client-certificate-stage").await;
    let assigned = bare_certificate(&gateway, "the assigned certificate").await;
    let unassigned = bare_certificate(&gateway, "the unassigned certificate").await;

    // A certificate NO stage names is deleted, which is what tells the refusals below
    // apart from a delete this service refuses outright.
    let deleted = delete_certificate(&gateway, &unassigned).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteClientCertificate(unassigned): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_certificate(&gateway, &unassigned).await,
        404,
        "NotFoundException",
        "GetClientCertificate after the delete",
    );
    assert_failure(
        &delete_certificate(&gateway, &unassigned).await,
        404,
        "NotFoundException",
        "a second DeleteClientCertificate of one record",
    );

    // Rows differ along WHICH stage presents the certificate, and along HOW the reference
    // is released: a `remove` of the member, and the empty value, which names no
    // certificate and asks for none.
    for (stage_name, release) in [
        ("prod", op("remove", "/clientCertificateId", None)),
        ("certstage", replace("/clientCertificateId", "")),
    ] {
        create_deployed_stage(&gateway, &api, stage_name).await;
        // The stage PRESENTS the certificate, which is the reference the delete is refused
        // while it stands.
        let stage = patch(
            &gateway,
            &format!("/restapis/{api}/stages/{stage_name}"),
            vec![replace("/clientCertificateId", &assigned)],
        )
        .await;
        assert_eq!(
            stage.status, 200,
            "case UpdateStage({stage_name}, /clientCertificateId): expected 200, \
             got {} with {:?}",
            stage.status, stage.body
        );

        assert_refusal(
            &delete_certificate(&gateway, &assigned).await,
            (400, "BadRequestException"),
            &format!(
                "Cannot delete client certificate. Certificate ID '{assigned}', is \
                 referenced in stage {stage_name}."
            ),
            &format!("DeleteClientCertificate while stage {stage_name} presents it"),
        );
        // The refusal removed nothing: the certificate is still the account's and still
        // the stage's.
        let read = get_certificate(&gateway, &assigned).await;
        assert_eq!(
            read.status, 200,
            "case GetClientCertificate after the refused delete: expected 200, \
             got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            crate::stage_lifecycle::get_stage(&gateway, &api, stage_name)
                .await
                .body["clientCertificateId"]
                .as_str(),
            Some(assigned.as_str()),
            "case the stage after the refused delete: expected it to still present \
             {assigned:?}"
        );

        // Releasing the reference is the caller's own `UpdateStage`.
        let released = patch(
            &gateway,
            &format!("/restapis/{api}/stages/{stage_name}"),
            vec![release],
        )
        .await;
        assert_eq!(
            released.status, 200,
            "case UpdateStage({stage_name}, release the certificate): expected 200, \
             got {} with {:?}",
            released.status, released.body
        );
        assert_eq!(
            released.body["clientCertificateId"],
            Value::Null,
            "case UpdateStage({stage_name}, release the certificate): expected the stage \
             to present none, got {:?}",
            released.body["clientCertificateId"]
        );
    }

    // With no stage presenting it any more, the delete the references were standing in the
    // way of succeeds.
    let deleted = delete_certificate(&gateway, &assigned).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteClientCertificate(released): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The account's collection reports neither record any more.
    let listed = list_certificates(&gateway, "").await;
    let items = page_items(&listed.body, "the collection after both deletes");
    assert!(
        items.is_empty(),
        "case the collection after both deletes: expected no certificate, got {items:?}"
    );
}

#[tokio::test]
async fn the_collection_is_listed_in_generation_order_one_page_at_a_time() {
    let gateway = Gateway::new();
    // An empty collection is the empty page rather than a refusal.
    let empty = list_certificates(&gateway, "").await;
    assert_eq!(
        empty.status, 200,
        "case GetClientCertificates(empty account): expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        page_items(&empty.body, "an empty account").is_empty(),
        "case an empty account: expected no certificate, got {:?}",
        empty.body
    );

    // Five certificates, generated in a known order.
    let mut generated_ids: Vec<String> = Vec::new();
    for index in 0..5 {
        let case = format!("certificate {index}");
        let body = generated(
            &gateway,
            &case,
            json!({ "description": format!("certificate {index}") }),
        )
        .await;
        generated_ids.push(string_member(&body, "clientCertificateId", &case));
    }

    let listed = list_certificates(&gateway, "").await;
    let ids: Vec<String> = page_items(&listed.body, "the default page")
        .iter()
        .map(|item| string_member(item, "clientCertificateId", "the default page"))
        .collect();
    assert_eq!(
        ids, generated_ids,
        "case the default page: expected the certificates in generation order, got {ids:?}"
    );

    // Rows differ along the page size a caller walks the collection by, including one
    // larger than the collection.
    for size in [1usize, 2, 5, 9] {
        let mut walked: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        loop {
            let query = match &position {
                None => format!("?limit={size}"),
                Some(cursor) => format!("?limit={size}&position={cursor}"),
            };
            let page = list_certificates(&gateway, &query).await;
            let case = format!("a walk by {size} at {position:?}");
            assert_eq!(
                page.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                page.status, page.body
            );
            let items = page_items(&page.body, &case);
            assert!(
                items.len() <= size,
                "case {case}: a page must hold at most {size} elements, got {}",
                items.len()
            );
            assert!(
                !items.is_empty(),
                "case {case}: a page of a collection that holds elements holds some, got \
                 {:?}",
                page.body
            );
            for item in &items {
                walked.push(string_member(item, "clientCertificateId", &case));
            }
            // The cursor a page reports is where the collection carries on, and a page
            // the collection holds nothing after reports none: that is what ends the walk.
            match page.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            walked, generated_ids,
            "case a walk by {size}: expected every certificate once, in generation order, \
             got {walked:?}"
        );
    }

    // A cursor this service did not issue for the collection is refused rather than
    // served an arbitrary page.
    for (case, cursor) in [
        ("an unissued cursor", "zzzzz99999".to_string()),
        ("a cursor of another form", "not-a-cursor".to_string()),
    ] {
        assert_failure(
            &list_certificates(&gateway, &format!("?position={cursor}")).await,
            400,
            "BadRequestException",
            case,
        );
    }
}

#[tokio::test]
async fn a_request_outside_what_the_service_stores_is_the_modelled_bad_request() {
    // Rows differ along which part of the request falls outside the rule: a description
    // longer than this service stores, a tag key in the reserved namespace, an oversized
    // tag key and value, and more tags than a resource carries.
    let mut rows: Vec<(&str, Value)> = vec![
        (
            "a description past the longest this service stores",
            json!({ "description": "d".repeat(MAX_STRING_CHARS + 1) }),
        ),
        (
            "a tag key in the reserved namespace",
            json!({ "tags": { "aws:owner": "someone" } }),
        ),
    ];
    let mut oversized_key = serde_json::Map::new();
    oversized_key.insert("k".repeat(200), json!("value"));
    rows.push((
        "an oversized tag key",
        json!({ "tags": Value::Object(oversized_key) }),
    ));
    let mut too_many = serde_json::Map::new();
    for index in 0..60 {
        too_many.insert(format!("key-{index}"), json!("value"));
    }
    rows.push((
        "more tags than a resource carries",
        json!({ "tags": Value::Object(too_many) }),
    ));

    let gateway = Gateway::new();
    for (case, request) in &rows {
        assert_failure(
            &generate(&gateway, request.clone()).await,
            400,
            "BadRequestException",
            case,
        );
    }

    // Nothing above was stored: the first certificate this account can read is the one the
    // next accepted request generates.
    let id = bare_certificate(&gateway, "after the refusals").await;
    let listed = list_certificates(&gateway, "").await;
    let ids: Vec<String> = page_items(&listed.body, "after the refusals")
        .iter()
        .map(|item| string_member(item, "clientCertificateId", "after the refusals"))
        .collect();
    assert_eq!(
        ids,
        vec![id],
        "case after the refusals: expected only the accepted certificate, got {ids:?}"
    );
}

#[tokio::test]
async fn a_generated_certificate_carries_the_accounts_tag_collection_for_it() {
    let gateway = Gateway::new();
    // Rows differ along how many tags the generate sets, including none.
    let rows: Vec<(&str, Value, Value)> = vec![
        ("untagged", json!({}), json!({})),
        (
            "tagged once",
            json!({ "tags": { "team": "api" } }),
            json!({ "team": "api" }),
        ),
        (
            "tagged twice",
            json!({ "tags": { "team": "api", "env": "test" } }),
            json!({ "team": "api", "env": "test" }),
        ),
    ];

    for (case, request, expected) in &rows {
        let created = generated(&gateway, case, request.clone()).await;
        let id = string_member(&created, "clientCertificateId", case);
        // The tag operations address a resource by ARN, and a generated certificate's ARN
        // resolves to the very collection the generate stored.
        let answer = get_tags(&gateway, &apigateway_arn(&format!("/clientcertificates/{id}"))).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from GetTags, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["tags"], *expected,
            "case {case}: expected the generated certificate's tags {expected:?}, got {:?}",
            answer.body["tags"]
        );
    }
}

#[tokio::test]
async fn a_generate_past_the_accounts_certificate_quota_is_the_modelled_limit_exceeded() {
    // The growth a caller drives is bounded at API Gateway's published quota for client
    // certificates in one Region (`MAX_CLIENT_CERTIFICATES`): every generate up to it is
    // accepted, the one past it is the modelled `LimitExceededException` (429), and the
    // certificates already stored are untouched by the refusal.
    let gateway = Gateway::new();
    let mut stored: Vec<String> = Vec::new();
    for index in 0..MAX_CLIENT_CERTIFICATES {
        let case = format!("GenerateClientCertificate({index}) within the quota");
        let body = generated(&gateway, &case, json!({})).await;
        stored.push(string_member(&body, "clientCertificateId", &case));
    }

    assert_failure(
        &generate(&gateway, json!({})).await,
        429,
        "LimitExceededException",
        &format!("GenerateClientCertificate past the quota of {MAX_CLIENT_CERTIFICATES}"),
    );

    for id in &stored {
        let read = get_certificate(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetClientCertificate({id}) after the refused generate: expected 200, \
             got {} with {:?}",
            read.status, read.body
        );
    }
}
