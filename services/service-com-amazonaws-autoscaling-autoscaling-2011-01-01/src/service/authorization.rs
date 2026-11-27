// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the Auto Scaling action it names —
//! the decision taken ahead of every ACCOUNT-WIDE READ, before any handler reaches the
//! store.
//!
//! Amazon EC2 Auto Scaling refuses a caller that may not perform the action it asked for
//! with HTTP 403 and the error code [`ACCESS_DENIED`], naming the principal the credential
//! speaks for and the action it asked for. The decision is a fact about the REQUEST's
//! credential and the policies in force at the instant of the call, not about the resources
//! the request names — which is why it is taken once here rather than per handler.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default
//!    deny.
//!
//! Only the store LOOKUPS live here; neither the decision, nor the refusal's sentence, nor
//! the ARN readings do — each comes from `services/common`, which is where the one copy of
//! them lives.
//!
//! ## How each action is decided
//!
//! An Auto Scaling `Describe` action supports no resource-level permissions: the IAM
//! Service Authorization Reference lists every `autoscaling:Describe*` action under
//! "actions with no resource types", so IAM judges one against every resource
//! ([`EVERY_RESOURCE`]) and a statement permits it only by covering every resource. That
//! makes the decision for such an action a pure function of the caller's policies and the
//! action name: [`judged_over_every_resource`] is the test.
//!
//! Every OTHER Auto Scaling action is judged by IAM against the ARN of the GROUP (or other
//! resource) the request addresses, which this gate does not resolve: the request carries a
//! NAME, and which resource an action is authorized on differs per operation. So that
//! decision belongs to the handler that knows which resource its operation addresses, and
//! this module states it once for the group-scoped ones ([`group_action_refusal`]), which
//! take it before they read or write anything: the refusal then names the group's ARN, as
//! IAM's own sentence does.
//!
//! A request can also ask for an action of ANOTHER service, which IAM judges beside the Auto
//! Scaling one: a group configured to launch from a launch template launches its instances
//! from that template, so pointing a group at one asks for a launch from it as well
//! ([`launch_template_refusal`], judged as [`RUN_INSTANCES_ACTION`] against the template's
//! own ARN). Such a decision rests on the same caller, the same documents and the same
//! evaluation as the service's own action; only the action name and the resource differ.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the account's store may hold no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises
//! the session expiries the clock has already made due; that write is owed at the same
//! instant whether or not anybody called, which is why a REPORTING operation may take this
//! decision too.

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema_server::ProtocolError;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::{Refused, Service};

/// This service's IAM action prefix: an action is `autoscaling:<OperationName>` ("Actions
/// defined by Amazon EC2 Auto Scaling", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "autoscaling";

/// The resource an action that supports no resource-level permissions is judged against, as
/// IAM writes it for such a request: every resource.
pub const EVERY_RESOURCE: &str = "*";

/// The error code Amazon EC2 Auto Scaling refuses an unauthorized caller with, which is
/// what a client branches on. A CLIENT fault, so the query envelope reports it as
/// `<Type>Sender</Type>` with HTTP 403.
pub const ACCESS_DENIED: &str = "AccessDenied";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The name prefix of the Auto Scaling operations whose action IAM judges over every
/// resource. See the module docs for the documented rule this is the one test of.
const ACCOUNT_WIDE_READ_PREFIX: &str = "Describe";

/// Whether IAM judges `operation`'s action over every resource, so this gate can decide it
/// from the caller's policies and the action name alone (see the module docs).
#[must_use]
pub fn judged_over_every_resource(operation: &str) -> bool {
    operation.starts_with(ACCOUNT_WIDE_READ_PREFIX)
}

/// An authorization refusal: who was refused, which action, on which resource when the
/// decision was taken on one, and which of IAM's rules withheld the grant.
#[derive(Debug, Clone)]
pub struct Denied {
    /// The principal the request speaks for, as `sts:GetCallerIdentity` reports it.
    principal: String,
    /// The IAM action the request asked for.
    action: String,
    /// The ARN of the resource the request was refused ON, when the decision was taken
    /// against a resource the request addressed; `None` when there was none to name.
    resource: Option<String>,
    /// Why the grant was withheld, as the refusal's closing clause words it — one of the
    /// four clauses `services/common` states.
    reason: String,
}

