// SPDX-License-Identifier: CC0-1.0

//! The account password policy and the operations that SET a console password, end to end
//! over the awsQuery wire.
//!
//! RULE: `UpdateAccountPasswordPolicy` replaces the account's password policy wholesale,
//! and every operation that sets an IAM user's console password evaluates the password it
//! carries against the policy in force — a password that breaks one of the policy's content
//! rules (its minimum length, or a character class it requires) is REFUSED and nothing is
//! written, so the user is left with the password it already had, or with none at all.
//!
//! The refusal differs by operation only in its channel, because that is what the two
//! families were measured answering: the administrative writes (`CreateLoginProfile`,
//! `UpdateLoginProfile`) answer the modelled `PasswordPolicyViolationException` — HTTP 400,
//! awsQuery code `PasswordPolicyViolation` — whose message NAMES the rule that was broken,
//! while `ChangePassword`, which a user makes about its own password, answers HTTP 403
//! `AccessDenied` with the one fixed sentence that covers a policy violation and a stale
//! old password alike, so a caller cannot read out of it which of the two failed.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: which rule
//! of the policy a password breaks, the minimum length the policy sets (both published
//! bounds of `minimumPasswordLengthType` among them), which character class it demands, the
//! symbol a password satisfies the symbol rule with, and which of the three password-setting
//! operations carries the password. No single request is the specification.
//!
//! The awsQuery harness (`router`, `call`, `call_signed_by`, `element`, `error_code`) is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.
//!
//! The read and the DELETE of that same account record are driven here too: the read reports
//! the policy the account configured, the delete removes it, and an account holding none
//! answers each of them with the modelled `NoSuchEntity` worded for the request it refuses.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// The awsQuery error code the administrative writes answer a password the policy does not
/// admit with: `PasswordPolicyViolationException`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="CreateLoginProfile")`), carried with HTTP
/// 400.
const VIOLATION_CODE: &str = "PasswordPolicyViolation";

/// The code `ChangePassword` answers the same condition with. See this module's header.
const CHANGE_REFUSED_CODE: &str = "AccessDenied";

/// The WHOLE sentence a refused `ChangePassword` carries, spelled out here rather than read
/// from the handler so a change of the wording fails this suite instead of following it. It
/// interpolates nothing at all: one sentence covers both the policy violation and a stale
/// `OldPassword`.
const CHANGE_REFUSED_MESSAGE: &str = "Either the new password does not conform to the \
     account password policy or the old password was incorrect.";

/// The published floor of `minimumPasswordLengthType`'s range, 6..128
/// (`smithy_query(query="shape", shape="minimumPasswordLengthType")`), which is also the
/// minimum an account that never named one holds: "if you do not specify a value for this
/// parameter, then the operation uses the default value of `6`".
const MIN_MINIMUM_PASSWORD_LENGTH: usize = 6;

/// The published ceiling of that range. See [`MIN_MINIMUM_PASSWORD_LENGTH`].
const MAX_MINIMUM_PASSWORD_LENGTH: usize = 128;

/// Every symbol `PasswordPolicy$RequireSymbols` lists as satisfying that rule: "! @ # $ % ^
/// & * ( ) _ + - = [ ] { } | '". Written out here as the catalog this suite reads the rule
/// over, so each one is exercised rather than only the first.
const POLICY_SYMBOLS: &[char] = &[
    '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+', '-', '=', '[', ']', '{', '}', '|',
    '\'',
];

/// The sentence AWS names the symbol rule with, in the spelling
/// `PasswordPolicy$RequireSymbols` writes the set out in.
const SYMBOL_RULE_MESSAGE: &str =
    "Password should have at least one of the following symbols: ! @ # $ % ^ & * ( ) _ + - = \
     [ ] { } | '";

