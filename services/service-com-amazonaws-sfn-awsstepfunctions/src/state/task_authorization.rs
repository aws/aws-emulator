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
//!    to, formed from the call itself ([`super::integration::access_requests`]): the
//!    IAM action is the invoked service's action namespace and the operation's own
//!    name, and the resource is the ARN of what that integration's request names —
//!    the ARN itself where the operation takes one, and otherwise the ARN of the
//!    resource of that NAME in the Region the call is made in. A request carrying one
//!    unit of work per resource — a publication naming a bus per entry — asks for one
//!    pair per resource, and the first one the documents withhold is a call that is not
//!    made;
//! 3. **what the role may do** — the identity-based documents the IAM store holds for
//!    it (its inline policies and the default version of each managed policy attached
//!    to it), read through IAM's own
//!    [`principal_policies`](IamState::principal_policies) accessor so this service
//!    sees exactly what `CreateRole`/`PutRolePolicy` wrote, plus the permissions
//!    boundary that caps them;
//! 4. **what the TARGET allows** — the resource-based policy in force on the resource
//!    the call acts on, read out of the owning service's own store
//!    ([`super::target_resource_policy`]), because a call on a resource of the same
//!    account is allowed when EITHER kind of document allows it: "if an identity-based
//!    policy or a resource-based policy grants the request, then AWS allows the
//!    request". A role carrying no identity-based grant at all may therefore make a
//!    call the target's own policy grants it — which is how a function whose
//!    `lambda:AddPermission` statement names the role is invoked — and an explicit
//!    `Deny` in that policy refuses the call whatever a grant says;
//! 5. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against a request, so this service and every
//!    other one handed the same documents decide alike. How the documents in force
//!    combine — an attachment whose document this account does not hold grants
//!    something unreadable, so it withholds a refusal without overturning an explicit
//!    `Deny`, and the boundary caps rather than grants — is read exactly as IAM's own
//!    authorizer reads it, which is the authority on its own store. A grant in the
//!    TARGET's policy stands on its own: "a permissions boundary does not limit the
//!    permissions granted to the entity by a resource-based policy that names it as a
//!    principal".
//!
//! The refusal is worded here, because only the invoked service can say what its own
//! refusal says: the principal is the SESSION of the role the call is made as — this
//! service holds no credentials of the role, it assumes the role for the resource's
//! work, and `sts:AssumeRole` reports the credentials it gets under an assumed-role
//! session ARN ([`assumed_role_session`]) whose name is the one the ASSUME gave it
//! ([`new_session_name`]), which is the identity an invoked service
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
//! grant in either kind of document allows, and policies that state nothing about the
//! call withhold it, because the documents in force are the authority on what the role
//! may do — and so is the absence of any: a role of this account carrying no
//! identity-based document, over a target whose own policy grants it nothing, grants
//! nothing, which is the implicit deny every identity starts from.
//!
//! That last reading is the INVOKED SERVICE's: a `Task` state's call is a call to
//! another service, which decides the same request itself when the call reaches its own
//! front door, so the gate standing in for that answer reads the documents the way that
//! service does. ONE decision of this module is not such a call — the child workflow
//! executions a Distributed Map state starts, which Step Functions makes itself, on the
//! state machine the Map Run belongs to — and it rests on [`Evidence::HeldDocuments`]:
//! no other authority answers it and it fails the whole run before its first item, so a
//! role this account holds no identity-based document for shows nothing about it, as an
//! unreadable attachment does. RECORDED: a Map Run whose execution role carries no such
//! document in this emulator runs its children and reports SUCCEEDED.
//!
//! ## Assuming the role at all
//!
//! Acting as the role is itself a call this service makes, BEFORE any of the above:
//! Step Functions assumes the `roleArn` through `sts:AssumeRole` to get credentials
//! for the work, so a role it cannot assume is a `Task` that fails rather than a call
//! made as nobody. That is the deploy-time/run-time split the service exposes
//! deliberately — `CreateStateMachine` does not require the role to exist, and the
//! data plane does — and it is decided from what IAM ANSWERS about the role
//! ([`::service_common::iam_role::role_assume_is_unauthorized_through`], the tree's one
//! reading of a caller-supplied execution role): this account holding no such role, or
//! holding one whose trust policy lets NEITHER of this service's two principals
//! ([`SERVICE_PRINCIPAL`] and the Region's own, [`regional_service_principal`]) assume
//! it, are both answers that the assume fails. Everything else — an ARN naming no role
//! of this account, an emulator with no IAM to ask — is evidence of nothing and leaves
//! the execution to run.
//!
//! The assume is one PARTICULAR assume, so a trust statement's `Condition` is part of
//! the grant it has to be covered by: this service presents the resource the work is
//! for under the two confused-deputy keys a service trust policy is written against
//! ([`assume_context`]), and a role whose grant is narrowed to another state machine,
//! or to another account, is one this service may not take on for THIS execution —
//! which is the whole point of hardening a service role that way. Nothing is guessed
//! for a key the assume does not carry: an undecidable clause is no grant, the
//! fail-closed reading the shared evaluator takes of every one of them.
//!
//! A `Task` state that names a role of its OWN through `Credentials` is a SECOND assume,
//! and the identity it is made as is the execution role rather than this service: the role
//! that state named is taken on only where ITS trust policy admits that identity
//! ([`assume_denied`]), whatever the execution role's own policies grant, because a trust
//! policy is the one document that can grant an assume of the role it is attached to.

use ::std::sync::Arc;

use ::service_common::iam_authorization::{self as iam, Decision};

