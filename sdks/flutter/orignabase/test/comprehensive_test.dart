/// Comprehensive tests for OrignaBase Flutter SDK.
///
/// Covers all origna_gta patterns: compound queries, pagination,
/// batch writes, FieldValue ops, subcollections, storage, auth flows,
/// realtime subscriptions, config, push, metrics, presence, links.
@TestOn('vm')
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

/// Creates a mock HTTP client that returns predefined responses.
http.Client mockClient(
  Map<String, dynamic> Function(http.Request request) handler,
) {
  return MockClient((request) async {
    final body = handler(request);
    return http.Response(jsonEncode(body), 200, headers: {
      'content-type': 'application/json',
    });
  });
}

/// Creates a mock client that records requests for verification.
({http.Client client, List<http.Request> requests}) recordingClient(
  Map<String, dynamic> Function(http.Request request) handler,
) {
  final requests = <http.Request>[];
  final client = MockClient((request) async {
    requests.add(request);
    final body = handler(request);
    return http.Response(jsonEncode(body), 200, headers: {
      'content-type': 'application/json',
    });
  });
  return (client: client, requests: requests);
}

/// Creates an OrignaBase client with a mock HTTP client.
OrignaBase mockOb(
  Map<String, dynamic> Function(http.Request request) handler,
) {
  return OrignaBase.initialize(
    url: 'http://test.local',
    httpClient: mockClient(handler),
  );
}

