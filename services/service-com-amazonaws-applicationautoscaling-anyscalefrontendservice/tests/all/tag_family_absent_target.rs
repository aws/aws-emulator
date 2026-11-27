// SPDX-License-Identifier: CC0-1.0

//! One rule for the whole tagging family, over the wire: which answer a `ResourceARN` that
//! resolves to no registered scalable target earns.
//!
//! RULE under test: `ResourceARN` "identifies the Application Auto Scaling scalable target"
//! the call addresses, and which refusal an ARN identifying no REGISTERED target earns is
//! decided by whether it names a scalable-target IDENTITY of the endpoint serving the request —
//! its account and Region, and a last segment of the form a scalable target's identifier takes —
//! the same answer
//! whichever of the three operations asked:
//!
//! * an ARN naming such an identity is the resource reported
//!   ABSENT — the modelled `ResourceNotFoundException`, which
//!   `smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//!   declares on all three operations, carrying the fixed sentence "Scalable Target not found"
//!   and the ARN asked about in the shape's own `ResourceName` member. It is served at the
//!   client-error status this service answers that condition on, not at the 404 the shape's
//!   `@httpError` declares. Whether this service ever minted that ARN does not enter into it:
//!   what the caller asked about is a scalable target of this account and Region, and the
//!   answer is that there is none;
//! * an ARN naming no such identity — another account, Region or partition, or an id in no form
//!   an identifier takes — names no resource this endpoint could hold,
//!   so the call is
//!   refused without the service reporting on the resource at all: the modelled
//!   `ValidationException` and the fixed sentence "Access Denied" (the
//!   model says which error covers which condition for neither).
//!
//! The rows span what the rule generalises over: the OPERATION (the read and both mutations,
//! each with its own payload) crossed with how the ARN fails to resolve (an identifier this
//! service never minted, an id of another form, the shortest id the modelled pattern admits,
//! another account, another Region, another partition, and the ARN of a target that WAS
//! registered and has since been deregistered). A registered target stands beside them and
//! round-trips a tag through the same three operations, so no row passes because the family
//! refuses everything.

use serde_json::{json, Value};

use crate::emulator::{error_code, error_message, Emulator, ACCOUNT, REGION};

/// The sentence the refusal of an ARN naming an identity of this endpoint that no registered
/// target answers to carries, written out here rather than read off the
/// code under test: the WORDING is part of what this suite holds the family to.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The code that refusal carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole sentence the refusal of an ARN that names no identity of this endpoint carries.
const ACCESS_DENIED: &str = "Access Denied";

/// The code that refusal carries.
const VALIDATION: &str = "ValidationException";

/// The status both are served on.
const CLIENT_ERROR: u16 = 400;

/// One request per operation of the family, all three addressed to `resource_arn` with a
/// payload of its own: the answer is about the resource the ARN names, so neither the tags
/// written nor the keys removed change it.
fn family(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": {"env": "sim2real"}}),
        ),
        (
            "TagResource",
            json!({
                "ResourceARN": resource_arn,
                "Tags": {"env": "sim2real", "owner": "orders-team"},
            }),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env"]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env", "owner"]}),
        ),
        ("ListTagsForResource", json!({"ResourceARN": resource_arn})),
    ]
}

/// ARNs that satisfy the modelled `@pattern` on `ResourceARN`
/// (`^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$`), so each one
/// reaches the handler rather than the boundary validator, and none of them names a registered
/// target. `deregistered` is the ARN this service minted for a target that has since been
/// deregistered, so the table carries an ARN that WAS real beside ones that never were — and
/// each row states whether the ARN names an IDENTITY of this endpoint, which is what decides its
/// refusal.
fn unresolvable(deregistered: &str) -> Vec<(String, String, bool)> {
    vec![
        (
            "the ARN of a target that has been deregistered".to_string(),
            deregistered.to_string(),
            true,
        ),
        (
            "an id this service never minted".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            true,
        ),
        (
            "an id of another form".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/nothing-here"
            ),
            false,
        ),
        (
            "the shortest id the pattern admits".to_string(),
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/a"),
            false,
        ),
        (
            "an ARN of another account".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
        (
            "an ARN of another Region".to_string(),
            format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
        (
            "an ARN of another partition".to_string(),
            format!(
                "arn:aws-us-gov:application-autoscaling:us-gov-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
            false,
        ),
    ]
}

#[tokio::test]
async fn every_tag_operation_answers_an_unresolvable_arn_by_the_endpoint_it_names() {
    let emulator = Emulator::empty();

    // One target that stays registered, so the rows below are about the ARN they name, and one
    // that is deregistered to give the table the ARN of a target that was real.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let withdrawn = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;
    let withdrawn_arn = emulator.target_arn(&withdrawn).await;
    let (status, body) = emulator
        .call("DeregisterScalableTarget", withdrawn.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected HTTP 200, got {status} with {body:?}"
    );

    for (reason, arn, served_here) in unresolvable(&withdrawn_arn) {
        for (operation, request) in family(&arn) {
            let case = format!("{reason} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
            );
            let code = error_code(&body);
            let message = error_message(&body);
            if served_here {
                assert_eq!(
                    code, RESOURCE_NOT_FOUND,
                    "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {code:?} in {body:?}"
                );
                assert_eq!(
                    message, TARGET_NOT_FOUND,
                    "case {case:?}: expected {TARGET_NOT_FOUND:?}, got {message:?}"
                );
                assert_eq!(
                    body["ResourceName"],
                    Value::String(arn.clone()),
                    "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
                );
            } else {
                // An ARN naming no identity of this endpoint names no resource this
                // endpoint reports on, so the refusal states the access-denied sentence and
                // names nothing.
                assert_eq!(
                    code, VALIDATION,
                    "case {case:?}: expected {VALIDATION}, got {code:?} in {body:?}"
                );
                assert_eq!(
                    message, ACCESS_DENIED,
                    "case {case:?}: expected {ACCESS_DENIED:?}, got {message:?}"
                );
                assert!(
                    body.get("ResourceName").is_none(),
                    "case {case:?}: expected no resource to be named, got {body:?}"
                );
            }
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: expected the sentence to echo nothing of the ARN, got {message:?}"
            );
        }
    }

    // None of those refusals touched the store, and none of them was a blanket refusal: the
    // target that IS registered takes a tag, reports it back, and gives it up again — the whole
    // family round-tripping through the same three operations.
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"tag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"read the resident target's tags\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({"env": "sim2real"}),
        "case \"read the resident target's tags\": expected the tag just written, got {body:?}"
    );
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": resident_arn, "TagKeys": ["env"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"untag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"read the emptied tag set\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({}),
        "case \"read the emptied tag set\": expected an empty tag set, got {body:?}"
    );
}

