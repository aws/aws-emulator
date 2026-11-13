// SPDX-License-Identifier: CC0-1.0

//! What a SCHEDULED configuration-snapshot delivery DOES: Config writes one snapshot
//! object into the delivery channel's Amazon S3 bucket, holding the configuration items
//! its recorder has captured, and records what became of that write as the channel's
//! last snapshot delivery.
//!
//! `DeliverConfigSnapshot` "schedules delivery of a configuration snapshot to the Amazon
//! S3 bucket in the specified delivery channel" and answers with "the ID of the snapshot
//! that is being created", and it models no error for the delivery itself — so the write
//! is work the clock owes ([`SNAPSHOT_DELIVERY_SETTLE_AFTER`] after the call), and its
//! outcome is reported only through the channel's `configSnapshotDeliveryInfo` and
//! through the object itself. The write is made the way every other cross-service call
//! of this crate is made, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service crate; and it is made as CONFIG, so the bucket's own policy has to allow the
//! Config service principal the write at the moment it happens
//! ([`super::delivered_object_authorization`]). A bucket whose policy withholds that write,
//! or which refuses it when it is made, earns the channel a `FAILURE` status carrying the
//! code and the message of the refusal, and no object.

use std::io::Write;
use std::time::SystemTime;

use super::{
    delivered_object_authorization, insufficient_delivery_policy_message, s3_client,
    service_state, ComAmazonawsConfigserviceStarlingdoveserviceState, DeliveryAuthorization,
    SnapshotDeliveryOutcome, INSUFFICIENT_DELIVERY_POLICY_CODE, SNAPSHOT_DELIVERY_SETTLE_AFTER,
};
use crate::ops::types::ConfigurationItem;

/// The `fileVersion` a delivered configuration snapshot carries, which is the version of
/// the FILE format the snapshot is written in (AWS Config Developer Guide, *Example
/// configuration snapshot*).
const SNAPSHOT_FILE_VERSION: &str = "1.0";

/// The first segment of the key every Amazon Web Services service's delivered log file
/// lives under, and the segment naming the service that delivered this one: a
/// configuration snapshot lands under `AWSLogs/<account-id>/Config/<region>/`.
const DELIVERED_LOG_ROOT: &str = "AWSLogs";
/// See [`DELIVERED_LOG_ROOT`].
const DELIVERED_LOG_SERVICE: &str = "Config";
/// The segment of the key, and the part of the file name, naming WHICH delivery of
/// Config's this object is: a configuration snapshot rather than a configuration history
/// file.
const DELIVERED_SNAPSHOT_KIND: &str = "ConfigSnapshot";
/// What a delivered snapshot object's key ends in: gzip-compressed JSON.
const DELIVERED_SNAPSHOT_SUFFIX: &str = ".json.gz";

/// The error code a delivery that could not be made at all is reported under, when the
/// answer carried none of its own: the delivery was not refused by the bucket, it did not
/// happen.
const DELIVERY_FAILURE_CODE: &str = "InternalError";

/// The message such a delivery is reported with, when the answer carried none.
const DELIVERY_FAILURE_MESSAGE: &str = "The configuration snapshot could not be delivered.";

/// The shortest a delivery waits before it is performed, so a due instant already past
/// still hands the runtime back rather than running the write inline.
const MIN_DELIVERY_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(1);

