// SPDX-License-Identifier: CC0-1.0

//! `DeletePublicKey`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states ("Remove a
//! public key you previously added to CloudFront"):
//!
//! * the delete REMOVES the ONE stored record every read answered from, so afterwards the id
//!   names no public key through either reader (`NoSuchPublicKey`, HTTP 404), the listing
//!   stops reporting it, and the caller reference it held is free for a later create to claim
//!   under a new id;
//! * the delete is gated on the key's own version token: an absent `If-Match` and one that is
//!   no version token at all are `InvalidIfMatchVersion` (HTTP 400) — a property of the
//!   REQUEST, so it is answered for an id no key holds too — a well-formed token that is not
//!   this key's current one is `PreconditionFailed` (HTTP 412), and in either case the key
//!   stays exactly where it was;
//! * a public key another stored record still references cannot be removed: a key group lists
//!   "the identifiers of the public keys in the key group", and a field-level encryption
//!   profile's encryption entity names "the public key associated with a set of field-level
//!   encryption patterns" — either reference is `PublicKeyInUse` (HTTP 409) and leaves the key
//!   readable, and once the referencing record has let go the same delete lands.
//!
//! The rows span what those rules generalise over: the form of the encoded key (single-line
//! and multi-line PEM, long and short bodies) and of the comment (absent, empty, ASCII, XML
//! metacharacters, and exactly the published 128-character bound); every form an `If-Match`
//! takes (absent, empty, bare, quoted, weak, lower case, another key's token, a token this
//! service never minted, a value that is no token at all); which kind of record holds the
//! reference and at which position of its list; and ids of several shapes the store holds no
//! key for.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the public key writer and reader
//! are [`crate::public_key_lifecycle`]'s, and the two records that reference a public key are
//! built through the key group and field-level encryption profile suites' own helpers —
//! reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_PUBLIC_KEY_COMMENT;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::field_level_encryption_profile_lifecycle::{create_profile, get_profile, Entity, Profile};
use crate::key_group_lifecycle::{create_group, get_group, Group};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentences CloudFront sends for these refusals, spelled out here rather than taken from
/// the crate, so a reworded refusal fails this suite.
const NO_SUCH_PUBLIC_KEY_MESSAGE: &str = "The specified CloudFront public key does not exist.";
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";
const PUBLIC_KEY_IN_USE_MESSAGE: &str =
    "The Cloudfront public key is currently associated with either Key Group or a field level \
     encryption profile. Please disassociate the key before deleting.";
const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

// ---------------------------------------------------------------------------
// Calling the surfaces this suite drives
// ---------------------------------------------------------------------------

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's own
/// `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

