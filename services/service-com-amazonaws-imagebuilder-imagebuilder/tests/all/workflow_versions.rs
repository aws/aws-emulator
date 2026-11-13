// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workflow slice of this service: `CreateWorkflow`,
//! `GetWorkflow` and `ListWorkflows`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * a workflow is a VERSIONED, immutable resource of this service, so a create mints one
//!   build version of a stage+name+version triple and every later read reports exactly what
//!   that create stored; a repeat of one triple mints the NEXT build version, and a
//!   reference whose trailing version nodes are `x` wildcards reaches the newest build it
//!   matches;
//! * `dryRun` is the caller asking for the validation WITHOUT the create: a request that
//!   would have succeeded is answered `DryRunOperationException` (HTTP 412) with the fixed
//!   sentence "Request would have succeeded, but dryRun flag is set.", and NOTHING is
//!   written — no workflow in the listing, no workflow to read, and the `clientToken` still
//!   free for the real request.
//!
//! The rows span what the rules generalise over: which stage the workflow applies to, its
//! version, which optional members the create carries, whether the document declares
//! parameters, how many builds of one triple exist, and which form of reference reads it
//! back.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the case that checks a regional endpoint reaches only its own
/// Region's workflows.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The sentence a dry run is refused with.
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but dryRun flag is set.";

/// A workflow document declaring one step and no parameters.
const PLAIN_DOCUMENT: &str = "name: plain\nschemaVersion: 1.0\nsteps:\n  - name: Run\n    \
                              action: ExecuteBash\n    inputs:\n      commands:\n        - echo \
                              hello\n";

/// A workflow document declaring two input parameters, so a read reports the pair the
/// document defined.
const PARAMETERIZED_DOCUMENT: &str = "name: parameterized\nschemaVersion: 1.0\nparameters:\n  - \
                                      name: waitFor\n    type: boolean\n    default: false\n    \
                                      description: whether to wait\n  - name: message\n    type: \
                                      string\nsteps:\n  - name: Run\n    action: ExecuteBash\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
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
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
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
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a refusal carries, from `__type`.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// The `message` a refusal carries.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// Assert the answer is a refusal carrying `status`, `code` and `message`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: u16, code: &str, message: &str) {
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected}, got {status} with {body}"
    );
    let actual_code = error_code(case, body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code:?}, got {actual_code:?} with {body}"
    );
    let actual_message = error_message(case, body);
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// The ARN one build version of a workflow carries.
fn workflow_build_arn(
    region: &str,
    stage: &str,
    normalized: &str,
    version: &str,
    build: u32,
) -> String {
    format!(
        "arn:aws:imagebuilder:{region}:{ACCOUNT}:workflow/{stage}/{normalized}/{version}/{build}"
    )
}

/// The version ARN (no build number) of a workflow name at `version`.
fn workflow_version_arn(region: &str, stage: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:workflow/{stage}/{normalized}/{version}")
}

/// `CreateWorkflow` with `body`, in `region`.
async fn create_workflow_with(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateWorkflow", Some(body)).await
}

/// `GetWorkflow` for the reference `arn`, in `region`.
async fn get_workflow(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetWorkflow?workflowBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `ListWorkflows` with `body`, in `region`.
async fn list_workflows(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/ListWorkflows", Some(body)).await
}

/// The ARNs one `ListWorkflows` answer carries, in the order it reports them.
fn listed_arns(case: &str, body: &Value) -> Vec<String> {
    body.get("workflowVersionList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a workflowVersionList in {body}"))
        .iter()
        .map(|entry| text(case, entry, "arn"))
        .collect()
}

/// The tags one member of a body carries, as a map.
fn tags_in(case: &str, body: &Value, member: &str) -> BTreeMap<String, String> {
    body.get(member)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} object in {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected text in {member}.{key}"))
                    .to_string(),
            )
        })
        .collect()
}

/// One workflow a create stores: everything the request carries, beside what a read of it
/// then has to report.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// The name as the request carries it.
    name: &'static str,
    /// The normalized name its ARN carries.
    normalized: &'static str,
    /// The stage, as the modelled `type` enum spells it.
    workflow_type: &'static str,
    /// The stage segment its ARN carries, which is that value in lower case.
    stage: &'static str,
    /// The three assigned version nodes.
    version: &'static str,
    /// The document the workflow carries.
    document: &'static str,
    /// The names of the parameters a read reports, read from the document.
    parameters: &'static [&'static str],
    /// The optional members the request carries, beside the required ones.
    extra: fn() -> Value,
}

