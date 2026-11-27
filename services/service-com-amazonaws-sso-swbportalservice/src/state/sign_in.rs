// SPDX-License-Identifier: CC0-1.0

//! The sign-in a presented access token names, resolved from the records the rest of the
//! emulator holds.
//!
//! # The rule
//!
//! The IAM Identity Center portal creates nothing. A sign-in session is minted by
//! `CreateToken` in the IAM Identity Center OIDC service, and the account/role assignments
//! are made by an administrator through IAM Identity Center — two events this service does
//! not serve and cannot observe. So this module resolves them where they are RECORDED, in
//! the same shared engine every service of the emulator is mounted on:
//!
//! * an access token names a sign-in exactly when the emulator's own OIDC service issued
//!   it and it is still inside its own lifetime;
//! * WHICH sign-in session it names is the mint's record too, because a sign-in session is
//!   what a whole chain of tokens belongs to rather than any one of them: every token a
//!   rotation or an exchange puts in another's place belongs to the session its
//!   predecessor did, so anything this service scopes to a sign-in rather than to one
//!   token is bound to that identifier ([`SignedIn::sign_in_session`]);
//! * the accounts that sign-in may see, and the roles in each, are the account assignments
//!   IAM Identity Center holds whose grant has completed and whose PRINCIPAL is the
//!   sign-in's own ([`SignInIdentity`]) — a role being the NAME of the permission set the
//!   assignment grants, which is what the access portal offers a user;
//! * an assigned account's display name and email address are the organization's own
//!   records of that account, and nothing when the emulator holds no record of it.
//!
//! Resolution is a pure READ of those stores: presenting a token neither creates a
//! sign-in nor moves anything on, so an assignment an administrator withdraws stops being
//! reported on the very next request and nothing has to propagate it.
//!
//! # What this service still owns
//!
//! Only what no other service can know: that a `Logout` ended a sign-in, and the role
//! credentials this portal minted. Those live in
//! [`ComAmazonawsSsoSwbportalserviceState`], and
//! [`ComAmazonawsSsoSwbportalserviceState::signed_in`] is the ONE place a record held
//! there and a sign-in resolved here are combined — so a token whose session this store
//! ended is never served again by resolving it afresh against the mint that made it.
//!
//! A sign-in session is not a token, though: it is what a whole chain of tokens belongs to,
//! and that chain is the MINT's record. So the one event this service owns is also
//! PROPAGATED to the mint ([`end_minted_sign_in`]), which is what keeps a logged-out
//! sign-in ended when its holder spends a grant the mint still has.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::{Account, ComAmazonawsSsoSwbportalserviceState, Session};

/// The identity store of an IAM Identity Center instance, where the groups a user is a
/// member of live: an assignment made to a GROUP is one of its MEMBERS' assignments, and
/// that membership is the identity store's own record.
use ::service_com_amazonaws_identitystore_awsidentitystore as identity_store;
/// IAM Identity Center's own control plane: the permission sets and the account
/// assignments an administrator made are its records, so they are read where they live.
use ::service_com_amazonaws_ssoadmin_swbexternalservice as identity_center;
/// The organization's records of its member accounts, where an account's display name and
/// email address live.
use ::service_com_amazonaws_organizations_awsorganizationsv20161128 as organizations;
/// IAM Identity Center OIDC: `CreateToken` mints the access token a portal request
/// presents, and its record of that token is what makes the token a sign-in.
use ::service_com_amazonaws_ssooidc_awsssooidcservice as oidc;

