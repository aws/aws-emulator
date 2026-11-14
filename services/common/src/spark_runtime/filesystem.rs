// SPDX-License-Identifier: CC0-1.0

//! `pyspark.filesystem`: the Hadoop-compatible filesystem a path resolves in.
//!
//! This is where a Spark program's reads and writes LAND. A path in Amazon S3 is an object
//! of this emulator's own object store, reached with the AWS SDK the runtime supplies the
//! guest; every other filesystem is one this emulator holds nothing for, and a path in it
//! raises the way Hadoop raises for a scheme it has no filesystem for.

/// The source of `pyspark/filesystem.py`.
pub(super) const FILESYSTEM_SOURCE: &str = r#"
"""The filesystems a Spark application's paths resolve in."""

# The schemes that name an object of Amazon S3: the EMR filesystem's own and the Hadoop
# S3A/S3N ones, which address the same store.
S3_SCHEMES = ("s3", "s3a", "s3n")

# How many bytes one write of this filesystem holds before it is flushed to the store. The
# bounds that matter are the interpreter's own, in host code; this one keeps a single stream
# from holding more than one object's worth of bytes.
MAX_OBJECT_BYTES = 64 * 1024 * 1024


class UnsupportedFileSystem(IOError):
    """A path whose filesystem this emulator does not have, as Hadoop words it."""


def _text(value):
    """A path argument as its text, whether it arrived as a string or as a Path."""
    if hasattr(value, "toString"):
        return value.toString()
    return str(value)


def split_uri(path):
    """The (scheme, authority, path) a URI names; the scheme is None for a bare path."""
    text = _text(path)
    if "://" not in text:
        return (None, None, text)
    scheme, rest = text.split("://", 1)
    if "/" not in rest:
        return (scheme, rest, "")
    authority, within = rest.split("/", 1)
    return (scheme, authority, within)


def object_of(path):
    """The (bucket, key) an Amazon S3 URI names, or None for any other path."""
    scheme, authority, within = split_uri(path)
    if scheme is None or scheme.lower() not in S3_SCHEMES:
        return None
    if not authority or not within:
        return None
    return (authority, within)


def _client():
    """The object store client, built through the SDK this runtime supplies the guest."""
    import boto3

    return boto3.client("s3")


class FileStatus(object):
    """One entry of a listing: the path, its length and whether it is a directory."""

    def __init__(self, path, length=0, directory=False):
        self._path = path
        self._length = length
        self._directory = directory

    def getPath(self):
        return self._path

    def getLen(self):
        return self._length

    def isDirectory(self):
        return self._directory

    def isDir(self):
        return self._directory

    def isFile(self):
        return not self._directory


class FSDataOutputStream(object):
    """An open write: the bytes are held until the stream is closed, then stored."""

    def __init__(self, bucket, key):
        self._bucket = bucket
        self._key = key
        self._buffer = bytearray()
        self._closed = False

    def write(self, data, offset=None, length=None):
        if self._closed:
            raise IOError("the stream for s3://%s/%s is closed" % (self._bucket, self._key))
        if isinstance(data, int):
            chunk = bytes(bytearray([data & 0xFF]))
        elif isinstance(data, str):
            chunk = data.encode("utf-8")
        else:
            chunk = bytes(bytearray(data))
        if offset is not None:
            end = len(chunk) if length is None else offset + length
            chunk = chunk[offset:end]
        if len(self._buffer) + len(chunk) > MAX_OBJECT_BYTES:
            raise IOError(
                "an object of at most %d bytes may be written" % (MAX_OBJECT_BYTES,)
            )
        self._buffer.extend(chunk)

    def writeBytes(self, text):
        self.write(text)

    def writeUTF(self, text):
        self.write(text)

    # Hadoop's flush and sync make written bytes visible to a reader. An object store has
    # one visible state, the stored object, so these hold the bytes until the close that
    # stores them.
    def flush(self):
        return None

    def hflush(self):
        return None

    def hsync(self):
        return None

    def sync(self):
        return None

    def close(self):
        if self._closed:
            return None
        self._closed = True
        _client().put_object(
            Bucket=self._bucket, Key=self._key, Body=bytes(self._buffer)
        )
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close()
        return False


