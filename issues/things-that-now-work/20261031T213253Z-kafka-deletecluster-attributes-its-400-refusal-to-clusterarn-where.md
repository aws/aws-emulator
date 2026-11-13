Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-decommission-malformed-arn
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-decommission-malformed-arn
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DeleteCluster|4xx->4xx|$.invalidParameter
Issue-Commit: f5bcb0a946788170dc0fa0f931c1e9072e8c121c

# DeleteCluster attributes its 400 refusal to clusterArn where MSK names currentVersion

> _Note: this issue was identified against the code base at commit `f5bcb0a946788170dc0fa0f931c1e9072e8c121c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
ONE root cause: on MSK's cluster-decommission path the emulator attributes its BadRequestException to the cluster ARN it could not parse, where MSK attributes the same refusal to the currentVersion input. The HTTP status (400) and the Error.Message already agree with the recording on this step — the ONLY diverging field is the BadRequestException's invalidParameter member, i.e. which input the service blames.

Cause 1 of 1: DeleteCluster refused with HTTP 400 reports invalidParameter 'clusterArn' instead of 'currentVersion'.
AWS: Smithy kafka#DeleteCluster, DELETE /v1/clusters/{ClusterArn}; input members ClusterArn (@httpLabel, required) and CurrentVersion (@httpQuery 'currentVersion'); modeled errors BadRequestException(400), ForbiddenException(403), NotFoundException(404), InternalServerErrorException(500). Smithy kafka#BadRequestException has members InvalidParameter (jsonName 'invalidParameter', documented as "The parameter that caused the error") and Message (jsonName 'message'). Recorded behavior: when DeleteCluster is called with a clusterArn that is not a well-formed MSK cluster ARN together with a currentVersion, MSK answers HTTP 400 BadRequestException whose invalidParameter is the wire (jsonName) token 'currentVersion' — the version input is what MSK names as the cause, because the delete's version precondition cannot be satisfied against a cluster that does not resolve. The accompanying message is the one the emulator already produces on this step (it matched the recording and is not in divergence). Hypothesis: MSK evaluates the currentVersion precondition for the decommission before (or in place of) reporting the ARN label as the faulty input; the model states only the two members and the 400, so the ordering itself is recorded behavior only.
Emulator: HTTP 400 with $.invalidParameter == 'clusterArn' — its ARN-syntax check answers first and blames the path label.
Why: invalidParameter is the one machine-readable field in BadRequestException that tells a caller WHICH input to fix. A client (or an SDK wrapper, or a CLI that maps the token back to the parameter it sent) driving an idempotent decommission loop is told to correct the cluster ARN when MSK tells it to correct/refresh currentVersion, so its retry logic re-sends the same version and never converges; a test asserting MSK's attribution never matches.
Fix: DeleteCluster's validation in the kafka service crate — raise the BadRequestException from the currentVersion precondition check for this input combination and set invalidParameter to the wire token 'currentVersion', rather than returning the ARN-parser's refusal naming 'clusterArn'. The status and message on this path are already correct and must not change.
Traces: kafka-cluster-decommission-malformed-arn#task#2.

Related open issues that do NOT carry this fixture#step and do not cover this cause: 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md covers refusals that emit a PascalCase/dotted Smithy member path instead of the jsonName spelling — here the emulator already emits a correct lowercase wire token ('clusterArn'), it just names the wrong input; 20261030T211347Z-kafka-updatebrokercount-validates-targetnumberofbrokernodes-before-resolving-the-cluster-an.md and 20261030T011042Z-kafka-msk-create-and-update-paths-answer-the-wrong.md record the same ordering family on other operations (UpdateBrokerCount, CreateCluster/CreateReplicator/UpdateClusterConfiguration) but neither names DeleteCluster.

## Evidence
Corpus: simparity-wire-aws-v1 (49 fixtures replayed, 148 steps, score 66.9%). Cluster verdict=mismatch, divergence_key 'service=kafka|DeleteCluster|status=400->400|keys=$.invalidParameter', stub_path empty, owned_by empty (no open issue carries this replay key).

kafka-cluster-decommission-malformed-arn#task#2, DeleteCluster, status expected 400 / actual 400:
  $.invalidParameter: value_mismatch (expected 'currentVersion', got 'clusterArn')
No other field of the response diverged: the HTTP status, the x-amzn-errortype header and $.message all matched the recording, so this is purely a mis-attribution of the faulty input.

Smithy cited: kafka#DeleteCluster DELETE /v1/clusters/{ClusterArn}, ClusterArn @httpLabel required, CurrentVersion @httpQuery 'currentVersion', errors BadRequestException(400)/ForbiddenException(403)/NotFoundException(404)/InternalServerErrorException(500); kafka#BadRequestException members invalidParameter + message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay kafka-cluster-decommission-malformed-arn#task#2: DeleteCluster against a clusterArn that is not a well-formed MSK cluster ARN, with currentVersion supplied as a query parameter.
3. Expected: HTTP 400 BadRequestException with $.invalidParameter == 'currentVersion' and the message unchanged from what the step already matches.
4. Observed: HTTP 400 with $.invalidParameter == 'clusterArn'.
5. Re-replay kafka-cluster-decommission-malformed-arn#task#2 — it must pass, and task#1 of the same fixture (currently passing) must keep passing.