/// One rule of the policy, as a row states it: the policy to put in force, a password that
/// breaks exactly that rule, the sentence naming it, and a password the same policy admits.
struct Rule {
    case: &'static str,
    /// The `UpdateAccountPasswordPolicy` parameters this row sets.
    policy: Vec<(&'static str, String)>,
    /// A password the policy above refuses.
    refused: String,
    /// The whole message that refusal carries.
    message: String,
    /// A password the policy above admits, so the row shows the policy discriminating
    /// rather than refusing everything.
    admitted: String,
}

/// A password long enough for any row's minimum, carrying one of every character class, so
/// a row that is about ONE rule can state the admitted password from it.
fn conforming_password(length: usize) -> String {
    let base = "Aa1!";
    let mut password = String::from(base);
    while password.chars().count() < length {
        password.push('x');
    }
    password
}

/// The rows: one per content rule the policy can state, plus the two published bounds of
/// the minimum length and a policy stating every rule at once.
fn rules() -> Vec<Rule> {
    let shortest = MIN_MINIMUM_PASSWORD_LENGTH;
    let longest = MAX_MINIMUM_PASSWORD_LENGTH;
    vec![
        Rule {
            case: "the minimum length at the floor of the published range",
            policy: vec![("MinimumPasswordLength", shortest.to_string())],
            refused: "a".repeat(shortest - 1),
            message: format!("Password should have a minimum length of {shortest}"),
            admitted: "a".repeat(shortest),
        },
        Rule {
            case: "a minimum length inside the published range",
            policy: vec![("MinimumPasswordLength", "24".to_string())],
            refused: "a".repeat(23),
            message: "Password should have a minimum length of 24".to_string(),
            admitted: "a".repeat(24),
        },
        Rule {
            case: "the minimum length at the ceiling of the published range",
            policy: vec![("MinimumPasswordLength", longest.to_string())],
            refused: "a".repeat(longest - 1),
            message: format!("Password should have a minimum length of {longest}"),
            // `passwordType` is modelled len 1..128, so the ceiling of the minimum is the
            // only length that both satisfies this policy and is a legal password.
            admitted: "a".repeat(longest),
        },
        Rule {
            case: "a required symbol",
            policy: vec![("RequireSymbols", "true".to_string())],
            refused: "noSymbols1".to_string(),
            message: SYMBOL_RULE_MESSAGE.to_string(),
            admitted: "withSymbol1!".to_string(),
        },
        Rule {
            case: "a required numeric character",
            policy: vec![("RequireNumbers", "true".to_string())],
            refused: "noDigitsHere!".to_string(),
            message: "Password should have at least one numeric character (0 to 9)".to_string(),
            admitted: "oneDigit7!".to_string(),
        },
        Rule {
            case: "a required uppercase character",
            policy: vec![("RequireUppercaseCharacters", "true".to_string())],
            refused: "no uppercase 1!".to_string(),
            message: "Password should have at least one uppercase character (A to Z)".to_string(),
            admitted: "One Uppercase 1!".to_string(),
        },
        Rule {
            case: "a required lowercase character",
            policy: vec![("RequireLowercaseCharacters", "true".to_string())],
            refused: "NO LOWERCASE 1!".to_string(),
            message: "Password should have at least one lowercase character (a to z)".to_string(),
            admitted: "ONE lOWERCASE 1!".to_string(),
        },
        Rule {
            case: "every rule at once, refused by the first the password breaks",
            policy: vec![
                ("MinimumPasswordLength", "12".to_string()),
                ("RequireSymbols", "true".to_string()),
                ("RequireNumbers", "true".to_string()),
                ("RequireUppercaseCharacters", "true".to_string()),
                ("RequireLowercaseCharacters", "true".to_string()),
            ],
            // Short AND missing every class: the rules are taken in the order
            // `PasswordPolicy` declares them, so the length is the one reported.
            refused: "short".to_string(),
            message: "Password should have a minimum length of 12".to_string(),
            admitted: conforming_password(12),
        },
    ]
}

/// Put `policy` in force as the account's password policy.
async fn set_policy(router: &MultiServiceRouter, case: &str, policy: &[(&str, String)]) {
    let params: Vec<(&str, &str)> = policy
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let (status, body) = call(router, "UpdateAccountPasswordPolicy", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateAccountPasswordPolicy must be served, got {status}: {body}"
    );
}

/// Create `user`, with no console password.
async fn create_user(router: &MultiServiceRouter, case: &str, user: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
}

/// One `CreateLoginProfile`.
async fn create_login_profile(
    router: &MultiServiceRouter,
    user: &str,
    password: &str,
) -> (u16, String) {
    call(
        router,
        "CreateLoginProfile",
        &[("UserName", user), ("Password", password)],
    )
    .await
}

/// One `UpdateLoginProfile` carrying a password.
async fn update_login_profile(
    router: &MultiServiceRouter,
    user: &str,
    password: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateLoginProfile",
        &[("UserName", user), ("Password", password)],
    )
    .await
}

/// `GetLoginProfile` for `user`.
async fn read_profile(router: &MultiServiceRouter, user: &str) -> (u16, String) {
    call(router, "GetLoginProfile", &[("UserName", user)]).await
}

