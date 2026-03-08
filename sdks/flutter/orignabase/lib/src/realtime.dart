import 'dart:async';
import 'dart:convert';
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

/// Manages a WebSocket connection for realtime subscriptions.
class RealtimeClient {
  final OrignaBase _client;
  WebSocketChannel? _channel;
  final _subscriptions = <String, StreamController<DocumentChange>>{};
  StreamSubscription<dynamic>? _listener;

  RealtimeClient(this._client);

  /// Connect to the realtime WebSocket endpoint.
  void connect() {
    final wsUrl = _client.url
        .replaceFirst('http://', 'ws://')
        .replaceFirst('https://', 'wss://');

    _channel = WebSocketChannel.connect(Uri.parse('$wsUrl/realtime'));
    _listener = _channel!.stream.listen(
      _handleMessage,
      onDone: _handleDisconnect,
      onError: (_) => _handleDisconnect(),
    );
  }

  /// Subscribe to changes on a collection.
  Stream<DocumentChange> subscribe(String collection, {String? filter}) {
    final subId = '${collection}_${DateTime.now().millisecondsSinceEpoch}';
    final controller = StreamController<DocumentChange>.broadcast(
      onCancel: () => _unsubscribe(subId),
    );
    _subscriptions[subId] = controller;

    // Send subscribe message
    _channel?.sink.add(jsonEncode({
      'type': 'subscribe',
      'subscription_id': subId,
      'collection': collection,
      if (filter != null) 'filter': filter,
    }));

    return controller.stream;
  }

  void _unsubscribe(String subId) {
    _channel?.sink.add(jsonEncode({
      'type': 'unsubscribe',
      'subscription_id': subId,
    }));
    _subscriptions.remove(subId)?.close();
  }

  void _handleMessage(dynamic rawMessage) {
    if (rawMessage is! String) return;
    final data = jsonDecode(rawMessage) as Map<String, dynamic>;
    final subId = data['subscription_id'] as String?;
    final event = data['event'] as Map<String, dynamic>?;

    if (subId == null || event == null) return;
    final controller = _subscriptions[subId];
    if (controller == null) return;

    final changeType = switch (event['action'] as String?) {
      'create' => ChangeType.create,
      'update' => ChangeType.update,
      'delete' => ChangeType.delete,
      _ => null,
    };

    if (changeType == null) return;

    final collection = event['collection'] as String? ?? '';
    final docData = event['data'] as Map<String, dynamic>? ?? {};

    controller.add(DocumentChange(
      type: changeType,
      document: Document.fromMap(collection, {
        'id': event['document_id'],
        ...docData,
      }),
    ));
  }

  void _handleDisconnect() {
    for (final controller in _subscriptions.values) {
      controller.close();
    }
    _subscriptions.clear();
  }

  /// Disconnect and clean up.
  void disconnect() {
    _listener?.cancel();
    _channel?.sink.close();
    _handleDisconnect();
  }
}
