// SPDX-License-Identifier: CC0-1.0

//! The PERMISSIONS a stack operation runs under: the identity policies of the stack's
//! service role, and the refusal a call those policies deny is answered with.
//!
//! ## The rule this module implements
//!
//! `CreateStackInput$RoleARN`: "The Amazon Resource Name (ARN) of an IAM role that
//! CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
//! to make calls on your behalf." So the calls a deployment makes in the services that own
//! a stack's resources ([`super::provisioning`]) are not made with the caller's own
//! permissions but with that role's: a change whose API call the role is denied cannot be
//! carried out, and CloudFormation reports the resource it could not act on as failed with
//! that refusal — which is what parks a stack in `UPDATE_ROLLBACK_FAILED` when a rollback's
//! own calls are the denied ones, and what `ContinueUpdateRollback`'s `ResourcesToSkip` then
//! steps over.
//!
//! [`super::role_trust_refusal`] is the other half of the same reading, answered when the
//! request arrives: a role whose TRUST policy does not let CloudFormation assume it at all
//! refuses the operation outright. This module is about a role that CAN be assumed and what
//! its PERMISSIONS policies say about one call.
//!
//! ## What is refused, and what is not
//!
//! The decision is [`::service_common::iam_authorization`]'s — the tree's ONE evaluation of
//! IAM policy documents, so two services handed the same documents cannot answer
//! differently — and for the calls a DEPLOYMENT makes in the services that own a stack's
//! resources ([`refusal`]) it is taken over the ACTION that call invokes. A handler's call is
//! an ordinary request of the owning service made with the role's credentials, so the grant it
//! needs is the grant that request needs: `<service prefix>:<operation name>`, which is how
//! the resource type's own registry schema lists what each of its handlers requires. Two
//! answers refuse such a call, each of them evidence that the role cannot make it:
//!
//! * an EXPLICIT `Deny` of the call, which refuses whatever else is in force ("an explicit
//!   deny in any policy overrides any allows"); and
//! * IAM's DEFAULT DENY, where no readable document of the role allows that action on the
//!   resource the call is about — so a role holding every permission the type's handlers
//!   require EXCEPT one cannot make the call that one action names, which is the
//!   least-privilege scoping a client builds such a role to establish, and the resource fails
//!   with that service's own authorization refusal.
//!
//! A grant that is in force is then CAPPED by the permissions boundary attached to the role —
//! "a permissions boundary sets the maximum permissions that an identity-based policy can
//! grant" (IAM, permissions boundaries) — so a call the boundary does not admit is refused
//! however wide the role's own policies are, and refused whatever this gate could establish
//! about those policies: a ceiling this emulator can read in full bounds every grant the role
//! might carry, including a grant it cannot read. It is asked of the same action and the same
//! resource the grant beneath it is, because a ceiling caps exactly the request a grant would
//! permit. A boundary whose document this emulator does not hold is a ceiling it cannot read,
//! and so applies to nothing.
//!
//! Nothing else about the ROLE is judged: an ARN the IAM store holds no role for may belong to
//! another account, a role carrying an attachment whose document this emulator does not hold
//! carries a grant that cannot be read, a role carrying no readable document at all is one whose
//! permissions this emulator cannot establish, and a document it cannot read as a policy is
//! evidence of nothing. A stack with no `RoleARN` has no role to judge at all: it is carried out
//! with the caller's own permissions ("CloudFormation uses a temporary session that's generated
//! from your user credentials"), which is the principal [`super::pass_role::deploying_principal`]
//! holds and [`refusal`] decides such a deployment's calls against — the same decision, over the
//! other identity.
//!
//! A call CLOUDFORMATION ITSELF makes on a resource the TEMPLATE names reaches the same
//! decision through [`refusal_without_a_grant`], over the action the service being emulated
//! documents for it and the ARN the template itself supplies rather than over an action read
//! off the wire a deployment's call travels. The one call of that kind is the invocation of
//! the provider a custom resource's `ServiceToken` names
//! ([`super::provisioning::custom_resource`]). The same unreadability rule holds there as
//! everywhere else in this module: a role carrying an attached policy whose document this
//! emulator does not hold carries a grant that cannot be read, so the call is not refused for
//! want of one.
//!
//! ## How a call reaches the decision
//!
//! The role is resolved by the OPERATION that drives a deployment, from the IAM store and
//! with no store of this service locked, and put in scope for the whole of that deployment
//! ([`acting_as`]); the provisioning walk puts the RESOURCE each call is about in scope
//! around that resource's own calls ([`on_resource`]). The in-process call helpers then ask
//! [`refusal`] before they send anything, so the one place a call is authorized is the one
//! place it is made, and a call made outside a deployment (a readiness read a client's poll
//! drives, say) is judged by nothing here.
//!
//! Both scopes are TASK-local: a deployment's calls run in the task serving that request, so
//! two requests deploying two stacks under two roles never see each other's.
//!
//! ## Bounds
//!
//! One role's documents per deployment, at most [`MAX_ROLE_DOCUMENTS`] of them — a role
//! carrying more is one this gate cannot read in full, so it decides nothing — plus the one
//! document of the boundary in force on it, cloned out of the IAM store once per deployment and
//! shared by reference for every call of it.

