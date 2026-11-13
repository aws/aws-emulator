// SPDX-License-Identifier: CC0-1.0

//! WHICH ROLE NAMES the member's own `@pattern` admits after `role/`, over the wire.
//!
//! THE RULE UNDER TEST: `channelRoleArn` is taken off the request boundary on both
//! operations that write it, so the handler applies the member's WHOLE declared form, and a
//! value that form does not admit is refused with the modelled `AccessDeniedException` (HTTP
//! 403) and recorded nowhere. The form is
//! `^arn:aws:iam::[0-9]{12}:role/(.+)$`
//! (`smithy_query(query="shape",shape="roleArn")`), a Smithy `@pattern` and therefore an
//! ECMA-262 expression: its `.` matches every character EXCEPT a line terminator — line feed,
//! carriage return, line separator U+2028 and paragraph separator U+2029 — so a value
//! carrying one of those cannot be matched by `(.+)` anchored with `$`, whatever else it says,
//! while every other character is one `.` matches and so one the member admits.
//!
//! Why the handler has to be the layer that says so: the SAME shape, with the same
//! `@pattern`, is an OUTPUT member — `UpdateSlackChannelConfiguration` answers with
//! `channelRoleArn` and `ListSlackChannelConfigurations` reports it inside every
//! `SlackChannelConfiguration` — so a value the pattern does not admit could not be reported
//! back through the shape it was stored in. Both read paths are therefore driven here on an
//! admitted value, and both are shown to report nothing at all for a refused one.
//!
//! The tables are that rule, not one recorded request. The refused rows are the cross product
//! of every line terminator the pattern's `.` excludes with every place one can sit in an
//! otherwise well-formed value — immediately after `role/`, inside the role name, at its end,
//! as the whole of it, between the segments of a role under a path, and inside the ARN prefix
//! — and are driven on BOTH writes. The admitted rows are the boundary of the rule: role
//! names carrying the characters that are whitespace or control to a reader but are NOT line
//! terminators (tab, vertical tab, form feed, the next-line character U+0085, a plain space),
//! plus a non-ASCII name, each served and reported back verbatim.
//!
//! Values that leave the member's form in some OTHER way — no ARN at all, another service's
//! ARN, an account segment that is not twelve digits, an empty role name — are the same
//! refusal and live in [`crate::channel_role_pass_role_form`]; the account the ARN names is
//! [`crate::channel_role_account`].

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::CROSS_ACCOUNT_PASS_ROLE_MESSAGE;

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// The `(teamId, channelId)` every row below configures.
const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";

/// The sentence AWS answers a refused pass-role attempt with, written out here so the WORDING
/// is pinned by this suite rather than read back off the code under test.
const AWS_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The characters a modelled `@pattern`'s `.` does NOT match, from ECMA-262: line feed,
/// carriage return, line separator (U+2028) and paragraph separator (U+2029). A role name
/// carrying one of them is outside `(.+)` however long it is.
const LINE_TERMINATORS: [(&str, char); 4] = [
    ("a line feed", '\n'),
    ("a carriage return", '\r'),
    ("the line separator U+2028", '\u{2028}'),
    ("the paragraph separator U+2029", '\u{2029}'),
];

