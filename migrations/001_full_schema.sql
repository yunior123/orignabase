-- =============================================================================
-- OrignaBase — Full PostgreSQL Schema
-- Generated: 2026-03-28
-- =============================================================================

BEGIN;

-- ---------------------------------------------------------------------------
-- 0. Auto-update trigger function
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Helper: create trigger for a table
CREATE OR REPLACE FUNCTION _apply_updated_at(tbl regclass)
RETURNS void AS $$
BEGIN
    EXECUTE format(
        'CREATE TRIGGER trg_%s_updated_at
         BEFORE UPDATE ON %s
         FOR EACH ROW EXECUTE FUNCTION set_updated_at()',
        regexp_replace(tbl::text, '\.', '_'),
        tbl
    );
END;
$$ LANGUAGE plpgsql;

-- ---------------------------------------------------------------------------
-- 1. USER DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           TEXT NOT NULL,
    name            TEXT,
    roles           TEXT[] DEFAULT '{}',
    address         JSONB,
    customer_id     TEXT,
    stripe_account_id       TEXT,
    payouts_enabled         BOOL DEFAULT false,
    charges_enabled         BOOL DEFAULT false,
    onboarding_completed    BOOL DEFAULT false,
    suspended               BOOL DEFAULT false,
    suspended_at            TIMESTAMPTZ,
    suspended_by            TEXT,
    suspension_reason       TEXT,
    commission_rate_bps     INT,
    mfa_enabled             BOOL DEFAULT false,
    email_consent           BOOL DEFAULT true,
    preferred_language      TEXT DEFAULT 'en',
    is_premium              BOOL DEFAULT false,
    premium_since           TIMESTAMPTZ,
    premium_expires_at      TIMESTAMPTZ,
    marketing_opt_in        BOOL DEFAULT false,
    last_low_stock_alert_at     TIMESTAMPTZ,
    last_cart_abandon_email_at  TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE UNIQUE INDEX idx_users_email ON users (lower(email));

