Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-network-lifecycle-and-not-found-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-network-lifecycle-and-not-found-errors
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|GetNetwork|4xx->4xx|status
Issue-Commit: d2c660dcba0958f9f444e52348f6b67098dedf4a

# wickr GetNetwork denies an unplaceable networkId 403 ForbiddenError instead of 401 UnauthorizedError

> _Note: this issue was identified against the code base at commit `d2c660dcba0958f9f444e52348f6b67098dedf4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetwork

## Description
One net-new cause in the wickr GetNetwork refusal classification: the handler now answers EVERY store outcome it cannot serve in the permission class (403 ForbiddenError), so the one refusal this API states in the AUTHENTICATION class — a well-formed networkId the store cannot place as a network of this account — is reported with the wrong error shape and status. The three refusals this read has are not one class: AWS answers two of them 403 and this one 401, and the emulator has collapsed all three onto 403.

Cause 1 of 1: GetNetwork refuses a well-formed, unplaceable `networkId` as ForbiddenError (403) where the API answers UnauthorizedError (401) with the same sentence.
  AWS: Smithy (services/service-com-amazonaws-wickr-wickradminapi/smithy-model/com-amazonaws-wickr-wickradminapi.json) — GetNetwork is GET /networks/{networkId}, success 200, readonly; input `networkId` (NetworkId, @required, @httpLabel, @length 8..8, @pattern ^[0-9]{8}$); modelled errors BadRequestError(400), ForbiddenError(403, "Access to the requested resource is forbidden ... the authenticated user does not have the necessary permissions ... even though they are authenticated"), InternalServerError(500), RateLimitError(429), ResourceNotFoundError(404), UnauthorizedError(401, "The request was not authenticated or the authentication credentials were invalid ... the request lacks valid authentication credentials or the credentials have expired"), ValidationError(422). The model does not say which outcome picks which class, so the CLASS of each refusal is recorded behavior only, and the recording separates them: a `networkId` that is well formed (8 digits) but names no network of this account is refused HTTP 401 carrying the modelled UnauthorizedError shape — restJson1 therefore also reports `x-amzn-errortype: UnauthorizedError` — with body `{"message": "Access Denied"}`. That sentence is ENTIRELY fixed text: "Access Denied" names neither the member nor the id and has no variable part, which is how the refusal discloses nothing about whether a network with that id exists. The other two refusals of this same read are 403: a path label that is not a NetworkId at all is denied 403 ForbiddenError with the same fixed "Access Denied" (wickr-network-create-and-validation#task#3, still passing), and a network of this account that has been deleted is 403 ForbiddenError with the fixed "Network not found" (wickr-network-lifecycle-and-not-found-errors#task#7, still passing). Hypothesis: the admin API resolves the label against the caller's account before it reads anything, so an id it cannot place is answered as a credential-scope failure, while a label it rejects outright and an id it CAN place but may not show are answered as permission failures.
  Emulator: the same request is answered HTTP 403 with the ForbiddenError shape. The `crate::state::NetworkAccess::Unplaceable` arm of the handler returns ForbiddenError with ACCESS_DENIED_MESSAGE, and the file's own doc comment states the intent that UnauthorizedError (401) "is never an answer this handler reaches from the store". The message text matches, so the status and the error shape are the ONLY things that diverge: status expected 401, got 403. This is an over-correction of the resolved issue 20261123T084914Z-wickr-getnetwork-refuses-an-unplaceable-networkid-as-401.md, which read the 403 recorded for the MALFORMED label (wickr-network-create-and-validation#task#3) as the expectation for the unplaceable id and moved this arm from 401 to 403; the malformed-label denial is taken ahead of the operation in `crate::service::network_ownership::OwnershipFirst`, not in this arm, so moving this arm fixed nothing there and broke the 401 case. A resolved issue does not cover a failing step, and the replay reports this cluster unowned.
  Why: a client branches on the two classes. 401 means the credentials are missing, invalid or expired, so SDK credential providers, signing middleware and operators re-resolve or refresh credentials and retry; 403 means authenticated-but-not-permitted, an IAM/ownership problem that is not retried. Against the emulator a correctly signed read of an id this account does not hold takes the access-denied branch instead of the re-authentication branch, so the client's 401 handling and any negative test asserting AWS's non-disclosure-plus-401 behavior never run, and the three refusals of this read become indistinguishable to a caller that switches on status alone.
  Fix: the refusal classification in the GetNetwork handler — the `NetworkAccess::Unplaceable` arm must raise the modelled UnauthorizedError (401) carrying the same fixed access-denied text, while the `NetworkAccess::Deleted` arm keeps ForbiddenError (403) with the network-gone text and the boundary denial of a non-NetworkId label keeps ForbiddenError (403) with the access-denied text. The three outcomes must stay three classifications rather than one; if any other operation resolves a networkId through the same three-way `NetworkAccess` answer, its arms must be classified the same way, and the fixed sentences stay the shared constants in the state module so no operation can word them differently.
  Traces: wickr-network-lifecycle-and-not-found-errors#task#5 (cluster representative, affected_count 1). Handler: services/service-com-amazonaws-wickr-wickradminapi/src/ops/get_network.rs lines 220-230 (the Unplaceable arm returning ForbiddenError) with the intent written into the module doc at lines 22-29, and the Deleted arm at lines 214-219; the ownership-first boundary in front of the operation is wired at src/service.rs lines 141 and 184-192; shared fixed texts ACCESS_DENIED_MESSAGE / NETWORK_GONE_MESSAGE at src/state.rs lines 225-235.

No open issue covers this cause: the open wickr issues are 20261121T040104Z-wickr-createbot-stub.md, 20261121T040104Z-wickr-getbotscount-stub.md, 20261124T141730Z-wickr-updatebot-accepts-an-update-that-names-no.md and 20261126T163231Z-wickr-listbots-reports-a-uname-hash-for-a.md — CreateBot, GetBotsCount, UpdateBot and the ListBots uname projection; none of them concerns GetNetwork's refusal class and none carries this replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up: true — /__admin/health answered 200 and the data listener accepts). Summary: 46 steps, 42 passed, 4 mismatched, score 91.3%.

Fixture wickr-network-lifecycle-and-not-found-errors, step task#5, operation GetNetwork, verdict mismatch:
  status: expected 401, got 403
  affected (1): wickr-network-lifecycle-and-not-found-errors#task#5
  divergence_key: service=wickr|GetNetwork|status=401->403|keys=status

Expected: HTTP 401 carrying the modelled UnauthorizedError (restJson1 error type UnauthorizedError) with body {"message": "Access Denied"}. Actual: HTTP 403 carrying the modelled ForbiddenError (error type ForbiddenError) with that same body. Fixed vs variable parts of the message: "Access Denied" is wholly fixed text for this refusal — it interpolates no member name, no networkId and nothing from emulator state — which is why the body is the only part of the response that already agrees; the error class and the HTTP status are what diverge.

The two sibling refusals of the same operation are recorded at 403 and still pass in this run: wickr-network-create-and-validation#task#3 (non-NetworkId label -> 403 ForbiddenError, "Access Denied") and wickr-network-lifecycle-and-not-found-errors#task#7 (deleted network -> 403 ForbiddenError, "Network not found"). A fix must therefore move only the unplaceable-id arm.

Smithy evidence: com.amazonaws.wickr#GetNetwork errors include both UnauthorizedError (@error client, @httpError 401, single `message` member) and ForbiddenError (@error client, @httpError 403, single `message` member); service com.amazonaws.wickr#WickrAdminApi, restJson1.

## Steps to Reproduce
1. Boot the emulator from this checkout and POST /networks to create a network, so the store is live and the caller is authenticated exactly as for a successful read.
2. GET /networks/{networkId} for a syntactically valid NetworkId (8 digits, ^[0-9]{8}$) that this store cannot place as a network of this account — e.g. an id that was never minted.
3. Expect: HTTP 401 with x-amzn-errortype: UnauthorizedError and body {"message": "Access Denied"}.
4. Observe: HTTP 403 with x-amzn-errortype: ForbiddenError and the same body {"message": "Access Denied"}.
5. Check the two sibling refusals still answer 403: GET /networks/{bad-label} (not 8 digits) -> 403 {"message": "Access Denied"}; create a network, DELETE it, then GET it -> 403 {"message": "Network not found"}.
6. Or replay fixture wickr-network-lifecycle-and-not-found-errors, step task#5 (and task#7 plus wickr-network-create-and-validation task#3 to confirm the 403 cases did not regress).
