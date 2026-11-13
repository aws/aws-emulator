// SPDX-License-Identifier: CC0-1.0

//! What the account's IAM shows about the role a scheduled report executes as.
//!
//! A scheduled report does not run as whoever created it: the model documents
//! `ScheduledReportInput$scheduledReportExecutionRoleArn` as "the ARN of the IAM role that
//! the scheduled report uses to execute. Amazon Web Services Billing and Cost Management
//! Dashboards will assume this IAM role while executing the scheduled report". So the role
//! has to let this service assume it AND, once assumed, let the execution do its work —
//! `HealthStatus$statusCode` documents `HEALTHY` as the report being "configured properly
//! and has all required permissions to execute", and `StatusReason` names the two failures
//! separately: [`super::STATUS_REASON_EXECUTION_ROLE_ASSUME_FAILED`] for the assume itself
//! and [`super::STATUS_REASON_EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS`] for what the role's
//! own policies allow under it.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the stored record's own `execution_role_arn`, read off the record at the
//!    moment of each health resolution, so a report whose role `UpdateScheduledReport` has
//!    switched is judged by the role it carries then;
//! 2. **what the account's IAM says about it** — the trust document the IAM store holds for
//!    that role and the identity policies in force on it, which are the very records
//!    `CreateRole`, `UpdateAssumeRolePolicy`, `PutRolePolicy` and `AttachRolePolicy` wrote,
//!    reached through the SAME shared engine this service's store lives in and collected by
//!    IAM's own reading of them
//!    (`ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`);
//! 3. **the decision** — [`::service_common::iam_trust_policy::admits_service_principal`]
//!    for the assume and [`::service_common::iam_authorization`] for the work, the tree's
//!    ONE reading of a trust document and ONE evaluation of policy documents, so this
//!    service and every other one handed the same records answer alike.
//!
//! # The work an execution does
//!
//! An execution hands one notification to the account's default EventBridge event bus
//! ([`super::deliver_execution_notification`]) — the model defines the unhealthy state as
//! the report being "unable to deliver the notification to the default Amazon Web Services
//! EventBridge EventBus in your account" — and that publication is the one API call the
//! execution makes. So the permission the execution needs under its role is
//! [`EXECUTION_ACTION`] on that bus, and nothing is judged about calls this emulator does
//! not make.
//!
//! # Nothing is reported on evidence this emulator does not have
//!
//! Each of these shows nothing about the role and leaves the report's health to the rest of
//! the derivation: an engine carrying no IAM store, a `scheduledReportExecutionRoleArn` the
//! store holds no role for (an execution role is routinely provisioned out of band, so
//! "this emulator's IAM holds no such role" is evidence about the store and not about the
//! role — the same reading `service_common::iam_role::held_role_is_unassumable` takes for an
//! execution role), a role carrying no trust document, and a document that cannot be read as
//! a policy.
//!
//! The permission half is read the same way, and that is why only a DENY of the execution's
//! work is reported: an allow of it may live in a document this store does not hold (an
//! attachment whose policy is not carried here, a grant provisioned out of band), so
//! "nothing here allows it" is evidence about the store, while an explicit deny is a
//! statement the account itself made about that call — and an explicit deny cannot be
//! overridden by any grant, so it decides the request whatever else is in force.
//!
//! The judgement is taken as a SNAPSHOT of the IAM store ([`ExecutionRoleFaults`]) rather
//! than per report: a listing resolves the health of a whole page of reports, and one pass
//! over the account's roles answers for every one of them, with the IAM guard released
//! before this service's own store is read.

use std::collections::BTreeSet;

use ::service_common::iam_authorization::{self, Decision};

/// The IAM store a scheduled report's execution role ARN is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so a scheduled report is judged against the very records IAM's
/// handlers wrote rather than against a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live. `None`
/// means the engine carries no such store — a deployment with no IAM to ask, which is
/// evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The service principal a scheduled report's execution role has to trust for this service
/// to assume it, as a trust document names one: this service's own endpoint prefix under
/// `amazonaws.com`, which is how AWS names a service principal.
pub const SERVICE_PRINCIPAL: &str = "bcm-dashboards.amazonaws.com";

/// The API call one execution of a scheduled report makes under its execution role: the
/// publication of the execution's notification onto the account's default event bus
/// ([`super::deliver_execution_notification`]), which is what the model defines a scheduled
/// report's execution by.
pub const EXECUTION_ACTION: &str = "events:PutEvents";

/// The ARN namespace the account's event buses carry, as EventBridge's `aws.api#service`
/// trait declares it — the same label this crate addresses that service under when it
/// delivers the notification (`events.<region>.amazonaws.com`).
const EVENT_BUS_ARN_NAMESPACE: &str = "events";

/// The ARN of the default event bus the execution's notification goes to in `region`:
/// `arn:aws:events:<region>:<account>:event-bus/default`.
///
/// The account's buses are regional, so the resource the execution's permission is judged
/// against is the bus of the Region the request was signed for — the very bus
/// [`super::deliver_execution_notification`] publishes to.
fn default_event_bus_arn(region: &str) -> String {
    ::engine::emulator_arn(
        EVENT_BUS_ARN_NAMESPACE,
        region,
        &format!("event-bus/{}", super::DEFAULT_EVENT_BUS),
    )
}

