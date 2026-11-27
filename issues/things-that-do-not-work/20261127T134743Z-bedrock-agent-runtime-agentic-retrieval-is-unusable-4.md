Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.agentic-retrieve-stream-incomplete-memory-configuration-rejected bedrock-agent-runtime.agentic-retrieve-stream-non-streaming-planner-model-ends-on-validation-event-rejected bedrock-agent-runtime.agentic-retrieve-stream-response-events-spell-the-result-answer bedrock-agent-runtime.agentic-retrieve-stream-unknown-custom-model-ends-on-validation-event-rejected bedrock-agent-runtime.agentic-retrieve-stream-zero-iterations-skips-planning bedrock-agent-runtime.agentic-retrieve-stream-unknown-retriever-refuses-whole-request-rejected bedrock-agent-runtime.agentic-retrieve-stream-iteration-ceiling-rejected bedrock-agent-runtime.agentic-retrieve-stream-traces-steps-before-one-final-result bedrock-agent-runtime.agentic-retrieve-stream-vector-knowledge-base-rejected bedrock-agent-runtime.agentic-retrieve-stream-rejects-an-invalid-custom-model-in-stream bedrock-agent-runtime.agentic-retrieve-stream-result-event-carries-ingested-passage
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|DependencyFailedException
Issue-Key: bedrock-agent-runtime.agentic-retrieve-stream-unknown-retriever-refuses-whole-request-rejected|_answers_alone#8|AssertionError|assert outcome["refusal"] is None, (
Issue-Key: bedrock-agent|StartIngestionJob|ConflictException
Issue-Key: bedrock-agent-runtime.agentic-retrieve-stream-vector-knowledge-base-rejected|_outcome#24|AssertionError|raise AssertionError(f"expected a refusal, but the stream completed: {events!r}")
Issue-Key: bedrock-agent-runtime.agentic-retrieve-stream-rejects-an-invalid-custom-model-in-stream|_until#2|Timeout|time.sleep(delay)
Issue-Key: bedrock-agent-runtime.agentic-retrieve-stream-result-event-carries-ingested-passage|_wait_for#6|Timeout|value = read()
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agent-runtime agentic retrieval is unusable: 4 net-new causes — MANAGED knowledge bases are refused as having no vector store, ingestion lands FAILED, VECTOR bases are answered instead of refused, and two streams never settle

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AgenticRetrieveStream

## Description
Four distinct root causes in the AgenticRetrieveStream surface and the knowledge-base ingestion it depends on. All four are in the bedrock-agent-runtime service and must each be closed before this issue is submitted.

Cause 1 of 4: AgenticRetrieveStream refuses a service-MANAGED knowledge base as a base with no vector store.
AWS: Smithy: AgenticRetrieveStream (POST /agenticRetrieveStream) takes retrievers: AgenticRetrievers (@length min 1, required), each AgenticRetriever.configuration -> RetrieverConfiguration naming a knowledgeBaseId. A knowledge base created with knowledgeBaseConfiguration.type = MANAGED / managedKnowledgeBaseConfiguration.embeddingModelType = MANAGED has NO customer vector store by design — AWS provisions and queries the store itself — and AgenticRetrieveStream over such a base is the operation's primary supported shape: it opens a 200 event stream and delivers trace/responseEvent/result events. DependencyFailedException is modeled (httpError 424) for a genuine downstream failure, not for a MANAGED base.
Emulator: every AgenticRetrieveStream naming a MANAGED knowledge base is refused, before any event streams, with HTTP 424 DependencyFailedException and the message "The vector store the knowledge base is backed by could not be queried: a knowledge base with no vector store. Check the resource configurations and retry the request." The fixed text of that message is entirely emulator-authored; AWS has no refusal class for "a knowledge base with no vector store". Because the refusal is unconditional, the checks never reach the behavior they grade (memoryConfiguration validation, planner-model validation, response/result event content, zero-iteration planning, multi-retriever not-found precedence).
Why: a client that creates the one knowledge-base flavour AWS recommends for agentic retrieval cannot perform agentic retrieval at all; every request is a 424, which clients classify as a transient dependency fault and retry forever instead of surfacing a configuration error.
Fix: the MANAGED-knowledge-base path in the AgenticRetrieveStream handler/its retriever resolution — a MANAGED base must resolve to the service-managed store and stream, not fall through to the "no vector store" refusal.
Traces: agentic-retrieve-stream-incomplete-memory-configuration-rejected, agentic-retrieve-stream-non-streaming-planner-model-ends-on-validation-event-rejected, agentic-retrieve-stream-response-events-spell-the-result-answer, agentic-retrieve-stream-unknown-custom-model-ends-on-validation-event-rejected, agentic-retrieve-stream-zero-iterations-skips-planning, agentic-retrieve-stream-unknown-retriever-refuses-whole-request-rejected (this last one reports the refusal as (424, 'DependencyFailedException', '<same message>')).

