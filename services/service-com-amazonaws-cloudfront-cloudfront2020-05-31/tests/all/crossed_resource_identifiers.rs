// SPDX-License-Identifier: CC0-1.0

//! An identifier of ONE resource kind submitted where another kind's belongs, end to end over
//! HTTP.
//!
//! The rule under test: an operation addressed by a resource identifier answers its own
//! not-found code for every `Id` it cannot resolve — these routes model no argument error
//! (`smithy_query(query="operation",operation="GetOriginAccessControl,GetFieldLevelEncryptionConfig,GetFieldLevelEncryptionProfile")`
//! reports each one's not-found error and `AccessDenied` and nothing else, over an `Id` that is
//! a plain `string` `@httpLabel` with no `@pattern` and no `@length`) — so the MESSAGE is what
//! tells two different conditions apart, and what decides it is whether the value COULD have
//! been an identifier of the addressed kind at all:
//!
//! * a value that could is carried through to the STORE, which is what records which
//!   identifier was ever issued, and the resource is reported ABSENT;
//! * a value that could NOT was issued for something else, or was never an identifier
//!   CloudFront issued, and is the identifier the route reports it cannot READ ("Id format is
//!   invalid").
//!
//! How much of the value answers that question is each resource kind's own recorded contract,
//! and the two families here are the two answers:
//!
//! * an origin access control's identifier comes from the namespace its kind SHARES with the
//!   distributions and the origin access identities, so neither the letter a value opens with
//!   nor the width of its body says which of them issued it: those routes read the ALPHABET and
//!   nothing more, and report every identifier of it they cannot resolve as the control that is
//!   ABSENT;
//! * a field-level encryption configuration's and a profile's identifiers come from namespaces
//!   of their OWN (`C` and `P`), so those routes read the WIDTH and the NAMESPACE LETTER too: a
//!   value WIDER than the width this service issues an identifier at could be none of these
//!   kinds', and a value written exactly the way an identifier is ISSUED — that alphabet at that
//!   width — under another kind's letter IS that kind's identifier, so no record of the
//!   addressed kind could answer to either and both are unreadable, while a value of the
//!   alphabet NARROWER than that width, or at that width under a letter no kind of this service
//!   issues one under, names no kind of
//!   its own and is carried through to the store — as is a value written in the OTHER form
//!   CloudFront issues an identifier in, the UUID its key groups and policy kinds are minted
//!   with, which carries no namespace marker at all. A value outside both forms is no identifier
//!   of the mint at all, so it too is unreadable.
//!
//! The rows are the CROSSOVER: four resource kinds are created through their own APIs, and
//! each one's really-minted identifier is then submitted to every other kind's route, beside
//! made-up values of the identifier alphabet and values written outside it. Every live resource
//! is read back at its own identifier afterwards, so no refusal here touched the store.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, is_minted_uuid, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{create_config, get_config, simple};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, upload_key, Entity, Profile,
};
use crate::origin_access_control_lifecycle::{create_control, get_control, table};

/// The sentences these routes answer on the wire, spelled out here rather than taken from the
/// crate, so a reworded refusal fails this suite.
const NO_SUCH_CONTROL: &str = "The specified origin access control does not exist.";
const NO_SUCH_CONFIG: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// One route addressed by a resource identifier, named for the resource it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    OriginAccessControl,
    OriginAccessControlConfig,
    FieldLevelEncryptionConfig,
    FieldLevelEncryptionProfile,
}

/// Every route this suite drives: both of the origin access control's readers, and one reader
/// of each field-level encryption kind.
const ROUTES: [Route; 4] = [
    Route::OriginAccessControl,
    Route::OriginAccessControlConfig,
    Route::FieldLevelEncryptionConfig,
    Route::FieldLevelEncryptionProfile,
];

