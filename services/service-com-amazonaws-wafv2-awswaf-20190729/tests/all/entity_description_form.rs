// SPDX-License-Identifier: CC0-1.0

//! The form an entity's `Description` has to take, end to end over the wire.
//!
//! The rule asserted here: `EntityDescription` — the shape EVERY create and update of
//! this service describes its entity with — is constrained `@length(1..256)` with a
//! `@pattern` over the ASCII word characters and a small punctuation and whitespace
//! set (`smithy_query(query="shape",shape="EntityDescription")`). A Smithy `@pattern`
//! is an ECMA-262 regular expression, where `\w` and `\s` are the ASCII classes, so a
//! description carrying characters outside ASCII satisfies none of them: the request
//! is refused as a request-SHAPE error — HTTP 400 `ValidationException`, naming the
//! member and the requirement it failed — and nothing is stored. The same model
//! writes its TAG patterns with the Unicode-aware classes (`\p{L}`, `\p{N}`,
//! `\p{Z}`), so a localized tag value round-trips where a localized description is
//! refused; `crate::tag_value_round_trip` pins that side.
//!
//! The rows span the class rather than one recorded description: the ASCII forms the
//! pattern admits (its shortest, every punctuation character of its class, and a
//! description at the published length bound), and the forms it does not (letters of
//! three different scripts, an emoji, a non-breaking space — which is whitespace only
//! under the Unicode reading of `\s` — a description shorter than the pattern's three
//! characters, and one past the published length bound). Every row is driven through
//! each entity this service describes, because the constraint is the shared shape's
//! and not one operation's.
//!
//! Every call goes through the router's front door, the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{visibility, Waf};

/// The scope the rows are saved in: a description is a member of the request, so one
/// namespace carries the table.
const SCOPE: &str = "REGIONAL";

/// `EntityDescription`'s `@pattern`, as the refusal's clause names it
/// (`smithy_query(query="shape",shape="EntityDescription")`). Written out here rather
/// than imported, so the table does not restate the code under test.
const PATTERN_REQUIREMENT: &str = r"Member must satisfy regular expression pattern: ^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$";

/// The upper bound of `EntityDescription`'s `@length`, as the refusal's clause names
/// it. See [`PATTERN_REQUIREMENT`].
const LENGTH_REQUIREMENT: &str = "Member must have length less than or equal to 256";

/// One description, and what the save must make of it.
struct Description {
    case: &'static str,
    value: String,
    /// The requirement the refusal must name, or `None` when the description is
    /// inside the modelled form.
    refused_by: Option<&'static str>,
}

