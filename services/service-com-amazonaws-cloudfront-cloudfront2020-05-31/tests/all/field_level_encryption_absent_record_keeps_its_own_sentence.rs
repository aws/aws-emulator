// SPDX-License-Identifier: CC0-1.0

//! An identifier no field-level encryption record answers to earns the ABSENT-record sentence,
//! not the sentence that says the identifier cannot be read, end to end over HTTP.
//!
//! The rule under test: an operation addressed by a field-level encryption record's `Id` answers
//! that kind's ONE modelled not-found code and its 404 for every identifier it cannot resolve
//! (`smithy_query(query="operation",operation="GetFieldLevelEncryptionConfig,GetFieldLevelEncryptionProfile,DeleteFieldLevelEncryptionConfig,DeleteFieldLevelEncryptionProfile")`
//! reports the kind's not-found error and `AccessDenied` and nothing else, over an `Id` that is a
//! plain `string` `@httpLabel` with no `@pattern` and no `@length`), so the MESSAGE is the whole
//! of what tells two different conditions apart:
//!
//! * a value that COULD be an identifier CloudFront issued for the addressed kind is carried
//!   through to the STORE — WHICH identifier was ever issued is a property of the store rather
//!   than of the value, and the width this service issues at is one width CloudFront writes an
//!   identifier in rather than the only one, so a NARROWER value could be one too — so the record
//!   the store holds none under is
//!   reported ABSENT;
//! * only a value that could NEVER be one — written outside the alphabet CloudFront writes an
//!   identifier in, written WIDER than an identifier of this mint is issued, or written exactly
//!   the way an identifier is ISSUED (that alphabet at the
//!   issued width) under the namespace letter of ANOTHER kind this service issues identifiers
//!   under, which makes it that kind's identifier — is the identifier the route reports it
//!   cannot READ.
//!
//! The two sentences are different client contracts: the first tells a caller the record is gone
//! (create it, or treat the teardown as done), the second that its own identifier handling is
//! broken.
//!
//! The rows vary what the rule generalises over:
//!
//! * the WIDTH a value of the identifier alphabet is written at: the width this service issues
//!   at and one character short of it, each resolved against the store, against one past it,
//!   twice it and a word well above it, each unreadable;
//! * the LEADING LETTER: the addressed kind's own, a letter no kind of this service issues an
//!   identifier under, a leading digit — each resolved against the store — against the sibling
//!   field-level encryption kind's and a public key's really-minted identifiers and the letter
//!   the other CloudFront entity ids are drawn from, which are another kind's identifiers;
//! * the CHARACTER CLASSES of the body: digits and letters, zeros alone, one letter repeated;
//! * the values written OUTSIDE that alphabet: a minted identifier in lower case, a hyphenated
//!   word and the record's own caller reference — each unreadable — beside a UUID, the OTHER
//!   form CloudFront issues an identifier in, which marks no kind and is resolved against the
//!   store;
//! * both RESOURCE KINDS of this surface, each through every operation addressed by one of its
//!   identifiers — both readers, the update and the delete — so the configuration's and the
//!   profile's reads and deletes are each driven.
//!
//! Every width and namespace letter is read off an identifier this emulator really issued, so no
//! width here is written out by hand, and both records are read back at their own identifiers
//! afterwards, so no refusal above touched the store.
//!
//! The harness is the one `field_level_encryption_identifier_width_and_namespace.rs` publishes
//! ([`Kind`], [`seed`], [`address`], [`assert_refusal`], [`reads_the_form`]), reused rather than
//! copied.

use crate::distribution_lifecycle::emulator;
use crate::field_level_encryption_config_lifecycle::get_config;
use crate::field_level_encryption_identifier_alphabet::body;
use crate::field_level_encryption_identifier_width_and_namespace::{
    address, assert_refusal, namespace_letter_of, reads_the_form, seed, Kind, Seeded,
    CONFIG_REFERENCE, PROFILE_REFERENCE, UNREADABLE_IDENTIFIER,
};
use crate::field_level_encryption_profile_lifecycle::{get_profile, upload_key};

/// Every value this suite submits where an identifier of `kind` belongs that the store holds no
/// record under and that COULD be an identifier CloudFront issued for that kind.
fn could_be_this_kinds_identifier(kind: Kind, seeded: &Seeded) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let letter = namespace_letter_of(minted);
    let issued_body = minted.len() - 1;

    let rows = vec![
        // The kind's own namespace letter, at the issued width and at four others.
        (
            "the kind's own letter over a body of zeros at the issued width".to_string(),
            format!("{letter}{}", "0".repeat(issued_body)),
        ),
        (
            "the kind's own letter over a body one character short".to_string(),
            format!("{letter}{}", body(issued_body - 1)),
        ),
        (
            "the kind's own letter over one letter repeated".to_string(),
            format!("{letter}{}", "Q".repeat(issued_body)),
        ),
        // Values of the alphabet carrying no namespace letter of this service, below the issued
        // width — a shape a caller writes when it makes an identifier up.
        (
            "a word of the alphabet shorter than any identifier is issued at".to_string(),
            "NOSUCH1".to_string(),
        ),
        // The OTHER form CloudFront issues an identifier in: a UUID carries no namespace marker
        // at all and every kind this service identifies by one is minted in it, so it says which
        // kind issued it no more than a letter no kind issues under does.
        (
            "a UUID, the shape the policy kinds' identifiers take".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a letter no kind of this service issues an identifier under, at the issued width"
                .to_string(),
            format!("W{}", body(issued_body)),
        ),
        (
            "a leading digit at the issued width".to_string(),
            format!("7{}", body(issued_body)),
        ),
        // And the identifier this service really issued, with its last character changed.
        (
            "the minted identifier with its last character changed".to_string(),
            {
                let mut changed: Vec<char> = minted.chars().collect();
                if let Some(last) = changed.last_mut() {
                    *last = if *last == 'A' { 'B' } else { 'A' };
                }
                changed.into_iter().collect()
            },
        ),
    ];
    for (case, id) in &rows {
        assert_ne!(
            id, minted,
            "case {case:?}: the submitted identifier must not be the one the store holds"
        );
    }
    rows
}

