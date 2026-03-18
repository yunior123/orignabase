/// OrignaBase SDK — Complete Example
///
/// Demonstrates all patterns used in origna_gta (e-commerce app)
/// migrated from Firebase/Firestore to OrignaBase.
///
/// Covers: Auth, CRUD, queries, realtime, batch writes, transactions,
/// FieldValue ops, pagination, subcollections, storage, push, config.
library;

import 'dart:typed_data';
import 'package:orignabase/orignabase.dart';

/// Initialize the OrignaBase client
final ob = OrignaBase.initialize(url: 'http://localhost:8080');

// ─── AUTH ────────────────────────────────────────────────────────────────

Future<void> authExamples() async {
  // Email/password registration
  final state = await ob.auth.register('user@example.com', 'SecureP@ss1');
  print('Registered: ${state.isAuthenticated}');

  // Email/password sign in (with MFA support)
  final loginState =
      await ob.auth.signInWithEmail('user@example.com', 'SecureP@ss1');
  if (loginState.mfaRequired) {
    // MFA is enabled — complete the challenge
    final mfaState = await ob.auth.verifyMfaChallenge(
      loginState.challengeToken!,
      '123456', // TOTP code from authenticator
    );
    print('MFA verified: ${mfaState.isAuthenticated}');
  }

  // OAuth providers
  // await ob.auth.signInWithGoogle(googleIdToken);
  // await ob.auth.signInWithApple(appleAuthCode);

  // Anonymous auth (guest checkout)
  final anonState = await ob.auth.signInAnonymously();
  print('Anonymous user: ${anonState.userId}');

  // Upgrade anonymous to real account
  await ob.auth.upgradeAnonymous('real@email.com', 'NewPassword1!');

  // Magic link (passwordless)
  await ob.auth.sendMagicLink('user@example.com');
  // User clicks link → verifyMagicLink(token)

  // Email verification
  await ob.auth.sendEmailVerification();

  // Password reset
  await ob.auth.forgotPassword('user@example.com');

  // MFA setup
  final mfaSetup = await ob.auth.setupMfa();
  print('QR Code: ${mfaSetup.qrCodeBase64}');
  print('Manual Key: ${mfaSetup.manualKey}');
  final recoveryCodes = await ob.auth.verifyMfaSetup('123456');
  print('Recovery codes: $recoveryCodes');

  // Auth state stream (like Firebase's authStateChanges)
  ob.auth.authStateChanges.listen((state) {
    print('Auth state: ${state.status}');
  });

  // Sign out
  ob.auth.signOut();
}

// ─── PRODUCTS CRUD (equivalent to Firestore products collection) ─────

Future<void> productCrud() async {
  final products = ob.collection('products');

  // Create a product (like Firestore add)
  final newProduct = await products.add({
    'title': 'Premium Widget',
    'price': 49.99,
    'currency': 'CAD',
    'status': 'active',
    'stock': 100,
    'categories': ['electronics', 'gadgets'],
    'seller_id': 'seller_abc',
    'created_at': DateTime.now().toIso8601String(),
  });
  print('Created product: ${newProduct.id}');

  // Get a specific product (like Firestore doc.get)
  final doc = await products.doc(newProduct.id).get();
  print('Product title: ${doc?['title']}');

  // Update product (merge)
  await products.doc(newProduct.id).update({
    'price': 39.99,
    'on_sale': true,
  });

  // Delete product
  await products.doc(newProduct.id).delete();
}

// ─── COMPOUND QUERIES (origna_gta uses 4+ where + orderBy + limit) ───

