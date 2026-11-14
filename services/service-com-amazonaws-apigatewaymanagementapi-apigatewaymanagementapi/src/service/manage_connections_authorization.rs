// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller MANAGE the connections of the stage its request addressed.
//!
//! The `@connections` endpoint is reached on the owning api's own `execute-api` invoke
//! endpoint, and every request made there is authorized by IAM: the caller needs
//! `execute-api:ManageConnections` on the stage it addressed, which is the one permission AWS
//! publishes for this surface ("to call the `@connections` API, you need an IAM policy that
//! allows the `execute-api:ManageConnections` action"). Without that decision an
//! authenticated but unprivileged caller is served, which is the one answer AWS never gives:
//! a role session holding no such grant pushes to, reads and closes other people's
//! connections.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`sts_store::resolve_caller`], the same record `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for
//!    the credentials it signed with), else the IAM user that owns the long-term key, else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal, the permissions boundary that caps them, and the SESSION POLICIES the mint of
//!    the credential scoped it with ("the resulting session's permissions are the intersection
//!    of the role's identity-based policy and the session policies",
//!    `AssumeRoleRequest$Policy`), each read through those services' own accessors, so a
//!    document `iam:PutRolePolicy` wrote takes effect on the next call and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation
//!    of those documents, asked about the RESOURCE this call addresses ([`managed_resource`]).
//!
//! ## The resource a `@connections` call is authorized on
//!
//! An `execute-api` resource is `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/`
//! followed by the method and path of the call, which for this surface is the route AWS
//! publishes it under: `POST/@connections/{connectionId}` for `PostToConnection`, and the
//! same with the method each of the other two is reached by. The connection id is the ROUTE's
//! own parameter rather than the value one request carried, so the ARN names
//! [`CONNECTION_ID_PARAMETER`]: that is the spelling AWS's own policy documentation uses for
//! this surface, the spelling the recorded refusal of it names, and one a
//! `…/@connections/*` statement covers.
//!
//! WHICH api and stage that is comes from the service that owns apis and stages, through the
//! same shared cell that judges whether the addressing serves anything at all
//! (`crate::state::connections_endpoint_authority`): this service cannot read those records.
//! An emulator that mounts no such owner resolves no resource, so there is nothing for a
//! statement's `Resource` element to be about and this gate decides nothing — the same
//! reading [`crate::service::connections_endpoint`] takes of an unjudged addressing.
//!
//! ## The callers this gate does not bound
//!
//! The same three the `execute-api` gate of the REST data plane does not bound
//! (`service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke/execute_api_authorization.rs`):
//! the account's own published credential, which holds no IAM policies and is what every
//! in-process caller of this emulator signs with; a principal carrying an attachment whose
//! document this emulator does not hold, where refusing would deny access AWS permits; and a
//! request carrying no SigV4 credential at all, which names no principal to decide about.
//! A credential whose session window has already closed speaks for nobody either, and is the
//! front door's to refuse rather than this gate's.
//!
//! Nothing here writes this service's records.
//!
//! LOCK ORDER: the shared cell's lock is released before the reading it holds is called, and
//! the STS registry's lock and then the IAM store's are each taken and released in turn —
//! this service's own registry is not held while any of them is.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::execute_api_connections_endpoint::AddressedStage;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM action a `@connections` call asks for: the one action AWS publishes for managing
/// a WebSocket api's connections.
pub const MANAGE_CONNECTIONS_ACTION: &str = "execute-api:ManageConnections";

/// The service an `execute-api` resource ARN names.
const ARN_SERVICE: &str = "execute-api";

/// The path the `@connections` surface is published under, as the ROUTE that serves it: its
/// connection id is a route parameter, so the resource an authorization decision is taken
/// about names the parameter rather than one call's value.
const CONNECTION_ID_PARAMETER: &str = "@connections/{connectionId}";

