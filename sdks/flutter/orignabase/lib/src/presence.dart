import 'client.dart';

/// Presence info for an online user.
class PresenceInfo {
  final String userId;
  final String connectionId;
  final String status;
  final String lastSeen;
  final Map<String, dynamic> metadata;

  PresenceInfo({
    required this.userId,
    required this.connectionId,
    required this.status,
    required this.lastSeen,
    this.metadata = const {},
  });

  factory PresenceInfo.fromMap(Map<String, dynamic> map) {
    return PresenceInfo(
      userId: map['user_id'] as String? ?? '',
      connectionId: map['connection_id'] as String? ?? '',
      status: map['status'] as String? ?? 'unknown',
      lastSeen: map['last_seen'] as String? ?? '',
      metadata: map['metadata'] as Map<String, dynamic>? ?? {},
    );
  }
}

/// Presence tracking — see who's online in realtime.
///
/// ```dart
/// final online = await ob.presence.getOnlineUsers();
/// final isOnline = await ob.presence.isOnline('user123');
/// ```
class OrignaBasePresence {
  final OrignaBase _client;

  OrignaBasePresence(this._client);

  /// Get all online users.
  Future<List<PresenceInfo>> getOnlineUsers() async {
    final response = await _client.request('GET', '/presence');
    final users = response['online'] as List<dynamic>? ?? [];
    return users
        .whereType<Map<String, dynamic>>()
        .map(PresenceInfo.fromMap)
        .toList();
  }

  /// Check if a specific user is online.
  Future<bool> isOnline(String userId) async {
    final response = await _client.request('GET', '/presence/$userId');
    return response['online'] == true;
  }

  /// Get presence info for a specific user.
  Future<PresenceInfo?> getUser(String userId) async {
    final response = await _client.request('GET', '/presence/$userId');
    if (response['online'] != true) return null;
    final info = response['presence'] as Map<String, dynamic>?;
    if (info == null) return null;
    return PresenceInfo.fromMap(info);
  }
}
