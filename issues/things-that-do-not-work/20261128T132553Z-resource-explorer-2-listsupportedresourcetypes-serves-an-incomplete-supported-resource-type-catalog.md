Issue-Source: trace-sweep-divergence
Issue-Trace: resource-explorer-2-catalog-list-supported-resource-types
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-explorer-2-catalog-list-supported-resource-types
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|ListSupportedResourceTypes|2xx->2xx|$.ResourceTypes[*]
Issue-Commit: ff49842cf7656b5a010943ef9d0fd100550d134a

# ListSupportedResourceTypes serves an incomplete supported-resource-type catalog, so most entries are absent or shifted

> _Note: this issue was identified against the code base at commit `ff49842cf7656b5a010943ef9d0fd100550d134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSupportedResourceTypes

## Description
Cause 1 of 1: the emulator's ListSupportedResourceTypes catalog is not the catalog AWS Resource Explorer serves. Whole resource types are missing from it, and because the response is a single ordered list, every omission shifts the remainder, so the majority of the list's positions no longer carry the entry AWS has there.

AWS: Smithy ListSupportedResourceTypes -> ListSupportedResourceTypesOutput.ResourceTypes (list of SupportedResourceType { Service, ResourceType, CFNResourceTypes }), HTTP POST /ListSupportedResourceTypes, code 200, paginated (NextToken/MaxResults, items=ResourceTypes); MaxResults @range min 1 max 1000. The model documents SupportedResourceType.ResourceType as "the unique identifier of the resource type" and CFNResourceTypes as "the CloudFormation resource type identifiers for this resource type", but it does not enumerate the member values -- the set of supported resource types is service data, not modeled, so for the contents this is recorded behavior only. The recording answers 200 with a catalog ordered by Service then ResourceType that includes, among others, apigateway:restapis/resources/methods (CFN AWS::ApiGateway::Method), apigateway:vpclinks (AWS::ApiGateway::VpcLink), app-integrations:application (AWS::AppIntegrations::Application), app-integrations:event-integration (AWS::AppIntegrations::EventIntegration), appconfig:application/environment (AWS::AppConfig::Environment), appconfig:extensionassociation (AWS::AppConfig::ExtensionAssociation) and appmesh:mesh/virtualGateway/gatewayRoute (AWS::AppMesh::GatewayRoute). The requirement is that each entry AWS lists is present with its own Service, ResourceType and CFNResourceTypes triple -- the identifiers and their CFN mappings are the contract; the catalog's length and any one index are not, they follow from which entries exist.

Emulator: status matches (200) and the envelope shape is right, but the catalog's membership does not. The entries named above are absent -- the app-integrations service contributes no entry at all, and apigateway contributes no restapis/resources/methods or vpclinks entry -- and for sub-resource types the emulator serves only the parent (apigateway:restapis/resources in place of apigateway:restapis/resources/methods). With those entries missing the tail slides forward, so the replay reports a diverging element at roughly 380 of the ~651 positions in $.ResourceTypes (e.g. $.ResourceTypes[15] expected apigateway:restapis/resources/methods, got apigateway:restapis/resources; $.ResourceTypes[17] expected apigateway:vpclinks, got appconfig:application; $.ResourceTypes[18] expected app-integrations:application, got appconfig:configurationprofile; $.ResourceTypes[19] expected app-integrations:event-integration, got appconfig:deploymentstrategy; $.ResourceTypes[21] expected appconfig:application/environment, got appconfig:extension; $.ResourceTypes[23] expected appconfig:extensionassociation, got appflow:flow). The large index count is one defect, not hundreds: it is the cascade from the missing members.

Why: this operation is how a client discovers what Resource Explorer can index. A client that enumerates it to validate a resource-type filter, to build a type picker, or to decide whether a CloudFormation type is searchable will get a false negative for every omitted type -- an app-integrations or apigateway-method filter looks unsupported and is rejected or silently dropped before a Search is ever issued. A client that joins CFNResourceTypes back to a stack's resources finds no mapping for those types. Clients that index the list positionally, or that diff two pages, also see every entry past the first omission attributed to the wrong service.

Fix: the static supported-resource-type catalog that backs the ListSupportedResourceTypes handler -- it is the data table, not the serializer or the pagination logic, that is wrong. The table needs the missing members added (including the sub-resource types that are currently represented only by their parent) with each one's Service, ResourceType and CFNResourceTypes, keeping the Service-then-ResourceType ordering the recording shows so pagination over the catalog stays stable.

Traces: resource-explorer-2-catalog-list-supported-resource-types#task#1.

## Evidence
Replay of fixture resource-explorer-2-catalog-list-supported-resource-types, step task#1 (operation ListSupportedResourceTypes, verdict mismatch, expected_status 200, actual_status 200).

The status and response envelope agree; the ResourceTypes list membership does not. Representative element-level divergences reported by the replay:

  $.ResourceTypes[15]  expected {Service: apigateway,        ResourceType: apigateway:restapis/resources/methods, CFNResourceTypes: [AWS::ApiGateway::Method]}
                       got      {Service: apigateway,        ResourceType: apigateway:restapis/resources,         CFNResourceTypes: [AWS::ApiGateway::Resource]}
  $.ResourceTypes[17]  expected {Service: apigateway,        ResourceType: apigateway:vpclinks,                    CFNResourceTypes: [AWS::ApiGateway::VpcLink]}
                       got      {Service: appconfig,         ResourceType: appconfig:application,                  CFNResourceTypes: [AWS::AppConfig::Application]}
  $.ResourceTypes[18]  expected {Service: app-integrations,  ResourceType: app-integrations:application,            CFNResourceTypes: [AWS::AppIntegrations::Application]}
                       got      {Service: appconfig,         ResourceType: appconfig:configurationprofile,         CFNResourceTypes: [AWS::AppConfig::ConfigurationProfile]}
  $.ResourceTypes[19]  expected {Service: app-integrations,  ResourceType: app-integrations:event-integration,      CFNResourceTypes: [AWS::AppIntegrations::EventIntegration]}
                       got      {Service: appconfig,         ResourceType: appconfig:deploymentstrategy,           CFNResourceTypes: [AWS::AppConfig::DeploymentStrategy]}
  $.ResourceTypes[21]  expected {Service: appconfig,         ResourceType: appconfig:application/environment,       CFNResourceTypes: [AWS::AppConfig::Environment]}
                       got      {Service: appconfig,         ResourceType: appconfig:extension,                    CFNResourceTypes: [AWS::AppConfig::Extension]}
  $.ResourceTypes[23]  expected {Service: appconfig,         ResourceType: appconfig:extensionassociation,          CFNResourceTypes: [AWS::AppConfig::ExtensionAssociation]}
                       got      {Service: appflow,           ResourceType: appflow:flow,                           CFNResourceTypes: [AWS::AppFlow::Flow]}
  $.ResourceTypes[27]  expected {Service: appmesh,           ResourceType: appmesh:mesh/virtualGateway/gatewayRoute, CFNResourceTypes: [AWS::AppMesh::GatewayRoute]}
                       got      an appmesh entry at a different position

The replay's divergence key lists roughly 380 diverging indices spanning $.ResourceTypes[14] through $.ResourceTypes[650] -- consistent with a handful of absent members shifting the whole tail rather than with independent per-index faults.

Model basis (smithy_query, com-amazonaws-resourceexplorer2-resourceexplorer): ListSupportedResourceTypes -> output {ResourceTypes: ResourceTypeList, NextToken: String}; ResourceTypeList member SupportedResourceType {Service: String, ResourceType: String, CFNResourceTypes: CFNResourceTypeList}; http POST /ListSupportedResourceTypes code 200; paginated inputToken=NextToken pageSize=MaxResults outputToken=NextToken items=ResourceTypes; MaxResults @range 1..1000. The catalog's member values are not modeled, so their expected contents come from the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout (resource-explorer-2 is bootstrapped: /__admin/health answers 200 and the data listener accepts).
2. Replay fixture resource-explorer-2-catalog-list-supported-resource-types, step task#1 -- i.e. POST /ListSupportedResourceTypes to the resource-explorer-2 endpoint with an empty body (no MaxResults, no NextToken), signed for any region/account.
3. The response is 200. Collect ResourceTypes and project each element to its ResourceType string.
4. Search that projection for the identifiers AWS serves: `apigateway:restapis/resources/methods`, `apigateway:vpclinks`, `app-integrations:application`, `app-integrations:event-integration`, `appconfig:application/environment`, `appconfig:extensionassociation`, `appmesh:mesh/virtualGateway/gatewayRoute`. Each is absent from the emulator's catalog.
5. Equivalently, filter ResourceTypes to Service == "app-integrations": AWS returns two entries, the emulator returns none.
6. Compare the list element-wise against the recorded response: the first divergence is at index 14/15 and, because of the omissions, almost every later index is attributed to the wrong service -- e.g. index 18 is an appconfig entry where the recording has app-integrations:application.
7. Expected: every resource type AWS lists is present with its own Service / ResourceType / CFNResourceTypes triple, in Service-then-ResourceType order, so a positional or filtered comparison against the recording agrees.
