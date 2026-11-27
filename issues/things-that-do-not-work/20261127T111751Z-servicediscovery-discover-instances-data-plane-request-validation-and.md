Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-discover-instances-revision-invalid-service-name
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-discover-instances-revision-invalid-service-name servicediscovery-discover-instances-unknown-service-answers-empty
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|DiscoverInstancesRevision|4xx->4xx|$.message
Issue-Key: servicediscovery|DiscoverInstancesRevision|4xx->2xx|$.Message
Issue-Key: servicediscovery|DiscoverInstancesRevision|4xx->2xx|$.__type
Issue-Key: servicediscovery|DiscoverInstancesRevision|4xx->2xx|status
Issue-Key: servicediscovery|DiscoverInstances|2xx->4xx|$.Instances
Issue-Key: servicediscovery|DiscoverInstances|2xx->4xx|$.InstancesRevision
Issue-Key: servicediscovery|DiscoverInstances|2xx->4xx|status
Issue-Key: servicediscovery|DiscoverInstancesRevision|2xx->4xx|$.InstancesRevision
Issue-Key: servicediscovery|DiscoverInstancesRevision|2xx->4xx|status
Issue-Commit: fe76b837b9c6a96309bb6d3401999300c9d3921a

# servicediscovery Discover instances data-plane request validation and unknown-service lookups diverge from Cloud Map

