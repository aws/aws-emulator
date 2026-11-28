Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-batch-get-invoice-profile
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-batch-get-invoice-profile
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.__type
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.message
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|status
Issue-Commit: d9773982178bffc206ea9531196a8841a020d5e3

# BatchGetInvoiceProfile ignores cross-account authorization and returns 200 instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `d9773982178bffc206ea9531196a8841a020d5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetInvoiceProfile

## Description
Cause 1 of 1: BatchGetInvoiceProfile performs no authorization check on the account ids it is asked about, so a request that AWS refuses is served as a successful lookup.

AWS: BatchGetInvoiceProfile takes a required AccountIds list (Smithy: BatchGetInvoiceProfile.AccountIds -> AccountIdList, required, @length min 1 max 1000) and models AccessDeniedException as a client error with httpError 403 (Smithy: BatchGetInvoiceProfile errors = AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); AccessDeniedException doc: "You don't have sufficient access to perform this action."). When the caller is not permitted to read invoice profiles for a requested account, AWS rejects the whole call rather than returning a partial or empty Profiles list: HTTP 403 with the AWS JSON error discriminator "__type": "AccessDeniedException" and a message of the form "Account <caller-account-id> is not authorized to access resources for account <requested-account-id>". In that message the fixed text is "Account ", " is not authorized to access resources for account " — the two account ids are variable parts taken from the signing credentials of the request and from the AccountIds entry in the request body respectively (the recorded step shows them both as 000000000000, which is this environment's account value and is not part of the requirement). Status 403 and the error code AccessDeniedException are taken from the model; the message wording is recorded behavior only.

Emulator: the replayed step returns HTTP 200 with a success body. Neither "$.__type" nor "$.message" is present in the response at all (replay detail: "status: expected 403, got 200; $.__type: missing (expected 'AccessDeniedException', got None); $.message: missing (expected 'Account 000000000000 is not authorized to access resources for account 000000000000', got None)"). The authorization decision is simply never made, and no AccessDeniedException is ever raised by this operation.

Why: a client that relies on AWS's behavior cannot distinguish "you may not read this account's invoice profile" from "this account has no invoice profile". SDK error handling for AccessDeniedException never fires, retry/escalation logic that keys on 403 is dead, and code under test that is supposed to prove it handles a cross-account denial passes against the emulator while failing against AWS. Worse, tests written to assert that an unauthorized account cannot read another account's billing profile silently assert nothing.

Fix: the BatchGetInvoiceProfile handler for the invoicing service — it needs the authorization gate that compares each requested AccountIds entry against the caller's account before the lookup runs, and must map a refusal onto the modeled AccessDeniedException (HTTP 403, "__type": "AccessDeniedException") with the message template above, rather than falling through to the success serializer.

Traces: invoicing-batch-get-invoice-profile#task#1.

## Evidence
Fixture invoicing-batch-get-invoice-profile, step task#1 (operation BatchGetInvoiceProfile, service invoicing).

Expected (recorded AWS response): HTTP 403, body containing "__type": "AccessDeniedException" and "message": "Account 000000000000 is not authorized to access resources for account 000000000000".

Actual (emulator): HTTP 200; "$.__type" missing; "$.message" missing.

Replay verdict: mismatch; divergence_key service=invoicing|BatchGetInvoiceProfile|status=403->200|keys=$.__type,$.message,status. Modeled error set for the operation includes AccessDeniedException with httpError 403, so the 403 is a modeled outcome the emulator never produces.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an invoicing client at its data listener.
2. Call BatchGetInvoiceProfile with an AccountIds list naming an account the caller is not authorized to read invoice profiles for (as recorded in fixture invoicing-batch-get-invoice-profile, step task#1).
3. Observe HTTP 200 with a success body and no error discriminator.
4. Expected: HTTP 403 with "__type": "AccessDeniedException" and message "Account <caller-account-id> is not authorized to access resources for account <requested-account-id>".
Equivalently: replay_wire_corpus() and observe the invoicing-batch-get-invoice-profile#task#1 mismatch.
