// SPDX-License-Identifier: CC0-1.0

//! How the bound a refusal is stated within is MEASURED, end to end over HTTP.
//!
//! RULE under test: the `@length` of a modelled member counts CHARACTERS, not
//! bytes — Smithy measures a string's length in characters — so a reason this
//! service brings within the 2048 of `ErrorMessage`
//! ([`crate::error_message_bounds::ERROR_MESSAGE_MAX`]) is cut by characters: a
//! reason naming non-ASCII text is reported at exactly that many characters even
//! though it carries more bytes, and it is never cut mid-character.
//!
//! `tests/all/unbounded_refusal_message_bounds.rs` asserts the bound itself over
//! the operations and members that reach it; this suite is about the UNIT the cut
//! is counted in, which only text outside ASCII can tell apart, and which every
//! one of those reasons depends on. What the rule asserts about a reported reason
//! comes from `tests/all/error_message_bounds.rs`
//! ([`crate::error_message_bounds::assert_within_bound`]), so this crate states
//! it once.
//!
//! The rows differ along what the rule generalises over: WHICH operation reports
//! the reason (a tagging call and a create, which build the same reason in
//! different places) and HOW WIDE the characters the request carried are (two
//! bytes each, and four — a letter outside the basic multilingual plane).

use serde_json::{json, Value};

use crate::error_message_bounds::{assert_within_bound, ERROR_MESSAGE_MAX};
use crate::harness::{workspace_arn, Emulator, Resp};
use crate::unbounded_refusal_message_bounds::{reserved_keys_padded, reserved_keys_reason};

/// The workspace the create case names. The reserved-key rule is decided on the
/// request alone, before the store is reached, so no row depends on the
/// workspace existing.
const WORKSPACE: &str = "widemsgbounds";

/// The greatest number of keys the modelled `TagMap` admits (`@length(0..50)`),
/// from `smithy_query(query="shape",shape="TagMap")`.
const TAG_KEYS_MAX: usize = 50;

/// How many characters each key below carries: within what `TagKey` admits
/// (`@length(1..128)`), and enough that the set of keys the rows name renders a
/// reason past the maximum of the member it travels on.
const KEY_CHARS: usize = 48;

/// One operation that refuses the reserved tag keys a request carried, naming
/// them.
#[derive(Debug, Clone, Copy)]
enum TagWriter {
    /// `TagResource`, which refuses the keys before it resolves the resource.
    Tag,
    /// `CreateComponentType`, which refuses them before anything is created.
    Create,
}

/// Both places the same reason is built.
const TAG_WRITERS: [TagWriter; 2] = [TagWriter::Tag, TagWriter::Create];

impl TagWriter {
    /// Drive this operation with `tags`.
    async fn send(self, emulator: &Emulator, tags: Value) -> Resp {
        match self {
            TagWriter::Tag => emulator.tag_resource(&workspace_arn(WORKSPACE), tags).await,
            TagWriter::Create => {
                emulator
                    .create_component_type(WORKSPACE, "com.example.wide", json!({"tags": tags}))
                    .await
            }
        }
    }
}

/// RULE: a reason too long for the member it travels on is reported at exactly
/// the member's maximum in CHARACTERS, keeping whole characters, however many
/// bytes those characters take.
#[tokio::test]
async fn a_cut_reason_is_measured_in_characters() {
    let emulator = Emulator::new();

    // The rows differ along how wide the characters are: a two-byte letter and a
    // four-byte one. Both are letters the modelled `TagKey` pattern admits
    // (`[\p{L}\p{Z}\p{N}_.:/=+\-@]*`), from
    // `smithy_query(query="shape",shape="TagKey")`.
    for (size_case, filler) in [("two-byte letters", 'é'), ("four-byte letters", '𝔄')] {
        let keys = reserved_keys_padded(TAG_KEYS_MAX, KEY_CHARS, filler);
        // The reason BEFORE the bound is applied, worded by the sibling suite
        // that owns this construction.
        let whole = reserved_keys_reason("tags", &keys);
        assert!(
            whole.chars().count() > ERROR_MESSAGE_MAX,
            "case {size_case:?}: the row must name more characters than the member admits, got {}",
            whole.chars().count()
        );
        let tags = Value::Object(
            keys.iter()
                .map(|key| (key.clone(), json!("whoever")))
                .collect(),
        );

        for writer in TAG_WRITERS {
            let case = format!("{writer:?} with {size_case}");
            let refused = writer.send(&emulator, tags.clone()).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {refused:?}"
            );
            let reported = refused
                .member("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_within_bound(&case, &reported, &whole, ERROR_MESSAGE_MAX);
            // The cut is counted in characters, so the reported reason carries
            // MORE bytes than the member's maximum while carrying exactly that
            // many characters — the statement a row of ASCII text cannot make.
            assert!(
                reported.len() > ERROR_MESSAGE_MAX,
                "case {case:?}: a reason of multi-byte characters reported at the \
                 {ERROR_MESSAGE_MAX}-character maximum carries more bytes than that, got {} \
                 bytes: {reported:?}",
                reported.len()
            );
        }
    }
}
