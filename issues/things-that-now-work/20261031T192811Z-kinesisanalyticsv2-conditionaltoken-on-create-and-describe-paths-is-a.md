Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-lifecycle kinesisanalyticsv2-application-version-history
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->2xx|$.ApplicationDetail.ConditionalToken
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->2xx|$.ApplicationDetail.ConditionalToken
Issue-Commit: 88218030aece2fbf10d6f0dd92e09bf5ad53454a

# ConditionalToken on create and describe paths is a constant non-conforming pseudo-UUID

> _Note: this issue was identified against the code base at commit `88218030aece2fbf10d6f0dd92e09bf5ad53454a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Shared fault: the ConditionalToken the emulator puts into every ApplicationDetail it serializes on the non-mutating paths (CreateApplication's response and DescribeApplication's response) is minted as a hyphenated 36-character pseudo-UUID that is NOT the opaque token shape AWS returns, and the same token characters are handed out for applications that are unrelated to each other. The token is this API's strong-concurrency handle, so it is not a cosmetic field. 2 causes.

Cause 1 of 2: CreateApplication returns 200 with $.ApplicationDetail.ConditionalToken in the wrong token shape. Fixture kinesisanalyticsv2-application-lifecycle, step task#2.
AWS: Smithy (com.amazonaws.kinesisanalyticsv2) CreateApplication output member ApplicationDetail -> ApplicationDetail is @required; ApplicationDetail declares member ConditionalToken -> shape ConditionalToken, type string, @length min 1 max 512, @pattern ^[a-zA-Z0-9-_+/=]+$, doc "A value you use to implement strong concurrency for application updates." The recording asserts this member by SHAPE, not by literal value (expected marker '<shaped-not-asserted>'), because the concrete characters are per-environment; what the recording does establish is the shape family - AWS issues an opaque token drawn from the modeled alphabet (letters, digits, '-', '_', '+', '/', '='), not a hyphenated 8-4-4-4-12 UUID, and it issues a token bound to the application version it describes.
Emulator: answers 200 (status matches) and all other members of the detail match, but ConditionalToken is emitted as a hyphenated 36-character UUID-layout string (observed 'eb33c0b0-26df-320a-d78d-446a715bcec3'), which the recording's shape assertion rejects. The value is additionally not a well-formed random UUID - its version nibble group ('320a') and variant group ('d78d') are not those of a v4 UUID - so it is a hash/derived constant wearing UUID punctuation rather than a freshly minted token. Hypothesis: the create handler formats some deterministic digest of the application name or request into UUID text and stores it as the token.
Why: a client that creates an application and keeps the returned ConditionalToken to drive a later conditional UpdateApplication is handed a value of the wrong shape; any client, SDK wrapper or proxy that validates the token against the modeled pattern family, or that round-trips it through a store expecting an opaque token rather than UUID text, mis-handles it. Because the value is derived rather than minted, it also cannot express "this is the token for this version", so the token-based optimistic-concurrency contract of the API is not reproduced at all on the create path.
Fix: belongs in the ApplicationDetail serializer used by the CreateApplication handler together with the application state's token minting - the state must mint an opaque ConditionalToken (modeled alphabet ^[a-zA-Z0-9-_+/=]+$, length 1..512) per application version at create time, and the serializer must emit that stored token, instead of formatting a derived UUID-shaped string.
Traces: kinesisanalyticsv2-application-lifecycle#task#2.

Cause 2 of 2: DescribeApplication returns 200 with the same wrong-shaped ConditionalToken, and the identical token characters are returned for two different applications. Fixture kinesisanalyticsv2-application-lifecycle, step task#3 (same cluster: kinesisanalyticsv2-application-version-history#task#3 and #task#9).
AWS: Smithy DescribeApplication output member ApplicationDetail -> ApplicationDetail is @required, carrying the same ConditionalToken member described above (string, @length 1..512, @pattern ^[a-zA-Z0-9-_+/=]+$). Recorded behavior: DescribeApplication echoes the token currently in force for the application version it is describing, asserted by shape only; distinct applications have distinct tokens, since a token is the concurrency handle of one application's current version.
Emulator: answers 200 but serializes ConditionalToken as the same hyphenated UUID-layout string the create path produced ('eb33c0b0-26df-320a-d78d-446a715bcec3'), failing the shape assertion. The very same characters are returned by the describe steps of a DIFFERENT application in a different fixture (kinesisanalyticsv2-application-version-history#task#3 and #task#9), so the emulator's token is not derived from, and does not distinguish, the application it describes - it is effectively a global constant.
Why: a client that reads the token via DescribeApplication before issuing a conditional update gets a value that is both the wrong shape and not unique to the application. Two clients operating on two different applications observe the same token, so a token value can never identify which application/version it guards; conditional-update conflict detection and the ConcurrentModificationException path it is supposed to trigger can never be exercised against this emulator.
Fix: belongs in the ApplicationDetail serializer shared by the DescribeApplication handler and in application state - the token must be read from the per-application, per-version stored token rather than from a shared/derived constant, and must be emitted in the modeled opaque-token shape.
Traces: kinesisanalyticsv2-application-lifecycle#task#3 (representative); also kinesisanalyticsv2-application-version-history#task#3 and kinesisanalyticsv2-application-version-history#task#9 (same cluster).

Note on overlap with open issues: 20261031T065833Z-kinesisanalyticsv2-updateapplication-response-omits-applicationmode-and-returns-a-non.md covers the ConditionalToken defect on the MUTATION path (UpdateApplication, where the token must additionally rotate per accepted update) and carries only that cluster's replay key (kinesisanalyticsv2-application-lifecycle#task#4); it does not own the CreateApplication or DescribeApplication clusters filed here. 20261029T173921Z-kinesisanalyticsv2-deleteapplication-skips-createtimestamp-deserialization-and-updateapplication-never-advances.md covers version-state advancement and CreateTimestamp deserialization, a different defect. Neither owns kinesisanalyticsv2-application-lifecycle#task#2 or #task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout. Summary: emulator_up true (/__admin/health answered 200, data listener accepts), 4 fixtures replayed, 55 steps total, 24 passed, 31 mismatched, 0 errored, 0 blocked, score 43.6%, 7 divergence clusters, all kinesisanalyticsv2. bootstrap_signal false.

Cause 1 - divergence key service=kinesisanalyticsv2|CreateApplication|status=200->200|keys=$.ApplicationDetail.ConditionalToken; verdict mismatch; expected_status 200, actual_status 200; cluster size 1; stub_path empty, owned_by empty.
Representative step kinesisanalyticsv2-application-lifecycle#task#2 (CreateApplication):
  $.ApplicationDetail.ConditionalToken: value_mismatch (expected '<shaped-not-asserted>', got 'eb33c0b0-26df-320a-d78d-446a715bcec3')

Cause 2 - divergence key service=kinesisanalyticsv2|DescribeApplication|status=200->200|keys=$.ApplicationDetail.ConditionalToken; verdict mismatch; expected_status 200, actual_status 200; cluster size 3; stub_path empty, owned_by empty.
Representative step kinesisanalyticsv2-application-lifecycle#task#3 (DescribeApplication):
  $.ApplicationDetail.ConditionalToken: value_mismatch (expected '<shaped-not-asserted>', got 'eb33c0b0-26df-320a-d78d-446a715bcec3')
Other affected steps in the same cluster: kinesisanalyticsv2-application-version-history#task#3, kinesisanalyticsv2-application-version-history#task#9 - a DIFFERENT application from the one in kinesisanalyticsv2-application-lifecycle, yet the identical token characters are returned, showing the token is a constant rather than per-application state.

Model evidence: smithy_query query=operation operation=CreateApplication,DescribeApplication (model services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json) -> both outputs are {ApplicationDetail (required) -> ApplicationDetail}. smithy_query query=shape shape=ApplicationDetail,ConditionalToken -> ApplicationDetail member ConditionalToken : ConditionalToken; ConditionalToken = string, @length {min 1, max 512}, @pattern ^[a-zA-Z0-9-_+/=]+$, doc "A value you use to implement strong concurrency for application updates."

Note: the concrete token characters quoted above are the recorded run's values and are NOT the requirement - the requirement is the opaque token shape family and that the token be per-application/per-version rather than a shared constant.

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it, and replay trace set simparity-wire-aws-v1 (no arguments; the corpus is bound).
2. Cause 1: replay fixture kinesisanalyticsv2-application-lifecycle through step task#2 - a CreateApplication for a FLINK-1_15 STREAMING application - and inspect $.ApplicationDetail.ConditionalToken in the 200 response. The emulator returns a hyphenated 36-character UUID-layout string; the recorded response carries an opaque token of the modeled shape, so the fixture's shape assertion fails.
3. Cause 2: continue the same fixture to step task#3 (DescribeApplication for that application) and inspect $.ApplicationDetail.ConditionalToken - the same wrong-shaped value is echoed.
4. Cross-check uniqueness: replay fixture kinesisanalyticsv2-application-version-history and inspect $.ApplicationDetail.ConditionalToken at its DescribeApplication steps task#3 and task#9. Although that fixture operates on a different application, the token characters are identical to those returned in step 2/3 above, demonstrating the token is a global constant rather than per-application, per-version state.
5. Confirm the model expectation with smithy_query query=shape shape=ConditionalToken against services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json.
