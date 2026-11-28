Issue-Source: trace-sweep-divergence
Issue-Trace: logs-cancel-import-task-import-id-pattern
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-cancel-import-task-import-id-pattern logs-associate-kms-key-resource-identifier-character-class logs-delete-destination-name-pattern logs-put-metric-filter-metric-name-dollar-sign logs-put-metric-filter-unit-enum-bounds logsdimlen-put-metric-filter-dimension-length-bounds logs-query-definition-parameter-name-pattern logs-metric-filter-namespace-reserved-characters logs-put-metric-filter-namespace-length-bounds
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CancelImportTask|4xx->4xx|$.message
Issue-Key: logs|AssociateKmsKey|4xx->4xx|$.message
Issue-Key: logs|AssociateKmsKey|2xx->4xx|$
Issue-Key: logs|AssociateKmsKey|2xx->4xx|status
Issue-Key: logs|DeleteDestination|4xx->4xx|$.message
Issue-Key: logs|DeleteDestination|4xx->4xx|$.__type
Issue-Key: logs|PutMetricFilter|4xx->4xx|$.message
Issue-Key: logs|PutQueryDefinition|4xx->4xx|$.message
Issue-Key: logs|PutMetricFilter|2xx->4xx|$
Issue-Key: logs|PutMetricFilter|2xx->4xx|status
Issue-Key: logs|PutMetricFilter|4xx->2xx|$
Issue-Key: logs|PutMetricFilter|4xx->2xx|status
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs pattern refusals quote Smithy-anchored regexes and enforce stale character classes

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelImportTask

## Description
This issue bundles 9 causes that share ONE fault: the emulator takes the @pattern trait string straight out of the Smithy model and both (a) prints it verbatim — including the ^...$ anchors, which AWS strips from the refusal sentence — and (b) enforces it verbatim, even where the live service's character class differs from the checked-in model. Where the model's class is narrower than the service's, legal requests are refused 400; where it is wider, illegal requests are admitted 200.

