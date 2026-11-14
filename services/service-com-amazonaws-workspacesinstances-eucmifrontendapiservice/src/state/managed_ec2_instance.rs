// SPDX-License-Identifier: CC0-1.0

//! The managed Amazon EC2 instance a WorkSpace Instance IS.
//!
//! A WorkSpace Instance is not a resource of this service alone: "Launches a new WorkSpace
//! Instance with specified configuration parameters"
//! (`smithy_query(query="operation",operation="CreateWorkspaceInstance")`) names an EC2
//! launch, `ManagedInstanceRequest` carries the parameters of one
//! (`smithy_query(query="shape",shape="ManagedInstanceRequest")` — `ImageId`,
//! `InstanceType`, `SubnetId`, `SecurityGroupIds`, `KeyName`, `IamInstanceProfile`,
//! `BlockDeviceMappings`, `Placement`, `UserData`, `TagSpecifications` and the rest, each
//! naming a resource or a setting of the CALLER's own account), the reported instance is
//! "the managed EC2 instance" (`EC2ManagedInstance$InstanceId`), and the failures the
//! caller is shown are EC2's own (`EC2InstanceError{EC2ErrorCode, EC2ExceptionType,
//! EC2ErrorMessage}`). So this service does not mint an instance of its own: it asks the
//! service that OWNS instances to launch one, reports the id EC2 minted, and terminates
//! that instance when the WorkSpace Instance is deleted.
//!
//! The ask goes over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), the same seam every other
//! service of this emulator reaches another one through: this crate names no other
//! service's crate, no socket is opened and nothing leaves the process. The credentials are
//! the callback's, which sign the hop and never appear in a response.
//!
//! Nothing here holds this service's store: every function takes the values it needs and is
//! awaited with no guard held, so a launch cannot be ordered behind a lock.

use std::collections::BTreeMap;

use crate::ops::types::{
    BlockDeviceMappingRequest, EC2InstanceError, IamInstanceProfileSpecification,
    InstanceNetworkInterfaceSpecification, ManagedInstanceRequest, Placement, TagSpecification,
    ValidationException,
};

/// The resource type a `TagSpecification` of the LAUNCH names for the instance itself.
///
/// The same token this service's own tag reader uses
/// ([`super::INSTANCE_TAG_SPECIFICATION_RESOURCE_TYPE`]), because it is the same
/// `ResourceTypeEnum` value on both sides of the hop.
const LAUNCH_INSTANCE_RESOURCE_TYPE: &str = super::INSTANCE_TAG_SPECIFICATION_RESOURCE_TYPE;

/// How many EC2 instances ONE WorkSpace Instance is: a WorkSpace Instance is "the managed
/// EC2 instance" (`EC2ManagedInstance`, one `InstanceId`), so the launch asks for exactly
/// one and the reservation it mints holds exactly one.
const INSTANCES_PER_WORKSPACE_INSTANCE: i32 = 1;

/// The class of failure the EC2 client reported, as [`EC2InstanceError::ec2_exception_type`]
/// names it: whether EC2 itself answered an error, or the call never reached it.
///
/// These are the kinds [`::aws_sdk_ec2`] distinguishes, not wording of this crate's own: a
/// caller reading `EC2ExceptionType` learns whether the launch was refused by EC2 (and can
/// act on `EC2ErrorCode`) or could not be made at all.
const EXCEPTION_TYPE_SERVICE_ERROR: &str = "ServiceError";
const EXCEPTION_TYPE_DISPATCH_FAILURE: &str = "DispatchFailure";
const EXCEPTION_TYPE_RESPONSE_ERROR: &str = "ResponseError";
const EXCEPTION_TYPE_TIMEOUT_ERROR: &str = "TimeoutError";
const EXCEPTION_TYPE_CONSTRUCTION_FAILURE: &str = "ConstructionFailure";

