// SPDX-License-Identifier: CC0-1.0

//! The ALPHABET a field-level encryption path identifier is written in is what decides whether
//! its FORM is read at all, end to end over HTTP.
//!
//! The rule under test: an operation addressed by a field-level encryption identifier answers
//! its one modelled not-found code (404) for every identifier it cannot resolve — the kind's
//! own not-found error and `AccessDenied` are all these operations model
//! (`smithy_query(query="operation",operation="GetFieldLevelEncryption,GetFieldLevelEncryptionConfig,GetFieldLevelEncryptionProfile,GetFieldLevelEncryptionProfileConfig")`)
//! — so the MESSAGE is what tells the two conditions apart. A value written in the identifier
//! alphabet claims to be an identifier this service issued, so it is carried through to the
//! STORE and reports the record ABSENT — unless it is written exactly as an identifier is ISSUED
//! under another kind's namespace letter, which
//! `field_level_encryption_identifier_width_and_namespace.rs` states and [`unresolved_answer`]
//! decides; the configuration's RESOURCE read reads no form at all, so there every value is
//! carried through to the store. A value carrying a character that alphabet does not admit
//! claims to be no identifier of this mint at all, so there is nothing in it to resolve and
//! every route that reads the form reports the identifier UNREADABLE.
//!
//! The rows vary the dimensions the rule generalises over: the WIDTH (one character, the width
//! this service issues, one short of it, one past it, and far beyond it), the LEADING LETTER
//! (the kind's own, the sibling kind's, a letter no kind of this service mints under, the letter
//! the other entity ids are drawn from, and a digit), the CHARACTER CLASSES the body mixes
//! (letters only, digits only, both), both RESOURCE KINDS of this surface and every OPERATION
//! addressed by one of their identifiers — against the values outside the alphabet, which are
//! the suite's contrast rows.
//!
//! The harness is the one `field_level_encryption_identifier_width_and_namespace.rs` publishes
//! ([`Kind`], [`seed`], [`address`], [`assert_refusal`], [`unresolved_answer`]), reused rather
//! than copied; the width this service issues is read
//! off an identifier it really minted, so no width here is written out by hand.

use crate::distribution_lifecycle::emulator;
use crate::field_level_encryption_config_lifecycle::get_config;
use crate::field_level_encryption_identifier_width_and_namespace::{
    address, assert_refusal, seed, unresolved_answer, Kind, Seeded,
};
use crate::field_level_encryption_profile_lifecycle::get_profile;

/// A body of `width` characters of the identifier alphabet, cycling its digits and uppercase
/// letters so no width is a value this emulator ever minted.
///
/// Published for the sibling suites that submit a body of the identifier alphabet
/// (`field_level_encryption_resource_read_reads_no_identifier_form.rs`), so the alphabet lives
/// in one place.
pub(crate) fn body(width: usize) -> String {
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .cycle()
        .take(width)
        .collect()
}

/// Every value this suite submits where an identifier of `kind` belongs that IS written in the
/// identifier alphabet, derived from an identifier this emulator really minted so the issued
/// width is never written out by hand.
fn in_the_alphabet(kind: Kind, seeded: &Seeded) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let issued_width = minted.len() - 1;
    let own_letter = minted.chars().next().unwrap_or('C');
    let sibling_letter = match kind {
        Kind::Config => seeded.minted(Kind::Profile),
        Kind::Profile => seeded.minted(Kind::Config),
    }
    .chars()
    .next()
    .unwrap_or(own_letter);

    let mut rows = Vec::new();

    // The WIDTH, under the kind's own leading letter: a bare letter, one short of the issued
    // width, the issued width, one past it, and far past it.
    for width in [
        0,
        issued_width - 1,
        issued_width,
        issued_width + 1,
        issued_width * 3,
    ] {
        rows.push((
            format!("the kind's own leading letter over a body of {width} characters"),
            format!("{own_letter}{}", body(width)),
        ));
    }

    // The LEADING CHARACTER, at the issued width: the sibling kind's letter, a letter no kind
    // of this service mints under, the letter every other entity id of this service is drawn
    // from, and a digit.
    for (case, leading) in [
        ("the sibling kind's leading letter", sibling_letter),
        ("a letter no kind of this service mints under", 'W'),
        ("the letter every other entity id is drawn from", 'E'),
        ("a leading digit", '7'),
    ] {
        rows.push((
            format!("{case} over a body of the issued width"),
            format!("{leading}{}", body(issued_width)),
        ));
    }

    // The CHARACTER CLASSES a body of the issued width mixes.
    for (case, filler) in [
        ("a body of one letter", 'Q'),
        ("a body of digits only", '7'),
        ("a body of zeros", '0'),
    ] {
        rows.push((
            case.to_string(),
            format!("{own_letter}{}", filler.to_string().repeat(issued_width)),
        ));
    }

    // And the minted identifier with its last character changed, which differs from a value
    // the store really holds in one position.
    let mut changed: Vec<char> = minted.chars().collect();
    if let Some(last) = changed.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }
    rows.push((
        "the minted identifier with its last character changed".to_string(),
        changed.into_iter().collect(),
    ));

    for (case, id) in &rows {
        assert_ne!(
            id, minted,
            "case {case:?}: the generated identifier must not be the one the store holds"
        );
    }
    rows
}