use super::integration::{
    access_requests, Activities, Addressing, Api, Call, Denied, ExpressMachines, Failed, Handovers,
    MapRuns, RoleGate, SdkClient, TASK_FAILED_ERROR,
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

/// `aws:SourceAccount` — the global condition key naming the ACCOUNT whose resource a
/// service is making a call on behalf of, and one of the two keys AWS publishes a
/// service role's trust policy as being scoped by.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// `aws:SourceArn` — the other one: the ARN of the RESOURCE the call is made on behalf
/// of, which for an assume of an execution role is the state machine whose execution
/// the credentials are taken on for.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition keys the assume of a state machine's execution role carries: the state
/// machine the credentials are taken on FOR, and the account that state machine belongs
/// to, read out of its own ARN.
///
/// These are the two keys a trust policy that guards against the confused-deputy problem
/// is written against, and the values are fixed by the state machine rather than by
/// anything a caller states, so a statement narrowed to one state machine or one account
/// is DECIDED rather than left undecidable. An ARN this cannot read carries neither key,
/// and a clause over a key the assume does not carry is no grant.
#[must_use]
pub fn assume_context(state_machine_arn: &str) -> Vec<(String, String)> {
    let Some(parsed) = ::service_common::arn::parse(state_machine_arn) else {
        return Vec::new();
    };
    let mut context = vec![(
        SOURCE_ARN_CONDITION_KEY.to_string(),
        state_machine_arn.to_string(),
    )];
    if !parsed.account.is_empty() {
        context.push((
            SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
            parsed.account.to_string(),
        ));
    }
    context
}

/// The two spellings of this service's principal a trust policy can be written for, in
/// the order an assume asks about them: the GLOBAL one ([`SERVICE_PRINCIPAL`]) and the
/// Region's own ([`regional_service_principal`]).
///
/// Either of them getting the role is enough — each names this same service — so a caller
/// asks about them in turn and stops at the first that is granted. The pair is spelled
/// here once, so every reading of a role this service has to act as asks about the same
/// two principals.
#[must_use]
pub fn assume_principals(region: &str) -> [String; 2] {
    [
        SERVICE_PRINCIPAL.to_string(),
        regional_service_principal(region),
    ]
}

/// What a `Task` state fails with when the execution role `role_arn` cannot be
/// assumed, or `None` when nothing of the kind can be shown (see the module docs).
///
/// The failure is the task's own — ASL's [`TASK_FAILED_ERROR`] — because the work the
/// state names is what did not happen, and the cause is the service's fixed sentence for
/// an execution role it cannot act as ([`unassumable_role_cause`]), which names the
/// service principal a trust policy is written for and the role itself.
///
/// It is resolved ONCE per walk, where the walk is started, because every `Task` state
/// of that walk acts as the same role: the credentials a real execution assumes are
/// held for the run rather than fetched per state.
///
/// `state_machine_arn` is the state machine the credentials are taken on FOR, which is
/// what the assume presents under the confused-deputy keys ([`assume_context`]): the
/// same role is assumable for one state machine and not for another whenever its trust
/// policy says so.
pub async fn unassumable_role(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    state_machine_arn: &str,
) -> Option<Failed> {
    if role_arn.is_empty() {
        return None;
    }
    // Either principal getting the role is enough, so the Region's own is asked about
    // only where the global one was refused ([`assume_principals`]).
    let context = assume_context(state_machine_arn);
    for principal in assume_principals(region) {
        let unassumable = ::service_common::iam_role::role_assume_is_unauthorized_through(
            callback,
            region,
            role_arn,
            &principal,
            &context,
        )
        .await;
        if !unassumable {
            return None;
        }
    }
    Some(Failed {
        error: TASK_FAILED_ERROR.to_string(),
        cause: unassumable_role_cause(role_arn),
    })
}

/// The ARN of a SESSION of the role `role_arn`, named `session` — the spelling
/// `sts:AssumeRole` reports a session by, and therefore the identity a refusal about a
/// call this service makes as that role names.
///
/// Step Functions holds no credentials of the role: it assumes the role for the work, and
/// what comes back is a session of it, under the name the ASSUME gave it
/// ([`new_session_name`]). The ONE rendering of that identity in this crate, so a refusal
/// about an integration call and one about a use of a key ([`super::encryption`]) name the
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

/// A fresh name for the session this service assumes a state machine's execution role
/// under.
///
/// The name of such a session is the SERVICE's to choose, not the caller's: a caller that
/// assumes a role itself states the `RoleSessionName`, and nothing a state machine's
/// definition or a `StartExecution` request carries states one for the sessions this
/// service takes — so the name names no resource of the account and is not derivable from
/// the request. It is an opaque token, minted per assume: the 32 hexadecimal digits of a
/// fresh version-4 UUID from the maintained crate, which is within the `length 2..64` over
/// the pattern `^[\w+=,.@-]*$` a `RoleSessionName` carries
/// (`smithy_query(query="shape",shape="roleSessionNameType",model_path="services/
/// service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/
/// com-amazonaws-sts-awssecuritytokenservicev20110615.json")`), so the identity a refusal
/// blames is one `sts:AssumeRole` could really have minted.
///
/// Minted once per ASSUME: the calls of one walk share the session that walk was given
/// ([`role_gate`]), so every refusal of its own work blames the same identity, and two
/// walks blame two. A call made outside a walk — the report a run-a-job pattern polls its
/// job with ([`job_report_denial`]), a use of a resource's key ([`super::encryption`]) —
/// is an assume of its own and carries its own session, exactly as a service that takes
/// credentials per piece of work does.
#[must_use]
pub fn new_session_name() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// The principal a refusal about a call made as `role_arn` names: the session of that
/// role this service assumed for the work ([`assumed_role_session`]), and the role's own
/// ARN where that names no session — a value that is not a role ARN of an account.
fn acting_principal(role_arn: &str, session_name: &str) -> String {
    assumed_role_session(role_arn, session_name).unwrap_or_else(|| role_arn.to_string())
}

/// The sentence the service refuses an execution role it cannot assume with.
///
/// Every word of it is fixed: the service blames its own service principal
/// ([`SERVICE_PRINCIPAL`]) — the identity a trust policy is written for — and names the
/// role reference an operator has to act on in a parenthesised suffix, so a catcher or
/// a log alarm that matches the sentence, or parses the role out of the parenthesis,
/// reads the same text here as from the service itself. The only variable part is
/// `role_arn`, the `roleArn` configured on the state machine the execution ran; the
/// Region's own principal ([`regional_service_principal`]) is this emulator's way of
/// deciding the assume and is named by no refusal.
fn unassumable_role_cause(role_arn: &str) -> String {
    format!(
        "The principal {SERVICE_PRINCIPAL} is not authorized to assume the provided role. \
         (role: {role_arn})"
    )
}

/// The EXCEPTION a failure of the RUN itself names, which is what the service reports when
/// it could not take the execution role on at all.
///
/// Such a failure is not a sentence the service composes: it is the exception the assume
/// raised, and a cause carrying an exception names its CLASS and then the message it
/// carries, which is the sentence [`unassumable_role_cause`] words. Fixed text, so a
/// catcher or a log alarm that matches on the class reads here what it reads from the
/// service.
const ASSUME_ROLE_FAILED_EXCEPTION: &str =
    "com.amazonaws.sfn.invoker.sts.AssumeRoleFailedCacheableException";

/// What an EXECUTION whose own work cannot be done at all without the role `role_arn`
/// ends as, when this service cannot act as that role ([`unassumable_role`]).
///
/// The error is the one the LANGUAGE reserves for a failure of the execution itself
/// ([`super::asl::RUNTIME_ERROR`]) rather than the one a state's work reports: nothing of
/// the definition ran, so no state failed — the run had no identity to be performed as.
/// The cause is the exception that failure carries ([`ASSUME_ROLE_FAILED_EXCEPTION`]) over
/// the one sentence this condition is worded by ([`unassumable_role_cause`]), so the two
/// places the condition surfaces cannot word it differently.
fn unassumable_run_failure(role_arn: &str) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{ASSUME_ROLE_FAILED_EXCEPTION}: {}",
            unassumable_role_cause(role_arn)
        ),
    }
}

