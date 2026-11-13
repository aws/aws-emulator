// SPDX-License-Identifier: CC0-1.0

//! The DEPENDENCIES an `UpdateWorkflow` configuration names, end to end over HTTP.
//!
//! RULE under test: before `UpdateWorkflow` freezes a new version, the WORKFLOW the
//! request addresses is resolved — a `WorkflowArn` that resolves to nothing is the lookup
//! miss it is, whatever configuration the request carries — and then the two dependencies
//! that version's configuration names are resolved against the services that OWN them,
//! and a request whose dependency one of them answers is unmet is refused with nothing
//! written — the workflow keeps the version history, and the latest version, it already
//! had. The `DefinitionS3Location` the version would snapshot is read out of Amazon S3,
//! and a location that store answers serves no readable object is the modelled
//! `ValidationException` (HTTP 400), which is documented for exactly this cause ("This can
//! occur … when S3 locations are inaccessible":
//! `smithy_query(query="shape", shape="ValidationException")`); the `RoleArn` the service
//! "assumes when it executes the … workflow" is resolved against this account's IAM, and a
//! role IAM answers this service cannot assume is the modelled `AccessDeniedException`
//! (HTTP 403, `smithy_query(query="operation", operation="UpdateWorkflow")`).
//!
//! Neither member declares a constraint the request boundary could judge it by
//! (`DefinitionS3Location$Bucket` and `$ObjectKey` target a bare `String`; a `RoleARN`
//! within its `@pattern` says nothing about the role it names), so every row below carries
//! a well-formed request and differs only in what the OWNING service answers about it.
//!
//! Amazon S3 and IAM are mounted beside this service on ONE engine — the shape
//! `core/router/src/lib.rs` mounts services in — every definition object is staged with
//! Amazon S3's own `PutObject` and every role created with IAM's own `CreateRole`, so a
//! dependency exists only because its owner answered for it, and the resolution under test
//! runs over the same in-process callback as in production.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::configuration_dependencies::SERVICE_PRINCIPAL;
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_definition_snapshot::{
    call, create_bucket, definition_router_on, get_workflow, stage, BUCKET, REGION,
};

/// The status an inaccessible definition location is refused with: `ValidationException`'s
/// own `@httpError`.
const DEFINITION_REFUSAL_STATUS: u16 = 400;

/// The status a role this service cannot assume is refused with: `AccessDeniedException`'s
/// own `@httpError` (`smithy_query(query="shape", shape="AccessDeniedException")`).
const ROLE_REFUSAL_STATUS: u16 = 403;

/// The sentence `AccessDeniedException` is documented with, which is the whole message:
/// "You do not have sufficient permission to perform this action."
/// (`smithy_query(query="shape", shape="AccessDeniedException")`), fixed text naming
/// nothing of the request.
const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient permission to perform this action.";

/// A service principal that is NOT this service, for the role that exists and trusts
/// someone else.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The definition object the created workflow's first version snapshots, so every row
/// starts from a workflow whose own configuration is sound.
const CREATED_KEY: &str = "dags/created.yaml";

/// A definition object an accepted update can snapshot.
const UPDATED_KEY: &str = "dags/updated.yaml";

/// The definition content the objects above hold: a DAG declaring one task, which is
/// enough for the rule under test (what a version snapshots is
/// `tests/all/workflow_definition_snapshot.rs`).
const DEFINITION: &str = "tasks:\n  - id: extract\n    operator: EmptyOperator\n";

/// This service, Amazon S3 and IAM on ONE shared engine: the mount this crate's suites
/// already share ([`definition_router_on`]), plus the IAM service the execution role is
/// resolved against.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = definition_router_on(Arc::clone(&engine));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM over the router — the protocol IAM speaks — and the body it
/// answered.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: IAM must serve {action}, got {status} with {answered}"
    );
    answered
}

