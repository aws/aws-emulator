// SPDX-License-Identifier: CC0-1.0

//! A field-level encryption configuration's REQUIRED forwarding flags, as the service reports
//! them for the request that submitted them — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! RULE: a configuration member the request STATES is persisted and projected verbatim; only a
//! member the request leaves out carries a value the service supplies. `QueryArgProfileConfig$
//! ForwardWhenQueryArgProfileIsUnknown` and `ContentTypeProfileConfig$
//! ForwardWhenContentTypeIsUnknown` are REQUIRED booleans the model gives no default and no
//! server-side override
//! (`smithy_query(query="shape",shape="FieldLevelEncryptionConfig,QueryArgProfileConfig,ContentTypeProfileConfig")`),
//! so a configuration whose request submits a stated section's flag is created, stored and read
//! back with exactly that setting, whatever the mappings beside it map — and a later update
//! that submits the other setting moves the stored configuration to that one.
//!
//! The rows differ along what the rule generalises over: the setting each required flag is
//! submitted with (both), how many query arguments the stated section maps (none, one, two,
//! three), whether that section states its `QueryArgProfiles` wrapper at all, whether a content
//! type section is stated beside it, and the comment carried (absent, empty, text).
//!
//! The second test is the refusal the same create path owes a mapping that names a profile this
//! account does not hold (`NoSuchFieldLevelEncryptionProfile`, 404), which also shows a refused
//! create stores nothing: the caller reference it named is still free afterwards.
//!
//! The harness (the router, the request helpers, the configuration writer and the profile every
//! mapping points at) is the configuration suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer, Node};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// `NoSuchFieldLevelEncryptionProfile`'s sentence as CloudFront sends it, spelled out here
/// rather than taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_PROFILE_MESSAGE: &str = "The specified field level encryption profile does not exist.";

/// A profile id written the way this service mints one (`P` and 13 characters of the id
/// alphabet) that no create of this suite stores, so it names no profile at all.
const ABSENT_PROFILE_ID: &str = "PZZZZZZZZZZZZZ";

/// The configurations the rule is asserted over, each mapping pointing at the stored profile
/// `profile`.
///
/// Every row names a profile in one of its two sections, since a configuration naming none in
/// either is refused (`QueryArgProfileEmpty`, 400).
fn rows(profile: &str) -> Vec<Config> {
    let mapped = |query_arg: &str| QueryArgMapping::new(query_arg, profile);
    let named_content_type = |content_type: &str| {
        Some(vec![ContentTypeMapping::new(content_type, Some(profile))])
    };
    vec![
        Config::new(
            "one query argument mapping, the flag submitted off",
            "fle-flag-one-off",
            Some("the configuration an emulator test submits"),
            Some(Section::new(false, Some(vec![mapped("fle-profile")]))),
            None,
        ),
        Config::new(
            "one query argument mapping, the flag submitted on",
            "fle-flag-one-on",
            Some("the configuration an emulator test submits"),
            Some(Section::new(true, Some(vec![mapped("fle-profile")]))),
            None,
        ),
        Config::new(
            "two query argument mappings, the flag submitted off beside a content type section \
             whose own flag is on",
            "fle-flag-two-off",
            None,
            Some(Section::new(
                false,
                Some(vec![mapped("fle-profile"), mapped("alt-profile")]),
            )),
            Some(Section::new(true, named_content_type("text/html"))),
        ),
        Config::new(
            "three query argument mappings, the flag submitted on beside a content type section \
             whose own flag is off",
            "fle-flag-three-on",
            Some(""),
            Some(Section::new(
                true,
                Some(vec![
                    mapped("fle-profile"),
                    mapped("alt-profile"),
                    mapped("profile&arg"),
                ]),
            )),
            Some(Section::new(false, named_content_type("application/json"))),
        ),
        Config::new(
            "a query argument wrapper that maps nothing, the flag submitted off",
            "fle-flag-empty-off",
            None,
            Some(Section::new(false, Some(Vec::new()))),
            Some(Section::new(true, named_content_type("text/plain"))),
        ),
        Config::new(
            "a query argument wrapper that maps nothing, the flag submitted on",
            "fle-flag-empty-on",
            Some("nothing mapped by query argument"),
            Some(Section::new(true, Some(Vec::new()))),
            Some(Section::new(false, named_content_type("text/xml"))),
        ),
        Config::new(
            "a query argument section stating no wrapper at all, the flag submitted off",
            "fle-flag-no-wrapper-off",
            None,
            Some(Section::new(false, None)),
            Some(Section::new(
                true,
                named_content_type("application/x-www-form-urlencoded"),
            )),
        ),
        Config::new(
            "a query argument section stating no wrapper at all, the flag submitted on",
            "fle-flag-no-wrapper-on",
            Some("no wrapper"),
            Some(Section::new(true, None)),
            Some(Section::new(false, named_content_type("text/csv"))),
        ),
    ]
}

