Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-acl-grantee-type-vocabulary
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bucket-acl-grant-permission-outside-enum-refused-keeps-stored-grants s3-bucket-acl-grantee-type-vocabulary metadata-inventory-table-sse-algorithm-outside-enum-refused s3-metadata-annotation-sse-algorithm-outside-enum s3-metadata-configuration-inventory-state-outside-enum s3-metadata-create-annotation-state-outside-enum-refused s3-metadata-journal-encryption-algorithm-outside-enum-refused s3-metadata-journal-expiration-state-closed-enum object-tag-set-key-and-value-length-bounds s3-object-tagging-key-length-bounds s3-analytics-and-filter-tag-key-floor s3-analytics-filter-tag-key-must-hold-at-least-one-character s3-bucket-tagging-key-length-floor s3-delete-objects-empty-key-refused s3-itc-filter-tag-key-length-floor s3-metadata-annotation-table-sse-algorithm-enum-refused s3-metadata-inventory-table-sse-algorithm-outside-its-enum-rejected s3-metadata-inventory-table-state-enum-refused s3-metadata-journal-expiration-outside-enum
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketAcl|4xx->4xx|$.Error.Code
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Related: 20261128T022809Z-s3-xml-payload-validation-refusals-answer-malformedxml-instead.md
Issue-Commit: 69de8c2ffec997e32503e26344b0f68b39cbe6a4

# s3 semantic XML refusals collapse onto MalformedXML across ten operations

