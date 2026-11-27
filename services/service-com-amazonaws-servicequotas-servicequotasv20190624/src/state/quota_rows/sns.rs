// SPDX-License-Identifier: CC0-1.0

//! Amazon Simple Notification Service's published default-quota catalog.

use super::*;

/// One Amazon SNS per-API request-rate quota: the transactions per second SNS serves
/// calls of that one API at.
///
/// SNS publishes a rate quota for each of its API actions, and every member of such a row
/// but the action's own name and the value is the same across the class: the window is one
/// second, the usage metric is that action's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the service's `AWS/Usage` display name, and the
/// `Description` is the sentence this service words a request-rate quota with
/// ([`sns_transaction_rate_description`]). Those are the quota's published members, so one
/// constructor fills them and two rows of the class cannot drift apart.
///
/// ADJUSTABILITY is not one of them: the recorded listing publishes some of these rates as
/// adjustable and others as fixed, so it is a fact of the row rather than of the class and
/// each row states it ([`QuotaRow::fixed`]).
fn sns_transaction_rate(code: &str, api_name: &str, transactions_per_second: f64) -> QuotaRow {
    quota(
        format!("{api_name} Transactions per Second"),
        transactions_per_second,
    )
    .code(code)
    .period(1, "SECOND")
    .metric(call_count_metric(SNS_USAGE_SERVICE, api_name))
    .describe(&sns_transaction_rate_description(api_name))
}

/// The `Description` Amazon SNS publishes for one of its per-API transaction-rate quotas,
/// with the action it throttles named in it.
///
/// SOURCE: the recorded default-quota listing of `sns`, which carries this exact sentence
/// for the rate quota of its `CreatePlatformApplication` action. SNS words the fact as the
/// number of TIMES AN ACTION MAY BE CALLED per second, which is not how other services
/// word the same fact ([`request_rate_description`], [`api_rate_description`]); it is the
/// wording of the whole SNS class, so one constructor fills it and two rows of the class
/// cannot drift apart.
fn sns_transaction_rate_description(api_name: &str) -> String {
    format!("The maximum number of times the {api_name} action can be called per second.")
}

/// The name `AWS/Usage` publishes Amazon SNS's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const SNS_USAGE_SERVICE: &str = "SNS";

