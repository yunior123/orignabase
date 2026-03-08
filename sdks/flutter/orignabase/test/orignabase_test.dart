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
  });
}
