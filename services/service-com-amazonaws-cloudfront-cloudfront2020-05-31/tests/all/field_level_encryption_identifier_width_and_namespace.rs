// SPDX-License-Identifier: CC0-1.0

//! The WIDTH and the NAMESPACE LETTER of a field-level encryption path identifier are not what
//! the service refuses a request on, end to end over HTTP.
//!
//! The rule under test: an operation addressed by a field-level encryption identifier answers
//! its one modelled not-found code (404) for every identifier it cannot resolve, and reports
//! the IDENTIFIER as unreadable ("Id format is invalid") only for a value it could not have
//! read as an identifier of the addressed kind at all — one not written in CloudFront's
//! identifier form (a leading namespace letter followed by a non-empty body of the identifier
//! alphabet: ASCII digits and uppercase letters), or a minted identifier of ANOTHER kind of
//! this service. Every identifier it DOES read and holds no resource for is the ABSENT
//! resource ("The specified field level encryption config/profile does not exist."), and the
//! members that carry these identifiers are plain `string` in the model — no `@pattern` and no
//! `@length` (`smithy_query(query="operation",operation="GetFieldLevelEncryption,GetFieldLevelEncryptionConfig,GetFieldLevelEncryptionProfile,GetFieldLevelEncryptionProfileConfig")`)
//! — so neither the number of characters in the body nor a namespace letter this emulator does
//! not happen to mint under is something a request may be refused on.
//!
//! The rows therefore vary what the rule generalises over:
//!
//! * the BODY WIDTH, from one character up to three times the width this service mints,
//!   including one short of and one past it;
//! * the NAMESPACE LETTER: the addressed kind's own, and one no kind of this service mints
//!   under;
//! * both RESOURCE KINDS of this surface, each through every operation that addresses it by a
//!   path identifier — both readers, the update and the delete;
//! * against the values that are NOT identifiers of the addressed kind at all: a minted
//!   identifier of each neighbouring kind, a bare namespace letter with no body, a minted id in
//!   lower case, a UUID, a caller reference and a value that starts with a digit.
//!
//! The minted width is read off an identifier this emulator really issued, so no width here is
//! written out by hand. The harness, the document builders and the create paths are the
//! crate's own, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
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

/// The caller references the two seeded resources are held under.
const CONFIG_REFERENCE: &str = "fle-id-width-config";
const PROFILE_REFERENCE: &str = "fle-id-width-profile";

/// A namespace letter no kind of this service mints an identifier under, so a value carrying it
/// is in the identifier form without being any other kind's identifier.
const UNMINTED_NAMESPACE: char = 'W';

/// Which field-level encryption resource an operation's path identifier names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Config,
    Profile,
}

impl Kind {
    /// The modelled error code every operation addressed by an identifier of this kind answers
    /// an identifier it cannot resolve.
    fn code(self) -> &'static str {
        match self {
            Self::Config => "NoSuchFieldLevelEncryptionConfig",
            Self::Profile => "NoSuchFieldLevelEncryptionProfile",
        }
    }

    /// The sentence this kind answers an identifier the service READS and holds no resource for.
    fn absent(self) -> &'static str {
        match self {
            Self::Config => NO_SUCH_CONFIG,
            Self::Profile => NO_SUCH_PROFILE,
        }
    }

    /// The path segment the kind's resources live under, from each operation's `http` binding.
    fn segment(self) -> &'static str {
        match self {
            Self::Config => "field-level-encryption",
            Self::Profile => "field-level-encryption-profile",
        }
    }
}

/// The two resources this emulator really stored, with the documents and version tokens the
/// updates and the deletes below submit, so every refusal is about the identifier rather than
/// about a payload or a precondition.
struct Seeded {
    config_id: String,
    config_etag: String,
    config_document: String,
    profile_id: String,
    profile_etag: String,
    profile_document: String,
}

impl Seeded {
    /// The identifier this emulator minted for `kind` — the row every width below is derived
    /// from.
    fn minted(&self, kind: Kind) -> &str {
        match kind {
            Kind::Config => &self.config_id,
            Kind::Profile => &self.profile_id,
        }
    }

