// SPDX-License-Identifier: CC0-1.0

//! Whether a `Task` state's EXECUTION ROLE lets the state make the integration call
//! its `Resource` names.
//!
//! An execution's `Task` states do not act as whoever started the execution: Step
//! Functions makes each integration call ACTING AS the state machine's `roleArn` —
//! "the Amazon Resource Name (ARN) of the IAM role to use for this state machine"
//! (`CreateStateMachine$roleArn`) — so a call the policies in force on that role do
//! not allow is a task that FAILS rather than a call that happens. That is the
//! difference between a least-privilege deployment that works and one that only
//! appears to, and it is why the call is decided here BEFORE the request goes
//! anywhere: a child workflow the role was denied must not run.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the `roleArn` of the configuration the execution RAN, read at the
//!    moment of each call, so an execution started before an update is judged by the
//!    role it carries;
//! 2. **what the call asks for** — the `(action, resource)` pair the call is subject
//!    to, formed from the call itself ([`super::integration::access_request`]): the
//!    IAM action is the invoked service's action namespace and the operation's own
//!    name, and the resource is the ARN of what that integration's request names —
//!    the ARN itself where the operation takes one, and otherwise the ARN of the
//!    resource of that NAME in the Region the call is made in;
//! 3. **what the role may do** — the identity-based documents the IAM store holds for
//!    it (its inline policies and the default version of each managed policy attached
//!    to it), read through IAM's own
//!    [`principal_policies`](IamState::principal_policies) accessor so this service
//!    sees exactly what `CreateRole`/`PutRolePolicy` wrote, plus the permissions
//!    boundary that caps them;
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against a request, so this service and every
//!    other one handed the same documents decide alike. How the documents in force
//!    combine — an attachment whose document this account does not hold grants
//!    something unreadable, so it withholds a refusal without overturning an explicit
//!    `Deny`, and the boundary caps rather than grants — is read exactly as IAM's own
//!    authorizer reads it, which is the authority on its own store.
//!
//! The refusal is worded here, because only the invoked service can say what its own
//! refusal says: the principal is the SESSION of the role the call is made as — this
//! service holds no credentials of the role, it assumes the role for the resource's
//! work, and `sts:AssumeRole` reports the credentials it gets under an assumed-role
//! session ARN ([`assumed_role_session`]), which is the identity an invoked service
//! blames — and the metadata tail names the service, the status and the error code the
//! answer came back with, and nothing it did not carry.
//!
//! A run-a-job (`.sync`) task's pattern makes a further call of its own, the one that
//! REPORTS the job, and it is the execution's call too: [`job_report_denial`] decides
//! it against the same documents, over the resource the job belongs to.
//!
//! Nothing is refused on evidence this emulator does not have. Each of these shows
//! nothing about the role and lets the call be made: an engine carrying no IAM store,
//! a `roleArn` the store holds no role for (it may belong to another account), a role
//! carrying an attachment whose document this account does not hold — an AWS-managed
//! policy might be the very one that carries the grant — and a call no
//! [`super::integration::TARGET_RESOURCES`] entry names the target resource of, since
//! an access request cannot be formed without the resource it is about. What the
//! documents DO state is decided in IAM's own order: an explicit `Deny` refuses, a
//! grant allows, and policies that state nothing about the call withhold it, because
//! the documents in force on a role are the authority on what it may do.
//!
//! ## Assuming the role at all
//!
//! Acting as the role is itself a call this service makes, BEFORE any of the above:
//! Step Functions assumes the `roleArn` through `sts:AssumeRole` to get credentials
//! for the work, so a role it cannot assume is a `Task` that fails rather than a call
//! made as nobody. That is the deploy-time/run-time split the service exposes
//! deliberately — `CreateStateMachine` does not require the role to exist, and the
//! data plane does — and it is decided from what IAM ANSWERS about the role
//! ([`::service_common::iam_role::role_is_unassumable_through`], the tree's one
//! reading of a caller-supplied execution role): this account holding no such role, or
//! holding one whose trust policy lets NEITHER of this service's two principals
//! ([`SERVICE_PRINCIPAL`] and the Region's own, [`regional_service_principal`]) assume
//! it, are both answers that the assume fails. Everything else — an ARN naming no role
//! of this account, an emulator with no IAM to ask — is evidence of nothing and leaves
//! the execution to run.

