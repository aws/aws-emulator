Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog opensearch-domain-version-capacity-and-software-update-management esdef-create-domain-gp3-server-defaults opensearch-application-error-paths
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: es|DescribeInstanceTypeLimits|4xx->4xx|$.message
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|PutDefaultApplicationSetting|4xx->4xx|$.message
Issue-Commit: 4931c0587e4227d2df501c213f0baf3c9cfd790a

# es instance-type catalog is a stale legacy list, and DescribeDomain Endpoint plus the PutDefaultApplicationSetting 404 message diverge (5 causes)

> _Note: this issue was identified against the code base at commit `4931c0587e4227d2df501c213f0baf3c9cfd790a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
This bundle carries FIVE net-new causes in the `es` read and refusal surface. A fixer must close all five before submitting.

Cause 1 of 5: ListInstanceTypeDetails serves a locally synthesized, stale instance-type catalog (previous-generation c5/m5-family entries) instead of the catalog AWS publishes for the requested engine version, and omits the modeled AvailabilityZones member.
AWS: Smithy (com.amazonaws.opensearch) ListInstanceTypeDetails GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, output InstanceTypeDetails -> InstanceTypeDetailsList; InstanceTypeDetails = {InstanceType: OpenSearchPartitionInstanceType (enum), EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole: InstanceRoleList, AvailabilityZones: AvailabilityZoneList}. The catalog is AWS's published set for the engine version, not a derived one; recorded behavior shows current-generation entries (m7i.*.search, c8g.*.search, oi2.*.search, r7i.*.search, i8ge/i8g/or1/or2/om2/r8g/r8gd families) and each row carries the AvailabilityZones key (null when RetrieveAZs was not requested).
Emulator: returns the legacy c5.large.search / c5.2xlarge.search / c5.4xlarge.search / c5.9xlarge.search ... family list for the same engine version, and each row OMITS the AvailabilityZones member entirely rather than emitting it as null. The ordering and membership diverge across essentially the whole list (150+ positions).
Why: a client that enumerates supported instance types to pick one for CreateDomain, or that validates a user's selection against this catalog, is offered instance types AWS no longer lists and cannot find the ones AWS does list — so a legitimate CreateDomain choice looks unsupported and an unsupported one looks fine. A client reading row['AvailabilityZones'] raises KeyError instead of reading null.
Fix: the ListInstanceTypeDetails handler's catalog source — serve AWS's published per-engine-version instance-type catalog instead of a synthesized family list, and always emit the AvailabilityZones member.
Traces: es-engine-version-and-instance-catalog#task#3.

Cause 2 of 5: the same stale catalog is served on the version/capacity fixture's ListInstanceTypeDetails read, confirming the defect is in the catalog itself and not in one fixture's engine version.
AWS: as Cause 1 — recorded behavior shows the current-generation rows (m7i.2xlarge.search, c8g.12xlarge.search, r7i.12xlarge.search, c8g.8xlarge.search, ...) with AvailabilityZones present.
Emulator: the identical c5-family synthesized list, again without AvailabilityZones, diverging across ~140 list positions.
Why: same as Cause 1; a fixer that corrects only one engine version's catalog leaves this read still wrong, which is why it is listed as its own cause with its own replay step.
Fix: the shared ListInstanceTypeDetails catalog table, not a per-fixture special case.
Traces: opensearch-domain-version-capacity-and-software-update-management#task#5.

Cause 3 of 5: DescribeInstanceTypeLimits' 400 ValidationException message enumerates the SAME stale instance-type set in its "Member must satisfy enum value set" clause, so the refusal text does not match AWS's.
AWS: Smithy DescribeInstanceTypeLimits errors include ValidationException (httpError 400). Recorded behavior: the message is AWS's standard constraint-violation template — fixed text "<N> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<the enum members>]" — where <value> is the instance type from the request, <memberName> is 'instanceType', and the bracketed list is the full OpenSearchPartitionInstanceType enum AWS accepts (including m7i.2xlarge.search, c8g.16xlarge.search, oi2.4xlarge.search, or1.*, or2.*, om2.*, i8g*/i8ge*, r8g*/r8gd*, r7gd.*, i7i.*).
Emulator: status and error class match (400), but the bracketed enum list in the message is the emulator's stale catalog, so the message text diverges.
Why: the bracketed set is the only machine-readable hint AWS gives about which instance types are valid; a client (or a human) that reads it to correct the request is told a different, wrong set of allowed values.
Fix: the DescribeInstanceTypeLimits validation path — render the enum-value-set clause from the same corrected OpenSearchPartitionInstanceType catalog as Cause 1. The fixed template text and the request-derived <value>/<memberName> parts are already correct; only the enum list is wrong.
Traces: es-engine-version-and-instance-catalog#task#5.

Cause 4 of 5: DescribeDomain publishes DomainStatus.Endpoint as a host:port with a path component instead of a DNS hostname.
AWS: recorded behavior only — DomainStatus.Endpoint is a bare DNS hostname of the form search-<domain-name>-<opaque-suffix>.<region>.es.amazonaws.com: no scheme, no port, and no path segment.
Emulator: emits a value of the form <ip>:<port>/_aws/es/<domain-name> — it carries a port and a URL path where AWS carries neither. (The emulator also pins DomainStatus.ServiceSoftwareOptions.CurrentVersion to a fixed release label; the specific label is a drifting AWS release value and is NOT claimed as a requirement here.)
Why: clients build a data-plane URL by prefixing a scheme to Endpoint and appending their own index path ("https://" + Endpoint + "/my-index/_doc"). A value that already contains a port and a path produces a malformed URL, and anything that treats Endpoint as a hostname (SigV4 host header, TLS SNI, DNS resolution, host-based routing) is handed a non-hostname. The particular IP, port and name suffix are environment-specific; the requirement is that Endpoint is a hostname with no port and no path.
Fix: the DescribeDomain/CreateDomain DomainStatus serializer's Endpoint field — publish a hostname and carry any port/path routing elsewhere.
Traces: esdef-create-domain-gp3-server-defaults#task#2.

Cause 5 of 5: PutDefaultApplicationSetting's not-found refusal carries the wrong message — it reports the ARN as unresolvable where AWS reports that the account holds no applications.
AWS: Smithy PutDefaultApplicationSetting PUT /2021-01-01/opensearch/defaultApplicationSetting, input {applicationArn (required, ARN), setAsDefault (required)}, errors AccessDeniedException(403), InternalException(500), ResourceNotFoundException, ValidationException(400). Recorded behavior: when the caller's account has no OpenSearch applications at all, the refusal is HTTP 404 with the message "No applications found for account ID: <account-id>" — fixed text "No applications found for account ID: " plus the caller's account id.
Emulator: answers HTTP 404 (status matches) with "Application not found for ARN: <applicationArn>" — it echoes the request's applicationArn and describes a per-ARN lookup miss instead of AWS's account-scoped "no applications" condition.
Why: the two messages describe different conditions. AWS tells the client the ACCOUNT has no applications (so creating one is the remedy); the emulator tells it THAT ARN is unknown (so correcting the ARN is the remedy). A client or operator that branches on, logs, or surfaces this message is pointed at the wrong fix, and a message-asserting test against AWS fails.
Fix: the PutDefaultApplicationSetting handler's not-found branch — when the account's application collection is empty, emit AWS's account-scoped message with the caller's account id as its only variable part, rather than an ARN-echoing message.
Traces: opensearch-application-error-paths#task#2.

Related but NOT covering these: 20261030T180653Z-es-the-engine-version-compatibility-matrix-and-reserved owns the GetCompatibleVersions matrix cluster, a different catalog; 20261105T004752Z-es-createapplication-still-mints-a-constant-application-id and 20261110T150745Z-es-getapplication-publishes-the-application-endpoint-on-a own the CreateApplication id and GetApplication endpoint clusters, not PutDefaultApplicationSetting's message or DescribeDomain's Endpoint; 20261114T022111Z-opensearch-the-warm-storage-unsupported-instance-type-validationexception covers a warm-storage ValidationException echoing the wrong instance-type SUFFIX, not the enum-value-set list in DescribeInstanceTypeLimits. None of the five causes above is owned by an open issue.

## Evidence
Cause 1 — es-engine-version-and-instance-catalog#task#3, ListInstanceTypeDetails, 200 -> 200, body diverges:
  $.InstanceTypeDetails[0]: expected {'InstanceType': 'm7i.2xlarge.search', 'AvailabilityZones': None, 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data','master','coordinator']}, got {'InstanceType': 'c5.large.search', ... no AvailabilityZones key ...}
  $.InstanceTypeDetails[2]: expected InstanceType 'c8g.12xlarge.search', got 'c5.2xlarge.search'
  $.InstanceTypeDetails[3]: expected InstanceType 'oi2.8xlarge.search' with InstanceRole ['data'], got 'c5.4xlarge.search' with InstanceRole ['data','master','coordinator']
  $.InstanceTypeDetails[4]: expected InstanceType 'r7i.12xlarge.search', got 'c5.9xlarge.search'
  divergence_key lists ~157 diverging InstanceTypeDetails positions ([0]..[229]).

Cause 2 — opensearch-domain-version-capacity-and-software-update-management#task#5, ListInstanceTypeDetails, 200 -> 200:
  $.InstanceTypeDetails[0]: expected 'm7i.2xlarge.search' (+AvailabilityZones), got 'c5.large.search' (no AvailabilityZones)
  $.InstanceTypeDetails[2]: expected 'c8g.12xlarge.search', got 'c5.2xlarge.search'
  $.InstanceTypeDetails[3]: expected 'r7i.12xlarge.search', got 'c5.4xlarge.search'
  $.InstanceTypeDetails[4]: expected 'c8g.8xlarge.search', got 'c5.9xlarge.search'
  divergence_key lists ~137 diverging positions.

Cause 3 — es-engine-version-and-instance-catalog#task#5, DescribeInstanceTypeLimits, 400 -> 400 (status and error class match), $.message value_mismatch:
  expected "1 validation error detected: Value 't9.nonexistent.search' at 'instanceType' failed to satisfy constraint: Member must satisfy enum value set: [m6g.xlarge.search, im4gn.8xlarge.search, ..., m7i.2xlarge.search, r7i.12xlarge.search, i8ge.18xlarge.search, oi2.4xlarge.search, or1.*, or2.*, om2.*, i8g.*, r8g.*, r8gd.medium.search, c8g.16xlarge.search, ...]"
  got the same template with the emulator's stale enum list.
  Fixed text: "<N> validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy enum value set: [" / "]". Variable from the request: the rejected value ('t9.nonexistent.search' here) and the member name ('instanceType'). Variable from the service catalog: the bracketed enum members — this is the part that diverges.

Cause 4 — esdef-create-domain-gp3-server-defaults#task#2, DescribeDomain, 200 -> 200:
  $.DomainStatus.Endpoint: expected 'search-sim2real-probe-g3-7128833f-diafy7krxiou6qhs4ifofxsmay.us-west-2.es.amazonaws.com', got '127.0.0.1:43349/_aws/es/sim2real-probe-g3-7128833f'
  $.DomainStatus.ServiceSoftwareOptions.CurrentVersion: expected 'OpenSearch_2_19_R20260929', got 'OpenSearch_2_19_R20250101' (recorded release label; drifting value, noted not claimed)
  NOTE: the host 127.0.0.1, the port 43349 and the suffix diafy7krxiou6qhs4ifofxsmay are environment-specific. The filed requirement is structural: Endpoint is a DNS hostname with NO port and NO path segment.

Cause 5 — opensearch-application-error-paths#task#2, PutDefaultApplicationSetting, 404 -> 404, $.message value_mismatch:
  expected 'No applications found for account ID: 000000000000'
  got 'Application not found for ARN: arn:aws:es:us-east-1:000000000000:application/zzzmissingappid000'
  Fixed text: "No applications found for account ID: ". Variable part: the caller's account id (000000000000 is this recording's value). The emulator's text is emulator-authored and describes a different condition.

Smithy cited: ListInstanceTypeDetails (GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}) -> InstanceTypeDetailsList; InstanceTypeDetails{InstanceType: OpenSearchPartitionInstanceType, EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole: InstanceRoleList, AvailabilityZones: AvailabilityZoneList}; DescribeInstanceTypeLimits errors include ValidationException(400); PutDefaultApplicationSetting{applicationArn required, setAsDefault required} errors AccessDeniedException(403)/InternalException(500)/ResourceNotFoundException/ValidationException(400).

## Steps to Reproduce
1. Clone the repo and boot the emulator from it, then replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Cause 1: replay fixture es-engine-version-and-instance-catalog, step task#3 — GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} (ListInstanceTypeDetails). Compare the InstanceTypeDetails list against the recorded one position by position and check that each row carries the AvailabilityZones member.
3. Cause 2: replay fixture opensearch-domain-version-capacity-and-software-update-management, step task#5 — the same operation on that fixture's engine version; confirm the corrected catalog is served here too.
4. Cause 3: replay es-engine-version-and-instance-catalog, step task#5 — DescribeInstanceTypeLimits with an instance type outside the enum. Confirm the 400 ValidationException message's bracketed enum-value set matches AWS's accepted instance-type enum.
5. Cause 4: replay esdef-create-domain-gp3-server-defaults, step task#2 — DescribeDomain on the created domain; assert DomainStatus.Endpoint is a DNS hostname containing no ':' port and no '/' path segment.
6. Cause 5: replay opensearch-application-error-paths, step task#2 — PutDefaultApplicationSetting with an applicationArn for an account that holds no applications; assert HTTP 404 with message 'No applications found for account ID: <caller account id>'.
7. Each step should pass against the recorded response.
