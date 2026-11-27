Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-db-capacity-reservation-name-and-engine-validation-rejected rds.create-instance-validation-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-db-capacity-reservation-name-and-engine-validation-rejected|run#11|AssertionError|assert err_engine.get("Code") == "InvalidParameterValue", (
Issue-Key: rds.create-instance-validation-rejected|_reject#8|AssertionError|raise AssertionError(
Issue-Commit: 4f741250707f5bfbfc0e3af8f0ba3eed659dbba6

# rds: 2 net-new control-plane validation causes — CreateDBCapacityReservation refuses an unsupported engine with DBCapacityReservationUnsupported instead of InvalidParameterValue, and CreateDBInstance accepts the deprecated magnetic storage type

> _Note: this issue was identified against the code base at commit `4f741250707f5bfbfc0e3af8f0ba3eed659dbba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCapacityReservation

## Description
Two independent, net-new RDS control-plane validation divergences, both grounded in failing behavioral checks the suite lists under the `rds` heading, neither owned by an open issue's replay key. A fixer must close BOTH before submitting.

Named neighbours that do NOT cover them: open issue 20261124T234610Z-rds-db-capacity-reservations-a-reservation-aws-creates.md is about CreateDBCapacityReservation refusing a configuration AWS ACCEPTS (200 -> 400) and about DescribeDBCapacityReservations' not-found wording, not about the code/message of the unsupported-engine refusal below; resolved issue 20261118T032939Z-rds-createdbinstance-input-validation-storage-type-allocatedstorage-floor.md covered the earlier legs of the same create-validation check (the gp3 AllocatedStorage floor, which now passes) and says nothing about the magnetic storage type.

=== Cause 1 of 2: CreateDBCapacityReservation refuses an engine Capacity Reservations do not support with the wrong error code and an emulator-authored message ===
AWS: Smithy (com.amazonaws.rds#AmazonRDSv19) models CreateDBCapacityReservation with required DBCapacityReservationName (@length 1..63, pattern ^[a-z](?:-?[a-z0-9]+){0,15}$), DBInstanceClass, Engine, CapacityPerAvailabilityZone and AvailabilityZones, and four modeled refusals — DBCapacityReservationAlreadyExistsFault, DBCapacityReservationUnsupportedFault, InstanceQuotaExceededFault, InsufficientDBInstanceCapacityFault (all client, httpError 400). The Engine VALUE is validated as a request parameter before any configuration/capacity question is asked, so recorded behavior for an engine outside the reserve-able set is HTTP 400 with the AWS Query common error code `InvalidParameterValue` and message `Unsupported engine type: <Engine>. Supported types are: aurora-mysql, aurora-postgresql, mysql, postgres, mariadb, sqlserver-ee, sqlserver-se, sqlserver-ex, sqlserver-web, sqlserver-dev-ee, sqlserver-dev-se, db2-ae, db2-se, db2-ce, oracle-ee, oracle-ee-cdb, oracle-se2, oracle-se2-cdb.` Fixed text: `Unsupported engine type: ` … `. Supported types are: ` + that 18-engine list + `.`; the only variable part is the Engine value from the request. (The name-pattern leg of the same check passes: a malformed DBCapacityReservationName is already refused InvalidParameterValue with AWS's `1 validation error detected: Value '<name>' at 'dBCapacityReservationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z](?:-?[a-z0-9]+){0,15}`.)
Emulator: answers HTTP 400 with Error.Code `DBCapacityReservationUnsupported` and message `The specified configuration isn't supported for DB capacity reservations: Engine=<Engine>` — a different error code (a modeled fault, but the one AWS reserves for an unsupported CONFIGURATION, not an invalid parameter value) and message text of its own wording that never lists the supported engines.
Why: a client branches on the code — `InvalidParameterValue` is a fix-your-request condition, while `DBCapacityReservationUnsupported` reads as "this configuration is not offered" and sends an operator looking for a capacity/配置 workaround; and AWS's message is the only place the caller learns WHICH engines can be reserved, so tooling that surfaces or parses that list gets nothing.
Fix: the Engine-eligibility gate in the CreateDBCapacityReservation handler's validation stage must raise the InvalidParameterValue refusal with AWS's `Unsupported engine type: ... Supported types are: ...` sentence built from the same reserve-able-engine table it checks against, instead of mapping to DBCapacityReservationUnsupportedFault.
Traces: failing check rds.create-db-capacity-reservation-name-and-engine-validation-rejected (AssertionError at check line 74 on the engine leg's code). Handler: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_capacity_reservation.rs and the engine table it consults in services/service-com-amazonaws-rds-amazonrdsv19/src/state (crate::state::reserves_capacity_for_engine).

=== Cause 2 of 2: CreateDBInstance accepts StorageType=standard (magnetic), which real RDS refuses at create time ===
AWS: Smithy CreateDBInstanceMessage.StorageType documents `Valid Values: gp2 | gp3 | io1 | io2 | standard`, so `standard` is still in the API surface and still modeled — the deprecation is recorded behavior only: a CreateDBInstance naming StorageType=standard is refused HTTP 400 with error code `InvalidParameterValue` (AWS also uses `InvalidParameterCombination` for this class) and a message that states magnetic storage is deprecated and names the replacement storage types; nothing is created, so a DescribeDBInstances on the requested identifier answers DBInstanceNotFound. The fixed text of that refusal is the deprecation sentence containing `magnetic storage is deprecated`; its variable parts are the storage type from the request and the replacements AWS offers.
Emulator: the identical request — Engine=postgres, DBInstanceClass=db.t3.micro, AllocatedStorage=20, StorageType=standard, every other field legal — is ACCEPTED (2xx) and the instance is minted. No refusal, no error code.
Why: a client gets a DB instance real RDS would never have created, on a storage type it cannot provision; IaC pre-flight and capacity/cost tooling that treats the 400 as the signal silently proceeds, and the same template then fails against real AWS. It also masks the only create-time signal that magnetic volumes are no longer provisionable.
Fix: the CreateDBInstance handler's storage validation stage — refuse a requested StorageType of `standard` for the engines where AWS no longer provisions magnetic volumes, with the InvalidParameterValue refusal worded as AWS's magnetic-deprecation sentence, and mint nothing. The sibling legs of the same validation stage (reserved master username, password minimum length, the gp3 AllocatedStorage floor, the unsupported instance class) already refuse correctly, so only this gate is missing.
Traces: failing check rds.create-instance-validation-rejected (AssertionError at check line 127/178: "deprecated magnetic storage type: CreateDBInstance was ACCEPTED"). Handler: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_instance.rs validation stage.

## Evidence
Cause 1 — check rds.create-db-capacity-reservation-name-and-engine-validation-rejected:
  leg 1 (PASSES today): CreateDBCapacityReservation(DBCapacityReservationName="Sim2Real-Bad-Name", DBInstanceClass="db.r7g.large", Engine="mysql", CapacityPerAvailabilityZone=1, AvailabilityZones=[<an available AZ>]) -> 400 InvalidParameterValue, "1 validation error detected: Value 'Sim2Real-Bad-Name' at 'dBCapacityReservationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z](?:-?[a-z0-9]+){0,15}".
  leg 2 (the divergence): same call with a well-formed name and Engine="custom-oracle-ee"
    expected: HTTP 400, Error.Code "InvalidParameterValue", Error.Message "Unsupported engine type: custom-oracle-ee. Supported types are: aurora-mysql, aurora-postgresql, mysql, postgres, mariadb, sqlserver-ee, sqlserver-se, sqlserver-ex, sqlserver-web, sqlserver-dev-ee, sqlserver-dev-se, db2-ae, db2-se, db2-ce, oracle-ee, oracle-ee-cdb, oracle-se2, oracle-se2-cdb." (the only variable part is the Engine value from the request)
    actual:   HTTP 400, Error.Code "DBCapacityReservationUnsupported", Error.Message "The specified configuration isn't supported for DB capacity reservations: Engine=custom-oracle-ee"
  replay output: AssertionError: well-formed name with an engine Capacity Reservations does not support: expected InvalidParameterValue, got {'Type': 'Sender', 'Code': 'DBCapacityReservationUnsupported', 'Message': "The specified configuration isn't supported for DB capacity reservations: Engine=custom-oracle-ee"}
  Model citation: CreateDBCapacityReservation errors = [DBCapacityReservationAlreadyExistsFault 400, DBCapacityReservationUnsupportedFault 400 (awsQueryError DBCapacityReservationUnsupported), InstanceQuotaExceededFault 400, InsufficientDBInstanceCapacityFault 400]; DBCapacityReservationName pattern ^[a-z](?:-?[a-z0-9]+){0,15}$.

Cause 2 — check rds.create-instance-validation-rejected:
  Request: CreateDBInstance(DBInstanceIdentifier=<fresh id>, Engine=postgres, EngineVersion=<DescribeDBEngineVersions DefaultOnly>, DBInstanceClass=db.t3.micro, StorageType="standard", AllocatedStorage=20, MasterUsername=sim2real, MasterUserPassword=<legal>, BackupRetentionPeriod=0, DBSubnetGroupName=<group over the run's subnets>).
  expected: HTTP 400, Error.Code InvalidParameterValue (or InvalidParameterCombination), message containing "magnetic storage is deprecated" and naming the replacement storage types; DescribeDBInstances on that identifier -> DBInstanceNotFound.
  actual:   the create is ACCEPTED (2xx) and the instance is minted.
  replay output: AssertionError: deprecated magnetic storage type: CreateDBInstance was ACCEPTED; real RDS refuses this input (mutation={'StorageType': 'standard', 'AllocatedStorage': 20})
  Model citation: CreateDBInstanceMessage.StorageType doc — "Valid Values: gp2 | gp3 | io1 | io2 | standard" (the value is modeled; the create-time refusal is recorded behavior).
  Earlier legs of the same check that pass: reserved MasterUsername 'rdsadmin', password below 8 characters, AllocatedStorage=5 on gp3, DBInstanceClass=db.t2.micro.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point an rds client at it; pick an available AZ with DescribeAvailabilityZones.
2. CreateDBCapacityReservation(DBCapacityReservationName="sim2real-dbcr-<suffix>", DBInstanceClass="db.r7g.large", Engine="custom-oracle-ee", CapacityPerAvailabilityZone=1, AvailabilityZones=[<that AZ>]).
3. Observe Error.Code DBCapacityReservationUnsupported with "The specified configuration isn't supported for DB capacity reservations: Engine=custom-oracle-ee" instead of InvalidParameterValue with "Unsupported engine type: custom-oracle-ee. Supported types are: <the 18 reserve-able engines>."
4. Confirm the malformed-name leg still answers InvalidParameterValue with AWS's validation sentence (it must keep passing).
5. Re-replay behavioral check rds.create-db-capacity-reservation-name-and-engine-validation-rejected.

Cause 2:
1. CreateDBSubnetGroup over the run's VPC subnets.
2. CreateDBInstance with Engine=postgres, DBInstanceClass=db.t3.micro, AllocatedStorage=20, StorageType="standard", a legal MasterUsername/MasterUserPassword and that subnet group.
3. Observe the call is accepted and DescribeDBInstances returns the instance; real RDS answers HTTP 400 InvalidParameterValue whose message states magnetic storage is deprecated and names the replacements, and mints nothing.
4. Repeat with StorageType="gp3", AllocatedStorage=20 — that must still be accepted (control case), confirming the gate is storage-type-specific.
5. Re-replay behavioral check rds.create-instance-validation-rejected.