/// Mint an access key for `user`, with the one grant a signed `ChangePassword` needs, and
/// answer that key's id — which is what `ChangePassword` resolves the calling user from.
async fn signing_key(router: &MultiServiceRouter, case: &str, user: &str) -> String {
    crate::identity_policy_enforcement::grant_action(router, case, user, "iam:ChangePassword")
        .await;
    let (status, minted) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {minted}"
    );
    element(&minted, "AccessKeyId")
}

/// One `ChangePassword` presented as signed by `akid`.
async fn change_password(
    router: &MultiServiceRouter,
    akid: &str,
    old_password: &str,
    new_password: &str,
) -> (u16, String) {
    call_signed_by(
        router,
        Some(akid),
        "ChangePassword",
        &[
            ("OldPassword", old_password),
            ("NewPassword", new_password),
        ],
    )
    .await
}

/// RULE: `CreateLoginProfile` refuses a password the account's policy does not admit with
/// HTTP 400 `PasswordPolicyViolation`, naming the rule that was broken, and creates no login
/// profile — so the user still has none, and a later create carrying a password the policy
/// DOES admit is served.
#[tokio::test]
async fn a_create_is_refused_by_the_rule_the_password_breaks() {
    for (row, rule) in rules().into_iter().enumerate() {
        // One router per row, so each row's policy is the only one in force.
        let router = router();
        set_policy(&router, rule.case, &rule.policy).await;
        let user = format!("creator{row}");
        create_user(&router, rule.case, &user).await;

        let (status, body) = create_login_profile(&router, &user, &rule.refused).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the create to be refused, got {status}: {body}",
            rule.case
        );
        assert_eq!(
            error_code(&body),
            VIOLATION_CODE,
            "case {:?}: expected {VIOLATION_CODE}, got {body}",
            rule.case
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, rule.message,
            "case {:?}: expected the refusal to name the rule broken, got {message:?}",
            rule.case
        );
        assert!(
            !body.contains(&rule.refused),
            "case {:?}: the refusal must not echo the password: {body}",
            rule.case
        );

        // The cascade the refusal has to leave behind: the user has NO login profile, so
        // the read reports the absence the family words that way.
        let (status, body) = read_profile(&router, &user).await;
        assert_eq!(
            status, 404,
            "case {:?}: a refused create must create no login profile, got {status}: {body}",
            rule.case
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity, got {body}",
            rule.case
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Login Profile for User {user} cannot be found."),
            "case {:?}: expected the family's absence wording, got {message:?}",
            rule.case
        );

        // And the create the policy admits is served, on the very user the refused one
        // named — not refused as a profile that already exists.
        let (status, body) = create_login_profile(&router, &user, &rule.admitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected a conforming create to be served, got {status}: {body}",
            rule.case
        );
        let reported = element(&body, "UserName");
        assert_eq!(
            reported, user,
            "case {:?}: the created profile must report its user, got {reported:?}",
            rule.case
        );
        let (status, body) = read_profile(&router, &user).await;
        assert_eq!(
            status, 200,
            "case {:?}: the created profile must read back, got {status}: {body}",
            rule.case
        );
    }
}

/// RULE: `UpdateLoginProfile` evaluates the same policy, answers the same refusal, and
/// leaves the password in force untouched — so the password the create set still verifies
/// afterwards.
#[tokio::test]
async fn an_update_is_refused_by_the_rule_the_password_breaks() {
    for (row, rule) in rules().into_iter().enumerate() {
        let router = router();
        let user = format!("updater{row}");
        create_user(&router, rule.case, &user).await;
        // The password in force is set BEFORE the policy, so the row is about the update
        // the policy refuses rather than about the create.
        let in_force = conforming_password(MAX_MINIMUM_PASSWORD_LENGTH);
        let (status, body) = create_login_profile(&router, &user, &in_force).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture create must succeed, got {status}: {body}",
            rule.case
        );
        let akid = signing_key(&router, rule.case, &user).await;
        set_policy(&router, rule.case, &rule.policy).await;

        let (status, body) = update_login_profile(&router, &user, &rule.refused).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the update to be refused, got {status}: {body}",
            rule.case
        );
        assert_eq!(
            error_code(&body),
            VIOLATION_CODE,
            "case {:?}: expected {VIOLATION_CODE}, got {body}",
            rule.case
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, rule.message,
            "case {:?}: expected the refusal to name the rule broken, got {message:?}",
            rule.case
        );

        // The refused update wrote nothing: the password the create set is still the one in
        // force, which is what a change presenting it as the current password proves.
        let (status, body) = change_password(&router, &akid, &in_force, &rule.admitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: the password in force must be the one the create set, got {status}: \
             {body}",
            rule.case
        );

        // And an update carrying a password the policy admits is served.
        let (status, body) = update_login_profile(&router, &user, &rule.admitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected a conforming update to be served, got {status}: {body}",
            rule.case
        );
    }
}

