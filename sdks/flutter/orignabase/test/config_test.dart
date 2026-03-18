import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:orignabase/orignabase.dart';
import 'package:test/test.dart';

/// Creates an OrignaBase client with a mock HTTP client.
OrignaBase mockOb(
  Map<String, dynamic> Function(http.Request request) handler,
) {
  final client = MockClient((request) async {
    final body = handler(request);
    return http.Response(jsonEncode(body), 200, headers: {
      'content-type': 'application/json',
    });
  });
  return OrignaBase.initialize(
    url: 'http://test.local',
    httpClient: client,
  );
}

void main() {
  group('OrignaBaseConfig', () {
    test('getAll returns map of config key-value pairs', () async {
      final ob = mockOb((req) {
        expect(req.method, 'GET');
        expect(req.url.path, '/config');
        return {
          'feature_x': 'enabled',
          'max_retries': 3,
          'debug_mode': true,
        };
      });

      final result = await ob.config.getAll();
      expect(result['feature_x'], 'enabled');
      expect(result['max_retries'], 3);
      expect(result['debug_mode'], true);
      ob.dispose();
    });

    test('get returns single value', () async {
      final ob = mockOb((req) {
        expect(req.url.path, '/config/feature_x');
        return {'key': 'feature_x', 'value': 'enabled'};
      });

      final result = await ob.config.get('feature_x');
      expect(result, 'enabled');
      ob.dispose();
    });

    test('get returns null for missing key', () async {
      final ob = mockOb((req) {
        return {'key': 'missing', 'value': null};
      });

      final result = await ob.config.get('missing');
      expect(result, isNull);
      ob.dispose();
    });

    test('getString returns string value', () async {
      final ob = mockOb((req) {
        return {'key': 'feature_x', 'value': 'enabled'};
      });

      final result = await ob.config.getString('feature_x');
      expect(result, 'enabled');
      expect(result, isA<String>());
      ob.dispose();
    });

    test('getString returns empty string when value is null', () async {
      final ob = mockOb((req) {
        return {'key': 'missing', 'value': null};
      });

      final result = await ob.config.getString('missing');
      expect(result, '');
      ob.dispose();
    });

    test('getString converts non-string to string', () async {
      final ob = mockOb((req) {
        return {'key': 'count', 'value': 42};
      });

      final result = await ob.config.getString('count');
      expect(result, '42');
      ob.dispose();
    });

    test('getBool returns true for boolean true', () async {
      final ob = mockOb((req) {
        return {'key': 'debug', 'value': true};
      });

      final result = await ob.config.getBool('debug');
      expect(result, true);
      ob.dispose();
    });

    test('getBool returns false for boolean false', () async {
      final ob = mockOb((req) {
        return {'key': 'debug', 'value': false};
      });

      final result = await ob.config.getBool('debug');
      expect(result, false);
      ob.dispose();
    });

    test('getBool parses string "true"', () async {
      final ob = mockOb((req) {
        return {'key': 'debug', 'value': 'true'};
      });

      final result = await ob.config.getBool('debug');
      expect(result, true);
      ob.dispose();
    });

    test('getBool returns false for null', () async {
      final ob = mockOb((req) {
        return {'key': 'missing', 'value': null};
      });

      final result = await ob.config.getBool('missing');
      expect(result, false);
      ob.dispose();
    });

    test('getInt returns integer value', () async {
      final ob = mockOb((req) {
        return {'key': 'max_retries', 'value': 5};
      });

      final result = await ob.config.getInt('max_retries');
      expect(result, 5);
      ob.dispose();
    });

    test('getInt parses string to int', () async {
      final ob = mockOb((req) {
        return {'key': 'max_retries', 'value': '10'};
      });

      final result = await ob.config.getInt('max_retries');
      expect(result, 10);
      ob.dispose();
    });

    test('getInt returns 0 for null', () async {
      final ob = mockOb((req) {
        return {'key': 'missing', 'value': null};
      });

      final result = await ob.config.getInt('missing');
      expect(result, 0);
      ob.dispose();
    });

    test('getInt returns 0 for unparseable string', () async {
      final ob = mockOb((req) {
        return {'key': 'bad', 'value': 'not_a_number'};
      });

      final result = await ob.config.getInt('bad');
      expect(result, 0);
      ob.dispose();
    });

    test('getDouble returns double value', () async {
      final ob = mockOb((req) {
        return {'key': 'threshold', 'value': 0.75};
      });

      final result = await ob.config.getDouble('threshold');
      expect(result, 0.75);
      ob.dispose();
    });

    test('getDouble parses string to double', () async {
      final ob = mockOb((req) {
        return {'key': 'threshold', 'value': '3.14'};
      });

      final result = await ob.config.getDouble('threshold');
      expect(result, 3.14);
      ob.dispose();
    });

    test('getDouble returns 0.0 for null', () async {
      final ob = mockOb((req) {
        return {'key': 'missing', 'value': null};
      });

      final result = await ob.config.getDouble('missing');
      expect(result, 0.0);
      ob.dispose();
    });

    test('set sends PUT to admin endpoint', () async {
      http.Request? capturedRequest;
      final client = MockClient((request) async {
        capturedRequest = request;
        return http.Response('{}', 200, headers: {
          'content-type': 'application/json',
        });
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      await ob.config.set('feature_x', 'enabled');
      expect(capturedRequest, isNotNull);
      expect(capturedRequest!.method, 'PUT');
      expect(capturedRequest!.url.path, '/_admin/config/feature_x');

      final body = jsonDecode(capturedRequest!.body) as Map<String, dynamic>;
      expect(body['value'], 'enabled');
      ob.dispose();
    });

    test('delete sends DELETE to admin endpoint', () async {
      http.Request? capturedRequest;
      final client = MockClient((request) async {
        capturedRequest = request;
        return http.Response('{}', 200, headers: {
          'content-type': 'application/json',
        });
      });
      final ob = OrignaBase.initialize(
        url: 'http://test.local',
        httpClient: client,
      );

      await ob.config.delete('feature_x');
      expect(capturedRequest, isNotNull);
      expect(capturedRequest!.method, 'DELETE');
      expect(capturedRequest!.url.path, '/_admin/config/feature_x');
      ob.dispose();
    });

    test('config is accessible from client', () {
      final ob = OrignaBase.initialize(url: 'http://localhost:8080');
      expect(ob.config, isA<OrignaBaseConfig>());
      ob.dispose();
    });
  });
}
