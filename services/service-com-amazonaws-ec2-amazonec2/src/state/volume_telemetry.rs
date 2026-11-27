// SPDX-License-Identifier: CC0-1.0

//! The `AWS/EBS` series one VOLUME publishes about the I/O performed on it: the metric families,
//! what one I/O operation is, and where the datapoints are stored.
//!
//! # What the series are
//!
//! Amazon EBS publishes each measurement of one volume as its own `AWS/EBS` metric, dimensioned on
//! exactly that volume ([`VOLUME_ID_DIMENSION`]) and measured in that metric's own unit
//! ([`VOLUME_METRICS`] is the catalog, with the unit of each entry). Those series are what a
//! per-volume dashboard plots and what every volume alarm is evaluated against, and they are
//! dimensioned on the VOLUME and not on the instance — which is what makes them follow the volume
//! when it is detached from one instance and attached to another.
//!
//! Only the measurements of real I/O are published here. `VolumeReadOps`/`VolumeWriteOps` are "the
//! total number of read/write operations in a specified period of time" and
//! `VolumeReadBytes`/`VolumeWriteBytes` "the total number of bytes transferred", so each of them is
//! a measurement of operations something performed ([`super::guest_disk_io`], the one place a guest
//! performs any) — in a period in which nothing read or wrote the volume there is nothing of it to
//! measure, which is why no datapoint is published for one. The queue, idle-time and burst-balance
//! metrics measure quantities nothing here observes, and so are claimed by nothing.
//!
//! `VolumeConsumedReadWriteOps` is published for a volume of a PROVISIONED IOPS type only — "this
//! metric is only available for Provisioned IOPS SSD volumes"
//! ([`super::volumes::PROVISIONED_IOPS_VOLUME_TYPES`] is the one reading of which types those are)
//! — so a volume of any other type carries that series not at all, however much I/O it took.
//!
//! # When a datapoint happens
//!
//! An I/O operation is a thing that ALREADY HAPPENED: the guest performed it, through the device
//! the volume is attached as, at the instant it performed it. So the datapoint carries that instant
//! ([`VolumeIoRecord::at_secs`]) and is published once — the record is taken as it is published
//! ([`due_volume_samples`]), the way the agent's collections are
//! ([`super::guest_cloudwatch_agent::due_samples`]) — rather than being recomputed per period by a
//! sampler. A reader taking a statistic of a period therefore sees exactly the operations performed
//! in it: the `Sum` of `VolumeWriteBytes` over a period is the bytes written in it.
//!
//! # ... and when one happens for a period nothing moved in
//!
//! "Data is only reported to CloudWatch when the volume is attached to an instance" (Amazon EBS
//! User Guide, "Amazon EBS CloudWatch metrics"): an ATTACHED volume is a volume Amazon EBS is
//! measuring, and it reports what it measured for every period of its own cadence — one minute for
//! the SSD volume types and five for the others ([`PUBLICATION_PERIODS`]) — so a period in which
//! nothing moved carries a reading of ZERO rather than no reading at all. That is what makes the
//! series of an attached volume tell an IDLE volume from one nothing is measuring, and what makes
//! the series STOP at the detach: a volume that is attached to nothing is reported for no period at
//! all ([`due_volume_samples`]), and publication starts again, from the period it is attached in,
//! when something attaches it.
//!
//! Only COMPLETE periods are reported that way — the period a reading would measure has to be over
//! before there is anything to report of it — and only the periods the volume was attached for,
//! which is why publication for a volume this store has not reported yet starts at the period the
//! first report falls in rather than at the volume's own creation.
//!
//! Nothing is published for a volume that is not attached, so a volume detached from every instance
//! is not a device of any guest, no I/O can happen on it at all, and no period of it is reported —
//! which is what makes the series stop when the volume is detached.

use ::std::collections::{BTreeMap, BTreeSet};
use ::std::time::{Instant, SystemTime};