/// The principal a `@connections` call speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on it, when it carries one whose document this
    /// emulator holds.
    boundary: Option<String>,
    /// The session policy documents the mint of this request's credential scoped it with, as
    /// they read at the instant of this request. Empty for a credential no mint scoped — every
    /// long-term access key, and every session minted without session policies — which is a
    /// credential capped by nothing.
    scope: sts_store::ScopingPolicies,
    /// Set when this call presents the account's own published credential, which holds no IAM
    /// policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has an attachment whose document the IAM store does not hold.
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing this gate can read may refuse the caller once no readable document
    /// DENIES the action (see the module docs).
    fn unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through IAM's own accessors.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
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

/// The principal the credential `access_key_id` speaks for, with the identity-based policies
/// in force on it and what its own mint scoped it to.
///
/// `credentials` is the emulator's own credential store, which is what STS resolves a session
/// key through; `now` is the instant the session windows are read at. `None` for a credential
/// whose session window has already closed: it speaks for nobody, so there is no principal to
/// decide about (see the module docs).
#[must_use]
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
    credentials: &::emulator_server::CredentialStore,
    now: SystemTime,
) -> Option<Caller> {
    // Who the credential speaks for AND what its mint scoped it to, read together from the
    // session registry: the scope travels with the credential, so the only thing a later
    // request carries is where it has to be read from.
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            access_key_id,
            now,
            credentials,
            sts_store::ambient_identity(),
        )
        .ok()?,
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy and which no mint
        // scoped.
        None => sts_store::Caller {
            identity: sts_store::ambient_identity(),
            kind: None,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            role_arn: None,
            federating_user_arn: None,
            federated_claims: Vec::new(),
            session_policies: sts_store::SessionPolicies::default(),
            mfa_authenticated: false,
        },
    };
    // WHOSE identity-based documents are in force for this credential, read through STS's one
    // answer for it. Taken before the resolved caller is broken up below.
    let identity_policies_arn = resolved.identity_policies_arn().to_string();
    let federated_session = resolved.federating_user_arn.is_some();
    let mut caller = Caller {
        account_credential: resolved.identity.arn == sts_store::ambient_identity().arn,
        arn: resolved.identity.arn,
        // The inline document the mint passed scopes the credential whether or not an IAM
        // store is there; where there is one, the scope is re-read below so a managed session
        // policy counts at the version in force NOW.
        scope: sts_store::session_scoping_documents(None, &resolved.session_policies),
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return Some(caller);
    };
    let store = iam.lock();
    caller.scope = sts_store::session_scoping_documents(Some(&store), &resolved.session_policies);
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it, so a request signed with one speaks for THAT user and is judged on
    // the user's policies. A key the IAM store does not hold either really is the account's
    // own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    if federated_session {
        // A FEDERATED-user session is judged on the documents of the IAM user whose long-term
        // credentials minted it — "the session permissions are the intersection of the IAM
        // user policies and the session policies that you pass" (`GetFederationToken`).
        caller.read_policies_of(&store, &identity_policies_arn);
    } else if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    Some(caller)
}

/// The ARN of the resource a `@connections` call addresses:
/// `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/@connections/{connectionId}`.
///
/// `method` is the HTTP method the operation is reached by, which is what names the route
/// within the stage.
#[must_use]
pub fn managed_resource(addressed: &AddressedStage, method: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        &addressed.region,
        &format!(
            "{api}/{stage}/{method}/{CONNECTION_ID_PARAMETER}",
            api = addressed.api_id,
            stage = addressed.stage_name,
        ),
    )
}

