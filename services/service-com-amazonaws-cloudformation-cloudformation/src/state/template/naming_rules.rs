// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The naming rules the service that owns `resource_type` holds a name to.
///
/// Each bound is the owning service's OWN contract for the member a resource of the type is
/// created under — the `@length` in that service's Smithy model where it carries one, and
/// the limit that service's API reference publishes where the model does not. A type absent
/// from this catalogue has no bound a generated name could break (its name is assigned by
/// the service, or its owning API publishes none), so its generated name is left as built.
pub(super) fn naming_rules(resource_type: &str) -> Option<NameRules> {
    let rules = |max_characters: usize, characters: Characters| {
        Some(NameRules {
            max_characters,
            characters,
        })
    };
    match resource_type {
        // "Bucket names must be between 3 (min) and 63 (max) characters long", lowercase
        // (Amazon S3 bucket naming rules).
        S3_BUCKET => rules(63, Characters::LowercaseAlphanumeric),
        // IAM `CreateRole.RoleName`: `@length(min: 1, max: 64)` (`roleNameType`, IAM's own
        // Smithy model). A longer name is the 400 `ValidationError` IAM answers, which the
        // stack that declared the role then rolls back on.
        "AWS::IAM::Role" => rules(64, Characters::AsGenerated),
        // IAM `CreateUser.UserName`: `userNameType`, `@length(min: 1, max: 64)`.
        "AWS::IAM::User" => rules(64, Characters::AsGenerated),
        // IAM `CreateGroup.GroupName`: `groupNameType`, `@length(min: 1, max: 128)`.
        "AWS::IAM::Group" => rules(128, Characters::AsGenerated),
        // IAM `CreateInstanceProfile.InstanceProfileName`: `instanceProfileNameType`,
        // `@length(min: 1, max: 128)`.
        "AWS::IAM::InstanceProfile" => rules(128, Characters::AsGenerated),
        // IAM `CreatePolicy.PolicyName`: `policyNameType`, `@length(min: 1, max: 128)`.
        "AWS::IAM::ManagedPolicy" => rules(128, Characters::AsGenerated),
        // Amazon Data Firehose `CreateDeliveryStream.DeliveryStreamName`:
        // `@length(min: 1, max: 64)` (Firehose's own Smithy model).
        FIREHOSE_DELIVERY_STREAM => rules(64, Characters::AsGenerated),
        // Amazon Kinesis Data Streams `CreateStream.StreamName`: `@length(min: 1, max:
        // 128)` with the pattern `^[a-zA-Z0-9_.-]+$` (that service's own model), which
        // admits every character a generated name is built from.
        KINESIS_STREAM => rules(128, Characters::AsGenerated),
        // Amazon ECR `CreateRepository.repositoryName`: `RepositoryName`,
        // `@length(min: 2, max: 256)`, lowercase (Amazon ECR's own Smithy model).
        ECR_REPOSITORY => rules(256, Characters::LowercaseAlphanumeric),
        // Amazon ECR Public `CreateRepository.repositoryName`: `RepositoryName`,
        // `@length(min: 2, max: 205)`, lowercase (Amazon ECR Public's own Smithy model).
        ECR_PUBLIC_REPOSITORY => rules(205, Characters::LowercaseAlphanumeric),
        // CodeBuild `CreateReportGroup.name`: `ReportGroupName`,
        // `@length(min: 2, max: 128)` (CodeBuild's own Smithy model), which a generated
        // name is already inside character-wise and may only be longer than.
        CODEBUILD_REPORT_GROUP => rules(128, Characters::AsGenerated),
        // CodeBuild `CreateProject.name`: `ProjectName`, `@length(min: 2, max: 150)` with
        // pattern `^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$` (CodeBuild's own Smithy model),
        // which a generated name is already inside — letters, digits and the hyphen are all
        // admitted, and it starts with a letter because a stack name does — but may be
        // longer than.
        CODEBUILD_PROJECT => rules(150, Characters::AsGenerated),
        // CodeBuild `CreateFleet.name`: `FleetName`, `@length(min: 2, max: 128)` with pattern
        // `^[A-Za-z0-9][A-Za-z0-9\-_]{1,127}$` (CodeBuild's own Smithy model), which a
        // generated name is already inside — letters, digits and the hyphen are all admitted,
        // and it starts with a letter because a stack name does — but may be longer than.
        crate::state::provisioner::codebuild_fleets::CODEBUILD_FLEET => {
            rules(128, Characters::AsGenerated)
        }
        // CodeCommit `CreateRepository.repositoryName`: `RepositoryName`,
        // `@length(min: 1, max: 100)` with pattern `^[\w\.-]+$` (CodeCommit's own Smithy
        // model), which a generated name is already inside — letters, digits and the hyphen
        // are all admitted — but may be longer than.
        CODECOMMIT_REPOSITORY => rules(100, Characters::AsGenerated),
        // CodeArtifact `CreateDomain.domain`: `DomainName`, `@length(min: 2, max: 50)` with
        // pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$` (CodeArtifact's own Smithy model), which
        // admits no upper case and requires an alphanumeric at the end. The LEADING
        // character that pattern requires is a letter, which a generated name starts with
        // because a stack name does (`[a-zA-Z][-a-zA-Z0-9]*`) and the lower-casing keeps it
        // one; `within` itself only trims the edges to an alphanumeric.
        CODEARTIFACT_DOMAIN => rules(50, Characters::LowercaseAlphanumeric),
        // CodeArtifact `CreateRepository.repository`: `RepositoryName`,
        // `@length(min: 2, max: 100)` with pattern `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$` (that
        // same model), which a generated name is already inside — letters, digits and the
        // hyphen are all admitted — but may be longer than.
        CODEARTIFACT_REPOSITORY => rules(100, Characters::AsGenerated),
        // CodePipeline `PipelineDeclaration.name`: `PipelineName`,
        // `@length(min: 1, max: 100)` (CodePipeline's own Smithy model).
        "AWS::CodePipeline::Pipeline" => rules(100, Characters::AsGenerated),
        // Amazon DynamoDB `CreateTable.TableName`: `TableName`,
        // `@length(min: 3, max: 255)` (DynamoDB's own Smithy model).
        "AWS::DynamoDB::Table" => rules(255, Characters::AsGenerated),
        // Amazon CloudWatch Logs `CreateLogGroup.logGroupName`: `LogGroupName`,
        // `@length(min: 1, max: 512)` (CloudWatch Logs's own Smithy model).
        "AWS::Logs::LogGroup" => rules(512, Characters::AsGenerated),
        // The same service's `CreateLogStream.logStreamName` (`LogStreamName`) and
        // `PutMetricFilter.filterName` (`FilterName`): each `@length(min: 1, max: 512)` with
        // pattern `^[^:*]*$` (that same model), which a generated name is already inside —
        // it carries neither a colon nor an asterisk — but may be longer than.
        crate::state::provisioning::logs_resources::LOGS_LOG_STREAM
        | crate::state::provisioning::logs_resources::LOGS_METRIC_FILTER => {
            rules(512, Characters::AsGenerated)
        }
        // Amazon SageMaker AI `CreateModel.ModelName`, `CreateEndpointConfig.\
        // EndpointConfigName`, `CreateEndpoint.EndpointName`, `CreateNotebookInstance.\
        // NotebookInstanceName` and `CreateNotebookInstanceLifecycleConfig.\
        // NotebookInstanceLifecycleConfigName`: each `@length(min: 0, max: 63)` with a pattern
        // admitting letters, digits and inner hyphens and requiring an alphanumeric at each
        // end (`^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, and the model's
        // `^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$`, in SageMaker's own Smithy model) —
        // which a generated name is already inside, since it starts with a stack name's
        // leading letter and carries only letters, digits and single hyphens, but may be
        // longer than.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_ENDPOINT_CONFIG
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_ENDPOINT
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            rules(63, Characters::AsGenerated)
        }
        // The same service's `CreateDomain.DomainName`, `CreateUserProfile.UserProfileName` and
        // `CreateSpace.SpaceName`: each `@length(min: 0, max: 63)` with that same pattern
        // (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, in SageMaker's own Smithy model), which a
        // generated name is already inside and may only be longer than. The domain's name is
        // the one CloudFormation generates for a declaration that states none; a member of a
        // domain is identified by its own name, so a declaration states that
        // ([`crate::state::provisioning::model_validation`]).
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_DOMAIN
        | crate::state::provisioning::sagemaker_studio::SAGEMAKER_USER_PROFILE
        | crate::state::provisioning::sagemaker_studio::SAGEMAKER_SPACE => {
            rules(63, Characters::AsGenerated)
        }
        // AWS IoT `CreateAuthorizer.authorizerName`: `AuthorizerName`,
        // `@length(min: 1, max: 128)` with pattern `^[\w=,@-]+$` (AWS IoT's own Smithy
        // model), which a generated name is already inside — letters, digits and the hyphen
        // are all admitted — but may be longer than.
        crate::state::provisioning::iot_authorizer::IOT_AUTHORIZER => rules(128, Characters::AsGenerated),
        // AWS IoT `CreateProvisioningTemplate.templateName`: `TemplateName`,
        // `@length(min: 1, max: 36)` with pattern `^[0-9A-Za-z_-]+$` (AWS IoT's own Smithy
        // model), which a generated name is already inside — letters, digits and the hyphen
        // are all admitted — but is very likely longer than.
        crate::state::provisioning::iot_provisioning_resources::IOT_PROVISIONING_TEMPLATE => {
            rules(36, Characters::AsGenerated)
        }
        // The same service's `CreateRoleAlias.roleAlias`: `RoleAlias`,
        // `@length(min: 1, max: 128)` with pattern `^[\w=,@-]+$` (that same model), which a
        // generated name is already inside but may be longer than.
        crate::state::provisioning::iot_provisioning_resources::IOT_ROLE_ALIAS => {
            rules(128, Characters::AsGenerated)
        }
        // The same service's `CreateThing.thingName`: `ThingName`,
        // `@length(min: 1, max: 128)` with pattern `^[a-zA-Z0-9:_-]+$` (AWS IoT's own Smithy
        // model), which a generated name is already inside — letters, digits and the hyphen
        // are all admitted — but may be longer than.
        crate::state::provisioning::iot_thing::IOT_THING => rules(128, Characters::AsGenerated),
        // The same service's `CreateTopicRule.ruleName`: `RuleName`,
        // `@length(min: 1, max: 128)` with pattern `^[a-zA-Z0-9_]+$` (AWS IoT's own Smithy
        // model), which admits letters, digits and the UNDERSCORE and no hyphen at all — so a
        // generated name's separators are what one breaks, beside the bound.
        crate::state::provisioning::iot_topic_rule::IOT_TOPIC_RULE => {
            rules(128, Characters::AlphanumericUnderscore)
        }
        // Amazon CloudWatch `PutMetricAlarm.AlarmName` / `PutCompositeAlarm.AlarmName`:
        // `AlarmName`, `@length(min: 1, max: 255)` (CloudWatch's own Smithy model).
        "AWS::CloudWatch::Alarm"
        | crate::state::provisioner::cloudwatch_composite_alarms::CLOUDWATCH_COMPOSITE_ALARM => {
            rules(255, Characters::AsGenerated)
        }
        // Amazon CloudWatch `PutDashboard.DashboardName`: "The maximum length is 255, and
        // valid characters are A-Z, a-z, 0-9, \"-\", and \"_\""
        // (`PutDashboardInput$DashboardName`) — the CloudWatch model carries no `@length` for
        // it, and a generated name is already inside that character set but may be longer
        // than the bound.
        CLOUDWATCH_DASHBOARD => rules(255, Characters::AsGenerated),
        // AWS Secrets Manager `CreateSecret.Name`: `NameType`,
        // `@length(min: 1, max: 512)` (Secrets Manager's own Smithy model).
        "AWS::SecretsManager::Secret" => rules(512, Characters::AsGenerated),
        // AWS Lambda `CreateFunction.FunctionName`: the modelled 140 characters covers the
        // full ARN — "If you specify only the function name, it is limited to 64 characters
        // in length" (`FunctionName`, AWS Lambda API reference).
        "AWS::Lambda::Function" => rules(64, Characters::AsGenerated),
        // AWS Lambda `AddPermission.StatementId`: `StatementId`, `@length(min: 1, max: 100)`
        // with pattern `^([a-zA-Z0-9-_]+)$` (AWS Lambda's own Smithy model). The statement a
        // stack declares is added under THIS resource's generated identifier — its physical
        // id is the statement id — so that identifier is held to the same bound every other
        // generated name is: a generated name is already inside the character set, since
        // letters, digits and the hyphen are all admitted, but may be longer than the bound.
        "AWS::Lambda::Permission" => rules(100, Characters::AsGenerated),
        // The same service's `PublishLayerVersion.LayerName`: `LayerName`,
        // `@length(min: 1, max: 140)`, whose bare-name branch is `[a-zA-Z0-9-_]+` (that same
        // model) — the layer a version is published under is the generated name wherever the
        // declaration states none, and such a name may be longer than the bound.
        "AWS::Lambda::LayerVersion" => rules(140, Characters::AsGenerated),
        // AWS Step Functions `CreateStateMachine.name` and `CreateActivity.name`: each
        // `Name`, `@length(min: 1, max: 80)` (that service's own Smithy model). A name "must
        // not contain" white space, brackets, wildcards, that member's listed special
        // characters, control characters or surrogates (`CreateStateMachineInput$name`) —
        // none of which a generated name carries — so the bound is what one may break.
        STEP_FUNCTIONS_STATE_MACHINE | STEP_FUNCTIONS_ACTIVITY => {
            rules(80, Characters::AsGenerated)
        }
        // The same service's `CreateStateMachineAlias.name`: `CharacterRestrictedName`,
        // `@length(min: 1, max: 80)` with the pattern
        // `^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$` (that same model) — a generated name is
        // already inside that character set, since letters, digits and the hyphen are all
        // admitted, so the bound is what one may break. An alias's generated name is the name
        // the create is made under, the identity in force being the ARN that service answers
        // ([`crate::state::provisioning::step_functions_versions_and_aliases`]).
        crate::state::provisioning::step_functions_versions_and_aliases::STEP_FUNCTIONS_STATE_MACHINE_ALIAS => {
            rules(80, Characters::AsGenerated)
        }
        // AppConfig `CreateApplication.Name`, `CreateEnvironment.Name` and
        // `CreateDeploymentStrategy.Name`: each `Name`, `@length(min: 1, max: 64)`
        // (AppConfig's own Smithy model), which a generated name is already inside
        // character-wise and may only be longer than.
        APPCONFIG_APPLICATION | APPCONFIG_ENVIRONMENT | APPCONFIG_DEPLOYMENT_STRATEGY => {
            rules(64, Characters::AsGenerated)
        }
        // The same service's `CreateConfigurationProfile.Name`: `LongName`,
        // `@length(min: 1, max: 128)` (that same model), the one of the four that takes the
        // longer name.
        APPCONFIG_CONFIGURATION_PROFILE => rules(128, Characters::AsGenerated),
        // Amazon SQS `CreateQueue.QueueName`: "A queue name can have up to 80 characters"
        // (Amazon SQS API reference) — SQS's model carries no `@length` for it.
        SQS_QUEUE => rules(80, Characters::AsGenerated),
        // Amazon EventBridge Pipes `CreatePipe.Name`: `PipeName`, `@length(min: 1, max: 64)`
        // with pattern `^[\.\-_A-Za-z0-9]+$` (that service's own Smithy model), which a
        // generated name is already inside — letters, digits and the hyphen are all admitted —
        // but may be longer than. A pipe's name is also this type's physical id ("Ref returns
        // the pipe name"), so a declaration that names no pipe is created under the generated
        // name and that name has to be one `CreatePipe` takes.
        PIPES_PIPE => rules(64, Characters::AsGenerated),
        // Amazon MemoryDB `CreateParameterGroup.ParameterGroupName` and
        // `CreateSubnetGroup.SubnetGroupName`: that service's model carries no `@length` for
        // either, and its own rule for a resource name — the one its refusal states — is at
        // most 40 characters, beginning with a letter, letters, digits and inner hyphens
        // only (`crate::state::provisioner::memorydb_groups`). A MemoryDB name is also a
        // CASE-INSENSITIVE identifier, stored folded, so a generated name is fitted to the
        // lower-case spelling the group will exist under rather than to one that only
        // resolves to it.
        MEMORYDB_PARAMETER_GROUP | MEMORYDB_SUBNET_GROUP => {
            rules(40, Characters::LowercaseAlphanumeric)
        }
        // Amazon SNS `CreateTopic.Name`: topic names "must be between 1 and 256 characters
        // long" (Amazon SNS API reference) — SNS's model carries no `@length` for it.
        SNS_TOPIC => rules(256, Characters::AsGenerated),
        // Amazon SES `CreateConfigurationSet.ConfigurationSet.Name`: that member's own rule
        // is "Contain only letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes
        // (-)" and "Contain 64 characters or fewer" (`ConfigurationSet$Name`, Amazon SES's
        // own Smithy model) — a generated name is already inside that character set, since
        // letters, digits and the hyphen are all admitted, but may be longer than the bound.
        SES_CONFIGURATION_SET => rules(64, Characters::AsGenerated),
        // Amazon SES `CreateReceiptRuleSet.RuleSetName` and `CreateReceiptRule.Rule.Name`:
        // each member's own rule is "Contain only ASCII letters (a-z, A-Z), numbers (0-9),
        // underscores (_), or dashes (-)" (a rule's name admits periods too), "Start and end
        // with a letter or number" and "Contain 64 characters or fewer"
        // (`ReceiptRuleSetName` and `ReceiptRule$Name`, Amazon SES's own Smithy model) — a
        // generated name is already inside both character sets and starts and ends with one
        // of their characters, but may be longer than the bound.
        SES_RECEIPT_RULE_SET | SES_RECEIPT_RULE => rules(64, Characters::AsGenerated),
        // Amazon ECS `CreateCluster.clusterName`: "Up to 255 letters (uppercase and
        // lowercase), numbers, underscores, and hyphens are allowed" (Amazon ECS API
        // reference) — ECS's model carries no `@length` for it.
        ECS_CLUSTER => rules(255, Characters::AsGenerated),
        // The same service's `CreateCapacityProvider.name`: "Up to 255 characters are
        // allowed. They include letters (both upper and lowercase letters), numbers,
        // underscores (_), and hyphens (-)" (`CreateCapacityProviderRequest$name`) — ECS's
        // model carries no `@length` for it either, and a generated name is already inside
        // that character set but may be longer than the bound.
        crate::state::provisioner::ecs_capacity_providers::ECS_CAPACITY_PROVIDER => {
            rules(255, Characters::AsGenerated)
        }
        // Amazon EKS `CreateCluster.name`: "The unique name to give to your cluster", whose
        // own `@length` is 1..100 and whose pattern is `^[0-9A-Za-z][A-Za-z0-9\-_]*$`
        // (`ClusterName` in that service's model) — a generated name is inside that
        // character set and starts with one of its characters, but may be longer than the
        // bound.
        EKS_CLUSTER => rules(100, Characters::AsGenerated),
        // AWS Batch `CreateComputeEnvironment.computeEnvironmentName`,
        // `CreateJobQueue.jobQueueName` and `RegisterJobDefinition.jobDefinitionName`: each
        // "can be up to 128 characters long. It can contain uppercase and lowercase letters,
        // numbers, hyphens (-), and underscores (_)" (those members' own docs) — Batch's model
        // types them all as plain strings.
        // The same service's `CreateSchedulingPolicy.name` ("up to 128 letters long … It can
        // contain uppercase and lowercase letters, numbers, hyphens (-), and underscores (_)")
        // and `CreateQuotaShare.quotaShareName` ("up to 128 characters long", the same charset)
        // are the same rule, so the sharing plane's two types share the arm.
        BATCH_COMPUTE_ENVIRONMENT
        | BATCH_JOB_QUEUE
        | BATCH_JOB_DEFINITION
        | BATCH_QUOTA_SHARE
        | BATCH_SCHEDULING_POLICY
        | BATCH_SERVICE_ENVIRONMENT => rules(128, Characters::AsGenerated),
        // Elastic Load Balancing `CreateLoadBalancer.Name` and
        // `CreateTargetGroup.Name`: each "can have a maximum of 32 characters" (Elastic
        // Load Balancing API reference) — the ELBv2 model carries no `@length` for either.
        // The same service's `CreateTrustStore.Name` is bounded by its own model
        // (`TrustStoreName`, `@length(min: 1, max: 32)` with pattern
        // `^([a-zA-Z0-9]+-)*[a-zA-Z0-9]+$`), which a generated name's character set satisfies.
        ELB_LOAD_BALANCER | ELB_TARGET_GROUP | ELB_TRUST_STORE => {
            rules(32, Characters::AsGenerated)
        }
        // Amazon S3 Tables `CreateTableBucket.name`: `TableBucketName`,
        // `@length(min: 3, max: 63)` with pattern `^[0-9a-z-]*$` (that service's own Smithy
        // model), which admits no upper case; that service also holds a name to beginning
        // and ending with an alphanumeric, which is what this character set trims to.
        S3TABLES_TABLE_BUCKET => rules(63, Characters::LowercaseAlphanumeric),
        // Amazon S3 Vectors `CreateVectorBucket.vectorBucketName` and
        // `CreateIndex.indexName`: each `@length(min: 3, max: 63)` with pattern
        // `[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]` (that service's own Smithy model), which
        // admits no upper case and requires an alphanumeric at each end.
        S3VECTORS_VECTOR_BUCKET | S3VECTORS_INDEX => {
            rules(63, Characters::LowercaseAlphanumeric)
        }
        // Amazon RDS `DBInstanceIdentifier` / `DBClusterIdentifier`: each "must contain
        // from 1 to 63 letters, numbers, or hyphens", "can't end with a hyphen or contain
        // two consecutive hyphens", and is "stored as all lowercase" (Amazon RDS API
        // reference) — RDS's model carries no `@length` for either.
        "AWS::RDS::DBInstance" | "AWS::RDS::DBCluster" => {
            rules(63, Characters::LowercaseAlphanumeric)
        }
        // The Amazon RDS GROUP names, each created under a member whose own doc says "This
        // value is stored as a lowercase string" — so a name carrying upper case names a
        // group that service does not hold:
        // `CreateDBParameterGroup.DBParameterGroupName` ("Must be 1 to 255 letters,
        // numbers, or hyphens", "First character must be a letter", "Can't end with a
        // hyphen or contain two consecutive hyphens"),
        // `CreateDBClusterParameterGroup.DBClusterParameterGroupName` (whose name a modify
        // answers with, `DBClusterParameterGroupNameMessage$DBClusterParameterGroupName`,
        // states "from 1 to 255 letters or numbers") and
        // `CreateDBSubnetGroup.DBSubnetGroupName` ("Must contain no more than 255 letters,
        // numbers, periods, underscores, spaces, or hyphens", "First character must be a
        // letter") and `CreateOptionGroup.OptionGroupName` ("Must be 1 to 255 letters,
        // digits, or hyphens", "First character must be a letter", "Can't end with a hyphen
        // or contain two consecutive hyphens") — RDS's model carries no `@length` for any of
        // them.
        "AWS::RDS::DBParameterGroup"
        | "AWS::RDS::DBClusterParameterGroup"
        | crate::state::provisioner::rds_option_groups::RDS_OPTION_GROUP
        | "AWS::RDS::DBSubnetGroup"
        // Amazon Neptune's groups are the same members of the same front door, so a name
        // CloudFormation generates for one is held to the same rules
        // ([`crate::state::provisioner::neptune_relational`]).
        | crate::state::provisioner::neptune_relational::NEPTUNE_DB_PARAMETER_GROUP
        | crate::state::provisioner::neptune_relational::NEPTUNE_DB_CLUSTER_PARAMETER_GROUP
        | crate::state::provisioner::neptune_relational::NEPTUNE_DB_SUBNET_GROUP => {
            rules(255, Characters::LowercaseAlphanumeric)
        }
        // The Amazon ElastiCache identifiers a stack GENERATES a name for, each created under
        // a member whose own doc says the value "is stored as a lowercase string" — so a name
        // carrying upper case names a resource that service does not hold under it, the
        // position the Amazon RDS identifiers above are in:
        // `CreateCacheCluster.CacheClusterId` ("A name must contain from 1 to 50 alphanumeric
        // characters or hyphens", "The first character must be a letter", "A name cannot end
        // with a hyphen or contain two consecutive hyphens"),
        // `CreateReplicationGroup.ReplicationGroupId` (the same rule with 40 characters) and
        // `CreateCacheSubnetGroup.CacheSubnetGroupName` ("Must contain no more than 255
        // alphanumeric characters or hyphens") — ElastiCache's model carries no `@length` for
        // any of them.
        crate::state::provisioner::elasticache_caches::ELASTICACHE_CACHE_CLUSTER => {
            rules(50, Characters::LowercaseAlphanumeric)
        }
        crate::state::provisioner::elasticache_replication_groups::ELASTICACHE_REPLICATION_GROUP => {
            rules(
                crate::state::provisioner::elasticache_replication_groups::REPLICATION_GROUP_ID_MAX_CHARACTERS,
                Characters::LowercaseAlphanumeric,
            )
        }
        crate::state::provisioner::elasticache_subnet_groups::ELASTICACHE_SUBNET_GROUP => {
            rules(255, Characters::LowercaseAlphanumeric)
        }
        // Amazon RDS `CreateDBSnapshot.DBSnapshotIdentifier`: "must contain from 1 to 255
        // letters, numbers, or hyphens", "first character must be a letter" and "can't end
        // with a hyphen or contain two consecutive hyphens" (that member's own doc) — RDS's
        // model carries no `@length` for it. The identifier is stored as the caller spelled
        // it, unlike the lower-cased identifiers around it, so a generated name for this one
        // only has its LENGTH held.
        crate::state::provisioner::rds_snapshots::RDS_DB_SNAPSHOT => {
            rules(255, Characters::AsGenerated)
        }
        // `CreateDBClusterSnapshot.DBClusterSnapshotIdentifier`,
        // `CreateDBClusterEndpoint.DBClusterEndpointIdentifier` and
        // `CreateGlobalCluster.GlobalClusterIdentifier`: each "must contain from 1 to 63
        // letters, numbers, or hyphens", "first character must be a letter" and "can't end
        // with a hyphen or contain two consecutive hyphens" (those members' own docs), and
        // each states "this parameter is stored as a lowercase string" — so a name carrying
        // upper case names a resource that service does not hold under it, the position the
        // group names above are in.
        crate::state::provisioner::rds_snapshots::RDS_CLUSTER_SNAPSHOT
        | crate::state::provisioner::rds_cluster_endpoints::RDS_DB_CLUSTER_ENDPOINT
        | crate::state::provisioner::rds_global_clusters::RDS_GLOBAL_CLUSTER
        // Amazon Neptune's global database is the same member of the same call.
        | crate::state::provisioner::neptune_relational::NEPTUNE_GLOBAL_CLUSTER => {
            rules(63, Characters::LowercaseAlphanumeric)
        }
        // Amazon RDS `CreateEventSubscription.SubscriptionName`: "The name of the
        // subscription", whose one published constraint is "The name must be less than 255
        // characters" (that member's own doc) — RDS's model carries no `@length` for it. That
        // member states no case folding, so a generated name is held as generated and only the
        // LENGTH is a bound one could break.
        crate::state::provisioner::rds_event_subscriptions::RDS_EVENT_SUBSCRIPTION => {
            rules(254, Characters::AsGenerated)
        }
        // Amazon RDS `CreateDBProxy.DBProxyName`: `@length(min: 1, max: 63)` with pattern
        // `^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$` (`smithy_query(query="operation",
        // operation="CreateDBProxy")`) — which, unlike the identifiers above, is NOT folded
        // to lower case, and admits only the letters, digits and single hyphens a generated
        // name is already built from; the bound is what such a name may break.
        RDS_DB_PROXY => rules(63, Characters::AsGenerated),
        // Amazon Redshift `CreateCluster.ClusterIdentifier`: "Must contain from 1 to 63
        // alphanumeric characters or hyphens", "Alphabetic characters must be lowercase",
        // "Cannot end with a hyphen or contain two consecutive hyphens" (that member's own
        // doc) — Redshift's model carries no `@length` for it.
        REDSHIFT_CLUSTER => rules(63, Characters::LowercaseAlphanumeric),
        // Amazon Redshift `CreateClusterSubnetGroup.ClusterSubnetGroupName`: "Must contain
        // no more than 255 alphanumeric characters or hyphens" (that member's own doc) —
        // Redshift's model carries no `@length` for it. Mixed case is accepted and folded
        // ("Amazon Redshift stores the value as a lowercase string"), so only the LENGTH is
        // a bound a generated name could break.
        REDSHIFT_CLUSTER_SUBNET_GROUP => rules(255, Characters::AsGenerated),
        // Amazon Redshift `CreateClusterParameterGroup.ParameterGroupName`: "Must be 1 to 255
        // alphanumeric characters or hyphens", "First character must be a letter", "Cannot end
        // with a hyphen or contain two consecutive hyphens" (that member's own doc) —
        // Redshift's model carries no `@length` for it. That member also states "This value is
        // stored as a lower-case string", so a name carrying upper case names a group that
        // service does not hold under it — the position the Amazon RDS group names above are
        // in.
        crate::state::provisioner::redshift_parameter_groups::REDSHIFT_CLUSTER_PARAMETER_GROUP => {
            rules(255, Characters::LowercaseAlphanumeric)
        }
        // Amazon Redshift `CreateSnapshotCopyGrant.SnapshotCopyGrantName`: "Must contain from 1
        // to 63 alphanumeric characters or hyphens", "Alphabetic characters must be lowercase",
        // "First character must be a letter", "Cannot end with a hyphen or contain two
        // consecutive hyphens" (that member's own doc) — Redshift's model carries no `@length`
        // for it. Lower case is required of the value itself, so a name carrying upper case
        // names a grant that service does not hold — the position the group names above are in.
        // The three remaining constraints need no further rule here: [`within`] gives this
        // character set a name that begins and ends with an alphanumeric and collapses a run
        // of separators to ONE hyphen, and that leading alphanumeric is a LETTER because a
        // stack name is `[a-zA-Z][-a-zA-Z0-9]*`, so only the LENGTH is a bound a generated
        // name could break.
        crate::state::provisioner::redshift_snapshot_copy_grants::REDSHIFT_SNAPSHOT_COPY_GRANT => {
            rules(63, Characters::LowercaseAlphanumeric)
        }
        // Amazon Redshift `CreateEventSubscription.SubscriptionName`: "Cannot be null, empty, or
        // blank", "Must contain from 1 to 255 alphanumeric characters or hyphens", "First
        // character must be a letter", "Cannot end with a hyphen or contain two consecutive
        // hyphens" (that member's own doc) — Redshift's model carries no `@length` for it. That
        // member states no case folding, so a generated name is held as generated and only the
        // LENGTH is a bound one could break, for the reason the grant's entry above records.
        crate::state::provisioner::redshift_event_subscriptions::REDSHIFT_EVENT_SUBSCRIPTION => {
            rules(255, Characters::AsGenerated)
        }
        // Amazon EC2 `CreateSecurityGroup.GroupName`: "Up to 255 characters in length"
        // (Amazon EC2 API reference) — EC2's model carries no `@length` for it.
        EC2_SECURITY_GROUP => rules(255, Characters::AsGenerated),
        // AWS Glue `CreateJob.Name`: `NameString`, `@length(min: 1, max: 255)` with pattern
        // `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$` (AWS Glue's own
        // Smithy model), which a generated name is already inside — every character one is
        // built from is printable — but may be longer than.
        "AWS::Glue::Job" => rules(255, Characters::AsGenerated),
        // The same `NameString` bound AWS Glue holds every entity of its own to:
        // `CreateDatabase.DatabaseInput$Name`, `CreateTable.TableInput$Name`,
        // `CreateCrawler.Name` and `CreateTrigger.Name` are all `@length(min: 1, max: 255)`
        // with that same printable pattern (AWS Glue's own Smithy model).
        GLUE_DATABASE | GLUE_TABLE | GLUE_CRAWLER => rules(255, Characters::AsGenerated),
        crate::state::provisioner::glue_triggers::GLUE_TRIGGER => {
            rules(255, Characters::AsGenerated)
        }
        // Amazon EventBridge `PutRule.Name`: `RuleName`, `@length(min: 1, max: 64)` with
        // pattern `^[\.\-_A-Za-z0-9]+$` (EventBridge's own Smithy model), which a
        // generated name is already inside but may be longer than.
        EVENTS_RULE => rules(64, Characters::AsGenerated),
        // Systems Manager `PutParameter.Name`: `PSParameterName`,
        // `@length(min: 1, max: 2048)` (the SSM model's own bound).
        SSM_PARAMETER => rules(2048, Characters::AsGenerated),
        // Systems Manager `CreateDocument.Name`: `DocumentName`, pattern
        // `^[a-zA-Z0-9_\-.]{3,128}$` (that same model), which admits the letters, digits and
        // hyphens a generated name is built from but is shorter than one may be.
        crate::state::provisioner::ssm_documents::SSM_DOCUMENT => {
            rules(128, Characters::AsGenerated)
        }
        // EventBridge Scheduler `CreateScheduleGroup.Name` (`ScheduleGroupName`) and
        // `CreateSchedule.Name` (`Name`): each `@length(min: 1, max: 64)` with pattern
        // `^[0-9a-zA-Z-_.]+$` (that service's own Smithy model), which a generated name is
        // already inside but may be longer than.
        SCHEDULER_SCHEDULE_GROUP | SCHEDULER_SCHEDULE => rules(64, Characters::AsGenerated),
        // Amazon Cognito `CreateGroup.GroupName`: `GroupNameType`,
        // `@length(min: 1, max: 128)` with pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`
        // (cognito-idp's own Smithy model), which a generated name is already inside —
        // letters, digits and the hyphen are all admitted — but may be longer than.
        COGNITO_USER_POOL_GROUP => rules(128, Characters::AsGenerated),
        // Amazon Cognito `CreateUserPool.PoolName`: `UserPoolNameType`,
        // `@length(min: 1, max: 128)` with pattern `^[\w\s+=,.@-]+$` (cognito-idp's own
        // Smithy model), which a generated name is already inside — letters, digits and the
        // hyphen are all admitted — but may be longer than.
        COGNITO_USER_POOL => rules(128, Characters::AsGenerated),
        // The same service's `CreateIdentityProvider.ProviderName`: `ProviderNameTypeV2`,
        // `@length(min: 1, max: 32)` (that same model) — the tightest of the three, and one a
        // generated name would otherwise break.
        COGNITO_USER_POOL_IDENTITY_PROVIDER => rules(32, Characters::AsGenerated),
        // The same service's `CreateUserPoolDomain.Domain`: `DomainType`,
        // `@length(min: 1, max: 63)` with pattern `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`
        // (that same model) — a DNS label, so a generated name has to be reduced to the
        // lowercase alphanumerics and hyphens it admits as well as fitted to its length.
        COGNITO_USER_POOL_DOMAIN => rules(63, Characters::LowercaseAlphanumeric),
        // The same service's `CreateResourceServer.Identifier`:
        // `ResourceServerIdentifierType`, `@length(min: 1, max: 256)` with pattern
        // `^[\x21\x23-\x5B\x5D-\x7E]+$` (that same model), which a generated name is already
        // inside — every character one is built from is printable and is neither a quotation
        // mark nor a backslash — but may be longer than.
        COGNITO_USER_POOL_RESOURCE_SERVER => rules(256, Characters::AsGenerated),
        // Elastic Beanstalk `CreateApplication.ApplicationName` (`ApplicationName`),
        // `CreateApplicationVersion.VersionLabel` (`VersionLabel`) and
        // `CreateConfigurationTemplate.TemplateName` (`ConfigurationTemplateName`): each
        // `@length(min: 1, max: 100)` in that service's own Smithy model.
        ELASTIC_BEANSTALK_APPLICATION
        | ELASTIC_BEANSTALK_APPLICATION_VERSION
        | ELASTIC_BEANSTALK_CONFIGURATION_TEMPLATE => rules(100, Characters::AsGenerated),
        // CodeDeploy `CreateApplication.applicationName` (`ApplicationName`),
        // `CreateDeploymentGroup.deploymentGroupName` (`DeploymentGroupName`) and
        // `CreateDeploymentConfig.deploymentConfigName` (`DeploymentConfigName`): each
        // `@length(min: 1, max: 100)` with pattern `^[A-Za-z0-9+=,.@_-]*$` (CodeDeploy's own
        // Smithy model), which a generated name is already inside — letters, digits and the
        // hyphen are all admitted — but may be longer than.
        CODEDEPLOY_APPLICATION | CODEDEPLOY_DEPLOYMENT_GROUP | CODEDEPLOY_DEPLOYMENT_CONFIG => {
            rules(100, Characters::AsGenerated)
        }
        // Elastic Beanstalk `CreateEnvironment.EnvironmentName`: `EnvironmentName`,
        // `@length(min: 4, max: 40)` (that service's own Smithy model) — the tightest
        // bound of the three, and one a generated name would otherwise break.
        ELASTIC_BEANSTALK_ENVIRONMENT => rules(40, Characters::AsGenerated),
        // Amazon MSK `CreateCluster.clusterName` and `CreateClusterV2.clusterName`:
        // `__stringMin1Max64`, `@length(min: 1, max: 64)` (that service's own Smithy
        // model), which a generated name is already inside but may be longer than.
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => rules(64, Characters::AsGenerated),
        // Amazon MSK `CreateConfiguration.name`: that service admits a configuration name of
        // 1 to 128 characters built from ASCII alphanumerics and `.`, `_` and `-`
        // (`crate::state::provisioner::MSK_CONFIGURATION_NAME_LENGTH` records where the bound
        // is read from), which a generated name is already inside — letters, digits and the
        // hyphen are all admitted — but may be longer than.
        MSK_CONFIGURATION => rules(
            crate::state::provisioner::MSK_CONFIGURATION_NAME_LENGTH,
            Characters::AsGenerated,
        ),
        // AWS Backup `CreateBackupVault.BackupVaultName`: `BackupVaultName`, pattern
        // `^[a-zA-Z0-9\-\_]{2,50}$` (that service's own Smithy model), which admits the
        // letters, digits and hyphens a generated name is built from but is shorter than
        // one may be.
        BACKUP_VAULT => rules(50, Characters::AsGenerated),
        // Amazon MQ `CreateBroker.BrokerName`: that member's own rule is "1-50 characters
        // long, must contain only letters, numbers, dashes, and underscores" (`BrokerName`,
        // Amazon MQ's own Smithy model) — a generated name is already inside that character
        // set, since letters, digits and the hyphen are all admitted, but may be longer
        // than the bound.
        MQ_BROKER => rules(50, Characters::AsGenerated),
        // Amazon MQ `CreateConfiguration.Name`: that member's own rule is "can contain only
        // alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~)" and
        // "must be 1-150 characters long" (`CreateConfigurationRequest$Name`, Amazon MQ's own
        // Smithy model) — a generated name is already inside that character set, since
        // letters, digits and the hyphen are all admitted, but may be longer than the bound.
        MQ_CONFIGURATION => rules(
            crate::state::provisioner::MQ_CONFIGURATION_NAME_LENGTH,
            Characters::AsGenerated,
        ),
        // Amazon OpenSearch Service `CreateDomain.DomainName`: `DomainName`,
        // `@length(min: 3, max: 28)` with pattern `^[a-z][a-z0-9\-]+$` (that service's own
        // Smithy model), which admits no upper case and no character a generated name
        // carries beyond letters, digits and the hyphen.
        OPENSEARCH_DOMAIN => rules(28, Characters::LowercaseAlphanumeric),
        // Amazon MWAA `CreateEnvironment.Name`: `EnvironmentName`,
        // `@length(min: 1, max: 80)` with pattern `^[a-zA-Z][0-9a-zA-Z-_]*$` (that service's
        // own Smithy model), which a generated name is already inside — letters, digits and
        // the hyphen are all admitted, and it starts with a letter because a stack name does
        // — but may be longer than.
        MWAA_ENVIRONMENT => rules(80, Characters::AsGenerated),
        // Amazon Kinesis Video Streams `CreateStream.StreamName` (`StreamName`) and
        // `CreateSignalingChannel.ChannelName` (`ChannelName`): each `@length(min: 1, max: 256)`
        // with pattern `^[a-zA-Z0-9_.-]+$` (that service's own Smithy model), which admits every
        // character a generated name carries — letters, digits and the hyphen — and bounds it at
        // 256.
        KINESISVIDEO_STREAM | KINESISVIDEO_SIGNALING_CHANNEL => {
            rules(256, Characters::AsGenerated)
        }
        // CloudWatch Application Signals `CreateServiceLevelObjective.Name`:
        // `ServiceLevelObjectiveName`, pattern
        // `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$` (that service's own Smithy model),
        // which admits every character a generated name carries — letters, digits and the
        // hyphen — and bounds it at 128.
        APPLICATION_SIGNALS_SERVICE_LEVEL_OBJECTIVE => rules(128, Characters::AsGenerated),
        // Amazon Connect Customer Profiles `CreateDomain.DomainName`: `name`,
        // `@length(min: 1, max: 64)` with pattern `^[a-zA-Z0-9_-]+$` (that service's own
        // Smithy model), which a generated name is already inside — letters, digits and the
        // hyphen are all admitted — but may be longer than.
        CUSTOMER_PROFILES_DOMAIN => rules(64, Characters::AsGenerated),
        // Amplify UI Builder `CreateComponent.componentToCreate.name` (`ComponentName`),
        // `CreateForm.formToCreate.name` (`FormName`) and `CreateTheme.themeToCreate.name`
        // (`ThemeName`): each `@length(min: 1, max: 255)` with no pattern (that service's own
        // Smithy model), so a generated name is already inside the characters they admit but
        // may be longer than the bound.
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_FORM | AMPLIFYUIBUILDER_THEME => {
            rules(255, Characters::AsGenerated)
        }
        // AWS Amplify `CreateApp.name`: `Name`, `@length(min: 1, max: 255)` with pattern
        // `^(?s).+$` (that service's own Smithy model), which admits every character a
        // generated name carries but is shorter than one may be.
        AMPLIFY_APP => rules(255, Characters::AsGenerated),
        // AWS X-Ray `CreateSamplingRule.SamplingRule.RuleName`: `RuleName`,
        // `@length(min: 1, max: 32)` (X-Ray's own Smithy model), which a generated name is
        // already inside — letters, digits and the hyphen are all admitted — but is far
        // shorter than one may be.
        XRAY_SAMPLING_RULE => rules(32, Characters::AsGenerated),
        // AWS X-Ray `CreateGroup.GroupName`: `GroupName`, `@length(min: 1, max: 32)`
        // (X-Ray's own Smithy model) — the same bound a sampling rule's name carries, and
        // one a generated name is already inside the characters of but far longer than.
        XRAY_GROUP => rules(32, Characters::AsGenerated),
        // Neptune Analytics `CreateGraph.graphName` (`GraphName`) and
        // `CreateGraphSnapshot.snapshotName` (`SnapshotName`): each `@length(min: 1, max: 63)`
        // with pattern `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$` / `^(?!gs-)…` (that service's own
        // Smithy model), which admits no upper case, requires a leading letter and admits no
        // character a generated name carries beyond letters, digits and the hyphen.
        NEPTUNE_GRAPH_GRAPH | NEPTUNE_GRAPH_GRAPH_SNAPSHOT => {
            rules(63, Characters::LowercaseAlphanumeric)
        }
        // Amazon Redshift Serverless `CreateNamespace.namespaceName` (`NamespaceName`) and
        // `CreateWorkgroup.workgroupName` (`WorkgroupName`): each `@length(min: 3, max: 64)`
        // with pattern `^[a-z0-9-]+$` (that service's own Smithy model), which admits no upper
        // case and no character a generated name carries beyond letters, digits and the hyphen.
        // (A SNAPSHOT's name is a plain `String` on that service's own `CreateSnapshot`, with
        // neither bound nor pattern, so a generated name needs no fitting at all.)
        crate::state::provisioner::redshift_serverless::REDSHIFT_SERVERLESS_NAMESPACE
        | crate::state::provisioner::redshift_serverless::REDSHIFT_SERVERLESS_WORKGROUP => {
            rules(64, Characters::LowercaseAlphanumeric)
        }
        // Amazon AppFlow `CreateConnectorProfile.connectorProfileName`: `ConnectorProfileName`,
        // `@length(min: 0, max: 256)` with pattern `^[\w/!@#+=.-]+$` (that service's own Smithy
        // model), which a generated name is already inside — letters, digits, the underscore
        // and the hyphen are all admitted — but may be longer than.
        crate::state::provisioner::appflow_connector_profiles::APPFLOW_CONNECTOR_PROFILE => {
            rules(256, Characters::AsGenerated)
        }
        _ => None,
    }
}

