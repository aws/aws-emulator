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

/// The message a software-token call owes a request that does not carry EXACTLY
/// ONE of the two credentials it may be authorized with.
///
/// Both `AccessToken` and `Session` are optional in the model and each
/// identifies the user on its own — "you can provide either an access token or a
/// session ID in the request" (`AssociateSoftwareToken.AccessToken` in this
/// service's model) — so a request carrying neither names no user to set a
/// factor up for, and one carrying both names the user twice over. Either way
/// the request's parameters are what is wrong with it, which is what
/// `InvalidParameterException` reports ("thrown when the Amazon Cognito service
/// encounters an invalid parameter"), and the sentence names no value from the
/// request.
pub const ONE_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE: &str =
    "Invalid request. You can provide either an access token or a session ID in the request.";

/// A fresh `SecretCode`: "a unique generated shared secret code that is used by
/// the TOTP algorithm to generate a one-time code"
/// (`AssociateSoftwareToken.SecretCode` in this service's model).
///
/// Spelled in base32, the encoding an authenticator app provisioned with a shared
/// secret reads a key in, and the one [`software_token_emits`] reads it back
/// from, so the code a user's app shows is computed over the same key this
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
/// around `at`.
///
/// RFC 6238's TOTP over RFC 4226's HOTP, computed by the `totp-lite` crate rather
/// than here: SHA-1 is the algorithm an authenticator app uses when the key it was
/// provisioned with names none, and the code has to be byte-identical to the one
/// that app shows. Decided against the steps within [`SOFTWARE_TOKEN_DRIFT_STEPS`]
/// of the one `at` falls in. A key that is not base32, or an instant no counter
/// counts from, emits nothing.
fn software_token_emits(secret_code: &str, user_code: &str, at: SystemTime) -> bool {
    let Some(key) = software_token_key(secret_code) else {
        return false;
    };
    let Some(step) = software_token_time_step(at) else {
        return false;
    };
    (step.saturating_sub(SOFTWARE_TOKEN_DRIFT_STEPS)
        ..=step.saturating_add(SOFTWARE_TOKEN_DRIFT_STEPS))
        .any(|step| software_token_code(&key, step) == user_code)
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
    /// around `at` ([`software_token_emits`]).
    #[must_use]
    pub fn emits(&self, user_code: &str, at: SystemTime) -> bool {
        software_token_emits(&self.0, user_code, at)
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
    /// The request carries neither credential, or both
    /// ([`ONE_SOFTWARE_TOKEN_CREDENTIAL_MESSAGE`]).
    NotOneCredential,
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
    /// never handed out, or one started for a user its pool no longer holds
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
    /// The code is none that key emits — and a user that has associated no key
    /// emits none at all, so a code presented before any association matches
    /// nothing either.
    CodeMismatch,
    /// The user the credential named is no longer held, so nothing was
    /// registered.
    UserGone,
}

impl User {
    /// Whether the authenticator this user associated a private key for emits
    /// `user_code` around `at`. A user that has associated none emits no code,
    /// so nothing presented for it matches.
    pub fn software_token_emits(&self, user_code: &str, at: SystemTime) -> bool {
        self.software_token_key
            .as_ref()
            .is_some_and(|key| key.emits(user_code, at))
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
    pub fn software_token_caller(
        &mut self,
        access_token: Option<&str>,
        session: Option<&str>,
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
            _ => Err(SoftwareTokenRefusal::NotOneCredential),
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
                true
            }
            None => false,
        }
    }

    /// Register the software token `caller` associated a key for, against the
    /// one-time password `user_code` presented as proof the authenticator holds
    /// that key, as of `now`.
    ///
    /// A code the key emits around `now` ([`User::software_token_emits`]) "marks
    /// the user's software token MFA status as verified" (`VerifySoftwareToken` in
    /// this service's model), which is the delivery configuration TOTP MFA is
    /// delivered with ([`User::holds_mfa_delivery_config`]), so from here the user
    /// can activate `SOFTWARE_TOKEN_MFA`. Any other code registers nothing and
    /// leaves the user exactly as it was, so the setup can be completed with the
    /// next code the authenticator shows.
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
        if !user.software_token_emits(user_code, now) {
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
                ..session
            },
        ))
    }
}
