/// Converts a Dart value to a GraphQL literal string.
///
/// GraphQL uses unquoted keys in objects: `{name: "Alice", age: 30}`
/// vs JSON's `{"name": "Alice", "age": 30}`.
String toGraphQLValue(dynamic value) {
  if (value == null) return 'null';
  if (value is String) return '"${_escapeString(value)}"';
  if (value is num || value is bool) return value.toString();
  if (value is List) {
    final items = value.map(toGraphQLValue).join(', ');
    return '[$items]';
  }
  if (value is Map) {
    final entries = value.entries.map((e) {
      final key = e.key.toString();
      return '$key: ${toGraphQLValue(e.value)}';
    }).join(', ');
    return '{$entries}';
  }
  // Fallback: treat as string
  return '"${_escapeString(value.toString())}"';
}

String _escapeString(String s) {
  return s
      .replaceAll('\\', '\\\\')
      .replaceAll('"', '\\"')
      .replaceAll('\n', '\\n')
      .replaceAll('\r', '\\r')
      .replaceAll('\t', '\\t');
}
