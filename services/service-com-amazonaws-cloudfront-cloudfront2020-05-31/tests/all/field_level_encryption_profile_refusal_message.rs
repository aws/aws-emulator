// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a field-level encryption profile refusal carries, over every path that raises
//! it.
//!
//! The rule under test: a modelled CloudFront refusal carries the sentence the SERVICE sends
//! on the wire for that error class — one sentence per code, the same on every operation that
//! raises it, stating the condition and, for a resource that is still referenced, the
//! remediation — and never the error shape's model documentation prose, which describes the
//! condition for a reader of the model rather than answering a client.
//!
//! So the rows here span what the rule generalises over:
//!
//! * the OPERATIONS that raise `NoSuchFieldLevelEncryptionProfile` — both readers of the
//!   profile, its update and its delete, where the id travels in the path, and the create and
//!   the update of a field-level encryption CONFIGURATION, where it travels in the submitted
//!   document as a mapping's `ProfileId`;
//! * the SHAPES of an id the store holds no profile for (one of the issued form never minted,
//!   a stored id in lower case, a token that is no profile id at all, and a caller reference
//!   rather than an id), each of which earns the sentence its own form decides;
//! * which MAPPING KIND of which section of a configuration still names the profile a delete
//!   is refused for — a query argument-profile mapping, a content type-profile mapping, or one
//!   of each at once.
//!
//! The id travels in the path on four of those operations and in a submitted DOCUMENT on the
//! other two. An id borne by a document addresses nothing, so any id the store holds no profile
//! for is the absent profile there; an id that ADDRESSES the operation is read for whether it
//! could be an identifier issued FOR A PROFILE first, and one that could not — a value written
//! outside the alphabet an identifier is written in, and a value written exactly as an
//! identifier is ISSUED under another kind's namespace letter — is the identifier reported
//! unreadable under that same one not-found code; these operations model no argument error. A
//! value of the alphabet at any other width names no kind of its own, so it is carried through
//! to the store and is the absent profile. The rows below span those shapes — another namespace
//! of this service, a value of no minted id's alphabet, a caller reference — beside well-formed
//! profile ids nothing answers to.
//!
//! Every message is also asserted NOT to be the documentation prose of its error shape
//! (`smithy_query(query="doc",shape="NoSuchFieldLevelEncryptionProfile")` and
//! `…shape="FieldLevelEncryptionProfileInUse"`), so the substitution this suite pins cannot
//! come back unnoticed.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the profile and configuration
//! builders are the ones the lifecycle suites publish, and the configuration delete is
//! [`crate::field_level_encryption_config_delete`]'s — reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::field_level_encryption_config_delete::delete_config;
use crate::field_level_encryption_config_lifecycle::{
    create_config, simple, Config, ContentTypeMapping, QueryArgMapping, Section,
};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, is_profile_id, upload_key, Entity, Profile,
};

/// The sentences CloudFront sends on the wire for these refusals, spelled out here rather than
/// taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_PROFILE_MESSAGE: &str = "The specified field level encryption profile does not exist.";
const PROFILE_IN_USE_MESSAGE: &str =
    "The field level encryption profile is currently associated with a field level encryption \
     config. Please disassociate the profile before deleting.";

/// The sentence an operation ADDRESSED by a profile `Id` answers a value that could never be an
/// identifier issued for a profile, naming the member that carried it.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The MODEL's documentation prose for those two error shapes — what the shapes say ABOUT the
/// condition, which is not what the service answers with. Source:
/// `smithy_query(query="doc",shape="NoSuchFieldLevelEncryptionProfile")` and
/// `smithy_query(query="doc",shape="FieldLevelEncryptionProfileInUse")`.
const PROFILE_DOC_PROSE: [&str; 2] = [
    "The specified profile for field-level encryption doesn't exist.",
    "The specified profile for field-level encryption is in use.",
];

