Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.describe-registries-alias-record-is-fully-populated ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless ecr-public.server-side-repository-uri-server-assigned
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public.describe-registries-alias-record-is-fully-populated|run#21|AssertionError|assert _public_ecr_host(host) and path == alias, (registry["registryUri"], alias)
Issue-Key: ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless|_assert_repository_shape#11|AssertionError|assert _public_ecr_host(host) and path == f"{alias}/{name}", uri
Issue-Key: ecr-public.server-side-repository-uri-server-assigned|run#7|AssertionError|assert _public_ecr_host(host), f"expected a public.ecr.<domain> host before the path, got {uri!r}"
Issue-Commit: 4589a545c981676c72550ecd598553d4d546f08e

# ecr-public: registryUri and repositoryUri are minted from the emulator's bare host:port with a constant 'emulator' segment instead of the public.ecr registry authority and the account's registry alias (2 causes)

> _Note: this issue was identified against the code base at commit `4589a545c981676c72550ecd598553d4d546f08e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Every ECR Public response that carries a container-registry URI is composed from the socket the emulator happens to be listening on, so no URI it mints has a registry authority at all. Two distinct response paths diverge (2 causes). Both are HTTP 200 with no error code — these are body-shape divergences in one field each, not status or error divergences.

Shared AWS rule (cited once; it applies to both causes). Smithy (com.amazonaws.ecrpublic): `Registry$registryUri` -> `Url`, @required — "The URI of a public registry. The URI contains a universal prefix and the registry alias."; `Repository$repositoryUri` -> `Url` — "The URI for the repository. You can use this URI for container image push and pull operations."; `RegistryAlias$name` -> `RegistryAliasName` @length 2..50 @pattern ^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$, with `RegistryAlias$primaryRegistryAlias` documented as "the primary registry alias and is displayed in both the repository URL and the image URI used in the docker pull commands"; `Registry$registryId` / `Repository$registryId` -> `RegistryId` @pattern ^[0-9]{12}$ carries the account separately. AWS therefore composes registryUri = `public.ecr.<dns-suffix>` and repositoryUri = `public.ecr.<dns-suffix>/<registryAlias>/<repositoryName>`. What is REQUIRED is that label structure: the fixed leading DNS labels `public` and `ecr`, then a real DNS suffix for the public registry (further non-empty labels, no bare IP and no port), then — for a registry — the account's primary alias as the single path segment, and — for a repository — that alias followed by `Repository$repositoryName` exactly as the request spelled it (slash-nested names survive verbatim). The alias string, the DNS suffix and the repository name of any one run are values from the account's registry state and the request, never part of the requirement.

Shared emulator behavior. In place of that authority the emulator emits its own bound listener address and a constant `emulator` path segment where the registry alias belongs: `127.0.0.1:<port>` + `/emulator`. The port differs on every run and between checks in one replay (35093, 34427, 40831 were observed in this sweep), so nothing derived from the URI is reproducible, and the authority is not a registry name at all.

Shared client impact (Why). repositoryUri is the one field a client uses to push and pull: `docker login public.ecr.<suffix>` then `docker push <repositoryUri>:<tag>`. A client relying on AWS's shape cannot tell the repository belongs to the PUBLIC registry rather than a private `<acct>.dkr.ecr.<region>` one, cannot recover the alias to hand an unauthenticated consumer a pull URI, and gets a value that changes with the emulator's port on each boot. Any tooling that parses the authority (to split registry from repository path, or to assert a regionless/global public registry) breaks.

Shared fix location. The ecr-public component that composes registry and repository URIs — the `Registry` and `Repository` serializers plus the registry state that owns the account's public-registry alias, shared by CreateRepository, DescribeRepositories, DeleteRepository and DescribeRegistries — must mint the documented `public.ecr.<dns-suffix>` authority and source the first path segment from the account's primary `RegistryAlias$name` in registry state rather than from the request Host header, the bound listener, or the constant `emulator`.

Cause 1 of 2: DescribeRegistries$registries[*].registryUri. The registry record is returned with `registryUri` = `127.0.0.1:<port>/emulator` instead of `public.ecr.<dns-suffix>/<primary alias>`. All other asserted registry fields passed (registryId is a 12-digit account, registryArn is `arn:aws:ecr-public::<acct>:registry/<acct>`, verified is a boolean, exactly one ACTIVE primary alias is present and the record is stable across calls), so the divergence is isolated to the authority of this field. Failing check: ecr-public.describe-registries-alias-record-is-fully-populated (AssertionError: ('127.0.0.1:35093/emulator', 'emulator')).

Cause 2 of 2: Repository$repositoryUri, on the create and read-back paths (CreateRepository's own response and DescribeRepositories; the same shape is echoed by DeleteRepository). The field is returned as `127.0.0.1:<port>/emulator/<repositoryName>` instead of `public.ecr.<dns-suffix>/<registryAlias>/<repositoryName>`. This URI is composed independently of the registry record in cause 1 and must be fixed with it. Failing checks: ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless (AssertionError: 127.0.0.1:34427/emulator/sim2real-behav-ecrpub-repo-d397bf8135fa) and ecr-public.server-side-repository-uri-server-assigned (AssertionError: expected a public.ecr.<domain> host before the path, got '127.0.0.1:40831/emulator/sim2real-behav-ecrpub-uri-460c1916f451').

Already-filed issues that do NOT cover these checks: open issue 20261117T002111Z-ecr-public-iam-ecr-public-describerepositories-createrepository-report-a-repositoryuri-that.md reports the same field but carries only the replay key of ecr-public__iam.iam-policy-without-create-repository-blocks-only-that-action; open issue 20261120T172111Z-ecr-public-repository-and-registry-uris-are-minted.md reports it from the WIRE corpus and carries wire fixture/step keys only. Neither carries any of the three behavioral check keys listed here, and the suite marks none of these three checks ALREADY FILED or OWNED. Note also the resolved issue 20261117T034413Z-ecr-public-registryuri-and-repositoryuri-are-minted-as.md, whose fix did not take: the bare host:port plus constant `emulator` segment is still what the emulator serves at this commit.

Traces: ecr-public.describe-registries-alias-record-is-fully-populated (cause 1); ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless, ecr-public.server-side-repository-uri-server-assigned (cause 2).

## Evidence
Behavioral suite: 86/92 passed (93.5%); service ecr-public scored 86/91 (95%) with 5 failures, three of which are these (the other two are OWNED by open issues and skipped).

[mismatch] ecr-public.describe-registries-alias-record-is-fully-populated
  check_ecr_public_describe_registries_alias_record_is_fully_populated.py line 84, in run:
    assert _public_ecr_host(host) and path == alias, (registry["registryUri"], alias)
  AssertionError: ('127.0.0.1:35093/emulator', 'emulator')
  Expected (AWS): HTTP 200; $.registries[0].registryUri == 'public.ecr.<dns-suffix>/<primary RegistryAlias$name>'.
  Actual (emulator): HTTP 200; $.registries[0].registryUri == '127.0.0.1:<port>/emulator'.

[mismatch] ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless
  check_ecr_public_repository_uri_and_arn_are_alias_scoped_and_regionless.py line 73, in _assert_repository_shape:
    assert _public_ecr_host(host) and path == f"{alias}/{name}", uri
  AssertionError: 127.0.0.1:34427/emulator/sim2real-behav-ecrpub-repo-d397bf8135fa
  (fails on CreateRepository's own response, before the DescribeRepositories/DeleteRepository/nested-name beats are reached)

[mismatch] ecr-public.server-side-repository-uri-server-assigned
  check_ecr_public_server_side_repository_uri_server_assigned.py line 25, in run:
    assert _public_ecr_host(host), f"expected a public.ecr.<domain> host before the path, got {uri!r}"
  AssertionError: expected a public.ecr.<domain> host before the path, got '127.0.0.1:40831/emulator/sim2real-behav-ecrpub-uri-460c1916f451'

The required-host predicate, defined identically in all three checks, is:
  def _public_ecr_host(host): labels = host.split('.'); return labels[:2] == ['public','ecr'] and len(labels) > 2 and all(labels[2:])
i.e. only the leading `public` and `ecr` labels plus the existence of a further non-empty domain are required — the domain itself is the target environment's and is NOT pinned. The emulator's value satisfies neither: there is no `public.ecr.` prefix and no domain, just an IP literal and a port.

No status-code and no error-code divergence: every one of these calls answered HTTP 200 and all surrounding assertions passed (registryArn `arn:aws:ecr-public::<acct>:registry/<acct>`, the empty region field in `arn:aws:ecr-public::<acct>:repository/<name>`, registryId as a 12-digit account, one ACTIVE primary alias, URI stability across describes). The ports (35093, 34427, 40831), the generated repository names and any one run's alias are per-run values; the fixed parts of the requirement are the leading `public` and `ecr` labels, a real DNS suffix with no port, the account's primary alias as the first path segment, and the repository name as the final segment.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an ecr-public client at it in us-east-1.
2. Create a repository: `aws ecr-public create-repository --repository-name <name> --region us-east-1`.
3. Inspect `$.repository.repositoryUri` and split it on the first '/':
   -> AWS: authority `public.ecr.<dns-suffix>` (fixed `public`,`ecr` labels then a real domain, no port), path `<registryAlias>/<name>`.
   -> Emulator: authority `127.0.0.1:<port>` (bare IP with a port, no `public.ecr.` labels), path `emulator/<name>`. (Cause 2)
4. Read it back: `aws ecr-public describe-repositories --repository-names <name> --region us-east-1` — `$.repositories[0].repositoryUri` shows the identical defect, and so does the repository echoed by `delete-repository --force`. (Cause 2)
5. Read the registry: `aws ecr-public describe-registries --region us-east-1`. `$.registries[0].registryUri` is `127.0.0.1:<port>/emulator` instead of `public.ecr.<dns-suffix>/<primary alias>`, even though `$.registries[0].aliases[?primaryRegistryAlias].name` is present and ACTIVE. (Cause 1)
6. Restart the emulator and repeat step 2 — the authority changes with the new listener port, confirming the URI is built from the bound socket rather than from registry state.
7. After fixing, re-replay ecr-public.describe-registries-alias-record-is-fully-populated (cause 1) and ecr-public.repository-uri-and-arn-are-alias-scoped-and-regionless plus ecr-public.server-side-repository-uri-server-assigned (cause 2) to confirm both causes are closed.
