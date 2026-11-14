// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-simpledbv2-simpledbv2 service.
//!
//! The service's whole modelled surface is the **domain export** lifecycle:
//! `StartDomainExport` mints one export record, `GetExport` reports it by ARN and
//! `ListExports` enumerates the records. Every one of those three answers from the
//! single store below, so a write by one call is what the next call reads.
//!
//! The store also holds the **domain registry** a `domainName` is resolved against,
//! and the items those domains hold ([`simpledb_domains`]). A domain enters the
//! registry from the operator's declaration ([`EXISTING_DOMAINS_ENV`]), from a stored
//! export, and from a `CreateDomain` on the Amazon SimpleDB 2009-04-15 query API this
//! endpoint serves ([`crate::service::sdb_query_api`]); a name the registry does not
//! hold is the modelled `NoSuchDomainException`.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::ExportSummary;

/// The sentences this service refuses a request with, stated in ONE place so no two
/// handlers word the same refusal differently.
pub mod refusal_message;

/// The domains the account holds and the items inside them — the registry a
/// `domainName` resolves against, and the attribute store the 2009-04-15 query API
/// writes.
pub mod simpledb_domains;

use simpledb_domains::Domain;

/// The ARN namespace, endpoint prefix and sigv4 signing name of this service — all
/// three are `sdb`, from the model's `aws.api#service` / `aws.auth#sigv4` traits.
pub const ARN_SERVICE: &str = "sdb";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The domains the operator running this emulator declares this account already has,
/// which the export operations resolve a `domainName` against: comma-separated
/// `DomainName` values, read once when this service's store is created
/// (`SIMPLEDB_EXISTING_DOMAINS="orders,customers"`).
///
/// A SimpleDB domain is created by the SimpleDB 2009-04-15 `CreateDomain` and removed
/// by its `DeleteDomain`; this service's whole bound surface is the three export
/// operations — `GetExport`, `ListExports` and `StartDomainExport` — so NOTHING on
/// this wire creates a domain. An account nobody declared a domain for therefore has
/// none, which is exactly what `StartDomainExport` and a `ListExports` domain filter
/// answer the modelled `NoSuchDomainException` for. Nothing is invented: unset means
/// no domains at all.
pub const EXISTING_DOMAINS_ENV: &str = "SIMPLEDB_EXISTING_DOMAINS";

/// The domain names `setting` declares: comma-separated, surrounding space trimmed.
///
/// An entry this service could not export anyway — an empty one, or one
/// [`domain_name_rejection`] refuses — is SKIPPED rather than guessed at, so a typo
/// declares no domain instead of declaring an unintended one.
pub fn parse_existing_domains(setting: &str) -> Vec<String> {
    setting
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty() && domain_name_rejection(DEFAULT_REGION, name).is_none())
        .map(str::to_string)
        .collect()
}

/// The `ExportStatus` an export is created in: "export request received".
pub const STATUS_PENDING: &str = "PENDING";
/// The `ExportStatus` an accepted export moves through: "export is being processed".
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// The terminal `ExportStatus` of an export that ran: "export completed successfully".
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after `StartDomainExport` the clock owes the export its move out of
/// `PENDING` and into `IN_PROGRESS`.
///
/// `PENDING` and `IN_PROGRESS` are non-terminal statuses, so each carries a
/// transition the clock owes. Both spans here are the emulator's own choice (the
/// service publishes no duration for export processing) and both are deliberately
/// non-zero, so a caller reading straight after the start call still observes
/// `PENDING`, exactly as it does against the real service. Nothing sleeps: the due
/// instants are stored on the record ([`StoredExport::in_progress_due_at`] /
/// [`StoredExport::succeeded_due_at`]) and materialised by [`StoredExport::settle`].
pub const IN_PROGRESS_SPAN: Duration = Duration::from_millis(400);

/// How long after `StartDomainExport` the clock owes the export its move into the
/// terminal `SUCCEEDED`. See [`IN_PROGRESS_SPAN`].
pub const SUCCEEDED_SPAN: Duration = Duration::from_millis(700);

/// How long a `clientToken` keeps its idempotent meaning.
///
/// From the `clientToken` member's own documentation: "A client token is valid for 8
/// hours after the first request that uses it is completed. After 8 hours, any
/// request with the same client token is treated as a new request."
pub const CLIENT_TOKEN_VALIDITY: Duration = Duration::from_secs(8 * 60 * 60);

