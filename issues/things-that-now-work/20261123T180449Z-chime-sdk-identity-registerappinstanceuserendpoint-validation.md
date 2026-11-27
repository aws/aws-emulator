Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 33ea92ed4461881d8f7ae2a85609594a4bd6a1fc

# A reused register ClientRequestToken carrying a different AppInstanceUserArn is accepted and mints a second endpoint, although AppInstanceUserArn is a member the register persists

_Filed: 2026-11-23T18:04:49Z_
_Operation: RegisterAppInstanceUserEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `33ea92ed4461881d8f7ae2a85609594a4bd6a1fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegisterAppInstanceUserEndpoint` decides token reuse from two pieces of state: the lookup `endpoint_registered_with(user_arn, token)`, which is SCOPED to the endpoints of the one `AppInstanceUserArn` the path names, and `endpoint_request_fingerprint(..)`, whose field list omits `AppInstanceUserArn` altogether. The consequence is that the one request member the handler cannot see is the one it is keyed on: presenting the same `ClientRequestToken` again under a DIFFERENT `AppInstanceUserArn` is not recognised as a reuse at all — the lookup finds nothing under the new user, so the handler mints a SECOND endpoint and answers 201. One token therefore names two live endpoints.

That contradicts three statements in the crate itself:

1. The handler's own comment directly above the lookup says the opposite rule: "the same token presented with different parameters is a mismatched reuse rather than a retry, and is refused with the modelled conflict". `AppInstanceUserArn` is a parameter of the request (a `@httpLabel` member of `RegisterAppInstanceUserEndpointRequest`) and it IS persisted on the record (`AppInstanceUserEndpointRecord.app_instance_user_arn`), reported back by `DescribeAppInstanceUserEndpoint` and by the endpoint summary.
2. The fingerprint helper's doc claims to cover exactly that set: "every member the register call persists". It does not — `app_instance_user_arn` is persisted and is not among the six fields hashed.
3. The crate's sibling creates do include the parent ARN in their fingerprint, so the analogous reuse there IS refused: `CreateAppInstanceUser` and `CreateAppInstanceBot` both hash `input.app_instance_arn` and key the token globally per operation (`idempotency_key(operation, token)`), so the same token presented under a different parent earns the modelled `ConflictException`. Register is the only create-style operation in this crate whose token is scoped per-parent rather than per-operation.

This is not a complaint about the token's STORAGE location (holding it on the endpoint record, since an endpoint has no ARN, is justified in src/state.rs:21-26 and is accepted here); it is that the mismatch CHECK the handler claims to perform silently excludes a persisted member, so a conflict the crate words and declares becomes a 201 that creates a resource the caller did not ask for.

## Evidence

The handler states the rule it does not implement — services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/register_app_instance_user_endpoint.rs:192-214:
```
        // Idempotency (`@idempotencyToken` on `ClientRequestToken`): a retry with the
        // same token answers with the endpoint the first call registered and registers
        // no second one. A token stands for one REQUEST, so the same token presented
        // with different parameters is a mismatched reuse rather than a retry, and is
        // refused with the modelled conflict instead of answering about an endpoint the
        // caller did not ask for.
        if let Some(registered) = state
            .endpoint_registered_with(&input.app_instance_user_arn, &input.client_request_token)
        {
            if registered.request_fingerprint == fingerprint {
```
The lookup is scoped to the ONE user the path names, so a different `AppInstanceUserArn` sees no reuse — src/state.rs:370-383:
```
    /// The endpoint a previous register under `user_arn` already minted for `token`.
...
    pub fn endpoint_registered_with(
        &self,
        user_arn: &str,
        token: &str,
    ) -> Option<&AppInstanceUserEndpointRecord> {
        self.user(user_arn)?
            .endpoints
            .values()
            .find(|endpoint| endpoint.client_request_token == token)
    }
```
The fingerprint claims to cover every persisted member but omits `AppInstanceUserArn` — src/state.rs:972-989:
```
/// The fingerprint of the register request an endpoint was minted by: every member the
/// register call persists, through [`request_fingerprint`].
pub fn endpoint_request_fingerprint(
    endpoint_type: &str,
    resource_arn: &str,
    attributes: &EndpointAttributes,
    name: Option<&str>,
    allow_messages: Option<&str>,
) -> String {
    request_fingerprint(&[
        Some(endpoint_type),
        Some(resource_arn),
        Some(attributes.device_token.as_str()),
        attributes.voip_device_token.as_deref(),
        name,
        allow_messages,
    ])
}
```
yet it IS persisted and reported — src/state.rs:152-155 (`pub struct AppInstanceUserEndpointRecord { ... pub app_instance_user_arn: String,`), written from the input at register_app_instance_user_endpoint.rs:255-257 (`app_instance_user_arn: input.app_instance_user_arn.clone(),`) and reported at describe_app_instance_user_endpoint.rs:213 (`app_instance_user_arn: Some(record.app_instance_user_arn.clone()),`).

