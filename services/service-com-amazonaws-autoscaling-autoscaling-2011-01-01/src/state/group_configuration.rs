// SPDX-License-Identifier: CC0-1.0

//! The rules a request that CONFIGURES an Auto Scaling group is taken against, beyond the
//! ones the model can state: which launch source the request may name, whether the places
//! and the resources it names exist, and the cross-member rules of the policies the group
//! carries.
//!
//! Every one of them is a rule about the REQUEST, decided before anything is stored, and
//! every one is shared by `CreateAutoScalingGroup` and `UpdateAutoScalingGroup`: the two
//! operations write the same group, so a configuration one of them refuses is not a
//! configuration the other may store. This module is the one place each rule is spelled, for
//! the reason [`super::iam_instance_profiles`] is one place: a rule stated twice is a rule
//! two handlers can disagree about.
//!
//! The references a group's launch source carries are resolved against the stores of the
//! services that OWN them — the Availability Zones and the launch templates are Amazon
//! EC2's — so a group this service stores can always launch what it promises, and
//! `DescribeAutoScalingGroups` never advertises a resource Amazon EC2 does not have.
//!
//! One of those resolutions serves a second caller, and so is the one rule here that is not
//! about a request: a launch template stays Amazon EC2's resource and the client's to delete
//! AFTER the group was configured against it, so the SAME reference is resolved again at the
//! instant of each launch ([`unresolved_launch_template_launch_failure`]) — one resolution
//! with one wording, instead of two that could disagree about whether a group can launch from
//! what it points at or about how a reference resolving to nothing is reported.
//!
//! LOCKING. The one pass that resolves a launch template takes Amazon EC2's store lock and
//! releases it before answering; the zone catalog needs no lock at all. The callers in
//! `src/ops/` ask these questions about a REQUEST before taking this service's own lock, so
//! no two stores are held at once there. The launch-time resolution is reached from the
//! launch itself, with this service's store already held, so it takes EC2's lock while
//! this service's is held — the SAME order the launch that follows it takes
//! (`super::ec2_instances`, whose header states it), and the only order in the tree:
//! Amazon EC2 never reaches this service, so no cycle exists.

use ::std::collections::BTreeSet;

use ::service_com_amazonaws_ec2_amazonec2::scaffolded::Scaffolded as Ec2Refusal;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::ops::types::{
    InstanceLifecyclePolicy, InstanceMaintenancePolicy, LaunchTemplateSpecification,
    MixedInstancesPolicy,
};
use crate::scaffolded::Scaffolded;

/// The sentence Amazon EC2 Auto Scaling opens every refusal of a launch source it could not
/// use with, followed by the detail that says what about it could not be used.
///
/// The service validates the launch source by asking Amazon EC2 to resolve it, so the detail
/// is Amazon EC2's own wording for the value that did not resolve and this prefix is Auto
/// Scaling's own — the same two-part sentence [`super::iam_instance_profiles`] answers for an
/// instance profile a template names and nothing holds.
const INVALID_LAUNCH_SOURCE: &str = "You must use a valid fully-formed launch template.";

/// What Amazon EC2 Auto Scaling answers a request that does not name exactly ONE launch
/// source.
///
/// Fixed text with no variable part: it names the four members, one of which a valid request
/// carries, and says nothing about which of them this request sent. The rule is about the
/// REQUEST's members alone, so it is decided before any of the values is looked up — a
/// request naming two of them is refused for naming two, whether or not either resolves.
pub const ONE_LAUNCH_SOURCE_MESSAGE: &str = "Valid requests must contain either LaunchTemplate, \
                                             LaunchConfigurationName, InstanceId or \
                                             MixedInstancesPolicy parameter.";

/// What Amazon EC2 Auto Scaling answers a request that states one healthy-percentage
/// threshold and not the other.
///
/// "Both `MinHealthyPercentage` and `MaxHealthyPercentage` must be specified"
/// (`smithy_query(query="shape",shape="InstanceMaintenancePolicy")`): fixed text naming the
/// two members and no value of the request.
pub const HEALTHY_PERCENTAGE_PAIR_MESSAGE: &str =
    "Both MinHealthyPercentage and MaxHealthyPercentage must be specified or not at all.";

/// The same sentence for a request that CHANGES a pair the group already carries, where a
/// threshold may also be CLEARED.
///
/// "To clear a previously set value, specify a value of `-1`" (the same members): a threshold
/// set to [`RESET_PERCENTAGE`] states none, so the rule holds over the thresholds that remain
/// and the service says so in the parenthesis. A request that states the pair for the first
/// time has nothing to clear, and is answered [`HEALTHY_PERCENTAGE_PAIR_MESSAGE`] instead.
pub const HEALTHY_PERCENTAGE_PAIR_RESET_MESSAGE: &str =
    "Both MinHealthyPercentage and MaxHealthyPercentage must be specified (excluding -1) or \
     not at all.";
/// The same rule again as the preferences of ONE instance refresh are answered it.
///
/// The preferences carry the same two members (`RefreshPreferences$MinHealthyPercentage` and
/// `$MaxHealthyPercentage`) under the same rule, and the service words it for them as an
/// instruction about the request it just refused: which member to add and which to remove.
pub const HEALTHY_PERCENTAGE_PAIR_REFRESH_MESSAGE: &str =
    "MinHealthyPercentage is required to use MaxHealthyPercentage. Specify MinHealthyPercentage, \
     or remove MaxHealthyPercentage, and try again.";

/// Whether a request STATES a pair of healthy-percentage thresholds for the first time,
/// CHANGES a pair that is already stored, or states the pair as the preferences of one
/// instance refresh — which is the only difference between the sentences the service answers
/// the pair rule with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthyThresholds {
    /// A create, which states the pair outright: there is no previously set value to clear.
    Created,
    /// An update, which changes the pair a group already carries, where a threshold may be
    /// cleared with [`RESET_PERCENTAGE`].
    Changed,
    /// The preferences of one instance refresh, which state the pair for that refresh alone.
    Refreshed,
}

impl HealthyThresholds {
    /// The sentence the service answers the pair rule with for this kind of request.
    #[must_use]
    pub fn pair_message(self) -> &'static str {
        match self {
            Self::Created => HEALTHY_PERCENTAGE_PAIR_MESSAGE,
            Self::Changed => HEALTHY_PERCENTAGE_PAIR_RESET_MESSAGE,
            Self::Refreshed => HEALTHY_PERCENTAGE_PAIR_REFRESH_MESSAGE,
        }
    }
}

