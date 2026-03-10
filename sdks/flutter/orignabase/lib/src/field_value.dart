/// Sentinel values for special write operations, matching Firestore's FieldValue.
///
/// ```dart
/// await ob.collection('counters').doc('page').update({
///   'views': FieldValue.increment(1),
///   'updated_at': FieldValue.serverTimestamp(),
///   'tags': FieldValue.arrayUnion(['dart', 'flutter']),
///   'old_tags': FieldValue.arrayRemove(['deprecated']),
///   'temp_field': FieldValue.delete(),
/// });
/// ```
class FieldValue {
  final _FieldValueType _type;
  final dynamic _value;

  const FieldValue._(this._type, [this._value]);

  /// Sets the field to the server's current timestamp.
  static FieldValue serverTimestamp() =>
      const FieldValue._(_FieldValueType.serverTimestamp);

  /// Increments a numeric field by [value] (can be negative for decrement).
  static FieldValue increment(num value) =>
      FieldValue._(_FieldValueType.increment, value);

  /// Adds elements to an array field (only adds elements not already present).
  static FieldValue arrayUnion(List<dynamic> elements) =>
      FieldValue._(_FieldValueType.arrayUnion, elements);

  /// Removes elements from an array field.
  static FieldValue arrayRemove(List<dynamic> elements) =>
      FieldValue._(_FieldValueType.arrayRemove, elements);

  /// Deletes the field from the document.
  static FieldValue delete() =>
      const FieldValue._(_FieldValueType.deleteField);

  /// Convert to API-compatible map representation.
  ///
  /// Produces `{fieldName: {_marker: value}}` format matching the server's
  /// `update_with_field_values` expectations.
  Map<String, dynamic> toApiMap(String fieldName) {
    return switch (_type) {
      _FieldValueType.serverTimestamp => {fieldName: {'_serverTimestamp': true}},
      _FieldValueType.increment => {fieldName: {'_increment': _value}},
      _FieldValueType.arrayUnion => {fieldName: {'_arrayUnion': _value}},
      _FieldValueType.arrayRemove => {fieldName: {'_arrayRemove': _value}},
      _FieldValueType.deleteField => {fieldName: {'_deleteField': true}},
    };
  }

  @override
  String toString() => 'FieldValue($_type, $_value)';
}

enum _FieldValueType {
  serverTimestamp,
  increment,
  arrayUnion,
  arrayRemove,
  deleteField,
}
