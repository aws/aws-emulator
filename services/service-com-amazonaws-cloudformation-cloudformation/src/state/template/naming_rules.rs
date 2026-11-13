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
        // Amazon SageMaker AI `CreateModel.ModelName`, `CreateNotebookInstance.\
        // NotebookInstanceName` and `CreateNotebookInstanceLifecycleConfig.\
        // NotebookInstanceLifecycleConfigName`: each `@length(min: 0, max: 63)` with a pattern
        // admitting letters, digits and inner hyphens and requiring an alphanumeric at each
        // end (`^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, and the model's
        // `^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$`, in SageMaker's own Smithy model) —
        // which a generated name is already inside, since it starts with a stack name's
        // leading letter and carries only letters, digits and single hyphens, but may be
        // longer than.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE
        | crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            rules(63, Characters::AsGenerated)
        }
        // AWS IoT `CreateAuthorizer.authorizerName`: `AuthorizerName`,
        // `@length(min: 1, max: 128)` with pattern `^[\w=,@-]+$` (AWS IoT's own Smithy
        // model), which a generated name is already inside — letters, digits and the hyphen
        // are all admitted — but may be longer than.
        crate::state::provisioning::iot_authorizer::IOT_AUTHORIZER => rules(128, Characters::AsGenerated),
        // Amazon CloudWatch `PutMetricAlarm.AlarmName`: `AlarmName`,
        // `@length(min: 1, max: 255)` (CloudWatch's own Smithy model).
        "AWS::CloudWatch::Alarm" => rules(255, Characters::AsGenerated),
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
        // Amazon EKS `CreateCluster.name`: "The unique name to give to your cluster", whose
        // own `@length` is 1..100 and whose pattern is `^[0-9A-Za-z][A-Za-z0-9\-_]*$`
        // (`ClusterName` in that service's model) — a generated name is inside that
        // character set and starts with one of its characters, but may be longer than the
        // bound.
        EKS_CLUSTER => rules(100, Characters::AsGenerated),
        // AWS Batch `CreateComputeEnvironment.computeEnvironmentName` and
        // `CreateJobQueue.jobQueueName`: each "can be up to 128 characters long. It can
        // contain uppercase and lowercase letters, numbers, hyphens (-), and underscores
        // (_)" (those members' own docs) — Batch's model types both as plain strings.
        BATCH_COMPUTE_ENVIRONMENT | BATCH_JOB_QUEUE | BATCH_SERVICE_ENVIRONMENT => {
            rules(128, Characters::AsGenerated)
        }
        // Elastic Load Balancing `CreateLoadBalancer.Name` and
        // `CreateTargetGroup.Name`: each "can have a maximum of 32 characters" (Elastic
        // Load Balancing API reference) — the ELBv2 model carries no `@length` for either.
        ELB_LOAD_BALANCER | ELB_TARGET_GROUP => rules(32, Characters::AsGenerated),
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
        // letter") — RDS's model carries no `@length` for any of the three.
        "AWS::RDS::DBParameterGroup"
        | "AWS::RDS::DBClusterParameterGroup"
        | "AWS::RDS::DBSubnetGroup" => rules(255, Characters::LowercaseAlphanumeric),
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
        // Amazon EC2 `CreateSecurityGroup.GroupName`: "Up to 255 characters in length"
        // (Amazon EC2 API reference) — EC2's model carries no `@length` for it.
        EC2_SECURITY_GROUP => rules(255, Characters::AsGenerated),
        // AWS Glue `CreateJob.Name`: `NameString`, `@length(min: 1, max: 255)` with pattern
        // `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$` (AWS Glue's own
        // Smithy model), which a generated name is already inside — every character one is
        // built from is printable — but may be longer than.
        "AWS::Glue::Job" => rules(255, Characters::AsGenerated),
        // The same `NameString` bound AWS Glue holds every entity of its own to:
        // `CreateDatabase.DatabaseInput$Name`, `CreateTable.TableInput$Name` and
        // `CreateCrawler.Name` are all `@length(min: 1, max: 255)` with that same printable
        // pattern (AWS Glue's own Smithy model).
        GLUE_DATABASE | GLUE_TABLE | GLUE_CRAWLER => rules(255, Characters::AsGenerated),
        // Amazon EventBridge `PutRule.Name`: `RuleName`, `@length(min: 1, max: 64)` with
        // pattern `^[\.\-_A-Za-z0-9]+$` (EventBridge's own Smithy model), which a
        // generated name is already inside but may be longer than.
        EVENTS_RULE => rules(64, Characters::AsGenerated),
        // Systems Manager `PutParameter.Name`: `PSParameterName`,
        // `@length(min: 1, max: 2048)` (the SSM model's own bound).
        SSM_PARAMETER => rules(2048, Characters::AsGenerated),
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
        // The same service's `CreateResourceServer.Identifier`:
        // `ResourceServerIdentifierType`, `@length(min: 1, max: 256)` with pattern
        // `^[\x21\x23-\x5B\x5D-\x7E]+$` (that same model), which a generated name is already
        // inside — every character one is built from is printable and is neither a quotation
        // mark nor a backslash — but may be longer than.
        COGNITO_USER_POOL_RESOURCE_SERVER => rules(256, Characters::AsGenerated),
        // Elastic Beanstalk `CreateApplication.ApplicationName` (`ApplicationName`) and
        // `CreateApplicationVersion.VersionLabel` (`VersionLabel`): each
        // `@length(min: 1, max: 100)` in that service's own Smithy model.
        ELASTIC_BEANSTALK_APPLICATION | ELASTIC_BEANSTALK_APPLICATION_VERSION => {
            rules(100, Characters::AsGenerated)
        }
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
        // AWS X-Ray `CreateSamplingRule.SamplingRule.RuleName`: `RuleName`,
        // `@length(min: 1, max: 32)` (X-Ray's own Smithy model), which a generated name is
        // already inside — letters, digits and the hyphen are all admitted — but is far
        // shorter than one may be.
        XRAY_SAMPLING_RULE => rules(32, Characters::AsGenerated),
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
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    let finished = hasher.finalize();
    let digest = match alphabet {
        Alphabet::Lower => ::hex::encode(finished),
        Alphabet::Upper => ::hex::encode_upper(finished),
    };
    digest[..digits.min(digest.len())].to_string()
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
        "AWS::BedrockAgentCore::Runtime" => Some("AgentRuntimeName"),
        // API Gateway's `CreateDomainName` takes the `DomainName` the resource IS, on both
        // of its control planes, and that name is what either type's `Ref` returns — the
        // type's last segment is `DomainName`, so neither `DomainNameName` nor `Name` finds
        // it.
        APIGATEWAY_DOMAIN_NAME | APIGATEWAYV2_DOMAIN_NAME => Some("DomainName"),
        ECR_PUBLIC_REPOSITORY => Some("RepositoryName"),
        // Amazon SES's `CreateReceiptRuleSet` takes a `RuleSetName` — "Ref returns the rule
        // set name" — so the type publishes `RuleSetName`; the type's last segment is
        // `ReceiptRuleSet`, so neither `ReceiptRuleSetName` nor `Name` finds it.
        SES_RECEIPT_RULE_SET => Some("RuleSetName"),
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
        // Amazon ElastiCache's `CreateCacheCluster` takes a `CacheClusterId`, and the type
        // publishes it as `ClusterName` — "Ref returns the name of the cache cluster" — so
        // neither `CacheClusterName` nor `Name` finds it. That name is what the cluster is
        // CREATED under and the identifier every ElastiCache call on one takes, so it is the
        // physical id.
        crate::state::provisioner::elasticache_caches::ELASTICACHE_CACHE_CLUSTER => {
            Some("ClusterName")
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
