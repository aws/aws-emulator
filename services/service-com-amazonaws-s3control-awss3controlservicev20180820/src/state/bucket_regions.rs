// SPDX-License-Identifier: CC0-1.0

//! Where a bucket this service associates actually lives.
//!
//! Several things S3 Control reports about a bucket are facts the BUCKET carries
//! rather than facts a request to this service carried: a Multi-Region Access
//! Point's `RegionReport$Region` is "a combination of a bucket and Region that's
//! part of a Multi-Region Access Point", while the create request that
//! associated it names each bucket WITHOUT its Region (`Region$Bucket`,
//! `Region$BucketAccountId`). The service resolves the Region from the bucket,
//! so a Multi-Region Access Point over buckets in two Regions reports two.
//!
//! A bucket's Region is held by the service that OWNS the bucket — Amazon S3,
//! whose own `GetBucketLocation` reports it — so it is read in that service's
//! store rather than copied into this one, exactly as a bucket's tag set is
//! (`crate::state::change_bucket_tags`).
//!
//! LOCK ORDER: this takes the bucket store's lock and holds no other while it
//! does, and hands back an owned value rather than a guard — so it must be
//! called with NO guard of this service's store held. Keeping the cross-service
//! access one-directional and guard-free is what makes a lock cycle between the
//! two stores unreachable.

use super::*;

use crate::ops::types::Region;
use std::collections::BTreeSet;

/// The Region the bucket `bucket` lives in, or `None` when the object store
/// holds no such bucket (and when this process has no object store at all).
///
/// `bucket` is the bucket's NAME: a bucket name is globally unique and the
/// bucket store keys a bucket by that name alone, which is why no account id is
/// taken here.
pub fn bucket_region<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> Option<String> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    store.bucket(bucket).map(|held| held.region.clone())
}

/// Whether the object store holds a bucket named `bucket`.
///
/// The referential read behind an association this service only makes over a
/// bucket that EXISTS: the bucket is the object store's resource, so its
/// existence is answered where the bucket lives rather than from a name this
/// service was handed ([`bucket_region`], which resolves a bucket to the Region
/// it lives in and answers `None` for a bucket the store does not hold).
///
/// Takes the bucket's NAME alone, for the same reason [`bucket_region`] does.
pub fn bucket_exists<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> bool {
    bucket_region(engine, bucket).is_some()
}

/// The Regions to report for the buckets a Multi-Region Access Point create
/// request associated, in the order the request listed them.
///
/// Each bucket's own Region when the object store holds it; otherwise the
/// Region the request was signed for, which is all this service knows about a
/// bucket it cannot resolve — a report that named no Region for a member bucket
/// would drop the association from the access point's own report.
pub fn associated_region_reports<Eng: Send + Sync + 'static>(
    engine: &Eng,
    associated: &[Region],
    signing_region: &str,
) -> Vec<RegionReport> {
    associated
        .iter()
        .map(|member| RegionReport {
            region: Some(
                bucket_region(engine, &member.bucket)
                    .unwrap_or_else(|| signing_region.to_string()),
            ),
            bucket: Some(member.bucket.clone()),
            bucket_account_id: member.bucket_account_id.clone(),
        })
        .collect()
}

/// Why the buckets a Multi-Region Access Point create request named cannot be
/// associated, or `None` when every one of them can be.
///
/// Two rules, read in the order the request has to satisfy them — the list
/// itself first, then the buckets it resolves to:
///
/// * a Multi-Region Access Point associates ONE bucket per Region, so a list
///   naming a bucket more than once asks for a configuration no access point
///   can have ([`duplicate_associated_bucket`]);
/// * a Multi-Region Access Point IS its set of associated buckets, so a bucket
///   the object store does not hold cannot be associated
///   ([`associated_bucket_absent`]) — the same referential read
///   `CreateAccessPoint` makes before it attaches an access point to a bucket.
///
/// The create binds no errors at all, so what comes back is the OUTCOME the
/// request settles with ([`AsyncRequestRecord::failure`]) rather than a refusal
/// of the call, and a create that cannot be carried out leaves no access point
/// behind.
///
/// Takes the bucket store's lock, so it is subject to the lock order the module
/// docs state: call it with NO guard of this service's store held.
pub fn create_association_failure<Eng: Send + Sync + 'static>(
    engine: &Eng,
    details: &CreateMultiRegionAccessPointInput2,
) -> Option<AsyncErrorDetails> {
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for member in &details.regions {
        if !named.insert(member.bucket.as_str()) {
            return Some(duplicate_associated_bucket(&details.name));
        }
    }
    details
        .regions
        .iter()
        .any(|member| !bucket_exists(engine, &member.bucket))
        .then(|| associated_bucket_absent(&details.name))
}
