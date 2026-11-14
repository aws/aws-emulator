// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice service.
//!
//! The service's single primary resource is the *report definition*: every operation
//! this crate implements creates, reads, updates, tags or deletes one of them, so the
//! whole store is the insertion-ordered list of definitions below, the tags attached to
//! each, and what the service itself knows about each one — when it was created and when
//! it was last delivered. Insertion order is the enumeration order
//! `DescribeReportDefinitions` pages over, which is what makes its pagination
//! deterministic.
//!
//! The store also holds the secret its continuation tokens are authenticated under
//! ([`PageTokenKey`]), which is what lets a paginated read tell a token it ISSUED from one
//! a caller wrote for itself.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

pub use ::service_common::page_tokens::PageTokenKey;

use crate::ops::types::{
    ReportDefinition, ReportStatus, ResourceNotFoundException, Tag, ValidationException,
};

/// How many report definitions one account may hold.
///
/// Grounded in the modelled `ReportLimitReachedException` doc: "This account already
/// has five reports defined. To define a new report, you must delete an existing
/// report." Enforced by `PutReportDefinition`, the only operation that adds one.
pub const MAX_REPORTS: usize = 5;

/// How many tags one report definition may hold.
///
/// Grounded in the modelled `Tag` doc: "Describes a tag. A tag is a key-value pair. You
/// can add up to 50 tags to a report definition."
/// (`smithy_query(shape="Tag")`). The modelled `TagList` length 0..200 bounds what ONE
/// request may carry; this bounds what a report ACCUMULATES across requests, so the
/// stored tag set of a report cannot grow without limit however often a caller tags it.
/// Enforced by every path that attaches tags — `PutReportDefinition` and `TagResource`
/// — through [`StoredReport::merge_tags`].
pub const MAX_TAGS_PER_REPORT: usize = 50;

/// The refusal for a `ReportName` the account does not hold, shared by every operation
/// that looks a report up by name before touching its tags.
///
/// Built with NO `Message`. The modelled exception's only member is an OPTIONAL
/// `Message`, documented as "A message to show the detail of the exception"
/// (`smithy_query(shape="ErrorMessage")`), and the error shape's own documentation
/// describes the error CLASS rather than being text this service puts on the wire — so
/// the refusal carries its error code and HTTP status alone, and nothing from the
/// request (least of all the report name) is echoed back in it. One constructor so the
/// handlers that answer this refusal cannot drift apart.
pub fn report_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException { message: None }
}

/// The refusal for an UPDATE whose definition renames a report onto a name the account
/// ALREADY holds — the same resource-state condition `PutReportDefinition` reports as
/// `DuplicateReportNameException`.
///
/// `ModifyReportDefinition` declares no duplicate-name error: `ValidationException` is its
/// only client-input error (`smithy_query(operation="ModifyReportDefinition")`), so the
/// condition is reported under that code. It is built with NO `Message`, for the same
/// reason as [`report_not_found`]: the modelled exception's only member is an OPTIONAL
/// `Message`, the error shape's own documentation describes the error CLASS rather than
/// being text this service puts on the wire, and nothing from the request — least of all a
/// report name — is echoed back in it. Unlike [`report_to_update_not_found`], which has to
/// identify the report the lookup did not find, this refusal reports a condition the
/// request already states both sides of, so its code and HTTP status are the whole of it.
/// One constructor, so the create and the update path cannot drift apart about the one
/// condition they share.
pub fn report_name_already_held() -> ValidationException {
    ValidationException { message: None }
}

/// The type every report definition this service holds is of.
///
/// The service's one resource is the Cost and Usage Report, and no member of the model
/// names its type (`smithy_query(query="digest")` lists no report-type shape), so the
/// single type this service's reports carry is stated here rather than read off a
/// request. It is a variable part of [`report_to_update_not_found`] only in the sense
/// that the sentence names it: a service holding one type always fills it the same way.
const REPORT_TYPE: &str = "CUR";

/// The refusal for an UPDATE naming a report the account does not hold.
///
/// Unlike the tag plane's [`report_not_found`], the update path refuses with the
/// operation's own `ValidationException` — the only client-input error it models — and
/// that refusal multiplexes several conditions, so the message is what tells them apart
/// and it IDENTIFIES the report the update could not find: the name the request named,
/// the type of report the service looked for ([`REPORT_TYPE`]), and the account the
/// lookup ran against. The account is this emulator's own canonical one, read through
/// `engine::emulator_account_id` rather than written as a literal.
pub fn report_to_update_not_found(report_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "The report name [{report_name}] and report type [{REPORT_TYPE}] to be update \
             doesn't exist in account [{}]",
            ::engine::emulator_account_id()
        )),
    }
}

