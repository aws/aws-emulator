// SPDX-License-Identifier: CC0-1.0

//! WHO a git request speaks for, and whether that principal may read from or write
//! to the repository it addressed — the decision the git transport takes on every
//! authorization step of a fetch and of a push.
//!
//! CodeCommit's git transport is not an anonymous one: every request to it is made
//! with HTTPS git credentials, and what the principal behind them may do is decided
//! by IAM. The two actions are `codecommit:GitPull` (a fetch or clone) and
//! `codecommit:GitPush` (a push), and a push is decided PER REFERENCE: the
//! `codecommit:References` condition key carries the fully qualified name of the ref
//! an update names, which is what makes a `Deny` on `codecommit:GitPush` scoped to
//! one ref the only branch protection the service has.
//!
//! A push is authorized in more than one STEP, and they do not all carry a
//! reference. The ref advertisement a push begins with names none — the client has
//! not said what it intends to update yet — so that step is decided with NO value
//! for `codecommit:References`, while each ref update command of the pack request is
//! decided with its own. That is why a `Deny` on `codecommit:GitPush` has to carry
//! the guard `Null: {"codecommit:References": "false"}`: without it, the
//! `StringEqualsIfExists` clause that scopes the deny to one ref holds on the
//! keyless step too, and the deny widens from that ref to the whole repository. The
//! steps are evaluated here exactly as they arrive, so that guard behaves here as it
//! does on AWS rather than being an artifact of evaluating the condition once with a
//! ref in hand.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the credential the request PRESENTED speaks for. A git
//!    client presents it as HTTP Basic credentials, whose user name is what the two
//!    documented ways of getting CodeCommit git credentials produce: the
//!    `aws codecommit credential-helper` writes the caller's access key id (with its
//!    session token after a `%`), and an IAM service-specific credential for
//!    `codecommit.amazonaws.com` is a generated `ServiceUserName`. Both are resolved
//!    through the stores that MINTED them — the STS session registry and the IAM
//!    account — so the principal a refusal is decided for is the one the caller
//!    reads back for its own credentials.
//! 2. **what it may do** — the identity-based policy documents in force on that
//!    principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors, where the collection rule lives.
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)` and the condition
//!    context, so this transport and every signed call of this service answer alike.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does.
//! They stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a
//! dependency cycle — the same reason every other service that decides this keeps
//! its lookups beside its own state.
//!
//! The password is NOT verified, and cannot be: the secret half of a git credential
//! is held by the credential store, which hands a secret back to nobody, so there is
//! nothing here to check it against. What the credential identifies is still the
//! principal that minted it, which is what the authorization decision needs; a
//! request that presents NO credential at all is answered with a challenge instead
//! ([`crate::service::git_http`]), as the service it emulates answers one.
//!
//! The decision is taken with no guard of this service's own store held, and it
//! writes to neither store it reads.

use std::time::SystemTime;

use ::base64::Engine as _;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM action a fetch or clone is performed under: "GitPull — Grants permission
/// to pull information from an CodeCommit repository" (IAM Service Authorization
/// Reference, Actions defined by AWS CodeCommit).
pub const GIT_PULL_ACTION: &str = "codecommit:GitPull";

/// The IAM action a push is performed under: "GitPush — Grants permission to push
/// information to an CodeCommit repository" (same reference).
pub const GIT_PUSH_ACTION: &str = "codecommit:GitPush";

/// The condition key a git authorization step carries the fully qualified name of
/// the reference it is about in: "codecommit:References — Filters access by Git
/// reference to specified AWS CodeCommit actions" (same reference). A step that is
/// about no particular reference carries no value for it.
pub const REFERENCES_KEY: &str = "codecommit:References";

/// What a push to one reference is refused with when the policies in force do not
/// permit `codecommit:GitPush` for it: the sentence CodeCommit's receive-pack
/// reports, which a git client prints as the reason beside
/// `! [remote rejected] <ref>`.
pub const NO_PERMISSION_TO_PUSH_REFERENCE: &str =
    "You don't have permission to push changes to this branch.";

/// What a push STEP that is about no particular reference is refused with — the ref
/// advertisement a push begins with. It is this transport's own wording: nothing is
/// recorded for it, because a git client prints its own message for the status this
/// refusal carries.
pub const NO_PERMISSION_TO_PUSH: &str =
    "You don't have permission to push changes to this repository.";

/// What a fetch or clone is refused with when the policies in force do not permit
/// `codecommit:GitPull`. This transport's own wording, for the same reason.
pub const NO_PERMISSION_TO_PULL: &str =
    "You don't have permission to read from this repository.";

/// The separator the CodeCommit credential helper writes between the access key id
/// and the session token of the credential it signs with, which is how a temporary
/// credential travels in one user-name field.
const SESSION_TOKEN_SEPARATOR: char = '%';

/// The service principal an IAM service-specific credential for this service's git
/// transport is created for: `CreateServiceSpecificCredential`'s own
/// `ServiceName` for CodeCommit git credentials.
const GIT_CREDENTIAL_SERVICE: &str = "codecommit.amazonaws.com";

/// The principal a git request speaks for, with everything its decision is made
/// from: a snapshot, so the IAM store is read once per request and the per-reference
/// steps are decided without reaching a store again.
#[derive(Debug, Clone, Default)]
pub struct GitPrincipal {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it for the credential
    /// that was presented. Recorded on the audit line of a refusal.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on it, when it carries one whose document
    /// this emulator holds.
    boundary: Option<String>,
    /// Set when nothing the identity-based decision can read bounds this caller: the
    /// account's own credential, which holds no IAM policies, or a principal
    /// carrying an attachment whose document this emulator does not hold (an entry
    /// of AWS's own catalog). Refusing on evidence this emulator does not have would
    /// deny access AWS permits.
    unbounded: bool,
}

impl GitPrincipal {
    /// Whether the policies in force permit `action` on the repository `resource`
    /// for the authorization step about `reference` — `None` for a step that is
    /// about no particular reference.
    ///
    /// IAM's order: an explicit `Deny` in a document that CAN be read refuses the
    /// step whatever else the principal carries; otherwise a grant is needed, which
    /// a caller this gate does not bound has by definition; and a grant is capped by
    /// the permissions boundary, since "a permissions boundary sets the maximum
    /// permissions that an identity-based policy can grant".
    ///
    /// [`REFERENCES_KEY`] is declared as a key this request MODELS whether or not
    /// this step carries a value for it, which is what makes IAM's `IfExists` and
    /// `Null` readings of it decidable: absence means the step carries no reference,
    /// not that the key is unknown here.
    pub fn permits(&self, action: &str, resource: &str, reference: Option<&str>) -> bool {
        let context: Vec<(String, String)> = reference
            .map(|reference| vec![(REFERENCES_KEY.to_string(), reference.to_string())])
            .unwrap_or_default();
        let published = [REFERENCES_KEY];
        // An identity-based document states no `Principal` element, so the decision
        // rests on the action, the resource and the condition context.
        let request = iam::Request::new(action, resource, &[])
            .with_context(&context)
            .with_published_keys(&published);
        let said = iam::identity_decision(&self.documents, &request);
        if said == Decision::Denied {
            return false;
        }
        if !(iam::is_allowed(said, Decision::NotStated) || self.unbounded) {
            return false;
        }
        match &self.boundary {
            None => true,
            Some(boundary) => {
                iam::identity_decision(::std::slice::from_ref(boundary), &request)
                    == Decision::Allowed
            }
        }
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM
    /// service's OWN accessors: the documents in force on it, whether it carries an
    /// attachment whose document this emulator cannot read, and the boundary that
    /// caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of
            // AWS's own catalog this emulator does not carry: a grant it cannot
            // read.
            self.unbounded = self.unbounded || !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal the credentials a git request PRESENTED speak for, or `None` for a
/// request that presented none at all — which is what the transport answers with a
/// challenge rather than with a decision.
pub fn presented_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    authorization: Option<&str>,
) -> Option<GitPrincipal> {
    let presented = presented_credential(authorization?)?;
    Some(principal_of(engine, &presented))
}

/// The credential id a git request presents, as the two clients of this transport
/// present one: the user name of HTTP Basic credentials (a git client), or the
/// access key id in the credential scope of a SigV4 signature (an SDK or a test
/// driving the same endpoint).
///
/// `None` when the header carries neither, which is a request that identifies
/// nobody.
fn presented_credential(authorization: &str) -> Option<String> {
    let value = authorization.trim();
    if let Some((scheme, rest)) = value.split_once(' ') {
        if scheme.eq_ignore_ascii_case("basic") {
            return basic_user_name(rest.trim());
        }
    }
    sigv4_access_key_id(value)
}

/// The user name of HTTP Basic credentials, with the session token the CodeCommit
/// credential helper appends to an access key id removed: `user:password`
/// base64-encoded is what the scheme carries, and the helper writes
/// `<access key id>%<session token>` as the user.
fn basic_user_name(encoded: &str) -> Option<String> {
    let decoded = ::base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let user = text.split_once(':').map_or(text.as_str(), |(user, _)| user);
    let user = user
        .split_once(SESSION_TOKEN_SEPARATOR)
        .map_or(user, |(id, _)| id);
    (!user.is_empty()).then(|| user.to_string())
}

/// The access key id a SigV4 `Authorization` header names, read by the CORE's own
/// reader of that field rather than by a second parse of it, so this transport and
/// the framework resolve one header alike. `None` for a header that names none.
fn sigv4_access_key_id(authorization: &str) -> Option<String> {
    let value = ::http::HeaderValue::from_str(authorization).ok()?;
    let mut headers = ::http::HeaderMap::new();
    headers.insert(::http::header::AUTHORIZATION, value);
    let access_key_id = ::engine::extract_akid(&headers);
    // What the core's reader answers for a header it could not read at all.
    (access_key_id != "default").then_some(access_key_id)
}

/// The principal the presented credential speaks for, and the documents in force on
/// it.
///
/// Resolved in the order the credential forms are minted in: a session of one of the
/// account's roles (the STS registry `sts:AssumeRole` writes), an IAM user's own
/// long-term access key (the map `iam:GetUser` resolves a caller through), and an
/// IAM service-specific credential's generated user name (the registry
/// `CreateServiceSpecificCredential` writes). A credential none of them holds a
/// record for is the account's own published credential, which holds no
/// identity-based policies — the position AWS account root credentials are in.
fn principal_of<Eng: Send + Sync + 'static>(engine: &Eng, presented: &str) -> GitPrincipal {
    let now = SystemTime::now();
    let session = sts_store::service_state(engine).and_then(|sts| {
        let store = sts.lock();
        let session = store.session(presented)?;
        // An expired session credential speaks for nobody: it carries no document,
        // so every step it asks for is refused, and the refusal still names what was
        // presented rather than a blank principal.
        Some((session.identity.arn.clone(), session.is_expired(now)))
    });
    if let Some((arn, true)) = session {
        return GitPrincipal {
            arn: format!("an expired session credential ({arn})"),
            ..GitPrincipal::default()
        };
    }
    let Some(iam) = iam_store::service_state(engine) else {
        // No IAM store on this engine: no principal of it can carry a policy, so
        // there is nothing to judge the credential on.
        return GitPrincipal {
            arn: session.map(|(arn, _)| arn).unwrap_or_default(),
            unbounded: true,
            ..GitPrincipal::default()
        };
    };
    let store = iam.lock();
    let mut principal = match session {
        Some((arn, _)) => GitPrincipal {
            arn,
            ..GitPrincipal::default()
        },
        None => long_term_principal(&store, presented),
    };
    // Which IAM identity the principal's policies are held under is read through the
    // tree's shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&principal.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is
        // addressed by the ARN the account holds it under — which carries its PATH,
        // where the session ARN does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            principal.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&principal.arn).is_some() {
        let user_arn = principal.arn.clone();
        principal.read_policies_of(&store, &user_arn);
    }
    principal
}

/// The principal a credential that names no STS session speaks for: the IAM user
/// that holds the long-term access key, the user that holds the service-specific git
/// credential of that generated user name, or the account's own published credential
/// when the account holds neither.
fn long_term_principal(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    presented: &str,
) -> GitPrincipal {
    if let Some(user) = store
        .access_keys
        .get(presented)
        .and_then(|record| store.users.get(&record.user_key))
    {
        return GitPrincipal {
            arn: user.arn.clone(),
            ..GitPrincipal::default()
        };
    }
    if let Some(user) = git_credential_holder(store, presented) {
        return GitPrincipal {
            arn: user,
            ..GitPrincipal::default()
        };
    }
    // The account's own published credential, and every credential this account
    // holds no record for: no identity-based policy is in force on it.
    GitPrincipal {
        arn: ::service_common::caller_principal::ambient_user_arn(),
        unbounded: true,
        ..GitPrincipal::default()
    }
}

/// The ARN of the IAM user that holds the ACTIVE service-specific credential for
/// this service whose generated user name is `service_user_name`, or `None` when the
/// account holds none.
fn git_credential_holder(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    service_user_name: &str,
) -> Option<String> {
    let held = store.service_specific_credentials.values().find(|credential| {
        credential.service_name == GIT_CREDENTIAL_SERVICE
            && credential.status == iam_store::CREDENTIAL_STATUS_ACTIVE
            && credential.service_user_name.as_deref() == Some(service_user_name)
    })?;
    let user = store.users.get(&iam_store::name_key(&held.user_name))?;
    Some(user.arn.clone())
}
