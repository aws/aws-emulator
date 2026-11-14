// SPDX-License-Identifier: CC0-1.0

//! The domain file an `ImportFirewallDomains` request names, read out of the
//! ONE object store this emulator has.
//!
//! `ImportFirewallDomains$DomainFileUrl` is "the fully qualified URL or URI of
//! the file stored in Amazon Simple Storage Service (Amazon S3) that contains
//! the list of domains to import. The file must be in an S3 bucket that's in
//! the same Region as your DNS Firewall. The file must be a text file and must
//! contain a single domain per line." So the file an import reads is an object
//! of the account's own Amazon S3 inventory, which lives in THAT service's
//! store and is read through its own accessors — a URL addressing no object
//! there is a file DNS Firewall cannot download, which is what the import then
//! reports.

use super::*;

/// The most bytes an imported domain file may hold.
///
/// A domain file is read whole into this process, so an unbounded one would
/// grow it without limit. The bound is [`MAX_DOMAINS_PER_LIST`] lines of the
/// longest domain the model's `FirewallDomainName` admits (255 characters) plus
/// its newline, so every file whose domains a list can hold is within it, and a
/// larger one is a file this import cannot take — which is the outcome the
/// service's own download failure states ("please try with a smaller file",
/// [`firewall_domain_list_import_download_failed`]).
pub const MAX_DOMAIN_FILE_BYTES: usize = MAX_DOMAINS_PER_LIST * 256;

/// The most characters a domain name an import writes may hold: the model's own
/// `FirewallDomainName` length (1..255)
/// (`smithy_query(query="shape",shape="FirewallDomainName")`), which the
/// framework enforces on the domains a request carries but cannot enforce on
/// the lines of a file.
pub const MAX_FIREWALL_DOMAIN_CHARS: usize = 255;

/// What the file a `DomainFileUrl` addresses holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallDomainFile {
    /// The domains the file lists, one per line, canonicalized
    /// ([`firewall_domain_canonical`]) and holding each domain once.
    Domains(Vec<String>),
    /// The URL addresses no file this emulator can download: it addresses no
    /// object of the account's inventory, or the object there is not a text
    /// file, or it is larger than [`MAX_DOMAIN_FILE_BYTES`].
    Undownloadable,
    /// The file was read, and a line of it is not a domain specification. The
    /// line is carried so the refusal can name it.
    NotDomains(String),
}

/// The file `url` addresses, read from the ONE object store this emulator has.
///
/// A pure read of that store: the content is what a `GetObject` of the same URL
/// would answer with — the CURRENT object at the key, so a key whose object was
/// deleted again reads as undownloadable.
///
/// LOCK ORDER: this takes the object store's lock and holds no other while it
/// does, and hands back an owned value rather than a guard — so it must be
/// called with NO guard of this service's store held. Keeping the cross-service
/// access one-directional and guard-free is what makes a lock cycle between the
/// two stores unreachable.
pub fn firewall_domain_file_at<Eng: Send + Sync + 'static>(
    engine: &Eng,
    url: &str,
) -> FirewallDomainFile {
    let Some((bucket, key)) = s3_object_address(url) else {
        return FirewallDomainFile::Undownloadable;
    };
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return FirewallDomainFile::Undownloadable;
    };
    let body = {
        let store = state.lock();
        let Some(object) = store.bucket(bucket).and_then(|bucket| bucket.object(key)) else {
            return FirewallDomainFile::Undownloadable;
        };
        if object.body.len() > MAX_DOMAIN_FILE_BYTES {
            return FirewallDomainFile::Undownloadable;
        }
        object.body.clone()
    };
    let Ok(text) = ::std::str::from_utf8(&body) else {
        // "The file must be a text file": bytes that are not text are not one.
        return FirewallDomainFile::Undownloadable;
    };
    domains_of(text)
}

