import 'dart:async';
import 'client.dart';
import 'document.dart';
import 'errors.dart';
import 'field_value.dart';
import 'graphql_utils.dart';
import 'query.dart';
import 'realtime.dart';
import 'subcollection.dart';

/// A reference to a collection, providing Firestore-like CRUD operations.
///
/// ```dart
/// final products = ob.collection('products');
/// final doc = await products.doc('abc123').get();
/// await products.add({'title': 'Widget', 'price': 29.99});
/// ```
class CollectionRef extends Query {
  CollectionRef(super.client, super.collectionName);

  /// Get a document reference by ID.
  DocumentRef doc(String id) => DocumentRef(client, collectionName, id);

  /// Access a subcollection of a document in this collection.
  ///
  /// Example: `client.collection('users').subcollection('uid123', 'orders')`
  SubcollectionRef subcollection(String docId, String childCollection) {
    return SubcollectionRef(client, collectionName, docId, childCollection);
  }

  /// Listen to realtime changes on this collection.
  ///
  /// Uses the shared RealtimeClient from the OrignaBase instance
  /// to avoid opening a new WebSocket per subscription.
  Stream<DocumentChange> snapshots() {
    final stream = client.realtime.subscribe(collectionName);
    final controller = StreamController<DocumentChange>.broadcast();
    stream.listen(
      controller.add,
      onError: controller.addError,
      onDone: controller.close,
    );
    return controller.stream;
  }

  /// Add a new document to the collection.
  Future<Document> add(Map<String, dynamic> data) async {
    final response = await client.graphql(
      'mutation { create(collection: "$collectionName", data: ${toGraphQLValue(data)}) }',
    );

    final result = response['data']?['create'];
    if (result is Map<String, dynamic>) {
      return Document.fromMap(collectionName, result);
    }
    // Return with data if no structured response
    return Document(id: '', collection: collectionName, data: data);
  }
}

/// A reference to a specific document.
class DocumentRef {
  final OrignaBase _client;
  final String collection;
  final String id;

  DocumentRef(this._client, this.collection, this.id);

  /// Access a subcollection under this document.
  ///
  /// Example: `ob.collection('products').doc('prod1').collection('reviews')`
  SubcollectionRef subcollection(String childCollection) {
    return SubcollectionRef(_client, collection, id, childCollection);
  }

  /// Get the document data. Returns null if the document doesn't exist.
  Future<Document?> get() async {
    try {
      final response = await _client.graphql(
        'query { get(collection: "$collection", id: "$id") }',
      );

      final result = response['data']?['get'];
      if (result is Map<String, dynamic>) {
        return Document.fromMap(collection, result);
      }
      return null;
    } on NotFoundException {
      return null;
    }
  }

  /// Update the document with new data (merge).
  ///
  /// Automatically handles [FieldValue] operations (serverTimestamp,
  /// increment, arrayUnion, arrayRemove, delete).
  Future<Document?> update(Map<String, dynamic> data) async {
    // Check if any FieldValue instances are present
    final hasFieldValues = data.values.any((v) => v is FieldValue);

    // Auto-process FieldValue instances
    final processed = <String, dynamic>{};
    for (final entry in data.entries) {
      if (entry.value is FieldValue) {
        processed.addAll((entry.value as FieldValue).toApiMap(entry.key));
      } else {
        processed[entry.key] = entry.value;
      }
    }

    // Use updateWithFieldValues mutation when FieldValue markers are present
    final mutation = hasFieldValues ? 'updateWithFieldValues' : 'update';
    final response = await _client.graphql(
      'mutation { $mutation(collection: "$collection", id: "$id", data: ${toGraphQLValue(processed)}) }',
    );

    final result = response['data']?[mutation];
    if (result is Map<String, dynamic>) {
      return Document.fromMap(collection, result);
    }
    return null;
  }

  /// Create or replace the document at this explicit ID.
  Future<Document?> set(Map<String, dynamic> data) async {
    final response = await _client.graphql(
      'mutation { set(collection: "$collection", id: "$id", data: ${toGraphQLValue(data)}) }',
    );

    final result = response['data']?['set'];
    if (result is Map<String, dynamic>) {
      return Document.fromMap(collection, result);
    }
    return null;
  }

  /// Delete the document.
  Future<void> delete() async {
    await _client.graphql(
      'mutation { delete(collection: "$collection", id: "$id") }',
    );
  }

  /// Listen to realtime changes on this specific document.
  ///
  /// ```dart
  /// ob.collection('users').doc(uid).snapshots().listen((change) {
  ///   print('User data changed: ${change.document.data}');
  /// });
  /// ```
  Stream<DocumentChange> snapshots() {
    final stream = _client.realtime.subscribeDocument(collection, id);
    final controller = StreamController<DocumentChange>.broadcast();
    stream.listen(
      controller.add,
      onError: controller.addError,
      onDone: controller.close,
    );
    return controller.stream;
  }
}
