Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-export-name-cannot-depend-on-resource
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-export-name-cannot-depend-on-resource cloudformation-mapping-value-must-be-literal
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|2xx->4xx|$.CreateStackResponse
Issue-Key: cloudformation|CreateStack|2xx->4xx|status
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.LastOperations.member.OperationId
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs.member.ExportName
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.StackId
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs.member.OutputValue
Issue-Related: 20261029T171940Z-cloudformation-change-set-previews-operation-events-and-the.md
Issue-Commit: 9e2ba0678642d895992d831c5d4d97ae7316945d

# CreateStack after a refused template cannot reuse the stack name, and stack Outputs lose the declared ExportName and the Fn::FindInMap OutputValue

> _Note: this issue was identified against the code base at commit `9e2ba0678642d895992d831c5d4d97ae7316945d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new causes on the CreateStack / DescribeStacks path, all exercised by the "corrected template after a refused template" fixtures. A fixer must close all three.

Cause 1 of 3 — CreateStack refuses a valid template that reuses the stack name of a create AWS never performed (fixture cloudformation-export-name-cannot-depend-on-resource, step task#3).
AWS: Smithy: CreateStack input StackName (required), TemplateBody; output CreateStackOutput.StackId (+ OperationId); modeled errors AlreadyExistsException(400), InsufficientCapabilitiesException(400), LimitExceededException(400), TokenAlreadyExistsException(400) plus the service-common query ValidationError. When a template is malformed AWS refuses the CreateStack with 400 ValidationError and creates NO stack at all — the recording confirms this: the preceding step of each fixture expects 400 ValidationError ("Template format error: ...") and the step after it expects DescribeStacks to answer 400 ValidationError "Stack with id <stack-name> does not exist". Because no stack exists, a following CreateStack that passes the SAME StackName with a corrected template is accepted and returns 200 with CreateStackResult.StackId (an arn:aws:cloudformation:<region>:<account>:stack/<stack-name>/<uuid>).
Emulator: the corrected CreateStack answers HTTP 400 and serializes no CreateStackResponse body at all.
Hypothesis: the emulator accepted the earlier malformed template with 200 (that refusal gap is already open as 20261103T171935Z-cloudformation-never-validates-a-stack-policy-so-invalid.md), so the stack name is still occupied and this create collides — i.e. the name-collision refusal is correct behaviour applied to state AWS would never have. The net-new divergence filed here is the 400 on the create that AWS answers 200; it is NOT covered by that open issue, whose replay key is the earlier step.
Why: a client that handles AWS's template refusal by fixing the template and re-creating under the same name is permanently blocked — the second create never succeeds, so every later step of the workflow (describe, outputs, update, delete) operates on the wrong stack or on none.
Fix: the CreateStack handler's template-validation ordering and the stack-state write — a create refused for template format must leave no stack record behind, and the name must stay free.
Traces: cloudformation-export-name-cannot-depend-on-resource#task#3, cloudformation-mapping-value-must-be-literal#task#3.

Cause 2 of 3 — a stack Output's ExportName is a generated physical name instead of the template's declared Export.Name (fixture cloudformation-export-name-cannot-depend-on-resource, step task#4).
AWS: Smithy: DescribeStacks output Stacks[].Outputs[] is Output{OutputKey, OutputValue, Description, ExportName}; ExportName is documented as "The name of the export associated with the output" — that is, the value the template's Outputs.<key>.Export.Name resolves to (intrinsics such as Fn::Sub/Fn::Join resolved against the stack's own pseudo-parameters, e.g. "${AWS::StackName}-export" becoming "<stack-name>-export"). The recorded response carries exactly that resolved form: the fixed suffix declared by the template appended to the stack name.
Emulator: ExportName is reported as a minted, physical-resource-style token of the shape "<stack-name>-<short-logical-id>-<uppercase-hex>" (recorded instance: expected 'sim2real-probe-cfn-exportdep-7a9a623d-export', got 'sim2real-probe-cfn-exportdep-7a9a623d-Q-F963754DF0CD'); the stack-name prefix is the only part that matches, the declared export name is dropped and replaced by the emulator's name generator.
Why: cross-stack references break. A consumer stack resolving Fn::ImportValue on the documented export name, or a caller filtering ListExports/ListImports by it, finds no such export, because the only export the emulator published is an unpredictable generated string.
Fix: the DescribeStacks output serializer / the stack-output resolution in state — populate ExportName from the resolved template Outputs.<key>.Export.Name, never from the physical-name generator.
Traces: cloudformation-export-name-cannot-depend-on-resource#task#4.

Cause 3 of 3 — an Output whose value comes from Fn::FindInMap is serialized with no OutputValue (fixture cloudformation-mapping-value-must-be-literal, step task#4).
AWS: Smithy: Output.OutputValue is "The value associated with the output". A settled stack whose Outputs entry resolves an intrinsic (here a Fn::FindInMap lookup into the template's Mappings section) reports that resolved literal as OutputValue; the recording shows the mapping's literal value present in the member.
Emulator: the Outputs member is serialized with OutputKey but OutputValue missing entirely (expected the resolved mapping literal, got None). The same step also reports Parameters[].ParameterValue as the value supplied to the earlier, wrongly-accepted create rather than the corrected one — a consequence of Cause 1, not a separate requirement; the StackId/OperationId differences in that step are unresolved harness placeholders from the failed create and per-run values, and are not part of this finding.
Why: a caller reading a declared stack output — the documented way to consume a stack's results — gets an entry with a key and no value, so any downstream wiring that depends on it (Fn::ImportValue, a deploy script reading Outputs) silently receives nothing instead of the mapped literal.
Fix: the stack-output resolution step for intrinsic functions (Fn::FindInMap over the Mappings section) and the DescribeStacks serializer that omits a member when resolution yielded no value. Related open issues cover OutputValue omission for other resource sets — 20261122T232606Z-cloudformation-docdb-dynamodb-ecs-elbv2-firehose-lambda-rds-f1eb40.md and 20261124T033023Z-cloudformation-ec2-vpc-lattice-describestacks-serializes-a-create.md — neither carries this fixture's replay key, so this cause is filed separately.
Traces: cloudformation-mapping-value-must-be-literal#task#4.

## Evidence
Cause 1 (CreateStack, cloudformation-export-name-cannot-depend-on-resource#task#3; also cloudformation-mapping-value-must-be-literal#task#3):
  expected status 200 with $.CreateStackResponse.CreateStackResult.StackId = arn:aws:cloudformation:<region>:<account>:stack/<stack-name>/<uuid> (+ OperationId)
  actual   status 400, $.CreateStackResponse missing (no body serialized)

Cause 2 (DescribeStacks, cloudformation-export-name-cannot-depend-on-resource#task#4):
  $.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs.member.ExportName
  expected 'sim2real-probe-cfn-exportdep-7a9a623d-export'  (= <stack-name> + the template's declared '-export' suffix; the stack name is a per-run value, the suffix is what the template declares)
  actual   'sim2real-probe-cfn-exportdep-7a9a623d-Q-F963754DF0CD'  (generated physical-name token)

Cause 3 (DescribeStacks, cloudformation-mapping-value-must-be-literal#task#4):
  $.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs.member.OutputValue
  expected the Fn::FindInMap-resolved literal (recorded instance: 'resolved-literal')
  actual   member absent (None)
  same step, consequence of Cause 1: Parameters.member.ParameterValue expected 'k', got 'x'

## Steps to Reproduce
1. Replay fixture cloudformation-export-name-cannot-depend-on-resource:
   - task#1 CreateStack with a template whose Outputs Export.Name references a resource -> AWS 400 ValidationError (already-open issue).
   - task#3 CreateStack with the corrected template under the SAME StackName -> AWS 200 + StackId; emulator answers 400 with no body (Cause 1).
   - task#4 DescribeStacks on that stack -> compare Stacks[].Outputs[].ExportName against the template's declared export name (Cause 2).
2. Replay fixture cloudformation-mapping-value-must-be-literal:
   - task#3 CreateStack with the corrected template under the same StackName -> emulator 400 (Cause 1).
   - task#4 DescribeStacks -> Stacks[].Outputs[] is serialized without OutputValue for the Fn::FindInMap-derived output (Cause 3).
