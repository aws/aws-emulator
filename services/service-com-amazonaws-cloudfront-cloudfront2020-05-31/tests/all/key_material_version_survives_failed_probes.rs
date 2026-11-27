// SPDX-License-Identifier: CC0-1.0

//! That a record of CloudFront's KEY-MATERIAL surface keeps the version token it last
//! published, so the token a caller was handed still deletes it later, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The surface is the key material a field-level encryption configuration
//! reaches: the signed-URL public keys, and the field-level encryption profiles that encrypt
//! with them. The rules under test are the ones the modelled `ETag` ("the current version of
//! the …") and `PreconditionFailed` ("the precondition in one or more of the request fields
//! evaluated to false") state together:
//!
//! * a record's version token is published by the WRITE that stored it and by nothing else, so
//!   no READ of that record (either of its two readers, and its listing) and no REFUSED call —
//!   a probe for an id the store holds nothing under, a rewrite whose `If-Match` carries no
//!   version token, a rewrite or delete whose `If-Match` carries one that is not the record's,
//!   a delete refused because another record still references it — moves the record on: every
//!   read after them reports the same token, and the conditional DELETE presenting that token
//!   is served HTTP 204 with an empty body, however many such calls stand between the read and
//!   the delete;
//! * the version token is what the delete is gated on, so one that is NOT the record's current
//!   version — the token it published before a rewrite, a token this account holds for another
//!   record, or one this service never issued — is `PreconditionFailed` (HTTP 412) and removes
//!   nothing, while the token the last write published is served;
//! * so a whole field-level encryption chain stood up by one caller — a public key, the profile
//!   that encrypts with it and the configuration that maps to that profile — is torn down again
//!   under the three tokens its three creates handed back, even when the calls between them are
//!   deliberately-failing probes: each delete answers 204 with an empty body.
//!
//! The rows span what the rules generalise over: both record kinds of the surface, each kind of
//! call that must NOT move a record on (two readers, the listing, a not-found probe, a rewrite
//! refused on the header's form, a rewrite refused on the precondition, a delete refused on the
//! precondition, a delete refused because the record is referenced), the three ways a token is
//! spelled in the header (bare, as CloudFront's own `ETag` reports it, and quoted or weak, as
//! HTTP spells an entity tag), and — for the refusals — the provenance of the token presented
//! (the record's own replaced version, another record's current version, one never issued).
//!
//! What each route reads of the header's FORM is tabled in
//! [`crate::key_material_if_match_form`], and the retry that follows a refused precondition in
//! [`crate::key_material_superseded_version_delete`]; this suite is about the token a record
//! keeps while everything else happens to it.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, and the creates, readers and
//! deletes are the public key, profile and configuration suites' own helpers — reused rather
//! than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::field_level_encryption_config_delete::delete_config;
use crate::field_level_encryption_config_lifecycle::{create_config, get_config, simple};
use crate::field_level_encryption_profile_delete::delete_profile;
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, Entity, Profile,
};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentences these refusals carry, spelled out here rather than taken from the crate, so a
/// reworded refusal fails this suite.
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";
const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";

/// A version token written the way this service writes one — `E` followed by 13 characters of
/// the identifier alphabet — that a fresh emulator never issued to anything.
const UNISSUED_TOKEN: &str = "EZZZZZZZZZZZZ9";

/// An `If-Match` value that is no version token at all, for the rewrite a row refuses on the
/// header's FORM rather than on the precondition.
const UNREADABLE_IF_MATCH: &str = "not-a-version";

/// Which record of the key-material surface a row addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    PublicKey,
    Profile,
}

/// Both kinds of the surface, so every row runs against each.
const KINDS: [Kind; 2] = [Kind::PublicKey, Kind::Profile];

/// How a row spells the token in the header.
type Spelling = fn(&str) -> String;

/// Each spelling, with the name every assertion of that row carries.
const SPELLINGS: &[(&str, Spelling)] = &[
    ("bare", |token| token.to_string()),
    ("quoted", |token| format!("\"{token}\"")),
    ("weak", |token| format!("W/\"{token}\"")),
];

/// An id of the shape this service mints for `kind` that a fresh emulator holds no record
/// under, for the probe a row makes before it deletes anything.
fn absent_id(kind: Kind) -> &'static str {
    match kind {
        Kind::PublicKey => "K9ZZZZZZZZZZZZ",
        Kind::Profile => "P9ZZZZZZZZZZZZ",
    }
}

