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
//! recogniser. The RECOGNISER does not exist in this process: nothing here INFERS the
//! identity of a photographed face, so no face is tied to a person this emulator was not
//! told about. What IS real is
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
//! need a trained model of WHO a face belongs to stay unimplemented 501s.
//!
//! The same rule governs the LABELS of a submitted image in [`label_detection`]: no
//! classifier over a label taxonomy runs here, so the entities it reports are the ones the
//! same geometric detector measured — a face, and the head and the person it is part of —
//! while the
//! properties of the image itself (its quality, its dominant colours and the two sides of
//! its foreground/background split) are measured off the decoded pixels and reported in
//! full.
//!
//! Where an operation PARTITIONS the faces of a submitted image by an IDENTITY, the identity
//! is one this service HOLDS rather than one inferred from the pixels:
//! `RecognizeCelebrities` resolves each face the detector measured against the seeded
//! celebrity catalogue in [`celebrity_catalogue`] — the entry whose reference face it
//! resembles most, by the same measured correlation `CompareFaces` reports — and reports it as
//! that catalogue entry's celebrity, with the resemblance measured for it as the match
//! confidence, or on the unrecognized side when it resembles no entry of the catalogue at all.
//! `GetCelebrityInfo` resolves an `Id` the catalogue holds back to the same entry. A USER of
//! a collection is such an identity too — it IS the faces `AssociateFaces` put on it — so
//! `SearchUsersByImage` resolves the face it searched against the users the named collection
//! holds, by the same measured resemblance, through [`user_search`].
//! `DetectProtectiveEquipment` answers the same way about the PERSONS
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
//! operation reads that job by, with the job's status owed by the clock. That start is one
//! sequence for the whole family, so it lives once in [`video_analysis`], and REPORTING the
//! job a start recorded is one sequence too, so it lives once in [`video_report`]: a `Get…`
//! operation resolves the identifier to a job of its own analysis, reports the status the
//! clock owes it and serves its results a page at a time. Those results are EMPTY — nothing
//! in this process decodes a video, so no detection is reported that nothing measured.
//!
//! The one comparison this emulator can make about two STORED faces is IDENTITY:
//! [`image_signature`] digests what an image is, so two indexed faces can be told to have
//! come from the same image content or not. `SearchFaces` reports exactly that and no
//! graded score — see its own module docs.

pub mod celebrity_catalogue;
pub mod dataset_source;
pub mod datasets;
pub mod face_association;
pub mod face_detection;
pub mod idempotency;
pub mod image_source;
pub mod label_detection;
pub mod label_taxonomy;
pub mod modeled_error;
pub mod project_policies;
pub mod user_search;
pub mod video_analysis;
pub mod video_jobs;
pub mod video_report;
pub mod video_source;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
/// about it. `CREATING`, the third value the model has (`smithy_query(query="shape",
/// shape="ProjectStatus")`: CREATING | CREATED | DELETING), belongs to a transition no
/// operation of this service starts.
pub const PROJECT_STATUS_CREATED: &str = "CREATED";

/// The `ProjectStatus` a project carries from the call that asked for its deletion until
/// the clock has taken it out of this store.
///
/// Deleting a project "is an asynchronous operation. To check if the project is deleted,
/// call DescribeProjects. The project is deleted when the project no longer appears in the
/// response" (`smithy_query(query="operation",operation="DeleteProject")`), so a project
/// whose deletion has been asked for is still one of the account's and reports this status
/// while it goes.
pub const PROJECT_STATUS_DELETING: &str = "DELETING";

/// How long after the call that asked for it a project's deletion is complete — the span
/// after which the project is no longer one of the account's and so is reported by nothing.
///
/// The delete answers while the deletion is still going, which is what makes the `Status`
/// it answers with worth reporting, so the span is never zero: a caller that describes the
/// project straight after deleting it still sees it, in [`PROJECT_STATUS_DELETING`]. AWS
/// takes as long as it takes; this emulator owes the transition at a span of its own
/// choosing, the same one a video analysis of this service completes in
/// ([`video_jobs::COMPLETION_SPAN`]). The instant it falls due is STORED rather than waited
/// for: nothing sleeps.
pub const PROJECT_DELETION_SPAN: Duration = Duration::from_secs(3);

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

