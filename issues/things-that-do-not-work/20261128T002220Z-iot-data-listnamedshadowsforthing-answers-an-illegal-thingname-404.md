Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-shadow-unrouted-verbs-and-listing-guards
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-shadow-unrouted-verbs-and-listing-guards
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->4xx|status
Issue-Related: 20261128T002112Z-iot-data-request-validation-refusals-carry-generic-or.md
Issue-Commit: e49fca9f5a8024b42dd64362f9cea4015f58fd68

# iot-data ListNamedShadowsForThing answers an illegal thingName 404 ResourceNotFoundException instead of 400 Invalid ThingName

> _Note: this issue was identified against the code base at commit `e49fca9f5a8024b42dd64362f9cea4015f58fd68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNamedShadowsForThing

## Description
This issue carries the replay pointer for one cause of the bundle "iot-data request-validation refusals carry generic or emulator-authored messages and the wrong error class" (20261128T002112Z-iot-data-request-validation-refusals-carry-generic-or.md), whose Cause 7 was filed with a mistyped fixture id and therefore has no usable replay binding. The defect itself is net-new and unowned.

Cause 1 of 1: ListNamedShadowsForThing answers an invalid thingName 404 ResourceNotFoundException instead of a 400 validation refusal.
AWS: Smithy: ListNamedShadowsForThing.thingName targets ThingName as a required @httpLabel with @length min 1 max 128 and @pattern ^[a-zA-Z0-9:_-]+$; the operation declares both InvalidRequestException (errorKind client, httpError 400) and ResourceNotFoundException (errorKind client, httpError 404). A thingName that violates the pattern is a malformed request, not a missing resource, so AWS refuses it 400 with the message "Invalid ThingName: \"<thingName>\"" — the fixed text is "Invalid ThingName: " and the surrounding double quotes; the only variable part is the thing name taken verbatim from the request path. 404 ResourceNotFoundException is reserved for a name that satisfies the pattern but names no registered thing.
Emulator: answers status 404 with header x-amzn-errortype "ResourceNotFoundException" and the emulator-authored message "Invalid thing name specified in request" — the wrong status, the wrong error class, and a message that is neither AWS's wording nor echoes the offending name.
Why: a client cannot distinguish "the thing name you sent is syntactically illegal" (fix the request, never retry) from "that thing does not exist" (provision it, or stop polling). Provisioning and retry logic branch on exactly that distinction, and a name-construction bug in the client is reported as a missing device. The constraint is also the model's own @pattern, so an SDK that validates client-side and a client that does not get different answers for the same call.
Fix: ListNamedShadowsForThing's input validation — apply the ThingName @length/@pattern check before the shadow-registry lookup and raise the modeled InvalidRequestException with AWS's message.
Traces: iotdata-shadow-unrouted-verbs-and-listing-guards#task#7.

## Evidence
iotdata-shadow-unrouted-verbs-and-listing-guards#task#7 (operation ListNamedShadowsForThing): status expected 400, got 404; $.message value_mismatch — expected 'Invalid ThingName: "verb!thing"', got 'Invalid thing name specified in request'; $headers.x-amzn-errortype shape_mismatch — expected shaped-not-asserted, got 'ResourceNotFoundException'. Divergence key service=iot-data|ListNamedShadowsForThing|status=400->404|keys=$.message,$headers.x-amzn-errortype,status; 1 affected step.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture iotdata-shadow-unrouted-verbs-and-listing-guards step task#7. By hand against the data listener: GET /api/things/shadow/ListNamedShadowsForThing/verb!thing (a thing name containing a character outside ^[a-zA-Z0-9:_-]+$). Expect 400 with body message 'Invalid ThingName: "verb!thing"'; the emulator answers 404 with x-amzn-errortype ResourceNotFoundException and the message 'Invalid thing name specified in request'.
