Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors cloudfront-field-level-encryption-lifecycle cloudfront-query-arg-profile-config-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary[Id="CVBB9JKPE6KWE0"].ContentTypeProfileConfig
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary[Id="CVBB9JKPE6KWE0"].QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Related: 20261109T120741Z-cloudfront-create-responses-announce-a-location-uri-on.md
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CloudFront field-level encryption configs lose ContentTypeProfileConfig and invert ForwardWhenQueryArgProfileIsUnknown

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionConfig

## Description
Four causes, one shared fault: the field-level-encryption config the caller submits is not stored as submitted - the whole `ContentTypeProfileConfig` block is discarded and `QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown` is reported as true regardless of the submitted value - and every representation of the config (create, get, list) repeats the loss.

Cause 1 of 4: CreateFieldLevelEncryptionConfig's response drops the submitted config members.
AWS: 201 whose `FieldLevelEncryption.FieldLevelEncryptionConfig` echoes what was submitted: `QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown` exactly as sent, and `ContentTypeProfileConfig` with its required `ForwardWhenContentTypeIsUnknown` and its `ContentTypeProfiles` items (Smithy: FieldLevelEncryptionConfig members QueryArgProfileConfig and ContentTypeProfileConfig; QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a REQUIRED boolean; ContentTypeProfileConfig.ForwardWhenContentTypeIsUnknown is a REQUIRED boolean, 'specifies what to do when an unknown content type is provided for the profile').
Emulator: returns `ForwardWhenQueryArgProfileIsUnknown` true where false was submitted, and no `ContentTypeProfileConfig` element at all.
Why: the two members decide whether CloudFront forwards content when a profile or content type is unknown - inverting one turns a deny-by-default FLE configuration into forward-anyway, and dropping the content-type block removes the field encryption mapping entirely. A client that creates an FLE config and reads the response believes a configuration is in force that the emulator does not hold.
Fix: CreateFieldLevelEncryptionConfig's handler/state - persist the submitted FieldLevelEncryptionConfig verbatim instead of substituting defaults.
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4.

Cause 2 of 4: GetFieldLevelEncryptionConfig reads back the same loss.
AWS: 200 with `FieldLevelEncryptionConfig` as submitted (both members as in cause 1).
Emulator: `ForwardWhenQueryArgProfileIsUnknown` true, `ContentTypeProfileConfig` absent.
Why: the loss is in stored state, so even a client that never saw the create response reads the wrong configuration; a read-modify-write through UpdateFieldLevelEncryptionConfig then erases the content-type profiles permanently.
Fix: FLE config state/readback serializer.
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6 (3 affected steps, incl. cloudfront-field-level-encryption-lifecycle#task#8, cloudfront-query-arg-profile-config-lifecycle#task#5).

Cause 3 of 4: CreateFieldLevelEncryptionConfig in the lifecycle fixture - same loss, plus the Location header is rooted at the production host.
AWS: as cause 1; Location is `<service endpoint>/2020-05-31/field-level-encryption/<config id>`.
Emulator: same dropped/inverted members, and Location on `cloudfront.amazonaws.com` (the Location fault itself is bundled in the Location-header issue filed alongside this one).
Why/Fix: as cause 1.
Traces: cloudfront-field-level-encryption-lifecycle#task#7 (2 affected steps).

Cause 4 of 4: ListFieldLevelEncryptionConfigs summaries repeat the loss.
AWS: 200 where each `FieldLevelEncryptionSummary` carries the config's `QueryArgProfileConfig` as submitted and its `ContentTypeProfileConfig`.
Emulator: summary has `ForwardWhenQueryArgProfileIsUnknown` true and no `ContentTypeProfileConfig`.
Why: inventory listings misreport every FLE configuration, so a client that audits FLE settings across configs sees forward-anyway everywhere.
Fix: ListFieldLevelEncryptionConfigs' summary projection (shared FLE state).
Traces: cloudfront-field-level-encryption-lifecycle#task#9 (2 affected steps).

The profile ids and content types quoted below come from the recording; the requirement is that whatever the request submits for these two members be stored and echoed unchanged.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). Four unowned mismatch clusters, all status-matching (201->201 / 200->200):
1. CreateFieldLevelEncryptionConfig - cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4: $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'; $...ContentTypeProfileConfig missing (expected {'ForwardWhenContentTypeIsUnknown': 'false', 'ContentTypeProfiles': {'Quantity': '1', 'Items': {'ContentTypeProfile': {'Format': 'URLEncoded', 'ProfileId': '', 'ContentType': 'application/x-www-form-urlencoded'}}}}, got None).
2. GetFieldLevelEncryptionConfig - cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6: identical two failures on $.FieldLevelEncryptionConfig (3 affected steps).
3. CreateFieldLevelEncryptionConfig - cloudfront-field-level-encryption-lifecycle#task#7: identical two failures plus $headers.location host_not_own ('https://cloudfront.amazonaws.com/2020-05-31/field-level-encryption/CVBB9JKPE6KWE0') (2 affected steps).
4. ListFieldLevelEncryptionConfigs - cloudfront-field-level-encryption-lifecycle#task#9: identical two failures on the FieldLevelEncryptionSummary (2 affected steps).
No stub_path and no owned_by on any of the four.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay the wire corpus (simparity-wire-aws-v1).
2. Representative step: fixture cloudfront-field-level-encryption-duplicate-and-ifmatch-errors, step task#4 - CreateFieldLevelEncryptionConfig submitting ForwardWhenQueryArgProfileIsUnknown=false and a ContentTypeProfileConfig; the response reports true and omits ContentTypeProfileConfig.
3. Same fixture, task#6 - GetFieldLevelEncryptionConfig shows the stored config has the same two defects.
4. cloudfront-field-level-encryption-lifecycle#task#7 and #task#9 - the create response and the ListFieldLevelEncryptionConfigs summary repeat them.
