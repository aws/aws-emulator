Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|_poll_until#36|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudfront__lambda: the edge enforces nothing because it answers nothing — AllowedMethods refusal and signed-URL source-IP/key-revocation enforcement are both unreachable

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles TWO root causes in the cloudfront__lambda pairing. BOTH must be closed before submitting. Both are the same symptom at the viewer — the distribution's edge domain produces no response — but they are two distinct enforcement rules the edge owes, and each needs its own implementation.

Cause 1 of 2: a cache behavior's AllowedMethods/CachedMethods are not enforced at the edge.
AWS: recorded behavior only (the edge is outside the Smithy control-plane model; control-plane side is Smithy: CreateDistribution -> POST /2020-05-31/distribution, 201, which echoes AllowedMethods/CachedMethods back verbatim whether or not anything enforces them). What the recording requires: a POST through a distribution whose behavior allows only GET,HEAD is refused BY CLOUDFRONT with HTTP 403 and `X-Cache: Error from cloudfront`, the body carries no origin bytes, and the origin's invocation counter for that request's nonce stays at ZERO behind a differential barrier (a later GET through the same distribution is logged); the identical POST sent directly to the origin's Lambda function URL with SigV4 succeeds (200) and moves that counter to exactly ONE. On a behavior whose AllowedMethods include OPTIONS but whose CachedMethods do not, a repeated GET is eventually served `Hit from cloudfront` while three identical OPTIONS requests each reach the origin (counter exactly 3).
Emulator: no response is ever obtained through the distribution's edge domain; the check is cut off at the 60 s cap, so there is no status, no X-Cache header and no origin counter to compare.
Why: AllowedMethods is a request-surface control — an origin that must never see POST will see it if the edge forwards everything, and an emulator that echoes the config while serving nothing lets a client "verify" a method restriction that does not exist. The CachedMethods half is equally invisible: a client cannot tell a cache hit from an origin fetch, so it cannot confirm that uncached methods really reach the origin every time.
Fix: the cloudfront crate's edge/data-plane request path — the distribution's DomainName must answer HTTPS requests, select the matching cache behavior by path pattern, refuse a method absent from that behavior's AllowedMethods with a CloudFront-generated 403 (and the X-Cache: Error from cloudfront header) without contacting the origin, and serve only CachedMethods from the cache while forwarding the rest to the origin on every request.
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected.

Cause 2 of 2: a signed URL's custom policy (source-IP condition) and the trusted key group's live membership are not enforced at the edge.
AWS: recorded behavior only (data plane). What the recording requires: on a behavior with TrustedKeyGroups, a URL signed with key A over a custom policy whose Condition.IpAddress.AWS:SourceIp is the viewer address CloudFront itself observes is served HTTP 200 from the origin; the same resource signed with a policy allowing only a range that provably excludes that address is refused HTTP 403 with error Code `AccessDenied` and no origin bytes; with two keys in the group both signatures are admitted; and after UpdateKeyGroup removes key B, once propagated B's signature is refused HTTP 403 with Code `InvalidKey` while A's still returns 200. The viewer address itself is discovered live from the CloudFront-Viewer-Address header the edge adds to the origin request on an unsigned behavior.
Emulator: no response is ever obtained through the distribution's edge domain; the check is cut off at the 60 s cap, so no admission, no 403 and no post-revocation refusal can be observed, and the CloudFront-Viewer-Address header the negative leg is built from is never produced.
Why: this is the authorization boundary for private content. An edge that answers nothing is indistinguishable from one that admits every signature; a client cannot confirm that an IP condition is honoured or — the operationally critical part — that removing a public key from a key group actually REVOKES the URLs signed with it. Note that UpdateKeyGroup is additionally refused 501 NotImplemented (filed separately for service cloudfront), so even a working edge could not complete the revocation leg.
Fix: the same cloudfront edge/data-plane path as cause 1, plus signed-URL verification on it: parse Policy/Signature/Key-Pair-Id, verify against the public keys currently in the behavior's TrustedKeyGroups (re-read per request so a removed key stops admitting), evaluate the custom policy's Resource/DateLessThan/IpAddress conditions against the observed viewer address, emit the distinct 403 Codes for a failed condition (AccessDenied) and an unknown/revoked key (InvalidKey), and add the CloudFront-Viewer-Address header to the origin request.
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied.

