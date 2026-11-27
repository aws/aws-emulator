Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-create-distribution cloudfront.op-contract-get-distribution cloudfront.distribution-create-deploy-lifecycle cloudfront.distribution-get-vs-get-config-shape-parity cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution cloudfront.cf-fetch-through-distribution cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected cloudfront.cf-wildcard-invalidation-clears-only-matching-objects cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag cloudfront.cfcache-cookie-allowlist-separates-cache-entries cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache cloudfront.continuous-deployment-policy-disabled-suppresses-header-routing cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header cloudfront.deploy-cache-policy-cfn-enforced-on-distribution cloudfront.deploy-cache-policy-cfn-ttl-enforced cloudfront.deploy-cache-policy-enforced-via-cloudformation cloudfront.deploy-cloudfront-distribution-serves-origin-content cloudfront.deploy-distribution-cfn-serves-live-content cloudfront.deploy-distribution-cfn-serves-live-traffic cloudfront.deploy-distribution-serves-content-via-cloudformation cloudfront.e2e-distribution-invalidation-cache-refresh cloudfront.e2e-distribution-webacl-association-blocks-request cloudfront.e2e-invalidation-clears-cached-content cloudfront.e2e-invalidation-clears-cached-object cloudfront.e2e-invalidation-refreshes-stale-cached-object cloudfront.e2e-webacl-association-blocks-then-unblocks-requests cloudfront.edge-requests-deliver-requests-metric-with-global-dimension cloudfront.https-only-and-disallowed-method-rejected-at-edge cloudfront.max-ttl-clamps-origin-cache-control-max-age cloudfront.origin-request-policy-header-does-not-enter-the-cache-key cloudfront.response-headers-policy-server-timing-edge-header cloudfront.server-side-origin-access-control-signs-requests cloudfront.server-side-security-headers-policy-enforced-on-response cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused cloudfront.signed-url-key-outside-trusted-key-group-rejected cloudfront.trusted-key-group-on-ordered-behavior-gates-only-its-path-pattern
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-create-distribution|run#11|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", dist["DomainName"]), (
Issue-Key: cloudfront.op-contract-get-distribution|run#8|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", dist["DomainName"]), (
Issue-Key: cloudfront.distribution-create-deploy-lifecycle|run#6|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", dist.get("DomainName", "")), (
Issue-Key: cloudfront.distribution-get-vs-get-config-shape-parity|run#9|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", dist["DomainName"]), dist["DomainName"]
Issue-Key: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-fetch-through-distribution|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cf-wildcard-invalidation-clears-only-matching-objects|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-cookie-allowlist-separates-cache-entries|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.continuous-deployment-policy-disabled-suppresses-header-routing|_poll_until#33|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl|_fetch_ok#27|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}") from exc
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-cache-policy-cfn-enforced-on-distribution|run#17|AssertionError|assert status == 200 and body == b"content-v1", (
Issue-Key: cloudfront.deploy-cache-policy-cfn-ttl-enforced|run#31|AssertionError|assert status == 200 and body == b"content-v1", (
Issue-Key: cloudfront.deploy-cache-policy-enforced-via-cloudformation|_poll_until_header#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-cloudfront-distribution-serves-origin-content|_fetch_with_retries#11|Timeout|time.sleep(2)
Issue-Key: cloudfront.deploy-distribution-cfn-serves-live-content|_poll_fetch#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-distribution-cfn-serves-live-traffic|_poll_http_get#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-distribution-serves-content-via-cloudformation|_poll_http_get#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-distribution-invalidation-cache-refresh|_poll_fetch#26|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-distribution-webacl-association-blocks-request|_poll_http_get#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-invalidation-clears-cached-content|_fetch_with_retries#10|Timeout|time.sleep(2)
Issue-Key: cloudfront.e2e-invalidation-clears-cached-object|_poll_http_get#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-invalidation-refreshes-stale-cached-object|_poll_http_get#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-webacl-association-blocks-then-unblocks-requests|_poll_http_get#20|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.edge-requests-deliver-requests-metric-with-global-dimension|_fetch_ok#29|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}") from exc
Issue-Key: cloudfront.https-only-and-disallowed-method-rejected-at-edge|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.max-ttl-clamps-origin-cache-control-max-age|_fetch_ok#27|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}") from exc
Issue-Key: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.response-headers-policy-server-timing-edge-header|_get#32|URLError|with opener.open(urllib.request.Request(url, method="GET"), timeout=30.0) as resp:
Issue-Key: cloudfront.server-side-origin-access-control-signs-requests|_fetch#22|Timeout|time.sleep(2)
Issue-Key: cloudfront.server-side-security-headers-policy-enforced-on-response|_fetch#12|Timeout|time.sleep(2)
Issue-Key: cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused|_poll_until#36|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.signed-url-key-outside-trusted-key-group-rejected|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.trusted-key-group-on-ordered-behavior-gates-only-its-path-pattern|_settle#38|Timeout|time.sleep(step_s)
Issue-Commit: 6651c87027da78b467298e3a28f696cadd44fc38

