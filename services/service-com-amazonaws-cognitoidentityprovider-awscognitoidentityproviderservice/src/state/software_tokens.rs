// SPDX-License-Identifier: CC0-1.0

//! The software token (TOTP authenticator) a user of a pool sets up as an MFA
//! factor: the private key an association mints for it, the form that key is
//! handed out in, the one-time password a registration presents from it, and
//! which user a call that carries one of the two self-service credentials is
//! for.

use super::*;

/// How many bytes of key material one software token's private key carries: 20
/// bytes, spelled as 32 base32 characters (5 bits each), which satisfies the
/// modelled `SecretCodeType` (length 16.., pattern `^[A-Za-z0-9]+$`).
///
/// It is the shared-secret length RFC 4226 (HOTP, which RFC 6238's TOTP extends)
/// recommends for a key: "it is recommended that they be 160 bits".
const SOFTWARE_TOKEN_KEY_BYTES: usize = 20;

/// How long one time step of a software token's one-time password lasts.
///
/// RFC 6238 states this as the default time step — "we RECOMMEND a default
/// time-step size of 30 seconds" — which is what an authenticator app
/// provisioned with a shared secret alone counts in.
const SOFTWARE_TOKEN_TIME_STEP: Duration = Duration::from_secs(30);

/// How many digits one presented code carries: `SoftwareTokenMFAUserCodeType` is
/// "a TOTP that the user generated in their configured authenticator app"
/// (`VerifySoftwareToken.UserCode` in this service's model), length 6..6 of
/// `^[0-9]+$`.
const SOFTWARE_TOKEN_CODE_DIGITS: u32 = 6;

/// How many time steps either side of the current one a presented code is still
/// accepted at: the resynchronisation allowance RFC 6238 states — "we RECOMMEND
/// that at most one time step is allowed as the network delay" — which is what
/// keeps a code read from the authenticator a moment before the step rolled over
/// from being rejected.
const SOFTWARE_TOKEN_DRIFT_STEPS: u64 = 1;

/// The message a software-token call owes a request that carries BOTH of the two
/// credentials it may be authorized with.
///
/// Both `AccessToken` and `Session` are optional in the model and each
/// identifies the user on its own — "you can provide either an access token or a
/// session ID in the request" (`AssociateSoftwareToken.AccessToken` in this
/// service's model) — so a request carrying both names the user twice over: the
/// request's parameters are what is wrong with it, which is what
/// `InvalidParameterException` reports ("thrown when the Amazon Cognito service
/// encounters an invalid parameter"), and the sentence names no value from the
/// request. A request carrying NEITHER presents no credential at all, which is an
/// authorization failure rather than a request-shape complaint
/// ([`NO_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE`]).
pub const ONE_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE: &str =
    "Invalid request. You can provide either an access token or a session ID in the request.";

/// The message a software-token call owes a request that carries NEITHER of the
/// two credentials it may be authorized with.
///
/// Such a call "can [be] authorize[d] ... with either the user's access token, or
/// a session string from a challenge response" (`AssociateSoftwareToken` in this
/// service's model), so the access token is the credential it is authorized with
/// unless a session stands in for it, and a request that carries neither presents
/// no access token — the same rejection of the credential itself that an access
/// token naming no signed-in user gets ([`TokenRejection::NotAnAccessToken`]),
/// reported as the authorization failure it is. The sentence names no value from
/// the request.
pub const NO_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE: &str = "Invalid Access Token";