/// `GET /2020-05-31/public-key/{Id}/config` — the public key's second reader, which this suite
/// drives to show that reading a record publishes nothing.
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

// ---------------------------------------------------------------------------
// Standing the surface up
// ---------------------------------------------------------------------------

/// The `PublicKeyConfig` a case submits: the ONE member a rewrite may move is the comment, so
/// every other member is resubmitted exactly as the create stated it.
fn key_config(reference: &str, comment: Option<&str>) -> Key {
    Key {
        case: format!("public key {reference}"),
        caller_reference: reference.to_string(),
        name: reference.to_string(),
        // The material is immaterial to these rules, and `EncodedKey` is fixed for a key's
        // life.
        encoded_key: pem(2, 'K'),
        comment: comment.map(str::to_string),
    }
}

/// The `FieldLevelEncryptionProfileConfig` a case submits, encrypting with `key`.
fn profile_config(reference: &str, key: &str) -> Profile {
    Profile::new(
        &format!("profile {reference}"),
        reference,
        None,
        vec![Entity::new(key, "provider-one", &["CreditCard"])],
    )
}

/// One record as a caller holds it: the id it is addressed by and the token the service last
/// published for it.
#[derive(Debug, Clone)]
struct Held {
    id: String,
    etag: String,
}

/// The chain one caller stands up: a public key, the profile that encrypts with it, and the
/// configuration that maps a query argument to that profile — each with the token its own
/// create handed back, which is the only token this suite ever presents.
#[derive(Debug)]
struct Chain {
    key: Held,
    profile: Held,
    config: Held,
}

/// Read the id and the `ETag` a create answered with, asserting the status and the token's
/// shape.
fn created(answer: &Answer, case: &str, status: u16) -> Held {
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status} from the create, got {} with {}",
        answer.status, answer.raw
    );
    let held = Held {
        id: answer.root().text_at("Id").unwrap_or_default().to_string(),
        etag: answer.header("etag").to_string(),
    };
    assert!(
        is_version_token(&held.etag),
        "case {case}: expected a minted version token from the create, got {:?}",
        held.etag
    );
    held
}

/// Stand up a public key, a profile over it, and a configuration over that profile, under the
/// caller reference prefix `reference`.
async fn chain(router: &MultiServiceRouter, reference: &str) -> Chain {
    let key_answer = create_key(router, &key_config(&format!("{reference}-key"), None).xml()).await;
    let key = created(&key_answer, &format!("{reference} public key"), 201);

    let profile_answer = create_profile(
        router,
        &profile_config(&format!("{reference}-profile"), &key.id).xml(),
    )
    .await;
    let profile = created(&profile_answer, &format!("{reference} profile"), 201);

    let config_answer = create_config(
        router,
        &simple(
            "the configuration of the chain",
            &format!("{reference}-config"),
            &profile.id,
        )
        .xml(),
    )
    .await;
    let config = created(&config_answer, &format!("{reference} configuration"), 201);

    Chain {
        key,
        profile,
        config,
    }
}

// ---------------------------------------------------------------------------
// Reading, rewriting and deleting, through each kind's own routes
// ---------------------------------------------------------------------------

/// The record of `kind` in `chain`.
fn addressed(chain: &Chain, kind: Kind) -> &Held {
    match kind {
        Kind::PublicKey => &chain.key,
        Kind::Profile => &chain.profile,
    }
}

/// The OTHER key-material record of `chain`, whose current token is a value that is not the
/// addressed record's version.
fn other(chain: &Chain, kind: Kind) -> &Held {
    match kind {
        Kind::PublicKey => &chain.profile,
        Kind::Profile => &chain.key,
    }
}

/// Read the record `id` names through the reader of its kind.
async fn read(router: &MultiServiceRouter, kind: Kind, id: &str) -> Answer {
    match kind {
        Kind::PublicKey => get_key(router, id).await,
        Kind::Profile => get_profile(router, id).await,
    }
}

