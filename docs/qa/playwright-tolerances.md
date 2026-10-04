# Playwright tolerances (DB-04 / ST-01)

Intentional measurement slack only — no soft-skip, no `test.skip` / `fixme` / `fail`.

| Case | Assertion | Slack | Rationale |
|------|-----------|-------|-----------|
| **DB-04** Embedded Vault first fold | Title top ≥ −2px; title text intersects viewport on **all** widths (incl. D960) | ±2px | Subpixel / anti-alias only. No `1.15×` fold multiplier. Dashboard: `min-[960px]:landscape` side-by-side (D960 first-fold); portrait (D3) stays stacked for L6. |
| **ST-01** Routing table not clipped | After `scrollIntoViewIfNeeded`, top in viewport; ≥48px visible height; if bottom past fold → scroll parent required | ±2px top; +1px bottom | Settings stack is tall — first-fold placement is **out of scope**. Test proves clip-freedom + interactable controls, not “above the fold on load”. |

If a case needs looser geometry, document the new number here and in the spec comment — do not hide with skip.
