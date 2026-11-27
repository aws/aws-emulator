Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-get-control
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-get-control controlcatalog-catalog-list-common-controls controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Aliases
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Arn
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Behavior
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.CreateTime
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Description
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedProviders
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedResources
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Implementation
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Name
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.ParameterRequirementSummary
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Parameters
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.RegionConfiguration
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Severity
Issue-Key: controlcatalog|GetControl|2xx->4xx|status
Issue-Key: controlcatalog|GetControl|2xx->2xx|$.RegionConfiguration.DeployableRegions[*]
Issue-Key: controlcatalog|GetControl|2xx->2xx|$.Description
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/76akwzuoa1j6cxqfsorr12e9k"].Description
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/76akwzuoa1j6cxqfsorr12e9k"].Name
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/26630f47adf02722c3dd047a5"]
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/4kdrp1fqw2zy6shq95defui2y"]
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/6i6o73iaofk1dk6h4rgm6sccd"]
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/6rxe3eaojbhyha52wtvahsjxn"]
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/7668291c1634052d05daf3a7d"]
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[Arn="arn:aws:controlcatalog:::common-control/e5c3cdc3ea89d1f75be401f47"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/1ks6hwrtiiia8t2djn1iys62x"].Description
Issue-Commit: 9f6915b2a05467415b994281cf9a47869b1e3775

# controlcatalog serves a synthesized control catalog: GetControl 404s on published ARNs, and region lists, names and descriptions diverge (6 causes)

> _Note: this issue was identified against the code base at commit `9f6915b2a05467415b994281cf9a47869b1e3775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
The ControlCatalog read surface answers from an emulator-authored catalog rather than from AWS's published catalog: published control ARNs are not resolvable at all, the per-control deployable-region list is a single shared list, and control / common-control Name and Description text is re-worded. Six net-new causes, all on service `controlcatalog`. (These clusters are reported un-owned by the replay. Related OPEN issues that do NOT cover them: 20261116T095506Z-controlcatalog-common-control-and-domain-catalog-text-is.md, 20261030T021409Z-controlcatalog-listcommoncontrols-returns-an-empty-commoncontrols-list-for-every--r2.md, 20261115T014344Z-controlcatalog-config-rule-controls-are-served-under-emulator-minted.md, 20261122T055800Z-controlcatalog-listcontrols-serves-a-divergent-aws-config-configrule.md, 20261118T190218Z-controlcatalog-listcontrols-rewrites-the-published-description-of-the.md.)

Cause 1 of 6: GetControl answers 404 ResourceNotFoundException for published control ARNs (fixture controlcatalog-catalog-get-control, step task#3; 18 steps in the cluster).
AWS: Smithy: GetControl POST /get-control, http code 200; input ControlArn is @required and @pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$; output Arn, Name, Description, Behavior and RegionConfiguration are @required; ResourceNotFoundException (httpError 404) is documented only as "The requested resource does not exist." A catalogued control ARN therefore resolves with 200 and a full record — the recording shows the 200 body for an AWS Config-implemented control (Behavior DETECTIVE, Implementation {Type AWS::Config::ConfigRule, Identifier EC2_INSTANCE_DETAILED_MONITORING_ENABLED}, Aliases carrying the human-readable designator, Severity MEDIUM, ParameterRequirementSummary NONE, RegionConfiguration.Scope REGIONAL with 34 deployable regions).
Emulator: answers HTTP 404 with no body members at all — every modelled member including the @required Arn/Name/Description/Behavior/RegionConfiguration is absent. The same 404 is returned for 18 distinct published control ARNs in the fixture.
Why: a client that reads a control by the ARN ListControls/Control Tower hands it is told the control does not exist, so any console view, compliance report or enable-control workflow keyed on a catalogued ARN fails outright instead of reading the control.
Fix: the GetControl handler's catalog lookup and the control-catalog state it resolves ARNs against — the published control ARNs are not present under the identifiers AWS publishes.
Traces: controlcatalog-catalog-get-control#task#3 (representative; cluster also covers task#4..task#20).

Cause 2 of 6: GetControl RegionConfiguration.DeployableRegions is not the control's own availability list (fixture controlcatalog-catalog-get-control, step task#1).
AWS: Smithy: GetControl.RegionConfiguration is @required; RegionConfiguration.DeployableRegions is documented "Regions in which the control is available to be deployed", i.e. a per-control list, and the shape doc stresses it is the control's availability and not the caller's governed Regions. Recorded behavior: the 200 body for this control carries 20 deployable regions, including eu-south-1, il-central-1 and me-south-1.
Emulator: answers 200 but with a 17-entry list that diverges from element 9 onward and drops the tail: the recorded eu-south-1, il-central-1 and me-south-1 never appear, every later element is shifted up by one or more positions (index 9 is eu-west-1 where the recording has eu-south-1, index 16 is us-west-2 where the recording has us-east-1), and indexes 17..19 (us-east-2, us-west-1, us-west-2) are missing entirely.
Why: a client enumerating where a control may be deployed is handed a shorter, differently-ordered region set, so it refuses to deploy the control in regions where AWS reports it available.
Fix: the GetControl handler's response projection for RegionConfiguration and the per-control region data in the catalog state (one shared region list is being projected for every control instead of the control's own).
Traces: controlcatalog-catalog-get-control#task#1.

Cause 3 of 6: GetControl Description is re-worded and the region list is again the shared one (fixture controlcatalog-catalog-get-control, step task#2).
AWS: Smithy: GetControl.Description is @required and documented as the control's description "as it may appear in the console" (ControlSummary.Description: "Describes the functionality of the control") — it is the catalog's published text, not a summary. Recorded behavior: Description is "This control checks whether public access is disabled for an Amazon Redshift Serverless workgroup. It evaluates the publiclyAccessible property of a Redshift Serverless workgroup. The control fails if public access is enabled (true) for the workgroup." and DeployableRegions is a 21-entry list beginning ap-east-1 (af-south-1 is NOT in it).
Emulator: answers 200 with the paraphrase "This control checks whether an Amazon Redshift Serverless workgroup is publicly accessible. The control fails if the workgroup is publicly accessible.", losing the sentence that names the evaluated property, and its DeployableRegions begins af-south-1 and is offset by one from the recording for the whole length of the list (index 4 ap-northeast-3 where the recording has ap-southeast-1, index 12 eu-south-1 where the recording has eu-west-1).
Why: console text and compliance documentation generated from the catalog no longer match AWS's published control text, and the region list again reports availability the control does not have (af-south-1) while omitting regions it does.
Fix: the GetControl handler's catalog text and region data in the control-catalog state (descriptions must be stored verbatim as published, and the region list must be per-control).
Traces: controlcatalog-catalog-get-control#task#2.

Cause 4 of 6: ListCommonControls re-words a published common control's Name and Description (fixture controlcatalog-catalog-list-common-controls, step task#16).
AWS: Smithy: CommonControlSummary.Name and .Description are both @required and documented as "The name of the common control" / "The description of the common control" — the catalog's published strings. Recorded behavior: for arn:aws:controlcatalog:::common-control/76akwzuoa1j6cxqfsorr12e9k, Name is "Secure asset management" and Description is "Implement asset management processes to securely track, control, and document all organizational assets. Maintain configuration control of systems undergoing service, repair, or pending return to service."
Emulator: answers 200 but reports Name "Asset use" and Description "Govern the use an asset is put to while it is in service, and the conditions it may be used under." — a different control concept under the published ARN.
Why: a client resolving a common-control ARN to its published name/description (control mappings, framework reporting) renders text for a different control than the one the ARN identifies.
Fix: the common-control catalog data behind ListCommonControls' response projection; the published Name/Description must be stored verbatim per ARN.
Traces: controlcatalog-catalog-list-common-controls#task#16.

Cause 5 of 6: ListCommonControls omits published Asset-management common controls from a filtered listing (fixture controlcatalog-catalog-list-common-controls, step task#37).
AWS: Smithy: ListCommonControls POST /common-controls, output CommonControls @required, paginated on NextToken/MaxResults; every CommonControlSummary carries Arn, Name, Description, Domain, Objective, CreateTime, LastUpdateTime, all @required. Recorded behavior: this filtered page carries the published Asset-management / "Asset loss prevention, response, and recovery" objective rows, among them arn:aws:controlcatalog:::common-control/4kdrp1fqw2zy6shq95defui2y ("Asset loss prevention, recovery, and response"), .../6rxe3eaojbhyha52wtvahsjxn ("Asset damage prevention and response") and .../6i6o73iaofk1dk6h4rgm6sccd (asset-related security incident procedures).
Emulator: answers 200 but each of those ARN-keyed entries is absent from CommonControls (got None). The same diff additionally flags three ARNs of a hex form the catalog never mints (.../26630f47adf02722c3dd047a5, .../7668291c1634052d05daf3a7d, .../e5c3cdc3ea89d1f75be401f47). Hypothesis: emulator-minted rows are being served in place of the published ones, which is why the published ARNs are missing while unpublished ARNs appear.
Why: a client enumerating the common controls of a domain/objective gets an incomplete set under identifiers that cannot be resolved anywhere else in the catalog, so control mappings and coverage reports silently under-report.
Fix: the common-control rows in the catalog state that ListCommonControls' filter walks — the published ARN set must be seeded and no synthesized ARNs emitted.
Traces: controlcatalog-catalog-list-common-controls#task#37.

Cause 6 of 6: ListControls re-words a published Security Hub control Description (fixture controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol, step task#488).
AWS: Smithy: ListControls POST /list-controls, Controls @required; ControlSummary.Description is @required and documented as the description "as it may appear in the console. Describes the functionality of the control." Recorded behavior: for arn:aws:controlcatalog:::control/1ks6hwrtiiia8t2djn1iys62x the Description is "This control checks whether public access is disabled for an Amazon Redshift Serverless workgroup. It evaluates the publiclyAccessible property of a Redshift Serverless workgroup. The control fails if public access is enabled (true) for the workgroup."
Emulator: answers 200 with "This control checks whether an Amazon Redshift Serverless workgroup is publicly accessible. The control fails if the workgroup is publicly accessible." — the same paraphrase as cause 3, so the listing and the get path share one re-worded store.
Why: catalog listings surfaced in consoles and compliance exports carry text that is not AWS's published control description, and the listing disagrees with nothing else to reconcile against.
Fix: the Security Hub control rows in the control-catalog state read by ListControls' response projection (store the published Description verbatim).
Traces: controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol#task#488.

## Evidence
Replay of trace set simparity-wire-aws-v1 (10 fixtures, 1408 steps, 52 passed, 1356 mismatched, score 3.7%; emulator up, bootstrap_signal false). Six net-new divergence clusters, all service=controlcatalog, none stub-blocked, none owned by an open issue:

1. controlcatalog-catalog-get-control#task#3 — GetControl: expected status 200, got 404. All members absent: Arn (expected 'arn:aws:controlcatalog:::control/27lgd3oknluio5kzcl324kr0d'), Aliases (expected ['CONFIG.EC2.DT.1']), Name ('Checks whether detailed monitoring is enabled for EC2 instances'), Description ('Checks whether detailed monitoring is enabled for EC2 instances.'), Behavior ('DETECTIVE'), Severity ('MEDIUM'), CreateTime, Implementation ({'Identifier': 'EC2_INSTANCE_DETAILED_MONITORING_ENABLED', 'Type': 'AWS::Config::ConfigRule'}), ParameterRequirementSummary ('NONE'), Parameters ([]), GovernedProviders (['AWS']), GovernedResources (['AWS::EC2::Instance']), RegionConfiguration ({'Scope': 'REGIONAL', 'DeployableRegions': 34 entries}) — got None. affected_count 18 (task#3..task#20).

2. controlcatalog-catalog-get-control#task#1 — GetControl 200 vs 200, RegionConfiguration.DeployableRegions: [9] expected 'eu-south-1' got 'eu-west-1'; [10] expected 'eu-west-1' got 'eu-west-2'; [11] expected 'eu-west-2' got 'eu-west-3'; [12] expected 'eu-west-3' got 'sa-east-1'; [13] expected 'il-central-1' got 'us-east-1'; [14] expected 'me-south-1' got 'us-east-2'; [15] expected 'sa-east-1' got 'us-west-1'; [16] expected 'us-east-1' got 'us-west-2'; [17] expected 'us-east-2' got None; [18] expected 'us-west-1' got None; [19] expected 'us-west-2' got None.

3. controlcatalog-catalog-get-control#task#2 — GetControl 200 vs 200. Description expected 'This control checks whether public access is disabled for an Amazon Redshift Serverless workgroup. It evaluates the publiclyAccessible property of a Redshift Serverless workgroup. The control fails if public access is enabled (true) for the workgroup.', got 'This control checks whether an Amazon Redshift Serverless workgroup is publicly accessible. The control fails if the workgroup is publicly accessible.' DeployableRegions diverges from [0]: expected 'ap-east-1' got 'af-south-1', ... [4] expected 'ap-southeast-1' got 'ap-northeast-3', [12] expected 'eu-west-1' got 'eu-south-1' (21 expected entries, all offset).

4. controlcatalog-catalog-list-common-controls#task#16 — ListCommonControls 200 vs 200. CommonControls[Arn="arn:aws:controlcatalog:::common-control/76akwzuoa1j6cxqfsorr12e9k"].Name expected 'Secure asset management' got 'Asset use'; .Description expected 'Implement asset management processes to securely track, control, and document all organizational assets. Maintain configuration control of systems undergoing service, repair, or pending return to service.' got 'Govern the use an asset is put to while it is in service, and the conditions it may be used under.'

5. controlcatalog-catalog-list-common-controls#task#37 — ListCommonControls 200 vs 200. CommonControls[Arn=".../4kdrp1fqw2zy6shq95defui2y"] missing (expected the full 'Asset loss prevention, recovery, and response' row in Domain 'Asset management', Objective 'Asset loss prevention, response, and recovery'), CommonControls[Arn=".../6rxe3eaojbhyha52wtvahsjxn"] missing ('Asset damage prevention and response'), CommonControls[Arn=".../6i6o73iaofk1dk6h4rgm6sccd"] missing (asset-related security incident procedures); divergence key additionally lists .../26630f47adf02722c3dd047a5, .../7668291c1634052d05daf3a7d, .../e5c3cdc3ea89d1f75be401f47.

6. controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol#task#488 — ListControls 200 vs 200. Controls[Arn="arn:aws:controlcatalog:::control/1ks6hwrtiiia8t2djn1iys62x"].Description expected 'This control checks whether public access is disabled for an Amazon Redshift Serverless workgroup. It evaluates the publiclyAccessible property of a Redshift Serverless workgroup. The control fails if public access is enabled (true) for the workgroup.', got 'This control checks whether an Amazon Redshift Serverless workgroup is publicly accessible. The control fails if the workgroup is publicly accessible.'

Model citations: Smithy GetControl (POST /get-control, 200; ControlArn @required+@pattern; Arn/Name/Description/Behavior/RegionConfiguration @required; errors ResourceNotFoundException 404, ValidationException 400, AccessDeniedException 403); RegionConfiguration.DeployableRegions doc 'Regions in which the control is available to be deployed'; CommonControlSummary.Name/.Description @required; ControlSummary.Description @required ('as it may appear in the console. Describes the functionality of the control.'); ListCommonControls POST /common-controls; ListControls POST /list-controls.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture controlcatalog-catalog-get-control step task#3 — POST /get-control with the published control ARN; observe HTTP 404 instead of the recorded 200 record. Steps task#4..task#20 of the same fixture reproduce it for 17 further published ARNs.
3. Cause 2: replay controlcatalog-catalog-get-control step task#1 and diff RegionConfiguration.DeployableRegions against the recorded 20-entry list (eu-south-1 / il-central-1 / me-south-1 absent, tail truncated).
4. Cause 3: replay controlcatalog-catalog-get-control step task#2 and diff $.Description and $.RegionConfiguration.DeployableRegions.
5. Cause 4: replay controlcatalog-catalog-list-common-controls step task#16 and diff Name/Description of common-control/76akwzuoa1j6cxqfsorr12e9k.
6. Cause 5: replay controlcatalog-catalog-list-common-controls step task#37 and check CommonControls for the published Asset-management ARNs 4kdrp1fqw2zy6shq95defui2y, 6rxe3eaojbhyha52wtvahsjxn, 6i6o73iaofk1dk6h4rgm6sccd.
7. Cause 6: replay controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol step task#488 and diff the Description of control/1ks6hwrtiiia8t2djn1iys62x.
All six must diff clean before the issue is closed.