/// RULE: `ChangePassword` evaluates the same policy and refuses a `NewPassword` it does not
/// admit with HTTP 403 `AccessDenied` and the one fixed sentence — which names no rule, so a
/// caller cannot tell it from a stale `OldPassword` — leaving the password in force, which a
/// change to a conforming password then replaces.
#[tokio::test]
async fn a_change_is_refused_without_naming_the_rule() {
    for (row, rule) in rules().into_iter().enumerate() {
        let router = router();
        let user = format!("changer{row}");
        create_user(&router, rule.case, &user).await;
        let in_force = conforming_password(MAX_MINIMUM_PASSWORD_LENGTH);
        let (status, body) = create_login_profile(&router, &user, &in_force).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture create must succeed, got {status}: {body}",
            rule.case
        );
        let akid = signing_key(&router, rule.case, &user).await;
        set_policy(&router, rule.case, &rule.policy).await;

        let (status, body) = change_password(&router, &akid, &in_force, &rule.refused).await;
        assert_eq!(
            status, 403,
            "case {:?}: expected the change to be refused, got {status}: {body}",
            rule.case
        );
        assert_eq!(
            error_code(&body),
            CHANGE_REFUSED_CODE,
            "case {:?}: expected {CHANGE_REFUSED_CODE}, got {body}",
            rule.case
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, CHANGE_REFUSED_MESSAGE,
            "case {:?}: expected the one fixed sentence, got {message:?}",
            rule.case
        );
        assert!(
            !message.contains(&user) && !body.contains(&rule.refused),
            "case {:?}: the refusal must disclose neither the user nor the password: {body}",
            rule.case
        );

        // The refused change left the password in force, so a change that presents it and
        // chooses a conforming new password is served.
        let (status, body) = change_password(&router, &akid, &in_force, &rule.admitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected a conforming change to be served, got {status}: {body}",
            rule.case
        );
    }
}

/// RULE (the control): an account that has never set a password policy enforces none, so
/// every password `passwordType` admits is accepted — including one at the published
/// minimum length of 1.
#[tokio::test]
async fn an_account_with_no_policy_admits_every_modelled_password() {
    let router = router();

    // Rows spanning the class `passwordType` admits: its published minimum length, a value
    // far below any policy minimum, and its published maximum.
    let passwords = [
        ("the published minimum length", "~".to_string()),
        ("one character of each class", "Aa1!".to_string()),
        ("the published maximum length", "p".repeat(128)),
    ];
    for (row, (case, password)) in passwords.iter().enumerate() {
        let user = format!("unpoliced{row}");
        create_user(&router, case, &user).await;
        let (status, body) = create_login_profile(&router, &user, password).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with no policy in force the create must be served, got {status}: \
             {body}"
        );
    }
}

/// RULE: `UpdateAccountPasswordPolicy` "does not support partial updates … if you do not
/// specify a parameter, that parameter's value reverts to its default value" — so a second
/// write REPLACES the policy rather than merging into it, and a rule the second write does
/// not state stops being enforced.
#[tokio::test]
async fn a_second_write_replaces_the_whole_policy() {
    let router = router();

    // In force first: a long minimum plus every character class.
    set_policy(
        &router,
        "the first policy",
        &[
            ("MinimumPasswordLength", "24".to_string()),
            ("RequireSymbols", "true".to_string()),
            ("RequireNumbers", "true".to_string()),
            ("RequireUppercaseCharacters", "true".to_string()),
            ("RequireLowercaseCharacters", "true".to_string()),
        ],
    )
    .await;
    create_user(&router, "the first policy", "replaced").await;
    let (status, body) = create_login_profile(&router, "replaced", "Aa1!shortish").await;
    assert_eq!(
        status, 400,
        "case \"the first policy\": a 12-character password must break the 24-character \
         minimum, got {status}: {body}"
    );

    // The second write names ONE parameter, so every other rule reverts to its default:
    // the minimum falls back to 6 and no character class is required any more.
    set_policy(
        &router,
        "the second policy",
        &[("MinimumPasswordLength", "8".to_string())],
    )
    .await;
    let (status, body) = create_login_profile(&router, "replaced", "aaaaaaaa").await;
    assert_eq!(
        status, 200,
        "case \"the second policy\": the replaced policy must demand no character class, \
         got {status}: {body}"
    );

    // And the third write names NOTHING at all, so the minimum itself reverts to the
    // documented default of 6.
    set_policy(&router, "the third policy", &[]).await;
    create_user(&router, "the third policy", "defaulted").await;
    let (status, body) = create_login_profile(&router, "defaulted", "aaaaaa").await;
    assert_eq!(
        status, 200,
        "case \"the third policy\": a policy naming nothing must hold the documented \
         6-character minimum, got {status}: {body}"
    );
    // A user of its own, so what refuses the five-character password is the policy and not
    // the profile the row above created.
    create_user(&router, "the third policy", "defaulted-too").await;
    let (status, body) = create_login_profile(&router, "defaulted-too", "aaaaa").await;
    assert_eq!(
        status, 400,
        "case \"the third policy\": five characters must still break that minimum, got \
         {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("Password should have a minimum length of {MIN_MINIMUM_PASSWORD_LENGTH}"),
        "case \"the third policy\": expected the default minimum in the refusal, got {body}"
    );
}

