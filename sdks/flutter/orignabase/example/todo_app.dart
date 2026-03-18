/// OrignaBase Todo App — Console Example
///
/// Demonstrates ALL OrignaBase SDK features against a running server.
/// Run with: dart run example/todo_app.dart
///
/// Prerequisites: OrignaBase server running at http://localhost:8080
import 'package:orignabase/orignabase.dart';

const serverUrl = 'http://localhost:8080';

void main() async {
  final timestamp = DateTime.now().millisecondsSinceEpoch;
  final email = 'todo_user_$timestamp@example.com';
  final password = 'SecurePass123!';
  final collectionName = 'todos_$timestamp'; // unique per run to avoid collisions

  print('=== OrignaBase Todo App ===\n');

  // ──────────────────────────────────────────────
  // 1. Initialize the OrignaBase client
  // ──────────────────────────────────────────────
  print('[1] Initializing OrignaBase client...');
  final ob = OrignaBase.initialize(url: serverUrl);
  print('    Connected to $serverUrl\n');

  try {
    // ──────────────────────────────────────────────
    // 2. Register a new user
    // ──────────────────────────────────────────────
    print('[2] Registering user: $email');
    final authState = await ob.auth.register(email, password);
    print('    Registered! Authenticated: ${authState.isAuthenticated}');
    print('    User ID: ${authState.userId}\n');

    // ──────────────────────────────────────────────
    // 3. CRUD — Create todo items
    // ──────────────────────────────────────────────
    print('[3] Creating todo items...');
    final todos = ob.collection(collectionName);

    final todo1 = await todos.add({
      'title': 'Buy groceries',
      'completed': false,
      'priority': 1,
      'tags': ['shopping', 'errands'],
    });
    print('    Created: "${todo1.data['title']}" (id: ${todo1.id})');

    final todo2 = await todos.add({
      'title': 'Write unit tests',
      'completed': false,
      'priority': 3,
      'tags': ['coding', 'work'],
    });
    print('    Created: "${todo2.data['title']}" (id: ${todo2.id})');

    final todo3 = await todos.add({
      'title': 'Read Rust book',
      'completed': true,
      'priority': 2,
      'tags': ['learning', 'coding'],
    });
    print('    Created: "${todo3.data['title']}" (id: ${todo3.id})\n');

    // ──────────────────────────────────────────────
    // 3b. Read — Get a single document by ID
    // ──────────────────────────────────────────────
    print('[3b] Reading todo by ID...');
    if (todo1.id.isNotEmpty) {
      final fetched = await todos.doc(todo1.id).get();
      if (fetched != null) {
        print('    Fetched: "${fetched.data['title']}" completed=${fetched.data['completed']}\n');
      } else {
        print('    (Document returned null — server may not support get-by-id yet)\n');
      }
    } else {
      print('    (Skipped — no ID returned from create)\n');
    }

    // ──────────────────────────────────────────────
    // 4. Query with filters — where, orderBy, limit
    // ──────────────────────────────────────────────
    print('[4] Querying todos...');

    // 4a. Get all todos
    final allTodos = await todos.get();
    print('    All todos: ${allTodos.size} found');

    // 4b. Filter: only incomplete todos
    final incomplete = await todos
        .where('completed', isEqualTo: false)
        .get();
    print('    Incomplete: ${incomplete.size} found');

    // 4c. Filter with ordering and limit
    final highPriority = await todos
        .where('priority', isGreaterThanOrEqualTo: 2)
        .orderBy('priority', descending: true)
        .limit(2)
        .get();
    print('    High priority (>=2, top 2): ${highPriority.size} found');
    for (final doc in highPriority.docs) {
      print('      - "${doc.data['title']}" priority=${doc.data['priority']}');
    }
    print('');

    // ──────────────────────────────────────────────
    // 5. Update a todo — mark as complete
    // ──────────────────────────────────────────────
    print('[5] Updating todo — marking "Buy groceries" as complete...');
    if (todo1.id.isNotEmpty) {
      final updated = await todos.doc(todo1.id).update({
        'completed': true,
      });
      print('    Updated! completed=${updated?.data['completed'] ?? true}\n');
    } else {
      print('    (Skipped — no ID from create)\n');
    }

    // ──────────────────────────────────────────────
    // 6. Batch operations — create multiple todos at once
    // ──────────────────────────────────────────────
    print('[6] Batch operations — creating 3 todos atomically...');
    final batch = ob.batch();
    batch.create(collectionName, {
      'title': 'Deploy to production',
      'completed': false,
      'priority': 5,
      'tags': ['devops'],
    });
    batch.create(collectionName, {
      'title': 'Code review PR #42',
      'completed': false,
      'priority': 4,
      'tags': ['coding', 'review'],
    });
    batch.create(collectionName, {
      'title': 'Update documentation',
      'completed': false,
      'priority': 1,
      'tags': ['docs'],
    });
    print('    Batch has ${batch.length} operations');
    final batchResults = await batch.commit();
    print('    Committed! ${batchResults.length} results returned');
    for (final r in batchResults) {
      print('      - id: ${r['id'] ?? 'n/a'}');
    }
    print('');

    // ──────────────────────────────────────────────
    // 7. FieldValue operations — increment, serverTimestamp
    // ──────────────────────────────────────────────
    print('[7] FieldValue operations...');
    if (todo2.id.isNotEmpty) {
      // 7a. Increment priority by 2
      print('    Incrementing priority of "Write unit tests" by 2...');
      await todos.doc(todo2.id).update({
        'priority': FieldValue.increment(2),
      });
      print('    Priority incremented (was 3, now should be 5)');

      // 7b. Set server timestamp
      print('    Setting server timestamp on "Write unit tests"...');
      await todos.doc(todo2.id).update({
        'updated_at': FieldValue.serverTimestamp(),
      });
      print('    Server timestamp set');

      // 7c. Array union — add a tag
      print('    Adding tag "urgent" via arrayUnion...');
      await todos.doc(todo2.id).update({
        'tags': FieldValue.arrayUnion(['urgent']),
      });
      print('    Tag added');

      // 7d. Verify the changes
      final verified = await todos.doc(todo2.id).get();
      if (verified != null) {
        print('    Verified: priority=${verified.data['priority']}, '
            'tags=${verified.data['tags']}, '
            'updated_at=${verified.data['updated_at']}');
      }
    } else {
      print('    (Skipped — no ID from create)');
    }
    print('');

    // ──────────────────────────────────────────────
    // 8. Delete a todo
    // ──────────────────────────────────────────────
    print('[8] Deleting todo "Read Rust book"...');
    if (todo3.id.isNotEmpty) {
      await todos.doc(todo3.id).delete();
      print('    Deleted!');

      // Verify deletion
      final deleted = await todos.doc(todo3.id).get();
      print('    Verified deleted: ${deleted == null ? 'yes (null)' : 'no (still exists)'}\n');
    } else {
      print('    (Skipped — no ID from create)\n');
    }

    // ──────────────────────────────────────────────
    // 9. Realtime subscription
    // ──────────────────────────────────────────────
    print('[9] Realtime subscription — listening for changes on $collectionName...');
    final realtime = RealtimeClient(ob);
    final changes = <DocumentChange>[];

    try {
      realtime.connect();
      final subscription = realtime.subscribe(collectionName);

      // Collect changes for up to 3 seconds
      final sub = subscription.listen((change) {
        changes.add(change);
        print('    Realtime event: ${change.type.name} → '
            '"${change.document.data['title'] ?? change.document.id}"');
      });

      // Trigger a change by creating a new todo
      await Future.delayed(const Duration(milliseconds: 500));
      print('    Creating a todo to trigger realtime event...');
      await todos.add({
        'title': 'Realtime test todo',
        'completed': false,
        'priority': 1,
      });

      // Wait a bit for the realtime event to arrive
      await Future.delayed(const Duration(seconds: 2));

      await sub.cancel();
      realtime.disconnect();
      print('    Received ${changes.length} realtime event(s)');
    } catch (e) {
      print('    Realtime error (server may not support WebSocket): $e');
      realtime.disconnect();
    }
    print('');

    // ──────────────────────────────────────────────
    // 10. Cleanup — delete all todos in the collection
    // ──────────────────────────────────────────────
    print('[10] Cleanup — deleting all todos...');
    final remaining = await todos.get();
    if (remaining.isNotEmpty) {
      final cleanupBatch = ob.batch();
      for (final doc in remaining.docs) {
        if (doc.id.isNotEmpty) {
          cleanupBatch.delete(collectionName, doc.id);
        }
      }
      if (!cleanupBatch.isEmpty) {
        await cleanupBatch.commit();
        print('    Deleted ${remaining.size} todo(s)');
      }
    } else {
      print('    No todos to clean up');
    }

    print('\n=== All steps completed successfully! ===');
  } catch (e) {
    print('\n[ERROR] $e');
    if (e is OrignaBaseException) {
      print('  Status code: ${e.statusCode}');
    }
  } finally {
    ob.dispose();
  }
}
