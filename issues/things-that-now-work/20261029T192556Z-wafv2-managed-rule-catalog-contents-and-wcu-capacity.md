Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-catalog-list-available-managed-rule-groups
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-catalog-list-available-managed-rule-groups wafv2-catalog-list-available-managed-rule-groups-cloudfront wafv2-managed-rule-group-and-marketplace-product-discovery wafv2-webacl-full-lifecycle-with-managed-rules wafv2-catalog-describe-all-managed-products wafv2-catalog-describe-all-managed-products-cloudfront
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesACFPRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesATPRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesAdminProtectionRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesAmazonIpReputationList"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesAnonymousIpList"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesAntiDDoSRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesBotControlRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesCommonRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesKnownBadInputsRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesLinuxRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesPHPRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesSQLiRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesUnixRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesWindowsRuleSet"].Description
Issue-Key: wafv2|ListAvailableManagedRuleGroups|2xx->2xx|$.ManagedRuleGroups[Name="AWSManagedRulesWordPressRuleSet"].Description
Issue-Key: wafv2|DescribeAllManagedProducts|2xx->2xx|$.ManagedProducts[*]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.NextMarker
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.0"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.10"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.11"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.12"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.14"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.15"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.16"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.17"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.18"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.19"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.20"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.21"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.22"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.3"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.4"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.6"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.7"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.8"]
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|2xx->2xx|$.Versions[Name="Version_1.9"]
Issue-Key: wafv2|CheckCapacity|2xx->2xx|$.Capacity
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.Capacity
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# wafv2 managed-rule catalog contents and WCU capacity accounting do not match WAF

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAvailableManagedRuleGroups

## Description
The emulator's managed-rule catalog is a hand-written stand-in rather than WAF's published catalog, and the WCU cost it charges for managed rule groups is off. This is ONE issue with FIVE causes; every cause below must be closed before the fix is submitted.

Cause 1 of 5: ListAvailableManagedRuleGroups returns paraphrased Descriptions for the AWS-vendor managed rule groups.
AWS: Smithy: ListAvailableManagedRuleGroups returns ManagedRuleGroupSummary entries carrying Name/VendorName/Description; the Description text is WAF's published catalog text for each group and is fixed, not generated. Recorded behavior: each AWS-vendor group carries its documented sentence, e.g. AWSManagedRulesCommonRuleSet is "Contains rules that are generally applicable to web applications. This provides protection against exploitation of a wide range of vulnerabilities, including those described in OWASP publications." and AWSManagedRulesAdminProtectionRuleSet is "Contains rules that allow you to block external access to exposed admin pages. This may be useful if you are running third-party software or would like to reduce the risk of a malicious actor gaining administrative access to your application."
Emulator: every AWS-vendor group's Description is a shortened paraphrase ("Blocks external access to exposed administrative pages." etc.); 15 groups diverge in one response.
Why: the catalog Description is what a console/CLI surfaces when a user picks a managed rule group, and clients that pin or diff the catalog text (compliance inventories, generated docs, golden-file tests) see every entry differ from AWS.
Fix: the managed-rule-group catalog data behind the ListAvailableManagedRuleGroups handler — carry WAF's published Description per group verbatim.
Traces: wafv2-catalog-list-available-managed-rule-groups#task#1 (cluster also covers the -cloudfront variant, wafv2-managed-rule-group-and-marketplace-product-discovery#task#1 and wafv2-webacl-full-lifecycle-with-managed-rules#task#1).

Cause 2 of 5: DescribeAllManagedProducts returns the AWS managed rule groups instead of the Marketplace managed-product catalog.
AWS: Smithy: DescribeAllManagedProducts returns ManagedProductDescriptor entries with VendorName, ManagedRuleSetName, ProductId, ProductLink, ProductTitle, ProductDescription, SnsTopicArn, IsVersioningSupported and IsAdvancedManagedRuleSet. Recorded behavior: the response is the Marketplace managed-products catalog — third-party vendors (e.g. Cyber Security Cloud Inc., Fortinet, F5) each with a ProductId, a ProductLink to their aws.amazon.com/marketplace listing and the vendor's own SnsTopicArn — 46 entries in the recording.
Emulator: returns AWS-vendor rule-group entries (VendorName "AWS", ProductId equal to the rule-group name, no ProductLink, SnsTopicArn synthesised in the local region/account), i.e. the wrong catalog entirely; all 46 positions diverge.
Why: this operation exists so a client can discover purchasable third-party managed products and their Marketplace links and SNS update topics; with the AWS rule groups substituted, no Marketplace product is discoverable and ProductLink is absent, so any subscribe/upgrade flow has nothing to act on.
Fix: the managed-products catalog data behind the DescribeAllManagedProducts handler — it must be the Marketplace product catalog, distinct from the AWS rule-group catalog of Cause 1.
Traces: wafv2-catalog-describe-all-managed-products#task#1 (cluster also covers the -cloudfront variant).

