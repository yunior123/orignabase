import 'dart:convert';

import 'offline.dart';

// Conditionally import dart:io. On web, this import is a no-op stub.
import 'persistent_storage_io.dart'
    if (dart.library.html) 'persistent_storage_stub.dart' as platform;

/// File-based persistent storage that survives app restarts.
///
/// Stores each key as a JSON entry in a single file per collection prefix.
/// For production apps, consider using Hive or SQLite instead.
///
/// **Note:** This class uses `dart:io` and is only available on native platforms
/// (iOS, Android, macOS, Linux, Windows). On web, use [InMemoryStorage] or a
/// web-compatible storage implementation (e.g., backed by IndexedDB or localStorage).
class FileStorage implements OfflineStorage {
  final String _directory;
  Map<String, String>? _cache;

  FileStorage(this._directory) {
    platform.assertPlatformSupported();
  }

  String get _filePath => '$_directory/orignabase_cache.json';

  Future<Map<String, String>> _load() async {
    if (_cache != null) return _cache!;
    if (await platform.fileExists(_filePath)) {
      final content = await platform.readFile(_filePath);
      final map = jsonDecode(content) as Map<String, dynamic>;
      _cache = map.map((k, v) => MapEntry(k, v as String));
    } else {
      _cache = {};
    }
    return _cache!;
  }

  Future<void> _persist() async {
    await platform.ensureDirectory(_directory);
    await platform.writeFile(_filePath, jsonEncode(_cache));
  }

  @override
  Future<void> write(String key, String value) async {
    final store = await _load();
    store[key] = value;
    await _persist();
  }

  @override
  Future<String?> read(String key) async {
    final store = await _load();
    return store[key];
  }

  @override
  Future<void> remove(String key) async {
    final store = await _load();
    store.remove(key);
    await _persist();
  }

  @override
  Future<void> removeByPrefix(String prefix) async {
    final store = await _load();
    store.removeWhere((key, _) => key.startsWith(prefix));
    await _persist();
  }

  @override
  Future<void> clear() async {
    _cache = {};
    await _persist();
  }
}
