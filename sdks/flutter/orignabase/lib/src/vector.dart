import 'dart:convert';
import 'client.dart';
import 'document.dart';

/// Vector similarity search over a collection's embedding field.
///
/// Uses PostgreSQL's native `vector::similarity::cosine()` function
/// to find the most similar documents to a query embedding.
///
/// ```dart
/// final vs = VectorSearch(ob);
/// final results = await vs.search(
///   collection: 'products',
///   vectorField: 'embedding',
///   embedding: [0.1, 0.2, 0.3, ...],
///   topK: 5,
///   threshold: 0.7,
/// );
/// ```
class VectorSearch {
  final OrignaBase _client;

  VectorSearch(this._client);

  /// Search for documents most similar to the given [embedding] vector.
  ///
  /// - [collection]: The collection to search in.
  /// - [vectorField]: The field name that contains the embedding vectors.
  /// - [embedding]: The query vector to compare against.
  /// - [topK]: Maximum number of results to return (default 10).
  /// - [threshold]: Minimum cosine similarity score (0.0-1.0). If null,
  ///   all results above 0.0 are returned.
  ///
  /// Returns a list of [VectorSearchResult] with the document and its
  /// similarity score, ordered by score descending.
  Future<List<VectorSearchResult>> search({
    required String collection,
    required String vectorField,
    required List<double> embedding,
    int topK = 10,
    double? threshold,
  }) async {
    final args = <String>[
      'collection: "$collection"',
      'vectorField: "$vectorField"',
      'embedding: [${embedding.join(', ')}]',
      'topK: $topK',
    ];
    if (threshold != null) args.add('threshold: $threshold');

    final response = await _client.graphql(
      'query { vectorSearch(${args.join(', ')}) }',
    );

    final results = response['data']?['vectorSearch'];
    if (results is List) {
      return results.map((item) {
        final map = item is Map<String, dynamic>
            ? item
            : (item is String
                ? jsonDecode(item) as Map<String, dynamic>
                : <String, dynamic>{});
        final score = (map.remove('score') as num?)?.toDouble() ?? 0.0;
        return VectorSearchResult(
          document: Document.fromMap(collection, map),
          score: score,
        );
      }).toList();
    }
    return [];
  }
}

/// A single result from a vector similarity search.
class VectorSearchResult {
  /// The matched document.
  final Document document;

  /// The cosine similarity score (0.0 to 1.0).
  final double score;

  VectorSearchResult({required this.document, required this.score});

  @override
  String toString() => 'VectorSearchResult(score: $score, doc: ${document.id})';
}
