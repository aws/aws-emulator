// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the store bounds `CreateConfigurationManager` measures a
//! request against once a teardown has come due.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an operation answers from the state the clock already owes at
//! the instant it runs, so a record whose removal is due is not counted against the
//! request that follows it and the answer never depends on whether some unrelated call
//! ran in between. Here that is the two store bounds a create is measured against — how
//! many configuration managers the store holds and how many configurations it deploys —
//! each read after a due teardown has been reclaimed, while a teardown still in flight
//! keeps counting because its records are still deployed.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    MAX_CONFIGURATIONS, MAX_CONFIGURATION_MANAGERS, MAX_DEPLOYMENT_TARGETS, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns, and so
/// the only Region whose endpoint serves it.
const REGION: &str = "us-west-2";

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for the reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// Long enough that a teardown the clock has made due has come due.
///
/// The rows below are about what the store owes when NOTHING has run since that instant,
/// so this suite may not reach the store to learn that the teardown came due: a read would
/// be the very intervening call the `read_in_between == false` rows exist to exclude.
/// Waiting out the span is therefore what the rule under test allows here, and the span is
/// given generous slack rather than being timed tightly.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One configuration definition deploying into `regions` Regions of this account, so a
/// definition's fan-out — and therefore how many configurations a create deploys — is
/// named by one number.
fn definition(regions: usize) -> Value {
    let targeted: String = (0..regions)
        .map(|index| format!("eu-west-{}", index + 1))
        .collect::<Vec<String>>()
        .join(",");
    json!({
        "Type": TYPE,
        "Parameters": {
            "TargetOrganizationalUnits": "ou-1",
            "TargetRegions": targeted,
        },
    })
}

/// The body of a create carrying `definitions` definitions, each deploying `regions`
/// configurations.
fn create_body(name: &str, definitions: usize, regions: usize) -> Value {
    json!({
        "Name": name,
        "ConfigurationDefinitions": (0..definitions)
            .map(|_| definition(regions))
            .collect::<Vec<Value>>(),
    })
}

/// Create one manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = call(router, "POST", "/configurationManager", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case}: filling the store must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// Which store bound the create under test is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    /// How many configuration managers the store holds
    /// (`MAX_CONFIGURATION_MANAGERS`).
    Managers,
    /// How many configurations the store deploys (`MAX_CONFIGURATIONS`).
    Configurations,
}

/// What became of the one manager this row tears down before the create under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Teardown {
    /// Nothing is deleted: the store is at its bound and stays there.
    None,
    /// A delete was accepted and its teardown is still in flight, so the manager and
    /// its configurations are still deployed.
    InFlight,
    /// A delete was accepted and the clock has made its teardown due, so the manager
    /// and its configurations are owed removal.
    Due,
}

/// Fill the store to `bound`, answering the ARN of the manager a row tears down.
///
/// At the manager bound the store holds `MAX_CONFIGURATION_MANAGERS` managers of one
/// configuration each; at the configuration bound one manager deploys exactly
/// `MAX_CONFIGURATIONS` configurations, as many definitions of the per-definition
/// maximum fan-out as that takes.
async fn fill(router: &MultiServiceRouter, case: &str, bound: Bound) -> String {
    match bound {
        Bound::Managers => {
            let mut first = String::new();
            for index in 0..MAX_CONFIGURATION_MANAGERS {
                let arn =
                    create_ok(router, case, &create_body(&format!("filler-{index}"), 1, 1)).await;
                if index == 0 {
                    first = arn;
                }
            }
            first
        }
        Bound::Configurations => {
            create_ok(
                router,
                case,
                &create_body(
                    "filler",
                    MAX_CONFIGURATIONS / MAX_DEPLOYMENT_TARGETS,
                    MAX_DEPLOYMENT_TARGETS,
                ),
            )
            .await
        }
    }
}

/// A create against a store at one of its bounds is measured against what the clock
/// owes at the instant it runs: the records of a teardown that has come due are not
/// counted against it, the records of a teardown still in flight are, and no unrelated
/// call has to run first for either answer to hold.
#[tokio::test]
async fn a_create_is_measured_against_the_store_the_clock_owes() {
    // (case, which bound the store is filled to, what became of one manager, whether an
    //  unrelated read runs between the teardown and the create, the status the create
    //  must answer)
    let cases: Vec<(&str, Bound, Teardown, bool, u16)> = vec![
        (
            "manager bound, nothing torn down",
            Bound::Managers,
            Teardown::None,
            false,
            409,
        ),
        (
            "manager bound, teardown in flight",
            Bound::Managers,
            Teardown::InFlight,
            false,
            409,
        ),
        (
            "manager bound, teardown due, no call in between",
            Bound::Managers,
            Teardown::Due,
            false,
            200,
        ),
        (
            "manager bound, teardown due, an unrelated listing in between",
            Bound::Managers,
            Teardown::Due,
            true,
            200,
        ),
        (
            "configuration bound, nothing torn down",
            Bound::Configurations,
            Teardown::None,
            false,
            409,
        ),
        (
            "configuration bound, teardown in flight",
            Bound::Configurations,
            Teardown::InFlight,
            false,
            409,
        ),
        (
            "configuration bound, teardown due, no call in between",
            Bound::Configurations,
            Teardown::Due,
            false,
            200,
        ),
        (
            "configuration bound, teardown due, an unrelated listing in between",
            Bound::Configurations,
            Teardown::Due,
            true,
            200,
        ),
    ];

    for (case, bound, teardown, read_in_between, want_status) in cases {
        // A store of its own per row, so what the create under test is measured against
        // is only what this row filled and tore down.
        let router = emulator();
        let torn_down = fill(&router, case, bound).await;

        if teardown != Teardown::None {
            let (status, answer) = call(
                &router,
                "DELETE",
                &format!("/configurationManager/{}", label(&torn_down)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: DeleteConfigurationManager must be accepted, got {status} / {answer:?}"
            );
        }
        if teardown == Teardown::Due {
            // No read here: the rows with `read_in_between == false` are about what the
            // store owes when nothing has run since the teardown came due, so this wait
            // may not drive a request against the store under test.
            tokio::time::sleep(past_transition()).await;
        }
        if read_in_between {
            let (status, answer) =
                call(&router, "POST", "/listConfigurationManagers", Some(&json!({}))).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
            );
        }

        // The create under test: one definition deploying one configuration, so it is
        // refused only by the bound this row filled.
        let (status, answer) = call(
            &router,
            "POST",
            "/configurationManager",
            Some(&create_body("after-teardown", 1, 1)),
        )
        .await;
        assert_eq!(
            status, want_status,
            "case {case}: expected {want_status}, got {status} / {answer:?}"
        );

        if want_status != 200 {
            assert_eq!(
                error_code(&answer),
                "ConflictException",
                "case {case}: expected ConflictException, got {answer:?}"
            );
            // A refused create minted nothing: the manager it would have created is not
            // readable, and the one being torn down is still the store's own business.
            continue;
        }

        // Round trip: what the accepted create minted is readable through the service's
        // own reader, and the manager whose teardown came due is gone from it.
        let created = answer
            .get("ManagerArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
            .to_string();
        let (status, read_back) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&created)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the created manager must be readable, got {status} / {read_back:?}"
        );
        assert_eq!(
            read_back.get("ManagerArn").and_then(Value::as_str),
            Some(created.as_str()),
            "case {case}: expected the manager {created}, got {read_back:?}"
        );
        let (status, gone) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&torn_down)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: the torn-down manager names no resource this endpoint serves, got \
             {status} / {gone:?}"
        );
    }
}
