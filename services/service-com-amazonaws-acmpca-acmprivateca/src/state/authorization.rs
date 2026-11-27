// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step every operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the request's
//! own input, so a caller whose policies do not grant the action is told it is not authorized
//! rather than told anything about the private CA it named, and the call performs nothing.
//! This service's model declares no error shape for that decision — no operation lists an
//! access error (`smithy_query(query="digest")`) — because the refusal is the AWS COMMON
//! `AccessDeniedException`, answered by the layer ahead of a service's own code. Every wire
//! protocol this service's model declares (`smithy_query(query="service_info")`) discriminates
//! a failure by the error type in the body rather than by the transport status, so whichever
//! one a request names, the refusal travels with this service's client-error status
//! ([`crate::scaffolded::ACCESS_DENIED_STATUS`]) under the error code
//! [`crate::scaffolded::ACCESS_DENIED_CODE`], through
//! [`crate::scaffolded::Scaffolded::AccessDenied`].
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for (`sts_store::resolve_caller`, the same place `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for
//!    the credentials it signed with), else the IAM user that owns the long-term key, else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes effect on
//!    the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)` under the request's condition context,
//!    so this service and every other one handed the same documents answer alike: an
//!    explicit `Deny` wins, otherwise an `Allow` about the request grants it, and with
//!    neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor its wording does. That is the
//! split every other gate in this tree makes (AWS Certificate Manager, AWS Secrets Manager,
//! Amazon OpenSearch Service, CloudWatch RUM), and the lookups stay in the service crate for
//! the reason those keep theirs there: hoisting them into `service-common` would make every
//! service that depends on that crate link the IAM and STS service crates. WHICH IAM ENTITY a
//! principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action, the resource and the condition keys
//!
//! The action is `acm-pca:<OperationName>` ([`ACTION_PREFIX`] is this service's
//! `arnNamespace`, `smithy_query(query="service_info")`), so a policy decides each operation
//! separately; nothing here is keyed on which operation asked, because the action is composed
//! from [`smithy_server_runtime::Operation::NAME`] at each handler's one call site.
//!
//! The resource is the private CA the request NAMES — the ARN its `CertificateAuthorityArn`
//! (or a policy operation's `ResourceArn`) carries, taken from the request rather than
//! resolved from a stored record, which is what keeps the decision independent of whether
//! that CA exists and keeps a refusal from disclosing whether it does. An action that
//! addresses no CA of its own — creating one, listing the account's — is judged against
//! [`ANY_RESOURCE`], as IAM writes such a request.
//!
//! A statement's `Condition` applies only for the keys the request can answer, and those are
//! the keys a handler publishes: the ARN of the certificate template an issuance names
//! ([`TEMPLATE_ARN_KEY`]), beside the keys that name the calling principal. Every other key
//! is left unpublished, so a statement naming one stays undecided and the shared evaluator
//! reads it fail-closed.
//!
//! The decision here is the IDENTITY-based one. The policy a private CA carries is a document
//! of a different kind: "a policy grants access on a private CA to an Amazon Web Services
//! customer account, to Amazon Web Services Organizations, or to an Amazon Web Services
//! Organizations unit" (`PutPolicy` docs) — the cross-account share RAM also writes — so it
//! grants to principals of ANOTHER account rather than bounding the owning account's own
//! callers, and it neither grants nor refuses the requests this gate judges.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the
//!   store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::scaffolded::AccessDeniedException;

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `acm-pca:<OperationName>` ("Actions defined
/// by AWS Private Certificate Authority", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::ARN_SERVICE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_SERVICE;

/// The condition key an issuance answers with the certificate template it named: the
/// service-specific key AWS Private Certificate Authority publishes for `IssueCertificate`,
/// whose value is the request's `TemplateArn`.
pub const TEMPLATE_ARN_KEY: &str = "acm-pca:TemplateArn";

/// The global condition keys that name the PRINCIPAL a request was signed by, which this gate
/// answers exactly: the ARN of the principal that made the request, and the account that
/// principal belongs to (IAM's global condition keys).
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES
/// (or [`ANY_RESOURCE`] for an action that addresses no CA of its own).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the refusal otherwise, which each operation returns through
/// [`crate::scaffolded::Scaffolded::AccessDenied`]. Call it FIRST in a handler, before the
/// store is read or written and before the request's own values are judged, so a refused call
/// performs nothing and tells an unauthorized caller nothing about what the request named.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may take
/// this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    authorize_with(engine, ctx, operation, resource, Vec::new())
}

/// [`authorize`] for a request that publishes condition keys of its own: `published` are the
/// `(key, value)` pairs its members populate — [`TEMPLATE_ARN_KEY`] for an issuance — which
/// the statements' `Condition` blocks are decided against beside the keys naming the caller.
pub fn authorize_with<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    published: Vec<(String, String)>,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    let refusal = |reason: &str| {
        AccessDeniedException::new(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            reason,
        ))
    };
    // Every key the decision may read: the ones the request's own members populate, and the
    // ones that name the principal it was signed by. A credential that speaks for nobody
    // populates neither of the latter.
    let mut context = published;
    if !caller.principals.is_empty() {
        context.push((PRINCIPAL_ARN_KEY.to_string(), caller.arn.clone()));
        if let Some(parsed) = ::service_common::arn::parse(&caller.arn) {
            if !parsed.account.is_empty() {
                context.push((PRINCIPAL_ACCOUNT_KEY.to_string(), parsed.account.to_string()));
            }
        }
    }
    // The keys the request DECLARES it models, which is what gives IAM's empty-set reading
    // something to decide: every key it carries a value for.
    let published: Vec<&str> = context.iter().map(|(key, _)| key.as_str()).collect();
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no such
    // element, so the decision rests on the action, the resource and the conditions.
    let request = iam::Request::new(&action, resource, &caller.principals)
        .with_context(&context)
        .with_published_keys(&published);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken before any grant is read, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(refusal(iam::IDENTITY_POLICY_EXPLICIT_DENY));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(refusal(&iam::no_identity_policy_allows(&action)));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(refusal(&reason));
    }
    Ok(())
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
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
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

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    ///
    /// `principal_policies` is where the collection rule itself lives — a role's or user's
    /// inline policies, the default version of each managed policy attached to it, and for a
    /// user the policies of the groups it belongs to — so this service reads an identity's
    /// permissions exactly as IAM's own authorizer does.
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

/// The principal this request speaks for, with the policies in force on it.
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
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request —
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before any handler runs.
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
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the user's
    // policies rather than served as the account. A key the IAM store does not hold either
    // really is the account's own published credential.
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
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
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