/// The `UserStatus` values a user of this store carries; see [`StoredUser::status_at`]
/// for which of them it carries when.
///
/// `UserStatus` "communicates if the UserID has been updated with latest set of faces
/// to be associated with the UserID"
/// (`smithy_query(query="shape",shape="User")`), and its values are
/// ACTIVE | UPDATING | CREATING | CREATED
/// (`smithy_query(query="shape",shape="UserStatus")`), documented as "ACTIVE - All
/// associations or disassociations of FaceID(s) for a UserID are complete. CREATED - A
/// UserID has been created, but has no FaceID(s) associated with it. UPDATING - A UserID
/// is being updated and there are current associations or disassociations of FaceID(s)
/// taking place" (`smithy_query(query="doc",shape="AssociateFaces")`). `CREATING` belongs
/// to a transition no operation of this service starts: a `CreateUser` that answers has
/// recorded the user.
pub const USER_STATUS_CREATED: &str = "CREATED";
pub const USER_STATUS_UPDATING: &str = "UPDATING";
pub const USER_STATUS_ACTIVE: &str = "ACTIVE";

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
    /// The user this face is associated with, set by `AssociateFaces` and cleared when the
    /// face is taken off that user again, and `None` for a face no user holds. It is
    /// honoured by `ListFaces`' `UserId` filter and `DeleteFaces`'
    /// `ASSOCIATED_TO_AN_EXISTING_USER` refusal, so one record answers for the face
    /// wherever the association shows. `DeleteUser` CLEARS it for every face of the user it
    /// removes, which is the disassociation that operation documents: the face outlives
    /// the user.
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

/// One user of a collection, as created by `CreateUser`, populated by `AssociateFaces`
/// and removed by `DeleteUser`.
#[derive(Debug, Clone)]
pub struct StoredUser {
    /// `CreateUser.UserId` — the caller's own id for the user and the collection's key
    /// for it, unique within the collection.
    pub user_id: String,
    /// The faces `AssociateFaces` has associated with this user, by their `FaceId`, in
    /// the order they were associated. Bounded by
    /// [`face_association::MAX_FACES_PER_USER`].
    pub face_ids: Vec<String>,
    /// When the update of this user's faces that the last `AssociateFaces` started is
    /// complete, and `None` while no call has started one. The instant is STORED rather
    /// than waited for: nothing sleeps, and the status below is computed from the clock
    /// whenever it is asked for.
    pub updating_until: Option<SystemTime>,
}