/// The sentence this call is refused with when IAM does not allow
/// `execute-api:ManageConnections` on the resource it addressed, or `None` when it does.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the call whatever
/// else the principal carries; otherwise a grant is needed, which a caller this gate does not
/// bound has by definition (see the module docs); and a grant is capped BOTH by the session
/// policies the credential was minted with, since "the resulting session's permissions are the
/// intersection of the role's identity-based policy and the session policies"
/// (`AssumeRoleRequest$Policy`), AND by the permissions boundary, since "a permissions boundary
/// sets the maximum permissions that an identity-based policy can grant" (IAM, permissions
/// boundaries). Each cap takes permissions away and grants none of its own.
///
/// The sentence closes with the clause IAM words the REASON in — which document class the
/// decision rested on, and whether its deny was explicit — and both the sentence and the
/// clauses come from the shared [`::service_common::iam_authorization`], so this gate and
/// every other that reaches the same decision word it the same way.
#[must_use]
pub fn refusal(caller: &Caller, resource: &str) -> Option<String> {
    // An identity-based document states no `Principal` element, so the decision rests on the
    // action and the resource the request addressed.
    let request = iam::Request::new(MANAGE_CONNECTIONS_ACTION, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    let granted = iam::is_allowed(said, Decision::NotStated)
        || (said != Decision::Denied && caller.unbounded());
    // What the SCOPE this credential was minted with says, through the tree's one reading of a
    // resolved scope: a credential no mint scoped is capped by nothing, which that reading
    // answers as allowed.
    let scoped = caller.scope.decision(&request);
    // The ceiling in force on the principal. A call an identity-based policy explicitly DENIES
    // is already refused on that deny, so no ceiling is read for it.
    let capped = match (said == Decision::Denied, caller.boundary.as_ref()) {
        (false, Some(boundary)) => {
            iam::identity_decision(::std::slice::from_ref(boundary), &request)
        }
        _ => Decision::Allowed,
    };
    if granted && scoped == Decision::Allowed && capped == Decision::Allowed {
        return None;
    }
    let reason = if said == Decision::Denied {
        iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
    } else if scoped == Decision::Denied {
        iam::SESSION_POLICY_EXPLICIT_DENY.to_string()
    } else if capped == Decision::Denied {
        iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string()
    } else if !granted {
        iam::no_identity_policy_allows(MANAGE_CONNECTIONS_ACTION)
    } else if scoped != Decision::Allowed {
        iam::no_session_policy_allows(MANAGE_CONNECTIONS_ACTION)
    } else {
        iam::no_permissions_boundary_allows(MANAGE_CONNECTIONS_ACTION)
    };
    Some(not_authorized(&caller.arn, resource, &reason))
}

/// The sentence the `execute-api` endpoint refuses an unauthorized `@connections` call with:
/// the principal that asked, the action it asked for, the resource it asked for and `reason`.
fn not_authorized(principal: &str, resource: &str, reason: &str) -> String {
    iam::not_authorized_by_identity_policy(
        principal,
        MANAGE_CONNECTIONS_ACTION,
        resource,
        reason,
    )
}

/// The sentence THIS request is refused with, or `None` when the caller may manage the
/// connections of the stage it addressed.
///
/// The one entry point the three operations take, so all three apply one decision at the same
/// point of their own walk: before the `ConnectionId` is judged and before the connection
/// store is consulted, which is where the endpoint takes it.
///
/// `None` is also the answer for everything this gate cannot decide — no owner of apis and
/// stages mounted, an addressing that resolves no stage, a request carrying no SigV4
/// credential, a credential whose session has already closed — each for the reason the module
/// docs give.
#[must_use]
pub fn denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<String> {
    let parts = ctx.parts();
    let addressed = addressed_stage(engine, parts)?;
    let resource = managed_resource(&addressed, parts.method.as_str());
    // WHICH credential the request presents is read by the emulator's own SigV4 scope parser,
    // the single source of that parsing, so this gate and the front door read one header the
    // same way.
    let scope = ::emulator_server::sigv4_credential_scope(&parts.headers)?;
    let caller = caller(
        engine,
        &scope.access_key_id,
        ctx.credential_store(),
        SystemTime::now(),
    )?;
    refusal(&caller, &resource)
}

/// WHICH api's stage this request addressed, as the owner of apis and stages resolves it
/// through the shared cell, or `None` when nothing resolves one.
fn addressed_stage<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> Option<AddressedStage> {
    // The invoke path in front of the modelled binding is what names the stage; see
    // [`crate::service`].
    let prefix = super::invoke_path_prefix(&parts.uri.to_string())?;
    let host = crate::state::addressed_at(parts);
    let cell = crate::state::connections_endpoint_authority(engine)?;
    // By value, so the cell's own lock is released before the reading takes the store it
    // resolves against — the lock order this service and the owner of stages share.
    let reading = cell.lock().installed_addressed_stage()?;
    reading(engine as &dyn ::std::any::Any, host.as_deref(), &prefix)
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: the resource a call is authorized on, how the
    //! documents combine, and the sentence a refusal states. What the three operations ANSWER
    //! an unprivileged caller is driven over the wire in
    //! `tests/all/manage_connections_authorization.rs`.

    use super::*;

    /// The api and stage every case below addresses.
    const API_ID: &str = "a1b2c3d4e5";
    const REGION: &str = "us-east-1";
    const STAGE: &str = "dev";

    fn addressed() -> AddressedStage {
        AddressedStage {
            api_id: API_ID.to_string(),
            region: REGION.to_string(),
            stage_name: STAGE.to_string(),
        }
    }

    /// A role session carrying `documents`, with no boundary, no session policy and no
    /// unreadable attachment.
    fn session(documents: &[&str]) -> Caller {
        Caller {
            arn: "arn:aws:sts::000000000000:assumed-role/pusher/session".to_string(),
            documents: documents.iter().map(|d| (*d).to_string()).collect(),
            ..Caller::default()
        }
    }

    /// A document allowing `action` on `resource`.
    fn allow(action: &str, resource: &str) -> String {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
        )
    }

    /// RULE: the resource names the api, the stage, the method the operation is reached by
    /// and the route's own connection-id parameter.
    #[test]
    fn the_resource_is_the_route_within_the_addressed_stage() {
        // The rows span the three methods the surface is reached by.
        for (case, method) in [
            ("GetConnection", "GET"),
            ("PostToConnection", "POST"),
            ("DeleteConnection", "DELETE"),
        ] {
            let resource = managed_resource(&addressed(), method);
            let expected = format!(
                "arn:aws:execute-api:{REGION}:{}:{API_ID}/{STAGE}/{method}/@connections/\
                 {{connectionId}}",
                ::engine::emulator_account_id()
            );
            assert_eq!(
                resource, expected,
                "case {case:?}: expected {expected:?}, got {resource:?}"
            );
        }
    }

    /// RULE: the call is served only where a readable document allows the action on the
    /// resource, and an explicit deny refuses it whatever else is granted.
    #[test]
    fn only_a_grant_of_the_action_on_the_resource_admits_the_call() {
        let resource = managed_resource(&addressed(), "POST");
        let every_resource = allow(MANAGE_CONNECTIONS_ACTION, "*");
        let this_route = allow(MANAGE_CONNECTIONS_ACTION, &resource);
        let the_stage = allow(
            MANAGE_CONNECTIONS_ACTION,
            &format!(
                "arn:aws:execute-api:{REGION}:{}:{API_ID}/{STAGE}/POST/@connections/*",
                ::engine::emulator_account_id()
            ),
        );
        let another_stage = allow(
            MANAGE_CONNECTIONS_ACTION,
            &format!(
                "arn:aws:execute-api:{REGION}:{}:{API_ID}/other/POST/@connections/*",
                ::engine::emulator_account_id()
            ),
        );
        let another_action = allow("execute-api:Invoke", "*");
        let deny_every_resource = format!(
            "{{\"Statement\":[{{\"Effect\":\"Deny\",\"Action\":\"{MANAGE_CONNECTIONS_ACTION}\",\
             \"Resource\":\"*\"}}]}}"
        );
        // The rows span what the rule generalises over: the resource a grant is scoped to,
        // the action it names, the absence of any document, and a deny beside a grant.
        let rows: &[(&str, &[&str], bool)] = &[
            ("every resource", &[&every_resource], true),
            ("this very route", &[&this_route], true),
            ("the stage's connections by wildcard", &[&the_stage], true),
            ("another stage of the same api", &[&another_stage], false),
            ("another action on every resource", &[&another_action], false),
            ("no document at all", &[], false),
            ("an explicit deny", &[&deny_every_resource], false),
            (
                "an explicit deny beside a grant",
                &[&every_resource, &deny_every_resource],
                false,
            ),
        ];
        for (case, documents, admitted) in rows {
            let caller = session(documents);
            let refused = refusal(&caller, &resource);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: the account's own published credential, and a principal carrying a grant this
    /// emulator cannot read, are not bounded by this gate — while a readable deny still
    /// refuses them.
    #[test]
    fn the_callers_this_gate_cannot_judge_are_not_refused() {
        let resource = managed_resource(&addressed(), "GET");
        let deny = format!(
            "{{\"Statement\":[{{\"Effect\":\"Deny\",\"Action\":\"{MANAGE_CONNECTIONS_ACTION}\",\
             \"Resource\":\"*\"}}]}}"
        );
        for (case, account_credential, unreadable) in [
            ("the account's own credential", true, false),
            ("an attachment this emulator does not hold", false, true),
        ] {
            let mut caller = session(&[]);
            caller.account_credential = account_credential;
            caller.unreadable_attachment = unreadable;
            assert!(
                refusal(&caller, &resource).is_none(),
                "case {case:?}: expected the call to be served"
            );

            let mut denied = caller.clone();
            denied.documents = vec![deny.clone()];
            let refused = refusal(&denied, &resource);
            assert!(
                refused.is_some(),
                "case {case:?}: a readable deny must still refuse, got {refused:?}"
            );

            // And the ceiling in force on the principal refuses it the same way: a document
            // this gate CAN read decides, whatever it could not read about the grant.
            let mut bounded = caller.clone();
            bounded.boundary = Some(deny.clone());
            let refused = refusal(&bounded, &resource);
            assert_eq!(
                refused.as_deref(),
                Some(
                    format!(
                        "User: {} is not authorized to perform: {MANAGE_CONNECTIONS_ACTION} on \
                         resource: {resource} {}",
                        bounded.arn,
                        iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY
                    )
                    .as_str()
                ),
                "case {case:?}: a readable boundary deny must refuse, got {refused:?}"
            );

            // A boundary that states nothing allows nothing either: a cap takes permissions
            // away and grants none of its own, so the refusal names THAT document rather than
            // the grant this gate could not read.
            let mut unstated = caller.clone();
            unstated.boundary = Some("{\"Statement\":[]}".to_string());
            let refused = refusal(&unstated, &resource);
            assert_eq!(
                refused.as_deref(),
                Some(
                    format!(
                        "User: {} is not authorized to perform: {MANAGE_CONNECTIONS_ACTION} on \
                         resource: {resource} {}",
                        unstated.arn,
                        iam::no_permissions_boundary_allows(MANAGE_CONNECTIONS_ACTION)
                    )
                    .as_str()
                ),
                "case {case:?}: a boundary allowing nothing caps the grant, got {refused:?}"
            );
        }
    }

    /// RULE: a permissions boundary caps what a policy granted, and the refusal names the
    /// document class that withheld it.
    #[test]
    fn a_permissions_boundary_caps_the_grant_and_is_named_in_the_refusal() {
        let resource = managed_resource(&addressed(), "DELETE");
        let every_resource = allow(MANAGE_CONNECTIONS_ACTION, "*");
        let boundary_other = allow(MANAGE_CONNECTIONS_ACTION, "arn:aws:execute-api:*:*:other/*");
        let boundary_deny = format!(
            "{{\"Statement\":[{{\"Effect\":\"Deny\",\"Action\":\"{MANAGE_CONNECTIONS_ACTION}\",\
             \"Resource\":\"*\"}}]}}"
        );
        let sentence = format!(
            "User: arn:aws:sts::000000000000:assumed-role/pusher/session is not authorized to \
             perform: {MANAGE_CONNECTIONS_ACTION} on resource: {resource}"
        );
        // The rows span the boundary readings this gate reaches.
        let rows: &[(&str, Option<&str>, Option<String>)] = &[
            ("no boundary at all", None, None),
            (
                "a boundary allowing the action",
                Some(&every_resource),
                None,
            ),
            (
                "a boundary allowing only another api",
                Some(&boundary_other),
                Some(format!(
                    "{sentence} {}",
                    iam::no_permissions_boundary_allows(MANAGE_CONNECTIONS_ACTION)
                )),
            ),
            (
                "a boundary denying the action",
                Some(&boundary_deny),
                Some(format!(
                    "{sentence} {}",
                    iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY
                )),
            ),
        ];
        for (case, boundary, expected) in rows {
            let mut caller = session(&[&every_resource]);
            caller.boundary = boundary.map(str::to_string);
            let refused = refusal(&caller, &resource);
            assert_eq!(
                refused.as_deref(),
                expected.as_deref(),
                "case {case:?}: got {refused:?}"
            );
        }
    }

    /// RULE: the SESSION POLICIES a credential was minted with cap what the role's own
    /// policies granted — "the resulting session's permissions are the intersection of the
    /// role's identity-based policy and the session policies" — and the refusal names that
    /// document class.
    #[test]
    fn the_session_policies_the_credential_was_minted_with_cap_the_grant() {
        let resource = managed_resource(&addressed(), "POST");
        let every_resource = allow(MANAGE_CONNECTIONS_ACTION, "*");
        let another_api = allow(MANAGE_CONNECTIONS_ACTION, "arn:aws:execute-api:*:*:other/*");
        let deny = format!(
            "{{\"Statement\":[{{\"Effect\":\"Deny\",\"Action\":\"{MANAGE_CONNECTIONS_ACTION}\",\
             \"Resource\":\"*\"}}]}}"
        );
        let sentence = format!(
            "User: arn:aws:sts::000000000000:assumed-role/pusher/session is not authorized to \
             perform: {MANAGE_CONNECTIONS_ACTION} on resource: {resource}"
        );
        // Every row's ROLE allows the action on every resource, so what decides each is the
        // scope its own mint passed: none at all, one allowing the call, one allowing only
        // another api, and one denying it.
        let rows: &[(&str, Option<&str>, Option<String>)] = &[
            ("no session policy at all", None, None),
            (
                "a session policy allowing the action",
                Some(&every_resource),
                None,
            ),
            (
                "a session policy allowing only another api",
                Some(&another_api),
                Some(format!(
                    "{sentence} {}",
                    iam::no_session_policy_allows(MANAGE_CONNECTIONS_ACTION)
                )),
            ),
            (
                "a session policy denying the action",
                Some(&deny),
                Some(format!("{sentence} {}", iam::SESSION_POLICY_EXPLICIT_DENY)),
            ),
        ];
        for (case, inline, expected) in rows {
            let mut caller = session(&[&every_resource]);
            caller.scope = sts_store::session_scoping_documents(
                None,
                &sts_store::SessionPolicies {
                    inline: inline.map(str::to_string),
                    ..sts_store::SessionPolicies::default()
                },
            );
            let refused = refusal(&caller, &resource);
            assert_eq!(
                refused.as_deref(),
                expected.as_deref(),
                "case {case:?}: got {refused:?}"
            );
        }
    }

    /// RULE: a call nothing allows is refused with the sentence IAM words an authorization
    /// failure in — the principal, the action, the resource and the reason clause.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let resource = managed_resource(&addressed(), "POST");
        let refused = refusal(&session(&[]), &resource)
            .expect("case \"no document at all\": the call must be refused");
        let expected = format!(
            "User: arn:aws:sts::000000000000:assumed-role/pusher/session is not authorized to \
             perform: {MANAGE_CONNECTIONS_ACTION} on resource: {resource} {}",
            iam::no_identity_policy_allows(MANAGE_CONNECTIONS_ACTION)
        );
        assert_eq!(
            refused, expected,
            "case \"no document at all\": expected {expected:?}, got {refused:?}"
        );
    }
}
