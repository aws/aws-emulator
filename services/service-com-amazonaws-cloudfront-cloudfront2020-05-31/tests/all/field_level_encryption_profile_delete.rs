// SPDX-License-Identifier: CC0-1.0

//! `DeleteFieldLevelEncryptionProfile`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states:
//!
//! * the delete REMOVES the stored record every read answered from, so the id afterwards names
//!   no profile (`NoSuchFieldLevelEncryptionProfile`, HTTP 404) — through the profile reader as
//!   well as the configuration one — the listing stops reporting it, and the caller reference
//!   it held is free for a later create to claim under a new id;
//! * the delete is gated on the profile's own version token: an absent `If-Match` and one that
//!   is no version token at all are `InvalidIfMatchVersion` (HTTP 400), a well-formed token
//!   that is not this profile's current one is `PreconditionFailed` (HTTP 412), and in either
//!   case the profile stays exactly where it was;
//! * a field-level encryption configuration names the profile its matching fields are
//!   encrypted under through a mapping's `ProfileId` — on a query argument-profile mapping or
//!   on a content type-profile one — so a profile any stored configuration still names is
//!   `FieldLevelEncryptionProfileInUse` (HTTP 409) and stays readable, and once every
//!   configuration has let go the same delete lands.
//!
//! The rows span what those rules generalise over: every profile of
//! [`crate::field_level_encryption_profile_lifecycle::table`] (no encryption entity, one and
//! several; one field pattern and several; the forms a pattern takes; and comments that are
//! absent, empty, plain ASCII, XML metacharacters or exactly on the published bound), every
//! form an `If-Match` takes (absent, empty, bare, quoted, weak, lower case, a token of another
//! resource, a value that is no token), which mapping kind of which section names the profile,
//! and ids of several shapes the store holds nothing for.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the profile create and
//! the two readers are [`crate::field_level_encryption_profile_lifecycle`]'s, and the
//! configuration that points at a profile is built and removed through the configuration
//! suites' own helpers — reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::field_level_encryption_config_delete::delete_config;
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, is_config_id, Config, ContentTypeMapping, QueryArgMapping, Section,
};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, is_profile_id, table, upload_keys, Entity,
    Profile,
};

/// The sentences CloudFront sends for these refusals, spelled out here rather than taken from
/// the crate, so a reworded refusal fails this suite.
const NO_SUCH_PROFILE_MESSAGE: &str = "The specified field level encryption profile does not exist.";
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";
const PROFILE_IN_USE_MESSAGE: &str =
    "The field level encryption profile is currently associated with a field level encryption \
     config. Please disassociate the profile before deleting.";

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's own
/// `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

/// `DELETE /2020-05-31/field-level-encryption-profile/{Id}` under the version token `if_match`
/// — the route and the header the model binds this operation to (`IfMatch` travels in
/// `If-Match`).
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

/// `GET /2020-05-31/field-level-encryption-profile` — the listing, used here only to check that
/// a deleted profile is no longer reported.
async fn list_profiles(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption-profile"),
        None,
        None,
    )
    .await
}

/// Create `row`, returning its minted id and its current version token.
async fn create(router: &MultiServiceRouter, row: &Profile) -> (String, String) {
    let answer = create_profile(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionProfile, got {} with {}",
        row.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_profile_id(&id),
        "case {:?}: expected a minted profile id, got {id:?}",
        row.case
    );
    assert!(
        is_version_token(&etag),
        "case {:?}: expected a minted version token, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Assert that `id` names no field-level encryption profile any more: both readers answer
/// `NoSuchFieldLevelEncryptionProfile` with the modelled sentence, and the listing has stopped
/// reporting it.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    for (reader, answer) in [
        ("GetFieldLevelEncryptionProfile", get_profile(router, id).await),
        (
            "GetFieldLevelEncryptionProfileConfig",
            get_profile_config(router, id).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from {reader} after the delete, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionProfile",
            "case {case:?}: expected NoSuchFieldLevelEncryptionProfile from {reader} after the \
             delete, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_PROFILE_MESSAGE,
            "case {case:?}: expected the modelled missing-profile sentence from {reader}, got \
             {message:?}"
        );
    }

    let listed = list_profiles(router).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListFieldLevelEncryptionProfiles, got {} with {}",
        listed.status, listed.raw
    );
    assert!(
        !listed.raw.contains(id),
        "case {case:?}: expected the listing to stop reporting the deleted profile {id:?}, got {}",
        listed.raw
    );
}