impl StoredUser {
    /// A user as `CreateUser` records it: created, with no face associated with it,
    /// since a face is associated by its own operation.
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            face_ids: Vec::new(),
            updating_until: None,
        }
    }

    /// Record that an update of this user's faces started at `now`, due to be complete one
    /// [`face_association::UPDATE_SPAN`] later.
    pub fn start_update(&mut self, now: SystemTime) {
        self.updating_until = Some(now + face_association::UPDATE_SPAN);
    }

    /// This user's `UserStatus` at `now`: updating until the instant the clock owes the
    /// completion of the update its faces are under, created while no face has been
    /// associated with it, and active once every association of its faces is complete.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        match self.updating_until {
            Some(until) if now < until => USER_STATUS_UPDATING,
            _ if self.face_ids.is_empty() => USER_STATUS_CREATED,
            _ => USER_STATUS_ACTIVE,
        }
    }

    /// This user as the modelled `User`: the id it was created under and the status it
    /// carries at `now`, which are the only two members the shape has
    /// (`smithy_query(query="shape",shape="User")`).
    pub fn modeled_at(&self, now: SystemTime) -> User {
        User {
            user_id: Some(self.user_id.clone()),
            user_status: Some(self.status_at(now).to_string()),
        }
    }

    /// This user as the modelled `User` as of this instant; see [`Self::modeled_at`]. The
    /// status is a function of the clock, so a reader asks for it when it reports the
    /// user rather than reading a value some earlier call left behind.
    pub fn modeled(&self) -> User {
        self.modeled_at(SystemTime::now())
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
    /// When the deletion `DeleteProject` started is complete — the instant from which this
    /// project is no longer one of the account's — and `None` while no call has asked for
    /// one. The instant is STORED rather than waited for: nothing sleeps, and both the
    /// status this project reports and whether it still exists at all are computed from the
    /// clock whenever they are asked for. See [`PROJECT_DELETION_SPAN`].
    pub deleting_until: Option<SystemTime>,
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
    /// unimplemented 501 until it is written, and read back by the operations that describe
    /// one. A project policy's document is NOT held to it: what a policy grants on is a
    /// version ARN of the project, which may name a version created after the policy — see
    /// [`project_policies`].
    pub versions: BTreeMap<String, ProjectVersion>,
    /// The project policies attached to this project, keyed by their `PolicyName` — the
    /// project's identity for a policy, since "you can attach multiple project policies to
    /// a project" and a policy is updated by naming it again
    /// (`smithy_query(query="operation",operation="PutProjectPolicy")`). A `BTreeMap` so
    /// `ListProjectPolicies`' order, and therefore its pagination, is deterministic;
    /// bounded by [`project_policies::MAX_POLICIES_PER_PROJECT`].
    pub policies: BTreeMap<String, project_policies::ProjectPolicyRecord>,
    /// The Custom Labels datasets this project holds, keyed by their `DatasetType` — the
    /// project's identity for one, a project having at most one training and one test dataset
    /// ([`datasets`]). A `BTreeMap` so the `Datasets` list a `ProjectDescription` carries is
    /// in a deterministic order; bounded by [`datasets::MAX_DATASETS_PER_PROJECT`], which is
    /// that same one-per-type rule.
    ///
    /// A project created now holds none: a dataset is recorded by `CreateDataset` and goes
    /// when the project goes, being a child of the record that goes.
    pub datasets: BTreeMap<String, datasets::Dataset>,
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
            deleting_until: None,
            feature,
            auto_update,
            tags,
            versions: BTreeMap::new(),
            policies: BTreeMap::new(),
            datasets: BTreeMap::new(),
        }
    }

    /// Record that the deletion of this project started at `now`, due to be complete one
    /// [`PROJECT_DELETION_SPAN`] later.
    ///
    /// A project already being deleted keeps the instant the first call owed it: asking for
    /// a deletion that is already under way does not postpone it.
    pub fn start_deletion(&mut self, now: SystemTime) {
        self.deleting_until
            .get_or_insert(now + PROJECT_DELETION_SPAN);
    }

    /// Whether a deletion of this project has been asked for.
    pub fn is_deleting(&self) -> bool {
        self.deleting_until.is_some()
    }

    /// Whether the clock has completed this project's deletion by `now`: from that instant
    /// the project is one the account no longer has, so no reader reports it and no ARN
    /// resolves to it.
    pub fn is_deleted_at(&self, now: SystemTime) -> bool {
        self.deleting_until.is_some_and(|until| now >= until)
    }

    /// This project's `ProjectStatus`: deleting once a call has asked for its deletion,
    /// and created otherwise. A project the clock has finished deleting is reported by
    /// nothing, so no reader sees a status past that instant.
    pub fn status(&self) -> &'static str {
        if self.is_deleting() {
            PROJECT_STATUS_DELETING
        } else {
            PROJECT_STATUS_CREATED
        }
    }

    /// This project as the modelled `ProjectDescription` at `now`.
    ///
    /// `Datasets` is the project's training and test datasets, each summarised as the
    /// `DatasetMetadata` that list is modelled as — so a dataset `CreateDataset` recorded is
    /// reported here under the status the clock owes it, and a project holding none reports an
    /// empty list rather than an absent one: the project HAS a dataset list and this is its
    /// content.
    pub fn modeled_at(&self, now: SystemTime) -> ProjectDescription {
        ProjectDescription {
            project_arn: Some(self.arn.clone()),
            creation_timestamp: Some(::aws_smithy_types::DateTime::from(self.creation_timestamp)),
            status: Some(self.status().to_string()),
            datasets: Some(
                self.datasets
                    .values()
                    .map(|dataset| dataset.summarised_at(now))
                    .collect(),
            ),
            feature: Some(self.feature.clone()),
            auto_update: self.auto_update.clone(),
        }
    }

    /// The dataset of this project whose ARN is `dataset_arn` and whose deletion the clock has
    /// not completed by `now`, or `None` where it holds none under it. A dataset is addressed by
    /// ARN alone by every operation on one, so the lookup is over the project's own datasets
    /// rather than over the key they are held under.
    ///
    /// A dataset past the instant its deletion was due is one this account no longer has — "if
    /// you try to access the dataset after it is deleted, you get a `ResourceNotFoundException`
    /// exception" — so it is not one this resolves to, whether or not a call has dropped its
    /// record yet. That is the same rule [`ComAmazonawsRekognitionRekognitionserviceState::live_projects_at`]
    /// applies one level up.
    pub fn dataset_by_arn(
        &self,
        dataset_arn: &str,
        now: SystemTime,
    ) -> Option<&datasets::Dataset> {
        self.datasets
            .values()
            .find(|dataset| !dataset.is_deleted_at(now) && arn_names(&dataset.arn, dataset_arn))
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
    /// Every project this account still has, keyed by its `ProjectName` — the name a
    /// project is created under and the one `DescribeProjects` filters by, unique per
    /// account because a create that repeats one is refused. A `BTreeMap` for the same
    /// reason as `collections`: `DescribeProjects`' order, and therefore its pagination, is
    /// deterministic. Bounded by [`MAX_PROJECTS`].
    ///
    /// A project whose deletion the clock has completed is no longer one of them, whether
    /// or not it has been dropped from the map yet, so a project of this map is reached
    /// through [`Self::live_projects`] or dropped by
    /// [`Self::forget_deleted_projects`] rather than read straight out of it.
    pub projects: BTreeMap<String, Project>,
    /// What each idempotency token this service has seen committed, so a retried
    /// request completes only once and a token reused for a different request is
    /// refused. Service-wide rather than per collection: a token names a REQUEST, and
    /// the request's own collection is one of the parameters compared.
    pub client_request_tokens: idempotency::TokenLedger,
    /// The celebrities this service holds, seeded once with this emulator's own catalogue
    /// and read — never written — by a call: `RecognizeCelebrities` resolves a measured
    /// face against it, and `GetCelebrityInfo` resolves the `Id` it reported back to the
    /// same entry, so one catalogue answers both halves of that documented chain. See
    /// [`celebrity_catalogue`].
    pub celebrities: celebrity_catalogue::CelebrityCatalogue,
    /// Every stored-video analysis job a `Start…` operation of this service has
    /// recorded, keyed by the `JobId` it minted. Service-wide and shared by the whole
    /// family, with each job naming the analysis that started it, so one `JobId` space
    /// serves every `Start…`/`Get…` pair rather than a map per operation. Bounded by
    /// [`video_jobs::MAX_REMEMBERED_JOBS`].
    pub video_jobs: video_jobs::VideoJobs,
    /// What each `AssociateFaces` call that carried an idempotency token answered, so the
    /// same token answers with it again rather than with what the store says now. Keyed
    /// by the token alone and service-wide, like the parameter ledger above. Bounded by
    /// [`idempotency::MAX_REMEMBERED_TOKENS`].
    pub association_answers: face_association::AssociationAnswers,
    /// What each `DisassociateFaces` call that carried an idempotency token answered, kept
    /// for the same reason and bounded the same way as the association answers above. A
    /// ledger per operation, since a token names a request to ONE operation.
    pub disassociation_answers: face_association::DisassociationAnswers,
}