/// The most exports this emulator will hold.
///
/// The service publishes no number for its export quota — it models
/// `NumberExportsLimitExceeded` without naming the limit — so this is NOT a fidelity
/// claim about the published quota: it is this emulator's own ceiling, set well above
/// any realistic script, so a caller looping on `StartDomainExport` cannot grow this
/// process's memory without bound. A start past it is refused with that modelled
/// error.
pub const MAX_EXPORTS: usize = 1_000;

/// The largest page `ListExports` will ever build, and the page size it applies when
/// the request names no `maxResults`.
///
/// `maxResults` is modelled `@range(min: 1)` with no maximum, so the page size is
/// clamped here instead: the page this service allocates is decided by the server,
/// never by the request.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The `@length` bounds of the modelled `ExportArn` (20..2048), which every ARN this
/// service mints or accepts must satisfy.
pub const MIN_EXPORT_ARN_LEN: usize = 20;
/// See [`MIN_EXPORT_ARN_LEN`].
pub const MAX_EXPORT_ARN_LEN: usize = 2048;

/// The resource-path segment an export ARN carries before the exported domain name.
pub const DOMAIN_ARN_RESOURCE_PREFIX: &str = "domain/";
/// The resource-path element an export ARN carries before the export's own id.
pub const EXPORT_ARN_RESOURCE_KEYWORD: &str = "export";

/// File name of the manifest summary an export writes beside its data, which
/// `exportManifest` reports once the export has succeeded.
pub const MANIFEST_SUMMARY_FILE: &str = "manifest-summary.json";

/// One stored export: everything `StartDomainExport` was given, what the service
/// minted for it, and the instants the clock owes its status moves.
#[derive(Debug, Clone)]
pub struct StoredExport {
    pub export_arn: String,
    /// The export's own id — the last segment of [`Self::export_arn`].
    pub export_id: String,
    pub client_token: String,
    pub domain_name: String,
    /// An `ExportStatus` value.
    pub export_status: String,
    pub requested_at: SystemTime,
    pub s3_bucket: String,
    pub s3_key_prefix: Option<String>,
    pub s3_sse_algorithm: Option<String>,
    pub s3_sse_kms_key_id: Option<String>,
    pub s3_bucket_owner: Option<String>,
    /// Set when the export reaches `SUCCEEDED`.
    pub export_manifest: Option<String>,
    /// Set when the export reaches `SUCCEEDED`.
    pub items_count: Option<i64>,
    /// Set when the export reaches `SUCCEEDED`.
    pub export_data_cutoff_time: Option<SystemTime>,
    /// Set only when the export reaches `FAILED`, which nothing in this emulator
    /// drives an export to: no modelled operation can fail a running export, so the
    /// honest answer for both failure members is absent.
    pub failure_code: Option<String>,
    /// See [`Self::failure_code`].
    pub failure_message: Option<String>,
    /// Instant the move `PENDING` -> `IN_PROGRESS` comes due.
    pub in_progress_due_at: SystemTime,
    /// Instant the move `IN_PROGRESS` -> `SUCCEEDED` comes due.
    pub succeeded_due_at: SystemTime,
}

impl StoredExport {
    /// Materialise the status moves the clock already owes this export.
    ///
    /// These are the ONLY writes a reporting operation makes: each move was owed at
    /// this instant whether or not anybody called, so a `GetExport` that applies it
    /// reports what the record already held rather than advancing it because it was
    /// read.
    pub fn settle(&mut self, now: SystemTime, items_in_domain: i64) {
        if self.export_status == STATUS_PENDING && now >= self.in_progress_due_at {
            self.export_status = STATUS_IN_PROGRESS.to_string();
        }
        if self.export_status == STATUS_IN_PROGRESS && now >= self.succeeded_due_at {
            self.export_status = STATUS_SUCCEEDED.to_string();
            self.export_manifest = Some(self.manifest_key());
            // The number of SimpleDB items the exported domain holds, counted off that
            // domain's own record in this store (`items_in_domain`) at the instant the
            // export completed — the items a caller wrote with the 2009-04-15
            // `PutAttributes`, and zero for a domain nothing has written to. It is
            // captured HERE, where the export reaches its terminal status, so a later
            // write to the domain does not change what a finished export reports.
            self.items_count = Some(items_in_domain);
            // "All data inserted or modified before this time will be present in the
            // exported data": the cutoff is the instant the export finished, which is
            // when the clock made the move due, not when somebody looked.
            self.export_data_cutoff_time = Some(self.succeeded_due_at);
        }
    }

