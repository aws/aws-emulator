Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-catalog-describe-all-managed-products
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-catalog-describe-all-managed-products wafv2-catalog-describe-all-managed-products-cloudfront
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|DescribeAllManagedProducts|2xx->2xx|$.ManagedProducts[*]
Issue-Commit: 5581abbb4b2483f03b471efb65d90d44a0779aaf

# wafv2 DescribeAllManagedProducts serves a synthesized managed-product catalog instead of AWS's published entries

> _Note: this issue was identified against the code base at commit `5581abbb4b2483f03b471efb65d90d44a0779aaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAllManagedProducts

## Description
Cause 1 of 1: DescribeAllManagedProducts answers HTTP 200 with a ManagedProducts list whose entries from index 2 onward do not match the catalog AWS publishes. The response status and the envelope are right; the reference DATA in it is not.

AWS: Smithy: DescribeAllManagedProducts(Scope) -> DescribeAllManagedProductsResponse.ManagedProducts : ManagedProductDescriptors, a list of ManagedProductDescriptor whose members are VendorName, ManagedRuleSetName, ProductId (@length 1..128, pattern \S), ProductLink (Marketplace listings only), ProductTitle, ProductDescription ("A short description of the managed rule group", @length min 1, pattern \S), SnsTopicArn ("The Amazon resource name (ARN) of the Amazon Simple Notification Service SNS topic that's used to provide notification of changes to the managed rule group", @length 20..2048), IsVersioningSupported and IsAdvancedManagedRuleSet. The model says nothing about WHICH products are published, so for the catalog content this is recorded behavior only: the recording shows one descriptor per published product, each carrying the publishing vendor's own text and identifiers — a ProductDescription on EVERY entry (e.g. for ProductId 'prod-qqlwxtijbjsm2' / ManagedRuleSetName 'CyberSecurityCloud-ProtocolEnforcement-', and for ProductId 'cdd10222-b7e0-466d-a710-6a35cbab6f9a' / 'all_rules' of vendor 'Fortinet'), and a SnsTopicArn naming the PUBLISHING VENDOR's account (the recording carries vendor-owned topics such as arn:aws:sns:us-east-1:343255486711:CyberSecurityCloud-ManagedRule and arn:aws:sns:us-east-1:040422370703:Fortinet_OWASP_Top_10_Notifications). Those account numbers, product ids and description texts are published catalog reference data, fixed per product and identical in every account and region — they are not per-run values.

Emulator: the replayed response diverges on $.ManagedProducts[2] through $.ManagedProducts[45] (44 of the 46 entries; [0] and [1] match). For the two entries the excerpt shows in full, the emulator's descriptor is byte-identical to the recording EXCEPT that the ProductDescription member is absent altogether: the Marketplace catalogue entry stores the summary as an Option and leaves it uncited for those two listings, and the descriptor builder maps None to an omitted member, so a modeled member AWS always populates is simply not serialized. Hypothesis for the remaining divergent entries ([4]..[45], the AWS Managed Rules family): those descriptors are built with a ProductId DERIVED locally (a UUID hashed from the (VendorName, ManagedRuleSetName) pair) rather than the identifier the publisher states, and with a SnsTopicArn minted through the emulator's own ARN builder in the EMULATOR's account rather than the AWS Managed Rules publishing account — both are synthesized values that cannot equal the published ones, which is consistent with every entry of that family being reported divergent while the two fully-cited Marketplace listings pass.

Why: DescribeAllManagedProducts (and DescribeManagedProductsByVendor, which filters the same list) is the catalog a client reads to choose a managed rule group and to wire up change notifications. A client that renders the product picker gets blank descriptions for some products; a client that subscribes to SnsTopicArn to learn of rule-group changes subscribes to a topic in the wrong account that no vendor publishes to, so it never receives a notification; a client that round-trips ProductId into another call keys on an identifier AWS does not recognize. Because the call returns 200, none of this surfaces as an error — the data is silently wrong.

Fix: the published managed-product catalog data and the descriptor builders that render it — services/service-com-amazonaws-wafv2-awswaf-20190729/src/state/managed_products.rs (MarketplaceManagedProduct::product, MARKETPLACE_MANAGED_PRODUCT_CATALOGUE, managed_product_id, vendor_notification_topic_arn) and services/service-com-amazonaws-wafv2-awswaf-20190729/src/state/catalog.rs (ManagedRuleGroupDescriptor::product, ManagedRuleGroupDescriptor::sns_topic_arn). The operation handler itself (src/ops/describe_all_managed_products.rs) is correct: it validates Scope and returns the catalog unfiltered. Every descriptor member must be the publisher's published value, never derived from the group's name or minted in the emulator's own account, and no entry may omit ProductDescription.

Traces: wafv2-catalog-describe-all-managed-products#task#1 (representative) and wafv2-catalog-describe-all-managed-products-cloudfront#task#1 — the same divergence in both Scopes, so it is not scope-dependent. No existing open wafv2 issue covers this operation; the nearest open wafv2 catalog-ish issues (20261025T231222Z-wafv2-getrevenuestatistics-stub.md, 20261127T073307Z-service-quotas-...-wafv2-default-quota-rows...) are about other operations and other data.

## Evidence
Replay cluster (verdict mismatch, expected_status 200, actual_status 200), divergence_key 'service=wafv2|DescribeAllManagedProducts|status=200->200|keys=$.ManagedProducts[2]..$.ManagedProducts[45]' — 44 divergent list elements, affected_count 2 fixtures.

$.ManagedProducts[2]: expected {'IsAdvancedManagedRuleSet': False, 'IsVersioningSupported': True, 'ManagedRuleSetName': 'CyberSecurityCloud-ProtocolEnforcement-', 'ProductDescription': 'This ruleset helps protect your website by detecting requests with HTTP protocol and encoding issues. By using it in conjunction with the HighSecurity OWASP ruleset, you can expect to further increase the security of your website.', 'ProductId': 'prod-qqlwxtijbjsm2', 'ProductLink': 'https://aws.amazon.com/marketplace/pp/prodview-g2dp6u5k5w5jo', 'ProductTitle': 'Cyber Security Cloud Managed Rules for AWS WAF -Protocol Enforcement by WafCharm-', 'SnsTopicArn': 'arn:aws:sns:us-east-1:343255486711:CyberSecurityCloud-ManagedRule', 'VendorName': 'Cyber Security Cloud Inc.'}; got the same descriptor with NO 'ProductDescription' member at all.

$.ManagedProducts[3]: expected the Fortinet 'all_rules' descriptor including 'ProductDescription': 'Based on the FortiWeb web application firewall signatures and updated on a regular basis to include the latest threat information from FortiGuard Labs, the ruleset provides a comprehensive package to help address threats as described in OWASP Top 10'; got the same descriptor with no 'ProductDescription' member.

Code confirming the omission: src/state/managed_products.rs lines 244-260 store product_description: None for the 'CyberSecurityCloud-ProtocolEnforcement-' and Fortinet 'all_rules' listings, and line 168 renders it as product_description: self.product_description.map(str::to_string) — i.e. the member is elided when uncited. src/state/catalog.rs line 195 builds ProductId as managed_product_id(vendor, name), which falls back to product_id_of_pair() (a SHA-256-derived UUID) for the AWS Managed Rules family, and line 221 builds SnsTopicArn as managed_product_topic_arn(AWS_MANAGED_RULE_NOTIFICATION_TOPIC) -> engine::emulator_arn("sns", ...), i.e. in the emulator's own account rather than the publishing vendor's.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture wafv2-catalog-describe-all-managed-products, step task#1 (and wafv2-catalog-describe-all-managed-products-cloudfront, step task#1).
2. The step calls wafv2 DescribeAllManagedProducts with Scope=REGIONAL (CLOUDFRONT in the second fixture).
3. Compare the ManagedProducts list element-wise against the recording: the response is HTTP 200, elements [0] and [1] match, and elements [2] through [45] differ.
4. On elements [2] and [3] the only difference is the missing ProductDescription member; on elements [4]..[45] (the AWS Managed Rules family) compare ProductId and SnsTopicArn against the recorded published values.
