Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation
Issue-Key: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation|run#13|KeyError|allowed_out = allowed["trace"]["guardrail"].get("outputAssessments", {}).get(gid) or []
Issue-Commit: 32539e30dc69a7e797fcddb8b2bba51f1e444ebd

# Converse ignores guardrailConfig.trace='enabled': the 200 response carries no `trace` member at all, so no guardrail assessment is ever returned

> _Note: this issue was identified against the code base at commit `32539e30dc69a7e797fcddb8b2bba51f1e444ebd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE on Converse's guardrail trace serialization. (Smithy model read: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.)

Cause 1 of 1 - a Converse request that names a guardrail with trace='enabled' is answered 200 with NO `trace` member, so the guardrail assessment the caller asked for (input assessment, output assessments, suppressed modelOutput) is absent from the response entirely.

AWS: Smithy ConverseRequest member `guardrailConfig` -> GuardrailConfiguration{guardrailIdentifier, guardrailVersion, trace -> GuardrailTrace enum [enabled, disabled, enabled_full], default 'disabled'}. ConverseResponse has member `trace` -> ConverseTrace{guardrail -> GuardrailTraceAssessment, promptRouter -> PromptRouterTrace}. GuardrailTraceAssessment carries `modelOutput` (ModelOutputs list - "The output from the model"), `inputAssessment` (GuardrailAssessmentMap, keyed by guardrail id), `outputAssessments` (GuardrailAssessmentListMap, keyed by guardrail id) and `actionReason`. With trace='enabled' and a READY published guardrail version named on the request, the 200 response carries trace.guardrail, and it carries it for an ALLOWED turn too - the output side is assessed and reported even when nothing is detected, which is the only way a caller can tell an evaluated completion from an unevaluated one. When the output side does intervene, AWS reports stopReason='guardrail_intervened' (StopReason enum value), substitutes the guardrail's blockedOutputsMessaging, and trace.guardrail.modelOutput holds the completion that was suppressed while trace.guardrail.outputAssessments[<guardrailId>] names the matched finding (for a custom word: wordPolicy.customWords entry with match=<the configured word>, action='BLOCKED', detected=true).

Emulator: Converse(modelId=amazon.nova-lite-v1:0, guardrailConfig={'guardrailIdentifier': <id>, 'guardrailVersion': <published version>, 'trace': 'enabled'}) returns 200 whose body has keys without `trace` at all - reading response['trace'] raises KeyError: 'trace'. The guardrail was created and polled to status READY on the control plane and a version was published first, so the identifier was resolvable; stopReason came back 'end_turn'.

Why: every caller that audits guardrail behavior reads trace.guardrail - it is the only evidence in the response that the completion was evaluated at all, and the only place an output-side block reports what it suppressed. A client doing exactly that gets a KeyError / null-dereference against the emulator rather than an assessment, and output-side guardrail enforcement is unobservable, so an emulator run cannot distinguish "nothing was detected" from "nothing was evaluated".

Related but NOT a duplicate: open issue 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md Cause 1 reports that a guardrail which must intervene does not (stopReason stays 'end_turn'). This cause is the response-shape half that that issue does not cover: even on a turn where nothing needs to intervene, the modeled `trace` member is never serialized. Explicitly OUT of scope here: the words the model generates (the suite's later assertions on the model's own answer) - generated model text is not a requirement and is not part of this finding.

Fix: the Converse handler's guardrail evaluation stage plus the ConverseResponse serializer in the bedrock-runtime crate - populate `trace` -> ConverseTrace{guardrail: GuardrailTraceAssessment} whenever guardrailConfig.trace is 'enabled' or 'enabled_full', with inputAssessment/outputAssessments keyed by the guardrail id from the request and modelOutput filled on an output-side intervention.

Traces: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock__bedrock-runtime 0/4 passed. The emulator answered the call (HTTP 200 from Converse, and the control-plane CreateGuardrail / CreateGuardrailVersion / GetGuardrail calls all succeeded with status READY), so the service is up and answering wrongly.

bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation [error]:
  File "check_bedrock__bedrock_runtime_bedrock_runtime_guardrail_blocks_the_model_output_after_generation.py", line 152, in run
    allowed_out = allowed["trace"]["guardrail"].get("outputAssessments", {}).get(gid) or []
  KeyError: 'trace'

The failing request: Converse(modelId='amazon.nova-lite-v1:0', guardrailConfig={'guardrailIdentifier': <minted id>, 'guardrailVersion': <published version>, 'trace': 'enabled'}, messages=[{'role':'user','content':[{'text': <prompt>}]}], inferenceConfig={'maxTokens': 20, 'temperature': 0.0}) against a guardrail created with wordPolicyConfig={'wordsConfig': [{'text': 'four'}]}. The response's stopReason was 'end_turn' and its content was non-empty, so the body was a normal 200 - it simply has no `trace` key.

Model evidence: smithy_query operation Converse and shape ConverseTrace / GuardrailTraceAssessment / GuardrailConfiguration / GuardrailTrace on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json - ConverseResponse members {output, stopReason, usage, metrics, additionalModelResponseFields, trace -> ConverseTrace, performanceConfig, serviceTier}; ConverseTrace{guardrail -> GuardrailTraceAssessment, promptRouter}; GuardrailTraceAssessment{modelOutput, inputAssessment, outputAssessments, actionReason}; GuardrailConfiguration.trace -> GuardrailTrace [enabled, disabled, enabled_full] default 'disabled'.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the check id in trace_ids.
3. Control plane: CreateGuardrail(name=<unique>, blockedInputMessaging=<sentinel-in>, blockedOutputsMessaging=<sentinel-out>, wordPolicyConfig={'wordsConfig': [{'text': 'four'}]}) and poll GetGuardrail until status == 'READY'.
4. CreateGuardrailVersion(guardrailIdentifier=<id>) and poll GetGuardrail(guardrailIdentifier=<id>, guardrailVersion=<version>) until status == 'READY'.
5. Data plane: Converse(modelId='amazon.nova-lite-v1:0', guardrailConfig={'guardrailIdentifier': <id>, 'guardrailVersion': <version>, 'trace': 'enabled'}, messages=[{'role':'user','content':[{'text':'Reply with only the lowercase English word for the result of 2+3. No punctuation, no other words.'}]}], inferenceConfig={'maxTokens': 20, 'temperature': 0.0}).
6. Read response['trace']['guardrail']. Expected: a GuardrailTraceAssessment with outputAssessments keyed by the guardrail id (present even when nothing is detected). Observed: the response has no 'trace' key -> KeyError: 'trace'.
7. Repeat step 5 with a prompt whose answer IS the blocked word. Expected: stopReason='guardrail_intervened', content replaced by blockedOutputsMessaging, trace.guardrail.modelOutput holding the suppressed completion and trace.guardrail.outputAssessments[<id>] naming the word finding with action='BLOCKED', detected=true. Observed: no trace member at all (and stopReason 'end_turn' - see the related open issue for that half).