/// The physical name CloudFormation generates for a resource the template does not name:
/// `<stack name>-<logical id>-<unique id>`, fitted to the naming rules of the service that
/// owns the type ([`naming_rules`]).
///
/// The unique segment is derived from the stack id and the logical id rather than drawn
/// fresh, so it is stable across reads and across a later update of the same stack — the
/// physical id of a resource that was not replaced does not change. It is also what keeps
/// two resources of one stack (and two stacks of one name) apart, so it is never what the
/// fitting gives up: the stack name and the logical id are what lose characters, and they
/// lose them from the FULL logical id and stack name the unique segment was derived from,
/// so two resources whose shortened segments coincide still carry different names.
///
/// A type's minimum name length needs nothing here: the unique segment alone is
/// [`GENERATED_NAME_DIGEST_CHARS`] characters, longer than any of the minimums in
/// [`naming_rules`] (3 for a bucket or a table, 2 for a repository), and it is kept whole.
///
/// `suffix` is what the owning service requires on the name of the variant the declaration
/// asked for ([`required_name_suffix`]). It is part of the name, so the limit is fitted
/// around it, and it is appended after the character set is applied — a suffix the owning
/// service publishes is spelled the way that service spells it.
pub(super) fn generated_name(
    resource_type: &str,
    logical_id: &str,
    scope: StackScope<'_>,
    suffix: &str,
) -> String {
    let unique = derived_digits(
        scope.stack_id,
        logical_id,
        GENERATED_NAME_DIGEST_CHARS,
        Alphabet::Upper,
    );
    let Some(rules) = naming_rules(resource_type) else {
        return format!("{}-{logical_id}-{unique}{suffix}", scope.stack_name);
    };
    // The characters the limit leaves for the generated name itself, the required suffix
    // taken off it first.
    let limit = rules.max_characters.saturating_sub(suffix.chars().count());
    // The room the stack name and the logical id have BETWEEN them: that limit, less the
    // unique segment and the two hyphens that join the three segments.
    let room = limit.saturating_sub(unique.chars().count() + GENERATED_NAME_SEPARATORS);
    let name = if room < 2 {
        // No owning service publishes a limit with no room for both segments; the unique
        // segment is what keeps the name unique, so it is what a limit that small leaves.
        unique.chars().take(limit).collect()
    } else {
        let (stack_name, shortened) = shortened_to(scope.stack_name, logical_id, room);
        format!("{stack_name}-{shortened}-{unique}")
    };
    format!("{}{suffix}", within(name, rules.characters))
}

