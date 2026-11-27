Issue-Source: trace-sweep-divergence
Issue-Trace: glue-connection-type-catalog
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-connection-type-catalog glue-data-quality-statistic-annotation-and-batch-run-reads
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Categories
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.LogoUrl
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Vendor
Issue-Key: glue|BatchGetDataQualityRulesetEvaluationRun|4xx->2xx|$.__type
Issue-Key: glue|BatchGetDataQualityRulesetEvaluationRun|4xx->2xx|status
Issue-Commit: bcdf81b7b90f0ccd5f5c96f3497e970f6d1caf2b

# Glue connection-type and data-quality batch reads diverge: catalog metadata omitted and invalid RunIds accepted

> _Note: this issue was identified against the code base at commit `bcdf81b7b90f0ccd5f5c96f3497e970f6d1caf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnectionType

## Description
Two net-new root causes on the glue emulator, both on read paths that answer 200 where AWS answers something else. Neither is covered by an open issue: the nearest open items are 20261117T133709Z-glue-read-paths-omit-or-misreport-members-aws.md (a different set of operations: GetCrawler, GetDataCatalogExportConfiguration, GetDataflowGraph, GetPlan, GetUnfilteredPartition*Metadata, GetDataCatalogEncryptionSettings) and 20261117T133709Z-glue-accepts-requests-aws-refuses-and-answers-200.md (GetDataflowGraph, GetPlan, GetMapping, CreateUsageProfile, GetUsageProfile); neither names DescribeConnectionType or BatchGetDataQualityRulesetEvaluationRun, and the replay reports both clusters as unowned.

Cause 1 of 2: DescribeConnectionType returns a near-empty response instead of the connection type's catalog metadata.
Fixture glue-connection-type-catalog, step task#2 (operation DescribeConnectionType).
AWS: Smithy: DescribeConnectionType input ConnectionType (NameString, @required, @length 1..255); output DescribeConnectionTypeResponse carries ConnectionType, Description (Description @length 0..1024), Capabilities (structure Capabilities with @required SupportedAuthenticationTypes / SupportedDataOperations / SupportedComputeEnvironments), ConnectionProperties / ConnectionOptions / PhysicalConnectionRequirements / AthenaConnectionProperties / PythonConnectionProperties / SparkConnectionProperties (PropertiesMap), AuthenticationConfiguration (AuthConfiguration, documented as "The authentication configuration for a connection returned by the DescribeConnectionType API", with @required AuthenticationType and the OAuth2/Basic/Custom property maps) and ComputeEnvironmentConfigurations. For a connection type the service knows, AWS populates this catalog metadata: the recording shows AuthenticationConfiguration (AuthenticationType with its AllowedValues, DataType/Description/DisplayName/Phase/PropertyType/PropertyTypes/Required/Validations per property, plus OAuth2Properties entries such as AuthorizationCodeUrl, AuthorizationCode and Scope with their ConditionallyRequired clauses and default values), Capabilities, ConnectionOptions, ConnectionProperties, Description and PhysicalConnectionRequirements, and additionally Categories, DisplayName, LogoUrl and Vendor — the last four are recorded behavior only (they are not in the checked-in model's output shape but AWS serializes them).
Emulator: answers HTTP 200 with those ten members absent (replay reports value_mismatch/missing on $.AuthenticationConfiguration, $.Capabilities, $.Categories, $.ConnectionOptions, $.ConnectionProperties, $.Description, $.DisplayName, $.LogoUrl, $.PhysicalConnectionRequirements and $.Vendor). The emulator has no connection-type catalog behind this operation, so every connection type looks identical and featureless.
Why: a client that uses DescribeConnectionType to drive connection creation — picking an authentication type from AuthenticationConfiguration.AuthenticationType.AllowedValues, discovering which properties are Required or ConditionallyRequired, reading defaults, or rendering the type's DisplayName/Categories/Vendor in a picker — gets nothing to work with. It either cannot build a valid CreateConnection request at all, or it builds one from guesses and is refused later, and feature-detection on Capabilities.SupportedComputeEnvironments silently reports that no environment is supported.
Fix: the DescribeConnectionType handler and the connection-type catalog data it reads — the emulator needs a static per-connection-type descriptor (authentication configuration, capabilities, property/option descriptors with their phases, requiredness, defaults and validations, plus the presentation members) and must serialize it, instead of returning a response with only the echoed ConnectionType.
Traces: glue-connection-type-catalog#task#2.

Cause 2 of 2: BatchGetDataQualityRulesetEvaluationRun accepts a RunIds input AWS refuses, answering 200 instead of InvalidInputException.
Fixture glue-data-quality-statistic-annotation-and-batch-run-reads, step task#2 (operation BatchGetDataQualityRulesetEvaluationRun).
AWS: Smithy: BatchGetDataQualityRulesetEvaluationRun input RunIds (DataQualityRulesetEvaluationRunIdList, @required, @length min 1 max 100, member HashString @length 1..255 @pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$); modeled errors InvalidInputException (client, HTTP 400, "The input provided was not valid."), OperationTimeoutException, InternalServiceException. The recording shows AWS refusing this request with HTTP 400 and __type "InvalidInputException" rather than returning a Runs/RunsNotFound body — i.e. the RunIds value is rejected by input validation before any lookup. The requirement is the status (400) and the error code (InvalidInputException); the recorded response asserts no Message for this refusal, so only the fixed error class is pinned here and the message must be whatever AWS words for an invalid RunIds input (its fixed text, with the offending member/value taken from the request).
Emulator: answers HTTP 200 with a success body — no __type at all (replay: "status: expected 400, got 200" and "$.__type: missing (expected 'InvalidInputException', got None)"). The operation performs no input validation on RunIds and falls straight through to the batch lookup.
Why: a client that relies on AWS rejecting a malformed or out-of-range RunIds list sees a 200 instead of an error, so its error branch never runs; a bad batch request is silently reported as a successful read of zero runs, and callers conflate "invalid request" with "no such runs". Code that distinguishes InvalidInputException (fix the request) from RunsNotFound (the ids simply do not exist) takes the wrong path.
Fix: BatchGetDataQualityRulesetEvaluationRun's input validation in its handler — enforce the modeled RunIds list and member constraints up front and raise InvalidInputException (400) before the lookup, instead of only serializing Runs/RunsNotFound.
Traces: glue-data-quality-statistic-annotation-and-batch-run-reads#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 1613 steps, 1440 passed, 41 divergence clusters). The two clusters below are the only ones with an empty owned_by and an empty stub_path.

Cause 1 — divergence_key service=glue|DescribeConnectionType|status=200->200|keys=$.AuthenticationConfiguration,$.Capabilities,$.Categories,$.ConnectionOptions,$.ConnectionProperties,$.Description,$.DisplayName,$.LogoUrl,$.PhysicalConnectionRequirements,$.Vendor
  fixture glue-connection-type-catalog step task#2
  expected_status 200, actual_status 200
  Expected: AuthenticationConfiguration populated, e.g. {'__type': 'AuthConfiguration', 'AuthenticationType': {'AllowedValues': [{'Description': 'OAUTH2 Authentication', 'DisplayName': 'OAUTH2', 'Value': 'OAUTH2'}], 'DataType': 'STRING', 'Description': 'Authentication type to connect to data source.', 'DisplayName': 'Authentication Type', 'Name': 'AuthenticationType', 'Phase': 'CONNECTION_CREATION', 'PropertyType': 'USER_INPUT', 'PropertyTypes': ['USER_INPUT'], 'Required': True, 'Validations': []}, 'OAuth2Properties': {'AuthorizationCodeUrl': {...'PropertyType': 'READ_ONLY'...}, 'AuthorizationCode': {'ConditionallyRequired': [{'OAuth2GrantType': 'AUTHORIZATION_CODE', ...}], ...}, 'Scope': {...}, ...}} together with Capabilities, Categories, ConnectionOptions, ConnectionProperties, Description, DisplayName, LogoUrl, PhysicalConnectionRequirements and Vendor.
  Actual: 200 with all ten of those members absent from the body.

Cause 2 — divergence_key service=glue|BatchGetDataQualityRulesetEvaluationRun|status=400->200|keys=$.__type,status
  fixture glue-data-quality-statistic-annotation-and-batch-run-reads step task#2
  expected_status 400, actual_status 200
  detail: "status: expected 400, got 200" / "$.__type: missing (expected 'InvalidInputException', got None)"
  Expected: HTTP 400 with __type InvalidInputException. Actual: HTTP 200 with a success body and no __type.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: replay fixture glue-connection-type-catalog and inspect step task#2 — the DescribeConnectionType call for a known connection type. Compare the 200 body against the recorded one: AuthenticationConfiguration, Capabilities, Categories, ConnectionOptions, ConnectionProperties, Description, DisplayName, LogoUrl, PhysicalConnectionRequirements and Vendor are all absent from the emulator's response.
4. Cause 2: replay fixture glue-data-quality-statistic-annotation-and-batch-run-reads and inspect step task#2 — the BatchGetDataQualityRulesetEvaluationRun call whose RunIds input AWS rejects. The emulator answers HTTP 200 with a success body; AWS answers HTTP 400 with __type InvalidInputException.
5. Both steps must pass (status, error code and the listed members) before this issue is closed.

## Resolution

Cause 2 is FIXED: `BatchGetDataQualityRulesetEvaluationRun` (and the single-run read beside it)
now refuse a `RunId` outside the grammar every data quality run id is minted in, with the
modelled `InvalidInputException` (400) before any lookup. Fixture
`glue-data-quality-statistic-annotation-and-batch-run-reads` passes end to end.

Cause 1 is DECLINED, and its ten `Issue-Key` lines still diverge on VALUES rather than on the
absence this report describes. `DescribeConnectionType` is now a read of a per-connection-type
catalog and answers `Description`, `Capabilities`, `ConnectionProperties`, `ConnectionOptions`,
`AuthenticationConfiguration`, `ComputeEnvironmentConfigurations` and
`PhysicalConnectionRequirements` from it. What remains is not reachable from the contract:
`Categories`, `DisplayName`, `LogoUrl` and `Vendor` are not members of
`DescribeConnectionTypeResponse` in the checked-in model (as this report itself notes), and the
rest would require transcribing one vendor connector's published catalogue text, which no rule
derives from the request, the model or stored state. A recording or model revision that states
that catalogue is what would reopen it.
