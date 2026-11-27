// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE interface of the AWS SDK a Python handler imports, for DynamoDB: the
//! `boto3.resource("dynamodb")` service resource, its `Table` objects, and the two
//! directions between a plain Python value and the attribute value an AWS JSON protocol
//! carries.
//!
//! The managed `python3.x` runtimes ship the whole AWS SDK for Python, and DynamoDB's
//! resource interface is the one an application reaches for by default: a `Table` carries
//! the table name for you, an item is a `dict` of plain Python values rather than the
//! wire's `{"S": "..."}` attribute values, a number is a `decimal.Decimal`, a set is a
//! `set`, binary is `Binary`, and a condition is built with `Key`/`Attr` rather than
//! written as an expression string with placeholders.
//!
//! What these modules do, over the SAME one host call every client method is made through
//! ([`super::super::aws_api`]):
//!
//! * the item members of an operation's input are SERIALIZED from plain Python values and
//!   the item members of its answer are DESERIALIZED back to them, at the places that
//!   operation's own model carries attribute values — which is what makes a resource-side
//!   `put_item(Item={...})` the same request a client-side `put_item(Item={"id": {"S":
//!   ...}})` makes;
//! * a `Key`/`Attr` condition handed to a condition member is built into the expression
//!   string the service reads, with the attribute-name and attribute-value placeholders it
//!   uses merged into the request's own `ExpressionAttributeNames`/`ExpressionAttributeValues`;
//! * a `Table`'s own attributes are the ones `DescribeTable` reports for it, read when
//!   first asked for, so nothing about a table is held here that the service did not
//!   answer;
//! * a `Table`'s own waits (`wait_until_exists`, `wait_until_not_exists`) are the client's
//!   waiters of those states ([`super::waiter`]), carrying this table's name.
//!
//! Nothing here is the containment, and nothing here makes a call of its own: every request
//! is the client's, signed and dispatched by host code, and a service's answer is the
//! service's own.

/// The modules of the resource interface, in the order they are put on the import path.
#[must_use]
pub(super) fn modules() -> Vec<(&'static str, &'static str)> {
    vec![
        ("boto3/exceptions.py", BOTO3_EXCEPTIONS_SOURCE),
        ("boto3/dynamodb/__init__.py", DYNAMODB_SOURCE),
        ("boto3/dynamodb/types.py", DYNAMODB_TYPES_SOURCE),
        ("boto3/dynamodb/conditions.py", DYNAMODB_CONDITIONS_SOURCE),
        ("boto3/dynamodb/transform.py", DYNAMODB_TRANSFORM_SOURCE),
        ("boto3/dynamodb/table.py", DYNAMODB_TABLE_SOURCE),
    ]
}

/// `boto3.exceptions`: the errors boto3's own interfaces raise, beside botocore's.
const BOTO3_EXCEPTIONS_SOURCE: &str = r##"
"""The errors the SDK's own interfaces raise, beside the ones a call raises."""


class Boto3Error(Exception):
    """Base class for all boto3 errors."""


class ResourceLoadException(Boto3Error):
    """A resource's own attributes could not be loaded."""


class DynamoDBOperationNotSupportedError(Boto3Error):
    """An operator was applied to something that is not a condition."""

    def __init__(self, operation, value):
        Exception.__init__(
            self,
            "%s operation cannot be applied to value %s of type %s directly. Must use "
            "AttributeBase object methods (i.e. Attr().eq()). to generate ConditionBase "
            "instances first." % (operation, value, type(value)),
        )


class DynamoDBNeedsConditionError(Boto3Error):
    """A condition was expected and something else was given."""

    def __init__(self, value):
        Exception.__init__(
            self,
            "Expecting a ConditionBase object. Got %s of type %s. Use AttributeBase "
            "object methods (i.e. Attr().eq()). to generate ConditionBase instances."
            % (value, type(value)),
        )


class DynamoDBNeedsKeyConditionError(Boto3Error):
    """A key condition named an attribute that is not a key."""
"##;

/// `boto3.dynamodb`: the package the DynamoDB resource interface lives under.
const DYNAMODB_SOURCE: &str = r##"
"""DynamoDB's own half of the SDK: the types, conditions and resources of a table."""
"##;

/// `boto3.dynamodb.types`: a plain Python value and the attribute value the wire carries.
const DYNAMODB_TYPES_SOURCE: &str = r##"
"""The two directions between a plain Python value and a DynamoDB attribute value.

A number is carried as text on the wire and read back as a `decimal.Decimal` in the
context DynamoDB's own range and precision describe; a blob is carried as the base64 text
an AWS JSON protocol writes it as and read back as `Binary`; a set, a map and a list are
the Python ones.
"""

import base64
from decimal import Clamped, Context, Decimal, Inexact, Overflow, Rounded, Underflow

STRING = "S"
NUMBER = "N"
BINARY = "B"
STRING_SET = "SS"
NUMBER_SET = "NS"
BINARY_SET = "BS"
NULL = "NULL"
BOOLEAN = "BOOL"
MAP = "M"
LIST = "L"

