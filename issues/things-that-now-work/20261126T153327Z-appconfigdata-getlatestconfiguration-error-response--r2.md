Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2554e5494fd859ae6eab8bc9ad0f2531c606d0f2

# GetLatestConfiguration refuses a blank configuration_token with BadRequestException before the identity-authorization gate runs, so an unauthorized caller is answered 400 instead of 403 — the opposite order from the sibling handler and from this handler's own stated rule

_Filed: 2026-11-26T15:33:27Z_
_Operation: GetLatestConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `2554e5494fd859ae6eab8bc9ad0f2531c606d0f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both operations of this crate gate the call on the caller's identity-based IAM policies, and both state the same ordering rule: the gate is decided FIRST, ahead of the service's own reading of the request's input. `StartConfigurationSession` implements exactly that — the authorization gate runs first, and only then does the handler refuse an identifier whose FORM its members do not accept (an ARN) with `BadRequestException`.

`GetLatestConfiguration` implements the reverse order for its equivalent handler-level request-form refusal. Its STEP 0 reads the request's own token value and returns `BadRequestException` (400, "ConfigurationToken must be provided…") for a blank/whitespace token BEFORE STEP 0b asks `identity_authorization::refusal`. So a caller whose policies grant none of this service's actions — one the crate's own doc says must be "refused `AccessDeniedException`" — is answered the 400 input refusal instead of the 403 the gate would raise, purely because of the token value it happened to send. The handler's own comment above the gate claims the opposite of what the code does: the gate is described as running "ahead of the service's own reading of the request", while STEP 0 immediately above it IS that reading.

The reason STEP 0 states for itself is only about the LOOKUP/verdict path ("refused as the validation failure it is BEFORE any lookup: the verdict path below answers for a token the caller actually presented") — it states no reason for preceding the authorization gate, and the sibling handler shows the crate's intended order for a handler-level request-form refusal. The result is that the two faults the crate says an operator must be able to tell apart (a missing GRANT vs. a bad request) are reported in the wrong class for this one input.

## Evidence

THE RULE THIS HANDLER STATES FOR ITSELF — services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/get_latest_configuration.rs:32-35:
```
//! Whether the CALLER may poll at all is decided first, against its own identity-based
//! IAM policies ([`crate::state::identity_authorization`]), as AWS decides it: a caller
//! granted none of this service's actions is refused `AccessDeniedException` rather than
//! told anything about the session or the configuration behind the token.
```

WHAT THE CODE DOES — src/ops/get_latest_configuration.rs:304-317 (STEP 0 runs before STEP 0b, and STEP 0b's own comment asserts the order the code does not take):
```
        // STEP 0 — a request that supplies no usable value for the token member supplied
        // no token, and is refused as the validation failure it is BEFORE any lookup: the
        // verdict path below answers for a token the caller actually presented. ...
        if token.trim().is_empty() {
            return Err(configuration_token_required());
        }

        // STEP 0b — whether the CALLER may poll at all, decided against its own
        // identity-based IAM policies before any verdict about the token's state: AWS
        // decides authorization at the IAM layer, ahead of the service's own reading of
        // the request ([`crate::state::identity_authorization`]).
```
and the refusal that STEP 0 returns — src/ops/get_latest_configuration.rs:261-272:
```
fn configuration_token_required() -> GetLatestConfigurationError {
    ...
    GetLatestConfigurationErrorModeled::BadRequestException(BadRequestException {
        message: Some(CONFIGURATION_TOKEN_REQUIRED_MESSAGE.to_string()),
```

THE SIBLING HANDLER IN THIS CRATE DOES THE OPPOSITE — src/ops/start_configuration_session.rs:18-23:
```
//! Whether the CALLER may open a session at all is decided first, against its own
//! identity-based IAM policies ([`crate::state::identity_authorization`]): AWS decides
//! authorization ahead of the service's reading of the request, so a caller granted none
//! of this service's actions is refused `AccessDeniedException` rather than told anything
//! about the identifiers it named
```
and src/ops/start_configuration_session.rs:266-297 — the gate first, the handler-level request-FORM refusal second:
```
        // AWS decides authorization at the IAM layer, ahead of the service looking at the
        // request's own input, so this is the FIRST thing this handler does ...
        if let Some(denial) = crate::state::identity_authorization::refusal(
            ...
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        // The FORM of each identifier is decided on the request alone, ahead of every
        // lookup: ...
        if let Some(refusal) = malformed_identifier_refusal(&input) {
            return Err(refusal);
        }
```

TEST COVERAGE: the authorization suite states the same rule — tests/all/identity_authorization.rs:5-14: "AWS decides such a call against that principal's identity-based IAM policies before the service reads the request … A refusal is `AccessDeniedException` with HTTP 403" — and its rows vary the token only between "resolves to a session" and "resolves to no session" (:20-25), never a blank value, so no test pins the current order. tests/all/token_validation_and_lifetime.rs:69-104 exercises the blank-token rows (`configuration_token=%20`, `%09`, `%0A`, `+`, empty) but always under the harness's single authorized key, so both orders satisfy it.

## Steps to Reproduce

1. Boot the emulator with AppConfigData, the AppConfig control plane, IAM and STS on one shared engine (as tests/all/identity_authorization.rs::Emulator does).
2. Create an IAM principal whose identity policies grant none of this service's actions (no `appconfig:GetLatestConfiguration` statement), and obtain a credential for it, exactly as that suite's no-policy row does.
3. Signed as that principal, call GetLatestConfiguration with a token value that resolves to a session: observed 403 AccessDeniedException — the gate decides the call.
4. Signed as the same principal, call `GET /configuration?configuration_token=%20` (a single space, i.e. no usable token value).
5. Observed: 400 BadRequestException with Message "ConfigurationToken must be provided. Use the StartConfigurationSession API to obtain an InitialConfigurationToken" — the handler's own STEP 0 answered before the gate ran.
6. Expected per src/ops/get_latest_configuration.rs:32-35 and :314-317, and per the order the sibling handler takes (src/ops/start_configuration_session.rs:266-297): 403 AccessDeniedException, the same answer the unauthorized caller gets for every other value it could put in that parameter.
7. Contrast control: signed as the same unauthorized principal, POST /configurationsessions with an ARN-shaped ApplicationIdentifier — answered 403 AccessDeniedException, because that handler runs the gate before its own request-form refusal.
