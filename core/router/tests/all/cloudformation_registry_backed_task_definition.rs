// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack declaring a private container registry and the task definition
//! that runs an image out of it creates BOTH in the services that own them.
//!
//! The rule under test: **a stack that reaches a successful terminal state has created every
//! resource its template declares through the owning service's own create API, so each one is
//! found afterwards by that service's own read — and an identifier one declared resource
//! publishes is resolved before it is handed to the resource that consumes it.** A stack that
//! reported itself complete while the owning service holds nothing publishes identifiers that
//! resolve to nothing, so the first read a client makes against what the stack handed it is a
//! not-found that points at the caller rather than at the stack. The same rule read the other
//! way is the second test here: a create the owning service REFUSES fails the deployment
//! instead of completing it, and leaves nothing behind in either service.
//!
//! Two services own the halves of that here: Amazon ECR holds the repository
//! (`DescribeRepositories`, `GetRepositoryPolicy`) and Amazon ECS holds the revision
//! (`DescribeTaskDefinition`, addressed by the family alone and by the ARN the stack
//! published). Three crates have to agree for one of these templates to deploy —
//! CloudFormation makes the calls, Amazon ECR and Amazon ECS serve them — so the suite lives
//! here: a crate-local test can only ever see one side of the call.
//!
//! However a declaration composes the container image out of the repository, the image the
//! registered revision carries names that repository through the registry URI AWS publishes
//! for it (`AWS::ECR::Repository`'s `RepositoryUri`: "The format is
//! `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name`"), with the tag the
//! declaration asked for — which is also the reference form a pull of one of its images is
//! addressed by.
//!
//! The rows differ along what the rule generalises over: how the declaration composes the
//! image from the repository (an `Fn::Sub` over the repository's `RepositoryUri` attribute, an
//! `Fn::Join` over it, and an `Fn::Sub` over the pseudo parameters and the repository's name),
//! whether the template NAMES the repository or leaves the name to CloudFormation, whether the
//! declaration orders the two with `DependsOn` or only by referring to the repository, which
//! launch type the revision declares, the tag the image carries, whether a repository policy
//! and a scanning configuration are declared, and both template spellings (JSON and YAML).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{
    elements, form_encode, members_of, opt_element, outputs_of, physical_ids_of,
};

/// The region every request here is signed for, and so the region every resource and ARN
/// below belongs to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How a stack read waits for the deployment's clock-owed span to come due: look again, with
/// a deadline generous enough that a loaded host still reaches it.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 80;

/// The `X-Amz-Target` prefix each service's model fixes, and the signing name that routes a
/// request to it (Amazon ECR: `sigv4Name: ecr`, service shape
/// `AmazonEC2ContainerRegistry_V20150921`; Amazon ECS: `sigv4Name: ecs`, service shape
/// `AmazonEC2ContainerServiceV20141113`). Both speak awsJson1.1.
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, String)],
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
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one awsJson1_1 operation of a service addressed by its signing name and
/// `X-Amz-Target` prefix.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

async fn ecr(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "ecr", ECR_TARGET_PREFIX, operation, request).await
}

async fn ecs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "ecs", ECS_TARGET_PREFIX, operation, request).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    described
}

/// Create a stack from `template` and read it to a terminal status. The answer is that
/// read's body.
async fn create(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack.to_string()),
            ("TemplateBody", template.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    poll_to_terminal(router, case, stack).await
}

/// The stack's own events, which is where the reason a failed deployment stopped is read.
async fn stack_events(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = cloudformation(
        router,
        "DescribeStackEvents",
        &[("StackName", stack.to_string())],
    )
    .await;
    body
}

/// The reason the event of `logical_id` reaching `resource_status` carries, read from the
/// member that holds both so no event's reason is paired with another's status.
fn failure_reason(events: &str, logical_id: &str, resource_status: &str) -> Option<String> {
    members_of(events).into_iter().find_map(|member| {
        let names = elements(member, "LogicalResourceId");
        let statuses = elements(member, "ResourceStatus");
        (names.iter().any(|name| name == logical_id)
            && statuses.iter().any(|status| status == resource_status))
        .then(|| elements(member, "ResourceStatusReason").into_iter().next())
        .flatten()
    })
}

