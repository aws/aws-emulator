Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 484a4b2008bb9a37e4730ecb33af9b9fee8e7572

# The crate's own StatedPerOperation wrapper answers a constraint refusal in front of the IAM gate, so an unauthorized caller with a malformed accountPlanType is told about its request instead of being refused 403

_Filed: 2026-11-25T17:05:46Z_
_Operation: UpgradeAccountPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `484a4b2008bb9a37e4730ecb33af9b9fee8e7572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpgradeAccountPlan` is one of the two operations of this service whose model declares `AccessDeniedException` (HTTP 403), and its handler states the rule it follows: the IAM decision is taken "before the request's own input is looked at or anything is written". The gate itself (`crate::service::authorization::authorize`) is the FIRST statement of `handle`.

But `handle` is not the first thing this crate runs for a request. Every modelled operation is served through this crate's own `StatedPerOperation` wrapper (`src/service.rs`), whose `run` collects the framework's constraint violations over the request's own input and RETURNS this service's 400 `ValidationException` itself, before it ever calls `self.operation.run(..)` — the only path on which `authorize` is reached. So for a request that breaks a modelled constraint of `UpgradeAccountPlanRequest`, the wrapper short-circuits and the gate never runs: a caller whose identity-based policies grant it nothing at all is answered 400 `ValidationException`, with the value it submitted echoed back in the clause, instead of the modelled 403.

That is the opposite order from the one the handler's own comment states, and the refusal it answers with reports the caller's input rather than refusing the identity. `UpgradeAccountPlanRequest` has exactly one member, `accountPlanType` (`@required`, enum `FREE | PAID`), so the reachable inputs are: the member absent, and the member carrying a value outside the enum — both answered 400 ahead of the gate. (`GetAccountPlanState`, the other gated operation, has no input members, so nothing can be refused in front of its gate; the ungated three model no 403 and are unaffected.)

The skipped step is not a new feature: the gate exists, is called by both gated handlers, and takes nothing from the handler but `(&engine, ctx, Self::NAME)` — values the wrapper's `run` already holds (`svc`/`req`/`ctx`) — so this is a path that skips a helper the crate already has.

## Evidence

The rule this crate states, `src/ops/upgrade_account_plan.rs:175-179`:
```rust
        // IAM first: this operation models `AccessDeniedException`, and the decision is
        // taken before the request's own input is looked at or anything is written, so a
        // caller no identity-based policy allows changes nothing.
        crate::service::authorization::authorize(&*self.engine, ctx, Self::NAME)
            .map_err(UpgradeAccountPlanErrorModeled::AccessDeniedException)?;
```
and the same rule in the gate's own module header, `src/service/authorization.rs:6-9`:
```
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! account's own records, so a caller whose identity-based policies do not grant the
//! action is told it is not authorized rather than told anything about the data it asked
//! for.
```

The code in the SAME crate that breaks it, `src/service.rs:263-274` (`impl ErasedOperation for StatedPerOperation`):
```rust
        Box::pin(async move {
            // The definition is read, used and dropped inside each statement:
            // ...
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return RESPONSE_MEDIA_TYPE.announce(svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                ));
            }
            // Every other answer of a routed request ...
            RESPONSE_MEDIA_TYPE.announce(self.operation.run(svc, req, ctx).await)
        })
```
`stated_refusal` is what looks at the request's own input: `src/service.rs:243-250` — "one read of the request yields the framework's own violations ... with each value echo restored to the bytes the caller sent" — and the `return` above happens before `self.operation.run(..)`, i.e. before `handle` and therefore before `authorize`.

That wrapper is wired for this operation: `src/service.rs:186-194`
```rust
        wrap_operations!(
            stated,
            engine,
            get_account_activity::GetAccountActivity,
            ...
            upgrade_account_plan::UpgradeAccountPlan,
        );
```

Test coverage: the two sides are each pinned, never together, so nothing catches the ordering and a fix changes no existing expectation. `tests/all/identity_authorization.rs:529-535` pins `(403, "AccessDeniedException")` for an unauthorized caller — but every row sends a WELL-FORMED body, built at `tests/all/identity_authorization.rs:125-131`:
```rust
    async fn call(&self, access_key_id: &str, operation: &str) -> (u16, Value) {
        let body = match operation {
            "UpgradeAccountPlan" => json!({ "accountPlanType": PLAN_TYPE_PAID }),
            _ => json!({}),
        };
```
The constraint-refusal rows (`tests/all/paid_plan_vetting.rs:213` `an_account_plan_type_outside_the_enum_names_the_published_value_set`, `tests/all/free_tier.rs:348` `upgrade_account_plan_refuses_a_plan_type_outside_the_enum`, `tests/all/validation_message_rules.rs`) are driven through the `harness()` router, i.e. with the ambient account credential the gate does not bound (`src/service/authorization.rs:54-57`: "Such a request is allowed."), so they stay 400 under either ordering.

## Steps to Reproduce

1. Mount the service the way tests/all/identity_authorization.rs does (`Mounted::new()`, which also mounts IAM and STS), mint an IAM user access key with NO policy granting `freetier:UpgradeAccountPlan` (`user_key("freetier-probe", None)`), so the caller falls to IAM's default deny.
2. Call `UpgradeAccountPlan` signed with that access key id and a WELL-FORMED body `{"accountPlanType":"PAID"}`: the answer is 403 `AccessDeniedException` (tests/all/identity_authorization.rs:529-535).
3. Repeat the identical call with the SAME unauthorized access key id but a body outside the enum, `{"accountPlanType":"GOLD"}` (or with the member omitted entirely, `{}`).
4. Observe the answer: 400 `ValidationException` whose message is the constraint clause echoing the submitted value (`1 validation error detected: Value 'GOLD' at 'accountPlanType' failed to satisfy constraint: ...`) rather than the modelled 403 — `crate::service::authorization::authorize` never ran, because `src/service.rs:268-273` returned before `self.operation.run`.
5. Compare the two answers for the one caller: the handler comment at src/ops/upgrade_account_plan.rs:175-177 says the IAM decision is taken 'before the request's own input is looked at', and step 4 shows the input was looked at, and reported back, first.
