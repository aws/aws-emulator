// SPDX-License-Identifier: CC0-1.0

//! A stack that owns AWS Elastic Beanstalk resources is reported `DELETE_COMPLETE` only once
//! Elastic Beanstalk no longer holds them.
//!
//! The rule under test: **CloudFormation does not report a stack operation complete while the
//! service that owns a resource that operation deleted still holds that resource; the
//! operation stays `*_IN_PROGRESS` until that service's own read reports the resource gone.**
//! It is the rule `cloudformation_resource_removal_stabilisation` states for the owners whose
//! delete this emulator answers with the resource still there, carried to the owner this suite
//! is about: `DeleteApplication` "deletes the specified application along with all associated
//! versions and configurations" and answers BEFORE it has, so the accepted call is not
//! evidence the application is gone. Two crates have to agree for the rule to hold
//! (CloudFormation makes the calls, Elastic Beanstalk decides when the application is gone),
//! so the suite lives here rather than in either of them.
//!
//! What it pins is the observation an infrastructure-as-code client makes: the instant the
//! stack first reports its deletion complete, the application it created is gone in Elastic
//! Beanstalk — `DescribeApplications` reports none, and a call naming it is refused the way
//! that service refuses a call naming an application that does not exist, so the name is free
//! to be created again.
//!
//! The rows differ along what the rule generalises over: how many Elastic Beanstalk resources
//! the stack owns and in which order they have to go (an application alone; an application
//! whose environment has to be terminated first), whether the template NAMES the application
//! or leaves CloudFormation to generate the name, whether the environment names its platform
//! by `SolutionStackName` or by `PlatformArn`, which Region the stack deploys in, and whether
//! the application carries a configuration template made outside the stack. Every expected
//! value is computed here from what the row declares, never read back out of the answer being
//! checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this test crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element, outputs_of};

