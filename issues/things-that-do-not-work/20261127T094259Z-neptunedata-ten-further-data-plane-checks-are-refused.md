Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptunedata.cancel-opencypher-query-unknown-uuid-rejected neptunedata.data-plane-parameter-refusals-name-the-rejected-value neptunedata.gremlin-explain-malformed-traversal-rejected neptunedata.gremlin-fail-step-rejected-and-rolls-back neptunedata.job-and-query-list-requests-with-bad-ids-or-flags-rejected neptunedata.list-ml-model-training-jobs-max-items-and-role-arn-rejected neptunedata.ml-job-unknown-id-rejected-by-get-and-cancel neptunedata.propertygraph-statistics-on-burstable-instance-rejected neptunedata.rdf-graph-summary-without-statistics-rejected neptunedata.sparql-statistics-on-burstable-instance-rejected
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata.cancel-opencypher-query-unknown-uuid-rejected|run#4|EndpointConnectionError|answered = nd.execute_open_cypher_query(openCypherQuery="RETURN 'ok' AS state")["results"]
Issue-Key: neptunedata.data-plane-parameter-refusals-name-the-rejected-value|run#3|EndpointConnectionError|listed = graph.list_loader_jobs(limit=limit)
Issue-Key: neptunedata.gremlin-explain-malformed-traversal-rejected|<lambda>#6|EndpointConnectionError|code, message = _refusal(lambda: graph.execute_gremlin_explain_query(gremlinQuery=broken))
Issue-Key: neptunedata.gremlin-fail-step-rejected-and-rolls-back|run#4|EndpointConnectionError|kept = nd.execute_gremlin_query(gremlinQuery=f"g.addV('Kept').property('tok', '{token}')")
Issue-Key: neptunedata.job-and-query-list-requests-with-bad-ids-or-flags-rejected|run#6|EndpointConnectionError|listed = graph.list_loader_jobs()
Issue-Key: neptunedata.list-ml-model-training-jobs-max-items-and-role-arn-rejected|run#2|EndpointConnectionError|at_limit = nd.list_ml_model_training_jobs(maxItems=1024)["ids"]
Issue-Key: neptunedata.ml-job-unknown-id-rejected-by-get-and-cancel|run#3|EndpointConnectionError|before = graph.list_ml_data_processing_jobs()
Issue-Key: neptunedata.propertygraph-statistics-on-burstable-instance-rejected|run#4|EndpointConnectionError|graph.execute_gremlin_query(gremlinQuery=f"g.addV('{label}')")
Issue-Key: neptunedata.rdf-graph-summary-without-statistics-rejected|run#4|EndpointConnectionError|graph.execute_gremlin_query(gremlinQuery=f"g.addV('{label}')")
Issue-Key: neptunedata.sparql-statistics-on-burstable-instance-rejected|run#2|EndpointConnectionError|served = nd.execute_open_cypher_query(openCypherQuery="RETURN 7 AS seven")["results"]
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# neptunedata: ten further data-plane checks are refused at connect on the published cluster endpoint (https://<DBCluster.Endpoint>:8182) — SPARQL/RDF, loader and ML listing, statistics and query cancel/status paths

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteOpenCypherQuery

## Description
THIS ISSUE HAS 1 CAUSE — it must be closed before submitting.

Cause 1 of 1: the address the Neptune control plane publishes for a DB cluster (DBCluster.Endpoint + DBCluster.Port, with Port = 8182) is not served at the TRANSPORT level, so every neptunedata request made to it fails at connect rather than returning an HTTP response. Ten behavioral checks that this replay exercised — none of them marked ALREADY FILED and none marked OWNED by any open issue — die before a single neptunedata operation is answered. Failing check ids for this cause: neptunedata.cancel-opencypher-query-unknown-uuid-rejected, neptunedata.data-plane-parameter-refusals-name-the-rejected-value, neptunedata.gremlin-explain-malformed-traversal-rejected, neptunedata.gremlin-fail-step-rejected-and-rolls-back, neptunedata.job-and-query-list-requests-with-bad-ids-or-flags-rejected, neptunedata.list-ml-model-training-jobs-max-items-and-role-arn-rejected, neptunedata.ml-job-unknown-id-rejected-by-get-and-cancel, neptunedata.propertygraph-statistics-on-burstable-instance-rejected, neptunedata.rdf-graph-summary-without-statistics-rejected, neptunedata.sparql-statistics-on-burstable-instance-rejected.