impl Denied {
    /// The refusal's message: the principal the credential speaks for, the action the
    /// request asked for, the resource it was refused on where the decision was taken on
    /// one, and which of IAM's rules withheld the grant.
    ///
    /// An action IAM judges over every resource has NO resource to name, which is why that
    /// form of the sentence is not
    /// [`::service_common::iam_authorization::not_authorized_by_identity_policy`], whose own
    /// documentation says a call addressing no single resource is outside it; an action judged
    /// against the resource the request addresses names that resource, and takes exactly that
    /// shared sentence.
    #[must_use]
    pub fn message(&self) -> String {
        match &self.resource {
            Some(resource) => iam::not_authorized_by_identity_policy(
                &self.principal,
                &self.action,
                resource,
                &self.reason,
            ),
            None => format!(
                "User: {} is not authorized to perform: {} {}",
                self.principal, self.action, self.reason
            ),
        }
    }
}

/// This service's [`ACCESS_DENIED`] refusal of `operation` on the Auto Scaling group
/// `group_name`, or `None` when the IAM policies in force permit it.
///
/// Every Auto Scaling action but the account-wide reads is judged by IAM against the ARN of
/// the GROUP the request addresses, and a request names that group by NAME: so the ARN it is
/// judged on — and the one a refusal names — is the group's OWN, as this service minted it
/// and as `DescribeAutoScalingGroups` reports it under `AutoScalingGroupARN`. A name this
/// store holds no group under has no such ARN to read: the id segment is then the wildcard
/// IAM itself writes for an id a request does not state, which is the position a group being
/// CREATED is in. The decision is a handler's to take — only the handler knows which resource
/// its operation addresses — and it is taken BEFORE the handler reads or writes anything,
/// because a refused request must leave the account exactly as it was.
///
/// Carried as this service's own 403 `AccessDenied` error
/// ([`crate::scaffolded::Scaffolded::access_denied`]), so it is rendered in the same envelope
/// as every other error of this service.
pub fn group_action_refusal<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    group_name: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let region = crate::state::request_region(ctx.parts());
    let resource = group_resource_arn(engine, &region, group_name);
    let refusal = authorize_on_resource(engine, ctx, operation, &resource).err()?;
    Some(crate::scaffolded::Scaffolded::access_denied(
        refusal.message(),
    ))
}

/// The ARN IAM judges an action on the group `group_name` against: the ARN the group itself
/// carries, read from this service's own store, and the wildcard-id form of it
/// ([`crate::state::group_arn`]) for a name no group of the account answers to.
///
/// The store's lock is taken for the one lookup and released before this returns, so the
/// decision below holds no guard — and this reads only, which is what an authorization
/// decision may do.
fn group_resource_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    group_name: &str,
) -> String {
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        store.groups.get(group_name).map(|group| group.arn.clone())
    });
    held.unwrap_or_else(|| crate::state::group_arn(region, EVERY_RESOURCE, group_name))
}

/// Whether IAM lets this request perform the action of `operation` on `resource`.
///
/// The decision every caller of [`group_action_refusal`] takes, over the ARN that caller
/// resolved: the same evaluation the account-wide reads take, against a resource instead of
/// against every resource.
fn authorize_on_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), Denied> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    authorize_action_on(engine, ctx, &action, resource)
}

/// Whether IAM lets this request perform `action` on `resource`.
///
/// `action` is the fully qualified action name, so this states the decision for an action of
/// ANOTHER service the request asks for as well as for this service's own
/// ([`launch_template_refusal`]): the caller, the documents in force on it and the evaluation
/// are the same whichever service's action is judged, and only the action name and the
/// resource differ.
fn authorize_action_on<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), Denied> {
    let caller = caller(engine, ctx);
    let documents = iam::prepare_documents(&caller.documents);
    let boundary = caller
        .boundary
        .as_ref()
        .map(|boundary| iam::prepare_documents(::std::slice::from_ref(boundary)));
    decide(
        &caller,
        &documents,
        boundary.as_deref(),
        action,
        resource,
        Some(resource),
    )
}

