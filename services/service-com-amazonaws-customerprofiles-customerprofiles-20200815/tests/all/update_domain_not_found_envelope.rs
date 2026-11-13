// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the envelope `UpdateDomain` refuses an absent domain in.
//!
//! The rule under test: whether a modelled refusal words prose is a property of the
//! OPERATION that raises it, not of the resource kind it is about. `ResourceNotFoundException`
//! declares a single, OPTIONAL `Message` member (`smithy_query(query="shape",
//! shape="ResourceNotFoundException")`), and an error document this service answers always
//! REPORTS that member: the sentence for a refusal that words one, and JSON `null` for a
//! refusal that words none — which is what lets a client tell "this refusal states no
//! message" from "the member was not reported" (an SDK reads a `null` as an absent message
//! and a missing key as the empty string it substitutes). The domain-configuration update
//! (`PUT /domains/{DomainName}`, per `smithy_query(query="operation",
//! operation="UpdateDomain")`) words no sentence for an unknown domain, while the
//! domain-scoped reads word theirs with one.
//!
//! Every case goes through the real router, so what is asserted is the wire body a client
//! parses. The HTTP plumbing and the create helpers are this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, Emulator};

/// The shape every one of these refusals names.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A name inside the modelled `name` charset (`^[a-zA-Z0-9_-]+$`, length 1..64, per
/// `smithy_query(query="operation", operation="UpdateDomain")`) at its published minimum
/// length, so the refusal under test is the only one in play.
const SHORTEST_NAME: &str = "d";

/// `UpdateDomain` on a domain the store does not hold answers 404 `ResourceNotFoundException`
/// whose `Message` member is reported as JSON `null` — the refusal words no sentence —
/// however the name came to be absent and whatever the update carried.
///
/// The rows differ along why the domain is absent (never created, created and then deleted,
/// a name differing from a live domain only in case, a name at the modelled minimum length,
/// a name at the modelled maximum length) and along the body the update carried (none at
/// all, a scalar, a nested structure, a map) — the envelope is the same for every one of
/// them.
#[tokio::test]
async fn updating_an_absent_domain_is_refused_with_a_null_message_member() {
    let emulator = Emulator::new();
    // A live domain, so a refusal cannot come from an empty store, and one that is created
    // and then deleted, which is the ordinary "mutate something that was deleted" path.
    emulator.create_domain("alive", 90).await;
    emulator.create_domain("recycled", 90).await;
    let (status, body) = emulator.send("DELETE", "/domains/recycled", None).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteDomain 200, got {status} {body:?}"
    );

    // (case, the absent name, the update body)
    let longest_name = "n".repeat(64);
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a name nothing ever created",
            "nevercreated".to_string(),
            json!({ "DefaultExpirationDays": 90 }),
        ),
        (
            "a name created earlier and deleted",
            "recycled".to_string(),
            json!({ "DefaultExpirationDays": 7 }),
        ),
        (
            "a name differing from a live domain only in case",
            "ALIVE".to_string(),
            json!({ "DefaultExpirationDays": 1098 }),
        ),
        (
            "an absent name at the modelled minimum length",
            SHORTEST_NAME.to_string(),
            json!({}),
        ),
        (
            "an absent name at the modelled maximum length",
            longest_name,
            json!({ "Tags": { "env": "dev" } }),
        ),
        (
            "an update carrying a nested configuration",
            "nested_absent".to_string(),
            json!({ "Matching": { "Enabled": true } }),
        ),
        (
            "an update carrying an empty string that would clear a member",
            "clear-absent".to_string(),
            json!({ "DefaultEncryptionKey": "" }),
        ),
    ];

    for (case, name, update) in &cases {
        let (status, error_type, body) = emulator
            .send_parts("PUT", &format!("/domains/{name}"), Some(update.clone()))
            .await;
        assert_eq!(
            (status, error_code(&body), error_type.as_str()),
            (404, NOT_FOUND, NOT_FOUND),
            "case {case:?}: expected 404 {NOT_FOUND} in the body and the error-type header, \
             got {status} {error_type:?} {body:?}"
        );
        assert_eq!(
            body.get("Message"),
            Some(&Value::Null),
            "case {case:?}: expected the update's refusal to report a null Message, got {:?} \
             (whole body {body:?})",
            body.get("Message")
        );
        // Nothing else leaks into the envelope either: the shape discriminator and that one
        // member are all the body carries.
        let members: Vec<&String> = body
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .filter(|key| *key != "__type" && *key != "Message")
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            members.is_empty(),
            "case {case:?}: expected the refusal body to carry only the shape, got {members:?} \
             in {body:?}"
        );
    }

    // The refused updates created nothing, and the live domain is untouched.
    for (case, name, _) in &cases {
        let (status, read) = emulator.send("GET", &format!("/domains/{name}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must create nothing, got {status} {read:?}"
        );
    }
    let (status, alive) = emulator.send("GET", "/domains/alive", None).await;
    assert_eq!(
        (status, alive.get("DefaultExpirationDays").and_then(Value::as_i64)),
        (200, Some(90)),
        "the live domain must be unchanged, got {status} {alive:?}"
    );
}

/// The same absent name, asked of the operations whose refusal DOES word prose, still
/// carries it: the presence of a sentence follows the operation, so the null-message
/// envelope above is not the store forgetting how to word a refusal.
///
/// The rows are the domain-scoped operations this crate words with the domain sentence —
/// the read (`GET /domains/{DomainName}`) and the delete (`DELETE /domains/{DomainName}`),
/// per `smithy_query(query="operation", operation="GetDomain,DeleteDomain")` — asked for
/// names that are absent for different reasons.
#[tokio::test]
async fn the_reads_of_the_same_absent_domain_still_carry_their_sentence() {
    let emulator = Emulator::new();
    emulator.create_domain("settled", 90).await;
    let (status, body) = emulator.send("DELETE", "/domains/settled", None).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteDomain 200, got {status} {body:?}"
    );

    for name in ["settled", "nevercreated", SHORTEST_NAME] {
        for (method, path) in [
            ("GET", format!("/domains/{name}")),
            ("DELETE", format!("/domains/{name}")),
        ] {
            let case = format!("{method} {path}");
            let (status, error_type, body) = emulator.send_parts(method, &path, None).await;
            assert_eq!(
                (status, error_code(&body), error_type.as_str()),
                (404, NOT_FOUND, NOT_FOUND),
                "case {case:?}: expected 404 {NOT_FOUND}, got {status} {error_type:?} {body:?}"
            );
            let expected = format!("Domain {name} is not found");
            assert_eq!(
                body.get("Message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected the sentence {expected:?}, got {:?} (whole body \
                 {body:?})",
                body.get("Message")
            );
        }
    }

    // And an update of a name one of those reads just refused still words no sentence, so
    // the two envelopes are observed against the same store in the same test.
    let (status, _, body) = emulator
        .send_parts(
            "PUT",
            "/domains/settled",
            Some(json!({ "DefaultExpirationDays": 30 })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body), body.get("Message")),
        (404, NOT_FOUND, Some(&Value::Null)),
        "case \"the update of the name the reads refused\": expected a 404 {NOT_FOUND} \
         reporting a null Message, got {status} {body:?}"
    );
}
