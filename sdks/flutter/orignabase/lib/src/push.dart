import 'client.dart';

/// Push notification result.
class PushResult {
  final int sent;
  final int failed;
  final int totalDevices;

  PushResult({
    required this.sent,
    required this.failed,
    required this.totalDevices,
  });

  factory PushResult.fromMap(Map<String, dynamic> map) {
    return PushResult(
      sent: (map['sent'] as num?)?.toInt() ?? 0,
      failed: (map['failed'] as num?)?.toInt() ?? 0,
      totalDevices: (map['total_devices'] as num?)?.toInt() ?? 0,
    );
  }
}

/// Push Notifications — Firebase Cloud Messaging replacement.
///
/// ```dart
/// await ob.push.registerToken(userId: 'u1', token: 'fcm_abc', platform: 'android');
/// await ob.push.sendToUser('u1', title: 'Hello', body: 'World');
/// ```
class OrignaBasePush {
  final OrignaBase _client;

  OrignaBasePush(this._client);

  /// Register a device token for push notifications.
  Future<void> registerToken({
    required String userId,
    required String token,
    required String platform,
  }) async {
    await _client.request('POST', '/push/register', body: {
      'user_id': userId,
      'token': token,
      'platform': platform,
    });
  }

  /// Unregister a device token.
  Future<void> unregisterToken(String token) async {
    await _client.request('DELETE', '/push/register', body: {
      'token': token,
    });
  }

  /// Send a push notification to a user (all their devices).
  Future<PushResult> sendToUser(
    String userId, {
    required String title,
    required String body,
    Map<String, dynamic>? data,
  }) async {
    return _send(
      to: userId,
      targetType: 'user',
      title: title,
      body: body,
      data: data,
    );
  }

  /// Send a push notification to a specific device token.
  Future<PushResult> sendToToken(
    String token, {
    required String title,
    required String body,
    Map<String, dynamic>? data,
  }) async {
    return _send(
      to: token,
      targetType: 'token',
      title: title,
      body: body,
      data: data,
    );
  }

  /// Send a push notification to all subscribers of a topic.
  Future<PushResult> sendToTopic(
    String topic, {
    required String title,
    required String body,
    Map<String, dynamic>? data,
  }) async {
    return _send(
      to: topic,
      targetType: 'topic',
      title: title,
      body: body,
      data: data,
    );
  }

  /// Subscribe a device to a topic.
  Future<void> subscribeToTopic(String token, String topic) async {
    await _client.request('POST', '/push/subscribe', body: {
      'token': token,
      'topic': topic,
    });
  }

  /// Unsubscribe a device from a topic.
  Future<void> unsubscribeFromTopic(String token, String topic) async {
    await _client.request('DELETE', '/push/subscribe', body: {
      'token': token,
      'topic': topic,
    });
  }

  Future<PushResult> _send({
    required String to,
    required String targetType,
    required String title,
    required String body,
    Map<String, dynamic>? data,
  }) async {
    final requestBody = <String, dynamic>{
      'to': to,
      'target_type': targetType,
      'title': title,
      'body': body,
    };
    if (data != null) requestBody['data'] = data;

    final response =
        await _client.request('POST', '/push/send', body: requestBody);
    return PushResult.fromMap(response);
  }
}
