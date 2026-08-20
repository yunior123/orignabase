//! Shared schema constants for all OrignaBase crates.
//!
//! These constants mirror `ob-handlers::shared::schema::fields` so that crates
//! which cannot depend on `ob-handlers` (e.g. ob-mcp, ob-admin, ob-auth,
//! ob-realtime, ob-search, ob-notifications) still avoid magic strings.

// ── Collections ──────────────────────────────────────────────────────────────

pub mod collections {
    pub const USERS: &str = "users";
    pub const PRODUCTS: &str = "products";
    pub const ORDERS: &str = "orders";
    pub const REVIEWS: &str = "reviews";
    pub const RETURN_REQUESTS: &str = "return_requests";
    pub const CART: &str = "cart";
    pub const NOTIFICATIONS: &str = "notifications";
    pub const COUPONS: &str = "coupons";
    pub const FCM_TOKENS: &str = "fcm_tokens";
    pub const SELLER_PROFILES: &str = "seller_profiles";
    pub const WAREHOUSES: &str = "warehouses";
    pub const CONFIG: &str = "config";
    pub const ADMIN_LOGS: &str = "admin_logs";
    pub const SUBSCRIPTIONS: &str = "subscriptions";
    pub const CHAT_MESSAGES: &str = "chat_messages";
    pub const CHAT_THREADS: &str = "chat_threads";
    pub const PUSH_TOKENS: &str = "_push_tokens";
    pub const LINKS: &str = "links";
    pub const METRICS: &str = "metrics";
    pub const ANALYTICS_EVENTS: &str = "analytics_events";
}

// ── Field names (DB / document keys) ─────────────────────────────────────────

pub mod fields {
    // Common timestamps
    pub const CREATED_AT: &str = "createdAt";
    pub const DATE_CREATED: &str = "dateCreated";
    pub const UPDATED_AT: &str = "updatedAt";
    pub const TIMESTAMP: &str = "timestamp";

    // Common record fields
    pub const ID: &str = "id";
    pub const STATUS: &str = "status";
    pub const NAME: &str = "name";
    pub const DESCRIPTION: &str = "description";
    pub const ROLES: &str = "roles";
    pub const EMAIL: &str = "email";
    pub const EMAIL_VERIFIED: &str = "email_verified";

    // Identity fields
    pub const UID: &str = "uid";
    pub const USER_ID: &str = "userId";
    pub const SELLER_ID: &str = "sellerId";
    pub const BUYER_ID: &str = "buyerId";
    pub const PARENT_ID: &str = "parent_id";
    pub const ADMIN_ID: &str = "admin_id";
    pub const TARGET_ID: &str = "target_id";

    // Product fields
    pub const PRODUCT_ID: &str = "productId";
    pub const PRICE_CENTS: &str = "priceCents";
    pub const STOCK_QUANTITY: &str = "stockQuantity";
    pub const LIFECYCLE_STATUS: &str = "lifecycleStatus";
    pub const CATEGORY_ID: &str = "categoryId";
    pub const SUBCATEGORY: &str = "subcategory";
    pub const IS_PERISHABLE: &str = "isPerishable";
    pub const IS_DIGITAL: &str = "isDigital";
    pub const IMAGE_URLS: &str = "imageUrls";
    pub const AVG_RATING: &str = "avgRating";
    pub const TOTAL_REVIEWS: &str = "totalReviews";
    pub const SLUG: &str = "slug";
    pub const COMPARE_AT_PRICE_CENTS: &str = "compareAtPriceCents";
    pub const IS_LOCAL_DELIVERY_ONLY: &str = "isLocalDeliveryOnly";
    pub const KEYWORDS: &str = "keywords";

