// SPDX-License-Identifier: CC0-1.0

//! What the OWNING service reports a provisioned resource's configuration to be now: the
//! per-type read arm drift detection compares a stack's expected configuration against.
//!
//! One concern of [`super`], split out of that file so the dispatch enum and the create,
//! update and delete arms stay inside the line bound this project holds a file to.

use super::*;

impl Provisioner {
    /// What the service that OWNS a resource reports the resource's configuration to be
    /// NOW: the document its own read answers with. This is the ACTUAL configuration
    /// [`super::drift`] compares a resource's expected template configuration against, so
    /// drift is read off the owning service rather than off CloudFormation's own record of
    /// what it once applied.
    ///
    /// `Ok(None)` is that service answering that it no longer holds the resource — judged
    /// on its own not-found CODE, never on a message, exactly as [`Self::delete`] does —
    /// which is the `DELETED` drift status. `Err(refusal)` is any other refusal: the check
    /// did not run, and the detection reports that rather than a resource as checked.
    pub(crate) async fn actual_configuration(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Option<Value>, Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                describe_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::KinesisStream => {
                describe_kinesis_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                describe_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchDashboard => {
                describe_dashboard(ctx, region, physical_resource_id).await
            }
            // CloudWatch's own read of the alarm the stack created, by the name it was
            // created under.
            Self::CloudWatchAlarm => {
                describe_metric_alarm(ctx, region, physical_resource_id).await
            }
            // CloudWatch's own read of the composite alarm the stack created, by the name it
            // was created under.
            Self::CloudWatchCompositeAlarm => {
                describe_composite_alarm(ctx, region, physical_resource_id).await
            }
            // Amazon ECS's own read of the service the stack created, by its name inside the
            // cluster it runs on.
            Self::EcsService => Box::pin(describe_ecs_service(ctx, region, resource)).await,
            Self::EcrPublicRepository => {
                describe_public_repository(ctx, region, physical_resource_id).await
            }
            Self::EcrRepository => describe_repository(ctx, region, physical_resource_id).await,
            // Amazon ECR publishes no read of ONE exclusion, so the record is read out of the
            // listing that registry answers its exclusion list in, by the principal ARN that
            // is this resource's physical id.
            Self::EcrPullTimeUpdateExclusion => {
                describe_pull_time_update_exclusion(ctx, region, physical_resource_id).await
            }
            // That registry's own read of its signing configuration, which carries no
            // identifier: a registry holds one, and it is the resource the stack declared.
            Self::EcrSigningConfiguration => describe_signing_configuration(ctx, region).await,

            Self::Ec2Vpc => describe_ec2_vpc(ctx, region, physical_resource_id).await,
            Self::Ec2Subnet => describe_ec2_subnet(ctx, region, physical_resource_id).await,
            Self::Ec2SecurityGroup => {
                describe_ec2_security_group(ctx, region, physical_resource_id).await
            }
            Self::Ec2Volume => describe_ec2_volume(ctx, region, physical_resource_id).await,
            Self::Ec2LaunchTemplate => {
                describe_ec2_launch_template(ctx, region, physical_resource_id).await
            }
            Self::AutoScalingGroup => {
                describe_auto_scaling_group(ctx, region, physical_resource_id).await
            }
            // Read back by the group and the name the policy was created under, which the
            // resource's own record carries.
            Self::AutoScalingScalingPolicy => {
                describe_scaling_policy(
                    ctx,
                    region,
                    &resource.properties,
                    &declared_name(resource),
                )
                .await
            }
            // The three children of a group are read back the same way: by the group the
            // resource's own record names and, for the two that carry a name, the name they
            // were created under.
            Self::AutoScalingLifecycleHook => {
                describe_lifecycle_hook(
                    ctx,
                    region,
                    &resource.properties,
                    &declared_name(resource),
                )
                .await
            }
            Self::AutoScalingScheduledAction => {
                describe_scheduled_action(
                    ctx,
                    region,
                    &resource.properties,
                    &declared_name(resource),
                )
                .await
            }
            Self::AutoScalingWarmPool => {
                describe_warm_pool(ctx, region, &resource.properties).await
            }
            Self::S3Bucket => describe_bucket(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the document the named bucket carries
            // now, read back from Amazon S3.
            Self::S3BucketPolicy => describe_bucket_policy(ctx, region, resource).await,
            Self::S3TablesTableBucket => {
                describe_table_bucket(ctx, region, physical_resource_id).await
            }
            // Read back by the same pair the delete addresses the namespace by.
            Self::S3TablesNamespace => describe_namespace(ctx, region, resource).await,
            Self::S3VectorsVectorBucket => {
                describe_vector_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsIndex => describe_index(ctx, region, physical_resource_id).await,
            Self::EmrCluster => describe_job_flow(ctx, region, physical_resource_id).await,
            // Amazon EMR's own reads of the three resources a stack declares beside a
            // cluster: the configuration by the name it was stored under, the Studio by the
            // id that service minted, and the mapping by the triple it keys one by, which
            // only the resource's own declaration carries.
            Self::EmrSecurityConfiguration => {
                describe_emr_security_configuration(ctx, region, physical_resource_id).await
            }
            Self::EmrStudio => describe_emr_studio(ctx, region, physical_resource_id).await,
            Self::EmrStudioSessionMapping => {
                describe_emr_studio_session_mapping(ctx, region, resource).await
            }
            Self::IamRole => describe_role(ctx, region, physical_resource_id).await,
            Self::IamInstanceProfile => {
                describe_instance_profile(ctx, region, physical_resource_id).await
            }
            Self::IamUser => describe_iam_user(ctx, region, physical_resource_id).await,
            // Read back from the user's own list of keys, IAM publishing no read of ONE
            // access key.
            Self::IamAccessKey => describe_iam_access_key(ctx, region, resource).await,
            Self::IamManagedPolicy => {
                describe_managed_policy(ctx, region, physical_resource_id).await
            }
            Self::CodePipelinePipeline => {
                describe_pipeline(ctx, region, physical_resource_id).await
            }
            // Read back by the same triple the delete addresses the registration by.
            Self::CodePipelineCustomActionType => {
                describe_custom_action_type(ctx, region, resource).await
            }
            // Read back out of the account's own webhook listing, CodePipeline publishing no
            // read of ONE webhook.
            Self::CodePipelineWebhook => {
                describe_webhook(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => describe_cluster(ctx, region, physical_resource_id).await,
            // Amazon ECS's own read of the capacity provider the stack created, by the name
            // it was created under.
            Self::EcsCapacityProvider => {
                describe_ecs_capacity_provider(ctx, region, physical_resource_id).await
            }
            // The association is the capacity-provider configuration of ONE cluster, so what
            // is read back is that cluster — the resource's own physical id — as Amazon ECS
            // reports it.
            Self::EcsClusterCapacityProviderAssociations => {
                describe_cluster_capacity_providers(ctx, region, physical_resource_id).await
            }
            // Amazon ECS's own read of the task set the stack created, by the triple that
            // service addresses one by, and of the task set a service reports as its primary
            // one.
            Self::EcsTaskSet => Box::pin(describe_ecs_task_set(ctx, region, resource)).await,
            Self::EcsPrimaryTaskSet => {
                Box::pin(describe_service_primary_task_set(ctx, region, resource)).await
            }
            Self::SqsQueue => describe_queue(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the `Policy` attribute of the queues it
            // names, which is read back from Amazon SQS as the document those queues carry
            // now.
            Self::SqsQueuePolicy => describe_queue_policy(ctx, region, resource).await,
            Self::OpenSearchServerlessCollection => {
                describe_collection(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServerlessSecurityPolicy => {
                describe_security_policy(ctx, region, resource).await
            }
            // The endpoint as that service reports it, read by the id it minted and
            // reported under the property names the type spells its members with.
            Self::OpenSearchServerlessVpcEndpoint => {
                describe_vpc_endpoint(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServiceDomain => {
                describe_opensearch_domain(ctx, region, physical_resource_id).await
            }
            Self::SageMakerModelPackageGroup => {
                describe_model_package_group(ctx, region, physical_resource_id).await
            }
            // SageMaker's own reads of the three resources, each by the name it was created
            // under, reported under the property names the type spells its members with.
            Self::SageMakerModel => {
                describe_sagemaker_model(ctx, region, physical_resource_id).await
            }
            Self::SageMakerEndpointConfig => {
                describe_sagemaker_endpoint_config(ctx, region, physical_resource_id).await
            }
            Self::SageMakerEndpoint => {
                describe_sagemaker_endpoint(ctx, region, physical_resource_id).await
            }
            Self::SageMakerNotebookInstance => {
                describe_sagemaker_notebook_instance(ctx, region, physical_resource_id).await
            }
            Self::SageMakerNotebookInstanceLifecycleConfig => {
                describe_sagemaker_notebook_instance_lifecycle_config(
                    ctx,
                    region,
                    physical_resource_id,
                )
                .await
            }
            Self::SnsTopic => describe_topic(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the `Policy` attribute of the topics it
            // names, which is read back from Amazon SNS as the document those topics carry
            // now.
            Self::SnsTopicPolicy => describe_topic_policy(ctx, region, resource).await,
            Self::SnsSubscription => describe_subscription(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                describe_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => describe_ip_set(ctx, region, physical_resource_id).await,
            // Read back by the same triple the delete addresses the entity by.
            Self::Wafv2WebAcl => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_WEB_ACL_CALLS).await
            }
            Self::Wafv2RuleGroup => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_RULE_GROUP_CALLS).await
            }
            Self::Wafv2IpSet => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_IP_SET_CALLS).await
            }
            Self::Wafv2RegexPatternSet => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_REGEX_PATTERN_SET_CALLS).await
            }
            // Read back as the web ACL AWS WAF reports in front of the protected resource
            // now, which is what the declared pair is compared against.
            Self::Wafv2WebAclAssociation => {
                describe_web_acl_association(ctx, region, resource).await
            }
            Self::GlueJob => describe_glue_job(ctx, region, physical_resource_id).await,
            Self::GlueTrigger => {
                describe_glue_trigger(ctx, region, physical_resource_id).await
            }
            // Read back by the same handles the deletes address the two catalog entities by,
            // and reported as the type's own properties: a database's definition is the
            // `DatabaseInput` property, a table's the `TableInput` one.
            Self::GlueDatabase => describe_glue_database(ctx, region, resource).await,
            Self::GlueTable => describe_glue_table(ctx, region, resource).await,
            Self::GlueCrawler => describe_glue_crawler(ctx, region, physical_resource_id).await,
            Self::GlueTableOptimizer => {
                describe_glue_table_optimizer(ctx, region, resource).await
            }
            Self::GrafanaWorkspace => {
                describe_workspace(ctx, region, physical_resource_id).await
            }
            Self::GuardDutyDetector => describe_detector(ctx, region, physical_resource_id).await,
            // Read back by the same pair the delete addresses the list by.
            Self::GuardDutyIpSet => describe_guardduty_ip_set(ctx, region, resource).await,
            Self::KmsKey => describe_kms_key(ctx, region, physical_resource_id).await,
            Self::KmsAlias => describe_kms_alias(ctx, region, physical_resource_id).await,
            // One `DescribeClusterV2` reads a cluster of either kind back.
            Self::MskCluster | Self::MskServerlessCluster => {
                describe_msk_cluster(ctx, region, physical_resource_id).await
            }
            // And `DescribeConfiguration` reads a configuration back, by the ARN that
            // service minted for it.
            Self::MskConfiguration => {
                describe_msk_configuration(ctx, region, physical_resource_id).await
            }
            Self::MqBroker => describe_mq_broker(ctx, region, physical_resource_id).await,
            // And `DescribeConfiguration` reads a configuration back, by the id that service
            // minted for it.
            Self::MqConfiguration => {
                describe_mq_configuration(ctx, region, physical_resource_id).await
            }
            Self::RdsDbInstance => {
                describe_db_instance(ctx, region, physical_resource_id).await
            }
            // The cluster is read back by the identifier it was created under, which is this
            // resource's physical id.
            Self::RdsDbCluster => {
                Box::pin(read_db_cluster(ctx, region, physical_resource_id)).await
            }
            Self::RdsDbSubnetGroup => {
                describe_db_subnet_group(ctx, region, physical_resource_id, RDS_SDK_SERVICE).await
            }
            // Amazon RDS's own read of the event subscription, by the name it was created
            // under.
            Self::RdsEventSubscription => {
                Box::pin(rds_event_subscriptions::read_subscription(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // Amazon RDS's own read of the option group, by the name it was created under.
            Self::RdsOptionGroup => {
                Box::pin(rds_option_groups::read_option_group(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            Self::RdsDbParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // Amazon RDS's own read of either snapshot, by the identifier it was taken under.
            Self::RdsDbSnapshot => {
                Box::pin(rds_snapshots::read_snapshot(
                    ctx,
                    region,
                    physical_resource_id,
                    rds_snapshots::DB_SNAPSHOT_CALLS,
                ))
                .await
            }
            Self::RdsClusterSnapshot => {
                Box::pin(rds_snapshots::read_snapshot(
                    ctx,
                    region,
                    physical_resource_id,
                    rds_snapshots::DB_CLUSTER_SNAPSHOT_CALLS,
                ))
                .await
            }
            // Amazon RDS's own read of the custom endpoint, by the identifier it was created
            // under.
            Self::RdsDbClusterEndpoint => {
                Box::pin(rds_cluster_endpoints::read_cluster_endpoint(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // Amazon RDS's own read of the global database, by the identifier it was created
            // under.
            Self::RdsGlobalCluster => {
                Box::pin(rds_global_clusters::read_global_cluster(
                    ctx,
                    region,
                    physical_resource_id,
                    RDS_SDK_SERVICE,
                ))
                .await
            }
            // The same front door's own reads of the Amazon Neptune relational resources, each
            // by the name or identifier it was created under.
            Self::NeptuneDbParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    neptune_relational::NEPTUNE_DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::NeptuneDbClusterParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    neptune_relational::NEPTUNE_DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::NeptuneDbSubnetGroup => {
                describe_db_subnet_group(
                    ctx,
                    region,
                    physical_resource_id,
                    neptune_relational::NEPTUNE_SDK_SERVICE,
                )
                .await
            }
            Self::NeptuneGlobalCluster => {
                Box::pin(rds_global_clusters::read_global_cluster(
                    ctx,
                    region,
                    physical_resource_id,
                    neptune_relational::NEPTUNE_SDK_SERVICE,
                ))
                .await
            }
            // Amazon RDS's own read of the proxy, by the name it was created under.
            Self::RdsDbProxy => read_db_proxy(ctx, region, physical_resource_id).await,
            // Read back by the same proxy-and-group pair the delete addresses the
            // registrations by, which only the resource's own declaration carries.
            Self::RdsDbProxyTargetGroup => {
                describe_db_proxy_target_group(ctx, region, resource).await
            }
            // The read is addressed by the rule's name AND the bus it is bound to, which
            // only the resource's own record carries.
            Self::EventsRule => describe_rule(ctx, region, resource).await,
            // EventBridge's own read of the bus the stack created, by the name it was created
            // under.
            Self::EventsEventBus => {
                event_buses::describe_event_bus(ctx, region, physical_resource_id).await
            }
            // A grant is read out of the POLICY the bus it was installed on carries now, by
            // the statement id that identifies it.
            Self::EventsEventBusPolicy => {
                event_buses::describe_event_bus_policy(ctx, region, resource).await
            }
            Self::SsmParameter => describe_parameter(ctx, region, physical_resource_id).await,
            // Systems Manager's own read of the association the stack created, by the id it
            // minted for it.
            Self::SsmAssociation => {
                describe_ssm_association(ctx, region, physical_resource_id).await
            }
            // Systems Manager's own read of the window the stack created, by the id it minted
            // for it — and of the target, out of that window's own listing, which is the one
            // read that service publishes for a target.
            Self::SsmMaintenanceWindow => {
                describe_ssm_maintenance_window(ctx, region, physical_resource_id).await
            }
            Self::SsmMaintenanceWindowTarget => {
                describe_ssm_maintenance_window_target(ctx, region, resource).await
            }
            // Systems Manager's own read of the document the stack created, by the name it was
            // created under.
            Self::SsmDocument => {
                describe_ssm_document(ctx, region, physical_resource_id).await
            }
            Self::StepFunctionsActivity => {
                describe_activity(ctx, region, physical_resource_id).await
            }
            Self::SchedulerScheduleGroup => {
                describe_schedule_group(ctx, region, physical_resource_id).await
            }
            // Resource Groups' own read of the group, by the name it was created under.
            Self::ResourceGroupsGroup => {
                describe_resource_group(ctx, region, physical_resource_id).await
            }
            // Application Insights' own read of the application, by the resource group it
            // monitors, which is how that service keys one.
            Self::ApplicationInsightsApplication => {
                describe_application(ctx, region, resource).await
            }
            // The read is addressed by the schedule's name AND its group, which only the
            // resource's own record carries.
            Self::SchedulerSchedule => describe_schedule(ctx, region, resource).await,
            Self::PipesPipe => {
                event_pipes::describe_pipe(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the group's name AND the pool that holds it, which
            // only the resource's own record carries.
            Self::CognitoUserPoolGroup => describe_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => {
                describe_acme_endpoint(ctx, region, physical_resource_id).await
            }
            Self::AcmCertificate => describe_certificate(ctx, region, physical_resource_id).await,
            Self::AcmPcaCertificateAuthority => {
                describe_certificate_authority(ctx, region, physical_resource_id).await
            }
            // None of the properties an `AWS::ACMPCA::Certificate` declares — the
            // certification request it had signed, the template, the validity — is reported
            // by any read Amazon Web Services Private CA publishes for an issued
            // certificate, so what the check can establish about one is whether that
            // service still holds it.
            Self::AcmPcaCertificate => {
                issued_certificate_configuration(ctx, region, physical_resource_id).await
            }
            // An activation's one declared property that is also READ back is the status it
            // left the CA in, which `DescribeCertificateAuthority` reports.
            Self::AcmPcaCertificateAuthorityActivation => {
                activation_configuration(ctx, region, physical_resource_id).await
            }
            // A grant is read out of the CA's own listing of its permissions, by the
            // principal this record granted to.
            Self::AcmPcaPermission => describe_ca_permission(ctx, region, resource).await,
            Self::Route53ResolverEndpoint => {
                describe_resolver_endpoint(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverRule => {
                describe_resolver_rule(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverRuleAssociation => {
                describe_resolver_rule_association(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallDomainList => {
                describe_firewall_domain_list(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallRuleGroup => {
                describe_firewall_rule_group(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallRuleGroupAssociation => {
                describe_firewall_rule_group_association(ctx, region, physical_resource_id).await
            }
            Self::Route53HostedZone => {
                describe_hosted_zone(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the record's own name, type and set identifier in the
            // zone its declaration names, which only the resource's own record carries.
            Self::Route53RecordSet => describe_record_set(ctx, region, resource).await,
            // Each record set the group declares, read back by its own name and type in the
            // zone the group names.
            Self::Route53RecordSetGroup => {
                describe_record_set_group(ctx, region, resource).await
            }
            // Read back by the id Route 53 minted, which is this resource's physical id.
            Self::Route53HealthCheck => describe_health_check(ctx, region, resource).await,
            // Read back by the identifier Route 53 minted for each, which is that
            // resource's physical id.
            Self::Route53CidrCollection => {
                describe_cidr_collection(ctx, region, physical_resource_id).await
            }
            Self::Route53DelegationSet => {
                describe_delegation_set(ctx, region, physical_resource_id).await
            }
            Self::Route53TrafficPolicy => {
                describe_traffic_policy(ctx, region, physical_resource_id).await
            }
            Self::ElasticBeanstalkApplication => {
                describe_eb_application(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the version's label AND the application that
            // holds it, which only the resource's own record carries.
            Self::ElasticBeanstalkApplicationVersion => {
                describe_eb_application_version(ctx, region, resource).await
            }
            // And the saved configuration's settings by the template name AND the
            // application that holds it, which only the resource's own record carries.
            Self::ElasticBeanstalkConfigurationTemplate => {
                describe_eb_configuration_template(ctx, region, resource).await
            }
            Self::ElasticBeanstalkEnvironment => {
                describe_eb_environment(ctx, region, physical_resource_id).await
            }
            // Read back by the assigned `ServerId`, like the delete.
            Self::TransferServer => describe_transfer_server(ctx, region, resource).await,
            Self::CodeDeployApplication => {
                describe_codedeploy_application(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the group's name AND the application that holds it,
            // which only the resource's own record carries.
            Self::CodeDeployDeploymentGroup => {
                describe_codedeploy_deployment_group(ctx, region, resource).await
            }
            Self::CodeDeployDeploymentConfig => {
                describe_codedeploy_deployment_config(ctx, region, physical_resource_id).await
            }
            // Batch's readers take a LIST and answer the records they found, so "no longer
            // held" is an empty list rather than a modelled not-found code
            // ([`batch_described`]).
            Self::BatchComputeEnvironment => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI,
                    "computeEnvironments",
                    physical_resource_id,
                )
                .await
            }
            Self::BatchJobQueue => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_JOB_QUEUES_URI,
                    "jobQueues",
                    physical_resource_id,
                )
                .await
            }
            Self::BatchJobDefinition => {
                describe_batch_job_definition(ctx, region, physical_resource_id).await
            }
            Self::BatchServiceEnvironment => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_SERVICE_ENVIRONMENTS_URI,
                    "serviceEnvironments",
                    physical_resource_id,
                )
                .await
            }
            // The two SHARING-plane types read back the same way: by the ARN that is the
            // resource's physical id, through a reader whose answer is a LIST.
            Self::BatchSchedulingPolicy => {
                describe_batch_scheduling_policy(ctx, region, physical_resource_id).await
            }
            Self::BatchQuotaShare => {
                describe_batch_quota_share(ctx, region, physical_resource_id).await
            }
            // Read back by the same cluster-and-principal pair the delete uses.
            Self::EksAccessEntry => describe_access_entry(ctx, region, resource).await,
            Self::EksCluster => describe_eks_cluster(ctx, region, physical_resource_id).await,
            // Read back by the same cluster-and-name pair the delete uses.
            Self::EksAddon => describe_eks_addon(ctx, region, resource).await,
            Self::EksNodegroup => describe_eks_nodegroup(ctx, region, resource).await,
            Self::EksPodIdentityAssociation => {
                describe_pod_identity_association(ctx, region, resource).await
            }
            Self::ApiGatewayDomainName => {
                describe_apigateway_domain_name(ctx, region, physical_resource_id).await
            }
            // Read back by the same domain-and-base-path pair the delete uses.
            Self::ApiGatewayBasePathMapping => {
                describe_apigateway_base_path_mapping(ctx, region, resource).await
            }
            Self::ApiGatewayV2Api => {
                describe_apigatewayv2_api(ctx, region, physical_resource_id).await
            }
            Self::ApiGatewayV2DomainName => {
                describe_apigatewayv2_domain_name(ctx, region, physical_resource_id).await
            }
            // Read back by the same domain-and-id pair the delete uses.
            Self::ApiGatewayV2ApiMapping => {
                describe_apigatewayv2_api_mapping(ctx, region, resource).await
            }
            // Read back by the same api-and-identifier pair each delete uses.
            Self::ApiGatewayV2Integration => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_INTEGRATION_CHILD)
                    .await
            }
            Self::ApiGatewayV2Route => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_ROUTE_CHILD).await
            }
            Self::ApiGatewayV2Stage => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_STAGE_CHILD).await
            }
            Self::ApiGatewayV2Deployment => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_DEPLOYMENT_CHILD)
                    .await
            }
            Self::ApiGatewayV2Authorizer => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_AUTHORIZER_CHILD)
                    .await
            }
            // Read back by the api, the parent and the identifier that address each one's
            // own delete.
            Self::ApiGatewayV2IntegrationResponse => {
                describe_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    &APIGATEWAYV2_INTEGRATION_RESPONSE_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2RouteResponse => {
                describe_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    &APIGATEWAYV2_ROUTE_RESPONSE_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Model => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_MODEL_CHILD).await
            }
            // Read back by the same ARN the delete addresses the routing rule by.
            Self::ApiGatewayV2RoutingRule => {
                describe_apigatewayv2_routing_rule(ctx, region, resource).await
            }
            Self::CodeCommitRepository => {
                describe_codecommit_repository(ctx, region, physical_resource_id).await
            }
            Self::CodeArtifactDomain => describe_codeartifact_domain(ctx, region, resource).await,
            Self::CodeArtifactRepository => {
                describe_codeartifact_repository(ctx, region, resource).await
            }
            // Read back by the same ARN the delete addresses the group by.
            Self::CodeBuildReportGroup => {
                describe_codebuild_report_group(ctx, region, resource).await
            }
            // Read back out of the batch read of projects, by the name the project was
            // created under.
            Self::CodeBuildProject => {
                describe_codebuild_project(ctx, region, physical_resource_id).await
            }
            // Read back out of the batch read of fleets, by the same handle the delete
            // addresses the fleet by.
            Self::CodeBuildFleet => describe_codebuild_fleet(ctx, region, resource).await,
            Self::SesConfigurationSet => {
                describe_ses_configuration_set(ctx, region, physical_resource_id).await
            }
            // Read back by the same text the delete addresses the identity by.
            Self::SesEmailIdentity => {
                describe_ses_email_identity(ctx, region, physical_resource_id).await
            }
            Self::SesReceiptRuleSet => {
                describe_ses_receipt_rule_set(ctx, region, physical_resource_id).await
            }
            // Read back by the same set-and-rule pair the delete addresses the rule by.
            Self::SesReceiptRule => describe_ses_receipt_rule(ctx, region, resource).await,
            Self::BackupVault => {
                describe_backup_vault(ctx, region, physical_resource_id).await
            }
            Self::BackupPlan => describe_backup_plan(ctx, region, physical_resource_id).await,
            // Read back by the same plan-and-selection pair the delete uses.
            Self::BackupSelection => describe_backup_selection(ctx, region, resource).await,
            // Amazon Redshift's own read of the cluster the stack created, by the
            // identifier it was created under.
            Self::RedshiftCluster => {
                describe_redshift_cluster(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock's own read of the guardrail the stack created, by the
            // identifier it was created under.
            Self::BedrockGuardrail => {
                describe_bedrock_guardrail(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock Agent's own read of the knowledge base the stack created, by
            // the id it minted for it.
            Self::BedrockKnowledgeBase => {
                describe_bedrock_knowledge_base(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock Agent's own read of the data source the stack created, by the
            // knowledge base and the id it minted for it.
            Self::BedrockDataSource => {
                describe_bedrock_data_source(ctx, region, resource).await
            }
            // Amazon Bedrock Agent's own read of the flow the stack created, by the ARN it
            // minted for it.
            Self::BedrockFlow => describe_bedrock_flow(ctx, region, physical_resource_id).await,
            // The Bedrock Agent runtime's own read of the session the stack created, by the
            // ARN it minted for it.
            Self::BedrockSession => {
                describe_bedrock_session(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock's own read of the application inference profile the stack
            // created, by the ARN it minted for it.
            Self::BedrockApplicationInferenceProfile => {
                describe_bedrock_inference_profile(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock Data Automation's own read of the blueprint, the project or the
            // library the stack created, by the ARN it minted for it.
            Self::BedrockDataAutomation(kind) => {
                describe_bedrock_data_automation(ctx, region, kind, physical_resource_id).await
            }
            // Amazon Redshift's own read of the parameter group the stack created, by the
            // name it was created under.
            Self::RedshiftClusterParameterGroup => {
                describe_cluster_parameter_group(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift's own read of the subnet group the stack created, by the name
            // it was created under.
            Self::RedshiftClusterSubnetGroup => {
                describe_cluster_subnet_group(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift's own read of the snapshot copy grant the stack created, by the
            // name it was created under.
            Self::RedshiftSnapshotCopyGrant => {
                describe_snapshot_copy_grant(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift's own read of the event subscription the stack created, by the
            // name it was created under.
            Self::RedshiftEventSubscription => {
                describe_event_subscription(ctx, region, physical_resource_id).await
            }
            // The AgentCore control plane's own read of the identity, by the name it was
            // created under.
            Self::BedrockAgentCoreWorkloadIdentity => {
                describe_workload_identity(ctx, region, physical_resource_id).await
            }
            // The same control plane's own reads of the rest of its family, each by the
            // identifier it minted for the resource.
            Self::BedrockAgentCoreRuntime => {
                describe_agentcore_runtime(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreMemory => {
                describe_agentcore_memory(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGateway => {
                describe_agentcore_gateway(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGatewayTarget => {
                describe_agentcore_gateway_target(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreSandbox(sandbox) => {
                describe_agentcore_sandbox(ctx, region, sandbox, physical_resource_id).await
            }
            // Amazon Braket publishes no read of ONE spending limit, so the limit is read
            // out of the account's own listing, by the ARN that service minted for it.
            Self::BraketSpendingLimit => {
                describe_braket_spending_limit(ctx, region, physical_resource_id).await
            }
            // That service's own read of the resource this physical id names: each of the six
            // resources out of its own listing narrowed to the one identifier, and a resource
            // policy through `GetResourcePolicy` on the ARN it is attached to. Boxed, as the
            // arms that reach a service through another of this emulator's own are.
            Self::SmsVoiceResource(resource_kind) => {
                Box::pin(describe_sms_voice_resource(
                    ctx,
                    region,
                    resource_kind,
                    physical_resource_id,
                ))
                .await
            }
            // Amazon CloudWatch investigations' own read of ONE investigation group, by the
            // ARN it minted for the group — this resource's physical id.
            Self::AiOpsInvestigationGroup => {
                describe_investigation_group(ctx, region, physical_resource_id).await
            }
            // Amazon GameLift Servers' own reads of the build and of the script, each by the
            // id it minted for the resource.
            Self::GameLiftBuild => {
                Box::pin(describe_gamelift_build(ctx, region, physical_resource_id)).await
            }
            Self::GameLiftScript => {
                Box::pin(describe_gamelift_script(ctx, region, physical_resource_id)).await
            }
            // AWS HealthLake's own reads: the data store by the `DatastoreId` it minted for
            // it, and the profile by the `ProfileId` inside the ARN the stack published.
            Self::HealthLakeFhirDatastore => {
                describe_fhir_datastore(ctx, region, physical_resource_id).await
            }
            Self::HealthLakeDataTransformationProfile => {
                describe_data_transformation_profile(ctx, region, physical_resource_id).await
            }
            // AWS Direct Connect's own reads: the gateway out of the listing its
            // `directConnectGatewayId` narrows, and the group through that service's own
            // `GetResiliencyGroup`, each by the identifier the ARN the stack published
            // carries.
            Self::DirectConnectGateway => {
                describe_direct_connect_gateway(ctx, region, physical_resource_id).await
            }
            Self::DirectConnectResiliencyGroup => {
                describe_resiliency_group(ctx, region, physical_resource_id).await
            }
            // That service's own read of the suite, by the id it drew for it.
            Self::IotCoreDeviceAdvisorSuiteDefinition => {
                describe_suite_definition(ctx, region, physical_resource_id).await
            }
            // That service's own reads of the dashboard and of the scheduled report, each by
            // the ARN it minted for the resource.
            Self::BcmDashboard => {
                describe_bcm_dashboard(ctx, region, physical_resource_id).await
            }
            Self::BcmScheduledReport => {
                describe_bcm_scheduled_report(ctx, region, physical_resource_id).await
            }
            // Application Signals' own reads: the objective by the ARN it is addressed by, the
            // account's grouping configuration by the read that reports this account's
            // definitions, and the instrumentation point by the identity the stack holds for
            // it.
            Self::ApplicationSignalsServiceLevelObjective => {
                describe_slo(ctx, region, physical_resource_id).await
            }
            Self::ApplicationSignalsGroupingConfiguration => {
                describe_grouping_configuration(ctx, region).await
            }
            Self::ApplicationSignalsInstrumentationConfig => {
                describe_instrumentation_config(ctx, region, resource).await
            }
            // Clean Rooms ML's own read of the dataset, by the ARN it minted.
            Self::CleanRoomsMlTrainingDataset => {
                describe_training_dataset(ctx, region, physical_resource_id).await
            }
            // CloudHSM's own description of the cluster, by the id inside the ARN that is this
            // resource's physical id.
            Self::CloudHsmCluster => {
                describe_cloudhsm_cluster(ctx, region, physical_resource_id).await
            }
            // Amplify UI Builder's own read of the entity, by the id it minted, inside the app
            // environment the declaration names.
            Self::AmplifyUiBuilderComponent => {
                describe_ui_entity(ctx, region, UiEntity::Component, resource).await
            }
            Self::AmplifyUiBuilderForm => {
                describe_ui_entity(ctx, region, UiEntity::Form, resource).await
            }
            Self::AmplifyUiBuilderTheme => {
                describe_ui_entity(ctx, region, UiEntity::Theme, resource).await
            }
            // AWS Amplify's own read of the app, by the id it minted for it.
            Self::AmplifyApp => describe_amplify_app(ctx, region, resource).await,
            // AWS Amplify's own read of the branch, by the name it was created under inside the
            // app it belongs to.
            Self::AmplifyBranch => describe_amplify_branch(ctx, region, resource).await,
            // Amazon MWAA's own read of the environment, by the name it was created under.
            Self::MwaaEnvironment => {
                describe_mwaa_environment(ctx, region, physical_resource_id).await
            }
            // Directory Service's own description of the directory, by the id it minted for
            // it.
            Self::DirectoryServiceDirectory(_) => {
                describe_ds_directory(ctx, region, physical_resource_id).await
            }

            // X-Ray publishes no read of ONE sampling rule, so the rule is read out of the
            // account's own listing, by the identity that service minted for it.
            Self::XraySamplingRule => describe_sampling_rule(ctx, region, resource).await,
            // X-Ray's own read of ONE group, by the identity that service minted for it.
            Self::XrayGroup => describe_xray_group(ctx, region, resource).await,
            // AWS Config's own reads of the three resources, each by the name it was
            // created under.
            Self::ConfigConfigurationRecorder => {
                describe_configuration_recorder(ctx, region, physical_resource_id).await
            }
            Self::ConfigDeliveryChannel => {
                describe_delivery_channel(ctx, region, physical_resource_id).await
            }
            Self::ConfigConfigRule => {
                describe_config_rule(ctx, region, physical_resource_id).await
            }
            // Amazon ElastiCache's own read of the cluster, by the identifier it was created
            // under. The cluster is reported with its SCALAR configuration — the engine, the
            // node type and count, the windows and the group names, each the member of the
            // same name the type declares — plus its configuration endpoint; a property that
            // is a LIST on the wire (the security groups, the log delivery configurations)
            // is left out, which is the position every arm whose property is not a scalar is
            // in and what [`super::drift`] documents.
            Self::ElastiCacheCacheCluster => {
                Box::pin(read_cache_cluster(ctx, region, physical_resource_id)).await
            }
            // The same service's own read of the serverless cache, by the name it was created
            // under, reported with its SCALAR configuration — the engine, the major version,
            // the description and the snapshot posture, each the member of the same name the
            // type declares — plus its two endpoints; a property that is a LIST on the wire
            // (the subnets, the security groups) or a STRUCTURE (the usage limits) is left
            // out, which is the position every arm whose property is not a scalar is in.
            Self::ElastiCacheServerlessCache => {
                Box::pin(read_serverless_cache(ctx, region, physical_resource_id)).await
            }
            // The same service's own read of the replication group, by the identifier it was
            // created under, reported with its SCALAR configuration — the engine, the node
            // type, the description and the snapshot posture, each the member of the same name
            // the type declares; a member that is a LIST (the node groups, the member
            // clusters) or a STRUCTURE (either endpoint) is left out, which is the position
            // every arm whose property is not a scalar is in.
            Self::ElastiCacheReplicationGroup => {
                Box::pin(read_replication_group(ctx, region, physical_resource_id)).await
            }
            // The same service's own read of the cache subnet group, by the name it was
            // created under, reported with its description and the VPC it sits in; its subnet
            // list is a composite member and is left out the same way.
            Self::ElastiCacheSubnetGroup => {
                Box::pin(read_cache_subnet_group(ctx, region, physical_resource_id)).await
            }
            // Amazon MemoryDB's own read of the group, by the name it was created under.
            Self::MemoryDbGroup(group) => {
                describe_memorydb_group(ctx, region, group, physical_resource_id).await
            }
            // Amazon Connect Customer Profiles' own read of the domain, by the name it was
            // created under.
            Self::CustomerProfilesDomain => {
                describe_customer_profiles_domain(ctx, region, physical_resource_id).await
            }
            // Amazon AppFlow's own read of the connector profile, by the name the stack
            // published for it. Boxed for the reason the other heavy arms of this match are.
            Self::AppFlowConnectorProfile => {
                Box::pin(describe_appflow_connector_profile(ctx, region, physical_resource_id))
                    .await
            }
            // Recycle Bin's own read of the rule, by the identifier it minted for it.
            Self::RbinRule => describe_rbin_rule(ctx, region, physical_resource_id).await,
            // Amazon Data Lifecycle Manager's own read of the policy, by the id it minted for it.
            Self::DlmLifecyclePolicy => {
                describe_dlm_lifecycle_policy(ctx, region, physical_resource_id).await
            }
            // AWS Network Security Manager's own read of the entity, by the ARN it minted for
            // it.
            Self::NetworkSecurityManagerEntity(entity) => {
                describe_entity(ctx, region, entity, physical_resource_id).await
            }
            // Neptune Analytics' own reads of the graph and of the snapshot, each by the id it
            // minted for the resource.
            Self::NeptuneGraphGraph => {
                describe_neptune_graph(ctx, region, physical_resource_id).await
            }
            Self::NeptuneGraphGraphSnapshot => {
                describe_graph_snapshot(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift Serverless' own reads of the namespace, the workgroup and the
            // snapshot, each by the name the stack published for the resource.
            Self::RedshiftServerlessNamespace => {
                describe_redshift_serverless_namespace(ctx, region, physical_resource_id).await
            }
            Self::RedshiftServerlessWorkgroup => {
                describe_redshift_serverless_workgroup(ctx, region, physical_resource_id).await
            }
            Self::RedshiftServerlessSnapshot => {
                describe_redshift_serverless_snapshot(ctx, region, physical_resource_id).await
            }
            // Backup Gateway's own read of the hypervisor, by the ARN it minted for it.
            Self::BackupGatewayHypervisor => {
                describe_backup_gateway_hypervisor(ctx, region, physical_resource_id).await
            }
            // IAM Access Analyzer's own read of the analyzer, by the name it was created under,
            // or of the archive rule, by that name and the rule's own.
            Self::AccessAnalyzerResource(kind) => {
                describe_access_analyzer_resource(ctx, region, kind, physical_resource_id).await
            }
            // MediaConvert publishes no read of its certificate associations, so the trust is
            // read from the other side: the resources Certificate Manager reports as using
            // the certificate that is this resource's physical id.
            Self::MediaConvertCertificateAssociation => {
                describe_mediaconvert_certificate_association(ctx, region, physical_resource_id)
                    .await
            }
            // Lake Formation's own read of what the principal the grant names holds on the
            // resource it is on, out of the listing of that resource's own grants.
            Self::LakeFormationPermissions(grant) => {
                describe_lake_formation_permissions(ctx, region, grant, resource).await
            }
            // Its own read of the data cells filter, by the four values that address one —
            // which is also what the delete addresses it by.
            // Boxed for the reason the create arm is.
            Self::LakeFormationDataCellsFilter => {
                Box::pin(describe_lake_formation_data_cells_filter(ctx, region, resource)).await
            }
            // Amazon EC2's own read of the Client VPN endpoint, by the id it minted for it.
            Self::Ec2ClientVpnEndpoint => {
                describe_client_vpn_endpoint(ctx, region, physical_resource_id).await
            }
            // Its own read of the authorization rule, out of the rules it reports for the
            // endpoint the declaration names — which, with the network, is the only handle
            // that service publishes for one.
            Self::Ec2ClientVpnAuthorizationRule => {
                describe_client_vpn_authorization_rule(ctx, region, resource).await
            }
            // Rekognition's own reads of the three resources a stack declares in it: the
            // collection by the id it was created under, the project out of the description it
            // reports for that name, and the dataset by the ARN it minted.
            Self::RekognitionCollection => {
                describe_rekognition_collection(ctx, region, physical_resource_id).await
            }
            Self::RekognitionProject => {
                describe_rekognition_project(ctx, region, physical_resource_id).await
            }
            Self::RekognitionDataset => {
                describe_rekognition_dataset(ctx, region, physical_resource_id).await
            }
            // Quick Setup's own read of the configuration manager, by the `ManagerArn` it
            // minted for it.
            Self::SsmQuickSetupConfigurationManager => {
                describe_configuration_manager(ctx, region, physical_resource_id).await
            }
            // Amazon VPC Lattice's own reads of the two resources a stack declares in it, each
            // by the ARN that service minted for it.
            Self::VpcLatticeServiceNetwork => {
                describe_vpc_lattice_service_network(ctx, region, physical_resource_id).await
            }
            Self::VpcLatticeAccessLogSubscription => {
                describe_vpc_lattice_access_log_subscription(ctx, region, physical_resource_id)
                    .await
            }
            // Its own read of one of the four RESOURCE-ACCESS resources, by that same ARN,
            // which is one of the two forms every identifier member of that service accepts.
            Self::VpcLatticeResource(kind) => {
                vpc_lattice_resources::describe_vpc_lattice_resource(
                    ctx,
                    region,
                    kind,
                    physical_resource_id,
                )
                .await
            }
            // Amazon Lex V2's own reads of the three resources a stack declares in it: the bot
            // by the `botId` it minted, and a version or an alias by the pair its identity is
            // composed of.
            Self::LexBot(kind) => describe_lex_resource(ctx, region, kind, resource).await,
        }
    }
}