/// The sign-in a presented access token names: who it is, which sign-in session the token
/// is one token OF, when it stops being usable, and the accounts (with the roles assigned
/// in each) it may report.
///
/// OWNED rather than borrowed from a store, because a sign-in is assembled from the
/// records of the services that hold the two events behind it, and a handler must not hold
/// two stores' locks at once to read it.
#[derive(Debug, Clone)]
pub struct SignedIn {
    /// Who the sign-in signed in as. Reported by no operation; it is the IAM Identity
    /// Center identity a role credential minted for this sign-in speaks for (see
    /// [`ComAmazonawsSsoSwbportalserviceState::signed_in_subject`]).
    pub subject: String,
    /// WHICH sign-in session the presented token belongs to, as the record that holds the
    /// token's chain names it. Reported by no operation; it is what anything this service
    /// scopes to a sign-in rather than to one token is bound to — the page cursors a
    /// listing hands out ([`super::cursor_scope`]).
    ///
    /// A sign-in session is not a token: it is what a whole chain of tokens belongs to,
    /// so this is the mint's own session identifier for a sign-in the emulator minted
    /// ([`minted_sign_in`]) — stable across the rotation that supersedes an access token
    /// — and the presenting token itself for a sign-in this store was GIVEN, which is
    /// held under that one token and has no chain.
    pub sign_in_session: String,
    /// When the sign-in stops being usable, whether or not anyone logs out.
    pub expires_at: SystemTime,
    /// The accounts this sign-in may see, by account id, ascending — the order a listing
    /// reports them in and the order its page cursor resumes through.
    pub accounts: BTreeMap<String, Account>,
}

impl SignedIn {
    /// Whether a request presenting this sign-in's token is served at `now`.
    pub fn is_usable_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }

    /// The account `account_id` names, when this sign-in may see it. `None` is the
    /// `ResourceNotFoundException` case: an account the presented sign-in is not assigned
    /// to does not exist as far as that sign-in is concerned.
    pub fn account(&self, account_id: &str) -> Option<&Account> {
        self.accounts.get(account_id)
    }
}

/// The sign-in the EMULATOR recorded for `access_token`, outside this service: the token
/// its own `CreateToken` minted, carrying the IAM Identity Center assignments that stand at
/// `now`. `None` when no such token is on record or its own lifetime has run out.
///
/// Called by a handler BEFORE it locks this service's store, so only one store's lock is
/// ever held at a time, and the answer is combined with what this store knows by
/// [`ComAmazonawsSsoSwbportalserviceState::signed_in`].
///
/// Whether a token names a sign-in is a question about the TOKEN alone, so a sign-in whose
/// assignments cannot be read at all — an emulator standing this service up beside the
/// token mint and no IAM Identity Center — is a sign-in with no assigned account, never a
/// refused token.
pub fn federated_sign_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_token: &str,
    now: SystemTime,
) -> Option<SignedIn> {
    let minted = minted_sign_in(engine, access_token, now)?;
    Some(SignedIn {
        subject: minted.subject,
        sign_in_session: minted.sign_in_session,
        expires_at: minted.expires_at,
        accounts: assigned_accounts(engine, minted.identity.as_ref(), now).unwrap_or_default(),
    })
}

/// Ends, in the emulator's own token mint, the sign-in session `access_token` belongs to:
/// every token of that sign-in the mint still holds — the access token itself and the
/// refresh grant that would rotate it into another pair — stops being one it holds.
///
/// This is the other half of `Logout`. Invalidating the sign-in session is what that
/// operation does, and a sign-in session is not a token: it is what a chain of tokens
/// belongs to, and the chain lives in the mint that issued it. A sign-in whose record is
/// ended HERE only (by [`ComAmazonawsSsoSwbportalserviceState::invalidate_session`], keyed
/// by the one token presented) would be usable again the moment its holder spent the
/// refresh token beside it, because the pair that rotation mints is resolved afresh against
/// the mint ([`minted_sign_in`]) and this store holds no record of it — the ended sign-in
/// serving again with no further user authentication. So the mint is told, and the only
/// thing that outlives a logout is what the model says does: the role credentials already
/// minted, for the rest of their own duration.
///
/// Called by the handler AFTER it has resolved the sign-in and released this store's lock,
/// so only one store's lock is ever held at a time. Answers whether the mint held the
/// presented token at all; a sign-in no mint of this emulator issued (one seeded straight
/// into this store) has no tokens there to end.
pub fn end_minted_sign_in<Eng: Send + Sync + 'static>(engine: &Eng, access_token: &str) -> bool {
    let Some(tokens) = oidc::state::service_state(engine) else {
        return false;
    };
    let mut tokens = tokens.lock();
    tokens.end_sign_in_session(access_token)
}