/// How many deliveries one pump performs before it releases its claim.
///
/// A delivery is owed per `DeliverConfigSnapshot` call, so the loop has no natural end;
/// this cap guarantees termination, and the next call that schedules a delivery starts a
/// fresh pump.
const MAX_DELIVERY_PASSES: usize = 10_000;

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Claim the right to perform this store's owed deliveries, answering `false` when a
    /// pump already holds it — so one store has at most one pump.
    pub fn claim_snapshot_delivery_pump(&mut self) -> bool {
        if self.snapshot_delivery_pump {
            return false;
        }
        self.snapshot_delivery_pump = true;
        true
    }

    /// Give up the claim [`Self::claim_snapshot_delivery_pump`] took.
    pub fn release_snapshot_delivery_pump(&mut self) {
        self.snapshot_delivery_pump = false;
    }

    /// The delivery this store owes the work for next — the channel it was scheduled
    /// through, its snapshot id and the instant that work falls due — earliest due first,
    /// releasing the pump's claim in the same critical section when nothing is owed, so no
    /// delivery can be owed while no pump is running.
    ///
    /// A delivery is owed while it carries no outcome: performing it records one
    /// ([`Self::record_snapshot_delivery_outcome`]), which is what takes it off this list
    /// and makes the pump's loop finite.
    pub fn next_owed_snapshot_delivery_or_release_pump(
        &mut self,
    ) -> Option<(String, String, SystemTime)> {
        let owed = self
            .delivery_channels
            .iter()
            .filter_map(|(name, channel)| {
                let delivery = channel.last_snapshot_delivery.as_ref()?;
                delivery
                    .outcome
                    .is_none()
                    .then(|| (delivery.completes_at, name.clone(), delivery.id.clone()))
            })
            .min();
        match owed {
            Some((due, channel, snapshot_id)) => Some((channel, snapshot_id, due)),
            None => {
                self.release_snapshot_delivery_pump();
                None
            }
        }
    }

    /// Where the delivery `snapshot_id` was scheduled through `channel` has to write its
    /// snapshot object — the channel's bucket and the key prefix it configures — or
    /// `None` when that delivery is no longer the one this channel is waiting on (it was
    /// superseded, the channel went away, the delivery was already performed) or the
    /// channel names no bucket to deliver to.
    pub fn snapshot_delivery_target(
        &self,
        channel: &str,
        snapshot_id: &str,
    ) -> Option<(String, Option<String>)> {
        let channel = self.delivery_channels.get(channel)?;
        let delivery = channel.last_snapshot_delivery.as_ref()?;
        if delivery.id != snapshot_id || delivery.outcome.is_some() {
            return None;
        }
        let bucket = channel.s3_bucket_name.clone()?;
        Some((bucket, channel.s3_key_prefix.clone()))
    }

    /// Whether `channel` is still waiting on the delivery `snapshot_id`: the delivery is
    /// the one it last scheduled and nothing has recorded an outcome for it yet.
    pub fn awaits_snapshot_delivery(&self, channel: &str, snapshot_id: &str) -> bool {
        self.delivery_channels
            .get(channel)
            .and_then(|channel| channel.last_snapshot_delivery.as_ref())
            .is_some_and(|delivery| delivery.id == snapshot_id && delivery.outcome.is_none())
    }

    /// Record what became of the delivery `snapshot_id` scheduled through `channel`.
    ///
    /// Written onto the delivery the channel is waiting on and only that one, so an
    /// outcome can neither revive a superseded delivery nor overwrite one already
    /// recorded.
    pub fn record_snapshot_delivery_outcome(
        &mut self,
        channel: &str,
        snapshot_id: &str,
        outcome: SnapshotDeliveryOutcome,
    ) {
        let Some(delivery) = self
            .delivery_channels
            .get_mut(channel)
            .and_then(|channel| channel.last_snapshot_delivery.as_mut())
            .filter(|delivery| delivery.id == snapshot_id && delivery.outcome.is_none())
        else {
            return;
        };
        delivery.outcome = Some(outcome);
    }

    /// The configuration items a snapshot of this account's recorded state carries: the
    /// CURRENT item of every resource Config's inventory holds, in the Region the
    /// snapshot is delivered for.
    ///
    /// A snapshot is "a complete picture of the resources that are being recorded"
    /// (AWS Config Developer Guide), so it carries one item per recorded resource —
    /// including the final item of one whose deletion was recorded, whose
    /// `configurationItemStatus` says the resource is gone.
    pub fn snapshot_configuration_items(&self, region: &str) -> Vec<ConfigurationItem> {
        self.discovered_resources
            .values()
            .map(|entry| {
                entry.to_configuration_item(
                    region,
                    self.recording_frequency_of(&entry.resource_type),
                )
            })
            .collect()
    }
}

