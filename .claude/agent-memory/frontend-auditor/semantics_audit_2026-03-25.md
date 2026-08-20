---
name: semantics_audit_2026-03-25
description: Full Playwright semantics audit of all lib/screens/ — identifies missing Semantics labels on interactive elements. Top 10 worst offenders ranked by missing count.
type: project
---

Audit run 2026-03-25. RESEARCH ONLY — no files were modified.

**Why:** ALL interactive elements need Semantics labels for Playwright E2E tests to locate them. Missing labels = E2E test failures.

**How to apply:** When fixing files, prioritize by missing count (descending). Apply `Semantics(label: 'btn-*')` wrapper to buttons, `semanticsLabel:` to TextFormField `InputDecoration`, and `ExcludeSemantics` to decorative Icons. See conventions: `btn-`, `input-`, `nav-`, `product-card-<id>`, `order-card-<id>`.

## Top 10 Worst Offenders (Missing Semantics)

| Rank | Screen File | Interactive Elements | With Semantics | Missing | Priority |
|------|-------------|---------------------|---------------|---------|----------|
| 1 | parts/addproduct_delivery_children_section.dart | 9 TextFormField + 2 GestureDetector | 0 | 11 | CRITICAL |
| 2 | parts/editproduct_basic_info_section.dart | 9 TextFormField + 3 SwitchListTile | 0 | 12 | CRITICAL |
| 3 | parts/addproduct_supplier_children_section.dart | 7 TextFormField + 4 toggles | 0 | 11 | CRITICAL |
| 4 | parts/addproduct_form_content_section.dart | 8 TextFormField + toggles | 0 | 8+ | CRITICAL |
| 5 | parts/editproduct_shipping_section.dart | 6 TextFormField + 3 SwitchListTile | 0 | 9 | CRITICAL |
| 6 | parts/addproduct_package_location_section.dart | 5 TextFormField + buttons | 0 | 7 | CRITICAL |
| 7 | parts/editproduct_delivery_section.dart | 4 TextFormField + SwitchListTile | 0 | 4 | CRITICAL |
| 8 | parts/editproduct_location_section.dart | 3 TextFormField + Dropdown | 0 | 4 | CRITICAL |
| 9 | seller_setup_screen.dart | 4 TextButton/ModernButton | 0 | 4 | CRITICAL |
| 10 | parts/cart_item_widget.dart | 4 buttons | 0 | 4 | CRITICAL |

## Additional Violations (not in top 10)

- seller/parts/warehouses_form_section.dart — 2 buttons, 1 TextField, 0 Semantics
- seller/parts/warehouses_helper_widgets.dart — 1 GestureDetector + 1 TextField, 0 Semantics
- common_screens.dart — 1 button, 0 Semantics
- email_verification_screen.dart — 1 button, 0 Semantics
- widgets/product_detail/product_info_section.dart — 1 GestureDetector, 0 Semantics

## Files With Partial Semantics (need review)

- mfa_setup_screen.dart — 11 interactive / 5 semantics → 6 missing
- seller/bulk_upload_screen.dart — 8 interactive / 4 semantics → 4 missing (buttons have labels, but retry/nav buttons inside don't)
- widgets/product_detail/product_reviews_section.dart — 8 interactive / 2 semantics → 6 missing (GestureDetector on photo, TextButton for retry and helpfulness vote)
- editaddress_screen.dart — 7 interactive / 2 semantics → 5 missing
- parts/product_form_helper_widgets.dart — 16 TextFormField (variant rows) / 3 semantics → 13 missing (variant TextFormFields in dialog have none)

## Notable GestureDetectors Without Semantics Wrappers

- addproduct_delivery_section.dart:56 — Info button (GestureDetector → Icon)
- addproduct_delivery_section.dart:223 — Bulk discount info button (GestureDetector → Icon)
- warehouses_helper_widgets.dart — Unknown GestureDetector
- product_info_section.dart — Unknown GestureDetector

## Files That Are Compliant (or near-compliant)

- seller/bulk_upload_screen.dart — download, select, upload-all, view-products buttons all have Semantics labels ✓
- parts/product_form_helper_widgets.dart — _DigitalTypeCard, _VariantOptionCard edit/remove have Semantics ✓
- widgets/product_detail/product_reviews_section.dart — _WriteReviewButton has Semantics ✓
- parts/login_form_panel.dart — best coverage in codebase (7 interactive, 11 semantics hits)