/// The flag `config` — one serialised `FieldLevelEncryptionConfig` — reports for the member
/// `member` of the section `section`.
fn flag_of(config: &Node, section: &str, member: &str) -> Option<String> {
    config.text_at(&format!("{section}/{member}")).map(str::to_string)
}

/// Assert that the configuration document inside `answer` reports both required flags at the
/// settings `row` submitted, and that the stated query argument mappings come back as they
/// were submitted.
fn assert_submitted_flags(answer: &Answer, row: &Config, expected_query_arg: bool, context: &str) {
    let case = row.case.as_str();
    let config = answer
        .root()
        .descendant("FieldLevelEncryptionConfig")
        .unwrap_or_else(|| {
            panic!("case {case:?} ({context}): expected a configuration in {}", answer.raw)
        });

    let actual = flag_of(
        config,
        "QueryArgProfileConfig",
        "ForwardWhenQueryArgProfileIsUnknown",
    );
    assert_eq!(
        actual.as_deref(),
        Some(expected_query_arg.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenQueryArgProfileIsUnknown \
         {expected_query_arg}, got {actual:?} in {}",
        answer.raw
    );

    if let Some(section) = &row.content_type {
        let actual = flag_of(
            config,
            "ContentTypeProfileConfig",
            "ForwardWhenContentTypeIsUnknown",
        );
        assert_eq!(
            actual.as_deref(),
            Some(section.forward_when_unknown.to_string().as_str()),
            "case {case:?} ({context}): expected ForwardWhenContentTypeIsUnknown {}, got \
             {actual:?} in {}",
            section.forward_when_unknown,
            answer.raw
        );
    }

    // The flag travels beside the mappings the request submitted, so both are read from the
    // same document: a configuration reported with the wrong flag beside the right mappings
    // would otherwise pass.
    let Some(mappings) = row
        .query_arg
        .as_ref()
        .and_then(|section| section.mappings.as_ref())
    else {
        return;
    };
    let listed: Vec<&Node> = config
        .at("QueryArgProfileConfig/QueryArgProfiles/Items")
        .map(|items| items.children_named("QueryArgProfile"))
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        mappings.len(),
        "case {case:?} ({context}): expected {} QueryArgProfile entries, got {} in {}",
        mappings.len(),
        listed.len(),
        answer.raw
    );
    let quantity = config.text_at("QueryArgProfileConfig/QueryArgProfiles/Quantity");
    assert_eq!(
        quantity,
        Some(mappings.len().to_string().as_str()),
        "case {case:?} ({context}): expected QueryArgProfiles Quantity {}, got {quantity:?} in {}",
        mappings.len(),
        answer.raw
    );
    for (expected, element) in mappings.iter().zip(listed) {
        let named = element.text_at("ProfileId").map(str::to_string);
        assert_eq!(
            named.as_deref(),
            Some(expected.profile_id.as_str()),
            "case {case:?} ({context}): expected QueryArgProfile ProfileId {:?}, got {named:?} \
             in {}",
            expected.profile_id,
            answer.raw
        );
        let query_arg = element.text_at("QueryArg").map(str::to_string);
        assert_eq!(
            query_arg.as_deref(),
            Some(expected.query_arg.as_str()),
            "case {case:?} ({context}): expected QueryArgProfile QueryArg {:?}, got \
             {query_arg:?} in {}",
            expected.query_arg,
            answer.raw
        );
    }
}

/// Assert that the `Comment` `config` reports is the one `row` submitted — the empty text for
/// a row that states none, which is the member the service supplies.
fn assert_comment(answer: &Answer, row: &Config, context: &str) {
    let case = row.case.as_str();
    let expected = row.effective_comment();
    let config = answer
        .root()
        .descendant("FieldLevelEncryptionConfig")
        .unwrap_or_else(|| {
            panic!("case {case:?} ({context}): expected a configuration in {}", answer.raw)
        });
    let actual = config.text_at("Comment").map(str::to_string);
    assert_eq!(
        actual.as_deref(),
        Some(expected.as_str()),
        "case {case:?} ({context}): expected Comment {expected:?}, got {actual:?} in {}",
        answer.raw
    );
}

/// Create `row`, assert the create answered 201 with a minted identity, and report the id and
/// the version token beside the create's own answer.
async fn created(router: &MultiServiceRouter, row: &Config) -> (String, String, Answer) {
    let answer = create_config(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from the create, got {} with {}",
        row.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case {:?}: the create must mint an id, got {}",
        row.case,
        answer.raw
    );
    let etag = answer.header("etag").to_string();
    assert!(
        !etag.is_empty(),
        "case {:?}: the create must hand back a version token, got {}",
        row.case,
        answer.raw
    );
    (id, etag, answer)
}

