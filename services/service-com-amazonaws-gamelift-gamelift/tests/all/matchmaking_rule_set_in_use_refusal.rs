// SPDX-License-Identifier: CC0-1.0

//! The refusal `DeleteMatchmakingRuleSet` answers while a matchmaking configuration still
//! references the rule set, over the wire.
//!
//! The rule this suite pins: that delete is refused with HTTP 400 and the modelled
//! `InvalidRequestException`, and its `Message` is the service's ONE sentence for that
//! class — the same string for every such request, with nothing of the request (the name
//! or the ARN it supplied) and nothing of the service's state (which configurations
//! reference the rule set, or how many) interpolated into it. The refused delete removes
//! nothing: the rule set is still describable, and the same delete succeeds once no
//! configuration references it any more.
//!
//! The table rows differ along what the rule generalises over — the form and length of the
//! rule set name, which identifier form the delete names it by, how many configurations
//! reference it, and whether the reference was taken up by the configuration's CREATE or
//! later by an UPDATE — so the recorded case (one configuration created on a hyphenated
//! name, deleted by that name) is one row among many.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `gamelift_arn`) is the one
//! `hosting_loop.rs` drives the production dispatch path with.

use serde_json::json;

use crate::hosting_loop::{error_code, error_message, gamelift_arn, GameLift};

/// The sentence the service answers a delete of a rule set a matchmaking configuration
/// still uses with. The whole text is fixed: it is the refusal's own wording, not a report
/// on the request or on the store.
const RULE_SET_IN_USE: &str = "Matchmaking rule set is still in use, cannot delete";

/// The modelled maximum length of `CreateMatchmakingRuleSetInput$Name`
/// (`smithy_query shape=MatchmakingIdStringModel`: length 0..128).
const MAX_RULE_SET_NAME_LENGTH: usize = 128;

/// The ARN a rule set named `name` is assigned: `MatchmakingRuleSet$RuleSetArn`'s doc —
/// "Format is `arn:aws:gamelift:<region>::matchmakingruleset/<ruleset name>`".
fn rule_set_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingruleset/{name}"))
}

/// A body that conforms to the FlexMatch rule language, so every create below is accepted
/// for what it asks for rather than refused for its body.
fn conformant_body() -> String {
    json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// One row: the rule set the delete names, how it names it, and how the references to it
/// came about.
struct Case {
    label: &'static str,
    rule_set: String,
    /// How many matchmaking configurations reference the rule set when the delete is sent.
    referencing: usize,
    /// Whether the delete names the rule set by its ARN rather than by its name.
    by_arn: bool,
    /// Whether the referencing configurations were created on ANOTHER rule set and moved
    /// onto this one by `UpdateMatchmakingConfiguration`.
    via_update: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one configuration created on a hyphenated name, deleted by that name",
            rule_set: "mmdel-inuse-rules".to_string(),
            referencing: 1,
            by_arn: false,
            via_update: false,
        },
        Case {
            label: "one configuration, the rule set deleted by its ARN",
            rule_set: "arn.named.rules".to_string(),
            referencing: 1,
            by_arn: true,
            via_update: false,
        },
        Case {
            label: "three configurations on one rule set, deleted by its name",
            rule_set: "shared-by-three".to_string(),
            referencing: 3,
            by_arn: false,
            via_update: false,
        },
        Case {
            label: "seven configurations on one rule set, deleted by its ARN",
            rule_set: "shared-by-seven".to_string(),
            referencing: 7,
            by_arn: true,
            via_update: false,
        },
        Case {
            label: "a reference taken up by an update rather than by a create",
            rule_set: "moved-onto-rules".to_string(),
            referencing: 1,
            by_arn: false,
            via_update: true,
        },
        Case {
            label: "two configurations moved onto it by an update, deleted by its ARN",
            rule_set: "moved-onto-twice".to_string(),
            referencing: 2,
            by_arn: true,
            via_update: true,
        },
        Case {
            label: "a rule set name at the modelled maximum length",
            rule_set: "n".repeat(MAX_RULE_SET_NAME_LENGTH),
            referencing: 2,
            by_arn: false,
            via_update: false,
        },
        Case {
            label: "a one-character rule set name",
            rule_set: "0".to_string(),
            referencing: 1,
            by_arn: false,
            via_update: false,
        },
    ]
}

/// The create request for a matchmaking configuration named `name` on rule set `rule_set`.
/// `STANDALONE` is the `FlexMatchMode` that needs no game session queue, so the row is
/// about the rule set reference and nothing else.
fn configuration_request(name: &str, rule_set: &str) -> serde_json::Value {
    json!({
        "Name": name,
        "RuleSetName": rule_set,
        "RequestTimeoutSeconds": 30,
        "AcceptanceRequired": false,
        "FlexMatchMode": "STANDALONE",
    })
}

