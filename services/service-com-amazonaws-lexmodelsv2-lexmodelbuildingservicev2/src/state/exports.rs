// SPDX-License-Identifier: CC0-1.0

//! The export jobs this control plane holds: what `CreateExport` records, and what
//! `DescribeExport` and `ListExports` read back off that record.
//!
//! An export is a JOB over a resource that already exists — a bot, or one locale of a bot
//! version — so the resource the job names is resolved against the stored bots, versions
//! and locales BEFORE any job is recorded, and a specification naming one the store does
//! not hold leaves nothing behind
//! ([`super::request_path_resource_missing`] is what the refusal says).
//!
//! A recorded job starts [`EXPORT_STATUS_IN_PROGRESS`] and the terminal status the clock
//! owes is due [`EXPORT_SETTLES_AFTER`] later: the due instant is stored and the status is
//! materialised by the next read, so a caller reading the export straight back still
//! observes the transient status, and nothing sleeps for it. The instant the transition
//! fell due is what `lastUpdatedDateTime` then reports ([`PendingExportStatus`]), so the
//! order two exports settle in is the order they were submitted in.
//!
//! The terminal status owed is the one that says the archive is ready to download: the
//! archive a job exists to assemble IS assembled, out of the records this store holds for
//! the resource the job names (see [`super::export_archive`]), by the call that records the
//! job — so the job carries its own bytes from that instant and a read past the due one
//! reports [`EXPORT_STATUS_COMPLETED`] and the address those bytes are served at.

use super::*;

use crate::ops::types::{ExportResourceSpecification, ExportSummary};

/// `ExportStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="ExportStatus")`:
/// InProgress | Completed | Failed | Deleting. AWS documents this one as the status a
/// submitted export is in while Amazon Lex is still assembling the archive.
pub const EXPORT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`EXPORT_STATUS_IN_PROGRESS`]. AWS documents this one as the status in which "the
/// export is ready to download", and `DescribeExport` reports the archive's pre-signed
/// URL on a job that reached it.
pub const EXPORT_STATUS_COMPLETED: &str = "Completed";
/// See [`EXPORT_STATUS_IN_PROGRESS`]. The modelled status of a job that did not produce
/// an archive; `DescribeExport` documents `failureReasons` as carrying "one or more
/// reasons why the export could not be completed" for exactly this status.
pub const EXPORT_STATUS_FAILED: &str = "Failed";

/// How long after a create the `InProgress` -> terminal transition of an export falls
/// due.
///
/// AWS documents the transition but not its duration (`CreateExport` answers while the
/// archive is being assembled, and the job later reaches a status "When the status is
/// `Completed`, you can use the `DescribeExport` operation to get the pre-signed S3 URL
/// link to your exported bot or bot locale"), so the span is CHOSEN: non-zero, so a
/// caller that reads the export straight back still observes `InProgress`, and short
/// enough that a client polling for the terminal status moves on quickly. Nothing sleeps
/// for it — the due instant is stored and the status is materialised by the next read,
/// which is what the clock already owed.
pub const EXPORT_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// How long an export is kept: `ListExports` documents it
/// (`smithy_query(query="doc",shape="ListExports")`) — "Exports are kept in the list for
/// 7 days" — so the duration is the service's, not a chosen one.
///
/// Dropping an export once it is past that is what the clock already owes, so it is
/// materialised by the next call ([`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::drop_expired_exports`])
/// rather than slept for, and it is also what bounds the stored exports: a caller cannot
/// grow that map beyond the jobs it submitted inside one retention window.
pub const EXPORT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many bytes of assembled archives this store keeps across every job it holds.
///
/// A CHOSEN bound, and the one that keeps the archives off the host's back: a job carries
/// the bytes it assembled for as long as the service keeps the job, and a caller may submit
/// jobs for as long as [`EXPORT_RETENTION`] runs, so the archives alone are what the stored
/// jobs can grow without one. Once the archives held pass this, the OLDEST jobs are dropped
/// until they fit again
/// ([`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::fit_export_archives`]), which
/// is the same drop [`EXPORT_RETENTION`] makes, only sooner: a job that was dropped is
/// reported exactly as one whose window has closed, so no answer this service gives is
/// affected by it.
pub const EXPORT_ARCHIVES_HELD_BYTES: usize = 64 * 1024 * 1024;

