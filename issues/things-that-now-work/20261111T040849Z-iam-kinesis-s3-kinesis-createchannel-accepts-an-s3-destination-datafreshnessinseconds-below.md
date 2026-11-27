Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__s3.s3-channel-echoes-freshness-and-refuses-below-minimum-while-put-records-accepted
Issue-Key: iam__kinesis__s3.s3-channel-echoes-freshness-and-refuses-below-minimum-while-put-records-accepted|_create_channel_error#11|AssertionError|raise AssertionError(f"CreateChannel accepted {kwargs['ChannelName']!r}: {resp['ChannelDescription']!r}")
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# Kinesis CreateChannel accepts an S3 destination DataFreshnessInSeconds below the documented minimum

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
One net-new root cause.

Cause 1 of 1: CreateChannel with an S3DestinationConfiguration whose DataFreshnessInSeconds is below the allowed minimum is created instead of refused.
AWS: Smithy — S3DestinationConfiguration.DataFreshnessInSeconds is range-constrained (minimum 300); a value under it is refused with a client validation error naming the member and its minimum, and no channel is created.
Emulator: the call returns 200 and the channel is created with ChannelStatus CREATING and S3DestinationConfiguration.DataFreshnessInSeconds echoed as 299.
Why: a client's invalid configuration is silently accepted, so it believes a freshness it will never get, and a validation test for the documented floor passes when it should fail; the channel also has to be cleaned up where AWS created nothing.
Fix: the CreateChannel input validation for S3DestinationConfiguration.DataFreshnessInSeconds — enforce the modelled minimum and refuse with the service's validation error, creating no channel.
Note: the open issue 20261110T221019Z-glue-iam-kinesis-s3-s3tables-kinesis-createchannel-accepts-an-s3-tables-datafreshnessinseconds-below.md records the same floor for an S3 Tables destination under a different service heading; this check's replay key is not on it.
Traces: iam__kinesis__s3.s3-channel-echoes-freshness-and-refuses-below-minimum-while-put-records-accepted.

## Evidence
Check iam__kinesis__s3.s3-channel-echoes-freshness-and-refuses-below-minimum-while-put-records-accepted [mismatch]:
AssertionError: CreateChannel accepted '<channel>': {'ChannelName': '<channel>', 'ChannelARN': 'arn:aws:kinesis:<region>:<account>:channel/<channel>', 'ChannelStatus': 'CREATING', ..., 'S3DestinationConfiguration': {'DataFreshnessInSeconds': 299, ...}}

Expected: a client validation refusal naming DataFreshnessInSeconds and its 300-second minimum, with no channel created.
Actual: HTTP 200 and a created channel echoing 299.

## Steps to Reproduce
1. Create an on-demand Kinesis stream, a bucket and a service execution role.
2. CreateChannel with S3DestinationConfiguration.DataFreshnessInSeconds = 299.
3. Observe HTTP 200 and a channel in CREATING instead of a validation refusal.
