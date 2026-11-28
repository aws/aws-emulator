// SPDX-License-Identifier: CC0-1.0

//! A field-level encryption identifier in the REQUEST PATH earns the sentence its own
//! FORM decides, end to end over HTTP.
//!
//! The rule under test: an operation ADDRESSED by a field-level encryption identifier answers
//! its one modelled not-found code (404) for every identifier it cannot resolve — these
//! operations model no argument error
//! (`smithy_query(query="operation",operation="GetFieldLevelEncryptionConfig")` reports
//! `errors: [AccessDenied, NoSuchFieldLevelEncryptionConfig]`) — so the MESSAGE is what tells
//! the two conditions apart, and what decides it is whether the value could be an identifier
//! CloudFront ISSUED FOR THE ADDRESSED KIND at all — for the routes that read a form at all. A
//! value written in the identifier ALPHABET (digits and uppercase letters, carrying no separator
//! and no lower case) is carried through to
//! the STORE whatever body and whatever width it carries — WHICH
//! identifier was ever issued is a property of the store, and the width this service issues at
//! is one width CloudFront writes an identifier in rather than the only one — and the record the
//! store holds none under is the ABSENT resource ("The specified field level encryption
//! config/profile does not exist."). A value of that alphabet written exactly the way an
//! identifier is ISSUED — at that width — under the namespace letter of ANOTHER kind this
//! service issues identifiers under IS that kind's identifier, so no record of the addressed
//! kind could answer to it and it earns the unreadable-identifier sentence ("Id format is
//! invalid"); so does a value written OUTSIDE the alphabet, which
//! claims to be no identifier of
//! this mint at all and leaves nothing to resolve.
//! `GetFieldLevelEncryption`, the
//! configuration's RESOURCE read, reads NO form of the value at all — its own recorded contract
//! — so there every value is carried through to the store and the absent resource is the only
//! sentence. Both sentences are fixed text: neither echoes the value submitted.
//!
//! So the rows span what the rule generalises over:
//!
//! * both RESOURCE KINDS of this surface — the configuration (`C`-prefixed) and the profile
//!   (`P`-prefixed) — each with its own sentence;
//! * every OPERATION that addresses one by a path identifier: both readers of each kind, its
//!   update and its delete;
//! * the values of the kind's own minted form the store holds nothing for: one of the
//!   kind's own namespace that was never minted and the minted id with its last character
//!   changed — each the ABSENT resource on every route;
//! * the values of the alphabet at a width OTHER than the width this service issues an
//!   identifier at:
//!   this kind's own
//!   namespace letter over a body one character short of it and one character past it, and an
//!   unminted namespace letter over a far longer body — each carried through to the store, which
//!   reports the record absent, since a value at another width names no kind of its own;
//! * the values of the alphabet at the ISSUED width that were issued for ANOTHER kind: the
//!   sibling kind's own minted id, a public key id and the `E`-prefixed namespace the other
//!   CloudFront entity ids are drawn from — each the identifier reported UNREADABLE;
//! * the value of the alphabet at the issued width under a letter no kind of this service mints
//!   under, which says nothing about the kind that issued it — carried through to the store, as
//!   is a UUID, the OTHER form CloudFront issues an identifier in, which carries no namespace
//!   marker at all;
//! * and the values written outside the alphabet: a minted id in lower case, a body outside the
//!   id alphabet, a bare word and the resource's own caller reference — each the
//!   identifier reported UNREADABLE, since no identifier of this mint is written that way.
//!
//! Each kind's width and alphabet are read off an id this emulator really MINTED rather than
//! written out here, so the table cannot drift from what the service issues.
//!
//! The counterpart case: an identifier borne by a submitted DOCUMENT — a mapping's `ProfileId`
//! on a configuration create — addresses nothing, so it is resolved against the store the same
//! way whatever its shape and reports the absent profile.
//!
//! The wire harness, the configuration and profile builders and their create and delete paths
//! are the crate's own, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::field_level_encryption_config_delete::delete_config;
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, simple,
};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, upload_key, Entity, Profile,
};

/// The sentences CloudFront sends on the wire for these refusals, spelled out here rather than
/// taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONFIG: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";