use std::sync::Arc;

use ::service_common::iam_authorization::{self as iam, Decision};

/// The error code the refusal of a denied provisioning call carries.
///
/// The refusal is CloudFormation's own — the call was never made, so no owning service
/// answered a code of its own, and the spellings those services use differ
/// (`AccessDeniedException`, `AuthorizationError`). What a client acts on is the CLASS the
/// refusal falls in, and this is the code that class is read off
/// ([`super::provisioning`]'s `HANDLER_ERROR_CLASSES`), which is what says whether the
/// failed resource is worth retrying.
pub const ACCESS_DENIED: &str = "AccessDenied";

/// The role session name CloudFormation's own calls are made under, which is what an
/// authorization refusal names the caller by.
///
/// `RoleARN`: "CloudFormation uses the role's credentials to make calls on your behalf" —
/// those calls are made as a SESSION of that role, so the principal a refusal is about is
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<this name>`.
const SESSION_NAME: &str = "AWSCloudFormation";

/// The most identity-policy documents this gate reads for one role.
///
/// A role carrying more is one whose permissions this gate cannot establish, so it decides
/// nothing for it rather than deciding on part of them. The documented default quota is 10
/// inline policies and 10 attached managed policies per role, so this is that pair.
const MAX_ROLE_DOCUMENTS: usize = 20;

/// The permissions in force on one stack's service role: the principal the role's calls are
/// made as, and the identity-policy documents that decide them.
///
/// [`Self::unspecified`] is a deployment with no role to judge — a stack that named none, an
/// engine with no IAM store, an ARN that account holds no role for — and it refuses nothing.
#[derive(Debug, Clone, Default)]
pub struct ServiceRole {
    /// The session principal a refusal names ([`SESSION_NAME`]), or `None` where there is no
    /// role.
    principal: Option<String>,
    /// The identity-policy documents in force on the role: its inline policies and the
    /// documents of the policies attached to it, as IAM's own store collects them.
    documents: Vec<String>,
    /// Set when the role carries an attached managed policy whose document this emulator
    /// holds nothing for — an entry of AWS's own catalog it does not carry, which IAM reports
    /// as uncarried. The GRANT such a policy would give cannot be read, so a call for want of
    /// a grant is not refused on it ([`Self::refusal_without_a_grant`]); an explicit `Deny` in
    /// a document that CAN be read still refuses.
    unreadable_attachment: bool,
    /// The permissions boundary attached to the role, when it carries one this emulator holds
    /// the document of: the ARN of the managed policy that is the boundary, and that document.
    ///
    /// It is the CEILING on the role's calls rather than another grant — "a permissions
    /// boundary sets the maximum permissions that an identity-based policy can grant" — so it
    /// is read whatever the role's own documents say, including where there are none and where
    /// an attachment's grant cannot be read: those make a GRANT unestablishable, and a ceiling
    /// that can be read in full bounds every grant there might be. The ARN is kept because a
    /// refusal an explicit `Deny` in the boundary explains NAMES the boundary that carried it.
    boundary: Option<(String, String)>,
}

impl ServiceRole {
    /// A deployment whose calls no service role governs.
    pub fn unspecified() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The permissions the role `role_arn` names carries, read from the IAM store in the
    /// same shared engine the account's roles live in.
    ///
    /// Call it with the handler's `&*self.engine` and NO store of this service locked: it
    /// takes the IAM guard, clones what it reads and releases it, so the deployment that
    /// follows holds neither lock.
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: Option<&str>) -> Arc<Self> {
        let Some(role_arn) = role_arn else {
            return Self::unspecified();
        };
        let Some(iam) = super::iam_state(engine) else {
            return Self::unspecified();
        };
        let store = iam.lock();
        // An ARN this account holds no role for says nothing about any call: the role may
        // belong to another account, or to a client that never told this emulator about it.
        let Some(role) = store.roles.values().find(|role| role.arn == role_arn) else {
            return Self::unspecified();
        };
        let role_name = role.role_name.clone();
        // The documents are IAM's OWN collection of what an identity carries — its inline
        // policies plus the documents of the policies attached to it — called rather than
        // re-derived, so this gate reads exactly what IAM itself judges that role on. An
        // attachment that collection resolved no document for is a grant this gate cannot
        // read, which it has to know about rather than read as the absence of a grant.
        let carried = store.principal_policies(role_arn).unwrap_or_default();
        // The ceiling in force on the role, read through IAM's OWN definition of it, which
        // resolves the attached managed policy's document at the version in effect and answers
        // `None` for a boundary whose document this emulator does not hold — a ceiling it
        // cannot read, and so cannot apply.
        let boundary = store.permissions_boundary_in_force(role_arn);
        drop(store);
        if carried.documents.len() > MAX_ROLE_DOCUMENTS {
            return Self::unspecified();
        }
        Arc::new(Self {
            principal: Some(session_principal(role_arn, &role_name)),
            documents: carried.documents,
            unreadable_attachment: !carried.uncarried.is_empty(),
            boundary,
        })
    }

    /// The sentence refusing a DEPLOYMENT's call of `operation` on the service `signing_name`,
    /// taken on `resource`, or `None` where the role's documents show nothing against that
    /// call.
    ///
    /// The call asks IAM for the action it invokes, so the decision is the one
    /// [`Self::refusal_without_a_grant`] takes — an explicit `Deny` refuses it, and so does
    /// IAM's default deny where no readable document allows that action on that resource —
    /// asked of the action the called service would authorize the request under
    /// ([`reported_action`]).
    ///
    /// A role carrying no readable document AT ALL is the one case no GRANT can be established
    /// for: its permissions are not something this emulator holds any of, so refusing its
    /// deployment's calls for want of a grant would refuse calls AWS permits. The same holds for
    /// a role carrying an attachment whose document is not held, which
    /// [`Self::refusal_without_a_grant`] reads the same way. Neither lifts the CEILING: the
    /// permissions boundary in force on the role caps every grant it might carry, readable or
    /// not ([`Self::outside_the_boundary`]).
    ///
    /// The wording is the one AWS answers an unauthorized call with — the principal, the
    /// action, the resource and why — every part of it read off the call and the account's own
    /// records, worded by the workspace's one authorization refusal
    /// ([`iam::not_authorized_by_identity_policy`]) so two services that reach the same
    /// decision cannot word it differently, and naming the action the way the service that
    /// refuses it words its own refusals ([`reported_action`]).
    fn refusal(&self, signing_name: &str, operation: &str, resource: &str) -> Option<String> {
        let action = reported_action(signing_name, operation);
        if self.documents.is_empty() {
            // No grant can be established — but the ceiling in force on the role caps every
            // grant it might carry, so a call the boundary refuses is still refused.
            return self.outside_the_ceiling(&action, resource);
        }
        self.refusal_without_a_grant(&action, resource)
    }

    /// The sentence refusing `action` on `resource` because the role's permissions BOUNDARY
    /// does not admit it, or `None` where it does — or where the role carries no boundary this
    /// emulator holds the document of, and where there is no role to judge the call on.
    fn outside_the_ceiling(&self, action: &str, resource: &str) -> Option<String> {
        let principal = self.principal.as_deref()?;
        let request = iam::Request::new(action, resource, &[]);
        let reason = self.outside_the_boundary(action, &request)?;
        Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, &reason,
        ))
    }

    /// Why the role's permissions BOUNDARY withholds `action`, or `None` where it does not — or
    /// where the role carries no boundary this emulator holds the document of.
    ///
    /// A boundary is a ceiling and never a grant: "a permissions boundary sets the maximum
    /// permissions that an identity-based policy can grant" (IAM, permissions boundaries), so a
    /// call it does not allow is refused however wide the role's own policies are — which is
    /// the one reading [`super::calling_principal`] gives the same ceiling for a call made with
    /// the caller's own credentials. It is asked of the same action and the same resource the
    /// grant beneath it is, because a ceiling caps exactly the request a grant would permit.
    ///
    /// An explicit `Deny` in the boundary is reported with the boundary NAMED
    /// ([`iam::permissions_boundary_explicit_deny`]): that ARN is the only part of the sentence
    /// saying which ceiling refused the call, and it is the same value the role's own IAM read
    /// reports under `PermissionsBoundary.PermissionsBoundaryArn`.
    fn outside_the_boundary(&self, action: &str, request: &iam::Request<'_>) -> Option<String> {
        let (boundary_arn, document) = self.boundary.as_ref()?;
        match iam::identity_decision(::std::slice::from_ref(document), request) {
            Decision::Allowed => None,
            Decision::Denied => Some(iam::permissions_boundary_explicit_deny(boundary_arn)),
            _ => Some(iam::no_permissions_boundary_allows(action)),
        }
    }

    /// The sentence refusing `action` on `resource` for want of a GRANT: the decision IAM
    /// itself makes, where a call no document allows is refused and an explicit `Deny`
    /// refuses it whatever else is in force.
    ///
    /// `None` where there is no role to judge — a deployment that named none, an ARN this
    /// account holds no role for — where the role's documents allow the call and its boundary
    /// admits it, and where the role carries a grant this emulator cannot read
    /// ([`Self::unreadable_attachment`]): refusing for want of a grant whose document is not
    /// held would refuse a call AWS permits, which is the reading
    /// [`super::calling_principal`] gives the same case. A grant that is in force is capped by
    /// the role's permissions boundary, which for this call has to admit the action itself
    /// ([`Self::outside_the_boundary`]). The reason clause names which of IAM's rules withheld
    /// the call, as the refusal AWS words does.
    fn refusal_without_a_grant(&self, action: &str, resource: &str) -> Option<String> {
        let principal = self.principal.as_deref()?;
        let request = iam::Request::new(action, resource, &[]);
        let withheld = match iam::identity_decision(&self.documents, &request) {
            Decision::Allowed => None,
            Decision::Denied => Some(iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()),
            Decision::NotStated if self.unreadable_attachment => None,
            Decision::NotStated => Some(iam::no_identity_policy_allows(action)),
        };
        let reason = withheld.or_else(|| self.outside_the_boundary(action, &request))?;
        Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, &reason,
        ))
    }
}

/// The IAM action a call of `operation` on `signing_name` asks for, spelled as the service that
/// would refuse it names an action in its own refusal.
///
/// An action is `<service prefix>:<operation name>` ([`iam::iam_action`]), and the prefix is the
/// name the service signs under; the case is that service's own wording, which the workspace
/// states once ([`iam::reported_action`]) so this report of a call the service would have
/// refused and that service's own refusal of the same call cannot spell one action two ways.
fn reported_action(signing_name: &str, operation: &str) -> String {
    iam::reported_action(&iam::iam_action(signing_name, operation))
}

/// The principal a session of `role_arn` is: the partition and the account are the role's
/// own, read off its ARN through the ONE ARN parse this workspace has, so a role of another
/// account is named as that role's session rather than as this emulator's.
fn session_principal(role_arn: &str, role_name: &str) -> String {
    let parsed = ::service_common::arn::parse(role_arn);
    let partition = parsed.map_or("aws", |arn| arn.partition);
    let account = parsed.map_or("", |arn| arn.account);
    format!("arn:{partition}:sts::{account}:assumed-role/{role_name}/{SESSION_NAME}")
}

tokio::task_local! {
    /// The service role the deployment running in this task makes its calls under, set by
    /// [`acting_as`] around the whole of that deployment.
    static ROLE: Arc<ServiceRole>;
    /// The resource the calls running inside this scope are about, set by [`on_resource`]
    /// around one resource's own calls: a policy statement's `Resource` element is what it
    /// is matched against.
    static TARGET: String;
}

/// Run `deployment` with `role`'s permissions in force for every call it makes.
pub async fn acting_as<F: ::std::future::Future>(role: Arc<ServiceRole>, deployment: F) -> F::Output {
    ROLE.scope(role, deployment).await
}

/// Run `calls` — the calls that create, change or delete ONE resource — with that resource
/// as what they are authorized against.
pub async fn on_resource<F: ::std::future::Future>(resource: String, calls: F) -> F::Output {
    TARGET.scope(resource, calls).await
}

/// The refusal the identity the deployment is carried out AS answers a call of `operation` on
/// `signing_name` with, or `None` where nothing refuses it.
///
/// `signing_name` is the service the call is signed for and `operation` the API operation it
/// invokes, which together are the IAM action it asks for: an action is
/// `<service prefix>:<operation name>`, and the prefix is the name the service signs under.
/// A call made outside a deployment, or about no resource, is judged by nothing: there is no
/// identity in scope to judge it on.
///
/// WHICH identity is what the stack's `RoleARN` decides, and the decision is the same one either
/// way — the identity-based documents in force and the boundary capping them, evaluated by the
/// workspace's one evaluation of policy documents:
///
/// * a stack that NAMES a role makes its calls with that role's credentials ("CloudFormation
///   uses the role's credentials to make calls on your behalf"), which is the role in scope here;
/// * a stack that names NONE is carried out with the CALLER's ("CloudFormation uses a temporary
///   session that's generated from your user credentials"), so the call is decided against the
///   principal that asked for the deployment ([`super::pass_role::deploying_principal`]) —
///   exactly as the owning service would have decided it had the client made the call itself.
///
/// Only one of the two ever answers: a deployment acting as a service role carries a principal
/// that names nobody and refuses nothing, and one acting as the caller carries no role.
///
/// The caller half reaches only the deployments that put a principal in scope
/// ([`super::pass_role::as_principal`]) — the operations that deploy a revision. A walk started
/// by another operation (a delete, a rollback, a continued rollback, a signal, an owed revert)
/// scopes the stack's recorded ROLE but no principal, so for a stack naming none its calls are
/// still judged by nothing; closing that is the business of those operations resolving the
/// caller the same way a deploy does, and until they do, what this answers for them is unchanged.
pub fn refusal(signing_name: &str, operation: &str) -> Option<String> {
    let role = ROLE.try_with(Arc::clone).ok()?;
    let resource = TARGET.try_with(String::clone).ok()?;
    role.refusal(signing_name, operation, &resource).or_else(|| {
        super::pass_role::deploying_principal()?
            .unauthorized_message(&reported_action(signing_name, operation), &resource)
    })
}

/// The refusal the service role in force answers a call of `action` on `resource` with when
/// its documents do not ALLOW it, or `None` where they do — or where there is no role to judge
/// the call on.
///
/// For a call CloudFormation itself makes on a resource the template names, where the action
/// asked for is the documented one of the service being emulated and the ARN is the
/// template's own: there IAM's default deny holds, so the role has to allow the call (see the
/// module docs). `action` is the fully qualified IAM action and `resource` the ARN the call
/// addresses; a call made outside a deployment is judged by nothing, as it is for [`refusal`].
pub fn refusal_without_a_grant(action: &str, resource: &str) -> Option<String> {
    let role = ROLE.try_with(Arc::clone).ok()?;
    role.refusal_without_a_grant(action, resource)
}

/// What a policy statement's `Resource` element is matched against for the calls about
/// `resource`: the ARN the resource carries, which is what a statement names it by, and
/// otherwise the identifier its owning service knows it by.
///
/// Every resource carries an ARN: either its physical id IS one ("Ref returns the Amazon
/// Resource Name (ARN)", for the types documented that way) or one of its `…Arn` return
/// values is, and a type that publishes neither still carries the conventional one
/// [`super::template::provision`] computes for it from the stack's identity and the
/// resource's name. The physical id is the fallback for a record carrying no ARN at all,
/// which is a resource no statement scoped to an ARN can be about either.
pub fn target_of(resource: &super::StoredResource) -> String {
    if resource.physical_resource_id.starts_with("arn:") {
        return resource.physical_resource_id.clone();
    }
    resource
        .attributes
        .iter()
        .find(|(attribute, value)| attribute.ends_with("Arn") && value.starts_with("arn:"))
        .map(|(_, arn)| arn.clone())
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN a provider-invocation decision is taken on, and the action it performs.
    const PROVIDER: &str = "arn:aws:lambda:us-east-1:000000000000:function:provider";
    const INVOKE: &str = "lambda:InvokeFunction";

    /// A role whose session is named and whose documents are `documents`, carrying no
    /// permissions boundary.
    fn role(documents: &[String], unreadable_attachment: bool) -> ServiceRole {
        ServiceRole {
            principal: Some(session_principal(
                "arn:aws:iam::000000000000:role/deployer",
                "deployer",
            )),
            documents: documents.to_vec(),
            unreadable_attachment,
            boundary: None,
        }
    }

    /// The same role with `boundary` as the document of the permissions boundary attached to
    /// it, under [`BOUNDARY_ARN`].
    fn bounded_by(mut judged: ServiceRole, boundary: &str) -> ServiceRole {
        judged.boundary = Some((BOUNDARY_ARN.to_string(), boundary.to_string()));
        judged
    }

    /// The managed policy in force as that boundary, which a refusal an explicit `Deny` in it
    /// explains names.
    const BOUNDARY_ARN: &str = "arn:aws:iam::000000000000:policy/ceiling";

    /// One statement document, as IAM's own grammar writes it.
    fn document(effect: &str, action: &str, resource: &str) -> String {
        ::serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }],
        })
        .to_string()
    }

    /// The same document with the action element written as the `NotAction` the grammar also
    /// admits: the statement is about every action EXCEPT the one its value names.
    fn excluding(effect: &str, not_action: &str, resource: &str) -> String {
        ::serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": effect, "NotAction": not_action, "Resource": resource }],
        })
        .to_string()
    }

    /// RULE: a call CloudFormation itself makes on a template-named resource is decided under
    /// IAM's default deny — allowed only where a readable document allows it, refused with the
    /// clause naming the rule that withheld it, and decided by nothing where there is no role
    /// or where a grant this emulator cannot read may be the one that allows it.
    ///
    /// The rows span what the decision turns on: whether a statement is about the call at all,
    /// how its action and resource are spelled (named in full, a wildcard, a pattern, another
    /// resource's ARN), the effect it states, and whether the role carries an attachment whose
    /// document is not held.
    #[test]
    fn a_call_needing_a_grant_is_refused_unless_a_readable_document_allows_it() {
        // (case, the role, the expected reason clause or None for a call that is not refused)
        let rows: Vec<(&str, ServiceRole, Option<String>)> = vec![
            (
                "an allow of the action on the provider's own ARN",
                role(&[document("Allow", INVOKE, PROVIDER)], false),
                None,
            ),
            (
                "an allow of every action on every resource",
                role(&[document("Allow", "*", "*")], false),
                None,
            ),
            (
                "an allow whose action and resource are patterns the call matches",
                role(
                    &[document(
                        "Allow",
                        "lambda:Invoke*",
                        "arn:aws:lambda:us-east-1:000000000000:function:*",
                    )],
                    false,
                ),
                None,
            ),
            (
                "no statement about the call at all",
                role(&[document("Allow", "sns:Publish", "*")], false),
                Some(iam::no_identity_policy_allows(INVOKE)),
            ),
            (
                "an allow of the action on ANOTHER resource",
                role(
                    &[document(
                        "Allow",
                        INVOKE,
                        "arn:aws:lambda:us-east-1:000000000000:function:someone-else",
                    )],
                    false,
                ),
                Some(iam::no_identity_policy_allows(INVOKE)),
            ),
            (
                "no documents at all",
                role(&[], false),
                Some(iam::no_identity_policy_allows(INVOKE)),
            ),
            (
                "an allow and an explicit deny of the same call",
                role(
                    &[
                        document("Allow", "*", "*"),
                        document("Deny", INVOKE, PROVIDER),
                    ],
                    false,
                ),
                Some(iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()),
            ),
            // A grant this emulator cannot read may be the one that allows the call, so the
            // absence of a readable grant is no evidence against it.
            (
                "an attachment whose document is not held",
                role(&[], true),
                None,
            ),
            // ...but an explicit deny in a document that CAN be read still refuses.
            (
                "an explicit deny beside an attachment whose document is not held",
                role(&[document("Deny", INVOKE, "*")], true),
                Some(iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()),
            ),
            (
                "no role to judge the call on",
                ServiceRole::default(),
                None,
            ),
        ];
        for (case, judged, clause) in rows {
            let expected = clause.map(|clause| {
                iam::not_authorized_by_identity_policy(
                    judged.principal.clone().unwrap_or_default().as_str(),
                    INVOKE,
                    PROVIDER,
                    &clause,
                )
            });
            let actual = judged.refusal_without_a_grant(INVOKE, PROVIDER);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The resource one DEPLOYMENT call of the rows below is about, and the call itself: the
    /// service it is made to and the operation it invokes.
    const QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:orders";
    const QUEUE_SERVICE: &str = "sqs";
    const CREATE_QUEUE: &str = "CreateQueue";

    /// RULE: a DEPLOYMENT's call is refused by an explicit `Deny` about it, and — under IAM's
    /// default deny — where no readable document of the role allows the ACTION that call
    /// invokes on the resource it is about; a role carrying a grant this emulator cannot read,
    /// a role carrying no readable document at all and a deployment with no role are decided by
    /// nothing.
    ///
    /// The rows span what the decision turns on: whether a statement is about the call at all,
    /// how its action element is spelled (the call's own action, another action of the same
    /// service, a wildcard over that service, every action, a `NotAction` excluding another
    /// action and one excluding the whole service), what its resource element names (the
    /// resource itself, another one, every one), the effect it states, and what the role carries
    /// beside its readable documents. The refusal's own wording is held to as well, action
    /// included: the case is the refusing service's ([`reported_action`]).
    #[test]
    fn a_deployment_call_is_refused_unless_a_readable_document_allows_that_action() {
        let action = reported_action(QUEUE_SERVICE, CREATE_QUEUE);
        // (case, the role, the expected reason clause or None for a call that is not refused)
        let rows: Vec<(&str, ServiceRole, Option<String>)> = vec![
            (
                "an allow of the call's own action on the resource",
                role(&[document("Allow", "sqs:CreateQueue", QUEUE)], false),
                None,
            ),
            // The least-privilege case this gate exists for: every action of the service but
            // the one the call invokes.
            (
                "an allow of ANOTHER action of the called service",
                role(&[document("Allow", "sqs:DeleteQueue", "*")], false),
                Some(iam::no_identity_policy_allows(&action)),
            ),
            (
                "an allow of the call's action on another resource",
                role(
                    &[document(
                        "Allow",
                        "sqs:CreateQueue",
                        "arn:aws:sqs:us-east-1:000000000000:someone-else",
                    )],
                    false,
                ),
                Some(iam::no_identity_policy_allows(&action)),
            ),
            (
                "an allow under a wildcard over the called service",
                role(&[document("Allow", "sqs:*", "*")], false),
                None,
            ),
            (
                "an allow of every action on every resource",
                role(&[document("Allow", "*", "*")], false),
                None,
            ),
            (
                "every action of ANOTHER service and none of this one",
                role(&[document("Allow", "ssm:*", "*")], false),
                Some(iam::no_identity_policy_allows(&action)),
            ),
            (
                "an allow of one unrelated action",
                role(&[document("Allow", "s3:GetObject", "*")], false),
                Some(iam::no_identity_policy_allows(&action)),
            ),
            (
                "an allow of every action but ANOTHER action of the called service",
                role(&[excluding("Allow", "sqs:DeleteQueue", "*")], false),
                None,
            ),
            (
                "an allow of every action but the called service's own",
                role(&[excluding("Allow", "sqs:*", "*")], false),
                Some(iam::no_identity_policy_allows(&action)),
            ),
            (
                "an allow of everything beside an explicit deny of the call",
                role(
                    &[
                        document("Allow", "*", "*"),
                        document("Deny", "sqs:CreateQueue", QUEUE),
                    ],
                    false,
                ),
                Some(iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()),
            ),
            // A grant this emulator cannot read may be the one that allows the call.
            (
                "an attachment whose document is not held",
                role(&[document("Allow", "ssm:*", "*")], true),
                None,
            ),
            // ...and a role whose permissions this emulator holds nothing of establishes
            // nothing about one call.
            ("no readable document at all", role(&[], false), None),
            ("no role to judge the call on", ServiceRole::default(), None),
        ];
        for (case, judged, clause) in rows {
            let expected = clause.map(|clause| {
                iam::not_authorized_by_identity_policy(
                    judged.principal.clone().unwrap_or_default().as_str(),
                    &action,
                    QUEUE,
                    &clause,
                )
            });
            let actual = judged.refusal(QUEUE_SERVICE, CREATE_QUEUE, QUEUE);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the permissions boundary attached to the role CAPS every grant it carries, so a
    /// DEPLOYMENT's call the boundary does not admit is refused however wide the role's own
    /// documents are — and refused whatever this gate could establish about those documents,
    /// since a ceiling read in full bounds a grant it cannot read as much as one it can.
    ///
    /// The rows differ along what the ceiling says about the call (every action, the called
    /// service, another action of it, the call's action on another resource, another service
    /// only, every action but that service's, an explicit deny) and along what the role carries
    /// beneath it (an allow of everything, no readable document, an attachment whose document is
    /// not held).
    #[test]
    fn a_deployment_call_the_role_boundary_does_not_admit_is_refused() {
        let action = reported_action(QUEUE_SERVICE, CREATE_QUEUE);
        let allowed_everything = || role(&[document("Allow", "*", "*")], false);
        // (case, the role, the expected reason clause or None for a call that is not refused)
        let rows: Vec<(&str, ServiceRole, Option<String>)> = vec![
            (
                "a ceiling over every action, beneath an allow of everything",
                bounded_by(allowed_everything(), &document("Allow", "*", "*")),
                None,
            ),
            (
                "a ceiling over the called service",
                bounded_by(allowed_everything(), &document("Allow", "sqs:*", "*")),
                None,
            ),
            // A ceiling is read over the same action and the same resource the grant beneath it
            // is, so one admitting only ANOTHER action of the called service admits no call of
            // this one — the least-privilege ceiling a client attaches a boundary to establish.
            (
                "a ceiling over another action of the called service",
                bounded_by(
                    allowed_everything(),
                    &document("Allow", "sqs:DeleteQueue", "*"),
                ),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            (
                "a ceiling over the call's own action on ANOTHER resource",
                bounded_by(
                    allowed_everything(),
                    &document(
                        "Allow",
                        "sqs:CreateQueue",
                        "arn:aws:sqs:us-east-1:000000000000:someone-else",
                    ),
                ),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            (
                "a ceiling naming only another service",
                bounded_by(allowed_everything(), &document("Allow", "ssm:*", "*")),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            (
                "a ceiling over every action but the called service's",
                bounded_by(allowed_everything(), &excluding("Allow", "sqs:*", "*")),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            (
                "a ceiling that explicitly denies the call",
                bounded_by(
                    allowed_everything(),
                    &document("Deny", "sqs:CreateQueue", QUEUE),
                ),
                Some(iam::permissions_boundary_explicit_deny(BOUNDARY_ARN)),
            ),
            // A role whose GRANTS this gate cannot establish is still bounded by a ceiling it
            // can read in full.
            (
                "a ceiling naming another service, over no readable document at all",
                bounded_by(role(&[], false), &document("Allow", "ssm:*", "*")),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            (
                "a ceiling naming another service, over an unreadable attachment",
                bounded_by(role(&[], true), &document("Allow", "ssm:*", "*")),
                Some(iam::no_permissions_boundary_allows(&action)),
            ),
            // CONTROLS: nothing changes for a role carrying no boundary, and none of this
            // decides a call there is no role to judge.
            (
                "no boundary at all",
                role(&[], true),
                None,
            ),
            (
                "no role to judge the call on",
                ServiceRole::default(),
                None,
            ),
        ];
        for (case, judged, clause) in rows {
            let expected = clause.map(|clause| {
                iam::not_authorized_by_identity_policy(
                    judged.principal.clone().unwrap_or_default().as_str(),
                    &action,
                    QUEUE,
                    &clause,
                )
            });
            let actual = judged.refusal(QUEUE_SERVICE, CREATE_QUEUE, QUEUE);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE (the same ceiling, for a call whose action is the documented one of the service
    /// being emulated): the boundary has to admit THAT action on THAT resource, exactly as the
    /// grant beneath it does.
    #[test]
    fn a_call_needing_a_grant_is_refused_unless_the_boundary_admits_its_action() {
        let allowed = || role(&[document("Allow", "*", "*")], false);
        // (case, the role, the expected reason clause or None for a call that is not refused)
        let rows: Vec<(&str, ServiceRole, Option<String>)> = vec![
            (
                "a ceiling over the action itself",
                bounded_by(allowed(), &document("Allow", INVOKE, PROVIDER)),
                None,
            ),
            (
                "a ceiling over another action of the same service",
                bounded_by(allowed(), &document("Allow", "lambda:CreateAlias", "*")),
                Some(iam::no_permissions_boundary_allows(INVOKE)),
            ),
            (
                "a ceiling over the action on ANOTHER resource",
                bounded_by(
                    allowed(),
                    &document(
                        "Allow",
                        INVOKE,
                        "arn:aws:lambda:us-east-1:000000000000:function:someone-else",
                    ),
                ),
                Some(iam::no_permissions_boundary_allows(INVOKE)),
            ),
            (
                "a ceiling that explicitly denies the call",
                bounded_by(allowed(), &document("Deny", INVOKE, "*")),
                Some(iam::permissions_boundary_explicit_deny(BOUNDARY_ARN)),
            ),
            (
                "a ceiling over every action, above an unreadable attachment",
                bounded_by(role(&[], true), &document("Allow", "*", "*")),
                None,
            ),
            (
                "a ceiling naming another service, above an unreadable attachment",
                bounded_by(role(&[], true), &document("Allow", "ssm:*", "*")),
                Some(iam::no_permissions_boundary_allows(INVOKE)),
            ),
            ("no boundary at all", allowed(), None),
        ];
        for (case, judged, clause) in rows {
            let expected = clause.map(|clause| {
                iam::not_authorized_by_identity_policy(
                    judged.principal.clone().unwrap_or_default().as_str(),
                    INVOKE,
                    PROVIDER,
                    &clause,
                )
            });
            let actual = judged.refusal_without_a_grant(INVOKE, PROVIDER);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
