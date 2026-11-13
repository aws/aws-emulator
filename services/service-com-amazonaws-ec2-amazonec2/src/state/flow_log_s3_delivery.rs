// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 bucket a flow log publishes to, and the permissions the log delivery
//! service needs on it — written where Amazon S3 OWNS them, in that bucket's own policy.
//!
//! "When publishing to Amazon S3, flow log records for all of the monitored network
//! interfaces are published to a single log file object that is stored in the specified
//! bucket" (`CreateFlowLogs`), and the records are written by the log delivery service
//! rather than by the caller. The permissions that lets it write are not a fact of this
//! service's own store: the bucket belongs to Amazon S3, and so does the policy on it. So a
//! create for an S3 destination writes those permissions into that bucket's policy, instead
//! of reporting a flow log whose delivery nothing could ever authorise.
//!
//! The grant is made of the write of the objects the records land in, under the key prefix
//! the destination ARN names ([`delivery_write_grant`], one statement per delivered
//! location), and the read of the bucket's ACL the delivery checks before it writes
//! ([`delivery_acl_check_grant`], one for the bucket). Both are MERGED into whatever document
//! the bucket already carries: a statement ALLOWING the delivery the same action is the grant
//! this write replaces, keeping the locations it already covers, and every other statement —
//! the bucket owner's enforce-TLS deny, or a refusal of the delivery principal itself — is
//! left exactly where and as it was.
//!
//! Amazon S3 is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]), through the tree's one
//! construction of that client (`service_common::s3_client`), so nothing here opens a
//! socket, leaves the process, or names another service crate.
//!
//! The one caller-supplied value that reaches the client's authority is the Region, which
//! [`super::request_region`] has already narrowed to the Region grammar this service admits,
//! so no request can re-point this client at a host of its own choosing.

/// The service principal that writes a flow log's records into the destination bucket — the
/// identity the bucket's policy grants.
pub const LOG_DELIVERY_SERVICE_PRINCIPAL: &str = "delivery.logs.amazonaws.com";

/// The `Sid` of the statement granting that principal the object write.
///
/// The name of the GRANT, which is why each delivered location's statement carries it: a
/// bucket that grants the delivery two prefixes grants it the same thing twice, once per
/// location.
pub const DELIVERY_WRITE_SID: &str = "AWSLogDeliveryWrite";

/// The `Sid` of the statement granting it the bucket-ACL read.
pub const DELIVERY_ACL_CHECK_SID: &str = "AWSLogDeliveryAclCheck";

/// The action the records are written with.
pub const DELIVERY_WRITE_ACTION: &str = "s3:PutObject";

/// The action the delivery reads the destination bucket's ACL with, before it writes.
pub const DELIVERY_ACL_CHECK_ACTION: &str = "s3:GetBucketAcl";

/// The canned ACL every delivered object has to be written with, so the objects the
/// delivery puts in the bucket belong to the bucket's owner.
pub const BUCKET_OWNER_FULL_CONTROL: &str = "bucket-owner-full-control";

/// The key segment every delivered flow log object sits under: "Note that you can't use
/// `AWSLogs` as a subfolder name" (`CreateFlowLogsRequest$LogDestination`) — the segment the
/// delivery itself writes, after the optional subfolder of the destination ARN.
pub const DELIVERED_LOGS_KEY_SEGMENT: &str = "AWSLogs";

/// The partition key the account segment of a delivered object's key is written as when the
/// flow log asks for Hive-compatible prefixes: "Indicates whether to use Hive-compatible
/// prefixes for flow logs stored in Amazon S3"
/// (`DestinationOptionsRequest$HiveCompatiblePartitions`), which names each prefix segment
/// `<key>=<value>` so a query engine reads it as a partition.
pub const HIVE_ACCOUNT_PARTITION_KEY: &str = "aws-account-id";

/// The version every AWS resource policy declares, as Amazon S3's own bucket-policy
/// examples spell it.
const POLICY_VERSION: &str = "2012-10-17";

/// The effect a GRANT carries, as IAM spells it — the one the delivery's own statements have,
/// and what tells a grant apart from the bucket owner's refusal of the same principal.
const ALLOW_EFFECT: &str = "Allow";

/// The error code Amazon S3 answers a read of the policy of a bucket that carries none with,
/// which is "this bucket has no policy yet" rather than a failure to ask: the grant is then
/// the first statement the document holds.
const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";

