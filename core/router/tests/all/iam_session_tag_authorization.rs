// SPDX-License-Identifier: CC0-1.0

//! A conditioned identity policy decided against the SESSION TAGS the calling credential
//! carries: two services, one decision.
//!
//! RULE: a request signed with an assumed-role session carries `aws:PrincipalTag/<key>` for
//! every tag the principal holds — the session tags the mint attached, exactly as the
//! request submitted them, and the tags of the role the session is of, the session's value
//! winning where both name one key — so a `StringEquals` clause over that key holds for the
//! session whose tag is that very text and for no other, whatever characters the text is
//! made of. A statement granting an action under such a clause therefore ALLOWS the call
//! made by the exactly-tagged session (HTTP 200) and withholds the grant from a session
//! carrying another value or none, which is refused `AccessDenied` (HTTP 403) with IAM's
//! sentence naming the principal, the action, the resource and the reason.
//!
//! IAM holds the role, its conditioned policy and its tags, while STS mints the session and
//! attaches the tags, so neither crate's own test target can tag a session and then make a
//! decision about it; this is the suite that can pin it.
//!
//! The rows span what the rule generalises over: the characters a tag value is made of
//! (ASCII letters, accented Latin and CJK letters, Greek and Cyrillic, digits and a
//! separator space, every punctuation character the value's modelled pattern admits), its
//! length (one character up to the modelled maximum of 256), how the mismatching control
//! differs (letter case, stripped diacritics, transliteration, one character, one trailing
//! character), where the tag comes from (the session, the role, a tag carried forward into a
//! chained session) and whether the session carries the key at all.
//!
//! The awsQuery encoder and the XML element readers are this test crate's own
//! (`query_wire`); the IAM+STS router and its call helpers are the ones
//! `iam_role_id_agrees_with_sts` already builds for exactly this pair of services.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::iam_role_id_agrees_with_sts::{build_router, iam, iam_as, sts, LONG_TERM_KEY};
use crate::query_wire::{element, elements, opt_element};

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The session tag key every row writes and every policy conditions on.
const TAG_KEY: &str = "Team";

/// The action every conditioned statement here grants, and the one every refusal names.
const ACTION: &str = "iam:ListRoles";

/// A trust policy admitting the account root both to assume the role and to attach session
/// tags: "to pass session tags … the role's trust policy must allow the `sts:TagSession`
/// action" (`AssumeRoleRequest$Tags`).
const TAGGING_TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"#,
    r#""Action":["sts:AssumeRole","sts:TagSession"]}]}"#
);

/// Create a role whose trust policy admits the account root to assume it and to tag the
/// session, and return its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            (
                "AssumeRolePolicyDocument",
                TAGGING_TRUST_POLICY.to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole expected 200, got {status} {body}"
    );
    element(&body, "Arn")
}

/// Embed `document` in the role as its inline policy.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, document: &str) {
    let (status, body) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyName", "tag-scoped".to_string()),
            ("PolicyDocument", document.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy expected 200, got {status} {body}"
    );
}

/// Put one tag on the role itself, as `iam:TagRole` does.
async fn tag_role(router: &MultiServiceRouter, role_name: &str, key: &str, value: &str) {
    let (status, body) = iam(
        router,
        "TagRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Tags.member.1.Key", key.to_string()),
            ("Tags.member.1.Value", value.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: TagRole expected 200, got {status} {body}"
    );
}

/// Assume `role_arn` as `signing_key`, under `session_name`, carrying `tags` as session tags
/// and naming `transitive` as the keys that pass to a subsequent session; returns the access
/// key id of the session STS minted.
async fn assume_role(
    router: &MultiServiceRouter,
    signing_key: &str,
    role_arn: &str,
    session_name: &str,
    tags: &[(&str, &str)],
    transitive: &[&str],
) -> String {
    // The member names a list of tags is spelled with, held so the parameters can borrow
    // them: one `Key` and one `Value` per tag, in the order the request names them.
    let mut names: Vec<String> = Vec::new();
    for position in 1..=tags.len() {
        names.push(format!("Tags.member.{position}.Key"));
        names.push(format!("Tags.member.{position}.Value"));
    }
    for position in 1..=transitive.len() {
        names.push(format!("TransitiveTagKeys.member.{position}"));
    }
    let mut spelled = names.iter();
    let mut next = |value: &str| {
        (
            spelled
                .next()
                .expect("one member name per parameter")
                .as_str(),
            value.to_string(),
        )
    };
    let mut params: Vec<(&str, String)> = vec![
        ("RoleArn", role_arn.to_string()),
        ("RoleSessionName", session_name.to_string()),
    ];
    for (key, value) in tags {
        params.push(next(key));
        params.push(next(value));
    }
    for key in transitive {
        params.push(next(key));
    }
    let (status, body) = sts(router, signing_key, "AssumeRole", &params).await;
    assert_eq!(
        status, 200,
        "case {session_name:?}: AssumeRole with tags {tags:?} expected 200, got {status} {body}"
    );
    element(&body, "AccessKeyId")
}

