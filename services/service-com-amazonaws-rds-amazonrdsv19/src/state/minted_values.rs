// SPDX-License-Identifier: CC0-1.0

//! The server-MINTED values this service assigns to the resources it creates: the
//! identifier prefixes, the one monotone counter they are drawn from, and the minting
//! function of each resource kind.
//!
//! Split out of [`super`] so that module keeps the store, the records and their
//! projections; every name here is re-exported from it, so a handler still reads them as
//! `crate::state::<name>`.
//!
//! Every value minted here is OPAQUE: it identifies the one resource it was minted for and
//! says nothing else — see [`minted_token`] for what that buys and why a minting draws on
//! more than the store's counter.

use super::*;

/// How many characters a REGIONAL resource id of this service carries after its `<kind>-`
/// prefix: the width AWS writes `DbiResourceId` and `DbClusterResourceId` in. A global
/// database's id has its own width ([`mint_global_cluster_resource_id`]).
const RESOURCE_ID_TOKEN_LEN: usize = 26;

/// One minting of a server-assigned, opaque token: `width` characters of the
/// digits-and-uppercase alphabet AWS writes a server-assigned id in.
///
/// `label` names the kind of resource the token belongs to and `mint` is the store's one
/// monotone counter, so two tokens minted from one store always differ. The process and the
/// clock go into the seed as well, so the nth resource of one store is NOT handed the token
/// the nth resource of another store got: the token is unique to its one resource across
/// the partition, which is what an "Amazon Web Services partition-unique, immutable
/// identifier" is — the handle a client keeps to tell two resources apart and to correlate
/// audit records with the resource they are about. Being opaque, it also says nothing about
/// WHEN in a sequence its resource was created, which is a fact no AWS resource id carries.
///
/// A minted token is STORED with the resource it was minted for and every later read
/// reports that stored value, so drawing on the clock costs nothing in stability: it is
/// minted once, at create, and is immutable from there on.
///
/// The digest behind it is the maintained `sha2` crate's ([`super::minted_base36`]), so
/// nothing here is hand-rolled.
pub fn minted_token(label: &str, mint: u64, width: usize) -> String {
    super::minted_base36(&mint_seed(label, mint), width)
}

/// One minting of a server-assigned token written in LOWERCASE HEXADECIMAL: `width`
/// characters, drawn from the same seed [`minted_token`] draws on, so everything said
/// there about uniqueness and stability holds here too.
///
/// Which of the two alphabets an id is written in is the recorded form of that id, per
/// resource kind; see [`mint_global_cluster_resource_id`].
pub fn minted_hex_token(label: &str, mint: u64, width: usize) -> String {
    super::minted_hex(&mint_seed(label, mint), width)
}

/// The seed ONE minting draws on: the kind of resource, the store's monotone counter, a
/// per-process minting nonce, this process, and the instant of the minting.
///
/// The counter separates two resources of one store; the nonce separates two mintings that
/// fall inside one clock tick; the process id and the clock separate two stores, so no
/// sequence of calls against one store can produce a value another store already handed
/// out. See [`minted_token`].
pub fn mint_seed(label: &str, mint: u64) -> String {
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// How many tokens this process has minted, so two mintings inside one clock tick
    /// still differ.
    static MINTINGS: AtomicU64 = AtomicU64::new(0);

    let nonce = MINTINGS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let process = ::std::process::id();
    format!("{label}:{mint}:{nonce}:{process}:{nanos}")
}

/// The immutable resource id RDS reports as `DbiResourceId`: `db-` plus 26
/// uppercase characters, minted once per instance and never reused.
pub fn mint_dbi_resource_id(mint: u64) -> String {
    format!("db-{}", minted_token("db-instance", mint, RESOURCE_ID_TOKEN_LEN))
}

/// The DNS label RDS assigns an instance's endpoint between the identifier and the
/// region: a server-minted, 12-character token, unique per instance.
pub fn mint_endpoint_host(mint: u64) -> String {
    minted_token("endpoint-host", mint, 12).to_ascii_lowercase()
}

/// The immutable resource id RDS reports as `GlobalClusterResourceId`: the
/// "partition-unique, immutable identifier for the global database cluster", written
/// `cluster-` followed by an opaque, fixed-length token minted once per global database and
/// never reused.
///
/// A global database's resource id is minted by the global-database control plane rather
/// than by the regional one, and its form is the one the RECORDED global-database envelope
/// carries: the lowercase hexadecimal token of the width below, not the
/// digits-and-uppercase token of a REGIONAL resource id ([`mint_db_cluster_resource_id`],
/// [`mint_dbi_resource_id`]). The model states no pattern for either member, so each id
/// kind follows the form AWS is recorded as reporting for it; replaying the recorded
/// envelope against this member accepts the form below and refuses the regional one, which
/// is why the two differ here.
pub fn mint_global_cluster_resource_id(mint: u64) -> String {
    format!(
        "cluster-{}",
        minted_hex_token("global-cluster", mint, GLOBAL_CLUSTER_RESOURCE_ID_TOKEN_LEN)
    )
}

/// How many characters the token of a GLOBAL database's resource id carries after the
/// `cluster-` prefix, in the lowercase hexadecimal alphabet
/// [`mint_global_cluster_resource_id`] writes it in.
const GLOBAL_CLUSTER_RESOURCE_ID_TOKEN_LEN: usize = 16;

/// The immutable resource id RDS reports as `DbClusterResourceId`: "the Amazon Web
/// Services Region-unique, immutable identifier for the DB cluster", `cluster-` plus 26
/// characters of the same server-minted alphabet [`mint_dbi_resource_id`] draws on. Both
/// are minted from the one store-wide counter and the per-minting seed, so no two
/// resources of this service can be minted the same id.
pub fn mint_db_cluster_resource_id(mint: u64) -> String {
    format!(
        "cluster-{}",
        minted_token("db-cluster", mint, RESOURCE_ID_TOKEN_LEN)
    )
}
