import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import 'package:http/http.dart' as http;

import 'client.dart';
import 'errors.dart';

/// OrignaBase storage service for file upload/download.
///
/// ```dart
/// // Upload
/// await ob.storage.upload('users/avatar.jpg', fileBytes, contentType: 'image/jpeg');
///
/// // Download
/// final bytes = await ob.storage.download('users/avatar.jpg');
///
/// // Resumable upload (large files)
/// final task = ob.storage.uploadResumable('videos/big.mp4', largeBytes,
///     contentType: 'video/mp4', chunkSize: 1024 * 1024);
/// task.onProgress.listen((p) => print('${p.bytesTransferred}/${p.totalBytes}'));
/// final result = await task.future;
/// ```
class OrignaBaseStorage {
  final OrignaBase _client;

  OrignaBaseStorage(this._client);

  /// Upload a file using a signed URL.
  ///
  /// The server generates a signed URL, then the client uploads directly.
  Future<Map<String, dynamic>> upload(
    String path,
    Uint8List data, {
    String contentType = 'application/octet-stream',
  }) async {
    final uri = Uri.parse('${_client.url}/storage/upload/$path');
    final headers = <String, String>{
      'Content-Type': contentType,
    };
    if (_client.auth.accessToken != null) {
      headers['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }

    final response =
        await _client.httpClient.put(uri, headers: headers, body: data);

    if (response.statusCode == 200) {
      return {'path': path, 'size': data.length, 'content_type': contentType};
    }

    throw OrignaBaseException(
      'Upload failed: ${response.body}',
      statusCode: response.statusCode,
    );
  }

  /// Download a file.
  Future<Uint8List> download(String path) async {
    final uri = Uri.parse('${_client.url}/storage/download/$path');
    final headers = <String, String>{};
    if (_client.auth.accessToken != null) {
      headers['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }

    final response = await _client.httpClient.get(uri, headers: headers);

    if (response.statusCode == 200) {
      return response.bodyBytes;
    }

    throw NotFoundException(
      'File not found: $path',
      statusCode: response.statusCode,
    );
  }

  /// Delete a file.
  Future<void> delete(String path) async {
    await _client.request('DELETE', '/storage/delete/$path');
  }

  /// Start a resumable upload for large files.
  ///
  /// Files are uploaded in chunks with automatic resume support.
  /// Default chunk size is 256KB. Returns an [UploadTask] that can be
  /// monitored for progress and cancelled.
  UploadTask uploadResumable(
    String path,
    Uint8List data, {
    String contentType = 'application/octet-stream',
    int chunkSize = 256 * 1024,
  }) {
    return UploadTask._(
      client: _client,
      path: path,
      data: data,
      contentType: contentType,
      chunkSize: chunkSize,
    );
  }

  /// Resume a previously interrupted upload.
  ///
  /// Queries the server for current progress, then continues from
  /// where it left off.
  UploadTask resumeUpload(
    String sessionId,
    Uint8List data, {
    int chunkSize = 256 * 1024,
  }) {
    return UploadTask._resume(
      client: _client,
      sessionId: sessionId,
      data: data,
      chunkSize: chunkSize,
    );
  }
}

/// Progress information for a resumable upload.
class UploadProgress {
  final int bytesTransferred;
  final int totalBytes;
  final String sessionId;

  UploadProgress({
    required this.bytesTransferred,
    required this.totalBytes,
    required this.sessionId,
  });

  double get fraction =>
      totalBytes > 0 ? bytesTransferred / totalBytes : 0.0;

  bool get isComplete => bytesTransferred >= totalBytes;
}

/// A resumable upload task with progress tracking and cancellation.
class UploadTask {
  final OrignaBase _client;
  final Uint8List _data;
  final int _chunkSize;
  final String? _path;
  final String? _contentType;
  String? _sessionId;
  bool _cancelled = false;

  late final Future<Map<String, dynamic>> future;
  void Function(UploadProgress)? _onProgress;

  UploadTask._({
    required OrignaBase client,
    required String path,
    required Uint8List data,
    required String contentType,
    required int chunkSize,
  })  : _client = client,
        _path = path,
        _contentType = contentType,
        _data = data,
        _chunkSize = chunkSize {
    future = _start();
  }

  UploadTask._resume({
    required OrignaBase client,
    required String sessionId,
    required Uint8List data,
    required int chunkSize,
  })  : _client = client,
        _sessionId = sessionId,
        _path = null,
        _contentType = null,
        _data = data,
        _chunkSize = chunkSize {
    future = _resume();
  }

  /// Set a progress callback.
  set onProgress(void Function(UploadProgress) callback) {
    _onProgress = callback;
  }

  /// The session ID (available after init).
  String? get sessionId => _sessionId;

  /// Cancel the upload.
  Future<void> cancel() async {
    _cancelled = true;
    if (_sessionId != null) {
      final uri = Uri.parse(
          '${_client.url}/storage/upload/resumable/$_sessionId');
      final headers = _authHeaders();
      await _client.httpClient.delete(uri, headers: headers);
    }
  }

  Map<String, String> _authHeaders() {
    final headers = <String, String>{};
    if (_client.auth.accessToken != null) {
      headers['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }
    return headers;
  }

  Future<Map<String, dynamic>> _start() async {
    // 1. Init session
    final initUri =
        Uri.parse('${_client.url}/storage/upload/resumable');
    final initResponse = await _client.httpClient.post(
      initUri,
      headers: {
        ..._authHeaders(),
        'Content-Type': 'application/json',
      },
      body: jsonEncode({
        'path': _path,
        'content_type': _contentType,
        'total_size': _data.length,
      }),
    );

    if (initResponse.statusCode != 200) {
      throw OrignaBaseException(
        'Failed to init resumable upload: ${initResponse.body}',
        statusCode: initResponse.statusCode,
      );
    }

    final session = jsonDecode(initResponse.body) as Map<String, dynamic>;
    _sessionId = session['id'] as String;

    return _uploadChunks(0);
  }

  Future<Map<String, dynamic>> _resume() async {
    // Query server for current offset
    final statusUri = Uri.parse(
        '${_client.url}/storage/upload/resumable/$_sessionId');
    final statusResponse =
        await _client.httpClient.get(statusUri, headers: _authHeaders());

    if (statusResponse.statusCode != 200) {
      throw OrignaBaseException(
        'Failed to query upload status: ${statusResponse.body}',
        statusCode: statusResponse.statusCode,
      );
    }

    final session =
        jsonDecode(statusResponse.body) as Map<String, dynamic>;
    final offset = (session['bytes_received'] as num).toInt();

    return _uploadChunks(offset);
  }

  Future<Map<String, dynamic>> _uploadChunks(int startOffset) async {
    int offset = startOffset;
    final total = _data.length;

    while (offset < total && !_cancelled) {
      final end = min(offset + _chunkSize, total);
      final chunk = _data.sublist(offset, end);

      final chunkUri = Uri.parse(
          '${_client.url}/storage/upload/resumable/$_sessionId');
      final response = await _client.httpClient.patch(
        chunkUri,
        headers: {
          ..._authHeaders(),
          'Content-Type': 'application/octet-stream',
          'Upload-Offset': offset.toString(),
        },
        body: chunk,
      );

      if (response.statusCode != 200) {
        throw OrignaBaseException(
          'Chunk upload failed: ${response.body}',
          statusCode: response.statusCode,
        );
      }

      offset = end;

      _onProgress?.call(UploadProgress(
        bytesTransferred: offset,
        totalBytes: total,
        sessionId: _sessionId!,
      ));
    }

    if (_cancelled) {
      throw OrignaBaseException('Upload cancelled', statusCode: 0);
    }

    return {
      'path': _path ?? '',
      'size': total,
      'session_id': _sessionId,
    };
  }
}