#[tokio::test]
async fn deleting_a_rule_set_a_configuration_uses_is_refused_with_the_one_in_use_sentence() {
    for case in cases() {
        let label = case.label;
        let gamelift = GameLift::new();

        gamelift
            .ok(
                "CreateMatchmakingRuleSet",
                json!({"Name": &case.rule_set, "RuleSetBody": conformant_body()}),
            )
            .await;

        // The configurations that reference it: created on it directly, or created on a
        // rule set of their own and moved onto it afterwards.
        let staging = "staging.rules".to_string();
        if case.via_update {
            gamelift
                .ok(
                    "CreateMatchmakingRuleSet",
                    json!({"Name": &staging, "RuleSetBody": conformant_body()}),
                )
                .await;
        }
        let mut configurations: Vec<String> = Vec::new();
        for index in 0..case.referencing {
            let name = format!("using-matchmaker-{index}");
            let created_on = if case.via_update {
                staging.clone()
            } else {
                case.rule_set.clone()
            };
            gamelift
                .ok(
                    "CreateMatchmakingConfiguration",
                    configuration_request(&name, &created_on),
                )
                .await;
            if case.via_update {
                gamelift
                    .ok(
                        "UpdateMatchmakingConfiguration",
                        json!({"Name": &name, "RuleSetName": &case.rule_set}),
                    )
                    .await;
            }
            configurations.push(name);
        }

        let selector = if case.by_arn {
            rule_set_arn(&case.rule_set)
        } else {
            case.rule_set.clone()
        };

        let (status, parsed) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: expected 400 InvalidRequestException deleting a rule set in use, \
             got {status} {code} with body {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, RULE_SET_IN_USE,
            "case {label:?}: expected Message {RULE_SET_IN_USE:?}, got {message:?}"
        );
        // The two values the text has no slot for, asserted independently of the string
        // above: the identifier the request supplied, and any count of referencing
        // configurations (the message carries no digit at all).
        assert!(
            !message.contains(&case.rule_set) && !message.contains(&selector),
            "case {label:?}: the in-use sentence must not carry the identifier the request \
             supplied ({selector:?}), got {message:?}"
        );
        assert!(
            !message.contains(|character: char| character.is_ascii_digit()),
            "case {label:?}: the in-use sentence must not carry a reference count, got \
             {message:?}"
        );

        // The refused delete removed nothing, and neither did reading the refusal: the rule
        // set is still served, with the body it was created with.
        for attempt in 1..=2 {
            let described = gamelift
                .ok(
                    "DescribeMatchmakingRuleSets",
                    json!({"Names": [&case.rule_set]}),
                )
                .await;
            assert_eq!(
                described["RuleSets"][0]["RuleSetName"],
                json!(&case.rule_set),
                "case {label:?}: read {attempt} after the refused delete must still serve the \
                 rule set, got {described}"
            );
            assert_eq!(
                described["RuleSets"][0]["RuleSetBody"],
                json!(conformant_body()),
                "case {label:?}: read {attempt} must serve the stored body, got {described}"
            );
        }

        // Every configuration but the last is removed: while even ONE still references the
        // rule set, the delete answers the same sentence.
        for configuration in &configurations[..configurations.len() - 1] {
            gamelift
                .ok(
                    "DeleteMatchmakingConfiguration",
                    json!({"Name": configuration}),
                )
                .await;
            let (status, parsed) = gamelift
                .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
                .await;
            let message = error_message(&parsed);
            assert_eq!(
                (status, message.as_str()),
                (400, RULE_SET_IN_USE),
                "case {label:?}: one remaining reference must answer the same refusal, got \
                 {status} with body {parsed}"
            );
        }

        // Nothing references it now, so the same delete is served and the rule set is gone.
        gamelift
            .ok(
                "DeleteMatchmakingConfiguration",
                json!({"Name": configurations.last().expect("every row references the rule set")}),
            )
            .await;
        let (status, body) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must be served once nothing references the rule set, \
             got {status} with body {body}"
        );
        let (status, parsed) = gamelift
            .call(
                "DescribeMatchmakingRuleSets",
                &json!({"Names": [&case.rule_set]}),
            )
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the deleted rule set must no longer be served, got {status} {code} \
             with body {parsed}"
        );
    }
}

/// The in-use refusal is about a rule set that IS referenced: a rule set nothing references
/// is deleted, and a name that resolves to no rule set is the not-found refusal rather than
/// the in-use one — so the sentence above is not a blanket answer.
#[tokio::test]
async fn a_rule_set_nothing_references_is_not_refused_as_in_use() {
    let gamelift = GameLift::new();
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "unreferenced.rules", "RuleSetBody": conformant_body()}),
        )
        .await;
    // A configuration on ANOTHER rule set references nothing of this one.
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "other.rules", "RuleSetBody": conformant_body()}),
        )
        .await;
    gamelift
        .ok(
            "CreateMatchmakingConfiguration",
            configuration_request("other-matchmaker", "other.rules"),
        )
        .await;

    // Nothing references it, so the delete is served.
    let (status, body) = gamelift
        .call(
            "DeleteMatchmakingRuleSet",
            &json!({"Name": "unreferenced.rules"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "a rule set nothing references must be deleted, got {status} with body {body}"
    );

    // A name that now resolves to no rule set is the not-found refusal, NOT the in-use one,
    // by either identifier form.
    for selector in [
        "unreferenced.rules".to_string(),
        rule_set_arn("unreferenced.rules"),
    ] {
        let (status, parsed) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
            .await;
        let code = error_code(&parsed);
        let message = error_message(&parsed);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "NotFoundException",
                format!("Matchmaking rule set {selector} not found").as_str()
            ),
            "case {selector:?}: a rule set that is gone is not-found, not in use, got {status} \
             {code} {message:?}"
        );
    }

    // The rule set that IS referenced still answers the in-use refusal.
    let (status, parsed) = gamelift
        .call("DeleteMatchmakingRuleSet", &json!({"Name": "other.rules"}))
        .await;
    let message = error_message(&parsed);
    assert_eq!(
        (status, message.as_str()),
        (400, RULE_SET_IN_USE),
        "the referenced rule set must answer the in-use refusal, got {status} with body {parsed}"
    );
}
