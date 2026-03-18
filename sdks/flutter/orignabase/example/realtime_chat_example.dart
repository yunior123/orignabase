/// Realtime chat example using OrignaBase WebSocket subscriptions.
///
/// Demonstrates: realtime subscribe, send/receive messages,
/// presence tracking, and cleanup.
library;

import 'dart:async';

import 'package:orignabase/orignabase.dart';

Future<void> main() async {
  // Initialize two clients (simulating two users)
  final alice = OrignaBase.initialize(url: 'http://localhost:8080');
  final bob = OrignaBase.initialize(url: 'http://localhost:8080');

  // Register both users
  await alice.auth.register('alice@chat.example', 'AlicePass123!');
  await bob.auth.register('bob@chat.example', 'BobPass123!');

  final chatRoom = 'chat_room_demo';
  final messages = <Map<String, dynamic>>[];

  // ── Subscribe to chat room ─────────────────────────────────────────
  print('=== Setting up realtime chat ===');

  final subscription = alice.realtime.subscribe(chatRoom);
  final listener = subscription.listen((change) {
    print('[Alice sees] ${change.type}: ${change.document.data}');
    messages.add(change.document.data);
  });

  // Wait for subscription to establish
  await Future.delayed(const Duration(seconds: 1));

  // ── Bob sends messages ─────────────────────────────────────────────
  print('\n=== Bob sending messages ===');

  await bob.collection(chatRoom).add({
    'sender': 'bob',
    'text': 'Hey Alice!',
    'timestamp': DateTime.now().toIso8601String(),
  });

  await bob.collection(chatRoom).add({
    'sender': 'bob',
    'text': 'How are you?',
    'timestamp': DateTime.now().toIso8601String(),
  });

  // Wait for realtime events
  await Future.delayed(const Duration(seconds: 2));

  // ── Alice replies ──────────────────────────────────────────────────
  print('\n=== Alice replying ===');

  await alice.collection(chatRoom).add({
    'sender': 'alice',
    'text': 'Hi Bob! Doing great!',
    'timestamp': DateTime.now().toIso8601String(),
  });

  await Future.delayed(const Duration(seconds: 1));

  // ── Read chat history ──────────────────────────────────────────────
  print('\n=== Chat History ===');
  final history = await alice
      .collection(chatRoom)
      .orderBy('timestamp')
      .get();

  for (final doc in history.docs) {
    final sender = doc.data['sender'];
    final text = doc.data['text'];
    print('  $sender: $text');
  }

  // ── Presence (online status) ───────────────────────────────────────
  print('\n=== Presence ===');
  try {
    final online = await alice.presence.getOnlineUsers();
    print('Online users: ${online.length}');
  } catch (e) {
    print('Presence not available: $e');
  }

  // ── Cleanup ────────────────────────────────────────────────────────
  await listener.cancel();
  print('\nMessages received by Alice: ${messages.length}');

  alice.dispose();
  bob.dispose();
  print('Done!');
}
