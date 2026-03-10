/// Web stub for [FileStorage] platform operations.
///
/// FileStorage is not supported on web. Use [InMemoryStorage] or a
/// web-compatible storage backend instead.

void assertPlatformSupported() {
  throw UnsupportedError(
    'FileStorage is not supported on web. '
    'Use InMemoryStorage or a web-compatible OfflineStorage implementation.',
  );
}

Future<bool> fileExists(String path) async {
  throw UnsupportedError('FileStorage is not supported on web.');
}

Future<String> readFile(String path) async {
  throw UnsupportedError('FileStorage is not supported on web.');
}

Future<void> writeFile(String path, String content) async {
  throw UnsupportedError('FileStorage is not supported on web.');
}

Future<void> ensureDirectory(String path) async {
  throw UnsupportedError('FileStorage is not supported on web.');
}
