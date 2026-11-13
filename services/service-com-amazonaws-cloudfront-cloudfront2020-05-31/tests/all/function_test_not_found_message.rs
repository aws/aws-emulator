// SPDX-License-Identifier: CC0-1.0

//! The refusal `TestFunction` answers for a function stage this service does not hold, end
//! to end over HTTP.
//!
//! Every assertion goes through the router, so each row exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a `TestFunction` request that addresses a name no
//! function of the account holds, or a stage the function it names has not been published
//! to, is refused with HTTP 404 and the modelled `NoSuchFunctionExists` carrying the fixed
//! per-code sentence for this resource kind — the whole message is service text, so neither
//! the name, nor the stage, nor the version token the request carried is any part of it —
//! and that refusal is decided BEFORE the `If-Match` precondition, so a request whose
//! precondition is also unsatisfiable is still refused for the stage that is not there.
//! A request that addresses a stage the service DOES hold, under that stage's current
//! version, is served instead: HTTP 200 with a `TestResult`.
//!
//! The rows differ along everything that rule generalises over:
//!
//! * the NAME: one character, the longest `FunctionName` admits (64), a name drawing on
//!   every character class its pattern allows, a name nothing was ever created under, and
//!   the name a function really held until it was deleted;
//! * the STAGE the request names: none at all, `DEVELOPMENT`, and `LIVE`;
//! * WHY the stage is not there: the function does not exist at all, or the function exists
//!   and nothing has published it to the stage named;
//! * the version token carried: the current token of a stage the function really has, the
//!   token a deleted function stood at, a well-formed token nothing holds, and a value that
//!   is no version token at all;
//! * the event object: a viewer-request one and a viewer-response one.
//!
//! One function in the store IS published, and the accepted rows at the end test stages that
//! are there, so the table cannot pass by refusing everything.
//!
//! Its own suite, beside `connection_function_not_found_message.rs` which states the same rule
//! for the neighbouring resource kind: the subject here is the WORDING of the refusal and the
//! ORDER the request is resolved in, which is a property of every addressing form at once,
//! while `function_test_run.rs` is about what a RUN produced.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer};
use crate::function_delete::delete_function;
use crate::function_lifecycle::{describe_function, Create};
use crate::function_publish::{create_one, publish_function};
use crate::function_test_run::{test_function, VIEWER_REQUEST_EVENT, VIEWER_RESPONSE_EVENT};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The wire error code the model names for this condition on `TestFunction`
/// (`NoSuchFunctionExists`, HTTP 404).
const NO_SUCH_FUNCTION_EXISTS: &str = "NoSuchFunctionExists";

/// The whole sentence CloudFront refuses a function it does not hold with, spelled out here
/// so a wrong constant in the crate cannot make these assertions pass.
const NOT_FOUND_SENTENCE: &str = "The specified function does not exist.";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no function in a fresh emulator holds.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

/// A value that is no version token at all, so a precondition carrying it could not be
/// satisfied by any stage: a row carrying it shows the stage is resolved first.
const MALFORMED_VERSION_TOKEN: &str = "not-a-version-token";

/// A function body small enough that each row is about which stage is addressed rather than
/// about the code.
const CODE: &str = "function handler(event) { return event.request; }\n";

/// One `CreateFunction` request for the function `name`.
fn create_named(case: &'static str, name: &str) -> Create {
    Create {
        case,
        name: name.to_string(),
        comment: "tested at a stage".to_string(),
        runtime: "cloudfront-js-2.0",
        code: CODE.as_bytes().to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    }
}

