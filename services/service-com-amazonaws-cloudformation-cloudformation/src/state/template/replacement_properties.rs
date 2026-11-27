// SPDX-License-Identifier: CC0-1.0

use super::*;

/// Whether a REPLACEMENT of this type must delete the resource it replaces BEFORE creating the
/// one that replaces it.
///
/// It is for a type whose owning service cannot hold two of the resource at once: "each subnet
/// in your VPC must be associated with a route table" and with exactly one, so Amazon EC2
/// refuses a second association for a subnet that already has one — on AWS the create-then-
/// delete order is not merely unusual there, it is impossible. CloudFormation therefore deletes
/// such a resource first, whatever its `UpdateReplacePolicy` states: there is no window in
/// which both exist, so the old physical resource cannot be kept while the new one is created.
///
/// Every other type's replacement is create-before-delete, which is the order that keeps the
/// resource it replaces serving its clients until the new one is in force — and the order the
/// deployment's cleanup phase takes the old one down in
/// ([`crate::state::provisioning::reconcile`]).
pub fn replacement_deletes_first(resource_type: &str) -> bool {
    resource_type == EC2_SUBNET_ROUTE_TABLE_ASSOCIATION
        // An Application Signals instrumentation configuration is the same case: one code
        // location holds at most one configuration — a create at a location that already has
        // one is refused as a conflict ("If a configuration already exists for the same
        // service, environment, signal type, and location, this operation returns a conflict
        // instead of overwriting it") — so a replacement that keeps the location cannot create
        // the new record while the old one is there.
        || resource_type == APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG
}

