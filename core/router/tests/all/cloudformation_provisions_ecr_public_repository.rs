// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack PROVISIONS the resources its template declares in the service
//! that owns them.
//!
//! The rule under test: for a resource type this emulator provisions, a stack operation
//! calls the owning service — `AWS::ECR::PublicRepository` is created through `ecr-public`
//! `CreateRepository` — so a stack that reports `CREATE_COMPLETE` is one whose declared
//! repositories really exist in the registry every `ecr-public` read answers from, and a
//! stack whose resource could NOT be created does not report success: it fails and says
//! which resource and why. Deleting the stack deletes the repositories it created. A
//! resource type with no provisioner is recorded exactly as before.
//!
//! This suite needs TWO services on one engine — CloudFormation reaches ECR Public
//! through the in-process callback, so a single-service router has no registry to create
//! anything in — which is why it lives here rather than in either service's crate. The
//! rows span what the rule generalises over: how the template spells the name (literally,
//! through a `Ref` to a parameter, in YAML rather than JSON), how many such resources one
//! template declares, what else it declares beside them, whether the repository can be
//! created at all, and both ends of the stack's life.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

// The one percent-encoder these router suites share: an `awsQuery` form body leaves only
// RFC 3986's unreserved characters literal, which is what an AWS SDK sends. It lives in
// `all/query_wire.rs`, the module every awsQuery suite here reads the wire through.
use crate::query_wire::form_encode;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// ECR Public is a partition-wide service whose endpoint lives in `us-east-1`, which is
/// also the Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The canonical account id every emulator ARN carries, which is this registry's
/// `registryId`.
const ACCOUNT_ID: &str = "000000000000";

