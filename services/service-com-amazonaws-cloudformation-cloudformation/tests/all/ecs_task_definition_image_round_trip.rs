// SPDX-License-Identifier: CC0-1.0

//! The container IMAGE a stack declares, as Amazon ECS holds it afterwards.
//!
//! RULE under test: **a property value a declaration states reaches the service that owns the
//! resource as the declaration RESOLVES — an `Fn::Sub` substituting the pseudo-parameters and
//! the template parameters it names and leaving every other character of the string as
//! written — and that service echoes it back unchanged.** `ContainerDefinition$image` is "the
//! image used to start a container… `<repository-url>/<image>:<tag>`"
//! (`smithy_query(query="doc",shape="ContainerDefinition",member="image")`): Amazon ECS stores
//! the reference it was registered with and `DescribeTaskDefinition` reports that reference, so
//! a registry authority a template wrote out — this account's registry host, another account's,
//! or a public registry — is what a client reads back, and only a declaration that ASKS for an
//! address this emulator announces (`Fn::GetAtt Repo.RepositoryUri`) resolves to one.
//!
//! Why it matters: a client reads the registered image to decide something — that the revision
//! points at this account's registry, which account and Region the reference names, whether the
//! deployed revision still matches the template that produced it — and re-registers a revision
//! from what a describe answered. An authority nothing in the template asked for makes every one
//! of those readings wrong, and an authority taken from the socket this process happens to be
//! served on is not even reproducible between runs.
//!
//! The rows differ along what the rule generalises over: WHAT the declared string is (this
//! account's ECR registry host, another account's, a public registry), which part of it an
//! intrinsic computes (both pseudo-parameters, one of them, none at all, template parameters
//! beside them), how the image is qualified (a tag, a digest) and how deep the repository path is
//! (one segment, a namespaced one). One row per container of a single revision, so the whole
//! table is asserted over one deployment, and that revision's declaration READS no repository —
//! which is what makes its values its own. The other side of the rule is a second revision of the
//! same stack whose image is built on `Repo.RepositoryUri`: that declaration does read the
//! repository, so what it reaches Amazon ECS as is the address Amazon ECR announced. The template
//! spelling is spanned by a third case that declares the same kind of reference in YAML.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_and_ecs_service_resources::{ecs, json_call};
use crate::harness::{
    create_stack, member, physical_resource_id, router, settled_stack_status, stack_event_reasons,
    REGION,
};

/// The account every resource of these stacks is created in, which is what `${AWS::AccountId}`
/// resolves to, and the Region every call here is signed for, which is what `${AWS::Region}`
/// resolves to ([`REGION`]).
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// Another account's id, for the row whose declared authority names a registry this emulator
/// does not hold the repository of: a cross-account reference is a value of the template's own
/// and is stored exactly as written.
const FOREIGN_ACCOUNT: &str = "210987654321";

/// Amazon ECR's awsJson version and the service shape its `X-Amz-Target` names an operation
/// under (`sigv4Name: ecr`, service shape `AmazonEC2ContainerRegistry_V20150921`, `awsJson1_1`).
const ECR_JSON: &str = "application/x-amz-json-1.1";
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The status a stack that deployed settles in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The repositories the table's stack declares: the one every row that writes this account's
/// registry host names, and a NAMESPACED one, because a repository name "can only contain
/// lowercase letters, numbers, hyphens, underscores, and forward slashes"
/// (`smithy_query(query="doc",shape="CreateRepositoryRequest",member="repositoryName")`) — so the
/// path after the authority is more than one segment for such a repository.
const REPOSITORY: &str = "image-round-trip-repo";
const NAMESPACED_REPOSITORY: &str = "image-round-trip/nested-repo";

/// The task execution role the revision names: Amazon ECS refuses a Fargate registration
/// whose container pulls from Amazon ECR without one ("Fargate requires task definition to
/// have execution role ARN to support ECR images"), so every row's revision states it. It is
/// no part of the rule under test — the image is — and reaches Amazon ECS as the ARN of a role
/// of this account.
const EXECUTION_ROLE_NAME: &str = "ecs-image-round-trip";

/// That role's ARN, as IAM renders one (`arn:aws:iam::<account>:role/<name>`).
fn execution_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{EXECUTION_ROLE_NAME}")
}

/// The digest form of a reference, as a container image may be qualified instead of by a tag:
/// `@sha256:<64 hex digits>`.
const DIGEST: &str = "@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The values the table's stack supplies for the parameters its `Fn::Sub` rows name.
const SUPPLIED_REPOSITORY: &str = REPOSITORY;
const SUPPLIED_TAG: &str = "2026-01";