use ::std::sync::Arc;

use ::service_common::iam_authorization::{self as iam, Decision};

use super::integration::{
    access_request, Call, Denied, Failed, Handovers, MapRuns, RoleGate, TASK_FAILED_ERROR,
};

/// The GLOBAL service principal Step Functions assumes a state machine's execution
/// role as.
pub const SERVICE_PRINCIPAL: &str = "states.amazonaws.com";

/// The REGIONAL service principal of the same service, which a trust policy may name
/// instead: the service assumes the role as either, so a role trusting the Region's own
/// principal is one it can act as.
///
/// It is also the service's REGIONAL ENDPOINT name, which is the value of the
/// `kms:ViaService` condition key for a use of a key this service makes on the
/// account's behalf ([`super::encryption`]): the two are the same string, so it is
/// spelled in one place.
pub fn regional_service_principal(region: &str) -> String {
    format!("states.{region}.amazonaws.com")
}

/// What a `Task` state fails with when the execution role `role_arn` cannot be
/// assumed, or `None` when nothing of the kind can be shown (see the module docs).
///
/// The failure is the task's own — ASL's [`TASK_FAILED_ERROR`] — because the work the
/// state names is what did not happen, and the cause is the service's own sentence for
/// an execution role it cannot act as, which names both of the principals it tried to
/// assume the role as.
///
/// It is resolved ONCE per walk, where the walk is started, because every `Task` state
/// of that walk acts as the same role: the credentials a real execution assumes are
/// held for the run rather than fetched per state.
pub async fn unassumable_role(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
) -> Option<Failed> {
    if role_arn.is_empty() {
        return None;
    }
    // Either principal getting the role is enough, so the Region's own is asked about
    // only where the global one was refused.
    let regional = regional_service_principal(region);
    for principal in [SERVICE_PRINCIPAL, regional.as_str()] {
        let unassumable = ::service_common::iam_role::role_is_unassumable_through(
            callback,
            region,
            role_arn,
            principal,
        )
        .await;
        if !unassumable {
            return None;
        }
    }
    Some(Failed {
        error: TASK_FAILED_ERROR.to_string(),
        cause: unassumable_role_cause(&regional, role_arn),
    })
}

/// The ARN of a SESSION of the role `role_arn`, named `session` — the spelling
/// `sts:AssumeRole` reports a session by, and therefore the identity a refusal about a
/// call this service makes as that role names.
///
/// Step Functions holds no credentials of the role: it assumes the role for the
/// resource's work, and the session it gets is named after the resource that work is
/// for. The ONE rendering of that identity in this crate, so a refusal about an
/// integration call and one about a use of a key ([`super::encryption`]) name the
/// acting principal alike. `None` for an ARN that is not a role of an account, or an
/// empty session name, neither of which names a session.
#[must_use]
pub fn assumed_role_session(role_arn: &str, session: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(role_arn)?;
    let name = parsed.resource.strip_prefix("role/")?.rsplit('/').next()?;
    (!name.is_empty() && !session.is_empty()).then(|| {
        format!(
            "arn:{}:sts::{}:assumed-role/{name}/{session}",
            parsed.partition, parsed.account
        )
    })
}

/// The principal a refusal about a call made as `role_arn` names: the session of that
/// role this service assumed for the work ([`assumed_role_session`]), and the role's own
/// ARN where that names no session — a value that is not a role ARN of an account, or a
/// call with no resource to name a session after.
fn acting_principal(role_arn: &str, session_name: &str) -> String {
    assumed_role_session(role_arn, session_name).unwrap_or_else(|| role_arn.to_string())
}

