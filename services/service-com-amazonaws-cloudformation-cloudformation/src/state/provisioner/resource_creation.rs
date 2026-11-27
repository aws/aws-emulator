// SPDX-License-Identifier: CC0-1.0

//! CREATING one resource in the service that owns it: the arm of [`Provisioner`] that makes
//! the call a declared resource is brought into existence by ([`Provisioner::create`]),
//! together with the one question every taggable arm of it asks — which of the tags
//! CloudFormation propagates the owning service will take ([`Provisioner::applicable_tags`]).
//!
//! One concern beside the catalogue's readiness walk ([`super::lifecycle_progress`]), its
//! in-place changes ([`super::in_place_changes`]), its reads ([`super::reported_configuration`])
//! and its delete, in a file of its own so each stays inside the line bound a change may touch.

use super::*;

impl Provisioner {
    /// Create one resource in the service that owns it, and report back what that service
    /// decided about it ([`Created`]): the attributes it reported (an ARN it minted, say),
    /// which replace the ones CloudFormation's naming rules computed, and — for a type
    /// whose identity is that service's
    /// ([`super::template::identity_is_service_assigned`]) — the physical id it assigned.
    /// `Err` is that service's own refusal.
    ///
    /// `propagated` are the tags CloudFormation gives the resource — the stack's own plus
    /// the three that name the stack and this resource's place in it
    /// ([`super::provisioning::StackProvenance::tags_for`]) — and every arm whose type is
    /// taggable applies them ([`tags_of`]) beside the ones the declaration itself asks for,
    /// so the owning service's tag read on the handle the stack publishes answers with
    /// them. The one arm that applies none is `AWS::WAFRegional::IPSet`, whose type
    /// publishes no `Tags` property at all: CloudFormation propagates no tags to a resource
    /// type that is not taggable.
    pub(crate) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        propagated: &[(String, String)],
    ) -> Result<Created, Refusal> {
        let propagated = &self.applicable_tags(propagated);
        match self {
            Self::FirehoseDeliveryStream => {
                create_delivery_stream(ctx, region, resource, propagated).await
            }
            Self::KinesisStream => {
                create_kinesis_stream(ctx, region, resource, propagated).await
            }
            Self::CloudWatchMetricStream => {
                put_metric_stream(ctx, region, resource, propagated).await
            }
            Self::CloudWatchDashboard => {
                put_dashboard(ctx, region, resource, propagated).await
            }
            // `AWS::CloudWatch::Alarm` publishes no `Tags` property at all — CloudWatch holds
            // an alarm's tags through its own tagging calls and `PutMetricAlarm` takes a
            // `Tags` member the type does not declare — so no tags are propagated to one.
            Self::CloudWatchAlarm => put_metric_alarm(ctx, region, resource, propagated).await,
            // A composite alarm's own `Tags` member is one `PutCompositeAlarm` takes, so the
            // tags CloudFormation propagates travel on the create like the declaration's own.
            Self::CloudWatchCompositeAlarm => {
                put_composite_alarm(ctx, region, resource, propagated).await
            }
            Self::EmrCluster => run_job_flow(ctx, region, resource, propagated).await,
            // `AWS::EMR::SecurityConfiguration` publishes no `Tags` property and
            // `CreateSecurityConfiguration` takes none, so no tags are propagated to one.
            Self::EmrSecurityConfiguration => {
                create_emr_security_configuration(ctx, region, resource).await
            }
            // `AWS::EMR::Studio` publishes `Tags`, and `CreateStudio` takes a `TagList` of
            // its own, so a Studio a stack creates carries the tags CloudFormation gives it
            // beside the ones the declaration asks for.
            Self::EmrStudio => create_emr_studio(ctx, region, resource, propagated).await,
            // `AWS::EMR::StudioSessionMapping` publishes no `Tags` property and
            // `CreateStudioSessionMapping` takes none: a mapping is no taggable resource, so
            // no tags are propagated to one.
            Self::EmrStudioSessionMapping => {
                create_emr_studio_session_mapping(ctx, region, resource).await
            }
            Self::EcrPublicRepository => {
                create_public_repository(ctx, region, resource, propagated).await
            }
            Self::EcrRepository => create_repository(ctx, region, resource, propagated).await,
            // The registry's signing configuration publishes no `Tags` property and its put
            // takes no tag member at all — it is the registry's own configuration rather than a
            // named resource — so no tags are propagated to one.
            Self::EcrSigningConfiguration => {
                create_signing_configuration(ctx, region, resource).await
            }
            // An exclusion on the registry's pull-time recording is one entry of that
            // registry's own list rather than a named resource: the type publishes no `Tags`
            // property and `RegisterPullTimeUpdateExclusion` takes no tag member, so no tags
            // are propagated to one.
            Self::EcrPullTimeUpdateExclusion => {
                register_pull_time_update_exclusion(ctx, region, resource).await
            }
            Self::Ec2Vpc => create_ec2_vpc(ctx, region, resource, propagated).await,
            Self::Ec2Subnet => create_ec2_subnet(ctx, region, resource, propagated).await,
            Self::Ec2SecurityGroup => {
                create_ec2_security_group(ctx, region, resource, propagated).await
            }
            Self::Ec2Volume => create_ec2_volume(ctx, region, resource, propagated).await,
            Self::Ec2LaunchTemplate => {
                create_ec2_launch_template(ctx, region, resource, propagated).await
            }
            Self::AutoScalingGroup => {
                create_auto_scaling_group(ctx, region, resource, propagated).await
            }
            // `AWS::AutoScaling::ScalingPolicy` publishes no `Tags` property and
            // `PutScalingPolicy` takes none, so no tag is propagated to a scaling policy.
            Self::AutoScalingScalingPolicy => put_scaling_policy(ctx, region, resource).await,
            // Neither `AWS::AutoScaling::LifecycleHook`, `AWS::AutoScaling::ScheduledAction`
            // nor `AWS::AutoScaling::WarmPool` publishes a `Tags` property, and none of the
            // three `Put…` calls takes one, so no tag is propagated to any of them.
            Self::AutoScalingLifecycleHook => put_lifecycle_hook(ctx, region, resource).await,
            Self::AutoScalingScheduledAction => {
                put_scheduled_action(ctx, region, resource).await
            }
            Self::AutoScalingWarmPool => put_warm_pool(ctx, region, resource).await,
            // Boxed, as the deeper arms of this match are: a bucket's create sends the
            // declared access controls and notification configuration after the create,
            // so this arm's own future is one of the largest here and a stack it shares
            // with the provisioning walk above it is the scarce resource.
            Self::S3Bucket => Box::pin(create_bucket(ctx, region, resource, propagated)).await,
            // `AWS::S3::BucketPolicy` publishes `Bucket` and `PolicyDocument` and no
            // `Tags`: a bucket's policy is no taggable resource, so no tags are
            // propagated to one.
            // Boxed for the same reason the bucket's create is: a put of the declared
            // document reaches Amazon S3 through that service's own SDK client.
            Self::S3BucketPolicy => Box::pin(put_bucket_policy(ctx, region, resource)).await,
            // `AWS::S3Tables::TableBucket` publishes `Tags`, and `CreateTableBucket` takes
            // a tag MAP of its own, so a table bucket a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::S3TablesTableBucket => {
                create_table_bucket(ctx, region, resource, propagated).await
            }
            // `AWS::S3Tables::Namespace` publishes no `Tags` property and `CreateNamespace`
            // takes none, so no tags are propagated to a namespace.
            Self::S3TablesNamespace => create_namespace(ctx, region, resource).await,
            Self::S3VectorsVectorBucket => {
                create_vector_bucket(ctx, region, resource, propagated).await
            }
            Self::S3VectorsIndex => create_index(ctx, region, resource, propagated).await,
            // Boxed for the reason the bucket's arm above is: each of these reaches IAM
            // through that service's own SDK client, whose future is among the largest this
            // match holds, and the stack it shares with the provisioning walk above it is the
            // scarce resource. The sizes of this match's arms are what every deployment pays,
            // whichever type it declares ([`super::provisioning::Owner::create`] boxes this
            // whole match for the same reason).
            Self::IamRole => Box::pin(create_role(ctx, region, resource, propagated)).await,
            // `AWS::IAM::InstanceProfile` publishes no `Tags` property and
            // `CreateInstanceProfile` takes none from this arm, so no tags are propagated to
            // a profile.
            Self::IamInstanceProfile => {
                Box::pin(create_instance_profile(ctx, region, resource)).await
            }
            Self::IamUser => Box::pin(create_iam_user(ctx, region, resource, propagated)).await,
            // `AWS::IAM::AccessKey` publishes no `Tags` property and `CreateAccessKey` takes
            // none, so no tags are propagated to an access key.
            Self::IamAccessKey => Box::pin(create_iam_access_key(ctx, region, resource)).await,
            // `AWS::IAM::ManagedPolicy` publishes no `Tags` property and `CreatePolicy`
            // takes none from this arm, so no tags are propagated to a policy.
            Self::IamManagedPolicy => Box::pin(create_managed_policy(ctx, region, resource)).await,
            Self::CodePipelinePipeline => {
                create_pipeline(ctx, region, resource, propagated).await
            }
            Self::CodePipelineCustomActionType => {
                create_custom_action_type(ctx, region, resource, propagated).await
            }
            // `AWS::CodePipeline::Webhook` publishes `Tags`, and `PutWebhook` takes a `tags`
            // list of its own, so a webhook a stack creates carries the tags CloudFormation
            // gives it beside the ones the declaration asks for.
            Self::CodePipelineWebhook => put_webhook(ctx, region, resource, propagated).await,
            Self::EcsCluster => create_cluster(ctx, region, resource, propagated).await,
            // `AWS::ECS::Service` publishes `Tags`, which `CreateService` takes as its own
            // `tags` list, so a service a stack creates carries the tags CloudFormation gives
            // it beside the ones the declaration asks for.
            // BOXED, for the reason the other heavy arms of this match are: the call's future
            // is the owning service's whole handler, which schedules the service's tasks.
            Self::EcsService => {
                Box::pin(create_ecs_service(ctx, region, resource, propagated)).await
            }
            // `AWS::ECS::CapacityProvider` publishes `Tags`, which `CreateCapacityProvider`
            // takes as its own `tags` list, so a provider a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::EcsCapacityProvider => {
                create_ecs_capacity_provider(ctx, region, resource, propagated).await
            }
            // `AWS::ECS::ClusterCapacityProviderAssociations` publishes no `Tags` property at
            // all — the configuration is the CLUSTER's, and the cluster carries the tags — so
            // no tags are propagated to one. The create IS the put: that one call sets the
            // whole configuration of the cluster it addresses.
            Self::EcsClusterCapacityProviderAssociations => {
                put_cluster_capacity_providers(ctx, region, resource, &resource.properties).await
            }
            // `CreateTaskSet` takes a `tags` list of its own, so a task set a stack creates
            // carries the tags CloudFormation gives it beside the ones the declaration asks
            // for.
            // BOXED, for the reason the other heavy arms of this match are: the call's future
            // is the owning service's whole handler, which places the task set's tasks.
            Self::EcsTaskSet => {
                Box::pin(create_ecs_task_set(ctx, region, resource, propagated)).await
            }
            // `AWS::ECS::PrimaryTaskSet` publishes no `Tags` property and
            // `UpdateServicePrimaryTaskSet` takes none: the resource is which task set of a
            // service serves production traffic, so no tags are propagated to one.
            Self::EcsPrimaryTaskSet => {
                update_service_primary_task_set(ctx, region, resource, &resource.properties).await
            }
            Self::SqsQueue => create_queue(ctx, region, resource, propagated).await,
            // Neither policy type publishes a `Tags` property — a policy is an attribute of
            // the queues it names, and those carry the tags — so no tags are propagated to
            // one.
            Self::SqsQueuePolicy => put_queue_policy(ctx, region, resource).await,
            Self::SageMakerModelPackageGroup => {
                create_model_package_group(ctx, region, resource, propagated).await
            }
            // Each of the three creates takes a `Tags` list, so a model, a notebook instance
            // and a lifecycle configuration a stack creates each carry the tags
            // CloudFormation gives them beside the ones their declaration asks for.
            Self::SageMakerModel => {
                create_sagemaker_model(ctx, region, resource, propagated).await
            }
            // Both endpoint types take a `Tags` list of their own, so a configuration and an
            // endpoint a stack creates each carry the tags CloudFormation gives them beside
            // the ones their declaration asks for.
            Self::SageMakerEndpointConfig => {
                create_sagemaker_endpoint_config(ctx, region, resource, propagated).await
            }
            Self::SageMakerEndpoint => {
                create_sagemaker_endpoint(ctx, region, resource, propagated).await
            }
            Self::SageMakerNotebookInstance => {
                create_sagemaker_notebook_instance(ctx, region, resource, propagated).await
            }
            Self::SageMakerNotebookInstanceLifecycleConfig => {
                create_sagemaker_notebook_instance_lifecycle_config(
                    ctx, region, resource, propagated,
                )
                .await
            }
            Self::SnsTopic => create_topic(ctx, region, resource, propagated).await,
            // Neither topic-policy type publishes a `Tags` property — a policy is an
            // attribute of the topics it names, and those carry the tags — so no tags are
            // propagated to one.
            Self::SnsTopicPolicy => put_topic_policy(ctx, region, resource).await,
            // A subscription publishes no `Tags` property — SNS holds tags on topics alone —
            // so no tags are propagated to one.
            Self::SnsSubscription => subscribe(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                create_experiment_template(ctx, region, resource, propagated).await
            }
            // `AWS::WAFRegional::IPSet` publishes `Name` and `IPSetDescriptors` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::WafRegionalIpSet => create_ip_set(ctx, region, resource).await,
            // Each WAFv2 type publishes a `Tags` property and its create takes that list as
            // its own member, so a stack's resources in WAF carry the tags CloudFormation
            // gives them. Every arm is the same call sequence under the type's own
            // operation names ([`Wafv2Calls`]).
            Self::Wafv2WebAcl => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_WEB_ACL_CALLS).await
            }
            Self::Wafv2RuleGroup => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_RULE_GROUP_CALLS)
                    .await
            }
            Self::Wafv2IpSet => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_IP_SET_CALLS).await
            }
            Self::Wafv2RegexPatternSet => {
                create_wafv2_entity(
                    ctx,
                    region,
                    resource,
                    propagated,
                    WAFV2_REGEX_PATTERN_SET_CALLS,
                )
                .await
            }
            // An association publishes no `Tags` property and `AssociateWebACL` takes none:
            // the binding is not a taggable resource — AWS WAF holds tags on its own
            // entities — so no tags are propagated to one.
            Self::Wafv2WebAclAssociation => associate_web_acl(ctx, region, resource).await,
            // An association publishes no `Tags` property and `AssociateCertificate` takes
            // none: the trust is not a taggable resource of MediaConvert's — that service
            // holds tags on its queues, presets, job templates and jobs — so no tags are
            // propagated to one.
            Self::MediaConvertCertificateAssociation => {
                associate_mediaconvert_certificate(ctx, region, resource).await
            }
            // Neither Lake Formation permission type publishes a `Tags` property and neither
            // `GrantPermissions` nor `RevokePermissions` takes one: a grant is not a taggable
            // resource — that service holds tags on the LF-tags its own tagging calls name —
            // so no tags are propagated to one, and the declared document IS the grant
            // ([`lake_formation_permissions`]).
            Self::LakeFormationPermissions(grant) => {
                grant_lake_formation_permissions(ctx, region, grant, resource).await
            }
            // A data cells filter publishes no `Tags` property either, and
            // `CreateDataCellsFilter` takes none: it is a resource OF a catalog table rather
            // than one Lake Formation holds tags on, so no tags are propagated to one and the
            // declared document IS the `TableData` that create takes
            // ([`lake_formation_data_cells_filters`]).
            // Boxed like the arms beside it whose future is one of the larger ones here: a
            // filter's create carries the whole declared document onto an in-process call, and
            // the enclosing match's future is as large as its widest arm.
            Self::LakeFormationDataCellsFilter => {
                Box::pin(create_lake_formation_data_cells_filter(ctx, region, resource)).await
            }
            // A Client VPN endpoint's properties ARE `CreateClientVpnEndpoint`'s own members,
            // each carried onto the call under the member of the same name in the spelling
            // Amazon EC2's query protocol gives it; the tags CloudFormation propagates ride
            // on that call's own `TagSpecifications` member, which is where this type states
            // its own ([`client_vpn`]).
            Self::Ec2ClientVpnEndpoint => {
                create_client_vpn_endpoint(ctx, region, resource, propagated).await
            }
            // An authorization rule publishes no `Tags` property at all — it is no entity of
            // Amazon EC2's own but a grant ON an endpoint, and `AuthorizeClientVpnIngress`
            // takes no tags — so no tags are propagated to one and the declaration IS the
            // request.
            Self::Ec2ClientVpnAuthorizationRule => {
                authorize_client_vpn_ingress(ctx, region, resource).await
            }
            // An `AWS::Glue::Job`'s properties ARE `CreateJob`'s own members, so the
            // declared document is the request, with the one member CloudFormation decides
            // (the job's name) set on the way out and the tag MAP that call takes merged
            // from the declaration's own and the stack's.
            Self::GlueJob => create_glue_job(ctx, region, resource, propagated).await,
            // A trigger IS taggable: its `Tags` property is the tag MAP `CreateTrigger`'s own
            // `Tags` member takes, so it carries the stack's tags beside its own, and every
            // other property of the type is a member of that same call.
            Self::GlueTrigger => create_glue_trigger(ctx, region, resource, propagated).await,
            // The two catalog types publish no `Tags` property at all — neither is a
            // taggable resource, AWS Glue holding tags on the catalog entities its own
            // tagging calls name — so no tags are propagated to one, and the declared
            // document IS the create's request with the name CloudFormation decided set
            // inside the input structure that call takes it in.
            Self::GlueDatabase => create_glue_database(ctx, region, resource).await,
            Self::GlueTable => create_glue_table(ctx, region, resource).await,
            // A crawler IS taggable: its `Tags` property is the tag MAP `CreateCrawler`'s
            // own `Tags` member takes, so it carries the stack's tags beside its own.
            Self::GlueCrawler => create_glue_crawler(ctx, region, resource, propagated).await,
            // An optimizer publishes no `Tags` property either: it is a resource OF a table
            // rather than one AWS Glue holds tags on, so no tags are propagated to one and
            // the declared document is the create's request with its configuration re-cased
            // to the members that call takes.
            Self::GlueTableOptimizer => {
                create_glue_table_optimizer(ctx, region, resource).await
            }
            // `AWS::Grafana::Workspace` publishes no `Tags` property either, so the type
            // is not taggable and no stack tags are propagated to it.
            Self::GrafanaWorkspace => create_workspace(ctx, region, resource).await,
            // `AWS::OpenSearchServerless::SecurityPolicy` publishes no `Tags` property and
            // `CreateSecurityPolicy` takes none, so a policy carries no propagated tag.
            Self::OpenSearchServerlessSecurityPolicy => {
                create_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServerlessCollection => {
                create_collection(ctx, region, resource, propagated).await
            }
            // `AWS::OpenSearchServerless::VpcEndpoint` publishes no `Tags` property and
            // `CreateVpcEndpoint` takes none, so an endpoint carries no propagated tag.
            Self::OpenSearchServerlessVpcEndpoint => {
                create_vpc_endpoint(ctx, region, resource).await
            }
            Self::OpenSearchServiceDomain => {
                create_domain(ctx, region, resource, propagated).await
            }
            Self::GuardDutyDetector => {
                create_detector(ctx, region, resource, propagated).await
            }
            Self::GuardDutyIpSet => {
                create_guardduty_ip_set(ctx, region, resource, propagated).await
            }
            Self::KmsKey => create_kms_key(ctx, region, resource, propagated).await,
            // `AWS::KMS::Alias` publishes `AliasName` and `TargetKeyId` and no `Tags`: an
            // alias is not a taggable resource — KMS holds tags on KEYS — so no tags are
            // propagated to one.
            Self::KmsAlias => create_kms_alias(ctx, region, resource).await,
            Self::MskCluster => create_msk_cluster(ctx, region, resource, propagated).await,
            Self::MskServerlessCluster => {
                create_msk_serverless_cluster(ctx, region, resource, propagated).await
            }
            // `AWS::MSK::Configuration` publishes no `Tags` property — a configuration is not
            // a taggable Amazon MSK resource — so no tags are propagated to one.
            Self::MskConfiguration => create_msk_configuration(ctx, region, resource).await,
            Self::MqBroker => create_mq_broker(ctx, region, resource, propagated).await,
            // A configuration is created by its own call and then given the document the
            // declaration states, which is a revision of it.
            Self::MqConfiguration => {
                create_mq_configuration(ctx, region, resource, propagated).await
            }
            Self::RdsDbInstance => {
                create_db_instance(ctx, region, resource, propagated).await
            }
            // `AWS::RDS::DBCluster` publishes `Tags` as the `{Key, Value}` list
            // `CreateDBCluster`'s own `Tags` member takes, so a cluster a stack creates
            // carries the tags CloudFormation gives it beside the declared ones.
            Self::RdsDbCluster => {
                // Boxed, like the other arms of this catalogue: the enclosing future holds
                // one pointer instead of this arm's whole state.
                Box::pin(create_db_cluster(ctx, region, resource, propagated)).await
            }
            // `AWS::ElastiCache::CacheCluster` publishes `Tags` as the `{Key, Value}` list
            // `CreateCacheCluster`'s own `Tags` member takes, so a cluster a stack creates
            // carries the tags CloudFormation gives it beside the declared ones.
            Self::ElastiCacheCacheCluster => {
                // Boxed, like every other arm of this catalogue should be as it grows: the
                // enclosing future holds one pointer instead of this arm's whole state, so
                // adding a type does not enlarge the future a nested stack's provisioning
                // walk keeps on the stack.
                Box::pin(create_cache_cluster(ctx, region, resource, propagated)).await
            }
            // `AWS::ElastiCache::ServerlessCache` publishes `Tags` as the same `{Key, Value}`
            // list, which `CreateServerlessCache` takes as its own `Tags` member, so a cache
            // a stack creates carries the tags CloudFormation gives it beside the declared
            // ones.
            Self::ElastiCacheServerlessCache => {
                Box::pin(create_serverless_cache(ctx, region, resource, propagated)).await
            }
            // `AWS::ElastiCache::ReplicationGroup` publishes `Tags` as the same `{Key, Value}`
            // list, which `CreateReplicationGroup` takes as its own `Tags` member, so a group
            // a stack creates carries the tags CloudFormation gives it beside the declared
            // ones.
            Self::ElastiCacheReplicationGroup => {
                Box::pin(create_replication_group(ctx, region, resource, propagated)).await
            }
            // `AWS::ElastiCache::SubnetGroup` publishes `Tags` as that same list, which
            // `CreateCacheSubnetGroup` takes as its own `Tags` member.
            Self::ElastiCacheSubnetGroup => {
                Box::pin(create_cache_subnet_group(ctx, region, resource, propagated)).await
            }
            Self::RedshiftCluster => {
                create_redshift_cluster(ctx, region, resource, propagated).await
            }
            Self::RedshiftClusterSubnetGroup => {
                create_cluster_subnet_group(ctx, region, resource, propagated).await
            }
            // `AWS::Redshift::ClusterParameterGroup` publishes `Tags` as the `{Key, Value}`
            // list `CreateClusterParameterGroup`'s own `Tags` member takes, so a group a
            // stack creates carries the tags CloudFormation gives it beside the declared ones.
            Self::RedshiftClusterParameterGroup => {
                Box::pin(create_cluster_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                ))
                .await
            }
            // `AWS::Redshift::SnapshotCopyGrant` publishes `Tags` as the `{Key, Value}` list
            // `CreateSnapshotCopyGrant`'s own `Tags` member takes, so a grant a stack creates
            // carries the tags CloudFormation gives it beside the declared ones.
            Self::RedshiftSnapshotCopyGrant => {
                Box::pin(create_snapshot_copy_grant(ctx, region, resource, propagated)).await
            }
            // `AWS::Redshift::EventSubscription` publishes `Tags` as the `{Key, Value}` list
            // `CreateEventSubscription`'s own `Tags` member takes, so a subscription a stack
            // creates carries the tags CloudFormation gives it beside the declared ones.
            Self::RedshiftEventSubscription => {
                Box::pin(create_event_subscription(ctx, region, resource, propagated)).await
            }
            Self::RdsDbSubnetGroup => {
                create_db_subnet_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    RDS_DB_SUBNET_GROUP,
                    RDS_SDK_SERVICE,
                )
                .await
            }
            // `AWS::RDS::EventSubscription` publishes `Tags` as the `{Key, Value}` list
            // `CreateEventSubscription`'s own `Tags` member takes, so a subscription a stack
            // creates carries the tags CloudFormation gives it beside the declared ones.
            Self::RdsEventSubscription => {
                Box::pin(rds_event_subscriptions::create_subscription(
                    ctx,
                    region,
                    resource,
                    propagated,
                ))
                .await
            }
            // `AWS::RDS::OptionGroup` publishes `Tags` as the `{Key, Value}` list
            // `CreateOptionGroup`'s own `Tags` member takes, so a group a stack creates
            // carries the tags CloudFormation gives it beside the declared ones.
            Self::RdsOptionGroup => {
                Box::pin(rds_option_groups::create_option_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                ))
                .await
            }
            Self::RdsDbParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // Either RDS snapshot type publishes `Tags` as the `{Key, Value}` list its own
            // create takes, so a snapshot a stack takes carries the tags CloudFormation gives
            // it beside the declared ones.
            Self::RdsDbSnapshot => {
                Box::pin(rds_snapshots::create_snapshot(
                    ctx,
                    region,
                    resource,
                    propagated,
                    rds_snapshots::DB_SNAPSHOT_CALLS,
                ))
                .await
            }
            Self::RdsClusterSnapshot => {
                Box::pin(rds_snapshots::create_snapshot(
                    ctx,
                    region,
                    resource,
                    propagated,
                    rds_snapshots::DB_CLUSTER_SNAPSHOT_CALLS,
                ))
                .await
            }
            // `AWS::RDS::DBClusterEndpoint` publishes `Tags` as that same list, which
            // `CreateDBClusterEndpoint` takes as its own `Tags` member.
            Self::RdsDbClusterEndpoint => {
                Box::pin(rds_cluster_endpoints::create_cluster_endpoint(
                    ctx,
                    region,
                    resource,
                    propagated,
                ))
                .await
            }
            // `AWS::RDS::GlobalCluster` publishes `Tags` as that same list too, which
            // `CreateGlobalCluster` takes as its own `Tags` member.
            Self::RdsGlobalCluster => {
                Box::pin(rds_global_clusters::create_global_cluster(
                    ctx,
                    region,
                    resource,
                    propagated,
                    rds_global_clusters::RDS_GLOBAL_CLUSTER,
                    RDS_SDK_SERVICE,
                ))
                .await
            }
            // Every one of the Amazon Neptune relational types publishes `Tags` as the
            // `{Key, Value}` list its own create's `Tags` member takes, so a group, a subnet
            // group and a global database a stack creates each carry the tags CloudFormation
            // gives them beside the declared ones — the position the Amazon RDS types of the
            // same document above are in, whose arms these are.
            Self::NeptuneDbParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    neptune_relational::NEPTUNE_DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::NeptuneDbClusterParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    neptune_relational::NEPTUNE_DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::NeptuneDbSubnetGroup => {
                create_db_subnet_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    neptune_relational::NEPTUNE_DB_SUBNET_GROUP,
                    neptune_relational::NEPTUNE_SDK_SERVICE,
                )
                .await
            }
            Self::NeptuneGlobalCluster => {
                Box::pin(rds_global_clusters::create_global_cluster(
                    ctx,
                    region,
                    resource,
                    propagated,
                    neptune_relational::NEPTUNE_GLOBAL_CLUSTER,
                    neptune_relational::NEPTUNE_SDK_SERVICE,
                ))
                .await
            }
            Self::RdsDbProxy => create_db_proxy(ctx, region, resource, propagated).await,
            // `AWS::RDS::DBProxyTargetGroup` publishes no `Tags` property — Amazon RDS
            // holds tags on the PROXY, not on the registrations in its target group — so
            // the type is not taggable and no tags are propagated to one.
            Self::RdsDbProxyTargetGroup => {
                create_db_proxy_target_group(ctx, region, resource).await
            }
            // `AWS::Events::Rule` publishes `Description`, `EventBusName`, `EventPattern`,
            // `Name`, `RoleArn`, `ScheduleExpression`, `State` and `Targets` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::EventsRule => put_rule(ctx, region, resource).await,
            // `AWS::Events::EventBus` publishes `Tags` as the `[{Key, Value}]` list most
            // types do, which is the shape `CreateEventBus`'s own `Tags` member takes, so a
            // bus a stack creates carries the tags CloudFormation gives it beside the ones
            // the declaration asks for.
            Self::EventsEventBus => {
                event_buses::create_event_bus(ctx, region, resource, propagated).await
            }
            // `AWS::Events::EventBusPolicy` publishes no `Tags` property — a grant on a bus's
            // resource policy is not a resource EventBridge tags — so the type is not taggable
            // and no tags are propagated to one.
            Self::EventsEventBusPolicy => {
                event_buses::put_event_bus_policy(ctx, region, resource).await
            }
            // `AWS::Pipes::Pipe` publishes `Tags` as a MAP, which is the shape
            // `CreatePipe`'s own `Tags` member takes, so a pipe a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::PipesPipe => event_pipes::create_pipe(ctx, region, resource, propagated).await,
            // `AWS::ResourceGroups::Group` publishes `Tags` as the `[{Key, Value}]` list most
            // types do, which `CreateGroup` takes as its own `Tags` MAP, so a group a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::ResourceGroupsGroup => {
                create_resource_group(ctx, region, resource, propagated).await
            }
            // `AWS::ApplicationInsights::Application` publishes `Tags` as the `[{Key, Value}]`
            // list most types do, which is the shape `CreateApplication`'s own `Tags` member
            // takes, so an application a stack creates carries the tags CloudFormation gives it
            // beside the ones the declaration asks for.
            Self::ApplicationInsightsApplication => {
                create_application(ctx, region, resource, propagated).await
            }
            Self::SsmParameter => put_parameter(ctx, region, resource, propagated).await,
            // `AWS::SSM::Association` publishes no `Tags` property — Systems Manager tags
            // documents and managed nodes rather than the associations between them — so the
            // type is not taggable and no tags are propagated to one.
            Self::SsmAssociation => create_ssm_association(ctx, region, resource).await,
            // `AWS::SSM::MaintenanceWindow` takes its own `Tags` property as a member of the
            // create itself, and `AWS::SSM::MaintenanceWindowTarget` publishes no `Tags`
            // property at all — Systems Manager tags the WINDOW, not the targets registered
            // with it — so neither type has tags propagated to it here.
            Self::SsmMaintenanceWindow => {
                create_ssm_maintenance_window(ctx, region, resource).await
            }
            Self::SsmMaintenanceWindowTarget => {
                create_ssm_maintenance_window_target(ctx, region, resource).await
            }
            // `AWS::SSM::Document` publishes `Tags` as the `[{Key, Value}]` list most types
            // do, which is the shape `CreateDocument`'s own `Tags` member takes, so a
            // document a stack creates carries the tags CloudFormation gives it beside the
            // ones the declaration asks for.
            Self::SsmDocument => {
                create_ssm_document(ctx, region, resource, propagated).await
            }
            Self::StepFunctionsActivity => {
                create_activity(ctx, region, resource, propagated).await
            }
            Self::SchedulerScheduleGroup => {
                create_schedule_group(ctx, region, resource, propagated).await
            }
            // `AWS::Scheduler::Schedule` publishes no `Tags` property — EventBridge
            // Scheduler tags a schedule GROUP, not a schedule — so the type is not
            // taggable and no tags are propagated to it.
            Self::SchedulerSchedule => create_schedule(ctx, region, resource).await,
            // `AWS::Cognito::UserPoolGroup` publishes `Description`, `GroupName`,
            // `Precedence`, `RoleArn` and `UserPoolId` and no `Tags`: the type is not
            // taggable, so no tags are propagated to a group.
            Self::CognitoUserPoolGroup => create_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => create_acme_endpoint(ctx, region, resource, propagated).await,
            Self::AcmCertificate => request_certificate(ctx, region, resource, propagated).await,
            Self::AcmPcaCertificateAuthority => {
                create_certificate_authority(ctx, region, resource, propagated).await
            }
            // Neither `AWS::ACMPCA::Certificate` nor
            // `AWS::ACMPCA::CertificateAuthorityActivation` publishes a `Tags` property:
            // Amazon Web Services Private CA tags the certificate AUTHORITY, not a
            // certificate it issued or the import of one, so no tags are propagated to
            // either.
            Self::AcmPcaCertificate => issue_certificate(ctx, region, resource).await,
            Self::AcmPcaCertificateAuthorityActivation => {
                activate_certificate_authority(ctx, region, resource).await
            }
            // `AWS::ACMPCA::Permission` publishes no `Tags` property either: a grant on a CA
            // is no taggable resource, so no tags are propagated to one.
            Self::AcmPcaPermission => create_ca_permission(ctx, region, resource).await,
            Self::Route53ResolverEndpoint => {
                create_resolver_endpoint(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverRule => {
                create_resolver_rule(ctx, region, resource, propagated).await
            }
            // `AWS::Route53Resolver::ResolverRuleAssociation` publishes `Name`,
            // `ResolverRuleId` and `VPCId` and no `Tags`: the type is not taggable, so no
            // tags are propagated to an association.
            Self::Route53ResolverRuleAssociation => {
                associate_resolver_rule(ctx, region, resource).await
            }
            Self::Route53ResolverFirewallDomainList => {
                create_firewall_domain_list(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverFirewallRuleGroup => {
                create_firewall_rule_group(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverFirewallRuleGroupAssociation => {
                associate_firewall_rule_group(ctx, region, resource, propagated).await
            }
            Self::Route53HostedZone => {
                create_hosted_zone(ctx, region, resource, propagated).await
            }
            // `AWS::Route53::RecordSet` publishes no `Tags` property — Route 53 tags a
            // hosted zone, not a record set — so the type is not taggable and no tags are
            // propagated to one.
            Self::Route53RecordSet => create_record_set(ctx, region, resource).await,
            // The same for a GROUP of record sets, which publishes no `Tags` property
            // either: the group IS the record sets it declares, applied in one change batch.
            Self::Route53RecordSetGroup => create_record_set_group(ctx, region, resource).await,
            // `AWS::Route53::HealthCheck` publishes `HealthCheckTags`, which the health
            // check carries beside the ones CloudFormation gives it — applied through
            // `ChangeTagsForResource`, since `CreateHealthCheck` takes no tags.
            Self::Route53HealthCheck => {
                create_health_check(ctx, region, resource, propagated).await
            }
            // None of the three types below publishes a `Tags` property — Route 53 tags a
            // hosted zone and a health check, not a CIDR collection, a delegation set or a
            // traffic policy — so no tags are propagated to one.
            Self::Route53CidrCollection => {
                create_cidr_collection(ctx, region, resource, propagated).await
            }
            Self::Route53DelegationSet => {
                create_delegation_set(ctx, region, resource, propagated).await
            }
            Self::Route53TrafficPolicy => create_traffic_policy(ctx, region, resource).await,
            // `AWS::ElasticBeanstalk::Application` publishes `ApplicationName`,
            // `Description` and `ResourceLifecycleConfig` and no `Tags`: the type is not
            // taggable, so no tags are propagated to it. The same goes for
            // `AWS::ElasticBeanstalk::ApplicationVersion`, whose properties are the
            // application it belongs to, its description and its source bundle.
            Self::ElasticBeanstalkApplication => create_eb_application(ctx, region, resource).await,
            Self::ElasticBeanstalkApplicationVersion => {
                create_eb_application_version(ctx, region, resource).await
            }
            // `AWS::ElasticBeanstalk::ConfigurationTemplate` publishes no `Tags` property
            // either: its properties are the application it belongs to, the platform version
            // its settings are for, the settings themselves and the configuration they are
            // copied from.
            Self::ElasticBeanstalkConfigurationTemplate => {
                create_eb_configuration_template(ctx, region, resource).await
            }
            Self::ElasticBeanstalkEnvironment => {
                create_eb_environment(ctx, region, resource, propagated).await
            }
            Self::TransferServer => {
                create_transfer_server(ctx, region, resource, propagated).await
            }
            Self::CodeDeployApplication => {
                create_codedeploy_application(ctx, region, resource, propagated).await
            }
            Self::CodeDeployDeploymentGroup => {
                create_codedeploy_deployment_group(ctx, region, resource, propagated).await
            }
            // `AWS::CodeDeploy::DeploymentConfig` publishes `ComputePlatform`,
            // `DeploymentConfigName`, `MinimumHealthyHosts`, `TrafficRoutingConfig` and
            // `ZonalConfig` and no `Tags`: the type is not taggable, so no tags are
            // propagated to a deployment configuration.
            Self::CodeDeployDeploymentConfig => {
                create_codedeploy_deployment_config(ctx, region, resource).await
            }
            Self::BatchComputeEnvironment => {
                create_compute_environment(ctx, region, resource, propagated).await
            }
            Self::BatchJobQueue => create_job_queue(ctx, region, resource, propagated).await,
            // A job-definition revision IS taggable: its `Tags` property is the tag map
            // `RegisterJobDefinition` takes, so the revision carries the tags CloudFormation
            // gives the resource beside its own.
            Self::BatchJobDefinition => {
                create_job_definition(ctx, region, resource, propagated).await
            }
            Self::BatchServiceEnvironment => {
                create_service_environment(ctx, region, resource, propagated).await
            }
            // Both SHARING-plane types are taggable and their creates take the tag set as a
            // member of their own, so the tags CloudFormation gives the resource reach AWS
            // Batch on the call that makes it.
            Self::BatchSchedulingPolicy => {
                create_batch_scheduling_policy(ctx, region, resource, propagated).await
            }
            Self::BatchQuotaShare => {
                create_batch_quota_share(ctx, region, resource, propagated).await
            }
            Self::EksAccessEntry => create_access_entry(ctx, region, resource, propagated).await,
            Self::EksCluster => create_eks_cluster(ctx, region, resource, propagated).await,
            // An add-on and a node group are taggable types whose create takes the tag set
            // as its own member, so the tags CloudFormation gives the resource reach Amazon
            // EKS on the call that makes it.
            Self::EksAddon => create_eks_addon(ctx, region, resource, propagated).await,
            Self::EksNodegroup => create_eks_nodegroup(ctx, region, resource, propagated).await,
            Self::EksPodIdentityAssociation => {
                create_pod_identity_association(ctx, region, resource, propagated).await
            }
            Self::ApiGatewayDomainName => {
                create_apigateway_domain_name(ctx, region, resource, propagated).await
            }
            // `AWS::ApiGateway::BasePathMapping` publishes `BasePath`, `DomainName`,
            // `DomainNameId`, `RestApiId` and `Stage` and no `Tags`: the type is not
            // taggable, so no tags are propagated to a mapping.
            Self::ApiGatewayBasePathMapping => {
                create_apigateway_base_path_mapping(ctx, region, resource).await
            }
            Self::ApiGatewayV2Api => {
                create_apigatewayv2_api(ctx, region, resource, propagated).await
            }
            Self::ApiGatewayV2DomainName => {
                create_apigatewayv2_domain_name(ctx, region, resource, propagated).await
            }
            // `AWS::ApiGatewayV2::ApiMapping` publishes `ApiId`, `ApiMappingKey`,
            // `DomainName` and `Stage` and no `Tags`: the type is not taggable, so no tags
            // are propagated to a mapping.
            Self::ApiGatewayV2ApiMapping => {
                create_apigatewayv2_api_mapping(ctx, region, resource).await
            }
            // The five types whose resources live INSIDE an api, each created in the
            // collection its api holds ([`ApiGatewayV2Child`]). Only the stage's own
            // property set publishes `Tags`, and the table is what says so, so the tags
            // CloudFormation gives the resource reach the one create that takes them.
            Self::ApiGatewayV2Integration => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_INTEGRATION_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Route => {
                create_apigatewayv2_child(ctx, region, resource, propagated, &APIGATEWAYV2_ROUTE_CHILD)
                    .await
            }
            Self::ApiGatewayV2Stage => {
                create_apigatewayv2_child(ctx, region, resource, propagated, &APIGATEWAYV2_STAGE_CHILD)
                    .await
            }
            Self::ApiGatewayV2Deployment => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_DEPLOYMENT_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Authorizer => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_AUTHORIZER_CHILD,
                )
                .await
            }
            // The three that say what a route REPLIES: a response of one integration, a
            // response of one route, and a model of the api. None of the three publishes
            // `Tags`, so no tags are propagated to one.
            Self::ApiGatewayV2IntegrationResponse => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_INTEGRATION_RESPONSE_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2RouteResponse => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_ROUTE_RESPONSE_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Model => {
                create_apigatewayv2_child(ctx, region, resource, propagated, &APIGATEWAYV2_MODEL_CHILD)
                    .await
            }
            // `AWS::ApiGatewayV2::RoutingRule` publishes `Actions`, `Conditions`,
            // `DomainNameArn` and `Priority` and no `Tags`: a routing rule is no taggable
            // resource, so no tags are propagated to one.
            Self::ApiGatewayV2RoutingRule => {
                create_apigatewayv2_routing_rule(ctx, region, resource).await
            }
            Self::CodeCommitRepository => {
                create_codecommit_repository(ctx, region, resource, propagated).await
            }
            Self::CodeArtifactDomain => {
                create_codeartifact_domain(ctx, region, resource, propagated).await
            }
            Self::CodeArtifactRepository => {
                create_codeartifact_repository(ctx, region, resource, propagated).await
            }
            // `AWS::CodeArtifact::PackageGroup` publishes `Tags`, and `CreatePackageGroup`
            // takes a tag list of its own, so a group a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::CodeArtifactPackageGroup => {
                create_codeartifact_package_group(ctx, region, resource, propagated).await
            }
            Self::CodeBuildReportGroup => {
                create_codebuild_report_group(ctx, region, resource, propagated).await
            }
            // `AWS::CodeBuild::Project` publishes `Tags`, and `CreateProject` takes a tag
            // list of its own, so a project a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::CodeBuildProject => {
                create_codebuild_project(ctx, region, resource, propagated).await
            }
            // `AWS::CodeBuild::Fleet` publishes `Tags`, and `CreateFleet` takes a tag list
            // of its own, so a fleet a stack creates carries the tags CloudFormation gives
            // it beside the ones the declaration asks for.
            Self::CodeBuildFleet => {
                create_codebuild_fleet(ctx, region, resource, propagated).await
            }
            // `AWS::SES::ConfigurationSet` publishes `Name`, `DeliveryOptions`,
            // `ReputationOptions`, `SendingOptions`, `SuppressionOptions`,
            // `TrackingOptions` and `VdmOptions` and no `Tags`: the type is not taggable,
            // so no tags are propagated to a configuration set.
            Self::SesConfigurationSet => {
                create_ses_configuration_set(ctx, region, resource).await
            }
            // `AWS::SES::EmailIdentity` publishes `EmailIdentity`,
            // `ConfigurationSetAttributes`, `DkimAttributes`, `DkimSigningAttributes`,
            // `FeedbackAttributes` and `MailFromAttributes` and no `Tags`: the type is not
            // taggable, so no tags are propagated to an email identity.
            Self::SesEmailIdentity => create_ses_email_identity(ctx, region, resource).await,
            // `AWS::SES::ReceiptRuleSet` publishes `RuleSetName` alone and
            // `AWS::SES::ReceiptRule` publishes `After`, `Rule` and `RuleSetName`: neither
            // type publishes a `Tags` property — Amazon SES holds no tags on either — so no
            // tags are propagated to one.
            Self::SesReceiptRuleSet => {
                create_ses_receipt_rule_set(ctx, region, resource).await
            }
            Self::SesReceiptRule => create_ses_receipt_rule(ctx, region, resource).await,
            Self::BackupVault => {
                create_backup_vault(ctx, region, resource, propagated).await
            }
            Self::BackupPlan => create_backup_plan(ctx, region, resource, propagated).await,
            // `AWS::Backup::BackupSelection` publishes `BackupPlanId` and
            // `BackupSelection` and no tags: AWS Backup tags a plan and a vault, not a
            // selection, so no tags are propagated to one.
            Self::BackupSelection => create_backup_selection(ctx, region, resource).await,
            // `AWS::Bedrock::Guardrail` publishes `Tags`, and `CreateGuardrail` takes the
            // `TagList` of its own, so a guardrail a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::BedrockGuardrail => {
                create_bedrock_guardrail(ctx, region, resource, propagated).await
            }
            // `AWS::Bedrock::KnowledgeBase` publishes `Tags` as a MAP, which is the shape
            // `CreateKnowledgeBase`'s own `tags` member takes, so a knowledge base a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BedrockKnowledgeBase => {
                create_bedrock_knowledge_base(ctx, region, resource, propagated).await
            }
            // `AWS::Bedrock::DataSource` publishes no `Tags` property and `CreateDataSource`
            // takes no `tags` member, so no tags are propagated to a data source.
            Self::BedrockDataSource => create_bedrock_data_source(ctx, region, resource).await,
            // `AWS::Bedrock::Flow` publishes `Tags` as a MAP, which is the shape
            // `CreateFlow`'s own `tags` member takes, so a flow a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::BedrockFlow => create_bedrock_flow(ctx, region, resource, propagated).await,
            // `AWS::Bedrock::Session` publishes `Tags` as a MAP, which is the shape
            // `CreateSession`'s own `tags` member takes, so a session a stack creates carries
            // the tags CloudFormation gives it beside the ones the declaration asks for.
            Self::BedrockSession => {
                create_bedrock_session(ctx, region, resource, propagated).await
            }
            // `AWS::Bedrock::ApplicationInferenceProfile` publishes `Tags` as a LIST, which is
            // the shape `CreateInferenceProfile`'s own `tags` member takes, so a profile a
            // stack creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BedrockApplicationInferenceProfile => {
                create_bedrock_inference_profile(ctx, region, resource, propagated).await
            }
            // `AWS::BedrockAgentCore::WorkloadIdentity` publishes `Tags`, and
            // `CreateWorkloadIdentity` takes a tag MAP of its own, so an identity a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BedrockAgentCoreWorkloadIdentity => {
                create_workload_identity(ctx, region, resource, propagated).await
            }
            // Every other `AWS::BedrockAgentCore::*` type publishes `Tags` and its create
            // takes a tag MAP of its own, so each resource a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for. A gateway
            // TARGET is the exception: `CreateGatewayTarget` takes no `tags` member and the
            // type publishes no `Tags` property, so no tag is propagated to one.
            Self::BedrockAgentCoreRuntime => {
                create_agentcore_runtime(ctx, region, resource, propagated).await
            }
            Self::BedrockAgentCoreMemory => {
                create_agentcore_memory(ctx, region, resource, propagated).await
            }
            Self::BedrockAgentCoreGateway => {
                create_agentcore_gateway(ctx, region, resource, propagated).await
            }
            Self::BedrockAgentCoreGatewayTarget => {
                create_agentcore_gateway_target(ctx, region, resource).await
            }
            Self::BedrockAgentCoreSandbox(sandbox) => {
                create_agentcore_sandbox(ctx, region, sandbox, resource, propagated).await
            }
            // `AWS::Braket::SpendingLimit` publishes `Tags` as a MAP, which is the shape
            // `CreateSpendingLimit`'s own `tags` member takes, so a spending limit a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BraketSpendingLimit => {
                create_braket_spending_limit(ctx, region, resource, propagated).await
            }
            // Every `AWS::SMSVOICE::*` type but the resource policy publishes `Tags` as the
            // `[{Key, Value}]` list its create's own `Tags` member takes, so each resource a
            // stack creates carries the tags CloudFormation gives it beside the ones its
            // declaration asks for; a resource policy is a document on another resource and
            // takes no tags at all. Boxed, as the arms that reach a service through another
            // of this emulator's own are, so this arm's future does not widen the one every
            // create is polled through.
            Self::SmsVoiceResource(resource_kind) => {
                Box::pin(create_sms_voice_resource(
                    ctx,
                    region,
                    resource_kind,
                    resource,
                    propagated,
                ))
                .await
            }
            // `AWS::AIOps::InvestigationGroup` publishes `Tags` as the usual `[{Key, Value}]`
            // list while `CreateInvestigationGroup` takes a tag MAP, so the tags
            // CloudFormation gives the group travel beside the declaration's own in that
            // shape, and the set is reconciled afterwards through that service's own
            // `TagResource`/`UntagResource` ([`retag_investigation_group`]).
            Self::AiOpsInvestigationGroup => {
                create_investigation_group(ctx, region, resource, propagated).await
            }
            // Both `AWS::GameLift::*` types publish `Tags` as the `[{Key, Value}]` list
            // `CreateBuild`'s and `CreateScript`'s own `Tags` member takes, so a build and a
            // script a stack creates each carry the tags CloudFormation gives them beside the
            // ones their declaration asks for. Boxed, as the arms that reach a service through
            // another of this emulator's own are, so this arm's future does not widen the one
            // every create is polled through.
            Self::GameLiftBuild => {
                Box::pin(create_gamelift_build(ctx, region, resource, propagated)).await
            }
            Self::GameLiftScript => {
                Box::pin(create_gamelift_script(ctx, region, resource, propagated)).await
            }
            // `AWS::HealthLake::FHIRDatastore` publishes `Tags` as the `[{Key, Value}]` list
            // `CreateFHIRDatastore`'s own `Tags` member takes, and
            // `AWS::HealthLake::DataTransformationProfile` publishes it as the map
            // `CreateDataTransformationProfile`'s does — so each resource a stack creates
            // carries the tags CloudFormation gives it beside the ones its declaration asks
            // for.
            Self::HealthLakeFhirDatastore => {
                create_fhir_datastore(ctx, region, resource, propagated).await
            }
            Self::HealthLakeDataTransformationProfile => {
                create_data_transformation_profile(ctx, region, resource, propagated).await
            }
            // `AWS::SSMQuickSetup::ConfigurationManager` publishes `Tags` as a MAP, which is
            // the shape `CreateConfigurationManager`'s own `Tags` member takes, so a manager a
            // stack creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::SsmQuickSetupConfigurationManager => {
                create_configuration_manager(ctx, region, resource, propagated).await
            }
            // Both `AWS::DirectConnect::*` types publish `Tags`, and each of their creates
            // takes a `TagList` of its own, so a gateway and a resiliency group a stack
            // creates each carry the tags CloudFormation gives them beside the ones their
            // declaration asks for.
            Self::DirectConnectGateway => {
                create_direct_connect_gateway(ctx, region, resource, propagated).await
            }
            Self::DirectConnectResiliencyGroup => {
                create_resiliency_group(ctx, region, resource, propagated).await
            }
            // `AWS::IoTCoreDeviceAdvisor::SuiteDefinition` publishes `Tags` as a list of
            // pairs, which `CreateSuiteDefinition` takes as the `TagMap` of its own `tags`
            // member, so a suite a stack creates carries the tags CloudFormation gives it
            // beside the ones the declaration asks for.
            Self::IotCoreDeviceAdvisorSuiteDefinition => {
                create_suite_definition(ctx, region, resource, propagated).await
            }
            // Both `AWS::BCM::*` types publish `Tags`, and each of their creates takes a
            // `resourceTags` list of its own, so a dashboard and a scheduled report a stack
            // creates each carry the tags CloudFormation gives them beside the ones their
            // declaration asks for.
            Self::BcmDashboard => create_bcm_dashboard(ctx, region, resource, propagated).await,
            Self::BcmScheduledReport => {
                create_bcm_scheduled_report(ctx, region, resource, propagated).await
            }
            // `AWS::ApplicationSignals::ServiceLevelObjective` publishes `Tags`, and
            // `CreateServiceLevelObjective` takes the `TagList` of its own, so an objective a
            // stack creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for. The same holds for an
            // `AWS::ApplicationSignals::InstrumentationConfig`, whose create takes a `TagList`
            // too. A GROUPING CONFIGURATION publishes no `Tags` property and its put takes no
            // tag member at all — it is the account's own configuration rather than a named
            // resource — so no tags are propagated to one.
            Self::ApplicationSignalsServiceLevelObjective => {
                create_slo(ctx, region, resource, propagated).await
            }
            Self::ApplicationSignalsGroupingConfiguration => {
                put_grouping_configuration(ctx, region, resource, &resource.properties).await
            }
            Self::ApplicationSignalsInstrumentationConfig => {
                create_instrumentation_config(ctx, region, resource, propagated).await
            }
            // `AWS::CleanRoomsML::TrainingDataset` publishes `Tags`, which
            // `CreateTrainingDataset` takes as its own tag MAP, so a dataset a stack creates
            // carries the tags CloudFormation gives it beside the ones the declaration asks
            // for.
            Self::CleanRoomsMlTrainingDataset => {
                create_training_dataset(ctx, region, resource, propagated).await
            }
            // `AWS::CloudHSM::Cluster` publishes `Tags`, which `CreateCluster` takes as its own
            // `TagList` of pairs, so a cluster a stack creates carries the tags CloudFormation
            // gives it beside the ones the declaration asks for.
            Self::CloudHsmCluster => {
                create_cloudhsm_cluster(ctx, region, resource, propagated).await
            }
            // Each of the three `AWS::AmplifyUIBuilder::*` types publishes `Tags` as a MAP,
            // which is the shape its create's own `tags` member takes, so an entity a stack
            // creates carries the tags CloudFormation gives it beside the ones the declaration
            // asks for.
            Self::AmplifyUiBuilderComponent => {
                create_ui_entity(ctx, region, UiEntity::Component, resource, propagated).await
            }
            Self::AmplifyUiBuilderForm => {
                create_ui_entity(ctx, region, UiEntity::Form, resource, propagated).await
            }
            Self::AmplifyUiBuilderTheme => {
                create_ui_entity(ctx, region, UiEntity::Theme, resource, propagated).await
            }
            // `AWS::Amplify::App` publishes `Tags` as a LIST of pairs, which `CreateApp` takes
            // as the `TagMap` of its own `tags` member, so an app a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::AmplifyApp => create_amplify_app(ctx, region, resource, propagated).await,
            // `AWS::Amplify::Branch` publishes `Tags` as a LIST of pairs too, which
            // `CreateBranch` takes as the `TagMap` of its own `tags` member, so a branch a stack
            // creates carries the tags CloudFormation gives it beside the ones the declaration
            // asks for.
            Self::AmplifyBranch => {
                create_amplify_branch(ctx, region, resource, propagated).await
            }
            Self::XraySamplingRule => {
                create_sampling_rule(ctx, region, resource, propagated).await
            }
            // `AWS::XRay::Group` publishes `Tags` as a LIST of pairs, which is the shape
            // `CreateGroup`'s own `Tags` member takes, so a group a stack creates carries
            // the tags CloudFormation gives it beside the ones the declaration asks for.
            Self::XrayGroup => create_xray_group(ctx, region, resource, propagated).await,
            // `AWS::MWAA::Environment` publishes `Tags` as a MAP, which is the shape
            // `CreateEnvironment`'s own `Tags` member takes, so an environment a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::MwaaEnvironment => {
                create_mwaa_environment(ctx, region, resource, propagated).await
            }
            // Neither `AWS::DirectoryService::*` type publishes a `Tags` property — Directory
            // Service holds a directory's tags through its own `AddTagsToResource` and takes
            // none on either create — so no tags are propagated to a directory.
            Self::DirectoryServiceDirectory(kind) => {
                create_ds_directory(ctx, region, kind, resource).await
            }

            // None of the three `AWS::Config::*` types publishes a `Tags` property — AWS
            // Config holds tags on a recorder and on a rule through its own tagging call
            // and takes none on either put — so no tags are propagated to one.
            Self::ConfigConfigurationRecorder => {
                put_configuration_recorder(ctx, region, resource).await
            }
            Self::ConfigDeliveryChannel => put_delivery_channel(ctx, region, resource).await,
            Self::ConfigConfigRule => put_config_rule(ctx, region, resource).await,
            // Each group's create takes a `Tags` list of its own, so a parameter group and
            // a subnet group a stack creates each carry the tags CloudFormation gives them
            // beside the ones their declaration asks for.
            Self::MemoryDbGroup(group) => {
                create_memorydb_group(ctx, region, group, resource, propagated).await
            }
            // `AWS::CustomerProfiles::Domain` publishes `Tags`, and `CreateDomain` takes a
            // `TagMap` of its own, so a domain a stack creates carries the tags CloudFormation
            // gives it beside the ones the declaration asks for.
            Self::CustomerProfilesDomain => {
                create_customer_profiles_domain(ctx, region, resource, propagated).await
            }
            // `AWS::Rbin::Rule` publishes `Tags`, and `CreateRule` takes a `TagList` of its
            // own, so a retention rule a stack creates carries the tags CloudFormation gives
            // it beside the ones the declaration asks for.
            Self::RbinRule => create_rbin_rule(ctx, region, resource, propagated).await,
            // Both `AWS::KinesisVideo::*` types publish `Tags`, and each create takes a tag set
            // of its own — a stream's as the `ResourceTags` map, a channel's as the
            // `TagOnCreateList` — so a stream and a channel a stack creates each carry the tags
            // CloudFormation gives them beside the ones their declaration asks for.
            // BOXED, for the reason the other heavy arms of this match are: the call's future is
            // the owning service's whole handler, and holding one of those inline in this
            // match's state machine is what takes a deployment past the thread's stack.
            Self::KinesisVideoResource(kind) => {
                Box::pin(create_kinesis_video_resource(
                    ctx, region, kind, resource, propagated,
                ))
                .await
            }
            // `AWS::DLM::LifecyclePolicy` publishes `Tags`, and `CreateLifecyclePolicy` takes a
            // `TagMap` of its own, so a lifecycle policy a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::DlmLifecyclePolicy => {
                create_dlm_lifecycle_policy(ctx, region, resource, propagated).await
            }
            // Every `AWS::NetworkSecurityManager::*` type publishes `Tags`, and each of their
            // creates takes a `tags` MAP of its own, so an entity a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::NetworkSecurityManagerEntity(entity) => {
                create_entity(ctx, region, entity, resource, propagated).await
            }
            // Both `AWS::NeptuneGraph::*` types publish `Tags`, and each of their creates
            // takes a `TagMap` of its own, so a graph and a snapshot a stack creates each
            // carry the tags CloudFormation gives them beside the ones their declaration asks
            // for.
            Self::NeptuneGraphGraph => {
                create_neptune_graph(ctx, region, resource, propagated).await
            }
            Self::NeptuneGraphGraphSnapshot => {
                create_graph_snapshot(ctx, region, resource, propagated).await
            }
            // All three `AWS::RedshiftServerless::*` types publish `Tags`, and each of their
            // creates takes a `TagList` of its own, so a namespace, a workgroup and a snapshot
            // a stack creates each carry the tags CloudFormation gives them beside the ones
            // their declaration asks for.
            Self::RedshiftServerlessNamespace => {
                create_redshift_serverless_namespace(ctx, region, resource, propagated).await
            }
            Self::RedshiftServerlessWorkgroup => {
                create_redshift_serverless_workgroup(ctx, region, resource, propagated).await
            }
            Self::RedshiftServerlessSnapshot => {
                create_redshift_serverless_snapshot(ctx, region, resource, propagated).await
            }
            // `AWS::BackupGateway::Hypervisor` publishes `Tags`, and
            // `ImportHypervisorConfiguration` takes a `Tags` list of its own, so a hypervisor a
            // stack imports carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BackupGatewayHypervisor => {
                create_backup_gateway_hypervisor(ctx, region, resource, propagated).await
            }
            // `AWS::AccessAnalyzer::Analyzer` publishes `Tags`, which `CreateAnalyzer` takes as
            // the `TagsMap` of its own, so an analyzer a stack creates carries the tags
            // CloudFormation gives it beside the ones its declaration asks for. An
            // `AWS::AccessAnalyzer::ArchiveRule` publishes none at all — `CreateArchiveRule`
            // takes no tags and that service holds none on a rule — so none are propagated to
            // one, which that arm decides from the kind it carries.
            Self::AccessAnalyzerResource(kind) => {
                create_access_analyzer_resource(ctx, region, kind, resource, propagated).await
            }
            // `AWS::Rekognition::Collection` publishes `Tags`, and `CreateCollection` takes a
            // `TagMap` of its own, so a collection a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::RekognitionCollection => {
                create_rekognition_collection(ctx, region, resource, propagated).await
            }
            // `AWS::Rekognition::Project` publishes no `Tags` property at all — Rekognition
            // holds a project's tags through its own `TagResource` and the type declares only
            // the project's name — so no tags are propagated to one.
            Self::RekognitionProject => create_rekognition_project(ctx, region, resource).await,
            // `AWS::AppFlow::ConnectorProfile` publishes no `Tags` property either — Amazon
            // AppFlow holds a connector profile's tags through its own `TagResource` and
            // `CreateConnectorProfile` takes none — so no tags are propagated to one.
            // BOXED, for the reason the other heavy arms of this match are: the call's future is
            // the owning service's whole handler — which here makes a nested call of its own, to
            // the service that holds the profile's credentials — and holding one of those inline
            // in this match's state machine is what takes a deployment past the thread's stack.
            Self::AppFlowConnectorProfile => {
                Box::pin(create_appflow_connector_profile(ctx, region, resource)).await
            }
            // `AWS::Rekognition::Dataset` publishes `Tags`, which `CreateDataset` takes as a
            // `TagMap` of its own, so a dataset a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::RekognitionDataset => {
                create_rekognition_dataset(ctx, region, resource, propagated).await
            }
            // Both Amazon VPC Lattice types publish `Tags`, which each create takes as a
            // `TagMap` of its own, so a network and a subscription a stack creates carry the
            // tags CloudFormation gives them beside the ones the declaration asks for.
            Self::VpcLatticeServiceNetwork => {
                create_vpc_lattice_service_network(ctx, region, resource, propagated).await
            }
            Self::VpcLatticeAccessLogSubscription => {
                create_vpc_lattice_access_log_subscription(ctx, region, resource, propagated)
                    .await
            }
            // Every one of the four RESOURCE-ACCESS types publishes `Tags` too, and every one
            // of their creates takes a `tags` MAP of its own, so a resource a stack creates
            // there carries the tags CloudFormation gives it beside the ones its declaration
            // asks for.
            Self::VpcLatticeResource(kind) => {
                create_vpc_lattice_resource(ctx, region, kind, resource, propagated).await
            }
            // Each Amazon Bedrock Data Automation type publishes `Tags` as the `[{Key, Value}]`
            // list its create's own `tags` member takes, so a blueprint, a project or a library
            // a stack creates carries the tags CloudFormation gives it beside the ones its
            // declaration asks for.
            Self::BedrockDataAutomation(kind) => {
                create_bedrock_data_automation(ctx, region, kind, resource, propagated).await
            }
            // `AWS::Lex::Bot` publishes `BotTags` and `TestBotAliasTags` and `AWS::Lex::BotAlias`
            // publishes `BotAliasTags`, each the tag MAP that resource's own create takes, so a
            // bot and an alias a stack creates carry the tags CloudFormation gives them beside
            // the ones their declarations ask for. A bot VERSION is no taggable resource:
            // `CreateBotVersion` takes no tags and the type publishes no tag property.
            Self::LexBot(kind) => {
                create_lex_resource(ctx, region, kind, resource, propagated).await
            }
        }
    }

    /// Those of the tags CloudFormation gives a resource that the owning service's own API
    /// will take.
    ///
    /// Every create here is made through the owning service's PUBLIC API — the same path a
    /// client's own call takes, which is what makes a stack's resource indistinguishable
    /// from one a client created. Some of those APIs refuse a tag key in AWS's reserved
    /// `aws:` namespace, which is AWS's rule for a CALLER's request ("don't use the `aws:`
    /// prefix ... it is reserved for AWS use"): IAM's `CreateRole`, CodePipeline's
    /// `CreatePipeline` and EventBridge Scheduler's `CreateScheduleGroup` all do, in their
    /// own crates' tag validation. AWS sets the
    /// `aws:cloudformation:*` tags from inside itself, through no public API: here that
    /// inside path is the in-process call this provisioning request IS, which the framework
    /// stamps and a client's request cannot be — so a type whose owning crate reads that
    /// stamp takes the three (see the paragraph below), and for a type whose crate does not
    /// they are left off rather than sent to be refused: a stack must not fail over tags
    /// CloudFormation adds of its own accord. The stack's own tags, which are ordinary keys,
    /// are applied either
    /// way. ACM is the same case: its `CreateAcmeEndpoint` refuses a reserved key on either
    /// half of a pair, and so do Amazon ECS's `CreateCluster`, Amazon MSK's two cluster
    /// creates, Amazon SQS's `CreateQueue` — which refuses it in its own crate's
    /// `state::tag_refusal`, the one check both `CreateQueue`'s `tags` map and `TagQueue`
    /// run — and Amazon Data Firehose's `CreateDeliveryStream`, whose `TagKey` shape is
    /// modelled with the reserved prefix excluded, the same check its `TagDeliveryStream`
    /// makes. CodeCommit's `CreateRepository` is in the same position: a tag key in that
    /// namespace is its `InvalidSystemTagUsageException`, and Amazon ECR Public's
    /// `CreateRepository` is the same case: a key in that namespace is the
    /// `InvalidTagParameterException` its own crate's tag validation answers, the one check
    /// its `TagResource` runs too. AWS WAF's four WAFv2 entity
    /// creates are the same case again: each takes a `TagList` and refuses a key in the
    /// reserved namespace in its own crate's tag validation, the one check `TagResource`
    /// runs too.
    ///
    /// A type whose owning service ADMITS the three on that in-process path — Amazon S3,
    /// whose crate refuses a reserved key to an API caller and takes one from a provisioning
    /// call, Amazon S3
    /// Vectors, whose crate does the same, and Amazon EventBridge Pipes, whose crate reads
    /// the same signal —
    /// is NOT in the set below: the three travel with the create, and that service's own
    /// read of the resource's tags on the handle the stack publishes answers with them. A
    /// type stays in the set only until its own crate reads that path too.
    pub(super) fn applicable_tags(self, propagated: &[(String, String)]) -> Vec<(String, String)> {
        let refuses_reserved_keys = matches!(
            self,
            Self::IamRole
                | Self::IamUser
                | Self::CodePipelinePipeline
                // CodePipeline refuses a tag key in the reserved namespace on
                // `CreateCustomActionType` too, in the same tag validation its
                // `CreatePipeline` runs.
                | Self::CodePipelineCustomActionType
                // And on `PutWebhook`'s own `tags` list, in that same validation, so a
                // webhook carries the stack's own tags without the three.
                | Self::CodePipelineWebhook
                | Self::EcsCluster
                // Amazon ECS refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateService`'s own `tags` list as much as `CreateCluster`'s
                // and `TagResource`, in the one check that crate's tag validation makes ("Tag
                // key must not start with aws:") — so a service carries the stack's own tags
                // without the three.
                | Self::EcsService
                // Amazon ECS refuses one on `CreateCapacityProvider`'s `tags` list too, in
                // that same check, so a capacity provider carries the stack's own tags
                // without the three.
                | Self::EcsCapacityProvider
                // And on `CreateTaskSet`'s `tags` list too, in that same check, so a task set
                // carries the stack's own tags without the three.
                | Self::EcsTaskSet
                | Self::EcrPublicRepository
                // Amazon ECR refuses a tag key in the reserved namespace on every call
                // that takes tags — `CreateRepository`'s `tags` and `TagResource` alike —
                // so a repository carries the stack's own tags without the three.
                | Self::EcrRepository
                | Self::FirehoseDeliveryStream
                // Amazon Kinesis Data Streams refuses a tag key in the reserved namespace
                // on every call that takes tags — `CreateStream`'s `Tags`, `TagResource` and
                // `AddTagsToStream` alike — so a stream carries the stack's own tags
                // without the three.
                | Self::KinesisStream
                | Self::SchedulerScheduleGroup
                | Self::AcmAcmeEndpoint
                // ACM refuses a tag key in the reserved namespace on every call that takes
                // tags, in the one check its own crate's tag validation makes, so a
                // certificate's are left off the same way an ACME endpoint's are.
                | Self::AcmCertificate
                // Amazon Web Services Private CA refuses a tag key in the reserved
                // namespace on `CreateCertificateAuthority` and on
                // `TagCertificateAuthority` alike, in the one check its own crate's tag
                // validation makes.
                | Self::AcmPcaCertificateAuthority
                | Self::MskCluster
                | Self::MskServerlessCluster
                | Self::SqsQueue
                | Self::CodeCommitRepository
                // KMS refuses a tag key in the reserved namespace on `CreateKey` and on
                // `TagResource` alike, in its own crate's tag validation.
                | Self::KmsKey
                | Self::Wafv2WebAcl
                | Self::Wafv2RuleGroup
                | Self::Wafv2IpSet
                | Self::Wafv2RegexPatternSet
                // Amazon API Gateway refuses a tag key in the reserved namespace on every
                // call that takes tags — the `tags` member of each create and `TagResource`
                // alike, in the one check its own crate's tag validation makes ("The tag
                // key … must not start with aws:") — so a custom domain name carries the
                // stack's own tags without the three.
                | Self::ApiGatewayDomainName
                // AWS X-Ray refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateSamplingRule`'s `Tags` and `TagResource` alike, in
                // the one check its own crate's tag validation makes ("Don't use `aws:` as
                // a prefix for keys; it's reserved for Amazon Web Services use") — so a
                // sampling rule carries the stack's own tags without the three.
                | Self::XraySamplingRule
                // The same check guards `CreateGroup`'s own `Tags` member, so a group is in
                // exactly that position.
                | Self::XrayGroup
                // Amazon MWAA refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateEnvironment`'s `Tags` map and `TagResource` alike, in
                // the one check its own crate's tag validation makes — so an environment
                // carries the stack's own tags without the three.
                | Self::MwaaEnvironment
                // Amazon ElastiCache refuses a tag key in the reserved namespace on every
                // call that takes tags — `CreateCacheCluster`'s `Tags` member and
                // `AddTagsToResource` alike, in the one check its own crate's tag validation
                // makes — so a cache cluster carries the stack's own tags without the three.
                | Self::ElastiCacheCacheCluster
                // The same check guards `CreateServerlessCache`'s own `Tags` member, so a
                // serverless cache is in exactly that position.
                | Self::ElastiCacheServerlessCache
                // The same check guards `CreateReplicationGroup`'s and
                // `CreateCacheSubnetGroup`'s own `Tags` members, so a replication group and a
                // cache subnet group are in exactly that position.
                | Self::ElastiCacheReplicationGroup
                | Self::ElastiCacheSubnetGroup
                // Resource Groups refuses a tag key in the reserved namespace on every call
                // that takes tags — `CreateGroup`'s `Tags` map and `Tag` alike, in the one
                // check its own crate's tag validation makes — so a group carries the stack's
                // own tags without the three.
                | Self::ResourceGroupsGroup
                // Amazon EventBridge refuses a tag key in the reserved namespace on every
                // call that takes tags — `CreateEventBus`'s `Tags` list and `TagResource`
                // alike, in the one check its own crate's tag merge makes — so a bus carries
                // the stack's own tags without the three.
                | Self::EventsEventBus
                // Amazon Redshift refuses a tag key in the reserved namespace on
                // `CreateSnapshotCopyGrant`'s own `Tags` member and on `CreateTags` alike, in
                // the one check its own crate's tag validation makes — so a grant carries the
                // stack's own tags without the three.
                | Self::RedshiftSnapshotCopyGrant
                // The same check guards `CreateEventSubscription`'s own `Tags` member, so an
                // event subscription carries the stack's own tags without the three.
                | Self::RedshiftEventSubscription
                // Amazon MemoryDB refuses a tag key in the reserved namespace on every call
                // that takes tags — the `Tags` member of each group create and `TagResource`
                // alike, in the one check its own crate's tag validation makes — so a
                // parameter group and a subnet group each carry the stack's own tags without
                // the three.
                | Self::MemoryDbGroup(_)
                // Amazon Connect Customer Profiles refuses a tag key in the reserved namespace
                // on every call that takes tags: its `TagKey` shape is modelled with the
                // reserved prefix excluded (`^(?!aws:)…$`), which is the one check both
                // `CreateDomain`'s `Tags` map and `UpdateDomain`'s run — so a domain carries
                // the stack's own tags without the three.
                | Self::CustomerProfilesDomain
                // Recycle Bin refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateRule`'s own `Tags` list and `TagResource` alike, in the
                // one check its own crate's tag validation makes — so a retention rule carries
                // the stack's own tags without the three.
                | Self::RbinRule
                // Amazon Data Lifecycle Manager refuses a tag key in the reserved namespace on
                // every call that takes tags — `CreateLifecyclePolicy`'s own `Tags` map and
                // `TagResource` alike, in the one check its own crate's tag validation makes — so
                // a lifecycle policy carries the stack's own tags without the three.
                | Self::DlmLifecyclePolicy
                // Neptune Analytics refuses a tag key in the reserved namespace on every call
                // that takes tags — the `tags` map of each create and `TagResource` alike, in
                // the one check its own crate's tag validation makes — so a graph and a
                // snapshot each carry the stack's own tags without the three.
                | Self::NeptuneGraphGraph
                | Self::NeptuneGraphGraphSnapshot
                // Backup Gateway refuses a tag key in the reserved namespace on every call that
                // takes tags — `ImportHypervisorConfiguration`'s own `Tags` list and
                // `TagResource` alike, in the one check its own crate's tag validation makes
                // ("The key can't start with `aws:`") — so a hypervisor carries the stack's own
                // tags without the three.
                | Self::BackupGatewayHypervisor
                // Billing and Cost Management Dashboards refuses a tag key in the reserved
                // namespace on every call that takes tags — the `resourceTags` list of each
                // create and `TagResource` alike, in the one check its own crate's tag
                // validation makes — so a dashboard and a scheduled report each carry the
                // stack's own tags without the three.
                | Self::BcmDashboard
                | Self::BcmScheduledReport
                // Quick Setup refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateConfigurationManager`'s own `Tags` map and `TagResource`
                // alike, in the one check its own crate's tag validation makes — so a
                // configuration manager carries the stack's own tags without the three.
                | Self::SsmQuickSetupConfigurationManager
                // Amazon CloudWatch investigations refuses a tag key in the reserved namespace
                // on every call that takes tags — `CreateInvestigationGroup`'s own `tags` map,
                // `TagResource` and `UntagResource` alike, in the one check its own crate's tag
                // validation makes — so an investigation group carries the stack's own tags
                // without the three.
                | Self::AiOpsInvestigationGroup
        );
        // ...and a type whose registry SCHEMA states that it does not take CloudFormation's own
        // system tags is in the same position for a different reason: a resource type's schema
        // declares, beside whether the type is taggable at all, whether CloudFormation records
        // the three tags that name the stack and the resource's place in it on a resource of
        // that type — and for one whose schema says it does not, those three are not refused by
        // the owning service but simply never recorded, so the resource carries the stack's own
        // tags and the declaration's and nothing else. `AWS::CloudHSM::Cluster` is one: a
        // cluster a stack creates carries exactly the tags a client can see it asked for.
        let carries_no_system_tags = matches!(self, Self::CloudHsmCluster);
        propagated
            .iter()
            .filter(|(key, _)| {
                !((refuses_reserved_keys || carries_no_system_tags) && is_reserved_tag_key(key))
            })
            .cloned()
            .collect()
    }
}
