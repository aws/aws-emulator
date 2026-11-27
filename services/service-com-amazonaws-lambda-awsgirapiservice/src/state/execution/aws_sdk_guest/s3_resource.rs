// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE interface of the AWS SDK a Python handler imports, for Amazon S3 — the
//! `boto3.resource("s3")` service resource, its `Bucket` and `Object` objects, and the
//! COLLECTION a bucket's objects are reached through — and the MANAGED TRANSFER the SDK's own
//! S3 customizations run an upload or a download through.
//!
//! The managed `python3.x` runtimes ship the whole AWS SDK for Python, and S3's resource
//! interface is the one an application that walks or filters storage reaches for: a `Bucket`
//! carries its name for you, an `Object` carries its bucket and its key, and
//! `bucket.objects.filter(Prefix=...)` is an ITERABLE that pages the listing through rather
//! than a call whose continuation token the caller has to carry.
//!
//! What these modules do, over the SAME client every other call of this seam is made through
//! ([`super::super::aws_api`]):
//!
//! * every call is the CLIENT's own — `bucket.objects.filter(...)` is `ListObjectsV2` paged by
//!   its own continuation member, `object.get()` is `GetObject`, `bucket.delete()` is
//!   `DeleteBucket` — so the request, the signature, the destination, the bounds and the
//!   refusal classes are the ones a client-side call of the same operation is made under, and
//!   a service's answer is the service's own;
//! * a resource's own attributes are what the service said about it, under the names the SDK
//!   reports them by (`e_tag`, `last_modified`, `content_length`): the ones a LISTING carried
//!   for an object it yielded, and otherwise the ones a read of that resource answers, fetched
//!   when one is first asked for;
//! * a collection honours `limit` and `page_size` the way the SDK's own does — `page_size`
//!   asks the listing for pages of that size, `limit` bounds how many objects the iteration
//!   yields — and `filter(...)` passes the listing's own input members, so a member the seam
//!   does not bind is reported as a call that was not made rather than dropped;
//! * a TRANSFER is the managed transfer library's ([`super::s3transfer`]), configured by the
//!   `TransferConfig` a caller states: an object inside the configured threshold is sent in
//!   one `PutObject` and one over it as the parts of a multipart upload, which is why the
//!   client's own `upload_fileobj`/`download_fileobj`/`upload_file`/`download_file` are run
//!   through the manager rather than written out as a single call.
//!
//! Nothing here is the containment, and nothing here holds state of its own.

/// The modules of the Amazon S3 half of the SDK, in the order they are put on the import
/// path.
#[must_use]
pub(super) fn modules() -> Vec<(&'static str, &'static str)> {
    vec![
        ("boto3/s3/__init__.py", S3_SOURCE),
        ("boto3/s3/transfer.py", S3_TRANSFER_SOURCE),
        ("boto3/s3/resource.py", S3_RESOURCE_SOURCE),
    ]
}

/// `boto3.s3`: the package Amazon S3's own half of the SDK lives under.
const S3_SOURCE: &str = r##"
"""Amazon S3's own half of the SDK: its resources, and the transfers of its objects."""
"##;

/// `boto3.s3.transfer`: the configuration a managed transfer is run under, and the manager
/// that runs one.
///
/// The client's own transfer methods are performed through this: WHICH calls a transfer makes
/// is the configuration's to decide, so they are stated here once rather than per method.
const S3_TRANSFER_SOURCE: &str = r##"
"""The managed transfer of one object, and the configuration it is run under.

A transfer is not an operation: the SDK adds `upload_fileobj`, `download_fileobj`,
`upload_file` and `download_file` to its Amazon S3 client and performs each as the calls the
managed transfer library makes — one `PutObject` or `GetObject` for an object inside the
configured threshold, and `CreateMultipartUpload`/`UploadPart`/`CompleteMultipartUpload` for
one over it. What decides which of those a transfer makes is the `TransferConfig` it is given,
which is why a caller states one.
"""

from s3transfer.constants import KB, MB
from s3transfer.exceptions import S3UploadFailedError
from s3transfer.manager import TransferConfig as _ManagerConfig
from s3transfer.manager import TransferManager
from s3transfer.subscribers import BaseSubscriber