/// The billing view a report definition is scoped to when its creation named none: the
/// PRIMARY billing view of the account that holds it.
///
/// A report definition read back from the service always names a billing view, and one
/// created without a `BillingViewArn` names the account's primary view. The ARN is the
/// billing service's `billingview/primary` under this emulator's account — built through
/// `engine::emulator_arn`, so the account id is never a literal here — and carries no
/// region, which is the form the modelled `BillingViewArn` pattern
/// `^(arn:aws(-cn)?:billing::[0-9]{12}:billingview/)?...` admits
/// (`smithy_query(shape="BillingViewArn")`).
pub fn primary_billing_view_arn() -> String {
    ::engine::emulator_arn("billing", "", "billingview/primary")
}

/// Whether a report is refreshed after it has been finalized, when its definition does
/// not say.
///
/// `RefreshClosedReports` is optional on the way in, but a report definition read back
/// from the service always states it, so what comes back for a definition that named none
/// is the value the service resolved for it: a report IS refreshed when charges for a
/// closed month appear unless its definition turns that off.
pub const DEFAULT_REFRESH_CLOSED_REPORTS: bool = true;

/// How a report is versioned when its definition does not say.
///
/// `ReportVersioning` is optional on the way in — "whether you want Amazon Web Services to
/// overwrite the previous version of each report or to deliver the report in addition to
/// the previous versions" (`smithy_query(shape="ReportDefinition",
/// member="ReportVersioning")`) — but a report definition read back from the service
/// always states it, so what comes back for a definition that named none is the value the
/// service resolved for it: each delivery is a NEW version, keeping the ones already
/// delivered. One of the modelled `ReportVersioning` values
/// (`smithy_query(shape="ReportVersioning")`).
pub const DEFAULT_REPORT_VERSIONING: &str = "CREATE_NEW_REPORT";

/// One instant in the form this service states a report timestamp in
/// (`yyyymmddThhmmssZ`).
///
/// That form is the modelled `LastDelivery` pattern
/// `^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$` (`smithy_query(shape="LastDelivery")`) — the
/// only timestamp form this service's model declares — so every instant this service
/// reports is stated in it. The calendar conversion comes from
/// `aws_smithy_types::DateTime`, the timestamp type every modelled member is serialized
/// through: only the separators that form leaves out are removed here.
pub fn report_timestamp(at: SystemTime) -> String {
    let at = ::aws_smithy_types::DateTime::from(at);
    ::aws_smithy_types::DateTime::from_secs(at.secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
        .chars()
        .filter(|character| !matches!(character, '-' | ':'))
        .collect()
}

/// The refusal text for a definition whose Parquet preference is not usable.
///
/// Fixed text, with nothing from the request in it: a report's Parquet preference is
/// stated by several members at once, so the refusal names the whole rule rather than one
/// member.
pub const PARQUET_PREFERENCE_MESSAGE: &str = "For valid Parquet preference, report format \
     and compression must be 'Parquet' and cannot subscribe to QuickSight and/or Redshift";

/// How `ReportFormat` and `CompressionFormat` both spell Parquet
/// (`smithy_query(shape="ReportFormat,CompressionFormat")`).
const PARQUET: &str = "Parquet";

/// The additional artifacts a Parquet report may not subscribe to: the manifests built for
/// the two engines that read a report in the delimited-text form
/// (`smithy_query(shape="AdditionalArtifact")`).
const NON_PARQUET_ARTIFACTS: [&str; 2] = ["QUICKSIGHT", "REDSHIFT"];

/// Whether `definition` states a usable Parquet preference.
///
/// A report's Parquet preference is its `Format` and `Compression` taken together with the
/// artifacts it subscribes to: a definition that states Parquet at all must state it in
/// BOTH members and may not also ask for the artifacts that are built from a
/// delimited-text report, while a definition that states it nowhere has no Parquet
/// preference to be wrong about. Shared by the two operations that take a
/// `ReportDefinition`, so the create and the update cannot drift apart.
pub fn parquet_preference_is_usable(definition: &ReportDefinition) -> bool {
    let format = definition.format == PARQUET;
    let compression = definition.compression == PARQUET;
    if !format && !compression {
        return true;
    }
    format
        && compression
        && !definition
            .additional_artifacts
            .iter()
            .flatten()
            .any(|artifact| NON_PARQUET_ARTIFACTS.contains(&artifact.as_str()))
}

/// Why a tag merge was refused: it would leave the report holding more tags than
/// [`MAX_TAGS_PER_REPORT`].
///
/// Carries the resulting count so the refusal can name the number the caller asked for
/// beside the limit it crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagLimitExceeded {
    /// How many distinct tag keys the report would hold if the merge were applied.
    pub requested: usize,
    /// The per-report limit the merge would cross: [`MAX_TAGS_PER_REPORT`].
    pub limit: usize,
}