/// The logical ids of the two revisions the table's stack declares: the one whose containers
/// are the rows, whose declaration reads no repository of the stack, and the one whose single
/// container's image is built on the address the stack asked Amazon ECR for.
const LITERAL_REVISION: &str = "TaskDef";
const REFERENCING_REVISION: &str = "ReferencingTaskDef";

/// The tag the referencing revision's declaration appends to the `RepositoryUri` it reads.
const REFERENCED_TAG: &str = ":latest";

/// One row: the container it declares, the `Image` value the template states for it, and what
/// Amazon ECS must report for that container afterwards.
struct Row {
    case: &'static str,
    container: &'static str,
    /// The `Image` property as the template states it — a literal, or an intrinsic document.
    declared: Value,
    /// The declaration RESOLVED: the string with the pseudo-parameters and the template
    /// parameters it names substituted and nothing else changed.
    expected: String,
    /// Whether the resolved reference is written on an Amazon ECR registry host, whose
    /// authority a client reads the owning account and the Region out of: its first four labels
    /// are the account id, the two fixed labels and the Region
    /// (`AuthorizationData$proxyEndpoint`: the registry URL format
    /// `https://aws_account_id.dkr.ecr.region.amazonaws.com`), followed by the zone the
    /// registry is served in.
    registry_form: bool,
}

/// This account's ECR registry host as a template writes it out of the pseudo-parameters, with
/// `path` as the reference after the authority.
fn declared_registry_reference(path: &str) -> String {
    format!("${{AWS::AccountId}}.dkr.ecr.${{AWS::Region}}.amazonaws.com/{path}")
}

/// The same reference once the two pseudo-parameters have resolved, which is what the stack's
/// own account and Region make of it.
fn resolved_registry_reference(path: &str) -> String {
    format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/{path}")
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both pseudo-parameters, this account's registry, a tag",
            container: "own-tagged",
            declared: json!({"Fn::Sub": declared_registry_reference(&format!("{REPOSITORY}:latest"))}),
            expected: resolved_registry_reference(&format!("{REPOSITORY}:latest")),
            registry_form: true,
        },
        Row {
            case: "both pseudo-parameters, this account's registry, a digest",
            container: "own-digest",
            declared: json!({"Fn::Sub": declared_registry_reference(&format!("{REPOSITORY}{DIGEST}"))}),
            expected: resolved_registry_reference(&format!("{REPOSITORY}{DIGEST}")),
            registry_form: true,
        },
        Row {
            case: "a namespaced repository path",
            container: "own-namespaced",
            declared: json!({
                "Fn::Sub": declared_registry_reference(&format!("{NAMESPACED_REPOSITORY}:1.4"))
            }),
            expected: resolved_registry_reference(&format!("{NAMESPACED_REPOSITORY}:1.4")),
            registry_form: true,
        },
        Row {
            case: "one pseudo-parameter, another account's registry",
            container: "foreign",
            declared: json!({
                "Fn::Sub": format!("{FOREIGN_ACCOUNT}.dkr.ecr.${{AWS::Region}}.amazonaws.com/app:1")
            }),
            expected: format!("{FOREIGN_ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/app:1"),
            registry_form: true,
        },
        Row {
            case: "no intrinsic at all, a public registry",
            container: "public",
            declared: json!("public.ecr.aws/docker/library/busybox:latest"),
            expected: "public.ecr.aws/docker/library/busybox:latest".to_string(),
            registry_form: false,
        },
        Row {
            case: "template parameters beside the pseudo-parameters",
            container: "parameterised",
            declared: json!({
                "Fn::Sub": declared_registry_reference("${RepositoryName}:${ImageTag}")
            }),
            expected: resolved_registry_reference(&format!(
                "{SUPPLIED_REPOSITORY}:{SUPPLIED_TAG}"
            )),
            registry_form: true,
        },
    ]
}