/// Every row the table is driven over.
fn descriptions() -> Vec<Description> {
    vec![
        Description {
            case: "an ASCII description with punctuation and digits",
            value: "Regional WAF ACL, desc-01 v1.0 test_env".to_string(),
            refused_by: None,
        },
        Description {
            case: "the shortest description the pattern admits",
            value: "a_b".to_string(),
            refused_by: None,
        },
        Description {
            case: "every punctuation character of the pattern's class",
            value: "net=10.0.0.0/8,port:443#main@edge-1.2".to_string(),
            refused_by: None,
        },
        Description {
            case: "a description at the published length bound",
            value: "d".repeat(256),
            refused_by: None,
        },
        Description {
            case: "a description one character past the published length bound",
            value: "d".repeat(257),
            refused_by: Some(LENGTH_REQUIREMENT),
        },
        Description {
            case: "Han characters",
            value: "网站防火墙 描述二 v2".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "a Latin letter with a diacritic",
            value: "café-blocked".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "Cyrillic characters",
            value: "описание-1".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "an emoji",
            value: "blocked 🎉 requests".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "a non-breaking space, whitespace only outside ASCII",
            value: "blocked\u{00a0}requests".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "two characters, below the three the pattern requires",
            value: "ab".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
        Description {
            case: "a leading space, outside the pattern's first class",
            value: " leading-space".to_string(),
            refused_by: Some(PATTERN_REQUIREMENT),
        },
    ]
}

/// One entity this service describes: the create that carries its `Description`, and
/// the read that hands it back.
struct Describable {
    /// The entity, for an assertion and for the name it is saved under.
    entity: &'static str,
    /// The operation that creates it.
    create: &'static str,
    /// The operation that reads it back.
    read: &'static str,
    /// The operation that removes it, so the table stays under the per-scope quota
    /// WAF publishes for that entity.
    delete: &'static str,
    /// The member of the read's response that holds the entity.
    member: &'static str,
}

/// The entities the rows are driven over: every one whose create takes an
/// `EntityDescription`.
const DESCRIBABLES: &[Describable] = &[
    Describable {
        entity: "web ACL",
        create: "CreateWebACL",
        read: "GetWebACL",
        delete: "DeleteWebACL",
        member: "WebACL",
    },
    Describable {
        entity: "IP set",
        create: "CreateIPSet",
        read: "GetIPSet",
        delete: "DeleteIPSet",
        member: "IPSet",
    },
    Describable {
        entity: "rule group",
        create: "CreateRuleGroup",
        read: "GetRuleGroup",
        delete: "DeleteRuleGroup",
        member: "RuleGroup",
    },
    Describable {
        entity: "regex pattern set",
        create: "CreateRegexPatternSet",
        read: "GetRegexPatternSet",
        delete: "DeleteRegexPatternSet",
        member: "RegexPatternSet",
    },
];

/// The create body for `entity` under `name`, describing it with `description`.
fn create_body(entity: &Describable, name: &str, description: &str) -> Value {
    let mut body = json!({
        "Name": name,
        "Scope": SCOPE,
        "Description": description,
    });
    match entity.create {
        "CreateWebACL" => {
            body["DefaultAction"] = json!({ "Allow": {} });
            body["VisibilityConfig"] = visibility(name);
        }
        "CreateIPSet" => {
            body["IPAddressVersion"] = json!("IPV4");
            body["Addresses"] = json!(["10.0.0.0/8"]);
        }
        "CreateRuleGroup" => {
            body["Capacity"] = json!(10);
            body["VisibilityConfig"] = visibility(name);
        }
        _ => {
            body["RegularExpressionList"] = json!([{ "RegexString": "^/api" }]);
        }
    }
    body
}

/// Remove the entity the row created, so the table stays under the per-scope quota
/// WAF publishes for that entity kind however many rows it holds.
async fn remove(waf: &Waf, entity: &Describable, name: &str, id: &str, lock_token: &Value) {
    waf.ok(
        entity.delete,
        json!({
            "Name": name,
            "Scope": SCOPE,
            "Id": id,
            "LockToken": lock_token,
        }),
    )
    .await;
}

/// Assert that `body` is the request-shape refusal naming the `Description` member
/// and `requirement`.
fn assert_validation_refusal(body: &Value, requirement: &str, case: &str) {
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("at 'description'"),
        "case {case:?}: the refusal must name the member that failed, got {body}"
    );
    assert!(
        message.contains(requirement),
        "case {case:?}: the refusal must name the requirement {requirement:?}, got {body}"
    );
    assert!(
        message.starts_with("1 validation error detected: "),
        "case {case:?}: the refusal must be the request validator's envelope, got {body}"
    );
}