/// Elastic Beanstalk's API version, as its model declares it (`version: 2010-12-01`), and
/// CloudFormation's.
const EB_API_VERSION: &str = "2010-12-01";
const CFN_API_VERSION: &str = "2010-05-15";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// How often the suite asks the stack whether its operation has concluded, and how many times.
///
/// The interval is short because the FIRST completed observation is what the rule is stated
/// about; the product is a budget far longer than the spans these two services choose for the
/// transitions one stack operation starts, so a loaded machine is merely slower rather than a
/// failure.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 1_600;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
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
    router.register_service(eb::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service` in `region`, returning its status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    region: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service, region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "cloudformation",
        region,
        CFN_API_VERSION,
        action,
        members,
    )
    .await
}

/// Call one Elastic Beanstalk operation over its awsQuery wire, signed for `region`.
async fn elasticbeanstalk(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "elasticbeanstalk",
        region,
        EB_API_VERSION,
        action,
        members,
    )
    .await
}

/// The solution stack a row's environment runs on: the current version of the FIRST branch of
/// the service's own published catalog (`eb::state::SOLUTION_STACKS`, what
/// `ListAvailableSolutionStacks` reports), named by its solution stack name.
fn first_solution_stack() -> String {
    eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .name()
}

/// A platform version named by its ARN in `region`: a DIFFERENT branch of the same catalog, so
/// the two ways of naming a platform are both exercised.
fn second_platform_arn(region: &str) -> String {
    eb::state::SOLUTION_STACKS
        .get(1)
        .expect("the service publishes several solution stacks")
        .platform_arn(region)
}

/// One row of the table: a stack of Elastic Beanstalk resources, and what the rule says must
/// be true of Elastic Beanstalk the instant that stack's deletion first reports itself
/// complete.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region its resources are created in.
    region: &'static str,
    /// The template body, built from the solution stack name and the platform ARN of the row's
    /// Region.
    template: fn(&str, &str) -> String,
    /// The name the template declares for the application, or `None` for a row that names
    /// none — whose application carries the name CloudFormation generated.
    declared_application: Option<&'static str>,
    /// Whether the template declares an environment ON that application, which the stack's
    /// deletion has to terminate before the application can go at all.
    declares_an_environment: bool,
    /// A configuration template created in Elastic Beanstalk directly, on the stack's
    /// application, before the stack is deleted — one of the "associated … configurations"
    /// that go with the application — or `None` for a row that makes none.
    seeded_configuration_template: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "an application alone, named by the template",
        region: "us-west-2",
        template: |_stack, _platform| {
            json!({
                "Resources": {
                    "EbApplication": {
                        "Type": "AWS::ElasticBeanstalk::Application",
                        "Properties": {
                            "ApplicationName": "removal-solo-app",
                            "Description": "an application nothing runs on"
                        }
                    }
                },
                "Outputs": { "AppRef": { "Value": { "Ref": "EbApplication" } } }
            })
            .to_string()
        },
        declared_application: Some("removal-solo-app"),
        declares_an_environment: false,
        seeded_configuration_template: None,
    },
    Row {
        case: "an application whose web-server environment on a solution stack has to go first",
        region: "us-east-1",
        template: |solution_stack, _platform| {
            json!({
                "Resources": {
                    "EbApplication": {
                        "Type": "AWS::ElasticBeanstalk::Application",
                        "Properties": { "ApplicationName": "removal-hosting-app" }
                    },
                    "EbEnvironment": {
                        "Type": "AWS::ElasticBeanstalk::Environment",
                        "Properties": {
                            "ApplicationName": { "Ref": "EbApplication" },
                            "EnvironmentName": "removal-hosting-env",
                            "SolutionStackName": solution_stack,
                            "CNAMEPrefix": "removal-hosting"
                        }
                    }
                },
                "Outputs": {
                    "AppRef": { "Value": { "Ref": "EbApplication" } },
                    "EnvRef": { "Value": { "Ref": "EbEnvironment" } }
                }
            })
            .to_string()
        },
        declared_application: Some("removal-hosting-app"),
        declares_an_environment: true,
        seeded_configuration_template: None,
    },
    Row {
        case: "an unnamed application and an environment on a platform ARN",
        region: "eu-west-1",
        template: |_stack, platform| {
            json!({
                "Resources": {
                    "EbApplication": { "Type": "AWS::ElasticBeanstalk::Application" },
                    "EbEnvironment": {
                        "Type": "AWS::ElasticBeanstalk::Environment",
                        "Properties": {
                            "ApplicationName": { "Ref": "EbApplication" },
                            "PlatformArn": platform,
                            "CNAMEPrefix": "removal-generated"
                        }
                    }
                },
                "Outputs": {
                    "AppRef": { "Value": { "Ref": "EbApplication" } },
                    "EnvRef": { "Value": { "Ref": "EbEnvironment" } }
                }
            })
            .to_string()
        },
        declared_application: None,
        declares_an_environment: true,
        seeded_configuration_template: None,
    },
    Row {
        case: "an application carrying a configuration template made outside the stack",
        region: "us-west-2",
        template: |_stack, _platform| {
            json!({
                "Resources": {
                    "EbApplication": {
                        "Type": "AWS::ElasticBeanstalk::Application",
                        "Properties": { "ApplicationName": "removal-configured-app" }
                    }
                },
                "Outputs": { "AppRef": { "Value": { "Ref": "EbApplication" } } }
            })
            .to_string()
        },
        declared_application: Some("removal-configured-app"),
        declares_an_environment: false,
        seeded_configuration_template: Some("removal-configured-template"),
    },
];

/// The application names an Elastic Beanstalk `DescribeApplications` answer reports: one
/// `ApplicationName` per application, and nothing else in that answer carries that element.
fn reported_applications(xml: &str) -> Vec<String> {
    elements(xml, "ApplicationName")
}

/// How many environments a `DescribeEnvironments` answer reports: one `EnvironmentId` per
/// environment.
fn environment_count(xml: &str) -> usize {
    elements(xml, "EnvironmentId").len()
}

/// The `(Code, Message)` an awsQuery error envelope carries, with the serializer's entity
/// escaping undone — the message quotes the application name in apostrophes, which the XML
/// writer escapes.
fn refusal(xml: &str) -> (String, String) {
    let text = |name: &str| elements(xml, name).into_iter().next().unwrap_or_default();
    (text("Code"), text("Message"))
}

/// The message Elastic Beanstalk refuses a request naming an application it does not hold
/// with: fixed text around the `ApplicationName` the REQUEST carried, which is the only
/// variable part of it.
fn no_application_named(application: &str) -> String {
    format!("No Application named '{application}' found.")
}

/// `CreateConfigurationTemplate` on `application`, which is "associated with a specific
/// Elastic Beanstalk application" — so it is served where that application exists and refused
/// where it does not.
async fn create_configuration_template(
    router: &MultiServiceRouter,
    region: &str,
    application: &str,
    template_name: &str,
) -> (u16, String) {
    elasticbeanstalk(
        router,
        region,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", application),
            ("TemplateName", template_name),
            ("SolutionStackName", &first_solution_stack()),
        ],
    )
    .await
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    stack: &str,
) -> (String, String) {
    let (status, body) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks for {stack:?}, got {status} with \
         body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, failing the case if it rests in anything else.
async fn wait_for(router: &MultiServiceRouter, case: &str, region: &str, stack: &str, wanted: &str) {
    for _ in 0..POLLS {
        let (status, body) = stack_status(router, case, region, stack).await;
        if status == wanted {
            return;
        }
        assert!(
            status.ends_with("_IN_PROGRESS"),
            "case {case:?}: expected the stack to be working towards {wanted}, got {status} in \
             {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = stack_status(router, case, region, stack).await;
    panic!("case {case:?}: expected the stack to reach {wanted}, got {status} in {body}");
}

/// THE RULE, over every row: the instant the stack first reports its deletion complete, the
/// application it created is gone in Elastic Beanstalk — the describe reports none, and a
/// request naming it is refused as a request naming an application that does not exist.
///
/// Elastic Beanstalk is read AFTER that first terminal observation, which is the sound
/// direction: an application that service has let go of stays gone, so one it still held when
/// asked a moment later was still there when the deletion concluded.
#[tokio::test]
async fn a_stack_deletion_completes_only_once_elastic_beanstalk_has_let_the_application_go() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("eb-removal-{index}");
        let platform = second_platform_arn(region);
        let template = (row.template)(&first_solution_stack(), &platform);

        let (status, created) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );
        // A stack whose deletion has completed is no longer addressable by name ("Deleted
        // stacks: you must specify the unique stack ID"), so every read below uses the id.
        let stack_id = opt_element(&created, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {created}"));
        wait_for(&router, case, region, &stack_id, CREATE_COMPLETE).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_id)]).await;
        let outputs = outputs_of(&described);
        let application = outputs
            .get("AppRef")
            .unwrap_or_else(|| panic!("case {case:?}: no AppRef output in {described}"))
            .clone();
        match row.declared_application {
            Some(declared) => assert_eq!(
                application, declared,
                "case {case:?}: expected the application the template named"
            ),
            None => assert!(
                !application.is_empty(),
                "case {case:?}: expected a generated application name, got {application:?}"
            ),
        }
        let environment = outputs.get("EnvRef").cloned();
        assert_eq!(
            environment.is_some(),
            row.declares_an_environment,
            "case {case:?}: expected an EnvRef output only where the template declares an \
             environment, got {described}"
        );

        // The application the stack reported as created is one Elastic Beanstalk really
        // holds — the read the rule is stated against, taken before anything is deleted.
        let (status, applications) = elasticbeanstalk(
            &router,
            region,
            "DescribeApplications",
            &[("ApplicationNames.member.1", &application)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeApplications, got {status} with body \
             {applications}"
        );
        assert_eq!(
            reported_applications(&applications),
            vec![application.clone()],
            "case {case:?}: expected Elastic Beanstalk to hold the application the stack \
             created, got {applications}"
        );

        if let Some(template_name) = row.seeded_configuration_template {
            let (status, seeded) =
                create_configuration_template(&router, region, &application, template_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateConfigurationTemplate on the live \
                 application, got {status} with body {seeded}"
            );
        }

        let (status, deleting) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleting}"
        );

        let mut settled = None;
        for _ in 0..POLLS {
            let (status, body) = stack_status(&router, case, region, &stack_id).await;
            if status == DELETE_COMPLETE {
                settled = Some(body);
                break;
            }
            assert_eq!(
                status, DELETE_IN_PROGRESS,
                "case {case:?}: expected the stack to be {DELETE_IN_PROGRESS} until Elastic \
                 Beanstalk has let {application:?} go, got {status} in {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let settled = settled.unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to reach {DELETE_COMPLETE}")
        });

        // The rule: at that first completed observation, the application is gone.
        let (status, gone) = elasticbeanstalk(
            &router,
            region,
            "DescribeApplications",
            &[("ApplicationNames.member.1", &application)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeApplications, got {status} with body \
             {gone}"
        );
        assert_eq!(
            reported_applications(&gone),
            Vec::<String>::new(),
            "case {case:?}: the stack reported {DELETE_COMPLETE} ({settled}) while Elastic \
             Beanstalk still held {application:?}, got {gone}"
        );

        // ...so the name resolves to nothing: a request naming it is refused the way that
        // service refuses a request naming an application it does not hold.
        let (status, refused) =
            create_configuration_template(&router, region, &application, "after-the-deletion")
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateConfigurationTemplate on the deleted application to \
             be refused with HTTP 400, got {status} with body {refused}"
        );
        assert_eq!(
            refusal(&refused),
            (
                "InvalidParameterValue".to_string(),
                no_application_named(&application)
            ),
            "case {case:?}: expected the refusal of a request naming no existing application, \
             got {refused}"
        );

        // The configurations that hung off the application went with it: the name one of them
        // held is refused as a name under no application, not as a name already taken.
        if let Some(template_name) = row.seeded_configuration_template {
            let (status, retried) =
                create_configuration_template(&router, region, &application, template_name).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the seeded template's name to be refused with HTTP \
                 400 once its application is gone, got {status} with body {retried}"
            );
            assert_eq!(
                refusal(&retried),
                (
                    "InvalidParameterValue".to_string(),
                    no_application_named(&application)
                ),
                "case {case:?}: expected the deleted application's own refusal, got {retried}"
            );
        }

        // And the environment the application hosted is gone too, which is what had to happen
        // before the application could go at all.
        if let Some(environment) = environment {
            let (status, environments) = elasticbeanstalk(
                &router,
                region,
                "DescribeEnvironments",
                &[("EnvironmentNames.member.1", &environment)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeEnvironments, got {status} with \
                 body {environments}"
            );
            assert_eq!(
                environment_count(&environments),
                0,
                "case {case:?}: the stack reported {DELETE_COMPLETE} while Elastic Beanstalk \
                 still held the environment {environment:?}, got {environments}"
            );
        }
    }
}