/// The version token the `stage` of `name` stands at, read back through the describe of that
/// stage — the version a test of it is checked against.
async fn stage_version(router: &MultiServiceRouter, name: &str, stage: Option<&str>) -> String {
    let answer = describe_function(router, name, stage).await;
    assert_eq!(
        answer.status, 200,
        "the describe of {name} at {stage:?} must answer HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    answer.header("etag").to_string()
}

/// Assert `answer` is the fixed `NoSuchFunctionExists` refusal of a function this service does
/// not hold, echoing nothing the request carried: `name` is the name it addressed, `stage` the
/// stage it named, and `token` the version token it carried, or `""` for a read that carries
/// none.
///
/// One helper for the whole resource kind, so the describe the no-mutation checks go through is
/// held to the same sentence as the test itself.
fn assert_not_found(answer: &Answer, case: &str, name: &str, stage: Option<&str>, token: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        NO_SUCH_FUNCTION_EXISTS,
        "case {case}: expected {NO_SUCH_FUNCTION_EXISTS}, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer
        .root()
        .find_text("Message")
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, NOT_FOUND_SENTENCE,
        "case {case}: expected the fixed sentence, got {message:?}"
    );
    // The equality above already fixes the WHOLE sentence; these say what that means, on the
    // values an echo could be spotted in at all. A one- or two-character value occurs inside
    // ordinary English words, so searching the sentence for it would assert nothing.
    let distinctive = |value: &str| value.len() > 2;
    assert!(
        !distinctive(name) || !message.contains(name),
        "case {case}: the refusal must not echo the name {name:?}, got {message:?}"
    );
    assert!(
        stage.is_none_or(|stage| !message.contains(stage)),
        "case {case}: the refusal must not echo the stage {stage:?}, got {message:?}"
    );
    assert!(
        !distinctive(token) || !message.contains(token),
        "case {case}: the refusal must not echo the version token {token:?}, got {message:?}"
    );
}

/// Assert `answer` is a `TestFunction` request that was refused for the stage it addressed:
/// the fixed refusal, and no result beside it, because a refusal is not a run.
fn assert_test_refused(
    answer: &Answer,
    case: &str,
    name: &str,
    stage: Option<&str>,
    token: &str,
) {
    assert_not_found(answer, case, name, stage, token);
    assert!(
        !answer.raw.contains("TestResult"),
        "case {case}: a refused test reports no result, got {}",
        answer.raw
    );
}