use super::guest_cloudwatch_agent::AgentSample;
use super::guest_disk_io::VolumeIo;
use super::volumes::PROVISIONED_IOPS_VOLUME_TYPES;
use super::{ComAmazonawsEc2Amazonec2State, VolumeRecord};

/// The namespace Amazon EBS publishes a volume's metrics under.
pub const NAMESPACE: &str = "AWS/EBS";

/// The dimension a volume's series is dimensioned on: "`VolumeId`: This dimension filters the data
/// you request for the specified volume" (Amazon EBS User Guide, "Amazon EBS metrics").
pub const VOLUME_ID_DIMENSION: &str = "VolumeId";

/// How many bytes ONE I/O operation moves at most.
///
/// Amazon EBS counts the operations a volume performed by this size: "I/O operations that are
/// smaller than 256 KiB each count as 1 consumed IOPS" and one larger "that is 1,024 KiB … consumes
/// 4 IOPS" (`VolumeConsumedReadWriteOps`, Amazon EBS User Guide) — so a transfer of `n` bytes is
/// the operations it takes to move them at this size each.
pub const IO_OPERATION_BYTES: i64 = 256 * 1024;

/// How many I/O OPERATIONS a transfer of `bytes` is, counted as Amazon EBS counts them
/// ([`IO_OPERATION_BYTES`]): a transfer of nothing is no operation, and anything up to one
/// operation's size is one.
#[must_use]
pub fn operations_for(bytes: i64) -> i64 {
    if bytes <= 0 {
        return 0;
    }
    bytes.saturating_add(IO_OPERATION_BYTES - 1) / IO_OPERATION_BYTES
}

/// One metric of the `AWS/EBS` families: its name, the unit it is measured in, and what it measures
/// of one transfer.
#[derive(Debug, Clone, Copy)]
pub struct VolumeMetric {
    /// The `MetricName` the series is published under.
    pub name: &'static str,
    /// The `StandardUnit` its datapoints carry.
    pub unit: &'static str,
    /// Whether it is published for a volume of a provisioned-IOPS type ALONE.
    pub provisioned_iops_only: bool,
    /// What it MEASURES of one transfer, read off the transfer itself — so the catalog and the
    /// measurement cannot drift apart.
    pub measures: fn(&VolumeIo) -> i64,
}

/// The metrics a volume publishes about the I/O performed on it, with the unit of each and what
/// each one measures.
///
/// The catalog and every unit in it come from the metrics Amazon EBS documents for one VOLUME
/// (Amazon EBS User Guide, "Amazon EBS metrics"); they are documented metrics rather than Smithy
/// shapes, so there is no model member to read them from.
pub const VOLUME_METRICS: &[VolumeMetric] = &[
    VolumeMetric {
        name: "VolumeReadOps",
        unit: super::instance_telemetry::COUNT_UNIT,
        provisioned_iops_only: false,
        measures: |performed| performed.read_ops,
    },
    VolumeMetric {
        name: "VolumeWriteOps",
        unit: super::instance_telemetry::COUNT_UNIT,
        provisioned_iops_only: false,
        measures: |performed| performed.write_ops,
    },
    VolumeMetric {
        name: "VolumeReadBytes",
        unit: super::instance_telemetry::BYTES_UNIT,
        provisioned_iops_only: false,
        measures: |performed| performed.read_bytes,
    },
    VolumeMetric {
        name: "VolumeWriteBytes",
        unit: super::instance_telemetry::BYTES_UNIT,
        provisioned_iops_only: false,
        measures: |performed| performed.write_bytes,
    },
    VolumeMetric {
        name: "VolumeConsumedReadWriteOps",
        unit: super::instance_telemetry::COUNT_UNIT,
        provisioned_iops_only: true,
        // "the total amount of read and write operations" the volume consumed.
        measures: |performed| performed.read_ops.saturating_add(performed.write_ops),
    },
];

