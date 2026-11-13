// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two secrets this service GENERATES for a caller — the
//! `passphrase` of a channel's `srt` block and the last field of a stream key's `value` —
//! driven over the wire through `MultiServiceRouter::dispatch` on the harness
//! `channel_lifecycle` owns.
//!
//! The rule under test, exercised over a SET of creates rather than the one the issue
//! cited:
//!
//! 1. A secret this service hands out is GENERATED, not derived from the store's mint
//!    counter, the clock or the process id: two stores driven through the very same
//!    sequence of calls hand out different secrets, and no two creates in one store ever
//!    repeat one.
//! 2. Each secret carries the class the member it fills is written in: an SRT passphrase
//!    is mixed-case alphanumeric, a stream key's secret field is lowercase hexadecimal,
//!    each of one width for every channel.
//! 3. A secret is persisted with the resource that carries it, so a read answers the
//!    same secret the create reported.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{text, Ivs};

/// Hosts no address this service announces may name: the ingest endpoint is an address a
/// client takes to an encoder, so it has to be one this emulator answers on and never an
/// Amazon production host (the rule `announced_channel_endpoints.rs` covers in full).
const FOREIGN_DOMAINS: [&str; 2] = ["live-video.net", "amazonaws.com"];

/// The window SRT itself admits for a passphrase: the protocol refuses anything shorter
/// than 10 or longer than 79 characters, so a passphrase outside it is one no SRT client
/// could use. The model constrains `SrtPassphrase` neither in length nor in pattern
/// (`smithy_query(query="shape", shape="SrtPassphrase")`), so this is the only published
/// bound there is to hold the width against.
const SRT_PASSPHRASE_BOUNDS: std::ops::RangeInclusive<usize> = 10..=79;

/// The creates this suite spans: rows differ along what rule 2 generalises over — the
/// channel type, the latency mode, the container format, the ingest flags, the name and
/// the tag count — so the class being asserted is the class of EVERY create, not of one.
fn create_requests() -> Vec<(&'static str, Value)> {
    vec![
        ("every member defaulted", json!({})),
        ("name only", json!({ "name": "secret-probe" })),
        (
            "the issue's own case: a secure-ingest standard channel",
            json!({ "name": "probe", "insecureIngest": false }),
        ),
        (
            "insecure ingest allowed",
            json!({ "name": "insecure_1", "insecureIngest": true }),
        ),
        (
            "basic, normal latency",
            json!({ "name": "basic-normal", "type": "BASIC", "latencyMode": "NORMAL" }),
        ),
        (
            "advanced-sd, authorized, fragmented mp4",
            json!({ "name": "advanced-sd", "type": "ADVANCED_SD", "authorized": true,
                    "containerFormat": "FRAGMENTED_MP4" }),
        ),
        (
            "advanced-hd with an explicit preset and several tags",
            json!({ "name": "advanced-hd", "type": "ADVANCED_HD",
                    "preset": "CONSTRAINED_BANDWIDTH_DELIVERY",
                    "tags": { "env": "prod", "team": "video" } }),
        ),
        ("an empty name", json!({ "name": "" })),
        (
            "a 128-character name, the modelled upper bound",
            json!({ "name": "n".repeat(128) }),
        ),
    ]
}

/// Whether every character of `value` is one `alphabet` admits, and `value` is not empty.
fn written_in(value: &str, alphabet: impl Fn(char) -> bool) -> bool {
    !value.is_empty() && value.chars().all(alphabet)
}

fn is_lowercase_hex(character: char) -> bool {
    character.is_ascii_digit() || ('a'..='f').contains(&character)
}