Cause 2 of 4: a freshly created data source is left in status FAILED, so StartIngestionJob can never be started on it.
AWS: recorded behavior only — a data source created on a knowledge base settles in AVAILABLE and StartIngestionJob on it is accepted (200, ingestionJob status STARTING/IN_PROGRESS). ConflictException on StartIngestionJob is reserved for a data source that is genuinely unusable or already has a job running.
Emulator: StartIngestionJob is refused with ConflictException and the message "You cannot start an ingestion job on a data source with status FAILED." — the data source the check just created has been put in FAILED. The fixed text is the status sentence; the variable part is the status value, which should never be FAILED for a newly created, well-formed data source.
Why: no document can ever be ingested, so every retrieval-grounded flow (agentic retrieval, RetrieveAndGenerate, GetDocumentContent) has nothing to retrieve; the client sees a terminal 409 for a resource it created correctly.
Fix: the data-source state machine behind CreateDataSource/StartIngestionJob (the knowledge-base state in the bedrock-agent crate) — a well-formed data source must reach AVAILABLE, not FAILED.
Traces: agentic-retrieve-stream-iteration-ceiling-rejected, agentic-retrieve-stream-traces-steps-before-one-final-result.

Cause 3 of 4: AgenticRetrieveStream over a VECTOR knowledge base completes a stream with a canned notice instead of refusing the unsupported base.
AWS: recorded behavior only — a knowledge base whose storage is a customer S3 Vectors index (type VECTOR) is not a supported retriever for AgenticRetrieveStream; the request is refused with HTTP 400 ValidationException, message "Knowledge Base with id <knowledgeBaseId> is not supported for AgenticRetrieveStream". The fixed text is "Knowledge Base with id " ... " is not supported for AgenticRetrieveStream"; the only variable part is the knowledgeBaseId taken from the request. Smithy: ValidationException is modeled on AgenticRetrieveStream with httpError 400.
Emulator: the request is ACCEPTED. The stream completes with a result event and a responseEvent whose answer is the constant emulator notice "No language model runs in this emulator. the service-managed model generated no response for this query, and the knowledge base retrieved no passage for it, so this response cites no source." and results: []. No refusal of any kind is emitted.
Why: a client gets a successful, empty, non-grounded answer for a configuration AWS rejects outright, so the unsupported-storage misconfiguration is invisible and the client treats the empty answer as "nothing matched".
Fix: the retriever-admissibility validation of the AgenticRetrieveStream handler — reject a VECTOR-storage knowledge base with the 400 ValidationException above before opening the stream.
Traces: agentic-retrieve-stream-vector-knowledge-base-rejected.