class FSDataInputStream(object):
    """An open read of a stored object."""

    def __init__(self, body):
        self._body = body
        self._at = 0

    def read(self, size=None):
        if size is None or size < 0:
            chunk = self._body[self._at:]
            self._at = len(self._body)
            return chunk
        chunk = self._body[self._at:self._at + size]
        self._at += len(chunk)
        return chunk

    def readFully(self, *rest):
        return self.read()

    def available(self):
        return len(self._body) - self._at

    def close(self):
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close()
        return False


class FileSystem(object):
    """The filesystem of one Amazon S3 bucket, as `Path.getFileSystem` hands one out."""

    def __init__(self, scheme, bucket):
        self._scheme = scheme
        self._bucket = bucket

    @staticmethod
    def get(uri, conf=None):
        """The filesystem a URI resolves in. `FileSystem.get(uri, conf)` in Hadoop."""
        named = object_of(uri)
        if named is None:
            scheme, authority, within = split_uri(uri)
            if scheme is not None and scheme.lower() in S3_SCHEMES and authority:
                return FileSystem(scheme.lower(), authority)
            raise UnsupportedFileSystem("No FileSystem for scheme: %s" % (scheme,))
        bucket, _ = named
        scheme, _, _ = split_uri(uri)
        return FileSystem(scheme.lower(), bucket)

    def getUri(self):
        return "%s://%s" % (self._scheme, self._bucket)

    def _key(self, path):
        named = object_of(path)
        if named is None:
            raise UnsupportedFileSystem("No FileSystem for path: %s" % (_text(path),))
        bucket, key = named
        if bucket != self._bucket:
            raise UnsupportedFileSystem(
                "Wrong FS: %s, expected %s" % (_text(path), self.getUri())
            )
        return key

    def create(self, path, overwrite=True, *rest, **named):
        key = self._key(path)
        if not overwrite and self.exists(path):
            raise IOError("%s already exists" % (_text(path),))
        return FSDataOutputStream(self._bucket, key)

    def append(self, path, *rest, **named):
        raise UnsupportedFileSystem(
            "Append is not supported by %s" % (self.getUri(),)
        )

    def open(self, path, bufferSize=None):
        return FSDataInputStream(read_bytes(path))

    def exists(self, path):
        key = self._key(path)
        client = _client()
        try:
            client.head_object(Bucket=self._bucket, Key=key)
            return True
        except Exception:
            listed = client.list_objects_v2(
                Bucket=self._bucket, Prefix=key.rstrip("/") + "/", MaxKeys=1
            )
            return int(listed.get("KeyCount") or 0) > 0

    def delete(self, path, recursive=False):
        key = self._key(path)
        client = _client()
        if not recursive:
            client.delete_object(Bucket=self._bucket, Key=key)
            return True
        for status in self.listStatus(path):
            client.delete_object(Bucket=self._bucket, Key=self._key(status.getPath()))
        return True

    def mkdirs(self, path, *rest):
        # An object store holds objects, not directories: the prefix a path names exists
        # once an object is written under it, so there is nothing to create here.
        self._key(path)
        return True

    def listStatus(self, path):
        prefix = self._key(path).rstrip("/")
        listed = _client().list_objects_v2(Bucket=self._bucket, Prefix=prefix)
        statuses = []
        for entry in listed.get("Contents") or []:
            statuses.append(
                FileStatus(
                    "%s://%s/%s" % (self._scheme, self._bucket, entry["Key"]),
                    int(entry.get("Size") or 0),
                )
            )
        return statuses

    def getFileStatus(self, path):
        key = self._key(path)
        read = _client().head_object(Bucket=self._bucket, Key=key)
        return FileStatus(
            "%s://%s/%s" % (self._scheme, self._bucket, key),
            int(read.get("ContentLength") or 0),
        )

    def close(self):
        return None


def read_bytes(path):
    """The bytes of the object `path` names."""
    named = object_of(path)
    if named is None:
        scheme, _, _ = split_uri(path)
        raise UnsupportedFileSystem("No FileSystem for scheme: %s" % (scheme,))
    bucket, key = named
    read = _client().get_object(Bucket=bucket, Key=key)
    body = read["Body"].read()
    if isinstance(body, str):
        return body.encode("utf-8")
    return bytes(bytearray(body))


def read_text(path):
    """The object `path` names, decoded as the text a line-based read sees."""
    return read_bytes(path).decode("utf-8")


def write_text(path, text):
    """Store `text` as the object `path` names, through one open stream."""
    stream = FileSystem.get(path).create(path, True)
    stream.write(text)
    stream.close()
    return None
"#;