Future<void> compoundQueries() async {
  final products = ob.collection('products');

  // Simple filter + sort + limit (like origna_gta product listing)
  final activeProducts = await products
      .where('status', isEqualTo: 'active')
      .orderBy('created_at', descending: true)
      .limit(20)
      .get();
  print('Active products: ${activeProducts.size}');

  // Multi-filter query (like origna_gta's seller dashboard)
  final sellerProducts = await products
      .where('seller_id', isEqualTo: 'seller_abc')
      .where('status', isEqualTo: 'active')
      .where('price', isGreaterThan: 10)
      .orderBy('price')
      .limit(50)
      .get();
  print('Seller products: ${sellerProducts.size}');

  // Price range query
  final affordableProducts = await products
      .where('price', isGreaterThanOrEqualTo: 10)
      .where('price', isLessThanOrEqualTo: 100)
      .orderBy('price')
      .get();
  print('Affordable: ${affordableProducts.size}');

  // Category filter (whereIn equivalent)
  final electronics = await products
      .where('categories', contains: 'electronics')
      .orderBy('created_at', descending: true)
      .limit(20)
      .get();
  print('Electronics: ${electronics.size}');

  // Search by prefix (like origna_gta's search bar)
  final searchResults = await products
      .where('title', startsWith: 'Premium')
      .limit(10)
      .get();
  print('Search results: ${searchResults.size}');

  // Field projection (only fetch needed fields — saves bandwidth)
  final nameOnly = await products
      .select(['title', 'price', 'status'])
      .where('status', isEqualTo: 'active')
      .limit(20)
      .get();
  print('Name-only results: ${nameOnly.size}');
}

// ─── CURSOR PAGINATION (Firestore startAfterDocument replacement) ────

Future<void> cursorPagination() async {
  final products = ob.collection('products');

  // First page
  final page1 = await products
      .orderBy('created_at', descending: true)
      .limit(20)
      .get();
  print('Page 1: ${page1.size} docs, hasMore: ${page1.hasMore}');

  // Next page (cursor-based — like Firestore's startAfterDocument)
  if (page1.hasMore) {
    final lastDoc = page1.lastDocument;
    if (lastDoc != null) {
      final page2 = await products
          .orderBy('created_at', descending: true)
          .startAfter(lastDoc)
          .limit(20)
          .get();
      print('Page 2: ${page2.size} docs');
    }
  }

  // Offset-based pagination (alternative)
  final page3 = await products
      .orderBy('created_at', descending: true)
      .offset(40)
      .limit(20)
      .get();
  print('Page 3: ${page3.size} docs');
}

// ─── REALTIME SUBSCRIPTIONS (Firestore .snapshots() replacement) ─────

Future<void> realtimeSubscriptions() async {
  // Collection-level realtime (like Firestore collection.snapshots)
  final products = ob.collection('products');
  // This requires a running realtime server (WebSocket)
  // products.where('status', isEqualTo: 'active').snapshots()...

  // Document-level realtime (like Firestore doc.snapshots)
  final docStream = products.doc('product_123').snapshots();
  final sub = docStream.listen((change) {
    print('Document changed: ${change.type}'); // created, updated, deleted
    print('New data: ${change.document.data}');
  });

  // Cancel subscription (prevents memory leak)
  await sub.cancel();
}

// ─── BATCH WRITES (Firestore WriteBatch replacement) ─────────────────

Future<void> batchWrites() async {
  // Atomic multi-document operations (like origna_gta's stock updates)
  final batch = ob.batch();

  // Create multiple documents
  batch.create('products', {'title': 'Widget A', 'price': 10});
  batch.create('products', {'title': 'Widget B', 'price': 20});
  batch.create('products', {'title': 'Widget C', 'price': 30});

  // Update existing documents
  batch.update('products', 'existing_id', {'status': 'sold'});

  // Delete documents
  batch.delete('products', 'old_id');

  // Commit all at once (like Firestore batch.commit)
  await batch.commit();
  print('Batch committed successfully');
}

// ─── FIELDVALUE OPERATIONS (Firestore FieldValue replacement) ────────

Future<void> fieldValueOps() async {
  final products = ob.collection('products');

  // Server timestamp (like FieldValue.serverTimestamp)
  await products.doc('prod1').update({
    'updated_at': FieldValue.serverTimestamp(),
  });

  // Increment (like FieldValue.increment — used for stock, ratings)
  await products.doc('prod1').update({
    'view_count': FieldValue.increment(1),
    'stock': FieldValue.increment(-1), // decrement
  });

  // Array union (like FieldValue.arrayUnion — used for tags, categories)
  await products.doc('prod1').update({
    'tags': FieldValue.arrayUnion(['new-tag', 'featured']),
  });

  // Array remove (like FieldValue.arrayRemove)
  await products.doc('prod1').update({
    'tags': FieldValue.arrayRemove(['old-tag']),
  });

  // Delete field (like FieldValue.delete)
  await products.doc('prod1').update({
    'deprecated_field': FieldValue.delete(),
  });
}

