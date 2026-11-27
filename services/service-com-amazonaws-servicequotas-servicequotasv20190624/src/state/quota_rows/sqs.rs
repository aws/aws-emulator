// SPDX-License-Identifier: CC0-1.0

//! Amazon Simple Queue Service's published default-quota catalog.

use super::*;

/// Amazon SQS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sqs` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code sqs` reports, which is
/// the "Amazon SQS quotas" page of the Amazon SQS Developer Guide), and the `QuotaCode`s
/// that catalog is keyed on.
///
/// SQS publishes almost all of its quotas as NOT adjustable: they are properties of the
/// queue the service implements — how large a message may be, how long it may be hidden
/// or kept, how many of them may be in flight — rather than allowances an account may ask
/// to have raised. A row that measures something carries the unit AWS publishes it in
/// (bytes for a size, seconds for a window, days for a retention) rather than being a
/// bare count.
///
/// The recorded listing states the quota published under `L-98134C75` (`Attributes per
/// Message`, 10, with the sentence SQS words it with), under `L-81970B52` (`Message Size
/// in S3 Bucket`, 2 Gigabytes), under `L-BBEFA6CF` (`Queue Policy Size`, 8192 Bytes, NOT
/// adjustable), under `L-BF2A6161` (`UTF-8 Queue Tag Value Length`, 256), under
/// `L-9F628B95` (`Statements per Queue Policy`, 20), under `L-4FF3DAAF` (`Batched Message
/// ID Length`, 80), under `L-B2A3B9D5` (`Message Invisibility Period`, 0 Seconds), under
/// `L-2DA3E3B2` (`Message Retention Time`, in seconds), under `L-A01B4DF0` (`UTF-8 Queue
/// Tag Key Length`, 128), under `L-F115E65A` (`Messages per Batch`, 10), under
/// `L-C491D5A4` (`In-Flight Messages per Standard Queue`, 120000, in `Unit` `None`), under
/// `L-E8A9C91E` (`Queue Name Length`, 80, in `Unit` `None`) and under `L-A7816957` (the
/// delay a delay queue applies, published in MINUTES rather than in seconds). The message
/// size, the two transaction rates, the second in-flight-message plane, the queue tag count
/// and the batch-entry count this catalog used to key on those codes are NOT published
/// here: no source available here states AWS's code for them, so they are left to the
/// default-quota-set gap rather than served under another quota's code. A row whose
/// published `Description` no source available here states carries none, rather than a
/// sentence this emulator words itself.
///
/// THE CODES NO SOURCE BINDS. `L-49DA5CEC` and `L-6A03DCE9` are codes this catalog
/// publishes whose own quota no source available here names, so each carries the plane one
/// of the bindings above displaced — the visibility timeout of a consumed message and the
/// time a receive request waits for one — as the module header says such a row does. That
/// is not a claim that AWS publishes those planes under those codes. The message
/// deduplication-ID length is left with no code of its own and is therefore published
/// under no row.
pub(in crate::state) fn sqs_quotas() -> Vec<QuotaRow> {
    vec![
        // The retention of ONE message. The quota is published in seconds, at the four
        // days a queue retains a message by default (the "Amazon SQS message retention
        // period" of the Amazon SQS Developer Guide), with the fourteen-day ceiling in
        // the quota's own text rather than in its value.
        quota("Message Retention Time", 345_600.0)
            .code("L-2DA3E3B2")
            .unit("Seconds")
            .fixed()
            .describe(
                "The length of time, in seconds, for which Amazon SQS retains a message if \
                 it isn't deleted. The maximum is 14 days (1,209,600 seconds).",
            ),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-E8A9C91E` binding displaced (see the header).
        quota("Visibility Timeout", 43_200.0)
            .code("L-49DA5CEC")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a received message is hidden from the other \
                 consumers of the queue.",
            ),
        quota("Maximum Receives per Redrive Policy", 1000.0)
            .code("L-4BE1B2BD")
            .fixed()
            .describe(
                "The number of times a message is received before a redrive policy moves \
                 it to the dead-letter queue.",
            ),
        quota("Batched Message ID Length", 80.0)
            .code("L-4FF3DAAF")
            .fixed()
            .describe("The length of a batched message ID."),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-C491D5A4` binding displaced (see the header).
        quota("Long Polling Wait Time", 20.0)
            .code("L-6A03DCE9")
            .unit("Seconds")
            .fixed()
            .describe("The time, in seconds, that a receive request waits for a message."),
        quota("Message Group ID Length", 128.0)
            .code("L-79B6240C")
            .fixed()
            .describe("The number of characters in a message group ID."),
        quota("Message Size in S3 Bucket", 2.0)
            .code("L-81970B52")
            .unit("Gigabytes")
            .fixed()
            .describe("The size of a message, in gigabytes, in an Amazon S3 bucket."),
        quota("Attributes per Message", 10.0)
            .code("L-98134C75")
            .fixed()
            .describe("The number of attributes added to a message."),
        quota("Statements per Queue Policy", 20.0)
            .code("L-9F628B95")
            .fixed()
            .describe("The number of statements in a queue policy."),
        quota("UTF-8 Queue Tag Key Length", 128.0)
            .code("L-A01B4DF0")
            .fixed()
            .describe("The length of a UTF-8 queue tag key."),
        // The delay a DELAY QUEUE applies to a message, which the recorded listing
        // publishes under this code in MINUTES — the fifteen minutes a queue may delay a
        // message by — rather than in the seconds the API takes.
        quota("Delay Queue Delay", 15.0)
            .code("L-A7816957")
            .unit("Minutes")
            .fixed(),
        // The window a consumed message stays hidden for, which SQS publishes at the zero
        // a queue applies by default.
        quota("Message Invisibility Period", 0.0)
            .code("L-B2A3B9D5")
            .unit("Seconds")
            .fixed()
            .describe(
                "The length of time, in seconds, for which a message consumed from a queue \
                 remains invisible to other consumers.",
            ),
        quota("Queue Policy Size", 8192.0)
            .code("L-BBEFA6CF")
            .unit("Bytes")
            .fixed()
            .describe("The size, in bytes, of a queue policy."),
        quota("UTF-8 Queue Tag Value Length", 256.0)
            .code("L-BF2A6161")
            .fixed()
            .describe("The length of a UTF-8 queue tag value."),
        // The messages one standard queue may have IN FLIGHT at once, which the recorded
        // listing publishes under this code as a bare count rather than in a unit.
        quota("In-Flight Messages per Standard Queue", 120_000.0)
            .code("L-C491D5A4")
            .fixed(),
        quota("Queue Name Length", 80.0)
            .code("L-E8A9C91E")
            .fixed()
            .describe("The number of characters in a queue name."),
        quota("Messages per Batch", 10.0).code("L-F115E65A").fixed(),
        // The principal plane of a queue policy: a count of the principals the policy
        // document may name, which AWS publishes as fixed.
        quota("Principals per Queue Policy", 50.0)
            .code("L-F61F33C3")
            .fixed()
            .describe("The number of principals in a queue policy."),
    ]
}
