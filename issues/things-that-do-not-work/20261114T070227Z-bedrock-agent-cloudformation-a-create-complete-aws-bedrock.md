Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle
Issue-Key: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle|run#15|AssertionError|assert len(listed_ds) == 1, f"expected one data source named {names['ds']!r}, got {listed_ds!r}"
Issue-Commit: 2ed76ae7be9a75788628a2bcb5631d9ad9216ae7

# bedrock-agent__cloudformation: a CREATE_COMPLETE AWS::Bedrock::DataSource is invisible to bedrock-agent — ListDataSources on its knowledge base returns no such data source

> _Note: this issue was identified against the code base at commit `2ed76ae7be9a75788628a2bcb5631d9ad9216ae7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataSources

## Description
Cause 1 of 1: a data source provisioned by a CloudFormation AWS::Bedrock::DataSource resource is never registered in bedrock-agent's own state, so after the stack reaches CREATE_COMPLETE (and exports Ref/GetAtt for the resource) bedrock-agent's ListDataSources for that knowledge base reports no data source with the template's Name. Failing check: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle.

AWS: Smithy (service bedrock-agent, com.amazonaws.bedrockagent): CreateDataSource (PUT /knowledgebases/{knowledgeBaseId}/datasources/) takes a required name with pattern ^([0-9a-zA-Z][_-]?){1,100}$ and returns the created dataSource; ListDataSources (POST /knowledgebases/{knowledgeBaseId}/datasources/, paginated on dataSourceSummaries, which is a required output member) answers 200 with one summary per data source of that knowledge base, each carrying name and dataSourceId. CloudFormation provisions AWS::Bedrock::DataSource by calling that API, so a stack that creates a KnowledgeBase plus a DataSource and reaches CREATE_COMPLETE leaves exactly one data source visible through the service API: ListDataSources(knowledgeBaseId=Ref(Kb)) contains a summary whose name equals Properties.Name and whose dataSourceId equals GetAtt <Ds>.DataSourceId, and GetDataSource on that id answers 200 with status AVAILABLE. The resource's PhysicalResourceId (Ref) is '<knowledgeBaseId>|<dataSourceId>'.

Emulator: the stack reaches CREATE_COMPLETE and the KnowledgeBase it created IS visible (ListKnowledgeBases returns it with id Ref(Kb)), but ListDataSources(knowledgeBaseId=Ref(Kb)) contains no summary whose name is the template's Properties.Name — the filtered result is the empty list, so nothing the CFN resource created exists in bedrock-agent's data-source state for that knowledge base. The subsequent reads the check never reaches (GetAtt DataSourceId identity, Ref = '<kbId>|<dsId>', GetDataSource status AVAILABLE, and the update-stack removal proving the data source gone) are all unverifiable while the data source is absent.

Why: a client that deploys a knowledge base and its data source with CloudFormation and then drives the service API gets a green stack with no addressable data source: discovery by ListDataSources finds nothing, so ingestion can never be started or monitored for the deployed resource, and any pipeline that reconciles stack contents against bedrock-agent concludes the data source was deleted out-of-band.

Fix: the AWS::Bedrock::DataSource resource-type handler's create path in the CloudFormation resource provisioning — it must call through to bedrock-agent CreateDataSource against the resolved KnowledgeBaseId and record the returned data source in bedrock-agent state (so ListDataSources/GetDataSource see it), rather than materialising the resource only inside the stack's own resource table.

Related but distinct from the two open issues on this resource type: 20261109T203511Z-bedrock-agent-a-cloudformation-aws-bedrock-datasource-hands.md is about the VALUE GetAtt DataSourceId hands back (the template Name instead of a 10-character id), and 20261109T203511Z-bedrock-agent-cloudformation-s3-an-aws-bedrock-datasource.md is about an unknown KnowledgeBaseId not being refused. Neither covers the data source being absent from bedrock-agent's listings after a successful create; this check is not marked owned by either.

Traces: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle.

## Evidence
Failing check bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle [mismatch]:

  AssertionError: expected one data source named 'sim2real-kbcfn-ds-name-0325c73eb5ca', got []

Sequence up to the failure: CreateStack with AWS::IAM::Role + AWS::S3Vectors::VectorBucket + AWS::S3Vectors::Index + AWS::S3::Bucket + AWS::Bedrock::KnowledgeBase (Type VECTOR, StorageConfiguration S3_VECTORS) + AWS::Bedrock::DataSource (KnowledgeBaseId = Ref(Kb), Name = <per-run name>, DataSourceConfiguration Type S3 on GetAtt DsBucket.Arn) reached CREATE_COMPLETE; the knowledge-base assertion passed (ListKnowledgeBases returned exactly the stack's KB with id Ref(Kb)); then the paginated ListDataSources(knowledgeBaseId=Ref(Kb)) yielded no dataSourceSummaries entry with name = Properties.Name.

Expected: exactly one summary with that name, whose dataSourceId equals the stack Output GetAtt <Ds>.DataSourceId, Ref(Ds) = '<knowledgeBaseId>|<dataSourceId>', and GetDataSource on it answering 200 with status 'AVAILABLE' and dataSourceConfiguration {type: S3, s3Configuration: {bucketArn: <DsBucket Arn>}}. The name 'sim2real-kbcfn-ds-name-0325c73eb5ca' and the ids are per-run values; the requirement is that the data source CloudFormation reported created be listed and readable through bedrock-agent.

## Steps to Reproduce
1. CreateStack (Capabilities CAPABILITY_IAM) with a template containing an AWS::IAM::Role, an AWS::S3Vectors::VectorBucket and Index, an AWS::S3::Bucket, an AWS::Bedrock::KnowledgeBase (KnowledgeBaseConfiguration Type VECTOR with an EmbeddingModelArn; StorageConfiguration Type S3_VECTORS referencing the vector bucket/index), and an AWS::Bedrock::DataSource with KnowledgeBaseId = {"Ref": "Kb"}, Name = <a name matching ^([0-9a-zA-Z][_-]?){1,100}$>, DataSourceConfiguration Type S3 with S3Configuration.BucketArn = GetAtt of the bucket. Export Outputs KbRef = Ref(Kb), DsRef = Ref(Ds), DsId = GetAtt Ds.DataSourceId.
2. Poll DescribeStacks(StackName=<StackId>) until CREATE_COMPLETE; read the Outputs.
3. Call bedrock-agent ListDataSources(knowledgeBaseId=<KbRef>) (all pages) and filter dataSourceSummaries by name == the template's Name.
Observed: no matching summary (empty list) although the stack is CREATE_COMPLETE.
Expected: exactly one summary, with dataSourceId == the DsId output, and GetDataSource(knowledgeBaseId=<KbRef>, dataSourceId=<DsId>) answering 200 with status 'AVAILABLE'.
