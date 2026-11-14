// SPDX-License-Identifier: CC0-1.0

//! The condition CONTEXT a request to IAM is decided against, end to end over the awsQuery
//! wire.
//!
//! The rule under test: IAM authorizes every one of its own actions against the calling
//! principal's identity-based policies, and a statement's `Condition` is decided against
//! the context of the request being made — the tags of the signing principal
//! (`aws:PrincipalTag/<key>`), the tags of the TARGET the request addresses
//! (`aws:ResourceTag/<key>`, and IAM's own spelling `iam:ResourceTag/<key>`), the tags a
//! tagging call WRITES (`aws:RequestTag/<key>`) and the set of keys it writes
//! (`aws:TagKeys`), and the instant of the call (`aws:CurrentTime`, `aws:EpochTime`).
//! Several keys in one `Condition` block hold together. For a key the request carries NO
//! value for, IAM's documented asymmetry decides the clause: a positive comparison does not
//! hold, while the `…IfExists` spelling, a negated comparison and `Null: {key: true}` do —
//! and under a set qualifier `ForAllValues:` holds over the empty set while `ForAnyValue:`
//! does not. A refused write stores nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! condition key the clause names, which operator and which set qualifier it is written
//! with, whether the request carries a value for the key, whether the value matches, how
//! many keys the clause carries, which operation the decision is made for, and whether the
//! answer is wanted of a real signed call or of the policy simulator.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{reported, simulate, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY};
use crate::user_lifecycle::{account_id, call, call_signed_by, element, elements, error_code, router};

/// The ARN of an IAM user of this account.
pub(crate) fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// Create a user carrying `tags`, answering nothing: the fixture's own writes are made with
/// the emulator's own credentials, which carry no identity-based policies.
pub(crate) async fn create_user(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    tags: &[(&str, &str)],
) {
    let mut params: Vec<(String, String)> = vec![("UserName".to_string(), user_name.to_string())];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push((
            format!("Tags.member.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.member.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, body) = call(router, "CreateUser", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser {user_name:?} must succeed, got {status}: {body}"
    );
}

/// Mint an access key for `user_name`, answering the id a request signs with to speak for
/// it.
pub(crate) async fn access_key(router: &MultiServiceRouter, case: &str, user_name: &str) -> String {
    let (status, body) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey for {user_name:?} must succeed, got \
         {status}: {body}"
    );
    element(&body, "AccessKeyId")
}

/// Put the inline policy `policy_name` on `user_name`.
pub(crate) async fn put_policy(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutUserPolicy {policy_name:?} must succeed, got \
         {status}: {body}"
    );
}

/// The tags the account holds for `user_name`, read with the fixture's own credentials so
/// the read is never the thing under test.
pub(crate) async fn held_tags(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(router, "ListUserTags", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture ListUserTags must succeed, got {status}: {body}"
    );
    let keys = elements(&body, "Key");
    let values = elements(&body, "Value");
    keys.into_iter().zip(values).collect()
}

/// A one-statement document allowing `iam:TagUser` on `resource` under `condition`.
fn tag_user_grant(resource: &str, condition: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:TagUser","Resource":"{resource}","Condition":{condition}}}]}}"#
    )
}

/// One row of the request-tag table: the `Condition` the caller's grant carries, the tags
/// the call writes, and whether the call must be served.
struct TagCase {
    case: &'static str,
    condition: &'static str,
    /// The `(key, value)` pairs the `TagUser` call writes.
    written: &'static [(&'static str, &'static str)],
    /// Whether the policy in force permits that call.
    permitted: bool,
}