/// Call `iam:ListRoles` as `access_key_id` and return `(status, body)`.
async fn list_roles(router: &MultiServiceRouter, access_key_id: &str) -> (u16, String) {
    iam_as(router, access_key_id, "ListRoles", &[]).await
}

/// One identity-based statement granting [`ACTION`] everywhere, under a `StringEquals`
/// clause over the principal tag [`TAG_KEY`] holding exactly `value`.
///
/// The document is written the way a client's JSON serializer writes one: every non-ASCII
/// character as a `\uXXXX` escape ([`ascii_json`]), because an IAM policy document is text
/// `policyDocumentType` admits — `@pattern [\u0009\u000A\u000D\u0020-\u00FF]+` — so a
/// clause about a value outside that range travels as JSON escapes and is the value again
/// once the document is parsed.
fn conditioned(value: &str) -> String {
    conditioned_on(TAG_KEY, value)
}

/// [`conditioned`] over a principal tag key other than [`TAG_KEY`].
fn conditioned_on(tag_key: &str, value: &str) -> String {
    let mut equals = Map::new();
    equals.insert(
        format!("aws:PrincipalTag/{tag_key}"),
        Value::String(value.to_string()),
    );
    ascii_json(
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ACTION,
                "Resource": "*",
                "Condition": {"StringEquals": Value::Object(equals)},
            }],
        })
        .to_string(),
    )
}

/// `text` with every non-ASCII character written as the JSON `\uXXXX` escape (a pair of
/// them for a character outside the basic multilingual plane), which is how a JSON
/// serializer emits one when it is asked for ASCII output.
fn ascii_json(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii() {
            written.push(character);
            continue;
        }
        let mut units = [0u16; 2];
        for unit in character.encode_utf16(&mut units) {
            written.push_str(&format!("\\u{unit:04x}"));
        }
    }
    written
}

/// IAM's refusal of [`ACTION`] for a session nothing in force grants it to: the sentence
/// naming the principal, the action, the resource the request was made on and the reason the
/// decision went that way.
fn withheld_message(role_name: &str, session_name: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{session_name} is not \
         authorized to perform: {ACTION} on resource: arn:aws:iam::{ACCOUNT}:role/ because no \
         identity-based policy allows the {ACTION} action"
    )
}

/// Assert the answer to `(status, body)` is the `AccessDenied` refusal of [`ACTION`] for
/// `session_name` of `role_name`.
fn assert_withheld(case: &str, role_name: &str, session_name: &str, status: u16, body: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: {session_name} expected HTTP 403, got {status} {body}"
    );
    assert_eq!(
        opt_element(body, "Code").as_deref(),
        Some("AccessDenied"),
        "case {case:?}: {session_name} expected the error code AccessDenied, got {body}"
    );
    assert_eq!(
        element(body, "Message"),
        withheld_message(role_name, session_name),
        "case {case:?}: {session_name} expected IAM's withheld-grant sentence, got {body}"
    );
}

/// Assert the answer to `(status, body)` is the listing, carrying `role_name`.
fn assert_lists(case: &str, role_name: &str, status: u16, body: &str) {
    assert_eq!(
        status, 200,
        "case {case:?}: the exactly tagged session expected HTTP 200, got {status} {body}"
    );
    let listed = elements(body, "RoleName");
    assert!(
        listed.iter().any(|name| name == role_name),
        "case {case:?}: expected {role_name} among the listed roles, got {listed:?}"
    );
}

/// One row of the table: a tag value, and a value differing from it that must NOT satisfy
/// the same clause.
struct Row {
    /// What this row is about, quoted by every assertion it makes.
    case: &'static str,
    /// The value the session is tagged with and the clause tests for.
    value: String,
    /// A value the clause must not hold for, which the control session is tagged with.
    other: String,
}