/// The IAM Identity Center PRINCIPAL a sign-in is for, when the record that minted the
/// sign-in names one: the identity-store user, the store that holds it and the instance
/// that holds the store.
///
/// This is what makes the sign-in's view of IAM Identity Center its own. An assignment is
/// made to a principal, so it is this user's assignments — the ones naming the user, and
/// the ones naming a group the identity store records the user as a member of — that the
/// portal reports ([`assigned_accounts`]), and this user that a role credential minted for
/// the sign-in speaks for.
#[derive(Debug, Clone)]
pub struct SignInIdentity {
    /// The `InstanceArn` of the IAM Identity Center instance the identity belongs to.
    pub instance_arn: String,
    /// The `IdentityStoreId` of that instance's identity store.
    pub identity_store_id: String,
    /// The `UserId` of the identity-store user the sign-in is for.
    pub user_id: String,
}

/// What the emulator's own token mint records about the sign-in a presented access token
/// belongs to.
struct MintedSignIn {
    /// The identity the sign-in is attributed to; see [`minted_sign_in`].
    subject: String,
    /// The identity-store principal the mint recorded for the sign-in, when it recorded
    /// one; see [`SignInIdentity`] and [`minted_sign_in`].
    identity: Option<SignInIdentity>,
    /// The mint's identifier for the sign-in session this token is one token OF, the same
    /// for every token of the chain.
    sign_in_session: String,
    /// When the presented token stops being usable.
    expires_at: SystemTime,
}

/// The sign-in the emulator's own token mint made for `access_token`: who it is attributed
/// to, which sign-in session the token belongs to, and when the token stops being usable.
///
/// A sign-in is attributed to the IAM Identity Center identity the MINT recorded for the
/// token. A token minted from a grant that authenticated a user of an identity store
/// carries that user, and the mint keeps it on the token precisely so a later read
/// resolves the same identity the caller was handed — so that user is the sign-in's
/// subject and its principal ([`SignInIdentity`]). A token of a grant that authenticated
/// only a CLIENT records no identity-store user: the approval that would name one is made
/// at the access-portal page, which is no operation of this API or of the OIDC API, so
/// such a sign-in is attributed to the registration the token was ISSUED TO and names no
/// principal. Either attribution is stable for the life of the chain — rotating a refresh
/// token re-issues against the same registration and the same identity — so a credential
/// minted before a refresh speaks for the same subject as one minted after.
///
/// `None` when the OIDC service holds no such token, or holds one whose own lifetime has
/// run out: a token past its lifetime names no sign-in, which is exactly the
/// `UnauthorizedException` case ("an invalid access token in the request").
fn minted_sign_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_token: &str,
    now: SystemTime,
) -> Option<MintedSignIn> {
    let tokens = oidc::state::service_state(engine)?;
    let tokens = tokens.lock();
    let issued = tokens.access_tokens.get(access_token)?;
    if issued.is_expired(now) {
        return None;
    }
    let identity = issued.identity.as_ref().map(|bound| SignInIdentity {
        instance_arn: bound.instance_arn.clone(),
        identity_store_id: bound.identity_store_id.clone(),
        user_id: bound.user_id.clone(),
    });
    Some(MintedSignIn {
        subject: match identity.as_ref() {
            Some(identity) => identity.user_id.clone(),
            None => issued.client_id.clone(),
        },
        identity,
        // The session the token is a token of, which is the SAME identifier on every
        // token of the chain: the pair a refresh rotation mints belongs to the session the
        // pair it superseded belonged to.
        sign_in_session: issued.sign_in_session.clone(),
        expires_at: issued.expires_at(),
    })
}

