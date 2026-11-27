Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-playback-configuration-log-configuration
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-playback-configuration-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.LiveAdBuffetMode
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.VodAdBuffetMode
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# MediaTailor GetPlaybackConfiguration response omits the top-level ad buffet mode settings

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlaybackConfiguration

## Description
Cause 1 of 1: GetPlaybackConfiguration returns a 200 whose body is missing the top-level LiveAdBuffetMode and VodAdBuffetMode members of the playback configuration.

AWS: recorded behavior only — the on-disk Smithy model for this service (services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json) does not declare these members: GetPlaybackConfigurationResponse lists AdDecisionServerUrl, AvailSuppression, Bumper, CdnConfiguration, ConfigurationAliases, DashConfiguration, HlsConfiguration, InsertionMode, LivePreRollConfiguration, LogConfiguration, ManifestProcessingRules, Name, PersonalizationThresholdSeconds, PlaybackConfigurationArn, the playback/session endpoint prefixes, SlateAdUrl, Tags, TranscodeProfileName, VideoContentSourceUrl, AdConditioningConfiguration, AdDecisionServerConfiguration, YieldOptimizationConfiguration, FunctionMapping, AdsPersonalizationTimeouts, AdsPersonalizationConcurrency and BeaconingConfiguration, and no shape named LiveAdBuffetMode / VodAdBuffetMode / AdBuffetMode exists in the model. The recording, however, shows real MediaTailor answering GET /playbackConfiguration/{Name} with HTTP 200 and a JSON body that carries both members at the top level, each an ad-buffet-mode enum value (the recorded value for both is INSERT_ALL — that is the recorded configuration's value, not a required constant). So this is an API surface newer than the checked-in model: MediaTailor persists these two settings with the playback configuration and always reflects them back on the read path, the same way it always reflects BeaconingConfiguration/InsertionMode even for configurations created before the setting existed.

Emulator: the emulator answers the same request with HTTP 200 (status matches, no error involved) but both keys are absent from the body. Replay reports: "$.LiveAdBuffetMode: missing (expected 'INSERT_ALL', got None)" and "$.VodAdBuffetMode: missing (expected 'INSERT_ALL', got None)". Every other asserted member of that response, including the LogConfiguration block the fixture exercises, matches. Hypothesis: the emulator's playback-configuration state is keyed off the model's member list, so the two unmodeled settings are neither captured from the PutPlaybackConfiguration request that created the configuration nor emitted by the GetPlaybackConfiguration serializer.

Why: a client that reads a playback configuration to inspect or round-trip its ad buffet behavior sees the settings as unset. Code that reads the value to decide whether to insert all ads gets None instead of the configured mode and either falls back to a different behavior or raises on a missing key; a read-modify-write of the configuration (Get then Put) silently drops the two settings, so a tool that manages MediaTailor configurations through the emulator reports and reproduces a configuration that differs from what the service holds. Nothing in the response signals the omission, so the error surfaces far from this call.

Fix: the mediatailor service crate — the playback-configuration state must retain these two settings when PutPlaybackConfiguration supplies them (defaulting them for configurations that predate the setting), and the GetPlaybackConfiguration response serializer must emit both as top-level members alongside InsertionMode and BeaconingConfiguration. Because the members are absent from the checked-in Smithy model, the model slice for this service needs the two enum members added (or the handler must carry them out-of-band) before the serializer can emit them.

Traces: fixture mediatailor-playback-configuration-log-configuration, step task#3 (GetPlaybackConfiguration, expected 200, actual 200, two missing body keys).

## Evidence
Fixture mediatailor-playback-configuration-log-configuration, step task#3, operation GetPlaybackConfiguration.
Expected status 200, actual status 200 — the divergence is in the body.
Replay detail, verbatim:
  $.LiveAdBuffetMode: missing (expected 'INSERT_ALL', got None)
  $.VodAdBuffetMode: missing (expected 'INSERT_ALL', got None)
Expected: the 200 body contains top-level LiveAdBuffetMode and VodAdBuffetMode, each carrying the configuration's stored ad buffet mode (INSERT_ALL for the recorded configuration).
Actual: both keys are absent from the emulator's 200 body.
Divergence key: service=mediatailor|GetPlaybackConfiguration|status=200->200|keys=$.LiveAdBuffetMode,$.VodAdBuffetMode (cluster size 1, no stub_path, not owned by an open issue).
The on-disk Smithy model does not declare these members (smithy_query shape LiveAdBuffetMode / VodAdBuffetMode / AdBuffetMode -> not found; GetPlaybackConfigurationResponse member list does not include them), so the requirement is grounded in the recording.
The only other open mediatailor issue, "MediaTailor 204 empty-body responses omit the Content-Length header" (20261112T020916Z-mediatailor-204-empty-body-responses-omit-the-content.md), is about 204 framing on empty-body operations and does not cover this response body.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture mediatailor-playback-configuration-log-configuration.
2. The fixture creates a playback configuration (PutPlaybackConfiguration) whose ad buffet modes are set for both live and VOD, together with the LogConfiguration the fixture asserts on.
3. At step task#3 the fixture issues GET /playbackConfiguration/{Name} (GetPlaybackConfiguration) for that configuration name.
4. Inspect the 200 JSON body: the emulator omits the top-level LiveAdBuffetMode and VodAdBuffetMode keys, while the recorded AWS response returns both with the configuration's stored mode (INSERT_ALL here).
5. Equivalently with the CLI against the emulator: `aws mediatailor get-playback-configuration --name <name>` returns no LiveAdBuffetMode/VodAdBuffetMode fields.