Cause 3 of 5: ListAvailableManagedRuleGroupVersions publishes the wrong version history and the wrong NextMarker.
AWS: Smithy: ListAvailableManagedRuleGroupVersions output has Versions (ManagedRuleGroupVersions, each with Name and LastUpdateTimestamp), CurrentDefaultVersion and NextMarker (@length 1..256, @pattern \S). Recorded behavior: WAF returns the group's full published version ladder (Version_1.0 ... Version_1.22 in the recording), newest first, each with its own LastUpdateTimestamp, and NextMarker is the key of the last version on the returned page so the next page continues from it.
Emulator: most version entries are absent (Version_1.6 through Version_1.22 and others are missing) and NextMarker names a different version than the last entry it returned.
Why: a client pinning a managed rule group to a specific version enumerates this list; with versions missing, a valid version cannot be pinned and a version-pinned web ACL cannot be reproduced, and a NextMarker that does not follow the returned page makes paging skip or repeat entries. (The per-version LastUpdateTimestamp values themselves are catalog data, not per-run clock values — the requirement is that each published version is present with a timestamp.)
Fix: the managed-rule-group version catalog and the marker computation in the ListAvailableManagedRuleGroupVersions handler.
Traces: wafv2-managed-rule-group-and-marketplace-product-discovery#task#2.

Cause 4 of 5: CheckCapacity under-reports the WCU cost of a rule set containing managed rule groups.
AWS: Smithy: CheckCapacity.Rules is required and output Capacity targets ConsumedCapacity (long, @default 0, @range min 0). The figure is the sum of the WCU cost of each rule in the set, and a ManagedRuleGroupStatement's contribution is the managed rule group's own published capacity. Recorded behavior: for the recording's rule set the total is the sum WAF computes, and the emulator's total is short of it by a fixed amount.
Emulator: returns a smaller Capacity than WAF for the same Rules.
Why: WCU is a hard account limit and the number a client uses to size a rule group or web ACL (CreateRuleGroup.Capacity is immutable once set). An under-reported figure leads the client to provision a rule group that AWS would reject at the limit, and capacity-planning assertions against CheckCapacity pass locally and fail against AWS.
Fix: the per-statement WCU cost table used by the CheckCapacity handler — in particular the cost charged for a ManagedRuleGroupStatement.
Traces: wafv2-webacl-full-lifecycle-with-managed-rules#task#2.

Cause 5 of 5: GetWebACL reports the same under-counted Capacity on a stored web ACL.
AWS: the web ACL's Capacity member is the consumed WCU for its rules, computed by the same accounting as CheckCapacity, and is persisted on the web ACL so later reads report the same figure CheckCapacity gave for the equivalent rule set.
Emulator: WebACL.Capacity is the same short figure as Cause 4, on both the post-create and post-update reads.
Why: a client reads the web ACL's Capacity to decide how much headroom remains before the account WCU limit; the stored value being short means every subsequent sizing decision is wrong, not just the pre-flight check.
Fix: have the web-ACL state compute and persist Capacity through the same corrected WCU accounting as Cause 4 rather than a second code path.
Traces: wafv2-webacl-full-lifecycle-with-managed-rules#task#4 (cluster also covers #task#9).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). All five clusters are verdict=mismatch with expected_status 200 and actual_status 200.

