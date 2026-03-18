#!/usr/bin/env python3
"""Seed OrignaBase VPS with test data for origna_gta development.

Usage:
    python3 scripts/seed_orignabase.py                          # defaults to http://127.0.0.1:8080
    python3 scripts/seed_orignabase.py --url http://204.168.137.16:8080  # VPS
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from typing import Optional

import requests

DEFAULT_URL = "http://127.0.0.1:8080"

# Test accounts matching origna_gta E2E config
ACCOUNTS = [
    {"email": "buyer1@example.com", "password": "TestPass123", "display_name": "Test Buyer 1"},
    {"email": "buyer2@example.com", "password": "TestPass123", "display_name": "Test Buyer 2"},
    {"email": "seller1@example.com", "password": "TestPass123", "display_name": "Test Seller 1"},
    {"email": "seller2@example.com", "password": "TestPass123", "display_name": "Test Seller 2"},
    {"email": "admin@example.com", "password": "AdminPass123", "display_name": "Admin User"},
]

PRODUCTS = [
    {
        "title": "Organic Maple Syrup",
        "description": "100% pure Canadian maple syrup from Quebec",
        "priceCents": 2499,
        "category": "food",
        "stockQuantity": 100,
        "isActive": True,
        "isPerishable": False,
    },
    {
        "title": "Artisan Sourdough Bread",
        "description": "Freshly baked artisan sourdough from local bakery",
        "priceCents": 899,
        "category": "food",
        "stockQuantity": 50,
        "isActive": True,
        "isPerishable": True,
    },
    {
        "title": "Handmade Wool Scarf",
        "description": "Canadian wool scarf, hand-knitted in Toronto",
        "priceCents": 4999,
        "category": "clothing",
        "stockQuantity": 25,
        "isActive": True,
        "isPerishable": False,
    },
    {
        "title": "Vintage Chess Set",
        "description": "Wooden chess set with Staunton pieces, competition size",
        "priceCents": 8999,
        "category": "games",
        "stockQuantity": 10,
        "isActive": True,
        "isPerishable": False,
    },
    {
        "title": "Bioinformatics Textbook",
        "description": "Introduction to Algorithms for Bioinformatics, 3rd Edition",
        "priceCents": 6499,
        "category": "books",
        "stockQuantity": 15,
        "isActive": True,
        "isPerishable": False,
    },
    {
        "title": "Fresh Atlantic Salmon Fillet",
        "description": "Wild-caught Atlantic salmon, 500g fillet",
        "priceCents": 1899,
        "category": "food",
        "stockQuantity": 30,
        "isActive": True,
        "isPerishable": True,
    },
]

REMOTE_CONFIG = {
    "maintenance_mode": "false",
    "min_app_version": "1.0.0",
    "free_shipping_threshold_cents": "7500",
    "premium_subscription_price_cad": "9.99",
    "max_products_per_seller": "100",
    "platform_fee_percent": "10",
}


def health_check(base_url: str) -> bool:
    try:
        r = requests.get(f"{base_url}/health", timeout=5)
        if r.status_code == 200:
            print(f"  Health check OK: {r.text.strip()}")
            return True
        print(f"  Health check FAILED: {r.status_code} {r.text[:200]}")
        return False
    except Exception as e:
        print(f"  Health check ERROR: {e}")
        return False


def _retry_on_429(func, *args, max_retries: int = 3, **kwargs) -> Optional[dict]:
    """Wrapper that retries on 429 rate-limit responses."""
    for attempt in range(max_retries):
        result = func(*args, **kwargs)
        if result is not None or not hasattr(func, '_last_429') or not func._last_429:
            return result
        wait = 3 * (attempt + 1)
        print(f"  Rate limited, waiting {wait}s...")
        time.sleep(wait)
    return None


def register_user(base_url: str, email: str, password: str, display_name: str) -> Optional[dict]:
    for attempt in range(3):
        try:
            r = requests.post(
                f"{base_url}/auth/register",
                json={"email": email, "password": password, "display_name": display_name},
                timeout=10,
            )
            if r.status_code == 429:
                wait = 3 * (attempt + 1)
                print(f"  Rate limited on register {email}, waiting {wait}s...")
                time.sleep(wait)
                continue
            if r.status_code in (200, 201):
                data = r.json()
                uid = data.get("user", {}).get("id", "?")
                print(f"  Registered: {email} -> uid={uid}")
                return data
            elif r.status_code == 409 or (r.status_code == 400 and "failed" in r.text.lower()):
                print(f"  Already exists: {email}, logging in...")
                return login_user(base_url, email, password)
            else:
                print(f"  Register FAILED ({r.status_code}): {r.text[:200]}")
                return None
        except Exception as e:
            print(f"  Register ERROR: {e}")
            return None
    return None


def login_user(base_url: str, email: str, password: str) -> Optional[dict]:
    for attempt in range(3):
        try:
            r = requests.post(
                f"{base_url}/auth/login",
                json={"email": email, "password": password},
                timeout=10,
            )
            if r.status_code == 429:
                wait = 3 * (attempt + 1)
                print(f"  Rate limited on login {email}, waiting {wait}s...")
                time.sleep(wait)
                continue
            if r.status_code == 200:
                data = r.json()
                print(f"  Logged in: {email}")
                return data
            else:
                print(f"  Login FAILED ({r.status_code}): {r.text[:200]}")
                return None
        except Exception as e:
            print(f"  Login ERROR: {e}")
            return None
    return None


def create_via_graphql(base_url: str, token: str, collection: str, doc: dict) -> Optional[dict]:
    """Create a document via GraphQL mutation. Returns the parsed JSON result or None."""
    query = "mutation CreateDoc($collection: String!, $data: JSON!) { create(collection: $collection, data: $data) }"
    variables = {"collection": collection, "data": doc}
    try:
        r = requests.post(
            f"{base_url}/graphql",
            json={"query": query, "variables": variables},
            headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
            timeout=10,
        )
        if r.status_code == 200:
            data = r.json()
            if "errors" in data:
                print(f"  GraphQL {collection} errors: {data['errors'][0].get('message', '')[:100]}")
                return None
            result = data.get("data", {}).get("create")
            doc_id = result.get("id", "?") if isinstance(result, dict) else "ok"
            print(f"  Created {collection}/{doc_id} via GraphQL")
            return result
        else:
            print(f"  GraphQL FAILED ({r.status_code}): {r.text[:200]}")
            return None
    except Exception as e:
        print(f"  GraphQL ERROR: {e}")
        return None


def main():
    parser = argparse.ArgumentParser(description="Seed OrignaBase with test data")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"Base URL (default: {DEFAULT_URL})")
    args = parser.parse_args()
    base_url = args.url.rstrip("/")

    print(f"\n=== Seeding OrignaBase at {base_url} ===\n")

    # 1. Health check
    print("[1/5] Health check...")
    if not health_check(base_url):
        print("Server not responding. Aborting.")
        sys.exit(1)

    # 2. Register accounts
    print("\n[2/5] Registering test accounts...")
    tokens: dict[str, str] = {}
    uids: dict[str, str] = {}
    for i, acc in enumerate(ACCOUNTS):
        if i > 0:
            time.sleep(2)  # avoid rate limiting
        result = register_user(base_url, acc["email"], acc["password"], acc["display_name"])
        if result:
            token = result.get("access_token") or result.get("token", "")
            # Auth response: {"access_token": "...", "user": {"id": "users:xxx", ...}}
            uid = result.get("user", {}).get("id", "")
            tokens[acc["email"]] = token
            uids[acc["email"]] = uid

    # 3. Create products (as seller1)
    print("\n[3/5] Creating products...")
    seller_token = tokens.get("seller1@example.com")
    seller_uid = uids.get("seller1@example.com", "")
    if seller_token:
        for i, prod in enumerate(PRODUCTS):
            if i > 0:
                time.sleep(1)
            prod_with_seller = {**prod, "sellerId": seller_uid}
            create_via_graphql(base_url, seller_token, "products", prod_with_seller)
    else:
        print("  No seller token — skipping products")

    # 4. Create user profiles
    print("\n[4/5] Creating user profiles...")
    for acc in ACCOUNTS:
        token = tokens.get(acc["email"])
        uid = uids.get(acc["email"], "")
        if not token:
            continue
        profile = {
            "uid": uid,
            "displayName": acc["display_name"],
            "email": acc["email"],
            "roles": ["admin"] if "admin" in acc["email"] else (["seller"] if "seller" in acc["email"] else ["buyer"]),
            "emailVerified": True,
            "createdAt": int(time.time()),
        }
        create_via_graphql(base_url, token, "users", profile)

    # 5. Remote config
    print("\n[5/5] Setting remote config...")
    admin_token = tokens.get("admin@example.com")
    if admin_token:
        for key, value in REMOTE_CONFIG.items():
            create_via_graphql(base_url, admin_token, "remote_config", {"key": key, "value": value})
    else:
        print("  No admin token — skipping config")

    print(f"\n=== Seeding complete ===")
    print(f"Accounts: {len(tokens)}/{len(ACCOUNTS)}")
    print(f"Products: {len(PRODUCTS)}")
    print(f"Config keys: {len(REMOTE_CONFIG)}")


if __name__ == "__main__":
    main()
