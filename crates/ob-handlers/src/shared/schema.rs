//! Schema constants — Rust port of Python schema_constants.py.
//! Single source of truth for all collection names, field names, and enum values.

// =============================================================================
// COLLECTIONS - Top-level database collection names
// =============================================================================

pub mod collections {
    pub const USERS: &str = "users";
    pub const PRODUCTS: &str = "products";
    pub const ORDERS: &str = "orders";
    pub const REVIEWS: &str = "reviews";
    pub const PAYOUTS: &str = "payouts";
    pub const REFUNDS: &str = "refunds";
    pub const WEBHOOK_LOGS: &str = "webhook_logs";
    pub const WEBHOOK_EVENTS: &str = "webhook_events";
    pub const SECURITY_ALERTS: &str = "security_alerts";
    pub const RATE_LIMITS: &str = "rate_limits";
    pub const CONFIG: &str = "config";
    pub const ADMIN_LOGS: &str = "admin_logs";
    pub const PRODUCT_RATINGS: &str = "product_ratings";
    pub const SELLER_RATINGS: &str = "seller_ratings";
    pub const REVIEW_VOTES: &str = "review_votes";
    pub const MEILISEARCH_SYNC_FAILURES: &str = "meilisearch_sync_failures";
    pub const CRON_LOCKS: &str = "_cron_locks";
    pub const CRON_FAILURES: &str = "_cron_failures";
    pub const RETURN_REQUESTS: &str = "return_requests";
    pub const PENDING_PROFILES: &str = "pending_profiles";
    // Subcollections
    pub const WAREHOUSES: &str = "warehouses";
    pub const CART: &str = "cart";
    pub const FAVORITES: &str = "favorites";
    pub const NOTIFICATIONS: &str = "notifications";
    pub const FCM_TOKENS: &str = "fcm_tokens";
    pub const LICENSES: &str = "licenses";
    pub const BOOK_ACCESS_TOKENS: &str = "book_access_tokens";
    pub const SOFTWARE_ACCESS_TOKENS: &str = "software_access_tokens";
    pub const ADDRESSES: &str = "addresses";
    pub const BUYER_ADDRESSES: &str = "buyer_addresses";
    pub const DOWNLOAD_SESSIONS: &str = "download_sessions";
    pub const STOCK_NOTIFICATIONS: &str = "stock_notifications";
    pub const PRODUCT_QUESTIONS: &str = "product_questions";
    pub const SELLER_METRICS: &str = "seller_metrics";
    pub const COUPONS: &str = "coupons";
    pub const INVENTORY_LEVELS: &str = "inventoryLevels";
    pub const ORDER_EVENTS: &str = "events";
    pub const COUPON_USES: &str = "coupon_uses";
    pub const USER_SECURITY: &str = "user_security";
    pub const SELLER_PROFILES: &str = "seller_profiles";
    pub const SELLER_SKUS: &str = "seller_skus";
    pub const MAIL_LOGS: &str = "_mail_logs";
    pub const PENDING_REDEMPTIONS: &str = "pending_redemptions";
    pub const SUBSCRIPTIONS: &str = "subscriptions";
    pub const CHATS: &str = "chats";
    pub const CHAT_MESSAGES: &str = "messages";
    pub const PLATFORM_DEBT: &str = "platform_debt";
    pub const MESSAGE_REPORTS: &str = "message_reports";
    pub const DISPUTES: &str = "disputes";
}

pub mod documents {
    pub const PAYMENT_PROVIDERS: &str = "payment_providers";
}

// =============================================================================
// APPLICATION CONSTANTS
// =============================================================================

pub const APP_NAME: &str = "Origna Marketplace";
pub const COUNTRY_CANADA: &str = "Canada";

pub mod email_config {
    pub const SUPPORT_EMAIL: &str = "support@orignaventures.ca";
    pub const SENDER_NAME: &str = "Origna GTA";
    pub const SENDER_NAME_SECURITY: &str = "Origna GTA Security";
    pub const COPYRIGHT_TEXT: &str = "\u{00a9} 2026 Origna Ventures Inc. All rights reserved.";
    pub const APP_TAGLINE: &str = "Canada's Modern Marketplace";
    pub const URL_PROD: &str = "https://orignagta.ca";
    pub const URL_STAGING: &str = "https://orignagta-staging.web.app";
    pub const URL_DEV: &str = "https://orignagta-dev.web.app";
    pub const MAILJET_API_VERSION: &str = "v3.1";
    pub const PHYSICAL_ADDRESS: &str =
        "Origna Ventures Inc., 136 Shaver Ave N, Toronto, ON M9B 4N8, Canada";
    pub const GST_HST_NUMBER: &str = "708286364RC0001";
    pub const UNSUBSCRIBE_URL_PROD: &str = "https://orignagta.ca/unsubscribe";
    pub const UNSUBSCRIBE_URL_STAGING: &str = "https://orignagta-staging.web.app/unsubscribe";
    pub const UNSUBSCRIBE_URL_DEV: &str = "https://orignagta-dev.web.app/unsubscribe";
    pub const PRIVACY_OFFICER_EMAIL: &str = "privacy@orignagta.ca";
    pub const PRIVACY_OFFICER_NAME: &str = "Yunior Rodriguez Osorio";
}