/// Create a stack from `template` and read it to a terminal status that must be
/// `CREATE_COMPLETE`. The answer is that read's body.
async fn deploy(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let described = create(router, case, stack, template).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete; reason {:?}; events {}",
        opt_element(&described, "StackStatusReason"),
        stack_events(router, stack).await,
    );
    described
}

/// The stack's logical-id -> physical-id map, which is where a client reads the identifier a
/// declared resource was created under.
async fn stack_resources(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> std::collections::BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        "DescribeStackResources",
        &[("StackName", stack.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {body}"
    );
    physical_ids_of(&body)
}

/// The error code an awsJson answer carries, without the namespace a `__type` may qualify it
/// with.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
}

/// The ARN Amazon ECS identifies the revision `revision` of `family` by.
fn task_definition_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/{family}:{revision}")
}

/// The registry URI AWS publishes for the repository `repository`, which is the reference a
/// pull of one of its images is addressed by.
///
/// Source: `AWS::ECR::Repository`'s `RepositoryUri` return value — "The format is
/// `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name`" — which is also the
/// reference form the Amazon ECR User Guide gives for a pull.
fn registry_uri(repository: &str) -> String {
    format!("{ACCOUNT_ID}.dkr.ecr.{REGION}.amazonaws.com/{repository}")
}

/// One deployment: the template, and what each owning service must then hold.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical id of the declared `AWS::ECR::Repository`.
    registry: &'static str,
    /// The logical id of the declared `AWS::ECS::TaskDefinition`.
    worker: &'static str,
    /// The repository name the template states, or `None` where CloudFormation generates it.
    repository_name: Option<&'static str>,
    /// The family the declared revision belongs to.
    family: &'static str,
    /// The tag the declared image carries.
    tag: &'static str,
    /// The `Sid` of the declared `RepositoryPolicyText`, where the template declares one.
    policy_sid: Option<&'static str>,
    /// The `ScanOnPush` the declaration asks for, where it declares a scanning configuration.
    scan_on_push: Option<bool>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named repository, its policy and scan configuration, and a Fargate revision \
               whose image is an Fn::Sub over the repository URI",
        stack: "registry-backed-worker",
        template: r#"{
          "Resources": {
            "Registry": {
              "Type": "AWS::ECR::Repository",
              "Properties": {
                "RepositoryName": "registry-backed-worker",
                "ImageScanningConfiguration": { "ScanOnPush": true },
                "RepositoryPolicyText": {
                  "Version": "2012-10-17",
                  "Statement": [{
                    "Sid": "OnlyThisAccountPulls",
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"],
                    "Condition": {
                      "StringNotEquals": {
                        "aws:PrincipalAccount": { "Ref": "AWS::AccountId" }
                      }
                    }
                  }]
                }
              }
            },
            "Worker": {
              "Type": "AWS::ECS::TaskDefinition",
              "DependsOn": "Registry",
              "Properties": {
                "Family": "registry-backed-worker",
                "Cpu": "256",
                "Memory": "512",
                "NetworkMode": "awsvpc",
                "RequiresCompatibilities": ["FARGATE"],
                "ContainerDefinitions": [{
                  "Name": "worker",
                  "Image": { "Fn::Sub": "${Registry.RepositoryUri}:latest" },
                  "Essential": true
                }]
              }
            }
          },
          "Outputs": {
            "RepositoryUri": { "Value": { "Fn::GetAtt": ["Registry", "RepositoryUri"] } },
            "RepositoryName": { "Value": { "Ref": "Registry" } },
            "TaskDefinitionArn": { "Value": { "Ref": "Worker" } }
          }
        }"#,
        registry: "Registry",
        worker: "Worker",
        repository_name: Some("registry-backed-worker"),
        family: "registry-backed-worker",
        tag: "latest",
        policy_sid: Some("OnlyThisAccountPulls"),
        scan_on_push: Some(true),
    },
    Row {
        case: "a repository CloudFormation names, ordered only by the reference, and an EC2 \
               revision whose image is an Fn::Join over the repository URI, in YAML",
        stack: "generated-registry-worker",
        template: concat!(
            "Resources:\n",
            "  Registry:\n",
            "    Type: AWS::ECR::Repository\n",
            "    Properties:\n",
            "      ImageScanningConfiguration:\n",
            "        ScanOnPush: false\n",
            "  Worker:\n",
            "    Type: AWS::ECS::TaskDefinition\n",
            "    Properties:\n",
            "      Family: generated-registry-worker\n",
            "      NetworkMode: bridge\n",
            "      ContainerDefinitions:\n",
            "        - Name: worker\n",
            "          Memory: 256\n",
            "          Essential: true\n",
            "          Image:\n",
            "            Fn::Join:\n",
            "              - \"\"\n",
            "              - - !GetAtt Registry.RepositoryUri\n",
            "                - \":2026-01\"\n",
            "Outputs:\n",
            "  RepositoryUri:\n",
            "    Value: !GetAtt Registry.RepositoryUri\n",
            "  RepositoryName:\n",
            "    Value: !Ref Registry\n",
            "  TaskDefinitionArn:\n",
            "    Value: !Ref Worker\n",
        ),
        registry: "Registry",
        worker: "Worker",
        repository_name: None,
        family: "generated-registry-worker",
        tag: "2026-01",
        policy_sid: None,
        scan_on_push: Some(false),
    },
    Row {
        case: "a revision whose image spells the published registry host from the pseudo \
               parameters and the repository name",
        stack: "registry-host-worker",
        template: r#"{
          "Resources": {
            "Registry": {
              "Type": "AWS::ECR::Repository",
              "Properties": { "RepositoryName": "registry-host-worker" }
            },
            "Worker": {
              "Type": "AWS::ECS::TaskDefinition",
              "DependsOn": "Registry",
              "Properties": {
                "Family": "registry-host-worker",
                "NetworkMode": "bridge",
                "ContainerDefinitions": [{
                  "Name": "worker",
                  "Memory": 512,
                  "Essential": true,
                  "Image": {
                    "Fn::Sub": "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/${Registry}:1.0"
                  }
                }]
              }
            }
          },
          "Outputs": {
            "RepositoryUri": { "Value": { "Fn::GetAtt": ["Registry", "RepositoryUri"] } },
            "RepositoryName": { "Value": { "Ref": "Registry" } },
            "TaskDefinitionArn": { "Value": { "Ref": "Worker" } }
          }
        }"#,
        registry: "Registry",
        worker: "Worker",
        repository_name: Some("registry-host-worker"),
        family: "registry-host-worker",
        tag: "1.0",
        policy_sid: None,
        scan_on_push: None,
    },
];