impl Route {
    /// The modelled error code this route answers an identifier it cannot resolve.
    fn code(self) -> &'static str {
        match self {
            Self::OriginAccessControl | Self::OriginAccessControlConfig => {
                "NoSuchOriginAccessControl"
            }
            Self::FieldLevelEncryptionConfig => "NoSuchFieldLevelEncryptionConfig",
            Self::FieldLevelEncryptionProfile => "NoSuchFieldLevelEncryptionProfile",
        }
    }

    /// The sentence this route answers a value it could have issued and holds nothing under.
    fn absent(self) -> &'static str {
        match self {
            Self::OriginAccessControl | Self::OriginAccessControlConfig => NO_SUCH_CONTROL,
            Self::FieldLevelEncryptionConfig => NO_SUCH_CONFIG,
            Self::FieldLevelEncryptionProfile => NO_SUCH_PROFILE,
        }
    }

    /// Send this route at `id`.
    async fn send(self, router: &MultiServiceRouter, id: &str) -> Answer {
        match self {
            Self::OriginAccessControl => get_control(router, id).await,
            Self::OriginAccessControlConfig => {
                call(
                    router,
                    "GET",
                    &format!("/{API_VERSION}/origin-access-control/{id}/config"),
                    None,
                    None,
                )
                .await
            }
            Self::FieldLevelEncryptionConfig => {
                call(
                    router,
                    "GET",
                    &format!("/{API_VERSION}/field-level-encryption/{id}/config"),
                    None,
                    None,
                )
                .await
            }
            Self::FieldLevelEncryptionProfile => get_profile(router, id).await,
        }
    }

    /// The namespace letter the addressed kind's identifiers are ISSUED under, where the kind
    /// has one of its own — asserted against the identifiers the creates in [`live`] really
    /// answered with. An origin access control has none of its own: its kind shares [`
    /// ISSUED_NAMESPACES`]'s `E` with the distributions and the origin access identities, so the
    /// letter says nothing about which of them issued the identifier, and its routes read the
    /// ALPHABET alone.
    fn own_namespace(self) -> Option<char> {
        match self {
            Self::OriginAccessControl | Self::OriginAccessControlConfig => None,
            Self::FieldLevelEncryptionConfig => Some('C'),
            Self::FieldLevelEncryptionProfile => Some('P'),
        }
    }

    /// The sentence this route owes `id` when nothing of its kind answers to it.
    ///
    /// How much of the value a route reads is the resource kind's own recorded contract, and
    /// every one of them reads the ALPHABET: a value written outside the characters CloudFront
    /// writes an identifier in is no identifier of this mint at all, so no record of any kind
    /// could answer to it and it is unreadable everywhere. Past that, the origin access
    /// control's routes read NOTHING MORE of it, because its identifiers come from the namespace
    /// its kind SHARES with the distributions: every value of the alphabet is carried through to
    /// the store, so the record it holds none under is reported ABSENT. The field-level
    /// encryption routes read the WIDTH and the NAMESPACE LETTER too: a value WIDER than an
    /// identifier of this mint is issued could be none of them, and a value written exactly the
    /// way an identifier is ISSUED under another kind's letter IS that kind's identifier, so both
    /// are unreadable, while a value of the alphabet narrower than the issued width, or at that
    /// width under a letter no kind of this service issues one under, is carried through to the
    /// store.
    fn expected(self, id: &str) -> &'static str {
        // The OTHER form CloudFront issues an identifier in: a UUID carries no namespace marker
        // at all and every kind identified by one is minted in it, so a route that reads the
        // NAMESPACE has nothing in it to contradict the addressed kind and the store answers for
        // it, while a route whose contract reads the ALPHABET alone refuses every value outside
        // that alphabet.
        if is_minted_uuid(id) {
            return match self.own_namespace() {
                Some(_) => self.absent(),
                None => UNREADABLE_IDENTIFIER,
            };
        }
        if !in_the_identifier_alphabet(id) {
            return UNREADABLE_IDENTIFIER;
        }
        let unreadable = self.own_namespace().is_some_and(|own| {
            id.len() > MINTED_ID_WIDTH
                || (id.len() == MINTED_ID_WIDTH
                    && id
                        .chars()
                        .next()
                        .is_some_and(|letter| letter != own && ISSUED_NAMESPACES.contains(&letter)))
        });
        if unreadable {
            UNREADABLE_IDENTIFIER
        } else {
            self.absent()
        }
    }
}

