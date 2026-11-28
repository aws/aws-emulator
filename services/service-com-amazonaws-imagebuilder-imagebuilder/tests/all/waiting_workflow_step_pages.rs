// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a NON-EMPTY `ListWaitingWorkflowSteps` listing.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is what
//! a signed AWS SDK request observes.
//!
//! The rules under test, each from the modelled documentation beside it:
//!
//! * the operation "lists the workflow steps in your Amazon Web Services account that have
//!   paused at a `WaitForAction` step, and are waiting for you to respond" — so every paused
//!   step the store holds for the calling Region is reported, in the step-execution-id order
//!   the index holds them in, whatever order they were filed in, and a step whose build
//!   version this Region does not hold is reported by no Region;
//! * `maxResults` bounds the page at 1..25, with the documented default of 25 entries when
//!   the request names none, and a page that does not exhaust the set answers a
//!   `nextToken` that continues THAT listing alone — a token this store did not issue, or one
//!   issued for another Region's endpoint, is refused as the value of the parameter it
//!   arrived in rather than read as a position;
//! * the read reports: two identical calls answer the same page, because listing writes
//!   nothing a later caller can observe.
//!
//! The rows span what those rules generalise over: how many steps are paused (none, one, a
//! few, one more than a page carries, two pages), the order they were filed in against the
//! order they are reported in, every page size the model admits, which Region asks, and how a
//! token is wrong.
//!
//! A step is filed through the store's own write path
//! (`ComAmazonawsImagebuilderImagebuilderState::insert_waiting_workflow_step`): a step pauses
//! inside a workflow RUN, so nothing a request to this listing carries can derive the set,
//! exactly as the package inventory a build observed is filed with its build version and read
//! by `ListImagePackages`. That write path's own bound is the last case.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::ops::types::WorkflowStepExecution;
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    image_build_version_arn, ComAmazonawsImagebuilderImagebuilderState, ImageBuildVersionRecord,
    MAX_WAITING_WORKFLOW_STEPS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The largest page the declared `maxResults` admits: the maximum Image Builder enforces,
/// declared in `smithy-model/local-additions.json` (see `smithy-model/LOCAL-ADDITIONS.md`)
/// as `range 1..25`.
const MAX_STEPS_PER_PAGE: usize = 25;

/// The most steps one page carries when the request asks for no page size, from the same
/// member's documentation: "If you don't specify a value, Image Builder returns up to 25
/// items."
const DEFAULT_STEPS_PER_PAGE: usize = 25;

/// The smallest page size a request may ask for: that same range's minimum.
const MIN_STEPS_PER_PAGE: usize = 1;

/// The error this service refuses a parameter value with, as every paginated listing of this
/// crate refuses a token it did not issue
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// One emulator with an empty store, mounted exactly as the router mounts it, beside the
/// shared engine a workflow run would file its paused steps in.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Status and parsed body of one call, signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
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

/// `ListWaitingWorkflowSteps` with an optional page size and token, in `region`.
async fn list_waiting(
    router: &MultiServiceRouter,
    region: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = Map::new();
    if let Some(max_results) = max_results {
        body.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        body.insert("nextToken".to_string(), json!(token));
    }
    call(
        router,
        region,
        "POST",
        "/ListWaitingWorkflowSteps",
        Some(&Value::Object(body)),
    )
    .await
}

/// A step execution id of the modelled `WorkflowStepExecutionId` form
/// (`smithy_query(query="shape",shape="WorkflowStepExecutionId")`:
/// `^step-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`),
/// built from `seed` so two seeds never render one id and the ids sort by seed.
fn step_execution_id(seed: usize) -> String {
    format!("step-{seed:08x}-4b1d-4f3a-9c2e-{seed:012x}")
}

/// A workflow execution id of the modelled `WorkflowExecutionId` form
/// (`smithy_query(query="shape",shape="WorkflowExecutionId")`:
/// `^wf-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
fn workflow_execution_id(seed: usize) -> String {
    format!("wf-{seed:08x}-7e55-4a10-8b44-{seed:012x}")
}