/// The AWS SDK client this service asks Amazon EC2 with.
///
/// `region` is the Region the WorkSpaces request was signed for, so the instance is launched
/// in the Region the caller addressed. It is one of the Regions this service publishes
/// ([`super::is_supported_region`], which every caller here checks first), never
/// caller-chosen text, which is what makes interpolating it into the endpoint authority
/// below safe; the endpoint is only the address the callback's transport routes on IN
/// PROCESS.
///
/// `None` for a Region this service does not serve: it has no endpoint of this service, so
/// it holds no WorkSpace Instances and there is no launch to make there.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    if !super::is_supported_region(region) {
        return None;
    }
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// The modelled `EC2InstanceError` for a call to EC2 that failed, built from what the EC2
/// client reported and nothing else.
///
/// `EC2ErrorCode` is EC2's own error code when EC2 answered one (`InvalidSubnetID.NotFound`,
/// `InvalidGroup.NotFound`, `Unsupported`, …) and absent otherwise; `EC2ErrorMessage` is
/// EC2's own sentence, falling back to the client's account of why the call could not be
/// made; `EC2ExceptionType` is the class of failure (see
/// [`EXCEPTION_TYPE_SERVICE_ERROR`]). Nothing here is invented: every member is either a
/// value EC2 sent or absent.
fn ec2_instance_error<E, R>(error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>) -> EC2InstanceError
where
    E: ::aws_sdk_ec2::error::ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug,
{
    use ::aws_sdk_ec2::error::ProvideErrorMetadata;
    use ::aws_smithy_runtime_api::client::result::SdkError;

    let exception_type = match error {
        SdkError::ConstructionFailure(_) => EXCEPTION_TYPE_CONSTRUCTION_FAILURE,
        SdkError::TimeoutError(_) => EXCEPTION_TYPE_TIMEOUT_ERROR,
        SdkError::DispatchFailure(_) => EXCEPTION_TYPE_DISPATCH_FAILURE,
        SdkError::ResponseError(_) => EXCEPTION_TYPE_RESPONSE_ERROR,
        SdkError::ServiceError(_) => EXCEPTION_TYPE_SERVICE_ERROR,
        // `SdkError` is `#[non_exhaustive]`: a kind this client has not been told about is
        // still a failure of the call, reported as one rather than as a success.
        _ => EXCEPTION_TYPE_DISPATCH_FAILURE,
    };
    EC2InstanceError {
        ec2_error_code: error.code().map(str::to_string),
        ec2_exception_type: Some(exception_type.to_string()),
        ec2_error_message: Some(error.message().map_or_else(
            || {
                ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
            },
            str::to_string,
        )),
    }
}

/// The `EC2InstanceError` for a Region this service serves no endpoint of, or an EC2 that
/// answered a launch with no instance at all: the launch was not made, so the WorkSpace
/// Instance has no managed instance behind it and says so.
fn launch_not_made(message: &str) -> EC2InstanceError {
    EC2InstanceError {
        ec2_error_code: None,
        ec2_exception_type: Some(EXCEPTION_TYPE_DISPATCH_FAILURE.to_string()),
        ec2_error_message: Some(message.to_string()),
    }
}

/// The codes Amazon EC2 answers when a request names an AMI it cannot resolve: an id the
/// account can see no image for, and a value that is no AMI id at all (Amazon EC2 error
/// codes, `InvalidAMIID.NotFound` and `InvalidAMIID.Malformed`). Either way the request
/// named no image the account could launch from.
const INVALID_AMI_ID_NOT_FOUND: &str = "InvalidAMIID.NotFound";
const INVALID_AMI_ID_MALFORMED: &str = "InvalidAMIID.Malformed";

/// The `Message` this service states for a `ManagedInstance.ImageId` that names no AMI the
/// account can see in the Region the call was signed for.
///
/// A FIXED sentence, like [`super::INSTANCE_NOT_FOUND_MESSAGE`] and for the same reason: it
/// states what is wrong with the request and echoes none of what the caller sent, the id at
/// fault included.
pub const INVALID_IMAGE_ID_MESSAGE: &str = "Invalid AMI ID provided";

/// The modelled `ValidationException` (HTTP 400) for a launch whose image cannot be
/// resolved.
///
/// Built through the crate's one bad-member refusal ([`super::field_validation_failed`]), so
/// `Reason` and `FieldList` name the member at fault the way every other refusal of this
/// service does.
pub fn invalid_image_id() -> ValidationException {
    super::field_validation_failed("ManagedInstance.ImageId", INVALID_IMAGE_ID_MESSAGE)
}