# The members this configuration takes under its OWN names, and the manager's member each of
# them states: the published configuration reads back under both.
ALIAS = {
    "max_concurrency": "max_request_concurrency",
    "max_io_queue": "max_io_queue_size",
}


class TransferConfig(_ManagerConfig):
    """What a transfer this SDK's Amazon S3 client makes is run under.

    The defaults are the published ones: an object larger than `multipart_threshold` is sent
    as the parts of a multipart upload, each `multipart_chunksize` long, and anything inside
    the threshold in one call. The CONCURRENCY members are taken and validated and bound
    nothing, because this runtime runs one function on one thread ([`s3transfer.manager`]).
    """

    def __init__(
        self,
        multipart_threshold=8 * MB,
        max_concurrency=10,
        multipart_chunksize=8 * MB,
        num_download_attempts=5,
        max_io_queue=100,
        io_chunksize=256 * KB,
        use_threads=True,
        max_bandwidth=None,
    ):
        _ManagerConfig.__init__(
            self,
            multipart_threshold=multipart_threshold,
            multipart_chunksize=multipart_chunksize,
            max_request_concurrency=max_concurrency,
            num_download_attempts=num_download_attempts,
            max_io_queue_size=max_io_queue,
            io_chunksize=io_chunksize,
            max_bandwidth=max_bandwidth,
        )
        # The members above under the names this configuration's own caller gave them, so a
        # handler reads back what it asked for.
        self.max_concurrency = max_concurrency
        self.max_io_queue = max_io_queue
        # "If True, threads will be used when performing S3 transfers": there are none to use
        # here, so it is taken and reported and decides nothing.
        self.use_threads = use_threads


class ProgressCallbackInvoker(BaseSubscriber):
    """A caller's own progress callback, as the subscriber a transfer announces to."""

    def __init__(self, callback):
        self._callback = callback

    def on_progress(self, bytes_transferred, **kwargs):
        self._callback(bytes_transferred)


def create_transfer_manager(client, config=None, osutil=None):
    """The manager one client's managed transfers are run by."""
    return TransferManager(client, config, osutil)
"##;

/// `boto3.s3.resource`: the service resource, a bucket, an object and the collections.
const S3_RESOURCE_SOURCE: &str = r##"
"""The Amazon S3 resources themselves: the service resource, a bucket and its objects."""

from botocore import xform_name

from boto3.resources.base import ResourceMeta

# The service these resources belong to, which is what each of them reports it was built for.
_SERVICE = "s3"

# The most keys one `DeleteObjects` may carry, which is what a collection's own delete sends
# its keys in: "the request can contain a list of up to 1000 keys" (`DeleteObjects`).
_KEYS_PER_DELETE = 1000


def _attributes(answered):
    """What a service's answer says about one resource, under the SDK's own names.

    The answer's own members, keyed as the SDK reports them (`ETag` -> `e_tag`), with the
    metadata of the HTTP answer itself left out: that is about the call, not the resource.
    """
    return {
        xform_name(name): value
        for name, value in answered.items()
        if name != "ResponseMetadata"
    }


