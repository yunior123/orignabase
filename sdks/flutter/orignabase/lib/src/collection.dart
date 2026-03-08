import 'dart:convert';
import 'client.dart';
import 'document.dart';
import 'query.dart';
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

  /// Add a new document to the collection.
  Future<Document> add(Map<String, dynamic> data) async {
    final response = await client.graphql(
      'mutation { create(collection: "$collectionName", data: ${jsonEncode(jsonEncode(data))}) }',
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

  /// Get the document data.
  Future<Document?> get() async {
    final response = await _client.graphql(
      'query { get(collection: "$collection", id: "$id") }',
    );

    final result = response['data']?['get'];
    if (result is Map<String, dynamic>) {
      return Document.fromMap(collection, result);
    }
    return null;
  }

  /// Update the document with new data (merge).
  Future<Document?> update(Map<String, dynamic> data) async {
    final response = await _client.graphql(
      'mutation { update(collection: "$collection", id: "$id", data: ${jsonEncode(jsonEncode(data))}) }',
    );

    final result = response['data']?['update'];
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
}
