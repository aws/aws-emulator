Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.batch-create-memory-records-all-entries-refused-still-answers bedrock-agentcore.batch-create-memory-records-duplicate-request-identifier-refused-once bedrock-agentcore.batch-create-memory-records-failed-entries-rejected-and-not-stored bedrock-agentcore.batch-create-memory-records-rejects-repeated-request-identifier-entry bedrock-agentcore.batch-create-memory-records-rejects-unknown-strategy-entry-beside-accepted-one bedrock-agentcore.batch-create-memory-records-unknown-strategy-entry-rejected-beside-stored-entry bedrock-agentcore.batch-create-memory-records-bare-namespace-stored-wrapped bedrock-agentcore.delete-memory-record-unnormalized-namespace-rejected bedrock-agentcore.get-memory-record-other-namespace-rejected bedrock-agentcore.list-memory-records-without-namespace-scope-rejected bedrock-agentcore.batch-delete-memory-records-refuses-missing-ids-by-id bedrock-agentcore.batch-update-memory-records-all-unknown-ids-refused-record-unchanged bedrock-agentcore.code-interpreter-stop-task-on-finished-task-rejected bedrock-agentcore.list-actors-forged-next-token-rejected bedrock-agentcore.list-actors-unissued-next-token-rejected bedrock-agentcore.list-batch-evaluations-unissued-next-token-rejected bedrock-agentcore.list-events-forged-next-token-rejected bedrock-agentcore.list-events-unissued-next-token-rejected bedrock-agentcore.list-memory-records-forged-next-token-rejected bedrock-agentcore.list-memory-records-unissued-next-token-rejected bedrock-agentcore.list-payment-sessions-unissued-next-token-rejected bedrock-agentcore.list-sessions-forged-next-token-rejected bedrock-agentcore.list-sessions-unissued-next-token-rejected bedrock-agentcore.retrieve-memory-records-unissued-next-token-rejected bedrock-agentcore.list-events-page-token-bound-to-its-metadata-filter-rejected bedrock-agentcore.memory-arn-outside-endpoint-region-or-partition-rejected bedrock-agentcore.get-workload-access-token-deleted-identity-refused bedrock-agentcore.workload-token-for-user-id-unknown-workload-rejected bedrock-agentcore.invoke-agent-runtime-agent-error-status-rejected bedrock-agentcore.invoke-agent-runtime-nonexistent-not-found bedrock-agentcore.invoke-code-interpreter-inactive-and-unknown-session-rejected bedrock-agentcore.invoke-code-interpreter-runtime-not-matching-language-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.batch-create-memory-records-all-entries-refused-still-answers|run#8|AssertionError|assert refused["successfulRecords"] == [], (
Issue-Key: bedrock-agentcore.batch-create-memory-records-duplicate-request-identifier-refused-once|run#11|AssertionError|assert ok == ["draft-a", "draft-b"], f"expected one success per distinct identifier, got {ok!r}"
Issue-Key: bedrock-agentcore.batch-create-memory-records-failed-entries-rejected-and-not-stored|run#8|AssertionError|assert sorted(stored) == ["kept-one", "twice"], (
Issue-Key: bedrock-agentcore.batch-create-memory-records-rejects-repeated-request-identifier-entry|run#9|AssertionError|assert len(ok) == 1, f"expected exactly 1 accepted entry, got {len(ok)}: {ok!r}"
Issue-Key: bedrock-agentcore.batch-create-memory-records-rejects-unknown-strategy-entry-beside-accepted-one|run#10|AssertionError|assert [r["requestIdentifier"] for r in ok] == ["entry-kept"], (
Issue-Key: bedrock-agentcore.batch-create-memory-records-unknown-strategy-entry-rejected-beside-stored-entry|run#14|AssertionError|assert [r["requestIdentifier"] for r in ok] == ["kept-entry"], (
Issue-Key: bedrock-agentcore.batch-create-memory-records-bare-namespace-stored-wrapped|run#19|AssertionError|assert stored["namespaces"] == [_WRAPPED], (
Issue-Key: bedrock-agentcore.delete-memory-record-unnormalized-namespace-rejected|run#8|AssertionError|assert stored["namespaces"] == ["/pottery/glazes/"], (
Issue-Key: bedrock-agentcore.get-memory-record-other-namespace-rejected|_refusal#13|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: bedrock-agentcore.list-memory-records-without-namespace-scope-rejected|_refusal#11|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: bedrock-agentcore.batch-delete-memory-records-refuses-missing-ids-by-id|_create_records#4|AssertionError|assert misshapen == [], f"minted record ids not shaped mem-<uuid> for {misshapen!r}"
Issue-Key: bedrock-agentcore.batch-update-memory-records-all-unknown-ids-refused-record-unchanged|_seed#4|AssertionError|assert misshapen == [], f"minted record ids not shaped mem-<uuid> for {misshapen!r}"
Issue-Key: bedrock-agentcore.code-interpreter-stop-task-on-finished-task-rejected|_issued_task#7|AssertionError|assert isinstance(task_id, str) and _TASK_ID.fullmatch(task_id), (
Issue-Key: bedrock-agentcore.list-actors-forged-next-token-rejected|run#14|AssertionError|assert code == "InvalidInputException", f"a forged token answered {code!r}"
Issue-Key: bedrock-agentcore.list-actors-unissued-next-token-rejected|run#21|AssertionError|assert got == ("InvalidInputException", "Invalid next token"), (
Issue-Key: bedrock-agentcore.list-batch-evaluations-unissued-next-token-rejected|run#20|AssertionError|assert got == expected, (
Issue-Key: bedrock-agentcore.list-events-forged-next-token-rejected|run#24|AssertionError|assert got == expected, f"a nextToken ListEvents never issued gave {got!r}, not {expected!r}"
Issue-Key: bedrock-agentcore.list-events-unissued-next-token-rejected|run#21|AssertionError|assert got == expected, f"an unissued nextToken was refused with {got!r}, expected {expected!r}"
Issue-Key: bedrock-agentcore.list-memory-records-forged-next-token-rejected|run#14|AssertionError|assert code == "InvalidInputException", f"forged token answered {code!r}: {message!r}"
Issue-Key: bedrock-agentcore.list-memory-records-unissued-next-token-rejected|run#15|AssertionError|assert (code, message) == ("InvalidInputException", _FORGED_MESSAGE), (
Issue-Key: bedrock-agentcore.list-payment-sessions-unissued-next-token-rejected|run#17|AssertionError|assert (code, message) == ("ValidationException", _FORGED_MESSAGE), (
Issue-Key: bedrock-agentcore.list-sessions-forged-next-token-rejected|run#17|AssertionError|assert got == expected, f"a forged nextToken was refused as {got!r}, expected {expected!r}"
Issue-Key: bedrock-agentcore.list-sessions-unissued-next-token-rejected|run#22|AssertionError|assert got == ("InvalidInputException", "Invalid next token"), (
Issue-Key: bedrock-agentcore.retrieve-memory-records-unissued-next-token-rejected|run#17|AssertionError|assert (status, code, message) == (400, "InvalidInputException", _BAD_TOKEN_MESSAGE), (
Issue-Key: bedrock-agentcore.list-events-page-token-bound-to-its-metadata-filter-rejected|_refusal#17|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {len(answer['events'])} events")
Issue-Key: bedrock-agentcore.memory-arn-outside-endpoint-region-or-partition-rejected|_refusal#17|AssertionError|raise AssertionError(f"the request was expected to be refused but returned {outcome!r}")
Issue-Key: bedrock-agentcore.get-workload-access-token-deleted-identity-refused|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {sorted(result)!r}")
Issue-Key: bedrock-agentcore.workload-token-for-user-id-unknown-workload-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {sorted(answer)!r}")
Issue-Key: bedrock-agentcore.invoke-agent-runtime-agent-error-status-rejected|run#40|AssertionError|assert refused == expected, f"an agent's 422 should surface as {expected!r}, got {refused!r}"
Issue-Key: bedrock-agentcore.invoke-agent-runtime-nonexistent-not-found|run#17|AssertionError|assert contrast_code == "ValidationException", (
Issue-Key: bedrock-agentcore.invoke-code-interpreter-inactive-and-unknown-session-rejected|run#15|AssertionError|assert unknown == (
Issue-Key: bedrock-agentcore.invoke-code-interpreter-runtime-not-matching-language-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the stream answered {events!r}")
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore: 10 net-new memory/identity/runtime causes — BatchCreateMemoryRecords never refuses an entry, namespaces are neither normalized nor scoped, minted ids are out of form, and every pagination/ARN/token refusal is the wrong error

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateMemoryRecords

