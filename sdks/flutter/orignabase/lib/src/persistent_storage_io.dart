import 'dart:io';

/// Native platform file operations for [FileStorage].

void assertPlatformSupported() {
  // dart:io is available — no-op on native platforms.
}

Future<bool> fileExists(String path) async {
  return File(path).exists();
}

Future<String> readFile(String path) async {
  return File(path).readAsString();
}

Future<void> writeFile(String path, String content) async {
  await File(path).writeAsString(content);
}

Future<void> ensureDirectory(String path) async {
  final dir = Directory(path);
  if (!await dir.exists()) {
    await dir.create(recursive: true);
  }
}
