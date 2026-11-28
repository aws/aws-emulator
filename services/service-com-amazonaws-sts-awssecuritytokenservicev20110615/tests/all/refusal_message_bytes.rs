// SPDX-License-Identifier: CC0-1.0

//! The BYTES an STS refusal states, over the class of requests that provoke one.
//!
//! Two rules, each asserted over a table rather than one recorded request.
//!
//! **A member whose value the service does not disclose.** A constraint refusal names the
//! member it refused and echoes the value it was handed — that echo is where a caller sees
//! what to fix — except for a member the model marks `@sensitive`, whose clause states no
//! value at all: `Value at '<member path>' failed to satisfy constraint: <requirement>`.
//! It holds whatever the constraint was and whether the member was there: the `null` the
//! framework writes for an absent member is a value segment too, and the member AWS
//! redacts has none. A member the model does NOT mark sensitive keeps its value half,
//! absent (`Value null at …`) or present (`Value '<as sent>' at …`) — which is what makes
//! the rule one about `@sensitive` rather than about absence.
//!
//! **The escaping of the message's text.** XML asks a text node to escape `&` and `<`
//! (and `>` only where it would close a `]]>`); the quote characters are markup only
//! inside an attribute value, so a message's apostrophes reach the wire as themselves.
//! This is read off the RAW response body, because an XML parser hides the difference —
//! the member path inside a validation message is delimited by literal apostrophes, and so
//! is every other apostrophe an STS message carries, including the ones a handler's own
//! sentence puts there.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the
//! emulator serves with.

use super::sts_sessions::{call, element, emulator, member, seed_demo_role, LONG_TERM_KEY};

/// A role ARN of this account, long enough to satisfy `arnType`'s 20-character minimum, so
/// a row is never refused for the ARN it carries alongside the member under test.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/demo";

/// A SAML provider ARN of this account, satisfying the same minimum.
const PROVIDER_ARN: &str = "arn:aws:iam::000000000000:saml-provider/demo";

/// The error code STS answers a request it refused over the values it carried.
const VALIDATION_ERROR: &str = "ValidationError";

/// One row: the operation, the form fields the request carries, and the message the
/// refusal must state — in the bytes a client reading the raw body sees.
struct Refusal {
    case: &'static str,
    action: &'static str,
    fields: Vec<(&'static str, String)>,
    message: String,
}

/// The rows span what the two rules generalise over: which operation is called, which
/// member is refused, whether the model marks that member `@sensitive`, whether the member
/// was absent or present-but-outside-its-bound, and which constraint it broke.
///
/// The sensitive members are the ones this service's model marks
/// (`smithy_query(query="operation", operation="…")` reports `sensitive: true` for
/// `GetDelegatedAccessTokenRequest$TradeInToken`,
/// `AssumeRoleWithWebIdentityRequest$WebIdentityToken` and
/// `AssumeRoleWithSAMLRequest$SAMLAssertion`); the contrast rows name members of the same
/// requests that it does not.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a @sensitive required member that is absent",
            action: "GetDelegatedAccessToken",
            fields: vec![],
            message: "1 validation error detected: Value at 'tradeInToken' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
        Refusal {
            case: "another operation's @sensitive required member, absent",
            action: "AssumeRoleWithWebIdentity",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
            ],
            message: "1 validation error detected: Value at 'webIdentityToken' failed to \
                      satisfy constraint: Member must not be null"
                .to_string(),
        },
        Refusal {
            case: "a @sensitive member PRESENT and under its length minimum",
            action: "AssumeRoleWithWebIdentity",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
                ("WebIdentityToken", "abc".to_string()),
            ],
            message: "1 validation error detected: Value at 'webIdentityToken' failed to \
                      satisfy constraint: Member must have length greater than or equal to 4"
                .to_string(),
        },
        Refusal {
            case: "a third operation's @sensitive required member, absent",
            action: "AssumeRoleWithSAML",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("PrincipalArn", PROVIDER_ARN.to_string()),
            ],
            message: "1 validation error detected: Value at 'sAMLAssertion' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
        Refusal {
            case: "that member PRESENT and under its length minimum",
            action: "AssumeRoleWithSAML",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("PrincipalArn", PROVIDER_ARN.to_string()),
                ("SAMLAssertion", "ab".to_string()),
            ],
            message: "1 validation error detected: Value at 'sAMLAssertion' failed to satisfy \
                      constraint: Member must have length greater than or equal to 4"
                .to_string(),
        },
        // The contrast: a member the model does not mark sensitive states the value half
        // the framework wrote — the `null` of an absent member, and the bytes of a
        // present one.
        Refusal {
            case: "a required member that is absent and NOT @sensitive",
            action: "GetFederationToken",
            fields: vec![],
            message: "1 validation error detected: Value null at 'name' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
        Refusal {
            case: "a member PRESENT, under its minimum and NOT @sensitive",
            action: "GetFederationToken",
            fields: vec![("Name", "a".to_string())],
            message: "1 validation error detected: Value 'a' at 'name' failed to satisfy \
                      constraint: Member must have length greater than or equal to 2"
                .to_string(),
        },
        Refusal {
            case: "another operation's non-@sensitive member, present and under its minimum",
            action: "AssumeRole",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "a".to_string()),
            ],
            message: "1 validation error detected: Value 'a' at 'roleSessionName' failed to \
                      satisfy constraint: Member must have length greater than or equal to 2"
                .to_string(),
        },
    ]
}