/// Every value this suite submits where an identifier of `kind` belongs that could NEVER be an
/// identifier CloudFront issued for that kind: a value wider than one is ever issued, another
/// kind's issued identifier, and values
/// written outside the alphabet an identifier is written in at all.
fn could_never_be_this_kinds_identifier(
    kind: Kind,
    seeded: &Seeded,
    public_key: &str,
) -> Vec<(String, String)> {
    let minted = seeded.minted(kind);
    let letter = namespace_letter_of(minted);
    let issued_body = minted.len() - 1;
    let sibling = match kind {
        Kind::Config => seeded.minted(Kind::Profile),
        Kind::Profile => seeded.minted(Kind::Config),
    };
    let reference = match kind {
        Kind::Config => CONFIG_REFERENCE,
        Kind::Profile => PROFILE_REFERENCE,
    };

    vec![
        (
            "the kind's own letter over a body one character too long".to_string(),
            format!("{letter}{}", body(issued_body + 1)),
        ),
        (
            "the kind's own letter over a body twice the issued width".to_string(),
            format!("{letter}{}", body(issued_body * 2)),
        ),
        (
            "a word of the alphabet longer than any identifier is issued at".to_string(),
            format!("NONEXISTENTRECORD{}", body(issued_body)),
        ),
        (
            "the sibling field-level encryption kind's really-minted identifier".to_string(),
            sibling.to_string(),
        ),
        (
            "a public key identifier this service really minted".to_string(),
            public_key.to_string(),
        ),
        (
            "the letter the other CloudFront entity ids are drawn from, at the issued width"
                .to_string(),
            format!("E{}", body(issued_body)),
        ),
        (
            "the minted identifier in lower case".to_string(),
            minted.to_lowercase(),
        ),
        (
            "a hyphenated lower-case word".to_string(),
            "nonexistent-record-id-000".to_string(),
        ),
        (
            "the record's caller reference rather than its identifier".to_string(),
            reference.to_string(),
        ),
    ]
}

/// Read both seeded records back at their own identifiers: every refusal above answered an
/// identifier rather than a store this suite broke.
async fn assert_both_records_survived(router: &smithy_server_runtime::MultiServiceRouter, seeded: &Seeded) {
    let config = get_config(router, &seeded.config_id).await;
    assert_eq!(
        config.status, 200,
        "case the seeded configuration: expected it to survive every refusal, got {} with {}",
        config.status, config.raw
    );
    let profile = get_profile(router, &seeded.profile_id).await;
    assert_eq!(
        profile.status, 200,
        "case the seeded profile: expected it to survive every refusal, got {} with {}",
        profile.status, profile.raw
    );
}

/// RULE: every value that could be an identifier of the addressed kind and that the store holds
/// no record under is answered the ABSENT-record sentence — whatever width it is written at and
/// whatever letter it opens with — through every operation addressed by one of these identifiers.
#[tokio::test]
async fn a_value_that_could_be_this_kinds_identifier_reports_the_absent_record() {
    let router = emulator();
    let seeded = seed(&router).await;

    for kind in [Kind::Config, Kind::Profile] {
        for (case, id) in could_be_this_kinds_identifier(kind, &seeded) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                let case = format!("{operation} with {case} ({id:?})");
                let actual = answer.root().find_text("Message").unwrap_or_default();
                assert_ne!(
                    actual, UNREADABLE_IDENTIFIER,
                    "case {case:?}: a value that could be an identifier of this kind must be \
                     resolved against the store, not refused on its form, got {actual:?}"
                );
                assert_refusal(&answer, &case, kind.code(), kind.absent());
            }
        }
    }

    assert_both_records_survived(&router, &seeded).await;
}

/// The RULE's boundary: a value that could never be an identifier of the addressed kind — one
/// written outside the alphabet an identifier is written in, one wider than an identifier of this
/// mint is issued, and another kind's really-issued
/// identifier — is answered the unreadable-identifier sentence by every route that reads the
/// form, while the configuration's resource read, which reads none of it, still reports the
/// record absent.
#[tokio::test]
async fn a_value_that_could_never_be_this_kinds_identifier_is_reported_unreadable() {
    let router = emulator();
    let seeded = seed(&router).await;
    let public_key = upload_key(&router, 1).await;

    for kind in [Kind::Config, Kind::Profile] {
        for (case, id) in could_never_be_this_kinds_identifier(kind, &seeded, &public_key) {
            for (operation, answer) in address(&router, kind, &id, &seeded).await {
                let expected = if reads_the_form(operation) {
                    UNREADABLE_IDENTIFIER
                } else {
                    kind.absent()
                };
                assert_refusal(
                    &answer,
                    &format!("{operation} with {case} ({id:?})"),
                    kind.code(),
                    expected,
                );
            }
        }
    }

    assert_both_records_survived(&router, &seeded).await;
}
