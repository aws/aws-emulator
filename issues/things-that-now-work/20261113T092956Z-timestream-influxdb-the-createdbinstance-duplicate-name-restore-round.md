Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Commit: 33c939d4ec001105a8b177394bd7a87b2127538e

# timestream-influxdb: the CreateDbInstance duplicate-name restore round trip never completes — create → AVAILABLE → delete → identical re-create never finishes, the check times out at 60s

> _Note: this issue was identified against the code base at commit `33c939d4ec001105a8b177394bd7a87b2127538e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbInstance

## Description
One net-new root cause in the timestream-influxdb emulator, grounded in the failing behavioral check timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected.

Cause 1 of 1: the DB-instance lifecycle the duplicate-name scenario needs — CreateDbInstance (CREATING) → GetDbInstance reports AVAILABLE → DeleteDbInstance frees the name → the identical CreateDbInstance is accepted again and the NEW instance reaches AVAILABLE — never completes when driven as a client drives it. The check no longer fails on the ConflictException message assertion (the signature already carried by open issue 20261109T183400Z-timestream-influxdb-2-net-new-causes-createdbinstance-duplicate.md, whose replay key for this check is the Message AssertionError at run#15); it now runs past that assertion into the restore leg and the whole check is cut off with "Check timed out after 60.0s", so the state transitions after the conflict are never observed at all. Failing check ids: timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected.

AWS: Smithy model of this service — CreateDbInstance returns `status` (enum Status: AVAILABLE | CREATING | DELETED | DELETING | FAILED | MAINTENANCE | MODIFYING | REBOOTING | …) with required `id`, `name`, `arn`; GetDbInstance (readonly) returns the same record and models ResourceNotFoundException (HTTP 404); DeleteDbInstance returns the record and models ConflictException (409) and ResourceNotFoundException (404). Recorded behavior: a freshly created instance reports CREATING and then settles to AVAILABLE; a second CreateDbInstance with a name a live instance still owns is refused 409 ConflictException while the ORIGINAL stays the only instance with that name; after that instance is deleted (DeleteDbInstance → status DELETING, and the id eventually stops resolving) the identical CreateDbInstance succeeds and returns a NEW `id` with status CREATING, which then reaches AVAILABLE. The requirement here is liveness of that sequence: every transient status an emulator reports must actually settle, promptly, for a client that polls GetDbInstance — the name-conflict is a STATE (a live instance owns the name), not an always-invalid input, so the post-delete re-create must become possible.

Emulator: the sequence does not finish inside the per-check budget: the replay output for this check is only "Error: Check timed out after 60.0s" — no response from the emulator is recorded past the conflict leg, so neither the original instance's AVAILABLE, nor the delete, nor the restored instance's CREATING/AVAILABLE is ever observed. Hypothesis (the harness does not isolate the leg): the settle of a transient status is materialized lazily, only when a reporting operation reads the record, after a fixed span — services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/state.rs defines `TRANSITION_SPAN = Duration::from_millis(500)` and `DbInstance::settle` applies the owed change on read, and `DeleteDbInstance` (src/ops/delete_db_instance.rs) erases the record outright while reporting DELETING, so a client that polls on the service's own cadence spends a whole probe interval per transient status and the create → AVAILABLE → delete → re-create → AVAILABLE round trip costs more wall time than the whole scenario is allowed, instead of settling before the client's first follow-up read.

Why: a client cannot use the normal create-if-absent pattern against this emulator. It creates an instance, gets the 409 for the duplicate name, and then waits forever on readiness or on the name becoming reusable: the duplicate-name conflict reads as permanent rather than as the transient state AWS makes it, and no caller can tell "still provisioning" from "never going to settle". Every workflow that provisions, waits, deletes and re-provisions the same instance name — the cheapest way to script this service — hangs.

Fix: the DB-instance lifecycle in the service state (services/.../src/state.rs — `DbInstance::settle` / `begin_transition` and the transient-status span) together with the CreateDbInstance and DeleteDbInstance handlers: a created instance must become AVAILABLE for the very next read, and a deleted instance must release its name and stop resolving at once, so the create → AVAILABLE → delete → identical-re-create → AVAILABLE sequence completes without the client having to burn a polling interval per transition.

Traces: timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected. Related but NOT covering: open issue 20261109T183400Z-timestream-influxdb-2-net-new-causes-createdbinstance-duplicate.md owns this check only under its ConflictException-message replay key (Cause 1: the "The DbInstance with id […], name […], customer […] already exists" wording) and its endpoint-publication cause; neither carries this timeout signature, so closing that issue leaves the restore leg of this scenario unrun.

## Evidence
Check: timestream-influxdb.create-db-instance-duplicate-name-conflict-rejected [mismatch]
  reported failure (verbatim, the only output the replay produced for it): "Error: Check timed out after 60.0s"

What the scenario asks of the emulator, in order (from the check body):
  1. CreateDbInstance(name=<N>, password=<random>, dbInstanceType=db.influx.medium, vpcSubnetIds=[<2 subnets>], vpcSecurityGroupIds=[<sg>], allocatedStorage=20, dbStorageType=InfluxIOIncludedT1, deploymentType=SINGLE_AZ) → expects status CREATING and name <N>.
  2. CreateDbInstance with the identical name → expects 409 ConflictException (message rule owned by the related open issue).
  3. ListDbInstances → expects exactly one instance named <N>, the original id.
  4. GetDbInstance(identifier=<id>) polled until status in (AVAILABLE, FAILED) → expects AVAILABLE.
  5. DeleteDbInstance(identifier=<id>), then GetDbInstance polled until the id no longer resolves or reports DELETED.
  6. CreateDbInstance with the identical name → expects success, a NEW id, status CREATING, exactly one instance owning <N>, and that new instance polled to AVAILABLE.

Expected: the whole sequence completes, legs 4-6 observable.
Actual: the check is cut off at 60.0s with nothing recorded past leg 2 — legs 4, 5 and 6 produce no observation at all, so the emulator never demonstrates that a created instance becomes AVAILABLE in this scenario, that the delete frees the name, or that the identical re-create then succeeds with a new id.
Supporting code in the checkout (citations, not values to copy): services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/state.rs:141 (`TRANSITION_SPAN`), :240 (`DbInstance::settle`, read-triggered), src/ops/get_db_instance.rs:229 (settle on read), src/ops/delete_db_instance.rs:226-244 (record removed, reported DELETING).

## Steps to Reproduce
1. Boot the emulator and create the networking prerequisites (a VPC with at least two subnets and a security group) via EC2.
2. Call CreateDbInstance(name=<N>, password=<8-64 chars matching ^[a-zA-Z0-9]+$>, dbInstanceType="db.influx.medium", vpcSubnetIds=[<subnet-a>,<subnet-b>], vpcSecurityGroupIds=[<sg>], allocatedStorage=20, dbStorageType="InfluxIOIncludedT1", deploymentType="SINGLE_AZ", tags={...}); keep the returned id. Expect status CREATING.
3. Call CreateDbInstance again with the identical name and a different password; expect 409 ConflictException.
4. ListDbInstances and confirm exactly one instance owns <N>, the original id.
5. Poll GetDbInstance(identifier=<id>) on a client-chosen interval until status is AVAILABLE or FAILED; expect AVAILABLE.
6. DeleteDbInstance(identifier=<id>) and poll GetDbInstance until the id no longer resolves (ResourceNotFoundException) or reports DELETED.
7. Call CreateDbInstance with the identical name <N> again; expect success with a NEW id and status CREATING, then poll it to AVAILABLE.
Observed: the sequence does not complete — replaying the behavioral check that performs exactly these steps ends in "Check timed out after 60.0s" with nothing observed after step 3.
