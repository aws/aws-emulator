Issue-Source: trace-sweep-divergence
Issue-Trace: cwa-range-operator-threshold-metric-id
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwa-range-operator-threshold-metric-id cwp-delete-anomaly-detector-describe-empty
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].AnomalyDetectorId
Issue-Key: monitoring|PutAnomalyDetector|2xx->2xx|$.AnomalyDetectorId
Issue-Commit: e5bd3f449037d977df62b379b9bbd75575215842

# cloudwatch AnomalyDetectorId is served as an opaque 32-char hex digest, not the identifier shape CloudWatch issues

> _Note: this issue was identified against the code base at commit `e5bd3f449037d977df62b379b9bbd75575215842`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAnomalyDetectors

## Description
This issue bundles 2 net-new root causes, both in the cloudwatch (monitoring, 2010-08-01) emulator and both the same underlying fault surfacing on the two operations that carry an anomaly detector's identifier on the wire. Each is numbered below with its own fixture and step. Both must be closed before submitting.

Cause 1 of 2: DescribeAnomalyDetectors reports each detector's AnomalyDetectorId as a 32-character lowercase hex digest, which is not the identifier shape CloudWatch serves.
AWS: Smithy (cloudwatch 2010-08-01) models DescribeAnomalyDetectors output AnomalyDetectors -> AnomalyDetector$AnomalyDetectorId as target AnomalyDetectorId with @length min 1 max 128 and pattern ^[A-Za-z0-9_./:%()+-]+$, documented "The unique identifier of the anomaly detector." The model therefore does not fix the id's internal form, so the required form is recorded behavior only: the fixture asserts this member by SHAPE rather than by literal value (expected '<shaped-not-asserted>'), and the emulator's value fails that shape assertion — so whatever CloudWatch records here, a bare 32-char hex digest is not of that shape. Note what the modelled pattern does permit beyond hex: '/', ':', '%', '(', ')', '+', '.', '_', '-' and upper-case letters. Hypothesis: CloudWatch derives the id from the detector's own identification (its namespace, metric name, statistic and dimensions, percent-encoded), which is what those extra allowances in the pattern exist for; the digest the emulator emits discards all of it. Independent of the exact form, two properties are requirements and must hold: the id obeys the modelled length and pattern, and it is STABLE for a given detector — the value PutAnomalyDetector returned is the value DescribeAnomalyDetectors reports for the same detector, and it is accepted back as a DescribeAnomalyDetectors AnomalyDetectorIds filter and by DeleteAnomalyDetector.
Emulator: HTTP 200 with the right body structure, but $.AnomalyDetectors[0].AnomalyDetectorId is a 32-character lowercase hex string (the replayed value was '8167b007131a53acdf57fcabf254d398'; that literal is one run's digest, not part of the requirement), which does not match the shape the recording carries.
Why: the id is the handle a client uses to name one detector among many. A client that reads it from DescribeAnomalyDetectors and feeds it back as an AnomalyDetectorIds filter, stores it in configuration, or correlates it with the id PutAnomalyDetector returned, is working with a token of the wrong form; anything that validates the identifier against CloudWatch's documented shape, or reconstructs the detector's metric from it, breaks against the emulator while passing against AWS.
Fix: the anomaly-detector identifier derivation in the cloudwatch anomaly-detector state (the function that mints the id when a detector is stored) plus the DescribeAnomalyDetectors serializer that reads it — not a change to the response structure, which is otherwise correct.
Traces: cwa-range-operator-threshold-metric-id#task#7 (representative), plus cwp-delete-anomaly-detector-describe-empty#task#2 in the same cluster.

Cause 2 of 2: PutAnomalyDetector returns the same wrongly shaped AnomalyDetectorId in its own response.
AWS: Smithy models PutAnomalyDetector output as a single member AnomalyDetectorId (target AnomalyDetectorId, @length min 1 max 128, pattern ^[A-Za-z0-9_./:%()+-]+$). The id's internal form is again recorded behavior only: the fixture shape-asserts this member (expected '<shaped-not-asserted>') and the emulator's value fails that assertion. The requirement is that the create response hands back an identifier of CloudWatch's form, and that the very same value is what the detector is afterwards known by on DescribeAnomalyDetectors and DeleteAnomalyDetector.
Emulator: HTTP 200 with $.AnomalyDetectorId set to a 32-character lowercase hex digest (the replayed value was '041bf56d1cc83235171e9257905f483e' — again one run's digest, not a requirement), diverging from the recorded shape in exactly the way Cause 1 does on the read path. Because both the write and the read path are affected, this is one id-derivation defect observed at both ends and not two unrelated serializer bugs.
Why: a client's first sight of a detector's identifier is this response, and it is the value it will persist and later use to describe or delete the detector. Handing back a token of the wrong form propagates the divergence into the caller's own state, so every later call made with the stored id is made with a value AWS would never have issued.
Fix: the same identifier derivation, at its call site in PutAnomalyDetector's handler/state write — mint one id of the modelled form per detector and return exactly the value the read path will report.
Traces: cwp-delete-anomaly-detector-describe-empty#task#1.

Neither cause is covered by the open cloudwatch anomaly-detector issue 20261117T082319Z-cloudwatch-7-net-new-causes-putmetricalarm-accepts-threshold.md: that bundle concerns PutAnomalyDetector/DeleteAnomalyDetector refusal MESSAGES (the InvalidParameterCombination source ordering) and the DescribeAnomalyDetectors InvalidNextToken text, all on 400 paths, and carries none of these fixture steps' replay keys. These two causes are on the 200 success path and concern the identifier value itself.

## Evidence
Cause 1 — fixture cwa-range-operator-threshold-metric-id, step task#7, operation DescribeAnomalyDetectors, verdict mismatch, status expected 200 / actual 200. Divergence detail: "$.AnomalyDetectors[0].AnomalyDetectorId: value_mismatch (expected '<shaped-not-asserted>', got '8167b007131a53acdf57fcabf254d398')". divergence_key service=monitoring|DescribeAnomalyDetectors|status=200->200|keys=$.AnomalyDetectors[0].AnomalyDetectorId. Cluster also contains cwp-delete-anomaly-detector-describe-empty#task#2 (affected_count 2). Expected: an AnomalyDetectorId of the shape the recording carries, within @length 1..128 and matching ^[A-Za-z0-9_./:%()+-]+$, equal to the value PutAnomalyDetector returned for that detector. Actual: a 32-char lowercase hex digest that fails the recorded shape assertion. No other member of the response diverged.

Cause 2 — fixture cwp-delete-anomaly-detector-describe-empty, step task#1, operation PutAnomalyDetector, verdict mismatch, status expected 200 / actual 200. Divergence detail: "$.AnomalyDetectorId: value_mismatch (expected '<shaped-not-asserted>', got '041bf56d1cc83235171e9257905f483e')". divergence_key service=monitoring|PutAnomalyDetector|status=200->200|keys=$.AnomalyDetectorId. affected_count 1. Expected: an identifier of CloudWatch's recorded shape, which is the value the detector is subsequently known by. Actual: a 32-char lowercase hex digest, same form as Cause 1. The hex literals quoted here are per-run generated values and are NOT requirements; the requirement is the identifier's shape, its conformance to the modelled length/pattern, and its stability across PutAnomalyDetector, DescribeAnomalyDetectors and DeleteAnomalyDetector.

## Steps to Reproduce
Cause 2 then Cause 1 (the write path mints the id the read path reports):
1. Call PutAnomalyDetector with a single-metric detector, e.g. SingleMetricAnomalyDetector={Namespace:<ns>, MetricName:<metric>, Stat:'Average'}, against the emulator's cloudwatch (monitoring) endpoint. Read $.AnomalyDetectorId from the 200 response: it comes back as a 32-character lowercase hex digest rather than an identifier of CloudWatch's form. (Replays as fixture cwp-delete-anomaly-detector-describe-empty, step task#1.)
2. Call DescribeAnomalyDetectors with no filters (or Namespace=<ns>) and read $.AnomalyDetectors[0].AnomalyDetectorId: the same wrongly shaped hex form appears on the read path. (Replays as fixture cwa-range-operator-threshold-metric-id, step task#7, and cwp-delete-anomaly-detector-describe-empty, step task#2.)
3. Check the two invariants that must survive the fix: the id returned in step 1 is byte-identical to the one reported in step 2 for the same detector, and passing it back as DescribeAnomalyDetectors(AnomalyDetectorIds=[<id>]) returns exactly that detector; it must also satisfy @length 1..128 and pattern ^[A-Za-z0-9_./:%()+-]+$.
4. Re-replay both fixture steps above to confirm the shape assertion on AnomalyDetectorId passes.