/// What it answers a request whose two healthy-percentage thresholds are further apart than
/// the service allows.
///
/// "the difference between them cannot be greater than 100" (the same member's
/// documentation), which is the bound this sentence states. Fixed text: it names the two
/// members and the bound, and no value of the request.
pub const HEALTHY_PERCENTAGE_SPREAD_MESSAGE: &str =
    "The difference between MaxHealthyPercentage and MinHealthyPercentage must be less than or \
     equal to 100.";

/// The widest spread the two healthy-percentage thresholds of one request may have, from
/// that member's own documentation (see [`HEALTHY_PERCENTAGE_SPREAD_MESSAGE`]).
pub const MAX_HEALTHY_PERCENTAGE_SPREAD: i32 = 100;

/// The value either healthy-percentage threshold carries to CLEAR a previously set one: "To
/// clear a previously set value, specify a value of `-1`"
/// (`smithy_query(query="shape",shape="InstanceMaintenancePolicy")`).
///
/// A threshold being reset states no threshold, so the cross-field rules below are about the
/// thresholds that remain.
pub const RESET_PERCENTAGE: i32 = -1;

/// The `RetentionAction` that keeps an instance instead of terminating it: "set to `retain`
/// to move instances to a retained state"
/// (`smithy_query(query="shape",shape="RetentionAction")`, whose values are `retain` and
/// `terminate`).
pub const RETENTION_ACTION_RETAIN: &str = "retain";

/// What Amazon EC2 Auto Scaling answers a forced delete of a group whose abandoned-terminate
/// hook retains its instances.
///
/// Fixed text with no variable part: it names neither the group nor the trigger's value,
/// because the trigger it names is the only one that can hold the delete, and it closes with
/// the change the caller has to make.
pub const FORCE_DELETE_RETAINS_INSTANCES_MESSAGE: &str =
    "You can't force delete this Auto Scaling group because the TerminateHookAbandon retention \
     trigger is set to retain. Change the retention trigger to terminate and try again.";

/// The refusal for an Availability Zone (or Availability Zone id) the Region does not have.
///
/// The value the request supplied is the one variable part, in the brackets the service puts
/// it in, after the launch-source sentence every refusal of a group's placement and launch
/// source opens with ([`INVALID_LAUNCH_SOURCE`]).
#[must_use]
pub fn invalid_availability_zone_message(zone: &str) -> String {
    format!("{INVALID_LAUNCH_SOURCE} Invalid availability zone: [{zone}]")
}

/// The refusal for a request whose group would be placed in a zone its Region does not have,
/// or `None` when every zone it names is one of the Region's own.
///
/// A group places instances in the Availability Zones it names, and a zone exists in exactly
/// one Region, so a zone of no Region is a placement no instance could ever be launched into:
/// the request is refused instead of stored. Which zones a Region has is Amazon EC2's own
/// catalog ([`ec2::availability_zones_in_region`]) — the same one its subnets are placed from
/// and the same one [`super::ec2_subnets`] resolves a subnet's zone against — so one zone is
/// never known to two services differently.
///
/// Both spellings are resolved against that catalog: `AvailabilityZones` names a zone by its
/// NAME and `AvailabilityZoneIds` by its ID, and a value that is neither a name nor an id of
/// the Region names nothing. Zones are resolved in the order the request wrote them, so the
/// first value that names nothing is the one the refusal is about.
pub fn unknown_availability_zone_error<E>(
    region: &str,
    availability_zones: Option<&[String]>,
    availability_zone_ids: Option<&[String]>,
) -> Option<Scaffolded<E>> {
    let catalog = ec2::availability_zones_in_region(region);
    let unknown = availability_zones
        .unwrap_or_default()
        .iter()
        .find(|zone| !catalog.iter().any(|held| held.zone_name == **zone))
        .or_else(|| {
            availability_zone_ids
                .unwrap_or_default()
                .iter()
                .find(|zone_id| !catalog.iter().any(|held| held.zone_id == **zone_id))
        })?;
    Some(Scaffolded::validation_error(
        invalid_availability_zone_message(unknown),
    ))
}

/// Whether a request names exactly ONE launch source, and the refusal when it does not.
///
/// "Valid requests must contain either LaunchTemplate, LaunchConfigurationName, InstanceId or
/// MixedInstancesPolicy parameter": the four members are alternative ways of saying what a
/// group launches from, so a group has exactly one launch source and a request names exactly
/// one member for it. A request naming none cannot describe a group, and one naming several
/// describes two groups.
///
/// Decided from the members ALONE, which is why it is answered before any of their values is
/// resolved: a request naming two launch sources is refused for naming two, and the caller is
/// told to remove one rather than to go and create the launch configuration it also named.
pub fn launch_source_error<E>(
    instance_id: Option<&str>,
    launch_configuration_name: Option<&str>,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    let stated = [
        instance_id.is_some(),
        launch_configuration_name.is_some(),
        launch_template.is_some(),
        mixed_instances_policy.is_some(),
    ]
    .into_iter()
    .filter(|stated| *stated)
    .count();
    (stated != 1).then(|| Scaffolded::validation_error(ONE_LAUNCH_SOURCE_MESSAGE))
}

/// What Amazon EC2 Auto Scaling answers a launch-template specification that names no launch
/// template at all.
///
/// "Conditional: You must specify either a `LaunchTemplateId` or a `LaunchTemplateName`"
/// (`smithy_query(query="shape",shape="LaunchTemplateSpecification")`, which both halves
/// carry): a specification states the VERSION of ONE template, so one carrying neither half
/// names nothing to launch from. Fixed text naming the two members, with nothing of the
/// request in it — the rule is about the specification's MEMBERS, exactly as
/// [`ONE_LAUNCH_SOURCE_MESSAGE`]'s is about the request's.
pub const LAUNCH_TEMPLATE_IDENTITY_MESSAGE: &str =
    "Valid requests must contain either launchTemplateId or LaunchTemplateName";