/// The two hyphens that join the three segments of a [`generated_name`].
pub(super) const GENERATED_NAME_SEPARATORS: usize = 2;

/// `stack_name` and `logical_id` shortened to `room` characters between them, each keeping
/// as much of itself as that allows.
///
/// Characters come off whichever of the two is LONGER, so a long stack name does not eat
/// the room the logical id needs to stay recognisable and neither segment is spent while
/// the other still has more to give. `room` is at least 2, so each keeps at least its first
/// character.
///
/// A segment the shortening CUT gives up a trailing hyphen too: a stack name may carry one
/// (`[a-zA-Z][-a-zA-Z0-9]*`), and a cut landing just after it would otherwise put two
/// hyphens together in the name — which several owning services' names may not carry (an
/// Elastic Load Balancing name and an Amazon RDS identifier among them). A segment the
/// shortening did not touch is left exactly as it is, so a name that needed no fitting is
/// the name CloudFormation builds.
pub(super) fn shortened_to(stack_name: &str, logical_id: &str, room: usize) -> (String, String) {
    let mut stack: Vec<char> = stack_name.chars().collect();
    let mut logical: Vec<char> = logical_id.chars().collect();
    let (full_stack, full_logical) = (stack.len(), logical.len());
    while stack.len() + logical.len() > room {
        if stack.len() >= logical.len() {
            stack.pop();
        } else {
            logical.pop();
        }
    }
    let cut = |mut segment: Vec<char>, was: usize| -> String {
        if segment.len() < was {
            while segment.len() > 1 && segment.ends_with(&['-']) {
                segment.pop();
            }
        }
        segment.into_iter().collect()
    };
    (cut(stack, full_stack), cut(logical, full_logical))
}

