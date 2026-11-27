// SPDX-License-Identifier: CC0-1.0

//! Consuming the CSV file a user import job was handed: the columns the import
//! format carries beside a pool's own attributes, and the rule that turns one
//! uploaded file into users of the pool.
//!
//! `StartUserImportJob` "instructs your user pool to start importing users from a
//! CSV file that contains their usernames and attributes" (this service's model),
//! and that file is the one a client wrote to the job's `PreSignedUrl`. So a start
//! READS the uploaded file and places one user per data row it can place, and the
//! job then reports what that run did: "the number of users that were
//! successfully imported", "the number of users that were skipped" and "the
//! number of users that couldn't be imported" (`UserImportJobType.ImportedUsers`,
//! `SkippedUsers` and `FailedUsers`).
//!
//! The file is read with the CSV reader this workspace already parses delimited
//! text with (the `csv` crate), so a quoted field, an embedded separator and a
//! trailing newline are read the way every other reader of that format reads
//! them rather than by a split written here.
//!
//! The run is BOUNDED and INTERRUPTIBLE, because both the file and the pool are
//! the caller's to grow: the records one run reads are capped at
//! [`MAX_USER_IMPORT_CSV_ROWS`] (an import cannot place more users than the pool
//! can hold), the file is parsed with no store lock held, and the rows are placed
//! in passes of [`IMPORT_ROWS_PER_PASS`] that release this service's store and
//! hand the executor back between them, so one upload neither blocks the async
//! worker nor holds the lock every other request of this service waits on.

use super::*;

/// This service's store, as [`service_state`] hands it back: what an import locks
/// for one pass and releases again.
pub type ServiceStore = ::engine::ServiceState<
    ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState,
>;

/// How many data records one import READS out of the uploaded file.
///
/// An import creates users of a pool, and a pool holds at most
/// [`MAX_USERS_PER_POOL`] of them, so a file carrying more records than that
/// cannot place more users than the pool can hold: the run reads this many and
/// stops. That bounds the run's work on the PRODUCT of the file and the store
/// rather than on the uploaded bytes alone.
pub const MAX_USER_IMPORT_CSV_ROWS: usize = MAX_USERS_PER_POOL;

/// How many rows one pass of the import places before it releases this service's
/// store and hands the executor back ([`run_user_import`]).
const IMPORT_ROWS_PER_PASS: usize = 64;

/// The column a CSV row names the user it imports by: "the username to create the
/// user under" (Amazon Cognito Developer Guide, "Importing users from a CSV
/// file"), which is why a file whose header carries no such column names no user
/// at all.
pub const CSV_USERNAME_COLUMN: &str = "cognito:username";

/// The column a CSV row states the imported user's MFA state in.
pub const CSV_MFA_ENABLED_COLUMN: &str = "cognito:mfa_enabled";

/// The column a CSV row carries the user's password as a HASH in — the hash made
/// with "the password hashing algorithm used to generate the hashes in the CSV
/// file for this import job" (`CreateUserImportJob.PasswordHashingAlgorithm` in
/// this service's model).
pub const CSV_PASSWORD_HASH_COLUMN: &str = "password_hash";

/// The status an imported user is created in.
///
/// "`RESET_REQUIRED` - The user is confirmed, but the user must request a code
/// and reset their password before they can sign in" (`UserStatusType` in this
/// service's model): a CSV row carries no password Amazon Cognito can verify — a
/// `password_hash` is a digest made with the job's own algorithm, not a password —
/// so an imported user owes a reset before its first sign-in.
pub const IMPORTED_USER_STATUS: UserStatus = UserStatus::ResetRequired;

/// What one run of a job's import did: the three counters the job reports, and
/// whether the file's header was one this pool's import could read at all.
///
/// The default is the outcome of a run that READ the file and placed no row,
/// which is what a job carrying an empty file earns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    /// "The number of users that were successfully imported"
    /// (`UserImportJobType.ImportedUsers`).
    pub imported_users: i64,
    /// "The number of users that were skipped" (`UserImportJobType.SkippedUsers`):
    /// a row naming a user the pool already holds.
    pub skipped_users: i64,
    /// "The number of users that couldn't be imported"
    /// (`UserImportJobType.FailedUsers`): a row the import format or the pool's
    /// schema rejects.
    pub failed_users: i64,
    /// Set when the file's header row is not a header of THIS pool's import
    /// template, so no row of it could be placed: the job then "has stopped due to
    /// an error" rather than having "completed successfully"
    /// (`UserImportJobType.Status`).
    pub header_unreadable: bool,
}

