Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-data-card-validation-data-cvv
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-data-card-validation-data-cvv payment-cryptography-data-encrypt-decrypt-and-cmac-roundtrip payment-cryptography-data-generate-verify-mac-hmac payment-cryptography-data-sdk-refused-inputs
Issue-Shape-Id: com.amazonaws.paymentcryptographydata#PaymentCryptographyDataPlane
Issue-Key: payment-cryptography-data|VerifyCardValidationData|4xx->4xx|$.Message
Issue-Key: payment-cryptography-data|VerifyMac|4xx->4xx|$.Message
Issue-Key: payment-cryptography-data|GenerateMac|4xx->4xx|$.message
Issue-Key: payment-cryptography-data|EncryptData|4xx->4xx|$.fieldList
Issue-Key: payment-cryptography-data|EncryptData|4xx->4xx|$.message
Issue-Key: payment-cryptography-data|EncryptData|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 080e564e5d4b68250a53aa4759786176da0afd3b

# payment-cryptography-data wire refusals: 4 error-fidelity causes across VerificationFailedException, GenerateMac precedence, ValidationException formatting/fieldList and the error-type header

> _Note: this issue was identified against the code base at commit `080e564e5d4b68250a53aa4759786176da0afd3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
VerifyMac, VerifyCardValidationData, GenerateMac, EncryptData

## Description
Replaying the wire corpus simparity-wire-aws-v1 against the Payment Cryptography DATA plane produced 5 divergence clusters, all of them 400 refusals whose STATUS is right and whose BODY/HEADERS are wrong. They reduce to FOUR independent root causes in this crate's error channel. Each cause below must be closed before this issue is submitted; Cause 1 covers TWO divergence clusters (two operations sharing one constant), Causes 2-4 one each.

Related already-open issue (does NOT cover these steps): 20261031T105700Z-payment-cryptography-400-refusals-serialize-validationexception-with-no.md and the behavioral bundle 20261101T212335Z-payment-cryptography-data-5-net-new-refusal-fidelity.md describe overlapping faults from the BEHAVIORAL suite, but they carry no wire replay key, so none of the fixture steps named here is owned by them and all of them still fail on replay. Smithy model used throughout: services/service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane/smithy-model/com-amazonaws-paymentcryptographydata-paymentcryptographydataplane.json (PaymentCryptographyDataPlane, 2022-02-03, restJson1).

=== Cause 1 of 4: every VerificationFailedException carries the EXCEPTION SHAPE'S DOC STRING as its Message instead of the operation's own verification-failure sentence (2 clusters: VerifyCardValidationData, VerifyMac) ===
AWS: Smithy models VerificationFailedException as a client error, HTTP 400, with required members Reason (VerificationFailedReason) and Message; the model does not state the text, so the wording is recorded behavior only, and the recording shows AWS wording it PER VERIFICATION CLASS. VerifyCardValidationData against a corrupted validation value answers 400, VerificationFailedException, Message exactly 'Card validation data verification failed.'; VerifyMac against a message/MAC that do not agree answers 400, VerificationFailedException, Message exactly 'Mac verification failed.' Both sentences are entirely FIXED text: they interpolate nothing from the request or from state.
Emulator: both operations answer the single shared sentence 'This request failed verification.' — which is verbatim the @documentation string of the VerificationFailedException SHAPE, not a Message AWS ever returns. Status 400 and the error code match; only $.Message diverges (replay: '$.Message: value_mismatch'). One sentence is emitted for every verification class.
Why: a client that distinguishes verification outcomes by the message — MAC/PIN retry counters, log filters, operator runbooks, SDK string assertions — cannot tell a MAC failure from a card-validation failure, and the sentence it does see is a documentation string that will never match anything written against AWS.
Fix: the per-operation message belongs in each verifying operation's handler (the point where it constructs VerificationFailedException at the comparison), replacing the one shared crate-level constant with one sentence per verification class; the Reason the handler already sets is the discriminator, and the remaining reasons (INVALID_PIN, INVALID_AUTH_REQUEST_CRYPTOGRAM) need their own sentences as those operations land.
Traces: payment-cryptography-data-card-validation-data-cvv#task#4 (VerifyCardValidationData, the representative of this issue) and payment-cryptography-data-encrypt-decrypt-and-cmac-roundtrip#task#7 (VerifyMac; same cluster also covers payment-cryptography-data-generate-verify-mac-hmac#task#5). Source cited by the sibling behavioral issue: src/state.rs VERIFICATION_FAILED_MESSAGE, used from src/ops/verify_mac.rs and src/ops/verify_card_validation_data.rs.

=== Cause 2 of 4: GenerateMac applies the KeyModesOfUse gate BEFORE checking the key's algorithm against the requested MAC algorithm, so a non-MAC key is refused with the wrong sentence (and with a key ARN AWS's sentence does not contain) ===
AWS: GenerateMac requires a MAC key whose KeyModesOfUse include Generate, and declares ValidationException (HTTP 400, httpError defaulted) in its error list. The model does not word the refusal, so this is recorded behavior only: GenerateMac with GenerationAttributes {Algorithm: HMAC} against a key whose KeyAlgorithm is TDES_2KEY is refused 400, ValidationException, Message exactly 'Invalid key with KeyAlgorithm HMAC used with MacAlgorithm TDES_2KEY. Please use valid HMAC Key.' Fixed text: 'Invalid key with KeyAlgorithm ' + A + ' used with MacAlgorithm ' + B + '. Please use valid ' + A + ' Key.', where A is the MAC algorithm the REQUEST named and B the key's own KeyAlgorithm read from state. AWS evaluates this algorithm-compatibility check FIRST: the recorded key also lacks the Generate mode of use, and AWS still answers the algorithm sentence.
Emulator: it answers 400 ValidationException with 'The KeyModesOfUse of key <keyArn> do not include Generate.' — the modes-of-use gate runs before any key/MAC-algorithm compatibility check, and the sentence interpolates the resolved key ARN, which is a per-run value that AWS's template does not contain at all. Status and code match; $.message diverges.
Why: the caller is pointed at the wrong fault — a control-plane attribute it might try to widen — when the real fault is that it handed a data-encryption key to a MAC operation; a client that re-creates the key with Generate still fails. The mis-ordering also makes the algorithm-compatibility sentence unreachable for any key that also lacks the mode, i.e. the normal case.
Fix: the validation order and wording in the GenerateMac handler — compare the requested MAC algorithm against the resolved key's KeyAlgorithm/KeyUsage before the KeyModesOfUse gate, and word that refusal with AWS's template above; the sibling VerifyMac handler gates on verify() first for the same reason.
Traces: payment-cryptography-data-generate-verify-mac-hmac#task#6 (src/ops/generate_mac.rs, the `if !key.modes.generate()` gate; the algorithm check lives later in src/state/crypto.rs).

=== Cause 3 of 4: the shared constraint-validation refusal is formatted wrongly (separator, member path, echoed value) AND never populates ValidationException.fieldList ===
AWS: EncryptDataInput.EncryptionAttributes is @required (Smithy: EncryptData input member EncryptionAttributes -> EncryptionDecryptionAttributes, required: true), and ValidationException (HTTP 400) is modeled with a required `message` plus a `fieldList` of ValidationExceptionField {path, message} (both required inside the field). A request omitting the member is refused 400 ValidationException with message exactly "1 validation error detected. Value at '/EncryptionAttributes' failed to satisfy constraint: Member must not be null" and fieldList [{path: '/EncryptionAttributes', message: "Value at '/EncryptionAttributes' failed to satisfy constraint: Member must not be null"}]. Fixed text: '<N> validation error(s) detected. ' + per-field sentence; the variable parts are N, the JSON-pointer path built from the MODELED member name with its capitalisation ('/EncryptionAttributes'), and the constraint clause ('Member must not be null' here).
Emulator: it answers "1 validation error detected: Value null at 'encryptionAttributes' failed to satisfy constraint: Member must not be null" and OMITS fieldList entirely (replay: '$.fieldList: missing ... got None'). Three defects in one formatter plus one missing member: 'detected:' where AWS writes 'detected.'; the member rendered as the lower-camel wire name in bare quotes where AWS writes the '/'-prefixed modeled name; a 'Value null ' clause AWS does not emit (and for a present-but-invalid member this same formatter interpolates the rejected value, which is a disclosure defect for @sensitive members such as PlainText/CipherText); and fieldList left null, so the structured per-field detail AWS models is never served.
Why: every constraint refusal in this crate is worded by this one formatter, so no client assertion, log filter or runbook written against AWS matches for ANY member of ANY operation here, and an SDK/console that renders ValidationException.fieldList to point at the offending member shows nothing.
Fix: the shared constraint-violation formatter in the request walk's validate step (separator, '/'-prefixed modeled member path, no value echo) plus population of ValidationException.fieldList from the collected violations; the operations' own ValidationException constructors hard-code field_list: None and must carry the field list through.
Traces: payment-cryptography-data-sdk-refused-inputs#task#2 (EncryptData with EncryptionAttributes omitted; src/ops/encrypt_data.rs:269-275 builds every ValidationException with field_list: None).

=== Cause 4 of 4: EncryptData's 400 refusal of a deserialization-level bad input announces x-amzn-errortype: ValidationException, which is not the error type the recorded refusal carries ===
AWS: recorded behavior only — the recording's x-amzn-errortype for this step is shape-asserted, and the emulator's value does not conform to it; the sibling step in the same fixture (Cause 3, a pure constraint violation) matches, so the recorded error type for THIS input is a different one, not merely a different spelling of the same value. restJson1 conveys the error class to the SDK in this header, so its value is part of the refusal's contract, not a per-run value. Hypothesis: the input at this step is refused by AWS at the deserialization/serialization layer rather than the constraint layer (e.g. a member whose JSON type does not match the modeled shape), which AWS answers with a framework error type (SerializationException-class) and/or a Coral type-suffixed header value, whereas the emulator collapses every 400 refusal of this operation onto its modeled ValidationException.
Emulator: status 400, header x-amzn-errortype: ValidationException (replay: '$headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got ValidationException)').
Why: SDKs dispatch on x-amzn-errortype to pick the exception class they raise, so a client that catches the framework/serialization error AWS raises for a malformed member never sees it here — it sees a modeled ValidationException instead, and error-class-based retry/branching diverges from AWS.
Fix: the error channel of the EncryptData request walk — the deserializer's refusals must keep their own error type (and the serializer must emit that type in x-amzn-errortype) instead of being mapped onto the operation's modeled ValidationException.
Traces: payment-cryptography-data-sdk-refused-inputs#task#3.

## Evidence
Replay of simparity-wire-aws-v1: 46 steps, 36 passed, 7 mismatched, 3 blocked, score 78.3%, emulator_up true (bootstrap_signal false). 5 divergence clusters, all service payment-cryptography-data, all with empty stub_path and empty owned_by.

Cause 1 (2 clusters, verdict mismatch, 400 -> 400):
  fixture payment-cryptography-data-card-validation-data-cvv step task#4, VerifyCardValidationData
    expected $.Message = 'Card validation data verification failed.'
    actual   $.Message = 'This request failed verification.'
  fixture payment-cryptography-data-encrypt-decrypt-and-cmac-roundtrip step task#7, VerifyMac (cluster also covers payment-cryptography-data-generate-verify-mac-hmac#task#5)
    expected $.Message = 'Mac verification failed.'
    actual   $.Message = 'This request failed verification.'
  Smithy: VerificationFailedException = client error, httpError 400, required Reason + Message; its @documentation is exactly 'This request failed verification.' — i.e. the emulator is serving the shape's doc string as the Message.

Cause 2 (verdict mismatch, 400 -> 400): fixture payment-cryptography-data-generate-verify-mac-hmac step task#6, GenerateMac
  expected $.message = 'Invalid key with KeyAlgorithm HMAC used with MacAlgorithm TDES_2KEY. Please use valid HMAC Key.'
  actual   $.message = 'The KeyModesOfUse of key arn:aws:payment-cryptography:us-west-2:000000000000:key/chn5lv2yabh30nuu do not include Generate.'
  The ARN in the actual message is a per-run value; AWS's sentence contains no ARN. Fixed/variable split of AWS's sentence: fixed 'Invalid key with KeyAlgorithm ' / ' used with MacAlgorithm ' / '. Please use valid ' / ' Key.'; variables = requested MAC algorithm (request) and key KeyAlgorithm (state).
  Smithy: GenerateMac errors include ValidationException (client, httpError 400, defaulted).

Cause 3 (verdict mismatch, 400 -> 400): fixture payment-cryptography-data-sdk-refused-inputs step task#2, EncryptData
  expected $.message = "1 validation error detected. Value at '/EncryptionAttributes' failed to satisfy constraint: Member must not be null"
  actual   $.message = "1 validation error detected: Value null at 'encryptionAttributes' failed to satisfy constraint: Member must not be null"
  expected $.fieldList = [{'path': '/EncryptionAttributes', 'message': "Value at '/EncryptionAttributes' failed to satisfy constraint: Member must not be null"}]
  actual   $.fieldList = missing (None)
  Smithy: EncryptData input member EncryptionAttributes -> EncryptionDecryptionAttributes, required: true; ValidationException members = message (required) + fieldList (ValidationExceptionFieldList of ValidationExceptionField {path, message}). Source: src/ops/encrypt_data.rs:269-275 (`field_list: None`).

Cause 4 (verdict mismatch, 400 -> 400): fixture payment-cryptography-data-sdk-refused-inputs step task#3, EncryptData
  expected $headers.x-amzn-errortype = <shaped-not-asserted> (shape assertion)
  actual   $headers.x-amzn-errortype = 'ValidationException' -> shape_mismatch
  Same fixture's task#2 (Cause 3) shows no header divergence, so the header value is not asserted away for this service generally: this step's recorded error type is a different class from the emulator's ValidationException.

Already filed, excluded here: the blocked steps behind the GeneratePinData / GenerateAuthRequestCryptogram scaffold stubs (not_implemented=1, steps_blocked=3).

## Steps to Reproduce
Boot the emulator and point a botocore payment-cryptography / payment-cryptography-data client at it (region us-west-2), then replay the named fixtures.

Cause 1: CreateKey (control plane) TR31_C0_CARD_VERIFICATION_KEY (TDES_2KEY, KeyModesOfUse {Generate, Verify}); GenerateCardValidationData(PrimaryAccountNumber, GenerationAttributes={'CardVerificationValue1': {...}}); corrupt one digit of the returned ValidationData and call VerifyCardValidationData -> 400 VerificationFailedException with Message 'This request failed verification.' instead of 'Card validation data verification failed.' (payment-cryptography-data-card-validation-data-cvv#task#4). Repeat with an HMAC/CMAC key: GenerateMac, then VerifyMac with a different MessageData -> Message 'This request failed verification.' instead of 'Mac verification failed.' (payment-cryptography-data-encrypt-decrypt-and-cmac-roundtrip#task#7, payment-cryptography-data-generate-verify-mac-hmac#task#5).

Cause 2: CreateKey a TDES_2KEY / TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY, then GenerateMac(KeyIdentifier=<that key arn>, MessageData=<hex>, GenerationAttributes={'Algorithm': 'HMAC'}) -> the KeyModesOfUse sentence instead of the 'Invalid key with KeyAlgorithm HMAC used with MacAlgorithm TDES_2KEY. Please use valid HMAC Key.' sentence (payment-cryptography-data-generate-verify-mac-hmac#task#6).

Cause 3: POST /keys/{KeyIdentifier}/encrypt with a body carrying PlainText but NO EncryptionAttributes (bypass the SDK's client-side check by sending the raw request) -> compare $.message wording and the absent $.fieldList against the recorded response (payment-cryptography-data-sdk-refused-inputs#task#2).

Cause 4: send the same EncryptData URI the fixture's task#3 sends, with its deliberately malformed member, and inspect the response's x-amzn-errortype header: the emulator announces 'ValidationException' where the recording carries a different error type (payment-cryptography-data-sdk-refused-inputs#task#3).