/// Start the in-process pump that performs the snapshot deliveries this store owes, once
/// each falls due.
///
/// Called by `DeliverConfigSnapshot` AFTER it has released the store's lock. It does not
/// change WHEN a delivery happens: the work runs at the due instant the entry holds, so a
/// caller reading the channel's status before that still sees the delivery in flight. At
/// most ONE pump runs per store ([`ComAmazonawsConfigserviceStarlingdoveserviceState::claim_snapshot_delivery_pump`]),
/// it is bounded by [`MAX_DELIVERY_PASSES`] deliveries, and it ends as soon as the store
/// owes none. With no async runtime to spawn onto there is no pump and the delivery is not
/// performed, which is reported as a delivery that has not completed — never as one that
/// succeeded.
pub fn drive_snapshot_delivery<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !state.lock().claim_snapshot_delivery_pump() {
        return;
    }

    let callback = ctx.callback().clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    let pumped = ::engine::ServiceState::clone(&state);
    runtime.spawn(async move {
        // `true` only if the cap below is what ended the loop; a pump that ran out of work
        // has already released its claim inside the one critical section that observed it.
        let mut capped = true;
        for _ in 0..MAX_DELIVERY_PASSES {
            let Some((channel, snapshot_id, due)) =
                pumped.lock().next_owed_snapshot_delivery_or_release_pump()
            else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past, so the cap above can
            // never cost a CPU-bound spin and the runtime is handed back between
            // deliveries.
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or(MIN_DELIVERY_WAIT)
                .clamp(MIN_DELIVERY_WAIT, SNAPSHOT_DELIVERY_SETTLE_AFTER);
            ::tokio::time::sleep(wait).await;
            run_due_snapshot_delivery(
                &callback,
                &*engine,
                &region,
                &channel,
                &snapshot_id,
                SystemTime::now(),
            )
            .await;
        }
        if capped {
            pumped.lock().release_snapshot_delivery_pump();
        }
    });
}

/// Perform the snapshot delivery `snapshot_id` scheduled through `channel`: write the
/// snapshot object into the channel's bucket and record the outcome on the channel.
///
/// No lock is held across the await: the store's guard is taken to decide what to deliver
/// and where, released for the write, and taken again to record what became of it. A
/// delivery the channel is no longer waiting on writes nothing.
pub async fn run_due_snapshot_delivery<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
    channel: &str,
    snapshot_id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let target = {
        let store = state.lock();
        if !store.awaits_snapshot_delivery(channel, snapshot_id) {
            // The channel is waiting on another delivery, or this one was already
            // performed: there is nothing owed and nothing to record.
            return;
        }
        store
            .snapshot_delivery_target(channel, snapshot_id)
            .map(|(bucket, prefix)| {
                (
                    bucket,
                    prefix,
                    // The resource of Config's the delivery is made for, which a bucket
                    // policy that conditions its grant on it is judged against.
                    store.customer_managed_recorder_arn(),
                    store.snapshot_configuration_items(region),
                )
            })
    };
    // A channel naming no bucket has nowhere to deliver to, so the delivery did not
    // happen — which is reported as the failure it is, never left in flight for ever.
    let Some((bucket, prefix, source_arn, items)) = target else {
        state.lock().record_snapshot_delivery_outcome(
            channel,
            snapshot_id,
            SnapshotDeliveryOutcome::Failed {
                at: now,
                code: DELIVERY_FAILURE_CODE.to_string(),
                message: DELIVERY_FAILURE_MESSAGE.to_string(),
            },
        );
        return;
    };
    let key = snapshot_object_key(
        prefix.as_deref(),
        ::emulator_server::emulator_account_id(),
        region,
        now,
        snapshot_id,
    );

    // Config writes a delivered object as its OWN service principal, so the write happens
    // only while the policy in force on the bucket at this moment allows that principal the
    // `s3:PutObject` on the key the object goes to. A policy that withholds it — one taken
    // away after the channel was established as much as one that never granted it — leaves
    // no object in the bucket, and the delivery is reported as the failure it then is, which
    // is the only place that outcome is raised at all.
    if let DeliveryAuthorization::Denied(denied) =
        delivered_object_authorization(callback, region, &bucket, &key, source_arn.as_deref())
            .await
    {
        ::tracing::warn!(
            %bucket,
            %key,
            "the bucket's policy does not allow Config to deliver a configuration snapshot"
        );
        state.lock().record_snapshot_delivery_outcome(
            channel,
            snapshot_id,
            SnapshotDeliveryOutcome::Failed {
                at: now,
                code: INSUFFICIENT_DELIVERY_POLICY_CODE.to_string(),
                message: insufficient_delivery_policy_message(
                    &bucket,
                    denied,
                    prefix.as_deref(),
                    None,
                ),
            },
        );
        return;
    }

    let outcome = match gzip(snapshot_document(snapshot_id, &items).to_string().as_bytes()) {
        None => SnapshotDeliveryOutcome::Failed {
            at: now,
            code: DELIVERY_FAILURE_CODE.to_string(),
            message: DELIVERY_FAILURE_MESSAGE.to_string(),
        },
        Some(body) => {
            let written = s3_client(callback, region)
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(body))
                .send()
                .await;
            match written {
                Ok(_) => SnapshotDeliveryOutcome::Delivered { at: now, key },
                Err(error) => {
                    // The delivery is reported as the bucket answered it: AWS raises the
                    // outcome of a snapshot delivery nowhere else, so the code and the
                    // message a client branches on are the write's own.
                    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
                    ::tracing::warn!(
                        %bucket,
                        %key,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "a configuration snapshot could not be delivered to the channel's bucket"
                    );
                    SnapshotDeliveryOutcome::Failed {
                        at: now,
                        code: error.code().unwrap_or(DELIVERY_FAILURE_CODE).to_string(),
                        message: error
                            .message()
                            .unwrap_or(DELIVERY_FAILURE_MESSAGE)
                            .to_string(),
                    }
                }
            }
        }
    };

    state
        .lock()
        .record_snapshot_delivery_outcome(channel, snapshot_id, outcome);
}