/// What an attempt at an execution of the state machine `state_machine_arn` identifies
/// ENDS as, when this service cannot perform that run at all — or `None` when it can.
///
/// Acting as the execution role is part of the RUN rather than of the call that begins an
/// attempt, so a role whose trust IAM answered for is never a refusal of that call: the
/// attempt is recorded and the EXECUTION fails ([`unassumable_run_failure`]). It fails
/// outright, with not one state entered, exactly where the run's own data is sealed as
/// that role ([`super::encryption::run_is_sealed_as_the_execution_role`]): the data key
/// every payload of the execution goes under is minted as the role before the first state,
/// so an attempt that cannot be made as it does nothing at all. Where the run asks nothing
/// of the role before its states do, the walk proceeds and each `Task` of it reports the
/// same condition as its own failure ([`role_gate`]).
///
/// `unassumable` is what [`unassumable_role`] answered for that role, and the decision is
/// taken only for a role this account's IAM HOLDS — the evidence every decision about a
/// role here requires: a role no record answers for is the deploy-time/run-time split this
/// service exposes rather than a role that was refused.
///
/// Asked by every operation that begins an attempt, so a start and a redrive cannot read
/// the condition differently. Call it with no store of this service locked.
#[must_use]
pub fn unrunnable_attempt<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    state_machine_arn: &str,
    role_arn: &str,
    unassumable: Option<&Failed>,
) -> Option<Failed> {
    unassumable?;
    if !iam_holds_role(engine, role_arn) {
        return None;
    }
    super::encryption::run_is_sealed_as_the_execution_role(engine, region, state_machine_arn)
        .then(|| unassumable_run_failure(role_arn))
}

/// The IAM store a state machine's `roleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so a task reads the very records IAM's handlers
/// wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The error an operation that MODELS the refusal of an unauthorized call answers it
/// with ([`Denied`]), or the UNMODELLED refusal for an operation that models none: the
/// code the answer names the class by, and the status it carries.
///
/// Both halves of an UNMODELLED refusal are the invoked service's own
/// ([`super::integration::Api::access_denied_code`],
/// [`super::integration::Api::access_denied_status`]): the decision is taken before
/// the operation is reached, so what answers it is that service's front door, and the
/// services differ both in whether they answer an authorization failure as a Forbidden
/// or as a client error and in the code they name it by.
fn denied_answer(api: &Api, denied: Option<Denied>) -> (&'static str, u16) {
    denied.map_or(
        (api.access_denied_code(), api.access_denied_status),
        |denied| (denied.code, denied.status),
    )
}

