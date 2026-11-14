Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.e2e-data-source-attach-ingest-detach-flow opensearch.e2e-data-source-attachment-ingest-search-detach
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.e2e-data-source-attach-ingest-detach-flow|run#27|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: opensearch.e2e-data-source-attachment-ingest-search-detach|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# opensearch DescribeDataSourceAttachment's ResourceNotFoundException is emulator-worded: "Data source <arn> is not attached to application <id>." where AWS names the missing attachment (1 cause)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataSourceAttachment

## Description
Cause 1 of 1 — DescribeDataSourceAttachment's not-found refusal uses the emulator's own sentence instead of AWS's.
AWS: Smithy: DescribeDataSourceAttachment (POST /2021-01-01/opensearch/application/{id}/describeDataSourceAttachment) declares ResourceNotFoundException (errorKind client, httpError 409) for an `id`/`dataSourceArn` pair that names no attachment. The refusal names the ABSENT ATTACHMENT, keyed by both inputs: "Data source attachment not found for application <id> and data source <dataSourceArn>". Fixed text: "Data source attachment not found for application ", " and data source "; variable parts: the `id` member from the request and the `dataSourceArn` member from the request. The recorded response shows that message for exactly this call after a successful DetachDataSource.
Emulator: the same call answers ResourceNotFoundException with "Data source <dataSourceArn> is not attached to application <id>." — different fixed text, the two values transposed, and a trailing full stop AWS does not write.
Why: the refusal is the documented observable consequence of a detach, so a client that confirms a detach by matching the not-found message (or that surfaces it to an operator) sees text that never appears in AWS, and anything keyed on the message does not fire. The error code and status are right; only the wording diverges.
Note: open issue 20261027T054419Z-opensearch-5-net-new-root-causes-attachdatasource-refuses.md mentions "attachment not-found message wording" but does not carry these two checks' replay keys, so it does not cover them.
Fix: the message rendered by the DescribeDataSourceAttachment handler's not-found path in the opensearch service crate's ops for that operation.
Traces: opensearch.e2e-data-source-attach-ingest-detach-flow, opensearch.e2e-data-source-attachment-ingest-search-detach

## Evidence
Expected message 'Data source attachment not found for application sfi9sv1baaaa and data source arn:aws:es:us-west-2:000000000000:domain/sim2real-ds-823c42d681d0'; got 'Data source arn:aws:es:us-west-2:000000000000:domain/sim2real-ds-823c42d681d0 is not attached to application sfi9sv1baaaa.' (the application id and the domain ARN are per-run values that fill the template; the fixed text and the member order are the requirement). The second check shows the same divergence with a different domain ARN: expected '... for application sfi9sv1baaaa and data source arn:aws:es:us-west-2:000000000000:domain/sim2r-att-0663234a48aa', got 'Data source arn:... is not attached to application sfi9sv1baaaa.'

## Steps to Reproduce
1. CreateDomain (OpenSearch_2.11, one t3.small.search node, gp3/10) and wait for it to be active; keep its ARN.
2. CreateApplication and wait for it to be ACTIVE; keep its `id`.
3. AttachDataSource(id=<appId>, dataSourceArn=<domainArn>, workspaceConfiguration={name, workspaceType: SEARCH}) and wait for the attachment to reach ATTACHED.
4. DetachDataSource(id=<appId>, dataSourceArn=<domainArn>) and wait until it no longer appears in ListDataSourceAttachments.
5. DescribeDataSourceAttachment(id=<appId>, dataSourceArn=<domainArn>) and read the error message: ResourceNotFoundException with the emulator's "Data source <arn> is not attached to application <id>." instead of AWS's "Data source attachment not found for application <id> and data source <arn>".