/// What asking Amazon EC2 about an AMI answered — the outcome [`resolve_image`] reports.
#[derive(Debug)]
pub enum ImageLookup {
    /// EC2 resolved the id: the account can see that image in the Region, so a launch
    /// there can boot from it.
    Resolved,
    /// EC2 answered that the id names no image this account can see in that Region.
    Unresolved,
    /// The question could not be answered: EC2 could not be reached, or answered a failure
    /// that is not about the id. Carries what EC2 reported, so a caller can tell this from
    /// an answer either way rather than reading it as one.
    Unanswered(EC2InstanceError),
}

/// Resolve the AMI `image_id` against the images the account can see in `region`.
///
/// The image a launch boots belongs to the CALLER's own account
/// (`ManagedInstanceRequest$ImageId` mirrors `RunInstances$ImageId`), and Amazon EC2 is the
/// service that owns AMIs, so the question is put to EC2 over the same in-process callback
/// the launch itself is made through rather than answered from anything this crate keeps: an
/// image registered, copied or deregistered in EC2 is one this answer follows.
///
/// A read, and only a read — `ec2:DescribeImages` reports images and writes nothing — and
/// awaited with no store guard held, like every other function here.
pub async fn resolve_image(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    image_id: &str,
) -> ImageLookup {
    use ::aws_sdk_ec2::error::ProvideErrorMetadata;

    let Some(client) = ec2(ctx, region) else {
        return ImageLookup::Unanswered(launch_not_made(
            "Amazon EC2 could not be reached to resolve the image the instance boots",
        ));
    };
    match client.describe_images().image_ids(image_id).send().await {
        Ok(described) => {
            if described
                .images()
                .iter()
                .any(|image| image.image_id() == Some(image_id))
            {
                ImageLookup::Resolved
            } else {
                ImageLookup::Unresolved
            }
        }
        // EC2's own answer for an id it cannot resolve — whether the id names no image the
        // account can see or is no AMI id at all — is the answer to the question asked, not
        // a failure of the call.
        Err(error) => match error.code() {
            Some(INVALID_AMI_ID_NOT_FOUND | INVALID_AMI_ID_MALFORMED) => ImageLookup::Unresolved,
            _ => ImageLookup::Unanswered(ec2_instance_error(&error)),
        },
    }
}

