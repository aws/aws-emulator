// SPDX-License-Identifier: CC0-1.0

//! The space-delimited filter-pattern ALIGNMENT, asserted against the matcher itself
//! (`filter_pattern::FilterPattern`) rather than over the wire.
//!
//! RULE: a `[ … ]` field list of N fields describes an event of AT LEAST N fields — the slots
//! name the event's leading fields, in order, and a field past the last slot is one the list
//! says nothing about — while an event carrying FEWER fields than the list names leaves a slot
//! with no field to align to, so the list does not describe it. The `...` wildcard is
//! unaffected: the slots before it still count from the event's start and the slots after it
//! from its end.
//!
//! The rows span what that rule generalises over: the list's field count against the event's
//! (fewer, equal, more, far more), whether a slot carries a condition, whether the condition
//! lands on a leading field or on one the wildcard aligned from the end, and whether a field of
//! the event is a `[…]` or `"…"` GROUP rather than a bare run of characters. The wire-level
//! counterpart — the same rule as `TestMetricFilter` and an ingested batch's metric filter
//! report it — is in [`crate::leading_field_alignment_and_document_quotas`].

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::filter_pattern::FilterPattern;

/// A space-delimited log event of the form the syntax documents: seven fields, two of which are
/// GROUPS — a `[…]` run and a `"…"` run — each of which is ONE field.
const APACHE_EVENT: &str = "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700] \
                            \"GET /apache_pb.gif HTTP/1.0\" 200 2326";

/// Which events each list describes.
#[test]
fn a_list_describes_an_event_of_at_least_as_many_fields() {
    // (case, the pattern, the event, whether the pattern describes it)
    let rows: [(&str, &str, &str, bool); 12] = [
        (
            "three slots, four fields, the fourth a bracket group",
            "[ip, identity, user_id]",
            "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700]",
            true,
        ),
        (
            "three slots, exactly three fields",
            "[ip, identity, user_id]",
            "127.0.0.1 - frank",
            true,
        ),
        (
            "three slots, two fields",
            "[ip, identity, user_id]",
            "127.0.0.1 -",
            false,
        ),
        (
            "three slots, one field",
            "[ip, identity, user_id]",
            "127.0.0.1",
            false,
        ),
        (
            "three slots, seven fields",
            "[ip, identity, user_id]",
            APACHE_EVENT,
            true,
        ),
        (
            "one slot, seven fields",
            "[ip]",
            APACHE_EVENT,
            true,
        ),
        (
            "seven slots, exactly seven fields",
            "[ip, user, username, timestamp, request, status_code, bytes]",
            APACHE_EVENT,
            true,
        ),
        (
            "seven slots, three fields",
            "[ip, user, username, timestamp, request, status_code, bytes]",
            "one two three",
            false,
        ),
        (
            "a condition on a leading field the event satisfies",
            "[ip, identity, user_id=frank]",
            APACHE_EVENT,
            true,
        ),
        (
            "a condition on a leading field the event does not satisfy",
            "[ip, identity, user_id=anna]",
            APACHE_EVENT,
            false,
        ),
        (
            "the wildcard aligns the slots after it to the event's end",
            "[..., status_code, bytes]",
            APACHE_EVENT,
            true,
        ),
        (
            "a numeric condition on a field the wildcard aligned from the end",
            "[..., status_code, bytes>10000]",
            APACHE_EVENT,
            false,
        ),
    ];

    for (case, pattern, message, describes) in rows {
        let parsed = FilterPattern::parse(pattern)
            .unwrap_or_else(|refusal| panic!("case {case}: {pattern:?} parses, got {refusal}"));
        let matched = parsed.match_message(message);
        assert_eq!(
            matched.is_some(),
            describes,
            "case {case}: {pattern:?} over {message:?}: expected describes={describes}, got \
             {matched:?}"
        );
    }
}

/// What a described event yields: the leading fields the list named, under the keys that select
/// them — so a field past the last slot is left out rather than folded into it.
#[test]
fn the_values_extracted_are_the_fields_the_list_named() {
    let parsed = FilterPattern::parse("[ip, identity, user_id]").expect("the pattern parses");
    let extracted = parsed
        .match_message(APACHE_EVENT)
        .expect("the list describes an event of more fields");
    let reported: Vec<(&str, &str)> = extracted
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        reported,
        vec![
            ("$identity", "-"),
            ("$ip", "127.0.0.1"),
            ("$user_id", "frank"),
        ],
        "case \"three slots over seven fields\": expected the three leading fields, got \
         {reported:?}"
    );
}