/// The sentence the service refuses an execution role it cannot assume with.
///
/// It names the two principals the assume was attempted as — the global one and
/// `regional`, the Region's own — and the role itself, so an operator reading the
/// failure knows which role reference to act on.
fn unassumable_role_cause(regional: &str, role_arn: &str) -> String {
    format!(
        "Neither the global service principal {SERVICE_PRINCIPAL}, nor the regional one \
         {regional}, is not authorized to assume the provided role {role_arn}."
    )
}

/// The IAM store a state machine's `roleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so a task reads the very records IAM's handlers
/// wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The error code an AWS service refuses an unauthorized call with, which the cause
/// of a failed task names as the code the call came back under, for an operation that
/// models no access-denied error of its OWN.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status that refusal carries.
const ACCESS_DENIED_STATUS: u16 = 400;

/// The error an operation that MODELS the refusal of an unauthorized call answers it
/// with ([`Denied`]), or the unmodelled refusal above for an operation that models
/// none: the code the answer names the class by, and the status it carries.
fn denied_answer(denied: Option<Denied>) -> (&'static str, u16) {
    denied.map_or((ACCESS_DENIED_CODE, ACCESS_DENIED_STATUS), |denied| {
        (denied.code, denied.status)
    })
}

/// What decides whether `role_arn` may make the calls of one walk, or `None` when
/// there is nothing to decide them against — an engine with no IAM store, or an
/// execution carrying no role at all.
///
/// `region` is the Region the walk's calls are made in — the execution's own — which
/// is where a resource a request names by NAME rather than by ARN lives, so it is
/// what the ARN the access request is over is formed in
/// ([`super::integration::TargetNaming`]).
///
/// `session_name` is the resource the walk's work is for — the state machine's own name
/// — which is what the session this service assumes the role under is named after, and
/// therefore the principal a refusal of one of its calls blames
/// ([`assumed_role_session`]).
///
/// `unassumable` is the refusal [`unassumable_role`] resolved for this walk's role: a
/// role this service cannot assume fails EVERY call of the walk, whatever the call
/// asks for, because the credentials the work would be done with are the ones that
/// assume would have minted. It is therefore answered before the policies in force on
/// the role are read at all.
///
/// Apart from that, the gate is consulted once per call the walk asks for, so a
/// statement withdrawn between two states of one execution stops granting from the
/// next call: nothing is read in advance and nothing is cached.
pub fn role_gate<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
    session_name: &str,
    unassumable: Option<Failed>,
) -> Option<RoleGate> {
    if role_arn.is_empty() {
        return None;
    }
    if unassumable.is_none() {
        iam_state(&**engine)?;
    }
    let engine = Arc::clone(engine);
    let region = region.to_string();
    let role_arn = role_arn.to_string();
    let session_name = session_name.to_string();
    Some(Arc::new(move |call: &Call, request: &::serde_json::Value| {
        if let Some(refused) = &unassumable {
            return Some(refused.clone());
        }
        // Only a call whose target this module can name is decided here; anything else
        // is left to the invoked operation's own answer.
        let targets = call.targets?;
        let iam = iam_state(&*engine)?;
        let (action, resource) = access_request(call, request, &region)?;
        let (code, status) = denied_answer(targets.denied);
        if let Some(refused) = refusal(
            &iam.lock(),
            Acts {
                role_arn: &role_arn,
                session_name: &session_name,
            },
            &action,
            &resource,
            call.api.sdk_client,
            code,
            status,
        ) {
            // The refusal reaches the state as the error the invoked OPERATION models for
            // an unauthorized call, where it models one; an operation that models none
            // leaves the invoked service's BASE SDK exception, which is what its client
            // deserializes an unmodelled code into.
            return Some(Failed {
                error: targets.denied.map_or_else(
                    || call.api.base_error(),
                    |denied| call.api.task_error(denied.exception),
                ),
                cause: refused,
            });
        }
        // A target of THIS service that is on a customer managed KMS key seals the data
        // this call hands it under that key, and the call is made as this execution's
        // own role — so a role the TARGET's key does not permit cannot have that data
        // sealed, which fails the task rather than starting work whose data cannot be
        // recorded. Decided after the role's own documents, because a call its policies
        // do not allow never reaches the key at all. The work the state named is what
        // did not happen, so the failure is the task's own — ASL's [`TASK_FAILED_ERROR`]
        // — carrying the refusal the key answered with, exactly as every other refusal
        // of a call this state would have made is carried.
        let key_refusal =
            super::encryption::target_key_denied(&*engine, &region, &resource, &role_arn)?;
        let cause = match key_refusal {
            super::encryption::KeyRefusal::InvalidState { message, .. }
            | super::encryption::KeyRefusal::AccessDenied(message) => message,
        };
        Some(Failed {
            error: TASK_FAILED_ERROR.to_string(),
            cause,
        })
    }))
}

