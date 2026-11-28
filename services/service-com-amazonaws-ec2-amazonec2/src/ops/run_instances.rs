// SPDX-License-Identifier: CC0-1.0

//! @op RunInstances
//!
//! Implemented: mints `MaxCount` instances in one fresh reservation, stores them in
//! [`crate::state`], and reports them back. Every instance starts in `pending` with
//! the `running` transition the clock owes it recorded, so a caller that describes it
//! immediately still sees `pending`.
//!
//! A launch that names an `IamInstanceProfile` gives every instance it mints that
//! profile: an association is recorded over each one through the store's single writer
//! seam ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_iam_instance_profile_association`]),
//! so the instance reports it as `Instance$IamInstanceProfile` and
//! `DescribeIamInstanceProfileAssociations` reports the same record.
//!
//! A launch that names its `NetworkInterfaces` gives every instance it mints exactly those
//! interfaces: each entry is created in the subnet it names, with the security groups,
//! private IPv4 addresses, description, type and options it names, and attached at its own
//! device index — or, for an entry naming an interface by id, that existing interface is
//! attached where it is. The entry at device index 0 is the instance's PRIMARY interface, so
//! its subnet, groups and primary private address are read into the launch's own members and
//! the rest of the launch takes exactly the path a launch that spelled them out takes. The
//! interfaces themselves are written through the store's one writer for them
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_launched_instance`]), so
//! `DescribeInstances` and `DescribeNetworkInterfaces` report one record per interface.
//!
//! A launch that names its `BlockDeviceMappings` creates its root volume from the entry it
//! carries for the AMI's own root device: that entry's size, type, IOPS, throughput,
//! encryption and delete-on-termination define the volume, and the AMI's own mapping answers
//! the members the launch left out ([`crate::state::ComAmazonawsEc2Amazonec2State::launch_root_block_device`]
//! is where the two are read into one device, the Region's EBS encryption default included).
//! An entry that names a `KmsKeyId` without asking for encryption names a volume that cannot
//! be created, so the launch is refused the parameter-dependency error before anything else;
//! an entry whose key KMS will not produce a data key under launches the instance and leaves
//! it the termination the clock owes it instead of `running`
//! ([`crate::state::launch_volume_encryption_keys`] is where the keys of one launch are read,
//! and [`crate::state::ComAmazonawsEc2Amazonec2State::withdraw_launch_under_unusable_volume_key`]
//! is what the refusal leaves behind).
//! Hibernation is admitted against THAT volume: "set to true to enable your instance for
//! hibernation" is valid "only if the instance meets the hibernation prerequisites", which
//! are read in one place ([`crate::state::hibernation_prerequisite_refusal`]) — the instance
//! type has to be one hibernation is offered on, the root device has to be encrypted and that
//! volume has to be large enough to hold the instance's memory — so a launch meeting none of
//! them is refused `UnsupportedHibernationConfiguration` before anything is created. An entry
//! for any OTHER device neither moves the root device nor creates a volume
//! of its own here.
//!
//! The MOVES every instance of a launch then makes are published as they fall due: an
//! instance's state change is an event of the account's default EventBridge bus
//! ([`crate::state::InstanceStateNotification`]), so the `pending` the launch answers and the
//! `running` — or the `terminated` an unusable volume key owes it — reach a client watching
//! the bus without it calling EC2 again.
//!
//! An `ImageId` may name a Systems Manager parameter instead of an AMI — "alternatively, you
//! can specify a Systems Manager parameter […] The Systems Manager parameter will resolve to
//! an AMI ID on launch", written `resolve:ssm:<reference>` — so such a value is read through
//! the one reading of that form this crate has ([`crate::state::ssm_image_alias_parameter`])
//! and resolved by the service that OWNS parameters
//! ([`crate::state::resolved_image_alias`]): the launch runs from the AMI the parameter's
//! value names, which is also the id the instance reports. A reference that store produces no
//! value for names no AMI at all and is refused as the invalid ALIAS it is
//! (`SsmInvalidParameter`); a value it resolves is held to the issued-id form exactly as a
//! value naming an AMI directly is.
//!
//! An `InstanceType` outside the catalog the model publishes as that member's enum is refused
//! at the BOUNDARY, before this handler runs, with the code and sentence EC2 words for the
//! parameter type (`crate::service`'s refusal override, keyed on that catalog). What this
//! handler asks about an instance type is therefore only WHERE a published one is offered,
//! which is per Availability Zone.
//!
//! A repeat of the same request under one `ClientToken` returns the reservation that
//! token already minted instead of launching again. A `LaunchTemplate` names the version
//! whose launch parameters the launch runs from: the version is resolved against the
//! account's stored templates and OVERLAID with the members the request itself carries —
//! "any additional parameters that you specify for the new instance overwrite the
//! corresponding parameters included in the launch template" — before anything else is read,
//! so a template-driven launch and the equivalent spelled-out one take the same path through
//! this handler and cannot be served differently.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `RunInstances::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RunInstances`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Launches the specified number of instances using an AMI for which you have permissions.
//! You can specify a number of options, or leave the default options. The following rules
//! apply: If you don't specify a subnet ID, we choose a default subnet from your default
//! VPC for you. If you don't have a default VPC, you must specify a subnet ID in the
//! request. All instances have a network interface with a primary private IPv4 address. If
//! you don't specify this address, we choose one from the IPv4 range of your subnet. Not
//! all instance types support IPv6 addresses. For more information, see Instance types. If
//! you don't specify a security group ID, we use the default security group for the VPC.
//! For more information, see Security groups. If any of the AMIs have a product code
//! attached for which the user has not subscribed, the request fails. You can create a
//! launch template, which is a resource that contains the parameters to launch an instance.
//! When you launch an instance using RunInstances, you can specify the launch template
//! instead of specifying the launch parameters. To ensure faster instance launches, break
//! up large requests into smaller batches. For example, create five separate launch
//! requests for 100 instances each instead of one launch request for 500 instances.
//! RunInstances is subject to both request rate limiting and resource rate limiting. For
//! more information, see Request throttling. An instance is ready for you to use when it's
//! in the running state. You can check the state of your instance using DescribeInstances.
//! You can tag instances and EBS volumes during launch, after launch, or both. For more
//! information, see CreateTags and Tagging your Amazon EC2 resources. Linux instances have
//! access to the public key of the key pair at boot. You can use this key to provide secure
//! access to the instance. Amazon EC2 public images use this feature to provide secure
//! access without passwords. For more information, see Key pairs. For troubleshooting, see
//! What to do if an instance immediately terminates, and Troubleshooting connecting to your
//! instance.
//!
//! ## Input
//!
//! * `AdditionalInfo` (String) — Reserved.
//! * `BlockDeviceMappings` (BlockDeviceMappingRequestList) — The block device mapping, which
//!   defines the EBS volumes and instance store volumes to attach to the instance at launch.
//!   For more information, see Block device mappings in the Amazon EC2 User Guide.
//! * `CapacityReservationSpecification` (CapacityReservationSpecification) — Information
//!   about the Capacity Reservation targeting option. If you do not specify this parameter,
//!   the instance's Capacity Reservation preference defaults to open, which enables it to run
//!   in any open Capacity Reservation that has matching attributes (instance type, platform,
//!   Availability Zone, and tenancy).
//! * `ClientToken` (String, @idempotencyToken) — Unique, case-sensitive identifier you
//!   provide to ensure the idempotency of the request. If you do not specify a client token,
//!   a randomly generated token is used for the request to ensure idempotency. For more
//!   information, see Ensuring idempotency in Amazon EC2 API requests. Constraints: Maximum
//!   64 ASCII characters
//! * `CpuOptions` (CpuOptionsRequest) — The CPU options for the instance. For more
//!   information, see Optimize CPU options in the Amazon EC2 User Guide.
//! * `CreditSpecification` (CreditSpecificationRequest) — The credit option for CPU usage of
//!   the burstable performance instance. Valid values are standard and unlimited. To change
//!   this attribute after launch, use ModifyInstanceCreditSpecification. For more
//!   information, see Burstable performance instances in the Amazon EC2 User Guide. Default:
//!   standard (T2 instances) or unlimited (T3/T3a/T4g instances) For T3 instances with host
//!   tenancy, only standard is supported.
//! * `DisableApiStop` (Boolean) — Indicates whether an instance is enabled for stop
//!   protection. For more information, see Enable stop protection for your EC2 instances.
//! * `DisableApiTermination` (Boolean) — Indicates whether termination protection is enabled
//!   for the instance. The default is false, which means that you can terminate the instance
//!   using the Amazon EC2 console, command line tools, or API. You can enable termination
//!   protection when you launch an instance, while the instance is running, or while the
//!   instance is stopped.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `EbsOptimized` (Boolean) — Indicates whether the instance is optimized for Amazon EBS
//!   I/O. This optimization provides dedicated throughput to Amazon EBS and an optimized
//!   configuration stack to provide optimal Amazon EBS I/O performance. This optimization
//!   isn't available with all instance types. Additional usage charges apply when using an
//!   EBS-optimized instance. Default: false
//! * `ElasticGpuSpecification` (ElasticGpuSpecifications) — An elastic GPU to associate with
//!   the instance. Amazon Elastic Graphics reached end of life on January 8, 2024.
//! * `ElasticInferenceAccelerators` (ElasticInferenceAccelerators) — An elastic inference
//!   accelerator to associate with the instance. Amazon Elastic Inference is no longer
//!   available.
//! * `EnablePrimaryIpv6` (Boolean) — If you’re launching an instance into a dual-stack or
//!   IPv6-only subnet, you can enable assigning a primary IPv6 address. A primary IPv6
//!   address is an IPv6 GUA address associated with an ENI that you have enabled to use a
//!   primary IPv6 address. Use this option if an instance relies on its IPv6 address not
//!   changing. When you launch the instance, Amazon Web Services will automatically assign an
//!   IPv6 address associated with the ENI attached to your instance to be the primary IPv6
//!   address. Once you enable an IPv6 GUA address to be a primary IPv6, you cannot disable
//!   it. When you enable an IPv6 GUA address to be a primary IPv6, the first IPv6 GUA will be
//!   made the primary IPv6 address until the …
//! * `EnclaveOptions` (EnclaveOptionsRequest) — Indicates whether the instance is enabled for
//!   Amazon Web Services Nitro Enclaves. For more information, see Amazon Web Services Nitro
//!   Enclaves User Guide. You can't enable Amazon Web Services Nitro Enclaves and hibernation
//!   on the same instance.
//! * `HibernationOptions` (HibernationOptionsRequest) — Indicates whether an instance is
//!   enabled for hibernation. This parameter is valid only if the instance meets the
//!   hibernation prerequisites. For more information, see Hibernate your Amazon EC2 instance
//!   in the Amazon EC2 User Guide. You can't enable hibernation and Amazon Web Services Nitro
//!   Enclaves on the same instance.
//! * `IamInstanceProfile` (IamInstanceProfileSpecification) — The name or Amazon Resource
//!   Name (ARN) of an IAM instance profile.
//! * `ImageId` (ImageId) — The ID of the AMI. An AMI ID is required to launch an instance and
//!   must be specified here or in a launch template.
//! * `InstanceInitiatedShutdownBehavior` (ShutdownBehavior, one of: stop | terminate) —
//!   Indicates whether an instance stops or terminates when you initiate shutdown from the
//!   instance (using the operating system command for system shutdown). Default: stop
//! * `InstanceMarketOptions` (InstanceMarketOptionsRequest) — The market (purchasing) option
//!   for the instances. For RunInstances, persistent Spot Instance requests are only
//!   supported when InstanceInterruptionBehavior is set to either hibernate or stop.
//! * `InstanceType` (InstanceType, one of: a1.2xlarge | a1.4xlarge | a1.large | a1.medium |
//!   a1.metal | a1.xlarge | c1.medium | c1.xlarge | c3.2xlarge | c3.4xlarge | c3.8xlarge |
//!   c3.large | c3.xlarge | c4.2xlarge | c4.4xlarge | c4.8xlarge | c4.large | c4.xlarge |
//!   c5.12xlarge | …) — The instance type. For more information, see Amazon EC2 Instance
//!   Types Guide.
//! * `Ipv6AddressCount` (Integer) — The number of IPv6 addresses to associate with the
//!   primary network interface. Amazon EC2 chooses the IPv6 addresses from the range of your
//!   subnet. You cannot specify this option and the option to assign specific IPv6 addresses
//!   in the same request. You can specify this option if you've specified a minimum number of
//!   instances to launch. You cannot specify this option and the network interfaces option in
//!   the same request.
//! * `Ipv6Addresses` (InstanceIpv6AddressList) — The IPv6 addresses from the range of the
//!   subnet to associate with the primary network interface. You cannot specify this option
//!   and the option to assign a number of IPv6 addresses in the same request. You cannot
//!   specify this option if you've specified a minimum number of instances to launch. You
//!   cannot specify this option and the network interfaces option in the same request.
//! * `KernelId` (KernelId) — The ID of the kernel. We recommend that you use PV-GRUB instead
//!   of kernels and RAM disks. For more information, see PV-GRUB in the Amazon EC2 User
//!   Guide.
//! * `KeyName` (KeyPairName) — The name of the key pair. For more information, see Create a
//!   key pair for your EC2 instance. If you do not specify a key pair, you can't connect to
//!   the instance unless you choose an AMI that is configured to allow users another way to
//!   log in.
//! * `LaunchTemplate` (LaunchTemplateSpecification) — The launch template. Any additional
//!   parameters that you specify for the new instance overwrite the corresponding parameters
//!   included in the launch template.
//! * `LicenseSpecifications` (LicenseSpecificationListRequest) — The license configurations.
//! * `MaintenanceOptions` (InstanceMaintenanceOptionsRequest) — The maintenance and recovery
//!   options for the instance.
//! * `MaxCount` (Integer) **required** — The maximum number of instances to launch. If you
//!   specify a value that is more capacity than Amazon EC2 can launch in the target
//!   Availability Zone, Amazon EC2 launches the largest possible number of instances above
//!   the specified minimum count. Constraints: Between 1 and the quota for the specified
//!   instance type for your account for this Region. For more information, see Amazon EC2
//!   instance type quotas.
//! * `MetadataOptions` (InstanceMetadataOptionsRequest) — The metadata options for the
//!   instance. For more information, see Configure the Instance Metadata Service options.
//! * `MinCount` (Integer) **required** — The minimum number of instances to launch. If you
//!   specify a value that is more capacity than Amazon EC2 can provide in the target
//!   Availability Zone, Amazon EC2 does not launch any instances. Constraints: Between 1 and
//!   the quota for the specified instance type for your account for this Region. For more
//!   information, see Amazon EC2 instance type quotas.
//! * `Monitoring` (RunInstancesMonitoringEnabled) — Specifies whether detailed monitoring is
//!   enabled for the instance.
//! * `NetworkInterfaces` (InstanceNetworkInterfaceSpecificationList) — The network interfaces
//!   to associate with the instance.
//! * `NetworkPerformanceOptions` (InstanceNetworkPerformanceOptionsRequest) — Contains
//!   settings for the network performance options for the instance.
//! * `Operator` (OperatorRequest) — Reserved for internal use.
//! * `Placement` (Placement) — The placement for the instance.
//! * `PrivateDnsNameOptions` (PrivateDnsNameOptionsRequest) — The options for the instance
//!   hostname. The default values are inherited from the subnet. Applies only if creating a
//!   network interface, not attaching an existing one.
//! * `PrivateIpAddress` (String) — The primary IPv4 address. You must specify a value from
//!   the IPv4 address range of the subnet. Only one private IP address can be designated as
//!   primary. You can't specify this option if you've specified the option to designate a
//!   private IP address as the primary IP address in a network interface specification. You
//!   cannot specify this option if you're launching more than one instance in the request.
//!   You cannot specify this option and the network interfaces option in the same request.
//! * `RamdiskId` (RamdiskId) — The ID of the RAM disk to select. Some kernels require
//!   additional drivers at launch. Check the kernel requirements for information about
//!   whether you need to specify a RAM disk. To find kernel requirements, go to the Amazon
//!   Web Services Resource Center and search for the kernel ID. We recommend that you use
//!   PV-GRUB instead of kernels and RAM disks. For more information, see PV-GRUB in the
//!   Amazon EC2 User Guide.
//! * `SecondaryInterfaces` (InstanceSecondaryInterfaceSpecificationListRequest) — The
//!   secondary interfaces to associate with the instance.
//! * `SecurityGroupIds` (SecurityGroupIdStringList) — The IDs of the security groups. If you
//!   specify a network interface, you must specify any security groups as part of the network
//!   interface instead of using this parameter.
//! * `SecurityGroups` (SecurityGroupStringList) — [Default VPC] The names of the security
//!   groups. If you specify a network interface, you must specify any security groups as part
//!   of the network interface instead of using this parameter. Default: Amazon EC2 uses the
//!   default security group.
//! * `SubnetId` (SubnetId) — The ID of the subnet to launch the instance into. If you specify
//!   a network interface, you must specify any subnets as part of the network interface
//!   instead of using this parameter.
//! * `TagSpecifications` (TagSpecificationList) — The tags to apply to the resources that are
//!   created during instance launch. You can specify tags for the following resources only:
//!   Instances Volumes Spot Instance requests Network interfaces To tag a resource after it
//!   has been created, see CreateTags.
//! * `UserData` (RunInstancesUserData) — The user data to make available to the instance.
//!   User data must be base64-encoded. Depending on the tool or SDK that you're using, the
//!   base64-encoding might be performed for you. For more information, see Run commands at
//!   launch using instance user data.
//!
//! ## Output
//!
//! * `Groups` (GroupIdentifierList) — Not supported.
//! * `Instances` (InstanceList) — The instances.
//! * `OwnerId` (String) — The ID of the Amazon Web Services account that owns the
//!   reservation.
//! * `RequesterId` (String) — The ID of the requester that launched the instances on your
//!   behalf (for example, Amazon Web Services Management Console or Auto Scaling).
//! * `ReservationId` (String) — The ID of the reservation.

#[allow(unused_imports)]
use super::types::*;

// The interfaces a launch gives its instance, as the store writes them — the one shape this
// handler resolves a launch's `NetworkInterfaces` into.
use crate::state::LaunchedInterface;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RunInstancesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RunInstancesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#RunInstances")]
#[derive(Debug)]
pub enum RunInstancesErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::run_instances::RunInstancesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RunInstancesError = crate::scaffolded::Scaffolded<RunInstancesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RunInstancesErrorModeled> for RunInstancesError {
    fn from(modeled: RunInstancesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RunInstances` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RunInstances<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RunInstances<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RunInstances<Eng>
{
    type Input = RunInstancesInput;
    type Output = RunInstancesOutput;
    type Error = RunInstancesError;

    const NAME: &'static str = "RunInstances";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RUN_INSTANCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RUN_INSTANCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            architecture_for, availability_zones_in_region,
            iam_instance_profile_association_payload_bytes, invalid_ami_id_malformed_expecting_ami,
            invalid_ssm_image_aliases, issued_image_id_is_well_formed,
            maximum_instance_count_below_minimum, private_dns_name,
            request_region, require_service_state, resolve_iam_instance_profile,
            resolved_image_alias, AliasResolution,
            ssm_image_alias_parameter, validate_kernel_or_ramdisk_id,
            validate_tag_size,
            IamInstanceProfileAssociationSpec, InstanceDetail, InstanceDetailSpec, InstanceRecord,
            InstanceStateName,
            MONITORING_STATE_DISABLED, MONITORING_STATE_ENABLED,
            PendingTransition, ReservationRecord,
            DEFAULT_INSTANCE_TYPE,
            DEFAULT_SOURCE_DEST_CHECK, MAX_BLOCK_DEVICE_MAPPINGS,
            MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS,
            MAX_INSTANCES_PER_RUN,
            MAX_SECURITY_GROUPS_PER_INSTANCE, MAX_STORED_BYTES, MAX_STORED_INSTANCES,
            MAX_NETWORK_INTERFACES, MAX_USER_DATA_BYTES, RUN_INSTANCES_TAGGABLE_RESOURCES,
            PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX,
            STORED_RECORD_OVERHEAD_BYTES,
            TRANSITION_DELAY,
        };

        // "If you specify a network interface, you must specify any subnets / security groups
        // as part of the network interface instead of using this parameter"
        // (`RunInstancesRequest$SubnetId`, `$SecurityGroupIds`, `$SecurityGroups`), and "you
        // can't specify this option if you've specified the option to designate a private IP
        // address as the primary IP address in a network interface specification"
        // (`$PrivateIpAddress`): a launch that names its interfaces names those three facts ON
        // them, so one that names the same fact twice is refused rather than served from
        // whichever of the two places this handler happened to read. Asked of the RESOLVED
        // launch below — after the launch template has been overlaid — because a template
        // expresses those same facts, so the combination holds whichever side of the launch
        // each half came from.
        //
        // A launch template carries the launch parameters this handler resolves, so a launch
        // that names one is answered from the template VERSION it names, overlaid with the
        // members the request itself carries — "any additional parameters that you specify
        // for the new instance overwrite the corresponding parameters included in the launch
        // template" (`RunInstancesRequest$LaunchTemplate`). The overlay happens HERE, before
        // anything else is read, so every check, ceiling, refusal and write below sees ONE
        // launch: a template-driven launch and the equivalent spelled-out one cannot be
        // served differently.
        //
        // The launch template this launch READ, as it resolved it: the template's id and the
        // version NUMBER, which every instance the launch mints carries as the two system tags
        // Amazon EC2 adds for a template launch ([`crate::state::LAUNCH_TEMPLATE_ID_TAG`]).
        let mut launched_template: Option<(String, i64)> = None;
        let mut input = match input.launch_template.clone() {
            None => input,
            Some(specification) => {
                let state = require_service_state(&*self.engine)?;
                // The template is read under its own lock and the version's data CLONED out of
                // it: nothing below holds a borrow of the store across the rest of the launch,
                // and the launch runs from the parameters as they stood when it resolved them.
                let data = {
                    let store = state.lock();
                    // The template is resolved the way every reader of one resolves it, which
                    // is what settles the key combination (both, or neither), the id's
                    // screen — an id no mint of this service produced is not an id of a
                    // launch template at all — and a name's lookup, as well as the sentence
                    // an id that named a template the account no longer holds is refused
                    // with.
                    let launch_template_id = crate::state::resolve_launch_template_id(
                        &store,
                        specification.launch_template_id.as_deref(),
                        specification.launch_template_name.as_deref(),
                    )?;
                    let record =
                        store.launch_templates.get(&launch_template_id).ok_or_else(|| {
                            Scaffolded::internal_error(format!(
                                "launch template '{launch_template_id}' resolved but is not \
                                 stored"
                            ))
                        })?;
                    let requested_version = specification
                        .version
                        .as_deref()
                        .filter(|version| !version.is_empty())
                        .unwrap_or(crate::state::LAUNCH_TEMPLATE_VERSION_DEFAULT);
                    // "A value of $Latest uses the latest version of the launch template. A
                    // value of $Default uses the default version of the launch template.
                    // Default: The default version of the launch template"
                    // (`LaunchTemplateSpecification$Version`), resolved through the one
                    // reading of that member this crate has — which refuses a version the
                    // template never minted in the sentence a launch answers
                    // (`crate::state::specified_launch_template_version_not_found`): the
                    // caller is told which version of which template could not be found.
                    let version =
                        crate::state::resolve_launch_template_version(record, requested_version)?;
                    // The pair this launch resolved, kept for the two system tags every
                    // instance it mints carries.
                    launched_template = Some((launch_template_id.clone(), version));
                    record
                        .versions
                        .get(&version)
                        .ok_or_else(|| {
                            Scaffolded::internal_error(format!(
                                "launch template '{launch_template_id}' version '{version}' \
                                 resolved but is not stored"
                            ))
                        })?
                        .data
                        .clone()
                };
                launch_overlaid_with_template(input, &data)?
            }
        };
        // An OPTIONAL member carrying the EMPTY STRING names no value: it is read as the
        // member being absent, so what the launch runs under is that member's documented
        // default rather than the empty text resolved as a zone or an address. That is how
        // this launch already reads its key pair name, its placement group name and its host
        // id, and how every other operation of this service reads a zone a request leaves
        // empty, so the two members below are read the same way — on the RESOLVED launch, so
        // a member a launch template left empty is read as absent too.
        //
        // "The Availability Zone of the instance. On input, you can specify
        // `AvailabilityZone` or `AvailabilityZoneId`, but not both. If you specify neither
        // one, Amazon EC2 automatically selects an Availability Zone for you"
        // (`Placement$AvailabilityZone`): a launch that leaves it empty names no zone,
        // so the zone is chosen below exactly as for a launch that omitted the member.
        if let Some(placement) = input.placement.as_mut() {
            if placement
                .availability_zone
                .as_deref()
                .is_some_and(str::is_empty)
            {
                placement.availability_zone = None;
            }
        }
        // "The primary IPv4 address. You must specify a value from the IPv4 address range of
        // the subnet" (`RunInstancesRequest$PrivateIpAddress`) — a launch that leaves it empty
        // names no address, so the subnet picks one as it does for a launch that omitted the
        // member, rather than the empty text being refused as an address it is not.
        if input
            .private_ip_address
            .as_deref()
            .is_some_and(str::is_empty)
        {
            input.private_ip_address = None;
        }
        // Whether this launch is a PROBE: `DryRun` "checks whether you have the required
        // permissions for the action, WITHOUT ACTUALLY MAKING THE REQUEST, and provides an
        // error response. If you have the required permissions, the error response is
        // DryRunOperation" (`RunInstancesRequest$DryRun`). So a probe is answered once the
        // request's own values have been read and the resources it NAMES resolved — which is
        // where the arm below sits — and before the resources it named are read AGAINST EACH
        // OTHER, which is part of carrying the launch out. The recorded contract draws that
        // line exactly: a probe whose network interfaces and instance-level members cannot
        // be given together is answered that parameter combination, while one naming a
        // security group of another VPC than the subnet it lands in is answered the verdict.
        let probe = input.dry_run == Some(true);
        // The combination rule above, asked of the launch as RESOLVED: the facts a launch
        // expresses are the facts of the merged parameters, so a launch whose interfaces come
        // from its template and whose subnet, security groups or primary private address come
        // from the request names the same fact twice exactly as a spelled-out one does — and
        // so does a template that carries both halves itself.
        if input
            .network_interfaces
            .as_ref()
            .is_some_and(|interfaces| !interfaces.is_empty())
        {
            refuse_interface_and_instance_level_members(&input)?;
        }
        // "The network interfaces to associate with the instance"
        // (`RunInstancesRequest$NetworkInterfaces`): a launch that names them launches each
        // of its instances with exactly those interfaces — created in the subnet, with the
        // security groups, private addresses and options each entry names, and attached at
        // its device index — instead of with the single primary interface a launch that names
        // none gets. Read HERE, before anything else, so the rest of this handler sees ONE
        // launch: the entry at device index 0 is the instance's primary interface, and its
        // subnet, groups and primary private address ARE the instance's own.
        let interface_requests = read_interface_specifications(
            input.network_interfaces.as_deref().unwrap_or_default(),
            input.max_count,
            input
                .instance_type
                .as_deref()
                .unwrap_or(DEFAULT_INSTANCE_TYPE),
        )?;
        // A secondary interface is a resource of its own
        // (`Instance$SecondaryInterfaces`, which `DescribeSecondaryInterfaces` reports),
        // created in a SECONDARY subnet of a secondary network — resources this launch path
        // holds none of, so an instance it minted could report none of what was asked for.
        // Refused rather than accepted and discarded.
        if input
            .secondary_interfaces
            .as_ref()
            .is_some_and(|interfaces| !interfaces.is_empty())
        {
            return Err(Scaffolded::not_implemented(
                "RunInstances with a secondary interface specification",
            ));
        }
        // An IPv6 address comes from the subnet's IPv6 range, which is not modelled, so an
        // instance could not report the address this asks for — whether the launch asks for
        // it on its own members or on one of its interfaces.
        if input
            .ipv6_addresses
            .as_ref()
            .is_some_and(|addresses| !addresses.is_empty())
            || input.ipv6_address_count.is_some_and(|count| count > 0)
            || input
                .network_interfaces
                .iter()
                .flatten()
                .any(interface_asks_for_ipv6)
        {
            return Err(Scaffolded::not_implemented(
                "RunInstances with IPv6 addresses",
            ));
        }

        let Some(image_id) = input.image_id.clone() else {
            return Err(Scaffolded::missing_parameter(
                "The request must contain the parameter ImageId",
            ));
        };
        // "Alternatively, you can specify a Systems Manager parameter […] The Systems Manager
        // parameter will resolve to an AMI ID on launch" (`ImageId`): a value carrying the
        // `resolve:ssm:` prefix names a PARAMETER, not an AMI, so it is read through the one
        // reading of that form this crate has ([`crate::state::ssm_image_alias_parameter`])
        // and resolved below, by the service that OWNS parameters, before the store is opened.
        let image_alias = ssm_image_alias_parameter(&image_id).map(str::to_string);
        // "An AMI ID is required to launch an instance" (`RunInstancesRequest$ImageId`), and
        // the id's FORM is read before anything is looked up under it: a value EC2 could not
        // have issued an AMI id names no image that could ever exist, so the launch is
        // refused as malformed rather than started from an id with no image behind it. A
        // client that cannot tell a mistyped id from a missing image goes on polling for an
        // instance that can never appear.
        if image_alias.is_none() && !issued_image_id_is_well_formed(&image_id) {
            return Err(invalid_ami_id_malformed_expecting_ami(&image_id));
        }
        if input.min_count < 1 {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{}' for minCount. Minimum count must be at least 1",
                input.min_count
            )));
        }
        // "Maximum instance count must not be smaller than minimum instance count" is the
        // whole rule and EC2's whole sentence for breaking it
        // ([`crate::state::maximum_instance_count_below_minimum`]): the refusal is about the
        // ORDER of the two counts, so it names neither.
        if input.max_count < input.min_count {
            return Err(maximum_instance_count_below_minimum());
        }
        if input.private_ip_address.is_some() && input.max_count > 1 {
            return Err(Scaffolded::invalid_parameter_combination(
                "PrivateIpAddress cannot be specified when launching more than one instance",
            ));
        }
        // `PrivateIpAddress` is a plain String in the model, so the handler is what
        // refuses a value that is not an IPv4 address at all.
        if let Some(address) = input.private_ip_address.as_deref() {
            if address.parse::<::std::net::Ipv4Addr>().is_err() {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{address}' for privateIpAddress. \
                     It must be an IPv4 address"
                )));
            }
        }
        // `MaxCount` arrives as a plain `i32` with no modelled `@range`, so this is
        // the only place a ceiling can exist. It must dominate the minting loop
        // below: without it one request could ask for `i32::MAX` records and stall
        // or exhaust the process.
        if input.max_count > MAX_INSTANCES_PER_RUN {
            return Err(Scaffolded::instance_limit_exceeded(format!(
                "You requested {} instances, above the {MAX_INSTANCES_PER_RUN} \
                 this account may launch in one request",
                input.max_count
            )));
        }
        for specification in input.tag_specifications.iter().flatten() {
            if let Some(resource_type) = specification.resource_type.as_deref() {
                if !RUN_INSTANCES_TAGGABLE_RESOURCES.contains(&resource_type) {
                    return Err(Scaffolded::invalid_parameter_value(format!(
                        "'{resource_type}' is not a valid resource type for a \
                         RunInstances tag specification"
                    )));
                }
            }
            for tag in specification.tags.iter().flatten() {
                validate_tag_size(
                    tag.key.as_deref().unwrap_or_default(),
                    tag.value.as_deref().unwrap_or_default(),
                )?;
            }
        }
        // "Each resource can have up to 50 tags" — [`crate::state::MAX_TAGS_PER_RESOURCE`],
        // the count every create of this crate holds its tag payload to
        // ([`crate::state::tags_from_specifications`]). A launch's entries are retained by
        // each resource it creates of their type, so the COUNT per type is bounded here
        // beside the per-tag lengths above: without it one request could hand a resource
        // more tags than it may hold, and retain them on every record the launch mints.
        for resource_type in RUN_INSTANCES_TAGGABLE_RESOURCES {
            let asked = crate::state::launch_resource_tags(
                input.tag_specifications.as_deref().unwrap_or_default(),
                resource_type,
            )
            .len();
            if asked > crate::state::MAX_TAGS_PER_RESOURCE {
                return Err(Scaffolded::tag_limit_exceeded(format!(
                    "The request carries {asked} tags, above the maximum of {max} per resource",
                    max = crate::state::MAX_TAGS_PER_RESOURCE,
                )));
            }
        }
        // Each member below is CLONED into every instance the request mints, so a
        // count ceiling on `MaxCount` alone does not bound the BYTES one request can
        // retain: each needs a size bound of its own.
        if input
            .user_data
            .as_ref()
            .is_some_and(|user_data| user_data.len() > MAX_USER_DATA_BYTES)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "User data accepts at most {MAX_USER_DATA_BYTES} bytes"
            )));
        }
        // A kernel or RAM disk id is copied onto every instance the launch mints, exactly
        // as its user data is, and neither member carries a modelled `@length`.
        validate_kernel_or_ramdisk_id("kernelId", input.kernel_id.as_deref())?;
        validate_kernel_or_ramdisk_id("ramdiskId", input.ramdisk_id.as_deref())?;
        let named_groups = input.security_group_ids.as_ref().map_or(0, Vec::len)
            + input.security_groups.as_ref().map_or(0, Vec::len);
        if named_groups > MAX_SECURITY_GROUPS_PER_INSTANCE {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "An instance may be launched with at most \
                 {MAX_SECURITY_GROUPS_PER_INSTANCE} security groups, not {named_groups}"
            )));
        }
        // "[Default VPC] The names of the security groups"
        // (`RunInstancesRequest$SecurityGroups`): a NAME is the default VPC's spelling of a
        // group, the id being how a group of any other network is named
        // (`$SecurityGroupIds`) — so a launch that names the SUBNET it goes into gives its
        // groups by id, and one that names them by name beside a subnet has given two members
        // that cannot be served together. Asked of the RESOLVED launch, so a name and a
        // subnet that reached it from its launch template are held to the same rule, and
        // asked HERE, with the rest of the parameter validation and BEFORE the DryRun arm: a
        // request whose own members cannot hold together is refused for that whether or not
        // it was only a probe.
        if input
            .subnet_id
            .as_deref()
            .is_some_and(|subnet_id| !subnet_id.is_empty())
            && input
                .security_groups
                .iter()
                .flatten()
                .any(|group_name| !group_name.is_empty())
        {
            return Err(crate::state::parameters_cannot_be_combined(
                "groupName", "subnet",
            ));
        }
        // `BlockDeviceMappings` carries no modelled `@length` either, and every entry of it is
        // walked below — the `KmsKeyId`/`Encrypted` pairing, the root device overlay, and the
        // keys this launch asks the service that owns them about — so the list needs a ceiling
        // of its own beside the count one, and it has to dominate all three: without it one
        // request could name a list as long as its body has room for and turn it into that much
        // work before any other bound is reached.
        let named_mappings = input.block_device_mappings.as_ref().map_or(0, Vec::len);
        if named_mappings > MAX_BLOCK_DEVICE_MAPPINGS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "An instance may be launched with at most {MAX_BLOCK_DEVICE_MAPPINGS} block \
                 device mappings, not {named_mappings}"
            )));
        }
        // "If KmsKeyId is specified, the encrypted state must be true" — the rule the same
        // pair of members carries wherever a volume is created
        // (`CreateVolumeRequest$KmsKeyId`): a key names HOW a volume is encrypted, so an
        // entry naming one without asking for encryption names a volume Amazon EC2 cannot
        // create, and the half the request left out is what it got wrong — EC2's parameter
        // DEPENDENCY refusal rather than a bad value. Asked of EVERY `BlockDeviceMappings`
        // entry of the RESOLVED launch, so a mapping that reached the launch from its launch
        // template is held to it exactly as one the request spelled out is, and asked HERE,
        // with the rest of the parameter validation and before the DryRun arm: a request
        // whose parameters cannot hold together is refused for that whether or not it was
        // only a probe.
        for mapping in input.block_device_mappings.iter().flatten() {
            let Some(ebs) = mapping.ebs.as_ref() else {
                continue;
            };
            if ebs
                .kms_key_id
                .as_deref()
                .is_some_and(|key_id| !key_id.is_empty())
                && ebs.encrypted != Some(true)
            {
                return Err(Scaffolded::invalid_parameter_dependency(
                    "KmsKeyId", "Encrypted",
                ));
            }
        }

        // "The name or Amazon Resource Name (ARN) of an IAM instance profile"
        // (`RunInstancesRequest$IamInstanceProfile`): a launch that names one gives every
        // instance it mints that profile, so the reference is read through the ONE reading
        // of that member this crate has — the same one `AssociateIamInstanceProfile` uses,
        // so a profile cannot be spelled one way here and another there — BEFORE the store
        // is touched. A reference this service cannot act on refuses the whole launch
        // rather than being accepted and dropped.
        //
        // And the profile a launch names has to EXIST: the instance would run as it, so a
        // name the account's IAM records hold no profile under names no profile the instance
        // could ever run as, and the launch is refused for that member
        // ([`crate::state::unheld_iam_instance_profile`]) — a `DryRun` probe included,
        // because this reads the launch's own member rather than the launch this handler
        // would have assembled. That is the recorded answer to a launch, probe or not, that
        // names a role's name where an instance profile's belongs.
        let iam_instance_profile = match input.iam_instance_profile.as_ref() {
            Some(specification) => {
                let resolved = resolve_iam_instance_profile(&*self.engine, specification)?;
                if let Some(refusal) = crate::state::unheld_iam_instance_profile(
                    ctx,
                    &request_region(ctx.parts()),
                    specification,
                )
                .await
                {
                    return Err(refusal);
                }
                Some(resolved)
            }
            None => None,
        };

        let region = request_region(ctx.parts());
        // The zone an instance launches into must be one the Region has, and the
        // naming rule for those zones lives in one place: `availability_zones_in_region`.
        let zones = availability_zones_in_region(&region);
        let availability_zone = match input
            .placement
            .as_ref()
            .and_then(|placement| placement.availability_zone.clone())
        {
            Some(requested) => {
                crate::state::availability_zone_of_region(
                    &zones,
                    &region,
                    "availabilityZone",
                    &requested,
                    |zone| zone.zone_name.as_str(),
                )?;
                requested
            }
            // AWS picks a zone when the launch names none; the first of the
            // Region's is as good a choice as any and is stable per Region.
            None => zones
                .first()
                .map(|zone| zone.zone_name.clone())
                .ok_or_else(|| {
                    Scaffolded::internal_error(format!(
                        "region '{region}' reports no Availability Zone"
                    ))
                })?,
        };

        // The AMI this launch runs from and the ROOT BLOCK DEVICE every instance of it would
        // be created with, read under a guard of this store that is taken, read and GIVEN
        // BACK here — because the keys that device names are asked of the service that owns
        // them straight after, and no lock of this store is ever held while another service
        // is called.
        //
        // These two reads therefore happen under their own guard, which materialises the
        // transitions the clock owes before reading exactly as the main guard below does — a
        // recycled AMI past its retention period and a `pending` AMI a copy produced are among
        // those moves, so the image this launch resolves is the image a describe would report
        // at this instant either way.
        //
        // The parameter an `ImageId` of the `resolve:ssm:` form names is resolved BEFORE that
        // guard is taken, by the service that OWNS Systems Manager parameters — "the Systems
        // Manager parameter will resolve to an AMI ID on launch" — so no lock of this store is
        // held while that service answers ([`crate::state::resolved_image_alias`]). The launch
        // runs from the AMI the parameter's value names and the instance reports THAT id, not
        // the parameter it was asked for; a reference that store produces no value for names no
        // AMI at all, which is the invalid-ALIAS refusal
        // ([`crate::state::invalid_ssm_image_aliases`], the sentence naming the reference the
        // request wrote), and a value it resolves that is no AMI id is read as the id it is not
        // ([`issued_image_id_is_well_formed`], the same screen a launch naming an AMI directly
        // goes through). An emulator with no Systems Manager to ask resolves the published
        // image catalog this service itself projects into that store, so a published parameter
        // resolves either way.
        let resolved_alias = match image_alias.as_deref() {
            Some(reference) => Some(resolved_image_alias(ctx, &region, reference).await),
            None => None,
        };

        // The AMI this launch runs from and the ROOT BLOCK DEVICE every instance of it would
        // be created with, read under a guard of this store that is taken, read and GIVEN
        // BACK here — because the keys that device names are asked of the service that owns
        // them straight after, and no lock of this store is ever held while another service
        // is called.
        //
        // These two reads therefore happen under their own guard, which materialises the
        // transitions the clock owes before reading exactly as the main guard below does — a
        // recycled AMI past its retention period and a `pending` AMI a copy produced are among
        // those moves, so the image this launch resolves is the image a describe would report
        // at this instant either way.
        //
        // "The block device mapping, which defines the EBS volumes and instance store volumes
        // to attach to the instance at launch" (`RunInstancesRequest$BlockDeviceMappings`):
        // the root device every instance of this launch is created with is the AMI's own with
        // the launch's entry for that device overlaid, read through the ONE resolution of it —
        // so the volume stored below, the prerequisite asked of it and the key asked about are
        // the same device.
        let (image_id, root_block_device, data_block_devices) = {
            let state = require_service_state(&*self.engine)?;
            let mut store = state.lock();
            store.materialize_due_transitions();
            let image_id = match (image_alias.as_deref(), resolved_alias) {
                (Some(reference), Some(resolution)) => {
                    let resolved = match resolution {
                        AliasResolution::Value(value) => value,
                        AliasResolution::Unresolved => {
                            return Err(invalid_ssm_image_aliases([reference]));
                        }
                        AliasResolution::Unknown => store
                            .public_ssm_parameter_image_ids(::engine::emulator_account_id())
                            .remove(reference)
                            .ok_or_else(|| invalid_ssm_image_aliases([reference]))?,
                    };
                    if !issued_image_id_is_well_formed(&resolved) {
                        return Err(invalid_ami_id_malformed_expecting_ami(&resolved));
                    }
                    resolved
                }
                _ => image_id,
            };
            let root_block_device = store
                .launch_root_block_device(
                    Some(&image_id),
                    input.block_device_mappings.as_deref().unwrap_or_default(),
                    &region,
                )
                .map(|(_, ebs)| ebs);
            // And the devices the mapping names BESIDE the root one, read through the one
            // resolution of those: each is a volume this launch creates for every instance
            // it mints.
            let data_block_devices = store.launch_data_block_devices(
                Some(&image_id),
                input.block_device_mappings.as_deref().unwrap_or_default(),
                &region,
            );
            (image_id, root_block_device, data_block_devices)
        };

        // WHETHER THE VOLUMES THIS LAUNCH WOULD CREATE CAN EXIST AT ALL under the keys they
        // name, asked of the service that owns those keys and asked BEFORE this store's guard
        // is taken, because the ask is a call into another service: "Amazon EBS … requests a
        // data key from KMS" for an encrypted volume, so a key that will not produce one —
        // disabled, pending deletion, in a state KMS will not use, or naming no key of the
        // account — leaves the instance with no root volume to boot from. The launch is still
        // ACCEPTED and answers the `pending` instances it minted: what the key refuses is
        // those instances coming up, which is recorded as the termination the clock then owes
        // each of them ([`crate::state::ComAmazonawsEc2Amazonec2State::withdraw_launch_under_unusable_volume_key`]).
        // A key that will serve, and an emulator with no KMS to ask, leave every instance on
        // the ordinary path to `running`.
        //
        // A `DryRun` probe asks nothing of KMS: it is answered below without creating
        // anything, and a data key is requested for volumes that are really being created.
        let mut volume_key_refusal = None;
        if !probe {
            for key_id in crate::state::launch_volume_encryption_keys(
                root_block_device.as_ref(),
                input.block_device_mappings.as_deref().unwrap_or_default(),
            ) {
                volume_key_refusal = crate::state::volume_key_refusal(ctx, &region, &key_id).await;
                if volume_key_refusal.is_some() {
                    break;
                }
            }
        }

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();
        store.materialize_due_transitions();
        // "If you don't specify a subnet ID, we choose a default subnet from your default
        // VPC for you" (`RunInstances`) — so the Region's default network has to be there
        // to choose from. It has been owed to the account since the account existed rather
        // than since this call, which is what makes materialising it here the same write
        // every describe of VPCs, subnets or security groups already makes
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::ensure_default_network`]).
        store.ensure_default_network(&region);

        // "The ID of the subnet to launch the instance into" (`RunInstancesRequest$SubnetId`):
        // the subnet is where every part of the instance's network identity comes from — its
        // VPC, its Availability Zone, its primary private address and the groups applied to
        // its interface — so an id the account holds no subnet under names a network no
        // instance could be placed in. Refused with the subnet's own not-found class
        // ([`crate::state::invalid_subnet_id_not_found`], the same refusal an interface
        // specification naming an unknown subnet is already given, so one launch cannot read
        // the member two ways) rather than served as an instance with no network at all.
        // Asked HERE, before the first write and before the DryRun arm, with the rest of the
        // resources the request NAMES.
        if let Some(subnet_id) = input
            .subnet_id
            .as_deref()
            .filter(|subnet_id| !subnet_id.is_empty())
        {
            if !store.subnets.contains_key(subnet_id) {
                return Err(crate::state::invalid_subnet_id_not_found(subnet_id));
            }
        }

        // The interfaces the launch named, resolved against the account: the subnet each one
        // sits in and, for an entry that names an interface by id, the interface of the
        // account it attaches. Resolved HERE, before the launch reads its own subnet,
        // because the PRIMARY interface is where a launch that names its interfaces names
        // the subnet the instance runs in, the groups applied to it and its primary private
        // address — so those are read into the launch's own members and everything below
        // takes exactly the path a launch that spelled them out takes.
        let planned_interfaces = plan_launch_interfaces(
            &store,
            &interface_requests,
            input.subnet_id.as_deref(),
            &availability_zone,
        )?;
        if let Some(primary) = planned_interfaces
            .iter()
            .find(|interface| interface.request.device_index == PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX)
        {
            input.subnet_id = Some(primary.subnet_id.clone());
            if input.security_group_ids.as_ref().is_none_or(Vec::is_empty) {
                input.security_group_ids = Some(primary.request.group_ids.clone());
            }
        }

        // The parameter an `ImageId` of the `resolve:ssm:` form names was resolved above, by
        // the service that owns parameters, before any guard of this store was taken.

        // "The ID of the subnet to launch the instance into" (`RunInstancesRequest$SubnetId`)
        // — and a subnet belongs to ONE Availability Zone, so a launch that named a subnet
        // of this account and no zone of its own runs in that subnet's zone rather than in
        // the Region's first. A launch that named a zone keeps it; the two are checked
        // against each other by nothing here, because the model's `Placement` is what the
        // caller asked for.
        //
        // A launch that named NEITHER, and targets a Capacity Reservation, runs in THAT
        // reservation's zone: a reservation reserves capacity in one Availability Zone
        // (`CapacityReservation$AvailabilityZone`), so the zone the caller left open is the
        // reservation's — a launch into the Region's first zone would be a launch the
        // reservation it explicitly targeted could not hold.
        let targeted_reservation_zone = input
            .capacity_reservation_specification
            .as_ref()
            .and_then(|specification| specification.capacity_reservation_target.as_ref())
            .and_then(|target| target.capacity_reservation_id.as_deref())
            .filter(|capacity_reservation_id| !capacity_reservation_id.is_empty())
            .and_then(|capacity_reservation_id| {
                store.capacity_reservations.get(capacity_reservation_id)
            })
            .and_then(|record| record.view.availability_zone.clone());
        let availability_zone = if input
            .placement
            .as_ref()
            .is_some_and(|placement| placement.availability_zone.is_some())
        {
            availability_zone
        } else {
            input
                .subnet_id
                .as_deref()
                .and_then(|subnet_id| store.subnets.get(subnet_id))
                .and_then(|subnet| subnet.availability_zone.clone())
                .or(targeted_reservation_zone)
                .unwrap_or(availability_zone)
        };

        // Every interface an instance holds sits in the instance's own Availability Zone:
        // "you can attach a network interface to an instance in the same Availability Zone"
        // (the EC2 User Guide's "Elastic network interfaces"), and an interface lives in one
        // subnet's zone. Asked once the launch's zone is settled, so a launch whose
        // interfaces are spread across zones is refused rather than answered with an
        // instance holding an interface it could not reach.
        refuse_interfaces_outside_zone(&planned_interfaces, &availability_zone)?;

        // Idempotency: one ClientToken names the one launch it was spent on. Looked up
        // here, answered below the DryRun check, because a replay stores nothing and
        // so passes the ceilings a fresh launch has to clear — and a token spent on ANOTHER
        // launch is refused here and now ([`crate::state::launch_idempotency`]): the request
        // is not the one that token named, so there is no reservation to answer it with and
        // no second launch to make either.
        let launch_digest = crate::state::launch_parameters_digest(&input);
        let replayed = match input.client_token.as_deref() {
            Some(token) => match store.client_tokens.get(token) {
                Some(claim) if claim.parameters != launch_digest => {
                    return Err(crate::state::launch_idempotent_parameter_mismatch());
                }
                claim => claim.map(|claim| claim.reservation_id.clone()),
            },
            None => None,
        };

        let instance_type = input
            .instance_type
            .clone()
            .unwrap_or_else(|| DEFAULT_INSTANCE_TYPE.to_string());
        // The type an instance is PLACED as has to be one the catalog holds. A launch that
        // names it itself is held to that set by the model, but a type this launch took from
        // a launch TEMPLATE reached here without passing it: a template stores launch
        // PARAMETERS, so an instance type the catalog does not hold is a finding on the write
        // that stored it (`crate::state::launch_template_warning`) and this refusal on the
        // placement that tries to use it — through the ONE writer of that refusal
        // ([`crate::state::unplaceable_instance_type`], which every placement of this service
        // answers from), so a caller reads one code and one wording whichever way the type
        // reached the launch.
        if let Some(refusal) = crate::state::unplaceable_instance_type(Some(&instance_type)) {
            return Err(refusal);
        }
        // An instance type is offered per Availability Zone, not per Region: AWS does not
        // offer every type in every zone, so a launch into a zone that does not offer the
        // requested type is refused `Unsupported` rather than placed there. Asked against
        // the same offerings `DescribeInstanceTypeOfferings` reports
        // ([`crate::state::zone_offers_instance_type`]), so a caller that enumerated the
        // offerings to choose a zone and a launch into that zone cannot disagree.
        if !crate::state::zone_offers_instance_type(&availability_zone, &instance_type) {
            return Err(crate::state::unsupported_instance_type_in_zone(
                &instance_type,
                &availability_zone,
            ));
        }
        // An instance runs the machine code its AMI holds, so an AMI built for a processor
        // architecture the instance type does not have could never boot on it: "specify an
        // instance type and an AMI that have matching architectures". Weighed through the ONE
        // place a launch's AMI and its instance type are weighed against each other
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::launch_architecture_mismatch`]),
        // which the launches another service makes into this account already go through — so
        // no launch path can accept what another refuses — and asked BEFORE the first write,
        // so a refused launch creates no instance at all.
        if let Some(message) = store.launch_architecture_mismatch(&image_id, &instance_type) {
            return Err(Scaffolded::invalid_parameter_value(message));
        }
        // How many interfaces an instance may hold is a fact of its instance type, so a
        // launch that names more than its type can hold is refused before anything is
        // stored.
        refuse_too_many_interfaces(&planned_interfaces, &instance_type)?;
        // "You can only specify `CapacityReservationPreference` and
        // `CapacityReservationTarget` if the `CapacityReservationPreference` is
        // `capacity-reservations-only`" (`CapacityReservationSpecification`): any other
        // preference describes where the instance runs INSTEAD of a reservation the request
        // also targets, so the two members name two different placements and cannot be
        // combined. Asked BEFORE the target is resolved, the way EC2 orders a parameter
        // combination against a resource lookup: a request whose members cannot be served
        // together is refused for that, not for the id it also carried.
        if let Some(specification) = input.capacity_reservation_specification.as_ref() {
            let targets_a_reservation = specification.capacity_reservation_target.is_some();
            let preference = specification
                .capacity_reservation_preference
                .as_deref()
                .filter(|preference| !preference.is_empty());
            if targets_a_reservation
                && preference.is_some_and(|preference| {
                    preference != crate::state::CAPACITY_RESERVATION_PREFERENCE_ONLY
                })
            {
                return Err(crate::state::parameters_cannot_be_combined(
                    "capacityReservationPreference",
                    "capacityReservationTarget",
                ));
            }
        }
        // "Use the CapacityReservationTarget parameter to explicitly target a specific
        // Capacity Reservation or a Capacity Reservation group"
        // (`CapacityReservationSpecification`), and a reservation serves an instance only
        // when it "has matching attributes (instance type, platform, Availability Zone, and
        // tenancy)" (`CapacityReservationPreference`) — so a launch that TARGETS one is held
        // to the reservation's own instance type before anything is minted.
        // The Capacity Reservation this launch TARGETS, resolved against the reservation
        // store here and carried onto every instance the launch mints, so an instance never
        // reports a reservation the account does not hold.
        let targeted_capacity_reservation: Option<String> = match input
            .capacity_reservation_specification
            .as_ref()
            .and_then(|specification| specification.capacity_reservation_target.as_ref())
            .and_then(|target| target.capacity_reservation_id.as_deref())
            .filter(|id| !id.is_empty())
        {
            None => None,
            Some(capacity_reservation_id) => {
                // The reservation is resolved the way this operation's validator reads a
                // `cr-` id: a value EC2 could not have ISSUED — the all-zero placeholder
                // among them — is malformed before any lookup, and only an issuable id the
                // account holds no reservation under is reported not found
                // ([`crate::state::require_known_capacity_reservation`]).
                let reservation = crate::state::require_known_capacity_reservation(
                    &store,
                    capacity_reservation_id,
                )?;
                let reserved_instance_type = reservation.view.instance_type.clone();
                let reserved_zone = reservation.view.availability_zone.clone();
                let available = reservation.available_instance_count();
                if reserved_instance_type
                    .as_deref()
                    .is_some_and(|reserved| reserved != instance_type)
                {
                    return Err(crate::state::capacity_reservation_attributes_do_not_match(
                        capacity_reservation_id,
                        "instance type",
                        reserved_instance_type.as_deref().unwrap_or_default(),
                        &instance_type,
                    ));
                }
                // A reservation reserves capacity in ONE Availability Zone
                // (`CapacityReservation$AvailabilityZone`), so a launch placed in another
                // zone has no capacity of this reservation to take, and neither has one
                // asking for more instances than the reservation holds — the same cause,
                // which EC2 reports as the reservation's capacity being exceeded rather than
                // as a bad parameter.
                let requested_count = usize::try_from(input.max_count).unwrap_or(usize::MAX);
                if reserved_zone
                    .as_deref()
                    .is_some_and(|zone| zone != availability_zone)
                    || usize::try_from(available).unwrap_or(0) < requested_count
                {
                    return Err(crate::state::reservation_capacity_exceeded(
                        capacity_reservation_id,
                    ));
                }
                Some(capacity_reservation_id.to_string())
            }
        };
        // "Set to true to enable your instance for hibernation"
        // (`HibernationOptionsRequest$Configured`) is valid "only if the instance meets the
        // hibernation prerequisites", which are read in the ONE place this crate reads them
        // ([`crate::state::hibernation_prerequisite_refusal`]): the instance type has to be
        // one hibernation is offered on, the root device has to be an encrypted Amazon EBS
        // volume and that volume has to be large enough to hold the memory the instance runs
        // with. A launch that meets none of them is refused HERE, before the first write, so
        // it creates neither an instance nor a volume.
        if input
            .hibernation_options
            .as_ref()
            .and_then(|options| options.configured)
            .unwrap_or(false)
        {
            if let Some(refusal) = crate::state::hibernation_prerequisite_refusal(
                &instance_type,
                root_block_device.as_ref(),
            ) {
                return Err(refusal);
            }
        }
        let monitoring_state = if input
            .monitoring
            .as_ref()
            .is_some_and(|monitoring| monitoring.enabled)
        {
            // One spelling of the `MonitoringState` words for every launch path.
            MONITORING_STATE_ENABLED
        } else {
            MONITORING_STATE_DISABLED
        };
        // "The name of the key pair" (`RunInstancesRequest$KeyName`): the key pair is the
        // credential every instance of this launch is reachable with, so a name the account
        // holds no key pair under names a credential no instance of it could ever carry, and
        // the launch is refused with the key pair's own not-found class rather than placed
        // with a name that resolves to nothing — the reading `RequestSpotFleet` already gives
        // a launch specification's key pair, so one launch path cannot accept a key pair name
        // another refuses. Read BEFORE the DryRun arm, with the rest of the resources the
        // request NAMES.
        if let Some(key_name) = input
            .key_name
            .as_deref()
            .filter(|key_name| !key_name.is_empty())
        {
            if store.key_pair_id_for_name(key_name).is_none() {
                return Err(crate::state::invalid_key_pair_name_not_found(key_name));
            }
        }
        // "The name of the placement group that the instance is in"
        // (`Placement$GroupName`): a launch that names one puts every instance it mints in
        // THAT group, so the name is resolved against the placement group store here — a
        // group the account does not hold is refused with EC2's own
        // `InvalidPlacementGroup.Unknown` rather than accepted and dropped, which would
        // leave the instance reporting a group nothing holds.
        let placement_group_name = match input
            .placement
            .as_ref()
            .and_then(|placement| placement.group_name.as_deref())
            .filter(|group_name| !group_name.is_empty())
        {
            Some(group_name) => {
                if store.placement_group_id_for_name(group_name).is_none() {
                    return Err(crate::state::invalid_placement_group_unknown(group_name));
                }
                Some(group_name.to_string())
            }
            None => None,
        };
        // "The number of the partition that the instance is in. Valid only if the placement
        // group strategy is set to `partition`" (`Placement$PartitionNumber`): a launch that
        // names one places every instance it mints in THAT partition, so the number is read
        // against the partition count of the group resolved above — through the ONE reading of
        // that rule, which `ModifyInstancePlacement` shares — and a number naming no partition
        // of that group refuses the launch rather than being accepted and dropped, which would
        // leave the instance reporting no partition at all.
        let placement_partition_number = input
            .placement
            .as_ref()
            .and_then(|placement| placement.partition_number);
        if let Some(partition_number) = placement_partition_number {
            crate::state::require_partition_of_group(
                &store,
                placement_group_name.as_deref(),
                partition_number,
            )?;
        }
        // "The ID of the Dedicated Host on which the instance resides"
        // (`Placement$HostId`): a launch that names one puts every instance it mints ON that
        // host, so the id is resolved against the Dedicated Host store here — a host the
        // account does not hold, or one it has given back, is refused with EC2's own
        // `InvalidHostID.NotFound` rather than accepted and dropped, which would leave the
        // instance reporting a host nothing holds and the host reporting no instance running
        // on it.
        let placement_host_id = match input
            .placement
            .as_ref()
            .and_then(|placement| placement.host_id.as_deref())
            .filter(|host_id| !host_id.is_empty())
        {
            Some(host_id) => {
                if store
                    .dedicated_hosts
                    .get(host_id)
                    .is_none_or(crate::state::host_is_released)
                {
                    return Err(crate::state::invalid_host_id_not_found(host_id));
                }
                Some(host_id.to_string())
            }
            None => None,
        };
        // The option sets this launch chose for every instance it mints — its metadata
        // options, enclave, stop protection, shutdown behaviour, Capacity Reservation
        // preference, maintenance and network performance options, instance hostname options
        // and tenancy — resolved ONCE for the
        // batch against the account's own IMDS defaults and the AMI's `ImdsSupport`, so
        // every instance of one launch reports the same detail. A word outside its modelled
        // catalog refuses the launch HERE, before the first write.
        //
        // The AMI's own IMDS requirement is read through the store's ONE reading of it
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::image_imds_support`]), so a launch
        // from an image this store does not hold runs under the requirement the images this
        // crate publishes carry rather than under none.
        // "The credit option for CPU usage of the burstable performance instance. Valid
        // values: standard | unlimited" (`CreditSpecificationRequest$CpuCredits`): the member
        // is a plain `String` in the model with no enum behind it, so this is the only place
        // a word outside the documented pair can be refused — through the one refusal this
        // service words that with ([`crate::state::invalid_cpu_credits`]) — and what reaches
        // the instance is the documented option rather than the request's own text. A launch
        // that names none, and one of a type that has no credit option at all, are resolved
        // by the store's ONE reading of a launch's credit option, below.
        let requested_cpu_credits = match input.credit_specification.as_ref() {
            Some(specification) => Some(
                crate::state::cpu_credits_value(&specification.cpu_credits)
                    .ok_or_else(crate::state::invalid_cpu_credits)?,
            ),
            None => None,
        };
        // The credit option every instance of this launch runs under, resolved ONCE for the
        // batch: the option above where the launch named one, the account's default for the
        // instance type's family in this Region otherwise, and none at all for a type that is
        // not a burstable performance type.
        let cpu_credits = store.launched_cpu_credits(&region, &instance_type, requested_cpu_credits);
        let image_imds_support = store.image_imds_support(Some(&image_id));
        // The account's own IMDS defaults, read ONCE: they stand in for what the request left
        // out below, and the enforcement setting among them decides whether this launch may
        // happen at all.
        let account_metadata_defaults = store.instance_metadata_defaults();
        let detail: InstanceDetail = InstanceDetail::launched(InstanceDetailSpec {
            metadata_options: input.metadata_options.as_ref(),
            enclave_options: input.enclave_options.as_ref(),
            disable_api_stop: input.disable_api_stop,
            instance_initiated_shutdown_behavior: input
                .instance_initiated_shutdown_behavior
                .as_deref(),
            capacity_reservation_preference: input
                .capacity_reservation_specification
                .as_ref()
                .and_then(|specification| specification.capacity_reservation_preference.as_deref()),
            capacity_reservation_id: targeted_capacity_reservation.as_deref(),
            maintenance_options: input.maintenance_options.as_ref(),
            network_performance_options: input.network_performance_options.as_ref(),
            private_dns_name_options: input.private_dns_name_options.as_ref(),
            cpu_options: input.cpu_options.as_ref(),
            instance_type: Some(instance_type.as_str()),
            tenancy: input
                .placement
                .as_ref()
                .and_then(|placement| placement.tenancy.as_deref()),
            placement_group_name: placement_group_name.as_deref(),
            partition_number: placement_partition_number,
            host_id: placement_host_id.as_deref(),
            account_metadata_defaults: account_metadata_defaults.clone(),
            image_imds_support: image_imds_support.as_deref(),
        })?;
        // "Specifies whether to enforce the requirement of IMDSv2 on an instance at the time
        // of launch. When enforcement is enabled, the instance can't launch unless IMDSv2
        // (`HttpTokens`) is set to `required`"
        // (`ModifyInstanceMetadataDefaultsRequest$HttpTokensEnforced`): an account that
        // enforces IMDSv2 launches no instance that would leave IMDSv1 usable, so the question
        // is asked of the `HttpTokens` the instance would REALLY run with — resolved just
        // above out of the request's own metadata options, the account's defaults and the
        // AMI's `ImdsSupport` — rather than of the member the request spelled: a launch that
        // asks for `optional` and one that inherits it are the same launch to this setting,
        // and one that inherits `required` is admitted. Refused here, before the first write
        // and before the `DryRun` arm below, because the launch is one this service will not
        // perform at all.
        if account_metadata_defaults.enforces_imdsv2_at_launch()
            && detail.http_tokens != crate::state::HTTP_TOKENS_REQUIRED
        {
            return Err(crate::state::imdsv2_enforced_launch_not_supported());
        }
        // "The IDs of the security groups. You can create a security group using
        // CreateSecurityGroup" (`RunInstancesRequest$SecurityGroupIds`) and "[Default VPC] The
        // names of the security groups" (`$SecurityGroups`): which groups this launch applies
        // is read against the subnet it is placed in, and whether that is admissible is asked
        // BELOW, after the DryRun arm — the recorded answer to a dry run of a launch naming a
        // group of another VPC is `DryRunOperation`, so the flag is answered before these two
        // resources are read against each other.
        let security_groups: Vec<(Option<String>, Option<String>)> = input
            .security_group_ids
            .iter()
            .flatten()
            .map(|group_id| (Some(group_id.clone()), None))
            .chain(
                input
                    .security_groups
                    .iter()
                    .flatten()
                    .map(|group_name| (None, Some(group_name.clone()))),
            )
            .collect();
        // "The tags to apply to the resources that are created during instance launch"
        // (`TagSpecifications`): each specification tags the resources of ITS OWN type, so
        // the instance carries the `instance` entries, the EBS volume this launch creates
        // the `volume` entries and the network interfaces it creates the
        // `network-interface` ones — one reading per type
        // ([`crate::state::launch_resource_tags`]), so no entry lands on a resource it does
        // not name. A `spot-instances-request` entry names a resource this slice stores none
        // of for a launch, so its tags are carried by nothing here.
        let specifications = input.tag_specifications.as_deref().unwrap_or_default();
        let mut tags = crate::state::launch_resource_tags(
            specifications,
            crate::state::RESOURCE_TYPE_INSTANCE,
        );
        let launched_volume_tags = crate::state::tag_list(&crate::state::launch_resource_tags(
            specifications,
            crate::state::RESOURCE_TYPE_VOLUME,
        ));
        let launched_interface_tags = crate::state::tag_list(&crate::state::launch_resource_tags(
            specifications,
            crate::state::RESOURCE_TYPE_NETWORK_INTERFACE,
        ));
        // A launch that READ A LAUNCH TEMPLATE adds the two system tags Amazon EC2 puts on an
        // instance launched from one, naming the template and the version it read
        // ([`crate::state::LAUNCH_TEMPLATE_ID_TAG`]): they are the service's own, so they are
        // written after the caller's and cannot be set — or overwritten — by a request.
        if let Some((launch_template_id, version)) = launched_template.as_ref() {
            tags.insert(
                crate::state::LAUNCH_TEMPLATE_ID_TAG.to_string(),
                launch_template_id.clone(),
            );
            tags.insert(
                crate::state::LAUNCH_TEMPLATE_VERSION_TAG.to_string(),
                version.to_string(),
            );
        }

        // Each instance retains a copy of EVERY caller-chosen string of the launch —
        // its user data, tags and security groups, and equally the image id, instance
        // type, subnet, key pair name and zone — so the launch has a byte size as well
        // as a count, and the size asked for here is the one
        // `crate::state::instance_payload_bytes` charges per instance below.
        let requested = usize::try_from(input.max_count).unwrap_or(usize::MAX);
        let per_instance_bytes = input.user_data.as_ref().map_or(0, String::len)
            + input.client_token.as_ref().map_or(0, String::len)
            // The RESOLVED image id, which is the value the instance record keeps: the charge
            // is for what this launch STORES, not for the parameter the request carried.
            + image_id.len()
            + instance_type.len()
            + input.subnet_id.as_ref().map_or(0, String::len)
            + input.key_name.as_ref().map_or(0, String::len)
            + input.kernel_id.as_ref().map_or(0, String::len)
            + input.ramdisk_id.as_ref().map_or(0, String::len)
            + availability_zone.len()
            // The placement group name the launch resolved is copied onto every instance it
            // mints, exactly as the zone is.
            + placement_group_name.as_ref().map_or(0, String::len)
            // The association each instance of the launch is given retains the profile
            // reference the request named, charged by the one definition of that charge
            // beside the per-record overhead the store's writer seam adds to it — so the
            // batch ceiling below covers every record the launch stores, not only the
            // instances.
            + iam_instance_profile.as_ref().map_or(0, |profile| {
                iam_instance_profile_association_payload_bytes(profile)
                    .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
            })
            + crate::state::tag_bytes(&tags)
            // The tags the launch asks for on the resources it CREATES beside the instance
            // are retained once per such RECORD, not once per launch: one volume per device
            // this launch creates (the root one and each device its own mapping names) and
            // one interface per interface it creates — plus the copy the instance record
            // itself keeps for those seams to read
            // ([`crate::state::instance_payload_bytes`] charges that one). So the charge
            // counts the PRODUCT of the list's size and the records that hold it, rather
            // than one factor of it.
            + crate::state::tag_list_bytes(&launched_volume_tags).saturating_mul(
                data_block_devices.len().saturating_add(2),
            )
            + crate::state::tag_list_bytes(&launched_interface_tags).saturating_mul(
                planned_interfaces
                    .iter()
                    .filter(|interface| interface.request.network_interface_id.is_none())
                    .count()
                    .max(1)
                    .saturating_add(1),
            )
            // The root block device this launch resolved is kept on every instance it mints,
            // charged by the one definition of that charge.
            + crate::state::root_block_device_bytes(root_block_device.as_ref())
            + security_groups
                .iter()
                .map(|(group_id, group_name)| {
                    group_id.as_ref().map_or(0, String::len)
                        + group_name.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
            // Each interface the launch creates for the instance is a stored record of its own,
            // charged what the interface store will charge for it ([`crate::state::network_interface_spec_bytes`]'s
            // three members, of which a launch names the description and the addresses)
            // beside the per-record overhead — so the batch ceiling below covers every record
            // this launch stores rather than only the instances.
            + planned_interfaces
                .iter()
                .filter(|interface| interface.request.network_interface_id.is_none())
                .map(|interface| {
                    interface
                        .request
                        .description
                        .len()
                        .saturating_add(
                            interface
                                .request
                                .addresses
                                .named
                                .iter()
                                .map(String::len)
                                .sum::<usize>(),
                        )
                        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
                })
                .sum::<usize>();
        let batch_bytes = per_instance_bytes.saturating_mul(requested);

        // Every remaining check is READ-ONLY, and a replay clears them all by
        // storing nothing.
        if replayed.is_none() {
            // The store keeps a terminated instance (the real service reports it for
            // a while), so its growth is bounded here rather than by anything the
            // clock reclaims.
            if store.instances.len().saturating_add(requested) > MAX_STORED_INSTANCES {
                return Err(Scaffolded::instance_limit_exceeded(format!(
                    "This emulator holds {} of at most {MAX_STORED_INSTANCES} instances, \
                     so it cannot launch {requested} more",
                    store.instances.len()
                )));
            }
            // One association per instance the launch mints, so the association store's
            // own ceiling bounds a launch that names a profile the way the instance
            // store's bounds every launch — asked here, before the first write, so a
            // launch that cannot be given them all leaves nothing behind.
            if iam_instance_profile.is_some()
                && store
                    .iam_instance_profile_associations
                    .len()
                    .saturating_add(requested)
                    > MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS
            {
                return Err(Scaffolded::request_limit_exceeded(format!(
                    "This emulator holds {} of at most \
                     {MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS} IAM instance profile \
                     associations, so it cannot associate {requested} more",
                    store.iam_instance_profile_associations.len()
                )));
            }
            if store.would_exceed_stored_bytes(batch_bytes) {
                return Err(Scaffolded::instance_limit_exceeded(format!(
                    "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                     caller-supplied instance data, so it cannot store {batch_bytes} more",
                    store.stored_bytes()
                )));
            }
            // An explicit primary private address must be free: EC2 refuses a second
            // launch onto an address another instance already holds.
            if let Some(address) = input.private_ip_address.as_deref() {
                if store.private_ip_in_use(address) {
                    return Err(Scaffolded::invalid_ip_address_in_use(address));
                }
            }
            // The same of every address an interface of the launch names for itself: it is
            // the same pool, so an address another resource of the account holds cannot be
            // given to an interface either. Asked here, read-only, so a launch that could
            // not be given its addresses is refused before the first write — and asked only
            // of the interfaces the launch CREATES, since an existing one it attaches holds
            // its own addresses already.
            for interface in &planned_interfaces {
                if interface.request.network_interface_id.is_some() {
                    continue;
                }
                for address in &interface.request.addresses.named {
                    if store.private_ip_in_use(address) {
                        return Err(Scaffolded::invalid_ip_address_in_use(address));
                    }
                }
            }
            // Every interface of every instance the launch mints is a record of the
            // interface store, so that store's own ceiling bounds an interface-named launch
            // the way the instance store's bounds every launch.
            let interfaces_to_create = planned_interfaces
                .iter()
                .filter(|interface| interface.request.network_interface_id.is_none())
                .count()
                .saturating_mul(requested);
            if store
                .network_interfaces
                .len()
                .saturating_add(interfaces_to_create)
                > MAX_NETWORK_INTERFACES
            {
                return Err(crate::state::network_interface_limit_exceeded());
            }
        }

        // The probe's own answer, now that the request's values have been read and every
        // resource it NAMES resolved — the image, the instance profile, the key pair, the
        // placement group, the Dedicated Host, the Capacity Reservation: "if you have the
        // required permissions, the error response is DryRunOperation" (`DryRun`). A launch
        // that would have been refused for one of its own members is refused with that
        // reason instead; one that got this far is answered the verdict, and the refusals
        // that come out of ASSEMBLING the launch — reading its groups against the subnet it
        // would sit in, and the interface plan skipped above — belong to the real call.
        if probe {
            return Err(Scaffolded::dry_run_operation());
        }

        // The groups this launch applies, read against the subnet it is placed in: a group the
        // account holds none of is a reference to nothing, and a group of ANOTHER VPC could
        // never be applied to an interface in this subnet. Asked here, after the DryRun arm,
        // for the reason given where `security_groups` is built.
        refuse_incompatible_security_groups(
            &store,
            store
                .launch_subnet(input.subnet_id.as_deref(), &availability_zone)
                .map(|subnet| {
                    (
                        subnet.subnet_id.clone().unwrap_or_default(),
                        subnet.vpc_id.clone().unwrap_or_default(),
                    )
                }),
            &security_groups,
        )?;
        // The same two rules for every OTHER interface of the launch, each read against its
        // own subnet: one reading of "may these groups be applied in this subnet" for every
        // interface of the instance, rather than one rule for the primary interface and
        // another for the rest.
        for interface in &planned_interfaces {
            if interface.request.device_index == PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX
                || interface.request.network_interface_id.is_some()
            {
                continue;
            }
            let named: Vec<(Option<String>, Option<String>)> = interface
                .request
                .group_ids
                .iter()
                .map(|group_id| (Some(group_id.clone()), None))
                .collect();
            refuse_incompatible_security_groups(
                &store,
                Some((interface.subnet_id.clone(), interface.vpc_id.clone())),
                &named,
            )?;
        }

        if let Some(reservation_id) = replayed {
            return store.whole_reservation_view(&reservation_id).ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "a client token names reservation '{reservation_id}', \
                     which is not stored"
                ))
            });
        }

        // Every address is taken BEFORE the reservation is stored, so a launch that
        // cannot be given one leaves no partial reservation behind — and the
        // addresses it did take go straight back to the pool.
        //
        // "If you don't specify this address, we choose one from the IPv4 range of your
        // subnet" (`RunInstancesRequest$PrivateIpAddress`): a launch that is placed in a
        // subnet this account holds ([`crate::state::ComAmazonawsEc2Amazonec2State::launch_subnet`],
        // the named one or the zone's default) draws from THAT subnet's block, through the
        // one choice-and-take every subnet address of this crate comes out of, so the
        // instance, its primary network interface and its subnet cannot disagree about where
        // the address came from.
        let subnet_cidr_block = store
            .launch_subnet(input.subnet_id.as_deref(), &availability_zone)
            .and_then(|subnet| subnet.cidr_block.clone());
        // The interfaces of each instance the launch mints, with their addresses taken — for
        // a launch that named its interfaces. The primary interface of each set carries that
        // instance's own primary private address, so the two lists below are one resolution
        // rather than two: `addresses[i]` is the primary address of `launched_interfaces[i]`.
        let mut addresses: Vec<String> = Vec::new();
        let mut launched_interfaces: Vec<Vec<LaunchedInterface>> = Vec::new();
        if !planned_interfaces.is_empty() {
            for _ in 0..input.max_count {
                let interfaces = match take_launch_interfaces(&mut store, &planned_interfaces) {
                    Ok(interfaces) => interfaces,
                    Err(refusal) => {
                        release_taken_interfaces(&mut store, &launched_interfaces);
                        return Err(refusal);
                    }
                };
                let primary = interfaces
                    .iter()
                    .find(|interface| {
                        interface.device_index == PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX
                    })
                    .and_then(|interface| interface.private_ip_addresses.first().cloned());
                match primary {
                    Some(private_ip) => addresses.push(private_ip),
                    None => {
                        release_taken_interfaces(&mut store, &launched_interfaces);
                        release_taken_interfaces(&mut store, &[interfaces]);
                        return Err(Scaffolded::insufficient_free_addresses());
                    }
                }
                launched_interfaces.push(interfaces);
            }
        } else {
            let mut exhausted = false;
            for _ in 0..input.max_count {
                match input.private_ip_address.clone() {
                    Some(address) => addresses.push(address),
                    None => {
                        let chosen = match subnet_cidr_block.as_deref() {
                            Some(cidr_block) => store.mint_private_ip_in_cidr(cidr_block),
                            None => store.allocate_private_ip(),
                        };
                        match chosen {
                            Some(address) => addresses.push(address),
                            None => {
                                exhausted = true;
                                break;
                            }
                        }
                    }
                }
            }
            if exhausted {
                store.release_private_ips(&addresses);
                return Err(Scaffolded::insufficient_free_addresses());
            }
        }

        let reservation_id = store.mint_reservation_id();
        store.insert_reservation(ReservationRecord {
            reservation_id: reservation_id.clone(),
            owner_id: ::engine::emulator_account_id().to_string(),
            requester_id: None,
            instance_ids: Vec::new(),
        });

        let launch_time = ::std::time::SystemTime::now();
        // The kernel and RAM disk the AMI carries, which an instance runs when the launch
        // names neither, read once for the whole batch.
        let (image_kernel_id, image_ramdisk_id) = store.image_kernel_and_ramdisk(Some(&image_id));
        for (launch_index, private_ip) in addresses.into_iter().enumerate() {
            let instance_id = store.mint_instance_id();
            // The interfaces THIS instance is given: the ones its launch named, with the
            // addresses already taken for it, or none for a launch that named none — which
            // gets the single primary interface the store creates for such a launch.
            let interfaces = launched_interfaces.get(launch_index).cloned().unwrap_or_default();
            let stored_all_interfaces = store.insert_launched_instance(
                InstanceRecord {
                instance_id: instance_id.clone(),
                reservation_id: reservation_id.clone(),
                image_id: Some(image_id.clone()),
                instance_type: instance_type.clone(),
                key_name: input.key_name.clone(),
                subnet_id: input.subnet_id.clone(),
                security_groups: security_groups.clone(),
                private_dns_name: private_dns_name(&private_ip, &region),
                private_ip,
                // A public address comes from an Elastic IP, or from the Amazon pool when the
                // launch asked its primary interface for one — assigned with that interface,
                // below, so the instance and the interface report one address.
                public_ip: None,
                // Whether Amazon maps one to this instance is decided where that address is
                // assigned, with the interface it goes to below.
                mapped_public_ipv4: false,
                state: InstanceStateName::Pending,
                pending_transition: Some(PendingTransition::starting_now(
                    InstanceStateName::Running,
                )),
                state_transition_reason: None,
                state_reason: None,
                // Nobody has asked to reboot an instance that has just been launched, and
                // nobody has interrupted it either.
                queued_reboots: 0,
                last_reboot_queued_at: None,
                diagnostic_interrupts: 0,
                last_diagnostic_interrupt_at: None,
                // The guest's FIRST boot: the operating system comes up as the instance
                // finishes starting, which is the same instant the launch's own
                // `pending` -> `running` move falls due.
                boots: Vec::new(),
                // The guest's own storage starts empty: what is on it is what the
                // instance's user-data program puts there, boot by boot.
                guest_files: ::std::collections::BTreeMap::new(),
                // And it is running nothing: what it runs is what that program installs and
                // starts ([`crate::state::guest_services`]).
                guest_runtime: crate::state::GuestRuntime::default(),
                boot_due: Some(launch_time + TRANSITION_DELAY),
                launch_time,
                ami_launch_index: i32::try_from(launch_index).unwrap_or(i32::MAX),
                user_data: input.user_data.clone(),
                placement_az: availability_zone.clone(),
                monitoring_state: monitoring_state.to_string(),
                // The launch says whether detailed monitoring is on from the start, so there
                // is no move for the clock to make; `MonitorInstances` is what records one.
                monitoring_transition: None,
                detail: detail.clone(),
                architecture: architecture_for(&instance_type),
                // "The ID of the kernel" (`RunInstancesRequest$KernelId`) the launch
                // named, or the one its image carries when it named none — the value
                // `ResetInstanceAttribute` restores the attribute to.
                kernel_id: input.kernel_id.clone().or_else(|| image_kernel_id.clone()),
                ramdisk_id: input
                    .ramdisk_id
                    .clone()
                    .or_else(|| image_ramdisk_id.clone()),
                // Nothing in a launch sets source/destination checking: it starts at the
                // documented default and only `ModifyInstanceAttribute` moves it.
                source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
                ebs_optimized: input.ebs_optimized.unwrap_or(false),
                // Nothing in a launch sets the instance's OWN enhanced networking: it
                // reports the AMI's until `ModifyInstanceAttribute` gives it a value.
                sriov_net_support: None,
                ena_support: None,
                hibernation_configured: input
                    .hibernation_options
                    .as_ref()
                    .and_then(|options| options.configured)
                    .unwrap_or(false),
                // The root device every instance of this launch is created with, resolved
                // once above against the AMI's own mapping and this Region's encryption
                // default.
                root_block_device: root_block_device.clone(),
                // And every other Amazon EBS device the launch's mapping names, resolved
                // once above by the same reading: each is a volume this launch creates and
                // the instance holds at that device.
                data_block_devices: data_block_devices.clone(),
                disable_api_termination: input.disable_api_termination.unwrap_or(false),
                tags: tags.clone(),
                // The tags this launch asks for on the resources it CREATES beside the
                // instance: the EBS volume and the network interfaces the store mints with
                // the instance carry them from the moment they exist.
                launched_volume_tags: launched_volume_tags.clone(),
                launched_interface_tags: launched_interface_tags.clone(),
                client_token: input.client_token.clone(),
                // An on-demand launch fulfills no Spot Instance request, so the instance
                // carries neither a request id nor the `spot` lifecycle.
                spot_instance_request_id: None,
                // The credit option this launch resolved for its burstable instances, which
                // every instance of the batch runs under; `None` for a type that has no
                // credit option, reported `standard`.
                cpu_credits,
                },
                &interfaces,
            );
            // The interface store had room for every interface of this launch when the
            // ceilings above were asked, under the one lock this handler holds; a store that
            // answers otherwise has not stored what this launch would report, and says so
            // rather than reporting an instance holding interfaces nothing holds.
            if !stored_all_interfaces {
                return Err(crate::state::network_interface_limit_exceeded());
            }
            // The profile the launch named is recorded over the instance as an association
            // of its own, in the non-terminal `associating` state the clock then moves to
            // `associated` — so a caller that reads straight after the launch still sees
            // the transient state — and the instance reports the profile off that same
            // record, so the launch, the instance report and the association report cannot
            // disagree about one instance's profile. Both of the store's ceilings were
            // asked above, so the seam has room; a store that answers otherwise has not
            // stored what this launch reports, and says so rather than reporting a profile
            // no association holds.
            if let Some(profile) = iam_instance_profile.clone() {
                if store
                    .insert_iam_instance_profile_association(IamInstanceProfileAssociationSpec {
                        instance_id: instance_id.clone(),
                        iam_instance_profile: profile,
                    })
                    .is_none()
                {
                    return Err(Scaffolded::internal_error(format!(
                        "the IAM instance profile association for instance \
                         '{instance_id}' was not stored"
                    )));
                }
            }
            // And the instance is taken back where the key its volumes are to be encrypted
            // under refused their data key, asked of the service that owns that key above:
            // the instance exists and is `pending`, with the clock owing it `terminated`
            // under the state reason that says why instead of `running`.
            if let Some(refusal) = volume_key_refusal.as_ref() {
                store.withdraw_launch_under_unusable_volume_key(&instance_id, refusal);
            }
        }

        if let Some(token) = input.client_token.clone() {
            store.client_tokens.insert(
                token,
                crate::state::LaunchClaim {
                    reservation_id: reservation_id.clone(),
                    parameters: launch_digest,
                },
            );
        }

        // The hosts this launch put instances on now report them: a Dedicated Host's
        // `Instances` follows from where the account's instances were launched
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::resync_dedicated_host_instances`]),
        // so it is recomputed here rather than written beside the instance records — which is
        // what makes a host carrying an instance refuse to be released under it.
        store.resync_dedicated_host_instances();

        // The moves every instance of this launch makes, published for whatever watches
        // instances: the `pending` it is in now and the state the clock owes it — `running`,
        // or the `terminated` a volume that cannot be encrypted owes it — each when that move
        // falls due ([`crate::state::InstanceStateNotification`]). Rendered from the STORED
        // instances, so what is published is what the store settled on, and handed on below
        // with the guard already given back: a lock of this store is never held while another
        // service is called.
        let launched: Vec<String> = store
            .reservations
            .get(&reservation_id)
            .map(|reservation| reservation.instance_ids.clone())
            .unwrap_or_default();
        let moves =
            store.instance_state_change_notifications(&launched, ::std::time::Instant::now());
        let answer = store
            .whole_reservation_view(&reservation_id)
            .ok_or_else(|| Scaffolded::internal_error("the launched reservation was not stored"));
        drop(store);
        let answer = answer?;
        for notification in moves {
            notification.publish_when_due(state.clone(), ctx.callback().clone());
        }
        Ok(answer)
    }
}

