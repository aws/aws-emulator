Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7703f69e9c77821dec580f43e82f27cc08c90ba5

# A keyName path segment outside @length is refused as a body error, contradicting the service's own URI-vs-body refusal rule

_Filed: 2026-11-27T17:29:42Z_
_Operation: DeleteWorkspaceApiKey | Class: error-response_

> _Note: this issue was identified against the code base at commit `7703f69e9c77821dec580f43e82f27cc08c90ba5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` states one rule for how this service words a boundary refusal: a violation of a member the request carries in its BODY is answered with the unmodelled `BadRequestException` / "Invalid request body", while a violation of a member bound to the request URI keeps the modelled `ValidationException`. The rule is implemented by matching the violated member name against `URI_BOUND_MEMBERS`, whose own doc comment claims it holds "every `@httpLabel` and `@httpQuery` member of any operation" of the service.

`keyName` is missing from that array (the comment above it says it is excluded deliberately, because the refusal renderer is handed the member without the operation and `CreateWorkspaceApiKeyRequest` carries `keyName` in the body). But `DeleteWorkspaceApiKey` binds `keyName` as an `@httpLabel` with `@length(min: 1, max: 100)` — `DELETE /workspaces/{workspaceId}/apikeys/{keyName}` carries NO body at all. So a delete whose key-name path segment is 101 characters is answered 400 `BadRequestException` with the message "Invalid request body", for a request that sent no body, instead of the modelled `ValidationException` the service's own rule reserves for a URI-bound member. A client branches on the code, which is exactly why this crate replaces the code in the first place.

The same array also omits `groupId`, which its own doc comment enumerates — evidence that the list has drifted from the rule it is supposed to encode (that member belongs to a still-scaffolded operation, so it is not filed here).

## Evidence

Rule side — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/service.rs:70-80:
```
/// This is recorded behaviour, not a modelled error: Amazon Managed Grafana
/// fronts its control plane with a request validator that checks the request body
/// against the operation's schema and refuses one that fails it with the
/// unmodelled code `BadRequestException` and the message "Invalid request body" —
/// the message names the body because the body is what it validated. A member
/// bound to the URI (a `@httpLabel` path segment, a `@httpQuery` parameter) is not
/// part of that body, so a refusal of one keeps the modelled `ValidationException`
/// the service itself raises.
```
List doc — src/service.rs:84-86: "The members this service carries in the request URI rather than in the body: every `@httpLabel` and `@httpQuery` member of any operation of `com.amazonaws.grafana#AWSGrafanaControlPlane`."

Code side — src/service.rs:98-113:
```
/// `keyName` is deliberately NOT here: `CreateWorkspaceApiKeyRequest` carries it
/// in the BODY, and the refusal renderer is handed the violated member without the
/// operation, so the member is classified by the binding it has where a request
/// body carries it at all.
const URI_BOUND_MEMBERS: [&str; 10] = [
    "workspaceId",
    "licenseType",
    "serviceAccountId",
    "tokenId",
    "resourceArn",
    "maxResults",
    "nextToken",
    "tagKeys",
    "userId",
    "userType",
];
```
and src/service.rs:152-165 — a member absent from that array falls through to `Some(Refused { code: BAD_REQUEST_CODE.to_string(), message: INVALID_REQUEST_BODY.to_string() })`.

Model side — `smithy_query(query="operation", operation="DeleteWorkspaceApiKey")`: `"keyName": {"target": "ApiKeyName", "httpLabel": true, "required": true, "length": {"min": 1, "max": 100}}`, `"http": {"method": "DELETE", "uri": "/workspaces/{workspaceId}/apikeys/{keyName}"}`, errors include `ValidationException` (400). Mirrored in OPERATIONS.md:66.

Test coverage — tests/all/request_body_refusals.rs:304-312 states the same rule ("A value bound to the request URI is not part of the body the validator checks, so its refusal keeps the modelled `ValidationException` and the member's own constraint message") but its rows cover only `workspaceId` (@httpLabel) and `maxResults` (@httpQuery); no row sends an over-long `keyName`, so nothing pins the current answer.

## Steps to Reproduce

1. Boot the emulator with this service registered.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} and take the workspace id.
3. Send DELETE /workspaces/<id>/apikeys/<a 101-character key name> with no request body.
4. Observe HTTP 400 with __type "BadRequestException" and message "Invalid request body", although the request carried no body and the violated member is an @httpLabel.
5. Compare with DELETE /workspaces/not-a-workspace (workspaceId label), which keeps the modelled ValidationException per the rule in src/service.rs:70-80.