impl ImportOutcome {
    /// The TERMINAL status this run earns the job: "`Failed` - The job has stopped
    /// due to an error" for a file whose header this pool's import cannot read,
    /// and "`Succeeded` - The job has completed successfully" for a run that read
    /// the file, whatever its individual rows then earned
    /// (`UserImportJobType.Status`).
    #[must_use]
    pub fn terminal_status(&self) -> &'static str {
        if self.header_unreadable {
            JOB_STATUS_FAILED
        } else {
            JOB_STATUS_SUCCEEDED
        }
    }

    /// "The message returned when the user import job is completed"
    /// (`UserImportJobType.CompletionMessage`).
    ///
    /// `CompletionMessageType` is modelled with length 1..128 and the pattern
    /// `^[\w]+$`, so the message is ONE word of word characters rather than a
    /// sentence: it names the result of the run, and the per-row detail of an
    /// import belongs to the CloudWatch Logs role the job names
    /// (`UserImportJobType.CloudWatchLogsRoleArn`).
    #[must_use]
    pub fn completion_message(&self) -> &'static str {
        self.terminal_status()
    }
}

/// Why a start of a user import job cannot run, which the operation states in its
/// own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRefusal {
    /// The request named no stored user pool.
    PoolNotFound,
    /// The named pool holds no such import job.
    JobNotFound,
    /// The job is not waiting to be started: it is in this status.
    NotWaiting(&'static str),
    /// No CSV file is waiting at the job's upload target.
    NoCsvUploaded,
}

/// The file one accepted start will read, taken out of the job by the same lock
/// that recorded the start.
#[derive(Debug)]
pub struct AcceptedImport {
    /// The pool's own import template, read while the pool was locked, which the
    /// file's header row is judged against.
    template: Vec<String>,
    /// The file itself, which the job no longer holds.
    csv: Vec<u8>,
}

/// Accept the start of the user import job `job_id` of `user_pool_id` at `now`,
/// under ONE short lock: check the preconditions a start has to meet, take the
/// uploaded file out of the job and record the start ([`UserImportJob::start`]),
/// so the job is `Pending` — and no longer startable — before the import runs.
///
/// The file leaves the job here because the import reads it once and nothing reads
/// it again, so a job does not hold its upload for the rest of its life.
/// [`run_user_import`] then places its rows.
pub fn begin_user_import(
    state: &ServiceStore,
    user_pool_id: &str,
    job_id: &str,
    now: SystemTime,
) -> Result<AcceptedImport, ImportRefusal> {
    let mut guard = state.lock();
    let pool = guard
        .pool_mut(user_pool_id)
        .ok_or(ImportRefusal::PoolNotFound)?;
    // The pool's own template, taken before the job is borrowed.
    let template = pool.csv_header();
    let job = pool
        .user_import_jobs
        .get_mut(job_id)
        .ok_or(ImportRefusal::JobNotFound)?;
    // "The ID of a user import job that you previously created"
    // (`StartUserImportJob.JobId`): only a job still waiting to be started can be
    // started, so one already started, stopped or expired keeps the status and the
    // dates it has.
    let status = job.status(now);
    if status != JOB_STATUS_CREATED {
        return Err(ImportRefusal::NotWaiting(status));
    }
    // "Instructs your user pool to start importing users from a CSV file"
    // (`StartUserImportJob`), and the file is the one uploaded to the job's
    // `PreSignedUrl` — so a job whose upload target was never written to has
    // nothing to import. Asked before the status move is stored, so a refused start
    // leaves the job `Created` and startable once its CSV arrives.
    if !job.has_uploaded_csv() {
        return Err(ImportRefusal::NoCsvUploaded);
    }
    let csv = ::std::mem::take(&mut job.csv);
    job.start(now);
    Ok(AcceptedImport { template, csv })
}