/// RULE: the symbol rule is satisfied by EVERY symbol
/// `PasswordPolicy$RequireSymbols` lists, and by none outside that set — so the catalog the
/// refusal spells out is the catalog the check reads.
#[tokio::test]
async fn every_published_symbol_satisfies_the_symbol_rule() {
    let router = router();
    set_policy(
        &router,
        "symbols",
        &[("RequireSymbols", "true".to_string())],
    )
    .await;

    for (row, symbol) in POLICY_SYMBOLS.iter().enumerate() {
        let user = format!("symbol{row}");
        create_user(&router, &format!("the symbol {symbol:?}"), &user).await;
        let password = format!("passwordWith{symbol}");
        let (status, body) = create_login_profile(&router, &user, &password).await;
        assert_eq!(
            status, 200,
            "case {symbol:?}: a published symbol must satisfy the rule, got {status}: {body}"
        );
    }

    // A character the published set does NOT carry does not satisfy the rule, and the
    // refusal spells the set out.
    for (row, outside) in ['~', '"', '<', ':', ';', ',', '.', '?', '/', '\\']
        .iter()
        .enumerate()
    {
        let user = format!("outside{row}");
        create_user(&router, &format!("the character {outside:?}"), &user).await;
        let password = format!("passwordWith{outside}");
        let (status, body) = create_login_profile(&router, &user, &password).await;
        assert_eq!(
            status, 400,
            "case {outside:?}: a character outside the published set must not satisfy the \
             rule, got {status}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, SYMBOL_RULE_MESSAGE,
            "case {outside:?}: expected the refusal to spell the published set out, got \
             {message:?}"
        );
    }
}

/// The error code a read of a policy the account never set answers with, from
/// `NoSuchEntityException`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="GetAccountPasswordPolicy")`), carried with
/// HTTP 404.
const NO_SUCH_ENTITY_CODE: &str = "NoSuchEntity";

/// One `GetAccountPasswordPolicy`: the operation takes no input at all.
async fn read_policy(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "GetAccountPasswordPolicy", &[]).await
}

/// The value the response reports for the member `tag`, or `None` when it carries that
/// member not at all — which is how an unset optional member of `PasswordPolicy` is
/// reported.
fn member(body: &str, tag: &str) -> Option<String> {
    let found = elements(body, tag);
    assert!(
        found.len() <= 1,
        "expected at most one <{tag}> in the response, got {}: {body}",
        found.len()
    );
    found.into_iter().next()
}

