Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__s3.cfn-cloudfront-distribution-in-place-update-serves-new-root-object-then-not-found
Issue-Key: cloudformation__cloudfront__s3.cfn-cloudfront-distribution-in-place-update-serves-new-root-object-then-not-found|_assert_serves#26|TimeoutError|raise TimeoutError(f"run budget ended before {url} served {body!r}")
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudformation__cloudfront__s3: a CloudFormation-provisioned CloudFront distribution never serves its S3 origin's DefaultRootObject, so the in-place root-object update is ungradable

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

Cause 1 of 1: a distribution a CREATE_COMPLETE stack provisioned over a private S3 bucket reached through origin access control never answers the root object's bytes at its published DomainName.
  AWS:      a stack declaring an encrypted, private AWS::S3::Bucket, an AWS::CloudFront::OriginAccessControl, and an AWS::CloudFront::Distribution whose DefaultRootObject names a key in that bucket serves that object's body over HTTPS at the distribution's DomainName output; the same bucket read directly and unsigned is refused by S3 with AccessDenied / "Access Denied". Recorded behavior only for the serve leg (no Smithy model covers the CloudFront edge): the recording shows the object's bytes being served at the distribution domain within the serve budget, then the new bytes after an in-place DefaultRootObject update, then NoSuchDistribution / "The specified distribution does not exist." after the delete.
  Emulator: the distribution never serves the object. The check exhausts its whole 600 s serve budget with `TimeoutError: run budget ended before <distribution-domain>/ served b'sim2real first root object\n'`, so the edge produced no response carrying the origin's body at all.
  Why:      content behind a CloudFormation-provisioned distribution is unreadable: a client that deploys a private-bucket + OAC + distribution stack can never fetch anything through the edge. Because the first serve never happens, every later behavior in the same trajectory — the in-place DefaultRootObject update serving the second object, and the post-delete NoSuchDistribution refusal — is unreachable and ungraded.
  Fix:      CloudFront's distribution data plane — the edge request handler that resolves DefaultRootObject against an S3 origin reached through origin access control — as it is wired from the CloudFormation-provisioned AWS::CloudFront::Distribution resource handler (the distribution the stack reports CREATE_COMPLETE must actually be serving at the DomainName it publishes).
  Related:  open issue 20261110T092303Z-cloudfront-s3-6-net-new-causes-in-the.md records DefaultRootObject and OAC-granted origin-fetch defects under the `cloudfront__s3` heading. It does not carry this check's replay key, so this cause is recorded here with the check that exercises it.
  Traces:   cloudformation__cloudfront__s3.cfn-cloudfront-distribution-in-place-update-serves-new-root-object-then-not-found.

## Evidence
cloudformation__cloudfront__s3.cfn-cloudfront-distribution-in-place-update-serves-new-root-object-then-not-found [error]:
  _assert_serves(ctx, out["DomainName"], _FIRST_BODY)
  TimeoutError: run budget ended before https://<distribution-domain>/ served b'sim2real first root object\n'

Expected: a GET of `/` at the distribution's DomainName returns the DefaultRootObject's body (b'sim2real first root object\n') within the serve budget, while an unsigned direct S3 read of the same object is refused AccessDenied / "Access Denied".
Actual:   the edge never returns the body; the 600 s serve budget expires with no response carrying the origin content, so the subsequent in-place DefaultRootObject update leg and the post-delete NoSuchDistribution / "The specified distribution does not exist." leg are never reached.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. CreateStack with the template from this check: a private, encrypted AWS::S3::Bucket holding a root object, an AWS::CloudFront::OriginAccessControl, and an AWS::CloudFront::Distribution whose DefaultRootObject names that key, with the distribution DomainName as a stack output.
3. Wait for CREATE_COMPLETE, then GET `/` at the DomainName output over HTTPS. AWS serves the root object's bytes; the emulator serves nothing until the budget expires.