/// `name` reduced to the characters `characters` admits.
///
/// Length is settled before this runs ([`generated_name`] fits the segments to the limit),
/// and nothing here lengthens a name: a character outside the set becomes one inside it,
/// and the edges are only trimmed.
pub(super) fn within(name: String, characters: Characters) -> String {
    match characters {
        Characters::AsGenerated => name,
        Characters::AlphanumericUnderscore => {
            // Letters, digits and the underscore, with every character outside the set —
            // the two hyphens that join a generated name's segments among them — replaced
            // by an underscore, which is in it, rather than dropped, so two names that
            // differ only in such a character cannot become one. A RUN of them becomes ONE
            // underscore, and the edges are trimmed, so the name carries no leading or
            // trailing separator either.
            let mut within_the_set = String::with_capacity(name.len());
            for character in name.chars() {
                if character.is_ascii_alphanumeric() {
                    within_the_set.push(character);
                } else if !within_the_set.ends_with('_') {
                    within_the_set.push('_');
                }
            }
            within_the_set.trim_matches('_').to_string()
        }
        Characters::LowercaseAlphanumeric => {
            // Lower case, with every character outside the set replaced by a hyphen —
            // which is in it — rather than dropped, so two names that differ only in such
            // a character cannot become one. A RUN of them becomes ONE hyphen, because a
            // repository name's pattern separates its alphanumeric groups by a single
            // separator, and the edges are trimmed to the alphanumeric both a bucket name
            // and a repository name have to begin and end with.
            let mut within_the_set = String::with_capacity(name.len());
            for character in name.chars().map(|character| character.to_ascii_lowercase()) {
                if character.is_ascii_lowercase() || character.is_ascii_digit() {
                    within_the_set.push(character);
                } else if !within_the_set.ends_with('-') {
                    within_the_set.push('-');
                }
            }
            within_the_set.trim_matches('-').to_string()
        }
    }
}

