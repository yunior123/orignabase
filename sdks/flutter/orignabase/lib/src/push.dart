import 'client.dart';

abstract final class PushApiRoutes {
  static const register = '/push/register';
  static const send = '/push/send';
  static const subscribe = '/push/subscribe';

  const PushApiRoutes._();
}

abstract final class PushApiKeys {
  static const userId = 'user_id';
  static const token = 'token';
  static const platform = 'platform';
  static const topic = 'topic';
  static const to = 'to';
  static const targetType = 'target_type';
  static const title = 'title';
  static const body = 'body';
  static const data = 'data';
  static const sent = 'sent';
  static const failed = 'failed';
  static const totalDevices = 'total_devices';

  const PushApiKeys._();
}

abstract final class PushTargetTypes {
  static const user = 'user';
  static const token = 'token';
  static const topic = 'topic';

  const PushTargetTypes._();
}

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
      sent: (map[PushApiKeys.sent] as num?)?.toInt() ?? 0,
      failed: (map[PushApiKeys.failed] as num?)?.toInt() ?? 0,
      totalDevices: (map[PushApiKeys.totalDevices] as num?)?.toInt() ?? 0,
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
    await _client.request('POST', PushApiRoutes.register, body: {
      PushApiKeys.userId: userId,
      PushApiKeys.token: token,
      PushApiKeys.platform: platform,
    });
  }

  /// Unregister a device token.
  Future<void> unregisterToken(String token) async {
    await _client.request('DELETE', PushApiRoutes.register, body: {
      PushApiKeys.token: token,
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
      targetType: PushTargetTypes.user,
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
      targetType: PushTargetTypes.token,
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
      targetType: PushTargetTypes.topic,
      title: title,
      body: body,
      data: data,
    );
  }

  /// Subscribe a device to a topic.
  Future<void> subscribeToTopic(String token, String topic) async {
    await _client.request('POST', PushApiRoutes.subscribe, body: {
      PushApiKeys.token: token,
      PushApiKeys.topic: topic,
    });
  }

  /// Unsubscribe a device from a topic.
  Future<void> unsubscribeFromTopic(String token, String topic) async {
    await _client.request('DELETE', PushApiRoutes.subscribe, body: {
      PushApiKeys.token: token,
      PushApiKeys.topic: topic,
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
      PushApiKeys.to: to,
      PushApiKeys.targetType: targetType,
      PushApiKeys.title: title,
      PushApiKeys.body: body,
    };
    if (data != null) requestBody[PushApiKeys.data] = data;

    final response =
        await _client.request('POST', PushApiRoutes.send, body: requestBody);
    return PushResult.fromMap(response);
  }
}
