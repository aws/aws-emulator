// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Glue::Job` resources its template declares — in AWS Glue, in the stack's own
//! Region, with the definition the declaration carries.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, through that service's own public API, in the stack's Region and
//! under the physical id the stack publishes — so the handle a stack's output carries
//! addresses a resource that service really holds, and deleting the stack releases it
//! there.** For a job definition the handle is its NAME ("Ref returns the job name"), so
//! right after the create waiter returns, `glue GetJob` answers 200 for the name the stack
//! exported, with `Job.Role` the role ARN the template passed, rather than AWS Glue
//! answering `EntityNotFoundException` for a job its own stack just reported complete.
//!
//! Two crates have to agree for that to hold (CloudFormation makes the calls, AWS Glue
//! serves them), so the suite lives here: a crate-local test can only ever see one side of
//! it. IAM is mounted beside them because the role a job assumes is the other half of the
//! template the ordinary deploy writes.
//!
//! The rows differ along what the rule generalises over: how the template names the job
//! (through `Fn::Sub` over the account and Region pseudo-parameters, literally, or not at
//! all — leaving the name to CloudFormation), whether it is written in JSON or YAML, which
//! command kind it declares (`glueetl`, `pythonshell`, `gluestreaming`), which optional
//! properties it carries (version, arguments, retries, timeout, worker configuration,
//! concurrency, capacity, maintenance window, connections), which of the two tag spellings
//! it uses, and the Region the stack is deployed in.
//!
//! The region half, the update half, the deletion half and the refusal half are rows of the
//! same rule: the job exists in the stack's Region ALONE, a changed declaration reaches AWS
//! Glue through that service's own update, deleting the stack deletes the job, and a
//! declaration AWS Glue REFUSES leaves the stack `ROLLBACK_COMPLETE` naming the resource
//! instead of a green deploy over a job nothing holds.
//!
//! The awsQuery form encoder and the XML readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.
//!
//! The router this suite mounts and the two wire helpers over it are `pub(crate)` for the
//! same reason: the suite beside this one that deploys ONE stack name into two Regions
//! ([`crate::cloudformation_glue_job_redeploy_across_regions`]) drives exactly these two
//! wires, so it calls these rather than carrying a second copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_stack_clock::settle_the_stacks;
use crate::query_wire::{element, form_encode, opt_element, outputs_of, physical_ids_of};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due for a stack whose resources depend on one another (the job's role is created first),
/// so the stack reports the deployment it completed. Neither an IAM role nor a Glue job
/// definition has a transient status of its own: each create answers with a resource that
/// is usable at once.
pub(crate) const PAST_STACK_SPAN: Duration = Duration::from_millis(1_200);

/// The account id every ARN this emulator mints carries, which the `Fn::Sub` row's job name
/// is derived from.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The longest job name AWS Glue accepts (`NameString`, `@length(min: 1, max: 255)` in that
/// service's own model), which a GENERATED name is held to.
const MAX_JOB_NAME: usize = 255;

/// The logical id every row gives its job, so the physical id is read from the one resource
/// the rule is about.
const JOB_LOGICAL_ID: &str = "GlueJob";

/// The output keys every row's template publishes, which is how a client reaches the
/// deployed job at all.
const NAME_OUTPUT: &str = "GlueJobName";
const ROLE_OUTPUT: &str = "GlueJobRoleArn";
const REGION_OUTPUT: &str = "DeploymentRegion";

/// The stack-level tag every `CreateStack` below supplies, which CloudFormation propagates
/// to the resources it creates.
const STACK_TAG: (&str, &str) = ("Stack", "glue-job-suite");

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
pub(crate) fn build_router() -> MultiServiceRouter {
    build_router_and_engine().0
}

/// [`build_router`] with the engine it was mounted on, for a case that drives the stack's own
/// CLOCK rather than waiting for it
/// ([`crate::cloudformation_stack_clock::settle_the_stacks`]).
pub(crate) fn build_router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
pub(crate) async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one AWS Glue operation over its awsJson1_1 wire, through `region`'s endpoint.
pub(crate) async fn glue(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("glue.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization("glue", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The error code an awsJson answer carries, without the namespace a `__type` may qualify
/// it with.
pub(crate) fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// The ARN a job definition of `name` carries in `region`: `job/<name>` under AWS Glue's own
/// ARN namespace, which is the handle that service's tagging calls take.
fn job_arn(region: &str, name: &str) -> String {
    format!("arn:aws:glue:{region}:{ACCOUNT}:job/{name}")
}

/// A Region that is NOT `region`, for the half of the rule that says a job definition is
/// reachable only through the endpoint of the Region its stack was deployed in.
fn foreign_region(region: &str) -> &'static str {
    if region == "us-east-2" {
        "us-east-1"
    } else {
        "us-east-2"
    }
}

/// The trust policy every row's role carries: AWS Glue has to be able to assume the role a
/// job runs as.
///
/// Written with no `&`, `=`, `+` or `%` so it travels in an awsQuery form body as written.
pub(crate) const TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"glue.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// The names AWS Glue reports for the job definitions `region`'s endpoint holds, sorted so
/// the comparison is about membership.
pub(crate) async fn listed_names(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
) -> Vec<String> {
    let (status, body) = glue(router, region, "ListJobs", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListJobs in {region}, got {status} with body \
         {body}"
    );
    let mut names: Vec<String> = body["JobNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no JobNames list in {body}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// One row of the table: a template, and what the rule says the job it declares must then be
/// in AWS Glue.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the job is created in and the one
    /// its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), built from the
    /// Region it deploys in.
    template: fn(&str) -> String,
    /// The job name the template asks for, as a function of the Region (the `Fn::Sub` row's
    /// name is derived from the account and the Region), or `None` for a template that names
    /// nothing — whose job carries the name CloudFormation generated.
    declared_name: fn(&str) -> Option<String>,
    /// The members `GetJob` must report for the job, as `(JSON pointer into Job, value)`
    /// pairs built from what the row declares.
    expected_members: fn(&str) -> Vec<(&'static str, Value)>,
    /// The tags the job's own declaration asks for, whichever spelling it wrote them in.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// The rows: four templates spanning how the job is named, which wire format the template is
/// written in, which command kind and optional properties it declares, which tag spelling it
/// uses, and which Region it deploys in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a job named through Fn::Sub over the account and Region, in JSON, with a \
                   Spark ETL command and default arguments",
            region: "us-east-1",
            template: |_| {
                json!({
                    "AWSTemplateFormatVersion": "2010-09-09",
                    "Description": "a Glue ETL job and the role it runs as",
                    "Resources": {
                        "GlueJobRole": {
                            "Type": "AWS::IAM::Role",
                            "Properties": {
                                "RoleName": "gjwr-role-report",
                                "AssumeRolePolicyDocument": serde_json::from_str::<Value>(
                                    TRUST_POLICY
                                ).expect("the trust policy is a document"),
                            }
                        },
                        JOB_LOGICAL_ID: {
                            "Type": "AWS::Glue::Job",
                            "Properties": {
                                "Name": {
                                    "Fn::Sub": "gjwr-RoleReport-${AWS::AccountId}-${AWS::Region}"
                                },
                                "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                                "Command": {
                                    "Name": "glueetl",
                                    "PythonVersion": "3",
                                    "ScriptLocation": "s3://sim2real-etl/report.py"
                                },
                                "GlueVersion": "4.0",
                                "DefaultArguments": { "--job-language": "python" },
                                "Tags": { "Owner": "analytics" }
                            }
                        }
                    },
                    "Outputs": {
                        NAME_OUTPUT: { "Value": { "Ref": JOB_LOGICAL_ID } },
                        ROLE_OUTPUT: { "Value": { "Fn::GetAtt": ["GlueJobRole", "Arn"] } },
                        REGION_OUTPUT: { "Value": { "Ref": "AWS::Region" } }
                    }
                })
                .to_string()
            },
            declared_name: |region| Some(format!("gjwr-RoleReport-{ACCOUNT}-{region}")),
            expected_members: |_| {
                vec![
                    ("/Command/Name", json!("glueetl")),
                    ("/Command/PythonVersion", json!("3")),
                    ("/Command/ScriptLocation", json!("s3://sim2real-etl/report.py")),
                    ("/GlueVersion", json!("4.0")),
                    ("/DefaultArguments/--job-language", json!("python")),
                ]
            },
            expected_tags: &[("Owner", "analytics")],
        },
        Row {
            case: "a job named literally, in YAML, with a Python shell command, a worker \
                   configuration and tags written as a list",
            region: "us-west-2",
            template: |_| {
                [
                    "Resources:",
                    "  GlueJobRole:",
                    "    Type: AWS::IAM::Role",
                    "    Properties:",
                    "      RoleName: gjwr-role-ingest",
                    "      AssumeRolePolicyDocument:",
                    "        Version: '2012-10-17'",
                    "        Statement:",
                    "          - Effect: Allow",
                    "            Principal:",
                    "              Service: glue.amazonaws.com",
                    "            Action: sts:AssumeRole",
                    "  GlueJob:",
                    "    Type: AWS::Glue::Job",
                    "    Properties:",
                    "      Name: gjwr-ingest",
                    "      Description: the nightly ingest",
                    "      Role: !GetAtt GlueJobRole.Arn",
                    "      Command:",
                    "        Name: pythonshell",
                    "        PythonVersion: '3.9'",
                    "        ScriptLocation: s3://sim2real-etl/ingest.py",
                    "      MaxRetries: 2",
                    "      Timeout: 60",
                    "      WorkerType: G.1X",
                    "      NumberOfWorkers: 5",
                    "      ExecutionProperty:",
                    "        MaxConcurrentRuns: 3",
                    "      Connections:",
                    "        Connections:",
                    "          - a-connection",
                    "      Tags:",
                    "        - Key: Owner",
                    "          Value: ingest",
                    "        - Key: Tier",
                    "          Value: prod",
                    "Outputs:",
                    "  GlueJobName:",
                    "    Value: !Ref GlueJob",
                    "  GlueJobRoleArn:",
                    "    Value: !GetAtt GlueJobRole.Arn",
                    "  DeploymentRegion:",
                    "    Value: !Ref AWS::Region",
                    "",
                ]
                .join("\n")
            },
            declared_name: |_| Some("gjwr-ingest".to_string()),
            expected_members: |_| {
                vec![
                    ("/Description", json!("the nightly ingest")),
                    ("/Command/Name", json!("pythonshell")),
                    ("/Command/PythonVersion", json!("3.9")),
                    ("/Command/ScriptLocation", json!("s3://sim2real-etl/ingest.py")),
                    ("/MaxRetries", json!(2)),
                    ("/Timeout", json!(60)),
                    ("/WorkerType", json!("G.1X")),
                    ("/NumberOfWorkers", json!(5)),
                    ("/ExecutionProperty/MaxConcurrentRuns", json!(3)),
                    ("/Connections/Connections/0", json!("a-connection")),
                ]
            },
            expected_tags: &[("Owner", "ingest"), ("Tier", "prod")],
        },
        Row {
            case: "a job the template does not name at all, with a streaming command and no \
                   optional property",
            region: "eu-west-1",
            template: |_| {
                json!({
                    "Resources": {
                        "GlueJobRole": {
                            "Type": "AWS::IAM::Role",
                            "Properties": {
                                "AssumeRolePolicyDocument": serde_json::from_str::<Value>(
                                    TRUST_POLICY
                                ).expect("the trust policy is a document"),
                            }
                        },
                        JOB_LOGICAL_ID: {
                            "Type": "AWS::Glue::Job",
                            "Properties": {
                                "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                                "Command": {
                                    "Name": "gluestreaming",
                                    "ScriptLocation": "s3://sim2real-etl/stream.py"
                                }
                            }
                        }
                    },
                    "Outputs": {
                        NAME_OUTPUT: { "Value": { "Ref": JOB_LOGICAL_ID } },
                        ROLE_OUTPUT: { "Value": { "Fn::GetAtt": ["GlueJobRole", "Arn"] } },
                        REGION_OUTPUT: { "Value": { "Ref": "AWS::Region" } }
                    }
                })
                .to_string()
            },
            declared_name: |_| None,
            expected_members: |_| {
                vec![
                    ("/Command/Name", json!("gluestreaming")),
                    ("/Command/ScriptLocation", json!("s3://sim2real-etl/stream.py")),
                ]
            },
            expected_tags: &[],
        },
        Row {
            case: "a job declaring a capacity, a maintenance window, queuing and \
                   non-overridable arguments, deployed in a third Region",
            region: "ap-southeast-1",
            template: |region| {
                json!({
                    "Resources": {
                        "GlueJobRole": {
                            "Type": "AWS::IAM::Role",
                            "Properties": {
                                "RoleName": { "Fn::Sub": "gjwr-role-${AWS::Region}" },
                                "AssumeRolePolicyDocument": serde_json::from_str::<Value>(
                                    TRUST_POLICY
                                ).expect("the trust policy is a document"),
                            }
                        },
                        JOB_LOGICAL_ID: {
                            "Type": "AWS::Glue::Job",
                            "Properties": {
                                "Name": format!("gjwr-capacity-{region}"),
                                "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                                "Command": {
                                    "Name": "glueetl",
                                    "ScriptLocation": "s3://sim2real-etl/capacity.py"
                                },
                                "MaxCapacity": 2.0,
                                "MaintenanceWindow": "Mon:10",
                                "JobRunQueuingEnabled": true,
                                "NonOverridableArguments": { "--enable-metrics": "true" },
                                "Tags": { "Owner": "platform", "Tier": "batch" }
                            }
                        }
                    },
                    "Outputs": {
                        NAME_OUTPUT: { "Value": { "Ref": JOB_LOGICAL_ID } },
                        ROLE_OUTPUT: { "Value": { "Fn::GetAtt": ["GlueJobRole", "Arn"] } },
                        REGION_OUTPUT: { "Value": { "Ref": "AWS::Region" } }
                    }
                })
                .to_string()
            },
            declared_name: |region| Some(format!("gjwr-capacity-{region}")),
            expected_members: |_| {
                vec![
                    ("/Command/Name", json!("glueetl")),
                    ("/Command/ScriptLocation", json!("s3://sim2real-etl/capacity.py")),
                    ("/MaxCapacity", json!(2.0)),
                    ("/MaintenanceWindow", json!("Mon:10")),
                    ("/JobRunQueuingEnabled", json!(true)),
                    ("/NonOverridableArguments/--enable-metrics", json!("true")),
                ]
            },
            expected_tags: &[("Owner", "platform"), ("Tier", "batch")],
        },
    ]
}

