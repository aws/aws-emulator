Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals bedrock-agent-stop-ingestion-job-identifier-refusals
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|$.message
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->2xx|status
Issue-Key: bedrock-agent|GetKnowledgeBaseDocuments|2xx->2xx|$.documentDetails[*]
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|$.message
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|StartIngestionJob|4xx->2xx|status
Issue-Key: bedrock-agent|ListIngestionJobs|2xx->2xx|$.ingestionJobSummaries[*]
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agent accepts ingestion requests AWS refuses (202 instead of 400), leaving phantom documents and ingestion jobs in state

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IngestKnowledgeBaseDocuments, StartIngestionJob, GetKnowledgeBaseDocuments, ListIngestionJobs

## Description
Four causes. Two ingestion pre-conditions AWS enforces are not checked at all, so the emulator answers the modeled success status (202) to a request AWS refuses with 400 ValidationException; the other two causes are the state those accepted-but-invalid requests leave behind, which a later read-only call then reports. Each is its own divergence cluster and each must be closed.

Cause 1 of 4: IngestKnowledgeBaseDocuments accepts inline document content whose metadata is NOT inline.
AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models KnowledgeBaseDocument with content (DocumentContent, required) and metadata (DocumentMetadata); DocumentMetadata.type is MetadataSourceType with values IN_LINE_ATTRIBUTE and S3_LOCATION, and DocumentContent.custom carries inline content. The model expresses no cross-member rule, so the refusal is recorded behavior only: when a document's content is defined INLINE but its metadata names an external source (S3_LOCATION), AWS refuses the whole request with HTTP 400, x-amzn-errortype ValidationException and the message "Because your document's content was defined inline, you must also define its metadata inline. Retry your request with the metadata defined inline." That text is entirely FIXED — it interpolates nothing from the request (no document id, no index, no member name).
Emulator: answers HTTP 202 with a documentDetails list, i.e. it ingests the document. No error body and no x-amzn-errortype at all. The handler (src/ops/ingest_knowledge_base_documents.rs, loop at lines 267-304) checks only that each document's content.dataSourceType matches the data source's ingestible kind and that the matching content member is present; metadata is passed straight into DocumentRecord::starting (line 374) without ever being compared against the content's inline-ness.
Why: a client whose request AWS rejects as malformed gets a success from the emulator, so the error path is untested and the knowledge base silently holds a document whose metadata source AWS would never have accepted. Every later read of that data source disagrees with AWS.
Fix: IngestKnowledgeBaseDocuments' validation — add the inline-content/inline-metadata consistency pre-condition to the per-document loop, refusing the whole request (nothing written for any document) with that ValidationException message.
Traces: bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals#task#9.

Cause 2 of 4: GetKnowledgeBaseDocuments reports a document the data source does not hold as INDEXED instead of NOT_FOUND.
AWS: Smithy models GetKnowledgeBaseDocuments -> KnowledgeBaseDocumentDetail with required knowledgeBaseId, dataSourceId, identifier and status (DocumentStatus, whose values include NOT_FOUND alongside INDEXED, PENDING, STARTING, ...), plus an optional updatedAt. Recorded behavior: an identifier the addressed data source does not hold is returned as a detail entry with status NOT_FOUND, echoing the requested identifier and dataSourceType, and with NO updatedAt (there is no document to have been updated). The entry is neither omitted nor reported as indexed.
Emulator: returns the entry with status INDEXED and an updatedAt timestamp, because the document was written by the request of cause 1 that should have been refused.
Why: a client uses GetKnowledgeBaseDocuments to decide whether a document needs (re-)ingesting; INDEXED for a document AWS reports as NOT_FOUND inverts that decision, and the spurious updatedAt makes the phantom look freshly synced.
Hypothesis: this entry is the stored-state consequence of cause 1 and may clear once cause 1 refuses the ingest — it must be re-replayed after that fix. Independently of cause 1, the handler must still map an identifier it holds no record for to a NOT_FOUND detail with no updatedAt.
Fix: GetKnowledgeBaseDocuments' handler/state lookup (the not_found_document_detail path in crate::state) must be what answers an unheld identifier. Note the open issue '20261124T235658Z-bedrock-agent-listknowledgebasedocuments-drops-a-completed-deletion-s.md' covers a DIFFERENT operation (ListKnowledgeBaseDocuments) and a different cause (a completed deletion dropped from the listing); it carries no replay key for this step and does not cover it.
Traces: bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals#task#12.

