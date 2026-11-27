Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-create-field-level-encryption-config
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-create-field-level-encryption-config|run#12|AssertionError|assert returned["QueryArgProfileConfig"]["ForwardWhenQueryArgProfileIsUnknown"] is False
Issue-Commit: eea346863107feece0c07c64ca5ae5407ab94bd8

# cloudfront: CreateFieldLevelEncryptionConfig does not echo the submitted QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown=false

> _Note: this issue was identified against the code base at commit `eea346863107feece0c07c64ca5ae5407ab94bd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionConfig

## Description
One net-new behavioral cause. Note: the open WIRE issue 20261109T120741Z-cloudfront-field-level-encryption-configs-lose-contenttypeprofileconfig-and.md describes the same family of defect from the wire corpus, but the replay report does NOT mark this behavioral check as OWNED by it or by any other open issue, so it is filed here with its own replay key; a fixer closing both should re-replay this check id as well.

Cause 1 of 1: the CreateFieldLevelEncryptionConfig response reports QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown with a value other than the one submitted.
AWS: CreateFieldLevelEncryptionConfig answers 201 whose FieldLevelEncryption.FieldLevelEncryptionConfig is the configuration AS SUBMITTED. Smithy: CreateFieldLevelEncryptionConfig input FieldLevelEncryptionConfig (required); FieldLevelEncryptionConfig members CallerReference (required), Comment, QueryArgProfileConfig, ContentTypeProfileConfig; QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a REQUIRED boolean — "Flag to set if you want a request to be forwarded to the origin even if the profile specified by the field-level encryption query argument, fle-profile, is unknown". The model gives the member no default and no server-side override, so a request that submits false must come back false, from the create echo and from the GetFieldLevelEncryptionConfig read of the same Id.
Emulator: for a create whose QueryArgProfileConfig carries ForwardWhenQueryArgProfileIsUnknown=false and one QueryArgProfiles item naming a real profile, the 2xx response's FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is NOT false (the member is present — a missing member would have raised KeyError — and compares unequal to False), so the check fails at `assert returned["QueryArgProfileConfig"]["ForwardWhenQueryArgProfileIsUnknown"] is False` with a bare AssertionError. The same corpus's wire replay recorded the value as true where false was submitted, i.e. the flag is reported inverted/defaulted rather than stored as submitted.
Why: this flag is the deny-or-forward decision for a request whose fle-profile query argument names an unknown profile. A client that submits false (do not forward content when the profile is unknown) and reads the create response — or the stored configuration — is told forward-anyway is in force. Worse, a read-modify-write through UpdateFieldLevelEncryptionConfig carries the wrong value back in, so the misreport becomes the configuration. Because the check aborts at this assertion, the later parts of the same contract are left unverified by this run: the independent GetFieldLevelEncryptionConfig read-back (ETag, Comment, flag, ProfileId) and the NoSuchFieldLevelEncryptionProfile refusal (404, message "The specified field level encryption profile does not exist.") for a query-arg profile naming a nonexistent ProfileId.
Fix: the CreateFieldLevelEncryptionConfig handler's configuration resolution and the FLE config state it stores/projects — the submitted FieldLevelEncryptionConfig must be persisted and projected verbatim for this member instead of being substituted with a service default (see Traces for the files involved).
Traces: failing check cloudfront.op-contract-create-field-level-encryption-config. Code sites: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_field_level_encryption_config.rs line 218 (`crate::state::resolve_field_level_encryption_config(config)`) and line 360 (`stored.to_field_level_encryption()`); services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs lines 3957-3986 (StoredFieldLevelEncryptionConfig and its to_field_level_encryption projection).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator up (563/702 overall).

### cloudfront
[mismatch] cloudfront.op-contract-create-field-level-encryption-config
  Error: File "run.py", line 456, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_cloudfront_op_contract_create_field_level_encryption_config.py", line 41, in run
    assert returned["QueryArgProfileConfig"]["ForwardWhenQueryArgProfileIsUnknown"] is False
  AssertionError

Expected (AWS): the create response's FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown == false, exactly the value the request submitted, with QueryArgProfiles.Items[0].ProfileId equal to the profile id created moments earlier.
Actual (emulator): the member is present but is not false (recorded elsewhere as true), so the submitted value is not what the service reports.
The check carried no marker in the replay report: no "ALREADY FILED" stub_path and no "OWNED by" line.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay simparity-behavioral-aws-v1.
2. Against the emulator's cloudfront endpoint: CreatePublicKey with a 2048-bit RSA SubjectPublicKeyInfo PEM, then CreateFieldLevelEncryptionProfile with one EncryptionEntities item naming that public key id; note the returned profile Id.
3. Call CreateFieldLevelEncryptionConfig with FieldLevelEncryptionConfig = {CallerReference: <unique>, Comment: <text>, QueryArgProfileConfig: {ForwardWhenQueryArgProfileIsUnknown: false, QueryArgProfiles: {Quantity: 1, Items: [{QueryArg: "q", ProfileId: <profile id>}]}}}.
4. Read FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown from the 2xx response: it is not false. Call GetFieldLevelEncryptionConfig with the returned Id and observe the stored configuration reports the same wrong value.
5. Re-replay check cloudfront.op-contract-create-field-level-encryption-config to confirm the fix; the check then goes on to assert the independent read-back (ETag/Comment/flag/ProfileId) and the NoSuchFieldLevelEncryptionProfile 404 with message "The specified field level encryption profile does not exist."