class Object:
    """One object addressed directly: `s3.Object(bucket, key)`.

    Its methods are the object's own operations with the bucket and the key carried for you,
    and its attributes are what a read of the object reports, fetched when one is first asked
    for.
    """

    def __init__(self, bucket_name, key, client, data=None):
        self.meta = ResourceMeta(
            _SERVICE, client, identifiers=["bucket_name", "key"], data=data
        )
        self.bucket_name = bucket_name
        self.key = key

    def __repr__(self):
        return "s3.Object(bucket_name=%r, key=%r)" % (self.bucket_name, self.key)

    def __eq__(self, other):
        return (
            isinstance(other, Object)
            and self.bucket_name == other.bucket_name
            and self.key == other.key
        )

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash(("s3.Object", self.bucket_name, self.key))

    def __getattr__(self, name):
        if name.startswith("_") or name in ("meta", "bucket_name", "key"):
            raise AttributeError(name)
        if self.meta.data is None:
            self.load()
        if name in self.meta.data:
            return self.meta.data[name]
        raise AttributeError("'s3.Object' object has no attribute '%s'" % name)

    def load(self):
        """Read what the service says about this object, as `HeadObject` reports it."""
        self.meta.data = _attributes(
            self.meta.client.head_object(Bucket=self.bucket_name, Key=self.key)
        )

    reload = load

    def Bucket(self):
        """The bucket this object is held in, as a resource."""
        return Bucket(self.bucket_name, self.meta.client)

    def get(self, **kwargs):
        return self.meta.client.get_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )

    def put(self, **kwargs):
        answered = self.meta.client.put_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )
        # What the write changed about the object is no longer what was read before it.
        self.meta.data = None
        return answered

    def delete(self, **kwargs):
        answered = self.meta.client.delete_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )
        self.meta.data = None
        return answered

    def copy_from(self, **kwargs):
        answered = self.meta.client.copy_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )
        self.meta.data = None
        return answered

    def upload_fileobj(self, Fileobj, ExtraArgs=None, Callback=None, Config=None):
        """This object written from a file-like value, through the client's own transfer."""
        answered = self.meta.client.upload_fileobj(
            Fileobj,
            self.bucket_name,
            self.key,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )
        self.meta.data = None
        return answered

    def download_fileobj(self, Fileobj, ExtraArgs=None, Callback=None, Config=None):
        """This object read into a file-like value, through the same transfer."""
        return self.meta.client.download_fileobj(
            self.bucket_name,
            self.key,
            Fileobj,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )

    def upload_file(self, Filename, ExtraArgs=None, Callback=None, Config=None):
        answered = self.meta.client.upload_file(
            Filename,
            self.bucket_name,
            self.key,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )
        self.meta.data = None
        return answered

    def download_file(self, Filename, ExtraArgs=None, Callback=None, Config=None):
        return self.meta.client.download_file(
            self.bucket_name,
            self.key,
            Filename,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )


class ObjectSummary:
    """One object as a LISTING reports it, which is what a bucket's objects yield.

    A summary carries what the listing said about the object — its key, its tag, its size and
    the instant it was last modified — so iterating a bucket's objects makes one call per page
    and no call per object. An attribute a listing does not carry is read with `HeadObject`,
    the way the SDK loads one.
    """

    def __init__(self, bucket_name, key, client, data=None):
        self.meta = ResourceMeta(
            _SERVICE, client, identifiers=["bucket_name", "key"], data=data
        )
        self.bucket_name = bucket_name
        self.key = key

    def __repr__(self):
        return "s3.ObjectSummary(bucket_name=%r, key=%r)" % (self.bucket_name, self.key)

    def __eq__(self, other):
        return (
            isinstance(other, ObjectSummary)
            and self.bucket_name == other.bucket_name
            and self.key == other.key
        )

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash(("s3.ObjectSummary", self.bucket_name, self.key))

    def __getattr__(self, name):
        if name.startswith("_") or name in ("meta", "bucket_name", "key"):
            raise AttributeError(name)
        if self.meta.data is None or name not in self.meta.data:
            self.load()
        if name in self.meta.data:
            return self.meta.data[name]
        raise AttributeError("'s3.ObjectSummary' object has no attribute '%s'" % name)

    def load(self):
        """Read what the service says about this object, as `HeadObject` reports it.

        A summary's own attributes are the LISTING's (`Object$Size`), and a read of the object
        reports the same fact under the name that read uses (`ContentLength`), so the one that
        differs is reported under the summary's name as well.
        """
        held = dict(self.meta.data or {})
        read = _attributes(
            self.meta.client.head_object(Bucket=self.bucket_name, Key=self.key)
        )
        if "content_length" in read:
            read["size"] = read["content_length"]
        held.update(read)
        self.meta.data = held

    def Object(self):
        """The same object as the resource its own operations hang off."""
        return Object(self.bucket_name, self.key, self.meta.client)

    def Bucket(self):
        return Bucket(self.bucket_name, self.meta.client)

    def get(self, **kwargs):
        return self.meta.client.get_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )

    def put(self, **kwargs):
        return self.meta.client.put_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )

    def delete(self, **kwargs):
        return self.meta.client.delete_object(
            Bucket=self.bucket_name, Key=self.key, **kwargs
        )