/// The properties of this type a resource cannot take a NEW VALUE for while it goes on
/// existing — the ones the resource and property types reference publishes with "Update
/// requires: `Replacement`" — BESIDE the name, which is in that position for every type
/// whose owning service cannot rename the resource ([`name_is_create_only`]) and is read
/// from the declaration rather than catalogued.
///
/// A resource is created with such a property and nothing changes it afterwards: an IAM
/// access key belongs to the user `CreateAccessKey` minted it for and carries the `Serial`
/// the template rotates it with ("incrementing this value notifies CloudFormation that you
/// want to rotate your access key… CloudFormation will replace the AccessKey resource with
/// a new access key"), so a declaration that states either differently asks for a DIFFERENT
/// resource — which CloudFormation replaces rather than updates.
///
/// A type with no entry has none beyond its name, so the name alone decides whether a
/// re-deploy means the same resource ([`super::declared_name`]).
///
/// The parts of a COMPOSITE primary identifier are in that position by construction
/// ([`composite_identity_properties`]): the resource IS the binding those values name, so
/// stating one of them differently asks for a different resource.
pub fn replacement_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        // An Application Insights application IS the monitoring of one resource group: that
        // group is what the application is keyed by, every call on one takes it, and
        // `UpdateApplication` carries no member for it — so a declaration naming another group
        // asks for a DIFFERENT application, which CloudFormation replaces. (The type declares no
        // name of its own, so [`name_is_create_only`] has nothing to catch here.)
        APPLICATION_INSIGHTS_APPLICATION => &["ResourceGroupName"],
        // A CodeArtifact PACKAGE GROUP is identified by the PATTERN it matches, inside the
        // domain that holds it: `CreatePackageGroup` fixes all three — `UpdatePackageGroup`
        // takes a group's contact information and description and no member for any of them —
        // so a declaration stating a different pattern, domain or domain owner asks for a
        // DIFFERENT group, which CloudFormation replaces. (The type declares no name of its
        // own, so [`name_is_create_only`] has nothing to catch here.)
        crate::state::provisioner::codeartifact_package_groups::CODEARTIFACT_PACKAGE_GROUP => {
            &["Pattern", "DomainName", "DomainOwner"]
        }
        IAM_ACCESS_KEY => &["Serial", "UserName"],
        // A pull through cache rule keeps the UPSTREAM it caches from: `upstreamRegistry`,
        // `upstreamRegistryUrl` and `upstreamRepositoryPrefix` are members of
        // `CreatePullThroughCacheRule` and of no member of `UpdatePullThroughCacheRule` — which
        // takes a rule's prefix with a new credential secret and custom role and nothing else —
        // so a declaration restating any of them asks for a DIFFERENT rule, which
        // CloudFormation replaces. (Its prefix is in that position through
        // [`name_is_create_only`].)
        crate::state::provisioning::ecr_pull_through_cache_rule::ECR_PULL_THROUGH_CACHE_RULE => {
            &[
                "UpstreamRegistry",
                "UpstreamRegistryUrl",
                "UpstreamRepositoryPrefix",
            ]
        }
        // A published state machine VERSION keeps everything it was published with: it is the
        // snapshot `PublishStateMachineVersion` took of the machine, and Step Functions
        // publishes no call that edits one — so a declaration restating any property asks for a
        // DIFFERENT version, which CloudFormation replaces by publishing one
        // ([`crate::state::provisioning::step_functions_versions_and_aliases`]). (An ALIAS
        // needs no entry: its description and its routing configuration are
        // `UpdateStateMachineAlias`' own members, so a change to either reaches the alias in
        // place, and its NAME is in this position through [`name_is_create_only`].)
        crate::state::provisioning::step_functions_versions_and_aliases::STEP_FUNCTIONS_STATE_MACHINE_VERSION => {
            &["Description", "StateMachineArn", "StateMachineRevisionId"]
        }
        // An Amazon Athena SAVED QUERY keeps everything it was saved with: `CreateNamedQuery`
        // takes the database, the SQL and the workgroup, and the one call that changes a query
        // afterwards (`UpdateNamedQuery`) takes the query's id with a new name, description and
        // query string and NO database or workgroup — Athena publishes no call that moves a
        // saved query to another database or workgroup. So a declaration restating any of them
        // asks for a DIFFERENT query, which CloudFormation replaces; the resource and property
        // types reference publishes every property of this type with "Update requires:
        // Replacement", and a replacement is how this emulator applies a change to one
        // ([`crate::state::provisioning::athena_resources`]). (Its NAME is in that position
        // through [`name_is_create_only`].)
        crate::state::provisioning::athena_resources::ATHENA_NAMED_QUERY => {
            &["Database", "Description", "QueryString", "WorkGroup"]
        }
        // A MemoryDB parameter group's engine FAMILY and its description are fixed when
        // Amazon MemoryDB makes it: both are members of `CreateParameterGroup`, and the one
        // call that changes a group afterwards (`UpdateParameterGroup`) takes nothing but
        // the group's parameter values — so a declaration restating either asks for a
        // different group, which CloudFormation replaces. (A subnet group needs no entry:
        // both of its properties beside the name are `UpdateSubnetGroup`'s own members.)
        MEMORYDB_PARAMETER_GROUP => &["Description", "Family"],
        // An `AWS::LakeFormation::DataCellsFilter` keeps the CELLS it was created selecting:
        // the resource provider of that type publishes no update handler at all, every
        // property of it carrying "Update requires: `Replacement`" — so a declaration
        // restating the row predicate or either way of naming columns asks for a DIFFERENT
        // filter, which CloudFormation replaces. (The four values that identify one are in
        // that position by construction, being the parts of its composite identifier.)
        crate::state::provisioner::lake_formation_data_cells_filters::LAKEFORMATION_DATA_CELLS_FILTER => {
            crate::state::provisioner::lake_formation_data_cells_filters::DATA_CELLS_FILTER_SELECTION
        }
        // A BACKUP VAULT keeps the KMS key AWS Backup encrypts it under: `EncryptionKeyArn`
        // is a member of `CreateBackupVault` and of no call that changes a vault — that
        // service publishes no `UpdateBackupVault` at all — so a declaration restating it
        // asks for a DIFFERENT vault, which CloudFormation replaces. (Its three remaining
        // configurations — the lock, the resource policy and the notifications — are each
        // applied by a call of that service's own, so a change to any of them reaches the
        // vault in place; its NAME is in this position through [`name_is_create_only`], and
        // its tags through that service's own tagging calls.)
        BACKUP_VAULT => &["EncryptionKeyArn"],
        // A MediaConvert queue keeps the PRICING PLAN it was created with: `pricingPlan` is a
        // member of `CreateQueue` and of no member of `UpdateQueue` — which takes a queue's
        // description, status, concurrency and reservation plan and no pricing plan at all
        // (`smithy_query(query="operation",operation="CreateQueue,UpdateQueue")`) — so a
        // declaration restating it asks for a different queue, which CloudFormation replaces.
        // (Its NAME is in that position through [`name_is_create_only`], as a job template's
        // and a preset's are; every other property of the three is a member of that type's own
        // update.)
        MEDIACONVERT_QUEUE => &["PricingPlan"],
        // An Amazon EFS FILE SYSTEM keeps what it was made with: the zone a One Zone file
        // system lives in, whether it is encrypted and under which key, and its performance
        // mode are members of `CreateFileSystem` and of no member of `UpdateFileSystem` —
        // which takes a file system's id with a throughput mode and a provisioned amount and
        // nothing else — so a declaration restating one of them asks for a DIFFERENT file
        // system, which CloudFormation replaces.
        EFS_FILE_SYSTEM => &[
            "AvailabilityZoneName",
            "Encrypted",
            "KmsKeyId",
            "PerformanceMode",
        ],
        // An ACCESS POINT keeps everything it was created with: Amazon EFS publishes no call
        // that edits one — `CreateAccessPoint` takes the file system, the POSIX user, the root
        // directory and the client token, and the only call that changes an access point
        // afterwards is a tagging call — so a declaration restating any of them asks for a
        // DIFFERENT access point.
        EFS_ACCESS_POINT => &["ClientToken", "FileSystemId", "PosixUser", "RootDirectory"],
        // A MOUNT TARGET is placed where its create placed it: the file system it serves, the
        // subnet it is in and the address it answers at are members of `CreateMountTarget` and
        // of no call that changes one — `ModifyMountTargetSecurityGroups` takes the security
        // groups and nothing else — so a declaration restating one of them asks for a
        // DIFFERENT mount target.
        EFS_MOUNT_TARGET => &["FileSystemId", "IpAddress", "SubnetId"],
        // A session is encrypted under the key it was created with: `encryptionKeyArn` is a
        // member of `CreateSession` and of no member of `UpdateSession` (which takes a
        // session's identifier and its metadata alone), so a declaration naming another key
        // asks for a different session.
        BEDROCK_SESSION => &["EncryptionKeyArn"],
        // An application inference profile keeps everything its create made it with: Amazon
        // Bedrock publishes no update of one at all — `CreateInferenceProfile` takes the
        // profile's name, its description and the model source it copies from, and the only
        // calls on a profile afterwards are its read, its delete and the tagging calls — so a
        // declaration restating any of them asks for a DIFFERENT profile, which CloudFormation
        // replaces. (Its NAME is one of them, which is why it is listed here rather than left
        // to the name convention.)
        BEDROCK_APPLICATION_INFERENCE_PROFILE => {
            &["Description", "InferenceProfileName", "ModelSource"]
        }
        // A trigger keeps the TYPE it fires on, the WORKFLOW it belongs to and whether its
        // create started it: all three are members of `CreateTrigger` and of no member of
        // `TriggerUpdate` — which takes a trigger's description, schedule, actions, predicate
        // and event batching condition and nothing else
        // (`smithy_query(query="shape",shape="TriggerUpdate")`) — so a declaration restating
        // one of them asks for a DIFFERENT trigger, which CloudFormation replaces. (Its NAME
        // is in that position through [`name_is_create_only`], and every other property of the
        // type is a member of that update.)
        crate::state::provisioner::glue_triggers::GLUE_TRIGGER => {
            crate::state::provisioner::glue_triggers::GLUE_TRIGGER_CREATE_ONLY_PROPERTIES
        }
        // A spending limit is a cost control ABOUT one device: `UpdateSpendingLimit` takes
        // the limit's amount and its period and no device at all, so a declaration naming
        // another device asks for a different limit.
        BRAKET_SPENDING_LIMIT => &["DeviceArn"],
        // An investigation group keeps the RETENTION its create fixed: `retentionInDays` is a
        // member of `CreateInvestigationGroup` and of no member of `UpdateInvestigationGroup`
        // — which takes the group's role, encryption configuration, tag key boundaries, chat
        // channels, CloudTrail flag and cross-account configurations and nothing else
        // (`smithy_query(query="operation",operation="UpdateInvestigationGroup")`) — so a
        // declaration restating it asks for a DIFFERENT group. (Its `Name` is in that same
        // position through [`name_is_create_only`], that call taking no name either.)
        AIOPS_INVESTIGATION_GROUP => &["RetentionInDays"],
        // A custom game build keeps what its create made it with: the operating system its
        // binaries run on, the server SDK version they were integrated with and the Amazon S3
        // location the files were read from are members of `CreateBuild` and of no member of
        // `UpdateBuild` — which takes a build's id with a new name and version and nothing
        // else — so a declaration restating one of them asks for a DIFFERENT build, which
        // CloudFormation replaces.
        GAMELIFT_BUILD => &["OperatingSystem", "ServerSdkVersion", "StorageLocation"],
        // A Realtime script keeps the runtime it was created on: `NodeJsVersion`'s own doc is
        // "This value cannot be updated later", and `UpdateScript` takes a script's id with a
        // new name, version and content source — so a declaration restating the runtime asks
        // for a different script. (Its `Name` is renamable in place through
        // [`name_is_create_only`], as a build's is.)
        GAMELIFT_SCRIPT => &["NodeJsVersion"],
        // Amazon VPC Lattice publishes an update for a resource gateway's SECURITY GROUPS
        // (`UpdateResourceGateway` takes `securityGroupIds` and nothing else) and for a
        // configuration's port ranges and allowed sharing (`UpdateResourceConfiguration`), so
        // every OTHER property of either type is one its create alone takes: a declaration
        // restating one asks for a different resource, which CloudFormation replaces. An
        // ASSOCIATION is the binding between two resources, so every side of it is
        // create-only; the VPC association's security groups are `UpdateServiceNetworkVpc\
        // Association`'s own member and so reach it in place. (Each type's `Name`, where it
        // has one, is in that position through [`name_is_create_only`].)
        VPCLATTICE_RESOURCE_GATEWAY => &["IpAddressType", "SubnetIds", "VpcIdentifier"],
        VPCLATTICE_RESOURCE_CONFIGURATION => &[
            "ResourceConfigurationDefinition",
            "ResourceConfigurationGroupId",
            "ResourceConfigurationType",
            "ResourceGatewayId",
        ],
        VPCLATTICE_SERVICE_NETWORK_RESOURCE_ASSOCIATION => {
            &["ResourceConfigurationId", "ServiceNetworkId"]
        }
        VPCLATTICE_SERVICE_NETWORK_VPC_ASSOCIATION => {
            &["ServiceNetworkIdentifier", "VpcIdentifier"]
        }
        // An AWS HealthLake FHIR data store keeps what its create made it with: the FHIR
        // release version it hosts, the Synthea data it was preloaded with and the key it is
        // encrypted under are members of `CreateFHIRDatastore` and of no member of
        // `UpdateFHIRDatastore` — which takes a store's id with its name and its analytics,
        // NLP, profile, identity-provider and backup configurations and nothing else
        // (`smithy_query(query="operation",operation="CreateFHIRDatastore,\
        // UpdateFHIRDatastore")`) — so a declaration restating one of them asks for a
        // DIFFERENT data store, which CloudFormation replaces.
        HEALTHLAKE_FHIR_DATASTORE => &[
            "DatastoreTypeVersion",
            "PreloadDataConfig",
            "SseConfiguration",
        ],
        // A data transformation profile keeps what it was created with too: its name, the
        // source format it converts from, the source its DRAFT content came from and the key
        // that content is encrypted at rest under are members of
        // `CreateDataTransformationProfile` and of no member of
        // `UpdateDataTransformationProfile`, which takes a profile's id with a new mapping and
        // a change description alone — so a declaration restating any of them asks for a
        // DIFFERENT profile.
        HEALTHLAKE_DATA_TRANSFORMATION_PROFILE => {
            &["KmsKeyId", "ProfileName", "Source", "SourceFormat"]
        }
        // An Amazon SageMaker AI MODEL keeps everything `CreateModel` made it with: that
        // service publishes no call that changes a model — its operations on one are
        // `DescribeModel`, `DeleteModel` and the tagging calls alone
        // (`smithy_query(query="list_operations")` on SageMaker's model lists no `UpdateModel`)
        // — so a declaration restating any of its properties asks for a DIFFERENT model, which
        // CloudFormation replaces: it creates the model the new declaration asks for, publishes
        // that name through `Ref` and the stack's outputs, and deletes the one it superseded.
        // Re-pointing a model at a newly trained artifact (`PrimaryContainer.ModelDataUrl`) is
        // that update. (Its `ModelName` is in this position through [`name_is_create_only`],
        // and its `Tags` are deliberately absent: SageMaker changes a resource's tags in place
        // through its own `AddTags`/`DeleteTags`.)
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL => &[
            "Containers",
            "EnableNetworkIsolation",
            "ExecutionRoleArn",
            "InferenceExecutionConfig",
            "PrimaryContainer",
            "VpcConfig",
        ],
        // An ENDPOINT CONFIGURATION is in exactly that position: `CreateEndpointConfig`
        // registers the variants and the settings an endpoint is then deployed from, and
        // SageMaker publishes no call that edits a configuration that exists (the same listing
        // shows no `UpdateEndpointConfig`) — an endpoint is moved to ANOTHER configuration
        // instead. So a declaration restating any of these asks for a different configuration,
        // which CloudFormation replaces, under the same two exceptions as the model's.
        //
        // The model registry's GROUP (`AWS::SageMaker::ModelPackageGroup`) is the one SageMaker
        // type these arms create that this rule covers and this catalogue deliberately leaves
        // out: that service publishes no call that changes a group either, but the pass that
        // gives a replacement a physical name of its own
        // ([`crate::state::replacing_update::rename_replacements`]) does not rename this type,
        // so the deployment would have nothing to create the replacement under and the update
        // would report a different refusal rather than being carried out. A row here without
        // that half would change which refusal a client reads and nothing else.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_ENDPOINT_CONFIG => &[
            "AsyncInferenceConfig",
            "DataCaptureConfig",
            "EnableNetworkIsolation",
            "ExecutionRoleArn",
            "ExplainerConfig",
            "KmsKeyId",
            "ProductionVariants",
            "ShadowProductionVariants",
            "VpcConfig",
        ],
        // A VPC Lattice service network keeps the SHARING setting it was created with:
        // `UpdateServiceNetwork` takes the network's `authType` and nothing else, so a
        // declaration restating the sharing configuration asks for a different network. (Its
        // `Name` is in that position through [`name_is_create_only`], and its `Tags` reach the
        // network through that service's own `TagResource`/`UntagResource`.)
        VPCLATTICE_SERVICE_NETWORK => &["SharingConfig"],
        // An access log subscription keeps the resource it MONITORS and the log type it
        // monitors it for: `UpdateAccessLogSubscription` takes the destination and nothing
        // else, so a declaration restating either asks for a different subscription.
        VPCLATTICE_ACCESS_LOG_SUBSCRIPTION => &["ResourceIdentifier", "ServiceNetworkLogType"],
        // A Quick Setup configuration manager keeps the DEFINITIONS it was created with:
        // `UpdateConfigurationDefinition` addresses ONE definition by the `Id` Quick Setup
        // minted for it — a value no template states — and takes neither the definition's
        // `Type` nor the list itself, so that service publishes no call that adds a
        // definition, removes one or re-types one. A declaration restating the list therefore
        // asks for a DIFFERENT manager, which CloudFormation replaces. (Its `Name` and
        // `Description` are `UpdateConfigurationManager`'s own members, so a change to either
        // reaches the manager in place — the `Name` through [`name_is_create_only`], which
        // excludes this type — and its `Tags` reach it through that service's own
        // `TagResource`/`UntagResource`.)
        SSM_QUICKSETUP_CONFIGURATION_MANAGER => &["ConfigurationDefinitions"],
        // A Direct Connect gateway keeps the Amazon-side ASN it was created with:
        // `amazonSideAsn` is a member of `CreateDirectConnectGateway` and of no member of
        // `UpdateDirectConnectGateway`, which "updates the NAME of a current Direct Connect
        // gateway" and takes nothing else — so a declaration naming another ASN asks for a
        // different gateway, which CloudFormation replaces.
        //
        // Its `Tags` are here for the reason a training dataset's are rather than as an
        // exception: that service does change a gateway's tags in place, through its own
        // `TagResource`/`UntagResource`, but no arm of [`crate::state::provisioner`] calls them
        // for this type yet — so a changed tag set is applied by replacing the gateway, whose
        // create carries the tags, rather than reported as applied by nothing. A tag arm
        // retires this entry.
        DIRECT_CONNECT_GATEWAY => &["AmazonSideAsn", "Tags"],
        // A resiliency group keeps the resiliency model it was created to meet:
        // `intendedResiliencyModel` is a member of `CreateResiliencyGroup` and of no member of
        // `UpdateResiliencyGroup`, which takes the group's id and a new name alone — so a
        // declaration naming another model asks for a different group. Its `Tags` are here for
        // the same reason the gateway's are.
        DIRECT_CONNECT_RESILIENCY_GROUP => &["IntendedResiliencyModel", "Tags"],
        // Everything about a Directory Service directory but its single sign-on flag is fixed
        // when that service makes it: the fully qualified name, the NetBIOS short name, the
        // administrator password, the placement and the edition or size are all members of
        // `CreateMicrosoftAD`/`CreateDirectory`, and that service publishes no call that
        // changes any of them on a directory that exists — so a declaration restating one asks
        // for a DIFFERENT directory, which CloudFormation replaces. `CreateAlias` is here for
        // the same reason: an alias "cannot be deleted or reused", so asking for one on a
        // directory that already carries its own is asking for another directory. (The `Name`
        // is in this position through [`name_is_create_only`] as well. `EnableSso` is
        // deliberately absent: `EnableSso`/`DisableSso` turn the flag over in place, which is
        // what [`crate::state::provisioner`] applies for a changed declaration of it.)
        DIRECTORY_SERVICE_MICROSOFT_AD => &[
            "CreateAlias",
            "Edition",
            "Password",
            "ShortName",
            "VpcSettings",
        ],
        DIRECTORY_SERVICE_SIMPLE_AD => &[
            "CreateAlias",
            "Description",
            "Password",
            "ShortName",
            "Size",
            "VpcSettings",
        ],
        // An Application Signals instrumentation configuration keeps everything it was created
        // with: "configurations are immutable after creation"
        // (`CreateInstrumentationConfiguration`), and that service publishes no call that
        // changes one at all — so a declaration restating any property asks for a DIFFERENT
        // configuration, which CloudFormation replaces. Its own four identity dimensions (the
        // service, the environment, the signal type and the location) are among them for the
        // same reason every other property is: the record the create wrote is the whole of the
        // resource. (A GROUPING CONFIGURATION needs no entry: `PutGroupingConfiguration` is
        // documented "Creates or updates the grouping configuration for this account", so a
        // changed declaration reaches the configuration in place. An SLO needs none either:
        // every property of it beside its name is a member of `UpdateServiceLevelObjective`.)
        APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG => &[
            "AttributeFilters",
            "CaptureConfiguration",
            "Description",
            "Environment",
            "ExpiresAt",
            "InstrumentationType",
            "Location",
            "Service",
            "SignalType",
            "Tags",
        ],
        // Clean Rooms ML publishes no call that changes a training dataset at all: the
        // dataset's description, the role it reads its data with and the datasets themselves
        // are all fixed when `CreateTrainingDataset` records it, so a declaration restating one
        // of them asks for a different dataset. (Its NAME is in the same position through
        // [`name_is_create_only`].) Its `Tags` are here for the same reason rather than as an
        // exception: that service does change a dataset's tags in place, through its own
        // `TagResource`/`UntagResource`, but no arm of [`crate::state::provisioner`] calls
        // them for this type yet — so a changed tag set is applied by replacing the dataset,
        // whose create carries the tags, rather than reported as applied by nothing. A tag arm
        // retires this entry.
        CLEANROOMSML_TRAINING_DATASET => &["Description", "RoleArn", "Tags", "TrainingData"],
        // Everything a CloudHSM cluster is made with is fixed once that service has placed it:
        // the subnets, the mode and the HSM type are all members of `CreateCluster`, and the one
        // call that changes a cluster afterwards (`ModifyCluster`) takes the HSM type alone and
        // no placement at all — so a declaration restating any of them asks for a DIFFERENT
        // cluster, which CloudFormation replaces. Its `Tags` are here for the reason a training
        // dataset's are: that service does change a cluster's tags in place, through its own
        // `TagResource`/`UntagResource`, but no arm of [`crate::state::provisioner`] calls them
        // for this type yet — so a changed tag set is applied by replacing the cluster, whose
        // create carries the tags, rather than reported as applied by nothing. A tag arm retires
        // that entry.
        CLOUDHSM_CLUSTER => &[
            "BackupRetentionPolicy",
            "HsmType",
            "Mode",
            "NetworkType",
            "SourceBackupId",
            "SubnetIds",
            "Tags",
        ],
        // A hypervisor keeps the encryption key it was imported with: `KmsKeyArn` is a member
        // of `ImportHypervisorConfiguration` and of no member of `UpdateHypervisor` (which
        // takes the host, the credentials, the name and the log group), so a declaration
        // restating it asks for a different hypervisor.
        BACKUP_GATEWAY_HYPERVISOR => &["KmsKeyArn"],
        // An analyzer keeps the KIND it was created as: `type` is a member of `CreateAnalyzer`
        // ("the type of analyzer to create", which fixes the zone of trust it monitors) and of
        // no member of `UpdateAnalyzer`, whose one member beside the analyzer's name is the
        // configuration — so a declaration naming another kind asks for a different analyzer,
        // which CloudFormation replaces. (Its NAME is in that position through
        // [`name_is_create_only`]: no call of that service renames an analyzer. An ARCHIVE
        // RULE's name is too, `UpdateArchiveRule` addressing a rule BY that name, and its
        // criteria are the one property that call can restate.)
        crate::state::provisioner::access_analyzer::ACCESSANALYZER_ANALYZER => &["Type"],
        // An archive rule belongs to the analyzer it was created on: `analyzerName` is the URI
        // label of `CreateArchiveRule`, of `UpdateArchiveRule` and of `DeleteArchiveRule`
        // alike, so no call of that service moves a rule to another analyzer and a declaration
        // naming one asks for a different rule.
        crate::state::provisioner::access_analyzer::ACCESSANALYZER_ARCHIVE_RULE => {
            &[crate::state::provisioner::access_analyzer::ANALYZER_NAME_PROPERTY]
        }
        // A retention rule retains the ONE resource type it was created for: `ResourceType` is
        // a member of `CreateRule`, and `UpdateRule`'s member of that name is documented
        // "currently not supported. You can't update a retention rule's resource type after
        // creation" — so a declaration naming another type asks for a different rule.
        RBIN_RULE => &["ResourceType"],
        // A lifecycle policy keeps the KIND of default policy it was created as: `DefaultPolicy`
        // is a member of `CreateLifecyclePolicy` ("Specify the type of default policy to
        // create") and of no member of `UpdateLifecyclePolicy`, whose `PolicyDetails` is
        // documented "You cannot update the policy type or the resource type" — so a
        // declaration naming another kind asks for a different policy.
        DLM_LIFECYCLE_POLICY => &["DefaultPolicy"],
        // Each AWS Network Security Manager entity keeps the FIREWALL TYPE it was created for:
        // `firewallType` is a member of `CreateRule`, `CreateTemplate` and `CreatePolicy`, and
        // of no member of the matching `Update…` (each takes the entity's description and the
        // configuration or associations it enforces, and no firewall type —
        // `smithy_query(query="operation",operation="CreateRule,UpdateRule,CreateTemplate,\
        // UpdateTemplate,CreatePolicy,UpdatePolicy,CreateScope,UpdateScope")`) — so a
        // declaration naming another firewall type asks for a different entity, which
        // CloudFormation replaces. (Each entity's NAME is in that position through
        // [`name_is_create_only`]: no call of that service renames one. A SCOPE declares no
        // firewall type at all, so its only create-only property beside its name is the tag
        // set below.)
        //
        // Their `Tags` are here for the same reason a training dataset's are rather than as an
        // exception: that service does change an entity's tags in place, through its own
        // `TagResource`/`UntagResource`, but no arm of [`crate::state::provisioner`] calls them
        // for these types yet — so a changed tag set is applied by replacing the entity, whose
        // create carries the tags, rather than reported as applied by nothing. A tag arm
        // retires these entries.
        NETWORK_SECURITY_MANAGER_RULE
        | NETWORK_SECURITY_MANAGER_TEMPLATE
        | NETWORK_SECURITY_MANAGER_POLICY => &["FirewallType", "Tags"],
        NETWORK_SECURITY_MANAGER_SCOPE => &["Tags"],
        // A Neptune Analytics graph keeps the replicas, the encryption key and the vector
        // index it was made with: each is a member of `CreateGraph` and of no member of
        // `UpdateGraph` (which takes the provisioned memory and the two flags), so a
        // declaration restating one asks for a different graph. (Its NAME is in the same
        // position through [`name_is_create_only`]: no call of that service renames a graph.)
        NEPTUNE_GRAPH_GRAPH => &[
            "KmsKeyIdentifier",
            "ReplicaCount",
            "VectorSearchConfiguration",
        ],
        // A snapshot is the capture of ONE graph: `graphIdentifier` is a member of
        // `CreateGraphSnapshot`, and that service publishes no call that changes a snapshot at
        // all, so a declaration naming another graph asks for a different snapshot.
        NEPTUNE_GRAPH_GRAPH_SNAPSHOT => &["GraphIdentifier"],
        // An Amazon Redshift Serverless NAMESPACE keeps the first database it was made with and
        // the IAM Identity Center application it was registered against: `dbName` and
        // `redshiftIdcApplicationArn` are members of `CreateNamespace` and of no member of
        // `UpdateNamespace` (which takes the admin credentials, the encryption keys, the IAM
        // roles and the log exports), so a declaration restating one asks for a different
        // namespace. Its `Tags` are here because that service's own `UntagResource` is not
        // served in this emulator, so a changed tag set is applied by replacing the namespace —
        // whose create carries the tags — rather than reported as applied by nothing; a tag arm
        // retires that entry. (Its NAME is create-only through [`name_is_create_only`]: no call
        // of that service renames a namespace.)
        crate::state::provisioner::redshift_serverless::REDSHIFT_SERVERLESS_NAMESPACE => {
            &["DbName", "RedshiftIdcApplicationArn", "Tags"]
        }
        // A WORKGROUP is the compute of ONE namespace: `namespaceName` is a member of
        // `CreateWorkgroup` and of no member of `UpdateWorkgroup`, and that service publishes no
        // call that moves a workgroup to another namespace, so a declaration naming another one
        // asks for a different workgroup. Its `Tags` are here for the reason the namespace's
        // are.
        crate::state::provisioner::redshift_serverless::REDSHIFT_SERVERLESS_WORKGROUP => {
            &["NamespaceName", "Tags"]
        }
        // A SNAPSHOT is the capture of ONE namespace, and nothing about it is changed in place
        // here: `namespaceName` is a member of `CreateSnapshot` alone, and that service's own
        // `UpdateSnapshot` — the one call that re-values a snapshot's retention — is not served
        // in this emulator, so a declaration restating the retention or the tag set is applied
        // by taking the snapshot again. An `UpdateSnapshot` arm retires the retention entry.
        crate::state::provisioner::redshift_serverless::REDSHIFT_SERVERLESS_SNAPSHOT => {
            &["NamespaceName", "RetentionPeriod", "Tags"]
        }
        // An Amazon AppFlow CONNECTOR PROFILE keeps the connector it is an instance of, that
        // connector's label and the key its credentials are encrypted with: `connectorType`,
        // `connectorLabel` and `kmsArn` are members of `CreateConnectorProfile` and of no member
        // of `UpdateConnectorProfile` (which takes the connection mode and the whole
        // `connectorProfileConfig`), so a declaration restating one asks for a different
        // profile. (Its NAME is create-only through [`name_is_create_only`]: no call of that
        // service renames a profile.)
        crate::state::provisioner::appflow_connector_profiles::APPFLOW_CONNECTOR_PROFILE => {
            &["ConnectorType", "ConnectorLabel", "KMSArn"]
        }
        // A Cloud Map SERVICE lives in the namespace its `NamespaceId` names, is of the kind
        // its `Type` states and has the custom health check its create configured: each is a
        // member of `CreateService` and of no member of `ServiceChange` (`UpdateService` takes
        // the description, the DNS records and the Route 53 health check), and that service
        // publishes no call that moves a service to another namespace — so a declaration
        // restating one of them asks for a DIFFERENT service, which CloudFormation replaces.
        crate::state::template::SERVICEDISCOVERY_SERVICE => {
            &["HealthCheckCustomConfig", "NamespaceId", "Type"]
        }
        // An INSTANCE is registered IN one service: `RegisterInstance$ServiceId` addresses the
        // instance rather than being a value of it, and Cloud Map publishes no call that moves
        // an instance to another service, so a declaration naming another service asks for a
        // different instance. (Its `InstanceId` is in the same position through
        // [`name_is_create_only`]: that id is what identifies the instance.)
        crate::state::template::SERVICEDISCOVERY_INSTANCE => {
            &[crate::state::template::SERVICEDISCOVERY_SERVICE_ID_PROPERTY]
        }
        // A PRIVATE DNS namespace's hosted zone is associated with the VPC its create named:
        // `Vpc` is a member of `CreatePrivateDnsNamespace` and of no member of
        // `PrivateDnsNamespaceChange` (that update takes the description and the SOA record), so
        // a declaration naming another VPC asks for a different namespace.
        crate::state::template::SERVICEDISCOVERY_PRIVATE_DNS_NAMESPACE => &["Vpc"],
        // A maintenance window TARGET belongs to the window it was registered with and is of
        // the kind the register stated: `WindowId` addresses the target rather than being a
        // value of it, and `ResourceType` is a member of the register and of no update
        // (`UpdateMaintenanceWindowTarget` takes the targets, the owner information, the name
        // and the description), so a declaration restating either asks for a different target.
        crate::state::provisioner::ssm_maintenance_windows::SSM_MAINTENANCE_WINDOW_TARGET => {
            &["ResourceType", "WindowId"]
        }
        // An Amplify UI Builder component, form or theme belongs to the app environment its
        // `AppId` and `EnvironmentName` name: both are URI labels of every call on one — the
        // create included — and that service publishes no call that moves an entity to another
        // app or environment, so a declaration restating either asks for a different entity.
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_FORM | AMPLIFYUIBUILDER_THEME => {
            &["AppId", "EnvironmentName"]
        }
        // An AWS Amplify BRANCH belongs to the app its `AppId` names: that id is the first URI
        // label of every call on a branch — the create included — and that service publishes no
        // call that moves a branch to another app, so a declaration restating it asks for a
        // different branch. (Its NAME is in the same position through [`name_is_create_only`]:
        // `UpdateBranch` addresses a branch BY its name and takes no new one.)
        AMPLIFY_BRANCH => &["AppId"],
        // An environment's encryption key and who manages its VPC endpoints are fixed when
        // Amazon MWAA makes it: both are members of `CreateEnvironment` and of no member of
        // `UpdateEnvironment`, so a declaration restating either asks for a different
        // environment.
        MWAA_ENVIRONMENT => &["EndpointManagement", "KmsKey"],
        // Each of the records a REST API is built out of is addressed BY the api that holds
        // it: `restApiId` is an `@httpLabel` of every call API Gateway publishes for a
        // resource, a method, a deployment and a stage — the create included — and that
        // service publishes no call that moves one to another api, so a declaration naming
        // another api asks for a DIFFERENT record, which is what that property's "Update
        // requires: Replacement" says. The rest of each row is the same reading of those
        // calls:
        //
        // * a RESOURCE is the path segment it adds under its parent, and the type publishes
        //   `ParentId` and `PathPart` with "Update requires: Replacement" too — every method
        //   on the resource and every deployment snapshot of the api was taken against the
        //   path those two make up, so a declaration restating either asks for a different
        //   resource;
        // * a METHOD is one verb on one resource: `resourceId` and `httpMethod` are the
        //   labels `UpdateMethod` addresses it by, and that call takes no new value for
        //   either;
        // * a DEPLOYMENT is an immutable snapshot released to one stage, and
        //   `UpdateDeployment`'s patch surface is its description alone — so the stage it
        //   releases to and the canary settings it is released with, both `CreateDeployment`
        //   members, are fixed when the snapshot is taken;
        // * a STAGE keeps the name `UpdateStage` addresses it by, which that call takes no
        //   new value for ([`name_is_create_only`] is what reads a stage's name in this
        //   position, so the row carries the api alone).
        //
        // Every other property of these types is one that patch call writes, so stating one
        // differently is a change to the record the stack already has
        // ([`crate::state::provisioning::apigateway_rest_api`]).
        APIGATEWAY_RESOURCE => &["ParentId", "PathPart", "RestApiId"],
        crate::state::provisioning::apigateway_rest_api::APIGATEWAY_METHOD => {
            &["HttpMethod", "ResourceId", "RestApiId"]
        }
        APIGATEWAY_DEPLOYMENT => &["DeploymentCanarySettings", "RestApiId", "StageName"],
        crate::state::provisioning::apigateway_rest_api::APIGATEWAY_STAGE => &["RestApiId"],
        // An api of that control plane is CLONED from another where its declaration names
        // one: `cloneFrom` is a member of `CreateRestApi` and of no member of the two calls
        // that change an api that exists — `UpdateRestApi`'s patch surface and `PutRestApi` —
        // so a declaration naming another api to clone asks for a different api. Everything
        // else about an api is what those two calls write.
        APIGATEWAY_REST_API => &["CloneFrom"],
        // An api's PROTOCOL is fixed when API Gateway v2 makes it: `ProtocolType` is a member
        // of `CreateApi` and of no member of `UpdateApi` — and no other call of that service
        // changes an api's protocol — so a declaration stating another protocol asks for a
        // DIFFERENT api, which is what that property's "Update requires: Replacement" says.
        // Every other property of the type is one that patch takes, so stating one
        // differently is a change to the api the stack already has
        // (`crate::state::provisioner::update_apigatewayv2_api`).
        APIGATEWAYV2_API => &["ProtocolType"],
        // An integration, a route, a stage, a deployment and an authorizer each exist INSIDE
        // the api their `ApiId` names, and API Gateway v2 publishes no call that moves one to
        // another api — every call on one takes the api beside the resource's own identifier —
        // so a declaration naming another api asks for a different resource, which is what
        // that property's "Update requires: Replacement" says. A DEPLOYMENT is a snapshot
        // released to one stage besides: `CreateDeployment` takes that stage and
        // `UpdateDeployment` takes the description alone
        // (`crate::state::provisioner::ApiGatewayV2Child::create_only`).
        APIGATEWAYV2_INTEGRATION
        | APIGATEWAYV2_ROUTE
        | APIGATEWAYV2_STAGE
        | APIGATEWAYV2_AUTHORIZER => &["ApiId"],
        APIGATEWAYV2_DEPLOYMENT => &["ApiId", "StageName"],
        // An INTEGRATION RESPONSE lives inside one integration of one api and a ROUTE
        // RESPONSE inside one route of one api: both labels address every call on the
        // resource, the create included, and API Gateway v2 publishes no call that moves one
        // to another integration, route or api — so a declaration naming another asks for a
        // different resource. A MODEL lives in its api the same way. Every other property of
        // the three is a member of their own patches
        // (`UpdateIntegrationResponse`, `UpdateRouteResponse`, `UpdateModel`), so stating one
        // differently is a change to the resource the stack already has.
        APIGATEWAYV2_INTEGRATION_RESPONSE => &["ApiId", "IntegrationId"],
        APIGATEWAYV2_ROUTE_RESPONSE => &["ApiId", "RouteId"],
        APIGATEWAYV2_MODEL => &["ApiId"],
        // A ROUTING RULE exists on the custom domain name its `DomainNameArn` names, and API
        // Gateway v2 publishes no call that moves one to another domain — the domain is an
        // `@httpLabel` of every call on a rule, the create included — so a declaration naming
        // another domain asks for a different rule, which is what that property's "Update
        // requires: Replacement" says. Every other property of the type is a member of
        // `PutRoutingRule`, so stating one differently is a change to the rule the stack
        // already has (`crate::state::provisioner::update_apigatewayv2_routing_rule`).
        APIGATEWAYV2_ROUTING_RULE => &[crate::state::provisioner::ROUTING_RULE_DOMAIN_NAME_ARN],
        // An AppSync custom domain is served with the certificate it was created with:
        // `certificateArn` is a member of `CreateDomainName`, `UpdateDomainName` takes the
        // description alone, and no other call of that service re-certifies a domain — so a
        // declaration naming another certificate asks for a different domain, which the
        // deployment replaces. (The MAPPING of an API onto a domain has none beyond its own
        // identity: its `ApiId` is what `AssociateApi` changes in place.)
        crate::state::provisioning::appsync_domain_names::APPSYNC_DOMAIN_NAME => {
            &["CertificateArn"]
        }
        // Every child of an AppSync GraphQL API lives UNDER the API its `ApiId` names: that id
        // is the first URI label of every call on the child — the create included — and that
        // service publishes no call that moves one to another API, so a declaration restating
        // it asks for a DIFFERENT resource. A RESOLVER is attached to one field of one type,
        // which `UpdateResolver` addresses it by and takes no new value for, so restating
        // either asks for a different resolver too. (A data source's NAME is in that same
        // position through [`name_is_create_only`]: `UpdateDataSource` addresses a data source
        // BY its name and takes no new one.)
        APPSYNC_GRAPHQL_SCHEMA
        | APPSYNC_DATA_SOURCE
        | APPSYNC_FUNCTION_CONFIGURATION
        | APPSYNC_API_KEY => &[APPSYNC_API_ID_PROPERTY],
        APPSYNC_RESOLVER => &[
            APPSYNC_API_ID_PROPERTY,
            APPSYNC_TYPE_NAME_PROPERTY,
            APPSYNC_FIELD_NAME_PROPERTY,
        ],
        // A GraphQL API is of the KIND it was created as and is reachable from where its
        // create said: `apiType` and `visibility` are members of `CreateGraphqlApi` and of no
        // member of `UpdateGraphqlApi`, so a declaration restating either asks for a different
        // API. (Its NAME is not one of them: that update takes a new one — see
        // [`name_is_create_only`].)
        APPSYNC_GRAPHQL_API => &["ApiType", "Visibility"],
        // An AppConfig environment and configuration profile belong to the application their
        // `ApplicationId` names, and a profile reads its configuration from the source its
        // `LocationUri` and `Type` fix: each is a member of that type's create and of no
        // member of its update (`UpdateConfigurationProfile` takes neither `LocationUri` nor
        // `Type`), so a declaration restating one asks for a different resource. A deployment
        // strategy's `ReplicateTo` is the same case against `UpdateDeploymentStrategy`.
        APPCONFIG_ENVIRONMENT => &["ApplicationId"],
        APPCONFIG_CONFIGURATION_PROFILE => &["ApplicationId", "LocationUri", "Type"],
        APPCONFIG_DEPLOYMENT_STRATEGY => &["ReplicateTo"],
        // A hosted configuration VERSION is its content under the number AppConfig minted, and a
        // DEPLOYMENT is one rollout of one version into one environment: that service publishes
        // no call that edits either — `CreateHostedConfigurationVersion` and `StartDeployment`
        // are the only calls that make one, and the only other call on an existing deployment
        // stops it — so every property of both types is create-only and a declaration restating
        // any of them asks for a different resource, which the deployment replaces. The parts of
        // each type's composite identity are in that position for the second reason as well.
        APPCONFIG_HOSTED_CONFIGURATION_VERSION => &[
            "ApplicationId",
            "ConfigurationProfileId",
            "Content",
            "ContentType",
            "Description",
            "LatestVersionNumber",
            "VersionLabel",
        ],
        APPCONFIG_DEPLOYMENT => &[
            "ApplicationId",
            "ConfigurationProfileId",
            "ConfigurationVersion",
            "DeploymentStrategyId",
            "Description",
            "DynamicExtensionParameters",
            "EnvironmentId",
            "KmsKeyIdentifier",
        ],
        // A grant on a private CA is made WHOLE and cannot be edited: Amazon Web Services
        // Private CA publishes no call that changes one — `CreatePermission` refuses a
        // request that re-asserts the same CA, principal and account for another action set,
        // and the only way to a different grant is to revoke and grant again — so every
        // property of the type is create-only and a declaration restating any of them asks
        // for a different resource, which the deployment replaces. The two that are also the
        // parts of its composite identity are in that position for the second reason as well.
        ACMPCA_PERMISSION => &["Actions", "CertificateAuthorityArn", "Principal", "SourceAccount"],
        // A State Manager association may be paired with ONE node by the deprecated
        // `InstanceId`, which `CreateAssociation` takes and `UpdateAssociation` does not: the
        // node an association was created for cannot be changed on it, so a declaration
        // naming another node asks for a different association.
        SSM_ASSOCIATION => &["InstanceId"],
        // The operating system a PATCH BASELINE applies to is fixed when Systems Manager makes
        // it: `OperatingSystem` is a member of `CreatePatchBaseline` and of no member of
        // `UpdatePatchBaseline`, so a declaration restating it asks for a different baseline —
        // which CloudFormation replaces rather than updates.
        crate::state::provisioning::ssm_patch_baseline::SSM_PATCH_BASELINE => {
            &["OperatingSystem"]
        }
        // The event source an AWS Lambda EVENT SOURCE MAPPING reads, and how it reads it, are
        // fixed when Lambda makes the mapping: each is a member of
        // `CreateEventSourceMapping` and of no member of `UpdateEventSourceMapping`, and that
        // service publishes no call that re-points a mapping at another source — so a
        // declaration restating one of them asks for a DIFFERENT mapping, which
        // CloudFormation replaces rather than updates. Every other property of the type is a
        // member of that update, so stating one differently is a change to the mapping the
        // stack already has
        // ([`crate::state::provisioning::lambda_event_source_mapping::update`]). The type
        // declares no name, so [`name_is_create_only`] has nothing to catch here.
        crate::state::provisioning::lambda_event_source_mapping::LAMBDA_EVENT_SOURCE_MAPPING => {
            crate::state::provisioning::lambda_event_source_mapping::CREATE_ONLY_PROPERTIES
        }
        // A LAYER VERSION cannot be changed at all: AWS Lambda publishes no call that edits
        // one, and "each time you call PublishLayerVersion with the same layer name, a new
        // version is created" — so every property of the type is create-only and a declaration
        // restating any of them asks for a DIFFERENT version, which CloudFormation replaces by
        // publishing one ([`crate::state::provisioning::lambda_layer_version`]). Its
        // `LayerName` is among them for the same reason: the version is published UNDER that
        // layer, and no call moves one to another layer.
        crate::state::provisioning::lambda_layer_version::LAMBDA_LAYER_VERSION => {
            crate::state::provisioning::lambda_layer_version::CREATE_ONLY_PROPERTIES
        }
        // An event bus is created FOR a partner source or for nothing, and EventBridge
        // publishes no call that re-points one: `EventSourceName` is a member of
        // `CreateEventBus` and of no member of `UpdateEventBus`, so a declaration restating it
        // asks for a different bus.
        EVENTS_EVENT_BUS => &["EventSourceName"],
        // An event bus's resource policy statement is installed ON one bus under one
        // statement id: `PutPermission` takes both and EventBridge publishes no call that
        // moves a statement to another bus or renames one, so a declaration restating either
        // asks for a different grant — which CloudFormation replaces rather than updates.
        EVENTS_EVENT_BUS_POLICY => &["EventBusName", "StatementId"],
        // A document's TYPE is fixed when Systems Manager makes it: `DocumentType` is a member
        // of `CreateDocument` and of no member of `UpdateDocument` — no call of that service
        // turns a command document into a runbook — and so is `Requires`, so a declaration
        // restating either asks for a different document. Every other property is a member of
        // that update, so stating one differently is a change to the document the stack
        // already has (`crate::state::provisioner::update_ssm_document`).
        crate::state::provisioner::ssm_documents::SSM_DOCUMENT => {
            &["DocumentType", "Requires"]
        }
        // An Amazon Cognito user pool's sign-in aliases and its username attributes and their
        // case sensitivity are fixed when the pool is made: each is a member of
        // `CreateUserPool` and of no member of `UpdateUserPool`, so a declaration restating
        // one asks for a different pool. (Its `Schema` is NOT one of them: the schema is
        // append-only on the pool that exists, and `AddCustomAttributes` is the call that adds
        // to it — `crate::state::provisioning::cognito_user_pool`.) An app client's secret
        // generation is the same case against `UpdateUserPoolClient`, and an identity
        // provider's type against `UpdateIdentityProvider`.
        //
        // The POOL each of those three belongs to is not among them, even though no update
        // call moves a resource between pools: a declaration naming another pool is reported
        // as a change that was NOT APPLIED, by the arm that would have made the call
        // (`cognito_user_pool`'s own `create_only_properties`), which is the same reading an
        // identity-pool role attachment moved to another pool gets. A resource server's
        // identifier and an identity provider's name are those resources' identities, which
        // `name_is_create_only` already covers.
        COGNITO_USER_POOL => &["AliasAttributes", "UsernameAttributes", "UsernameConfiguration"],
        COGNITO_USER_POOL_CLIENT => &["GenerateSecret"],
        COGNITO_USER_POOL_IDENTITY_PROVIDER => &["ProviderType"],
        // A rotation schedule IS the rotation configured on the secret its `SecretId` names:
        // `RotateSecret` takes that secret's id and there is no call that moves a rotation to
        // another secret, so a declaration naming another secret asks for a different
        // resource — which is what that property's "Update requires: Replacement" says.
        SECRETSMANAGER_ROTATION_SCHEDULE => &["SecretId"],
        // A resource POLICY is the same case: `PutResourcePolicy` takes the secret's id and
        // there is no call that moves a policy to another secret, so a declaration naming
        // another secret asks for a different resource.
        SECRETSMANAGER_RESOURCE_POLICY => &["SecretId"],
        // A log stream, a metric filter and a subscription filter each exist INSIDE the log
        // group their `LogGroupName` names, and Amazon CloudWatch Logs publishes no call that
        // moves one to another group — every call on one takes the group beside the child's
        // own name — so a declaration naming another group asks for a different resource,
        // which is what that property's "Update requires: Replacement" says.
        crate::state::provisioning::logs_resources::LOGS_LOG_STREAM
        | crate::state::provisioning::logs_resources::LOGS_METRIC_FILTER
        | crate::state::provisioning::logs_resources::LOGS_SUBSCRIPTION_FILTER => {
            &["LogGroupName"]
        }
        // An Amazon ECS task-definition REVISION is immutable: that service publishes no call
        // that edits one — `RegisterTaskDefinition` is the only way to state a configuration,
        // and it makes a NEW revision — so every property of the type carries "Update
        // requires: Replacement", and a declaration that states any of them differently asks
        // for a new revision rather than a change to the one the stack has.
        ECS_TASK_DEFINITION => &[
            "ContainerDefinitions",
            "Cpu",
            "EnableFaultInjection",
            "EphemeralStorage",
            "ExecutionRoleArn",
            "Family",
            "InferenceAccelerators",
            "IpcMode",
            "Memory",
            "NetworkMode",
            "PidMode",
            "PlacementConstraints",
            "ProxyConfiguration",
            "RequiresCompatibilities",
            "RuntimePlatform",
            "TaskRoleArn",
            "Volumes",
        ],
        // An AWS Batch job-definition REVISION is immutable in the same way: that service
        // publishes no call that edits one — `RegisterJobDefinition` is the only way to state
        // a configuration, and it makes a NEW revision under the name — so a declaration that
        // states any of these differently asks for a new revision, with its own ARN, rather
        // than a change to the one the stack has.
        BATCH_JOB_DEFINITION => &[
            "ConsumableResourceProperties",
            "ContainerProperties",
            "EcsProperties",
            "EksProperties",
            "NodeProperties",
            "Parameters",
            "PlatformCapabilities",
            "PropagateTags",
            "RetryStrategy",
            "SchedulingPriority",
            "Timeout",
            "Type",
        ],
        // An Amazon RDS DB INSTANCE is created with the database it hosts, the character sets
        // it stores text in, the master user it is reached as, whether its storage is
        // encrypted and under which key, the time zone its engine runs in, the system
        // identifier an Oracle instance carries, the cluster it joined and what it was created
        // FROM: each is a member of `CreateDBInstance` (or of the restore and read-replica
        // creates) and of no member of `ModifyDBInstance` — Amazon RDS publishes no call that
        // changes any of them on the instance that exists, which is why the update walk sends
        // none of them (`DB_INSTANCE_UPDATE_PROPERTIES_NOT_SENT`) — so a declaration restating
        // one asks for a DIFFERENT DB instance, which is what each property's "Update
        // requires: Replacement" says. Every other property of the type is one that
        // modification takes, so stating it differently is a change to the instance the stack
        // already has; `AvailabilityZone` is left out deliberately, since the resource
        // reference publishes it as an update with "some interruptions" rather than as a
        // replacement.
        RDS_DB_INSTANCE_TYPE => &[
            "CharacterSetName",
            "DBClusterIdentifier",
            "DBName",
            "DBSnapshotIdentifier",
            "DBSystemId",
            "KmsKeyId",
            "MasterUsername",
            "NcharCharacterSetName",
            "SourceDBInstanceIdentifier",
            "SourceDbiResourceId",
            "SourceRegion",
            "StorageEncrypted",
            "Timezone",
        ],
        // ---------------------------------------------------------------------------
        // Amazon EC2
        // ---------------------------------------------------------------------------
        //
        // Source for every row below: AWS's CloudFormation resource reference for the type —
        // the properties it publishes with "Update requires: `Replacement`" — read against the
        // calls Amazon EC2 publishes for changing a resource that exists
        // (`crate::state::provisioner::ec2_in_place`,
        // `crate::state::provisioning::ec2_vpc_network`). Each is a value fixed when that
        // service makes the resource: no call of it changes the block a VPC or a subnet is
        // addressed on, the zone a subnet is in, the VPC a subnet, a route table, a security
        // group or an endpoint belongs to, the description or the name a security group was
        // created with, the service an endpoint reaches, or the AMI, the key pair and the
        // placement an instance was launched with. So a declaration restating one asks for a
        // DIFFERENT resource, which CloudFormation replaces; every property NOT named here is
        // one the owning service takes a new value for on the resource that exists, which the
        // deployment applies in place.
        EC2_VPC => &["CidrBlock", "Ipv4IpamPoolId", "Ipv4NetmaskLength"],
        EC2_SUBNET => &[
            "AvailabilityZone",
            "AvailabilityZoneId",
            "CidrBlock",
            "Ipv4IpamPoolId",
            "Ipv4NetmaskLength",
            "Ipv6CidrBlock",
            "Ipv6CidrBlocks",
            "Ipv6IpamPoolId",
            "Ipv6Native",
            "Ipv6NetmaskLength",
            "OutpostArn",
            "VpcId",
        ],
        EC2_SECURITY_GROUP => &["GroupDescription", "GroupName", "VpcId"],
        EC2_ROUTE_TABLE => &["VpcId"],
        EC2_VPN_GATEWAY => &["AmazonSideAsn", "Type"],
        EC2_NAT_GATEWAY => &[
            "AllocationId",
            "ConnectivityType",
            "PrivateIpAddress",
            "SubnetId",
        ],
        EC2_EIP => &[
            "Address",
            "Domain",
            "IpamPoolId",
            "NetworkBorderGroup",
            "PublicIpv4Pool",
            "TransferAddress",
        ],
        EC2_VPC_ENDPOINT => &["ServiceName", "VpcEndpointType", "VpcId"],
        // A CLIENT VPN ENDPOINT's three create-only properties: "the address range cannot be
        // changed after the Client VPN endpoint has been created"
        // (`CreateClientVpnEndpointRequest$ClientCidrBlock`), and neither the authentication
        // options an endpoint checks clients against nor the transport protocol a session
        // speaks is a member of `ModifyClientVpnEndpoint` — the one call that reconfigures an
        // endpoint that exists — so a declaration restating one asks for a different endpoint.
        crate::state::provisioner::client_vpn::EC2_CLIENT_VPN_ENDPOINT => {
            &["AuthenticationOptions", "ClientCidrBlock", "TransportProtocol"]
        }
        // An AUTHORIZATION RULE has no updatable property at all: Amazon EC2 publishes no
        // call that changes a rule that exists — the surface is the authorize that adds one
        // and the revoke that takes it off — so every property of the type is create-only and
        // a declaration restating any of them asks for a different rule.
        crate::state::provisioner::client_vpn::EC2_CLIENT_VPN_AUTHORIZATION_RULE => &[
            "AccessGroupId",
            "AuthorizeAllGroups",
            "ClientVpnEndpointId",
            "Description",
            "TargetNetworkCidr",
        ],
        // An attachment IS the pair of a VPC and a gateway, and its physical id is built from
        // the VPC half ([`vpc_gateway_attachment_identity`]) — so a declaration naming another
        // VPC names a different attachment, while the GATEWAY it puts on that VPC is swapped on
        // the attachment that exists.
        EC2_VPC_GATEWAY_ATTACHMENT => &["VpcId"],
        // A ROUTE is identified by the table it is in and the ONE destination it matches on,
        // which is what its physical id is built from ([`route_identity`]): restating either
        // names a different route, while the TARGET is `ReplaceRoute`'s to change on the route
        // that exists.
        EC2_ROUTE => &[
            "DestinationCidrBlock",
            "DestinationIpv6CidrBlock",
            "DestinationPrefixListId",
            "RouteTableId",
        ],
        // An ASSOCIATION is the binding between one subnet and one route table, and Amazon EC2
        // publishes no call that re-points one — `ReplaceRouteTableAssociation` makes a new
        // association, with a new id — so restating either side asks for a different
        // association.
        EC2_SUBNET_ROUTE_TABLE_ASSOCIATION => &["RouteTableId", "SubnetId"],
        // A security group RULE is the ONE permission it is: everything about it but its
        // DESCRIPTION is fixed when the authorize adds it, the only call that changes a rule
        // that exists being the one that writes its description.
        EC2_SECURITY_GROUP_INGRESS => &[
            "CidrIp",
            "CidrIpv6",
            "FromPort",
            "GroupId",
            "GroupName",
            "IpProtocol",
            "SourcePrefixListId",
            "SourceSecurityGroupId",
            "SourceSecurityGroupName",
            "SourceSecurityGroupOwnerId",
            "ToPort",
        ],
        EC2_SECURITY_GROUP_EGRESS => &[
            "CidrIp",
            "CidrIpv6",
            "DestinationPrefixListId",
            "DestinationSecurityGroupId",
            "FromPort",
            "GroupId",
            "IpProtocol",
            "ToPort",
        ],
        // A launch template is a versioned document: its launch DATA reaches the template that
        // exists as a new version of it, and only the name the template is known by is fixed.
        // (That name is also in this position through [`name_is_create_only`].)
        EC2_LAUNCH_TEMPLATE => &["LaunchTemplateName"],
        // An INSTANCE keeps the machine it was launched as: the image it boots, the subnet and
        // the zone it was placed in, the addresses and interfaces the launch gave it, the key
        // pair it admits and the placement it runs under are each a member of `RunInstances`
        // that no call changes afterwards.
        EC2_INSTANCE => &[
            "AdditionalInfo",
            "Affinity",
            "AvailabilityZone",
            "HostId",
            "HostResourceGroupArn",
            "ImageId",
            "Ipv6AddressCount",
            "Ipv6Addresses",
            "KeyName",
            // The launch template an instance was launched FROM is the machine it runs as: the
            // image, the type, the profile and the interfaces the launch read out of that
            // template version, none of which a call of that service re-reads for an instance
            // that exists. A declaration naming another template, or another version of one,
            // asks for an instance launched again.
            "LaunchTemplate",
            "NetworkInterfaces",
            "PlacementGroupName",
            "PrivateIpAddress",
            "SecurityGroups",
            "SubnetId",
            "Tenancy",
        ],
        // An elastic network INTERFACE is created in Amazon EC2 itself
        // ([`crate::state::provisioning::ec2_vpc_network`]), and the ids a declaration states
        // for the resources it names — its subnet, its security groups — are the ones those
        // services minted. Its own create-only properties are the ones no call of that service
        // changes on the interface that exists: the subnet it is in, the addresses the create
        // gave it and the kind of interface it is.
        EC2_NETWORK_INTERFACE => &[
            "EnablePrimaryIpv6",
            "InterfaceType",
            "Ipv6AddressCount",
            "PrivateIpAddress",
            "SecondaryPrivateIpAddressCount",
            "SubnetId",
        ],
        // An Amazon EBS VOLUME keeps the zone it is in, what it was created FROM and how it is
        // encrypted; its size, its performance and its attachment settings are `ModifyVolume`'s
        // to change.
        EC2_VOLUME => &[
            "AvailabilityZone",
            "Encrypted",
            "KmsKeyId",
            "OutpostArn",
            "SnapshotId",
        ],
        _ => composite_identity_properties(resource_type),
    }
}
