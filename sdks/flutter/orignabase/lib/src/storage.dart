import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

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

  /// Upload a file using a presigned URL.
  ///
  /// 1. POST to `/storage/presign/upload` to get a signed upload URL.
  /// 2. PUT the file data to the signed URL.
  Future<Map<String, dynamic>> upload(
    String path,
    Uint8List data, {
    String contentType = 'application/octet-stream',
  }) async {
    // Step 1: Get presigned upload URL
    final presignUri = Uri.parse('${_client.url}/storage/presign/upload');
    final presignHeaders = <String, String>{
      'Content-Type': 'application/json',
    };
    if (_client.auth.accessToken != null) {
      presignHeaders['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }

    final presignResponse = await _client.httpClient.post(
      presignUri,
      headers: presignHeaders,
      body: jsonEncode({
        'paths': [path],
        'ttl_secs': 3600,
      }),
    );

    if (presignResponse.statusCode != 200) {
      throw OrignaBaseException(
        'Failed to get presigned URL: ${presignResponse.body}',
        statusCode: presignResponse.statusCode,
      );
    }

    final presignBody =
        jsonDecode(presignResponse.body) as Map<String, dynamic>;
    final urls = presignBody['urls'] as List<dynamic>;
    if (urls.isEmpty) {
      throw OrignaBaseException('No presigned URL returned');
    }
    var uploadUrl = (urls.first as Map<String, dynamic>)['upload_url'] as String;

    // Rewrite the upload URL to use the client's base URL if the server returned
    // a local address (e.g., http://0.0.0.0:8080) that isn't reachable from the client.
    final parsedUpload = Uri.parse(uploadUrl);
    final parsedClient = Uri.parse(_client.url);
    if (parsedUpload.host != parsedClient.host) {
      uploadUrl = parsedClient.replace(
        path: parsedUpload.path,
        query: parsedUpload.query,
      ).toString();
    }

    // Step 2: PUT file data to the signed URL
    final uploadHeaders = <String, String>{
      'Content-Type': contentType,
    };

    final uploadResponse = await _client.httpClient.put(
      Uri.parse(uploadUrl),
      headers: uploadHeaders,
      body: data,
    );

    if (uploadResponse.statusCode >= 200 && uploadResponse.statusCode < 300) {
      return {'path': path, 'size': data.length, 'content_type': contentType};
    }

    throw OrignaBaseException(
      'Upload failed: ${uploadResponse.body}',
      statusCode: uploadResponse.statusCode,
    );
  }

  /// Download a file using a presigned URL.
  Future<Uint8List> download(String path) async {
    // Step 1: Get presigned download URL
    final presignUri = Uri.parse('${_client.url}/storage/presign/download');
    final presignHeaders = <String, String>{
      'Content-Type': 'application/json',
    };
    if (_client.auth.accessToken != null) {
      presignHeaders['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }

    final presignResponse = await _client.httpClient.post(
      presignUri,
      headers: presignHeaders,
      body: jsonEncode({
        'paths': [path],
        'ttl_secs': 3600,
      }),
    );

    if (presignResponse.statusCode != 200) {
      throw NotFoundException(
        'File not found: $path',
        statusCode: presignResponse.statusCode,
      );
    }

    final presignBody =
        jsonDecode(presignResponse.body) as Map<String, dynamic>;
    final urls = presignBody['urls'] as List<dynamic>;
    if (urls.isEmpty) {
      throw NotFoundException('No presigned download URL returned');
    }
    var downloadUrl =
        (urls.first as Map<String, dynamic>)['download_url'] as String;

    // Rewrite URL host if server returned a local address
    final parsedDownload = Uri.parse(downloadUrl);
    final parsedClient = Uri.parse(_client.url);
    if (parsedDownload.host != parsedClient.host) {
      downloadUrl = parsedClient.replace(
        path: parsedDownload.path,
        query: parsedDownload.query,
      ).toString();
    }

    // Step 2: GET file data from signed URL
    final response = await _client.httpClient.get(Uri.parse(downloadUrl));

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
    await _client.request('POST', '/storage/batch-delete', body: {
      'paths': [path],
    });
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