/// RULE: `GetAccountPasswordPolicy` reports the password policy the account CONFIGURED —
/// every member as `UpdateAccountPasswordPolicy` left it, with `ExpirePasswords` derived
/// from the expiry age it set ("Returns true if MaxPasswordAge contains a value greater
/// than 0") — and an account that has never set a policy has none to read, which is the
/// modelled `NoSuchEntity` naming the account the policy would belong to.
///
/// The rows differ along what the account configured: nothing at all (the documented
/// defaults a write with no parameters leaves), a minimum length, each character class, an
/// expiry age, a reuse count, self-service, hard expiry, and every member at once.
#[tokio::test]
async fn the_account_password_policy_is_read_back_as_the_account_configured_it() {
    // An account that never set a policy: the read is refused rather than answering a
    // policy of defaults.
    let empty_account = router();
    let (status, body) = read_policy(&empty_account).await;
    assert_eq!(
        status, 404,
        "case \"no policy at all\": expected HTTP 404 for a policy nothing set, got \
         {status}: {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, NO_SUCH_ENTITY_CODE,
        "case \"no policy at all\": expected the code {NO_SUCH_ENTITY_CODE}, got {code:?}: \
         {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        format!(
            "The Password Policy with domain name {} cannot be found.",
            engine::emulator_account_id()
        ),
        "case \"no policy at all\": expected the refusal to name the account, got {message:?}"
    );

    // One row per shape of policy a write can leave behind: the parameters it carries, and
    // the members the read has to report for it.
    type Row = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static [(&'static str, &'static str)],
    );
    let rows: [Row; 9] = [
        (
            "a write carrying no parameter at all",
            &[],
            &[
                ("MinimumPasswordLength", "6"),
                ("RequireSymbols", "false"),
                ("RequireNumbers", "false"),
                ("RequireUppercaseCharacters", "false"),
                ("RequireLowercaseCharacters", "false"),
                ("AllowUsersToChangePassword", "false"),
                ("ExpirePasswords", "false"),
            ],
        ),
        (
            "a minimum length",
            &[("MinimumPasswordLength", "24")],
            &[("MinimumPasswordLength", "24"), ("ExpirePasswords", "false")],
        ),
        (
            "the published floor of the minimum length",
            &[("MinimumPasswordLength", "6")],
            &[("MinimumPasswordLength", "6")],
        ),
        (
            "the published ceiling of the minimum length",
            &[("MinimumPasswordLength", "128")],
            &[("MinimumPasswordLength", "128")],
        ),
        (
            "every character class required",
            &[
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
            ],
            &[
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
            ],
        ),
        (
            "an expiry age, which is what makes passwords expire",
            &[("MaxPasswordAge", "90")],
            &[("MaxPasswordAge", "90"), ("ExpirePasswords", "true")],
        ),
        (
            "a reuse count",
            &[("PasswordReusePrevention", "24")],
            &[
                ("PasswordReusePrevention", "24"),
                ("ExpirePasswords", "false"),
            ],
        ),
        (
            "self-service password changes",
            &[("AllowUsersToChangePassword", "true")],
            &[("AllowUsersToChangePassword", "true")],
        ),
        (
            "every member at once",
            &[
                ("MinimumPasswordLength", "14"),
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
                ("AllowUsersToChangePassword", "true"),
                ("MaxPasswordAge", "45"),
                ("PasswordReusePrevention", "12"),
                ("HardExpiry", "true"),
            ],
            &[
                ("MinimumPasswordLength", "14"),
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
                ("AllowUsersToChangePassword", "true"),
                ("MaxPasswordAge", "45"),
                ("PasswordReusePrevention", "12"),
                ("HardExpiry", "true"),
                ("ExpirePasswords", "true"),
            ],
        ),
    ];

    for (case, written, expected) in rows {
        // One router per row, so each row reads back only what its own write left.
        let router = router();
        let policy: Vec<(&str, String)> = written
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        set_policy(&router, case, &policy).await;

        let (status, body) = read_policy(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the configured policy to be served, got {status}: {body}"
        );
        for (tag, value) in expected {
            assert_eq!(
                member(&body, tag).as_deref(),
                Some(*value),
                "case {case:?}: expected <{tag}> to be {value:?}, got {body}"
            );
        }
        // A member the write did not name is reported as the account holds it, never as a
        // value this read invented: an expiry age and a reuse count the account never set
        // are absent, which is the documented default under which passwords never expire
        // and reuse is not prevented.
        for absent in ["MaxPasswordAge", "PasswordReusePrevention"] {
            if expected.iter().any(|(tag, _)| *tag == absent) {
                continue;
            }
            assert_eq!(
                member(&body, absent),
                None,
                "case {case:?}: expected <{absent}> to be absent, got {body}"
            );
        }

        // The read is a REPORT: reading again answers the same policy.
        let (again_status, again) = read_policy(&router).await;
        assert_eq!(
            (again_status, element(&again, "MinimumPasswordLength")),
            (status, element(&body, "MinimumPasswordLength")),
            "case {case:?}: a second read must answer the same policy, got {again}"
        );
    }
}

/// One `DeleteAccountPasswordPolicy`: the operation takes no input at all — "deletes the
/// password policy for the Amazon Web Services account. There are no parameters."
async fn delete_policy(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "DeleteAccountPasswordPolicy", &[]).await
}

