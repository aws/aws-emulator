Issue-Source: trace-sweep-divergence
Issue-Trace: cwp-delete-anomaly-detector-describe-empty
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwp-delete-anomaly-detector-describe-empty cwa-range-operator-threshold-metric-id
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutAnomalyDetector|2xx->2xx|$.AnomalyDetectorId
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].AnomalyDetectorId
Issue-Commit: 46b30d139c4fe1f2d788240b5195cc8dbe513ba9

# monitoring: AnomalyDetectorId is minted in the wrong shape class on PutAnomalyDetector and DescribeAnomalyDetectors

> _Note: this issue was identified against the code base at commit `46b30d139c4fe1f2d788240b5195cc8dbe513ba9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAnomalyDetector

## Description
This issue bundles 2 net-new root causes in the cloudwatch (monitoring, 2010-08-01) emulator, both produced by the single identifier minter the anomaly-detector writer and reader share. Both must be closed before submitting.

Cause 1 of 2: PutAnomalyDetector reports an AnomalyDetectorId whose shape class is not the one CloudWatch mints.
AWS: Smithy (cloudwatch 2010-08-01) models PutAnomalyDetectorOutput$AnomalyDetectorId -> AnomalyDetectorId, a string with @length min 1 max 128 and @pattern ^[A-Za-z0-9_./:%()+-]+$, documented "The unique identifier of the anomaly detector that you created or updated." The model fixes only length and character set, so the concrete form is recorded behavior only: the recorded PutAnomalyDetector responses carry an opaque service-minted identifier whose shape class the corpus asserts (the value itself is volatile and recorded as '<shaped-not-asserted>'), and the emulator's value does not belong to that class. Hypothesis: AWS mints the identifier in hyphen-separated uuid form (8-4-4-4-12 lowercase hex), i.e. a token containing '-' separators, whereas the emulator mints an unseparated run of hex. What is a requirement here is the SHAPE of the minted identifier (its separator structure and length class), never a particular recorded value: every id differs per run and per environment.
Emulator: PutAnomalyDetector returns HTTP 200 with AnomalyDetectorId as a bare 32-character lowercase hexadecimal run containing no separator (observed form, e.g. a value like 'e0ecd47dd22910c1e381275f7ef60c99'). That satisfies the modelled pattern and length but is a different shape class from the identifier the recordings carry, so the replay's shaped comparison rejects it.
Why: a client that validates, canonicalises or re-parses the identifier it got back - for example one that asserts the uuid form before interpolating the id into a path, an ARN or a dashboard document, or one that keys local records on a parsed uuid - rejects or mis-handles an identifier AWS would have accepted. The id is opaque, so nothing else in the response signals the difference; the breakage appears only in the client's own parsing layer.
Fix: the anomaly-detector identifier minter the write path uses (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state/anomaly_detector_reads.rs, minted_anomaly_detector_id, cited under Traces) - render the minted bytes in CloudWatch's identifier shape instead of bare hex. The uniqueness/opacity properties the current doc comment pins (per-run bytes, stable for the life of the model, unrelated to the request) are correct and must be preserved; only the rendering changes.
Traces: cwp-delete-anomaly-detector-describe-empty#task#1 (representative); source: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_anomaly_detector.rs:226-245 and src/state/anomaly_detector_reads.rs:191-194.

Cause 2 of 2: DescribeAnomalyDetectors reports the same wrongly-shaped identifier for every stored model.
AWS: Smithy models DescribeAnomalyDetectorsOutput$AnomalyDetectors -> AnomalyDetectors, a list of AnomalyDetector, whose AnomalyDetectorId member targets the same AnomalyDetectorId shape (@length 1..128, @pattern ^[A-Za-z0-9_./:%()+-]+$) and is documented "The unique identifier of the anomaly detector." The read must report back the identifier the creating write minted, in the shape CloudWatch mints it in; the corpus records it as '<shaped-not-asserted>' and asserts its shape class.
Emulator: DescribeAnomalyDetectors answers HTTP 200 and the correct detector set, but $.AnomalyDetectors[0].AnomalyDetectorId is the same bare unseparated 32-character lowercase hex run (observed form, e.g. a value like 'b66c245c754d006dea8de057bd4ddaac'), so the read diverges in the same shape class as the write. This is a separate wire surface from Cause 1 (a different operation and a different serializer path), and both are reached from the one minter, so a fix applied only to the write path leaves this read diverging for models already stored.
Why: a client that lists detectors and then feeds an id back into DeleteAnomalyDetector, or validates the listed ids before storing them, gets identifiers AWS would never have returned; code that round-trips list -> validate -> delete fails against the emulator while passing against AWS.
Fix: the shared anomaly-detector read projection / the identifier minter it reports from (src/state/anomaly_detector_reads.rs, StoredAnomalyDetector::reported and minted_anomaly_detector_id) - once the minter renders CloudWatch's shape, the read must carry it through unchanged rather than re-deriving anything.
Traces: cwa-range-operator-threshold-metric-id#task#7 (representative), cwp-delete-anomaly-detector-describe-empty#task#2.

Related but NOT covering these: the open issue 20261117T082319Z-cloudwatch-7-net-new-causes-putmetricalarm-accepts-threshold.md bundles anomaly-detector refusal-message and InvalidNextToken causes for the same operations, but it is a behavioral issue and carries none of these fixtures' replay keys; neither of the two clusters above is owned by any open issue.

## Evidence
Cause 1 - PutAnomalyDetector, fixture cwp-delete-anomaly-detector-describe-empty step task#1: expected HTTP 200 with $.AnomalyDetectorId matching the recorded identifier's shape class (recorded as '<shaped-not-asserted>'); actual HTTP 200 with $.AnomalyDetectorId = 'e0ecd47dd22910c1e381275f7ef60c99' - a 32-char unseparated lowercase hex run. Replay verdict: mismatch, divergence_key service=monitoring|PutAnomalyDetector|status=200->200|keys=$.AnomalyDetectorId. Status and every other member of the response matched.

Cause 2 - DescribeAnomalyDetectors, fixture cwa-range-operator-threshold-metric-id step task#7 (also cwp-delete-anomaly-detector-describe-empty#task#2): expected HTTP 200 with $.AnomalyDetectors[0].AnomalyDetectorId in the recorded shape class ('<shaped-not-asserted>'); actual 'b66c245c754d006dea8de057bd4ddaac'. Replay verdict: mismatch, divergence_key service=monitoring|DescribeAnomalyDetectors|status=200->200|keys=$.AnomalyDetectors[0].AnomalyDetectorId. The detector set, filters and all other members matched.

The specific hex values above are per-run outputs of the emulator's minter and are quoted only to show the FORM (32 hex characters, no separator); the requirement is the identifier's shape class, not any literal value. Source of the form: src/state/anomaly_detector_reads.rs:193 `::hex::encode(super::alarm_reads::minted_bytes())`.

## Steps to Reproduce
1. PutAnomalyDetector(SingleMetricAnomalyDetector={Namespace:<ns>, MetricName:<m>, Stat:'Average'}) and inspect $.AnomalyDetectorId in the 200 response: it is 32 hexadecimal characters with no separator, instead of the separator-bearing identifier shape CloudWatch mints (reproduces Cause 1; fixture cwp-delete-anomaly-detector-describe-empty step task#1).
2. DescribeAnomalyDetectors(Namespace=<ns>) and inspect $.AnomalyDetectors[0].AnomalyDetectorId: same wrong shape class (reproduces Cause 2; fixture cwa-range-operator-threshold-metric-id step task#7, and cwp-delete-anomaly-detector-describe-empty step task#2).
3. Re-replay both fixtures to confirm: cwp-delete-anomaly-detector-describe-empty#task#1 and cwa-range-operator-threshold-metric-id#task#7.