/// The IAM store in the SAME shared engine, which is where the account's roles live.
/// `None` means the engine carries no such store — a deployment with no IAM to ask,
/// which is evidence of nothing.
fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// What one walk ACTS AS while it performs its states' work: the execution role its
/// integration calls are made as, and the EventBridge connections its HTTP Tasks
/// authenticate their requests through.
///
/// Both are decided from another service's own store, reached through the shared engine,
/// and neither is anything the walk itself can read — which is why they are handed to it
/// as closures ([`RoleGate`], [`super::http_task::Connections`]) rather than as records.
#[derive(Clone, Default)]
pub struct Acting {
    /// What decides whether the execution role may make a call ([`role_gate`]).
    pub role: Option<RoleGate>,
    /// What resolves the connection an HTTP Task names
    /// ([`super::http_task::connections`]).
    pub connections: Option<super::http_task::Connections>,
    /// Where the handover of a callback (`.waitForTaskToken`) token is recorded and its
    /// answer read back ([`super::callbacks::handovers`]).
    pub handovers: Option<Handovers>,
    /// What answers which Map Run a Distributed Map state of the walk is ALREADY
    /// running ([`super::map_runs::resumed`]), for the call that CONTINUES an attempt
    /// rather than beginning one. `None` for every other call, whose Map states each
    /// start a Map Run of their own.
    pub map_runs: Option<MapRuns>,
}

/// Debug without disclosing anything either closure reads: a connection holds
/// credentials, and a role's policies are another service's records.
impl ::std::fmt::Debug for Acting {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("Acting")
            .field("role", &self.role.is_some())
            .field("connections", &self.connections.is_some())
            .field("handovers", &self.handovers.is_some())
            .field("map_runs", &self.map_runs.is_some())
            .finish()
    }
}

/// What a walk of this execution acts as: see [`Acting`], [`role_gate`] and
/// [`super::http_task::connections`], each of which answers nothing for an engine
/// carrying no store to read.
pub fn acting<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
    session_name: &str,
    unassumable: Option<Failed>,
) -> Acting {
    Acting {
        role: role_gate(engine, region, role_arn, session_name, unassumable),
        connections: super::http_task::connections(engine),
        handovers: super::callbacks::handovers(engine),
        // A walk that BEGINS an attempt runs every item of every Map state it enters,
        // each under a Map Run of its own: only the call that continues an attempt
        // carries the Map Runs it is continuing ([`Acting::map_runs`]).
        map_runs: None,
    }
}

/// Why the role `role_arn` may not perform `action` on `resource`, as the service
/// named `service` words that refusal, or `None` when nothing of the kind can be shown
/// (see the module docs for the cases that show nothing).
///
/// The ONE evaluation of a role's identity-based documents in this crate, so a `Task`
/// state's integration call and every other decision taken about what a state
/// machine's execution role may do read the same documents in the same order. Call it
/// with no store of this service locked: it takes the IAM store's own.
pub fn role_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    session_name: &str,
    action: &str,
    resource: &str,
    service: &str,
) -> Option<String> {
    if role_arn.is_empty() {
        return None;
    }
    let iam = iam_state(engine)?;
    let denial = refusal(
        &iam.lock(),
        Acts {
            role_arn,
            session_name,
        },
        action,
        resource,
        service,
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
    );
    denial
}

