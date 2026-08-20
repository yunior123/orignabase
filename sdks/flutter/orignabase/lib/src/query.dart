import 'client.dart';
import 'document.dart';
import 'graphql_utils.dart';

/// A filter condition for queries.
class QueryFilter {
  final String field;
  final String operator;
  final dynamic value;

  QueryFilter(this.field, this.operator, this.value);

  Map<String, dynamic> toGraphQL() {
    return {
      field: {'_$operator': value}
    };
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
        _selectFields =
            other._selectFields != null ? List.of(other._selectFields!) : null;

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
    if (isNotEqualTo != null)
      q._filters.add(QueryFilter(field, 'ne', isNotEqualTo));
    if (isGreaterThan != null)
      q._filters.add(QueryFilter(field, 'gt', isGreaterThan));
    if (isGreaterThanOrEqualTo != null)
      q._filters.add(QueryFilter(field, 'gte', isGreaterThanOrEqualTo));
    if (isLessThan != null)
      q._filters.add(QueryFilter(field, 'lt', isLessThan));
    if (isLessThanOrEqualTo != null)
      q._filters.add(QueryFilter(field, 'lte', isLessThanOrEqualTo));
    if (whereIn != null) q._filters.add(QueryFilter(field, 'in', whereIn));
    if (contains != null)
      q._filters.add(QueryFilter(field, 'contains', contains));
    if (startsWith != null)
      q._filters.add(QueryFilter(field, 'starts_with', startsWith));
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
    final filters = _buildFiltersMap();
    final args = <String>['collection: "$collectionName"'];
    if (filters.isNotEmpty) args.add('filters: ${toGraphQLValue(filters)}');
    if (_orderByField != null) args.add('orderBy: "$_orderByField"');
    if (_descending) args.add('descending: true');
    if (_limitCount != null) args.add('limit: $_limitCount');
    // Some backends reject `offset: 0` on otherwise-empty result sets even
    // though it is semantically a no-op. Omit zero so first-page queries
    // remain stable across server implementations.
    if (_offsetCount != null && _offsetCount! > 0) {
      args.add('offset: $_offsetCount');
    }
    if (_startAfter != null) args.add('startAfter: "$_startAfter"');
    if (_selectFields != null && _selectFields!.isNotEmpty) {
      args.add('select: ${toGraphQLValue(_selectFields)}');
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

  /// Build filters as a merged Map (server expects an object, not a list).
  /// Multiple filters on the same field are merged into one object.
  Map<String, dynamic> _buildFiltersMap() {
    if (_filters.isEmpty) return {};
    final merged = <String, dynamic>{};
    for (final f in _filters) {
      final m = f.toGraphQL();
      for (final entry in m.entries) {
        if (merged.containsKey(entry.key) &&
            merged[entry.key] is Map &&
            entry.value is Map) {
          (merged[entry.key] as Map).addAll(entry.value as Map);
        } else {
          merged[entry.key] = entry.value;
        }
      }
    }
    return merged;
  }
}
