// SPDX-License-Identifier: CC0-1.0

//! The anomaly detection models of this store: the identifier a write that CREATES one mints
//! for it, the record such a write stores, and what a READ of that record reports.
//!
//! Three rules live here, and every writer and reader of a model goes through them, so two of
//! them cannot answer differently:
//!
//! * a model carries the identifier the write that CREATED it minted
//!   ([`minted_anomaly_detector_id`]) — ONE opaque token of this service's, not a function of
//!   what the model watches, so a model deleted and created again is a DIFFERENT model with a
//!   different identifier, which is what lets a client tell a re-created model from the one it
//!   replaced. Which model a write updates is decided by what the model WATCHES
//!   ([`super::AnomalyDetectorForm::detector_key`]) rather than by that identifier, so a second
//!   write of the same metric and statistic reports the identifier the first one minted.
//! * a model is created from the metric or the queries the write names, PENDING TRAINING
//!   ([`StoredAnomalyDetector::created`]) — one construction for both writes that create one,
//!   the `PutAnomalyDetector` that creates a model directly and the alarm write whose threshold
//!   is drawn from a model's band.
//! * what a read reports is the identity the model was created with, IN FULL
//!   ([`StoredAnomalyDetector::reported`]): every `Dimensions` list on it is present and names
//!   no dimension where the write named none, the training configuration is present with the
//!   time ranges it excludes, and the account the series belongs to is named — "If you omit this
//!   parameter, the current account is used."
//!   (`SingleMetricAnomalyDetector$AccountId`) — so a `DescribeAnomalyDetectors` result can be
//!   fed straight back into `PutAnomalyDetector` and names the same model.

use super::{configured_list, AnomalyDetectorForm, StoredAnomalyDetector};
use crate::ops::types::{
    AnomalyDetector, AnomalyDetectorConfiguration, MetricCharacteristics,
    SingleMetricAnomalyDetector,
};

impl StoredAnomalyDetector {
    /// The model a write of `form` stores: the metric or the queries it watches, the training
    /// configuration and the metric characteristics the write configured it with, and the
    /// identifier `detector_id` names it by — pending training, with that training owed
    /// [`super::TRAINING_DELAY_SECS`] after `now_secs`.
    ///
    /// One construction for every write that creates a model, so a model the alarm write created
    /// for the band its threshold is drawn from is the same record, in the same state, as one
    /// `PutAnomalyDetector` created.
    #[must_use]
    pub fn created(
        form: &AnomalyDetectorForm,
        configuration: Option<AnomalyDetectorConfiguration>,
        metric_characteristics: Option<MetricCharacteristics>,
        detector_id: String,
        now_secs: f64,
    ) -> Self {
        let single = form.single_metric().cloned();
        let math = match form {
            AnomalyDetectorForm::SingleMetric(_) => None,
            AnomalyDetectorForm::MetricMath(math) => Some(math.clone()),
        };
        Self {
            detector: AnomalyDetector {
                anomaly_detector_id: Some(detector_id),
                // The deprecated flat members report the SAME metric the single-metric
                // structure carries ("Use SingleMetricAnomalyDetector.Namespace property.",
                // `AnomalyDetector$Namespace`), so a client reading either spelling reads one
                // model; a metric-math model has no single metric to report them for.
                namespace: single.as_ref().and_then(|single| single.namespace.clone()),
                metric_name: single
                    .as_ref()
                    .and_then(|single| single.metric_name.clone()),
                dimensions: single.as_ref().and_then(|single| single.dimensions.clone()),
                stat: single.as_ref().and_then(|single| single.stat.clone()),
                configuration,
                // A write trains the model from the configuration it carries, and training is
                // owed a span later, so the model this write stored is pending training whether
                // it created the model or updated one that was already trained.
                state_value: Some(super::ANOMALY_DETECTOR_PENDING_TRAINING.to_string()),
                metric_characteristics,
                single_metric_anomaly_detector: single,
                metric_math_anomaly_detector: math,
            },
            trained_due_secs: now_secs + super::TRAINING_DELAY_SECS,
        }
    }

    /// This model as a READ of it reports it: the identity it was created with, in full.
    ///
    /// Through this one projection, so every reader of a model reports the same thing for it.
    #[must_use]
    pub fn reported(&self) -> AnomalyDetector {
        let detector = &self.detector;
        let single = detector
            .single_metric_anomaly_detector
            .as_ref()
            .map(|single| SingleMetricAnomalyDetector {
                // "If you omit this parameter, the current account is used."
                // (`SingleMetricAnomalyDetector$AccountId`) — so the account a read names is
                // the one the series belongs to, whether the write named it or left it to the
                // caller's own.
                account_id: Some(detector_account_id(single.account_id.as_deref())),
                namespace: single.namespace.clone(),
                metric_name: single.metric_name.clone(),
                dimensions: configured_list(single.dimensions.as_deref()),
                stat: single.stat.clone(),
            });
        AnomalyDetector {
            anomaly_detector_id: detector.anomaly_detector_id.clone(),
            // The deprecated flat members mirror the structure, so they are reported from the
            // structure this read already normalised rather than from the stored copy.
            namespace: single.as_ref().and_then(|single| single.namespace.clone()),
            metric_name: single
                .as_ref()
                .and_then(|single| single.metric_name.clone()),
            dimensions: single.as_ref().and_then(|single| single.dimensions.clone()),
            stat: single.as_ref().and_then(|single| single.stat.clone()),
            configuration: Some(reported_configuration(detector.configuration.as_ref())),
            state_value: detector.state_value.clone(),
            metric_characteristics: detector.metric_characteristics.clone(),
            single_metric_anomaly_detector: single,
            metric_math_anomaly_detector: detector.metric_math_anomaly_detector.clone(),
        }
    }
}

