// SPDX-License-Identifier: CC0-1.0

//! What an origin access control `Id` the service cannot resolve earns, end to end over HTTP.
//!
//! The rule under test: an operation addressed by an origin access control `Id`
//! (`GetOriginAccessControl`, `GetOriginAccessControlConfig`, `UpdateOriginAccessControl`,
//! `DeleteOriginAccessControl` — `smithy_query(query="operation",
//! operation="GetOriginAccessControl")` reports `Id` as a required `@httpLabel` on a plain
//! `string`, with no `@pattern` and no `@length`, and `errors: [AccessDenied,
//! NoSuchOriginAccessControl]`) answers that one code for every `Id` it cannot resolve, so the
//! MESSAGE is what tells two conditions apart. What decides it is the ALPHABET the value is
//! written in, and nothing more: a value written in the one CloudFront writes an entity
//! identifier in (decimal digits and uppercase letters) is an identifier of that mint, and
//! WHICH one was ever issued — and for which of the four kinds that share its namespace letter
//! — is a property of the STORE, so such an `Id` the account holds no control under is the
//! control reported ABSENT: "The specified origin access control does not exist." A value
//! written OUTSIDE that alphabet was never an identifier of this mint at all, so the route
//! reports the identifier it cannot READ ("Id format is invalid"). Both sentences are fixed
//! text: each echoes nothing of the value submitted.
//!
//! The rows span what the rule generalises over: the minted identifiers of TWO other kinds of
//! the identifier form a distribution, a streaming distribution, an origin access identity and
//! an origin access control share (a distribution's and an origin access identity's), a kind
//! of this service drawn from ANOTHER form (a public key), bodies wider and narrower than the
//! one this service issues, an unminted namespace letter, a body of digits alone, five values
//! written in no alphabet CloudFront mints an identifier in (a minted identifier in lower
//! case, a UUID, a bare word, a caller reference, a domain name), the control's own identifier
//! with a character changed, an identifier of the shared form that was never minted, and an
//! identifier this account really issued for a control a delete retired. Every row is driven
//! through all four operations, and the live resources are read back afterwards to show the
//! refusals touched nothing.
//!
//! The wire harness and the create helpers are the crate's own.

use smithy_server_runtime::MultiServiceRouter;

use crate::announced_location_uri::origin_access_identity_xml;
use crate::distribution_lifecycle::{call, config, create_distribution, emulator, Answer, API_VERSION};
use crate::origin_access_control_lifecycle::{create_control, get_control, table};
use crate::public_key_lifecycle::{create_key, pem, Key};

/// The sentence a control the store holds nothing for earns, spelled out here rather than taken
/// from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONTROL: &str = "The specified origin access control does not exist.";

/// The sentence a value written in no alphabet CloudFront mints an identifier in earns on the
/// same routes, under the same code and status — spelled out here for the same reason.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// One operation addressed by an origin access control `Id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    GetOriginAccessControl,
    GetOriginAccessControlConfig,
    UpdateOriginAccessControl,
    DeleteOriginAccessControl,
}

/// Every operation of this resource addressed by its `Id`.
const OPS: &[Op] = &[
    Op::GetOriginAccessControl,
    Op::GetOriginAccessControlConfig,
    Op::UpdateOriginAccessControl,
    Op::DeleteOriginAccessControl,
];

impl Op {
    /// Send this operation at `id`, carrying the live control's current version token so the
    /// conditional paths are never refused for a precondition, and a configuration the update
    /// accepts so it is never refused for its payload.
    async fn send(
        self,
        router: &MultiServiceRouter,
        id: &str,
        document: &str,
        version: &str,
    ) -> Answer {
        match self {
            Self::GetOriginAccessControl => get_control(router, id).await,
            Self::GetOriginAccessControlConfig => {
                call(
                    router,
                    "GET",
                    &format!("/{API_VERSION}/origin-access-control/{id}/config"),
                    None,
                    None,
                )
                .await
            }
            Self::UpdateOriginAccessControl => {
                call(
                    router,
                    "PUT",
                    &format!("/{API_VERSION}/origin-access-control/{id}/config"),
                    Some(document),
                    Some(version),
                )
                .await
            }
            Self::DeleteOriginAccessControl => {
                call(
                    router,
                    "DELETE",
                    &format!("/{API_VERSION}/origin-access-control/{id}"),
                    None,
                    Some(version),
                )
                .await
            }
        }
    }
}

/// The live resources these rows address, and the document and version token the conditional
/// operations submit.
struct Live {
    control_id: String,
    control_document: String,
    control_etag: String,
    distribution_id: String,
    origin_access_identity_id: String,
    public_key_id: String,
}