/// A fresh `SecretCode`: "a unique generated shared secret code that is used by
/// the TOTP algorithm to generate a one-time code"
/// (`AssociateSoftwareToken.SecretCode` in this service's model).
///
/// Spelled in base32, the encoding an authenticator app provisioned with a shared
/// secret reads a key in, and the one [`User::software_token_step_of`] reads it
/// back from, so the code a user's app shows is computed over the same key this
/// service stored.
///
/// Nothing in a service may construct a secret itself, so the material is minted
/// by the one component that generates them — the emulator's credential store —
/// and the record is then removed from it again: a TOTP key authenticates its
/// holder through a one-time password of this service's own checking, never to
/// the emulator over SigV4, so the pair must not stay in the credential store.
///
/// `None` when the store minted no material, when the minted credential could not
/// be removed from the store again — a credential the emulator would still accept
/// as a request signature is not the private key this operation promises, so it
/// is refused rather than handed out, and the failed removal is logged — or when
/// the material is shorter than a key. Either way the caller refuses rather than
/// reporting a key nothing minted.
pub fn mint_software_token_secret(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    // The key is not a credential of this emulator; see above.
    if let Err(error) = store.delete(&minted.id) {
        ::tracing::warn!(
            access_key_id = %minted.id,
            error = ?error,
            "a credential minted for a software token key could not be deleted"
        );
        return None;
    }
    let material: Vec<u8> = secret?.bytes().take(SOFTWARE_TOKEN_KEY_BYTES).collect();
    (material.len() == SOFTWARE_TOKEN_KEY_BYTES)
        .then(|| ::data_encoding::BASE32_NOPAD.encode(&material))
}

/// Whether an authenticator provisioned with `secret_code` emits `user_code`
/// around `at`, and which time step's one-time password it is.
///
/// RFC 6238's TOTP over RFC 4226's HOTP, computed by the `totp-lite` crate rather
/// than here: SHA-1 is the algorithm an authenticator app uses when the key it was
/// provisioned with names none, and the code has to be byte-identical to the one
/// that app shows. Decided against the steps within [`SOFTWARE_TOKEN_DRIFT_STEPS`]
/// of the one `at` falls in, the EARLIEST matching step first, so a code is
/// attributed to the step it was generated in. A key that is not base32, or an
/// instant no counter counts from, emits nothing.
fn software_token_emitting_step(
    secret_code: &str,
    user_code: &str,
    at: SystemTime,
) -> Option<u64> {
    let key = software_token_key(secret_code)?;
    let step = software_token_time_step(at)?;
    (step.saturating_sub(SOFTWARE_TOKEN_DRIFT_STEPS)
        ..=step.saturating_add(SOFTWARE_TOKEN_DRIFT_STEPS))
        .find(|step| software_token_code(&key, *step) == user_code)
}

/// The key material a `SecretCode` spells, or `None` for text that is not base32
/// at all — the encoding [`mint_software_token_secret`] writes without padding and
/// an authenticator app may be handed with it.
fn software_token_key(secret_code: &str) -> Option<Vec<u8>> {
    let spelled = secret_code.trim_end_matches('=');
    ::data_encoding::BASE32_NOPAD
        .decode(spelled.as_bytes())
        .ok()
        .filter(|key| !key.is_empty())
}

/// The time step `at` falls in: the TOTP counter, `floor(unix seconds / time
/// step)`. `None` for an instant before the UNIX epoch, which no counter counts
/// from.
fn software_token_time_step(at: SystemTime) -> Option<u64> {
    let since_epoch = at.duration_since(UNIX_EPOCH).ok()?;
    Some(since_epoch.as_secs() / SOFTWARE_TOKEN_TIME_STEP.as_secs())
}

/// The one-time password an authenticator holding `key` emits for the time step
/// `step`.
fn software_token_code(key: &[u8], step: u64) -> String {
    ::totp_lite::totp_custom::<::totp_lite::Sha1>(
        SOFTWARE_TOKEN_TIME_STEP.as_secs(),
        SOFTWARE_TOKEN_CODE_DIGITS,
        key,
        step.saturating_mul(SOFTWARE_TOKEN_TIME_STEP.as_secs()),
    )
}

/// The private key of one user's software token, as the association that minted it
/// spelled it ([`mint_software_token_secret`]).
///
/// `SecretCodeType` is `@sensitive` in this service's model, so the key is held in
/// a type of its own whose rendering carries none of it (the
/// [`::std::fmt::Debug`] impl below): the stored user derives `Debug`, and key
/// material that cannot be rendered cannot reach a log or a trace through it.
#[derive(Clone, PartialEq, Eq)]
pub struct SoftwareTokenKey(String);

/// Renders that this user holds a key and nothing of the key itself: private key
/// material is never logged or traced, so it is not in this rendering either.
impl ::std::fmt::Debug for SoftwareTokenKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("SoftwareTokenKey")
            .finish_non_exhaustive()
    }
}