/// The last delivery of a report: when the report was delivered and how that delivery
/// ended.
///
/// What [`StoredReport::delivery_status`] reports as the modelled `ReportStatus`. Nothing
/// in this service's modelled surface delivers a report — delivery is the billing
/// pipeline's, not an API call's — so a report created through `PutReportDefinition`
/// holds none until a delivery is recorded against it, and a read of such a report states
/// no `ReportStatus` at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDelivery {
    /// The instant the report was delivered, reported through [`report_timestamp`].
    pub at: SystemTime,
    /// How the delivery ended: one of the modelled `LastStatus` values
    /// (`smithy_query(shape="LastStatus")`).
    pub status: String,
}

/// One persisted report definition: the definition exactly as the caller supplied it,
/// its tags, when it was first created, and the last delivery recorded against it.
///
/// Tags live in a `BTreeMap` because a report definition carries at most one tag per
/// key ("Each report definition can only have up to one tag with the same key", per
/// the modelled `Tag$Key` doc), and a sorted map gives tag reads a stable order.
#[derive(Debug, Clone)]
pub struct StoredReport {
    /// The definition as supplied, member for member.
    pub definition: ReportDefinition,
    /// `Key` -> `Value` for every tag currently attached to this report.
    pub tags: BTreeMap<String, String>,
    /// When `PutReportDefinition` first inserted this report.
    pub created_at: SystemTime,
    /// The last delivery of this report, or `None` while it has never been delivered.
    pub last_delivery: Option<ReportDelivery>,
}

impl StoredReport {
    /// A new stored report from a supplied definition and the tags that came with it.
    ///
    /// Refused with [`TagLimitExceeded`] when those tags alone would already take the
    /// report past [`MAX_TAGS_PER_REPORT`], so a report is never created over the limit.
    pub fn new(definition: ReportDefinition, tags: &[Tag]) -> Result<Self, TagLimitExceeded> {
        let mut stored = Self {
            definition,
            tags: BTreeMap::new(),
            created_at: SystemTime::now(),
            last_delivery: None,
        };
        stored.merge_tags(tags)?;
        Ok(stored)
    }

    /// This report as the service reports it: every member of the wire shape the service
    /// holds a value for.
    ///
    /// A read-back of a report definition is not the echo of the creation request. It
    /// states EVERY member: the ones the definition named, the value the service resolved
    /// for each optional member it left unset — no additional artifacts is the empty list,
    /// an unstated refresh preference is [`DEFAULT_REFRESH_CLOSED_REPORTS`], an unstated
    /// versioning preference is [`DEFAULT_REPORT_VERSIONING`], and an unstated billing view
    /// is the account's primary one ([`primary_billing_view_arn`]) — and the service's own
    /// facts about the report: the status of its last delivery
    /// ([`Self::delivery_status`], stated only once a delivery has been recorded against
    /// the report) and the date it was created, which is the STORED instant and so the
    /// same on every read rather than the moment of the read.
    pub fn wire_definition(&self) -> ReportDefinition {
        ReportDefinition {
            additional_artifacts: Some(
                self.definition
                    .additional_artifacts
                    .clone()
                    .unwrap_or_default(),
            ),
            refresh_closed_reports: Some(
                self.definition
                    .refresh_closed_reports
                    .unwrap_or(DEFAULT_REFRESH_CLOSED_REPORTS),
            ),
            report_versioning: Some(
                self.definition
                    .report_versioning
                    .clone()
                    .unwrap_or_else(|| DEFAULT_REPORT_VERSIONING.to_string()),
            ),
            billing_view_arn: Some(
                self.definition
                    .billing_view_arn
                    .clone()
                    .unwrap_or_else(primary_billing_view_arn),
            ),
            report_status: self.delivery_status(),
            report_creation_date: Some(report_timestamp(self.created_at)),
            ..self.definition.clone()
        }
    }