/// What it answers a `MixedInstancesPolicy` that carries no `LaunchTemplate` AT ALL.
///
/// A distinct verdict from [`LAUNCH_TEMPLATE_IDENTITY_MESSAGE`], because the caller left out a
/// different MEMBER: a policy carrying no `LaunchTemplate` names nothing the halves of a
/// specification could be weighed on — "one or more launch templates and the instance types
/// (overrides) that are used to launch EC2 instances" (`MixedInstancesPolicy$LaunchTemplate`)
/// is what such a group launches from — while a policy that carries one and names no template
/// in it is told which pair of members to state. Fixed text: it states the requirement and the
/// change the caller has to make, and names nothing of the request.
pub const MIXED_INSTANCES_POLICY_TEMPLATE_MESSAGE: &str =
    "A launch template is required when configuring an Auto Scaling group with a mixed instances \
     policy. Add a launch template to the mixed instances policy and try again.";

/// The refusal for a launch source that does not name the launch template it launches from,
/// or `None` when it names one everywhere it has to.
///
/// Decided from the MEMBERS alone, so it is answered before any of their values is resolved
/// ([`unresolvable_launch_template_error`]): a launch source naming no template has nothing
/// to resolve, and the caller is told to name one rather than told that a template it never
/// named does not exist.
///
/// Three ways a launch source names none, in the order the members nest, and the first two
/// are their own verdict because the caller left out a different member:
///
/// * a `MixedInstancesPolicy` carries no `LaunchTemplate` member at all, so there is no launch
///   template in the policy to weigh and the caller is told to add one
///   ([`MIXED_INSTANCES_POLICY_TEMPLATE_MESSAGE`]). Read FIRST, for that reason;
/// * that member is there and names a template in NEITHER of the places it may carry one — its
///   own `LaunchTemplateSpecification` and the one each override may name
///   ([`super::ec2_instances::specifications`]) — so the pair of members that states the
///   template is what is missing ([`LAUNCH_TEMPLATE_IDENTITY_MESSAGE`]). ONE of the places is
///   enough: a policy whose overrides name the template they launch their type from has named
///   it;
/// * a specification the launch source carries states neither half — the group's own
///   `LaunchTemplate`, the launch template of its `MixedInstancesPolicy` and the one each
///   override of that policy may name are all read, the rule being a fact about the SHAPE and
///   holding wherever a request carries one — and that is the same pair of members, so it is
///   the same sentence.
pub fn launch_template_identity_error<E>(
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    if mixed_instances_policy.is_some_and(|policy| policy.launch_template.is_none()) {
        return Some(Scaffolded::validation_error(
            MIXED_INSTANCES_POLICY_TEMPLATE_MESSAGE,
        ));
    }
    let policy_names_no_template = mixed_instances_policy.is_some_and(|policy| {
        super::ec2_instances::specifications(None, Some(policy)).is_empty()
    });
    let a_specification_names_no_template =
        super::ec2_instances::specifications(launch_template, mixed_instances_policy)
            .into_iter()
            .any(|specification| TemplateIdentity::of(specification).is_none());
    (policy_names_no_template || a_specification_names_no_template)
        .then(|| Scaffolded::validation_error(LAUNCH_TEMPLATE_IDENTITY_MESSAGE))
}

/// The detail a refusal carries for a launch-template version that asks for a public address
/// on a launch of SEVERAL network interfaces.
///
/// "The public IP address can only be assigned to a network interface for eth0, and can only
/// be assigned to a new network interface, not an existing one. You cannot specify more than
/// one network interface in the request"
/// (`LaunchTemplateInstanceNetworkInterfaceSpecificationRequest$AssociatePublicIpAddress`):
/// the assignment and a launch of several interfaces cannot both happen, so a version asking
/// for both could launch no instance. Entirely fixed text — Amazon EC2's own wording for that
/// conflict, naming the parameter and nothing of the request.
const PUBLIC_ADDRESS_WITH_MULTIPLE_INTERFACES: &str =
    "The associatePublicIPAddress parameter cannot be specified when launching with multiple \
     network interfaces.";

/// The launch parameter a version has to state for ANY instance to be launched from it, as the
/// wire of the service that launches it spells the member.
///
/// "An AMI ID is required to launch an instance"
/// (`smithy_query(query="doc",shape="RunInstancesRequest",member="ImageId")`) — a version
/// stating no image names nothing to launch, so the detail a refusal carries for it is Amazon
/// EC2's own sentence for that parameter left out
/// ([`ec2::missing_wire_parameter_message`]), the one a caller launching from the same version
/// against that service is told.
const REQUIRED_LAUNCH_PARAMETER_IMAGE: &str = "ImageId";

/// The refusal for a launch source whose launch-template version could launch NO instance at
/// all, or `None` when every version it names could.
///
/// A group exists to launch instances from the version its specification names, so Amazon EC2
/// Auto Scaling reads that version's launch parameters when the group is configured and
/// refuses a request pointing it at parameters that cannot launch anything — the group would
/// otherwise be left permanently unable to hold capacity, with nothing telling the caller
/// why. The sentence is this service's own preamble for a launch source it cannot use
/// ([`INVALID_LAUNCH_SOURCE`]) followed by Amazon EC2's own detail for the conflict, exactly
/// as an instance profile that does not resolve is reported
/// ([`super::iam_instance_profiles`]).
///
/// Asked of every specification the launch source carries, for the reason
/// [`launch_template_identity_error`] reads them all: each is a template instances of the
/// group are launched from.
///
/// `None` covers a version the account does not hold — a reference that resolves to nothing
/// is [`unresolvable_launch_template_error`]'s refusal, and is read first — and an engine
/// carrying no Amazon EC2 store: a refusal has to be grounded in launch parameters the
/// account's Amazon EC2 state answered with, never assumed.
pub fn unlaunchable_launch_template_error<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    super::ec2_instances::specifications(launch_template, mixed_instances_policy)
        .into_iter()
        .filter_map(|specification| {
            super::ec2_instances::launch_template_data(engine, specification)
        })
        .find_map(|data| unlaunchable_launch_parameters(&data))
        .map(|detail| Scaffolded::validation_error(format!("{INVALID_LAUNCH_SOURCE} {detail}")))
}