The sibling creates in THIS crate do hash the parent ARN, so the equivalent reuse is refused there — src/ops/create_app_instance_user.rs:214-231:
```
        let fingerprint = {
            let mut fields: Vec<Option<&str>> = vec![
                Some(input.app_instance_arn.as_str()),
                Some(input.app_instance_user_id.as_str()),
```
(the same `Some(input.app_instance_arn.as_str())` at src/ops/create_app_instance_bot.rs:207-208), and the token key is per-OPERATION, not per-parent — src/state.rs:651-656:
```
/// The key an idempotency token is stored under. Prefixed with the operation name so
/// `CreateAppInstance` and `CreateAppInstanceUser` can be called with the same token
/// (they are different requests) without either replaying the other's ARN.
fn idempotency_key(operation: &str, token: &str) -> String {
```

Test coverage: the suite pins the very rule that is broken, and its own comment claims the table is exhaustive over persisted members — tests/all/user_endpoint_registration.rs:558-560:
```
/// RULE: a `ClientRequestToken` stands for ONE request, so presenting it again with any
/// different parameter is refused `ConflictException` (HTTP 409) and leaves the endpoint
/// that token already registered exactly as it was.
```
and tests/all/user_endpoint_registration.rs:576-577:
```
    // Rows: the same token with exactly one member changed — one row per member a
    // register persists.
```
The seven rows that follow (lines 578-615) change `Type`, `ResourceArn`, `DeviceToken`, `VoipDeviceToken`, `Name`, an absent `Name` and `AllowMessages` — every call in the loop goes to the SAME `user_arn` (line 620, `register(&emulator, &user_arn, body)`), so no row varies `AppInstanceUserArn` and nothing pins the current acceptance.

## Steps to Reproduce

1. POST /app-instances {"Name":"ep","ClientRequestToken":"tok-inst"} -> 201, AppInstanceArn A
2. POST /app-instance-users {"AppInstanceArn":A,"AppInstanceUserId":"alice","Name":"Alice","ClientRequestToken":"tok-alice"} -> 201, A/user/alice
3. POST /app-instance-users {"AppInstanceArn":A,"AppInstanceUserId":"bob","Name":"Bob","ClientRequestToken":"tok-bob"} -> 201, A/user/bob
4. POST /app-instance-users/{A/user/alice}/endpoints {"Type":"APNS","ResourceArn":"arn:aws:mobiletargeting:us-east-1:000000000000:apps/00000000000000000000000000000011","EndpointAttributes":{"DeviceToken":"d1"},"ClientRequestToken":"tok-reuse"} -> 201 with EndpointId E1
5. POST /app-instance-users/{A/user/bob}/endpoints with the IDENTICAL body and the SAME "ClientRequestToken":"tok-reuse" -> observe 201 and a second EndpointId E2, instead of the 409 ConflictException "The provided ClientRequestToken is already associated with an existing resource." the handler's comment and tests/all/user_endpoint_registration.rs:558-560 state for a token presented again with a different parameter
6. GET /app-instance-users/{A/user/alice}/endpoints/{E1} and GET /app-instance-users/{A/user/bob}/endpoints/{E2} -> both 200: one ClientRequestToken now names two live endpoints
7. Contrast the sibling create: POST /app-instance-users {"AppInstanceArn":<a second app instance B>,"AppInstanceUserId":"alice","Name":"Alice","ClientRequestToken":"tok-alice"} -> 409 Conflict, because create_app_instance_user.rs:215-216 hashes the parent ARN into the fingerprint and src/state.rs:654-656 keys the token per operation rather than per parent