/// The sentence these operations answer a value written as an identifier of this service's mint
/// that the addressed kind's issued form rules out, naming the member that carried it.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The caller references the two stored resources of each case are held under.
const CONFIG_REFERENCE: &str = "fle-id-format-config";
const PROFILE_REFERENCE: &str = "fle-id-format-profile";

// ---------------------------------------------------------------------------
// The surface this suite drives
// ---------------------------------------------------------------------------

/// Which field-level encryption resource an operation's path identifier names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Config,
    Profile,
}

/// One operation addressed by a field-level encryption identifier in its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    GetFieldLevelEncryption,
    GetFieldLevelEncryptionConfig,
    UpdateFieldLevelEncryptionConfig,
    DeleteFieldLevelEncryptionConfig,
    GetFieldLevelEncryptionProfile,
    GetFieldLevelEncryptionProfileConfig,
    UpdateFieldLevelEncryptionProfile,
    DeleteFieldLevelEncryptionProfile,
}

/// Every operation of this surface that addresses its resource by a path identifier.
const OPS: &[Op] = &[
    Op::GetFieldLevelEncryption,
    Op::GetFieldLevelEncryptionConfig,
    Op::UpdateFieldLevelEncryptionConfig,
    Op::DeleteFieldLevelEncryptionConfig,
    Op::GetFieldLevelEncryptionProfile,
    Op::GetFieldLevelEncryptionProfileConfig,
    Op::UpdateFieldLevelEncryptionProfile,
    Op::DeleteFieldLevelEncryptionProfile,
];

impl Op {
    /// The resource kind this operation's path identifier names.
    fn kind(self) -> Kind {
        match self {
            Self::GetFieldLevelEncryption
            | Self::GetFieldLevelEncryptionConfig
            | Self::UpdateFieldLevelEncryptionConfig
            | Self::DeleteFieldLevelEncryptionConfig => Kind::Config,
            _ => Kind::Profile,
        }
    }

    /// The modelled error code the operation answers an identifier it cannot resolve.
    fn code(self) -> &'static str {
        match self.kind() {
            Kind::Config => "NoSuchFieldLevelEncryptionConfig",
            Kind::Profile => "NoSuchFieldLevelEncryptionProfile",
        }
    }

    /// Whether this operation reads the identifier's FORM at all before it resolves it — its
    /// own recorded contract, read from the one place this surface states it
    /// ([`crate::field_level_encryption_identifier_width_and_namespace::reads_the_form`]), so
    /// the split cannot drift between the suites. Each variant is named for the operation it
    /// drives, which is the name that helper takes.
    fn reads_the_form(self) -> bool {
        crate::field_level_encryption_identifier_width_and_namespace::reads_the_form(&format!(
            "{self:?}"
        ))
    }

    /// Send this operation at `id`, carrying the stored resource's current version token so
    /// every refusal below is about the identifier rather than about a precondition, and a
    /// well-formed document on the updates so none of them is refused for its payload.
    async fn send(self, router: &MultiServiceRouter, id: &str, stored: &Stored) -> Answer {
        match self {
            Self::GetFieldLevelEncryption => get_config(router, id).await,
            Self::GetFieldLevelEncryptionConfig => get_config_document(router, id).await,
            Self::UpdateFieldLevelEncryptionConfig => {
                call(
                    router,
                    "PUT",
                    &format!("/{API_VERSION}/field-level-encryption/{id}/config"),
                    Some(&stored.config_document),
                    Some(&stored.config_etag),
                )
                .await
            }
            Self::DeleteFieldLevelEncryptionConfig => {
                delete_config(router, id, Some(&stored.config_etag)).await
            }
            Self::GetFieldLevelEncryptionProfile => get_profile(router, id).await,
            Self::GetFieldLevelEncryptionProfileConfig => get_profile_config(router, id).await,
            Self::UpdateFieldLevelEncryptionProfile => {
                call(
                    router,
                    "PUT",
                    &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
                    Some(&stored.profile_document),
                    Some(&stored.profile_etag),
                )
                .await
            }
            Self::DeleteFieldLevelEncryptionProfile => {
                call(
                    router,
                    "DELETE",
                    &format!("/{API_VERSION}/field-level-encryption-profile/{id}"),
                    None,
                    Some(&stored.profile_etag),
                )
                .await
            }
        }
    }
}

