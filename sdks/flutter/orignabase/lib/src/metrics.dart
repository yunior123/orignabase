import 'client.dart';

/// A performance metric aggregation result.
class MetricSummary {
  final String name;
  final double avg;
  final double min;
  final double max;
  final int count;

  MetricSummary({
    required this.name,
    required this.avg,
    required this.min,
    required this.max,
    required this.count,
  });

  factory MetricSummary.fromMap(Map<String, dynamic> map) {
    return MetricSummary(
      name: map['name'] as String? ?? '',
      avg: (map['avg'] as num?)?.toDouble() ?? 0.0,
      min: (map['min'] as num?)?.toDouble() ?? 0.0,
      max: (map['max'] as num?)?.toDouble() ?? 0.0,
      count: (map['count'] as num?)?.toInt() ?? 0,
    );
  }
}

/// Performance Monitoring — Firebase Performance Monitoring replacement.
///
/// ```dart
/// await ob.metrics.record('page_load', 1250, tags: {'page': '/home'});
/// final stats = await ob.metrics.query();
/// ```
class OrignaBaseMetrics {
  final OrignaBase _client;

  OrignaBaseMetrics(this._client);

  /// Record a performance metric.
  Future<void> record(
    String name,
    num value, {
    Map<String, dynamic>? tags,
  }) async {
    final body = <String, dynamic>{
      'name': name,
      'value': value,
    };
    if (tags != null) body['tags'] = tags;

    await _client.request('POST', '/metrics', body: body);
  }

  /// Query aggregated metrics (admin only, last 24h).
  Future<List<MetricSummary>> query() async {
    final response = await _client.request('GET', '/_admin/metrics');
    final metrics = response['metrics'] as List<dynamic>? ?? [];
    return metrics
        .whereType<Map<String, dynamic>>()
        .map(MetricSummary.fromMap)
        .toList();
  }
}
