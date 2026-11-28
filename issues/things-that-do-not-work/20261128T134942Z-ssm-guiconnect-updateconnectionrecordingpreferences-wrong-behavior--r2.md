Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d7a75d2240808bc2db1a7500caa93b9ab357f9b1

# Update reads the request's Region two different ways, so the just-in-time gate and the stored record can land in different Regions

_Filed: 2026-11-28T13:49:42Z_
_Operation: UpdateConnectionRecordingPreferences | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d7a75d2240808bc2db1a7500caa93b9ab357f9b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnectionRecordingPreferences` reads the Region of the request TWICE, through two different functions that do not agree:

* the account-level just-in-time node access gate reads it through Systems Manager's reader, `ssm_store::request_region(parts)`, which SILENTLY REPLACES any credential-scope region containing a character outside `[A-Za-z0-9-]` (or longer than 64 bytes) with `us-east-1`;
* the record itself, the quota check and the idempotency key are all filed under `crate::state::request_region(ctx.parts())`, which returns the credential scope's region VERBATIM with no such filtering.

For every credential scope whose region text is outside `[A-Za-z0-9-]` the two readings disagree, and the handler then enforces the gate for one Region while writing the record into another. Consequences, both directions:

1. An account enrolled ONLY in `us-east-1` is served an Update signed for region `us_east_1` (gate asks about `us-east-1`, which is enrolled) and the preferences are filed under the key `us_east_1` — a "Region" whose enrollment was never checked and which holds no approval policy. A later Get signed for `us_east_1` returns that record (200) while a Get signed for `us-east-1` answers 404, so the gate's Region and the resource's Region are provably not the same Region.
2. An account enrolled ONLY in `eu-west-1` is refused 403 for an Update signed for `eu_west_1`, because the gate was asked about `us-east-1` instead of the Region the record would have been written in.

This contradicts the crate's own stated rule that the Region a record is stored under and the Region a call is decided for are read from the request in exactly one way, and the crate's own test rule that "an Update signed for a Region whose policy the account holds is served, and one signed for any other Region is refused". No test covers a region spelling outside `[A-Za-z0-9-]`: every suite signs with well-formed names (`us-east-1`, `eu-west-1`, `ap-south-1`, `emu-test-<n>`), so nothing pins the current divergence.

## Evidence

Side 1 — this crate declares ONE reading of the request Region, covering both the storage key and the decision:
`services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:68-70`:
```
/// Re-exported from [`authorization`], which reads it to scope the resource an
/// authorization decision is taken against, so the Region a record is STORED under and
/// the Region a call is DECIDED for are read from the request in exactly one way.
pub use authorization::request_region;
```
and that reading does NOT filter the scope text — `src/state/authorization.rs:131-134`:
```
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}
```

Side 2 — the gate in the SAME handler uses a SECOND reading, `src/state/just_in_time_node_access.rs:46-48`:
```
    let region = ssm_store::request_region(parts);
    ssm_store::service_state(engine)
        .is_some_and(|ssm| ssm.lock().has_access_request_approval_policy(&region))
```
while the record is filed under the first reading, `src/ops/update_connection_recording_preferences.rs:224-227` then `:256`:
```
        if !crate::state::just_in_time_node_access::account_is_enrolled(
            &*self.engine,
            ctx.parts(),
        ) {
...
        let region = crate::state::request_region(ctx.parts());
```

The two readings provably differ: `services/service-com-amazonaws-ssm-amazonssm/src/state.rs:4133-4138` drops a scope the first reading keeps —
```
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_is_well_formed(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
with `services/service-com-amazonaws-ssm-amazonssm/src/state.rs:4147-4153`:
```
pub fn region_is_well_formed(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
```
and the enrollment it looks up is keyed by that normalised Region — `services/service-com-amazonaws-ssm-amazonssm/src/state/just_in_time_node_access.rs:271`: `.filter(move |((held, _), _)| held == region)`.

The rule the divergence breaks, stated by this crate's own doc and its own test:
`src/state/just_in_time_node_access.rs:25-28`: "The decision is taken per REGION ... the Region is the one the request's SigV4 credential scope names" — for a scope outside `[A-Za-z0-9-]` it is NOT the one the scope names, it is `us-east-1`.
`tests/all/jit_node_access_gate_and_error_messages.rs:584-587`: "The enrollment is decided per REGION, because the approval policy that constitutes it is a regional document: an Update signed for a Region whose policy the account holds is served, and one signed for any other Region is refused — in both directions, as policies come and go." That suite's rows only use `us-east-1`, `us-west-2`, `eu-west-3` (`:592-593`, `:612-616`), and the per-Region suite only `us-east-1`, `eu-west-1`, `ap-south-1`, `eu-central-1`, `us-west-2` plus `emu-test-<n>` (`tests/all/connection_recording_preferences.rs:1913-1917`, `:2150`), so no test exercises a scope the two readers disagree on.

## Steps to Reproduce

1. Build a router over one engine carrying both `EmulatedService` for ssm-guiconnect and the Systems Manager service (as `tests/all/connection_recording_preferences.rs::router_with_engine` does).
2. Enroll ONLY `us-east-1`: SSM `CreateDocument` of an `AutoApprovalPolicy`, signed with `Credential=AKIDEXAMPLE/20260101/us-east-1/ssm/aws4_request`.
3. Call `POST /UpdateConnectionRecordingPreferences` with a valid body (`BucketOwner=123456789012`, `BucketName=rdp-recordings`, a `KMSKeyArn`) signed with `Credential=AKIDEXAMPLE/20260101/us_east_1/ssm-guiconnect/aws4_request` (underscores — a region spelling `region_is_well_formed` rejects).
4. Observe HTTP 200: the gate was answered from `us-east-1`'s approval policy, but `crate::state::request_region` filed the record under the key `us_east_1`.
5. Call `POST /GetConnectionRecordingPreferences` signed for `us_east_1` — 200 with the record; call it signed for `us-east-1` — 404. The record therefore lives in a Region whose just-in-time enrollment was never checked.
6. Reverse direction: on a fresh engine enroll ONLY `eu-west-1`, then Update signed for `eu_west_1` — 403 `AccessDeniedException` with the just-in-time message, even though a successful call would have written the record under `eu_west_1`.
