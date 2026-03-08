import 'client.dart';
import 'collection.dart';
import 'document.dart';
import 'query.dart';

/// Subcollection support via naming conventions.
///
/// Since SurrealDB doesn't have native subcollections like Firestore,
/// this emulates them using a double-underscore naming convention
/// and an automatic `parent_id` field.
///
/// Firestore: `users/{uid}/orders` → SurrealDB: `users__orders` (with `parent_id` field)
///
/// ```dart
/// final orders = ob.collection('users').subcollection('user123', 'orders');
/// // queries users__orders WHERE parent_id = 'users:user123'
/// ```
class SubcollectionRef extends Query {
  final OrignaBase _subclient;
  final String parentCollection;
  final String parentId;
  final String childCollection;

  SubcollectionRef(
    this._subclient,
    this.parentCollection,
    this.parentId,
    this.childCollection,
  ) : super(_subclient, '${parentCollection}__$childCollection');

  /// The actual SurrealDB collection name uses double underscore separator.
  String get collectionPath => collectionName;

  /// Get a document reference within this subcollection.
  DocumentRef doc(String id) => DocumentRef(_subclient, collectionPath, id);

  /// Add a document with the parent reference automatically included.
  Future<Document> add(Map<String, dynamic> data) async {
    final enriched = {
      ...data,
      'parent_id': '$parentCollection:$parentId',
      'parent_collection': parentCollection,
    };
    return CollectionRef(_subclient, collectionPath).add(enriched);
  }

  /// Override get() to filter by parent_id automatically.
  @override
  Future<QuerySnapshot> get() async {
    return _parentFilteredQuery().get();
  }

  /// Get a nested subcollection (e.g., `users/{uid}/orders/{oid}/items`).
  SubcollectionRef subcollection(String docId, String nestedCollection) {
    return SubcollectionRef(
        _subclient, collectionPath, docId, nestedCollection);
  }

  /// Add a filter condition, automatically including the parent_id filter.
  @override
  Query where(
    String field, {
    dynamic isEqualTo,
    dynamic isNotEqualTo,
    dynamic isGreaterThan,
    dynamic isGreaterThanOrEqualTo,
    dynamic isLessThan,
    dynamic isLessThanOrEqualTo,
    List<dynamic>? whereIn,
    dynamic contains,
    dynamic startsWith,
  }) {
    // Start with parent filter, then chain user's filter
    var q = _parentFilteredQuery();
    return q.where(
      field,
      isEqualTo: isEqualTo,
      isNotEqualTo: isNotEqualTo,
      isGreaterThan: isGreaterThan,
      isGreaterThanOrEqualTo: isGreaterThanOrEqualTo,
      isLessThan: isLessThan,
      isLessThanOrEqualTo: isLessThanOrEqualTo,
      whereIn: whereIn,
      contains: contains,
      startsWith: startsWith,
    );
  }

  @override
  Query orderBy(String field, {bool descending = false}) {
    return _parentFilteredQuery().orderBy(field, descending: descending);
  }

  @override
  Query limit(int count) {
    return _parentFilteredQuery().limit(count);
  }

  @override
  Query offset(int count) {
    return _parentFilteredQuery().offset(count);
  }

  /// Creates a base query scoped to this parent document.
  Query _parentFilteredQuery() {
    return Query(_subclient, collectionPath)
        .where('parent_id', isEqualTo: '$parentCollection:$parentId');
  }
}