/// Amazon SNS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sns` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code sns` reports, which is
/// the "Amazon Simple Notification Service endpoints and quotas" page of the AWS General
/// Reference together with the "Amazon SNS quotas" page of the SNS Developer Guide), and
/// the `QuotaCode`s that catalog is keyed on.
///
/// The catalog is mostly a RATE catalog: SNS publishes the transactions per second it
/// serves each of its API actions at, which is why all but a handful of rows are built by
/// [`sns_transaction_rate`]. The ones that are not are the resource planes of the account
/// — its topics, their subscriptions, the account's pending subscriptions and the shape of
/// one message — plus the rate SNS publishes for the MESSAGES an account publishes rather
/// than for one API action.
///
/// THE BINDINGS A SOURCE STATES. The recorded listing states which quota is published
/// under `L-4738DDCE` (the `CreatePlatformApplication` rate, 900 per second), under
/// `L-FF07E5EA` (`ConfirmSubscription`, 900 per second), under `L-390AADBE`
/// (`ListSubscriptionsByTopic`, 30 per second and published as NOT adjustable), under
/// `L-B6107771` (`SetSubscriptionAttributes`, 900 per second), under `L-876E1222`
/// (`GetSubscriptionAttributes`, 900 per second), under `L-AB928142` (`CreateTopic`, 900
/// per second), under `L-154ECCA7` (`SetSMSAttributes`, ONE per second and published as
/// not adjustable), under `L-93308085` (`Unsubscribe`, 100 per second and published as not
/// adjustable), under `L-4EE37BC0` (`SetEndpointAttributes`, 900 per second), under
/// `L-B98ECD3E` (`DeleteTopic`), under `L-F8E2BA85` (the account's published-MESSAGE rate,
/// which SNS names and describes as a count of messages per second rather than as a count
/// of `Publish` calls, at 9000 messages per second and metered by the account's
/// `NumberOfMessagesPublishedPerAccount` series) and under `L-1A43D3DB` (the account's
/// PENDING SUBSCRIPTION plane, metered by the service's `AWS/Usage` resource count under
/// its `ApproximateNumberOfPendingSubscriptions` series). Those are the bindings carried
/// here, and every member of each of those rows is that one quota's.
///
/// THE CODES NO SOURCE BINDS. The other codes of this catalog are codes no source
/// available here names the quota of. Each keeps a row, as the module header says every
/// published code does — a code AWS publishes a quota under that resolved nowhere would
/// make a quota that exists in AWS look nonexistent — and such a row carries the NAME, the
/// SENTENCE and the LIMIT of the quota it has carried, or, where a binding above took that
/// quota to its own code, of the quota that binding displaced (`L-039289D5`, `L-21C7FFA6`,
/// `L-6635B4C9`, `L-93CEC191`, `L-A4340BCD`, `L-BCD4AAF3` and `L-BF824DE9` carry the
/// `CreateSMSSandboxPhoneNumber`, `DeleteSMSSandboxPhoneNumber`, `GetEndpointAttributes`,
/// `ListSMSSandboxPhoneNumbers`, `PutDataProtectionPolicy`, `UntagResource` and
/// `VerifySMSSandboxPhoneNumber` rates, each a published SNS action whose own code no
/// source states; `L-4126E74A` and `L-F6A3071A` carry the `ListSubscriptions` and
/// `GetPlatformApplicationAttributes` rates, the planes the `L-93308085` and `L-4EE37BC0`
/// bindings displaced). Such a row is not a claim that AWS publishes that quota under that
/// code, and it is replaced the moment the quota the code publishes can be sourced.
///
/// What IS each of those codes' own published fact is its ADJUSTABILITY, which the
/// recorded listing reports per code — it publishes some of this service's rates as fixed
/// and others as taking an increase — so every RATE row below carries the adjustability
/// recorded for ITS code rather than one the rate class would imply. A counted PLANE keeps
/// the adjustability AWS publishes for that plane, which is a sourced fact of the quota
/// itself: a message's shape is fixed, the account's topic and subscription planes are not.
pub(in crate::state) fn sns_quotas() -> Vec<QuotaRow> {
    vec![
        sns_transaction_rate("L-01EACE54", "AddPermission", 30.0).fixed(),
        sns_transaction_rate("L-038DA0E0", "CheckIfPhoneNumberIsOptedOut", 30.0),
        sns_transaction_rate("L-039289D5", "CreateSMSSandboxPhoneNumber", 30.0).fixed(),
        // The SMS attributes of the account are set at ONE call per second, which the
        // recorded listing publishes under this code as a fixed quota.
        sns_transaction_rate("L-154ECCA7", "SetSMSAttributes", 1.0).fixed(),
        sns_transaction_rate("L-19F66BE5", "CreatePlatformEndpoint", 15.0).fixed(),
        // The account's PENDING SUBSCRIPTION plane: a counted plane rather than a rate,
        // so CloudWatch publishes its usage as the service's `AWS/Usage` RESOURCE count,
        // under the series the recorded listing names for it.
        quota("Pending Subscriptions per Account", 5000.0)
            .code("L-1A43D3DB")
            .metric(usage_metric(
                SNS_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "ApproximateNumberOfPendingSubscriptions"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of pending subscriptions per AWS account, across all \
                 regions.",
            ),
        sns_transaction_rate("L-21C7FFA6", "DeleteSMSSandboxPhoneNumber", 30.0).fixed(),
        sns_transaction_rate("L-2EE2434F", "DeleteEndpoint", 15.0).fixed(),
        sns_transaction_rate("L-2FE42A3E", "DeletePlatformApplication", 15.0),
        // The rates the recorded listing names, each under the code THAT listing publishes
        // it under; `ListSubscriptionsByTopic` is published as NOT adjustable.
        sns_transaction_rate("L-390AADBE", "ListSubscriptionsByTopic", 30.0).fixed(),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-93308085` binding displaced, at this code's own recorded
        // adjustability (see the header).
        sns_transaction_rate("L-4126E74A", "ListSubscriptions", 30.0),
        quota("FIFO Topics per Account", 1000.0)
            .code("L-41B81227")
            .describe(
                "The maximum number of FIFO topics in this account in the current Region.",
            ),
        sns_transaction_rate("L-4659BFC2", "GetDataProtectionPolicy", 30.0).fixed(),
        sns_transaction_rate("L-4738DDCE", "CreatePlatformApplication", 900.0),
        sns_transaction_rate("L-4EE37BC0", "SetEndpointAttributes", 900.0),
        sns_transaction_rate("L-61103206", "GetSMSAttributes", 30.0),
        sns_transaction_rate("L-6521CF3A", "GetSMSSandboxAccountStatus", 30.0).fixed(),
        sns_transaction_rate("L-6635B4C9", "GetEndpointAttributes", 15.0).fixed(),
        sns_transaction_rate("L-6A04D1EB", "GetTopicAttributes", 30.0).fixed(),
        sns_transaction_rate("L-7B7E3FB8", "ListEndpointsByPlatformApplication", 30.0).fixed(),
        sns_transaction_rate("L-7E62C088", "ListOriginationNumbers", 30.0).fixed(),
        sns_transaction_rate("L-820458B0", "ListPhoneNumbersOptedOut", 30.0),
        sns_transaction_rate("L-856966E8", "ListPlatformApplications", 15.0).fixed(),
        sns_transaction_rate("L-876E1222", "GetSubscriptionAttributes", 900.0),
        sns_transaction_rate("L-93308085", "Unsubscribe", 100.0).fixed(),
        sns_transaction_rate("L-93CEC191", "ListSMSSandboxPhoneNumbers", 30.0).fixed(),
        sns_transaction_rate("L-9A72FF33", "ListTagsForResource", 100.0).fixed(),
        sns_transaction_rate("L-9D5EC8F7", "ListTopics", 30.0),
        // The shape of ONE message: the attributes it may carry and the bytes it may hold.
        quota("Message Attributes per Message", 10.0)
            .code("L-A05852B7")
            .fixed()
            .describe("The number of message attributes in a message."),
        quota("Message Size", 262_144.0)
            .code("L-A228E0EB")
            .unit("Bytes")
            .fixed()
            .describe("The size, in bytes, of a published message."),
        sns_transaction_rate("L-A30AD1BE", "OptInPhoneNumber", 30.0).fixed(),
        sns_transaction_rate("L-A4340BCD", "PutDataProtectionPolicy", 30.0).fixed(),
        sns_transaction_rate("L-AB928142", "CreateTopic", 900.0),
        sns_transaction_rate("L-B3509C67", "PublishBatch", 30_000.0).fixed(),
        sns_transaction_rate("L-B6107771", "SetSubscriptionAttributes", 900.0),
        sns_transaction_rate("L-B96EDA7D", "RemovePermission", 30.0),
        sns_transaction_rate("L-B98ECD3E", "DeleteTopic", 30.0),
        sns_transaction_rate("L-BBD4D2FF", "SetPlatformApplicationAttributes", 15.0).fixed(),
        sns_transaction_rate("L-BCD4AAF3", "UntagResource", 100.0),
        sns_transaction_rate("L-BF824DE9", "VerifySMSSandboxPhoneNumber", 30.0).fixed(),
        sns_transaction_rate("L-C1E19420", "SetTopicAttributes", 30.0).fixed(),
        sns_transaction_rate("L-CC5551AD", "Subscribe", 100.0).fixed(),
        quota("Subscriptions per Topic", 12_500_000.0)
            .code("L-E1E48E53")
            .describe("The maximum number of subscriptions on one topic."),
        sns_transaction_rate("L-F35E8445", "TagResource", 100.0),
        quota("Topics per Account", 100_000.0)
            .code("L-F514C636")
            .describe("The maximum number of topics in this account in the current Region."),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-4EE37BC0` binding displaced, at this code's own recorded
        // adjustability (see the header).
        sns_transaction_rate("L-F6A3071A", "GetPlatformApplicationAttributes", 15.0).fixed(),
        // The account's PUBLISHED-MESSAGE rate, the one rate of this catalog SNS publishes
        // as a count of MESSAGES over the second rather than as a count of one action's
        // calls: the recorded listing names and describes it that way, publishes it over
        // the one-second window of the rate class, and meters it with an `AWS/Usage`
        // metric that is NOT a call count of an API — a resource-usage series summed over
        // the window. The listing names the series it is read from — the account's
        // published-message count — so the metric carries that `Resource` dimension.
        quota("Messages Published per Second", 9000.0)
            .code("L-F8E2BA85")
            .period(1, "SECOND")
            .metric(aws_usage_metric(
                "ResourceCount",
                "Sum",
                SNS_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "NumberOfMessagesPublishedPerAccount"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of messages published per second."),
        sns_transaction_rate("L-FF07E5EA", "ConfirmSubscription", 900.0),
    ]
}
