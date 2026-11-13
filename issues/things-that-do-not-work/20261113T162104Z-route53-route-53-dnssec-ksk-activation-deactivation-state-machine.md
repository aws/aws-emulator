Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.key-signing-key-in-use-deactivation-rejected route53.ksk-concurrent-activation-race
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.key-signing-key-in-use-deactivation-rejected|run#13|AssertionError|assert err["Code"] == "KeySigningKeyInUse"
Issue-Key: route53.ksk-concurrent-activation-race|run#13|AssertionError|assert code in allowed_failure_codes, f"unexpected racer failure code: {code}"
Issue-Commit: 9c322f20db98c6414e021e385ca8f227769f47e0

# Route 53 DNSSEC KSK activation/deactivation state machine is unreachable: both ActivateKeySigningKey and DeactivateKeySigningKey answer 501 NotImplemented, so the KeySigningKeyInUse guard and idempotent concurrent activation are never exercised (2 causes)

> _Note: this issue was identified against the code base at commit `9c322f20db98c6414e021e385ca8f227769f47e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeactivateKeySigningKey

## Description
Two behavioral checks drive the Route 53 DNSSEC key-signing-key (KSK) status state machine end to end (create zone, create KMS key, create KSK, EnableHostedZoneDNSSEC, then mutate KSK status). Both are refused by the emulator on the KSK status-mutation call itself, so no part of the state machine's AWS-defined semantics is observable. The two scaffold-stub issues `20261113T100545Z-route53-activatekeysigningkey-stub.md` and `20261113T100545Z-route53-deactivatekeysigningkey-stub.md` name the two handler files but do not carry these checks' replay keys, and the behavioral report did not mark either check as covered — this issue records the behavior those checks require of the two handlers once written, and the replay list that confirms it.

Cause 1 of 2: DeactivateKeySigningKey does not enforce the KeySigningKeyInUse guard.
AWS: Smithy `DeactivateKeySigningKey` (POST /2013-04-01/keysigningkey/{HostedZoneId}/{Name}/deactivate, success 200 with required `ChangeInfo`) models `KeySigningKeyInUse` (HTTP 400), documented as "The key-signing key (KSK) that you specified can't be deactivated because it's the only KSK for a currently-enabled DNSSEC. Disable DNSSEC signing, or add or enable another KSK." When a hosted zone's ServeSignature is SIGNING and the named KSK is the only ACTIVE key, the request is refused with HTTP 400, error code `KeySigningKeyInUse`, and (recorded behavior) the Message `Key Signing Key with name '<Name>' is the only available ACTIVE key for the hosted zone.` — the fixed text is everything outside the quotes; the only variable part is `<Name>`, taken verbatim from the request's `Name` label. The refusal leaves state untouched: GetDNSSEC still reports the KSK ACTIVE and the zone SIGNING, and the identical deactivation succeeds (200, `ChangeInfo`) once DisableHostedZoneDNSSEC has been called.
Emulator: the request never reaches any such guard. `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/deactivate_key_signing_key.rs` is an unimplemented scaffold whose `handle` returns `Scaffolded::not_implemented("DeactivateKeySigningKey")`, i.e. HTTP 501 `NotImplemented`. The check's assertion on the returned error code fails because the code is not `KeySigningKeyInUse`; nothing about status 400, the code, or the message matches.
Why: a client that relies on AWS refusing the removal of a zone's last ACTIVE KSK gets a 501 instead of a 400 — an unmodeled status that botocore surfaces as a non-retryable unknown failure rather than the documented `KeySigningKeyInUse`. Rotation tooling cannot distinguish "you must add another KSK first" from "the service is broken", and cannot deactivate a KSK at all, so DNSSEC key rotation and zone unsigning are impossible.
Fix: the DeactivateKeySigningKey handler — it must implement the KSK status transition against the hosted zone's DNSSEC state and the ACTIVE-key count, and raise the modeled `KeySigningKeyInUse` (and `InvalidSigningStatus` / `NoSuchKeySigningKey`) rather than the scaffold's 501.
Traces: route53.key-signing-key-in-use-deactivation-rejected.

Cause 2 of 2: ActivateKeySigningKey answers a fabricated `NotImplemented` error code where AWS accepts the activation idempotently.
AWS: Smithy `ActivateKeySigningKey` (POST /2013-04-01/keysigningkey/{HostedZoneId}/{Name}/activate, success 200 with required `ChangeInfo`) models exactly these client errors: `ConcurrentModification`, `InvalidInput`, `InvalidKeySigningKeyStatus`, `InvalidKMSArn`, `InvalidSigningStatus`, `NoSuchKeySigningKey` — all 400 except `NoSuchKeySigningKey` (404). Recorded behavior: several concurrent activations of one INACTIVE KSK are each accepted (200 + `ChangeInfo`); AWS surfaces no contention error for them, and any error it does surface is one of the modeled contention codes `ConcurrentModification` / `InvalidKeySigningKeyStatus`. The KSK converges to ACTIVE with exactly one entry for that Name in GetDNSSEC.
Emulator: every racer is refused with error code `NotImplemented` (assertion: `unexpected racer failure code: NotImplemented`), from the unimplemented scaffold `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/activate_key_signing_key.rs` answering HTTP 501. `NotImplemented` is not a code this operation models, so no success and no documented contention code is ever produced and the KSK never leaves INACTIVE.
Why: a client cannot activate a KSK, and the error it receives is outside the operation's modeled error set, so retry/idempotency logic keyed on `ConcurrentModification` never fires and the caller sees an unclassifiable failure instead of a 200.
Fix: the ActivateKeySigningKey handler — implement the INACTIVE→ACTIVE transition with idempotent re-activation, returning 200 with `ChangeInfo`, and reserve modeled failures for the modeled conditions.
Traces: route53.ksk-concurrent-activation-race.

## Evidence
route53.key-signing-key-in-use-deactivation-rejected — after EnableHostedZoneDNSSEC and GetDNSSEC reporting ServeSignature=SIGNING, DeactivateKeySigningKey(HostedZoneId=<zone>, Name=<ksk>) is expected to fail with Error.Code == "KeySigningKeyInUse" and Error.Message == "Key Signing Key with name '<ksk>' is the only available ACTIVE key for the hosted zone." (HTTP 400). Replay fails at `assert err["Code"] == "KeySigningKeyInUse"` (check line 51): the returned code is not KeySigningKeyInUse. The handler file services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/deactivate_key_signing_key.rs carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeactivateKeySigningKey"))` (line 172), i.e. HTTP 501 NotImplemented.