/// Rows spanning the keys a tagging request publishes and the operators a clause over one
/// is written with: the value the request carries (`aws:RequestTag/<key>`), the keys it
/// carries (`aws:TagKeys`), a key the request does NOT carry under each of the four
/// readings IAM's absent-key rule gives, and both set qualifiers over a key the request
/// carries two values for.
const TAG_CASES: &[TagCase] = &[
    TagCase {
        case: "StringEquals on the value the request writes",
        condition: r#"{"StringEquals":{"aws:RequestTag/env":"dev"}}"#,
        written: &[("env", "dev")],
        permitted: true,
    },
    TagCase {
        case: "StringEquals on a value the request does not write",
        condition: r#"{"StringEquals":{"aws:RequestTag/env":"dev"}}"#,
        written: &[("env", "prod")],
        permitted: false,
    },
    TagCase {
        case: "two clauses that hold together",
        condition: r#"{"StringEquals":{"aws:RequestTag/env":"dev","aws:RequestTag/owner":"ops"}}"#,
        written: &[("env", "dev"), ("owner", "ops")],
        permitted: true,
    },
    TagCase {
        case: "two clauses of which one fails",
        condition: r#"{"StringEquals":{"aws:RequestTag/env":"dev","aws:RequestTag/owner":"ops"}}"#,
        written: &[("env", "dev"), ("owner", "finance")],
        permitted: false,
    },
    TagCase {
        case: "StringLike on the value the request writes",
        condition: r#"{"StringLike":{"aws:RequestTag/env":"de*"}}"#,
        written: &[("env", "dev")],
        permitted: true,
    },
    // The absent-key readings: the request writes a tier tag and names no env tag at all.
    TagCase {
        case: "a positive comparison over a key the request omits",
        condition: r#"{"StringEquals":{"aws:RequestTag/env":"dev"}}"#,
        written: &[("tier", "gold")],
        permitted: false,
    },
    TagCase {
        case: "the IfExists spelling over a key the request omits",
        condition: r#"{"StringEqualsIfExists":{"aws:RequestTag/env":"dev"}}"#,
        written: &[("tier", "gold")],
        permitted: true,
    },
    TagCase {
        case: "a negated comparison over a key the request omits",
        condition: r#"{"StringNotEquals":{"aws:RequestTag/env":"dev"}}"#,
        written: &[("tier", "gold")],
        permitted: true,
    },
    TagCase {
        case: "Null asking for the absence of a key the request omits",
        condition: r#"{"Null":{"aws:RequestTag/env":"true"}}"#,
        written: &[("tier", "gold")],
        permitted: true,
    },
    TagCase {
        case: "Null asking for the presence of a key the request omits",
        condition: r#"{"Null":{"aws:RequestTag/env":"false"}}"#,
        written: &[("tier", "gold")],
        permitted: false,
    },
    TagCase {
        case: "Null asking for the presence of a key the request writes",
        condition: r#"{"Null":{"aws:RequestTag/env":"false"}}"#,
        written: &[("env", "dev")],
        permitted: true,
    },
    // The set qualifiers over the keys the request writes.
    TagCase {
        case: "ForAllValues over a key set the request stays inside",
        condition: r#"{"ForAllValues:StringEquals":{"aws:TagKeys":["env","owner"]}}"#,
        written: &[("env", "dev"), ("owner", "ops")],
        permitted: true,
    },
    TagCase {
        case: "ForAllValues over a key set the request steps outside",
        condition: r#"{"ForAllValues:StringEquals":{"aws:TagKeys":["env"]}}"#,
        written: &[("env", "dev"), ("owner", "ops")],
        permitted: false,
    },
    TagCase {
        case: "ForAnyValue over a key set the request meets once",
        condition: r#"{"ForAnyValue:StringEquals":{"aws:TagKeys":["env"]}}"#,
        written: &[("env", "dev"), ("owner", "ops")],
        permitted: true,
    },
    TagCase {
        case: "ForAnyValue over a key set the request never meets",
        condition: r#"{"ForAnyValue:StringEquals":{"aws:TagKeys":["cost-centre"]}}"#,
        written: &[("env", "dev"), ("owner", "ops")],
        permitted: false,
    },
];

