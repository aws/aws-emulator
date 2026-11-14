// SPDX-License-Identifier: CC0-1.0

//! The reading of a caller-supplied IAM ARN that services are migrating onto: which
//! entity of this account it names, if any.
//!
//! Several emulated services take an IAM ARN in a request and have to resolve it against
//! the account's IAM records before they write anything (EventBridge Scheduler for a
//! schedule's `Target.RoleArn`, FIS for an experiment template's `roleArn`, Firehose for
//! every `RoleARN` a Firehose stream's configuration carries, EMR for a cluster's
//! `JobFlowRole`, which is an EC2 instance profile). Each of them needs the same two facts
//! out of the ARN — does it name an IAM entity of THAT type of THIS account, and under what
//! name does IAM hold it — so the reading is stated here for them to share.
//!
//! The trust-policy half of the question, once the role is found, is
//! [`crate::iam_trust_policy`]; what the role's own permissions let the service do UNDER it
//! is [`role_withholds_action`].
//!
//! Today Firehose reads it here; the copies that predate this module — `iam_role_name` in
//! `service-com-amazonaws-scheduler-awschronosservice/src/state.rs`, `is_iam_role_arn` in
//! `service-com-amazonaws-fis-faultinjectionsimulator`, and the readings in
//! `service-com-amazonaws-eks-awswesleyfrontend` and
//! `service-com-amazonaws-rolesanywhere-rolesanywhere` — are to be MIGRATED onto it, each
//! in the change that owns its service, rather than copied again. Nothing new should
//! inline the parse, and this module does not inline the ARN grammar either: the six
//! fields come from [`crate::arn`], the crate's one ARN parse.