/// Launch the EC2 instance this WorkSpace Instance IS, and answer the `InstanceId` EC2
/// minted for it.
///
/// EVERY member of `managed_instance` is carried into the launch: the member names a
/// `RunInstances` parameter of the same name, so a WorkSpace Instance's image, instance
/// type, subnet, security groups, key pair, instance profile, block devices, placement,
/// user data, network interfaces, metadata/enclave/hibernation/maintenance options, market
/// and capacity-reservation choices and tag specifications are the launched instance's own
/// — nothing is kept on this service's record and applied to no resource.
///
/// `tags` is the WorkSpace Instance's own tag set, as this service stored it (the top-level
/// `Tags` merged over the instance-typed `TagSpecifications`), applied to the instance as a
/// tag specification of its own, LAST — so where a caller's specification and its `Tags`
/// name the same key, the instance carries the value this service reports for the WorkSpace
/// Instance. The caller's other specifications reach the launch unchanged, because they tag
/// the other resources the launch creates.
///
/// `workspace_instance_id` is the launch's `ClientToken`: one WorkSpace Instance is one
/// reservation, so a launch repeated for the same WorkSpace Instance replays that
/// reservation rather than minting a second instance.
///
/// `Err` carries what EC2 answered, as the modelled `EC2InstanceError` the caller reads
/// through `GetWorkspaceInstance$EC2InstanceErrors` — the launch failed, so no instance id
/// is invented for it.
pub async fn launch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    workspace_instance_id: &str,
    managed_instance: &ManagedInstanceRequest,
    tags: &BTreeMap<String, String>,
) -> Result<String, EC2InstanceError> {
    let Some(client) = ec2(ctx, region) else {
        return Err(launch_not_made(
            "Amazon EC2 could not be reached to launch the managed instance",
        ));
    };
    let launched = client
        .run_instances()
        .set_client_token(Some(workspace_instance_id.to_string()))
        .set_min_count(Some(INSTANCES_PER_WORKSPACE_INSTANCE))
        .set_max_count(Some(INSTANCES_PER_WORKSPACE_INSTANCE))
        .set_image_id(managed_instance.image_id.clone())
        .set_instance_type(managed_instance.instance_type.as_deref().map(Into::into))
        .set_key_name(managed_instance.key_name.clone())
        .set_subnet_id(managed_instance.subnet_id.clone())
        .set_security_group_ids(managed_instance.security_group_ids.clone())
        .set_security_groups(managed_instance.security_groups.clone())
        .set_user_data(managed_instance.user_data.clone())
        .set_kernel_id(managed_instance.kernel_id.clone())
        .set_ramdisk_id(managed_instance.ramdisk_id.clone())
        .set_private_ip_address(managed_instance.private_ip_address.clone())
        .set_ebs_optimized(managed_instance.ebs_optimized)
        .set_disable_api_stop(managed_instance.disable_api_stop)
        .set_enable_primary_ipv6(managed_instance.enable_primary_ipv6)
        .set_ipv6_address_count(managed_instance.ipv6_address_count)
        .set_ipv6_addresses(ipv6_addresses(managed_instance))
        .set_block_device_mappings(block_device_mappings(managed_instance))
        .set_placement(placement(managed_instance.placement.as_ref()))
        .set_iam_instance_profile(iam_instance_profile(
            managed_instance.iam_instance_profile.as_ref(),
        ))
        .set_monitoring(monitoring(managed_instance))
        .set_cpu_options(cpu_options(managed_instance))
        .set_credit_specification(credit_specification(managed_instance))
        .set_capacity_reservation_specification(capacity_reservation(managed_instance))
        .set_enclave_options(enclave_options(managed_instance))
        .set_hibernation_options(hibernation_options(managed_instance))
        .set_maintenance_options(maintenance_options(managed_instance))
        .set_metadata_options(metadata_options(managed_instance))
        .set_network_performance_options(network_performance_options(managed_instance))
        .set_private_dns_name_options(private_dns_name_options(managed_instance))
        .set_instance_market_options(instance_market_options(managed_instance))
        .set_license_specifications(license_specifications(managed_instance))
        .set_network_interfaces(network_interfaces(managed_instance))
        .set_tag_specifications(Some(tag_specifications(
            managed_instance.tag_specifications.as_deref().unwrap_or_default(),
            tags,
        )))
        .send()
        .await
        .map_err(|error| ec2_instance_error(&error))?;
    launched
        .instances()
        .first()
        .and_then(|instance| instance.instance_id().map(str::to_string))
        .ok_or_else(|| launch_not_made("Amazon EC2 reported no instance for the launch"))
}

/// Terminate the managed EC2 instance `ec2_instance_id`, as the teardown
/// `DeleteWorkspaceInstance` starts does: the WorkSpace Instance IS that instance, so
/// deleting it shuts the instance down.
///
/// `Err` carries what EC2 answered, for the same reason [`launch`] does: the teardown of
/// the instance did not happen, so the WorkSpace Instance reports a failed deallocation
/// rather than a finished one.
pub async fn terminate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ec2_instance_id: &str,
) -> Result<(), EC2InstanceError> {
    let Some(client) = ec2(ctx, region) else {
        return Err(launch_not_made(
            "Amazon EC2 could not be reached to terminate the managed instance",
        ));
    };
    client
        .terminate_instances()
        .instance_ids(ec2_instance_id)
        .send()
        .await
        .map(|_| ())
        .map_err(|error| ec2_instance_error(&error))
}

