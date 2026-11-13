Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-vpc-origin-denies-internet-read-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-vpc-origin-denies-internet-read-rejected|run#22|AssertionError|assert code == "AccessDenied", (
Issue-Commit: f1473582b6746d116724ae43d0fc6ffbe3d6e185

# REOPENS: an internet-borne read of a VPC-origin access point is still refused NoSuchBucket instead of AccessDenied

> _Note: this issue was identified against the code base at commit `f1473582b6746d116724ae43d0fc6ffbe3d6e185`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject (Bucket addressed by an access point ARN); CreateAccessPoint/GetAccessPoint VpcConfiguration

## Description
REOPENS 20261030T064829Z-s3control-an-internet-borne-read-of-a-vpc.md: its fix left the call refused.

Cause 1 of 1: a GetObject whose Bucket field carries a VPC-origin access point ARN is still refused with the wrong error class, so the VPC network-origin restriction remains unobservable on the data path. Failing check: s3control.access-point-vpc-origin-denies-internet-read-rejected.

AWS: Smithy (com.amazonaws.s3control) models CreateAccessPoint/GetAccessPoint with VpcConfiguration, whose only member VpcId is @required with @length min 1 max 1024 and whose documentation states the rule verbatim: "If this field is specified, this access point will only allow connections from the specified VPC ID." NetworkOrigin is an enum with values Internet|VPC. No errors are bound to those operations, so the refusal itself is recorded behavior only: a GetObject addressed to an access point ARN (arn:aws:s3:<region>:<account>:accesspoint/<name>) whose NetworkOrigin is VPC, arriving from outside that VPC, is refused with HTTP 403, error code AccessDenied and the message `User: <caller-arn> is not authorized to perform: s3:GetObject on resource: "<access-point-arn>/object/<key>" with an explicit deny in a resource-based policy`. Fixed text of that message: `User: `, ` is not authorized to perform: s3:GetObject on resource: `, the double quotes around the resource, and ` with an explicit deny in a resource-based policy`; the variable parts are the caller identity ARN (from the signing identity), the access point ARN and the object key (both from the request). The deny is implicit in the VPC origin and is NOT liftable: after PutAccessPointPolicy stores an explicit Allow for Principal {"AWS": "*"} on s3:GetObject over <ap-arn>/object/*, the same internet-borne read is still refused with the identical code and message. Positive control: an Internet-origin access point over the same bucket serves the object's bytes with 200.

Emulator: the internet-borne GetObject through the VPC-origin access point ARN is refused with error code NoSuchBucket — the access point ARN is still not resolved on the data path, so the network-origin gate the previous fix added is never reached from a client call. Because the read never resolves to an access point, the AccessDenied message above is never produced, the "an explicit Allow for AWS:* does not lift the VPC deny" step is unobservable, and the Internet-origin positive control read behind it never runs.

The prior issue's Resolution attributes the surviving refusal to the emulator's endpoint announcement (naming the open issue 20261029T083133Z-s3-virtual-hosted-style-bucket-addressing-is-unreachable.md): against an IP-literal endpoint the SDK degrades the access-point ARN away, so the request arrives as `GET /private.txt` with the bare endpoint authority as Host. That is still the emulator's own announced address, and from a client's point of view the operation is refused where AWS serves/denies it, so the call this issue closed is still refused and is reopened here rather than filed as a new defect. Neighbouring open issues that do NOT carry this check's replay key: 20261103T080052Z-s3control-s3-policy-enforcement-and-batch-operations-diverge.md (its cause 3 covers access-point ARN reads answering NoSuchBucket for the plain Internet-origin path) and 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md.

Why: a client that confines a bucket's data path to one VPC by creating a VPC-origin access point still gets no evidence the confinement exists — the control plane reports VpcConfiguration and NetworkOrigin faithfully (those assertions pass) while the data path enforces nothing a client can reach. A caller branching on 403/AccessDenied (non-retryable authorization failure) instead receives NoSuchBucket, which ordinary S3 clients treat as "the bucket is gone" and often as a reason to create or re-provision it.

Fix: the S3 data-plane bucket/host resolution for access-point addressing — an access point ARN presented in the Bucket field (and the address form the SDK derives from it against the emulator's announced endpoint) must resolve against the s3control access-point registry instead of being taken as a literal bucket name, so the already-implemented VPC network-origin gate and the access-point policy evaluation run; and the shared S3 authorization-refusal serializer must render the resource segment as <ap-arn>/object/<key> for that refusal. This includes whatever endpoint/authority the emulator announces to the SDK, since the resolution must work over the address clients are actually given.

Traces: s3control.access-point-vpc-origin-denies-internet-read-rejected.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1): overall 29/44 passed; s3control 26/35 (74%). The check below is the only s3control failure the report marks neither ALREADY FILED nor OWNED.

[mismatch] s3control.access-point-vpc-origin-denies-internet-read-rejected, at check_s3control_access_point_vpc_origin_denies_internet_read_rejected.py:62:

  AssertionError: an internet-borne read of a VPC-origin access point returned 'NoSuchBucket', expected AccessDenied

The assertion is `assert code == "AccessDenied"` on the error raised by `s3.get_object(Bucket=vpc_arn, Key="private.txt")`, where vpc_arn is the AccessPointArn returned by `s3control.create_access_point(AccountId=..., Name=..., Bucket=..., VpcConfiguration={"VpcId": <an available VPC>})`.

The three assertions immediately before it PASSED, pinning the divergence to the data path and not the control-plane projection:
  - GetAccessPoint reported NetworkOrigin == 'VPC';
  - GetAccessPoint echoed VpcConfiguration == {'VpcId': <vpc-id>};
  - GetAccessPoint reported an Endpoints['ipv4'] matching [A-Za-z0-9.-]+(:\d+)? (the check notes AWS advertises the same regional hostnames for both origins, so enforcement cannot come from the endpoint set).

Expected on that call: HTTP 403, Code AccessDenied, Message `User: <caller identity ARN> is not authorized to perform: s3:GetObject on resource: "<AccessPointArn>/object/private.txt" with an explicit deny in a resource-based policy`. Got: Code NoSuchBucket.

Two further requirements of the same check were never reached because the first aborted it: (a) after PutAccessPointPolicy with an explicit Allow for Principal {"AWS": "*"} on s3:GetObject over <ap-arn>/object/*, the internet-borne read must STILL be AccessDenied with the same message; (b) an Internet-origin access point over the same bucket must serve the exact bytes b"vpc-only".

Model evidence (Smithy, com-amazonaws-s3control-awss3controlservicev20180820): VpcConfiguration.VpcId required, @length 1..1024, doc "If this field is specified, this access point will only allow connections from the specified VPC ID."; NetworkOrigin enum values [Internet, VPC].

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run the check in trace_ids (s3control.access-point-vpc-origin-denies-internet-read-rejected), or by hand:
1. ec2 DescribeVpcs(Filters=[{Name: state, Values: [available]}]) and take any available VpcId.
2. s3 CreateBucket (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1); s3 PutObject(Key="private.txt", Body=b"vpc-only").
3. s3control CreateAccessPoint(AccountId, Name=<vpc-ap>, Bucket=<bucket>, VpcConfiguration={"VpcId": <that VpcId>}); keep the returned AccessPointArn.
4. s3control GetAccessPoint(AccountId, Name=<vpc-ap>) — NetworkOrigin=='VPC' and VpcConfiguration=={'VpcId': ...} (these pass today).
5. s3 GetObject(Bucket=<AccessPointArn>, Key="private.txt") from outside that VPC, through the endpoint the emulator announces. Expect HTTP 403 / AccessDenied with message `User: <caller arn> is not authorized to perform: s3:GetObject on resource: "<AccessPointArn>/object/private.txt" with an explicit deny in a resource-based policy`. Observed: NoSuchBucket.
6. s3control PutAccessPointPolicy(AccountId, Name=<vpc-ap>, Policy={Allow, Principal {"AWS":"*"}, Action s3:GetObject, Resource "<AccessPointArn>/object/*"}) and repeat step 5 — the refusal must be unchanged.
7. Positive control: s3control CreateAccessPoint(AccountId, Name=<net-ap>, Bucket=<same bucket>) with no VpcConfiguration, then s3 GetObject(Bucket=<that AccessPointArn>, Key="private.txt") — expect 200 and the bytes b"vpc-only".