    /// The S3 key of this export's manifest summary — the key the export writes its
    /// manifest to: the stored `s3KeyPrefix` (empty when the start named none), this
    /// export's id, and the summary file name. Those are the only inputs the service
    /// has for it.
    ///
    /// It is the key the export WOULD write; this emulator stores no S3 object for it,
    /// so a caller that goes on to `HeadObject` that key against an S3 emulator will
    /// not find it.
    fn manifest_key(&self) -> String {
        format!(
            "{}{}/{MANIFEST_SUMMARY_FILE}",
            self.s3_key_prefix.as_deref().unwrap_or(""),
            self.export_id
        )
    }

    /// Whether this export was started with exactly the parameters now being asked
    /// for — the test an idempotent repeat must pass, and whose failure is the
    /// `ConflictException` the `clientToken` documentation describes ("the same client
    /// token but a change in other parameters ... a ConflictException will be
    /// returned").
    pub fn started_with(
        &self,
        domain_name: &str,
        s3_bucket: &str,
        s3_key_prefix: Option<&str>,
        s3_sse_algorithm: Option<&str>,
        s3_sse_kms_key_id: Option<&str>,
        s3_bucket_owner: Option<&str>,
    ) -> bool {
        self.domain_name == domain_name
            && self.s3_bucket == s3_bucket
            && self.s3_key_prefix.as_deref() == s3_key_prefix
            && self.s3_sse_algorithm.as_deref() == s3_sse_algorithm
            && self.s3_sse_kms_key_id.as_deref() == s3_sse_kms_key_id
            && self.s3_bucket_owner.as_deref() == s3_bucket_owner
    }