/// Why the launch parameters of one launch-template version could launch no instance, or
/// `None` for parameters that could.
///
/// ONE reading of those parameters, so the two operations that configure a group and the
/// refresh that states a desired configuration cannot disagree about which versions are
/// launchable. Two ways a version launches nothing are read here:
///
/// * it states none of the launch parameters a launch REQUIRES
///   ([`REQUIRED_LAUNCH_PARAMETER_IMAGE`]) — a version carrying no image has nothing for an
///   instance to run, and a value present but empty names no image either;
/// * it names an instance TYPE that does not exist — no instance is ever placed as a type the
///   service that owns them does not publish ([`ec2::instance_type_is_modelled`]), so a group
///   pointed at such a version could hold no capacity, and the detail is that service's own
///   sentence for the value ([`ec2::instance_type_refusal_message`]). A version naming NO type
///   is a different verdict ([`launch_template_instance_type_error`]), because a mixed
///   instances group states the types elsewhere;
/// * it asks for the ASSIGNMENT of a public address on a launch of more than one interface —
///   `Some(true)` is what asks for the assignment, the reading Amazon EC2's own launch path
///   takes of the same member.
fn unlaunchable_launch_parameters(
    data: &::service_com_amazonaws_ec2_amazonec2::ops::types::RequestLaunchTemplateData,
) -> Option<String> {
    if data
        .image_id
        .as_deref()
        .is_none_or(|image_id| image_id.trim().is_empty())
    {
        return Some(ec2::missing_wire_parameter_message(
            REQUIRED_LAUNCH_PARAMETER_IMAGE,
        ));
    }
    if let Some(instance_type) = data
        .instance_type
        .as_deref()
        .map(str::trim)
        .filter(|instance_type| !instance_type.is_empty())
    {
        if !ec2::instance_type_is_modelled(instance_type) {
            return Some(ec2::instance_type_refusal_message(instance_type));
        }
    }
    let interfaces = data.network_interfaces.as_deref().unwrap_or_default();
    (interfaces.len() > 1
        && interfaces
            .iter()
            .any(|interface| interface.associate_public_ip_address == Some(true)))
    .then(|| PUBLIC_ADDRESS_WITH_MULTIPLE_INTERFACES.to_string())
}

/// What Amazon EC2 Auto Scaling answers a group whose launch-template version names no
/// instance type while the group is not a mixed instances group.
///
/// Entirely fixed text: it states the rule, the condition the rule holds under and the change
/// the caller has to make, and names neither the group nor the template — the launch parameter
/// that is missing is the only variable the sentence could carry, and it names that parameter
/// by its member name rather than by a value of the request.
pub const LAUNCH_TEMPLATE_INSTANCE_TYPE_MESSAGE: &str =
    "An instance type does not exist in the specified launch template. An instance type must be \
     defined in the launch template while the Auto Scaling group is not configured as a mixed \
     instances group. Use a different launch template that contains the InstanceType parameter, \
     and try again.";

/// The refusal for a launch source whose launch-template version states NO instance type while
/// the group it would configure is not a mixed instances group, or `None` when the instance
/// types the group launches are stated somewhere the group carries them.
///
/// An instance of a group is launched as one instance TYPE, and a group has exactly two places
/// to state it: the launch-template version it launches from (`InstanceType` of that version's
/// launch parameters) and the overrides of a mixed instances policy, which are "the instance
/// types (overrides) that are used to launch EC2 instances"
/// (`MixedInstancesPolicy$LaunchTemplate`) — so a group that is not configured as a mixed
/// instances group has to take the type from its template, and one pointed at a version
/// stating none could launch nothing. The refusal is the service's own
/// `InvalidQueryParameter` rather than the generic client-error code, because what the request
/// got wrong is not one of its own arguments but the contents of the launch template they
/// reference ([`Scaffolded::invalid_query_parameter`]).
///
/// Asked of every specification the launch source carries, for the reason
/// [`launch_template_identity_error`] reads them all, and asked AFTER
/// [`unlaunchable_launch_template_error`]: a version stating none of the launch parameters a
/// launch requires at all is reported for those parameters.
///
/// `None` covers a version the account does not hold — a reference that resolves to nothing is
/// [`unresolvable_launch_template_error`]'s refusal, and is read first — and an engine carrying
/// no Amazon EC2 store: a refusal has to be grounded in launch parameters the account's Amazon
/// EC2 state answered with, never assumed.
pub fn launch_template_instance_type_error<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    if policy_states_instance_types(mixed_instances_policy) {
        return None;
    }
    super::ec2_instances::specifications(launch_template, mixed_instances_policy)
        .into_iter()
        .filter_map(|specification| {
            super::ec2_instances::launch_template_data(engine, specification)
        })
        .any(|data| {
            data.instance_type
                .as_deref()
                .is_none_or(|instance_type| instance_type.trim().is_empty())
        })
        .then(|| Scaffolded::invalid_query_parameter(LAUNCH_TEMPLATE_INSTANCE_TYPE_MESSAGE))
}

/// Whether a launch source is configured as a MIXED INSTANCES group: one whose policy states
/// the instance types its capacity is spread over, so the template it launches from does not
/// have to state one.
///
/// A policy states them through its overrides — "the instance types (overrides) that are used
/// to launch EC2 instances" — each override naming either the type outright
/// (`LaunchTemplateOverrides$InstanceType`) or the attributes the service selects types by
/// instead (`$InstanceRequirements`: "Amazon EC2 Auto Scaling will select instance types with
/// the provided requirements"). A policy carrying no override states no type, so a group
/// configured with one is not spared the rule.
fn policy_states_instance_types(mixed_instances_policy: Option<&MixedInstancesPolicy>) -> bool {
    mixed_instances_policy
        .and_then(|policy| policy.launch_template.as_ref())
        .and_then(|template| template.overrides.as_ref())
        .into_iter()
        .flatten()
        .any(|entry| {
            entry
                .instance_type
                .as_deref()
                .is_some_and(|instance_type| !instance_type.trim().is_empty())
                || entry.instance_requirements.is_some()
        })
}

/// What Amazon EC2 Auto Scaling answers a launch source whose `Version` is a word that names
/// no version at all.
///
/// "Valid values are `$Latest`, `$Default`, and numbers"
/// (`smithy_query(query="shape",shape="LaunchTemplateSpecification",member="Version")`): a
/// version is one of the two aliases or a number, so a word that is neither names nothing the
/// template could be resolved at. Entirely fixed text — it lists the forms a version may take
/// and carries no part of the request, and it is this service's own wording rather than the
/// sentence the service that owns launch templates answers a caller of ITS OWN with, because
/// the request refused here configures a GROUP.
pub const UNREADABLE_LAUNCH_TEMPLATE_VERSION_MESSAGE: &str =
    "Invalid launch template version: either '$Default', '$Latest', or a numeric version are \
     allowed.";

