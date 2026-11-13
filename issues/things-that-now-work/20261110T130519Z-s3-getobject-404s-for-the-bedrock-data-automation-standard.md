Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Shape-Id: com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService
Issue-Key: s3|GetObject|2xx->4xx|$.document
Issue-Key: s3|GetObject|2xx->4xx|$.metadata
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Commit: 3d6b446e08b087d2665f5ca5dd389ba075b4c25a

# GetObject 404s for the Bedrock Data Automation standard-output result object a finished job never writes

> _Note: this issue was identified against the code base at commit `3d6b446e08b087d2665f5ca5dd389ba075b4c25a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: after an asynchronous data-automation job reports a terminal Success, reading that job's standard-output result object out of the submitted output bucket fails.

AWS: recorded behavior only for the output layout (the Smithy models of neither s3 GetObject nor bedrock-data-automation-runtime state the key layout). The recording shows that a successful InvokeDataAutomationAsync job materializes its extraction output into the bucket/prefix the submission's outputConfiguration.s3Uri names, under a per-job segment: <prefix>/<invocationId>/<assetIndex>/standard_output/<index>/result.json. A GetObject on that key answers 200 with a JSON body carrying the extraction result — a 'metadata' object (asset_id, logical_subdocument_id, semantic_modality 'DOCUMENT', the source s3_bucket / s3_key, number_of_pages, start_page_index, end_page_index, file_type) and a 'document' object whose representation.text holds the text extracted from the input asset — plus the ordinary object-read headers for a stored object: content-type binary/octet-stream, accept-ranges bytes, last-modified, etag, x-amz-checksum-crc32 with x-amz-checksum-type FULL_OBJECT, and x-amz-server-side-encryption AES256. Host/bucket names, the generated invocationId, the etag, last-modified and the checksum value are per-run and are not requirements; the 200, the key layout, the two body objects and the header set are.

Emulator: the same GetObject answers 404 with content-type application/xml (an S3 error document) and no body members at all — $.metadata and $.document missing, and accept-ranges / last-modified / etag / x-amz-checksum-crc32 / x-amz-checksum-type / x-amz-server-side-encryption all absent. The object simply does not exist: the data-automation job settles its RECORD to Success without ever writing any output object, which the emulator states outright (services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/state.rs, Invocation::advance_to doc, lines 957-963: 'It does not claim that result objects exist at output_metadata_s3_uri ... nothing here writes them or synthesises analysis output'), and InvokeDataAutomationAsync's handler performs no object write at all. So S3 correctly reports a missing key; the divergence is that the finished job produced nothing to read.

Why: a client that relies on AWS's behavior drives the documented flow — submit, poll until Success, then read the job's output objects out of its own bucket — and gets 404 NoSuchKey for every result file after a job the service told it had succeeded. The extracted text is unreachable, so the whole async extraction workflow is unusable even though every status response looked healthy; a client that treats 404 after Success as data loss or as a bucket/permission fault fails in a way it cannot diagnose.

Fix: the data-automation job's terminal-transition path (the settle in state.rs that advances an Invocation to Success, reached from InvokeDataAutomationAsync's settle_when_due and from GetDataAutomationStatus) must write the job's output into the submitted outputConfiguration.s3Uri through the in-process S3 callback it already uses to READ the input asset — the per-job standard_output result object(s) carrying metadata/document, alongside the job_metadata.json index whose location the status response already reports. Not S3's GetObject handler: it answers a genuinely absent key.

Traces: bedrock-data-automation-runtime-async-document-extraction-writes-text#task#8. The same fixture's task#7 (ListObjectsV2 returning no Contents for that prefix) is the listing-side shadow of the same absence and is already owned by the open issue 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md, which covers the prefix-decoding defect; that issue does NOT cover this read, so it is filed here.

## Evidence
Fixture bedrock-data-automation-runtime-async-document-extraction-writes-text, step task#8, GetObject: status expected 200, got 404.
Body: $.metadata missing (expected {'asset_id': '0', 'logical_subdocument_id': '0', 'semantic_modality': 'DOCUMENT', 's3_bucket': <input bucket>, 's3_key': 'input/invoice.pdf', 'number_of_pages': 1, 'start_page_index': 0, 'end_page_index': 0, 'file_type': 'PDF'}, got None); $.document missing (expected {'representation': {'text': <text extracted from the input asset>}, 'statistics': {}}, got None).
Headers: content-type expected 'binary/octet-stream', got 'application/xml' (an S3 error document); accept-ranges expected 'bytes', got None; x-amz-checksum-type expected 'FULL_OBJECT', got None; x-amz-server-side-encryption expected 'AES256', got None; last-modified, etag and x-amz-checksum-crc32 expected present (values per-run, not asserted), got None.
The key read is the per-job standard output under the submitted prefix, of the form output/<invocationId>/0/standard_output/0/result.json (the recorded run's invocationId segment was 18e6b0ae7b2619dc-0000000000000001 — a generated id, not a requirement).
Code: services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/ops/invoke_data_automation_async.rs (handler mints and persists the invocation, writes no object); src/state.rs lines 957-963 and Invocation::output_metadata_s3_uri lines 1059-1065.

## Steps to Reproduce
1. Replay the wire corpus; the diverging step is fixture bedrock-data-automation-runtime-async-document-extraction-writes-text step task#8.
2. Minimal repro: PutObject an input document into a bucket, call InvokeDataAutomationAsync with inputConfiguration.s3Uri pointing at it and outputConfiguration.s3Uri pointing at s3://<bucket>/output, poll GetDataAutomationStatus until status is Success, then GetObject the per-job standard output key under that prefix (output/<invocationId>/0/standard_output/0/result.json). AWS answers 200 with the metadata/document JSON and the stored-object headers; the emulator answers 404 with an application/xml error document, and a ListObjectsV2 under the prefix shows no object was ever written.