# The range and precision a DynamoDB number has: 38 digits of precision, and an exponent
# between -128 and 126. A value outside it is not one the service stores, so the context
# traps rather than quietly rounding or clamping it.
DYNAMODB_CONTEXT = Context(
    Emin=-128,
    Emax=126,
    prec=38,
    traps=[Clamped, Overflow, Inexact, Rounded, Underflow],
)

BINARY_TYPES = (bytearray, bytes)


class Binary:
    """A DynamoDB binary attribute's value: the bytes themselves."""

    def __init__(self, value):
        if not isinstance(value, BINARY_TYPES):
            types = ", ".join([str(kind) for kind in BINARY_TYPES])
            raise TypeError("Value must be of the following types: %s" % types)
        self.value = bytes(value)

    def __eq__(self, other):
        if isinstance(other, Binary):
            return self.value == other.value
        return self.value == other

    def __ne__(self, other):
        return not self.__eq__(other)

    def __repr__(self):
        return "Binary(%r)" % (self.value,)

    def __bytes__(self):
        return self.value

    def __hash__(self):
        return hash(self.value)


def _as_bytes(value):
    """The bytes of a blob, however it arrived.

    A blob travels as base64 text under an AWS JSON protocol, so that is what a value read
    back off the wire is; a value a handler passed is already bytes.
    """
    if isinstance(value, Binary):
        return value.value
    if isinstance(value, str):
        return base64.b64decode(value)
    return bytes(value)


class TypeSerializer:
    """A plain Python value as the attribute value a request carries."""

    def serialize(self, value):
        dynamodb_type = self._get_dynamodb_type(value)
        serializer = getattr(self, "_serialize_%s" % dynamodb_type.lower())
        return {dynamodb_type: serializer(value)}

    def _get_dynamodb_type(self, value):
        if self._is_null(value):
            return NULL
        if self._is_boolean(value):
            return BOOLEAN
        if self._is_number(value):
            return NUMBER
        if self._is_string(value):
            return STRING
        if self._is_binary(value):
            return BINARY
        if self._is_type_set(value, self._is_number):
            return NUMBER_SET
        if self._is_type_set(value, self._is_string):
            return STRING_SET
        if self._is_type_set(value, self._is_binary):
            return BINARY_SET
        if self._is_map(value):
            return MAP
        if self._is_list(value):
            return LIST
        raise TypeError('Unsupported type "%s" for value "%s"' % (type(value), value))

    def _is_null(self, value):
        return value is None

    def _is_boolean(self, value):
        return isinstance(value, bool)

    def _is_number(self, value):
        if isinstance(value, (int, Decimal)):
            return True
        if isinstance(value, float):
            raise TypeError("Float types are not supported. Use Decimal types instead.")
        return False

    def _is_string(self, value):
        return isinstance(value, str)

    def _is_binary(self, value):
        return isinstance(value, (Binary, bytearray, bytes))

    def _is_set(self, value):
        return isinstance(value, (set, frozenset))

    def _is_type_set(self, value, type_validator):
        if self._is_set(value):
            if False not in map(type_validator, value):
                return True
        return False

    def _is_map(self, value):
        return isinstance(value, dict)

    def _is_list(self, value):
        return isinstance(value, (list, tuple))

    def _serialize_null(self, value):
        return True

    def _serialize_bool(self, value):
        return value

    def _serialize_n(self, value):
        number = str(DYNAMODB_CONTEXT.create_decimal(value))
        if number in ["Infinity", "NaN"]:
            raise TypeError("Infinity and NaN not supported")
        return number

    def _serialize_s(self, value):
        return value

    def _serialize_b(self, value):
        # The bytes themselves: the client encodes a blob to the base64 text an AWS JSON
        # protocol carries it as.
        return _as_bytes(value)

    def _serialize_ss(self, value):
        return [self._serialize_s(member) for member in value]

    def _serialize_ns(self, value):
        return [self._serialize_n(member) for member in value]

    def _serialize_bs(self, value):
        return [self._serialize_b(member) for member in value]

    def _serialize_m(self, value):
        return {key: self.serialize(member) for key, member in value.items()}

    def _serialize_l(self, value):
        return [self.serialize(member) for member in value]


class TypeDeserializer:
    """An attribute value an answer carries as the plain Python value it stands for."""

    def deserialize(self, value):
        if not isinstance(value, dict) or not value:
            raise TypeError(
                "Value must be a nonempty dictionary whose key is a valid dynamodb type."
            )
        dynamodb_type = list(value.keys())[0]
        try:
            deserializer = getattr(self, "_deserialize_%s" % dynamodb_type.lower())
        except AttributeError:
            raise TypeError("Dynamodb type %s is not supported" % dynamodb_type)
        return deserializer(value[dynamodb_type])

    def _deserialize_null(self, value):
        return None

    def _deserialize_bool(self, value):
        return value

    def _deserialize_n(self, value):
        return DYNAMODB_CONTEXT.create_decimal(value)

    def _deserialize_s(self, value):
        return value

    def _deserialize_b(self, value):
        return Binary(_as_bytes(value))

    def _deserialize_ns(self, value):
        return set(map(self._deserialize_n, value))

    def _deserialize_ss(self, value):
        return set(map(self._deserialize_s, value))

    def _deserialize_bs(self, value):
        return set(map(self._deserialize_b, value))

    def _deserialize_m(self, value):
        return {key: self.deserialize(member) for key, member in value.items()}

    def _deserialize_l(self, value):
        return [self.deserialize(member) for member in value]