/// The values a session tag may carry, as `tagValueType` admits them: `@length` 0..256 with
/// `@pattern ^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$` — Unicode letters, separators, numbers and that
/// punctuation set — from
/// `smithy_query(query="shape",shape="Tag",model_path="services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ASCII letters, the control differing only in case",
            value: "Platform".to_string(),
            other: "platform".to_string(),
        },
        Row {
            case: "accented Latin and CJK letters, the control with the diacritics stripped",
            value: "Café-Zürich-北京".to_string(),
            other: "Cafe-Zurich-北京".to_string(),
        },
        Row {
            case: "Greek and Cyrillic letters, the control transliterated",
            value: "Ωμέγα-команда".to_string(),
            other: "Omega-komanda".to_string(),
        },
        Row {
            case: "letters, a separator space and digits",
            value: "équipe 42".to_string(),
            other: "équipe 43".to_string(),
        },
        Row {
            case: "every punctuation character the pattern admits",
            value: "a_b.c:d/e=f+g-h@i".to_string(),
            other: "a_b.c:d/e=f+g-h@j".to_string(),
        },
        Row {
            case: "one character",
            value: "é".to_string(),
            other: "e".to_string(),
        },
        Row {
            // The modelled maximum length, and a control of the same length.
            case: "the modelled maximum of 256 characters",
            value: "T".repeat(256),
            other: format!("{}X", "T".repeat(255)),
        },
    ]
}

/// RULE (the session's own tag): the tag value a session was minted with is what
/// `aws:PrincipalTag/<key>` holds for every request that session signs, so a `StringEquals`
/// clause granting an action under that key admits the session tagged with exactly that text
/// — whatever characters it is made of — and withholds the grant from a session tagged with
/// any other value and from one carrying no such tag, which IAM refuses `AccessDenied`.
#[tokio::test]
async fn a_principal_tag_clause_admits_the_session_tagged_with_exactly_its_value() {
    let router = build_router();
    for (index, row) in rows().into_iter().enumerate() {
        let Row { case, value, other } = row;
        let role_name = format!("session-tag-role-{index}");
        let role_arn = create_role(&router, &role_name).await;
        put_role_policy(&router, &role_name, &conditioned(&value)).await;

        // The three sessions of one role: tagged with exactly the value the clause tests
        // for, tagged with a value that differs, and carrying no tag at all.
        let exact = assume_role(
            &router,
            LONG_TERM_KEY,
            &role_arn,
            "exact-session",
            &[(TAG_KEY, value.as_str())],
            &[],
        )
        .await;
        let mismatched = assume_role(
            &router,
            LONG_TERM_KEY,
            &role_arn,
            "mismatched-session",
            &[(TAG_KEY, other.as_str())],
            &[],
        )
        .await;
        let untagged =
            assume_role(&router, LONG_TERM_KEY, &role_arn, "untagged-session", &[], &[]).await;

        let (status, body) = list_roles(&router, &exact).await;
        assert_lists(case, &role_name, status, &body);
        let (status, body) = list_roles(&router, &mismatched).await;
        assert_withheld(case, &role_name, "mismatched-session", status, &body);
        let (status, body) = list_roles(&router, &untagged).await;
        assert_withheld(case, &role_name, "untagged-session", status, &body);
    }
}

/// RULE (the role's own tags, and which value wins): the principal of a role session holds
/// the tags of the ROLE as well as the session's own, so the same clause is satisfied by a
/// session carrying no tag of a role tagged with its value — and where both name one key the
/// SESSION's value is the one in force, so a session tagged with another value is refused
/// although the role carries the matching tag.
#[tokio::test]
async fn a_role_tag_decides_the_same_clause_and_a_session_tag_overrides_it() {
    let router = build_router();
    let value = "Café-Zürich-北京";
    let other = "Cafe-Zurich-北京";
    let role_name = "role-tag-role".to_string();
    let role_arn = create_role(&router, &role_name).await;
    put_role_policy(&router, &role_name, &conditioned(value)).await;
    tag_role(&router, &role_name, TAG_KEY, value).await;

    // One row per session: the tags it is minted with, and whether the conditioned statement
    // then admits its call.
    struct SessionRow {
        case: &'static str,
        tags: Vec<(&'static str, &'static str)>,
        answered: bool,
    }
    let sessions = vec![
        SessionRow {
            case: "the role's tag decides for a session carrying none",
            tags: vec![],
            answered: true,
        },
        SessionRow {
            case: "the session's own tag repeats the role's",
            tags: vec![(TAG_KEY, value)],
            answered: true,
        },
        SessionRow {
            case: "the session's own tag overrides the role's",
            tags: vec![(TAG_KEY, other)],
            answered: false,
        },
    ];
    for (index, row) in sessions.into_iter().enumerate() {
        let SessionRow {
            case,
            tags,
            answered,
        } = row;
        let session_name = format!("role-tagged-{index}");
        let key = assume_role(&router, LONG_TERM_KEY, &role_arn, &session_name, &tags, &[]).await;
        let (status, body) = list_roles(&router, &key).await;
        if answered {
            assert_lists(case, &role_name, status, &body);
        } else {
            assert_withheld(case, &role_name, &session_name, status, &body);
        }
    }
}