/// The token the service reports for `id` right now, read through that kind's own reader.
async fn read_token(router: &MultiServiceRouter, kind: Kind, id: &str) -> String {
    let answer = read(router, kind, id).await;
    assert_eq!(
        answer.status, 200,
        "case reading {kind:?} {id}: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    answer.header("etag").to_string()
}

/// `PUT <the record's config route>` — the rewrite of the record of `kind`, resubmitting the
/// configuration its create stated so only the `If-Match` header decides the answer.
async fn rewrite(
    router: &MultiServiceRouter,
    kind: Kind,
    chain: &Chain,
    reference: &str,
    if_match: Option<&str>,
) -> Answer {
    let (path, body) = match kind {
        Kind::PublicKey => (
            format!("/{API_VERSION}/public-key/{}/config", chain.key.id),
            key_config(&format!("{reference}-key"), None).xml(),
        ),
        Kind::Profile => (
            format!(
                "/{API_VERSION}/field-level-encryption-profile/{}/config",
                chain.profile.id
            ),
            profile_config(&format!("{reference}-profile"), &chain.key.id).xml(),
        ),
    };
    call(router, "PUT", &path, Some(&body), if_match).await
}

/// `DELETE` the record `id` names, carrying the `If-Match` header `if_match`.
async fn delete(
    router: &MultiServiceRouter,
    kind: Kind,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    match kind {
        Kind::PublicKey => delete_key(router, id, if_match).await,
        Kind::Profile => delete_profile(router, id, if_match).await,
    }
}

/// `GET` the listing of `kind`, which reports the record without publishing a version for it.
async fn list(router: &MultiServiceRouter, kind: Kind) -> Answer {
    let path = match kind {
        Kind::PublicKey => format!("/{API_VERSION}/public-key"),
        Kind::Profile => format!("/{API_VERSION}/field-level-encryption-profile"),
    };
    call(router, "GET", &path, None, None).await
}

/// Assert `answer` is the refusal `status`/`code` carries, with `message` as its sentence.
fn assert_refusal(answer: &Answer, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status}, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case}: expected {code}, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case}: expected the sentence {message:?}, got {actual:?}"
    );
}

/// What a row puts between the token being published and the delete that presents it: a call
/// that must leave the record exactly where it is.
#[derive(Clone, Copy, Debug)]
enum Between {
    /// The record's own reader, which reports the version it is at.
    Read,
    /// The record's CONFIG reader, the second route that reports it.
    ReadConfig,
    /// The kind's listing, which reports the record without a version of its own.
    List,
    /// A read of an id the store holds no record of this kind under.
    AbsentProbe,
    /// A rewrite whose `If-Match` carries no version token at all (HTTP 400).
    RewriteRefusedOnTheHeader,
    /// A rewrite whose `If-Match` carries a version token that is not this record's (412).
    RewriteRefusedOnThePrecondition,
    /// A delete whose `If-Match` carries a version token that is not this record's (412).
    DeleteRefusedOnThePrecondition,
    /// A delete refused because the configuration of the chain still references the record
    /// (HTTP 409) — the profile through a mapping's `ProfileId`, the public key through the
    /// profile's encryption entity.
    DeleteRefusedOnAReference,
}

/// Every call that must not move a record on.
const BETWEEN: [Between; 8] = [
    Between::Read,
    Between::ReadConfig,
    Between::List,
    Between::AbsentProbe,
    Between::RewriteRefusedOnTheHeader,
    Between::RewriteRefusedOnThePrecondition,
    Between::DeleteRefusedOnThePrecondition,
    Between::DeleteRefusedOnAReference,
];

/// Make the call `between` names against the chain's record of `kind`, and assert it answered
/// the way its own route answers it — so a row that stops refusing, or stops serving, fails
/// here rather than silently leaving the record untouched.
async fn make(
    router: &MultiServiceRouter,
    kind: Kind,
    chain: &Chain,
    reference: &str,
    between: Between,
    case: &str,
) {
    match between {
        Between::Read => {
            let answer = read(router, kind, &addressed(chain, kind).id).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the reader to serve the record, got {} with {}",
                answer.status, answer.raw
            );
        }
        Between::ReadConfig => {
            let id = &addressed(chain, kind).id;
            let answer = match kind {
                Kind::PublicKey => get_key_config(router, id).await,
                Kind::Profile => get_profile_config(router, id).await,
            };
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the config reader to serve the record, got {} with {}",
                answer.status, answer.raw
            );
        }
        Between::List => {
            let answer = list(router, kind).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the listing to be served, got {} with {}",
                answer.status, answer.raw
            );
        }
        Between::AbsentProbe => {
            let answer = read(router, kind, absent_id(kind)).await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected HTTP 404 for an id the store holds nothing under, got {} \
                 with {}",
                answer.status, answer.raw
            );
        }
        Between::RewriteRefusedOnTheHeader => {
            let answer = rewrite(router, kind, chain, reference, Some(UNREADABLE_IF_MATCH)).await;
            assert_refusal(
                &answer,
                case,
                400,
                "InvalidIfMatchVersion",
                INVALID_IF_MATCH_VERSION_MESSAGE,
            );
        }
        Between::RewriteRefusedOnThePrecondition => {
            let answer = rewrite(router, kind, chain, reference, Some(UNISSUED_TOKEN)).await;
            assert_refusal(
                &answer,
                case,
                412,
                "PreconditionFailed",
                PRECONDITION_FAILED_MESSAGE,
            );
        }
        Between::DeleteRefusedOnThePrecondition => {
            let answer = delete(
                router,
                kind,
                &addressed(chain, kind).id,
                Some(&other(chain, kind).etag),
            )
            .await;
            assert_refusal(
                &answer,
                case,
                412,
                "PreconditionFailed",
                PRECONDITION_FAILED_MESSAGE,
            );
        }
        Between::DeleteRefusedOnAReference => {
            let answer = delete(
                router,
                kind,
                &addressed(chain, kind).id,
                Some(&addressed(chain, kind).etag),
            )
            .await;
            assert_eq!(
                answer.status, 409,
                "case {case}: expected HTTP 409 while the chain still references the record, got \
                 {} with {}",
                answer.status, answer.raw
            );
        }
    }
}

