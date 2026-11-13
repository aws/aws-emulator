Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-alias-management-v2
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-alias-management-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|CreateAlias|4xx->2xx|$.Message
Issue-Key: payment-cryptography|CreateAlias|4xx->2xx|$.__type
Issue-Key: payment-cryptography|CreateAlias|4xx->2xx|status
Issue-Key: payment-cryptography|GetAlias|4xx->2xx|$.ResourceId
Issue-Key: payment-cryptography|GetAlias|4xx->2xx|$.__type
Issue-Key: payment-cryptography|GetAlias|4xx->2xx|status
Issue-Commit: 2b1389a4a6c4df71c4118b776bf7e768de987e7c

# payment-cryptography alias registry enforces neither uniqueness nor existence

> _Note: this issue was identified against the code base at commit `2b1389a4a6c4df71c4118b776bf7e768de987e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
The emulator's alias store accepts every alias request as a success: creating an alias that already exists is not a conflict, and reading an alias that has been deleted is not a miss. Both refusals AWS makes on the alias plane are therefore unreachable. Two causes.

Cause 1 of 2: CreateAlias for an alias name that already exists returns 200 instead of refusing.
AWS: Smithy — CreateAlias is @idempotent in signature but its modeled errors include ConflictException (errorKind client, @httpError 409, single member `Message`), documented as "This request can cause an inconsistent state for the resource ... trying to create a resource that already exists"; AliasName is @required with @pattern ^alias/[a-zA-Z0-9/_-]+$ and alias names are unique per account and Region. Recorded wire response: HTTP 400 (this service reports client errors as 400, see the separate status-mapping issue) with __type com.amazonaws.paymentcryptography.exception#ConflictException and Message whose FIXED text is "Alias <aliasName> already exists." — the only VARIABLE part is the alias name echoed from the request (the recording's sim2real probe name is a fixture value, not part of the template).
Emulator: answers HTTP 200 with a success CreateAliasOutput; `$.__type` and `$.Message` are absent and no refusal occurs, so the duplicate silently overwrites or re-registers the alias.
Why: alias uniqueness is how callers safely pin a stable name to a rotating key — a client that creates an alias to claim it, expecting ConflictException when another actor already owns it, instead believes it owns an alias it may have just repointed. Any provisioning flow that treats the conflict as "already done by someone else" takes the wrong branch.
Fix: the CreateAlias handler plus the alias state in the payment-cryptography control-plane crate: the alias registry must be keyed uniquely per account/Region and the handler must refuse an existing name with ConflictException carrying the "Alias <aliasName> already exists." message.
Traces: payment-cryptography-alias-management-v2#task#7.

Cause 2 of 2: GetAlias for a deleted alias returns 200 instead of ResourceNotFoundException.
AWS: Smithy — GetAlias -> GetAliasOutput.Alias is @required, and GetAlias's errors include ResourceNotFoundException (client; member `ResourceId`, documented as "the specific resource identifier (such as a key ARN or alias name) that could not be located"). After DeleteAlias the alias no longer exists. Recorded wire response: HTTP 400 with __type com.amazonaws.paymentcryptography.exception#ResourceNotFoundException and `ResourceId` set to the alias name from the request; this error class carries ResourceId, not Message.
Emulator: answers HTTP 200 (expected 400) with neither `$.__type` nor `$.ResourceId`, i.e. it still serves the alias after it was deleted — DeleteAlias does not remove it from the store, or GetAlias does not consult deletion.
Why: a caller cannot tell a live alias from a deleted one. Cleanup verification loops that poll GetAlias until it refuses never terminate, and a client that resolves a stale alias is handed a key binding AWS would have rejected.
Fix: the alias state module and the GetAlias handler: DeleteAlias must remove the binding and GetAlias must refuse an unknown or deleted name with ResourceNotFoundException whose ResourceId is the requested alias name.
Traces: payment-cryptography-alias-management-v2#task#9.

Related but not duplicate: the two open issues "Payment Cryptography control plane: every success-path control operation refused with AmbiguousService 500" and "... modeled error responses and CreateKey replaced by AmbiguousService 500" describe the earlier router fault that refused all traffic; the service now answers, and these alias-semantics gaps are net-new behind it.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture payment-cryptography-alias-management-v2 (the fixture creates a key, creates an alias, re-creates the same alias, deletes it, then reads it back).
- Cluster service=payment-cryptography|CreateAlias|status=400->200|keys=$.Message,$.__type,status at step task#7: "status: expected 400, got 200 / $.__type: missing (expected 'com.amazonaws.paymentcryptography.exception#ConflictException', got None) / $.Message: missing (expected 'Alias alias/sim2real-probe-payment-cryptography-alias-048d463d already exists.', got None)".
- Cluster service=payment-cryptography|GetAlias|status=400->200|keys=$.ResourceId,$.__type,status at step task#9: "status: expected 400, got 200 / $.__type: missing (expected 'com.amazonaws.paymentcryptography.exception#ResourceNotFoundException', got None) / $.ResourceId: missing (expected 'alias/sim2real-probe-payment-cryptography-alias-048d463d', got None)".
The alias name in both expectations is the fixture's generated probe name: it is the value the template interpolates from the request, not a literal requirement.
Smithy: CreateAlias errors include ConflictException (client, httpError 409, member Message); GetAlias errors include ResourceNotFoundException (client, member ResourceId); GetAliasOutput.Alias is required.

## Steps to Reproduce
1. Replay fixture payment-cryptography-alias-management-v2 against the emulator.
2. At step task#7 the fixture calls CreateAlias with an AliasName it already created earlier in the same fixture: the emulator answers HTTP 200 instead of the recorded HTTP 400 ConflictException "Alias <aliasName> already exists.".
3. At step task#9, after the alias has been deleted, the fixture calls GetAlias with that name: the emulator answers HTTP 200 instead of the recorded HTTP 400 ResourceNotFoundException whose ResourceId is the alias name.
