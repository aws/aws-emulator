// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rekognition-rekognitionservice service.
//!
//! Besides the state struct this module holds everything the face-collection
//! handlers share — ARN construction, the request's region, the record-to-modelled
//! shape conversions, the list cursor codec, the store's capacity bounds, the
//! image-identity digest and the construction of the modelled errors in
//! [`modeled_error`] — so the nine operation files agree on one implementation of
//! each instead of carrying copies that can drift apart.
//!
//! ## What this emulator does and does not compute
//!
//! Rekognition's face operations are backed by a face detector and a face
//! recogniser. The RECOGNISER does not exist in this process: nothing here maps a face
//! to an IDENTITY, so no face is tied to the person it belongs to. What IS real is
//! the collection store — a collection, the faces indexed into it, their minted ids, and
//! the counts, filters, pages and lookups computed from them — and, for the operations
//! that analyse a submitted image, the geometric face DETECTOR in
//! [`face_detection`]: it decodes the image and measures the faces it finds in the
//! pixels, reporting a box, a confidence, landmarks, a roll and a quality it measured
//! and leaving every classifier's judgement (an age, a gender, an emotion) absent. Two
//! faces the detector found in two SUBMITTED images can also be compared, because both
//! sides of that comparison are pixels this process holds: `CompareFaces` reports the
//! resemblance [`face_detection::FaceAppearance`] measures between them, which is a
//! measured correlation of two faces and still not an identity.
//!
//! Nothing here reports a value nothing measured. The same detector is what decides what
//! `IndexFaces` stores: a collection holds one face per face DETECTED in a submitted
//! image, and an image carrying none adds nothing to it. A STORED face therefore REPORTS
//! its minted identity plus the two members the detector measured for it and `Face`
//! models — its `BoundingBox` and its `Confidence` — and nothing else: no landmarks, pose
//! or quality, no demographic attribute and no orientation correction. Beside those it
//! keeps, unreported, the [`face_detection::FaceAppearance`] the detector measured off its
//! own pixels, which is what lets a face submitted later be compared with the faces a
//! collection already holds: `SearchFacesByImage` reports that measured resemblance, the
//! same measurement `CompareFaces` makes between two submitted images, and it is a
//! resemblance of two face patches and still not an identity. The operations that would
//! need a trained model of WHO a face belongs to (`SearchUsersByImage`, …) stay
//! unimplemented 501s.
//!
//! The same rule governs the LABELS of a submitted image in [`label_detection`]: no
//! classifier over a label taxonomy runs here, so the entities it reports are the ones the
//! same geometric detector measured — a face, and the person it is part of — while the
//! properties of the image itself (its quality, its dominant colours and the two sides of
//! its foreground/background split) are measured off the decoded pixels and reported in
//! full.
//!
//! Where an operation PARTITIONS the faces of a submitted image by an identity nothing here
//! holds, the detector still answers for the part it measured: `RecognizeCelebrities`
//! reports every face it measured as an unrecognized face — the side the operation itself
//! puts a face it does not recognise on — and names no celebrity, rather than refusing the
//! call or inventing one. `DetectProtectiveEquipment` answers the same way about the PERSONS
//! a frame carries: each face the detector measured locates a person, reported with the
//! confidence and the one body part measured for it, while an item of equipment and whether
//! it covers a body part are a judgement no detector here makes, so each person the summary
//! covers is reported as one whose PPE adornment could not be determined.
//!
//! The same rule governs a video: no video decoder runs in this process either, so nothing
//! measures a property of the media and nothing is found IN it. What IS real about a
//! stored-video analysis is the JOB — a `Start…` operation establishes with this emulator's
//! own Amazon S3 that the object it was given is served ([`video_source`]), records the
//! request as a job of [`video_jobs`] and hands back the identifier the matching `Get…`
//! operation reads that job by, with the job's status owed by the clock. The analysis
//! RESULTS belong to the `Get…` operation that reports them, and the ones that would report
//! them stay unimplemented 501s.
//!
//! The one comparison this emulator can make about two STORED faces is IDENTITY:
//! [`image_signature`] digests what an image is, so two indexed faces can be told to have
//! come from the same image content or not. `SearchFaces` reports exactly that and no
//! graded score — see its own module docs.