/// How often Amazon EBS reports an attached volume's series, per volume TYPE: "For … volumes,
/// metrics are reported to CloudWatch every 1 minute" for the SSD types, and every 5 minutes for
/// the others (Amazon EBS User Guide, "Amazon EBS CloudWatch metrics").
///
/// The cadence of the ZERO readings an attached volume publishes for the periods nothing moved in
/// (see the module docs); the datapoints of I/O something performed carry the instant it was
/// performed at and are not on this grid at all.
pub const PUBLICATION_PERIODS: &[(&[&str], i64)] = &[
    (&["gp2", "gp3", "io1", "io2"], 60),
    (&["st1", "sc1", "standard"], 300),
];

/// The period Amazon EBS reports a volume of `volume_type` at ([`PUBLICATION_PERIODS`]), and the
/// one-minute cadence for a type this emulator knows no figure for — the cadence of every type AWS
/// publishes a figure of one minute for, and the one a volume created with no type at all gets,
/// its default type being an SSD one.
#[must_use]
pub fn publication_period(volume_type: Option<&str>) -> i64 {
    let named = volume_type.unwrap_or_default();
    PUBLICATION_PERIODS
        .iter()
        .find(|(types, _)| types.contains(&named))
        .map_or(60, |(_, period)| *period)
}

/// How many of those periods ONE publication reports at most.
///
/// A store that nothing has read for an hour owes every period of it; this is the bound on what one
/// publication materialises for one volume, so the datapoints of a long-idle account arrive over
/// the next few publications rather than all at once. Well above the periods between two
/// publications of a store something is using, the metric clock publishing far more often than
/// once a period ([`super::instance_telemetry::drive_instance_metric_clock`]).
pub const MAX_PERIODS_PER_PUBLICATION: i64 = 10;

/// How many I/O records ONE volume keeps waiting to be published.///
/// The records are published off the next request this service serves and off the metric clock, so
/// one waits for the length of a request at most; this bounds what a guest performing I/O without
/// anything reading the account can make this process hold, by dropping the OLDEST, the way a
/// fixed-size buffer does.
pub const MAX_PENDING_IO_RECORDS: usize = 256;

/// One transfer the guest performed on a volume, as the volume holds it until it is published: what
/// was moved, and WHEN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeIoRecord {
    /// The operations and the bytes they moved ([`VolumeIo`]).
    pub performed: VolumeIo,
    /// The instant they were performed at, as epoch seconds — the time stamp the datapoints carry.
    pub at_secs: i64,
}

impl VolumeRecord {
    /// Record that the guest performed `performed` on this volume at `at`.
    ///
    /// The one writer of a volume's pending I/O, so every path a guest can perform I/O by leaves
    /// one kind of record behind.
    pub fn record_guest_io(&mut self, performed: VolumeIo, at: SystemTime) {
        if !performed.happened() {
            return;
        }
        self.guest_io.push(VolumeIoRecord {
            performed,
            at_secs: epoch_secs(at),
        });
        while self.guest_io.len() > MAX_PENDING_IO_RECORDS {
            self.guest_io.remove(0);
        }
    }
}