/// Rules 2 and 3 over the create table: what class each generated secret is written in,
/// that one width serves every channel, that no two creates repeat a secret, and that a
/// read answers the secret the create reported.
#[tokio::test]
async fn every_generated_ingest_secret_carries_the_class_this_service_issues() {
    let ivs = Ivs::new();
    let mut passphrases: BTreeSet<String> = BTreeSet::new();
    let mut stream_key_values: BTreeSet<String> = BTreeSet::new();
    let mut passphrase_widths: BTreeSet<usize> = BTreeSet::new();
    let mut secret_widths: BTreeSet<usize> = BTreeSet::new();
    // Every character every passphrase of the run is written with, so the ALPHABET the
    // class draws on is asserted over the sample and not guessed from one value.
    let mut passphrase_alphabet: BTreeSet<char> = BTreeSet::new();

    for (case, request) in create_requests() {
        let created = ivs.create(request).await;
        let channel = &created["channel"];
        let srt = &channel["srt"];
        let passphrase = text(srt, "passphrase");

        // Rule 2: the class of an SRT passphrase.
        assert!(
            written_in(&passphrase, |character| character.is_ascii_alphanumeric()),
            "case {case:?}: the srt passphrase must be written in alphanumerics, got \
             {passphrase:?}"
        );
        assert!(
            SRT_PASSPHRASE_BOUNDS.contains(&passphrase.chars().count()),
            "case {case:?}: the srt passphrase must be a width SRT admits \
             ({SRT_PASSPHRASE_BOUNDS:?} characters), got {passphrase:?}"
        );
        assert!(
            !written_in(&passphrase, is_lowercase_hex),
            "case {case:?}: the srt passphrase must be an opaque secret over the \
             mixed-case alphanumerics, not a lowercase-hex digest, got {passphrase:?}"
        );
        passphrase_widths.insert(passphrase.chars().count());
        passphrase_alphabet.extend(passphrase.chars());
        assert!(
            passphrases.insert(passphrase.clone()),
            "case {case:?}: no two channels may share an srt passphrase, got \
             {passphrase:?} twice"
        );

        // The endpoint the passphrase goes with: the channel's own ingest endpoint, which
        // is this deployment's address and not a secret.
        let ingest_endpoint = text(channel, "ingestEndpoint");
        assert_eq!(
            text(srt, "endpoint"),
            ingest_endpoint,
            "case {case:?}: srt.endpoint must be the channel's ingest endpoint, got {srt}"
        );
        for domain in FOREIGN_DOMAINS {
            assert!(
                !ingest_endpoint.is_empty() && !ingest_endpoint.ends_with(domain),
                "case {case:?}: the ingest endpoint must be an address this emulator \
                 answers on, not a {domain} host, got {ingest_endpoint:?}"
            );
        }

        // Rule 2 for the other secret this create hands out: `sk_<region>_<id>_<secret>`,
        // whose last field is the generated one.
        let value = text(&created["streamKey"], "value");
        let fields: Vec<&str> = value.split('_').collect();
        assert_eq!(
            fields.len(),
            4,
            "case {case:?}: a stream key value must read sk_<region>_<id>_<secret>, got \
             {value:?}"
        );
        assert!(
            fields[0] == "sk" && !fields[1].is_empty(),
            "case {case:?}: a stream key value must name this service and the region, \
             got {value:?}"
        );
        assert!(
            written_in(fields[3], is_lowercase_hex),
            "case {case:?}: a stream key's secret field must be written in lowercase \
             hexadecimal, got {value:?}"
        );
        secret_widths.insert(fields[3].chars().count());
        assert!(
            stream_key_values.insert(value.clone()),
            "case {case:?}: no two creates may mint the same stream key value, got \
             {value:?} twice"
        );

        // Rule 3: both secrets read back.
        let arn = text(channel, "arn");
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            text(&fetched["channel"]["srt"], "passphrase"),
            passphrase,
            "case {case:?}: GetChannel must answer the passphrase the create reported, \
             got {}",
            fetched["channel"]["srt"]
        );
        let key_arn = text(&created["streamKey"], "arn");
        let read_key = ivs.ok("GetStreamKey", json!({ "arn": key_arn })).await;
        assert_eq!(
            text(&read_key["streamKey"], "value"),
            value,
            "case {case:?}: GetStreamKey must answer the value the create reported, got {}",
            read_key["streamKey"]
        );
    }

    // One width per member, for every channel in the table.
    assert_eq!(
        passphrase_widths.len(),
        1,
        "every channel's srt passphrase must have the one width this service issues, got \
         widths {passphrase_widths:?}"
    );
    assert_eq!(
        secret_widths.len(),
        1,
        "every stream key's secret field must have the one width this service issues, got \
         widths {secret_widths:?}"
    );
    // The class is MIXED-case alphanumeric, so the sample must span all three groups; a
    // hex or single-case generator cannot.
    for (group, admits) in [
        ("uppercase letters", char::is_ascii_uppercase as fn(&char) -> bool),
        ("lowercase letters", char::is_ascii_lowercase),
        ("digits", char::is_ascii_digit),
    ] {
        assert!(
            passphrase_alphabet.iter().any(admits),
            "the passphrases of this run must draw on {group}, but their whole alphabet \
             was {passphrase_alphabet:?}"
        );
    }
}