/// The Amazon S3 location a flow log's `LogDestination` ARN names: the partition it is in,
/// the bucket the records are delivered to, and the optional subfolder under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowLogS3Destination {
    /// The partition segment of the ARN the request carried, so every ARN the grant names
    /// stays in the partition the caller named.
    pub partition: String,
    /// The bucket the records are published to.
    pub bucket: String,
    /// "The subfolder is optional" (`CreateFlowLogsRequest$LogDestination`), with no leading
    /// or trailing `/`, and empty for an ARN naming the bucket alone.
    pub prefix: String,
}

/// The [`FlowLogS3Destination`] `log_destination` names, or `None` when it is not the ARN of
/// an Amazon S3 location at all.
///
/// "If the destination type is `s3`, specify the ARN of an S3 bucket. For example:
/// `arn:aws:s3:::my_bucket/my_subfolder/`" (`CreateFlowLogsRequest$LogDestination`) — Amazon
/// S3 is a global service, so the Region and account id segments of its ARNs are empty. The
/// bucket is held to the Amazon S3 general purpose bucket naming rules
/// ([`super::s3_bucket_name_is_well_formed`]), because a grant naming a bucket Amazon S3
/// could not hold is a grant to nothing.
#[must_use]
pub fn flow_log_s3_destination(log_destination: &str) -> Option<FlowLogS3Destination> {
    // Six parts: the `arn` literal, the partition, the service, the empty Region, the empty
    // account, and the resource — which is the only part that may itself contain a `:`.
    let mut segments = log_destination.splitn(6, ':');
    (segments.next()? == "arn").then_some(())?;
    let partition = segments.next()?;
    (segments.next()? == "s3").then_some(())?;
    segments.next()?.is_empty().then_some(())?;
    segments.next()?.is_empty().then_some(())?;
    let resource = segments.next()?;

    let partition_is_well_formed = !partition.is_empty()
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !partition_is_well_formed {
        return None;
    }
    let (bucket, prefix) = resource.split_once('/').unwrap_or((resource, ""));
    if !super::s3_bucket_name_is_well_formed(bucket) {
        return None;
    }
    Some(FlowLogS3Destination {
        partition: partition.to_string(),
        bucket: bucket.to_string(),
        prefix: prefix.trim_matches('/').to_string(),
    })
}

/// Write the permissions a flow log's delivery to `log_destination` needs into that
/// destination bucket's own policy, where Amazon S3 keeps it.
///
/// Does nothing at all for a destination that is not the ARN of an Amazon S3 location: there
/// is no bucket to grant on. Call it with NO guard of this service's store held: it awaits.
pub async fn grant_flow_log_delivery(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_destination: &str,
    hive_compatible_partitions: bool,
) {
    let Some(destination) = flow_log_s3_destination(log_destination) else {
        return;
    };
    let client = s3_client(ctx, region);
    let carried = match client
        .get_bucket_policy()
        .bucket(&destination.bucket)
        .send()
        .await
    {
        Ok(answer) => answer
            .policy
            .as_deref()
            .and_then(|text| ::serde_json::from_str::<::serde_json::Value>(text).ok()),
        Err(error) => {
            // A bucket that carries no policy yet is the ordinary case of a first flow log,
            // not a failure: the grants become the document's first statements. Anything
            // else is a bucket this emulator could not ask about, which the put below
            // reports.
            if ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error)
                != Some(NO_SUCH_BUCKET_POLICY_CODE)
            {
                ungranted(&destination.bucket, &error);
            }
            None
        }
    };
    let merged = merged_delivery_grants(
        carried,
        &destination,
        region,
        hive_compatible_partitions,
    );
    let document = match ::serde_json::to_string(&merged) {
        Ok(document) => document,
        Err(error) => {
            ungranted(&destination.bucket, &error);
            return;
        }
    };
    if let Err(error) = client
        .put_bucket_policy()
        .bucket(&destination.bucket)
        .policy(document)
        .send()
        .await
    {
        ungranted(&destination.bucket, &error);
    }
}

