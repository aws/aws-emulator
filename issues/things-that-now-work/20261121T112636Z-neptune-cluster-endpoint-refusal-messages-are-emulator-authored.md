Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-cluster-endpoint-rejection-classes
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-cluster-endpoint-rejection-classes neptune-db-cluster-lifecycle-and-custom-endpoint
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|DescribeDBClusterEndpoints|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBClusterEndpoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|ModifyDBClusterEndpoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|DeleteDBClusterEndpoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 3f1d064d908eb4a469d73223dfef0ca475b0d2e0

# Neptune cluster-endpoint refusal messages are emulator-authored not AWS wording

> _Note: this issue was identified against the code base at commit `3f1d064d908eb4a469d73223dfef0ca475b0d2e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterEndpoints

## Description
Every refusal path of Neptune's cluster-endpoint operations returns the right HTTP status and the right awsQuery error code, but the Error.Message body is text the emulator authored itself rather than the message Neptune serves for that error class. A client that matches, logs or surfaces the refusal text sees a different sentence from AWS on all five paths. This bundle carries 5 CAUSES; all five must be closed.

Cause 1 of 5: DescribeDBClusterEndpoints on a DB cluster that does not exist.
AWS: Smithy: DescribeDBClusterEndpoints errors DBClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code DBClusterNotFoundFault / httpResponseCode 404). The model does not fix the message text, so recorded behavior only: the recorded 404 ErrorResponse.Error.Message is "The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>" — the fixed text is "The source cluster could not be found or cannot be accessed: " and the only variable part is the DBClusterIdentifier taken verbatim from the request.
Emulator: answers 404 with Error.Message "DBCluster <DBClusterIdentifier> not found." — the generic cluster-lookup message used by DescribeDBClusters, not the cluster-endpoint family's wording.
Why: a client relying on AWS's text (log scraping, user-facing error surfacing, message-based test assertions on the "source cluster" phrasing) never sees it; the two messages share no fixed text at all.
Fix: the DescribeDBClusterEndpoints handler's cluster-resolution/refusal path — the DBClusterNotFoundFault it raises must carry the cluster-endpoint family's message template instead of reusing the shared cluster-not-found message.
Traces: neptune-cluster-endpoint-rejection-classes#task#2 (also observed at neptune-db-cluster-lifecycle-and-custom-endpoint#task#12).

Cause 2 of 5: CreateDBClusterEndpoint against a DB cluster that does not exist.
AWS: Smithy: CreateDBClusterEndpoint errors DBClusterNotFoundFault (client, httpError 404, awsQueryError DBClusterNotFoundFault/404). Recorded behavior only for the text: Error.Message is "The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>", fixed text as in Cause 1 with the identifier taken from the request.
Emulator: answers 404 with Error.Message "DBCluster <DBClusterIdentifier> not found."
Why: same as Cause 1 — the create path's refusal text is not AWS's, so message-sensitive clients misclassify the failure.
Fix: the CreateDBClusterEndpoint handler's validation/refusal path.
Traces: neptune-cluster-endpoint-rejection-classes#task#3.

Cause 3 of 5: ModifyDBClusterEndpoint naming a cluster endpoint that does not exist.
AWS: Smithy: ModifyDBClusterEndpoint errors DBClusterEndpointNotFoundFault (client, httpError 400, awsQueryError DBClusterEndpointNotFoundFault/400). Recorded behavior only for the text: Error.Message is "DBClusterEndpoint <DBClusterEndpointIdentifier> not found" — fixed text "DBClusterEndpoint " + " not found", the identifier taken verbatim from the request.
Emulator: answers 400 with Error.Message "The specified custom endpoint doesn't exist." — a constant sentence that never names the endpoint the caller asked for.
Why: the emulator's message omits the variable part entirely, so a client cannot tell from the refusal which endpoint identifier was rejected, and the fixed text does not match AWS's.
Fix: the ModifyDBClusterEndpoint handler's endpoint-lookup refusal path.
Traces: neptune-cluster-endpoint-rejection-classes#task#4.

Cause 4 of 5: DeleteDBClusterEndpoint naming a cluster endpoint that does not exist.
AWS: Smithy: DeleteDBClusterEndpoint errors DBClusterEndpointNotFoundFault (client, httpError 400, awsQueryError DBClusterEndpointNotFoundFault/400). Recorded behavior only for the text: Error.Message "DBClusterEndpoint <DBClusterEndpointIdentifier> not found", built as in Cause 3.
Emulator: answers 400 with Error.Message "The specified custom endpoint doesn't exist."
Why: as Cause 3 — constant text, endpoint identifier never echoed.
Fix: the DeleteDBClusterEndpoint handler's endpoint-lookup refusal path.
Traces: neptune-cluster-endpoint-rejection-classes#task#5.

Cause 5 of 5: CreateDBClusterEndpoint with a syntactically invalid DBClusterEndpointIdentifier.
AWS: Smithy: CreateDBClusterEndpoint.DBClusterEndpointIdentifier is a required String with no modelled pattern, so recorded behavior only: the recorded 400 refusal carries Error.Message "Invalid cluster endpoint identifier:  <DBClusterEndpointIdentifier>" — fixed text "Invalid cluster endpoint identifier: " followed by the rejected identifier taken verbatim from the request (the recording shows the two spaces before the value).
Emulator: answers 400 with the RDS-style generic identifier message "The parameter DBClusterEndpointIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens."
Why: Neptune's cluster-endpoint validator reports a short, value-echoing message; the emulator substitutes the generic DB-identifier prose, so a client keying on the "Invalid cluster endpoint identifier" class sees an unrelated sentence.
Fix: the CreateDBClusterEndpoint handler's identifier validation.
Traces: neptune-cluster-endpoint-rejection-classes#task#6 (also observed at neptune-db-cluster-lifecycle-and-custom-endpoint#task#8).

Related open issues (none of which carries these replay keys): 20261117T150412Z-neptune-3-net-new-causes-createdbcluster-ignores-the.md and 20261119T170405Z-neptune-2-net-new-port-causes-a-db.md report the same emulator-authored-message family for CreateDBCluster/ModifyDBCluster port and cluster-not-empty refusals; the five causes above are the cluster-endpoint operations and are net-new.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 90.6%, 17 mismatched steps):

1. fixture neptune-cluster-endpoint-rejection-classes step task#2, DescribeDBClusterEndpoints, expected 404 / actual 404: $.ErrorResponse.Error.Message value_mismatch — expected 'The source cluster could not be found or cannot be accessed: sim2real-probe-neptune-missing-7c1a9e42', got 'DBCluster sim2real-probe-neptune-missing-7c1a9e42 not found.' (cluster also affected: neptune-db-cluster-lifecycle-and-custom-endpoint#task#12)
2. fixture neptune-cluster-endpoint-rejection-classes step task#3, CreateDBClusterEndpoint, expected 404 / actual 404: same Error.Message value_mismatch, expected 'The source cluster could not be found or cannot be accessed: <id>', got 'DBCluster <id> not found.'
3. fixture neptune-cluster-endpoint-rejection-classes step task#4, ModifyDBClusterEndpoint, expected 400 / actual 400: Error.Message expected 'DBClusterEndpoint sim2real-probe-neptune-missingep-7c1a9e42 not found', got "The specified custom endpoint doesn't exist."
4. fixture neptune-cluster-endpoint-rejection-classes step task#5, DeleteDBClusterEndpoint, expected 400 / actual 400: Error.Message expected 'DBClusterEndpoint <id> not found', got "The specified custom endpoint doesn't exist."
5. fixture neptune-cluster-endpoint-rejection-classes step task#6, CreateDBClusterEndpoint, expected 400 / actual 400: Error.Message expected 'Invalid cluster endpoint identifier:  Bad_Endpoint_Name!', got 'The parameter DBClusterEndpointIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' (cluster also affected: neptune-db-cluster-lifecycle-and-custom-endpoint#task#8)

In every one of the five the HTTP status and the error code agree with the recording; only Error.Message diverges. The identifiers quoted above are this recording's values filling the templates described in the issue, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: POST the Neptune query-protocol Action=DescribeDBClusterEndpoints with DBClusterIdentifier naming a cluster that does not exist — fixture neptune-cluster-endpoint-rejection-classes, step task#2. Observe 404 DBClusterNotFoundFault with Error.Message 'DBCluster <id> not found.' instead of 'The source cluster could not be found or cannot be accessed: <id>'.
3. Cause 2: Action=CreateDBClusterEndpoint with that same non-existent DBClusterIdentifier plus a valid DBClusterEndpointIdentifier and EndpointType=READER — step task#3. Same message divergence.
4. Cause 3: Action=ModifyDBClusterEndpoint with a DBClusterEndpointIdentifier that does not exist — step task#4. Observe 400 DBClusterEndpointNotFoundFault with "The specified custom endpoint doesn't exist." instead of 'DBClusterEndpoint <id> not found'.
5. Cause 4: Action=DeleteDBClusterEndpoint with that same non-existent endpoint identifier — step task#5. Same message divergence.
6. Cause 5: Action=CreateDBClusterEndpoint on an existing cluster with a DBClusterEndpointIdentifier containing underscores/uppercase/'!' — step task#6. Observe the generic RDS identifier prose instead of 'Invalid cluster endpoint identifier:  <value>'.
7. Re-replay neptune-cluster-endpoint-rejection-classes (task#2..task#6) and neptune-db-cluster-lifecycle-and-custom-endpoint (task#8, task#12) to confirm all five causes.
