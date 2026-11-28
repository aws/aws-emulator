// SPDX-License-Identifier: CC0-1.0

//! Serving the CREATION of a log stream — and of the log GROUP a driver asked to create one
//! creates it in — this service is asked for by another AWS service's LOG DRIVER, on a
//! ROLE's behalf.
//!
//! A compute service that delivers a workload's output to this service does not sign a
//! request of its own: its agent creates the stream the output goes to AS a role of the
//! caller's account — an Amazon ECS task execution role, say — before the workload exists.
//! That creation is still this service's to answer, because the questions it turns on are
//! facts about this service's own resources, and they are the very ones `CreateLogStream`
//! itself answers:
//!
//! 1. **may this principal create it** — `logs:CreateLogStream` ("Creates a log stream for
//!    the specified log group", `CreateLogStream`) on the stream the driver names, decided
//!    over the documents in force on the role by the same shared evaluation this service's
//!    signed requests go through ([`super::identity_authorization`]);
//! 2. **is there a group to create it in** — `CreateLogStream` answers
//!    `ResourceNotFoundException` for a group this account does not hold, in this service's
//!    own words ([`super::resource_not_found_message`]), and that is what a driver
//!    configured to deliver into a group nobody created finds.
//!
//! The order is IAM's: authorization is decided before the resource is looked up, so a
//! principal that may not create the stream is told that and nothing about what the account
//! holds.
//!
//! A driver that was configured to create the group it delivers into answers the second of
//! those with a creation of its own — `logs:CreateLogGroup` ("Creates a log group with the
//! specified name", `CreateLogGroup`) on the group it names — and that is the one further
//! question this module serves ([`serve_log_group_creation`]): whether the role may create
//! that group, decided over the same documents and against the same resource ARN this
//! service decides its own signed `CreateLogGroup` against. It is asked only where the
//! driver makes that call, which the delivering service's own driver order decides.
//!
//! Once there is a stream, the driver PUTS the records into it — `logs:PutLogEvents`
//! ("Uploads a batch of log events to the specified log stream", `PutLogEvents`) on that
//! stream — and that is the last question of the same kind
//! ([`serve_log_events_delivery`]): a role a policy lets create the stream but not write to
//! it delivers no event, so each of the driver's calls is decided by the service that would
//! serve it rather than one standing for the others.
//!
//! A delivering service does not always hold the engine when the delivery happens — a log
//! router of a workload whose own clock owes the run decides it from a plane that service
//! opened earlier — so the two stores these questions read are taken once
//! ([`DeliveryPlane::of`], with a handler's `&*self.engine`) and kept. They are the SAME
//! stores the engine-borne entry points below resolve, so a delivery decided through either
//! answers alike.
//!
//! Nothing is withheld on evidence this emulator does not have. An engine carrying no IAM
//! store, a role that store holds no record for, and a role carrying an attachment whose
//! document this account does not hold (an entry of AWS's own catalog) each show nothing
//! about the role, so the creation is not refused on them — the same rule every other gate
//! over a role's documents in this tree keeps. A creation made with no principal at all is
//! not decided either: a caller that names no role states nothing for a policy to be read
//! against.
//!
//! A READ throughout: the decision consults the IAM store and this one and writes to
//! neither — the stream itself is created by the driver's own `CreateLogStream` call, which
//! is what a client then reads back — so deciding a creation changes nothing a later caller
//! of either service sees.
//!
//! LOCK ORDER: the IAM store's lock is taken with no guard of this service's store held, and
//! each guard hands back owned values — the contract that keeps a lock cycle between the
//! stores unreachable. A caller reaches this with no lock of its own held.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The operation a log driver creates the stream it delivers to through, which is the action
/// IAM publishes for it ("Actions defined by Amazon CloudWatch Logs", IAM Service
/// Authorization Reference).
pub const CREATE_LOG_STREAM_ACTION: &str = "CreateLogStream";

/// The operation a log driver creates the GROUP it delivers into through, when it was
/// configured to create one, which is the action IAM publishes for it ("Actions defined by
/// Amazon CloudWatch Logs", IAM Service Authorization Reference).
pub const CREATE_LOG_GROUP_ACTION: &str = "CreateLogGroup";