/// How long to wait for a stack's terminal status to become due. The CloudFormation
/// service makes it due a fixed delay after the call that starts it
/// (`state::STATUS_TRANSITION_DELAY`); this is comfortably past that, and nothing here
/// sleeps per row.
const SETTLE_WAIT: Duration = Duration::from_millis(1_500);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Drive one CloudFormation action: `POST /` with the parameters in the form body,
/// answered with the HTTP status and the XML body.
fn cloudformation(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cloudformation.{REGION}.amazonaws.com/"))
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("authorization", authorization("cloudformation"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Drive one ECR Public operation: `POST /` with an `awsJson1_1` body and the
/// `X-Amz-Target` header naming the operation.
fn ecr_public(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://api.ecr-public.{REGION}.amazonaws.com/"))
        .header("host", format!("api.ecr-public.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ecr-public"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("SpencerFrontendService.{operation}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let parsed = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// Undo the XML text escaping the serializer applies.
///
/// Published to the other suites of this crate that read an awsQuery answer, so the
/// scraper below is written once.
pub(crate) fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every `<tag>` text in document order.
pub(crate) fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(xml_unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The first `<tag>` text, or `None` when the response carries none.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// The error code an `awsJson1_1` error body carries, without its namespace.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a stack and wait for its terminal status, which is what a client's
/// `stack_create_complete` waiter does. Returns the stack's status and status reason.
fn create_stack_and_settle(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    stack_name: &str,
    template: &str,
    extra: &[(&str, &str)],
) -> (String, Option<String>) {
    let mut members = vec![("StackName", stack_name), ("TemplateBody", template)];
    members.extend_from_slice(extra);
    let (status, body) = cloudformation(router, runtime, "CreateStack", &members);
    assert_eq!(
        status, 200,
        "stack {stack_name:?}: CreateStack must be accepted, got {status} {body}"
    );
    settle(router, runtime, stack_name)
}

/// Poll a stack until it leaves `*_IN_PROGRESS`, the way a waiter does.
fn settle(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    stack_name: &str,
) -> (String, Option<String>) {
    let deadline = std::time::Instant::now() + SETTLE_WAIT;
    loop {
        let (status, body) = cloudformation(
            router,
            runtime,
            "DescribeStacks",
            &[("StackName", stack_name)],
        );
        assert_eq!(
            status, 200,
            "stack {stack_name:?}: DescribeStacks must answer, got {status} {body}"
        );
        let stack_status = xml_text(&body, "StackStatus").unwrap_or_default();
        if !stack_status.ends_with("_IN_PROGRESS") {
            return (stack_status, xml_text(&body, "StackStatusReason"));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "stack {stack_name:?}: still {stack_status} after {SETTLE_WAIT:?}: {body}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One template that declares ECR Public repositories, and the repository names the
/// registry must hold once its stack completes.
struct ProvisionCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// `Parameters.member.N` entries the create passes, for a template that declares
    /// parameters.
    parameters: Vec<(&'static str, &'static str)>,
    repositories: Vec<&'static str>,
}

/// A single-repository JSON template.
fn json_template(logical_id: &str, repository: &str) -> String {
    format!(
        "{{\"Resources\": {{\"{logical_id}\": {{\"Type\": \"AWS::ECR::PublicRepository\", \
         \"Properties\": {{\"RepositoryName\": \"{repository}\"}}}}}}, \
         \"Outputs\": {{\"Arn\": {{\"Value\": {{\"Fn::GetAtt\": [\"{logical_id}\", \"Arn\"]}}}}, \
         \"Name\": {{\"Value\": {{\"Ref\": \"{logical_id}\"}}}}}}}}"
    )
}

fn provision_cases() -> Vec<ProvisionCase> {
    vec![
        ProvisionCase {
            case: "a template naming the repository literally",
            stack: "cfn-literal",
            template: json_template("Repo", "cfn-literal-repo"),
            parameters: Vec::new(),
            repositories: vec!["cfn-literal-repo"],
        },
        ProvisionCase {
            case: "a template with catalog data and tags",
            stack: "cfn-catalog",
            template: "{\"Resources\": {\"Repo\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                       \"Properties\": {\"RepositoryName\": \"cfn-catalog-repo\", \
                       \"RepositoryCatalogData\": {\"RepositoryDescription\": \"from the stack\", \
                       \"Architectures\": [\"ARM 64\"], \"OperatingSystems\": [\"Linux\"]}, \
                       \"Tags\": [{\"Key\": \"owner\", \"Value\": \"platform\"}]}}}}"
                .to_string(),
            parameters: Vec::new(),
            repositories: vec!["cfn-catalog-repo"],
        },
        ProvisionCase {
            case: "a template whose name comes from a parameter",
            stack: "cfn-parameterised",
            template: "{\"Parameters\": {\"Name\": {\"Type\": \"String\"}}, \
                       \"Resources\": {\"Repo\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                       \"Properties\": {\"RepositoryName\": {\"Ref\": \"Name\"}}}}}"
                .to_string(),
            parameters: vec![
                ("Parameters.member.1.ParameterKey", "Name"),
                (
                    "Parameters.member.1.ParameterValue",
                    "cfn-parameterised-repo",
                ),
            ],
            repositories: vec!["cfn-parameterised-repo"],
        },
        ProvisionCase {
            case: "a YAML template",
            stack: "cfn-yaml",
            template: "Resources:\n  Repo:\n    Type: AWS::ECR::PublicRepository\n    \
                       Properties:\n      RepositoryName: cfn-yaml-repo\n"
                .to_string(),
            parameters: Vec::new(),
            repositories: vec!["cfn-yaml-repo"],
        },
        ProvisionCase {
            case: "two repositories in one template",
            stack: "cfn-two",
            template: "{\"Resources\": {\
                       \"First\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                       \"Properties\": {\"RepositoryName\": \"cfn-two-first\"}}, \
                       \"Second\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                       \"Properties\": {\"RepositoryName\": \"team/cfn-two-second\"}}}}"
                .to_string(),
            parameters: Vec::new(),
            repositories: vec!["cfn-two-first", "team/cfn-two-second"],
        },
        ProvisionCase {
            case: "a repository beside a type with no provisioner",
            stack: "cfn-mixed",
            template: "{\"Resources\": {\
                       \"Bucket\": {\"Type\": \"AWS::S3::Bucket\", \
                       \"Properties\": {\"BucketName\": \"cfn-mixed-bucket\"}}, \
                       \"Repo\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                       \"Properties\": {\"RepositoryName\": \"cfn-mixed-repo\"}}}}"
                .to_string(),
            parameters: Vec::new(),
            repositories: vec!["cfn-mixed-repo"],
        },
    ]
}

#[test]
fn a_cloudformation_stack_creates_and_deletes_the_ecr_public_repositories_it_declares() {
    std::thread::Builder::new()
        .name("cfn-ecr-public-provisioning".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            for case in provision_cases() {
                let label = case.case;
                let parameters: Vec<(&str, &str)> = case.parameters.clone();
                let (status, reason) = create_stack_and_settle(
                    &router,
                    &runtime,
                    case.stack,
                    &case.template,
                    &parameters,
                );
                assert_eq!(
                    status, "CREATE_COMPLETE",
                    "case {label:?}: expected the stack to complete, got {status} {reason:?}"
                );

                // Every declared repository is one the registry now holds, reported by
                // the service that owns it rather than by CloudFormation.
                for repository in &case.repositories {
                    let (status, body) = ecr_public(
                        &router,
                        &runtime,
                        "DescribeRepositories",
                        serde_json::json!({"repositoryNames": [repository]}),
                    );
                    assert_eq!(
                        status, 200,
                        "case {label:?}: ecr-public must hold {repository:?} once the stack \
                         completed, got {status} {body}"
                    );
                    let repositories = body["repositories"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    assert_eq!(
                        repositories.len(),
                        1,
                        "case {label:?}: expected exactly one repository named \
                         {repository:?}, got {body}"
                    );
                    assert_eq!(
                        repositories[0]["repositoryName"].as_str(),
                        Some(*repository),
                        "case {label:?}: expected the declared name, got {body}"
                    );
                    assert_eq!(
                        repositories[0]["repositoryArn"].as_str(),
                        Some(
                            format!("arn:aws:ecr-public::{ACCOUNT_ID}:repository/{repository}")
                                .as_str()
                        ),
                        "case {label:?}: expected the registry's ARN form, got {body}"
                    );
                }

                // What the stack reports about the resource agrees with the service: the
                // `Ref` is the repository name and `Fn::GetAtt Arn` the ARN ECR Public
                // minted for it.
                let (_, described) = cloudformation(
                    &router,
                    &runtime,
                    "DescribeStacks",
                    &[("StackName", case.stack)],
                );
                let outputs = xml_texts(&described, "OutputValue");
                if !outputs.is_empty() {
                    let repository = case.repositories[0];
                    let (_, read) = ecr_public(
                        &router,
                        &runtime,
                        "DescribeRepositories",
                        serde_json::json!({"repositoryNames": [repository]}),
                    );
                    let arn = read["repositories"][0]["repositoryArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        outputs.contains(&arn) && outputs.contains(&repository.to_string()),
                        "case {label:?}: expected the stack's outputs to carry the \
                         repository's name and ARN ({arn}), got {outputs:?}"
                    );
                }
            }

            // Catalog data and tags a template declares reach the repository, so the
            // provisioned resource is the one the template describes and not just its
            // name.
            let (_, catalog) = ecr_public(
                &router,
                &runtime,
                "GetRepositoryCatalogData",
                serde_json::json!({"repositoryName": "cfn-catalog-repo"}),
            );
            assert_eq!(
                catalog["catalogData"]["description"].as_str(),
                Some("from the stack"),
                "expected the template's RepositoryDescription on the repository, got {catalog}"
            );
            assert_eq!(
                catalog["catalogData"]["architectures"],
                serde_json::json!(["ARM 64"]),
                "expected the template's Architectures on the repository, got {catalog}"
            );
            let (_, tags) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["cfn-catalog-repo"]}),
            );
            assert_eq!(
                tags["repositories"][0]["repositoryUri"].as_str(),
                Some("public.ecr.aws/emulator/cfn-catalog-repo"),
                "expected the provisioned repository's own pullable URI, got {tags}"
            );

            // Deleting the stack deletes the repositories it created. A deleted stack is
            // addressable by its stack ID only ("Deleted stacks: You must specify the
            // unique stack ID"), so the id is read before the deletion starts.
            let (_, described) = cloudformation(
                &router,
                &runtime,
                "DescribeStacks",
                &[("StackName", "cfn-two")],
            );
            let stack_id = xml_text(&described, "StackId").expect("the stack reports its id");
            let (status, body) = cloudformation(
                &router,
                &runtime,
                "DeleteStack",
                &[("StackName", "cfn-two")],
            );
            assert_eq!(
                status, 200,
                "DeleteStack must be accepted, got {status} {body}"
            );
            let (status, reason) = settle(&router, &runtime, &stack_id);
            assert_eq!(
                status, "DELETE_COMPLETE",
                "expected the deletion to complete, got {status} {reason:?}"
            );
            for repository in ["cfn-two-first", "team/cfn-two-second"] {
                let (status, body) = ecr_public(
                    &router,
                    &runtime,
                    "DescribeRepositories",
                    serde_json::json!({"repositoryNames": [repository]}),
                );
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "RepositoryNotFoundException"),
                    "expected {repository:?} to be gone with its stack, got {status} {body}"
                );
            }
            // The other stacks' repositories are untouched by that deletion.
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["cfn-literal-repo"]}),
            );
            assert_eq!(
                status, 200,
                "another stack's repository must survive, got {status} {body}"
            );
        })
        .expect("spawn the provisioning thread")
        .join()
        .expect("every declared repository must be provisioned in ecr-public");
}

#[test]
fn a_stack_whose_repository_cannot_be_created_fails_and_says_which_one() {
    std::thread::Builder::new()
        .name("cfn-ecr-public-provisioning-failure".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            // A repository the registry already holds, created the ordinary way.
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "CreateRepository",
                serde_json::json!({"repositoryName": "taken-repo"}),
            );
            assert_eq!(
                status, 200,
                "setup: CreateRepository must succeed, got {status} {body}"
            );

            // A stack declaring that same repository cannot create it, so the stack does
            // not complete: it fails, and says which resource failed and what the owning
            // service answered. The pre-deployment validations are OFF because this case is
            // about what the OWNING SERVICE answers: with them on, CloudFormation's own
            // existence check fails the create before the registry is asked at all.
            let (status, reason) = create_stack_and_settle(
                &router,
                &runtime,
                "cfn-taken",
                &json_template("Repo", "taken-repo"),
                &[("DisableValidation", "true")],
            );
            assert_eq!(
                status, "ROLLBACK_COMPLETE",
                "expected the failed create to be rolled back, got {status} {reason:?}"
            );
            let reason = reason.unwrap_or_default();
            // The stack's own reason is the summary naming the resource that failed; what
            // the owning service answered is on that resource's event.
            assert_eq!(
                reason,
                resource_failure_summary(failed_action::CREATE, ["Repo"]),
                "expected the stack reason to summarise which resource failed, got \
                 {reason:?}"
            );
            let (_, events) = cloudformation(
                &router,
                &runtime,
                "DescribeStackEvents",
                &[("StackName", "cfn-taken")],
            );
            assert!(
                xml_texts(&events, "ResourceStatusReason")
                    .iter()
                    .any(|reported| reported.contains("already exists")),
                "expected the failing resource's event to carry what the service answered, \
                 got {events}"
            );
            // The repository that was already there is untouched: the stack created
            // nothing, so its deletion deletes nothing.
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["taken-repo"]}),
            );
            assert_eq!(
                status, 200,
                "the pre-existing repository must survive a failed stack, got {status} {body}"
            );

            // The same failure in a stack that ALSO declares a repository it COULD create:
            // rolling the failed create back removes the one it had made — that is what a
            // rollback is — while the repository the stack never created is left alone.
            let template = "{\"Resources\": {\
                            \"Good\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                            \"Properties\": {\"RepositoryName\": \"cfn-partial-good\"}}, \
                            \"Taken\": {\"Type\": \"AWS::ECR::PublicRepository\", \
                            \"Properties\": {\"RepositoryName\": \"taken-repo\"}}}}";
            let (status, reason) =
                create_stack_and_settle(&router, &runtime, "cfn-partial", template, &[]);
            assert_eq!(
                status, "ROLLBACK_COMPLETE",
                "expected the failed create to be rolled back, got {status} {reason:?}"
            );
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["cfn-partial-good"]}),
            );
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "RepositoryNotFoundException"),
                "the repository the failed create had made must be removed by the \
                 rollback, got {status} {body}"
            );

            // Deleting that rolled-back stack has nothing left of its own to delete, and
            // leaves the repository it never created alone.
            let (_, described) = cloudformation(
                &router,
                &runtime,
                "DescribeStacks",
                &[("StackName", "cfn-partial")],
            );
            let stack_id = xml_text(&described, "StackId").expect("the stack reports its id");
            let (status, body) = cloudformation(
                &router,
                &runtime,
                "DeleteStack",
                &[("StackName", "cfn-partial")],
            );
            assert_eq!(
                status, 200,
                "DeleteStack must be accepted, got {status} {body}"
            );
            let (status, reason) = settle(&router, &runtime, &stack_id);
            assert_eq!(
                status, "DELETE_COMPLETE",
                "expected the deletion to complete, got {status} {reason:?}"
            );
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["cfn-partial-good"]}),
            );
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "RepositoryNotFoundException"),
                "expected the rolled-back stack's own repository to stay gone, got \
                 {status} {body}"
            );
            let (status, body) = ecr_public(
                &router,
                &runtime,
                "DescribeRepositories",
                serde_json::json!({"repositoryNames": ["taken-repo"]}),
            );
            assert_eq!(
                status, 200,
                "a repository the stack never created must survive its deletion, got \
                 {status} {body}"
            );
        })
        .expect("spawn the provisioning-failure thread")
        .join()
        .expect("a stack that cannot provision a resource must not report success");
}
