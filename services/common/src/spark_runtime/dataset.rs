// SPDX-License-Identifier: CC0-1.0

//! `pyspark.rdd`: the dataset itself — where every transformation and action is COMPUTED.

/// The source of `pyspark/rdd.py`.
pub(super) const RDD_SOURCE: &str = r#"
"""The resilient distributed dataset of an application, computed in this process."""

from pyspark import filesystem

# The name Spark writes a successful output's marker under, beside the part files of the
# output it wrote.
SUCCESS_MARKER = "_SUCCESS"

# How many partitions ONE dataset is held in, at most.
#
# A partition count is the APPLICATION's: every transformation that takes one — a
# repartition, a coalesce, a grouping, a sort, a join — passes it straight to `slices`. The
# work here runs in this one process, so a count is bounded before anything is allocated:
# past the elements the dataset holds a partition holds nothing, and past this ceiling an
# application would be asking this process for an unbounded number of empty lists. Neither
# bound changes what the dataset COMPUTES, because an action is over the elements across its
# partitions.
MAX_PARTITIONS = 1024


def slices(items, count):
    """`items` split into `count` partitions, the way a collection is sliced into them.

    The count is bounded BEFORE any partition is built ([`MAX_PARTITIONS`]), so a very
    large one costs this process nothing.
    """
    values = list(items)
    count = max(int(count), 1)
    count = min(count, max(len(values), 1), MAX_PARTITIONS)
    size = len(values) // count
    extra = len(values) % count
    partitions = []
    start = 0
    for index in range(count):
        end = start + size + (1 if index < extra else 0)
        partitions.append(values[start:end])
        start = end
    return partitions


def _folded(operator, values):
    """`operator` folded over `values`, left to right, as reduce does."""
    total = values[0]
    for value in values[1:]:
        total = operator(total, value)
    return total