/// What the task a run-a-job (`.sync`) pattern is parked at fails with when the
/// execution role `role_arn` may not make `call` — the call that REPORTS its job — on
/// `resource`, the ARN of the resource that job belongs to; `None` when nothing of the
/// kind can be shown (see the module docs).
///
/// The pattern's own calls are the execution's too: Step Functions polls the job acting
/// as the same role the starting call was made as, so a role whose policies do not
/// allow the reporting call makes that poll not happen. The task was already submitted
/// and the job is really running, so what the failure says is not a status — nothing
/// reported one — but that the task's own work failed: ASL's
/// [`TASK_FAILED_ERROR`], carrying the refusal the invoked service answers an
/// unauthorized call with, exactly as the starting call's refusal carries it.
///
/// Call it with no store of this service locked: it takes the IAM store's own.
pub fn job_report_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    session_name: &str,
    call: &Call,
    resource: &str,
) -> Option<Failed> {
    if role_arn.is_empty() {
        return None;
    }
    let iam = iam_state(engine)?;
    let cause = refusal(
        &iam.lock(),
        Acts {
            role_arn,
            session_name,
        },
        &call.action(),
        resource,
        call.api.sdk_client,
        // The reporting call is an operation of the pattern rather than the `Task`
        // state's own work, and no [`super::integration::TargetResource`] entry names
        // a modelled access-denied error for one, so its refusal carries the code and
        // status an unmodelled one does ([`denied_answer`]).
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
    )?;
    Some(Failed {
        error: TASK_FAILED_ERROR.to_string(),
        cause,
    })
}

/// Whether this account HOLDS a role under `role_arn`, which is what makes a decision
/// about that role's permissions possible at all.
///
/// A role no IAM record answers for — and an engine carrying no IAM store — is evidence
/// of nothing, so neither can be the basis of a refusal: the deploy-time/run-time split
/// this service exposes lets a state machine carry a `roleArn` the account does not
/// hold, and nothing is refused on documents that cannot be read (see the module docs).
#[must_use]
pub fn iam_holds_role<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> bool {
    let Some(iam) = iam_state(engine) else {
        return false;
    };
    let iam = iam.lock();
    iam.roles.values().any(|role| role.arn == role_arn)
}

/// Whether the role `role_arn` may not perform EVERY one of `actions` on `resource`.
///
/// The same reading as [`role_denial`], for a decision whose own refusal is not an
/// invoked service's message but a sentence of THIS service's: a capability a state
/// machine's role needs is often several actions at once, and the role has it only
/// when its documents allow all of them. `false` wherever nothing can be shown — an
/// engine with no IAM store, a role this account does not hold, a role carrying a
/// grant this emulator cannot read — exactly as that function reports it.
pub fn role_is_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    actions: &[&str],
    resource: &str,
) -> bool {
    if role_arn.is_empty() {
        return false;
    }
    let Some(iam) = iam_state(engine) else {
        return false;
    };
    let iam = iam.lock();
    actions
        .iter()
        .any(|action| withheld(&iam, role_arn, action, resource).is_some())
}

/// WHO a call is made as: the execution role, and the name of the session this service
/// assumes it under — the resource the work is for ([`assumed_role_session`]).
///
/// The two travel together because the DECISION is the role's and the PRINCIPAL a refusal
/// blames is the session's, so every reading of one needs the other.
#[derive(Debug, Clone, Copy)]
struct Acts<'a> {
    /// The state machine's `roleArn`, whose documents decide the call.
    role_arn: &'a str,
    /// The name of the session the role is assumed under.
    session_name: &'a str,
}