/// The longest `ResourceARN` the family accepts, from the `@length` the model publishes on the
/// shape all three operations' `ResourceARN` targets — 1 to 1011 characters
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#AmazonResourceName")`).
const MAX_RESOURCE_ARN_CHARS: usize = 1011;

/// The member path a boundary constraint clause names `ResourceARN` by, which is the member's
/// own name as the wire protocol spells it.
const RESOURCE_ARN_PATH: &str = "resourceARN";

/// A scalable-target ARN of `account` in the Region serving the request, exactly `chars`
/// characters long: the id is padded out with characters the modelled `@pattern` admits
/// (`[a-zA-Z0-9-]`), so LENGTH is the only thing that varies from row to row.
fn arn_of_length(account: &str, chars: usize) -> String {
    let mut arn = format!("arn:aws:application-autoscaling:{REGION}:{account}:scalable-target/");
    assert!(
        arn.len() < chars,
        "case {chars:?}: expected room for an id of at least one character after {arn:?}"
    );
    while arn.len() < chars {
        arn.push('a');
    }
    arn
}

/// RULE: which refusal an unresolvable `ResourceARN` earns is decided by the IDENTITY it names,
/// so the length of the ARN does not enter into it — up to the published
/// `@length` maximum of the shape it targets, past which the request never reaches the handler
/// and is refused at the protocol boundary instead.
///
/// The rows walk the published bound: at the maximum, one character inside it and one character
/// past it, each crossed with an ARN of this endpoint's own account and one of another account,
/// and each asked on all three operations of the family. An ARN this long carries an id far
/// longer than a scalable target's identifier, so inside the bound every row names no identity
/// the service could hold and earns the refusal that reports on no resource.
#[tokio::test]
async fn the_family_answers_an_unresolvable_arn_alike_up_to_the_published_length_bound() {
    let emulator = Emulator::empty();

    // A registered target carrying a tag, so the rows below are about the ARN they name and so
    // the refusals can be shown to have left the store as they found it.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"tag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );

    // Inside the bound the answer is the one the rest of this suite holds the family to, whether
    // the ARN is the longest one the model admits or one character shorter.
    for chars in [MAX_RESOURCE_ARN_CHARS, MAX_RESOURCE_ARN_CHARS - 1] {
        for (reason, account) in [
            ("an ARN of this endpoint's account", ACCOUNT),
            ("an ARN of another account", "111122223333"),
        ] {
            let arn = arn_of_length(account, chars);
            assert_eq!(
                arn.len(),
                chars,
                "case {reason:?}: expected an ARN of {chars} characters, got {}",
                arn.len()
            );
            for (operation, request) in family(&arn) {
                let case = format!("{reason} of {chars} characters on {operation}");
                let (status, body) = emulator.call(operation, request).await;
                assert_eq!(
                    status, CLIENT_ERROR,
                    "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
                );
                let code = error_code(&body);
                let message = error_message(&body);
                assert_eq!(
                    code, VALIDATION,
                    "case {case:?}: expected {VALIDATION}, got {code:?} in {body:?}"
                );
                assert_eq!(
                    message, ACCESS_DENIED,
                    "case {case:?}: expected {ACCESS_DENIED:?}, got {message:?}"
                );
                assert!(
                    body.get("ResourceName").is_none(),
                    "case {case:?}: expected no resource to be named, got {body:?}"
                );
            }
        }
    }

    // One character past the published maximum the request breaks a constraint the MODEL
    // declares, so it is refused at the protocol boundary — the framework's constraint envelope,
    // quoting the value submitted, the member it was submitted at and the bound it had to
    // satisfy — rather than by the resolution that answers an ARN the handler reads. An ARN of
    // another account is refused there too, so the over-long row is about the bound and not
    // about the ARN's own account.
    for (reason, account) in [
        ("an ARN of this endpoint's account", ACCOUNT),
        ("an ARN of another account", "111122223333"),
    ] {
        let arn = arn_of_length(account, MAX_RESOURCE_ARN_CHARS + 1);
        let expected = format!(
            "1 validation error detected: Value '{arn}' at '{RESOURCE_ARN_PATH}' failed to \
             satisfy constraint: Member must have length less than or equal to \
             {MAX_RESOURCE_ARN_CHARS}"
        );
        for (operation, request) in family(&arn) {
            let case = format!("{reason} one character past the bound on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, VALIDATION,
                "case {case:?}: expected {VALIDATION}, got {code:?} in {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // None of those refusals touched the store: the registered target still carries the tag
    // written before them, so no row passed because the family stopped serving.
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"read the resident target's tags\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({"env": "sim2real"}),
        "case \"read the resident target's tags\": expected the refusals to change nothing, got \
         {body:?}"
    );
}
