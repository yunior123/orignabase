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

  /// Get a field value by key.
  dynamic operator [](String key) => data[key];

  /// Check if a field exists.
  bool containsKey(String key) => data.containsKey(key);

  /// Create from a raw API response map.
  factory Document.fromMap(String collection, Map<String, dynamic> map) {
    final id = (map['id'] ?? map['_id'] ?? '').toString();
    final data = Map<String, dynamic>.from(map)
      ..remove('id')
      ..remove('_id');
    return Document(id: id, collection: collection, data: data);
  }

  @override
  String toString() => 'Document($collection/$id: $data)';
}

/// A snapshot of query results, similar to Firestore's QuerySnapshot.
class QuerySnapshot {
  final List<Document> docs;
  final int size;

  QuerySnapshot({required this.docs}) : size = docs.length;

  bool get isEmpty => docs.isEmpty;
  bool get isNotEmpty => docs.isNotEmpty;
}
