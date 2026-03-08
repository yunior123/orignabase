import 'dart:convert';
import 'package:http/http.dart' as http;

import 'auth.dart';
import 'collection.dart';
import 'errors.dart';
import 'storage.dart';

/// Main OrignaBase client. Entry point for all SDK operations.
///
/// ```dart
/// final ob = OrignaBase.initialize(url: 'http://localhost:8080');
/// await ob.auth.signInWithEmail('user@example.com', 'password');
/// final products = ob.collection('products');
/// ```
class OrignaBase {
  final String url;
  final http.Client _httpClient;
  late final OrignaBaseAuth auth;
  late final OrignaBaseStorage storage;

  OrignaBase._({
    required this.url,
    http.Client? httpClient,
  }) : _httpClient = httpClient ?? http.Client() {
    auth = OrignaBaseAuth(this);
    storage = OrignaBaseStorage(this);
  }

  /// Initialize the OrignaBase client.
  static OrignaBase initialize({
    required String url,
    http.Client? httpClient,
  }) {
    final trimmedUrl = url.endsWith('/') ? url.substring(0, url.length - 1) : url;
    return OrignaBase._(url: trimmedUrl, httpClient: httpClient);
  }

  /// Get a collection reference for Firestore-like queries.
  CollectionRef collection(String name) => CollectionRef(this, name);

  /// Execute a GraphQL query.
  Future<Map<String, dynamic>> graphql(
    String query, {
    Map<String, dynamic>? variables,
  }) async {
    final body = <String, dynamic>{'query': query};
    if (variables != null) body['variables'] = variables;

    final response = await request('POST', '/graphql', body: body);
    return response;
  }

  /// Execute a raw HTTP request against the OrignaBase server.
  Future<Map<String, dynamic>> request(
    String method,
    String path, {
    Map<String, dynamic>? body,
    Map<String, String>? headers,
  }) async {
    final uri = Uri.parse('$url$path');
    final requestHeaders = <String, String>{
      'Content-Type': 'application/json',
      ...?headers,
    };

    // Add auth token if available
    if (auth.accessToken != null) {
      requestHeaders['Authorization'] = 'Bearer ${auth.accessToken}';
    }

    final http.Response response;
    switch (method.toUpperCase()) {
      case 'GET':
        response = await _httpClient.get(uri, headers: requestHeaders);
      case 'POST':
        response = await _httpClient.post(
          uri,
          headers: requestHeaders,
          body: body != null ? jsonEncode(body) : null,
        );
      case 'PUT':
        response = await _httpClient.put(
          uri,
          headers: requestHeaders,
          body: body != null ? jsonEncode(body) : null,
        );
      case 'DELETE':
        response = await _httpClient.delete(uri, headers: requestHeaders);
      default:
        throw OrignaBaseException('Unsupported HTTP method: $method');
    }

    if (response.statusCode >= 200 && response.statusCode < 300) {
      if (response.body.isEmpty) return {};
      return jsonDecode(response.body) as Map<String, dynamic>;
    }

    // Handle errors
    final errorBody = response.body.isNotEmpty
        ? jsonDecode(response.body) as Map<String, dynamic>
        : <String, dynamic>{'message': 'Unknown error'};
    final message = errorBody['message'] as String? ?? 'Request failed';

    switch (response.statusCode) {
      case 401:
        throw AuthException(message, statusCode: 401);
      case 403:
        throw ForbiddenException(message, statusCode: 403);
      case 404:
        throw NotFoundException(message, statusCode: 404);
      case 422:
        throw ValidationException(message, statusCode: 422);
      default:
        throw OrignaBaseException(message, statusCode: response.statusCode);
    }
  }

  /// Dispose the HTTP client.
  void dispose() {
    _httpClient.close();
  }
}
