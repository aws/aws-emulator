Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-connectivity-info-roundtrip
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-connectivity-info-roundtrip
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|UpdateConnectivityInfo|4xx->4xx|$.Message
Issue-Commit: 814edd6f7b3ad9f8ffb89a4a014f6c44b204f5ee

# greengrass UpdateConnectivityInfo answers a generic JSON-parse message instead of AWS's unparsable-parameter BadRequestException text

> _Note: this issue was identified against the code base at commit `814edd6f7b3ad9f8ffb89a4a014f6c44b204f5ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectivityInfo

## Description
UpdateConnectivityInfo's malformed-body refusal returns the right status (400) and error class but emulator-authored message text: it reports a blanket JSON-parse failure instead of naming the connectivity-info parameter it could not parse. The error message is a requirement: AWS's fixed text exactly, with its variable part (the parameter name(s) that failed to parse) taken from the request. 1 cause to close.

Related-but-not-covering open issues (neither carries this wire replay key): 20261116T105306Z-greengrass-updateconnectivityinfo-mints-a-random-uuid-as-version.md covers the SUCCESS path of the same operation (the Version token it mints on 200), not this refusal path; 20261103T145648Z-greengrass-greengrassv2-wire-refusal-paths-diverge-wrong-status-error.md bundles refusal-text divergences on the greengrassv2 control-plane surface (ResolveComponentCandidates, CreateDeployment, GetDeployment, ListTagsForResource) and does not touch the greengrass v1 /greengrass/things/{ThingName}/connectivityInfo path.

Cause 1 of 1: the body-parse refusal does not name the unparsable parameter. AWS: Smithy: UpdateConnectivityInfo PUT /greengrass/things/{ThingName}/connectivityInfo, success code 200 (output members Version, Message with jsonName 'message'); input members ThingName (@required, httpLabel) and ConnectivityInfo (list of ConnectivityInfo{HostAddress,Id,Metadata,PortNumber}); modeled errors BadRequestException (httpError 400, 'General error information.', members Message/ErrorDetails) and InternalServerErrorException (500). Recorded behavior for the message: when the request body is present but the connectivity-info list member cannot be deserialized into the modeled list (e.g. the member is given a JSON type other than a list of connectivity-info structures, or its elements have the wrong member types), the service refuses with HTTP 400 BadRequestException and message 'The following required parameter(s) are unparsable: connectivityInfoList.' Fixed text: 'The following required parameter(s) are unparsable: ' followed by the offending parameter name(s) as the service names them on the wire, followed by '.'. The variable part is the parameter-name list taken from the request: here the single name 'connectivityInfoList' (the service's own name for the ConnectivityInfo list member — not the modeled member name, and not a per-run value). Emulator: HTTP 400 with $.Message = 'Invalid json format found in the request body.' — a generic, emulator-authored sentence that is neither AWS's wording nor parameter-specific, so it is identical for every malformed body regardless of which member failed. Why: a client (or a test suite, or an operator reading a log) relying on AWS's behavior cannot tell WHICH parameter of the request it must correct, because the emulator's sentence names none; and any code or assertion matching AWS's 'The following required parameter(s) are unparsable: ...' sentence never matches, so malformed-input handling validated against the emulator breaks in production. Fix: the request-deserialization/validation step of UpdateConnectivityInfo's handler must distinguish a body that is not JSON at all from a body whose connectivity-info list member fails to deserialize, and raise BadRequestException with the unparsable-parameter sentence, filling in the wire parameter name(s) that failed; the generic parse message must not be the single answer for every malformed body.

Traces: greengrassv2-connectivity-info-roundtrip#task#5 (UpdateConnectivityInfo, expected 400 / actual 400, $.Message diverges).

## Evidence
Expected (AWS, from the recorded response) vs actual (emulator), quoted from the replay output. Status and error class agree; only the message text diverges.

Cause 1 - greengrassv2-connectivity-info-roundtrip#task#5, UpdateConnectivityInfo (status 400 on both sides):
  $.Message expected 'The following required parameter(s) are unparsable: connectivityInfoList.'
  $.Message actual   'Invalid json format found in the request body.'

Divergence key: service=greengrass|UpdateConnectivityInfo|status=400->400|keys=$.Message (cluster size 1).

Message anatomy: the fixed text is 'The following required parameter(s) are unparsable: ' and the trailing '.'; the variable part is the list of wire parameter names that failed to parse, taken from the request - 'connectivityInfoList' in this recording. No per-run value (thing name, request id, timestamp) appears in the sentence, so the whole string is a requirement once the parameter name is filled in.

## Steps to Reproduce
Boot the emulator and point a greengrass (v1) client at it. <acct>/<region> are the caller's account and region.
1. Create/choose a thing name <thing> and seed connectivity info for it: PUT /greengrass/things/<thing>/connectivityInfo with a well-formed body (ConnectivityInfo list with one entry) - returns 200.
2. Cause 1 (fixture greengrassv2-connectivity-info-roundtrip, step task#5): PUT /greengrass/things/<thing>/connectivityInfo with a syntactically valid JSON body whose connectivity-info list member cannot be deserialized into the modeled list (wrong JSON type for that member / wrong element member types).
3. Expect HTTP 400 BadRequestException with Message 'The following required parameter(s) are unparsable: connectivityInfoList.'
4. Observe HTTP 400 with Message 'Invalid json format found in the request body.' - the parameter that failed is never named.