/// File the image build version a paused step's run belongs to, the way the image build path
/// does, and answer its ARN — the step is reported through the Region that holds it.
fn built_image(engine: &Arc<engine::InMemoryEngine>, region: &str) -> String {
    let arn = image_build_version_arn(region, "paused-build-image", "1.0.0", 1);
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(ImageBuildVersionRecord {
            arn: arn.clone(),
            region: region.to_string(),
            name: "paused-build-image".to_string(),
            version: "1.0.0".to_string(),
            build_version: 1,
            ..ImageBuildVersionRecord::default()
        });
    assert!(
        filed,
        "case recording {arn}: expected the store to file the build version"
    );
    arn
}

/// One paused step execution as a workflow run records it.
fn paused_step(seed: usize, image_build_version_arn: Option<&str>) -> WorkflowStepExecution {
    WorkflowStepExecution {
        step_execution_id: Some(step_execution_id(seed)),
        image_build_version_arn: image_build_version_arn.map(str::to_string),
        workflow_execution_id: Some(workflow_execution_id(seed)),
        workflow_build_version_arn: None,
        name: Some(format!("WaitStep{seed}")),
        action: Some("WaitForAction".to_string()),
        start_time: Some("2026-11-06T01:09:45Z".to_string()),
    }
}

/// File one paused step through the store's own write path.
fn file_paused_step(
    engine: &Arc<engine::InMemoryEngine>,
    step: WorkflowStepExecution,
) -> (bool, Option<String>) {
    let id = step.step_execution_id.clone();
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_waiting_workflow_step(step);
    (filed, id)
}

/// The step execution ids one page carries, in the order it carried them.
fn page_step_ids(case: &str, body: &Value) -> Vec<String> {
    body.get("steps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a steps list, got {body}"))
        .iter()
        .map(|step| {
            step.get("stepExecutionId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a stepExecutionId in {step}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` one page carries, if any.
fn page_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_paused_step_of_the_calling_region_is_listed_one_page_at_a_time() {
    // Each row is one waiting set: the case and how many steps are paused in it. Each row
    // files into a store of its own, since the listing is the Region's rather than one
    // resource's.
    let rows: Vec<(&str, usize)> = vec![
        ("no step paused", 0),
        ("one step paused", 1),
        ("a few steps paused", 3),
        (
            "one step more than a default page carries",
            DEFAULT_STEPS_PER_PAGE + 1,
        ),
        ("two default pages of steps", DEFAULT_STEPS_PER_PAGE * 2),
    ];

    for (case, count) in rows {
        let (router, engine) = emulator();
        let built = built_image(&engine, REGION);
        // Filed in DESCENDING seed order, so the order a case files in is not the order the
        // listing reports.
        for seed in (1..=count).rev() {
            let (filed, id) = file_paused_step(&engine, paused_step(seed, Some(&built)));
            assert!(filed, "case {case:?}: expected {id:?} to be filed");
        }
        let expected: Vec<String> = (1..=count).map(step_execution_id).collect();

        // With no page size the answer carries at most the documented default, and a token
        // only while a step it did not carry remains.
        let (status, listed) = list_waiting(&router, REGION, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let first_page = page_step_ids(case, &listed);
        let head: Vec<String> = expected
            .iter()
            .take(DEFAULT_STEPS_PER_PAGE)
            .cloned()
            .collect();
        assert_eq!(
            first_page, head,
            "case {case:?}: expected {head:?}, got {first_page:?}"
        );
        assert_eq!(
            page_token(&listed).is_some(),
            expected.len() > DEFAULT_STEPS_PER_PAGE,
            "case {case:?}: expected a nextToken only with steps left over, got {listed}"
        );

        // Listing writes nothing a later caller sees: the same call answers the same page.
        let (_, again) = list_waiting(&router, REGION, None, None).await;
        let repeated = page_step_ids(case, &again);
        assert_eq!(
            repeated, first_page,
            "case {case:?}: expected a read to leave the waiting set alone, got {repeated:?} \
             after {first_page:?}"
        );

        // Every page size the model admits, walked to the end: the pages together are the
        // whole waiting set, in order, and none is longer than the size asked for.
        for page_size in [
            MIN_STEPS_PER_PAGE,
            2,
            7,
            DEFAULT_STEPS_PER_PAGE,
            MAX_STEPS_PER_PAGE,
        ] {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let (status, page) =
                    list_waiting(&router, REGION, Some(page_size), token.as_deref()).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} at {page_size} per page: expected HTTP 200, got {status} \
                     with {page}"
                );
                let entries = page_step_ids(case, &page);
                assert!(
                    entries.len() <= page_size,
                    "case {case:?} at {page_size} per page: expected at most {page_size} \
                     entries, got {entries:?}"
                );
                walked.extend(entries);
                pages += 1;
                token = page_token(&page);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= expected.len(),
                    "case {case:?} at {page_size} per page: expected the walk to end, got \
                     {pages} pages for {} steps",
                    expected.len()
                );
            }
            assert_eq!(
                walked, expected,
                "case {case:?} at {page_size} per page: expected {expected:?}, got {walked:?}"
            );
            let want_pages = expected.len().div_ceil(page_size).max(1);
            assert_eq!(
                pages, want_pages,
                "case {case:?} at {page_size} per page: expected {want_pages} page(s), got \
                 {pages}"
            );
        }
    }
}

