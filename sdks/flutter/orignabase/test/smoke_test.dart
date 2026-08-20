/// Smoke tests for OrignaBase Flutter SDK.
///
/// Validates core SDK functionality using mock HTTP responses.
/// For live-server validation, use test/live_integration_test.dart.
@TestOn('vm')
library;

import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

const _testUrl = 'http://test.local';

/// Build a mock HTTP client that routes requests to a handler.
http.Client _mockHttp(
  http.Response Function(http.Request request) handler,
) {
  return MockClient((request) async => handler(request));
}

/// Standard auth response for register/login.
http.Response _authOk({String userId = 'users:smoke1'}) {
  return http.Response(
    jsonEncode({
      'access_token': 'eyJ.mock.token',
      'refresh_token': 'refresh_mock',
      'user_id': userId,
      'user': {'id': userId, 'email': 'smoke@test.com'},
    }),
    200,
    headers: {'content-type': 'application/json'},
  );
}

/// Standard error response.
http.Response _errorResp(int status, String message) {
  return http.Response(
    jsonEncode({'error': message}),
    status,
    headers: {'content-type': 'application/json'},
  );
}

void main() {
  group('Smoke Tests', () {
    test('client initializes without error', () {
      final ob = OrignaBase.initialize(url: _testUrl);
      expect(ob, isNotNull);
      expect(ob.url, equals(_testUrl));
      ob.dispose();
    });

    test('register returns authenticated state', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email = 'smoke_${DateTime.now().millisecondsSinceEpoch}@test.com';
      final result = await ob.auth.register(email, 'TestPassword123!');
      expect(result.isAuthenticated, isTrue);
      expect(result.userId, isNotNull);
      ob.dispose();
    });

    test('login with wrong creds fails', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/login')) {
          return _errorResp(401, 'Invalid credentials');
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      expect(
        () => ob.auth.signInWithEmail(
          'nonexistent_smoke@test.com',
          'WrongPassword',
        ),
        throwsException,
      );
      ob.dispose();
    });

    test('CRUD create works', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          return http.Response(
            jsonEncode({
              'data': {
                'create': {
                  'id': 'smoke_test:item1',
                  'name': 'smoke_item',
                  'value': 42,
                },
              },
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'smoke_crud_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');

      final doc = await ob.collection('smoke_test').add({
        'name': 'smoke_item',
        'value': 42,
      });
      expect(doc.id, isNotEmpty);
      ob.dispose();
    });

    test('CRUD read works', () async {
      var registerCalled = false;
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          registerCalled = true;
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          final body = jsonDecode(req.body) as Map<String, dynamic>;
          final query = body['query'] as String? ?? '';
          // First graphql call is create, second is get
          if (query.contains('create')) {
            return http.Response(
              jsonEncode({
                'data': {
                  'create': {
                    'id': 'smoke_read:item1',
                    'name': 'readable',
                  },
                },
              }),
              200,
              headers: {'content-type': 'application/json'},
            );
          }
          return http.Response(
            jsonEncode({
              'data': {
                'get': {
                  'id': 'smoke_read:item1',
                  'name': 'readable',
                },
              },
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'smoke_read_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');
      expect(registerCalled, isTrue);

      final created = await ob.collection('smoke_read').add({
        'name': 'readable',
      });

      final fetched = await ob.collection('smoke_read').doc(created.id).get();
      expect(fetched, isNotNull);
      expect(fetched!.data['name'], equals('readable'));
      ob.dispose();
    });

    test('GraphQL introspection works', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/graphql')) {
          return http.Response(
            jsonEncode({
              'data': {
                '__schema': {
                  'queryType': {'name': 'Query'},
                },
              },
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final result = await ob.graphql('{ __schema { queryType { name } } }');
      expect(result, isNotNull);
      expect(result['data'], isNotNull);
      ob.dispose();
    });

    test('unauthenticated mutation fails', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/graphql')) {
          return _errorResp(401, 'Unauthorized');
        }
        return _errorResp(404, 'Not found');
      });

      final freshOb = OrignaBase.initialize(url: _testUrl, httpClient: client);
      try {
        await freshOb.collection('protected').add({'test': true});
        fail('Should have thrown');
      } catch (e) {
        expect(e, isNotNull);
      } finally {
        freshOb.dispose();
      }
    });

    test('search endpoint reachable', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          return http.Response(
            jsonEncode({
              'data': {
                'search': {
                  'hits': [],
                  'totalHits': 0,
                },
              },
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'smoke_search_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');

      final results = await ob.search('products', 'test_query');
      expect(results, isNotNull);
      ob.dispose();
    });
  });
}
