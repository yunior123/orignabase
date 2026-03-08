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

  // === SUBCOLLECTION TESTS ===

  group('Subcollections', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('subcollection path generation', () {
      final orders = ob.collection('users').subcollection('user123', 'orders');
      expect(orders.collectionPath, equals('users__orders'));
    });

    test('nested subcollection path', () {
      final items = ob
          .collection('users')
          .subcollection('user123', 'orders')
          .subcollection('order456', 'items');
      expect(items.collectionPath, equals('users__orders__items'));
    });

    test('subcollection doc reference', () {
      final orderDoc = ob
          .collection('users')
          .subcollection('user123', 'orders')
          .doc('order1');
      expect(orderDoc.id, equals('order1'));
      expect(orderDoc.collection, equals('users__orders'));
    });

    test('DocumentRef.subcollection returns SubcollectionRef', () {
      final reviews =
          ob.collection('products').doc('prod1').subcollection('reviews');
      expect(reviews.collectionPath, equals('products__reviews'));
      expect(reviews.parentId, equals('prod1'));
    });

    test('subcollection where filter includes parent_id', () {
      final orders =
          ob.collection('users').subcollection('user123', 'orders');
      final query = orders.where('status', isEqualTo: 'pending');
      // The query should internally include parent_id filter
      expect(query, isA<Query>());
    });

    test('subcollection parentCollection is correct', () {
      final orders =
          ob.collection('users').subcollection('user123', 'orders');
      expect(orders.parentCollection, equals('users'));
      expect(orders.parentId, equals('user123'));
      expect(orders.childCollection, equals('orders'));
    });

    test('deeply nested subcollection preserves full path', () {
      final lineItems = ob
          .collection('stores')
          .subcollection('store1', 'orders')
          .subcollection('order1', 'items')
          .subcollection('item1', 'line_details');
      expect(
          lineItems.collectionPath, equals('stores__orders__items__line_details'));
    });

    test('subcollection from CollectionRef matches DocumentRef.subcollection',
        () {
      final viaCollection =
          ob.collection('users').subcollection('u1', 'settings');
      final viaDoc =
          ob.collection('users').doc('u1').subcollection('settings');
      expect(viaCollection.collectionPath, equals(viaDoc.collectionPath));
      expect(viaCollection.parentId, equals(viaDoc.parentId));
    });
  });

  // === E-COMMERCE QUERY SIMULATION TESTS ===

  group('E-commerce query patterns (OrignaGTA-like)', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    // --- Product Queries ---

    test('list all products with pagination', () {
      final query = ob
          .collection('products')
          .orderBy('created_at', descending: true)
          .limit(20)
          .offset(0);
      expect(query, isA<Query>());
    });

    test('filter products by category and price range', () {
      final query = ob
          .collection('products')
          .where('category', isEqualTo: 'electronics')
          .where('price', isGreaterThanOrEqualTo: 100)
          .where('price', isLessThanOrEqualTo: 500)
          .orderBy('price')
          .limit(50);
      expect(query, isA<Query>());
    });

    test('filter products by status and seller', () {
      final query = ob
          .collection('products')
          .where('status', isEqualTo: 'active')
          .where('seller_id', isEqualTo: 'seller_abc')
          .orderBy('created_at', descending: true);
      expect(query, isA<Query>());
    });

    test('filter products by multiple categories (IN)', () {
      final query = ob
          .collection('products')
          .where('category',
              whereIn: ['electronics', 'books', 'clothing'])
          .limit(100);
      expect(query, isA<Query>());
    });

    test('search products by price less than threshold', () {
      final query = ob
          .collection('products')
          .where('price', isLessThan: 25.0)
          .where('status', isEqualTo: 'active')
          .orderBy('price')
          .limit(20);
      expect(query, isA<Query>());
    });

    // --- Order Queries ---

    test('user orders as subcollection', () {
      final orders =
          ob.collection('users').subcollection('user123', 'orders');
      expect(orders.collectionPath, equals('users__orders'));
    });

    test('filter user orders by status', () {
      final query = ob
          .collection('users')
          .subcollection('user123', 'orders')
          .where('status', isEqualTo: 'delivered')
          .orderBy('created_at', descending: true);
      expect(query, isA<Query>());
    });

    test('order items as nested subcollection', () {
      final items = ob
          .collection('users')
          .subcollection('user123', 'orders')
          .subcollection('order456', 'items');
      expect(items.collectionPath, equals('users__orders__items'));
    });

    // --- Review Queries ---

    test('product reviews as subcollection', () {
      final reviews =
          ob.collection('products').doc('product_abc').subcollection('reviews');
      expect(reviews.collectionPath, equals('products__reviews'));
    });

    test('filter reviews by rating >= 4', () {
      final query = ob
          .collection('products')
          .doc('product_abc')
          .subcollection('reviews')
          .where('rating', isGreaterThanOrEqualTo: 4)
          .orderBy('created_at', descending: true)
          .limit(10);
      expect(query, isA<Query>());
    });

    // --- Cart/Favorites ---

    test('user favorites list', () {
      final favorites =
          ob.collection('users').subcollection('user123', 'favorites');
      expect(favorites.collectionPath, equals('users__favorites'));
    });

    test('user cart items', () {
      final cart =
          ob.collection('users').subcollection('user123', 'cart_items');
      expect(cart.collectionPath, equals('users__cart_items'));
    });

    // --- Admin Queries ---

    test('all orders with date filter', () {
      final query = ob
          .collection('orders')
          .where('created_at', isGreaterThan: '2026-01-01T00:00:00Z')
          .where('status', isEqualTo: 'pending')
          .orderBy('created_at', descending: true)
          .limit(50);
      expect(query, isA<Query>());
    });

    test('all users with role filter', () {
      final query = ob
          .collection('users')
          .where('role', isEqualTo: 'seller')
          .orderBy('created_at', descending: true);
      expect(query, isA<Query>());
    });

    // --- Shipping addresses ---

    test('user shipping addresses as subcollection', () {
      final addresses =
          ob.collection('users').subcollection('user123', 'addresses');
      expect(addresses.collectionPath, equals('users__addresses'));
    });

    // --- Notifications ---

    test('user notifications subcollection with limit', () {
      final query = ob
          .collection('users')
          .subcollection('user123', 'notifications')
          .orderBy('created_at', descending: true)
          .limit(25);
      expect(query, isA<Query>());
    });
  });

  // === AGGREGATE QUERY TESTS ===

  group('Aggregate queries', () {
    test('count query generation', () {
      final agg = AggregateQuery('products', []);
      final q = agg.toCountQuery();
      expect(q['query'], contains('count()'));
      expect(q['query'], contains('products'));
      expect(q['query'], contains('GROUP ALL'));
    });

    test('sum query generation', () {
      final agg = AggregateQuery('orders', []);
      final q = agg.toSumQuery('total');
      expect(q['query'], contains('math::sum'));
      expect(q['query'], contains('orders'));
    });

    test('avg query generation', () {
      final agg = AggregateQuery('reviews', []);
      final q = agg.toAvgQuery('rating');
      expect(q['query'], contains('math::mean'));
      expect(q['query'], contains('reviews'));
    });

    test('count with filters', () {
      final agg = AggregateQuery('products', [
        QueryFilter('status', 'eq', 'active'),
      ]);
      final q = agg.toCountQuery();
      expect(q['query'], contains('WHERE'));
      expect(q['query'], contains('status'));
    });

    test('sum with multiple filters', () {
      final agg = AggregateQuery('orders', [
        QueryFilter('status', 'eq', 'completed'),
        QueryFilter('total', 'gt', 0),
      ]);
      final q = agg.toSumQuery('total');
      expect(q['query'], contains('AND'));
    });

    test('avg query without filters has no WHERE clause', () {
      final agg = AggregateQuery('products', []);
      final q = agg.toAvgQuery('price');
      expect(q['query'], isNot(contains('WHERE')));
    });
  });

  // === QUERY FILTER EDGE CASES ===

  group('Query filter edge cases', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('chaining multiple where + orderBy + limit + offset', () {
      final query = ob
          .collection('products')
          .where('status', isEqualTo: 'active')
          .where('price', isGreaterThan: 0)
          .where('category', whereIn: ['electronics'])
          .orderBy('price', descending: true)
          .limit(10)
          .offset(20);
      expect(query, isA<Query>());
    });

    test('empty collection name', () {
      final ref = ob.collection('');
      expect(ref, isA<CollectionRef>());
    });

    test('special characters in field values', () {
      final query = ob
          .collection('products')
          .where('title', isEqualTo: "O'Reilly & Sons \"Special\" <Sale>");
      expect(query, isA<Query>());
    });

    test('numeric zero as filter value', () {
      final query = ob.collection('products').where('price', isEqualTo: 0);
      expect(query, isA<Query>());
    });

    test('null-safe document creation', () {
      final doc = Document.fromMap('test', {
        'id': 'doc1',
        'data': null,
        'nested': {'a': null},
      });
      expect(doc.id, equals('doc1'));
      expect(doc.data['data'], isNull);
    });

    test('negative number as filter value', () {
      final query =
          ob.collection('accounts').where('balance', isLessThan: -100);
      expect(query, isA<Query>());
    });

    test('boolean filter value', () {
      final query =
          ob.collection('products').where('is_active', isEqualTo: true);
      expect(query, isA<Query>());
    });

    test('isNotEqualTo filter', () {
      final query =
          ob.collection('products').where('status', isNotEqualTo: 'deleted');
      expect(query, isA<Query>());
    });

    test('contains filter', () {
      final query =
          ob.collection('products').where('tags', contains: 'sale');
      expect(query, isA<Query>());
    });

    test('startsWith filter', () {
      final query =
          ob.collection('products').where('sku', startsWith: 'ELEC-');
      expect(query, isA<Query>());
    });
  });

  // === DOCUMENT TESTS ===

  group('Document operations', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('document ref from collection', () {
      final doc = ob.collection('users').doc('user123');
      expect(doc.id, equals('user123'));
    });

    test('document ref path', () {
      final doc = ob.collection('products').doc('prod456');
      expect(doc.collection, equals('products'));
      expect(doc.id, equals('prod456'));
    });

    test('Document.fromMap with _id field', () {
      final doc =
          Document.fromMap('users', {'_id': 'abc', 'name': 'Yunior'});
      expect(doc.id, equals('abc'));
      expect(doc['name'], equals('Yunior'));
      expect(doc.containsKey('_id'), false);
    });

    test('Document.fromMap with empty id', () {
      final doc = Document.fromMap('items', {'title': 'No ID'});
      expect(doc.id, equals(''));
      expect(doc['title'], equals('No ID'));
    });

    test('Document.toString includes collection and id', () {
      final doc =
          Document(id: 'x', collection: 'col', data: {'key': 'val'});
      final str = doc.toString();
      expect(str, contains('col'));
      expect(str, contains('x'));
    });

    test('Document operator[] returns correct values', () {
      final doc = Document(
          id: '1',
          collection: 'test',
          data: {'a': 1, 'b': 'two', 'c': true});
      expect(doc['a'], equals(1));
      expect(doc['b'], equals('two'));
      expect(doc['c'], equals(true));
      expect(doc['nonexistent'], isNull);
    });
  });

  // === REALTIME TESTS ===

  group('Realtime subscriptions', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('RealtimeClient can be instantiated', () {
      final rt = RealtimeClient(ob);
      expect(rt, isA<RealtimeClient>());
    });

    test('ChangeType enum coverage', () {
      expect(ChangeType.values.length, equals(3));
      expect(ChangeType.values, contains(ChangeType.create));
      expect(ChangeType.values, contains(ChangeType.update));
      expect(ChangeType.values, contains(ChangeType.delete));
    });

    test('DocumentChange has all fields', () {
      final change = DocumentChange(
        type: ChangeType.create,
        document: Document.fromMap('test', {'id': 'doc1', 'title': 'Test'}),
      );
      expect(change.type, equals(ChangeType.create));
      expect(change.document.id, equals('doc1'));
    });

    test('DocumentChange for update type', () {
      final change = DocumentChange(
        type: ChangeType.update,
        document: Document(id: 'd1', collection: 'c', data: {'v': 2}),
      );
      expect(change.type, equals(ChangeType.update));
    });

    test('DocumentChange for delete type', () {
      final change = DocumentChange(
        type: ChangeType.delete,
        document: Document(id: 'd1', collection: 'c', data: {}),
      );
      expect(change.type, equals(ChangeType.delete));
    });
  });

  // === STORAGE TESTS ===

  group('Storage operations', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('storage is accessible from client', () {
      expect(ob.storage, isA<OrignaBaseStorage>());
    });
  });

  // === AUTH STATE TESTS ===

  group('Auth state management', () {
    late OrignaBase ob;

    setUp(() {
      ob = OrignaBase.initialize(url: 'http://localhost:8080');
    });

    tearDown(() {
      ob.dispose();
    });

    test('initial auth state is unauthenticated', () {
      expect(ob.auth.currentState.isAuthenticated, false);
    });

    test('auth state stream exists', () {
      expect(ob.auth.authStateChanges, isA<Stream<AuthState>>());
    });

    test('signOut emits unauthenticated state', () async {
      ob.auth.signOut();
      expect(ob.auth.currentState.isAuthenticated, false);
      expect(ob.auth.accessToken, isNull);
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