impl SoftwareTokenKey {
    /// The key the `SecretCode` `secret_code` spells.
    #[must_use]
    pub fn spelled(secret_code: &str) -> Self {
        Self(secret_code.to_string())
    }

    /// Whether the authenticator provisioned with this key emits `user_code`
    /// around `at` ([`software_token_emitting_step`]).
    #[must_use]
    pub fn emits(&self, user_code: &str, at: SystemTime) -> bool {
        self.emitting_step(user_code, at).is_some()
    }

    /// The time step whose one-time password `user_code` is, for the
    /// authenticator provisioned with this key as of `at`
    /// ([`software_token_emitting_step`]).
    #[must_use]
    pub fn emitting_step(&self, user_code: &str, at: SystemTime) -> Option<u64> {
        software_token_emitting_step(&self.0, user_code, at)
    }
}

/// Which user a software-token call is for, and the challenge session that
/// authorized it, if any ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::software_token_caller`]).
#[derive(Debug, Clone)]
pub struct SoftwareTokenCaller {
    pub user_pool_id: String,
    /// The user as STORED, which a case-insensitive pool may case differently
    /// from the name the sign-in that issued the credential used.
    pub username: String,
    /// The stored challenge session a `Session`-authorized call arrived on, and
    /// `None` for one an access token authorized: only the session flow carries
    /// a session onward, so only it has one to carry.
    pub session: Option<AuthSession>,
}

/// Why a software-token call identifies no user to act for
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::software_token_caller`]).
/// The operation that asked maps each one to a modelled error IT declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoftwareTokenRefusal {
    /// The request carries NEITHER credential, so it presents nothing this call
    /// could be authorized with
    /// ([`NO_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE`]).
    NoCredential,
    /// The request carries BOTH credentials, which names the user twice over
    /// ([`ONE_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE`]).
    BothCredentials,
    /// The `AccessToken` identifies no signed-in user, for the reason the
    /// rejection names.
    AccessToken(TokenRejection),
    /// The `Session` resolves a stored session whose validity window has already
    /// closed ([`SESSION_EXPIRED_MESSAGE`]).
    SessionExpired,
    /// The `Session` resolves a stored session an earlier challenge response
    /// already answered ([`SESSION_ALREADY_USED_MESSAGE`]).
    SessionAlreadyUsed,
    /// The `Session` resolves no session of this request: a value this emulator
    /// never handed out, one a call this operation's `Session` does not come
    /// from handed out, or one started for a user its pool no longer holds
    /// ([`SESSION_NOT_FOR_THIS_REQUEST_MESSAGE`]).
    SessionNotForThisRequest,
}

/// What presenting a one-time password for a user's software token did
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::register_software_token`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftwareTokenRegistration {
    /// The code is one the associated private key emits around now, so the
    /// authenticator is registered and TOTP MFA can be activated for this user.
    Registered,
    /// The user has associated NO private key, so it holds no software token for
    /// a presented code to register: there is nothing to verify rather than
    /// something that did not match
    /// ([`NO_SOFTWARE_TOKEN_MESSAGE`]).
    NotAssociated,
    /// The code is none the associated key emits.
    CodeMismatch,
    /// The user the credential named is no longer held, so nothing was
    /// registered.
    UserGone,
}

/// The message a software-token call owes a user that has associated no private
/// key at all.
///
/// A setup "begins ... with a unique private key that Amazon Cognito generates
/// and returns in the API response" (`AssociateSoftwareToken` in this service's
/// model), so a user no association minted a key for holds no software token:
/// there is no TOTP factor of that user for a presented code to be checked
/// against, which is what `SoftwareTokenMFANotFoundException` reports rather
/// than a code that failed to match. The sentence names no value from the
/// request: it states the factor that does not exist and the user it does not
/// exist for, and nothing of either.
pub const NO_SOFTWARE_TOKEN_MESSAGE: &str = "Software Token MFA does not exist for the user.";

/// The message a DELETE of a user's software token owes a user that holds none
/// for it to take away ([`User::holds_software_token`]).
///
/// The call "deletes a user's registered time-based one-time password (TOTP)
/// multi-factor authentication (MFA) factor" (`AdminDeleteSoftwareToken` in this
/// service's model), so a user that holds no such factor names no resource for
/// the call to act on: it is refused as the missing resource it is rather than
/// reported as a deletion that happened. The sentence states the factor that is
/// not configured and the user it is not configured for, and no value of either
/// the request or this emulator's state appears in it.
pub const SOFTWARE_TOKEN_MFA_NOT_CONFIGURED_MESSAGE: &str =
    "Software token MFA has not been configured for this user.";

