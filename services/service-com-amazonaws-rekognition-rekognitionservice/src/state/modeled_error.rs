// SPDX-License-Identifier: CC0-1.0

//! How this service builds the modelled exceptions it answers a refused request with:
//! the members every error body carries, and the wording it words the recurring
//! refusals with.
//!
//! Each of this model's exception shapes carries `Message`, `Code` and `Logref`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException")`), and an error body
//! this service sends names the failure TWICE: once in the protocol envelope's `__type`
//! and once in the shape's own `Code` member, both as the exception's shape name. A
//! client that classifies a failure from the body's `Code` has to reach the same class
//! as one that reads the envelope, so neither may be left out. `Logref` is the request
//! identifier the real service correlates its own logs by; this emulator mints no such
//! identifier, so that member stays absent.
//!
//! Every handler constructs its modelled errors through [`ModeledError::with_message`]
//! or one of the named constructors below, so the rule holds for all of them rather
//! than one handler at a time.

use crate::ops::types::*;

/// A modelled exception of this service, constructed with every member the service
/// fills in.
pub trait ModeledError: Sized {
    /// This exception's shape name: the value of its `Code` member, and the value the
    /// error envelope carries in `__type`.
    const CODE: &'static str;

    /// This exception carrying `message`, with `Code` set to [`Self::CODE`].
    fn with_message(message: impl Into<String>) -> Self;
}

/// Implements [`ModeledError`] for each named exception shape. The shape's name is the
/// Rust type's name — the generated types in `ops::types` are one per modelled shape,
/// named after it — and every member beyond `Message` and `Code` is left as the shape's
/// default, i.e. absent.
macro_rules! impl_modeled_error {
    ($($exception:ident),+ $(,)?) => {
        $(
            impl ModeledError for $exception {
                const CODE: &'static str = stringify!($exception);

                fn with_message(message: impl Into<String>) -> Self {
                    Self {
                        message: Some(message.into()),
                        code: Some(Self::CODE.to_string()),
                        ..Self::default()
                    }
                }
            }
        )+
    };
}

// Every exception shape this service models, so an operation implemented later builds
// its own errors the same way the collection and image operations build theirs.
impl_modeled_error!(
    AccessDeniedException,
    ConflictException,
    HumanLoopQuotaExceededException,
    IdempotentParameterMismatchException,
    ImageTooLargeException,
    InternalServerError,
    InvalidImageFormatException,
    InvalidManifestException,
    InvalidPaginationTokenException,
    InvalidParameterException,
    InvalidPolicyRevisionIdException,
    InvalidS3ObjectException,
    LimitExceededException,
    MalformedPolicyDocumentException,
    ProvisionedThroughputExceededException,
    ResourceAlreadyExistsException,
    ResourceInUseException,
    ResourceNotFoundException,
    ResourceNotReadyException,
    ServiceQuotaExceededException,
    SessionNotFoundException,
    ThrottlingException,
    VideoTooLargeException,
);

/// Why a handler could not reach this service's store: a server fault, since nothing
/// about the request caused it.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the Rekognition collection store is unavailable";

/// The fixed text a request naming a `FaceId` the collection does not hold is refused
/// with. A face id the collection does not hold is a parameter value the service
/// rejects — the collection it names exists — so the class is
/// `InvalidParameterException` rather than `ResourceNotFoundException`, which this API
/// keeps for the collection itself being absent. The text names no id: it interpolates
/// nothing from the request.
pub const FACE_ID_NOT_IN_COLLECTION_MESSAGE: &str = "faceId was not found in the collection.";

/// The pagination token refusal: a `NextToken` this service's lists did not mint names
/// no position in them.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token";

/// AWS's fixed wording for the `InvalidParameterException` class: "Input parameter
/// violated a constraint" is what the exception SHAPE documents, while the `Message` the
/// service puts on the wire for it is this one sentence. It interpolates nothing — neither
/// a request value nor the member at fault — so every refusal of this class carries it
/// byte for byte.
pub const INVALID_PARAMETERS_MESSAGE: &str = "Request has invalid parameters";

/// The fixed text an analysis that needs a face to work on is refused with when the
/// image it was given carries none — `CompareFaces` "returns an InvalidParameterException
/// error" when "no faces are detected in the source or target images". It is AWS's
/// wording for that class and interpolates nothing: neither which of the two images it
/// was nor anything else from the request, both of which the caller already knows.
pub const NO_FACE_IN_IMAGE_MESSAGE: &str = INVALID_PARAMETERS_MESSAGE;