/// The launch `request` asks for, overlaid with the launch template version `data` it named.
///
/// "Any additional parameters that you specify for the new instance overwrite the
/// corresponding parameters included in the launch template"
/// (`RunInstancesRequest$LaunchTemplate`), so EVERY member here is the request's own when the
/// request carried one and the template version's otherwise — one rule, applied member by
/// member, never a branch on what the recorded template happened to hold. A list member counts
/// as carried when the request named at least one entry: a launch that names its own security
/// groups replaces the template's rather than adding to them, the groups of an instance being
/// one set.
///
/// The members whose request and template shapes differ only in the shape's NAME are rebuilt
/// member by member ([`template_placement`] and the rest below), so nothing a template holds is
/// dropped on the way into the launch.
///
/// One member is NOT overlaid, because the launch path reads it from a request either, so
/// taking it from a template would claim something the launch does not do: the retired
/// elastic graphics and inference accelerators ("Amazon Elastic Graphics reached end of life
/// on January 8, 2024", "Amazon Elastic Inference is no longer available"). A template whose
/// version names the block device mappings of its instance is read through
/// [`crate::state::template_block_device_mapping`] and one that names its network interfaces
/// through [`template_primary_interface`].
fn launch_overlaid_with_template(
    request: RunInstancesInput,
    data: &RequestLaunchTemplateData,
) -> Result<RunInstancesInput, RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    // "If you specify InstanceRequirements, you can't specify InstanceType … attribute-based
    // instance type selection is only supported when using Auto Scaling groups, EC2 Fleet, and
    // Spot Fleet to launch instances" (`RequestLaunchTemplateData$InstanceRequirements`): a
    // version that describes the instance type by ATTRIBUTES names no instance type for a
    // launch to run, and this operation is not one of the three that can select one from them
    // — so a launch whose type would have to come from them is refused rather than placed on
    // an instance type nothing chose. A request that names its own `InstanceType` decides it
    // itself, and the attributes are moot.
    if data.instance_requirements.is_some()
        && request.instance_type.is_none()
        && data.instance_type.is_none()
    {
        return Err(Scaffolded::invalid_parameter_value(
            "Invalid value 'InstanceRequirements' for launchTemplate. Attribute-based \
             instance type selection is only supported when using Auto Scaling groups, EC2 \
             Fleet, and Spot Fleet to launch instances",
        ));
    }

    let mut merged = request;
    merged.image_id = merged.image_id.or_else(|| data.image_id.clone());
    merged.instance_type = merged.instance_type.or_else(|| data.instance_type.clone());
    merged.key_name = merged.key_name.or_else(|| data.key_name.clone());
    merged.user_data = merged.user_data.or_else(|| data.user_data.clone());
    merged.kernel_id = merged.kernel_id.or_else(|| data.kernel_id.clone());
    // The template spells the RAM disk `RamDiskId` where the launch spells it `RamdiskId`;
    // both name the same thing.
    merged.ramdisk_id = merged.ramdisk_id.or_else(|| data.ram_disk_id.clone());
    merged.ebs_optimized = merged.ebs_optimized.or(data.ebs_optimized);
    merged.disable_api_termination = merged
        .disable_api_termination
        .or(data.disable_api_termination);
    merged.disable_api_stop = merged.disable_api_stop.or(data.disable_api_stop);
    merged.instance_initiated_shutdown_behavior = merged
        .instance_initiated_shutdown_behavior
        .or_else(|| data.instance_initiated_shutdown_behavior.clone());
    merged.credit_specification = merged
        .credit_specification
        .or_else(|| data.credit_specification.clone());
    merged.monitoring = merged.monitoring.or_else(|| {
        data.monitoring
            .as_ref()
            .map(|monitoring| RunInstancesMonitoringEnabled {
                // "Specifies whether detailed monitoring is enabled for the instance"
                // (`RunInstancesMonitoringEnabled$Enabled`) is required of a launch and
                // optional on a template, so a template that holds the option without
                // answering it is not asking for detailed monitoring.
                enabled: monitoring.enabled.unwrap_or(false),
            })
    });
    merged.placement = merged
        .placement
        .or_else(|| data.placement.as_ref().map(template_placement));
    merged.iam_instance_profile = merged.iam_instance_profile.or_else(|| {
        data.iam_instance_profile
            .as_ref()
            .map(|profile| IamInstanceProfileSpecification {
                arn: profile.arn.clone(),
                name: profile.name.clone(),
            })
    });
    merged.hibernation_options = merged.hibernation_options.or_else(|| {
        data.hibernation_options
            .as_ref()
            .map(|options| HibernationOptionsRequest {
                configured: options.configured,
            })
    });
    // "The block device mapping" (`RequestLaunchTemplateData$BlockDeviceMappings`) a template
    // holds applies to every launch made from it, and the launch's own entries replace it the
    // way its other list members do — so a template-driven launch and the equivalent
    // spelled-out one create the same root volume.
    if merged
        .block_device_mappings
        .as_ref()
        .is_none_or(Vec::is_empty)
    {
        merged.block_device_mappings = data.block_device_mappings.as_ref().map(|mappings| {
            mappings
                .iter()
                .map(crate::state::template_block_device_mapping)
                .collect::<Vec<BlockDeviceMapping>>()
        });
    }
    merged.metadata_options = merged.metadata_options.or_else(|| {
        data.metadata_options
            .as_ref()
            .map(|options| InstanceMetadataOptionsRequest {
                http_tokens: options.http_tokens.clone(),
                http_put_response_hop_limit: options.http_put_response_hop_limit,
                http_endpoint: options.http_endpoint.clone(),
                http_protocol_ipv6: options.http_protocol_ipv6.clone(),
                instance_metadata_tags: options.instance_metadata_tags.clone(),
            })
    });
    merged.enclave_options = merged.enclave_options.or_else(|| {
        data.enclave_options
            .as_ref()
            .map(|options| EnclaveOptionsRequest {
                enabled: options.enabled,
            })
    });
    merged.maintenance_options = merged.maintenance_options.or_else(|| {
        data.maintenance_options
            .as_ref()
            .map(|options| InstanceMaintenanceOptionsRequest {
                auto_recovery: options.auto_recovery.clone(),
            })
    });
    merged.network_performance_options = merged.network_performance_options.or_else(|| {
        data.network_performance_options
            .as_ref()
            .map(|options| InstanceNetworkPerformanceOptionsRequest {
                bandwidth_weighting: options.bandwidth_weighting.clone(),
            })
    });
    merged.private_dns_name_options = merged.private_dns_name_options.or_else(|| {
        data.private_dns_name_options
            .as_ref()
            .map(|options| PrivateDnsNameOptionsRequest {
                hostname_type: options.hostname_type.clone(),
                enable_resource_name_dns_a_record: options.enable_resource_name_dns_a_record,
                enable_resource_name_dns_aaaa_record: options.enable_resource_name_dns_aaaa_record,
            })
    });
    merged.capacity_reservation_specification =
        merged.capacity_reservation_specification.or_else(|| {
            data.capacity_reservation_specification
                .as_ref()
                .map(|specification| CapacityReservationSpecification {
                    capacity_reservation_preference: specification
                        .capacity_reservation_preference
                        .clone(),
                    capacity_reservation_target: specification.capacity_reservation_target.clone(),
                })
        });
    merged.cpu_options = merged.cpu_options.or_else(|| {
        data.cpu_options.as_ref().map(|options| CpuOptionsRequest {
            core_count: options.core_count,
            threads_per_core: options.threads_per_core,
            amd_sev_snp: options.amd_sev_snp.clone(),
            nested_virtualization: options.nested_virtualization.clone(),
        })
    });
    merged.instance_market_options = merged.instance_market_options.or_else(|| {
        data.instance_market_options
            .as_ref()
            .map(|options| InstanceMarketOptionsRequest {
                market_type: options.market_type.clone(),
                spot_options: options.spot_options.as_ref().map(|spot| SpotMarketOptions {
                    max_price: spot.max_price.clone(),
                    spot_instance_type: spot.spot_instance_type.clone(),
                    block_duration_minutes: spot.block_duration_minutes,
                    valid_until: spot.valid_until,
                    instance_interruption_behavior: spot.instance_interruption_behavior.clone(),
                }),
            })
    });
    if merged
        .license_specifications
        .as_ref()
        .is_none_or(Vec::is_empty)
    {
        merged.license_specifications = data.license_specifications.as_ref().map(|entries| {
            entries
                .iter()
                .map(|entry| LicenseConfigurationRequest {
                    license_configuration_arn: entry.license_configuration_arn.clone(),
                })
                .collect()
        });
    }
    if merged.tag_specifications.as_ref().is_none_or(Vec::is_empty) {
        // "The tags to apply to the resources that are created during instance launch. These
        // tags are not applied to the launch template"
        // (`RequestLaunchTemplateData$TagSpecifications`) — the same member a launch carries,
        // so which resource type each entry tags is decided where a request's own entries are.
        merged.tag_specifications = data.tag_specifications.as_ref().map(|entries| {
            entries
                .iter()
                .map(|entry| TagSpecification {
                    resource_type: entry.resource_type.clone(),
                    tags: entry.tags.clone(),
                })
                .collect()
        });
    }
    if merged.security_group_ids.as_ref().is_none_or(Vec::is_empty) {
        merged.security_group_ids = data.security_group_ids.clone();
    }
    if merged.security_groups.as_ref().is_none_or(Vec::is_empty) {
        merged.security_groups = data.security_groups.clone();
    }

    // "The network interfaces for the instance" (`RequestLaunchTemplateData$NetworkInterfaces`)
    // is how a template names the interfaces of the instance it launches — the same facts a
    // request names on its own `NetworkInterfaces` — so they are read into THAT member and a
    // template-driven launch takes exactly the path a spelled-out one takes. The request's own
    // interfaces win, as everywhere else in the overlay.
    //
    // The subnet, the security groups and the primary private address are NOT overlaid onto
    // that interface: a launch names each of those three facts either on its own members or on
    // its interfaces and never both ("if you specify a network interface, you must specify any
    // subnets / security groups as part of the network interface instead of using this
    // parameter"), so a launch that names interfaces and one of them is refused by the caller
    // of this overlay rather than served from one of the two places.
    if merged
        .network_interfaces
        .as_ref()
        .is_none_or(Vec::is_empty)
    {
        merged.network_interfaces = data
            .network_interfaces
            .as_ref()
            .map(|interfaces| interfaces.iter().map(template_interface).collect());
    }
    Ok(merged)
}