impl User {
    /// The time step whose one-time password `user_code` is, for the
    /// authenticator this user associated a private key for as of `at`, and
    /// `None` for a code that key emits in no step of the window — a user that
    /// has associated no key emits none at all.
    pub fn software_token_step_of(&self, user_code: &str, at: SystemTime) -> Option<u64> {
        self.software_token_key
            .as_ref()
            .and_then(|key| key.emitting_step(user_code, at))
    }

    /// Whether the one-time password of the time step `step` is one this user's
    /// software token has already SPENT ([`User::software_token_spent_step`]):
    /// the step recorded as spent and every step before it, since a password
    /// older than the last one accepted is at least as used up as that one.
    pub fn software_token_step_spent(&self, step: u64) -> bool {
        self.software_token_spent_step
            .is_some_and(|spent| step <= spent)
    }

    /// Record the one-time password of the time step `step` as SPENT by this
    /// user's software token, keeping the LATEST step spent
    /// ([`User::software_token_spent_step`]).
    pub fn spend_software_token_step(&mut self, step: u64) {
        self.software_token_spent_step = Some(match self.software_token_spent_step {
            Some(spent) => spent.max(step),
            None => step,
        });
    }

    /// Whether this user holds a software token at all: a registered
    /// authenticator, the private key of a setup that got that far, or
    /// `SOFTWARE_TOKEN_MFA` among the factors it has activated.
    pub fn holds_software_token(&self) -> bool {
        self.software_token_registered
            || self.software_token_key.is_some()
            || self
                .mfa_setting_list
                .iter()
                .any(|setting| setting == SOFTWARE_TOKEN_MFA_SETTING)
    }

