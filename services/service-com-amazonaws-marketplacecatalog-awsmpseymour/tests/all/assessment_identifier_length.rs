// SPDX-License-Identifier: CC0-1.0

//! `DescribeAssessmentRequest$AssessmentIdentifier` against BOTH ends of its modelled
//! `@length`, end to end over HTTP.
//!
//! One rule is under test: a member this service judges with a predicate of its own
//! stands in for the WHOLE modelled constraint — the `@length(min: 1, max: 2048)` of
//! `AssessmentIdentifier` as well as the syntax the service publishes for it — so an
//! identifier outside either is malformed and refused `422 ValidationException` BEFORE
//! the store is read, whatever the store holds, and only an identifier inside both
//! reaches the lookup, where an unknown one is `404 ResourceNotFoundException` and a held
//! one is served.
//!
//! The rows span what the rule generalises over: which of the two accepted forms the
//! identifier takes (a bare id, the full assessment ARN), and how long it is — nothing at
//! all, the shortest value either form can take, the published maximum, one character
//! past it and far past it — plus a value inside the bound but outside the syntax, so
//! neither half of the predicate can be dropped without a row failing.
//!
//! `modelled_member_lengths.rs` pins the same rule across the OTHER members this service
//! judges itself; this suite is the one that also pins the lower bound and the ordering
//! against a store that already holds an assessment.
//!
//! An identifier this member admits can be as long as a `Message` may be, so the sentence
//! an answer carries is that sentence with the quoted identifier cut to fit — never a
//! `Message` past `ExceptionMessageContent`'s own `@length`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::ops::types::ControlAssessment;
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    service_state, AssessmentRecord,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How long an `AssessmentIdentifier` may be, and how short: that shape's
/// `@length(min: 1, max: 2048)` (`smithy_query(query="shape",
/// shape="AssessmentIdentifier")`). Written out here rather than read from the crate so a
/// wrong implementation cannot make this suite pass by quoting its own bound.
const ASSESSMENT_IDENTIFIER_MAX: usize = 2048;

/// How long any `Message` of this service may be: `ExceptionMessageContent`'s
/// `@length(min: 1, max: 2048)` (`smithy_query(query="shape",
/// shape="ExceptionMessageContent")`). An identifier this member admits can be that long
/// on its own, so a sentence quoting one whole does not always fit.
const EXCEPTION_MESSAGE_MAX: usize = 2048;

/// The syntax the service publishes for an `AssessmentIdentifier`, reported verbatim in
/// the refusal for one outside it: the two forms the member's doc documents ("You can
/// provide either the assessment ID … or the full assessment ARN"), as two separately
/// anchored alternatives joined by `|` with no account field in the ARN branch — the
/// service's own string, not the shape's modelled `@pattern`.
const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"(^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::",
    r"[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$)"
);

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_holding(Vec::new())
}

