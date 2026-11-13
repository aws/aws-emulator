// SPDX-License-Identifier: CC0-1.0

//! PUBLISHING an access point's data path into the object store that serves it.
//!
//! An access point is created, reported and removed here, and it is USED in the Amazon S3
//! data plane: "Amazon S3 access points are named network endpoints that are attached to
//! buckets that you can use to perform S3 object operations" (Amazon S3 User Guide,
//! *Managing data access with Amazon S3 access points*). A request addressed to the access
//! point reaches the bucket it fronts and is judged against the access point's own policy —
//! and neither of those can be answered from this store, which the data plane does not read.
//!
//! So the data path is published into the store that serves it, the way a bucket's tag set
//! is read and written in the store that owns the bucket ([`crate::state::change_resource_tags`]):
//! this service's record stays the resource ([`crate::state::AccessPointRecord`], which is
//! what `GetAccessPoint` and `ListAccessPoints` report), and the part a DATA-PLANE request is
//! served and judged by is kept once, in that store
//! (`service_com_amazonaws_s3_amazons3::state::access_points`). The four calls that change it
//! publish it: the create, the delete, and the two that put and remove the policy.
//!
//! LOCK ORDER: every function here takes the object store's lock and holds no other while it
//! does, so it must be called with NO guard of this service's store held — the same
//! discipline, and for the same reason, as this service's other reach into that store.

use ::service_com_amazonaws_s3_amazons3::state::access_points::AccessPointRoute;

use crate::state::AccessPointRecord;

/// Publish `record`'s data path, so a request addressed to the access point reaches the
/// bucket it fronts and is judged by the policy it carries.
///
/// Answers whether the object store recorded it: it does not when that store's own ceiling
/// on how many paths one process holds is reached, which is above this service's quota on
/// how many access points may exist ([`crate::state::ACCESS_POINT_QUOTA`]), so a create this
/// service admits is always published. A caller that cannot publish must not report the
/// access point as created.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, record: &AccessPointRecord) -> bool {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    store.record_access_point(AccessPointRoute {
        arn: record.arn.clone(),
        name: record.name.clone(),
        account_id: record.account_id.clone(),
        alias: record.alias.clone(),
        bucket: record.bucket.clone(),
        // The VPC the access point is CONFINED to, when the create bound it to one: "if this
        // field is specified, this access point will only allow connections from the
        // specified VPC ID" (`VpcConfiguration$VpcId`). The restriction is on the data path,
        // so it is published with the path rather than kept here, where no data-plane
        // request could consult it — read through this service's own one reading of it
        // ([`crate::state::confining_vpc`]), the same reading the reported network origin is.
        vpc_id: crate::state::confining_vpc(record.vpc_configuration.as_ref()),
        policy: record.policy.clone(),
    })
}

/// Put `policy` on the published path of the access point whose ARN is `arn` — the document
/// a `PutAccessPointPolicy` stored, or `None` for one a `DeleteAccessPointPolicy` removed —
/// answering whether the object store holds a path for it.
pub fn publish_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &str,
    policy: Option<String>,
) -> bool {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    store.set_access_point_policy(arn, policy)
}

/// Withdraw the published path of the access point whose ARN is `arn`: the addresses that
/// reached it stop resolving at the moment the resource stops existing.
pub fn withdraw<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> bool {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    store.forget_access_point(arn)
}