Cause 3 of 4: StartIngestionJob accepts a CUSTOM data source instead of refusing it.
AWS: Smithy models StartIngestionJob (PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/ingestionjobs/, success 202, error ValidationException 400) and DataSourceType with the value CUSTOM; the operation is documented as "Begins a data ingestion job. Data sources are ingested into your knowledge base...". The model states no type restriction, so the refusal is recorded behavior only: a CUSTOM data source has no source for the service to crawl — its documents are pushed with IngestKnowledgeBaseDocuments — so AWS refuses StartIngestionJob for one with HTTP 400, x-amzn-errortype ValidationException and the message "CUSTOM data sources don't support the StartIngestionJob operation." The FIXED text is "<type> data sources don't support the StartIngestionJob operation."; the only VARIABLE part is the addressed data source's type, read from service state.
Emulator: answers HTTP 202 with a started ingestionJob (status STARTING) and no error body or x-amzn-errortype — it starts a job for a data source that can never have one.
Why: the client's error path is never exercised and the emulator then reports an ingestion job AWS would not have created, so any workflow that polls that job for completion diverges from AWS from the first call onwards.
Fix: StartIngestionJob's validation — refuse the request when the addressed data source's type (or its configuration's ingestible-content kind, the same notion crate::state::ingestible_content_type expresses) admits no crawled content, with that ValidationException message interpolating the data source's type.
Traces: bedrock-agent-stop-ingestion-job-identifier-refusals#task#8.

Cause 4 of 4: ListIngestionJobs returns the phantom ingestion job, so the listing is non-empty where AWS returns none.
AWS: Smithy models ListIngestionJobs -> ingestionJobSummaries (required IngestionJobSummaries) and nextToken. Recorded behavior: for a data source on which no ingestion job could be started, the listing contains NO summary (ingestionJobSummaries empty).
Emulator: returns one IngestionJobSummary for the job created by cause 3 (knowledgeBaseId/dataSourceId/ingestionJobId, description, status STARTING, startedAt/updatedAt and a zeroed statistics block). Replay verdict: $.ingestionJobSummaries[0] extra (expected None).
Why: a client listing a data source's ingestion history sees a job AWS does not report, so convergence checks ("wait until no job is STARTING/IN_PROGRESS") and idempotence checks behave differently against the emulator.
Hypothesis: this is the stored-state consequence of cause 3 and is expected to clear once StartIngestionJob refuses the CUSTOM data source; it must be re-replayed after that fix rather than assumed closed, since ListIngestionJobs is the only step that asserts the absence.
Fix: no separate listing filter is intended — the fix belongs in StartIngestionJob's validation (cause 3) so the record is never written; verify via this step that ListIngestionJobs then reports an empty ingestionJobSummaries.
Traces: bedrock-agent-stop-ingestion-job-identifier-refusals#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals, step task#9, IngestKnowledgeBaseDocuments, verdict mismatch. Expected: HTTP 400, x-amzn-errortype ValidationException (shaped, not asserted on value), $.message = "Because your document's content was defined inline, you must also define its metadata inline. Retry your request with the metadata defined inline.". Actual: HTTP 202, no $.message, no x-amzn-errortype header. divergence_key service=bedrock-agent|IngestKnowledgeBaseDocuments|status=400->202|keys=$.message,$headers.x-amzn-errortype,status.

Cause 2 — same fixture, step task#12, GetKnowledgeBaseDocuments, HTTP 200 both sides. Expected $.documentDetails[4] = {dataSourceId, identifier {custom {id}, dataSourceType CUSTOM}, knowledgeBaseId, status 'NOT_FOUND'} with no updatedAt. Actual: the same identifier reported with status 'INDEXED' plus an updatedAt timestamp. divergence_key service=bedrock-agent|GetKnowledgeBaseDocuments|status=200->200|keys=$.documentDetails[4]. (The ids/timestamps in the recording are per-run values; the requirement is status NOT_FOUND and the absence of updatedAt for an identifier the data source does not hold.)

Cause 3 — fixture bedrock-agent-stop-ingestion-job-identifier-refusals, step task#8, StartIngestionJob, verdict mismatch. Expected: HTTP 400, x-amzn-errortype ValidationException, $.message = "CUSTOM data sources don't support the StartIngestionJob operation.". Actual: HTTP 202, no $.message, no x-amzn-errortype. divergence_key service=bedrock-agent|StartIngestionJob|status=400->202|keys=$.message,$headers.x-amzn-errortype,status.

Cause 4 — same fixture, step task#9, ListIngestionJobs, HTTP 200 both sides. Expected: no $.ingestionJobSummaries[0]. Actual: one summary with status 'STARTING', description 'sync the handbook', startedAt/updatedAt and an all-zero statistics block — the job created by cause 3. divergence_key service=bedrock-agent|ListIngestionJobs|status=200->200|keys=$.ingestionJobSummaries[0].

Source reference for causes 1-2: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/ingest_knowledge_base_documents.rs, per-document validation loop lines 267-304 and record write at lines 365-391 (metadata is stored unchecked at line 374).

## Steps to Reproduce
1. Boot the emulator with the bedrock-agent service mounted and create a knowledge base with a CUSTOM data source.
2. Cause 1: PUT /knowledgebases/{kb}/datasources/{ds}/documents with one document whose content is inline (content.dataSourceType=CUSTOM, content.custom with inline content) and whose metadata.type=S3_LOCATION with an s3Location. Emulator answers 202 and ingests it; AWS answers 400 ValidationException "Because your document's content was defined inline, you must also define its metadata inline. Retry your request with the metadata defined inline."
3. Cause 2: POST /knowledgebases/{kb}/datasources/{ds}/documents/getDocuments asking for that document's identifier (plus any others). Emulator reports it with status INDEXED and an updatedAt; AWS reports status NOT_FOUND with no updatedAt.
4. Cause 3: PUT /knowledgebases/{kb}/datasources/{ds}/ingestionjobs/ for that CUSTOM data source. Emulator answers 202 with an ingestionJob in STARTING; AWS answers 400 ValidationException "CUSTOM data sources don't support the StartIngestionJob operation."
5. Cause 4: POST /knowledgebases/{kb}/datasources/{ds}/ingestionjobs/ (ListIngestionJobs) for the same data source. Emulator returns one summary for the job from step 4; AWS returns an empty ingestionJobSummaries.
6. Equivalent: replay fixture bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals (task#9, task#12) and fixture bedrock-agent-stop-ingestion-job-identifier-refusals (task#8, task#9).
