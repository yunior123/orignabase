import 'dart:async';
import 'client.dart';
import 'collection.dart';
import 'document.dart';
import 'query.dart';
import 'realtime.dart';

/// Subcollection support via naming conventions.
///
/// Since PostgreSQL doesn't have native subcollections like Firestore,
/// this emulates them using a double-underscore naming convention
/// and an automatic `parent_id` field.
///
/// Firestore: `users/{uid}/orders` → PostgreSQL: `users__orders` (with `parent_id` field)
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

  /// The actual PostgreSQL collection name uses double underscore separator.
  String get collectionPath => collectionName;

  /// The parent filter value used to scope queries.
  String get _parentFilterValue => '$parentCollection:$parentId';

  /// Get a document reference within this subcollection.
  DocumentRef doc(String id) => DocumentRef(_subclient, collectionPath, id);

  /// Add a document with the parent reference automatically included.
  Future<Document> add(Map<String, dynamic> data) async {
    final enriched = {
      ...data,
      'parent_id': _parentFilterValue,
      'parent_collection': parentCollection,
    };
    return CollectionRef(_subclient, collectionPath).add(enriched);
  }

  /// Override get() to always include the parent_id filter, even when called
  /// on a plain Query returned by chained methods like .where().orderBy().
  @override
  Future<QuerySnapshot> get() async {
    // Prepend parent_id filter then delegate to base get()
    final withParent = super.where('parent_id', isEqualTo: _parentFilterValue);
    return withParent.get();
  }

  /// Get a nested subcollection (e.g., `users/{uid}/orders/{oid}/items`).
  SubcollectionRef subcollection(String docId, String nestedCollection) {
    return SubcollectionRef(
        _subclient, collectionPath, docId, nestedCollection);
  }

  /// Returns a _SubcollectionQuery that preserves the parent filter context
  /// through all subsequent chaining operations.
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
    final baseQuery = super.where(
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
    return _SubcollectionQuery(baseQuery, parentCollection, parentId);
  }

  @override
  Query orderBy(String field, {bool descending = false}) {
    return _SubcollectionQuery(
      super.orderBy(field, descending: descending),
      parentCollection,
      parentId,
    );
  }

  @override
  Query limit(int count) {
    return _SubcollectionQuery(
      super.limit(count),
      parentCollection,
      parentId,
    );
  }

  @override
  Query offset(int count) {
    return _SubcollectionQuery(
      super.offset(count),
      parentCollection,
      parentId,
    );
  }

  /// Listen to realtime changes on this subcollection.
  ///
  /// Uses the shared RealtimeClient from the OrignaBase instance.
  Stream<DocumentChange> snapshots() {
    final stream = _subclient.realtime.subscribe(collectionPath);
    final controller = StreamController<DocumentChange>.broadcast();
    stream.listen(
      (change) {
        // Only forward changes for docs belonging to this parent
        final parentRef = change.document.data['parent_id'];
        if (parentRef == _parentFilterValue || parentRef == null) {
          controller.add(change);
        }
      },
      onError: controller.addError,
      onDone: controller.close,
    );
    return controller.stream;
  }
}

/// A Query wrapper that injects the parent_id filter on get().
///
/// This ensures that no matter how many times the query is chained
/// (.where().orderBy().limit()), the parent filter is always applied
/// when the query is finally executed.
class _SubcollectionQuery extends Query {
  final Query _inner;
  final String _parentCollection;
  final String _parentId;

  _SubcollectionQuery(this._inner, this._parentCollection, this._parentId)
      : super(_inner.client, _inner.collectionName);

  String get _parentFilterValue => '$_parentCollection:$_parentId';

  @override
  Future<QuerySnapshot> get() async {
    // Inject parent_id filter into the inner query before executing
    final withParent = _inner.where('parent_id', isEqualTo: _parentFilterValue);
    return withParent.get();
  }

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
    return _SubcollectionQuery(
      _inner.where(
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
      ),
      _parentCollection,
      _parentId,
    );
  }

  @override
  Query orderBy(String field, {bool descending = false}) {
    return _SubcollectionQuery(
      _inner.orderBy(field, descending: descending),
      _parentCollection,
      _parentId,
    );
  }

  @override
  Query limit(int count) {
    return _SubcollectionQuery(
      _inner.limit(count),
      _parentCollection,
      _parentId,
    );
  }

  @override
  Query offset(int count) {
    return _SubcollectionQuery(
      _inner.offset(count),
      _parentCollection,
      _parentId,
    );
  }
}