// ─── SUBCOLLECTIONS (Firestore subcollection pattern) ────────────────

Future<void> subcollections() async {
  // Order items as subcollection of orders (like origna_gta)
  final orderItems =
      ob.collection('orders').subcollection('order_abc', 'items');
  print('Subcollection path: ${orderItems.collectionPath}');

  // User's addresses
  final addresses =
      ob.collection('users').subcollection('user_123', 'addresses');
  await addresses.add({
    'street': '123 Main St',
    'city': 'Toronto',
    'postal_code': 'M5V 1A1',
  });

  // Product reviews
  final reviews =
      ob.collection('products').subcollection('prod_abc', 'reviews');
  await reviews.add({
    'rating': 5,
    'text': 'Great product!',
    'user_id': 'user_123',
  });

  // Nested subcollections (3 levels deep — like origna_gta)
  final itemVariants = ob
      .collection('users')
      .subcollection('user_123', 'orders')
      .subcollection('order_456', 'items');
  print('Nested path: ${itemVariants.collectionPath}');
}

// ─── STORAGE (Firebase Storage replacement) ──────────────────────────

Future<void> storageExamples() async {
  final storage = ob.storage;

  // Upload a file (requires Uint8List)
  final uploadResult = await storage.upload(
    'products/images/widget.png',
    Uint8List.fromList([0x89, 0x50, 0x4E, 0x47]),
    contentType: 'image/png',
  );
  print('Uploaded to: ${uploadResult['path']}');

  // Download a file
  final bytes = await storage.download('products/images/widget.png');
  print('Downloaded ${bytes.length} bytes');

  // Delete a file
  await storage.delete('products/images/old.png');
}

// ─── REMOTE CONFIG (Firebase Remote Config replacement) ──────────────

Future<void> remoteConfig() async {
  final config = ob.config;

  // Get typed config values (like Firebase Remote Config)
  final featureEnabled = await config.getBool('new_checkout_flow');
  final maxItems = await config.getInt('cart_max_items');
  final shippingRate = await config.getDouble('shipping_rate_cad');
  final welcomeMsg = await config.getString('welcome_message');
  print('Feature: $featureEnabled, Max: $maxItems, Rate: $shippingRate');
  print('Welcome: $welcomeMsg');

  // Get all config values
  final allConfig = await config.getAll();
  print('All config keys: ${allConfig.keys}');

  // Admin: set config values
  await config.set('maintenance_mode', false);
  await config.delete('deprecated_flag');
}

// ─── PUSH NOTIFICATIONS (FCM replacement) ────────────────────────────

Future<void> pushNotifications() async {
  final push = ob.push;

  // Register device token (like FCM token registration)
  await push.registerToken(
    userId: 'user_123',
    token: 'fcm_device_token_abc',
    platform: 'android',
  );

  // Send to specific user (all their devices)
  final result = await push.sendToUser(
    'user_123',
    title: 'Order Shipped!',
    body: 'Your order #12345 has been shipped.',
    data: {'order_id': '12345', 'type': 'order_update'},
  );
  print('Sent: ${result.sent}, Failed: ${result.failed}');

  // Send to topic (like FCM topics)
  await push.sendToTopic(
    'promotions',
    title: 'Flash Sale!',
    body: '50% off all electronics today.',
  );

  // Subscribe/unsubscribe from topics
  await push.subscribeToTopic('fcm_device_token_abc', 'promotions');
  await push.unsubscribeFromTopic('fcm_device_token_abc', 'old_topic');

  // Unregister device
  await push.unregisterToken('old_token');
}

// ─── PERFORMANCE METRICS (Firebase Performance replacement) ──────────