/// `ManagedInstance.Ipv6Addresses`, as the launch's own member.
fn ipv6_addresses(
    managed_instance: &ManagedInstanceRequest,
) -> Option<Vec<::aws_sdk_ec2::types::InstanceIpv6Address>> {
    let addresses = managed_instance.ipv6_addresses.as_ref()?;
    Some(
        addresses
            .iter()
            .map(|address| {
                ::aws_sdk_ec2::types::InstanceIpv6Address::builder()
                    .set_ipv6_address(address.ipv6_address.clone())
                    .set_is_primary_ipv6(address.is_primary_ipv6)
                    .build()
            })
            .collect(),
    )
}

/// `ManagedInstance.BlockDeviceMappings` — "configures block device mappings for storage" —
/// as the launch's `BlockDeviceMappings`, each entry's EBS settings included.
fn block_device_mappings(
    managed_instance: &ManagedInstanceRequest,
) -> Option<Vec<::aws_sdk_ec2::types::BlockDeviceMapping>> {
    let mappings = managed_instance.block_device_mappings.as_ref()?;
    Some(mappings.iter().map(block_device_mapping).collect())
}

/// ONE entry of [`block_device_mappings`].
fn block_device_mapping(
    mapping: &BlockDeviceMappingRequest,
) -> ::aws_sdk_ec2::types::BlockDeviceMapping {
    let ebs = mapping.ebs.as_ref().map(|ebs| {
        ::aws_sdk_ec2::types::EbsBlockDevice::builder()
            .set_volume_type(ebs.volume_type.as_deref().map(Into::into))
            .set_volume_size(ebs.volume_size)
            .set_iops(ebs.iops)
            .set_throughput(ebs.throughput)
            .set_encrypted(ebs.encrypted)
            .set_kms_key_id(ebs.kms_key_id.clone())
            .build()
    });
    ::aws_sdk_ec2::types::BlockDeviceMapping::builder()
        .set_device_name(mapping.device_name.clone())
        .set_virtual_name(mapping.virtual_name.clone())
        .set_no_device(mapping.no_device.clone())
        .set_ebs(ebs)
        .build()
}

/// `ManagedInstance.Placement` — "specifies instance placement preferences" — as the
/// launch's `Placement`: where the instance is placed is a fact of the instance, so the
/// whole structure is carried rather than the zone alone.
fn placement(placement: Option<&Placement>) -> Option<::aws_sdk_ec2::types::Placement> {
    let placement = placement?;
    Some(
        ::aws_sdk_ec2::types::Placement::builder()
            .set_availability_zone(placement.availability_zone.clone())
            .set_affinity(placement.affinity.clone())
            .set_group_id(placement.group_id.clone())
            .set_group_name(placement.group_name.clone())
            .set_host_id(placement.host_id.clone())
            .set_host_resource_group_arn(placement.host_resource_group_arn.clone())
            .set_partition_number(placement.partition_number)
            .set_tenancy(placement.tenancy.as_deref().map(Into::into))
            .build(),
    )
}

/// `ManagedInstance.IamInstanceProfile`, as the launch's own member: the profile the
/// instance runs with.
fn iam_instance_profile(
    profile: Option<&IamInstanceProfileSpecification>,
) -> Option<::aws_sdk_ec2::types::IamInstanceProfileSpecification> {
    let profile = profile?;
    Some(
        ::aws_sdk_ec2::types::IamInstanceProfileSpecification::builder()
            .set_arn(profile.arn.clone())
            .set_name(profile.name.clone())
            .build(),
    )
}

/// `ManagedInstance.CpuOptions`, as the launch's own member.
fn cpu_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::CpuOptionsRequest> {
    let options = managed_instance.cpu_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::CpuOptionsRequest::builder()
            .set_core_count(options.core_count)
            .set_threads_per_core(options.threads_per_core)
            .set_amd_sev_snp(options.amd_sev_snp.as_deref().map(Into::into))
            .set_nested_virtualization(options.nested_virtualization.as_deref().map(Into::into))
            .build(),
    )
}

/// `ManagedInstance.Monitoring` — "enables or disables detailed instance monitoring" — as
/// the launch's own member.
///
/// `Enabled` is optional on this service's side and required on the launch's, and the
/// absence of it asks for no detailed monitoring, which is what the instance gets.
fn monitoring(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::RunInstancesMonitoringEnabled> {
    let monitoring = managed_instance.monitoring.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::RunInstancesMonitoringEnabled::builder()
            .enabled(monitoring.enabled.unwrap_or(false))
            .build(),
    )
}

