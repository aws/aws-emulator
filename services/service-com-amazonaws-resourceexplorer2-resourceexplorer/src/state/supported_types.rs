// SPDX-License-Identifier: CC0-1.0

//! The supported-type CATALOGUE Resource Explorer publishes.
//!
//! `ListSupportedResourceTypes` "retrieves a list of all resource types currently
//! supported by Amazon Web Services Resource Explorer" and its `ResourceTypes` member is
//! "the list of resource types supported by Resource Explorer"
//! (`smithy_query(query="operation",operation="ListSupportedResourceTypes")`), so what it
//! answers is service-wide METADATA: every type an index is able to hold, whatever the
//! account holds and whatever any one deployment of this emulator implements. It is
//! therefore static data of this service — nothing here is read from or written to the
//! store — and it is a SUPERSET of [`super::owners`], the far smaller set of types a
//! Region's index in this emulator actually reports resources for.
//!
//! Each row carries the three members `SupportedResourceType` declares: `ResourceType`,
//! the "unique identifier of the resource type" in Resource Explorer's
//! `<service>:<type>` form; `Service`, "the Amazon Web Services service that is
//! associated with the resource type", which is that identifier's first segment and so is
//! derived here rather than stored twice; and `CFNResourceTypes`, "the CloudFormation
//! resource type identifiers for this resource type, such as `AWS::EC2::Instance`" —
//! a LIST, because one Resource Explorer type can cover several CloudFormation types
//! (`datasync:location` is one per location flavour) and a type CloudFormation does not
//! model at all carries none.
//!
//! Source: Resource Explorer's published "Supported resource types" reference, which
//! lists the `<service>:<type>` identifier and the CloudFormation type of every type the
//! service indexes. [`CATALOGUE`] is held in ascending `ResourceType` order, which is the
//! order the operation publishes it in; [`is_sorted`] is the invariant that says so.

/// One row of the published catalogue: a resource type Resource Explorer supports.
#[derive(Debug, Clone, Copy)]
pub struct CatalogueEntry {
    /// `SupportedResourceType$ResourceType`: the `<service>:<type>` identifier.
    pub resource_type: &'static str,
    /// `SupportedResourceType$CFNResourceTypes`: the CloudFormation type identifiers
    /// this type is registered under, which is empty for a type CloudFormation does not
    /// model.
    pub cfn_resource_types: &'static [&'static str],
}

impl CatalogueEntry {
    /// `SupportedResourceType$Service` for this row.
    pub fn service(&self) -> &'static str {
        service_of(self.resource_type)
    }
}

/// The service namespace a `<service>:<type>` resource-type identifier names: everything
/// before its first colon.
///
/// The ONE place that split is made, so the `Service` the catalogue publishes and the
/// `Service` an indexed resource is reported with cannot disagree.
pub fn service_of(resource_type: &str) -> &str {
    resource_type
        .split_once(':')
        .map_or(resource_type, |(service, _)| service)
}

/// Whether `entries` are in the ascending `ResourceType` order this catalogue is
/// published in, with no identifier listed twice.
///
/// A caller may diff or binary-search the catalogue, so the order is part of the answer
/// rather than an accident of how the rows are written down.
pub fn is_sorted(entries: &[CatalogueEntry]) -> bool {
    entries
        .windows(2)
        .all(|pair| pair[0].resource_type < pair[1].resource_type)
}

/// One catalogue row, written out once per supported type below.
const fn entry(
    resource_type: &'static str,
    cfn_resource_types: &'static [&'static str],
) -> CatalogueEntry {
    CatalogueEntry {
        resource_type,
        cfn_resource_types,
    }
}