/// The refusal for a launch source naming a launch template — or a VERSION of one — the
/// account does not hold, or `None` when every reference it names resolves.
///
/// A launch template is Amazon EC2's resource, and Amazon EC2 Auto Scaling resolves the one a
/// group launches from when the group is configured — which is why a group reports both
/// halves of the specification it was given ([`super::ec2_instances`]). A specification that
/// resolves to nothing names a template the group could never launch from, so the request is
/// refused instead of stored: a group created against it could launch nothing, and every read
/// of it would advertise a template Amazon EC2 does not have. The sentence is Amazon EC2's own
/// for the half of the reference that did not resolve ([`UnresolvedReference`]), which is what
/// a caller naming the same template against that service is told.
///
/// Every specification the launch source carries is resolved — the group's own
/// `LaunchTemplate`, the launch template of its `MixedInstancesPolicy` and the one each
/// override of that policy may name ([`super::ec2_instances::specifications`]) — because each
/// is a template instances of the group are launched from. A specification stating neither
/// half names no template to resolve and is left to the rule about the halves themselves.
///
/// `None` also covers an engine carrying no Amazon EC2 store at all: a refusal has to be
/// grounded in what the account's Amazon EC2 state answered, never assumed.
pub fn unresolvable_launch_template_error<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    first_unresolved(engine, launch_template, mixed_instances_policy)
        .map(UnresolvedReference::message)
        .map(Scaffolded::validation_error)
}

/// Why a LAUNCH from these launch sources reads no launch-template version at all, worded as
/// Amazon EC2 answers a launch naming a template it cannot resolve — or `None` when every
/// template they name still resolves.
///
/// A group is configured against a launch template that resolved
/// ([`unresolvable_launch_template_error`]), but the template is Amazon EC2's resource and
/// stays the client's to delete: the group keeps the reference it was given, so the reference
/// is resolved AGAIN at the instant of each launch, and one that no longer resolves is a
/// launch Amazon EC2 cannot make. The launch therefore places nothing and the reason is what
/// the client reads off the scaling activity's `StatusMessage`
/// ([`super::ec2_instances::launch_failure_reason`]), which is Amazon EC2's own sentence for
/// the half of the reference that did not resolve — the one a launch of a caller's own naming
/// the same template is answered.
///
/// `None` also covers an engine carrying no Amazon EC2 store at all: a launch is stopped only
/// on what the account's Amazon EC2 state answered, never on an assumption.
pub fn unresolved_launch_template_launch_failure<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<String> {
    first_unresolved(engine, launch_template, mixed_instances_policy)
        .map(UnresolvedReference::message)
}

/// The first launch-template reference of a launch source the account's own Amazon EC2 store
/// does not resolve, in the order the request names them, and WHICH half of it did not
/// resolve.
///
/// THE ONE resolution, shared by the refusal a request that CONFIGURES the group is answered
/// ([`unresolvable_launch_template_error`]) and the failed activity a LAUNCH from an
/// already-stored configuration records ([`unresolved_launch_template_launch_failure`]), so
/// the two cannot disagree about which references resolve, nor about how the half that did
/// not is worded ([`UnresolvedReference`]). EC2's store lock is taken for the one pass
/// that answers this and released before the answer is returned.
fn first_unresolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<UnresolvedReference> {
    let references: Vec<TemplateReference> =
        super::ec2_instances::specifications(launch_template, mixed_instances_policy)
            .into_iter()
            .filter_map(TemplateReference::of)
            .collect();
    if references.is_empty() {
        return None;
    }
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    references
        .into_iter()
        .find_map(|reference| reference.unresolved(&store))
}

/// WHICH half of one launch-template reference the account's own Amazon EC2 store could not
/// answer for.
///
/// A group launches from ONE VERSION of one template, so either half can stop resolving, and
/// each half carries ONE sentence — Amazon EC2's own for the half it names — whoever reads the
/// reference: the request that CONFIGURES a group against it and the LAUNCH that reads the
/// stored configuration are both told that the launch template, or the version of it, the
/// reference names does not exist.
#[derive(Debug, Clone)]
enum UnresolvedReference {
    /// Amazon EC2 cannot READ the id half the specification stated — a value no mint of that
    /// service ever produced — as that service words it.
    Unreadable(String),
    /// The account holds no launch template under the half the specification stated.
    Template(TemplateIdentity),
    /// It holds the template and not the version, as Amazon EC2 words that half.
    Version(String),
}

impl UnresolvedReference {
    /// The sentence this unresolved reference is reported with — to the caller whose request
    /// carries it, and in the activity of a launch that read it off a stored configuration.
    fn message(self) -> String {
        match self {
            Self::Unreadable(message) | Self::Version(message) => message,
            Self::Template(template) => template.absent_template_message(),
        }
    }
}

/// ONE launch-template reference a launch source carries: the template it names, and the
/// version of it the group would launch from.
#[derive(Debug, Clone)]
struct TemplateReference {
    /// How the specification names the template.
    template: TemplateIdentity,
    /// The `Version` the specification states, as it wrote it, or `None` when it states
    /// none: "the default value is `$Default`"
    /// (`smithy_query(query="shape",shape="LaunchTemplateSpecification")`), which is the
    /// version a group naming no version launches from.
    version: Option<String>,
}

impl TemplateReference {
    /// The reference `specification` carries, or `None` when it names no template — the
    /// empty string names none either.
    fn of(specification: &LaunchTemplateSpecification) -> Option<Self> {
        Some(Self {
            template: TemplateIdentity::of(specification)?,
            version: specification
                .version
                .as_deref()
                .filter(|version| !version.is_empty())
                .map(::std::string::ToString::to_string),
        })
    }