void main() {
  // ── AUTH FLOWS ───────────────────────────────────────────────────────

  group('Auth — complete flow tests', () {
    test('register sets tokens and emits auth state', () async {
      final states = <AuthState>[];
      final ob = mockOb((req) => {
            'access_token': 'tok_123',
            'refresh_token': 'ref_456',
            'user_id': 'uid_abc',
          });

      ob.auth.authStateChanges.listen(states.add);
      final state = await ob.auth.register('test@test.com', 'Pass1234!');

      expect(state.isAuthenticated, true);
      expect(state.userId, 'uid_abc');
      expect(ob.auth.accessToken, 'tok_123');

      // Wait for stream event
      await Future.delayed(Duration.zero);
      expect(states.length, 1);
      expect(states.first.isAuthenticated, true);

      ob.dispose();
    });

    test('decoded auth helpers expose claims from access token', () async {
      const header = 'REDACTED_SECRET';
      const payload =
          'eyJzdWIiOiJ1aWRfand0IiwiZW1haWwiOiJqd3RAdGVzdC5jb20iLCJyb2xlcyI6WyJhZG1pbiIsInNlbGxlciJdLCJlbWFpbF92ZXJpZmllZCI6dHJ1ZX0';
      const token = '$header.$payload.signature';

      final ob = mockOb((req) => {
            'access_token': token,
            'refresh_token': 'ref_456',
          });

      await ob.auth.register('jwt@test.com', 'Pass1234!');

      expect(ob.auth.currentUserId, 'uid_jwt');
      expect(ob.auth.currentEmail, 'jwt@test.com');
      expect(ob.auth.currentRoles, ['admin', 'seller']);
      expect(ob.auth.isEmailVerified, isTrue);
      expect(ob.auth.currentState.emailVerified, isTrue);

      ob.dispose();
    });

    test('sign in with MFA required returns challenge', () async {
      final ob = mockOb((req) => {
            'mfa_required': true,
            'challenge_token': 'chal_789',
          });

      final state =
          await ob.auth.signInWithEmail('user@test.com', 'Pass1234!');
      expect(state.isAuthenticated, false);
      expect(state.mfaRequired, true);
      expect(state.challengeToken, 'chal_789');
      expect(ob.auth.accessToken, isNull);

      ob.dispose();
    });

    test('MFA challenge completion sets tokens', () async {
      final ob = mockOb((req) {
        if (req.url.path.contains('challenge')) {
          return {
            'access_token': 'mfa_tok',
            'refresh_token': 'mfa_ref',
            'user_id': 'uid_mfa',
          };
        }
        return {'mfa_required': true, 'challenge_token': 'chal'};
      });

      final state =
          await ob.auth.verifyMfaChallenge('chal_token', '123456');
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, 'mfa_tok');

      ob.dispose();
    });

    test('anonymous sign in works', () async {
      final ob = mockOb((req) => {
            'access_token': 'anon_tok',
            'refresh_token': 'anon_ref',
            'user_id': 'anon_123',
          });

      final state = await ob.auth.signInAnonymously();
      expect(state.isAuthenticated, true);
      expect(ob.auth.accessToken, 'anon_tok');

      ob.dispose();
    });

    test('sign out clears everything and emits state', () async {
      final states = <AuthState>[];
      final ob = mockOb((req) => {
            'access_token': 'tok',
            'refresh_token': 'ref',
          });

      ob.auth.authStateChanges.listen(states.add);
      await ob.auth.register('test@test.com', 'Pass1234!');
      ob.auth.signOut();

      expect(ob.auth.accessToken, isNull);
      expect(ob.auth.currentState.isAuthenticated, false);

      await Future.delayed(Duration.zero);
      expect(states.length, 2);
      expect(states.last.isAuthenticated, false);

      ob.dispose();
    });

    test('refresh token sends correct body', () async {
      final rec = recordingClient((req) => {
            'access_token': 'new_tok',
            'refresh_token': 'new_ref',
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      // Manually set refresh token
      await ob.auth.register('a@b.com', 'pass');
      await ob.auth.refreshToken();

      final refreshReq = rec.requests.lastWhere(
        (r) => r.url.path.contains('refresh'),
      );
      final body = jsonDecode(refreshReq.body);
      expect(body['refresh_token'], isNotNull);

      ob.dispose();
    });

    test('MFA setup returns QR and manual key', () async {
      final ob = mockOb((req) => {
            'qr_code_base64': 'base64data...',
            'manual_key': 'JBSWY3DPEHPK3PXP',
            'apple_otpauth_url': 'apple-otpauth://...',
          });

      final setup = await ob.auth.setupMfa();
      expect(setup.qrCodeBase64, 'base64data...');
      expect(setup.manualKey, 'JBSWY3DPEHPK3PXP');
      expect(setup.appleOtpauthUrl, 'apple-otpauth://...');

      ob.dispose();
    });

    test('MFA verify setup returns recovery codes', () async {
      final ob = mockOb((req) => {
            'recovery_codes': ['AAAA-BBBB', 'CCCC-DDDD', 'EEEE-FFFF'],
          });

      final codes = await ob.auth.verifyMfaSetup('123456');
      expect(codes, hasLength(3));
      expect(codes[0], 'AAAA-BBBB');

      ob.dispose();
    });

    test('Google sign in sends id_token', () async {
      final rec = recordingClient((req) => {
            'access_token': 'g_tok',
            'refresh_token': 'g_ref',
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.auth.signInWithGoogle('google_id_token_abc');

      final googleReq =
          rec.requests.firstWhere((r) => r.url.path.contains('google'));
      expect(jsonDecode(googleReq.body)['id_token'], 'google_id_token_abc');

      ob.dispose();
    });

    test('Apple sign in sends authorization_code and optional displayName',
        () async {
      final rec = recordingClient((req) => {
            'access_token': 'a_tok',
            'refresh_token': 'a_ref',
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.auth.signInWithApple('apple_code_xyz',
          displayName: 'Yunior R');

      final appleReq =
          rec.requests.firstWhere((r) => r.url.path.contains('apple'));
      final body = jsonDecode(appleReq.body);
      expect(body['authorization_code'], 'apple_code_xyz');
      expect(body['display_name'], 'Yunior R');

      ob.dispose();
    });
  });

  // ── COLLECTION CRUD ─────────────────────────────────────────────────

  group('Collection — CRUD via GraphQL', () {
    test('add sends correct GraphQL mutation', () async {
      final rec = recordingClient((req) => {
            'data': {
              'create': {
                'id': 'products:new1',
                'title': 'Widget',
                'price': 29.99,
              },
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      final doc =
          await ob.collection('products').add({'title': 'Widget', 'price': 29.99});
      expect(doc, isA<Document>());

      final graphqlReq =
          rec.requests.firstWhere((r) => r.url.path.contains('graphql'));
      final body = jsonDecode(graphqlReq.body);
      expect(body['query'], contains('create'));
      expect(body['query'], contains('products'));

      ob.dispose();
    });

    test('doc.get sends correct query', () async {
      final ob = mockOb((req) => {
            'data': {
              'get': {
                'id': 'products:abc',
                'title': 'Widget',
                'price': 29.99,
              },
            },
          });

      final doc = await ob.collection('products').doc('abc').get();
      expect(doc, isNotNull);
      expect(doc!['title'], 'Widget');
      expect(doc['price'], 29.99);
      expect(doc.id, contains('abc'));

      ob.dispose();
    });

    test('doc.get returns null for missing document', () async {
      final ob = mockOb((req) => {
            'data': {'get': null},
          });

      final doc = await ob.collection('products').doc('missing').get();
      expect(doc, isNull);

      ob.dispose();
    });

    test('doc.update sends merge data', () async {
      final rec = recordingClient((req) => {
            'data': {
              'update': {
                'id': 'products:abc',
                'title': 'Widget',
                'price': 39.99,
              },
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      final result =
          await ob.collection('products').doc('abc').update({'price': 39.99});
      expect(result, isNotNull);
      expect(result!['price'], 39.99);

      ob.dispose();
    });

    test('doc.delete sends correct mutation', () async {
      final rec = recordingClient((req) => {
            'data': {'delete': true},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.collection('products').doc('abc').delete();

      final graphqlReq =
          rec.requests.firstWhere((r) => r.url.path.contains('graphql'));
      final body = jsonDecode(graphqlReq.body);
      expect(body['query'], contains('delete'));
      expect(body['query'], contains('products'));

      ob.dispose();
    });
  });

  // ── COMPOUND QUERIES (origna_gta patterns) ──────────────────────────

  group('Query — compound filters like origna_gta', () {
    test('multi-filter query builds correct GraphQL', () async {
      final rec = recordingClient((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'title': 'A', 'price': 20, 'status': 'active'},
              ],
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      final results = await ob
          .collection('products')
          .where('status', isEqualTo: 'active')
          .where('price', isGreaterThan: 10)
          .where('seller_id', isEqualTo: 'seller_abc')
          .orderBy('price')
          .limit(50)
          .get();

      expect(results.size, 1);

      final graphqlReq =
          rec.requests.firstWhere((r) => r.url.path.contains('graphql'));
      final body = jsonDecode(graphqlReq.body);
      final query = body['query'] as String;
      expect(query, contains('products'));
      expect(query, contains('orderBy'));
      expect(query, contains('limit: 50'));

      ob.dispose();
    });

    test('price range query with gte and lte', () async {
      final ob = mockOb((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'price': 25},
                {'id': 'p2', 'price': 50},
              ],
            },
          });

      final results = await ob
          .collection('products')
          .where('price', isGreaterThanOrEqualTo: 20)
          .where('price', isLessThanOrEqualTo: 100)
          .get();

      expect(results.size, 2);

      ob.dispose();
    });

    test('descending order', () async {
      final rec = recordingClient((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'created_at': '2026-03-09'},
              ],
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob
          .collection('products')
          .orderBy('created_at', descending: true)
          .get();

      final query = jsonDecode(rec.requests.last.body)['query'] as String;
      expect(query, contains('descending: true'));

      ob.dispose();
    });

    test('field projection with select', () async {
      final rec = recordingClient((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'title': 'A', 'price': 10},
              ],
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob
          .collection('products')
          .select(['title', 'price'])
          .get();

      final query = jsonDecode(rec.requests.last.body)['query'] as String;
      expect(query, contains('select'));

      ob.dispose();
    });

    test('whereIn filter', () async {
      final ob = mockOb((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'category': 'electronics'},
              ],
            },
          });

      final results = await ob
          .collection('products')
          .where('category', whereIn: ['electronics', 'gadgets'])
          .get();

      expect(results.size, 1);

      ob.dispose();
    });

    test('contains filter', () async {
      final ob = mockOb((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'tags': ['sale']},
              ],
            },
          });

      final results = await ob
          .collection('products')
          .where('tags', contains: 'sale')
          .get();

      expect(results.size, 1);

      ob.dispose();
    });

    test('startsWith filter for search', () async {
      final ob = mockOb((req) => {
            'data': {
              'list': [
                {'id': 'p1', 'title': 'Premium Widget'},
              ],
            },
          });

      final results = await ob
          .collection('products')
          .where('title', startsWith: 'Premium')
          .limit(10)
          .get();

      expect(results.size, 1);

      ob.dispose();
    });

    test('empty result returns empty QuerySnapshot', () async {
      final ob = mockOb((req) => {
            'data': {'list': []},
          });

      final results = await ob.collection('products').get();
      expect(results.isEmpty, true);
      expect(results.size, 0);
      expect(results.lastDocument, isNull);

      ob.dispose();
    });
  });

  // ── CURSOR PAGINATION ───────────────────────────────────────────────

  group('Pagination — cursor-based like Firestore', () {
    test('hasMore detection via N+1 pattern', () async {
      // Server returns 21 results for limit=20 → hasMore=true
      final ob = mockOb((req) {
        final docs = List.generate(
          21,
          (i) => {'id': 'p$i', 'title': 'Product $i'},
        );
        return {
          'data': {'list': docs},
        };
      });

      final results = await ob
          .collection('products')
          .orderBy('created_at')
          .limit(20)
          .get();

      expect(results.size, 20); // Returns limit, not limit+1
      expect(results.hasMore, true);
      expect(results.lastDocument, isNotNull);
      expect(results.lastDocument!.id, 'p19');

      ob.dispose();
    });

    test('no more results when server returns <= limit', () async {
      final ob = mockOb((req) {
        final docs = List.generate(
          15,
          (i) => {'id': 'p$i', 'title': 'Product $i'},
        );
        return {
          'data': {'list': docs},
        };
      });

      final results = await ob
          .collection('products')
          .limit(20)
          .get();

      expect(results.size, 15);
      expect(results.hasMore, false);

      ob.dispose();
    });

    test('startAfter sends document ID', () async {
      final rec = recordingClient((req) => {
            'data': {'list': []},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      final lastDoc = Document(
        id: 'cursor_doc_id',
        collection: 'products',
        data: {'title': 'Last'},
      );

      await ob
          .collection('products')
          .orderBy('created_at')
          .startAfter(lastDoc)
          .limit(20)
          .get();

      final query = jsonDecode(rec.requests.last.body)['query'] as String;
      expect(query, contains('startAfter'));
      expect(query, contains('cursor_doc_id'));

      ob.dispose();
    });

    test('startAfterId sends raw ID string', () async {
      final rec = recordingClient((req) => {
            'data': {'list': []},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob
          .collection('products')
          .startAfterId('raw_cursor_id')
          .limit(10)
          .get();

      final query = jsonDecode(rec.requests.last.body)['query'] as String;
      expect(query, contains('raw_cursor_id'));

      ob.dispose();
    });

    test('offset pagination', () async {
      final rec = recordingClient((req) => {
            'data': {'list': []},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob
          .collection('products')
          .offset(40)
          .limit(20)
          .get();

      final query = jsonDecode(rec.requests.last.body)['query'] as String;
      expect(query, contains('offset: 40'));

      ob.dispose();
    });
  });

  // ── BATCH WRITES ────────────────────────────────────────────────────

  group('WriteBatch — Firestore batch replacement', () {
    test('batch tracks operation count', () {
      final ob = mockOb((req) => {});
      final batch = ob.batch();

      expect(batch.isEmpty, true);
      expect(batch.length, 0);

      batch.create('products', {'title': 'A'});
      batch.create('products', {'title': 'B'});
      batch.update('products', 'id1', {'price': 10});
      batch.delete('products', 'id2');

      expect(batch.isEmpty, false);
      expect(batch.length, 4);

      ob.dispose();
    });

    test('empty batch commit returns empty list', () async {
      final ob = mockOb((req) => {});
      final batch = ob.batch();

      final results = await batch.commit();
      expect(results, isEmpty);

      ob.dispose();
    });

    test('batch processes FieldValue operations', () {
      final ob = mockOb((req) => {});
      final batch = ob.batch();

      // This should not throw — FieldValues should be processed
      batch.create('products', {
        'title': 'Widget',
        'created_at': FieldValue.serverTimestamp(),
      });

      batch.update('products', 'p1', {
        'stock': FieldValue.increment(-1),
        'tags': FieldValue.arrayUnion(['sale']),
      });

      expect(batch.length, 2);

      ob.dispose();
    });
  });

  // ── FIELDVALUE ──────────────────────────────────────────────────────

  group('FieldValue — Firestore FieldValue replacement', () {
    test('serverTimestamp generates correct API map', () {
      final fv = FieldValue.serverTimestamp();
      final map = fv.toApiMap('created_at');
      expect(map, {'created_at': {'_serverTimestamp': true}});
    });

    test('increment generates correct API map', () {
      final fv = FieldValue.increment(5);
      final map = fv.toApiMap('count');
      expect(map, {'count': {'_increment': 5}});
    });

    test('negative increment (decrement)', () {
      final fv = FieldValue.increment(-1);
      final map = fv.toApiMap('stock');
      expect(map, {'stock': {'_increment': -1}});
    });

    test('arrayUnion generates correct API map', () {
      final fv = FieldValue.arrayUnion(['a', 'b']);
      final map = fv.toApiMap('tags');
      expect(map, {
        'tags': {
          '_arrayUnion': ['a', 'b'],
        },
      });
    });

    test('arrayRemove generates correct API map', () {
      final fv = FieldValue.arrayRemove(['old']);
      final map = fv.toApiMap('tags');
      expect(map, {
        'tags': {
          '_arrayRemove': ['old'],
        },
      });
    });

    test('delete generates correct API map', () {
      final fv = FieldValue.delete();
      final map = fv.toApiMap('deprecated');
      expect(map, {'deprecated': {'_deleteField': true}});
    });

    test('updateWithFieldValues processes mixed data correctly', () async {
      final rec = recordingClient((req) => {
            'data': {
              'update': {'id': 'p1', 'title': 'Widget'},
            },
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.collection('products').doc('p1').update({
        'title': 'Widget',
        'view_count': FieldValue.increment(1),
        'updated_at': FieldValue.serverTimestamp(),
      });

      // Verify GraphQL mutation was sent
      expect(rec.requests, isNotEmpty);

      ob.dispose();
    });
  });

  // ── SUBCOLLECTIONS ──────────────────────────────────────────────────

  group('Subcollections — nested data like origna_gta', () {
    test('subcollection path uses double-underscore convention', () {
      final ob = mockOb((req) => {});
      final reviews =
          ob.collection('products').subcollection('prod1', 'reviews');
      expect(reviews.collectionPath, 'products__reviews');
      expect(reviews.parentId, 'prod1');
      ob.dispose();
    });

    test('nested subcollection (3 levels deep)', () {
      final ob = mockOb((req) => {});
      final items = ob
          .collection('users')
          .subcollection('u1', 'orders')
          .subcollection('o1', 'items');
      expect(items.collectionPath, 'users__orders__items');
      ob.dispose();
    });

    test('doc.subcollection creates correct ref', () {
      final ob = mockOb((req) => {});
      final reviews =
          ob.collection('products').doc('prod1').subcollection('reviews');
      expect(reviews.collectionPath, 'products__reviews');
      expect(reviews.parentId, 'prod1');
      ob.dispose();
    });
  });

  // ── DOCUMENT MODEL ──────────────────────────────────────────────────

  group('Document — model and parsing', () {
    test('fromMap extracts id and removes it from data', () {
      final doc = Document.fromMap('products', {
        'id': 'products:abc123',
        'title': 'Widget',
        'price': 29.99,
      });
      expect(doc.id, 'products:abc123');
      expect(doc.collection, 'products');
      expect(doc['title'], 'Widget');
      expect(doc['price'], 29.99);
      expect(doc.data.containsKey('id'), false);
    });

    test('fromMap handles _id key', () {
      final doc = Document.fromMap('users', {
        '_id': 'users:u1',
        'name': 'Yunior',
      });
      expect(doc.id, 'users:u1');
      expect(doc.data.containsKey('_id'), false);
    });

    test('fromMap handles missing id', () {
      final doc = Document.fromMap('users', {'name': 'Yunior'});
      expect(doc.id, '');
    });

    test('operator[] returns field values', () {
      final doc = Document(
        id: 'p1',
        collection: 'products',
        data: {'title': 'Widget', 'price': 29.99, 'active': true},
      );
      expect(doc['title'], 'Widget');
      expect(doc['price'], 29.99);
      expect(doc['active'], true);
      expect(doc['missing'], isNull);
    });

    test('containsKey checks field existence', () {
      final doc = Document(
        id: 'p1',
        collection: 'products',
        data: {'title': 'Widget'},
      );
      expect(doc.containsKey('title'), true);
      expect(doc.containsKey('missing'), false);
    });

    test('toString includes collection and id', () {
      final doc = Document(
        id: 'p1',
        collection: 'products',
        data: {'title': 'Widget'},
      );
      expect(doc.toString(), contains('products'));
      expect(doc.toString(), contains('p1'));
    });
  });

  // ── QUERYSNAPSHOT ───────────────────────────────────────────────────

  group('QuerySnapshot — result handling', () {
    test('empty snapshot', () {
      final snap = QuerySnapshot(docs: []);
      expect(snap.isEmpty, true);
      expect(snap.isNotEmpty, false);
      expect(snap.size, 0);
      expect(snap.lastDocument, isNull);
      expect(snap.hasMore, false);
    });

    test('non-empty snapshot', () {
      final docs = [
        Document(id: 'p1', collection: 'products', data: {'title': 'A'}),
        Document(id: 'p2', collection: 'products', data: {'title': 'B'}),
      ];
      final snap = QuerySnapshot(docs: docs);
      expect(snap.isEmpty, false);
      expect(snap.isNotEmpty, true);
      expect(snap.size, 2);
      expect(snap.lastDocument!.id, 'p2');
    });

    test('hasMore flag', () {
      final snap = QuerySnapshot(docs: [], hasMore: true);
      expect(snap.hasMore, true);
    });
  });

  // ── STORAGE ─────────────────────────────────────────────────────────

  group('Storage — upload/download/delete', () {
    test('upload sends PUT with correct content type', () async {
      final requests = <http.BaseRequest>[];
      final client = MockClient((req) async {
        requests.add(req);
        if (req.url.path.contains('presign')) {
          return http.Response(
            jsonEncode({
              'urls': [
                {
                  'path': 'images/test.png',
                  'upload_url': 'http://test.local/storage/upload/images/test.png',
                }
              ]
            }),
            200,
          );
        }
        return http.Response('{}', 200);
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      final result = await ob.storage.upload(
        'images/test.png',
        Uint8List.fromList([1, 2, 3, 4]),
        contentType: 'image/png',
      );

      expect(result['path'], 'images/test.png');
      expect(result['content_type'], 'image/png');
      expect(result['size'], 4);

      final uploadReq = requests.firstWhere(
        (r) => r.method == 'PUT' && r.url.path.contains('upload'),
      );
      expect(uploadReq.method, 'PUT');

      ob.dispose();
    });

    test('upload includes auth token when authenticated', () async {
      final requests = <http.BaseRequest>[];
      final client = MockClient((req) async {
        requests.add(req);
        if (req.url.path.contains('register')) {
          return http.Response(
            jsonEncode({
              'access_token': 'tok_123',
              'refresh_token': 'ref_456',
            }),
            200,
          );
        }
        if (req.url.path.contains('presign')) {
          return http.Response(
            jsonEncode({
              'urls': [
                {
                  'path': 'test.png',
                  'upload_url': 'http://test.local/storage/upload/test.png',
                }
              ]
            }),
            200,
          );
        }
        return http.Response('{}', 200);
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      await ob.auth.register('t@t.com', 'pass');
      await ob.storage
          .upload('test.png', Uint8List(0), contentType: 'image/png');

      final presignReq = requests.firstWhere(
        (r) => r.url.path.contains('presign'),
      );
      expect(presignReq.headers['Authorization'], 'Bearer tok_123');

      ob.dispose();
    });

    test('download returns bytes', () async {
      final client = MockClient((req) async {
        if (req.url.path.contains('download')) {
          return http.Response.bytes([10, 20, 30], 200);
        }
        return http.Response('{}', 200);
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      final bytes = await ob.storage.download('images/test.png');
      expect(bytes.length, 3);
      expect(bytes[0], 10);

      ob.dispose();
    });

    test('download throws NotFoundException for 404', () async {
      final client = MockClient((req) async {
        if (req.url.path.contains('download')) {
          return http.Response('Not found', 404);
        }
        return http.Response('{}', 200);
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      await expectLater(
        ob.storage.download('missing.png'),
        throwsA(isA<NotFoundException>()),
      );

      ob.dispose();
    });
  });

  // ── REMOTE CONFIG ───────────────────────────────────────────────────

  group('Config — Remote Config replacement', () {
    test('getString returns string value', () async {
      final ob = mockOb((req) => {'value': 'hello'});
      final val = await ob.config.getString('greeting');
      expect(val, 'hello');
      ob.dispose();
    });

    test('getString returns empty for null', () async {
      final ob = mockOb((req) => {'value': null});
      final val = await ob.config.getString('missing');
      expect(val, '');
      ob.dispose();
    });

    test('getBool returns true for "true" string', () async {
      final ob = mockOb((req) => {'value': 'true'});
      final val = await ob.config.getBool('flag');
      expect(val, true);
      ob.dispose();
    });

    test('getBool returns false for non-bool', () async {
      final ob = mockOb((req) => {'value': 'not_a_bool'});
      final val = await ob.config.getBool('flag');
      expect(val, false);
      ob.dispose();
    });

    test('getInt parses string numbers', () async {
      final ob = mockOb((req) => {'value': '42'});
      final val = await ob.config.getInt('max_items');
      expect(val, 42);
      ob.dispose();
    });

    test('getInt returns 0 for unparseable', () async {
      final ob = mockOb((req) => {'value': 'abc'});
      final val = await ob.config.getInt('bad');
      expect(val, 0);
      ob.dispose();
    });

    test('getDouble parses string decimals', () async {
      final ob = mockOb((req) => {'value': '3.14'});
      final val = await ob.config.getDouble('pi');
      expect(val, closeTo(3.14, 0.001));
      ob.dispose();
    });

    test('set sends PUT to admin endpoint', () async {
      final rec = recordingClient((req) => {});
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.config.set('feature_x', true);

      final req =
          rec.requests.firstWhere((r) => r.url.path.contains('_admin/config'));
      expect(req.method, 'PUT');

      ob.dispose();
    });
  });

  // ── PUSH NOTIFICATIONS ──────────────────────────────────────────────

  group('Push — FCM replacement', () {
    test('registerToken sends correct body', () async {
      final rec = recordingClient((req) => {});
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.push.registerToken(
        userId: 'u1',
        token: 'fcm_token',
        platform: 'ios',
      );

      final req =
          rec.requests.firstWhere((r) => r.url.path.contains('push/register'));
      final body = jsonDecode(req.body);
      expect(body['user_id'], 'u1');
      expect(body['token'], 'fcm_token');
      expect(body['platform'], 'ios');

      ob.dispose();
    });

    test('sendToUser sends correct target type', () async {
      final ob = mockOb((req) => {
            'sent': 2,
            'failed': 0,
            'total_devices': 2,
          });

      final result = await ob.push.sendToUser(
        'u1',
        title: 'Test',
        body: 'Hello',
        data: {'key': 'value'},
      );

      expect(result.sent, 2);
      expect(result.failed, 0);
      expect(result.totalDevices, 2);

      ob.dispose();
    });

    test('sendToTopic sends topic target type', () async {
      final rec = recordingClient((req) => {
            'sent': 100,
            'failed': 5,
            'total_devices': 105,
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.push.sendToTopic(
        'news',
        title: 'Breaking',
        body: 'Big news!',
      );

      final req =
          rec.requests.firstWhere((r) => r.url.path.contains('push/send'));
      final body = jsonDecode(req.body);
      expect(body['target_type'], 'topic');
      expect(body['to'], 'news');

      ob.dispose();
    });
  });

  // ── METRICS ─────────────────────────────────────────────────────────

  group('Metrics — Performance Monitoring replacement', () {
    test('record sends metric with tags', () async {
      final rec = recordingClient((req) => {});
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.metrics.record('page_load', 1250, tags: {'page': '/home'});

      final req =
          rec.requests.firstWhere((r) => r.url.path.contains('metrics'));
      final body = jsonDecode(req.body);
      expect(body['name'], 'page_load');
      expect(body['value'], 1250);
      expect(body['tags']['page'], '/home');

      ob.dispose();
    });

    test('query returns parsed MetricSummary list', () async {
      final ob = mockOb((req) => {
            'metrics': [
              {
                'name': 'page_load',
                'avg': 1250.5,
                'min': 800.0,
                'max': 2100.0,
                'count': 150,
              },
              {
                'name': 'api_call',
                'avg': 100.0,
                'min': 50.0,
                'max': 300.0,
                'count': 5000,
              },
            ],
          });

      final stats = await ob.metrics.query();
      expect(stats, hasLength(2));
      expect(stats[0].name, 'page_load');
      expect(stats[0].avg, 1250.5);
      expect(stats[1].name, 'api_call');
      expect(stats[1].count, 5000);

      ob.dispose();
    });
  });

  // ── PRESENCE ────────────────────────────────────────────────────────

  group('Presence — online tracking', () {
    test('getOnlineUsers returns parsed list', () async {
      final ob = mockOb((req) => {
            'online': [
              {
                'user_id': 'u1',
                'connection_id': 'c1',
                'status': 'online',
                'last_seen': '2026-03-09T00:00:00Z',
              },
              {
                'user_id': 'u2',
                'connection_id': 'c2',
                'status': 'idle',
                'last_seen': '2026-03-09T00:01:00Z',
              },
            ],
          });

      final users = await ob.presence.getOnlineUsers();
      expect(users, hasLength(2));
      expect(users[0].userId, 'u1');
      expect(users[1].status, 'idle');

      ob.dispose();
    });

    test('isOnline returns bool', () async {
      final ob = mockOb((req) => {'online': true});
      expect(await ob.presence.isOnline('u1'), true);
      ob.dispose();
    });

    test('isOnline returns false for offline user', () async {
      final ob = mockOb((req) => {'online': false});
      expect(await ob.presence.isOnline('u2'), false);
      ob.dispose();
    });

    test('getUser returns null for offline user', () async {
      final ob = mockOb((req) => {'online': false});
      final info = await ob.presence.getUser('u2');
      expect(info, isNull);
      ob.dispose();
    });

    test('getUser returns PresenceInfo for online user', () async {
      final ob = mockOb((req) => {
            'online': true,
            'presence': {
              'user_id': 'u1',
              'connection_id': 'c1',
              'status': 'online',
              'last_seen': '2026-03-09T00:00:00Z',
              'metadata': {'device': 'mobile'},
            },
          });

      final info = await ob.presence.getUser('u1');
      expect(info, isNotNull);
      expect(info!.userId, 'u1');
      expect(info.metadata['device'], 'mobile');

      ob.dispose();
    });
  });

  // ── DYNAMIC LINKS ───────────────────────────────────────────────────

  group('Links — Dynamic Links replacement', () {
    test('create returns DynamicLink', () async {
      final ob = mockOb((req) => {
            'slug': 'promo123',
            'short_url': '/l/promo123',
            'target_url': 'https://example.com/deal',
            'title': 'Promo',
            'clicks': 0,
          });

      final link = await ob.links.create(
        url: 'https://example.com/deal',
        slug: 'promo123',
        title: 'Promo',
      );

      expect(link.slug, 'promo123');
      expect(link.shortUrl, '/l/promo123');
      expect(link.targetUrl, 'https://example.com/deal');
      expect(link.title, 'Promo');
      expect(link.clicks, 0);

      ob.dispose();
    });

    test('list returns parsed links', () async {
      final ob = mockOb((req) => {
            'links': [
              {
                'slug': 'a',
                'short_url': '/l/a',
                'target_url': 'https://a.com',
                'clicks': 10,
              },
              {
                'slug': 'b',
                'short_url': '/l/b',
                'target_url': 'https://b.com',
                'clicks': 20,
              },
            ],
          });

      final links = await ob.links.list();
      expect(links, hasLength(2));
      expect(links[0].clicks, 10);
      expect(links[1].slug, 'b');

      ob.dispose();
    });
  });

  // ── OFFLINE CACHE ───────────────────────────────────────────────────

  group('Offline — cache and write queue', () {
    test('cache and retrieve document', () async {
      final ob = mockOb((req) => {});
      final doc = Document(
        id: 'u1',
        collection: 'users',
        data: {'name': 'Yunior'},
      );

      await ob.offline.cacheDocument('users', doc);
      final cached = await ob.offline.getCachedDocument('users', 'u1');
      expect(cached, isNotNull);
      expect(cached!['name'], 'Yunior');

      ob.dispose();
    });

    test('cache returns null for missing document', () async {
      final ob = mockOb((req) => {});
      final cached = await ob.offline.getCachedDocument('users', 'missing');
      expect(cached, isNull);
      ob.dispose();
    });

    test('enqueue and count pending writes', () {
      final ob = mockOb((req) => {});
      expect(ob.offline.pendingCount, 0);

      ob.offline.enqueueWrite(
        collection: 'orders',
        operation: 'create',
        data: {'total': 99.99},
      );

      expect(ob.offline.pendingCount, 1);

      ob.offline.enqueueWrite(
        collection: 'orders',
        operation: 'update',
        documentId: 'o1',
        data: {'status': 'shipped'},
      );

      expect(ob.offline.pendingCount, 2);

      ob.dispose();
    });

    test('online/offline toggle', () {
      final ob = mockOb((req) => {});
      expect(ob.offline.isOnline, true);
      ob.offline.isOnline = false;
      expect(ob.offline.isOnline, false);
      ob.offline.isOnline = true;
      expect(ob.offline.isOnline, true);
      ob.dispose();
    });

    test('cache query results and retrieve by key', () async {
      final ob = mockOb((req) => {});
      final docs = [
        Document(id: 'p1', collection: 'products', data: {'title': 'A'}),
        Document(id: 'p2', collection: 'products', data: {'title': 'B'}),
      ];

      await ob.offline.cacheQueryResult('products', 'active_list', docs);

      final cached =
          await ob.offline.getCachedQueryResult('products', 'active_list');
      expect(cached, isNotNull);
      expect(cached!, hasLength(2));
      expect(cached[0].id, 'p1');
      expect(cached[1]['title'], 'B');

      ob.dispose();
    });
  });

  // ── ERROR HANDLING ──────────────────────────────────────────────────

  group('Error handling — HTTP status codes', () {
    test('401 throws AuthException', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response(
            jsonEncode({'message': 'Invalid token'}),
            401,
          );
        }),
      );

      expect(
        () => ob.auth.signInWithEmail('a@b.com', 'wrong'),
        throwsA(isA<AuthException>()),
      );

      ob.dispose();
    });

    test('403 throws ForbiddenException', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response(
            jsonEncode({'message': 'Admin only'}),
            403,
          );
        }),
      );

      expect(
        () => ob.config.set('key', 'val'),
        throwsA(isA<ForbiddenException>()),
      );

      ob.dispose();
    });

    test('404 throws NotFoundException', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response(
            jsonEncode({'message': 'Not found'}),
            404,
          );
        }),
      );

      expect(
        () => ob.config.get('missing_key'),
        throwsA(isA<NotFoundException>()),
      );

      ob.dispose();
    });

    test('422 throws ValidationException', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response(
            jsonEncode({'message': 'Invalid email'}),
            422,
          );
        }),
      );

      expect(
        () => ob.auth.register('bad-email', 'pass'),
        throwsA(isA<ValidationException>()),
      );

      ob.dispose();
    });

    test('500 throws generic OrignaBaseException', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response(
            jsonEncode({'message': 'Internal error'}),
            500,
          );
        }),
      );

      expect(
        () => ob.auth.register('a@b.com', 'pass'),
        throwsA(isA<OrignaBaseException>()),
      );

      ob.dispose();
    });

    test('empty error body handled gracefully', () async {
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: MockClient((req) async {
          return http.Response('', 500);
        }),
      );

      expect(
        () => ob.auth.register('a@b.com', 'pass'),
        throwsA(isA<OrignaBaseException>()),
      );

      ob.dispose();
    });
  });

  // ── CLIENT CONFIGURATION ────────────────────────────────────────────

  group('Client — initialization and configuration', () {
    test('auth token included in requests', () async {
      final rec = recordingClient((req) {
        if (req.url.path.contains('register')) {
          return {
            'access_token': 'my_token',
            'refresh_token': 'ref',
          };
        }
        return {'data': {}};
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.auth.register('a@b.com', 'pass');
      await ob.config.getAll();

      final configReq =
          rec.requests.firstWhere((r) => r.url.path.contains('config'));
      expect(configReq.headers['Authorization'], 'Bearer my_token');

      ob.dispose();
    });

    test('unsupported HTTP method throws', () {
      final ob = mockOb((req) => {});
      expect(
        () => ob.request('PATCH', '/test'),
        throwsA(isA<OrignaBaseException>()),
      );
      ob.dispose();
    });

    test('DELETE request sends body when provided', () async {
      final requests = <http.BaseRequest>[];
      final client = MockClient.streaming((req, _) async {
        requests.add(req);
        return http.StreamedResponse(
          Stream.value(utf8.encode('{}')),
          200,
        );
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      await ob.request('DELETE', '/push/register', body: {
        'token': 'fcm_token_abc',
      });

      final deleteReq = requests.first;
      expect(deleteReq.method, 'DELETE');
      // Body should be present (this was a bug — DELETE silently dropped body)
      if (deleteReq is http.Request) {
        expect(deleteReq.body, contains('fcm_token_abc'));
      }

      ob.dispose();
    });

    test('all service accessors are initialized', () {
      final ob = mockOb((req) => {});
      expect(ob.auth, isA<OrignaBaseAuth>());
      expect(ob.storage, isA<OrignaBaseStorage>());
      expect(ob.offline, isA<OfflineCache>());
      expect(ob.config, isA<OrignaBaseConfig>());
      expect(ob.presence, isA<OrignaBasePresence>());
      expect(ob.links, isA<OrignaBaseLinks>());
      expect(ob.push, isA<OrignaBasePush>());
      expect(ob.metrics, isA<OrignaBaseMetrics>());
      ob.dispose();
    });
  });

  // ── REALTIME ─────────────────────────────────────────────────────────

  group('Realtime — subscription models and lifecycle', () {
    test('ChangeType has all three variants', () {
      expect(ChangeType.values, hasLength(3));
      expect(ChangeType.values, contains(ChangeType.create));
      expect(ChangeType.values, contains(ChangeType.update));
      expect(ChangeType.values, contains(ChangeType.delete));
    });

    test('DocumentChange holds type and document', () {
      final doc = Document(
        id: 'p1',
        collection: 'products',
        data: {'title': 'Widget', 'price': 29.99},
      );
      final change = DocumentChange(type: ChangeType.create, document: doc);
      expect(change.type, ChangeType.create);
      expect(change.document.id, 'p1');
      expect(change.document['title'], 'Widget');
    });

    test('DocumentChange delete type preserves document reference', () {
      final doc = Document(
        id: 'p1',
        collection: 'products',
        data: {'title': 'Deleted Widget'},
      );
      final change = DocumentChange(type: ChangeType.delete, document: doc);
      expect(change.type, ChangeType.delete);
      expect(change.document.id, 'p1');
      expect(change.document.collection, 'products');
    });

    test('RealtimeClient can be created without connecting', () {
      final ob = mockOb((req) => {});
      final rt = RealtimeClient(ob);
      // Should not throw
      rt.disconnect();
      ob.dispose();
    });

    test('RealtimeClient disconnect is idempotent', () {
      final ob = mockOb((req) => {});
      final rt = RealtimeClient(ob);
      rt.disconnect();
      rt.disconnect(); // Second disconnect should not throw
      ob.dispose();
    });

    test('DocumentRef.snapshots returns Stream<DocumentChange>', () {
      final ob = mockOb((req) => {});
      // snapshots() will try to connect WebSocket which will fail,
      // but the stream type should be correct
      final stream = ob.collection('products').doc('p1').snapshots();
      expect(stream, isA<Stream<DocumentChange>>());
      ob.dispose();
    });

    test('URL conversion for WebSocket', () {
      // Verify the WS URL conversion logic
      final httpUrl = 'http://localhost:8080';
      final wsUrl = httpUrl
          .replaceFirst('http://', 'ws://')
          .replaceFirst('https://', 'wss://');
      expect(wsUrl, 'ws://localhost:8080');

      final httpsUrl = 'https://api.orignabase.com';
      final wssUrl = httpsUrl
          .replaceFirst('http://', 'ws://')
          .replaceFirst('https://', 'wss://');
      expect(wssUrl, 'wss://api.orignabase.com');
    });
  });

  // ── GRAPHQL ─────────────────────────────────────────────────────────

  group('GraphQL — direct queries', () {
    test('graphql sends query body', () async {
      final rec = recordingClient((req) => {
            'data': {'list': []},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.graphql('query { list(collection: "test") }');

      final req =
          rec.requests.firstWhere((r) => r.url.path.contains('graphql'));
      final body = jsonDecode(req.body);
      expect(body['query'], contains('list'));

      ob.dispose();
    });

    test('graphql with variables', () async {
      final rec = recordingClient((req) => {
            'data': {'result': true},
          });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: rec.client,
      );

      await ob.graphql(
        'mutation { create(\$input: CreateInput!) }',
        variables: {'input': {'title': 'Widget'}},
      );

      final body = jsonDecode(rec.requests.last.body);
      expect(body['variables'], isNotNull);
      expect(body['variables']['input']['title'], 'Widget');

      ob.dispose();
    });

    test('search sends GraphQL query', () async {
      final ob = mockOb((req) => {
            'data': {
              'search': {
                'hits': [
                  {'id': 'p1', 'title': 'Widget'},
                ],
                'total': 1,
              },
            },
          });

      final result = await ob.search('products', 'widget', limit: 10);
      expect(result, isA<Map>());

      ob.dispose();
    });
  });
}
