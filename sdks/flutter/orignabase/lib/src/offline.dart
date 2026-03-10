import 'dart:async';
import 'dart:collection';
import 'dart:convert';

import 'client.dart';
import 'document.dart';
import 'errors.dart';

/// Pending write operation for offline queue.
class PendingWrite {
  final String id;
  final String collection;
  final String operation; // 'create', 'update', 'delete'
  final Map<String, dynamic>? data;
  final String? documentId;
  final DateTime createdAt;
  int retries;

  PendingWrite({
    required this.id,
    required this.collection,
    required this.operation,
    this.data,
    this.documentId,
    DateTime? createdAt,
    this.retries = 0,
  }) : createdAt = createdAt ?? DateTime.now();

  Map<String, dynamic> toJson() => {
        'id': id,
        'collection': collection,
        'operation': operation,
        'data': data,
        'documentId': documentId,
        'createdAt': createdAt.toIso8601String(),
        'retries': retries,
      };

  factory PendingWrite.fromJson(Map<String, dynamic> json) => PendingWrite(
        id: json['id'] as String,
        collection: json['collection'] as String,
        operation: json['operation'] as String,
        data: json['data'] as Map<String, dynamic>?,
        documentId: json['documentId'] as String?,
        createdAt: DateTime.parse(json['createdAt'] as String),
        retries: json['retries'] as int? ?? 0,
      );
}

/// In-memory offline cache and write queue.
///
/// Provides cache-first reads with background network refresh,
/// and automatic replay of queued mutations on reconnect.
///
/// For persistent storage, subclass [OfflineStorage] with a
/// platform-specific implementation (e.g., Hive, shared_preferences, SQLite).
class OfflineCache {
  final OfflineStorage _storage;
  final Queue<PendingWrite> _writeQueue = Queue<PendingWrite>();
  final _writeQueueController = StreamController<int>.broadcast();
  bool _isOnline = true;
  bool _isReplaying = false;
  int _idCounter = 0;
  OrignaBase? _client;

  OfflineCache({OfflineStorage? storage})
      : _storage = storage ?? InMemoryStorage();

  /// Bind to an OrignaBase client so that [replayQueue] can re-send operations.
  void bindClient(OrignaBase client) {
    _client = client;
  }

  /// Stream of pending write count changes.
  Stream<int> get pendingWriteCount => _writeQueueController.stream;

  /// Current number of pending writes.
  int get pendingCount => _writeQueue.length;

  /// Whether the client believes it's online.
  bool get isOnline => _isOnline;

  /// Set online/offline status. When going online, automatically replays
  /// any queued writes.
  set isOnline(bool value) {
    final wasOffline = !_isOnline;
    _isOnline = value;
    if (value && wasOffline && _writeQueue.isNotEmpty) {
      replayQueue();
    }
  }

  /// Replay all pending writes in order, removing each on success.
  ///
  /// Failed writes are left in the queue with incremented retry count.
  /// Requires [bindClient] to have been called first.
  Future<void> replayQueue() async {
    if (_client == null) return;
    if (_isReplaying) return; // prevent concurrent replays
    _isReplaying = true;

    try {
      final pending = List<PendingWrite>.from(_writeQueue);
      for (final write in pending) {
        try {
          switch (write.operation) {
            case 'create':
              if (write.data != null) {
                await _client!.collection(write.collection).add(write.data!);
              }
            case 'update':
              if (write.documentId != null && write.data != null) {
                await _client!
                    .collection(write.collection)
                    .doc(write.documentId!)
                    .update(write.data!);
              }
            case 'delete':
              if (write.documentId != null) {
                await _client!
                    .collection(write.collection)
                    .doc(write.documentId!)
                    .delete();
              }
          }
          // Success — remove from queue
          removePendingWrite(write.id);
        } on NetworkException {
          // Still offline or network error — stop replaying
          write.retries++;
          break;
        } catch (_) {
          // Other error — skip this write, increment retry count
          write.retries++;
        }
      }
    } finally {
      _isReplaying = false;
    }
  }