/// One `NetworkInterfaces` entry of a launch template version, as the same entry of a launch.
///
/// Member by member rather than a conversion written once, for the reason
/// [`template_placement`] is: the two shapes differ only in the name of the shape and in how
/// each spells an IPv6 address, so nothing a template says about an interface is dropped on
/// the way into the launch, and a member added to either shape later shows up here as a
/// compile error rather than as a silently dropped setting.
fn template_interface(
    interface: &LaunchTemplateInstanceNetworkInterfaceSpecificationRequest,
) -> InstanceNetworkInterfaceSpecification {
    InstanceNetworkInterfaceSpecification {
        associate_public_ip_address: interface.associate_public_ip_address,
        delete_on_termination: interface.delete_on_termination,
        description: interface.description.clone(),
        device_index: interface.device_index,
        groups: interface.groups.clone(),
        ipv6_address_count: interface.ipv6_address_count,
        ipv6_addresses: interface.ipv6_addresses.as_ref().map(|addresses| {
            addresses
                .iter()
                .map(|address| InstanceIpv6Address {
                    ipv6_address: address.ipv6_address.clone(),
                    is_primary_ipv6: None,
                })
                .collect()
        }),
        network_interface_id: interface.network_interface_id.clone(),
        private_ip_address: interface.private_ip_address.clone(),
        private_ip_addresses: interface.private_ip_addresses.clone(),
        secondary_private_ip_address_count: interface.secondary_private_ip_address_count,
        subnet_id: interface.subnet_id.clone(),
        associate_carrier_ip_address: interface.associate_carrier_ip_address,
        interface_type: interface.interface_type.clone(),
        network_card_index: interface.network_card_index,
        ipv4_prefixes: interface.ipv4_prefixes.clone(),
        ipv4_prefix_count: interface.ipv4_prefix_count,
        ipv6_prefixes: interface.ipv6_prefixes.clone(),
        ipv6_prefix_count: interface.ipv6_prefix_count,
        primary_ipv6: interface.primary_ipv6,
        ena_srd_specification: interface.ena_srd_specification.clone(),
        connection_tracking_specification: interface.connection_tracking_specification.clone(),
        ena_queue_count: interface.ena_queue_count,
    }
}

