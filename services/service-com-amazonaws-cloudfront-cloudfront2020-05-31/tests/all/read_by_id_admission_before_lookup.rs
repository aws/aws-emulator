// SPDX-License-Identifier: CC0-1.0

//! What a CloudFront read ADDRESSED BY AN IDENTIFIER decides about the submitted value before it
//! goes near the store, end to end over HTTP.
//!
//! The rule under test: a read addressed by an identifier of a kind CloudFront mints under its
//! OWN namespace letter — a public key (`K`), a field-level encryption profile (`P`), a
//! field-level encryption configuration (`C`) — answers the kind's ONE modelled not-found code
//! and its 404 for every identifier it cannot resolve
//! (`smithy_query(query="operation",operation="GetPublicKey,GetFieldLevelEncryptionProfileConfig,\
//! GetFieldLevelEncryptionConfig,GetDistributionConfig")` reports each one's not-found error and
//! `AccessDenied` and no argument error, over an `Id` that is a plain `string` `@httpLabel` with
//! no `@pattern` and no `@length`), so the MESSAGE is the whole of what tells two conditions
//! apart, and it is decided from the VALUE alone, before the lookup:
//!
//! * a value that IS an identifier CloudFront issued for ANOTHER kind of this service — the
//!   alphabet of decimal digits and uppercase letters at the width an identifier is issued at,
//!   under another kind's namespace letter — names that other kind's record, so no record of the
//!   addressed kind could ever answer to it and it is the identifier the route reports it cannot
//!   READ ("Id format is invalid");
//! * beyond that one claim, HOW MUCH of the value a route reads is the addressed kind's own
//!   recorded contract: the field-level encryption records' routes also refuse a value written
//!   OUTSIDE the alphabet an identifier is written in, while the public key's routes read
//!   nothing else at all, as their recorded answers to a probe written in lower case around
//!   separators report;
//! * every other value is carried through to the STORE — WHICH identifier of a kind was ever
//!   issued is a property of the store rather than of the value, the width this service
//!   issues at is one width CloudFront writes an identifier in rather than the only one, so a
//!   value at ANY other width could be one too, whatever letter it opens with, and a UUID marks
//!   no kind at all — and the record the store
//!   holds none under is the resource reported ABSENT, under that kind's own fixed sentence.
//!
//! The distribution is the further contrast this suite drives beside the three kinds above: a
//! distribution's
//! identifier comes from the namespace its kind SHARES with three others (`E`), so neither the
//! letter nor the width says which of them issued it and `GetDistributionConfig` reads the
//! ALPHABET and nothing more — a value written in it is carried through to the store however wide
//! it is and whichever other kind holds it, while a value written outside it is the identifier
//! that route cannot read.
//!
//! The rows span what the rule generalises over, derived from identifiers these creates really
//! minted so no width or letter is written out by hand:
//!
//! * the KIND addressed: a public key, a field-level encryption profile and a field-level
//!   encryption configuration, each through the read the recorded probes exercise, plus the
//!   distribution's configuration read as the contrast contract;
//! * the WIDTH: the issued width, one character short of it, the bare namespace letter, one
//!   character past it and twice it;
//! * the LEADING CHARACTER at the issued width: the kind's own letter, another kind's letter
//!   (including an identifier another kind of this account really holds) and a letter no kind of
//!   this service issues an identifier under;
//! * the CHARACTER CLASSES: a body of zeros, an uppercase word, the minted identifier recased, a
//!   UUID and a body broken by a separator.
//!
//! Both sentences are fixed text, so no row may see the submitted value echoed back, and every
//! live resource is read at its own identifier after all of them — these are reporting calls, so
//! none of them may change what a later caller sees.
//!
//! The wire harness and the create helpers are the crate's own, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, is_minted_uuid, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config_document, simple,
};
// The alphabet predicate and the refusal helper are the crate's own: one statement of what an
// identifier is written in and one place that holds a refusal to its status, code and sentence,
// so no two suites can disagree about either.
use crate::field_level_encryption_identifier_width_and_namespace::{
    assert_refusal, in_the_identifier_alphabet,
};
use crate::field_level_encryption_profile_lifecycle::{get_profile_config, store_profile};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentences CloudFront sends on the wire for these refusals, spelled out here rather than
/// read off the crate under test, so a reworded refusal fails this suite.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";
const NO_SUCH_CONFIG: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_DISTRIBUTION: &str = "The specified distribution does not exist.";