AWS: neptunedata has no regional, account-wide front door — every one of its operations is addressed at the owning cluster's own connection endpoint on the Neptune engine port. Smithy (services/service-com-amazonaws-neptunedata-amazonneptunedataplane/smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json): service AmazonNeptuneDataplane, sdkId neptunedata, protocol restJson1, sigv4Name/arnNamespace neptune-db; its endpoint rule set resolves host names only (https://neptune-db[-fips].{Region}.{dnsSuffix|dualStackDnsSuffix}) and never a host:port pair, so the address must come from the cluster record. Smithy (services/service-com-amazonaws-neptune-amazonrdsv19): com.amazonaws.neptune#DBCluster member Endpoint — "Specifies the connection endpoint for the primary instance of the DB cluster" (a bare host, no scheme, no port) — and sibling member Port — "Specifies the port that the database engine is listening on", documented Default 8182 for engine=neptune. The requirement that follows: whatever host/port pair the control plane publishes, https://<Endpoint>:<Port> MUST be an address that answers this service's restJson1 API for that cluster. For an emulator that means the published pair has to be an address it actually accepts connections on.

Emulator: the host half resolves (client construction succeeds) and Port is published as 8182, but nothing is bound there. Every request ends in botocore.exceptions.EndpointConnectionError raised from httpsession.py send(): "Could not connect to the endpoint URL: \"https://<published host>:8182/<path>\"". No HTTP status, no error code and no body are produced, because no HTTP exchange takes place. The refused paths across these ten checks cover /opencypher, /opencypher/status/<queryId>, /gremlin, /gremlin/explain, /gremlin/status/<queryId>, /loader, /loader?limit=<n>, /ml/modeltraining?maxItems=<n>, /ml/dataprocessing and /propertygraph/statistics. The loopback host, the emulator's run-assigned listener port and the per-run cluster identifiers vary per run and are not part of the requirement; the requirement is that the published pair be served.

Related but NOT covering these checks: open issue 20261119T200025Z-neptunedata-is-still-unreachable-nothing-serves-the-cluster.md reports the same transport-level root cause and carries the replay keys of nineteen OTHER check ids (this replay marks those nineteen OWNED by it and they are excluded here); the ten ids above are not in its Issue-Trace-Ids and the replay does not mark them owned, so they are not gated by it. Open issue 20261116T010226Z-neptune-dbcluster-projection-publishes-the-emulator-s-listener.md argues the published Port must be the engine constant 8182 (that value is what is published now) and says nothing about anything serving it. Open issues 20261115T144528Z-neptunedata-refusal-paths-diverge-ml-opencypher-status-cancel.md, 20261116T193419Z-neptunedata-modelled-error-bodies-carry-the-numeric-http.md, 20261125T131037Z-neptunedata-executegremlinexplainquery-explain-header-block-diverges-no-blank.md and 20261109T174758Z-neptunedata-data-plane-responses-diverge-from-neptune-engine.md describe divergences in neptunedata RESPONSES, which presupposes a reachable data plane.

Why: DBCluster.Endpoint/Port is the only documented way a client finds a Neptune cluster. A client doing exactly the documented thing — create the cluster and writer, read Endpoint and Port, connect to https://Endpoint:Port — is sent to an address that refuses the connection and sees a transport error it cannot classify, retry or map to any AWS error. SPARQL and RDF summary reads, openCypher/Gremlin query cancel and status lookups, bulk-loader job listing and starting, ML training/data-processing job listing and lookup, and propertygraph statistics are all unusable, and any client that hardcodes a workaround port against the emulator will not work against AWS. Because nothing is exercised, these operations remain ungradable.

Fix: the serving/listener layer that binds the neptunedata restJson1 front door, together with the Neptune control plane's DBCluster endpoint projection and the cluster state that records it — the published Endpoint:Port and the socket the emulator accepts on must be the same address, by binding a per-cluster data-plane listener on the published host and port 8182 that routes each request to the owning cluster's graph state, or by publishing a port the emulator already serves; a second cluster's endpoint must not see the first cluster's data.

Traces: the ten failing check ids listed above, each ending in EndpointConnectionError against https://<published host>:8182/<operation path>.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: Total 0/33 passed (0.0%); neptunedata 0/33 (0%), 33 failed — Mismatched 4, Errored 29, Skipped 0, Unloadable 0. Nineteen of the errored checks are marked "OWNED by 20261119T200025Z-neptunedata-is-still-unreachable-nothing-serves-the-cluster.md" and are excluded from this issue; the four mismatches are filed separately. The ten checks below carry no ownership marker.

All ten end in the identical tail:

  File "endpoint.py", line 385, in _send
    return self.http_session.send(request)
  File "httpsession.py", line 540, in send
    raise EndpointConnectionError(endpoint_url=request.url, error=e)
  botocore.exceptions.EndpointConnectionError: Could not connect to the endpoint URL: "https://127.0.0.1:8182/<path>"

Fixed part of that message: "Could not connect to the endpoint URL" — no HTTP exchange occurred, so there is no status, no error code and no body to compare. Variable parts, not requirements: the loopback host the published Endpoint resolved to, and the per-run cluster/job identifiers in the path or query string.