## Description
Ten net-new root causes in the bedrock-agentcore data plane. Each is numbered with its own failing checks.

Cause 1 of 10: BatchCreateMemoryRecords performs no per-entry validation — every entry lands in successfulRecords and failedRecords is always empty.
AWS: Smithy: BatchCreateMemoryRecords output has required members successfulRecords and failedRecords (both MemoryRecordsOutputList of MemoryRecordOutput); MemoryRecordOutput carries required status (MemoryRecordStatus: SUCCEEDED/FAILED), requestIdentifier, errorCode and errorMessage, i.e. the operation reports per-entry outcomes rather than refusing the batch. MemoryRecordCreateInput requires requestIdentifier, namespaces (NamespacesList, @length max 1), content and timestamp, and memoryStrategyId must name a strategy of the memory. The recording shows an entry naming an unknown memoryStrategyId, an entry with no namespace, and a second entry reusing a requestIdentifier already used in the same batch are each returned in failedRecords with status FAILED and are NOT stored, while the legal entries in the same batch are returned in successfulRecords; when every entry is bad the call still answers 200 with an empty successfulRecords list.
Emulator: every entry is accepted and stored: 'expected no successful entries, got 2', 'expected exactly 1 accepted entry, got 2' for a repeated requestIdentifier, "successfulRecords requestIdentifiers ['entry-kept', 'entry-refused'], expected ['entry-kept']", and "expected exactly kept-one and twice stored, got ['kept-one', 'no-namespace', 'twice', 'unknown-strategy']" — failedRecords stays [].
Why: a client cannot tell which records were written; it believes invalid records (unknown strategy, missing namespace, duplicate identifier) are durable, and the FAILED/errorCode/errorMessage half of the response contract never appears.
Fix: per-entry validation and failedRecords construction in the BatchCreateMemoryRecords handler and the memory-record state it writes.
Traces: bedrock-agentcore.batch-create-memory-records-all-entries-refused-still-answers, -duplicate-request-identifier-refused-once, -failed-entries-rejected-and-not-stored, -rejects-repeated-request-identifier-entry, -rejects-unknown-strategy-entry-beside-accepted-one, -unknown-strategy-entry-rejected-beside-stored-entry.