The open issue 20261025T220525Z-cloudfront-lambda-the-oac-signed-lambda-function-url.md describes the same silent edge for the OAC-signed Lambda function URL check and carries only that check's id; it does not cover either cause here.

## Evidence
Cause 1 — cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected [mismatch]:
  Error: Check timed out after 60.0s
  The check creates a Lambda function URL origin behind an OAC, a distribution whose default behavior allows GET,HEAD and an /opt/* behavior allowing GET,HEAD,OPTIONS with CachedMethods GET,HEAD, then sends a nonce-tagged POST through the distribution. Expected: HTTP 403 with `X-Cache: Error from cloudfront`, no origin bytes in the body, origin invocation count for that nonce == 0; the same POST direct to the function URL -> HTTP 200 and count == 1; three OPTIONS through /opt/* -> origin count == 3 while a repeated GET is served `Hit from cloudfront`. Actual: no response is ever produced through the distribution domain and the check is killed at the 60 s cap, so no status, no X-Cache value and no origin counter exist.

Cause 2 — cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied [mismatch]:
  Error: Check timed out after 60.0s
  The check creates the same Lambda-function-URL origin behind an OAC, a public key + key group, and a distribution with TrustedKeyGroups and IsIPV6Enabled=false, then discovers its own viewer address from the CloudFront-Viewer-Address header echoed on an unsigned /whoami* behavior. Expected: signed URL with a /32 policy for that address -> HTTP 200 with the origin marker; policy for a provably excluded range -> HTTP 403 Code AccessDenied with no origin bytes; both keys admitted while both are in the group; after UpdateKeyGroup removes one, that key's URL -> HTTP 403 Code InvalidKey while the other still returns 200. Actual: no edge response at all; the check is killed at the 60 s cap, and the CloudFront-Viewer-Address header the negative leg needs is never observed.

In both cases the divergence is the absence of any response from the distribution's edge domain. Generated domain names, function URLs, nonces and viewer addresses differ per run and are not requirements.

## Steps to Reproduce
Cause 1:
1. Create a Lambda function with a function URL that echoes the HTTP method and the raw query string and logs one line per invocation; front it with a CloudFront OAC.
2. CreateDistribution over that origin: DefaultCacheBehavior AllowedMethods=[GET,HEAD]; a /opt/* behavior AllowedMethods=[GET,HEAD,OPTIONS], CachedMethods=[GET,HEAD] with a caching cache policy. Poll GetDistribution until Status == Deployed.
3. POST https://<DomainName>/?nonce=<nonce> -> expect HTTP 403 with `X-Cache: Error from cloudfront`, no origin bytes, and zero origin invocations for <nonce>.
4. POST the same body/nonce directly to the function URL with SigV4 -> expect HTTP 200 and exactly one origin invocation for <nonce>.
5. Send three OPTIONS to https://<DomainName>/opt/?nonce=<nonce2> -> expect three origin invocations; repeat a GET on the same path -> expect `Hit from cloudfront`.
6. Observe: step 3 never returns; nothing is served through <DomainName>.

Cause 2:
1. Same OAC-fronted function-URL origin; CreatePublicKey (A) + CreateKeyGroup([A]); CreateDistribution with TrustedKeyGroups=[that group], IsIPV6Enabled=false, an unsigned /whoami* behavior; wait for Deployed.
2. GET https://<DomainName>/whoami and read the CloudFront-Viewer-Address the origin echoes -> expect HTTP 200 and a viewer address.
3. Sign a URL with A over a custom policy whose Condition.IpAddress.AWS:SourceIp is that address as a /32 -> expect HTTP 200 from the origin.
4. Sign the same resource with a policy allowing only a range that excludes the address -> expect HTTP 403 with error Code AccessDenied and no origin bytes.
5. Add a second public key B to the key group; expect URLs signed with either key to be admitted. Then UpdateKeyGroup to remove B and poll -> expect B's URL to settle at HTTP 403 Code InvalidKey while A's still returns 200.
6. Observe: step 2 never returns; nothing is served through <DomainName>.

Re-replay cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected and cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied to confirm.