1. ListAvailableManagedRuleGroups wafv2-catalog-list-available-managed-rule-groups#task#1 — divergence_key lists 15 diverging Description keys, one per AWS-vendor group (AWSManagedRulesACFPRuleSet, ATPRuleSet, AdminProtectionRuleSet, AmazonIpReputationList, AnonymousIpList, AntiDDoSRuleSet, BotControlRuleSet, CommonRuleSet, KnownBadInputsRuleSet, LinuxRuleSet, PHPRuleSet, SQLiRuleSet, UnixRuleSet, WindowsRuleSet, WordPressRuleSet). Example: CommonRuleSet expected 'Contains rules that are generally applicable to web applications. This provides protection against exploitation of a wide range of vulnerabilities, including those described in OWASP publications.', got 'Provides protection against exploitation of a wide range of vulnerabilities, including high risk and commonly occurring vulnerabilities described in OWASP publications.' affected_count 4.
2. DescribeAllManagedProducts wafv2-catalog-describe-all-managed-products#task#1 — all of $.ManagedProducts[0..45] diverge. [0] expected the Cyber Security Cloud Inc. entry (ProductId '8bbf295b-34b3-40c0-bee7-5ba8f602705f', ProductLink 'http://aws.amazon.com/marketplace/pp/B081TKTBQ8', SnsTopicArn 'arn:aws:sns:us-east-1:343255486711:CyberSecurityCloud-ManagedRule'), got VendorName 'AWS', ManagedRuleSetName 'AWSManagedRulesCommonRuleSet', ProductId 'AWSManagedRulesCommonRuleSet', no ProductLink, SnsTopicArn 'arn:aws:sns:us-west-2:000000000000:AWSManagedRulesCommonRuleSet'. affected_count 2.
3. ListAvailableManagedRuleGroupVersions wafv2-managed-rule-group-and-marketplace-product-discovery#task#2 — "$.NextMarker: value_mismatch (expected 'Version_1.0', got 'Version_1.1')" plus $.Versions[Name="Version_1.22"] ... [Name="Version_1.6"] all 'missing ... got None' (19 version keys in divergence_key).
4. CheckCapacity wafv2-webacl-full-lifecycle-with-managed-rules#task#2 — "$.Capacity: value_mismatch (expected 712, got 710)".
5. GetWebACL wafv2-webacl-full-lifecycle-with-managed-rules#task#4 — "$.WebACL.Capacity: value_mismatch (expected 712, got 710)"; affected also #task#9. The 712/710 pair is this corpus's rule set; the requirement is that the figure equals WAF's WCU sum for the rules supplied, including each managed rule group's published capacity.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: ListAvailableManagedRuleGroups with Scope=REGIONAL (and CLOUDFRONT) -> compare each AWS-vendor entry's Description against WAF's published catalog text; all 15 are paraphrases.
3. Cause 2: DescribeAllManagedProducts with Scope=REGIONAL (and CLOUDFRONT) -> the response lists AWS-vendor rule groups with no ProductLink instead of the Marketplace managed products with their vendor ProductIds, ProductLinks and SnsTopicArns.
4. Cause 3: ListAvailableManagedRuleGroupVersions for VendorName=AWS, Name=AWSManagedRulesCommonRuleSet -> most published versions are absent and NextMarker does not match the last returned version.
5. Causes 4-5: CheckCapacity with a rule set that includes a ManagedRuleGroupStatement, then CreateWebACL with those rules and GetWebACL -> both report a WCU total short of WAF's sum (710 vs 712 in the corpus), on the post-create and post-update reads alike.

## Resolution

Cause 1 — FIXED, with six catalogue strings DECLINED. The managed-rule-group catalogue now carries, per group, the description the group's PUBLISHER states for its catalogue entry, and `ListAvailableManagedRuleGroups` reports that text verbatim instead of the managed product's short summary; the two modelled members (`ManagedRuleGroupSummary$Description` and `ManagedProductDescriptor$ProductDescription`) are now separate published texts. Nine of the fifteen entries match the recorded text byte for byte. For AWSManagedRulesACFPRuleSet, ATPRuleSet, AnonymousIpList, AntiDDoSRuleSet, PHPRuleSet and UnixRuleSet the publisher's exact wording is DECLINED: it is one fixed reference string per entry that no rule produces and that neither the Smithy model nor the published documentation available here states, so those entries carry the closest published statement instead. Each is a one-line data change in `services/service-com-amazonaws-wafv2-awswaf-20190729/src/state/catalog.rs` should the text become available.

Cause 2 — DECLINED. `DescribeAllManagedProducts` reports the managed products this service publishes, and the AWS-vendor half already follows the model (`ProductLink` is "for Amazon Web Services Marketplace managed rule groups only", so it is absent for an AWS-vendor product, and `SnsTopicArn` is the published us-east-1 notification topic). The missing half is a snapshot of third-party AWS Marketplace seller inventory — each listing's own product identifier, Marketplace listing URL and cross-account SNS topic — which no AWS rule produces and the emulator cannot discover; reproducing the recorded rows would fit the recording rather than implement a rule.

Cause 3 — FIXED. The catalogue publishes the core rule set's whole version ladder with the versions its vendor has retired, and each group's vendor-published default version; `ListAvailableManagedRuleGroupVersions` lists the offered versions newest first with the page's last version as `NextMarker`.

Causes 4 and 5 — FIXED. The WCU accounting both `CheckCapacity` and the stored `WebACL$Capacity` go through prices each statement by its kind, with a `ManagedRuleGroupStatement` contributing the published capacity of the group it runs; both replay keys pass, and a mixed managed-group rule set is pinned in `tests/all/check_capacity.rs`.
