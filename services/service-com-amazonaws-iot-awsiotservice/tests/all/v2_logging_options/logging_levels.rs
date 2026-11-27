// SPDX-License-Identifier: CC0-1.0

//! The account's per-target V2 LOGGING LEVELS, end to end over HTTP: `SetV2LoggingLevel`,
//! the `ListV2LoggingLevels` reader that proves what it persisted, and
//! `DeleteV2LoggingLevel`.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `POST /v2LoggingLevel` and
//! `GET /v2LoggingLevel` bindings, the handlers and the response serialization are all
//! exercised.
//!
//! The rules under test, none of them specific to one log target:
//!
//! * a log target is the PAIR `targetType` + `targetName`, and `SetV2LoggingLevel`
//!   records the level it names for THAT pair, which `ListV2LoggingLevels` reports back
//!   as a `LogTargetConfiguration`;
//! * one target holds one level: a second set for the same pair replaces it, while the
//!   same name under another target type is another target;
//! * a listing that names a target type reports the targets OF that type, one page at a
//!   time in a stable order, and a page resumes where the last one ended;
//! * a delete takes ONE target's level away and leaves every other alone, and a target
//!   that holds no level has nothing to take away: the delete is the same empty 200;
//! * the account-wide `DEFAULT` target names no resource, so its identity is that type
//!   alone and the writer, the reader and the remover resolve the one record it has;
//! * the levels OVERRIDE the account's V2 logging options, so an account that has never
//!   set them is refused with the modelled `NotConfiguredException` (404) by both the
//!   set and the listing — the two operations that model that error — and nothing is
//!   recorded for it;
//! * every target type but the account-wide `DEFAULT` one names a resource, so a set
//!   that names none is refused (400) and records nothing;
//! * the number of targets one account holds a level for is bounded by the emulator's
//!   own ceiling, past which a set for a NEW target is refused with the modelled
//!   `LimitExceededException` (410) while a set for a target already held still
//!   replaces its level;
//! * reading is a read: repeating a listing with no set between reports the same page
//!   and leaves the store alone.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::{
    LOG_LEVEL_VALUES, LOG_TARGET_TYPE_VALUES,
};
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, LoggedTarget, MAX_V2_LOGGING_LEVELS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. The levels are account-wide,
/// so no assertion depends on it.
const REGION: &str = "eu-west-3";

/// A role ARN the account's V2 logging options may name. Nothing asserts on the text:
/// the member is an opaque `AwsArn` in the model, and it is here only so the account
/// HAS logging options to override.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/iot-logs";

/// AWS's own sentence for an account whose V2 logging options were never set, written
/// out here so the wording is pinned by the test and not only by the constant under
/// test.
const EXPECTED_NOT_CONFIGURED_MESSAGE: &str =
    "SetV2LoggingOptions was not previously called. No logging levels have been set.";