/// The accounts IAM Identity Center's assignments put in force for `identity` at `now`,
/// with the roles each grants, and the organization's name and email address for each
/// account.
///
/// `None` only when IAM Identity Center's own store is not reachable — an emulator that
/// mounted this service without it — which is no assignment rather than no sign-in (see
/// [`federated_sign_in`]).
///
/// An assignment is in force once the asynchronous operation that granted it has
/// completed: `CreateAccountAssignment` is accepted `IN_PROGRESS` and reaches `SUCCEEDED`
/// when the clock makes it due, so a user's portal does not offer an account the grant has
/// not finished delivering. (An assignment carrying no operation status is in force: IAM
/// Identity Center drops that status record only together with the assignment row itself,
/// so a grant still running always has one.) An assignment whose permission set no longer
/// exists grants no role and is skipped: the role a portal reports IS that permission set's
/// name, and so is one under an INSTANCE the account no longer holds — an instance that is
/// gone grants nothing.
///
/// Which PRINCIPAL an assignment names is a filter whenever the sign-in HAS one
/// ([`SignInIdentity`]): the portal "lists all AWS accounts assigned to the user"
/// (`ListAccounts`) and "all roles that are assigned to the user" (`ListAccountRoles`), so
/// an assignment made to another principal is none of that sign-in's business. What it
/// reports is the assignments naming the user itself and those naming a group of its
/// identity store the user is a member of ([`assigned_principals`]), and only under the
/// instance the identity belongs to. A sign-in the mint recorded no identity-store
/// principal for names no principal to filter by, so what it reports is the assignments
/// the administrator made in the account's directory, all of them. The directory is one:
/// IAM Identity Center allows a single instance per account, so the instances this filter
/// admits are that one instance.
fn assigned_accounts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity: Option<&SignInIdentity>,
    now: SystemTime,
) -> Option<BTreeMap<String, Account>> {
    let mut roles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for granted in granted_roles(engine, identity, now)? {
        roles
            .entry(granted.account_id)
            .or_default()
            .insert(granted.role_name);
    }
    Some(named_accounts(engine, roles))
}

/// One role an in-force account assignment grants a sign-in: the account it is granted in,
/// the role itself, and how long a role session of it runs.
///
/// Both come off the SAME permission-set record, because both are that record's: the role a
/// portal offers is the permission set's name, and the length of the role session it
/// creates is the `SessionDuration` the administrator configured on it.
#[derive(Debug, Clone)]
pub struct GrantedRole {
    /// The account the assignment grants the role in.
    pub account_id: String,
    /// The role, which is the granting permission set's name.
    pub role_name: String,
    /// The granting permission set's `SessionDuration`, as the administrator configured it,
    /// and `None` for a permission set configured with none (the member is optional).
    pub session_duration: Option<String>,
}

/// Every role IAM Identity Center's assignments put in force for `identity` at `now`, in
/// the order the directory holds the assignments.
///
/// The ONE walk of those assignments: both what the portal REPORTS
/// ([`assigned_accounts`]) and how long a credential minted for one of them lasts
/// ([`role_session_duration`]) are read off it, so a role and its session length can never
/// come from different records.
///
/// `None` only when IAM Identity Center's own store is not reachable; see
/// [`assigned_accounts`] for which assignments are in force and for the principal filter.
fn granted_roles<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity: Option<&SignInIdentity>,
    now: SystemTime,
) -> Option<Vec<GrantedRole>> {
    // The identity store is read FIRST and its lock released, so IAM Identity Center's own
    // lock below is the only one held while the assignments are walked.
    let principals = identity.map(|identity| assigned_principals(engine, identity));
    let identity_center = identity_center::state::service_state(engine)?;
    let identity_center = identity_center.lock();
    let mut granted_roles = Vec::new();
    for assignment in &identity_center.account_assignments {
        if !identity_center
            .instances
            .contains_key(&assignment.instance_arn)
        {
            continue;
        }
        if principals
            .as_ref()
            .is_some_and(|principals| !principals.is_assigned_by(assignment))
        {
            continue;
        }
        let granted = identity_center
            .assignment_operations
            .get(&assignment.request_id)
            .is_none_or(|operation| {
                operation.status(now) == identity_center::state::STATUS_SUCCEEDED
            });
        if !granted {
            continue;
        }
        let Some(permission_set) = identity_center
            .permission_sets
            .get(&assignment.permission_set_arn)
        else {
            continue;
        };
        granted_roles.push(GrantedRole {
            account_id: assignment.account_id.clone(),
            role_name: permission_set.name.clone(),
            session_duration: permission_set.session_duration.clone(),
        });
    }
    drop(identity_center);
    Some(granted_roles)
}

