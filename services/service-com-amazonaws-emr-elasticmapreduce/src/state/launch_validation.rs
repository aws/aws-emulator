// SPDX-License-Identifier: CC0-1.0

//! The REFERENCES a cluster launch names, resolved once the launch has been accepted, and the
//! shutdown a cluster built on one that resolves to nothing is settled into.
//!
//! # The rule this module implements
//!
//! A launch names resources that are not this service's: the Amazon VPC subnet its instances
//! are launched into ("set this parameter to the identifier of the Amazon VPC subnet where you
//! want the cluster to launch", `JobFlowInstancesConfig$Ec2SubnetId` doc) and the IAM role
//! Amazon EMR acts under ("the IAM role that Amazon EMR assumes in order to access Amazon Web
//! Services resources on your behalf", `RunJobFlow$ServiceRole` doc) — and, through that role,
//! the permission it needs for each Amazon EC2 resource the request ASKS Amazon EMR to create
//! for the cluster. None of them is resolved while the call is being answered: `RunJobFlow`
//! answers a `JobFlowId` for every request whose capacity it can describe, and the references
//! are resolved afterwards, in the services that OWN them.
//!
//! A reference that resolves to nothing is therefore not a refusal of the call but a failure of
//! the CLUSTER: it reaches `TERMINATED_WITH_ERRORS` through `TERMINATING`, carrying the modelled
//! `VALIDATION_ERROR` reason (`ClusterStateChangeReasonCode`) and the descriptive sentence for
//! what could not be resolved ([`ClusterRecord::begin_due_validation_failure`]), and NONE of the
//! Amazon EC2 resources that launch would have created is created: no instance is launched and
//! no placement group is made, because the launch never got past resolving what it named.
//!
//! The shutdown happens at the instant the cluster's capacity would have come up — the same
//! phase span the clock owes every launch ([`CLUSTER_PHASE_SPAN`]) — so a caller reading the
//! cluster straight after `RunJobFlow` still sees it `STARTING`, and the terminal state arrives
//! without anybody asking for it.
//!
//! Termination protection does not hold this shutdown off, as it holds off the one a failed
//! bootstrap action brings ([`super::bootstrap_run`]): protection "prevents the Amazon EC2
//! instances from being terminated" (`Cluster$TerminationProtected` doc) and this cluster has
//! none — the launch provisioned nothing — so there is nothing for it to keep. That is also
//! what makes it apply to a highly available cluster, whose launch forces protection on
//! ([`forces_termination_protection`]).
//!
//! # What grounds a failure
//!
//! Every one of these answers comes from the service that HOLDS the reference, over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]) this crate
//! reaches every other service through — Amazon EC2 for the subnet
//! ([`super::studio_network::ec2_client`]), IAM for the role and for what its permissions allow
//! (`service_common::iam_role`, the tree's one route to that answer). So a failure is settled
//! only where the owning service ANSWERED that the reference is not there, exactly as the
//! synchronous reading of a launch's `JobFlowRole` is ([`instance_profile_violation`]): an
//! emulator with no Amazon EC2 or no IAM mounted knows nothing about the launch's references and
//! settles no cluster, and neither does an answer this emulator could not read.

use ::service_common::iam_authorization::Decision;

use super::*;

/// The members a launch names its resolvable references under, as the model names them: what a
/// `VALIDATION_ERROR` sentence states so a caller learns WHICH reference to fix.
pub mod launch_reference {
    /// `JobFlowInstancesConfig$Ec2SubnetId`, the subnet the cluster's instances are launched
    /// into.
    pub const EC2_SUBNET_ID: &str = "Ec2SubnetId";

    /// `RunJobFlow$ServiceRole`, the role Amazon EMR acts under.
    pub const SERVICE_ROLE: &str = "ServiceRole";
}

/// The Amazon EC2 action that CREATES a placement group, which is the grant a launch asking
/// Amazon EMR for one needs on its service role ("ec2:CreatePlacementGroup", Amazon EC2 API
/// Reference, `CreatePlacementGroup`).
const CREATE_PLACEMENT_GROUP_ACTION: &str = "ec2:CreatePlacementGroup";

/// The Amazon EC2 resource type a placement group is named under in an ARN
/// (`arn:aws:ec2:<region>:<account>:placement-group/<name>`), which is the resource the grant
/// above is decided against.
const PLACEMENT_GROUP_RESOURCE_TYPE: &str = "placement-group";

/// The vendor that owns the resources a launch's references name.
const EC2_ARN_VENDOR: &str = "ec2";

/// `ClusterStateChangeReason$Message` — "the descriptive message for the state change reason"
/// — for a launch whose reference `member` resolved to nothing, naming the value the request
/// carried.
///
/// The template is the one this service's recorded refusal of its OTHER unresolvable reference
/// takes ([`invalid_instance_profile_message`], `Invalid InstanceProfile: <name>.`), with the
/// member named as the model names it ([`launch_reference`]): one sentence per reference, whose
/// variable part is the caller's own value, so a client is told which of its names resolved to
/// nothing.
#[must_use]
pub fn invalid_launch_reference_message(member: &str, value: &str) -> String {
    format!("Invalid {member}: {value}.")
}

