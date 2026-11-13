Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative|run#35|Timeout|count_after = search_client.count(
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# bedrock-agent__lambda__opensearchserverless: CreateKnowledgeBase never answers for an AOSS collection whose data access policy omits the knowledge-base role — the negative leg reaches no verdict inside the budget

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
Cause 1 of 1: the scenario that must see bedrock-agent CreateKnowledgeBase refuse a storageConfiguration whose OpenSearch Serverless data access policy grants the harness identity but NOT the knowledge-base role never reaches an answer — the check exhausts the harness budget with no response of any kind. Failing check: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative.

NOTE: open issue 20261113T112337Z-bedrock-agent-lambda-opensearchserverles-bedrock-agent-lambda-opensearchserverless-the-knowledge-base-storage.md describes the same family of stall, but its Issue-Key/Issue-Trace-Ids carry only the check bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative, so it does not carry the replay key of the check filed here. Also related (AOSS endpoint reachability/data-plane enforcement, different causes): 20261109T154357Z-bedrock-agent-lambda-opensearchserverles-...-the-published-aoss-collectionendpoint.md and 20261108T223024Z-opensearchserverless-3-net-new-causes-on-the-collection.md.

AWS: recorded behavior only for the cross-service wiring; the modelled surface is bedrock-agent CreateKnowledgeBase (Smithy: PUT /knowledgebases/, success 202; modeled errors AccessDeniedException 403, ConflictException 409, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400, InternalServerException 500; roleArn and knowledgeBaseConfiguration required, storageConfiguration optional). On AWS, every step the scenario waits on converges and the call is answered synchronously: CreateVpcEndpoint/BatchGetVpcEndpoint reaches ACTIVE, CreateCollection reaches ACTIVE with a reachable collectionEndpoint, the harness's index create and HEAD succeed, and CreateKnowledgeBase then answers — refusing with HTTP 400 ValidationException whose message is the fixed text "The knowledge base storage configuration provided is invalid... Dependency error document status code: 403, error message: Bad Authorization" (fixed text throughout for this error class; the only variable parts AWS substitutes in this family are the dependency status code and dependency message it relays from the vector store). Once the data access policy is updated to add the knowledge-base role principal, the identical request is accepted (202) and the knowledge base reaches ACTIVE, a StartIngestionJob run reaches COMPLETE and the written documents become searchable in the index.

Emulator: 'Check timed out after 60.0s'. Neither leg produces anything gradeable — no 400 ValidationException for the ungranted role, and no accepted knowledge base for the granted retry. The scenario stalls on one of the states it polls (the AOSS VPC endpoint or collection reaching ACTIVE, the newly created index becoming visible on the published collectionEndpoint, or CreateKnowledgeBase itself never returning), so the request never reaches a verdict at all.

Why: a client gets an indefinite hang where AWS gives either an immediate, actionable ValidationException identifying the bad storage configuration or a working knowledge base. Provisioning code cannot distinguish a missing AOSS data access grant from a slow create, the guard that should stop a misconfigured deploy never fires, and the follow-on ingestion/search path is unreachable, so no permission-boundary behavior of this path can be exercised at all.

Fix: the bedrock-agent CreateKnowledgeBase handler — it must answer synchronously, validating storageConfiguration against the referenced OpenSearch Serverless collection's data access policy and returning ValidationException (400) with the storage-configuration refusal message when the roleArn principal has no grant — together with whichever waited-on opensearchserverless state on this path never converges (VPC endpoint / collection reaching ACTIVE, and the published collectionEndpoint serving the index HEAD) so the granted positive control can proceed to ACTIVE and ingestion.

Traces: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative.

## Evidence
Check bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-data-access-policy-missing-negative — verdict [mismatch], Error: 'Check timed out after 60.0s'.

Expected (AWS): with a data access policy naming only the caller identity, CreateKnowledgeBase answers HTTP 400 ValidationException, Message = "The knowledge base storage configuration provided is invalid... Dependency error document status code: 403, error message: Bad Authorization" (fixed text for this error class; the dependency status code and dependency message are the relayed values). After UpdateAccessPolicy adds the knowledge-base role principal, the same CreateKnowledgeBase is accepted (202), the knowledge base reaches status ACTIVE, StartIngestionJob reaches status COMPLETE and a match_all count on the index returns >= 1.

Actual (emulator): no response of either kind within the 60 s budget — the check never reaches its first assertion, so neither the refusal code/message nor the accepted-path states are produced.

## Steps to Reproduce
1. Create an IAM role trusted by bedrock.amazonaws.com with aoss:APIAccessAll, bedrock:InvokeModel and s3 read permissions (the knowledge-base role).
2. Create an OpenSearch Serverless encryption policy for a collection name, create an AOSS VPC endpoint in the run's private subnets and poll BatchGetVpcEndpoint until ACTIVE, and create a network policy admitting only that endpoint plus bedrock.amazonaws.com.
3. Create a data access policy whose Principal list contains ONLY the caller's own identity ARN (deliberately omitting the knowledge-base role).
4. Create a VECTORSEARCH collection, wait for ACTIVE, and through the published collectionEndpoint create a vector index (knn vector field, text and metadata fields) and wait until it is visible.
5. Call bedrock-agent CreateKnowledgeBase with roleArn = the knowledge-base role and a storageConfiguration of type OPENSEARCH_SERVERLESS pointing at that collection ARN and index. Observe that no answer arrives: no 400 ValidationException with the storage-configuration refusal message, and no accepted knowledge base; the check hits its 60 s cap.
6. Positive control: UpdateAccessPolicy to add the knowledge-base role to the data access policy Principal list and repeat step 5 — the create must be accepted, reach ACTIVE, and a StartIngestionJob over an S3 data source must reach COMPLETE with documents searchable in the index.