    /// WHICH half of this reference the account's own Amazon EC2 store does not answer for, or
    /// `None` when it holds the launch-template version the reference names.
    ///
    /// Both halves are resolved, because a group launches from ONE VERSION of one template:
    /// a version the template never minted is a launch source the group could never launch
    /// from, exactly as a template the account does not hold is. Either way the sentence a
    /// caller is answered is Amazon EC2's own for the half that did not resolve
    /// ([`UnresolvedReference`]) — the version half read from Amazon EC2's own resolution of
    /// it ([`ec2::resolve_launch_template_version`], the one reading of that member in the
    /// tree), so the two services cannot disagree about which version a word names or about
    /// how a caller is told it does not exist.
    fn unresolved(
        &self,
        store: &ec2::ComAmazonawsEc2Amazonec2State,
    ) -> Option<UnresolvedReference> {
        // Amazon EC2 SCREENS a launch template id before it looks it up, so a reference this
        // service resolves there is screened the same way ([`TemplateIdentity::unreadable`]):
        // an id that service's mint never issued is not an id of a launch template at all,
        // and is reported as the value it could not read rather than as a template that once
        // existed and has gone.
        if let Some(message) = self.template.unreadable(store) {
            return Some(UnresolvedReference::Unreadable(message));
        }
        let Some(record) = self.template.record(store) else {
            return Some(UnresolvedReference::Template(self.template.clone()));
        };
        let version = self
            .version
            .as_deref()
            .unwrap_or(ec2::LAUNCH_TEMPLATE_VERSION_DEFAULT);
        // WHICH version a word names is a rule about the word
        // ([`ec2::launch_template_version_request`], the one reading of that member in the
        // tree), and Amazon EC2 Auto Scaling words a word it cannot read ITSELF: the caller
        // configuring a GROUP is told which forms a version may take
        // ([`UNREADABLE_LAUNCH_TEMPLATE_VERSION_MESSAGE`]), not what Amazon EC2 answers a
        // caller of its own. A word that reads as a version and names none the template holds
        // is a different verdict, and stays Amazon EC2's own sentence for it.
        if ec2::launch_template_version_request::<()>(version).is_err() {
            return Some(UnresolvedReference::Version(
                UNREADABLE_LAUNCH_TEMPLATE_VERSION_MESSAGE.to_string(),
            ));
        }
        match ec2::resolve_launch_template_version::<()>(record, version) {
            Ok(_) => None,
            Err(Ec2Refusal::ServiceError(refusal)) => {
                Some(UnresolvedReference::Version(refusal.message))
            }
            // The resolution answers nothing but that refusal; a version it could not place
            // is unusable all the same, and the sentence for the version half is Amazon
            // EC2's own either way.
            Err(_) => Some(UnresolvedReference::Version(
                ec2::specified_launch_template_version_not_found_message(
                    record.launch_template_id(),
                    version,
                ),
            )),
        }
    }
}

/// How ONE launch-template specification names its template: "you must specify either a
/// `LaunchTemplateId` or a `LaunchTemplateName`" (`LaunchTemplateSpecification`).
#[derive(Debug, Clone)]
enum TemplateIdentity {
    /// The template's id, as the specification's `LaunchTemplateId` states it.
    Id(String),
    /// The template's name, as its `LaunchTemplateName` states it.
    Name(String),
}

impl TemplateIdentity {
    /// The identity `specification` carries, or `None` when it carries neither half — the
    /// empty string names no template either.
    fn of(specification: &LaunchTemplateSpecification) -> Option<Self> {
        if let Some(id) = specification
            .launch_template_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            return Some(Self::Id(id.to_string()));
        }
        let name = specification
            .launch_template_name
            .as_deref()
            .filter(|name| !name.is_empty())?;
        Some(Self::Name(name.to_string()))
    }

    /// Amazon EC2's own sentence for the id half of this identity where that service cannot
    /// READ it, or `None` for an identity it can read — every name, and every id it resolves
    /// by looking it up.
    ///
    /// Amazon EC2's LONG id form is an obfuscated identifier that service decodes before it
    /// resolves anything ([`ec2::ComAmazonawsEc2Amazonec2State::long_form_id_was_never_issued`],
    /// where that clause and its reasoning live), so a value in that form whose identifier its
    /// mint never issued names no launch template at all and is refused as unreadable BEFORE
    /// any lookup — ahead of the template an issued id names and may since have been deleted.
    /// Amazon EC2 Auto Scaling resolves a group's reference through that service, so a caller
    /// naming such an id is told what Amazon EC2 tells a caller naming it there
    /// ([`ec2::malformed_launch_template_id_message`]) rather than that a template of that id
    /// does not exist.
    ///
    /// A value OUTSIDE that form carries no identifier to decode, so it is not unreadable: it
    /// is looked up like any other reference and reported as a template that does not exist —
    /// the recorded answer for an id too short to be one. A NAME has no shape to be wrong
    /// either, so nothing is screened for the name half.
    fn unreadable(&self, store: &ec2::ComAmazonawsEc2Amazonec2State) -> Option<String> {
        match self {
            Self::Id(id) if store.long_form_id_was_never_issued(id) => {
                Some(ec2::malformed_launch_template_id_message(id))
            }
            Self::Id(_) | Self::Name(_) => None,
        }
    }

    /// The launch template of the account's own Amazon EC2 store this identity names, looked
    /// up by the half the specification stated — the same two lookups Amazon EC2 resolves its
    /// own launch-template references by — or `None` when the account holds none.
    fn record<'store>(
        &self,
        store: &'store ec2::ComAmazonawsEc2Amazonec2State,
    ) -> Option<&'store ec2::LaunchTemplateRecord> {
        match self {
            Self::Id(id) => store.launch_templates.get(id),
            Self::Name(name) => store
                .launch_template_id_for_name(name)
                .and_then(|id| store.launch_templates.get(&id)),
        }
    }

    /// What a reference naming a template the account does not hold is reported with: the
    /// sentence that names the half of the reference the launch source stated, worded by the
    /// service that owns launch templates, since the template Amazon EC2 Auto Scaling resolves
    /// IS that service's resource.
    ///
    /// The two halves are one sentence family — "the specified launch template, with template
    /// ID <id>, does not exist" ([`ec2::specified_launch_template_id_not_found_message`]) and
    /// "… with template name <name> …"
    /// ([`ec2::specified_launch_template_name_not_found_message`]) — whose fixed text differs
    /// only in which half it names and whose one variable part is the identifier the
    /// specification carries.
    ///
    /// THE ONE wording for a reference that resolves to no template, whether it is read when a
    /// group is CONFIGURED against it — the create, the update and the refresh that states a
    /// desired configuration — or when an instance is LAUNCHED from an already-stored
    /// configuration: the reference resolves to nothing in both cases, so the caller and the
    /// scaling activity are told the same thing about it.
    fn absent_template_message(&self) -> String {
        match self {
            Self::Id(id) => ec2::specified_launch_template_id_not_found_message(id),
            Self::Name(name) => ec2::specified_launch_template_name_not_found_message(name),
        }
    }
}

