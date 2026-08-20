/// Integration tests for OrignaBase Flutter SDK.
///
/// Tests auth flows, subcollections, and CRUD using mock HTTP responses.
/// For live-server integration, use test/live_integration_test.dart.
@TestOn('vm')
library;

import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

const _testUrl = 'http://test.local';

/// Creates a mock HTTP client from a handler.
http.Client _mockHttp(
  http.Response Function(http.Request request) handler,
) {
  return MockClient((request) async => handler(request));
}

/// Standard auth success response.
http.Response _authOk({
  String userId = 'users:test1',
  String email = 'test@integration.test',
}) {
  return http.Response(
    jsonEncode({
      'access_token': 'eyJ.mock.token',
      'refresh_token': 'refresh_mock',
      'user_id': userId,
      'user': {'id': userId, 'email': email},
    }),
    200,
    headers: {'content-type': 'application/json'},
  );
}

/// Error response.
http.Response _errorResp(int status, String message) {
  return http.Response(
    jsonEncode({'error': message}),
    status,
    headers: {'content-type': 'application/json'},
  );
}

/// GraphQL success response.
http.Response _graphqlOk(Map<String, dynamic> data) {
  return http.Response(
    jsonEncode({'data': data}),
    200,
    headers: {'content-type': 'application/json'},
  );
}