/// The namespace letters this service ISSUES an identifier under, each the letter of exactly
/// one resource kind: `E` for the distribution family (a distribution, a streaming
/// distribution, an origin access identity, an origin access control), `K` for a public key,
/// `P` for a field-level encryption profile and `C` for a field-level encryption configuration
/// — read off the identifiers the creates below really answered with, which this suite asserts.
const ISSUED_NAMESPACES: [char; 4] = ['E', 'K', 'P', 'C'];

/// The width CloudFront writes an entity identifier at: a namespace letter over a body of 13
/// characters, which is what every create in this suite answers with (asserted in [`live`]).
const MINTED_ID_WIDTH: usize = 14;

/// Whether `value` is written in the characters CloudFront writes an identifier in.
fn in_the_identifier_alphabet(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// The identifiers this emulator really minted for four kinds, each through its own API.
struct Live {
    control: String,
    distribution: String,
    config: String,
    profile: String,
}

/// Create one origin access control, one distribution, one field-level encryption profile and
/// one configuration naming that profile, answering the identifier each create handed back.
async fn live(router: &MultiServiceRouter) -> Live {
    let control_document = table()[0].with_name("the control these rows cross", "crossed-oac");
    let created_control = create_control(router, &control_document.xml()).await;
    assert_eq!(
        created_control.status, 201,
        "case creating the control: expected HTTP 201, got {} with {}",
        created_control.status, created_control.raw
    );

    let created_distribution = create_distribution(
        router,
        &config("the distribution these rows cross", "crossed-distribution").xml(),
    )
    .await;
    assert_eq!(
        created_distribution.status, 201,
        "case creating the distribution: expected HTTP 201, got {} with {}",
        created_distribution.status, created_distribution.raw
    );

    let key = upload_key(router, 0).await;
    let created_profile = create_profile(
        router,
        &Profile::new(
            "the profile these rows cross",
            "crossed-profile",
            None,
            vec![Entity::new(&key, "provider-one", &["CreditCard"])],
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_profile.status, 201,
        "case creating the profile: expected HTTP 201, got {} with {}",
        created_profile.status, created_profile.raw
    );
    let profile = created_profile
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let created_config = create_config(
        router,
        &simple("the configuration these rows cross", "crossed-config", &profile).xml(),
    )
    .await;
    assert_eq!(
        created_config.status, 201,
        "case creating the configuration: expected HTTP 201, got {} with {}",
        created_config.status, created_config.raw
    );

    let live = Live {
        control: created_control
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        distribution: created_distribution
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        config: created_config
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        profile,
    };

    // Every identifier this suite reasons about is one the service really issued, in the
    // namespace and at the width this suite's table reads values against.
    for (case, id) in [
        ("the origin access control", &live.control),
        ("the distribution", &live.distribution),
        ("the configuration", &live.config),
        ("the profile", &live.profile),
    ] {
        assert_eq!(
            id.len(),
            MINTED_ID_WIDTH,
            "case {case}: expected a minted identifier of {MINTED_ID_WIDTH} characters, got \
             {id:?}"
        );
        assert!(
            id.chars()
                .next()
                .is_some_and(|letter| ISSUED_NAMESPACES.contains(&letter)),
            "case {case}: expected a minted identifier under a namespace letter this service \
             issues one under, got {id:?}"
        );
    }

    // And the two kinds whose identifiers come from a namespace of their OWN really carry the
    // letter the table reads their routes against ([`Route::own_namespace`]).
    for (route, id) in [
        (Route::FieldLevelEncryptionConfig, &live.config),
        (Route::FieldLevelEncryptionProfile, &live.profile),
    ] {
        let own = route.own_namespace().expect("this kind has its own namespace");
        assert!(
            id.starts_with(own),
            "case {route:?}: expected the minted identifier to open with {own:?}, got {id:?}"
        );
    }
    live
}

/// Assert one refusal: its status, its code and — the point of this suite — its message.
fn assert_refusal(answer: &Answer, case: &str, code: &str, message: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case}: expected code {code}, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case}: expected the sentence {message:?}, got {actual:?}"
    );
}

