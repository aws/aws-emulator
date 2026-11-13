// SPDX-License-Identifier: CC0-1.0

//! Policy VARIABLES, and the signing principal's own condition keys, end to end over the
//! awsQuery wire.
//!
//! The rule under test: a policy element is matched after substituting its `${…}`
//! variables with the value the REQUEST carries for the condition key each names — among
//! them the keys that describe the signing principal (`aws:username`, `aws:userid` and
//! `aws:PrincipalTag/<key>`, the last read from the principal's tags at the instant of the
//! call) — so ONE stored document decides differently per caller and over a retag, while
//! the document itself is stored and reported verbatim. A variable's VALUE is substituted
//! as text: a wildcard the document wrote is still a wildcard, one that arrives with a
//! value is that character, `${*}`/`${?}`/`${$}` stand for the characters they name, and a
//! variable the request carries no value for grants nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which key
//! the variable names, where it sits in the element, the case the key is spelled in,
//! whether the value is the caller's own name or another's, whether a wildcard belongs to
//! the document or to the value, which operation the decision is made for, and — for the
//! tag keys — what the principal is tagged with at the moment of the call.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{reported, simulate, ALLOWED, IMPLICIT_DENY};
use crate::user_lifecycle::{account_id, call, call_signed_by, element, error_code, router};

/// The three per-user reads this suite drives, each gated on the user the request
/// addresses, so no row's answer can come from one operation's own quirk.
const GATED_READS: [(&str, &str); 3] = [
    ("ListAccessKeys", "iam:ListAccessKeys"),
    ("ListUserTags", "iam:ListUserTags"),
    ("ListUserPolicies", "iam:ListUserPolicies"),
];

/// A one-statement document allowing [`GATED_READS`] on the single `Resource` value
/// `resource` — the element whose variables each row is about.
fn shared_document(resource: &str) -> String {
    let actions: Vec<String> = GATED_READS
        .iter()
        .map(|(_, action)| format!("\"{action}\""))
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"SelfServiceOnly","Effect":"Allow","Action":[{}],"Resource":"{resource}"}}]}}"#,
        actions.join(",")
    )
}

/// The ARN of an IAM user of this account whose name part is `tail` — which a row writes
/// with policy variables in it.
fn user_resource(tail: &str) -> String {
    format!("arn:aws:iam::{}:user/{tail}", account_id())
}