/// What a decision about what the execution role may do is allowed to rest on.
///
/// The documents this account holds decide every call either way; the two readings
/// differ only about a role the account holds NO identity-based document for, which is a
/// role whose permissions were written outside this emulator exactly as the role itself
/// may have been ([`unassumable_role`]'s deploy-time/run-time split, and
/// [`super::execution_role_session::role_session`], which mints no session for such a
/// role rather than refusing its calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Every document, and the IMPLICIT DENY a role carrying none starts from: the
    /// reading the INVOKED service takes of the same request when a `Task` state's call
    /// reaches its own front door, which is what the gate standing in for that answer
    /// has to read.
    Documents,
    /// Only what this account HOLDS: a role it holds no identity-based document for
    /// shows nothing about the call, as an attachment whose document it does not hold
    /// does. The reading for a decision NO other authority answers — the child workflow
    /// executions Step Functions itself starts for a Map Run.
    HeldDocuments,
}

/// What decides whether the role `role_arn` may make the calls of one walk, or `None`
/// when there is nothing to decide them against — an engine with no IAM store, or an
/// execution carrying no role at all.
///
/// `region` is the Region the walk's calls are made in — the execution's own — which
/// is where a resource a request names by NAME rather than by ARN lives, so it is
/// what the ARN the access request is over is formed in
/// ([`super::integration::TargetNaming`]).
///
/// The session this service assumes the role under is the one the credential of the
/// walk was minted for ([`super::execution_role_session`]), so every refusal of its
/// work blames the identity its calls were really made as; where no credential could be
/// minted (see that module), a fresh name stands in for the assume this emulator could
/// not record.
///
/// `unassumable` is the refusal [`unassumable_role`] resolved for this walk's role: a
/// role this service cannot assume fails EVERY call of the walk, whatever the call
/// asks for, because the credentials the work would be done with are the ones that
/// assume would have minted. It is therefore answered before the policies in force on
/// the role are read at all.
///
/// `evidence` is what the gate's decisions may rest on ([`Evidence`]), which is why one
/// walk is handed TWO gates: the calls it makes to other services read the documents as
/// those services do, and the child workflow starts this service makes itself read what
/// the account holds.
///
/// Apart from that, the gate is consulted once per call the walk asks for, so a
/// statement withdrawn between two states of one execution stops granting from the
/// next call: nothing is read in advance and nothing is cached.
pub fn role_gate<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
    unassumable: Option<Failed>,
    session_name: &str,
    evidence: Evidence,
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
        // WHERE this call is addressed, which is the Region the resource its request
        // names lives in: the one the target's own value carries, the one this emulator
        // holds that resource in for a value that carries none
        // ([`super::target_addressing`]), and the execution's own otherwise. The access
        // request is formed in it, so the ARN it is over is the ARN of the resource the
        // call really acts on — and the call is made in the same Region
        // ([`super::integration::Addressing`]).
        let region = super::target_addressing::call_region(&*engine, call, request, &region)
            .unwrap_or_else(|| region.clone());
        // Every resource the call acts on, each decided in the order the request names
        // them: a call whose request carries one unit of work per resource — a
        // publication naming a bus per entry — is the execution's call on each of them,
        // and the first one the documents in force withhold is a call that is not made,
        // exactly as the invoked service refuses the whole request over the first unit it
        // may not perform.
        let asked = access_requests(call, request, &region);
        let (code, status) = denied_answer(call.api, targets.denied);
        for (action, resource) in &asked {
            // The sentence reports the action as the INVOKED service's own refusal spells
            // it; IAM compares action names without regard to case, so the decision is
            // the same either way ([`super::integration::Api::refused_action`]).
            let action = call.api.refused_action(action);
            // What the TARGET's own policy says about the call, read before IAM's store is
            // locked: a grant in it allows the call even where the role carries no
            // identity-based grant ([`super::target_resource_policy`]).
            let target_policy = super::target_resource_policy::in_force(&*engine, resource);
            if let Some(refused) = refusal(
                &iam.lock(),
                Acts {
                    role_arn: &role_arn,
                    session_name: &session_name,
                },
                &action,
                resource,
                target_policy.as_deref(),
                Answered {
                    api: call.api,
                    client: call.client(),
                    code,
                    status,
                },
                evidence,
            ) {
                // The refusal reaches the state as the error the invoked OPERATION models
                // for an unauthorized call, where it models one; an operation that models
                // none leaves the invoked service's BASE SDK exception, which is what its
                // client deserializes an unmodelled code into.
                return Some(Failed {
                    error: targets.denied.map_or_else(
                        || call.base_error(),
                        |denied| call.task_error(denied.exception),
                    ),
                    cause: refused,
                });
            }
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
        let key_refusal = asked.iter().find_map(|(_, resource)| {
            super::encryption::target_key_denied(&*engine, &region, resource, &role_arn)
        })?;
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
    /// What decides whether the execution role may have the CHILD WORKFLOW EXECUTIONS
    /// of a Distributed Map state started for it: the same gate over the same documents,
    /// on the evidence a decision no invoked service answers may rest on
    /// ([`Evidence::HeldDocuments`]).
    pub child_starts: Option<RoleGate>,
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
    /// The CREDENTIAL the walk's integration calls are signed with: a session of the
    /// execution role ([`super::execution_role_session`]), so each call is made as that
    /// role rather than as this emulator's own principal. `None` where no session could
    /// be minted, which leaves the calls signed with the credential the framework hands
    /// this emulator's handlers.
    pub signing: Option<super::execution_role_session::RoleSession>,
    /// What answers whether this emulator holds the activity a `Task` state's
    /// `Resource` names ([`super::activity_resources::activities`]).
    pub activities: Option<Activities>,
    /// What answers whether a state machine a nested-execution task names is of the
    /// EXPRESS workflow type ([`super::express_machines::express_machines`]), which is
    /// what decides how such a task RUNS its child.
    pub express_machines: Option<ExpressMachines>,
    /// What answers WHERE one of the walk's calls is addressed, for a call whose target is
    /// named by a value that carries no Region of its own
    /// ([`super::target_addressing::addressing`]).
    pub addressing: Option<Addressing>,
    /// What serves a `Task` state that names a role of its OWN through `Credentials`
    /// ([`TaskRoles`]); `None` for a walk with no execution role to assume from.
    pub task_roles: Option<TaskRoles>,
    /// What mints the credential such a call is signed with ([`TaskSigning`]).
    pub task_signing: Option<TaskSigning>,
}

