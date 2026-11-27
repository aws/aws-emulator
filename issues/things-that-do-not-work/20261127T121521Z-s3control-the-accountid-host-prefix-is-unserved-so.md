Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-and-bucket-policy-allow-combinations-for-a-role-session
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-and-bucket-policy-allow-combinations-for-a-role-session|run#17|EgressRefused|ap_arn = s3c.create_access_point(
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# s3control: the {AccountId}. host prefix is unserved, so the access-point/bucket-policy allow-combination check never reaches the emulator (1 cause)

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
This check sets up an access point plus access-point and bucket policies and evaluates the allow combinations for a role session. No s3control call in it is ever delivered. This bundle has 1 cause; it must be closed.

Cause 1 of 1: the emulator serves/announces no address for the account-id-prefixed s3control host that the API model mandates, so the compliant SDK request cannot be delivered at all.
Failing check id: s3control.access-point-and-bucket-policy-allow-combinations-for-a-role-session.

AWS: Smithy (com.amazonaws.s3control#AWSS3ControlServiceV20180820, restXml, sigv4Name 's3'): every account-scoped operation carries @endpoint(hostPrefix: "{AccountId}.") and binds AccountId as both @httpHeader("x-amz-account-id") and @hostLabel (target AccountId, pattern ^\d{12}$) — e.g. CreateAccessPoint (PUT /v20180820/accesspoint/{Name}, output AccessPointArn + Alias) and PutAccessPointPolicy. An AWS SDK therefore addresses '<12-digit account id>.<endpoint authority>': against real AWS <account-id>.s3-control.<region>.amazonaws.com, against a custom endpoint authority '<account-id>.<that authority>', and AWS answers there with the operation's modeled 200/204 body or its modeled refusal. The account id is fixed text only in its position; its value comes from the request's AccountId member.

Emulator: only the bare authority it is reached on is announced, so when the SDK applies the modeled host prefix the address is refused before any HTTP request is made — 'CapabilityNotOffered: the target serves no address for <account-id>.<authority>' / simparity.egress.EgressRefused: "egress refused: '<account-id>.<authority>' is not the endpoint under test (<authority>)". No HTTP status, no error code and no body is produced. (Observed host in this run: 000000000000.127.0.0.1 — account id and loopback authority are per-environment values; the requirement is that '<12-digit account id>.<served authority>' is served, not those literals.)

Why: a client cannot create an access point or put its policy at all, so no authorization behaviour (access-point policy combined with bucket policy for a role session) can be exercised; the whole s3control front door is unusable from boto3, the AWS CLI and every other SDK, which apply the modeled hostPrefix unconditionally.

Fix: the emulator's HTTP front door — the hostnames/authorities it announces it serves and its host-based request dispatch — must accept and route the modeled '{AccountId}.' host prefix for the s3control endpoint, stripping and validating it against the x-amz-account-id header and the AccountId @hostLabel pattern ^\d{12}$, so host-prefixed requests land on the same s3control handlers as bare-authority requests. The open issue 20261116T222947Z-s3control-the-account-id-host-prefix-every-operation.md describes the same front-door defect for this service but does not carry this check id, so this check is not covered by it.

Traces: the failing check id above and in trace_ids.

## Evidence
Behavioral suite: 0/59 passed; s3control 0/36. This check is the one s3control failure not owned by an open issue, and it reports the same pre-request failure as the owned ones:

  Error: CapabilityNotOffered: the target serves no address for 000000000000.127.0.0.1
    File "egress.py", line 417, in resolve -> refuse(...)
    File "egress.py", line 317, in refuse -> raise EgressRefused(
    simparity.egress.EgressRefused: egress refused: '000000000000.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

Expected: s3control.create_access_point(AccountId=<account>, Name=<name>, Bucket=<bucket>) and the subsequent put_access_point_policy / get_access_point calls reach the emulator at '<account id>.<authority>' (per @endpoint(hostPrefix "{AccountId}.")) and answer their modeled responses, so the allow-combination evaluation for the role session can be observed.

Actual: nothing is served on that host, so the call fails at address resolution — no HTTP status, no error code, no body. The account id (000000000000) and authority (127.0.0.1) in the message are per-run values; the fixed requirement is that the served authority prefixed with '<12-digit account id>.' is served.

## Steps to Reproduce
1. Clone the repo and boot the emulator; confirm the s3control endpoint answers on the bare authority.
2. With boto3, create an s3control client against the emulator's endpoint and call create_access_point(AccountId=<12-digit account id>, Name='any', Bucket='<an existing bucket>').
3. Observe botocore applies the modeled @endpoint(hostPrefix "{AccountId}.") and addresses '<account id>.<emulator authority>'; the request never reaches the emulator — under the harness it is refused with 'CapabilityNotOffered: the target serves no address for <account id>.<authority>'.
4. After the fix the same call must be routed to the s3control handlers (host prefix accepted, validated against x-amz-account-id and the ^\d{12}$ label pattern) and answer the modeled CreateAccessPoint body (AccessPointArn, Alias).
5. Re-replay the check id in trace_ids.