/// Create a user carrying `tags` and one access key, answering the access key id a
/// request signs with to speak for it.
///
/// `pub(crate)` because the suites about what the decision knows of the caller and of the
/// TARGET (`tests/all/resource_tag_condition_context.rs`) need the same tagged fixture
/// principal.
pub(crate) async fn tagged_principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    tags: &[(String, String)],
) -> String {
    let mut params: Vec<(String, String)> = vec![("UserName".to_string(), user_name.to_string())];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push((format!("Tags.member.{}.Key", position + 1), key.clone()));
        params.push((format!("Tags.member.{}.Value", position + 1), value.clone()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, created) = call(router, "CreateUser", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser {user_name:?} must succeed, got {status}: {created}"
    );
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey for {user_name:?} must succeed, got \
         {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// Create the customer managed policy `policy_name` carrying `document`, answering its
/// ARN, and assert the stored document is the one supplied — byte for byte, variables and
/// all.
async fn shared_policy(
    router: &MultiServiceRouter,
    case: &str,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {created}"
    );
    let arn = element(&created, "Arn");
    let (status, version) = call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", &arn), ("VersionId", "v1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPolicyVersion must succeed, got {status}: {version}"
    );
    // Substitution happens at EVALUATION time: the write path stores the document as it
    // was supplied, so a reader sees the variable and never a caller's value.
    assert_eq!(
        crate::user_lifecycle::document_element(&version, "Document"),
        document,
        "case {case:?}: the stored document must be the one supplied, variables unexpanded"
    );
    arn
}

/// Attach `policy_arn` to `user_name`.
async fn attach(router: &MultiServiceRouter, case: &str, policy_arn: &str, user_name: &str) {
    let (status, attached) = call(
        router,
        "AttachUserPolicy",
        &[("UserName", user_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture AttachUserPolicy for {user_name:?} must succeed, got \
         {status}: {attached}"
    );
}

/// Drive one gated read of `target`, signed by `access_key_id`, and assert it is served or
/// refused `AccessDenied` as `served` requires.
async fn assert_read(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
    target: &str,
    served: bool,
) {
    for (operation, action) in GATED_READS {
        let (status, body) = call_signed_by(
            router,
            Some(access_key_id),
            operation,
            &[("UserName", target)],
        )
        .await;
        let expected = if served { 200 } else { 403 };
        assert_eq!(
            status, expected,
            "case {case:?}: {action} on {target:?} expected HTTP {expected}, got {status}: {body}"
        );
        if !served {
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: {action} on {target:?} expected the error code AccessDenied, \
                 got {:?}",
                error_code(&body)
            );
        }
    }
}

/// One row: the `Resource` value's name part, and whether the one shared document then
/// grants a caller the read of ITSELF and the read of its sibling.
struct Row {
    case: &'static str,
    /// What follows `user/` in the statement's single `Resource` value.
    resource_tail: &'static str,
    /// Whether a signer may read ITS OWN user through that element.
    reads_itself: bool,
    /// Whether the same signer may read the OTHER user of the row.
    reads_the_other: bool,
}

/// The rows. Each runs against a fresh pair of users, tagged symmetrically — `alias` is
/// the user's own name and `peer` the other's — so one element is decided twice from the
/// same document and the two answers are what the row states.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the signing user's own name",
            resource_tail: "${aws:username}",
            reads_itself: true,
            reads_the_other: false,
        },
        Row {
            // "Condition key names are not case-sensitive", and a variable names a
            // condition key.
            case: "the same key spelled in another case",
            resource_tail: "${AWS:UserName}",
            reads_itself: true,
            reads_the_other: false,
        },
        Row {
            case: "a wildcard the DOCUMENT wrote after the variable",
            resource_tail: "${aws:username}*",
            reads_itself: true,
            reads_the_other: false,
        },
        Row {
            // `?` is one character, which the expanded ARN has none of left.
            case: "a single-character wildcard the document wrote after the variable",
            resource_tail: "${aws:username}?",
            reads_itself: false,
            reads_the_other: false,
        },
        Row {
            case: "a principal tag whose value is the caller's own name",
            resource_tail: "${aws:PrincipalTag/alias}",
            reads_itself: true,
            reads_the_other: false,
        },
        Row {
            // The variable is not a stand-in for "self": it is whatever the key's value
            // is, which here names the OTHER user.
            case: "a principal tag whose value names the other user",
            resource_tail: "${aws:PrincipalTag/peer}",
            reads_itself: false,
            reads_the_other: true,
        },
        Row {
            case: "no variable at all, every user of the account",
            resource_tail: "*",
            reads_itself: true,
            reads_the_other: true,
        },
        Row {
            // `${$}` is the literal `$`, so what follows it opens no variable and the
            // element names a user called `${aws:username}`.
            case: "the dollar escape, which leaves the rest matched literally",
            resource_tail: "${$}{aws:username}",
            reads_itself: false,
            reads_the_other: false,
        },
        Row {
            // `${*}` is the literal `*`, not the wildcard the bare character is.
            case: "the star escape after the variable",
            resource_tail: "${aws:username}${*}",
            reads_itself: false,
            reads_the_other: false,
        },
        Row {
            case: "a principal tag the callers do not carry",
            resource_tail: "${aws:PrincipalTag/nosuchtag}",
            reads_itself: false,
            reads_the_other: false,
        },
        Row {
            // The unique id is not what a user ARN's name part carries, so the expanded
            // element names no user.
            case: "the caller's unique id, which is not its ARN's name part",
            resource_tail: "${aws:userid}",
            reads_itself: false,
            reads_the_other: false,
        },
    ]
}