CREATE TABLE user_security (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    mfa_secret          TEXT,
    backup_codes        TEXT[] DEFAULT '{}',
    last_login_at       TIMESTAMPTZ,
    login_count         INT DEFAULT 0,
    failed_login_count  INT DEFAULT 0,
    last_failed_login_at TIMESTAMPTZ,
    password_hash       TEXT,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE UNIQUE INDEX idx_user_security_user_id ON user_security (user_id);

CREATE TABLE pending_profiles (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email       TEXT NOT NULL UNIQUE,
    name        TEXT,
    roles       TEXT[] DEFAULT '{}',
    token       TEXT,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE addresses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    label       TEXT,
    street      TEXT,
    apartment   TEXT,
    city        TEXT,
    state       TEXT,
    postal_code TEXT,
    country     TEXT,
    is_default  BOOL DEFAULT false,
    latitude    DOUBLE PRECISION,
    longitude   DOUBLE PRECISION,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_addresses_user ON addresses (user_id);

CREATE TABLE buyer_addresses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    label       TEXT,
    street      TEXT,
    apartment   TEXT,
    city        TEXT,
    state       TEXT,
    postal_code TEXT,
    country     TEXT,
    is_default  BOOL DEFAULT false,
    latitude    DOUBLE PRECISION,
    longitude   DOUBLE PRECISION,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_buyer_addresses_user ON buyer_addresses (user_id);

CREATE TABLE fcm_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    platform    TEXT,
    device_id   TEXT,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_fcm_tokens_user ON fcm_tokens (user_id);

-- ---------------------------------------------------------------------------
-- 2. PRODUCT DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE products (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                    TEXT NOT NULL,
    description             TEXT,
    price_cents             INT NOT NULL,
    compare_at_price_cents  INT,
    stock_quantity          INT DEFAULT 0,
    image_urls              TEXT[] DEFAULT '{}',
    category_id             INT,
    is_active               BOOL DEFAULT true,
    lifecycle_status        TEXT DEFAULT 'draft',
    avg_rating              DOUBLE PRECISION DEFAULT 0,
    total_reviews           INT DEFAULT 0,
    is_perishable           BOOL DEFAULT false,
    is_age_restricted       BOOL DEFAULT false,
    is_digital              BOOL DEFAULT false,
    product_type            TEXT,
    estimated_ship_days     INT,
    minimum_order_quantity  INT DEFAULT 1,
    low_stock_threshold     INT DEFAULT 5,
    track_quantity          BOOL DEFAULT true,
    tags                    TEXT[] DEFAULT '{}',
    video_url               TEXT,
    video_duration_seconds  INT,
    variants                JSONB,
    nutrition_facts         JSONB,
    food_metadata           JSONB,
    specs                   JSONB,
    cost_cents              INT,
    trending_score          INT DEFAULT 0,
    view_count              INT DEFAULT 0,
    purchase_count          INT DEFAULT 0,
    favorite_count          INT DEFAULT 0,
    date_created            TIMESTAMPTZ DEFAULT now(),
    last_trending_at        TIMESTAMPTZ,
    created_at              TIMESTAMPTZ DEFAULT now(),
    updated_at              TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_products_seller ON products (seller_id);
CREATE INDEX idx_products_category ON products (category_id);
CREATE INDEX idx_products_status ON products (lifecycle_status);
CREATE INDEX idx_products_price ON products (price_cents);
CREATE INDEX idx_products_created ON products (date_created DESC);
CREATE INDEX idx_products_tags ON products USING GIN (tags);
CREATE INDEX idx_products_images ON products USING GIN (image_urls);

CREATE TABLE favorites (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id  UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE (user_id, product_id)
);
CREATE INDEX idx_favorites_user ON favorites (user_id);
CREATE INDEX idx_favorites_product ON favorites (product_id);

CREATE TABLE reviews (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id      UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    seller_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    buyer_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    order_id        UUID,
    rating          INT NOT NULL CHECK (rating BETWEEN 1 AND 5),
    review_text     TEXT,
    helpful_count   INT DEFAULT 0,
    image_urls      TEXT[] DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_reviews_product ON reviews (product_id);
CREATE INDEX idx_reviews_buyer ON reviews (buyer_id);

CREATE TABLE product_ratings (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id          UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE UNIQUE,
    avg_rating          DOUBLE PRECISION DEFAULT 0,
    total_reviews       INT DEFAULT 0,
    rating_distribution JSONB,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE seller_ratings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    avg_rating      DOUBLE PRECISION DEFAULT 0,
    total_reviews   INT DEFAULT 0,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE review_votes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    review_id   UUID NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    vote        INT NOT NULL CHECK (vote IN (-1, 1)),
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE (review_id, user_id)
);

CREATE TABLE product_questions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id  UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    seller_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    buyer_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    question    TEXT NOT NULL,
    answer      TEXT,
    answered_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_product_questions_product ON product_questions (product_id);

CREATE TABLE product_recommendations (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id          UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE UNIQUE,
    recommendations     JSONB,
    recommendation_type TEXT,
    updated_at          TIMESTAMPTZ DEFAULT now(),
    created_at          TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE user_recommendations (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    product_ids UUID[] DEFAULT '{}',
    updated_at  TIMESTAMPTZ DEFAULT now(),
    created_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE stock_notifications (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id  UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    notified    BOOL DEFAULT false,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE (product_id, user_id)
);
CREATE INDEX idx_stock_notifications_product ON stock_notifications (product_id) WHERE NOT notified;

-- ---------------------------------------------------------------------------
-- 3. ORDER DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE orders (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    buyer_id                    UUID NOT NULL REFERENCES users(id),
    order_status                TEXT DEFAULT 'pending',
    payment_status              TEXT DEFAULT 'unpaid',
    items                       JSONB NOT NULL DEFAULT '[]',
    total_amount_cents          INT NOT NULL DEFAULT 0,
    subtotal_cents              INT DEFAULT 0,
    tax_amount_cents            INT DEFAULT 0,
    shipping_cost_cents         INT DEFAULT 0,
    platform_fee_total_cents    INT DEFAULT 0,
    discount_amount_cents       INT DEFAULT 0,
    cumulative_refunded_cents   INT DEFAULT 0,
    partial_refund_amount_cents INT DEFAULT 0,
    stripe_session_id           TEXT,
    stripe_payment_intent_id    TEXT,
    payment_intent_id           TEXT,
    shipping_address            JSONB,
    delivery_speed              TEXT,
    delivery_instructions       TEXT,
    customer_email              TEXT,
    preferred_language          TEXT DEFAULT 'en',
    last_actor_id               TEXT,
    cancellation_reason         TEXT,
    tracking_number             TEXT,
    shipping_carrier            TEXT,
    shipping_approval           JSONB,
    shipped_at                  TIMESTAMPTZ,
    delivered_at                TIMESTAMPTZ,
    archived                    BOOL DEFAULT false,
    archived_at                 TIMESTAMPTZ,
    confirmed_at                TIMESTAMPTZ,
    confirmed_by_client         BOOL DEFAULT false,
    version                     INT DEFAULT 1,
    schema_version              INT DEFAULT 1,
    created_at                  TIMESTAMPTZ DEFAULT now(),
    updated_at                  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_orders_buyer ON orders (buyer_id);
CREATE INDEX idx_orders_status ON orders (order_status);
CREATE INDEX idx_orders_created ON orders (created_at DESC);
CREATE INDEX idx_orders_stripe_session ON orders (stripe_session_id);
CREATE INDEX idx_orders_stripe_pi ON orders (stripe_payment_intent_id);

CREATE TABLE order_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id    UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    user_id     UUID REFERENCES users(id),
    event_type  TEXT NOT NULL,
    description TEXT,
    metadata    JSONB,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_order_events_order ON order_events (order_id);

CREATE TABLE return_requests (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id            UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id          UUID,
    buyer_id            UUID NOT NULL REFERENCES users(id),
    seller_id           UUID NOT NULL REFERENCES users(id),
    status              TEXT DEFAULT 'pending',
    reason              TEXT,
    quantity            INT DEFAULT 1,
    refund_amount_cents INT DEFAULT 0,
    images              TEXT[] DEFAULT '{}',
    resolution          TEXT,
    escalated_at        TIMESTAMPTZ,
    escalation_reason   TEXT,
    requested_at        TIMESTAMPTZ DEFAULT now(),
    resolved_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_return_requests_order ON return_requests (order_id);

CREATE TABLE refunds (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id            UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    stripe_refund_id    TEXT,
    amount_cents        INT NOT NULL,
    reason              TEXT,
    status              TEXT DEFAULT 'pending',
    stock_restored      BOOL DEFAULT false,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_refunds_order ON refunds (order_id);

CREATE TABLE disputes (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id            UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    stripe_dispute_id   TEXT,
    amount_cents        INT NOT NULL,
    reason              TEXT,
    status              TEXT DEFAULT 'open',
    evidence_due_at     TIMESTAMPTZ,
    resolved_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_disputes_order ON disputes (order_id);

CREATE TABLE coupons (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code            TEXT NOT NULL UNIQUE,
    coupon_type     TEXT,
    discount_value  DOUBLE PRECISION,
    min_order_cents INT DEFAULT 0,
    max_uses_total  INT,
    used_count      INT DEFAULT 0,
    expires_at      TIMESTAMPTZ,
    is_active       BOOL DEFAULT true,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE coupon_uses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    coupon_id   UUID NOT NULL REFERENCES coupons(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    order_id    UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    used_at     TIMESTAMPTZ DEFAULT now(),
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_coupon_uses_coupon ON coupon_uses (coupon_id);
CREATE INDEX idx_coupon_uses_user ON coupon_uses (user_id);

CREATE TABLE pending_redemptions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    coupon_code TEXT,
    order_id    UUID REFERENCES orders(id) ON DELETE CASCADE,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_pending_redemptions_user ON pending_redemptions (user_id);

-- ---------------------------------------------------------------------------
-- 4. CART DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE cart (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id      UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    variant_id      TEXT DEFAULT '',
    variant_title   TEXT,
    variant_options  JSONB,
    variant_sku     TEXT,
    quantity        INT NOT NULL DEFAULT 1 CHECK (quantity > 0),
    price_snapshot  INT NOT NULL,
    product_name    TEXT,
    product_description TEXT,
    image_urls      TEXT[] DEFAULT '{}',
    buyer_note      TEXT,
    parent_id       TEXT,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    UNIQUE (user_id, product_id, variant_id)
);
CREATE INDEX idx_cart_user ON cart (user_id);
CREATE INDEX idx_cart_product ON cart (product_id);
CREATE INDEX idx_cart_user_product ON cart (user_id, product_id);

-- ---------------------------------------------------------------------------
-- 5. SELLER DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE seller_profiles (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    business_name       TEXT,
    description         TEXT,
    logo_url            TEXT,
    banner_url          TEXT,
    commission_rate_bps INT DEFAULT 250,
    total_sales         INT DEFAULT 0,
    total_reviews       INT DEFAULT 0,
    avg_rating          DOUBLE PRECISION DEFAULT 0,
    return_window_days  INT DEFAULT 30,
    payout_hold_days    INT DEFAULT 0,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE seller_metrics (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    total_revenue_cents     INT DEFAULT 0,
    total_orders            INT DEFAULT 0,
    total_items_sold        INT DEFAULT 0,
    dispute_rate            DOUBLE PRECISION DEFAULT 0,
    refund_rate             DOUBLE PRECISION DEFAULT 0,
    cancellation_rate       DOUBLE PRECISION DEFAULT 0,
    avg_order_value_cents   INT DEFAULT 0,
    computed_at             TIMESTAMPTZ,
    created_at              TIMESTAMPTZ DEFAULT now(),
    updated_at              TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE seller_skus (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id  UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    sku         TEXT,
    warehouse_id UUID,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_seller_skus_seller ON seller_skus (seller_id);
CREATE INDEX idx_seller_skus_product ON seller_skus (product_id);
CREATE UNIQUE INDEX idx_seller_skus_sku ON seller_skus (seller_id, sku) WHERE sku IS NOT NULL;

CREATE TABLE warehouses (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    address     JSONB,
    is_default  BOOL DEFAULT false,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_warehouses_seller ON warehouses (seller_id);

CREATE TABLE inventory_levels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id      UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    warehouse_id    UUID NOT NULL,
    quantity        INT DEFAULT 0,
    reserved        INT DEFAULT 0,
    last_restocked_at TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_inventory_product ON inventory_levels (product_id);
CREATE UNIQUE INDEX idx_inventory_product_warehouse ON inventory_levels (product_id, warehouse_id);

CREATE TABLE payouts (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    order_id            UUID,
    amount_cents        INT NOT NULL,
    stripe_transfer_id  TEXT,
    status              TEXT DEFAULT 'pending',
    payout_date         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_payouts_seller ON payouts (seller_id);

-- ---------------------------------------------------------------------------
-- 6. CHAT DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE chats (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    participants        TEXT[] DEFAULT '{}',
    buyer_id            UUID,
    seller_id           UUID,
    product_id          UUID,
    order_id            UUID,
    last_message        TEXT,
    last_message_at     TIMESTAMPTZ,
    buyer_unread_count  INT DEFAULT 0,
    seller_unread_count INT DEFAULT 0,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_chats_buyer ON chats (buyer_id);
CREATE INDEX idx_chats_seller ON chats (seller_id);

CREATE TABLE messages (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chat_id         UUID NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    sender_id       UUID,
    text            TEXT,
    read            BOOL DEFAULT false,
    image_url       TEXT,
    message_type    TEXT DEFAULT 'text',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_messages_thread ON messages (chat_id, created_at);

CREATE TABLE message_reports (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id  UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    reporter_id UUID NOT NULL,
    reason      TEXT,
    status      TEXT DEFAULT 'pending',
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_message_reports_message ON message_reports (message_id);

-- ---------------------------------------------------------------------------
-- 7. NOTIFICATION DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE notifications (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    notification_type   TEXT,
    title               TEXT,
    body                TEXT,
    data                JSONB,
    is_read             BOOL DEFAULT false,
    read_at             TIMESTAMPTZ,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_notifications_user ON notifications (user_id, is_read);

CREATE TABLE _mail_logs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    to_email            TEXT,
    subject             TEXT,
    template_id         TEXT,
    status              TEXT,
    email_message_id  TEXT,
    error_message       TEXT,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE licenses (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id            UUID,
    product_id          UUID,
    buyer_id            UUID,
    license_key         TEXT NOT NULL UNIQUE,
    device_id           TEXT,
    max_devices         INT DEFAULT 5,
    activated_devices   TEXT[] DEFAULT '{}',
    is_active           BOOL DEFAULT true,
    created_at          TIMESTAMPTZ DEFAULT now(),
    updated_at          TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_licenses_buyer ON licenses (buyer_id);
CREATE INDEX idx_licenses_order ON licenses (order_id);

CREATE TABLE book_access_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    license_id  UUID NOT NULL REFERENCES licenses(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ,
    used        BOOL DEFAULT false,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE software_access_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    license_id  UUID NOT NULL REFERENCES licenses(id) ON DELETE CASCADE,
    device_id   TEXT,
    token       TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE download_sessions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token       TEXT NOT NULL UNIQUE,
    license_id  UUID,
    product_id  UUID,
    buyer_id    UUID,
    expires_at  TIMESTAMPTZ,
    used        BOOL DEFAULT false,
    ip_address  TEXT,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- 8. AUTH / PAYMENT DOMAIN
-- ---------------------------------------------------------------------------

CREATE TABLE webhook_events (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id        TEXT NOT NULL UNIQUE,
    event_type      TEXT,
    payload         JSONB,
    processed       BOOL DEFAULT false,
    processed_at    TIMESTAMPTZ,
    error_message   TEXT,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_webhook_events_type ON webhook_events (event_type);
CREATE INDEX idx_webhook_events_processed ON webhook_events (processed) WHERE NOT processed;

CREATE TABLE webhook_logs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    webhook_id      TEXT,
    event_type      TEXT,
    url             TEXT,
    status_code     INT,
    request_body    TEXT,
    response_body   TEXT,
    duration_ms     INT,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE security_alerts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    alert_type      TEXT,
    severity        TEXT,
    user_id         UUID,
    ip_address      TEXT,
    details         JSONB,
    resolved        BOOL DEFAULT false,
    resolved_at     TIMESTAMPTZ,
    resolved_by     UUID,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_security_alerts_user ON security_alerts (user_id);
CREATE INDEX idx_security_alerts_unresolved ON security_alerts (created_at DESC) WHERE NOT resolved;

CREATE TABLE rate_limits (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID,
    action      TEXT NOT NULL,
    count       INT DEFAULT 1,
    window_start TIMESTAMPTZ DEFAULT now(),
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_rate_limits_window ON rate_limits (user_id, action, created_at);

CREATE TABLE subscriptions (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                 UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    stripe_subscription_id  TEXT UNIQUE,
    stripe_price_id         TEXT,
    subscription_status     TEXT,
    current_period_end      TIMESTAMPTZ,
    cancel_at_period_end    BOOL DEFAULT false,
    cancels_at              TIMESTAMPTZ,
    created_at              TIMESTAMPTZ DEFAULT now(),
    updated_at              TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE payment_providers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    is_enabled  BOOL DEFAULT true,
    config      JSONB,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- 9. INTERNAL / SYSTEM
-- ---------------------------------------------------------------------------

CREATE TABLE _task_queue (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_name        TEXT NOT NULL,
    queue           TEXT NOT NULL DEFAULT 'default',
    status          TEXT DEFAULT 'pending',
    payload         JSONB,
    scheduled_at    TIMESTAMPTZ DEFAULT now(),
    locked_at       TIMESTAMPTZ,
    locked_by       TEXT,
    completed_at    TIMESTAMPTZ,
    error_message   TEXT,
    retry_count     INT DEFAULT 0,
    max_retries     INT DEFAULT 3,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_task_queue_claim ON _task_queue (queue, status, scheduled_at) WHERE status = 'pending';

CREATE TABLE _cron_locks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_name    TEXT NOT NULL UNIQUE,
    locked_at   TIMESTAMPTZ,
    locked_by   TEXT,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE _cron_failures (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_name        TEXT,
    error_message   TEXT,
    failed_at       TIMESTAMPTZ DEFAULT now(),
    resolved        BOOL DEFAULT false,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE _locks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    locked_at   TIMESTAMPTZ DEFAULT now(),
    locked_by   TEXT,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE config (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key         TEXT NOT NULL UNIQUE,
    value       JSONB,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE _admin_audit_log (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id    UUID,
    action      TEXT NOT NULL,
    target_type TEXT,
    target_id   TEXT,
    details     JSONB,
    ip_address  TEXT,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_audit_log_admin ON _admin_audit_log (admin_id);
CREATE INDEX idx_audit_log_target ON _admin_audit_log (target_type, target_id);

CREATE TABLE _analytics_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type  TEXT NOT NULL,
    user_id     UUID,
    session_id  TEXT,
    properties  JSONB,
    page        TEXT,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_analytics_events_type ON _analytics_events (event_type);
CREATE INDEX idx_analytics_events_user ON _analytics_events (user_id);
CREATE INDEX idx_analytics_events_created ON _analytics_events (created_at DESC);

CREATE TABLE _metrics (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    metric_name     TEXT NOT NULL,
    value           DOUBLE PRECISION NOT NULL,
    tags            JSONB,
    recorded_at     TIMESTAMPTZ DEFAULT now(),
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_metrics_name ON _metrics (metric_name);
CREATE INDEX idx_metrics_recorded ON _metrics (recorded_at DESC);

CREATE TABLE _dynamic_links (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    short_code  TEXT NOT NULL UNIQUE,
    long_url    TEXT NOT NULL,
    click_count INT DEFAULT 0,
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE platform_debt (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seller_id   UUID,
    order_id    UUID,
    amount_cents INT NOT NULL,
    reason      TEXT,
    status      TEXT DEFAULT 'pending',
    created_at  TIMESTAMPTZ DEFAULT now(),
    updated_at  TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX idx_platform_debt_seller ON platform_debt (seller_id);

-- ---------------------------------------------------------------------------
-- 10. Apply updated_at trigger to all tables
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    tbl TEXT;
BEGIN
    FOR tbl IN
        SELECT unnest(ARRAY[
            'users', 'user_security', 'pending_profiles', 'addresses', 'buyer_addresses', 'fcm_tokens',
            'products', 'favorites', 'reviews', 'product_ratings', 'seller_ratings', 'review_votes',
            'product_questions', 'product_recommendations', 'user_recommendations', 'stock_notifications',
            'orders', 'order_events', 'return_requests', 'refunds', 'disputes', 'coupons', 'coupon_uses', 'pending_redemptions',
            'cart',
            'seller_profiles', 'seller_metrics', 'seller_skus', 'warehouses', 'inventory_levels', 'payouts',
            'chats', 'messages', 'message_reports',
            'notifications', '_mail_logs', 'licenses', 'book_access_tokens', 'software_access_tokens', 'download_sessions',
            'webhook_events', 'webhook_logs', 'security_alerts', 'rate_limits', 'subscriptions', 'payment_providers',
            '_task_queue', '_cron_locks', '_cron_failures', '_locks', 'config', '_admin_audit_log',
            '_analytics_events', '_metrics', '_dynamic_links', 'platform_debt'
        ])
    LOOP
        EXECUTE format(
            'CREATE TRIGGER trg_%s_updated_at
             BEFORE UPDATE ON %I
             FOR EACH ROW EXECUTE FUNCTION set_updated_at()',
            tbl, tbl
        );
    END LOOP;
END $$;

-- Drop the helper function — triggers are now applied
DROP FUNCTION _apply_updated_at(regclass);

COMMIT;