/// The refusal for a pair of healthy-percentage thresholds that cannot hold together, or
/// `None` when they can.
///
/// The thresholds bound how much of a group may be replaced at once: "both
/// `MinHealthyPercentage` and `MaxHealthyPercentage` must be specified, and the difference
/// between them cannot be greater than 100"
/// (`smithy_query(query="shape",shape="InstanceMaintenancePolicy")`). Neither rule is about
/// one member's value, so the model cannot state either and the service enforces both.
///
/// The ONE reading of that pair, for every request that carries one: the maintenance policy
/// a group is created or updated with ([`instance_maintenance_policy_error`]) and the
/// preferences an instance refresh is started with, which carry the same two members and are
/// answered the same sentences. A threshold of [`RESET_PERCENTAGE`] states no threshold, and
/// `thresholds` says which wording the pair rule is answered with ([`HealthyThresholds`]).
///
/// The pair rule is read in ONE direction, which is the direction the service enforces: an
/// upper threshold without a lower one is refused, while a LOWER threshold alone is served —
/// `MinHealthyPercentage` is the member that bounds a replacement on its own, and the
/// recorded refreshes that state it alone are accepted.
pub fn healthy_percentage_error<E>(
    min_healthy_percentage: Option<i32>,
    max_healthy_percentage: Option<i32>,
    thresholds: HealthyThresholds,
) -> Option<Scaffolded<E>> {
    let stated = |value: Option<i32>| value.filter(|percentage| *percentage != RESET_PERCENTAGE);
    match (
        stated(min_healthy_percentage),
        stated(max_healthy_percentage),
    ) {
        // One threshold without the other: an upper threshold bounds a lower one, so there is
        // nothing for it to bound when the request states none.
        (None, Some(_)) => Some(Scaffolded::validation_error(thresholds.pair_message())),
        (Some(min), Some(max)) if max.saturating_sub(min) > MAX_HEALTHY_PERCENTAGE_SPREAD => Some(
            Scaffolded::validation_error(HEALTHY_PERCENTAGE_SPREAD_MESSAGE),
        ),
        _ => None,
    }
}

/// The refusal for an instance maintenance policy whose two thresholds cannot hold together,
/// or `None` when they can — the group's own copy of [`healthy_percentage_error`].
pub fn instance_maintenance_policy_error<E>(
    policy: Option<&InstanceMaintenancePolicy>,
    thresholds: HealthyThresholds,
) -> Option<Scaffolded<E>> {
    let policy = policy?;
    healthy_percentage_error(
        policy.min_healthy_percentage,
        policy.max_healthy_percentage,
        thresholds,
    )
}

/// What Amazon EC2 Auto Scaling answers a mixed instances policy that overrides an instance
/// type more than once; the types the request stated twice are the one variable part, named
/// in the order the request first repeated each of them.
#[must_use]
pub fn duplicate_override_message(instance_types: &[&str]) -> String {
    format!(
        "Cannot add same instance type override more than once. Remove these duplicates from \
         the request and try again: [{}]",
        instance_types.join(", ")
    )
}