/// Which alphabet a derived identifier's digits are drawn from — the ONE thing the
/// identifier forms below differ in.
#[derive(Debug, Clone, Copy)]
pub(super) enum Alphabet {
    /// Lowercase hexadecimal, the alphabet of an EC2/Transfer id and a DNS label.
    Lower,
    /// Uppercase hexadecimal, the alphabet of CloudFormation's own generated name
    /// segment and of Amazon RDS's immutable resource ids.
    Upper,
}

/// `digits` characters derived from `salt` and `name`, in `alphabet`.
///
/// The ONE derivation every identifier this module has to invent is built on: a SHA-256
/// digest of `<salt>/<name>`, rendered as hexadecimal. It is a function of the resource's
/// own name — itself derived from the stack id and the logical id — so every identifier
/// built from it is stable across reads and across a later update of the same stack, and
/// two resources of one stack never collide. The `salt` keeps two identifiers OF ONE
/// resource (its endpoint label and its resource id, say) from being the same digits.
pub(super) fn derived_digits(salt: &str, name: &str, digits: usize, alphabet: Alphabet) -> String {
    let finished = salted_digest(salt, name);
    let digest = match alphabet {
        Alphabet::Lower => ::hex::encode(finished),
        Alphabet::Upper => ::hex::encode_upper(finished),
    };
    digest[..digits.min(digest.len())].to_string()
}

/// `digits` characters derived from `salt` and `name`, drawn from `alphabet`.
///
/// [`derived_digits`] renders the one digest as hexadecimal, which is the alphabet most
/// service-assigned identifiers are written over. A few are written over another one — a
/// CloudHSM cluster id is `cluster-` and eleven characters of `[2-7a-zA-Z]`, which carries no
/// `0`, `1`, `8` or `9` — so each byte of the SAME digest ([`salted_digest`], which both
/// derivations take it from) is folded into the alphabet that identifier's own pattern admits
/// instead, which keeps the value inside the form the owning service assigns while staying as
/// stable as every other derived identifier here.
///
/// An EMPTY alphabet draws nothing, which is the empty string rather than a division by zero.
pub(super) fn derived_over(salt: &str, name: &str, digits: usize, alphabet: &[u8]) -> String {
    if alphabet.is_empty() {
        return String::new();
    }
    salted_digest(salt, name)
        .into_iter()
        .take(digits)
        .map(|byte| char::from(alphabet[usize::from(byte) % alphabet.len()]))
        .collect()
}