pub mod face_detection;
pub mod idempotency;
pub mod image_source;
pub mod label_detection;
pub mod modeled_error;
pub mod project_policies;
pub mod video_jobs;
pub mod video_source;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{BoundingBox, Face, Image, ProjectDescription, User};

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: rekognition`).
pub const ARN_NAMESPACE: &str = "rekognition";

/// The face model version a collection created by this emulator is associated
/// with. `CreateCollection` documents that "when you create a collection, it is
/// associated with the latest version of the face model version", which is the generation
/// Rekognition associates a collection created today with; the version is an opaque
/// string on the wire, so one value is pinned here and every operation echoes the
/// collection's own stored copy rather than this constant.
pub const FACE_MODEL_VERSION: &str = "7.0";

/// The page size `ListFaces` / `ListCollections` use when the caller sends no
/// `MaxResults`: the largest page the model allows (`PageSize` range 0..4096).
pub const DEFAULT_PAGE_SIZE: usize = 4096;

/// The cap `SearchFaces` / `SearchFacesByImage` use when the caller sends no `MaxFaces`:
/// the largest the model allows (`MaxFaces` range 1..4096). The model documents no default
/// for it.
pub const DEFAULT_MAX_FACES: usize = 4096;

/// The match floor `SearchFaces` / `SearchFacesByImage` use when the caller sends no
/// `FaceMatchThreshold`. Documented on the member itself in both requests
/// (`smithy_query(query="doc",shape="SearchFacesRequest,SearchFacesByImageRequest",member="FaceMatchThreshold")`:
/// "The default value is 80%").
pub const DEFAULT_FACE_MATCH_THRESHOLD: f32 = 80.0;

/// The `CompareFaces` match floor used when the caller sends no `SimilarityThreshold`.
/// Documented on the operation itself
/// (`smithy_query(query="doc",shape="CompareFaces")`: "By default, only faces with a
/// similarity score of greater than or equal to 80% are returned in the response. You can
/// change this value by specifying the SimilarityThreshold parameter").
pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 80.0;

/// The `Similarity` `SearchFaces` reports for the only match this emulator can
/// establish: a face indexed from the same image content as the searched one. It is
/// the top of the modelled `Percent` range, and the ONLY value reported, because this
/// process computes no graded similarity.
pub const IDENTICAL_CONTENT_SIMILARITY: f32 = 100.0;

/// The `CustomizationFeature` a project created without one is customized for.
/// Documented on the member itself
/// (`smithy_query(query="doc",shape="CreateProjectRequest",member="Feature")`: "If no
/// value is provided CUSTOM_LABELS is used as a default").
pub const DEFAULT_CUSTOMIZATION_FEATURE: &str = "CUSTOM_LABELS";

/// The `CustomizationFeature` whose projects are adapters, for which `AutoUpdate` is a
/// required argument
/// (`smithy_query(query="doc",shape="CreateProjectRequest",member="AutoUpdate")`:
/// "Required argument for Content Moderation").
pub const CONTENT_MODERATION_FEATURE: &str = "CONTENT_MODERATION";

/// The `ProjectStatus` a project of this store carries once it has been created.
///
/// `CreateProject` provisions nothing — a project is a container for datasets and model
/// versions, both of which are created by their own operations — and its response
/// carries no status, so the project is already created by the time any reader can ask
/// about it. The other two `ProjectStatus` values (`smithy_query(query="shape",
/// shape="ProjectStatus")`: CREATING | CREATED | DELETING) belong to transitions no
/// operation of this slice starts.
pub const PROJECT_STATUS_CREATED: &str = "CREATED";

/// How many collections this store will hold, and how many faces one collection will
/// hold, before a create or an index is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// This store lives in the emulator's memory, so it is bounded far below Rekognition's
/// own published quotas: these are the EMULATOR's limits, not AWS's, and they exist so
/// that a caller repeating `CreateCollection` or `IndexFaces` is refused rather than
/// allowed to grow the process without limit. The modelled error is the same one AWS
/// answers past its quota ("The size of the collection exceeds the allowed limit"), so
/// a client sees a quota refusal either way.
pub const MAX_COLLECTIONS: usize = 100;
pub const MAX_FACES_PER_COLLECTION: usize = 1_000;

/// How many users one collection will hold before a create is refused with the
/// modelled `ServiceQuotaExceededException`. The same reasoning as
/// [`MAX_FACES_PER_COLLECTION`]: this store is the emulator's memory, so a caller
/// repeating `CreateUser` is refused rather than allowed to grow the process without
/// limit, and the modelled error is the one AWS answers past its own quota.
pub const MAX_USERS_PER_COLLECTION: usize = 1_000;

/// The `UserStatus` a user carries from the moment `CreateUser` records it.
///
/// `UserStatus` "communicates if the UserID has been updated with latest set of faces
/// to be associated with the UserID"
/// (`smithy_query(query="shape",shape="User")`), and its values are
/// ACTIVE | UPDATING | CREATING | CREATED
/// (`smithy_query(query="shape",shape="UserStatus")`). A user `CreateUser` has just
/// recorded is created and has no face associated with it yet, which is `CREATED`; the
/// other values belong to transitions `AssociateFaces` and `DisassociateFaces` start,
/// and neither is implemented yet.
pub const USER_STATUS_CREATED: &str = "CREATED";

/// How many tags one resource of this store will hold: the top of the modelled `TagMap`
/// length range (`length 0..200`,
/// `smithy_query(query="operation",operation="TagResource")`), which is also AWS's own
/// published limit of tags per resource. Past it a tagging request is refused with the
/// modelled quota error rather than growing the record without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// How many projects this store will hold before a create is refused with the modelled
/// `LimitExceededException`. The same reasoning as [`MAX_COLLECTIONS`]: the store is
/// this process's memory, so a caller repeating `CreateProject` is refused rather than
/// allowed to grow it without limit.
pub const MAX_PROJECTS: usize = 100;

/// One face stored in a collection by `IndexFaces`.
///
/// Its members are the face's IDENTITY — what the service minted and what the caller
/// supplied — plus the two measurements `Face` models for a stored face, taken by the
/// detector that found it, and the appearance that same detector measured off its pixels,
/// which this API reports nowhere and a later search compares against. Nothing nothing
/// measured is stored.
#[derive(Debug, Clone)]
pub struct StoredFace {
    /// The id this service minted for the face; the collection's key for it.
    pub face_id: String,
    /// The id this service minted for the image the face was indexed from.
    pub image_id: String,
    /// `IndexFaces.ExternalImageId`, carried through unchanged.
    pub external_image_id: Option<String>,
    /// The face model version of the collection at the time of indexing.
    pub index_faces_model_version: String,
    /// Where in the indexed image the detector found this face, as the fractions of the
    /// image a `BoundingBox` is expressed in.
    pub bounding_box: Option<BoundingBox>,
    /// The detector's own measured agreement that the box holds a face.
    pub confidence: Option<f32>,
    /// The user this face is associated with, set by `AssociateFaces` (not yet
    /// implemented), so it is `None` for every face this slice stores. It is carried
    /// here, and honoured by `ListFaces`' `UserId` filter and `DeleteFaces`'
    /// `ASSOCIATED_TO_AN_EXISTING_USER` refusal, as pre-wiring for the rest of the user
    /// slice: a user exists (`CreateUser`) but nothing associates a face with one yet.
    pub user_id: Option<String>,
    /// The digest of the image content this face was indexed from: what lets
    /// `SearchFaces` tell whether two stored faces came from the same image.
    pub signature: [u8; 32],
    /// What this face LOOKS LIKE, measured by the detector off the pixels inside its own
    /// box when it was indexed (`face_detection::FaceAppearance`). It is not a member of
    /// any modelled shape and is reported by nothing: it is what a face submitted later is
    /// compared WITH, so `SearchFacesByImage` can measure how alike that face and this one
    /// look instead of reporting a resemblance nothing measured.
    pub appearance: face_detection::FaceAppearance,
    /// This face's position in the collection's insertion order. Monotonic per
    /// collection and never reused, so a `ListFaces` cursor built from it neither
    /// repeats nor skips a face when other faces are added or deleted between
    /// pages.
    pub sequence: u64,
}

impl StoredFace {
    /// This face as the modelled `Face`.
    ///
    /// `BoundingBox` and `Confidence` are the detector's own measurements of the face in
    /// the image it was indexed from, carried through unchanged.
    pub fn modeled(&self) -> Face {
        Face {
            face_id: Some(self.face_id.clone()),
            bounding_box: self.bounding_box.clone(),
            image_id: Some(self.image_id.clone()),
            external_image_id: self.external_image_id.clone(),
            confidence: self.confidence,
            index_faces_model_version: Some(self.index_faces_model_version.clone()),
            user_id: self.user_id.clone(),
        }
    }
}

/// One user of a collection, as created by `CreateUser` and populated by
/// `AssociateFaces` (not implemented yet, so `face_ids` is empty for every user this
/// slice stores).
#[derive(Debug, Clone)]
pub struct StoredUser {
    /// `CreateUser.UserId` — the caller's own id for the user and the collection's key
    /// for it, unique within the collection.
    pub user_id: String,
    /// The user's `UserStatus`; see [`USER_STATUS_CREATED`].
    pub user_status: String,
    /// The faces `AssociateFaces` has associated with this user, by their `FaceId`.
    pub face_ids: Vec<String>,
}

impl StoredUser {
    /// A user as `CreateUser` records it: created, with no face associated with it,
    /// since a face is associated by its own operation.
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            user_status: USER_STATUS_CREATED.to_string(),
            face_ids: Vec::new(),
        }
    }

    /// This user as the modelled `User`: the id it was created under and the status it
    /// currently carries, which are the only two members the shape has
    /// (`smithy_query(query="shape",shape="User")`).
    pub fn modeled(&self) -> User {
        User {
            user_id: Some(self.user_id.clone()),
            user_status: Some(self.user_status.clone()),
        }
    }
}

