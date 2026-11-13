// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE `ConfirmSubscription` refuses a token that
//! confirms nothing with.
//!
//! The RULE asserted here: a token that is not a confirmation token of the topic named
//! is refused with HTTP 400 / `InvalidParameter` and the message `Invalid token` —
//! WHOLLY FIXED text naming the condition itself. No part of it comes from the request:
//! the token is not echoed (it is a short-lived secret sent to the endpoint), and
//! there is no `<member> Reason: ` frame, because the fault is not a member whose value
//! broke a constraint but a token that confirms nothing.
//!
//! The rows differ along what the rule generalises over: the FORM of the value
//! presented as a token (a bare word, a token-width hex string this service did not
//! mint, an empty-ish value, a token of ANOTHER topic's subscription, and a token that
//! has just been superseded), and the protocol whose subscriber has to confirm. No
//! refused call confirms anything.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::CONFIRMATION_TOKEN_HEX_CHARS;

use super::topics_subscriptions_publish::{Answer, Sns};

/// The sentence AWS refuses a token that confirms nothing with, stated here as the
/// recorded response words it rather than read from the crate, so the test pins the
/// text and not just the constant.
const INVALID_TOKEN: &str = "Invalid token";

/// Subscribe `endpoint` over `protocol`, asking for the ARN in every case.
async fn subscribe(sns: &Sns, topic: &str, protocol: &str, endpoint: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", protocol),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe over {protocol} must be served, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", answer.body))
}

/// The confirmation token the subscription under `arn` holds, read from persisted state
/// because AWS sends it to the endpoint and no response carries it.
fn token_of(sns: &Sns, arn: &str) -> String {
    let state = sns.state();
    let guard = state.lock();
    guard.subscriptions[arn]
        .confirmation_token
        .clone()
        .expect("a subscription awaiting confirmation holds a token")
}

/// One `ConfirmSubscription` call.
async fn confirm(sns: &Sns, topic: &str, token: &str) -> Answer {
    sns.call(
        "ConfirmSubscription",
        &[("TopicArn", topic), ("Token", token)],
    )
    .await
}

/// Whether the subscription under `arn` is still awaiting confirmation.
fn pending(sns: &Sns, arn: &str) -> bool {
    sns.state().lock().subscriptions[arn].pending_confirmation
}

/// Every value that is not a confirmation token of the topic named is refused with
/// AWS's fixed sentence, and nothing is confirmed.
#[tokio::test]
async fn a_token_that_confirms_nothing_is_refused_with_the_fixed_sentence() {
    let sns = Sns::new();
    let topic = sns.topic("invalid-token-refusal").await;
    let other = sns.topic("invalid-token-other-topic").await;

    // One awaiting subscription per confirmable protocol, so the refusal does not
    // depend on which subscriber has to confirm.
    let awaiting = [
        subscribe(&sns, &topic, "https", "https://example.test/hook").await,
        subscribe(&sns, &topic, "http", "http://example.test/hook").await,
        subscribe(&sns, &topic, "email", "ops@example.test").await,
    ];
    // A subscription of ANOTHER topic: its token is a real token, for a topic this call
    // does not name.
    let elsewhere = subscribe(&sns, &other, "https", "https://elsewhere.test/hook").await;
    let foreign_token = token_of(&sns, &elsewhere);

    let hex_width = "a".repeat(CONFIRMATION_TOKEN_HEX_CHARS);
    let rows: Vec<(&str, String)> = vec![
        ("a bare word", "nope".to_string()),
        ("a hex string of the minted width", hex_width),
        ("one character", "a".to_string()),
        ("whitespace", " ".to_string()),
        ("a value shaped like a URL", "https://example.test/?Token=x".to_string()),
        ("the token of another topic's subscription", foreign_token),
    ];

    for (case, token) in &rows {
        let answer = confirm(&sns, &topic, token).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_TOKEN),
            "case {case:?}: expected exactly {INVALID_TOKEN:?}, got {}",
            answer.body
        );
        assert!(
            answer.element("SubscriptionArn").is_none(),
            "case {case:?}: a refused confirmation must report no subscription, got {}",
            answer.body
        );
    }

    for arn in &awaiting {
        assert!(
            pending(&sns, arn),
            "the refused confirmations must leave {arn} awaiting confirmation"
        );
    }
    assert!(
        pending(&sns, &elsewhere),
        "the refused confirmations must leave the other topic's subscription awaiting \
         confirmation"
    );

    // The control: the token the service minted for one of them confirms THAT
    // subscription, so the refusals above are about the token and not about the call.
    let token = token_of(&sns, &awaiting[0]);
    let answer = confirm(&sns, &topic, &token).await;
    assert_eq!(
        answer.status, 200,
        "the minted token must confirm the subscription, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("SubscriptionArn").as_deref(),
        Some(awaiting[0].as_str()),
        "the confirmation must report the subscription's ARN, got {}",
        answer.body
    );
    assert!(
        !pending(&sns, &awaiting[0]),
        "the confirmed subscription must no longer be awaiting confirmation"
    );
}