/// The Amazon EC2 action using a launch template to launch an instance requires.
///
/// A group configured to launch from a launch template launches its instances from that
/// template, so a request that points a group at one asks for a launch from it as well as for
/// the Auto Scaling action it names, and IAM judges that launch as `ec2:RunInstances` on the
/// launch template — one of the resource types the IAM Service Authorization Reference lists
/// for that action ("Resource types defined by Amazon EC2": `launch-template`).
pub const RUN_INSTANCES_ACTION: &str = "ec2:RunInstances";

/// The fixed text Amazon EC2 Auto Scaling refuses a caller that may not launch from the
/// launch template a request names with. What follows it is the launch template the refusal is
/// about, which is the only part taken from the request
/// ([`unauthorized_launch_template_message`]).
///
/// The refusal is about the TEMPLATE rather than about the policy statement that withheld the
/// grant, so this is the sentence whichever of IAM's rules withheld it — an explicit deny and
/// a missing allow are refused alike, and nothing of the caller's policies is quoted back.
const UNAUTHORIZED_LAUNCH_TEMPLATE_PREFIX: &str = "You are not authorized to use launch template: ";

/// The refusal naming the launch template `launch_template_id` the caller may not launch from:
/// the id Amazon EC2 holds that template under, which is how Amazon EC2 identifies a launch
/// template and what every read of a group pointed at one reports.
#[must_use]
pub fn unauthorized_launch_template_message(launch_template_id: &str) -> String {
    format!("{UNAUTHORIZED_LAUNCH_TEMPLATE_PREFIX}{launch_template_id}")
}

/// The ARN IAM judges a launch from the launch template `launch_template_id` against: Amazon
/// EC2's own form for a launch template of this account in the Region the request was signed
/// for, which is the resource a policy scoping the launch to particular templates names.
fn launch_template_resource(region: &str, launch_template_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("launch-template/{launch_template_id}"),
    )
}

/// This service's [`ACCESS_DENIED`] refusal of a request that would point a group at a launch
/// template the caller may not launch from, or `None` when the IAM policies in force permit
/// the launch from every template it names.
///
/// The decision is the SAME evaluation every action of a request takes
/// ([`authorize_action_on`]), over [`RUN_INSTANCES_ACTION`] and the ARN of ONE launch template
/// ([`launch_template_resource`]) — so a policy that grants the launch on one template does
/// not grant it on another — and it is taken for each template the request names
/// ([`crate::state::ec2_instances::launch_template_ids`]), in the order the request names
/// them: the refusal is about the first template the caller may not launch from.
///
/// Taken by the handler that configures the group's launch source, once that source has
/// resolved — the id judged names a template the account HOLDS — and before anything is
/// written, because a refused request must leave the account exactly as it was.
pub fn launch_template_refusal<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    launch_template: Option<&crate::ops::types::LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&crate::ops::types::MixedInstancesPolicy>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let region = crate::state::request_region(ctx.parts());
    let refused = crate::state::ec2_instances::launch_template_ids(
        engine,
        launch_template,
        mixed_instances_policy,
    )
    .into_iter()
    .find(|launch_template_id| {
        let resource = launch_template_resource(&region, launch_template_id);
        authorize_action_on(engine, ctx, RUN_INSTANCES_ACTION, &resource).is_err()
    })?;
    Some(crate::scaffolded::Scaffolded::access_denied(
        unauthorized_launch_template_message(&refused),
    ))
}

