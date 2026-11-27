Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 98f3508375edd68a74a0898c3bb96f24298a5655

# The @idempotencyToken replay rule is implemented three times, once per create, so the copies can drift

_Filed: 2026-11-20T21:56:00Z_
_Operation: CreateRequesterGateway | Class: maintainability_

> _Note: this issue was identified against the code base at commit `98f3508375edd68a74a0898c3bb96f24298a5655`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers implement the same `@idempotencyToken` rule — derive the token key from the operation and `clientToken`, look up a prior record, refuse with `ValidationException` when the parameter fingerprint differs, otherwise answer the stored resource, and check `IDEMPOTENCY_QUOTA` before inserting a new record. The decision is written out longhand in each of `create_requester_gateway.rs`, `create_responder_gateway.rs` and `create_link_routing_rule.rs`; only the fingerprint's member list genuinely differs between them.

This crate's own idiom for a rule more than one operation needs is to state it once under `src/state/` precisely so the copies cannot drift (the page walk, the token codec, the tagging ARN reading, the certificate surface all say so in their module docs), and two of the three copies even point the reader at the third for the contract instead of sharing it. The copies have already drifted: the routing-rule create settles the clock before it answers a replay (`store.settle_all(now)`), the two gateway creates do not, which is the open issue "CreateRequesterGateway never calls settle_all…".

The fix is to lift the replay decision (key, lookup, fingerprint comparison, refusal wording, quota check) into one helper beside `idempotency_key`/`parameter_fingerprint` in `crate::state`, leaving each handler to supply only its own fingerprint rendering and its own "answer the stored resource" projection. Every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/create_requester_gateway.rs:179-205:
```
        // `clientToken` is an `@idempotencyToken`: the same token replays the first
        // call's answer, and the same token with different parameters is refused
        // rather than silently answering for parameters the caller did not send.
        // The model declares no `IdempotentParameterMismatch`, so the refusal is
        // this operation's `ValidationException` (400).
        let fingerprint = crate::state::parameter_fingerprint(&format!(...));
        let token = crate::state::idempotency_key(Self::NAME, &input.client_token);
        if let Some(prior) = store.idempotency.get(&token).cloned() {
            if prior.fingerprint != fingerprint {
                return Err(CreateRequesterGatewayErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "clientToken {} was already used for a {} request with different parameters",
                            input.client_token,
                            Self::NAME,
                        ),
                    },
                )
                .into());
            }
```
plus its quota half, create_requester_gateway.rs:229-239:
```
        if store.idempotency.len() >= crate::state::IDEMPOTENCY_QUOTA {
            return Err(CreateRequesterGatewayErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this emulator holds its limit of {} client-token records",
                        crate::state::IDEMPOTENCY_QUOTA
                    ),
```

Copy 2 — src/ops/create_responder_gateway.rs:252-285, which names the first copy as the statement of the rule rather than sharing it:
```
        // See `create_requester_gateway` for the `@idempotencyToken` contract; the
        // token is scoped to this operation, so the two creates never collide.
        let fingerprint = crate::state::parameter_fingerprint(&format!(...));
        let token = crate::state::idempotency_key(Self::NAME, &input.client_token);
        if let Some(prior) = store.idempotency.get(&token).cloned() {
            if prior.fingerprint != fingerprint {
                return Err(CreateResponderGatewayErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "clientToken {} was already used for a {} request with different parameters",
```
plus create_responder_gateway.rs:309-319 (the same `IDEMPOTENCY_QUOTA` check).

Copy 3 — src/ops/create_link_routing_rule.rs:211-235:
```
        // See `create_requester_gateway` for the `@idempotencyToken` contract; the
        // token is scoped to this operation, so it never collides with a create of
        // another resource.
        let fingerprint = crate::state::parameter_fingerprint(&format!(...));
        let token = crate::state::idempotency_key(Self::NAME, &input.client_token);
        if let Some(prior) = store.idempotency.get(&token).cloned() {
            if prior.fingerprint != fingerprint {
                return Err(CreateLinkRoutingRuleErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "clientToken {} was already used for a {} request with different parameters",
```
plus create_link_routing_rule.rs:280-292 (the same `IDEMPOTENCY_QUOTA` check).

The crate's own stated reason for centralising a shared rule, src/state/paging.rs:3-11:
```
//! The ONE page walk this service's paginated reads take over their records.
//!
//! Every listing of this service orders its records by the unique id it pages by and
//! names the page boundary with a cursor the token codec beside this module renders,
//! so the walk itself ... is stated once here rather than once per listing: two listings then cannot
//! drift apart about what a page is, and a rule fixed for one walk is fixed for
//! every walk.
```

Tests that cover the behaviour and must stay unchanged: tests/all/gateway_link_lifecycle.rs:800-848 `a_repeated_client_token_returns_the_first_gateway_and_a_changed_one_is_refused` (both gateway creates, replay and 400 mismatch), tests/all/gateway_link_lifecycle.rs:1202-1212 (a replay answered 200 at the quota), tests/all/link_routing_rules.rs:531-545 (a rule token replayed with other parameters refused) and tests/all/link_routing_rules.rs:958-1045 `a_client_token_whose_resource_is_still_stored_keeps_replaying_it`.

## Steps to Reproduce

1. Read src/ops/create_requester_gateway.rs:179-205 and 229-239.
2. Read src/ops/create_responder_gateway.rs:252-285 and 309-319 — the same key/lookup/fingerprint-mismatch/quota decision, differing only in the fingerprint's member list and the error enum's name.
3. Read src/ops/create_link_routing_rule.rs:211-235 and 280-292 — a third copy of the same decision, which additionally calls store.settle_all(now) before answering a replay where the other two do not, showing the copies have already drifted.
4. Compare with src/state/paging.rs:3-11, src/state/page_tokens.rs:3-10 and src/state/tagging.rs:6-9, where this crate states a rule shared by several operations exactly once.