/// The `Placement` a launch template version's placement describes.
///
/// Member by member rather than a conversion written once: the two shapes differ only in the
/// name of the shape, so the launch is placed exactly where the template says — a partition, a
/// Dedicated Host, a host resource group, a spread domain and a tenancy included — and a member
/// added to either shape later shows up here as a compile error rather than as a silently
/// dropped placement.
fn template_placement(placement: &LaunchTemplatePlacementRequest) -> Placement {
    Placement {
        availability_zone: placement.availability_zone.clone(),
        availability_zone_id: placement.availability_zone_id.clone(),
        affinity: placement.affinity.clone(),
        group_name: placement.group_name.clone(),
        group_id: placement.group_id.clone(),
        partition_number: placement.partition_number,
        host_id: placement.host_id.clone(),
        tenancy: placement.tenancy.clone(),
        spread_domain: placement.spread_domain.clone(),
        host_resource_group_arn: placement.host_resource_group_arn.clone(),
    }
}

/// Refuse a launch whose security groups cannot be applied in the subnet it is placed in.
///
/// `launch_subnet` is the (`SubnetId`, `VpcId`) of the subnet the launch lands in
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::launch_subnet`]), or `None` when the account
/// holds no subnet for it — there is then no network to read the groups against, so nothing is
/// refused here. `named` is the launch's groups as ids and names, in the order the request
/// gave them, so the group the refusal names is the one the caller sent.
///
/// Two rules, both about one reference:
///
/// * a group the account holds none of is a reference to nothing, so the launch is refused with
///   the group's own `InvalidGroup.NotFound` rather than answered with an instance reporting a
///   group no describe can report. A NAME is resolved in the VPC the launch is placed in —
///   "[Default VPC] The names of the security groups" (`RunInstancesRequest$SecurityGroups`),
///   and a name is unique within one VPC — which is the same reading
///   [`crate::state::ComAmazonawsEc2Amazonec2State::group_identifier_of`] completes a named
///   group with;
/// * a security group belongs to ONE VPC, while the instance's primary network interface sits
///   in the subnet's VPC, so a group of another VPC could never be applied to this instance:
///   EC2 refuses the launch `InvalidParameter`, naming the group and the subnet that do not
///   meet.
fn refuse_incompatible_security_groups(
    store: &crate::state::ComAmazonawsEc2Amazonec2State,
    launch_subnet: Option<(String, String)>,
    named: &[(Option<String>, Option<String>)],
) -> Result<(), RunInstancesError> {
    let Some((subnet_id, vpc_id)) = launch_subnet else {
        return Ok(());
    };
    for (group_id, group_name) in named {
        let resolved_group_id = match (group_id.as_deref(), group_name.as_deref()) {
            (Some(group_id), _) => store
                .security_groups
                .get(group_id)
                .map(|_| group_id.to_string())
                .ok_or_else(|| crate::state::invalid_group_not_found(group_id))?,
            (None, Some(group_name)) => store
                .security_groups
                .values()
                .find(|group| {
                    group.group_name.as_deref() == Some(group_name)
                        && group.vpc_id.as_deref() == Some(vpc_id.as_str())
                })
                .map(|group| group.group_id.clone().unwrap_or_default())
                .ok_or_else(|| {
                    // The name was resolved IN the launch subnet's VPC, so the sentence names
                    // the network the lookup was scoped to: a name is only a name INSIDE one
                    // network, so which network it was looked up in is what the caller has to
                    // correct. A RULE's referenced group is named by an id instead, which
                    // identifies one group account-wide, so that path answers EC2's generic
                    // different-networks sentence rather than this one
                    // (`crate::state::RESOURCES_IN_DIFFERENT_NETWORKS`).
                    crate::scaffolded::Scaffolded::service_error(
                        crate::state::INVALID_GROUP_NOT_FOUND,
                        400,
                        format!(
                            "The security group '{group_name}' does not exist in VPC '{vpc_id}'"
                        ),
                    )
                })?,
            (None, None) => continue,
        };
        // A security group is applied to the instance's primary network interface, which
        // sits in the subnet's VPC: a group that VPC cannot use could never be applied to
        // this instance. "Usable" is the crate's one reading of that question
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::security_group_usable_in_vpc`]) —
        // the group was created in that VPC, or `AssociateSecurityGroupVpc` has associated
        // it with that VPC — and EC2 refuses a launch that names any other group
        // `InvalidParameter`, naming the group and the subnet that do not meet.
        if !store.security_group_usable_in_vpc(
            &resolved_group_id,
            vpc_id.as_str(),
            ::std::time::Instant::now(),
        ) {
            return Err(crate::scaffolded::Scaffolded::service_error(
                crate::state::INVALID_PARAMETER,
                400,
                format!(
                    "Security group {resolved_group_id} and subnet {subnet_id} belong to \
                     different networks."
                ),
            ));
        }
    }
    Ok(())
}