/// `ManagedInstance.CreditSpecification` — "defines CPU credit configuration for burstable
/// instances" — as the launch's own member.
///
/// `CpuCredits` is optional on this service's side and required on the launch's, so a
/// specification that names none names no credit configuration to apply.
fn credit_specification(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::CreditSpecificationRequest> {
    let specification = managed_instance.credit_specification.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::CreditSpecificationRequest::builder()
            .set_cpu_credits(specification.cpu_credits.clone())
            .build(),
    )
}

/// `ManagedInstance.CapacityReservationSpecification`, as the launch's own member.
fn capacity_reservation(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::CapacityReservationSpecification> {
    let specification = managed_instance.capacity_reservation_specification.as_ref()?;
    let target = specification.capacity_reservation_target.as_ref().map(|target| {
        ::aws_sdk_ec2::types::CapacityReservationTarget::builder()
            .set_capacity_reservation_id(target.capacity_reservation_id.clone())
            .set_capacity_reservation_resource_group_arn(
                target.capacity_reservation_resource_group_arn.clone(),
            )
            .build()
    });
    Some(
        ::aws_sdk_ec2::types::CapacityReservationSpecification::builder()
            .set_capacity_reservation_preference(
                specification
                    .capacity_reservation_preference
                    .as_deref()
                    .map(Into::into),
            )
            .set_capacity_reservation_target(target)
            .build(),
    )
}

/// `ManagedInstance.EnclaveOptions`, as the launch's own member.
fn enclave_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::EnclaveOptionsRequest> {
    let options = managed_instance.enclave_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::EnclaveOptionsRequest::builder()
            .set_enabled(options.enabled)
            .build(),
    )
}

/// `ManagedInstance.HibernationOptions`, as the launch's own member.
fn hibernation_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::HibernationOptionsRequest> {
    let options = managed_instance.hibernation_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::HibernationOptionsRequest::builder()
            .set_configured(options.configured)
            .build(),
    )
}

/// `ManagedInstance.MaintenanceOptions`, as the launch's own member.
fn maintenance_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::InstanceMaintenanceOptionsRequest> {
    let options = managed_instance.maintenance_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::InstanceMaintenanceOptionsRequest::builder()
            .set_auto_recovery(options.auto_recovery.as_deref().map(Into::into))
            .build(),
    )
}

/// `ManagedInstance.MetadataOptions`, as the launch's own member.
fn metadata_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::InstanceMetadataOptionsRequest> {
    let options = managed_instance.metadata_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::InstanceMetadataOptionsRequest::builder()
            .set_http_endpoint(options.http_endpoint.as_deref().map(Into::into))
            .set_http_protocol_ipv6(options.http_protocol_ipv6.as_deref().map(Into::into))
            .set_http_put_response_hop_limit(options.http_put_response_hop_limit)
            .set_http_tokens(options.http_tokens.as_deref().map(Into::into))
            .set_instance_metadata_tags(options.instance_metadata_tags.as_deref().map(Into::into))
            .build(),
    )
}

/// `ManagedInstance.NetworkPerformanceOptions`, as the launch's own member.
fn network_performance_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::InstanceNetworkPerformanceOptionsRequest> {
    let options = managed_instance.network_performance_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::InstanceNetworkPerformanceOptionsRequest::builder()
            .set_bandwidth_weighting(options.bandwidth_weighting.as_deref().map(Into::into))
            .build(),
    )
}

/// `ManagedInstance.PrivateDnsNameOptions`, as the launch's own member.
fn private_dns_name_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::PrivateDnsNameOptionsRequest> {
    let options = managed_instance.private_dns_name_options.as_ref()?;
    Some(
        ::aws_sdk_ec2::types::PrivateDnsNameOptionsRequest::builder()
            .set_hostname_type(options.hostname_type.as_deref().map(Into::into))
            .set_enable_resource_name_dns_a_record(options.enable_resource_name_dns_a_record)
            .set_enable_resource_name_dns_aaaa_record(options.enable_resource_name_dns_aaaa_record)
            .build(),
    )
}