    /// The modelled `ReportStatus` of this report: "the date and status of the last
    /// delivered report for a particular report definition" (the modelled `ReportStatus`
    /// doc).
    ///
    /// `None` for a report no delivery has been recorded against: there is no delivery
    /// date and no delivery outcome to state, and both members are constrained — the
    /// `lastDelivery` to length 16..20 and the pattern
    /// `^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$`, the `lastStatus` to the `LastStatus`
    /// catalog (`smithy_query(shape="LastDelivery,LastStatus")`) — so the optional member
    /// is left out of the read-back rather than filled with a stand-in those constraints
    /// reject. That is what keeps the definition this service reports a definition it also
    /// accepts back, which is the read-modify-write `ModifyReportDefinition` is built on.
    pub fn delivery_status(&self) -> Option<ReportStatus> {
        let delivery = self.last_delivery.as_ref()?;
        Some(ReportStatus {
            last_delivery: Some(report_timestamp(delivery.at)),
            last_status: Some(delivery.status.clone()),
        })
    }

    /// Merge `tags` onto this report: a key already present takes the new value, a key
    /// not present is added. This is what `TagResource` does, and what
    /// `PutReportDefinition` does with the tags supplied beside the definition.
    ///
    /// The resulting key count is computed BEFORE anything is inserted, so a merge that
    /// would cross [`MAX_TAGS_PER_REPORT`] is refused whole: the stored tag set is left
    /// exactly as it was rather than partly grown. Re-tagging keys the report already
    /// holds never crosses the limit, because it adds no key.
    pub fn merge_tags(&mut self, tags: &[Tag]) -> Result<(), TagLimitExceeded> {
        let mut keys: BTreeSet<&str> = self.tags.keys().map(String::as_str).collect();
        keys.extend(tags.iter().map(|tag| tag.key.as_str()));
        let requested = keys.len();
        if requested > MAX_TAGS_PER_REPORT {
            return Err(TagLimitExceeded {
                requested,
                limit: MAX_TAGS_PER_REPORT,
            });
        }

        for tag in tags {
            self.tags.insert(tag.key.clone(), tag.value.clone());
        }
        Ok(())
    }

    /// Remove every tag whose key is in `keys`; a key that is not attached is ignored.
    pub fn remove_tags(&mut self, keys: &[String]) {
        for key in keys {
            self.tags.remove(key);
        }
    }

    /// This report's tags as the modelled `TagList`, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState {
    /// Every report definition this account holds, in the order they were created.
    ///
    /// A `Vec` rather than a map: the account holds at most [`MAX_REPORTS`] of them, so
    /// a name lookup is a short scan, and the vector *is* the insertion order
    /// `DescribeReportDefinitions` pages over.
    pub reports: Vec<StoredReport>,
    /// The secret THIS store's continuation tokens are authenticated under, drawn from
    /// the system's randomness when the store is created ([`PageTokenKey`]).
    ///
    /// Private, and handed out only as a [`Copy`] of the key ([`Self::page_token_key`]),
    /// so a token is minted and verified through the key the store holds rather than from
    /// anything a request carries. A token minted under another store's key — another
    /// emulator process, a restarted one — is therefore not a token this store issued.
    page_token_key: PageTokenKey,
}

impl ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState {
    /// This store's page-token secret, for the paginated read that mints and verifies a
    /// `NextToken`. A `Copy` of the key, so a read can take it and go on to borrow the
    /// reports it is about to report in the same call.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }

    /// The index of the report called `report_name`, if the account holds it.
    ///
    /// Report names are the resource's identity, and AWS report names are
    /// case-sensitive, so this compares them exactly.
    pub fn position(&self, report_name: &str) -> Option<usize> {
        self.reports
            .iter()
            .position(|report| report.definition.report_name == report_name)
    }

    /// The stored report called `report_name`, if the account holds it.
    pub fn get(&self, report_name: &str) -> Option<&StoredReport> {
        self.position(report_name).map(|at| &self.reports[at])
    }

    /// The stored report called `report_name`, mutably, if the account holds it.
    pub fn get_mut(&mut self, report_name: &str) -> Option<&mut StoredReport> {
        match self.position(report_name) {
            Some(at) => self.reports.get_mut(at),
            None => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState>)
}