class ObjectCollection:
    """`bucket.objects`: the objects one bucket holds, as the listing pages them.

    Iterating it makes one `ListObjectsV2` per page and continues from the member that
    listing's own answer carries, so a bucket holding more objects than one page is walked
    whole. `filter(...)` names the listing's own input members, `page_size` asks for pages of
    a size and `limit` bounds how many objects the iteration yields.
    """

    def __init__(self, bucket_name, client, parameters=None, limit=None, page_size=None):
        self._bucket_name = bucket_name
        self._client = client
        self._parameters = dict(parameters or {})
        self._limit = limit
        self._page_size = page_size

    def _with(self, parameters=None, limit=None, page_size=None):
        return ObjectCollection(
            self._bucket_name,
            self._client,
            parameters=self._parameters if parameters is None else parameters,
            limit=self._limit if limit is None else limit,
            page_size=self._page_size if page_size is None else page_size,
        )

    def all(self):
        """Every object the bucket holds, under no filter at all."""
        return ObjectCollection(
            self._bucket_name, self._client, limit=self._limit, page_size=self._page_size
        )

    def filter(self, **kwargs):
        """The objects the listing's own members select, as the SDK passes them through."""
        parameters = dict(self._parameters)
        parameters.update(kwargs)
        return self._with(parameters=parameters)

    def limit(self, count):
        """At most `count` objects, however many pages they take."""
        return self._with(limit=int(count))

    def page_size(self, count):
        """Pages of `count` objects, which is what the listing is asked for."""
        return self._with(page_size=int(count))

    def pages(self):
        """The pages themselves, each the summaries one listing answered."""
        yielded = 0
        token = None
        while True:
            parameters = dict(self._parameters)
            parameters["Bucket"] = self._bucket_name
            if self._page_size is not None:
                parameters["MaxKeys"] = self._page_size
            if token is not None:
                parameters["ContinuationToken"] = token
            answered = self._client.list_objects_v2(**parameters)
            page = []
            for held in answered.get("Contents") or []:
                if self._limit is not None and yielded >= self._limit:
                    break
                yielded += 1
                page.append(
                    ObjectSummary(
                        self._bucket_name,
                        held.get("Key"),
                        self._client,
                        data=_attributes(held),
                    )
                )
            yield page
            if self._limit is not None and yielded >= self._limit:
                return
            token = answered.get("NextContinuationToken")
            if not token:
                return

    def __iter__(self):
        for page in self.pages():
            for summary in page:
                yield summary

    def delete(self, **kwargs):
        """Every object this collection names, removed in the batches one call carries."""
        answered = []
        batch = []
        for summary in self:
            batch.append({"Key": summary.key})
            if len(batch) >= _KEYS_PER_DELETE:
                answered.append(self._delete_batch(batch, kwargs))
                batch = []
        if batch:
            answered.append(self._delete_batch(batch, kwargs))
        return answered

    def _delete_batch(self, batch, kwargs):
        return self._client.delete_objects(
            Bucket=self._bucket_name, Delete={"Objects": batch}, **kwargs
        )