    /// Delete the software token this user set up, as of `now`, reporting
    /// whether it held one at all ([`User::holds_software_token`]).
    ///
    /// Everything the factor consists of goes, because "after this operation,
    /// the user can no longer sign in with TOTP MFA, and can register a new TOTP
    /// factor with `AssociateSoftwareToken`" (`AdminDeleteSoftwareToken` in this
    /// service's model): the registration that is the delivery configuration
    /// TOTP MFA is delivered with ([`User::holds_mfa_delivery_config`]), the
    /// private key a presented one-time password is checked against — a new
    /// setup mints its own ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::associate_software_token`]) —
    /// and the time step that key spent
    /// ([`User::software_token_spent_step`]), which belongs to a key
    /// nothing reads from any more.
    ///
    /// A factor the user can no longer receive is not one it has activated, so
    /// `SOFTWARE_TOKEN_MFA` leaves `UserMFASettingList` exactly as deactivating
    /// it does ([`User::set_mfa_preference`]) — and a factor that is not
    /// activated is not one the user prefers, so a preference for it goes with
    /// it. A user that holds no software token is left untouched, its
    /// `UserLastModifiedDate` included: there was nothing to delete, which the
    /// `false` this reports is what the caller is refused for
    /// ([`SOFTWARE_TOKEN_MFA_NOT_CONFIGURED_MESSAGE`]).
    pub fn delete_software_token(&mut self, now: SystemTime) -> bool {
        if !self.holds_software_token() {
            return false;
        }
        self.software_token_registered = false;
        self.software_token_key = None;
        self.software_token_spent_step = None;
        self.mfa_setting_list
            .retain(|setting| setting != SOFTWARE_TOKEN_MFA_SETTING);
        if self.preferred_mfa_setting.as_deref() == Some(SOFTWARE_TOKEN_MFA_SETTING) {
            self.preferred_mfa_setting = None;
        }
        // The user record itself changed: it holds one fewer MFA delivery
        // configuration than before.
        self.user_last_modified_date = now;
        true
    }
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The stored user a software-token call is for, as of `now`, or why the
    /// credentials it carries identify none.
    ///
    /// The two credentials are alternatives — "you can authorize an
    /// `AssociateSoftwareToken` request with either the user's access token, or a
    /// session string from a challenge response that you received from Amazon
    /// Cognito" (`AssociateSoftwareToken` in this service's model) — so exactly
    /// one of them is what names the user: an access token through the sign-in
    /// that issued it ([`Self::signed_in_user`]), and a session through the
    /// challenge it maintains the state of ([`Self::session`]). A session
    /// identifies the user whose sign-in started it whatever challenge that
    /// sign-in is waiting on; which challenge it is decides what ANSWERS the
    /// challenge, not who the factor is being set up for.
    ///
    /// Only a session one of the calls in `accepted` handed out resolves a
    /// caller: an operation's `Session` names the call the session it takes
    /// comes from ([`SESSIONS_OF_A_SIGN_IN`] and
    /// [`SESSIONS_OF_AN_ASSOCIATION`]), so a live session of a different
    /// exchange is no session of this request whoever it names.
    pub fn software_token_caller(
        &mut self,
        access_token: Option<&str>,
        session: Option<&str>,
        accepted: &[SessionIssuer],
        now: SystemTime,
    ) -> Result<SoftwareTokenCaller, SoftwareTokenRefusal> {
        match (access_token, session) {
            (Some(access_token), None) => {
                let (pool, user) = self
                    .signed_in_user(access_token, now)
                    .map_err(SoftwareTokenRefusal::AccessToken)?;
                Ok(SoftwareTokenCaller {
                    user_pool_id: pool.id.clone(),
                    username: user.username.clone(),
                    session: None,
                })
            }
            (None, Some(session)) => {
                let stored = match self.session(session, now) {
                    SessionLookup::Found(stored) => stored,
                    SessionLookup::Expired(_) => {
                        return Err(SoftwareTokenRefusal::SessionExpired);
                    }
                    SessionLookup::Spent(_) => {
                        return Err(SoftwareTokenRefusal::SessionAlreadyUsed);
                    }
                    SessionLookup::Unknown => {
                        return Err(SoftwareTokenRefusal::SessionNotForThisRequest);
                    }
                };
                // A session this operation's `Session` does not come from
                // maintains the state of an exchange this call is not part of.
                if !accepted.contains(&stored.issued_by) {
                    return Err(SoftwareTokenRefusal::SessionNotForThisRequest);
                }
                // The user the session was started for may have been deleted
                // since, in which case the session resolves no user of this
                // request either.
                let username = self
                    .pool(&stored.user_pool_id)
                    .and_then(|pool| pool.user(&stored.username))
                    .ok_or(SoftwareTokenRefusal::SessionNotForThisRequest)?
                    .username
                    .clone();
                Ok(SoftwareTokenCaller {
                    user_pool_id: stored.user_pool_id.clone(),
                    username,
                    session: Some(stored),
                })
            }
            (None, None) => Err(SoftwareTokenRefusal::NoCredential),
            (Some(_), Some(_)) => Err(SoftwareTokenRefusal::BothCredentials),
        }
    }

    /// Store `secret_code` as the private key of the software token `caller` is
    /// setting up, answering `false` when the user is no longer held.
    ///
    /// A fresh association REPLACES the key the user was setting up before: each
    /// one begins the setup "with a unique private key that Amazon Cognito
    /// generates and returns in the API response" (`AssociateSoftwareToken` in
    /// this service's model), so the key a presented code is checked against is
    /// the one the LAST association handed out. The registration the user already
    /// holds, if any, is untouched: a restarted setup is registered by verifying a
    /// code from the new key, not by associating it.
    pub fn associate_software_token(
        &mut self,
        caller: &SoftwareTokenCaller,
        secret_code: &str,
    ) -> bool {
        match self
            .pool_mut(&caller.user_pool_id)
            .and_then(|pool| pool.user_mut(&caller.username))
        {
            Some(user) => {
                user.software_token_key = Some(SoftwareTokenKey::spelled(secret_code));
                // The key the one-time passwords are read from is a NEW one, so
                // no step of it has been spent yet: what the user's previous key
                // spent is not spent on this one
                // ([`User::software_token_spent_step`]).
                user.software_token_spent_step = None;
                true
            }
            None => false,
        }
    }

