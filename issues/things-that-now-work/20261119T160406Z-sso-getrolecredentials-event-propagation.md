Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: cd64e37d3e3efbbf828b0eac083377c231c88f4f

# A permission set's configured SessionDuration never reaches the role credential the portal mints (always 1 hour)

_Filed: 2026-11-19T16:04:06Z_
_Operation: GetRoleCredentials | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `cd64e37d3e3efbbf828b0eac083377c231c88f4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The role a portal response reports IS the name of an IAM Identity Center permission set: `assigned_accounts` resolves each assignment's permission-set record out of the sso-admin store and reports `permission_set.name` as the role. That same record carries the administrator's configured `SessionDuration` (`PermissionSetRecord::session_duration`, which the sso-admin crate validates into a 1h-12h window), and the model sentence this crate itself cites as the governing rule says a role session created through a permission set runs for "the duration configured in the permission set".

`GetRoleCredentials` never reads that configuration. Every minted credential is stamped with the crate-wide constant `ROLE_SESSION_DURATION` (1 hour), so a sign-in whose permission set is configured `PT12H` is handed a credential advertising a 1-hour `expiration` — and because `end_expired_credentials` deletes the credential from the emulator's credential store at the advertised instant, the emulator also stops accepting it for signing 11 hours early. The administrator's permission-set configuration therefore never propagates into the portal's role session at all; the portal's own listings propagate everything else from that same record (the role name) on the very next request.

The crate's comment justifies the constant on the ground that "Nothing in this API lets a caller ask for another duration" — which is true of the REQUEST, and is a sound reason to fall back to the default for a permission set that stores no `SessionDuration` (the member is optional). It says nothing about a permission set that DOES store one, which is the case the handler silently flattens to 1 hour.

This is a different defect from the open GetRoleCredentials issue about the expiry being materialised lazily (`20261104T151736Z-sso-getrolecredentials-wrong-behavior--r2`): that one is about WHEN the advertised instant is enforced, this one is about the advertised instant being computed from a constant instead of from the permission set.

## Evidence

The model rule, from `smithy_query(query="doc", shape="Logout")` (the Logout operation documentation of this service's own model):
```
After user logout, any existing IAM role sessions that were created by using IAM Identity Center
        permission sets continue based on the duration configured in the permission set.
```

The crate quoting that same sentence as the rule it implements, then using a constant — `services/service-com-amazonaws-sso-swbportalservice/src/state.rs:279-286`:
```rust
/// How long a minted role credential lasts.
///
/// IAM Identity Center returns the credentials of an IAM role session whose length is
/// "the duration configured in the permission set" (the `Logout` model documentation
/// says so in as many words), and the IAM Identity Center User Guide's "Set session
/// duration" page gives that permission-set default as 1 hour. Nothing in this API
/// lets a caller ask for another duration, so the default is the duration used.
pub const ROLE_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);
```

The handler stamping it unconditionally — `src/ops/get_role_credentials.rs:234-242`:
```rust
                let record = crate::state::RoleCredentialRecord {
                    access_key_id: minted.id.clone(),
                    ...
                    expiration_millis: crate::state::epoch_millis(crate::state::expiry_after(
                        now,
                        crate::state::ROLE_SESSION_DURATION,
                    )),
                };
```

The crate ALREADY reads the record that holds the configured duration — `src/state/sign_in.rs:248-257`:
```rust
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
```
and that record carries it — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:2099-2105`:
```rust
pub struct PermissionSetRecord {
    pub permission_set_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub created_date: DateTime,
    pub session_duration: Option<String>,
```
with the stored value already validated into a real span — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state/session_duration.rs:8-13, 37-41`:
```
//! `SessionDuration` is "the length of time that the application user sessions are valid in
//! the ISO-8601 standard", and IAM Identity Center documents that length as being between
//! ONE HOUR and TWELVE HOURS ([`MIN_SESSION_DURATION`], [`MAX_SESSION_DURATION`])
...
pub const MIN_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);
pub const MAX_SESSION_DURATION: Duration = Duration::from_secs(12 * 60 * 60);
```

No test pins the configured case. The two lifetime assertions run on sign-ins seeded straight into this store (`put_session`), where no permission set exists at all: `tests/all/portal_session.rs:589-596` (fixture `portal.sign_in(token, &every_account())`) and `tests/all/credential_expiry.rs:308-321` (fixture tokens `tok-expiring`/`tok-other`). The tests that DO build permission sets through sso-admin create them with no `SessionDuration` and assert nothing about the credential's lifetime — `tests/all/minted_sign_in.rs:236-245`:
```rust
    async fn permission_set(&self, instance_arn: &str, name: &str) -> String {
        ...
                "CreatePermissionSet",
                &json!({"InstanceArn": instance_arn, "Name": name}),
```
(`grep -n 'SessionDuration\|session_duration\|ROLE_SESSION_DURATION' tests/` matches only the two seeded-fixture assertions above.)

## Steps to Reproduce

1. Through sso-admin, create an IAM Identity Center instance and `CreatePermissionSet` with `{"InstanceArn": <arn>, "Name": "TwelveHour", "SessionDuration": "PT12H"}` (accepted: 12h is the top of the window `state/session_duration.rs:41` admits).
2. `CreateAccountAssignment` that permission set onto an account and let the operation reach SUCCEEDED.
3. Mint a portal sign-in with sso-oidc `CreateToken` and confirm `GET /assignment/roles?account_id=<account>` reports the role `TwelveHour` — the portal is reading the very permission-set record that carries `session_duration` (`src/state/sign_in.rs:248-257`).
4. Call `GET /federation/credentials?account_id=<account>&role_name=TwelveHour` with that bearer token and read `roleCredentials.expiration`.
5. Observed: the expiration is now + 1 hour (`ROLE_SESSION_DURATION`, `src/state.rs:286`), not now + the configured 12 hours; one hour later `end_expired_credentials` deletes the credential from the emulator's credential store, so requests signed with it are refused for the remaining 11 hours of the configured session.
6. Fix: compute the minted credential's duration from the permission set the resolved role came from (its `session_duration`, read through the existing `service_common::iso8601_span` helper), keeping `ROLE_SESSION_DURATION` as the fallback for a permission set that stores none — which is the only case the comment at `src/state.rs:284-285` justifies.