Refused path per check (path = the operation's modelled HTTP binding):
  - neptunedata.cancel-opencypher-query-unknown-uuid-rejected -> .../opencypher (ExecuteOpenCypherQuery, before CancelOpenCypherQuery/GetOpenCypherQueryStatus are ever reached)
  - neptunedata.data-plane-parameter-refusals-name-the-rejected-value -> .../loader?limit=100 (ListLoaderJobs)
  - neptunedata.gremlin-explain-malformed-traversal-rejected -> .../gremlin/explain (ExecuteGremlinExplainQuery)
  - neptunedata.gremlin-fail-step-rejected-and-rolls-back -> .../gremlin (ExecuteGremlinQuery)
  - neptunedata.job-and-query-list-requests-with-bad-ids-or-flags-rejected -> .../loader (ListLoaderJobs)
  - neptunedata.list-ml-model-training-jobs-max-items-and-role-arn-rejected -> .../ml/modeltraining?maxItems=1024 (ListMLModelTrainingJobs)
  - neptunedata.ml-job-unknown-id-rejected-by-get-and-cancel -> .../ml/dataprocessing (ListMLDataProcessingJobs)
  - neptunedata.propertygraph-statistics-on-burstable-instance-rejected -> .../gremlin (ExecuteGremlinQuery, before ManagePropertygraphStatistics)
  - neptunedata.rdf-graph-summary-without-statistics-rejected -> .../gremlin (ExecuteGremlinQuery, before GetRDFGraphSummary)
  - neptunedata.sparql-statistics-on-burstable-instance-rejected -> .../opencypher (ExecuteOpenCypherQuery, before ManageSparqlStatistics)

Each check reaches that point only because EC2 (CreateSecurityGroup, AuthorizeSecurityGroupIngress) and the Neptune control plane (CreateDBSubnetGroup, CreateDBCluster, CreateDBInstance, DescribeDBInstances, DescribeDBClusters) all answered — so the emulator IS serving; it is just not serving the address it published. Each check then builds its client from the published values:
  cluster = neptune.describe_db_clusters(DBClusterIdentifier=name)["DBClusters"][0]
  client  = ctx.client("neptunedata", endpoint_url=ctx.endpoint_url(f"{cluster['Endpoint']}:{cluster['Port']}"))
with Port published as 8182; the URL is accepted and the subsequent TCP connect to 8182 is refused.

Expected: https://<DBCluster.Endpoint>:<DBCluster.Port> reaches the emulator's restJson1 neptunedata front door for that cluster, and each check is then graded on its real assertions — e.g. CancelOpenCypherQuery/GetOpenCypherQueryStatus for a well-formed UUID naming no query answer HTTP 400 InvalidParameterException with message "Supplied queryId <queryId> is invalid" (fixed text "Supplied queryId " + " is invalid"; the queryId is the value taken from the request), ListMLModelTrainingJobs accepts maxItems at the documented ceiling 1024, and ListLoaderJobs answers 200 with a loadIds list.
Actual: no neptunedata request is answered at all — the connection to the published port is refused, so all ten checks error out at connect.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout, and replay the simparity-behavioral-aws-v1 behavioral suite.
2. Observe neptunedata 0/33, with the ten check ids in trace_ids erroring with botocore EndpointConnectionError against https://<published Endpoint host>:8182/<path>, and no HTTP response produced for any of them.
3. Reproduce by hand: CreateSecurityGroup + AuthorizeSecurityGroupIngress on port 8182 in the private subnets, CreateDBSubnetGroup over those subnets, CreateDBCluster with Engine=neptune, StorageEncrypted=true, EnableIAMDatabaseAuthentication=true, then CreateDBInstance with DBInstanceClass=db.t4g.medium, PubliclyAccessible=false, and wait for the writer to report DBInstanceStatus 'available'.
4. Read DBCluster.Endpoint and DBCluster.Port from DescribeDBClusters (Endpoint port-less, Port 8182 today) and open a TCP connection to <Endpoint>:<Port>. It must be accepted; today the connect is refused because the emulator listens only on its run-assigned port.
5. Point a neptunedata client at https://<Endpoint>:<Port> and call ListLoaderJobs(limit=100): it must answer HTTP 200 with a payload.loadIds list.
6. Against the same endpoint call GetOpenCypherQueryStatus with a well-formed UUID that names no query: it must answer HTTP 400 InvalidParameterException with message "Supplied queryId <that uuid> is invalid".
7. Confirm the route is bound to the owning cluster: ExecuteOpenCypherQuery "CREATE (n:Probe {k:'v'}) RETURN n", read it back with a MATCH, and confirm a second cluster's endpoint does not see the first cluster's data.
8. Re-replay the ten check ids in trace_ids to confirm the fix — each must get past the connect and then be graded on its actual neptunedata assertions.
