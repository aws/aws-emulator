Issue-Source: trace-sweep-divergence
Issue-Trace: glue-blueprint-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-blueprint-lifecycle glue-catalog-lifecycle glue-catalog-read-and-error-paths glue-classifier-custom-entity-and-data-quality-ruleset glue-data-quality-ruleset-lifecycle glue-data-quality-ruleset-listing glue-schema-registry-diff-and-metadata-removal glue-schema-registry-versioning glue-workflow-run-properties-roundtrip glue-workflow-trigger-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetBlueprint|2xx->2xx|$.Blueprint.ErrorMessage
Issue-Key: glue|GetBlueprint|2xx->2xx|$.Blueprint.Status
Issue-Key: glue|GetBlueprint|2xx->2xx|$.Blueprint.Version
Issue-Key: glue|GetCatalog|2xx->4xx|$.Catalog
Issue-Key: glue|GetCatalog|2xx->4xx|status
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.CreatedOn
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.Identifier
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.LastModifiedOn
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.Ruleset
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.TargetTable
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.Description
Issue-Key: glue|GetRegistry|2xx->2xx|$.Description
Issue-Key: glue|GetSchema|2xx->2xx|$.Compatibility
Issue-Key: glue|GetSchema|2xx->2xx|$.Description
Issue-Key: glue|GetSchema|2xx->2xx|$.LatestSchemaVersion
Issue-Key: glue|GetSchema|2xx->2xx|$.NextSchemaVersion
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.LastRun.Statistics.RunningActions
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Edges[*].DestinationId
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Edges[*].SourceId
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Nodes[*].UniqueId
Issue-Related: 20261029T201859Z-glue-delete-and-update-mutations-are-never-applied.md
Issue-Related: 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue reads report stale or wrong resource state and omit resource identifiers

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprint

## Description
This issue bundles 9 net-new causes with one shared fault: these reads answer 200 but describe a resource the emulator never fully tracked — a status that never leaves its initial value, an update that was never recorded, run statistics that never settle, a catalog that cannot be read at all, and identifiers AWS returns that the emulator does not mint. HIGH because causes 2, 5 and 6 make a successful AWS call wrong or impossible.

Cause 1 of 9: GetBlueprint never reports a failed registration.
AWS: when CreateBlueprint's BlueprintLocation cannot be read, registration settles terminal: Blueprint.Status 'FAILED', Blueprint.ErrorMessage describing the unreadable location (template 'Access denied on object at location: <blueprintLocation>.'), and Blueprint.Version present (1 for the first registration).
Emulator: Status stays 'ACTIVE', ErrorMessage and Version are absent.
Why: a client that polls for FAILED waits forever and then runs a blueprint that has no code.
Fix: the blueprint registration state machine (CreateBlueprint's location resolution) plus the GetBlueprint serializer's Version/ErrorMessage members.
Traces: glue-blueprint-lifecycle#task#3. Related open issues '20261028T175030Z-glue-blueprint-registration-always-settles-active-a-blueprintlocation.md' and '20261029T163346Z-glue-createblueprint-registration-for-an-unresolvable-blueprintlocation-settles.md' describe this area but do not carry this fixture step (and Version/ErrorMessage shape is asserted here).

Cause 2 of 9: GetCatalog refuses the account-root catalog.
AWS: Smithy: GetCatalog output Catalog. The account id names the default native catalog and the read succeeds: HTTP 200 with Catalog carrying CatalogId/CatalogIdentifier equal to the account id, CatalogType 'NATIVE', the fixed Description 'This is the Default Catalog.', empty CatalogProperties/CreateDatabaseDefaultPermissions/CreateTableDefaultPermissions and ResourceArn 'arn:aws:glue:<region>:<accountId>:catalog'.
Emulator: HTTP 400 and no Catalog member.
Why: any client that resolves the default catalog before creating databases/tables fails at step one.
Fix: GetCatalog handler (synthesize the account-root native catalog) and the catalog state seed.
Traces: glue-catalog-lifecycle#task#1 (also glue-catalog-read-and-error-paths#task#2). Related open issue '20261027T124611Z-glue-3-net-new-root-causes-createcatalog-accepts.md' names this area but does not carry this fixture step.

Cause 3 of 9: CreateDataQualityRuleset's response omits the ruleset it created.
AWS: Smithy models only Name on CreateDataQualityRulesetResponse, so the rest is recorded behavior only: AWS echoes Identifier, Ruleset, TargetTable (DatabaseName/TableName) and the CreatedOn/LastModifiedOn timestamps.
Emulator: all of those members are absent.
Why: a client that uses the returned Identifier to address the ruleset has nothing to use.
Fix: CreateDataQualityRuleset serializer + ruleset state (assign and return an identifier and the echoed definition).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#11.

Cause 4 of 9: GetDataQualityRuleset omits Identifier.
AWS: recorded behavior only — the read returns the ruleset's Identifier alongside Name/Ruleset/timestamps (the model's output has no Identifier member).
Emulator: absent.
Why: the ruleset cannot be addressed by identifier in follow-up calls.
Fix: GetDataQualityRuleset serializer + ruleset state.
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#13 (also glue-data-quality-ruleset-lifecycle#task#2).