/// Stand up one origin access control, one distribution, one origin access identity and one
/// public key: the control is what the rows address, and the other three are the identifiers a
/// row submits in its place.
async fn live(router: &MultiServiceRouter) -> Live {
    let control = table()[0].with_name("the control these rows address", "oac-readability");
    let control_document = control.xml();
    let created = create_control(router, &control_document).await;
    assert_eq!(
        created.status, 201,
        "case creating the control: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let control_id = created.root().text_at("Id").unwrap_or_default().to_string();

    let distribution = create_distribution(
        router,
        &config("the distribution these rows name", "oac-readability").xml(),
    )
    .await;
    assert_eq!(
        distribution.status, 201,
        "case creating the distribution: expected HTTP 201, got {} with {}",
        distribution.status, distribution.raw
    );

    let identity = call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
        Some(&origin_access_identity_xml("oac-readability-oai")),
        None,
    )
    .await;
    assert_eq!(
        identity.status, 201,
        "case creating the origin access identity: expected HTTP 201, got {} with {}",
        identity.status, identity.raw
    );

    let key = create_key(
        router,
        &Key {
            case: "the public key these rows name".to_string(),
            caller_reference: "oac-readability-key".to_string(),
            name: "oac-readability-key".to_string(),
            encoded_key: pem(1, 'A'),
            comment: None,
        }
        .xml(),
    )
    .await;
    assert_eq!(
        key.status, 201,
        "case creating the public key: expected HTTP 201, got {} with {}",
        key.status, key.raw
    );

    Live {
        control_etag: created.header("etag").to_string(),
        control_document,
        control_id,
        distribution_id: distribution.root().text_at("Id").unwrap_or_default().to_string(),
        origin_access_identity_id: identity.root().text_at("Id").unwrap_or_default().to_string(),
        public_key_id: key.root().text_at("Id").unwrap_or_default().to_string(),
    }
}

/// Assert one refusal: its status, its code and — the point of this suite — its message.
fn assert_refusal(answer: &Answer, case: &str, message: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchOriginAccessControl",
        "case {case}: expected NoSuchOriginAccessControl, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case}: expected the sentence {message:?}, got {actual:?}"
    );
}

/// The identifiers of ANOTHER resource kind, live in this account: one of each other kind of
/// the shared identifier form, one of a kind drawn from another form, and values of that
/// alphabet at widths this service issues no identifier at.
fn issued_for_another_kind(live: &Live) -> Vec<(String, String)> {
    let held = &live.control_id;
    let body = &held[1..];
    let mut short = held.clone();
    short.pop();

    vec![
        (
            "a distribution's own minted identifier".to_string(),
            live.distribution_id.clone(),
        ),
        (
            "an origin access identity's own minted identifier".to_string(),
            live.origin_access_identity_id.clone(),
        ),
        (
            "a public key identifier, a kind drawn from another form".to_string(),
            live.public_key_id.clone(),
        ),
        ("a body one character short".to_string(), short),
        (
            "a body one character too long".to_string(),
            format!("{held}Z"),
        ),
        (
            "a far wider body than this service issues".to_string(),
            format!("E{body}{body}"),
        ),
        (
            "a namespace letter no kind of this service mints".to_string(),
            format!("W{body}"),
        ),
        (
            "a body of digits alone".to_string(),
            "0123456789012".to_string(),
        ),
    ]
}

/// The values that COULD address an origin access control — written in the form this service
/// issues one in — which no control answers to: each is the control reported ABSENT. The rows
/// vary how the value came to name no control.
fn no_resource_answers_to(live: &Live) -> Vec<(String, String)> {
    let held = &live.control_id;
    let body = &held[1..];
    let mut changed: Vec<char> = held.chars().collect();
    if let Some(last) = changed.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }

    vec![
        (
            "the control's own identifier with its last character changed".to_string(),
            changed.into_iter().collect(),
        ),
        (
            "an identifier of the shared form that was never minted".to_string(),
            format!("E{}", "0".repeat(body.len())),
        ),
    ]
}

