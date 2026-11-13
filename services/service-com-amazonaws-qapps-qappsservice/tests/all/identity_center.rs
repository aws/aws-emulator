// SPDX-License-Identifier: CC0-1.0

//! The IAM Identity Center sign-in a Q Apps caller is federated through, as a fixture
//! every suite in this crate signs its requests behind.
//!
//! Q Apps serves only a caller whose credentials carry an Identity Center identity
//! (`crate::service::identity_center` in the crate under test), and in this emulator
//! such a credential is one the Identity Center portal issued to a signed-in user. The
//! portal models no sign-in API — a sign-in and an administrator's assignments arrive
//! through its own store — so that is where this fixture puts them, through the
//! portal's own public seeding accessors, on the SAME shared engine the qapps service
//! reads.
//!
//! What it seeds is exactly what a `GetRoleCredentials` call leaves behind: an account
//! with a role assigned in it, an active sign-in session, and the role credential
//! issued to that session. `tests/all/identity_center_front_door.rs` drives that call
//! over HTTP instead, so the shape seeded here is pinned against the portal's own
//! answer rather than assumed.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_sso_swbportalservice::state::{
    self as portal, RoleCredentialRecord,
};

/// How long a seeded sign-in and its credential last. The sign-in session length is a
/// directory setting in IAM Identity Center, so a fixture picks one; it is long enough
/// that no test can see it lapse.
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The account the portal assigns the role in: this emulator's own account, which is
/// the account every ARN it mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The role a signed-in user holds in that account.
pub(crate) const ROLE: &str = "QAppsUser";

/// The access token of the sign-in session `access_key_id` was issued to. One sign-in
/// per caller, so two callers are two Identity Center users.
pub(crate) fn access_token_for(access_key_id: &str) -> String {
    format!("sso-access-token-{access_key_id}")
}

/// Federate `access_key_id`: record the Identity Center sign-in that issued it, so a
/// qapps request signed with it carries an Identity Center identity.
pub(crate) fn federate(engine: &Arc<engine::InMemoryEngine>, access_key_id: &str) {
    let state = portal::service_state(&**engine).expect("the portal store is reachable");
    let mut store = state.lock();
    let access_token = access_token_for(access_key_id);
    store.put_account(ACCOUNT, "Emulator", "qapps@example.com");
    store.put_session(
        &access_token,
        format!("{access_key_id}@example.com"),
        [(ACCOUNT, [ROLE])],
        SESSION_TTL,
    );
    store.issued_credentials.insert(
        (access_token, ACCOUNT.to_string(), ROLE.to_string()),
        RoleCredentialRecord {
            access_key_id: access_key_id.to_string(),
            // The portal mints both secrets from the emulator's credential store; a
            // fixture that seeds the record instead names them, and nothing in these
            // suites signs with them (the router's signature check is off).
            secret_access_key: format!("secret-for-{access_key_id}"),
            session_token: format!("session-token-for-{access_key_id}"),
            expiration_millis: portal::epoch_millis(
                SystemTime::now()
                    .checked_add(SESSION_TTL)
                    .expect("a representable expiry"),
            ),
        },
    );
}
