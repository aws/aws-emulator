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
    // Amazon DynamoDB Streams, which OWNS the stream a table's `StreamSpecification` enables:
    // the registry a declared stream is minted in, and the records the table's writes publish
    // to it, live in that service's own store, so a suite that deploys a streamed table —
    // and the event source mapping the same template wires to it — needs it mounted for the
    // stream to carry anything.
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
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
    // Amazon VPC Lattice, for the `AWS::VpcLattice::ServiceNetwork` and the
    // `AWS::VpcLattice::AccessLogSubscription` a stack really creates there, so a suite that
    // deploys an access-logging estate reads the network and the subscription back through
    // that service's own wire, by the ARNs it minted.
    router.register_service(
        service_com_amazonaws_vpclattice_mercurycontrolplane::EmulatedService::new(Arc::clone(
            &engine,
        )),
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
    // The Amazon Bedrock AgentCore control plane, for the `AWS::BedrockAgentCore::*` types a
    // stack really creates there — a sandbox, a runtime, a memory, a gateway and one of its
    // targets — so a suite that deploys one reads the resource back through that service's
    // own wire, by the identifier the stack publishes.
    router.register_service(
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
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
    // AWS AppSync, for the `AWS::AppSync::Api`, its `::ChannelNamespace` and the
    // `::SourceApiAssociation` a stack really creates there, so a suite that deploys an Event
    // API or a Merged API's link reads every one of them back through that service's own wire,
    // by the ids, ARNs and endpoints the stack publishes.
    router.register_service(
        service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(
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
    // Amazon MemoryDB, for the `AWS::MemoryDB::ParameterGroup` and `AWS::MemoryDB::SubnetGroup`
    // a stack really creates there, so a suite that deploys the configuration of a MemoryDB
    // estate reads both groups back through that service's own wire, by the names and ARNs the
    // stack publishes.
    router.register_service(
        service_com_amazonaws_memorydb_amazonmemorydb::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon Connect Customer Profiles, for the `AWS::CustomerProfiles::Domain` a stack really
    // creates there, so a suite that deploys one reads the domain back through that service's
    // own wire by the name the stack publishes — and drives the data plane that domain serves.
    router.register_service(
        service_com_amazonaws_customerprofiles_customerprofiles_20200815::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Recycle Bin, for the `AWS::Rbin::Rule` a stack really creates there, so a suite that
    // deploys one reads the retention rule back through that service's own wire by the identity
    // the stack publishes.
    router.register_service(
        service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService::new(Arc::clone(&engine)),
    );
    // IAM Access Analyzer, for the `AWS::AccessAnalyzer::Analyzer` and the
    // `AWS::AccessAnalyzer::ArchiveRule` of one a stack really creates there, so a suite that
    // deploys one reads the analyzer and its archive rules back through that service's own wire
    // by the identities the stack publishes.
    router.register_service(
        service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // AWS Direct Connect, for the `AWS::DirectConnect::DirectConnectGateway` and the
    // `AWS::DirectConnect::ResiliencyGroup` a stack really creates there, so a suite that
    // deploys one reads the gateway and the group back through that service's own wire by the
    // identities the stack publishes.
    router.register_service(
        service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // AWS Amplify, for the `AWS::Amplify::App` and the `AWS::Amplify::Branch` of one a stack
    // really creates there, so a suite that deploys an Amplify hosting topology reads the app
    // and its branches back through that service's own wire, by the id and the ARN the stack
    // publishes.
    router.register_service(
        service_com_amazonaws_amplify_amplify::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon EKS, for the `AWS::EKS::Cluster` a stack really creates there and for the
    // `AWS::EKS::Addon` and `AWS::EKS::Nodegroup` it installs ON that cluster, so a suite that
    // deploys one reads the add-on and the node group back through that service's own wire, by
    // the cluster and the name the stack publishes.
    router.register_service(
        service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    // Neptune Analytics, for the `AWS::NeptuneGraph::Graph` a stack really creates there and
    // the `AWS::NeptuneGraph::GraphSnapshot` it takes OF that graph, so a suite that deploys
    // one reads the graph and the snapshot back through that service's own wire, by the ids the
    // stack publishes.
    router.register_service(
        service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon Redshift Serverless, for the `AWS::RedshiftServerless::Namespace`, the
    // `::Workgroup` of one and the `::Snapshot` of one a stack really creates there, so a suite
    // that deploys the estate reads every one of them back through that service's own wire, by
    // the names the stack publishes.
    router.register_service(
        service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon AppFlow, for the `AWS::AppFlow::ConnectorProfile` a stack really creates there, so
    // a suite that deploys one reads the profile back through that service's own wire by the name
    // the stack publishes — and the credentials secret AppFlow creates for it resolves in the
    // Secrets Manager mounted above.
    router.register_service(
        service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // CloudWatch Application Signals, for the
    // `AWS::ApplicationSignals::ServiceLevelObjective`, the
    // `AWS::ApplicationSignals::GroupingConfiguration` and the
    // `AWS::ApplicationSignals::InstrumentationConfig` a stack really creates there, so a suite
    // that deploys one reads the objective, the account's grouping attribute definitions and
    // the instrumentation point back through that service's own wire.
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // AWS Cloud Map, for the `AWS::ServiceDiscovery::HttpNamespace`,
    // `::PublicDnsNamespace`, `::PrivateDnsNamespace`, `::Service` and `::Instance` a stack
    // really creates there, so a suite that deploys a namespace's estate reads every one of
    // them back through that service's own wire, by the ids the stack publishes. Amazon
    // Route 53 comes with it because a DNS namespace IS a hosted zone of that service: the
    // `HostedZoneId` such a namespace publishes is the id of a zone Route 53 holds.
    router.register_service(
        service_com_amazonaws_servicediscovery_route53autonaming_v20170314::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    // Billing and Cost Management Dashboards, for the `AWS::BCM::Dashboard` a stack really
    // creates there and the `AWS::BCM::ScheduledReport` it schedules ON that dashboard, so a
    // suite that deploys one reads the dashboard and the report back through that service's
    // own wire, by the ARNs the stack publishes.
    router.register_service(
        service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // AWS IoT Core Device Advisor, for the `AWS::IoTCoreDeviceAdvisor::SuiteDefinition` a stack
    // really creates there, so a suite that deploys one reads the test suite back through that
    // service's own wire, by the id the stack publishes.
    router.register_service(
        service_com_amazonaws_iotdeviceadvisor_iotsenateservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // AWS Network Security Manager, for the rule, the template, the policy and the scope an
    // `AWS::NetworkSecurityManager::*` a stack declares is really created there, so a suite
    // that deploys one reads those entities back through that service's own wire, by the ARNs
    // the stack publishes.
    router.register_service(
        service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon Data Lifecycle Manager, for the `AWS::DLM::LifecyclePolicy` a stack really creates
    // there, so a suite that deploys one reads the policy back through that service's own wire,
    // by the id the stack publishes.
    router.register_service(
        service_com_amazonaws_dlm_dlm_20180112::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon Kinesis Video Streams, for the `AWS::KinesisVideo::Stream` and the
    // `AWS::KinesisVideo::SignalingChannel` a stack really creates there, so a suite that
    // deploys one reads the stream and the channel back through that service's own wire, by the
    // names the stack publishes. Its SIGNALING service comes with it, exactly as
    // `core/router/src/lib.rs` mounts the two: the registry a created channel's own data plane
    // resolves lives in that crate's store, and `CreateSignalingChannel` puts the channel it
    // made there.
    router.register_service(
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice::EmulatedService::new(
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
    call_as_in(router, region, AMBIENT_ACCESS_KEY_ID, action, members).await
}

/// The access key id every call of this harness presents unless a case names another: a key
/// neither IAM nor STS holds a record for, which is the account's own published credential.
pub const AMBIENT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// [`call`] signed with `access_key_id`, which is what says WHO the request speaks for: the
/// emulator resolves the calling principal from the credential scope, exactly as it does for a
/// client's own SDK.
///
/// The ONE place a suite of this target signs as a particular principal, so the suites that
/// decide something about the CALLER (what it may read by reference, what role it may pass)
/// cannot disagree about the envelope.
pub async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_as_in(router, REGION, access_key_id, action, members).await
}

/// [`call_as`], signed for `region`.
pub async fn call_as_in(
    router: &MultiServiceRouter,
    region: &str,
    access_key_id: &str,
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
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/cloudformation/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
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

/// How long [`settled_stack_status`] waits for a stack operation to settle before it calls
/// the stack stuck, and how often it looks. The deadline is far longer than the spans this
/// emulator gives a stack's transitions, so a loaded host cannot fail a case; what a caller
/// asserts is the CONDITION (a status that is not an operation in flight), never the time it
/// took.
pub const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
/// See [`SETTLE_DEADLINE`].
pub const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `DescribeStacks` the way a client's waiter does and answer the RESPONSE the settled
/// stack was reported by: the HTTP status and the body of the first read whose `StackStatus` is
/// not an operation in flight, and the first read this service answers with something other
/// than 200 — which is what a completed DELETION leaves behind.
///
/// The ONE waiter of this target, which the two readers below are built on: a stack's terminal
/// status is due at an INSTANT this service records, so what a caller waits for is that status
/// appearing, under [`SETTLE_DEADLINE`].
async fn settled_stack_response(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> (u16, String) {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let (code, body) =
            call_in(router, region, "DescribeStacks", &[member("StackName", stack)]).await;
        if code != 200 {
            return (code, body);
        }
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return (code, body);
        }
        if seen.last().map(String::as_str) != Some(reported.as_str()) {
            seen.push(reported);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {stack} to settle, saw {seen:?}"
        );
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
}

/// Poll `DescribeStacks` the way a client's waiter does and answer the `StackStatus` the stack
/// SETTLED on — the first status that is not an operation in flight.
///
/// The shared waiter for the suites that deploy a stack and then read what it produced: a
/// stack's terminal status is due at an INSTANT this service records, so what a caller waits
/// for is that status appearing, under [`SETTLE_DEADLINE`].
pub async fn settled_stack_status(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> String {
    let (code, body) = settled_stack_response(case, router, region, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The same wait, answered with the settled stack's whole `DescribeStacks` BODY — for a suite
/// that reads what the stack reported beside its status (its `StackStatusReason`, its outputs).
///
/// The empty string where this service no longer reports the stack at all, which is what a
/// completed deletion can leave behind.
pub async fn settled_stack(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> String {
    match settled_stack_response(case, router, region, stack).await {
        (200, body) => body,
        _ => String::new(),
    }
}

/// The quoted provider message and the reported `HandlerErrorCode` of one
/// `ResourceStatusReason`.
///
/// CloudFormation reports a failed resource's cause in one envelope — `Resource handler
/// returned message: "<the provider's message>" (RequestToken: <token>, HandlerErrorCode:
/// <class>)` — so the ONE reader of that envelope lives here and every suite that asks what a
/// provider reported splits it the same way.
pub fn quoted_and_class(case: &str, reason: &str) -> (String, String) {
    let (quoted, _, class) = envelope_parts(case, reason);
    (quoted, class)
}

/// The same split, with the `RequestToken` the envelope carries as well — for a suite that
/// asserts the token is the one this service mints per resource request ([`is_a_request_id`]),
/// rather than only what the provider said and how it was classified.
pub fn envelope_parts(case: &str, reason: &str) -> (String, String, String) {
    const OPENS: &str = "Resource handler returned message: \"";
    const TOKEN: &str = "\" (RequestToken: ";
    const CLASS: &str = ", HandlerErrorCode: ";
    let rest = reason.strip_prefix(OPENS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the reason to open with {OPENS:?}, got {reason:?}")
    });
    let (quoted, tail) = rest.split_once(TOKEN).unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted message and a RequestToken, got {reason:?}")
    });
    let (token, class) = tail
        .split_once(CLASS)
        .and_then(|(token, class)| class.strip_suffix(')').map(|class| (token, class)))
        .unwrap_or_else(|| panic!("case {case:?}: expected a HandlerErrorCode, got {reason:?}"));
    (quoted.to_string(), token.to_string(), class.to_string())
}

/// Hold one quoted exception to the diagnostic the owning service's SDK appends: the sentence
/// that service worded, then `service` as its SDK names it, the `status` it answered and the id
/// of the call — and nothing beyond the call itself.
///
/// Shared, because the shape is the SDK's rather than any one service's: a suite about another
/// service's refusals asks the same question with another name and status.
pub fn assert_sdk_diagnostic(case: &str, quoted: &str, service: &str, status: u16) {
    const OPENS: &str = " (Service: ";
    let (sentence, diagnostic) = quoted.rsplit_once(OPENS).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the SDK diagnostic after the service's own sentence, got \
             {quoted:?}"
        )
    });
    assert!(
        !sentence.trim().is_empty(),
        "case {case:?}: expected the service's own sentence ahead of the diagnostic, got \
         {quoted:?}"
    );
    let envelope = diagnostic.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the diagnostic to close, got {quoted:?}")
    });
    let fields: Vec<&str> = envelope.split(", ").collect();
    assert_eq!(
        fields.len(),
        3,
        "case {case:?}: expected the service, the status and the call's id and nothing else, \
         got {envelope:?}"
    );
    assert_eq!(
        fields[0], service,
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1],
        format!("Status Code: {status}"),
        "case {case:?}: expected the status the owning service answered, got {:?}",
        fields[1]
    );
    let request_id = fields[2]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Request ID, got {:?}", fields[2]));
    assert!(
        is_a_request_id(request_id),
        "case {case:?}: expected the id of the call in the form that service's answers carry, \
         got {request_id:?}"
    );
}

/// Hold one quoted exception to the diagnostic an SDK that COUNTS ATTEMPTS appends: the
/// envelope [`assert_sdk_diagnostic`] reads, and then how many attempts that SDK made at the
/// call — each of the FORM that SDK words rather than one recorded value.
///
/// Shared for the same reason the envelope reading is: the attempt count belongs to the SHAPE
/// an SDK words rather than to any one service, so every suite over a service whose SDK words
/// it asks this same question with another name and status.
pub fn assert_sdk_diagnostic_with_attempts(case: &str, quoted: &str, service: &str, status: u16) {
    const ATTEMPTS: &str = ") (SDK Attempt Count: ";
    let (envelope, attempts) = quoted.split_once(ATTEMPTS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the attempts the SDK made, got {quoted:?}")
    });
    assert_sdk_diagnostic(case, &format!("{envelope})"), service, status);
    let attempts = attempts.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the diagnostic to close, got {quoted:?}")
    });
    assert!(
        attempts.parse::<u32>().is_ok_and(|made| made >= 1),
        "case {case:?}: expected the attempts the SDK made, got {attempts:?}"
    );
}

/// Whether `value` has the 8-4-4-4-12 lowercase-hexadecimal form an AWS request id carries.
pub fn is_a_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|held| held.is_ascii_hexdigit() && !held.is_ascii_uppercase())
        })
}

/// How long [`failed_resource_reason`] waits for a failed resource's own event to be recorded,
/// and how often it looks again.
///
/// Generous on purpose, exactly as [`SETTLE_DEADLINE`] is: what is waited FOR is the event
/// appearing, which is on the stack's own clock, and the wait ends as soon as it is readable.
pub const EVENT_DEADLINE: Duration = Duration::from_secs(60);
/// See [`EVENT_DEADLINE`].
pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `ResourceStatusReason` `logical_id` recorded on its event in `resource_status`, or the
/// empty string where the stack has recorded none.
///
/// `StackEvent$ResourceStatusReason` is the "success/failure message associated with the
/// resource", so this is the PER-RESOURCE field a failing resource's own diagnosis — the
/// refusal the service that owns it answered — is reported on. The ONE reader of it, which
/// [`failed_resource_reason`] polls and which [`crate::stack_lifecycle::resource_event_reason`]
/// answers from, so every suite that asks reads the same field the same way.
pub async fn resource_event_reason_in(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
    resource_status: &str,
) -> String {
    let (code, body) = call_in(
        router,
        region,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| {
            xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id)
                && xml_text(entry, "ResourceStatus").as_deref() == Some(resource_status)
        })
        .and_then(|entry| xml_text(entry, "ResourceStatusReason"))
        .unwrap_or_default()
}

/// The `ResourceStatusReason` of `logical_id`'s event in `resource_status`, looked for again
/// until the stack has recorded it or [`EVENT_DEADLINE`] passes.
///
/// The shared waiter for the suites that read what a provider reported: a resource's failure is
/// recorded at an INSTANT the deployment reaches, so what a caller waits for is that event
/// appearing. A wait that runs out names every reason the stack DID record, so a case whose
/// resource failed somewhere else says where.
pub async fn failed_resource_reason(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
    resource_status: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + EVENT_DEADLINE;
    loop {
        let reason =
            resource_event_reason_in(router, region, stack, logical_id, resource_status).await;
        if !reason.is_empty() {
            return reason;
        }
        if tokio::time::Instant::now() >= deadline {
            let recorded = stack_event_reasons(router, region, stack).await;
            panic!(
                "case {case:?}: {logical_id} never recorded a {resource_status} reason; the \
                 stack recorded {recorded:?}"
            );
        }
        tokio::time::sleep(EVENT_POLL_INTERVAL).await;
    }
}

/// The `ResourceStatusReason` of every event a stack recorded — the deployment's own account
/// of itself, which is what a failed assertion names and what a case expecting a REFUSAL
/// reads the owning service's report out of.
pub async fn stack_event_reasons(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> Vec<String> {
    let (_, events) = call_in(
        router,
        region,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    xml_texts(&events, "ResourceStatusReason")
}

/// One recorded event of a stack ITSELF, as the three members its own transitions report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackOwnEvent {
    pub resource_status: String,
    pub detailed_status: String,
    pub reason: String,
}

/// The events whose `LogicalResourceId` is the stack itself, OLDEST FIRST — the order the
/// stack moved through them, which `DescribeStackEvents` reports reversed ("Returns all stack
/// related events for a specified stack in reverse chronological order").
///
/// The ONE reader of a stack's own transitions for the suites that assert the SEQUENCE they
/// were recorded in, so none of them re-spells the "filter to the stack's own logical id, then
/// un-reverse" scan. The whole stream is read in one page and the absence of a `NextToken` is
/// asserted, so a caller counting those events counts every one the stack recorded and not the
/// head of a longer log.
pub async fn stack_own_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<StackOwnEvent> {
    let (code, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "case {case:?}: expected the whole event stream in one page, got {body}"
    );
    let mut oldest_first: Vec<StackOwnEvent> = xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "LogicalResourceId").as_deref() == Some(stack))
        .map(|event| StackOwnEvent {
            resource_status: xml_text(event, "ResourceStatus").unwrap_or_default(),
            detailed_status: xml_text(event, "DetailedStatus").unwrap_or_default(),
            reason: xml_text(event, "ResourceStatusReason").unwrap_or_default(),
        })
        .collect();
    oldest_first.reverse();
    oldest_first
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

// ---------------------------------------------------------------------------
// Reading a resource back from AMAZON CLOUDFRONT, the service that owns it
// ---------------------------------------------------------------------------

/// The API version every CloudFront route is under, from the HTTP bindings its model declares
/// (`GET /2020-05-31/cache-policy/{Id}/config`).
pub const CLOUDFRONT_API_VERSION: &str = "2020-05-31";

/// The host a CloudFront request is addressed to: that service's own published host.
pub const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";

/// One CloudFront request, over the restXml wire that service speaks, answered with the HTTP
/// status and the XML body.
///
/// HERE rather than in one suite, because more than one asks it: a cache policy, a real-time
/// log configuration and the distribution that attaches them live in Amazon CloudFront, so
/// every suite that reads one back goes through that service's own wire, signed for
/// `cloudfront`.
pub async fn cloudfront(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
) -> (u16, String) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cloudfront/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{CLOUDFRONT_HOST}{path}"))
        .header("host", CLOUDFRONT_HOST)
        .header("authorization", authorization)
        .body(bytes::Bytes::new())
        .expect("a well-formed CloudFront request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// The physical resource id the stack reports for one logical id — the identifier the OWNING
/// service minted, which is what a read of that resource is addressed by.
///
/// Shared for the reason [`cloudfront`] is: every suite that reads a declared resource back
/// from its owning service needs the id the stack published for it.
pub async fn physical_resource_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResource({stack}, {logical_id}) must answer 200, \
         got {code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical resource id for {logical_id}, got {body}")
    })
}