route53.ksk-concurrent-activation-race — five concurrent ActivateKeySigningKey(HostedZoneId=<zone>, Name=<ksk>) calls on one INACTIVE KSK. Expected: >=1 success (HTTP 200 with ChangeInfo), every failure's code in {ConcurrentModification, InvalidKeySigningKeyStatus}, and GetDNSSEC converging to Status=ACTIVE with exactly one entry for that Name. Actual: `AssertionError: unexpected racer failure code: NotImplemented` (check line 54) — all racers refused with code NotImplemented from the ActivateKeySigningKey scaffold stub (HTTP 501), so there is no success and the KSK stays INACTIVE.

Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): DeactivateKeySigningKey errors = ConcurrentModification/InvalidInput/InvalidKeySigningKeyStatus/InvalidSigningStatus/KeySigningKeyInParentDSRecord/KeySigningKeyInUse (400) + NoSuchKeySigningKey (404); ActivateKeySigningKey errors = ConcurrentModification/InvalidInput/InvalidKeySigningKeyStatus/InvalidKMSArn/InvalidSigningStatus (400) + NoSuchKeySigningKey (404). Neither models `NotImplemented`, and both declare 200 + required ChangeInfo on success.

## Steps to Reproduce
Cause 1 (route53.key-signing-key-in-use-deactivation-rejected):
1. CreateHostedZone for a public zone; create a KMS asymmetric ECC_NIST_P256 SIGN_VERIFY key with a policy granting dnssec-route53.amazonaws.com Sign/GetPublicKey/DescribeKey.
2. route53 CreateKeySigningKey(HostedZoneId=<zone>, KeyManagementServiceArn=<key>, Name=<ksk>, Status="ACTIVE"); wait for the ChangeInfo to go INSYNC.
3. route53 EnableHostedZoneDNSSEC(HostedZoneId=<zone>); poll GetDNSSEC until Status.ServeSignature == "SIGNING".
4. route53 DeactivateKeySigningKey(HostedZoneId=<zone>, Name=<ksk>).
   Expected: HTTP 400, Error.Code "KeySigningKeyInUse", Error.Message "Key Signing Key with name '<ksk>' is the only available ACTIVE key for the hosted zone."; GetDNSSEC still shows the KSK ACTIVE and the zone SIGNING; the same call succeeds (200 + ChangeInfo) after DisableHostedZoneDNSSEC.
   Actual: HTTP 501 NotImplemented from the scaffold stub — the code is not KeySigningKeyInUse.

Cause 2 (route53.ksk-concurrent-activation-race):
1. Create a public hosted zone and a DNSSEC-capable KMS key as above.
2. route53 CreateKeySigningKey(..., Status="INACTIVE"); poll GetDNSSEC until that KSK reads INACTIVE.
3. Issue five concurrent route53 ActivateKeySigningKey(HostedZoneId=<zone>, Name=<ksk>) calls from a 5-worker pool.
   Expected: at least one 200 with ChangeInfo; any failure carries code ConcurrentModification or InvalidKeySigningKeyStatus; GetDNSSEC converges to Status ACTIVE with exactly one entry for <ksk>.
   Actual: every call fails with Error.Code "NotImplemented" (HTTP 501) and the KSK remains INACTIVE.