/// Rule 1: the secrets are generated, so nothing about the sequence of calls predicts
/// them — two stores driven through the identical calls agree on every member the request
/// decides and differ on every secret.
#[tokio::test]
async fn the_same_calls_against_two_stores_hand_out_different_secrets() {
    let first = Ivs::new();
    let second = Ivs::new();

    for (case, request) in create_requests() {
        let here = first.create(request.clone()).await;
        let there = second.create(request).await;

        // What the request decides is the same on both sides, so the comparison below is
        // of two answers to one call and not of two different channels.
        for member in ["name", "type", "latencyMode", "preset", "containerFormat"] {
            assert_eq!(
                text(&here["channel"], member),
                text(&there["channel"], member),
                "case {case:?}: {member} is decided by the request, so both stores must \
                 answer the same value, got {} and {}",
                here["channel"],
                there["channel"]
            );
        }

        let (here_passphrase, there_passphrase) = (
            text(&here["channel"]["srt"], "passphrase"),
            text(&there["channel"]["srt"], "passphrase"),
        );
        assert_ne!(
            here_passphrase, there_passphrase,
            "case {case:?}: the same call against a second store must not hand out the \
             same srt passphrase, got {here_passphrase:?} twice"
        );
        let (here_value, there_value) = (
            text(&here["streamKey"], "value"),
            text(&there["streamKey"], "value"),
        );
        assert_ne!(
            here_value, there_value,
            "case {case:?}: the same call against a second store must not hand out the \
             same stream key value, got {here_value:?} twice"
        );
    }
}

/// Rules 1 and 2 on the OTHER create that hands out a secret: `CreateStreamKey`, over the
/// rotations a client performs by deleting a channel's key and asking for another.
#[tokio::test]
async fn a_rotated_stream_key_is_a_fresh_secret_of_the_same_class() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "rotating" })).await;
    let channel_arn = text(&created["channel"], "arn");
    let mut seen = BTreeSet::from([text(&created["streamKey"], "value")]);
    let mut current = created["streamKey"].clone();

    // Rows: each rotation of the one key the channel's quota allows.
    for rotation in 1..=4 {
        let case = format!("rotation {rotation}");
        let retired = ivs
            .call("DeleteStreamKey", &json!({ "arn": text(&current, "arn") }))
            .await;
        assert_eq!(
            retired.status, 204,
            "case {case:?}: deleting the key must free the channel's quota slot, got {} {}",
            retired.status, retired.body
        );
        let replacement = ivs
            .ok("CreateStreamKey", json!({ "channelArn": channel_arn }))
            .await;
        current = replacement["streamKey"].clone();

        let value = text(&current, "value");
        let secret = value.split('_').next_back().unwrap_or_default();
        assert!(
            written_in(secret, is_lowercase_hex),
            "case {case:?}: a rotated key's secret field must be written in lowercase \
             hexadecimal, got {value:?}"
        );
        assert!(
            seen.insert(value.clone()),
            "case {case:?}: a rotation must hand out a secret no earlier key had, got \
             {value:?} twice"
        );
    }
}
