Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-controls-aws-config-configrule controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[*]
Issue-Commit: be401828d7c22be926a6d039f45c420752ef7aa4

# controlcatalog ListControls returns an empty Controls page for Config-rule control listings

> _Note: this issue was identified against the code base at commit `be401828d7c22be926a6d039f45c420752ef7aa4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControls

## Description
ONE net-new cause, on service `controlcatalog`.

Cause 1 of 1: ListControls answers HTTP 200 with an empty `Controls` list where AWS returns a page of AWS::Config::ConfigRule control summaries — fixture controlcatalog-catalog-list-controls-aws-config-configrule, step task#5 (this cluster covers 1241 steps of that fixture: task#5, task#6, task#7 ... and onward).

AWS: Smithy ListControls — POST /list-controls, http code 200; input `Filter` (ControlFilter) with `Implementations.Types` (@length 1..1, documented "you can filter for controls implemented as Amazon Web Services Config Rules by specifying AWS::Config::ConfigRule as a type"), `Implementations.Identifiers` (@length 1..1, "filter for controls with specific Amazon Web Services Config Rule IDs"), `GovernedProviders` (@length 1..1), plus `MaxResults` (range 1..100) and `NextToken` (@length ..1024); output `Controls` @required, `NextToken` @length ..1024; @paginated inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults, items=Controls. ControlSummary requires Arn (@length 34..2048, @pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$), Name and Description, and carries Aliases, Behavior, Severity, ParameterRequirementSummary, Implementation {Type, Identifier}, CreateTime, GovernedResources, GovernedProviders. WHICH controls the catalog publishes is recorded behavior only: the recording for the representative step returns a non-empty page whose first row is a DETECTIVE Config-rule control (Implementation {Type AWS::Config::ConfigRule, Identifier ACM_CERTIFICATE_TRANSPARENT_LOGGING_ENABLED}, Severity MEDIUM, GovernedProviders [AWS], GovernedResources [AWS::CertificateManager::Certificate], Aliases [CONFIG.*], ParameterRequirementSummary NONE, a release CreateTime). The general requirement: a listing whose filter names a published implementation type/identifier must return the matching published control rows, not an empty list; an empty page is correct only when nothing published matches.

Emulator: answers HTTP 200 (status itself matches) but the page has no row at index 0 at all — replay reports `$.Controls[0]: missing (expected {...}, got None)` — i.e. the required `Controls` member comes back empty for 1241 recorded steps of this fixture, the single largest failure family in the run (1342 mismatched steps overall).

Why: ListControls is the only discovery path for the catalog, and `Controls` is a @required output. A client that enumerates AWS::Config::ConfigRule controls, or resolves one Config-rule identifier to its control ARN/alias before enabling or reporting on it, sees an empty catalog and concludes no such control exists. Pagination compounds it: with no rows and no NextToken the client stops after the first page, so governance tooling builds an empty inventory, skips enabling controls, and emits compliance reports that silently omit every Config-rule control.

Fix: the seeded control inventory in the control-catalog state that this listing reads (`crate::state`, the same rows GetControl projects) — the published AWS::Config::ConfigRule controls must be present so the handler's filter can match them. The handler itself (services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_controls.rs) already implements type/identifier/provider filtering and pagination over `state.controls`, so Hypothesis: the store holds only a small subset of the published Config-rule controls and every request whose filter selects one outside that subset matches nothing and pages empty. If the subset turns out to be complete, the defect is instead in `matches_filter` / `filter_fingerprint` paging for these filters — either way the fix belongs in the state seeding plus that handler's filter path, and the verification is the same replay.

Related OPEN issues that do NOT cover this cluster (the replay reports it owned_by empty, stub_path empty, so none of them carries its replay key): 20261124T130941Z-controlcatalog-listcontrols-rewrites-published-config-rule-control-text.md (rewritten Name/Description and minted CreateTime on rows that ARE served, steps task#226/250/312/325/469/484/528/552 of this same fixture), 20261127T064732Z-controlcatalog-serves-an-incomplete-and-rewritten-control-catalog.md (GetControl 404s and CreateTime dating, incl. this fixture's task#246), 20261122T143633Z-controlcatalog-publishes-no-resource-control-policy-controls-no.md (a different Implementation.Type). The earlier 20261122T055800Z-controlcatalog-listcontrols-serves-a-divergent-aws-config-configrule issue is RESOLVED and therefore covers nothing here: the empty pages are back for 1241 recorded steps.

Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#5 (cluster members task#6 .. task#29 and onward, 1241 steps in all).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true ("/__admin/health answered 200 and the :41943 data listener accepts"), bootstrap_signal false, 10 fixtures replayed, 1408 steps total (66 passed, 1342 mismatched, 0 errored, 0 blocked), score_pct 4.7, 44 divergence clusters. This is the ONLY cluster in that run with stub_path empty AND owned_by empty; the other 43 are owned by open issues.

Cluster: fixture_id controlcatalog-catalog-list-controls-aws-config-configrule, step_key task#5, service controlcatalog, operation ListControls, verdict mismatch, expected_status 200, actual_status 200, affected_count 1241 (listed members: task#5, task#6, task#7, task#8, task#9, task#10, task#11, task#12, task#13, task#14, task#15, task#16, task#17, task#18, task#19, task#20, task#21, task#22, task#23, task#24, task#25, task#26, task#27, task#28, task#29, ...).

detail: "$.Controls[0]: missing (expected {'Aliases': ['CONFIG.ACM.DT.2'], 'Arn': 'arn:aws:controlcatalog:::control/807si05f8op37hy9qnk5nzylq', 'Behavior': 'DETECTIVE', 'CreateTime': 1779840000.0, 'Description': 'Checks if AWS Certificate Manager certificates have certificate transparency logging enabled. The rule is NON_COMPLIANT if CertificateTransparencyLoggingPreference is explicitly set DISABLED.', 'GovernedProviders': ['AWS'], 'GovernedResources': ['AWS::CertificateManager::Certificate'], 'Implementation': {'Identifier': 'ACM_CERTIFICATE_TRANSPARENT_LOGGING_ENABLED', 'Type': 'AWS::Config::ConfigRule'}, 'Name': 'Checks if AWS Certificate Manager certificates have certificate transparency logging enabled', 'ParameterRequirementSummary': 'NONE', 'Severity': 'MEDIUM'}, got None)"

divergence_key: service=controlcatalog|ListControls|status=200->200|keys=$.Controls[0]

Expected: HTTP 200 with a non-empty Controls page whose rows are the published Config-rule control summaries matching the step's filter. Actual: HTTP 200 with no row at index 0 — an empty Controls list. The specific ARN, identifier and CreateTime above are the representative recording's values filling the template, not the requirement; the requirement is that a filter selecting published controls returns those rows.

Model citations used (services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json): ListControls POST /list-controls, http code 200, errors AccessDeniedException 403 / InternalServerException 500 / ThrottlingException 429 / ValidationException 400; input Filter -> ControlFilter {Implementations -> ImplementationFilter {Types @length 1..1, Identifiers @length 1..1}, GovernedProviders @length 1..1}, MaxResults range 1..100, NextToken @length ..1024; output Controls @required, NextToken @length ..1024; @paginated items=Controls; ControlSummary.Arn/.Name/.Description @required, .Implementation -> ImplementationSummary, .CreateTime -> Timestamp. Emulator code inspected: services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_controls.rs lines 83-147 (MAX_LIST_CONTROLS_RESULTS, control_summary, matches_filter) and 271-294 (filter_fingerprint + state::page over state.controls).

## Steps to Reproduce
1. Clone this checkout, boot the emulator built from it, and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Replay fixture controlcatalog-catalog-list-controls-aws-config-configrule step task#5 — POST /list-controls with that step's recorded Config-rule implementation filter — and compare the response's Controls list against the recording: the recording has a populated page (first row Implementation {Type AWS::Config::ConfigRule, Identifier ACM_CERTIFICATE_TRANSPARENT_LOGGING_ENABLED}), the emulator returns HTTP 200 with Controls empty ($.Controls[0] absent).
3. Repeat for the cluster's other members (task#6 through task#29 and onward, 1241 steps in all), each a different recorded filter/page over the published Config-rule catalog; every one must return its recorded rows.
4. This cause must diff clean before the issue is closed.