pub mod app_config {
    pub const PLATFORM_NAME: &str = "origna_gta";
    pub const DEFAULT_COUNTRY_CODE: &str = "CA";
    pub const DEFAULT_COUNTRY_NAME: &str = "Canada";
    pub const API_TIMEOUT_SECONDS: u64 = 30;
    pub const GEOAPIFY_TIMEOUT_SECONDS: u64 = 5;
    pub const TOKEN_CACHE_MINUTES: u64 = 25;
    pub const MEILISEARCH_MAX_RETRIES: u32 = 3;
    pub const MEILISEARCH_HITS_PER_PAGE: u32 = 20;
    pub const SITE_URL: &str = "https://orignagta.ca";
    pub const CHECKOUT_SUCCESS_PATH: &str = "/payment-success";
    pub const CHECKOUT_CANCEL_PATH: &str = "/payment-cancel";
    pub const SELLER_REFRESH_PATH: &str = "/seller/refresh";
    pub const SELLER_RETURN_PATH: &str = "/seller/return";

    pub const CORS_ORIGINS: &[&str] = &[
        "https://orignagta.ca",
        "https://www.orignagta.ca",
        "https://orignagta.web.app",
        "https://orignagta.firebaseapp.com",
        "https://orignagta-dev.web.app",
        "https://orignagta-dev.firebaseapp.com",
        "https://dev.orignagta.ca",
        "https://orignagta-staging.web.app",
        "https://orignagta-staging.firebaseapp.com",
        "https://staging.orignagta.ca",
        "http://localhost:5005",
        "http://localhost:5001",
    ];
}

pub mod external_urls {
    pub const SUPPORT_CHAT: &str = "https://tawk.to/chat/65d836479131ed19d9703644/1hnb2980k";
    pub const PRIVACY_POLICY: &str = "https://orignagta.ca/privacy";
    pub const TERMS_OF_SERVICE: &str = "https://orignagta.ca/terms";
    pub const REFUND_POLICY: &str = "https://orignagta.ca/refund";
}

// =============================================================================
// ENUMS
// =============================================================================

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    PendingPayment,
    PaymentAuthorized,
    AwaitingShippingApproval,
    Processing,
    Shipped,
    Delivered,
    Cancelled,
    Refunded,
    Disputed,
    Expired,
    Failed,
    ReturnRequested,
    ReturnApproved,
    ReturnRejected,
    Returned,
    Archived,
}

impl OrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PendingPayment => "PENDING_PAYMENT",
            Self::PaymentAuthorized => "PAYMENT_AUTHORIZED",
            Self::AwaitingShippingApproval => "AWAITING_SHIPPING_APPROVAL",
            Self::Processing => "PROCESSING",
            Self::Shipped => "SHIPPED",
            Self::Delivered => "DELIVERED",
            Self::Cancelled => "CANCELLED",
            Self::Refunded => "REFUNDED",
            Self::Disputed => "DISPUTED",
            Self::Expired => "EXPIRED",
            Self::Failed => "FAILED",
            Self::ReturnRequested => "RETURN_REQUESTED",
            Self::ReturnApproved => "RETURN_APPROVED",
            Self::ReturnRejected => "RETURN_REJECTED",
            Self::Returned => "RETURNED",
            Self::Archived => "ARCHIVED",
        }
    }

    /// Terminal states that cannot transition further.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Delivered | Self::Cancelled | Self::Expired | Self::Failed | Self::Disputed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PaymentStatus {
    Pending,
    Authorized,
    Captured,
    Refunded,
    PartialRefund,
    Failed,
    Cancelled,
    Disputed,
    Expired,
}

impl PaymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Authorized => "AUTHORIZED",
            Self::Captured => "CAPTURED",
            Self::Refunded => "REFUNDED",
            Self::PartialRefund => "PARTIAL_REFUND",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::Disputed => "DISPUTED",
            Self::Expired => "EXPIRED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Buyer,
    Seller,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Active,
    Cancelled,
    CancelPending,
    PastDue,
    Expired,
}

