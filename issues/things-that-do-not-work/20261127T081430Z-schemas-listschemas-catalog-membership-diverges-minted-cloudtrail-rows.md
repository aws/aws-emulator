Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#22
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: 51d78c748acd56cb8b78be671a4bd9d576b494fb

# schemas ListSchemas catalog membership diverges: minted CloudTrail rows, absent published sources, shifted pages

> _Note: this issue was identified against the code base at commit `51d78c748acd56cb8b78be671a4bd9d576b494fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
ListSchemas on the AWS-managed discovery registry (aws.events) is served from a seeded catalog whose MEMBERSHIP diverges from the registry AWS publishes: for some event sources the emulator mints the six generic CloudTrail/Console schemas AWS does not publish, for others it holds no rows at all, and in two places it carries a duplicate row spelled '...@AWSAPICallViaCloudTrail' (capital 'Via') beside the published '...@AWSAPICallviaCloudTrail'. Because ListSchemas is paginated (Smithy: ListSchemas GET /v1/registries/name/{RegistryName}/schemas, @paginated inputToken NextToken / outputToken NextToken / items Schemas / pageSize Limit), a wrong row count re-cuts every page boundary, so a page of the recorded walk comes back holding a different event source's rows — or, past the end of the short catalog, holding nothing. This is a different defect from the two open schemas issues '20261118T060835Z-schemas-response-payload-framing-still-diverges-from-aws.md' (payload framing) and '20261122T071708Z-schemas-aws-managed-cloudtrail-document-still-diverges-on.md' (the CloudTrail schema DOCUMENT on DescribeSchema/ExportSchema); neither covers which rows ListSchemas enumerates. 10 causes, one per diverging page of the recorded walk.

Cause 1 of 10 — aws.application-autoscaling source over-populated (fixture schemas-catalog-list-schemas, step task#22).
AWS: Schemas[*] is a list of SchemaSummary (Smithy: SchemaSummary{SchemaName, LastModified, VersionCount, SchemaArn, Tags(jsonName tags)}). For event source aws.application-autoscaling the recorded registry publishes the service's own event schema (aws.application-autoscaling@ApplicationAutoScalingScalingActivityStateChange) and no generic CloudTrail/Console rows; the next published source on the page is aws.application-insights.
Emulator: the page opens with aws.application-autoscaling@AWSAPICallviaCloudTrail, @AWSConsoleActionviaCloudTrail, @AWSConsoleSignInviaCloudTrail, @AWSInsightviaCloudTrail, @AWSNetworkActivityEventviaCloudTrail and @AWSServiceEventviaCloudTrail — six rows AWS does not publish for that source — and the published ApplicationAutoScalingScalingActivityStateChange row is absent, so all 21 positions of the page report a different schema than AWS returned.
Why: a client enumerating the registry (console schema browser, code-binding generation, `aws schemas list-schemas --registry-name aws.events`) sees schemas that do not exist and misses the one that does, and every NextToken page after it is offset.
Fix: the schemas service's seeded aws.events catalog state (services/service-com-amazonaws-schemas-schemas/src/state.rs) must enumerate exactly the schemas the registry publishes per event source rather than synthesizing a fixed six-row CloudTrail set for every source.
Traces: schemas-catalog-list-schemas#task#22 (status 200 both sides; keys $.Schemas[0]..$.Schemas[20]).

Cause 2 of 10 — duplicate 'AWSAPICallViaCloudTrail' row under aws.autoscaling (step task#32).
AWS: the aws.autoscaling source publishes one API-call schema, spelled aws.autoscaling@AWSAPICallviaCloudTrail (lowercase 'via'); the recorded page runs aws.autoscaling@AWSAPICallviaCloudTrail, @AWSConsoleActionviaCloudTrail, ... then the EC2 Auto Scaling event schemas.
Emulator: the catalog also holds aws.autoscaling@AWSAPICallViaCloudTrail (capital 'Via') carrying LastModified 2019-12-01T00:59:43Z, and the preceding aws.autoscaling-plans rows are still being emitted where AWS has already moved to aws.autoscaling, so the page is shifted and all 28 positions mismatch.
Why: two rows for one schema break de-duplicated client inventories and the extra name resolves to nothing on DescribeSchema; the shift corrupts the rest of the walk.
Fix: remove the mis-cased duplicate from the seeded catalog and keep one row per published schema name.
Traces: schemas-catalog-list-schemas#task#32.

Cause 3 of 10 — aws.ec2 source incomplete (step task#70).
AWS: expected Schemas[7] is aws.ec2@CapacityBlockDelivered, with the rest of EC2's published event schemas following in name order.
Emulator: Schemas[7] is aws.ec2@EC2InstanceStateChangeNotification — the emulator's aws.ec2 set omits published schemas, so 77 positions of the page name a different schema than AWS returned.
Why: a client cannot discover or bind EC2 event schemas it is told do not exist.
Fix: seed the full published aws.ec2 schema set.
Traces: schemas-catalog-list-schemas#task#70.

Cause 4 of 10 — aws.ec2fleet rows misdated by one second and shifted (step task#71).
AWS: aws.ec2fleet@EC2FleetError, @EC2FleetInformation and @EC2FleetInstanceChange carry LastModified 2020-12-17T18:43:40Z, and from Schemas[5] the EC2SpotFleet* schemas carry their own, later last-modified instants.
Emulator: those three rows carry 2020-12-17T18:43:39Z, and from Schemas[5] every EC2SpotFleet* row carries 2020-12-17T18:43:40Z — the seed stores one instant per group rather than the per-schema value, so rows 0-2 and 5-9 all diverge.
Why: LastModified is the member clients poll to decide whether a cached schema or generated code binding is stale; a wrong instant makes a changed schema look unchanged.
Fix: carry each published schema's own LastModified in the seeded catalog record.
Traces: schemas-catalog-list-schemas#task#71.

Cause 5 of 10 — aws.ecs source incomplete (step task#75).
AWS: expected Schemas[7] is aws.ecs@ECSContainerInstanceHealthChange.
Emulator: Schemas[7] is aws.ecs@ECSTaskStateChange; published ECS schemas are missing from the seed, shifting 15 positions of the page.
Why: as above — undiscoverable published schemas plus an offset walk.
Fix: seed the full published aws.ecs schema set.
Traces: schemas-catalog-list-schemas#task#75.

Cause 6 of 10 — aws.moonsail and aws.moontide sources absent, page comes back empty (step task#143).
AWS: this page of the walk returns 16 rows: aws.moonsail@AWSAPICallviaCloudTrail and its five sibling CloudTrail/Console rows, then the aws.moontide@MDE*/DevelopmentEnvironmentStateChange schemas.
Emulator: every one of the 16 positions is absent (the comparator reports 'got None') — the seeded catalog holds no aws.moonsail or aws.moontide source and is short enough overall that this NextToken page resolves past its end.
Why: a paginated enumeration silently ends early, so a client's registry inventory is truncated.
Fix: seed the missing event sources so the catalog's length and page boundaries match the published registry.
Traces: schemas-catalog-list-schemas#task#143.

Cause 7 of 10 — aws.qapps and aws.qbusiness sources absent (step task#147).
AWS: the page runs aws.qapps@AWSAPICallviaCloudTrail ... then aws.qbusiness@AWSAPICallviaCloudTrail ...
Emulator: it returns aws.qldb@* and aws.quicksight@* rows instead — the two sources are not in the catalog, so the page boundary falls elsewhere and 86 positions mismatch.
Why: whole services' event schemas are undiscoverable and the page offsets diverge.
Fix: seed the missing sources.
Traces: schemas-catalog-list-schemas#task#147.

Cause 8 of 10 — aws.route53-recovery-control-config source absent (step task#161).
AWS: the page opens with aws.route53-recovery-control-config@AWSAPICallviaCloudTrail and its five siblings before reaching aws.route53.
Emulator: it opens with aws.route53@AWSAPICallviaCloudTrail; the recovery-control-config source is missing, shifting 41 positions.
Why/Fix/Traces: as cause 7. Traces: schemas-catalog-list-schemas#task#161.

Cause 9 of 10 — aws.sagemaker-geospatial absent plus a mis-cased duplicate sagemaker row (step task#168).
AWS: the page opens with aws.sagemaker-geospatial@AWSAPICallviaCloudTrail and its five siblings; the aws.sagemaker source publishes one API-call schema, spelled with lowercase 'via'.
Emulator: it opens with aws.sagemaker@AWSAPICallViaCloudTrail (capital 'Via', LastModified 2019-12-01T00:59:39Z) followed by the lowercase row — the geospatial source is absent and the sagemaker source carries a duplicate, so 72 positions mismatch.
Why: an invented schema name beside the real one, and a whole source undiscoverable.
Fix: seed the missing source and drop the mis-cased duplicate.
Traces: schemas-catalog-list-schemas#task#168.

Cause 10 of 10 — aws.securityagent source absent (step task#176).
AWS: the page opens with aws.securityagent@AWSAPICallviaCloudTrail and its five siblings before aws.securityhub.
Emulator: it opens with aws.securityhub@AWSAPICallviaCloudTrail; 54 positions mismatch.
Why/Fix/Traces: as cause 7. Traces: schemas-catalog-list-schemas#task#176.

## Evidence
Both sides answer HTTP 200 and no error code or message is involved: the divergence is in the ListSchemas response body. Examples from the replay:
- task#22: $.Schemas[0] expected {'SchemaName': 'aws.application-autoscaling@ApplicationAutoScalingScalingActivityStateChange', 'VersionCount': 1, 'tags': {}}, got {'SchemaName': 'aws.application-autoscaling@AWSAPICallviaCloudTrail', 'VersionCount': 1, 'tags': {}}; $.Schemas[1] expected 'aws.application-insights@AWSAPICallviaCloudTrail', got 'aws.application-autoscaling@AWSConsoleActionviaCloudTrail' (21 positions diverge).
- task#32: $.Schemas[6] expected 'aws.autoscaling@AWSServiceEventviaCloudTrail', got 'aws.autoscaling@AWSAPICallViaCloudTrail' with LastModified '2019-12-01T00:59:43Z' (capital 'Via' duplicate; 28 positions diverge).
- task#70: $.Schemas[7] expected 'aws.ec2@CapacityBlockDelivered', got 'aws.ec2@EC2InstanceStateChangeNotification' (77 positions diverge).
- task#71: $.Schemas[0] expected LastModified '2020-12-17T18:43:40Z', got '2020-12-17T18:43:39Z' for 'aws.ec2fleet@EC2FleetError'; $.Schemas[5] expected 'aws.ec2fleet@EC2SpotFleetError' with its own instant, got the same '2020-12-17T18:43:40Z'.
- task#75: $.Schemas[7] expected 'aws.ecs@ECSContainerInstanceHealthChange', got 'aws.ecs@ECSTaskStateChange'.
- task#143: $.Schemas[0]..$.Schemas[15] expected the aws.moonsail/aws.moontide rows, got None for every position (page empty).
- task#147: $.Schemas[0] expected 'aws.qapps@AWSAPICallviaCloudTrail', got 'aws.qldb@AWSAPICallviaCloudTrail'.
- task#161: $.Schemas[0] expected 'aws.route53-recovery-control-config@AWSAPICallviaCloudTrail', got 'aws.route53@AWSAPICallviaCloudTrail'.
- task#168: $.Schemas[0] expected 'aws.sagemaker-geospatial@AWSAPICallviaCloudTrail', got 'aws.sagemaker@AWSAPICallViaCloudTrail' (LastModified '2019-12-01T00:59:39Z').
- task#176: $.Schemas[0] expected 'aws.securityagent@AWSAPICallviaCloudTrail', got 'aws.securityhub@AWSAPICallviaCloudTrail'.
Smithy citations: ListSchemas GET /v1/registries/name/{RegistryName}/schemas, @paginated{inputToken NextToken, outputToken NextToken, items Schemas, pageSize Limit}; SchemaSummary members SchemaName, LastModified (__timestampIso8601, @timestampFormat date-time), VersionCount (__long), SchemaArn, Tags (jsonName 'tags').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture schemas-catalog-list-schemas.
2. Walk ListSchemas over the AWS-managed registry: GET /v1/registries/name/aws.events/schemas with the recorded limit, following NextToken (equivalently `aws schemas list-schemas --registry-name aws.events`).
3. Compare $.Schemas[*] at steps task#22, task#32, task#70, task#71, task#75, task#143, task#147, task#161, task#168 and task#176 against the recorded responses: the SchemaName sequence diverges at each of those pages, and task#143 returns no rows at all.
