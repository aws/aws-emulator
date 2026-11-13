Issue-Source: trace-sweep-divergence
Issue-Trace: es-vpc-endpoint-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-vpc-endpoint-errors es-engine-version-and-instance-catalog
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeVpcEndpoints|4xx->4xx|$.message
Issue-Key: es|UpdateVpcEndpoint|4xx->4xx|$.message
Issue-Key: es|DeleteVpcEndpoint|4xx->4xx|$.message
Issue-Key: es|DescribeInstanceTypeLimits|4xx->4xx|$.message
Issue-Commit: 7f3f2187a2100f71560b62bea9ff35bd72db691b

# es ValidationException message text diverges: @pattern anchors stripped and the instanceType enum value set is not AWS's

> _Note: this issue was identified against the code base at commit `7f3f2187a2100f71560b62bea9ff35bd72db691b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpoints

## Description
4 net-new root causes on the `es` service token (the OpenSearch/Elasticsearch control plane served on endpoint prefix `es`). All four are the same class of defect: the status (400) and the error class (ValidationException) are right, but the *text* of the `message` the emulator renders for a constraint violation is not the text AWS renders — the constraint rendering drops the modeled regex anchors, and the enum-value rendering enumerates a value set that is not AWS's. Each cause below is a separate replay unit and must be re-replayed separately before this issue is closed.

RELATED OPEN WORK (does NOT cover these steps — none of these replay keys appears in them): 20261101T032039Z-es-listinstancetypedetails-serves-a-legacy-instance-type-catalog.md (the legacy instance-type catalog behind ListInstanceTypeDetails) and 20261029T190009Z-opensearch-the-package-offering-version-and-instance-type.md (synthetic catalogs on the `opensearch` token). Cause 4 below is the same catalog surfacing inside a refusal message, and is filed here because it carries its own replay key.

Cause 1 of 4 — DescribeVpcEndpoints with an id that violates the member pattern (fixture es-vpc-endpoint-errors, step task#2).
AWS: Smithy: DescribeVpcEndpoints POST /2015-01-01/es/vpcEndpoints/describe, input VpcEndpointIds: VpcEndpointIdList (required); member VpcEndpointId @length(min 5, max 256) @pattern("^aos-[a-zA-Z0-9]*$"); error ValidationException (client, HTTP 400). The recording shows AWS answering 400 ValidationException with the standard aggregate template: `1 validation error detected: Value '[<submitted list>]' at 'vpcEndpointIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 5, Member must satisfy regular expression pattern: ^aos-[a-zA-Z0-9]*$]`. Fixed text: everything except the submitted value; the only variable parts are the member path (`vpcEndpointIds`), the echoed value, and the limits/pattern, which come from the model trait — so the pattern is reproduced EXACTLY as the trait writes it, anchors included.
Emulator: answers 400 ValidationException with the same template but renders the pattern with its anchors removed — `Member must satisfy regular expression pattern: aos-[a-zA-Z0-9]*` instead of `^aos-[a-zA-Z0-9]*$`.
Why: the message is the only place AWS states the accepted id syntax. `aos-[a-zA-Z0-9]*` is an UNANCHORED expression, so a client (or an SDK test, or a human) that compiles the quoted regex to pre-validate ids accepts strings AWS refuses (anything merely CONTAINING an `aos-` run, e.g. `vpce-aos-1`), and any assertion on the refusal text fails.
Fix: the shared constraint-violation message formatter in this service's request validation — the branch that renders a @pattern trait into `Member must satisfy regular expression pattern: <pattern>` must emit the trait verbatim rather than trimming `^`/`$`.
Traces: es-vpc-endpoint-errors#task#2.