# cloudfront: 2 net-new causes — a distribution's published DomainName is not a DNS host name, and the announced edge address serves no viewer traffic (41 data-plane checks reach no verdict)

> _Note: this issue was identified against the code base at commit `6651c87027da78b467298e3a28f696cadd44fc38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Two net-new root causes, both grounded in failing behavioral checks of this suite that no open issue carries the replay key for. Related open issues exist but the suite did not mark these checks as owned by them: 20261114T142904Z-cloudfront-reopens-generated-identifiers-and-published-domain-names.md (identifier/domain-name forms) and 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md (the edge serving nothing for a different set of 24 checks). Neither covers the checks listed here.

Cause 1 of 2: CreateDistribution/GetDistribution publish Distribution.DomainName as an authority-plus-path string instead of a DNS host name.
AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31) documents Distribution$DomainName as "The distribution's CloudFront domain name. For example: d111111abcdef8.cloudfront.net" — a bare DNS host name with no path component, no slash, and no scheme. The same member on DistributionSummary and on the Route 53 alias-target contract is a host name; AWS's value is always of the form <distribution-id-lowercased>.cloudfront.net. CreateDistribution returns it at creation time, before the distribution reaches Deployed, and GetDistribution must agree with it.
Emulator: DomainName comes back containing a path: the value has the shape "<authority>/_aws/cloudfront/<DistributionId>" (one replay produced '127.0.0.1/_aws/cloudfront/E7IYCGPKTANSF3'; the authority and the id are per-run values, the '/_aws/cloudfront/<id>' path tail is the fixed, diverging part). It fails the host-name shape test r"[A-Za-z0-9.-]+(:\d+)?" because of the embedded '/'. CreateDistribution and GetDistribution both report it, so the divergence is in how the member is produced, not in one operation's serializer.
Why: a client cannot use the value anywhere a host name is required. It cannot be put in a Route 53 alias target or a CNAME, it cannot be passed to an SNI/TLS client, and any client that composes "https://" + DomainName + "/" + key builds a URL with a doubled path segment. Every downstream consumer of a distribution's published domain breaks, including the CloudFormation GetAtt of DomainName.
Fix: the CloudFront distribution state/serialization that mints the published domain name — the DomainName assigned when a distribution record is created and echoed by the GetDistribution/GetDistributionConfig/ListDistributions readback. The published value must be a host name only; any emulator-local request path must live outside the member.
Traces: cloudfront.op-contract-create-distribution, cloudfront.op-contract-get-distribution, cloudfront.distribution-create-deploy-lifecycle, cloudfront.distribution-get-vs-get-config-shape-parity.

Cause 2 of 2: a Deployed distribution's announced edge address answers no viewer request at all, so the entire CloudFront data plane is unobservable.
AWS: recorded behavior only (the viewer-facing edge is not modeled in Smithy). The recordings show that once GetDistribution reports Status "Deployed", a GET for an object that exists at the configured origin, addressed to the distribution's published domain over HTTPS, is answered 200 with the origin's bytes, and that the edge then applies the configured behavior (cache-policy TTL clamping, MinTTL floors, query-string/cookie cache-key allowlists, Range and conditional-GET handling, compression, response-headers policies, CustomErrorResponses, signed URL/cookie and trusted-key-group enforcement, continuous-deployment header routing, invalidation, geo/method restrictions, web-ACL enforcement, and metric/log emission for the request).
Emulator: no viewer request ever completes against the announced address. Three symptoms, one cause: (a) the TLS connect is refused outright — "urllib.error.URLError: <urlopen error [Errno 111] Connection refused>"; (b) the fetch helper polls and never sees a 200 — "AssertionError: DATAPLANE_UNIMPLEMENTED: timed out waiting for 200 from https://<announced-address>/<key> after 180.0s (last: None)", and the CFN-driven variants report "expected initial 200 content-v1, got status=None body=b''"; (c) 33 further checks exhaust their whole budget with no answer — "Error: Check timed out after 60.0s". In every case nothing is served, so no edge behavior can be asserted.
Why: a client that deploys a distribution and then fetches through it gets nothing — no content, no cache semantics, no signed-access enforcement, no error pages, no request metrics or access logs. Every CloudFront capability that is only observable from the viewer side is absent, and the failure is announced-address-specific: the emulator publishes a domain it does not accept connections on, so even a correct client cannot reach it.
Fix: the CloudFront edge/viewer data plane — the listener that must accept viewer requests on whatever address a distribution publishes, plus the request pipeline that resolves the matching cache behavior, fetches from the configured origin, and applies cache, policy, signed-access and error-response handling to the response. It is coupled to Cause 1: the address the emulator serves and the address it announces must be the same one.
Traces: the 41 check ids listed in trace_ids below, minus the four named under Cause 1.

## Evidence
Cause 1 — expected vs actual:
  expected: dist["DomainName"] is a DNS host name, e.g. d111111abcdef8.cloudfront.net (matches r"[A-Za-z0-9.-]+(:\d+)?").
  actual:   AssertionError: expected a hostname DomainName, got '127.0.0.1/_aws/cloudfront/E7IYCGPKTANSF3'
            (check_cloudfront_op_contract_create_distribution.py line 33; identical value from GetDistribution at check_cloudfront_op_contract_get_distribution.py line 29, and from the create/deploy lifecycle at check_cloudfront_distribution_create_deploy_lifecycle.py line 94: "expected a hostname DomainName to be assigned at creation, got '127.0.0.1/_aws/cloudfront/E7IYCGPKTANSF3'"). The authority and the distribution id vary per run; the embedded '/_aws/cloudfront/<id>' path is the invariant divergence.

Cause 2 — expected vs actual:
  expected: HTTPS GET https://<published DomainName>/<key> on a Deployed distribution -> 200 with the origin object's bytes, then the configured edge behavior on top of it.
  actual (connect refused): urllib.error.URLError: <urlopen error [Errno 111] Connection refused>  (cloudfront.response-headers-policy-server-timing-edge-header)
  actual (never 200):      AssertionError: DATAPLANE_UNIMPLEMENTED: timed out waiting for 200 from https://127.0.0.1/_aws/cloudfront/EJZ50Z92T9DP30/present.txt after 180.0s (last: None)  (cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl, line 225/142)
                           AssertionError: DATAPLANE_UNIMPLEMENTED: timed out waiting for 200 from https://127.0.0.1/_aws/cloudfront/EJZ50Z92T9DP30/metric-probe.txt after 180.0s (last: None)  (cloudfront.edge-requests-deliver-requests-metric-with-global-dimension, line 232/134)
                           AssertionError: DATAPLANE_UNIMPLEMENTED: timed out waiting for 200 from https://127.0.0.1/_aws/cloudfront/EZGEITDQ1HLFY0/clamped/obj.txt after 180.0s (last: None)  (cloudfront.max-ttl-clamps-origin-cache-control-max-age, line 229/124)
  actual (no response at all): AssertionError: expected initial 200 content-v1, got status=None body=b''  (cloudfront.deploy-cache-policy-cfn-enforced-on-distribution, line 137)
                               AssertionError: expected cold fetch 200 content-v1, got status=None body=b''  (cloudfront.deploy-cache-policy-cfn-ttl-enforced, line 167)
  actual (no verdict):     Error: Check timed out after 60.0s — for the remaining 33 data-plane checks in trace_ids (cache-behavior ordering, cache-key allowlists, compression, continuous deployment, invalidation, conditional GET/Range, CloudFront Functions at viewer request/response, signed URLs and cookies, trusted key groups, HTTPS-only/method restriction, origin-request policy, OAC origin signing, security-headers policy, CFN-deployed distributions serving live content).
  Hosts, ports, distribution ids and object keys above are per-run values; the requirement is that the announced address answers a viewer GET at all.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Cause 1: replay cloudfront.op-contract-create-distribution — CreateDistribution, then read Distribution.DomainName from the response and from GetDistribution. Assert it matches r"[A-Za-z0-9.-]+(:\d+)?" (a bare host name). It currently contains an embedded '/_aws/cloudfront/<Id>' path.
3. Cause 2: replay cloudfront.cf-fetch-through-distribution (or any check in trace_ids). Create a distribution over a reachable origin, poll GetDistribution until Status == "Deployed", then HTTPS GET https://<Distribution.DomainName>/<an object that exists at the origin>. The connection is refused or never yields 200; no edge behavior (cache TTLs, policies, signed access, custom error responses, metrics/logs) can be observed.
