Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected
Issue-Key: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected|run#20|AssertionError|assert (labels[:4] == [account_id, "dkr", "ecr", ctx.region] and len(labels) > 4
Issue-Commit: 5b88798ccff0ec7f765809a75ef2c7dfb1d7aad0

# cloudwatch__ecr__ecs: a CFN-deployed ECS task definition's container image is rewritten to the emulator's bare host:port instead of the ECR registry URI the template declared (1 cause)

> _Note: this issue was identified against the code base at commit `5b88798ccff0ec7f765809a75ef2c7dfb1d7aad0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskDefinition

## Description
Cause 1 of 1: a container image URI that a CloudFormation template declares literally as an ECR registry DNS name is not preserved — the task definition the emulator registers and reads back carries the emulator's bare socket address `<host>:<port>` as the image authority. Failing check: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected.

AWS: the template's `AWS::ECS::TaskDefinition` declares `ContainerDefinitions[0].Image` as `{"Fn::Sub": "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/<repoName>:latest"}`. CloudFormation resolves `Fn::Sub` by substituting only the pseudo-parameters (`AWS::AccountId` -> the stack's 12-digit account id, `AWS::Region` -> the stack's region) and leaves every other character of the string untouched; ECS then stores `ContainerDefinition$image` verbatim (Smithy: ecs `ContainerDefinition$image` — the image used to start a container, "<repository-url>/<image>:<tag>"; ECS does not rewrite or canonicalise it, and DescribeTaskDefinition echoes what was registered). The resulting image is therefore `<accountId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>:latest` — the FIXED text is the `dkr` and `ecr` labels in the ordering `<accountId>.dkr.ecr.<region>.`, the `/` separator and the `:latest` tag; the VARIABLE parts (account id, region, repository name) come from the stack's identity and the template. The check asserts exactly that rule (first four authority labels == [accountId, "dkr", "ecr", region] plus at least one non-empty suffix label, path == `<repoName>:latest`), so an emulator-served DNS suffix is permitted.

Emulator: the stack deploys and DescribeTaskDefinition answers HTTP 200 (no status or error-code divergence), but `taskDefinition.containerDefinitions[0].image` is `127.0.0.1:36223/sim2real-xsvc-repo-d1812b63836c:latest` — the emulator's own listening socket, with no account-id label, no `dkr`/`ecr` labels and no region label, even though the declared string contained `.dkr.ecr.` and `.amazonaws.com` literally and no `GetAtt ...RepositoryUri` was used anywhere in the template. Hypothesis: a component rewrites ECR-looking image URIs to the local registry endpoint on registration (or `Fn::Sub` pseudo-parameter resolution substitutes the emulator endpoint rather than the account id and region); the recording shows only that the registered value was replaced by the serving socket.

Why: a client gets back an image reference it never asked for. Anything that reads the registered image to make a decision — verifying the task definition points at this account's registry, extracting the account id or region for cross-account/cross-region checks, matching the image against an ECR repository policy condition such as `aws:PrincipalAccount`, diffing a deployed task definition against the template that produced it, or re-registering a revision from a describe — sees a value that does not round-trip and cannot be parsed as a registry URI. Since the authority is the per-run socket, the stack's output is also not reproducible between runs.

Fix: the CloudFormation `AWS::ECS::TaskDefinition` provisioner together with the ECS RegisterTaskDefinition/DescribeTaskDefinition path must store and echo `ContainerDefinition$image` exactly as supplied, and `Fn::Sub` resolution must substitute only the pseudo-parameters; any image-URI redirection to the emulator's own registry must happen at pull time, not by mutating the stored task-definition field. If the intent is that an announced ECR authority must also be reachable, the registry-DNS form has to be served (host alias) rather than substituted into stored state.

Note: 20261116T122519Z-cloudwatch-ecr-ecs-the-ecr-repositoryuri-handed-to.md carries this check's replay key but is marked FIXED, so it no longer covers this failure; the check still fails on the same assertion with the same bare host:port, and the template path here is a literal `Fn::Sub` string, not `GetAtt RepositoryUri`, so the defect is in the task-definition image round-trip rather than only in the ECR `repositoryUri` serializer.

Traces: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected (assertion at line 97 of the check).

## Evidence
Suite: service `cloudwatch__ecr__ecs` scored 15/27 (56%), 7 failed (plus 5 unloadable); this is the only failure not owned by an open issue.

[mismatch] cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected
  check_cloudwatch__ecr__ecs_deploy_cfn_ecr_repository_policy_restricts_cross_account_pull_rejected.py line 97:
    assert (labels[:4] == [account_id, "dkr", "ecr", ctx.region] and len(labels) > 4 and all(labels[4:]) and path == f"{repo_name}:latest")
  AssertionError: expected task def image 000000000000.dkr.ecr.us-west-2.<domain>/sim2real-xsvc-repo-d1812b63836c:latest, got '127.0.0.1:36223/sim2real-xsvc-repo-d1812b63836c:latest'

Template input (verbatim): ContainerDefinitions[0].Image = {"Fn::Sub": "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/<repoName>:latest"}.
Expected (AWS): CreateStack succeeds; DescribeTaskDefinition 200 with containerDefinitions[0].image == `<accountId>.dkr.ecr.<region>.<dns-suffix>/<repoName>:latest` (only the two pseudo-parameters substituted, no port component).
Actual (emulator): DescribeTaskDefinition 200 with containerDefinitions[0].image == `127.0.0.1:36223/<repoName>:latest` — the serving socket replaced the declared registry authority.
No status, error code or message divergence: the 200 body does not round-trip the registered value. The account id, region, repository name and port in the recorded run are per-run values filling the template, not part of the requirement.
Because the check aborts here, its later assertions (the DenyForeignAccountPull statement persisting verbatim via GetRepositoryPolicy, and TagResource under a caller-scoped Deny failing with AccessDeniedException and the message "User: <callerArn> is not authorized to perform: ecr:TagResource on resource: <repositoryArn> with an explicit deny in a resource-based policy") were never reached and are not claimed here.

## Steps to Reproduce
1. Point an AWS client at the emulator and call sts:GetCallerIdentity to get the account id.
2. CreateStack with a template containing an `AWS::ECR::Repository` (RepositoryName <repoName>) and an `AWS::ECS::TaskDefinition` whose ContainerDefinitions[0].Image is {"Fn::Sub": "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/<repoName>:latest"}; wait for CREATE_COMPLETE.
3. Call ecs:DescribeTaskDefinition on the family and read containerDefinitions[0].image, splitting authority from path on the first '/'.
   -> AWS: authority labels are `<accountId>`, `dkr`, `ecr`, `<region>`, then the DNS suffix; no port; path == `<repoName>:latest`.
   -> Emulator: authority is the emulator's bare `127.0.0.1:<port>`.
4. Re-replay check cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected to confirm the fix.