/// RULE: a statement's `Resource` element is matched with its policy variables
/// substituted from the request's context, so ONE document attached to several users
/// scopes itself per signer — and a wildcard belongs to whoever wrote it, the document or
/// the escape, never to a value.
#[tokio::test]
async fn one_documents_resource_variables_are_substituted_per_signer() {
    let router = router();

    for (number, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // Equal-length names, so a row about a single-character wildcard is decided by
        // the wildcard and not by a difference in length.
        let first = format!("pvar-a{number:02}");
        let second = format!("pvar-b{number:02}");
        let signers = [(&first, &second), (&second, &first)];

        let document = shared_document(&user_resource(row.resource_tail));
        assert!(
            !document.contains(&first) && !document.contains(&second),
            "case {case:?}: the document must name neither user, got {document}"
        );
        let policy_arn =
            shared_policy(&router, case, &format!("pvar-shared{number:02}"), &document).await;

        let mut keys = Vec::new();
        for (user, other) in signers {
            let tags = [
                ("alias".to_string(), user.clone()),
                ("peer".to_string(), other.clone()),
            ];
            keys.push(tagged_principal(&router, case, user, &tags).await);
            attach(&router, case, &policy_arn, user).await;
        }

        for (position, (user, other)) in signers.into_iter().enumerate() {
            let signer = &keys[position];
            let case = format!("{case} (signed by {user})");
            assert_read(&router, &case, signer, user, row.reads_itself).await;
            assert_read(&router, &case, signer, other, row.reads_the_other).await;
        }
    }
}

/// The condition key a tag-scoped statement is written over, and the two values the
/// transition below moves the caller between.
const TEAM_KEY: &str = "aws:PrincipalTag/team";
const FIRST_TEAM: &str = "payments";
const SECOND_TEAM: &str = "storage";