/// The policy the destination bucket is to carry: the one it carries now with this flow
/// log's delivery grants merged in.
///
/// ONE write grant per delivered location, so a bucket several flow logs publish to carries
/// the grant of each of their prefixes beside the others, and a location already granted adds
/// nothing however many flow logs deliver there. The grants REPLACE the statements already
/// granting their action to the delivery principal, where the first of those statements sat,
/// and every other statement is left exactly as it was — a document that is not an object
/// holding statements is not one this service can merge into, so the grants stand alone in a
/// fresh document, which is the state a bucket carrying no policy is in.
fn merged_delivery_grants(
    carried: Option<::serde_json::Value>,
    destination: &FlowLogS3Destination,
    region: &str,
    hive_compatible_partitions: bool,
) -> ::serde_json::Value {
    let account = ::engine::emulator_account_id();
    let source_arn = delivery_source_arn(&destination.partition, region, account);
    let mut document = match carried {
        Some(::serde_json::Value::Object(document)) => document,
        _ => ::serde_json::Map::new(),
    };
    let mut statements: Vec<::serde_json::Value> = match document.get("Statement") {
        Some(::serde_json::Value::Array(statements)) => statements.clone(),
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
        _ => Vec::new(),
    };

    // Every location the bucket already grants the delivery, in the order it grants them,
    // then this flow log's — which is already there when another flow log delivers to the
    // same place.
    let mut locations: Vec<String> = Vec::new();
    let mut write_at: Option<usize> = None;
    for (at, statement) in statements.iter().enumerate() {
        if !grants_delivery(statement, DELIVERY_WRITE_ACTION) {
            continue;
        }
        if write_at.is_none() {
            write_at = Some(at);
        }
        for granted in resource_entries(statement) {
            if !locations.contains(&granted) {
                locations.push(granted);
            }
        }
    }
    let delivered = delivered_objects_arn(destination, account, hive_compatible_partitions);
    if !locations.contains(&delivered) {
        locations.push(delivered);
    }
    statements.retain(|statement| !grants_delivery(statement, DELIVERY_WRITE_ACTION));
    // Where the first write grant sat: only write grants were taken out, and none of them
    // sat before that one, so the statements kept before it are still exactly the ones that
    // were before it.
    let at = write_at.unwrap_or(statements.len()).min(statements.len());
    for (offset, location) in locations.iter().enumerate() {
        statements.insert(
            at + offset,
            delivery_write_grant(location, &source_arn, account),
        );
    }

    let acl_check = delivery_acl_check_grant(destination, &source_arn, account);
    // One ACL-check grant whatever the bucket's flow logs are: the read it covers is of the
    // bucket itself, which every one of them makes the same.
    match statements
        .iter()
        .position(|statement| grants_delivery(statement, DELIVERY_ACL_CHECK_ACTION))
    {
        Some(at) => statements[at] = acl_check,
        None => statements.push(acl_check),
    }

    document.insert(
        "Version".to_string(),
        ::serde_json::Value::String(POLICY_VERSION.to_string()),
    );
    document.insert(
        "Statement".to_string(),
        ::serde_json::Value::Array(statements),
    );
    ::serde_json::Value::Object(document)
}

/// The statement granting the delivery the object write over `location`.
///
/// One statement per location the delivery writes to, each naming that location alone, which
/// is how a bucket grants the delivery of several flow logs. The conditions are what bound
/// the grant: the objects have to be written so their content belongs to the bucket's owner,
/// and the delivery has to be acting for THIS account's own logging
/// ([`delivery_source_arn`]).
fn delivery_write_grant(
    location: &str,
    source_arn: &str,
    account: &str,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "Sid": DELIVERY_WRITE_SID,
        "Effect": ALLOW_EFFECT,
        "Principal": { "Service": LOG_DELIVERY_SERVICE_PRINCIPAL },
        "Action": DELIVERY_WRITE_ACTION,
        "Resource": location,
        "Condition": {
            "StringEquals": {
                "s3:x-amz-acl": BUCKET_OWNER_FULL_CONTROL,
                "aws:SourceAccount": account,
            },
            "ArnLike": { "aws:SourceArn": source_arn },
        },
    })
}

/// The statement granting the delivery the read of the destination bucket's ACL, which it
/// makes against the bucket itself rather than against any object of it.
fn delivery_acl_check_grant(
    destination: &FlowLogS3Destination,
    source_arn: &str,
    account: &str,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "Sid": DELIVERY_ACL_CHECK_SID,
        "Effect": ALLOW_EFFECT,
        "Principal": { "Service": LOG_DELIVERY_SERVICE_PRINCIPAL },
        "Action": DELIVERY_ACL_CHECK_ACTION,
        "Resource": bucket_arn(destination),
        "Condition": {
            "StringEquals": { "aws:SourceAccount": account },
            "ArnLike": { "aws:SourceArn": source_arn },
        },
    })
}