/// The operation a log driver PUTS the records it delivers through, which is the action IAM
/// publishes for it ("Actions defined by Amazon CloudWatch Logs", IAM Service Authorization
/// Reference).
pub const PUT_LOG_EVENTS_ACTION: &str = "PutLogEvents";

/// Why this service did not serve a log driver's stream creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCreationRefusal {
    /// No policy in force on the principal permits the creation: the sentence AWS refuses it
    /// with, under the class a refused call carries.
    NotAuthorized {
        /// The authorization refusal itself.
        message: String,
    },
    /// There is no log group to create the stream in — this account holds none by that name
    /// — carrying this service's own sentence about it.
    NotFound {
        /// Why nothing was created, in this service's own words.
        message: String,
    },
}

/// Why this service did not serve a log driver's creation of a log GROUP: no policy in force
/// on the principal permits it.
///
/// The group's own existence is not among the reasons — a driver reaches this call exactly
/// because there is no group — so this carries the one refusal such a creation earns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupCreationRefused {
    /// The authorization refusal itself: the sentence AWS refuses a call no policy permits
    /// with.
    pub message: String,
}

/// Why this service did not serve a log driver's DELIVERY of records into a stream: no
/// policy in force on the principal permits the put.
///
/// The stream's own existence is not among the reasons — the driver creates the stream it
/// delivers to before it puts anything ([`StreamCreationRefusal`]) — so this carries the one
/// refusal a put earns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRefused {
    /// The authorization refusal itself: the sentence AWS refuses a call no policy permits
    /// with.
    pub message: String,
}

/// The principal a log driver's stream creation is made as.
#[derive(Debug, Clone, Copy)]
pub struct DeliveryPrincipal<'a> {
    /// The IAM ROLE whose identity-based documents are in force on the creation.
    pub role_arn: &'a str,
    /// What a refusal NAMES as the principal: the ARN of the SESSION the delivering service
    /// assumed that role in, which is what AWS reports for a call made under one. `None`
    /// names the role itself, for a caller that holds no session of its own.
    pub session_arn: Option<&'a str>,
}

impl DeliveryPrincipal<'_> {
    /// The ARN a refusal names as the principal (see [`Self::session_arn`]).
    fn reported_arn(&self) -> &str {
        self.session_arn.unwrap_or(self.role_arn)
    }
}

/// The two stores a delivery made on a role's behalf is decided from: the log groups this
/// service holds, and the IAM records the role's documents are in.
///
/// Taken once and kept, for the deliverer that no longer holds the engine when the delivery
/// happens (see the module docs). The engine-borne entry points below build one of these per
/// call, so both forms read the same two stores.
#[derive(Clone)]
pub struct DeliveryPlane {
    /// This service's store, and `None` for an engine that carries none — which knows
    /// nothing about any group, so nothing is refused on it.
    logs: Option<::engine::ServiceState<crate::state::ComAmazonawsCloudwatchlogsLogs20140328State>>,
    /// The IAM store the principal's identity documents are read from, and `None` for an
    /// engine with no IAM — which shows nothing about the role, so nothing is withheld on it.
    identities: Option<
        ::engine::ServiceState<iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State>,
    >,
}

/// Written by hand rather than derived: what is diagnosable is whether the stores are there
/// at all, not what they hold.
impl ::std::fmt::Debug for DeliveryPlane {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("DeliveryPlane")
            .field("logs", &self.logs.is_some())
            .field("identities", &self.identities.is_some())
            .finish()
    }
}