Cause 5 of 9: CreateDataQualityRuleset omits the echoed definition in the listing fixture.
AWS: as cause 3 — Description, Ruleset, CreatedOn and LastModifiedOn come back on the create response.
Emulator: all absent.
Why: as cause 3.
Fix: same serializer as cause 3; distinct cluster, must be re-replayed.
Traces: glue-data-quality-ruleset-listing#setup#1.

Cause 6 of 9: GetRegistry does not reflect UpdateRegistry.
AWS: after UpdateRegistry changes the registry Description, GetRegistry returns the new Description.
Emulator: returns the pre-update Description — the update is accepted and discarded.
Why: a client editing registry metadata sees its edit vanish.
Fix: UpdateRegistry handler/state (persist Description) and the GetRegistry read path.
Traces: glue-schema-registry-diff-and-metadata-removal#task#5.

Cause 7 of 9: GetSchema does not reflect UpdateSchema or registered versions.
AWS: after UpdateSchema sets Compatibility and Description and a new version is registered, GetSchema returns the new Compatibility, the new Description, LatestSchemaVersion incremented to the newest registered version and NextSchemaVersion one higher.
Emulator: returns the creation-time Compatibility, no Description, and version counters still at their initial values.
Why: schema evolution is invisible — a producer cannot tell which version is current or what compatibility is enforced.
Fix: UpdateSchema handler/state and the schema version counters in the GetSchema read path (see also open issue '20261025T170315Z-glue-registerschemaversion-stub.md' for the version-registration half).
Traces: glue-schema-registry-versioning#task#15.

Cause 8 of 9: GetWorkflow reports a finished run as still running.
AWS: Workflow.LastRun.Statistics.RunningActions counts actions still running; for a settled run it is 0.
Emulator: reports 1 — the action count is never decremented when the run settles.
Why: a client that waits for RunningActions to reach 0 never proceeds.
Fix: the workflow-run statistics bookkeeping (run/action state transitions) read by GetWorkflow.
Traces: glue-workflow-run-properties-roundtrip#task#11.

Cause 9 of 9: GetWorkflow graph node and edge identifiers are not AWS's.
AWS: each Graph.Nodes[].UniqueId (and the Edges[] SourceId/DestinationId referencing them) is an opaque AWS-minted node id whose recorded shape the fixture asserts; a job node also carries JobDetails.
Emulator: builds ids by concatenating the node kind and the resource name ('job_<jobName>', 'trigger_<triggerName>'), which fails the shape assertion, and omits JobDetails on the job node.
Why: a client that treats these ids as opaque keys (or correlates them with AWS-side ids) works with values AWS would never mint.
Fix: the workflow graph builder's node-id generation and the node serializer, in the GetWorkflow read path.
Traces: glue-workflow-trigger-lifecycle#task#7.

## Evidence
Replay of simparity-wire-aws-v1:
1. GetBlueprint 200/200: $.Blueprint.Status expected 'FAILED' got 'ACTIVE'; $.Blueprint.ErrorMessage missing (expected 'Access denied on object at location: s3://.../blueprints/....zip.'); $.Blueprint.Version missing (expected 1).
2. GetCatalog: status expected 200, got 400; $.Catalog missing (expected {'CatalogId': '000000000000', 'CatalogIdentifier': '000000000000', 'CatalogProperties': {}, 'CatalogType': 'NATIVE', 'CreateDatabaseDefaultPermissions': [], 'CreateTableDefaultPermissions': [], 'Description': 'This is the Default Catalog.', 'Name': '000000000000', 'ResourceArn': 'arn:aws:glue:us-west-2:000000000000:catalog'}).
3. CreateDataQualityRuleset 200/200: $.Identifier, $.Ruleset, $.TargetTable, $.CreatedOn, $.LastModifiedOn all missing.
4. GetDataQualityRuleset 200/200: $.Identifier missing.
5. CreateDataQualityRuleset 200/200: $.Description, $.Ruleset, $.CreatedOn, $.LastModifiedOn all missing.
6. GetRegistry 200/200: $.Description expected 'sim2real probe diff registry updated' got 'sim2real probe diff registry'.
7. GetSchema 200/200: $.Compatibility expected 'FULL' got 'BACKWARD'; $.Description missing (expected 'sim2real probe schema updated'); $.LatestSchemaVersion expected 2 got 1; $.NextSchemaVersion expected 3 got 2.
8. GetWorkflow 200/200: $.Workflow.LastRun.Statistics.RunningActions expected 0 got 1.
9. GetWorkflow 200/200: $.Workflow.Graph.Nodes[0].UniqueId expected '<shaped-not-asserted>' got 'trigger_sim2real-probe-glue-glue-trg-ec515cf1'; Nodes[1].UniqueId got 'job_sim2real-probe-glue-glue-job-ec515cf1'; Edges[0].SourceId/DestinationId likewise; Nodes[1].JobDetails missing (expected {}).
(Names, locations and descriptions above are that run's values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-blueprint-lifecycle#task#3, glue-catalog-lifecycle#task#1, glue-classifier-custom-entity-and-data-quality-ruleset#task#11, glue-classifier-custom-entity-and-data-quality-ruleset#task#13, glue-data-quality-ruleset-listing#setup#1, glue-schema-registry-diff-and-metadata-removal#task#5, glue-schema-registry-versioning#task#15, glue-workflow-run-properties-roundtrip#task#11, glue-workflow-trigger-lifecycle#task#7.
3. Each fixture creates (and where relevant updates or runs) the resource, then reads it; compare status, state members and identifiers against the AWS behavior quoted per cause.
