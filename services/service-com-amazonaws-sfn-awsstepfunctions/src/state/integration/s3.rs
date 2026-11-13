// SPDX-License-Identifier: CC0-1.0

//! Amazon S3's own wire facts, which this crate reaches for a reason none of the
//! OPTIMIZED family's services are reached for.
//!
//! S3 publishes no optimized integration, so no `Task` state's `Resource` names it that
//! way: it is reached as the DATASET a `Map` state's `ItemReader` names, as the place its
//! `ResultWriter` writes, and through the AWS SDK integration family's own token. Its
//! entry is therefore kept out of [`APIS`] — the table the optimized family is looked up
//! in — and lives here with the REST bindings of the operations those readers and writes
//! make, so the one concern sits in one file.

use super::*;

/// Amazon S3, for the calls this crate makes to it: the DATASET a `Map` state's
/// `ItemReader` names ([`super::item_reader`]), the result objects its `ResultWriter`
/// writes ([`super::result_writer`]), and a `Task` state whose `Resource` names one of
/// its operations through the AWS SDK integration family.
///
/// Deliberately NOT one of [`APIS`], which is where the OPTIMIZED family's services are
/// looked up: S3 publishes no optimized integration, so `arn:aws:states:::s3:<action>`
/// names no `Task` state's work — it is how a `Map` state's `ItemReader` and
/// `ResultWriter` name their dataset. [`read`] reaches these facts for the AWS SDK
/// family's own token (`aws-sdk:s3:<action>`) and reports an optimized `Resource` as
/// work nothing was run for, exactly as before.
///
/// `services/service-com-amazonaws-s3-amazons3/smithy-model/
/// com-amazonaws-s3-amazons3.json`: restXml, sigv4Name `s3`, endpointPrefix `s3`,
/// sdkId `S3`, arnNamespace `s3`.
pub(super) static S3_API: Api = Api {
    namespace: S3_NAMESPACE,
    action_prefix: None,
    signing_name: S3_NAMESPACE,
    endpoint_prefix: S3_NAMESPACE,
    wire: Wire::Rest(S3_REST),
    sdk_id: "S3",
    error_prefix: "S3",
    // The client class `AmazonS3Client`, whose base exception is `AmazonS3Exception`.
    sdk_client: "AmazonS3",
    sdk_exception: "AmazonS3Exception",
    access_denied_status: TARGET_FORBIDDEN_STATUS,
    // `GetObject`'s input: `Bucket`, `Key`; `ListObjectsV2`'s: `Bucket`, `Prefix`.
    member_case: MemberCase::Pascal,
    query_compatible: false,
};

/// S3's ARN namespace, its signing name and its endpoint prefix, which are the same
/// label.
pub(super) const S3_NAMESPACE: &str = "s3";