"##;

/// `boto3.dynamodb.conditions`: `Key` and `Attr`, and the expression they are built into.
const DYNAMODB_CONDITIONS_SOURCE: &str = r##"
"""The conditions a DynamoDB expression is built from.

`Key(...)` names a key attribute of the table or index a query runs over, `Attr(...)` any
attribute of an item, and the comparisons on them compose with `&`, `|` and `~`. The
builder turns one of those into the expression string the service reads, together with the
attribute-name and attribute-value placeholders it uses.
"""

import re
from collections import namedtuple

from boto3.exceptions import (
    DynamoDBNeedsConditionError,
    DynamoDBNeedsKeyConditionError,
    DynamoDBOperationNotSupportedError,
)

# The parts of an attribute name that are names rather than list indexes: a document path
# is written `a.b[2].c`, and only `a`, `b` and `c` of it are attribute names.
ATTR_NAME_REGEX = re.compile(r"[^.\[\]]+(?![^\[]*\])")


class ConditionBase:
    """One condition, as its format, its operator and the values it is applied to."""

    expression_format = "{0} {operator} {1}"
    expression_operator = ""
    has_grouped_values = False

    def __init__(self, *values):
        self._values = values

    def get_expression(self):
        return {
            "format": self.expression_format,
            "operator": self.expression_operator,
            "values": self._values,
        }

    def __eq__(self, other):
        if isinstance(other, type(self)):
            return self._values == other._values
        return False

    def __ne__(self, other):
        return not self.__eq__(other)

    def __and__(self, other):
        if not isinstance(other, ConditionBase):
            raise DynamoDBOperationNotSupportedError("AND", other)
        return And(self, other)

    def __or__(self, other):
        if not isinstance(other, ConditionBase):
            raise DynamoDBOperationNotSupportedError("OR", other)
        return Or(self, other)

    def __invert__(self):
        return Not(self)


class AttributeBase:
    """One attribute of an item, by name, and the conditions it can be put under."""

    def __init__(self, name):
        self.name = name

    def __and__(self, value):
        raise DynamoDBOperationNotSupportedError("AND", self)

    def __or__(self, value):
        raise DynamoDBOperationNotSupportedError("OR", self)

    def __invert__(self):
        raise DynamoDBOperationNotSupportedError("NOT", self)

    def eq(self, value):
        """The attribute is equal to `value`."""
        return Equals(self, value)

    def lt(self, value):
        """The attribute is less than `value`."""
        return LessThan(self, value)

    def lte(self, value):
        """The attribute is less than or equal to `value`."""
        return LessThanEquals(self, value)

    def gt(self, value):
        """The attribute is greater than `value`."""
        return GreaterThan(self, value)

    def gte(self, value):
        """The attribute is greater than or equal to `value`."""
        return GreaterThanEquals(self, value)

    def begins_with(self, value):
        """The attribute begins with `value`."""
        return BeginsWith(self, value)

    def between(self, low_value, high_value):
        """The attribute is between the two values, inclusive."""
        return Between(self, low_value, high_value)

    def __eq__(self, other):
        return isinstance(other, type(self)) and self.name == other.name

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash((type(self).__name__, self.name))


class Key(AttributeBase):
    """A key attribute of the table or index being queried."""


class Attr(AttributeBase):
    """Any attribute of an item, with the conditions only a filter may use."""

    def ne(self, value):
        """The attribute is not equal to `value`."""
        return NotEquals(self, value)

    def is_in(self, value):
        """The attribute is one of the values in `value`."""
        return In(self, value)

    def exists(self):
        """The item has the attribute."""
        return AttributeExists(self)

    def not_exists(self):
        """The item does not have the attribute."""
        return AttributeNotExists(self)

    def contains(self, value):
        """The attribute contains `value`."""
        return Contains(self, value)

    def size(self):
        """The size of the attribute, which a comparison can then be applied to."""
        return Size(self)

    def attribute_type(self, value):
        """The attribute is of the DynamoDB type `value` names."""
        return AttributeType(self, value)


class Equals(ConditionBase):
    expression_operator = "="


class NotEquals(ConditionBase):
    expression_operator = "<>"


class LessThan(ConditionBase):
    expression_operator = "<"


class LessThanEquals(ConditionBase):
    expression_operator = "<="


class GreaterThan(ConditionBase):
    expression_operator = ">"


class GreaterThanEquals(ConditionBase):
    expression_operator = ">="