/// The caller reference every resource this suite stands up is created under.
const REFERENCE: &str = "read-by-id-admission";

/// One resource kind this suite addresses a read of, named for the kind rather than for the
/// route, since what each route reads of its identifier is the KIND's contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `GET /2020-05-31/public-key/{Id}` — the read of cause 3's recorded probe.
    PublicKey,
    /// `GET /2020-05-31/field-level-encryption-profile/{Id}/config` — cause 2's.
    FieldLevelEncryptionProfile,
    /// `GET /2020-05-31/field-level-encryption/{Id}/config` — the sibling record of the same
    /// surface, whose identifiers come from a namespace of its own as well.
    FieldLevelEncryptionConfig,
    /// `GET /2020-05-31/distribution/{Id}/config` — cause 1's, and the contrast: its kind
    /// SHARES its namespace letter, so this route reads the alphabet and nothing else.
    Distribution,
}

/// Every kind this suite drives.
const KINDS: &[Kind] = &[
    Kind::PublicKey,
    Kind::FieldLevelEncryptionProfile,
    Kind::FieldLevelEncryptionConfig,
    Kind::Distribution,
];

impl Kind {
    /// The modelled error code the read answers an identifier it cannot resolve.
    fn code(self) -> &'static str {
        match self {
            Self::PublicKey => "NoSuchPublicKey",
            Self::FieldLevelEncryptionProfile => "NoSuchFieldLevelEncryptionProfile",
            Self::FieldLevelEncryptionConfig => "NoSuchFieldLevelEncryptionConfig",
            Self::Distribution => "NoSuchDistribution",
        }
    }

    /// The sentence that code carries for an identifier the store holds no record under.
    fn absent(self) -> &'static str {
        match self {
            Self::PublicKey => NO_SUCH_PUBLIC_KEY,
            Self::FieldLevelEncryptionProfile => NO_SUCH_PROFILE,
            Self::FieldLevelEncryptionConfig => NO_SUCH_CONFIG,
            Self::Distribution => NO_SUCH_DISTRIBUTION,
        }
    }

    /// Whether this kind's identifiers come from a namespace of its OWN, which is what lets a
    /// route read the leading character of a value written at the issued width. The distribution
    /// shares its letter with three other kinds, so its routes read no letter at all.
    fn has_its_own_namespace(self) -> bool {
        self != Self::Distribution
    }

    /// Whether this kind's routes refuse a value written OUTSIDE the alphabet CloudFront writes
    /// an identifier in, as their recorded answers report: the field-level encryption records'
    /// and the distribution's do, while the public key's carry such a value through to the store
    /// and report the key it holds none under as absent.
    fn reads_the_alphabet(self) -> bool {
        self != Self::PublicKey
    }

    /// Address `id` through this kind's read.
    async fn read(self, router: &MultiServiceRouter, id: &str) -> Answer {
        match self {
            Self::PublicKey => get_key(router, id).await,
            Self::FieldLevelEncryptionProfile => get_profile_config(router, id).await,
            Self::FieldLevelEncryptionConfig => get_config_document(router, id).await,
            Self::Distribution => {
                call(
                    router,
                    "GET",
                    &format!("/{API_VERSION}/distribution/{id}/config"),
                    None,
                    None,
                )
                .await
            }
        }
    }
}

/// The identifier this account really holds for each kind, each minted by that kind's own create.
struct Live {
    public_key: String,
    profile: String,
    config: String,
    distribution: String,
}

impl Live {
    /// The identifier of `kind` this account holds.
    fn minted(&self, kind: Kind) -> &str {
        match kind {
            Kind::PublicKey => &self.public_key,
            Kind::FieldLevelEncryptionProfile => &self.profile,
            Kind::FieldLevelEncryptionConfig => &self.config,
            Kind::Distribution => &self.distribution,
        }
    }