/// RULE (a tag carried forward): a session tag named transitive passes to the session a
/// chained assume mints — "if you set a tag key as transitive, the corresponding key and
/// value passes to subsequent sessions in a role chain"
/// (`AssumeRoleRequest$TransitiveTagKeys`) — so the chained session satisfies a clause over
/// that tag exactly as the session it was minted from, while a chain that named no
/// transitive key carries nothing forward and is refused.
#[tokio::test]
async fn a_transitive_session_tag_decides_the_clause_in_the_chained_session() {
    let router = build_router();
    let value = "Café-Zürich-北京";
    let source = "chain-source-role".to_string();
    let source_arn = create_role(&router, &source).await;
    let target = "chain-target-role".to_string();
    let target_arn = create_role(&router, &target).await;
    put_role_policy(&router, &target, &conditioned(value)).await;

    // (case, the transitive keys the first assume names, whether the chained call is
    // answered)
    let chains: Vec<(&str, Vec<&str>, bool)> = vec![
        ("the tag is named transitive", vec![TAG_KEY], true),
        ("no transitive key is named", vec![], false),
    ];
    for (index, (case, transitive, answered)) in chains.into_iter().enumerate() {
        let first = assume_role(
            &router,
            LONG_TERM_KEY,
            &source_arn,
            &format!("first-{index}"),
            &[(TAG_KEY, value)],
            &transitive,
        )
        .await;
        let session_name = format!("chained-{index}");
        let chained = assume_role(&router, &first, &target_arn, &session_name, &[], &[]).await;
        let (status, body) = list_roles(&router, &chained).await;
        if answered {
            assert_lists(case, &target, status, &body);
        } else {
            assert_withheld(case, &target, &session_name, status, &body);
        }
    }
}

/// RULE (one key, however it is cased): a tag key names one tag whatever case it is spelled
/// in — tag keys "are not case sensitive, but case is preserved" (`AssumeRoleRequest$Tags`)
/// — so a session tag whose key differs from a role tag's only in case sets the SAME tag,
/// and the value in force for it is the session's. A clause testing for the role's value is
/// therefore not satisfied by such a session, while the same session carrying that value is
/// admitted. The key is spelled with letters outside ASCII, which `Tag$Key`'s pattern
/// (`^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`) admits, so the casing is folded over the whole of
/// Unicode rather than over ASCII alone.
#[tokio::test]
async fn a_session_tag_sets_the_role_tag_whose_key_differs_only_in_case() {
    let router = build_router();
    let role_key = "ÉQUIPE";
    let session_key = "équipe";
    let role_value = "Café-Zürich-北京";
    let role_name = "cased-key-role".to_string();
    let role_arn = create_role(&router, &role_name).await;
    put_role_policy(&router, &role_name, &conditioned_on(session_key, role_value)).await;
    tag_role(&router, &role_name, role_key, role_value).await;

    // (case, the value the session's own tag carries, whether the call is answered)
    let sessions = [
        (
            "the session's value is the one in force",
            "Cafe-Zurich-北京",
            false,
        ),
        (
            "the session carries the value the clause tests for",
            role_value,
            true,
        ),
    ];
    for (index, (case, session_value, answered)) in sessions.into_iter().enumerate() {
        let session_name = format!("cased-key-{index}");
        let key = assume_role(
            &router,
            LONG_TERM_KEY,
            &role_arn,
            &session_name,
            &[(session_key, session_value)],
            &[],
        )
        .await;
        let (status, body) = list_roles(&router, &key).await;
        if answered {
            assert_lists(case, &role_name, status, &body);
        } else {
            assert_withheld(case, &role_name, &session_name, status, &body);
        }
    }
}
