// SPDX-License-Identifier: CC0-1.0

//! A field-level encryption identifier in the REQUEST PATH is judged by its FORM before the
//! store is consulted, end to end over HTTP.
//!
//! The rule under test: an operation ADDRESSED by a field-level encryption identifier answers
//! its one modelled not-found code (404) for every identifier it cannot resolve, and the
//! MESSAGE says which condition it was — a value outside the shape that resource kind's ids
//! take (the kind's namespace letter followed by a body of the fixed width and alphabet every
//! id this service mints carries) could never have named such a resource, so the identifier
//! itself is what the service reports it cannot read ("Id format is invalid"), while a
//! WELL-FORMED id the store holds nothing for is the absent resource ("The specified field
//! level encryption config/profile does not exist."). Both sentences are fixed text: neither
//! echoes the value submitted.
//!
//! So the rows span what the rule generalises over:
//!
//! * both RESOURCE KINDS of this surface — the configuration (`C`-prefixed) and the profile
//!   (`P`-prefixed) — each with its own namespace letter and its own pair of sentences;
//! * every OPERATION that addresses one by a path identifier: both readers of each kind, its
//!   update and its delete;
//! * the SHAPES an identifier takes outside the addressed kind's: the sibling kind's own
//!   minted id, a public key id, the `E`-prefixed namespace every other CloudFront entity id
//!   is drawn from, the right prefix with a body one character short of and one character past
//!   the width this service mints, a minted id in lower case, a body outside the id alphabet,
//!   a UUID, a bare word and the resource's own caller reference — against the well-formed
//!   rows that carry an id of the addressed kind that was simply never minted.
//!
//! Each kind's width and alphabet are read off an id this emulator really MINTED rather than
//! written out here, so the table cannot drift from what the service issues.
//!
//! The counterpart case: an identifier that addresses NO operation is not judged this way. A
//! profile id borne by a submitted DOCUMENT — a mapping's `ProfileId` on a configuration
//! create — is resolved against the store alone and reports the absent profile whatever its
//! shape.
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
const ID_FORMAT_INVALID: &str = "Id format is invalid";
const NO_SUCH_CONFIG: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";

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

    /// The sentence the operation answers a WELL-FORMED identifier of its kind that the store
    /// holds no resource for.
    fn absent(self) -> &'static str {
        match self.kind() {
            Kind::Config => NO_SUCH_CONFIG,
            Kind::Profile => NO_SUCH_PROFILE,
        }
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

/// One identifier a case submits where an identifier of the addressed kind belongs, and
/// whether the operation reads it at all.
struct Shape {
    case: String,
    id: String,
    /// `true` when the identifier is well-formed for the kind it was submitted to, so the
    /// refusal is the ABSENT resource; `false` when it is outside that shape, so the refusal
    /// is the identifier the service cannot read.
    well_formed: bool,
}

impl Shape {
    fn new(case: impl Into<String>, id: impl Into<String>, well_formed: bool) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            well_formed,
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

    let mut changed: Vec<char> = minted.chars().collect();
    if let Some(last) = changed.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }
    let changed: String = changed.into_iter().collect();

    vec![
        // Well formed for this kind, and the store holds nothing for it.
        Shape::new(
            "an id of this kind that was never minted",
            format!("{prefix}{}", "0".repeat(body.len())),
            true,
        ),
        Shape::new(
            "the minted id with its last character changed",
            changed,
            true,
        ),
        // Outside the shape this kind's ids take.
        Shape::new(
            "the sibling field-level encryption kind's own minted id",
            sibling,
            false,
        ),
        Shape::new(
            "a public key id this service minted",
            public_key,
            false,
        ),
        Shape::new(
            "the namespace every other CloudFront entity id is drawn from",
            format!("E{body}"),
            false,
        ),
        Shape::new(
            "this kind's prefix with a body one character short",
            format!("{prefix}{}", &body[..body.len() - 1]),
            false,
        ),
        Shape::new(
            "this kind's prefix with a body one character too long",
            format!("{prefix}{body}A"),
            false,
        ),
        Shape::new(
            "the minted id in lower case",
            minted.to_lowercase(),
            false,
        ),
        Shape::new(
            "this kind's prefix with a body outside the id alphabet",
            format!("{prefix}{}", body.to_lowercase()),
            false,
        ),
        Shape::new(
            "a UUID, the namespace the policy kinds are keyed by",
            "00000000-0000-0000-0000-000000000000",
            false,
        ),
        Shape::new("a bare word", "not-an-identifier", false),
        Shape::new(
            "the resource's caller reference rather than its id",
            reference,
            false,
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
/// code for an identifier it cannot resolve, and the MESSAGE follows the identifier's SHAPE:
/// the unreadable-identifier sentence for a value outside the shape the addressed kind's ids
/// take, the absent-resource sentence for a well-formed id of that kind the store holds
/// nothing for.
#[tokio::test]
async fn a_path_identifier_outside_its_kinds_shape_is_reported_as_unreadable() {
    let router = emulator();
    let stored = seed(&router).await;
    let public_key = upload_key(&router, 1).await;

    for op in OPS {
        for shape in shapes(op.kind(), &stored, &public_key) {
            let expected = if shape.well_formed {
                op.absent()
            } else {
                ID_FORMAT_INVALID
            };
            let answer = op.send(&router, &shape.id, &stored).await;
            assert_refusal(
                &answer,
                &format!("{op:?} with {}", shape.case),
                op.code(),
                expected,
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

/// A well-formed identifier of the addressed kind that the store DOES hold is served, so the
/// shape check above gates nothing a caller may legitimately ask for: both readers of each
/// kind answer 200 at the minted id, at the version token its create handed back.
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

/// An identifier that addresses NO operation is not judged by its shape: a profile id borne by
/// a submitted DOCUMENT — a mapping's `ProfileId` — is resolved against the store alone, so a
/// configuration create naming a profile the store does not hold reports the ABSENT profile
/// whatever shape that id takes.
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