/// Debug without disclosing anything either closure reads: a connection holds
/// credentials, and a role's policies are another service's records.
impl ::std::fmt::Debug for Acting {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("Acting")
            .field("role", &self.role.is_some())
            .field("child_starts", &self.child_starts.is_some())
            .field("connections", &self.connections.is_some())
            .field("handovers", &self.handovers.is_some())
            .field("map_runs", &self.map_runs.is_some())
            .field("activities", &self.activities.is_some())
            .field("express_machines", &self.express_machines.is_some())
            .field("addressing", &self.addressing.is_some())
            .field("signing", &self.signing.is_some())
            .field("task_roles", &self.task_roles.is_some())
            .field("task_signing", &self.task_signing.is_some())
            .finish()
    }
}

/// What a walk of this execution acts as: see [`Acting`], [`role_gate`] and
/// [`super::http_task::connections`], each of which answers nothing for an engine
/// carrying no store to read.
///
/// `store` is where a minted session of the execution role is recorded, so the walk's
/// calls are made AS that role ([`super::execution_role_session::role_session`]); a
/// caller with none gets a session only where one is already held.
pub fn acting<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
    unassumable: Option<Failed>,
    store: Option<&::emulator_server::CredentialStore>,
) -> Acting {
    // The credential comes first: the session it speaks for is the identity the calls
    // are really made as, so it is the one a refusal of one of them blames. A role this
    // service cannot ACT AS vends none: the assume is what would have minted the
    // credentials, so a role whose trust policy admits neither of this service's
    // principals leaves nothing to sign with — and nothing of it recorded where a
    // credential is resolved.
    let signing = match unassumable {
        Some(_) => None,
        None => super::execution_role_session::role_session(
            &**engine,
            store,
            role_arn,
            // The WALL clock, and not the offsets the walk measures a definition's own
            // spans by: what makes a credential unusable is the instant the token
            // service minted it to last until, which it dates in real time.
            ::std::time::SystemTime::now(),
        ),
    };
    let session_name = signing
        .as_ref()
        .map_or_else(new_session_name, |session| session.session_name.clone());
    Acting {
        role: role_gate(
            engine,
            region,
            role_arn,
            unassumable.clone(),
            &session_name,
            Evidence::Documents,
        ),
        // The SAME gate over the same documents, for the one decision no invoked service
        // answers: the child workflow executions a Distributed Map state starts
        // ([`Evidence::HeldDocuments`]).
        child_starts: role_gate(
            engine,
            region,
            role_arn,
            unassumable,
            &session_name,
            Evidence::HeldDocuments,
        ),
        connections: super::http_task::connections(engine),
        handovers: super::callbacks::handovers(engine),
        // A walk that BEGINS an attempt runs every item of every Map state it enters,
        // each under a Map Run of its own: only the call that continues an attempt
        // carries the Map Runs it is continuing ([`Acting::map_runs`]).
        map_runs: None,
        activities: super::activity_resources::activities(engine),
        express_machines: super::express_machines::express_machines(engine),
        addressing: Some(super::target_addressing::addressing(engine, region)),
        signing,
        // And what serves a `Task` state that names a role of its OWN, resolved per call
        // because the role is one the state's own members produce as the walk runs.
        task_roles: task_roles(engine, region, role_arn),
        task_signing: task_signing(engine),
    }
}

/// What a `Task` state's own `Credentials` resolves to for ONE target role: whether the
/// call may be made under that role at all, and what then decides the call.
///
/// "Credentials … Specify a target role that the state machine's execution role can
/// assume": the ASSUME comes first, so a role the execution role may not take on is a task
/// that FAILS rather than a call quietly made as the execution role instead. Where the
/// assume is allowed, the call is made BY the target role, so it is that role's own
/// policies the call is judged against and none of the execution role's.
pub struct TaskRole {
    /// Why the call may not be made under the role at all; `None` where the assume is
    /// allowed, and wherever nothing of the kind can be shown (see the module docs).
    pub refused: Option<Failed>,
    /// What decides whether THAT role may make the call ([`role_gate`]).
    pub gate: Option<RoleGate>,
}

/// What answers [`TaskRole`] for the role a `Task` state's `Credentials` named.
///
/// A closure for the reason [`RoleGate`] is one: the documents live in another service's
/// store, reached through the engine the operation holds, and the walk knows only the ARN
/// its state resolved.
pub type TaskRoles = Arc<dyn Fn(&str) -> TaskRole + Send + Sync>;

/// What mints the CREDENTIAL a call made under a task's own role is signed with: a session
/// of that role ([`super::execution_role_session::role_session`]).
///
/// Asked where the call is served rather than where the walk is set up, because the role is
/// one the walk resolves as it runs; the credential store a minted session is recorded in
/// is the serving call's, so it is handed in per call.
pub type TaskSigning = Arc<
    dyn Fn(
            &str,
            Option<&::emulator_server::CredentialStore>,
        ) -> Option<super::execution_role_session::RoleSession>
        + Send
        + Sync,
>;

