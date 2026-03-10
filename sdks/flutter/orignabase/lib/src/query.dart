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
  final List<QueryFilter> _filters;
  String? _orderByField;
  bool _descending;
  int? _limitCount;
  int? _offsetCount;
  String? _startAfter;
  List<String>? _selectFields;

  Query(this.client, this.collectionName)
      : _filters = [],
        _descending = false;

  Query._copy(Query other)
      : client = other.client,
        collectionName = other.collectionName,
        _filters = List.of(other._filters),
        _orderByField = other._orderByField,
        _descending = other._descending,
        _limitCount = other._limitCount,
        _offsetCount = other._offsetCount,
        _startAfter = other._startAfter,
        _selectFields = other._selectFields != null ? List.of(other._selectFields!) : null;

  /// Add a filter condition. Returns a new Query (immutable pattern).
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
    final q = Query._copy(this);
    if (isEqualTo != null) q._filters.add(QueryFilter(field, 'eq', isEqualTo));
    if (isNotEqualTo != null) q._filters.add(QueryFilter(field, 'ne', isNotEqualTo));
    if (isGreaterThan != null) q._filters.add(QueryFilter(field, 'gt', isGreaterThan));
    if (isGreaterThanOrEqualTo != null) q._filters.add(QueryFilter(field, 'gte', isGreaterThanOrEqualTo));
    if (isLessThan != null) q._filters.add(QueryFilter(field, 'lt', isLessThan));
    if (isLessThanOrEqualTo != null) q._filters.add(QueryFilter(field, 'lte', isLessThanOrEqualTo));
    if (whereIn != null) q._filters.add(QueryFilter(field, 'in', whereIn));
    if (contains != null) q._filters.add(QueryFilter(field, 'contains', contains));
    if (startsWith != null) q._filters.add(QueryFilter(field, 'starts_with', startsWith));
    return q;
  }

  /// Set the field to order results by. Returns a new Query.
  Query orderBy(String field, {bool descending = false}) {
    final q = Query._copy(this);
    q._orderByField = field;
    q._descending = descending;
    return q;
  }

  /// Limit the number of results. Returns a new Query.
  Query limit(int count) {
    final q = Query._copy(this);
    q._limitCount = count;
    return q;
  }

  /// Skip the first N results. Returns a new Query.
  Query offset(int count) {
    final q = Query._copy(this);
    q._offsetCount = count;
    return q;
  }

  /// Start results after the given document (cursor-based pagination).
  /// Returns a new Query.
  Query startAfter(Document document) {
    final q = Query._copy(this);
    q._startAfter = document.id;
    return q;
  }

  /// Start results after the given document ID string. Returns a new Query.
  Query startAfterId(String documentId) {
    final q = Query._copy(this);
    q._startAfter = documentId;
    return q;
  }

  /// Select only specific fields (field projection). Returns a new Query.
  Query select(List<String> fields) {
    final q = Query._copy(this);
    q._selectFields = fields;
    return q;
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
    if (_startAfter != null) args.add('startAfter: "$_startAfter"');
    if (_selectFields != null && _selectFields!.isNotEmpty) {
      args.add('select: ${jsonEncode(_selectFields)}');
    }

    final query = 'query { list(${args.join(', ')}) }';
    final response = await client.graphql(query);

    final data = response['data'] as Map<String, dynamic>?;
    if (data == null) return QuerySnapshot(docs: []);

    final rawList = data['list'];
    if (rawList is! List) return QuerySnapshot(docs: []);

    // N+1 pattern: if we requested limit, check if there are more results
    final requestedLimit = _limitCount;
    final allDocs = rawList
        .whereType<Map<String, dynamic>>()
        .map((m) => Document.fromMap(collectionName, m))
        .toList();

    final bool hasMore;
    final List<Document> docs;
    if (requestedLimit != null && allDocs.length > requestedLimit) {
      docs = allDocs.sublist(0, requestedLimit);
      hasMore = true;
    } else {
      docs = allDocs;
      hasMore = false;
    }

    return QuerySnapshot(docs: docs, hasMore: hasMore);
  }

  String _buildFiltersJson() {
    if (_filters.isEmpty) return '';
    // Use a list of filter objects to preserve multiple filters on the same field
    // (e.g., price > 10 AND price < 100). A map keyed by field name would collide.
    final filterList = _filters.map((f) => f.toGraphQL()).toList();
    return jsonEncode(filterList);
  }
}
