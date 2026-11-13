Issue-Source: trace-sweep-divergence
Issue-Trace: pcs-cluster-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pcs-cluster-error-paths
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs|CreateCluster|4xx->4xx|$.message
Issue-Commit: 753bdd73d5313a4157e41dbd5de5e41f691f653f

# pcs CreateCluster refuses a valid request on an emulator-authored scheduler-version rule and never validates networking.subnetIds

> _Note: this issue was identified against the code base at commit `753bdd73d5313a4157e41dbd5de5e41f691f653f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Two root causes on the PCS CreateCluster path (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the wire corpus simparity-wire-aws-v1 (fixture pcs-cluster-error-paths, step task#3). BOTH must be closed before this issue is submitted; they share the one fixture step because the first one masks the second — fixing only the version gate turns the step from the wrong 400 into a 200.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, awsJson1_0).

Related but NOT covering: open issue 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md. Its Cause 5 named this same fixture step for the then-observed behavior (CreateCluster answered HTTP 200), and the emulator has since changed: the step now answers HTTP 400 with a scheduler-version refusal, so that issue's replay key no longer binds this divergence (the replay reports it with no owner) and its Cause 5 never describes the version gate that now short-circuits the path. The subnet requirement below is restated here because this step cannot pass until both are in place.

=== Cause 1 of 2: CreateCluster refuses a scheduler version the service accepts, with an emulator-authored message ===
Fixture/step: pcs-cluster-error-paths#task#3 (CreateCluster).
AWS: Smithy: CreateClusterRequest.scheduler (SchedulerRequest, required) has required members type (SchedulerType enum) and version (plain String — the model imposes NO enum, @pattern or @length on it); its documentation lists "Valid Values: 24.11 | 25.05 | 25.11 | 26.05" and points at the Slurm-versions page, and ValidationException's own docs list "The scheduler version isn't supported." as an example. Recorded behavior: a CreateCluster carrying scheduler {type SLURM, version 23.11} was NOT refused for its scheduler version — PCS carried on and refused the request on its networking instead (see Cause 2). So the recording is the requirement here: this version reaches the networking checks rather than being refused. The version set the model documents is prose documentation that has moved over time, not a wire constraint, and PCS's own wording for a version it genuinely cannot provision is the fixed sentence "AWS PCS doesn't support the scheduler version you provided." (the wording this crate already uses on the UpdateCluster path), naming no type and no version. Hypothesis: the recorded request predates the documented list, which is exactly why a hard-coded set is the wrong gate.
Emulator: refuses the request up front with HTTP 400 ValidationException whose message is the emulator's own text "Scheduler version not found: type=SLURM, version=23.11" — fixed text "Scheduler version not found: type=" / ", version=", with the type and version taken from the request. The gate is crate::state::create_scheduler_version_violation, matching input.scheduler.version against the hard-coded CREATE_SCHEDULER_VERSIONS set, called before any networking check.
Why: a client issuing a CreateCluster that PCS accepts gets a terminal 400 from the emulator, so provisioning code and infrastructure tests that pin a working Slurm version cannot create a cluster at all; and a client that matches on the refusal text for a genuinely unsupported version never matches, because the message is not the sentence PCS serves.
Fix: the CreateCluster handler's validation (ops/create_cluster.rs scheduler-version step and the crate::state::create_scheduler_version_violation helper it calls): stop refusing versions outside a hard-coded list — only refuse one the emulator truly cannot provision, and when refusing, serve the modeled ValidationException with PCS's fixed "AWS PCS doesn't support the scheduler version you provided." sentence rather than emulator-authored text naming the type and version.
Traces: pcs-cluster-error-paths#task#3.

=== Cause 2 of 2: networking.subnetIds are never checked for existence, so no subnet refusal exists on the path ===
Fixture/step: pcs-cluster-error-paths#task#3 (CreateCluster).
AWS: Smithy: CreateClusterRequest.networking (NetworkingRequest, required) carries subnetIds (SubnetIdList, @length min 1), documented as the subnets where PCS creates the control-plane ENI, and CreateCluster's own documentation states "The subnets and security groups for the cluster must already exist before you use this API action"; ValidationException (errorKind client, @httpError 400) lists "There are networking related errors, such as network validation failure" as an example. Recorded behavior: a CreateCluster naming a subnet id that exists in no account state is refused with HTTP 400, __type com.amazon.aws.pcs.service.exceptions#ValidationException and the fixed message "AWS PCS couldn't find 1 or more of the subnets you provided." — the whole sentence is fixed text, it names no subnet id, and no cluster is created.
Emulator: has no subnet-existence check anywhere on the CreateCluster path. ops/create_cluster.rs validates clusterName's pattern, the scheduler version (Cause 1) and the SIZES of the caller-sized stored lists (crate::state::stored_list_violation, which counts networking.subnetIds entries but never resolves them), then mints the id and inserts the ClusterRecord. With Cause 1 fixed this step answers HTTP 200 with a created cluster, which is what the replay observed before the version gate was added.
Why: the guardrail is missing rather than merely worded differently: a client that relies on CreateCluster to reject an unusable network receives a cluster whose control-plane ENI could never be placed, so the failure surfaces much later or not at all, the bogus cluster pollutes state and counts against the cluster quota, and tests asserting the refusal pass against the emulator and fail against PCS.
Fix: add the networking validation to the CreateCluster handler (ops/create_cluster.rs), before the id is minted and the record inserted: resolve every networking.subnetIds entry against the emulator's subnet state and, if any is unknown, return the modeled ValidationException with the fixed "AWS PCS couldn't find 1 or more of the subnets you provided." message and persist nothing.
Traces: pcs-cluster-error-paths#task#3.

## Evidence
Wire replay of simparity-wire-aws-v1 against the booted emulator: steps_total 14, passed 2, mismatched 12, 7 divergence clusters, score 14.3%, bootstrap_signal false. Six of the seven clusters are owned by open issue 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md; this one is reported with no owner and no blocking stub.

pcs-cluster-error-paths#task#3 CreateCluster [mismatch], divergence_key service=pcs|CreateCluster|status=400->400|keys=$.message:
- status: expected 400, got 400 (matches)
- $.__type: matches the recorded com.amazon.aws.pcs.service.exceptions#ValidationException
- $.message: expected "AWS PCS couldn't find 1 or more of the subnets you provided.", got "Scheduler version not found: type=SLURM, version=23.11"

The recorded message is entirely fixed text (it names no subnet). The emulator's message is emulator-authored: fixed text "Scheduler version not found: type=" / ", version=" with SLURM and 23.11 filled in from the request.

Confirmed in the checkout (commit of this branch):
- services/.../src/ops/create_cluster.rs lines ~207-215 call crate::state::create_scheduler_version_violation(&input.scheduler.r#type, &input.scheduler.version) and return ValidationException before any networking check.
- services/.../src/state.rs lines ~86 and ~889-899: CREATE_SCHEDULER_VERSIONS = ["24.11", "25.05", "25.11", "26.05"]; the helper formats "Scheduler version not found: type={type}, version={version}". Line ~905-911 shows the update path already uses PCS's fixed sentence "AWS PCS doesn't support the scheduler version you provided."
- ops/create_cluster.rs lines ~221-261 count networking.subnetIds only for the stored-list size ceiling; nothing resolves the ids, and lines ~299-341 mint and insert the record unconditionally.

## Steps to Reproduce
Boot the emulator and point an AWS SDK/botocore PCS client at it (awsJson1_0, sigv4), then replay fixture pcs-cluster-error-paths, step task#3.

Cause 1: pcs.create_cluster(clusterName=<a valid name>, scheduler={'type': 'SLURM', 'version': '23.11'}, size='SMALL', networking={'subnetIds': ['subnet-<an id that exists in no subnet state>']}). Expect the request to pass the scheduler-version check (the recording shows PCS does not refuse this version) and be refused on its networking instead. Observe HTTP 400 ValidationException with message 'Scheduler version not found: type=SLURM, version=23.11'. Separately, issue the same call with a version the emulator truly cannot provision and expect the refusal message to be exactly 'AWS PCS doesn't support the scheduler version you provided.'

Cause 2: repeat the call with a version inside CREATE_SCHEDULER_VERSIONS (e.g. '24.11') so the version gate does not fire, keeping the nonexistent subnet id. Expect HTTP 400, __type ValidationException and message exactly "AWS PCS couldn't find 1 or more of the subnets you provided.", with no cluster created. Observe HTTP 200 with a cluster in CREATING; a following pcs.list_clusters() shows the cluster that should not exist.
