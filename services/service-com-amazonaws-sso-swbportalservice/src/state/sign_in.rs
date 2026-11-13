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
//! * the accounts that sign-in may see, and the roles in each, are the account assignments
//!   IAM Identity Center holds whose grant has completed — a role being the NAME of the
//!   permission set the assignment grants, which is what the access portal offers a user;
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

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use super::{Account, ComAmazonawsSsoSwbportalserviceState, Session};

/// IAM Identity Center's own control plane: the permission sets and the account
/// assignments an administrator made are its records, so they are read where they live.
use ::service_com_amazonaws_ssoadmin_swbexternalservice as identity_center;
/// The organization's records of its member accounts, where an account's display name and
/// email address live.
use ::service_com_amazonaws_organizations_awsorganizationsv20161128 as organizations;
/// IAM Identity Center OIDC: `CreateToken` mints the access token a portal request
/// presents, and its record of that token is what makes the token a sign-in.
use ::service_com_amazonaws_ssooidc_awsssooidcservice as oidc;

/// The sign-in a presented access token names: who it is, when it stops being usable, and
/// the accounts (with the roles assigned in each) it may report.
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
    let (subject, expires_at) = minted_sign_in(engine, access_token, now)?;
    Some(SignedIn {
        subject,
        expires_at,
        accounts: assigned_accounts(engine, now).unwrap_or_default(),
    })
}

/// The sign-in the emulator's own token mint made for `access_token`: who it is attributed
/// to, and when the token stops being usable.
///
/// IAM Identity Center attributes a sign-in to the identity-store user who approved it at
/// the access portal, and that approval is made at the portal page rather than through any
/// operation of this API or of the OIDC API, so the registration the token was ISSUED TO
/// is the identity the emulator records for it. It is stable for the life of that
/// registration — rotating a refresh token re-issues against the same `clientId` — so a
/// credential minted before a refresh speaks for the same subject as one minted after.
///
/// `None` when the OIDC service holds no such token, or holds one whose own lifetime has
/// run out: a token past its lifetime names no sign-in, which is exactly the
/// `UnauthorizedException` case ("an invalid access token in the request").
fn minted_sign_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_token: &str,
    now: SystemTime,
) -> Option<(String, SystemTime)> {
    let tokens = oidc::state::service_state(engine)?;
    let tokens = tokens.lock();
    let issued = tokens.access_tokens.get(access_token)?;
    (!issued.is_expired(now)).then(|| (issued.client_id.clone(), issued.expires_at()))
}

/// The accounts IAM Identity Center's assignments put in force at `now`, with the roles
/// each grants, and the organization's name and email address for each account.
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
/// Which PRINCIPAL an assignment names is not a filter here. The portal reports the
/// accounts assigned to the signed-in user, and the emulator records no mapping from a
/// device-authorization sign-in to one identity-store principal — the approval that would
/// name one is made outside every API the emulator serves — so what a sign-in reports is
/// the assignments the administrator made in the account's directory, all of them. The
/// directory is one: IAM Identity Center allows a single instance per account, so the
/// instances this filter admits are that one instance.
fn assigned_accounts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
) -> Option<BTreeMap<String, Account>> {
    let identity_center = identity_center::state::service_state(engine)?;
    let identity_center = identity_center.lock();
    let mut roles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for assignment in &identity_center.account_assignments {
        if !identity_center
            .instances
            .contains_key(&assignment.instance_arn)
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
        roles
            .entry(assignment.account_id.clone())
            .or_default()
            .insert(permission_set.name.clone());
    }
    drop(identity_center);
    Some(named_accounts(engine, roles))
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
    /// 1. a record THIS store holds decides whether the sign-in is still usable, because
    ///    ending a sign-in is what `Logout` writes here and nothing else records it;
    /// 2. the sign-in the emulator's own mint made is then reported as it stands NOW, so
    ///    an assignment made or withdrawn since the token was first presented is in force;
    /// 3. a sign-in this store was GIVEN (one seeded through
    ///    [`Self::put_session`](ComAmazonawsSsoSwbportalserviceState::put_session))
    ///    reports its own assignments — the accounts, and in each the roles assigned to
    ///    its subject — skipping an assignment whose account this store holds no record
    ///    of, since there is nothing to report for it.
    ///
    /// A record this store merely REMEMBERED of a minted sign-in ([`Session::federated`])
    /// never answers in step 3: the mint's record is what says whether such a token still
    /// names a sign-in, so a token the mint no longer holds — one a refresh superseded, or
    /// one past its own lifetime — is refused at that instant rather than served from a
    /// frozen copy.
    pub fn signed_in(
        &self,
        access_token: &str,
        federated: Option<&SignedIn>,
        now: SystemTime,
    ) -> Option<SignedIn> {
        let recorded = self.sessions.get(access_token);
        let resolved = match recorded {
            Some(session) if !session.is_usable_at(now) => None,
            _ => match federated.filter(|sign_in| sign_in.is_usable_at(now)) {
                Some(sign_in) => Some(sign_in.clone()),
                None => recorded
                    .filter(|session| !session.federated)
                    .map(|session| self.seeded_sign_in(session)),
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
    fn seeded_sign_in(&self, session: &Session) -> SignedIn {
        SignedIn {
            subject: session.subject.clone(),
            expires_at: session.expires_at,
            accounts: session
                .assignments
                .iter()
                .filter_map(|(account_id, roles)| {
                    let record = self.accounts.get(account_id)?;
                    Some((
                        account_id.clone(),
                        Account {
                            account_id: account_id.clone(),
                            account_name: record.account_name.clone(),
                            email_address: record.email_address.clone(),
                            roles: roles.clone(),
                        },
                    ))
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