/// The datapoints the account's volumes owe at `now`, taking the I/O records they carry as
/// published.
///
/// Each record is one transfer that happened, so it becomes one datapoint per metric it is a
/// measurement of — and nothing at all for the measurements it is not: a transfer that only read
/// publishes no write series, and a volume of a type Amazon EBS publishes no consumed-IOPS series
/// for carries none. No instant decides those: each datapoint carries the instant its own I/O was
/// performed at ([`VolumeIoRecord::at_secs`]).
///
/// `now` decides the OTHER half: the periods of an ATTACHED volume that are over and carry no I/O
/// at all, each of which is a reading of zero (see the module docs). A volume attached to nothing
/// owes none of those, which is what makes its series stop at the detach.
pub fn due_volume_samples(
    store: &mut ComAmazonawsEc2Amazonec2State,
    now: SystemTime,
) -> Vec<AgentSample> {
    let mut samples: Vec<AgentSample> = Vec::new();
    let at_secs = epoch_secs(now);
    // The clock the attachments are read against, which is the monotonic one they are held on.
    let reading = Instant::now();
    for (volume_id, record) in &mut store.volumes {
        let provisioned_iops = record
            .view
            .volume_type
            .as_deref()
            .is_some_and(|volume_type| PROVISIONED_IOPS_VOLUME_TYPES.contains(&volume_type));
        let period = publication_period(record.view.volume_type.as_deref());
        // The periods the I/O something performed on this volume falls in: those are reported from
        // the records themselves, so no zero is reported for them.
        let mut moved_in: BTreeSet<i64> = BTreeSet::new();
        for performed in ::std::mem::take(&mut record.guest_io) {
            moved_in.insert(period_of(performed.at_secs, period));
            samples.extend(samples_of(volume_id, &performed, provisioned_iops));
        }
        // ... and the periods it was ATTACHED for that are over and carry none.
        if record.attachments_at(reading).is_empty() {
            // Amazon EBS reports nothing for a volume attached to nothing, and starts again from
            // the period something attaches it in.
            record.io_published_through_secs = None;
            continue;
        }
        let current = period_of(at_secs, period);
        let Some(published_through) = record.io_published_through_secs else {
            // The first report of this volume: the periods before it are periods nothing was
            // measuring it for, so none of them is reported.
            record.io_published_through_secs = Some(current);
            continue;
        };
        let from = published_through
            .saturating_add(period)
            .max(current.saturating_sub(period.saturating_mul(MAX_PERIODS_PER_PUBLICATION)));
        let mut reported = published_through;
        let mut period_start = from;
        while period_start < current {
            if !moved_in.contains(&period_start) {
                samples.extend(idle_samples_of(volume_id, period_start, provisioned_iops));
            }
            reported = period_start;
            period_start = period_start.saturating_add(period);
        }
        record.io_published_through_secs = Some(reported.max(published_through));
    }
    samples
}

/// The START of the `period`-long period `at_secs` falls in, which is the instant a reading of that
/// period carries — the grid CloudWatch aggregates a period of that length on.
fn period_of(at_secs: i64, period: i64) -> i64 {
    let period = period.max(1);
    at_secs - at_secs.rem_euclid(period)
}

/// The datapoints an attached volume owes for a period NOTHING moved in: a reading of zero for
/// every series it publishes, carrying that period's own instant.
fn idle_samples_of(volume_id: &str, at_secs: i64, provisioned_iops: bool) -> Vec<AgentSample> {
    let dimensions = BTreeMap::from([(VOLUME_ID_DIMENSION.to_string(), volume_id.to_string())]);
    VOLUME_METRICS
        .iter()
        .filter(|metric| provisioned_iops || !metric.provisioned_iops_only)
        .map(|metric| AgentSample {
            namespace: NAMESPACE.to_string(),
            metric_name: metric.name.to_string(),
            dimensions: dimensions.clone(),
            value: 0.0,
            unit: Some(metric.unit.to_string()),
            at_secs,
        })
        .collect()
}

/// The datapoints ONE transfer on the volume `volume_id` is a measurement of.
fn samples_of(
    volume_id: &str,
    record: &VolumeIoRecord,
    provisioned_iops: bool,
) -> Vec<AgentSample> {
    let performed = record.performed;
    let dimensions = BTreeMap::from([(VOLUME_ID_DIMENSION.to_string(), volume_id.to_string())]);
    VOLUME_METRICS
        .iter()
        .filter(|metric| provisioned_iops || !metric.provisioned_iops_only)
        .filter_map(|metric| {
            let measured = (metric.measures)(&performed);
            // A measurement of nothing is not a measurement: a period in which nothing wrote the
            // volume carries no write datapoint, which is what tells it from one that wrote zero
            // bytes.
            (measured > 0).then(|| {
                #[allow(clippy::cast_precision_loss)]
                let value = measured as f64;
                AgentSample {
                    namespace: NAMESPACE.to_string(),
                    metric_name: metric.name.to_string(),
                    dimensions: dimensions.clone(),
                    value,
                    unit: Some(metric.unit.to_string()),
                    at_secs: record.at_secs,
                }
            })
        })
        .collect()
}

