Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.knowledge-base-document-blob-round-trip
Issue-Key: bedrock-agent.knowledge-base-document-blob-round-trip|_create_vector_index#2|Timeout|time.sleep(10)
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent: the OpenSearch Serverless collection endpoint answers the AWS-JSON front door (400 SerializationException 'missing required header x-amz-target'), so a vector knowledge base can never be built and the document round-trip never runs

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
ONE root cause, reached through a bedrock-agent check.

=== Cause 1 of 1: a VECTORSEARCH collection's published collectionEndpoint serves the AWS-JSON RPC dispatcher instead of the OpenSearch REST data plane, so the vector index a knowledge base requires can never be created ===
AWS: recorded behavior only for the data plane (the opensearchserverless Smithy model covers the control plane; a collection's data plane is the OpenSearch REST API and is not modeled). Smithy consulted for the Bedrock side: bedrock-agent CreateKnowledgeBase takes a storageConfiguration of type OPENSEARCH_SERVERLESS carrying collectionArn + vectorIndexName + fieldMapping, i.e. the index must already exist in the collection before the knowledge base is created. Recorded behavior: once CreateCollection(type='VECTORSEARCH') is ACTIVE, BatchGetCollection publishes a collectionEndpoint, and a SigV4-signed (service name `aoss`) `PUT https://<collectionEndpoint>/<indexName>` with a knn_vector mapping — the exact request opensearch-py's `indices.create` issues, which carries NO `x-amz-target` header because the OpenSearch REST API has no RPC target — is accepted and creates the index.
Emulator: that request is answered HTTP 400 with body `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}`. The published collectionEndpoint is wired to the emulator's AWS-JSON protocol front door, which requires an `X-Amz-Target` header, so no OpenSearch REST verb can be served on a collection endpoint at all.
Why: the whole bedrock-agent knowledge-base pipeline is blocked at its first step. The check cannot create the index, so CreateKnowledgeBase / CreateDataSource(CUSTOM) / IngestKnowledgeBaseDocuments / GetKnowledgeBaseDocuments / Retrieve are never reached, and the byte-identity guarantee for an inline BYTE document (multi-byte UTF-8 preserved end to end, tracked under the caller's customDocumentIdentifier, reaching INDEXED) is unverifiable. Because the check's `_create_vector_index` helper retries the failing PUT 12 times at 10s intervals, the emulator's 400 is never surfaced as the verdict: the check is reported as 'Check timed out after 60.0s', which is this failure inside the retry loop.
Fix: the opensearchserverless collection data plane — route the published collectionEndpoint host to an OpenSearch REST handler (index create/get/delete, document index/search) that authorises SigV4 `aoss` requests against the collection's data-access policy, instead of letting the host fall through to the AWS-JSON `x-amz-target` dispatcher (services/service-com-amazonaws-opensearchserverless-opensearchserverless, plus the emulator's host/endpoint routing table that resolves collection endpoints). The identical wall is reported for bedrock-agent-runtime and for the bedrock-agent__lambda__opensearchserverless cross-service checks under their own service tokens.
Traces: bedrock-agent.knowledge-base-document-blob-round-trip.

## Evidence
Expected: `PUT https://<collectionEndpoint>/<indexName>` (SigV4 service `aoss`, body {'settings':{'index.knn':true},'mappings':{'properties':{'bedrock-knowledge-base-default-vector':{'type':'knn_vector','dimension':1024,...},'AMAZON_BEDROCK_TEXT_CHUNK':{'type':'text'},'AMAZON_BEDROCK_METADATA':{'type':'text','index':false}}}}) accepted, then the ingest/retrieve round trip returns the byte-identical UTF-8 marker.
Actual: the endpoint answers HTTP 400 `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}` to every attempt (the same body that the sibling bedrock-agent-runtime check surfaces directly as opensearchpy.exceptions.RequestError(400, ...)). Because the helper retries 12 times at 10s, the suite reports this check as 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1) opensearchserverless: create_security_policy(name=<c>-enc, type='encryption'), create_security_policy(name=<c>-net, type='network', policy AllowFromPublic=true), create_access_policy(name=<c>-data, type='data', granting aoss:* on collection/<c> and index/<c>/*), create_collection(name=<c>, type='VECTORSEARCH'). 2) Poll batch_get_collection(ids=[<id>]) until status=='ACTIVE' and read collectionDetails[0].collectionEndpoint. 3) SigV4-sign with service name 'aoss' and send `PUT https://<collectionEndpoint>/<indexName>` with the knn_vector mapping body above (e.g. via opensearch-py OpenSearch(...).indices.create(index=<indexName>, body=<body>)). The emulator answers 400 SerializationException 'missing required header `x-amz-target`' instead of creating the index, so the follow-on bedrock-agent create_knowledge_base(storageConfiguration={'type':'OPENSEARCH_SERVERLESS', 'opensearchServerlessConfiguration':{'collectionArn':...,'vectorIndexName':<indexName>,'fieldMapping':{...}}}) / create_data_source(CUSTOM) / ingest_knowledge_base_documents round trip can never be started.