/// The two resources this emulator really stored for a case, and the documents and version
/// tokens the updates above submit against them.
struct Stored {
    config_id: String,
    config_etag: String,
    config_document: String,
    profile_id: String,
    profile_etag: String,
    profile_document: String,
}

impl Stored {
    /// The id this emulator minted for `kind` — the row every shape below is derived from.
    fn minted(&self, kind: Kind) -> &str {
        match kind {
            Kind::Config => &self.config_id,
            Kind::Profile => &self.profile_id,
        }
    }
}

/// Store one profile (encrypting with a freshly uploaded public key) and one configuration
/// naming it, reporting both resources with the documents and tokens the updates need.
async fn seed(router: &MultiServiceRouter) -> Stored {
    let key = upload_key(router, 0).await;
    let profile_document = Profile::new(
        "the stored profile",
        PROFILE_REFERENCE,
        None,
        vec![Entity::new(&key, "provider-one", &["CreditCard"])],
    )
    .xml();
    let created_profile = create_profile(router, &profile_document).await;
    assert_eq!(
        created_profile.status, 201,
        "case seeding the profile: expected HTTP 201, got {} with {}",
        created_profile.status, created_profile.raw
    );
    let profile_id = created_profile
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let config_document = simple("the stored configuration", CONFIG_REFERENCE, &profile_id).xml();
    let created_config = create_config(router, &config_document).await;
    assert_eq!(
        created_config.status, 201,
        "case seeding the configuration: expected HTTP 201, got {} with {}",
        created_config.status, created_config.raw
    );
    let config_id = created_config
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    Stored {
        config_etag: created_config.header("etag").to_string(),
        config_id,
        config_document,
        profile_etag: created_profile.header("etag").to_string(),
        profile_id,
        profile_document,
    }
}

/// One identifier a case submits where an identifier of the addressed kind belongs, and the
/// sentence its FORM earns it: `expected` from an operation that READS the identifier's form,
/// `absent` from the one that reads none of it.
struct Shape {
    case: String,
    id: String,
    expected: &'static str,
    absent: &'static str,
}

impl Shape {
    /// A value of the form the addressed kind's identifiers are ISSUED in: every route of this
    /// surface resolves it, so the store holding nothing for it is the absent resource.
    fn own(case: impl Into<String>, id: impl Into<String>, absent: &'static str) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: absent,
            absent,
        }
    }

    /// A value written in the identifier alphabet at a width OTHER than the one this service
    /// issues an identifier at, on either side of it: the width this service issues at is one
    /// width CloudFront writes an identifier in
    /// rather than the only one, so such a value names no kind of its own and every route
    /// carries it through to the store.
    fn other_width(case: impl Into<String>, id: impl Into<String>, absent: &'static str) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: absent,
            absent,
        }
    }

    /// A value written in the identifier alphabet at the ISSUED width under the namespace letter
    /// of ANOTHER kind this service issues identifiers under: it IS that kind's identifier, so
    /// no record of the addressed kind could answer to it and every route that reads the form
    /// reports the identifier unreadable.
    fn another_kinds_identifier(
        case: impl Into<String>,
        id: impl Into<String>,
        absent: &'static str,
    ) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: UNREADABLE_IDENTIFIER,
            absent,
        }
    }

    /// A value written in the identifier alphabet, at the issued width, under a letter that
    /// identifies NO namespace of this service: nothing about it says which kind issued it, so
    /// every route carries it through to the store.
    fn no_namespace(case: impl Into<String>, id: impl Into<String>, absent: &'static str) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: absent,
            absent,
        }
    }

    /// A value written in the OTHER form CloudFront issues an identifier in — the UUID its key
    /// groups and policy kinds are minted with. That form carries no namespace marker at all and
    /// is shared by every kind identified by one, so it says which kind issued it no more than a
    /// letter no kind issues under does: every route carries it through to the store.
    fn other_issued_form(
        case: impl Into<String>,
        id: impl Into<String>,
        absent: &'static str,
    ) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: absent,
            absent,
        }
    }

    /// A value written OUTSIDE that alphabet, which claims to be no identifier of this mint at
    /// all: there is nothing in it for the service to resolve, so every route that reads the
    /// form reports the identifier unreadable.
    fn foreign(case: impl Into<String>, id: impl Into<String>, absent: &'static str) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            expected: UNREADABLE_IDENTIFIER,
            absent,
        }
    }

    /// The sentence this identifier earns from `op`: what its form earns it where `op` reads
    /// one, and the absent resource where `op` reads no form at all ([`Op::reads_the_form`]).
    fn expected_from(&self, op: Op) -> &'static str {
        if op.reads_the_form() {
            self.expected
        } else {
            self.absent
        }
    }
}

