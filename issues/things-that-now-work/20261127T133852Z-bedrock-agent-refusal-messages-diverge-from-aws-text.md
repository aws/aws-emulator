Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-retrieval-instruction-input-refusals
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-retrieval-instruction-input-refusals bedrock-agent-stop-ingestion-job-identifier-refusals bedrock-agent-vpc-configuration-unknown-knowledge-base bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|DeleteRetrievalInstruction|4xx->4xx|$.message
Issue-Key: bedrock-agent|CreateRetrievalInstruction|4xx->4xx|$.message
Issue-Key: bedrock-agent|StopIngestionJob|4xx->4xx|$.message
Issue-Key: bedrock-agent|GetVpcConfiguration|4xx->4xx|$.message
Issue-Key: bedrock-agent|IngestKnowledgeBaseDocuments|4xx->4xx|$.message
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agent refusal messages diverge from AWS text: anchored @pattern echoes, generic batch-limit message, and a 404 naming the wrong resource

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRetrievalInstruction, CreateRetrievalInstruction, StopIngestionJob, GetVpcConfiguration, IngestKnowledgeBaseDocuments

## Description
Six causes. Every one of them answers with the correct HTTP status and the correct error code, and diverges ONLY in the message body — so each is invisible to a client until it reads the message, and each is a separate defect a fixer must close. Causes 1-4 share one mechanism (the constraint-violation message renderer prints the Smithy @pattern with its ^ and $ anchors, which AWS strips); cause 5 is a service-specific ValidationException the emulator delegates to that generic renderer; cause 6 is a ResourceNotFoundException that names the wrong resource.

Cause 1 of 6: DeleteRetrievalInstruction — anchored pattern in the validation message.
AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models DeleteRetrievalInstruction input member retrievalInstructionVersion (target RetrievalInstructionNumericalVersion, @httpQuery retrievalInstructionVersion, @pattern ^[1-9][0-9]{0,8}$) and error ValidationException (errorKind client, httpError 400). The model does not fix the message text, so the text is recorded behavior only: AWS answers HTTP 400 ValidationException with the standard constraint sentence "<n> validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". The FIXED parts are that sentence frame; the VARIABLE parts are the error count, the rejected value and the member name (both taken from the request) and the pattern itself. AWS renders the pattern WITHOUT the anchors it uses to bound the match, i.e. [1-9][0-9]{0,8}.
Emulator: returns the same 400 / ValidationException but renders the raw modeled pattern including the anchors — "...Member must satisfy regular expression pattern: ^[1-9][0-9]{0,8}$".
Why: a client that parses the reported pattern out of the message (SDK test suites, CLI users, validation-mirroring clients that recompile the pattern, log matchers keyed on the documented sentence) gets a string AWS never emits; a client that re-anchors the pattern itself ends up with ^^...$$ and rejects values AWS accepts.
Fix: the shared constraint-validation message renderer in the request-validation layer (the code that formats "Member must satisfy regular expression pattern:" for a @pattern violation) — strip the leading ^ and trailing $ when printing the pattern. Fixing it there closes causes 1-4 at once.
Traces: bedrock-agent-retrieval-instruction-input-refusals#task#2.