/// RULE: a clause over the tags a `Tag*` call writes is decided from that call's own input,
/// so the combination the grant names is served and every other one is refused
/// `AccessDenied` with the target's tags untouched.
#[tokio::test]
async fn a_tagging_call_is_decided_against_the_tags_it_writes() {
    let router = router();
    let caller = "ctx-tag-caller";
    create_user(&router, "the caller", caller, &[]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    for (row, case) in TAG_CASES.iter().enumerate() {
        // A fresh target per row, so the tags a row finds are the ones it wrote.
        let target = format!("ctx-tag-target{row}");
        create_user(&router, case.case, &target, &[]).await;
        put_policy(
            &router,
            case.case,
            caller,
            "tagging",
            &tag_user_grant(&user_arn(&target), case.condition),
        )
        .await;

        let mut params: Vec<(String, String)> =
            vec![("UserName".to_string(), target.clone())];
        for (position, (key, value)) in case.written.iter().enumerate() {
            params.push((
                format!("Tags.member.{}.Key", position + 1),
                (*key).to_string(),
            ));
            params.push((
                format!("Tags.member.{}.Value", position + 1),
                (*value).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let (status, body) =
            call_signed_by(&router, Some(&signing_key), "TagUser", &borrowed).await;

        let held = held_tags(&router, case.case, &target).await;
        if case.permitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the grant to permit the call, got {status}: {body}",
                case.case
            );
            let written: Vec<(String, String)> = case
                .written
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                held, written,
                "case {:?}: expected the permitted call to store its tags, got {held:?}",
                case.case
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?}: expected the call to be refused, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {:?}: expected AccessDenied, got {:?}",
                case.case,
                error_code(&body)
            );
            assert!(
                held.is_empty(),
                "case {:?}: a refused write must store nothing, got {held:?}",
                case.case
            );
        }
    }
}

/// One row of the resource-tag table: the key the clause is written with, the tag the
/// TARGET carries, and whether the call must be served.
struct ResourceTagCase {
    case: &'static str,
    /// The condition key the clause names — the two spellings IAM publishes a resource tag
    /// under.
    key: &'static str,
    /// The value the target's `project` tag carries.
    target_tag: &'static str,
    /// Whether the policy in force permits the call.
    permitted: bool,
}

/// Rows spanning both spellings of the resource-tag key and both answers the comparison
/// against the CALLER's own tag can give.
const RESOURCE_TAG_CASES: &[ResourceTagCase] = &[
    ResourceTagCase {
        case: "aws:ResourceTag matching the caller's own tag",
        key: "aws:ResourceTag/project",
        target_tag: "alpha",
        permitted: true,
    },
    ResourceTagCase {
        case: "aws:ResourceTag differing from the caller's own tag",
        key: "aws:ResourceTag/project",
        target_tag: "beta",
        permitted: false,
    },
    ResourceTagCase {
        case: "iam:ResourceTag matching the caller's own tag",
        key: "iam:ResourceTag/project",
        target_tag: "alpha",
        permitted: true,
    },
    ResourceTagCase {
        case: "iam:ResourceTag differing from the caller's own tag",
        key: "iam:ResourceTag/project",
        target_tag: "beta",
        permitted: false,
    },
];

/// The reads this suite asks of a target user, so no row's answer can come from one
/// operation's own quirk.
const TARGET_READS: [(&str, &str); 3] = [
    ("ListAccessKeys", "iam:ListAccessKeys"),
    ("ListUserTags", "iam:ListUserTags"),
    ("GetUser", "iam:GetUser"),
];