/// The types a launch may ask one of its network interfaces for.
///
/// "Valid values: `interface` | `efa` | `efa-only`"
/// (`InstanceNetworkInterfaceSpecification$InterfaceType`) — a member the model targets at a
/// plain string rather than at an enum, so the boundary validator refuses nothing for it and
/// this is where a launch is held to the catalog that member's documentation publishes.
const LAUNCH_INTERFACE_TYPES: &[&str] = &[
    crate::state::NETWORK_INTERFACE_TYPE_INTERFACE,
    LAUNCH_INTERFACE_TYPE_EFA,
    crate::state::NETWORK_INTERFACE_TYPE_EFA_ONLY,
];

/// The Elastic Fabric Adapter member of [`LAUNCH_INTERFACE_TYPES`], which no other path of
/// this crate names.
const LAUNCH_INTERFACE_TYPE_EFA: &str = "efa";

/// The most interfaces a launch may name when this service derives no interface limit for its
/// instance type: the largest number any type holds — "15 of 50 from the `16xlarge` up", the
/// top row of the published "IP addresses per network interface per instance type" table
/// ([`crate::state::network_interface_limit`] is the per-type reading of it).
///
/// `InstanceNetworkInterfaceSpecificationList` carries no modelled `@length`, so the list a
/// caller may send is unbounded and this is the ceiling that bounds it.
const MAX_LAUNCH_INTERFACES: usize = 15;

