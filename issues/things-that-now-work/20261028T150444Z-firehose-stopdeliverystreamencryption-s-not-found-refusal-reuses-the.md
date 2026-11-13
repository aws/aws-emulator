Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.stop-encryption-nonexistent-stream-not-found
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.stop-encryption-nonexistent-stream-not-found|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# firehose: StopDeliveryStreamEncryption's not-found refusal reuses the control-plane-read sentence, so it reads "Firehose under account <id> not found." where AWS writes "Firehose under accountId <id> not found" (1 cause)

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeliveryStreamEncryption

## Description
Cause 1 of 1: Firehose does NOT word its not-found sentence identically across the control plane, but the emulator renders every control-plane not-found from one shared helper. StopDeliveryStreamEncryption on an absent delivery stream is therefore answered with DescribeDeliveryStream's sentence instead of its own.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json): StopDeliveryStreamEncryption takes DeliveryStreamName (@required, @length 1..64, @pattern ^[a-zA-Z0-9_.-]+$) and models InvalidArgumentException, LimitExceededException, ResourceInUseException and ResourceNotFoundException (com.amazonaws.firehose#ResourceNotFoundException, errorKind client, HTTP 400, doc "The specified resource could not be found."; its only member is `message`). The model does not fix message text, so the message is recorded behavior only: for a DeliveryStreamName that resolves to no stream, this operation answers HTTP 400 / Error.Code ResourceNotFoundException with the message template

    Firehose under accountId <accountId> not found

Its FIXED text is "Firehose under accountId " + " not found" — the noun is "accountId" and there is NO terminating period — and its single VARIABLE part is the 12-digit account id the lookup was scoped to (taken from the caller's credentials, not from the request; the recorded run fills it with the emulator's account, which is an environment value and not part of the template). This is deliberately NOT the same sentence as the control-plane READ path: DescribeDeliveryStream / DeleteDeliveryStream / ListTagsForDeliveryStream / TagDeliveryStream answer "Firehose under account <accountId> not found." (noun "account", terminal period) — those checks pass today — and the data-plane PutRecord path answers "Firehose not found under account <accountId>." Three distinct sentences, one per path.

Emulator: the StopDeliveryStreamEncryption handler's stream lookup miss constructs ResourceNotFoundException from the shared read-path helper, so the refusal carries "Firehose under account <accountId> not found." — "account" instead of "accountId", plus a terminal period AWS does not send here. Error code (ResourceNotFoundException) and status (400) are correct; only the message diverges. Replay: expected 'Firehose under accountId 000000000000 not found', got 'Firehose under account 000000000000 not found.'

Context for a fixer: two OPEN issues describe the read-path wording — 20261028T071710Z-firehose-describedeliverystream-s-resourcenotfoundexception-message-is-misworded-firehose.md and Cause 1 of 20261028T075033Z-firehose-3-net-new-root-causes-the-resourcenotfoundexception.md. Neither covers this check (the replay report marks it neither ALREADY FILED nor OWNED), and both describe the fix as a change to the ONE shared helper that "StopDeliveryStreamEncryption" also reaches. That framing is what produces this divergence: correcting the shared sentence for the read path is right, but StopDeliveryStreamEncryption must NOT pick it up — its sentence is the "accountId"/no-period one. Do not re-break the read path to satisfy this check; the two sentences must coexist.

Why: the message is a requirement of the refusal. A client, CLI user, log-scraping/alerting rule or integration test that string-matches AWS's wording to recognise "that encryption target does not exist" sees text real Firehose never emits for this operation, and because code and status are right the divergence is invisible to code-based branching — it surfaces only where the message itself is the signal. It also erases a real distinction in AWS's surface (per-path wording), so anyone deriving the read-path sentence from the encryption path, or vice versa, is misled.

Fix: the StopDeliveryStreamEncryption handler's not-found path must stop calling the shared control-plane-read helper and emit this operation's own sentence ("Firehose under accountId {account_id} not found", no terminal period) — i.e. the service's refusal-wording module needs a second not-found template, selected by path, exactly as it already distinguishes the encryption-blocked and already-disabled refusals. The account id must stay the emulator's canonical account, never a literal.

Traces: firehose.stop-encryption-nonexistent-stream-not-found. Code citations: services/service-com-amazonaws-firehose-firehose-20150804/src/ops/stop_delivery_stream_encryption.rs lines 171-182 (the lookup miss calling `crate::service::not_found_message()`); services/service-com-amazonaws-firehose-firehose-20150804/src/service.rs lines 531-538 (`not_found_message()`, currently formatting "Firehose under account {} not found.").

## Evidence
Check firehose.stop-encryption-nonexistent-stream-not-found (group error-semantics, SERVICES=["firehose"]) calls stop_delivery_stream_encryption(DeliveryStreamName=<name never created>) and asserts, in order:
1. Error.Code == "ResourceNotFoundException" — PASSES (emulator returns that code at HTTP 400).
2. Error.Message == f"Firehose under accountId {ctx.account} not found" — FAILS.

Verbatim replay output:
  AssertionError: expected message 'Firehose under accountId 000000000000 not found', got 'Firehose under account 000000000000 not found.'

Expected (AWS template): fixed text "Firehose under accountId " … " not found" with NO trailing period; one variable part, the 12-digit account id of the calling credentials (000000000000 is this environment's account id — the template's fill for this run, not a requirement).
Actual (emulator): "Firehose under account " … " not found." — two divergences, both in the FIXED text: "accountId" rendered as "account", and an added sentence-final ".".

Cross-check that this is a per-operation wording defect and not the read-path one already filed: firehose.describe-nonexistent-stream-not-found, firehose.delete-nonexistent-stream-not-found, firehose.list-tags-nonexistent-stream-not-found and firehose.tag-delivery-stream-nonexistent-rejected all PASS in this run against "Firehose under account <id> not found.", i.e. the shared helper's current text satisfies the read path while diverging on the encryption path. Source confirming the single shared template: src/service.rs:533-538 `not_found_message()` -> format!("Firehose under account {} not found.", engine::emulator_account_id()), reached from ops/stop_delivery_stream_encryption.rs:177.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a botocore/aws-cli firehose client at it.
2. Call StopDeliveryStreamEncryption with a DeliveryStreamName that was never created (any name satisfying @length 1..64 / @pattern ^[a-zA-Z0-9_.-]+$), e.g.
   aws firehose stop-delivery-stream-encryption --delivery-stream-name sim2real-fh-missing-xyz
3. Read Error.Code and Error.Message from the 400 response. Code is ResourceNotFoundException (correct); Message reads "Firehose under account <accountId> not found." instead of AWS's "Firehose under accountId <accountId> not found" — "account" for "accountId", and an extra trailing period.
4. For contrast, call DescribeDeliveryStream on the same absent name: its sentence "Firehose under account <accountId> not found." is correct and must stay as it is; and PutRecord on the same name answers the data-plane sentence "Firehose not found under account <accountId>.", also correct. Only the encryption path's sentence is wrong.
5. Re-replay the grounding check: firehose.stop-encryption-nonexistent-stream-not-found (it also exercises a positive control — StartDeliveryStreamEncryption to ENABLED then StopDeliveryStreamEncryption to DISABLED on a real stream — which must keep passing).
