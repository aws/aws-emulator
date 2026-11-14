// SPDX-License-Identifier: CC0-1.0

//! `UpdateFieldLevelEncryptionProfile` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this operation
//! ("Update a field-level encryption profile"):
//!
//! * the submitted `FieldLevelEncryptionProfileConfig` REPLACES the stored one whole, so the
//!   profile reader and the configuration reader both answer with it afterwards — the name,
//!   the comment (down to a member the request leaves out) and every encryption entity with
//!   its field patterns — while the id the create minted stays as it was,
//!   `LastModifiedTime` moves to the instant the update wrote the record and the mutation
//!   mints a fresh version token;
//! * the version token the last mutation handed back is the only one an `If-Match` against
//!   that profile satisfies: an absent or malformed header is `InvalidIfMatchVersion` (HTTP
//!   400), a well-formed token that is not the current one is `PreconditionFailed` (HTTP
//!   412), and the current one — bare, quoted or weak — is accepted;
//! * a `CallerReference` identifies one profile for its whole life, so an update that moves
//!   it is `IllegalUpdate` (HTTP 400);
//! * the checks the create path applies to a configuration apply on the update too, because
//!   a configuration is checked on every payload that states one: a list wrapper whose
//!   `Quantity` disagrees with its `Items` is `InconsistentQuantities` (HTTP 400) — over the
//!   `EncryptionEntities` wrapper and over the `FieldPatterns` wrapper each entity carries —
//!   a `Comment` past the documented 128 characters is `InvalidArgument` (HTTP 400), and an
//!   `EncryptionEntity$PublicKeyId` no stored public key carries is `NoSuchPublicKey` (HTTP
//!   404);
//! * an id no stored profile holds is `NoSuchFieldLevelEncryptionProfile` (HTTP 404);
//! * every refusal writes nothing: the stored configuration, its instant and its version
//!   token are exactly what they were.
//!
//! Each read-back goes through BOTH readers of the one record —
//! `GetFieldLevelEncryptionProfile` and `GetFieldLevelEncryptionProfileConfig` — so neither
//! can drift from what the update stored.
//!
//! The rows differ along everything these rules generalise over: the number of encryption
//! entities the update submits (none, one, several) and which public keys they name, the
//! number and form of the field patterns under each, whether the comment is absent, empty,
//! ASCII, XML metacharacters or exactly on the published bound, the form the `If-Match`
//! header takes, and which wrapper's quantity disagrees. The configuration writer, the
//! readers and the table are the crate's own, in `field_level_encryption_profile_lifecycle.rs`;
//! the recorded case is one row.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE, MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT,
    NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE, NO_SUCH_PUBLIC_KEY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, is_profile_id, is_wire_timestamp, table,
    upload_key, upload_keys, Entity, Profile,
};

