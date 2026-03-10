import 'client.dart';

/// Remote Config — Firebase Remote Config replacement.
///
/// ```dart
/// final config = ob.config;
/// final value = await config.getString('feature_flag');
/// final all = await config.getAll();
/// ```
class OrignaBaseConfig {
  final OrignaBase _client;

  OrignaBaseConfig(this._client);

  /// Get all remote config key-value pairs.
  Future<Map<String, dynamic>> getAll() async {
    return await _client.request('GET', '/config');
  }

  /// Get a single config value by key.
  Future<dynamic> get(String key) async {
    final response = await _client.request('GET', '/config/$key');
    return response['value'];
  }

  /// Get a string config value (returns empty string if not found).
  Future<String> getString(String key) async {
    final value = await get(key);
    return value?.toString() ?? '';
  }

  /// Get a bool config value (returns false if not found).
  Future<bool> getBool(String key) async {
    final value = await get(key);
    if (value is bool) return value;
    if (value is String) return value.toLowerCase() == 'true';
    return false;
  }

  /// Get an int config value (returns 0 if not found).
  Future<int> getInt(String key) async {
    final value = await get(key);
    if (value is int) return value;
    if (value is num) return value.toInt();
    if (value is String) return int.tryParse(value) ?? 0;
    return 0;
  }

  /// Get a double config value (returns 0.0 if not found).
  Future<double> getDouble(String key) async {
    final value = await get(key);
    if (value is double) return value;
    if (value is num) return value.toDouble();
    if (value is String) return double.tryParse(value) ?? 0.0;
    return 0.0;
  }

  /// Set a config value (admin only).
  Future<void> set(String key, dynamic value) async {
    await _client.request('PUT', '/_admin/config/$key', body: {
      'value': value,
    });
  }

  /// Delete a config value (admin only).
  Future<void> delete(String key) async {
    await _client.request('DELETE', '/_admin/config/$key');
  }
}