class In(ConditionBase):
    expression_operator = "IN"
    has_grouped_values = True


class Between(ConditionBase):
    expression_operator = "BETWEEN"
    expression_format = "{0} {operator} {1} AND {2}"


class BeginsWith(ConditionBase):
    expression_operator = "begins_with"
    expression_format = "{operator}({0}, {1})"


class Contains(ConditionBase):
    expression_operator = "contains"
    expression_format = "{operator}({0}, {1})"


class Size(ConditionBase, AttributeBase):
    expression_operator = "size"
    expression_format = "{operator}({0})"

    def __init__(self, name):
        ConditionBase.__init__(self, name)


class AttributeType(ConditionBase):
    expression_operator = "attribute_type"
    expression_format = "{operator}({0}, {1})"


class AttributeExists(ConditionBase):
    expression_operator = "attribute_exists"
    expression_format = "{operator}({0})"


class AttributeNotExists(ConditionBase):
    expression_operator = "attribute_not_exists"
    expression_format = "{operator}({0})"


class And(ConditionBase):
    expression_operator = "AND"
    expression_format = "({0} {operator} {1})"


class Or(ConditionBase):
    expression_operator = "OR"
    expression_format = "({0} {operator} {1})"


class Not(ConditionBase):
    expression_operator = "NOT"
    expression_format = "({operator} {0})"


BuiltConditionExpression = namedtuple(
    "BuiltConditionExpression",
    [
        "condition_expression",
        "attribute_name_placeholders",
        "attribute_value_placeholders",
    ],
)


class ConditionExpressionBuilder:
    """One condition as the expression string and the placeholders a request carries."""

    def __init__(self):
        self._name_placeholder = "n"
        self._value_placeholder = "v"
        self._name_count = 0
        self._value_count = 0

    def _get_name_placeholder(self):
        return "#" + self._name_placeholder + str(self._name_count)

    def _get_value_placeholder(self):
        return ":" + self._value_placeholder + str(self._value_count)

    def reset(self):
        """Start the placeholder names over, as one call's own."""
        self._name_count = 0
        self._value_count = 0

    def build_expression(self, condition, is_key_condition=False):
        if not isinstance(condition, ConditionBase):
            raise DynamoDBNeedsConditionError(condition)
        attribute_name_placeholders = {}
        attribute_value_placeholders = {}
        condition_expression = self._build_expression(
            condition,
            attribute_name_placeholders,
            attribute_value_placeholders,
            is_key_condition,
        )
        return BuiltConditionExpression(
            condition_expression=condition_expression,
            attribute_name_placeholders=attribute_name_placeholders,
            attribute_value_placeholders=attribute_value_placeholders,
        )

    def _build_expression(self, condition, names, values, is_key_condition):
        expression = condition.get_expression()
        replaced = []
        for value in expression["values"]:
            replaced.append(
                self._build_expression_component(
                    value, names, values, is_key_condition, condition.has_grouped_values
                )
            )
        return expression["format"].format(*replaced, operator=expression["operator"])

    def _build_expression_component(
        self, value, names, values, is_key_condition, has_grouped_values
    ):
        if isinstance(value, ConditionBase):
            return self._build_expression(value, names, values, is_key_condition)
        if isinstance(value, AttributeBase):
            if is_key_condition and not isinstance(value, Key):
                raise DynamoDBNeedsKeyConditionError(
                    "Attribute object %s is must be of type Key" % value.name
                )
            return self._build_name_placeholder(value, names)
        if has_grouped_values:
            return "(%s)" % ", ".join(
                self._build_value_placeholder(member, values) for member in value
            )
        return self._build_value_placeholder(value, values)

    def _build_name_placeholder(self, value, attribute_name_placeholders):
        attribute_name = value.name
        # Every part of the name is substituted, so a name that is a reserved word or
        # carries a character an expression would otherwise read is never written into one.
        parts = ATTR_NAME_REGEX.findall(attribute_name)
        placeholder_format = ATTR_NAME_REGEX.sub("%s", attribute_name)
        placeholders = []
        for part in parts:
            placeholder = self._get_name_placeholder()
            self._name_count += 1
            placeholders.append(placeholder)
            attribute_name_placeholders[placeholder] = part
        return placeholder_format % tuple(placeholders)

    def _build_value_placeholder(self, value, attribute_value_placeholders):
        placeholder = self._get_value_placeholder()
        self._value_count += 1
        attribute_value_placeholders[placeholder] = value
        return placeholder
"##;

/// `boto3.dynamodb.transform`: where one operation carries attribute values and conditions.
const DYNAMODB_TRANSFORM_SOURCE: &str = r##"
"""The document transformation the DynamoDB resource interface is.

A resource-side call carries plain Python values where the operation's own model carries
attribute values, and a condition object where it carries an expression string. The tables
below say WHERE that is, per operation, and the client here is the client those tables are
applied around: `resource.meta.client`.
"""

from botocore.client import _operation_name
from boto3.dynamodb.conditions import ConditionBase, ConditionExpressionBuilder
from boto3.dynamodb.types import TypeDeserializer, TypeSerializer