    /// The version token the kind's stored resource is currently at.
    fn etag(&self, kind: Kind) -> &str {
        match kind {
            Kind::Config => &self.config_etag,
            Kind::Profile => &self.profile_etag,
        }
    }

    /// A document the kind's update accepts, so an update is never refused for its payload.
    fn document(&self, kind: Kind) -> &str {
        match kind {
            Kind::Config => &self.config_document,
            Kind::Profile => &self.profile_document,
        }
    }
}

/// Store one profile (encrypting with a freshly uploaded public key) and one configuration
/// naming it.
async fn seed(router: &MultiServiceRouter) -> Seeded {
    let key = upload_key(router, 0).await;
    let profile_document = Profile::new(
        "the seeded profile",
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

    let config_document = simple("the seeded configuration", CONFIG_REFERENCE, &profile_id).xml();
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

    Seeded {
        config_etag: created_config.header("etag").to_string(),
        config_id,
        config_document,
        profile_etag: created_profile.header("etag").to_string(),
        profile_id,
        profile_document,
    }
}

/// Send every operation addressed by an identifier of `kind` at `id`, each answer labelled with
/// the operation's name.
async fn address(
    router: &MultiServiceRouter,
    kind: Kind,
    id: &str,
    seeded: &Seeded,
) -> Vec<(&'static str, Answer)> {
    let segment = kind.segment();
    let etag = seeded.etag(kind);
    let document = seeded.document(kind);
    let (resource_reader, config_reader) = match kind {
        Kind::Config => ("GetFieldLevelEncryption", "GetFieldLevelEncryptionConfig"),
        Kind::Profile => (
            "GetFieldLevelEncryptionProfile",
            "GetFieldLevelEncryptionProfileConfig",
        ),
    };
    let (update, delete) = match kind {
        Kind::Config => (
            "UpdateFieldLevelEncryptionConfig",
            "DeleteFieldLevelEncryptionConfig",
        ),
        Kind::Profile => (
            "UpdateFieldLevelEncryptionProfile",
            "DeleteFieldLevelEncryptionProfile",
        ),
    };

    let resource = match kind {
        Kind::Config => get_config(router, id).await,
        Kind::Profile => get_profile(router, id).await,
    };
    let configuration = match kind {
        Kind::Config => get_config_document(router, id).await,
        Kind::Profile => get_profile_config(router, id).await,
    };
    let updated = call(
        router,
        "PUT",
        &format!("/{API_VERSION}/{segment}/{id}/config"),
        Some(document),
        Some(etag),
    )
    .await;
    let deleted = call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/{segment}/{id}"),
        None,
        Some(etag),
    )
    .await;

    vec![
        (resource_reader, resource),
        (config_reader, configuration),
        (update, updated),
        (delete, deleted),
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

/// The widths this suite submits a body at, around the width this service mints (`minted`):
/// one character, two, one short of the minted width, the minted width itself, one past it,
/// twice it and three times it.
fn widths(minted: usize) -> Vec<usize> {
    vec![1, 2, minted - 1, minted, minted + 1, minted * 2, minted * 3]
}

/// A body of `width` characters of the identifier alphabet that no identifier this emulator
/// minted carries: the digits and uppercase letters repeat, so every width is a value of the
/// form without being a value the store ever issued.
fn never_minted_body(width: usize) -> String {
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .cycle()
        .take(width)
        .collect()
}

/// Every identifier of the CloudFront identifier form this suite submits where an identifier of
/// `kind` belongs, each a value the service READS and the store holds nothing for: the
/// addressed kind's own namespace letter and one no kind of this service mints, each at every
/// width in [`widths`].
fn readable_identifiers(kind: Kind, seeded: &Seeded) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let namespace = minted.chars().next().unwrap_or(UNMINTED_NAMESPACE);
    let minted_width = minted.len() - 1;

    let mut rows = Vec::new();
    for letter in [namespace, UNMINTED_NAMESPACE] {
        for width in widths(minted_width) {
            let id = format!("{letter}{}", never_minted_body(width));
            assert_ne!(
                id, minted,
                "case the generated identifier {id:?}: it must not be the one the store holds"
            );
            rows.push((
                format!("the namespace letter {letter} with a body of {width} characters"),
                id,
            ));
        }
    }
    rows
}

/// Every value this suite submits where an identifier of `kind` belongs that the service could
/// not have read as one at all: a minted identifier of each neighbouring kind, a bare namespace
/// letter, the minted identifier in lower case, a UUID, the resource's caller reference and a
/// value starting with a digit.
fn unreadable_identifiers(kind: Kind, seeded: &Seeded, public_key: &str) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let sibling = match kind {
        Kind::Config => seeded.minted(Kind::Profile),
        Kind::Profile => seeded.minted(Kind::Config),
    };
    let reference = match kind {
        Kind::Config => CONFIG_REFERENCE,
        Kind::Profile => PROFILE_REFERENCE,
    };
    let body = &minted[1..];

    vec![
        (
            "the sibling field-level encryption kind's minted identifier".to_string(),
            sibling.to_string(),
        ),
        (
            "a public key identifier this service minted".to_string(),
            public_key.to_string(),
        ),
        (
            "the namespace other CloudFront entity identifiers are drawn from".to_string(),
            format!("E{body}"),
        ),
        (
            "a namespace letter with no body at all".to_string(),
            UNMINTED_NAMESPACE.to_string(),
        ),
        (
            "the minted identifier in lower case".to_string(),
            minted.to_lowercase(),
        ),
        (
            "a UUID, the namespace the policy kinds are keyed by".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the resource's caller reference rather than its identifier".to_string(),
            reference.to_string(),
        ),
        (
            "a value that starts with a digit".to_string(),
            format!("0{body}"),
        ),
    ]
}