/// The workflows the round-trip case stores. The rows differ along what the rule
/// generalises over: the stage (every value of the modelled `WorkflowType`:
/// `smithy_query(query="shape",shape="WorkflowType")` reports BUILD, TEST and
/// DISTRIBUTION), the version, the name's normalization, whether the document declares
/// parameters, and which optional members the create carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a build workflow carrying only the required members",
            name: "build-flow",
            normalized: "build-flow",
            workflow_type: "BUILD",
            stage: "build",
            version: "1.0.0",
            document: PLAIN_DOCUMENT,
            parameters: &[],
            extra: || json!({}),
        },
        Row {
            case: "a test workflow whose name normalizes, with every optional member",
            name: "Nightly Test_Flow",
            normalized: "nightly-test-flow",
            workflow_type: "TEST",
            stage: "test",
            version: "2.11.3",
            document: PARAMETERIZED_DOCUMENT,
            parameters: &["waitFor", "message"],
            extra: || {
                json!({
                    "description": "the nightly test workflow",
                    "changeDescription": "the first version",
                    "kmsKeyId": "alias/aws/imagebuilder",
                    "tags": { "Owner": "platform" },
                })
            },
        },
        Row {
            case: "a distribution workflow at the version bound",
            name: "dist-flow",
            normalized: "dist-flow",
            workflow_type: "DISTRIBUTION",
            stage: "distribution",
            version: "1073741823.0.1",
            document: PLAIN_DOCUMENT,
            parameters: &[],
            extra: || json!({ "description": "the distribution workflow" }),
        },
    ]
}

/// The request body one row's create carries, with `token` and whatever `dry_run` says.
fn create_body(row: &Row, token: &str, dry_run: bool) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(row.name));
    body.insert("semanticVersion".to_string(), json!(row.version));
    body.insert("type".to_string(), json!(row.workflow_type));
    body.insert("data".to_string(), json!(row.document));
    body.insert("clientToken".to_string(), json!(token));
    if dry_run {
        body.insert("dryRun".to_string(), json!(true));
    }
    if let Value::Object(extra) = (row.extra)() {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

#[tokio::test]
async fn a_created_workflow_is_what_every_later_read_reports() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let (status, answer) =
            create_workflow_with(&router, REGION, &create_body(&row, row.normalized, false)).await;
        let answer = assert_ok(case, status, answer);
        let expected_arn = workflow_build_arn(REGION, row.stage, row.normalized, row.version, 1);
        let actual = text(case, &answer, "workflowBuildVersionArn");
        assert_eq!(
            actual, expected_arn,
            "case {case:?}: expected the build version {expected_arn:?}, got {actual:?}"
        );
        assert_eq!(
            text(case, &answer, "clientToken"),
            row.normalized,
            "case {case:?}: expected the create to echo its own token"
        );
        // The four wildcard references the create reports all name this workflow's stage
        // and name, and none of them pins a build.
        let references = answer
            .get("latestVersionReferences")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: expected latestVersionReferences"));
        let latest = text(case, &answer["latestVersionReferences"], "latestVersionArn");
        assert_eq!(
            latest,
            workflow_version_arn(REGION, row.stage, row.normalized, "x.x.x"),
            "case {case:?}: expected the fully wildcarded reference, got {latest:?}"
        );
        assert_eq!(
            references.len(),
            4,
            "case {case:?}: expected four wildcard references, got {references:?}"
        );

        // The read reports what the create stored, whether it is asked by the build
        // version ARN or by a wildcard version ARN that reaches it.
        for (reference_case, reference) in [
            ("its build version ARN", expected_arn.clone()),
            (
                "a fully wildcarded version ARN",
                workflow_version_arn(REGION, row.stage, row.normalized, "x.x.x"),
            ),
        ] {
            let case = format!("{case}, read by {reference_case}");
            let (status, read) = get_workflow(&router, REGION, &reference).await;
            let read = assert_ok(&case, status, read);
            let workflow = read
                .get("workflow")
                .unwrap_or_else(|| panic!("case {case:?}: expected a workflow in {read}"));
            for (member, expected) in [
                ("arn", expected_arn.as_str()),
                ("name", row.name),
                ("version", row.version),
                ("type", row.workflow_type),
                ("data", row.document),
                ("owner", ACCOUNT),
            ] {
                let actual = text(&case, workflow, member);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {member} {expected:?}, got {actual:?}"
                );
            }
            let declared: Vec<String> = workflow
                .get("parameters")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| text(&case, entry, "name"))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                declared,
                row.parameters
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: expected the document's parameters, got {declared:?}"
            );
            assert!(
                workflow.get("dateCreated").is_some(),
                "case {case:?}: expected a dateCreated in {workflow}"
            );
        }

        // And the tags the create carried are the tags the tagging surface reports for it.
        let expected_tags: BTreeMap<String, String> = match (row.extra)() {
            Value::Object(extra) => extra
                .get("tags")
                .and_then(Value::as_object)
                .map(|tags| {
                    tags.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => BTreeMap::new(),
        };
        let case = format!("{case}, its tags");
        let (status, listed) = call_in(
            &router,
            REGION,
            "GET",
            &format!("/tags/{}", encode_label(&expected_arn)),
            None,
        )
        .await;
        let listed = assert_ok(&case, status, listed);
        let actual = tags_in(&case, &listed, "tags");
        assert_eq!(
            actual, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {actual:?}"
        );
    }

    // Every workflow stored above is listed once, ordered by the ARN its entry reports.
    let (status, listed) = list_workflows(&router, REGION, &json!({})).await;
    let listed = assert_ok("the listing", status, listed);
    let arns = listed_arns("the listing", &listed);
    let mut expected: Vec<String> = rows()
        .iter()
        .map(|row| workflow_version_arn(REGION, row.stage, row.normalized, row.version))
        .collect();
    expected.sort();
    assert_eq!(
        arns, expected,
        "case \"the listing\": expected one entry per stored version, got {arns:?}"
    );

    // A regional endpoint reaches only its own Region's workflows.
    let other = workflow_build_arn(OTHER_REGION, "build", "build-flow", "1.0.0", 1);
    let (status, read) = get_workflow(&router, OTHER_REGION, &other).await;
    assert_refused(
        "a workflow of another Region",
        status,
        &read,
        404,
        "ResourceNotFoundException",
        &format!("The following required resource 'Workflow' cannot be found: '{other}'."),
    );
    let (status, listed) = list_workflows(&router, OTHER_REGION, &json!({})).await;
    let listed = assert_ok("the other Region's listing", status, listed);
    assert!(
        listed_arns("the other Region's listing", &listed).is_empty(),
        "case \"the other Region's listing\": expected no workflow, got {listed}"
    );
}

