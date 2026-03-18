/// Batch operations example for OrignaBase SDK.
///
/// Demonstrates: batch create, update, delete, mixed operations,
/// and bulk data import patterns.
library;

import 'package:orignabase/orignabase.dart';

Future<void> main() async {
  final ob = OrignaBase.initialize(url: 'http://localhost:8080');
  await ob.auth.register('batch@example.com', 'BatchPass123!');

  final collection = 'batch_demo';

  // ── Batch Create ───────────────────────────────────────────────────
  print('=== Batch Create ===');
  final createBatch = ob.batch();
  final ids = <String>[];

  for (var i = 0; i < 20; i++) {
    createBatch.create(collection, {
      'index': i,
      'name': 'Item $i',
      'category': i % 3 == 0 ? 'A' : (i % 3 == 1 ? 'B' : 'C'),
      'value': i * 10,
      'active': true,
    });
  }
  await createBatch.commit();
  print('Created 20 documents');

  // Read back to get IDs
  final allDocs = await ob.collection(collection).limit(20).get();
  for (final doc in allDocs.docs) {
    ids.add(doc.id);
  }
  print('Retrieved ${ids.length} IDs');

  // ── Batch Update ───────────────────────────────────────────────────
  print('\n=== Batch Update ===');
  final updateBatch = ob.batch();

  // Mark first 10 as inactive
  for (var i = 0; i < 10 && i < ids.length; i++) {
    updateBatch.update(collection, ids[i], {
      'active': false,
      'deactivatedAt': FieldValue.serverTimestamp(),
    });
  }
  await updateBatch.commit();
  print('Deactivated first 10 items');

  // Verify
  final inactive = await ob
      .collection(collection)
      .where('active', isEqualTo: false)
      .get();
  print('Inactive count: ${inactive.docs.length}');

  // ── Batch Delete ───────────────────────────────────────────────────
  print('\n=== Batch Delete ===');
  final deleteBatch = ob.batch();

  // Delete items 15-19
  for (var i = 15; i < ids.length; i++) {
    deleteBatch.delete(collection, ids[i]);
  }
  await deleteBatch.commit();
  print('Deleted items 15-19');

  final remaining = await ob.collection(collection).get();
  print('Remaining: ${remaining.docs.length}');

  // ── Mixed Batch ────────────────────────────────────────────────────
  print('\n=== Mixed Batch (create + update + delete) ===');
  final mixedBatch = ob.batch();

  // Create a new item
  mixedBatch.create(collection, {
    'index': 100,
    'name': 'New Item',
    'category': 'D',
    'value': 1000,
    'active': true,
  });

  // Update an existing item
  if (ids.isNotEmpty) {
    mixedBatch.update(collection, ids[0], {
      'name': 'Updated First Item',
      'value': FieldValue.increment(999),
    });
  }

  // Delete another
  if (ids.length > 1) {
    mixedBatch.delete(collection, ids[1]);
  }

  await mixedBatch.commit();
  print('Mixed batch committed');

  // ── Bulk Import Pattern ────────────────────────────────────────────
  print('\n=== Bulk Import (chunked) ===');
  const totalItems = 100;
  const chunkSize = 25;

  for (var offset = 0; offset < totalItems; offset += chunkSize) {
    final chunk = ob.batch();
    final end = (offset + chunkSize).clamp(0, totalItems);

    for (var i = offset; i < end; i++) {
      chunk.create('bulk_import', {
        'index': i,
        'data': 'Record #$i',
        'importedAt': FieldValue.serverTimestamp(),
      });
    }

    await chunk.commit();
    print('  Imported ${offset + chunkSize} / $totalItems');
  }

  final imported = await ob.collection('bulk_import').get();
  print('Total imported: ${imported.docs.length}');

  // ── FieldValue in Batches ──────────────────────────────────────────
  print('\n=== FieldValue Batch Operations ===');
  final fvBatch = ob.batch();

  // Increment multiple counters atomically
  for (var i = 0; i < 5 && i < remaining.docs.length; i++) {
    fvBatch.update(collection, remaining.docs[i].id, {
      'value': FieldValue.increment(100),
    });
  }
  await fvBatch.commit();
  print('Incremented 5 counters by 100');

  // ── Cleanup ────────────────────────────────────────────────────────
  ob.dispose();
  print('\nDone!');
}