/// Run the import `accepted` into the pool `user_pool_id` and record what it did
/// on the job `job_id`, answering the stored job as every read of it renders it.
///
/// The file is PARSED with no store lock held, and its rows are placed in passes of
/// [`IMPORT_ROWS_PER_PASS`], each taking and releasing the store and handing the
/// executor back after it — so neither the async worker nor this service's lock is
/// held for the whole run. `None` only for a pool or a job that is gone by the time
/// the run finishes.
pub async fn run_user_import(
    state: &ServiceStore,
    user_pool_id: &str,
    job_id: &str,
    accepted: AcceptedImport,
    now: SystemTime,
) -> Option<UserImportJobType> {
    let AcceptedImport { template, csv } = accepted;
    let ParsedImport { columns, rows } = parse_import_file(&template, &csv);
    let mut outcome = ImportOutcome::default();
    match columns {
        // The header is not this pool's, so no row of the file can be placed —
        // and each record behind it is a user that could not be imported.
        None => {
            outcome.header_unreadable = true;
            outcome.failed_users = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        }
        Some(columns) => {
            for pass in rows.chunks(IMPORT_ROWS_PER_PASS) {
                {
                    let mut guard = state.lock();
                    let pool = guard.pool_mut(user_pool_id)?;
                    for row in pass {
                        match place_row(pool, &columns, row, now) {
                            RowResult::Imported => {
                                outcome.imported_users = outcome.imported_users.saturating_add(1);
                            }
                            RowResult::Skipped => {
                                outcome.skipped_users = outcome.skipped_users.saturating_add(1);
                            }
                            RowResult::Failed => {
                                outcome.failed_users = outcome.failed_users.saturating_add(1);
                            }
                        }
                    }
                }
                // The store is released above, and the executor is handed back
                // here, so a long import does not starve this service.
                ::tokio::task::yield_now().await;
            }
        }
    }
    let mut guard = state.lock();
    let job = guard
        .pool_mut(user_pool_id)?
        .user_import_jobs
        .get_mut(job_id)?;
    job.record_import(outcome);
    Some(job.to_user_import_job_type(now))
}

/// The file one run will place: the columns its header row names — `None` when the
/// header is not one this pool's import can read — and its data records, at most
/// [`MAX_USER_IMPORT_CSV_ROWS`] of them.
struct ParsedImport {
    columns: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
}

/// Read `csv` against the pool's import `template`, with no store lock held.
///
/// The rule, which holds for every file a job is handed rather than for one shape
/// of file:
///
/// * the first record is the HEADER row, and the columns it names have to be
///   columns of the pool's own import template ([`UserPool::csv_header`]) with
///   [`CSV_USERNAME_COLUMN`] among them — "this list is the header for the CSV
///   file that determines the users in a user import job" (`GetCSVHeader` in this
///   service's model). A header naming a column the template does not carry, a
///   column twice, or no username column at all is one this pool cannot import
///   from, and so is a file carrying no record at all;
/// * every later record is one user, read as the values its columns carry, and the
///   records are capped at [`MAX_USER_IMPORT_CSV_ROWS`].
fn parse_import_file(template: &[String], csv: &[u8]) -> ParsedImport {
    let mut reader = ::csv::ReaderBuilder::new()
        // The header row is read as a record of its own, so the columns it names
        // are checked against the pool's template before any row is placed.
        .has_headers(false)
        // A row carrying fewer or more fields than the header is a row of its own
        // to account for rather than a read error that stops the file.
        .flexible(true)
        .from_reader(csv);
    let mut records = reader.records();
    let Some(header) = records.next().and_then(Result::ok) else {
        // No record at all: such a file carries no header row, so it is not one
        // this pool's import can read.
        return ParsedImport {
            columns: None,
            rows: Vec::new(),
        };
    };
    let rows: Vec<Vec<String>> = records
        .take(MAX_USER_IMPORT_CSV_ROWS)
        .filter_map(Result::ok)
        .map(|record| record.iter().map(str::to_string).collect())
        .collect();
    ParsedImport {
        columns: import_columns(template, &header),
        rows,
    }
}

/// What one data row of the file earned.
enum RowResult {
    /// The row became a user of the pool.
    Imported,
    /// The pool already holds the user the row names.
    Skipped,
    /// The row is not one this pool can place.
    Failed,
}

/// The columns the header row names, in the order the file carries them, or `None`
/// when the header is not one this pool's import can read (see
/// [`parse_import_file`]).
fn import_columns(template: &[String], header: &::csv::StringRecord) -> Option<Vec<String>> {
    let known: BTreeSet<&str> = template.iter().map(String::as_str).collect();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut columns: Vec<String> = Vec::with_capacity(header.len());
    for column in header.iter() {
        let column = column.trim();
        if !known.contains(column) || !seen.insert(column) {
            return None;
        }
        columns.push(column.to_string());
    }
    // A header that names no username column names no user to create, whatever
    // else it carries.
    seen.contains(CSV_USERNAME_COLUMN).then_some(columns)
}