/// The `LogTargetType` value that names the account's own default logging rather than
/// one of its resources, and so carries no target name
/// (`smithy_query(query="shape",shape="LogTargetType")`).
const DEFAULT_TARGET_TYPE: &str = "DEFAULT";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: std::sync::Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(std::sync::Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store, as the handlers reach it.
    fn state(&self) -> engine::ServiceState<ComAmazonawsIotAwsiotserviceState> {
        self.engine.state::<ComAmazonawsIotAwsiotserviceState>()
    }

    /// The levels the store holds, as the handlers see them.
    fn stored(&self) -> BTreeMap<LoggedTarget, String> {
        self.state().lock().v2_logging_levels.clone()
    }

    /// Put a level straight into the store, so a listing can be tested against records
    /// no request in the test wrote and a ceiling can be reached without one call per
    /// record.
    fn seed(&self, target_type: &str, target_name: Option<&str>, log_level: &str) {
        self.state().lock().v2_logging_levels.insert(
            LoggedTarget {
                target_type: target_type.to_string(),
                target_name: target_name.map(str::to_string),
            },
            log_level.to_string(),
        );
    }

    async fn dispatch(&self, method: &str, path: &str, query: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}{query}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the v2-logging-level request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    /// Give the account V2 logging options, which is what a per-target level overrides.
    async fn configure_logging(&self, case: &str) {
        let reply = self
            .dispatch(
                "POST",
                "/v2LoggingOptions",
                "",
                Some(&json!({ "roleArn": ROLE_ARN, "defaultLogLevel": "INFO" })),
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected SetV2LoggingOptions to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    async fn set_v2_logging_level(&self, log_target: &Value, log_level: &str) -> Reply {
        let body = json!({ "logTarget": log_target, "logLevel": log_level });
        self.dispatch("POST", "/v2LoggingLevel", "", Some(&body))
            .await
    }

    /// Set a level the service must accept, failing the case if it does not.
    async fn set_ok(&self, case: &str, log_target: &Value, log_level: &str) {
        let reply = self.set_v2_logging_level(log_target, log_level).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected SetV2LoggingLevel to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// Remove the level held for one target. Both members are `@required` and bound to
    /// the query string the model declares for them.
    async fn delete_v2_logging_level(&self, target_type: &str, target_name: &str) -> Reply {
        let query = format!("?targetType={target_type}&targetName={target_name}");
        self.dispatch("DELETE", "/v2LoggingLevel", &query, None)
            .await
    }

    /// Remove a level the service must accept removing, failing the case if it does not.
    async fn delete_ok(&self, case: &str, target_type: &str, target_name: &str) {
        let reply = self.delete_v2_logging_level(target_type, target_name).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected DeleteV2LoggingLevel to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    async fn list_v2_logging_levels(
        &self,
        target_type: Option<&str>,
        max_results: Option<u32>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut parameters: Vec<String> = Vec::new();
        if let Some(target_type) = target_type {
            parameters.push(format!("targetType={target_type}"));
        }
        if let Some(max_results) = max_results {
            parameters.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            parameters.push(format!("nextToken={next_token}"));
        }
        let query = if parameters.is_empty() {
            String::new()
        } else {
            format!("?{}", parameters.join("&"))
        };
        self.dispatch("GET", "/v2LoggingLevel", &query, None).await
    }

    /// The configurations a successful listing reports.
    async fn reported(
        &self,
        case: &str,
        target_type: Option<&str>,
        max_results: Option<u32>,
        next_token: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let reply = self
            .list_v2_logging_levels(target_type, max_results, next_token)
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected ListV2LoggingLevels to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let body = reply.json();
        let configurations = body["logTargetConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a list of configurations, got {body}")
            });
        let next = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        (configurations, next)
    }

    /// Every configuration the account holds, walked page by page with `page_size` rows
    /// at a time, so what is asserted is the whole listing and not only its first page.
    async fn every_configuration(
        &self,
        case: &str,
        target_type: Option<&str>,
        page_size: u32,
    ) -> Vec<Value> {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        // One page per record the store can hold at worst, plus the page that ends the
        // walk: a listing that never stops is a defect, not a reason to loop for ever.
        for _ in 0..=MAX_V2_LOGGING_LEVELS + 1 {
            let (page, next) = self
                .reported(case, target_type, Some(page_size), token.as_deref())
                .await;
            assert!(
                page.len() <= page_size as usize,
                "case {case}: expected at most {page_size} rows a page, got {}",
                page.len()
            );
            walked.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => return walked,
            }
        }
        panic!("case {case}: the listing never ran out of pages");
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// One `LogTarget`, in the JSON form the model binds it to. A name left `None` is left
/// out of the object, which is how a caller omits it.
fn log_target(target_type: &str, target_name: Option<&str>) -> Value {
    let mut target = Map::new();
    target.insert(
        "targetType".to_string(),
        Value::String(target_type.to_string()),
    );
    if let Some(target_name) = target_name {
        target.insert(
            "targetName".to_string(),
            Value::String(target_name.to_string()),
        );
    }
    Value::Object(target)
}

/// The configuration a listing must report for one target at one level.
fn configuration(target_type: &str, target_name: Option<&str>, log_level: &str) -> Value {
    json!({ "logTarget": log_target(target_type, target_name), "logLevel": log_level })
}

/// A target name for `target_type` that is of the FORM that type addresses, so no row
/// of a table depends on one recorded name.
fn target_name_for(target_type: &str) -> Option<&'static str> {
    match target_type {
        // The account-wide default target names no resource.
        DEFAULT_TARGET_TYPE => None,
        "THING_GROUP" => Some("factory-floor"),
        "CLIENT_ID" => Some("sensor-7"),
        "SOURCE_IP" => Some("198.51.100.14/32"),
        "PRINCIPAL_ID" => Some("AIDACKCEVSQ6C2EXAMPLE"),
        _ => Some("defender-targets"),
    }
}

/// Every level a caller writes for every target type the model declares is reported
/// back for THAT target, with the pair the request named.
#[tokio::test]
async fn a_level_set_for_a_target_is_reported_for_that_target() {
    // The rows span what the rule generalises over: every `LogTargetType` value the
    // model declares (`smithy_query(query="shape",shape="LogTargetType")`, re-exported
    // as `LOG_TARGET_TYPE_VALUES`) against every `LogLevel` value it declares
    // (`LOG_LEVEL_VALUES`) — including DISABLED, which the issue this suite was written
    // for cited neither a level nor a target type for.
    for target_type in LOG_TARGET_TYPE_VALUES {
        for log_level in LOG_LEVEL_VALUES {
            let case = format!("{target_type} at {log_level}");
            let target_name = target_name_for(target_type);
            // A fresh emulator per row, so each row is read against its own write.
            let emulator = Emulator::new();
            emulator.configure_logging(&case).await;
            emulator
                .set_ok(&case, &log_target(target_type, target_name), log_level)
                .await;

            let expected = vec![configuration(target_type, target_name, log_level)];
            for (listing, filter) in [("unfiltered", None), ("filtered", Some(*target_type))] {
                let (reported, next) = emulator.reported(&case, filter, None, None).await;
                assert_eq!(
                    reported, expected,
                    "case {case} ({listing}): expected {expected:?}, got {reported:?}"
                );
                assert_eq!(
                    next, None,
                    "case {case} ({listing}): expected no continuation token, got {next:?}"
                );
            }

            // A listing of any OTHER type reports nothing: the level belongs to the pair
            // and not to the name alone.
            for other in LOG_TARGET_TYPE_VALUES
                .iter()
                .filter(|other| *other != target_type)
            {
                let (reported, _) = emulator.reported(&case, Some(other), None, None).await;
                assert!(
                    reported.is_empty(),
                    "case {case} (listing {other}): expected no configuration, got {reported:?}"
                );
            }
        }
    }
}

/// One target holds ONE level: a second set for the same pair replaces it, and the same
/// name under another target type is a target of its own.
#[tokio::test]
async fn a_second_set_for_one_target_replaces_its_level() {
    let emulator = Emulator::new();
    emulator.configure_logging("replacement").await;

    // The same name under two target types, so a store keyed by the name alone would
    // collapse the two rows and fail this.
    let shared_name = Some("shared-identifier");
    for (case, target_type, levels, expected) in [
        (
            "a named target set twice",
            "THING_GROUP",
            vec!["DEBUG", "WARN"],
            "WARN",
        ),
        (
            "another type under the same name, set three times",
            "CLIENT_ID",
            vec!["ERROR", "DISABLED", "INFO"],
            "INFO",
        ),
    ] {
        for log_level in &levels {
            emulator
                .set_ok(case, &log_target(target_type, shared_name), log_level)
                .await;
        }
        let (reported, _) = emulator.reported(case, Some(target_type), None, None).await;
        let expected = vec![configuration(target_type, shared_name, expected)];
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
    }

    // Both targets are still held: replacing one level did not replace the other's.
    let (reported, _) = emulator.reported("both targets", None, None, None).await;
    assert_eq!(
        reported.len(),
        2,
        "case both targets: expected one configuration per target type, got {reported:?}"
    );
}

/// A listing walks its targets one page at a time, in a stable order, and a page
/// resumes where the last one ended — whatever page size the caller asks for.
#[tokio::test]
async fn a_listing_pages_through_every_target_it_holds() {
    let emulator = Emulator::new();
    emulator.configure_logging("paging").await;

    // Rows that differ along what the paging generalises over: the number of targets of
    // one type, a type with a single target, and the account-wide target with no name.
    let seeded: Vec<(&str, Option<&str>, &str)> = vec![
        (DEFAULT_TARGET_TYPE, None, "WARN"),
        ("THING_GROUP", Some("alpha"), "DEBUG"),
        ("THING_GROUP", Some("beta"), "INFO"),
        ("THING_GROUP", Some("gamma"), "ERROR"),
        ("THING_GROUP", Some("delta"), "DISABLED"),
        ("CLIENT_ID", Some("sensor-7"), "INFO"),
    ];
    for (target_type, target_name, log_level) in &seeded {
        emulator
            .set_ok(
                "paging",
                &log_target(target_type, *target_name),
                log_level,
            )
            .await;
    }

    for (case, filter, expected_count) in [
        ("every target", None, seeded.len()),
        ("one type", Some("THING_GROUP"), 4),
        ("a type with one target", Some("CLIENT_ID"), 1),
        ("a type with none", Some("SOURCE_IP"), 0),
    ] {
        // The page sizes span one row a page, a size that does not divide the count, and
        // one larger than the whole listing.
        let whole = emulator.every_configuration(case, filter, 250).await;
        assert_eq!(
            whole.len(),
            expected_count,
            "case {case}: expected {expected_count} configurations, got {whole:?}"
        );
        for page_size in [1, 2, 3, 250] {
            let walked = emulator.every_configuration(case, filter, page_size).await;
            assert_eq!(
                walked, whole,
                "case {case} (pages of {page_size}): expected the same listing in the same \
                 order, got {walked:?}"
            );
        }
    }
}

/// A continuation token belongs to the listing that issued it: anything else is refused
/// with the modelled `InvalidRequestException` (400) rather than resuming a listing from
/// the middle.
#[tokio::test]
async fn a_token_of_another_listing_is_refused() {
    let emulator = Emulator::new();
    emulator.configure_logging("tokens").await;
    for name in ["alpha", "beta"] {
        emulator
            .set_ok("tokens", &log_target("THING_GROUP", Some(name)), "INFO")
            .await;
    }

    let (_, token) = emulator.reported("tokens", None, Some(1), None).await;
    let token = token.expect("a truncated listing issues a continuation token");

    for (case, filter, token) in [
        (
            "the token of the unfiltered listing",
            Some("THING_GROUP"),
            token.clone(),
        ),
        (
            "a token nothing issued",
            None,
            "not-a-continuation-token".to_string(),
        ),
        (
            // Hex, so it decodes, but it names no listing of this service.
            "a token that decodes to nothing this listing issued",
            None,
            "6e6f742d612d6c697374696e67".to_string(),
        ),
    ] {
        let reply = emulator
            .list_v2_logging_levels(filter, Some(1), Some(&token))
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {case}: expected ListV2LoggingLevels to refuse with 400, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    // The token the listing DID issue resumes it.
    let (resumed, _) = emulator.reported("tokens", None, Some(1), Some(&token)).await;
    assert_eq!(
        resumed,
        vec![configuration("THING_GROUP", Some("beta"), "INFO")],
        "case the listing's own token: expected the row after the first, got {resumed:?}"
    );
}

/// An account that has never set V2 logging options has nothing for a per-target level
/// to override, so both the set and the listing are refused with the modelled
/// `NotConfiguredException` (404) and nothing is recorded.
#[tokio::test]
async fn an_account_with_no_logging_options_is_refused_as_not_configured() {
    let emulator = Emulator::new();

    for round in 0..3 {
        let case = format!("round {round}");
        let set = emulator
            .set_v2_logging_level(&log_target("THING_GROUP", Some("factory-floor")), "DEBUG")
            .await;
        let list = emulator.list_v2_logging_levels(None, None, None).await;
        for (call, reply) in [("SetV2LoggingLevel", set), ("ListV2LoggingLevels", list)] {
            assert_eq!(
                reply.status,
                404,
                "case {case} ({call}): expected 404, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.error_code(),
                "NotConfiguredException",
                "case {case} ({call}): expected NotConfiguredException, got {}",
                reply.text()
            );
            assert_eq!(
                reply.json()["message"].as_str(),
                Some(EXPECTED_NOT_CONFIGURED_MESSAGE),
                "case {case} ({call}): expected the refusal to name the call that configures \
                 logging, got {}",
                reply.text()
            );
        }
        assert!(
            emulator.stored().is_empty(),
            "case {case}: expected the refusals to record nothing, got {:?}",
            emulator.stored()
        );
    }

    // Once the account HAS options, the same set is accepted and reported.
    emulator.configure_logging("configured").await;
    emulator
        .set_ok(
            "configured",
            &log_target("THING_GROUP", Some("factory-floor")),
            "DEBUG",
        )
        .await;
    let (reported, _) = emulator.reported("configured", None, None, None).await;
    assert_eq!(
        reported,
        vec![configuration("THING_GROUP", Some("factory-floor"), "DEBUG")],
        "case configured: expected the level just set, got {reported:?}"
    );
}

/// Every target type but the account-wide `DEFAULT` one names the resource the level
/// applies to, so a set that names none identifies no target: it is refused (400) and
/// records nothing.
#[tokio::test]
async fn a_named_target_type_without_a_name_is_refused() {
    let emulator = Emulator::new();
    emulator.configure_logging("missing name").await;

    for target_type in LOG_TARGET_TYPE_VALUES
        .iter()
        .filter(|target_type| **target_type != DEFAULT_TARGET_TYPE)
    {
        for (form, target_name) in [("absent", None), ("empty", Some(""))] {
            let case = format!("{target_type} with an {form} name");
            let reply = emulator
                .set_v2_logging_level(&log_target(target_type, target_name), "INFO")
                .await;
            assert_eq!(
                reply.status,
                400,
                "case {case}: expected SetV2LoggingLevel to refuse with 400, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.error_code(),
                "InvalidRequestException",
                "case {case}: expected InvalidRequestException, got {}",
                reply.text()
            );
            assert!(
                emulator.stored().is_empty(),
                "case {case}: expected the refusal to record nothing, got {:?}",
                emulator.stored()
            );
        }
    }

    // The account-wide target is the one that takes no name, and it is accepted.
    emulator
        .set_ok(
            "the default target",
            &log_target(DEFAULT_TARGET_TYPE, None),
            "WARN",
        )
        .await;
    let (reported, _) = emulator
        .reported("the default target", None, None, None)
        .await;
    assert_eq!(
        reported,
        vec![configuration(DEFAULT_TARGET_TYPE, None, "WARN")],
        "case the default target: expected the account-wide level, got {reported:?}"
    );
}

/// A level outside the `LogLevel` catalog the model declares is refused and changes
/// nothing. The refusal is the boundary validator's — a value outside a modelled enum
/// never reaches the handler — which is why the status, and not the error code, is what
/// this asserts.
#[tokio::test]
async fn a_level_outside_the_modelled_catalog_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    emulator.configure_logging("levels").await;
    let target = log_target("THING_GROUP", Some("factory-floor"));

    // Every value the model declares is accepted; the rows beside them are outside the
    // catalog in the ways a caller gets it wrong: wrong case, a level of another
    // service's vocabulary, and the empty string.
    for log_level in LOG_LEVEL_VALUES {
        emulator.set_ok(log_level, &target, log_level).await;
    }
    let before = emulator.stored();

    for log_level in ["debug", "VERBOSE", "TRACE", "", "DISABLED "] {
        let case = format!("the level {log_level:?}");
        let reply = emulator.set_v2_logging_level(&target, log_level).await;
        assert_eq!(
            reply.status,
            400,
            "case {case}: expected SetV2LoggingLevel to refuse with 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            emulator.stored(),
            before,
            "case {case}: expected the refusal to leave the stored levels alone, got {:?}",
            emulator.stored()
        );
    }
}

/// The listing reports what STATE holds, not a value compiled into it: records seeded
/// straight into the store are what the next listing answers with.
#[tokio::test]
async fn the_listing_reports_what_the_store_holds() {
    let emulator = Emulator::new();
    emulator.configure_logging("seeded").await;

    // Two seeded records, so no single hardcoded response can satisfy both rows.
    emulator.seed("PRINCIPAL_ID", Some("AIDACKCEVSQ6C2EXAMPLE"), "ERROR");
    emulator.seed("SOURCE_IP", Some("203.0.113.9/32"), "DEBUG");

    let (reported, _) = emulator.reported("seeded", None, None, None).await;
    assert_eq!(
        reported,
        vec![
            configuration("PRINCIPAL_ID", Some("AIDACKCEVSQ6C2EXAMPLE"), "ERROR"),
            configuration("SOURCE_IP", Some("203.0.113.9/32"), "DEBUG"),
        ],
        "case seeded: expected the seeded records, got {reported:?}"
    );

    // Reading is a read: three listings with no set between them report the same page
    // and leave the store alone.
    let before = emulator.stored();
    for round in 0..3 {
        let case = format!("round {round}");
        let (again, _) = emulator.reported(&case, None, None, None).await;
        assert_eq!(
            again, reported,
            "case {case}: expected the same page, got {again:?}"
        );
        assert_eq!(
            emulator.stored(),
            before,
            "case {case}: expected the listing to leave the store alone, got {:?}",
            emulator.stored()
        );
    }
}

/// The store is one a caller grows, so it is bounded: at the ceiling a set for a NEW
/// target is refused with the modelled `LimitExceededException` (410) and records
/// nothing, while a set for a target already held still replaces its level.
#[tokio::test]
async fn targets_are_recorded_up_to_the_ceiling_and_refused_past_it() {
    let emulator = Emulator::new();
    emulator.configure_logging("ceiling").await;

    // One short of the ceiling is seeded, so the last admitted record is written by a
    // REQUEST and the one past it is refused by the handler.
    for index in 0..MAX_V2_LOGGING_LEVELS - 1 {
        emulator.seed("THING_GROUP", Some(&format!("group-{index:04}")), "INFO");
    }
    emulator
        .set_ok(
            "the last target the ceiling admits",
            &log_target("CLIENT_ID", Some("sensor-7")),
            "DEBUG",
        )
        .await;
    assert_eq!(
        emulator.stored().len(),
        MAX_V2_LOGGING_LEVELS,
        "case the last target the ceiling admits: expected the ceiling to be reached, got {}",
        emulator.stored().len()
    );

    let before = emulator.stored();
    for (case, target_type, target_name) in [
        ("a new name under a held type", "THING_GROUP", Some("one-more")),
        ("a held name under a new type", "SOURCE_IP", Some("group-0000")),
        ("the account-wide target", DEFAULT_TARGET_TYPE, None),
    ] {
        let reply = emulator
            .set_v2_logging_level(&log_target(target_type, target_name), "WARN")
            .await;
        assert_eq!(
            reply.status,
            410,
            "case {case}: expected SetV2LoggingLevel to refuse with 410, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "LimitExceededException",
            "case {case}: expected LimitExceededException, got {}",
            reply.text()
        );
        assert_eq!(
            emulator.stored(),
            before,
            "case {case}: expected the refusal to record nothing, got {} records",
            emulator.stored().len()
        );
    }

    // A target the account already holds adds no record, so its level is still set at
    // the ceiling.
    emulator
        .set_ok(
            "a target already held",
            &log_target("CLIENT_ID", Some("sensor-7")),
            "DISABLED",
        )
        .await;
    let (reported, _) = emulator
        .reported("a target already held", Some("CLIENT_ID"), None, None)
        .await;
    assert_eq!(
        reported,
        vec![configuration("CLIENT_ID", Some("sensor-7"), "DISABLED")],
        "case a target already held: expected the replaced level, got {reported:?}"
    );
    assert_eq!(
        emulator.stored().len(),
        MAX_V2_LOGGING_LEVELS,
        "case a target already held: expected no record to be added, got {}",
        emulator.stored().len()
    );
}

/// A delete takes ONE target's level away and leaves every other target's alone, and a
/// target that holds no level has nothing to take away: the delete answers the same
/// empty 200 and records nothing, whether it was already deleted, never set, or the
/// account has no logging options at all.
#[tokio::test]
async fn a_delete_takes_one_targets_level_away_and_repeats_as_a_no_op() {
    let emulator = Emulator::new();

    // An account with no logging options holds no level either, and this operation
    // models no not-found or not-configured error
    // (`smithy_query(query="operation",operation="DeleteV2LoggingLevel")`), so the
    // delete is accepted and records nothing.
    emulator
        .delete_ok("an unconfigured account", "THING_GROUP", "factory-floor")
        .await;
    assert!(
        emulator.stored().is_empty(),
        "case an unconfigured account: expected nothing recorded, got {:?}",
        emulator.stored()
    );

    emulator.configure_logging("deletion").await;
    let held: Vec<(&str, Option<&str>, &str)> = vec![
        (DEFAULT_TARGET_TYPE, None, "WARN"),
        ("THING_GROUP", Some("factory-floor"), "DEBUG"),
        ("THING_GROUP", Some("warehouse"), "INFO"),
        ("CLIENT_ID", Some("sensor-7"), "ERROR"),
    ];
    for (target_type, target_name, log_level) in &held {
        emulator
            .set_ok("deletion", &log_target(target_type, *target_name), log_level)
            .await;
    }

    // The rows span what the rule generalises over: a target among several of its type,
    // the last target of a type, the account-wide target (addressed by its type, with a
    // name the target does not have), a target never set, and a repeat of a delete that
    // already happened.
    for (case, target_type, target_name, expected_gone) in [
        (
            "one of several targets of a type",
            "THING_GROUP",
            "factory-floor",
            Some(("THING_GROUP", Some("factory-floor"))),
        ),
        (
            "the same delete again",
            "THING_GROUP",
            "factory-floor",
            None,
        ),
        (
            "a target that never held a level",
            "SOURCE_IP",
            "198.51.100.14",
            None,
        ),
        (
            "the only target of its type",
            "CLIENT_ID",
            "sensor-7",
            Some(("CLIENT_ID", Some("sensor-7"))),
        ),
        (
            "the account-wide target",
            DEFAULT_TARGET_TYPE,
            "a-name-the-default-target-does-not-have",
            Some((DEFAULT_TARGET_TYPE, None)),
        ),
    ] {
        let before = emulator.stored();
        emulator.delete_ok(case, target_type, target_name).await;
        let after = emulator.stored();

        match expected_gone {
            Some((gone_type, gone_name)) => {
                let gone = LoggedTarget {
                    target_type: gone_type.to_string(),
                    target_name: gone_name.map(str::to_string),
                };
                assert!(
                    !after.contains_key(&gone),
                    "case {case}: expected {gone:?} to be gone, got {after:?}"
                );
                let mut expected = before.clone();
                expected.remove(&gone);
                assert_eq!(
                    after, expected,
                    "case {case}: expected every other target to be left alone, got {after:?}"
                );
            }
            None => assert_eq!(
                after, before,
                "case {case}: expected the delete to change nothing, got {after:?}"
            ),
        }
    }

    // What survived the deletes is what the listing reports, and nothing else.
    let (reported, _) = emulator.reported("what survived", None, None, None).await;
    assert_eq!(
        reported,
        vec![configuration("THING_GROUP", Some("warehouse"), "INFO")],
        "case what survived: expected the one target never deleted, got {reported:?}"
    );
}

/// The account-wide `DEFAULT` target names no resource, so its identity is that target
/// type ALONE: a name sent beside it is not part of the target, and the writer, the
/// reader and the remover all resolve the one record it has.
#[tokio::test]
async fn the_account_wide_target_is_identified_by_its_type_alone() {
    let emulator = Emulator::new();
    emulator.configure_logging("the default target").await;

    // The rows differ in the name sent beside the account-wide type: none, one name,
    // then another. Each is the SAME target, so each set replaces the one record.
    for (case, target_name, log_level) in [
        ("no name", None, "WARN"),
        ("a name", Some("ignored"), "DEBUG"),
        ("another name", Some("also-ignored"), "DISABLED"),
    ] {
        emulator
            .set_ok(case, &log_target(DEFAULT_TARGET_TYPE, target_name), log_level)
            .await;
        let (reported, _) = emulator.reported(case, None, None, None).await;
        assert_eq!(
            reported,
            vec![configuration(DEFAULT_TARGET_TYPE, None, log_level)],
            "case {case}: expected one account-wide configuration at {log_level}, got {reported:?}"
        );
    }
}
