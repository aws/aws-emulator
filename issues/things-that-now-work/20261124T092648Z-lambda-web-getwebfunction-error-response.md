Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cb1793cf7c5a4b5a69e7dca7c8a291b52f4b88d5

# The documented 64-character limit on the function-name form is applied only by CreateWebFunction, so addressing a stored function by an over-long name answers 404 instead of ValidationException

_Filed: 2026-11-24T09:26:48Z_
_Operation: GetWebFunction | Class: error-response_

> _Note: this issue was identified against the code base at commit `cb1793cf7c5a4b5a69e7dca7c8a291b52f4b88d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`FunctionName` is modelled `@length(1..256)`, and every member that ADDRESSES a function documents the narrower rule for the name form: "If you specify only the function name, it is limited to 64 characters in length." The crate already implements that exact rule once — `crate::state::FUNCTION_NAME_MAX_CHARS = 64` and `crate::state::function_name_violation` — with a doc comment explaining that the boundary validator cannot enforce it because the model carries no pattern for it. But the ONLY caller is `CreateWebFunction`.

Every path that addresses an already-stored function by name hands the raw value to `crate::state::resolve_function_name` and then to the store lookup, with no name-rule check: `GetWebFunction`, `DeleteWebFunction`, `CreateWebFunctionRevision`, `GetWebFunctionRevision`, `ListWebFunctionRevisions`, `GetWebFunctionEndpoint`, `CreateWebFunctionEndpoint`, `UpdateWebFunctionEndpoint`, `DeleteWebFunctionRevision`, and the tag / resource-policy paths. A 65..256-character value is therefore not a malformed parameter to those handlers: it is simply a name the store does not hold, so the request earns `404 ResourceNotFoundException` ("The function <name> does not exist.") where the same value on the create path earns `400 ValidationException` — the error class the model documents for a parameter that is "missing, malformed, or outside the allowed range". `GetWebFunction`'s own module doc header states the rule it then does not apply.

The same gap exists for the endpoint-name form: `ENDPOINT_NAME_MAX_CHARS` (64) is checked only through `endpoint_config_violation` on the two CREATE paths, never when `GetWebFunctionEndpoint` / `UpdateWebFunctionEndpoint` resolve an endpoint by name.

## Evidence

Smithy (`smithy_query(query="doc", shape="GetWebFunctionRequest", member="functionName")`): "The web function to retrieve. You can specify either of the following: The function name, for example my-function. The function ARN. The length constraint applies only to the full ARN. **If you specify only the function name, it is limited to 64 characters in length.**" (the identical sentence appears on `UpdateWebFunctionEndpointRequest$functionName`/`$endpointName`, `DeleteWebFunctionRevisionRequest`, etc.)

The handler quotes the rule in its OWN doc header, services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/get_web_function.rs:51-54:
```
//! * `functionName` (FunctionName, len 1..256) **required** — The web function to retrieve.
//!   You can specify either of the following: The function name, for example my-function. The
//!   function ARN. The length constraint applies only to the full ARN. If you specify only
//!   the function name, it is limited to 64 characters in length.
```
…and never applies it, services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/get_web_function.rs:170-182:
```
        // The caller may name the function by name or by ARN; both resolve to the
        // one stored record.
        let name = crate::state::resolve_function_name(&input.function_name);
        let Some(function) = state.functions.get(name) else {
            return Err(GetWebFunctionErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::function_not_found_message(&input.function_name),
```

The rule and the constant the crate already has, services/service-com-amazonaws-lambdaweb-lambdaweb/src/state.rs:106-114:
```
/// The longest `functionName` the service accepts.
///
/// The model constrains `FunctionName` to 1..256, but the member's own
/// documentation states the narrower rule this emulator enforces: "The name can
/// contain letters, numbers, hyphens (-), and underscores (_), and can't begin or
/// end with a hyphen or an underscore. The name is limited to 64 characters in
/// length." ...
pub const FUNCTION_NAME_MAX_CHARS: usize = 64;
```
and services/service-com-amazonaws-lambdaweb-lambdaweb/src/state.rs:674-676:
```
pub fn function_name_violation(name: &str) -> Option<String> {
    resource_name_violation("functionName", name, FUNCTION_NAME_MAX_CHARS)
}
```

The sibling path that DOES apply it, services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/create_web_function.rs:239-247:
```
        // The model bounds `functionName`'s length only; the member's own
        // documentation states the narrower rule the service enforces, so it is
        // refused here rather than at the boundary.
        if let Some(message) = crate::state::function_name_violation(&input.function_name) {
            return Err(
                CreateWebFunctionErrorModeled::ValidationException(ValidationException { message })
```
`grep` confirms `function_name_violation` has exactly one caller (create_web_function.rs:242) and `endpoint_config_violation` only two (create_web_function.rs:283, create_web_function_endpoint.rs:417/458).

Test coverage: the create-path rule is pinned by tests/all/web_function_lifecycle.rs:465-512 (`a_name_outside_the_documented_rule_is_refused_and_stores_nothing`, row `&too_long` = 65 chars → 400 ValidationException) and the endpoint one by tests/all/web_function_endpoint.rs:1484 (`let long_name = "e".repeat(65);`). No test addresses an EXISTING resource by an over-long name, so nothing pins the 404 these readers answer.

## Steps to Reproduce

1. Start the emulator with an empty store
2. GET /2025-03-07/web-functions/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee (65 'e' characters — inside the modelled @length(1..256), past the documented 64-character name limit)
3. Observe 404 ResourceNotFoundException "The function eee… does not exist." instead of the 400 ValidationException the member's documented length rule calls for
4. PUT /2025-03-07/web-functions with {"functionName": "<the same 65-character name>"} and observe 400 ValidationException — the two paths judge the same value differently
5. Repeat with GET /2025-03-07/web-functions/{functionName}/endpoints/<65-character endpointName> for the endpointName form