/// The role name an IAM role ARN carries, when the ARN names a role of THIS account:
/// everything after the last `/`, so a role under a path (`:role/service-role/<name>`)
/// resolves to the name IAM holds it under.
///
/// `None` when the value does not name an IAM role of this account:
///
/// * it is not an ARN of the six-field form, or its vendor field is not `iam`;
/// * an IAM ARN carries no Region, so a value with one is not one;
/// * it names another account, whose roles a service acting for this account cannot
///   assume;
/// * its resource is not a `role/…`, or the role name is empty.
///
/// An EMPTY account field reads as THIS account: several members' modelled patterns make
/// that field optional (`RoleArn` of bedrock-agentcore-control is
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`), and a value that names no account names
/// no OTHER account either, so the role it names is one IAM can be asked about.
///
/// A malformed role ARN is usually refused by the modelled `@pattern` on the member before
/// a handler runs; this reading is what says whether a WELL-FORMED one is this account's.
#[must_use]
pub fn role_name(role_arn: &str) -> Option<&str> {
    entity_name(role_arn, "role")
}

/// The user name an IAM ARN carries, when the ARN names an IAM user of THIS account
/// (`arn:<partition>:iam::<account>:user<path><name>`).
///
/// A user and a role are different IAM entities even where they share a name, so a
/// `role/…` ARN resolves to nothing here. `None` otherwise for the same reasons
/// [`role_name`] answers it.
#[must_use]
pub fn user_name(user_arn: &str) -> Option<&str> {
    entity_name(user_arn, "user")
}

/// The instance profile name an IAM ARN carries, when the ARN names an EC2 instance
/// profile of THIS account (`arn:<partition>:iam::<account>:instance-profile<path><name>`).
///
/// An instance profile and a role are different IAM entities even where they share a name,
/// so a `role/…` ARN resolves to nothing here. `None` otherwise for the same reasons
/// [`role_name`] answers it.
#[must_use]
pub fn instance_profile_name(profile_arn: &str) -> Option<&str> {
    entity_name(profile_arn, "instance-profile")
}

/// The role name an STS ASSUMED-ROLE SESSION ARN names, when the session is one of THIS
/// account (`arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`).
///
/// `sts:AssumeRole` reports the session it minted under that form, and the permissions in
/// force on such a session are the ROLE's — so a service that authorizes a request reads
/// the role out of the principal the credential speaks for, and then reads that role's
/// policies. The role name is the first segment after `assumed-role/`: a session ARN
/// carries no path (the role's path is not part of it) and the session name follows, which
/// may itself carry `/`.
///
/// `None` when the value does not name an assumed-role session of this account: it is not
/// an ARN of the six-field form, its vendor is not `sts`, it carries a Region (an STS
/// session ARN is partition-global), it names another account, its resource is not an
/// `assumed-role/…`, or either the role name or the session name is missing.
///
/// The one reading of this form, for the services that authorize a request against the
/// policies of the role behind a session (`service/authorization.rs` in Amazon Textract
/// today; the copies in AWS Certificate Manager, Amazon Bedrock, CodeArtifact, Amazon ECR
/// Public, AWS KMS, Amazon OpenSearch Service and CloudWatch RUM are to be MIGRATED onto
/// it, each in the change that owns its service).
#[must_use]
pub fn assumed_role_session_role_name(session_arn: &str) -> Option<&str> {
    assumed_role_session(session_arn).map(|(role, _)| role)
}

/// The SESSION name an STS assumed-role session ARN names, when the session is one of
/// THIS account: everything after the role name, which is the `RoleSessionName` the
/// vending call carried and may itself contain `/`.
///
/// `None` in exactly the cases [`assumed_role_session_role_name`] answers it, since both
/// read the one form. The session name is what IAM spells into the session's unique id
/// after the role's own id (`aws:userid` for an assumed-role session is
/// `<role id>:<session name>`), which is why a service deciding a request from such a
/// credential reads it here rather than parsing the ARN again.
#[must_use]
pub fn assumed_role_session_name(session_arn: &str) -> Option<&str> {
    assumed_role_session(session_arn).map(|(_, session)| session)
}

/// The `(role name, session name)` an STS assumed-role session ARN of THIS account
/// carries: the ONE reading of that form, which both of its readers above take.
fn assumed_role_session(session_arn: &str) -> Option<(&str, &str)> {
    // The six fields come from the crate's ONE ARN parse (`crate::arn`), which also
    // settles that the value is an ARN of the `sts` vendor at all.
    let arn = crate::arn::parse_of(session_arn, "sts")?;
    // An STS session ARN is partition-global, like every identity ARN.
    if !arn.region.is_empty() {
        return None;
    }
    if !arn.account.is_empty() && arn.account != ::emulator_server::emulator_account_id() {
        return None;
    }
    let named = arn.resource.strip_prefix("assumed-role")?.strip_prefix('/')?;
    let (role, session) = named.split_once('/')?;
    (!role.is_empty() && !session.is_empty()).then_some((role, session))
}

/// The group name an IAM ARN carries, when the ARN names an IAM group of THIS account
/// (`arn:<partition>:iam::<account>:group<path><name>`).
///
/// A group is a different IAM entity from a user and a role even where they share a name, so
/// a `user/…` or `role/…` ARN resolves to nothing here. `None` otherwise for the same reasons
/// [`role_name`] answers it.
///
/// A group is NOT an identity a request can speak for and not one a resource-based policy can
/// grant to — "you cannot identify a user group as a `Principal` in a policy (such as a
/// resource-based policy) because groups relate to permissions, not authentication" (IAM User
/// Guide, *IAM identities*) — so a service that resolves the principals of a document it is
/// handed reads this to tell such a value apart from an identity that merely does not exist.
#[must_use]
pub fn group_name(group_arn: &str) -> Option<&str> {
    entity_name(group_arn, "group")
}

/// The name an IAM ARN of the entity type `entity_type` carries, when the ARN names an
/// entity of that type of THIS account: everything after the last `/`, so an entity under
/// a path resolves to the name IAM holds it under.
fn entity_name<'a>(value: &'a str, entity_type: &str) -> Option<&'a str> {
    // The six fields come from the crate's ONE ARN parse (`crate::arn`), which also
    // settles that the value is an ARN of the `iam` vendor at all.
    let arn = crate::arn::parse_of(value, "iam")?;
    // An IAM ARN is partition-global: it carries no Region, so a value with one in that
    // field names no IAM resource.
    if !arn.region.is_empty() {
        return None;
    }
    if !arn.account.is_empty() && arn.account != ::emulator_server::emulator_account_id() {
        return None;
    }
    let name = arn
        .resource
        .strip_prefix(entity_type)?
        .strip_prefix('/')?
        .rsplit('/')
        .next()?;
    (!name.is_empty()).then_some(name)
}

/// Whether IAM ANSWERED that `role_arn` is a role `service_principal` cannot assume: it
/// holds no such role of this account, or the trust policy the role carries does not admit
/// that principal.
///
/// This is the whole question a service asks before it stores a resource that RUNS AS a
/// caller-supplied role, stated once here so no service copies the three parts of it: which
/// role of this account the ARN names ([`role_name`]), the in-process `GetRole`
/// ([`crate::iam_client`]), and the trust-policy reading
/// ([`crate::iam_trust_policy::admits_service_principal`]).
///
/// `false` — do not refuse — for everything that is NOT such an answer, because a refusal
/// has to be grounded in what IAM said: a value that names no role of this account (another
/// account's IAM is not this emulator's to ask), an answer this emulator could not read, and
/// any failure other than IAM reporting that the role is absent, including an emulator with
/// no IAM mounted at all. A caller that wants to SEE those cases asks this before it writes
/// and reports what it wrote; this reading stays silent so the one question has one answer.
pub async fn role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    service_principal: &str,
) -> bool {
    role_is_unassumable_through(ctx.callback(), region, role_arn, service_principal).await
}

/// [`role_is_unassumable`] asked through the emulator callback rather than through a
/// request's own context, for a service that resolves the role on a task of its own.
///
/// The question, the grounds and the answer are [`role_is_unassumable`]'s; only who is
/// asking differs ([`crate::iam_client::iam_client_through`]).
pub async fn role_is_unassumable_through(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    service_principal: &str,
) -> bool {
    let Some(role_name) = role_name(role_arn) else {
        return false;
    };
    match role_answer(callback, region, role_name).await {
        // IAM reported the role: it is assumable only if the trust policy it carries
        // admits the principal. A role with no readable grant is refused — that IS what
        // IAM answered about it.
        RoleAnswer::Held(document) => trust_withholds(document.as_deref(), service_principal),
        // IAM says this account holds no such role: the refusal is grounded.
        RoleAnswer::Absent => true,
        // Nothing was learned, so nothing is refused.
        RoleAnswer::Unknown => false,
    }
}

/// Whether IAM ANSWERED that `role_arn` is a role `service_principal` cannot assume FOR ONE
/// PARTICULAR assume — the request whose condition keys `context` states, as `(key, value)`
/// pairs.
///
/// The question [`role_is_unassumable_through`] answers, narrowed to the assume a named piece
/// of work would make: a service that takes a role on to do that work may act under it only
/// where the grant covers it, so a statement's `Condition` is part of the decision
/// ([`crate::iam_trust_policy::grants_service_principal`]) — which is what makes the
/// confused-deputy keys a service presents when it assumes a role (`aws:SourceAccount`,
/// `aws:SourceArn`, naming the resource the work is for) decide anything at all. The keys the
/// assume carries are the caller's to state, and no value is guessed for one it does not: a
/// clause the request cannot be shown to satisfy is no grant, the fail-closed reading
/// [`crate::iam_authorization`] takes of every undecidable clause.
///
/// The grounds and the non-answers are [`role_is_unassumable_through`]'s, so the two readings
/// of one document differ only in how much of the assume they are told about: `false` — do
/// not refuse — for a value that names no role of this account, an answer this emulator could
/// not read, and an emulator with no IAM to ask.
pub async fn role_assume_is_unauthorized_through(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    service_principal: &str,
    context: &[(String, String)],
) -> bool {
    let Some(role_name) = role_name(role_arn) else {
        return false;
    };
    match role_answer(callback, region, role_name).await {
        RoleAnswer::Held(document) => !document.is_some_and(|document| {
            crate::iam_trust_policy::grants_service_principal(
                &document,
                service_principal,
                role_arn,
                context,
            )
            .unwrap_or(false)
        }),
        RoleAnswer::Absent => true,
        RoleAnswer::Unknown => false,
    }
}

/// Whether IAM ANSWERED that `role_arn` is a role it HOLDS and `service_principal` cannot
/// assume: this account holds a role under the name the ARN carries, and the trust policy
/// that stored role carries does not admit that principal.
///
/// The same question [`role_is_unassumable`] answers, for a service whose recorded behaviour
/// SERVES a request naming a role this account's IAM holds nothing under. The two readings
/// differ in that one case only, and the caller picks between them by what its service is
/// recorded doing: a role record is created in this emulator's IAM only by a client that
/// created it HERE, while an execution role is routinely provisioned out of band, so "IAM
/// holds no such role" is evidence about this emulator's store and not about the role the
/// caller named. A service that answers a request for such a role with a refusal makes its
/// whole create surface unreachable for that client, so it reads the question here and
/// refuses only what the stored trust document itself withholds.
///
/// `false` — do not refuse — for every other case, for the reason [`role_is_unassumable`]
/// gives: a value that names no role of this account, an answer this emulator could not read,
/// an emulator with no IAM mounted at all, and now a role this account's IAM holds nothing
/// under.
pub async fn held_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    service_principal: &str,
) -> bool {
    held_role_is_unassumable_by_every(ctx, region, role_arn, &[service_principal]).await
}

/// Whether IAM ANSWERED that `role_arn` is a role it HOLDS that NONE of `service_principals`
/// can assume: this account holds a role under the name the ARN carries, and the trust policy
/// that stored role carries admits not one of those principals.
///
/// [`held_role_is_unassumable`] is this asked about ONE principal. A service a trust policy may
/// name under SEVERAL spellings — the global `<service>.amazonaws.com` and the Regional
/// `<service>.<region>.amazonaws.com` a policy written for one Region's resources names — is
/// admitted by a document naming ANY of them, because each of them names that same service, so
/// a caller offers every spelling it is written under and the role is unassumable only when the
/// document admits none. The rule per principal, the grounds and the non-answers are
/// [`held_role_is_unassumable`]'s, and the ONE hop to IAM is shared by the spellings.
///
/// `false` — do not refuse — for an EMPTY list as well: it asks nothing of the document, so it
/// establishes nothing about the role.
pub async fn held_role_is_unassumable_by_every(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    service_principals: &[&str],
) -> bool {
    if service_principals.is_empty() {
        return false;
    }
    let Some(role_name) = role_name(role_arn) else {
        return false;
    };
    match role_answer(ctx.callback(), region, role_name).await {
        RoleAnswer::Held(document) => service_principals
            .iter()
            .all(|principal| trust_withholds(document.as_deref(), principal)),
        RoleAnswer::Absent | RoleAnswer::Unknown => false,
    }
}

/// Whether IAM ANSWERED that this account holds no role under the name `role_arn` carries.
///
/// The EXISTENCE half of the question the readings above answer, on its own: a service that
/// has to tell a value naming no role of this account apart from one naming a role it may
/// not be given — because its API answers the two with different modelled errors — asks this
/// first and takes the other reading afterwards.
///
/// `false` — establish nothing — for every answer that is NOT IAM reporting the role absent:
/// a value that names no role of THIS account (another account's IAM is not this emulator's
/// to ask, and whether such a value may be passed at all is the pass-role question), an
/// answer this emulator could not read, and an emulator with no IAM mounted to ask. A
/// refusal has to be grounded in what IAM said.
pub async fn role_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    let Some(role_name) = role_name(role_arn) else {
        return false;
    };
    matches!(
        role_answer(ctx.callback(), region, role_name).await,
        RoleAnswer::Absent
    )
}

/// Whether the trust document IAM reported for a role it HOLDS withholds `service_principal`:
/// the ONE reading of a stored trust document both readings above take, so a role IAM
/// answered about is judged the same way whichever of them asked.
///
/// A document IAM carried none of, and one this emulator cannot read as a policy, carry no
/// grant that can be acted on, so they withhold it: that document IS what IAM answered about
/// the role.
fn trust_withholds(document: Option<&str>, service_principal: &str) -> bool {
    !document.is_some_and(|document| {
        crate::iam_trust_policy::admits_service_principal(document, service_principal)
            .unwrap_or(false)
    })
}

/// Whether IAM ANSWERED that `role_arn` is a role this account cannot hand to
/// `service_principal` for the request `context` describes: the value names no role of this
/// account, this account holds no such role, or the trust policy the role carries does not
/// grant that principal the assume THIS request would make.
///
/// The question [`role_is_unassumable`] answers, narrowed to ONE pass: a service a caller
/// hands a role to, to act under it for the request being authorized, may act under it only
/// where the grant covers that request, so the statement's `Condition` is part of the
/// decision ([`crate::iam_trust_policy::grants_service_principal`]) and the keys the pass
/// carries are the caller's to state — nothing is guessed for a key it does not. That is
/// also why a role of ANOTHER account is refused here where the weaker reading stays silent:
/// `iam:PassRole` is granted by the account that HOLDS the role, so a value naming another
/// account's role is one no principal of this account can pass, whatever that role trusts.
///
/// `false` — do not refuse — for everything that is NOT such an answer, for the reason
/// [`role_is_unassumable`] gives: an answer this emulator could not READ, and any failure
/// other than IAM reporting the role absent, including an emulator with no IAM mounted at
/// all. A trust document IAM DID return and this cannot read as a policy is the other case,
/// and is refused: that document is what IAM answered about the role, and it carries no
/// grant this can act on.
pub async fn role_pass_is_unauthorized(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    service_principal: &str,
    context: &[(String, String)],
) -> bool {
    let Some(role_name) = role_name(role_arn) else {
        return true;
    };
    match role_answer(ctx.callback(), region, role_name).await {
        RoleAnswer::Held(document) => !document.is_some_and(|document| {
            crate::iam_trust_policy::grants_service_principal(
                &document,
                service_principal,
                role_arn,
                context,
            )
            .unwrap_or(false)
        }),
        RoleAnswer::Absent => true,
        RoleAnswer::Unknown => false,
    }
}

/// Whether IAM ANSWERED that the permissions in force on the role `role_arn` names do NOT
/// allow `action` on `resource`.
///
/// This is the question a service asks before it ACTS UNDER a caller-supplied role: the call
/// it then makes is authorized against that role's permissions rather than against the
/// caller's own, so a role whose policies grant that call nothing cannot make it, and the
/// service must not make it either. It is put to IAM itself — the simulation of a principal's
/// policies ("determine the policies' effective permissions"), reached over the in-process
/// hop ([`crate::iam_client`]) — so the documents in force (the attached ones, the inline ones
/// and the ceiling a permissions boundary sets) are collected and decided by the service that
/// HOLDS them, and this crate keeps one evaluation of an IAM document rather than two.
///
/// `false` — withhold nothing — for everything that is NOT such an answer, for the reason
/// [`role_is_unassumable`] gives: a value that names no role of this account, an answer this
/// emulator could not read, and any failure of the hop at all, including an emulator with no
/// IAM mounted. A role this account's IAM holds nothing under falls there too, the way
/// [`held_role_is_unassumable`] reads it: a role record exists in this emulator's IAM only
/// where a client created it HERE, while a delivery or execution role is routinely
/// provisioned out of band, so its absence is evidence about this store and not about the
/// role's permissions. An attachment whose document this account does not hold is a grant
/// nothing can read, so it is one of those non-answers as well — the same deliberate
/// non-refusal the store-reading readers take of it.
///
/// This is the form for a service that does NOT hold the IAM store: it has no access to those
/// records, so it asks their holder. A service that reads them directly instead — Amazon
/// EventBridge Pipes' `state::execution_role`, AWS Step Functions' `state::task_authorization`
/// and the Lambda core's `state::network` — answers the SAME question, and the two routes
/// agree because both end in this crate's one evaluation of an IAM document
/// ([`crate::iam_authorization`]), which the simulation itself is built on. Nothing new should
/// grow a third route: a service with the records at hand reads them, and every other asks
/// here.
pub async fn role_withholds_action(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> bool {
    role_withholds_action_through(ctx.callback(), region, role_arn, action, resource).await
}

/// [`role_withholds_action`] from the emulator callback itself rather than from a request's
/// own context.
///
/// The question is the same and so is the hop; what differs is who is asking, exactly as for
/// [`crate::iam_client::iam_client_through`]. A service that acts under a caller-supplied role
/// OUTSIDE the request that named it — on a task it spawned, or on work the clock made due,
/// which holds a clone of the callback and no `OperationContext` — asks through this, so there
/// is still one route to that answer.
pub async fn role_withholds_action_through(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> bool {
    if role_name(role_arn).is_none() {
        return false;
    }
    let answer = crate::iam_client::iam_client_through(callback, region)
        .simulate_principal_policy()
        .policy_source_arn(role_arn)
        .action_names(action)
        .resource_arns(resource)
        .send()
        .await;
    let Ok(answer) = answer else {
        return false;
    };
    // One result per action asked about, each carrying the decision over the resources asked
    // about; the pair here is one action and one resource, so the FIRST result is the answer,
    // and an answer that reports no result at all is one nothing can be read from.
    answer
        .evaluation_results()
        .first()
        .map(::aws_sdk_iam::types::EvaluationResult::eval_decision)
        .is_some_and(|decision| {
            !matches!(
                decision,
                ::aws_sdk_iam::types::PolicyEvaluationDecisionType::Allowed
            )
        })
}

/// Whether IAM ANSWERED that the permissions in force on the role `role_arn` names DENY
/// `action` on `resource` — an EXPLICIT deny, a statement about this role that says no.
///
/// The narrower half of [`role_withholds_action`], for a service whose work a role is expected
/// to be provisioned for OUT OF BAND: a role record exists in this emulator's IAM only where a
/// client created it here, and a client that created one for a background job routinely attaches
/// no permissions policy at all, so a policy set that merely says NOTHING about the action is no
/// evidence about what the role may do. A statement that denies it is: that is an answer about
/// this role, and the service must not make the call.
///
/// `false` — deny nothing — for everything that is NOT such an answer, for the reasons
/// [`role_withholds_action`] gives: a value that names no role of this account, a policy set
/// that allows or merely does not mention the action, an answer this emulator could not read,
/// and any failure of the hop at all, including an emulator with no IAM mounted.
///
/// The hop is the SAME one [`role_withholds_action`] makes — the simulation of a principal's
/// policies, reached over the in-process callback — so there is still one route to what IAM
/// answers about a role's permissions, and the two readings differ only in which decision they
/// act on.
pub async fn role_denies_action(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> bool {
    if role_name(role_arn).is_none() {
        return false;
    }
    let answer = crate::iam_client::iam_client(ctx, region)
        .simulate_principal_policy()
        .policy_source_arn(role_arn)
        .action_names(action)
        .resource_arns(resource)
        .send()
        .await;
    let Ok(answer) = answer else {
        return false;
    };
    // One result per action asked about, each carrying the decision over the resources asked
    // about; the pair here is one action and one resource, so the FIRST result is the answer,
    // and an answer that reports no result at all is one nothing can be read from.
    answer
        .evaluation_results()
        .first()
        .map(::aws_sdk_iam::types::EvaluationResult::eval_decision)
        .is_some_and(|decision| {
            matches!(
                decision,
                ::aws_sdk_iam::types::PolicyEvaluationDecisionType::ExplicitDeny
            )
        })
}

/// Whether IAM ANSWERED that the role `role_arn` names carries identity-based policies AT ALL
/// — an inline policy embedded in it or a managed policy attached to it.
///
/// This is the question a service asks before it reads an IMPLICIT deny as an answer: a role
/// whose policy set says nothing about an action says nothing about it because the set is
/// EMPTY, which is the routine state of a role provisioned out of band, and a role that states
/// permissions is the authority on what it may do. A caller that acts only on a statement which
/// says no does not need it ([`role_denies_action`]).
///
/// `false` — read no implicit deny — for everything that is NOT an answer of "it carries
/// these": a value that names no role of this account, an answer this emulator could not read,
/// and any failure of either hop, including an emulator with no IAM mounted.
///
/// Two hops, the two lists IAM publishes for a role's identity-based policies
/// (`ListRolePolicies` and `ListAttachedRolePolicies`), asked through the same in-process
/// client every other reading here uses ([`crate::iam_client`]).
pub async fn role_states_permissions(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    let Some(role) = role_name(role_arn) else {
        return false;
    };
    let client = crate::iam_client::iam_client(ctx, region);
    let inline = client
        .list_role_policies()
        .role_name(role)
        .send()
        .await
        .map(|answer| !answer.policy_names().is_empty())
        .unwrap_or(false);
    if inline {
        return true;
    }
    client
        .list_attached_role_policies()
        .role_name(role)
        .send()
        .await
        .map(|answer| !answer.attached_policies().is_empty())
        .unwrap_or(false)
}

/// What IAM answers about a role of this account, as the two readings above share ONE hop.
enum RoleAnswer {
    /// IAM holds a role under that name, and reported this trust document for it (`None`
    /// when the answer carried none).
    Held(Option<String>),
    /// IAM answered that this account holds no role under that name.
    Absent,
    /// Nothing was learned: there is no IAM to ask, or the answer carried no role at all,
    /// which is this emulator failing to READ an answer rather than IAM reporting a bad
    /// role.
    Unknown,
}

/// The in-process `GetRole` every reading above is grounded in
/// ([`crate::iam_client`]).
async fn role_answer(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_name: &str,
) -> RoleAnswer {
    let answer = crate::iam_client::iam_client_through(callback, region)
        .get_role()
        .role_name(role_name)
        .send()
        .await;
    match answer {
        Ok(output) => match output.role() {
            Some(role) => RoleAnswer::Held(
                role.assume_role_policy_document()
                    .map(::std::string::ToString::to_string),
            ),
            None => RoleAnswer::Unknown,
        },
        Err(error) => {
            if error.as_service_error().is_some_and(
                ::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception,
            ) {
                RoleAnswer::Absent
            } else {
                RoleAnswer::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assumed_role_session_name, assumed_role_session_role_name, instance_profile_name,
        role_name, user_name,
    };

    /// The role an STS assumed-role SESSION ARN names: the first segment after
    /// `assumed-role/`, whatever the session name after it, and nothing from any other
    /// form.
    #[test]
    fn a_session_arn_names_the_role_its_permissions_come_from() {
        let account = ::emulator_server::emulator_account_id();
        // (case, value, the role the reading must report)
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "a session of this account",
                format!("arn:aws:sts::{account}:assumed-role/reader/session-1"),
                Some("reader"),
            ),
            (
                // The session name is the caller's `RoleSessionName`, which admits more
                // segments; the role is still the first one.
                "a session name carrying a slash",
                format!("arn:aws:sts::{account}:assumed-role/reader/one/two"),
                Some("reader"),
            ),
            (
                "another partition",
                format!("arn:aws-cn:sts::{account}:assumed-role/reader/session-1"),
                Some("reader"),
            ),
            (
                "a session ARN carrying no account",
                "arn:aws:sts:::assumed-role/reader/session-1".to_string(),
                Some("reader"),
            ),
            (
                "a role name holding the characters IAM admits in one",
                format!("arn:aws:sts::{account}:assumed-role/a_b-1+2=3,4.5@6/session-1"),
                Some("a_b-1+2=3,4.5@6"),
            ),
            (
                "another account",
                "arn:aws:sts::123456789012:assumed-role/reader/session-1".to_string(),
                None,
            ),
            (
                "a session with no session name",
                format!("arn:aws:sts::{account}:assumed-role/reader"),
                None,
            ),
            (
                "an empty role name",
                format!("arn:aws:sts::{account}:assumed-role//session-1"),
                None,
            ),
            (
                "a Region, which an STS session ARN does not carry",
                format!("arn:aws:sts:us-east-1:{account}:assumed-role/reader/session-1"),
                None,
            ),
            (
                "the same resource under the IAM vendor",
                format!("arn:aws:iam::{account}:assumed-role/reader/session-1"),
                None,
            ),
            (
                "a federated user, the other temporary identity",
                format!("arn:aws:sts::{account}:federated-user/reader"),
                None,
            ),
            (
                "an IAM role, whose policies a session's are read from",
                format!("arn:aws:iam::{account}:role/reader"),
                None,
            ),
            ("not an ARN at all", "reader".to_string(), None),
            ("the empty string", String::new(), None),
        ];

        for (case, value, expected) in rows {
            let read = assumed_role_session_role_name(&value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
            // The two readings of the ONE form agree about whether the value names a
            // session at all, and the session name is everything the role name is not:
            // the segments after it, which may themselves carry `/`.
            let session = assumed_role_session_name(&value);
            assert_eq!(
                session.is_some(),
                expected.is_some(),
                "case {case:?}: expected a session name exactly where a role name is read, \
                 got {session:?}"
            );
            if let (Some(role), Some(session)) = (read, session) {
                assert!(
                    value.ends_with(&format!("assumed-role/{role}/{session}")),
                    "case {case:?}: expected the two readings to spell the resource, got \
                     {role:?} and {session:?}"
                );
            }
            // A session ARN names no IAM entity directly: the readings do not cross.
            assert!(
                read.is_none() || (role_name(&value).is_none() && user_name(&value).is_none()),
                "case {case:?}: a session ARN must not read as an IAM role or user ARN"
            );
        }
    }

    /// The name is read from the resource field of an IAM role ARN of this account,
    /// whatever path the role lives under — and from nothing else.
    #[test]
    fn a_role_name_is_read_only_from_this_accounts_role_arn() {
        let account = ::emulator_server::emulator_account_id();
        // (case, value, the name the parse must report)
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "a role under the default path",
                format!("arn:aws:iam::{account}:role/delivery"),
                Some("delivery"),
            ),
            (
                "a role under a path",
                format!("arn:aws:iam::{account}:role/service-role/delivery"),
                Some("delivery"),
            ),
            (
                "a role under a nested path",
                format!("arn:aws:iam::{account}:role/a/b/c/delivery"),
                Some("delivery"),
            ),
            (
                "another partition",
                format!("arn:aws-cn:iam::{account}:role/delivery"),
                Some("delivery"),
            ),
            (
                "a role name holding the characters IAM admits in one",
                format!("arn:aws:iam::{account}:role/a_b-1+2=3,4.5@6"),
                Some("a_b-1+2=3,4.5@6"),
            ),
            (
                // The account field is optional in several modelled role-ARN patterns, and
                // an ARN with it empty names no other account.
                "a role ARN carrying no account",
                "arn:aws:iam:::role/delivery".to_string(),
                Some("delivery"),
            ),
            (
                "another account",
                "arn:aws:iam::123456789012:role/delivery".to_string(),
                None,
            ),
            (
                "another IAM entity type",
                format!("arn:aws:iam::{account}:user/delivery"),
                None,
            ),
            (
                "an empty role name",
                format!("arn:aws:iam::{account}:role/"),
                None,
            ),
            (
                "a Region in the IAM field, which an IAM ARN does not carry",
                format!("arn:aws:iam:us-east-1:{account}:role/delivery"),
                None,
            ),
            (
                "another vendor",
                format!("arn:aws:sts::{account}:role/delivery"),
                None,
            ),
            ("not an ARN at all", "delivery".to_string(), None),
            ("the empty string", String::new(), None),
        ];

        for (case, value, expected) in rows {
            let read = role_name(&value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The same reading, for the OTHER IAM entity a service resolves by ARN: an EC2
    /// instance profile. An instance profile and a role are different entities even where
    /// they share a name, so each ARN resolves under its own entity type and under no
    /// other.
    #[test]
    fn an_instance_profile_name_is_read_only_from_this_accounts_instance_profile_arn() {
        let account = ::emulator_server::emulator_account_id();
        // (case, value, the name the parse must report)
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "an instance profile under the default path",
                format!("arn:aws:iam::{account}:instance-profile/EMR_EC2_DefaultRole"),
                Some("EMR_EC2_DefaultRole"),
            ),
            (
                "an instance profile under a path",
                format!("arn:aws:iam::{account}:instance-profile/service-role/EMR_EC2_DefaultRole"),
                Some("EMR_EC2_DefaultRole"),
            ),
            (
                "another partition",
                format!("arn:aws-cn:iam::{account}:instance-profile/EMR_EC2_DefaultRole"),
                Some("EMR_EC2_DefaultRole"),
            ),
            (
                "another account",
                "arn:aws:iam::123456789012:instance-profile/EMR_EC2_DefaultRole".to_string(),
                None,
            ),
            (
                "a Region in the IAM field, which an IAM ARN does not carry",
                format!("arn:aws:iam:us-east-1:{account}:instance-profile/EMR_EC2_DefaultRole"),
                None,
            ),
            (
                "a role ARN, which names another IAM entity",
                format!("arn:aws:iam::{account}:role/EMR_EC2_DefaultRole"),
                None,
            ),
            (
                "an empty instance profile name",
                format!("arn:aws:iam::{account}:instance-profile/"),
                None,
            ),
            (
                "another vendor",
                "arn:aws:s3:::bucket/EMR_EC2_DefaultRole".to_string(),
                None,
            ),
            ("not an ARN at all", "EMR_EC2_DefaultRole".to_string(), None),
            ("the empty string", String::new(), None),
        ];

        for (case, value, expected) in rows {
            let read = instance_profile_name(&value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
            // The entity types do not cross: nothing that reads as an instance profile
            // reads as a role as well.
            assert!(
                read.is_none() || role_name(&value).is_none(),
                "case {case:?}: an instance profile ARN must not read as a role ARN"
            );
        }
    }

    /// The THIRD IAM entity a policy names by ARN: a user. Read under its own entity type,
    /// so a role ARN and a user ARN never resolve to each other.
    #[test]
    fn a_user_name_is_read_only_from_this_accounts_user_arn() {
        let account = ::emulator_server::emulator_account_id();
        // (case, value, the name the parse must report)
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "a user under the default path",
                format!("arn:aws:iam::{account}:user/builder"),
                Some("builder"),
            ),
            (
                "a user under a path",
                format!("arn:aws:iam::{account}:user/team/builder"),
                Some("builder"),
            ),
            (
                "another partition",
                format!("arn:aws-cn:iam::{account}:user/builder"),
                Some("builder"),
            ),
            (
                "a user ARN carrying no account",
                "arn:aws:iam:::user/builder".to_string(),
                Some("builder"),
            ),
            (
                "another account",
                "arn:aws:iam::123456789012:user/builder".to_string(),
                None,
            ),
            (
                "a role ARN, which names another IAM entity",
                format!("arn:aws:iam::{account}:role/builder"),
                None,
            ),
            (
                "an account's root, which names no user",
                format!("arn:aws:iam::{account}:root"),
                None,
            ),
            (
                "an empty user name",
                format!("arn:aws:iam::{account}:user/"),
                None,
            ),
            (
                "a Region in the IAM field, which an IAM ARN does not carry",
                format!("arn:aws:iam:us-east-1:{account}:user/builder"),
                None,
            ),
            ("the wildcard every identity is named by", "*".to_string(), None),
            ("the empty string", String::new(), None),
        ];

        for (case, value, expected) in rows {
            let read = user_name(&value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
            // A value reads as at most ONE entity type.
            assert!(
                read.is_none() || role_name(&value).is_none(),
                "case {case:?}: a user ARN must not read as a role ARN"
            );
        }
    }
}