/// The sentence CloudFront sends for a path identifier outside the shape its resource kind's
/// ids take, spelled out here rather than taken from the crate, so a reworded refusal fails
/// this suite.
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/field-level-encryption-profile/{Id}/config` with the
/// `FieldLevelEncryptionProfileConfig` document `body` and the `If-Match` header `if_match`
/// (left off entirely when `None`).
async fn update_profile(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// Create `row` and report the id, the version token and the instant the create recorded.
async fn seed(router: &MultiServiceRouter, row: &Profile) -> (String, String, String) {
    let created = create_profile(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected HTTP 201 from the create, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        is_profile_id(&id),
        "case {:?}: expected a minted profile id from the create, got {id:?}",
        row.case
    );
    let last_modified = created
        .root()
        .text_at("LastModifiedTime")
        .unwrap_or_default()
        .to_string();
    (id, created.header("etag").to_string(), last_modified)
}

/// Assert both readers of the profile `id` report `row`'s configuration at the version token
/// `etag` and the instant `last_modified` — the one stored record.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    row: &Profile,
    etag: &str,
    last_modified: &str,
    context: &str,
) {
    let case = &row.case;
    let read = get_profile(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?} ({context}): expected HTTP 200 from GetFieldLevelEncryptionProfile, got \
         {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Id"),
        Some(id),
        "case {case:?} ({context}): expected the profile to keep the id {id}, got {}",
        read.raw
    );
    assert_eq!(
        read.root().text_at("LastModifiedTime"),
        Some(last_modified),
        "case {case:?} ({context}): expected LastModifiedTime {last_modified:?}, got {:?}",
        read.root().text_at("LastModifiedTime")
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case {case:?} ({context}): expected the stored version token {etag}, got {:?}",
        read.header("etag")
    );
    row.assert_matches(
        read.root()
            .at("FieldLevelEncryptionProfileConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} ({context}): the read carries no configuration: {}",
                    read.raw
                )
            }),
        context,
        &read.raw,
    );

    // Parity with the configuration reader: both reads answer from the ONE record, so the
    // configuration and the version token they report are the same. The configuration is
    // that output's `@httpPayload`, so it IS the document's root element.
    let config = get_profile_config(router, id).await;
    assert_eq!(
        config.status, 200,
        "case {case:?} ({context}): expected HTTP 200 from \
         GetFieldLevelEncryptionProfileConfig, got {} with {}",
        config.status, config.raw
    );
    assert_eq!(
        config.header("etag"),
        etag,
        "case {case:?} ({context}): expected one version token across both reads, got {:?} and \
         {etag}",
        config.header("etag")
    );
    row.assert_matches(config.root(), context, &config.raw);
}

/// The configurations an update submits over a profile created from `row`, built over the
/// public keys `keys` holds.
///
/// Every member an update may move, moved: the name, the comment in each of its forms
/// (added, replaced, emptied, left out, and exactly on the published bound), and the
/// encryption entities taken from none to several and back, over different keys, provider
/// ids and field-pattern shapes.
fn targets(row: &Profile, keys: &[String]) -> Vec<Profile> {
    let on_the_bound = "c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT);
    let entity_sets: Vec<Vec<Entity>> = vec![
        Vec::new(),
        vec![Entity::new(&keys[1], "provider-updated", &["Updated-Field"])],
        vec![
            Entity::new(&keys[0], "provider-one", &["Card*", "SSN"]),
            Entity::new(&keys[2], "provider-three", &[&"L".repeat(64), "A<B>&C"]),
        ],
        vec![
            Entity::new(&keys[2], "provider-three", &["Only"]),
            Entity::new(&keys[1], "provider-two", &["X", "Y", "Z"]),
            Entity::new(&keys[0], "provider-one", &["Secret*"]),
        ],
    ];
    let comments: Vec<Option<String>> = vec![
        Some("updated once".to_string()),
        Some(String::new()),
        None,
        Some(on_the_bound),
        Some("プロダクション — tags <field> & \"quotes\"".to_string()),
    ];

    comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| Profile {
            case: format!("{} -> update {index}", row.case),
            // The name is a member an update may move, so each target renames the profile.
            name: format!("{}-updated-{index}", row.name),
            caller_reference: row.caller_reference.clone(),
            comment,
            entities_quantity: i32::try_from(entity_sets[index % entity_sets.len()].len())
                .unwrap_or(i32::MAX),
            entities: entity_sets[index % entity_sets.len()].clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An update under the profile's current version token REPLACES the stored configuration: the
/// answer and both readers report the submitted configuration, the id the create minted is
/// kept, `LastModifiedTime` is a wire instant the readers agree on, and the fresh version
/// token supersedes the one the update matched on.
#[tokio::test]
async fn an_update_replaces_the_stored_configuration_and_both_reads_report_it() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let mut etags = BTreeSet::new();

    for row in table(&keys) {
        let (id, create_etag, _) = seed(&router, &row).await;
        assert!(
            etags.insert(create_etag.clone()),
            "case {:?}: the version token {create_etag} repeated",
            row.case
        );
        let mut etag = create_etag;

        for target in targets(&row, &keys) {
            let answer = update_profile(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from the update, got {} with {}",
                target.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.root().text_at("Id"),
                Some(id.as_str()),
                "case {:?}: expected the update to keep the id {id}, got {}",
                target.case,
                answer.raw
            );
            target.assert_matches(
                answer
                    .root()
                    .at("FieldLevelEncryptionProfileConfig")
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: the update carries no configuration: {}",
                            target.case, answer.raw
                        )
                    }),
                "the update",
                &answer.raw,
            );

            let written = answer
                .root()
                .text_at("LastModifiedTime")
                .unwrap_or_default()
                .to_string();
            assert!(
                is_wire_timestamp(&written),
                "case {:?}: expected a wire timestamp for LastModifiedTime, got {written:?}",
                target.case
            );

            let fresh = answer.header("etag").to_string();
            assert!(
                is_version_token(&fresh),
                "case {:?}: expected a version token in the ETag header, got {fresh:?}",
                target.case
            );
            assert_ne!(
                fresh, etag,
                "case {:?}: expected the update to mint a fresh version token, got {fresh} again",
                target.case
            );
            assert!(
                etags.insert(fresh.clone()),
                "case {:?}: the version token {fresh} repeated",
                target.case
            );

            assert_stored(&router, &id, &target, &fresh, &written, "after the update").await;

            // The token the update matched on is superseded.
            let stale = update_profile(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                stale.status, 412,
                "case {:?}: expected HTTP 412 for the superseded token {etag}, got {} with {}",
                target.case, stale.status, stale.raw
            );
            assert_eq!(
                stale.error_code(),
                "PreconditionFailed",
                "case {:?}: expected PreconditionFailed, got {}",
                target.case,
                stale.raw
            );

            etag = fresh;
        }
    }
}

/// An `If-Match` the profile's version cannot satisfy is refused and writes nothing: an
/// absent header or one carrying no version token at all is `InvalidIfMatchVersion` (HTTP
/// 400), and a well-formed token that is not this profile's is `PreconditionFailed` (HTTP
/// 412). The current token is accepted bare, quoted and weak.
#[tokio::test]
async fn an_if_match_the_profile_cannot_satisfy_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let subject = Profile::new(
        "the profile every version refusal is asserted on",
        "fle-profile-version-gate",
        Some("gated on its own ETag"),
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    let other = Profile::new(
        "another profile, whose token is not the subject's",
        "fle-profile-version-other",
        None,
        vec![Entity::new(&keys[1], "provider-two", &["SSN"])],
    );
    let (_, other_etag, _) = seed(&router, &other).await;
    let (id, etag, instant) = seed(&router, &subject).await;
    let body = Profile {
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
            "a version token that is another profile's",
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
        let answer = update_profile(&router, &id, &body, if_match.as_deref()).await;
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
        assert_stored(&router, &id, &subject, &etag, &instant, case).await;
    }

    // The current token in each form an entity tag takes, on a fresh profile per form.
    for (index, form) in ["{token}", "\"{token}\"", "W/\"{token}\""]
        .into_iter()
        .enumerate()
    {
        let fresh_profile = Profile::new(
            &format!("the accepted token form {index}"),
            &format!("fle-profile-version-accepted-{index}"),
            None,
            vec![Entity::new(&keys[index % keys.len()], "provider-one", &["Field"])],
        );
        let (fresh_id, fresh_etag, _) = seed(&router, &fresh_profile).await;
        let target = Profile {
            comment: Some(format!("accepted with form {index}")),
            ..fresh_profile.clone()
        };
        let header = form.replace("{token}", &fresh_etag);
        let accepted = update_profile(&router, &fresh_id, &target.xml(), Some(&header)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the current token as {header:?} to be accepted, got {} with {}",
            target.case, accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &fresh_id,
            &target,
            accepted.header("etag"),
            accepted.root().text_at("LastModifiedTime").unwrap_or_default(),
            "after the accepted token form",
        )
        .await;
    }
}

/// A caller reference names one profile for its whole life, so an update that moves it is
/// `IllegalUpdate` (HTTP 400) with the sentence this service refuses that modification with,
/// and the stored record is exactly what it was. The same request with the stored reference
/// then lands, so the refusal is keyed on the member that moved rather than on the request's
/// shape.
#[tokio::test]
async fn an_update_that_moves_the_caller_reference_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    for row in table(&keys) {
        let (id, etag, instant) = seed(&router, &row).await;

        // One row per form the change takes: another value, an empty one, a longer one, the
        // same value in another case, and another profile's reference.
        let references: [(&str, String); 5] = [
            ("another value", format!("{}-moved", row.caller_reference)),
            ("an empty reference", String::new()),
            (
                "a far longer reference",
                format!("{}-{}", row.caller_reference, "r".repeat(64)),
            ),
            (
                "the stored reference in upper case",
                row.caller_reference.to_uppercase(),
            ),
            ("the first row's reference", "fle-profile-empty".to_string()),
        ];

        for (description, reference) in &references {
            if *reference == row.caller_reference {
                continue;
            }
            let case = format!("{} / {description}", row.case);
            let moved = Profile {
                case: case.clone(),
                caller_reference: reference.clone(),
                ..row.clone()
            };
            let answer = update_profile(&router, &id, &moved.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "IllegalUpdate",
                "case {case}: expected IllegalUpdate, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE,
                "case {case}: expected the caller-reference sentence, got {}",
                answer.raw
            );
            assert_stored(&router, &id, &row, &etag, &instant, &case).await;
        }

        // The control: the same request with the stored reference lands.
        let legal = Profile {
            case: format!("{} / the stored reference", row.case),
            comment: Some("the legal update after the refusals".to_string()),
            ..row.clone()
        };
        let accepted = update_profile(&router, &id, &legal.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the update under the stored reference to land, got {} with {}",
            legal.case, accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &id,
            &legal,
            accepted.header("etag"),
            accepted.root().text_at("LastModifiedTime").unwrap_or_default(),
            "after the legal update",
        )
        .await;
    }
}

/// The checks the create path applies to a configuration apply to an update of one: a wrapper
/// whose `Quantity` disagrees with its `Items` is `InconsistentQuantities` (HTTP 400) —
/// whichever wrapper disagrees — a comment past the documented 128 characters is
/// `InvalidArgument` (HTTP 400) while the bound itself lands, and an encryption entity naming
/// a public key the store does not hold is `NoSuchPublicKey` (HTTP 404). Every one of them
/// writes nothing.
#[tokio::test]
async fn an_update_whose_configuration_the_create_would_refuse_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let row = Profile::new(
        "the profile the configuration checks are asserted on",
        "fle-profile-update-checks",
        Some("the stored comment"),
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    let (id, etag, instant) = seed(&router, &row).await;

    // The `EncryptionEntities` wrapper and the `FieldPatterns` wrapper of an entity, each
    // overstated and understated, plus a wrapper stating a count with no `Items` beside it.
    let mut entities_overstated = row.clone();
    entities_overstated.entities_quantity += 1;
    let mut entities_understated = row.clone();
    entities_understated.entities_quantity = 0;
    let mut patterns_overstated = row.clone();
    patterns_overstated.entities[0].patterns_quantity += 2;
    let mut patterns_understated = row.clone();
    patterns_understated.entities[0].patterns_quantity = 0;
    let mut empty_but_counted = row.clone();
    empty_but_counted.entities = Vec::new();
    empty_but_counted.entities_quantity = 2;

    for (case, body) in [
        ("EncryptionEntities overstated", entities_overstated),
        ("EncryptionEntities understated", entities_understated),
        ("FieldPatterns overstated", patterns_overstated),
        ("FieldPatterns understated", patterns_understated),
        ("a count with no Items at all", empty_but_counted),
    ] {
        let answer = update_profile(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &row, &etag, &instant, case).await;
    }

    // "The comment cannot be longer than 128 characters": one character past the bound and
    // far past it are refused; the bound itself is admissible.
    for (case, length) in [
        (
            "one character past the comment bound",
            MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT + 1,
        ),
        (
            "far past the comment bound",
            MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT * 4,
        ),
    ] {
        let body = Profile {
            comment: Some("c".repeat(length)),
            ..row.clone()
        };
        let answer = update_profile(&router, &id, &body.xml(), Some(&etag)).await;
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
        assert_stored(&router, &id, &row, &etag, &instant, case).await;
    }

    // An entity naming a public key the store does not hold, in each form the unknown id
    // takes, and one row where only the SECOND entity names it.
    let present = keys[0].clone();
    let unknown_rows: Vec<(&str, Profile)> = vec![
        (
            "an id never minted",
            Profile {
                entities: vec![Entity::new("K2NEVERMINTED1", "provider-one", &["CreditCard"])],
                ..row.clone()
            },
        ),
        (
            "the stored key's id in lower case",
            Profile {
                entities: vec![Entity::new(
                    &present.to_lowercase(),
                    "provider-one",
                    &["CreditCard"],
                )],
                ..row.clone()
            },
        ),
        (
            "a token of no public key id's shape",
            Profile {
                entities: vec![Entity::new("not-a-key", "provider-one", &["CreditCard"])],
                ..row.clone()
            },
        ),
        (
            "a second entity naming an unknown key",
            Profile {
                entities_quantity: 2,
                entities: vec![
                    Entity::new(&present, "provider-one", &["CreditCard"]),
                    Entity::new("K3ALSONEVERMI1", "provider-two", &["SSN"]),
                ],
                ..row.clone()
            },
        ),
    ];

    for (case, body) in &unknown_rows {
        let answer = update_profile(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchPublicKey",
            "case {case}: expected NoSuchPublicKey, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_PUBLIC_KEY_MESSAGE,
            "case {case}: expected the modelled message, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &row, &etag, &instant, case).await;
    }

    // The comment exactly on the bound, over an entity naming a key uploaded afterwards:
    // every refusal above left the record able to take a legal update.
    let late_key = upload_key(&router, 7).await;
    let at_bound = Profile {
        case: "the comment on the bound, over a key uploaded later".to_string(),
        comment: Some("c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT)),
        entities_quantity: 1,
        entities: vec![Entity::new(&late_key, "provider-late", &["Card*", "SSN"])],
        ..row.clone()
    };
    let accepted = update_profile(&router, &id, &at_bound.xml(), Some(&etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case the comment on the bound: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert_stored(
        &router,
        &id,
        &at_bound,
        accepted.header("etag"),
        accepted.root().text_at("LastModifiedTime").unwrap_or_default(),
        "after the accepted update",
    )
    .await;
}

/// An id no stored profile holds is `NoSuchFieldLevelEncryptionProfile` (HTTP 404) whatever
/// shape the id takes and whatever the `If-Match` header carries, and the refused update
/// creates nothing under it. The MESSAGE follows the id's SHAPE, judged before the store is
/// consulted: an id outside the shape a profile id takes is the identifier the service cannot
/// read, a well-formed one the store holds nothing for is the absent profile.
#[tokio::test]
async fn an_id_no_profile_holds_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let present = Profile::new(
        "the one profile that exists",
        "fle-profile-update-unknown",
        None,
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    let (id, etag, instant) = seed(&router, &present).await;
    let body = Profile {
        comment: Some("never stored".to_string()),
        ..present.clone()
    }
    .xml();

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let candidates: [(&str, String, &str); 4] = [
        (
            "an id never minted",
            "P2NEVERMINTED1".to_string(),
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE,
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE,
        ),
        (
            "the existing id in lower case",
            id.to_lowercase(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a token of no profile id's shape",
            "not-a-profile".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
    ];

    for (case, candidate, expected) in &candidates {
        for if_match in [None, Some(etag.clone()), Some("not-a-version".to_string())] {
            let answer = update_profile(&router, candidate, &body, if_match.as_deref()).await;
            assert_eq!(
                answer.status, 404,
                "case {case} with If-Match {if_match:?}: expected HTTP 404 for {candidate:?}, got \
                 {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFieldLevelEncryptionProfile",
                "case {case} with If-Match {if_match:?}: expected \
                 NoSuchFieldLevelEncryptionProfile, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                *expected,
                "case {case} with If-Match {if_match:?}: expected {expected:?}, got {}",
                answer.raw
            );
        }
        assert_eq!(
            get_profile(&router, candidate).await.status,
            404,
            "case {case}: expected the refused update to create nothing under {candidate:?}"
        );
        assert_eq!(
            get_profile_config(&router, candidate).await.status,
            404,
            "case {case}: expected the config read of {candidate:?} to report no profile"
        );
    }

    // The profile that does exist is untouched by every refusal above.
    assert_stored(
        &router,
        &id,
        &present,
        &etag,
        &instant,
        "after the refusals",
    )
    .await;
}