/// The create ECHOES the configuration as submitted, and every reader of the stored record
/// reports the same settings at the same version.
#[tokio::test]
async fn a_create_echoes_the_submitted_forwarding_flags_and_both_reads_agree() {
    let router = emulator();
    let profile = store_profile(&router, "fle-flag-suite-profile").await;

    for row in rows(&profile) {
        let submitted = row
            .query_arg
            .as_ref()
            .map(|section| section.forward_when_unknown)
            .expect("every row of this table states a query argument section");
        let (id, etag, create) = created(&router, &row).await;
        assert_submitted_flags(&create, &row, submitted, "CreateFieldLevelEncryptionConfig");
        assert_comment(&create, &row, "CreateFieldLevelEncryptionConfig");

        let read = get_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryption, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_submitted_flags(&read, &row, submitted, "GetFieldLevelEncryption");
        assert_comment(&read, &row, "GetFieldLevelEncryption");
        assert_eq!(
            read.header("etag"),
            etag,
            "case {:?}: the read answers at the version the create minted, got {}",
            row.case,
            read.raw
        );

        let document = get_config_document(&router, &id).await;
        assert_eq!(
            document.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryptionConfig, got {} with {}",
            row.case, document.status, document.raw
        );
        assert_submitted_flags(
            &document,
            &row,
            submitted,
            "GetFieldLevelEncryptionConfig",
        );
        assert_comment(&document, &row, "GetFieldLevelEncryptionConfig");
        assert_eq!(
            document.header("etag"),
            etag,
            "case {:?}: the configuration read answers at the same version, got {}",
            row.case,
            document.raw
        );
    }
}

/// The flag TRACKS the request: an update that submits the other setting of the same section
/// moves the stored configuration to it, and the reads report the new setting.
#[tokio::test]
async fn an_update_that_submits_the_other_setting_stores_that_setting() {
    let router = emulator();
    let profile = store_profile(&router, "fle-flag-update-profile").await;

    for row in rows(&profile) {
        let submitted = row
            .query_arg
            .as_ref()
            .map(|section| section.forward_when_unknown)
            .expect("every row of this table states a query argument section");
        let (id, etag, _) = created(&router, &row).await;

        let toggled = Config {
            query_arg: row.query_arg.clone().map(|mut section| {
                section.forward_when_unknown = !submitted;
                section
            }),
            ..row.clone()
        };
        let updated = update_config(&router, &id, &toggled.xml(), Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from the update, got {} with {}",
            row.case, updated.status, updated.raw
        );
        assert_submitted_flags(
            &updated,
            &toggled,
            !submitted,
            "UpdateFieldLevelEncryptionConfig",
        );

        let document = get_config_document(&router, &id).await;
        assert_submitted_flags(
            &document,
            &toggled,
            !submitted,
            "GetFieldLevelEncryptionConfig after the update",
        );
    }
}

/// A query argument mapping names the PROFILE its matching fields are encrypted under, so one
/// naming an id this account holds no profile for is refused — and the refusal stores nothing:
/// the caller reference it named is free for a configuration that names a stored profile.
#[tokio::test]
async fn a_mapping_naming_no_stored_profile_is_refused_and_stores_nothing() {
    let router = emulator();
    let profile = store_profile(&router, "fle-flag-refusal-profile").await;

    let reference = "fle-flag-absent-profile";
    let absent = Config::new(
        "a query argument mapping naming a profile this account does not hold",
        reference,
        Some("the profile is gone"),
        Some(Section::new(
            false,
            Some(vec![QueryArgMapping::new("fle-profile", ABSENT_PROFILE_ID)]),
        )),
        None,
    );
    let refused = create_config(&router, &absent.xml()).await;
    assert_eq!(
        refused.status, 404,
        "case {:?}: expected HTTP 404, got {} with {}",
        absent.case, refused.status, refused.raw
    );
    let code = refused.root().find_text("Code").map(str::to_string);
    assert_eq!(
        code.as_deref(),
        Some("NoSuchFieldLevelEncryptionProfile"),
        "case {:?}: expected NoSuchFieldLevelEncryptionProfile, got {code:?} in {}",
        absent.case,
        refused.raw
    );
    let message = refused.root().find_text("Message").map(str::to_string);
    assert_eq!(
        message.as_deref(),
        Some(NO_SUCH_PROFILE_MESSAGE),
        "case {:?}: expected the refusal's own sentence, got {message:?} in {}",
        absent.case,
        refused.raw
    );

    // Nothing was stored under that reference: the same reference now creates a configuration
    // whose mapping names the stored profile, and its flag comes back as submitted.
    let allowed = Config::new(
        "the same reference, naming the stored profile",
        reference,
        Some("the profile is gone"),
        Some(Section::new(
            false,
            Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
        )),
        None,
    );
    let (_id, _etag, create) = created(&router, &allowed).await;
    assert_submitted_flags(&create, &allowed, false, "CreateFieldLevelEncryptionConfig");
}