/// An identifier in CloudFront's identifier form is READ whatever its width and whatever
/// namespace letter it carries: every operation addressed by one answers the ABSENT resource,
/// never the unreadable-identifier sentence.
#[tokio::test]
async fn an_identifier_of_the_form_is_read_at_every_width_and_namespace() {
    let router = emulator();
    let seeded = seed(&router).await;

    for kind in [Kind::Config, Kind::Profile] {
        for (case, id) in readable_identifiers(kind, &seeded) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                assert_refusal(
                    &answer,
                    &format!("{operation} with {case} ({id:?})"),
                    kind.code(),
                    kind.absent(),
                );
            }
        }
    }

    // Every refusal above left both seeded resources where they were.
    assert_eq!(
        get_config(&router, &seeded.config_id).await.status,
        200,
        "case the seeded configuration: expected it to survive every refusal"
    );
    assert_eq!(
        get_profile(&router, &seeded.profile_id).await.status,
        200,
        "case the seeded profile: expected it to survive every refusal"
    );
}

/// A value that is not an identifier of the addressed kind at all — another kind's minted
/// identifier, or a value outside the identifier form — is reported as the identifier the
/// service cannot read, by every operation addressed by one.
#[tokio::test]
async fn a_value_that_is_no_identifier_of_the_kind_is_reported_unreadable() {
    let router = emulator();
    let seeded = seed(&router).await;
    let public_key = upload_key(&router, 1).await;

    for kind in [Kind::Config, Kind::Profile] {
        for (case, id) in unreadable_identifiers(kind, &seeded, &public_key) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                assert_refusal(
                    &answer,
                    &format!("{operation} with {case} ({id:?})"),
                    kind.code(),
                    ID_FORMAT_INVALID,
                );
            }
        }
    }

    // And both seeded resources are still readable at their own identifiers afterwards.
    assert_eq!(
        get_config(&router, &seeded.config_id).await.status,
        200,
        "case the seeded configuration: expected it to survive every refusal"
    );
    assert_eq!(
        get_profile(&router, &seeded.profile_id).await.status,
        200,
        "case the seeded profile: expected it to survive every refusal"
    );
}