    /// Register the software token `caller` associated a key for, against the
    /// one-time password `user_code` presented as proof the authenticator holds
    /// that key, as of `now`.
    ///
    /// A code the key emits around `now` ([`User::software_token_step_of`])
    /// "marks the user's software token MFA status as verified"
    /// (`VerifySoftwareToken` in this service's model), which is the delivery
    /// configuration TOTP MFA is delivered with
    /// ([`User::holds_mfa_delivery_config`]), so from here the user can activate
    /// `SOFTWARE_TOKEN_MFA`. Any other code registers nothing and leaves the
    /// user exactly as it was, so the setup can be completed with the next code
    /// the authenticator shows, and a user that has associated no key at all
    /// holds no software token to verify ([`SoftwareTokenRegistration`]).
    ///
    /// The presented code is neither refused as SPENT nor spent itself here:
    /// this call proves possession of the key rather than authenticating a
    /// sign-in — it mints no token and moves no sign-in forward — so the one use
    /// a time step's password is good for is the sign-in it completes
    /// ([`User::software_token_spent_step`], which the challenge responses that
    /// DO authenticate with a code write), and the authenticator a setup just
    /// registered can complete the sign-in that asked for the setup. The error a
    /// used-up password earns, `ExpiredCodeException`, is one this operation's
    /// model does not declare
    /// (`smithy_query(query="operation",operation="VerifySoftwareToken")` lists
    /// `CodeMismatchException`, `EnableSoftwareTokenMFAException`,
    /// `SoftwareTokenMFANotFoundException` and the rest, and no expired-code
    /// error), so that refusal belongs to those responses alone.
    pub fn register_software_token(
        &mut self,
        caller: &SoftwareTokenCaller,
        user_code: &str,
        now: SystemTime,
    ) -> SoftwareTokenRegistration {
        let Some(user) = self
            .pool_mut(&caller.user_pool_id)
            .and_then(|pool| pool.user_mut(&caller.username))
        else {
            return SoftwareTokenRegistration::UserGone;
        };
        if user.software_token_key.is_none() {
            return SoftwareTokenRegistration::NotAssociated;
        }
        if user.software_token_step_of(user_code, now).is_none() {
            return SoftwareTokenRegistration::CodeMismatch;
        }
        user.software_token_registered = true;
        // The registration is a change to the user record itself: what the user
        // holds as an MFA delivery configuration is now one more than before.
        user.user_last_modified_date = now;
        SoftwareTokenRegistration::Registered
    }

    /// A session that carries `caller`'s software-token setup FORWARD, for a call
    /// the session flow authorized, and `None` for one an access token did.
    ///
    /// Each call of the setup hands the next one a session: the session of
    /// `AssociateSoftwareToken` is the one "from an `AssociateSoftwareToken`
    /// request" that a verification takes, and the session of a verification is
    /// the one that "satisfies an `MFA_SETUP` challenge"
    /// (`VerifySoftwareToken.Session` in this service's model). It therefore
    /// maintains the state of the SAME challenge for the same user and app client,
    /// and its window is the one the app client configures for a session token
    /// ([`UserPoolClient::auth_session_validity`]) — the window of the session it
    /// continues, for a client this emulator no longer holds.
    ///
    /// The session the call arrived on is left OUTSTANDING: setting a factor up
    /// answers no challenge — the challenge response is what does that
    /// ([`Self::end_session`]) — so the challenge the sign-in is waiting on is
    /// still waiting.
    pub fn continue_software_token_session(
        &mut self,
        caller: &SoftwareTokenCaller,
        issued_by: SessionIssuer,
        now: SystemTime,
    ) -> Option<String> {
        let session = caller.session.clone()?;
        let expires_at = self
            .pool(&session.user_pool_id)
            .and_then(|pool| pool.clients.get(&session.client_id))
            .map_or(session.expires_at, |client| {
                now + client.auth_session_validity()
            });
        Some(self.start_session(
            now,
            AuthSession {
                expires_at,
                // Outstanding: this session has answered nothing yet.
                spent: false,
                // The challenge it maintains the state of is answered by
                // registering a factor, not with a code anything delivered.
                code: None,
                // The call handing it out is what the next call of the setup
                // takes a session from.
                issued_by,
                ..session
            },
        ))
    }
}
