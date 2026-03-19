#!/usr/bin/env python3
"""Fix auth/signature drift in ob-handlers test modules.

Scope is intentionally narrow: this script only rewrites code inside `#[cfg(test)]`
modules for the four requested files and only applies known-safe transforms.
"""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FILES = {
    "users": ROOT / "crates/ob-handlers/src/users/mod.rs",
    "checkout": ROOT / "crates/ob-handlers/src/payments/checkout.rs",
    "subscriptions": ROOT / "crates/ob-handlers/src/payments/subscriptions.rs",
    "shipping": ROOT / "crates/ob-handlers/src/shipping_calc/mod.rs",
}

USERS_AUTH_CALLS = [
    "create_profile",
    "update_profile",
    "get_profile",
    "email_consent",
    "notification_preferences",
    "cleanup_fcm_token",
    "add_buyer_address",
    "update_buyer_address",
    "delete_buyer_address",
    "set_default_buyer_address",
]

CHECKOUT_AUTH_CALLS = ["create_checkout_session"]
SUBSCRIPTIONS_AUTH_CALLS = [
    "create_subscription",
    "cancel_subscription",
    "subscription_status",
    "reactivate_subscription",
    "update_notification_preferences",
]

USERS_MOCK_AUTH = """
    fn mock_auth(user_id: &str) -> AuthContext {
        AuthContext {
            user_id: user_id.to_string(),
            roles: vec![],
            authenticated: true,
            email_verified: true,
            custom_claims: serde_json::Value::Null,
        }
    }
"""


def read(path: Path) -> str:
    return path.read_text()


def write_if_changed(path: Path, before: str, after: str) -> bool:
    if before == after:
        return False
    path.write_text(after)
    return True