/// The WHOLE sentence a delete of a policy the account does not hold carries, spelled out
/// here rather than read from the handler so a change of the wording fails this suite
/// instead of following it. It interpolates nothing: the delete addresses the
/// ACCOUNT-LEVEL policy, held under one fixed name, and its request carries no value.
const NO_ACCOUNT_POLICY_MESSAGE: &str = "The account policy with name PasswordPolicy cannot \
     be found.";

/// The sentence the READ of an absent policy carries instead — a different sentence, naming
/// the account the policy would belong to.
fn no_password_policy_message() -> String {
    format!(
        "The Password Policy with domain name {} cannot be found.",
        engine::emulator_account_id()
    )
}

/// Assert that `router`'s account holds no password policy at all: the read is refused with
/// the modelled `NoSuchEntity` naming the account, rather than answering a policy.
async fn assert_no_policy_held(router: &MultiServiceRouter, case: &str) {
    let (status, body) = read_policy(router).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from a read of a policy the account does not \
         hold, got {status}: {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, NO_SUCH_ENTITY_CODE,
        "case {case:?}: expected the code {NO_SUCH_ENTITY_CODE} from the read, got {code:?}: \
         {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        no_password_policy_message(),
        "case {case:?}: expected the read's refusal to name the account, got {message:?}"
    );
}

/// RULE: `DeleteAccountPasswordPolicy` REMOVES the account's one password-policy record, so
/// every later reader of it sees the account holding none — the read answers the modelled
/// `NoSuchEntity` rather than a policy — and a delete made when the account holds no policy
/// references nothing, which is that same modelled refusal, worded for the account-level
/// policy the request addresses.
///
/// The rows differ along what the account had configured when the delete arrived: nothing
/// at all, a bare write of defaults, each kind of member a write can set, and every member
/// at once. What the record HELD cannot change what deleting it leaves behind.
#[tokio::test]
async fn deleting_the_account_password_policy_leaves_the_account_holding_none() {
    // One row per shape of policy a write can leave behind, as the parameters that write
    // carries. The empty row is a write naming no parameter at all, which still leaves a
    // policy (of documented defaults) for the delete to remove.
    type Row = (&'static str, &'static [(&'static str, &'static str)]);
    let rows: [Row; 7] = [
        ("a write carrying no parameter at all", &[]),
        (
            "the published floor of the minimum length",
            &[("MinimumPasswordLength", "6")],
        ),
        (
            "the published ceiling of the minimum length",
            &[("MinimumPasswordLength", "128")],
        ),
        (
            "every character class required",
            &[
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
            ],
        ),
        (
            "an expiry age and hard expiry",
            &[("MaxPasswordAge", "90"), ("HardExpiry", "true")],
        ),
        (
            "a reuse count and self-service changes",
            &[
                ("PasswordReusePrevention", "24"),
                ("AllowUsersToChangePassword", "true"),
            ],
        ),
        (
            "every member at once",
            &[
                ("MinimumPasswordLength", "14"),
                ("RequireSymbols", "true"),
                ("RequireNumbers", "true"),
                ("RequireUppercaseCharacters", "true"),
                ("RequireLowercaseCharacters", "true"),
                ("AllowUsersToChangePassword", "true"),
                ("MaxPasswordAge", "45"),
                ("PasswordReusePrevention", "12"),
                ("HardExpiry", "true"),
            ],
        ),
    ];

    for (case, written) in rows {
        // One router per row, so each row deletes only what its own write left.
        let router = router();
        let policy: Vec<(&str, String)> = written
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        set_policy(&router, case, &policy).await;

        // The policy is in force before the delete, so the row shows the delete removing a
        // record that was there rather than a read that never worked.
        let (status, body) = read_policy(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the written policy to be served before the delete, got \
             {status}: {body}"
        );

        let (status, body) = delete_policy(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the held policy to be deletable, got {status}: {body}"
        );

        // The record is GONE: the read reports the account holding no policy.
        assert_no_policy_held(&router, case).await;

        // A second delete references nothing, which is the modelled refusal worded for the
        // account-level policy — a different sentence from the read's.
        let (status, body) = delete_policy(&router).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a repeat delete, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, NO_SUCH_ENTITY_CODE,
            "case {case:?}: expected the code {NO_SUCH_ENTITY_CODE} from a repeat delete, \
             got {code:?}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, NO_ACCOUNT_POLICY_MESSAGE,
            "case {case:?}: expected the repeat delete to name the account-level policy, \
             got {message:?}"
        );

        // The delete clears the record rather than disabling the feature: a fresh write is
        // held and read back, and is deletable in its turn.
        set_policy(
            &router,
            case,
            &[("MinimumPasswordLength", "20".to_string())],
        )
        .await;
        let (status, body) = read_policy(&router).await;
        assert_eq!(
            (status, member(&body, "MinimumPasswordLength").as_deref()),
            (200, Some("20")),
            "case {case:?}: expected a write after the delete to be read back, got {status}: \
             {body}"
        );
        let (status, body) = delete_policy(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-written policy to be deletable, got {status}: \
             {body}"
        );
        assert_no_policy_held(&router, case).await;
    }
}

