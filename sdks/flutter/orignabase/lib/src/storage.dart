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
    // For direct upload (without pre-signed URL flow), POST to storage endpoint
    final uri = Uri.parse('${_client.url}/storage/upload/$path');
    final headers = <String, String>{
      'Content-Type': contentType,
    };
    if (_client.auth.accessToken != null) {
      headers['Authorization'] = 'Bearer ${_client.auth.accessToken}';
    }

    final response = await http.put(uri, headers: headers, body: data);

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

    final response = await http.get(uri, headers: headers);

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
}
