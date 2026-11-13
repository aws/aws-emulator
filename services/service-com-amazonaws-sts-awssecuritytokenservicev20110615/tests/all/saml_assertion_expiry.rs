// SPDX-License-Identifier: CC0-1.0

//! `AssumeRoleWithSAML` refuses an authentication response whose validity window has closed,
//! and that refusal is a fact about the response rather than something the service spends.
//!
//! RULE: a presented SAML authentication response is exchanged for a role session only while
//! the clock is inside EVERY validity window the response states — the subject
//! confirmation's and the assertion's own conditions alike, the closing bound being exclusive
//! — and a response read after any one of those windows has closed is refused with
//! `ExpiredTokenException` at HTTP 400 and the fixed text AWS words that class with, vending
//! no credential and recording no session; because the refusal is derived from the response's
//! own timestamps and from nothing the service consumes, presenting the same response again
//! is answered identically, and a response still inside its windows is exchanged, reporting
//! the `NameID` it names as the `Subject` of the session it mints.
//!
//! The rows drive the real path end to end on ONE shared engine: the provider and the role
//! are registered through IAM's own operations, each response is signed HERE with the key
//! pair minted for the provider, and every call is made UNSIGNED, as a SAML client holding no
//! Amazon Web Services credential makes it. They differ along what the rule generalises over:
//! which of the two windows has closed, how long ago it closed (a second, an hour, a day, a
//! month), whether the window ever opened in the recent past at all, and — on the served side
//! — how much of the window is left and which name the response states.

use std::sync::Arc;

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState;

use super::saml_federation::{
    audience_condition, create_saml_provider, create_saml_role, emulator_with_iam, exchange,
    saml_assertion, IdentityProvider, Response,
};
use super::sts_sessions::{element, member};

/// The sentence AWS words this refusal class with: a response read outside the lifetime its
/// provider gave it.
const RESPONSE_EXPIRED_MESSAGE: &str = "Response has expired";

/// The modelled error such a response travels as, and the status it carries
/// (`ExpiredTokenException`, HTTP 400, from
/// `smithy_query(query="operation",operation="AssumeRoleWithSAML")`).
const EXPIRED_TOKEN_CODE: &str = "ExpiredTokenException";
const EXPIRED_TOKEN_STATUS: u16 = 400;

/// How many times a row presents the very same response: more than once, because the point
/// the rule makes is that the answer does not change with the presenting.
const PRESENTATIONS: usize = 3;

/// The sessions this service holds, which is what "no session was recorded" is read from.
fn sessions_held(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine.state::<StsState>().lock().sessions.len()
}

/// The trust this suite's rows are decided under: one provider, one role that admits it.
async fn trust(router: &MultiServiceRouter, name: &str) -> (IdentityProvider, String, String) {
    let provider = IdentityProvider::mint("https://idp.example.test/saml");
    let provider_arn = create_saml_provider(router, name, &provider).await;
    let role_arn =
        create_saml_role(router, &format!("role-{name}"), &provider_arn, audience_condition()).await;
    (provider, provider_arn, role_arn)
}