/// How the `ExportResourceType` filter of `ListExports` names an export of a whole bot.
/// The values that filter admits are the ones its own documentation lists
/// (`smithy_query(query="doc",shape="ExportFilter",member="values")`: "The values must be
/// `Bot`, `BotLocale`, or `CustomVocabulary`"), plus the test set the modelled
/// `ExportResourceSpecification` carries a fourth specification for.
pub const EXPORT_RESOURCE_TYPE_BOT: &str = "Bot";
/// See [`EXPORT_RESOURCE_TYPE_BOT`]: an export of one locale of one bot version.
pub const EXPORT_RESOURCE_TYPE_BOT_LOCALE: &str = "BotLocale";
/// See [`EXPORT_RESOURCE_TYPE_BOT`]: an export of the custom vocabulary of one locale.
pub const EXPORT_RESOURCE_TYPE_CUSTOM_VOCABULARY: &str = "CustomVocabulary";
/// See [`EXPORT_RESOURCE_TYPE_BOT`]: an export of a test set.
pub const EXPORT_RESOURCE_TYPE_TEST_SET: &str = "TestSet";

/// Every member of the modelled `ExportFilterName` enum
/// (`smithy_query(query="shape",shape="ExportFilterName")`), which names only the kind of
/// resource an export is of.
pub const EXPORT_FILTER_NAMES: [&str; 1] = ["ExportResourceType"];

/// Every member of the modelled `ExportFilterOperator` enum
/// (`smithy_query(query="shape",shape="ExportFilterOperator")`).
pub const EXPORT_FILTER_OPERATORS: [&str; 2] = ["CO", "EQ"];

/// Every member of the modelled `ExportSortAttribute` enum
/// (`smithy_query(query="shape",shape="ExportSortAttribute")`), which names only the
/// instant an export was last updated.
pub const EXPORT_SORT_ATTRIBUTES: [&str; 1] = ["LastUpdatedDateTime"];

/// A transition an export still owes: the status it moves to, the instant it falls due,
/// and the wall clock reading of that same instant.
///
/// The wall clock reading is stored with the transition so that `lastUpdatedDateTime`
/// reports WHEN the export was updated rather than when somebody happened to read it:
/// the status moved at the instant it fell due, whether or not anybody called, so two
/// exports settle in the order they were submitted however they are read.
#[derive(Debug, Clone)]
pub struct PendingExportStatus {
    pub status: String,
    pub due: Instant,
    pub due_at: SystemTime,
    /// The reasons the job will report once that status is its own — empty for a status
    /// that reports none.
    pub failure_reasons: Vec<String>,
}

/// An export job, as `CreateExport` stored it and `DescribeExport` / `ListExports` read
/// it back.
#[derive(Debug, Clone)]
pub struct Export {
    pub export_id: String,
    /// The resource this job exports, exactly as the request specified it: one read
    /// reports it back, so what a caller asked for is what every later read states.
    pub resource_specification: ExportResourceSpecification,
    pub file_format: String,
    pub export_status: String,
    pub failure_reasons: Vec<String>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The zip archive this job assembled, held as the bytes a download of it serves (see
    /// [`super::export_archive`]). Assembled by the call that recorded the job, so it is
    /// that instant's snapshot of the exported resource whenever it is downloaded, and
    /// refcounted, so announcing or serving it copies nothing.
    pub archive: ::bytes::Bytes,
    /// The instant this export falls out of the window the service keeps one for
    /// ([`EXPORT_RETENTION`]), after which it is no longer one of the account's exports.
    pub kept_until: Instant,
    /// The transition the clock owes this export. `None` once the export has reached a
    /// status nothing further is owed from. Set it through [`Export::schedule_status`]
    /// rather than by hand.
    pub pending_status: Option<PendingExportStatus>,
}

impl Export {
    /// The export a create records: in progress from the call that submitted it, carrying
    /// the `archive` that call assembled, with the terminal status the clock owes due
    /// [`EXPORT_SETTLES_AFTER`] later.
    ///
    /// That terminal status is [`EXPORT_STATUS_COMPLETED`], and it reports no failure
    /// reason: the archive the job exists to assemble is already assembled when the job is
    /// recorded, so what the clock owes is the status that tells a caller the archive is
    /// ready to download.
    pub fn created(
        export_id: String,
        resource_specification: ExportResourceSpecification,
        file_format: String,
        archive: ::bytes::Bytes,
        now: SystemTime,
    ) -> Self {
        let mut export = Self {
            export_id,
            resource_specification,
            file_format,
            export_status: EXPORT_STATUS_IN_PROGRESS.to_string(),
            failure_reasons: Vec::new(),
            creation_date_time: now,
            last_updated_date_time: now,
            archive,
            kept_until: Instant::now() + EXPORT_RETENTION,
            pending_status: None,
        };
        export.schedule_status(
            EXPORT_STATUS_COMPLETED,
            Vec::new(),
            EXPORT_SETTLES_AFTER,
            now,
        );
        export
    }

