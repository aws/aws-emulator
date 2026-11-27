Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected
Issue-Key: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected|run#20|AssertionError|assert (labels[:4] == [account_id, "dkr", "ecr", ctx.region] and len(labels) > 4
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__ecr__ecs: a CFN-deployed ECS task definition's container image is the emulator's bare host:port instead of a <registryId>.dkr.ecr.<region>.<suffix> registry authority

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskDefinition

## Description
Cause 1 of 1: in the cross-account-pull repository-policy scenario, the AWS::ECS::TaskDefinition the stack deploys takes its container image from the stack's AWS::ECR::Repository, and the image read back from ECS has no ECR registry authority at all — it is the emulator's bare socket address <host>:<port> followed by '<repositoryName>:latest'. Failing check: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected.

AWS: Smithy (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921) Repository$repositoryUri — "The URI for the repository. You can use this URI for container image push and pull operations." AWS composes it as <registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>, and a task definition whose ContainerDefinitions[].Image is derived from that repository (Fn::Sub/Fn::GetAtt over RepositoryUri) echoes the same authority. FIXED: the label ordering with the literal labels 'dkr' and 'ecr' in the positions <account>.dkr.ecr.<region>., at least one further non-empty suffix label, and no port component; the path is '<repositoryName>:latest'. VARIABLE, taken from the request and state: the 12-digit account/registry id, the region of the call and the repository name. The check asserts exactly that rule (first four authority labels == [account, 'dkr', 'ecr', region], len(labels) > 4, all suffix labels non-empty), so an emulator-served DNS suffix is acceptable and amazonaws.com is not required.

Emulator: the stack deploys and DescribeTaskDefinition answers HTTP 200, but containerDefinitions[0].image is '127.0.0.1:<port>/<repositoryName>:latest' — no registry id, no 'dkr'/'ecr'/region labels, and an explicit port. The check aborts there, so everything it asserts afterwards (the persisted DenyForeignAccountPull statement with its aws:PrincipalAccount condition, the AccessDeniedException refusal of ecr:TagResource under the caller-scoped Deny, the absence of the tag, and the positive control after narrowing the policy) is never reached and is not claimed as diverging here.

Why: the point of deriving an image from the stack's own repository is that downstream runtimes address the registry the stack made. A client that feeds this value to a task definition, a buildspec push target or `docker push` reaches the emulator's control-plane socket rather than a registry, and cannot recover the registry id or region from it for cross-account/cross-region or policy-condition logic. Because the authority is the per-run socket, every template or task definition derived from it is irreproducible between runs.

Fix: the component that computes the ECR repository URI from state (registry id, region, repository name) and the CFN attribute/Fn::Sub resolution that feeds it into AWS::ECS::TaskDefinition must emit a registry DNS authority of the documented shape, and the authority announced must also be one the emulator answers on — both halves, not one traded for the other.

Traces: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected (assertion at line 97 of its check file). Related open issue 20261124T024116Z-cloudformation-cloudwatch-ecr-ecs-elbv2-logs-a-repository.md reports the same URI shape through ecr:DescribeRepositories in the nested-stack scenario, but carries a different service grouping and neither of this check's replay keys, so it does not cover this check; the ECR-side issues it names (20261123T002251Z, 20261116T084727Z, 20261118T072013Z) likewise do not.

## Evidence
Suite: service cloudwatch__ecr__ecs scored 17/27 (63%), 5 failed; the other four are OWNED by open issues and skipped. This one is not owned.

[mismatch] cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected
  check line 97:
    task_def = ecs.describe_task_definition(taskDefinition=family)["taskDefinition"]
    host, _, path = task_def["containerDefinitions"][0]["image"].partition("/"); labels = host.split(".")
    assert labels[:4] == [account_id, "dkr", "ecr", ctx.region] and len(labels) > 4 and all(labels[4:]) and path == f"{repo_name}:latest"
  AssertionError: expected task def image <account>.dkr.ecr.<region>.<domain>/<repo>:latest, got '127.0.0.1:35905/<repo>:latest'

Expected (AWS): HTTP 200 and containerDefinitions[0].image == <registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>:latest, no port.
Actual (emulator): HTTP 200 and the emulator's bare socket as the authority; labels[:4] is ['127','0','0','1:35905'].

No status, error code or error message divergence is involved — the 200 body is wrong. The host, port and generated repository name of the recorded run are per-run values filling the template, not requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateStack with an AWS::ECR::Repository, a RepositoryPolicyText carrying a caller-scoped Deny on ecr:TagResource plus a DenyForeignAccountPull statement (Principal '*' narrowed by Condition StringNotEquals aws:PrincipalAccount = the caller's account), and an AWS::ECS::TaskDefinition whose ContainerDefinitions[].Image is derived from the repository's URI with ':latest'. Wait CREATE_COMPLETE.
3. ecs:DescribeTaskDefinition(taskDefinition=<family>) and split containerDefinitions[0].image on the first '/'.
   -> AWS: the authority's labels are <registryId>, 'dkr', 'ecr', <region>, then a DNS suffix, with no port; the path is '<repositoryName>:latest'.
   -> Emulator: the authority is '127.0.0.1:<port>'.
4. Re-replay the check in trace_ids to confirm the fix; it then continues into the repository-policy persistence, the ecr:TagResource AccessDeniedException leg and the narrowed-policy positive control, which this run never reached.