/// The domains a downloaded domain file lists: "a single domain per line".
///
/// A line is read without its surrounding whitespace, a blank line lists no
/// domain, and a line that is not a domain specification makes the file not a
/// domain list. The domains are canonicalized and de-duplicated exactly as the
/// ones an update writes are, so a list holds the same entries however they
/// reached it.
fn domains_of(text: &str) -> FirewallDomainFile {
    let mut domains: Vec<String> = Vec::new();
    let mut keys: HashSet<String> = HashSet::new();
    for line in text.lines() {
        let domain = line.trim();
        if domain.is_empty() {
            continue;
        }
        if !firewall_domain_is_wellformed(domain) || domain.len() > MAX_FIREWALL_DOMAIN_CHARS {
            return FirewallDomainFile::NotDomains(domain.to_string());
        }
        let canonical = firewall_domain_canonical(domain);
        if keys.insert(canonical.clone()) {
            domains.push(canonical);
        }
    }
    FirewallDomainFile::Domains(domains)
}

/// Whether `url` addresses an object of Amazon S3 at all: one of the
/// addressing forms [`s3_object_address`] reads.
///
/// This is the FORM of the value, decided without looking at the account's
/// inventory, and it is what tells the two outcomes of an import apart: a value
/// that is not an S3 address is a request DNS Firewall will not start an import
/// for at all ([`invalid_domain_file_url`]), while an S3 address is accepted and
/// the import then either reads the object or terminates in the failure a file
/// it could not download leaves on the list
/// ([`firewall_domain_list_import_download_failed`]).
pub fn is_s3_object_url(url: &str) -> bool {
    s3_object_address(url).is_some()
}

/// The Route 53 Resolver service error code reported when `DomainFileUrl` is
/// not an Amazon S3 address.
///
/// Carried for the same reason as [`RSLVR_CODE_RESOURCE_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded refusal of an import whose URL was not an S3 one. See
/// [`invalid_domain_file_url`].
pub const RSLVR_CODE_INVALID_DOMAIN_FILE_URL: &str = "RSLVR-02038";

/// The `ValidationException` an import is refused with when the URL it names is
/// not an Amazon S3 address.
///
/// `ImportFirewallDomains$DomainFileUrl` is "the fully qualified URL or URI of
/// the file stored in Amazon Simple Storage Service (Amazon S3)", so a value
/// addressing anything else is a value the operation cannot act on: it is
/// refused before the import starts, and the sentence states what the request
/// has to carry instead. The whole sentence is fixed text — it quotes nothing
/// from the request, because the fault is the FORM of the value and not which
/// object it names.
pub fn invalid_domain_file_url() -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_INVALID_DOMAIN_FILE_URL,
            "A valid S3 URL or S3 URI is required",
        )),
    }
}

/// The bucket and key an Amazon S3 URL addresses, or `None` when it addresses
/// no object.
///
/// The three forms a fully qualified S3 URL or URI takes: the
/// `s3://<bucket>/<key>` scheme in either case, and the two HTTPS addressings
/// of the same object — virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<key>`) and path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<key>`). An HTTPS URL whose
/// authority does not name the Amazon S3 service addresses some other server's
/// file, which is not an object of this account's inventory and so addresses no
/// object here; nor does a URL with no key under the bucket, which addresses a
/// bucket rather than a file.
fn s3_object_address(url: &str) -> Option<(&str, &str)> {
    let rest = ["s3://", "S3://", "https://", "http://"]
        .into_iter()
        .find_map(|scheme| url.strip_prefix(scheme))?;
    let (authority, path) = rest.split_once('/')?;
    let (bucket, key) = if url.starts_with("http") {
        // The LAST `.s3` label is the service one, so a bucket name that itself
        // contains `.s3` stays whole.
        match authority.rsplit_once(".s3") {
            // Virtual-hosted: the bucket is everything before the service label.
            Some((bucket, _)) if !bucket.is_empty() => (bucket, path),
            // Path-style: the bucket is the first segment of the path, and the
            // authority is the service's own endpoint rather than any host.
            _ if authority.starts_with("s3.") || authority.starts_with("s3-") => {
                path.split_once('/')?
            }
            _ => return None,
        }
    } else {
        (authority, path)
    };
    if bucket.is_empty() || key.is_empty() {
        return None;
    }
    Some((bucket, key))
}