/// `ClusterStateChangeReason$Message` for a launch whose service role EXISTS and does not allow
/// what Amazon EMR has to do under it to create the cluster's Amazon EC2 resources.
///
/// Published Amazon EMR wording for a service role that is missing the Amazon EC2 permissions
/// its cluster needs (Amazon EMR Management Guide, "Service role for Amazon EMR"), whose one
/// variable part is the role the launch named. It carries no trailing stop: the sentence ends
/// as Amazon EMR writes it.
#[must_use]
pub fn insufficient_ec2_permissions_message(service_role: &str) -> String {
    format!("Service role {service_role} has insufficient EC2 permissions")
}

/// Why the cluster this launch was accepted for cannot come up, or `None` when every reference
/// it names resolved — which includes every case where the owning service could not be asked,
/// for the reason this module's header gives.
///
/// The references are resolved in the order the launch needs them: the cluster's instances are
/// launched INTO the subnet, so that is resolved first; then the role Amazon EMR acts under, and
/// only then what that role's permissions allow — a role that does not exist has no permissions
/// to ask about.
///
/// `placement` is the placement the stored cluster launches with ([`LaunchPlacement`]), so this
/// resolves exactly the subnet the instances would be launched into and names each placement
/// group exactly as the launch would create it ([`LaunchPlacement::placement_group_name_for`]).
///
/// `configured_placement_group_roles` are the instance roles the REQUEST asked Amazon EMR to
/// place in a placement group of its own ("the configuration specifies the placement strategy
/// that can be applied to instance roles during cluster creation", `PlacementGroupConfig` doc).
/// It is what the role's permissions are judged against, because the grant a launch needs is
/// the grant for what it ASKED Amazon EMR to create: the placement group Amazon EMR applies to
/// a multi-primary cluster's primaries on its own is not something the request asked for, and a
/// launch that configured none needs no such grant at all — recorded behavior, which is how a
/// multi-primary launch whose role carries no placement-group grant still comes up while one
/// that CONFIGURES the primaries' placement group does not.
pub async fn launch_validation_failure(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    placement: &LaunchPlacement,
    service_role: Option<&str>,
    configured_placement_group_roles: &[String],
) -> Option<String> {
    if let Some(subnet_id) = placement.subnet_id.as_deref() {
        if subnet_is_absent(ctx, region, subnet_id).await {
            return Some(invalid_launch_reference_message(
                launch_reference::EC2_SUBNET_ID,
                subnet_id,
            ));
        }
    }
    // A launch that names no service role at all never got here: the member is required, and a
    // request carrying none is refused while it is being answered
    // ([`required_cluster_role_message`]).
    let service_role = service_role.filter(|role| !role.is_empty())?;
    let role_arn = match held_service_role(ctx, region, service_role).await {
        ServiceRoleAnswer::Held(role_arn) => role_arn,
        ServiceRoleAnswer::Absent => {
            return Some(invalid_launch_reference_message(
                launch_reference::SERVICE_ROLE,
                service_role,
            ))
        }
        ServiceRoleAnswer::Unknown => return None,
    };
    for role in configured_placement_group_roles {
        let group_arn = ::engine::emulator_arn(
            EC2_ARN_VENDOR,
            region,
            &format!(
                "{PLACEMENT_GROUP_RESOURCE_TYPE}/{}",
                placement.placement_group_name_for(role)
            ),
        );
        if role_withholds_action(
            ctx,
            region,
            &role_arn,
            CREATE_PLACEMENT_GROUP_ACTION,
            &group_arn,
        )
        .await
        {
            return Some(insufficient_ec2_permissions_message(service_role));
        }
    }
    None
}

/// Whether Amazon EC2 ANSWERED that the account holds no subnet under `subnet_id`.
///
/// Amazon EC2 answers a read naming an identifier it holds no subnet for by refusing it, under
/// one of the codes that say exactly that ([`super::studio_network`] states that prefix for the
/// same reading of a Studio's subnets), so a refusal under one of those is Amazon EC2 reporting
/// the subnet absent. Any other failure is this emulator failing to ASK — an emulator with no
/// Amazon EC2 mounted looks exactly like that, the transport being in-process — and establishes
/// nothing, which is traced rather than guessed at.
async fn subnet_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_id: &str,
) -> bool {
    let answer = studio_network::ec2_client(ctx, region)
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await;
    let error = match answer {
        // A read Amazon EC2 answered is one it resolved the id of. An answer that somehow
        // carries no row for it says nothing about the subnet either way.
        Ok(_) => return false,
        Err(error) => error,
    };
    let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error)
        .unwrap_or_default();
    if code.starts_with(studio_network::INVALID_SUBNET_ID) {
        return true;
    }
    ::tracing::debug!(
        requested = ?subnet_id,
        code = ?code,
        "the subnet a cluster launch named could not be resolved against Amazon EC2"
    );
    false
}

