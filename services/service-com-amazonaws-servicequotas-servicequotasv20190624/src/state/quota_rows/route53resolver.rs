// SPDX-License-Identifier: CC0-1.0

//! Amazon Route 53 Resolver's published default-quota catalog.

use super::*;

/// Amazon Route 53 Resolver's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `route53resolver` service code
/// (the "Quotas on Route 53 Resolver" page of the Amazon Route 53 Developer Guide,
/// which publishes the Resolver planes beside the DNS Firewall ones), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota. The
/// rest of the service's published catalog names no quota this catalog can cite under
/// the remaining codes that listing reports, so those rows are left to the
/// default-quota-set gap rather than published under a guessed binding (see
/// [`super::descriptive_quota_code`]).
pub(in crate::state) fn route53resolver_quotas() -> Vec<QuotaRow> {
    vec![
        quota("DNS Firewall rules groups per Region", 1000.0)
            .code("L-02CC8B74")
            .describe("The maximum number of DNS Firewall rules groups per Region."),
        quota("DNS Firewall rule group associations per VPC", 5.0)
            .code("L-15219E1D")
            .fixed()
            .describe(
                "The maximum number of DNS Firewall rule groups that you can associate to a \
                 VPC.",
            ),
        // A plane counted in DOMAIN NAMES rather than in resources: the size of the
        // domain list a file imported from Amazon S3 may carry.
        quota("Domains in a file imported from S3", 250_000.0)
            .code("L-1B2BDF0A")
            .describe(
                "The maximum number of domains that you can import from a single file that's \
                 stored in an Amazon S3 bucket.",
            ),
        // The ENDPOINT plane of a Region, which the recorded listing publishes under
        // this code as adjustable, at its published default, and names in the "Maximum
        // number of …" form this service's own sentences use. Its published sentence is
        // not among the data available here, so it is written in that same form rather
        // than cited.
        quota("Maximum number of resolver endpoints per AWS Region", 4.0)
            .code("L-4A669CC0")
            .describe("Maximum number of resolver endpoints per AWS Region"),
        // The two Resolver planes whose sentence this service publishes in the
        // "Maximum number of …" form, with no closing period: the rules of a Region
        // and the targets of one rule.
        quota("Resolver rules per AWS Region", 1000.0)
            .code("L-51D8A1FB")
            .describe("Maximum number of resolver rules per AWS Region"),
        // The DOMAIN plane of the account, which the recorded listing publishes under
        // this code at its published default. Its sentence is not among the data
        // available here, so it is written in the DNS Firewall voice this service words
        // such a row with rather than cited.
        quota("Domains per account", 100_000.0)
            .code("L-740A4B31")
            .describe("The maximum number of domains per account."),
        // The Route 53 Profiles plane of the service: its name, sentence, adjustability
        // and VALUE are all the recorded listing's.
        quota("Resolver rule associations to a Route 53 Profile.", 1000.0)
            .code("L-8B4B9B75")
            .describe(
                "The maximum number of Resolver rule associations per Route 53 Profile.",
            ),
        // The VPC-association plane of a Region: how many associations between resolver
        // rules and VPCs the Region may hold, which the recorded listing publishes under
        // this code at its published default.
        quota(
            "Associations between resolver rules and VPCs per AWS Region",
            2000.0,
        )
        .code("L-94E19253")
        .describe("Maximum number of associations between resolver rules and VPCs per AWS Region"),
        // The IP-address plane of ONE resolver endpoint, which AWS publishes as
        // adjustable, beside the target plane of a rule below, which it publishes as
        // fixed at the same count.
        quota("IP addresses per resolver endpoint", 6.0)
            .code("L-D2FE9758")
            .describe("Maximum number of IP addresses per resolver endpoint"),
        // The target plane of a rule, which AWS publishes as not adjustable.
        quota("Target IP addresses per resolver rule", 6.0)
            .code("L-D74B6237")
            .fixed()
            .describe("Maximum number of target IP addresses per resolver rule"),
        // The DNS Firewall plane of a Route 53 Profile, which the recorded listing
        // publishes under this code as NOT adjustable and names with the closing stop
        // the Profiles plane above carries. Its published sentence is not among the data
        // available here, so it is written in the voice of that plane's own rather than
        // cited.
        quota(
            "DNS Firewall rule groups associations to a Route 53 Profile.",
            5.0,
        )
        .code("L-F8A07EF1")
        .fixed()
        .describe(
            "The maximum number of DNS Firewall rule group associations per Route 53 \
             Profile.",
        ),
    ]
}
