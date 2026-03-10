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
    return Document(id: id, collection: collection, data: data);
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

  QuerySnapshot({required this.docs, this.hasMore = false}) : size = docs.length;

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