/// How long the role session `GetRoleCredentials` creates for `role_name` in `account_id`
/// runs, for the sign-in `access_token` names.
///
/// A role session created through an IAM Identity Center permission set runs for "the
/// duration configured in the permission set" (this service's own `Logout` documentation
/// says so in as many words), so the length is read off the permission set that GRANTS the
/// role — the one whose name the portal reports as that role ([`granted_roles`]) — and
/// never from anything the request carries, which models no duration at all.
///
/// [`super::ROLE_SESSION_DURATION`] is the answer exactly where no configured duration
/// reaches this call: a permission set the administrator configured none on, and a role no
/// permission set of this emulator grants at all (a sign-in seeded straight into this
/// store, or an emulator mounted without IAM Identity Center) — the permission-set default
/// is what such a session runs for.
///
/// A configured value naming no fixed span is treated as none: the span is read with the one
/// ISO 8601 duration reading this project shares, which is also the reading IAM Identity
/// Center validates a `SessionDuration` through before storing it.
///
/// Called BEFORE this service's store is locked, like every other read of a store that is
/// not this one's, so only one store's lock is ever held at a time.
pub fn role_session_duration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_token: &str,
    account_id: &str,
    role_name: &str,
    now: SystemTime,
) -> Duration {
    configured_role_session_duration(engine, access_token, account_id, role_name, now)
        .unwrap_or(super::ROLE_SESSION_DURATION)
}

/// The configured span [`role_session_duration`] reports, or `None` for each way a role
/// session has no configured length of its own.
///
/// The permission set is the one an in-force assignment of the SIGN-IN's own grants, so the
/// principal filter and the grant's completion are asked here exactly as they are asked of
/// the role the portal reports. Two directories holding a permission set of the same name
/// assigned in the same account report ONE role, so the first grant of it the directory
/// holds is the one that names the session's length.
fn configured_role_session_duration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_token: &str,
    account_id: &str,
    role_name: &str,
    now: SystemTime,
) -> Option<Duration> {
    let identity = minted_sign_in(engine, access_token, now)?.identity;
    granted_roles(engine, identity.as_ref(), now)?
        .into_iter()
        .find(|granted| granted.account_id == account_id && granted.role_name == role_name)?
        .session_duration
        .as_deref()
        .and_then(::service_common::iso8601_span::iso8601_span)
}

/// The `PrincipalType` an assignment made to one identity-store USER carries — one of the
/// two values the model's enum admits
/// (`smithy_query(query="shape",shape="PrincipalType")`: `USER | GROUP`).
const PRINCIPAL_TYPE_USER: &str = "USER";

/// The `PrincipalType` an assignment made to an identity-store GROUP carries; see
/// [`PRINCIPAL_TYPE_USER`].
const PRINCIPAL_TYPE_GROUP: &str = "GROUP";

/// The principals an account assignment may name for it to be one of a sign-in's own: the
/// identity-store user itself, and every group of that identity store the user is a member
/// of.
#[derive(Debug)]
struct AssignedPrincipals {
    /// The instance the identity belongs to: an assignment under another instance is made
    /// in another directory, so it names no principal of this one.
    instance_arn: String,
    /// The sign-in's `UserId`, keyed the way the identity store keys an id.
    user: String,
    /// The `GroupId` of every group the identity store records the user as a member of,
    /// keyed the same way.
    groups: BTreeSet<String>,
}

impl AssignedPrincipals {
    /// Whether `assignment` is one of this sign-in's: made under its instance, to the user
    /// itself or to a group the user is a member of.
    ///
    /// A `PrincipalId` is a GUID whose modelled pattern admits either letter case, and the
    /// identity store keys an id by its own one reading of such a value
    /// (`identity_store::state::resource_key`), so the two sides are compared through that
    /// reading rather than byte for byte.
    fn is_assigned_by(&self, assignment: &identity_center::state::AccountAssignmentRecord) -> bool {
        if assignment.instance_arn != self.instance_arn {
            return false;
        }
        let principal = identity_store::state::resource_key(&assignment.principal_id);
        match assignment.principal_type.as_str() {
            PRINCIPAL_TYPE_USER => principal == self.user,
            PRINCIPAL_TYPE_GROUP => self.groups.contains(&principal),
            // A principal type outside the modelled enum names neither a user nor a group
            // of the identity store, so it names no principal of this sign-in.
            _ => false,
        }
    }
}