/// Every create that takes an `EntityDescription` accepts exactly the descriptions
/// the modelled form admits: an accepted one reads back byte for byte, and a refused
/// one is the request validator's `ValidationException` with nothing stored.
#[tokio::test]
async fn a_description_outside_the_modelled_form_is_refused_by_every_create_that_takes_one() {
    let waf = Waf::new();
    for entity in DESCRIBABLES {
        for (index, row) in descriptions().iter().enumerate() {
            let case = format!("{}: {}", entity.entity, row.case);
            let name = format!("described-{index}");
            let body = create_body(entity, &name, &row.value);

            let Some(requirement) = row.refused_by else {
                let created = waf.ok(entity.create, body).await;
                let id = created["Summary"]["Id"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a Summary.Id, got {created}")
                    })
                    .to_string();
                let got = waf
                    .ok(
                        entity.read,
                        json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
                    )
                    .await;
                assert_eq!(
                    got[entity.member]["Description"].as_str(),
                    Some(row.value.as_str()),
                    "case {case:?}: an accepted description must read back byte for \
                     byte, got {got}"
                );
                remove(&waf, entity, &name, &id, &created["Summary"]["LockToken"]).await;
                continue;
            };

            let refused = waf
                .refused(entity.create, body, "ValidationException", &case)
                .await;
            assert_validation_refusal(&refused, requirement, &case);
            assert!(
                refused["Summary"].is_null(),
                "case {case:?}: a refused create must answer no summary, got {refused}"
            );
            // Nothing was stored: the name is still free for a save that describes
            // the entity the way the model admits.
            let retried = waf
                .ok(
                    entity.create,
                    create_body(entity, &name, "retried after the refusal"),
                )
                .await;
            let id = retried["Summary"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {retried}"))
                .to_string();
            let got = waf
                .ok(
                    entity.read,
                    json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
                )
                .await;
            assert_eq!(
                got[entity.member]["Description"].as_str(),
                Some("retried after the refusal"),
                "case {case:?}: the refused name must still be free, got {got}"
            );
            remove(&waf, entity, &name, &id, &retried["Summary"]["LockToken"]).await;
        }
    }
}

/// `UpdateWebACL` judges the same member the same way, and a refused update changes
/// nothing: the stored description and the `LockToken` are what they were.
#[tokio::test]
async fn update_web_acl_refuses_the_same_descriptions_and_changes_nothing() {
    let waf = Waf::new();
    let name = "redescribed-acl";
    let stored = "Regional WAF ACL, desc-01 v1.0 test_env";
    let created = waf
        .ok("CreateWebACL", create_body(&DESCRIBABLES[0], name, stored))
        .await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let mut lock_token = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    for row in descriptions().iter() {
        let case = format!("UpdateWebACL: {}", row.case);
        let mut body = create_body(&DESCRIBABLES[0], name, &row.value);
        body["Id"] = json!(&id);
        body["LockToken"] = json!(&lock_token);

        let Some(requirement) = row.refused_by else {
            // An accepted description is committed, and then put back, so the next
            // row is judged against the same starting point and the same token.
            let updated = waf.ok("UpdateWebACL", body).await;
            let next = updated["NextLockToken"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a NextLockToken, got {updated}"))
                .to_string();
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": name, "Scope": SCOPE, "Id": &id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Description"].as_str(),
                Some(row.value.as_str()),
                "case {case:?}: an accepted description must be committed, got {got}"
            );
            let mut restore = create_body(&DESCRIBABLES[0], name, stored);
            restore["Id"] = json!(&id);
            restore["LockToken"] = json!(&next);
            let restored = waf.ok("UpdateWebACL", restore).await;
            lock_token = restored["NextLockToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the restore must answer a lock token, got {restored}")
                })
                .to_string();
            continue;
        };

        let refused = waf
            .refused("UpdateWebACL", body, "ValidationException", &case)
            .await;
        assert_validation_refusal(&refused, requirement, &case);
        assert!(
            refused["NextLockToken"].is_null(),
            "case {case:?}: a refused update must answer no lock token, got {refused}"
        );
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": SCOPE, "Id": &id }),
            )
            .await;
        assert_eq!(
            (
                got["WebACL"]["Description"].as_str(),
                got["LockToken"].as_str()
            ),
            (Some(stored), Some(lock_token.as_str())),
            "case {case:?}: a refused update must change nothing at all, got {got}"
        );
    }
}