/// The ARN of the destination bucket itself.
fn bucket_arn(destination: &FlowLogS3Destination) -> String {
    format!(
        "arn:{partition}:s3:::{bucket}",
        partition = destination.partition,
        bucket = destination.bucket
    )
}

/// The ARN pattern the delivery acts under: the logging of `account` in `region`, which is
/// what scopes the grant to this account's own flow logs rather than to anything the
/// delivery service is asked to write.
fn delivery_source_arn(partition: &str, region: &str, account: &str) -> String {
    format!("arn:{partition}:logs:{region}:{account}:*")
}

/// The ARN of the objects a flow log delivers to `destination`: the bucket, the destination's
/// own subfolder when it named one, the segment the delivery writes under
/// ([`DELIVERED_LOGS_KEY_SEGMENT`]) and the account the records are of.
///
/// With Hive-compatible prefixes the account segment is written as a partition
/// (`aws-account-id=<account>`), which is the one thing that option changes about where an
/// object lands.
fn delivered_objects_arn(
    destination: &FlowLogS3Destination,
    account: &str,
    hive_compatible_partitions: bool,
) -> String {
    let subfolder = if destination.prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/", prefix = destination.prefix)
    };
    let account_segment = if hive_compatible_partitions {
        format!("{HIVE_ACCOUNT_PARTITION_KEY}={account}")
    } else {
        account.to_string()
    };
    format!(
        "{bucket}/{subfolder}{DELIVERED_LOGS_KEY_SEGMENT}/{account_segment}/*",
        bucket = bucket_arn(destination),
    )
}

/// Whether `statement` ALLOWS `action` to the delivery principal — the statement this
/// service's own grant for that action replaces.
///
/// A statement that DENIES the delivery principal is not a grant and is never one of these:
/// it is the bucket owner's own refusal, which the merge leaves exactly where it is rather
/// than taking its place or absorbing what it names.
fn grants_delivery(statement: &::serde_json::Value, action: &str) -> bool {
    let allows = statement.get("Effect").and_then(::serde_json::Value::as_str) == Some(ALLOW_EFFECT);
    let granted = statement
        .get("Principal")
        .and_then(|named| named.get("Service"))
        .is_some_and(|service| names(service, LOG_DELIVERY_SERVICE_PRINCIPAL));
    allows
        && granted
        && statement
            .get("Action")
            .is_some_and(|actions| names(actions, action))
}

/// Whether a policy element written as one string or a list of them names `value`.
fn names(element: &::serde_json::Value, value: &str) -> bool {
    match element {
        ::serde_json::Value::String(named) => named == value,
        ::serde_json::Value::Array(named) => {
            named.iter().any(|named| named.as_str() == Some(value))
        }
        _ => false,
    }
}