    /// Has this job reached the status in which "the export is ready to download"?
    ///
    /// Read off the status the job CARRIES, so a job still inside the span the clock owes
    /// is not one yet: the archive is announced and served only once the transition that
    /// completes the job has fallen due.
    #[must_use]
    pub fn completed(&self) -> bool {
        self.export_status == EXPORT_STATUS_COMPLETED
    }

    /// Apply the status transition the clock has already made due, if any — the same
    /// contract as [`Bot::settle`]. The instant the export was last updated moves to the
    /// instant that transition fell due, because that transition is what updated it, and
    /// the reasons the new status reports arrive with it.
    pub fn settle(&mut self) {
        let Some(pending) = self.pending_status.take() else {
            return;
        };
        if Instant::now() >= pending.due {
            self.export_status = pending.status;
            self.failure_reasons = pending.failure_reasons;
            self.last_updated_date_time = pending.due_at;
        } else {
            self.pending_status = Some(pending);
        }
    }

    /// Schedule `status`, and the `failure_reasons` it reports, to become this export's
    /// once `after` has elapsed from `now`, which is the wall clock reading of the call
    /// that scheduled it.
    pub fn schedule_status(
        &mut self,
        status: &str,
        failure_reasons: Vec<String>,
        after: Duration,
        now: SystemTime,
    ) {
        self.pending_status = Some(PendingExportStatus {
            status: status.to_string(),
            due: Instant::now() + after,
            due_at: now + after,
            failure_reasons,
        });
    }

    /// This export as a `ListExports` summary entry.
    pub fn summary(&self) -> ExportSummary {
        ExportSummary {
            export_id: Some(self.export_id.clone()),
            resource_specification: Some(self.resource_specification.clone()),
            file_format: Some(self.file_format.clone()),
            export_status: Some(self.export_status.clone()),
            creation_date_time: Some(to_date_time(self.creation_date_time)),
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
        }
    }

    /// The kind of resource this export is of, as the `ExportResourceType` filter of
    /// `ListExports` names it — read from the one specification the job carries, so the
    /// filter and the stored job cannot disagree about it.
    pub fn resource_type(&self) -> &'static str {
        let specification = &self.resource_specification;
        if specification.bot_export_specification.is_some() {
            EXPORT_RESOURCE_TYPE_BOT
        } else if specification.bot_locale_export_specification.is_some() {
            EXPORT_RESOURCE_TYPE_BOT_LOCALE
        } else if specification
            .custom_vocabulary_export_specification
            .is_some()
        {
            EXPORT_RESOURCE_TYPE_CUSTOM_VOCABULARY
        } else {
            EXPORT_RESOURCE_TYPE_TEST_SET
        }
    }

    /// The bot this export is of, or `None` for one of a resource that belongs to no bot.
    pub fn bot_id(&self) -> Option<&str> {
        let specification = &self.resource_specification;
        specification
            .bot_export_specification
            .as_ref()
            .map(|bot| bot.bot_id.as_str())
            .or_else(|| {
                specification
                    .bot_locale_export_specification
                    .as_ref()
                    .map(|locale| locale.bot_id.as_str())
            })
            .or_else(|| {
                specification
                    .custom_vocabulary_export_specification
                    .as_ref()
                    .map(|vocabulary| vocabulary.bot_id.as_str())
            })
    }

