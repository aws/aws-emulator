Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 54a59f82019e3068961c13485094165efa260a0f

# Offline token whose `nbf` has not been reached is honoured when it names no expiry

_Filed: 2026-11-28T14:20:44Z_
_Operation: RegisterSubscriptionProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `54a59f82019e3068961c13485094165efa260a0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`offline_token_check` judges the registered access token against its own validity window at the instant the token validation behind the registration falls due (`now + TOKEN_VALIDATION_DELAY`). Its own doc states the rule for that instant twice over: a token whose window does not cover it — "it expires in between, OR ITS `nbf` HAS NOT BEEN REACHED" — must leave the registration landing on `INVALID` with the service's sentence as its `SubscriptionProviderStatusMessage`.

The code asks that question through `service_common::jwt::CompactJws::outside_window`, which reports AT MOST ONE defect and returns `WindowDefect::NoExpiry` BEFORE it ever looks at `nbf`. The handler then maps `NoExpiry` straight onto `CredentialCheck::Honoured`. So for any offline token that names no `exp` at all — which the same doc calls the ordinary shape of an offline refresh token ("an offline refresh token is issued to be held without one") — the `nbf` arm of the rule is DEAD CODE: a token carrying `{"typ":"Offline","nbf":<far future>}` and no `exp` is honoured.

The effect is a refusal turned into a success: `SubscriptionProviderRecord::settle` takes the `Honoured` verdict, writes `SubscriptionProviderStatus: ACTIVE`, clears the status message and records a `LastSuccessfulDataRetrievalTime` — a subscription-data retrieval that cannot have happened with a token the provider will not accept yet. The same registration carrying an `exp` beside the identical future `nbf` correctly lands on `INVALID`, so one of the two tokens is judged by the documented rule and the other is not.

The handler cannot read the `nbf` answer out of a single-defect reply: when the reader answers `NoExpiry`, the `nbf` question is still unanswered, and the code treats it as answered.

## Evidence

The rule the handler states for itself — services/.../src/ops/register_subscription_provider.rs:585-590:
```
/// * the instant the validation running behind an admitted registration falls due
///   (`now + `[`crate::state::TOKEN_VALIDATION_DELAY`]) — the instant this service would
///   first present the token to the provider. A token whose window does not cover that
///   instant (it expires in between, or its `nbf` has not been reached) is not one the
///   provider honours then, so the registration is taken and that validation lands on the
///   status which says the token was rejected, with the message the service has about it.
```
The code under it — register_subscription_provider.rs:612-618:
```
    if matches!(token.outside_window(now), Some(WindowDefect::Expired)) {
        return CredentialCheck::Unusable(WindowDefect::Expired.to_string());
    }
    match token.outside_window(now + crate::state::TOKEN_VALIDATION_DELAY) {
        None | Some(WindowDefect::NoExpiry) => CredentialCheck::Honoured,
        Some(defect) => CredentialCheck::NotHonoured(defect.to_string()),
    }
```
The reader it asks, which never reaches `nbf` once `exp` is absent — services/common/src/jwt.rs:241-253:
```
        let Some(expiry) = self.numeric_date(EXPIRY_CLAIM) else {
            return Some(WindowDefect::NoExpiry);
        };
        if seconds >= expiry {
            return Some(WindowDefect::Expired);
        }
        if self
            .numeric_date(NOT_BEFORE_CLAIM)
            .is_some_and(|starts| seconds < starts)
        {
            return Some(WindowDefect::NotYetValid);
        }
        None
```
What the `Honoured` verdict then writes — src/state.rs:249-254:
```
        match &self.token_verdict {
            TokenVerdict::Honoured => {
                self.status = PROVIDER_STATUS_ACTIVE.to_string();
                self.status_message = None;
                self.last_successful_data_retrieval_time = Some(iso8601(due));
```
Test coverage: tests/all/provider_token_window.rs states the same rule (lines 14-19: "a token whose window does not cover it (its `nbf` has not been reached) lands on `INVALID` carrying the service's sentence as its `SubscriptionProviderStatusMessage` and no retrieval time") and its table pins both halves SEPARATELY but never together — `{ "nbf": now + 3_600, "exp": now + 86_400 }` -> `Lands(INVALID)` (lines 139-143) and `window: json!({})` (no expiry, no nbf) -> `Lands(ACTIVE)` (lines 94-98). No row carries a future `nbf` WITHOUT an `exp`, so nothing pins the behaviour this issue reports and no test states a reason for it.

## Steps to Reproduce

1. Mount this service with Secrets Manager beside it on one engine (as tests/all/provider_token_window.rs does).
2. Create a secret whose `SecretString` is an offline JWT whose payload carries `{"typ":"Offline","nbf":4102444800}` and NO `exp` claim (a token valid from the year 2100 onwards).
3. Call `RegisterSubscriptionProvider` with `SubscriptionProviderSource=RedHat` and that `SecretArn`: the call is admitted and answers `SubscriptionProviderStatus: PENDING`.
4. Poll `GetRegisteredSubscriptionProvider` with the returned `SubscriptionProviderArn` until the status is terminal.
5. Observed: `SubscriptionProviderStatus: ACTIVE`, no `SubscriptionProviderStatusMessage`, and a `LastSuccessfulDataRetrievalTime`. Expected per the handler's own documented rule for the due instant: `INVALID` carrying the service's provider-token sentence and no retrieval time — the same answer the identical token with an `exp` beside its future `nbf` gets.