/// One stored face collection: what `CreateCollection` was given, the ARN and
/// creation time the service recorded for it, and the faces indexed into it.
#[derive(Debug)]
pub struct Collection {
    pub collection_id: String,
    pub arn: String,
    pub face_model_version: String,
    pub creation_timestamp: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// Insertion-ordered: `FaceCount` is its length and `ListFaces` reports it in
    /// this order.
    pub faces: Vec<StoredFace>,
    /// Every user of the collection, keyed by its `UserId`. A `BTreeMap` so
    /// `ListUsers`' order, and therefore its pagination, is deterministic. Bounded by
    /// [`MAX_USERS_PER_COLLECTION`].
    pub users: BTreeMap<String, StoredUser>,
    /// The sequence number the next indexed face takes.
    pub next_sequence: u64,
}

impl Collection {
    /// A collection with no faces and no users, as `CreateCollection` records it.
    pub fn new(
        collection_id: String,
        arn: String,
        tags: BTreeMap<String, String>,
        creation_timestamp: SystemTime,
    ) -> Self {
        Self {
            collection_id,
            arn,
            face_model_version: FACE_MODEL_VERSION.to_string(),
            creation_timestamp,
            tags,
            faces: Vec::new(),
            users: BTreeMap::new(),
            next_sequence: 0,
        }
    }