/// The digest both derivations above are built on: SHA-256 over `<salt>/<name>`, in ONE place
/// so the two cannot come to hash different things.
fn salted_digest(salt: &str, name: &str) -> [u8; 32] {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    hasher.finalize().into()
}

/// `digits` lowercase hexadecimal characters derived from `content` under `salt`.
///
/// [`derived_digits`] is the crate's ONE digest derivation; this is the name it is reached
/// under from outside this module, for the content hash a macro expansion puts on the
/// logical id of a resource that cannot be updated in place (see
/// [`super::sam`](crate::state::sam)).
pub(crate) fn content_digest(salt: &str, content: &str, digits: usize) -> String {
    derived_digits(salt, content, digits, Alphabet::Lower)
}

/// How many characters of the digest the unique segment of a [`generated_name`] carries.
///
/// Enough that two resources of one stack — and two stacks of one name — cannot collide,
/// and short enough to leave room for the stack name and the logical id inside a name an
/// owning service bounds (see [`naming_rules`]).
pub(super) const GENERATED_NAME_DIGEST_CHARS: usize = 12;

/// The name CloudFormation generates for the REPLACEMENT of a resource whose name it
/// generated itself: the name in force with a fresh segment of its own.
///
/// A generated name is stable across reads and across an update that does not recreate the
/// resource ([`generated_name`] derives it from the stack and the logical id), which is what
/// keeps the identifier of a resource that was not replaced where it is. A REPLACEMENT is the
/// other case: the resource being replaced still holds the name it was created under while the
/// replacement is created — and under an `UpdateReplacePolicy` of `Retain` it holds it for good
/// — and an owning service refuses a second resource under a name it already has (a security
/// group name is unique in its VPC, a bucket name worldwide). So the replacement is created
/// under a name of its own.
///
/// The segment is derived from the name in force and the declaration being deployed, so the
/// same deployment asked for twice names the same replacement — nothing here is drawn fresh per
/// call — while a further deployment that replaces the resource again names another. The result
/// is fitted to the owning service's own naming rules, exactly as the first generated name was:
/// the stem gives up the characters the fresh segment and its separator need.
pub fn replacement_name(resource_type: &str, name_in_force: &str, declared: &Value) -> String {
    let unique = content_digest(
        name_in_force,
        &declared.to_string(),
        GENERATED_NAME_DIGEST_CHARS,
    );
    let rules = naming_rules(resource_type);
    let limit = rules.map_or(MAX_REPLACEMENT_NAME_CHARS, |rules| rules.max_characters);
    let room = limit.saturating_sub(unique.chars().count() + 1);
    let stem: String = name_in_force.chars().take(room).collect();
    let name = format!("{stem}-{unique}");
    match rules {
        Some(rules) => within(name, rules.characters),
        None => name,
    }
}

/// The most characters a [`replacement_name`] carries for a type whose owning service
/// publishes no bound a generated name could break ([`naming_rules`]): the widest bound in
/// that catalogue, which every such name is already inside.
const MAX_REPLACEMENT_NAME_CHARS: usize = 255;

/// A service-generated identifier of the form `<prefix>-<digits hexadecimal characters>`.
///
/// Some resource types do not take a name from the template at all: the owning service
/// ASSIGNS the identifier, in a documented form (an AWS Transfer Family server is
/// `s-` + 17 hexadecimal characters, which is the `ServerId` pattern
/// `^s-([0-9a-f]{17})$` in Transfer's own Smithy model). CloudFormation's
/// `<stack name>-<logical id>-<unique id>` name is not such an identifier, so a resource
/// of one of those types reports an identifier of ITS form instead — see
/// [`derived_digits`] for why it is stable.
pub(super) fn generated_service_id(prefix: &str, name: &str, digits: usize) -> String {
    format!(
        "{prefix}-{}",
        derived_digits(prefix, name, digits, Alphabet::Lower)
    )
}

/// A service-assigned resource id of `digits` UPPERCASE characters after `prefix`.
///
/// Amazon RDS's immutable resource ids (`DbClusterResourceId`, `DbiResourceId`) are
/// published as `cluster-`/`db-` followed by 26 uppercase alphanumeric characters, which
/// is a different alphabet from [`generated_service_id`]'s lowercase hexadecimal; the
/// derivation is the same one.
pub(super) fn generated_upper_resource_id(prefix: &str, name: &str, digits: usize) -> String {
    format!(
        "{prefix}-{}",
        derived_digits(prefix, name, digits, Alphabet::Upper)
    )
}

/// A label of `digits` characters the OWNING SERVICE assigns inside an identifier of its
/// own, under `salt`.
///
/// Two kinds of published identifier carry one: the middle label of an Amazon RDS endpoint
/// DNS name (`<identifier>.cluster-<label>.<region>.rds.amazonaws.com`) and the suffix of a
/// Secrets Manager secret ARN (`:secret:<name>-<suffix>`). Each passes its own `salt`, so
/// one resource's endpoint label and ARN suffix are different digits.
pub(super) fn service_assigned_label(salt: &str, name: &str, digits: usize) -> String {
    derived_digits(salt, name, digits, Alphabet::Lower)
}

