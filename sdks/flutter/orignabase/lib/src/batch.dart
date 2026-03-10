import 'dart:convert';
import 'client.dart';
import 'field_value.dart';

/// A batch of write operations, similar to Firestore's WriteBatch.
///
/// ```dart
/// final batch = ob.batch();
/// batch.create('products', {'title': 'Widget', 'price': 29.99});
/// batch.update('products', 'abc123', {'price': 39.99});
/// batch.delete('products', 'old-id');
/// final results = await batch.commit();
/// ```
class WriteBatch {
  final OrignaBase _client;
  final List<_BatchOperation> _operations = [];

  WriteBatch(this._client);

  /// Add a create operation to the batch.
  void create(String collection, Map<String, dynamic> data) {
    _operations.add(_BatchOperation(
      type: _BatchOpType.create,
      collection: collection,
      data: _processFieldValues(data),
    ));
  }

  /// Add an update operation to the batch.
  void update(String collection, String id, Map<String, dynamic> data) {
    _operations.add(_BatchOperation(
      type: _BatchOpType.update,
      collection: collection,
      id: id,
      data: _processFieldValues(data),
    ));
  }

  /// Add a delete operation to the batch.
  void delete(String collection, String id) {
    _operations.add(_BatchOperation(
      type: _BatchOpType.delete,
      collection: collection,
      id: id,
    ));
  }

  /// How many operations are in the batch.
  int get length => _operations.length;

  /// Whether the batch is empty.
  bool get isEmpty => _operations.isEmpty;

  /// Commit all operations.
  ///
  /// **Important:** This is NOT truly atomic. Operations are grouped by type
  /// (creates, updates, deletes) and sent as separate batch API calls.
  /// If one batch call fails, previously completed batches are NOT rolled back.
  /// For true atomicity, use a server-side transaction endpoint if available.
  ///
  /// Returns the list of results for each operation.
  Future<List<Map<String, dynamic>>> commit() async {
    if (_operations.isEmpty) return [];

    // Group operations by type for batch API calls
    final creates = <_BatchOperation>[];
    final updates = <_BatchOperation>[];
    final deletes = <_BatchOperation>[];

    for (final op in _operations) {
      switch (op.type) {
        case _BatchOpType.create:
          creates.add(op);
        case _BatchOpType.update:
          updates.add(op);
        case _BatchOpType.delete:
          deletes.add(op);
      }
    }

    final results = <Map<String, dynamic>>[];

    // Execute batch creates (grouped by collection)
    final createsByCollection = <String, List<Map<String, dynamic>>>{};
    for (final op in creates) {
      createsByCollection.putIfAbsent(op.collection, () => []).add(op.data!);
    }
    for (final entry in createsByCollection.entries) {
      final response = await _client.graphql(
        'mutation { batchCreate(collection: "${entry.key}", docs: ${jsonEncode(entry.value)}) }',
      );
      final data = response['data']?['batchCreate'];
      if (data is List) {
        results.addAll(data.cast<Map<String, dynamic>>());
      }
    }

    // Execute batch updates (grouped by collection)
    final updatesByCollection =
        <String, List<Map<String, dynamic>>>{};
    for (final op in updates) {
      updatesByCollection.putIfAbsent(op.collection, () => []).add({
        'id': op.id,
        'data': op.data,
      });
    }
    for (final entry in updatesByCollection.entries) {
      final response = await _client.graphql(
        'mutation { batchUpdate(collection: "${entry.key}", updates: ${jsonEncode(entry.value)}) }',
      );
      final data = response['data']?['batchUpdate'];
      if (data is List) {
        results.addAll(data.cast<Map<String, dynamic>>());
      }
    }

    // Execute batch deletes (grouped by collection)
    final deletesByCollection = <String, List<String>>{};
    for (final op in deletes) {
      if (op.id != null) {
        deletesByCollection.putIfAbsent(op.collection, () => []).add(op.id!);
      }
    }
    for (final entry in deletesByCollection.entries) {
      await _client.graphql(
        'mutation { batchDelete(collection: "${entry.key}", ids: ${jsonEncode(entry.value)}) }',
      );
      for (final id in entry.value) {
        results.add({'id': id, 'deleted': true});
      }
    }

    _operations.clear();
    return results;
  }

  /// Process FieldValue sentinels into API-compatible format.
  Map<String, dynamic> _processFieldValues(Map<String, dynamic> data) {
    final result = <String, dynamic>{};
    for (final entry in data.entries) {
      final value = entry.value;
      if (value is FieldValue) {
        result.addAll(value.toApiMap(entry.key));
      } else {
        result[entry.key] = value;
      }
    }
    return result;
  }
}

enum _BatchOpType { create, update, delete }

class _BatchOperation {
  final _BatchOpType type;
  final String collection;
  final String? id;
  final Map<String, dynamic>? data;

  _BatchOperation({
    required this.type,
    required this.collection,
    this.id,
    this.data,
  });
}
