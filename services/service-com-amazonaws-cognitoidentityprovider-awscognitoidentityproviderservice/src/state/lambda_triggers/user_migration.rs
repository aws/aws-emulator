// SPDX-License-Identifier: CC0-1.0

//! The migrate-user trigger: the one place a pool's `LambdaConfig.UserMigration` becomes an
//! invocation of the function that member names.
//!
//! "The configuration of a migrate user Lambda trigger in a user pool. This trigger can
//! create user profiles when users sign in or attempt to reset their password with
//! credentials that don't exist yet" (`LambdaConfigType$UserMigration`, this service's
//! model). So on a pool that configures one, a username the pool does not hold is not an
//! unknown user yet: the function is the pool's authority on whether that user exists
//! somewhere else, and it is asked — with the event of the stage that asked
//! ([`MigrationSource`]) — before the request is refused. A function that answers with a
//! profile has the user CREATED from that profile, and the operation then decides over the
//! state the migration left; a function that answers with no profile has found no such user,
//! so the pool still holds none and the operation refuses the username exactly as it would
//! for a pool that configures no trigger at all.
//!
//! This trigger GATES the stage that fired it — the stage cannot decide whether the user
//! exists on its own — so a function that could not be invoked, one that did not answer
//! inside the trigger budget, and one that RAN AND REPORTED AN ERROR each fail the operation
//! with the trigger's own failure rather than with a verdict nothing gave
//! ([`super::invoke_gating`]).
//!
//! The work splits the same way every other trigger of this service does ([`super`]):
//! [`user_migration`] resolves the member and builds the event from the pool read under the
//! state lock, and [`fire_user_migration`] performs the invocation and stores what it
//! answered once that lock is RELEASED.
//!
//! The event names no stored user — there is none yet, which is the whole point of the stage
//! — so its request half is the stage's own: the password the sign-in presented, which a
//! password reset has none of, and the metadata the request assigned.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use ::serde_json::{Map, Value};

use super::{Caller, Dispatch, Failure, State};
use crate::state::{User, UserPool, UserStatus};

/// The trigger whose configured function every migrate-user event is delivered to, as a
/// failure of that trigger names it ([`super::failure_message`]).
pub const USER_MIGRATION_TRIGGER: &str = "UserMigration";

/// The response member a migrating function returns the user's profile in.
const USER_ATTRIBUTES: &str = "userAttributes";

/// The response member a migrating function states the created user's status in.
const FINAL_USER_STATUS: &str = "finalUserStatus";

/// The attribute a migrated user's profile may NOT carry a value for: `sub` is the user's
/// own id, which the pool mints when it creates the user.
const SUB_ATTRIBUTE: &str = "sub";

/// The status a migrated user is created in where the answering function named none.
///
/// `finalUserStatus` is what a function sets "to `CONFIRMED` to auto-confirm your users, so
/// that they can sign in with their previous passwords" (Amazon Cognito's migrate-user
/// event), so a migration that does not ask for that is one whose user has no password of
/// its own in this pool yet and has to set one before it signs in — which is the status
/// `RESET_REQUIRED` names: "user is confirmed, but the user must request a code and reset
/// their password" (`UserStatusType` in this service's model).
const MIGRATED_WITHOUT_A_STATUS: UserStatus = UserStatus::ResetRequired;

/// Which stage asked a pool to migrate a username it does not hold, which is what the
/// event's `triggerSource` reports: the trigger's name and that stage.
///
/// The trigger "can create user profiles when users sign in or attempt to reset their
/// password with credentials that don't exist yet"
/// (`LambdaConfigType$UserMigration`), so those two stages are the ones that reach it, and a
/// function tells them apart by this member alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationSource {
    /// A sign-in that presented a password: `InitiateAuth` with `USER_PASSWORD_AUTH` and
    /// `AdminInitiateAuth` with its administrative password flows.
    Authentication,
    /// A self-service password reset, by `ForgotPassword`.
    ForgotPassword,
}

impl MigrationSource {
    /// This stage's `triggerSource`.
    #[must_use]
    pub fn trigger_source(self) -> &'static str {
        match self {
            Self::Authentication => "UserMigration_Authentication",
            Self::ForgotPassword => "UserMigration_ForgotPassword",
        }
    }
}

/// The profile a migrating function answered with: the attributes the migrated user is
/// created with, and the status it is created in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MigratedProfile {
    attributes: BTreeMap<String, String>,
    status: UserStatus,
}