/// The training configuration a read of a model reports.
///
/// Every model is trained under a configuration — "The configuration specifies details about how
/// the anomaly detection model is to be trained, including time ranges to exclude from use for
/// training the model" (`AnomalyDetector$Configuration`) — so the member is present whether or
/// not the write that stored the model named one, and the ranges it excludes are the list the
/// write named, EMPTY where it named none ([`configured_list`]). The members that select how the
/// model adapts are reported only where the write set them: each has a default the model itself
/// publishes, so naming none of them is not the same as naming one.
fn reported_configuration(
    configuration: Option<&AnomalyDetectorConfiguration>,
) -> AnomalyDetectorConfiguration {
    AnomalyDetectorConfiguration {
        excluded_time_ranges: configured_list(
            configuration.and_then(|configuration| configuration.excluded_time_ranges.as_deref()),
        ),
        metric_timezone: configuration.and_then(|configuration| configuration.metric_timezone.clone()),
        adaptation_speed: configuration
            .and_then(|configuration| configuration.adaptation_speed.clone()),
        outlier_filtering_strength: configuration
            .and_then(|configuration| configuration.outlier_filtering_strength.clone()),
    }
}

/// The account the series a single-metric model watches belongs to: the one the write named, or
/// the caller's own where it named none.
///
/// "If you omit this parameter, the current account is used."
/// (`SingleMetricAnomalyDetector$AccountId`) — so an omitted account and this account's own id
/// name the same series, which is what both the identity a write is keyed by
/// ([`super::AnomalyDetectorForm::detector_key`]) and the account a read reports are resolved
/// through. The caller's own account is the account contract's id
/// (`emulator_server::emulator_account_id`) rather than a literal.
#[must_use]
pub fn detector_account_id(named: Option<&str>) -> String {
    named
        .filter(|account| !account.is_empty())
        .unwrap_or_else(|| ::emulator_server::emulator_account_id())
        .to_string()
}

/// The identifier a write that CREATES an anomaly detection model mints for it.
///
/// "The unique identifier of the anomaly detector." (`AnomalyDetector$AnomalyDetectorId`) — an
/// opaque value the SERVICE mints, so nothing in it comes from the request: two models created
/// for identical definitions carry different identifiers, and a model deleted and created again
/// never reproduces the identifier it had.
///
/// ONE TOKEN: the identifier is a single run of hexadecimal characters, two for each of the
/// opaque bytes it is minted from, well inside the modelled `AnomalyDetectorId` (length
/// 1..128, pattern `^[A-Za-z0-9_./:%()+-]+$`). An identifier a service hands a client is
/// opaque, so its text carries NO separator a client could split it into segments on and NO
/// clock a client could read the creation instant out of: a client that validates the
/// identifier, places it in a path or interpolates it into an ARN or a URL reads one token, and
/// nothing the identifier says changes with the instant the write happened at.
///
/// The bytes are the mixing every identity this service mints draws from
/// ([`super::alarm_reads::minted_bytes`]), which is where one identifier's uniqueness comes
/// from; the RENDERING is this identifier's own, since the creation identity an alarm read
/// reports is a composite of a uuid and the instant of its write and this identifier is not.
///
/// MINTED, NEVER RENDERED FROM THE REQUEST: the identifier cannot carry the identity of the
/// series the model watches, because 128 characters do not hold it — a namespace and a metric
/// name of 255 characters each (`SingleMetricAnomalyDetector$Namespace`, `$MetricName`), and up
/// to thirty dimensions whose name runs to 255 characters and whose value to 1024
/// (`Dimension$Name`, `$Value`). So the model fixes the identifier's LENGTH and CHARACTER SET
/// and nothing else, and what a client may rely on is exactly that, plus the identity
/// properties the suite `tests/all/anomaly_detector_id_is_minted.rs` pins: the value is the
/// model's for as long as the model stands, it names that model and no other wherever an
/// operation takes an identifier, and the bytes of any one of them are this run's.
#[must_use]
pub fn minted_anomaly_detector_id() -> String {
    ::hex::encode(super::alarm_reads::minted_bytes())
}
