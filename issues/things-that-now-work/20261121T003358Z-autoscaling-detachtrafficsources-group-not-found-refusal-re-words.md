Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-group-configuration-and-topology-negative
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-group-configuration-and-topology-negative autoscaling-group-dependent-operations-on-nonexistent-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DetachTrafficSources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 7b83df792075b2a128316f9b5135ddba9a81dab1

# autoscaling DetachTrafficSources group-not-found refusal re-words AWS's fixed message and interpolates the group name

> _Note: this issue was identified against the code base at commit `7b83df792075b2a128316f9b5135ddba9a81dab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachTrafficSources

## Description
Cause 1 of 1: DetachTrafficSources against an Auto Scaling group that does not exist returns the right HTTP status (400) and the right error Code, but Error.Message is emulator-authored prose that also interpolates the requested group name, where AWS emits a fixed sentence that carries no interpolated name at all.

AWS: recorded behavior only — the Smithy model (com.amazonaws.autoscaling#AutoScaling_2011_01_01, awsQuery protocol) declares DetachTrafficSources with required input members AutoScalingGroupName (XmlStringMaxLen255, @length 1..255) and TrafficSources (list of TrafficSourceIdentifier), and models only ResourceContentionFault (server, 500, awsQueryError code ResourceContention). The group-not-found refusal is therefore not modeled and the message is recorded behavior. The recording shows HTTP 400 with Error.Message 'AutoScalingGroup name not found - null'. Every character of that sentence is FIXED text for this refusal class on this operation: AWS does NOT substitute the requested AutoScalingGroupName into it — the trailing token is the literal four-letter word 'null'. There is no variable part taken from the request or from state.

Emulator: the same step answers HTTP 400 but with Error.Message 'AutoScalingGroup name not found - AutoScalingGroup <AutoScalingGroupName> not found' — the shared not-found template of the autoscaling crate, which appends a second re-framed clause and substitutes the group name from the request into it (the recorded run happens to carry sim2real-probe-autoscaling-missing-asg; that value is not part of the requirement, the interpolation itself is the divergence).

Why: a client that matches the refusal sentence to classify the failure (test fixtures, retry/remediation rules, log scrapers, SDK message assertions) does not recognize AWS's DetachTrafficSources refusal, because the emulator's sentence has extra text after the fixed prefix. Conversely, tooling written against the emulator learns to parse a group name out of a message that real AWS never puts one in, so it breaks the other way round against AWS.

Fix: the group-lookup refusal in the DetachTrafficSources handler in the autoscaling crate — it must emit this operation's fixed AWS sentence rather than formatting the crate's generic '<resource> not found' template with the requested name.

Related but not covering (named per instructions; neither carries this fixture step or this operation's message key): 20261117T053015Z-autoscaling-refusal-messages-and-validation-precedence-diverge-on.md and 20261117T053015Z-autoscaling-traffic-source-and-scheduled-action-state-diverges.md describe the traffic-source FAMILY (validation precedence and attach/detach state), and 20261116T221312Z-autoscaling-refusal-messages-diverge-from-aws-s-fixed.md covers the EnableMetricsCollection / LaunchInstances / DisableMetricsCollection messages. 20261108T093128Z-auto-scaling-detachtrafficsources-stub.md is an unimplemented-operation issue filed under the 'auto-scaling' service token; the autoscaling service DOES answer this operation here, with the wrong message.

Traces: autoscaling-group-configuration-and-topology-negative#task#14 (representative); the same cluster also covers autoscaling-group-dependent-operations-on-nonexistent-group#task#19.

## Evidence
Replay of trace set simparity-wire-aws-v1 (90 fixtures, 1066 steps, score 85.3%) reported this cluster with no owning issue and no blocking stub:

autoscaling DetachTrafficSources, status 400 -> 400, key $.ErrorResponse.Error.Message
  expected: 'AutoScalingGroup name not found - null'   (entirely fixed text; AWS interpolates nothing)
  actual:   'AutoScalingGroup name not found - AutoScalingGroup <group-name-from-request> not found'
  cluster (2 steps): autoscaling-group-configuration-and-topology-negative#task#14, autoscaling-group-dependent-operations-on-nonexistent-group#task#19

The HTTP status (400) and the error Code agree with the recording; only Error.Message diverges.
Model grounding: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json — DetachTrafficSources input requires AutoScalingGroupName + TrafficSources, output DetachTrafficSourcesResultType, errors: ResourceContentionFault only (no ValidationError), so the refusal wording is recorded behavior only.

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 against it.
3. Replay fixture autoscaling-group-configuration-and-topology-negative step task#14 — call autoscaling DetachTrafficSources with an AutoScalingGroupName that does not exist and one TrafficSources entry.
4. Compare $.ErrorResponse.Error.Message against the recorded 'AutoScalingGroup name not found - null': the emulator answers 'AutoScalingGroup name not found - AutoScalingGroup <that group name> not found'.
5. Re-check the other cluster member autoscaling-group-dependent-operations-on-nonexistent-group#task#19 before closing.