/// RULE: a clause over the TARGET's tags is decided from the tags that target holds at the
/// instant of the call, compared against the value the caller's own tags give a policy
/// variable — so one stored document admits the targets whose tag matches the caller's and
/// refuses the rest.
#[tokio::test]
async fn a_read_is_decided_against_the_targets_tags() {
    let router = router();
    let caller = "ctx-abac-caller";
    create_user(&router, "the caller", caller, &[("project", "alpha")]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    for (action, iam_action) in TARGET_READS {
        for (row, case) in RESOURCE_TAG_CASES.iter().enumerate() {
            let target = format!("ctx-abac-{}{row}", action.to_ascii_lowercase());
            create_user(
                &router,
                case.case,
                &target,
                &[("project", case.target_tag)],
            )
            .await;
            // The grant names every user of the account and narrows itself by the
            // comparison of the two tags, which is the ABAC pattern: nothing in the
            // document names a user.
            let document = format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{iam_action}","Resource":"arn:aws:iam::{account}:user/*","Condition":{{"StringEquals":{{"{key}":"${{aws:PrincipalTag/project}}"}}}}}}]}}"#,
                account = account_id(),
                key = case.key,
            );
            put_policy(&router, case.case, caller, "abac", &document).await;

            let (status, body) = call_signed_by(
                &router,
                Some(&signing_key),
                action,
                &[("UserName", &target)],
            )
            .await;
            if case.permitted {
                assert_eq!(
                    status, 200,
                    "case {:?}/{action}: expected the matching tag to permit the read, got \
                     {status}: {body}",
                    case.case
                );
            } else {
                assert_eq!(
                    (status, error_code(&body)),
                    (403, "AccessDenied".to_string()),
                    "case {:?}/{action}: expected the differing tag to refuse the read, got \
                     {status}: {body}",
                    case.case
                );
            }
        }
    }
}

/// RULE: the clause is decided from the tags held NOW, so retagging either side changes
/// what the same unchanged request decides.
#[tokio::test]
async fn retagging_either_side_changes_the_same_requests_answer() {
    let router = router();
    let caller = "ctx-retag-caller";
    let target = "ctx-retag-target";
    create_user(&router, "the caller", caller, &[("project", "alpha")]).await;
    create_user(&router, "the target", target, &[("project", "beta")]).await;
    let signing_key = access_key(&router, "the caller", caller).await;
    put_policy(
        &router,
        "retagging",
        caller,
        "abac",
        &format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserTags","Resource":"arn:aws:iam::{account}:user/*","Condition":{{"StringEquals":{{"aws:ResourceTag/project":"${{aws:PrincipalTag/project}}"}}}}}}]}}"#,
            account = account_id(),
        ),
    )
    .await;

    /// One probe of the same unchanged request.
    async fn probe(router: &MultiServiceRouter, signing_key: &str, target: &str) -> u16 {
        let (status, _) = call_signed_by(
            router,
            Some(signing_key),
            "ListUserTags",
            &[("UserName", target)],
        )
        .await;
        status
    }

    assert_eq!(
        probe(&router, &signing_key, target).await,
        403,
        "the tags must not match before either side is retagged"
    );

    // Retag the TARGET onto the caller's project: the same request is now permitted.
    let (status, body) = call(
        &router,
        "TagUser",
        &[
            ("UserName", target),
            ("Tags.member.1.Key", "project"),
            ("Tags.member.1.Value", "alpha"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture retag of the target must succeed, got {status}: {body}"
    );
    assert_eq!(
        probe(&router, &signing_key, target).await,
        200,
        "a target retagged onto the caller's project must be readable"
    );

    // Retag the CALLER instead: the comparison fails again, from the other side.
    let (status, body) = call(
        &router,
        "TagUser",
        &[
            ("UserName", caller),
            ("Tags.member.1.Key", "project"),
            ("Tags.member.1.Value", "gamma"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture retag of the caller must succeed, got {status}: {body}"
    );
    assert_eq!(
        probe(&router, &signing_key, target).await,
        403,
        "a caller retagged off the target's project must lose the read"
    );

    // Untagging the caller takes the variable's value away, and an element naming a
    // variable with no value matches nothing.
    let (status, body) = call(
        &router,
        "UntagUser",
        &[("UserName", caller), ("TagKeys.member.1", "project")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture untag of the caller must succeed, got {status}: {body}"
    );
    assert_eq!(
        probe(&router, &signing_key, target).await,
        403,
        "a caller holding no project tag must not match any target"
    );
}

/// One row of the date table: the operator the clause is written with, the instant it
/// names, and whether the call must be served.
struct DateCase {
    case: &'static str,
    operator: &'static str,
    /// The instant the clause names, in one of the two forms IAM admits.
    instant: &'static str,
    /// Whether the policy in force permits the call at the time of the call, which is
    /// after every past instant below and before every future one.
    permitted: bool,
}

/// Rows spanning the date operators and both sides of the instant of the call, in both the
/// ISO 8601 and the epoch-seconds form AWS documents for a date condition's value ("you
/// must specify date/time values with one of the W3C implementations of the ISO 8601 date
/// formats or in epoch (UNIX) time").
const DATE_CASES: &[DateCase] = &[
    DateCase {
        case: "DateGreaterThan a past instant",
        operator: "DateGreaterThan",
        instant: "2020-01-01T00:00:00Z",
        permitted: true,
    },
    DateCase {
        case: "DateGreaterThan a future instant",
        operator: "DateGreaterThan",
        instant: "2099-01-01T00:00:00Z",
        permitted: false,
    },
    DateCase {
        case: "DateLessThan a future instant",
        operator: "DateLessThan",
        instant: "2099-01-01T00:00:00Z",
        permitted: true,
    },
    DateCase {
        case: "DateLessThan a past instant",
        operator: "DateLessThan",
        instant: "2020-01-01T00:00:00Z",
        permitted: false,
    },
    DateCase {
        case: "DateNotEquals a past instant",
        operator: "DateNotEquals",
        instant: "2020-01-01T00:00:00Z",
        permitted: true,
    },
    DateCase {
        case: "DateEquals a past instant",
        operator: "DateEquals",
        instant: "2020-01-01T00:00:00Z",
        permitted: false,
    },
    DateCase {
        case: "DateGreaterThan a past instant in epoch seconds",
        operator: "DateGreaterThan",
        instant: "1577836800",
        permitted: true,
    },
    DateCase {
        case: "DateLessThanEquals a past instant carrying a UTC offset",
        operator: "DateLessThanEquals",
        instant: "2020-01-01T00:00:00+02:00",
        permitted: false,
    },
];

/// RULE: a date clause is decided against the instant of the call, whichever of the forms
/// IAM admits its value is written in.
#[tokio::test]
async fn a_date_clause_is_decided_against_the_instant_of_the_call() {
    let router = router();
    let caller = "ctx-clock-caller";
    create_user(&router, "the caller", caller, &[]).await;
    let signing_key = access_key(&router, "the caller", caller).await;

    for case in DATE_CASES {
        for key in ["aws:CurrentTime", "aws:EpochTime"] {
            // `aws:EpochTime` carries the same instant as epoch seconds, so a clause over
            // it decides exactly as the same clause over `aws:CurrentTime` does.
            put_policy(
                &router,
                case.case,
                caller,
                "clock",
                &format!(
                    r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:GetUser","Resource":"*","Condition":{{"{}":{{"{key}":"{}"}}}}}}]}}"#,
                    case.operator, case.instant,
                ),
            )
            .await;
            let (status, body) = call_signed_by(
                &router,
                Some(&signing_key),
                "GetUser",
                &[("UserName", caller)],
            )
            .await;
            let expected = if case.permitted { 200 } else { 403 };
            assert_eq!(
                status, expected,
                "case {:?}/{key}: expected HTTP {expected}, got {status}: {body}",
                case.case
            );
        }
    }
}

/// The keys one simulation result reports as missing a value, in report order.
///
/// `MissingContextValues` is a member list, so the shared leaf reader is taken over each
/// list's own text rather than over the element that carries it.
fn missing_keys(body: &str) -> Vec<String> {
    const OPEN: &str = "<MissingContextValues>";
    const CLOSE: &str = "</MissingContextValues>";
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(OPEN) {
        let text = &rest[start + OPEN.len()..];
        let end = text
            .find(CLOSE)
            .unwrap_or_else(|| panic!("{OPEN} is never closed in {body}"));
        found.extend(elements(&text[..end], "member"));
        rest = &text[end..];
    }
    found
}

/// One row of the simulated-context table: the clause the `Deny` is guarded by, the value
/// the request supplies for its key, the decision the simulation must report and the keys
/// it must report as missing.
struct SimulatedCase {
    case: &'static str,
    /// The `Condition` the `Deny` on `iam:ListUserTags` carries.
    condition: &'static str,
    /// The value the request supplies for `aws:SourceVpce`, when it supplies one.
    supplied: Option<&'static str>,
    /// The decision reported for `iam:ListUserTags`.
    decision: &'static str,
    /// Whether the REAL signed call for `iam:ListUserTags` is refused.
    ///
    /// A real call carries no value for a key nothing fills — nothing in a signed IAM
    /// request supplies `aws:SourceVpce` — so such a clause is decided by the absent-key
    /// rule whatever the simulation was handed: the deny applies where the absence
    /// satisfies its clause, and does not where the absence falsifies it. A clause this
    /// evaluation cannot decide keeps the refusal. A key IAM fills from the CONNECTION is
    /// different: a real call carries a value for it, so its clause is decided against
    /// that value.
    real_call_refused: bool,
    /// The context keys the result must report as missing.
    missing: &'static [&'static str],
}

/// Rows spanning the readings an absent key gets under a `Deny`'s clause and the two ways
/// the request can settle it: a negated operator the absence itself satisfies (so the deny
/// applies and nothing was needed), the same clause with the value supplied, the positive
/// operator the absence makes false (so the deny does not apply, and the value the policy
/// needed is reported) — in its text and its ADDRESS spellings — and an operator this
/// evaluation does not implement, which no value settles, so the deny stands.
const SIMULATED_CASES: &[SimulatedCase] = &[
    SimulatedCase {
        case: "a negated operator over a key the request leaves out",
        condition: r#"{"StringNotEquals":{"aws:SourceVpce":"vpce-0abc"}}"#,
        supplied: None,
        decision: EXPLICIT_DENY,
        real_call_refused: true,
        missing: &[],
    },
    SimulatedCase {
        case: "the same clause with the matching value supplied",
        condition: r#"{"StringNotEquals":{"aws:SourceVpce":"vpce-0abc"}}"#,
        supplied: Some("vpce-0abc"),
        decision: ALLOWED,
        real_call_refused: true,
        missing: &[],
    },
    SimulatedCase {
        case: "the same clause with another value supplied",
        condition: r#"{"StringNotEquals":{"aws:SourceVpce":"vpce-0def"}}"#,
        supplied: Some("vpce-0abc"),
        decision: EXPLICIT_DENY,
        real_call_refused: true,
        missing: &[],
    },
    SimulatedCase {
        case: "the positive operator over a key the request leaves out",
        condition: r#"{"StringEquals":{"aws:SourceVpce":"vpce-0abc"}}"#,
        supplied: None,
        decision: ALLOWED,
        real_call_refused: false,
        missing: &["aws:SourceVpce"],
    },
    SimulatedCase {
        case: "an address operator over a key the request leaves out",
        condition: r#"{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}"#,
        supplied: None,
        decision: ALLOWED,
        // The SIMULATION was handed no address, so the positive clause is false and the
        // key is reported as one the decision needed. The REAL call does carry one — IAM
        // fills `aws:SourceIp` from the connection, and this emulator is reached from the
        // local host — which is outside the block the deny names, so the deny does not
        // apply to it either.
        real_call_refused: false,
        missing: &["aws:SourceIp"],
    },
    SimulatedCase {
        case: "an operator this evaluation cannot decide at all",
        condition: r#"{"BinaryEquals":{"aws:SourceVpce":"dnBjZS0wYWJj"}}"#,
        supplied: None,
        decision: EXPLICIT_DENY,
        real_call_refused: true,
        missing: &["aws:SourceVpce"],
    },
];

/// RULE: `MissingContextValues` reports the keys the simulated DECISION needed a value for
/// and the request supplied none — so a key whose clause the absence itself decides is not
/// reported, and the decision is the one that absence gives.
#[tokio::test]
async fn the_simulator_reports_only_the_keys_its_decision_needed() {
    let router = router();

    for (row, case) in SIMULATED_CASES.iter().enumerate() {
        let user_name = format!("ctx-sim{row}");
        create_user(&router, case.case, &user_name, &[]).await;
        let arn = user_arn(&user_name);
        put_policy(
            &router,
            case.case,
            &user_name,
            "allow",
            &format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["iam:GetUser","iam:ListUserTags"],"Resource":"{arn}"}}]}}"#
            ),
        )
        .await;
        put_policy(
            &router,
            case.case,
            &user_name,
            "deny-conditioned",
            &format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Action":"iam:ListUserTags","Resource":"{arn}","Condition":{}}}]}}"#,
                case.condition
            ),
        )
        .await;

        let context: Vec<(String, String)> = case
            .supplied
            .map(|value| {
                vec![
                    (
                        "ContextEntries.member.1.ContextKeyName".to_string(),
                        "aws:SourceVpce".to_string(),
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
            })
            .unwrap_or_default();
        let (status, body) = simulate(
            &router,
            &arn,
            &["iam:GetUser", "iam:ListUserTags"],
            &[&arn],
            &context,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            reported(&body),
            vec![
                ("iam:GetUser".to_string(), ALLOWED.to_string()),
                ("iam:ListUserTags".to_string(), case.decision.to_string()),
            ],
            "case {:?}: expected the unconditioned grant to stand and the conditioned deny \
             to be decided, got {body}",
            case.case
        );
        let missing: Vec<String> = case
            .missing
            .iter()
            .map(|key| (*key).to_string())
            .collect();
        // Reported per (action, resource) pair: the action the clause is about is the only
        // one that needed the key, so the other action's result reports none.
        let reported_missing = missing_keys(&body);
        assert_eq!(
            reported_missing,
            // One entry per result the key is reported on: the action's own result and its
            // one resource-specific result.
            missing
                .iter()
                .chain(missing.iter())
                .cloned()
                .collect::<Vec<String>>(),
            "case {:?}: expected the keys the decision needed, got {reported_missing:?}: \
             {body}",
            case.case
        );

        // The REAL signed call is decided the same way the simulation says it would be:
        // the two answers are about one set of documents, so a deny the simulator reports
        // is a refusal the call gets, and an action the simulator allows is served.
        let signing_key = access_key(&router, case.case, &user_name).await;
        let (status, served) = call_signed_by(
            &router,
            Some(&signing_key),
            "GetUser",
            &[("UserName", &user_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the action no statement denies must be served, got {status}: {served}",
            case.case
        );
        let (status, read) = call_signed_by(
            &router,
            Some(&signing_key),
            "ListUserTags",
            &[("UserName", &user_name)],
        )
        .await;
        // The real call is decided against the context IT carries: a deny over a key
        // nothing fills is decided by that absence, and one over a key IAM fills from the
        // connection is decided against the value it carries.
        if case.real_call_refused {
            assert_eq!(
                (status, error_code(&read)),
                (403, "AccessDenied".to_string()),
                "case {:?}: expected the real call to be refused as the simulation says, \
                 got {status}: {read}",
                case.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: expected the real call to be served as the simulation says, \
                 got {status}: {read}",
                case.case
            );
        }
    }
}

/// RULE: an action a statement in force neither grants nor denies is the implicit deny,
/// whatever the conditions the other statements carry — so a conditioned clause never
/// becomes a grant for an action nothing allows.
#[tokio::test]
async fn a_conditioned_grant_covers_only_the_actions_it_names() {
    let router = router();
    let user_name = "ctx-unnamed";
    create_user(&router, "an unnamed action", user_name, &[]).await;
    let arn = user_arn(user_name);
    put_policy(
        &router,
        "an unnamed action",
        user_name,
        "allow",
        &format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserTags","Resource":"{arn}","Condition":{{"StringEqualsIfExists":{{"aws:RequestTag/env":"dev"}}}}}}]}}"#
        ),
    )
    .await;

    let (status, body) = simulate(
        &router,
        &arn,
        &["iam:ListUserTags", "iam:ListAccessKeys"],
        &[&arn],
        &[],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            ("iam:ListUserTags".to_string(), ALLOWED.to_string()),
            ("iam:ListAccessKeys".to_string(), IMPLICIT_DENY.to_string()),
        ],
        "expected the named action to be granted and the other to be the implicit deny, \
         got {body}"
    );
}

/// The longest name a context key is named by, which is the bound the member naming one
/// publishes: `ContextKeyNameType` is length 5..256
/// (`smithy_query(query="shape",shape="ContextKeyNameType")`).
const SHORTEST_CONTEXT_KEY_NAME: usize = 5;
const LONGEST_CONTEXT_KEY_NAME: usize = 256;

/// A condition key name of exactly `characters` characters, opening with a service prefix so
/// it reads as a context key name and not as something else.
fn context_key_of(characters: usize) -> String {
    let prefix = "aws:k";
    format!(
        "{prefix}{}",
        "e".repeat(characters.saturating_sub(prefix.len()))
    )
}

/// RULE: a key the simulated decision needed a value for is reported only where its name is
/// a context key NAME — so a name within the published length is reported, at either bound,
/// and a name past it is not reported at all, since no request could have supplied a value
/// for a key that cannot be named.
#[tokio::test]
async fn only_a_name_a_context_key_can_have_is_reported_as_missing() {
    let router = router();
    // The rows are the published bounds and one character past the longest: the shortest
    // name the member admits, the longest, and the first that is no context key name.
    let rows: &[(&str, usize, bool)] = &[
        ("the shortest name a context key has", SHORTEST_CONTEXT_KEY_NAME, true),
        ("the longest name a context key has", LONGEST_CONTEXT_KEY_NAME, true),
        (
            "one character past the longest name",
            LONGEST_CONTEXT_KEY_NAME + 1,
            false,
        ),
    ];
    for (row, (case, characters, reportable)) in rows.iter().enumerate() {
        let user_name = format!("ctx-name{row}");
        create_user(&router, case, &user_name, &[]).await;
        let arn = user_arn(&user_name);
        let key = context_key_of(*characters);
        assert_eq!(
            key.len(),
            *characters,
            "case {case:?}: the row's key must be {characters} characters, got {}",
            key.len()
        );
        // A positive clause over a key the request supplies no value for is false, so the
        // deny does not apply and the key is one the decision needed a value for — the
        // reading `the_simulator_reports_only_the_keys_its_decision_needed` pins.
        put_policy(
            &router,
            case,
            &user_name,
            "allow",
            &format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUserTags","Resource":"{arn}"}}]}}"#
            ),
        )
        .await;
        put_policy(
            &router,
            case,
            &user_name,
            "deny-conditioned",
            &format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Action":"iam:ListUserTags","Resource":"{arn}","Condition":{{"StringEquals":{{"{key}":"anything"}}}}}}]}}"#
            ),
        )
        .await;

        let (status, body) = simulate(&router, &arn, &["iam:ListUserTags"], &[&arn], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            vec![("iam:ListUserTags".to_string(), ALLOWED.to_string())],
            "case {case:?}: the clause the absence falsifies must leave the grant standing, \
             got {body}"
        );
        let reported_missing = missing_keys(&body);
        let expected: Vec<String> = if *reportable {
            // One entry per result the key is reported on: the action's own result and its
            // one resource-specific result.
            vec![key.clone(), key.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_missing, expected,
            "case {case:?}: expected {} reported for a {characters}-character name, got \
             {reported_missing:?}",
            if *reportable { "the key" } else { "no key" }
        );
    }
}

/// How many context keys one simulation may report as missing a value
/// (`crate::state::simulated_decisions::MAX_SIMULATED_CONTEXT_KEYS`).
const MAX_REPORTABLE_MISSING_KEYS: usize = 100;

/// RULE: a simulation whose documents test more keys than one report may carry is refused
/// rather than answered with a report nothing bounds — at the bound it is still served.
#[tokio::test]
async fn a_report_past_the_reportable_key_bound_is_refused() {
    let router = router();
    // The bound itself and one key past it: the first is answered, the second refused.
    for (case, keys) in [
        ("as many keys as one report carries", MAX_REPORTABLE_MISSING_KEYS),
        ("one key past the bound", MAX_REPORTABLE_MISSING_KEYS + 1),
    ] {
        let user_name = format!("ctx-count{keys}");
        create_user(&router, case, &user_name, &[]).await;
        let arn = user_arn(&user_name);
        let clauses: Vec<String> = (0..keys)
            .map(|at| format!(r#""aws:key{at}":"anything""#))
            .collect();
        // Supplied with the request rather than attached: a document testing this many keys
        // is past the size an inline policy may have, and "an optional list of additional
        // policy documents to include in the simulation" is what carries it.
        let supplied = vec![(
            "PolicyInputList.member.1".to_string(),
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Action":"iam:ListUserTags","Resource":"{arn}","Condition":{{"StringEquals":{{{}}}}}}}]}}"#,
                clauses.join(",")
            ),
        )];

        let (status, body) =
            simulate(&router, &arn, &["iam:ListUserTags"], &[&arn], &supplied).await;
        if keys <= MAX_REPORTABLE_MISSING_KEYS {
            assert_eq!(
                status, 200,
                "case {case:?}: a report within the bound must be answered, got {status}: \
                 {body}"
            );
            assert_eq!(
                missing_keys(&body).len(),
                // One list per result the keys are reported on: the action's own and its
                // one resource-specific result.
                keys * 2,
                "case {case:?}: expected every key the decision needed, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a report past the bound must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
    }
}