/// The key a delivered configuration snapshot is written under:
/// `AWSLogs/<account-id>/Config/<region>/<yyyy>/<m>/<d>/ConfigSnapshot/<account-id>_Config_<region>_ConfigSnapshot_<stamp>_<id>.json.gz`,
/// under the channel's own `s3KeyPrefix` where it configures one
/// (`DeliveryChannel$s3KeyPrefix`: "The prefix for the specified Amazon S3 bucket").
///
/// Fixed in the form is the `AWSLogs/<account-id>/Config/<region>/` root every delivered
/// Config object lives under, the calendar path that partitions it by delivery day, and
/// the snapshot id in the file name, which is what ties an object to the
/// `configSnapshotId` the scheduling call answered with. The calendar fields are the
/// LIBRARY's ([`utc_fields`]), and the account, Region and id come from the emulator's
/// state and the delivery itself.
fn snapshot_object_key(
    prefix: Option<&str>,
    account_id: &str,
    region: &str,
    at: SystemTime,
    snapshot_id: &str,
) -> String {
    let fields = utc_fields(at);
    let day = format!(
        "{}/{}/{}",
        fields.year,
        trim_zeros(&fields.month),
        trim_zeros(&fields.day),
    );
    let stamp = format!(
        "{}{}{}T{}{}{}Z",
        fields.year, fields.month, fields.day, fields.hour, fields.minute, fields.second,
    );
    let file = format!(
        "{account_id}_{DELIVERED_LOG_SERVICE}_{region}_{DELIVERED_SNAPSHOT_KIND}_{stamp}_\
         {snapshot_id}{DELIVERED_SNAPSHOT_SUFFIX}"
    );
    let root = delivered_root(prefix, account_id, region);
    format!("{root}/{day}/{DELIVERED_SNAPSHOT_KIND}/{file}")
}