/// What serves a `Task` state's `Credentials` for the execution role `role_arn`: see
/// [`TaskRoles`]. `None` for a walk with no role to assume FROM, and for an engine
/// carrying no IAM store to read.
fn task_roles<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
) -> Option<TaskRoles> {
    if role_arn.is_empty() {
        return None;
    }
    iam_state(&**engine)?;
    let engine = Arc::clone(engine);
    let region = region.to_string();
    let execution_role = role_arn.to_string();
    Some(Arc::new(move |target: &str| {
        // One assume per call, under a session of its own, exactly as every other assume
        // this service takes outside a walk ([`new_session_name`]).
        let session_name = new_session_name();
        TaskRole {
            refused: assume_denied(&engine, &execution_role, target, &session_name),
            // The call is made BY the target role, so it is judged as the invoked service
            // judges a call from that role: the same gate over that role's documents.
            gate: role_gate(
                &engine,
                &region,
                target,
                None,
                &session_name,
                Evidence::Documents,
            ),
        }
    }))
}

/// See [`TaskSigning`]. Always answers one: the store a session is recorded in is handed
/// in per call rather than captured here, so what is missing on an engine that holds no
/// sessions store is answered by the mint itself ([`super::execution_role_session::role_session`]).
fn task_signing<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> Option<TaskSigning> {
    let engine = Arc::clone(engine);
    Some(Arc::new(move |role_arn: &str, store| {
        super::execution_role_session::role_session(
            &*engine,
            store,
            role_arn,
            // The WALL clock: what makes a credential unusable is the instant the token
            // service minted it to last until ([`acting`]).
            ::std::time::SystemTime::now(),
        )
    }))
}

/// Why the execution role `execution_role` may NOT assume `target` to make a `Task`
/// state's call under it, or `None` where it may and wherever nothing of the kind can be
/// shown (see the module docs).
///
/// The assume is `sts:AssumeRole` on the target role, made as the execution role's session,
/// and it is decided in IAM's own order for that one request:
///
/// 1. the TARGET's trust policy is what says who may take the role on. It is the only
///    document that can grant an assume of it — "update the role's trust policy to allow
///    the entity to assume it" — so an assume no statement of that document covers is
///    refused however much the assuming role's own policies grant, and the identity it is
///    matched against is every spelling of the caller a `Principal` element can name: the
///    session this service assumed the execution role under, the execution role itself, and
///    the account it belongs to ([`::service_common::iam_trust_policy::grants_any_principal`]);
/// 2. then the documents in force on the execution role, read exactly as every other call
///    of the walk is ([`withheld`]), with that same trust policy as the resource-based
///    document — so a trust statement naming the execution role directly is enough on its
///    own, while one that merely names its ACCOUNT delegates to those documents and needs a
///    grant among them.
///
/// Nothing is refused on evidence this emulator does not have: an account holding no role
/// under `target`, or holding one whose trust document it cannot read, shows nothing about
/// the assume and leaves the call to be made.
///
/// The failure is the task's own ([`TASK_FAILED_ERROR`]) carrying this service's own
/// sentence for it ([`unassumable_task_role_cause`]): Step Functions takes the role on
/// itself, so what a caller reads is how this service words a target role it could not
/// take on.
fn assume_denied<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    execution_role: &str,
    target: &str,
    session_name: &str,
) -> Option<Failed> {
    let iam = iam_state(&**engine)?;
    let acts = Acts {
        role_arn: execution_role,
        session_name,
    };
    let trust = role_trust_policy(&**engine, target);
    let refused = || Failed {
        error: TASK_FAILED_ERROR.to_string(),
        cause: unassumable_task_role_cause(execution_role, target),
    };
    if trust
        .as_deref()
        .is_some_and(|document| !trust_admits_the_caller(document, acts, target))
    {
        return Some(refused());
    }
    let because = withheld(
        &iam.lock(),
        acts,
        ::service_common::iam_trust_policy::ASSUME_ROLE_ACTION,
        target,
        trust.as_deref(),
        // The assume is a call to the token service, which decides the same request
        // itself: the documents are read as that service reads them.
        Evidence::Documents,
    );
    because.map(|_| refused())
}

/// Whether the TRUST document `document` of the role `target` names the identity `acts`
/// makes the assume as, which is what a trust policy has to do for that role to be
/// assumable by it at all.
///
/// Every spelling of that identity is asked about ([`acting_principals`], and the ACCOUNT
/// it belongs to, which a trust policy may name instead of the role), because each of them
/// is a name a `Principal` element gives the same caller. `true` for a document that is not
/// a policy this can be read out of: an unreadable grant is evidence of nothing, so it
/// cannot be the basis of a refusal (see the module docs).
fn trust_admits_the_caller(document: &str, acts: Acts<'_>, target: &str) -> bool {
    let mut principals = acting_principals(acts);
    principals.extend(iam::account_principals(acts.role_arn));
    // The assume carries no condition key of its own: it is one role taking another on,
    // which presents none of the keys a service's own assume does ([`assume_context`]), so
    // a statement whose `Condition` cannot be decided is no grant — the fail-closed reading
    // the shared evaluator takes.
    ::service_common::iam_trust_policy::grants_any_principal(document, &principals, target, &[])
        != Some(false)
}

/// The sentence this service fails a `Task` state with when the execution role may not take
/// on the role the state's `Credentials` named.
///
/// Every word of it is fixed: Step Functions assumes the target role ITSELF, as the
/// execution role, so the refusal a caller reads is this service's own report of a target
/// role it could not take on rather than the token service's exception — and the only
/// variable parts are the two roles it names, the `roleArn` of the state machine whose
/// execution ran and the role that state resolved its `Credentials` to.
fn unassumable_task_role_cause(execution_role: &str, target: &str) -> String {
    format!(
        "The role {execution_role} is not authorized to assume the task state's role, {target}."
    )
}

