@TestOn('vm')
library;

import 'package:test/test.dart';
import 'package:http/http.dart' as http;
import 'package:orignabase/orignabase.dart';

const baseUrl = 'http://localhost:8080';

/// Generates a unique email to avoid conflicts across test runs.
String uniqueEmail() =>
    'test_${DateTime.now().millisecondsSinceEpoch}@integration.test';

void main() {
  late bool serverAvailable;

  setUpAll(() async {
    serverAvailable = false;
    try {
      final resp = await http
          .get(Uri.parse('$baseUrl/health'))
          .timeout(const Duration(seconds: 3));
      serverAvailable = resp.statusCode == 200;
    } catch (_) {}
  });

  group('Auth integration', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: baseUrl);
    });

    tearDown(() {
      ob.dispose();
    });

    test('register a new user and verify auth state', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
      final email = uniqueEmail();
      final state = await ob.auth.register(email, 'TestPassword123!');
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, isNotNull);
    });

    test('sign in with email and verify tokens set', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
      final email = uniqueEmail();
      // Register first
      await ob.auth.register(email, 'TestPassword123!');
      ob.auth.signOut();
      // Sign in
      final state = await ob.auth.signInWithEmail(email, 'TestPassword123!');
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, isNotNull);
      expect(ob.auth.accessToken!.isNotEmpty, true);
    });

    test('sign out clears tokens', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
      final email = uniqueEmail();
      await ob.auth.register(email, 'TestPassword123!');
      expect(ob.auth.accessToken, isNotNull);

      ob.auth.signOut();
      expect(ob.auth.accessToken, isNull);
      expect(ob.auth.currentState.isAuthenticated, false);
    });

    test('invalid credentials throw AuthException', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
      expect(
        () => ob.auth.signInWithEmail('nonexistent@test.com', 'wrongpassword'),
        throwsA(isA<AuthException>()),
      );
    });
  });

  group('Collection CRUD integration', () {
    late OrignaBase ob;

    setUp(() async {
      ob = OrignaBase.initialize(url: baseUrl);
      if (serverAvailable) {
        final email = uniqueEmail();
        await ob.auth.register(email, 'TestPassword123!');
      }
    });

    tearDown(() {
      ob.dispose();
    });

    test('add document and get it back', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
      final collection = ob.collection('integration_test_items');
      final created = await collection.add({
        'title': 'Test Item',
        'price': 42.0,
        'timestamp': DateTime.now().millisecondsSinceEpoch,
      });

      expect(created, isA<Document>());

      // If the server returned an ID, fetch it back
      if (created.id.isNotEmpty) {
        final fetched = await collection.doc(created.id).get();
        expect(fetched, isNotNull);
        expect(fetched!['title'], 'Test Item');
        expect(fetched['price'], 42.0);

        // Cleanup
        await collection.doc(created.id).delete();
      }
    });

    test('query with filters', () async {
      if (!serverAvailable) {
        markTestSkipped('OrignaBase server not running at $baseUrl');
        return;
      }
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
    });
  });
}