#[tokio::test]
async fn a_repeat_of_one_name_and_version_mints_the_next_build_version() {
    let router = emulator();
    let row = &rows()[0];

    // Three creates of the same stage+name+version, each with its own token, count the
    // build number up — and a wildcard reference reaches the NEWEST of them.
    for build in 1..=3u32 {
        let case = format!("build {build}");
        let (status, answer) =
            create_workflow_with(&router, REGION, &create_body(row, &case, false)).await;
        let answer = assert_ok(&case, status, answer);
        let expected = workflow_build_arn(REGION, row.stage, row.normalized, row.version, build);
        let actual = text(&case, &answer, "workflowBuildVersionArn");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build version {expected:?}, got {actual:?}"
        );

        let reference = workflow_version_arn(REGION, row.stage, row.normalized, "x.x.x");
        let (status, read) = get_workflow(&router, REGION, &reference).await;
        let read = assert_ok(&case, status, read);
        let reached = text(&case, &read["workflow"], "arn");
        assert_eq!(
            reached, expected,
            "case {case:?}: expected the wildcard reference to reach {expected:?}, got \
             {reached:?}"
        );
    }

    // Repeating ONE request — the same token and the same parameters — answers the original
    // response rather than minting a fourth build.
    let (status, answer) =
        create_workflow_with(&router, REGION, &create_body(row, "build 3", false)).await;
    let answer = assert_ok("the retried create", status, answer);
    let actual = text("the retried create", &answer, "workflowBuildVersionArn");
    assert_eq!(
        actual,
        workflow_build_arn(REGION, row.stage, row.normalized, row.version, 3),
        "case \"the retried create\": expected the original response, got {actual:?}"
    );

    // One entry per version, whatever how many builds it has; `byName` aggregates the
    // versions of one name under the name ARN.
    for (case, body, expected) in [
        (
            "one entry per version",
            json!({}),
            vec![workflow_version_arn(
                REGION,
                row.stage,
                row.normalized,
                row.version,
            )],
        ),
        (
            "one entry per name",
            json!({ "byName": true }),
            vec![workflow_version_arn(
                REGION,
                row.stage,
                row.normalized,
                "x.x.x",
            )],
        ),
    ] {
        let (status, listed) = list_workflows(&router, REGION, &body).await;
        let listed = assert_ok(case, status, listed);
        let arns = listed_arns(case, &listed);
        assert_eq!(
            arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }
}