/// What ONE entry of a launch's `NetworkInterfaces` asks for, read off the request alone.
///
/// Everything here is decided without the store, so a launch is refused for what it ASKED
/// before any record of the account is read: the device index and network card it attaches
/// at, whether it names an existing interface, the subnet and groups it names, its type and
/// description, the addresses it asks for ([`crate::state::requested_private_ips`], the one
/// reading of those members this crate has) and the options the attachment carries.
#[derive(Debug, Clone, Default)]
struct InterfaceRequest {
    device_index: i32,
    network_card_index: i32,
    network_interface_id: Option<String>,
    subnet_id: Option<String>,
    group_ids: Vec<String>,
    description: String,
    interface_type: String,
    delete_on_termination: bool,
    /// What the entry asked for, if it asked at all: an entry that names no
    /// `AssociatePublicIpAddress` takes the answer of the SUBNET it is placed in, which is
    /// not known until the account's records are read ([`take_launch_interfaces`]).
    associate_public_ip_address: Option<bool>,
    addresses: crate::state::RequestedPrivateIps,
    ena_srd_specification: Option<AttachmentEnaSrdSpecification>,
    ena_queue_count: Option<i32>,
}

/// One [`InterfaceRequest`] once the account's own records have been read: the subnet the
/// interface sits in and that subnet's VPC, zone and address range.
#[derive(Debug, Clone)]
struct PlannedInterface {
    request: InterfaceRequest,
    subnet_id: String,
    vpc_id: String,
    availability_zone: String,
    availability_zone_id: String,
    cidr_block: String,
}