/// `DELETE /2020-05-31/public-key/{Id}` under the version token `if_match` — the route and
/// the header the model binds this operation to (`IfMatch` travels in `If-Match`).
///
/// `pub(crate)` so a sibling suite that has to release a public key — the key group delete
/// suite, whose rule is that a group's removal frees the keys it listed — goes over the same
/// wire path rather than growing a copy of this.
pub(crate) async fn delete_key(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/public-key/{id}"),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/public-key/{Id}/config` — the second reader of the one record.
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

/// `GET /2020-05-31/public-key` — the listing, read here only to check what it reports.
async fn list_keys(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/key-group/{Id}` with the `KeyGroupConfig` document `body` under the
/// version token `if_match` — how a key group lets go of a public key it lists.
async fn update_group(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-group/{id}"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// `PUT /2020-05-31/field-level-encryption-profile/{Id}/config` with the
/// `FieldLevelEncryptionProfileConfig` document `body` under the version token `if_match` —
/// how a profile lets go of a public key one of its entities names.
async fn update_profile(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
        Some(body),
        Some(if_match),
    )
    .await
}

// ---------------------------------------------------------------------------
// The keys under test
// ---------------------------------------------------------------------------

/// One public key configuration, varying the members every rule here generalises over.
fn key_row(index: usize, tag: &str) -> Key {
    // "The comment cannot be longer than 128 characters": the bound itself is admissible, so
    // one form sits exactly on it.
    let comments: [Option<String>; 5] = [
        None,
        Some(String::new()),
        Some("the key a signed-URL workflow retires".to_string()),
        Some("tags <key> & \"quotes\" and 'apostrophes'".to_string()),
        Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
    ];
    Key {
        case: format!("{tag} row {index}"),
        caller_reference: format!("{tag}-{index}"),
        name: format!("{tag}-{index}"),
        encoded_key: pem(index % 4 + 1, char::from(b'A' + u8::try_from(index).unwrap_or(0))),
        comment: comments[index % comments.len()].clone(),
    }
}

/// The configurations every case below is run over: the encoded key's form and the comment's
/// form varied across the rows, so no rule here rests on one shape of either.
fn table(tag: &str) -> Vec<Key> {
    (0..5).map(|index| key_row(index, tag)).collect()
}

/// Upload `key` and report the id and the version token the create minted.
async fn seed(router: &MultiServiceRouter, key: &Key) -> (String, String) {
    let created = create_key(router, &key.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreatePublicKey, got {} with {}",
        key.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token, got {etag:?}",
        key.case
    );
    (id, etag)
}

/// Assert that `id` names no public key any more: both readers answer `NoSuchPublicKey` with
/// the service's sentence, and the listing has stopped reporting it.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    for (reader, answer) in [
        ("GetPublicKey", get_key(router, id).await),
        ("GetPublicKeyConfig", get_key_config(router, id).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from {reader} after the delete, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchPublicKey",
            "case {case:?}: expected NoSuchPublicKey from {reader} after the delete, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_PUBLIC_KEY_MESSAGE,
            "case {case:?}: expected the service's missing-key sentence from {reader}, got \
             {message:?}"
        );
    }

    let listed = list_keys(router).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListPublicKeys, got {} with {}",
        listed.status, listed.raw
    );
    assert!(
        !listed.raw.contains(id),
        "case {case:?}: expected the listing to stop reporting the deleted key {id:?}, got {}",
        listed.raw
    );
}

/// Assert that the key `id` names is still readable at the version token `etag`, which is what
/// a refused delete leaves behind.
async fn assert_still_stored(router: &MultiServiceRouter, id: &str, etag: &str, case: &str) {
    let fetched = get_key(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case:?}: expected the refused delete to leave the key readable, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case:?}: expected the refused delete to leave the version token {etag} alone, got \
         {:?}",
        fetched.header("etag")
    );

    let listed = list_keys(router).await;
    assert!(
        listed.raw.contains(id),
        "case {case:?}: expected the listing to still report the key {id:?}, got {}",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Every key the create accepts is removed by its delete under its version token: the delete
/// answers HTTP 204 with no body, both readers then report the id as naming nothing, the
/// listing stops reporting it, deleting it again is `NoSuchPublicKey`, and the caller reference
/// it held is free for a NEW key under a new id.
#[tokio::test]
async fn every_stored_key_is_removed_by_its_delete() {
    let router = emulator();

    for row in table("public-key-delete") {
        let (id, etag) = seed(&router, &row).await;

        // Round trip: the key is readable before the delete, so the 404 after it is the
        // delete's doing and not a key that was never stored.
        let fetched = get_key(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetPublicKey before the delete, got {} with {}",
            row.case, fetched.status, fetched.raw
        );
        let config = fetched.root().at("PublicKeyConfig").unwrap_or_else(|| {
            panic!(
                "case {}: the read carries no PublicKeyConfig: {}",
                row.case, fetched.raw
            )
        });
        row.assert_matches(config, "before the delete", &fetched.raw);

        let deleted = delete_key(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {}: expected HTTP 204 from DeletePublicKey, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {}: expected no body from a 204, got {}",
            row.case,
            deleted.raw
        );

        assert_gone(&router, &id, &row.case).await;

        let again = delete_key(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {}: expected HTTP 404 from a second delete, got {} with {}",
            row.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchPublicKey",
            "case {}: expected NoSuchPublicKey from a second delete, got {}",
            row.case,
            again.raw
        );

        // The caller reference the deleted key held identifies nothing any more, so a create
        // under it mints a SECOND key rather than reporting the one that is gone.
        let (reused_id, reused_etag) = seed(&router, &row).await;
        assert_ne!(
            reused_id, id,
            "case {}: expected a freshly minted id for the key recreated under the freed caller \
             reference, got the deleted one {id:?}",
            row.case
        );

        // Leave the store as this row found it, so the next row's reference is the only one in
        // play.
        let cleanup = delete_key(&router, &reused_id, Some(&reused_etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {}: expected HTTP 204 deleting the recreated key, got {} with {}",
            row.case, cleanup.status, cleanup.raw
        );
    }
}

/// The delete is gated on the key's OWN version token, and a header no version could satisfy is
/// answered on the header rather than on the id.
///
/// The rows span every form an `If-Match` takes: absent, empty, a value that is no version
/// token, a token spelled in lower case, a well-formed token belonging to another key, one this
/// service never minted, and the current token bare, quoted and weak — the three ways HTTP and
/// CloudFront's own `ETag` header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();

    let other = key_row(0, "public-key-delete-gate-other");
    let (_, other_etag) = seed(&router, &other).await;

    let refusals: [(&str, Option<String>, u16, &str, &str); 6] = [
        (
            "no If-Match at all",
            None,
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an empty If-Match",
            Some(String::new()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match that is no version token",
            Some("not-a-version".to_string()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match carrying a lowercase token",
            Some(other_etag.to_lowercase()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "a version token that is another key's",
            Some(other_etag.clone()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
        (
            "a well-formed token this service never minted",
            Some("E1Z2Y3X4W5V6U7".to_string()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
    ];

    for (index, (case, if_match, status, code, message)) in refusals.iter().enumerate() {
        let row = key_row(index, "public-key-delete-gate");
        let (id, etag) = seed(&router, &row).await;

        let answer = delete_key(&router, &id, if_match.as_deref()).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            *message,
            "case {case:?}: expected the service's sentence for {code}, got {}",
            answer.raw
        );
        assert_still_stored(&router, &id, &etag, case).await;

        // The same header against an id no key holds: a form no version could satisfy is
        // answered on the header, a well-formed one on the id.
        let unknown = delete_key(&router, "K2NEVERMINTED1", if_match.as_deref()).await;
        let (expected_status, expected_code) = if *status == 400 {
            (400, "InvalidIfMatchVersion")
        } else {
            (404, "NoSuchPublicKey")
        };
        assert_eq!(
            unknown.status, expected_status,
            "case {case:?} on an unknown id: expected HTTP {expected_status}, got {} with {}",
            unknown.status, unknown.raw
        );
        assert_eq!(
            unknown.error_code(),
            expected_code,
            "case {case:?} on an unknown id: expected {expected_code}, got {}",
            unknown.raw
        );

        // Clear the way for the next row, which creates a key under its own reference.
        let cleanup = delete_key(&router, &id, Some(&etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: expected the current token to delete the key, got {} with {}",
            cleanup.status, cleanup.raw
        );
        assert_gone(&router, &id, case).await;
    }

    // The current token, spelled each of the three admissible ways, deletes the key.
    let spellings: [(&str, Spelling); 3] = [
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (index, (case, spell)) in spellings.iter().enumerate() {
        let row = key_row(index, "public-key-delete-spelling");
        let (id, etag) = seed(&router, &row).await;
        let answer = delete_key(&router, &id, Some(&spell(&etag))).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// How one row below builds the record that references the subject key, from the subject's id:
/// the referencing record is created and then moved OFF the subject key, and the two closures
/// are what each kind of record needs for that.
struct Reference {
    /// What this row is a case of, named in every assertion it drives.
    case: &'static str,
    /// Which position of the referencing record's list names the subject key, so the gate is
    /// shown not to rest on the subject being the only or the first entry.
    kind: ReferenceKind,
}

/// The kinds of stored record that reach a public key, and how many entries their list carries
/// around the subject.
enum ReferenceKind {
    /// A key group listing `before` other key ids, then the subject, then `after` others —
    /// `KeyGroupConfig$Items`, "a list of the identifiers of the public keys in the key group".
    KeyGroup { before: usize, after: usize },
    /// A field-level encryption profile whose entity list carries `before` entities naming other
    /// keys, then one naming the subject — `EncryptionEntity$PublicKeyId`.
    Profile { before: usize },
}

/// A public key another stored record references cannot be deleted, and once that record has let
/// go the same delete lands.
///
/// The rows cover both kinds of record that reach a public key and the position of the reference
/// in each one's list. A spare key nothing references is deleted in every row, so the gate is
/// shown to select on the REFERENCE rather than on the store merely holding such a record.
#[tokio::test]
async fn a_key_a_stored_record_references_cannot_be_deleted() {
    let rows = [
        Reference {
            case: "a key group whose only entry is the key",
            kind: ReferenceKind::KeyGroup {
                before: 0,
                after: 0,
            },
        },
        Reference {
            case: "a key group that lists the key last",
            kind: ReferenceKind::KeyGroup {
                before: 2,
                after: 0,
            },
        },
        Reference {
            case: "a key group that lists the key in the middle",
            kind: ReferenceKind::KeyGroup {
                before: 1,
                after: 1,
            },
        },
        Reference {
            case: "a profile whose only encryption entity names the key",
            kind: ReferenceKind::Profile { before: 0 },
        },
        Reference {
            case: "a profile whose second encryption entity names the key",
            kind: ReferenceKind::Profile { before: 1 },
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();

        // The key under the gate, a key the referencing record is moved onto when it lets go,
        // some filler keys for the other entries of its list, and a spare key nothing ever
        // references.
        let subject_row = key_row(1, "public-key-in-use-subject");
        let (subject, subject_etag) = seed(&router, &subject_row).await;
        let replacement_row = key_row(2, "public-key-in-use-replacement");
        let (replacement, _) = seed(&router, &replacement_row).await;
        let spare_row = key_row(3, "public-key-in-use-spare");
        let (spare, spare_etag) = seed(&router, &spare_row).await;
        let mut filler: Vec<String> = Vec::new();
        for index in 0..4 {
            let (id, _) = seed(&router, &key_row(index, "public-key-in-use-filler")).await;
            filler.push(id);
        }

        // The referencing record, and the configuration it is given when it lets go of the
        // subject key.
        let (record_id, record_etag, released) = match row.kind {
            ReferenceKind::KeyGroup { before, after } => {
                let mut items: Vec<String> = filler[..before].to_vec();
                items.push(subject.clone());
                items.extend(filler[before..before + after].iter().cloned());
                let group = Group {
                    case: case.to_string(),
                    name: "public-key-in-use-group".to_string(),
                    items,
                    comment: Some("the group that holds the reference".to_string()),
                };
                let created = create_group(&router, &group.xml()).await;
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
                    created.status, created.raw
                );
                let id = created.root().text_at("Id").unwrap_or_default().to_string();
                let etag = created.header("etag").to_string();
                let released = Group {
                    items: vec![replacement.clone()],
                    ..group
                }
                .xml();
                (id, etag, released)
            }
            ReferenceKind::Profile { before } => {
                let mut entities: Vec<Entity> = filler[..before]
                    .iter()
                    .map(|key| Entity::new(key, "provider-filler", &["Filler"]))
                    .collect();
                entities.push(Entity::new(&subject, "provider-subject", &["CreditCard"]));
                let profile = Profile::new(
                    case,
                    "public-key-in-use-profile",
                    Some("the profile that holds the reference"),
                    entities,
                );
                let created = create_profile(&router, &profile.xml()).await;
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: expected HTTP 201 from CreateFieldLevelEncryptionProfile, got \
                     {} with {}",
                    created.status, created.raw
                );
                let id = created.root().text_at("Id").unwrap_or_default().to_string();
                let etag = created.header("etag").to_string();
                let released = Profile::new(
                    case,
                    "public-key-in-use-profile",
                    Some("the profile that holds the reference"),
                    vec![Entity::new(&replacement, "provider-subject", &["CreditCard"])],
                )
                .xml();
                (id, etag, released)
            }
        };

        // While the record names the key, the delete is refused and the key stays readable.
        let refused = delete_key(&router, &subject, Some(&subject_etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 while a record references the key, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "PublicKeyInUse",
            "case {case:?}: expected PublicKeyInUse while the key is referenced, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            PUBLIC_KEY_IN_USE_MESSAGE,
            "case {case:?}: expected the service's in-use sentence, got {}",
            refused.raw
        );
        assert_still_stored(&router, &subject, &subject_etag, case).await;

        // A version that is not the key's is refused on its own terms, before anything about the
        // reference.
        for (description, if_match, status, code) in [
            ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
            (
                "a version token that is the referencing record's",
                Some(record_etag.as_str()),
                412,
                "PreconditionFailed",
            ),
        ] {
            let answer = delete_key(&router, &subject, if_match).await;
            assert_eq!(
                answer.status, status,
                "case {case:?} with {description}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?} with {description}: expected {code}, got {}",
                answer.raw
            );
        }

        // The spare key no record names is deletable throughout, so the refusal above is the
        // reference's doing rather than the store holding a referencing record at all.
        let spare_deleted = delete_key(&router, &spare, Some(&spare_etag)).await;
        assert_eq!(
            spare_deleted.status, 204,
            "case {case:?}: expected HTTP 204 deleting the key no record names, got {} with {}",
            spare_deleted.status, spare_deleted.raw
        );
        assert_gone(&router, &spare, "a key no record names").await;

        // Move the referencing record off the subject key, and the same delete lands.
        let released_answer = match row.kind {
            ReferenceKind::KeyGroup { .. } => {
                update_group(&router, &record_id, &released, &record_etag).await
            }
            ReferenceKind::Profile { .. } => {
                update_profile(&router, &record_id, &released, &record_etag).await
            }
        };
        assert_eq!(
            released_answer.status, 200,
            "case {case:?}: expected HTTP 200 moving the record off the key, got {} with {}",
            released_answer.status, released_answer.raw
        );

        let deleted = delete_key(&router, &subject, Some(&subject_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no record references the key, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &subject, case).await;

        // The record that let go is still there, and the key it was moved onto is now the one
        // that cannot be deleted.
        let reader = match row.kind {
            ReferenceKind::KeyGroup { .. } => get_group(&router, &record_id).await,
            ReferenceKind::Profile { .. } => get_profile(&router, &record_id).await,
        };
        assert_eq!(
            reader.status, 200,
            "case {case:?}: expected the referencing record to survive the key's delete, got {} \
             with {}",
            reader.status, reader.raw
        );
        let moved = delete_key(&router, &replacement, Some("E1Z2Y3X4W5V6U7")).await;
        assert_eq!(
            moved.error_code(),
            "PreconditionFailed",
            "case {case:?}: expected the key the record was moved onto to be gated on its own \
             version first, got {}",
            moved.raw
        );
    }
}

/// A well-formed public key id the store holds no key for is `NoSuchPublicKey` (HTTP 404)
/// with the absent-key sentence, and an `Id` outside the shape a public key id takes is the
/// same code with the unreadable-identifier sentence — never a 204, which would claim
/// something was removed.
#[tokio::test]
async fn an_id_no_key_holds_is_refused() {
    let router = emulator();
    let present = key_row(2, "public-key-delete-unknown");
    let (known, etag) = seed(&router, &present).await;

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    // Each row: the case, the `Id` submitted, and the sentence it is refused with — the
    // unreadable-identifier sentence for a well-formed identifier of another resource kind
    // this service mints, the absent-key sentence for every other value.
    let absent: [(&str, String, &str); 7] = [
        (
            "a public key id never minted",
            "K2NEVERMINTED1".to_string(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "the known id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "the known id in lower case",
            known.to_lowercase(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "a token that is no public key id",
            "not-a-public-key-id".to_string(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "the caller reference rather than the id",
            present.caller_reference.clone(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
        ),
        (
            "a well-formed distribution id",
            "EDFDVBD6EXAMPL".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a well-formed key group id",
            "4fbc5e5c-1ee4-4f3b-9e9f-4a2a3b3b7c11".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
    ];

    for (case, candidate, sentence) in absent {
        let answer = delete_key(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchPublicKey",
            "case {case:?}: expected NoSuchPublicKey for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, sentence,
            "case {case:?}: expected {sentence:?} for {candidate:?}, got {message:?}"
        );
    }

    // None of those refusals touched the key the account really holds.
    assert_still_stored(&router, &known, &etag, "the only key the account holds").await;
}
