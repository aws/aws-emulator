// SPDX-License-Identifier: CC0-1.0

//! In what ORDER the sign-up door's two refusals are decided, and what a refused call
//! leaves behind.
//!
//! RULE: the modelled constraints on a request are checked at the boundary FIRST, so a
//! request that breaks one is refused 400 `ValidationException` whether or not it arrived
//! through the console; a request that satisfies them is then admitted only when it
//! carries the console CSRF header, and otherwise refused 403 with the fixed text — and
//! because that decision is made from the request alone, ahead of every read and every
//! write, a refused sign-up mints no `associateInProgressToken` and records no pending
//! signup, so nothing it could have started is finalizable afterwards.
//!
//! The rows span the `accessToken` length against its declared maximum
//! (`smithy_query(query="shape", shape="WhatsAppSignupCallback")`: `accessToken` is
//! `@length max 1000`, `callbackUrl` `@length max 100`) — well inside it, one under it, at
//! it, one past it and far past it — crossed with whether the call came through the
//! console, because the length check does not depend on the door and the door does not
//! depend on the length.

use serde_json::{json, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::state::ComAmazonawsSocialmessagingSocialmessagingState as Store;

use crate::harness::{
    call, console_call, emulator_with_engine, setup_body, Answer, CONSOLE_CSRF_TOKEN,
    INVALID_PARAMETERS_ERROR_TYPE,
};

/// The error code the console door's refusal carries
/// (`CONSOLE_ACCESS_DENIED_ERROR_TYPE` in `src/scaffolded.rs`).
const CORAL_ACCESS_DENIED: &str =
    "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/";

/// What that refusal says (`CSRF_HEADER_MISSING_MESSAGE` in `src/state.rs`).
const CSRF_MISSING: &str = "CSRF header does not exist";

/// `WhatsAppSignupCallback.accessToken`'s declared maximum length, from the model.
const ACCESS_TOKEN_MAX: usize = 1000;

/// `WhatsAppSignupCallback.callbackUrl`'s declared maximum length, from the model.
const CALLBACK_URL_MAX: usize = 100;

/// The longest value a refusal clause quotes back (`MAX_QUOTED_VALUE` in
/// `src/service.rs`): past it the clause states the member alone.
const QUOTING_BOUND: usize = 512;

/// A sign-up callback carrying an `accessToken` of `len` characters.
fn callback_of(len: usize) -> Value {
    json!({
        "signupCallback": {
            "accessToken": "t".repeat(len),
            "callbackUrl": "https://example.com/callback",
        }
    })
}

/// The whole message a single-clause length refusal carries: the value is quoted back
/// only while it is short enough to carry.
fn too_long(member: &str, max: usize, value: &str) -> String {
    let subject = if value.len() > QUOTING_BOUND {
        format!("Value at '{member}'")
    } else {
        format!("Value '{value}' at '{member}'")
    };
    format!(
        "1 validation error detected: {subject} failed to satisfy constraint: \
         Member must have length less than or equal to {max}"
    )
}

#[tokio::test]
async fn the_modelled_length_is_checked_before_the_console_door_and_the_door_before_the_store()
{
    let (router, engine) = emulator_with_engine();

    // The lengths this rule generalises over: inside the maximum, one under it, at it,
    // one past it, far past it.
    let lengths = [
        1,
        ACCESS_TOKEN_MAX - 1,
        ACCESS_TOKEN_MAX,
        ACCESS_TOKEN_MAX + 1,
        ACCESS_TOKEN_MAX * 2,
    ];
    let mut admitted_tokens: Vec<String> = Vec::new();

    for len in lengths {
        let over_the_maximum = len > ACCESS_TOKEN_MAX;

        // Outside the console: a length violation is still a 400, and a well-formed
        // request is a 403 — never a 200.
        let case = format!("an accessToken of {len} characters, outside the console");
        let answer = call(&router, "POST", "/v1/whatsapp/signup", Some(callback_of(len))).await;
        if over_the_maximum {
            assert_validation(
                &answer,
                &case,
                "signupCallback.accessToken",
                ACCESS_TOKEN_MAX,
                &"t".repeat(len),
            );
        } else {
            assert_console_denied(&answer, &case);
        }

        // Through the console: the same length violation, and otherwise the sign-up runs.
        let case = format!("an accessToken of {len} characters, through the console");
        let answer =
            console_call(&router, "POST", "/v1/whatsapp/signup", Some(callback_of(len))).await;
        if over_the_maximum {
            assert_validation(
                &answer,
                &case,
                "signupCallback.accessToken",
                ACCESS_TOKEN_MAX,
                &"t".repeat(len),
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let token = answer.string_at("/signupCallbackResult/associateInProgressToken");
            assert!(
                !token.is_empty(),
                "case {case:?}: expected a minted associateInProgressToken, got {:?}",
                answer.body
            );
            admitted_tokens.push(token);
        }
    }

    // Only the admitted calls left a signup behind: the refused ones mint nothing, so the
    // store holds exactly one entry per 200 above.
    let recorded = engine.state::<Store>().lock().pending_signups.len();
    assert_eq!(
        recorded,
        admitted_tokens.len(),
        "after the whole table: expected the {} signups the console started, got {recorded}",
        admitted_tokens.len()
    );

    // And the refused calls started nothing finalizable: a finalization through the
    // console, carrying a token of the form a callback issues, is refused — the refused
    // callbacks issued none.
    let guesses = [
        format!("signup-{}", "0".repeat(32)),
        format!("signup-{}", "f".repeat(32)),
        CONSOLE_CSRF_TOKEN.to_string(),
    ];
    for guess in &guesses {
        let case = format!("finalizing the token {guess:?} no admitted callback issued");
        let answer = console_call(
            &router,
            "POST",
            "/v1/whatsapp/signup",
            Some(setup_body(
                guess,
                Some("waba-33333333333333333333333333333333"),
                &["phone-number-id-dddd"],
            )),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case {case:?}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.at("/linkedWhatsAppBusinessAccountId"),
            &Value::Null,
            "case {case:?}: expected no account to be linked, got {:?}",
            answer.body
        );
    }
    let linked = engine.state::<Store>().lock().linked_accounts.len();
    assert_eq!(
        linked, 0,
        "after the finalization attempts: expected no linked account, got {linked}"
    );
}

/// The same precedence over ANOTHER member of the same structure: `callbackUrl`'s own
/// declared maximum is checked at the boundary, and a request inside it meets the console
/// door.
#[tokio::test]
async fn a_second_members_length_is_checked_the_same_way() {
    let (router, _engine) = emulator_with_engine();
    let lengths = [1, CALLBACK_URL_MAX, CALLBACK_URL_MAX + 1, CALLBACK_URL_MAX * 3];

    for len in lengths {
        let url = "u".repeat(len);
        let body = json!({
            "signupCallback": {
                "accessToken": "meta-access-token",
                "callbackUrl": url,
            }
        });
        let case = format!("a callbackUrl of {len} characters, outside the console");
        let answer = call(&router, "POST", "/v1/whatsapp/signup", Some(body.clone())).await;
        if len > CALLBACK_URL_MAX {
            assert_validation(
                &answer,
                &case,
                "signupCallback.callbackUrl",
                CALLBACK_URL_MAX,
                &url,
            );
        } else {
            assert_console_denied(&answer, &case);
        }

        let case = format!("a callbackUrl of {len} characters, through the console");
        let answer = console_call(&router, "POST", "/v1/whatsapp/signup", Some(body)).await;
        if len > CALLBACK_URL_MAX {
            assert_validation(
                &answer,
                &case,
                "signupCallback.callbackUrl",
                CALLBACK_URL_MAX,
                &url,
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// A boundary refusal: 400 `ValidationException`, with the whole clause the length
/// violation states — the value quoted back only while the service can carry it.
fn assert_validation(answer: &Answer, case: &str, member: &str, max: usize, value: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.body
    );
    let expected = too_long(member, max, value);
    let actual = answer.message();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
    assert_eq!(
        answer.at("/signupCallbackResult"),
        &Value::Null,
        "case {case:?}: a refused sign-up mints no token, got {:?}",
        answer.body
    );
}

/// The console door's refusal: 403, the access-denied error code, the fixed text, and
/// nothing minted.
fn assert_console_denied(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected 403, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, CORAL_ACCESS_DENIED,
        "case {case:?}: expected the error type {CORAL_ACCESS_DENIED:?}, got {:?}",
        answer.error_type
    );
    let actual = answer.message();
    assert_eq!(
        actual, CSRF_MISSING,
        "case {case:?}: expected the message {CSRF_MISSING:?}, got {actual:?}"
    );
    assert_eq!(
        answer.at("/signupCallbackResult"),
        &Value::Null,
        "case {case:?}: a refused sign-up mints no token, got {:?}",
        answer.body
    );
}