Future<void> performanceMetrics() async {
  final metrics = ob.metrics;

  // Record custom metrics (like Firebase Performance custom traces)
  final stopwatch = Stopwatch()..start();
  // ... do work ...
  stopwatch.stop();
  await metrics.record('page_load', stopwatch.elapsedMilliseconds, tags: {
    'page': '/products',
    'platform': 'android',
  });

  // Record API latency
  await metrics.record('api_call', 250, tags: {
    'endpoint': '/api/products',
    'method': 'GET',
  });

  // Query aggregated metrics (admin)
  final stats = await metrics.query();
  for (final stat in stats) {
    print('${stat.name}: avg=${stat.avg}ms, min=${stat.min}, max=${stat.max}, count=${stat.count}');
  }
}

// ─── PRESENCE (who's online) ─────────────────────────────────────────

Future<void> presenceTracking() async {
  final presence = ob.presence;

  // Check who's online
  final onlineUsers = await presence.getOnlineUsers();
  print('Online: ${onlineUsers.length} users');

  // Check specific user
  final isOnline = await presence.isOnline('user_123');
  print('User 123 is online: $isOnline');

  // Get detailed presence info
  final info = await presence.getUser('user_123');
  if (info != null) {
    print('Status: ${info.status}, Last seen: ${info.lastSeen}');
  }
}

// ─── DYNAMIC LINKS (Firebase Dynamic Links replacement) ──────────────

Future<void> dynamicLinks() async {
  final links = ob.links;

  // Create a short link (like Firebase Dynamic Links)
  final link = await links.create(
    url: 'https://orignabase.com/products/widget-123',
    slug: 'widget-promo',
    title: 'Amazing Widget',
    description: 'Check out this amazing widget!',
  );
  print('Short URL: ${link.shortUrl}');
  print('Clicks: ${link.clicks}');

  // List all links (admin)
  final allLinks = await links.list();
  for (final l in allLinks) {
    print('${l.slug} → ${l.targetUrl} (${l.clicks} clicks)');
  }
}

// ─── GRAPHQL (direct queries) ────────────────────────────────────────

Future<void> graphqlQueries() async {
  // Direct GraphQL for complex queries
  final response = await ob.graphql(
    'query { list(collection: "products", filters: "{\\"status\\": {\\"_eq\\": \\"active\\"}}", orderBy: "price", limit: 10) }',
  );
  print('GraphQL response: $response');

  // Full-text search
  final searchResults = await ob.search(
    'products_index',
    'wireless headphones',
    limit: 10,
  );
  print('Search hits: $searchResults');
}

// ─── OFFLINE SUPPORT ─────────────────────────────────────────────────

Future<void> offlineSupport() async {
  // Cache documents for offline use
  final doc = Document(id: 'u1', collection: 'users', data: {
    'name': 'Yunior',
    'role': 'admin',
  });
  await ob.offline.cacheDocument('users', doc);

  // Read from cache (works offline)
  final cached = await ob.offline.getCachedDocument('users', 'u1');
  print('Cached profile: ${cached?.data}');

  // Enqueue writes for later replay
  ob.offline.enqueueWrite(
    collection: 'orders',
    operation: 'create',
    data: {'total': 99.99, 'status': 'pending'},
  );
  print('Pending writes: ${ob.offline.pendingCount}');
}

// ─── MAIN ────────────────────────────────────────────────────────────

void main() async {
  print('OrignaBase SDK — Complete Example');
  print('Covers all Firebase services used by origna_gta\n');

  // In a real app, you'd call these based on user actions.
  // Here we list them for reference:
  //
  // await authExamples();
  // await productCrud();
  // await compoundQueries();
  // await cursorPagination();
  // await realtimeSubscriptions();
  // await batchWrites();
  // await fieldValueOps();
  // await subcollections();
  // await storageExamples();
  // await remoteConfig();
  // await pushNotifications();
  // await performanceMetrics();
  // await presenceTracking();
  // await dynamicLinks();
  // await graphqlQueries();
  // await offlineSupport();

  ob.dispose();
}