/// The identifiers a case submits where an identifier of `kind` belongs, derived from the ids
/// this emulator really minted: the body's width and alphabet are read off the minted id, so
/// nothing here is a width written out by hand.
fn shapes(kind: Kind, stored: &Stored, public_key: &str) -> Vec<Shape> {
    let minted = stored.minted(kind);
    let (prefix, body) = minted.split_at(1);
    let sibling = match kind {
        Kind::Config => stored.minted(Kind::Profile),
        Kind::Profile => stored.minted(Kind::Config),
    };
    let reference = match kind {
        Kind::Config => CONFIG_REFERENCE,
        Kind::Profile => PROFILE_REFERENCE,
    };
    let absent = match kind {
        Kind::Config => NO_SUCH_CONFIG,
        Kind::Profile => NO_SUCH_PROFILE,
    };

    let mut changed: Vec<char> = minted.chars().collect();
    if let Some(last) = changed.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }
    let changed: String = changed.into_iter().collect();

    vec![
        // Values of the form this kind's identifiers are issued in, which the store holds
        // nothing for.
        Shape::own(
            "an id of this kind that was never minted",
            format!("{prefix}{}", "0".repeat(body.len())),
            absent,
        ),
        Shape::own(
            "the minted id with its last character changed",
            changed,
            absent,
        ),
        // Values of the identifier alphabet narrower than the width this service issues an
        // identifier at.
        Shape::other_width(
            "this kind's prefix with a body one character short",
            format!("{prefix}{}", &body[..body.len() - 1]),
            absent,
        ),
        Shape::other_width(
            "this kind's prefix with a body one character too long",
            format!("{prefix}{body}A"),
            absent,
        ),
        // Values of the alphabet at the ISSUED width under another kind's namespace letter.
        Shape::another_kinds_identifier(
            "the sibling field-level encryption kind's own minted id",
            sibling,
            absent,
        ),
        Shape::another_kinds_identifier(
            "a public key id this service minted",
            public_key,
            absent,
        ),
        Shape::another_kinds_identifier(
            "the namespace every other CloudFront entity id is drawn from",
            format!("E{body}"),
            absent,
        ),
        Shape::other_width(
            "a namespace letter no kind of this service mints and a far longer body",
            format!("W{body}{body}"),
            absent,
        ),
        // And the value of the alphabet at the issued width under a letter that identifies no
        // namespace of this service, which says nothing about the kind that issued it.
        Shape::no_namespace(
            "a namespace letter no kind of this service mints",
            format!("W{body}"),
            absent,
        ),
        // And every value written outside that alphabet.
        Shape::foreign(
            "the minted id in lower case",
            minted.to_lowercase(),
            absent,
        ),
        Shape::foreign(
            "this kind's prefix with a body outside the id alphabet",
            format!("{prefix}{}", body.to_lowercase()),
            absent,
        ),
        Shape::other_issued_form(
            "a UUID, the namespace the policy kinds are keyed by",
            "00000000-0000-0000-0000-000000000000",
            absent,
        ),
        Shape::foreign("a bare word", "not-an-identifier", absent),
        Shape::foreign(
            "the resource's caller reference rather than its id",
            reference,
            absent,
        ),
    ]
}

