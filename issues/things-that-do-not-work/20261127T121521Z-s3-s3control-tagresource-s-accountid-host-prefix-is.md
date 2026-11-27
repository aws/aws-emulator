Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__s3control.tag-resource-rejects-tag-key-outside-allowed-character-set
Issue-Key: s3__s3control.tag-resource-rejects-tag-key-outside-allowed-character-set|<lambda>#8|EgressRefused|lambda: s3c.tag_resource(
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# s3__s3control: TagResource's {AccountId}. host prefix is unserved, so the invalid-tag-key rejection check never reaches the emulator (1 cause)

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
This check creates an S3 bucket and then exercises s3control TagResource on the bucket's ARN with a tag key outside the allowed character set. The TagResource call is never delivered. This bundle has 1 cause; it must be closed.

Cause 1 of 1: the emulator serves/announces no address for the account-id-prefixed s3control host that the model mandates for TagResource (and ListTagsForResource), so the compliant SDK request cannot be delivered.
Failing check id: s3__s3control.tag-resource-rejects-tag-key-outside-allowed-character-set.

AWS: Smithy (com.amazonaws.s3control#AWSS3ControlServiceV20180820, restXml): TagResource is POST /v20180820/tags/{ResourceArn+} with endpoint.hostPrefix "{AccountId}.", AccountId bound as both @httpHeader("x-amz-account-id") and @hostLabel (pattern ^\d{12}$), ResourceArn as @httpLabel (pattern ^arn:[^:]+:s3(express)?:[^:]), Tags required (@length max 50); success is HTTP 204 with an empty body. An SDK therefore addresses '<12-digit account id>.<endpoint authority>' (against real AWS <account-id>.s3-control.<region>.amazonaws.com). AWS answers there: for a tag key containing a character outside the allowed set it refuses with error code 'InvalidTag' and the message "This request contains a tag key or value that isn't valid. Valid characters include the following: [a-zA-Z+-=._:/]. Tag keys can contain up to 128 characters. Tag values can contain up to 256 characters." (that text is fixed, including the character class and the two limits), the rejected tag does not land, and a key built only from the allowed set is accepted and read back by ListTagsForResource.

Emulator: only the bare authority is announced. When the SDK applies the modeled host prefix the address is refused before any HTTP request — 'CapabilityNotOffered: the target serves no address for <account-id>.<authority>' / simparity.egress.EgressRefused: "egress refused: '<account-id>.<authority>' is not the endpoint under test (<authority>)". No HTTP status, no error code and no body is produced, so neither the InvalidTag refusal nor the positive control can be observed. (Observed host in this run: 000000000000.127.0.0.1 — account id and loopback authority are per-environment values.)

Why: every SDK/CLI client of s3control tagging gets a name-resolution/connection failure instead of the modeled 204 or the InvalidTag refusal, so tags set through S3 Control and read through S3 (and vice versa) cannot be exercised at all.

Fix: the emulator's HTTP front door — the authorities it announces and its host-based dispatch — must accept and route the modeled '{AccountId}.' host prefix for the s3control endpoint, stripping and validating it against the x-amz-account-id header and the AccountId @hostLabel pattern ^\d{12}$, so host-prefixed requests reach the same s3control handlers as bare-authority ones. The open issue 20261116T222947Z-s3-s3control-the-s3control-account-id-host-prefix.md describes the same front-door defect for this service heading but does not carry this check id, so this check is not covered by it.

Traces: the failing check id above and in trace_ids.

## Evidence
Behavioral suite: 0/59 passed; s3__s3control 0/3 (the other two failures are owned by 20261116T222947Z-s3-s3control-the-s3control-account-id-host-prefix.md and are not re-filed).

  Error: CapabilityNotOffered: the target serves no address for 000000000000.127.0.0.1
    File "egress.py", line 417, in resolve -> refuse(...)
    File "egress.py", line 317, in refuse -> raise EgressRefused(
    simparity.egress.EgressRefused: egress refused: '000000000000.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

Expected: s3control.tag_resource(AccountId=<account>, ResourceArn='arn:aws:s3:::<bucket>', Tags=[{'Key': 'bad#key', 'Value': 'x'}]) reaches the emulator at '<account id>.<authority>' and is refused with code 'InvalidTag' and message "This request contains a tag key or value that isn't valid. Valid characters include the following: [a-zA-Z+-=._:/]. Tag keys can contain up to 128 characters. Tag values can contain up to 256 characters."; ListTagsForResource then shows no tags, and tag_resource with Key='good.key:1' succeeds (204) and reads back.

Actual: nothing is served on that host, so the call fails at address resolution — no HTTP status, no error code, no body. The account id (000000000000) and authority (127.0.0.1) are per-run values; the fixed requirement is that '<12-digit account id>.<served authority>' is served.

## Steps to Reproduce
1. Clone the repo and boot the emulator.
2. With boto3, create an S3 bucket (CreateBucketConfiguration.LocationConstraint=<region> outside us-east-1).
3. Create an s3control client against the emulator's endpoint and call tag_resource(AccountId=<12-digit account id>, ResourceArn='arn:aws:s3:::<bucket>', Tags=[{'Key': 'bad#key', 'Value': 'x'}]).
4. Observe botocore applies @endpoint(hostPrefix "{AccountId}.") and addresses '<account id>.<emulator authority>'; the request never reaches the emulator — under the harness it is refused with 'CapabilityNotOffered: the target serves no address for <account id>.<authority>'.
5. After the fix the call must reach the s3control handlers (host prefix accepted and validated against x-amz-account-id and ^\d{12}$) and be refused with InvalidTag and the documented message, while 'good.key:1' is accepted with 204.
6. Re-replay the check id in trace_ids.
