Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-create-unsupported-kafka-version-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-create-unsupported-kafka-version-rejected|run#6|AssertionError|assert message == _UNSUPPORTED_VERSION_MESSAGE, message
Issue-Commit: ce5d737b221088734079d6fd5fe0c37b5d786af6

# CreateClusterV2's unsupported-Kafka-version refusal advertises a valid-value list wider than MSK's standard-broker new-cluster offering (adds 3.6.0, 3.7.x, 3.7.x.kraft)

> _Note: this issue was identified against the code base at commit `ce5d737b221088734079d6fd5fe0c37b5d786af6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
One net-new root cause, grounded in the single failing behavioral check listed in trace_ids. That check is not marked ALREADY FILED and not marked OWNED by the suite report, and no open kafka issue carries its replay key.

Cause 1 of 1: the "<category> instance types are not supported for Kafka version <v>. Valid values: [...]" refusal of a provisioned CreateClusterV2 renders its valid-value list by DERIVING it from the version catalog (every row whose Status is ACTIVE, minus a hard-coded Express-only line), so the sentence advertises three Apache Kafka lines that Amazon MSK does not offer a NEW standard-broker cluster. The error class and HTTP status are right; the message's variable list is wrong. Failing check: kafka.cluster-create-unsupported-kafka-version-rejected.

AWS: Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): CreateClusterV2 POST /api/v2/clusters, input ClusterName (__stringMin1Max64, required) + Provisioned (ProvisionedRequest, carrying BrokerNodeGroupInfo.InstanceType and KafkaVersion), errors BadRequestException (400) / ConflictException (409) / ForbiddenException (403) / UnauthorizedException (401) / TooManyRequestsException (429) / InternalServerErrorException (500) / ServiceUnavailableException (503). The model enumerates no concrete Kafka versions, so WHICH versions a new cluster may be built on is recorded behavior only. The recording for this check is a provisioned CreateClusterV2 with KafkaVersion naming a version MSK does not offer at all (a bogus "9.9.9") and a standard broker instance type (kafka.m5.large): AWS answers 400 BadRequestException with the message "Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]". Message anatomy: the fixed text is "<category> instance types are not supported for Kafka version <version>. Valid values: [<comma-separated versions>]"; <category> is derived from the request's InstanceType ("Standard" for the kafka.* families, "Express" for express.*), <version> is the request's KafkaVersion verbatim, and the bracketed list is the versions MSK offers for a NEW cluster of that broker category — the requirement being that this list is MSK's standard-broker new-cluster offering, which the recording shows is the six-entry set above and does NOT include the older lines 3.6.0, 3.7.x or 3.7.x.kraft even though the catalog still publishes rows for them.

Emulator: answers the same request 400 BadRequestException (correct class) but with the message "Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.6.0, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]" — nine entries, three of them (3.6.0, 3.7.x, 3.7.x.kraft) lines MSK does not offer for a new standard-broker cluster. The list is not recorded beside the refusal; it is computed as "every catalog row with Status=ACTIVE, minus KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY", which conflates the catalog's ACTIVE (still-supported / upgrade-target) set with MSK's much narrower new-cluster offering, so every version the catalog keeps publishing for existing clusters leaks into the advertised valid values.

Why: the valid-value list is the actionable half of this refusal — a client that reads it to pick a version it may retry with is steered onto 3.6.0 / 3.7.x / 3.7.x.kraft, versions real MSK refuses a new standard-broker cluster on, so the retry fails again on real AWS while succeeding against the emulator. Tooling and tests that assert the refusal sentence (its fixed text plus the offering it names) diverge on the emulator, and provisioning code generated from the emulator's advice is wrong against AWS.

Fix: the version-offering state behind the provisioned create's refusal — services/service-com-amazonaws-kafka-kafka/src/state/kafka_versions.rs: the per-category new-cluster offering must be MSK's recorded new-cluster set for that broker category rather than a projection of the catalog's ACTIVE rows, and the refusal renderer must list exactly that set, in MSK's published order. Note the tension with open issue 20261121T113417Z-kafka-createcluster-createclusterv2-refuse-active-kafka-versions-the-service.md (that issue asks the create GATE not to refuse catalog-ACTIVE versions); it does not cover this cause and carries no key for this check — this cause is about the ADVERTISED valid-value list of the refusal sentence, which must stay MSK's recorded standard-broker new-cluster offering regardless of how wide the admission gate is.

Traces: kafka.cluster-create-unsupported-kafka-version-rejected (provisioned CreateClusterV2, KafkaVersion=9.9.9, InstanceType=kafka.m5.large, 2 brokers, 2 client subnets). Code citations: new_cluster_offering (lines 637-647), new_cluster_kafka_versions (lines 659-680) and create_kafka_version_violation (lines 701-712) of services/service-com-amazonaws-kafka-kafka/src/state/kafka_versions.rs; KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY at line 618.

## Evidence
Expected (AWS), CreateClusterV2 with Provisioned.KafkaVersion = a version MSK does not offer and BrokerNodeGroupInfo.InstanceType = kafka.m5.large:
  HTTP 400, Error.Code "BadRequestException",
  Error.Message "Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]"

Actual (emulator), same request — status and code match, message diverges (verbatim from the replay assertion):
  AssertionError: Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.6.0, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]

Message anatomy: fixed text = "<category> instance types are not supported for Kafka version <version>. Valid values: [<comma-separated versions>]". Variable parts: <category> from the request's InstanceType ("Standard" here), <version> the request's KafkaVersion verbatim ("9.9.9" here — a request value, not part of the template), and the bracketed list the service's new-cluster offering for that category. The divergence is purely in that bracketed list: the emulator adds the three lines 3.6.0, 3.7.x and 3.7.x.kraft that AWS's recorded standard-broker offering does not name. No per-run value (host, id, timestamp, request id) is involved.

Source of the emulator's list: services/service-com-amazonaws-kafka-kafka/src/state/kafka_versions.rs — new_cluster_offering() (lines 637-647) filters the catalog to Status=ACTIVE and removes only KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY = ["4.2.x.kraft"] (line 618); create_kafka_version_violation() (lines 701-712) joins that vector into the sentence.

The rest of the check (code == BadRequestException, and ListClustersV2 reporting no cluster left behind by the rejected create) was not reached because the message assertion fails first.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers).
2. Discover a VPC with two subnets in distinct AZs and a security group.
3. Call CreateClusterV2 with ClusterName=<any 1-64 char name> and Provisioned = {KafkaVersion: "9.9.9", BrokerNodeGroupInfo: {InstanceType: "kafka.m5.large", ClientSubnets: [the two subnets], SecurityGroups: [the sg], StorageInfo: {EbsStorageInfo: {VolumeSize: 10}}}, NumberOfBrokerNodes: 2}.
4. Observe HTTP 400 BadRequestException whose Message is "Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.6.0, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]" instead of AWS's "... Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]".
5. Repeat with InstanceType="express.m7g.large" to confirm the Express category's advertised list is likewise derived from the catalog's ACTIVE rows rather than recorded.
6. Re-replay kafka.cluster-create-unsupported-kafka-version-rejected; it asserts the full refusal sentence and currently stops on the valid-value list.