/// The locations a statement's `Resource` names, in the order it names them — written as one
/// string or as a list of them.
fn resource_entries(statement: &::serde_json::Value) -> Vec<String> {
    match statement.get("Resource") {
        Some(::serde_json::Value::String(named)) => vec![named.clone()],
        Some(::serde_json::Value::Array(named)) => named
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Amazon S3's client for `region`, over the in-process callback into this same emulator.
///
/// The tree's ONE construction of that client (`service_common::s3_client`), so this crate
/// configures the hop no differently from every other service that reaches the account's
/// object store.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    ::service_common::s3_client::s3_client(ctx.callback(), region)
}

/// Trace a destination bucket whose policy this emulator could not amend, which is a
/// deployment with no object store mounted rather than a bucket reported missing.
///
/// The caller's own value is rendered in its escaped `Debug` form, so a name carrying a
/// newline cannot forge a log line.
fn ungranted<E: ::std::error::Error + 'static>(bucket: &str, error: &E) {
    ::tracing::warn!(
        bucket_name = ?bucket,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "the log delivery grants of a flow log could not be written into the destination \
         bucket's policy"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the ARN-reading table: the case, the destination ARN, and the partition,
    /// bucket and subfolder it names, or `None` when it names no Amazon S3 location.
    type ReadRow = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str, &'static str)>,
    );

    /// The ARNs a `LogDestination` is read as — the bucket alone, a subfolder, a trailing
    /// slash, several segments — and the forms that name no Amazon S3 location at all.
    #[test]
    fn an_s3_destination_is_read_out_of_the_arn_the_request_carried() {
        // (case, the destination ARN, the location it names)
        let read: Vec<ReadRow> = vec![
            (
                "the bucket alone",
                "arn:aws:s3:::flow-logs",
                Some(("aws", "flow-logs", "")),
            ),
            (
                "the subfolder the member documents, with its trailing slash",
                "arn:aws:s3:::flow-logs/vpc/",
                Some(("aws", "flow-logs", "vpc")),
            ),
            (
                "a subfolder of several segments",
                "arn:aws:s3:::flow-logs/vpc/prod",
                Some(("aws", "flow-logs", "vpc/prod")),
            ),
            (
                "another partition",
                "arn:aws-cn:s3:::flow.logs-2026/a",
                Some(("aws-cn", "flow.logs-2026", "a")),
            ),
            ("a CloudWatch Logs group ARN", "arn:aws:logs:us-east-1:000000000000:log-group:g", None),
            ("a bucket name Amazon S3 could not hold", "arn:aws:s3:::A", None),
            ("an ARN carrying a Region", "arn:aws:s3:us-east-1::flow-logs", None),
            ("a bare bucket name", "flow-logs", None),
        ];
        for (case, arn, expected) in read {
            let destination = flow_log_s3_destination(arn);
            let expected = expected.map(|(partition, bucket, prefix)| FlowLogS3Destination {
                partition: partition.to_string(),
                bucket: bucket.to_string(),
                prefix: prefix.to_string(),
            });
            assert_eq!(
                destination, expected,
                "case {case:?}: {arn} must be read as {expected:?}, got {destination:?}"
            );
        }
    }

    /// Where the delivered objects of a flow log sit: under the destination's own subfolder
    /// when it named one, under the segment the delivery writes, and under the account —
    /// written as a Hive partition when the flow log asked for Hive-compatible prefixes.
    #[test]
    fn the_write_grant_covers_the_path_the_records_land_under() {
        let account = ::engine::emulator_account_id();
        // (case, the destination ARN, Hive-compatible prefixes, the granted location)
        let cases: Vec<(&str, &str, bool, String)> = vec![
            (
                "the bucket alone",
                "arn:aws:s3:::flow-logs",
                false,
                format!("arn:aws:s3:::flow-logs/AWSLogs/{account}/*"),
            ),
            (
                "a subfolder",
                "arn:aws:s3:::flow-logs/vpc/",
                false,
                format!("arn:aws:s3:::flow-logs/vpc/AWSLogs/{account}/*"),
            ),
            (
                "the bucket alone, Hive-partitioned",
                "arn:aws:s3:::flow-logs",
                true,
                format!("arn:aws:s3:::flow-logs/AWSLogs/aws-account-id={account}/*"),
            ),
            (
                "a subfolder, Hive-partitioned",
                "arn:aws:s3:::flow-logs/vpc/",
                true,
                format!("arn:aws:s3:::flow-logs/vpc/AWSLogs/aws-account-id={account}/*"),
            ),
        ];
        for (case, arn, hive, expected) in cases {
            let destination =
                flow_log_s3_destination(arn).unwrap_or_else(|| panic!("case {case:?}: {arn}"));
            let granted = delivered_objects_arn(&destination, account, hive);
            assert_eq!(
                granted, expected,
                "case {case:?}: the write grant must cover {expected}, got {granted}"
            );
        }
    }

    /// What the merge leaves the bucket carrying, over the policies a bucket can already
    /// have: none at all, one a client wrote, one REFUSING the delivery principal, one a
    /// previous flow log's grants, and a document that is not a policy this service can read.
    #[test]
    fn the_grants_are_merged_into_whatever_policy_the_bucket_carries() {
        let destination = flow_log_s3_destination("arn:aws:s3:::flow-logs")
            .expect("the bucket ARN names a location");
        let region = "us-east-1";
        let account = ::engine::emulator_account_id();
        let foreign = ::serde_json::json!({
            "Sid": "DenyInsecureTransport",
            "Effect": "Deny",
            "Principal": "*",
            "Action": "s3:*",
            "Resource": "arn:aws:s3:::flow-logs/*",
            "Condition": { "Bool": { "aws:SecureTransport": "false" } },
        });
        // The bucket owner's own refusal of the very principal the grant names, which is not
        // a grant to replace: it stays, and nothing it names is absorbed into one.
        let refused = ::serde_json::json!({
            "Sid": "DenyTheDelivery",
            "Effect": "Deny",
            "Principal": { "Service": LOG_DELIVERY_SERVICE_PRINCIPAL },
            "Action": [DELIVERY_WRITE_ACTION, DELIVERY_ACL_CHECK_ACTION],
            "Resource": "arn:aws:s3:::flow-logs/quarantine/*",
        });
        let already = merged_delivery_grants(None, &destination, region, false);
        // (case, the policy the bucket carries, the statements left beside the grants)
        let cases: Vec<(&str, Option<::serde_json::Value>, Vec<::serde_json::Value>)> = vec![
            ("a bucket carrying no policy at all", None, Vec::new()),
            (
                "a policy of the bucket owner's own",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": [foreign.clone()],
                })),
                vec![foreign.clone()],
            ),
            (
                "a policy carrying one statement rather than a list",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": foreign.clone(),
                })),
                vec![foreign.clone()],
            ),
            (
                "a policy refusing the delivery principal outright",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": [refused.clone()],
                })),
                vec![refused.clone()],
            ),
            (
                "a policy a previous flow log's grants already wrote",
                Some(already.clone()),
                Vec::new(),
            ),
            (
                "a document that is not a policy",
                Some(::serde_json::json!("not a policy")),
                Vec::new(),
            ),
        ];

        for (case, carried, kept) in cases {
            let merged = merged_delivery_grants(carried, &destination, region, false);
            assert_eq!(
                merged["Version"],
                ::serde_json::json!(POLICY_VERSION),
                "case {case:?}: the merged policy must declare the policy version, got {merged}"
            );
            let statements = merged["Statement"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the merge must hold a statement list"))
                .clone();
            for action in [DELIVERY_WRITE_ACTION, DELIVERY_ACL_CHECK_ACTION] {
                let granting: Vec<&::serde_json::Value> = statements
                    .iter()
                    .filter(|statement| grants_delivery(statement, action))
                    .collect();
                assert_eq!(
                    granting.len(),
                    1,
                    "case {case:?}: the bucket must grant {action} exactly once, \
                     got {statements:?}"
                );
            }
            let beside: Vec<::serde_json::Value> = statements
                .iter()
                .filter(|statement| {
                    !grants_delivery(statement, DELIVERY_WRITE_ACTION)
                        && !grants_delivery(statement, DELIVERY_ACL_CHECK_ACTION)
                })
                .cloned()
                .collect();
            assert_eq!(
                beside, kept,
                "case {case:?}: every other statement must be left as it was, \
                 got {beside:?} against {kept:?}"
            );
            assert_eq!(
                granted_locations(&statements),
                vec![delivered_objects_arn(&destination, account, false)],
                "case {case:?}: the delivered path must be granted once, got {merged}"
            );
        }
    }

    /// A second flow log delivering to the same bucket under another prefix is granted BESIDE
    /// the first: each delivered location gets its own write grant, and a location already
    /// granted is granted once.
    #[test]
    fn a_second_flow_log_is_granted_its_own_location_beside_the_first() {
        let account = ::engine::emulator_account_id();
        let region = "us-west-2";
        let plain = flow_log_s3_destination("arn:aws:s3:::flow-logs")
            .expect("the bucket ARN names a location");
        let prefixed = flow_log_s3_destination("arn:aws:s3:::flow-logs/vpc/")
            .expect("the prefixed ARN names a location");

        // (case, the destination the next flow log delivers to, Hive prefixes, the locations
        // the bucket must then grant)
        let steps: Vec<(&str, &FlowLogS3Destination, bool, Vec<String>)> = vec![
            (
                "the first flow log, to the bucket itself",
                &plain,
                false,
                vec![delivered_objects_arn(&plain, account, false)],
            ),
            (
                "a second flow log, Hive-partitioned under a subfolder",
                &prefixed,
                true,
                vec![
                    delivered_objects_arn(&plain, account, false),
                    delivered_objects_arn(&prefixed, account, true),
                ],
            ),
            (
                "the same flow log's destination again",
                &plain,
                false,
                vec![
                    delivered_objects_arn(&plain, account, false),
                    delivered_objects_arn(&prefixed, account, true),
                ],
            ),
        ];

        let mut carried: Option<::serde_json::Value> = None;
        for (case, destination, hive, expected) in steps {
            let merged = merged_delivery_grants(carried, destination, region, hive);
            let statements = merged["Statement"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the merge must hold a statement list"))
                .clone();
            assert_eq!(
                granted_locations(&statements),
                expected,
                "case {case:?}: the bucket must grant {expected:?}, got {merged}"
            );
            assert_eq!(
                statements.len(),
                expected.len() + 1,
                "case {case:?}: the bucket must carry one write grant per location and one \
                 ACL-check grant, got {merged}"
            );
            carried = Some(merged);
        }
    }

    /// The locations `statements` grant the delivery the object write over, in the order they
    /// grant them — one statement per location, each naming the one it covers.
    fn granted_locations(statements: &[::serde_json::Value]) -> Vec<String> {
        statements
            .iter()
            .filter(|statement| grants_delivery(statement, DELIVERY_WRITE_ACTION))
            .flat_map(resource_entries)
            .collect()
    }

    /// Both grants state the delivery principal, the action, the account the records are of
    /// and the logging the delivery acts under — with the Region and the partition of the
    /// request in them.
    #[test]
    fn both_grants_scope_the_delivery_to_this_account() {
        let account = ::engine::emulator_account_id();
        // (case, the destination ARN, the Region the flow log was created in)
        let cases = [
            ("the Region of the request", "arn:aws:s3:::flow-logs", "us-east-1"),
            ("another Region", "arn:aws:s3:::flow-logs/vpc/", "eu-west-1"),
            ("another partition", "arn:aws-cn:s3:::flow-logs", "cn-north-1"),
        ];
        for (case, arn, region) in cases {
            let destination =
                flow_log_s3_destination(arn).unwrap_or_else(|| panic!("case {case:?}: {arn}"));
            let merged = merged_delivery_grants(None, &destination, region, false);
            let statements = merged["Statement"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let source_arn = format!(
                "arn:{partition}:logs:{region}:{account}:*",
                partition = destination.partition,
            );
            for statement in &statements {
                assert_eq!(
                    statement["Effect"],
                    ::serde_json::json!("Allow"),
                    "case {case:?}: a delivery grant must allow, got {statement}"
                );
                assert_eq!(
                    statement["Principal"]["Service"],
                    ::serde_json::json!(LOG_DELIVERY_SERVICE_PRINCIPAL),
                    "case {case:?}: a delivery grant must name the delivery principal, \
                     got {statement}"
                );
                assert_eq!(
                    statement["Condition"]["StringEquals"]["aws:SourceAccount"],
                    ::serde_json::json!(account),
                    "case {case:?}: a delivery grant must scope the source account, \
                     got {statement}"
                );
                assert_eq!(
                    statement["Condition"]["ArnLike"]["aws:SourceArn"],
                    ::serde_json::json!(source_arn),
                    "case {case:?}: a delivery grant must scope the source ARN to \
                     {source_arn}, got {statement}"
                );
            }
            let write = statements
                .iter()
                .find(|statement| grants_delivery(statement, DELIVERY_WRITE_ACTION))
                .unwrap_or_else(|| panic!("case {case:?}: the write grant is there"));
            assert_eq!(
                write["Condition"]["StringEquals"]["s3:x-amz-acl"],
                ::serde_json::json!(BUCKET_OWNER_FULL_CONTROL),
                "case {case:?}: the write grant must require the owner-controlled ACL, \
                 got {write}"
            );
            assert_eq!(
                write["Sid"],
                ::serde_json::json!(DELIVERY_WRITE_SID),
                "case {case:?}: the write grant carries its own statement id, got {write}"
            );
            let acl_check = statements
                .iter()
                .find(|statement| grants_delivery(statement, DELIVERY_ACL_CHECK_ACTION))
                .unwrap_or_else(|| panic!("case {case:?}: the ACL-check grant is there"));
            assert_eq!(
                acl_check["Resource"],
                ::serde_json::json!(bucket_arn(&destination)),
                "case {case:?}: the ACL-check grant must name the bucket itself, \
                 got {acl_check}"
            );
            assert_eq!(
                acl_check["Sid"],
                ::serde_json::json!(DELIVERY_ACL_CHECK_SID),
                "case {case:?}: the ACL-check grant carries its own statement id, \
                 got {acl_check}"
            );
        }
    }
}
