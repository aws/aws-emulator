Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body|_website_base_url#18|CapabilityNotOffered|candidates = ctx.s3_website_urls(
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# s3: the emulator publishes no S3 website endpoint, so website-endpoint behaviour (301 for a stored WebsiteRedirectLocation) cannot be exercised at all (1 cause)

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: the emulator offers no S3 website endpoint for a bucket that has a website configuration, so the website-endpoint half of S3's object-serving contract is unreachable. Failing check: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body.

AWS: Smithy models the stored value — PutObject/HeadObject member WebsiteRedirectLocation bound to the x-amz-website-redirect-location header, and PutBucketWebsite/GetBucketWebsite the bucket's website configuration. The endpoint behaviour itself is recorded behavior only: a bucket with a website configuration is reachable on a website endpoint host distinct from the REST endpoint, and on that endpoint an object whose x-amz-website-redirect-location is stored answers HTTP 301 with Location set to exactly the stored value ("/new.html" or "https://example.com/landing"), serving no body, while the SAME key fetched from the REST endpoint answers HTTP 200 with the object's bytes. (The check also covers the refusal of a location that is neither "/..." nor http(s)://: 400 InvalidRedirectLocation with message "The website redirect location must have a prefix of 'http://' or 'https://' or '/'.")

Emulator: no website endpoint is published at all. Asking the harness for the bucket's website URLs raises simparity.behavioral.context.CapabilityNotOffered: "the emulator offers no S3 website endpoint (the run names no website endpoint template: s3_website_url=, SIMPARITY_S3_WEBSITE_URL)", so not one website-endpoint request is ever made and neither the 301 nor the REST/website split can be observed.

Why: a client that hosts a static site on S3 (or relies on WebsiteRedirectLocation to move pages) has no endpoint to point at: every redirect, index/error-document and routing-rule behaviour is simply absent, and code that works against AWS cannot be exercised against the emulator at all. It also makes the REST-vs-website distinction untestable, so a 200-with-body answer can never be told apart from AWS's 301.

Related open issue, which does NOT carry this check's replay key: 20261109T220008Z-s3-7-net-new-causes-cors-preflight-answers.md owns other s3 website-hosting checks, where the failure is a different one (the emulator published/accepted an '<bucket>.s3-website-<region>.amazonaws.com' host the harness refuses because it is not one of the emulator's own hosts). The cause filed here is the absence of any published website endpoint template.

Fix: the S3 front door's endpoint publication and request routing — the emulator must publish a website endpoint on one of its own hosts (a per-bucket/website listener reachable at the emulator's address) and route requests arriving on it through website-endpoint semantics (301 to the stored WebsiteRedirectLocation, index/error documents, no writes), leaving the REST endpoint serving the object body.

Traces: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body.

## Evidence
Check s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body [mismatch].

Expected: the bucket's website endpoint answers GET /old.html with HTTP 301 and Location: '/new.html' (and GET /ext.html with 301 Location: 'https://example.com/landing'), while the REST endpoint answers the same keys 200 with their bodies.

Actual: the check never reaches a website request — File "check_s3_...website_redirect_location....py", line 142, in _website_base_url: candidates = ctx.s3_website_urls(...) -> File "context.py", line 560, in s3_website_urls: raise CapabilityNotOffered -> simparity.behavioral.context.CapabilityNotOffered: the emulator offers no S3 website endpoint (the run names no website endpoint template: s3_website_url=, SIMPARITY_S3_WEBSITE_URL).

## Steps to Reproduce
1) CreateBucket a bucket and give it a website configuration (PutBucketWebsite with an index document) plus a policy allowing the read.
2) PutObject Key='old.html' with WebsiteRedirectLocation='/new.html' and Key='new.html' with a body; HeadObject('old.html') correctly reports WebsiteRedirectLocation='/new.html'.
3) Ask the emulator for the bucket's website endpoint (the harness' ctx.s3_website_urls, i.e. the published s3_website_url template): the emulator names none, so no GET can be issued against a website host.
4) Consequently the expected 301-with-Location on the website endpoint vs 200-with-body on the REST endpoint cannot be served.
