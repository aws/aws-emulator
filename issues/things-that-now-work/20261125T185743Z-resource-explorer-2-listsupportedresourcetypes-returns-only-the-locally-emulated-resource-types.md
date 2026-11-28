Issue-Source: trace-sweep-divergence
Issue-Trace: resource-explorer-2-catalog-list-supported-resource-types
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-explorer-2-catalog-list-supported-resource-types
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|ListSupportedResourceTypes|2xx->2xx|$.ResourceTypes[*]
Issue-Commit: 8bf2bd5c40cd5968f6fa936e1c0608724e59a2f4

# ListSupportedResourceTypes returns only the locally emulated resource types instead of Resource Explorer's full supported-type catalog

> _Note: this issue was identified against the code base at commit `8bf2bd5c40cd5968f6fa936e1c0608724e59a2f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSupportedResourceTypes

## Description
Cause 1 of 1: ListSupportedResourceTypes publishes a catalog derived from the emulator's own implemented services rather than the service-wide catalog AWS Resource Explorer publishes, so the response body is a 4-element list where AWS returns the full type catalog.

AWS: Smithy: ListSupportedResourceTypes (POST /ListSupportedResourceTypes, success 200, paginated inputToken=NextToken/outputToken=NextToken/pageSize=MaxResults/items=ResourceTypes; MaxResults @range min 1 max 1000; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The model documents ResourceTypes as "The list of resource types supported by Resource Explorer" and the operation as "Retrieves a list of all resource types currently supported by Amazon Web Services Resource Explorer" — i.e. a service-wide CATALOG of every type an index can hold, independent of what the account or the emulator actually implements. Each SupportedResourceType carries Service, ResourceType ("<service>:<type>" form) and CFNResourceTypes (the CloudFormation type identifiers). Recorded behavior shows the catalog is large (the fixture's expected ResourceTypes runs to index 653, i.e. ~654 entries) and is ordered ascending by ResourceType: access-analyzer:analyzer (AWS::AccessAnalyzer::Analyzer), acm-pca:certificate-authority (AWS::ACMPCA::CertificateAuthority), acm:certificate (AWS::CertificateManager::Certificate), airflow:environment (AWS::MWAA::Environment), amplify:apps / amplify:apps/branches / amplify:apps/domains, aoss:collection, apigateway:apis, apigateway:apis/integrations, ... — the catalog spans services the emulator does not emulate at all.

Emulator: status 200 (matching) but the body carries only four entries, in an order unrelated to AWS's: ResourceTypes[0] = {Service: resource-explorer-2, ResourceType: resource-explorer-2:index, CFNResourceTypes: [AWS::ResourceExplorer2::Index]}, [1] = resource-explorer-2:view / AWS::ResourceExplorer2::View, [2] = sns:topic / AWS::SNS::Topic, [3] = sqs:queue / AWS::SQS::Queue, and every index from 4 upward is absent. The replay reports a missing/mismatched element at essentially every expected index ($.ResourceTypes[0] through $.ResourceTypes[653]). The handler builds the response by mapping crate::state::owners::SUPPORTED_TYPES, an emulator-internal set of the types its own index can hold, so the published catalog shrinks to whatever this emulator implements rather than what Resource Explorer supports.

Why: a client uses this operation to discover which resource types it may name — in a ListResources/Search filter ("resourcetype:..."), in a view's Filters, or to validate user input before a query. Against the emulator such a client concludes that Resource Explorer supports four types, so it rejects or omits every legitimate type (ec2:instance, s3:bucket, lambda:function, iam:role, ...) that AWS would accept, and any pagination logic driven by this catalog terminates on the first page where AWS yields many. The ordering difference additionally breaks clients that binary-search or diff the catalog, since AWS returns it sorted by ResourceType.

Fix: the ListSupportedResourceTypes handler's catalog data source — it must serve Resource Explorer's full, fixed supported-type catalog (Service / ResourceType / CFNResourceTypes per entry, ordered ascending by ResourceType) as static service data, decoupled from the emulator's own implemented-service set, and then page that catalog through the existing MaxResults/NextToken pagination. Reusing the index-population set as the published catalog is the defect; the catalog should be the superset and the index may hold a subset of it.

Traces: resource-explorer-2-catalog-list-supported-resource-types#task#1 (expected 200 with the full catalog, got 200 with 4 entries). Related but NOT covering: open issue "ListResources accepts a malformed Filters.FilterString (bare \"service:\") and returns 200 instead of ValidationException" (20261118T024816Z-resource-explorer-2-listresources-accepts-a-malformed-filters-filterstring-bare-service.md) concerns ListResources filter validation, not this operation's catalog contents.

## Evidence
Fixture resource-explorer-2-catalog-list-supported-resource-types, step task#1, operation ListSupportedResourceTypes, verdict mismatch.

Expected status 200, actual status 200 — the divergence is wholly in the response body.

Expected (recorded AWS) ResourceTypes, ordered ascending by ResourceType, ~654 entries:
  [0] {"Service":"access-analyzer","ResourceType":"access-analyzer:analyzer","CFNResourceTypes":["AWS::AccessAnalyzer::Analyzer"]}
  [1] {"Service":"acm-pca","ResourceType":"acm-pca:certificate-authority","CFNResourceTypes":["AWS::ACMPCA::CertificateAuthority"]}
  [2] {"Service":"acm","ResourceType":"acm:certificate","CFNResourceTypes":["AWS::CertificateManager::Certificate"]}
  [3] {"Service":"airflow","ResourceType":"airflow:environment","CFNResourceTypes":["AWS::MWAA::Environment"]}
  [4] {"Service":"amplify","ResourceType":"amplify:apps","CFNResourceTypes":["AWS::Amplify::App"]}
  [5] amplify:apps/branches, [6] amplify:apps/domains, [7] aoss:collection, [8] apigateway:apis, [9] apigateway:apis/integrations, ... through [653]

Actual (emulator) ResourceTypes, 4 entries:
  [0] {"Service":"resource-explorer-2","ResourceType":"resource-explorer-2:index","CFNResourceTypes":["AWS::ResourceExplorer2::Index"]}
  [1] {"Service":"resource-explorer-2","ResourceType":"resource-explorer-2:view","CFNResourceTypes":["AWS::ResourceExplorer2::View"]}
  [2] {"Service":"sns","ResourceType":"sns:topic","CFNResourceTypes":["AWS::SNS::Topic"]}
  [3] {"Service":"sqs","ResourceType":"sqs:queue","CFNResourceTypes":["AWS::SQS::Queue"]}
  [4..653] absent (replay: "got None")

Replay divergence key: service=resource-explorer-2|ListSupportedResourceTypes|status=200->200|keys=$.ResourceTypes[0]...$.ResourceTypes[653].

Source of the emulator behavior: services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/list_supported_resource_types.rs lines 164-178 map crate::state::owners::SUPPORTED_TYPES (the emulator's own index-population set) into the response, with the module doc at lines 4-6 stating the catalog is "the one catalogue ... that the index itself is built from". Pagination at lines 180-191 then pages that short list.

No value in the requirement is environment-specific: the catalog entries are fixed service metadata (Service, ResourceType, CFNResourceTypes), not generated ids, hosts or timestamps.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a Resource Explorer client at it.
2. Call ListSupportedResourceTypes with no parameters (POST /ListSupportedResourceTypes, empty JSON body) — or replay fixture resource-explorer-2-catalog-list-supported-resource-types, step task#1.
3. Observe HTTP 200 whose ResourceTypes array holds exactly 4 entries (resource-explorer-2:index, resource-explorer-2:view, sns:topic, sqs:queue) and no NextToken, instead of Resource Explorer's full supported-type catalog (~654 entries) ordered ascending by ResourceType beginning access-analyzer:analyzer, acm-pca:certificate-authority, acm:certificate, airflow:environment, amplify:apps.
4. Call it again with MaxResults=100: AWS returns a 100-entry page plus a NextToken; the emulator still returns the same 4 entries and no NextToken, so a paginating client stops after one page.
