// SPDX-License-Identifier: CC0-1.0

//! The `AccessPoint` summary shape an access point LISTING carries, as the wire
//! carries it.
//!
//! Every answer that reports an access point reports what KIND of access point
//! it is (`AccessPointType`), and an entry of `ListAccessPoints` or
//! `ListAccessPointsForDirectoryBuckets` carries it beside the modelled members
//! — but the on-disk model declares no such member on `AccessPoint`, so the
//! shape cannot be declared through `#[smithy_macros::smithy(shape = ...)]` at
//! all (see `types::unmodelled_member_wire` for why, and for what the macro
//! below emits in the derive's place).
//!
//! The shape lives HERE, in a file no model refresh writes, rather than beside
//! the other shapes several operations reach (`types::shared_types`, which is
//! rendered from the model and rewritten whenever the model is refreshed): a
//! declaration that has to carry a member the model does not have cannot survive
//! in a generated file. `types/mod.rs` re-exports this module like every other,
//! so `AccessPoint` resolves unchanged wherever it is named.
//!
//! What the extra member CARRIES is not decided here: it is the stored access
//! point's own recorded type ([`crate::state::AccessPointRecord::summary`]), so
//! a listing reports what the create left on each record.
#![allow(unused_imports)]

use super::*;

/// One access point as a listing reports it: the modelled `AccessPoint`, plus
/// `AccessPointType`.
///
/// Member set, order and requiredness from
/// `smithy_query(query="shape", shape="com.amazonaws.s3control#AccessPoint")`,
/// plus `AccessPointType`, which is RECORDED BEHAVIOUR rather than modelled —
/// the same member `GetAccessPoint` reports for one access point
/// (`types::get_access_point_types`).
#[derive(Debug, Clone, Default)]
pub struct AccessPoint {
    pub name: String,
    pub network_origin: String,
    pub vpc_configuration: Option<VpcConfiguration>,
    pub bucket: String,
    pub access_point_arn: Option<String>,
    pub alias: Option<String>,
    pub bucket_account_id: Option<String>,
    pub data_source_id: Option<String>,
    pub data_source_type: Option<String>,
    pub access_point_type: Option<String>,
}

s3c_schema_and_codec!(
    AccessPoint,
    ACCESS_POINT_SCHEMA,
    "com.amazonaws.s3control#AccessPoint",
    "com.amazonaws.s3control",
    "AccessPoint",
    (M_NAME, name, "Name", 0, req, string),
    (
        M_NETWORK_ORIGIN,
        network_origin,
        "NetworkOrigin",
        1,
        req,
        string
    ),
    (
        M_VPC_CONFIGURATION,
        vpc_configuration,
        "VpcConfiguration",
        2,
        opt,
        struct_of(VpcConfiguration)
    ),
    (M_BUCKET, bucket, "Bucket", 3, req, string),
    (
        M_ACCESS_POINT_ARN,
        access_point_arn,
        "AccessPointArn",
        4,
        opt,
        string
    ),
    (M_ALIAS, alias, "Alias", 5, opt, string),
    (
        M_BUCKET_ACCOUNT_ID,
        bucket_account_id,
        "BucketAccountId",
        6,
        opt,
        string
    ),
    (
        M_DATA_SOURCE_ID,
        data_source_id,
        "DataSourceId",
        7,
        opt,
        string
    ),
    (
        M_DATA_SOURCE_TYPE,
        data_source_type,
        "DataSourceType",
        8,
        opt,
        string
    ),
    (
        M_ACCESS_POINT_TYPE,
        access_point_type,
        "AccessPointType",
        9,
        opt,
        string
    ),
);