/// RULE: every route reports an identifier it cannot resolve under its own not-found code, and
/// the sentence follows whether the value could have been an identifier of the addressed kind —
/// over the identifiers of FOUR kinds this account really issued, made-up values of the
/// identifier alphabet, and values written outside it.
#[tokio::test]
async fn an_identifier_of_another_kind_earns_the_sentence_its_route_can_read() {
    let router = emulator();
    let live = live(&router).await;

    // (case, the value submitted). The rows span: each other kind's really-minted identifier,
    // the addressed kind's own namespace at widths CloudFront writes no identifier at, a
    // namespace letter no kind of this service issues one under, a body of digits alone, and
    // five values written outside the identifier alphabet.
    let rows: Vec<(String, String)> = vec![
        (
            "a live origin access control's identifier".to_string(),
            live.control.clone(),
        ),
        (
            "a live distribution's identifier".to_string(),
            live.distribution.clone(),
        ),
        (
            "a live field-level encryption configuration's identifier".to_string(),
            live.config.clone(),
        ),
        (
            "a live field-level encryption profile's identifier".to_string(),
            live.profile.clone(),
        ),
        (
            "a configuration's namespace over a body one character short".to_string(),
            "C012345678901".to_string(),
        ),
        (
            "a profile's namespace over a body one character too long".to_string(),
            "P01234567890123".to_string(),
        ),
        (
            "a namespace letter no kind of this service issues one under".to_string(),
            "W0123456789012".to_string(),
        ),
        ("a body of digits alone".to_string(), "01234567890123".to_string()),
        (
            "a minted identifier in lower case".to_string(),
            live.control.to_lowercase(),
        ),
        (
            "a UUID, the namespace the policy kinds are keyed by".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a bare word".to_string(), "not-an-identifier".to_string()),
        (
            "a caller reference rather than an identifier".to_string(),
            "crossed-oac".to_string(),
        ),
        (
            "a distribution domain name rather than an identifier".to_string(),
            "d111111abcdef8.cloudfront.net".to_string(),
        ),
    ];

    for route in ROUTES {
        for (case, id) in &rows {
            // A row that names the route's OWN live resource is not a crossover: the route
            // serves it, which the control below reads back.
            let serves_it = match route {
                Route::OriginAccessControl | Route::OriginAccessControlConfig => {
                    id == &live.control
                }
                Route::FieldLevelEncryptionConfig => id == &live.config,
                Route::FieldLevelEncryptionProfile => id == &live.profile,
            };
            if serves_it {
                continue;
            }
            let answer = route.send(&router, id).await;
            assert_refusal(
                &answer,
                &format!("{route:?} with {case} ({id:?})"),
                route.code(),
                route.expected(id),
            );
        }
    }

    // None of those refusals touched the store: every live resource is still served at its own
    // identifier.
    for (case, answer) in [
        (
            "the origin access control",
            get_control(&router, &live.control).await,
        ),
        (
            "the field-level encryption configuration",
            get_config(&router, &live.config).await,
        ),
        (
            "the field-level encryption profile",
            get_profile(&router, &live.profile).await,
        ),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {case}: expected it to survive every refusal, got {} with {}",
            answer.status, answer.raw
        );
    }
}