SERIALIZER = TypeSerializer()
DESERIALIZER = TypeDeserializer()

# How a path below addresses what it names: a member by name, every value of a map, or
# every element of a list.
EVERY_VALUE = "*"
EVERY_ELEMENT = "[]"

# What a path's leaf holds: a map of attribute names to attribute values, one attribute
# value, or a list of them.
ITEM = "item"
VALUE = "value"
VALUES = "values"

# Where each operation's INPUT carries attribute values, from the DynamoDB model: a member
# whose target is `AttributeValue` (`Value`), a map of them (`Key`, `PutItemInputAttributeMap`,
# `ExpressionAttributeValueMap`) or a list of them (`AttributeValueList`,
# `PreparedStatementParameters`, `KeyList`).
INPUT_ATTRIBUTE_VALUES = {
    "PutItem": (
        (("Item",), ITEM),
        (("ExpressionAttributeValues",), ITEM),
        (("Expected", EVERY_VALUE, "Value"), VALUE),
        (("Expected", EVERY_VALUE, "AttributeValueList"), VALUES),
    ),
    "GetItem": ((("Key",), ITEM),),
    "UpdateItem": (
        (("Key",), ITEM),
        (("ExpressionAttributeValues",), ITEM),
        (("AttributeUpdates", EVERY_VALUE, "Value"), VALUE),
        (("Expected", EVERY_VALUE, "Value"), VALUE),
        (("Expected", EVERY_VALUE, "AttributeValueList"), VALUES),
    ),
    "DeleteItem": (
        (("Key",), ITEM),
        (("ExpressionAttributeValues",), ITEM),
        (("Expected", EVERY_VALUE, "Value"), VALUE),
        (("Expected", EVERY_VALUE, "AttributeValueList"), VALUES),
    ),
    "Query": (
        (("ExclusiveStartKey",), ITEM),
        (("ExpressionAttributeValues",), ITEM),
        (("KeyConditions", EVERY_VALUE, "AttributeValueList"), VALUES),
        (("QueryFilter", EVERY_VALUE, "AttributeValueList"), VALUES),
    ),
    "Scan": (
        (("ExclusiveStartKey",), ITEM),
        (("ExpressionAttributeValues",), ITEM),
        (("ScanFilter", EVERY_VALUE, "AttributeValueList"), VALUES),
    ),
    "BatchGetItem": (
        (("RequestItems", EVERY_VALUE, "Keys", EVERY_ELEMENT), ITEM),
    ),
    "BatchWriteItem": (
        (("RequestItems", EVERY_VALUE, EVERY_ELEMENT, "PutRequest", "Item"), ITEM),
        (("RequestItems", EVERY_VALUE, EVERY_ELEMENT, "DeleteRequest", "Key"), ITEM),
    ),
    "TransactGetItems": ((("TransactItems", EVERY_ELEMENT, "Get", "Key"), ITEM),),
    "TransactWriteItems": (
        (("TransactItems", EVERY_ELEMENT, "ConditionCheck", "Key"), ITEM),
        (
            ("TransactItems", EVERY_ELEMENT, "ConditionCheck", "ExpressionAttributeValues"),
            ITEM,
        ),
        (("TransactItems", EVERY_ELEMENT, "Put", "Item"), ITEM),
        (("TransactItems", EVERY_ELEMENT, "Put", "ExpressionAttributeValues"), ITEM),
        (("TransactItems", EVERY_ELEMENT, "Delete", "Key"), ITEM),
        (("TransactItems", EVERY_ELEMENT, "Delete", "ExpressionAttributeValues"), ITEM),
        (("TransactItems", EVERY_ELEMENT, "Update", "Key"), ITEM),
        (("TransactItems", EVERY_ELEMENT, "Update", "ExpressionAttributeValues"), ITEM),
    ),
    "ExecuteStatement": ((("Parameters",), VALUES),),
    "BatchExecuteStatement": (
        (("Statements", EVERY_ELEMENT, "Parameters"), VALUES),
    ),
    "ExecuteTransaction": (
        (("TransactStatements", EVERY_ELEMENT, "Parameters"), VALUES),
    ),
}

