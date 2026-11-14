Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.create-ksk-malformed-key-argument-rejected route53.delete-active-ksk-blocked
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.create-ksk-malformed-key-argument-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53.delete-active-ksk-blocked|run#8|AssertionError|assert code == "InvalidKeySigningKeyStatus", f"expected InvalidKeySigningKeyStatus, got {code!r}"
Issue-Commit: a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c

# Route 53 DNSSEC key-signing-key refusals diverge: CreateKeySigningKey's InvalidKMSArn Message is the model's documentation sentence, and DeleteKeySigningKey answers 501 NotImplemented instead of InvalidKeySigningKeyStatus / NoSuchKeySigningKey (2 causes)

> _Note: this issue was identified against the code base at commit `a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeySigningKey

## Description
Two net-new root causes on the Route 53 DNSSEC key-signing-key (KSK) surface. A fixer must close BOTH before submitting. Smithy model consulted: services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json.

=== Cause 1 of 2: CreateKeySigningKey's InvalidKMSArn refusal carries the error shape's documentation text as its Message ===
AWS: Smithy: CreateKeySigningKey POST /2013-04-01/keysigningkey (success 201) models error InvalidKMSArn (errorKind client, httpError 400, defaulted) alongside InvalidKeySigningKeyName, InvalidKeySigningKeyStatus, KeySigningKeyAlreadyExists (409) and NoSuchHostedZone (404). Route 53 DNSSEC requires an ECC_NIST_P256 SIGN_VERIFY customer-managed key; a well-formed ARN naming a key of another spec (e.g. RSA_2048) is refused with 400 InvalidKMSArn. AWS's Message for that refusal is built from the request's ARN, and the template is the requirement: fixed text "The customer managed key with ARN '" + <the KeyManagementServiceArn from the request> + "' could not be used by Route 53 DNSSEC. Check to make sure that the customer managed key has not been deleted. Otherwise, review its configurations and key policy, to make sure it is in compliance with Route 53 DNSSEC requirements." — only the ARN inside the quotes is a value taken from the request; everything else is fixed text. (The arn:aws:kms:us-east-1:<account>:key/<key-id> seen in one replay is that variable part filled in, never part of the template.)
Emulator: the code is right (InvalidKMSArn) but the Message is the Smithy shape's own documentation sentence: "The KeyManagementServiceArn that you specified isn't valid to use with DNSSEC signing." It names neither the offending ARN nor the remediation AWS words, i.e. the serializer is emitting the modeled error's doc string in place of a condition-specific message.
Why: a client surfacing or matching on the refusal text gets a sentence AWS never sends and loses the identity of the rejected key, so DNSSEC-setup tooling cannot tell the operator WHICH key was refused or why (deleted key vs. wrong key spec vs. key policy). Any assertion/log scraping built against AWS's wording fails.
Fix: the CreateKeySigningKey handler's KMS-ARN/key-spec validation and the message it attaches to the modeled InvalidKMSArn error (the operation's handler file under services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/). The emulator must stop defaulting an error's Message to the shape's documentation text. Context: 20261024T160546Z-route53-route-53-validation-semantics-gaps-five-operations-answer.md (open) pinned only that this call used to answer NotImplemented with the wrong CODE; the code is now correct and this cause is about the Message. 20261024T114159Z-route53-createkeysigningkey-stub.md is marked fixed and stood the handler up; the call is correctly REFUSED here, so this is not a reopen of it.
Traces: route53.create-ksk-malformed-key-argument-rejected.

=== Cause 2 of 2: DeleteKeySigningKey is an unimplemented scaffold stub, so an ACTIVE KSK's delete is answered 501 NotImplemented instead of 400 InvalidKeySigningKeyStatus ===
AWS: Smithy: DeleteKeySigningKey DELETE /2013-04-01/keysigningkey/{HostedZoneId}/{Name} (success 200, output ChangeInfo required) models InvalidKeySigningKeyStatus (400), NoSuchKeySigningKey (404), ConcurrentModification (400), InvalidInput (400), InvalidKMSArn (400), InvalidSigningStatus (400). The modeled documentation states the KSK must be deactivated before it can be deleted, regardless of whether the zone is signing. So deleting a KSK whose current status is ACTIVE is refused with 400 InvalidKeySigningKeyStatus and nothing is deleted (GetDNSSEC still lists the KSK with Status ACTIVE); the message template is fixed text with the requested name substituted: "Key Signing Key with name " + <Name from the request> + " cannot be deleted because current status is not INACTIVE. You can use DeactivateKeySigningKey to deactivate the Key Signing Key before you delete it." After the KSK is deactivated and deleted, the same DELETE is refused 404 NoSuchKeySigningKey with fixed text "The Key Signing Key with name '" + <Name> + "', could not be found."
Emulator: every DeleteKeySigningKey call answers HTTP 501 with Error.Code NotImplemented, message "operation DeleteKeySigningKey is scaffolded but not implemented in this emulator" — the handler is still the generated scaffold (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_key_signing_key.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented("DeleteKeySigningKey")`).
Why: a client cannot learn that signing material is protected until the KSK is deactivated — an unclassifiable 501 is indistinguishable from an outage, and a DNSSEC teardown flow can neither verify the ACTIVE-key guard (nothing deleted, status still ACTIVE) nor the terminal 404 after a legitimate delete. No DNSSEC lifecycle can be completed against the emulator.
Fix: implement the DeleteKeySigningKey handler and its status validation in services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_key_signing_key.rs (return the modeled InvalidKeySigningKeyStatus / NoSuchKeySigningKey with the messages above, a ChangeInfo on success) against the DNSSEC/KSK state GetDNSSEC reads; remove the SCAFFOLDED marker. No open issue names this operation. Note the same check's positive control then calls DeactivateKeySigningKey, which is itself still a stub and is covered by its own already-filed stub issue — this cause is only about DeleteKeySigningKey.
Traces: route53.delete-active-ksk-blocked.