/// Place one data row of the file as a user of `pool`, and report what it earned.
///
/// * the value under [`CSV_USERNAME_COLUMN`] is the name to create the user under,
///   and the value under each attribute column is stored as that attribute, blank
///   values storing nothing ([`stored_text`]) and a column whose attribute the
///   pool's schema declares a boolean being read as the boolean its text states
///   ([`import_attribute_value`]);
/// * a row the format or the schema rejects — no username, a non-boolean where the
///   format carries a boolean, a value no attribute of the pool can hold
///   ([`attribute_value_defect`]), or a row arriving once the pool holds all the
///   users it can — is FAILED and nothing is stored for it;
/// * a row naming a user the pool already holds, by any of the names that resolve
///   one ([`UserPool::user_storage_key`]), is SKIPPED: an import creates users, it
///   does not overwrite them;
/// * every other row is IMPORTED, as a user in [`IMPORTED_USER_STATUS`].
fn place_row(
    pool: &mut UserPool,
    columns: &[String],
    row: &[String],
    now: SystemTime,
) -> RowResult {
    let value_of = |wanted: &str| -> &str {
        columns
            .iter()
            .position(|column| column == wanted)
            .and_then(|at| row.get(at))
            .map_or("", |value| value.trim())
    };
    // "The username to create the user under": a row that names none names no
    // user, so there is nothing to create.
    let submitted = value_of(CSV_USERNAME_COLUMN);
    if submitted.is_empty() {
        return RowResult::Failed;
    }
    // The MFA column carries a boolean, so a value that is neither is a row this
    // import cannot read.
    let mfa_enabled = match boolean_column(value_of(CSV_MFA_ENABLED_COLUMN)) {
        Some(value) => value,
        None => return RowResult::Failed,
    };
    // The row's attribute values, blank values storing nothing and every value
    // judged against the form its attribute's values take — the same question
    // every other writer of a user attribute asks
    // ([`attribute_value_defect`]).
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    for (at, column) in columns.iter().enumerate() {
        if is_import_format_column(column) {
            continue;
        }
        let Some(value) = stored_text(row.get(at).map(|value| value.trim().to_string())) else {
            continue;
        };
        // A column whose attribute the pool's schema declares a BOOLEAN carries
        // a boolean, in the CSV format's own spelling of one rather than in the
        // spelling this service stores it as ([`boolean_column`]) — so it is
        // read as the boolean it states and stored canonically, and a value that
        // states no boolean is a row this import cannot read.
        let Some(value) = import_attribute_value(pool, column, value) else {
            return RowResult::Failed;
        };
        if attribute_value_defect(column, &value).is_some() {
            return RowResult::Failed;
        }
        // A value that misses a constraint the pool's schema declares for its
        // attribute is a value no user of the pool can hold either
        // ([`satisfies_schema_constraints`]), so the row it arrived in stores
        // nothing.
        if !satisfies_schema_constraints(pool, column, &value) {
            return RowResult::Failed;
        }
        attributes.insert(column.clone(), value);
    }
    // This emulator holds a pool's users in memory, so the store is bounded: a row
    // arriving once the pool holds all the users it can is a user that could not be
    // imported. Asked BEFORE the name is resolved, so such a row costs no lookup.
    if pool.users.len() >= MAX_USERS_PER_POOL {
        return RowResult::Failed;
    }
    // An import creates users rather than overwriting them, so a name the pool
    // already answers to is skipped.
    if pool.user_storage_key(submitted).is_some() {
        return RowResult::Skipped;
    }
    let sub = new_uuid();
    // "If your user pool only supports phone numbers or email addresses as
    // sign-in attributes, Amazon Cognito automatically generates a username
    // value" (`AdminCreateUser.Username`), so in such a pool the row's name is the
    // value of that attribute rather than a username — the same reading every
    // other path that creates a user from a submitted name takes
    // ([`UserPool::sign_in_attribute`]).
    let sign_in_attribute = pool.sign_in_attribute(submitted).map(str::to_string);
    let (username, key) = match &sign_in_attribute {
        Some(_) => (sub.clone(), sub.clone()),
        None => {
            let key = pool.user_key(submitted);
            (key.clone(), key)
        }
    };
    // `sub` is the user's own id, not a value the file may set.
    attributes.insert(SUB_ATTRIBUTE.to_string(), sub.clone());
    if let Some(attribute) = sign_in_attribute {
        attributes
            .entry(attribute)
            .or_insert_with(|| submitted.to_string());
    }
    // An attribute whose confirmation is recorded by a flag the file did not
    // supply goes unconfirmed, exactly as it does for a user any other path
    // creates ([`verified_flag`]).
    let unflagged: Vec<&'static str> = attributes.keys().filter_map(|name| verified_flag(name)).collect();
    for flag in unflagged {
        attributes
            .entry(flag.to_string())
            .or_insert_with(|| FLAG_UNCONFIRMED.to_string());
    }
    let mut user = User {
        username,
        sub,
        attributes,
        // Nothing is pending for a user the import just created: the attributes
        // above are its values, not values awaiting a confirmation.
        pending_attributes: BTreeMap::new(),
        // A `password_hash` is a digest made with the job's own hashing
        // algorithm, not a password this emulator could verify, so no password is
        // stored for an imported user and the status below is what makes it reset
        // one before it signs in.
        password: None,
        password_permanent: false,
        password_set_at: None,
        password_history: Vec::new(),
        status: IMPORTED_USER_STATUS,
        enabled: true,
        mfa_options: Vec::new(),
        preferred_mfa_setting: None,
        mfa_setting_list: Vec::new(),
        user_create_date: now,
        user_last_modified_date: now,
        groups: BTreeSet::new(),
        // An imported user has no remembered device, no outstanding code, no
        // passkey, no linked external identity, no registered authenticator and no
        // event history: each exists only once something of this pool has created
        // one.
        devices: BTreeMap::new(),
        outstanding_codes: BTreeMap::new(),
        web_authn_credentials: Vec::new(),
        provider_links: BTreeMap::new(),
        software_token_registered: false,
        software_token_spent_step: None,
        software_token_key: None,
        web_authn_mfa_enabled: None,
        auth_events: Vec::new(),
    };
    if mfa_enabled {
        // The imported user's MFA is the one option an `MFAOptionType` can
        // carry — SMS to the user's phone number ([`sms_mfa_option`]) — stored
        // the way every other writer of that option stores it
        // ([`User::set_sms_mfa_options`]).
        user.set_sms_mfa_options(vec![sms_mfa_option()], now);
    }
    pool.users.insert(key, user);
    RowResult::Imported
}