impl DeliveryPlane {
    /// The stores in `engine`.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self {
            logs: crate::state::service_state(engine),
            identities: iam_store::service_state(engine),
        }
    }

    /// [`serve_log_stream_creation`], decided from these stores.
    pub fn serve_log_stream_creation(
        &self,
        region: &str,
        log_group: &str,
        log_stream: &str,
        principal: Option<DeliveryPrincipal<'_>>,
    ) -> Result<(), StreamCreationRefusal> {
        // The RESOURCE the creation addresses: the stream itself, which is what a grant
        // written for a log driver names (the group's ARN with the trailing wildcard covers
        // it).
        let resource = super::log_stream_arn(region, log_group, log_stream);
        if let Some(message) = self.withheld_call(
            principal,
            CREATE_LOG_STREAM_ACTION,
            &resource,
            "ServeLogStreamCreation",
            "no policy in force permits this role to create this log stream",
        ) {
            return Err(StreamCreationRefusal::NotAuthorized { message });
        }
        let Some(state) = self.logs.clone() else {
            // No store of this service to ask: nothing is known about the group, so the
            // creation is not refused on it.
            return Ok(());
        };
        let store = state.lock();
        if !store.log_groups.contains_key(log_group) {
            // The sentence `CreateLogStream` itself refuses a group this account does not
            // hold with, so the driver is told exactly what its own call would have been
            // told.
            return Err(StreamCreationRefusal::NotFound {
                message: super::resource_not_found_message("log group"),
            });
        }
        Ok(())
    }

    /// [`serve_log_group_creation`], decided from these stores.
    pub fn serve_log_group_creation(
        &self,
        region: &str,
        log_group: &str,
        principal: Option<DeliveryPrincipal<'_>>,
    ) -> Result<(), GroupCreationRefused> {
        // The resource ARN this service decides a `CreateLogGroup` against is the GROUP's own
        // ([`super::identity_authorization`]), so one group and one role are decided alike
        // however the creation is asked for.
        let resource = super::log_group_arn(region, log_group, false);
        match self.withheld_call(
            principal,
            CREATE_LOG_GROUP_ACTION,
            &resource,
            "ServeLogGroupCreation",
            "no policy in force permits this role to create this log group",
        ) {
            Some(message) => Err(GroupCreationRefused { message }),
            None => Ok(()),
        }
    }

    /// [`serve_log_events_delivery`], decided from these stores.
    pub fn serve_log_events_delivery(
        &self,
        region: &str,
        log_group: &str,
        log_stream: &str,
        principal: Option<DeliveryPrincipal<'_>>,
    ) -> Result<(), DeliveryRefused> {
        // `PutLogEvents` addresses the STREAM the records go into, which is the resource this
        // service decides its own signed put against.
        let resource = super::log_stream_arn(region, log_group, log_stream);
        match self.withheld_call(
            principal,
            PUT_LOG_EVENTS_ACTION,
            &resource,
            "ServeLogEventsDelivery",
            "no policy in force permits this role to put log events into this log stream",
        ) {
            Some(message) => Err(DeliveryRefused { message }),
            None => Ok(()),
        }
    }

    /// Why no policy in force on `principal` permits `action_name` on `resource`, as the
    /// sentence AWS refuses it with, or `None` when one does — or when nothing of the kind
    /// can be shown (see the module docs).
    ///
    /// `audit_operation` and `audit_message` name the call in the emulator's audit log.
    fn withheld_call(
        &self,
        principal: Option<DeliveryPrincipal<'_>>,
        action_name: &str,
        resource: &str,
        audit_operation: &'static str,
        audit_message: &'static str,
    ) -> Option<String> {
        withheld_call_over(
            self.identities.as_ref(),
            principal,
            action_name,
            resource,
            audit_operation,
            audit_message,
        )
    }
}

/// Serve the creation of the stream `log_stream` of the group `log_group` in `region`, made
/// on behalf of `principal`.
///
/// `Ok(())` for a creation this service would serve — and for every caller this gate does
/// not bound (see the module docs) — and the refusal otherwise, which the delivering service
/// reports in its own vocabulary. The stream is not created here: the driver creates it
/// through `CreateLogStream`, so what this answers is whether that call would be served at
/// all, which is what decides whether the workload the delivery is for can start.
pub fn serve_log_stream_creation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    log_group: &str,
    log_stream: &str,
    principal: Option<DeliveryPrincipal<'_>>,
) -> Result<(), StreamCreationRefusal> {
    DeliveryPlane::of(engine).serve_log_stream_creation(region, log_group, log_stream, principal)
}