/// Both rules over every row: the clause states a value only for a member the model does
/// not mark `@sensitive`, and the raw `<Message>` carries its apostrophes as themselves.
#[tokio::test]
async fn a_refusal_withholds_a_sensitive_members_value_and_states_apostrophes_as_themselves() {
    let (router, engine) = emulator();
    // `AssumeRole` resolves its role in IAM, so the role the rows name is one this account
    // holds: a row must be refused for the member under test and nothing else.
    let (_role_arn, _role_id) = seed_demo_role(&engine);

    for row in refusals() {
        let (status, body) = call(&router, LONG_TERM_KEY, row.action, &row.fields).await;
        let case = row.case;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(VALIDATION_ERROR),
            "case {case:?}: expected the {VALIDATION_ERROR} code, got {body}"
        );
        // The RAW text of the element: nothing unescapes it here, so the bytes asserted
        // are the bytes the client is sent.
        let message = member(case, &body, "Message");
        assert_eq!(
            message, row.message,
            "case {case:?}: expected {:?}, got {message:?}",
            row.message
        );
        assert!(
            !body.contains("&apos;"),
            "case {case:?}: an apostrophe is not markup in a text node, got {body}"
        );
    }
}

/// The response document with the text of every `<RequestId>` removed.
///
/// That id is minted for the response independently of what the request carried — the
/// `awsQuery` error envelope carries one for every refusal — so a run of characters inside
/// it is not an echo of a caller's value, while a short value written in the characters an
/// id is written in can appear inside one by chance. Stripping it leaves the echo check
/// reading the parts of the document a request's own bytes could reach.
fn without_request_ids(body: &str) -> String {
    const OPEN: &str = "<RequestId>";
    const CLOSE: &str = "</RequestId>";
    let mut stripped = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find(OPEN) {
        let after_open = open + OPEN.len();
        let Some(close) = rest[after_open..].find(CLOSE) else {
            break;
        };
        stripped.push_str(&rest[..after_open]);
        rest = &rest[after_open + close..];
    }
    stripped.push_str(rest);
    stripped
}

/// The value a @sensitive member was handed reaches no part of the response — the clause
/// that refused it, and the document around it.
///
/// The rows differ in what makes the value refusable (too short for the member's minimum,
/// and too long for its maximum) and in what the value is made of, including the
/// characters an XML document would have to escape.
#[tokio::test]
async fn a_refused_sensitive_member_is_echoed_nowhere_in_the_response() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);

    // `clientTokenType` is 4..20000 (`AssumeRoleWithWebIdentityRequest$WebIdentityToken`),
    // so a value on either side of that window is refused for its length.
    let rows: Vec<(&str, String)> = vec![
        ("under the minimum", "abc".to_string()),
        ("a single character", "z".to_string()),
        (
            "markup characters XML would have to escape",
            "a'<".to_string(),
        ),
        ("past the maximum", "t".repeat(20_001)),
    ];

    for (case, presented) in rows {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRoleWithWebIdentity",
            &[
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
                ("WebIdentityToken", presented.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}"
        );
        let message = member(case, &body, "Message");
        assert!(
            message.starts_with("1 validation error detected: Value at 'webIdentityToken' "),
            "case {case:?}: expected a clause stating no value, got {message:?}"
        );
        // The document the request's own bytes could reach: the envelope's request id is
        // minted for the response, so the strip below is what keeps this check about the
        // echo rather than about which characters that id happened to be written in.
        assert!(
            element(&body, "RequestId").is_some_and(|id| !id.trim().is_empty()),
            "case {case:?}: the awsQuery envelope must carry a RequestId, got {body}"
        );
        let document = without_request_ids(&body);
        assert!(
            !document.contains(&presented),
            "case {case:?}: the response must carry nothing of a @sensitive value, got \
             {document}"
        );
    }
}

/// The text rule is about the DOCUMENT, not about one layer of it: a message a HANDLER
/// wrote reaches the wire with its apostrophes as themselves too.
///
/// The rows are session policy documents a parser cannot read, which
/// `MalformedPolicyDocumentException` reports by naming the offending character between
/// apostrophes — so the sentence under test carries apostrophes the service put there
/// rather than ones the refusal envelope did.
#[tokio::test]
async fn a_message_a_handler_wrote_states_its_apostrophes_as_themselves() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);

    // (case, the document, the character the parser stops on)
    let rows: [(&str, &str, char); 3] = [
        ("a field name that is not quoted", "{this is not valid json", 't'),
        ("nothing that can start a value", "xyz", 'x'),
        ("a trailing comma before the close", r#"{"Version":"2012-10-17",}"#, '}'),
    ];

    for (case, policy, offender) in rows {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetFederationToken",
            &[
                ("Name", "probe".to_string()),
                ("Policy", policy.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("MalformedPolicyDocument"),
            "case {case:?}: expected the handler's own refusal, got {body}"
        );
        let message = member(case, &body, "Message");
        assert!(
            message.contains(&format!("('{offender}' (code {}))", u32::from(offender))),
            "case {case:?}: expected the offending character named between apostrophes, got \
             {message:?}"
        );
        assert!(
            !body.contains("&apos;"),
            "case {case:?}: an apostrophe is not markup in a text node, got {body}"
        );
    }
}