/// RULE: a delete of a policy the account never set references nothing, so it is refused
/// with the modelled `NoSuchEntity` and writes nothing at all.
///
/// The rows differ along how many deletes in turn the account is sent: an account that
/// never set a policy answers every one of them the same way, since none of them changes
/// what it holds.
#[tokio::test]
async fn deleting_a_password_policy_the_account_never_set_is_refused() {
    for deletes in [1_usize, 2, 3] {
        let router = router();
        let case = format!("{deletes} delete(s) against an account that never set a policy");
        for attempt in 1..=deletes {
            let (status, body) = delete_policy(&router).await;
            assert_eq!(
                status, 404,
                "case {case:?}: attempt {attempt} expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, NO_SUCH_ENTITY_CODE,
                "case {case:?}: attempt {attempt} expected the code {NO_SUCH_ENTITY_CODE}, \
                 got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, NO_ACCOUNT_POLICY_MESSAGE,
                "case {case:?}: attempt {attempt} expected the account-level policy \
                 sentence, got {message:?}"
            );
        }
        // The refused deletes wrote nothing: the account still holds no policy, and a
        // write after them is held.
        assert_no_policy_held(&router, &case).await;
        set_policy(
            &router,
            &case,
            &[("MinimumPasswordLength", "16".to_string())],
        )
        .await;
        let (status, body) = read_policy(&router).await;
        assert_eq!(
            (status, member(&body, "MinimumPasswordLength").as_deref()),
            (200, Some("16")),
            "case {case:?}: expected a write after the refused deletes to be held, got \
             {status}: {body}"
        );
    }
}

/// RULE: the deleted policy is in force for NOBODY — the password checks every
/// password-setting operation makes read the same account record, so a password one of
/// the policy's content rules refused is admitted once the policy is deleted.
///
/// The rows differ along which rule the policy stated, and therefore which password it
/// refused: the minimum length, and each of the four character classes.
#[tokio::test]
async fn a_password_the_deleted_policy_refused_is_admitted_after_the_delete() {
    // One row per rule the policy can state: the rule, the parameters stating it, and a
    // password it refuses that the Amazon Web Services defaults admit — at least the
    // default minimum of six characters, with no character class required.
    type Row = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    );
    let rows: [Row; 5] = [
        (
            "a minimum length above the default",
            &[("MinimumPasswordLength", "24")],
            "Aa1!short",
        ),
        (
            "a required symbol",
            &[("RequireSymbols", "true")],
            "Password123",
        ),
        (
            "a required digit",
            &[("RequireNumbers", "true")],
            "Passwordxx!",
        ),
        (
            "a required uppercase character",
            &[("RequireUppercaseCharacters", "true")],
            "password1!",
        ),
        (
            "a required lowercase character",
            &[("RequireLowercaseCharacters", "true")],
            "PASSWORD1!",
        ),
    ];

    for (case, written, password) in rows {
        let router = router();
        let policy: Vec<(&str, String)> = written
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        set_policy(&router, case, &policy).await;

        let user = "console-user";
        create_user(&router, case, user).await;

        // Under the policy the password is refused, and no login profile is written.
        let (status, body) = create_login_profile(&router, user, password).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the policy to refuse {password:?}, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, VIOLATION_CODE,
            "case {case:?}: expected the code {VIOLATION_CODE}, got {code:?}: {body}"
        );

        let (status, body) = delete_policy(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the policy to be deletable, got {status}: {body}"
        );

        // With no policy in force the same password breaks no rule.
        let (status, body) = create_login_profile(&router, user, password).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {password:?} to be admitted once the policy is \
             deleted, got {status}: {body}"
        );
        let (status, profile) = read_profile(&router, user).await;
        assert_eq!(
            (status, element(&profile, "UserName")),
            (200, user.to_string()),
            "case {case:?}: expected the login profile to be readable back, got {status}: \
             {profile}"
        );
    }
}
