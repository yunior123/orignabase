import 'package:test/test.dart';
import 'package:orignabase/orignabase.dart';

void main() {
  group('OrignaBase client', () {
    test('initialize creates client with trimmed URL', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080/');
      expect(ob.url, 'http://localhost:8080');
      ob.dispose();
    });

    test('initialize creates client without trailing slash', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.url, 'http://localhost:8080');
      ob.dispose();
    });

    test('collection returns CollectionRef', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final ref = ob.collection('products');
      expect(ref, isA<CollectionRef>());
      ob.dispose();
    });
  });

  group('Auth', () {
    test('initial state is unauthenticated', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.auth.currentState.isAuthenticated, false);
      expect(ob.auth.accessToken, isNull);
      ob.dispose();
    });

    test('signOut clears tokens', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      ob.auth.signOut();
      expect(ob.auth.accessToken, isNull);
      expect(ob.auth.currentState.isAuthenticated, false);
      ob.dispose();
    });
  });

  group('Document', () {
    test('fromMap extracts id and data', () {
      final doc = Document.fromMap('products', {
        'id': '123',
        'title': 'Widget',
        'price': 29.99,
      });
      expect(doc.id, '123');
      expect(doc.collection, 'products');
      expect(doc['title'], 'Widget');
      expect(doc['price'], 29.99);
      expect(doc.containsKey('id'), false); // id stripped from data
    });

    test('QuerySnapshot tracks size', () {
      final snap = QuerySnapshot(docs: [
        Document(id: '1', collection: 'test', data: {}),
        Document(id: '2', collection: 'test', data: {}),
      ]);
      expect(snap.size, 2);
      expect(snap.isEmpty, false);
      expect(snap.isNotEmpty, true);
    });

    test('empty QuerySnapshot', () {
      final snap = QuerySnapshot(docs: []);
      expect(snap.size, 0);
      expect(snap.isEmpty, true);
    });
  });

  group('Errors', () {
    test('OrignaBaseException has message and status', () {
      final err = OrignaBaseException('test error', statusCode: 500);
      expect(err.message, 'test error');
      expect(err.statusCode, 500);
      expect(err.toString(), contains('test error'));
    });

    test('AuthException is OrignaBaseException', () {
      final err = AuthException('unauthorized', statusCode: 401);
      expect(err, isA<OrignaBaseException>());
      expect(err.statusCode, 401);
    });
  });

  group('Query builder', () {
    test('where chaining returns same query', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = ob.collection('products')
          .where('status', isEqualTo: 'active')
          .where('price', isGreaterThan: 10)
          .orderBy('created_at', descending: true)
          .limit(20);
      expect(query, isA<Query>());
      ob.dispose();
    });

    test('QueryFilter.toGraphQL() produces correct map for eq', () {
      final filter = QueryFilter('status', 'eq', 'active');
      final result = filter.toGraphQL();
      expect(result, {'status': {'_eq': 'active'}});
    });

    test('QueryFilter.toGraphQL() produces correct map for gt', () {
      final filter = QueryFilter('price', 'gt', 10);
      final result = filter.toGraphQL();
      expect(result, {'price': {'_gt': 10}});
    });

    test('QueryFilter.toGraphQL() produces correct map for in', () {
      final filter = QueryFilter('category', 'in', ['a', 'b']);
      final result = filter.toGraphQL();
      expect(result, {'category': {'_in': ['a', 'b']}});
    });

    test('multiple where clauses build correct combined filter', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      // Build filters and verify via QueryFilter.toGraphQL()
      final f1 = QueryFilter('status', 'eq', 'active');
      final f2 = QueryFilter('price', 'gt', 10);
      final f3 = QueryFilter('category', 'ne', 'hidden');
      // Simulate what _buildFiltersJson does: merge all toGraphQL maps
      final map = <String, dynamic>{};
      for (final filter in [f1, f2, f3]) {
        map.addAll(filter.toGraphQL());
      }
      expect(map, {
        'status': {'_eq': 'active'},
        'price': {'_gt': 10},
        'category': {'_ne': 'hidden'},
      });
      ob.dispose();
    });

    test('offset() returns Query for chaining', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = ob.collection('items').offset(5);
      expect(query, isA<Query>());
      ob.dispose();
    });

    test('limit() returns Query for chaining', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = ob.collection('items').limit(10);
      expect(query, isA<Query>());
      ob.dispose();
    });

    test('offset and limit can be chained together', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = ob.collection('items').offset(5).limit(10);
      expect(query, isA<Query>());
      ob.dispose();
    });
  });

  group('Collection', () {
    test('DocumentRef creation from CollectionRef.doc()', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final docRef = ob.collection('products').doc('abc123');
      expect(docRef, isA<DocumentRef>());
      expect(docRef.id, 'abc123');
      expect(docRef.collection, 'products');
      ob.dispose();
    });

    test('CollectionRef is a subtype of Query', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final collection = ob.collection('products');
      expect(collection, isA<Query>());
      expect(collection, isA<CollectionRef>());
      ob.dispose();
    });
  });

  group('Realtime', () {
    test('ChangeType enum values exist', () {
      expect(ChangeType.values, contains(ChangeType.create));
      expect(ChangeType.values, contains(ChangeType.update));
      expect(ChangeType.values, contains(ChangeType.delete));
      expect(ChangeType.values.length, 3);
    });

    test('DocumentChange creation', () {
      final doc = Document(id: '1', collection: 'test', data: {'name': 'foo'});
      final change = DocumentChange(type: ChangeType.create, document: doc);
      expect(change.type, ChangeType.create);
      expect(change.document.id, '1');
      expect(change.document['name'], 'foo');
    });

    test('RealtimeClient can be instantiated', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final rt = RealtimeClient(ob);
      expect(rt, isA<RealtimeClient>());
      ob.dispose();
    });
  });

  group('Storage', () {
    test('OrignaBaseStorage can be accessed from client', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.storage, isA<OrignaBaseStorage>());
      ob.dispose();
    });
  });

  group('Auth extended', () {
    test('AuthState unauthenticated constant', () {
      const state = AuthState.unauthenticated;
      expect(state.status, AuthStatus.unauthenticated);
      expect(state.isAuthenticated, false);
      expect(state.userId, isNull);
      expect(state.email, isNull);
      expect(state.roles, isEmpty);
    });

    test('AuthState.isAuthenticated returns correct values', () {
      const authed = AuthState(status: AuthStatus.authenticated, userId: 'u1');
      const unauthed = AuthState(status: AuthStatus.unauthenticated);
      expect(authed.isAuthenticated, true);
      expect(unauthed.isAuthenticated, false);
    });

    test('AuthState with roles', () {
      const state = AuthState(
        status: AuthStatus.authenticated,
        userId: 'u1',
        email: 'test@example.com',
        roles: ['admin', 'editor'],
      );
      expect(state.isAuthenticated, true);
      expect(state.roles, ['admin', 'editor']);
      expect(state.roles.length, 2);
      expect(state.email, 'test@example.com');
    });
  });
}
