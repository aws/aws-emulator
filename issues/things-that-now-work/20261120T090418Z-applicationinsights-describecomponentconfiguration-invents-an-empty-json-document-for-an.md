Issue-Source: trace-sweep-divergence
Issue-Trace: application-insights-component-configuration-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-insights-component-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: applicationinsights|DescribeComponentConfiguration|2xx->2xx|$.ComponentConfiguration
Issue-Commit: 2023ab28b75899ce9dcd52680a027d99b0db10e8

# DescribeComponentConfiguration invents an empty JSON document for an unconfigured component

> _Note: this issue was identified against the code base at commit `2023ab28b75899ce9dcd52680a027d99b0db10e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComponentConfiguration

## Description
One cause.

Cause 1 of 1: DescribeComponentConfiguration reports a synthetic ComponentConfiguration value for a component whose monitoring configuration has never been written, instead of leaving the optional member out of the response. Fixture application-insights-component-configuration-lifecycle, step task#6.

AWS: Smithy: DescribeComponentConfiguration -> DescribeComponentConfigurationResponse; member ComponentConfiguration targets ComponentConfiguration with @length(min 1, max 10000) and @pattern "^[\\S\\s]+$", and is NOT required; its documentation is "The configuration settings of the component. The value is the escaped JSON of the configuration." Because the member is optional and its target forbids a zero-length value, a component that carries no stored configuration has no value this member can take, so AWS answers 200 with Monitor and Tier present and ComponentConfiguration simply ABSENT from the response body. The recorded response for this step confirms that: the step's expected body carries no ComponentConfiguration value at all (replay reports expected ''). Recommended settings for an unconfigured component are a separate operation (DescribeComponentConfigurationRecommendation), not a value this one fabricates. Status is 200 in both the recording and the emulator; no error code or error message is involved in this divergence.

Emulator: answers 200 but always populates the member, substituting the two-character string "{}" when nothing has been stored for the component. Replay reports $.ComponentConfiguration: value_mismatch (expected '', got '{}'). The emulator's own source documents this as a local convention rather than modelled behavior: the handler sets component_configuration: Some(component.reported_configuration()) unconditionally, and reported_configuration() falls back to a hard-coded EMPTY_CONFIGURATION constant whenever the stored Option<String> is None.

Why: a client cannot distinguish "this component has never been configured" from "this component has an empty configuration". The idiomatic read — test whether the member is present/None before deciding to call UpdateComponentConfiguration or to fetch a recommendation — always sees a configuration, so onboarding flows skip the configuration step and components silently stay unmonitored. Clients that parse the member as escaped JSON and then branch on its contents also see a well-formed-but-empty document where AWS gave them nothing, and SDK deserializers that enforce the modelled @length(min 1) / @pattern constraints on output receive a value AWS would never emit for this state.

Fix: the DescribeComponentConfiguration handler and the state helper it calls. The output member must mirror the stored optional value — report the escaped JSON the last UpdateComponentConfiguration stored, and report NOTHING (omit the member) when no configuration has been stored — rather than defaulting an absent configuration to a placeholder document. Nothing else in the response changes: Monitor and Tier are already correct for this step.

Traces: fixture application-insights-component-configuration-lifecycle, step task#6 (cluster of 1; divergence_key service=applicationinsights|DescribeComponentConfiguration|status=200->200|keys=$.ComponentConfiguration). Relevant source: services/service-com-amazonaws-applicationinsights-ec2windowsbarleyservice/src/ops/describe_component_configuration.rs:206-213 (component_configuration: Some(component.reported_configuration())) and services/service-com-amazonaws-applicationinsights-ec2windowsbarleyservice/src/state.rs:384-393 (reported_configuration() / EMPTY_CONFIGURATION = "{}"). The one other open applicationinsights issue, "Ref on AWS::ApplicationInsights::Application returns a generated physical id instead of the application ARN", is a CloudFormation Ref defect and does not cover this response member.

## Evidence
Fixture application-insights-component-configuration-lifecycle, step task#6, operation DescribeComponentConfiguration (service applicationinsights), verdict mismatch.

Expected (recorded AWS response): HTTP 200, body with no ComponentConfiguration member (replay renders the expected value as '').
Actual (emulator): HTTP 200, body whose ComponentConfiguration member is the string "{}".
Replay detail: "$.ComponentConfiguration: value_mismatch (expected '', got '{}')".

Model evidence (smithy_query query=operation, operation=DescribeComponentConfiguration, model_path=services/service-com-amazonaws-applicationinsights-ec2windowsbarleyservice/smithy-model/com-amazonaws-applicationinsights-ec2windowsbarleyservice.json): output member ComponentConfiguration -> target ComponentConfiguration, length {min: 1, max: 10000}, pattern ^[\S\s]+$, not required; Monitor (boolean) and Tier (enum) also optional. Doc for the member: "The configuration settings of the component. The value is the escaped JSON of the configuration."

Emulator code evidence: ops/describe_component_configuration.rs always wraps the value in Some(...); state.rs defines pub const EMPTY_CONFIGURATION: &str = "{}" and Component::reported_configuration() returns it via unwrap_or_else when component_configuration is None, with a comment stating the member is "ALWAYS reported" and that the empty document is "this emulator's own convention, not a modelled value".

## Steps to Reproduce
1. Replay fixture application-insights-component-configuration-lifecycle against the emulator and stop at step task#6.
2. The step issues DescribeComponentConfiguration for a component of the fixture's application that no UpdateComponentConfiguration call has configured (its stored configuration is unset).
3. Compare the 200 response body against the recording: the recording has no ComponentConfiguration member; the emulator returns ComponentConfiguration = "{}".
4. Equivalently, by hand: CreateApplication over a resource group, let onboarding complete, then call DescribeComponentConfiguration for one discovered/created component without ever calling UpdateComponentConfiguration for it, and observe that the emulator reports a ComponentConfiguration value where AWS omits the member.
