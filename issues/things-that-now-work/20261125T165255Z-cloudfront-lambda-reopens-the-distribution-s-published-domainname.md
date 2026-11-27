Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|run#38|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked|run#17|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|run#36|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|run#34|PublishedUrlRejected|cdn_url, route_headers = ctx.endpoint_route(f"{domain}/")
Issue-Key: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected|run#37|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Commit: 6e4695f2e2e0380b6e4530904193a77f767a96ef

# cloudfront__lambda: REOPENS — the distribution's published DomainName is still an unserved <id>.cloudfront.net host, so all five edge-enforcement checks over a Lambda origin never reach the edge

> _Note: this issue was identified against the code base at commit `6e4695f2e2e0380b6e4530904193a77f767a96ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
REOPENS 20261109T181319Z-cloudfront-lambda-edge-enforcement-over-a-lambda-origin.md: its fix left the call refused.

Cause 1 of 1: a Deployed distribution over a Lambda function-URL origin still announces DomainName as <id>.cloudfront.net, an address the emulator does not serve, and no per-distribution listener is published in its place, so not one viewer request in this service group reaches the edge. Five checks die at the same step, before any edge-enforcement assertion runs: allowed-methods enforcement (POST rejected), geo-restriction blocklist, signed-URL source-IP policy denial, origin-access-control signing of a Lambda-URL origin (with the direct call denied), and the Lambda@Edge unpublished-version ($LATEST) association refusal.

AWS: Smithy (cloudfront, 2020-05-31) documents Distribution.DomainName as "The distribution's CloudFront domain name. For example: d111111abcdef8.cloudfront.net", and GetDistribution reports Status "Deployed" only once that name answers viewer traffic. In AWS every behavior these checks assert is observable only through a viewer request to https://<DomainName>/<path>: a method outside AllowedMethods is answered 403 by the edge without reaching the origin; a viewer in a blocked country is answered 403; a signed URL whose policy pins a different source IP is answered 403; an OAC-signed origin accepts the edge's SigV4-signed request while a direct call to the function URL is denied; and a cache behavior associating a $LATEST Lambda@Edge ARN is refused at the control plane while the published-version association serves.

Emulator: the distribution reaches Deployed and publishes DomainName <id>.cloudfront.net (this run: d5ni6znxjb8do0.cloudfront.net, dfgu4i109b1t52.cloudfront.net, dgzni4pnd7h2s3.cloudfront.net, d0lc535g268ho1.cloudfront.net), a host that is not one of the emulator's own hosts and that a customer of the emulator cannot reach. Each check stops with "simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.cloudfront.net[/] (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The host label and the loopback port differ per run; the requirement is only that the announced DomainName is an address the emulator serves.

Why: a client that does exactly what AWS documents — create a distribution, wait for Deployed, then fetch https://<DomainName>/<path> — gets no answer at all. The whole CloudFront edge-enforcement surface over a Lambda origin (method allowlists, geo restriction, signed-URL policies, OAC signing, Lambda@Edge triggers) is unusable and untestable even though the control plane reports a healthy Deployed distribution.

Fix: the CloudFront distribution create/read serializer that mints DomainName, plus the viewer listener it should stand up — the announced DomainName must be an address the emulator serves, and that listener must serve viewer requests by relaying to the configured origin and enforcing AllowedMethods, GeoRestriction, trusted key-group signed-URL policies, OAC request signing and the attached Lambda@Edge triggers.

Related open issues 20261125T055910Z-cloudfront-lambda-2-causes-four-edge-enforcement-scenarios.md and 20261125T035244Z-cloudfront-lambda-the-cloudfront-geo-restriction-blocklist-scenario.md describe the same service group as an edge that times out; neither carries these checks' replay keys.

Traces: the five failing checks listed in trace_ids, all stopping on PublishedUrlRejected for the announced DomainName.

## Evidence
Expected (AWS): a viewer request to https://<DomainName>/<path> on the Deployed distribution is answered by the edge — 403 for a method outside AllowedMethods, 403 for a blocked viewer country, 403 for a signed URL whose policy names another source IP, 200 for the OAC-signed origin fetch while the direct function-URL call is denied — so each enforcement assertion can run.

Actual (emulator): no viewer request is ever issued. Each of the five checks fails with "simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" (observed hosts d5ni6znxjb8do0/dfgu4i109b1t52/dgzni4pnd7h2s3/d0lc535g268ho1.cloudfront.net; ports 39035/37779/38303/34889/43145 — all per-run values).

## Steps to Reproduce
1. Create a Lambda function with a function URL and (for the Lambda@Edge leg) a us-east-1 function with a published version.
2. CreateDistribution over that origin with, in turn: AllowedMethods limited to GET/HEAD; GeoRestriction blocklist; TrustedKeyGroups with a signed-URL policy pinning a source IP; an OriginAccessControl signing the function-URL origin; a cache behavior associating a $LATEST Lambda@Edge ARN. Poll GetDistribution until Status=Deployed.
3. Read Distribution.DomainName from the create/read response.
4. Issue the viewer request to https://<DomainName>/<path> (POST, blocked-country GET, signed URL from another IP, plain GET).
5. Observe there is no listener for the announced host — the viewer request cannot be issued, while AWS answers and enforces it at the edge.
