Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-serverless-cache-iam-user-endpoint-discovery
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateUser|2xx->2xx|$.CreateUserResponse.CreateUserResult.MinimumEngineVersion
Issue-Key: elasticache|DescribeUsers|2xx->2xx|$.DescribeUsersResponse.DescribeUsersResult.Users.member.MinimumEngineVersion
Issue-Key: elasticache|DescribeUsers|2xx->2xx|$.DescribeUsersResponse.DescribeUsersResult.Users.member.UserGroupIds
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# ElastiCache user records carry the wrong MinimumEngineVersion and never list UserGroupIds membership

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
2 net-new wire causes on the elasticache RBAC user-projection path. Neither step was marked owned by an open issue and neither was blocked by a stub. The open issue 20261030T130325Z-elasticache-reference-catalogs-diverge-from-aws-engine-versions.md covers the engine-version/parameter-group catalogs, not the MinimumEngineVersion stamped onto a User record, and it does not carry these fixtures' replay keys.

Cause 1 of 2: CreateUser stamps the wrong MinimumEngineVersion on the user it returns. AWS: Smithy - CreateUser's output is the User shape, whose MinimumEngineVersion member reports the lowest engine version that supports the user's access configuration; the model states no fixed value (recorded behavior only for the value). Recorded behavior: for a valkey RBAC user created with an AccessString, AWS returns MinimumEngineVersion '7.2'. Emulator: returns '7.0' - '$.CreateUserResponse.CreateUserResult.MinimumEngineVersion: value_mismatch (expected 7.2, got 7.0)'. Why: a client that gates on MinimumEngineVersion to decide which engine versions may attach this user (or which replication groups can use its group) computes a lower floor than AWS and plans a cache on an engine version that AWS would refuse. Fix: the User projection in the CreateUser handler/serializer in elasticache - MinimumEngineVersion must be derived from the user's engine/access configuration rather than hard-coded to the emulator's default. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#7.

Cause 2 of 2: DescribeUsers repeats the wrong MinimumEngineVersion and returns an empty UserGroupIds for a user that belongs to a group. AWS: Smithy - the User shape carries MinimumEngineVersion and UserGroupIds (target UserGroupIdList), the list of user groups the user is a member of; DescribeUsers returns Users as that same shape. Recorded behavior: the user reports MinimumEngineVersion '7.2' and UserGroupIds containing the group it was added to. Emulator: MinimumEngineVersion '7.0' and UserGroupIds serialized as the empty string instead of a member list - '$.DescribeUsersResponse.DescribeUsersResult.Users.member.MinimumEngineVersion: value_mismatch (expected 7.2, got 7.0)' and '$.DescribeUsersResponse.DescribeUsersResult.Users.member.UserGroupIds: type_mismatch (expected {member: <user-group-id>}, got "")'. Why: a client that reads UserGroupIds to find out which groups a user belongs to - before deleting the user, or to audit which caches it can reach - sees no membership at all and concludes the user is unattached. Fix: the User projection in the DescribeUsers handler/serializer plus the user<->user-group back-reference in elasticache state. Note: the empty UserGroupIds in this recording is also reachable because the user group in this fixture is never created (see the companion issue on the user/user-group lifecycle); the membership back-reference must still be populated once the group exists. Traces: elasticache-serverless-cache-iam-user-endpoint-discovery#task#12.

## Evidence
Both quotes are from the wire replay of corpus simparity-wire-aws-v1 (verdict: mismatch). The user-group id below is a per-run value filling the shape; the member structure and the engine version are the requirement.

Cause 1 - fixture elasticache-serverless-cache-iam-user-endpoint-discovery, step task#7, operation CreateUser; expected status 200, actual status 200:
  $.CreateUserResponse.CreateUserResult.MinimumEngineVersion: value_mismatch (expected '7.2', got '7.0')

Cause 2 - same fixture, step task#12, operation DescribeUsers; expected status 200, actual status 200:
  $.DescribeUsersResponse.DescribeUsersResult.Users.member.MinimumEngineVersion: value_mismatch (expected '7.2', got '7.0')
  $.DescribeUsersResponse.DescribeUsersResult.Users.member.UserGroupIds: type_mismatch (expected {'member': '<user-group-id>'}, got '')

Smithy cited: CreateUser output shape User with members MinimumEngineVersion and UserGroupIds (target UserGroupIdList, type list); DescribeUsers output Users (target UserList) of the same User shape.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (git_clone, then replay_wire_corpus).
2. Cause 1: call elasticache CreateUser for a valkey RBAC user with an AccessString - read MinimumEngineVersion in the CreateUserResult and observe '7.0' where AWS reports '7.2'. Replay fixture elasticache-serverless-cache-iam-user-endpoint-discovery step task#7.
3. Cause 2: add that user to a user group, then call DescribeUsers for it - observe MinimumEngineVersion '7.0' and UserGroupIds serialized as an empty string where AWS reports '7.2' and a UserGroupIds member list naming the group. Step task#12.