/// The refusal for a mixed instances policy that overrides the same instance type more than
/// once, or `None` when each type it names is named once.
///
/// An override says what to use INSTEAD of the launch template's own instance type, and
/// carries the weight that type counts for ("the number of capacity units provided by each
/// instance type", `LaunchTemplateOverrides$WeightedCapacity`): one type therefore has one
/// override, since two would give the group two weights for one type and no answer to which
/// of them a launch costs. The refusal names EVERY type the request repeated, which is what
/// the caller has to remove.
///
/// An override that names no instance type names it zero times — `InstanceRequirements` is
/// the alternative way of choosing types, and "if you specify `InstanceRequirements`, you
/// can't specify `InstanceType`" — so it is passed over rather than counted as a duplicate
/// of the other overrides that name none.
pub fn duplicate_override_error<E>(
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<Scaffolded<E>> {
    let overrides = mixed_instances_policy?
        .launch_template
        .as_ref()?
        .overrides
        .as_ref()?;
    let mut named: BTreeSet<&str> = BTreeSet::new();
    let mut repeated_once: BTreeSet<&str> = BTreeSet::new();
    let mut repeated: Vec<&str> = Vec::new();
    for instance_type in overrides
        .iter()
        .filter_map(|entry| entry.instance_type.as_deref())
        .filter(|instance_type| !instance_type.is_empty())
    {
        // Both memberships are set lookups, so the pass costs the same per override however
        // many the request carries: nothing here is the product of the request's own size.
        if !named.insert(instance_type) && repeated_once.insert(instance_type) {
            repeated.push(instance_type);
        }
    }
    (!repeated.is_empty())
        .then(|| Scaffolded::validation_error(duplicate_override_message(&repeated)))
}

/// Whether this instance lifecycle policy keeps a group's instances when a termination
/// lifecycle hook is abandoned.
///
/// "Specifies the action when a termination lifecycle hook is abandoned … set to `retain` to
/// move instances to a retained state" (`RetentionTriggers$TerminateHookAbandon`), and
/// "retained instances … remain until you call `TerminateInstanceInAutoScalingGroup`" — so a
/// group under this trigger has instances the service may not terminate on its own, which is
/// what a forced delete would have to do.
#[must_use]
pub fn retains_abandoned_terminations(policy: Option<&InstanceLifecyclePolicy>) -> bool {
    policy
        .and_then(|policy| policy.retention_triggers.as_ref())
        .and_then(|triggers| triggers.terminate_hook_abandon.as_deref())
        .is_some_and(|action| action == RETENTION_ACTION_RETAIN)
}

/// The refusal for a forced delete of a group whose abandoned-terminate hook retains its
/// instances.
///
/// A forced delete is "the group … deleted along with all instances associated with the
/// group" (`DeleteAutoScalingGroupType$ForceDelete`), and this trigger is the group's standing
/// instruction to RETAIN the instances a termination would otherwise take: the two cannot both
/// be honoured, so the call is refused and the group survives.
pub fn force_delete_retention_error<E>() -> Scaffolded<E> {
    Scaffolded::validation_error(FORCE_DELETE_RETAINS_INSTANCES_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the launch-source table: the case, whether the request names an instance,
    /// a launch configuration, a launch template and a mixed instances policy, and whether
    /// it is refused.
    type LaunchSourceRow = (&'static str, bool, bool, bool, bool, bool);

    /// One row of the threshold table: the case, `MinHealthyPercentage`,
    /// `MaxHealthyPercentage`, and the refusal expected of the pair.
    type ThresholdRow = (
        &'static str,
        Option<i32>,
        Option<i32>,
        Option<&'static str>,
    );

    /// The wire code and message of a refusal, for an assertion that reads both.
    fn refused<E>(error: &Scaffolded<E>) -> String {
        match error {
            Scaffolded::ValidationError(exception) => exception.message.clone(),
            _ => "not a ValidationError".to_string(),
        }
    }

    /// RULE: a request names exactly one launch source; naming none and naming several are
    /// the same refusal, and it is the one AWS states.
    #[test]
    fn exactly_one_launch_source_is_accepted() {
        let template = LaunchTemplateSpecification {
            launch_template_name: Some("lt".to_string()),
            ..LaunchTemplateSpecification::default()
        };
        let policy = MixedInstancesPolicy::default();
        // (case, instance id, launch configuration, launch template, mixed instances policy,
        //  whether the request is refused)
        let rows: &[LaunchSourceRow] = &[
            ("no launch source at all", false, false, false, false, true),
            ("a launch template alone", false, false, true, false, false),
            ("a launch configuration alone", false, true, false, false, false),
            ("a mixed instances policy alone", false, false, false, true, false),
            ("an instance alone", true, false, false, false, false),
            ("a launch template and a configuration", false, true, true, false, true),
            ("a launch template and a policy", false, false, true, true, true),
            ("an instance and a launch template", true, false, true, false, true),
            ("all four", true, true, true, true, true),
        ];
        for (case, instance, configuration, launch_template, mixed, expected) in rows {
            let error = launch_source_error::<()>(
                instance.then_some("i-0"),
                configuration.then_some("lc"),
                launch_template.then_some(&template),
                mixed.then_some(&policy),
            );
            assert_eq!(
                error.is_some(),
                *expected,
                "case {case:?}: expected refused {expected}, got {error:?}"
            );
            if let Some(error) = error {
                assert_eq!(
                    refused(&error),
                    ONE_LAUNCH_SOURCE_MESSAGE,
                    "case {case:?}: expected the one-of sentence"
                );
            }
        }
    }

    /// RULE: a maintenance policy states a lower threshold for every upper one, and the two
    /// are at most 100 apart; a threshold being reset states none.
    #[test]
    fn the_maintenance_policy_thresholds_must_hold_together() {
        // (case, MinHealthyPercentage, MaxHealthyPercentage, the expected refusal)
        let rows: &[ThresholdRow] = &[
            ("no policy member at all", None, None, None),
            ("a lower threshold alone", Some(90), None, None),
            ("both, 10 apart", Some(90), Some(100), None),
            ("both, exactly 100 apart", Some(0), Some(100), None),
            (
                "both, 101 apart",
                Some(49),
                Some(150),
                Some(HEALTHY_PERCENTAGE_SPREAD_MESSAGE),
            ),
            (
                "an upper threshold alone",
                None,
                Some(150),
                Some(HEALTHY_PERCENTAGE_PAIR_RESET_MESSAGE),
            ),
            (
                "an upper threshold beside a reset lower one",
                Some(RESET_PERCENTAGE),
                Some(150),
                Some(HEALTHY_PERCENTAGE_PAIR_RESET_MESSAGE),
            ),
            (
                "both reset",
                Some(RESET_PERCENTAGE),
                Some(RESET_PERCENTAGE),
                None,
            ),
        ];
        for (case, min, max, expected) in rows {
            let policy = InstanceMaintenancePolicy {
                min_healthy_percentage: *min,
                max_healthy_percentage: *max,
            };
            let error =
                instance_maintenance_policy_error::<()>(Some(&policy), HealthyThresholds::Changed);            match expected {
                Some(message) => {
                    let error = error.unwrap_or_else(|| {
                        panic!("case {case:?}: expected {message:?}, got no refusal")
                    });
                    assert_eq!(
                        refused(&error),
                        *message,
                        "case {case:?}: expected AWS's sentence for this rule"
                    );
                }
                None => assert!(
                    error.is_none(),
                    "case {case:?}: expected the policy to be accepted, got {error:?}"
                ),
            }
        }
    }

    /// RULE: a launch whose launch source names a launch template the account does not hold
    /// reports the sentence for the HALF of the reference the specification stated — the
    /// identifier it carries, never the other half and never a sentence naming no value.
    ///
    /// Driven on the reference itself rather than through a group, for the reason the reading
    /// of a launch configuration's instance profile is
    /// (`super::ec2_instances`' own tests): a group created against a template the account
    /// holds reports BOTH halves of the specification afterwards, so which half a stored
    /// reference states cannot be chosen over the wire, and the rule is held to here. The
    /// sentences are written out, not taken from the implementation.
    #[test]
    fn a_launch_names_the_half_of_the_reference_the_launch_source_stated() {
        // (case, the specification the launch source carries, the sentence the launch reports)
        let rows: &[(&str, LaunchTemplateSpecification, String)] = &[
            (
                "a reference stating the template's id",
                LaunchTemplateSpecification {
                    launch_template_id: Some("lt-0123456789abcdef0".to_string()),
                    ..LaunchTemplateSpecification::default()
                },
                "The specified launch template, with template ID lt-0123456789abcdef0, does \
                 not exist."
                    .to_string(),
            ),
            (
                "a reference stating only the template's name",
                LaunchTemplateSpecification {
                    launch_template_name: Some("web-tier".to_string()),
                    ..LaunchTemplateSpecification::default()
                },
                "The specified launch template, with template name web-tier, does not exist."
                    .to_string(),
            ),
            (
                "a reference stating both halves, which names the id",
                LaunchTemplateSpecification {
                    launch_template_id: Some("lt-00000000000000001".to_string()),
                    launch_template_name: Some("web-tier".to_string()),
                    ..LaunchTemplateSpecification::default()
                },
                "The specified launch template, with template ID lt-00000000000000001, does \
                 not exist."
                    .to_string(),
            ),
        ];
        for (case, specification, expected) in rows {
            let identity = TemplateIdentity::of(specification)
                .unwrap_or_else(|| panic!("case {case:?}: the specification names a template"));
            let reported = identity.absent_template_message();
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected the sentence naming the half the reference stated, \
                 got {reported:?}"
            );
        }
        // A specification stating NEITHER half names no template, so there is no reference to
        // resolve and no sentence to report.
        assert!(
            TemplateIdentity::of(&LaunchTemplateSpecification::default()).is_none(),
            "a specification naming neither half must name no template"
        );
    }
}