/// The key root every delivered Config object of one account and Region lives under:
/// `AWSLogs/<account-id>/Config/<region>`, behind the channel's own `s3KeyPrefix` where it
/// configures one, separated from it by one delimiter however the prefix itself ends.
pub fn delivered_root(prefix: Option<&str>, account_id: &str, region: &str) -> String {
    format!(
        "{}/{region}",
        delivered_account_root(prefix, account_id)
    )
}

/// The key root this account's delivered Config objects and the writability check share:
/// `AWSLogs/<account-id>/Config`, behind the channel's own prefix where it configures one.
///
/// The one place that path is written, so the delivery's own key
/// ([`snapshot_object_key`]) and the key the delivery AUTHORIZATION asks the bucket's
/// policy about ([`delivery_writability_check_key`]) cannot come to disagree about it.
fn delivered_account_root(prefix: Option<&str>, account_id: &str) -> String {
    let root = format!("{DELIVERED_LOG_ROOT}/{account_id}/{DELIVERED_LOG_SERVICE}");
    match prefix.map(str::trim).filter(|prefix| !prefix.is_empty()) {
        // "The prefix for the specified Amazon S3 bucket": the configured prefix stands
        // in front of the root, separated from it by one delimiter however the prefix
        // itself ends.
        Some(prefix) => format!("{}/{root}", prefix.trim_end_matches('/')),
        None => root,
    }
}

/// The name of the object Config writes into a delivery channel's bucket to CHECK that it
/// can deliver there (AWS Config Developer Guide, *Permissions for the Amazon S3 bucket*:
/// Config "creates a file named `ConfigWritabilityCheckFile`" when it validates the
/// bucket).
const DELIVERY_WRITABILITY_CHECK_FILE: &str = "ConfigWritabilityCheckFile";

/// The key of that check object: `[<prefix>/]AWSLogs/<account-id>/Config/` plus its name.
///
/// This is the key the delivery AUTHORIZATION asks the bucket's policy about
/// ([`super::delivery_authorization`]) — the write Config itself performs before it will
/// take a channel — rather than the root the delivered objects sit below, so a policy that
/// grants the objects under that root is judged to allow the write and one that grants
/// nothing there is not.
pub fn delivery_writability_check_key(prefix: Option<&str>, account_id: &str) -> String {
    format!(
        "{}/{DELIVERY_WRITABILITY_CHECK_FILE}",
        delivered_account_root(prefix, account_id)
    )
}

/// `field` without the leading zero a calendar path does not carry: the day segments of a
/// delivered object's key are written as the numbers they are (`2026/1/9`).
fn trim_zeros(field: &str) -> &str {
    let trimmed = field.trim_start_matches('0');
    if trimmed.is_empty() {
        field
    } else {
        trimmed
    }
}

/// The UTC calendar fields of an instant, each zero-padded as a file name carries it.
struct UtcFields {
    year: String,
    month: String,
    day: String,
    hour: String,
    minute: String,
    second: String,
}

/// The UTC calendar fields of `at`.
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)` renders an
/// instant as RFC 3339 in UTC (`YYYY-MM-DDThh:mm:ss[.fff]Z`), and the fields are read off
/// that text rather than computed here. An instant that form cannot state — one outside
/// the four-digit year range — has no calendar fields, and the epoch's stand in, so a key
/// is always a key.
fn utc_fields(at: SystemTime) -> UtcFields {
    let rendered = super::rfc3339_instant(at)
        .or_else(|| super::rfc3339_instant(SystemTime::UNIX_EPOCH))
        .unwrap_or_default();
    let (date, time) = rendered.split_once('T').unwrap_or_default();
    let mut date = date.split('-');
    let mut time = time.split(':');
    UtcFields {
        year: date.next().unwrap_or_default().to_string(),
        month: date.next().unwrap_or_default().to_string(),
        day: date.next().unwrap_or_default().to_string(),
        hour: time.next().unwrap_or_default().to_string(),
        minute: time.next().unwrap_or_default().to_string(),
        // The seconds field carries the zone (and any fractional part) with it.
        second: time
            .next()
            .and_then(|second| second.split(['.', 'Z', 'z', '+']).next())
            .unwrap_or_default()
            .to_string(),
    }
}

/// The document a delivered configuration snapshot holds: the file format's own version,
/// the id of the snapshot, and the configuration items the snapshot captures.
///
/// The three members are the ones a delivered snapshot file carries (AWS Config Developer
/// Guide, *Example configuration snapshot*); the items are the recorded ones
/// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::snapshot_configuration_items`]).
pub fn snapshot_document(snapshot_id: &str, items: &[ConfigurationItem]) -> ::serde_json::Value {
    ::serde_json::json!({
        "fileVersion": SNAPSHOT_FILE_VERSION,
        "configSnapshotId": snapshot_id,
        "configurationItems": items
            .iter()
            .map(delivered_configuration_item)
            .collect::<Vec<::serde_json::Value>>(),
    })
}