/// Every principal an assignment may name for it to be `identity`'s own.
///
/// An administrator assigns account access to a user or to a GROUP the user belongs to
/// ("Assign User Access", `ListAccounts`), and an assignment to a group is an assignment
/// of each of its members — so the groups are resolved from the identity store that HOLDS
/// the membership, through that store's own reader for the member side of it
/// (`IdentityStoreData::groups_of_member`), exactly as the assignments are read where they
/// live. A store this emulator holds no record of (one nothing was ever created in, or an
/// emulator standing this service up without an identity store) records no membership,
/// which is a user assigned through no group rather than a sign-in with no principal: the
/// assignments naming the user itself are still its own.
///
/// LOCK ORDER: takes the identity store's state lock and nothing else, and holds no other
/// lock while it does.
fn assigned_principals<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity: &SignInIdentity,
) -> AssignedPrincipals {
    let mut groups = BTreeSet::new();
    if let Some(stores) = identity_store::state::service_state(engine) {
        let stores = stores.lock();
        if let Some(store) = stores.store(&identity.identity_store_id) {
            groups = store.groups_of_member(&identity.user_id);
        }
    }
    AssignedPrincipals {
        instance_arn: identity.instance_arn.clone(),
        user: identity_store::state::resource_key(&identity.user_id),
        groups,
    }
}

/// `roles` as the accounts a listing reports: each with the display name and email address
/// the organization holds for it.
///
/// `AccountInfo$accountName` and `$emailAddress` are an account's own data — the portal
/// shows the account name and email the organization carries — so they are read from the
/// organization's records and left ABSENT for an account the emulator holds no record of,
/// rather than derived from the account id. Both members are optional in the model.
fn named_accounts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    roles: BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, Account> {
    let organization = organizations::state::service_state(engine);
    let organization = organization.as_ref().map(|state| state.lock());
    roles
        .into_iter()
        .map(|(account_id, roles)| {
            let member = organization
                .as_ref()
                .and_then(|organization| organization.accounts.get(&account_id));
            let account = Account {
                account_id: account_id.clone(),
                account_name: member.map(|member| member.name.clone()),
                email_address: member.map(|member| member.email.clone()),
                roles,
            };
            (account_id, account)
        })
        .collect()
}

impl ComAmazonawsSsoSwbportalserviceState {
    /// The sign-in `access_token` names at `now`, or `None` for every way it can name
    /// none — unknown token, logged-out sign-in, expired sign-in — because the API
    /// distinguishes none of them. Word the refusal with
    /// [`super::unauthorized_session`], never with a message of the operation's own.
    ///
    /// `federated` is what [`federated_sign_in`] resolved for the same token and instant.
    /// The two sources are combined here and nowhere else:
    ///
    /// 1. a record THIS store still holds — one the clock has not made spent — decides
    ///    whether the sign-in is still usable, because ending a sign-in is what `Logout`
    ///    writes here and nothing else records it;
    /// 2. the sign-in the emulator's own mint made is then reported as it stands NOW, so
    ///    an assignment made or withdrawn since the token was first presented is in force;
    /// 3. a sign-in this store was GIVEN (one seeded through
    ///    [`Self::put_session`](ComAmazonawsSsoSwbportalserviceState::put_session))
    ///    reports its own assignments — the accounts, and in each the roles assigned to
    ///    its subject — including an assignment whose account this store holds no record
    ///    of: the assignment is what makes the account one this sign-in may see, and an
    ///    account no record is held for simply has no name and no email to report.
    ///
    /// A record this store merely REMEMBERED of a minted sign-in ([`Session::federated`])
    /// never answers in step 3: the mint's record is what says whether such a token still
    /// names a sign-in, so a token the mint no longer holds — one a refresh superseded, or
    /// one past its own lifetime — is refused at that instant rather than served from a
    /// frozen copy.
    ///
    /// A record the CLOCK has made spent is read as absent
    /// ([`Self::live_sign_in_record`]): it is a record whose reclaim this store owes at
    /// that instant, so it answers nothing in step 1 either. That is what makes this
    /// question independent of when the reclaim is materialised
    /// ([`super::end_expired_credentials`]) — the same token resolves to the same sign-in
    /// whether that sweep has already run or is still outstanding, so a caller asking
    /// before a handler has swept (the service's authentication gate) and the handler
    /// behind it cannot reach different verdicts.
    pub fn signed_in(
        &self,
        access_token: &str,
        federated: Option<&SignedIn>,
        now: SystemTime,
    ) -> Option<SignedIn> {
        let recorded = self.live_sign_in_record(access_token, now);
        let resolved = match recorded {
            Some(session) if !session.is_usable_at(now) => None,
            _ => match federated.filter(|sign_in| sign_in.is_usable_at(now)) {
                Some(sign_in) => Some(sign_in.clone()),
                None => recorded
                    .filter(|session| !session.federated)
                    .map(|session| self.seeded_sign_in(access_token, session)),
            },
        };
        if resolved.is_none() {
            // The access attempt real IAM Identity Center would record. The token is
            // `@sensitive` and the subject is personal data, so neither is recorded:
            // the fact of the refusal is what an assertion needs.
            tracing::debug!(
                target: "sso.portal",
                "a presented access token named no usable sign-in session"
            );
        }
        resolved
    }

