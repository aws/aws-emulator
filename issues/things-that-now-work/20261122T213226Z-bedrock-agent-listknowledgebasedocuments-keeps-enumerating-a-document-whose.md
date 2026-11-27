Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable|run#15|Timeout|time.sleep(5)
Issue-Commit: 95ac6b97296b25f638775ef6173e027be717227c

# bedrock-agent ListKnowledgeBaseDocuments keeps enumerating a document whose deletion has completed, so a deleted document is never observably gone

> _Note: this issue was identified against the code base at commit `95ac6b97296b25f638775ef6173e027be717227c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKnowledgeBaseDocuments

## Description
Cause 1 of 1: a document removed by DeleteKnowledgeBaseDocuments stays in the ListKnowledgeBaseDocuments page for the rest of the data source's life, so no read of the collection ever shows the delete took effect.

Failing check ids for this cause: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable

AWS: ListKnowledgeBaseDocuments is modelled as "Retrieves all the documents contained in a data source that is connected to a knowledge base" (Smithy: ListKnowledgeBaseDocuments, GET-style readonly POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents, output documentDetails required). DeleteKnowledgeBaseDocuments (Smithy: HTTP 202, @idempotent, documentIdentifiers @length 1..10) is accepted asynchronously, and the document it names is on its way out of the data source: the modelled KnowledgeBaseDocumentDetail$status chain for a delete is "DELETING - You submitted the delete job containing the document" then "DELETE_IN_PROGRESS - The document is being deleted". Once the delete has run, the data source no longer CONTAINS that document, so it is not among the documents the listing retrieves; the recorded behaviour the check was taken from is exactly this - after the delete, ListKnowledgeBaseDocuments converges to the surviving document alone, and only a targeted GetKnowledgeBaseDocuments on the dead identifier still answers (absent, or NOT_FOUND / DELETING / DELETE_IN_PROGRESS). The model says nothing about retaining a tombstone in the collection listing; "NOT_FOUND - The document wasn't found" is the status a targeted read of an unknown identifier reports, not a member of the data source's contents.

Emulator: the delete is accepted and the document's status chain does advance (DELETING -> DELETE_IN_PROGRESS -> NOT_FOUND), but the record is deliberately kept in the collection and the listing reports every record whatever its status, so the deleted identifier is returned by ListKnowledgeBaseDocuments for ever (bounded only by the emulator's own DELETED_DOCUMENT_RECORDS_KEPT history cap, which equals the per-data-source document quota and is therefore never reached by a client that deleted one document). The check's independent read loop - poll ListKnowledgeBaseDocuments until the deleted identifier disappears - therefore never converges; the deleted document and its surviving twin are both still listed, and the check is killed by the harness: "Error: Check timed out after 60.0s". The assertions that would have fired are the collection ones ("document <id> was still listed 180s after DeleteKnowledgeBaseDocuments" / "the delete must remove ONLY the document it named: expected the listing to be exactly [<survivor>]"), not the targeted-read ones, which the emulator already answers acceptably.

Why: a client cannot confirm a deletion through the operation AWS gives it for that. Any workflow that deletes a document and then waits for the data source listing to settle (a reconciler that re-syncs a data source, a CI step that asserts a purge, an index-size/quota accounting pass that counts listed documents) blocks until its own timeout against the emulator while succeeding against AWS, and code that recomputes a data source's contents from the listing keeps feeding deleted document ids back into downstream calls. Because the retained tombstones also still occupy the listing, paging a data source returns rows AWS would not return at all.

Fix: the listing projection for this operation's state - the page builder that ListKnowledgeBaseDocuments' handler calls must enumerate only the documents the data source still HOLDS (the same is_held predicate the quota accounting and the re-ingestion path already use), so a completed deletion leaves the collection; the deletion's own outcome stays reportable through the targeted GetKnowledgeBaseDocuments read and through the DELETING / DELETE_IN_PROGRESS statuses the listing shows while the delete is still in flight. No open issue covers this: the filed bedrock-agent items concern VECTOR knowledge-base and prompt refusal wording, prompt lifecycle state, ListIngestionJobs and unimplemented stubs.

Traces: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable. Supporting file citations in the checkout: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/state.rs lines 3670-3710 (page_of_documents reports "every document the data source has a record for ... whatever its status", including the terminal NOT_FOUND one), lines 2391-2402 and 2500-2508 (the retained-tombstone decision and DELETED_DOCUMENT_RECORDS_KEPT), lines 3418-3444 (the document status chain and forget_oldest_deleted_documents).

## Evidence
Expected (AWS, as recorded by the check): after DeleteKnowledgeBaseDocuments(knowledgeBaseId, dataSourceId, documentIdentifiers=[{dataSourceType: CUSTOM, custom:{id: <doomed>}}]) is accepted with HTTP 202, ListKnowledgeBaseDocuments on that data source converges to exactly the identifiers the data source still contains - {<survivor>} - while GetKnowledgeBaseDocuments on <doomed> answers either nothing for it or a detail whose status is one of DELETING / DELETE_IN_PROGRESS / NOT_FOUND; a repeat delete of the dead identifier is accepted and changes nothing; re-ingesting <doomed> reaches INDEXED and the listing returns to {<doomed>, <survivor>}.

Actual (emulator): ListKnowledgeBaseDocuments keeps returning BOTH identifiers after the delete completes - the deleted one is reported with status NOT_FOUND instead of being absent from the page - so the check's convergence loop (ListKnowledgeBaseDocuments every 5s for up to 180s, waiting for <doomed> to leave the listing) never breaks and the check dies on the harness cap: "[mismatch] bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable / Error: Check timed out after 60.0s". Status codes and error codes are not at issue here: every call in the sequence succeeds; the divergence is the content of the 200 listing page (a documentDetails entry AWS does not return).

Message/template note: the identifiers in the evidence above (<doomed>, <survivor>) and the knowledge-base/data-source ids are per-run values supplied by the request, not part of any requirement; the requirement is that a completed deletion's identifier is absent from documentDetails.

## Steps to Reproduce
1. Create an S3 Vectors vector bucket and index, an IAM role bedrock.amazonaws.com can assume, then CreateKnowledgeBase with knowledgeBaseConfiguration {type: VECTOR, vectorKnowledgeBaseConfiguration.embeddingModelArn = amazon.titan-embed-text-v2:0} and storageConfiguration {type: S3_VECTORS, s3VectorsConfiguration: {vectorBucketArn, indexArn}}; wait for the knowledge base to report ACTIVE.
2. CreateDataSource on it with dataSourceConfiguration {type: CUSTOM} and dataDeletionPolicy RETAIN; wait for AVAILABLE.
3. IngestKnowledgeBaseDocuments twice, once per in-line CUSTOM/BYTE document, with customDocumentIdentifier ids A (to be deleted) and B (control). Poll GetKnowledgeBaseDocuments until both report INDEXED.
4. ListKnowledgeBaseDocuments: both A and B are listed (correct).
5. DeleteKnowledgeBaseDocuments with documentIdentifiers=[{dataSourceType: CUSTOM, custom: {id: A}}].
6. Poll ListKnowledgeBaseDocuments. AWS stops listing A within seconds and reports only B. The emulator keeps listing A indefinitely (status NOT_FOUND), so a loop waiting for A to disappear never terminates.