Cause 2 of 10: memory namespaces are stored verbatim and an unnormalized namespace is never refused.
AWS: Smithy: Namespace is @length 1..1024 @pattern ^[a-zA-Z0-9/*][a-zA-Z0-9-_/*]*(?::[a-zA-Z0-9-_/*]+)*[a-zA-Z0-9-_/*]*$. Recorded behavior: a namespace written bare ('kitchen/recipes') is stored normalized with one leading and one trailing '/' ('/kitchen/recipes/'), and a read or delete scoped by an unnormalized namespace is refused 400 ValidationException with the message "namespace '<value>' is not normalized." … " Use one leading and one trailing '/'." (fixed text; the quoted value is the namespace from the request).
Emulator: the record is stored under the string as sent — "the record written under 'kitchen/recipes' is stored with namespaces ['kitchen/recipes']" and "record stored under ['pottery/glazes'], expected the normalized namespace" — and an unnormalized namespace is accepted instead of refused.
Why: namespaces written one way and read another never match, so scoped reads/deletes silently miss records, and the client never learns its namespace was malformed.
Fix: namespace normalization plus the ValidationException in the memory-record write/read/delete validation path.
Traces: bedrock-agentcore.batch-create-memory-records-bare-namespace-stored-wrapped, bedrock-agentcore.delete-memory-record-unnormalized-namespace-rejected.

Cause 3 of 10: namespace scope is ignored on reads.
AWS: Smithy: GetMemoryRecord takes namespace as @httpQuery and models ResourceNotFoundException (404); ListMemoryRecords takes namespace and namespacePath and models ValidationException (400). Recorded behavior: GetMemoryRecord for a record stored under '/x/alpha/' answers 404 ResourceNotFoundException when read under a sibling ('/x/beta/') or the parent ('/x/') namespace, with the message 'The memory record <memoryRecordId> was not found' (fixed text around the id from the request); ListMemoryRecords naming neither namespace nor namespacePath is refused 400 ValidationException "At least one of 'namespace' or 'namespacePath' must be provided" (fixed text).
Emulator: both calls succeed with HTTP 200 — GetMemoryRecord returns the record under any namespace, and ListMemoryRecords with no scope returns memoryRecordSummaries.
Why: namespace isolation does not exist: an agent scoped to one namespace reads another's records, and a caller that forgot the required scope gets data instead of a refusal.
Fix: namespace matching in the GetMemoryRecord/ListMemoryRecords handlers and their input validation. Related but distinct from open issue 20261121T033538Z-bedrock-agentcore-3-net-new-causes-listmemoryrecords-ignores.md (that one is the bedrock-agentcore:namespace IAM condition key); this is the request's own namespace scope.
Traces: bedrock-agentcore.get-memory-record-other-namespace-rejected, bedrock-agentcore.list-memory-records-without-namespace-scope-rejected.

Cause 4 of 10: minted memory-record and code-interpreter task ids are not in AWS's form.
AWS: Smithy: MemoryRecordId is @length 40..50 @pattern ^mem-[a-zA-Z0-9-_]*$; beyond the model, the recording shows AWS mints 'mem-' followed by a lowercase UUID, and startCommandExecution returns a taskId that is a lowercase UUID.
Emulator: record ids are a zero-padded counter ('mem-000001a21d41bb52000000000000000000000001') — 'minted record ids not shaped mem-<uuid>' — and startCommandExecution issues a taskId shaped 'thsk-hhhhhhhhhhhhhhhhhhhhhhhhhhh'.
Why: clients and SDK-level validators that parse these ids (or round-trip them through systems expecting a UUID) reject the emulator's ids, and the counter leaks ordering/state.
Fix: the id minting used by the memory-record state and the code-interpreter task state.
Traces: bedrock-agentcore.batch-delete-memory-records-refuses-missing-ids-by-id, bedrock-agentcore.batch-update-memory-records-all-unknown-ids-refused-record-unchanged, bedrock-agentcore.code-interpreter-stop-task-on-finished-task-rejected.

Cause 5 of 10: every paginated operation refuses a bad nextToken with one emulator-authored error instead of its own modeled error and message.
AWS: Smithy: ListMemoryRecords, RetrieveMemoryRecords, ListSessions, ListActors and ListEvents each model InvalidInputException (400) alongside ValidationException (400); the recording fixes each operation's code and sentence: ListActors/ListSessions → InvalidInputException 'Invalid next token'; ListMemoryRecords/RetrieveMemoryRecords → InvalidInputException 'Provided next token is not valid'; ListEvents → ValidationException 'Invalid next token.'; ListBatchEvaluations → ValidationException 'The provided nextToken is invalid. Use the nextToken value returned by a previous request.'; ListPaymentSessions → ValidationException 'The request failed a data-layer precondition. Please check your input parameters.' All are fixed text with no request values.
Emulator: all of them answer 400 ValidationException with the single emulator-authored message 'nextToken is not a token this operation issued'.
Why: a client branching on the error code (InvalidInputException vs ValidationException) or matching the message mis-handles an expired or truncated page token on every list operation.
Fix: per-operation token-rejection errors in the pagination/token decoding shared by these list handlers.
Traces: bedrock-agentcore.list-actors-forged-next-token-rejected, bedrock-agentcore.list-actors-unissued-next-token-rejected, bedrock-agentcore.list-batch-evaluations-unissued-next-token-rejected, bedrock-agentcore.list-events-forged-next-token-rejected, bedrock-agentcore.list-events-unissued-next-token-rejected, bedrock-agentcore.list-memory-records-forged-next-token-rejected, bedrock-agentcore.list-memory-records-unissued-next-token-rejected, bedrock-agentcore.list-payment-sessions-unissued-next-token-rejected, bedrock-agentcore.list-sessions-forged-next-token-rejected, bedrock-agentcore.list-sessions-unissued-next-token-rejected, bedrock-agentcore.retrieve-memory-records-unissued-next-token-rejected.

Cause 6 of 10: a ListEvents nextToken is not bound to the filter that issued it.
AWS: Smithy: ListEvents is @paginated (inputToken nextToken) and models ValidationException (400); recorded behavior: a page token issued for one metadata filter, replayed with a different filter, is refused rather than served.
Emulator: the replayed token is accepted and the call answers 200 with 1 event.
Why: a client that changes its filter while paging silently reads a page computed for another filter, so results are wrong rather than refused.
Fix: bind the filter/criteria into the page token and verify it in the ListEvents handler.
Traces: bedrock-agentcore.list-events-page-token-bound-to-its-metadata-filter-rejected.

Cause 7 of 10: a memory ARN naming another Region or partition is accepted.
AWS: Smithy: memoryId accepts an ARN form ^(arn:(aws|aws-cn|aws-us-gov):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:memory/)?… and the operations model ValidationException (400). Recorded behavior: GetEvent/ListSessions given the same ARN rewritten to a foreign Region are refused ValidationException "Cross-region calls are not supported. Resource region '<region in the ARN>' does not match the target service region '<endpoint region>'.", and rewritten to a foreign partition are refused "Cross-partition calls are not supported. Resource partition '<partition in the ARN>' does not match the target service partition '<endpoint partition>'." (fixed text; the two quoted values come from the ARN and the endpoint).
Emulator: the foreign-Region/partition ARN is resolved and served with HTTP 200 (the event is returned).
Why: cross-Region/partition isolation is not enforced, so a client that mis-targets a Region gets data from the local one instead of the refusal AWS gives.
Fix: ARN parsing/validation in the memory data-plane handlers (shared memoryId resolution).
Traces: bedrock-agentcore.memory-arn-outside-endpoint-region-or-partition-rejected.

Cause 8 of 10: workload access tokens are minted for workload identities that do not exist.
AWS: Smithy: GetWorkloadAccessToken models AccessDeniedException (403). Recorded behavior: a never-registered or deleted workloadName is refused 403 AccessDeniedException 'Workload Identity does not belong to caller account' (fixed text), and the same call succeeds again once the identity is re-created; GetWorkloadAccessTokenForUserId behaves the same for an unknown workload.
Emulator: both calls succeed and return a workloadAccessToken ('the call was expected to be refused but returned [..., workloadAccessToken]').
Why: tokens are issued for identities the account does not have, so an authorization boundary that exists on AWS does not exist here.
Fix: existence/ownership check before minting in the GetWorkloadAccessToken and GetWorkloadAccessTokenForUserId handlers.
Traces: bedrock-agentcore.get-workload-access-token-deleted-identity-refused, bedrock-agentcore.workload-token-for-user-id-unknown-workload-rejected.

Cause 9 of 10: InvokeAgentRuntime maps the agent's own failure and a malformed ARN to the wrong error.
AWS: Smithy: InvokeAgentRuntime models RuntimeClientError (httpError 424) and ValidationException (400) as well as ResourceNotFoundException (404). Recorded behavior: when the agent container answers a 4xx of its own, the caller sees RuntimeClientError with 'Received error (<agent status>) from runtime. Please check your CloudWatch logs for more information.' (fixed text; the status in parentheses is the agent's response code); an agentRuntimeArn that is not a well-formed ARN is refused ValidationException, not ResourceNotFoundException.
Emulator: the agent's 422 surfaces as error code '422' with an empty message, and a malformed ARN answers ResourceNotFoundException.
Why: a client cannot distinguish 'the agent failed' from 'the service refused', and retry/branching on RuntimeClientError or ValidationException never fires.
Fix: error mapping in the InvokeAgentRuntime handler (agent-response translation) and ARN validation ordering ahead of lookup.
Traces: bedrock-agentcore.invoke-agent-runtime-agent-error-status-rejected, bedrock-agentcore.invoke-agent-runtime-nonexistent-not-found.

Cause 10 of 10: code-interpreter session refusals are reworded, and a runtime/language mismatch is answered as a tool result instead of a refusal.
AWS: Smithy: InvokeCodeInterpreter models ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: invoking a sessionId that was never started is refused ResourceNotFoundException 'Session not found for code interpreter: <codeInterpreterIdentifier>' (fixed text; the identifier comes from the request and the session id is NOT named), and executing code whose language does not match the session's runtime is refused as an API error rather than returned as stream content.
Emulator: the unknown session answers 'session <sessionId> not found for code interpreter <identifier>' (emulator wording, and it leaks the session id), and the runtime mismatch answers 200 with a stream item [{'result': {'content': [{'type': 'text', 'text': 'Error executing tool executeCode: the deno runtime does not run python; …'}], 'isError': True}}] instead of raising.
Why: a client matching the documented message or catching the modeled exception treats a refused execution as a successful call that returned text.
Fix: the refusal paths of the InvokeCodeInterpreter handler (message rendering and the language/runtime check). Related to open issue 20261102T113600Z-bedrock-agentcore-agentcore-not-found-refusals-diverge-wrong-status-wrong.md, which does not carry these two checks.
Traces: bedrock-agentcore.invoke-code-interpreter-inactive-and-unknown-session-rejected, bedrock-agentcore.invoke-code-interpreter-runtime-not-matching-language-rejected.

## Evidence
Cause 1 — expected failedRecords to carry the refused entries (status FAILED) and nothing to be stored; actual: `AssertionError: expected no successful entries, got 2`, `successfulRecords should hold only 'kept-entry'; got [... 'kept-entry', ... 'refused-entry']`, `expected one success per distinct identifier, got ['draft-a', 'draft-a', 'draft-b']`.
Cause 2 — expected namespaces ['/kitchen/recipes/']; actual stored ['kitchen/recipes']; an unnormalized namespace accepted instead of ValidationException "namespace 'sim2real/alpha' is not normalized. … Use one leading and one trailing '/'."
Cause 3 — expected 404 ResourceNotFoundException 'The memory record <id> was not found' / 400 ValidationException "At least one of 'namespace' or 'namespacePath' must be provided"; actual HTTP 200 with the record / with memoryRecordSummaries.
Cause 4 — expected mem-<uuid> and a lowercase-UUID taskId; actual 'mem-000001a21d41bb52000000000000000000000001' and 'thsk-hhhhhhhhhhhhhhhhhhhhhhhhhhh'.
Cause 5 — e.g. ListSessions expected ('InvalidInputException', 'Invalid next token'), ListEvents expected ('ValidationException', 'Invalid next token.'), ListBatchEvaluations expected ('ValidationException', 'The provided nextToken is invalid. Use the nextToken value returned by a previous request.'); actual every one of them ('ValidationException', 'nextToken is not a token this operation issued').
Cause 6 — expected a refusal of the filter-bound token; actual 200 with 1 event.
Cause 7 — expected ValidationException "Cross-region calls are not supported. Resource region '…' does not match the target service region '…'."; actual HTTP 200 returning the event.
Cause 8 — expected ('AccessDeniedException', 'Workload Identity does not belong to caller account'); actual a minted workloadAccessToken.
Cause 9 — expected ('RuntimeClientError', 'Received error (422) from runtime. Please check your CloudWatch logs for more information.'); actual ('422', ''). Malformed ARN: expected ValidationException, actual ResourceNotFoundException.
Cause 10 — expected ('ResourceNotFoundException', 'Session not found for code interpreter: aws.codeinterpreter.v1'); actual ('ResourceNotFoundException', 'session 7A93FDD0B53D8BCEE8177838D5 not found for code interpreter aws.codeinterpreter.v1'); language mismatch expected to raise, actual an isError stream item.

## Steps to Reproduce
Cause 1: CreateMemory, then BatchCreateMemoryRecords with four entries — one legal, one naming an unknown memoryStrategyId, one with no namespace, one reusing an earlier requestIdentifier. Observe all four in successfulRecords and failedRecords == [].
Cause 2: BatchCreateMemoryRecords with namespaces ['kitchen/recipes'], then GetMemoryRecord: the stored namespaces echo the bare string. DeleteMemoryRecord with namespace 'pottery/glazes' is accepted.
Cause 3: store a record under '/x/alpha/', then GetMemoryRecord with namespace='/x/beta/' (200 instead of 404) and ListMemoryRecords with neither namespace nor namespacePath (200 instead of ValidationException).
Cause 4: read memoryRecordId from BatchCreateMemoryRecords and taskId from startCommandExecution.
Cause 5: call each list operation with a nextToken it never issued and compare (code, message).
Cause 6: ListEvents with a metadata filter, take the nextToken, re-issue it with a different filter.
Cause 7: CreateMemory, then GetEvent/ListSessions with the memory ARN rewritten to another Region and another partition.
Cause 8: CreateWorkloadIdentity, GetWorkloadAccessToken (ok), DeleteWorkloadIdentity, GetWorkloadAccessToken again — still returns a token.
Cause 9: InvokeAgentRuntime against an agent that answers 422; and InvokeAgentRuntime with a malformed agentRuntimeArn.
Cause 10: InvokeCodeInterpreter with a never-started sessionId; and run python code in a deno-runtime session.
