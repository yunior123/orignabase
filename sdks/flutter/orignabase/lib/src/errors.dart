/// Base exception for all OrignaBase errors.
class OrignaBaseException implements Exception {
  final String message;
  final int? statusCode;

  OrignaBaseException(this.message, {this.statusCode});

  @override
  String toString() => 'OrignaBaseException: $message (status: $statusCode)';
}

/// Authentication error (invalid credentials, expired token, etc.)
class AuthException extends OrignaBaseException {
  AuthException(super.message, {super.statusCode});
}

/// Permission denied by security rules.
class ForbiddenException extends OrignaBaseException {
  ForbiddenException(super.message, {super.statusCode});
}

/// Document or resource not found.
class NotFoundException extends OrignaBaseException {
  NotFoundException(super.message, {super.statusCode});
}

/// Validation error (invalid input).
class ValidationException extends OrignaBaseException {
  ValidationException(super.message, {super.statusCode});
}

/// Network/connection error.
class NetworkException extends OrignaBaseException {
  NetworkException(super.message, {super.statusCode});
}

/// Conflict error (e.g., document version mismatch, duplicate key).
class ConflictException extends OrignaBaseException {
  ConflictException(super.message, {super.statusCode});
}

/// Rate limit exceeded — too many requests.
class RateLimitException extends OrignaBaseException {
  RateLimitException(super.message, {super.statusCode});
}
