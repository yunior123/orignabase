/// Represents a document retrieved from OrignaBase.
class Document {
  /// The document ID.
  final String id;

  /// The collection this document belongs to.
  final String collection;

  /// The document data as a map.
  final Map<String, dynamic> data;

  Document({
    required this.id,
    required this.collection,
    required this.data,
  });

  /// Whether this document exists (has a non-empty ID).
  bool get exists => id.isNotEmpty;

  /// Get a typed field value.
  T? get<T>(String field) => data[field] as T?;

  /// Get a field value by key.
  dynamic operator [](String key) => data[key];

  /// Check if a field exists.
  bool containsKey(String key) => data.containsKey(key);

  /// Create from a raw API response map.
  factory Document.fromMap(String collection, Map<String, dynamic> map) {
    final id = (map['id'] ?? map['_id'] ?? '').toString();
    final data = Map<String, dynamic>.from(map)
      ..remove('id')
      ..remove('_id')
      ..remove('_rev')
      ..remove('_created')
      ..remove('_updated');
    // PostgreSQL returns nanosecond-precision timestamps (9 decimal digits).
    // Dart's DateTime.parse only supports up to microseconds (6 digits).
    // Truncate any ISO-8601 strings with >6 subsecond digits.
    _normalizeTimestamps(data);
    return Document(id: id, collection: collection, data: data);
  }

  static final _nanoPattern =
      RegExp(r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6})\d+');

  static void _normalizeTimestamps(Map<String, dynamic> data) {
    for (final key in data.keys.toList()) {
      final v = data[key];
      if (v is String) {
        data[key] = v.replaceAllMapped(_nanoPattern, (m) => m.group(1)!);
      } else if (v is Map<String, dynamic>) {
        _normalizeTimestamps(v);
      }
    }
  }

  @override
  String toString() => 'Document($collection/$id: $data)';
}

/// A snapshot of query results, similar to Firestore's QuerySnapshot.
class QuerySnapshot {
  final List<Document> docs;
  final int size;

  /// Whether there are more results available (cursor pagination).
  final bool hasMore;

  QuerySnapshot({required this.docs, this.hasMore = false})
      : size = docs.length;

  bool get isEmpty => docs.isEmpty;
  bool get isNotEmpty => docs.isNotEmpty;

  /// The last document in the snapshot, for cursor-based pagination.
  ///
  /// ```dart
  /// final page1 = await query.limit(20).get();
  /// if (page1.hasMore) {
  ///   final page2 = await query.startAfter(page1.lastDocument!).limit(20).get();
  /// }
  /// ```
  Document? get lastDocument => docs.isNotEmpty ? docs.last : null;
}
