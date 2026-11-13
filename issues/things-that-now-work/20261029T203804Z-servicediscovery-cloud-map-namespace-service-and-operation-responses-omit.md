Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-dns-namespace-request-validation-errors
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-dns-namespace-request-validation-errors servicediscovery-http-namespace-service-instance-discovery-lifecycle servicediscovery-resource-tagging-lifecycle-and-limits servicediscovery-operation-polling-and-filtered-listing servicediscovery-service-attributes-crud-and-limits servicediscovery-multi-service-instance-isolation-and-revision-tracking servicediscovery-service-attribute-binary-blob-round-trip servicediscovery-service-attributes-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetNamespace|2xx->2xx|$.Namespace.Properties.DnsProperties
Issue-Key: servicediscovery|GetNamespace|2xx->2xx|$.Namespace.ResourceOwner
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.OwnerAccount
Issue-Key: servicediscovery|CreateService|2xx->2xx|$.Service.CreatedByAccount
Issue-Key: servicediscovery|CreateService|2xx->2xx|$.Service.DnsConfig
Issue-Key: servicediscovery|CreateService|2xx->2xx|$.Service.ResourceOwner
Issue-Key: servicediscovery|GetService|2xx->2xx|$.Service.CreatedByAccount
Issue-Key: servicediscovery|GetService|2xx->2xx|$.Service.DnsConfig
Issue-Key: servicediscovery|GetService|2xx->2xx|$.Service.ResourceOwner
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn
Issue-Key: servicediscovery|GetServiceAttributes|2xx->2xx|$.ServiceAttributes.ResourceOwner
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map namespace, service and operation responses omit ResourceOwner, CreatedByAccount, OwnerAccount and DnsConfig

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetService

## Description
SIX independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator: successful (HTTP 200) namespace, operation, service and service-attribute responses are missing modelled members that AWS always populates - the ownership members (ResourceOwner / CreatedByAccount / Operation.OwnerAccount) and the structural members Namespace.Properties.DnsProperties and Service.DnsConfig. Each of the six causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact: AWSAccountId is a 12-character account id and the ownership members are documented as 'the ID of the Amazon Web Services account that created/owns ...' - in a single-account call they all carry the CALLER'S own account id, which is the emulator's configured account, not a literal from one recording.

Cause 1 of 6: GetNamespace omits Namespace.ResourceOwner and Namespace.Properties.DnsProperties.
AWS: Smithy GetNamespace{Id (required)} -> Namespace{Id, Arn, ResourceOwner: AWSAccountId, Name, Type, Description, ServiceCount, Properties: NamespaceProperties{DnsProperties: DnsProperties{HostedZoneId, SOA}, HttpProperties}, CreateDate, CreatorRequestId}. Recorded: the Namespace carries ResourceOwner = the owning account id and Properties.DnsProperties present with an SOA sub-structure - for HTTP namespaces too, where DnsProperties is present (with SOA) alongside HttpProperties rather than omitted.
Emulator: HTTP 200 with the namespace, but no ResourceOwner member and no Properties.DnsProperties member (Properties carries HttpProperties only).
Why: a client that reads Namespace['ResourceOwner'] (cross-account sharing decisions) or Namespace['Properties']['DnsProperties'] (hosted-zone / SOA discovery) gets a KeyError against the emulator where AWS always supplies the member.
Fix: the Namespace serializer/state projection for GetNamespace (and the namespace record in state), which must populate ResourceOwner from the account and always emit Properties.DnsProperties.
Traces: servicediscovery-dns-namespace-request-validation-errors#task#9; same cluster also covers servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#4, #task#7 and servicediscovery-resource-tagging-lifecycle-and-limits#setup#3.

Cause 2 of 6: GetOperation omits Operation.OwnerAccount.
AWS: Smithy GetOperation{OperationId (required), OwnerAccount} -> Operation{Id, OwnerAccount: AWSAccountId ('the ID of the account that owns the namespace associated with the operation'), Type, Status, ErrorMessage, ErrorCode, CreateDate, UpdateDate, Targets}. Recorded: OwnerAccount is present on every operation read.
Emulator: HTTP 200 with the Operation but no OwnerAccount member.
Why: pollers and cross-account tooling that read Operation['OwnerAccount'] break; the member is also the only ownership signal on this shape.
Fix: the Operation serializer / operation record in the service state, for every operation-producing path.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#3; same cluster also covers servicediscovery-operation-polling-and-filtered-listing#task#3, #task#4, servicediscovery-resource-tagging-lifecycle-and-limits#setup#2 and servicediscovery-service-attributes-crud-and-limits#setup#2.