/// Every row: the name the stack reports for its `AWS::Glue::Job` is a job definition AWS
/// Glue really holds in the stack's Region, configured as the template declared it, carrying
/// the role ARN the template passed and the tags CloudFormation gave it — and deleting the
/// stack releases it there.
#[tokio::test]
async fn a_stack_provisions_the_glue_job_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("glue-job-stack-{index}");
        let template = (row.template)(region);

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                // Each row's template declares the IAM role the job runs as, and three of
                // them name it, so the request acknowledges the template's IAM resources.
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
                ("Tags.member.1.Key", STACK_TAG.0),
                ("Tags.member.1.Value", STACK_TAG.1),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let job_name = physical_ids
            .get(JOB_LOGICAL_ID)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {JOB_LOGICAL_ID} resource in {resources}"));

        // "Ref returns the job name": the physical id IS the name the template asked for, or
        // the one CloudFormation generated for a template that named nothing — which is held
        // to the name AWS Glue accepts.
        if let Some(declared) = (row.declared_name)(region) {
            assert_eq!(
                job_name, declared,
                "case {case:?}: expected the physical id to be the declared name {declared:?}, \
                 got {job_name:?}"
            );
        }
        assert!(
            !job_name.is_empty() && job_name.chars().count() <= MAX_JOB_NAME,
            "case {case:?}: expected a name inside AWS Glue's own NameString bound of \
             {MAX_JOB_NAME}, got {job_name:?} ({} characters)",
            job_name.chars().count()
        );

        // The stack publishes that name, and the Region it was deployed in, through its
        // outputs — which is how a client reaches the job at all.
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get(NAME_OUTPUT),
            Some(&job_name),
            "case {case:?}: expected output {NAME_OUTPUT:?} to be the job name {job_name:?}, \
             got {outputs:?}"
        );
        assert_eq!(
            outputs.get(REGION_OUTPUT).map(String::as_str),
            Some(region),
            "case {case:?}: expected output {REGION_OUTPUT:?} to be {region:?}, got {outputs:?}"
        );
        let role_arn = outputs
            .get(ROLE_OUTPUT)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {ROLE_OUTPUT} output in {described}"));

        // CloudFormation's own resource index resolves that name back to the stack, the join
        // a client makes to ask which stack owns a physical id.
        let (status, owned) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("PhysicalResourceId", &job_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources on the physical id, \
             got {status} with body {owned}"
        );
        assert_eq!(
            element(&owned, "StackName"),
            stack_name,
            "case {case:?}: expected the physical id to resolve to {stack_name:?}, got {owned}"
        );

        // And AWS Glue itself resolves that name to the job definition the template declared.
        let (status, read) = glue(&router, region, "GetJob", &json!({ "JobName": job_name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetJob on the name the stack reported, got \
             {status} with body {read}"
        );
        let job = read.get("Job").cloned().unwrap_or(Value::Null);
        assert_eq!(
            job.get("Name").and_then(Value::as_str),
            Some(job_name.as_str()),
            "case {case:?}: expected Job.Name to be {job_name:?}, got {job}"
        );
        assert_eq!(
            job.get("Role").and_then(Value::as_str),
            Some(role_arn.as_str()),
            "case {case:?}: expected Job.Role to be the role ARN the template passed \
             ({role_arn:?}), got {job}"
        );
        for (pointer, expected) in (row.expected_members)(region) {
            let actual = job.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Job{pointer} to be {expected}, got {actual} in {job}"
            );
        }

        // The job exists in the stack's Region ALONE: AWS Glue is regional, so another
        // Region's endpoint holds no job under that name.
        let elsewhere = foreign_region(region);
        let (status, absent) =
            glue(&router, elsewhere, "GetJob", &json!({ "JobName": job_name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetJob in {elsewhere}, got {status} with \
             body {absent}"
        );
        assert_eq!(
            error_code(&absent),
            "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException in {elsewhere}, got {absent}"
        );

        // The Region's own listing names it, so a client enumerating that Region's jobs sees
        // the deployed one.
        let listed = listed_names(&router, case, region).await;
        assert!(
            listed.contains(&job_name),
            "case {case:?}: expected ListJobs in {region} to name {job_name:?}, got {listed:?}"
        );

        // The tags the declaration asked for are on the job, beside the stack's own and the
        // three CloudFormation records on every resource it creates.
        let arn = job_arn(region, &job_name);
        let (status, tagged) = glue(&router, region, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTags on {arn}, got {status} with body \
             {tagged}"
        );
        let reported: BTreeMap<String, String> = tagged
            .get("Tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no Tags map in {tagged}"))
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get(STACK_TAG.0).map(String::as_str),
            Some(STACK_TAG.1),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("aws:cloudformation:stack-name")
                .map(String::as_str),
            Some(stack_name.as_str()),
            "case {case:?}: expected the job to carry the stack it belongs to, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("aws:cloudformation:logical-id")
                .map(String::as_str),
            Some(JOB_LOGICAL_ID),
            "case {case:?}: expected the job to carry its logical id, got {reported:?}"
        );

        // Deleting the stack releases the job AWS Glue holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = glue(&router, region, "GetJob", &json!({ "JobName": job_name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetJob once the stack deleted the job, got \
             {status} with body {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException once the job was deleted, got {gone}"
        );
        let listed = listed_names(&router, case, region).await;
        assert!(
            !listed.contains(&job_name),
            "case {case:?}: expected ListJobs in {region} to name {job_name:?} no longer, got \
             {listed:?}"
        );
    }
}