/// One configuration item as a DELIVERED snapshot spells it, which is also how the
/// configuration-item change notification a delivery channel streams spells it
/// ([`super::change_notification`]).
///
/// The item is the one the API reports ([`super::configuration_item_document`], which
/// spells every member as the model does), with the four members a delivered file spells
/// differently: `ARN`, `awsAccountId`, `configurationItemVersion` and
/// `configurationStateMd5Hash` (AWS Config Developer Guide, *Example configuration
/// snapshot*). Rendering the file from the API projection is what keeps the two from
/// drifting: a member the item does not carry is absent from both.
pub fn delivered_configuration_item(item: &ConfigurationItem) -> ::serde_json::Value {
    let mut rendered = super::configuration_item_document(item);
    for (api_member, file_member) in [
        ("arn", "ARN"),
        ("accountId", "awsAccountId"),
        ("version", "configurationItemVersion"),
        ("configurationItemMD5Hash", "configurationStateMd5Hash"),
    ] {
        let Some(value) = rendered.get_mut(api_member).map(::serde_json::Value::take) else {
            continue;
        };
        if let Some(document) = rendered.as_object_mut() {
            document.remove(api_member);
            document.insert(file_member.to_string(), value);
        }
    }
    rendered
}

/// `body` as a gzip stream, which is the framing a delivered snapshot object carries, or
/// `None` when it could not be compressed.
fn gzip(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(body).ok()?;
    encoder.finish().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The account and Region every key below is delivered for.
    const ACCOUNT_ID: &str = "000000000000";
    /// See [`ACCOUNT_ID`].
    const REGION: &str = "us-west-2";

    /// A delivered snapshot's key carries the fixed `AWSLogs/<account>/Config/<region>/`
    /// root, the delivery day as a calendar path, and the snapshot id in its file name —
    /// behind the channel's own prefix where it configures one. The rows differ along the
    /// instant delivered at (a padded and an unpadded month and day, the epoch), the
    /// snapshot id, and the configured prefix (none, one, one already ending in the
    /// delimiter).
    #[test]
    fn a_delivered_snapshots_key_names_the_account_the_region_the_day_and_the_snapshot() {
        let cases: [(&str, u64, Option<&str>, &str, &str); 5] = [
            (
                "no prefix, a two-digit month and day",
                1_793_500_000,
                None,
                "s-0000000a",
                "AWSLogs/000000000000/Config/us-west-2/2026/11/1/ConfigSnapshot/\
                 000000000000_Config_us-west-2_ConfigSnapshot_20261101T022640Z_s-0000000a.json.gz",
            ),
            (
                "no prefix, a one-digit month and day",
                1_767_312_000,
                None,
                "s-0000000b",
                "AWSLogs/000000000000/Config/us-west-2/2026/1/2/ConfigSnapshot/\
                 000000000000_Config_us-west-2_ConfigSnapshot_20260102T000000Z_s-0000000b.json.gz",
            ),
            (
                "the epoch",
                0,
                None,
                "s-0000000c",
                "AWSLogs/000000000000/Config/us-west-2/1970/1/1/ConfigSnapshot/\
                 000000000000_Config_us-west-2_ConfigSnapshot_19700101T000000Z_s-0000000c.json.gz",
            ),
            (
                "a configured prefix",
                1_767_312_000,
                Some("audit"),
                "s-0000000d",
                "audit/AWSLogs/000000000000/Config/us-west-2/2026/1/2/ConfigSnapshot/\
                 000000000000_Config_us-west-2_ConfigSnapshot_20260102T000000Z_s-0000000d.json.gz",
            ),
            (
                "a configured prefix ending in the delimiter",
                1_767_312_000,
                Some("audit/"),
                "s-0000000e",
                "audit/AWSLogs/000000000000/Config/us-west-2/2026/1/2/ConfigSnapshot/\
                 000000000000_Config_us-west-2_ConfigSnapshot_20260102T000000Z_s-0000000e.json.gz",
            ),
        ];

        for (case, seconds, prefix, snapshot_id, expected) in cases {
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
            let key = snapshot_object_key(prefix, ACCOUNT_ID, REGION, at, snapshot_id);
            assert_eq!(
                key, expected,
                "case {case:?}: expected the key {expected:?}, got {key:?}"
            );
        }
    }

    /// A delivered snapshot's document carries the file version, the snapshot id and one
    /// entry per configuration item, each spelling the four members a delivered file
    /// spells differently from the API.
    #[test]
    fn a_delivered_snapshot_document_spells_the_items_as_the_file_format_does() {
        let item = ConfigurationItem {
            account_id: Some(ACCOUNT_ID.to_string()),
            arn: Some("arn:aws:s3:::audited-bucket".to_string()),
            version: Some("1.3".to_string()),
            configuration_item_md5_hash: Some("hash".to_string()),
            resource_type: Some("AWS::S3::Bucket".to_string()),
            resource_id: Some("audited-bucket".to_string()),
            aws_region: Some(REGION.to_string()),
            ..ConfigurationItem::default()
        };
        let document = snapshot_document("s-0000000f", &[item]);

        assert_eq!(
            document["fileVersion"],
            ::serde_json::json!(SNAPSHOT_FILE_VERSION),
            "the snapshot names the file format it is written in, got {document}"
        );
        assert_eq!(
            document["configSnapshotId"],
            ::serde_json::json!("s-0000000f"),
            "the snapshot names the id it was delivered for, got {document}"
        );
        let delivered = &document["configurationItems"][0];
        for (member, expected) in [
            ("ARN", "arn:aws:s3:::audited-bucket"),
            ("awsAccountId", ACCOUNT_ID),
            ("configurationItemVersion", "1.3"),
            ("configurationStateMd5Hash", "hash"),
            ("resourceId", "audited-bucket"),
            ("awsRegion", REGION),
        ] {
            assert_eq!(
                delivered[member],
                ::serde_json::json!(expected),
                "case {member:?}: expected {expected:?}, got {delivered}"
            );
        }
        for api_member in ["arn", "accountId", "version", "configurationItemMD5Hash"] {
            assert!(
                delivered.get(api_member).is_none(),
                "case {api_member:?}: a delivered item does not carry the API spelling, \
                 got {delivered}"
            );
        }
    }

    /// The delivered body is gzip-framed, and gunzips back to the document.
    #[test]
    fn the_delivered_body_is_gzip_framed() {
        let document = snapshot_document("s-00000010", &[]);
        let body = gzip(document.to_string().as_bytes())
            .unwrap_or_else(|| panic!("the snapshot body must compress, got none"));
        assert!(
            body.starts_with(&[0x1f, 0x8b]),
            "a delivered snapshot is gzip-framed, got {:?}",
            &body[..2.min(body.len())]
        );
        let mut decoded = Vec::new();
        ::std::io::copy(
            &mut ::flate2::read::GzDecoder::new(::std::io::Cursor::new(&body)),
            &mut decoded,
        )
        .unwrap_or_else(|error| panic!("the delivered body must gunzip, got {error:?}"));
        let parsed: ::serde_json::Value = ::serde_json::from_slice(&decoded)
            .unwrap_or_else(|error| panic!("the gunzipped body must be JSON, got {error:?}"));
        assert_eq!(
            parsed, document,
            "the gunzipped body is the snapshot document, got {parsed}"
        );
    }
}
