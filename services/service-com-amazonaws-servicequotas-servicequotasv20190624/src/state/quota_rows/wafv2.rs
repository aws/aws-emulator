// SPDX-License-Identifier: CC0-1.0

//! AWS WAF's published default-quota catalog, under the `wafv2` `ServiceCode`.

use super::*;

/// AWS WAF's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `wafv2` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code wafv2` reports, which is
/// the "AWS WAF quotas" page of the AWS WAF Developer Guide), and the `QuotaCode`s that
/// catalog is keyed on.
///
/// Three rows are published as quotas OF A RESOURCE rather than of the account: the AWS
/// AppSync GraphQL API associations, the API Gateway REST API associations and the custom
/// response bodies of one regional web ACL may be raised for an individual resource as
/// well as account-wide, so each names the level AWS publishes it as adjustable at and the
/// resource type that plane applies to ([`QuotaRow::adjustable_at`]), and reports the
/// RESOURCE scope on every record of it, because the plane it is counted on is the
/// resource whichever record is read ([`QuotaRow::counted_per_resource`]).
///
/// The recorded listing states the quota published under `L-B32E8672` (the API Gateway
/// REST API associations of one regional web ACL, 100, scoped to the web ACL), under
/// `L-D6DA96AE` (the account's regional IP sets, 100, metered by the `AWS/Usage`
/// `ResourceCount` of that plane over the window those counts are published over), under
/// `L-5B2F9B11` (the AWS AppSync GraphQL API associations of one regional web ACL, 100,
/// scoped to the web ACL the same way), under `L-71C2E81B` (the custom response bodies of
/// one regional web ACL or rule group, published as ADJUSTABLE and as a quota of the web
/// ACL, adjustable at the `ALL` level, under the name it states for it), under `L-D95DED7E`
/// (the inspectable request-body size, in kilobytes, of the protections it names, 64, NOT
/// adjustable) and under `L-BF0029B0` (the account's regional REGEX PATTERN SETS, published
/// as adjustable and metered by the `AWS/Usage` `ResourceCount` of that plane's own series
/// over the window those counts are published over). The rule-group capacity, the
/// `GetSampledRequests` rate, the web ACL's capacity units and the retained
/// managed-rule-group versions this catalog used to key on those codes are NOT published
/// here: no source available here states AWS's code for them, so they are left to the
/// default-quota-set gap rather than served under another quota's code. A row whose
/// published `Description` no source available here states carries none, rather than a
/// sentence this emulator words itself.
///
/// The statement nesting depth of one rule, which this catalog used to key on
/// `L-71C2E81B`, is the plane that binding displaced: it is carried by `L-0224FEE0`, whose
/// own published quota no source available here states. The per-web-ACL IP set references,
/// which this catalog used to key on `L-BF0029B0`, are carried by `L-08788A4E` the same way.
/// Neither is a claim that AWS publishes that plane under that code, and each is replaced
/// the moment the quota that code publishes can be sourced.
pub(in crate::state) fn wafv2_quotas() -> Vec<QuotaRow> {
    vec![
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-71C2E81B` binding displaced (see the header).
        quota("Statement nesting depth per rule", 10.0)
            .code("L-0224FEE0")
            .fixed()
            .describe("The maximum nesting depth of the statements of one rule."),
        quota("Custom headers per rule", 5.0)
            .code("L-030B54CC")
            .fixed()
            .describe("The maximum number of custom headers inserted by one rule."),
        quota("Text transformations per rule", 10.0)
            .code("L-041DD6D3")
            .fixed()
            .describe("The maximum number of text transformations in one rule."),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-BF0029B0` binding displaced (see the header).
        quota("IP set references per web ACL", 50.0)
            .code("L-08788A4E")
            .fixed()
            .describe("The maximum number of IP set references in one web ACL."),
        quota("IP addresses per IP set", 10_000.0)
            .code("L-0A8A309C")
            .describe("The maximum number of IP address ranges in one IP set."),
        quota("Regex pattern sets per Region", 10.0)
            .code("L-11D00F38")
            .describe(
                "The maximum number of regex pattern sets in this account in the current \
                 Region.",
            ),
        quota("Regular expressions per regex pattern set", 10.0)
            .code("L-291AF103")
            .fixed()
            .describe("The maximum number of regular expressions in one regex pattern set."),
        quota("Rule groups per Region", 100.0)
            .code("L-2B217866")
            .describe(
                "The maximum number of rule groups in this account in the current Region.",
            ),
        quota("Rule groups per web ACL", 100.0)
            .code("L-2D9CB303")
            .fixed()
            .describe("The maximum number of rule groups referenced by one web ACL."),
        quota("Web ACLs per Region", 100.0)
            .code("L-2EC3DE7B")
            .describe("The maximum number of web ACLs in this account in the current Region."),
        quota("Rate-based rules per web ACL", 10.0)
            .code("L-3D6A3ADF")
            .fixed()
            .describe("The maximum number of rate-based rules in one web ACL."),
        quota("Logging configurations per web ACL", 1.0)
            .code("L-45C90A8A")
            .fixed()
            .describe("The maximum number of logging configurations on one web ACL."),
        // The AWS AppSync GraphQL API associations of ONE regional web ACL, which AWS
        // publishes as adjustable for an individual web ACL as well as for the account,
        // counted on the web ACL.
        quota(
            "Maximum number of AWS AppSync GraphQL API associations per web ACL in WAF for \
             regional",
            100.0,
        )
        .code("L-5B2F9B11")
        .adjustable_at(LEVEL_ALL, "AWS::WAFv2::WebACL")
        .counted_per_resource(),
        quota("Managed rule groups per web ACL", 10.0)
            .code("L-5E8DF1EF")
            .fixed()
            .describe("The maximum number of managed rule groups referenced by one web ACL."),
        quota("Labels per rule", 50.0)
            .code("L-6F32B880")
            .fixed()
            .describe("The maximum number of labels added by one rule."),
        // The custom response bodies of one regional web ACL or rule group, which the
        // recorded listing publishes under this code as ADJUSTABLE and as a quota OF the
        // web ACL — so the row carries that level and that resource type — under the name
        // it states for it. Its limit is the plane's published default and the sentence AWS
        // publishes for it no source available here states, so the row carries none.
        quota(
            "Maximum number of custom response bodies per web ACL or rule group for \
             regional",
            50.0,
        )
        .code("L-71C2E81B")
        .adjustable_at(LEVEL_ALL, "AWS::WAFv2::WebACL")
        .counted_per_resource(),
        quota("Token domains per web ACL", 10.0)
            .code("L-74EBD1DD")
            .fixed()
            .describe("The maximum number of token domains configured on one web ACL."),
        quota("Body inspection size for CloudFront distributions", 65_536.0)
            .code("L-8B48556D")
            .unit("Bytes")
            .describe(
                "The maximum size, in bytes, of the request body AWS WAF inspects for a \
                 CloudFront distribution.",
            ),
        quota("Body inspection size for regional resources", 8192.0)
            .code("L-91DEFBB6")
            .unit("Bytes")
            .describe(
                "The maximum size, in bytes, of the request body AWS WAF inspects for a \
                 regional resource.",
            ),
        quota("Web ACLs per Firewall Manager policy", 1.0)
            .code("L-9E6FF091")
            .fixed()
            .describe("The maximum number of web ACLs created by one Firewall Manager policy."),
        quota("Resources per web ACL", 1000.0)
            .code("L-9FD4FE94")
            .describe("The maximum number of resources associated with one web ACL."),
        quota("CAPTCHA immunity time", 300.0)
            .code("L-A85235F6")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a client is immune from CAPTCHA after solving \
                 one.",
            ),
        quota("Challenge immunity time", 300.0)
            .code("L-B1635397")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a client is immune from a challenge after \
                 passing one.",
            ),
        quota("Characters per regular expression", 512.0)
            .code("L-B24834C1")
            .fixed()
            .describe("The maximum number of characters in one regular expression."),
        // The API Gateway REST API associations of ONE regional web ACL, which AWS
        // publishes as adjustable for an individual web ACL as well as for the account,
        // counted on the web ACL.
        quota(
            "Maximum number of Amazon API Gateway REST API associations per web ACL in WAF \
             for regional",
            100.0,
        )
        .code("L-B32E8672")
        .adjustable_at(LEVEL_ALL, "AWS::WAFv2::WebACL")
        .counted_per_resource(),
        // The account's regional REGEX PATTERN SETS, metered by the `AWS/Usage`
        // `ResourceCount` of that plane: naming the metric also carries the window those
        // counts are published over.
        quota("Maximum regex pattern sets per account in WAF for regional", 10.0)
            .code("L-BF0029B0")
            .metric(usage_metric(
                WAF_USAGE_METRIC_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "RegexPatternSetsPerAccountRegional"),
                    ("Type", "Resource"),
                ],
            )),
        quota("Scope-down statements per managed rule group", 1.0)
            .code("L-C5BCD850")
            .fixed()
            .describe(
                "The maximum number of scope-down statements on one managed rule group \
                 reference.",
            ),
        quota("Tags per resource", 50.0)
            .code("L-C5E4D334")
            .fixed()
            .describe("The maximum number of tags on one AWS WAF resource."),
        quota("Excluded rules per managed rule group", 50.0)
            .code("L-C928281E")
            .fixed()
            .describe(
                "The maximum number of rules excluded from one managed rule group \
                 reference.",
            ),
        quota("Request sampling window", 3.0)
            .code("L-CCCD1D7B")
            .unit("Hours")
            .fixed()
            .describe("The time, in hours, over which sampled requests are available."),
        // The account's regional IP sets, metered by the `AWS/Usage` `ResourceCount` of
        // that plane: naming the metric also carries the window those counts are published
        // over.
        quota("Maximum IP sets per account in WAF for regional", 100.0)
            .code("L-D6DA96AE")
            .metric(usage_metric(
                WAF_USAGE_METRIC_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "IpSetsPerAccountRegional"),
                    ("Type", "Resource"),
                ],
            )),
        // The request body AWS WAF inspects for the protections the quota's own name
        // lists, which the service publishes in kilobytes with the unit in that name.
        quota(
            "Maximum size in kilobytes of a web request body that can be inspected for \
             CloudFront, API Gateway, Amazon Cognito, App Runner, and Verified Access \
             protections",
            64.0,
        )
        .code("L-D95DED7E")
        .fixed(),
        quota("Rules per rule group", 100.0)
            .code("L-D9F31E8A")
            .fixed()
            .describe("The maximum number of rules in one rule group."),
        quota("Web ACLs per CloudFront distribution", 1.0)
            .code("L-E4E414A8")
            .fixed()
            .describe("The maximum number of web ACLs associated with one CloudFront distribution."),
    ]
}

/// The name `AWS/Usage` publishes AWS WAF's RESOURCE counts under.
///
/// SOURCE: the recorded default-quota listing of `wafv2`, whose metered row carries this
/// `Service` dimension. It is the dimension value a `GetMetricData` query has to name for
/// one of these counts, so it is held once here rather than spelled at each row.
const WAF_USAGE_METRIC_SERVICE: &str = "AWS WAF";
