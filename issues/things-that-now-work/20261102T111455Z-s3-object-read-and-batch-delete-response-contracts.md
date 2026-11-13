Issue-Source: trace-sweep-divergence
Issue-Trace: s3-rcsv-bucket-reference-mistakes
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-rcsv-bucket-reference-mistakes s3-rcsv-encryption-header-mistakes-on-sse-kms-read s3-rcsv-key-mistakes-then-prefix-discovery s3-rcsv-object-attributes-route-mistakes s3-rcsv-part-number-reads-on-single-part-csv s3-rcsv-reference-cp-head-then-get-sse-kms s3-rcsv-version-id-mistakes-on-versioned-csv s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key s3-ovm-traj-delete-marker-as-most-recent-entry s3-mvb-reference-move-versioned s3-csbs-object-attributes-request-mistakes s3-csvcols-encryption-header-mistakes s3-mbes-traj-delete-objects-batch-mistakes s3-b2b-summary-inline-tagging-header-mistakes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-server-side-encryption-aws-kms-key-id
Issue-Key: s3|HeadObject|2xx->2xx|$headers.x-amz-server-side-encryption-aws-kms-key-id
Issue-Key: s3|PutObject|2xx->2xx|$headers.x-amz-server-side-encryption-aws-kms-key-id
Issue-Key: s3|HeadObject|4xx->4xx|$headers.x-amz-delete-marker
Issue-Key: s3|HeadObject|4xx->4xx|$headers.x-amz-version-id
Issue-Key: s3|HeadObject|4xx->4xx|$headers.last-modified
Issue-Key: s3|HeadObject|4xx->4xx|$
Issue-Key: s3|GetObjectAttributes|4xx->2xx|$
Issue-Key: s3|GetObjectAttributes|4xx->2xx|$headers.content-type
Issue-Key: s3|GetObjectAttributes|4xx->2xx|status
Issue-Key: s3|HeadObject|4xx->2xx|$headers.content-type
Issue-Key: s3|HeadObject|4xx->2xx|status
Issue-Key: s3|DeleteObjects|2xx->4xx|$.DeleteResult
Issue-Key: s3|DeleteObjects|2xx->4xx|status
Issue-Key: s3|DeleteObjects|2xx->2xx|$.DeleteResult.Error.Code
Issue-Key: s3|DeleteObjects|2xx->2xx|$.DeleteResult.Error.Message
Issue-Key: s3|PutObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutObject|4xx->4xx|$.Error.ArgumentValue
Issue-Related: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 object read and batch delete response contracts diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 7: SSE-KMS reads omit the key-id header. AWS: a GetObject/HeadObject/PutObject of an object encrypted with SSE-KMS returns x-amz-server-side-encryption-aws-kms-key-id carrying the key ARN the object is bound to. Recorded behavior only. Emulator: the header is absent on 200 and 206 reads, on HEAD, and on the PutObject response. Why: a client verifying which key protects an object (or echoing it into an audit) gets nothing. Fix: the object read/write response header assembly, reading the key recorded on the object in state. Traces: s3-rcsv-bucket-reference-mistakes#task#7 (cluster of 8 steps), s3-rcsv-part-number-reads-on-single-part-csv#task#4, s3-rcsv-reference-cp-head-then-get-sse-kms#task#4, s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key#task#10.

Cause 2 of 7: delete-marker reads omit their signalling headers. AWS: a HEAD or GET whose latest version is a delete marker answers 404/405 with x-amz-delete-marker: true, x-amz-version-id = the marker's version id and (on 405) Last-Modified. Recorded behavior only. Emulator: none of those headers is set. Why: a client cannot distinguish a deleted object from a missing one, nor address the marker to restore the object. Fix: the delete-marker branch of the object read handler. Traces: s3-ovm-traj-delete-marker-as-most-recent-entry#task#7, s3-ovm-traj-delete-marker-as-most-recent-entry#task#8, s3-mvb-reference-move-versioned#task#17.

Cause 3 of 7: GetObjectAttributes does not require x-amz-object-attributes. AWS: a request with the header missing or empty answers 400 Code 'InvalidRequest', Message 'The x-amz-object-attributes header specifying the attributes to be retrieved is either missing or empty', content-type application/xml. Smithy: GetObjectAttributes.ObjectAttributes is a REQUIRED httpHeader member (x-amz-object-attributes). Emulator: 200 with the object's body and content-type text/plain - it serves a plain GetObject instead. Why: the caller gets an object body where an attributes document (or a refusal) belongs. Fix: enforce the required header in the GetObjectAttributes handler and route the request away from the object-body path. Traces: s3-csbs-object-attributes-request-mistakes#task#3.

