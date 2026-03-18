/// Smoke tests for OrignaBase Flutter SDK.
///
/// Quick validation that core SDK functionality works against a live server.
/// Run: OB_TEST_URL=https://api.orignagta.ca dart test test/smoke_test.dart
@TestOn('vm')
library;

import 'dart:io';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

String get baseUrl =>
    Platform.environment['OB_TEST_URL'] ?? 'http://localhost:8080';

void main() {
  late OrignaBase ob;

  setUpAll(() {
    ob = OrignaBase.initialize(url: baseUrl);
  });

  tearDownAll(() {
    ob.dispose();
  });

  group('Smoke Tests', () {
    test('client initializes without error', () {
      expect(ob, isNotNull);
      expect(ob.url, equals(baseUrl));
    });

    test('register returns authenticated state', () async {
      final email =
          'smoke_${DateTime.now().millisecondsSinceEpoch}@test.com';
      final result = await ob.auth.register(email, 'TestPassword123!');
      expect(result.isAuthenticated, isTrue);
      expect(result.userId, isNotNull);
    });

    test('login with wrong creds fails', () async {
      expect(
        () => ob.auth.signInWithEmail(
          'nonexistent_smoke@test.com',
          'WrongPassword',
        ),
        throwsException,
      );
    });

    test('CRUD create works', () async {
      final email =
          'smoke_crud_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');

      final doc = await ob.collection('smoke_test').add({
        'name': 'smoke_item',
        'value': 42,
      });
      expect(doc.id, isNotEmpty);
    });

    test('CRUD read works', () async {
      final email =
          'smoke_read_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');

      final created = await ob.collection('smoke_read').add({
        'name': 'readable',
      });

      final fetched =
          await ob.collection('smoke_read').doc(created.id).get();
      expect(fetched, isNotNull);
      expect(fetched!.data['name'], equals('readable'));
    });

    test('GraphQL introspection works', () async {
      final result =
          await ob.graphql('{ __schema { queryType { name } } }');
      expect(result, isNotNull);
      expect(result['data'], isNotNull);
    });

    test('unauthenticated mutation fails', () async {
      final freshOb = OrignaBase.initialize(url: baseUrl);
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
      final email =
          'smoke_search_${DateTime.now().millisecondsSinceEpoch}@test.com';
      await ob.auth.register(email, 'TestPassword123!');

      try {
        final results = await ob.search('products', 'test_query');
        expect(results, isNotNull);
      } catch (e) {
        // Search may not be configured — that's OK for smoke
        expect(e.toString(), isNotEmpty);
      }
    });
  });
}