Message anatomy — fixed text: `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. Variable parts: the member path from the request and the pattern itself; AWS renders the pattern WITHOUT ^ and $ anchors.

Cause 1 of 9: CancelImportTask.importId. AWS: Smithy ImportId @pattern ^[\-a-zA-Z0-9]+$, reported as `... pattern: [\-a-zA-Z0-9]+`. Emulator: reports `^[\-a-zA-Z0-9]+$`. Why: refusal text differs from AWS for every pattern violation in the service. Fix: the pattern arm of the shared validation-message renderer must strip the anchors when rendering. Traces: logs-cancel-import-task-import-id-pattern#task#1.

Cause 2 of 9: AssociateKmsKey.resourceIdentifier pattern text and class. AWS: reports `... pattern: [\w#+=/:,.@\-\*]*` — unanchored and including `#`, where the checked-in ResourceIdentifier is ^[\w+=/:,.@\-\*]*$ (recorded behavior: the live class is wider). Emulator: reports the anchored, narrower model pattern. Why/Fix: as cause 1, plus the class itself. Traces: logs-associate-kms-key-resource-identifier-character-class#task#3.

Cause 3 of 9: AssociateKmsKey refuses a resourceIdentifier AWS accepts. AWS: 200 with an empty body for an identifier containing `#`. Emulator: 400 InvalidParameterException quoting the narrow pattern. Why: a KMS key cannot be associated with resources whose identifier contains a character AWS allows. Fix: widen the enforced class to the service's. Traces: logs-associate-kms-key-resource-identifier-character-class#task#4.

Cause 4 of 9: DeleteDestination.destinationName pattern. AWS: reports `... pattern: (?!\s*$)[^:*]+` — the live constraint also forbids an all-whitespace name, where the checked-in DestinationName is ^[^:*]*$. Emulator: reports `[^:*]*`. Why/Fix: as cause 1, plus the pattern itself. Traces: logs-delete-destination-name-pattern#task#1 (also task#2).

Cause 5 of 9: DeleteDestination admits a whitespace-only destinationName. AWS: 400 InvalidParameterException with the `(?!\s*$)[^:*]+` pattern message — the request is invalid on its face and never reaches a lookup. Emulator: accepts the name, looks it up, and answers 400 ResourceNotFoundException `The specified destination does not exist.`. Why: a malformed request is reported as a missing resource, so a client retries or creates instead of fixing its input. Fix: apply the whitespace-rejecting pattern in DeleteDestination's input validation, before resolution. Traces: logs-delete-destination-name-pattern#task#4.

Cause 6 of 9: PutMetricFilter.metricTransformations[].metricName pattern. AWS: Smithy MetricName @pattern ^[^:*$]*$, reported as `... pattern: [^:*$]*`. Emulator: reports the anchored form. Why/Fix: as cause 1. Traces: logs-put-metric-filter-metric-name-dollar-sign#task#2 (also logs-put-metric-filter-unit-enum-bounds#task#4, #task#5, logsdimlen-put-metric-filter-dimension-length-bounds#task#2).

Cause 7 of 9: PutQueryDefinition.parameters[].name pattern. AWS: reports `... pattern: ^[a-zA-Z_][a-zA-Z0-9_]*` — a leading anchor only, with no trailing `$`. Emulator: reports `^[a-zA-Z_][a-zA-Z0-9_]*$`. Why/Fix: as cause 1 (the renderer must reproduce the service's pattern text, not re-anchor it). Traces: logs-query-definition-parameter-name-pattern#task#1 (also task#2).

Cause 8 of 9: PutMetricFilter refuses metricNamespace values AWS accepts. AWS: 200 with an empty body for namespaces containing `:`, `$` or `*` (the live constraint on metricNamespace is only `[^:].*`, i.e. it forbids a LEADING colon, not the reserved characters anywhere). Emulator: 400 InvalidParameterException quoting ^[^:*$]*$ from the model's MetricNamespace. Why: metric filters whose namespace contains a colon, dollar or star — which AWS creates — cannot be created at all. Fix: replace the model-derived namespace pattern in PutMetricFilter's validation with the service's leading-character rule. Traces: logs-metric-filter-namespace-reserved-characters#task#3 (also task#4, task#5).

Cause 9 of 9: PutMetricFilter admits a metricNamespace AWS refuses. AWS: 400 InvalidParameterException `1 validation error detected: Value at 'metricTransformations.1.member.metricNamespace' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:].*` for a namespace starting with `:` (and for an empty one). Emulator: 200 with an empty body. Why: the filter is created with a namespace AWS would never store, and metrics published under it are unaddressable. Fix: as cause 8 — the rule is a leading-character rule and must be enforced. Traces: logs-put-metric-filter-namespace-length-bounds#task#4 (also logsdimlen-put-metric-filter-dimension-length-bounds#task#3).

## Evidence
logs-cancel-import-task-import-id-pattern#task#1 (400/400): expected pattern text "[\\-a-zA-Z0-9]+", actual "^[\\-a-zA-Z0-9]+$".
logs-associate-kms-key-resource-identifier-character-class#task#3: expected "[\\w#+=/:,.@\\-\\*]*", actual "^[\\w+=/:,.@\\-\\*]*$"; #task#4: expected 200 {}, actual 400 InvalidParameterException.
logs-delete-destination-name-pattern#task#1: expected "(?!\\s*$)[^:*]+", actual "[^:*]*"; #task#4: expected 400 InvalidParameterException with that pattern, actual 400 ResourceNotFoundException "The specified destination does not exist.".
logs-put-metric-filter-metric-name-dollar-sign#task#2: expected "[^:*$]*", actual "^[^:*$]*$".
logs-query-definition-parameter-name-pattern#task#1: expected "^[a-zA-Z_][a-zA-Z0-9_]*", actual "^[a-zA-Z_][a-zA-Z0-9_]*$".
logs-metric-filter-namespace-reserved-characters#task#3: expected 200 {}, actual 400 with "...metricNamespace ... pattern: ^[^:*$]*$".
logs-put-metric-filter-namespace-length-bounds#task#4: expected 400 InvalidParameterException "... metricNamespace ... pattern: [^:].*", actual 200 empty body.

## Steps to Reproduce
1. Replay logs-cancel-import-task-import-id-pattern#task#1, logs-put-metric-filter-metric-name-dollar-sign#task#2 and logs-query-definition-parameter-name-pattern#task#1: the quoted pattern carries ^/$ anchors AWS does not emit.
2. Replay logs-associate-kms-key-resource-identifier-character-class#task#3 and #task#4: the enforced class omits '#', so a legal identifier is refused 400.
3. Replay logs-delete-destination-name-pattern#task#1 and #task#4: the pattern text differs and an all-whitespace name is answered ResourceNotFoundException instead of the pattern refusal.
4. Replay logs-metric-filter-namespace-reserved-characters#task#3 and logs-put-metric-filter-namespace-length-bounds#task#4: PutMetricFilter refuses namespaces containing ':'/'$'/'*' that AWS accepts, and accepts a leading-colon namespace AWS refuses.