def find_matching_brace(text: str, open_index: int) -> int:
    depth = 0
    for index in range(open_index, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise ValueError("unmatched brace")


def extract_tests(text: str) -> tuple[int, int, str]:
    cfg = text.index("#[cfg(test)]")
    mod = text.index("mod tests", cfg)
    open_brace = text.index("{", mod)
    close_brace = find_matching_brace(text, open_brace)
    return cfg, close_brace + 1, text[cfg : close_brace + 1]


def replace_tests(text: str, new_tests: str) -> str:
    start, end, _ = extract_tests(text)
    return text[:start] + new_tests + text[end:]


def ensure_extension_import(tests: str) -> str:
    if "use axum::{Extension, extract::State};" in tests:
        return tests
    return tests.replace(
        "use axum::extract::State;",
        "use axum::{Extension, extract::State};",
        1,
    )


def add_turnstile_to_setup_state(tests: str) -> str:
    pattern = re.compile(
        r"(async fn setup_state(?:_with_config)?\([^)]*\) -> HandlersState \{\n"
        r"(?:.*\n)*?"
        r"\s*stripe_base_url[^\n]*,\n)"
        r"(?!\s*turnstile_secret_key: None,)",
        re.MULTILINE,
    )
    return pattern.sub(r"\1            turnstile_secret_key: None,\n", tests)


def ensure_users_mock_auth(tests: str) -> str:
    if "fn mock_auth(" in tests:
        return tests
    marker = "    #[test]\n"
    return tests.replace(marker, USERS_MOCK_AUTH + "\n" + marker, 1)


def fix_users_option_string_literals(tests: str) -> str:
    return re.sub(
        r'user_id:\s*"([^"]+)"\.into\(\)',
        r'user_id: Some("\1".to_string())',
        tests,
    )


def fix_users_deser_asserts(tests: str) -> str:
    tests = tests.replace(
        'assert_eq!(req.user_id, "u1");',
        'assert_eq!(req.user_id.as_deref(), Some("u1"));',
    )
    return tests


def fix_generic_user_id_literals(tests: str) -> str:
    return re.sub(
        r'user_id:\s*"([^"]+)"\.into\(\)',
        r'user_id: Some("\1".to_string())',
        tests,
    )


def fix_generic_user_id_asserts(tests: str) -> str:
    return re.sub(
        r'assert_eq!\(req\.user_id,\s*"([^"]+)"\);',
        r'assert_eq!(req.user_id.as_deref(), Some("\1"));',
        tests,
    )


def add_missing_terms_fields(tests: str) -> str:
    def repl(match: re.Match[str]) -> str:
        body = match.group(1)
        if "terms_version:" in body or "terms_accepted_at:" in body:
            return match.group(0)
        indent = " " * 16
        body = body.rstrip()
        if not body.endswith(","):
            body += ","
        body += (
            f"\n{indent}terms_version: None,"
            f"\n{indent}terms_accepted_at: None,\n"
            f"{' ' * 12}"
        )
        return f"Json(UpdateProfileRequest {{\n{body}}})"

    pattern = re.compile(
        r"Json\(UpdateProfileRequest \{\n(.*?)\n\s{12}\}\)",
        re.DOTALL,
    )
    return pattern.sub(repl, tests)


def extract_user_id_literal(call_text: str) -> str:
    match = re.search(r'user_id:\s*Some\("([^"]+)"\.to_string\(\)', call_text)
    if match:
        return match.group(1)
    return "test_user"


def add_missing_auth_arg(tests: str, fn_name: str) -> str:
    needle = f"{fn_name}("
    index = 0
    while True:
        start = tests.find(needle, index)
        if start == -1:
            return tests
        open_paren = start + len(fn_name)
        depth = 0
        close_paren = None
        for pos in range(open_paren, len(tests)):
            char = tests[pos]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    close_paren = pos
                    break
        if close_paren is None:
            return tests
        call_text = tests[start : close_paren + 1]
        if "mock_auth(" in call_text or "Extension(" in call_text:
            index = close_paren + 1
            continue
        if "State(" not in call_text or "Json(" not in call_text:
            index = close_paren + 1
            continue
        user_id = extract_user_id_literal(call_text)
        call_text = call_text.replace(
            "State(state),\n",
            f'State(state),\n            Extension(mock_auth("{user_id}")),\n',
            1,
        )
        call_text = call_text.replace(
            "State(state.clone()),\n",
            f'State(state.clone()),\n            Extension(mock_auth("{user_id}")),\n',
            1,
        )
        tests = tests[:start] + call_text + tests[close_paren + 1 :]
        index = start + len(call_text)


def fix_users_file(path: Path) -> bool:
    before = read(path)
    _, _, tests = extract_tests(before)
    updated = tests
    updated = ensure_extension_import(updated)
    updated = add_turnstile_to_setup_state(updated)
    updated = ensure_users_mock_auth(updated)
    updated = fix_users_option_string_literals(updated)
    updated = fix_users_deser_asserts(updated)
    updated = add_missing_terms_fields(updated)
    for fn_name in USERS_AUTH_CALLS:
        updated = add_missing_auth_arg(updated, fn_name)
    return write_if_changed(path, before, replace_tests(before, updated))


def add_checkout_turnstile_token(tests: str) -> str:
    pattern = re.compile(
        r"(Json\(CreateCheckoutRequest \{\n(?:.*\n)*?\s*idempotency_key: [^\n]+,\n)"
        r"(?!\s*turnstile_token: )",
        re.MULTILINE,
    )
    return pattern.sub(r'\1                turnstile_token: Some("test-token".to_string()),\n', tests)


def fix_checkout_file(path: Path) -> bool:
    before = read(path)
    _, _, tests = extract_tests(before)
    updated = tests
    updated = ensure_extension_import(updated)
    updated = add_turnstile_to_setup_state(updated)
    updated = ensure_users_mock_auth(updated)
    updated = fix_generic_user_id_literals(updated)
    updated = re.sub(
        r'Json\(VerifyPricesRequest \{\n(\s*)user_id: Some\("([^"]+)"\.to_string\(\)\),',
        r'Json(VerifyPricesRequest {\n\1user_id: "\2".to_string(),',
        updated,
    )
    updated = add_checkout_turnstile_token(updated)
    for fn_name in CHECKOUT_AUTH_CALLS:
        updated = add_missing_auth_arg(updated, fn_name)
    return write_if_changed(path, before, replace_tests(before, updated))


def fix_subscriptions_file(path: Path) -> bool:
    before = read(path)
    _, _, tests = extract_tests(before)
    updated = tests
    updated = ensure_extension_import(updated)
    updated = add_turnstile_to_setup_state(updated)
    updated = ensure_users_mock_auth(updated)
    updated = fix_generic_user_id_literals(updated)
    updated = fix_generic_user_id_asserts(updated)
    for fn_name in SUBSCRIPTIONS_AUTH_CALLS:
        updated = add_missing_auth_arg(updated, fn_name)
    return write_if_changed(path, before, replace_tests(before, updated))


def fix_shipping_file(path: Path) -> bool:
    before = read(path)
    _, _, tests = extract_tests(before)
    updated = add_turnstile_to_setup_state(tests)
    updated = re.sub(
        r"(Json\(CalculateShippingRequest \{\n(?:.*\n)*?\s*speed: [^\n]+,\n)(?!\s*subtotal_cents: )",
        r"\1                subtotal_cents: None,\n",
        updated,
    )
    updated = updated.replace("total_cost:", "total_cost_cents:")
    updated = updated.replace('json["totalCost"]', 'json["totalCostCents"]')
    updated = updated.replace('breakdown.insert("item1".to_string(), 8.99);', 'breakdown.insert("item1".to_string(), 899);')
    updated = updated.replace("total_cost_cents: 8.99,", "total_cost_cents: 899,")
    updated = updated.replace('assert_eq!(json["totalCostCents"], 8.99);', 'assert_eq!(json["totalCostCents"], 899);')
    updated = updated.replace('assert_eq!(json["breakdown"]["item1"], 8.99);', 'assert_eq!(json["breakdown"]["item1"], 899);')
    updated = updated.replace("assert_eq!(resp.total_cost, 0.0);", "assert_eq!(resp.total_cost_cents, 0);")
    updated = updated.replace("assert!(resp.total_cost > 0.0);", "assert!(resp.total_cost_cents > 0);")
    updated = updated.replace("assert_eq!(resp.total_cost, 46.68);", "assert_eq!(resp.total_cost_cents, 4668);")
    updated = updated.replace('assert_eq!(resp.breakdown["cart_perishable"], 16.99);', 'assert_eq!(resp.breakdown["cart_perishable"], 1699);')
    updated = updated.replace('assert_eq!(resp.breakdown["cart_standard"], 29.69);', 'assert_eq!(resp.breakdown["cart_standard"], 2969);')
    updated = updated.replace(
        "assert!(perish_cost > 1.0 * FALLBACK_ADJACENT * ADDITIONAL_ITEM_RATE);",
        "assert!(cents_to_dollars(perish_cost) > 1.0 * FALLBACK_ADJACENT * ADDITIONAL_ITEM_RATE);",
    )
    updated = re.sub(
        r"assert!\(\((cost|total) - ([^)]+)\)\.abs\(\) < 0\.01\);",
        r"assert!((cents_to_dollars(\1) - \2).abs() < 0.01);",
        updated,
    )
    updated = re.sub(
        r"assert!\(\((breakdown\[[^\]]+\]) - ([^)]+)\)\.abs\(\) < 0\.01\);",
        r"assert!((cents_to_dollars(\1) - \2).abs() < 0.01);",
        updated,
    )
    updated = re.sub(
        r"assert!\(\((express / standard) - ([^)]+)\)\.abs\(\) < 0\.01\);",
        r"assert!(((express as f64 / standard as f64) - \2).abs() < 0.01);",
        updated,
    )
    updated = re.sub(
        r"assert!\(\((same_day / standard) - ([^)]+)\)\.abs\(\) < 0\.01\);",
        r"assert!(((same_day as f64 / standard as f64) - \2).abs() < 0.01);",
        updated,
    )
    return write_if_changed(path, before, replace_tests(before, updated))


def main() -> None:
    changed: list[str] = []
    if fix_users_file(FILES["users"]):
        changed.append(FILES["users"].relative_to(ROOT).as_posix())
    if fix_checkout_file(FILES["checkout"]):
        changed.append(FILES["checkout"].relative_to(ROOT).as_posix())
    if fix_subscriptions_file(FILES["subscriptions"]):
        changed.append(FILES["subscriptions"].relative_to(ROOT).as_posix())
    if fix_shipping_file(FILES["shipping"]):
        changed.append(FILES["shipping"].relative_to(ROOT).as_posix())
    if changed:
        print("Updated:")
        for item in changed:
            print(f"  - {item}")
    else:
        print("No changes needed.")


if __name__ == "__main__":
    main()
