// SPDX-License-Identifier: CC0-1.0

//! The not-found sentence over the WHOLE space of delivery-stream names, with the names
//! generated rather than hand-picked.
//!
//! The invariant: a lookup that resolves no stream states the account it was scoped to and
//! nothing of the name it was given, so one sentence answers every name in the modelled
//! class — the control-plane reads' "Firehose under account `<account>` not found.", the
//! encryption changes' "Firehose under accountId `<account>` not found" and the data
//! plane's "Firehose not found under account `<account>`.". A sentence that varied with
//! the name, or that carried another path's wording, would break it.
//!
//! The generator draws names from `DeliveryStreamName`'s own class — its `@pattern`
//! alphabet, between its two `@length` bounds (`smithy_query(query="shape",
//! shape="DeliveryStreamName")`) — with a bounded number of cases, nothing written to
//! disk, and the drawn seed carried in every assertion message so a failure replays
//! exactly. A name one character PAST the upper bound is asserted to be refused by the
//! request boundary instead, which is what says the sentence belongs to the lookup rather
//! than to any 400.
//!
//! Every request goes through the real router; the harness and the probe tables are the
//! sibling suites'.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;

use super::delivery_stream_lifecycle::{error_code, error_message, Emulator};
use super::stream_not_found_message::{
    control_plane_probes, data_plane_probes, NotFoundWording, Probe,
};

/// The characters `DeliveryStreamName`'s modelled `@pattern` `^[a-zA-Z0-9_.-]+$` admits,
/// and no others.
const NAME_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-";

/// `DeliveryStreamName`'s modelled `@length` bounds.
const MIN_NAME_LENGTH: usize = 1;
const MAX_NAME_LENGTH: usize = 64;

/// How many names are drawn at random, beside the two bound cases. Bounded so the suite
/// stays a test: each name is probed through every operation that resolves one.
const DRAWN_NAMES: usize = 6;

/// The name of the one stream that DOES exist, so each refusal below is a lookup that
/// resolved nothing rather than an empty register.
const PRESENT_STREAM: &str = "present-stream";

/// A name of exactly `length` characters, every character drawn from [`NAME_ALPHABET`].
fn drawn_name(rng: &mut StdRng, length: usize) -> String {
    (0..length)
        .map(|_| char::from(NAME_ALPHABET[rng.random_range(0..NAME_ALPHABET.len())]))
        .collect()
}

/// The names one case of the suite probes: both published `@length` bounds, then
/// [`DRAWN_NAMES`] names of lengths drawn between them.
fn drawn_names(rng: &mut StdRng) -> Vec<String> {
    let mut names = vec![
        drawn_name(rng, MIN_NAME_LENGTH),
        drawn_name(rng, MAX_NAME_LENGTH),
    ];
    for _ in 0..DRAWN_NAMES {
        let length = rng.random_range(MIN_NAME_LENGTH..=MAX_NAME_LENGTH);
        names.push(drawn_name(rng, length));
    }
    names
}

/// Probes `names` through `probes` and asserts every refusal is the sentence that probe's
/// own path words the miss in, verbatim.
async fn each_probe_states(
    emulator: &Emulator,
    probes: &[Probe],
    names: &[String],
    account: &str,
    seed: u64,
) {
    for probe in probes {
        let expected = probe.wording.sentence(account);
        for name in names {
            let case = format!("seed {seed}: {} of {name:?}", probe.case);
            let (status, refused) = emulator.call(probe.operation, (probe.request)(name)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {refused:?}"
            );
            let code = error_code(&refused);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected \"ResourceNotFoundException\", got {code:?}"
            );
            let message = error_message(&refused);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_sentence_answers_every_name_in_the_modelled_class() {
    let seed: u64 = rand::random();
    let mut rng = StdRng::seed_from_u64(seed);
    let names = drawn_names(&mut rng);

    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let control_plane = NotFoundWording::ControlPlaneRead.sentence(account);
    let encryption = NotFoundWording::EncryptionChange.sentence(account);
    let data_plane = NotFoundWording::DataPlane.sentence(account);
    for (left, right) in [
        (&control_plane, &encryption),
        (&control_plane, &data_plane),
        (&encryption, &data_plane),
    ] {
        assert_ne!(
            left, right,
            "seed {seed}: the paths word the same fact differently, so neither sentence \
             may stand in for the other"
        );
    }

    emulator.create_stream(PRESENT_STREAM, None).await;
    emulator.await_active(PRESENT_STREAM).await;

    // Every drawn name is a name no stream has: the generator never draws the one that
    // exists, and a draw that happened to is dropped rather than asserted on.
    let absent: Vec<String> = names
        .into_iter()
        .filter(|name| name != PRESENT_STREAM)
        .collect();
    assert!(
        absent.len() >= DRAWN_NAMES,
        "seed {seed}: the suite needs at least {DRAWN_NAMES} absent names, got {absent:?}"
    );

    each_probe_states(&emulator, &control_plane_probes(), &absent, account, seed).await;
    each_probe_states(&emulator, &data_plane_probes(), &absent, account, seed).await;

    // The stream that does exist is untouched by all of those refusals: it still resolves,
    // which is what says the sentence above came from a lookup and not from a register
    // that lost its contents.
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": PRESENT_STREAM }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seed {seed}: the present stream still resolves, got {status} {described:?}"
    );
    let resolved = described["DeliveryStreamDescription"]["DeliveryStreamName"].as_str();
    assert_eq!(
        resolved,
        Some(PRESENT_STREAM),
        "seed {seed}: the present stream still resolves, got {described:?}"
    );
}

#[tokio::test]
async fn a_name_past_the_modelled_bound_is_refused_by_the_boundary_and_not_by_the_lookup() {
    let seed: u64 = rand::random();
    let mut rng = StdRng::seed_from_u64(seed);
    // One character past `@length`'s upper bound: outside the class the sentence above
    // covers, so the request never reaches a lookup at all.
    let too_long = drawn_name(&mut rng, MAX_NAME_LENGTH + 1);

    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    for probe in control_plane_probes() {
        let not_found = probe.wording.sentence(account);
        let case = format!(
            "seed {seed}: {} of a {}-character name",
            probe.case,
            too_long.len()
        );
        let (status, refused) = emulator
            .call(probe.operation, (probe.request)(&too_long))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected \"ValidationException\", got {code:?} {refused:?}"
        );
        let message = error_message(&refused);
        assert_ne!(
            message, not_found,
            "case {case:?}: a value the boundary refuses must not be answered as a \
             resolved-nothing lookup, got {message:?}"
        );
    }
}
