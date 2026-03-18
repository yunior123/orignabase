/// E-commerce workflow example matching origna_gta patterns.
///
/// Demonstrates: product CRUD, cart operations, checkout flow,
/// favorites, stock management, and order lifecycle.
library;

import 'package:orignabase/orignabase.dart';

Future<void> main() async {
  final ob = OrignaBase.initialize(url: 'http://localhost:8080');

  // Register a test user
  await ob.auth.register('shopper@origna.example', 'ShopPass123!');

  // ── Product Catalog ────────────────────────────────────────────────
  print('=== Creating Products ===');

  final batch = ob.batch();
  batch.create('products', {
    'title': 'Vintage T-Shirt',
    'description': 'Classic cotton tee',
    'priceCents': 2999,
    'category': 'clothing',
    'stock': 50,
    'tags': ['sale', 'new'],
    'sellerId': 'seller_001',
    'active': true,
  });
  batch.create('products', {
    'title': 'Running Shoes',
    'description': 'Lightweight performance shoes',
    'priceCents': 12999,
    'category': 'footwear',
    'stock': 20,
    'tags': ['premium'],
    'sellerId': 'seller_001',
    'active': true,
  });
  batch.create('products', {
    'title': 'Baseball Cap',
    'description': 'Adjustable cotton cap',
    'priceCents': 1999,
    'category': 'accessories',
    'stock': 100,
    'tags': ['sale', 'clearance'],
    'sellerId': 'seller_002',
    'active': true,
  });
  await batch.commit();
  print('3 products created');

  // ── Browse & Search ────────────────────────────────────────────────
  print('\n=== Browsing Products ===');

  // All products
  final all = await ob.collection('products').get();
  print('Total products: ${all.docs.length}');

  // Filter by category
  final clothing = await ob
      .collection('products')
      .where('category', isEqualTo: 'clothing')
      .get();
  print('Clothing items: ${clothing.docs.length}');

  // Price range
  final affordable = await ob
      .collection('products')
      .where('priceCents', isLessThan: 5000)
      .orderBy('priceCents')
      .get();
  print('Under \$50: ${affordable.docs.length}');

  // Sale items
  final onSale = await ob
      .collection('products')
      .where('tags', contains: 'sale')
      .get();
  print('On sale: ${onSale.docs.length}');

  // ── Favorites / Wishlist ───────────────────────────────────────────
  print('\n=== Favorites ===');
  final userId = ob.auth.currentUserId ?? 'user_1';
  final favDoc = ob.collection('user_favorites').doc(userId);

  // Add to favorites
  await favDoc.set({'productIds': []});
  await favDoc.update({
    'productIds': FieldValue.arrayUnion([all.docs.first.id]),
  });
  print('Added ${all.docs.first.id} to favorites');

  // Remove from favorites
  await favDoc.update({
    'productIds': FieldValue.arrayRemove([all.docs.first.id]),
  });
  print('Removed from favorites');

  // ── Cart Operations ────────────────────────────────────────────────
  print('\n=== Cart ===');
  final cartCollection = 'carts';
  final cartDoc = ob.collection(cartCollection).doc(userId);

  // Create cart
  await cartDoc.set({
    'items': [],
    'totalCents': 0,
    'updatedAt': FieldValue.serverTimestamp(),
  });

  // Add item to cart (using subcollection pattern)
  final cartItems = ob.collection(cartCollection).subcollection(userId, 'items');
  final cartItem = await cartItems.add({
    'productId': all.docs.first.id,
    'title': all.docs.first.data['title'],
    'priceCents': all.docs.first.data['priceCents'],
    'quantity': 2,
  });
  print('Added to cart: ${all.docs.first.data['title']} x2');

  // Update quantity
  await cartItems.doc(cartItem.id).update({'quantity': 3});
  print('Updated quantity to 3');

  // ── Stock Management ───────────────────────────────────────────────
  print('\n=== Stock Update ===');

  // Decrement stock atomically
  final productRef = ob.collection('products').doc(all.docs.first.id);
  await productRef.update({
    'stock': FieldValue.increment(-3), // sold 3 items
  });
  print('Stock decremented by 3');

  // Check stock
  final updated = await productRef.get();
  print('Remaining stock: ${updated?.data['stock']}');

  // ── Order Lifecycle ────────────────────────────────────────────────
  print('\n=== Creating Order ===');

  final order = await ob.collection('orders').add({
    'userId': userId,
    'items': [
      {
        'productId': all.docs.first.id,
        'title': all.docs.first.data['title'],
        'priceCents': all.docs.first.data['priceCents'],
        'quantity': 3,
      }
    ],
    'totalCents': (all.docs.first.data['priceCents'] as int) * 3,
    'status': 'pending',
    'shippingAddress': {
      'street': '123 King St W',
      'city': 'Toronto',
      'province': 'ON',
      'postalCode': 'M5V 1A1',
      'country': 'CA',
    },
    'createdAt': FieldValue.serverTimestamp(),
  });
  print('Order created: ${order.id}');

  // Update order status
  await ob.collection('orders').doc(order.id).update({
    'status': 'confirmed',
    'confirmedAt': FieldValue.serverTimestamp(),
  });
  print('Order confirmed');

  await ob.collection('orders').doc(order.id).update({
    'status': 'shipped',
    'trackingNumber': 'CP123456789CA',
    'shippedAt': FieldValue.serverTimestamp(),
  });
  print('Order shipped');

  // ── Order History ──────────────────────────────────────────────────
  print('\n=== Order History ===');
  final orders = await ob
      .collection('orders')
      .where('userId', isEqualTo: userId)
      .orderBy('createdAt', descending: true)
      .limit(10)
      .get();
  print('Total orders: ${orders.docs.length}');

  for (final o in orders.docs) {
    print('  Order ${o.id}: ${o.data['status']}');
  }

  // ── Cleanup ────────────────────────────────────────────────────────
  ob.dispose();
  print('\nDone!');
}