/// Whether one `NetworkInterfaces` entry asks for an IPv6 address — on either of the two
/// members that ask for one (`Ipv6Addresses`, `Ipv6AddressCount`).
fn interface_asks_for_ipv6(interface: &InstanceNetworkInterfaceSpecification) -> bool {
    interface
        .ipv6_addresses
        .as_ref()
        .is_some_and(|addresses| !addresses.is_empty())
        || interface.ipv6_address_count.is_some_and(|count| count > 0)
}

/// Refuse a launch whose `NetworkInterfaces` name state this slice holds nothing of.
///
/// Each of these members CONTRADICTS what the interface would otherwise report — a delegated
/// prefix comes out of the subnet's prefixes, a primary IPv6 address out of its IPv6 range, a
/// carrier IP address out of a Wavelength zone's carrier network, and a connection tracking
/// configuration is a setting of its own — so a launch that asks for one is refused as
/// unwritten rather than answered with an interface that silently lacks it.
///
/// Asked of every launch that names its interfaces, before anything is made of the entries:
/// the answer is a statement about the REQUEST — this emulator cannot perform the work it
/// asks for — rather than one about the launch the handler would have assembled from it.
fn refuse_unwritten_interface_members(
    interfaces: &[InstanceNetworkInterfaceSpecification],
) -> Result<(), RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    for interface in interfaces {
        if interface
            .ipv4_prefixes
            .as_ref()
            .is_some_and(|prefixes| !prefixes.is_empty())
            || interface.ipv4_prefix_count.is_some_and(|count| count > 0)
            || interface
                .ipv6_prefixes
                .as_ref()
                .is_some_and(|prefixes| !prefixes.is_empty())
            || interface.ipv6_prefix_count.is_some_and(|count| count > 0)
        {
            return Err(Scaffolded::not_implemented(
                "RunInstances with a delegated prefix on a network interface",
            ));
        }
        if interface.primary_ipv6 == Some(true) {
            return Err(Scaffolded::not_implemented(
                "RunInstances with a primary IPv6 address",
            ));
        }
        if interface.associate_carrier_ip_address == Some(true) {
            return Err(Scaffolded::not_implemented(
                "RunInstances with a carrier IP address",
            ));
        }
        if interface.connection_tracking_specification.is_some() {
            return Err(Scaffolded::not_implemented(
                "RunInstances with a connection tracking specification on a network interface",
            ));
        }
    }
    Ok(())
}

/// Read a launch's `NetworkInterfaces` into what each entry asks for, refusing what the
/// entries themselves rule out. Empty in, empty out: a launch that names no interface gets
/// the single primary one the store creates for it.
///
/// `max_count` is the launch's own, because two members of an entry can serve exactly ONE
/// instance: a private address it names, and an existing interface it attaches. `instance_type`
/// is the launch's, because how many interfaces an instance may hold is a fact of its type.
fn read_interface_specifications(
    interfaces: &[InstanceNetworkInterfaceSpecification],
    max_count: i32,
    instance_type: &str,
) -> Result<Vec<InterfaceRequest>, RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    if interfaces.is_empty() {
        return Ok(Vec::new());
    }
    // The members of an entry this slice holds no state for, read before anything else is
    // made of the entries ([`refuse_unwritten_interface_members`]).
    refuse_unwritten_interface_members(interfaces)?;
    // The list a caller may send carries no modelled length, so it is bounded HERE, before any
    // work proportional to its length — reading each entry, and the scan for a device index
    // another entry already took — is done. The ceiling is the number of interfaces the
    // instance type can hold, read off the one derivation of a type's facts.
    let held_at_most = crate::state::network_interface_limit(instance_type)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(MAX_LAUNCH_INTERFACES);
    if interfaces.len() > held_at_most {
        return Err(crate::state::attachment_limit_exceeded(format!(
            "Interface count {} exceeds the limit for {instance_type}",
            interfaces.len()
        )));
    }
    let mut requests: Vec<InterfaceRequest> = Vec::with_capacity(interfaces.len());
    for interface in interfaces {
        // "The position of the network interface in the attachment order. A primary network
        // interface has a device index of 0" (`DeviceIndex`), which is the position an entry
        // that names none takes. A position cannot be below zero, and two interfaces cannot
        // share one: an index is how a client finds one interface of an instance among the
        // rest.
        let device_index = interface
            .device_index
            .unwrap_or(crate::state::PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX);
        if device_index < 0 {
            return Err(crate::state::invalid_parameter_value(format!(
                "Value ({device_index}) for parameter deviceIndex is invalid. A device index \
                 is zero or above"
            )));
        }
        if requests
            .iter()
            .any(|earlier| earlier.device_index == device_index)
        {
            return Err(crate::state::invalid_parameter_value(format!(
                "Value ({device_index}) for parameter deviceIndex is invalid. Each network \
                 interface must be attached at a device index of its own"
            )));
        }
        // "The default is network card index 0" (`NetworkCardIndex`).
        let network_card_index = interface.network_card_index.unwrap_or(0);
        if network_card_index < 0 {
            return Err(crate::state::invalid_parameter_value(format!(
                "Value ({network_card_index}) for parameter networkCardIndex is invalid. A \
                 network card index is zero or above"
            )));
        }
        // "The type of network interface" (`InterfaceType`), which defaults to the plain
        // interface every other path of this crate creates.
        let interface_type = interface
            .interface_type
            .clone()
            .filter(|interface_type| !interface_type.is_empty())
            .unwrap_or_else(|| crate::state::NETWORK_INTERFACE_TYPE_INTERFACE.to_string());
        if !LAUNCH_INTERFACE_TYPES.contains(&interface_type.as_str()) {
            return Err(crate::state::invalid_parameter_value(format!(
                "Value ({interface_type}) for parameter interfaceType is invalid. Valid \
                 values are {}",
                LAUNCH_INTERFACE_TYPES.join(" | ")
            )));
        }
        // "The IDs of the security groups for the network interface" (`Groups`), held to the
        // number AWS documents per interface — the same ceiling a create of one is held to.
        let group_ids = crate::state::deduplicate_ids(interface.groups.as_deref().unwrap_or_default());
        if group_ids.len() > crate::state::MAX_SECURITY_GROUPS_PER_INSTANCE {
            return Err(crate::state::invalid_parameter_value(format!(
                "The request names {} security groups, above the maximum of {max} per \
                 network interface",
                group_ids.len(),
                max = crate::state::MAX_SECURITY_GROUPS_PER_INSTANCE,
            )));
        }
        let existing = interface
            .network_interface_id
            .clone()
            .filter(|network_interface_id| !network_interface_id.is_empty());
        let addresses = crate::state::requested_private_ips(
            interface.private_ip_address.as_deref(),
            interface.private_ip_addresses.as_deref().unwrap_or_default(),
            interface.secondary_private_ip_address_count.unwrap_or(0),
            interface_type == crate::state::NETWORK_INTERFACE_TYPE_EFA_ONLY,
        )?;
        // An address is a value from the subnet's IPv4 range, so a value that is not an IPv4
        // address at all names nothing to assign — the same reading the launch's own
        // `PrivateIpAddress` gets.
        for address in &addresses.named {
            if address.parse::<::std::net::Ipv4Addr>().is_err() {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{address}' for privateIpAddress. \
                     It must be an IPv4 address"
                )));
            }
        }
        // One address and one interface can be held by ONE instance, so a launch of several
        // that names either is refused the way a launch naming its own private address is.
        if max_count > 1 && !addresses.named.is_empty() {
            return Err(Scaffolded::invalid_parameter_combination(
                "PrivateIpAddress cannot be specified when launching more than one instance",
            ));
        }
        if max_count > 1 && existing.is_some() {
            return Err(Scaffolded::invalid_parameter_combination(
                "A network interface ID cannot be specified when launching more than one \
                 instance",
            ));
        }
        // "If set to true, the interface is deleted when the instance is terminated. You can
        // specify true only if creating a new network interface when launching an instance"
        // (`DeleteOnTermination`) — and an interface a launch creates goes with its instance
        // unless the launch says otherwise, where one the launch merely attaches outlives it
        // ("if you attach a network interface to an existing instance, the
        // DeleteOnTermination attribute is set to false by default").
        let delete_on_termination = if existing.is_some() {
            if interface.delete_on_termination == Some(true) {
                return Err(Scaffolded::invalid_parameter_combination(
                    "DeleteOnTermination can be specified as true only for a network \
                     interface the launch creates",
                ));
            }
            false
        } else {
            interface.delete_on_termination.unwrap_or(true)
        };
        // "The public IP address can only be assigned to a network interface for eth0, and
        // can only be assigned to a new network interface, not an existing one. You cannot
        // specify more than one network interface in the request"
        // (`AssociatePublicIpAddress`). Each refusal is for what the entry ASKED for, so an
        // entry that asked for nothing is refused for nothing here and takes its subnet's
        // own answer below.
        let associate_public_ip_address = interface.associate_public_ip_address;
        if associate_public_ip_address == Some(true) {
            if existing.is_some() {
                return Err(Scaffolded::invalid_parameter_combination(
                    "A public IPv4 address can be assigned only to a network interface the \
                     launch creates",
                ));
            }
            if device_index != crate::state::PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX {
                return Err(Scaffolded::invalid_parameter_combination(
                    "A public IPv4 address can be assigned only to the network interface for \
                     eth0",
                ));
            }
            if interfaces.len() > 1 {
                return Err(Scaffolded::invalid_parameter_combination(
                    "A public IPv4 address cannot be assigned when the request specifies \
                     more than one network interface",
                ));
            }
        }
        requests.push(InterfaceRequest {
            device_index,
            network_card_index,
            network_interface_id: existing,
            subnet_id: interface.subnet_id.clone(),
            group_ids,
            // AWS reports an empty description for an interface nothing described.
            description: interface.description.clone().unwrap_or_default(),
            interface_type,
            delete_on_termination,
            associate_public_ip_address,
            addresses,
            // "Specifies the ENA Express settings for the network interface that's attached
            // to the instance" (`EnaSrdSpecification`): the request's shape and the one the
            // attachment reports carry the same settings, so what the launch asked for
            // round-trips through `DescribeNetworkInterfaces` rather than being dropped.
            ena_srd_specification: interface.ena_srd_specification.as_ref().map(|asked| {
                AttachmentEnaSrdSpecification {
                    ena_srd_enabled: asked.ena_srd_enabled,
                    ena_srd_udp_specification: asked.ena_srd_udp_specification.as_ref().map(
                        |udp| AttachmentEnaSrdUdpSpecification {
                            ena_srd_udp_enabled: udp.ena_srd_udp_enabled,
                        },
                    ),
                }
            }),
            ena_queue_count: interface.ena_queue_count,
        });
    }
    // "All instances have a network interface with a primary private IPv4 address"
    // (`RunInstances`), attached at device index 0 — so a launch that names its interfaces
    // names that one: without it there is no interface to take the instance's own subnet,
    // groups and primary address from.
    if !requests
        .iter()
        .any(|request| request.device_index == crate::state::PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX)
    {
        return Err(Scaffolded::invalid_parameter_combination(
            "A launch that specifies network interfaces must specify the primary network \
             interface at device index 0",
        ));
    }
    // In attachment order, which is what a device index is: the instance's interfaces are
    // then created in that order whatever order the request listed them in.
    requests.sort_by_key(|request| request.device_index);
    Ok(requests)
}