/// What IAM answers about the role a launch acts under.
enum ServiceRoleAnswer {
    /// IAM holds a role for the value the launch carried, under the ARN it reported for it —
    /// which is what the permissions behind it are then asked about.
    Held(String),
    /// IAM answered that this account holds no role the value names.
    Absent,
    /// Nothing was learned: the value names no role of this account, there is no IAM to ask, or
    /// the answer carried no role at all.
    Unknown,
}

/// The role IAM holds for the `ServiceRole` value `service_role`.
///
/// A role is named either by its friendly name or by its ARN, so an ARN is read through the
/// tree's one reading of an IAM role ARN (`service_common::iam_role::role_name`): a value that
/// is ARN-shaped but names another vendor, another IAM entity type, another account or a Region
/// an IAM ARN does not carry names no role of this account, and nothing is established about it
/// — another account's IAM is not this emulator's to ask.
///
/// The ARN answered is the one IAM REPORTS for the role, so a role held under a path is named
/// the way IAM holds it, and the permissions behind it are asked about under that same ARN.
async fn held_service_role(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    service_role: &str,
) -> ServiceRoleAnswer {
    let name = if service_role.starts_with("arn:") {
        match ::service_common::iam_role::role_name(service_role) {
            Some(name) => name,
            None => return ServiceRoleAnswer::Unknown,
        }
    } else {
        service_role
    };
    let answer = ::service_common::iam_client::iam_client(ctx, region)
        .get_role()
        .role_name(name)
        .send()
        .await;
    match answer {
        Ok(held) => match held.role() {
            Some(role) => ServiceRoleAnswer::Held(role.arn().to_string()),
            None => ServiceRoleAnswer::Unknown,
        },
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception,
            ) =>
        {
            ServiceRoleAnswer::Absent
        }
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the service role a cluster launch named could not be resolved against IAM"
            );
            ServiceRoleAnswer::Unknown
        }
    }
}

/// Whether IAM ANSWERED that the permissions in force on the role `role_arn` do not allow
/// `action` on `resource`.
///
/// Both halves of that answer come from IAM itself — the simulation of a principal's policies
/// and the two lists of its identity-based policies, asked through the tree's one route to them
/// (`service_common::iam_role`) — so the documents in force are collected and decided by the
/// service that HOLDS them.
///
/// A statement that DENIES the action withholds it. A policy set that merely says nothing about
/// it withholds it only when the role states permissions AT ALL: a role carrying no
/// identity-based policy is the routine state of one provisioned out of band, and its silence is
/// evidence about this store rather than about what the role may do, while a role that does
/// state permissions is the authority on what it may do. Nothing learned withholds nothing.
async fn role_withholds_action(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> bool {
    let decision = ::service_common::iam_role::role_action_decision_through(
        ctx.callback(),
        region,
        role_arn,
        action,
        resource,
        &[],
    )
    .await;
    match decision {
        Some(Decision::Denied) => true,
        Some(Decision::NotStated) => {
            ::service_common::iam_role::role_states_permissions_through(
                ctx.callback(),
                region,
                role_arn,
            )
            .await
        }
        Some(Decision::Allowed) | None => false,
    }
}

impl ClusterRecord {
    /// Start the shutdown the launch's own validation brings, once the clock has reached the
    /// instant it is owed at, answering whether it was started.
    ///
    /// The instant is the one the cluster's capacity would have come up at
    /// ([`ClusterStatusRecord::bootstrapping_due_at`]): until then the cluster is `STARTING`,
    /// exactly as every other launch's is, and from then it is shutting down for the reference
    /// its launch could not resolve. The shutdown itself is the one every failure of a cluster
    /// takes — `TERMINATING` for a [`TRANSITION_SPAN`] and then the terminal state its reason
    /// decides ([`terminal_cluster_state`]) — and it takes the cluster's step queue with it,
    /// because the nodes those steps would have run on were never provisioned.
    ///
    /// Answers `false` without touching the cluster when its launch resolved every reference it
    /// named, when the instant has not come, and when the cluster is no longer `STARTING`.
    pub(crate) fn begin_due_validation_failure(&mut self, now: SystemTime) -> bool {
        if self.status.state != cluster_state::STARTING {
            return false;
        }
        let Some(message) = self.validation_failure.clone() else {
            return false;
        };
        let Some(due) = self
            .status
            .bootstrapping_due_at
            .filter(|due| now >= *due)
        else {
            return false;
        };
        self.status.state = cluster_state::TERMINATING.to_string();
        self.status.state_change_reason = Some(StateChangeReasonRecord {
            code: cluster_state_change_reason_code::VALIDATION_ERROR.to_string(),
            message: Some(message),
        });
        self.status.bootstrapping_due_at = None;
        self.status.ready_due_at = None;
        let terminated_at = due + TRANSITION_SPAN;
        self.status.terminated_due_at = Some(terminated_at);
        for step in &mut self.steps {
            step.stop_with_cluster(terminated_at);
        }
        true
    }
}