class RDD(object):
    """One dataset: its partitions, and the computations over them."""

    def __init__(self, partitions):
        self._partitions = [list(partition) for partition in partitions]

    # --- the partitions, and the elements across them -----------------------------------

    def _elements(self):
        return [value for partition in self._partitions for value in partition]

    def getNumPartitions(self):
        return len(self._partitions)

    def glom(self):
        return RDD([[list(partition)] for partition in self._partitions])

    def repartition(self, numPartitions):
        return RDD(slices(self._elements(), numPartitions))

    def coalesce(self, numPartitions, shuffle=False):
        return self.repartition(numPartitions)

    # --- transformations: the application's own functions, called on its own data --------

    def map(self, f, preservesPartitioning=False):
        return RDD([[f(value) for value in part] for part in self._partitions])

    def flatMap(self, f, preservesPartitioning=False):
        return RDD(
            [[out for value in part for out in f(value)] for part in self._partitions]
        )

    def filter(self, f):
        return RDD([[value for value in part if f(value)] for part in self._partitions])

    def mapPartitions(self, f, preservesPartitioning=False):
        return RDD([list(f(iter(part))) for part in self._partitions])

    def mapPartitionsWithIndex(self, f, preservesPartitioning=False):
        return RDD(
            [list(f(index, iter(part))) for index, part in enumerate(self._partitions)]
        )

    def keyBy(self, f):
        return self.map(lambda value: (f(value), value))

    def mapValues(self, f):
        return self.map(lambda pair: (pair[0], f(pair[1])))

    def flatMapValues(self, f):
        return self.flatMap(lambda pair: [(pair[0], out) for out in f(pair[1])])

    def keys(self):
        return self.map(lambda pair: pair[0])

    def values(self):
        return self.map(lambda pair: pair[1])

    def distinct(self, numPartitions=None):
        kept = []
        for value in self._elements():
            if value not in kept:
                kept.append(value)
        return RDD(slices(kept, numPartitions or self.getNumPartitions()))

    def union(self, other):
        return RDD(self._partitions + other._partitions)

    def intersection(self, other):
        held = other._elements()
        return RDD(
            slices(
                [value for value in self.distinct()._elements() if value in held],
                self.getNumPartitions(),
            )
        )

    def subtract(self, other, numPartitions=None):
        held = other._elements()
        return RDD(
            slices(
                [value for value in self._elements() if value not in held],
                numPartitions or self.getNumPartitions(),
            )
        )

    def zip(self, other):
        return RDD(
            slices(
                list(zip(self._elements(), other._elements())), self.getNumPartitions()
            )
        )

    def zipWithIndex(self):
        return RDD(
            slices(
                [(value, index) for index, value in enumerate(self._elements())],
                self.getNumPartitions(),
            )
        )

    def groupByKey(self, numPartitions=None):
        order = []
        grouped = {}
        for key, value in self._elements():
            if key not in grouped:
                grouped[key] = []
                order.append(key)
            grouped[key].append(value)
        return RDD(
            slices(
                [(key, grouped[key]) for key in order],
                numPartitions or self.getNumPartitions(),
            )
        )

    def reduceByKey(self, func, numPartitions=None):
        return self.groupByKey(numPartitions).mapValues(
            lambda values: _folded(func, values)
        )

    def _partitioned_by_key(self):
        """The values of each key, per partition: [{key: [value, ...]}, ...]."""
        per_partition = []
        for partition in self._partitions:
            grouped = {}
            for key, value in partition:
                grouped.setdefault(key, []).append(value)
            per_partition.append(grouped)
        return per_partition

    def _keys_in_order(self, per_partition):
        """Every key the dataset holds, in the order the partitions first carry it."""
        order = []
        for grouped in per_partition:
            for key in grouped:
                if key not in order:
                    order.append(key)
        return order

    def foldByKey(self, zeroValue, func, numPartitions=None):
        def folded(values):
            total = zeroValue
            for value in values:
                total = func(total, value)
            return total

        return self.groupByKey(numPartitions).mapValues(folded)

    def aggregateByKey(self, zeroValue, seqFunc, combFunc, numPartitions=None):
        # The values of a key WITHIN one partition are folded with seqFunc from the zero
        # value, and those per-partition results are combined with combFunc — so an
        # application whose two functions differ gets what the real action computes.
        per_partition = self._partitioned_by_key()
        aggregated = []
        for key in self._keys_in_order(per_partition):
            combined = None
            for grouped in per_partition:
                if key not in grouped:
                    continue
                folded = zeroValue
                for value in grouped[key]:
                    folded = seqFunc(folded, value)
                combined = folded if combined is None else combFunc(combined, folded)
            aggregated.append((key, combined))
        return RDD(slices(aggregated, numPartitions or self.getNumPartitions()))

    def combineByKey(self, createCombiner, mergeValue, mergeCombiners, numPartitions=None):
        # The same two stages: a combiner per key per partition, then the combiners merged
        # across partitions.
        per_partition = self._partitioned_by_key()
        combined_pairs = []
        for key in self._keys_in_order(per_partition):
            combined = None
            for grouped in per_partition:
                if key not in grouped:
                    continue
                values = grouped[key]
                partial = createCombiner(values[0])
                for value in values[1:]:
                    partial = mergeValue(partial, value)
                combined = partial if combined is None else mergeCombiners(combined, partial)
            combined_pairs.append((key, combined))
        return RDD(slices(combined_pairs, numPartitions or self.getNumPartitions()))

    def sortByKey(self, ascending=True, numPartitions=None, keyfunc=None):
        chosen = keyfunc or (lambda key: key)
        ordered = sorted(
            self._elements(), key=lambda pair: chosen(pair[0]), reverse=not ascending
        )
        return RDD(slices(ordered, numPartitions or self.getNumPartitions()))

    def sortBy(self, keyfunc, ascending=True, numPartitions=None):
        ordered = sorted(self._elements(), key=keyfunc, reverse=not ascending)
        return RDD(slices(ordered, numPartitions or self.getNumPartitions()))

    def join(self, other, numPartitions=None):
        right = other._elements()
        joined = []
        for key, value in self._elements():
            for other_key, other_value in right:
                if other_key == key:
                    joined.append((key, (value, other_value)))
        return RDD(slices(joined, numPartitions or self.getNumPartitions()))

    # Caching holds a dataset in memory for the next computation. Every dataset here is
    # already the computed partitions, so these answer the dataset itself.
    def cache(self):
        return self

    def persist(self, storageLevel=None):
        return self

    def unpersist(self, blocking=False):
        return self

    def setName(self, name):
        self._name = str(name)
        return self

    # --- actions ------------------------------------------------------------------------

    def collect(self):
        return list(self._elements())

    def toLocalIterator(self):
        return iter(self._elements())

    def count(self):
        return len(self._elements())

    def isEmpty(self):
        return not self._elements()

    def first(self):
        values = self._elements()
        if not values:
            raise ValueError("RDD is empty")
        return values[0]

    def take(self, num):
        return self._elements()[: max(int(num), 0)]

    def takeOrdered(self, num, key=None):
        return sorted(self._elements(), key=key)[: max(int(num), 0)]

    def top(self, num, key=None):
        return sorted(self._elements(), key=key, reverse=True)[: max(int(num), 0)]

    def reduce(self, f):
        values = self._elements()
        if not values:
            raise ValueError("Can not reduce() empty RDD")
        return _folded(f, values)

    def fold(self, zeroValue, op):
        # "Aggregate the elements of each partition, and then the results for all the
        # partitions", from the zero value in both halves.
        total = zeroValue
        for partition in self._partitions:
            folded = zeroValue
            for value in partition:
                folded = op(folded, value)
            total = op(total, folded)
        return total

    def aggregate(self, zeroValue, seqOp, combOp):
        # Each partition is folded with seqOp from the zero value, and the partition
        # results are then combined with combOp from it, which is what the action does.
        total = zeroValue
        for partition in self._partitions:
            folded = zeroValue
            for value in partition:
                folded = seqOp(folded, value)
            total = combOp(total, folded)
        return total

    def sum(self):
        return sum(self._elements())

    def max(self, key=None):
        return max(self._elements(), key=key) if key else max(self._elements())

    def min(self, key=None):
        return min(self._elements(), key=key) if key else min(self._elements())

    def mean(self):
        values = self._elements()
        if not values:
            raise ValueError("RDD is empty")
        return float(sum(values)) / len(values)

    def stats(self):
        values = self._elements()
        return {"count": len(values), "sum": sum(values)}

    def countByKey(self):
        counts = {}
        for key, _ in self._elements():
            counts[key] = counts.get(key, 0) + 1
        return counts

    def countByValue(self):
        counts = {}
        for value in self._elements():
            counts[value] = counts.get(value, 0) + 1
        return counts

    def collectAsMap(self):
        return dict(self._elements())

    def lookup(self, key):
        return [value for held, value in self._elements() if held == key]

    def foreach(self, f):
        for value in self._elements():
            f(value)
        return None

    def foreachPartition(self, f):
        for partition in self._partitions:
            f(iter(partition))
        return None

    def saveAsTextFile(self, path, compressionCodecClass=None):
        """Write one part file per partition under `path`, beside the success marker.

        That is the output layout Spark writes: the path names a directory and each
        partition is a file in it, so a reader of the output reads the parts. Every write
        goes through the filesystem the path resolves in, so the objects are real.
        """
        prefix = filesystem._text(path).rstrip("/")
        for index, partition in enumerate(self._partitions):
            lines = "".join("%s\n" % (value,) for value in partition)
            filesystem.write_text("%s/part-%05d" % (prefix, index), lines)
        filesystem.write_text("%s/%s" % (prefix, SUCCESS_MARKER), "")
        return None
"#;