> _Note: this issue was identified against the code base at commit `69de8c2ffec997e32503e26344b0f68b39cbe6a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAcl

## Description
SUPERSEDES 20261128T022809Z-s3-xml-payload-validation-refusals-answer-malformedxml-instead.md, which carries the identical 10 causes but whose representative pointer could not be placed against the bound corpus (fixture 'bucket-acl-grant-permission-outside-enum-refused' did not resolve), leaving it unverifiable by replay. This filing repoints the same bundle at the sibling step of the same divergence cluster. Fix the causes once and close both.

10 net-new causes sharing ONE fault: when a value inside an XML request payload fails a *semantic* rule (a closed enum, a tag-key length floor, a required key), the emulator reports the generic schema-level refusal 'MalformedXML' / 'The XML you provided was not well-formed or did not validate against our published schema' (or, for ACLs, MalformedXML in place of MalformedACLError). AWS reserves MalformedXML for documents that are not well-formed or do not match the schema, and uses a distinct error Code per semantic rule. The HTTP status (400) is already correct in every cause; only Code, Message and the error class's context members diverge. The Smithy model declares no error shapes on these operations (errors: [] on PutBucketAcl, PutObjectTagging, PutBucketTagging, DeleteObjects, PutBucketAnalyticsConfiguration, PutBucketIntelligentTieringConfiguration, CreateBucketMetadataConfiguration and the UpdateBucketMetadata*TableConfiguration family), so each requirement below is recorded behavior only; the messages quoted are the fixed text of that error class and any names/values in them come from the request.

Cause 1 of 10 — PutBucketAcl, a Grantee type or Grant Permission outside the vocabulary. AWS: Smithy com.amazonaws.s3#Permission is a closed enum [FULL_CONTROL, WRITE, WRITE_ACP, READ, READ_ACP] and com.amazonaws.s3#Grantee requires the xsi:type attribute; a grant whose Grantee type or Permission is outside its vocabulary is refused 400 with Code 'MalformedACLError' (recorded behavior only for the Code/Message pairing). Emulator: 400 with Code 'MalformedXML'. Why: a client that distinguishes 'your ACL document is unparseable' from 'this grant is not a legal grant' takes the wrong branch and may retry a request that can never succeed. Fix: the PutBucketAcl ACL-document validator — raise the ACL-specific error class rather than falling through to the XML-schema error. Traces: s3-bucket-acl-grantee-type-vocabulary#task#3, bucket-acl-grant-permission-outside-enum-refused#task#5 (same cluster).

Cause 2 of 10 — CreateBucketMetadataConfiguration, an SSE algorithm / inventory-table state / journal-expiration value outside its enum. AWS: refused 400 with Code 'InvalidRequest' and Message 'Request invalid' (fixed text, no variable part). Recorded behavior only. Emulator: 400 'MalformedXML' with the schema sentence. Why: InvalidRequest is the code the S3 metadata-configuration APIs use for all semantic refusals; a caller branching on it never sees it. Fix: the CreateBucketMetadataConfiguration input validation, which must classify enum violations in the MetadataConfiguration payload as InvalidRequest. Traces: metadata-inventory-table-sse-algorithm-outside-enum-refused#task#2 (and #task#3), s3-metadata-annotation-sse-algorithm-outside-enum#task#2, s3-metadata-configuration-inventory-state-outside-enum#task#2, s3-metadata-create-annotation-state-outside-enum-refused#task#2, s3-metadata-journal-encryption-algorithm-outside-enum-refused#task#2, s3-metadata-journal-expiration-state-closed-enum#task#2.

Cause 3 of 10 — PutObjectTagging, an empty Tag Key. AWS: Smithy com.amazonaws.s3#Tag.Key targets ObjectKey with @length min 1 and is @required; an empty key is refused 400 with Code 'InvalidTag', Message 'The TagKey you have provided is invalid' (fixed text) and a <TagKey> member carrying the offending key as sent (here the empty string). Emulator: 400 'MalformedXML' with the schema sentence and no TagKey member. Why: tagging clients repair the request from InvalidTag + TagKey; with MalformedXML they cannot tell which tag failed. Fix: the object-tagging payload validator and its error serializer. Traces: object-tag-set-key-and-value-length-bounds#task#7, s3-object-tagging-key-length-bounds#task#3.

Cause 4 of 10 — PutBucketAnalyticsConfiguration, an And-filter Tag Key below the length floor. AWS: refused 400 with Code 'InvalidTag' and Message 'Tag Key cannot be empty or longer than 128 chars.' — fixed text whose 128 is the service limit, not a request value. Recorded behavior only for the pairing. Emulator: 400 'MalformedXML' with the schema sentence. Why: same as Cause 3, for a bucket-analytics filter. Fix: the analytics-configuration filter validator. Traces: s3-analytics-and-filter-tag-key-floor#task#2, s3-analytics-filter-tag-key-must-hold-at-least-one-character#task#2.

Cause 5 of 10 — PutBucketTagging, an empty Tag Key. AWS: 400 'InvalidTag', Message 'The TagKey you have provided is invalid', plus <TagKey> = the key as sent. Emulator: 400 'MalformedXML', no TagKey. Why/Fix: as Cause 3, in the bucket-tagging validator. Traces: s3-bucket-tagging-key-length-floor#task#2.

Cause 6 of 10 — DeleteObjects, an <Object> entry with an empty <Key>. AWS: Smithy ObjectKey carries @length min 1; the batch is refused 400 with Code 'UserKeyMustBeSpecified' and Message 'User key must be specified.' (fixed text). Recorded behavior only for the pairing. Emulator: 400 'MalformedXML' with the schema sentence. Why: a batch-delete client distinguishes 'one of my entries has no key' from 'my whole document is malformed' to decide whether to resend a repaired subset. Fix: the DeleteObjects Delete-payload validator. Traces: s3-delete-objects-empty-key-refused#task#3.

Cause 7 of 10 — PutBucketIntelligentTieringConfiguration, a filter Tag Key below the length floor. AWS: 400 'InvalidTag' with Message 'Tag Key cannot be null or empty.' (fixed text — note this configuration family words the sentence differently from analytics, Cause 4). Recorded behavior only. Emulator: 400 'MalformedXML' with the schema sentence. Why/Fix: as Cause 4, in the intelligent-tiering filter validator. Traces: s3-itc-filter-tag-key-length-floor#task#2 (and #task#3).

Cause 8 of 10 — UpdateBucketMetadataAnnotationTableConfiguration, an SSE algorithm outside its enum. AWS: 400 'InvalidRequest' / 'Request invalid'. Recorded behavior only. Emulator: 400 'MalformedXML' with the schema sentence. Why/Fix: as Cause 2, in this operation's validator. Traces: s3-metadata-annotation-table-sse-algorithm-enum-refused#task#2.

Cause 9 of 10 — UpdateBucketMetadataInventoryTableConfiguration, an SSE algorithm / table state outside its enum. AWS: 400 'InvalidRequest' / 'Request invalid'. Recorded behavior only. Emulator: 400 'MalformedXML' with the schema sentence. Why/Fix: as Cause 2. Traces: s3-metadata-inventory-table-sse-algorithm-outside-its-enum-rejected#task#2, s3-metadata-inventory-table-state-enum-refused#task#2.

Cause 10 of 10 — UpdateBucketMetadataJournalTableConfiguration, a journal expiration value outside its enum. AWS: 400 'InvalidRequest' / 'Request invalid'. Recorded behavior only. Emulator: 400 'MalformedXML' with the schema sentence. Why/Fix: as Cause 2. Traces: s3-metadata-journal-expiration-outside-enum#task#2.

Related open issue (does NOT cover these): 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md addresses the absence of context members such as TagKey on refusals that already carry the right Code; here the Code and Message themselves are wrong.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; status matched in all ten causes, body Code/Message diverged.
Representative s3-bucket-acl-grantee-type-vocabulary#task#3 (PutBucketAcl, expected 400 / actual 400, same divergence cluster as bucket-acl-grant-permission-outside-enum-refused#task#5): $.Error.Code value_mismatch (expected 'MalformedACLError', got 'MalformedXML').
Other causes, same shape:
- metadata-inventory-table-sse-algorithm-outside-enum-refused#task#2 (CreateBucketMetadataConfiguration): $.Error.Code expected 'InvalidRequest' got 'MalformedXML'; $.Error.Message expected 'Request invalid' got 'The XML you provided was not well-formed or did not validate against our published schema'.
- object-tag-set-key-and-value-length-bounds#task#7 (PutObjectTagging): Code expected 'InvalidTag' got 'MalformedXML'; Message expected 'The TagKey you have provided is invalid'; $.Error.TagKey missing (expected '').
- s3-analytics-and-filter-tag-key-floor#task#2 (PutBucketAnalyticsConfiguration): Code expected 'InvalidTag'; Message expected 'Tag Key cannot be empty or longer than 128 chars.'
- s3-bucket-tagging-key-length-floor#task#2 (PutBucketTagging): Code expected 'InvalidTag'; Message expected 'The TagKey you have provided is invalid'; TagKey missing.
- s3-delete-objects-empty-key-refused#task#3 (DeleteObjects): Code expected 'UserKeyMustBeSpecified'; Message expected 'User key must be specified.'
- s3-itc-filter-tag-key-length-floor#task#2 (PutBucketIntelligentTieringConfiguration): Code expected 'InvalidTag'; Message expected 'Tag Key cannot be null or empty.'
- s3-metadata-annotation-table-sse-algorithm-enum-refused#task#2, s3-metadata-inventory-table-sse-algorithm-outside-its-enum-rejected#task#2, s3-metadata-journal-expiration-outside-enum#task#2: Code expected 'InvalidRequest', Message expected 'Request invalid'.

## Steps to Reproduce
1. Replay fixture s3-bucket-acl-grantee-type-vocabulary and compare step task#3's response body against the recording: the status is 400 in both, but Error.Code must be MalformedACLError, not MalformedXML. The sibling step bucket-acl-grant-permission-outside-enum-refused#task#5 is the same cluster.
2. Repeat for each fixture#step listed in trace_ids (one per remaining cause) and for the other members of each cluster named under Traces.
3. For each, assert the recorded Error.Code and Error.Message (and the TagKey context member where the recording carries one). MalformedXML must only be answered when the document is genuinely not well-formed or fails the schema.
