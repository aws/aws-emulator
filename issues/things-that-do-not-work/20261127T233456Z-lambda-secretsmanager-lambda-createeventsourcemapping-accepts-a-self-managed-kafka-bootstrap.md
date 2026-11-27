Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager.self-managed-kafka-bootstrap-server-of-300-chars-rejected-as-malformed
Issue-Key: lambda__secretsmanager.self-managed-kafka-bootstrap-server-of-300-chars-rejected-as-malformed|_refusal#19|AssertionError|raise AssertionError(f"the request was accepted, answering {out!r}")
Issue-Commit: c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6

# lambda: CreateEventSourceMapping accepts a self-managed Kafka bootstrap server at the 300-character bound that AWS refuses as malformed

> _Note: this issue was identified against the code base at commit `c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
1 net-new cause.

Cause 1 of 1: CreateEventSourceMapping does not apply Lambda's own well-formedness check to SelfManagedEventSource.Endpoints["KAFKA_BOOTSTRAP_SERVERS"] entries, so a host:port at the modelled 300-character ceiling is accepted and a mapping is created.
AWS: Smithy CreateEventSourceMapping.SelfManagedEventSource -> SelfManagedEventSource.Endpoints -> Endpoints map (@length {1,2}) of EndpointLists -> list @length {1,10} of Endpoint -> string @length {min:1,max:300} @pattern "^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\\-]*[a-zA-Z0-9])\\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\\-]*[A-Za-z0-9]):[0-9]{1,5}$". An entry built from legal DNS labels that is exactly 300 characters long therefore satisfies BOTH the modelled length bound and the modelled pattern — yet Lambda refuses it, because no well-formed host:port is that long (a DNS name is capped at 253 characters): HTTP 400, error code InvalidParameterValueException, message "Malformed self managed event source endpoints provided" — entirely fixed text, no variable part. (Recorded behaviour: the model states only the 300-character bound and the pattern; the semantic refusal and its wording come from the recording.) The refusal creates NOTHING: the function still lists only the previously created mapping, and that mapping's stored SelfManagedEventSource is unchanged. The same labels at 200 characters — inside the 253-character DNS limit — ARE accepted and stored verbatim.
Emulator: the 300-character entry is ACCEPTED. CreateEventSourceMapping answers HTTP 202 with a full mapping body (a fresh UUID, State 'Disabled', StateTransitionReason 'USER_INITIATED', the BASIC_AUTH SourceAccessConfigurations entry pointing at the Secrets Manager secret, and the 300-character server echoed back under SelfManagedEventSource.Endpoints.KAFKA_BOOTSTRAP_SERVERS), so a second mapping now exists where AWS has none.
Why: a client that validates its broker configuration against the emulator ships an event source mapping real Lambda refuses, and the error surfaces only against AWS. Worse, the emulator leaves behind an extra mapping that AWS never creates, so any subsequent assertion over ListEventSourceMappings for the function (count, or the single mapping's identity) diverges from AWS for the rest of the session, and no InvalidParameterValueException is ever raised on this path for a client's error handling to exercise.
Fix: CreateEventSourceMapping's validation of SelfManagedEventSource (and the same check in UpdateEventSourceMapping) — reject a bootstrap-server entry whose host portion cannot be a well-formed DNS name, with 400 InvalidParameterValueException and that message, before any mapping is minted.
Traces: lambda__secretsmanager.self-managed-kafka-bootstrap-server-of-300-chars-rejected-as-malformed.

## Evidence
Behavioural suite simparity-behavioral-aws-v1, service heading '### lambda__secretsmanager' (5/6 passed).

lambda__secretsmanager.self-managed-kafka-bootstrap-server-of-300-chars-rejected-as-malformed [mismatch]
  AssertionError: the request was accepted, answering {... 'HTTPStatusCode': 202, 'UUID': <fresh uuid>, 'StartingPosition': 'LATEST', 'BatchSize': 100, 'State': 'Disabled', 'StateTransitionReason': 'USER_INITIATED', 'Topics': ['sim2real-orders'], 'SourceAccessConfigurations': [{'Type': 'BASIC_AUTH', 'URI': <the broker secret's ARN>}], 'SelfManagedEventSource': {'Endpoints': {'KAFKA_BOOTSTRAP_SERVERS': ['kkkk....invalid:9092'  — 300 characters]}}, ...}

  expected: the call is refused with ('InvalidParameterValueException', 'Malformed self managed event source endpoints provided'), the function still lists exactly the one previously created mapping, and that mapping's SelfManagedEventSource is byte-identical to what it held before the refused call.

  Positive control in the same check passes: the same '.invalid' labels built to 200 characters are accepted, the mapping settles as 'Disabled', and GetEventSourceMapping echoes KAFKA_BOOTSTRAP_SERVERS back exactly as sent. So the divergence is specific to the over-long (but model-legal) entry, not to the self-managed Kafka path as a whole.

Model citations (smithy_query on services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json): SelfManagedEventSource.Endpoints -> Endpoints @length{1,2}; EndpointLists -> list @length{1,10} of Endpoint; Endpoint -> string @length{min:1,max:300} @pattern ^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9]):[0-9]{1,5}$ — i.e. the 300-character entry is model-legal, so this is a service-level semantic check the handler is missing.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-behavioral-aws-v1 suite and look under the '### lambda__secretsmanager' heading.
3. Replay lambda__secretsmanager.self-managed-kafka-bootstrap-server-of-300-chars-rejected-as-malformed. It creates a Secrets Manager username/password secret, an IAM role allowed to read it, and a python3.13 function, then:
   a. calls CreateEventSourceMapping with Enabled=False, Topics=['sim2real-orders'], StartingPosition='LATEST', SourceAccessConfigurations=[{Type:'BASIC_AUTH', URI:<secret ARN>}] and a KAFKA_BOOTSTRAP_SERVERS entry of exactly 200 characters built from legal DNS labels under the reserved .invalid TLD — this must be accepted, settle as 'Disabled', and store the server verbatim;
   b. calls it again with the identical request but a 300-character entry built from the same labels — this must be refused 400 with error code InvalidParameterValueException and message 'Malformed self managed event source endpoints provided';
   c. asserts ListEventSourceMappings for the function still returns only the UUID from step (a), and that GetEventSourceMapping on it reports the same SelfManagedEventSource as before the refusal.
