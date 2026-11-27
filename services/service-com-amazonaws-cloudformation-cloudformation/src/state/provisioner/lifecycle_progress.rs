// SPDX-License-Identifier: CC0-1.0

//! What the OWNING service says about a resource's LIFECYCLE PROGRESS: whether the resource
//! it was asked to create has come up yet, what it publishes for one now that it has, and
//! whether it still holds one it was asked to delete.
//!
//! One concern of [`super`], split out of that file so the dispatch enum and the create,
//! update and delete arms stay inside the line bound this project holds a file to — the same
//! split [`super::reported_configuration`] is.

use super::*;

impl Provisioner {
    /// Whether the resource this provisioner CREATED has reached the state the owning
    /// service reports it ready in — read from that service, never decided here.
    ///
    /// Most creates leave the resource ready, so most types answer [`Readiness::Ready`]
    /// without asking anything: the call that created the resource already said everything
    /// there was to say about it. A type whose create is documented to leave the resource
    /// coming up is the exception, and for such a type this is what CloudFormation holds the
    /// resource's creation open on ([`super::stabilising`]) — asked once where the create
    /// answered, and again by [`super::provisioning::advance_stabilisations`] until that
    /// service reports it ready.
    ///
    /// `Err` is the owning service's own refusal of the READ, which says nothing about the
    /// resource: the caller decides what an unanswered read means.
    ///
    /// `declared` is the properties the resource was declared with, where the caller knows
    /// them: a CHILD resource is read back by its parent and its own name, and the parent is
    /// a property of the declaration rather than part of the physical id (an Amazon EKS
    /// add-on's and node group's cluster). An arm that needs none of them ignores it, and
    /// `None` is a declaration the caller does not hold, which such an arm reports as the
    /// refused read it is.
    pub(crate) async fn readiness(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
        declared: Option<&Value>,
    ) -> Result<Readiness, Refusal> {
        match self {
            // "A certificate enters status PENDING_VALIDATION upon being requested"
            // (`CertificateDetail$Status`) and is usable once ACM has issued it, so the
            // resource's creation is finished when ACM reports it `ISSUED` — and the
            // validation records the declaration names hosted zones for are published while
            // it is still waiting on them, which is why this arm reads the declaration.
            Self::AcmCertificate => {
                certificate_readiness(ctx, region, physical_resource_id, declared).await
            }
            // "Private CA is pending creation" is the status a created CA is reported in
            // (`CertificateAuthorityStatus$CREATING`), and its certificate signing request
            // is not available in it, so the resource's creation is finished once that
            // service reports the CA out of it.
            Self::AcmPcaCertificateAuthority => {
                certificate_authority_readiness(ctx, region, physical_resource_id).await
            }
            // `DomainStatus$Created` is "true if domain creation is complete" and
            // `DomainStatus$Processing` "true if OpenSearch Service is processing
            // configuration changes", so a domain the create has just answered for is still
            // coming up and the resource's creation is finished when that service reports it
            // settled.
            Self::OpenSearchServiceDomain => {
                domain_readiness(ctx, region, physical_resource_id).await
            }
            // "The volume state" a created EBS volume is reported in is `creating`
            // (`Volume$State`), and a volume cannot be attached until it is `available`, so
            // the resource's creation is finished once Amazon EC2 reports it there.
            Self::Ec2Volume => volume_readiness(ctx, region, physical_resource_id).await,
            // `CreateDeliveryStream` "is an asynchronous operation that immediately
            // returns. The initial status of the Firehose stream is CREATING. After the
            // Firehose stream is created, its status is ACTIVE and it now accepts data …
            // Attempts to send data to a delivery stream that is not in the ACTIVE state
            // cause an exception", so a stream the create has just answered for cannot take
            // the data a client writes to it and the resource's creation is finished once
            // that service reports it `ACTIVE`.
            Self::FirehoseDeliveryStream => {
                delivery_stream_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateStream` "is an asynchronous operation. Upon receiving a CreateStream
            // request, Kinesis Data Streams immediately returns and sets the stream status
            // to CREATING. After the stream is created, Kinesis Data Streams sets the stream
            // status to ACTIVE. You should perform read and write operations only on an
            // ACTIVE stream", so a stream the create has just answered for serves no read
            // or write yet and the resource's creation is finished once that service reports
            // it `ACTIVE`.
            Self::KinesisStream => {
                kinesis_stream_readiness(ctx, region, physical_resource_id).await
            }
            // "If the status is UPDATING, you cannot update the domain name"
            // (`DomainName$domainNameStatus`), and an edge-optimized domain is served by a
            // CloudFront distribution that is not in place when the create answers, so the
            // resource's creation is finished once API Gateway reports the domain
            // `AVAILABLE`.
            Self::ApiGatewayDomainName => {
                domain_name_readiness(ctx, region, physical_resource_id).await
            }
            // "A status of `available` means the proxy is ready to handle requests. Other
            // values indicate that you must wait for the proxy to be ready, or take some
            // action to resolve an issue" (`DBProxy$Status`), so a proxy the create has
            // just answered for handles no connection yet and the resource's creation is
            // finished once Amazon RDS reports it `available`.
            Self::RdsDbProxy => db_proxy_readiness(ctx, region, physical_resource_id).await,
            // "When the cluster is in the `creating` state, only cluster-level information
            // is displayed" (`DescribeCacheClusters`) and the cluster serves nothing in it,
            // so the resource's creation is finished once Amazon ElastiCache reports it
            // `available`.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_readiness(ctx, region, physical_resource_id)).await
            }
            // `CreateServerlessCache` answers the cache `creating`, in which it has no
            // endpoint and serves no client, so the resource's creation is finished once
            // Amazon ElastiCache reports it `available`.
            Self::ElastiCacheServerlessCache => {
                Box::pin(serverless_cache_readiness(ctx, region, physical_resource_id)).await
            }
            // `CreateReplicationGroup` answers the group `creating`, in which it publishes no
            // endpoint and serves no client, so the resource's creation is finished once
            // Amazon ElastiCache reports it `available`.
            Self::ElastiCacheReplicationGroup => {
                Box::pin(replication_group_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                    declared,
                ))
                .await
            }
            // `CreateDBInstance` answers the instance `creating`, in which it serves no
            // connection and publishes no endpoint a client can reach, so the resource's
            // creation is finished once Amazon RDS reports it `available` — which is also
            // what the rest of a stack needs before it can use the database.
            Self::RdsDbInstance => {
                Box::pin(db_instance_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                    declared,
                ))
                .await
            }
            // `CreateDBCluster` answers the cluster `creating`, in which it serves no
            // connection and publishes no endpoint a client can reach, so the resource's
            // creation is finished once Amazon RDS reports it `available` — which is also
            // what a member DB instance of the same stack needs before it can join it.
            Self::RdsDbCluster => {
                Box::pin(db_cluster_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                    declared,
                ))
                .await
            }
            // Either snapshot create answers the snapshot `creating`, in which "percent
            // progress" is 0 and nothing can be restored from it, so the resource's creation
            // is finished once Amazon RDS reports it `available`.
            Self::RdsDbSnapshot => {
                Box::pin(rds_snapshots::snapshot_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                    rds_snapshots::DB_SNAPSHOT_CALLS,
                ))
                .await
            }
            Self::RdsClusterSnapshot => {
                Box::pin(rds_snapshots::snapshot_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                    rds_snapshots::DB_CLUSTER_SNAPSHOT_CALLS,
                ))
                .await
            }
            // `CreateDBClusterEndpoint` answers the endpoint `creating`, in which it resolves
            // to nothing and serves no connection, so the resource's creation is finished once
            // Amazon RDS reports it `available`.
            Self::RdsDbClusterEndpoint => {
                Box::pin(rds_cluster_endpoints::cluster_endpoint_readiness(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // "When you create a new cluster, the Amazon EKS API server endpoint is private
            // to your VPC" only once the control plane is up: a created cluster is reported
            // `CREATING` and "you must wait for the cluster status to become ACTIVE before
            // you can … add nodes" (`CreateCluster` / `Cluster$status`), so the resource's
            // creation is finished once that service reports it `ACTIVE`.
            Self::EksCluster => eks_cluster_readiness(ctx, region, physical_resource_id).await,
            // An add-on is reported `CREATING` while Amazon EKS applies its Kubernetes
            // resources to the cluster and a node group while that service brings its
            // instances up, so in neither does the resource the declaration asked for serve
            // the cluster yet: each is held open until that service reports it `ACTIVE`,
            // read by the cluster the declaration names and the resource's own name.
            Self::EksAddon => {
                let identity = eks_child_identity(EKS_ADDON, declared, physical_resource_id)?;
                eks_addon_readiness(ctx, region, &identity).await
            }
            Self::EksNodegroup => {
                let identity = eks_child_identity(EKS_NODEGROUP, declared, physical_resource_id)?;
                eks_nodegroup_readiness(ctx, region, &identity).await
            }
            // `CreateKnowledgeBase` answers 202 with the knowledge base `CREATING` — "the
            // knowledge base is being created", in which it cannot be queried — so the
            // resource's creation is finished once Amazon Bedrock Agent reports it `ACTIVE`.
            Self::BedrockKnowledgeBase => {
                knowledge_base_readiness(ctx, region, physical_resource_id).await
            }
            // The modelled `DataSource$status` documents `AVAILABLE` as the status of a data
            // source that "has been created and is ready for ingestion into the knowledge
            // base", so a data source still `CREATING` serves no ingestion yet and the
            // resource's creation is finished once that service reports it `AVAILABLE`.
            Self::BedrockDataSource => {
                data_source_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateFlow` leaves the flow `NotPrepared` and the preparation this type's
            // create then asks for answers it `Preparing` — in neither can the flow be
            // invoked — so the resource is held open until Amazon Bedrock Agent reports the
            // flow `Prepared`.
            Self::BedrockFlow => flow_readiness(ctx, region, physical_resource_id).await,
            // `CreateDataAutomationProject` answers the project `IN_PROGRESS` — Amazon Bedrock
            // Data Automation is still building it, and an inference run against it in that
            // state has nothing to run — so the resource is held open until that service reports
            // the project `COMPLETED`. A blueprint and a library are complete as their creates
            // answer, which is what that arm says of them.
            Self::BedrockDataAutomation(kind) => {
                bedrock_data_automation_readiness(ctx, region, kind, physical_resource_id).await
            }
            // `ImportHypervisorConfiguration` leaves the hypervisor `PENDING` — the first of
            // the states `HypervisorDetails$State` documents, in which Backup Gateway has not
            // reached the host yet, so no gateway can be associated with it and no metadata
            // sync of it would report anything — so the resource is held open until that
            // service reports the hypervisor `ONLINE`.
            Self::BackupGatewayHypervisor => {
                backup_gateway_hypervisor_readiness(ctx, region, physical_resource_id).await
            }
            // `CreatePipe` answers the pipe `CREATING` and `UpdatePipe` answers it
            // `UPDATING` — "the state the pipe is in" — and in neither does the pipe carry
            // the configuration the declaration asked for, so the resource is held open
            // until EventBridge Pipes reports it resting ([`event_pipes::readiness`]).
            Self::PipesPipe => event_pipes::readiness(ctx, region, physical_resource_id).await,
            // A created environment is `CREATING` — "the request to create the environment is
            // in progress" — and one whose VPC endpoints the caller owns is `PENDING` until
            // they exist, while an accepted update walks `CREATING_SNAPSHOT` and `UPDATING`;
            // in none of those does the environment carry what the deployment asked for, so
            // the resource is held open until Amazon MWAA reports it `AVAILABLE`.
            Self::MwaaEnvironment => {
                mwaa_environment_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateStream` "is an asynchronous operation" and `CreateSignalingChannel` leaves
            // the channel coming up the same way: a created stream takes no media and a created
            // channel carries no signaling until Amazon Kinesis Video Streams reports it
            // `ACTIVE`, so the resource is held open until it does.
            Self::KinesisVideoResource(kind) => {
                Box::pin(kinesis_video_readiness(
                    ctx,
                    region,
                    kind,
                    physical_resource_id,
                ))
                .await
            }
            // `CreateFHIRDatastore` answers the data store `CREATING` — "the data store
            // status" a store is in while HealthLake brings it up, in which its FHIR endpoint
            // serves no interaction — so the resource is held open until that service reports
            // it `ACTIVE` ([`healthlake_resources::fhir_datastore_readiness`]). (A data
            // transformation PROFILE needs no arm: its create makes it in DRAFT state, which
            // is readable as that call answers.)
            Self::HealthLakeFhirDatastore => {
                fhir_datastore_readiness(ctx, region, physical_resource_id).await
            }
            // A created directory is `Requested` and then `Creating` — its domain controllers
            // are still being built, and it serves no directory traffic in either stage — so
            // the resource is held open until Directory Service reports it `Active`, which is
            // also when the alias and the single sign-on flag the declaration asks for are
            // applied: that service takes either only on a directory that is serving.
            Self::DirectoryServiceDirectory(_) => {
                ds_directory_readiness(ctx, region, physical_resource_id, declared).await
            }
            // `CreateCluster` answers a cluster `CREATE_IN_PROGRESS` — CloudHSM is still placing
            // its hardware, and the cluster takes no HSM and no initialization in that state —
            // so the resource is held open until that service reports the cluster created
            // ([`cloudhsm_clusters::cloudhsm_cluster_readiness`]).
            Self::CloudHsmCluster => {
                cloudhsm_cluster_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateCluster` answers a cluster `creating`, in which it serves no connection
            // and publishes no endpoint at all, so the resource's creation is finished once
            // Amazon Redshift reports it `available`.
            Self::RedshiftCluster => {
                Box::pin(redshift_cluster_readiness(ctx, region, physical_resource_id)).await
            }
            // `CreateNotebookInstance` answers the instance `Pending` — SageMaker creates
            // its network interface and launches the ML compute instance after the create
            // has answered, and the instance serves no Jupyter session and mints no
            // presigned URL until it is `InService` — so the resource is held open until
            // that service reports it there
            // ([`sagemaker_resources::notebook_instance_readiness`]).
            Self::SageMakerNotebookInstance => {
                notebook_instance_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateEndpoint` answers the endpoint `Creating` — SageMaker launches the ML
            // compute instances of its production variants after the create has answered, and
            // the endpoint serves no inference and takes no inference component until it is
            // `InService` — so the resource is held open until that service reports it there
            // ([`sagemaker_resources::sagemaker_endpoint_readiness`]).
            Self::SageMakerEndpoint => {
                sagemaker_endpoint_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateDocument` answers a document `Creating` — "the document is being
            // created" — and Systems Manager reports it `Active` once it is in place, so the
            // resource is held open until that service resolves the document by the name the
            // stack publishes ([`ssm_documents::ssm_document_readiness`]).
            Self::SsmDocument => {
                ssm_document_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateCapacityProvider` answers a provider whose capacity Amazon ECS has to
            // stand up `PROVISIONING` — "Only capacity providers in an ACTIVE state can be
            // used in a cluster" — so the resource is held open until that service reports it
            // there ([`ecs_capacity_providers::ecs_capacity_provider_readiness`]). A provider
            // over an Auto Scaling group the caller already has is answered `ACTIVE`, so that
            // read ends the wait at once.
            Self::EcsCapacityProvider => {
                ecs_capacity_provider_readiness(ctx, region, physical_resource_id).await
            }
            // `PutClusterCapacityProviders` leaves the cluster's attachments
            // `UPDATE_IN_PROGRESS` — "the available capacity or capacity providers are
            // updating", the state a second association of the same cluster is refused in —
            // so the resource is held open until Amazon ECS reports the update finished
            // ([`ecs_cluster_capacity_providers::cluster_capacity_providers_readiness`]).
            Self::EcsClusterCapacityProviderAssociations => {
                cluster_capacity_providers_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateGraph` answers the graph `CREATING` — a graph serves no query and
            // publishes no endpoint in it — and `CreateGraphSnapshot` answers the snapshot
            // `CREATING`, in which there is nothing to restore from, so each resource is held
            // open until Neptune Analytics reports it `AVAILABLE`.
            Self::NeptuneGraphGraph => {
                neptune_graph_readiness(ctx, region, physical_resource_id).await
            }
            Self::NeptuneGraphGraphSnapshot => {
                graph_snapshot_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateWorkgroup` answers the workgroup `CREATING` — in which it serves no
            // connection and publishes no reachable endpoint — and `CreateSnapshot` answers the
            // snapshot `CREATING`, in which there is nothing to restore from, so each resource
            // is held open until Amazon Redshift Serverless reports it `AVAILABLE`. (A
            // NAMESPACE is answered `AVAILABLE` by its own create, so it needs no arm.)
            Self::RedshiftServerlessWorkgroup => {
                workgroup_readiness(ctx, region, physical_resource_id).await
            }
            Self::RedshiftServerlessSnapshot => {
                redshift_serverless_snapshot_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateCollection` answers the collection `CREATING` — in which it publishes
            // no reachable endpoint and takes no index — so the resource's creation is
            // finished once OpenSearch Serverless reports it `ACTIVE`, which is also what
            // the rest of a stack needs before it can use the collection.
            Self::OpenSearchServerlessCollection => {
                collection_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateVpcEndpoint` answers the endpoint `PENDING` — a "Pending VPCEndpoint
            // resource", through which no client reaches a collection yet — and OpenSearch
            // Serverless reports it `ACTIVE` once it "is ready to use", so the resource's
            // creation is finished once that service reports it there.
            Self::OpenSearchServerlessVpcEndpoint => {
                vpc_endpoint_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateApplication` answers the application `CREATING` — Application Insights is
            // still onboarding the resources of the group it was created over, and the
            // components a configuration is applied to are not there in it — so the resource is
            // held open until that service reports the onboarding finished.
            Self::ApplicationInsightsApplication => {
                application_readiness(ctx, region, physical_resource_id, declared).await
            }
            // `AuthorizeClientVpnIngress` answers the rule `authorizing` — in which it grants
            // no client access to the network yet — and Amazon EC2 reports it `active` once
            // it does, so the resource's creation is finished once that service reports it
            // there ([`client_vpn::client_vpn_authorization_rule_readiness`]).
            Self::Ec2ClientVpnAuthorizationRule => {
                client_vpn_authorization_rule_readiness(ctx, region, declared).await
            }
            // Each of these Amazon Bedrock AgentCore creates answers `202 Accepted` with the
            // resource in the transient status its own family documents — a runtime, a
            // gateway and a target `CREATING`, a memory `CREATING` on its way to `ACTIVE` —
            // and in none of them does the resource serve anything, so each is held open
            // until that control plane reports it up. (The two SANDBOX types are not here:
            // either create answers the sandbox `READY`.)
            Self::BedrockAgentCoreRuntime => {
                agentcore_runtime_readiness(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreMemory => {
                agentcore_memory_readiness(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGateway => {
                agentcore_gateway_readiness(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGatewayTarget => {
                agentcore_gateway_target_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateDirectConnectGateway` answers the gateway `pending` — "the initial state
            // after calling CreateDirectConnectGateway", in which it passes no traffic — and
            // `CreateResiliencyGroup` answers the group `pending` the same way, so each
            // resource is held open until AWS Direct Connect reports it `available`
            // ([`directconnect_resources`]).
            Self::DirectConnectGateway => {
                direct_connect_gateway_readiness(ctx, region, physical_resource_id).await
            }
            Self::DirectConnectResiliencyGroup => {
                resiliency_group_readiness(ctx, region, physical_resource_id).await
            }
            // "Creating a dataset takes a while to complete. Use DescribeDataset to check the
            // current status. The dataset created successfully if the value of Status is
            // CREATE_COMPLETE", so a dataset the create has just answered for holds nothing a
            // model could train on yet and the resource's creation is finished once Amazon
            // Rekognition reports it `CREATE_COMPLETE` ([`rekognition_resources`]).
            Self::RekognitionDataset => {
                rekognition_dataset_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateConfigurationManager` answers the manager with its deployment still
            // walking the phases Quick Setup publishes for one (`INITIALIZING`, `DEPLOYING`,
            // `SUCCEEDED`) — a configuration manager IS the deployment of its definitions, and
            // no configuration of it is in place until that work has succeeded — so the
            // resource's creation is finished once that service reports the deployment
            // `SUCCEEDED` ([`ssm_quicksetup_configuration_managers`]).
            Self::SsmQuickSetupConfigurationManager => {
                configuration_manager_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateEnvironment` "launches an environment for the specified application"
            // and answers while that launch is still running, so the environment it answers
            // for is `Launching` — not yet "available to have an action performed on it,
            // such as update or terminate" — and the resource's creation is finished once
            // Elastic Beanstalk reports it `Ready` ([`eb_environment_readiness`]).
            Self::ElasticBeanstalkEnvironment => {
                eb_environment_readiness(ctx, region, physical_resource_id).await
            }
            _ => Ok(Readiness::Ready),
        }
    }

    /// The attributes the owning service publishes for the resource this provisioner created
    /// now that it reports the resource UP — the ones its create answer could not carry,
    /// because the resource did not have them yet.
    ///
    /// Only a type whose create leaves the resource coming up has any ([`Self::readiness`]),
    /// and only where that type publishes an attribute the owning service fills in when the
    /// resource is running; every other type answers nothing, so its records stay exactly
    /// what its create reported.
    ///
    /// `Err` is the owning service's own refusal of the READ: the caller decides what an
    /// unanswered read means, exactly as it does for [`Self::readiness`].
    pub(crate) async fn ready_attributes(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource_type: &str,
        physical_resource_id: &str,
    ) -> Result<BTreeMap<String, String>, Refusal> {
        match self {
            // `DomainStatus$Endpoint` is "the domain-specific endpoint used to submit index,
            // search, and data upload requests", and a domain still being provisioned has no
            // such endpoint — nor the dual-stack or VPC hosts beside it — so the create's
            // answer carries none of them and the type's endpoint attributes are read off the
            // domain once that service reports it settled.
            Self::OpenSearchServiceDomain => {
                domain_ready_attributes(ctx, region, resource_type, physical_resource_id).await
            }
            // "Nothing of the control plane itself exists yet" while Amazon EKS reports a
            // cluster `CREATING`: its API server endpoint, the certificate authority a
            // client verifies that endpoint with, the OIDC issuer of its identity provider
            // and the security group it makes for the control plane are all values that
            // service fills in as it provisions the control plane, so they are read off the
            // cluster once it reports it ready.
            Self::EksCluster => {
                Ok(read_eks_cluster(ctx, region, physical_resource_id)
                    .await?
                    .map(|cluster| eks_cluster_attributes(&cluster))
                    .unwrap_or_default())
            }
            // A knowledge base's `Status` and `UpdatedAt` are not the ones its create
            // answered with once that service has brought it up, so the type's attributes
            // are read off the knowledge base again when it reports it `ACTIVE`.
            Self::BedrockKnowledgeBase => {
                knowledge_base_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A data source's `DataSourceStatus` and `UpdatedAt` are not the ones its create
            // answered with once that service has settled it, so the type's attributes are
            // read off the data source again when it reports it `AVAILABLE`.
            Self::BedrockDataSource => {
                data_source_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A flow's `Status` and `UpdatedAt` are not the ones its create answered with once
            // that service has prepared it, so the type's attributes are read off the flow
            // again when it reports it `Prepared`.
            Self::BedrockFlow => flow_ready_attributes(ctx, region, physical_resource_id).await,
            // A project's `status` and the instants Amazon Bedrock Data Automation records for it
            // are not the ones its create answered once that service has built it, so the type's
            // attributes are read off the project again when it reports it `COMPLETED`.
            Self::BedrockDataAutomation(kind) => {
                bedrock_data_automation_ready_attributes(ctx, region, kind, physical_resource_id)
                    .await
            }
            // A pipe's `CurrentState`, its `StateReason` and the instant it was last
            // modified are not the ones its create answered with once that service has
            // brought it up, so the type's attributes are read off the pipe again when it
            // reports it resting.
            Self::PipesPipe => {
                event_pipes::ready_attributes(ctx, region, physical_resource_id).await
            }
            // An environment's web server host, the queue of its Celery Executor, its two VPC
            // endpoint services and the log group of each enabled Apache Airflow log type are
            // values Amazon MWAA fills in as it brings the environment up, so the type's
            // attributes are read off the environment again once it reports it `AVAILABLE`.
            Self::MwaaEnvironment => {
                mwaa_environment_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A data store's `DatastoreStatus` is `CREATING` when its create answers, and the
            // FHIR endpoint HealthLake serves it at is that service's to decide, so the
            // type's attributes are read off the store again once it reports it `ACTIVE`.
            Self::HealthLakeFhirDatastore => {
                fhir_datastore_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A directory's `Alias` is the directory id itself until an alias is assigned, and
            // its DNS servers hold no address until its controllers are up, so the type's
            // attributes are read off the directory again once that service reports it
            // `Active`.
            Self::DirectoryServiceDirectory(kind) => {
                ds_directory_ready_attributes(ctx, region, kind, physical_resource_id).await
            }
            // A cluster's `State` is the one its create answered (`CREATE_IN_PROGRESS`) until
            // CloudHSM has placed it, so the type's attributes are read off the cluster again
            // once that service reports it created.
            Self::CloudHsmCluster => {
                cloudhsm_cluster_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A graph has no endpoint while it is `CREATING`: the address a client reaches it
            // at is one Neptune Analytics publishes once the graph is serving, so the type's
            // attributes are read off the graph again once it reports it `AVAILABLE`. (A
            // snapshot publishes no attribute at all, so it needs no arm.)
            Self::NeptuneGraphGraph => {
                neptune_graph_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A workgroup still `CREATING` has no endpoint a client can reach — that address is
            // what Amazon Redshift Serverless publishes once the workgroup is serving — and a
            // snapshot still `CREATING` carries none of the sizes that service measures, so
            // each type's attributes are read off the resource again once it reports it
            // `AVAILABLE`.
            Self::RedshiftServerlessWorkgroup => {
                workgroup_ready_attributes(ctx, region, physical_resource_id).await
            }
            Self::RedshiftServerlessSnapshot => {
                redshift_serverless_snapshot_ready_attributes(ctx, region, physical_resource_id)
                    .await
            }
            // A cache cluster has no endpoint while it is `creating`: the address a client
            // connects to is one Amazon ElastiCache fills in as it provisions the cluster,
            // so the type's endpoint attributes are read off the cluster once it reports it
            // `available`.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_ready_attributes(ctx, region, physical_resource_id)).await
            }
            // A serverless cache has neither of its endpoints while it is `creating`, and
            // neither the full engine version ElastiCache settles on for the declared major
            // version nor the time it created the cache is known before that service has: the
            // type's attributes are read off the cache once it reports it `available`.
            Self::ElastiCacheServerlessCache => {
                Box::pin(serverless_cache_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // A replication group publishes none of its endpoints while it is `creating`: the
            // addresses a client writes to and reads from are ones Amazon ElastiCache fills in
            // as it provisions the group's shards, so the type's endpoint attributes are read
            // off the group once it reports it `available`.
            Self::ElastiCacheReplicationGroup => {
                Box::pin(replication_group_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // A Redshift cluster has no connection endpoint while it is `creating`: the
            // address and the port a client connects to are what Amazon Redshift publishes
            // once the cluster is serving, so the type's endpoint attributes are read off the
            // cluster when it reports it `available`.
            Self::RedshiftCluster => {
                Box::pin(redshift_cluster_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // A DB cluster and a DB instance are both `creating` when their create answers,
            // and a resource that is still coming up publishes no endpoint: the address and
            // the port a client connects to are what Amazon RDS publishes once it serves, so
            // the endpoint attributes either type publishes are read off the resource when
            // that service reports it `available`.
            Self::RdsDbCluster => {
                Box::pin(rds_published_endpoints::db_cluster_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            Self::RdsDbInstance => {
                Box::pin(rds_published_endpoints::db_instance_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            _ => Ok(BTreeMap::new()),
        }
    }

    /// Whether the service that owns the resource this provisioner DELETED has finished
    /// removing it — read from that service, never decided here.
    ///
    /// The mirror of [`Self::readiness`], and stated per type for the same reason: most
    /// deletes take the resource away, so most types answer [`Removal::Gone`] without asking
    /// anything, while a type whose delete its own service documents as asynchronous leaves
    /// the resource in a state it is still being removed from. For such a type this is what
    /// CloudFormation holds the stack operation open on ([`super::stabilising`]) — asked once
    /// where the delete answered, and again by
    /// [`super::provisioning::advance_stabilisations`] until that service no longer holds the
    /// resource — so no operation is reported complete while a resource it deleted is still
    /// there.
    ///
    /// `Err` is the owning service's own refusal of the READ, which says nothing about the
    /// resource: the caller decides what an unanswered read means.
    pub(crate) async fn removal(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
    ) -> Result<Removal, Refusal> {
        match self {
            // "DeleteDeliveryStream is an asynchronous API. When an API request to
            // DeleteDeliveryStream succeeds, the Firehose stream is marked for deletion, and
            // it goes into the DELETING state … Removal of a Firehose stream that is in the
            // DELETING state is a low priority operation for the service", so the stream is
            // still there when the delete answers and the deletion is finished once Amazon
            // Data Firehose no longer holds it.
            Self::FirehoseDeliveryStream => {
                delivery_stream_removal(ctx, region, physical_resource_id).await
            }
            // "DeleteStream is an asynchronous operation … After a DeleteStream request, the
            // specified stream is in the DELETING state until Kinesis Data Streams completes
            // the deletion", so the stream is still there when the delete answers and the
            // deletion is finished once that service no longer holds it.
            Self::KinesisStream => {
                kinesis_stream_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteKnowledgeBase` answers 202 with the knowledge base `DELETING` — "the
            // knowledge base is being deleted" — so it is still there when the delete
            // answers and the deletion is finished once that service no longer holds it.
            Self::BedrockKnowledgeBase => {
                knowledge_base_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteDataSource` answers 202 with the data source `DELETING` — "the data
            // source is being deleted" — so it is still there when the delete answers and the
            // deletion is finished once that service no longer holds it.
            Self::BedrockDataSource => {
                data_source_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteDataAutomationLibrary` ACCEPTS the delete and answers the library
            // `DELETING` — that service still holds it, and a library it holds still occupies
            // the `libraryName` the declaration asked for — so the deletion is finished once it
            // no longer answers for the library. A blueprint and a project are gone as their
            // deletes answer, which is what that arm says of them.
            Self::BedrockDataAutomation(kind) => {
                bedrock_data_automation_removal(ctx, region, kind, physical_resource_id).await
            }
            // `DeleteEnvironment` leaves the environment `DELETING` — "the request to delete
            // the environment is in progress" — so it is still there when the delete answers
            // and the deletion is finished once Amazon MWAA no longer holds it.
            Self::MwaaEnvironment => {
                mwaa_environment_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteStream` "marks the stream for deletion" and leaves it `DELETING`, so the
            // stream is still there when the delete answers and the deletion is finished once
            // Amazon Kinesis Video Streams no longer holds it; a signaling channel is gone as
            // its own delete answers, which the same arm reports.
            Self::KinesisVideoResource(kind) => {
                Box::pin(kinesis_video_removal(ctx, region, kind, physical_resource_id)).await
            }
            // `DeleteFHIRDatastore` leaves the data store `DELETING` and HealthLake goes on
            // reporting it in that status while it takes the store away, so it is still there
            // when the delete answers and the deletion is finished once that service reports
            // it `DELETED` ([`healthlake_resources::fhir_datastore_removal`]). (A PROFILE
            // needs no arm: the call that deletes one takes it away as it answers.)
            Self::HealthLakeFhirDatastore => {
                fhir_datastore_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteDirectory` leaves the directory `Deleting` — the modelled stage it is
            // reported in while its servers are decommissioned — so it is still there when the
            // delete answers and the deletion is finished once Directory Service no longer
            // holds it.
            Self::DirectoryServiceDirectory(_) => {
                ds_directory_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteCluster` leaves the cluster `DELETE_IN_PROGRESS` — the modelled state it is
            // reported in while CloudHSM takes its hardware away — so it is still being removed
            // when the delete answers, and the deletion is finished once that service reports it
            // `DELETED` ([`cloudhsm_clusters::cloudhsm_cluster_removal`]). Which is what keeps a
            // stack from deleting the subnets the cluster is still placed in.
            Self::CloudHsmCluster => {
                cloudhsm_cluster_removal(ctx, region, physical_resource_id).await
            }
            // "The `DeleteCacheCluster` operation deletes a previously provisioned cluster"
            // and leaves it `deleting` while it does, so the cluster is still there when the
            // delete answers and the deletion is finished once Amazon ElastiCache no longer
            // holds it — which is what keeps a stack from deleting the subnet group the
            // cluster still sits in.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_removal(ctx, region, physical_resource_id)).await
            }
            // `DeleteServerlessCache` leaves the cache `deleting` while Amazon ElastiCache
            // takes it away, so it is still there when the delete answers and the deletion is
            // finished once that service no longer holds it — which is what keeps a stack from
            // deleting the subnets and the security groups the cache still sits in.
            Self::ElastiCacheServerlessCache => {
                Box::pin(serverless_cache_removal(ctx, region, physical_resource_id)).await
            }
            // `DeleteReplicationGroup` leaves the group `deleting` while Amazon ElastiCache
            // takes it and its member clusters away, so it is still there when the delete
            // answers and the deletion is finished once that service no longer holds it —
            // which is what keeps a stack from deleting the subnet group those clusters still
            // sit in.
            Self::ElastiCacheReplicationGroup => {
                Box::pin(replication_group_removal(ctx, region, physical_resource_id)).await
            }
            // `DeleteDBCluster` leaves the cluster `deleting` until the deletion completes,
            // so it is still there when the delete answers and the deletion is finished once
            // Amazon RDS no longer holds it — which is what keeps a stack from deleting the
            // DB subnet group the cluster still sits in.
            Self::RdsDbCluster => {
                Box::pin(db_cluster_removal(ctx, region, physical_resource_id)).await
            }
            // "Before you can delete a notebook instance, you must call the
            // StopNotebookInstance API", so the delete stopped the instance and the
            // deletion is finished once SageMaker no longer holds it
            // ([`sagemaker_resources::notebook_instance_removal`]).
            Self::SageMakerNotebookInstance => {
                notebook_instance_removal(ctx, region, physical_resource_id).await
            }
            // "SageMaker frees up all of the resources that were deployed when the endpoint
            // was created" (`DeleteEndpoint`) after that call has answered, so the deletion is
            // finished once SageMaker no longer holds the endpoint
            // ([`sagemaker_resources::sagemaker_endpoint_removal`]) — which is what keeps a
            // stack from deleting the endpoint configuration it is still deployed from.
            Self::SageMakerEndpoint => {
                sagemaker_endpoint_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteApplication` "deletes the specified application along with all
            // associated versions and configurations" and answers before it has — the
            // application is still described while the deletion runs — so it is still there
            // when the delete answers and the deletion is finished once Elastic Beanstalk no
            // longer holds it. (Its ENVIRONMENT needs no arm here: that delete waits for the
            // termination itself, because the application this stack deletes next cannot go
            // while the environment still runs; and an application VERSION is removed by the
            // call that deletes it.)
            Self::ElasticBeanstalkApplication => {
                eb_application_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteCapacityProvider` answers a provider whose capacity Amazon ECS has to
            // take down again `DEPROVISIONING`, so the stack's deletion is held open until
            // that service reports it deleted
            // ([`ecs_capacity_providers::ecs_capacity_provider_removal`]).
            Self::EcsCapacityProvider => {
                ecs_capacity_provider_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteGraph` answers the graph `DELETING` and `DeleteGraphSnapshot` answers the
            // snapshot `DELETING`, so each is still there when its delete answers and the
            // deletion is finished once Neptune Analytics no longer holds it.
            Self::NeptuneGraphGraph => {
                neptune_graph_removal(ctx, region, physical_resource_id).await
            }
            Self::NeptuneGraphGraphSnapshot => {
                graph_snapshot_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteWorkgroup` answers the workgroup it removed, and the endpoint network
            // interfaces it had placed in the stack's own subnets go back with it — so the
            // deletion is finished once Amazon Redshift Serverless no longer holds the
            // workgroup, which is what keeps a stack from deleting those subnets while the
            // workgroup is still in them. A SNAPSHOT is read back the same way.
            Self::RedshiftServerlessWorkgroup => {
                workgroup_removal(ctx, region, physical_resource_id).await
            }
            Self::RedshiftServerlessSnapshot => {
                redshift_serverless_snapshot_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteAutoScalingGroup` with `ForceDelete` terminates the instances the group
            // was holding as it answers, and a termination is not instant: each of those
            // instances holds the subnet it was placed in and the security groups it ran with
            // until Amazon EC2 reports it `terminated`, so the resource is still being removed
            // while any of them is left — which is what keeps a stack from deleting the network
            // it launched them into while they are still in it
            // ([`compute_fleet::auto_scaling_group_removal`]).
            Self::AutoScalingGroup => {
                auto_scaling_group_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteConfiguration` answers the configuration `DELETING` — Amazon MSK takes a
            // deleted configuration away asynchronously, so its own reads still resolve the
            // ARN while they do — and the deletion is finished once that service no longer
            // holds it.
            Self::MskConfiguration => {
                msk_configuration_removal(ctx, region, physical_resource_id).await
            }
            // Every Amazon Bedrock AgentCore delete answers `202 Accepted` with the resource
            // in a `DELETING` status — that control plane takes a deleted resource away
            // asynchronously, so its own reads still resolve the identifier while it does —
            // and the deletion is finished once it no longer holds the resource. Which is
            // what keeps a stack from deleting the gateway a target it just deleted is still
            // served by.
            Self::BedrockAgentCoreRuntime => {
                agentcore_runtime_removal(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreMemory => {
                agentcore_memory_removal(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGateway => {
                agentcore_gateway_removal(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreGatewayTarget => {
                agentcore_gateway_target_removal(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreSandbox(sandbox) => {
                agentcore_sandbox_removal(ctx, region, sandbox, physical_resource_id).await
            }
            // Either AWS Direct Connect delete leaves the resource `deleting` — "the initial
            // state after calling DeleteDirectConnectGateway" — so the resource is still
            // reported when the delete answers, and the deletion is finished once that
            // service reports the terminal `deleted` ([`directconnect_resources`]).
            Self::DirectConnectGateway => {
                direct_connect_gateway_removal(ctx, region, physical_resource_id).await
            }
            Self::DirectConnectResiliencyGroup => {
                resiliency_group_removal(ctx, region, physical_resource_id).await
            }
            // Both Rekognition deletes are asynchronous: "the project is deleted when the
            // project no longer appears in the DescribeProjects response" and a deleted
            // dataset answers `ResourceNotFoundException` only once it is gone, so each
            // resource's deletion is finished on that service's own report
            // ([`rekognition_resources`]). A project is addressed by the NAME it was created
            // under, which is its physical id, and a dataset by the ARN that service minted.
            Self::RekognitionProject => {
                rekognition_project_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteConfigurationManager` leaves the manager reported while Quick Setup tears
            // the configurations it deployed down — its deployment summary moves to `DELETING`
            // — so it is still there when the delete answers and the deletion is finished once
            // that service no longer holds it
            // ([`ssm_quicksetup_configuration_managers`]).
            Self::SsmQuickSetupConfigurationManager => {
                configuration_manager_removal(ctx, region, physical_resource_id).await
            }
            Self::RekognitionDataset => {
                rekognition_dataset_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteCluster` answers with the cluster `DELETING` and Amazon EKS goes on
            // serving reads of it in that status until the control plane is torn down, so the
            // cluster is still there when the delete answers and the deletion is finished
            // once that service no longer holds it
            // ([`compute_and_developer_tools::eks_cluster_removal`]).
            Self::EksCluster => eks_cluster_removal(ctx, region, physical_resource_id).await,
            _ => Ok(Removal::Gone),
        }
    }
}