class Bucket:
    """One bucket of the resource interface: `s3.Bucket(name)`.

    Its methods are the bucket's own operations with the name carried for you, its `objects`
    are the collection above, and its attributes are what the account's own listing reports
    for it.
    """

    def __init__(self, name, client, data=None):
        self.meta = ResourceMeta(_SERVICE, client, identifiers=["name"], data=data)
        self.name = name

    def __repr__(self):
        return "s3.Bucket(name=%r)" % (self.name,)

    def __eq__(self, other):
        return isinstance(other, Bucket) and self.name == other.name

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash(("s3.Bucket", self.name))

    def __getattr__(self, name):
        if name.startswith("_") or name in ("meta", "name"):
            raise AttributeError(name)
        if self.meta.data is None:
            self.load()
        if name in self.meta.data:
            return self.meta.data[name]
        raise AttributeError("'s3.Bucket' object has no attribute '%s'" % name)

    def load(self):
        """What the account's own listing says about this bucket, which is where the SDK
        reads a bucket's attributes from: `ListBuckets` is the call that reports when each
        bucket the account holds came into being."""
        listed = self.meta.client.list_buckets()
        for held in listed.get("Buckets") or []:
            if held.get("Name") == self.name:
                self.meta.data = _attributes(held)
                return
        self.meta.data = {}

    reload = load

    @property
    def objects(self):
        return ObjectCollection(self.name, self.meta.client)

    def Object(self, key):
        """One object of this bucket, as a resource."""
        return Object(self.name, key, self.meta.client)

    def create(self, **kwargs):
        return self.meta.client.create_bucket(Bucket=self.name, **kwargs)

    def delete(self, **kwargs):
        return self.meta.client.delete_bucket(Bucket=self.name, **kwargs)

    def put_object(self, **kwargs):
        """One object written into this bucket, answered as the resource it is."""
        self.meta.client.put_object(Bucket=self.name, **kwargs)
        return self.Object(kwargs.get("Key"))

    def delete_objects(self, **kwargs):
        return self.meta.client.delete_objects(Bucket=self.name, **kwargs)

    def upload_fileobj(self, Fileobj, Key, ExtraArgs=None, Callback=None, Config=None):
        return self.meta.client.upload_fileobj(
            Fileobj,
            self.name,
            Key,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )

    def download_fileobj(self, Key, Fileobj, ExtraArgs=None, Callback=None, Config=None):
        return self.meta.client.download_fileobj(
            self.name,
            Key,
            Fileobj,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )

    def upload_file(self, Filename, Key, ExtraArgs=None, Callback=None, Config=None):
        return self.meta.client.upload_file(
            Filename,
            self.name,
            Key,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )

    def download_file(self, Key, Filename, ExtraArgs=None, Callback=None, Config=None):
        return self.meta.client.download_file(
            self.name,
            Key,
            Filename,
            ExtraArgs=ExtraArgs,
            Callback=Callback,
            Config=Config,
        )


class BucketCollection:
    """`s3.buckets`: the buckets the calling identity may list, as `Bucket` resources."""

    def __init__(self, client, parameters=None, limit=None, page_size=None):
        self._client = client
        self._parameters = dict(parameters or {})
        self._limit = limit
        self._page_size = page_size

    def all(self):
        return BucketCollection(
            self._client, limit=self._limit, page_size=self._page_size
        )

    def filter(self, **kwargs):
        parameters = dict(self._parameters)
        parameters.update(kwargs)
        return BucketCollection(
            self._client,
            parameters=parameters,
            limit=self._limit,
            page_size=self._page_size,
        )

    def limit(self, count):
        return BucketCollection(
            self._client,
            parameters=self._parameters,
            limit=int(count),
            page_size=self._page_size,
        )

    def page_size(self, count):
        return BucketCollection(
            self._client,
            parameters=self._parameters,
            limit=self._limit,
            page_size=int(count),
        )

    def __iter__(self):
        yielded = 0
        token = None
        while True:
            parameters = dict(self._parameters)
            if self._page_size is not None:
                parameters["MaxBuckets"] = self._page_size
            if token is not None:
                parameters["ContinuationToken"] = token
            answered = self._client.list_buckets(**parameters)
            for held in answered.get("Buckets") or []:
                if self._limit is not None and yielded >= self._limit:
                    return
                yielded += 1
                yield Bucket(held.get("Name"), self._client, data=_attributes(held))
            token = answered.get("ContinuationToken")
            if not token:
                return


class ServiceResource:
    """`boto3.resource("s3")`: the service resource the buckets hang off."""

    def __init__(self, client):
        self.meta = ResourceMeta(_SERVICE, client)

    def __repr__(self):
        return "s3.ServiceResource()"

    @property
    def buckets(self):
        return BucketCollection(self.meta.client)

    def Bucket(self, name):
        """The bucket of that name, as a resource."""
        return Bucket(name, self.meta.client)

    def Object(self, bucket_name, key):
        """One object of one bucket, as a resource."""
        return Object(bucket_name, key, self.meta.client)

    def ObjectSummary(self, bucket_name, key):
        """The same object as a listing reports one."""
        return ObjectSummary(bucket_name, key, self.meta.client)

    def create_bucket(self, **kwargs):
        """Create a bucket, answering it as a resource."""
        self.meta.client.create_bucket(**kwargs)
        return self.Bucket(kwargs.get("Bucket"))


def service_resource(client):
    """The Amazon S3 service resource over `client`."""
    return ServiceResource(client)
"##;
