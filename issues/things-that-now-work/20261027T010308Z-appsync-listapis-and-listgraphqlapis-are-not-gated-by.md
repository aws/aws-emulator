Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.unauthorized-list-apis appsync.access-denied-event-api-mgmt
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.unauthorized-list-apis|_stable_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: appsync.access-denied-event-api-mgmt|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Commit: 0790bb7a78fe50d7afa4bce89d8f5918d3395bcd

# AppSync ListApis and ListGraphqlApis are not gated by identity-based IAM: a zero-permission principal gets HTTP 200 and the account's full API inventory instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `0790bb7a78fe50d7afa4bce89d8f5918d3395bcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApis

## Description
Cause 1 of 1: the AppSync control-plane list operations perform no identity-based authorization, so an authenticated-but-unauthorized caller is served the real list of Apis / GraphqlApis in the account.

AWS: Smithy ListApis (GET /v2/apis, success 200, output member `apis`) and ListGraphqlApis (GET /v1/apis, success 200, output member `graphqlApis`) model only BadRequestException (400), UnauthorizedException (401) and InternalFailureException (500) — the published model does NOT enumerate the authorization refusal. Recorded behavior (both checks state it as measured against the live service in us-west-2): a principal that is authenticated but has NO identity-based policy granting the action is refused with error code `AccessDeniedException` (HTTP 403), never with a 200 carrying a partial or empty list. The message is AWS's standard implicit-deny template; its FIXED text is `User: ` … ` is not authorized to perform: appsync:` … ` on resource: ` … ` because no identity-based policy allows the appsync:` … ` action`, and its VARIABLE parts are taken from the request and from IAM state: the caller's principal ARN (an IAM user ARN, or an `arn:aws:sts::<account-id>:assumed-role/<role>/<session>` ARN for an assumed-role session), the action short name (`ListApis` / `ListGraphqlApis`), and the resource `arn:aws:appsync:<region>:<account-id>:*`. The action name appears twice and must match the operation being refused.

Emulator: the call is authorized unconditionally. Replay shows `list_apis()` from a principal provisioned with zero permissions (a throwaway IAM user with no attached policies in appsync.access-denied-event-api-mgmt; a same-account assumed role with no attached policies in appsync.unauthorized-list-apis) returning HTTPStatusCode 200 with a populated `apis` list — including the APIs the check had just created under the runner's own credentials, i.e. the unauthorized principal is handed the account's inventory (apiId, name, tags, dns HTTP/REALTIME hosts, apiArn, eventConfig). No AccessDeniedException is produced for either operation. Because this is the FIRST denial both checks assert, neither reaches its later assertions, so ListGraphqlApis' gate and the per-operation message text are unverified beyond ListApis' own evidence.

Why: a client or operator relying on AWS's authorization model gets a silent authorization bypass. Code that treats a successful ListApis as proof the caller is permitted will proceed; a least-privilege principal that must be denied instead enumerates every AppSync Event API and GraphQL API in the account together with its realtime/HTTP endpoint hostnames, which is an information disclosure. Negative-path tests and policy-regression suites that assert AccessDeniedException will pass against AWS and fail against the emulator, and a caller matching on the error code/message to drive a retry-or-escalate path never sees it.

Fix: the authorization gate in front of the appsync control-plane list handlers — ListApis (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/list_apis.rs) and ListGraphqlApis (…/src/ops/list_graphql_apis.rs) — must consult the calling principal's identity-based policy before reading state, and the shared error serializer must be able to emit AccessDeniedException with HTTP 403 and the implicit-deny message template above. This is the same missing gate already filed for neighbouring surfaces but for operations none of them names: open 20261025T022306Z-appsync-graphql-api-control-plane-operations-are-not.md covers Get/Update/DeleteGraphqlApi, open 20261025T041655Z-appsync-event-api-domain-control-plane-no-identity.md covers UpdateApi / AssociateApi / GetApiAssociation / DisassociateApi, and open 20261026T233737Z-appsync-channel-namespace-control-plane-get-update-deletechannelnamespace.md covers the channel-namespace trio; none of the three carries ListApis or ListGraphqlApis, and neither failing check here is owned by them. Fix them together if the gate is implemented centrally.

Traces: appsync.unauthorized-list-apis, appsync.access-denied-event-api-mgmt.

## Evidence
appsync.unauthorized-list-apis (verdict: mismatch):
  expected: ListApis from a same-account assumed role with zero attached permissions -> error code AccessDeniedException (HTTP 403), message `User: arn:aws:sts::<account-id>:assumed-role/<role>/<session> is not authorized to perform: appsync:ListApis on resource: arn:aws:appsync:<region>:<account-id>:* because no identity-based policy allows the appsync:ListApis action`; then the same refusal for ListGraphqlApis with `appsync:ListGraphqlApis` substituted.
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'apis': [{'apiId': ..., 'name': 'sim2real-behav-appsync-eventapi-...', 'tags': {...}, 'dns': {'REALTIME': '...appsync-realtime-api.us-west-2.amazonaws.com', 'HTTP': '...appsync-api.us-west-2.amazonaws.com'}, 'apiArn': 'arn:aws:appsync:us-west-2:000000000000:apis/...', 'eventConfig': {...}}]}
  The check's positive control passed first: the runner's own permissioned credentials DID see both freshly created APIs, so the 200 above is not an artifact of empty state — the unauthorized principal was served the real inventory.

appsync.access-denied-event-api-mgmt (verdict: mismatch):
  expected: the same AccessDeniedException/403 for ListApis issued by a throwaway IAM user created with no attached policies, as the gate-is-live precondition before asserting the same denial for GetApi, UpdateApi, DeleteApi, AssociateApi, GetApiAssociation and DisassociateApi.
  actual:   AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'apis': [{'apiId': 'knlgzqpdsejjdis7viwvij77rp', 'name': 'sim2real-behav-appsync-eventapi-...', 'apiArn': 'arn:aws:appsync:us-west-2:000000000000:apis/knlgzqpdsejjdis7viwvij77rp', ...}]}
  (the generated ids, names, hosts, account id and timestamps above are per-run values filling the template, not requirements; the requirements are the 403 status, the AccessDeniedException code and the message template.)

Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): ListApis GET /v2/apis code 200, errors = BadRequestException(400), UnauthorizedException(401), InternalFailureException(500); ListGraphqlApis GET /v1/apis code 200, same error set. The authorization refusal is recorded behavior, not modeled.

## Steps to Reproduce
1. Boot the emulator.
2. With the runner's normal credentials, create one Event API (appsync CreateApi with an eventConfig) and one GraphQL API (appsync CreateGraphqlApi with authenticationType=API_KEY). Confirm both appear in ListApis / ListGraphqlApis — the positive control.
3. Create a principal with NO permissions: either an IAM user with zero attached policies plus an access key, or an IAM role trusted by this account's root with zero attached policies, assumed via sts:AssumeRole.
4. Build an appsync client signed with that principal's credentials and call ListApis, then ListGraphqlApis.
5. Observe HTTP 200 with the account's real `apis` / `graphqlApis` lists, where AWS answers HTTP 403 AccessDeniedException with the message `User: <principal-arn> is not authorized to perform: appsync:<ListApis|ListGraphqlApis> on resource: arn:aws:appsync:<region>:<account-id>:* because no identity-based policy allows the appsync:<ListApis|ListGraphqlApis> action`.
6. Replay appsync.unauthorized-list-apis and appsync.access-denied-event-api-mgmt.