/// The migrate-user invocation a request naming `username`, which `pool` does not hold,
/// owes — or `None` when the pool configures no migrate-user function and so the username is
/// simply unknown.
///
/// Built while the caller still holds the state lock, and invoked by
/// [`fire_user_migration`] once that lock is released.
#[must_use]
pub fn user_migration(
    pool: &UserPool,
    username: &str,
    source: MigrationSource,
    password: Option<&str>,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Dispatch> {
    let function_arn = pool
        .lambda_config
        .user_migration
        .clone()
        .filter(|arn| !arn.is_empty())?;

    let mut request = Map::new();
    // "password — The password that the user entered for sign-in. Amazon Cognito doesn't
    // send this value in a request for the `UserMigration_ForgotPassword` trigger source"
    // (Amazon Cognito's migrate-user event), so the member is there exactly when the stage
    // that asked presented one.
    if let Some(password) = password {
        request.insert(
            "password".to_string(),
            Value::String(password.to_string()),
        );
    }
    // "validationData — One or more key-value pairs ... You can pass this data to your
    // Lambda function by using the ClientMetadata parameter", and `clientMetadata` "provides
    // the data that you assigned to the ClientMetadata parameter in your request" (every
    // operation that models it): both are the metadata THIS request assigned, and a request
    // that assigned none carries neither.
    if let Some(metadata) = client_metadata.filter(|metadata| !metadata.is_empty()) {
        let members = super::metadata_members(metadata);
        request.insert(
            "validationData".to_string(),
            Value::Object(members.clone()),
        );
        request.insert("clientMetadata".to_string(), Value::Object(members));
    }

    // Built here rather than through [`super::event`]: that event reports a STORED user's
    // attributes and this stage has no stored user at all — the function is being asked to
    // supply one.
    let event = ::serde_json::json!({
        "version": super::EVENT_VERSION,
        "triggerSource": source.trigger_source(),
        // The pool's own Region and id, not the request's: the event names the resource the
        // trigger is configured on.
        "region": pool.region,
        "userPoolId": pool.id,
        // The name the request submitted, which is the only name this user has yet.
        "userName": username,
        "callerContext": {
            "awsSdkVersion": caller
                .aws_sdk_version
                .unwrap_or(super::UNKNOWN_AWS_SDK_VERSION),
            "clientId": caller.client_id.unwrap_or(super::CLIENT_ID_NOT_APPLICABLE),
        },
        "request": Value::Object(request),
        // The response half is the function's to fill in: it carries the profile the
        // migrated user is created from, so the event is delivered with none of it.
        "response": Value::Object(Map::new()),
    });
    Some(super::dispatch(
        pool,
        function_arn,
        USER_MIGRATION_TRIGGER,
        event,
    ))
}

/// Ask `user_pool_id`'s migrate-user function about `username`, and store the user it
/// answered with.
///
/// For an operation that holds NO lock: the pool is read under a lock of its own, which is
/// released before the invocation, so no other request of this service waits on the
/// function; the user the function answered with is stored under a second one. A pool that
/// configures no migrate-user function is answered by nothing being invoked at all.
///
/// What a RETURNING function left is the STATE: a caller reads the pool again and finds the
/// migrated user, or finds the username still unknown — "this field must contain one entry
/// for each user attribute you want to populate" (the event's `userAttributes`), so a
/// function that returned none populated no user and the username stays as unknown as it
/// was, which is the one case the operation refuses as a user that does not exist.
///
/// Every other outcome is this trigger's own failure and the operation's to report, because
/// this trigger GATES the stage that fired it: the function decides whether the user exists
/// at all, so an invocation that reached no function, one that was not answered inside the
/// trigger budget, and a function that RAN AND REPORTED AN ERROR each leave the operation
/// nothing to decide over ([`super::invoke_gating`]).
#[allow(clippy::too_many_arguments)]
pub async fn fire_user_migration(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    store: &::engine::ServiceState<State>,
    user_pool_id: &str,
    username: &str,
    source: MigrationSource,
    password: Option<&str>,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Result<(), Failure> {
    let dispatch = {
        let guard = store.lock();
        guard.pool(user_pool_id).and_then(|pool| {
            user_migration(pool, username, source, password, caller, client_metadata)
        })
    };
    let Some(dispatch) = dispatch else {
        return Ok(());
    };

    let answered = super::invoke_gating(ctx, &dispatch).await?;
    let Some(profile) = migrated_profile(&answered) else {
        ::tracing::debug!(
            function_arn = %dispatch.function_arn(),
            trigger = USER_MIGRATION_TRIGGER,
            "a user pool migrate-user trigger answered with no user profile; no user is \
             created for it"
        );
        return Ok(());
    };

    let now = SystemTime::now();
    let guard = &mut *store.lock();
    store_migrated_user(guard, user_pool_id, username, profile, password, now);
    Ok(())
}

/// The profile a returning function answered with, or `None` when it answered with none —
/// which is a function that found no such user to migrate.
///
/// "userAttributes — This field must contain one entry for each user attribute you want to
/// populate" (Amazon Cognito's migrate-user event), so the entries it carries ARE the
/// migrated user's attributes, bar `sub`, which is the pool's to mint. A value that is not
/// text is read as the text it states, since every user attribute of this service is stored
/// as text.
fn migrated_profile(answered: &Value) -> Option<MigratedProfile> {
    let response = answered.get("response")?;
    let returned = response.get(USER_ATTRIBUTES)?.as_object()?;
    let attributes: BTreeMap<String, String> = returned
        .iter()
        .filter(|(name, _)| name.as_str() != SUB_ATTRIBUTE)
        .filter_map(|(name, value)| attribute_text(value).map(|text| (name.clone(), text)))
        .filter(|(_, value)| !value.is_empty())
        .collect();
    if attributes.is_empty() {
        return None;
    }
    let status = response
        .get(FINAL_USER_STATUS)
        .and_then(Value::as_str)
        .and_then(UserStatus::named)
        .unwrap_or(MIGRATED_WITHOUT_A_STATUS);
    Some(MigratedProfile { attributes, status })
}

/// The text an answered attribute value states, or `None` for a value that states no single
/// value at all.
fn attribute_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// Create the migrated user in `user_pool_id` from `profile`.
///
/// `submitted` is the name the request named it by. The user is created the way every other
/// path of this service creates one: its `sub` is minted here, a pool whose users sign in
/// with an attribute holds the submitted value under that attribute and mints the username
/// itself ([`UserPool::sign_in_attribute`]), and a value stored for a verifiable attribute
/// the function stated no flag for is flagged unconfirmed.
///
/// `password` is the one the stage presented, which is what the migrated user signs in with
/// from now on: a function sets `finalUserStatus` to `CONFIRMED` "to auto-confirm your
/// users, so that they can sign in with their previous passwords" (Amazon Cognito's
/// migrate-user event), and that password is the one it just presented. A stage that
/// presented none — a password reset — creates a user holding none, which is the password
/// the reset it continues sets.
///
/// Nothing is created where the pool already holds the user (another request migrated it
/// first) or where it holds as many users as this emulator stores for one pool.
fn store_migrated_user(
    state: &mut State,
    user_pool_id: &str,
    submitted: &str,
    profile: MigratedProfile,
    password: Option<&str>,
    now: SystemTime,
) {
    let Some(pool) = state.pool(user_pool_id) else {
        return;
    };
    if pool.user(submitted).is_some() || pool.users.len() >= crate::state::MAX_USERS_PER_POOL {
        return;
    }
    let sub = crate::state::new_uuid();
    let sign_in_attribute = pool.sign_in_attribute(submitted).map(str::to_string);
    let (username, key) = match &sign_in_attribute {
        Some(_) => (sub.clone(), sub.clone()),
        None => {
            let key = pool.user_key(submitted);
            (key.clone(), key)
        }
    };

    let MigratedProfile {
        mut attributes,
        status,
    } = profile;
    // `sub` is the user's own id, not something the answering function may set.
    attributes.insert(SUB_ATTRIBUTE.to_string(), sub.clone());
    if let Some(attribute) = sign_in_attribute {
        attributes
            .entry(attribute)
            .or_insert_with(|| submitted.to_string());
    }
    let unflagged: Vec<&'static str> = attributes
        .keys()
        .filter_map(|name| crate::state::verified_flag(name))
        .collect();
    for flag in unflagged {
        attributes
            .entry(flag.to_string())
            .or_insert_with(|| crate::state::FLAG_UNCONFIRMED.to_string());
    }

    let user = User {
        username,
        sub,
        attributes,
        // Nothing is pending for a user that was just created: the attributes above are its
        // values, not values awaiting a confirmation of a changed one.
        pending_attributes: BTreeMap::new(),
        password: password.map(str::to_string),
        // The password is the user's own — the one it signed in with — so there is nothing
        // to change at the next sign-in.
        password_permanent: true,
        password_set_at: password.map(|_| now),
        // A user that was just created has held no password before this one.
        password_history: Vec::new(),
        status,
        enabled: true,
        mfa_options: Vec::new(),
        preferred_mfa_setting: None,
        mfa_setting_list: Vec::new(),
        user_create_date: now,
        user_last_modified_date: now,
        groups: BTreeSet::new(),
        // A migrated user has no remembered device, no outstanding code, no passkey, no
        // linked external identity, no registered authenticator and no event history: each
        // exists only once something of this pool has created one.
        devices: BTreeMap::new(),
        outstanding_codes: BTreeMap::new(),
        web_authn_credentials: Vec::new(),
        provider_links: BTreeMap::new(),
        software_token_registered: false,
        software_token_key: None,
        web_authn_mfa_enabled: None,
        auth_events: Vec::new(),
    };
    if let Some(pool) = state.pool_mut(user_pool_id) {
        pool.users.insert(key, user);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `triggerSource` this trigger reports names the trigger and the stage that
    /// reached it, so a function can tell a password reset from a sign-in by that member
    /// alone.
    #[test]
    fn every_trigger_source_names_the_trigger_and_its_stage() {
        // The two stages Amazon Cognito documents a migrate-user source for.
        let cases = [
            (MigrationSource::Authentication, "Authentication"),
            (MigrationSource::ForgotPassword, "ForgotPassword"),
        ];
        for (source, stage) in cases {
            let reported = source.trigger_source();
            assert_eq!(
                reported,
                format!("{USER_MIGRATION_TRIGGER}_{stage}"),
                "case {source:?}: expected the trigger and the stage, got {reported:?}"
            );
        }
    }

    /// One expected profile of the table below: the attributes the user is created with,
    /// and the status it is created in.
    type Expected = Option<(Vec<(&'static str, &'static str)>, UserStatus)>;

    /// The profile read out of an answered event: the attributes the function returned, the
    /// status it asked for, and nothing where it returned no profile at all.
    #[test]
    fn the_profile_is_what_the_function_answered() {
        // (case, the response half the function answered with, expected profile)
        let rows: [(&str, Value, Expected); 8] = [
            (
                "a profile with a status",
                ::serde_json::json!({
                    "userAttributes": {"email": "legacy@example.test", "email_verified": "true"},
                    "finalUserStatus": "CONFIRMED",
                }),
                Some((
                    vec![("email", "legacy@example.test"), ("email_verified", "true")],
                    UserStatus::Confirmed,
                )),
            ),
            (
                "a profile naming no status at all",
                ::serde_json::json!({ "userAttributes": {"email": "legacy@example.test"} }),
                Some((
                    vec![("email", "legacy@example.test")],
                    MIGRATED_WITHOUT_A_STATUS,
                )),
            ),
            (
                "a profile whose status is no modelled one",
                ::serde_json::json!({
                    "userAttributes": {"email": "legacy@example.test"},
                    "finalUserStatus": "MIGRATED",
                }),
                Some((
                    vec![("email", "legacy@example.test")],
                    MIGRATED_WITHOUT_A_STATUS,
                )),
            ),
            (
                "a profile asking for a reset",
                ::serde_json::json!({
                    "userAttributes": {"phone_number": "+15550100"},
                    "finalUserStatus": "RESET_REQUIRED",
                }),
                Some((vec![("phone_number", "+15550100")], UserStatus::ResetRequired)),
            ),
            (
                "a profile stating a flag as a JSON boolean",
                ::serde_json::json!({
                    "userAttributes": {"email": "legacy@example.test", "email_verified": true},
                    "finalUserStatus": "CONFIRMED",
                }),
                Some((
                    vec![("email", "legacy@example.test"), ("email_verified", "true")],
                    UserStatus::Confirmed,
                )),
            ),
            (
                "a profile trying to set the user's own id",
                ::serde_json::json!({
                    "userAttributes": {"sub": "not-the-pools-id", "email": "legacy@example.test"},
                    "finalUserStatus": "CONFIRMED",
                }),
                Some((
                    vec![("email", "legacy@example.test")],
                    UserStatus::Confirmed,
                )),
            ),
            (
                "an empty profile",
                ::serde_json::json!({ "userAttributes": {} }),
                None,
            ),
            ("no profile at all", ::serde_json::json!({}), None),
        ];

        for (case, response, expected) in rows {
            let answered = ::serde_json::json!({ "response": response });
            let read = migrated_profile(&answered);
            let expected = expected.map(|(attributes, status)| MigratedProfile {
                attributes: attributes
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                status,
            });
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