#[tokio::test]
async fn a_dry_run_validates_and_writes_nothing() {
    let router = emulator();

    for row in rows() {
        let case = format!("a dry run of {}", row.case);
        let token = format!("dry-{}", row.normalized);
        let (status, answer) =
            create_workflow_with(&router, REGION, &create_body(&row, &token, true)).await;
        assert_refused(
            &case,
            status,
            &answer,
            412,
            "DryRunOperationException",
            DRY_RUN_MESSAGE,
        );

        // Nothing was written: the listing filtered by this row's own name carries no
        // entry, and there is nothing to read back.
        let by_name = json!({ "filters": [{ "name": "name", "values": [row.name] }] });
        let (status, listed) = list_workflows(&router, REGION, &by_name).await;
        let listed = assert_ok(&case, status, listed);
        assert!(
            listed_arns(&case, &listed).is_empty(),
            "case {case:?}: expected the dry run to store no workflow, got {listed}"
        );
        let arn = workflow_build_arn(REGION, row.stage, row.normalized, row.version, 1);
        let (status, read) = get_workflow(&router, REGION, &arn).await;
        assert_refused(
            &format!("{case}, read back"),
            status,
            &read,
            404,
            "ResourceNotFoundException",
            &format!("The following required resource 'Workflow' cannot be found: '{arn}'."),
        );

        // And the dry run spent nothing: the SAME token still creates the workflow, which
        // is then the one entry its name's listing carries.
        let (status, answer) =
            create_workflow_with(&router, REGION, &create_body(&row, &token, false)).await;
        let created = format!("{case}, then created");
        let answer = assert_ok(&created, status, answer);
        let actual = text(&created, &answer, "workflowBuildVersionArn");
        assert_eq!(
            actual, arn,
            "case {case:?}: expected the token to still create {arn:?}, got {actual:?}"
        );
        let (status, listed) = list_workflows(&router, REGION, &by_name).await;
        let listed = assert_ok(&created, status, listed);
        assert_eq!(
            listed_arns(&created, &listed),
            vec![workflow_version_arn(
                REGION,
                row.stage,
                row.normalized,
                row.version
            )],
            "case {created:?}: expected exactly the workflow the real create stored"
        );
    }
}