/// The TRUST policy in force on the role `role_arn`, as this account's IAM holds it, or
/// `None` where it holds no role under that ARN or the role carries no such document.
///
/// A role's trust policy is its resource-based policy — the document whose statements name
/// the principals that may assume it — so it is read here for the one decision that needs
/// it and handed to the SAME evaluator every other resource-based policy goes through
/// ([`withheld`]).
fn role_trust_policy<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> Option<String> {
    let iam = iam_state(engine)?;
    let held = iam.lock();
    held.roles
        .values()
        .find(|role| role.arn == role_arn)?
        .assume_role_policy_document
        .clone()
}

/// The REFUSAL the call that reports a run-a-job (`.sync`) pattern's job answers with
/// when the execution role `role_arn` may not make `call` on `resource`, the ARN of the
/// resource that job belongs to; `None` when nothing of the kind can be shown (see the
/// module docs).
///
/// The pattern's own calls are the execution's too: Step Functions polls the job acting
/// as the same role the starting call was made as, so a role whose policies do not
/// allow the reporting call makes that poll not happen. What the task parked at the job
/// is then left with is the integration's own
/// ([`super::integration::ReportDenial`], applied by [`super::run_a_job`]): this is the
/// refusal itself, worded as the invoked service words an unauthorized call, under ASL's
/// [`TASK_FAILED_ERROR`] for the integration whose poll surfaces it, exactly as the
/// starting call's refusal is.
///
/// The poll is a call of its own, made under a session of its own
/// ([`new_session_name`]).
///
/// Call it with no store of this service locked: it takes the IAM store's own.
pub fn job_report_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    call: &Call,
    resource: &str,
) -> Option<Failed> {
    if role_arn.is_empty() {
        return None;
    }
    let iam = iam_state(engine)?;
    // The reporting call is an operation of the pattern rather than the `Task` state's
    // own work, and no [`super::integration::TargetResource`] entry names a modelled
    // access-denied error for one, so its refusal carries the code and the status an
    // unmodelled one does ([`denied_answer`]).
    let (code, status) = denied_answer(call.api, None);
    let target_policy = super::target_resource_policy::in_force(engine, resource);
    let cause = refusal(
        &iam.lock(),
        Acts {
            role_arn,
            session_name: &new_session_name(),
        },
        &call.api.refused_action(&call.action()),
        resource,
        target_policy.as_deref(),
        Answered {
            api: call.api,
            client: call.client(),
            code,
            status,
        },
        // The reporting call is a call to the invoked service, which answers the same
        // request itself: the documents are read as that service reads them.
        Evidence::Documents,
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
/// The same reading as [`withheld`], for a decision whose own refusal is not an
/// invoked service's message but a sentence of THIS service's: a capability a state
/// machine's role needs is often several actions at once, and the role has it only
/// when its documents allow all of them — the documents in force on the role and the
/// resource-based policy in force on `resource`, read as that function reads them.
/// `false` wherever nothing can be shown — an engine with no IAM store, a role this
/// account does not hold, a role carrying a grant this emulator cannot read — exactly
/// as that function reports it.
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
    let target_policy = super::target_resource_policy::in_force(engine, resource);
    let iam = iam.lock();
    actions.iter().any(|action| {
        withheld(
            &iam,
            Acts {
                role_arn,
                // No session: the decision is the ROLE's, and what this reading reports
                // is worded by the caller rather than blamed on a session of it.
                session_name: "",
            },
            action,
            resource,
            target_policy.as_deref(),
            // The capability is one the role's documents have to state: this reading
            // answers a call this service makes to another service's API on the role's
            // behalf, so a role carrying no document states nothing that allows it.
            Evidence::Documents,
        )
        .is_some()
    })
}

/// WHO a call is made as: the execution role, and the name of the session this service
/// assumes it under ([`new_session_name`]).
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
/// `answered` is the answer this gate is standing in for: whose answer it is, and what
/// the invoked operation's own answer to an unauthorized call carries
/// ([`denied_answer`]).
fn refusal(
    iam: &IamState,
    acts: Acts<'_>,
    action: &str,
    resource: &str,
    target_policy: Option<&str>,
    answered: Answered<'_>,
    evidence: Evidence,
) -> Option<String> {
    let sentence = not_authorized(iam, acts, action, resource, target_policy, evidence)?;
    Some(super::integration::sdk_exception_message(
        answered.api,
        answered.client,
        &sentence,
        answered.code,
        answered.status,
        Some(&::service_common::query_error_envelope::new_request_id()),
    ))
}

/// The answer this gate stands in for, as the state sees it: WHOSE answer it is — the
/// invoked service, and the CLIENT the call was made with, which decides how the
/// exception is rendered ([`super::integration::sdk_exception_message`]) — and the code
/// and status that answer carries.
#[derive(Debug, Clone, Copy)]
struct Answered<'a> {
    /// The service whose API refused the call.
    api: &'a Api,
    /// The client the integration the `Resource` named is served through, which is the
    /// one that renders the refusal ([`super::integration::Call::client`]).
    client: SdkClient,
    /// The code the answer names the class by.
    code: &'a str,
    /// The status it carries.
    status: u16,
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
    target_policy: Option<&str>,
    evidence: Evidence,
) -> Option<String> {
    let because = withheld(iam, acts, action, resource, target_policy, evidence)?;
    let principal = acting_principal(acts.role_arn, acts.session_name);
    Some(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         {because}"
    ))
}

