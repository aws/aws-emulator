Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-instance-parameter-group-identifier-checked-and-stable
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-instance-parameter-group-identifier-checked-and-stable|run#6|AssertionError|raise AssertionError(f"CreateDbInstance with an unknown parameter group returned {stray!r}")
Issue-Commit: 8c63d5b7f8677a460355a4ac20e9a08b37d56b80

# timestream-influxdb: CreateDbInstance does not validate dbParameterGroupIdentifier — an unknown parameter group is accepted with HTTP 200 instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `8c63d5b7f8677a460355a4ac20e9a08b37d56b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbInstance

## Description
Cause 1 of 1: CreateDbInstance accepts a dbParameterGroupIdentifier that names no existing DB parameter group and creates the instance anyway.

AWS: CreateDbInstance's input member `dbParameterGroupIdentifier` is a reference to the DbParameterGroupResource (Smithy: CreateDbInstanceInput.dbParameterGroupIdentifier -> DbParameterGroupIdentifier, @length min 3 max 64, @pattern ^[a-zA-Z0-9]+$; CreateDbInstanceInput carries @references [com.amazonaws.timestreaminfluxdb#DbParameterGroupResource]). When the referenced group does not exist in the caller's account and region, AWS refuses the create with the operation's modeled ResourceNotFoundException (Smithy: CreateDbInstance errors include ResourceNotFoundException, errorKind client, httpError 404) and creates no instance. Recorded behavior: the refusal carries error code `ResourceNotFoundException` with message `The parameter group with id does not exist`. In that message the fixed text is the whole sentence `The parameter group with id does not exist` as AWS words it for this error class; the only variable part of this class is the group identifier taken from the request's `dbParameterGroupIdentifier` (the recording shows AWS emitting the sentence with no id interpolated, so the id value from any single run must not be baked into the text).

Emulator: the call returns HTTP 200 with a full CreateDbInstanceOutput for a brand-new instance, echoing the nonexistent group back in `dbParameterGroupIdentifier` (replayed output shows `status: 'CREATING'`, a fresh `id`/`arn`, and `dbParameterGroupIdentifier` set to the unknown value supplied in the request). No validation of the identifier against parameter-group state happens at all, and the instance is then listed by ListDbInstances.

Why: a client relying on AWS's behavior treats a successful CreateDbInstance as proof that the referenced parameter group exists and is attached. Against the emulator, typo'd or not-yet-created parameter groups silently produce instances whose configuration references nothing; negative-path tests that assert the 404/ResourceNotFoundException refusal pass on AWS and fail here, and error-handling code paths for a missing group are never exercised. It also leaks a stray instance (and its ARN, secret ARN and tags) that AWS would never have created, so subsequent quota, listing and cleanup logic diverges.

Fix: the CreateDbInstance handler's input validation in the timestream-influxdb service crate — before any instance is allocated, resolve `dbParameterGroupIdentifier` (when present) against the stored DB parameter groups and, on miss, return the modeled ResourceNotFoundException (HTTP 404, code `ResourceNotFoundException`, message as worded above) without mutating state. The same lookup belongs in any other operation that accepts the member (e.g. UpdateDbInstance) so attach/modify refuses an unknown group identically.

Traces: behavioral check timestream-influxdb.create-db-instance-parameter-group-identifier-checked-and-stable — it calls create_db_instance with dbParameterGroupIdentifier="nosuchgroup<random hex>" and asserts a ClientError whose (Code, Message) equals ("ResourceNotFoundException", "The parameter group with id does not exist"); instead the create returned 200 and the check raised `AssertionError: CreateDbInstance with an unknown parameter group returned {...}`.

## Evidence
Check: timestream-influxdb.create-db-instance-parameter-group-identifier-checked-and-stable (verdict: mismatch)

Expected (AWS): CreateDbInstance with an unknown dbParameterGroupIdentifier is refused —
  HTTP 404, Error.Code = "ResourceNotFoundException",
  Error.Message = "The parameter group with id does not exist"
  and no DB instance is created (the refused name must not appear in ListDbInstances).
Model support: CreateDbInstance errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400]; CreateDbInstanceInput @references DbParameterGroupResource.

Actual (emulator): HTTP 200 with a created instance, e.g.
  {'id': '<generated>', 'name': '<requested name>', 'arn': 'arn:aws:timestream-influxdb:<region>:<account>:db-instance/<generated id>',
   'status': 'CREATING', 'dbInstanceType': 'db.influx.medium', 'dbStorageType': 'InfluxIOIncludedT1',
   'allocatedStorage': 20, 'deploymentType': 'SINGLE_AZ',
   'dbParameterGroupIdentifier': '<the unknown group id from the request>', ...,
   'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Failure raised by the check:
  AssertionError: CreateDbInstance with an unknown parameter group returned {...}
(generated ids, ARNs, ports, endpoints and timestamps above are per-run values, not requirements; the requirement is the 404 + code + message and the absence of any created instance.)

## Steps to Reproduce
1. Boot the emulator and point a botocore `timestream-influxdb` client at it.
2. Create (or reuse) a VPC with at least two subnets and one security group.
3. Call CreateDbInstance with a dbParameterGroupIdentifier that matches the member's pattern ^[a-zA-Z0-9]+$ but names no existing parameter group, e.g.:
     influx.create_db_instance(name="sim2real-instpg-bad-<suffix>", password=<16 hex chars>,
         dbInstanceType="db.influx.medium", dbStorageType="InfluxIOIncludedT1",
         allocatedStorage=20, deploymentType="SINGLE_AZ",
         vpcSubnetIds=[<subnet-a>, <subnet-b>], vpcSecurityGroupIds=[<sg>],
         dbParameterGroupIdentifier="nosuchgroup" + secrets.token_hex(8),
         tags={"sim2real": "create-db-instance-parameter-group-identifier-checked-and-stable"})
4. Observe HTTP 200 and a CreateDbInstanceOutput instead of a ClientError.
   Expected: ClientError with HTTP 404, Code "ResourceNotFoundException", Message "The parameter group with id does not exist".
5. Call ListDbInstances: the emulator lists the instance that should never have been created.