impl ComAmazonawsRekognitionRekognitionserviceState {
    /// Every project this account still has at `now`: the projects of the store the clock
    /// has not finished deleting.
    ///
    /// Deleting a project is asynchronous, so a deletion asked for becomes a deletion done
    /// at an instant the clock decides ([`PROJECT_DELETION_SPAN`]) and not at a call. One
    /// filter therefore answers for every reader of this store: a project past that instant
    /// is gone whether or not a later call has dropped its record.
    pub fn live_projects_at(&self, now: SystemTime) -> impl Iterator<Item = &Project> {
        self.projects
            .values()
            .filter(move |project| !project.is_deleted_at(now))
    }

    /// Every project this account still has as of this instant; see
    /// [`Self::live_projects_at`].
    pub fn live_projects(&self) -> impl Iterator<Item = &Project> {
        self.live_projects_at(SystemTime::now())
    }

    /// Drop the record of every project whose deletion the clock has completed by `now`.
    ///
    /// The removal is owed by the clock alone — it was due at the same instant whether or
    /// not anybody called — so a reader may materialise it, and that is what frees the
    /// project's name and its room in the store for a later create.
    pub fn forget_deleted_projects(&mut self, now: SystemTime) {
        self.projects
            .retain(|_, project| !project.is_deleted_at(now));
    }