/// Every `TestFunction` request addressing a stage this service does not hold — whatever the
/// name's shape, whichever stage it named, whatever version token it carried and whichever
/// event object it submitted — is refused with the SAME fixed sentence, and the stages that
/// ARE there are still tested.
#[tokio::test]
async fn a_test_of_a_stage_this_service_does_not_hold_carries_the_fixed_sentence() {
    let router = emulator();

    // Three functions that stay, spanning the name forms the model admits. None of them is
    // published, so each one's LIVE stage is a stage the service does not hold while the
    // function itself is right there.
    let short = "a";
    let short_etag = create_one(&router, &create_named("the shortest admissible name", short)).await;
    let longest = "n".repeat(64);
    let longest_etag = create_one(
        &router,
        &create_named("the longest admissible name (64)", &longest),
    )
    .await;
    let mixed = "Fn-9_tested";
    let mixed_etag = create_one(
        &router,
        &create_named("every character class the name pattern admits", mixed),
    )
    .await;

    // One function that really existed and was then deleted, so its name is one the store
    // once answered for and no longer does, and the token it stood at is a real one.
    let dead = "deleted_function";
    let dead_etag = create_one(&router, &create_named("the function that is deleted", dead)).await;
    let deleted = delete_function(&router, dead, &dead_etag).await;
    assert_eq!(
        deleted.status, 204,
        "the delete that makes the name dead must answer HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );

    // One function that IS published, so the rows below are refused for the stage they name
    // rather than because this store has nothing in the published stage at all.
    let published = "published_function";
    let published_etag =
        create_one(&router, &create_named("the function that is published", published)).await;
    let publish = publish_function(&router, published, &published_etag).await;
    assert_eq!(
        publish.status, 200,
        "the publish that fills the LIVE stage must answer HTTP 200, got {} with {}",
        publish.status, publish.raw
    );

    // (case, name, stage, If-Match token, event object)
    let rows: Vec<(&str, String, Option<&str>, &str, &str)> = vec![
        (
            "a name no function holds, naming no stage",
            "absent_function".to_string(),
            None,
            short_etag.as_str(),
            VIEWER_REQUEST_EVENT,
        ),
        (
            "a name no function holds, the stage a function is created in",
            "absent_function".to_string(),
            Some(STAGE_DEVELOPMENT),
            UNHELD_VERSION_TOKEN,
            VIEWER_RESPONSE_EVENT,
        ),
        (
            "a name no function holds, the published stage",
            "absent_function".to_string(),
            Some(STAGE_LIVE),
            UNHELD_VERSION_TOKEN,
            VIEWER_REQUEST_EVENT,
        ),
        (
            "a one-character name no function holds",
            "q".to_string(),
            None,
            MALFORMED_VERSION_TOKEN,
            VIEWER_REQUEST_EVENT,
        ),
        (
            "the longest name the model admits, never created",
            "z".repeat(64),
            Some(STAGE_LIVE),
            UNHELD_VERSION_TOKEN,
            VIEWER_RESPONSE_EVENT,
        ),
        (
            "the name a function held until it was deleted, naming no stage",
            dead.to_string(),
            None,
            dead_etag.as_str(),
            VIEWER_REQUEST_EVENT,
        ),
        (
            "the name a function held until it was deleted, the published stage",
            dead.to_string(),
            Some(STAGE_LIVE),
            dead_etag.as_str(),
            VIEWER_RESPONSE_EVENT,
        ),
        (
            "the published stage of a function nothing published, under that function's \
             current version",
            short.to_string(),
            Some(STAGE_LIVE),
            short_etag.as_str(),
            VIEWER_REQUEST_EVENT,
        ),
        (
            "the published stage of a function nothing published, under a token nothing holds",
            longest.clone(),
            Some(STAGE_LIVE),
            UNHELD_VERSION_TOKEN,
            VIEWER_RESPONSE_EVENT,
        ),
        (
            "the published stage of a function nothing published, under a value that is no \
             version token at all",
            mixed.to_string(),
            Some(STAGE_LIVE),
            MALFORMED_VERSION_TOKEN,
            VIEWER_REQUEST_EVENT,
        ),
    ];

    for (case, name, stage, token, event) in &rows {
        let answer = test_function(&router, name, *stage, event.as_bytes(), token).await;
        assert_test_refused(&answer, case, name, *stage, token);
    }

    // A refused test wrote nothing: each function that stays is still there, at the version
    // it stood at before the rows ran, and its LIVE stage is still absent.
    for (case, name, etag) in [
        ("the shortest admissible name", short.to_string(), &short_etag),
        ("the longest admissible name (64)", longest.clone(), &longest_etag),
        (
            "every character class the name pattern admits",
            mixed.to_string(),
            &mixed_etag,
        ),
    ] {
        assert_eq!(
            &stage_version(&router, &name, None).await,
            etag,
            "case {case}: a refused test must leave the stored version as it stands"
        );
        let live = describe_function(&router, &name, Some(STAGE_LIVE)).await;
        assert_not_found(&live, case, &name, Some(STAGE_LIVE), "");
    }

    // The accepted rows: a stage this service DOES hold, under that stage's current version,
    // is tested rather than refused — whether the request names the stage a function is
    // created in, names none at all, or names the stage a publish filled.
    let live_version = stage_version(&router, published, Some(STAGE_LIVE)).await;
    for (case, name, stage, token, stage_tested) in [
        (
            "the stage a function is created in, named",
            short.to_string(),
            Some(STAGE_DEVELOPMENT),
            short_etag.clone(),
            STAGE_DEVELOPMENT,
        ),
        (
            "the stage a request that names none addresses",
            longest.clone(),
            None,
            longest_etag.clone(),
            STAGE_DEVELOPMENT,
        ),
        (
            "the stage a publish filled",
            published.to_string(),
            Some(STAGE_LIVE),
            live_version.clone(),
            STAGE_LIVE,
        ),
    ] {
        let answer = test_function(&router, &name, stage, VIEWER_REQUEST_EVENT.as_bytes(), &token)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: a stage this service holds must be tested, got {} with {}",
            answer.status, answer.raw
        );
        let result = answer.root();
        assert_eq!(
            result.name(),
            "TestResult",
            "case {case}: expected the TestResult payload, got {}",
            answer.raw
        );
        assert_eq!(
            result.text_at("FunctionSummary/Name"),
            Some(name.as_str()),
            "case {case}: expected the tested function's name in the summary, got {}",
            answer.raw
        );
        assert_eq!(
            result.text_at("FunctionSummary/FunctionMetadata/Stage"),
            Some(stage_tested),
            "case {case}: expected the summary of the stage that ran, got {}",
            answer.raw
        );
    }
}