/// The port the database engine a DB cluster or DB instance declares listens on, or `None`
/// where the declaration does not say.
///
/// `AWS::RDS::DBCluster`/`AWS::RDS::DBInstance` publish `Endpoint.Port` as a return value:
/// the port a template DECLARES fixes it, and otherwise it is the declared engine's
/// documented default (Amazon RDS: "RDS for MySQL - 3306, RDS for PostgreSQL - 5432, RDS
/// for Oracle - 1521, RDS for SQL Server - 1433, RDS for Db2 - 50000"). The engine name is
/// the prefix the property's own value list uses (`aurora-postgresql`, `sqlserver-ee`), so
/// the family is matched on the prefix.
///
/// Both properties are evaluated through the template's intrinsics, exactly as
/// [`physical_name`] evaluates a declared name, so `Port: !Ref DBPort` is the port and
/// `Engine: !Ref EngineName` the engine. A declaration whose ENGINE cannot be resolved
/// yields NO port rather than one engine's default standing in for an unknown engine's.
pub(super) fn database_port(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Option<String> {
    if let Some(port) = properties
        .get("Port")
        .and_then(|value| text(value, scope, 0, budget))
    {
        return Some(port);
    }
    let engine = properties
        .get("Engine")
        .and_then(|value| text(value, scope, 0, budget))?
        .to_ascii_lowercase();
    Some(
        if engine.contains("db2") {
            "50000"
        } else if engine.contains("sqlserver") {
            "1433"
        } else if engine.contains("oracle") {
            "1521"
        } else if engine.contains("postgres") {
            "5432"
        } else {
            // mysql, mariadb, aurora-mysql and the Aurora MySQL aliases.
            "3306"
        }
        .to_string(),
    )
}

/// The property a resource type takes its NAME from when that property is neither the
/// type's last segment plus `Name` nor plain `Name`, which is what [`physical_name`]
/// reads by convention.
///
/// A CloudFormation resource type's name property is the name member of the API resource
/// it wraps, and a few APIs spell that member with the resource's full noun: the Bedrock
/// AgentCore control plane's `CreateAgentRuntime` takes `agentRuntimeName`, so
/// `AWS::BedrockAgentCore::Runtime` takes `AgentRuntimeName` — the type's last segment is
/// `Runtime`, so neither `RuntimeName` nor `Name` finds it. The same for
/// `AWS::ECR::PublicRepository`, whose property is `RepositoryName` ("The name to use for
/// the public repository") while the type's last segment is `PublicRepository`.
///
/// This is the OTHER HALF of the per-type catalogue [`return_values`] below keys: a type
/// whose name property is unconventional has to be registered here AND there. The two are
/// separate functions only because the name is needed BEFORE the return values are
/// computed from it ([`provision`] calls [`physical_name`] first), so they cannot share one
/// `match`; keep them adjacent, and fold both into one per-type descriptor returned by a
/// single lookup when this list grows enough to make that worth it.
pub(super) fn declared_name_property(resource_type: &str) -> Option<&'static str> {
    match resource_type {
        // IAM Access Analyzer's `CreateArchiveRule` takes the rule's own name as its `ruleName`
        // member — the name every call of that service on a rule addresses it by — so
        // `AWS::AccessAnalyzer::ArchiveRule` publishes `RuleName`, where the type's last
        // segment is `ArchiveRule`: neither `ArchiveRuleName` nor `Name` finds it, and a
        // generated `<stack>-<logical id>-<unique id>` would name no rule that service holds.
        // (The ANALYZER's own `AnalyzerName` IS the convention, its last segment being
        // `Analyzer`.)
        crate::state::provisioner::access_analyzer::ACCESSANALYZER_ARCHIVE_RULE => {
            Some(crate::state::provisioner::access_analyzer::RULE_NAME_PROPERTY)
        }
        "AWS::BedrockAgentCore::Runtime" => Some("AgentRuntimeName"),
        // Amazon Bedrock Data Automation's `CreateDataAutomationProject` takes a `projectName`
        // and `CreateDataAutomationLibrary` a `libraryName`, so each type takes `ProjectName` /
        // `LibraryName`, where the types' last segments are `DataAutomationProject` and
        // `DataAutomationLibrary`: neither `DataAutomationProjectName` nor `Name` finds either,
        // and a generated `<stack>-<logical id>-<unique id>` would name no resource that service
        // holds. (A BLUEPRINT needs no row: its property is `BlueprintName`, which the
        // convention already finds.)
        BEDROCK_DATA_AUTOMATION_PROJECT => Some("ProjectName"),
        BEDROCK_DATA_AUTOMATION_LIBRARY => Some("LibraryName"),
        // Amazon Bedrock's `CreateInferenceProfile` takes an `inferenceProfileName`, so
        // `AWS::Bedrock::ApplicationInferenceProfile` takes `InferenceProfileName`, where the
        // type's last segment is `ApplicationInferenceProfile`: neither
        // `ApplicationInferenceProfileName` nor `Name` finds it.
        BEDROCK_APPLICATION_INFERENCE_PROFILE => Some("InferenceProfileName"),
        // Amazon RDS takes a cluster snapshot's own identifier as
        // `CreateDBClusterSnapshot.DBClusterSnapshotIdentifier` — the identifier this type's
        // `Ref` returns and every call on the snapshot takes — where the type's last segment
        // is `ClusterSnapshot`, so neither `ClusterSnapshotName`, `Name` nor
        // `ClusterSnapshotIdentifier` finds it.
        crate::state::provisioner::rds_snapshots::RDS_CLUSTER_SNAPSHOT => {
            Some("DBClusterSnapshotIdentifier")
        }
        // The same service's `CreateEventSubscription` takes a `SubscriptionName` — "the name of
        // the subscription", and "Ref returns the subscription name" — and the type publishes it,
        // so the name the template states is the subscription's. The type's last segment is
        // `EventSubscription`, so neither `EventSubscriptionName` nor `Name` finds it, and a
        // generated `<stack>-<logical id>-<unique id>` would name no subscription that service
        // holds.
        crate::state::provisioner::rds_event_subscriptions::RDS_EVENT_SUBSCRIPTION => {
            Some(crate::state::provisioner::rds_event_subscriptions::SUBSCRIPTION_NAME_MEMBER)
        }
        // The same service's `PutResourcePolicy` identifies an ACCOUNT-scoped policy by its
        // `policyName` — "Ref returns the policy name" — so the type publishes `PolicyName`;
        // the type's last segment is `ResourcePolicy`, so neither `ResourcePolicyName` nor
        // `Name` finds it. That name is what the policy is PUT under and what every call on
        // one takes, so it is the physical id: a generated `<stack>-<logical id>-<unique id>`
        // would name no policy the declaration asked for.
        crate::state::provisioning::logs_resources::LOGS_RESOURCE_POLICY => Some("PolicyName"),
        // AWS IoT's `CreateProvisioningTemplate` takes a `templateName` — "Ref returns the
        // provisioning template name" — so the type publishes `TemplateName`; the type's last
        // segment is `ProvisioningTemplate`, so neither `ProvisioningTemplateName` nor `Name`
        // finds it. That name is what the template is CREATED under and the identifier every
        // call on one takes, so it is the physical id.
        crate::state::provisioning::iot_provisioning_resources::IOT_PROVISIONING_TEMPLATE => {
            Some("TemplateName")
        }
        // The same service's `CreateRoleAlias` takes the alias's own text as its `roleAlias`
        // member — "Ref returns the role alias" — so the type publishes `RoleAlias`; the
        // type's last segment is `RoleAlias`, so neither `RoleAliasName` nor `Name` finds it.
        crate::state::provisioning::iot_provisioning_resources::IOT_ROLE_ALIAS => {
            Some("RoleAlias")
        }
        // A monitoring subscription is identified by the DISTRIBUTION it is made on: every
        // call on one binds that `DistributionId` as its URI label, CloudFront mints no
        // identifier for the subscription itself, and a distribution has at most one — so the
        // id the resource carries is the distribution's, which is what "Ref returns" for this
        // type and what its create, read and delete each take.
        CLOUDFRONT_MONITORING_SUBSCRIPTION => Some("DistributionId"),
        // API Gateway's `CreateDomainName` takes the `DomainName` the resource IS, on both
        // of its control planes, and that name is what either type's `Ref` returns — the
        // type's last segment is `DomainName`, so neither `DomainNameName` nor `Name` finds
        // it.
        APIGATEWAY_DOMAIN_NAME | APIGATEWAYV2_DOMAIN_NAME => Some("DomainName"),
        // The same service keys a DOCUMENTATION SNAPSHOT by the version itself: "Ref returns
        // the documentation version", `CreateDocumentationVersion$documentationVersion` is the
        // version the snapshot is created under, and every call on one takes that version
        // (`GetDocumentationVersion$documentationVersion`, `Stage$documentationVersion`). The
        // type's last segment is `DocumentationVersion`, so neither `DocumentationVersionName`
        // nor `Name` finds it, and a generated `<stack>-<logical id>-<unique id>` would name
        // no snapshot that service holds.
        APIGATEWAY_DOCUMENTATION_VERSION => Some("DocumentationVersion"),
        // AppSync addresses a custom domain, and the mapping of an API onto it, by the
        // `DomainName` the resource IS: every operation on either binds that name as its
        // `@httpLabel` and the service mints no identifier of its own, so it is the
        // identifier each type publishes — the domain's `Ref` and the mapping's
        // `ApiAssociationIdentifier`. The domain type's last segment is `DomainName` and the
        // mapping's is `DomainNameApiAssociation`, so neither convention finds it.
        crate::state::provisioning::appsync_domain_names::APPSYNC_DOMAIN_NAME
        | crate::state::provisioning::appsync_domain_names::APPSYNC_DOMAIN_NAME_API_ASSOCIATION => {
            Some("DomainName")
        }
        // Amazon ECR registers a pull through cache rule UNDER its prefix: "the repository name
        // prefix to use when caching images from the source registry"
        // (`CreatePullThroughCacheRule$ecrRepositoryPrefix`) is what every call on a rule
        // addresses it by, and "Ref returns the EcrRepositoryPrefix" — so the declared prefix is
        // this resource's physical id. The type's last segment is `PullThroughCacheRule`, so
        // neither `PullThroughCacheRuleName` nor `Name` finds it, and a generated
        // `<stack>-<logical id>-<unique id>` would name a prefix no image reference carries.
        crate::state::provisioning::ecr_pull_through_cache_rule::ECR_PULL_THROUGH_CACHE_RULE => {
            Some("EcrRepositoryPrefix")
        }
        ECR_PUBLIC_REPOSITORY => Some("RepositoryName"),
        // An AWS End User Messaging SMS and Voice RESOURCE POLICY *is* the document attached
        // to another resource: `PutResourcePolicy` takes that resource's `ResourceArn` and
        // mints no identifier of its own, `GetResourcePolicy` and `DeleteResourcePolicy`
        // address the document by the same ARN, and "Ref returns the ResourceArn" — so the
        // declared ARN is this resource's physical id. The type's last segment is
        // `ResourcePolicy`, so neither `ResourcePolicyName` nor `Name` finds it, and a
        // generated `<stack>-<logical id>-<unique id>` would name no resource at all.
        crate::state::provisioner::sms_voice::SMSVOICE_RESOURCE_POLICY => Some("ResourceArn"),
        // An Amazon ECR pull-time-update exclusion IS the IAM principal it excludes:
        // `RegisterPullTimeUpdateExclusion` takes that `principalArn` and no identifier of
        // its own, `DeregisterPullTimeUpdateExclusion` addresses the record by the same ARN,
        // and `ListPullTimeUpdateExclusions` reports nothing else about one — so the declared
        // `PrincipalArn` is this resource's physical id. The type's last segment is
        // `PullTimeUpdateExclusion`, so neither `PullTimeUpdateExclusionName` nor `Name`
        // finds it, and a generated `<stack>-<logical id>-<unique id>` would name no
        // principal that registry can exclude at all.
        ECR_PULL_TIME_UPDATE_EXCLUSION => Some("PrincipalArn"),
        // An AWS Elemental MediaConvert certificate association IS the certificate it trusts:
        // `AssociateCertificate` and `DisassociateCertificate` each take that `Arn` and no
        // identifier of their own, and "Ref returns" it — so the declared `Arn` is this
        // resource's physical id. The type's last segment is `CertificateAssociation`, so
        // neither `CertificateAssociationName` nor `Name` finds it, and the type publishes no
        // name property at all.
        MEDIACONVERT_CERTIFICATE_ASSOCIATION => Some("Arn"),
        // `RegisterInstance$InstanceId` is "an identifier that you want to associate with the
        // instance", which is what identifies a registered instance in Cloud Map and what
        // "Ref returns" for the type — so the declaration's own `InstanceId` is this
        // resource's name. The type's last segment is `Instance`, so neither `InstanceName`
        // nor `Name` finds it, and the type publishes no name property at all.
        crate::state::template::SERVICEDISCOVERY_INSTANCE => {
            Some(crate::state::template::SERVICEDISCOVERY_INSTANCE_ID_PROPERTY)
        }
        // AWS IoT's `CreateTopicRule` takes a `ruleName` — "Ref returns the rule name" — so
        // the type publishes `RuleName`; the type's last segment is `TopicRule`, so neither
        // `TopicRuleName` nor `Name` finds it. That name is what the rule is CREATED under and
        // the identifier every call on one takes, so it is the physical id.
        crate::state::provisioning::iot_topic_rule::IOT_TOPIC_RULE => {
            Some(crate::state::provisioning::iot_topic_rule::RULE_NAME)
        }
        // Amazon SES's `CreateReceiptRuleSet` takes a `RuleSetName` — "Ref returns the rule
        // set name" — so the type publishes `RuleSetName`; the type's last segment is
        // `ReceiptRuleSet`, so neither `ReceiptRuleSetName` nor `Name` finds it.
        SES_RECEIPT_RULE_SET => Some("RuleSetName"),
        // The same service's `CreateEmailIdentity` takes the identity's own text as its
        // `EmailIdentity` member — "Ref returns the email identity" — so the type publishes
        // `EmailIdentity`; the type's last segment is `EmailIdentity`, so neither
        // `EmailIdentityName` nor `Name` finds it. That text is what the identity is CREATED
        // under and the identifier every SES operation on one takes, so it is the physical
        // id: a generated `<stack>-<logical id>-<unique id>` would name no identity the
        // owning service holds.
        SES_EMAIL_IDENTITY => Some("EmailIdentity"),
        // Neptune Analytics' `CreateGraphSnapshot` takes a `snapshotName`, so the type
        // publishes `SnapshotName`; the type's last segment is `GraphSnapshot`, so neither
        // `GraphSnapshotName` nor `Name` finds it. (The GRAPH's own `GraphName` IS the
        // convention, its last segment being `Graph`.)
        NEPTUNE_GRAPH_GRAPH_SNAPSHOT => Some("SnapshotName"),
        // Amazon Redshift's `CreateCluster` takes a `ClusterIdentifier` — "You use this
        // identifier to refer to the cluster for any subsequent cluster operations" — so
        // the type publishes `ClusterIdentifier`; the type's last segment is `Cluster`, so
        // neither `ClusterName` nor `Name` finds it.
        REDSHIFT_CLUSTER => Some("ClusterIdentifier"),
        // The same service's `CreateClusterParameterGroup` takes a `ParameterGroupName`, and
        // the type publishes it — "Ref returns the parameter group name" — so the name the
        // template states is the group's. The type's last segment is `ClusterParameterGroup`,
        // so neither `ClusterParameterGroupName` nor `Name` finds it.
        crate::state::provisioner::redshift_parameter_groups::REDSHIFT_CLUSTER_PARAMETER_GROUP => {
            Some("ParameterGroupName")
        }
        // The same service's `CreateEventSubscription` takes a `SubscriptionName` — "the name of
        // the event subscription to be created" — and the type publishes it, so the name the
        // template states is the subscription's. The type's last segment is `EventSubscription`,
        // so neither `EventSubscriptionName` nor `Name` finds it.
        crate::state::provisioner::redshift_event_subscriptions::REDSHIFT_EVENT_SUBSCRIPTION => {
            Some("SubscriptionName")
        }
        // Amazon ElastiCache's `CreateCacheCluster` takes a `CacheClusterId`, and the type
        // publishes it as `ClusterName` — "Ref returns the name of the cache cluster" — so
        // neither `CacheClusterName` nor `Name` finds it. That name is what the cluster is
        // CREATED under and the identifier every ElastiCache call on one takes, so it is the
        // physical id.
        crate::state::provisioner::elasticache_caches::ELASTICACHE_CACHE_CLUSTER => {
            Some("ClusterName")
        }
        // The same service's `CreateReplicationGroup` takes a `ReplicationGroupId`, and the
        // type publishes it under that name — "Ref returns the replication group's id" — while
        // the type's last segment is `ReplicationGroup`, so neither `ReplicationGroupName` nor
        // `Name` finds it. That id is what the group is CREATED under and the identifier every
        // ElastiCache call on one takes, so it is the physical id.
        crate::state::provisioner::elasticache_replication_groups::ELASTICACHE_REPLICATION_GROUP => {
            Some(
                crate::state::provisioner::elasticache_replication_groups::REPLICATION_GROUP_ID_PROPERTY,
            )
        }
        // The same service's `CreateCacheSubnetGroup` takes a `CacheSubnetGroupName`, and the
        // type publishes it under that name — "Ref returns the name of the cache subnet group"
        // — while the type's last segment is `SubnetGroup`, so neither `SubnetGroupName` nor
        // `Name` finds it. That name is what the group is CREATED under and the name a cache's
        // own `CacheSubnetGroupName` resolves against, so it is the physical id.
        crate::state::provisioner::elasticache_subnet_groups::ELASTICACHE_SUBNET_GROUP => {
            Some("CacheSubnetGroupName")
        }
        // Amazon Cognito's `CreateGroup` takes `GroupName`, so the type publishes
        // `GroupName` — the type's last segment is `UserPoolGroup`, so neither
        // `UserPoolGroupName` nor `Name` finds it.
        COGNITO_USER_POOL_GROUP => Some("GroupName"),
        // The same service's `CreateIdentityProvider` takes a `ProviderName`, so the type
        // publishes `ProviderName` — the type's last segment is `UserPoolIdentityProvider`,
        // so neither `UserPoolIdentityProviderName` nor `Name` finds it. That name is what
        // the provider is CREATED under and the identifier every cognito-idp call on one
        // takes beside its pool, so it is the physical id.
        COGNITO_USER_POOL_IDENTITY_PROVIDER => Some("ProviderName"),
        // The same service's `CreateUserPoolDomain` takes a `Domain` — the domain string
        // `DescribeUserPoolDomain` resolves a pool's domain by, and what "Ref returns" for
        // the type — so the type publishes `Domain`; the type's last segment is
        // `UserPoolDomain`, so neither `UserPoolDomainName` nor `Name` finds it. That string
        // is what the domain is CREATED under and the identifier every call on one takes
        // beside its pool, so it is the physical id.
        COGNITO_USER_POOL_DOMAIN => Some("Domain"),
        // The same service's `CreateResourceServer` takes an `Identifier` — "a unique
        // resource server identifier" — beside the resource server's `Name`, and that
        // identifier is what every call on one takes and what a custom scope of it is spelled
        // with, so it is the physical id rather than the `Name` the convention would find.
        COGNITO_USER_POOL_RESOURCE_SERVER => Some("Identifier"),
        // Amazon EC2's `CreateSecurityGroup` takes a `GroupName`, so the type publishes
        // `GroupName` — the type's last segment is `SecurityGroup`, so neither
        // `SecurityGroupName` nor `Name` finds it. That name is what the group is CREATED
        // under ([`super::provisioner`]), while the id the create answers with is the
        // identifier the type's `Ref` returns.
        EC2_SECURITY_GROUP => Some("GroupName"),
        // Amazon MSK's `CreateClusterV2` takes a `ClusterName` for a serverless cluster
        // just as `CreateCluster` does for a provisioned one, so the serverless type
        // publishes `ClusterName` — the type's last segment is `ServerlessCluster`, so
        // neither `ServerlessClusterName` nor `Name` finds it. (The provisioned type's own
        // `ClusterName` IS the convention, its last segment being `Cluster`.)
        MSK_SERVERLESS_CLUSTER => Some("ClusterName"),
        // Amazon CloudWatch Logs's `PutMetricFilter` takes a `filterName` — "Ref returns the
        // metric filter name" — so the type publishes `FilterName`; the type's last segment
        // is `MetricFilter`, so neither `MetricFilterName` nor `Name` finds it. Its
        // `PutSubscriptionFilter` takes the same `filterName` — "Ref returns the
        // subscription filter name" — for the same reason.
        crate::state::provisioning::logs_resources::LOGS_METRIC_FILTER
        | crate::state::provisioning::logs_resources::LOGS_SUBSCRIPTION_FILTER => {
            Some("FilterName")
        }
        // Amazon CloudWatch's `PutCompositeAlarm` takes the `AlarmName` every call about an
        // alarm takes — "Ref returns the alarm name" — so the type publishes `AlarmName`; the
        // type's last segment is `CompositeAlarm`, so neither `CompositeAlarmName` nor `Name`
        // finds it. (The METRIC alarm type's own `AlarmName` IS the convention, its last
        // segment being `Alarm`.)
        crate::state::provisioner::cloudwatch_composite_alarms::CLOUDWATCH_COMPOSITE_ALARM => {
            Some(crate::state::provisioner::cloudwatch_composite_alarms::COMPOSITE_ALARM_NAME_MEMBER)
        }
        // Amazon DynamoDB's `CreateTable` takes the `TableName` of the table a global table
        // IS in each of its Regions — "Ref returns the table name" for
        // `AWS::DynamoDB::GlobalTable` as it does for `AWS::DynamoDB::Table` — so the type
        // publishes `TableName`; the type's last segment is `GlobalTable`, so neither
        // `GlobalTableName` nor `Name` finds it. That name is what every regional table of
        // the group is CREATED under and the identifier every call on one takes, so it is
        // the physical id.
        crate::state::template::DYNAMODB_GLOBAL_TABLE => Some("TableName"),
        // Amazon Rekognition's `CreateCollection` takes a `CollectionId` — the case-sensitive
        // id every call on a face collection takes, and what "Ref returns" for the type — so
        // the type publishes `CollectionId`; the type's last segment is `Collection`, so
        // neither `CollectionName` nor `Name` finds it. That id is what the collection is
        // CREATED under, so it is the physical id: a generated `<stack>-<logical id>-<unique
        // id>` would name no collection the owning service holds.
        crate::state::provisioner::rekognition_resources::REKOGNITION_COLLECTION => {
            Some(crate::state::provisioner::rekognition_resources::COLLECTION_ID_PROPERTY)
        }
        _ => None,
    }
}

