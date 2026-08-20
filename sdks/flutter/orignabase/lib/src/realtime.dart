import 'dart:async';
import 'dart:convert';
import 'package:meta/meta.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import 'client.dart';
import 'document.dart';

/// Change type for realtime events.
enum ChangeType { create, update, delete }

/// A document change event from the realtime subscription.
class DocumentChange {
  final ChangeType type;
  final Document document;

  DocumentChange({required this.type, required this.document});
}

/// Internal subscription registration.
class _SubEntry {
  final String id;
  final String collection;
  final String? documentId;
  final String? filter;
  final StreamController<DocumentChange> controller;

  _SubEntry({
    required this.id,
    required this.collection,
    this.documentId,
    this.filter,
    required this.controller,
  });
}

/// Manages a WebSocket connection for realtime subscriptions.
///
/// Reconnects automatically on disconnect with exponential back-off:
/// 1s → 2s → 4s → 8s → 16s → 30s (cap).  Active subscriptions are
/// re-registered transparently so callers never need to re-subscribe.
class RealtimeClient {
  final OrignaBase _client;
  WebSocketChannel? _channel;
  StreamSubscription<dynamic>? _listener;

  /// All active subscriptions keyed by subscription ID.
  final _subs = <String, _SubEntry>{};

  /// Monotonic counter to guarantee unique subscription IDs even within
  /// the same millisecond.
  int _subCounter = 0;

  bool _disconnecting = false;
  Timer? _reconnectTimer;
  int _reconnectAttempts = 0;
  static const _maxBackoffSeconds = 30;

  RealtimeClient(this._client);

  /// Create a RealtimeClient with a pre-connected channel (for testing).
  @visibleForTesting
  RealtimeClient.withChannel(this._client, WebSocketChannel channel)
      : _channel = channel {
    _listener = _channel!.stream.listen(
      _handleMessage,
      onDone: _scheduleReconnect,
      onError: (_) => _scheduleReconnect(),
    );
  }

  // ---------------------------------------------------------------------------
  // Connection management
  // ---------------------------------------------------------------------------

  /// Connect to the realtime WebSocket endpoint.
  void connect() {
    _disconnecting = false;
    _doConnect();
  }

  void _doConnect() {
    _reconnectTimer?.cancel();
    _reconnectTimer = null;

    // P1-23: Cancel the previous listener before creating a new one to prevent
    // leaked StreamSubscriptions accumulating on every reconnect.
    _listener?.cancel();
    _listener = null;

    final baseUri = Uri.parse(_client.url);
    final wsScheme = baseUri.scheme == 'https' ? 'wss' : 'ws';
    // Explicit port to avoid Dart URI defaulting wss to port 0.
    final port =
        baseUri.hasPort ? baseUri.port : (baseUri.scheme == 'https' ? 443 : 80);
    // Use query param auth (server expects ?token=<jwt>, not Authorization header)
    final wsUri = Uri(
      scheme: wsScheme,
      host: baseUri.host,
      port: port,
      path: '${baseUri.path}/realtime',
      queryParameters: _client.auth.accessToken != null
          ? {'token': _client.auth.accessToken!}
          : null,
    );

    try {
      _channel = WebSocketChannel.connect(wsUri);
      _listener = _channel!.stream.listen(
        _handleMessage,
        onDone: _scheduleReconnect,
        onError: (_) => _scheduleReconnect(),
      );

      // Catch the WebSocket ready future so DNS/connection errors don't leak
      // into the current zone as unhandled async exceptions.
      _channel!.ready.catchError((_) => _scheduleReconnect());

      // Re-register all active subscriptions after (re)connect.
      for (final sub in _subs.values) {
        _sendSubscribe(sub);
      }

      _reconnectAttempts = 0;
    } catch (_) {
      // Connection setup failed synchronously — schedule retry.
      _scheduleReconnect();
    }
  }

