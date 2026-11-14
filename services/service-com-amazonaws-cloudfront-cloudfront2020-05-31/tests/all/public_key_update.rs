// SPDX-License-Identifier: CC0-1.0

//! `UpdatePublicKey` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this operation
//! ("Update public key information. Note that the only value you can change is the
//! comment"):
//!
//! * `Comment` is the ONE member an update may move: a request that resubmits the stored
//!   `CallerReference`, `Name` and `EncodedKey` unchanged beside a new comment lands, mints
//!   a fresh version token and is what `GetPublicKey` answers from afterwards — with the
//!   instant the create recorded, which no update moves;
//! * a request that submits a different value for any one of `CallerReference`, `Name` or
//!   `EncodedKey` is refused with `CannotChangeImmutablePublicKeyFields` (HTTP 400, "You
//!   cannot modify encoded material and name of a public key once created.") and leaves the
//!   stored record and its version token exactly as they were;
//! * the version token the last mutation handed back is the only one an `If-Match` against
//!   that key satisfies: an absent or malformed header is `InvalidIfMatchVersion` (HTTP
//!   400), which is a property of the REQUEST and so is answered for an id no key holds
//!   too, a well-formed token that is not the current one is `PreconditionFailed` (HTTP
//!   412), and the current one — bare, quoted or weak — is accepted;
//! * the documented comment bound holds on an update as it does on the create, so a comment
//!   past it is `InvalidArgument` (HTTP 400) and stores nothing;
//! * an id no stored key holds is `NoSuchPublicKey` (HTTP 404).
//!
//! Every read-back goes through BOTH readers of the one record — `GetPublicKey` and
//! `GetPublicKeyConfig` — so neither can drift from what the update stored.
//!
//! The rows differ along everything these rules generalise over: which immutable member the
//! request moves and how (a different value, an empty one, a longer one), the form of the
//! encoded key (single-line and multi-line PEM, long and short bodies), whether the comment
//! is absent, empty, ASCII, XML metacharacters or at the published 128-character bound, and
//! the form the `If-Match` header takes. The configuration writer and reader are the
//! crate's own, in `public_key_lifecycle.rs`; the recorded case is one row.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_PUBLIC_KEY_COMMENT, NO_SUCH_PUBLIC_KEY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};
use crate::response_headers_policy_lifecycle::text;