/// Every row's repository exists in Amazon ECR and every row's revision is registered in
/// Amazon ECS, carrying the image the declaration composed from that repository.
#[tokio::test]
async fn a_stacks_registry_and_its_task_definition_exist_in_the_services_that_own_them() {
    let router = build_router();
    for row in ROWS {
        let described = deploy(&router, row.case, row.stack, row.template).await;
        let outputs = outputs_of(&described);
        let resources = stack_resources(&router, row.case, row.stack).await;

        // The repository: the name the template stated, or the one CloudFormation generated
        // for the resource — which is also the physical resource id the stack reports, since
        // "Ref returns the repository name".
        let repository = outputs.get("RepositoryName").cloned().unwrap_or_else(|| {
            panic!(
                "case {:?}: no RepositoryName output with a value, got {outputs:?}",
                row.case
            )
        });
        if let Some(named) = row.repository_name {
            assert_eq!(
                repository, named,
                "case {:?}: expected the declared repository name, got {repository:?}",
                row.case
            );
        } else {
            let prefix = format!("{}-{}-", row.stack, row.registry.to_ascii_lowercase());
            assert!(
                repository.starts_with(&prefix),
                "case {:?}: expected a generated repository name starting {prefix:?}, got \
                 {repository:?}",
                row.case
            );
        }
        assert_eq!(
            resources.get(row.registry).map(String::as_str),
            Some(repository.as_str()),
            "case {:?}: expected the stack resource to report the repository it created, got \
             {resources:?}",
            row.case
        );

        // Amazon ECR's own read: the repository the stack says it created is there, under the
        // identifier the stack published, with the settings the declaration asked for.
        let (status, reported) = ecr(
            &router,
            "DescribeRepositories",
            &json!({ "repositoryNames": [repository] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ecr DescribeRepositories on {repository:?} to answer 200, got \
             {status} with body {reported}",
            row.case
        );
        let held = &reported["repositories"][0];
        assert_eq!(
            (
                held["repositoryName"].as_str(),
                held["repositoryArn"].as_str()
            ),
            (
                Some(repository.as_str()),
                Some(
                    format!("arn:aws:ecr:{REGION}:{ACCOUNT_ID}:repository/{repository}").as_str()
                )
            ),
            "case {:?}: expected Amazon ECR to hold the stack's repository, got {reported}",
            row.case
        );
        if let Some(scan_on_push) = row.scan_on_push {
            assert_eq!(
                held["imageScanningConfiguration"]["scanOnPush"].as_bool(),
                Some(scan_on_push),
                "case {:?}: expected scanOnPush == {scan_on_push}, got {reported}",
                row.case
            );
        }

        // The URI Amazon ECR published for the repository, which is an ADDRESS a container
        // client dials: whatever endpoint this registry announces it at, it ends in the
        // repository's own name, so a client composes a tag or a digest on what it was
        // handed. The stack publishes one for the repository it created too. Asserted by that
        // FORM rather than against the documented
        // `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name` value because the host
        // this registry announces is the open
        // `20261111T021229Z-ecr-describerepositories-reports-a-repositoryuri-whose-host-is`;
        // tighten this to `registry_uri(&repository)` once that issue is fixed.
        let published_uri = held["repositoryUri"].as_str().unwrap_or_default().to_string();
        let suffix = format!("/{repository}");
        for (reported_by, uri) in [
            ("Amazon ECR", published_uri.clone()),
            (
                "the stack's RepositoryUri output",
                outputs.get("RepositoryUri").cloned().unwrap_or_default(),
            ),
        ] {
            assert!(
                uri.ends_with(&suffix) && uri.len() > suffix.len(),
                "case {:?}: expected the repository URI {reported_by} reports to be a registry \
                 endpoint followed by {suffix:?}, got {uri:?}",
                row.case
            );
        }

        // The repository policy the declaration carried: set on the repository the create
        // just made, with the intrinsics inside the document resolved.
        if let Some(sid) = row.policy_sid {
            let (status, reported) = ecr(
                &router,
                "GetRepositoryPolicy",
                &json!({ "repositoryName": repository }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected ecr GetRepositoryPolicy on {repository:?} to answer 200, \
                 got {status} with body {reported}",
                row.case
            );
            let document: Value = reported["policyText"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            assert_eq!(
                (
                    document["Statement"][0]["Sid"].as_str(),
                    document["Statement"][0]["Effect"].as_str(),
                    document["Statement"][0]["Condition"]["StringNotEquals"]
                        ["aws:PrincipalAccount"]
                        .as_str(),
                ),
                (Some(sid), Some("Deny"), Some(ACCOUNT_ID)),
                "case {:?}: expected the declared policy with its intrinsics resolved, got \
                 {reported}",
                row.case
            );
        }

        // The revision: registered in Amazon ECS under the declared family, at the revision
        // that service allocated, carrying the image the declaration composed.
        let expected_arn = task_definition_arn(row.family, 1);
        let published_arn = outputs.get("TaskDefinitionArn").cloned().unwrap_or_default();
        assert_eq!(
            published_arn, expected_arn,
            "case {:?}: expected the stack to publish {expected_arn:?}, got {published_arn:?}",
            row.case
        );
        assert_eq!(
            resources.get(row.worker).map(String::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected the stack resource to report the revision it registered, got \
             {resources:?}",
            row.case
        );
        // However the declaration composed it, the image the revision carries names this
        // repository through the registry URI AWS publishes for it, with the declared tag.
        let expected_image = format!("{}:{}", registry_uri(&repository), row.tag);
        // Addressed by the FAMILY alone, which is "the latest ACTIVE revision in that
        // family" — the read that answered `ClientException` while nothing was registered.
        for (addressed, handle) in [
            ("the family", row.family.to_string()),
            ("the ARN the stack published", published_arn.clone()),
        ] {
            let (status, reported) = ecs(
                &router,
                "DescribeTaskDefinition",
                &json!({ "taskDefinition": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?} addressed by {addressed}: expected ecs DescribeTaskDefinition on \
                 {handle:?} to answer 200, got {status} with body {reported}",
                row.case
            );
            let definition = &reported["taskDefinition"];
            assert_eq!(
                (
                    definition["taskDefinitionArn"].as_str(),
                    definition["family"].as_str(),
                    definition["revision"].as_i64(),
                    definition["status"].as_str(),
                ),
                (
                    Some(expected_arn.as_str()),
                    Some(row.family),
                    Some(1),
                    Some("ACTIVE")
                ),
                "case {:?} addressed by {addressed}: expected the registered revision, got \
                 {reported}",
                row.case
            );
            assert_eq!(
                definition["containerDefinitions"][0]["image"].as_str(),
                Some(expected_image.as_str()),
                "case {:?} addressed by {addressed}: expected the image the declaration \
                 composed, got {reported}",
                row.case
            );
        }
    }
}

/// The family of the revision Amazon ECS refuses below, and the stack that declares it.
const REFUSED_FAMILY: &str = "refused-registry-worker";
const REFUSED_STACK: &str = "refused-registry-worker-stack";

/// The same two resources, with a task definition Amazon ECS will not register: its one
/// container reserves no memory and the task reserves none either, which that service refuses
/// ("at least one of 'memory' or 'memoryReservation' must be specified").
fn refused_template() -> String {
    json!({
        "Resources": {
            "Registry": {
                "Type": "AWS::ECR::Repository",
                "Properties": { "RepositoryName": REFUSED_FAMILY }
            },
            "Worker": {
                "Type": "AWS::ECS::TaskDefinition",
                "DependsOn": "Registry",
                "Properties": {
                    "Family": REFUSED_FAMILY,
                    "NetworkMode": "bridge",
                    "ContainerDefinitions": [{
                        "Name": "worker",
                        "Essential": true,
                        "Image": { "Fn::Sub": "${Registry.RepositoryUri}:latest" }
                    }]
                }
            }
        }
    })
    .to_string()
}

/// A registration Amazon ECS REFUSES fails the deployment instead of completing it, and
/// neither resource is left behind in the service that owns it.
#[tokio::test]
async fn a_registration_ecs_refuses_fails_the_stack_and_registers_nothing() {
    let router = build_router();
    let case = "a container reserving no memory, which Amazon ECS refuses";
    let described = create(&router, case, REFUSED_STACK, &refused_template()).await;
    let events = stack_events(&router, REFUSED_STACK).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {:?}; events {events}",
        opt_element(&described, "StackStatus"),
    );
    let reason = failure_reason(&events, "Worker", "CREATE_FAILED").unwrap_or_else(|| {
        panic!("case {case:?}: expected a CREATE_FAILED event for the task definition, got {events}")
    });
    // Amazon ECS's own refusal of a container that reserves nothing, as that service words it
    // (`RegisterTaskDefinition`: "Invalid setting for container '<name>': at least one of
    // 'memory' or 'memoryReservation' must be specified").
    assert!(
        reason.contains("at least one of 'memory' or 'memoryReservation' must be specified"),
        "case {case:?}: expected Amazon ECS's own refusal on the failed resource's event, got \
         {reason:?}"
    );

    // Nothing the deployment could not carry through is left behind: Amazon ECS holds no
    // revision of the family, and the repository the rollback deleted is gone from Amazon ECR.
    // Each absence is asserted as the service's own not-found answer rather than as any 400,
    // so a validation refusal of the read itself could not stand in for it.
    let (status, reported) = ecs(
        &router,
        "DescribeTaskDefinition",
        &json!({ "taskDefinition": REFUSED_FAMILY }),
    )
    .await;
    assert_eq!(
        (status, error_code(&reported)),
        (400, "ClientException"),
        "case {case:?}: expected ecs DescribeTaskDefinition on the refused family to answer \
         400 ClientException, got {status} with body {reported}"
    );
    let (status, reported) = ecr(
        &router,
        "DescribeRepositories",
        &json!({ "repositoryNames": [REFUSED_FAMILY] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&reported)),
        (400, "RepositoryNotFoundException"),
        "case {case:?}: expected ecr DescribeRepositories to hold no repository for the rolled \
         back stack, got {status} with body {reported}"
    );
}
