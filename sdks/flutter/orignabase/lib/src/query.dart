import 'dart:convert';
import 'client.dart';
import 'document.dart';

/// A filter condition for queries.
class QueryFilter {
  final String field;
  final String operator;
  final dynamic value;

  QueryFilter(this.field, this.operator, this.value);

  Map<String, dynamic> toGraphQL() {
    return {field: {'_$operator': value}};
  }
}

/// A query builder with Firestore-like chaining API.
///
/// ```dart
/// final results = await ob.collection('products')
///     .where('status', isEqualTo: 'active')
///     .where('price', isGreaterThan: 10)
///     .orderBy('created_at', descending: true)
///     .limit(20)
///     .get();
/// ```
class Query {
  final OrignaBase client;
  final String collectionName;
  final List<QueryFilter> _filters = [];
  String? _orderByField;
  bool _descending = false;
  int? _limitCount;
  int? _offsetCount;

  Query(this.client, this.collectionName);

  /// Add a filter condition.
  Query where(
    String field, {
    dynamic isEqualTo,
    dynamic isNotEqualTo,
    dynamic isGreaterThan,
    dynamic isGreaterThanOrEqualTo,
    dynamic isLessThan,
    dynamic isLessThanOrEqualTo,
    List<dynamic>? whereIn,
    dynamic contains,
    dynamic startsWith,
  }) {
    if (isEqualTo != null) _filters.add(QueryFilter(field, 'eq', isEqualTo));
    if (isNotEqualTo != null) _filters.add(QueryFilter(field, 'ne', isNotEqualTo));
    if (isGreaterThan != null) _filters.add(QueryFilter(field, 'gt', isGreaterThan));
    if (isGreaterThanOrEqualTo != null) _filters.add(QueryFilter(field, 'gte', isGreaterThanOrEqualTo));
    if (isLessThan != null) _filters.add(QueryFilter(field, 'lt', isLessThan));
    if (isLessThanOrEqualTo != null) _filters.add(QueryFilter(field, 'lte', isLessThanOrEqualTo));
    if (whereIn != null) _filters.add(QueryFilter(field, 'in', whereIn));
    if (contains != null) _filters.add(QueryFilter(field, 'contains', contains));
    if (startsWith != null) _filters.add(QueryFilter(field, 'starts_with', startsWith));
    return this;
  }

  /// Set the field to order results by.
  Query orderBy(String field, {bool descending = false}) {
    _orderByField = field;
    _descending = descending;
    return this;
  }

  /// Limit the number of results.
  Query limit(int count) {
    _limitCount = count;
    return this;
  }

  /// Skip the first N results.
  Query offset(int count) {
    _offsetCount = count;
    return this;
  }

  /// Execute the query and return results.
  Future<QuerySnapshot> get() async {
    final filters = _buildFiltersJson();
    final args = <String>['collection: "$collectionName"'];
    if (filters.isNotEmpty) args.add('filters: ${jsonEncode(filters)}');
    if (_orderByField != null) args.add('orderBy: "$_orderByField"');
    if (_descending) args.add('descending: true');
    if (_limitCount != null) args.add('limit: $_limitCount');
    if (_offsetCount != null) args.add('offset: $_offsetCount');

    final query = 'query { list(${args.join(', ')}) }';
    final response = await client.graphql(query);

    final data = response['data'] as Map<String, dynamic>?;
    if (data == null) return QuerySnapshot(docs: []);

    final rawList = data['list'];
    if (rawList is! List) return QuerySnapshot(docs: []);

    final docs = rawList
        .whereType<Map<String, dynamic>>()
        .map((m) => Document.fromMap(collectionName, m))
        .toList();

    return QuerySnapshot(docs: docs);
  }

  String _buildFiltersJson() {
    if (_filters.isEmpty) return '';
    final map = <String, dynamic>{};
    for (final filter in _filters) {
      map.addAll(filter.toGraphQL());
    }
    return jsonEncode(map);
  }
}