    // Food & Nutrition fields (search-indexed)
    pub const DIETARY_BADGES: &str = "dietaryBadges";
    pub const ALLERGENS: &str = "allergens";
    pub const MAY_CONTAIN_ALLERGENS: &str = "mayContainAllergens";
    pub const FOP_HIGH_SODIUM: &str = "fopHighSodium";
    pub const FOP_HIGH_SUGARS: &str = "fopHighSugars";
    pub const FOP_HIGH_SATURATED_FAT: &str = "fopHighSaturatedFat";

    // Product specs (denormalized for search)
    pub const BRAND: &str = "brand";
    pub const COLOR: &str = "color";
    pub const MATERIAL: &str = "material";

    // Order fields
    pub const ORDER_ID: &str = "orderId";
    pub const ORDER_STATUS: &str = "orderStatus";
    pub const ITEMS: &str = "items";
    pub const TOTAL_AMOUNT_CENTS: &str = "totalAmountCents";
    pub const SUBTOTAL_CENTS: &str = "subtotalCents";
    pub const TAX_AMOUNT_CENTS: &str = "taxAmountCents";
    pub const SHIPPING_COST_CENTS: &str = "shippingCostCents";
    pub const PLATFORM_FEE_CENTS: &str = "platformFeeTotalCents";
    pub const PAYMENT_STATUS: &str = "paymentStatus";
    pub const REASON: &str = "reason";
    pub const CANCELLATION_REASON: &str = "cancellationReason";
    pub const SHIPPING_ADDRESS: &str = "shippingAddress";

    // Cart / checkout
    pub const QUANTITY: &str = "quantity";

    // Auth fields
    pub const CUSTOM_CLAIMS: &str = "custom_claims";
    pub const MFA_ENABLED: &str = "mfaEnabled";
    pub const MFA_SECRET: &str = "mfa_secret";
    pub const MFA_RECOVERY_CODES: &str = "mfa_recovery_codes";
    pub const MFA_LAST_USED_STEP: &str = "mfa_last_used_step";
    pub const REFRESH_TOKEN: &str = "refresh_token";
    pub const PASSWORD_HASH: &str = "passwordHash";
    pub const PROVIDER: &str = "provider";
    pub const PROVIDER_ID: &str = "provider_id";
    pub const EMAIL_VERIFICATION_TOKEN: &str = "email_verification_token";
    pub const EMAIL_VERIFICATION_EXPIRES: &str = "email_verification_expires";

    // FCM / Notification
    pub const TOKEN: &str = "token";
    pub const PUSH_TOKEN: &str = "push_token";
    pub const TOPIC: &str = "topic";

    // Search / Meilisearch
    pub const ORIG_ID: &str = "origId";
    pub const VALUE: &str = "value";

    // Admin dashboard
    pub const KEY: &str = "key";
    pub const TYPE: &str = "type";
    pub const CLICKS: &str = "clicks";
    pub const TARGET_URL: &str = "target_url";
    pub const TAGS: &str = "tags";

    // Analytics
    pub const COUNT: &str = "count";
    pub const EVENT: &str = "event";
    pub const PATH: &str = "path";
    pub const MESSAGE: &str = "message";
}

// ── MCP tool parameter names ────────────────────────────────────────────────
// These are JSON-RPC parameter keys for the MCP protocol, not DB field names.

pub mod mcp_params {
    pub const STATUS: &str = "status";
    pub const LIMIT: &str = "limit";
    pub const OFFSET: &str = "offset";
    pub const ORDER_ID: &str = "order_id";
    pub const PRODUCT_ID: &str = "product_id";
    pub const QUANTITY: &str = "quantity";
    pub const REASON: &str = "reason";
    pub const ITEMS: &str = "items";
    pub const SHIPPING_ADDRESS: &str = "shipping_address";
    pub const IDEMPOTENCY_KEY: &str = "idempotency_key";
    pub const PRICE_CENTS: &str = "price_cents";
    pub const CODE: &str = "code";
    pub const QUERY: &str = "query";
    pub const CATEGORY: &str = "category";
    pub const MIN_PRICE: &str = "min_price";
    pub const MAX_PRICE: &str = "max_price";
}