/// Whether IAM lets this request perform the action of `operation` — the operation's own
/// name, which is the routing key the request named.
///
/// `Ok(())` for a request IAM permits, for an operation whose action is judged against the
/// ARN of a resource this gate does not read (those are decided by the handler that knows
/// which resource they address — [`group_action_refusal`]), and for every caller this gate
/// does not bound (see the module docs); `Err` carries the refusal this service answers as
/// [`ACCESS_DENIED`].
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Result<(), Denied> {
    // An action that supports no resource-level permissions is judged over EVERY resource,
    // so a statement permits it only by covering every resource — which is the decision this
    // gate can take from the caller's policies and the action name alone.
    if !judged_over_every_resource(operation) {
        return Ok(());
    }
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    let documents = iam::prepare_documents(&caller.documents);
    let boundary = caller
        .boundary
        .as_ref()
        .map(|boundary| iam::prepare_documents(::std::slice::from_ref(boundary)));
    decide(
        &caller,
        &documents,
        boundary.as_deref(),
        &action,
        EVERY_RESOURCE,
        // There is no resource to name in the refusal of an action judged over every one.
        None,
    )
}

/// IAM's decision about `action` on `judged`, refused in the name of `named`.
///
/// `named` is what a refusal states the request was refused ON — the resource the request
/// addressed, or `None` where the decision was taken over every resource and there is no
/// resource to name.
fn decide(
    caller: &Caller,
    documents: &[iam::PreparedDocument],
    boundary: Option<&[iam::PreparedDocument]>,
    action: &str,
    judged: &str,
    named: Option<&str>,
) -> Result<(), Denied> {
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(action, judged, &caller.principals);
    let identity_said = iam::identity_decision_prepared(documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(caller.refused(action, named, iam::IDENTITY_POLICY_EXPLICIT_DENY));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(caller.refused(action, named, &iam::no_identity_policy_allows(action)));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = boundary {
        let reason = match iam::identity_decision_prepared(boundary, &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(action),
        };
        return Err(caller.refused(action, named, &reason));
    }
    Ok(())
}

/// This service's [`ACCESS_DENIED`] response for a request whose caller the IAM policies in
/// force do not permit to perform the action it named, or `None` when they do.
///
/// The action asked about is the operation's own name, taken from the request's routing key
/// and only when the routing table SERVES that key: so it is always a modelled operation's
/// name, never an arbitrary value the body carried, and a request naming no operation of
/// this service still reaches the routing failure rather than an authorization decision
/// about a name nothing serves.
///
/// The code and the sentence are this service's ([`ACCESS_DENIED`], [`Denied::message`]);
/// the status is the 403 the framework's own pre-dispatch renderer answers an authorization
/// refusal with, and the envelope is the protocol's, closed by this service's own
/// `<RequestId>` writer — so this refusal is rendered exactly like every other error of
/// this service.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<RtResponse> {
    let operation = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(operation)?;
    let refusal = authorize(engine, ctx, operation).err()?;
    Some(super::query_error_envelope::with_request_id(
        ::smithy_server_runtime::protocol_error_response_with(
            ::smithy_server_runtime::PreDispatchProtocol::Query { ec2: false },
            ProtocolError::Unauthenticated {
                code: ACCESS_DENIED,
                message: ACCESS_DENIED,
            },
            Some(Refused {
                code: ACCESS_DENIED.to_string(),
                message: refusal.message(),
            }),
        ),
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// This caller's refusal of `action` on `named`, closed by `reason`.
    fn refused(&self, action: &str, named: Option<&str>, reason: &str) -> Denied {
        Denied {
            principal: self.arn.clone(),
            action: action.to_string(),
            resource: named.map(str::to_string),
            reason: reason.to_string(),
        }
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, EVERY_RESOURCE, &[]),
        )
    }

    /// RULE: the action this gate asks about is `autoscaling:<OperationName>`, so a policy
    /// scoped to this service's actions covers the operations of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"autoscaling:*","Resource":"*"}]}"#;
        for operation in [
            "DescribeTags",
            "DescribeAutoScalingGroups",
            "DescribeScalingActivities",
            "DescribeAccountLimits",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected autoscaling:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "elasticloadbalancing:DescribeTags");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected autoscaling:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a grant scoped to particular resource ARNs does not cover an action IAM judges
    /// over EVERY resource, which is the reason this gate decides those actions alone.
    #[test]
    fn a_resource_scoped_grant_does_not_cover_an_account_wide_action() {
        let scoped = r#"{"Statement":[{"Effect":"Allow","Action":"autoscaling:DescribeTags",
             "Resource":"arn:aws:autoscaling:us-west-2:000000000000:autoScalingGroup:x:autoScalingGroupName/web"}]}"#;
        let reached = decision(scoped, "autoscaling:DescribeTags");
        assert_eq!(
            reached,
            Decision::NotStated,
            "case \"a resource-scoped grant\": expected no grant of an account-wide action, \
             got {reached:?}"
        );
    }

    /// RULE: the refusal names the principal, the action and which of IAM's rules withheld
    /// the grant — every part taken from the request and the stored policies — and names no
    /// resource.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_rule() {
        let principal = "arn:aws:sts::000000000000:assumed-role/restricted/session-1";
        let action = format!("{ACTION_PREFIX}:DescribeTags");
        // (case, the clause the decision closed with)
        let rows: &[(&str, String)] = &[
            (
                "an explicit deny",
                iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            ),
            (
                "no statement at all",
                iam::no_identity_policy_allows(&action),
            ),
            (
                "a permissions boundary that allows nothing",
                iam::no_permissions_boundary_allows(&action),
            ),
        ];
        for (case, reason) in rows {
            let refusal = Denied {
                principal: principal.to_string(),
                action: action.clone(),
                resource: None,
                reason: reason.clone(),
            };
            let message = refusal.message();
            assert_eq!(
                message,
                format!("User: {principal} is not authorized to perform: {action} {reason}"),
                "case {case:?}: expected this service's wording, got {message:?}"
            );
            assert!(
                !message.contains("on resource"),
                "case {case:?}: an action judged over every resource names none, got \
                 {message:?}"
            );
        }
    }

    /// RULE: an action IAM judges over every resource is decided over [`EVERY_RESOURCE`] and
    /// names no resource; every other action is judged against the ARN of the resource the
    /// request addresses and names it.
    #[test]
    fn a_refusal_names_the_resource_exactly_when_the_decision_was_taken_on_one() {
        let principal = "arn:aws:sts::000000000000:assumed-role/restricted/session-1";
        let action = format!("{ACTION_PREFIX}:CreateAutoScalingGroup");
        let resource = crate::state::group_arn("us-west-2", EVERY_RESOURCE, "web");
        let reason = iam::no_identity_policy_allows(&action);
        let refusal = Denied {
            principal: principal.to_string(),
            action: action.clone(),
            resource: Some(resource.clone()),
            reason: reason.clone(),
        };
        let message = refusal.message();
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource} {reason}"
            ),
            "case \"an action judged against a group\": expected IAM's sentence naming the \
             group's ARN, got {message:?}"
        );
        assert!(
            resource.contains(":autoScalingGroup:*:autoScalingGroupName/web"),
            "case \"the ARN a request names a group by\": the resource id a request does not \
             state is the wildcard, got {resource:?}"
        );
    }

    /// RULE: an action IAM judges over every resource is decided by the pre-dispatch gate;
    /// every other action is decided by the handler that knows which resource it addresses.
    #[test]
    fn the_account_wide_reads_are_the_ones_judged_over_every_resource() {
        // (the operation, whether IAM judges its action over every resource)
        let rows: &[(&str, bool)] = &[
            ("DescribeTags", true),
            ("DescribeAutoScalingGroups", true),
            ("DescribeScalingActivities", true),
            ("DescribeAccountLimits", true),
            ("CreateAutoScalingGroup", false),
            ("UpdateAutoScalingGroup", false),
            ("SetDesiredCapacity", false),
            ("CreateOrUpdateTags", false),
            ("AttachLoadBalancers", false),
        ];
        for (operation, expected) in rows {
            let decided = judged_over_every_resource(operation);
            assert_eq!(
                decided, *expected,
                "case {operation:?}: expected judged_over_every_resource {expected}, got \
                 {decided}"
            );
        }
    }
}