/// Serve the creation of the log group `log_group` in `region`, made on behalf of
/// `principal` by a driver that was configured to create the group it delivers into.
///
/// `Ok(())` for a creation this service would serve — and for every caller this gate does
/// not bound (see the module docs) — and the refusal otherwise, which the delivering service
/// reports in its own vocabulary. The group is not created here: the driver creates it
/// through `CreateLogGroup`, so what this answers is whether that call would be served at
/// all, which is what decides whether the workload the delivery is for can start.
pub fn serve_log_group_creation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    log_group: &str,
    principal: Option<DeliveryPrincipal<'_>>,
) -> Result<(), GroupCreationRefused> {
    DeliveryPlane::of(engine).serve_log_group_creation(region, log_group, principal)
}

/// Serve the PUT of records into the stream `log_stream` of `log_group` in `region`, made on
/// behalf of `principal` by a log driver delivering a workload's output.
///
/// `Ok(())` for a put this service would serve — and for every caller this gate does not
/// bound (see the module docs) — and the refusal otherwise. Nothing is written here: the
/// driver puts the records through `PutLogEvents`, so what this answers is whether that call
/// would be served at all, which is what decides whether the workload's output is delivered.
pub fn serve_log_events_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    log_group: &str,
    log_stream: &str,
    principal: Option<DeliveryPrincipal<'_>>,
) -> Result<(), DeliveryRefused> {
    DeliveryPlane::of(engine).serve_log_events_delivery(region, log_group, log_stream, principal)
}

/// Why no policy in force on `principal` permits `action_name` on `resource`, as the sentence
/// AWS refuses it with, or `None` when one does — or when nothing of the kind can be shown
/// (see the module docs).
///
/// `audit_operation` and `audit_message` name the call in the emulator's audit log.
fn withheld_call_over(
    identities: Option<
        &::engine::ServiceState<iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State>,
    >,
    principal: Option<DeliveryPrincipal<'_>>,
    action_name: &str,
    resource: &str,
    audit_operation: &'static str,
    audit_message: &'static str,
) -> Option<String> {
    let principal = principal?;
    let role_arn = principal.role_arn;
    let iam = identities?;
    let (documents, boundary) = {
        let store = iam.lock();
        // A role this account's IAM holds no record for may belong to another account, so
        // its documents are not this store's to read.
        store.roles.values().find(|role| role.arn == role_arn)?;
        let policies = store.principal_policies(role_arn)?;
        // An attachment whose document this account does not hold — an entry of AWS's own
        // catalog, such as the managed policy AWS publishes for this very delivery — may be
        // the policy that carries the grant, so nothing is withheld on it.
        if !policies.uncarried.is_empty() {
            return None;
        }
        (
            policies.documents,
            store.permissions_boundary_document(role_arn),
        )
    };
    let action = super::identity_authorization::action_for(action_name);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the creation addressed.
    let request = iam::Request::new(&action, resource, &[]);
    let identity = iam::identity_decision(&documents, &request);
    // IAM's own evaluation order, the order this service's gate for a signed request takes
    // too ([`super::identity_authorization`]), so one group and one role are decided alike
    // however the creation is asked for.
    let reason = if identity == Decision::Denied {
        iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
    } else if identity != Decision::Allowed {
        iam::no_identity_policy_allows(&action)
    } else {
        match boundary.as_ref().map_or(Decision::Allowed, |document| {
            iam::identity_decision(::std::slice::from_ref(document), &request)
        }) {
            Decision::Allowed => return None,
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        }
    };
    // The refusal is the security-relevant event of this creation, so it is attributable in
    // the emulator's audit log the way this service's own gate logs a refused request: who
    // the creation was made as, what it asked for, and the outcome. No policy document is
    // logged, and no log event is read.
    ::tracing::info!(
        target: "emulator::audit",
        audit = true,
        operation = audit_operation,
        outcome = "refused_access_denied",
        principal = %principal.reported_arn(),
        action = %action,
        resource = %resource,
        explicit_deny = identity == Decision::Denied,
        "{audit_message}"
    );
    Some(iam::not_authorized_by_identity_policy(
        principal.reported_arn(),
        &action,
        resource,
        &reason,
    ))
}