// ---------------------------------------------------------------------------
// The paths that raise these codes
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/field-level-encryption-profile/{Id}`.
async fn delete_profile(
    router: &MultiServiceRouter,
    profile_id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/field-level-encryption-profile/{profile_id}"),
        None,
        if_match,
    )
    .await
}

/// `PUT /2020-05-31/field-level-encryption-profile/{Id}/config` with a
/// `FieldLevelEncryptionProfileConfig` document.
async fn update_profile(
    router: &MultiServiceRouter,
    profile_id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption-profile/{profile_id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// `PUT /2020-05-31/field-level-encryption/{Id}/config` with a `FieldLevelEncryptionConfig`
/// document.
async fn update_config(
    router: &MultiServiceRouter,
    config_id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption/{config_id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// Store one profile that encrypts with a freshly uploaded public key, reporting its minted id
/// and its current version token.
async fn store_profile_with_token(
    router: &MultiServiceRouter,
    reference: &str,
    key: &str,
) -> (String, String) {
    let row = Profile::new(
        "a stored profile",
        reference,
        None,
        vec![Entity::new(key, "provider-one", &["CreditCard"])],
    );
    let answer = create_profile(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case storing the profile {reference:?}: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_profile_id(&id),
        "case storing the profile {reference:?}: expected a minted profile id, got {id:?}"
    );
    (id, etag)
}

/// Store `config` and report its minted id and version token.
async fn store_config(router: &MultiServiceRouter, config: &Config) -> (String, String) {
    let answer = create_config(router, &config.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
        config.case, answer.status, answer.raw
    );
    (
        answer.root().text_at("Id").unwrap_or_default().to_string(),
        answer.header("etag").to_string(),
    )
}

/// Assert one refusal: its status, its code and — the point of this suite — its message,
/// which must be the wire sentence and must not be any of the shapes' documentation prose.
fn assert_refusal(answer: &Answer, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected code {code}, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case:?}: expected message {message:?}, got {actual:?}"
    );
    for prose in PROFILE_DOC_PROSE {
        assert_ne!(
            actual, prose,
            "case {case:?}: expected the wire sentence, got the error shape's model \
             documentation {prose:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// Every path that reports a field-level encryption profile id as naming nothing answers
/// `NoSuchFieldLevelEncryptionProfile`, with the unreadable-identifier sentence where the id
/// could never be one issued for a profile — written outside the alphabet this service writes an
/// identifier in, or written as another kind's issued identifier — and the absent-profile wire
/// sentence for every id that could be one — while an id that travels in a submitted DOCUMENT as
/// a mapping's `ProfileId` addresses nothing, so it reports the absent profile whatever shape it
/// takes.
#[tokio::test]
async fn every_path_that_reports_a_missing_profile_sends_the_wire_sentence() {
    let router = emulator();
    let key = upload_key(&router, 0).await;
    let (known, known_etag) =
        store_profile_with_token(&router, "fle-profile-message-known", &key).await;

    // A stored configuration to aim the configuration UPDATE at: it names the stored profile,
    // so the update below differs from it only in the profile id it moves to.
    let referencing = simple(
        "a configuration naming the stored profile",
        "fle-profile-message-config",
        &known,
    );
    let (config_id, config_etag) = store_config(&router, &referencing).await;

    // The shapes an id takes that no stored profile carries, each earning the one sentence an
    // operation addressed by it answers: a well-formed profile id never minted, a stored id
    // with a character changed, a stored id in lower case, a token that is no profile id and a
    // caller reference.
    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let absent: [(&str, String, &str); 5] = [
        (
            "a profile id never minted",
            "P0000000000000".to_string(),
            NO_SUCH_PROFILE_MESSAGE,
        ),
        (
            "a stored id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_PROFILE_MESSAGE,
        ),
        (
            "a stored id in lower case",
            known.to_lowercase(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a token that is no profile id",
            "not-a-profile-id".to_string(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "the caller reference rather than the id",
            "fle-profile-message-known".to_string(),
            UNREADABLE_IDENTIFIER,
        ),
    ];

    let body = Profile::new(
        "a well-formed profile document",
        "fle-profile-message-update",
        None,
        vec![Entity::new(&key, "provider-one", &["CreditCard"])],
    )
    .xml();

    for (shape, id, addressed_message) in &absent {
        // The id in the PATH: both readers, the update and the delete.
        let answers = [
            ("GetFieldLevelEncryptionProfile", get_profile(&router, id).await),
            (
                "GetFieldLevelEncryptionProfileConfig",
                get_profile_config(&router, id).await,
            ),
            (
                "UpdateFieldLevelEncryptionProfile",
                update_profile(&router, id, &body, Some(&known_etag)).await,
            ),
            (
                "DeleteFieldLevelEncryptionProfile",
                delete_profile(&router, id, Some(&known_etag)).await,
            ),
        ];
        for (operation, answer) in &answers {
            assert_refusal(
                answer,
                &format!("{operation} with {shape}"),
                404,
                "NoSuchFieldLevelEncryptionProfile",
                addressed_message,
            );
        }

        // The id in the DOCUMENT: a configuration create and a configuration update whose
        // mapping names it.
        let created = create_config(
            &router,
            &simple(
                "a configuration naming a profile the store does not hold",
                &format!("fle-profile-message-create-{}", id.to_lowercase()),
                id,
            )
            .xml(),
        )
        .await;
        assert_refusal(
            &created,
            &format!("CreateFieldLevelEncryptionConfig with {shape}"),
            404,
            "NoSuchFieldLevelEncryptionProfile",
            NO_SUCH_PROFILE_MESSAGE,
        );

        let updated = update_config(
            &router,
            &config_id,
            &simple(
                "a configuration moved onto a profile the store does not hold",
                "fle-profile-message-config",
                id,
            )
            .xml(),
            Some(&config_etag),
        )
        .await;
        assert_refusal(
            &updated,
            &format!("UpdateFieldLevelEncryptionConfig with {shape}"),
            404,
            "NoSuchFieldLevelEncryptionProfile",
            NO_SUCH_PROFILE_MESSAGE,
        );
    }

    // None of those refusals disturbed the profile the account really holds, so each message
    // above answered a missing id rather than a broken store.
    let fetched = get_profile(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the stored profile: expected it to survive every refusal, got {} with {}",
        fetched.status, fetched.raw
    );
}

/// A READ of a profile answers its one not-found code for every `Id` it cannot resolve, and
/// whether the value could be an identifier issued FOR A PROFILE decides the sentence: a value
/// written in the alphabet this service writes an identifier in is carried through to the STORE,
/// so it reports the profile that is ABSENT — as is a value written in the OTHER form CloudFront
/// issues an identifier in, the UUID its key groups and policy kinds are minted with, which
/// carries no namespace marker at all — while a value written outside both forms, and a
/// value written exactly as an identifier is ISSUED under ANOTHER kind's namespace letter, which
/// makes it that kind's identifier, is the identifier reported UNREADABLE.
///
/// The rows span the shapes an `Id` takes outside a profile id's: the namespaces of this
/// service a profile id's is disjoint from, taking two of them from ids this emulator really
/// minted, a stored id in another case, a bare word and the profile's own caller reference —
/// beside a well-formed profile id that was never minted.
#[tokio::test]
async fn a_profile_read_answers_one_sentence_for_every_id_it_cannot_resolve() {
    let router = emulator();
    let key = upload_key(&router, 0).await;
    let (known, _) = store_profile_with_token(&router, "fle-profile-kinds-known", &key).await;
    let (config_id, _) = store_config(
        &router,
        &simple(
            "a configuration whose id is of the sibling resource kind",
            "fle-profile-kinds-config",
            &known,
        ),
    )
    .await;

    // `(case, the Id submitted, the sentence the read answers)`
    let rows: [(&str, String, &str); 8] = [
        (
            "the sibling configuration's own id",
            config_id.clone(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a public key id this service minted",
            key.clone(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a distribution id's namespace",
            "E1234567890ABC".to_string(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a UUID, the namespace the policy kinds are keyed by",
            "00000000-0000-0000-0000-000000000000".to_string(),
            NO_SUCH_PROFILE_MESSAGE,
        ),
        (
            "a profile id never minted",
            "P0000000000000".to_string(),
            NO_SUCH_PROFILE_MESSAGE,
        ),
        (
            "the stored id in lower case",
            known.to_lowercase(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a bare word",
            "not-a-profile-id".to_string(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "the profile's caller reference rather than its id",
            "fle-profile-kinds-known".to_string(),
            UNREADABLE_IDENTIFIER,
        ),
    ];

    for (case, id, message) in &rows {
        let answer = get_profile(&router, id).await;
        assert_refusal(
            &answer,
            &format!("GetFieldLevelEncryptionProfile with {case}"),
            404,
            "NoSuchFieldLevelEncryptionProfile",
            message,
        );
    }

    // The profile the account really holds is still readable, so every row above answered an
    // identifier rather than a broken store.
    let fetched = get_profile(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the stored profile: expected it to survive every refusal, got {} with {}",
        fetched.status, fetched.raw
    );
}

/// How one row below builds the configuration that references the subject profile.
type BuildConfig = fn(&str) -> Config;

/// A delete refused because a stored configuration still names the profile answers
/// `FieldLevelEncryptionProfileInUse` with the ONE wire sentence for that code — which names
/// the remediation — whichever mapping kind of whichever section holds the reference.
#[tokio::test]
async fn every_in_use_refusal_sends_the_wire_sentence() {
    let references: &[(&str, BuildConfig)] = &[
        ("a query argument-profile mapping names the profile", |profile| {
            Config::new(
                "a query argument-profile mapping names the profile",
                "fle-profile-message-in-use",
                None,
                Some(Section::new(
                    true,
                    Some(vec![QueryArgMapping::new("fle-profile", profile)]),
                )),
                None,
            )
        }),
        ("a content type-profile mapping names the profile", |profile| {
            Config::new(
                "a content type-profile mapping names the profile",
                "fle-profile-message-in-use",
                None,
                None,
                Some(Section::new(
                    false,
                    Some(vec![ContentTypeMapping::new(
                        "application/x-www-form-urlencoded",
                        Some(profile),
                    )]),
                )),
            )
        }),
        ("a mapping of each section names the profile", |profile| {
            Config::new(
                "a mapping of each section names the profile",
                "fle-profile-message-in-use",
                None,
                Some(Section::new(
                    false,
                    Some(vec![QueryArgMapping::new("fle-profile", profile)]),
                )),
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/xml", Some(profile))]),
                )),
            )
        }),
    ];

    for (case, build) in references {
        let router = emulator();
        let key = upload_key(&router, 0).await;
        let (profile_id, profile_etag) =
            store_profile_with_token(&router, "fle-profile-message-subject", &key).await;

        let config = build(&profile_id);
        let (config_id, config_etag) = store_config(&router, &config).await;

        let refused = delete_profile(&router, &profile_id, Some(&profile_etag)).await;
        assert_refusal(
            &refused,
            case,
            409,
            "FieldLevelEncryptionProfileInUse",
            PROFILE_IN_USE_MESSAGE,
        );

        // The refusal left the profile where it was, so the same delete lands once the
        // configuration has let go of it — and the sentence above really did refuse a delete
        // of a live, referenced profile.
        assert_eq!(
            get_profile(&router, &profile_id).await.status,
            200,
            "case {case:?}: expected the refused delete to leave the profile readable"
        );
        let removed = delete_config(&router, &config_id, Some(&config_etag)).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: expected HTTP 204 deleting the configuration, got {} with {}",
            removed.status, removed.raw
        );
        let deleted = delete_profile(&router, &profile_id, Some(&profile_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no configuration names the profile, got {} \
             with {}",
            deleted.status, deleted.raw
        );

        // And the profile that is now gone reports the MISSING-profile sentence, not the
        // in-use one: one sentence per code, on the same operation.
        let again = delete_profile(&router, &profile_id, Some(&profile_etag)).await;
        assert_refusal(
            &again,
            &format!("{case} — the profile after its delete"),
            404,
            "NoSuchFieldLevelEncryptionProfile",
            NO_SUCH_PROFILE_MESSAGE,
        );
    }
}
