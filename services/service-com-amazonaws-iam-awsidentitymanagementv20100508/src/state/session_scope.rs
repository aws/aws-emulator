// SPDX-License-Identifier: CC0-1.0

//! What SCOPES a temporary credential down: the session policies one mint passed, the
//! scope-down layers a later call narrowed the session with, and the one reading of what they
//! say about a request.
//!
//! One concern, one file — every service that decides a request signed with a credential this
//! account's STS vended reads the scope through here, so none of them can cap one
//! differently.

use super::*;

/// The session policies ONE mint of a temporary credential passed, which scope the
/// session's permissions down.
///
/// "The resulting session's permissions are the intersection of the role's identity-based
/// policy and the session policies… You cannot use session policies to grant more
/// permissions than those allowed by the identity-based policy of the role that is being
/// assumed" (`AssumeRoleRequest$PolicyArns`, the STS model): what a request signed with the
/// credential may do is what the identity's own policies allow AND what these documents
/// allow, and an explicit `Deny` in any of them refuses whatever a grant says. A managed
/// session policy named by ARN narrows exactly as an inline document does.
///
/// They live beside the role-session record ([`RoleSessionRecord::scope`]) because the
/// credential is the only thing a later request carries, and this store is what the service
/// deciding that request reads the principal out of. The inline document is held exactly as
/// the mint passed it; a managed session policy is held by ARN instead, because what it says
/// is what its DEFAULT version says at the instant of the request, which is read then
/// ([`session_scoping_documents`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionPolicies {
    /// The inline session policy document the mint passed, if any.
    pub inline: Option<String>,
    /// The managed session policies the mint named, by ARN, in the order it named them.
    pub managed_arns: Vec<String>,
    /// The SCOPE-DOWN documents a later call narrowed the session with, in the order they
    /// were appended — each one a document that caps what the credential may do without
    /// granting anything of its own, exactly as [`Self::inline`] does.
    ///
    /// They are kept apart from the inline document because the mint that passed that
    /// document and the calls that appended these are different calls: a credential scoped
    /// down from another one keeps the scope its own mint passed and adds a layer
    /// ("the resulting session's permissions are the intersection of the calling session's
    /// existing permissions and this scope down policy",
    /// `GetScopedCredentialsRequest$Policy`, the STS model), and how many layers one session
    /// carries is a value that model reports back
    /// (`GetScopedCredentialsResponse$ScopedPoliciesCount`).
    pub scope_downs: Vec<String>,
    /// Set when the credential's permissions come from these session policies and from
    /// nothing else, so a mint that passed NONE vends a credential with no permissions at
    /// all: "though the session policy parameters are optional, if you do not pass a policy,
    /// then the resulting federated user session has no permissions"
    /// (`GetFederationToken`, the STS model).
    ///
    /// Clear for a mint whose credential stands for an identity it keeps the permissions of
    /// — a role an `AssumeRole` session was minted for, the IAM user a `GetSessionToken`
    /// session is of — where passing no session policy caps nothing. It is what tells an
    /// EMPTY scope that grants nothing from an empty scope that withholds nothing
    /// ([`ScopingPolicies::decision`]).
    pub none_means_no_permissions: bool,
}

/// The session policy DOCUMENTS in force on a credential, read for one request.
#[derive(Debug, Clone, Default)]
pub struct ScopingPolicies {
    /// Every scoping document the MINT named that could be read, in the order it named them:
    /// its inline session policy and each managed session policy's document. They are read
    /// together, as one scope.
    pub documents: Vec<String>,
    /// Each SCOPE-DOWN document a later call narrowed the session with
    /// ([`SessionPolicies::scope_downs`]), in the order they were appended.
    ///
    /// They are kept apart from [`Self::documents`] because each one caps ON ITS OWN: "the
    /// resulting session's permissions are the intersection of the calling session's existing
    /// permissions and this scope down policy" (`GetScopedCredentialsRequest$Policy`, the STS
    /// model), so a layer can only narrow what the layers before it left — reading them
    /// beside the mint's own documents would let a later layer WIDEN the scope instead.
    pub scope_downs: Vec<String>,
    /// Set when the mint named a managed session policy whose document this emulator does
    /// not hold — an entry of AWS's catalog it does not carry — so the scope may allow
    /// more than [`Self::documents`] shows.
    pub unreadable: bool,
    /// Carried over from the mint ([`SessionPolicies::none_means_no_permissions`]): set when
    /// a scope holding NO document grants nothing rather than withholding nothing.
    pub none_means_no_permissions: bool,
}

