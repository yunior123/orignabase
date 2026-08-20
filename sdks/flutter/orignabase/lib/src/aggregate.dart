import 'query.dart';

/// Aggregate query helpers for PostgreSQL.
///
/// Builds raw SQL aggregate queries (COUNT, SUM, AVG) from
/// a collection name and optional filters.
///
/// ```dart
/// final agg = AggregateQuery('products', []);
/// final countQ = agg.toCountQuery();
/// // => { 'query': 'SELECT count() as total FROM products GROUP ALL' }
/// ```
class AggregateQuery {
  final String collection;
  final List<QueryFilter> _filters;

  AggregateQuery(this.collection, this._filters);

  /// Build a COUNT query.
  Map<String, dynamic> toCountQuery() {
    return {
      'query':
          'SELECT count() as total FROM $collection ${_buildWhere()} GROUP ALL',
    };
  }

  /// Build a SUM query for a numeric field.
  Map<String, dynamic> toSumQuery(String field) {
    return {
      'query':
          'SELECT math::sum($field) as total FROM $collection ${_buildWhere()} GROUP ALL',
    };
  }

  /// Build an AVG query for a numeric field.
  Map<String, dynamic> toAvgQuery(String field) {
    return {
      'query':
          'SELECT math::mean($field) as average FROM $collection ${_buildWhere()} GROUP ALL',
    };
  }

  String _buildWhere() {
    if (_filters.isEmpty) return '';
    final conditions =
        _filters.map((f) => _filterToSqlCondition(f)).join(' AND ');
    return 'WHERE $conditions';
  }

  String _filterToSqlCondition(QueryFilter f) {
    final op = switch (f.operator) {
      'eq' => '=',
      'ne' => '!=',
      'gt' => '>',
      'gte' => '>=',
      'lt' => '<',
      'lte' => '<=',
      'in' => 'IN',
      'contains' => 'CONTAINS',
      'starts_with' => '~',
      _ => '=',
    };
    final val = f.value is String
        ? "'${(f.value as String).replaceAll("'", "''")}'"
        : '${f.value}';
    return '${f.field} $op $val';
  }
}
