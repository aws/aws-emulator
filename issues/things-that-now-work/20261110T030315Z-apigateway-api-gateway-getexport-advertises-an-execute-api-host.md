Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-deploy-and-export-api-definition
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-deploy-and-export-api-definition apigateway-documentation-parts-versions-and-export apigateway-import-configure-deploy-export-sdk apigateway-import-rest-api-deploy-export apigateway-put-rest-api-merge-then-overwrite apigateway-rest-api-import-export-and-put apigateway-rest-api-import-export-put
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetExport|2xx->2xx|$.host
Issue-Key: apigateway|GetExport|2xx->2xx|$.servers[*].url
Issue-Commit: c01090bc0e97483ffdf7a7efd2d0a67727486686

# API Gateway GetExport advertises an execute-api host the emulator does not serve

> _Note: this issue was identified against the code base at commit `c01090bc0e97483ffdf7a7efd2d0a67727486686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
2 causes, one per export format. The exported API definition names the API's AWS execute-api hostname as the endpoint clients should call, but the emulator does not answer on that address - it is reached on its own host/port. The replay flags both as host_not_own.

Cause 1 of 2 - GetExport with exportType 'swagger' (Swagger 2.0): the document's top-level 'host' field.
AWS: 200, and 'host' is the hostname at which that deployed stage actually serves traffic - '<restApiId>.execute-api.<region>.amazonaws.com' on AWS. Smithy: GetExport GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, output ExportResponse with contentType/contentDisposition headers and body as the @httpPayload blob; the model says nothing about the document's contents, so for the field's meaning this is recorded behavior only - the recording shows the host of the endpoint that serves the API.
Emulator: 200 with an export whose 'host' is an execute-api.<region>.amazonaws.com name that this emulator is not reachable on (host_not_own), rather than the host the request arrived on.
Why: the export is the artifact clients feed to code generators, mock servers and API clients. Every client built from the emulator's export points at the real AWS domain, so requests leave the emulator entirely (or fail to resolve) instead of reaching the emulated API.
Fix: the GetExport serializer's host synthesis - derive the advertised host from the endpoint the emulator serves (the request's Host/configured data listener), not from a hard-coded execute-api template.
Traces: apigateway-deploy-and-export-api-definition#task#5 (5 steps in the cluster, across apigateway-documentation-parts-versions-and-export, apigateway-import-configure-deploy-export-sdk, apigateway-import-rest-api-deploy-export and apigateway-put-rest-api-merge-then-overwrite).

Cause 2 of 2 - the same defect in the OpenAPI 3 export: servers[0].url.
AWS: 200 with servers[0].url = 'https://<host serving the stage>/{basePath}'. Recorded behavior only (the Smithy model does not describe the payload document); the recording shows the serving endpoint followed by the /{basePath} template.
Emulator: 200 with servers[0].url built on an execute-api.<region>.amazonaws.com host it does not serve (host_not_own), so the generated base URL is unreachable.
Why: same as cause 1, for every OAS3 consumer - the url is what an SDK or mock generator uses as the base URL.
Fix: the same host synthesis path, applied to the oas30 servers[] renderer.
Traces: apigateway-rest-api-import-export-and-put#task#5 (2 steps in the cluster, also apigateway-rest-api-import-export-put#task#5).

Note on values: the concrete 'jho0tn34ri.execute-api.us-west-2.amazonaws.com' string in the recording is a per-run host and is not itself the requirement; the requirement is that the exported document advertises an endpoint the emulator answers on.

## Evidence
Cause 1 (GetExport swagger, apigateway-deploy-and-export-api-definition#task#5): $.host - verdict host_not_own; the emulator emits '<restApiId>.execute-api.<region>.amazonaws.com', a host it does not serve. Status 200 on both sides. Cluster also covers apigateway-documentation-parts-versions-and-export#task#17, apigateway-import-configure-deploy-export-sdk#task#12, apigateway-import-rest-api-deploy-export#task#4, apigateway-put-rest-api-merge-then-overwrite#task#6.
Cause 2 (GetExport oas30, apigateway-rest-api-import-export-and-put#task#5): $.servers[0].url - verdict host_not_own for 'https://<restApiId>.execute-api.<region>.amazonaws.com/{basePath}'. Status 200 on both sides. Cluster also covers apigateway-rest-api-import-export-put#task#5.

## Steps to Reproduce
1. Replay the corpus: replay_wire_corpus().
2. Cause 1: create and deploy a REST API, then GET /restapis/{restApiId}/stages/{stageName}/exports/swagger (fixture apigateway-deploy-and-export-api-definition, step task#5). Inspect the returned document's 'host' field: it names an execute-api AWS domain rather than the host the emulator is reached on.
3. Cause 2: GET .../exports/oas30 on an imported-then-exported API (fixture apigateway-rest-api-import-export-and-put, step task#5) and inspect servers[0].url: same unreachable host, with the /{basePath} suffix.
4. Point any generated client at either advertised endpoint: it does not reach the emulator.
