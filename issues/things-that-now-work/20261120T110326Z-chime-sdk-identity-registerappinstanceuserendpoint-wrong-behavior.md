Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30d927f99e8e54f81e8ebf641df74b5be52ff769

# A ResourceArn in Pinpoint's apps space with an invalid application id is refused 403 instead of registering the INACTIVE/INVALID_PINPOINT_ARN endpoint the model documents

_Filed: 2026-11-20T11:03:26Z_
_Operation: RegisterAppInstanceUserEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30d927f99e8e54f81e8ebf641df74b5be52ff769`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents exactly what an invalid Pinpoint ARN supplied through `ResourceArn` produces: the endpoint IS registered and reports `EndpointState` `INACTIVE` with `StatusReason` `INVALID_PINPOINT_ARN`. The handler honours that for only ONE class of invalid value — an ARN that is not a Pinpoint application ARN at all (an SNS topic, an S3 bucket, a Pinpoint `templates/...` ARN) is stored INACTIVE with `INVALID_PINPOINT_ARN` — while the class the documented sentence describes most directly, an ARN that IS in Pinpoint's `mobiletargeting` `apps` space but whose application id is not one Pinpoint mints (`apps/<not 32 hex chars>`, or `apps` with no id), is refused with a 403 `ForbiddenException` and nothing is registered.

So the nearer the caller's `ResourceArn` gets to a real Pinpoint application ARN, the harsher the answer: `arn:aws:sns:...:push-topic` registers an endpoint, but `arn:aws:mobiletargeting:us-east-1:123456789012:apps/not-an-app-id` — an invalid pinpoint ARN input through `ResourceArn`, which is the literal trigger the model gives for `INVALID_PINPOINT_ARN` — is answered 403 with a message the model does not carry ("You are not authorized to perform this action."), and the endpoint the caller asked for does not exist. A client that follows the documented contract (register, then read `EndpointState` to learn why delivery is impossible) gets a refusal it cannot distinguish from an authorization failure, and `DescribeAppInstanceUserEndpoint`/`ListAppInstanceUserEndpoints` have nothing to report.

## Evidence

The model states the outcome for this input, `smithy_query(query="doc", shape="com.amazonaws.chimesdkidentity#AppInstanceUserEndpoint", member="EndpointState")`:

> "INACTIVE: The AppInstanceUserEndpoint is inactive and can't receive message. When INACTIVE, the corresponding reason will be conveyed through EndpointStatusReason."
> "INVALID_PINPOINT_ARN indicates that an AppInstanceUserEndpoint is INACTIVE due to an invalid pinpoint ARN that was input through the ResourceArn field."

(the same two sentences are the documentation of `com.amazonaws.chimesdkidentity#EndpointState` itself, and `EndpointStatusReason` is the enum `["INVALID_DEVICE_TOKEN", "INVALID_PINPOINT_ARN"]`)

The handler refuses instead of registering — services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/register_app_instance_user_endpoint.rs:229-251:
```
        let endpoint_state = match crate::state::pinpoint_application_of(&input.resource_arn) {
            crate::state::PinpointApplication::Resolved => EndpointState {
                status: "ACTIVE".to_string(),
                status_reason: None,
            },
            crate::state::PinpointApplication::NotAPinpointApplication => EndpointState {
                status: "INACTIVE".to_string(),
                status_reason: Some("INVALID_PINPOINT_ARN".to_string()),
            },
            crate::state::PinpointApplication::Unresolvable => {
                return Err(
                    RegisterAppInstanceUserEndpointErrorModeled::ForbiddenException(
                        ForbiddenException {
                            code: Some("Forbidden".to_string()),
                            message: Some(
                                "You are not authorized to perform this action.".to_string(),
                            ),
```

Which inputs land in that refused class — src/state.rs:910-930:
```
pub fn pinpoint_application_of(resource_arn: &str) -> PinpointApplication {
    if !PINPOINT_APPLICATION.admits(resource_arn) {
        return PinpointApplication::NotAPinpointApplication;
    }
    match ::service_common::arn::resource_id_of_type(resource_arn, "apps") {
        Some(application_id) if names_pinpoint_application_id(application_id) => {
            PinpointApplication::Resolved
        }
        // `apps` with no id at all (`apps`, no separator) names no application either.
        _ => PinpointApplication::Unresolvable,
    }
}
```
with `names_pinpoint_application_id` requiring exactly 32 lowercase-hex characters (src/state.rs:925-930), so `apps/<31 or 33 chars>`, `apps/<uppercase hex>`, `apps/<non-hex>` and `apps/` are all `Unresolvable` → 403.

Test coverage: the current 403 is pinned by tests/all/user_endpoint_registration.rs:345-350 — "an `apps` ARN that names no application Pinpoint mints is refused with the modelled `ForbiddenException`, an ARN that is not a Pinpoint application ARN leaves the endpoint registered but `INACTIVE` with `StatusReason` `INVALID_PINPOINT_ARN`" — with the rows at user_endpoint_registration.rs:378-402 ("an application id one character short", "one character long", "not hexadecimal", "in upper case", "an apps ARN naming no application" → `Resolution::Forbidden`) and the assertion of that message at line 438. That stated reason is the emulator's own split of "invalid Pinpoint ARN" into two answers; the model sentence quoted above covers precisely the rows it refuses, and gives them the INACTIVE registration the neighbouring rows (user_endpoint_registration.rs:403-418, `Resolution::Inactive`) already get.

## Steps to Reproduce

1. POST /app-instances with {"Name":"i","ClientRequestToken":"t1"} -> AppInstanceArn A
2. POST /app-instance-users with {"AppInstanceArn":A,"AppInstanceUserId":"owner","Name":"owner","ClientRequestToken":"t2"} -> AppInstanceUserArn U
3. POST /app-instance-users/{U}/endpoints with {"ClientRequestToken":"t3","Type":"APNS","ResourceArn":"arn:aws:sns:us-east-1:123456789012:push-topic","EndpointAttributes":{"DeviceToken":"d"}} -> 201; GET the endpoint reports EndpointState {Status: INACTIVE, StatusReason: INVALID_PINPOINT_ARN}
4. POST /app-instance-users/{U}/endpoints with {"ClientRequestToken":"t4","Type":"APNS","ResourceArn":"arn:aws:mobiletargeting:us-east-1:123456789012:apps/not-an-app-id","EndpointAttributes":{"DeviceToken":"d"}} -> 403 ForbiddenException "You are not authorized to perform this action.", no endpoint registered
5. Expected per the quoted model sentence: step 4 registers the endpoint and reports EndpointState {Status: INACTIVE, StatusReason: INVALID_PINPOINT_ARN}, the documented outcome for an invalid pinpoint ARN input through ResourceArn
6. GET /app-instance-users/{U}/endpoints shows only the step-3 endpoint, so the state the caller was told to read does not exist
