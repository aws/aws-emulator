Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# Compose the assumed-role user id (`<role id>:<session name>`) through one helper, as its ARN sibling already is

_Filed: 2026-11-27T10:05:20Z_
_Operation: AssumeRoleForPodIdentity | Class: maintainability_

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state/role_session.rs` composes the assumed-role user id — a role's id, a colon, and the role session name — twice, by hand, in two different functions, while the ARN form of the very same identity already has ONE helper (`assumed_role_user_arn`) that both call sites use.

The two inlined copies are:

* `chained_role` builds it as the `aws:userid` condition-key value the TARGET role's trust policy is evaluated against (`format!("{}:{}", association_role.id, plan.session_name)`);
* `mint` builds it as `assume_role_id`, which the handler returns verbatim in `assumedRoleUser.assumeRoleId` and stores on `PodIdentitySession::assume_role_id`.

Both are the same computation of the same documented value (`VendedSession::assume_role_id` documents it as "`<role id>:<session name>`"), so the copies can drift: only the `mint` copy is covered by a test, so changing the composition there (or in a future partition/format tweak) would leave the trust-policy copy spelling a different `aws:userid` than the id the response reports for the same session, and a trust policy conditioned on `aws:userid` would silently stop matching. Two lines above the second copy, the ARN form of the identical identity is already funnelled through one function — so this crate has the pattern and this one value skips it.

Fix: give the id form the same single helper next to `assumed_role_user_arn` (e.g. `fn assumed_role_user_id(role: &ResolvedRole, session_name: &str) -> String`) and call it from both sites. Every response string and every existing test stays byte-identical.

## Evidence

The helper the crate ALREADY has for the ARN half of this identity, used by BOTH call sites — services/service-com-amazonaws-eksauth-eksauthfrontend/src/state/role_session.rs:313-320:
```
/// The ARN of the session `session_name` of `role`, which is what AWS reports for an
/// assumed-role identity.
fn assumed_role_user_arn(role: &ResolvedRole, session_name: &str) -> String {
    format!(
        "arn:{}:sts::{}:assumed-role/{}/{session_name}",
        role.partition, role.account, role.name
    )
}
```
called at role_session.rs:201 (`let session_arn = assumed_role_user_arn(association_role, &plan.session_name);`) and role_session.rs:266 (`let assumed_role_user_arn = assumed_role_user_arn(role, &plan.session_name);`).

Copy 1 of the id form — role_session.rs:215-218, inside `chained_role`:
```
        (
            "aws:userid".to_string(),
            format!("{}:{}", association_role.id, plan.session_name),
        ),
```
Copy 2 — role_session.rs:267, inside `mint`:
```
    let assume_role_id = format!("{}:{}", role.id, plan.session_name);
```
The one value both are computing, documented once — role_session.rs:140-141:
```
    /// `<role id>:<session name>`.
    pub assume_role_id: String,
```
and role_session.rs:329 on `ResolvedRole`: `/// Its unique id, which `assumeRoleId` is built from.`

Test coverage (unchanged by the fix): `tests/all/assume_role_for_pod_identity.rs:315-325` pins copy 2 through the wire response —
```
        let (role_id, session_name) = assume_role_id
            .split_once(':')
            .unwrap_or_else(|| panic!("case {seed:?}: no role id in {assume_role_id:?}"));
        assert_eq!(
            role_id, association.role_id,
```
and `tests/all/assume_role_for_pod_identity.rs:888-900` (`the_role_id_belongs_to_the_role_not_to_the_association`) splits the same value again. Copy 1 is covered by no test in this crate: `grep -n 'userid|target_role' services/service-com-amazonaws-eksauth-eksauthfrontend/tests` matches nothing, which is why a drift there would be silent.

## Steps to Reproduce

1. Open services/service-com-amazonaws-eksauth-eksauthfrontend/src/state/role_session.rs and read line 201 and line 266: the ARN of an assumed-role session is composed in ONE place, `assumed_role_user_arn` (lines 313-320), and both call sites use it.
2. Read line 217: `format!("{}:{}", association_role.id, plan.session_name)` — the assumed-role user id, inlined for the `aws:userid` condition key the target role's trust policy is judged with.
3. Read line 267: `let assume_role_id = format!("{}:{}", role.id, plan.session_name);` — the same composition inlined again, this time for `assumedRoleUser.assumeRoleId` in the response and `PodIdentitySession::assume_role_id` in state.
4. Grep the crate's tests for `userid` or `target_role`: no test exercises the first copy, so an edit to the second (the tested one) leaves the first spelling a different user id for the same session with nothing failing.
5. Extract one `assumed_role_user_id(role, session_name)` helper beside `assumed_role_user_arn`, call it from both sites, and re-run `cargo test -p service-com-amazonaws-eksauth-eksauthfrontend`: every assertion, including `every_seeded_association_exchanges_its_token_for_credentials_from_state`, still passes with identical responses.