    /// An identifier another kind of this account really holds, at the width identifiers of this
    /// mint are issued at: the row only the leading character tells from this kind's own.
    fn another_kinds(&self, kind: Kind) -> &str {
        match kind {
            Kind::PublicKey => &self.profile,
            Kind::FieldLevelEncryptionProfile => &self.public_key,
            Kind::FieldLevelEncryptionConfig => &self.public_key,
            Kind::Distribution => &self.public_key,
        }
    }
}

/// Create one resource of each kind through its own API, and report the identifier each create
/// handed back. Every one is asserted to be written at one width in the identifier alphabet, so
/// the rows derived from them really are of the form they claim.
async fn live(router: &MultiServiceRouter) -> Live {
    let created_key = create_key(
        router,
        &Key {
            case: "the public key this table addresses".to_string(),
            caller_reference: format!("{REFERENCE}-key"),
            name: format!("{REFERENCE}-key"),
            encoded_key: pem(2, 'A'),
            comment: None,
        }
        .xml(),
    )
    .await;
    assert_eq!(
        created_key.status, 201,
        "case creating the public key: expected HTTP 201, got {} with {}",
        created_key.status, created_key.raw
    );

    let profile = store_profile(router, &format!("{REFERENCE}-profile")).await;

    let created_config = create_config(
        router,
        &simple(
            "the configuration this table addresses",
            &format!("{REFERENCE}-config"),
            &profile,
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_config.status, 201,
        "case creating the configuration: expected HTTP 201, got {} with {}",
        created_config.status, created_config.raw
    );

    let created_distribution = create_distribution(
        router,
        &config("the distribution this table addresses", REFERENCE).xml(),
    )
    .await;
    assert_eq!(
        created_distribution.status, 201,
        "case creating the distribution: expected HTTP 201, got {} with {}",
        created_distribution.status, created_distribution.raw
    );

    let live = Live {
        public_key: created_key
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        profile,
        config: created_config
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        distribution: created_distribution
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
    };

    // Every identifier the rows are derived from is written in the identifier alphabet, and all
    // four are issued at ONE width — which is what makes "the issued width" a property this
    // suite can read off the service rather than write out.
    let issued = live.public_key.len();
    for kind in KINDS {
        let id = live.minted(*kind);
        assert!(
            in_the_identifier_alphabet(id),
            "case {kind:?}: expected a minted identifier of the identifier alphabet, got {id:?}"
        );
        assert_eq!(
            id.len(),
            issued,
            "case {kind:?}: expected a minted identifier of {issued} characters, got {id:?}"
        );
    }
    live
}

/// One row: the case it is a case of, the value submitted where an identifier belongs, and what
/// about the value rules it out as an identifier of the addressed kind.
struct Row {
    case: String,
    id: String,
    /// `true` where the value is written OUTSIDE the alphabet CloudFront writes an identifier
    /// in, so it claims to be no identifier of this mint at all — which the routes whose kind's
    /// recorded answers read the alphabet refuse it for.
    outside_the_alphabet: bool,
    /// `true` where the value IS an identifier CloudFront issued for another kind of this
    /// service: that alphabet at the issued width under another kind's namespace letter — which
    /// only a kind whose namespace letter is its OWN reads.
    issued_for_another_kind: bool,
}

/// The rows this suite submits where an identifier of `kind` belongs, each derived from the
/// identifiers this account really holds.
fn rows(kind: Kind, live: &Live) -> Vec<Row> {
    let minted = live.minted(kind);
    let letter = minted.chars().next().expect("a minted identifier");
    let body_width = minted.len() - 1;
    let body = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .cycle()
        .take(body_width)
        .collect::<String>();
    let rows = vec![
        // Carried through to the store: the issued width under the kind's own letter.
        Row {
            case: "the kind's own letter over a body of zeros at the issued width".to_string(),
            id: format!("{letter}{}", "0".repeat(body_width)),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        // Carried through: any width other than the issued one, whatever letter it opens with,
        // since the width this service issues at is one width CloudFront writes an identifier in
        // rather than the only one.
        Row {
            case: "the kind's own letter over a body one character short".to_string(),
            id: format!("{letter}{}", &body[..body_width - 1]),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        Row {
            case: "the kind's own namespace letter with no body at all".to_string(),
            id: letter.to_string(),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        Row {
            case: "a letter no kind of this service issues an identifier under, at the issued \
                   width"
                .to_string(),
            id: format!("W{body}"),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        Row {
            case: "the kind's own letter over a body one character too long".to_string(),
            id: format!("{letter}{body}A"),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        Row {
            case: "the kind's own letter over a body twice the issued width".to_string(),
            id: format!("{letter}{body}{body}"),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        Row {
            case: "an uppercase word wider than any identifier is issued at".to_string(),
            id: "NONEXISTENTIDENTIFIER".to_string(),
            outside_the_alphabet: false,
            issued_for_another_kind: false,
        },
        // Outside the alphabet an identifier is written in at all.
        Row {
            case: "the minted identifier in lower case".to_string(),
            id: minted.to_lowercase(),
            outside_the_alphabet: true,
            issued_for_another_kind: false,
        },
        Row {
            case: "a UUID, the shape the policy kinds' identifiers take".to_string(),
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            outside_the_alphabet: true,
            issued_for_another_kind: false,
        },
        Row {
            case: "the issued width with its body broken by a separator".to_string(),
            id: format!("{letter}{}-A", &body[..body_width - 2]),
            outside_the_alphabet: true,
            issued_for_another_kind: false,
        },
        Row {
            case: "the caller reference rather than an identifier".to_string(),
            id: REFERENCE.to_string(),
            outside_the_alphabet: true,
            issued_for_another_kind: false,
        },
        // Ruled out within the alphabet by the LEADING CHARACTER alone, at the issued width:
        // the value IS an identifier of another kind.
        Row {
            case: "an identifier another kind of this account really holds".to_string(),
            id: live.another_kinds(kind).to_string(),
            outside_the_alphabet: false,
            issued_for_another_kind: true,
        },
        Row {
            case: "the namespace letter the shared-form family is drawn from, at the issued width"
                .to_string(),
            id: format!("E{body}"),
            outside_the_alphabet: false,
            // Where the addressed kind is itself of that family the letter is its OWN, and a
            // value under it is one only the store can answer for.
            issued_for_another_kind: letter != 'E',
        },
    ];
    for row in &rows {
        assert_ne!(
            row.id, minted,
            "case {:?}: the value submitted must not be the identifier the store holds",
            row.case
        );
    }
    rows
}

/// The sentence `kind`'s read owes `row`.
///
/// A value that IS an identifier issued for another kind of this service is the identifier a
/// route of a kind with a namespace of its OWN reports it cannot read. Beyond that, how much of
/// a value a route reads is that kind's own recorded contract: the field-level encryption
/// records' and the distribution's routes read the identifier ALPHABET, so a value written
/// outside it is unreadable to them, while the public key's routes read nothing else at all and
/// carry every other value through to the store.
///
/// A value written in the OTHER form CloudFront issues an identifier in — the UUID its key
/// groups and policy kinds are minted with — carries no namespace marker at all and is shared by
/// every kind identified by one, so a route that reads the NAMESPACE finds nothing in it to
/// contradict the addressed kind and carries it through to the store, while the shared-form
/// family's routes, which read the alphabet alone, refuse it.
fn expected(kind: Kind, row: &Row) -> &'static str {
    if row.issued_for_another_kind && kind.has_its_own_namespace() {
        return UNREADABLE_IDENTIFIER;
    }
    if !row.outside_the_alphabet || !kind.reads_the_alphabet() {
        return kind.absent();
    }
    if is_minted_uuid(&row.id) && kind.has_its_own_namespace() {
        return kind.absent();
    }
    UNREADABLE_IDENTIFIER
}

/// RULE: a read addressed by an identifier answers the unreadable-identifier sentence for every
/// value that could never be an identifier issued for the addressed kind, and the absent-record
/// sentence for every other value the store holds nothing under — one status and one code either
/// way.
#[tokio::test]
async fn an_unresolvable_identifier_earns_the_sentence_its_own_form_decides() {
    let router = emulator();
    let live = live(&router).await;

    for kind in KINDS {
        for row in rows(*kind, &live) {
            let expected = expected(*kind, &row);
            let case = format!("{kind:?} with {} ({:?})", row.case, row.id);
            let answer = kind.read(&router, &row.id).await;
            // The status, the code and the sentence are asserted by the crate's own refusal
            // helper, so this suite holds a refusal to the same three things its siblings do.
            assert_refusal(&answer, &case, kind.code(), expected);
            // Both sentences are fixed text: nothing the request submitted reaches the wire.
            // Read only of a value long enough for its presence to mean something, since a
            // single character of the alphabet occurs in the envelope's own element names.
            if row.id.len() >= 8 {
                assert!(
                    !answer.raw.contains(row.id.as_str()),
                    "case {case}: the refusal echoed {:?} back in {}",
                    row.id,
                    answer.raw
                );
            }
        }
    }
}

/// RULE, as the positive control: the identifier each kind was really issued is served by that
/// kind's read, before and after every refusal above — so no admission check stands between a
/// caller and a record, and no reporting call changed what a later caller sees.
#[tokio::test]
async fn every_minted_identifier_is_served_before_and_after_every_refusal() {
    let router = emulator();
    let live = live(&router).await;

    for stage in ["before any refusal", "after every refusal"] {
        for kind in KINDS {
            let id = live.minted(*kind);
            let answer = kind.read(&router, id).await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?} at its own identifier ({id:?}), {stage}: expected HTTP 200, got {} \
                 with {}",
                answer.status, answer.raw
            );
        }
        if stage == "before any refusal" {
            for kind in KINDS {
                for row in rows(*kind, &live) {
                    let answer = kind.read(&router, &row.id).await;
                    assert_eq!(
                        answer.status, 404,
                        "case {kind:?} with {} ({:?}): expected HTTP 404, got {} with {}",
                        row.case, row.id, answer.status, answer.raw
                    );
                }
            }
        }
    }
}

/// RULE, at the boundary the two contracts differ on: the distribution's configuration read
/// carries an identifier another kind of this account really holds through to the store, because
/// its kind SHARES its namespace letter and the value could still be a distribution identifier,
/// while the three kinds that own their letter report the same value as the identifier they
/// cannot read.
#[tokio::test]
async fn the_form_beyond_the_alphabet_is_read_only_where_the_kind_owns_its_namespace() {
    let router = emulator();
    let live = live(&router).await;

    let crossed = live.public_key.clone();
    let distribution_answer = Kind::Distribution.read(&router, &crossed).await;
    let message = distribution_answer
        .root()
        .find_text("Message")
        .unwrap_or_default();
    assert_eq!(
        message,
        NO_SUCH_DISTRIBUTION,
        "case GetDistributionConfig with a public key identifier ({crossed:?}): expected the \
         sentence {NO_SUCH_DISTRIBUTION:?}, got {message:?}"
    );

    for kind in [
        Kind::FieldLevelEncryptionProfile,
        Kind::FieldLevelEncryptionConfig,
    ] {
        let answer = kind.read(&router, &crossed).await;
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, UNREADABLE_IDENTIFIER,
            "case {kind:?} with a public key identifier ({crossed:?}): expected the sentence \
             {UNREADABLE_IDENTIFIER:?}, got {message:?}"
        );
    }

    // And the distribution's own identifier, submitted to the public key read, is the identifier
    // that read cannot see past: the two contracts are not the same rule read twice.
    let answer = Kind::PublicKey.read(&router, &live.distribution).await;
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, UNREADABLE_IDENTIFIER,
        "case GetPublicKey with a distribution identifier ({:?}): expected the sentence \
         {UNREADABLE_IDENTIFIER:?}, got {message:?}",
        live.distribution
    );
}
