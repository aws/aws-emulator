Issue-Source: trace-sweep-divergence
Issue-Trace: socialmessaging-business-public-key-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: socialmessaging-business-public-key-not-found
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: social-messaging|PutWhatsAppBusinessPublicKey|4xx->4xx|$.message
Issue-Key: social-messaging|PutWhatsAppBusinessPublicKey|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: social-messaging|PutWhatsAppBusinessPublicKey|4xx->4xx|status
Issue-Commit: d58f683b0a231787aea4e0c79c54b755bdc7a862

# social-messaging: PutWhatsAppBusinessPublicKey skips businessPublicKey PEM validation and answers 404 ResourceNotFoundException instead of 400 InvalidParametersException

> _Note: this issue was identified against the code base at commit `d58f683b0a231787aea4e0c79c54b755bdc7a862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutWhatsAppBusinessPublicKey

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: PutWhatsAppBusinessPublicKey never inspects the `businessPublicKey` it is handed, so a request carrying a value that is not a PEM-encoded X.509 RSA public key is never refused on the key at all. The handler decides only that exactly one of `businessPublicKey`/`kmsKeyArn` is present, and then goes straight to resolving `originationPhoneNumberId` in the store — so the first thing that can fail for such a request is the resource lookup, and the caller is told the phone number does not exist rather than that the key is malformed. Fixture: socialmessaging-business-public-key-not-found, step task#2.

AWS: the operation models both `InvalidParametersException` (HTTP 400, "One or more parameters provided to the action are not valid.") and `ResourceNotFoundException` (HTTP 404, "The resource was not found."). Smithy: PutWhatsAppBusinessPublicKey.businessPublicKey -> BusinessPublicKeyPem @length 1..8192 (no @pattern), documented as "The PEM-encoded 2048-bit RSA public key to set. Mutually exclusive with kmsKeyArn."; PutWhatsAppBusinessPublicKey.originationPhoneNumberId -> WhatsAppPhoneNumberId @length 1..115 @pattern (^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$), required. The model states the key must be a PEM-encoded RSA public key but encodes no pattern for it, so the refusal itself is recorded behavior only: the recording shows AWS validating the key material from the REQUEST before it resolves the phone number, and answering HTTP 400 with error code `InvalidParametersException` and message "businessPublicKey is not a valid X.509 RSA public key.". Message rule for this error class: the whole sentence is FIXED text except the leading token, which is the offending INPUT MEMBER NAME as the wire spells it (`businessPublicKey`) — i.e. "<memberName> is not a valid X.509 RSA public key." taken from the request, with no id, phone number, key bytes or timestamp interpolated. The recording's `x-amzn-errortype` header carries the Coral-qualified form `InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/`; the shape name before the colon is the requirement and the namespace suffix after it is this service's constant Coral namespace, so correcting only the error class still leaves this header diverging if it is serialized as the bare shape name.

Emulator: answers HTTP 404 with `x-amzn-errortype: ResourceNotFoundException` and message "Origination phone number id not found for account." — the store's generic origination-number-not-found wording — because the malformed key is accepted silently and the unknown-number branch is the first refusal reached. Replay reports: status expected 400, got 404; $.message value_mismatch (expected 'businessPublicKey is not a valid X.509 RSA public key.', got 'Origination phone number id not found for account.'); $headers.x-amzn-errortype value_mismatch (expected 'InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/', got 'ResourceNotFoundException').

Why: a client that sent a bad key is told the wrong thing twice over. On the status class it sees 404, so SDK error handling and retry/branching logic that distinguishes "my input is wrong" (4xx InvalidParametersException, never retry, fix the payload) from "that resource is gone" (ResourceNotFoundException, re-register or re-discover the phone number) takes the resource branch and goes looking for a number that is in fact fine. Worse, the diagnosis is inverted: a caller debugging a key-generation or PEM-encoding bug is pointed at its phone-number registration instead, and any code keying off `x-amzn-errortype` string-matches a class AWS would not have returned. Conversely, a provisioning flow that treats ResourceNotFoundException as "number not registered yet" will loop or re-associate on a request that can never succeed as written.

Fix: PutWhatsAppBusinessPublicKey's validation, in its handler — services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/put_whats_app_business_public_key.rs. The request-derived checks must ALL run before the store is consulted: the existing mutual-exclusivity check is correctly placed there, and the key-material check belongs beside it, refusing a `businessPublicKey` that is not a PEM-encoded X.509 RSA public key with InvalidParametersException (HTTP 400) and the member-named message above, before `store.phone_number_id(...)` is reached. The error serializer for this service must also render `x-amzn-errortype` in the Coral-qualified form the recording shows rather than the bare shape name.

## Evidence
Fixture socialmessaging-business-public-key-not-found, step task#2, verdict mismatch.

Expected (recorded AWS response):
  HTTP 400
  x-amzn-errortype: InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/
  $.message: "businessPublicKey is not a valid X.509 RSA public key."

Actual (emulator):
  HTTP 404
  x-amzn-errortype: ResourceNotFoundException
  $.message: "Origination phone number id not found for account."

Divergence key: service=social-messaging|PutWhatsAppBusinessPublicKey|status=400->404|keys=$.message,$headers.x-amzn-errortype,status

Model evidence (smithy_query, services/service-com-amazonaws-socialmessaging-socialmessaging/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json): PutWhatsAppBusinessPublicKey errors include InvalidParametersException (httpError 400, client) and ResourceNotFoundException (httpError 404, client); businessPublicKey targets BusinessPublicKeyPem (string, @length 1..8192, no @pattern).

Code evidence: src/ops/put_whats_app_business_public_key.rs lines 170-198 — the handler checks only `input.business_public_key.is_some() != input.kms_key_arn.is_some()`, then falls through to `store.phone_number_id(&input.origination_phone_number_id)` and returns ResourceNotFoundException with crate::state::ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE. No inspection of the PEM contents exists anywhere in the handler.

Related (already RESOLVED, so not covering this): 20261029T171221Z-social-messaging-whatsapp-wire-responses-diverge-from-recording.md and 20261022T232716Z-socialmessaging-validationexception-messages-render-patterns-oversize-values-and.md touched this service's error wording but neither added businessPublicKey key-material validation.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the wire corpus: replay_wire_corpus().
3. Fixture socialmessaging-business-public-key-not-found, step task#2 sends PUT /v1/whatsapp/business-public-key with a required `originationPhoneNumberId` matching the modelled pattern and a `businessPublicKey` whose value is not a PEM-encoded X.509 RSA public key (and no `kmsKeyArn`).
4. Observe the response: the emulator returns HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Origination phone number id not found for account.".
5. Expected per the recording: HTTP 400, x-amzn-errortype: InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/, message "businessPublicKey is not a valid X.509 RSA public key." — i.e. the request-level key-material refusal must be decided before the phone-number lookup.
