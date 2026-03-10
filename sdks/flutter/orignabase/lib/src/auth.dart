import 'dart:async';
import 'dart:convert';
import 'client.dart';

/// Authentication state.
enum AuthStatus { authenticated, unauthenticated }

/// Auth state with user info.
class AuthState {
  final AuthStatus status;
  final String? userId;
  final String? email;
  final List<String> roles;
  final bool emailVerified;
  final bool mfaRequired;
  final String? challengeToken;

  const AuthState({
    required this.status,
    this.userId,
    this.email,
    this.roles = const [],
    this.emailVerified = false,
    this.mfaRequired = false,
    this.challengeToken,
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

  /// Decoded claims from the current access token.
  Map<String, dynamic> get currentClaims => _decodeClaims(_accessToken);

  /// Current user id from the active access token.
  String? get currentUserId => currentClaims['sub'] as String?;

  /// Current email from the active access token or last auth response.
  String? get currentEmail => currentClaims['email'] as String?;

  /// Current roles from the active access token.
  List<String> get currentRoles {
    final roles = currentClaims['roles'];
    if (roles is List) {
      return roles.map((r) => r.toString()).toList();
    }
    return const [];
  }

  /// Current email verification status from the active access token.
  bool get isEmailVerified => currentClaims['email_verified'] == true;

  /// Current auth state.
  AuthState get currentState => _accessToken != null
      ? AuthState(
          status: AuthStatus.authenticated,
          userId: currentUserId,
          email: currentEmail,
          roles: currentRoles,
          emailVerified: isEmailVerified,
        )
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
  ///
  /// If MFA is enabled, returns an [AuthState] with [mfaRequired] = true.
  /// Use [verifyMfaChallenge] with the [AuthState.challengeToken] to complete.
  Future<AuthState> signInWithEmail(String email, String password) async {
    final response = await _client.request('POST', '/auth/login', body: {
      'email': email,
      'password': password,
    });

    return _handleAuthResponseWithMfa(response);
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

  /// Request a password reset email.
  Future<void> forgotPassword(String email) async {
    await _client.request('POST', '/auth/forgot-password', body: {
      'email': email,
    });
  }

  /// Reset password using a reset token.
  Future<void> resetPassword(String token, String newPassword) async {
    await _client.request('POST', '/auth/reset-password', body: {
      'token': token,
      'new_password': newPassword,
    });
  }

  /// Sign in anonymously. The user can later be upgraded to a real account.
  Future<AuthState> signInAnonymously() async {
    final response =
        await _client.request('POST', '/auth/anonymous', body: {});
    return _handleAuthResponse(response);
  }

  /// Upgrade an anonymous account to email/password.
  Future<AuthState> upgradeAnonymous(
    String email,
    String password, {
    String? displayName,
  }) async {
    final body = <String, dynamic>{
      'email': email,
      'password': password,
    };
    if (displayName != null) body['display_name'] = displayName;
    final response =
        await _client.request('POST', '/auth/anonymous/upgrade', body: body);
    return _handleAuthResponse(response);
  }

  /// Send a magic link email for passwordless sign-in.
  Future<void> sendMagicLink(String email) async {
    await _client.request('POST', '/auth/magic-link', body: {
      'email': email,
    });
  }

  /// Verify a magic link token and sign in.
  Future<AuthState> verifyMagicLink(String token) async {
    final response =
        await _client.request('POST', '/auth/verify-magic-link', body: {
      'token': token,
    });
    return _handleAuthResponse(response);
  }

  /// Send email verification to the current user.
  Future<void> sendEmailVerification() async {
    await _client.request('POST', '/auth/send-verification', body: {});
  }

  /// Verify email with token.
  Future<void> verifyEmail(String token) async {
    await _client.request('POST', '/auth/verify-email', body: {
      'token': token,
    });
  }

  // ── MFA / TOTP ──

  /// Set up MFA — returns QR code and manual key for authenticator app.
  Future<MfaSetupResult> setupMfa() async {
    final response =
        await _client.request('POST', '/auth/mfa/setup', body: {});
    return MfaSetupResult(
      qrCodeBase64: response['qr_code_base64'] as String? ?? '',
      manualKey: response['manual_key'] as String? ?? '',
      appleOtpauthUrl: response['apple_otpauth_url'] as String?,
    );
  }

  /// Verify MFA setup with a TOTP code from the authenticator app.
  /// Returns recovery codes that should be stored securely.
  Future<List<String>> verifyMfaSetup(String totpCode) async {
    final response =
        await _client.request('POST', '/auth/mfa/verify-setup', body: {
      'code': totpCode,
    });
    final codes = response['recovery_codes'] as List<dynamic>?;
    return codes?.map((c) => c.toString()).toList() ?? [];
  }

  /// Complete MFA challenge during login. Pass the TOTP code and challenge token.
  Future<AuthState> verifyMfaChallenge(
      String challengeToken, String totpCode) async {
    final response =
        await _client.request('POST', '/auth/mfa/challenge', body: {
      'challenge_token': challengeToken,
      'code': totpCode,
    });
    return _handleAuthResponse(response);
  }

  /// Use a recovery code to complete MFA challenge.
  Future<AuthState> useMfaRecoveryCode(
      String challengeToken, String recoveryCode) async {
    final response =
        await _client.request('POST', '/auth/mfa/recovery', body: {
      'challenge_token': challengeToken,
      'recovery_code': recoveryCode,
    });
    return _handleAuthResponse(response);
  }

  /// Disable MFA for the current user (requires current TOTP code).
  Future<void> disableMfa(String totpCode) async {
    await _client.request('DELETE', '/auth/mfa', body: {
      'code': totpCode,
    });
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
      userId: response['user_id'] as String? ?? currentUserId,
      email: response['email'] as String? ?? currentEmail,
      roles: currentRoles,
      emailVerified: isEmailVerified,
    );
    _authStateController.add(state);
    return state;
  }

  /// Handle login response that might require MFA.
  ///
  /// If MFA is required, returns an [AuthState] with [mfaRequired] = true
  /// and the [challengeToken] to use with [verifyMfaChallenge].
  AuthState _handleAuthResponseWithMfa(Map<String, dynamic> response) {
    if (response['mfa_required'] == true) {
      return AuthState(
        status: AuthStatus.unauthenticated,
        mfaRequired: true,
        challengeToken: response['challenge_token'] as String?,
      );
    }
    return _handleAuthResponse(response);
  }

  Map<String, dynamic> _decodeClaims(String? token) {
    if (token == null || token.isEmpty) return const {};
    try {
      final parts = token.split('.');
      if (parts.length != 3) return const {};
      final payload = base64Url.normalize(parts[1]);
      final decoded = utf8.decode(base64Url.decode(payload));
      final claims = jsonDecode(decoded);
      return claims is Map<String, dynamic> ? claims : const {};
    } catch (_) {
      return const {};
    }
  }

  /// Dispose the auth state stream.
  void dispose() {
    _authStateController.close();
  }
}

/// Result of MFA setup — contains QR code and manual key for authenticator apps.
class MfaSetupResult {
  final String qrCodeBase64;
  final String manualKey;
  final String? appleOtpauthUrl;

  MfaSetupResult({
    required this.qrCodeBase64,
    required this.manualKey,
    this.appleOtpauthUrl,
  });
}
