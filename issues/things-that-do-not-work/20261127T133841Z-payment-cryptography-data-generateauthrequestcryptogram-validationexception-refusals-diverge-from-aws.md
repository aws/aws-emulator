Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-data-arqc-generate-refusals
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-data-arqc-generate-refusals
Issue-Shape-Id: com.amazonaws.paymentcryptographydata#PaymentCryptographyDataPlane
Issue-Key: payment-cryptography-data|GenerateAuthRequestCryptogram|4xx->4xx|$.message
Issue-Key: payment-cryptography-data|GenerateAuthRequestCryptogram|4xx->2xx|$.message
Issue-Key: payment-cryptography-data|GenerateAuthRequestCryptogram|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: payment-cryptography-data|GenerateAuthRequestCryptogram|4xx->2xx|status
Issue-Commit: 8d5141c03be9b925ee477a344ccf4c77417006a0

# GenerateAuthRequestCryptogram ValidationException refusals diverge from AWS

> _Note: this issue was identified against the code base at commit `8d5141c03be9b925ee477a344ccf4c77417006a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateAuthRequestCryptogram

## Description
The emulator's input-refusal path for Payment Cryptography Data GenerateAuthRequestCryptogram (POST /cryptogram/generate) diverges from AWS in two independent ways: one refusal it does raise carries the wrong message text, and one refusal it does not raise at all. 2 causes.

Cause 1 of 2 — ValidationException message for a key whose KeyUsage is not permitted for this operation (fixture payment-cryptography-data-arqc-generate-refusals, step task#5).
AWS: GenerateAuthRequestCryptogram takes a required KeyIdentifier (KeyArnOrKeyAliasType) and models ValidationException at HTTP 400 (Smithy: GenerateAuthRequestCryptogram input KeyIdentifier required, target KeyArnOrKeyAliasType @length 7..322 with the arn:aws:payment-cryptography key/alias @pattern; errors ValidationException httpError 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500). The exact refusal wording is recorded behavior only: when the referenced key exists but its KeyUsage is not one this operation accepts, AWS answers HTTP 400 with x-amzn-errortype ValidationException and the message "KeyUsage of the input key <keyArn> is invalid for the operation." The fixed text is "KeyUsage of the input key " ... " is invalid for the operation."; the single variable part is the key ARN, taken verbatim from the request's KeyIdentifier (resolved to the key ARN when an alias was supplied) — AWS names the offending KEY, not the usage value.
Emulator: same HTTP 400, but the message is "KeyUsage <KEY_USAGE> is not valid for this operation." (replay saw "KeyUsage TR31_C0_CARD_VERIFICATION_KEY is not valid for this operation."). Different fixed text, and it substitutes the key-usage enum value where AWS substitutes the key ARN, so the key identity is absent from the message entirely.
Why: clients and SDK-based tooling that match on this message, or that parse the offending resource out of it to report which key was mis-selected, see neither AWS's sentence nor the ARN. Payment-cryptography callers commonly pass an alias; without the resolved ARN in the message the operator cannot tell which key the alias resolved to, so a key-selection bug is undiagnosable against the emulator.
Fix: the error-construction path of the GenerateAuthRequestCryptogram handler's key-usage validation in the payment-cryptography-data service — build the ValidationException message from AWS's template with the resolved key ARN as the substituted value, not from the KeyUsage enum.
Traces: payment-cryptography-data-arqc-generate-refusals#task#5.

Cause 2 of 2 — TransactionData length is not validated as a multiple of 16, so a malformed request succeeds (fixture payment-cryptography-data-arqc-generate-refusals, step task#6).
AWS: TransactionData is required, target TransactionDataType, @length 2..1024, @pattern ^[0-9a-fA-F]+$, @sensitive (Smithy: GenerateAuthRequestCryptogram input TransactionData). Beyond those modeled constraints AWS enforces a block-alignment rule that the model does not state (recorded behavior only): the hex transaction data must be a whole number of 8-byte cipher blocks, i.e. its length in hex characters must be a multiple of 16. A request whose TransactionData passes the pattern and length bounds but is not block-aligned is refused with HTTP 400, x-amzn-errortype ValidationException, message "TransactionData should be of length multiple of 16." — that sentence is entirely fixed text, with no part taken from the request.
Emulator: accepts the request and answers HTTP 200 with a normal GenerateAuthRequestCryptogram success body; no status 400, no x-amzn-errortype header, no message at all. Replay: expected 400, got 200; $.message missing (expected "TransactionData should be of length multiple of 16.", got None); $headers.x-amzn-errortype missing.
Why: the misalignment is silently tolerated and a cryptogram is generated from padded or truncated input, so a client gets an AuthRequestCryptogram that a real EMV authorization host would never have produced. Negative-path tests that assert the refusal pass vacuously, and a caller whose transaction-data assembly is off by a nibble ships the bug: against AWS the request is rejected at the edge, against the emulator it returns a 200 with a plausible-looking but meaningless cryptogram.
Fix: the input validation of the GenerateAuthRequestCryptogram handler in the payment-cryptography-data service — after the modeled @length/@pattern checks on TransactionData, reject a value whose hex length is not a multiple of 16 with ValidationException (400) carrying AWS's fixed message.
Traces: payment-cryptography-data-arqc-generate-refusals#task#6.

No open issue covers either cause; the only open payment-cryptography issue (20261109T223435Z-cloudformation-payment-cryptography-cloudformation-does-not-register-aws) is about CloudFormation resource-type registration and is unrelated to the data-plane cryptogram operations.

## Evidence
Replay of fixture payment-cryptography-data-arqc-generate-refusals (trace set simparity-wire-aws-v1), service payment-cryptography-data, operation GenerateAuthRequestCryptogram.

Cause 1 — step task#5, verdict mismatch, expected 400 / actual 400:
  detail: $.message: value_mismatch (expected 'KeyUsage of the input key arn:aws:payment-cryptography:us-west-2:000000000000:key/eqloz0wehlhpl6qa is invalid for the operation.', got 'KeyUsage TR31_C0_CARD_VERIFICATION_KEY is not valid for this operation.')
  Expected message template: fixed text "KeyUsage of the input key " + <resolved key ARN from the request> + " is invalid for the operation." (the ARN above is this run's value, not a requirement).
  Actual message template: fixed text "KeyUsage " + <KeyUsage enum of the key> + " is not valid for this operation." — wrong fixed text and wrong substituted value.

Cause 2 — step task#6, verdict mismatch, expected 400 / actual 200:
  detail: status: expected 400, got 200
          $.message: missing (expected 'TransactionData should be of length multiple of 16.', got None)
          $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  Expected: HTTP 400, x-amzn-errortype: ValidationException, message exactly "TransactionData should be of length multiple of 16." (all fixed text).
  Actual: HTTP 200 with a success payload; no error type header and no message.

Smithy model consulted: services/service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane/smithy-model/com-amazonaws-paymentcryptographydata-paymentcryptographydataplane.json — GenerateAuthRequestCryptogram, http POST /cryptogram/generate (success 200); input KeyIdentifier (required, KeyArnOrKeyAliasType, @length 7..322, arn/alias @pattern), TransactionData (required, @length 2..1024, @pattern ^[0-9a-fA-F]+$, @sensitive), MajorKeyDerivationMode (required enum), SessionKeyDerivationAttributes (required); errors ValidationException (400, defaulted), ResourceNotFoundException (404), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500). The model does not state either refusal's message text or the multiple-of-16 rule; those are recorded behavior.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm the payment-cryptography-data data plane answers (POST /cryptogram/generate).
2. Replay the wire corpus (trace set simparity-wire-aws-v1), fixture payment-cryptography-data-arqc-generate-refusals.

Cause 1 (step task#5): call GenerateAuthRequestCryptogram with a KeyIdentifier naming an existing key whose KeyUsage is not accepted by this operation (e.g. a TR31_C0 card-verification key), plus otherwise valid TransactionData, MajorKeyDerivationMode and SessionKeyDerivationAttributes. Observe HTTP 400 with message "KeyUsage <usage> is not valid for this operation." instead of AWS's "KeyUsage of the input key <resolved key ARN> is invalid for the operation."

Cause 2 (step task#6): call GenerateAuthRequestCryptogram with a key of correct usage and a TransactionData hex string that satisfies @length 2..1024 and ^[0-9a-fA-F]+$ but whose character count is NOT a multiple of 16 (e.g. 18 hex characters). Observe HTTP 200 with a generated AuthRequestCryptogram, instead of AWS's HTTP 400 / x-amzn-errortype ValidationException / message "TransactionData should be of length multiple of 16."

After fixing, re-replay payment-cryptography-data-arqc-generate-refusals and confirm BOTH task#5 and task#6 pass; closing only one leaves the other divergence live.
