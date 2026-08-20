import 'dart:async';
import 'dart:convert';
import 'package:http/http.dart' as http;

import 'auth.dart';
import 'batch.dart';
import 'collection.dart';
import 'config.dart';
import 'errors.dart';
import 'links.dart';
import 'metrics.dart';
import 'offline.dart';
import 'presence.dart';
import 'push.dart';
import 'realtime.dart';
import 'storage.dart';
import 'vector.dart';

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

  /// Internal HTTP client — used by storage for direct file operations.
  http.Client get httpClient => _httpClient;
  late final OrignaBaseAuth auth;
  late final OrignaBaseStorage storage;
  late final OfflineCache offline;
  late final OrignaBaseConfig config;
  late final OrignaBasePresence presence;
  late final OrignaBaseLinks links;
  late final OrignaBasePush push;
  late final OrignaBaseMetrics metrics;
  late final VectorSearch vectorSearch;
  RealtimeClient? _realtime;

  /// Shared realtime client, lazily initialized on first use.
  /// All snapshots() calls should use this instead of creating new connections.
  RealtimeClient get realtime {
    if (_realtime == null) {
      _realtime = RealtimeClient(this);
      _realtime!.connect();
    }
    return _realtime!;
  }

  OrignaBase._({
    required this.url,
    http.Client? httpClient,
    OfflineStorage? offlineStorage,
  }) : _httpClient = httpClient ?? http.Client() {
    auth = OrignaBaseAuth(this);
    storage = OrignaBaseStorage(this);
    offline = OfflineCache(storage: offlineStorage)..bindClient(this);
    config = OrignaBaseConfig(this);
    presence = OrignaBasePresence(this);
    links = OrignaBaseLinks(this);
    push = OrignaBasePush(this);
    metrics = OrignaBaseMetrics(this);
    vectorSearch = VectorSearch(this);
  }

  /// Initialize the OrignaBase client.
  ///
  /// Optionally pass an [OfflineStorage] implementation for persistent
  /// offline caching (e.g., Hive-based storage). Defaults to in-memory.
  static OrignaBase initialize({
    required String url,
    http.Client? httpClient,
    OfflineStorage? offlineStorage,
  }) {
    final trimmedUrl =
        url.endsWith('/') ? url.substring(0, url.length - 1) : url;
    return OrignaBase._(
        url: trimmedUrl,
        httpClient: httpClient,
        offlineStorage: offlineStorage);
  }

  /// Get a collection reference for Firestore-like queries.
  CollectionRef collection(String name) => CollectionRef(this, name);

  /// Create a write batch for atomic multi-document operations.
  ///
  /// ```dart
  /// final batch = ob.batch();
  /// batch.create('products', {'title': 'Widget'});
  /// batch.update('products', 'abc', {'price': 39.99});
  /// batch.delete('products', 'old-id');
  /// await batch.commit();
  /// ```
  WriteBatch batch() => WriteBatch(this);

  /// Execute a GraphQL query.
  ///
  /// Throws [OrignaBaseException] if the GraphQL response contains errors.
  Future<Map<String, dynamic>> graphql(
    String query, {
    Map<String, dynamic>? variables,
  }) async {
    final body = <String, dynamic>{'query': query};
    if (variables != null) body['variables'] = variables;

    final response = await request('POST', '/graphql', body: body);

    // Check for GraphQL-level errors (returned as 200 with errors array)
    if (response.containsKey('errors') && response['errors'] is List) {
      final errors = response['errors'] as List;
      if (errors.isNotEmpty) {
        final message =
            (errors.first as Map<String, dynamic>)['message'] as String? ??
                'GraphQL error';
        if (message.contains('Permission denied') ||
            message.contains('Unauthorized')) {
          throw ForbiddenException(message, statusCode: 403);
        }
        if (message.contains('Not found') || message.contains('not found')) {
          throw NotFoundException(message, statusCode: 404);
        }
        throw OrignaBaseException(message);
      }
    }

    return response;
  }

  /// Maximum number of retries on 429 (rate limit) responses.
  static const _maxRetries = 3;

  /// Initial backoff duration for 429 retries. Doubles each attempt, capped at 60s.
  static const _initialBackoff = Duration(seconds: 1);
  static const _maxBackoff = Duration(seconds: 60);

  /// Single-flight completer for token refresh.
  /// Prevents parallel refresh storms when multiple requests get 401 simultaneously.
  Completer<bool>? _refreshCompleter;

  /// Auth paths that should never trigger a 401 refresh-and-retry cycle.
  static const _authPaths = {'/auth/login', '/auth/register', '/auth/refresh'};

  /// Execute a raw HTTP request against the OrignaBase server.
  ///
  /// Automatically retries with exponential backoff on 429 (rate limit) responses.
  /// Starts at 1s, doubles each attempt (1s -> 2s -> 4s), max 3 retries, capped at 60s.
  /// Respects `Retry-After` header from the server when present.
  ///
  /// On 401 (expired token), attempts a single token refresh and retries the
  /// original request. Uses a single-flight pattern to prevent parallel refresh
  /// storms when multiple requests receive 401 simultaneously.
  Future<Map<String, dynamic>> request(
    String method,
    String path, {
    Map<String, dynamic>? body,
    Map<String, String>? headers,
  }) async {
    for (int attempt = 0; attempt <= _maxRetries; attempt++) {
      final response =
          await _executeHttp(method, path, body: body, headers: headers);

      if (response.statusCode >= 200 && response.statusCode < 300) {
        if (response.body.isEmpty) return {};
        try {
          return jsonDecode(response.body) as Map<String, dynamic>;
        } on FormatException {
          throw OrignaBaseException(
            'Invalid JSON in response body',
            statusCode: response.statusCode,
          );
        }
      }

      // On 401, attempt a single token refresh and retry (skip for auth paths).
      if (response.statusCode == 401 && !_authPaths.contains(path)) {
        final refreshed = await _refreshTokenOnce();
        if (refreshed) {
          // Retry the original request with the new token.
          final retryResponse =
              await _executeHttp(method, path, body: body, headers: headers);
          if (retryResponse.statusCode >= 200 &&
              retryResponse.statusCode < 300) {
            if (retryResponse.body.isEmpty) return {};
            try {
              return jsonDecode(retryResponse.body) as Map<String, dynamic>;
            } on FormatException {
              throw OrignaBaseException(
                'Invalid JSON in response body',
                statusCode: retryResponse.statusCode,
              );
            }
          }
          // Retry also failed — throw based on retry response.
          _throwForStatus(retryResponse);
        }
        // Refresh failed — throw the original 401 error.
        _throwForStatus(response);
      }

      // On 429, retry with exponential backoff (unless we've exhausted retries).
      if (response.statusCode == 429 && attempt < _maxRetries) {
        final retryAfter = response.headers['retry-after'];
        Duration backoff;
        if (retryAfter != null) {
          final seconds = int.tryParse(retryAfter);
          backoff = seconds != null
              ? Duration(seconds: seconds)
              : _calculateBackoff(attempt);
        } else {
          backoff = _calculateBackoff(attempt);
        }
        await Future<void>.delayed(backoff);
        continue;
      }

      // Non-retryable error or retries exhausted — throw.
      _throwForStatus(response);
    }

    // Should never reach here, but satisfy the type system.
    throw OrignaBaseException('Request failed after $_maxRetries retries');
  }

  /// Attempt to refresh the access token exactly once.
  /// Returns true if refresh succeeded, false otherwise.
  /// Uses a [Completer]-based single-flight pattern so concurrent callers
  /// all wait on the same refresh operation.
  Future<bool> _refreshTokenOnce() async {
    // If a refresh is already in progress, wait for it.
    if (_refreshCompleter != null) {
      return _refreshCompleter!.future;
    }

    _refreshCompleter = Completer<bool>();
    try {
      await auth.refreshToken();
      _refreshCompleter!.complete(true);
      return true;
    } catch (_) {
      _refreshCompleter!.complete(false);
      return false;
    } finally {
      _refreshCompleter = null;
    }
  }

  /// Calculate exponential backoff duration for the given attempt.
  Duration _calculateBackoff(int attempt) {
    final seconds = (_initialBackoff.inSeconds * (1 << attempt))
        .clamp(1, _maxBackoff.inSeconds);
    return Duration(seconds: seconds);
  }

  /// Execute a single HTTP request (no retry logic).
  Future<http.Response> _executeHttp(
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

    try {
      switch (method.toUpperCase()) {
        case 'GET':
          return await _httpClient
              .get(uri, headers: requestHeaders)
              .timeout(const Duration(seconds: 30));
        case 'POST':
          return await _httpClient
              .post(
                uri,
                headers: requestHeaders,
                body: body != null ? jsonEncode(body) : null,
              )
              .timeout(const Duration(seconds: 30));
        case 'PUT':
          return await _httpClient
              .put(
                uri,
                headers: requestHeaders,
                body: body != null ? jsonEncode(body) : null,
              )
              .timeout(const Duration(seconds: 30));
        case 'DELETE':
          // Use Request directly to support body in DELETE (needed by push, config)
          final deleteRequest = http.Request('DELETE', uri);
          deleteRequest.headers.addAll(requestHeaders);
          if (body != null) deleteRequest.body = jsonEncode(body);
          final streamedResponse = await _httpClient
              .send(deleteRequest)
              .timeout(const Duration(seconds: 30));
          return await http.Response.fromStream(streamedResponse);
        default:
          throw OrignaBaseException('Unsupported HTTP method: $method');
      }
    } on http.ClientException catch (e) {
      throw NetworkException('HTTP client error: ${e.message}');
    } on TimeoutException {
      throw NetworkException('Request timed out after 30 seconds');
    } on OrignaBaseException {
      rethrow;
    } catch (e) {
      // Catches SocketException (dart:io) and any other transport errors
      throw NetworkException('Network error: $e');
    }
  }

  /// Parse error body and throw the appropriate typed exception.
  Never _throwForStatus(http.Response response) {
    Map<String, dynamic> errorBody;
    if (response.body.isEmpty) {
      errorBody = <String, dynamic>{'message': 'Unknown error'};
    } else {
      try {
        errorBody = jsonDecode(response.body) as Map<String, dynamic>;
      } on FormatException {
        errorBody = <String, dynamic>{'message': response.body};
      }
    }
    final message = errorBody['message'] as String? ?? 'Request failed';

    switch (response.statusCode) {
      case 401:
        throw AuthException(message, statusCode: 401);
      case 403:
        throw ForbiddenException(message, statusCode: 403);
      case 404:
        throw NotFoundException(message, statusCode: 404);
      case 409:
        throw ConflictException(message, statusCode: 409);
      case 422:
        throw ValidationException(message, statusCode: 422);
      case 429:
        throw RateLimitException(message, statusCode: 429);
      default:
        throw OrignaBaseException(message, statusCode: response.statusCode);
    }
  }

  /// Full-text search via the OrignaBase search API.
  Future<Map<String, dynamic>> search(
    String index,
    String query, {
    int? limit,
    int? offset,
    String? filter,
  }) async {
    final args = <String>[
      'index: "$index"',
      'query: "$query"',
    ];
    if (limit != null) args.add('limit: $limit');
    if (offset != null) args.add('offset: $offset');
    if (filter != null) args.add('filter: "$filter"');

    final response = await graphql(
      'query { search(${args.join(', ')}) }',
    );
    return response['data']?['search'] as Map<String, dynamic>? ?? {};
  }

  /// Close the realtime WebSocket connection if it was opened.
  /// Called on sign-out to prevent stale subscriptions from lingering.
  /// A new connection will be lazily created on next use via [realtime].
  void closeRealtime() {
    _realtime?.disconnect();
    _realtime = null;
  }

  /// Dispose all resources: realtime connection, auth stream, offline cache, and HTTP client.
  void dispose() {
    closeRealtime();
    auth.dispose();
    offline.dispose();
    _httpClient.close();
  }
}