/// Release the records of the chain that reference the one of `kind`, so its own delete is
/// gated on nothing but the header: the configuration lets go of the profile, and the profile
/// lets go of the public key.
async fn release(router: &MultiServiceRouter, kind: Kind, chain: &Chain) {
    let removed = delete_config(router, &chain.config.id, Some(&chain.config.etag)).await;
    assert_eq!(
        removed.status, 204,
        "case releasing the chain: expected HTTP 204 deleting the configuration under the token \
         its create published, got {} with {}",
        removed.status, removed.raw
    );
    if kind == Kind::PublicKey {
        let removed = delete_profile(router, &chain.profile.id, Some(&chain.profile.etag)).await;
        assert_eq!(
            removed.status, 204,
            "case releasing the chain: expected HTTP 204 deleting the profile under the token its \
             create published, got {} with {}",
            removed.status, removed.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// RULE: only a WRITE moves a record of this surface on, so after any read, listing, probe or
/// refused call the record still reports the version its create published — and the conditional
/// delete presenting that very token is served HTTP 204 with an empty body.
#[tokio::test]
async fn a_read_or_a_refused_call_leaves_the_version_the_create_published() {
    for kind in KINDS {
        for (index, between) in BETWEEN.into_iter().enumerate() {
            // A fresh emulator per row: the delete each row ends with removes the record.
            let router = emulator();
            let reference = "version-survives";
            let stood_up = chain(&router, reference).await;
            let held = addressed(&stood_up, kind).clone();
            let case = format!("{kind:?} after {between:?}");

            make(&router, kind, &stood_up, reference, between, &case).await;

            // The record still reports the version its create published, through its own
            // reader: nothing above wrote to it.
            let reported = read_token(&router, kind, &held.id).await;
            assert_eq!(
                reported, held.etag,
                "case {case}: expected the record to keep the version its create published \
                 ({:?}), got {reported:?}",
                held.etag
            );

            // And the token the caller has been holding since the create still deletes it,
            // whichever of the three admissible ways the header spells it.
            let (spelling, spell) = SPELLINGS[index % SPELLINGS.len()];
            release(&router, kind, &stood_up).await;
            let served = delete(&router, kind, &held.id, Some(&spell(&held.etag))).await;
            assert_eq!(
                served.status, 204,
                "case {case}, deleted under the create's token spelled {spelling}: expected HTTP \
                 204, got {} with {}",
                served.status, served.raw
            );
            assert!(
                served.raw.trim().is_empty(),
                "case {case}: expected an empty body from the served delete, got {}",
                served.raw
            );
            let gone = read(&router, kind, &held.id).await;
            assert_eq!(
                gone.status, 404,
                "case {case}: expected the deleted record to be gone, got {} with {}",
                gone.status, gone.raw
            );
        }
    }
}

/// RULE: the delete is gated on the version the record is CURRENTLY at, so a readable token
/// that is not that version — the one the record published before a rewrite, one this account
/// holds for another record, or one this service never issued — is `PreconditionFailed` and
/// removes nothing, while the version the last write published is served.
#[tokio::test]
async fn a_token_that_is_not_the_records_current_version_is_refused() {
    for kind in KINDS {
        let router = emulator();
        let reference = "version-not-current";
        let stood_up = chain(&router, reference).await;
        let held = addressed(&stood_up, kind).clone();
        release(&router, kind, &stood_up).await;

        // Move the record on, so the token its create published is a version it has replaced.
        let moved_on = rewrite(&router, kind, &stood_up, reference, Some(&held.etag)).await;
        assert_eq!(
            moved_on.status, 200,
            "case {kind:?}: expected HTTP 200 from the rewrite under the create's token, got {} \
             with {}",
            moved_on.status, moved_on.raw
        );
        let current = moved_on.header("etag").to_string();
        assert_ne!(
            current, held.etag,
            "case {kind:?}: expected the rewrite to publish a version the record did not already \
             hold, got {current:?} again"
        );

        for (provenance, token) in [
            ("the version the record published before the rewrite", held.etag.clone()),
            (
                "the current version of the other record of the chain",
                other(&stood_up, kind).etag.clone(),
            ),
            (
                "a token this service never issued",
                UNISSUED_TOKEN.to_string(),
            ),
        ] {
            for (spelling, spell) in SPELLINGS {
                let case = format!("{kind:?} deleted under {provenance}, spelled {spelling}");
                let answer = delete(&router, kind, &held.id, Some(&spell(&token))).await;
                assert_refusal(
                    &answer,
                    &case,
                    412,
                    "PreconditionFailed",
                    PRECONDITION_FAILED_MESSAGE,
                );

                // The refusal removed nothing and advanced nothing: the record is still at the
                // version the rewrite published.
                let reported = read_token(&router, kind, &held.id).await;
                assert_eq!(
                    reported, current,
                    "case {case}: expected the record to keep the version {current:?}, got \
                     {reported:?}"
                );
            }
        }

        // The version the last write published is what the delete takes.
        let served = delete(&router, kind, &held.id, Some(&current)).await;
        assert_eq!(
            served.status, 204,
            "case {kind:?} deleted under the version the rewrite published: expected HTTP 204, \
             got {} with {}",
            served.status, served.raw
        );
    }
}

/// RULE: so the teardown of a whole field-level encryption chain completes under the three
/// tokens its three creates handed back, with the deliberately-failing probes of this surface
/// standing between the creates and the deletes.
#[tokio::test]
async fn the_chain_is_torn_down_under_the_tokens_its_creates_published() {
    let router = emulator();
    let reference = "version-teardown";
    let stood_up = chain(&router, reference).await;

    // The probes: an id the store holds no configuration under, a rewrite of each record
    // refused on the header's form and on the precondition, and the profile's delete refused
    // while the configuration still names it.
    let absent = get_config(&router, "C9ZZZZZZZZZZZZ").await;
    assert_eq!(
        absent.status, 404,
        "case probing a configuration id the store holds nothing under: expected HTTP 404, got \
         {} with {}",
        absent.status, absent.raw
    );
    for kind in KINDS {
        for between in [
            Between::RewriteRefusedOnTheHeader,
            Between::RewriteRefusedOnThePrecondition,
            Between::DeleteRefusedOnThePrecondition,
            Between::DeleteRefusedOnAReference,
        ] {
            let case = format!("the chain's {kind:?} under {between:?}");
            make(&router, kind, &stood_up, reference, between, &case).await;
        }
    }

    // And then the teardown, in the order a caller releases the chain: the configuration, the
    // profile it mapped to, and the public key that profile encrypted with — each under the
    // token its OWN create published, each 204 with an empty body.
    for (case, answer) in [
        (
            "the configuration",
            delete_config(&router, &stood_up.config.id, Some(&stood_up.config.etag)).await,
        ),
        (
            "the profile",
            delete_profile(&router, &stood_up.profile.id, Some(&stood_up.profile.etag)).await,
        ),
        (
            "the public key",
            delete_key(&router, &stood_up.key.id, Some(&stood_up.key.etag)).await,
        ),
    ] {
        assert_eq!(
            answer.status, 204,
            "case tearing down {case} under the token its create published: expected HTTP 204, \
             got {} with {}",
            answer.status, answer.raw
        );
        assert!(
            answer.raw.trim().is_empty(),
            "case tearing down {case}: expected an empty body, got {}",
            answer.raw
        );
    }

    // Gone means gone: every record of the chain is removed.
    for (case, answer) in [
        ("the configuration", get_config(&router, &stood_up.config.id).await),
        ("the profile", get_profile(&router, &stood_up.profile.id).await),
        ("the public key", get_key(&router, &stood_up.key.id).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case reading {case} after the teardown: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
    }
}
