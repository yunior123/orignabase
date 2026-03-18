/// Complete authentication flows example for OrignaBase SDK.
///
/// Demonstrates: register, login, anonymous, OAuth, password reset,
/// email verification, MFA setup, and magic links.
library;

import 'package:orignabase/orignabase.dart';

Future<void> main() async {
  final ob = OrignaBase.initialize(url: 'http://localhost:8080');

  // ── Email Registration ─────────────────────────────────────────────
  print('=== Email Registration ===');
  final regState = await ob.auth.register(
    'user@example.com',
    'SecurePassword123!',
  );
  print('Registered: ${regState.isAuthenticated}');
  print('User ID: ${regState.userId}');
  print('Access token: ${ob.auth.accessToken?.substring(0, 20)}...');

  // ── Sign Out ───────────────────────────────────────────────────────
  ob.auth.signOut();
  print('\nSigned out: ${ob.auth.currentState.isAuthenticated == false}');

  // ── Email Login ────────────────────────────────────────────────────
  print('\n=== Email Login ===');
  final loginState = await ob.auth.signInWithEmail(
    'user@example.com',
    'SecurePassword123!',
  );
  print('Logged in: ${loginState.isAuthenticated}');

  // ── Token Refresh ──────────────────────────────────────────────────
  print('\n=== Token Refresh ===');
  final refreshed = await ob.auth.refreshToken();
  print('Refreshed: ${refreshed.isAuthenticated}');

  // ── Anonymous Sign-In ──────────────────────────────────────────────
  print('\n=== Anonymous Auth ===');
  final ob2 = OrignaBase.initialize(url: 'http://localhost:8080');
  final anonState = await ob2.auth.signInAnonymously();
  print('Anonymous: ${anonState.isAuthenticated}');

  // Upgrade anonymous to email
  try {
    final upgraded = await ob2.auth.upgradeAnonymous(
      'upgraded@example.com',
      'UpgradePass123!',
    );
    print('Upgraded: ${upgraded.isAuthenticated}');
  } catch (e) {
    print('Upgrade not supported in this environment: $e');
  }
  ob2.dispose();

  // ── Forgot / Reset Password ────────────────────────────────────────
  print('\n=== Password Reset ===');
  await ob.auth.forgotPassword('user@example.com');
  print('Reset email sent (or no-op in dev)');

  // In production, user receives email with token, then:
  // await ob.auth.resetPassword(token, 'NewPassword123!');

  // ── Email Verification ─────────────────────────────────────────────
  print('\n=== Email Verification ===');
  await ob.auth.sendEmailVerification();
  print('Verification email sent');

  // In production, user clicks link with token:
  // await ob.auth.verifyEmail(token);

  // ── Magic Link ─────────────────────────────────────────────────────
  print('\n=== Magic Link ===');
  await ob.auth.sendMagicLink('user@example.com');
  print('Magic link sent');

  // In production: await ob.auth.verifyMagicLink(token);

  // ── MFA Setup ──────────────────────────────────────────────────────
  print('\n=== MFA Setup ===');
  try {
    final mfa = await ob.auth.setupMfa();
    print('MFA manual key: ${mfa.manualKey}');
    print('QR code base64 length: ${mfa.qrCodeBase64.length}');

    // User enters TOTP code from authenticator app:
    // final backupCodes = await ob.auth.verifyMfaSetup('123456');
    // print('Backup codes: $backupCodes');

    // To disable MFA:
    // await ob.auth.disableMfa('123456');
  } catch (e) {
    print('MFA requires verified email: $e');
  }

  // ── OAuth Sign-In ──────────────────────────────────────────────────
  print('\n=== OAuth (Google/Apple) ===');
  // Google: obtain ID token from Google Sign-In package
  // final googleState = await ob.auth.signInWithGoogle(googleIdToken);

  // Apple: obtain authorization code from sign_in_with_apple package
  // final appleState = await ob.auth.signInWithApple(authCode, identityToken: idToken);

  // Generic OIDC:
  // final oidcState = await ob.auth.signInWithOidc(accessToken);
  print('OAuth methods available but require platform tokens');

  // ── Auth State Stream ──────────────────────────────────────────────
  print('\n=== Auth State Listener ===');
  ob.auth.authStateChanges.listen((state) {
    print('Auth state changed: ${state.status}');
  });

  ob.dispose();
  print('\nDone!');
}