/// The values that are written in no identifier alphabet CloudFront issues one in. No record of
/// any kind could ever answer to one, so each is the identifier the route reports it cannot
/// READ rather than a resource it looked for and did not find. The rows vary HOW the alphabet
/// is broken.
fn outside_the_identifier_alphabet(live: &Live) -> Vec<(String, String)> {
    vec![
        (
            "the control's own identifier in lower case".to_string(),
            live.control_id.to_lowercase(),
        ),
        (
            "a UUID, the namespace the policy kinds are keyed by".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a bare word".to_string(), "not-an-identifier".to_string()),
        (
            "the control's caller reference rather than its identifier".to_string(),
            "oac-readability".to_string(),
        ),
        (
            "a domain name rather than an identifier".to_string(),
            "d111111abcdef8.cloudfront.net".to_string(),
        ),
    ]
}

/// RULE: an identifier this account issued for ANOTHER kind of resource names no origin
/// access control, and neither does a value of no width or alphabet this service mints one
/// in: each is resolved against this kind's own store and reported as the control that is
/// ABSENT.
#[tokio::test]
async fn another_kinds_identifier_is_the_absent_control() {
    let router = emulator();
    let live = live(&router).await;

    for op in OPS {
        for (case, id) in issued_for_another_kind(&live) {
            let answer = op
                .send(&router, &id, &live.control_document, &live.control_etag)
                .await;
            assert_refusal(
                &answer,
                &format!("{op:?} with {case} ({id:?})"),
                NO_SUCH_CONTROL,
            );
        }
    }

    // Every refusal above touched nothing: the control is still readable at its own identifier.
    let read = get_control(&router, &live.control_id).await;
    assert_eq!(
        read.status, 200,
        "case the live control: expected it to survive every refusal, got {} with {}",
        read.status, read.raw
    );
}

/// RULE: a value written in no alphabet CloudFront mints an identifier in names no control and
/// could never have been one — so the route reports the identifier it cannot READ rather than
/// the control that is absent, under the same code and status.
#[tokio::test]
async fn a_value_this_service_issues_no_identifier_in_is_the_unreadable_identifier() {
    let router = emulator();
    let live = live(&router).await;

    for op in OPS {
        for (case, id) in outside_the_identifier_alphabet(&live) {
            let answer = op
                .send(&router, &id, &live.control_document, &live.control_etag)
                .await;
            assert_refusal(
                &answer,
                &format!("{op:?} with {case} ({id:?})"),
                UNREADABLE_IDENTIFIER,
            );
        }
    }

    let read = get_control(&router, &live.control_id).await;
    assert_eq!(
        read.status, 200,
        "case the live control: expected it to survive every refusal, got {} with {}",
        read.status, read.raw
    );
}

/// RULE, over a value this account really DID issue for a control: an identifier a delete
/// retired names a control that is no longer there, and so does the live identifier of
/// another kind of the shared form — both are the control reported ABSENT.
/// Driven through all four operations addressed by an `Id`.
#[tokio::test]
async fn a_deleted_controls_own_identifier_is_the_absent_control() {
    let router = emulator();
    let live = live(&router).await;

    // This route's own kind, created and then deleted.
    let control = table()[0].with_name("the control this test deletes", "oac-readability-dead");
    let created = create_control(&router, &control.xml()).await;
    assert_eq!(
        created.status, 201,
        "case creating the control to delete: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let dead_control_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let deleted_control = call(
        &router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{dead_control_id}"),
        None,
        Some(created.header("etag")),
    )
    .await;
    assert_eq!(
        deleted_control.status, 204,
        "case deleting the control: expected HTTP 204, got {} with {}",
        deleted_control.status, deleted_control.raw
    );

    for op in OPS {
        for (case, id, message) in [
            (
                "a deleted origin access control's own identifier",
                dead_control_id.as_str(),
                NO_SUCH_CONTROL,
            ),
            (
                "a live origin access identity's identifier",
                live.origin_access_identity_id.as_str(),
                NO_SUCH_CONTROL,
            ),
        ] {
            let answer = op
                .send(&router, id, &live.control_document, &live.control_etag)
                .await;
            assert_refusal(&answer, &format!("{op:?} with {case} ({id:?})"), message);
        }
    }

    // And the control the account really holds is still readable at its own identifier.
    let read = get_control(&router, &live.control_id).await;
    assert_eq!(
        read.status, 200,
        "case the live control: expected it to survive every refusal, got {} with {}",
        read.status, read.raw
    );
}

#[tokio::test]
async fn an_identifier_that_could_address_a_control_and_names_none_is_the_absent_control() {
    let router = emulator();
    let live = live(&router).await;

    for op in OPS {
        for (case, id) in no_resource_answers_to(&live) {
            let answer = op
                .send(&router, &id, &live.control_document, &live.control_etag)
                .await;
            assert_refusal(&answer, &format!("{op:?} with {case} ({id:?})"), NO_SUCH_CONTROL);
        }
    }

    // And the control the account really holds is still readable at its own identifier.
    let read = get_control(&router, &live.control_id).await;
    assert_eq!(
        read.status, 200,
        "case the live control: expected it to survive every refusal, got {} with {}",
        read.status, read.raw
    );
}
