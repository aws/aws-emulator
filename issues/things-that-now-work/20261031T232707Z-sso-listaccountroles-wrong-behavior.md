Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 58e93ad2f9bc5cf422504c21c341172571142218

# Role assignments are stored account-wide, so ListAccountRoles reports the same roles to every session assigned to the account instead of the roles assigned to the user

_Filed: 2026-10-31T23:27:07Z_
_Operation: ListAccountRoles | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `58e93ad2f9bc5cf422504c21c341172571142218`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `ListAccountRoles` as reporting the roles assigned to the USER in a given account, and `RoleInfo$roleName` as "the role that is assigned to the user". The handler reports something different: it resolves the account and then lists `account.roles` — the role set held on the ACCOUNT record — with no reference to the sign-in session that asked. Every session assigned to an account therefore sees an identical role list, and there is no representation in `ComAmazonawsSsoSwbportalserviceState` of a role assignment belonging to a user at all: `Session` holds `accounts: BTreeSet<String>` (account scoping per session) but no roles, while `Account` holds the roles.

The crate contradicts itself on this point. `Session::subject` is documented as the thing "the assignments belong to" (`src/state.rs:30-33`), and `state.rs` scopes the ACCOUNT dimension to the session through a dedicated helper (`visible_accounts` / `visible_account`, which refuse an account the presented session is not assigned to). There is no equivalent for roles: `ListAccountRoles` applies session scoping to the account and then drops the session entirely for the role dimension.

The same account-wide set is what `GetRoleCredentials` authorizes against (`account.roles.contains(&input.role_name)`, `src/ops/get_role_credentials.rs:173`), so the consequence is not only a listing: a session is vended STS credentials for any role present in an account it can see, including a role that belongs to another user's permission set. Because the store cannot express a per-user role set, a seeder (`put_account`) cannot set up two users with different permission sets in one account — the role dimension of an IAM Identity Center assignment is lost.

## Evidence

The modeled behaviour, from the Smithy model (`smithy_query(query="doc", shape="com.amazonaws.sso#ListAccountRoles")`), reproduced verbatim in the handler's own modeled-contract header at `services/service-com-amazonaws-sso-swbportalservice/src/ops/list_account_roles.rs:46`:
```
//! Lists all roles that are assigned to the user for a given AWS account.
```
and `com.amazonaws.sso#RoleInfo$roleName`:
```
"roleName": {"target": "RoleNameType", "doc": "<p>The friendly name of the role that is assigned to the user.</p>"}
```

What the handler actually lists — `src/ops/list_account_roles.rs:188-196` (the session resolved at :166 is never consulted again):
```rust
        let assigned = account.roles.iter().map(|role_name| {
            (
                role_name.clone(),
                RoleInfo {
                    role_name: Some(role_name.clone()),
                    account_id: Some(account.account_id.clone()),
                },
            )
        });
```

The state that makes a per-user role set unrepresentable — `src/state.rs:112-119`:
```rust
pub struct Account {
    pub account_id: String,
    pub account_name: String,
    pub email_address: String,
    /// The role names assigned in this account, ascending.
    pub roles: BTreeSet<String>,
}
```
against the session record that the crate says the assignments belong to — `src/state.rs:29-34`:
```rust
pub struct Session {
    /// Who the session signed in as. Reported by no operation; it is what the
    /// assignments belong to.
    pub subject: String,
    /// The account ids this session may see, ascending.
    pub accounts: BTreeSet<String>,
```
The account dimension HAS a per-session helper (`src/state.rs:288-294`):
```rust
    pub fn visible_account(&self, session: &Session, account_id: &str) -> Option<&Account> {
        session
            .accounts
            .contains(account_id)
            .then(|| self.accounts.get(account_id))
            .flatten()
    }
```
and the role dimension has none; `GetRoleCredentials` authorizes against the same account-wide set (`src/ops/get_role_credentials.rs:170-173`):
```rust
        let role_assigned = match state.session(&input.access_token, now) {
            Some(session) => state
                .visible_account(session, &input.account_id)
                .is_some_and(|account| account.roles.contains(&input.role_name)),
```

Test coverage: no suite varies the role set per session, because the state cannot. `tests/all/portal_session.rs:43-58` seeds roles on the ACCOUNT (`FIXTURE` rows are `(account id, name, email, roles)`), `tests/all/portal_session.rs:67-75` (`roles_of(account_id)`) computes the expected role list from the account id alone with no user dimension, and `list_account_roles_is_scoped_to_one_visible_account` (`tests/all/portal_session.rs:366`) pins only the ACCOUNT scoping — "a role listing is scoped to one account" (`tests/all/portal_session.rs:12-13`). So the user dimension the model names is unpinned and unimplemented.

## Steps to Reproduce

1. Stand up the service as `tests/all/portal_session.rs:85-114` does and seed one account holding two roles: `store.put_account("111111111111", "Production", "production@example.com", ["AdministratorAccess", "ReadOnly"])`.
2. Seed two different users' sign-in sessions on that same account: `store.put_session("tok-alice", "alice@example.com", ["111111111111"], ttl)` and `store.put_session("tok-bob", "bob@example.com", ["111111111111"], ttl)` — note there is no parameter for the roles each subject is assigned, because `Session` holds none (`src/state.rs:29-39`).
3. `GET /assignment/roles?account_id=111111111111` with `x-amz-sso_bearer_token: tok-alice`, then with `tok-bob`.
4. Observed: both answer the identical `roleList` — `AdministratorAccess` and `ReadOnly` — because the handler reads `account.roles` (`src/ops/list_account_roles.rs:188`) and never looks at the session; no seeding can make the two differ.
5. `GET /federation/credentials?account_id=111111111111&role_name=AdministratorAccess` with `tok-bob` also answers 200 with minted credentials (`src/ops/get_role_credentials.rs:170-173`), i.e. a role is vended to any session that can see the account.
6. Expected per `src/ops/list_account_roles.rs:46` ("Lists all roles that are assigned to the user for a given AWS account") and `RoleInfo$roleName` ("the role that is assigned to the user"): the role set is a property of the (user, account) assignment, so the store must hold it per session subject — the way `Session::accounts` already holds the account dimension — and the listing must report only the asking session's roles.