    /// Drop the record of every DATASET whose deletion the clock has completed by `now`.
    ///
    /// The same rule as [`Self::forget_deleted_projects`], one level down: a dataset
    /// `DeleteDataset` started removing is gone one [`datasets::DATASET_DELETION_SPAN`] later
    /// whether or not anybody called, and dropping it is what frees the `DatasetType` its
    /// project held for a later create — "if you try to access the dataset after it is
    /// deleted, you get a `ResourceNotFoundException` exception".
    pub fn forget_deleted_datasets(&mut self, now: SystemTime) {
        for project in self.projects.values_mut() {
            project
                .datasets
                .retain(|_, dataset| !dataset.is_deleted_at(now));
        }
    }

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
        if let Some(project) = self.project_by_arn(resource_arn) {
            return Some(&project.tags);
        }
        // A dataset is taggable too — `CreateDataset` takes a `TagMap` of its own — and its
        // ARN names it as unambiguously as a project's names a project, so one ARN still
        // resolves to at most one resource.
        self.dataset_by_arn(resource_arn)
            .map(|(_, dataset)| &dataset.tags)
    }

    /// The project holding the dataset `dataset_arn` names, and that dataset, or `None` when
    /// this account has no such dataset.
    ///
    /// A dataset is addressed by its own ARN by every operation on one while the store holds
    /// it on the project it belongs to, so the resolution lives here once rather than in each
    /// of those handlers — and only the LIVE projects are searched, for the reason
    /// [`Self::project_by_arn`] records: a dataset of a project this account no longer has is
    /// one nothing holds.
    pub fn dataset_by_arn(&self, dataset_arn: &str) -> Option<(&Project, &datasets::Dataset)> {
        let now = SystemTime::now();
        self.live_projects_at(now).find_map(|project| {
            project
                .dataset_by_arn(dataset_arn, now)
                .map(|held| (project, held))
        })
    }

    /// The project `project_arn` names, in either form of its ARN (see [`arn_names`]), or
    /// `None` when this account has no such project.
    ///
    /// Every operation ON a project names it by ARN while the store keys a project by its
    /// name, so the resolution lives here once rather than in each of those handlers — and
    /// so does the one rule about which projects an ARN may resolve to, the live ones of
    /// [`Self::live_projects`].
    pub fn project_by_arn(&self, project_arn: &str) -> Option<&Project> {
        self.live_projects()
            .find(|project| arn_names(&project.arn, project_arn))
    }

    /// The same resolution, for the operations that CHANGE the project.
    pub fn project_by_arn_mut(&mut self, project_arn: &str) -> Option<&mut Project> {
        let now = SystemTime::now();
        self.projects
            .values_mut()
            .filter(|project| !project.is_deleted_at(now))
            .find(|project| arn_names(&project.arn, project_arn))
    }

    /// The project holding the dataset `dataset_arn` names, for the operations that CHANGE
    /// that dataset; see [`Self::dataset_by_arn`].
    pub fn project_holding_dataset_mut(&mut self, dataset_arn: &str) -> Option<&mut Project> {
        let now = SystemTime::now();
        self.projects
            .values_mut()
            .filter(|project| !project.is_deleted_at(now))
            .find(|project| project.dataset_by_arn(dataset_arn, now).is_some())
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
        let now = SystemTime::now();
        // One walk of the live projects answers both kinds: the project the ARN names, and the
        // dataset of a project it names. Two walks would be two mutable borrows of the same
        // map, and one resource is what an ARN names either way.
        self.projects
            .values_mut()
            .filter(|project| !project.is_deleted_at(now))
            .find_map(|project| {
                if arn_names(&project.arn, resource_arn) {
                    return Some(&mut project.tags);
                }
                project
                    .datasets
                    .values_mut()
                    .find(|dataset| {
                        !dataset.is_deleted_at(now) && arn_names(&dataset.arn, resource_arn)
                    })
                    .map(|dataset| &mut dataset.tags)
            })
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