void main() {
  group('Auth integration', () {
    late OrignaBase ob;

    test('register a new user and verify auth state', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk(userId: 'users:new1');
        }
        return _errorResp(404, 'Not found');
      });

      ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final state = await ob.auth.register(
        'test_${DateTime.now().millisecondsSinceEpoch}@integration.test',
        'TestPassword123!',
      );
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, isNotNull);
      ob.dispose();
    });

    test('sign in with email and verify tokens set', () async {
      var callCount = 0;
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk(userId: 'users:signin1');
        }
        if (req.url.path.contains('/auth/login')) {
          callCount++;
          return _authOk(userId: 'users:signin1');
        }
        return _errorResp(404, 'Not found');
      });

      ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'test_${DateTime.now().millisecondsSinceEpoch}@integration.test';
      await ob.auth.register(email, 'TestPassword123!');
      await ob.auth.signOut();

      final state = await ob.auth.signInWithEmail(email, 'TestPassword123!');
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, isNotNull);
      expect(ob.auth.accessToken!.isNotEmpty, true);
      expect(callCount, 1);
      ob.dispose();
    });

    test('sign out clears tokens', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk(userId: 'users:signout1');
        }
        return _errorResp(404, 'Not found');
      });

      ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'test_${DateTime.now().millisecondsSinceEpoch}@integration.test';
      await ob.auth.register(email, 'TestPassword123!');
      expect(ob.auth.accessToken, isNotNull);

      await ob.auth.signOut();
      expect(ob.auth.accessToken, isNull);
      expect(ob.auth.currentState.isAuthenticated, false);
      ob.dispose();
    });

    test('invalid credentials throw AuthException', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/login')) {
          return _errorResp(401, 'Invalid credentials');
        }
        return _errorResp(404, 'Not found');
      });

      ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      expect(
        () => ob.auth.signInWithEmail('nonexistent@test.com', 'wrongpassword'),
        throwsA(isA<AuthException>()),
      );
      ob.dispose();
    });
  });

  group('Subcollection integration', () {
    test('subcollection path is correct for user orders', () {
      final ob = OrignaBase.initialize(url: _testUrl);
      final orders = ob.collection('users').subcollection('user123', 'orders');
      expect(orders.collectionPath, equals('users__orders'));
      expect(orders.parentId, equals('user123'));
      ob.dispose();
    });

    test('nested subcollection path for order items', () {
      final ob = OrignaBase.initialize(url: _testUrl);
      final items = ob
          .collection('users')
          .subcollection('user123', 'orders')
          .subcollection('order456', 'items');
      expect(items.collectionPath, equals('users__orders__items'));
      ob.dispose();
    });

    test('create and query subcollection documents', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          return _graphqlOk({
            'create': {
              'id': 'products__reviews:rev1',
              'rating': 5,
              'text': 'Great product',
              'parent_id': 'prod1',
            },
          });
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      await ob.auth.register('sub@test.com', 'TestPassword123!');

      final reviews =
          ob.collection('products').subcollection('prod1', 'reviews');
      expect(reviews.collectionPath, equals('products__reviews'));

      // Verify subcollection CRUD works through the mock
      final doc = await reviews.add({
        'rating': 5,
        'text': 'Great product',
      });
      expect(doc.id, isNotEmpty);
      ob.dispose();
    });
  });

  group('Collection CRUD integration', () {
    test('add document and get it back', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          final body = jsonDecode(req.body) as Map<String, dynamic>;
          final query = body['query'] as String? ?? '';
          if (query.contains('create')) {
            return _graphqlOk({
              'create': {
                'id': 'integration_test_items:item1',
                'title': 'Test Item',
                'price': 42.0,
              },
            });
          }
          if (query.contains('get')) {
            return _graphqlOk({
              'get': {
                'id': 'integration_test_items:item1',
                'title': 'Test Item',
                'price': 42.0,
              },
            });
          }
          // delete
          return _graphqlOk({'delete': true});
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'test_${DateTime.now().millisecondsSinceEpoch}@integration.test';
      await ob.auth.register(email, 'TestPassword123!');

      final collection = ob.collection('integration_test_items');
      final created = await collection.add({
        'title': 'Test Item',
        'price': 42.0,
        'timestamp': DateTime.now().millisecondsSinceEpoch,
      });

      expect(created, isA<Document>());

      if (created.id.isNotEmpty) {
        final fetched = await collection.doc(created.id).get();
        expect(fetched, isNotNull);
        expect(fetched!['title'], 'Test Item');
        expect(fetched['price'], 42.0);

        // Cleanup
        await collection.doc(created.id).delete();
      }
      ob.dispose();
    });

    test('query with filters', () async {
      final client = _mockHttp((req) {
        if (req.url.path.contains('/auth/register')) {
          return _authOk();
        }
        if (req.url.path.contains('/graphql')) {
          final body = jsonDecode(req.body) as Map<String, dynamic>;
          final query = body['query'] as String? ?? '';
          if (query.contains('create')) {
            return _graphqlOk({
              'create': {
                'id': 'integration_test_query:item1',
                'label': 'test',
                'value': 20,
              },
            });
          }
          // list/query response
          return _graphqlOk({
            'list': [
              {
                'id': 'integration_test_query:item2',
                'label': 'beta',
                'value': 20,
              },
              {
                'id': 'integration_test_query:item3',
                'label': 'gamma',
                'value': 30,
              },
            ],
          });
        }
        return _errorResp(404, 'Not found');
      });

      final ob = OrignaBase.initialize(url: _testUrl, httpClient: client);
      final email =
          'test_${DateTime.now().millisecondsSinceEpoch}@integration.test';
      await ob.auth.register(email, 'TestPassword123!');

      final collection = ob.collection('integration_test_query');
      final ts = DateTime.now().millisecondsSinceEpoch;

      // Seed data
      await collection.add({'label': 'alpha_$ts', 'value': 10});
      await collection.add({'label': 'beta_$ts', 'value': 20});
      await collection.add({'label': 'gamma_$ts', 'value': 30});

      // Query with filter and limit
      final results = await collection
          .where('value', isGreaterThan: 15)
          .orderBy('value')
          .limit(2)
          .get();

      expect(results, isA<QuerySnapshot>());
      expect(results.size, lessThanOrEqualTo(2));

      // All returned docs should have value > 15
      for (final doc in results.docs) {
        expect((doc['value'] as num), greaterThan(15));
      }
      ob.dispose();
    });
  });
}