/// A configuration whose only mapping is one query argument naming `profile`.
fn query_arg_config(case: &str, reference: &str, profile: &str) -> Config {
    Config::new(
        case,
        reference,
        None,
        Some(Section::new(
            true,
            Some(vec![QueryArgMapping::new("fle-profile", profile)]),
        )),
        None,
    )
}

/// A configuration whose only mapping is one content type naming `profile` — the mapping kind
/// whose `ProfileId` is OPTIONAL.
fn content_type_config(case: &str, reference: &str, profile: Option<&str>) -> Config {
    Config::new(
        case,
        reference,
        None,
        None,
        Some(Section::new(
            false,
            Some(vec![ContentTypeMapping::new(
                "application/x-www-form-urlencoded",
                profile,
            )]),
        )),
    )
}

/// A configuration naming `profile` from a mapping in BOTH sections, with a second mapping in
/// each that names something else or nothing at all.
fn both_sections_config(case: &str, reference: &str, profile: &str, other: &str) -> Config {
    Config::new(
        case,
        reference,
        None,
        Some(Section::new(
            false,
            Some(vec![
                QueryArgMapping::new("other-profile", other),
                QueryArgMapping::new("fle-profile", profile),
            ]),
        )),
        Some(Section::new(
            true,
            Some(vec![
                ContentTypeMapping::new("text/xml", None),
                ContentTypeMapping::new("application/x-www-form-urlencoded", Some(profile)),
            ]),
        )),
    )
}

/// Store `config` and report its minted id and version token.
async fn store_config(router: &MultiServiceRouter, config: &Config) -> (String, String) {
    let answer = create_config(router, &config.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
        config.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_config_id(&id),
        "case {:?}: expected a minted configuration id, got {id:?}",
        config.case
    );
    (id, etag)
}

/// Every profile the create accepts is removed by its delete under its version token: the
/// delete answers HTTP 204 with no body, both readers then report the id as naming nothing,
/// deleting it again is `NoSuchFieldLevelEncryptionProfile`, and the caller reference it held is
/// free for a NEW profile under a new id.
#[tokio::test]
async fn every_stored_profile_is_removed_by_its_delete() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    for row in table(&keys) {
        let (id, etag) = create(&router, &row).await;

        // Round trip: the profile is readable before the delete, so the 404 after it is the
        // delete's doing and not a profile that was never stored.
        let fetched = get_profile(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryptionProfile before the delete, \
             got {} with {}",
            row.case, fetched.status, fetched.raw
        );

        let deleted = delete_profile(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 from DeleteFieldLevelEncryptionProfile, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {:?}: expected no body from a 204, got {}",
            row.case,
            deleted.raw
        );

        assert_gone(&router, &id, &row.case).await;
        let again = delete_profile(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: expected HTTP 404 from a second delete, got {} with {}",
            row.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchFieldLevelEncryptionProfile",
            "case {:?}: expected NoSuchFieldLevelEncryptionProfile from a second delete, got {}",
            row.case,
            again.raw
        );

        // The caller reference the deleted profile held identifies nothing any more, so a create
        // under it mints a SECOND profile rather than reporting the one that is gone.
        let (reused_id, reused_etag) = create(&router, &row).await;
        assert_ne!(
            reused_id, id,
            "case {:?}: expected a freshly minted id for the profile recreated under the freed \
             caller reference, got the deleted one {id:?}",
            row.case
        );

        // Leave the store as this row found it, so the next row's reference is the only one in
        // play.
        let cleanup = delete_profile(&router, &reused_id, Some(&reused_etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {:?}: expected HTTP 204 deleting the recreated profile, got {} with {}",
            row.case, cleanup.status, cleanup.raw
        );
    }
}

