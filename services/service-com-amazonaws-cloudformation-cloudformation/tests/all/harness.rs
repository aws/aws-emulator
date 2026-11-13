// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate's test target drives the emulator through.
//!
//! One copy, shared by the suites declared in `tests/all.rs`: a request is built as the
//! form-encoded body an AWS SDK sends, dispatched through a router carrying only this
//! service, and answered with the HTTP status and the XML body — so what a suite asserts
//! is what a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::publisher::MarketplaceSellerEnrollment;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    ComAmazonawsCloudformationCloudformationState, STATUS_TRANSITION_DELAY,
};
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests' requests are signed for.
pub const REGION: &str = "us-east-1";

/// A router carrying only this service.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The router AND the engine behind it, for a case that has to inspect the store or seed
/// it with more records than a bound's worth of wire calls could create.
///
/// The services that OWN the resource types `state::provisioning` creates are mounted
/// beside CloudFormation on the SAME engine, exactly as `core/router/src/lib.rs` mounts
/// them: a stack's create calls those services, so a suite that deploys a template
/// declaring one of those types must have them there — Amazon SNS among them, since an
/// `AWS::SNS::Topic` a template declares is really created in SNS by the stack. (Amazon
/// Data Firehose is deliberately NOT one of them — `unprovisioned_resources.rs` is the
/// suite that pins what a stack does when an owning service cannot be reached.)
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // Amazon S3 Tables, for the `AWS::S3Tables::TableBucket` a stack really creates there:
    // that service keys a table bucket by its own ARN, which is the handle the stack
    // publishes, so a suite that deploys one reads the bucket back through S3 Tables' own
    // wire by that ARN.
    router.register_service(
        service_com_amazonaws_s3tables_s3tablebuckets::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon RDS, for the `AWS::RDS::DBInstance` and `AWS::RDS::DBSubnetGroup` a stack
    // really creates there: a suite that deploys one reads the database back through RDS's
    // own wire (`stack_lifecycle::rds`).
    router.register_service(service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // AWS Systems Manager, for the `AWS::SSM::Parameter` a stack really creates there.
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // Amazon EC2, for the `AWS::EC2::VPC` and `AWS::EC2::Subnet` a stack really creates
    // there: a suite that deploys a network reads the VPC and its subnets back through
    // EC2's own wire (`stack_lifecycle::ec2`).
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // AWS Lambda, for the `AWS::Lambda::Function` a stack really creates there — including
    // the one an `AWS::Serverless::Function` becomes — so a suite that deploys one reads the
    // function back through Lambda's own wire.
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon DynamoDB, for the `AWS::DynamoDB::Table` a stack really creates there —
    // including the one an `AWS::Serverless::SimpleTable` becomes — so a suite that deploys
    // one reads the table, and the stream its `StreamSpecification` enabled, back through
    // DynamoDB's own wire.
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon Cognito Identity, for the `AWS::Cognito::IdentityPool` and the
    // `AWS::Cognito::IdentityPoolRoleAttachment` a stack really creates there, so a suite
    // that deploys one reads the pool and its roles back through that service's own wire.
    router.register_service(
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon Cognito user pools, for the `AWS::Cognito::UserPool`, the
    // `AWS::Cognito::UserPoolClient` and the `AWS::Cognito::UserPoolIdentityProvider` a stack
    // really creates there, so a suite that deploys one reads the pool, its app client and
    // its identity provider back through that service's own wire, by the ids the stack
    // publishes.
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::
            EmulatedService::new(Arc::clone(&engine)),
    );
    // AWS Step Functions, for the `AWS::StepFunctions::StateMachine` a stack really creates
    // there — including the one an `AWS::Serverless::StateMachine` becomes — so a suite that
    // deploys one reads the machine back through that service's own wire.
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon CloudWatch Logs, for the `AWS::Logs::LogGroup`, `AWS::Logs::LogStream` and
    // `AWS::Logs::MetricFilter` a stack really creates there, so a suite that deploys one
    // reads the group, its stream and the filter on it back through that service's own wire.
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // AWS IoT, for the `AWS::IoT::Authorizer` a stack really creates there, so a suite that
    // deploys one reads the authorizer back through that service's own wire.
    router.register_service(
        service_com_amazonaws_iot_awsiotservice::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon CloudFront, for the `AWS::CloudFront::Distribution` and the
    // `AWS::CloudFront::OriginAccessControl` a stack really creates there, so a suite that
    // deploys a content-delivery estate reads the distribution and the control back through
    // that service's own wire, by the ids it minted.
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon API Gateway v2, for the `AWS::ApiGatewayV2::Api` a stack really creates there,
    // so a suite that deploys an HTTP api — and a grant that names it — reads the api back
    // through that service's own wire.
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon API Gateway, for the `AWS::ApiGateway::RestApi` a stack really creates there —
    // including the one the AWS SAM transform synthesizes — and for the resources, methods,
    // deployment and stage of it, so a suite that deploys a REST API reads the api back
    // through that service's own wire.
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon Kinesis, for the data stream an `AWS::DynamoDB::Table`'s
    // `KinesisStreamSpecification` names: DynamoDB resolves that ARN against this service's
    // own stream registry when it starts replicating, so a suite that declares one creates
    // the stream here first.
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    // AWS Secrets Manager, for the `AWS::SecretsManager::Secret` a stack really creates
    // there, so a suite that deploys one reads the secret back — and reads its value — through
    // that service's own wire.
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Application Auto Scaling, for the `AWS::ApplicationAutoScaling::ScalableTarget` and the
    // `AWS::ApplicationAutoScaling::ScalingPolicy` a stack really registers there, so a suite
    // that deploys one reads the target and the policy back through that service's own wire —
    // with Amazon CloudWatch beside it, since a target-tracking policy's alarms are created
    // there.
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon ECR, for the `AWS::ECR::Repository` a stack really creates there, so a suite
    // that deploys a registry reads the repository back through that service's own wire.
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon ECS, for the `AWS::ECS::Cluster` and the `AWS::ECS::TaskDefinition` a stack
    // really creates there: a suite that deploys a container platform reads the cluster back
    // through that service's own wire, and the task definition by the revision-qualified ARN
    // Amazon ECS allocated for it.
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon Bedrock Agent's build-time control plane, for the `AWS::Bedrock::KnowledgeBase`
    // a stack really creates there: a suite that deploys one reads the knowledge base back
    // through that service's own wire, by the id the stack publishes. (Amazon S3 Vectors is
    // deliberately NOT mounted beside it — `rolled_back_resource_records.rs` is the suite
    // that pins what a stack does when an owning service cannot be reached — so the vector
    // store a knowledge base binds to here is one that needs no second service, and the
    // S3-Vectors-backed knowledge base is
    // `core/router/tests/all/cloudformation_provisions_a_bedrock_knowledge_base.rs`.)
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon EC2 Auto Scaling, for the `AWS::AutoScaling::AutoScalingGroup` a stack really
    // creates there, so a suite that deploys a fleet reads the group — its zones, its ARN and
    // the launch template it launches from — back through that service's own wire.
    router.register_service(
        service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // AWS AppConfig, for the `AWS::AppConfig::Application`, `::Environment`,
    // `::ConfigurationProfile` and `::DeploymentStrategy` a stack really creates there, so a
    // suite that deploys a configuration estate reads every one of them back through that
    // service's own wire, by the ids the stack publishes.
    router.register_service(
        service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Elastic Load Balancing, for the `AWS::ElasticLoadBalancingV2::LoadBalancer`,
    // `::TargetGroup`, `::Listener` and `::ListenerRule` a stack really creates there, so a
    // suite that deploys a load balancer reads it — and its target group, listener and rule —
    // back through that service's own wire, by the ARNs the stack publishes.
    //
    // BOTH generations are mounted, exactly as `core/router/src/lib.rs` mounts them: the two
    // models share one endpoint and one signing name, and the CLASSIC service is the front
    // door that owns that routing key — it serves the ELBv2 model's operations for an
    // envelope naming the ELBv2 API version, which is the only way an ELBv2 call is routed
    // at all.
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// This service's store in the shared engine, as every handler reaches it.
pub fn store(
    engine: &Arc<engine::InMemoryEngine>,
) -> engine::ServiceState<ComAmazonawsCloudformationCloudformationState> {
    engine.state::<ComAmazonawsCloudformationCloudformationState>()
}

/// Say that the account these requests are signed for is an enrolled Amazon Web Services
/// Marketplace seller, which is what `RegisterPublisher` requires of the account registering.
///
/// The enrollment is made outside this API — no CloudFormation operation performs it — so a
/// suite whose cases run as a publisher says so on the store, exactly as an operator says it
/// with `state::publisher::MARKETPLACE_SELLER_ENV`. A store nobody says it of is an account
/// that never enrolled, which is what `publisher_registration.rs` holds the refusal to.
pub fn enroll_as_a_marketplace_seller(engine: &Arc<engine::InMemoryEngine>) {
    store(engine).lock().marketplace_seller = MarketplaceSellerEnrollment::enrolled();
}

/// How many hexadecimal characters an identifier its OWNING service assigns carries after
/// its prefix: an Amazon EC2 resource id's current (long) form is 17
/// (`vpc-0123456789abcdef0`).
pub const SERVICE_ID_HEX_DIGITS: usize = 17;

/// Whether `value` is an identifier of the form the service that assigns it publishes:
/// `<prefix>-` plus [`SERVICE_ID_HEX_DIGITS`] lowercase hexadecimal characters.
///
/// The ONE spelling of that check, because more than one suite asks it — of a stack's
/// outputs, of the physical ids it reports and of the ARNs built on one — and no request
/// fixes the hexadecimal part, so the rule is the FORM. `prefix` carries the ARN's head
/// where the value is an ARN that ends in such an identifier.
pub fn is_service_assigned_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|digits| {
            digits.len() == SERVICE_ID_HEX_DIGITS
                && digits
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// Wait past the instant a stack operation's terminal status becomes due.
pub async fn wait_for_the_transition() {
    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// Call one operation: `Action` plus the request's members, form-encoded exactly as an AWS
/// SDK sends them, answered with the HTTP status and the XML body.
///
/// The body is built with `form_urlencoded`, the encoder behind the `url` crate, rather
/// than a percent-encoder of this suite's own.
pub async fn call(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, REGION, action, members).await
}

/// [`call`], signed for `region`: CloudFormation is Regional, so the credential scope of
/// the request is what decides which Region's stacks it reads and writes.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", "2010-05-15");
    for (key, value) in members {
        form.append_pair(key, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cloudformation.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cloudformation/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One request member as the pair [`call`] takes.
pub fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Undo the XML text escaping the serializer applies, so a round-tripped value can be
/// compared with what was sent.
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every `<tag>` text in document order.
pub fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
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
pub fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// The tag value a described Amazon EC2 resource carries under `key`, read off the `tagSet`
/// the answer reports.
///
/// The ONE reading of an EC2 tag set in this crate's suites: an `ec2Query` answer reports a
/// tag as a `<key>`/`<value>` pair inside its `tagSet`, so the two members are read as
/// PARALLEL lists and zipped — sound exactly because the answering service reports both on
/// every entry. `None` where the resource carries no tag under that key.
pub fn ec2_tag_value(body: &str, key: &str) -> Option<String> {
    xml_texts(body, "key")
        .into_iter()
        .zip(xml_texts(body, "value"))
        .find(|(held, _)| held == key)
        .map(|(_, value)| value)
}

/// The `Code` and `Message` a refusal's envelope carries.
///
/// The ONE reading of an error envelope for the suites that assert WHICH refusal was due:
/// every awsQuery refusal of this service is one `<Error>` with those two elements, so the
/// suites that read them share this reader rather than each spelling it.
pub fn refusal(body: &str) -> (String, String) {
    (
        xml_text(body, "Code").unwrap_or_default(),
        xml_text(body, "Message").unwrap_or_default(),
    )
}

/// An instant far enough ahead that a record seeded as still running is still running when a
/// case reads it: an asynchronous conclusion is due at an INSTANT, and nothing in the service
/// sleeps for it.
///
/// Shared by the suites that seed records carrying such an instant, so one definition fixes
/// what "not due yet" means for all of them.
pub fn still_running() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3_600)
}

/// An instant already past, for a record whose conclusion the clock has already made due.
pub fn already_due() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// `CreateStack` with a template body and whatever else the case needs.
pub async fn create_stack(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackName", name), member("TemplateBody", template)];
    members.extend_from_slice(extra);
    call(router, "CreateStack", &members).await
}

/// The inner text of every non-nested `<tag …>…</tag>` BLOCK, in document order, left
/// exactly as the serializer wrote it.
///
/// [`xml_texts`] is the leaf form of this scan: it unescapes what it finds, which is right
/// for a value and wrong for a block that carries more elements. A caller reading a nested
/// list (the `<member>`s inside one `<Details>`) takes the block first and scans INSIDE it,
/// so a sibling list's members cannot be swept up with it.
///
/// The open tag may carry ATTRIBUTES — the query serializer puts an `xmlns` on the first
/// member of a list it renders — so the element NAME is matched and whatever follows it up
/// to the `>` is skipped. A longer element name that merely starts with `tag` is not that
/// element (`<Tagging>` is not a `<Tag>`), so the scan moves past it.
pub fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
            rest = &rest[start + 1..];
            continue;
        }
        let Some(open_end) = after.find('>') else { break };
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else { break };
        found.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    found
}

/// One `ListImports` read: the HTTP status, the XML body, the stack names the page reports
/// in wire order, and the `NextToken` that resumes the read just past them.
///
/// The ONE `ListImports` reader of this crate's suites — `cross_stack_imports.rs` asserts
/// WHICH stacks the import index holds and `list_imports_pages.rs` how that index is paged —
/// so neither can drift about how the response is read. The status and the body come back
/// rather than being asserted here, because a case that sends a cursor the service never
/// issued is asserting the REFUSAL.
pub async fn list_imports_page(
    router: &MultiServiceRouter,
    region: &str,
    export_name: &str,
    token: Option<&str>,
) -> (u16, String, Vec<String>, Option<String>) {
    let mut members = vec![member("ExportName", export_name)];
    if let Some(token) = token {
        members.push(member("NextToken", token));
    }
    let (status_code, body) = call_in(router, region, "ListImports", &members).await;
    // The `Imports` list is read INSIDE its own block, so a `<member>` of another list the
    // envelope carries cannot be swept up with it.
    let names = match xml_blocks(&body, "Imports").into_iter().next() {
        Some(block) => xml_texts(&block, "member"),
        None => Vec::new(),
    };
    let next_token = xml_text(&body, "NextToken");
    (status_code, body, names, next_token)
}

/// [`list_imports_page`] for a case that expects the read to SUCCEED: the page's stack names
/// and its `NextToken`.
///
/// A read ANSWERS where the export it names has importers to report; an export name the
/// Region publishes nothing under, and an export no stack imports, are each refused instead
/// (`unimported_exports.rs`), so a case that expects one of those reads the refusal through
/// [`list_imports_page`].
pub async fn list_imports_ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    export_name: &str,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status_code, body, names, next_token) =
        list_imports_page(router, region, export_name, token).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected ListImports to succeed, got {status_code} {body}"
    );
    (names, next_token)
}

/// [`list_imports_page`] for a case that expects the read to be REFUSED: the `(Code, Message)`
/// the error envelope carries.
///
/// The ONE refusal reader of this crate's suites, beside [`list_imports_ok`], so no suite
/// spells the "400, then read the code and the message" sequence itself: the status and the
/// empty page are asserted here and each suite asserts only WHICH refusal was due.
pub async fn list_imports_refusal(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    export_name: &str,
) -> (String, String) {
    let (status_code, body, names, _) = list_imports_page(router, region, export_name, None).await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected ListImports to be refused, got {status_code} {body}"
    );
    assert!(
        names.is_empty(),
        "case {case:?}: expected no importers in a refusal, got {names:?}"
    );
    (
        xml_text(&body, "Code").unwrap_or_default(),
        xml_text(&body, "Message").unwrap_or_default(),
    )
}

/// `GetTemplate` at one stage, or at the service's default when `stage` is `None`.
pub async fn get_template(
    router: &MultiServiceRouter,
    name_or_id: &str,
    stage: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", name_or_id)];
    if let Some(stage) = stage {
        members.push(member("TemplateStage", stage));
    }
    call(router, "GetTemplate", &members).await
}