Cause 2 of 6: CreateRetrievalInstruction — same anchored pattern, different member.
AWS: Smithy models CreateRetrievalInstruction input member name (target RetrievalInstructionName, required, @length min 1 max 100, @pattern ^([0-9a-zA-Z][_-]?){1,100}$, @sensitive) with error ValidationException (400). Recorded behavior only for the text: AWS answers 400 ValidationException with "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-zA-Z][_-]?){1,100}" — note that because the member is @sensitive the offending value is NOT echoed (fixed text "Value at '<member>'"), and the pattern is unanchored.
Emulator: same status and code, message ends "...pattern: ^([0-9a-zA-Z][_-]?){1,100}$".
Why: as cause 1; additionally this is the create path, so it is the first message most clients of this resource see.
Fix: same renderer as cause 1 (CreateRetrievalInstruction's input validation goes through it).
Traces: bedrock-agent-retrieval-instruction-input-refusals#task#3.

Cause 3 of 6: StopIngestionJob — same anchored pattern for the ingestionJobId label.
AWS: Smithy models StopIngestionJob input member ingestionJobId (target Id, @httpLabel, required, @pattern ^[0-9a-zA-Z]{10}$) with error ValidationException (400). Recorded: 400 ValidationException, "1 validation error detected: Value '<value>' at 'ingestionJobId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z]{10}" — the value is echoed for this non-sensitive member, the pattern is unanchored.
Emulator: same status and code, pattern printed as ^[0-9a-zA-Z]{10}$.
Why: as cause 1.
Fix: same renderer as cause 1.
Traces: bedrock-agent-stop-ingestion-job-identifier-refusals#task#5.

Cause 4 of 6: GetVpcConfiguration — anchored pattern inside a MULTI-error message.
AWS: Smithy models GetVpcConfiguration input member vpcConfigurationId (target VpcConfigurationId, @httpLabel, required, @length min 32 max 32, @pattern ^[a-z0-9](?:[a-z0-9-]{30}[a-z0-9])$) with error ValidationException (400). Recorded: 400 ValidationException with both violations in one message, "2 validation errors detected: Value '<value>' at 'vpcConfigurationId' failed to satisfy constraint: Member must have length greater than or equal to 32; Value '<value>' at 'vpcConfigurationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9](?:[a-z0-9-]{30}[a-z0-9])". The emulator already gets the count, the ordering, the ';' join and the length clause right — only the pattern clause carries anchors AWS does not print.
Emulator: same status, code and both clauses, but "...pattern: ^[a-z0-9](?:[a-z0-9-]{30}[a-z0-9])$".
Why: as cause 1.
Fix: same renderer as cause 1.
Traces: bedrock-agent-vpc-configuration-unknown-knowledge-base#task#4.

Cause 5 of 6: IngestKnowledgeBaseDocuments refuses an over-size documents batch with the generic constraint sentence instead of the service's own batch-limit message.
AWS: Smithy models IngestKnowledgeBaseDocuments input member documents (target KnowledgeBaseDocuments, required, @length min 1 max 10) and error ValidationException (400). The model states only the bound, so the message is recorded behavior only: for a batch larger than the bound AWS does NOT emit the generic "failed to satisfy constraint" sentence — it emits a service-authored ValidationException whose FIXED text is "The number of documents (<n>) exceeds the maximum allowed (<max>) for <kbType> knowledge base type.", where <n> is the number of documents in the request, <max> is the per-request bound (10) and <kbType> is the addressed knowledge base's type (MANAGED in the recording). Status 400, error code ValidationException.
Emulator: 400 ValidationException with "1 validation error detected: Value at 'documents' failed to satisfy constraint: Member must have length less than or equal to 10" — the generic list-length sentence, because the handler leaves the bound entirely to the modeled @length check and never performs its own batch-limit check (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/ingest_knowledge_base_documents.rs: handle() validates content kind and quota but has no documents-count refusal).
Why: the batch-limit message is the only place AWS states which knowledge-base type's limit was hit; a client (or an operator reading a log) that keys on it to resize its batch sees an unrelated generic constraint error and cannot tell the per-type limit from it.
Fix: IngestKnowledgeBaseDocuments' validation — refuse an over-size batch in the handler (or in a pre-validation hook that runs before the modeled @length check) with the service's batch-limit ValidationException message, interpolating the request's document count, the bound and the knowledge base's type.
Traces: bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals#task#8.

Cause 6 of 6: StopIngestionJob's 404 names the knowledge base when AWS names the missing ingestion job.
AWS: Smithy models StopIngestionJob with all three ids as required @httpLabel members and error ResourceNotFoundException (errorKind client, httpError 404, documented "The specified resource Amazon Resource Name (ARN) was not found..."). Recorded behavior for the text and the precedence: for a StopIngestionJob whose knowledgeBaseId does not exist, AWS answers HTTP 404 ResourceNotFoundException with the message "IngestionJob with id <ingestionJobId> is not found." — the FIXED text is "IngestionJob with id " / " is not found." and the only VARIABLE part is the ingestionJobId taken from the request. I.e. the refusal is reported against the addressed job, not against its parent.
Emulator: answers the correct 404 ResourceNotFoundException but with "KnowledgeBase with id <knowledgeBaseId> is not found.", because handle() checks live_knowledge_base() first and returns knowledge_base_not_found_message(..) before it ever looks for the job (stop_ingestion_job.rs lines 181-224: knowledge-base check at 181-193, data-source check at 194-206, job check at 211-224).
Why: the message is the only signal that distinguishes which resource of the triple is missing. A client that reads it concludes the knowledge base is gone and (for example) recreates it or aborts a retry loop, when against AWS the same request says only that this job id is unknown.
Fix: StopIngestionJob's handler — make the not-found precedence report the addressed ingestion job for this class of request, so a missing parent still yields the IngestionJob-not-found message AWS emits. Note the open issue '20261119T134619Z-bedrock-agentcore-control-deleteagentruntimeendpoint-not-found-message-names.md' is the same shape of defect in a DIFFERENT service (bedrock-agentcore-control) and does NOT cover this step; no open bedrock-agent issue carries this replay key.
Traces: bedrock-agent-stop-ingestion-job-identifier-refusals#task#6 (cluster also covers #task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All six clusters are verdict=mismatch with matching status and matching x-amzn-errortype; only $.message diverges.

Cause 1 — fixture bedrock-agent-retrieval-instruction-input-refusals, step task#2, DeleteRetrievalInstruction. Expected 400: "1 validation error detected: Value 'DRAFT' at 'retrievalInstructionVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: [1-9][0-9]{0,8}". Actual 400: same sentence ending "pattern: ^[1-9][0-9]{0,8}$". divergence_key service=bedrock-agent|DeleteRetrievalInstruction|status=400->400|keys=$.message.

Cause 2 — fixture bedrock-agent-retrieval-instruction-input-refusals, step task#3, CreateRetrievalInstruction. Expected 400: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-zA-Z][_-]?){1,100}". Actual 400: "...pattern: ^([0-9a-zA-Z][_-]?){1,100}$".

Cause 3 — fixture bedrock-agent-stop-ingestion-job-identifier-refusals, step task#5, StopIngestionJob. Expected 400: "1 validation error detected: Value 'job_00001' at 'ingestionJobId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z]{10}". Actual 400: "...pattern: ^[0-9a-zA-Z]{10}$".

Cause 4 — fixture bedrock-agent-vpc-configuration-unknown-knowledge-base, step task#4, GetVpcConfiguration. Expected 400: "2 validation errors detected: Value 'short-id' at 'vpcConfigurationId' failed to satisfy constraint: Member must have length greater than or equal to 32; Value 'short-id' at 'vpcConfigurationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9](?:[a-z0-9-]{30}[a-z0-9])". Actual 400: identical except "pattern: ^[a-z0-9](?:[a-z0-9-]{30}[a-z0-9])$".

Cause 5 — fixture bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals, step task#8, IngestKnowledgeBaseDocuments, 11 documents in one request. Expected 400: "The number of documents (11) exceeds the maximum allowed (10) for MANAGED knowledge base type." Actual 400: "1 validation error detected: Value at 'documents' failed to satisfy constraint: Member must have length less than or equal to 10".

Cause 6 — fixture bedrock-agent-stop-ingestion-job-identifier-refusals, step task#6 (and task#7), StopIngestionJob against a knowledge base id that does not exist. Expected 404: "IngestionJob with id JOBQ000001 is not found." Actual 404: "KnowledgeBase with id NOKB000077 is not found." divergence_key service=bedrock-agent|StopIngestionJob|status=404->404|keys=$.message; affected_count 2. (The ids quoted here are this recording's values — the requirement is the message template with the request's ingestionJobId substituted.)

## Steps to Reproduce
1. Boot the emulator with the bedrock-agent service mounted.
2. Cause 1: DELETE /retrievalinstructions/{id}?retrievalInstructionVersion=DRAFT (a version that is not a decimal number). Observe the 400 ValidationException message printing the pattern with ^...$ anchors; AWS prints it unanchored.
3. Cause 2: POST /retrievalinstructions/ with a name that violates ^([0-9a-zA-Z][_-]?){1,100}$ (e.g. containing a space). Same anchor leak, and note AWS does not echo the @sensitive value.
4. Cause 3: POST /knowledgebases/{kb}/datasources/{ds}/ingestionjobs/{ingestionJobId}/stop with ingestionJobId='job_00001'. Same anchor leak.
5. Cause 4: GET /knowledgebases/{kb}/vpcconfigurations/short-id. Both clauses are produced; the pattern clause carries anchors AWS omits.
6. Cause 5: PUT /knowledgebases/{kb}/datasources/{ds}/documents for a MANAGED knowledge base with 11 entries in documents. Emulator emits the generic @length sentence; AWS emits "The number of documents (11) exceeds the maximum allowed (10) for MANAGED knowledge base type."
7. Cause 6: POST /knowledgebases/NOKB000077/datasources/{ds}/ingestionjobs/JOBQ000001/stop where knowledge base NOKB000077 does not exist. Emulator: 404 "KnowledgeBase with id NOKB000077 is not found."; AWS: 404 "IngestionJob with id JOBQ000001 is not found."
8. Equivalent: replay fixtures bedrock-agent-retrieval-instruction-input-refusals (task#2, task#3), bedrock-agent-stop-ingestion-job-identifier-refusals (task#5, task#6, task#7), bedrock-agent-vpc-configuration-unknown-knowledge-base (task#4) and bedrock-agent-ingest-documents-batch-limit-and-metadata-source-refusals (task#8).
