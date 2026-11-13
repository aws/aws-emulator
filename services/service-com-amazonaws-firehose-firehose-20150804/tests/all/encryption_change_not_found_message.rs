// SPDX-License-Identifier: CC0-1.0

//! The sentence an ENCRYPTION CHANGE answers with when it resolves no stream.
//!
//! The rule under test: an operation's not-found sentence is a property of the path it is
//! on, not of the service, so the two encryption changes
//! (`StartDeliveryStreamEncryption`, `StopDeliveryStreamEncryption`) state the account the
//! lookup was scoped to in their own wording — "Firehose under accountId `<account>` not
//! found", the noun `accountId` and no terminal period — and never in the control-plane
//! reads' or the data plane's. The account is the emulator's canonical one, filled from
//! the environment rather than from the request, and the probed name is not stated at all.
//!
//! Asserted over a SET of inputs: both operations, each request shape the model admits for
//! them, and names differing in length, alphabet and resemblance to a name that exists.
//! The recorded case (one stop-encryption of one absent name) is one row. The probe table
//! and the three wordings are the sibling suite's, so this suite and the ones that assert
//! the other two paths cannot drift apart.
//!
//! A positive control closes the loop: the same two calls on a stream that DOES exist are
//! served, and `DescribeDeliveryStream` reports what each one wrote — which is what says
//! the refusals above came from the lookup and not from the encryption path as such.
//!
//! Every request goes through the real router, so what is asserted is the body a client
//! reads. The harness is the lifecycle suite's.

use serde_json::json;

use super::delivery_stream_encryption::described_encryption;
use super::delivery_stream_lifecycle::{error_code, error_message, Emulator};
use super::stream_not_found_message::{control_plane_probes, NotFoundWording, Probe};

/// The name of the one stream that DOES exist while the refusals are asserted, so each
/// one is a lookup that resolved nothing rather than an empty register.
const PRESENT_STREAM: &str = "encryption-present";

/// Names differing in every way a `DeliveryStreamName` can — its 1-character `@length`
/// minimum, its 64-character maximum, the characters of its `@pattern` alphabet, and how
/// closely it resembles the name that exists. None of them occurs inside the expected
/// sentence, so the "never echoed" assertion below means what it says.
fn absent_names() -> Vec<String> {
    vec![
        "q".to_string(),
        "ghost-stream".to_string(),
        "ghost.stream".to_string(),
        "GHOST_1".to_string(),
        format!("{PRESENT_STREAM}-2"),
        "z".repeat(64),
    ]
}

/// The encryption-change rows of the shared probe table: every request shape those two
/// operations are probed with.
fn encryption_probes() -> Vec<Probe> {
    control_plane_probes()
        .into_iter()
        .filter(|probe| probe.wording == NotFoundWording::EncryptionChange)
        .collect()
}

#[tokio::test]
async fn an_encryption_change_on_an_absent_stream_states_the_account_in_its_own_wording() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let expected = NotFoundWording::EncryptionChange.sentence(account);
    // The sentences the OTHER two paths word this same fact in: neither may be answered
    // here.
    let other_paths = [
        NotFoundWording::ControlPlaneRead.sentence(account),
        NotFoundWording::DataPlane.sentence(account),
    ];

    emulator.create_stream(PRESENT_STREAM, None).await;
    emulator.await_active(PRESENT_STREAM).await;

    let probes = encryption_probes();
    assert!(
        probes.len() >= 2,
        "both encryption changes are probed, got {} rows",
        probes.len()
    );

    for probe in &probes {
        for name in absent_names() {
            let case = format!("{} of {name:?}", probe.case);
            let (status, refused) = emulator.call(probe.operation, (probe.request)(&name)).await;
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
            for other in &other_paths {
                assert_ne!(
                    &message, other,
                    "case {case:?}: another path's sentence must not stand in for this \
                     one, got {message:?}"
                );
            }
            assert!(
                !message.contains(&name),
                "case {case:?}: the probed name must not be echoed, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_same_two_calls_are_served_for_a_stream_that_exists() {
    let emulator = Emulator::new();
    emulator.create_stream(PRESENT_STREAM, None).await;
    emulator.await_active(PRESENT_STREAM).await;

    // Enable: the call returns immediately and leaves the transient status the clock owes
    // `ENABLED`, which the describe reports.
    let (status, started) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": PRESENT_STREAM }),
        )
        .await;
    assert_eq!(status, 200, "start encryption: got {started:?}");
    let enabling = described_encryption(&emulator, PRESENT_STREAM).await;
    assert_eq!(
        enabling["Status"],
        json!("ENABLING"),
        "right after the start: got {enabling:?}"
    );
    // "Firehose first sets the encryption status of the stream to ENABLING, and then to
    // ENABLED": the stop below needs the enable settled, so the test waits for the
    // status the clock owes rather than for a span.
    emulator
        .await_reported(
            PRESENT_STREAM,
            "DeliveryStreamDescription/DeliveryStreamEncryptionConfiguration/Status",
            "ENABLED",
        )
        .await;

    // Disable: served for the same stream, and reported the same way.
    let (status, stopped) = emulator
        .call(
            "StopDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": PRESENT_STREAM }),
        )
        .await;
    assert_eq!(status, 200, "stop encryption: got {stopped:?}");
    let disabling = described_encryption(&emulator, PRESENT_STREAM).await;
    assert_eq!(
        disabling["Status"],
        json!("DISABLING"),
        "right after the stop: got {disabling:?}"
    );
}
