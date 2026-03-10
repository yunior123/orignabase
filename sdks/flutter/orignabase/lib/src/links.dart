import 'client.dart';

/// A dynamic link with its slug and target.
class DynamicLink {
  final String slug;
  final String shortUrl;
  final String targetUrl;
  final String? title;
  final String? description;
  final int clicks;

  DynamicLink({
    required this.slug,
    required this.shortUrl,
    required this.targetUrl,
    this.title,
    this.description,
    this.clicks = 0,
  });

  factory DynamicLink.fromMap(Map<String, dynamic> map) {
    return DynamicLink(
      slug: map['slug'] as String? ?? '',
      shortUrl: map['short_url'] as String? ?? '/l/${map['slug']}',
      targetUrl: map['target_url'] as String? ?? '',
      title: map['title'] as String?,
      description: map['description'] as String?,
      clicks: (map['clicks'] as num?)?.toInt() ?? 0,
    );
  }
}

/// Dynamic Links — Firebase Dynamic Links replacement.
///
/// ```dart
/// final link = await ob.links.create(url: 'https://example.com/promo');
/// print(link.shortUrl); // /l/a3f2b1c0
/// ```
class OrignaBaseLinks {
  final OrignaBase _client;

  OrignaBaseLinks(this._client);

  /// Create a new dynamic link.
  Future<DynamicLink> create({
    required String url,
    String? slug,
    String? title,
    String? description,
  }) async {
    final body = <String, dynamic>{'url': url};
    if (slug != null) body['slug'] = slug;
    if (title != null) body['title'] = title;
    if (description != null) body['description'] = description;

    final response = await _client.request('POST', '/links', body: body);
    return DynamicLink.fromMap(response);
  }

  /// List all dynamic links (admin only).
  Future<List<DynamicLink>> list() async {
    final response = await _client.request('GET', '/_admin/links');
    final links = response['links'] as List<dynamic>? ?? [];
    return links
        .whereType<Map<String, dynamic>>()
        .map(DynamicLink.fromMap)
        .toList();
  }
}