/// The sentence the invoked service refuses the call with when the identity `acts`
/// may not perform `action` on `resource`, or `None` when nothing of the kind can be
/// shown (see the module docs for the cases that show nothing).
///
/// It is the refusal AS THE SDK REPORTS IT to the state: the service's own
/// not-authorized sentence, followed by the metadata that answer carried — rendered by
/// the ONE renderer of an SDK exception's message
/// ([`super::integration::sdk_exception_message`]), so a refusal this gate answers for
/// the service reads exactly as one the service answered itself. That metadata includes
/// the id of the refused request: an AWS service answers every call it refuses under a
/// request id of its own, so the answer this gate stands in for carries one too, minted
/// per refusal through the tree's one minter of a response's own id — a correlation
/// value, so it is no part of the sentence and two refusals never share it.
///
/// `code` and `status` are what the invoked operation's own answer to an unauthorized
/// call carries ([`denied_answer`]), because that is the answer this gate is standing
/// in for.
fn refusal(
    iam: &IamState,
    acts: Acts<'_>,
    action: &str,
    resource: &str,
    service: &str,
    code: &str,
    status: u16,
) -> Option<String> {
    let sentence = not_authorized(iam, acts, action, resource)?;
    Some(super::integration::sdk_exception_message(
        service,
        &sentence,
        code,
        status,
        Some(&::service_common::query_error_envelope::new_request_id()),
    ))
}

/// IAM's own not-authorized sentence for the identity `acts` performing `action` on
/// `resource`, or `None` when nothing of the kind can be shown (see the module docs for
/// the cases that show nothing).
///
/// The principal it blames is the SESSION, which is the identity the call was really made
/// as ([`acting_principal`]); the DECISION is the role's, and nothing it reads is
/// re-spelled — the statements that grant to an assumed-role session name the role, and
/// `aws:PrincipalArn` of such a request is the role's ARN.
fn not_authorized(
    iam: &IamState,
    acts: Acts<'_>,
    action: &str,
    resource: &str,
) -> Option<String> {
    let because = withheld(iam, acts.role_arn, action, resource)?;
    let principal = acting_principal(acts.role_arn, acts.session_name);
    Some(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         {because}"
    ))
}

/// WHY the documents in force on `role_arn` withhold `action` on `resource` — the clause
/// IAM's own sentence ends with — or `None` where they allow it, and wherever nothing of
/// the kind can be shown (see the module docs for the cases that show nothing).
///
/// The ONE decision about a state machine's role in this crate: whatever the refusal is
/// then reported AS — an invoked service's SDK exception ([`refusal`]), or a sentence
/// this service words itself — the documents are read here, in this order.
fn withheld(
    iam: &IamState,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> Option<String> {
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The documents in force on the role. The collection is bounded by IAM's own
    // quotas on how many policies one role carries, so this walk is bounded by the
    // store rather than by a number of its own.
    let policies = iam.principal_policies(role_arn)?;
    let request = iam::Request::new(action, resource, &[]);
    let stated = iam::identity_decision(&policies.documents, &request);
    // An attachment whose document this account does not hold is a grant this
    // emulator cannot read, so it cannot be the basis of a refusal — but it cannot
    // overturn one either: an explicit `Deny` in a document that IS held wins over
    // every grant, including one whose content is unknown here.
    let granted = iam::is_allowed(stated, Decision::NotStated)
        || (stated != Decision::Denied && !policies.uncarried.is_empty());
    // "A permissions boundary sets the maximum permissions that an identity-based
    // policy can grant to an entity": the boundary CAPS what those policies grant and
    // grants nothing itself, so a call it does not allow is refused even where they
    // allow it. A role carrying none — or one whose document is not held, a ceiling
    // this emulator cannot read — is capped by nothing. A call an identity-based
    // policy explicitly DENIES is already refused on that deny, so no ceiling is read
    // for it.
    let capped = if stated == Decision::Denied {
        Decision::Allowed
    } else {
        iam.permissions_boundary_document(role_arn)
            .map_or(Decision::Allowed, |document| {
                iam::identity_decision(::std::slice::from_ref(&document), &request)
            })
    };
    if granted && capped == Decision::Allowed {
        return None;
    }
    // Why the call was refused: an explicit `Deny` that is about it — stated in an
    // identity-based policy or in the boundary, either of which wins over every
    // `Allow` — or the absence of an `Allow`, which names the DOCUMENT that withheld
    // it: the boundary where the identity-based policies did allow the call, and the
    // identity-based policies where nothing allowed it at all.
    let because = if stated == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if capped == Decision::Denied {
        "with an explicit deny in a permissions boundary".to_string()
    } else if granted {
        format!("because no permissions boundary allows the {action} action")
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    Some(because)
}