> _Note: this issue was identified against the code base at commit `fe76b837b9c6a96309bb6d3401999300c9d3921a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DiscoverInstances

## Description
The emulator's Cloud Map data-plane operations (DiscoverInstances / DiscoverInstancesRevision, host prefix `data-`) diverge from AWS in four distinct ways: the text of the ServiceName pattern-validation message, a missing OwnerAccount format validation, and two cases where an unknown service name is refused instead of answered with an empty result. This bundle contains 4 causes; all four must be fixed before it is closed.

Cause 1 of 4 — DiscoverInstancesRevision ServiceName pattern-violation message quotes the wrong regular expression (fixture servicediscovery-discover-instances-revision-invalid-service-name, step task#7; same cluster also covers task#8).
AWS: Smithy: DiscoverInstancesRevision.ServiceName (and DiscoverInstances.ServiceName) is @required with @pattern "^((?=^.{1,127}$)^([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9])(\\.([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9]))*$)|(^\\.$)$"; the modeled error for a constraint violation is InvalidInput (client, HTTP 400). Per the recording, AWS answers HTTP 400 and the message follows the standard constraint-violation template: the fixed text "1 validation error detected: Value '" + <the rejected value, from the request> + "' at '" + <the member name, i.e. serviceName> + "' failed to satisfy constraint: Member must satisfy regular expression pattern: " + <the pattern AWS reports for that member>. The variable parts are the request's value and the member name; the pattern AWS reports is fixed text for this member and is NOT the model's trait string verbatim — AWS reports it WITHOUT the outer wrapping anchors, i.e. "((?=^.{1,127}$)^([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9])(\\.([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9]))*$)|(^\\.$)".
Emulator: status and error class are right (HTTP 400), but it interpolates the raw Smithy @pattern string, emitting the same sentence with the pattern wrapped as "^((?=...)...)|(^\\.$)$" — a leading "^(" and trailing ")$" that AWS does not include.
Why: a client or test that asserts on the Cloud Map validation sentence (or extracts the advertised pattern from it to pre-validate names locally) sees a regular expression that is not the one AWS publishes, and the comparison fails even though the request was rejected for the right reason.
Fix: the ServiceName constraint-validation message formatting for the Discover* operations (the pattern text used by the input validation layer, which must be the AWS-reported pattern for the member rather than the model trait string).
Traces: servicediscovery-discover-instances-revision-invalid-service-name#task#7 (cluster also covers #task#8).

Cause 2 of 4 — DiscoverInstancesRevision accepts a malformed OwnerAccount instead of rejecting it (fixture servicediscovery-discover-instances-revision-invalid-service-name, step task#9).
AWS: Smithy: DiscoverInstancesRevision.OwnerAccount targets AWSAccountId with @length min 12 / max 12; InvalidInput (client, HTTP 400) is the modeled error. Recorded behavior: a request whose OwnerAccount is not a 12-digit account id is refused with HTTP 400, JSON body carrying __type "InvalidInput" and Message "Invalid AWS account ID format. Expected 12 digits." — that message is entirely fixed text for this error class (no request- or state-derived parts).
Emulator: returns HTTP 200 with a normal DiscoverInstancesRevision result; no __type and no Message are emitted, so the malformed account id is silently accepted.
Why: a client that relies on Cloud Map validating OwnerAccount gets a success for a request AWS rejects, so cross-account discovery bugs (a truncated or non-numeric account id) stay invisible in the emulator and surface only against the real service; error-path tests that expect InvalidInput never see it.
Fix: input validation for the Discover* operations — add the AWSAccountId format/length check on OwnerAccount and raise InvalidInput (400) with the documented message.
Traces: servicediscovery-discover-instances-revision-invalid-service-name#task#9.

Cause 3 of 4 — DiscoverInstances refuses an unknown service name instead of answering with an empty instance list (fixture servicediscovery-discover-instances-unknown-service-answers-empty, step task#4).
AWS: Smithy: DiscoverInstances returns DiscoverInstancesResponse { Instances: HttpInstanceSummaryList, InstancesRevision: long }. Recorded behavior: for an existing namespace and a syntactically valid ServiceName that matches no service, the data-plane answers HTTP 200 with Instances = [] and InstancesRevision = 0 (it does not raise the modeled ServiceNotFound/404 and certainly not a 400).
Emulator: answers HTTP 400; neither Instances nor InstancesRevision is present in the body.
Why: discovery clients treat DiscoverInstances as a best-effort lookup and expect an empty list to mean "nothing registered yet" — a 400 turns a normal cold-start poll into a hard client error, so service-discovery retry loops and health-based routing break against the emulator.
Fix: the DiscoverInstances handler's service lookup — an unresolvable service name in a known namespace must produce the empty 200 result rather than a validation refusal.
Traces: servicediscovery-discover-instances-unknown-service-answers-empty#task#4.

Cause 4 of 4 — DiscoverInstancesRevision refuses an unknown service name instead of returning revision 0 (fixture servicediscovery-discover-instances-unknown-service-answers-empty, step task#5).
AWS: Smithy: DiscoverInstancesRevision returns DiscoverInstancesRevisionResponse { InstancesRevision: long }. Recorded behavior: for an existing namespace and a valid ServiceName that matches no service, AWS answers HTTP 200 with InstancesRevision = 0, consistent with the empty DiscoverInstances result.
Emulator: answers HTTP 400 with no InstancesRevision member.
Why: clients poll DiscoverInstancesRevision to decide whether to re-fetch instances; a 400 for a not-yet-registered service makes the cheap revision poll unusable and forces error handling where AWS returns a well-defined zero revision.
Fix: the DiscoverInstancesRevision handler's service lookup (same resolution path as cause 3) — return revision 0 for an unknown service in a known namespace.
Traces: servicediscovery-discover-instances-unknown-service-answers-empty#task#5.

Related but distinct open issue, not covering any cause above: 20261123T024804Z-servicediscovery-cloud-map-getoperation-reports-route-53-s-raw.md (GetOperation ErrorMessage), which already owns the fifth replayed divergence.

## Evidence
Replay of trace set simparity-wire-aws-v1 (20 fixtures, 205 steps, 199 passed, 6 mismatched, score 97.1%):

Cause 1 — servicediscovery-discover-instances-revision-invalid-service-name#task#7 (DiscoverInstancesRevision), expected 400 / actual 400, $.message value_mismatch:
  expected: "1 validation error detected: Value 'two words' at 'serviceName' failed to satisfy constraint: Member must satisfy regular expression pattern: ((?=^.{1,127}$)^([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9])(\\.([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9]))*$)|(^\\.$)"
  actual:   "1 validation error detected: Value 'two words' at 'serviceName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((?=^.{1,127}$)^([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9])(\\.([a-zA-Z0-9_][a-zA-Z0-9-_]{0,61}[a-zA-Z0-9_]|[a-zA-Z0-9]))*$)|(^\\.$)$"
  (difference is only the added leading '^(' and trailing ')$' around the alternation; 'two words' and 'serviceName' are request-derived and template-filled, the pattern text is fixed.) Cluster also covers #task#8.

Cause 2 — servicediscovery-discover-instances-revision-invalid-service-name#task#9 (DiscoverInstancesRevision): status expected 400, got 200; $.__type missing (expected 'InvalidInput', got None); $.Message missing (expected 'Invalid AWS account ID format. Expected 12 digits.', got None).

Cause 3 — servicediscovery-discover-instances-unknown-service-answers-empty#task#4 (DiscoverInstances): status expected 200, got 400; $.Instances missing (expected [], got None); $.InstancesRevision missing (expected 0, got None).

Cause 4 — servicediscovery-discover-instances-unknown-service-answers-empty#task#5 (DiscoverInstancesRevision): status expected 200, got 400; $.InstancesRevision missing (expected 0, got None).

Model evidence (services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json): DiscoverInstances/DiscoverInstancesRevision ServiceName @required + @pattern as quoted; OwnerAccount -> AWSAccountId @length 12..12; errors InvalidInput(400), NamespaceNotFound(404), ServiceNotFound(404), RequestLimitExceeded(429); outputs Instances (list) and InstancesRevision (long).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture servicediscovery-discover-instances-revision-invalid-service-name step task#7 — call DiscoverInstancesRevision with NamespaceName set to the fixture's existing namespace and ServiceName='two words'; compare the 400 body's $.message pattern text against AWS's (no outer '^(' ... ')$').
4. Cause 2: same fixture, step task#9 — call DiscoverInstancesRevision with a valid NamespaceName/ServiceName and an OwnerAccount that is not 12 digits; the emulator answers 200 instead of 400 InvalidInput 'Invalid AWS account ID format. Expected 12 digits.'
5. Cause 3: replay fixture servicediscovery-discover-instances-unknown-service-answers-empty step task#4 — call DiscoverInstances for an existing namespace with a valid but unregistered ServiceName; the emulator answers 400 instead of 200 with Instances=[] and InstancesRevision=0.
6. Cause 4: same fixture, step task#5 — call DiscoverInstancesRevision with the same unknown ServiceName; the emulator answers 400 instead of 200 with InstancesRevision=0.