/// RULE: `aws:PrincipalTag/<key>` is read from the SIGNING principal's current tags, so a
/// statement conditioned on one is decided by what the principal is tagged with at the
/// instant of the call: retagging the user — which changes no policy and no request byte —
/// flips the same call from served to refused and back.
#[tokio::test]
async fn a_principal_tag_condition_follows_the_callers_current_tags() {
    let router = router();
    let user_name = "abac-caller";
    let access_key_id = tagged_principal(
        &router,
        "principal tag transition",
        user_name,
        &[("team".to_string(), FIRST_TEAM.to_string())],
    )
    .await;
    let own = user_resource(user_name);

    // The conditioned grant, and an unconditioned control over another gated read of the
    // same user: the control must stay served throughout, so a row's refusal is the
    // condition and not the gate turning everything off.
    for (policy_name, document) in [
        (
            "abac",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserPolicies","Resource":"{own}","Condition":{{"StringEquals":{{"{TEAM_KEY}":"{FIRST_TEAM}"}}}}}}]}}"#
            ),
        ),
        (
            "control",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserTags","Resource":"{own}"}}]}}"#
            ),
        ),
    ] {
        let (status, put) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture PutUserPolicy {policy_name:?} must succeed, got {status}: {put}"
        );
    }

    // The rows are the states the caller's tags are moved through, each by a call that
    // touches no policy: the value the condition admits, another value, the key removed
    // altogether, and back. Twice around, because neither the refusal nor its lifting is
    // a one-shot.
    enum Retag {
        To(&'static str),
        Removed,
    }
    let transitions = [
        ("tagged with the value the condition admits", Retag::To(FIRST_TEAM), true),
        ("retagged to another value", Retag::To(SECOND_TEAM), false),
        ("tagged back", Retag::To(FIRST_TEAM), true),
        ("untagged, so the key has no value at all", Retag::Removed, false),
        ("tagged with the admitted value again", Retag::To(FIRST_TEAM), true),
    ];
    for round in 1..=2 {
        for (case, retag, served) in &transitions {
            let case = format!("round {round}: {case}");
            match retag {
                Retag::To(value) => {
                    let (status, tagged) = call(
                        &router,
                        "TagUser",
                        &[
                            ("UserName", user_name),
                            ("Tags.member.1.Key", "team"),
                            ("Tags.member.1.Value", value),
                        ],
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: TagUser must succeed, got {status}: {tagged}"
                    );
                }
                Retag::Removed => {
                    let (status, untagged) = call(
                        &router,
                        "UntagUser",
                        &[("UserName", user_name), ("TagKeys.member.1", "team")],
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: UntagUser must succeed, got {status}: {untagged}"
                    );
                }
            }

            // Asked twice: a decision read off the caller's tags is the same both times,
            // and neither answer consumes anything.
            for attempt in 1..=2 {
                let (status, body) = call_signed_by(
                    &router,
                    Some(&access_key_id),
                    "ListUserPolicies",
                    &[("UserName", user_name)],
                )
                .await;
                let expected = if *served { 200 } else { 403 };
                assert_eq!(
                    status, expected,
                    "case {case:?} attempt {attempt}: expected HTTP {expected}, got \
                     {status}: {body}"
                );
                if !*served {
                    assert_eq!(
                        error_code(&body),
                        "AccessDenied",
                        "case {case:?} attempt {attempt}: expected AccessDenied, got {:?}",
                        error_code(&body)
                    );
                }
            }

            // The unconditioned grant over the same user is untouched by any of it.
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                "ListUserTags",
                &[("UserName", user_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the unconditioned control must stay served, got {status}: {body}"
            );
        }
    }
}

/// RULE: the simulator decides the same clause the real call does from the value the
/// request supplies for the key — a value it is given is TEXT, so a `*` in it matches the
/// character and not every resource — and with no value for the key it does not guess: the
/// decision is the implicit deny and the key is reported in `MissingContextValues`.
#[tokio::test]
async fn the_simulator_decides_a_supplied_value_as_text() {
    let router = router();
    let user_name = "sim-abac";
    let own = user_resource(user_name);
    let arn = own.clone();
    let _key = tagged_principal(
        &router,
        "simulated context",
        user_name,
        &[("team".to_string(), FIRST_TEAM.to_string())],
    )
    .await;
    for (policy_name, document) in [
        (
            "abac",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserPolicies","Resource":"{own}","Condition":{{"StringEquals":{{"{TEAM_KEY}":"{FIRST_TEAM}"}}}}}}]}}"#
            ),
        ),
        (
            "self-service",
            shared_document(&user_resource("${aws:username}")),
        ),
    ] {
        let (status, put) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture PutUserPolicy {policy_name:?} must succeed, got {status}: {put}"
        );
    }

    /// One `ContextEntries` member: the key, and the single value supplied for it.
    fn entry(key: &str, value: &str) -> Vec<(String, String)> {
        vec![
            (
                "ContextEntries.member.1.ContextKeyName".to_string(),
                key.to_string(),
            ),
            (
                "ContextEntries.member.1.ContextKeyValues.member.1".to_string(),
                value.to_string(),
            ),
            (
                "ContextEntries.member.1.ContextKeyType".to_string(),
                "string".to_string(),
            ),
        ]
    }

    // Rows: the key supplied, the value supplied for it, the action asked about, and the
    // decision the rule requires. They span both ways a value is read — as a condition
    // clause's and as a `Resource` variable's — and, for each, a value that holds and one
    // that does not, including a `*` that must be matched as the character it is.
    let rows = [
        (
            "the tag value the condition admits",
            TEAM_KEY,
            FIRST_TEAM,
            "iam:ListUserPolicies",
            ALLOWED,
        ),
        (
            "another tag value",
            TEAM_KEY,
            SECOND_TEAM,
            "iam:ListUserPolicies",
            IMPLICIT_DENY,
        ),
        (
            "the user name the Resource variable expands to",
            "aws:username",
            user_name,
            "iam:ListAccessKeys",
            ALLOWED,
        ),
        (
            "a star supplied as the variable's value, which is that character",
            "aws:username",
            "*",
            "iam:ListAccessKeys",
            IMPLICIT_DENY,
        ),
        (
            "another user's name supplied as the variable's value",
            "aws:username",
            "somebody-else",
            "iam:ListAccessKeys",
            IMPLICIT_DENY,
        ),
    ];
    for (case, key, value, action, expected) in rows {
        let (status, body) = simulate(&router, &arn, &[action], &[&own], &entry(key, value)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            vec![(action.to_string(), expected.to_string())],
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // No value for the key the condition tests: nothing is guessed — the decision is the
    // implicit deny and the key is named as missing.
    let (status, body) = simulate(
        &router,
        &arn,
        &["iam:ListUserPolicies"],
        &[&own],
        &[],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListUserPolicies".to_string(), IMPLICIT_DENY.to_string())],
        "expected an undecidable condition to grant nothing, got {body}"
    );
    assert!(
        body.contains(&format!("<MissingContextValues><member>{TEAM_KEY}</member>")),
        "expected {TEAM_KEY} to be reported as a missing context value, got {body}"
    );
}