/// The delete is gated on the profile's OWN version token.
///
/// The rows span every form an `If-Match` takes: absent, empty, a value that is no version token
/// at all, a token spelled in lower case, a well-formed token belonging to another profile, and
/// the current token bare, quoted and weak — the three ways HTTP and CloudFront's own `ETag`
/// header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    let other = Profile::new(
        "another profile, whose token is not the subject's",
        "fle-profile-version-gate-other",
        None,
        Vec::new(),
    );
    let (_, other_etag) = create(&router, &other).await;

    let subject = Profile::new(
        "the profile under the version gate",
        "fle-profile-version-gate",
        None,
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );

    // Each refusal is asserted on its own freshly created profile, and the profile is read back
    // after it: a refused delete removes nothing.
    let refusals: &[(&str, Option<&str>, u16, &str)] = &[
        ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
        ),
        ("an empty If-Match", Some(""), 400, "InvalidIfMatchVersion"),
        (
            "an If-Match carrying a lowercase token",
            Some("e1stale1version"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a version token that is another profile's",
            Some(other_etag.as_str()),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, code) in refusals {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_profile(&router, &id, *if_match).await;
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
        let fetched = get_profile(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the refused delete to leave the profile readable, got {} \
             with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case:?}: expected the refused delete to leave the version token alone, got {:?}",
            fetched.header("etag")
        );

        // Clear the way for the next row, which creates a profile under the same caller
        // reference.
        let cleanup = delete_profile(&router, &id, Some(&etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: expected the current token to delete the profile, got {} with {}",
            cleanup.status, cleanup.raw
        );
    }

    // The current token, spelled each of the three admissible ways, deletes the profile.
    let spellings: &[(&str, Spelling)] = &[
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (case, spell) in spellings {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_profile(&router, &id, Some(&spell(&etag))).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// How one row below builds the configuration that references the subject profile: from the
/// subject's id and the id of another profile a second mapping of the same configuration names.
type BuildConfig = fn(&str, &str) -> Config;

/// A profile a stored field-level encryption configuration names cannot be deleted, and once
/// every configuration has let go the same delete lands — the stored `ProfileId` is the gate.
///
/// The rows cover which mapping kind names the profile: a query argument-profile mapping, a
/// content type-profile mapping, and a mapping of each section at once. A second profile no
/// configuration names is deleted in each case, so the gate is shown to select on the reference
/// rather than on the store holding a configuration at all — as is a configuration whose content
/// type mapping names no profile, which leaves every profile deletable.
#[tokio::test]
async fn a_profile_a_configuration_names_cannot_be_deleted() {
    let references: &[(&str, BuildConfig)] = &[
        (
            "a query argument-profile mapping names the profile",
            |profile, _| {
                query_arg_config(
                    "a query argument-profile mapping names the profile",
                    "fle-profile-in-use-query-arg",
                    profile,
                )
            },
        ),
        (
            "a content type-profile mapping names the profile",
            |profile, _| {
                content_type_config(
                    "a content type-profile mapping names the profile",
                    "fle-profile-in-use-content-type",
                    Some(profile),
                )
            },
        ),
        (
            "a mapping of each section names the profile",
            |profile, other| {
                both_sections_config(
                    "a mapping of each section names the profile",
                    "fle-profile-in-use-both",
                    profile,
                    other,
                )
            },
        ),
    ];

    for (case, build) in references {
        let router = emulator();
        let keys = upload_keys(&router).await;

        let subject = Profile::new(
            "the profile under the reference gate",
            "fle-profile-in-use",
            None,
            vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
        );
        let (profile_id, profile_etag) = create(&router, &subject).await;

        // A second profile nothing names: the configuration below leaves it alone, so it is
        // deletable throughout.
        let spare = Profile::new(
            "a profile no configuration names",
            "fle-profile-not-in-use",
            None,
            Vec::new(),
        );
        let (spare_id, spare_etag) = create(&router, &spare).await;

        // A third profile, for the mapping a row points at something OTHER than the subject: the
        // spare above stays unreferenced throughout, which is what makes its own delete land.
        let other = Profile::new(
            "a profile another mapping of the same configuration names",
            "fle-profile-other-reference",
            None,
            Vec::new(),
        );
        let (other_id, _) = create(&router, &other).await;

        let config = build(&profile_id, &other_id);
        let (config_id, config_etag) = store_config(&router, &config).await;

        let refused = delete_profile(&router, &profile_id, Some(&profile_etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 while a configuration names the profile, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FieldLevelEncryptionProfileInUse",
            "case {case:?}: expected FieldLevelEncryptionProfileInUse while in use, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            PROFILE_IN_USE_MESSAGE,
            "case {case:?}: expected the modelled in-use sentence, got {}",
            refused.raw
        );
        assert_eq!(
            get_profile(&router, &profile_id).await.status,
            200,
            "case {case:?}: expected the refused delete to leave the profile readable"
        );

        // A version that is not the profile's is refused on its own terms, before anything about
        // the reference.
        for (description, if_match, status, code) in [
            ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
            (
                "a version token that is the configuration's",
                Some(config_etag.as_str()),
                412,
                "PreconditionFailed",
            ),
        ] {
            let answer = delete_profile(&router, &profile_id, if_match).await;
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

        // A configuration whose content type mapping names NO profile references nothing
        // through that mapping, so it leaves the spare profile deletable while it stands.
        // Its query argument mapping names the subject the gate above already holds, since a
        // configuration naming no profile in either section is not one the service stores.
        let nameless = Config::new(
            "a configuration whose content type mapping names no profile",
            "fle-profile-in-use-nameless",
            None,
            Some(Section::new(
                false,
                Some(vec![QueryArgMapping::new("fle-profile", &profile_id)]),
            )),
            Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new(
                    "application/x-www-form-urlencoded",
                    None,
                )]),
            )),
        );
        let (nameless_id, nameless_etag) = store_config(&router, &nameless).await;
        let spare_deleted = delete_profile(&router, &spare_id, Some(&spare_etag)).await;
        assert_eq!(
            spare_deleted.status, 204,
            "case {case:?}: expected HTTP 204 deleting the profile no configuration names, got {} \
             with {}",
            spare_deleted.status, spare_deleted.raw
        );
        assert_gone(&router, &spare_id, "a profile no configuration names").await;
        let nameless_removed = delete_config(&router, &nameless_id, Some(&nameless_etag)).await;
        assert_eq!(
            nameless_removed.status, 204,
            "case {case:?}: expected HTTP 204 deleting the configuration that names no profile, \
             got {} with {}",
            nameless_removed.status, nameless_removed.raw
        );

        // Take the configuration that names the profile out of the store, and the same delete
        // lands.
        let config_removed = delete_config(&router, &config_id, Some(&config_etag)).await;
        assert_eq!(
            config_removed.status, 204,
            "case {case:?}: expected HTTP 204 deleting the configuration that names the profile, \
             got {} with {}",
            config_removed.status, config_removed.raw
        );
        assert_eq!(
            get_config(&router, &config_id).await.status,
            404,
            "case {case:?}: expected the configuration to be gone before the profile's delete"
        );

        let deleted = delete_profile(&router, &profile_id, Some(&profile_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no configuration names the profile, got {} \
             with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &profile_id, case).await;
    }
}

/// An id the store holds no field-level encryption profile for is
/// `NoSuchFieldLevelEncryptionProfile`, whatever shape the id takes — never a 204, which would
/// claim something was removed. The MESSAGE says which condition the delete found: the
/// identifier's shape is judged before the store is consulted, so a value outside the shape a
/// profile id takes is the identifier CloudFront cannot read, while a well-formed profile id
/// the store holds nothing for is the absent profile.
#[tokio::test]
async fn an_id_no_profile_holds_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let row = Profile::new(
        "the only profile the account holds",
        "fle-profile-the-only-one",
        None,
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    let (known, etag) = create(&router, &row).await;

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
            "a known id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_PROFILE_MESSAGE,
        ),
        (
            "a known id in lower case",
            known.to_lowercase(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a token that is no profile id",
            "not-a-profile-id".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "the caller reference rather than the id",
            "fle-profile-the-only-one".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
    ];

    for (case, candidate, expected) in absent {
        let answer = delete_profile(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionProfile",
            "case {case:?}: expected NoSuchFieldLevelEncryptionProfile for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // None of those refusals touched the profile the account really holds.
    let fetched = get_profile(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the only profile the account holds: expected it to survive every refusal, got {} \
         with {}",
        fetched.status, fetched.raw
    );
}