/// The server fault a handler answers with when this service's store cannot be reached.
pub fn store_unavailable() -> InternalServerError {
    InternalServerError::with_message(STORE_UNAVAILABLE_MESSAGE)
}

/// A collection the store does not hold, as the operations ON a collection as a
/// resource — `DescribeCollection` and `DeleteCollection` — word it:
/// `Collection:<CollectionId> not found!`, the request's own id between two pieces of
/// fixed text.
pub fn collection_not_found(collection_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException::with_message(format!("Collection:{collection_id} not found!"))
}

/// A collection the store does not hold, as the operations on the FACES of a collection
/// — `IndexFaces`, `ListFaces`, `DeleteFaces`, `SearchFaces` — word it. The two paths
/// word the same class differently on the wire, so each operation answers with the
/// wording the service answers it with rather than with one invented for both.
pub fn collection_id_does_not_exist(collection_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException::with_message(format!(
        "The collection id: {collection_id} does not exist"
    ))
}

/// A `FaceId` the named collection does not hold; see
/// [`FACE_ID_NOT_IN_COLLECTION_MESSAGE`] for why this is a parameter refusal.
pub fn face_id_not_in_collection() -> InvalidParameterException {
    InvalidParameterException::with_message(FACE_ID_NOT_IN_COLLECTION_MESSAGE)
}

/// A `NextToken` no list of this service minted.
pub fn invalid_pagination_token() -> InvalidPaginationTokenException {
    InvalidPaginationTokenException::with_message(INVALID_PAGINATION_TOKEN_MESSAGE)
}

/// A resource ARN no live resource of this store carries, as the tagging operations word
/// it: the request's own ARN between two pieces of fixed text. One wording for all three,
/// since they resolve the resource the same way.
pub fn resource_arn_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException::with_message(format!("The resource {resource_arn} does not exist"))
}

/// A submitted image in which the analysis found no face to work on; see
/// [`NO_FACE_IN_IMAGE_MESSAGE`].
pub fn no_face_in_image() -> InvalidParameterException {
    InvalidParameterException::with_message(NO_FACE_IN_IMAGE_MESSAGE)
}

/// A request whose parameters do not identify what the operation is to work on; see
/// [`INVALID_PARAMETERS_MESSAGE`].
pub fn invalid_parameters() -> InvalidParameterException {
    InvalidParameterException::with_message(INVALID_PARAMETERS_MESSAGE)
}

/// A `UserId` the named collection already holds, as `CreateUser` refuses it: the
/// request's own id between two pieces of fixed text, worded the way this service words
/// the same class for a collection id a create repeats ("The collection id: … already
/// exists"), since both say "the id you asked to create is taken".
///
/// The class is `ConflictException`, which this API models for exactly this: "A User
/// with the same Id already exists within the collection"
/// (`smithy_query(query="doc",shape="ConflictException")`).
pub fn user_id_already_exists(user_id: &str) -> ConflictException {
    ConflictException::with_message(format!("The user id: {user_id} already exists"))
}

/// The fixed text an idempotency-token reuse is refused with.
///
/// It describes none of the request, and it does not restate the rule — "a
/// `ClientRequestToken` input parameter was reused with an operation, but at least one
/// of the other input parameters is different from the previous call" is what the
/// exception SHAPE documents
/// (`smithy_query(query="doc",shape="IdempotentParameterMismatchException")`), not what
/// the service puts in `Message`. This is the `Message` a recorded AWS `CreateUser`
/// refusal of a reused token carries verbatim (the recorded exchange this refusal was
/// implemented against, `rekognition.create-user-idempotent-token-mismatch-rejected`),
/// and the wire message is part of the contract, so the service answers what AWS
/// answers rather than a better-worded sentence of its own. It interpolates nothing:
/// the caller sent both requests and so already holds every value that differs.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str = "Service Unavailable";

/// A `ClientRequestToken` reused for a request that differs from the one it already
/// committed; one wording for every operation that takes one, since the rule is one.
pub fn idempotent_parameter_mismatch() -> IdempotentParameterMismatchException {
    IdempotentParameterMismatchException::with_message(IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE)
}