  void _scheduleReconnect() {
    if (_disconnecting) return;

    final backoff = _backoffDuration();
    _reconnectAttempts++;

    _reconnectTimer?.cancel();
    _reconnectTimer = Timer(backoff, _doConnect);
  }

  Duration _backoffDuration() {
    final seconds = (1 << _reconnectAttempts).clamp(1, _maxBackoffSeconds);
    return Duration(seconds: seconds);
  }

  /// Disconnect and clean up all resources permanently.
  void disconnect() {
    _disconnecting = true;
    _reconnectTimer?.cancel();
    _listener?.cancel();
    _channel?.sink.close();
    for (final sub in _subs.values) {
      sub.controller.close();
    }
    _subs.clear();
  }

  // ---------------------------------------------------------------------------
  // Subscription API
  // ---------------------------------------------------------------------------

  /// Subscribe to changes on a specific document.
  ///
  /// ```dart
  /// final stream = realtime.subscribeDocument('users', 'user123');
  /// stream.listen((change) => print('User updated: ${change.document.data}'));
  /// ```
  Stream<DocumentChange> subscribeDocument(
      String collection, String documentId) {
    final subId =
        '${collection}_${documentId}_${DateTime.now().millisecondsSinceEpoch}_${_subCounter++}';
    final entry = _SubEntry(
      id: subId,
      collection: collection,
      documentId: documentId,
      controller: StreamController<DocumentChange>.broadcast(
        onCancel: () => _unsubscribe(subId),
      ),
    );
    _subs[subId] = entry;
    _sendSubscribe(entry);
    return entry.controller.stream;
  }

  /// Subscribe to changes on a collection.
  Stream<DocumentChange> subscribe(String collection, {String? filter}) {
    final subId =
        '${collection}_${DateTime.now().millisecondsSinceEpoch}_${_subCounter++}';
    final entry = _SubEntry(
      id: subId,
      collection: collection,
      filter: filter,
      controller: StreamController<DocumentChange>.broadcast(
        onCancel: () => _unsubscribe(subId),
      ),
    );
    _subs[subId] = entry;
    _sendSubscribe(entry);
    return entry.controller.stream;
  }

  void _sendSubscribe(_SubEntry sub) {
    final msg = <String, dynamic>{
      'type': 'subscribe',
      'id': sub.id,
      'collection': sub.collection,
      if (sub.documentId != null) 'document_id': sub.documentId,
      if (sub.filter != null) 'filter': sub.filter,
    };
    try {
      _channel?.sink.add(jsonEncode(msg));
    } catch (_) {
      // Channel not yet open — will be resent after reconnect.
    }
  }

  void _unsubscribe(String subId) {
    final sub = _subs.remove(subId);
    if (sub == null) return;
    try {
      _channel?.sink.add(jsonEncode({'type': 'unsubscribe', 'id': subId}));
    } catch (_) {
      // Channel may already be closed.
    }
    sub.controller.close();
  }

  // ---------------------------------------------------------------------------
  // Message handling
  // ---------------------------------------------------------------------------

  void _handleMessage(dynamic rawMessage) {
    if (rawMessage is! String) return;
    final data = jsonDecode(rawMessage) as Map<String, dynamic>;
    final subId = data['subscription_id'] as String?;
    final event = data['event'] as Map<String, dynamic>?;

    if (subId == null || event == null) return;
    final sub = _subs[subId];
    if (sub == null) return;

    final changeType = switch (event['action'] as String?) {
      'create' => ChangeType.create,
      'update' => ChangeType.update,
      'delete' => ChangeType.delete,
      _ => null,
    };

    if (changeType == null) return;

    final collection = event['collection'] as String? ?? '';
    final docData = event['data'] as Map<String, dynamic>? ?? {};

    sub.controller.add(DocumentChange(
      type: changeType,
      document: Document.fromMap(collection, {
        'id': event['document_id'],
        ...docData,
      }),
    ));
  }
}