#[tokio::test]
async fn a_step_is_listed_only_through_the_region_that_holds_its_build_version() {
    let (router, engine) = emulator();
    let here = built_image(&engine, REGION);
    let elsewhere = built_image(&engine, OTHER_REGION);
    // A build version ARN of the calling Region that nothing built, so the step naming it
    // belongs to no build this store holds.
    let unbuilt = image_build_version_arn(REGION, "never-built-image", "9.9.9", 7);

    // Each row is one filed step: the case, the build version its run belongs to, and which
    // Region's endpoint reports it (none, for a step no Region can resolve).
    let rows: Vec<(&str, usize, Option<&str>, Option<&str>)> = vec![
        (
            "a step of a build version this Region holds",
            1,
            Some(here.as_str()),
            Some(REGION),
        ),
        (
            "a second step of that same build version",
            2,
            Some(here.as_str()),
            Some(REGION),
        ),
        (
            "a step of another Region's build version",
            3,
            Some(elsewhere.as_str()),
            Some(OTHER_REGION),
        ),
        (
            "a step of a build version nothing built",
            4,
            Some(unbuilt.as_str()),
            None,
        ),
        ("a step naming no build version at all", 5, None, None),
    ];

    for (case, seed, arn, _) in &rows {
        let (filed, id) = file_paused_step(&engine, paused_step(*seed, *arn));
        assert!(filed, "case {case:?}: expected {id:?} to be filed");
    }

    for region in [REGION, OTHER_REGION] {
        let mut expected: Vec<String> = rows
            .iter()
            .filter(|(_, _, _, reported_by)| *reported_by == Some(region))
            .map(|(_, seed, _, _)| step_execution_id(*seed))
            .collect();
        expected.sort();

        let (status, listed) = list_waiting(&router, region, None, None).await;
        assert_eq!(
            status, 200,
            "case {region:?}'s waiting set: expected HTTP 200, got {status} with {listed}"
        );
        let listed_ids = page_step_ids(region, &listed);
        assert_eq!(
            listed_ids, expected,
            "case {region:?}'s waiting set: expected {expected:?}, got {listed_ids:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_belongs_to_the_listing_that_issued_it() {
    let (router, engine) = emulator();
    let here = built_image(&engine, REGION);
    let elsewhere = built_image(&engine, OTHER_REGION);
    for seed in 1..=3 {
        let (filed, id) = file_paused_step(&engine, paused_step(seed, Some(&here)));
        assert!(filed, "case staging the listing: expected {id:?} to be filed");
    }
    let (filed, id) = file_paused_step(&engine, paused_step(9, Some(&elsewhere)));
    assert!(
        filed,
        "case staging the other Region: expected {id:?} to be filed"
    );

    let (status, page) = list_waiting(&router, REGION, Some(1), None).await;
    assert_eq!(
        status, 200,
        "case the first page: expected HTTP 200, got {status} with {page}"
    );
    let issued = page_token(&page)
        .unwrap_or_else(|| panic!("case the first page: expected a nextToken in {page}"));

    // A token this listing did not issue is refused however it is wrong: fabricated, edited,
    // or spent on another Region's endpoint.
    let edited = format!("{issued}00");
    let refusals: Vec<(&str, &str, &str)> = vec![
        ("a fabricated token", "not-a-token", REGION),
        ("an edited token", edited.as_str(), REGION),
        (
            "a token issued for another Region",
            issued.as_str(),
            OTHER_REGION,
        ),
    ];
    for (case, token, region) in &refusals {
        let (status, body) = list_waiting(&router, region, Some(1), Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(case, &body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?}"
        );
    }

    // The token it DID issue continues its own listing, at the step after the page it ended.
    let (status, second) = list_waiting(&router, REGION, Some(1), Some(&issued)).await;
    assert_eq!(
        status, 200,
        "case the token's own listing: expected HTTP 200, got {status} with {second}"
    );
    let continued = page_step_ids("the token's own listing", &second);
    assert_eq!(
        continued,
        vec![step_execution_id(2)],
        "case the token's own listing: expected the step after the first, got {continued:?}"
    );
}

#[tokio::test]
async fn the_paused_step_index_is_bounded_where_it_is_filed() {
    let (router, engine) = emulator();
    let built = built_image(&engine, REGION);

    // A step carrying no id is filed nowhere: nothing could list it or name it in an action.
    let (filed, _) = file_paused_step(
        &engine,
        WorkflowStepExecution {
            step_execution_id: None,
            ..paused_step(1, Some(&built))
        },
    );
    assert!(
        !filed,
        "case a step with no step execution id: expected the store to file nothing"
    );

    // The index holds as many steps as this emulator keeps; one more is not filed.
    for seed in 1..=MAX_WAITING_WORKFLOW_STEPS {
        let (filed, id) = file_paused_step(&engine, paused_step(seed, Some(&built)));
        assert!(filed, "case filling the index: expected {id:?} to be filed");
    }
    let state = engine.state::<ComAmazonawsImagebuilderImagebuilderState>();
    assert!(
        state.lock().waiting_workflow_steps_store_is_full(),
        "case a full index: expected the store to report itself full at \
         {MAX_WAITING_WORKFLOW_STEPS} steps, got {}",
        state.lock().waiting_workflow_steps.len()
    );
    let (filed, _) = file_paused_step(
        &engine,
        paused_step(MAX_WAITING_WORKFLOW_STEPS + 1, Some(&built)),
    );
    assert!(
        !filed,
        "case one step past the bound: expected the store to file nothing, got {} steps",
        state.lock().waiting_workflow_steps.len()
    );
    // A step the index already holds is replaced whatever the count, since that stores no
    // additional record.
    let (filed, _) = file_paused_step(&engine, paused_step(1, Some(&built)));
    assert!(
        filed,
        "case replacing a held step: expected the store to accept it at the bound"
    );
    assert_eq!(
        state.lock().waiting_workflow_steps.len(),
        MAX_WAITING_WORKFLOW_STEPS,
        "case replacing a held step: expected the index to hold the same count"
    );

    // A full index still answers one page at a time, and the step past the bound is nowhere
    // in the listing.
    let (status, listed) = list_waiting(&router, REGION, Some(MAX_STEPS_PER_PAGE), None).await;
    assert_eq!(
        status, 200,
        "case a full index: expected HTTP 200, got {status} with {listed}"
    );
    let page = page_step_ids("a full index", &listed);
    assert_eq!(
        page.len(),
        MAX_STEPS_PER_PAGE,
        "case a full index: expected a full page, got {page:?}"
    );
    let overflow = step_execution_id(MAX_WAITING_WORKFLOW_STEPS + 1);
    assert!(
        !page.contains(&overflow),
        "case a full index: expected {overflow} to be nowhere in {page:?}"
    );
}