/// A declaration AWS Glue REFUSES does not report a resource nothing created: the stack fails
/// with that service's own refusal, and no job is left behind.
///
/// The rows differ along WHAT that service refuses: a command kind no job can run, a timeout
/// past the documented bound, a capacity stated two ways at once, and a name a client already
/// holds a job under in that Region.
#[tokio::test]
async fn a_glue_job_declaration_the_owning_service_refuses_fails_the_stack() {
    let (router, engine) = build_router_and_engine();
    let region = "us-east-2";

    // The job a client already holds, which the last row's template re-declares.
    let held = "gjwr-already-there";
    let (status, created) = glue(
        &router,
        region,
        "CreateJob",
        &json!({
            "Name": held,
            "Role": format!("arn:aws:iam::{ACCOUNT}:role/the-clients-own"),
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/theirs.py" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateJob to succeed, got {status} with body {created}"
    );

    // (case, the job's declared properties beyond its role, the name the declaration asks
    // for — which must hold no job once the stack has failed, except where a client created
    // it).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a command kind no job can run",
            json!({
                "Name": "gjwr-bad-command",
                "Command": { "Name": "notacommand", "ScriptLocation": "s3://sim2real-etl/x.py" },
            }),
            "gjwr-bad-command",
        ),
        (
            "a timeout past the documented bound",
            json!({
                "Name": "gjwr-bad-timeout",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/x.py" },
                "Timeout": 20_161,
            }),
            "gjwr-bad-timeout",
        ),
        (
            "a capacity stated as both a maximum and a worker configuration",
            json!({
                "Name": "gjwr-bad-capacity",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/x.py" },
                "MaxCapacity": 4.0,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 4,
            }),
            "gjwr-bad-capacity",
        ),
        (
            "a name a client already holds a job under",
            json!({
                "Name": held,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/x.py" },
            }),
            held,
        ),
    ];

    for (index, (case, declared, job_name)) in rows.into_iter().enumerate() {
        let stack_name = format!("glue-job-refusal-stack-{index}");
        let mut properties = declared.as_object().cloned().expect("declared properties");
        properties.insert(
            "Role".to_string(),
            json!({ "Fn::GetAtt": ["GlueJobRole", "Arn"] }),
        );
        let template = json!({
            "Resources": {
                "GlueJobRole": {
                    "Type": "AWS::IAM::Role",
                    "Properties": {
                        "AssumeRolePolicyDocument": serde_json::from_str::<Value>(TRUST_POLICY)
                            .expect("the trust policy is a document"),
                    }
                },
                JOB_LOGICAL_ID: { "Type": "AWS::Glue::Job", "Properties": properties }
            }
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                // The refusal these rows are about is the OWNING SERVICE's, so the create has
                // to reach it: with the pre-deployment validations on, CloudFormation's own
                // existence check fails a create whose declared name that service already
                // holds before any resource is asked for.
                ("DisableValidation", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        // The create failed, so the stack is being rolled back. Both transitions it owes are
        // on the stack's own clock, so the case drives that clock rather than waiting for it
        // ([`crate::cloudformation_stack_clock::settle_the_stacks`]).
        settle_the_stacks(&router, &engine, case).await;
        settle_the_stacks(&router, &engine, case).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back, got {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains(JOB_LOGICAL_ID),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        let (status, read) = glue(&router, region, "GetJob", &json!({ "JobName": job_name })).await;
        if job_name == held {
            // The job the CLIENT created is still exactly as it was: the stack's failure
            // neither replaced nor removed it.
            assert_eq!(
                status, 200,
                "case {case:?}: expected the client's job to still resolve, got {status} with \
                 body {read}"
            );
            assert_eq!(
                read.pointer("/Job/Command/ScriptLocation").and_then(Value::as_str),
                Some("s3://sim2real-etl/theirs.py"),
                "case {case:?}: expected the client's own job untouched, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected no job to be left behind by the failed stack, got \
                 {status} with body {read}"
            );
            assert_eq!(
                error_code(&read),
                "EntityNotFoundException",
                "case {case:?}: expected EntityNotFoundException for the failed resource, got \
                 {read}"
            );
        }
    }
}

/// One deployment of the update suite's one stack: the case, the properties the template
/// declares for the job beyond its name and role, and the `(JSON pointer into Job, value)`
/// pairs `GetJob` must then report.
type Deployment = (&'static str, Value, Vec<(&'static str, Value)>);

/// A CHANGED `AWS::Glue::Job` declaration reaches AWS Glue through that service's own update:
/// the stack keeps the job it created and the definition in force is the one the template now
/// declares.
///
/// The rows differ along which member the second deployment changes — the script the job
/// runs, its description and timeout, and its concurrency — so what is asserted is that the
/// declaration in force reaches the owning service, not that one member does.
#[tokio::test]
async fn a_changed_glue_job_declaration_is_applied_through_the_owning_service() {
    let router = build_router();
    let region = "eu-central-1";
    let stack_name = "glue-job-update-stack";
    let job_name = "gjwr-updated";

    // (case, the properties the deployment declares beyond the job's name and role, the
    // members GetJob must then report).
    let deployments: Vec<Deployment> = vec![
        (
            "the first deployment, which creates the job",
            json!({
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/one.py" },
                "Timeout": 30,
            }),
            vec![
                ("/Command/ScriptLocation", json!("s3://sim2real-etl/one.py")),
                ("/Timeout", json!(30)),
            ],
        ),
        (
            "a changed script and a changed timeout",
            json!({
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/two.py" },
                "Timeout": 90,
                "Description": "the second deployment",
            }),
            vec![
                ("/Command/ScriptLocation", json!("s3://sim2real-etl/two.py")),
                ("/Timeout", json!(90)),
                ("/Description", json!("the second deployment")),
            ],
        ),
        (
            "a changed concurrency and worker configuration",
            json!({
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://sim2real-etl/two.py" },
                "Timeout": 90,
                "Description": "the second deployment",
                "ExecutionProperty": { "MaxConcurrentRuns": 7 },
                "WorkerType": "G.2X",
                "NumberOfWorkers": 3,
            }),
            vec![
                ("/ExecutionProperty/MaxConcurrentRuns", json!(7)),
                ("/WorkerType", json!("G.2X")),
                ("/NumberOfWorkers", json!(3)),
            ],
        ),
    ];

    for (index, (case, declared, expected)) in deployments.into_iter().enumerate() {
        let mut properties = declared.as_object().cloned().expect("declared properties");
        properties.insert("Name".to_string(), json!(job_name));
        properties.insert(
            "Role".to_string(),
            json!({ "Fn::GetAtt": ["GlueJobRole", "Arn"] }),
        );
        let template = json!({
            "Resources": {
                "GlueJobRole": {
                    "Type": "AWS::IAM::Role",
                    "Properties": {
                        "AssumeRolePolicyDocument": serde_json::from_str::<Value>(TRUST_POLICY)
                            .expect("the trust policy is a document"),
                    }
                },
                JOB_LOGICAL_ID: { "Type": "AWS::Glue::Job", "Properties": properties }
            }
        })
        .to_string();

        let action = if index == 0 { "CreateStack" } else { "UpdateStack" };
        let (status, body) = cloudformation(
            &router,
            region,
            action,
            &[
                ("StackName", stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        let expected_status = if index == 0 {
            "CREATE_COMPLETE"
        } else {
            "UPDATE_COMPLETE"
        };
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some(expected_status),
            "case {case:?}: expected the stack to reach {expected_status}, got {described}"
        );

        let (status, read) = glue(&router, region, "GetJob", &json!({ "JobName": job_name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetJob, got {status} with body {read}"
        );
        let job = read.get("Job").cloned().unwrap_or(Value::Null);
        assert_eq!(
            job.get("Name").and_then(Value::as_str),
            Some(job_name),
            "case {case:?}: expected the job to keep its identity, got {job}"
        );
        for (pointer, expected) in expected {
            let actual = job.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Job{pointer} to be {expected}, got {actual} in {job}"
            );
        }
    }
}

/// A stack that declares no resource of this type at all still creates no Glue job: the arm
/// is reached by the TYPE, not by anything in the request.
///
/// This is the control row of the rule above — a stack whose resource types this emulator
/// leaves declared-only must create nothing in AWS Glue, and must still complete.
#[tokio::test]
async fn a_stack_declaring_no_glue_job_creates_none() {
    let router = build_router();
    let region = "ap-northeast-1";
    let template = json!({
        "Resources": {
            "Waiter": { "Type": "AWS::CloudFormation::WaitConditionHandle", "Properties": {} }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", "no-glue-job-stack"),
            ("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "no-glue-job-stack")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the stack to complete, got {described}"
    );

    let listed = listed_names(&router, "no job declared", region).await;
    assert!(
        listed.is_empty(),
        "expected no job definition to exist in {region}, got {listed:?}"
    );
}