/// Every value this suite submits where an identifier of `kind` belongs that is written OUTSIDE
/// the identifier alphabet: the character that is not of it is what each row varies. The UUID
/// form CloudFront issues the identifiers of its other kinds in is outside the alphabet too but
/// is no such value — it is a form CloudFront really issues, so it is carried through to the
/// store, which
/// `field_level_encryption_identifier_width_and_namespace.rs` drives.
fn outside_the_alphabet(kind: Kind, seeded: &Seeded) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let issued_width = minted.len() - 1;
    let own_letter = minted.chars().next().unwrap_or('C');

    vec![
        (
            "the minted identifier in lower case".to_string(),
            minted.to_lowercase(),
        ),
        (
            "one lower-cased character in a body of the issued width".to_string(),
            format!("{own_letter}{}a", body(issued_width - 1)),
        ),
        (
            "a hyphen in a body of the issued width".to_string(),
            format!("{own_letter}{}-", body(issued_width - 1)),
        ),
        (
            "an underscore in a body of the issued width".to_string(),
            format!("{own_letter}{}_", body(issued_width - 1)),
        ),
        ("a hyphenated word".to_string(), "not-an-identifier".to_string()),
    ]
}

/// RULE: a value written in the identifier alphabet claims to be an identifier this service
/// issued, so it is carried through to the STORE whatever body or width it carries — unless it
/// is written exactly as an identifier is ISSUED under the namespace letter of another kind this
/// service issues identifiers under, which makes it that kind's identifier ([`unresolved_answer`])
/// — and one the store holds no record for is the record reported ABSENT, through every
/// operation addressed by an identifier of either kind.
#[tokio::test]
async fn a_value_of_the_identifier_alphabet_is_carried_through_to_the_store() {
    let router = emulator();
    let seeded = seed(&router).await;

    for kind in [Kind::Config, Kind::Profile] {
        let minted = seeded.minted(kind).to_string();
        for (case, id) in in_the_alphabet(kind, &seeded) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                let case = format!("{operation} with {case} ({id:?})");
                assert_refusal(
                    &answer,
                    &case,
                    kind.code(),
                    unresolved_answer(operation, kind, &id, &minted),
                );
            }
        }
    }

    // Both seeded resources are still readable at their own identifiers, so every sentence
    // above answered an identifier rather than a store this suite broke.
    for (case, answer) in [
        (
            "the seeded configuration",
            get_config(&router, &seeded.config_id).await,
        ),
        (
            "the seeded profile",
            get_profile(&router, &seeded.profile_id).await,
        ),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected it to survive every refusal, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// RULE's boundary: a value carrying a character the alphabet does not admit claims to be no
/// identifier this service issued, so there is nothing in it to resolve — every operation that
/// reads the alphabet reports the identifier UNREADABLE, and the one that reads no form at all
/// carries it through to the store ([`unresolved_answer`]).
#[tokio::test]
async fn a_value_carrying_a_character_outside_the_alphabet_is_reported_unreadable() {
    let router = emulator();
    let seeded = seed(&router).await;

    for kind in [Kind::Config, Kind::Profile] {
        let minted = seeded.minted(kind).to_string();
        for (case, id) in outside_the_alphabet(kind, &seeded) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                assert_refusal(
                    &answer,
                    &format!("{operation} with {case} ({id:?})"),
                    kind.code(),
                    unresolved_answer(operation, kind, &id, &minted),
                );
            }
        }
    }

    // And both seeded resources are still readable at their own identifiers afterwards.
    for (case, answer) in [
        (
            "the seeded configuration",
            get_config(&router, &seeded.config_id).await,
        ),
        (
            "the seeded profile",
            get_profile(&router, &seeded.profile_id).await,
        ),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected it to survive every refusal, got {} with {}",
            answer.status, answer.raw
        );
    }
}