/// `at` as the epoch seconds a datapoint's time stamp carries.
fn epoch_secs(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a transfer of `n` bytes is the operations it takes to move them at the documented
    /// operation size each — over each published bound and one past it.
    #[test]
    fn a_transfer_is_counted_in_operations_of_the_documented_size() {
        // (case, bytes, operations)
        let rows = [
            ("nothing at all", 0_i64, 0_i64),
            ("one byte", 1, 1),
            ("exactly one operation", IO_OPERATION_BYTES, 1),
            ("one byte past one operation", IO_OPERATION_BYTES + 1, 2),
            ("the documented four", 1_024 * 1_024, 4),
            ("a negative figure", -1, 0),
        ];
        for (case, bytes, operations) in rows {
            let counted = operations_for(bytes);
            assert_eq!(
                counted, operations,
                "case {case:?}: expected {operations} operations, got {counted}"
            );
        }
    }

    /// RULE: a transfer publishes exactly the series it is a measurement of, under the volume's own
    /// dimension — and the consumed-IOPS series only for a provisioned-IOPS volume type.
    #[test]
    fn a_transfer_publishes_the_series_it_measures() {
        let at_secs = 1_700_000_000;
        // (case, the transfer, whether the volume provisions IOPS, the series expected)
        let rows: [(&str, VolumeIo, bool, &[&str]); 4] = [
            (
                "a write on a gp3 volume",
                VolumeIo {
                    write_ops: 4,
                    write_bytes: 1_048_576,
                    ..VolumeIo::default()
                },
                false,
                &["VolumeWriteOps", "VolumeWriteBytes"],
            ),
            (
                "the same write on an io2 volume",
                VolumeIo {
                    write_ops: 4,
                    write_bytes: 1_048_576,
                    ..VolumeIo::default()
                },
                true,
                &[
                    "VolumeWriteOps",
                    "VolumeWriteBytes",
                    "VolumeConsumedReadWriteOps",
                ],
            ),
            (
                "a read alone",
                VolumeIo {
                    read_ops: 2,
                    read_bytes: 512_000,
                    ..VolumeIo::default()
                },
                false,
                &["VolumeReadOps", "VolumeReadBytes"],
            ),
            (
                "a read and a write together",
                VolumeIo {
                    read_ops: 1,
                    read_bytes: 1_024,
                    write_ops: 1,
                    write_bytes: 1_024,
                },
                false,
                &[
                    "VolumeReadOps",
                    "VolumeWriteOps",
                    "VolumeReadBytes",
                    "VolumeWriteBytes",
                ],
            ),
        ];
        for (case, performed, provisioned_iops, expected) in rows {
            let record = VolumeIoRecord {
                performed,
                at_secs,
            };
            let samples = samples_of("vol-0123456789abcdef0", &record, provisioned_iops);
            let mut published: Vec<&str> = samples
                .iter()
                .map(|sample| sample.metric_name.as_str())
                .collect();
            published.sort_unstable();
            let mut wanted: Vec<&str> = expected.to_vec();
            wanted.sort_unstable();
            assert_eq!(
                published, wanted,
                "case {case:?}: expected exactly those series, got {samples:?}"
            );
            for sample in &samples {
                assert_eq!(
                    sample.namespace, NAMESPACE,
                    "case {case:?}: expected the volume namespace, got {sample:?}"
                );
                assert_eq!(
                    sample.dimensions.get(VOLUME_ID_DIMENSION).map(String::as_str),
                    Some("vol-0123456789abcdef0"),
                    "case {case:?}: expected the volume's own dimension, got {sample:?}"
                );
                assert_eq!(
                    sample.at_secs, at_secs,
                    "case {case:?}: expected the instant the I/O happened, got {sample:?}"
                );
                assert!(
                    sample.value > 0.0,
                    "case {case:?}: expected a measurement of what happened, got {sample:?}"
                );
            }
        }
    }
}
