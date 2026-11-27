Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol
Issue-Key: apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol|run#22|AssertionError|assert labels == (api_id, "execute-api", ctx.region), (
Issue-Commit: bac79ba9101886d75f66581df23a29a106fe59e8

# apigatewayv2 GetApi publishes ApiEndpoint as a path-style loopback URL (https://<host>:<port>/_aws/execute-api/<api-id>), so it carries none of the documented {api-id}.execute-api.{region} host labels

> _Note: this issue was identified against the code base at commit `bac79ba9101886d75f66581df23a29a106fe59e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApi

## Description
One net-new cause (Cause 1 of 1): the apigatewayv2 Api view's `ApiEndpoint` is serialized as a path-style URL on the emulator's own listener authority, with the API id carried in the PATH rather than in the host's leading labels.

Cause 1 of 1 — GetApi (and every response that serializes the `Api` view; observed here through GetApi after a CloudFormation AWS::ApiGatewayV2::Api create whose Fn::GetAtt ApiEndpoint agreed with GetApi, so the same derivation feeds both).

AWS: Smithy apigatewayv2, member doc on Api$ApiEndpoint — "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." The requirement is therefore structural: an absolute URI whose hostname's leading dot-separated labels are, in order, (1) the API's own id — the same value returned in `ApiId` and the value CloudFormation returns for `Ref` — (2) the literal label `execute-api`, and (3) the region, with the stage and route appended as PATH segments after the host. GetApi is HTTP GET /v2/apis/{ApiId}, success status 200; the recorded response status (200) agrees and no error is involved — only the member's value diverges. The concrete api id, the DNS suffix, the port and the region in any one run are per-run environment values and are not part of the requirement; the label ORDER and the fact that the id/`execute-api`/region live in the HOST are.

Emulator: GetApi answers 200 but publishes `https://127.0.0.1:44343/_aws/execute-api/mp67t4dbd4` — i.e. `https://<listener-host>:<listener-port>/_aws/execute-api/<api-id>`. Splitting that URI's hostname on '.' yields the leading labels ('127','0','0') instead of (<api-id>, 'execute-api', <region>): the api id and the `execute-api` label have been moved out of the host into a `/_aws/execute-api/<api-id>` path prefix, and the region label is absent entirely. The check's assertion message is: "GetApi ApiEndpoint 'https://127.0.0.1:44343/_aws/execute-api/mp67t4dbd4' does not carry the api id, execute-api and the region as its leading host labels: ('127', '0', '0')".

Why: a client that follows the documented contract treats `ApiEndpoint` as the base URL of the deployed API and (a) appends `/<stage>/<route>` to it, which against the emulator lands under the `/_aws/execute-api/<api-id>` prefix only by accident of ordering and breaks any client that parses the URI, and (b) derives the SigV4 signing service and region from the endpoint host's labels — with a bare loopback IP there is no `execute-api` label and no region label to derive, so request signing for the execute-api data plane cannot be constructed at all. CloudFormation consumers are affected identically because Fn::GetAtt ApiEndpoint reports the same string.

Fix: the single apigatewayv2 Api-endpoint derivation used when serializing the `Api` view (the helper feeding the Get/Create/Update/List/Import/Reimport response serializers, and through it the CloudFormation AWS::ApiGatewayV2::Api ApiEndpoint attribute) must put the api id, the literal `execute-api` label and the region in the published host's leading labels on a suffix the emulator's execute-api data plane actually answers on, instead of emitting the listener authority plus a `/_aws/execute-api/<api-id>` path prefix.

Related but NOT covering: open issue 20261126T153221Z-apigatewayv2-api-views-publish-apiendpoint-on-an-unserved.md and the 20261126T140544Z-*/20261126T140244Z-* issues describe an `<api-id>.execute-api.<region>.localhost:<port>` endpoint — a host that DOES carry the three documented leading labels and diverges only in reachability. The form observed here carries NONE of those labels (path-style loopback), so those issues' requirement is not the one failing, and the replay reports this check as unowned. Open issue 20261125T063702Z-apigateway-createapi-and-reimportapi-publish-apiendpoint-as-a.md reports the same path-style shape under the `apigateway` (v1) service token, not apigatewayv2.

Traces: behavioral check apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol (assertion at line 155 of the check).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 165/176 passed. Service heading `### apigatewayv2__cloudcontrol__cloudformation`: 1/2 (50%), 1 failed.

[mismatch] apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol

  File "check_apigatewayv2__cloudcontrol__cloudformation_cfn_apigatewayv2_identity_readonly_ignored_writeonly_body_hidden_from_cloudcontrol.py", line 121, in run
    assert labels == (api_id, "execute-api", ctx.region), (
  AssertionError: GetApi ApiEndpoint 'https://127.0.0.1:44343/_aws/execute-api/mp67t4dbd4' does not carry the api id, execute-api and the region as its leading host labels: ('127', '0', '0')

Earlier assertions in the same check PASSED, which localizes the fault:
  - stack reached CREATE_COMPLETE;
  - GetApi(ApiId=<Ref>) returned ApiId == the CFN Ref value;
  - Fn::GetAtt ApiEndpoint == GetApi's own ApiEndpoint (so both come from one derivation);
  - Fn::GetAtt ApiEndpoint != the template's bogus ApiEndpoint override.

Expected (AWS): urlsplit(ApiEndpoint).hostname.split('.')[:3] == (<ApiId>, 'execute-api', <region>) — e.g. host `<api-id>.execute-api.<region>.amazonaws.com`, with no api id in the path.
Actual (emulator): hostname is the bare listener IP, the api id and `execute-api` appear as the path `/_aws/execute-api/<api-id>`, and the leading host labels are the dotted-quad octets ('127','0','0').

Status/error: none diverges — GetApi answered 200 as recorded; the divergence is confined to the ApiEndpoint member's value. The api id `mp67t4dbd4`, the host `127.0.0.1` and the port `44343` in the message above are per-run values, not requirements.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral check apigatewayv2__cloudcontrol__cloudformation.cfn-apigatewayv2-identity-readonly-ignored-writeonly-body-hidden-from-cloudcontrol.

Or by hand:
1. cloudformation.create_stack with a template containing one resource of Type AWS::ApiGatewayV2::Api whose Properties carry an OpenAPI `Body` (and, optionally, a bogus read-only `ApiEndpoint` override), plus Outputs {"ApiRef": {"Ref": "Api"}, "ApiEndpointOut": {"Fn::GetAtt": ["Api","ApiEndpoint"]}}; wait for CREATE_COMPLETE.
2. Read the `ApiRef` output as <apiId>, then call apigatewayv2.get_api(ApiId=<apiId>).
3. Evaluate urllib.parse.urlsplit(response["ApiEndpoint"]).hostname.split(".")[:3].
   Expected: (<apiId>, "execute-api", <region>).
   Actual: the first three octets of the emulator's loopback listener address, because ApiEndpoint is published as https://<listener-host>:<listener-port>/_aws/execute-api/<apiId>.
4. The same string is returned for the stack's Fn::GetAtt ApiEndpoint output, so a CloudFormation consumer sees the identical divergence.