## Evidence
Suite simparity-behavioral-aws-v1, 97/130 passed. Both verdicts are 'mismatch' under heading `### route53`.

Cause 1 — route53.create-ksk-malformed-key-argument-rejected (check line 50):
  Expected (AWS): Error.Code InvalidKMSArn, HTTP 400, Message "The customer managed key with ARN '<requested KMS ARN>' could not be used by Route 53 DNSSEC. Check to make sure that the customer managed key has not been deleted. Otherwise, review its configurations and key policy, to make sure it is in compliance with Route 53 DNSSEC requirements."
  Actual (emulator): Error.Code InvalidKMSArn (correct) but Message "The KeyManagementServiceArn that you specified isn't valid to use with DNSSEC signing." — verbatim the InvalidKMSArn shape's Smithy documentation sentence.
  AssertionError: expected message "The customer managed key with ARN '...' could not be used by Route 53 DNSSEC. ...", got "The KeyManagementServiceArn that you specified isn't valid to use with DNSSEC signing."

Cause 2 — route53.delete-active-ksk-blocked (check line 33):
  Expected (AWS): Error.Code InvalidKeySigningKeyStatus, HTTP 400, Message "Key Signing Key with name <Name> cannot be deleted because current status is not INACTIVE. You can use DeactivateKeySigningKey to deactivate the Key Signing Key before you delete it."; the KSK survives with Status ACTIVE.
  Actual (emulator): AssertionError: expected InvalidKeySigningKeyStatus, got 'NotImplemented' (HTTP 501, "operation DeleteKeySigningKey is scaffolded but not implemented in this emulator").
  Source confirmation: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_key_signing_key.rs lines 3-6 (`SCAFFOLDED, NOT IMPLEMENTED` marker) and line 171 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteKeySigningKey"))`).

Model confirmation (smithy_query query=operation on com-amazonaws-route53-awsdnsv20130401.json): CreateKeySigningKey errors include InvalidKMSArn (400, defaulted); DeleteKeySigningKey (DELETE /2013-04-01/keysigningkey/{HostedZoneId}/{Name}, 200) errors include InvalidKeySigningKeyStatus (400) and NoSuchKeySigningKey (404).

## Steps to Reproduce
Boot the emulator and replay the two checks in trace_ids.

Cause 1: CreateHostedZone (public). Create a KMS SIGN_VERIFY key in us-east-1 with KeySpec=RSA_2048 and the dnssec-route53.amazonaws.com key policy. Call CreateKeySigningKey(HostedZoneId=<zone>, KeyManagementServiceArn=<that RSA ARN>, Name=<ksk name>, Status='INACTIVE', CallerReference=<uuid>). Expect HTTP 400 InvalidKMSArn whose Message quotes the submitted ARN and carries AWS's full remediation sentence; observe instead the one-line InvalidKMSArn documentation sentence. Then confirm GetDNSSEC lists no KSK under that name.

Cause 2: CreateHostedZone (public); create an ECC_NIST_P256 DNSSEC KMS key; CreateKeySigningKey(..., Status='ACTIVE'); then DeleteKeySigningKey(HostedZoneId=<zone>, Name=<same name>). Expect HTTP 400 InvalidKeySigningKeyStatus with the "cannot be deleted because current status is not INACTIVE" message and GetDNSSEC still showing that KSK ACTIVE; observe HTTP 501 NotImplemented. After implementing, also check the terminal case: deactivate, delete (200 with ChangeInfo), then repeat the delete and expect 404 NoSuchKeySigningKey with Message "The Key Signing Key with name '<Name>', could not be found."