/// The template the table deploys: the two repositories its rows name, the revision whose
/// containers are the rows — a declaration that reads neither repository, so every image it
/// states is a value of its own — and a second revision whose image is built on the address
/// the stack asked Amazon ECR for.
fn table_template(rows: &[Row]) -> String {
    let containers: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "Name": row.container,
                "Image": row.declared.clone(),
                "Essential": false,
            })
        })
        .collect();
    json!({
        "Parameters": {
            "RepositoryName": {"Type": "String"},
            "ImageTag": {"Type": "String"},
        },
        "Resources": {
            "Repo": {
                "Type": "AWS::ECR::Repository",
                "Properties": {"RepositoryName": REPOSITORY},
            },
            "Namespaced": {
                "Type": "AWS::ECR::Repository",
                "Properties": {"RepositoryName": NAMESPACED_REPOSITORY},
            },
            LITERAL_REVISION: {
                "Type": "AWS::ECS::TaskDefinition",
                "Properties": {
                    "Family": "image-round-trip-family",
                    "RequiresCompatibilities": ["FARGATE"],
                    "NetworkMode": "awsvpc",
                    "Cpu": "256",
                    "Memory": "512",
                    "ExecutionRoleArn": execution_role_arn(),
                    "ContainerDefinitions": containers,
                },
            },
            REFERENCING_REVISION: {
                "Type": "AWS::ECS::TaskDefinition",
                "Properties": {
                    "Family": "image-round-trip-referencing-family",
                    "RequiresCompatibilities": ["FARGATE"],
                    "NetworkMode": "awsvpc",
                    "Cpu": "256",
                    "Memory": "512",
                    "ExecutionRoleArn": execution_role_arn(),
                    "ContainerDefinitions": [{
                        "Name": "announced",
                        "Image": {"Fn::Sub": format!("${{Repo.RepositoryUri}}{REFERENCED_TAG}")},
                        "Essential": true,
                    }],
                },
            },
        },
    })
    .to_string()
}

/// The `Parameters.member.N.*` pairs the table's stack is created with.
fn supplied() -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (position, (key, value)) in [
        ("RepositoryName", SUPPLIED_REPOSITORY),
        ("ImageTag", SUPPLIED_TAG),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = position + 1;
        members.push(member(&format!("Parameters.member.{slot}.ParameterKey"), key));
        members.push(member(
            &format!("Parameters.member.{slot}.ParameterValue"),
            value,
        ));
    }
    members
}

/// Deploy `template` as `stack` and wait for it to settle in `CREATE_COMPLETE`.
async fn deployed_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
    extra: &[(String, String)],
) {
    let (status, body) = create_stack(router, stack, template, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    let settled = settled_stack_status(case, router, REGION, stack).await;
    assert_eq!(
        settled, CREATE_COMPLETE,
        "case {case:?}: expected the stack to create, got {settled:?} because {:?}",
        stack_event_reasons(router, REGION, stack).await
    );
}

/// The `image` Amazon ECS itself reports for the container `container` of the revision `arn`,
/// read through `DescribeTaskDefinition` — which is the value a client that deployed the stack
/// and then read the revision back sees.
async fn held_image(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    container: &str,
) -> String {
    let (code, answered) = ecs(
        router,
        "DescribeTaskDefinition",
        &json!({"taskDefinition": arn}),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from DescribeTaskDefinition({arn}), got {code} with \
         {answered}"
    );
    let definitions = answered["taskDefinition"]["containerDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let found = definitions
        .iter()
        .find(|definition| definition["name"].as_str() == Some(container))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a container definition named {container:?} on {arn}, \
                 got {answered}"
            )
        });
    found["image"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an image on the container {container:?}, got {found}")
        })
        .to_string()
}

/// The `repositoryUri` Amazon ECR itself announces for `name`.
async fn announced_repository_uri(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (code, answered) = json_call(
        router,
        "ecr",
        ECR_JSON,
        &format!("{ECR_TARGET_PREFIX}.DescribeRepositories"),
        &json!({"repositoryNames": [name]}),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from DescribeRepositories({name}), got {code} with \
         {answered}"
    );
    answered["repositories"][0]["repositoryUri"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a repositoryUri for {name:?}, got {answered}")
        })
        .to_string()
}

/// The authority and the reference a container image splits into: everything ahead of the first
/// `/`, which a client DIALS and reads the registry out of, and everything after it.
fn authority_and_reference(case: &str, image: &str) -> (String, String) {
    let (authority, reference) = image.split_once('/').unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected an image carrying an authority and a path, got {image:?}"
        )
    });
    (authority.to_string(), reference.to_string())
}

/// Assert that `image`'s authority is an Amazon ECR registry host of `account`: the account id,
/// the two fixed labels and the Region, then a non-empty zone the registry is served in — the
/// layout the registry URL format states, with no port, which is what a client reads the owning
/// account and the Region back out of.
fn assert_registry_authority(case: &str, image: &str, account: &str) {
    let (authority, _) = authority_and_reference(case, image);
    let labels: Vec<&str> = authority.split('.').collect();
    assert!(
        labels.len() > 4
            && labels[..4] == [account, "dkr", "ecr", REGION]
            && labels[4..].iter().all(|label| !label.is_empty()),
        "case {case:?}: expected the authority of {image:?} to be \
         {account}.dkr.ecr.{REGION}.<zone>, got {authority:?}"
    );
    assert!(
        !authority.contains(':'),
        "case {case:?}: expected no port on the registry authority of {image:?}, got \
         {authority:?}"
    );
}

