Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-network-create-and-validation
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-network-create-and-validation
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|GetNetwork|4xx->4xx|status
Issue-Commit: ba6912b745241f4a8c68b84730217fa4109e44cb

# wickr GetNetwork refuses an unplaceable networkId as 401 UnauthorizedError instead of 403 ForbiddenError

> _Note: this issue was identified against the code base at commit `ba6912b745241f4a8c68b84730217fa4109e44cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetwork

## Description
One net-new cause in the Wickr network read path's refusal classification.

Cause 1 of 1: GetNetwork answers the authorization refusal for a `networkId` the store cannot place as a network of this account in the WRONG error class and therefore the wrong HTTP status.
  AWS: GetNetwork models both `ForbiddenError` (@httpError 403 — "Access to the requested resource is forbidden ... the authenticated user does not have the necessary permissions to perform the requested operation, even though they are authenticated") and `UnauthorizedError` (@httpError 401 — "The request was not authenticated or the authentication credentials were invalid ... the request lacks valid authentication credentials or the credentials have expired") (Smithy: com.amazonaws.wickr#GetNetwork errors; com.amazonaws.wickr#ForbiddenError httpError 403; com.amazonaws.wickr#UnauthorizedError httpError 401; service WickrAdminApi, restJson1). The recorded wire shows that a GET of a networkId this authenticated caller cannot be shown to own is refused in the PERMISSION class: HTTP 403 with the ForbiddenError shape (restJson1 therefore also reports `x-amzn-errortype: ForbiddenError`). The refusal body's message is fixed text that names neither the member nor the id — it carries no variable part — so it discloses nothing about whether a network with that id exists; the emulator already serves the matching text ("Access Denied"), and only the class/status is wrong.
  Emulator: the same request is answered HTTP 401 with `UnauthorizedError`. `GetNetwork::handle` maps `crate::state::NetworkAccess::Unplaceable` to `UnauthorizedError` (services/service-com-amazonaws-wickr-wickradminapi/src/ops/get_network.rs lines 211-216, with the intent written into the file's own doc comment at lines 18-20), so every id the store cannot place is reported as an authentication failure rather than an authorization one.
  Why: a client distinguishes the two classes. 401 means "your credentials are missing/expired", so SDKs, credential providers and middleware respond by re-resolving or refreshing credentials and retrying, and operators read it as a signing or expiry problem; 403 means "these credentials are authenticated but not permitted", which is an IAM/ownership problem and is not retried. Against the emulator a correctly signed request for an unknown network id drives the credential-refresh path and surfaces as an auth-configuration failure instead of the access denial AWS reports.
  Fix: the refusal classification in the GetNetwork handler — the `NetworkAccess::Unplaceable` arm must raise the modeled `ForbiddenError` (403) with the same fixed access-denied text, keeping `UnauthorizedError` (401) for genuinely unauthenticated/invalid-credential requests. If other operations share this three-way resolution (crate::state::NetworkAccess / network_ownership), the same arm must be classified consistently there.
  Traces: wickr-network-create-and-validation#task#3 (GetNetwork, expected 403, got 401).

No open issue covers this cause: the GetNetwork/ListNetworks read-path issue (20261109T141502Z-wickr-getnetwork-and-listnetworks-network-records-drop-tags.md) is about the success-body record (tags, standing, migrationState, null members) and the deleted-network issue (20261122T142729Z-wickr-createbot-duplicate-username-reason-re-worded-and.md) is about GetBot under a deleted network; neither carries this step's replay key and the replay reports it unowned.

## Evidence
wickr-network-create-and-validation#task#3 (GetNetwork): status: expected 403, got 401 — the only asserted divergence on the step (divergence_key service=wickr|GetNetwork|status=403->401|keys=status, affected_count 1). Expected: HTTP 403 carrying the modeled ForbiddenError (restJson1 error type ForbiddenError) with the fixed access-denied sentence. Actual: HTTP 401 carrying the modeled UnauthorizedError (error type UnauthorizedError) with that same fixed sentence. The message's text is fixed for this error class and has no variable parts; the class and status are what differ.

## Steps to Reproduce
1. POST /networks to create a network (so the store is live and the caller is authenticated the same way as for a successful read).
2. GET /networks/{networkId} for a syntactically valid NetworkId (8 digits, ^[0-9]{8}$) that this store cannot place as a network of this account — e.g. an id that was never minted.
3. Observe the response status and error type: AWS answers 403 with ForbiddenError; the emulator answers 401 with UnauthorizedError (same message text).
4. Replay fixture wickr-network-create-and-validation, step task#3.
