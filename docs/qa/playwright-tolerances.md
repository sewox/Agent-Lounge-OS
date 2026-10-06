# Playwright tolerances (DB-04 / ST-01 / D4-scale)

Intentional measurement slack only — no soft-skip, no `test.skip` / `fixme` / `fail`.

| Case | Assertion | Slack | Rationale |
|------|-----------|-------|-----------|
| **DB-04** Embedded Vault first fold | Title top ≥ −2px; title text intersects viewport on **all** widths (incl. D960) | ±2px | Subpixel / anti-alias only. No `1.15×` fold multiplier. Dashboard: `min-[960px]:landscape` side-by-side (D960 first-fold); portrait (D3) stays stacked for L6. |
| **ST-01** Routing table not clipped | After `scrollIntoViewIfNeeded`, top in viewport; ≥48px visible height; if `bottom > vp.height - 4` → scroll parent required | ±2px top; **−4px** bottom edge (restored; not +1) | Settings stack is tall — first-fold on load is **out of scope**. −4px is the prior stricter near-fold trigger (not a relaxation). |
| **D4-scale** 130% UI scale (a11y seal) | Root `font-size` > 16px (`16 × 1.3 = 20.8`); `data-ui-scale="1.3"`; L1/L3 layout-fill; no L5 overflow | same L1–L5 rules as D0 | Applied via `e2e/storage/d4-scale.json` (`al-os-ui-scale=1.3`) + `deviceScaleFactor: 1.3`. Not a D0 duplicate — rem must enlarge. |

If a case needs looser geometry, document the new number here and in the spec comment — do not hide with skip.
