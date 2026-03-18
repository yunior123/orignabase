/// Fuzz-style tests for OrignaBase Flutter SDK.
///
/// Tests SDK resilience against random/malformed inputs.
/// Run: dart test test/fuzz_test.dart
@TestOn('vm')
library;

import 'dart:math';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

void main() {
  final random = Random(42); // Seeded for reproducibility

  String randomString(int length) {
    const chars =
        'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#\$%^&*()';
    return String.fromCharCodes(
      Iterable.generate(
          length, (_) => chars.codeUnitAt(random.nextInt(chars.length))),
    );
  }

  Map<String, dynamic> randomMap(int depth) {
    if (depth <= 0) return {'leaf': randomString(10)};
    final map = <String, dynamic>{};
    for (var i = 0; i < random.nextInt(5) + 1; i++) {
      final key = randomString(random.nextInt(20) + 1);
      switch (random.nextInt(5)) {
        case 0:
          map[key] = randomString(random.nextInt(100));
        case 1:
          map[key] = random.nextInt(100000);
        case 2:
          map[key] = random.nextDouble();
        case 3:
          map[key] = random.nextBool();
        case 4:
          map[key] = randomMap(depth - 1);
      }
    }
    return map;
  }

  group('Fuzz: Document serialization', () {
    for (var i = 0; i < 10; i++) {
      test('random map roundtrip #$i', () {
        final data = randomMap(3);
        final doc =
            Document(id: 'fuzz_$i', collection: 'fuzz', data: data);
        expect(doc.data, equals(data));
        expect(doc.id, equals('fuzz_$i'));
      });
    }
  });

  group('Fuzz: Query builder', () {
    test('random filter combinations', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      for (var i = 0; i < 20; i++) {
        final field = randomString(random.nextInt(30) + 1);
        final value = random.nextBool()
            ? randomString(20)
            : random.nextInt(1000);
        final query = ob
            .collection('fuzz_col')
            .where(field, isEqualTo: value)
            .limit(random.nextInt(100) + 1);
        expect(query, isNotNull);
      }
      ob.dispose();
    });

    test('random sort combinations', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      for (var i = 0; i < 10; i++) {
        final field = randomString(random.nextInt(20) + 1);
        final query = ob
            .collection('fuzz_sort')
            .orderBy(field, descending: random.nextBool())
            .limit(random.nextInt(50) + 1)
            .offset(random.nextInt(100));
        expect(query, isNotNull);
      }
      ob.dispose();
    });

    test('empty/null-like inputs', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(() => ob.collection(''), returnsNormally);
      expect(() => ob.collection(randomString(500)), returnsNormally);
      expect(() => ob.collection('a/b/c'), returnsNormally);
      expect(() => ob.collection('a..b'), returnsNormally);
      ob.dispose();
    });
  });

  group('Fuzz: Client config', () {
    test('random base URLs do not crash constructor', () {
      for (var i = 0; i < 10; i++) {
        final url = randomString(random.nextInt(100) + 1);
        final ob = OrignaBase.initialize(url: url);
        expect(ob, isNotNull);
        ob.dispose();
      }
    });

    test('empty base URL', () {
      final ob = OrignaBase.initialize(url: '');
      expect(ob, isNotNull);
      ob.dispose();
    });
  });

  group('Fuzz: Special characters in data', () {
    test('unicode strings', () {
      final data = {
        'emoji': '🔥🎉💀',
        'chinese': '你好世界',
        'arabic': 'مرحبا بالعالم',
        'special': '<script>alert("xss")</script>',
        'null_char': 'hello\x00world',
        'newlines': 'line1\nline2\rline3',
        'tabs': 'col1\tcol2\tcol3',
      };
      final doc =
          Document(id: 'unicode_test', collection: 'fuzz', data: data);
      expect(doc.data, equals(data));
    });

    test('very large values', () {
      final data = {
        'big_string': 'x' * 100000,
        'big_number': 9999999999999,
        'big_array': List.generate(1000, (i) => i),
      };
      final doc =
          Document(id: 'large_test', collection: 'fuzz', data: data);
      expect(doc.data['big_string']?.length, equals(100000));
    });
  });
}