/// An emulator whose store already holds `records`.
///
/// An assessment is the RESULT of a framework evaluation a change set submits a delivery
/// option to (`assessment_production.rs` drives that path end to end); the rule here is
/// about the identifier a READ is asked for, so the record it may find is seeded
/// directly. Everything asserted after that goes through the router.
fn emulator_holding(records: Vec<AssessmentRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the marketplace catalog store");
        let mut store = state.lock();
        for held in records {
            store.assessments.insert(held.id.clone(), held);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The fault this answer carries: every error of this service spells it `Message`.
    fn message(&self) -> String {
        self.text("/Message")
    }
}

async fn describe_assessment(router: &MultiServiceRouter, identifier: &str) -> Answer {
    let payload = json!({"Catalog": CATALOG, "AssessmentIdentifier": identifier}).to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/DescribeAssessment")
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "DescribeAssessment answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// An assessment id of exactly `length` characters: the `assessment-` prefix and a run of
/// alphanumerics, which is the id form `AssessmentIdentifier` accepts.
fn assessment_id(length: usize) -> String {
    let head = "assessment-";
    format!("{head}{}", "7".repeat(length - head.chars().count()))
}

/// An assessment ARN of exactly `length` characters, naming an assessment of this catalog
/// in this service's own ARN namespace — the other form `AssessmentIdentifier` accepts.
/// Spelled out here rather than taken from the crate's own builder so a wrong
/// implementation cannot make this suite pass.
fn assessment_arn(length: usize) -> String {
    let head = format!("arn:aws:aws-marketplace:eu-west-1::{CATALOG}/Assessment/assessment-");
    format!("{head}{}", "8".repeat(length - head.chars().count()))
}

/// The ARN of `id` as this catalog names it, for the ARN form of a read.
fn arn_of(id: &str) -> String {
    format!("arn:aws:aws-marketplace:eu-west-1::{CATALOG}/Assessment/{id}")
}

/// The assessment an identifier of either accepted form names: the id at the end of it.
fn named_id(identifier: &str) -> String {
    identifier
        .rsplit('/')
        .next()
        .unwrap_or(identifier)
        .to_string()
}

/// The fault a malformed identifier is refused with: the sentence this service states for
/// one outside the syntax it publishes, built from the request.
fn invalid(identifier: &str) -> String {
    format!(
        "[Requested assessment identifier '{identifier}' is invalid. \
         It should match with {ASSESSMENT_IDENTIFIER_PATTERN}.]"
    )
}

/// One row: the case, the identifier sent, and the answer the rule requires — status,
/// wire error code (empty for a success) and the exact fault.
type Row = (String, String, u16, &'static str, String);

/// One row for an identifier the rule accepts as well formed: it reaches the lookup, so
/// the empty store reports the assessment it names as missing.
fn reaches_the_lookup(case: &str, identifier: String) -> Row {
    let message = format!(
        "Requested assessment '{}' does not exist.",
        named_id(&identifier)
    );
    (
        format!("{case} ({} characters)", identifier.chars().count()),
        identifier,
        404,
        "ResourceNotFoundException",
        message,
    )
}

/// One row for an identifier the rule refuses as malformed.
fn is_refused(case: &str, identifier: String) -> Row {
    let message = invalid(&identifier);
    (
        format!("{case} ({} characters)", identifier.chars().count()),
        identifier,
        422,
        "ValidationException",
        message,
    )
}

/// Assert `answer` carries `sentence` — cut to fit when the whole sentence is longer than
/// a `Message` may be.
///
/// An identifier inside this member's own `@length` can be as long as a `Message` may be,
/// so a sentence quoting one whole does not fit: what the answer must carry is the same
/// sentence with the quoted value cut, inside the bound.
fn assert_message(case: &str, answer: &Answer, sentence: &str) {
    let actual = answer.message();
    let length = actual.chars().count();
    assert!(
        (1..=EXCEPTION_MESSAGE_MAX).contains(&length),
        "case {case:?}: the Message must stay inside its modelled \
         {EXCEPTION_MESSAGE_MAX} characters, got {length}"
    );
    if sentence.chars().count() <= EXCEPTION_MESSAGE_MAX {
        assert_eq!(
            actual, sentence,
            "case {case:?}: the answer must carry the sentence built from the request, got \
             {actual:?}"
        );
        return;
    }
    // The cut falls at the quoted value, far past this prefix, so what the answer
    // carries begins with what the whole sentence begins with.
    let shared: String = actual.chars().take(512).collect();
    assert!(
        sentence.starts_with(&shared),
        "case {case:?}: the answer must carry that same sentence, cut to fit, got {actual:?}"
    );
}

/// RULE: the predicate this service judges `AssessmentIdentifier` with stands in for the
/// member's whole modelled constraint, so an identifier shorter than its `@length` minimum
/// or longer than its maximum is refused as malformed (422 `ValidationException`, naming
/// the syntax the service publishes) exactly as one outside that syntax is, while an
/// identifier inside both reaches the lookup and is reported missing (404).
#[tokio::test]
async fn an_assessment_identifier_is_bounded_at_both_ends_of_its_modelled_length() {
    let router = emulator();

    let rows: Vec<Row> = vec![
        // Below the modelled minimum: a member that carries nothing is no identifier at
        // all, so it is malformed rather than a lookup of the empty id.
        is_refused("an empty identifier", String::new()),
        // Inside the bound: the shortest value either form can take, and the longest.
        reaches_the_lookup("the shortest id", "assessment-1".to_string()),
        reaches_the_lookup("the shortest arn", arn_of("assessment-1")),
        reaches_the_lookup("an id at the bound", assessment_id(ASSESSMENT_IDENTIFIER_MAX)),
        reaches_the_lookup("an arn at the bound", assessment_arn(ASSESSMENT_IDENTIFIER_MAX)),
        // Past the bound, in either form: one character over, and far over.
        is_refused(
            "an id one past the bound",
            assessment_id(ASSESSMENT_IDENTIFIER_MAX + 1),
        ),
        is_refused(
            "an arn one past the bound",
            assessment_arn(ASSESSMENT_IDENTIFIER_MAX + 1),
        ),
        is_refused("an id far past the bound", assessment_id(5011)),
        is_refused("an arn far past the bound", assessment_arn(8192)),
        // Inside the bound but outside the syntax: the length is not the only half of
        // the predicate.
        is_refused(
            "an id inside the bound with a character the syntax excludes",
            format!("{}_x", assessment_id(64)),
        ),
    ];

    for (case, identifier, status, code, message) in rows {
        let answer = describe_assessment(&router, &identifier).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (status, code),
            "case {case:?}: an identifier outside the member's modelled length is refused \
             as malformed and one inside it is judged by the rules behind the member, got \
             {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_message(&case, &answer, &message);
    }
}

/// RULE, against a store that is not empty: the identifier is judged BEFORE the lookup,
/// so what the store holds cannot turn a malformed identifier into a 404 (or a hit), and
/// a refused read leaves the held assessment exactly as it was.
#[tokio::test]
async fn an_over_long_identifier_is_refused_whatever_the_store_holds() {
    let held_id = "assessment-00000000002a";
    let held = AssessmentRecord {
        id: held_id.to_string(),
        arn: arn_of(held_id),
        framework_id: "AMISecurity@1".to_string(),
        framework: "AMISecurity".to_string(),
        delivery_option_id: Some("do-1".to_string()),
        entity_id: Some("ent-1".to_string()),
        change_set_id: None,
        result: "PASS".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        expires_at: "2026-02-01T00:00:00Z".to_string(),
        controls: vec![ControlAssessment {
            control_id: Some("control-1".to_string()),
            control_assessment_result: Some("PASS".to_string()),
            verification_source: Some("AWS_VERIFIED".to_string()),
            errors: None,
            control_summary: None,
        }],
        // A finished assessment: nothing of it is still running.
        runs_until: None,
        tags: Vec::new(),
    };
    let router = emulator_holding(vec![held]);

    // The held assessment is served through either accepted form of its identifier.
    for (case, identifier) in [
        ("the id form", held_id.to_string()),
        ("the arn form", arn_of(held_id)),
    ] {
        let answer = describe_assessment(&router, &identifier).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (200, ""),
            "case {case:?}: the held assessment must be served, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        let reported = answer.text("/AssessmentId");
        assert_eq!(
            reported, held_id,
            "case {case:?}: the report must name the assessment the store holds, got \
             {reported:?}"
        );
    }

    // The same id padded past the member's modelled length is a different, malformed
    // identifier: it is refused for its syntax, never answered as the held record and
    // never as a missing one.
    let padded = format!(
        "{held_id}{}",
        "0".repeat(ASSESSMENT_IDENTIFIER_MAX + 1 - held_id.chars().count())
    );
    for (case, identifier) in [
        ("an id past the bound", padded.clone()),
        ("an arn past the bound", assessment_arn(4096)),
    ] {
        let answer = describe_assessment(&router, &identifier).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: an identifier past the modelled length is refused before the \
             store is read, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_message(case, &answer, &invalid(&identifier));
    }

    // Padded only up TO the bound, it is well formed — so it reaches the lookup and is
    // reported missing, not served as the held record whose id it begins with.
    let at_bound = format!(
        "{held_id}{}",
        "0".repeat(ASSESSMENT_IDENTIFIER_MAX - held_id.chars().count())
    );
    let answer = describe_assessment(&router, &at_bound).await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (404, "ResourceNotFoundException"),
        "case {:?}: an identifier at the bound names no held assessment, got {} {:?} with \
         {:?}",
        at_bound.chars().count(),
        answer.status,
        answer.error_code(),
        answer.body
    );

    // The refusals changed nothing: the held assessment reads back as it did.
    let answer = describe_assessment(&router, held_id).await;
    assert_eq!(
        (answer.status, answer.text("/AssessmentId")),
        (200, held_id.to_string()),
        "the held assessment must still be served after the refused reads, got {} with \
         {:?}",
        answer.status,
        answer.body
    );
}