/// `ManagedInstance.InstanceMarketOptions`, as the launch's own member, with the Spot
/// options it carries.
fn instance_market_options(
    managed_instance: &ManagedInstanceRequest,
) -> Option<::aws_sdk_ec2::types::InstanceMarketOptionsRequest> {
    let options = managed_instance.instance_market_options.as_ref()?;
    let spot = options.spot_options.as_ref().map(|spot| {
        ::aws_sdk_ec2::types::SpotMarketOptions::builder()
            .set_max_price(spot.max_price.clone())
            .set_spot_instance_type(spot.spot_instance_type.as_deref().map(Into::into))
            .set_block_duration_minutes(spot.block_duration_minutes)
            // `SpotMarketOptions$ValidUntilUtc` on this service's side is
            // `SpotMarketOptions$ValidUntil` on the launch's: the same instant, named for
            // the zone it is always in.
            .set_valid_until(spot.valid_until_utc)
            .set_instance_interruption_behavior(
                spot.instance_interruption_behavior.as_deref().map(Into::into),
            )
            .build()
    });
    Some(
        ::aws_sdk_ec2::types::InstanceMarketOptionsRequest::builder()
            .set_market_type(options.market_type.as_deref().map(Into::into))
            .set_spot_options(spot)
            .build(),
    )
}

/// `ManagedInstance.LicenseSpecifications`, as the launch's own member.
fn license_specifications(
    managed_instance: &ManagedInstanceRequest,
) -> Option<Vec<::aws_sdk_ec2::types::LicenseConfigurationRequest>> {
    let specifications = managed_instance.license_specifications.as_ref()?;
    Some(
        specifications
            .iter()
            .map(|specification| {
                ::aws_sdk_ec2::types::LicenseConfigurationRequest::builder()
                    .set_license_configuration_arn(specification.license_configuration_arn.clone())
                    .build()
            })
            .collect(),
    )
}

/// `ManagedInstance.NetworkInterfaces` — "configures network interface settings" — as the
/// launch's own member: every entry, with the subnet, groups, addresses, prefixes and
/// options it names.
fn network_interfaces(
    managed_instance: &ManagedInstanceRequest,
) -> Option<Vec<::aws_sdk_ec2::types::InstanceNetworkInterfaceSpecification>> {
    let interfaces = managed_instance.network_interfaces.as_ref()?;
    Some(interfaces.iter().map(network_interface).collect())
}

