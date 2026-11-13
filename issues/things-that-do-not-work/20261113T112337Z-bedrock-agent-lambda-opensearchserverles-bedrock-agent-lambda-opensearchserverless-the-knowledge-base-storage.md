Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative
Issue-Key: bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative|_poll_until#7|Timeout|return last
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# bedrock-agent__lambda__opensearchserverless: the knowledge-base storage-configuration refusal never converges — CreateKnowledgeBase without the role in the AOSS data access policy reaches no verdict inside the budget

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
Cause 1 of 1: the negative leg that must see CreateKnowledgeBase refuse a storage configuration whose OpenSearch Serverless data access policy omits the knowledge-base role never reaches an answer — the check exhausts the harness budget. Failing check: bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative.

AWS: recorded behavior only for the end-to-end wiring; the modelled surface is bedrock-agent CreateKnowledgeBase, whose errors include ValidationException (HTTP 400). On AWS, creating a knowledge base whose storageConfiguration points at an AOSS collection whose data access policy does not grant the knowledge-base role is refused synchronously with ValidationException naming the storage configuration, while the same setup that does grant the role creates the knowledge base, lets StartIngestionJob complete and lets Retrieve return the ingested document. Every step on the way — CreateVpcEndpoint/BatchGetVpcEndpoint reaching ACTIVE, CreateCollection reaching ACTIVE, CreateKnowledgeBase answering — converges, so the check reaches its assertion.

Emulator: 'Check timed out after 60.0s'. Neither the refusal nor the positive control is produced: no ValidationException, no knowledge base, no ingestion result. The scenario stalls on one of the resources it waits for (the AOSS VPC endpoint/collection reaching ACTIVE, or CreateKnowledgeBase itself never answering), so nothing is gradeable.

Why: a caller gets a hang where AWS gives either an immediate, actionable ValidationException or a working knowledge base. Infrastructure code that provisions a Bedrock knowledge base over OpenSearch Serverless cannot tell a misconfigured data access policy from a slow create, and the guard that should stop a bad deploy never fires.

Fix: the bedrock-agent CreateKnowledgeBase handler's validation of storageConfiguration against the referenced OpenSearch Serverless collection's data access policy (it must answer ValidationException synchronously), plus whichever waited-on state in the opensearchserverless path never converges (VPC endpoint / collection reaching ACTIVE) so the positive control can proceed.

Traces: the failing check id above.

## Evidence
deploy-stack-missing-opensearchserverless-data-access-negative — Error: Check timed out after 60.0s.

Expected (AWS): CreateKnowledgeBase is refused with ValidationException (HTTP 400) naming the storage configuration when the knowledge-base role is absent from the collection's AOSS data access policy; the granting control creates the knowledge base, StartIngestionJob completes and Retrieve returns the ingested document. Actual: no response of any kind inside the budget — the check never reaches its assertion.

## Steps to Reproduce
1. Create an OpenSearch Serverless VPC endpoint in private subnets and wait for BatchGetVpcEndpoint to report ACTIVE; create two collections with network policies admitting only that endpoint.
2. For the negative leg create an encryption/network policy pair but a data access policy that does NOT name the knowledge-base role, then call bedrock-agent CreateKnowledgeBase with a storageConfiguration pointing at that collection.
3. Observe that no ValidationException arrives (and the positive control never ingests); the check hits its 60 s cap.