# Where each operation's OUTPUT carries attribute values, from the same model: `AttributeMap`
# (`Item`, `Attributes`), `Key` (`LastEvaluatedKey`), `ItemList` (`Items`) and
# `ItemCollectionKeyAttributeMap`.
OUTPUT_ATTRIBUTE_VALUES = {
    "GetItem": ((("Item",), ITEM),),
    "PutItem": (
        (("Attributes",), ITEM),
        (("ItemCollectionMetrics", "ItemCollectionKey"), ITEM),
    ),
    "UpdateItem": (
        (("Attributes",), ITEM),
        (("ItemCollectionMetrics", "ItemCollectionKey"), ITEM),
    ),
    "DeleteItem": (
        (("Attributes",), ITEM),
        (("ItemCollectionMetrics", "ItemCollectionKey"), ITEM),
    ),
    "Query": ((("Items", EVERY_ELEMENT), ITEM), (("LastEvaluatedKey",), ITEM)),
    "Scan": ((("Items", EVERY_ELEMENT), ITEM), (("LastEvaluatedKey",), ITEM)),
    "BatchGetItem": (
        (("Responses", EVERY_VALUE, EVERY_ELEMENT), ITEM),
        (("UnprocessedKeys", EVERY_VALUE, "Keys", EVERY_ELEMENT), ITEM),
    ),
    "BatchWriteItem": (
        (("UnprocessedItems", EVERY_VALUE, EVERY_ELEMENT, "PutRequest", "Item"), ITEM),
        (("UnprocessedItems", EVERY_VALUE, EVERY_ELEMENT, "DeleteRequest", "Key"), ITEM),
        (
            ("ItemCollectionMetrics", EVERY_VALUE, EVERY_ELEMENT, "ItemCollectionKey"),
            ITEM,
        ),
    ),
    "TransactGetItems": ((("Responses", EVERY_ELEMENT, "Item"), ITEM),),
    "TransactWriteItems": (
        (
            ("ItemCollectionMetrics", EVERY_VALUE, EVERY_ELEMENT, "ItemCollectionKey"),
            ITEM,
        ),
    ),
    "ExecuteStatement": (
        (("Items", EVERY_ELEMENT), ITEM),
        (("LastEvaluatedKey",), ITEM),
    ),
    "BatchExecuteStatement": ((("Responses", EVERY_ELEMENT, "Item"), ITEM),),
    "ExecuteTransaction": ((("Responses", EVERY_ELEMENT, "Item"), ITEM),),
}

# Where each operation carries a CONDITION: the structure holding the member, the member
# itself, and whether it is a key condition — the members the model targets at
# `ConditionExpression` and at `KeyExpression`.
INPUT_CONDITIONS = {
    "PutItem": (((), "ConditionExpression", False),),
    "UpdateItem": (((), "ConditionExpression", False),),
    "DeleteItem": (((), "ConditionExpression", False),),
    "Query": (
        ((), "KeyConditionExpression", True),
        ((), "FilterExpression", False),
    ),
    "Scan": (((), "FilterExpression", False),),
    "TransactWriteItems": (
        (("TransactItems", EVERY_ELEMENT, "ConditionCheck"), "ConditionExpression", False),
        (("TransactItems", EVERY_ELEMENT, "Put"), "ConditionExpression", False),
        (("TransactItems", EVERY_ELEMENT, "Delete"), "ConditionExpression", False),
        (("TransactItems", EVERY_ELEMENT, "Update"), "ConditionExpression", False),
    ),
}


def _mapped(node, path, convert):
    """`node` with everything the rest of `path` names replaced by `convert` of it."""
    if not path:
        return convert(node)
    step = path[0]
    rest = path[1:]
    if step == EVERY_ELEMENT:
        if not isinstance(node, list):
            return node
        return [_mapped(element, rest, convert) for element in node]
    if step == EVERY_VALUE:
        if not isinstance(node, dict):
            return node
        return {key: _mapped(value, rest, convert) for key, value in node.items()}
    if not isinstance(node, dict) or node.get(step) is None:
        return node
    replaced = dict(node)
    replaced[step] = _mapped(node[step], rest, convert)
    return replaced


def _transform(document, path, convert):
    """Replace what `path` names in `document`, in place at its first step."""
    if not isinstance(document, dict):
        return
    head = path[0]
    if document.get(head) is None:
        return
    document[head] = _mapped(document[head], path[1:], convert)


def _structures(document, path):
    """Every structure `path` names inside `document`."""
    if not path:
        return [document] if isinstance(document, dict) else []
    step = path[0]
    rest = path[1:]
    if step == EVERY_ELEMENT:
        if not isinstance(document, list):
            return []
        found = []
        for element in document:
            found.extend(_structures(element, rest))
        return found
    if step == EVERY_VALUE:
        if not isinstance(document, dict):
            return []
        found = []
        for value in document.values():
            found.extend(_structures(value, rest))
        return found
    if not isinstance(document, dict) or document.get(step) is None:
        return []
    return _structures(document[step], rest)


def _serialize_item(item):
    return {name: SERIALIZER.serialize(value) for name, value in item.items()}


def _deserialize_item(item):
    return {name: DESERIALIZER.deserialize(value) for name, value in item.items()}


def _converter(kind, serializing):
    if kind == ITEM:
        return _serialize_item if serializing else _deserialize_item
    if kind == VALUE:
        return SERIALIZER.serialize if serializing else DESERIALIZER.deserialize
    one = SERIALIZER.serialize if serializing else DESERIALIZER.deserialize
    return lambda values: [one(value) for value in values]