/// WHY the documents in force withhold `action` on `resource` from the identity `acts` —
/// the clause IAM's own sentence ends with — or `None` where they allow it, and wherever
/// nothing of the kind can be shown (see the module docs for the cases that show nothing).
///
/// `target_policy` is the resource-based policy in force on `resource`, where this
/// emulator holds one ([`super::target_resource_policy`]): a call on a resource of the
/// same account is allowed when EITHER that policy or the role's own identity-based
/// documents allow it, and an explicit `Deny` in either refuses it.
///
/// The ONE decision about a state machine's role in this crate: whatever the refusal is
/// then reported AS — an invoked service's SDK exception ([`refusal`]), or a sentence
/// this service words itself — the documents are read here, in this order.
///
/// `evidence` is what the decision may rest on ([`Evidence`]): every caller but the
/// service's own child-workflow start reads the documents as the INVOKED service does,
/// implicit deny included.
fn withheld(
    iam: &IamState,
    acts: Acts<'_>,
    action: &str,
    resource: &str,
    target_policy: Option<&str>,
    evidence: Evidence,
) -> Option<String> {
    let role_arn = acts.role_arn;
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The documents in force on the role. The collection is bounded by IAM's own
    // quotas on how many policies one role carries, so this walk is bounded by the
    // store rather than by a number of its own.
    let policies = iam.principal_policies(role_arn)?;
    // The spellings of the identity the call is made as travel with the request, because
    // that is what a resource-based statement's `Principal` element is matched against;
    // an identity-based document states no such element, so for those the decision rests
    // on the action and the resource alone ([`acting_principals`]).
    let principals = acting_principals(acts);
    let request = iam::Request::new(action, resource, &principals);
    // Every spelling, the account's included: a statement that DENIES the account denies
    // each of its principals, while one that merely names the account delegates to that
    // account's own identity-based policies rather than granting.
    let mut named = principals.clone();
    named.extend(iam::account_principals(role_arn));
    let denying = iam::Request::new(action, resource, &named);
    // An explicit `Deny` in the TARGET's own policy refuses the call whatever any grant
    // says, which is the order IAM evaluates the two kinds of document in.
    if target_policy
        .is_some_and(|document| iam::resource_decision(document, &denying) == Decision::Denied)
    {
        return Some("with an explicit deny in a resource-based policy".to_string());
    }
    let stated = iam::identity_decision(&policies.documents, &request);
    // Taken before any grant is read, so an explicit `Deny` in a document that CAN be
    // read refuses the call whatever else is in force — including a grant this emulator
    // cannot read, which only lifts the default deny of a call no readable statement
    // allows.
    if stated == Decision::Denied {
        return Some("with an explicit deny in an identity-based policy".to_string());
    }
    // A grant in the target's own policy names this identity directly, and "a permissions
    // boundary does not limit the permissions granted to the entity by a resource-based
    // policy that names it as a principal": such a grant stands on its own, so a role
    // carrying no identity-based document at all makes the call the target allows it.
    if target_policy
        .is_some_and(|document| iam::resource_decision(document, &request) == Decision::Allowed)
    {
        return None;
    }
    // Whether the documents in force grant the call at all. Nothing stated is IAM's
    // implicit deny, so a role carrying NO identity-based document grants nothing: a
    // grant has to come from a document, which is how IAM's own gate over this same
    // store reads the identity it holds — except where the decision rests on what this
    // account HOLDS alone ([`Evidence::HeldDocuments`]), and there a role it holds no
    // document for shows nothing rather than denying. An attachment whose document this
    // account does not hold is the grant this emulator cannot read either way, so it
    // cannot be the basis of a refusal — but neither can overturn one: an explicit `Deny`
    // in a document that IS held wins over every grant, including one whose content is
    // unknown here.
    let unreadable = !policies.uncarried.is_empty()
        || (evidence == Evidence::HeldDocuments && policies.documents.is_empty());
    let granted = iam::is_allowed(stated, Decision::NotStated) || unreadable;
    // "A permissions boundary sets the maximum permissions that an identity-based
    // policy can grant to an entity": the boundary CAPS what those policies grant and
    // grants nothing itself, so a call it does not allow is refused even where they
    // allow it. A role carrying none — or one whose document is not held, a ceiling
    // this emulator cannot read — is capped by nothing.
    let capped = iam
        .permissions_boundary_document(role_arn)
        .map_or(Decision::Allowed, |document| {
            iam::identity_decision(::std::slice::from_ref(&document), &request)
        });
    if granted && capped == Decision::Allowed {
        return None;
    }
    // Why the call was refused: an explicit `Deny` in the boundary, which wins over every
    // `Allow`, or the absence of an `Allow`, which names the DOCUMENT that withheld it:
    // the boundary where the identity-based policies did allow the call, and the
    // identity-based policies where nothing allowed it at all.
    let because = if capped == Decision::Denied {
        "with an explicit deny in a permissions boundary".to_string()
    } else if granted {
        format!("because no permissions boundary allows the {action} action")
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    Some(because)
}

/// Every spelling of the identity a call is made as, which is what a resource-based
/// statement's `Principal` element is matched against: the SESSION of the execution role
/// this service assumed for the work ([`assumed_role_session`]), and the ROLE's own ARN —
/// a statement naming the role covers every session of it ("the principals in the policy
/// include the role").
fn acting_principals(acts: Acts<'_>) -> Vec<String> {
    let mut principals = Vec::new();
    if let Some(session) = assumed_role_session(acts.role_arn, acts.session_name) {
        principals.push(session);
    }
    principals.push(acts.role_arn.to_string());
    principals
}