    /// The modelled `ExportSummary` for this record, as `ListExports` returns it.
    pub fn to_summary(&self) -> ExportSummary {
        ExportSummary {
            export_arn: self.export_arn.clone(),
            export_status: self.export_status.clone(),
            requested_at: ::aws_smithy_types::DateTime::from(self.requested_at),
            domain_name: self.domain_name.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-simpledbv2-simpledbv2. A handler reaches it through [`service_state`].
///
/// The indexes are kept private so they cannot drift apart: `exports` is the
/// authoritative record store, `export_order` is the insertion order `ListExports`
/// pages through, `client_tokens` is the idempotency index `StartDomainExport` looks a
/// repeat up in, and `domains` is the registry a `domainName` is resolved against.
#[derive(Debug)]
pub struct ComAmazonawsSimpledbv2Simpledbv2State {
    exports: HashMap<String, StoredExport>,
    export_order: Vec<String>,
    client_tokens: HashMap<String, String>,
    /// The SimpleDB domains this account holds, in name order, each with the items it
    /// holds ([`simpledb_domains`]).
    ///
    /// BOUNDED: [`Self::create_domain`] stores no more than
    /// [`simpledb_domains::MAX_DOMAINS`] of them, and each domain's own contents are
    /// bounded at [`simpledb_domains::MAX_DOMAIN_BYTES`], so neither the 2009-04-15 wire
    /// nor [`Self::insert_export`] can grow this map without end.
    domains: BTreeMap<String, Domain>,
    /// The secret this store's `ListDomains` continuation tokens are authenticated
    /// under, so no caller can name a position in the domain enumeration that this
    /// store did not hand it.
    page_token_key: ::service_common::page_tokens::PageTokenKey,
    /// Strictly increasing, and mixed into every minted id, so two ids minted in the
    /// same nanosecond still differ.
    id_counter: u64,
}

/// A fresh store: no exports, and no domains beyond the ones [`EXISTING_DOMAINS_ENV`]
/// declares (none, unless it is set).
///
/// `engine::InMemoryEngine::state` builds a service's state with `Default` the first
/// time a handler reaches it, so this is the ONE construction every store goes
/// through, and it is where the operator's declared domains are read and where this
/// store's page-token secret is drawn.
impl Default for ComAmazonawsSimpledbv2Simpledbv2State {
    fn default() -> Self {
        let mut state = Self {
            exports: HashMap::new(),
            export_order: Vec::new(),
            client_tokens: HashMap::new(),
            domains: BTreeMap::new(),
            page_token_key: ::service_common::page_tokens::PageTokenKey::new(),
            id_counter: 0,
        };
        for domain in ::std::env::var(EXISTING_DOMAINS_ENV)
            .map(|setting| parse_existing_domains(&setting))
            .unwrap_or_default()
        {
            // A declaration longer than the registry's ceiling is truncated at it rather
            // than growing the store past the bound this emulator holds itself to.
            let _ = state.create_domain(&domain);
        }
        state
    }
}

impl ComAmazonawsSimpledbv2Simpledbv2State {
    /// Record that this account holds a domain named `domain_name`, answering whether it
    /// holds one after the call.
    ///
    /// Reached from [`EXISTING_DOMAINS_ENV`] when the store is created, from
    /// [`Self::insert_export`], from the 2009-04-15 `CreateDomain`, and from a test
    /// standing up an account that already has domains. Creating a name the registry
    /// already holds changes nothing and keeps the items that name already has, which is
    /// what makes that `CreateDomain` idempotent.
    ///
    /// `false` means the account already holds [`simpledb_domains::MAX_DOMAINS`] domains
    /// and this NEW name was not stored; a handler maps that onto the published quota
    /// refusal. The ceiling is enforced here, where the growth bound is documented, so no
    /// caller of this store can drop it.
    pub fn create_domain(&mut self, domain_name: &str) -> bool {
        if self.domains.contains_key(domain_name) {
            return true;
        }
        if self.domains.len() >= simpledb_domains::MAX_DOMAINS {
            return false;
        }
        self.domains
            .insert(domain_name.to_string(), Domain::new(SystemTime::now()));
        true
    }

    /// Whether this account holds a domain named `domain_name`.
    ///
    /// This is the precondition `StartDomainExport` and a `ListExports` `domainName`
    /// filter resolve before they mint or enumerate anything, and whose failure is the
    /// modelled `NoSuchDomainException`.
    pub fn has_domain(&self, domain_name: &str) -> bool {
        self.domains.contains_key(domain_name)
    }

    /// Every domain this account holds, in name order.
    pub fn domains(&self) -> impl Iterator<Item = &str> + '_ {
        self.domains.keys().map(String::as_str)
    }

    /// A fresh identifier, rendered the way RFC 4122 renders a version-4 UUID
    /// (`8-4-4-4-12` lower-case hex, [`UUID_LEN`] characters) — the form the service's
    /// own smoke test shows an export id taking.
    ///
    /// The 128 bits are the leading half of a SHA-256 over this store's strictly
    /// increasing counter and the current nanosecond, with the version and variant
    /// nibbles set as RFC 4122 requires. Because the counter is part of the digest's
    /// input and advances on every call, no two ids this store mints are ever equal,
    /// whatever the clock does.
    pub fn fresh_id(&mut self) -> String {
        use ::sha2::Digest;
        self.id_counter = self.id_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0u128, |since| since.as_nanos());
        let mut hasher = ::sha2::Sha256::new();
        hasher.update(self.id_counter.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Version 4 in the high nibble of octet 6, and the RFC 4122 variant in the two
        // high bits of octet 8.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = ::hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// How many exports are stored.
    pub fn export_count(&self) -> usize {
        self.export_order.len()
    }

    /// Materialise every status move the clock already owes, so each operation
    /// reports the record as it stands at `now`.
    pub fn settle_all(&mut self, now: SystemTime) {
        // The two fields are borrowed disjointly: an export's terminal `itemsCount` is
        // the item count of the domain it exported, which lives on that domain's record.
        let domains = &self.domains;
        for export in self.exports.values_mut() {
            let items_in_domain = domains
                .get(&export.domain_name)
                .map_or(0, Domain::item_count);
            export.settle(now, i64::try_from(items_in_domain).unwrap_or(i64::MAX));
        }
    }

    /// The export `export_arn` names, or `None` when nothing was ever stored under it.
    pub fn export(&self, export_arn: &str) -> Option<&StoredExport> {
        self.exports.get(export_arn)
    }

    /// Every export in the order it was created — the order `ListExports` pages
    /// through, so a page token can name an offset into it.
    pub fn exports_in_order(&self) -> impl Iterator<Item = &StoredExport> + '_ {
        self.export_order
            .iter()
            .filter_map(|arn| self.exports.get(arn))
    }

    /// The export a `clientToken` already started, when that token is still inside
    /// its [`CLIENT_TOKEN_VALIDITY`] window. Past the window the token has no
    /// idempotent meaning, so this answers `None` and the caller treats the request
    /// as a new one.
    pub fn export_for_client_token(
        &self,
        client_token: &str,
        now: SystemTime,
    ) -> Option<&StoredExport> {
        let export = self
            .client_tokens
            .get(client_token)
            .and_then(|arn| self.exports.get(arn))?;
        // `checked_add` rather than `+`: the panicking impl would abort the handler on
        // a clock far enough in the future for the sum to be unrepresentable, and an
        // unrepresentable expiry is a token past its window, not a crash.
        export
            .requested_at
            .checked_add(CLIENT_TOKEN_VALIDITY)
            .is_some_and(|expires_at| now < expires_at)
            .then_some(export)
    }

    /// Store a newly minted export and index it by its client token, answering `false`
    /// when the store already holds [`MAX_EXPORTS`] of them and nothing was stored.
    ///
    /// The ceiling is enforced HERE, where the growth bound is documented, so no
    /// caller can drop it; a handler maps a `false` onto its own modelled quota error.
    /// A token whose window has lapsed is re-pointed at the new export, which is what
    /// "treated as a new request" means for the index.
    ///
    /// The export's domain is recorded in the registry at the same time: an export can
    /// only be started for a domain that exists, so a stored record is itself evidence
    /// of its domain, and that is what keeps `ListExports` from refusing a filter on a
    /// domain it reports summaries for.
    pub fn insert_export(&mut self, export: StoredExport) -> bool {
        if self.export_order.len() >= MAX_EXPORTS {
            return false;
        }
        // A domain the registry could not take (it already holds
        // `simpledb_domains::MAX_DOMAINS`) is not a reason to lose the export record:
        // the record itself is the evidence of its domain, and `StartDomainExport` only
        // ever stores one for a domain the registry already held.
        let _ = self.create_domain(&export.domain_name);
        self.client_tokens
            .insert(export.client_token.clone(), export.export_arn.clone());
        self.export_order.push(export.export_arn.clone());
        self.exports.insert(export.export_arn.clone(), export);
        true
    }
}

/// Whether `candidate` is a well-formed ARN naming an export of this service.
///
/// Six colon-separated ARN fields, the `sdb` service namespace, and a resource path
/// whose last path element is an `export/<id>` pair carrying a non-empty id. Both
/// resource forms the model's own documents use are admitted — the plain
/// `export/<id>` of the `GetExport` example and the
/// `domain/<domainName>/export/<id>` of its smoke test — and the partition, region
/// and account fields are not required to be this emulator's own, so an ARN naming
/// another account is well-formed but simply names no stored export.
pub fn is_export_arn(candidate: &str) -> bool {
    if !(MIN_EXPORT_ARN_LEN..=MAX_EXPORT_ARN_LEN).contains(&candidate.len()) {
        return false;
    }
    let mut fields = candidate.splitn(6, ':');
    let (Some(prefix), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    if prefix != "arn" || partition.is_empty() || service != ARN_SERVICE {
        return false;
    }
    // The `export` keyword must be a whole path element, not a suffix of one, so a
    // resource like `notexport/abc` is not an export ARN.
    let mut elements = resource.split('/');
    let Some(id) = elements.next_back() else {
        return false;
    };
    let Some(keyword) = elements.next_back() else {
        return false;
    };
    keyword == EXPORT_ARN_RESOURCE_KEYWORD && !id.is_empty()
}

/// Why a `domainName` cannot be exported — the `message` of the refusal it earns — or
/// `None` when it can.
///
/// `DomainName` carries only `@length(min: 1)`, so the centralized walk admits any
/// text; the rule enforced here is the one the minted ARN imposes. An `exportArn` is
/// a required output whose `:`-separated fields and `/`-separated resource path are
/// what identifies the export, so a domain name carrying `/`, `:` or an ASCII control
/// character — which would make the minted ARN name a different resource, or none —
/// cannot be exported, and nor can one long enough to push the ARN past
/// [`MAX_EXPORT_ARN_LEN`].
///
/// Both refusals are stated as the service states an invalid parameter
/// ([`refusal_message::invalid_parameter_value`]), which names the parameter and its
/// requirement and echoes nothing the caller sent — not even the offending character.
pub fn domain_name_rejection(region: &str, domain_name: &str) -> Option<String> {
    if domain_name
        .chars()
        .any(|c| c == '/' || c == ':' || c.is_control())
    {
        return Some(refusal_message::invalid_parameter_value(
            DOMAIN_NAME_MEMBER,
            DOMAIN_NAME_CHARACTER_REQUIREMENT,
        ));
    }
    // The id segment is a hyphenated UUID, whose length is fixed, so the ARN's length
    // is known before one is minted.
    let arn_len = export_arn(region, domain_name, &"0".repeat(UUID_LEN)).len();
    (arn_len > MAX_EXPORT_ARN_LEN).then(|| {
        refusal_message::invalid_parameter_value(
            DOMAIN_NAME_MEMBER,
            &format!(
                "be short enough for the export ARN naming it to stay within the \
                 {MAX_EXPORT_ARN_LEN} characters an exportArn may carry"
            ),
        )
    })
}

/// The modelled member a [`domain_name_rejection`] faults, as the model names it.
const DOMAIN_NAME_MEMBER: &str = "domainName";

/// The requirement a `domainName` has to meet for the minted `exportArn` to name the
/// export and nothing else. Stated as the characters it may not carry rather than as
/// the one the request carried.
const DOMAIN_NAME_CHARACTER_REQUIREMENT: &str =
    "be free of '/', ':' and ASCII control characters, each of which would make the \
     exportArn this service mints name a different export resource";

/// Characters in the hyphenated RFC 4122 rendering of an identifier, which is what
/// an export id and a generated client token are.
pub const UUID_LEN: usize = 36;

/// The ARN of the export `export_id` of `domain_name` in `region`.
///
/// The resource path is the form the service's own smoke test uses:
/// `domain/<domainName>/export/<id>`.
pub fn export_arn(region: &str, domain_name: &str, export_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{DOMAIN_ARN_RESOURCE_PREFIX}{domain_name}/{EXPORT_ARN_RESOURCE_KEYWORD}/{export_id}"
        ),
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside a generated ARN, so
/// anything that is not a plausible region label is refused here and the default used
/// instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSimpledbv2Simpledbv2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSimpledbv2Simpledbv2State>)
}

/// Encode a `ListExports` page token naming the offset the next page starts at and
/// the `domainName` filter the page was built under.
///
/// `nextToken` is modelled opaque, so the pair is hex-encoded rather than handed back
/// in the clear — opaque to a caller, not secret. The filter is carried as a short
/// digest rather than verbatim: `domainName` has no modelled maximum length, so the
/// token stays well inside the member's own 1..2048 bound whatever the filter was.
pub fn encode_page_token(offset: usize, domain_filter: Option<&str>) -> String {
    ::hex::encode(format!(
        "{offset}.{}",
        page_token_filter_digest(domain_filter)
    ))
}

/// The offset a page token names, or `None` when the token is not one this service
/// issued, names a position a store this size does not have, or was issued under a
/// different `domainName` filter.
///
/// Continuing one enumeration with another's cursor would hand the caller a page of a
/// stream it never started, so the token is refused rather than honoured.
pub fn decode_page_token(
    token: &str,
    export_count: usize,
    domain_filter: Option<&str>,
) -> Option<usize> {
    let text = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let (offset, digest) = text.split_once('.')?;
    if digest != page_token_filter_digest(domain_filter) {
        return None;
    }
    offset
        .parse::<usize>()
        .ok()
        .filter(|offset| *offset > 0 && *offset <= export_count)
}

/// Characters of the filter digest a page token carries: 4 bytes of SHA-256, which is
/// plenty to tell one filter from another in a token that is never a security
/// boundary.
const PAGE_TOKEN_DIGEST_LEN: usize = 8;

/// The digest identifying the `domainName` filter a page was enumerated under.
///
/// An absent filter and a filter are distinguished by the prefix, so no filter value
/// can collide with "no filter at all".
fn page_token_filter_digest(domain_filter: Option<&str>) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    match domain_filter {
        Some(domain) => {
            hasher.update(b"domain:");
            hasher.update(domain.as_bytes());
        }
        None => hasher.update(b"all"),
    }
    ::hex::encode(hasher.finalize())[..PAGE_TOKEN_DIGEST_LEN].to_string()
}