/// Where in an otherwise well-formed role ARN of the caller's own account a line terminator
/// sits. Each entry turns the terminator into a whole value, so the cross product below spans
/// both axes the rule generalises over: WHICH terminator, and WHERE it is.
fn placements(terminator: char) -> Vec<(&'static str, String)> {
    vec![
        (
            "immediately after role/",
            role_arn(&format!("{terminator}SupportAppRole")),
        ),
        (
            "inside the role name",
            role_arn(&format!("Support{terminator}AppRole")),
        ),
        (
            "at the end of the role name",
            role_arn(&format!("SupportAppRole{terminator}")),
        ),
        ("as the whole role name", role_arn(&terminator.to_string())),
        (
            "between the segments of a role under a path",
            role_arn(&format!("service-role{terminator}/SupportAppRole")),
        ),
        (
            "inside the ARN prefix",
            format!(
                "arn:aws:iam::{account}{terminator}:role/SupportAppRole",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ]
}

/// Every refused value: each line terminator in each placement.
fn values_outside_the_members_pattern() -> Vec<(String, String)> {
    LINE_TERMINATORS
        .iter()
        .flat_map(|(terminator_case, terminator)| {
            placements(*terminator)
                .into_iter()
                .map(move |(placement, submitted)| {
                    (format!("{terminator_case} {placement}"), submitted)
                })
        })
        .collect()
}

/// Role names made of characters the pattern's `.` DOES match, including the ones a reader
/// would call whitespace or control but ECMA-262 does not call line terminators. Each is a
/// role of the caller's own account, so each is served and reported back verbatim.
fn role_names_the_pattern_admits() -> Vec<(&'static str, String)> {
    vec![
        ("a tab inside the role name", "Support\tAppRole".to_string()),
        (
            "a vertical tab inside the role name",
            "Support\u{000B}AppRole".to_string(),
        ),
        (
            "a form feed inside the role name",
            "Support\u{000C}AppRole".to_string(),
        ),
        (
            "the next-line character U+0085 inside the role name",
            "Support\u{0085}AppRole".to_string(),
        ),
        ("a space inside the role name", "Support App".to_string()),
        (
            "a non-ASCII role name",
            "Suppörtrolle\u{1F600}".to_string(),
        ),
        (
            "the IAM role-name punctuation class",
            "Support+=,.@-Role".to_string(),
        ),
    ]
}

/// A create body carrying `channel_role_arn`, with everything else the model requires.
fn create_body(channel_role_arn: &str) -> Value {
    json!({
        "teamId": TEAM_ID,
        "channelId": CHANNEL_ID,
        "channelName": "aws-support",
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": channel_role_arn,
    })
}

/// Require `reply` to be the modelled pass-role refusal: `AccessDeniedException`, HTTP 403,
/// worded as AWS words that refusal.
fn assert_pass_role_refusal(case: &str, submitted: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 403,
        "case {case:?} ({submitted:?}): expected HTTP 403, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "AccessDeniedException",
        "case {case:?} ({submitted:?}): expected AccessDeniedException, got {:?} / {}",
        reply.error_type, reply.body
    );
    let message = reply.string("message").unwrap_or_else(|| {
        panic!(
            "case {case:?} ({submitted:?}): the refusal carries no message: {}",
            reply.body
        )
    });
    assert_eq!(
        message, AWS_PASS_ROLE_MESSAGE,
        "case {case:?} ({submitted:?}): expected {AWS_PASS_ROLE_MESSAGE:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_create_whose_role_name_the_pattern_rejects_is_the_pass_role_refusal() {
    // The crate's own wording for this refusal is the sentence AWS answers with, so a drift in
    // it fails here rather than passing unnoticed through the rows below.
    assert_eq!(
        CROSS_ACCOUNT_PASS_ROLE_MESSAGE, AWS_PASS_ROLE_MESSAGE,
        "case \"the refusal's wording\": expected {AWS_PASS_ROLE_MESSAGE:?}, got \
         {CROSS_ACCOUNT_PASS_ROLE_MESSAGE:?}"
    );
    for (case, submitted) in values_outside_the_members_pattern() {
        let emulator = Emulator::new();
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_pass_role_refusal(&case, &submitted, &reply);

        // Nothing was recorded, so neither read path can report a value the member's own
        // `@pattern` does not admit.
        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?} ({submitted:?}): expected no configuration to be recorded, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_update_onto_a_role_name_the_pattern_rejects_is_the_pass_role_refusal() {
    let stored = role_arn("SupportAppRole");
    for (case, submitted) in values_outside_the_members_pattern() {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&stored))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the fixture create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );

        let reply = emulator
            .update(json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelRoleArn": submitted,
            }))
            .await;
        assert_pass_role_refusal(&case, &submitted, &reply);

        // The refused update left the stored role exactly as the create recorded it.
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to survive"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(stored.as_str()),
            "case {case:?} ({submitted:?}): expected the stored role ARN to be unchanged, got \
             {listed}"
        );
    }
}

#[tokio::test]
async fn a_role_name_the_pattern_admits_round_trips_on_both_writes() {
    for (case, role_name) in role_names_the_pattern_admits() {
        let submitted = role_arn(&role_name);
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({submitted:?}): expected create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to be reported"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(submitted.as_str()),
            "case {case:?}: expected the created role ARN to be reported, got {listed}"
        );

        // And the other read path of the same member: the update answers with the value it
        // persisted, re-pointed at another admitted role name of the same account.
        let repointed = role_arn(&format!("Repointed\t{role_name}"));
        let updated = emulator
            .update(json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelRoleArn": repointed,
            }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?} ({repointed:?}): expected update to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("channelRoleArn"),
            Some(repointed.clone()),
            "case {case:?}: expected the update to answer with the new role ARN, got {}",
            updated.body
        );
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to survive"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(repointed.as_str()),
            "case {case:?}: expected the updated role ARN to be persisted, got {listed}"
        );
    }
}