/// Assert one refusal: its status, its code and — the point of this suite — its message.
fn assert_refusal(answer: &Answer, case: &str, code: &str, message: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} with {}",
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
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// Every operation addressed by a field-level encryption identifier answers its one not-found
/// code for an identifier it cannot resolve, and the identifier's FORM decides which of that
/// code's two sentences on every route that READS one: the unreadable-identifier sentence for a
/// value that could never be an identifier issued for the addressed kind, and the
/// absent-resource sentence for every value that could be one — while the configuration's
/// RESOURCE read, which reads no form, answers the absent-resource sentence for every value.
#[tokio::test]
async fn a_path_identifier_that_resolves_to_nothing_earns_the_sentence_its_form_decides() {
    let router = emulator();
    let stored = seed(&router).await;
    let public_key = upload_key(&router, 1).await;

    for op in OPS {
        for shape in shapes(op.kind(), &stored, &public_key) {
            let answer = op.send(&router, &shape.id, &stored).await;
            assert_refusal(
                &answer,
                &format!("{op:?} with {}", shape.case),
                op.code(),
                shape.expected_from(*op),
            );
        }
    }

    // Every refusal above left both resources where they were: each is still readable at its
    // own id, so the sentences answered an identifier rather than a store this suite broke.
    let config = get_config(&router, &stored.config_id).await;
    assert_eq!(
        config.status, 200,
        "case the stored configuration: expected it to survive every refusal, got {} with {}",
        config.status, config.raw
    );
    let profile = get_profile(&router, &stored.profile_id).await;
    assert_eq!(
        profile.status, 200,
        "case the stored profile: expected it to survive every refusal, got {} with {}",
        profile.status, profile.raw
    );
}

/// A well-formed identifier of the addressed kind that the store DOES hold is served, so
/// nothing above gates what a caller may legitimately ask for: both readers of each kind
/// answer 200 at the minted id, at the version token its create handed back.
#[tokio::test]
async fn a_minted_identifier_is_still_served_by_every_reader() {
    let router = emulator();
    let stored = seed(&router).await;

    let readers = [
        (
            "GetFieldLevelEncryption",
            get_config(&router, &stored.config_id).await,
            stored.config_etag.clone(),
        ),
        (
            "GetFieldLevelEncryptionConfig",
            get_config_document(&router, &stored.config_id).await,
            stored.config_etag.clone(),
        ),
        (
            "GetFieldLevelEncryptionProfile",
            get_profile(&router, &stored.profile_id).await,
            stored.profile_etag.clone(),
        ),
        (
            "GetFieldLevelEncryptionProfileConfig",
            get_profile_config(&router, &stored.profile_id).await,
            stored.profile_etag.clone(),
        ),
    ];

    for (reader, answer, etag) in readers {
        assert_eq!(
            answer.status, 200,
            "case {reader}: expected HTTP 200 at the minted id, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.header("etag"),
            etag,
            "case {reader}: expected the version token the create handed back, got {:?}",
            answer.header("etag")
        );
    }
}

/// An identifier borne by a submitted DOCUMENT is resolved the same way: a mapping's
/// `ProfileId` on a configuration create reports the ABSENT profile whatever shape that id
/// takes.
#[tokio::test]
async fn a_document_borne_profile_id_reports_the_absent_profile_whatever_its_shape() {
    let router = emulator();
    let stored = seed(&router).await;
    let public_key = upload_key(&router, 1).await;

    for shape in shapes(Kind::Profile, &stored, &public_key) {
        let created = create_config(
            &router,
            &simple(
                "a configuration naming a profile the store does not hold",
                &format!("fle-id-format-document-{}", shape.id.to_lowercase()),
                &shape.id,
            )
            .xml(),
        )
        .await;
        assert_refusal(
            &created,
            &format!("CreateFieldLevelEncryptionConfig with {}", shape.case),
            "NoSuchFieldLevelEncryptionProfile",
            NO_SUCH_PROFILE,
        );
    }

    // And the configuration the account really holds is untouched by those refusals.
    assert_eq!(
        get_config(&router, &stored.config_id).await.status,
        200,
        "case the stored configuration: expected it to survive every refused create"
    );
}