  /// Cache a document locally.
  Future<void> cacheDocument(String collection, Document doc) async {
    final key = _docKey(collection, doc.id);
    await _storage.write(key, jsonEncode(doc.data));
  }

  /// Cache multiple documents from a query result.
  Future<void> cacheQueryResult(
    String collection,
    String queryKey,
    List<Document> docs,
  ) async {
    // Cache individual documents
    for (final doc in docs) {
      await cacheDocument(collection, doc);
    }
    // Cache the query result (list of IDs)
    final ids = docs.map((d) => d.id).toList();
    await _storage.write(
      _queryKey(collection, queryKey),
      jsonEncode(ids),
    );
  }

  /// Get a cached document.
  Future<Document?> getCachedDocument(String collection, String id) async {
    final key = _docKey(collection, id);
    final data = await _storage.read(key);
    if (data == null) return null;
    final map = jsonDecode(data) as Map<String, dynamic>;
    return Document(id: id, collection: collection, data: map);
  }

  /// Get cached query results.
  Future<List<Document>?> getCachedQueryResult(
    String collection,
    String queryKey,
  ) async {
    final idsJson = await _storage.read(_queryKey(collection, queryKey));
    if (idsJson == null) return null;

    final ids = (jsonDecode(idsJson) as List).cast<String>();
    final docs = <Document>[];
    for (final id in ids) {
      final doc = await getCachedDocument(collection, id);
      if (doc != null) docs.add(doc);
    }
    return docs;
  }

  /// Enqueue a write operation for later replay.
  void enqueueWrite({
    required String collection,
    required String operation,
    Map<String, dynamic>? data,
    String? documentId,
  }) {
    final write = PendingWrite(
      id: 'pw_${++_idCounter}_${DateTime.now().millisecondsSinceEpoch}',
      collection: collection,
      operation: operation,
      data: data,
      documentId: documentId,
    );
    _writeQueue.add(write);
    _writeQueueController.add(_writeQueue.length);
  }

  /// Get all pending writes (for replay).
  List<PendingWrite> get pendingWrites => _writeQueue.toList();

  /// Remove a pending write after successful replay.
  void removePendingWrite(String id) {
    _writeQueue.removeWhere((w) => w.id == id);
    _writeQueueController.add(_writeQueue.length);
  }

  /// Clear all cached data.
  Future<void> clearAll() async {
    await _storage.clear();
    _writeQueue.clear();
    _writeQueueController.add(0);
  }

  /// Remove cached documents for a collection.
  Future<void> invalidateCollection(String collection) async {
    await _storage.removeByPrefix('doc:$collection:');
    await _storage.removeByPrefix('query:$collection:');
  }

  String _docKey(String collection, String id) => 'doc:$collection:$id';
  String _queryKey(String collection, String queryKey) =>
      'query:$collection:$queryKey';

  void dispose() {
    _writeQueueController.close();
  }
}

/// Abstract storage interface for offline cache persistence.
abstract class OfflineStorage {
  Future<void> write(String key, String value);
  Future<String?> read(String key);
  Future<void> remove(String key);
  Future<void> removeByPrefix(String prefix);
  Future<void> clear();
}

/// Default in-memory storage (no persistence across app restarts).
class InMemoryStorage implements OfflineStorage {
  final Map<String, String> _store = {};

  @override
  Future<void> write(String key, String value) async {
    _store[key] = value;
  }

  @override
  Future<String?> read(String key) async {
    return _store[key];
  }

  @override
  Future<void> remove(String key) async {
    _store.remove(key);
  }

  @override
  Future<void> removeByPrefix(String prefix) async {
    _store.removeWhere((key, _) => key.startsWith(prefix));
  }

  @override
  Future<void> clear() async {
    _store.clear();
  }
}
