Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.resolve-component-candidates-device-absent-member-invalid-request-rejected greengrassv2.resolve-component-candidates-device-conflicting-requirements-not-found-rejected greengrassv2.resolve-component-candidates-device-unsatisfiable-requirement-not-found-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.resolve-component-candidates-device-absent-member-invalid-request-rejected|run#12|AssertionError|assert (status, code) == (200, None), f"the complete request was refused: {status} {code!r} {body!r}"
Issue-Key: greengrassv2.resolve-component-candidates-device-conflicting-requirements-not-found-rejected|_picked#10|AssertionError|assert (status, code) == (200, None), (
Issue-Key: greengrassv2.resolve-component-candidates-device-unsatisfiable-requirement-not-found-rejected|accepted#10|AssertionError|assert (status, code) == (200, None), (
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# greengrassv2 data-plane ResolveComponentCandidates is unroutable: a core device's client-certificate request on the IoT data endpoint gets 404 UnknownService

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResolveComponentCandidates

## Description
1 cause to close.

Cause 1 of 1: ResolveComponentCandidates posted on the Greengrass V2 data plane — POST /greengrass/v2/resolveComponentCandidates against the account's iot:Data-ATS endpoint on the client-certificate port (8443), authenticated by mutual TLS with an IoT device certificate and carrying NO SigV4 Authorization header and NO X-Amz-Target — never reaches the greengrassv2 service at all. Failing checks: greengrassv2.resolve-component-candidates-device-absent-member-invalid-request-rejected, greengrassv2.resolve-component-candidates-device-conflicting-requirements-not-found-rejected, greengrassv2.resolve-component-candidates-device-unsatisfiable-requirement-not-found-rejected.

AWS: Smithy (com.amazonaws.greengrassv2#GreengrassV2): ResolveComponentCandidates is bound to POST /greengrass/v2/resolveComponentCandidates with success code 200 and output member resolvedComponentVersions (list of ResolvedComponentVersion with componentName/componentVersion/arn/recipe); modeled errors AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Recorded behavior only for the data-plane dialect: a core device whose certificate is ACTIVE, attached to a thing and to an IoT policy granting greengrass:ResolveComponentCandidates, opens an mTLS connection to its iot:Data-ATS endpoint on port 8443, sends only `content-type: application/json`, and gets HTTP 200 with the resolved version; a request whose body omits componentCandidates, omits platform, or is truncated JSON is refused HTTP 400 with response header x-amzn-ErrorType `InvalidRequestException` and a body that is the empty JSON object `{}` (all of that is fixed: the status, the error code and the empty body carry no variable part); a request with `componentCandidates: []` is accepted with HTTP 200 and body `{"resolvedComponentVersions": []}`. Hypothesis: the emulator's front door resolves the target service only from a SigV4 credential scope, X-Amz-Target or the Host header, and a device data-plane request supplies none of those, so no path-based route exists for the data plane.

Emulator: every one of those requests is answered HTTP 404 with body {"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} and no x-amzn-ErrorType naming a greengrassv2 error. The failure hits the very first, fully valid control request in each check, so none of the data-plane surface (resolution, the empty-list case, the InvalidRequestException refusals) is exercised or implemented.

Why: this is how every real Greengrass nucleus resolves the component versions it must download — it never signs with IAM credentials, it presents its device certificate to the data endpoint. A client (or a nucleus under test) gets a 404 with an emulator-authored `UnknownService` type instead of either the resolved component list or a modeled greengrassv2 error, so it cannot tell "component not resolvable" from "my request was misaddressed", and deployment-time resolution logic written against the emulator cannot run at all.

Fix: the front-door request router / service resolver (the component that produces the `UnknownService` response) must resolve greengrassv2 for the data-plane route by its HTTP path, and the data endpoint's client-certificate listener must accept an mTLS device credential as a credential class instead of requiring a SigV4 scope; then ResolveComponentCandidates' handler plus its request validation must serve the data-plane dialect: 200 with resolvedComponentVersions, 200 with an empty list for an empty candidate list, and 400 / x-amzn-ErrorType InvalidRequestException with an empty JSON object body for a missing componentCandidates, a missing platform, or a body that is not valid JSON.

Related but not covering: open issue 20261103T145648Z-greengrass-greengrassv2-wire-refusal-paths-diverge-wrong-status-error.md, whose Cause 1 is about the SAME operation reached with ordinary IAM SigV4 credentials (it must answer 403 AccessDeniedException there). That cause is about a control-plane-credential refusal; this one is the device data-plane path, which must SUCCEED, and it is not routed at all.

Traces: the three failing checks named above.

## Evidence
All three checks fail on the first data-plane POST, before any assertion about resolution content.

greengrassv2.resolve-component-candidates-device-absent-member-invalid-request-rejected:
  AssertionError: the complete request was refused: 404 'UnknownService' {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}
  expected: (status, x-amzn-ErrorType) == (200, None) with body {'resolvedComponentVersions': [{'componentVersion': '1.0.0', 'arn': 'arn:aws:greengrass:<region>:<account>:components:<name>:versions:1.0.0', ...}]}

greengrassv2.resolve-component-candidates-device-conflicting-requirements-not-found-rejected:
  AssertionError: requirements {'a': '>=1.0.0'} were refused: 404 'UnknownService' {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}
  expected: 200 with the resolved version

greengrassv2.resolve-component-candidates-device-unsatisfiable-requirement-not-found-rejected:
  AssertionError: resolving '>=1.0.0' was refused with 404 'UnknownService': {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}
  expected: 200 with the resolved version

Still-unreached expectations recorded in the same checks (they become assertable once the route exists):
  componentCandidates absent      -> expected (400, 'InvalidRequestException', {})
  platform absent                 -> expected (400, 'InvalidRequestException', {})
  truncated JSON body             -> expected (400, 'InvalidRequestException', {})
  componentCandidates: []         -> expected (200, None, {'resolvedComponentVersions': []})
The ARN, component name and region/account above are per-run values filled from the request and emulator state; the status, the error code `InvalidRequestException`, the empty `{}` error body and the member names are the requirement.

## Steps to Reproduce
1. Boot the emulator. With an iot client, create an IoT policy granting Action greengrass:ResolveComponentCandidates on Resource "*", create_thing(<device>), create_keys_and_certificate(setAsActive=True), attach_policy(policy, certArn) and attach_thing_principal(<device>, certArn); write certificatePem/keyPair.PrivateKey to disk.
2. iot.describe_endpoint(endpointType='iot:Data-ATS') and connect to that host on port 8443 with an SSL context loading the certificate/key (mTLS), no SigV4 and no X-Amz-Target.
3. greengrassv2.create_component_version(inlineRecipe=<a 1.0.0 linux recipe for <name>>) and wait for componentState DEPLOYABLE.
4. POST /greengrass/v2/resolveComponentCandidates with headers {'content-type': 'application/json'} and body {"platform": {"name": "linux-amd64", "attributes": {"os": "linux", "architecture": "amd64"}}, "componentCandidates": [{"componentName": "<name>", "componentVersion": "1.0.0", "versionRequirements": {"d": "1.0.0"}}]}.
   Expect HTTP 200 and {"resolvedComponentVersions": [...]} naming <name> 1.0.0; observe HTTP 404 {"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}.
5. Repeat step 4 with componentCandidates removed, with platform removed, and with the last byte of the JSON body cut: expect HTTP 400, header x-amzn-ErrorType: InvalidRequestException and body {}.
6. Repeat step 4 with "componentCandidates": []: expect HTTP 200 and {"resolvedComponentVersions": []}.