#[tokio::test]
async fn a_declared_container_image_reaches_amazon_ecs_as_the_declaration_resolves() {
    let stack = "ecs-td-image-round-trip";
    let rows = rows();
    let router = router();
    deployed_stack(
        &router,
        "the image forms a template declares",
        stack,
        &table_template(&rows),
        &supplied()[..],
    )
    .await;

    // Every image the revision that reads no repository of the stack declares: the declaration
    // resolved, with nothing else rewritten.
    let revision =
        physical_resource_id(&router, "the table's revision", stack, LITERAL_REVISION).await;
    for row in &rows {
        let case = row.case;
        let expected = &row.expected;
        let held = held_image(&router, case, &revision, row.container).await;
        assert_eq!(
            &held, expected,
            "case {case:?}: expected Amazon ECS to report the image {expected:?}, got {held:?}"
        );
        if row.registry_form {
            let account = match held.starts_with(FOREIGN_ACCOUNT) {
                true => FOREIGN_ACCOUNT,
                false => ACCOUNT,
            };
            assert_registry_authority(case, &held, account);
        }
    }

    // ...and the other side of the rule: the revision whose declaration READS the repository
    // reaches Amazon ECS carrying the address Amazon ECR itself announced for it.
    let case = "the address the stack asked Amazon ECR itself for";
    let referencing = physical_resource_id(&router, case, stack, REFERENCING_REVISION).await;
    let announced = announced_repository_uri(&router, case, REPOSITORY).await;
    let expected = format!("{announced}{REFERENCED_TAG}");
    let held = held_image(&router, case, &referencing, "announced").await;
    assert_eq!(
        held, expected,
        "case {case:?}: expected Amazon ECS to report the image {expected:?}, got {held:?}"
    );
    // What the AUTHORITY of that address is, is Amazon ECR's to decide and not this rule's:
    // `Repository$repositoryUri` is the endpoint that registry answers a push and a pull at,
    // which this emulator announces as the address it is really served at rather than the
    // host AWS publishes (the ECR crate's `state::registry_address`), so the declaration
    // that READ it is pinned by the equality above — it reaches Amazon ECS as the address
    // Amazon ECR announced, whatever that address is. The registry-host layout is asserted
    // on the rows above, whose declarations WRITE a reference in AWS's own spelling.
}

/// The same declaration in the other template spelling: a YAML `!Sub` over the two
/// pseudo-parameters, which is the form a hand-written template states it in.
#[tokio::test]
async fn a_yaml_declared_container_image_reaches_amazon_ecs_as_the_declaration_resolves() {
    let case = "a YAML !Sub over the two pseudo-parameters";
    let stack = "ecs-td-image-round-trip-yaml";
    let repository = "image-round-trip-yaml-repo";
    let declared = declared_registry_reference(&format!("{repository}:latest"));
    let template = [
        "Resources:",
        "  Repo:",
        "    Type: AWS::ECR::Repository",
        "    Properties:",
        &format!("      RepositoryName: {repository}"),
        "  TaskDef:",
        "    Type: AWS::ECS::TaskDefinition",
        "    Properties:",
        "      Family: image-round-trip-yaml-family",
        "      RequiresCompatibilities:",
        "        - FARGATE",
        "      NetworkMode: awsvpc",
        "      Cpu: '256'",
        "      Memory: '512'",
        &format!("      ExecutionRoleArn: {}", execution_role_arn()),
        "      ContainerDefinitions:",
        "        - Name: app",
        &format!("          Image: !Sub '{declared}'"),
        "          Essential: 'true'",
        "",
    ]
    .join("\n");
    let router = router();
    deployed_stack(&router, case, stack, &template, &[]).await;
    let revision = physical_resource_id(&router, case, stack, "TaskDef").await;
    let held = held_image(&router, case, &revision, "app").await;
    let expected = resolved_registry_reference(&format!("{repository}:latest"));
    assert_eq!(
        held, expected,
        "case {case:?}: expected Amazon ECS to report the image {expected:?}, got {held:?}"
    );
    assert_registry_authority(case, &held, ACCOUNT);
}