/// Resource Explorer's supported-type catalogue, ascending by `ResourceType`.
pub const CATALOGUE: &[CatalogueEntry] = &[
    entry("access-analyzer:analyzer", &["AWS::AccessAnalyzer::Analyzer"]),
    entry(
        "acm-pca:certificate-authority",
        &["AWS::ACMPCA::CertificateAuthority"],
    ),
    entry("acm:certificate", &["AWS::CertificateManager::Certificate"]),
    entry("airflow:environment", &["AWS::MWAA::Environment"]),
    entry("amplify:apps", &["AWS::Amplify::App"]),
    entry("amplify:apps/branches", &["AWS::Amplify::Branch"]),
    entry("amplify:apps/domains", &["AWS::Amplify::Domain"]),
    entry("aoss:collection", &["AWS::OpenSearchServerless::Collection"]),
    entry("apigateway:apis", &["AWS::ApiGatewayV2::Api"]),
    entry(
        "apigateway:apis/integrations",
        &["AWS::ApiGatewayV2::Integration"],
    ),
    entry("apigateway:apis/routes", &["AWS::ApiGatewayV2::Route"]),
    entry("apigateway:apis/stages", &["AWS::ApiGatewayV2::Stage"]),
    entry("apigateway:restapis", &["AWS::ApiGateway::RestApi"]),
    entry(
        "apigateway:restapis/deployments",
        &["AWS::ApiGateway::Deployment"],
    ),
    entry("apigateway:restapis/models", &["AWS::ApiGateway::Model"]),
    entry(
        "apigateway:restapis/resources",
        &["AWS::ApiGateway::Resource"],
    ),
    entry("apigateway:restapis/stages", &["AWS::ApiGateway::Stage"]),
    entry("appconfig:application", &["AWS::AppConfig::Application"]),
    entry(
        "appconfig:configurationprofile",
        &["AWS::AppConfig::ConfigurationProfile"],
    ),
    entry(
        "appconfig:deploymentstrategy",
        &["AWS::AppConfig::DeploymentStrategy"],
    ),
    entry("appconfig:environment", &["AWS::AppConfig::Environment"]),
    entry("appconfig:extension", &["AWS::AppConfig::Extension"]),
    entry("appflow:connectorprofile", &["AWS::AppFlow::ConnectorProfile"]),
    entry("appflow:flow", &["AWS::AppFlow::Flow"]),
    entry("appmesh:mesh", &["AWS::AppMesh::Mesh"]),
    entry(
        "appmesh:mesh/virtualGateway",
        &["AWS::AppMesh::VirtualGateway"],
    ),
    entry("appmesh:mesh/virtualNode", &["AWS::AppMesh::VirtualNode"]),
    entry("appmesh:mesh/virtualRouter", &["AWS::AppMesh::VirtualRouter"]),
    entry(
        "appmesh:mesh/virtualService",
        &["AWS::AppMesh::VirtualService"],
    ),
    entry(
        "apprunner:autoscalingconfiguration",
        &["AWS::AppRunner::AutoScalingConfiguration"],
    ),
    entry(
        "apprunner:observabilityconfiguration",
        &["AWS::AppRunner::ObservabilityConfiguration"],
    ),
    entry("apprunner:service", &["AWS::AppRunner::Service"]),
    entry("apprunner:vpcconnector", &["AWS::AppRunner::VpcConnector"]),
    entry("appstream:fleet", &["AWS::AppStream::Fleet"]),
    entry("appstream:image-builder", &["AWS::AppStream::ImageBuilder"]),
    entry("appstream:stack", &["AWS::AppStream::Stack"]),
    entry("appsync:apis", &["AWS::AppSync::GraphQLApi"]),
    entry("aps:rulegroupsnamespace", &["AWS::APS::RuleGroupsNamespace"]),
    entry("aps:workspace", &["AWS::APS::Workspace"]),
    entry(
        "athena:capacity-reservation",
        &["AWS::Athena::CapacityReservation"],
    ),
    entry("athena:datacatalog", &["AWS::Athena::DataCatalog"]),
    entry("athena:workgroup", &["AWS::Athena::WorkGroup"]),
    entry("auditmanager:assessment", &["AWS::AuditManager::Assessment"]),
    entry(
        "autoscaling:autoScalingGroup",
        &["AWS::AutoScaling::AutoScalingGroup"],
    ),
    entry(
        "autoscaling:launchConfiguration",
        &["AWS::AutoScaling::LaunchConfiguration"],
    ),
    entry("backup:backup-plan", &["AWS::Backup::BackupPlan"]),
    entry("backup:backup-vault", &["AWS::Backup::BackupVault"]),
    entry("backup:framework", &["AWS::Backup::Framework"]),
    entry("backup:report-plan", &["AWS::Backup::ReportPlan"]),
    entry(
        "batch:compute-environment",
        &["AWS::Batch::ComputeEnvironment"],
    ),
    entry("batch:job-definition", &["AWS::Batch::JobDefinition"]),
    entry("batch:job-queue", &["AWS::Batch::JobQueue"]),
    entry("batch:scheduling-policy", &["AWS::Batch::SchedulingPolicy"]),
    entry("budgets:budget", &["AWS::Budgets::Budget"]),
    entry("cassandra:keyspace", &["AWS::Cassandra::Keyspace"]),
    entry("cassandra:table", &["AWS::Cassandra::Table"]),
    entry("ce:anomalymonitor", &["AWS::CE::AnomalyMonitor"]),
    entry("ce:anomalysubscription", &["AWS::CE::AnomalySubscription"]),
    entry("ce:costcategory", &["AWS::CE::CostCategory"]),
    entry("cloud9:environment", &["AWS::Cloud9::EnvironmentEC2"]),
    entry("cloudformation:stack", &["AWS::CloudFormation::Stack"]),
    entry("cloudformation:stackset", &["AWS::CloudFormation::StackSet"]),
    entry("cloudfront:cache-policy", &["AWS::CloudFront::CachePolicy"]),
    entry("cloudfront:distribution", &["AWS::CloudFront::Distribution"]),
    entry("cloudfront:function", &["AWS::CloudFront::Function"]),
    entry("cloudfront:key-group", &["AWS::CloudFront::KeyGroup"]),
    entry(
        "cloudfront:origin-access-identity",
        &["AWS::CloudFront::CloudFrontOriginAccessIdentity"],
    ),
    entry(
        "cloudfront:origin-request-policy",
        &["AWS::CloudFront::OriginRequestPolicy"],
    ),
    entry("cloudfront:public-key", &["AWS::CloudFront::PublicKey"]),
    entry(
        "cloudfront:realtime-log-config",
        &["AWS::CloudFront::RealtimeLogConfig"],
    ),
    entry(
        "cloudfront:response-headers-policy",
        &["AWS::CloudFront::ResponseHeadersPolicy"],
    ),
    entry(
        "cloudfront:streaming-distribution",
        &["AWS::CloudFront::StreamingDistribution"],
    ),
    entry("cloudtrail:channel", &["AWS::CloudTrail::Channel"]),
    entry(
        "cloudtrail:eventdatastore",
        &["AWS::CloudTrail::EventDataStore"],
    ),
    entry("cloudtrail:trail", &["AWS::CloudTrail::Trail"]),
    entry("cloudwatch:alarm", &["AWS::CloudWatch::Alarm"]),
    entry("cloudwatch:dashboard", &["AWS::CloudWatch::Dashboard"]),
    entry("cloudwatch:insight-rule", &["AWS::CloudWatch::InsightRule"]),
    entry("cloudwatch:metric-stream", &["AWS::CloudWatch::MetricStream"]),
    entry("codeartifact:domain", &["AWS::CodeArtifact::Domain"]),
    entry("codeartifact:repository", &["AWS::CodeArtifact::Repository"]),
    entry("codebuild:project", &["AWS::CodeBuild::Project"]),
    entry("codebuild:report-group", &["AWS::CodeBuild::ReportGroup"]),
    entry("codecommit:repository", &["AWS::CodeCommit::Repository"]),
    entry("codedeploy:application", &["AWS::CodeDeploy::Application"]),
    entry(
        "codedeploy:deploymentconfig",
        &["AWS::CodeDeploy::DeploymentConfig"],
    ),
    entry(
        "codedeploy:deploymentgroup",
        &["AWS::CodeDeploy::DeploymentGroup"],
    ),
    entry(
        "codeguru-profiler:profilingGroup",
        &["AWS::CodeGuruProfiler::ProfilingGroup"],
    ),
    entry(
        "codeguru-reviewer:association",
        &["AWS::CodeGuruReviewer::RepositoryAssociation"],
    ),
    entry("codepipeline:pipeline", &["AWS::CodePipeline::Pipeline"]),
    entry("codepipeline:webhook", &["AWS::CodePipeline::Webhook"]),
    entry(
        "codestar-connections:connection",
        &["AWS::CodeStarConnections::Connection"],
    ),
    entry("cognito-identity:identitypool", &["AWS::Cognito::IdentityPool"]),
    entry("cognito-idp:userpool", &["AWS::Cognito::UserPool"]),
    entry(
        "comprehend:document-classifier",
        &["AWS::Comprehend::DocumentClassifier"],
    ),
    entry("comprehend:flywheel", &["AWS::Comprehend::Flywheel"]),
    entry("config:config-rule", &["AWS::Config::ConfigRule"]),
    entry("config:conformance-pack", &["AWS::Config::ConformancePack"]),
    entry(
        "config:organization-config-rule",
        &["AWS::Config::OrganizationConfigRule"],
    ),
    entry(
        "config:organization-conformance-pack",
        &["AWS::Config::OrganizationConformancePack"],
    ),
    entry("connect:instance", &["AWS::Connect::Instance"]),
    entry("databrew:dataset", &["AWS::DataBrew::Dataset"]),
    entry("databrew:job", &["AWS::DataBrew::Job"]),
    entry("databrew:project", &["AWS::DataBrew::Project"]),
    entry("databrew:recipe", &["AWS::DataBrew::Recipe"]),
    entry("databrew:ruleset", &["AWS::DataBrew::Ruleset"]),
    entry("databrew:schedule", &["AWS::DataBrew::Schedule"]),
    entry("dataexchange:data-sets", &["AWS::DataExchange::DataSet"]),
    entry("datapipeline:pipeline", &["AWS::DataPipeline::Pipeline"]),
    entry("datasync:agent", &["AWS::DataSync::Agent"]),
    // One Resource Explorer type, one CloudFormation type per location flavour.
    entry(
        "datasync:location",
        &[
            "AWS::DataSync::LocationEFS",
            "AWS::DataSync::LocationFSxLustre",
            "AWS::DataSync::LocationFSxONTAP",
            "AWS::DataSync::LocationFSxOpenZFS",
            "AWS::DataSync::LocationFSxWindows",
            "AWS::DataSync::LocationHDFS",
            "AWS::DataSync::LocationNFS",
            "AWS::DataSync::LocationObjectStorage",
            "AWS::DataSync::LocationS3",
            "AWS::DataSync::LocationSMB",
        ],
    ),
    entry("datasync:task", &["AWS::DataSync::Task"]),
    entry("dax:cache", &["AWS::DAX::Cluster"]),
    entry("detective:graph", &["AWS::Detective::Graph"]),
    entry("devicefarm:project", &["AWS::DeviceFarm::Project"]),
    entry(
        "devicefarm:testgrid-project",
        &["AWS::DeviceFarm::TestGridProject"],
    ),
    entry("dlm:policy", &["AWS::DLM::LifecyclePolicy"]),
    entry("dms:endpoint", &["AWS::DMS::Endpoint"]),
    entry("dms:rep", &["AWS::DMS::ReplicationInstance"]),
    entry("dms:subgrp", &["AWS::DMS::ReplicationSubnetGroup"]),
    entry("dms:task", &["AWS::DMS::ReplicationTask"]),
    entry("docdb-elastic:cluster", &["AWS::DocDBElastic::Cluster"]),
    entry(
        "ds:directory",
        &[
            "AWS::DirectoryService::MicrosoftAD",
            "AWS::DirectoryService::SimpleAD",
        ],
    ),
    entry("dynamodb:table", &["AWS::DynamoDB::Table"]),
    entry(
        "ec2:capacity-reservation",
        &["AWS::EC2::CapacityReservation"],
    ),
    entry("ec2:carrier-gateway", &["AWS::EC2::CarrierGateway"]),
    entry("ec2:client-vpn-endpoint", &["AWS::EC2::ClientVpnEndpoint"]),
    entry("ec2:customer-gateway", &["AWS::EC2::CustomerGateway"]),
    entry("ec2:dedicated-host", &["AWS::EC2::Host"]),
    entry("ec2:dhcp-options", &["AWS::EC2::DHCPOptions"]),
    entry(
        "ec2:egress-only-internet-gateway",
        &["AWS::EC2::EgressOnlyInternetGateway"],
    ),
    entry("ec2:elastic-ip", &["AWS::EC2::EIP"]),
    entry("ec2:fleet", &["AWS::EC2::EC2Fleet"]),
    // An AMI is indexed, and CloudFormation models no resource for one.
    entry("ec2:image", &[]),
    entry("ec2:instance", &["AWS::EC2::Instance"]),
    entry(
        "ec2:instance-event-window",
        &["AWS::EC2::InstanceEventWindow"],
    ),
    entry("ec2:internet-gateway", &["AWS::EC2::InternetGateway"]),
    entry("ec2:ipam", &["AWS::EC2::IPAM"]),
    entry("ec2:ipam-pool", &["AWS::EC2::IPAMPool"]),
    entry("ec2:ipam-scope", &["AWS::EC2::IPAMScope"]),
    entry("ec2:key-pair", &["AWS::EC2::KeyPair"]),
    entry("ec2:launch-template", &["AWS::EC2::LaunchTemplate"]),
    entry("ec2:natgateway", &["AWS::EC2::NatGateway"]),
    entry("ec2:network-acl", &["AWS::EC2::NetworkAcl"]),
    entry(
        "ec2:network-insights-access-scope",
        &["AWS::EC2::NetworkInsightsAccessScope"],
    ),
    entry(
        "ec2:network-insights-analysis",
        &["AWS::EC2::NetworkInsightsAnalysis"],
    ),
    entry(
        "ec2:network-insights-path",
        &["AWS::EC2::NetworkInsightsPath"],
    ),
    entry("ec2:network-interface", &["AWS::EC2::NetworkInterface"]),
    entry("ec2:placement-group", &["AWS::EC2::PlacementGroup"]),
    entry("ec2:prefix-list", &["AWS::EC2::PrefixList"]),
    entry("ec2:route-table", &["AWS::EC2::RouteTable"]),
    entry("ec2:security-group", &["AWS::EC2::SecurityGroup"]),
    entry(
        "ec2:security-group-rule",
        &[
            "AWS::EC2::SecurityGroupEgress",
            "AWS::EC2::SecurityGroupIngress",
        ],
    ),
    // An EBS snapshot is indexed, and CloudFormation models no resource for one.
    entry("ec2:snapshot", &[]),
    entry("ec2:subnet", &["AWS::EC2::Subnet"]),
    entry(
        "ec2:traffic-mirror-filter",
        &["AWS::EC2::TrafficMirrorFilter"],
    ),
    entry(
        "ec2:traffic-mirror-session",
        &["AWS::EC2::TrafficMirrorSession"],
    ),
    entry(
        "ec2:traffic-mirror-target",
        &["AWS::EC2::TrafficMirrorTarget"],
    ),
    entry("ec2:transit-gateway", &["AWS::EC2::TransitGateway"]),
    entry(
        "ec2:transit-gateway-attachment",
        &["AWS::EC2::TransitGatewayAttachment"],
    ),
    entry(
        "ec2:transit-gateway-multicast-domain",
        &["AWS::EC2::TransitGatewayMulticastDomain"],
    ),
    entry(
        "ec2:transit-gateway-route-table",
        &["AWS::EC2::TransitGatewayRouteTable"],
    ),
    entry("ec2:volume", &["AWS::EC2::Volume"]),
    entry("ec2:vpc", &["AWS::EC2::VPC"]),
    entry("ec2:vpc-endpoint", &["AWS::EC2::VPCEndpoint"]),
    entry(
        "ec2:vpc-endpoint-service",
        &["AWS::EC2::VPCEndpointService"],
    ),
    entry("ec2:vpc-flow-log", &["AWS::EC2::FlowLog"]),
    entry(
        "ec2:vpc-peering-connection",
        &["AWS::EC2::VPCPeeringConnection"],
    ),
    entry("ec2:vpn-connection", &["AWS::EC2::VPNConnection"]),
    entry("ec2:vpn-gateway", &["AWS::EC2::VPNGateway"]),
    entry("ecr-public:repository", &["AWS::ECR::PublicRepository"]),
    entry("ecr:repository", &["AWS::ECR::Repository"]),
    entry("ecs:capacity-provider", &["AWS::ECS::CapacityProvider"]),
    entry("ecs:cluster", &["AWS::ECS::Cluster"]),
    entry("ecs:service", &["AWS::ECS::Service"]),
    entry("ecs:task-definition", &["AWS::ECS::TaskDefinition"]),
    entry("ecs:task-set", &["AWS::ECS::TaskSet"]),
    entry("eks:addon", &["AWS::EKS::Addon"]),
    entry("eks:cluster", &["AWS::EKS::Cluster"]),
    entry("eks:fargateprofile", &["AWS::EKS::FargateProfile"]),
    entry(
        "eks:identityproviderconfig",
        &["AWS::EKS::IdentityProviderConfig"],
    ),
    entry("eks:nodegroup", &["AWS::EKS::Nodegroup"]),
    entry("elasticache:cluster", &["AWS::ElastiCache::CacheCluster"]),
    entry(
        "elasticache:globalreplicationgroup",
        &["AWS::ElastiCache::GlobalReplicationGroup"],
    ),
    entry(
        "elasticache:parametergroup",
        &["AWS::ElastiCache::ParameterGroup"],
    ),
    entry(
        "elasticache:replicationgroup",
        &["AWS::ElastiCache::ReplicationGroup"],
    ),
    entry(
        "elasticache:serverlesscache",
        &["AWS::ElastiCache::ServerlessCache"],
    ),
    entry("elasticache:subnetgroup", &["AWS::ElastiCache::SubnetGroup"]),
    entry("elasticache:user", &["AWS::ElastiCache::User"]),
    entry("elasticache:usergroup", &["AWS::ElastiCache::UserGroup"]),
    entry(
        "elasticbeanstalk:application",
        &["AWS::ElasticBeanstalk::Application"],
    ),
    entry(
        "elasticbeanstalk:applicationversion",
        &["AWS::ElasticBeanstalk::ApplicationVersion"],
    ),
    entry(
        "elasticbeanstalk:configurationtemplate",
        &["AWS::ElasticBeanstalk::ConfigurationTemplate"],
    ),
    entry(
        "elasticbeanstalk:environment",
        &["AWS::ElasticBeanstalk::Environment"],
    ),
    entry("elasticfilesystem:access-point", &["AWS::EFS::AccessPoint"]),
    entry("elasticfilesystem:file-system", &["AWS::EFS::FileSystem"]),
    entry(
        "elasticloadbalancing:listener",
        &["AWS::ElasticLoadBalancingV2::Listener"],
    ),
    entry(
        "elasticloadbalancing:listener-rule",
        &["AWS::ElasticLoadBalancingV2::ListenerRule"],
    ),
    entry(
        "elasticloadbalancing:loadbalancer",
        &[
            "AWS::ElasticLoadBalancing::LoadBalancer",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
        ],
    ),
    entry(
        "elasticloadbalancing:targetgroup",
        &["AWS::ElasticLoadBalancingV2::TargetGroup"],
    ),
    entry("elasticmapreduce:cluster", &["AWS::EMR::Cluster"]),
    entry("elasticmapreduce:studio", &["AWS::EMR::Studio"]),
    entry(
        "es:domain",
        &[
            "AWS::Elasticsearch::Domain",
            "AWS::OpenSearchService::Domain",
        ],
    ),
    entry("events:api-destination", &["AWS::Events::ApiDestination"]),
    entry("events:archive", &["AWS::Events::Archive"]),
    entry("events:connection", &["AWS::Events::Connection"]),
    entry("events:endpoint", &["AWS::Events::Endpoint"]),
    entry("events:event-bus", &["AWS::Events::EventBus"]),
    entry("events:rule", &["AWS::Events::Rule"]),
    entry("evidently:project", &["AWS::Evidently::Project"]),
    entry(
        "firehose:deliverystream",
        &["AWS::KinesisFirehose::DeliveryStream"],
    ),
    entry("fis:experiment-template", &["AWS::FIS::ExperimentTemplate"]),
    entry("forecast:dataset", &["AWS::Forecast::Dataset"]),
    entry("forecast:dataset-group", &["AWS::Forecast::DatasetGroup"]),
    entry("fsx:file-system", &["AWS::FSx::FileSystem"]),
    entry("fsx:snapshot", &["AWS::FSx::Snapshot"]),
    entry(
        "fsx:storage-virtual-machine",
        &["AWS::FSx::StorageVirtualMachine"],
    ),
    entry("fsx:volume", &["AWS::FSx::Volume"]),
    entry("gamelift:alias", &["AWS::GameLift::Alias"]),
    entry("gamelift:build", &["AWS::GameLift::Build"]),
    entry("gamelift:fleet", &["AWS::GameLift::Fleet"]),
    entry(
        "gamelift:gamesessionqueue",
        &["AWS::GameLift::GameSessionQueue"],
    ),
    entry(
        "gamelift:matchmakingconfiguration",
        &["AWS::GameLift::MatchmakingConfiguration"],
    ),
    entry(
        "gamelift:matchmakingruleset",
        &["AWS::GameLift::MatchmakingRuleSet"],
    ),
    entry("gamelift:script", &["AWS::GameLift::Script"]),
    entry(
        "geo:geofence-collection",
        &["AWS::Location::GeofenceCollection"],
    ),
    entry("geo:map", &["AWS::Location::Map"]),
    entry("geo:place-index", &["AWS::Location::PlaceIndex"]),
    entry("geo:route-calculator", &["AWS::Location::RouteCalculator"]),
    entry("geo:tracker", &["AWS::Location::Tracker"]),
    entry("glacier:vaults", &["AWS::Glacier::Vault"]),
    entry(
        "globalaccelerator:accelerator",
        &["AWS::GlobalAccelerator::Accelerator"],
    ),
    entry(
        "globalaccelerator:accelerator/listener",
        &["AWS::GlobalAccelerator::Listener"],
    ),
    entry(
        "globalaccelerator:accelerator/listener/endpoint-group",
        &["AWS::GlobalAccelerator::EndpointGroup"],
    ),
    entry("glue:database", &["AWS::Glue::Database"]),
    entry("glue:devEndpoint", &["AWS::Glue::DevEndpoint"]),
    entry("glue:job", &["AWS::Glue::Job"]),
    entry("glue:mlTransform", &["AWS::Glue::MLTransform"]),
    entry("glue:registry", &["AWS::Glue::Registry"]),
    entry("glue:schema", &["AWS::Glue::Schema"]),
    entry("glue:table", &["AWS::Glue::Table"]),
    entry("glue:trigger", &["AWS::Glue::Trigger"]),
    entry("glue:workflow", &["AWS::Glue::Workflow"]),
    entry("grafana:workspace", &["AWS::Grafana::Workspace"]),
    entry(
        "greengrass:components",
        &["AWS::GreengrassV2::ComponentVersion"],
    ),
    entry("greengrass:deployments", &["AWS::GreengrassV2::Deployment"]),
    entry("greengrass:groups", &["AWS::Greengrass::Group"]),
    entry("groundstation:config", &["AWS::GroundStation::Config"]),
    entry(
        "groundstation:dataflow-endpoint-group",
        &["AWS::GroundStation::DataflowEndpointGroup"],
    ),
    entry(
        "groundstation:mission-profile",
        &["AWS::GroundStation::MissionProfile"],
    ),
    entry("guardduty:detector", &["AWS::GuardDuty::Detector"]),
    entry("guardduty:detector/filter", &["AWS::GuardDuty::Filter"]),
    entry("guardduty:detector/ipset", &["AWS::GuardDuty::IPSet"]),
    entry(
        "guardduty:detector/threatintelset",
        &["AWS::GuardDuty::ThreatIntelSet"],
    ),
    entry("healthlake:datastore", &["AWS::HealthLake::FHIRDatastore"]),
    entry("iam:group", &["AWS::IAM::Group"]),
    entry("iam:instance-profile", &["AWS::IAM::InstanceProfile"]),
    entry("iam:oidc-provider", &["AWS::IAM::OIDCProvider"]),
    entry("iam:policy", &["AWS::IAM::ManagedPolicy"]),
    entry("iam:role", &["AWS::IAM::Role"]),
    entry("iam:saml-provider", &["AWS::IAM::SAMLProvider"]),
    entry("iam:server-certificate", &["AWS::IAM::ServerCertificate"]),
    entry("iam:user", &["AWS::IAM::User"]),
    entry("imagebuilder:component", &["AWS::ImageBuilder::Component"]),
    entry(
        "imagebuilder:container-recipe",
        &["AWS::ImageBuilder::ContainerRecipe"],
    ),
    entry(
        "imagebuilder:distribution-configuration",
        &["AWS::ImageBuilder::DistributionConfiguration"],
    ),
    entry("imagebuilder:image", &["AWS::ImageBuilder::Image"]),
    entry(
        "imagebuilder:image-pipeline",
        &["AWS::ImageBuilder::ImagePipeline"],
    ),
    entry(
        "imagebuilder:image-recipe",
        &["AWS::ImageBuilder::ImageRecipe"],
    ),
    entry(
        "imagebuilder:infrastructure-configuration",
        &["AWS::ImageBuilder::InfrastructureConfiguration"],
    ),
    entry("iot:authorizer", &["AWS::IoT::Authorizer"]),
    entry("iot:billinggroup", &["AWS::IoT::BillingGroup"]),
    entry("iot:cacert", &["AWS::IoT::CACertificate"]),
    entry("iot:cert", &["AWS::IoT::Certificate"]),
    entry("iot:custommetric", &["AWS::IoT::CustomMetric"]),
    entry("iot:dimension", &["AWS::IoT::Dimension"]),
    entry("iot:domainconfiguration", &["AWS::IoT::DomainConfiguration"]),
    entry("iot:fleetmetric", &["AWS::IoT::FleetMetric"]),
    entry("iot:jobtemplate", &["AWS::IoT::JobTemplate"]),
    entry("iot:mitigationaction", &["AWS::IoT::MitigationAction"]),
    entry("iot:policy", &["AWS::IoT::Policy"]),
    entry(
        "iot:provisioningtemplate",
        &["AWS::IoT::ProvisioningTemplate"],
    ),
    entry("iot:rolealias", &["AWS::IoT::RoleAlias"]),
    entry("iot:rule", &["AWS::IoT::TopicRule"]),
    entry("iot:scheduledaudit", &["AWS::IoT::ScheduledAudit"]),
    entry("iot:securityprofile", &["AWS::IoT::SecurityProfile"]),
    entry("iot:thing", &["AWS::IoT::Thing"]),
    entry("iot:thinggroup", &["AWS::IoT::ThingGroup"]),
    entry("iot:thingtype", &["AWS::IoT::ThingType"]),
    entry(
        "iot:topicruledestination",
        &["AWS::IoT::TopicRuleDestination"],
    ),
    entry("iotanalytics:channel", &["AWS::IoTAnalytics::Channel"]),
    entry("iotanalytics:dataset", &["AWS::IoTAnalytics::Dataset"]),
    entry("iotanalytics:datastore", &["AWS::IoTAnalytics::Datastore"]),
    entry("iotanalytics:pipeline", &["AWS::IoTAnalytics::Pipeline"]),
    entry("iotevents:alarmModel", &["AWS::IoTEvents::AlarmModel"]),
    entry("iotevents:detectorModel", &["AWS::IoTEvents::DetectorModel"]),
    entry("iotevents:input", &["AWS::IoTEvents::Input"]),
    entry("iotsitewise:asset", &["AWS::IoTSiteWise::Asset"]),
    entry("iotsitewise:asset-model", &["AWS::IoTSiteWise::AssetModel"]),
    entry("iotsitewise:dashboard", &["AWS::IoTSiteWise::Dashboard"]),
    entry("iotsitewise:gateway", &["AWS::IoTSiteWise::Gateway"]),
    entry("iotsitewise:portal", &["AWS::IoTSiteWise::Portal"]),
    entry("iotsitewise:project", &["AWS::IoTSiteWise::Project"]),
    entry("iottwinmaker:workspace", &["AWS::IoTTwinMaker::Workspace"]),
    entry("ivs:channel", &["AWS::IVS::Channel"]),
    entry("ivs:playback-key-pair", &["AWS::IVS::PlaybackKeyPair"]),
    entry(
        "ivs:recording-configuration",
        &["AWS::IVS::RecordingConfiguration"],
    ),
    entry("ivs:stream-key", &["AWS::IVS::StreamKey"]),
    entry("kafka:cluster", &["AWS::MSK::Cluster"]),
    entry("kafka:configuration", &["AWS::MSK::Configuration"]),
    entry("kendra:index", &["AWS::Kendra::Index"]),
    entry("kinesis:stream", &["AWS::Kinesis::Stream"]),
    entry(
        "kinesisanalytics:application",
        &["AWS::KinesisAnalyticsV2::Application"],
    ),
    entry("kinesisvideo:channel", &["AWS::KinesisVideo::SignalingChannel"]),
    entry("kinesisvideo:stream", &["AWS::KinesisVideo::Stream"]),
    entry("kms:alias", &["AWS::KMS::Alias"]),
    entry("kms:key", &["AWS::KMS::Key"]),
    entry(
        "lambda:code-signing-config",
        &["AWS::Lambda::CodeSigningConfig"],
    ),
    entry(
        "lambda:event-source-mapping",
        &["AWS::Lambda::EventSourceMapping"],
    ),
    entry("lambda:function", &["AWS::Lambda::Function"]),
    entry("lambda:layer", &["AWS::Lambda::LayerVersion"]),
    entry("lex:bot", &["AWS::Lex::Bot"]),
    entry("lex:bot-alias", &["AWS::Lex::BotAlias"]),
    entry(
        "license-manager:license-configuration",
        &["AWS::LicenseManager::LicenseConfiguration"],
    ),
    entry("logs:destination", &["AWS::Logs::Destination"]),
    entry("logs:log-group", &["AWS::Logs::LogGroup"]),
    entry("m2:application", &["AWS::M2::Application"]),
    entry("m2:environment", &["AWS::M2::Environment"]),
    entry("macie2:allow-list", &["AWS::Macie::AllowList"]),
    entry(
        "macie2:custom-data-identifier",
        &["AWS::Macie::CustomDataIdentifier"],
    ),
    entry("macie2:findings-filter", &["AWS::Macie::FindingsFilter"]),
    entry("mediaconnect:flow", &["AWS::MediaConnect::Flow"]),
    entry("mediaconvert:jobTemplates", &["AWS::MediaConvert::JobTemplate"]),
    entry("mediaconvert:presets", &["AWS::MediaConvert::Preset"]),
    entry("mediaconvert:queues", &["AWS::MediaConvert::Queue"]),
    entry("medialive:channel", &["AWS::MediaLive::Channel"]),
    entry("medialive:input", &["AWS::MediaLive::Input"]),
    entry(
        "medialive:inputSecurityGroup",
        &["AWS::MediaLive::InputSecurityGroup"],
    ),
    entry("mediapackage-vod:assets", &["AWS::MediaPackage::Asset"]),
    entry(
        "mediapackage-vod:packaging_configurations",
        &["AWS::MediaPackage::PackagingConfiguration"],
    ),
    entry(
        "mediapackage-vod:packaging_groups",
        &["AWS::MediaPackage::PackagingGroup"],
    ),
    entry("mediapackage:channels", &["AWS::MediaPackage::Channel"]),
    entry(
        "mediapackage:origin_endpoints",
        &["AWS::MediaPackage::OriginEndpoint"],
    ),
    entry("mediastore:container", &["AWS::MediaStore::Container"]),
    entry(
        "mediatailor:playbackConfiguration",
        &["AWS::MediaTailor::PlaybackConfiguration"],
    ),
    entry("memorydb:acl", &["AWS::MemoryDB::ACL"]),
    entry("memorydb:cluster", &["AWS::MemoryDB::Cluster"]),
    entry("memorydb:parametergroup", &["AWS::MemoryDB::ParameterGroup"]),
    entry("memorydb:subnetgroup", &["AWS::MemoryDB::SubnetGroup"]),
    entry("memorydb:user", &["AWS::MemoryDB::User"]),
    entry("mq:broker", &["AWS::AmazonMQ::Broker"]),
    entry("mq:configuration", &["AWS::AmazonMQ::Configuration"]),
    entry("network-firewall:firewall", &["AWS::NetworkFirewall::Firewall"]),
    entry(
        "network-firewall:firewall-policy",
        &["AWS::NetworkFirewall::FirewallPolicy"],
    ),
    entry(
        "network-firewall:stateful-rulegroup",
        &["AWS::NetworkFirewall::RuleGroup"],
    ),
    entry(
        "network-firewall:stateless-rulegroup",
        &["AWS::NetworkFirewall::RuleGroup"],
    ),
    entry(
        "networkmanager:core-network",
        &["AWS::NetworkManager::CoreNetwork"],
    ),
    entry("networkmanager:device", &["AWS::NetworkManager::Device"]),
    entry(
        "networkmanager:global-network",
        &["AWS::NetworkManager::GlobalNetwork"],
    ),
    entry("networkmanager:link", &["AWS::NetworkManager::Link"]),
    entry("networkmanager:site", &["AWS::NetworkManager::Site"]),
    entry("oam:link", &["AWS::Oam::Link"]),
    entry("oam:sink", &["AWS::Oam::Sink"]),
    entry("organizations:account", &["AWS::Organizations::Account"]),
    entry("organizations:ou", &["AWS::Organizations::OrganizationalUnit"]),
    entry("organizations:policy", &["AWS::Organizations::Policy"]),
    entry("personalize:dataset", &["AWS::Personalize::Dataset"]),
    entry(
        "personalize:dataset-group",
        &["AWS::Personalize::DatasetGroup"],
    ),
    entry("personalize:schema", &["AWS::Personalize::Schema"]),
    entry("personalize:solution", &["AWS::Personalize::Solution"]),
    entry("pipes:pipe", &["AWS::Pipes::Pipe"]),
    entry("profile:domain", &["AWS::CustomerProfiles::Domain"]),
    entry(
        "proton:environment-template",
        &["AWS::Proton::EnvironmentTemplate"],
    ),
    entry("proton:service-template", &["AWS::Proton::ServiceTemplate"]),
    entry("qldb:ledger", &["AWS::QLDB::Ledger"]),
    entry("qldb:stream", &["AWS::QLDB::Stream"]),
    entry("quicksight:analysis", &["AWS::QuickSight::Analysis"]),
    entry("quicksight:dashboard", &["AWS::QuickSight::Dashboard"]),
    entry("quicksight:dataset", &["AWS::QuickSight::DataSet"]),
    entry("quicksight:datasource", &["AWS::QuickSight::DataSource"]),
    entry("quicksight:template", &["AWS::QuickSight::Template"]),
    entry("quicksight:theme", &["AWS::QuickSight::Theme"]),
    entry("ram:resource-share", &["AWS::RAM::ResourceShare"]),
    entry("rbin:rule", &["AWS::Rbin::Rule"]),
    entry("rds:cluster", &["AWS::RDS::DBCluster"]),
    entry("rds:cluster-pg", &["AWS::RDS::DBClusterParameterGroup"]),
    entry("rds:db", &["AWS::RDS::DBInstance"]),
    entry("rds:db-proxy", &["AWS::RDS::DBProxy"]),
    entry("rds:es", &["AWS::RDS::EventSubscription"]),
    entry("rds:global-cluster", &["AWS::RDS::GlobalCluster"]),
    entry("rds:og", &["AWS::RDS::OptionGroup"]),
    entry("rds:pg", &["AWS::RDS::DBParameterGroup"]),
    entry("rds:secgrp", &["AWS::RDS::DBSecurityGroup"]),
    entry("rds:subgrp", &["AWS::RDS::DBSubnetGroup"]),
    entry(
        "redshift-serverless:namespace",
        &["AWS::RedshiftServerless::Namespace"],
    ),
    entry(
        "redshift-serverless:workgroup",
        &["AWS::RedshiftServerless::Workgroup"],
    ),
    entry("redshift:cluster", &["AWS::Redshift::Cluster"]),
    entry(
        "redshift:eventsubscription",
        &["AWS::Redshift::EventSubscription"],
    ),
    entry(
        "redshift:parametergroup",
        &["AWS::Redshift::ClusterParameterGroup"],
    ),
    entry(
        "redshift:subnetgroup",
        &["AWS::Redshift::ClusterSubnetGroup"],
    ),
    entry(
        "refactor-spaces:environment",
        &["AWS::RefactorSpaces::Environment"],
    ),
    entry("resiliencehub:app", &["AWS::ResilienceHub::App"]),
    entry(
        "resiliencehub:resiliency-policy",
        &["AWS::ResilienceHub::ResiliencyPolicy"],
    ),
    entry(
        "resource-explorer-2:index",
        &["AWS::ResourceExplorer2::Index"],
    ),
    entry("resource-explorer-2:view", &["AWS::ResourceExplorer2::View"]),
    entry("resource-groups:group", &["AWS::ResourceGroups::Group"]),
    entry(
        "robomaker:robot-application",
        &["AWS::RoboMaker::RobotApplication"],
    ),
    entry(
        "robomaker:simulation-application",
        &["AWS::RoboMaker::SimulationApplication"],
    ),
    entry(
        "route53-recovery-control:cluster",
        &["AWS::Route53RecoveryControl::Cluster"],
    ),
    entry(
        "route53-recovery-control:controlpanel",
        &["AWS::Route53RecoveryControl::ControlPanel"],
    ),
    entry(
        "route53-recovery-control:routingcontrol",
        &["AWS::Route53RecoveryControl::RoutingControl"],
    ),
    entry(
        "route53-recovery-control:safetyrule",
        &["AWS::Route53RecoveryControl::SafetyRule"],
    ),
    entry(
        "route53-recovery-readiness:cell",
        &["AWS::Route53RecoveryReadiness::Cell"],
    ),
    entry(
        "route53-recovery-readiness:readinesscheck",
        &["AWS::Route53RecoveryReadiness::ReadinessCheck"],
    ),
    entry(
        "route53-recovery-readiness:recoverygroup",
        &["AWS::Route53RecoveryReadiness::RecoveryGroup"],
    ),
    entry(
        "route53-recovery-readiness:resourceset",
        &["AWS::Route53RecoveryReadiness::ResourceSet"],
    ),
    entry("route53:healthcheck", &["AWS::Route53::HealthCheck"]),
    entry("route53:hostedzone", &["AWS::Route53::HostedZone"]),
    entry(
        "route53resolver:firewall-domain-list",
        &["AWS::Route53Resolver::FirewallDomainList"],
    ),
    entry(
        "route53resolver:firewall-rule-group",
        &["AWS::Route53Resolver::FirewallRuleGroup"],
    ),
    entry(
        "route53resolver:resolver-endpoint",
        &["AWS::Route53Resolver::ResolverEndpoint"],
    ),
    entry(
        "route53resolver:resolver-query-log-config",
        &["AWS::Route53Resolver::ResolverQueryLoggingConfig"],
    ),
    entry(
        "route53resolver:resolver-rule",
        &["AWS::Route53Resolver::ResolverRule"],
    ),
    entry("rum:appmonitor", &["AWS::RUM::AppMonitor"]),
    entry("s3:accesspoint", &["AWS::S3::AccessPoint"]),
    entry("s3:bucket", &["AWS::S3::Bucket"]),
    entry("s3:storage-lens", &["AWS::S3::StorageLens"]),
    entry(
        "sagemaker:app-image-config",
        &["AWS::SageMaker::AppImageConfig"],
    ),
    entry(
        "sagemaker:code-repository",
        &["AWS::SageMaker::CodeRepository"],
    ),
    entry("sagemaker:domain", &["AWS::SageMaker::Domain"]),
    entry("sagemaker:endpoint", &["AWS::SageMaker::Endpoint"]),
    entry(
        "sagemaker:endpoint-config",
        &["AWS::SageMaker::EndpointConfig"],
    ),
    entry("sagemaker:model", &["AWS::SageMaker::Model"]),
    entry(
        "sagemaker:notebook-instance",
        &["AWS::SageMaker::NotebookInstance"],
    ),
    entry("sagemaker:pipeline", &["AWS::SageMaker::Pipeline"]),
    entry("sagemaker:project", &["AWS::SageMaker::Project"]),
    entry("scheduler:schedule", &["AWS::Scheduler::Schedule"]),
    entry("scheduler:schedule-group", &["AWS::Scheduler::ScheduleGroup"]),
    entry("schemas:discoverer", &["AWS::EventSchemas::Discoverer"]),
    entry("schemas:registry", &["AWS::EventSchemas::Registry"]),
    entry("schemas:schema", &["AWS::EventSchemas::Schema"]),
    entry("secretsmanager:secret", &["AWS::SecretsManager::Secret"]),
    entry("servicecatalog:portfolio", &["AWS::ServiceCatalog::Portfolio"]),
    entry(
        "servicediscovery:namespace",
        &[
            "AWS::ServiceDiscovery::HttpNamespace",
            "AWS::ServiceDiscovery::PrivateDnsNamespace",
            "AWS::ServiceDiscovery::PublicDnsNamespace",
        ],
    ),
    entry("servicediscovery:service", &["AWS::ServiceDiscovery::Service"]),
    entry("ses:configuration-set", &["AWS::SES::ConfigurationSet"]),
    entry("ses:contact-list", &["AWS::SES::ContactList"]),
    entry("ses:identity", &["AWS::SES::EmailIdentity"]),
    entry("ses:template", &["AWS::SES::Template"]),
    entry("shield:protection", &["AWS::Shield::Protection"]),
    entry("signer:signing-profile", &["AWS::Signer::SigningProfile"]),
    entry("sns:topic", &["AWS::SNS::Topic"]),
    entry("sqs:queue", &["AWS::SQS::Queue"]),
    entry("ssm-contacts:contact", &["AWS::SSMContacts::Contact"]),
    entry(
        "ssm-incidents:response-plan",
        &["AWS::SSMIncidents::ResponsePlan"],
    ),
    entry("ssm:association", &["AWS::SSM::Association"]),
    entry("ssm:document", &["AWS::SSM::Document"]),
    entry("ssm:maintenancewindow", &["AWS::SSM::MaintenanceWindow"]),
    entry("ssm:parameter", &["AWS::SSM::Parameter"]),
    entry("ssm:patchbaseline", &["AWS::SSM::PatchBaseline"]),
    entry("ssm:resource-data-sync", &["AWS::SSM::ResourceDataSync"]),
    entry("states:activity", &["AWS::StepFunctions::Activity"]),
    entry("states:stateMachine", &["AWS::StepFunctions::StateMachine"]),
    entry("synthetics:canary", &["AWS::Synthetics::Canary"]),
    entry("synthetics:group", &["AWS::Synthetics::Group"]),
    entry("timestream:database", &["AWS::Timestream::Database"]),
    entry("timestream:table", &["AWS::Timestream::Table"]),
    entry("transfer:server", &["AWS::Transfer::Server"]),
    entry("transfer:user", &["AWS::Transfer::User"]),
    entry("transfer:workflow", &["AWS::Transfer::Workflow"]),
    entry("voiceid:domain", &["AWS::VoiceID::Domain"]),
    entry("wafv2:ipset", &["AWS::WAFv2::IPSet"]),
    entry("wafv2:regexpatternset", &["AWS::WAFv2::RegexPatternSet"]),
    entry("wafv2:rulegroup", &["AWS::WAFv2::RuleGroup"]),
    entry("wafv2:webacl", &["AWS::WAFv2::WebACL"]),
    entry("wellarchitected:workload", &["AWS::WellArchitected::Workload"]),
    entry(
        "workspaces:connectionalias",
        &["AWS::WorkSpaces::ConnectionAlias"],
    ),
    entry("workspaces:workspace", &["AWS::WorkSpaces::Workspace"]),
    entry("xray:group", &["AWS::XRay::Group"]),
    entry("xray:sampling-rule", &["AWS::XRay::SamplingRule"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The published order is part of the answer: ascending by `ResourceType`, each
    /// identifier listed once.
    #[test]
    fn the_catalogue_is_sorted_and_has_no_duplicate_resource_type() {
        for pair in CATALOGUE.windows(2) {
            assert!(
                pair[0].resource_type < pair[1].resource_type,
                "case {:?} then {:?}: the catalogue is ascending by ResourceType with no \
                 duplicates",
                pair[0].resource_type,
                pair[1].resource_type
            );
        }
        assert!(
            is_sorted(CATALOGUE),
            "the sorted invariant must agree with the rows as written"
        );
    }

    /// Every row is a `<service>:<type>` identifier whose first segment is the `Service`
    /// the row publishes, and every CloudFormation identifier it carries is an
    /// `AWS::<Service>::<Type>` name.
    #[test]
    fn every_row_is_well_formed() {
        for row in CATALOGUE {
            let case = row.resource_type;
            let (service, kind) = case
                .split_once(':')
                .unwrap_or_else(|| panic!("case {case:?}: a ResourceType is <service>:<type>"));
            assert!(
                !service.is_empty() && !kind.is_empty(),
                "case {case:?}: both segments of a ResourceType carry text"
            );
            assert_eq!(
                row.service(),
                service,
                "case {case:?}: Service is the identifier's first segment"
            );
            for cfn in row.cfn_resource_types {
                assert!(
                    cfn.starts_with("AWS::") && cfn.split("::").count() == 3,
                    "case {case:?}: {cfn:?} is not an AWS::<Service>::<Type> identifier"
                );
            }
        }
    }

    /// The catalogue is service-wide, so it is a SUPERSET of the far smaller set of
    /// types a Region's index in this emulator reports resources for.
    #[test]
    fn the_catalogue_covers_every_type_this_emulators_index_can_hold() {
        for indexed in super::super::owners::INDEXED_TYPES {
            let case = indexed.resource_type;
            let published = CATALOGUE
                .iter()
                .find(|row| row.resource_type == case)
                .unwrap_or_else(|| panic!("case {case:?}: an indexed type must be published"));
            assert!(
                published
                    .cfn_resource_types
                    .contains(&indexed.cfn_resource_type),
                "case {case:?}: expected the catalogue to carry {:?}, got {:?}",
                indexed.cfn_resource_type,
                published.cfn_resource_types
            );
        }
    }
}