/// The sentence `CannotChangeImmutablePublicKeyFields` carries: it names the members that are
/// fixed for a key's life, which is what tells a caller that `Comment` is updatable while
/// `Name` and `EncodedKey` are not. Spelled out here as a literal rather than read back off
/// the crate's constant, so the WORDING is what these rows are held to.
const IMMUTABLE_FIELDS_MESSAGE: &str =
    "You cannot modify encoded material and name of a public key once created.";

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/public-key/{Id}/config` with the `PublicKeyConfig` document `body` and
/// the `If-Match` header `if_match` (left off entirely when `None`).
async fn update_key(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// `GET /2020-05-31/public-key/{Id}/config` — the config read of the same record.
async fn get_key_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// Upload `key` and report the id, the version token and the created time the create minted.
async fn seed(router: &MultiServiceRouter, key: &Key) -> (String, String, String) {
    let created = create_key(router, &key.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        key.case, created.status, created.raw
    );
    let document = created.root();
    let id = text(document, "Id", &key.case, "the create", &created.raw);
    let created_time = text(document, "CreatedTime", &key.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();
    (id, etag, created_time)
}

/// Assert `GetPublicKey` answers for `id` with `key`'s configuration, the created time
/// `created_time` and the version token `etag` — the one stored record.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    key: &Key,
    etag: &str,
    created_time: &str,
    context: &str,
) {
    let case = &key.case;
    let fetched = get_key(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case} ({context}): expected HTTP 200 from GetPublicKey, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.root().text_at("Id"),
        Some(id),
        "case {case} ({context}): expected the key to keep the id {id}, got {}",
        fetched.raw
    );
    assert_eq!(
        fetched.root().text_at("CreatedTime"),
        Some(created_time),
        "case {case} ({context}): expected the instant the create recorded ({created_time}), got \
         {}",
        fetched.raw
    );
    let config = fetched.root().at("PublicKeyConfig").unwrap_or_else(|| {
        panic!(
            "case {case} ({context}): the read carries no PublicKeyConfig: {}",
            fetched.raw
        )
    });
    key.assert_matches(config, context, &fetched.raw);
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        fetched.header("etag")
    );

    // Parity with `GetPublicKeyConfig`: both reads answer from the ONE record, so the
    // configuration and the version token they report are the same.
    let config_read = get_key_config(router, id).await;
    assert_eq!(
        config_read.status, 200,
        "case {case} ({context}): expected HTTP 200 from GetPublicKeyConfig, got {} with {}",
        config_read.status, config_read.raw
    );
    // `PublicKeyConfig` is that output's `@httpPayload`, so the configuration IS the
    // document's root element.
    key.assert_matches(config_read.root(), context, &config_read.raw);
    assert_eq!(
        config_read.header("etag"),
        etag,
        "case {case} ({context}): expected one version token across both reads, got {:?} and \
         {etag}",
        config_read.header("etag")
    );
}

/// The keys every rule below is asserted over: the encoded key's form and the comment's
/// form varied across the rows, so no rule rests on one shape of either.
fn key_table() -> Vec<Key> {
    let comments: [Option<String>; 5] = [
        None,
        Some(String::new()),
        Some("the key a signed-URL workflow rotates".to_string()),
        Some("tags <key> & \"quotes\" and 'apostrophes'".to_string()),
        // "The comment cannot be longer than 128 characters": the bound itself is
        // admissible, so one row sits exactly on it.
        Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
    ];
    comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| Key {
            case: format!("row {index}"),
            caller_reference: format!("public-key-update-{index}"),
            name: format!("public-key-update-{index}"),
            encoded_key: pem(index % 3 + 1, char::from(b'A' + u8::try_from(index).unwrap_or(0))),
            comment,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A comment-only update lands: the stored record keeps its id, its created time and every
/// immutable member, reports the submitted comment, and carries the fresh version token the
/// update handed back — which is the only one a later `If-Match` satisfies.
///
/// Each row is updated to every comment form in turn, so the rule is asserted over a comment
/// that is added, replaced, emptied, dropped and taken to the published bound.
#[tokio::test]
async fn a_comment_only_update_lands_and_the_read_reports_it() {
    let router = emulator();
    let mut etags = BTreeSet::new();

    for key in key_table() {
        let (id, create_etag, created_time) = seed(&router, &key).await;
        assert!(
            etags.insert(create_etag.clone()),
            "case {}: the version token {create_etag} repeated",
            key.case
        );
        let mut current = key.clone();
        let mut etag = create_etag.clone();

        for comment in [
            Some("rotated once".to_string()),
            Some(String::new()),
            None,
            Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
            Some("プロダクション — a & b < c".to_string()),
        ] {
            let target = Key {
                case: format!("{} -> comment {comment:?}", key.case),
                comment,
                ..current.clone()
            };
            let answer = update_key(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 200,
                "case {}: expected HTTP 200 from the comment-only update, got {} with {}",
                target.case, answer.status, answer.raw
            );
            let config = answer.root().at("PublicKeyConfig").unwrap_or_else(|| {
                panic!(
                    "case {}: the update carries no PublicKeyConfig: {}",
                    target.case, answer.raw
                )
            });
            target.assert_matches(config, "the update", &answer.raw);
            assert_eq!(
                answer.root().text_at("CreatedTime"),
                Some(created_time.as_str()),
                "case {}: expected the create's instant to be kept, got {}",
                target.case,
                answer.raw
            );

            let fresh = answer.header("etag").to_string();
            assert!(
                is_version_token(&fresh),
                "case {}: expected a version token in the ETag header, got {fresh:?}",
                target.case
            );
            assert_ne!(
                fresh, etag,
                "case {}: expected the update to mint a fresh version token, got {fresh} again",
                target.case
            );
            assert!(
                etags.insert(fresh.clone()),
                "case {}: the version token {fresh} repeated",
                target.case
            );

            assert_stored(&router, &id, &target, &fresh, &created_time, "after the update").await;

            // The token the update matched on is superseded.
            let stale = update_key(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                stale.status, 412,
                "case {}: expected HTTP 412 for the superseded token {etag}, got {} with {}",
                target.case, stale.status, stale.raw
            );
            assert_eq!(
                stale.error_code(),
                "PreconditionFailed",
                "case {}: expected PreconditionFailed, got {}",
                target.case,
                stale.raw
            );

            current = target;
            etag = fresh;
        }
    }
}

/// Every member but the comment is fixed for the key's life: an update that moves one is
/// refused with `CannotChangeImmutablePublicKeyFields` (HTTP 400) and the sentence that
/// condition carries, and the stored record — configuration, created time and version token
/// — is exactly what it was. The comment-only update on the same key then lands, so the
/// refusal is keyed on the member that moved rather than on the request's shape.
#[tokio::test]
async fn an_update_that_moves_an_immutable_member_is_refused() {
    let router = emulator();

    for key in key_table() {
        let (id, etag, created_time) = seed(&router, &key).await;

        // One row per immutable member and per form the change takes: a different value, an
        // empty one, a longer one, and a value differing only in case.
        let changes: Vec<(String, Key)> = vec![
            (
                "a different CallerReference".to_string(),
                Key {
                    caller_reference: format!("{}-moved", key.caller_reference),
                    ..key.clone()
                },
            ),
            (
                "an empty CallerReference".to_string(),
                Key {
                    caller_reference: String::new(),
                    ..key.clone()
                },
            ),
            (
                "a different Name".to_string(),
                Key {
                    name: format!("{}-renamed", key.name),
                    ..key.clone()
                },
            ),
            (
                "the Name differing only in case".to_string(),
                Key {
                    name: key.name.to_uppercase(),
                    ..key.clone()
                },
            ),
            (
                "a different EncodedKey".to_string(),
                Key {
                    encoded_key: pem(4, 'Z'),
                    ..key.clone()
                },
            ),
            (
                "the EncodedKey with one line dropped".to_string(),
                Key {
                    encoded_key: pem(1, 'Q'),
                    ..key.clone()
                },
            ),
            (
                "an empty EncodedKey".to_string(),
                Key {
                    encoded_key: String::new(),
                    ..key.clone()
                },
            ),
            (
                "an immutable member moved beside a new comment".to_string(),
                Key {
                    encoded_key: pem(2, 'Y'),
                    comment: Some("a new comment that does not license the key change".to_string()),
                    ..key.clone()
                },
            ),
        ];

        for (description, moved) in &changes {
            let case = format!("{} / {description}", key.case);
            let answer = update_key(&router, &id, &moved.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "CannotChangeImmutablePublicKeyFields",
                "case {case}: expected CannotChangeImmutablePublicKeyFields, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                IMMUTABLE_FIELDS_MESSAGE,
                "case {case}: expected the immutability sentence {IMMUTABLE_FIELDS_MESSAGE:?}, \
                 got {}",
                answer.raw
            );
            assert_stored(&router, &id, &key, &etag, &created_time, &case).await;
        }

        // The control: the same request with every immutable member left as stored lands.
        let legal = Key {
            case: format!("{} / comment only", key.case),
            comment: Some("the legal update after the refusals".to_string()),
            ..key.clone()
        };
        let accepted = update_key(&router, &id, &legal.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {}: expected the comment-only update to land, got {} with {}",
            legal.case, accepted.status, accepted.raw
        );
        let fresh = accepted.header("etag").to_string();
        assert_stored(
            &router,
            &id,
            &legal,
            &fresh,
            &created_time,
            "after the legal update",
        )
        .await;
    }
}

/// An `If-Match` the key's version cannot satisfy is refused and writes nothing: an absent
/// header or one carrying no version token at all is `InvalidIfMatchVersion` (HTTP 400) —
/// answered for an id no key holds too, since the header is the request's — and a
/// well-formed token that is not this key's is `PreconditionFailed` (HTTP 412). The current
/// token is accepted bare, quoted and weak.
#[tokio::test]
async fn an_if_match_the_key_cannot_satisfy_is_refused() {
    let router = emulator();
    let subject = Key {
        case: "the key every version refusal is asserted on".to_string(),
        caller_reference: "public-key-version-gate".to_string(),
        name: "public-key-version-gate".to_string(),
        encoded_key: pem(2, 'K'),
        comment: Some("gated on its own ETag".to_string()),
    };
    let other = Key {
        case: "another key, whose token is not the subject's".to_string(),
        caller_reference: "public-key-version-other".to_string(),
        name: "public-key-version-other".to_string(),
        encoded_key: pem(1, 'L'),
        comment: None,
    };
    let (_, other_etag, _) = seed(&router, &other).await;
    let (id, etag, created_time) = seed(&router, &subject).await;
    let body = Key {
        comment: Some("the comment the refused updates submit".to_string()),
        ..subject.clone()
    }
    .xml();

    let rows: [(&str, Option<String>, u16, &str); 6] = [
        ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            Some("not-a-version".to_string()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty If-Match",
            Some(String::new()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match carrying a lowercase token",
            Some(etag.to_lowercase()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a version token that is another key's",
            Some(other_etag.clone()),
            412,
            "PreconditionFailed",
        ),
        (
            "a well-formed token this service never minted",
            Some("E1Z2Y3X4W5V6U7".to_string()),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, code) in &rows {
        let answer = update_key(&router, &id, &body, if_match.as_deref()).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &subject, &etag, &created_time, case).await;

        // The same header against an id no key holds: a form no version could satisfy is
        // answered on the header, a well-formed one on the id.
        let unknown = update_key(&router, "K2NEVERMINTED1", &body, if_match.as_deref()).await;
        let (expected_status, expected_code) = if *status == 400 {
            (400, "InvalidIfMatchVersion")
        } else {
            (404, "NoSuchPublicKey")
        };
        assert_eq!(
            unknown.status, expected_status,
            "case {case} on an unknown id: expected HTTP {expected_status}, got {} with {}",
            unknown.status, unknown.raw
        );
        assert_eq!(
            unknown.error_code(),
            expected_code,
            "case {case} on an unknown id: expected {expected_code}, got {}",
            unknown.raw
        );
    }

    // The current token in each form an entity tag takes, on a fresh key per form.
    for (index, form) in ["{token}", "\"{token}\"", "W/\"{token}\""]
        .into_iter()
        .enumerate()
    {
        let fresh_key = Key {
            case: format!("the accepted token form {index}"),
            caller_reference: format!("public-key-version-accepted-{index}"),
            name: format!("public-key-version-accepted-{index}"),
            encoded_key: pem(1, 'M'),
            comment: None,
        };
        let (fresh_id, fresh_etag, fresh_created) = seed(&router, &fresh_key).await;
        let target = Key {
            comment: Some(format!("accepted with form {index}")),
            ..fresh_key.clone()
        };
        let header = form.replace("{token}", &fresh_etag);
        let accepted = update_key(&router, &fresh_id, &target.xml(), Some(&header)).await;
        assert_eq!(
            accepted.status, 200,
            "case {}: expected the current token as {header:?} to be accepted, got {} with {}",
            target.case, accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &fresh_id,
            &target,
            accepted.header("etag"),
            &fresh_created,
            "after the accepted token form",
        )
        .await;
    }
}

/// The documented comment bound holds on an update as it does on the create: a comment
/// longer than 128 characters is `InvalidArgument` (HTTP 400) and stores nothing, while the
/// bound itself is admissible.
#[tokio::test]
async fn a_comment_past_the_documented_bound_is_refused() {
    let router = emulator();
    let key = Key {
        case: "the key the comment bound is asserted on".to_string(),
        caller_reference: "public-key-comment-bound".to_string(),
        name: "public-key-comment-bound".to_string(),
        encoded_key: pem(1, 'C'),
        comment: None,
    };
    let (id, etag, created_time) = seed(&router, &key).await;

    // One character past the bound, and far past it.
    for (case, length) in [
        ("one character past the bound", MAX_PUBLIC_KEY_COMMENT + 1),
        ("far past the bound", MAX_PUBLIC_KEY_COMMENT * 4),
    ] {
        let target = Key {
            comment: Some("c".repeat(length)),
            ..key.clone()
        };
        let answer = update_key(&router, &id, &target.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a comment of {length}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &key, &etag, &created_time, case).await;
    }

    // The bound itself lands.
    let at_bound = Key {
        comment: Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
        ..key.clone()
    };
    let accepted = update_key(&router, &id, &at_bound.xml(), Some(&etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case the bound itself: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert_stored(
        &router,
        &id,
        &at_bound,
        accepted.header("etag"),
        &created_time,
        "after the comment at the bound",
    )
    .await;
}

/// An id no stored key holds is `NoSuchPublicKey` (HTTP 404) — under the absent-key sentence
/// for an id of a public key's own shape and the unreadable-identifier sentence for one
/// outside it — and the refused update creates nothing under it.
#[tokio::test]
async fn an_id_no_key_holds_is_refused() {
    let router = emulator();
    let present = Key {
        case: "the one key that exists".to_string(),
        caller_reference: "public-key-unknown-present".to_string(),
        name: "public-key-unknown-present".to_string(),
        encoded_key: pem(1, 'P'),
        comment: None,
    };
    let (id, etag, created_time) = seed(&router, &present).await;
    let body = Key {
        comment: Some("never stored".to_string()),
        ..present.clone()
    }
    .xml();

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let rows: [(&str, String, &str); 4] = [
        (
            "an id never minted",
            "K2NEVERMINTED1".to_string(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "the existing id in lower case",
            id.to_lowercase(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "a token of no public key id's shape",
            "not-a-key".to_string(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
    ];

    for (case, candidate, sentence) in &rows {
        let answer = update_key(&router, candidate, &body, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchPublicKey",
            "case {case}: expected NoSuchPublicKey for {candidate:?}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            *sentence,
            "case {case}: expected {sentence:?}, got {}",
            answer.raw
        );
        assert_eq!(
            get_key(&router, candidate).await.status,
            404,
            "case {case}: expected the refused update to create nothing under {candidate:?}"
        );
        assert_eq!(
            get_key_config(&router, candidate).await.status,
            404,
            "case {case}: expected the config read of {candidate:?} to report NoSuchPublicKey"
        );
    }

    // The key that does exist is untouched by every refusal above.
    assert_stored(
        &router,
        &id,
        &present,
        &etag,
        &created_time,
        "after the refusals",
    )
    .await;
}