    /// The bot version this export is of, under the same contract as [`Export::bot_id`].
    pub fn bot_version(&self) -> Option<&str> {
        let specification = &self.resource_specification;
        specification
            .bot_export_specification
            .as_ref()
            .map(|bot| bot.bot_version.as_str())
            .or_else(|| {
                specification
                    .bot_locale_export_specification
                    .as_ref()
                    .map(|locale| locale.bot_version.as_str())
            })
            .or_else(|| {
                specification
                    .custom_vocabulary_export_specification
                    .as_ref()
                    .map(|vocabulary| vocabulary.bot_version.as_str())
            })
    }

    /// The locale this export is of, or `None` for an export of a whole bot or of a
    /// resource that belongs to no locale.
    pub fn locale_id(&self) -> Option<&str> {
        let specification = &self.resource_specification;
        specification
            .bot_locale_export_specification
            .as_ref()
            .map(|locale| locale.locale_id.as_str())
            .or_else(|| {
                specification
                    .custom_vocabulary_export_specification
                    .as_ref()
                    .map(|vocabulary| vocabulary.locale_id.as_str())
            })
    }

    /// The stored `failureReasons`, as the wire shape renders them: absent rather than an
    /// empty list while the job has reached no status that reports one.
    pub fn wire_failure_reasons(&self) -> Option<Vec<String>> {
        (!self.failure_reasons.is_empty()).then(|| self.failure_reasons.clone())
    }

    /// Is this export past the [`EXPORT_RETENTION`] the service keeps one for?
    pub fn expired(&self, now: Instant) -> bool {
        now >= self.kept_until
    }
}

impl ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Mint an unused `exportId`, from the same counter and in the same
    /// `[0-9a-zA-Z]{10}` space [`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::mint_bot_id`]
    /// draws from, and checked against the stored exports before it is handed out.
    pub fn mint_export_id(&mut self) -> String {
        loop {
            self.id_seq += 1;
            let candidate = render_id(self.id_seq);
            if !self.exports.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The stored export the id names, with any due status transition applied first — the
    /// same contract as [`ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State::settled_bot`].
    ///
    /// An export the service no longer keeps is not one of them: the drop was owed at the
    /// instant its window closed, whether or not anybody called, so a read past that
    /// instant reports the export as the one it does not hold.
    pub fn settled_export(&mut self, export_id: &str) -> Option<&Export> {
        self.drop_expired_exports();
        let export = self.exports.get_mut(export_id)?;
        export.settle();
        Some(export)
    }

    /// Apply every due export transition, so a listing reports the same status a
    /// per-export read would, and drop the exports the service no longer keeps.
    pub fn settle_exports(&mut self) {
        self.drop_expired_exports();
        for export in self.exports.values_mut() {
            export.settle();
        }
    }

    /// Drop every export past the window the service keeps one for
    /// ([`EXPORT_RETENTION`]) — the one write a read of this slice makes, because it was
    /// owed by the clock rather than by the call that noticed it. It is also what bounds
    /// this map: an export a caller submitted is held for that window and no longer.
    pub fn drop_expired_exports(&mut self) {
        let now = Instant::now();
        self.exports.retain(|_, export| !export.expired(now));
    }

    /// Drop the OLDEST jobs until the archives this store holds fit inside
    /// [`EXPORT_ARCHIVES_HELD_BYTES`], and answer how many went.
    ///
    /// Called by the create that records a job, so the bytes a caller can make this store
    /// hold are capped however many jobs it submits inside one retention window. A job
    /// dropped here is reported exactly as one whose retention window has closed — the read
    /// answers the resource it does not hold — so this bounds the host's memory without
    /// changing what any answer says.
    pub fn fit_export_archives(&mut self) -> usize {
        let mut held: usize = self
            .exports
            .values()
            .map(|export| export.archive.len())
            .sum();
        if held <= EXPORT_ARCHIVES_HELD_BYTES {
            return 0;
        }
        let mut by_age: Vec<(SystemTime, String)> = self
            .exports
            .values()
            .map(|export| (export.creation_date_time, export.export_id.clone()))
            .collect();
        by_age.sort();
        let mut dropped = 0;
        for (_, export_id) in by_age {
            if held <= EXPORT_ARCHIVES_HELD_BYTES {
                break;
            }
            if let Some(export) = self.exports.remove(&export_id) {
                held = held.saturating_sub(export.archive.len());
                dropped += 1;
            }
        }
        dropped
    }
}