def inject_condition_expressions(operation_name, parameters):
    """Build every condition object in `parameters` into the expression it stands for.

    The placeholders the expression uses are merged into the structure's own
    `ExpressionAttributeNames` and `ExpressionAttributeValues`, beside any the caller wrote
    itself; the values are still plain Python ones, which the attribute-value injection
    below then serializes.
    """
    sites = INPUT_CONDITIONS.get(operation_name)
    if not sites:
        return
    builder = ConditionExpressionBuilder()
    for path, member, is_key_condition in sites:
        for structure in _structures(parameters, path):
            condition = structure.get(member)
            if not isinstance(condition, ConditionBase):
                continue
            built = builder.build_expression(condition, is_key_condition=is_key_condition)
            structure[member] = built.condition_expression
            if built.attribute_name_placeholders:
                names = dict(structure.get("ExpressionAttributeNames") or {})
                names.update(built.attribute_name_placeholders)
                structure["ExpressionAttributeNames"] = names
            if built.attribute_value_placeholders:
                values = dict(structure.get("ExpressionAttributeValues") or {})
                values.update(built.attribute_value_placeholders)
                structure["ExpressionAttributeValues"] = values


def inject_attribute_values(operation_name, parameters):
    """Serialize every plain Python value the operation's input carries as an attribute."""
    for path, kind in INPUT_ATTRIBUTE_VALUES.get(operation_name, ()):
        _transform(parameters, path, _converter(kind, True))


def read_attribute_values(operation_name, answered):
    """Read every attribute value the operation's answer carries back to Python."""
    for path, kind in OUTPUT_ATTRIBUTE_VALUES.get(operation_name, ()):
        _transform(answered, path, _converter(kind, False))
    return answered


class DocumentClient:
    """The DynamoDB client of the resource interface: items are plain Python values.

    Every method is the underlying client's own — the same one host call, the same
    signature, the same refusal classes — with the transformation above applied to what
    goes out and what comes back.
    """

    def __init__(self, client):
        self._client = client
        self.exceptions = client.exceptions
        self.meta = client.meta

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        operation_name = _operation_name(name)
        call = getattr(self._client, name)

        def method(**parameters):
            inject_condition_expressions(operation_name, parameters)
            inject_attribute_values(operation_name, parameters)
            return read_attribute_values(operation_name, call(**parameters))

        method.__name__ = name
        return method

    def can_paginate(self, operation_name):
        return self._client.can_paginate(operation_name)

    def get_paginator(self, operation_name):
        return self._client.get_paginator(operation_name)

    def get_waiter(self, waiter_name):
        return self._client.get_waiter(waiter_name)

    @property
    def waiter_names(self):
        return self._client.waiter_names

    def close(self):
        return self._client.close()
"##;

/// `boto3.dynamodb.table`: the service resource, its tables, and the batch writer.
const DYNAMODB_TABLE_SOURCE: &str = r##"
"""The DynamoDB resources themselves: the service resource, a `Table`, and its writer."""

from botocore import xform_name

from boto3.dynamodb.transform import DocumentClient
from boto3.resources.base import ResourceMeta


class BatchWriter:
    """Buffers writes and sends them as `BatchWriteItem` requests.

    The buffer is flushed whenever it holds a full batch and once more when the context
    manager is left, and an item the service reported as unprocessed goes back on the
    buffer to be sent again.
    """

    def __init__(self, table_name, client, flush_amount=25, overwrite_by_pkeys=None):
        self._table_name = table_name
        self._client = client
        self._items_buffer = []
        self._flush_amount = flush_amount
        self._overwrite_by_pkeys = overwrite_by_pkeys

    def put_item(self, Item):
        self._add_request_and_process({"PutRequest": {"Item": Item}})

    def delete_item(self, Key):
        self._add_request_and_process({"DeleteRequest": {"Key": Key}})

    def _add_request_and_process(self, request):
        if self._overwrite_by_pkeys:
            self._remove_dup_pkeys_request_if_any(request)
        self._items_buffer.append(request)
        self._flush_if_needed()

    def _remove_dup_pkeys_request_if_any(self, request):
        pkey_values_new = self._extract_pkey_values(request)
        for item in list(self._items_buffer):
            if self._extract_pkey_values(item) == pkey_values_new:
                self._items_buffer.remove(item)

    def _extract_pkey_values(self, request):
        if request.get("PutRequest"):
            return [
                request["PutRequest"]["Item"][key] for key in self._overwrite_by_pkeys
            ]
        if request.get("DeleteRequest"):
            return [
                request["DeleteRequest"]["Key"][key] for key in self._overwrite_by_pkeys
            ]
        return None

    def _flush_if_needed(self):
        if len(self._items_buffer) >= self._flush_amount:
            self._flush()

    def _flush(self):
        items_to_send = self._items_buffer[: self._flush_amount]
        self._items_buffer = self._items_buffer[self._flush_amount :]
        answered = self._client.batch_write_item(
            RequestItems={self._table_name: items_to_send}
        )
        unprocessed_items = answered.get("UnprocessedItems") or {}
        self._items_buffer.extend(unprocessed_items.get(self._table_name, []))

    def __enter__(self):
        return self

    def __exit__(self, exception_type, exception_value, traceback):
        while self._items_buffer:
            self._flush()
        return None