Cause 2 of 4 — UpdateVpcEndpoint with a pattern-violating VpcEndpointId (fixture es-vpc-endpoint-errors, step task#7).
AWS: Smithy: UpdateVpcEndpoint POST /2015-01-01/es/vpcEndpoints/update, input VpcEndpointId (required) @length(5,256) @pattern("^aos-[a-zA-Z0-9]*$"); error ValidationException (client, 400). Recording: 400 with the scalar form of the template — `1 validation error detected: Value '<submitted id>' at 'vpcEndpointId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aos-[a-zA-Z0-9]*$`. Fixed text apart from the echoed id and the member path.
Emulator: 400 with the same sentence but `... regular expression pattern: aos-[a-zA-Z0-9]*` — anchors stripped again.
Why: same as cause 1; this is the mutation path, so a client that surfaces the refusal to a user documents an id syntax AWS does not use.
Fix: same formatter as cause 1 (scalar-member branch).
Traces: es-vpc-endpoint-errors#task#7.

Cause 3 of 4 — DeleteVpcEndpoint with a pattern-violating VpcEndpointId (fixture es-vpc-endpoint-errors, step task#8).
AWS: Smithy: DeleteVpcEndpoint DELETE /2015-01-01/es/vpcEndpoints/{VpcEndpointId}, VpcEndpointId @httpLabel (required) @length(5,256) @pattern("^aos-[a-zA-Z0-9]*$"); errors BaseException(400, defaulted), ResourceNotFoundException, DisabledOperationException, InternalException. Recording: 400 with `1 validation error detected: Value '<submitted id>' at 'vpcEndpointId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aos-[a-zA-Z0-9]*$`.
Emulator: 400 with the anchors stripped from the quoted pattern.
Why: same as cause 1. It is listed as its own cause because the value arrives on a URI label here rather than in the body, i.e. a different validation entry point reaches the same formatter; a fix that only touches body validation leaves this step failing.
Fix: same formatter as cause 1, reached from httpLabel validation.
Traces: es-vpc-endpoint-errors#task#8.

Cause 4 of 4 — DescribeInstanceTypeLimits with an instanceType outside the enum (fixture es-engine-version-and-instance-catalog, step task#5).
AWS: Smithy: DescribeInstanceTypeLimits GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}, InstanceType @httpLabel (required) targets the enum OpenSearchPartitionInstanceType; error ValidationException (client, 400). The recording shows 400 with the enum form of the template: `1 validation error detected: Value '<submitted type>' at 'instanceType' failed to satisfy constraint: Member must satisfy enum value set: [<the service's search instance types>]`. Fixed text apart from the echoed value and the bracketed value set; the value set is a requirement because it is the service's own accepted-value list, and the recording's list is the current one — it contains current-generation families (m7i.2xlarge.search, c8g.16xlarge.search, i8ge.12xlarge.search, or2.16xlarge.search, oi2.4xlarge.search, om2.2xlarge.search, r8gd.medium.search, r7i.12xlarge.search, ...) alongside the legacy ones.
Emulator: 400 ValidationException with the same `1 validation error detected: ... Member must satisfy enum value set: [...]` template, but the bracketed value set does not match the recording — the replay reports `$.message: value_mismatch` as the ONLY diverging key (status and error class agree). The reporter truncates the detail inside the expected list, so the emulator's list is not quotable from the replay output; what is established is that the enumerated value set it serves is not AWS's.
Hypothesis: it is the same legacy reference table reported in 20261101T032039Z-es-listinstancetypedetails-serves-a-legacy-instance-type-catalog.md (a c5/m5/r5-era catalog with no current-generation families), rendered here into the refusal message; note the ON-DISK model's OpenSearchPartitionInstanceType enum is itself an older snapshot (it stops at or1.*/t4g.*), so rendering the on-disk enum verbatim will NOT reproduce the recording either — the accepted-value list must be the service's current one.
Why: this message is how a client discovers which instance types the API accepts. Enumerating a stale set tells the caller that types AWS does accept are invalid, and omits the types it must actually choose from, so capacity-selection code written against the emulator is wrong against AWS.
Fix: the instance-type reference data used by this operation's validation, plus the enum branch of the constraint-violation message formatter that renders it.
Traces: es-engine-version-and-instance-catalog#task#5.

The ids, limits and type names above are this recording's values filling the templates; the requirement is the template's fixed text plus the model trait / accepted-value list it quotes.

## Evidence
Cause 1 — es-vpc-endpoint-errors#task#2, DescribeVpcEndpoints, expected 400 / actual 400, diverging key $.message only:
  expected "1 validation error detected: Value '[vpce-0000000000absent]' at 'vpcEndpointIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 5, Member must satisfy regular expression pattern: ^aos-[a-zA-Z0-9]*$]"
  got      "1 validation error detected: Value '[vpce-0000000000absent]' at 'vpcEndpointIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 5, Member must satisfy regular expression pattern: aos-[a-zA-Z0-9]*]"
  (divergence_key service=es|DescribeVpcEndpoints|status=400->400|keys=$.message)

Cause 2 — es-vpc-endpoint-errors#task#7, UpdateVpcEndpoint, expected 400 / actual 400:
  expected "1 validation error detected: Value 'vpce-0000000000absent' at 'vpcEndpointId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aos-[a-zA-Z0-9]*$"
  got      "... Member must satisfy regular expression pattern: aos-[a-zA-Z0-9]*"
  (divergence_key service=es|UpdateVpcEndpoint|status=400->400|keys=$.message)

Cause 3 — es-vpc-endpoint-errors#task#8, DeleteVpcEndpoint, expected 400 / actual 400: identical expected/actual pair to cause 2 with member path 'vpcEndpointId' (divergence_key service=es|DeleteVpcEndpoint|status=400->400|keys=$.message).

Cause 4 — es-engine-version-and-instance-catalog#task#5, DescribeInstanceTypeLimits, expected 400 / actual 400, diverging key $.message only: expected "1 validation error detected: Value 't9.nonexistent.search' at 'instanceType' failed to satisfy constraint: Member must satisfy enum value set: [m6g.xlarge.search, im4gn.8xlarge.search, m3.xlarge.search, r4.16xlarge.search, c6g.4xlarge.search, m5.xlarge.search, r7gd.4xlarge.search, m8g.12xlarge.search, ... m7i.2xlarge.search, r7i.12xlarge.search, i8ge.18xlarge.search, ... oi2.12xlarge.search, r8g.large.search, c8g.16xlarge.search, ...]" — the replay detail is truncated by the reporter inside this expected list, so the emulator's rendered value set is not quoted in the output; the verdict is $.message value_mismatch with status and error class matching.

The echoed values ('vpce-0000000000absent', 't9.nonexistent.search') are this recording's inputs; the fixed template text and the quoted pattern/value set are the requirement.

## Steps to Reproduce
1. Replay fixture es-vpc-endpoint-errors step task#2 (POST /2015-01-01/es/vpcEndpoints/describe with a vpcEndpointIds member that violates the @pattern) and compare the `message` body member against the recording: the quoted regex must be `^aos-[a-zA-Z0-9]*$`, anchors included.
2. Replay fixture es-vpc-endpoint-errors step task#7 (POST /2015-01-01/es/vpcEndpoints/update) and compare the scalar-member form of the same sentence.
3. Replay fixture es-vpc-endpoint-errors step task#8 (DELETE /2015-01-01/es/vpcEndpoints/{VpcEndpointId}) and compare it again — the value arrives on a URI label here.
4. Replay fixture es-engine-version-and-instance-catalog step task#5 (GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType} with an instanceType outside the enum) and diff the enumerated value set in `message` against the recording.
5. All four steps must pass before this issue is closed.

## Status at close

Causes 1, 2 and 3 are fixed: steps 1-3 replay clean (`es|DescribeVpcEndpoints`,
`es|UpdateVpcEndpoint` and `es|DeleteVpcEndpoint` report `fixed`). A `@pattern` clause now
quotes the expression the API publishes for the member's shape, and a constraint broken by a
collection's elements is stated at the collection member over the element shape's whole set,
quoting the collection the request carried.

Cause 4's rule is implemented — the clause enumerates the accepted-value list the SERVICE
publishes (its instance-type catalog) instead of the older snapshot this model carries — but
its replay key `es|DescribeInstanceTypeLimits|4xx->4xx|$.message` is STILL OPEN, on two
residuals outside the refusal seam:

* the catalog's exact membership, which is
  `20261101T032039Z-es-listinstancetypedetails-serves-a-legacy-instance-type-catalog.md`'s
  subject — the replay still reports `es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]`
  as an open cause of the same fixture, so the catalog is not yet the one AWS publishes and no
  message rendering it can be either. That issue is this cause's blocking dependency;
* the ORDER inside the bracketed set, which matches no order computable from the values: the
  recording's first eight values are not bucket-monotone under the fleet's hashed value-set
  order at any table width (16-2048), on either the value or the enum-name spelling, plain or
  Java-spread.