    /// A sign-in held in THIS store, with the accounts its own assignments name and, in
    /// each, the roles those assignments grant its subject.
    ///
    /// Such a sign-in is the record held under the ONE token it was seeded with — no mint
    /// elsewhere holds a chain for it — so that token is what names the sign-in session
    /// ([`SignedIn::sign_in_session`]).
    ///
    /// EVERY account the sign-in's assignments name is reported. What makes an account one
    /// this sign-in may see is the assignment, and an assignment names the account by the
    /// id that is the one member `AccountInfo` always carries; a record of the ACCOUNT
    /// itself is only where the account's OWN data lives. So an account this store holds no
    /// record of is reported with no name and no email address
    /// ([`Account::account_name`]) — exactly what the other source of a sign-in does for an
    /// account the organization's records do not carry ([`named_accounts`]) — rather than
    /// dropped, which would turn an assignment an administrator made into an account that
    /// does not exist.
    fn seeded_sign_in(&self, access_token: &str, session: &Session) -> SignedIn {
        SignedIn {
            subject: session.subject.clone(),
            sign_in_session: access_token.to_string(),
            expires_at: session.expires_at,
            accounts: session
                .assignments
                .iter()
                .map(|(account_id, roles)| {
                    let record = self.accounts.get(account_id);
                    (
                        account_id.clone(),
                        Account {
                            account_id: account_id.clone(),
                            account_name: record.and_then(|record| record.account_name.clone()),
                            email_address: record.and_then(|record| record.email_address.clone()),
                            roles: roles.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Record `signed_in` in this store, if it holds no record of `access_token` yet.
    ///
    /// A sign-in the emulator's own mint made is resolved from that mint on every request,
    /// so this store needs a record of it only once something it alone knows happens to
    /// that sign-in: a `Logout` ends it ([`Self::invalidate_session`]), or a role
    /// credential is minted for it and the identity that credential speaks for has to stay
    /// resolvable for the credential's own duration
    /// ([`Self::signed_in_subject`]). Both are WRITES their operation makes anyway.
    ///
    /// A record already here is left exactly as it is: it is this store's own answer about
    /// a sign-in it has already been told something about.
    pub fn remember_sign_in(&mut self, access_token: &str, signed_in: &SignedIn) {
        let was_absent = !self.sessions.contains_key(access_token);
        self.sessions
            .entry(access_token.to_string())
            .or_insert_with(|| Session {
                subject: signed_in.subject.clone(),
                assignments: signed_in
                    .accounts
                    .iter()
                    .map(|(account_id, account)| (account_id.clone(), account.roles.clone()))
                    .collect(),
                active: true,
                expires_at: signed_in.expires_at,
                // A COPY of a sign-in minted elsewhere: it is kept for the `Logout` that
                // may end it and for the credential whose identity it resolves, and never
                // answers as a sign-in of its own (see [`Self::signed_in`]).
                federated: true,
            });
        if was_absent {
            // The one write this adds to an operation that was already writing. Neither
            // the token (`@sensitive`) nor the subject (personal data) is recorded.
            tracing::debug!(
                target: "sso.portal",
                "a sign-in minted elsewhere in the emulator was recorded by the portal"
            );
        }
    }
}
