Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused
Issue-Key: cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused|run#10|PublishedUrlRejected|url, route = ctx.endpoint_route(f"{dist['DomainName']}/{KEY}")
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudfront__s3: a distribution's DomainName is published as <id>.cloudfront.net, which the emulator does not serve, so a response-headers-policy CORS verdict can never be observed at the edge

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One cause.

Cause 1 of 1: CloudFront publishes an edge hostname that is not one of the emulator's own hosts, so the viewer request never reaches it.

AWS: Distribution.DomainName is the address a viewer fetches from; GET/OPTIONS https://<DomainName>/<path> reaches the distribution, the attached response-headers policy adds the configured Access-Control-* headers for a granted origin/method, and a method the policy does not allow is refused at the edge. Smithy: cloudfront CreateDistribution/GetDistribution output Distribution.DomainName; ResponseHeadersPolicy CorsConfig carries AccessControlAllowOrigins/AccessControlAllowMethods.

Emulator: DomainName is minted in AWS's public shape (recorded as a '<id>.cloudfront.net' host, with the object path appended by the check) while the emulator answers only on its own host/port: 'endpoint_url was given https://<id>.cloudfront.net/app/config.json (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The AWS-shaped host is the fixed part of the defect; the distribution id, path and port vary per run.

Why: a client that follows the documented contract — read DomainName, fetch it — cannot reach the emulator, so the entire CloudFront edge (cache behavior, response-headers policies, CORS grants and refusals) is unverifiable while the control plane reports the distribution Deployed.

Fix: the CloudFront distribution state/serializer must publish a DomainName the emulator serves (its own host, or a per-distribution listener), and the CloudFront data plane must route requests for that name through the configured behaviors and response-headers policy.

Traces: cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused. Open issues 20261109T181319Z-cloudfront-s3-the-s3-origin-edge-is-unreachable.md and 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md describe neighbouring symptoms but do not carry this check's replay key.

## Evidence
Expected: a GET through the distribution with an allowed Origin carries the policy's Access-Control-Allow-Origin/-Methods headers; a method the policy disallows is refused at the edge.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://d5ni6znxjb8do0.cloudfront.net/app/config.json (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34549/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

## Steps to Reproduce
1. Create an S3 origin bucket with an object at app/config.json.
2. cloudfront.create_response_headers_policy(...) with a CORS config granting an origin and GET.
3. cloudfront.create_distribution(...) attaching that policy to the default cache behavior.
4. Fetch https://<Distribution.DomainName>/app/config.json with the granted Origin -> the published host is not one the emulator answers on.