/// ONE entry of [`network_interfaces`].
fn network_interface(
    interface: &InstanceNetworkInterfaceSpecification,
) -> ::aws_sdk_ec2::types::InstanceNetworkInterfaceSpecification {
    let private_ip_addresses = interface.private_ip_addresses.as_ref().map(|addresses| {
        addresses
            .iter()
            .map(|address| {
                ::aws_sdk_ec2::types::PrivateIpAddressSpecification::builder()
                    .set_primary(address.primary)
                    .set_private_ip_address(address.private_ip_address.clone())
                    .build()
            })
            .collect()
    });
    let ipv6_addresses = interface.ipv6_addresses.as_ref().map(|addresses| {
        addresses
            .iter()
            .map(|address| {
                ::aws_sdk_ec2::types::InstanceIpv6Address::builder()
                    .set_ipv6_address(address.ipv6_address.clone())
                    .set_is_primary_ipv6(address.is_primary_ipv6)
                    .build()
            })
            .collect()
    });
    let ipv4_prefixes = interface.ipv4_prefixes.as_ref().map(|prefixes| {
        prefixes
            .iter()
            .map(|prefix| {
                ::aws_sdk_ec2::types::Ipv4PrefixSpecificationRequest::builder()
                    .set_ipv4_prefix(prefix.ipv4_prefix.clone())
                    .build()
            })
            .collect()
    });
    let ipv6_prefixes = interface.ipv6_prefixes.as_ref().map(|prefixes| {
        prefixes
            .iter()
            .map(|prefix| {
                ::aws_sdk_ec2::types::Ipv6PrefixSpecificationRequest::builder()
                    .set_ipv6_prefix(prefix.ipv6_prefix.clone())
                    .build()
            })
            .collect()
    });
    let ena_srd = interface.ena_srd_specification.as_ref().map(|ena| {
        let udp = ena.ena_srd_udp_specification.as_ref().map(|udp| {
            ::aws_sdk_ec2::types::EnaSrdUdpSpecificationRequest::builder()
                .set_ena_srd_udp_enabled(udp.ena_srd_udp_enabled)
                .build()
        });
        ::aws_sdk_ec2::types::EnaSrdSpecificationRequest::builder()
            .set_ena_srd_enabled(ena.ena_srd_enabled)
            .set_ena_srd_udp_specification(udp)
            .build()
    });
    let connection_tracking = interface
        .connection_tracking_specification
        .as_ref()
        .map(|tracking| {
            ::aws_sdk_ec2::types::ConnectionTrackingSpecificationRequest::builder()
                .set_tcp_established_timeout(tracking.tcp_established_timeout)
                .set_udp_stream_timeout(tracking.udp_stream_timeout)
                .set_udp_timeout(tracking.udp_timeout)
                .build()
        });
    ::aws_sdk_ec2::types::InstanceNetworkInterfaceSpecification::builder()
        .set_device_index(interface.device_index)
        .set_network_card_index(interface.network_card_index)
        .set_network_interface_id(interface.network_interface_id.clone())
        .set_subnet_id(interface.subnet_id.clone())
        .set_groups(interface.groups.clone())
        .set_description(interface.description.clone())
        .set_interface_type(interface.interface_type.clone())
        .set_associate_public_ip_address(interface.associate_public_ip_address)
        .set_associate_carrier_ip_address(interface.associate_carrier_ip_address)
        .set_primary_ipv6(interface.primary_ipv6)
        .set_private_ip_address(interface.private_ip_address.clone())
        .set_private_ip_addresses(private_ip_addresses)
        .set_secondary_private_ip_address_count(interface.secondary_private_ip_address_count)
        .set_ipv6_address_count(interface.ipv6_address_count)
        .set_ipv6_addresses(ipv6_addresses)
        .set_ipv4_prefix_count(interface.ipv4_prefix_count)
        .set_ipv4_prefixes(ipv4_prefixes)
        .set_ipv6_prefix_count(interface.ipv6_prefix_count)
        .set_ipv6_prefixes(ipv6_prefixes)
        .set_ena_srd_specification(ena_srd)
        .set_connection_tracking_specification(connection_tracking)
        .build()
}

/// The launch's `TagSpecifications`: every specification the caller sent, unchanged, plus
/// the WorkSpace Instance's own tag set as an `instance` specification LAST — see
/// [`launch`].
fn tag_specifications(
    specifications: &[TagSpecification],
    tags: &BTreeMap<String, String>,
) -> Vec<::aws_sdk_ec2::types::TagSpecification> {
    let mut carried: Vec<::aws_sdk_ec2::types::TagSpecification> = specifications
        .iter()
        .map(|specification| {
            ::aws_sdk_ec2::types::TagSpecification::builder()
                .set_resource_type(specification.resource_type.as_deref().map(Into::into))
                .set_tags(Some(
                    specification
                        .tags
                        .iter()
                        .flatten()
                        .map(|tag| {
                            ::aws_sdk_ec2::types::Tag::builder()
                                .set_key(tag.key.clone())
                                .set_value(tag.value.clone())
                                .build()
                        })
                        .collect(),
                ))
                .build()
        })
        .collect();
    if !tags.is_empty() {
        carried.push(
            ::aws_sdk_ec2::types::TagSpecification::builder()
                .set_resource_type(Some(LAUNCH_INSTANCE_RESOURCE_TYPE.into()))
                .set_tags(Some(
                    tags.iter()
                        .map(|(key, value)| {
                            ::aws_sdk_ec2::types::Tag::builder()
                                .key(key)
                                .value(value)
                                .build()
                        })
                        .collect(),
                ))
                .build(),
        );
    }
    carried
}