class Table:
    """One table of the resource interface: `dynamodb.Table(name)`.

    Its methods are the table's own operations with the table name carried for you, and its
    attributes are what `DescribeTable` reports for it, read when one is first asked for.
    """

    def __init__(self, name, client):
        self.meta = ResourceMeta("dynamodb", client, identifiers=["name"])
        self.name = name

    def __repr__(self):
        return "dynamodb.Table(name=%r)" % (self.name,)

    def __eq__(self, other):
        return isinstance(other, Table) and self.name == other.name

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash(("dynamodb.Table", self.name))

    def __getattr__(self, name):
        if name.startswith("_") or name in ("meta", "name"):
            raise AttributeError(name)
        if self.meta.data is None:
            self.load()
        if name in self.meta.data:
            return self.meta.data[name]
        raise AttributeError("'dynamodb.Table' object has no attribute '%s'" % name)

    def load(self):
        """Read the table's own attributes, as `DescribeTable` reports them."""
        answered = self.meta.client.describe_table(TableName=self.name)
        described = answered.get("Table") or {}
        self.meta.data = {xform_name(key): value for key, value in described.items()}

    reload = load

    def put_item(self, **kwargs):
        return self.meta.client.put_item(TableName=self.name, **kwargs)

    def get_item(self, **kwargs):
        return self.meta.client.get_item(TableName=self.name, **kwargs)

    def update_item(self, **kwargs):
        return self.meta.client.update_item(TableName=self.name, **kwargs)

    def delete_item(self, **kwargs):
        return self.meta.client.delete_item(TableName=self.name, **kwargs)

    def query(self, **kwargs):
        return self.meta.client.query(TableName=self.name, **kwargs)

    def scan(self, **kwargs):
        return self.meta.client.scan(TableName=self.name, **kwargs)

    def delete(self, **kwargs):
        return self.meta.client.delete_table(TableName=self.name, **kwargs)

    def update(self, **kwargs):
        answered = self.meta.client.update_table(TableName=self.name, **kwargs)
        self.meta.data = None
        return answered

    def wait_until_exists(self, **kwargs):
        """Wait until this table exists, which is the table waiter of that state."""
        self.meta.client.get_waiter("table_exists").wait(TableName=self.name, **kwargs)

    def wait_until_not_exists(self, **kwargs):
        """Wait until this table is gone, which is the table waiter of that state."""
        self.meta.client.get_waiter("table_not_exists").wait(
            TableName=self.name, **kwargs
        )

    def batch_writer(self, overwrite_by_pkeys=None, flush_amount=25):
        """A writer that sends this table's writes in batches."""
        return BatchWriter(
            self.name,
            self.meta.client,
            flush_amount=flush_amount,
            overwrite_by_pkeys=overwrite_by_pkeys,
        )


class TableCollection:
    """`dynamodb.tables`: the tables of this account and Region, as `Table` resources."""

    def __init__(self, resource, limit=None, page_size=None):
        self._resource = resource
        self._limit = limit
        self._page_size = page_size

    def all(self):
        return TableCollection(self._resource, self._limit, self._page_size)

    def filter(self, **kwargs):
        return TableCollection(
            self._resource, kwargs.get("Limit", self._limit), self._page_size
        )

    def limit(self, count):
        return TableCollection(self._resource, count, self._page_size)

    def page_size(self, count):
        return TableCollection(self._resource, self._limit, count)

    def __iter__(self):
        returned = 0
        start = None
        while True:
            parameters = {}
            if self._page_size is not None:
                parameters["Limit"] = self._page_size
            if start is not None:
                parameters["ExclusiveStartTableName"] = start
            answered = self._resource.meta.client.list_tables(**parameters)
            for name in answered.get("TableNames") or []:
                if self._limit is not None and returned >= self._limit:
                    return
                returned += 1
                yield self._resource.Table(name)
            start = answered.get("LastEvaluatedTableName")
            if not start:
                return


class ServiceResource:
    """`boto3.resource("dynamodb")`: the service resource DynamoDB's tables hang off."""

    def __init__(self, client):
        self.meta = ResourceMeta("dynamodb", client)

    def __repr__(self):
        return "dynamodb.ServiceResource()"

    @property
    def tables(self):
        return TableCollection(self)

    def Table(self, name):
        """The table of that name, as a resource."""
        return Table(name, self.meta.client)

    def create_table(self, **kwargs):
        """Create a table, answering it as a resource."""
        answered = self.meta.client.create_table(**kwargs)
        described = answered.get("TableDescription") or {}
        return self.Table(described.get("TableName") or kwargs.get("TableName"))

    def batch_get_item(self, **kwargs):
        return self.meta.client.batch_get_item(**kwargs)

    def batch_write_item(self, **kwargs):
        return self.meta.client.batch_write_item(**kwargs)


def service_resource(client):
    """The DynamoDB service resource over `client`, items and all."""
    return ServiceResource(DocumentClient(client))
"##;