/// The REST bindings of the S3 operations this crate calls: the ones a dataset read
/// and a dataset write make, and the ones a `Task` state's AWS SDK integration names
/// ([`S3_SDK_OPERATIONS`]).
///
/// `smithy_query(query="operation",operation="ListObjectsV2,GetObject,PutObject,
/// HeadBucket,DeleteObject",model_path="services/service-com-amazonaws-s3-amazons3/
/// smithy-model/com-amazonaws-s3-amazons3.json")`: `ListObjectsV2` is `http` GET
/// `/{Bucket}?list-type=2` with `Bucket` an `httpLabel` and `Prefix`,
/// `ContinuationToken` and `MaxKeys` `httpQuery` members, answering an XML document
/// as the whole body; `GetObject` is GET `/{Bucket}/{Key+}?x-id=GetObject` with
/// `Bucket` and the GREEDY `Key` as `httpLabel`s, its output binds `Body` as its
/// `httpPayload` and `ETag`, `ContentType` and `VersionId` to headers whose value is
/// the text they carry; `PutObject` is PUT `/{Bucket}/{Key+}?x-id=PutObject` over
/// the same two labels, with `Body` as the request's own `httpPayload` and `ETag` bound
/// to the answer's `ETag` header; `HeadBucket` is HEAD `/{Bucket}` over the one label,
/// every member of its output bound to a header; `DeleteObject` is DELETE
/// `/{Bucket}/{Key+}?x-id=DeleteObject` over the two labels, with `VersionId` an
/// `httpQuery` member, `MFA`, `BypassGovernanceRetention`, `IfMatch` and the rest
/// `httpHeader`s, and every member of its output bound to a header.
pub(super) const S3_REST: &[Rest] = &[
    Rest {
        operation: LIST_OBJECTS_OPERATION,
        method: "GET",
        uri: "/{Bucket}?list-type=2",
        request_body: Body::Empty,
        headers: &[],
        query: &[
            ("Prefix", "prefix"),
            ("ContinuationToken", "continuation-token"),
            ("MaxKeys", "max-keys"),
        ],
        response_body: Body::Text(DATASET_BODY),
        response_status: "",
        response_headers: &[],
    },
    Rest {
        operation: GET_OBJECT_OPERATION,
        method: "GET",
        uri: "/{Bucket}/{Key+}?x-id=GetObject",
        request_body: Body::Empty,
        headers: &[],
        query: &[],
        response_body: Body::Text(DATASET_BODY),
        response_status: "",
        // The output members a read's answer carries beside the object's bytes: the
        // ones whose value IS the text the header holds, so what the result reports is
        // the answer's own. A member the model types as a number or an instant
        // (`ContentLength`, `LastModified`) is deliberately absent: its header carries
        // text, and reporting that text under a member a client reads as a number
        // would be the wrong type rather than the answer.
        response_headers: &[
            ("ETag", "ETag"),
            ("ContentType", "Content-Type"),
            ("VersionId", "x-amz-version-id"),
        ],
    },
    Rest {
        operation: PUT_OBJECT_OPERATION,
        method: "PUT",
        uri: "/{Bucket}/{Key+}?x-id=PutObject",
        // The object's own bytes are the request's body: the operation binds `Body` as
        // its `httpPayload`, so what the write carries under that member IS the object.
        request_body: Body::Member(DATASET_BODY),
        headers: &[],
        query: &[],
        // A write answers no document at all — every member of its output is a header —
        // so the one the caller of a write reads is the entity tag the answer carried.
        response_body: Body::Empty,
        response_status: "",
        response_headers: &[("ETag", "ETag")],
    },
    Rest {
        operation: HEAD_BUCKET_OPERATION,
        method: "HEAD",
        uri: "/{Bucket}",
        // `Bucket` is the path, and `ExpectedBucketOwner` the one other input member,
        // is a header: a HEAD of a bucket sends no body at all.
        request_body: Body::Empty,
        headers: &[("ExpectedBucketOwner", "x-amz-expected-bucket-owner")],
        query: &[],
        // A HEAD carries no body: every member of this operation's output is a header,
        // so the answer's headers ARE its output.
        response_body: Body::Empty,
        response_status: "",
        // The output members whose value IS the text the header holds. The remaining
        // one, `AccessPointAlias`, the model types as a boolean, and reporting a
        // header's text under a member a client reads as a flag would be the wrong type
        // rather than the answer — the same reading `GetObject`'s own numbers and
        // instants are left out under.
        response_headers: &[
            ("BucketArn", "x-amz-bucket-arn"),
            ("BucketRegion", "x-amz-bucket-region"),
            ("BucketLocationName", "x-amz-bucket-location-name"),
            ("BucketLocationType", "x-amz-bucket-location-type"),
        ],
    },
    Rest {
        operation: DELETE_OBJECT_OPERATION,
        method: "DELETE",
        uri: "/{Bucket}/{Key+}?x-id=DeleteObject",
        // Both labels are in the path and every other input member is a header or a
        // query parameter, so a delete sends no body.
        request_body: Body::Empty,
        headers: &[
            ("MFA", "x-amz-mfa"),
            ("RequestPayer", "x-amz-request-payer"),
            (
                "BypassGovernanceRetention",
                "x-amz-bypass-governance-retention",
            ),
            ("ExpectedBucketOwner", "x-amz-expected-bucket-owner"),
            ("IfMatch", "If-Match"),
        ],
        query: &[("VersionId", "versionId")],
        // A delete answers no body either: its output is the headers its answer
        // carried, exactly as a write's is.
        response_body: Body::Empty,
        response_status: "",
        // `DeleteMarker` is left out for the reason `HeadBucket`'s `AccessPointAlias`
        // is: the model types it as a boolean.
        response_headers: &[
            ("VersionId", "x-amz-version-id"),
            ("RequestCharged", "x-amz-request-charged"),
        ],
    },
];
