Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.cidr-collection-lifecycle
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.cidr-collection-lifecycle|run#14|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# Route 53 ChangeCidrCollection CidrCollectionVersionMismatchException carries the error shape's documentation text as its Message; AWS sends it empty

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeCidrCollection

## Description
One net-new cause.

Cause 1 of 1: a ChangeCidrCollection request whose CollectionVersion does not match the collection's current version is refused with the right class but the wrong Message body. Failing check: route53.cidr-collection-lifecycle.

AWS: Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) models ChangeCidrCollection with error CidrCollectionVersionMismatchException (client error, HTTP 409) whose only member is `Message`. Recorded behavior: real Route 53 answers this refusal with error Code `CidrCollectionVersionMismatchException` and an EMPTY Message — botocore surfaces Error.Message as "". The error shape's `doc` string ("The CIDR collection version you provided, doesn't match the one in the ListCidrCollections operation.") is API documentation of the error class, not the message AWS puts on the wire for it.

Emulator: the refusal is classified correctly (code CidrCollectionVersionMismatchException) but its Message is populated with that documentation sentence verbatim: "The CIDR collection version you provided, doesn't match the one in the ListCidrCollections operation." The whole string is fixed, emulator-authored text — it carries no value from the request (no collection id, no asked-for or current version), which is itself a sign it was lifted from the model's doc rather than reproduced from a recording.

Why: a client that compares, logs, or matches on the refusal Message sees a body AWS never sends. Callers that key optimistic-concurrency retry on an empty-vs-populated Message, and test suites that assert the exact refusal payload, diverge on an otherwise correctly classified 409. The same class of defect — a doc string or emulator prose standing in for AWS's message — is already filed for other Route 53 refusals (20261109T063005Z-route53-route-53-nosuchtrafficpolicy-is-answered-with-an-empty.md covers the inverse for NoSuchTrafficPolicy, and 20261103T020342Z-route53-route-53-refusal-payloads-diverge-publiczonevpcassociation-message-is.md covers emulator-authored PublicZoneVPCAssociation text); neither issue covers this operation or this error shape.

Fix: the version-mismatch refusal path of the ChangeCidrCollection handler and the message constant it uses (the `VERSION_MISMATCH_MESSAGE` it passes as `CidrCollectionVersionMismatchException { message }`). The refusal must be serialized with no Message content for this class, leaving status 409 and the code unchanged.

Traces: route53.cidr-collection-lifecycle. Note that the same check later asserts the NoSuchCidrCollectionException refusal on a deleted collection also carries an empty Message (the handler builds it from a `no_such_cidr_collection()` helper); that assertion was never reached because the run aborted at the version-mismatch Message, so it is not part of this report's evidence.

## Evidence
Check route53.cidr-collection-lifecycle, line 86:
  assert message == "", f"expected an empty message, got {message!r}"
  AssertionError: expected an empty message, got "The CIDR collection version you provided, doesn't match the one in the ListCidrCollections operation."

Expected (recorded AWS): ClientError Error.Code == "CidrCollectionVersionMismatchException" (HTTP 409), Error.Message == "".
Actual (emulator): Error.Code == "CidrCollectionVersionMismatchException", Error.Message == "The CIDR collection version you provided, doesn't match the one in the ListCidrCollections operation."

That actual string is byte-identical to the `doc` of com.amazonaws.route53#CidrCollectionVersionMismatchException in the service's Smithy model, and is emitted at services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_cidr_collection.rs:251-263 as `CidrCollectionVersionMismatchException { message: Some(VERSION_MISMATCH_MESSAGE.to_string()) }`.

## Steps to Reproduce
1. route53 CreateCidrCollection(Name=<unique>, CallerReference=<uuid>) -> note Collection.Id.
2. ListCidrCollections -> note the collection's Version (call it V).
3. ChangeCidrCollection(Id=<id>, Changes=[{LocationName: "loc-a", Action: "PUT", CidrList: ["10.0.0.0/24"]}]) with no CollectionVersion — succeeds and bumps the version past V.
4. ChangeCidrCollection(Id=<id>, Changes=[{LocationName: "loc-b", Action: "PUT", CidrList: ["10.0.1.0/24"]}], CollectionVersion=V).
5. Inspect the ClientError: the code is CidrCollectionVersionMismatchException as AWS sends it, but Error.Message is the documentation sentence instead of the empty string AWS returns.
