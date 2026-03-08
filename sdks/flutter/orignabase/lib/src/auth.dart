import 'dart:async';
import 'client.dart';

/// Authentication state.
enum AuthStatus { authenticated, unauthenticated }

/// Auth state with user info.
class AuthState {
  final AuthStatus status;
  final String? userId;
  final String? email;
  final List<String> roles;

  const AuthState({
    required this.status,
    this.userId,
    this.email,
    this.roles = const [],
  });

  bool get isAuthenticated => status == AuthStatus.authenticated;

  static const unauthenticated = AuthState(status: AuthStatus.unauthenticated);
}

/// OrignaBase authentication service.
///
/// ```dart
/// await ob.auth.signInWithEmail('user@example.com', 'password');
/// ob.auth.authStateChanges.listen((state) => print(state.status));
/// ```
class OrignaBaseAuth {
  final OrignaBase _client;
  String? _accessToken;
  String? _refreshToken;

  final _authStateController = StreamController<AuthState>.broadcast();

  OrignaBaseAuth(this._client);

  /// Current access token (null if not authenticated).
  String? get accessToken => _accessToken;

  /// Stream of auth state changes.
  Stream<AuthState> get authStateChanges => _authStateController.stream;

  /// Current auth state.
  AuthState get currentState => _accessToken != null
      ? AuthState(status: AuthStatus.authenticated)
      : AuthState.unauthenticated;

  /// Register a new user with email and password.
  Future<AuthState> register(String email, String password) async {
    final response = await _client.request('POST', '/auth/register', body: {
      'email': email,
      'password': password,
    });

    return _handleAuthResponse(response);
  }

  /// Sign in with email and password.
  Future<AuthState> signInWithEmail(String email, String password) async {
    final response = await _client.request('POST', '/auth/login', body: {
      'email': email,
      'password': password,
    });

    return _handleAuthResponse(response);
  }

  /// Refresh the access token using the refresh token.
  Future<AuthState> refreshToken() async {
    if (_refreshToken == null) {
      throw StateError('No refresh token available');
    }

    final response = await _client.request('POST', '/auth/refresh', body: {
      'refresh_token': _refreshToken,
    });

    return _handleAuthResponse(response);
  }

  /// Sign in with Google. Pass the ID token obtained from Google Sign-In SDK.
  Future<AuthState> signInWithGoogle(String idToken) async {
    final response = await _client.request('POST', '/auth/google', body: {
      'id_token': idToken,
    });

    return _handleAuthResponse(response);
  }

  /// Sign in with Apple. Pass the authorization code from Apple Sign-In.
  /// [displayName] is optional — Apple only sends it on first sign-in.
  Future<AuthState> signInWithApple(String authorizationCode,
      {String? displayName}) async {
    final body = <String, dynamic>{
      'authorization_code': authorizationCode,
    };
    if (displayName != null) {
      body['display_name'] = displayName;
    }

    final response = await _client.request('POST', '/auth/apple', body: body);
    return _handleAuthResponse(response);
  }

  /// Sign in with a generic OIDC provider. Pass the access token.
  Future<AuthState> signInWithOidc(String accessToken) async {
    final response = await _client.request('POST', '/auth/oidc', body: {
      'access_token': accessToken,
    });

    return _handleAuthResponse(response);
  }

  /// Sign out the current user.
  void signOut() {
    _accessToken = null;
    _refreshToken = null;
    _authStateController.add(AuthState.unauthenticated);
  }

  AuthState _handleAuthResponse(Map<String, dynamic> response) {
    _accessToken = response['access_token'] as String?;
    _refreshToken = response['refresh_token'] as String?;

    final state = AuthState(
      status: AuthStatus.authenticated,
      userId: response['user_id'] as String?,
    );
    _authStateController.add(state);
    return state;
  }

  /// Dispose the auth state stream.
  void dispose() {
    _authStateController.close();
  }
}