    /// The stored face with `face_id`, if this collection holds it.
    pub fn face(&self, face_id: &str) -> Option<&StoredFace> {
        self.faces.iter().find(|face| face.face_id == face_id)
    }
}

/// One project recorded by `CreateProject`: the name it was created under, the ARN
/// this service minted for it, when it was created, what it is customized for, and the
/// tags the create attached to it.
///
/// A project is a container (`CreateProject`: "A project is a group of resources
/// (datasets, model versions)"), so nothing here measures or trains anything: the
/// record is what the caller asked for plus what the service assigned.
#[derive(Debug, Clone)]
pub struct Project {
    /// `CreateProject.ProjectName` — this API's identity for the project, and the
    /// store's key for it.
    pub name: String,
    /// The ARN this service minted for the project; what every operation ON a project
    /// names it by.
    pub arn: String,
    pub creation_timestamp: SystemTime,
    /// The project's `ProjectStatus`; see [`PROJECT_STATUS_CREATED`].
    pub status: String,
    /// The `CustomizationFeature` the project is customized for: what the create
    /// named, or [`DEFAULT_CUSTOMIZATION_FEATURE`] when it named none.
    pub feature: String,
    /// `CreateProject.AutoUpdate`, carried through unchanged — absent when the create
    /// named none, since nothing else decides whether a project auto-updates.
    pub auto_update: Option<String>,
    /// The tags the create attached to the project, and the set the operations that
    /// report and change a resource's tags work on when the ARN they are given is this
    /// project's: a tag set belongs to the resource it was attached to, whatever kind of
    /// resource that is.
    pub tags: BTreeMap<String, String>,
    /// The model versions this project manages, keyed by their `VersionName` — "Models and
    /// adapters are managed as part of a Rekognition project"
    /// (`smithy_query(query="operation",operation="CreateProjectVersion")`), so a version is
    /// a child of the project that holds it and goes when the project goes.
    ///
    /// A project created now manages none: a version is recorded by the operations that
    /// create one (`CreateProjectVersion`, `CopyProjectVersion`), each of which is an
    /// unimplemented 501 until it is written. What reads this map is the resource check a
    /// project policy's document is held to — see [`project_policies`].
    pub versions: BTreeMap<String, ProjectVersion>,
    /// The project policies attached to this project, keyed by their `PolicyName` — the
    /// project's identity for a policy, since "you can attach multiple project policies to
    /// a project" and a policy is updated by naming it again
    /// (`smithy_query(query="operation",operation="PutProjectPolicy")`). A `BTreeMap` so
    /// `ListProjectPolicies`' order, and therefore its pagination, is deterministic;
    /// bounded by [`project_policies::MAX_POLICIES_PER_PROJECT`].
    pub policies: BTreeMap<String, project_policies::ProjectPolicyRecord>,
}

/// One model version of a project: the name it was created under and the ARN this service
/// minted for it.
///
/// A version is what a project policy grants copying of, so what this record has to answer
/// is whether an ARN names a version this project holds. Everything a version reports about
/// its TRAINING belongs to the operations that create and describe one, each of which is an
/// unimplemented 501 until it is written; this record carries what identifies the version
/// and nothing a trainer would have had to measure.
#[derive(Debug, Clone)]
pub struct ProjectVersion {
    /// `CreateProjectVersion.VersionName` — the project's key for the version.
    pub name: String,
    /// The ARN this service minted for the version; what every operation ON a version, and
    /// every policy document that grants on one, names it by.
    pub arn: String,
    pub creation_timestamp: SystemTime,
}

