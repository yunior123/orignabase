// Stub for dart:html on non-web platforms.
// This file is only imported when dart.library.html is NOT available.

class _StubStorage {
  String? operator [](String key) => null;
  void operator []=(String key, String value) {}
  void remove(String key) {}
}

class _StubWindow {
  final localStorage = _StubStorage();
}

final window = _StubWindow();