Cause 4 of 4: two agentic-retrieve streams never reach a state the caller can proceed from, exhausting the check budget.
AWS: recorded behavior only — knowledge-base creation, ingestion and the AgenticRetrieveStream read each settle in a bounded time; the recorded runs reach a terminal knowledge-base/ingestion status and a stream that ends, well inside the suite budget.
Emulator: both checks are reported as "Check timed out after 60.0s" — no response, no refusal, no terminal status. The emulator never settles the state these checks poll for (knowledge-base/ingestion status, or the stream's end), so the operation hangs rather than answering.
Why: a client polling for a terminal status or draining the event stream blocks indefinitely instead of getting an answer or an error; there is no code or status for it to act on.
Fix: the state transitions the agentic-retrieve path waits on — the knowledge-base/ingestion state machine and the AgenticRetrieveStream handler's stream termination must reach a terminal state and close the stream.
Traces: agentic-retrieve-stream-rejects-an-invalid-custom-model-in-stream, agentic-retrieve-stream-result-event-carries-ingested-passage.

## Evidence
Cause 1 — Expected: HTTP 200 and an event stream over a MANAGED knowledge base (Smithy AgenticRetrieveStream output.stream, @httpPayload @streaming, required). Actual: botocore.errorfactory.DependencyFailedException: An error occurred (DependencyFailedException) when calling the AgenticRetrieveStream operation: The vector store the knowledge base is backed by could not be queried: a knowledge base with no vector store. Check the resource configurations and retry the request. — and in the multi-retriever check the refusal is captured as (424, 'DependencyFailedException', 'The vector store the knowledge base is backed by could not be queried: a knowledge base with no vector store. Check the resource configurations and retry the request.') for the EXISTING base alone.

Cause 2 — Expected: StartIngestionJob accepted (200) on a newly created data source. Actual: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the StartIngestionJob operation: You cannot start an ingestion job on a data source with status FAILED.

Cause 3 — Expected: ('ValidationException', 'Knowledge Base with id <knowledge-base-id> is not supported for AgenticRetrieveStream'). Actual: no refusal; AssertionError: expected a refusal, but the stream completed: [{'result': {'generatedResponse': {'answer': 'No language model runs in this emulator. the service-managed model generated no response for this query, and the knowledge base retrieved no passage for it, so this response cites no source.'}, 'results': []}}, {'responseEvent': {'text': 'No language model runs in this emulator. ...'}}]. The same check's control read (Retrieve over the same ACTIVE base) succeeded, so the base was serving.

Cause 4 — Expected: the knowledge-base/ingestion wait and the stream drain complete. Actual: "Error: Check timed out after 60.0s" for both checks, with no HTTP response recorded.

## Steps to Reproduce
1. Boot the emulator and replay the bedrock-agent-runtime behavioral suite.
2. Cause 1: iam.create_role for bedrock.amazonaws.com; bedrock-agent.create_knowledge_base with knowledgeBaseConfiguration={'type':'MANAGED','managedKnowledgeBaseConfiguration':{'embeddingModelType':'MANAGED'}}; wait for ACTIVE; call bedrock-agent-runtime.agentic_retrieve_stream(messages=[{'role':'user','content':{'text':'<question>'}}], retrievers=[{'configuration':{'knowledgeBase':{'knowledgeBaseId':<kbId>}}}], agenticRetrieveConfiguration={'maxAgentIteration':1}). Observe HTTP 424 DependencyFailedException instead of a 200 stream.
3. Cause 2: create a knowledge base and a data source over an S3 prefix, then bedrock-agent.start_ingestion_job(knowledgeBaseId=..., dataSourceId=...). Observe 409 ConflictException naming status FAILED; bedrock-agent.get_data_source shows the data source in FAILED.
4. Cause 3: s3vectors.create_vector_bucket + create_index; create a knowledge base with VECTOR storage over that index; wait for ACTIVE; confirm runtime.retrieve succeeds; then call agentic_retrieve_stream over that kbId. Observe a completed stream with the constant notice instead of 400 ValidationException 'Knowledge Base with id <kbId> is not supported for AgenticRetrieveStream'.
5. Cause 4: run agentic-retrieve-stream-rejects-an-invalid-custom-model-in-stream and agentic-retrieve-stream-result-event-carries-ingested-passage; both exhaust the 60 s budget with no answer.