/// What the account's IAM shows is WRONG with the execution roles of this account, as of the
/// call that resolved it.
///
/// Re-resolved on every health resolution and never cached on a record: a trust policy and a
/// role's permissions are edited outside this service, so the verdict is a property of the
/// IAM store at the instant of the call — which is also what makes a repaired role restore a
/// report's health with no further call to this service.
#[derive(Debug, Default, Clone)]
pub struct ExecutionRoleFaults {
    /// The ARN of every role the store HOLDS whose trust document withholds
    /// [`::service_common::iam_trust_policy::ASSUME_ROLE_ACTION`] from
    /// [`SERVICE_PRINCIPAL`]. A role the store does not hold, or whose document shows
    /// nothing, is absent — see the module docs.
    unassumable: BTreeSet<String>,
    /// The ARN of every role the store HOLDS that nothing shows this service cannot assume
    /// and whose policies in force deny it [`EXECUTION_ACTION`] on the account's default
    /// event bus.
    ///
    /// A role the trust document shows this service cannot assume is absent: the execution
    /// never gets as far as the permissions in force on it, so the assume is the whole
    /// fault. A role whose trust document shows NOTHING either way is judged here like any
    /// other, because an explicit deny of the execution's call is a statement the account
    /// made whatever its trust document says.
    without_permissions: BTreeSet<String>,
}

impl ExecutionRoleFaults {
    /// What the IAM store of `engine` shows about this account's execution roles, for a
    /// request answered in the Region `ctx` was signed for.
    ///
    /// The IAM guard is taken and released here, BEFORE this service's own store is read, so
    /// no caller holds two stores at once.
    pub fn resolve<Eng: Send + Sync + 'static>(
        engine: &Eng,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
    ) -> Self {
        let Some(iam) = iam_state(engine) else {
            // No IAM in this emulator to ask: nothing is shown about any role.
            return Self::default();
        };
        let event_bus_arn = default_event_bus_arn(&super::request_region(ctx));
        let held = iam.lock();
        Self::of(&held, &event_bus_arn)
    }

    /// The same reading against an IAM store the caller already holds, for the execution's
    /// notification going to the bus `event_bus_arn` names.
    ///
    /// One pass over the account's roles — bounded by IAM's own store rather than by a
    /// number chosen here — so resolving the health of a page of reports costs one walk and
    /// then a lookup per report.
    pub fn of(iam: &IamState, event_bus_arn: &str) -> Self {
        let mut faults = Self::default();
        for role in iam.roles.values() {
            if trust_withholds_this_service(role.assume_role_policy_document.as_deref()) {
                faults.unassumable.insert(role.arn.clone());
            } else if denies_execution_work(iam, &role.arn, event_bus_arn) {
                faults.without_permissions.insert(role.arn.clone());
            }
        }
        faults
    }

    /// Whether the account's IAM shows that this service cannot assume `role_arn`, so a
    /// report carrying it has nothing it could execute as.
    pub fn assume_failed(&self, role_arn: &str) -> bool {
        self.unassumable.contains(role_arn)
    }

    /// Whether the account's IAM shows that the role `role_arn` names, assumable as it is,
    /// may not do the work an execution does under it — so a report carrying it has a role
    /// it can take on and nothing it could do with it.
    pub fn insufficient_permissions(&self, role_arn: &str) -> bool {
        self.without_permissions.contains(role_arn)
    }
}

/// Whether the trust document a role the store HOLDS carries withholds the assume from
/// [`SERVICE_PRINCIPAL`]: the document is one this emulator can read as a policy AND it does
/// not admit this service.
///
/// A role carrying no trust document, and one carrying a document that cannot be read as a
/// policy, show nothing — see the module docs.
fn trust_withholds_this_service(document: Option<&str>) -> bool {
    document.and_then(|document| {
        ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL)
    }) == Some(false)
}

/// Whether the policies in force on the role `role_arn` names DENY it [`EXECUTION_ACTION`]
/// on the bus `event_bus_arn` names.
///
/// The documents are the ones IAM itself collects for the identity — its inline policies and
/// the default version of every managed policy attached to it — plus the ceiling attached to
/// it as a permissions boundary, because an explicit deny in that ceiling is in force just
/// as one in the identity's own policies is. The decision is the shared evaluation
/// ([`::service_common::iam_authorization`]), so a document this service reads and the same
/// document read by IAM's own simulation cannot be decided differently.
///
/// Only [`Decision::Denied`] is answered here: see the module docs for why a request nothing
/// in this store allows is not reported as a fault of the role.
fn denies_execution_work(iam: &IamState, role_arn: &str, event_bus_arn: &str) -> bool {
    let Some(carried) = iam.principal_policies(role_arn) else {
        // IAM resolves the ARN to no identity of this account: nothing is shown about what
        // it may do, exactly as nothing is shown about whether it can be assumed.
        return false;
    };
    let mut documents = carried.documents;
    documents.extend(iam.permissions_boundary_document(role_arn));
    let request = iam_authorization::Request::new(EXECUTION_ACTION, event_bus_arn, &[]);
    iam_authorization::identity_decision(&documents, &request) == Decision::Denied
}
