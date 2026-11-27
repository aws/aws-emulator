// SPDX-License-Identifier: CC0-1.0

//! Applying a CHANGED declaration to a resource the stack ALREADY has: the two arms of
//! [`Provisioner`] that reach the owning service about a resource it already created —
//! its configuration ([`Provisioner::update`]) and the tags CloudFormation gives it
//! ([`Provisioner::retag`]) — which together are one concern beside the create, the
//! readiness walk and the delete in [`super`].

use super::*;

impl Provisioner {
    /// Apply a CHANGED configuration to a resource that already exists, through the owning
    /// service's own update API, and report what that service decided about it (the same
    /// [`Created`] an initial create answers with: the attributes it reported now).
    ///
    /// `resource` is the record of the resource as it stands — the identity its service
    /// knows it by, and the properties it was last deployed with — and `declared` is the
    /// property document this deployment declares for it. The difference between the two is
    /// what the call carries, so a property neither side changed is not re-sent.
    ///
    /// `Ok(None)` is a type this emulator has no update path for YET: nothing is asked of
    /// the owning service, and [`super::provisioning::reconcile`] reports the change as not
    /// applied rather than as a success that dropped it. `Err` is that service's own
    /// refusal of the change — including a change its API cannot make to an existing
    /// resource, which is the refusal a client would get.
    ///
    /// This is the resource's CONFIGURATION alone. The tags CloudFormation gives it travel
    /// separately, in [`Self::retag`], because they change independently of any declaration —
    /// with one exception, which is why `propagated` is here: an arm that applies a changed
    /// declaration by REPLACING the resource makes a CREATE, and a resource CloudFormation
    /// creates carries the stack's tags from the moment it exists.
    pub(crate) async fn update(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
        propagated: &[(String, String)],
    ) -> Result<Option<Created>, Refusal> {
        match self {
            Self::SqsQueue => update_queue(ctx, region, resource, declared).await.map(Some),
            // The same attribute write the create made, against the queues the NEW
            // declaration names: a queue it no longer names stops carrying the document,
            // which is how an update revokes a grant.
            Self::SqsQueuePolicy => update_queue_policy(ctx, region, resource, declared)
                .await
                .map(Some),
            // CloudWatch's own `PutMetricStream` is documented "Creates or updates a metric
            // stream", so the call that created the stream is the call that applies a
            // changed declaration to it — under the name the stack gave it, with the
            // configuration the template now declares.
            Self::CloudWatchMetricStream => {
                put_metric_stream_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    declared,
                )
                .await
                .map(Some)
            }
            // `PutDashboard` is documented "Creates a dashboard if it does not already
            // exist, or updates an existing dashboard. If you update a dashboard, the
            // entire contents are replaced with what you specify here", so the call that
            // created the dashboard is the call that applies a changed declaration to it.
            Self::CloudWatchDashboard => {
                put_dashboard_body(ctx, region, &resource.physical_resource_id, declared)
                    .await
                    .map(Some)
            }
            // `PutRule` is documented "Creates or updates the specified rule", so the call
            // that created the rule is the call that applies a changed declaration to it,
            // with the declared target list reconciled against the one it carries.
            Self::EventsRule => update_rule(ctx, region, resource, declared).await,
            // `UpdateEventBus` is the call that applies a changed declaration to the bus the
            // stack already has, under the name it was created under.
            Self::EventsEventBus => {
                event_buses::update_event_bus(ctx, region, resource, declared).await
            }
            // `PutPermission` is the call that installs a grant, and a grant installed again
            // under a statement id already on the bus replaces that one statement.
            Self::EventsEventBusPolicy => {
                event_buses::update_event_bus_policy(ctx, region, declared).await
            }
            // `PutParameter` is documented "Create or update a parameter in Parameter
            // Store", and `Overwrite` is what makes the second of the two: the changed
            // declaration reaches Systems Manager as a new version of the same parameter.
            Self::SsmParameter => update_parameter(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateAssociation` takes the id Systems Manager minted together with the whole
            // declaration — the document the association applies included — so a changed
            // declaration reaches that service as a new version of the same association
            // rather than replacing it.
            Self::SsmAssociation => update_ssm_association(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateDocument` "updates one or more values for an SSM document" — it adds the
            // declared content to the document that NAME belongs to as a new version — so a
            // changed declaration reaches that service as a version of the same document, and
            // the version it created becomes the default one every later run resolves.
            Self::SsmDocument => update_ssm_document(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateGroup` "updates any of the properties for precedence, IAM role, or
            // description" of a group the pool already holds, so a changed declaration of
            // those reaches Amazon Cognito through it — while the two members that make the
            // group's identity are not members of that call at all.
            Self::CognitoUserPoolGroup => update_group(ctx, region, resource, declared).await,
            // A role's identity is its NAME, and every other property of
            // `AWS::IAM::Role` has an IAM call that changes it on the role that name
            // belongs to — the trust policy through `UpdateAssumeRolePolicy` — so a
            // changed declaration is applied to the role the stack created rather than
            // replacing it ([`update_role`]).
            Self::IamRole => update_role(ctx, region, resource, declared).await,
            // A user's identity is its NAME, and every other property of `AWS::IAM::User`
            // has an IAM call that changes it on the user that name belongs to — its path
            // through `UpdateUser`, which keeps the user's unique identifier — so a
            // changed declaration is applied to the user the stack created rather than
            // replacing it ([`update_iam_user`]).
            Self::IamUser => update_iam_user(ctx, region, resource, declared).await,
            // An access key's `Status` is the one property of a key that exists IAM can
            // change — `UpdateAccessKey` "changes the status of the specified access key
            // from Active to Inactive, or vice versa" — so a changed declaration of it is
            // applied to the key the stack created ([`update_iam_access_key`]), while the
            // user it belongs to and the `Serial` it is rotated with replace the resource
            // rather than reaching this call at all.
            Self::IamAccessKey => update_iam_access_key(ctx, region, resource, declared).await,
            // A managed policy's identity is its ARN, and the document it grants from is
            // changed by adding a new DEFAULT version to the policy that ARN names, so a
            // changed declaration is applied to the policy the stack created rather than
            // replacing it ([`update_managed_policy`]).
            Self::IamManagedPolicy => {
                update_managed_policy(ctx, region, resource, declared).await
            }
            // Every property of a KMS key that can change on an existing one has a KMS
            // call that changes it — its description, its key policy, and the two switches
            // — so a changed declaration is applied to the key the stack created rather
            // than replacing it ([`update_kms_key`]).
            Self::KmsKey => update_kms_key(ctx, region, resource, declared).await.map(Some),
            // `UpdateAlias` "associates an existing KMS alias with a different KMS key", so
            // a declaration that re-points an alias is applied to the alias the stack
            // created rather than replacing it ([`update_kms_alias`]).
            Self::KmsAlias => update_kms_alias(ctx, region, resource, declared).await,
            // Every attribute of a live subscription has one SNS call that writes it, so a
            // changed declaration of one is applied to the subscription the stack created
            // rather than dropped ([`update_subscription`]).
            Self::SnsSubscription => update_subscription(ctx, region, resource, declared).await,
            // A topic's properties ARE its attributes, and SNS has one call that writes an
            // attribute of a live topic, so a changed declaration is applied to the topic
            // the stack created — under the ARN its `Ref` resolves to — rather than
            // replacing it ([`update_topic`]).
            Self::SnsTopic => update_topic(ctx, region, resource, declared).await.map(Some),
            // The same attribute write the create made, against the topics the NEW
            // declaration names: a topic it no longer names stops carrying the document,
            // which is how an update revokes a grant.
            Self::SnsTopicPolicy => update_topic_policy(ctx, region, resource, declared)
                .await
                .map(Some),
            // A parameter group's `Parameters` map is the one property of either group type
            // Amazon RDS can change on a group that exists, and the modify is the call that
            // changes it — the same call the create makes for the values a template declares
            // — so a changed declaration is applied to the group the stack created rather
            // than dropped ([`update_parameter_group`]).
            Self::RdsDbParameterGroup => {
                update_parameter_group(ctx, region, resource, declared, DB_PARAMETER_GROUP_CALLS)
                    .await
            }
            // Every setting of a DB cluster that Amazon RDS can change on a cluster that
            // exists is a member of `ModifyDBCluster`, so a changed declaration is applied
            // to the cluster the stack created rather than dropped
            // ([`rds_db_clusters::update_db_cluster`]).
            Self::RdsDbCluster => {
                Box::pin(update_db_cluster(ctx, region, resource, declared)).await
            }
            // The same for a DB instance, through `ModifyDBInstance` — except for a
            // create-only setting, which no modification can change and which that arm
            // therefore deploys by REPLACING the instance, so it is handed the tags a
            // replacement is created with ([`update_db_instance`]).
            Self::RdsDbInstance => {
                Box::pin(update_db_instance(ctx, region, resource, declared, propagated)).await
            }
            Self::RdsDbClusterParameterGroup => {
                update_parameter_group(
                    ctx,
                    region,
                    resource,
                    declared,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // Every setting of a replication group that Amazon ElastiCache can change on a
            // group that exists is a member of `ModifyReplicationGroup`, so a changed
            // declaration is applied to the group the stack created rather than dropped
            // ([`elasticache_replication_groups::update_replication_group`]).
            Self::ElastiCacheReplicationGroup => {
                Box::pin(update_replication_group(ctx, region, resource, declared)).await
            }
            // A cache subnet group's description and its subnet list are everything it carries
            // beside the name it is addressed by, and `ModifyCacheSubnetGroup` takes both, so
            // a changed declaration reaches the group the stack created
            // ([`elasticache_subnet_groups::update_cache_subnet_group`]).
            Self::ElastiCacheSubnetGroup => {
                Box::pin(update_cache_subnet_group(ctx, region, resource, declared)).await
            }
            // The Amazon Neptune parameter groups are in exactly that position: their
            // `Parameters` map is the one part of either declaration the owning front door can
            // change on a group that exists, through the same modify their create makes for the
            // declared values.
            Self::NeptuneDbParameterGroup => {
                update_parameter_group(
                    ctx,
                    region,
                    resource,
                    declared,
                    neptune_relational::NEPTUNE_DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::NeptuneDbClusterParameterGroup => {
                update_parameter_group(
                    ctx,
                    region,
                    resource,
                    declared,
                    neptune_relational::NEPTUNE_DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // A Redshift parameter group's `Parameters` are the one part of its declaration
            // Amazon Redshift can change on a group that exists, and the modify is the call
            // that changes it — the same call the create makes for the declared values — so a
            // changed declaration is applied to the group the stack created rather than
            // dropped ([`update_cluster_parameter_group`]).
            Self::RedshiftClusterParameterGroup => {
                Box::pin(update_cluster_parameter_group(ctx, region, resource, declared)).await
            }
            // A snapshot copy grant has nothing Amazon Redshift can change on it — that
            // service publishes no modify for one — so the arm decides whether the change is
            // one the grant the stack has can take at all.
            Self::RedshiftSnapshotCopyGrant => update_snapshot_copy_grant(resource, declared),
            // Everything an event subscription carries beside its name and its tags is a
            // member of `ModifyEventSubscription`, so a changed declaration is applied to the
            // subscription the stack created rather than replacing it
            // ([`update_event_subscription`]).
            Self::RedshiftEventSubscription => {
                Box::pin(update_event_subscription(ctx, region, resource, declared)).await
            }
            // Every member of a health check's configuration that can change on a live one
            // is a member of `UpdateHealthCheck`, so a changed declaration is applied to the
            // health check the stack created — under the id its `Ref` resolves to — rather
            // than replacing it ([`update_health_check`]).
            Self::Route53HealthCheck => update_health_check(ctx, region, resource, declared).await,
            // `UpdateJob` takes the `JobUpdate` that "completely overwrites" the previous
            // job definition, and every property of `AWS::Glue::Job` but its name is a
            // member of that shape — so a changed declaration is applied to the job the
            // stack created rather than replacing it ([`update_glue_job`]).
            Self::GlueJob => update_glue_job(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateTrigger` takes the `TriggerUpdate` that "updates the previous trigger
            // definition by overwriting it completely", so a changed declaration is applied
            // to the trigger the stack created rather than replacing it — less the three
            // properties that shape carries no member for, which a change replaces the
            // trigger over ([`glue_triggers::GLUE_TRIGGER_CREATE_ONLY_PROPERTIES`]).
            Self::GlueTrigger => update_glue_trigger(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateDatabase` "updates an existing database definition" and takes the same
            // `DatabaseInput` the create did, so a changed declaration is applied to the
            // database the stack created rather than replacing it; `UpdateTable` is the same
            // call for a table's `TableInput`, and every property of `AWS::Glue::Crawler`
            // but its tags is a member of `UpdateCrawler`.
            Self::GlueDatabase => update_glue_database(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::GlueTable => update_glue_table(ctx, region, resource, declared)
                .await
                .map(Some),
            // Lake Formation publishes no call that SETS the permissions a grant carries, so
            // a changed declaration reaches it as the revoke of what the grant no longer
            // carries and the grant of what it adds — the two calls a client makes
            // ([`update_lake_formation_permissions`]).
            Self::LakeFormationPermissions(grant) => {
                update_lake_formation_permissions(ctx, region, grant, resource, declared).await
            }
            Self::GlueCrawler => update_glue_crawler(ctx, region, resource, declared)
                .await
                .map(Some),
            // EventBridge Scheduler's `UpdateSchedule` "uses all values, including empty
            // values, specified in the request and overrides the existing schedule", which
            // is exactly what a declaration states about a schedule — so the document that
            // created it is the document that applies a changed declaration to it
            // ([`update_schedule`]).
            Self::SchedulerSchedule => update_schedule(ctx, region, resource, declared).await,
            // `UpdatePipe` takes a pipe's name together with the configuration it is to
            // carry, so a changed declaration is applied to the pipe the stack created
            // rather than replacing it — less the `Source` that call takes no member for,
            // which a pipe cannot be repointed at ([`event_pipes::update_pipe`]).
            Self::PipesPipe => event_pipes::update_pipe(ctx, region, resource, declared).await,
            // A schedule group publishes its `Name`, which IS the group's identity, and its
            // `Tags`, which are no part of any call that changes a group that exists and
            // reach EventBridge Scheduler through its own tagging calls ([`Self::retag`]) —
            // so a declaration whose change is its tags has nothing else to ask of that
            // service ([`update_schedule_group`]).
            Self::SchedulerScheduleGroup => Ok(update_schedule_group(resource, declared)),
            // `UpdateGroup` changes a resource group's description and `UpdateGroupQuery` its
            // resource query, which are the two halves of a group a declaration can state
            // differently; its name is its identity and its tags reach that service through
            // its own tagging calls ([`Self::retag`]).
            Self::ResourceGroupsGroup => {
                update_resource_group(ctx, region, resource, declared).await
            }
            // `UpdateWorkloadIdentity` takes an identity's name together with the whole
            // list of allowed OAuth2 return URLs, which "replaces the existing list" — so
            // a changed declaration of that list is applied to the identity the stack
            // created rather than replacing it, and a declaration that states none leaves
            // it with none ([`update_workload_identity`]).
            // `UpdateGuardrail` "updates a guardrail with the values you specify", so a
            // changed declaration is applied to the guardrail the stack created rather
            // than replacing it; the guardrail's identity is the ARN Amazon Bedrock
            // minted, which an update leaves alone.
            Self::BedrockGuardrail => update_bedrock_guardrail(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateKnowledgeBase` takes a knowledge base's id together with the whole
            // configuration it is to carry, so a changed declaration is applied to the
            // knowledge base the stack created rather than replacing it; its identity is the
            // id Amazon Bedrock Agent minted, which an update leaves alone. A changed tag
            // map is no part of that call, which that arm reports as not applied.
            Self::BedrockKnowledgeBase => {
                update_bedrock_knowledge_base(ctx, region, resource, declared).await
            }
            // `UpdateDataSource` takes the knowledge base and the data source's own id
            // together with the whole declared configuration, so it is what applies a changed
            // declaration to the data source the stack created.
            Self::BedrockDataSource => {
                update_bedrock_data_source(ctx, region, resource, declared).await
            }
            // `UpdateFlow` takes the flow's whole declared configuration by the identity
            // Amazon Bedrock Agent minted for it, the flow's name and its definition
            // included, so a changed declaration is applied to that flow rather than
            // replacing it. Its tags travel separately ([`Self::retag`]).
            Self::BedrockFlow => update_bedrock_flow(ctx, region, resource, declared).await,
            // `UpdateSession` takes a session's identifier together with the metadata it is to
            // carry, so a changed declaration is applied to the session the stack created
            // rather than replacing it; its identity is the ARN the Bedrock Agent runtime
            // minted, which an update leaves alone.
            Self::BedrockSession => {
                update_bedrock_session(ctx, region, resource, declared).await
            }
            // Each Amazon Bedrock Data Automation update takes the resource's ARN together with
            // the configuration it is to carry — a blueprint's schema and stage, a project's
            // stage, description and output configurations, a library's description — so a
            // changed declaration is applied to the resource the stack created rather than
            // replacing it; its identity is the ARN that service minted, which an update leaves
            // alone. A changed tag list is no part of those calls, which that arm reports as not
            // applied.
            Self::BedrockDataAutomation(kind) => {
                update_bedrock_data_automation(ctx, region, kind, resource, declared).await
            }
            Self::BedrockAgentCoreWorkloadIdentity => {
                update_workload_identity(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateAgentRuntime`, `UpdateGateway` and `UpdateGatewayTarget` each take the
            // whole declared configuration of the resource by the identifier that control
            // plane minted for it, so a changed declaration is applied to that resource
            // rather than replacing it. Neither sandbox type has an arm: that service
            // publishes no update of a code interpreter or a browser, and the memory family's
            // own update takes a MODIFY document rather than the declaration
            // ([`Self::BedrockAgentCoreMemory`]) — a change either type cannot carry is
            // reported as not applied.
            Self::BedrockAgentCoreRuntime => {
                update_agentcore_runtime(ctx, region, resource, declared).await
            }
            Self::BedrockAgentCoreGateway => {
                update_agentcore_gateway(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            Self::BedrockAgentCoreGatewayTarget => {
                update_agentcore_gateway_target(ctx, region, resource, declared).await
            }
            // Each of the three AWS Config puts is documented to create OR update the
            // resource it names, so the call that created the recorder, the channel or the
            // rule is the call that applies a changed declaration to it — under the name
            // the stack gave it, with the configuration the template now declares.
            Self::ConfigConfigurationRecorder => put_recorder_configuration(
                ctx,
                region,
                &resource.physical_resource_id,
                declared,
            )
            .await
            .map(Some),
            Self::ConfigDeliveryChannel => put_channel_configuration(
                ctx,
                region,
                &resource.physical_resource_id,
                declared,
            )
            .await
            .map(Some),
            Self::ConfigConfigRule => {
                put_config_rule_configuration(ctx, region, &resource.physical_resource_id, declared)
                    .await
                    .map(Some)
            }
            // `UpdateDomainConfig` "modifies the cluster configuration of the specified
            // Amazon OpenSearch Service domain", and every property of
            // `AWS::OpenSearchService::Domain` that is a member of it can change on a domain
            // that exists — so a changed declaration is applied to the domain the stack
            // created, which keeps its name, its id, its ARN and the address it is reached
            // at, rather than replacing it ([`update_domain`]).
            Self::OpenSearchServiceDomain => update_domain(ctx, region, resource, declared).await,
            // `PutBucketPolicy` "applies an Amazon S3 bucket policy to an Amazon S3
            // bucket", replacing the one the bucket carries, so the call that attached the
            // policy is the call that applies a changed declaration to it.
            Self::S3BucketPolicy => set_bucket_policy(ctx, region, declared).await.map(Some),
            // `PutTableBucketMaintenanceConfiguration` "creates a new maintenance
            // configuration or replaces an existing maintenance configuration for a table
            // bucket", so a changed `UnreferencedFileRemoval` is applied to the table bucket
            // the stack created — which keeps the ARN that is its physical id and everything
            // it holds — rather than replacing it ([`update_table_bucket`]).
            Self::S3TablesTableBucket => {
                update_table_bucket(ctx, region, resource, declared).await
            }
            // Amazon EC2 Auto Scaling's own `UpdateAutoScalingGroup` is the call that
            // "updates the configuration for the specified Auto Scaling group", so a changed
            // declaration reaches the group through it — under the name the group was created
            // with, which is this resource's physical id.
            Self::AutoScalingGroup => {
                update_auto_scaling_group(ctx, region, resource, declared).await
            }
            // `PutScalingPolicy` "creates or updates a scaling policy for an Auto Scaling
            // group", so a changed declaration reaches the policy through the same call that
            // created it, under the name it was created with.
            Self::AutoScalingScalingPolicy => {
                update_scaling_policy(ctx, region, resource, declared).await
            }
            // Each of the three children of a group is stated by one upsert of its own —
            // `PutLifecycleHook`, `PutScheduledUpdateGroupAction` and `PutWarmPool` each
            // "creates or updates" the resource — so a changed declaration reaches the child
            // through the same call that created it, on the group it is already on.
            Self::AutoScalingLifecycleHook => {
                update_lifecycle_hook(ctx, region, resource, declared).await
            }
            Self::AutoScalingScheduledAction => {
                update_scheduled_action(ctx, region, resource, declared).await
            }
            Self::AutoScalingWarmPool => {
                update_warm_pool(ctx, region, resource, declared).await
            }
            // A LAUNCH TEMPLATE is a versioned document: `CreateLaunchTemplateVersion`
            // "creates a new version of a launch template", so a changed declaration of its
            // launch data reaches Amazon EC2 as a new version of the template the stack
            // created — under the `lt-` id that template keeps
            // ([`update_ec2_launch_template`]).
            Self::Ec2LaunchTemplate => {
                update_ec2_launch_template(ctx, region, resource, declared).await
            }
            // Each of these Amazon EC2 types takes a changed property through the call that
            // service publishes for it, so the resource keeps the identifier EC2 minted and
            // the stack reports the change as applied ([`ec2_in_place`]). A property no such
            // call carries is create-only and replaces the resource instead
            // ([`super::template::replacement_properties`]).
            Self::Ec2Vpc => update_ec2_vpc(ctx, region, resource, declared).await,
            Self::Ec2Subnet => update_ec2_subnet(ctx, region, resource, declared).await,
            Self::Ec2SecurityGroup => {
                update_ec2_security_group(ctx, region, resource, declared).await
            }
            // An Amazon EBS VOLUME's own `ModifyVolume` arm is not written here yet, so a
            // changed declaration of one is reported as not applied rather than as a success
            // that dropped the caller's change; its tags are reconciled all the same
            // ([`Self::retag`]), which is the one property a deployment can change without
            // asking anything of that call.
            Self::Ec2Volume if changed_properties(&resource.properties, declared)
                .iter()
                .all(|property| property == "Tags") =>
            {
                Ok(applied_in_place())
            }
            // `UpdateSpendingLimit` "updates an existing spending limit" by the ARN Amazon
            // Braket minted, taking the limit's amount and its period — every property of
            // the type except the device it caps, which replaces the resource instead — so a
            // changed declaration of either reaches the limit the stack created.
            // Amazon ECS's own `UpdateService` "modifies the parameters of a service", so a
            // changed declaration of a member it takes is applied to the service the stack
            // created rather than replacing it.
            Self::EcsService => Box::pin(update_ecs_service(ctx, region, resource, declared))
                .await
                .map(Some),
            // `PutClusterCapacityProviders` sets the WHOLE capacity-provider configuration of
            // the cluster it addresses, so a declaration stating different providers or a
            // different default strategy is that same call with the lists the template now
            // states — the type's update and its create alike.
            Self::EcsClusterCapacityProviderAssociations => {
                put_cluster_capacity_providers(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateTaskSet` changes the one property of a task set that is not fixed when it
            // is made, which is the scale its tasks are kept at
            // ([`ecs_task_sets::update_ecs_task_set`]).
            Self::EcsTaskSet => {
                Box::pin(update_ecs_task_set(ctx, region, resource, declared)).await
            }
            // There is one primary task set per service, so a declaration naming a different
            // task set is `UpdateServicePrimaryTaskSet` again with the one it now names.
            Self::EcsPrimaryTaskSet => {
                update_service_primary_task_set(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // CloudWatch's own `PutMetricAlarm` is documented "Creates or updates an alarm",
            // so a changed declaration reaches that service as the configuration the
            // template now declares, under the name the alarm was created with.
            Self::CloudWatchAlarm => update_metric_alarm(ctx, region, resource, declared)
                .await
                .map(Some),
            // `PutCompositeAlarm` is documented "Creates or updates a composite alarm" the
            // same way, so a changed rule reaches that service under the name the alarm was
            // created with.
            Self::CloudWatchCompositeAlarm => {
                update_composite_alarm(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // The objective's own update, which takes every member of the create but the
            // name; and the grouping configuration's PUT, documented "Creates or updates the
            // grouping configuration for this account", so the call that created it is the
            // call that applies a changed declaration to it. An INSTRUMENTATION CONFIG has no
            // arm here on purpose: "configurations are immutable after creation", so every
            // property of that type replaces the resource rather than changing it.
            Self::ApplicationSignalsServiceLevelObjective => {
                update_slo(ctx, region, resource, declared).await
            }
            Self::ApplicationSignalsGroupingConfiguration => {
                put_grouping_configuration(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            Self::BraketSpendingLimit => update_braket_spending_limit(ctx, region, resource, declared)
                .await
                .map(Some),
            // `PutResourcePolicy` attaches the document to the ARN it names whether or not
            // one is attached already, so the call that created an `AWS::SMSVOICE::
            // ResourcePolicy` is the call that applies a changed declaration to it. Every
            // other `AWS::SMSVOICE::*` type answers `Ok(None)`: that service changes a
            // configuration set's defaults, a protect configuration's deletion protection and
            // an RCS agent's channel through calls this emulator serves no provisioning path
            // for yet, so a changed declaration of one is reported as NOT applied rather than
            // as a success that dropped it.
            Self::SmsVoiceResource(sms_voice::Resource::Policy) => {
                update_sms_voice_resource_policy(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            Self::SmsVoiceResource(_) => Ok(None),
            // `UpdateInvestigationGroup` "updates the configuration of the specified
            // investigation group" by the ARN Amazon CloudWatch investigations minted, taking
            // the role it gathers data as, its encryption configuration, its tag key
            // boundaries, its chat channels, its CloudTrail flag and its cross-account
            // configurations — every property of the type but the two its create alone fixes,
            // which replace the resource instead — so a changed declaration of one reaches the
            // group the stack created. The group's resource POLICY is a call of its own, which
            // the same arm makes ([`update_investigation_group`]).
            Self::AiOpsInvestigationGroup => {
                update_investigation_group(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateBuild` and `UpdateScript` each change the resource the stack created by
            // the id Amazon GameLift Servers minted for it: a build's name and version, and a
            // script's name, version and content source, are what those calls take — so a
            // declaration restating one of them is applied to the build or the script rather
            // than replacing it. Every other property of either type is fixed when the
            // resource is made ([`super::template::replacement_properties`]).
            Self::GameLiftBuild => Box::pin(update_gamelift_build(ctx, region, resource, declared))
                .await
                .map(Some),
            Self::GameLiftScript => {
                Box::pin(update_gamelift_script(ctx, region, resource, declared))
                    .await
                    .map(Some)
            }
            // Either `Update…` of AWS Direct Connect renames the resource the stack created,
            // by the identifier that service minted for it — the one thing either call
            // changes — so a declaration stating another name is applied to the gateway or
            // the group rather than replacing it. Every other property of either type is
            // fixed when the resource is made ([`super::template::replacement_properties`]).
            Self::DirectConnectGateway => {
                update_direct_connect_gateway(ctx, region, resource, declared).await
            }
            Self::DirectConnectResiliencyGroup => {
                update_resiliency_group(ctx, region, resource, declared).await
            }
            // Each `Update…` takes the resource's whole declared configuration by the ARN
            // that service minted for it, the name included — so a changed declaration is
            // applied to that dashboard or report rather than replacing it. Their tags travel
            // separately ([`Self::retag`]).
            Self::BcmDashboard => update_bcm_dashboard(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::BcmScheduledReport => {
                update_bcm_scheduled_report(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateEnvironment` reconfigures the environment the stack created, by the name
            // Amazon MWAA knows it by, taking every member of the create but the ones fixed
            // when an environment is made — which replace the resource instead — so a changed
            // declaration is applied to that environment rather than replacing it.
            Self::MwaaEnvironment => update_mwaa_environment(ctx, region, resource, declared).await,
            // The single sign-on flag is the one property of either Directory Service
            // directory type a call of that service changes in place (`EnableSso` /
            // `DisableSso`); every other property is create-only, so a changed declaration of
            // one replaces the directory instead of reaching here.
            Self::DirectoryServiceDirectory(kind) => {
                update_ds_directory(ctx, region, kind, resource, declared).await
            }

            // `UpdateApi` ("updates an Api resource") is the one call that changes an api API
            // Gateway v2 already holds, and every property of
            // `AWS::ApiGatewayV2::Api` but its protocol, its definition and its tags is a
            // member of it — so a changed declaration of one is applied to the api the stack
            // created, which keeps the `ApiId` that service minted and the endpoint it serves
            // the api on, rather than replacing it. A declaration carrying a DEFINITION is
            // applied by `ReimportApi` instead, which puts that definition over the same api
            // ([`update_apigatewayv2_api`]).
            // BOXED, for the reason the other heavy arms of this match are: the call's future
            // is the owning service's whole handler, and holding one of those inline in this
            // match's state machine is what takes a deployment past the thread's stack.
            Self::ApiGatewayV2Api => {
                Box::pin(update_apigatewayv2_api(ctx, region, resource, declared)).await
            }
            // Each of the resources that live INSIDE an api has its own patch, addressed by
            // the api that holds it and the identifier that service minted for it, and taking
            // the same members its create does — so a changed declaration of an integration,
            // a route, a stage or an authorizer is applied to the resource the stack created
            // rather than refused ([`update_apigatewayv2_child`]). Boxed for the reason the
            // api's own arm above is.
            Self::ApiGatewayV2Integration => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_INTEGRATION_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2Route => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_ROUTE_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2Stage => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_STAGE_CHILD,
                ))
                .await
            }
            // An `AWS::ApiGatewayV2::Deployment` has NO arm here: the one member
            // `UpdateDeployment` takes is the deployment's description, and this emulator does
            // not serve that operation yet (its own scaffold), so there is nothing to ask API
            // Gateway v2 for — a changed declaration is reported as not applied rather than
            // sent to a call that answers 501. A declaration that names another api or another
            // stage is create-only either way
            // ([`super::super::template::replacement_properties`]), so such a change replaces
            // the deployment.
            Self::ApiGatewayV2Authorizer => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_AUTHORIZER_CHILD,
                ))
                .await
            }
            // The three that say what a route replies have their own patches too
            // (`UpdateIntegrationResponse`, `UpdateRouteResponse`, `UpdateModel`), each
            // addressed by the api, the parent that holds the resource and the identifier
            // that service minted for it, and each taking the same members its create does.
            // Boxed for the reason the arms above are.
            Self::ApiGatewayV2IntegrationResponse => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_INTEGRATION_RESPONSE_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2RouteResponse => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_ROUTE_RESPONSE_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2Model => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_MODEL_CHILD,
                ))
                .await
            }
            // `PutRoutingRule` ("puts a routing rule") is the one call that changes a routing
            // rule the custom domain name already holds, and it takes every property of
            // `AWS::ApiGatewayV2::RoutingRule` but the domain name that addresses it — so a
            // changed priority, condition set or action list is applied to the rule the stack
            // created, which keeps the id and the ARN that service minted, rather than
            // replacing it. Boxed for the reason the arms above are.
            Self::ApiGatewayV2RoutingRule => {
                Box::pin(update_apigatewayv2_routing_rule(
                    ctx, region, resource, declared,
                ))
                .await
                .map(Some)
            }
            // Each `Update…` takes the entity's whole declared configuration by the id Amplify
            // UI Builder minted for it, in the app environment that is fixed when the entity
            // is made — a declaration restating either replaces the resource instead — so a
            // changed declaration is applied to that entity rather than replacing it.
            Self::AmplifyUiBuilderComponent => {
                update_ui_entity(ctx, region, UiEntity::Component, resource, declared).await
            }
            Self::AmplifyUiBuilderForm => {
                update_ui_entity(ctx, region, UiEntity::Form, resource, declared).await
            }
            Self::AmplifyUiBuilderTheme => {
                update_ui_entity(ctx, region, UiEntity::Theme, resource, declared).await
            }
            // `UpdateApp` takes the app's whole declared configuration by the id AWS Amplify
            // minted for it, the app's name included, so a changed declaration is applied to
            // that app rather than replacing it.
            Self::AmplifyApp => update_amplify_app(ctx, region, resource, declared).await,
            // `UpdateBranch` takes the branch's whole declared configuration by the app and the
            // name it was created under, so a changed declaration is applied to that branch
            // rather than replacing it. (A re-declared app or name replaces the resource
            // instead: both are [`super::template::replacing_properties`] of this type.)
            Self::AmplifyBranch => update_amplify_branch(ctx, region, resource, declared).await,
            // What a MemoryDB group's own service can change reaches it: a parameter group's
            // declared values, and a subnet group's description and subnets
            // ([`memorydb_groups::update_memorydb_group`]).
            Self::MemoryDbGroup(group) => {
                update_memorydb_group(ctx, region, group, resource, declared).await
            }
            // `UpdateDomain` reconfigures the domain the stack created, by the name Amazon
            // Connect Customer Profiles knows it by, taking every property of the type but
            // that name — so a changed declaration is applied to that domain rather than
            // replacing it. Its tags travel separately ([`Self::retag`]).
            Self::CustomerProfilesDomain => {
                update_customer_profiles_domain(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateConnectorProfile` re-configures the connector profile the stack created, by
            // the name Amazon AppFlow holds it under, taking the connection mode and the whole
            // `connectorProfileConfig` — so a changed declaration of one of those is applied to
            // that profile rather than replacing it. (The type publishes no `Tags` property, so
            // it has no arm in [`Self::retag`].)
            // BOXED, for the reason the other heavy arms of this match are: the call's future is
            // the owning service's whole handler — which here makes a nested call of its own, to
            // the service that holds the profile's credentials — and holding one of those inline
            // in this match's state machine is what takes a deployment past the thread's stack.
            Self::AppFlowConnectorProfile => {
                Box::pin(update_appflow_connector_profile(ctx, region, resource, declared))
                    .await
                    .map(Some)
            }
            // `UpdateRule` reconfigures the retention rule the stack created, by the
            // identifier Recycle Bin minted for it, taking the description, the retention
            // period and either resource selection — so a changed declaration of one of those
            // is applied to that rule rather than replacing it. Its tags travel separately
            // ([`Self::retag`]).
            Self::RbinRule => update_rbin_rule(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateLifecyclePolicy` reconfigures the lifecycle policy the stack created, by the
            // id Amazon Data Lifecycle Manager minted for it, taking its description, its
            // activation state, the role it runs under and its whole configuration — so a changed
            // declaration of one of those is applied to that policy rather than replacing it. Its
            // tags travel separately ([`Self::retag`]).
            Self::DlmLifecyclePolicy => update_dlm_lifecycle_policy(ctx, region, resource, declared)
                .await
                .map(Some),
            // Each entity's own `Update…` reconfigures the one the stack created, by the ARN
            // AWS Network Security Manager minted for it, taking the description and the
            // configuration or associations it enforces — so a changed declaration of one of
            // those is applied to that entity rather than replacing it. (Its tags have no arm
            // in [`Self::retag`]: that service changes them through its own
            // `TagResource`/`UntagResource`, which no arm calls yet, so a changed tag set
            // replaces the entity — see [`template::replacement_properties`].)
            Self::NetworkSecurityManagerEntity(entity) => {
                update_entity(ctx, region, entity, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateHypervisor` reconfigures the hypervisor the stack imported, by the ARN
            // Backup Gateway minted for it, taking the host, the credentials, the name and the
            // log group — so a changed declaration of one of those is applied to that
            // hypervisor rather than replacing it. Its tags travel separately
            // ([`Self::retag`]).
            Self::BackupGatewayHypervisor => {
                update_backup_gateway_hypervisor(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateAnalyzer` reconfigures the analyzer the stack created, by the name it was
            // created under, taking the configuration alone — and the analyzer's own inline
            // archive rules are reconciled beside it, each through the call that adds, restates
            // or removes one. `UpdateArchiveRule` restates the criteria of a rule declared as a
            // resource of its own. So a changed declaration of either reaches that service
            // rather than replacing the resource; the analyzer's tags travel separately
            // ([`Self::retag`]).
            Self::AccessAnalyzerResource(kind) => {
                update_access_analyzer_resource(ctx, region, kind, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateGraph` reconfigures the graph the stack created, by the id Neptune
            // Analytics minted for it, taking the memory it is provisioned with and its two
            // flags — so a changed declaration of one of those is applied to that graph rather
            // than replacing it. Its tags travel separately ([`Self::retag`]). (A SNAPSHOT has
            // no arm: that service publishes no call that changes a snapshot's configuration,
            // and the one property of that type a deployment can change without replacing it
            // is its tags, which travel there too.)
            Self::NeptuneGraphGraph => update_neptune_graph(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateNamespace` reconfigures the namespace the stack created, by the name it was
            // created under, taking the admin credentials, the encryption keys, the IAM roles
            // and the log exports; `UpdateWorkgroup` reconfigures the workgroup the same way,
            // taking its compute, its ceiling, its port, its two flags, its subnets, its
            // security groups, its database settings and its price-performance target. So a
            // changed declaration of one of those reaches the resource rather than replacing
            // it. (A SNAPSHOT has no arm: that service's own `UpdateSnapshot` is not served
            // here, so a changed retention replaces the snapshot, whose create carries it.)
            Self::RedshiftServerlessNamespace => {
                update_redshift_serverless_namespace(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            Self::RedshiftServerlessWorkgroup => {
                update_redshift_serverless_workgroup(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateDestination` "updates the specified destination of the specified
            // Firehose stream", and "the target Firehose stream remains active while the
            // configurations are updated" — so a declaration stating the stream's
            // destination differently is applied to the stream the stack created, which
            // keeps its name, its ARN and the source it reads, rather than replacing it
            // ([`firehose_delivery_streams::update_delivery_stream`]).
            Self::FirehoseDeliveryStream => {
                update_delivery_stream(ctx, region, resource, declared).await
            }
            // Every setting of an Amazon ECR repository that service can write on a
            // repository that exists has a call of its own — its lifecycle policy through
            // `PutLifecyclePolicy`, its resource-based policy through `SetRepositoryPolicy`,
            // its tag mutability and its scanning configuration through their own puts — so a
            // changed declaration is applied to the repository the stack created, which keeps
            // its name, its ARN and its URI, rather than replacing it
            // ([`update_repository`]). Its tags travel separately
            // ([`Self::retag`]).
            Self::EcrRepository => update_repository(ctx, region, resource, declared).await,
            // And the registry's own signing configuration through the PUT that wrote it,
            // documented "Creates or updates the registry's signing configuration": the call
            // that created it is the call that applies a changed declaration to it, and the
            // registry holds one, so the configuration in force is replaced rather than
            // doubled.
            Self::EcrSigningConfiguration => {
                put_signing_configuration(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateConfigurationManager` takes the `ManagerArn` Quick Setup minted together
            // with a new name and description — everything that call can change on a manager
            // that exists — so a changed declaration of either reaches the manager the stack
            // created rather than replacing it. Its DEFINITIONS are not members of that call at
            // all, which is why a declaration restating them replaces the resource
            // ([`super::template::replacement_properties`]); its tags travel separately
            // ([`Self::retag`]).
            Self::SsmQuickSetupConfigurationManager => {
                update_configuration_manager(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // Every other type's in-place update is its own entry here, exactly as its
            // create and delete are: until one is written, nothing is asked of the owning
            // service and the deployment says so.
            // `UpdateSuiteDefinition` takes the suite's id together with its WHOLE
            // configuration — the same document the create carried — so a changed declaration
            // reaches the suite AWS IoT Core Device Advisor already holds rather than replacing
            // it, and that service allocates the new version label.
            Self::IotCoreDeviceAdvisorSuiteDefinition => {
                update_suite_definition(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // `UpdateServiceNetwork` takes the network's `authType` and nothing else, and
            // `UpdateAccessLogSubscription` takes the subscription's destination and nothing
            // else, so a changed declaration of either reaches the resource Amazon VPC Lattice
            // already holds rather than replacing it. Every other property of either type is
            // create-only ([`super::super::template::replacement_properties`]).
            Self::VpcLatticeServiceNetwork => {
                update_vpc_lattice_service_network(ctx, region, resource, declared).await
            }
            Self::VpcLatticeAccessLogSubscription => {
                update_vpc_lattice_access_log_subscription(ctx, region, resource, declared).await
            }
            // AWS Backup publishes no `UpdateBackupVault`: what a vault that exists can be
            // given a new value for is exactly the three configurations its own calls carry —
            // its lock, its resource policy and its notifications — so a changed declaration
            // of any of them reaches the vault AWS Backup already holds rather than replacing
            // it. Its name and its encryption key are create-only
            // ([`super::super::template::replacement_properties`]), and its tags travel
            // through [`Self::retag`].
            Self::BackupVault => update_backup_vault(ctx, region, resource, declared).await,
            _ => Ok(None),
        }
    }

    /// Bring the tags of a resource the stack ALREADY has back into step with the ones
    /// CloudFormation gives it ([`Self::create`]'s `propagated`), through the owning
    /// service's own tagging API.
    ///
    /// A stack operation's `Tags` are "propagated to the resources that CloudFormation
    /// creates in the stack", and a deployment that states them differently states them for
    /// the resources the stack already has as much as for the ones it creates — which is why
    /// this is asked of every carried-forward resource rather than only of one whose
    /// declaration changed: a stack's tags change without any resource's declaration
    /// changing at all.
    ///
    /// `Ok(())` for a type with no arm here: nothing is asked of its service, exactly as a
    /// type with no create arm is left declared-only. Each arm is expected to be
    /// IDEMPOTENT — it is called on every deployment — so a resource whose tags already
    /// agree with the set has nothing called about it.
    ///
    /// `declared` is the property document this deployment declares for the resource, whose
    /// own `Tags` are part of the set the resource must end up carrying.
    pub(crate) async fn retag(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
        propagated: &[(String, String)],
    ) -> Result<(), Refusal> {
        let propagated = &self.applicable_tags(propagated);
        match self {
            // `PutDashboard`'s `Tags` member cannot carry a change to a dashboard's tags:
            // that member is documented as ignored when the call updates a dashboard that
            // exists, so the change reaches CloudWatch through its own tagging calls.
            Self::CloudWatchDashboard => {
                retag_dashboard(ctx, region, resource, declared, propagated).await
            }
            // A repository's tags are Amazon ECR's own `TagResource`/`UntagResource`:
            // `CreateRepository`'s `tags` list applies to the repository that call creates and
            // to no repository that already exists, which is why
            // [`update_repository`] does not carry them — this is where the
            // set a repository is to end up carrying is reconciled.
            Self::EcrRepository => {
                retag_repository(ctx, region, resource, declared, propagated).await
            }
            // A role's tags are IAM's own `TagRole`/`UntagRole`, which is also why
            // [`update_role`] does not carry them: this is the one place the tag set a role
            // is to end up with is reconciled. Boxed, as the heavy arms of this file's other
            // matches are: the reconciliation reaches IAM through that service's own SDK
            // client, and this match's size is what every carried-forward resource pays.
            Self::IamRole => {
                Box::pin(retag_role(ctx, region, resource, declared, propagated)).await
            }
            // A user's tags are IAM's own `TagUser`/`UntagUser`, for the same reason a
            // role's are not carried by [`update_iam_user`].
            Self::IamUser => {
                Box::pin(retag_iam_user(ctx, region, resource, declared, propagated)).await
            }
            // A key's tags are KMS's own `TagResource`/`UntagResource`, which is why
            // [`update_kms_key`] does not carry them either.
            Self::KmsKey => retag_kms_key(ctx, region, resource, declared, propagated).await,
            // A topic's tags are SNS's own `TagResource`/`UntagResource` — `Tags` is a
            // member of `CreateTopic` and no attribute of a topic — which is why
            // [`update_topic`] does not carry them.
            Self::SnsTopic => retag_topic(ctx, region, resource, declared, propagated).await,
            // A queue's tags are Amazon SQS's own `TagQueue`/`UntagQueue`: `CreateQueue`'s
            // `tags` map applies to the queue that call creates, and [`update_queue`] carries
            // only the tags the DECLARATION states — so this is where the set a queue is to
            // end up carrying, the stack's own tags included, is reconciled.
            Self::SqsQueue => retag_queue(ctx, region, resource, declared, propagated).await,
            // A schedule group's tags are EventBridge Scheduler's own `TagResource` and
            // `UntagResource`: `CreateScheduleGroup`'s `Tags` member applies to the group
            // that call creates and to no group that already exists, so this is where the
            // set a group is to end up carrying is reconciled.
            Self::SchedulerScheduleGroup => {
                retag_schedule_group(ctx, region, resource, declared, propagated).await
            }
            // A resource group's tags are Resource Groups' own `Tag` and `Untag`:
            // `CreateGroup`'s `Tags` member applies to the group that call creates and to no
            // group that already exists, so this is where the set a group is to end up
            // carrying is reconciled.
            Self::ResourceGroupsGroup => {
                retag_resource_group(ctx, region, resource, declared, propagated).await
            }
            // A stream's tags are Amazon Kinesis Data Streams' own `AddTagsToStream` and
            // `RemoveTagsFromStream`: `CreateStream`'s `Tags` member applies to the stream
            // that call creates and to no stream that already exists, so this is where the
            // set a stream is to end up carrying is reconciled.
            Self::KinesisStream => {
                retag_kinesis_stream(ctx, region, resource, declared, propagated).await
            }
            // An environment's tags are Amazon MWAA's own `TagResource` and `UntagResource`:
            // `CreateEnvironment`'s `Tags` member applies to the environment that call
            // creates, and `UpdateEnvironment` carries no tags at all, so this is where the
            // set an environment is to end up carrying is reconciled.
            Self::MwaaEnvironment => {
                retag_mwaa_environment(ctx, region, resource, declared, propagated).await
            }
            // A domain's tag set is carried by `UpdateDomain`'s own `Tags` member, which
            // REPLACES the map the domain holds: `CreateDomain`'s applies to the domain that
            // call creates, and the configuration update leaves tags alone, so this is where
            // the set a domain is to end up carrying is reconciled.
            Self::CustomerProfilesDomain => {
                retag_customer_profiles_domain(ctx, region, resource, declared, propagated).await
            }
            // A retention rule's tags are Recycle Bin's own `TagResource` and `UntagResource`:
            // `CreateRule`'s `Tags` member applies to the rule that call creates, and
            // `UpdateRule` carries no tags at all, so this is where the set a rule is to end
            // up carrying is reconciled.
            Self::RbinRule => {
                retag_rbin_rule(ctx, region, resource, declared, propagated).await
            }
            // A lifecycle policy's tags are Amazon Data Lifecycle Manager's own `TagResource` and
            // `UntagResource`: `CreateLifecyclePolicy`'s `Tags` map applies to the policy that
            // call creates, and `UpdateLifecyclePolicy` carries no tags at all, so this is where
            // the set a policy is to end up carrying is reconciled.
            Self::DlmLifecyclePolicy => {
                retag_dlm_lifecycle_policy(ctx, region, resource, declared, propagated).await
            }
            // A hypervisor's tags are Backup Gateway's own `TagResource` and `UntagResource`:
            // `ImportHypervisorConfiguration`'s `Tags` list applies to the hypervisor that call
            // imports, and `UpdateHypervisor` carries no tags at all, so this is where the set
            // a hypervisor is to end up carrying is reconciled.
            Self::BackupGatewayHypervisor => {
                retag_backup_gateway_hypervisor(ctx, region, resource, declared, propagated).await
            }
            // An analyzer's tags are IAM Access Analyzer's own `TagResource` and
            // `UntagResource`: `CreateAnalyzer`'s `tags` map applies to the analyzer that call
            // creates, and `UpdateAnalyzer` carries no tags at all, so this is where the set an
            // analyzer is to end up carrying is reconciled. An ARCHIVE RULE has no tags of its
            // own, which that arm decides from the kind it carries.
            Self::AccessAnalyzerResource(kind) => {
                retag_access_analyzer_resource(ctx, region, kind, resource, declared, propagated)
                    .await
            }
            // A dashboard's and a scheduled report's tags are that service's own
            // `TagResource` and `UntagResource`: each create's `resourceTags` list applies to
            // the resource it created, and neither update carries a tag at all, so this is
            // where the set either resource is to end up carrying is reconciled. One arm for
            // both, because `ResourceArn` names either kind.
            Self::BcmDashboard | Self::BcmScheduledReport => {
                retag_bcm_resource(ctx, region, resource, declared, propagated).await
            }
            // A graph's tags are Neptune Analytics' own `TagResource` and `UntagResource`:
            // `CreateGraph`'s `tags` map applies to the graph that call creates, and
            // `UpdateGraph` carries no tags at all, so this is where the set a graph is to end
            // up carrying is reconciled. A SNAPSHOT is in the same position: its `Tags` are the
            // one property of that type a deployment can change without replacing it, and that
            // service changes a snapshot's tags through those same two calls.
            Self::NeptuneGraphGraph => {
                retag_neptune_graph(ctx, region, resource, declared, propagated).await
            }
            Self::NeptuneGraphGraphSnapshot => {
                retag_graph_snapshot(ctx, region, resource, declared, propagated).await
            }
            // Every Amazon EC2 resource's tags are that service's own `CreateTags` and
            // `DeleteTags`: a create's tagging call applies to the resource it created and to
            // no resource that already exists, and none of these types' update calls carries a
            // tag — so this is where the set one of them is to end up carrying is reconciled
            // ([`ec2_in_place::retag_ec2_resource`]). A LAUNCH TEMPLATE states its own tags in
            // the `TagSpecifications` entry that names the template itself rather than in a
            // `Tags` property, so its set is read the way its create reads it
            // ([`launch_template_tags`]).
            Self::Ec2Vpc | Self::Ec2Subnet | Self::Ec2SecurityGroup | Self::Ec2Volume => {
                retag_ec2_resource(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    &tags_of(declared, propagated),
                )
                .await
            }
            Self::Ec2LaunchTemplate => {
                retag_ec2_resource(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    &launch_template_tags(declared, propagated),
                )
                .await
            }
            // An Auto Scaling group's tags are Amazon EC2 Auto Scaling's own
            // `CreateOrUpdateTags` and `DeleteTags`: `CreateAutoScalingGroup`'s `Tags` member
            // applies to the group that call creates, and `UpdateAutoScalingGroup` carries no
            // tags at all — the member does not exist on it — so this is where the set a group
            // is to end up carrying is reconciled, propagation flag included
            // ([`retag_auto_scaling_group`]).
            Self::AutoScalingGroup => {
                retag_auto_scaling_group(ctx, region, resource, declared, propagated).await
            }
            // A suite definition's tags are that service's own `TagResource` and
            // `UntagResource`: `CreateSuiteDefinition`'s `tags` map applies to the suite that
            // call creates, and `UpdateSuiteDefinition` carries no tags at all, so this is
            // where the set a suite is to end up carrying is reconciled.
            Self::IotCoreDeviceAdvisorSuiteDefinition => {
                retag_suite_definition(ctx, region, resource, declared, propagated).await
            }
            // A configuration manager's tags are Quick Setup's own `TagResource` and
            // `UntagResource`: `CreateConfigurationManager`'s `Tags` map applies to the manager
            // that call creates, and `UpdateConfigurationManager` carries no tags at all, so
            // this is where the set a manager is to end up carrying is reconciled.
            Self::SsmQuickSetupConfigurationManager => {
                retag_configuration_manager(ctx, region, resource, declared, propagated).await
            }
            // An investigation group's tags are Amazon CloudWatch investigations' own
            // `TagResource` and `UntagResource`: `CreateInvestigationGroup`'s `tags` map
            // applies to the group that call creates, and `UpdateInvestigationGroup` carries no
            // tags at all, so this is where the set a group is to end up carrying is
            // reconciled.
            Self::AiOpsInvestigationGroup => {
                retag_investigation_group(ctx, region, resource, declared, propagated).await
            }
            // Either Amazon VPC Lattice resource's tags are that service's own `TagResource`
            // and `UntagResource`: each create's `tags` map applies to the resource that call
            // creates, and neither update carries tags at all, so this is where the set either
            // resource is to end up carrying is reconciled — by the ARN that service minted,
            // which is the resource's physical id.
            Self::VpcLatticeServiceNetwork | Self::VpcLatticeAccessLogSubscription => {
                retag_vpc_lattice_resource(ctx, region, resource, declared, propagated).await
            }
            // A vault's tags are AWS Backup's own `TagResource` and `UntagResource`:
            // `CreateBackupVault`'s `BackupVaultTags` map applies to the vault that call
            // creates, and that service publishes no call that changes a vault's
            // configuration at all, so this is where the set a vault is to end up carrying is
            // reconciled — by the ARN it minted, which the type publishes as an attribute.
            Self::BackupVault => {
                retag_backup_vault(ctx, region, resource, declared, propagated).await
            }
            // Every Amazon RDS resource's tags are that service's own `AddTagsToResource` and
            // `RemoveTagsFromResource`, addressed by the resource's ARN: each create's `Tags`
            // list applies to the resource that call creates, and no call that changes a
            // resource which already exists carries a tag at all — `ModifyDBInstance` and its
            // siblings publish no `Tags` member — so this is where the set one of them is to
            // end up carrying is reconciled, for every type whose tags that service holds
            // ([`rds_tags::retag_rds_resource`], over
            // [`rds_tags::reported_arn_member`]'s catalogue).
            owner if reported_arn_member(owner).is_some() => {
                retag_rds_resource(ctx, region, owner, resource, declared, propagated).await
            }
            // Every other type's re-tagging is its own entry here, for the same reason its
            // create, update and delete are: the tag set is the same for every type, but the
            // calls that apply it are each service's own.
            _ => Ok(()),
        }
    }
}