Cause 4 of 7: malformed SSE-C read headers are ignored. AWS: a HEAD carrying invalid server-side-encryption-customer-* headers answers 400 with an application/xml Error. Recorded behavior only. Emulator: 200 with content-type text/csv - the object is served. Why: a request AWS refuses returns plaintext, which is a confidentiality-relevant divergence. Fix: SSE-C header validation in the object read path. Traces: s3-csvcols-encryption-header-mistakes#task#7.

Cause 5 of 7: DeleteObjects refuses a valid batch and re-words its schema refusal. AWS: the batch in task#6 answers 200 with <DeleteResult><Deleted> naming the key and its VersionId; an over-limit batch answers 400 MalformedXML with exactly 'The XML you provided was not well-formed or did not validate against our published schema'. Recorded behavior only. Emulator: 400 with no DeleteResult for the valid batch, and the schema sentence with an appended clause ': a Delete request carries from 1 to 1000 Object elements, and this one carries 1001'. Why: bulk deletes fail outright, and the refusal text is not AWS's. Fix: the DeleteObjects request validator and its error serializer. Traces: s3-mbes-traj-delete-objects-batch-mistakes#task#6, s3-mbes-traj-delete-objects-batch-mistakes#task#7.

Cause 6 of 7: per-key errors inside a 200 DeleteResult carry the wrong code. AWS: a bad version id for one key is reported as <Error><Code>NoSuchVersion</Code><Message>The specified version does not exist.</Message>. Recorded behavior only. Emulator: 'InvalidArgument' / 'Invalid version id specified'. Why: a batch client that retries NoSuchVersion separately from InvalidArgument takes the wrong branch per key. Fix: the per-key error mapping in the DeleteObjects handler. Traces: s3-mbes-traj-delete-objects-batch-mistakes#task#8.

Cause 7 of 7: the x-amz-tagging header refusal uses the wrong class and sentence. AWS: 400 Code 'InvalidArgument', Message "The header 'x-amz-tagging' shall be encoded as UTF-8 then URLEncoded URL query parameters without tag name duplicates." (fixed text) with ArgumentName 'x-amz-tagging' and ArgumentValue the header as sent. Recorded behavior only. Emulator: 400 Code 'InvalidTag', Message 'The TagValue you have provided is invalid'. Why: the caller is told a tag value is bad when the fault is the header's encoding. Fix: the inline-tagging header parser in PutObject. Traces: s3-b2b-summary-inline-tagging-header-mistakes#task#4.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-rcsv-bucket-reference-mistakes#task#7 (GetObject 200/200): x-amz-server-side-encryption-aws-kms-key-id missing (cluster of 8 steps). s3-ovm-traj-delete-marker-as-most-recent-entry#task#7: x-amz-delete-marker and x-amz-version-id missing on the 404; #task#8 adds last-modified on the 405. s3-mvb-reference-move-versioned#task#17: body expected {'VersionId': ...} and the two delete-marker headers, got an empty body. s3-csbs-object-attributes-request-mistakes#task#3: status expected 400 InvalidRequest 'The x-amz-object-attributes header specifying the attributes to be retrieved is either missing or empty', got 200 with body 'Hello world!!.' and content-type text/plain. s3-csvcols-encryption-header-mistakes#task#7: expected 400 application/xml, got 200 text/csv. s3-mbes-traj-delete-objects-batch-mistakes#task#6: expected 200 with DeleteResult.Deleted, got 400; #task#7: Message carries an extra clause after AWS's fixed sentence; #task#8: DeleteResult.Error.Code expected 'NoSuchVersion' got 'InvalidArgument'. s3-b2b-summary-inline-tagging-header-mistakes#task#4: Code expected 'InvalidArgument' got 'InvalidTag'.

## Steps to Reproduce
1. Replay s3-rcsv-bucket-reference-mistakes step task#7 and check the SSE-KMS key-id header. 2. Replay s3-ovm-traj-delete-marker-as-most-recent-entry steps task#7/task#8 and s3-mvb-reference-move-versioned step task#17. 3. Replay s3-csbs-object-attributes-request-mistakes step task#3 without x-amz-object-attributes. 4. Replay s3-csvcols-encryption-header-mistakes step task#7. 5. Replay s3-mbes-traj-delete-objects-batch-mistakes steps task#6, task#7, task#8. 6. Replay s3-b2b-summary-inline-tagging-header-mistakes step task#4.