#[tokio::test]
async fn a_create_is_refused_for_what_the_model_cannot_say() {
    let router = emulator();
    let row = &rows()[0];

    // (case, what to change about an otherwise valid request, the status, code and message)
    let cases: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "neither a document nor a document location",
            json!({ "data": Value::Null }),
            400,
            "InvalidParameterCombinationException",
            "The value supplied for parameter 'data' and 'uri' is not valid. You must specify \
             either data or a uri."
                .to_string(),
        ),
        (
            "both a document and a document location",
            json!({ "uri": "s3://documents/workflow.yaml" }),
            400,
            "InvalidParameterCombinationException",
            "The value supplied for parameter 'data' and 'uri' is not valid. You must specify \
             either data or a uri."
                .to_string(),
        ),
        (
            "a version node past the documented bound",
            json!({ "semanticVersion": "1073741824.0.0" }),
            400,
            "InvalidVersionNumberException",
            "The value supplied for parameter 'semanticVersion' is not valid. The supplied \
             semantic version does not follow the required format."
                .to_string(),
        ),
        (
            "a document that is not a YAML mapping",
            json!({ "data": "- a sequence\n" }),
            400,
            "InvalidParameterValueException",
            "The value supplied for parameter 'data' is not valid. The workflow document must \
             be a YAML mapping of document keys to values."
                .to_string(),
        ),
    ];

    for (position, (case, change, status_code, code, message)) in
        cases.into_iter().enumerate()
    {
        let mut body = match create_body(row, &format!("refusal-{position}"), false) {
            Value::Object(body) => body,
            other => panic!("case {case:?}: expected an object body, got {other}"),
        };
        if let Value::Object(change) = change {
            for (member, value) in change {
                if value.is_null() {
                    body.remove(&member);
                } else {
                    body.insert(member, value);
                }
            }
        }
        let (status, answer) = create_workflow_with(&router, REGION, &Value::Object(body)).await;
        assert_refused(case, status, &answer, status_code, code, &message);

        // Every refusal above stored nothing.
        let (status, listed) = list_workflows(&router, REGION, &json!({})).await;
        let listed = assert_ok(case, status, listed);
        assert!(
            listed_arns(case, &listed).is_empty(),
            "case {case:?}: expected a refused create to store nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_workflow_listing_filters_and_pages_over_what_it_holds() {
    let router = emulator();

    // Three workflows of one stage, differing in name and version, so a filter and a page
    // each select a known subset.
    for (name, version) in [("alpha", "1.0.0"), ("beta", "1.0.0"), ("beta", "2.0.0")] {
        let row = Row {
            case: "listing fixture",
            name,
            normalized: name,
            workflow_type: "BUILD",
            stage: "build",
            version,
            document: PLAIN_DOCUMENT,
            parameters: &[],
            extra: || json!({}),
        };
        let case = format!("create {name} {version}");
        let (status, answer) = create_workflow_with(
            &router,
            REGION,
            &create_body(&row, &format!("token-{name}-{version}"), false),
        )
        .await;
        assert_ok(&case, status, answer);
    }

    // (case, the request, the entries it answers with)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter",
            json!({}),
            vec![
                workflow_version_arn(REGION, "build", "alpha", "1.0.0"),
                workflow_version_arn(REGION, "build", "beta", "1.0.0"),
                workflow_version_arn(REGION, "build", "beta", "2.0.0"),
            ],
        ),
        (
            "a filter on the name",
            json!({ "filters": [{ "name": "name", "values": ["beta"] }] }),
            vec![
                workflow_version_arn(REGION, "build", "beta", "1.0.0"),
                workflow_version_arn(REGION, "build", "beta", "2.0.0"),
            ],
        ),
        (
            "a filter on a wildcard version",
            json!({ "filters": [{ "name": "version", "values": ["2.x.x"] }] }),
            vec![workflow_version_arn(REGION, "build", "beta", "2.0.0")],
        ),
        (
            "a filter on the stage",
            json!({ "filters": [{ "name": "type", "values": ["TEST"] }] }),
            Vec::new(),
        ),
        (
            "an owner this store holds nothing for",
            json!({ "owner": "Amazon" }),
            Vec::new(),
        ),
    ];
    for (case, body, expected) in cases {
        let (status, listed) = list_workflows(&router, REGION, &body).await;
        let listed = assert_ok(case, status, listed);
        let arns = listed_arns(case, &listed);
        assert_eq!(
            arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }

    // One entry per page, followed to the end: the pages together are the whole listing
    // and no entry is reported twice.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = Map::new();
        body.insert("maxResults".to_string(), json!(1));
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), json!(token));
        }
        let (status, listed) = list_workflows(&router, REGION, &Value::Object(body)).await;
        let listed = assert_ok("one entry per page", status, listed);
        let page = listed_arns("one entry per page", &listed);
        assert!(
            page.len() <= 1,
            "case \"one entry per page\": expected at most one entry, got {page:?}"
        );
        seen.extend(page);
        token = listed
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= 3,
            "case \"one entry per page\": the pages did not end, got {seen:?}"
        );
    }
    assert_eq!(
        seen,
        vec![
            workflow_version_arn(REGION, "build", "alpha", "1.0.0"),
            workflow_version_arn(REGION, "build", "beta", "1.0.0"),
            workflow_version_arn(REGION, "build", "beta", "2.0.0"),
        ],
        "case \"one entry per page\": expected the whole listing, got {seen:?}"
    );

    // A token this store did not issue is refused rather than read as a position.
    let (status, answer) = list_workflows(
        &router,
        REGION,
        &json!({ "nextToken": "not-a-real-pagination-token==" }),
    )
    .await;
    assert_refused(
        "a forged page token",
        status,
        &answer,
        400,
        "InvalidParameterValueException",
        "The value supplied for parameter 'nextToken' is not valid. The provided token does not \
         meet the required format.",
    );

    // A filter this listing does not filter on, and `byName` beside the version filter, are
    // refused rather than ignored.
    let (status, answer) = list_workflows(
        &router,
        REGION,
        &json!({ "filters": [{ "name": "platform", "values": ["Linux"] }] }),
    )
    .await;
    assert_eq!(
        error_code("a filter on an unlisted member", &answer),
        "InvalidParameterValueException",
        "case \"a filter on an unlisted member\": got {answer}"
    );
    assert_eq!(
        status, 400,
        "case \"a filter on an unlisted member\": expected HTTP 400, got {status}"
    );
    let (status, answer) = list_workflows(
        &router,
        REGION,
        &json!({ "byName": true, "filters": [{ "name": "version", "values": ["1.0.0"] }] }),
    )
    .await;
    assert_eq!(
        (status, error_code("byName beside the version filter", &answer)),
        (400, "InvalidRequestException".to_string()),
        "case \"byName beside the version filter\": got {answer}"
    );
}