/// The sentence EC2 refuses a launch that names its security groups on BOTH its own members
/// and its network interfaces with, whichever of the two instance-level members named them:
/// `SecurityGroupIds` and `SecurityGroups` are two spellings of the one fact the interface
/// also carries, so one refusal states it.
const SECURITY_GROUPS_AND_INTERFACES: &str =
    "Network interfaces and an instance-level security groups may not be specified on the \
     same request";

/// Refuse a request that names the same fact on its own members and on its interfaces.
///
/// "If you specify a network interface, you must specify any subnets / security groups as
/// part of the network interface instead of using this parameter"
/// (`RunInstancesRequest$SubnetId`, `$SecurityGroupIds`, `$SecurityGroups`), and "you can't
/// specify this option if you've specified the option to designate a private IP address as
/// the primary IP address in a network interface specification" (`$PrivateIpAddress`).
fn refuse_interface_and_instance_level_members(
    input: &RunInstancesInput,
) -> Result<(), RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    if input.subnet_id.is_some() {
        return Err(Scaffolded::invalid_parameter_combination(
            "Network interfaces and an instance-level subnet ID may not be specified on the \
             same request",
        ));
    }
    if input
        .security_group_ids
        .as_ref()
        .is_some_and(|groups| !groups.is_empty())
    {
        return Err(Scaffolded::invalid_parameter_combination(
            SECURITY_GROUPS_AND_INTERFACES,
        ));
    }
    if input
        .security_groups
        .as_ref()
        .is_some_and(|groups| !groups.is_empty())
    {
        return Err(Scaffolded::invalid_parameter_combination(
            SECURITY_GROUPS_AND_INTERFACES,
        ));
    }
    if input.private_ip_address.is_some() {
        return Err(Scaffolded::invalid_parameter_combination(
            "Network interfaces and an instance-level private IP address may not be \
             specified on the same request",
        ));
    }
    Ok(())
}

/// Resolve each interface the launch named against the account: the subnet it sits in, and
/// for an entry that names an interface by id, that interface.
///
/// `launch_subnet_id` is the launch's own `SubnetId` and `availability_zone` the zone it is
/// placed in, which together are where an entry that names NO subnet is placed — "if you
/// don't specify a subnet ID, we choose a default subnet from your default VPC for you"
/// (`RunInstances`), read through the one resolution of that
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::launch_subnet`]).
fn plan_launch_interfaces(
    store: &crate::state::ComAmazonawsEc2Amazonec2State,
    requests: &[InterfaceRequest],
    launch_subnet_id: Option<&str>,
    availability_zone: &str,
) -> Result<Vec<PlannedInterface>, RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    let mut planned: Vec<PlannedInterface> = Vec::with_capacity(requests.len());
    for request in requests {
        let subnet_id = match request.network_interface_id.as_deref() {
            // An interface the account already holds brings its own subnet: it is attached
            // where it is rather than moved. One something else already holds cannot be
            // attached to this instance — "the specified interface is currently in use and
            // cannot be deleted or attached to another instance".
            Some(existing) => {
                let Some(interface) = store.network_interfaces.get(existing) else {
                    return Err(crate::state::invalid_network_interface_id_not_found(
                        existing,
                    ));
                };
                if store.network_interface_attachment(existing).is_some() {
                    return Err(crate::state::network_interface_in_use(existing));
                }
                interface.subnet_id.clone().unwrap_or_default()
            }
            None => match request
                .subnet_id
                .as_deref()
                .filter(|subnet_id| !subnet_id.is_empty())
            {
                Some(named) => {
                    if !store.subnets.contains_key(named) {
                        return Err(crate::state::invalid_subnet_id_not_found(named));
                    }
                    named.to_string()
                }
                None => {
                    let Some(subnet) = store.launch_subnet(launch_subnet_id, availability_zone)
                    else {
                        return Err(Scaffolded::missing_parameter(
                            "The request must contain the parameter subnetId",
                        ));
                    };
                    subnet.subnet_id.clone().unwrap_or_default()
                }
            },
        };
        let Some(subnet) = store.subnets.get(&subnet_id) else {
            return Err(crate::state::invalid_subnet_id_not_found(&subnet_id));
        };
        planned.push(PlannedInterface {
            request: request.clone(),
            subnet_id,
            vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
            availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
            availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
            cidr_block: subnet.cidr_block.clone().unwrap_or_default(),
        });
    }
    Ok(planned)
}

/// Refuse a launch one of whose interfaces sits in another Availability Zone than the
/// instance.
///
/// "You can attach a network interface to an instance in the same Availability Zone" (the EC2
/// User Guide's "Elastic network interfaces"): an interface lives in one subnet's zone and an
/// instance runs in one zone, so the two must be the same zone — the refusal names the
/// parameter the zone came from and both zones, the way the attach of one does.
fn refuse_interfaces_outside_zone(
    planned: &[PlannedInterface],
    availability_zone: &str,
) -> Result<(), RunInstancesError> {
    for interface in planned {
        if interface.availability_zone == availability_zone {
            continue;
        }
        return Err(match interface.request.network_interface_id.as_deref() {
            Some(existing) => crate::state::invalid_parameter_value(format!(
                "Value ({existing}) for parameter networkInterfaceId is invalid. The network \
                 interface is in the Availability Zone {} and the instance is in \
                 {availability_zone}",
                interface.availability_zone
            )),
            None => crate::state::invalid_parameter_value(format!(
                "Value ({}) for parameter subnetId is invalid. The subnet is in the \
                 Availability Zone {} and the instance is in {availability_zone}",
                interface.subnet_id, interface.availability_zone
            )),
        });
    }
    Ok(())
}

/// Refuse a launch that asks for more interfaces than its instance type can hold.
///
/// Read off the one derivation of an instance type's facts
/// ([`crate::state::network_interface_limit`]), so the limit a launch is refused by is the one
/// `DescribeInstanceTypes` publishes — and a device index is a POSITION, so an interface at
/// index N gives the instance N+1 interfaces: the count and the highest index are two
/// readings of the same number and the larger decides, exactly as an attach reads them.
fn refuse_too_many_interfaces(
    planned: &[PlannedInterface],
    instance_type: &str,
) -> Result<(), RunInstancesError> {
    let Some(limit) = crate::state::network_interface_limit(instance_type) else {
        return Ok(());
    };
    let highest_index = planned
        .iter()
        .map(|interface| interface.request.device_index)
        .max()
        .unwrap_or(-1);
    let count = i32::try_from(planned.len())
        .unwrap_or(i32::MAX)
        .max(highest_index.saturating_add(1));
    if count > limit {
        return Err(crate::state::attachment_limit_exceeded(format!(
            "Interface count {count} exceeds the limit for {instance_type}"
        )));
    }
    Ok(())
}

/// Take the addresses ONE instance's interfaces need and answer the interfaces to store.
///
/// Called once per instance the launch mints, so each instance gets interfaces and addresses
/// of its own. Every address is taken here, BEFORE anything is stored
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::reserve_private_ip`] for an address the
/// launch named, [`crate::state::ComAmazonawsEc2Amazonec2State::mint_private_ip_in_cidr`] for
/// one the service chooses — the one pool every subnet address of this crate comes out of), so
/// a launch that cannot be given them leaves no half-built instance behind: this call hands
/// back everything it took before refusing, and the caller hands back what the instances
/// before it took.
///
/// An existing interface keeps the addresses and groups it already holds: it is attached, not
/// created.
fn take_launch_interfaces(
    store: &mut crate::state::ComAmazonawsEc2Amazonec2State,
    planned: &[PlannedInterface],
) -> Result<Vec<LaunchedInterface>, RunInstancesError> {
    use crate::scaffolded::Scaffolded;

    let mut launched: Vec<LaunchedInterface> = Vec::with_capacity(planned.len());
    let mut taken: Vec<String> = Vec::new();
    for interface in planned {
        let mut addresses: Vec<String> = Vec::new();
        let groups: Vec<GroupIdentifier>;
        match interface.request.network_interface_id.as_deref() {
            Some(existing) => {
                let Some(held) = store.network_interfaces.get(existing) else {
                    store.release_private_ips(&taken);
                    return Err(crate::state::invalid_network_interface_id_not_found(
                        existing,
                    ));
                };
                addresses = held
                    .private_ip_addresses
                    .iter()
                    .flatten()
                    .filter_map(|address| address.private_ip_address.clone())
                    .collect();
                groups = held.groups.clone().unwrap_or_default();
            }
            None => {
                // Each group the launch named for this interface, completed to the id AND the
                // name a report carries; an interface the launch named none for gets its VPC's
                // own default group, which is what "Default: Amazon EC2 uses the default
                // security group" gives it.
                let mut named_groups: Vec<GroupIdentifier> = interface
                    .request
                    .group_ids
                    .iter()
                    .map(|group_id| {
                        store.group_identifier_of(
                            Some(interface.vpc_id.as_str()),
                            Some(group_id.as_str()),
                            None,
                        )
                    })
                    .collect();
                if named_groups.is_empty() {
                    named_groups.extend(store.default_security_group_of_vpc(&interface.vpc_id));
                }
                groups = named_groups;
                for address in &interface.request.addresses.named {
                    // An address the launch named has to be one of the subnet's assignable
                    // addresses: outside its CIDR block, or one of the five AWS reserves in
                    // it, there is nothing to assign.
                    if !crate::state::is_assignable_subnet_address(
                        &interface.cidr_block,
                        address,
                    ) {
                        store.release_private_ips(&taken);
                        return Err(crate::state::invalid_parameter_value(format!(
                            "Value ({address}) for parameter privateIpAddress is invalid. \
                             Address does not fall within the subnet's assignable address \
                             range {}",
                            interface.cidr_block
                        )));
                    }
                    if !store.reserve_private_ip(address) {
                        store.release_private_ips(&taken);
                        return Err(Scaffolded::invalid_ip_address_in_use(address));
                    }
                    taken.push(address.clone());
                    addresses.push(address.clone());
                }
                // The number of addresses this interface asks for is bounded BEFORE any of
                // them is minted: `wanted` is what
                // [`crate::state::requested_private_ips`] answered for this entry, and that
                // is the one place the per-interface ceiling
                // [`crate::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`] is applied to a
                // request's `SecondaryPrivateIpAddressCount`. The ceiling is re-asked here —
                // the belt-and-braces check this crate keeps where a bound decides how much
                // work a request may cause — so the loop below cannot be driven past it by a
                // count a caller chose, however wide the subnet's own range is.
                if interface.request.addresses.wanted
                    > crate::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE
                {
                    store.release_private_ips(&taken);
                    return Err(crate::state::invalid_parameter_value(format!(
                        "The request asks for {wanted} private IPv4 addresses, above the \
                         maximum of {max} per network interface",
                        wanted = interface.request.addresses.wanted,
                        max = crate::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE,
                    )));
                }
                while addresses.len() < interface.request.addresses.wanted {
                    let Some(free) = store.mint_private_ip_in_cidr(&interface.cidr_block) else {
                        store.release_private_ips(&taken);
                        return Err(Scaffolded::insufficient_free_addresses());
                    };
                    taken.push(free.clone());
                    addresses.push(free);
                }
            }
        }
        // "Indicates whether instances launched in this subnet receive a public IPv4
        // address" (`Subnet$MapPublicIpOnLaunch`) — the attribute "if launching into a
        // default subnet, the default value is true" (`AssociatePublicIpAddress`) reads
        // off: an entry that asked either way is honoured, and one that asked nothing takes
        // the answer of the subnet it is placed in. The restrictions the same member states
        // bind the implicit answer as well as the explicit one, because they are facts of
        // where such an address can go rather than of who asked for it: it "can only be
        // assigned to a network interface for eth0", "can only be assigned to a new network
        // interface, not an existing one", and "you cannot specify more than one network
        // interface in the request" — so a launch naming several interfaces receives none.
        let associate_public_ip_address = interface
            .request
            .associate_public_ip_address
            .unwrap_or_else(|| {
                planned.len() == 1
                    && interface.request.network_interface_id.is_none()
                    && interface.request.device_index
                        == crate::state::PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX
                    && store
                        .subnets
                        .get(&interface.subnet_id)
                        .is_some_and(|subnet| subnet.map_public_ip_on_launch == Some(true))
            });
        launched.push(LaunchedInterface {
            device_index: interface.request.device_index,
            network_card_index: interface.request.network_card_index,
            network_interface_id: interface.request.network_interface_id.clone(),
            subnet_id: interface.subnet_id.clone(),
            vpc_id: interface.vpc_id.clone(),
            availability_zone: interface.availability_zone.clone(),
            availability_zone_id: interface.availability_zone_id.clone(),
            // The Region the interface's private DNS names are built for is the one its zone
            // belongs to, read through the ONE reading of that, as the primary interface of a
            // launch that names none reads it.
            region: crate::state::region_of_zone_name(&interface.availability_zone).to_string(),
            description: interface.request.description.clone(),
            interface_type: interface.request.interface_type.clone(),
            groups,
            private_ip_addresses: addresses,
            delete_on_termination: interface.request.delete_on_termination,
            associate_public_ip_address,
            ena_srd_specification: interface.request.ena_srd_specification.clone(),
            ena_queue_count: interface.request.ena_queue_count,
        });
    }
    Ok(launched)
}

/// Hand back every private IPv4 address the interfaces of `launched` had taken.
///
/// The reverse of [`take_launch_interfaces`]'s taking, for a launch refused after some of its
/// instances' interfaces were resolved: the addresses go back to the one pool they came out
/// of, so a refused launch leaves the subnets' addresses exactly as it found them. An
/// existing interface the launch would have attached is skipped — it holds its addresses
/// whether or not this launch happened.
fn release_taken_interfaces(
    store: &mut crate::state::ComAmazonawsEc2Amazonec2State,
    launched: &[Vec<LaunchedInterface>],
) {
    for interfaces in launched {
        for interface in interfaces {
            if interface.network_interface_id.is_some() {
                continue;
            }
            store.release_private_ips(&interface.private_ip_addresses);
        }
    }
}