#[tokio::test]
async fn a_response_read_after_any_window_it_states_has_closed_is_refused_as_expired() {
    let (router, engine) = emulator_with_iam();
    let (provider, provider_arn, role_arn) = trust(&router, "expiry-idp").await;
    let valid = || Response::valid(&provider, &role_arn, &provider_arn);

    // Each row closes a window the response itself states, leaving everything else as the
    // positive control has it — in particular an `IssueInstant` inside the five minutes a
    // response may be redeemed in, so what is refused is the expiry and not the redemption.
    let rows: Vec<(&str, Response)> = vec![
        (
            "both windows closed an hour ago, the window having opened two hours ago",
            Response {
                not_before_offset: -7200,
                not_on_or_after_offset: Some(-3600),
                conditions_not_on_or_after_offset: Some(-3600),
                ..valid()
            },
        ),
        (
            "the subject confirmation window closed one second ago, the conditions still open",
            Response {
                not_on_or_after_offset: Some(-1),
                conditions_not_on_or_after_offset: Some(600),
                ..valid()
            },
        ),
        (
            "the assertion's own conditions closed a day ago, the subject confirmation open",
            Response {
                not_before_offset: -172_800,
                not_on_or_after_offset: Some(600),
                conditions_not_on_or_after_offset: Some(-86_400),
                ..valid()
            },
        ),
        (
            "both windows closed a month ago",
            Response {
                not_before_offset: -2_592_000,
                not_on_or_after_offset: Some(-2_591_000),
                conditions_not_on_or_after_offset: Some(-2_591_000),
                ..valid()
            },
        ),
        (
            // The closing bound is exclusive: a response is outside its window at the very
            // instant the window names, not a second later.
            "a window that closes this very second",
            Response {
                not_on_or_after_offset: Some(0),
                conditions_not_on_or_after_offset: Some(600),
                ..valid()
            },
        ),
        (
            "a window that closes this very second in the assertion's own conditions",
            Response {
                not_on_or_after_offset: Some(600),
                conditions_not_on_or_after_offset: Some(0),
                ..valid()
            },
        ),
    ];

    for (case, row) in &rows {
        let assertion = saml_assertion(&provider, row);
        let held_before = sessions_held(&engine);
        let mut answers = Vec::new();
        for presentation in 1..=PRESENTATIONS {
            let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion).await;
            assert_eq!(
                status, EXPIRED_TOKEN_STATUS,
                "case {case:?} presentation {presentation}: expected HTTP \
                 {EXPIRED_TOKEN_STATUS}, got {status} {body}"
            );
            assert!(
                body.contains("<ErrorResponse"),
                "case {case:?} presentation {presentation}: expected an awsQuery XML \
                 ErrorResponse, got {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some(EXPIRED_TOKEN_CODE),
                "case {case:?} presentation {presentation}: expected \
                 <Code>{EXPIRED_TOKEN_CODE}</Code>, got {body}"
            );
            let message = member(case, &body, "Message").replace("&amp;", "&");
            assert_eq!(
                message, RESPONSE_EXPIRED_MESSAGE,
                "case {case:?} presentation {presentation}: expected the message \
                 {RESPONSE_EXPIRED_MESSAGE:?}, got {message:?}"
            );
            assert!(
                element(&body, "Credentials").is_none(),
                "case {case:?} presentation {presentation}: a refused exchange must vend no \
                 credential, got {body}"
            );
            answers.push((status, element(&body, "Code"), message));
        }
        // Nothing about the refusal was spent: every presentation of the same response is
        // answered alike.
        let first = &answers[0];
        for (presentation, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, first,
                "case {case:?}: presentation {} must be answered exactly as the first was, \
                 expected {first:?}, got {answer:?}",
                presentation + 1
            );
        }
        let held_after = sessions_held(&engine);
        assert_eq!(
            held_after, held_before,
            "case {case:?}: a refused exchange must record no session, expected \
             {held_before} held, got {held_after}"
        );
    }

    // And the trust itself is untouched by any of it: a response still inside its windows is
    // exchanged after every refusal above.
    let assertion = saml_assertion(&provider, &valid());
    let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion).await;
    assert_eq!(
        status, 200,
        "a response inside its windows must still be exchanged after the refusals, got \
         {status} {body}"
    );
    assert!(
        !member("the control after the refusals", &body, "AccessKeyId").is_empty(),
        "the served exchange must vend a credential, got {body}"
    );
}

#[tokio::test]
async fn a_response_inside_its_windows_is_exchanged_and_reports_the_name_it_states() {
    let (router, engine) = emulator_with_iam();
    let (provider, provider_arn, role_arn) = trust(&router, "served-idp").await;
    let valid = || Response::valid(&provider, &role_arn, &provider_arn);

    // The served side of the same rule. The rows differ in how much of the window is left and
    // in the name the response states — every value drawn from the character class
    // `RoleSessionName in AuthnResponse must match [a-zA-Z_0-9+=,.@-]{2,64}`, which is what
    // the refusal of a name outside it quotes.
    let rows: Vec<(&str, &str, Response)> = vec![
        (
            "a window with a minute left",
            "saml-user",
            Response {
                name_id: "saml-user".to_string(),
                not_on_or_after_offset: Some(60),
                ..valid()
            },
        ),
        (
            "a window with an hour left, and a name written as an address",
            "user@example.test",
            Response {
                name_id: "user@example.test".to_string(),
                not_on_or_after_offset: Some(3600),
                conditions_not_on_or_after_offset: Some(3600),
                ..valid()
            },
        ),
        (
            "windows of different lengths, the shorter one still open",
            "a_b+c=d,e.f-2",
            Response {
                name_id: "a_b+c=d,e.f-2".to_string(),
                not_on_or_after_offset: Some(120),
                conditions_not_on_or_after_offset: Some(900),
                ..valid()
            },
        ),
    ];

    for (case, name, row) in &rows {
        let assertion = saml_assertion(&provider, row);
        let held_before = sessions_held(&engine);
        let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the exchange to be served, got {status} {body}"
        );
        let subject = member(case, &body, "Subject").replace("&amp;", "&");
        assert_eq!(
            subject, *name,
            "case {case:?}: expected the Subject {name:?} the response names, got {subject:?}"
        );
        assert!(
            !member(case, &body, "AccessKeyId").is_empty(),
            "case {case:?}: expected a credential, got {body}"
        );
        let held_after = sessions_held(&engine);
        assert_eq!(
            held_after,
            held_before + 1,
            "case {case:?}: a served exchange must record its session, expected \
             {} held, got {held_after}",
            held_before + 1
        );
    }
}