Cause 3 of 6: CreateService's returned Service omits ResourceOwner, CreatedByAccount and DnsConfig.
AWS: Smithy CreateService -> Service{Id, Arn, ResourceOwner: AWSAccountId, Name, NamespaceId, Description, InstanceCount, DnsConfig, Type, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatorRequestId, CreatedByAccount: AWSAccountId}. Recorded: both account members are present, and DnsConfig is present as an EMPTY structure even for a service in an HTTP namespace that was created without any DnsConfig - AWS emits the member rather than omitting it.
Emulator: HTTP 200 with the Service but no ResourceOwner, no CreatedByAccount and no DnsConfig member.
Why: callers that read Service['DnsConfig'] unconditionally (to inspect routing policy / record list) or the account members get a KeyError where AWS always supplies them.
Fix: the Service serializer shared by CreateService/GetService/ListServices plus the service record in state.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#8; same cluster also covers servicediscovery-resource-tagging-lifecycle-and-limits#setup#4 and servicediscovery-service-attributes-crud-and-limits#setup#3.

Cause 4 of 6: GetService's Service omits the same three members.
AWS: Smithy GetService{Id (required)} -> Service (shape as in cause 3). Recorded: ResourceOwner, CreatedByAccount and DnsConfig (empty structure for an HTTP-namespace service) all present.
Emulator: all three members absent.
Why/Fix: as cause 3, on the read path.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#9; same cluster also covers servicediscovery-service-attributes-crud-and-limits#task#5.

Cause 5 of 6: ListServices' summaries omit ResourceOwner, CreatedByAccount and DnsConfig on every entry.
AWS: Smithy ListServices -> Services: ServiceSummaryList of ServiceSummary{Id, Arn, ResourceOwner, Name, Type, Description, InstanceCount, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatedByAccount}. Recorded: each summary carries the two account members and a DnsConfig structure.
Emulator: HTTP 200 with the list, every summary missing those three members.
Why: a client that filters or displays a list by owner, or that inspects DnsConfig per summary, has to re-read each service individually and still gets nothing.
Fix: the ServiceSummary projection in ListServices' handler (same state fields as cause 3).
Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5.

Cause 6 of 6: GetServiceAttributes omits ServiceAttributes.ResourceOwner.
AWS: Smithy GetServiceAttributes{ServiceId (required)} -> ServiceAttributes{ServiceArn, ResourceOwner: AWSAccountId, Attributes: ServiceAttributesMap (length 1..30)}. Recorded: ResourceOwner present.
Emulator: HTTP 200 with ServiceArn and Attributes only.
Why: the ownership member of this shape is unreadable; cross-account attribute tooling breaks.
Fix: the ServiceAttributes serializer in GetServiceAttributes' handler.
Traces: servicediscovery-service-attribute-binary-blob-round-trip#task#5; same cluster also covers servicediscovery-service-attributes-crud-and-limits#task#7, #task#9 and servicediscovery-service-attributes-unicode-value-round-trip#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. All six clusters are status-matched 200 responses with missing members (expected values are the recorded AWS responses; the account id in the recording is the caller's own account):

Cause 1 - servicediscovery-dns-namespace-request-validation-errors#task#9, GetNamespace: '$.Namespace.Properties.DnsProperties: missing (expected {\'SOA\': {}}, got None)'; '$.Namespace.ResourceOwner: missing (expected the caller account id, got None)'. 4 steps affected.
Cause 2 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#3, GetOperation: '$.Operation.OwnerAccount: missing (expected the caller account id, got None)'. 5 steps affected.
Cause 3 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#8, CreateService: '$.Service.CreatedByAccount: missing', '$.Service.DnsConfig: missing (expected {}, got None)', '$.Service.ResourceOwner: missing'. 3 steps affected.
Cause 4 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#9, GetService: the same three members missing. 2 steps affected.
Cause 5 - servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5, ListServices: for each summary, '$.Services[Arn=...].CreatedByAccount: missing', '.DnsConfig: missing (expected {}, got None)', '.ResourceOwner: missing' (both services in the page).
Cause 6 - servicediscovery-service-attribute-binary-blob-round-trip#task#5, GetServiceAttributes: '$.ServiceAttributes.ResourceOwner: missing'. 4 steps affected.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_http_namespace(Name='<unique>')['OperationId']; sd.get_operation(OperationId=op) -> Cause 2: 'OwnerAccount' absent from Operation. Take ns = Operation['Targets']['NAMESPACE'].
2. sd.get_namespace(Id=ns) -> Cause 1: Namespace has no 'ResourceOwner' and Namespace['Properties'] has no 'DnsProperties'. Repeat for a public DNS namespace.
3. svc = sd.create_service(Name='<unique>', NamespaceId=ns) -> Cause 3: Service has no 'ResourceOwner', 'CreatedByAccount' or 'DnsConfig'; sd.get_service(Id=svc['Service']['Id']) -> Cause 4: same three absent; sd.list_services() -> Cause 5: every summary missing the same three.
4. sd.update_service_attributes(ServiceId=<svc id>, Attributes={'k':'v'}); sd.get_service_attributes(ServiceId=<svc id>) -> Cause 6: ServiceAttributes has no 'ResourceOwner'.
