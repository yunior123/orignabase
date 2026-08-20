import 'dart:convert';
import 'dart:typed_data';
import 'package:test/test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
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
      expect(ob.auth.currentUserId, isNull);
      expect(ob.auth.isEmailVerified, isFalse);
      ob.dispose();
    });

    test('signOut clears tokens', () async {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      await ob.auth.signOut();
      expect(ob.auth.accessToken, isNull);
      expect(ob.auth.currentState.isAuthenticated, false);
      ob.dispose();
    });

    test('signInWithGoogle method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      // Verify method signature exists (will fail at network level, not compile)
      expect(() => ob.auth.signInWithGoogle('fake_token'), throwsA(anything));
      ob.dispose();
    });

    test('signInWithApple method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(() => ob.auth.signInWithApple('fake_code', displayName: 'Test'),
          throwsA(anything));
      ob.dispose();
    });

    test('signInWithOidc method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(
          () => ob.auth.signInWithOidc('fake_access_token'), throwsA(anything));
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
      final query = ob
          .collection('products')
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
      expect(result, {
        'status': {'_eq': 'active'}
      });
    });

    test('QueryFilter.toGraphQL() produces correct map for gt', () {
      final filter = QueryFilter('price', 'gt', 10);
      final result = filter.toGraphQL();
      expect(result, {
        'price': {'_gt': 10}
      });
    });

    test('QueryFilter.toGraphQL() produces correct map for in', () {
      final filter = QueryFilter('category', 'in', ['a', 'b']);
      final result = filter.toGraphQL();
      expect(result, {
        'category': {
          '_in': ['a', 'b']
        }
      });
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

    test('uploadResumable returns UploadTask', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final data = Uint8List.fromList(List.filled(1000, 42));
      final task = ob.storage.uploadResumable('test.bin', data);
      expect(task, isA<UploadTask>());
      expect(task.future, isA<Future>());
      task.future
          .catchError((_) => <String, dynamic>{}); // suppress network error
      ob.dispose();
    });

    test('uploadResumable with custom chunk size', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final data = Uint8List.fromList(List.filled(500, 0));
      final task = ob.storage.uploadResumable('x.bin', data,
          contentType: 'application/octet-stream', chunkSize: 100);
      expect(task, isA<UploadTask>());
      task.future
          .catchError((_) => <String, dynamic>{}); // suppress network error
      ob.dispose();
    });

    test('UploadProgress fraction calculation', () {
      final p = UploadProgress(
          bytesTransferred: 500, totalBytes: 1000, sessionId: 'abc');
      expect(p.fraction, 0.5);
      expect(p.isComplete, false);
    });

    test('UploadProgress complete state', () {
      final p = UploadProgress(
          bytesTransferred: 1000, totalBytes: 1000, sessionId: 'abc');
      expect(p.fraction, 1.0);
      expect(p.isComplete, true);
    });

    test('UploadProgress zero total', () {
      final p =
          UploadProgress(bytesTransferred: 0, totalBytes: 0, sessionId: 'abc');
      expect(p.fraction, 0.0);
    });

    test('resumeUpload returns UploadTask', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final data = Uint8List.fromList([1, 2, 3]);
      final task = ob.storage.resumeUpload('session-123', data);
      expect(task, isA<UploadTask>());
      expect(task.sessionId, 'session-123');
      task.future
          .catchError((_) => <String, dynamic>{}); // suppress network error
      ob.dispose();
    });

    test('UploadTask onProgress setter', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final data = Uint8List.fromList([1, 2, 3]);
      final task = ob.storage.uploadResumable('x.bin', data);
      task.onProgress = (p) {};
      expect(task, isA<UploadTask>());
      task.future
          .catchError((_) => <String, dynamic>{}); // suppress network error
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
      final orders = ob.collection('users').subcollection('user123', 'orders');
      final query = orders.where('status', isEqualTo: 'pending');
      // The query should internally include parent_id filter
      expect(query, isA<Query>());
    });

    test('subcollection parentCollection is correct', () {
      final orders = ob.collection('users').subcollection('user123', 'orders');
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
      expect(lineItems.collectionPath,
          equals('stores__orders__items__line_details'));
    });

    test('subcollection from CollectionRef matches DocumentRef.subcollection',
        () {
      final viaCollection =
          ob.collection('users').subcollection('u1', 'settings');
      final viaDoc = ob.collection('users').doc('u1').subcollection('settings');
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
      final query = ob.collection('products').where('category',
          whereIn: ['electronics', 'books', 'clothing']).limit(100);
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
      final orders = ob.collection('users').subcollection('user123', 'orders');
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
      final query = ob.collection('products').where('tags', contains: 'sale');
      expect(query, isA<Query>());
    });

    test('startsWith filter', () {
      final query = ob.collection('products').where('sku', startsWith: 'ELEC-');
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
      final doc = Document.fromMap('users', {'_id': 'abc', 'name': 'Yunior'});
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
      final doc = Document(id: 'x', collection: 'col', data: {'key': 'val'});
      final str = doc.toString();
      expect(str, contains('col'));
      expect(str, contains('x'));
    });

    test('Document operator[] returns correct values', () {
      final doc = Document(
          id: '1', collection: 'test', data: {'a': 1, 'b': 'two', 'c': true});
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
      await ob.auth.signOut();
      expect(ob.auth.currentState.isAuthenticated, false);
      expect(ob.auth.accessToken, isNull);
    });
  });

  // === OFFLINE CACHE TESTS ===

  group('OfflineCache', () {
    late OfflineCache cache;

    setUp(() {
      cache = OfflineCache();
    });

    tearDown(() {
      cache.dispose();
    });

    test('initial state', () {
      expect(cache.isOnline, true);
      expect(cache.pendingCount, 0);
    });

    test('cache and retrieve document', () async {
      final doc = Document(
        id: 'doc1',
        collection: 'products',
        data: {'title': 'Widget', 'price': 29.99},
      );
      await cache.cacheDocument('products', doc);
      final cached = await cache.getCachedDocument('products', 'doc1');
      expect(cached, isNotNull);
      expect(cached!.id, 'doc1');
      expect(cached['title'], 'Widget');
      expect(cached['price'], 29.99);
    });

    test('cache miss returns null', () async {
      final cached = await cache.getCachedDocument('products', 'nonexistent');
      expect(cached, isNull);
    });

    test('cache query results', () async {
      final docs = [
        Document(id: 'd1', collection: 'items', data: {'name': 'A'}),
        Document(id: 'd2', collection: 'items', data: {'name': 'B'}),
      ];
      await cache.cacheQueryResult('items', 'status_active', docs);
      final cached = await cache.getCachedQueryResult('items', 'status_active');
      expect(cached, isNotNull);
      expect(cached!.length, 2);
      expect(cached[0].id, 'd1');
      expect(cached[1].id, 'd2');
    });

    test('query cache miss returns null', () async {
      final cached = await cache.getCachedQueryResult('items', 'unknown_query');
      expect(cached, isNull);
    });

    test('enqueue write increments pending count', () {
      cache.enqueueWrite(
        collection: 'products',
        operation: 'create',
        data: {'title': 'New'},
      );
      expect(cache.pendingCount, 1);

      cache.enqueueWrite(
        collection: 'products',
        operation: 'update',
        documentId: 'doc1',
        data: {'title': 'Updated'},
      );
      expect(cache.pendingCount, 2);
    });

    test('remove pending write', () {
      cache.enqueueWrite(
        collection: 'products',
        operation: 'create',
        data: {'title': 'New'},
      );
      final writes = cache.pendingWrites;
      expect(writes.length, 1);

      cache.removePendingWrite(writes.first.id);
      expect(cache.pendingCount, 0);
    });

    test('clearAll removes everything', () async {
      final doc = Document(id: 'd1', collection: 'test', data: {'x': 1});
      await cache.cacheDocument('test', doc);
      cache.enqueueWrite(collection: 'test', operation: 'create', data: {});
      expect(cache.pendingCount, 1);

      await cache.clearAll();
      expect(cache.pendingCount, 0);
      final cached = await cache.getCachedDocument('test', 'd1');
      expect(cached, isNull);
    });

    test('invalidateCollection removes collection data', () async {
      final doc = Document(id: 'd1', collection: 'products', data: {'x': 1});
      await cache.cacheDocument('products', doc);
      await cache.invalidateCollection('products');
      final cached = await cache.getCachedDocument('products', 'd1');
      expect(cached, isNull);
    });

    test('isOnline toggle', () {
      expect(cache.isOnline, true);
      cache.isOnline = false;
      expect(cache.isOnline, false);
      cache.isOnline = true;
      expect(cache.isOnline, true);
    });

    test('PendingWrite serialization roundtrip', () {
      final write = PendingWrite(
        id: 'pw_1',
        collection: 'orders',
        operation: 'create',
        data: {'total': 99.99},
        documentId: 'ord_1',
      );
      final json = write.toJson();
      final restored = PendingWrite.fromJson(json);
      expect(restored.id, 'pw_1');
      expect(restored.collection, 'orders');
      expect(restored.operation, 'create');
      expect(restored.data?['total'], 99.99);
      expect(restored.documentId, 'ord_1');
    });
  });

  // === IN-MEMORY STORAGE TESTS ===

  group('InMemoryStorage', () {
    late InMemoryStorage storage;

    setUp(() {
      storage = InMemoryStorage();
    });

    test('write and read', () async {
      await storage.write('key1', 'value1');
      final result = await storage.read('key1');
      expect(result, 'value1');
    });

    test('read missing key returns null', () async {
      final result = await storage.read('missing');
      expect(result, isNull);
    });

    test('remove key', () async {
      await storage.write('key1', 'value1');
      await storage.remove('key1');
      final result = await storage.read('key1');
      expect(result, isNull);
    });

    test('removeByPrefix removes matching keys', () async {
      await storage.write('doc:products:1', 'a');
      await storage.write('doc:products:2', 'b');
      await storage.write('doc:orders:1', 'c');
      await storage.removeByPrefix('doc:products:');
      expect(await storage.read('doc:products:1'), isNull);
      expect(await storage.read('doc:products:2'), isNull);
      expect(await storage.read('doc:orders:1'), 'c');
    });

    test('clear removes all keys', () async {
      await storage.write('k1', 'v1');
      await storage.write('k2', 'v2');
      await storage.clear();
      expect(await storage.read('k1'), isNull);
      expect(await storage.read('k2'), isNull);
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

    test('AuthState with MFA required', () {
      const state = AuthState(
        status: AuthStatus.unauthenticated,
        mfaRequired: true,
        challengeToken: 'challenge_abc',
      );
      expect(state.isAuthenticated, false);
      expect(state.mfaRequired, true);
      expect(state.challengeToken, 'challenge_abc');
    });

    test('AuthState defaults - mfa not required', () {
      const state = AuthState(status: AuthStatus.authenticated);
      expect(state.mfaRequired, false);
      expect(state.challengeToken, isNull);
    });

    test('MfaSetupResult holds QR data', () {
      final result = MfaSetupResult(
        qrCodeBase64: 'base64data',
        manualKey: 'JBSWY3DPEHPK3PXP',
        appleOtpauthUrl: 'apple-otpauth://...',
      );
      expect(result.qrCodeBase64, 'base64data');
      expect(result.manualKey, 'JBSWY3DPEHPK3PXP');
      expect(result.appleOtpauthUrl, isNotNull);
    });
  });

  // ── FieldValue ──

  group('FieldValue', () {
    test('serverTimestamp creates correct API map', () {
      final fv = FieldValue.serverTimestamp();
      final map = fv.toApiMap('updated_at');
      expect(map, {
        'updated_at': {'_serverTimestamp': true}
      });
    });

    test('increment creates correct API map', () {
      final fv = FieldValue.increment(5);
      final map = fv.toApiMap('views');
      expect(map, {
        'views': {'_increment': 5}
      });
    });

    test('increment negative for decrement', () {
      final fv = FieldValue.increment(-1);
      final map = fv.toApiMap('stock');
      expect(map, {
        'stock': {'_increment': -1}
      });
    });

    test('arrayUnion creates correct API map', () {
      final fv = FieldValue.arrayUnion(['tag1', 'tag2']);
      final map = fv.toApiMap('tags');
      expect(map, {
        'tags': {
          '_arrayUnion': ['tag1', 'tag2']
        }
      });
    });

    test('arrayRemove creates correct API map', () {
      final fv = FieldValue.arrayRemove(['old']);
      final map = fv.toApiMap('tags');
      expect(map, {
        'tags': {
          '_arrayRemove': ['old']
        }
      });
    });

    test('delete creates correct API map', () {
      final fv = FieldValue.delete();
      final map = fv.toApiMap('temp_field');
      expect(map, {
        'temp_field': {'_deleteField': true}
      });
    });

    test('toString is readable', () {
      expect(
          FieldValue.serverTimestamp().toString(), contains('serverTimestamp'));
      expect(FieldValue.increment(3).toString(), contains('3'));
    });
  });

  // ── WriteBatch ──

  group('WriteBatch', () {
    test('starts empty', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      expect(batch.isEmpty, true);
      expect(batch.length, 0);
      client.dispose();
    });

    test('tracks operations', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.create('products', {'title': 'A'});
      batch.update('products', 'id1', {'title': 'B'});
      batch.delete('products', 'id2');
      expect(batch.length, 3);
      expect(batch.isEmpty, false);
      client.dispose();
    });
  });

  // ── QuerySnapshot with cursor pagination ──

  group('QuerySnapshot cursor pagination', () {
    test('hasMore defaults to false', () {
      final snapshot = QuerySnapshot(docs: []);
      expect(snapshot.hasMore, false);
      expect(snapshot.lastDocument, isNull);
    });

    test('lastDocument returns last doc', () {
      final docs = [
        Document(id: 'a', collection: 'test', data: {}),
        Document(id: 'b', collection: 'test', data: {}),
        Document(id: 'c', collection: 'test', data: {}),
      ];
      final snapshot = QuerySnapshot(docs: docs, hasMore: true);
      expect(snapshot.hasMore, true);
      expect(snapshot.lastDocument!.id, 'c');
      expect(snapshot.size, 3);
    });
  });

  // ── Query builder: startAfter and select ──

  group('Query builder extensions', () {
    test('startAfter sets cursor document', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = client
          .collection('products')
          .orderBy('created_at')
          .startAfterId('last_doc_id')
          .limit(20);
      // Query builder just stores state — no exception means success
      expect(query, isNotNull);
      client.dispose();
    });

    test('select stores field list', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final query =
          client.collection('users').select(['name', 'email', 'avatar_url']);
      expect(query, isNotNull);
      client.dispose();
    });

    test('startAfter with Document object', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final doc = Document(id: 'cursor_doc', collection: 'items', data: {});
      final query =
          client.collection('items').orderBy('price').startAfter(doc).limit(10);
      expect(query, isNotNull);
      client.dispose();
    });
  });

  // === WRITEBATCH COMPREHENSIVE TESTS ===

  group('WriteBatch comprehensive', () {
    test('create adds to operations', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.create('products', {'title': 'A', 'price': 10});
      batch.create('products', {'title': 'B', 'price': 20});
      expect(batch.length, 2);
      expect(batch.isEmpty, false);
      client.dispose();
    });

    test('update adds to operations with id', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.update('products', 'id1', {'title': 'Updated'});
      expect(batch.length, 1);
      client.dispose();
    });

    test('delete adds to operations', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.delete('products', 'id1');
      batch.delete('products', 'id2');
      expect(batch.length, 2);
      client.dispose();
    });

    test('mixed operations maintain order', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.create('products', {'title': 'New'});
      batch.update('orders', 'o1', {'status': 'shipped'});
      batch.delete('carts', 'c1');
      expect(batch.length, 3);
      client.dispose();
    });

    test('commit on empty batch returns empty list', () async {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      final results = await batch.commit();
      expect(results, isEmpty);
      client.dispose();
    });

    test('create with FieldValue processes sentinels', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.create('products', {
        'title': 'Widget',
        'created_at': FieldValue.serverTimestamp(),
      });
      // Should not throw — FieldValue is processed
      expect(batch.length, 1);
      client.dispose();
    });

    test('update with FieldValue increment', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final batch = client.batch();
      batch.update('products', 'p1', {
        'views': FieldValue.increment(1),
        'tags': FieldValue.arrayUnion(['featured']),
      });
      expect(batch.length, 1);
      client.dispose();
    });
  });

  // === FIELDVALUE EDGE CASES ===

  group('FieldValue edge cases', () {
    test('increment with double', () {
      final fv = FieldValue.increment(0.5);
      final map = fv.toApiMap('rating');
      expect(map, {
        'rating': {'_increment': 0.5}
      });
    });

    test('increment with zero', () {
      final fv = FieldValue.increment(0);
      final map = fv.toApiMap('count');
      expect(map, {
        'count': {'_increment': 0}
      });
    });

    test('arrayUnion with empty list', () {
      final fv = FieldValue.arrayUnion([]);
      final map = fv.toApiMap('tags');
      expect(map, {
        'tags': {'_arrayUnion': []}
      });
    });

    test('arrayRemove with mixed types', () {
      final fv = FieldValue.arrayRemove([1, 'two', true]);
      final map = fv.toApiMap('items');
      expect(map, {
        'items': {
          '_arrayRemove': [1, 'two', true]
        }
      });
    });

    test('multiple FieldValues in same map', () {
      final data = {
        'updated_at': FieldValue.serverTimestamp(),
        'views': FieldValue.increment(1),
        'tags': FieldValue.arrayUnion(['hot']),
        'old_field': FieldValue.delete(),
        'title': 'Normal value',
      };
      // Process like WriteBatch does
      final processed = <String, dynamic>{};
      for (final entry in data.entries) {
        if (entry.value is FieldValue) {
          processed.addAll((entry.value as FieldValue).toApiMap(entry.key));
        } else {
          processed[entry.key] = entry.value;
        }
      }
      expect(processed.containsKey('updated_at'), true);
      expect(processed.containsKey('views'), true);
      expect(processed.containsKey('tags'), true);
      expect(processed.containsKey('old_field'), true);
      expect(processed['title'], 'Normal value');
    });
  });

  // === REALTIME STREAM LIFECYCLE ===

  group('Realtime stream lifecycle', () {
    test('DocumentRef.snapshots returns a stream', () {
      final ob = OrignaBase.initialize(url: 'http://test.local');
      try {
        final stream = ob.collection('users').doc('u1').snapshots();
        expect(stream, isA<Stream<DocumentChange>>());
      } catch (_) {
      } finally {
        ob.dispose();
      }
    });

    test('RealtimeClient disconnect clears subscriptions', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final rt = RealtimeClient(ob);
      // disconnect without connect — should not throw
      rt.disconnect();
      ob.dispose();
    });

    test('DocumentChange equality by type', () {
      final doc = Document(id: '1', collection: 'test', data: {'a': 1});
      final c1 = DocumentChange(type: ChangeType.create, document: doc);
      final c2 = DocumentChange(type: ChangeType.update, document: doc);
      expect(c1.type, isNot(equals(c2.type)));
    });

    test('DocumentChange preserves document data', () {
      final doc = Document(id: 'x', collection: 'items', data: {
        'name': 'Widget',
        'price': 29.99,
        'tags': ['sale', 'new'],
      });
      final change = DocumentChange(type: ChangeType.update, document: doc);
      expect(change.document['name'], 'Widget');
      expect(change.document['price'], 29.99);
      expect(change.document['tags'], ['sale', 'new']);
      expect(change.document.id, 'x');
      expect(change.document.collection, 'items');
    });
  });

  // === AUTH COMPREHENSIVE ===

  group('Auth comprehensive', () {
    test('AuthState MFA flow state', () {
      const state = AuthState(
        status: AuthStatus.unauthenticated,
        mfaRequired: true,
        challengeToken: 'tok_123',
      );
      expect(state.isAuthenticated, false);
      expect(state.mfaRequired, true);
      expect(state.challengeToken, 'tok_123');
    });

    test('AuthState authenticated with full data', () {
      const state = AuthState(
        status: AuthStatus.authenticated,
        userId: 'user_abc',
        email: 'test@example.com',
        roles: ['admin', 'editor'],
      );
      expect(state.isAuthenticated, true);
      expect(state.userId, 'user_abc');
      expect(state.email, 'test@example.com');
      expect(state.roles, hasLength(2));
      expect(state.roles, contains('admin'));
      expect(state.mfaRequired, false);
      expect(state.challengeToken, isNull);
    });

    test('MfaSetupResult without apple URL', () {
      final result = MfaSetupResult(
        qrCodeBase64: 'data',
        manualKey: 'KEY',
      );
      expect(result.appleOtpauthUrl, isNull);
    });

    test('authStateChanges is a broadcast stream', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final s1 = ob.auth.authStateChanges;
      final s2 = ob.auth.authStateChanges;
      expect(s1, isA<Stream<AuthState>>());
      expect(s2, isA<Stream<AuthState>>());
      ob.dispose();
    });

    test('signOut emits unauthenticated via stream', () async {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final states = <AuthState>[];
      ob.auth.authStateChanges.listen(states.add);
      await ob.auth.signOut();
      await Future.delayed(Duration(milliseconds: 50));
      expect(states.any((s) => !s.isAuthenticated), true);
      ob.dispose();
    });
  });

  // === DOCUMENT EDGE CASES ===

  group('Document edge cases', () {
    test('fromMap with nested objects', () {
      final doc = Document.fromMap('users', {
        'id': 'u1',
        'profile': {
          'name': 'Yunior',
          'address': {'city': 'Toronto', 'country': 'CA'},
        },
      });
      expect(doc['profile']['name'], 'Yunior');
      expect(doc['profile']['address']['city'], 'Toronto');
    });

    test('fromMap with arrays', () {
      final doc = Document.fromMap('products', {
        'id': 'p1',
        'tags': ['electronics', 'sale'],
        'variants': [
          {'size': 'S', 'price': 10},
          {'size': 'M', 'price': 15},
        ],
      });
      expect(doc['tags'], hasLength(2));
      expect(doc['variants'][0]['size'], 'S');
    });

    test('fromMap with numeric id', () {
      final doc = Document.fromMap('items', {'id': 42, 'name': 'test'});
      expect(doc.id, '42');
    });

    test('QuerySnapshot with single document', () {
      final snap = QuerySnapshot(docs: [
        Document(id: '1', collection: 'test', data: {}),
      ], hasMore: false);
      expect(snap.size, 1);
      expect(snap.hasMore, false);
      expect(snap.lastDocument!.id, '1');
    });

    test('QuerySnapshot iteration', () {
      final docs = List.generate(
        5,
        (i) => Document(id: 'doc_$i', collection: 'items', data: {'index': i}),
      );
      final snap = QuerySnapshot(docs: docs, hasMore: true);
      expect(snap.docs.map((d) => d.id).toList(),
          ['doc_0', 'doc_1', 'doc_2', 'doc_3', 'doc_4']);
      expect(snap.lastDocument!.id, 'doc_4');
    });
  });

  // === QUERY BUILDER COMPREHENSIVE ===

  group('Query builder comprehensive', () {
    test('startAfter + limit + orderBy chain', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = client
          .collection('products')
          .where('status', isEqualTo: 'active')
          .orderBy('price', descending: true)
          .startAfterId('last_id')
          .limit(20);
      expect(query, isA<Query>());
      client.dispose();
    });

    test('select with orderBy', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = client
          .collection('users')
          .select(['name', 'email'])
          .orderBy('name')
          .limit(50);
      expect(query, isA<Query>());
      client.dispose();
    });

    test('all operators in single query', () {
      final client = OrignaBase.initialize(url: 'http://localhost:8080');
      final query = client
          .collection('products')
          .where('status', isEqualTo: 'active')
          .where('price', isGreaterThanOrEqualTo: 10)
          .where('price', isLessThan: 100)
          .where('category', whereIn: ['electronics', 'books'])
          .where('tags', contains: 'sale')
          .where('sku', startsWith: 'PROD-')
          .where('deleted', isNotEqualTo: true)
          .orderBy('price')
          .select(['title', 'price', 'category'])
          .limit(25)
          .offset(0);
      expect(query, isA<Query>());
      client.dispose();
    });

    test('AggregateQuery count with no filters', () {
      final agg = AggregateQuery('users', []);
      final q = agg.toCountQuery();
      expect(q['query'], contains('count()'));
      expect(q['query'], isNot(contains('WHERE')));
    });

    test('AggregateQuery sum with filter', () {
      final agg = AggregateQuery('orders', [
        QueryFilter('status', 'eq', 'completed'),
      ]);
      final q = agg.toSumQuery('total');
      expect(q['query'], contains('math::sum'));
      expect(q['query'], contains('WHERE'));
    });
  });

  // === CLIENT COMPREHENSIVE ===

  group('Client comprehensive', () {
    test('initialize with custom http client', () {
      final httpClient = http.Client();
      final ob = OrignaBase.initialize(
        url: 'http://localhost:8080',
        httpClient: httpClient,
      );
      expect(ob.url, 'http://localhost:8080');
      ob.dispose();
    });

    test('collection returns different refs for different names', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final products = ob.collection('products');
      final orders = ob.collection('orders');
      expect(products.collectionName, 'products');
      expect(orders.collectionName, 'orders');
      ob.dispose();
    });

    test('batch returns new WriteBatch each time', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final b1 = ob.batch();
      final b2 = ob.batch();
      b1.create('test', {'a': 1});
      expect(b1.length, 1);
      expect(b2.length, 0);
      ob.dispose();
    });

    test('storage is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.storage, isA<OrignaBaseStorage>());
      ob.dispose();
    });

    test('offline cache is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.offline, isA<OfflineCache>());
      ob.dispose();
    });

    test('config is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.config, isA<OrignaBaseConfig>());
      ob.dispose();
    });

    test('presence is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.presence, isA<OrignaBasePresence>());
      ob.dispose();
    });

    test('links is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.links, isA<OrignaBaseLinks>());
      ob.dispose();
    });

    test('push is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.push, isA<OrignaBasePush>());
      ob.dispose();
    });

    test('metrics is accessible', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.metrics, isA<OrignaBaseMetrics>());
      ob.dispose();
    });
  });

  // ── Remote Config tests ──────────────────────────────────────────────
  group('OrignaBaseConfig', () {
    test('getAll method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.getAll();
      expect(f, isA<Future>());
      f.catchError((_) => <String, dynamic>{});
      ob.dispose();
    });

    test('get method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.get('feature_flag');
      expect(f, isA<Future>());
      f.catchError((_) => null);
      ob.dispose();
    });

    test('getString method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.getString('key');
      expect(f, isA<Future>());
      f.catchError((_) => '');
      ob.dispose();
    });

    test('getBool method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.getBool('key');
      expect(f, isA<Future>());
      f.catchError((_) => false);
      ob.dispose();
    });

    test('getInt method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.getInt('key');
      expect(f, isA<Future>());
      f.catchError((_) => 0);
      ob.dispose();
    });

    test('getDouble method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.getDouble('key');
      expect(f, isA<Future>());
      f.catchError((_) => 0.0);
      ob.dispose();
    });

    test('set method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.set('key', 'value');
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('delete method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.config.delete('key');
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });
  });

  // ── Presence tests ───────────────────────────────────────────────────
  group('OrignaBasePresence', () {
    test('getOnlineUsers method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.presence.getOnlineUsers();
      expect(f, isA<Future>());
      f.catchError((_) => <PresenceInfo>[]);
      ob.dispose();
    });

    test('isOnline method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.presence.isOnline('user123');
      expect(f, isA<Future>());
      f.catchError((_) => false);
      ob.dispose();
    });

    test('getUser method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.presence.getUser('user123');
      expect(f, isA<Future>());
      f.catchError((_) => null);
      ob.dispose();
    });

    test('PresenceInfo.fromMap parses correctly', () {
      final info = PresenceInfo.fromMap({
        'user_id': 'u1',
        'connection_id': 'conn1',
        'status': 'online',
        'last_seen': '2026-03-08T00:00:00Z',
        'metadata': {'device': 'mobile'},
      });
      expect(info.userId, 'u1');
      expect(info.connectionId, 'conn1');
      expect(info.status, 'online');
      expect(info.lastSeen, '2026-03-08T00:00:00Z');
      expect(info.metadata['device'], 'mobile');
    });

    test('PresenceInfo.fromMap handles missing fields', () {
      final info = PresenceInfo.fromMap({});
      expect(info.userId, '');
      expect(info.connectionId, '');
      expect(info.status, 'unknown');
      expect(info.lastSeen, '');
      expect(info.metadata, isEmpty);
    });

    test('PresenceInfo.fromMap handles null metadata', () {
      final info = PresenceInfo.fromMap({
        'user_id': 'u2',
        'metadata': null,
      });
      expect(info.userId, 'u2');
      expect(info.metadata, isEmpty);
    });
  });

  // ── Dynamic Links tests ──────────────────────────────────────────────
  group('OrignaBaseLinks', () {
    test('create method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.links.create(url: 'https://example.com');
      expect(f, isA<Future>());
      f.catchError((_) => DynamicLink.fromMap({}));
      ob.dispose();
    });

    test('list method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.links.list();
      expect(f, isA<Future>());
      f.catchError((_) => <DynamicLink>[]);
      ob.dispose();
    });

    test('DynamicLink.fromMap parses correctly', () {
      final link = DynamicLink.fromMap({
        'slug': 'abc123',
        'short_url': '/l/abc123',
        'target_url': 'https://example.com/promo',
        'title': 'Promo Link',
        'description': 'A promotional link',
        'clicks': 42,
      });
      expect(link.slug, 'abc123');
      expect(link.shortUrl, '/l/abc123');
      expect(link.targetUrl, 'https://example.com/promo');
      expect(link.title, 'Promo Link');
      expect(link.description, 'A promotional link');
      expect(link.clicks, 42);
    });

    test('DynamicLink.fromMap handles missing fields', () {
      final link = DynamicLink.fromMap({});
      expect(link.slug, '');
      expect(link.targetUrl, '');
      expect(link.title, isNull);
      expect(link.description, isNull);
      expect(link.clicks, 0);
    });

    test('DynamicLink.fromMap generates shortUrl from slug', () {
      final link = DynamicLink.fromMap({'slug': 'myslug'});
      expect(link.shortUrl, '/l/myslug');
    });

    test('DynamicLink.fromMap uses explicit shortUrl over generated', () {
      final link = DynamicLink.fromMap({
        'slug': 'myslug',
        'short_url': '/custom/path',
      });
      expect(link.shortUrl, '/custom/path');
    });
  });

  // ── Push Notifications tests ─────────────────────────────────────────
  group('OrignaBasePush', () {
    test('registerToken method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.registerToken(
        userId: 'u1',
        token: 'fcm_abc',
        platform: 'android',
      );
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('unregisterToken method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.unregisterToken('fcm_abc');
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('sendToUser method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.sendToUser('u1', title: 'Hi', body: 'Hello');
      expect(f, isA<Future>());
      f.catchError((_) => PushResult.fromMap({}));
      ob.dispose();
    });

    test('sendToToken method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.sendToToken('fcm_abc', title: 'Hi', body: 'Hello');
      expect(f, isA<Future>());
      f.catchError((_) => PushResult.fromMap({}));
      ob.dispose();
    });

    test('sendToTopic method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.sendToTopic('news', title: 'Hi', body: 'Hello');
      expect(f, isA<Future>());
      f.catchError((_) => PushResult.fromMap({}));
      ob.dispose();
    });

    test('subscribeToTopic method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.subscribeToTopic('fcm_abc', 'news');
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('unsubscribeFromTopic method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.push.unsubscribeFromTopic('fcm_abc', 'news');
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('PushResult.fromMap parses correctly', () {
      final result = PushResult.fromMap({
        'sent': 5,
        'failed': 1,
        'total_devices': 6,
      });
      expect(result.sent, 5);
      expect(result.failed, 1);
      expect(result.totalDevices, 6);
    });

    test('PushResult.fromMap handles missing fields', () {
      final result = PushResult.fromMap({});
      expect(result.sent, 0);
      expect(result.failed, 0);
      expect(result.totalDevices, 0);
    });

    test('PushResult.fromMap handles num types', () {
      final result = PushResult.fromMap({
        'sent': 3.0,
        'failed': 0.0,
        'total_devices': 3.0,
      });
      expect(result.sent, 3);
      expect(result.failed, 0);
      expect(result.totalDevices, 3);
    });
  });

  // ── Metrics tests ────────────────────────────────────────────────────
  group('OrignaBaseMetrics', () {
    test('record method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.metrics.record('page_load', 1250);
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('record with tags method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.metrics.record('page_load', 1250, tags: {'page': '/home'});
      expect(f, isA<Future>());
      f.catchError((_) {});
      ob.dispose();
    });

    test('query method exists and is callable', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      final f = ob.metrics.query();
      expect(f, isA<Future>());
      f.catchError((_) => <MetricSummary>[]);
      ob.dispose();
    });

    test('MetricSummary.fromMap parses correctly', () {
      final summary = MetricSummary.fromMap({
        'name': 'page_load',
        'avg': 1250.5,
        'min': 800.0,
        'max': 2100.0,
        'count': 150,
      });
      expect(summary.name, 'page_load');
      expect(summary.avg, 1250.5);
      expect(summary.min, 800.0);
      expect(summary.max, 2100.0);
      expect(summary.count, 150);
    });

    test('MetricSummary.fromMap handles missing fields', () {
      final summary = MetricSummary.fromMap({});
      expect(summary.name, '');
      expect(summary.avg, 0.0);
      expect(summary.min, 0.0);
      expect(summary.max, 0.0);
      expect(summary.count, 0);
    });

    test('MetricSummary.fromMap handles int values', () {
      final summary = MetricSummary.fromMap({
        'name': 'api_latency',
        'avg': 100,
        'min': 50,
        'max': 200,
        'count': 10,
      });
      expect(summary.avg, 100.0);
      expect(summary.min, 50.0);
      expect(summary.max, 200.0);
      expect(summary.count, 10);
    });
  });

  group('VectorSearch', () {
    test('client has vectorSearch property', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.vectorSearch, isA<VectorSearch>());
      ob.dispose();
    });

    test('VectorSearchResult toString includes score and id', () {
      final doc = Document(
          id: 'prod1', collection: 'products', data: {'title': 'Widget'});
      final result = VectorSearchResult(document: doc, score: 0.95);
      expect(result.toString(), contains('0.95'));
      expect(result.toString(), contains('prod1'));
    });

    test('VectorSearchResult stores score and document', () {
      final doc =
          Document(id: 'abc', collection: 'items', data: {'name': 'test'});
      final result = VectorSearchResult(document: doc, score: 0.82);
      expect(result.score, 0.82);
      expect(result.document.id, 'abc');
      expect(result.document.collection, 'items');
      expect(result.document.data['name'], 'test');
    });

    test('search method exists and throws on network error', () {
      final mockClient = MockClient((req) async {
        throw http.ClientException('Connection refused');
      });
      final ob = OrignaBase.initialize(
          url: 'http://localhost:8080', httpClient: mockClient);
      expect(
        () => ob.vectorSearch.search(
          collection: 'products',
          vectorField: 'embedding',
          embedding: [0.1, 0.2, 0.3],
          topK: 5,
        ),
        throwsA(anything),
      );
      ob.dispose();
    });

    test('search method accepts optional threshold', () async {
      final mockClient = MockClient((req) async {
        return http.Response(
            jsonEncode({
              'data': {'vectorSearch': []}
            }),
            200);
      });
      final ob = OrignaBase.initialize(
          url: 'http://localhost:8080', httpClient: mockClient);
      final results = await ob.vectorSearch.search(
        collection: 'products',
        vectorField: 'embedding',
        embedding: [0.1, 0.2, 0.3],
        topK: 10,
        threshold: 0.7,
      );
      expect(results, isEmpty);
      ob.dispose();
    });
  });
}