/// The nested member path a resource type states its NAME at, for a type whose owning API
/// takes that name inside an INPUT STRUCTURE rather than as a member of the create's own
/// request — so neither convention [`physical_name`] reads, nor the per-type property
/// [`declared_name_property`] names, can reach it.
///
/// AWS Glue's `CreateDatabase` takes the database's name as `DatabaseInput$Name` and its
/// `CreateTable` the table's as `TableInput$Name`, and `AWS::Glue::Database` and
/// `AWS::Glue::Table` publish exactly those structures as their properties. The name inside
/// is still the name the resource is CREATED under and the identifier every operation on it
/// takes, so it is the physical id — read here, before the conventions, because a type
/// naming itself this way carries no name property at all.
pub(super) fn declared_name_path(resource_type: &str) -> Option<&'static [&'static str]> {
    match resource_type {
        GLUE_DATABASE => Some(&["DatabaseInput", "Name"]),
        GLUE_TABLE => Some(&["TableInput", "Name"]),
        _ => None,
    }
}

/// The value a declaration's properties carry at a nested member path, or `None` where the
/// declaration carries no member at it ([`declared_name_path`]).
pub(crate) fn property_at<'a>(properties: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(properties, |document, member| document.get(member))
}

/// The LABEL an AgentCore identifier derived from `name` carries: `name` reduced to the
/// characters that identifier's documented pattern admits, cut to
/// [`AGENTCORE_LABEL_CHARS`], and made to start with a letter.
///
/// The three AgentCore identifier patterns admit three different label alphabets
/// (`[a-zA-Z0-9-_]` for a memory, `[a-zA-Z0-9_]` for a runtime, lower-case
/// `[0-9a-z]` with single `-` separators for a gateway), so `admits` is the pattern's
/// alphabet and `lowercase` says whether the pattern admits upper case at all. A
/// CloudFormation physical name (`<stack>-<logical id>-<unique id>`) is not inside any of
/// them, which is why it is reduced here rather than used as it is: an identifier this
/// emulator reports must be of the form the owning service's model publishes.
///
/// `fallback` is the single letter the label starts with when `name` begins with
/// something the pattern cannot (a digit, or a character that was dropped) — every one of
/// the patterns requires a leading letter except the gateway's, and a leading letter
/// satisfies that one too.
pub(super) fn agentcore_label(name: &str, admits: fn(char) -> bool, lowercase: bool, fallback: char) -> String {
    let source = if lowercase {
        name.to_ascii_lowercase()
    } else {
        name.to_string()
    };
    let kept: String = source.chars().filter(|c| admits(*c)).collect();
    // A `-` may not repeat in a gateway label (`([0-9a-z][-]?){1,48}` allows at most one
    // after each character) and may not end it, since the suffix adds its own.
    let mut label = String::with_capacity(kept.len() + 1);
    if !kept.starts_with(|c: char| c.is_ascii_alphabetic()) {
        label.push(fallback);
    }
    for c in kept.chars() {
        if c == '-' && (label.is_empty() || label.ends_with('-')) {
            continue;
        }
        label.push(c);
    }
    label.truncate(AGENTCORE_LABEL_CHARS);
    while label.ends_with('-') {
        label.pop();
    }
    if label.is_empty() {
        label.push(fallback);
    }
    label
}

/// Whether `c` is admitted by a memory identifier's label alphabet
/// (`MemoryId`: `[a-zA-Z][a-zA-Z0-9-_]{0,99}`).
pub(super) fn memory_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Whether `c` is admitted by an agent-runtime identifier's label alphabet
/// (`AgentRuntimeId`: `[a-zA-Z][a-zA-Z0-9_]{0,99}`, which has no `-`).
pub(super) fn runtime_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `c` is admitted by a gateway identifier's label alphabet
/// (`GatewayId`: `([0-9a-z][-]?){1,100}`, which is lower case only).
pub(super) fn gateway_label_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}