/// Percent-encode one awsQuery form value, over the character set
/// `application/x-www-form-urlencoded` leaves unescaped.
fn form_encode(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the FIRST `<name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = body.find(&opening)? + opening.len();
    let end = body[start..].find(&closing)? + start;
    Some(body[start..end].to_string())
}

/// A trust policy carrying `statements`.
fn trust(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One trust statement: `effect` the `action` for the service principals `principals`.
fn statement(effect: &str, principals: Value, action: &str) -> Value {
    json!({
        "Effect": effect,
        "Principal": {"Service": principals},
        "Action": action,
    })
}

/// Create `role_name` under `path` carrying `trust_document` through IAM itself, and
/// answer the ARN IAM minted for it.
async fn create_role(
    router: &MultiServiceRouter,
    path: &str,
    role_name: &str,
    trust_document: &str,
) -> String {
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", path.to_string()),
            ("AssumeRolePolicyDocument", trust_document.to_string()),
        ],
    )
    .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: IAM answered no ARN for {role_name}: {created}"))
}

/// A role of this account whose trust policy admits this service, so an update naming it
/// is held only to its OTHER dependency.
async fn assumable_role(router: &MultiServiceRouter, role_name: &str) -> String {
    create_role(
        router,
        "/",
        role_name,
        &trust(json!([statement(
            "Allow",
            json!(SERVICE_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await
}

/// Create the workflow every row updates, with the definition object already staged, and
/// answer its ARN and the version the create minted.
async fn seed_workflow(router: &MultiServiceRouter, name: &str, role_arn: &str) -> (String, String) {
    let (status, created) = call(
        router,
        "CreateWorkflow",
        json!({
            "Name": name,
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": CREATED_KEY},
            "RoleArn": role_arn,
            "Description": "the configuration the workflow was created with",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow({name}) to succeed, got {status} {created:?}"
    );
    (
        created["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        created["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The `(WorkflowVersion, IsLatestVersion)` pairs `ListWorkflowVersions` reports for
/// `arn`, which is how a row shows what an update did or did not append.
async fn listed_versions(router: &MultiServiceRouter, arn: &str) -> Vec<(String, bool)> {
    let (status, page) = call(
        router,
        "ListWorkflowVersions",
        json!({"WorkflowArn": arn, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflowVersions({arn}) to succeed, got {status} {page:?}"
    );
    page["WorkflowVersions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            (
                summary["WorkflowVersion"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["IsLatestVersion"].as_bool().unwrap_or_default(),
            )
        })
        .collect()
}

/// `UpdateWorkflow` for `arn` with the configuration `location` and `role_arn` name.
async fn update(
    router: &MultiServiceRouter,
    arn: &str,
    location: &Value,
    role_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "UpdateWorkflow",
        json!({
            "WorkflowArn": arn,
            "DefinitionS3Location": location,
            "RoleArn": role_arn,
        }),
    )
    .await
}

/// What a row expects of the update it makes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// Every dependency is met: the version is appended.
    Appended,
    /// Amazon S3 answered that the definition location serves no readable object.
    Inaccessible,
    /// IAM answered that the execution role is not one this service can assume.
    Unassumable,
}

/// Assert that a refused update left `arn` exactly as it was: the created version is still
/// the only one, still the latest, and the workflow still reports the configuration it
/// froze.
async fn nothing_was_appended(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    created_version: &str,
) {
    let versions = listed_versions(router, arn).await;
    assert_eq!(
        versions,
        vec![(created_version.to_string(), true)],
        "case {case:?}: a refused update must append no version, got {versions:?}"
    );
    let reported = get_workflow(router, arn, None).await;
    assert_eq!(
        reported["WorkflowVersion"].as_str(),
        Some(created_version),
        "case {case:?}: the workflow must still report the version it had, got {:?}",
        reported["WorkflowVersion"]
    );
    assert_eq!(
        reported["DefinitionS3Location"]["ObjectKey"].as_str(),
        Some(CREATED_KEY),
        "case {case:?}: the workflow must still report the definition it froze, got {:?}",
        reported["DefinitionS3Location"]
    );
}

/// RULE: the definition location an update would freeze is READ before the version is
/// appended, and a location Amazon S3 answers serves no readable object is refused with
/// `ValidationException`, naming the member at fault and the address the request carried,
/// with the version history untouched.
#[tokio::test]
async fn an_update_resolves_its_definition_location_before_it_appends_a_version() {
    let router = emulator();
    create_bucket(&router).await;
    stage(&router, CREATED_KEY, DEFINITION.as_bytes()).await;
    stage(&router, UPDATED_KEY, DEFINITION.as_bytes()).await;
    stage(&router, "dags/nested/deeper/v3.yaml", DEFINITION.as_bytes()).await;
    stage(&router, "dags/empty.yaml", b"").await;
    stage(&router, "dags/binary.yaml", &[0xf0, 0x9f, 0x00, 0xff]).await;
    let role = assumable_role(&router, "mwaa-serverless-definition-rows").await;

    // Rows differ along what AMAZON S3 answers about the address: an object it serves
    // (at the top of a prefix, deeper in one, holding no bytes, holding bytes that are
    // not text — each readable, so each an update the store does not refuse), an address
    // it holds nothing at (no such key, a key one segment past an object it holds, no
    // such bucket), and an address that names no object anywhere (a blank bucket, a blank
    // key, a name Amazon S3 holds no bucket under at all).
    let rows: &[(&str, &str, &str, Verdict)] = &[
        ("an object the store serves", BUCKET, UPDATED_KEY, Verdict::Appended),
        (
            "an object under a deeper prefix",
            BUCKET,
            "dags/nested/deeper/v3.yaml",
            Verdict::Appended,
        ),
        (
            "an object holding no bytes: readable, with nothing to snapshot",
            BUCKET,
            "dags/empty.yaml",
            Verdict::Appended,
        ),
        (
            "an object holding bytes that are not text",
            BUCKET,
            "dags/binary.yaml",
            Verdict::Appended,
        ),
        (
            "no object at that key",
            BUCKET,
            "dags/absent.yaml",
            Verdict::Inaccessible,
        ),
        (
            "a key one segment past an object the store holds",
            BUCKET,
            "dags/updated.yaml/extra",
            Verdict::Inaccessible,
        ),
        (
            "no bucket of that name",
            "mwaa-serverless-unheld",
            "dags/any.yaml",
            Verdict::Inaccessible,
        ),
        ("a blank bucket", "", UPDATED_KEY, Verdict::Inaccessible),
        ("a blank key", BUCKET, "", Verdict::Inaccessible),
        (
            // `Not_A_Bucket` is no bucket name at all: Amazon S3 admits neither an
            // uppercase letter nor an underscore in one, so no read of it could serve an
            // object.
            "a value Amazon S3 holds no bucket under at all",
            "Not_A_Bucket",
            UPDATED_KEY,
            Verdict::Inaccessible,
        ),
    ];

    for (index, (case, bucket, key, verdict)) in rows.iter().enumerate() {
        // A workflow per row, so one row's appended version cannot answer another's read.
        let (arn, created_version) =
            seed_workflow(&router, &format!("definition.row{index}"), &role).await;
        let location = json!({"Bucket": bucket, "ObjectKey": key});
        let (status, body) = update(&router, &arn, &location, &role).await;

        match verdict {
            Verdict::Appended => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to be served, got {status} {body:?}"
                );
                let appended = body["WorkflowVersion"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let versions = listed_versions(&router, &arn).await;
                assert_eq!(
                    versions,
                    vec![(created_version.clone(), false), (appended.clone(), true)],
                    "case {case:?}: expected the appended version to be the only latest, got \
                     {versions:?}"
                );
                // ROUND TRIP: the workflow now reports the configuration the update froze.
                let reported = get_workflow(&router, &arn, None).await;
                assert_eq!(
                    reported["DefinitionS3Location"], location,
                    "case {case:?}: expected the new version to carry the location supplied, \
                     got {:?}",
                    reported["DefinitionS3Location"]
                );
            }
            Verdict::Inaccessible => {
                assert_eq!(
                    status, DEFINITION_REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {DEFINITION_REFUSAL_STATUS}, got \
                     {status} {body:?}"
                );
                assert!(
                    body["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("ValidationException")),
                    "case {case:?}: expected a ValidationException, got {:?}",
                    body["__type"]
                );
                assert_eq!(
                    body["Reason"].as_str(),
                    Some("fieldValidationFailed"),
                    "case {case:?}: expected the field-validation reason, got {:?}",
                    body["Reason"]
                );
                let named: Vec<&str> = body["FieldList"]
                    .as_array()
                    .map(|fields| {
                        fields
                            .iter()
                            .filter_map(|field| field["Name"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    named,
                    vec!["DefinitionS3Location"],
                    "case {case:?}: expected the member at fault to be named, got {named:?}"
                );
                let address = format!("s3://{bucket}/{key}");
                let message = body["Message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&address),
                    "case {case:?}: expected the message to name {address:?}, got {message:?}"
                );
                nothing_was_appended(&router, case, &arn, &created_version).await;
            }
            Verdict::Unassumable => unreachable!("this table does not vary the role"),
        }
    }
}

/// RULE: the execution role an update would freeze is resolved against this account's IAM
/// before the version is appended, and a role IAM answers this service cannot assume is
/// refused with `AccessDeniedException` and the documented sentence, with the version
/// history untouched. A role IAM was not asked about, or is not this account's, grounds no
/// refusal.
#[tokio::test]
async fn an_update_resolves_its_execution_role_before_it_appends_a_version() {
    let router = emulator();
    create_bucket(&router).await;
    stage(&router, CREATED_KEY, DEFINITION.as_bytes()).await;
    stage(&router, UPDATED_KEY, DEFINITION.as_bytes()).await;

    // Rows differ along what IAM answers about the value: a role of this account that
    // trusts this service (at the default path, under a path, among several principals,
    // through an action wildcard), a role it holds that withholds the assume (trusting
    // another service, denying this one explicitly), a well-formed ARN it holds no role
    // under, and a role of ANOTHER account, which this emulator's IAM is not the
    // authority on.
    let trusted = assumable_role(&router, "mwaa-serverless-trusted").await;
    let under_path = create_role(
        &router,
        "/service-role/",
        "mwaa-serverless-path",
        &trust(json!(statement(
            "Allow",
            json!(SERVICE_PRINCIPAL),
            "sts:AssumeRole"
        ))),
    )
    .await;
    let among_principals = create_role(
        &router,
        "/",
        "mwaa-serverless-shared",
        &trust(json!([statement(
            "Allow",
            json!([OTHER_PRINCIPAL, SERVICE_PRINCIPAL]),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let wildcard_action = create_role(
        &router,
        "/",
        "mwaa-serverless-wildcard",
        &trust(json!([statement(
            "Allow",
            json!(SERVICE_PRINCIPAL),
            "sts:*"
        )])),
    )
    .await;
    let other_service = create_role(
        &router,
        "/",
        "mwaa-serverless-other",
        &trust(json!([statement(
            "Allow",
            json!(OTHER_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;
    let denied = create_role(
        &router,
        "/",
        "mwaa-serverless-denied",
        &trust(json!([
            statement("Allow", json!(SERVICE_PRINCIPAL), "sts:AssumeRole"),
            statement("Deny", json!(SERVICE_PRINCIPAL), "sts:AssumeRole"),
        ])),
    )
    .await;

    let rows: Vec<(&str, String, Verdict)> = vec![
        (
            "a role of this account that trusts this service",
            trusted.clone(),
            Verdict::Appended,
        ),
        ("the same grant on a role under a path", under_path, Verdict::Appended),
        (
            "a role that trusts this service among several principals",
            among_principals,
            Verdict::Appended,
        ),
        (
            "a role that trusts this service through an action wildcard",
            wildcard_action,
            Verdict::Appended,
        ),
        (
            "a role IAM holds that trusts another service",
            other_service,
            Verdict::Unassumable,
        ),
        (
            "a role whose trust policy denies this service explicitly",
            denied,
            Verdict::Unassumable,
        ),
        (
            "a well-formed ARN this account's IAM holds no role under",
            "arn:aws:iam::000000000000:role/never-created".to_string(),
            Verdict::Unassumable,
        ),
        (
            // IAM is partition-global and this emulator holds one account's records, so
            // a role of another account is one it was not asked about: nothing it said
            // grounds a refusal.
            "a role of another account, which this IAM is not the authority on",
            "arn:aws:iam::210987654321:role/elsewhere".to_string(),
            Verdict::Appended,
        ),
    ];

    for (index, (case, role_arn, verdict)) in rows.iter().enumerate() {
        let (arn, created_version) =
            seed_workflow(&router, &format!("role.row{index}"), &trusted).await;
        let location = json!({"Bucket": BUCKET, "ObjectKey": UPDATED_KEY});
        let (status, body) = update(&router, &arn, &location, role_arn).await;

        match verdict {
            Verdict::Appended => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to be served, got {status} {body:?}"
                );
                // ROUND TRIP: the version the update appended carries that role.
                let reported = get_workflow(&router, &arn, None).await;
                assert_eq!(
                    reported["RoleArn"].as_str(),
                    Some(role_arn.as_str()),
                    "case {case:?}: expected the new version to carry the role supplied, got {:?}",
                    reported["RoleArn"]
                );
                assert_eq!(
                    reported["WorkflowVersion"].as_str(),
                    body["WorkflowVersion"].as_str(),
                    "case {case:?}: expected the workflow to report the appended version, got {:?}",
                    reported["WorkflowVersion"]
                );
            }
            Verdict::Unassumable => {
                assert_eq!(
                    status, ROLE_REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {ROLE_REFUSAL_STATUS}, got {status} \
                     {body:?}"
                );
                assert!(
                    body["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("AccessDeniedException")),
                    "case {case:?}: expected an AccessDeniedException, got {:?}",
                    body["__type"]
                );
                assert_eq!(
                    body["Message"].as_str(),
                    Some(ACCESS_DENIED_MESSAGE),
                    "case {case:?}: expected the documented sentence, got {:?}",
                    body["Message"]
                );
                nothing_was_appended(&router, case, &arn, &created_version).await;
            }
            Verdict::Inaccessible => unreachable!("this table does not vary the location"),
        }
    }
}

/// RULE, at the other end of the same rule: an emulator with NO Amazon S3 and no IAM
/// mounted answers nothing about either dependency, and an answer this emulator does not
/// have grounds no refusal — so the same update is served, with the version appended.
#[tokio::test]
async fn an_update_is_not_refused_on_an_answer_no_owning_service_gave() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mwaaserverless_amazonmwaaserverless::EmulatedService::new(engine),
    );

    let role = "arn:aws:iam::000000000000:role/mwaa-serverless-exec".to_string();
    let (arn, created_version) = seed_workflow(&router, "unanswered.dependencies", &role).await;
    let location = json!({"Bucket": "definitions", "ObjectKey": "dags/none.yaml"});
    let (status, body) = update(&router, &arn, &location, &role).await;
    assert_eq!(
        status, 200,
        "expected the update to be served where nothing answered, got {status} {body:?}"
    );
    let appended = body["WorkflowVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let versions = listed_versions(&router, &arn).await;
    assert_eq!(
        versions,
        vec![(created_version, false), (appended, true)],
        "expected the version to be appended, got {versions:?}"
    );
}

/// RULE: the WORKFLOW an update addresses is resolved BEFORE the dependencies the
/// configuration it carries names, so a `WorkflowArn` that resolves to nothing is the
/// lookup miss it is — `ResourceNotFoundException`, the entity-level message and the
/// `ResourceId`/`ResourceType` of the workflow class — whatever else the request names.
///
/// The rows differ along what the configuration would ALSO be refused for had it been
/// judged first (a definition location Amazon S3 serves nothing at, a role IAM answers
/// this service cannot assume, both at once, and a configuration with nothing wrong with
/// it) and along the form of the ARN that resolves to nothing — every form
/// [`crate::missing_resource_lookups::ghost_arns`] spans.
#[tokio::test]
async fn an_update_resolves_the_workflow_it_addresses_before_its_configuration() {
    let router = emulator();
    create_bucket(&router).await;
    stage(&router, CREATED_KEY, DEFINITION.as_bytes()).await;
    stage(&router, UPDATED_KEY, DEFINITION.as_bytes()).await;
    let assumable = assumable_role(&router, "mwaa-serverless-precedence").await;
    let unassumable = create_role(
        &router,
        "/",
        "mwaa-serverless-precedence-other",
        &trust(json!([statement(
            "Allow",
            json!(OTHER_PRINCIPAL),
            "sts:AssumeRole"
        )])),
    )
    .await;

    // The configurations a row carries beside the unresolvable workflow.
    let readable = json!({"Bucket": BUCKET, "ObjectKey": UPDATED_KEY});
    let unreadable = json!({"Bucket": BUCKET, "ObjectKey": "dags/absent.yaml"});
    let configurations: Vec<(&str, Value, &str)> = vec![
        (
            "a configuration with nothing wrong with it",
            readable.clone(),
            assumable.as_str(),
        ),
        (
            "a definition location that serves no object",
            unreadable.clone(),
            assumable.as_str(),
        ),
        (
            "a role this service cannot assume",
            readable,
            unassumable.as_str(),
        ),
        (
            "both dependencies unmet at once",
            unreadable,
            unassumable.as_str(),
        ),
    ];

    for arn in &crate::missing_resource_lookups::ghost_arns() {
        for (configuration, location, role_arn) in &configurations {
            let case = format!("{configuration} on {arn}");
            let (status, body) = update(&router, arn, location, role_arn).await;
            assert_eq!(
                status,
                crate::missing_resource_lookups::LOOKUP_MISS_STATUS,
                "case {case:?}: expected the lookup miss at {}, got {status} {body:?}",
                crate::missing_resource_lookups::LOOKUP_MISS_STATUS
            );
            assert!(
                body["__type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
                "case {case:?}: expected a ResourceNotFoundException, got {:?}",
                body["__type"]
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(crate::missing_resource_lookups::WORKFLOW_MISSING),
                "case {case:?}: expected the workflow's entity-level message, got {:?}",
                body["Message"]
            );
            assert_eq!(
                body["ResourceType"].as_str(),
                Some("Workflow"),
                "case {case:?}: expected the workflow resource type, got {:?}",
                body["ResourceType"]
            );
            let reported = body["ResourceId"].as_str().unwrap_or_default();
            assert!(
                !reported.is_empty() && arn.ends_with(reported),
                "case {case:?}: expected the workflow's own identifier, got {reported:?}"
            );
        }
    }
}