impl ProjectVersion {
    /// A version as the operation that creates one records it.
    pub fn new(name: String, arn: String, creation_timestamp: SystemTime) -> Self {
        Self {
            name,
            arn,
            creation_timestamp,
        }
    }
}

impl Project {
    /// A project as `CreateProject` records it: created, with no datasets and no model
    /// versions, since each of those is created by its own operation.
    pub fn new(
        name: String,
        arn: String,
        feature: String,
        auto_update: Option<String>,
        tags: BTreeMap<String, String>,
        creation_timestamp: SystemTime,
    ) -> Self {
        Self {
            name,
            arn,
            creation_timestamp,
            status: PROJECT_STATUS_CREATED.to_string(),
            feature,
            auto_update,
            tags,
            versions: BTreeMap::new(),
            policies: BTreeMap::new(),
        }
    }

    /// Whether `arn` names a model version this project holds, in either form of the ARN
    /// (see [`arn_names`]).
    pub fn has_version_arn(&self, arn: &str) -> bool {
        self.versions
            .values()
            .any(|version| arn_names(&version.arn, arn))
    }

    /// This project as the modelled `ProjectDescription`.
    ///
    /// `Datasets` is the project's training and test datasets, of which it has none
    /// until `CreateDataset` records one, so the list is empty rather than absent: the
    /// project HAS a dataset list and this is its content.
    pub fn modeled(&self) -> ProjectDescription {
        ProjectDescription {
            project_arn: Some(self.arn.clone()),
            creation_timestamp: Some(::aws_smithy_types::DateTime::from(self.creation_timestamp)),
            status: Some(self.status.clone()),
            datasets: Some(Vec::new()),
            feature: Some(self.feature.clone()),
            auto_update: self.auto_update.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-rekognition-rekognitionservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRekognitionRekognitionserviceState {
    /// Every live collection, keyed by its `CollectionId` — the resource's identity
    /// in this API, and case-sensitive per `CreateCollection`'s documentation. A
    /// `BTreeMap` so `ListCollections` order, and therefore its pagination, is
    /// deterministic. Bounded by [`MAX_COLLECTIONS`].
    pub collections: BTreeMap<String, Collection>,
    /// Every live project, keyed by its `ProjectName` — the name a project is created
    /// under and the one `DescribeProjects` filters by, unique per account because a
    /// create that repeats one is refused. A `BTreeMap` for the same reason as
    /// `collections`: `DescribeProjects`' order, and therefore its pagination, is
    /// deterministic. Bounded by [`MAX_PROJECTS`].
    pub projects: BTreeMap<String, Project>,
    /// What each idempotency token this service has seen committed, so a retried
    /// request completes only once and a token reused for a different request is
    /// refused. Service-wide rather than per collection: a token names a REQUEST, and
    /// the request's own collection is one of the parameters compared.
    pub client_request_tokens: idempotency::TokenLedger,
    /// Every stored-video analysis job a `Start…` operation of this service has
    /// recorded, keyed by the `JobId` it minted. Service-wide and shared by the whole
    /// family, with each job naming the analysis that started it, so one `JobId` space
    /// serves every `Start…`/`Get…` pair rather than a map per operation. Bounded by
    /// [`video_jobs::MAX_REMEMBERED_JOBS`].
    pub video_jobs: video_jobs::VideoJobs,
}

impl ComAmazonawsRekognitionRekognitionserviceState {
    /// The tag set of the resource `resource_arn` names, in either form of its ARN (see
    /// [`arn_names`]), or `None` when no live resource of this store carries that ARN.
    ///
    /// Every taggable resource this store holds carries its own tag set, and one ARN
    /// names at most one resource, so an ARN is resolved across every kind of resource
    /// the store holds rather than one of them: that is what lets the tagging operations
    /// report and change the tags of whichever resource the caller named.
    pub fn tags_by_arn(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(collection) = self
            .collections
            .values()
            .find(|collection| arn_names(&collection.arn, resource_arn))
        {
            return Some(&collection.tags);
        }
        self.projects
            .values()
            .find(|project| arn_names(&project.arn, resource_arn))
            .map(|project| &project.tags)
    }

    /// The project `project_arn` names, in either form of its ARN (see [`arn_names`]), or
    /// `None` when this store holds no such project.
    ///
    /// Every operation ON a project names it by ARN while the store keys a project by its
    /// name, so the resolution lives here once rather than in each of those handlers.
    pub fn project_by_arn(&self, project_arn: &str) -> Option<&Project> {
        self.projects
            .values()
            .find(|project| arn_names(&project.arn, project_arn))
    }

    /// The same resolution, for the operations that CHANGE the project.
    pub fn project_by_arn_mut(&mut self, project_arn: &str) -> Option<&mut Project> {
        self.projects
            .values_mut()
            .find(|project| arn_names(&project.arn, project_arn))
    }

    /// The same resolution, for the operations that CHANGE the resource's tags.
    pub fn tags_by_arn_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(collection) = self
            .collections
            .values_mut()
            .find(|collection| arn_names(&collection.arn, resource_arn))
        {
            return Some(&mut collection.tags);
        }
        self.projects
            .values_mut()
            .find(|project| arn_names(&project.arn, resource_arn))
            .map(|project| &mut project.tags)
    }
}

/// The position of a project in the order `DescribeProjects` reports projects in —
/// documented on the output member itself
/// (`smithy_query(query="doc",shape="DescribeProjectsResponse",member="ProjectDescriptions")`:
/// "The list is sorted by the date and time the projects are created") — with the
/// project's name breaking the tie between two projects created in the same
/// millisecond, so the order is total and therefore deterministic.
///
/// The instant is rendered fixed-width so the keys sort as strings exactly as they sort
/// by creation instant. A page cursor is built from this key rather than from a project,
/// so a page still resumes at the right position when the project the token was minted
/// at has since been deleted.
pub fn project_order_key(project: &Project) -> String {
    let millis = project
        .creation_timestamp
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    format!("{millis:020}/{}", project.name)
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
///
/// The scope is caller-supplied and this value reaches both the ARNs this service reports
/// and the address an in-process read of Amazon S3 is sent to, so a value that is not
/// shaped like a Region identifier is not reflected: the real service would not have
/// served the request under a Region it does not have.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region identifier: two or more
/// hyphen-separated lowercase-letter parts followed by a digit group (`us-east-1`,
/// `ap-southeast-3`, `us-gov-west-1`, `cn-north-1`).
fn is_region_like(region: &str) -> bool {
    let Some((parts, index)) = region.rsplit_once('-') else {
        return false;
    };
    region.len() <= 32
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && parts.split('-').count() >= 2
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// The ARN of a collection in `region`:
/// `arn:aws:rekognition:<region>:<account>:collection/<CollectionId>`.
pub fn collection_arn(region: &str, collection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection/{collection_id}"),
    )
}

/// A collection's ARN as `CreateCollection` reports it: the stored ARN without the
/// leading `arn:` label.
///
/// Rekognition's `CreateCollection` answers with
/// `aws:rekognition:<region>:<account>:collection/<CollectionId>` — the form in that
/// operation's own published response example, and the one a recorded create answers with
/// — while `DescribeCollection`'s `CollectionARN` carries the full ARN. Only the create's
/// answer drops the label: the record keeps the canonical ARN, so no other operation has
/// to know about the difference, and [`arn_names`] is what lets the value the create
/// handed out still name its collection.
pub fn create_collection_arn_form(arn: &str) -> String {
    arn_identity(arn).to_string()
}

/// Whether `requested` names the resource whose stored ARN is `stored`.
///
/// The ARN a caller holds for a collection is the one `CreateCollection` handed it, which
/// omits the `arn:` label, and the tagging operations resolve that value, so an ARN is
/// matched on the part that identifies the resource. Nothing else matches: the partition,
/// service, region, account and resource path all still have to agree.
pub fn arn_names(stored: &str, requested: &str) -> bool {
    arn_identity(stored) == arn_identity(requested)
}

/// The part of an ARN that identifies the resource: everything after the optional `arn:`
/// label.
fn arn_identity(arn: &str) -> &str {
    arn.strip_prefix("arn:").unwrap_or(arn)
}

/// The ARN of a project in `region` created at `creation_timestamp`:
/// `arn:aws:rekognition:<region>:<account>:project/<ProjectName>/<version>`.
///
/// A project ARN ends in a numeric version segment the service assigns when it creates
/// the project (`ProjectArn`'s pattern ends `project\/[a-zA-Z0-9_.\-]{1,255}\/[0-9]+$`,
/// `smithy_query(query="operation",operation="CreateProject")`). It is the project's
/// creation instant in milliseconds, so the ARN a create answers with is derived from
/// the record it just wrote and stays that project's ARN for as long as the project
/// lives.
pub fn project_arn(region: &str, project_name: &str, creation_timestamp: SystemTime) -> String {
    let version = creation_timestamp
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("project/{project_name}/{version}"),
    )
}

/// The ARN of a model version named `version_name` of the project named `project_name` in
/// `region`, created at `creation_timestamp`:
/// `arn:aws:rekognition:<region>:<account>:project/<ProjectName>/version/<VersionName>/<version>`.
///
/// The form is the one `ProjectVersionArn`'s pattern requires — `project\/[a-zA-Z0-9_.\-]{1,255}\/version\/[a-zA-Z0-9_.\-]{1,255}\/[0-9]+$`
/// (`smithy_query(query="operation",operation="CreateProjectVersion")`) — with the trailing
/// numeric segment the version's creation instant in milliseconds, for the same reason
/// [`project_arn`]'s is: it is derived from the record the service wrote and stays that
/// version's ARN for as long as the version lives.
pub fn project_version_arn(
    region: &str,
    project_name: &str,
    version_name: &str,
    creation_timestamp: SystemTime,
) -> String {
    let version = creation_timestamp
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("project/{project_name}/version/{version_name}/{version}"),
    )
}

/// The cursor scope of each paginated list, so one list's token cannot be replayed
/// against another and a hex string the service never minted names no position.
pub const FACES_CURSOR_SCOPE: &str = "faces";
pub const COLLECTIONS_CURSOR_SCOPE: &str = "collections";
pub const PROJECTS_CURSOR_SCOPE: &str = "projects";
pub const USERS_CURSOR_SCOPE: &str = "users";

/// The pagination cursor of the `scope` list for a page that starts at `key`.
///
/// Hex of a scope-tagged key rather than the key itself, so a caller cannot read
/// structure into the token and has nothing to build one from: a `NextToken` is opaque
/// and only ever travels back unchanged, to the list that minted it.
pub fn encode_cursor(scope: &str, key: &str) -> String {
    ::hex::encode(format!("{scope}:{key}").as_bytes())
}

/// The start key a cursor minted by [`encode_cursor`] for `scope` carries, or `None`
/// when the token was not minted by that list of this service.
pub fn decode_cursor(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{scope}:"))
        .map(ToString::to_string)
}

/// The `ListFaces` cursor for a page that starts at the face with `sequence`.
pub fn encode_sequence_cursor(sequence: u64) -> String {
    encode_cursor(FACES_CURSOR_SCOPE, &sequence.to_string())
}

/// The sequence number a cursor minted by [`encode_sequence_cursor`] carries, or
/// `None` when the token was not minted by this service's `ListFaces`.
pub fn decode_sequence_cursor(token: &str) -> Option<u64> {
    decode_cursor(FACES_CURSOR_SCOPE, token)?.parse().ok()
}

/// The page size a `MaxResults` of `requested` asks for, capped to what the model
/// allows and defaulting to [`DEFAULT_PAGE_SIZE`] when the caller sent none.
///
/// `PageSize`'s range starts at 0, so a caller may ask for an empty page; see
/// [`page_ends_here`] for what a list then answers. A value that is not a page size at
/// all (a negative one, which the modelled range already refuses at the boundary) asks
/// for no entries rather than for the largest page.
pub fn page_size(requested: Option<i32>) -> usize {
    requested.map_or(DEFAULT_PAGE_SIZE, |max| {
        usize::try_from(max).unwrap_or(0).min(DEFAULT_PAGE_SIZE)
    })
}

/// Whether a list must end the iteration rather than mint a `NextToken`, given the
/// page size it served.
///
/// A `MaxResults` of 0 is inside the modelled range and asks for no entries, so the
/// answer carries none. No cursor is minted for it either: the cursor would name the
/// position the caller is already at, and a client that followed it would ask for
/// nothing again forever. Asking for zero entries is answered with zero entries and a
/// finished iteration.
pub fn page_ends_here(page_size: usize) -> bool {
    page_size == 0
}

/// The largest page `DescribeProjects` serves, and the one it serves when the caller
/// sends no `MaxResults`: the top of the modelled `ProjectsPageSize` range
/// (`range 1..100`, `smithy_query(query="operation",operation="DescribeProjects")`).
/// The model documents no other default, and the range's floor of 1 means a projects
/// page is never empty while projects remain.
pub const PROJECTS_PAGE_SIZE_MAX: usize = 100;

/// The page size a `MaxResults` of `requested` asks `DescribeProjects` for, held inside
/// the modelled range — which the boundary validator has already enforced, so this only
/// keeps the page size a page size for a value that reached here anyway.
pub fn projects_page_size(requested: Option<i32>) -> usize {
    requested.map_or(PROJECTS_PAGE_SIZE_MAX, |max| {
        usize::try_from(max)
            .unwrap_or(1)
            .clamp(1, PROJECTS_PAGE_SIZE_MAX)
    })
}

/// The largest page `ListUsers` serves, and the one it serves when the caller sends no
/// `MaxResults`: the top of the modelled `MaxUserResults` range (`range 1..500`,
/// `smithy_query(query="operation",operation="ListUsers")`). The model documents no
/// other default, and the range's floor of 1 means a users page is never empty while
/// users remain.
pub const USERS_PAGE_SIZE_MAX: usize = 500;

/// The page size a `MaxResults` of `requested` asks `ListUsers` for, held inside the
/// modelled range — which the boundary validator has already enforced, so this only
/// keeps the page size a page size for a value that reached here anyway.
pub fn users_page_size(requested: Option<i32>) -> usize {
    requested.map_or(USERS_PAGE_SIZE_MAX, |max| {
        usize::try_from(max).unwrap_or(1).clamp(1, USERS_PAGE_SIZE_MAX)
    })
}

/// What an `Image` identifies, as bytes to digest: the image bytes themselves, or
/// the S3 object's identity when the image is stored in S3 (its content is not
/// reachable from this process).
///
/// `None` when the member carries neither form, which is what makes a request
/// refusable: the model marks both members optional but the service needs one.
fn image_content_key(image: &Image) -> Option<Vec<u8>> {
    if let Some(bytes) = image.bytes.as_ref().filter(|bytes| !bytes.is_empty()) {
        let mut key = b"bytes:".to_vec();
        key.extend_from_slice(bytes);
        return Some(key);
    }
    let object = image.s3_object.as_ref()?;
    let bucket = object.bucket.as_deref().filter(|b| !b.is_empty())?;
    let name = object.name.as_deref().filter(|n| !n.is_empty())?;
    let version = object.version.as_deref().unwrap_or_default();
    Some(format!("s3:{bucket}/{name}#{version}").into_bytes())
}

/// The digest of an image's content: the identity `IndexFaces` records for the image
/// a face came from, so that two stored faces can be told apart from two stored faces
/// that came from the same image.
///
/// Domain-separated so an image passed as bytes and an S3 object whose key
/// happens to spell the same text never collide. It identifies an image; it says
/// nothing about what is in it.
pub fn image_signature(image: &Image) -> Option<[u8; 32]> {
    let key = image_content_key(image)?;
    let digest = Sha256::digest(&key);
    Some(digest.into())
}

/// How many ids this process has minted. Part of every minted id, so two ids are
/// distinct even when the clock below reports the same instant twice.
static MINTED_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh identifier in the canonical uuid form the model requires of a `FaceId`
/// (`pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), minted
/// from `domain`, this process's mint counter and the clock, and therefore unique:
/// the counter alone makes every input distinct, and the digest of distinct inputs
/// does not repeat. It names a resource for correlation and is not a secret.
fn mint_id(domain: &str) -> String {
    let digest = mint_digest(domain);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version and variant nibbles of a random-form uuid, so the id is a canonical
    // v4 uuid string and not merely 32 hex characters with hyphens.
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

/// The digest every identifier this service mints is built from: `domain`, this
/// process's mint counter and the clock. The counter alone makes every input distinct,
/// so the digests of two mints never repeat, whatever form the caller renders them in.
fn mint_digest(domain: &str) -> [u8; 32] {
    let serial = MINTED_IDS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    Sha256::digest(format!("{domain}:{serial}:{nanos}").as_bytes()).into()
}

/// A fresh `FaceId` for a face being indexed.
pub fn new_face_id() -> String {
    mint_id("face")
}

/// A fresh `ImageId` for the image an `IndexFaces` call submitted.
pub fn new_image_id() -> String {
    mint_id("image")
}

/// A fresh `JobId` for a stored-video analysis job a `Start…` operation records.
///
/// The model constrains a `JobId` to `length 1..64` and
/// `pattern ^[a-zA-Z0-9-_]+$` (`smithy_query(query="shape",shape="JobId")`), and the
/// identifier Rekognition hands out for a video job is a long opaque hexadecimal token
/// rather than a uuid, so this renders the whole mint digest as hex: 64 characters, the
/// longest the constraint admits, unique for the same reason [`mint_digest`] is. It
/// names a job for correlation and is not a secret.
pub fn new_job_id() -> String {
    ::hex::encode(mint_digest("job"))
}

/// A fresh `PolicyRevisionId` for a project policy being written.
///
/// The model constrains it to `length 0..64` and `pattern ^[0-9A-Fa-f]+$`
/// (`smithy_query(query="shape",shape="ProjectPolicyRevisionId")`), and the revision
/// Rekognition assigns a project policy is a 32-character hexadecimal token, so this
/// renders half the mint digest as hex. It is unique for the same reason [`mint_digest`] is,
/// which is what lets a write that names a stale revision be told from one that names the
/// current one. It identifies a revision of a policy and is not a secret.
pub fn new_project_policy_revision_id() -> String {
    ::hex::encode(&mint_digest("project-policy-revision")[..16])
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRekognitionRekognitionserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRekognitionRekognitionserviceState>)
}