impl ScopingPolicies {
    /// What this scope says about one request.
    ///
    /// A session policy CAPS what the identity's own documents grant and grants nothing of
    /// its own: "the resulting session's permissions are the intersection of the role's
    /// identity-based policy and the session policies" (`AssumeRoleRequest$PolicyArns`, the
    /// STS model), and "you cannot use session policies to grant more permissions than
    /// those allowed by the identity-based policy of the role that is being assumed". So
    /// the answer is read exactly as a permissions boundary's is by the gate that asks:
    /// an explicit `Deny` refuses whatever any grant says, a call this does not allow is
    /// refused even where the identity's documents allow it, and a call only this allows is
    /// refused as if it had allowed nothing.
    ///
    /// A credential NO mint scoped — every long-term access key, and every session minted
    /// without session policies — is capped by nothing, which is the position of a scope
    /// that allows the call. A credential scoped by a managed session policy whose document
    /// this emulator does not hold ([`Self::unreadable`]) is in that same position:
    /// refusing on evidence it does not have would deny access AWS permits, while a `Deny`
    /// in a document it CAN read still refuses.
    ///
    /// The ONE exception is a mint whose credential draws its permissions from the session
    /// policies alone ([`Self::none_means_no_permissions`]): passing none there left the
    /// session with no permissions, so an empty scope allows nothing instead of capping
    /// nothing.
    ///
    /// A SCOPE-DOWN layer ([`Self::scope_downs`]) caps on its own, after the mint's own
    /// documents: the request has to be allowed by every layer, so a layer that does not
    /// allow it withholds it however wide the scope under it is.
    ///
    /// The ONE reading of a resolved scope, so this service's gate and every other service
    /// that decides a request signed with such a credential cannot cap one differently.
    ///
    /// For a gate that asks about SEVERAL resources of one request, ask through
    /// [`Self::prepare`] instead: this reading parses the scope's documents each time it is
    /// called, and a request addressing many resources would pay that parse per resource.
    #[must_use]
    pub fn decision(
        &self,
        request: &::service_common::iam_authorization::Request<'_>,
    ) -> ::service_common::iam_authorization::Decision {
        self.prepare().decision(request)
    }

    /// This scope with its documents PARSED ONCE, for a gate that reads it more than once
    /// per request — the form [`::service_common::iam_authorization::prepare_documents`]
    /// exists for.
    #[must_use]
    pub fn prepare(&self) -> PreparedScopingPolicies {
        PreparedScopingPolicies {
            documents: ::service_common::iam_authorization::prepare_documents(&self.documents),
            scope_downs: self
                .scope_downs
                .iter()
                .map(|document| {
                    ::service_common::iam_authorization::prepare_documents(
                        ::std::slice::from_ref(document),
                    )
                })
                .collect(),
            mint_caps_nothing: self.mint_caps_nothing(),
        }
    }

    /// Whether the MINT's own scope caps nothing: a scope holding no document of the mint's —
    /// unless the mint it came from vends its permissions through session policies alone,
    /// where passing none left the credential with no permissions at all
    /// ([`SessionPolicies::none_means_no_permissions`]), which is an absence of grant rather
    /// than a deny — and one naming a managed session policy whose document this emulator
    /// does not hold, where refusing on evidence it does not have would deny access AWS
    /// permits ([`Self::unreadable`]).
    ///
    /// It is the reading of [`Self::documents`] ALONE, which is what the scope-down layers
    /// are intersected WITH ([`PreparedScopingPolicies::decision`]): a layer narrows what the
    /// mint's own scope left, so a mint that capped nothing leaves every layer's own grant
    /// standing rather than erasing it.
    #[must_use]
    pub fn mint_caps_nothing(&self) -> bool {
        (self.documents.is_empty() && !self.none_means_no_permissions) || self.unreadable
    }

    /// Whether this scope caps NOTHING AT ALL, so every reading of it allows the request:
    /// the mint's own scope caps nothing ([`Self::mint_caps_nothing`]) and no scope-down layer
    /// narrows it either.
    ///
    /// A scope carrying a SCOPE-DOWN layer caps whatever that layer does not allow, so it
    /// never caps nothing — a layer this emulator CAN read narrows the credential even where
    /// a managed policy beside it could not be read.
    #[must_use]
    pub fn caps_nothing(&self) -> bool {
        self.scope_downs.is_empty() && self.mint_caps_nothing()
    }
}