/// Whether `column` is a column the import FORMAT adds rather than an attribute
/// of the pool's schema, so its value is not stored as an attribute.
fn is_import_format_column(column: &str) -> bool {
    column == CSV_USERNAME_COLUMN
        || column == CSV_MFA_ENABLED_COLUMN
        || column == CSV_PASSWORD_HASH_COLUMN
}

/// The value one data row stores for the attribute column `column`, or `None`
/// when the row carries no value that attribute can hold.
///
/// A CSV carries every value as TEXT, so a column whose attribute the pool's
/// schema declares a boolean ([`declares_boolean_attribute`]) carries a boolean
/// SPELLED the way the import format spells one, which is not necessarily the
/// spelling this service stores it as: the value is read as the boolean it
/// states ([`boolean_column`]) and stored as the one spelling the pool's
/// booleans take ([`stored_boolean`]), so an imported user's flags are the same
/// values every other writer of them stores and every reader of them compares
/// against. Text that states no boolean states no value such an attribute can
/// hold at all.
///
/// Every other attribute holds the text the row carried.
fn import_attribute_value(pool: &UserPool, column: &str, value: String) -> Option<String> {
    if !declares_boolean_attribute(pool, column) {
        return Some(value);
    }
    Some(stored_boolean(boolean_column(&value)?).to_string())
}

/// The boolean `value` states, `Some(false)` for a blank value (a column the row
/// left empty states nothing) and `None` for a value that is no boolean at all.
///
/// The CSV import format states a boolean column's value in either case — the
/// import template's own documented values are upper case (Amazon Cognito
/// Developer Guide, "Importing users from a CSV file"), while this service
/// stores a boolean attribute as [`FLAG_CONFIRMED`] or [`FLAG_UNCONFIRMED`] — so
/// the two spellings are compared without case, and this is the ONE reading of a
/// boolean column every column of the import that carries one is read through.
fn boolean_column(value: &str) -> Option<bool> {
    if value.is_empty() {
        return Some(false);
    }
    if value.eq_ignore_ascii_case(FLAG_CONFIRMED) {
        return Some(true);
    }
    value.eq_ignore_ascii_case(FLAG_UNCONFIRMED).then_some(false)
}
