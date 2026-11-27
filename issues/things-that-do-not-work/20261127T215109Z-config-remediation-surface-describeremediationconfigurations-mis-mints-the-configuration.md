Issue-Source: trace-sweep-divergence
Issue-Trace: config-start-remediation-execution-rule-and-type-lookup
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-start-remediation-execution-rule-and-type-lookup
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeRemediationConfigurations|2xx->2xx|$.RemediationConfigurations[*].Arn
Issue-Key: config|DescribeRemediationConfigurations|2xx->2xx|$.RemediationConfigurations[*].Parameters
Issue-Key: config|StartRemediationExecution|4xx->2xx|$.__type
Issue-Key: config|StartRemediationExecution|4xx->2xx|$.message
Issue-Key: config|StartRemediationExecution|4xx->2xx|status
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# config remediation surface: DescribeRemediationConfigurations mis-mints the configuration ARN and omits Parameters, and StartRemediationExecution never refuses an inapplicable resource type (2 causes)

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRemediationConfigurations

## Description
TWO net-new root causes in the Config (StarlingDoveService) emulator's remediation-configuration surface, both found by replaying the wire corpus simparity-wire-aws-v1 and neither blocked by a stub nor owned by an open issue. TWO CAUSES must be closed before submitting.

Cause 1 of 2 (fixture config-start-remediation-execution-rule-and-type-lookup, step task#5, DescribeRemediationConfigurations): the echoed remediation configuration carries an Arn whose resource-id segment is not of AWS's form, and it drops the Parameters member entirely.
AWS: Smithy — DescribeRemediationConfigurations(ConfigRuleNames required, 0..25) returns RemediationConfigurations: RemediationConfiguration, whose Arn member targets StringWithCharLimit1024 ('Amazon Resource Name (ARN) of remediation configuration') and whose Parameters member targets the RemediationParameters map (length min 0, max 25, 'An object of the RemediationParameterValue'). The operation models no errors. Recorded behavior for the ARN's form: the recorded response's Arn is arn:aws:config:<region>:<account>:remediation-configuration/<configRuleName>/<id> where <id> is a 36-character lowercase UUID in 8-4-4-4-12 hex form (the recording's own value is one instance of that form and is NOT the requirement). Recorded behavior for Parameters: a remediation configuration stored through PutRemediationConfigurations with no parameters is read back with Parameters present as an EMPTY MAP, not with the member absent.
Emulator: for the same stored configuration the replay reports $.RemediationConfigurations[0].Arn value_mismatch — the final ARN segment is a 16-character lowercase alphanumeric token (shape [a-z0-9]{16}) instead of a UUID — and $.RemediationConfigurations[0].Parameters missing (expected {}, got None), i.e. the member is omitted from the serialized body. Status is 200 in both cases; only the body diverges.
Why: a client that parses the remediation-configuration ARN (splitting the resource id, matching a UUID, or correlating the ARN with one it stored earlier) cannot parse or match the emulator's id form, and resource-tracking code that validates ARNs against the AWS pattern rejects it. Code that reads RemediationConfigurations[].Parameters unconditionally (iterating the map to render or diff the remediation's inputs) gets a null instead of an empty map and raises instead of showing 'no parameters'. The same id-form defect class is already filed for another Config id by 20261126T230015Z-config-startresourceevaluation-mints-a-resourceevaluationid-whose-form-the.md, which does NOT cover this ARN.
Fix: the remediation-configuration state and serializer of the Config service — the id minted when PutRemediationConfigurations stores a configuration must be a UUID in AWS's form so the derived Arn matches arn:aws:config:<region>:<account>:remediation-configuration/<rule>/<uuid>, and DescribeRemediationConfigurations' serializer must always emit Parameters (an empty map when none are stored) rather than omitting the member.
Traces: config-start-remediation-execution-rule-and-type-lookup#task#5.

Cause 2 of 2 (fixture config-start-remediation-execution-rule-and-type-lookup, step task#7, StartRemediationExecution; same divergence also at task#8 and task#9): StartRemediationExecution answers 200 for ResourceKeys whose resource type no remediation configuration applies to, instead of refusing it.
AWS: Smithy — StartRemediationExecution(ConfigRuleName required, ResourceKeys required, 1..100) models NoSuchRemediationConfigurationException (errorKind client, httpError 400, doc 'You specified an Config rule without a remediation configuration'), alongside InsufficientPermissionsException and InvalidParameterValueException. Recorded behavior: when the named rule's remediation configuration does not apply to the resource type named in ResourceKeys, the call is refused 400 with __type NoSuchRemediationConfigurationException and the message 'There is no applicable RemediationConfiguration for resource type(s) [AWS::SQS::Queue]'. Fixed text of that message is 'There is no applicable RemediationConfiguration for resource type(s) [' ... ']'; the only variable part is the list of resource types taken from the request's ResourceKeys (here AWS::SQS::Queue) — nothing in it comes from a per-run value.
Emulator: the request is accepted — status 200 with a success body, so $.__type and $.message are both absent (replay: 'status: expected 400, got 200; $.__type: missing (expected NoSuchRemediationConfigurationException); $.message: missing'). The emulator starts (or claims to start) a remediation for a resource type the rule has no remediation configuration for.
Why: a client that probes applicability by calling StartRemediationExecution and branching on NoSuchRemediationConfigurationException never takes that branch; it believes a remediation was queued for a resource type AWS would never remediate, and then waits for an execution status that will never describe real work. Guardrail and negative tests asserting the 400/NoSuchRemediationConfigurationException pair fail.
Fix: the pre-flight validation in the operation's handler — StartRemediationExecution must resolve the named ConfigRuleName's stored remediation configuration, intersect its ResourceType/scope with the resource types in ResourceKeys, and when no configuration applies refuse 400 NoSuchRemediationConfigurationException with the message above, formatting the bracketed list from the request's resource types.
Traces: config-start-remediation-execution-rule-and-type-lookup#task#7, #task#8, #task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 57 fixtures replayed, 299 steps, 282 passed, 17 mismatched, score 94.3%. The two clusters below carry empty stub_path and empty owned_by.

Cause 1 — [mismatch] fixture config-start-remediation-execution-rule-and-type-lookup, step task#5, operation DescribeRemediationConfigurations, expected_status 200, actual_status 200:
  $.RemediationConfigurations[0].Arn: value_mismatch (expected 'arn:aws:config:us-west-2:000000000000:remediation-configuration/sim2real-probe-config-remrule-605396e7/273ed28c-ce70-4714-bad9-112ff618ad69', got 'arn:aws:config:us-west-2:000000000000:remediation-configuration/sim2real-probe-config-remrule-605396e7/b8cpdrceyvdck3yb')
  $.RemediationConfigurations[0].Parameters: missing (expected {}, got None)
  divergence_key service=config|DescribeRemediationConfigurations|status=200->200|keys=$.RemediationConfigurations[0].Arn,$.RemediationConfigurations[0].Parameters. The UUID in the expected ARN is one recorded instance; the requirement is its 8-4-4-4-12 hex form, against the emulator's 16-char [a-z0-9] token.

Cause 2 — [mismatch] fixture config-start-remediation-execution-rule-and-type-lookup, step task#7, operation StartRemediationExecution, expected_status 400, actual_status 200 (cluster also covers task#8 and task#9):
  status: expected 400, got 200
  $.__type: missing (expected 'NoSuchRemediationConfigurationException', got None)
  $.message: missing (expected 'There is no applicable RemediationConfiguration for resource type(s) [AWS::SQS::Queue]', got None)
  divergence_key service=config|StartRemediationExecution|status=400->200|keys=$.__type,$.message,status. Model: NoSuchRemediationConfigurationException errorKind client, httpError 400 (defaulted).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. replay_wire_corpus() and re-replay fixture config-start-remediation-execution-rule-and-type-lookup (steps task#5, task#7, task#8, task#9).
3. Cause 1: `aws configservice put-config-rule` for a rule, then `aws configservice put-remediation-configurations` with TargetType=SSM_DOCUMENT and no Parameters; call `aws configservice describe-remediation-configurations --config-rule-names <rule>` — the returned Arn's last segment is a 16-character alphanumeric token instead of a UUID, and the Parameters member is absent from the body where AWS returns an empty map.
4. Cause 2: with that rule's remediation configuration applying to a resource type other than AWS::SQS::Queue, call `aws configservice start-remediation-execution --config-rule-name <rule> --resource-keys resourceType=AWS::SQS::Queue,resourceId=<queue>` — the emulator answers 200 instead of 400 NoSuchRemediationConfigurationException with message 'There is no applicable RemediationConfiguration for resource type(s) [AWS::SQS::Queue]'.