/// A resolved scope whose documents are parsed once, for the gate that reads it several
/// times while deciding ONE request — once per resource the request addresses.
///
/// Both readings a gate needs, in the one place the unprepared
/// [`ScopingPolicies::decision`] states them, so neither can drift from it: what the scope
/// says about a request on a NAMED resource ([`Self::decision`]), and whether it allows an
/// action on any resource at all ([`Self::may_allow_action`]), which is the weaker question
/// asked where the resource a statement must cover is not known.
#[derive(Debug, Clone, Default)]
pub struct PreparedScopingPolicies {
    /// Every scoping document that could be read, parsed, in the order the mint named them.
    documents: Vec<::service_common::iam_authorization::PreparedDocument>,
    /// Each scope-down LAYER, parsed, in the order it was appended
    /// ([`ScopingPolicies::scope_downs`]): one set per layer, because each caps on its own.
    scope_downs: Vec<Vec<::service_common::iam_authorization::PreparedDocument>>,
    /// Carried over from [`ScopingPolicies::mint_caps_nothing`]: whether the MINT's own scope
    /// caps nothing, which is what the scope-down layers are intersected with.
    mint_caps_nothing: bool,
}

impl PreparedScopingPolicies {
    /// What this scope says about one request — [`ScopingPolicies::decision`], over the
    /// documents already parsed.
    #[must_use]
    pub fn decision(
        &self,
        request: &::service_common::iam_authorization::Request<'_>,
    ) -> ::service_common::iam_authorization::Decision {
        use ::service_common::iam_authorization::{identity_decision_prepared, Decision};

        let stated = identity_decision_prepared(&self.documents, request);
        let mut decision = if stated != Decision::Denied && self.mint_caps_nothing {
            Decision::Allowed
        } else {
            stated
        };
        if decision == Decision::Denied {
            return decision;
        }
        // Each scope-down layer is an INTERSECTION with what the layers before it left, so a
        // layer that does not allow the request withholds it however wide the scope under it
        // is, and a `Deny` in any layer refuses outright.
        for layer in &self.scope_downs {
            match identity_decision_prepared(layer, request) {
                Decision::Denied => return Decision::Denied,
                Decision::Allowed => {}
                Decision::NotStated => decision = Decision::NotStated,
            }
        }
        decision
    }

    /// Whether this scope allows `action` on ANY resource: the weaker question a gate asks
    /// for a request whose resource it could not read, where a statement scoped to resource
    /// ARNs may well cover whatever the request addresses
    /// ([`::service_common::iam_authorization::identity_may_allow_action`]).
    ///
    /// A scope whose MINT caps nothing allows it, for the reason
    /// [`ScopingPolicies::mint_caps_nothing`] states; every other scope allows it only by
    /// stating an `Allow` of the action, so an EMPTY scope on a credential that draws its
    /// permissions from its session policies alone allows nothing. Each scope-down layer has
    /// to allow it too, which is the same intersection [`Self::decision`] makes.
    #[must_use]
    pub fn may_allow_action(&self, action: &str) -> bool {
        use ::service_common::iam_authorization::identity_may_allow_action;

        if !self.scope_downs.is_empty()
            && !self
                .scope_downs
                .iter()
                .all(|layer| identity_may_allow_action(layer, action))
        {
            return false;
        }
        self.mint_caps_nothing || identity_may_allow_action(&self.documents, action)
    }
}

/// The documents the session policies `policies` stand for, as they read at the instant of
/// the request: the inline document the mint passed, every scope-down document a later call
/// narrowed the session with, and the version of each managed
/// session policy that is in force now ([`ComAmazonawsIamAwsidentitymanagementv20100508State::default_version_document`]).
///
/// The ONE resolution of "what scopes this session down", so this service's own gate and
/// every other service that decides a request signed with such a credential read the same
/// documents. `store` is the account's IAM records, or `None` where there are none to read —
/// in which case every managed session policy is unreadable, while the inline document the
/// mint passed scopes the session whether or not a store is there.
#[must_use]
pub fn session_scoping_documents(
    store: Option<&ComAmazonawsIamAwsidentitymanagementv20100508State>,
    policies: &SessionPolicies,
) -> ScopingPolicies {
    let mut documents: Vec<String> = policies.inline.iter().cloned().collect();
    let mut unreadable = false;
    for policy_arn in &policies.managed_arns {
        match store.and_then(|store| store.default_version_document(policy_arn)) {
            Some(document) => documents.push(document),
            None => unreadable = true,
        }
    }
    ScopingPolicies {
        documents,
        scope_downs: policies.scope_downs.clone(),
        unreadable,
        none_means_no_permissions: policies.none_means_no_permissions,
    }
}