impl SubscriptionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Cancelled => "cancelled",
            Self::CancelPending => "cancel_pending",
            Self::PastDue => "past_due",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnRequestStatus {
    Requested,
    Approved,
    Rejected,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouponType {
    Percentage,
    FixedAmount,
    FreeShipping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterValue {
    Recent,
    Popular,
    PriceLowToHigh,
    PriceHighToLow,
    TopRated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvinceCode {
    AB,
    BC,
    MB,
    NB,
    NL,
    NS,
    NT,
    NU,
    ON,
    PE,
    QC,
    SK,
    YT,
}

// =============================================================================
// FIELD NAMES - Database document field names
// =============================================================================

pub mod fields {
    // Common timestamps
    pub const SAVED_AT: &str = "savedAt";
    pub const CREATED_AT: &str = "createdAt";
    pub const UPDATED_AT: &str = "updatedAt";
    pub const VERSION: &str = "version";
    pub const DELETED_AT: &str = "deletedAt";
    pub const DELETED_BY: &str = "deletedBy";
    pub const DELETED: &str = "deleted";

    // User fields
    pub const UID: &str = "uid";
    pub const EMAIL: &str = "email";
    pub const NAME: &str = "name";
    pub const ROLES: &str = "roles";
    pub const ADDRESS: &str = "address";
    pub const SELLER_PROFILE: &str = "sellerProfile";
    pub const BUSINESS_ADDRESS: &str = "businessAddress";
    pub const CUSTOMER_ID: &str = "customerId";
    pub const STRIPE_ACCOUNT_ID: &str = "stripeAccountId";
    pub const PAYOUTS_ENABLED: &str = "payoutsEnabled";
    pub const CHARGES_ENABLED: &str = "chargesEnabled";
    pub const ONBOARDING_COMPLETED: &str = "onboardingCompleted";
    pub const SUSPENDED: &str = "suspended";
    pub const SUSPENDED_AT: &str = "suspendedAt";
    pub const SUSPENDED_BY: &str = "suspendedBy";
    pub const SUSPENSION_REASON: &str = "suspensionReason";
    pub const COMMISSION_RATE_BPS: &str = "commissionRateBps";
    pub const MFA_ENABLED: &str = "mfaEnabled";
    pub const EMAIL_CONSENT: &str = "emailConsent";
    pub const LANGUAGE: &str = "language";
    pub const IS_PREMIUM: &str = "isPremium";

    // Product fields
    pub const PRODUCT_ID: &str = "productId";
    pub const SELLER_ID: &str = "sellerId";
    pub const BUYER_ID: &str = "buyerId";
    pub const TITLE: &str = "title";
    pub const DESCRIPTION: &str = "description";
    pub const PRICE_CENTS: &str = "priceCents";
    pub const STOCK_QUANTITY: &str = "stockQuantity";
    pub const IMAGE_URLS: &str = "imageUrls";
    pub const CATEGORY: &str = "category";
    pub const IS_ACTIVE: &str = "isActive";
    pub const LIFECYCLE_STATUS: &str = "lifecycleStatus";
    pub const AVG_RATING: &str = "avgRating";
    pub const TOTAL_REVIEWS: &str = "totalReviews";
    pub const IS_PERISHABLE: &str = "isPerishable";

    // Order fields
    pub const ORDER_ID: &str = "orderId";
    pub const ORDER_STATUS: &str = "orderStatus";
    pub const RETURN_STATUS: &str = "returnStatus";
    pub const STATUS: &str = "status";
    pub const ITEMS: &str = "items";
    pub const TOTAL_AMOUNT_CENTS: &str = "totalAmountCents";
    pub const SUBTOTAL_CENTS: &str = "subtotalCents";
    pub const TAX_AMOUNT_CENTS: &str = "taxAmountCents";
    pub const SHIPPING_COST_CENTS: &str = "shippingCostCents";
    pub const PLATFORM_FEE_CENTS: &str = "platformFeeCents";
    pub const PAYMENT_INTENT_ID: &str = "paymentIntentId";
    pub const CHECKOUT_SESSION_ID: &str = "checkoutSessionId";
    pub const PAYMENT_STATUS: &str = "paymentStatus";
    pub const CUMULATIVE_REFUNDED_CENTS: &str = "cumulativeRefundedCents";
    pub const PARTIAL_REFUND_AMOUNT_CENTS: &str = "partialRefundAmountCents";
    pub const CANCELLATION_REASON: &str = "cancellationReason";
    pub const CUSTOMER_EMAIL: &str = "customerEmail";
    pub const PREFERRED_LANGUAGE: &str = "preferredLanguage";
    pub const LAST_ACTOR_ID: &str = "lastActorId";
    pub const CART_ITEM_ID: &str = "cartItemId";

    // Shipping fields
    pub const SHIPPING_ADDRESS: &str = "shippingAddress";
    pub const TRACKING_NUMBER: &str = "trackingNumber";
    pub const SHIPPING_CARRIER: &str = "shippingCarrier";

    // Subscription fields
    pub const SUBSCRIPTION_ID: &str = "subscriptionId";
    pub const STRIPE_SUBSCRIPTION_ID: &str = "stripeSubscriptionId";
    pub const SUBSCRIPTION_STATUS: &str = "subscriptionStatus";
    pub const CURRENT_PERIOD_END: &str = "currentPeriodEnd";

    // Chat fields
    pub const CHAT_ID: &str = "chatId";
    pub const PARTICIPANTS: &str = "participants";
    pub const LAST_MESSAGE: &str = "lastMessage";
    pub const LAST_MESSAGE_AT: &str = "lastMessageAt";
    pub const UNREAD_COUNT: &str = "unreadCount";
    pub const MESSAGE_TEXT: &str = "messageText";
    pub const SENDER_ID: &str = "senderId";
    pub const READ: &str = "read";

    // Rating fields
    pub const RATING: &str = "rating";
    pub const REVIEW_TEXT: &str = "reviewText";
    pub const HELPFUL_COUNT: &str = "helpfulCount";

    // Coupon fields
    pub const CODE: &str = "code";
    pub const COUPON_TYPE: &str = "couponType";
    pub const DISCOUNT_VALUE: &str = "discountValue";
    pub const MIN_ORDER_CENTS: &str = "minOrderCents";
    pub const MAX_USES: &str = "maxUses";
    pub const USED_COUNT: &str = "usedCount";
    pub const EXPIRES_AT: &str = "expiresAt";

    // License fields
    pub const LICENSE_KEY: &str = "licenseKey";
    pub const DEVICE_ID: &str = "deviceId";
    pub const MAX_DEVICES: &str = "maxDevices";
    pub const ACTIVATED_DEVICES: &str = "activatedDevices";

    // Address fields
    pub const LABEL: &str = "label";
    pub const STREET: &str = "street";
    pub const CITY: &str = "city";
    pub const PROVINCE: &str = "province";
    pub const POSTAL_CODE: &str = "postalCode";
    pub const COUNTRY: &str = "country";
    pub const APARTMENT: &str = "apartment";
    pub const IS_DEFAULT: &str = "isDefault";
    pub const LATITUDE: &str = "latitude";
    pub const LONGITUDE: &str = "longitude";

    // FCM / Notification fields
    pub const FCM_TOKEN: &str = "fcmToken";
    pub const TOKEN: &str = "token";
    pub const PLATFORM: &str = "platform";
    pub const NOTIFICATION_TYPE: &str = "notificationType";
}

// =============================================================================
// BUSINESS RULES
// =============================================================================

pub mod business_rules {
    /// Free shipping threshold in cents ($75 CAD).
    pub const FREE_SHIPPING_THRESHOLD_CENTS: i64 = 7500;
    /// Platform commission rate in basis points (2.50%).
    pub const DEFAULT_COMMISSION_RATE_BPS: u32 = 250;
    /// Premium subscription price in CAD.
    pub const PREMIUM_SUBSCRIPTION_PRICE_CAD: f64 = 9.99;
    /// Maximum return window in days.
    pub const RETURN_WINDOW_DAYS: u32 = 30;
    /// Days before auto-archiving delivered/cancelled orders.
    pub const AUTO_ARCHIVE_DAYS: u32 = 30;
    /// Days before payment authorization expires.
    pub const AUTHORIZATION_EXPIRY_DAYS: u32 = 7;
    /// Maximum FCM push notifications per user per day.
    pub const MAX_PUSH_PER_DAY: u32 = 20;
    /// Abandoned cart email threshold in hours.
    pub const ABANDONED_CART_HOURS: u32 = 24;
    /// Abandoned cart email cooldown in hours.
    pub const ABANDONED_CART_COOLDOWN_HOURS: u32 = 72;
    /// Low stock alert cooldown in hours.
    pub const LOW_STOCK_ALERT_COOLDOWN_HOURS: u32 = 23;
    /// Rate limit window for stale docs in hours.
    pub const RATE_LIMIT_STALE_HOURS: u32 = 2;
    /// Webhook event retention in days.
    pub const WEBHOOK_EVENT_RETENTION_DAYS: u32 = 7;
    /// Security alert archive threshold in days.
    pub const SECURITY_ALERT_ARCHIVE_DAYS: u32 = 90;
    /// Orphaned image safety window in hours (don't delete recent uploads).
    pub const ORPHAN_IMAGE_SAFETY_HOURS: u32 = 24;
    /// Product video max bytes (100 MB).
    pub const MAX_VIDEO_BYTES: u64 = 100 * 1024 * 1024;
    /// Product video max duration in seconds.
    pub const MAX_VIDEO_DURATION_SECONDS: u32 = 60;
    /// Meilisearch sync failure DLQ max retries.
    pub const MEILISEARCH_DLQ_MAX_RETRIES: u32 = 3;
    /// Return request escalation threshold in days.
    pub const RETURN_ESCALATION_DAYS: u32 = 7;
    /// License key format: 4 groups of 4 uppercase alphanumeric chars.
    pub const LICENSE_KEY_PATTERN: &str = r"^[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}$";
    /// Download token validity in minutes.
    pub const DOWNLOAD_TOKEN_MINUTES: u32 = 15;
    /// Maximum devices per digital license.
    pub const MAX_DEVICES_PER_LICENSE: u32 = 5;
    /// Trending products scoring weights.
    pub const TRENDING_VIEW_WEIGHT: f64 = 1.0;
    pub const TRENDING_PURCHASE_WEIGHT: f64 = 3.0;
    pub const TRENDING_FAVORITE_WEIGHT: f64 = 2.0;
    /// Trending products time window in hours.
    pub const TRENDING_WINDOW_HOURS: u32 = 24;
    /// Local delivery radius for perishables in km.
    pub const LOCAL_DELIVERY_RADIUS_KM: f64 = 50.0;
}

/// Cancellation reason values.
pub mod cancellation_reasons {
    pub const BUYER_REQUESTED: &str = "requested_by_customer";
    pub const SELLER_CANCELLED: &str = "seller_cancelled";
    pub const SHIPPING_REJECTED: &str = "Buyer rejected shipping cost";
    pub const PAYMENT_FAILED: &str = "payment_failed";
    pub const EXPIRED: &str = "authorization_expired";
}

/// Notification type constants.
pub mod notification_types {
    pub const ORDER_STATUS_CHANGED: &str = "order_status_changed";
    pub const SHIPPING_APPROVAL_REQUIRED: &str = "shipping_approval_required";
    pub const PAYMENT_CAPTURED: &str = "payment_captured";
    pub const REFUND_ISSUED: &str = "refund_issued";
    pub const NEW_MESSAGE: &str = "new_message";
    pub const STOCK_BACK_IN: &str = "stock_back_in";
    pub const LOW_STOCK: &str = "low_stock";
    pub const PERISHABLE_ORDER_URGENT: &str = "perishable_order_urgent";
    pub const NEW_QUESTION: &str = "new_question";
    pub const QUESTION_ANSWERED: &str = "question_answered";
    pub const RETURN_REQUESTED: &str = "return_requested";
    pub const RETURN_APPROVED: &str = "return_approved";
    pub const RETURN_REJECTED: &str = "return_rejected";
    pub const RETURN_ESCALATED_ADMIN: &str = "return_escalated_admin";
    pub const TRENDING_PRODUCT: &str = "trending_product";
    pub const SUBSCRIPTION_RENEWAL: &str = "subscription_renewal";
    pub const SELLER_SUSPENDED: &str = "seller_suspended";
    pub const ABANDONED_CART: &str = "abandoned_cart";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_status_as_str() {
        let statuses = vec![
            OrderStatus::PendingPayment,
            OrderStatus::PaymentAuthorized,
            OrderStatus::AwaitingShippingApproval,
            OrderStatus::Processing,
            OrderStatus::Shipped,
            OrderStatus::Delivered,
            OrderStatus::Cancelled,
            OrderStatus::Refunded,
            OrderStatus::Disputed,
            OrderStatus::Expired,
            OrderStatus::Failed,
            OrderStatus::ReturnRequested,
            OrderStatus::ReturnApproved,
            OrderStatus::ReturnRejected,
            OrderStatus::Returned,
            OrderStatus::Archived,
        ];
        for s in statuses {
            assert!(!s.as_str().is_empty());
        }
    }

    #[test]
    fn test_order_status_terminal() {
        assert!(OrderStatus::Delivered.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(!OrderStatus::Processing.is_terminal());
        assert!(!OrderStatus::Shipped.is_terminal());
    }

    #[test]
    fn test_collections_not_empty() {
        assert!(!collections::USERS.is_empty());
        assert!(!collections::PRODUCTS.is_empty());
        assert!(!collections::ORDERS.is_empty());
    }

    #[test]
    fn test_business_rules_values() {
        assert_eq!(business_rules::FREE_SHIPPING_THRESHOLD_CENTS, 7500);
        assert_eq!(business_rules::DEFAULT_COMMISSION_RATE_BPS, 250);
        assert_eq!(business_rules::RETURN_WINDOW_DAYS, 30);
    }

    #[test]
    fn test_order_status_serde_roundtrip() {
        let status = OrderStatus::AwaitingShippingApproval;
        let json = serde_json::to_string(&status).unwrap();
        let back: OrderStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, back);
    }

    #[test]
    fn test_province_code_serde() {
        let prov = ProvinceCode::ON;
        let json = serde_json::to_string(&prov).unwrap();
        assert!(json.contains("ON"));
    }

    #[test]
    fn test_payment_status_as_str() {
        assert_eq!(PaymentStatus::Pending.as_str(), "PENDING");
        assert_eq!(PaymentStatus::Authorized.as_str(), "AUTHORIZED");
        assert_eq!(PaymentStatus::Captured.as_str(), "CAPTURED");
        assert_eq!(PaymentStatus::Refunded.as_str(), "REFUNDED");
        assert_eq!(PaymentStatus::PartialRefund.as_str(), "PARTIAL_REFUND");
        assert_eq!(PaymentStatus::Failed.as_str(), "FAILED");
        assert_eq!(PaymentStatus::Cancelled.as_str(), "CANCELLED");
        assert_eq!(PaymentStatus::Disputed.as_str(), "DISPUTED");
        assert_eq!(PaymentStatus::Expired.as_str(), "EXPIRED");
    }

    #[test]
    fn test_subscription_status_as_str() {
        assert_eq!(SubscriptionStatus::Active.as_str(), "active");
        assert_eq!(SubscriptionStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(SubscriptionStatus::CancelPending.as_str(), "cancel_pending");
        assert_eq!(SubscriptionStatus::PastDue.as_str(), "past_due");
        assert_eq!(SubscriptionStatus::Expired.as_str(), "expired");
    }

    #[test]
    fn test_exhaustive_enum_serde_roundtrips() {
        let roles = vec![UserRole::Buyer, UserRole::Seller, UserRole::Admin];
        for r in roles {
            let json = serde_json::to_string(&r).unwrap();
            let back: UserRole = serde_json::from_str(&json).unwrap();
            assert_eq!(r, back);
        }

        let return_statuses = vec![
            ReturnRequestStatus::Requested,
            ReturnRequestStatus::Approved,
            ReturnRequestStatus::Rejected,
            ReturnRequestStatus::Completed,
        ];
        for s in return_statuses {
            let json = serde_json::to_string(&s).unwrap();
            let back: ReturnRequestStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }

        let coupon_types = vec![
            CouponType::Percentage,
            CouponType::FixedAmount,
            CouponType::FreeShipping,
        ];
        for c in coupon_types {
            let json = serde_json::to_string(&c).unwrap();
            let back: CouponType = serde_json::from_str(&json).unwrap();
            assert_eq!(c, back);
        }

        let filter_values = vec![
            FilterValue::Recent,
            FilterValue::Popular,
            FilterValue::PriceLowToHigh,
            FilterValue::PriceHighToLow,
            FilterValue::TopRated,
        ];
        for f in filter_values {
            let json = serde_json::to_string(&f).unwrap();
            let back: FilterValue = serde_json::from_str(&json).unwrap();
            assert_eq!(f, back);
        }

        let provinces = vec![
            ProvinceCode::AB,
            ProvinceCode::BC,
            ProvinceCode::MB,
            ProvinceCode::NB,
            ProvinceCode::NL,
            ProvinceCode::NS,
            ProvinceCode::NT,
            ProvinceCode::NU,
            ProvinceCode::ON,
            ProvinceCode::PE,
            ProvinceCode::QC,
            ProvinceCode::SK,
            ProvinceCode::YT,
        ];
        for p in provinces {
            let json = serde_json::to_string(&p).unwrap();
            let back: ProvinceCode = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn test_order_status_is_terminal_exhaustive() {
        assert!(!OrderStatus::PendingPayment.is_terminal());
        assert!(!OrderStatus::PaymentAuthorized.is_terminal());
        assert!(!OrderStatus::AwaitingShippingApproval.is_terminal());
        assert!(!OrderStatus::Processing.is_terminal());
        assert!(!OrderStatus::Shipped.is_terminal());
        assert!(OrderStatus::Delivered.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(!OrderStatus::Refunded.is_terminal());
        assert!(OrderStatus::Disputed.is_terminal());
        assert!(OrderStatus::Expired.is_terminal());
        assert!(OrderStatus::Failed.is_terminal());
        assert!(!OrderStatus::ReturnRequested.is_terminal());
        assert!(!OrderStatus::ReturnApproved.is_terminal());
        assert!(!OrderStatus::ReturnRejected.is_terminal());
        assert!(!OrderStatus::Returned.is_terminal());
        assert!(!OrderStatus::Archived.is_terminal());
    }

    #[test]
    fn test_all_constants_accessed() {
        let _ = collections::USERS;
        let _ = collections::PRODUCTS;
        let _ = collections::ORDERS;
        let _ = collections::REVIEWS;
        let _ = collections::PAYOUTS;
        let _ = collections::REFUNDS;
        let _ = collections::WEBHOOK_LOGS;
        let _ = collections::WEBHOOK_EVENTS;
        let _ = collections::SECURITY_ALERTS;
        let _ = collections::RATE_LIMITS;
        let _ = collections::CONFIG;
        let _ = collections::ADMIN_LOGS;
        let _ = collections::PRODUCT_RATINGS;
        let _ = collections::SELLER_RATINGS;
        let _ = collections::REVIEW_VOTES;
        let _ = collections::MEILISEARCH_SYNC_FAILURES;
        let _ = collections::CRON_LOCKS;
        let _ = collections::CRON_FAILURES;
        let _ = collections::RETURN_REQUESTS;
        let _ = collections::PENDING_PROFILES;
        let _ = collections::WAREHOUSES;
        let _ = collections::CART;
        let _ = collections::FAVORITES;
        let _ = collections::NOTIFICATIONS;
        let _ = collections::FCM_TOKENS;
        let _ = collections::LICENSES;
        let _ = collections::BOOK_ACCESS_TOKENS;
        let _ = collections::SOFTWARE_ACCESS_TOKENS;
        let _ = collections::ADDRESSES;
        let _ = collections::BUYER_ADDRESSES;
        let _ = collections::DOWNLOAD_SESSIONS;
        let _ = collections::STOCK_NOTIFICATIONS;
        let _ = collections::PRODUCT_QUESTIONS;
        let _ = collections::SELLER_METRICS;
        let _ = collections::COUPONS;
        let _ = collections::INVENTORY_LEVELS;
        let _ = collections::ORDER_EVENTS;
        let _ = collections::COUPON_USES;
        let _ = collections::USER_SECURITY;
        let _ = collections::SELLER_PROFILES;
        let _ = collections::SELLER_SKUS;
        let _ = collections::MAIL_LOGS;
        let _ = collections::PENDING_REDEMPTIONS;
        let _ = collections::SUBSCRIPTIONS;
        let _ = collections::CHATS;
        let _ = collections::CHAT_MESSAGES;
        let _ = collections::PLATFORM_DEBT;
        let _ = collections::MESSAGE_REPORTS;
        let _ = collections::DISPUTES;

        let _ = documents::PAYMENT_PROVIDERS;

        let _ = APP_NAME;
        let _ = COUNTRY_CANADA;

        let _ = email_config::SUPPORT_EMAIL;
        let _ = email_config::SENDER_NAME;
        let _ = email_config::SENDER_NAME_SECURITY;
        let _ = email_config::COPYRIGHT_TEXT;
        let _ = email_config::APP_TAGLINE;
        let _ = email_config::URL_PROD;
        let _ = email_config::URL_STAGING;
        let _ = email_config::URL_DEV;
        let _ = email_config::MAILJET_API_VERSION;
        let _ = email_config::PHYSICAL_ADDRESS;
        let _ = email_config::GST_HST_NUMBER;
        let _ = email_config::UNSUBSCRIBE_URL_PROD;
        let _ = email_config::UNSUBSCRIBE_URL_STAGING;
        let _ = email_config::UNSUBSCRIBE_URL_DEV;
        let _ = email_config::PRIVACY_OFFICER_EMAIL;
        let _ = email_config::PRIVACY_OFFICER_NAME;

        let _ = app_config::PLATFORM_NAME;
        let _ = app_config::DEFAULT_COUNTRY_CODE;
        let _ = app_config::DEFAULT_COUNTRY_NAME;
        let _ = app_config::API_TIMEOUT_SECONDS;
        let _ = app_config::GEOAPIFY_TIMEOUT_SECONDS;
        let _ = app_config::TOKEN_CACHE_MINUTES;
        let _ = app_config::MEILISEARCH_MAX_RETRIES;
        let _ = app_config::MEILISEARCH_HITS_PER_PAGE;
        let _ = app_config::SITE_URL;
        let _ = app_config::CHECKOUT_SUCCESS_PATH;
        let _ = app_config::CHECKOUT_CANCEL_PATH;
        let _ = app_config::SELLER_REFRESH_PATH;
        let _ = app_config::SELLER_RETURN_PATH;
        let _ = app_config::CORS_ORIGINS;

        let _ = external_urls::SUPPORT_CHAT;
        let _ = external_urls::PRIVACY_POLICY;
        let _ = external_urls::TERMS_OF_SERVICE;
        let _ = external_urls::REFUND_POLICY;

        let _ = fields::SAVED_AT;
        let _ = fields::CREATED_AT;
        let _ = fields::UPDATED_AT;
        let _ = fields::VERSION;
        let _ = fields::DELETED_AT;
        let _ = fields::DELETED_BY;
        let _ = fields::DELETED;
        let _ = fields::UID;
        let _ = fields::EMAIL;
        let _ = fields::NAME;
        let _ = fields::ROLES;
        let _ = fields::ADDRESS;
        let _ = fields::SELLER_PROFILE;
        let _ = fields::BUSINESS_ADDRESS;
        let _ = fields::CUSTOMER_ID;
        let _ = fields::STRIPE_ACCOUNT_ID;
        let _ = fields::PAYOUTS_ENABLED;
        let _ = fields::CHARGES_ENABLED;
        let _ = fields::ONBOARDING_COMPLETED;
        let _ = fields::SUSPENDED;
        let _ = fields::SUSPENDED_AT;
        let _ = fields::SUSPENDED_BY;
        let _ = fields::SUSPENSION_REASON;
        let _ = fields::COMMISSION_RATE_BPS;
        let _ = fields::MFA_ENABLED;
        let _ = fields::EMAIL_CONSENT;
        let _ = fields::LANGUAGE;
        let _ = fields::IS_PREMIUM;
        let _ = fields::PRODUCT_ID;
        let _ = fields::SELLER_ID;
        let _ = fields::BUYER_ID;
        let _ = fields::TITLE;
        let _ = fields::DESCRIPTION;
        let _ = fields::PRICE_CENTS;
        let _ = fields::STOCK_QUANTITY;
        let _ = fields::IMAGE_URLS;
        let _ = fields::CATEGORY;
        let _ = fields::IS_ACTIVE;
        let _ = fields::LIFECYCLE_STATUS;
        let _ = fields::AVG_RATING;
        let _ = fields::TOTAL_REVIEWS;
        let _ = fields::IS_PERISHABLE;
        let _ = fields::ORDER_ID;
        let _ = fields::ORDER_STATUS;
        let _ = fields::RETURN_STATUS;
        let _ = fields::STATUS;
        let _ = fields::ITEMS;
        let _ = fields::TOTAL_AMOUNT_CENTS;
        let _ = fields::SUBTOTAL_CENTS;
        let _ = fields::TAX_AMOUNT_CENTS;
        let _ = fields::SHIPPING_COST_CENTS;
        let _ = fields::PLATFORM_FEE_CENTS;
        let _ = fields::PAYMENT_INTENT_ID;
        let _ = fields::CHECKOUT_SESSION_ID;
        let _ = fields::PAYMENT_STATUS;
        let _ = fields::CUMULATIVE_REFUNDED_CENTS;
        let _ = fields::PARTIAL_REFUND_AMOUNT_CENTS;
        let _ = fields::CANCELLATION_REASON;
        let _ = fields::CUSTOMER_EMAIL;
        let _ = fields::PREFERRED_LANGUAGE;
        let _ = fields::LAST_ACTOR_ID;
        let _ = fields::CART_ITEM_ID;
        let _ = fields::SHIPPING_ADDRESS;
        let _ = fields::TRACKING_NUMBER;
        let _ = fields::SHIPPING_CARRIER;
        let _ = fields::SUBSCRIPTION_ID;
        let _ = fields::STRIPE_SUBSCRIPTION_ID;
        let _ = fields::SUBSCRIPTION_STATUS;
        let _ = fields::CURRENT_PERIOD_END;
        let _ = fields::CHAT_ID;
        let _ = fields::PARTICIPANTS;
        let _ = fields::LAST_MESSAGE;
        let _ = fields::LAST_MESSAGE_AT;
        let _ = fields::UNREAD_COUNT;
        let _ = fields::MESSAGE_TEXT;
        let _ = fields::SENDER_ID;
        let _ = fields::READ;
        let _ = fields::RATING;
        let _ = fields::REVIEW_TEXT;
        let _ = fields::HELPFUL_COUNT;
        let _ = fields::CODE;
        let _ = fields::COUPON_TYPE;
        let _ = fields::DISCOUNT_VALUE;
        let _ = fields::MIN_ORDER_CENTS;
        let _ = fields::MAX_USES;
        let _ = fields::USED_COUNT;
        let _ = fields::EXPIRES_AT;
        let _ = fields::LICENSE_KEY;
        let _ = fields::DEVICE_ID;
        let _ = fields::MAX_DEVICES;
        let _ = fields::ACTIVATED_DEVICES;
        let _ = fields::LABEL;
        let _ = fields::STREET;
        let _ = fields::CITY;
        let _ = fields::PROVINCE;
        let _ = fields::POSTAL_CODE;
        let _ = fields::COUNTRY;
        let _ = fields::APARTMENT;
        let _ = fields::IS_DEFAULT;
        let _ = fields::LATITUDE;
        let _ = fields::LONGITUDE;
        let _ = fields::FCM_TOKEN;
        let _ = fields::TOKEN;
        let _ = fields::PLATFORM;
        let _ = fields::NOTIFICATION_TYPE;

        let _ = business_rules::FREE_SHIPPING_THRESHOLD_CENTS;
        let _ = business_rules::DEFAULT_COMMISSION_RATE_BPS;
        let _ = business_rules::PREMIUM_SUBSCRIPTION_PRICE_CAD;
        let _ = business_rules::RETURN_WINDOW_DAYS;
        let _ = business_rules::AUTO_ARCHIVE_DAYS;
        let _ = business_rules::AUTHORIZATION_EXPIRY_DAYS;
        let _ = business_rules::MAX_PUSH_PER_DAY;
        let _ = business_rules::ABANDONED_CART_HOURS;
        let _ = business_rules::ABANDONED_CART_COOLDOWN_HOURS;
        let _ = business_rules::LOW_STOCK_ALERT_COOLDOWN_HOURS;
        let _ = business_rules::RATE_LIMIT_STALE_HOURS;
        let _ = business_rules::WEBHOOK_EVENT_RETENTION_DAYS;
        let _ = business_rules::SECURITY_ALERT_ARCHIVE_DAYS;
        let _ = business_rules::ORPHAN_IMAGE_SAFETY_HOURS;
        let _ = business_rules::MAX_VIDEO_BYTES;
        let _ = business_rules::MAX_VIDEO_DURATION_SECONDS;
        let _ = business_rules::MEILISEARCH_DLQ_MAX_RETRIES;
        let _ = business_rules::RETURN_ESCALATION_DAYS;
        let _ = business_rules::LICENSE_KEY_PATTERN;
        let _ = business_rules::DOWNLOAD_TOKEN_MINUTES;
        let _ = business_rules::MAX_DEVICES_PER_LICENSE;
        let _ = business_rules::TRENDING_VIEW_WEIGHT;
        let _ = business_rules::TRENDING_PURCHASE_WEIGHT;
        let _ = business_rules::TRENDING_FAVORITE_WEIGHT;
        let _ = business_rules::TRENDING_WINDOW_HOURS;
        let _ = business_rules::LOCAL_DELIVERY_RADIUS_KM;

        let _ = cancellation_reasons::BUYER_REQUESTED;
        let _ = cancellation_reasons::SELLER_CANCELLED;
        let _ = cancellation_reasons::SHIPPING_REJECTED;
        let _ = cancellation_reasons::PAYMENT_FAILED;
        let _ = cancellation_reasons::EXPIRED;

        let _ = notification_types::ORDER_STATUS_CHANGED;
        let _ = notification_types::SHIPPING_APPROVAL_REQUIRED;
        let _ = notification_types::PAYMENT_CAPTURED;
        let _ = notification_types::REFUND_ISSUED;
        let _ = notification_types::NEW_MESSAGE;
        let _ = notification_types::STOCK_BACK_IN;
        let _ = notification_types::LOW_STOCK;
        let _ = notification_types::PERISHABLE_ORDER_URGENT;
        let _ = notification_types::NEW_QUESTION;
        let _ = notification_types::QUESTION_ANSWERED;
        let _ = notification_types::RETURN_REQUESTED;
        let _ = notification_types::RETURN_APPROVED;
        let _ = notification_types::RETURN_REJECTED;
        let _ = notification_types::RETURN_ESCALATED_ADMIN;
        let _ = notification_types::TRENDING_PRODUCT;
        let _ = notification_types::SUBSCRIPTION_RENEWAL;
        let _ = notification_types::SELLER_SUSPENDED;
        let _ = notification_types::ABANDONED_CART;
    }
}
