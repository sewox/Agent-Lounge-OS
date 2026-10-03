# Agent Lounge OS — Yol Haritası

Bu belge ürün ve mimari önceliklerin kısa özetidir. Detaylı QA ve geliştirme planı için bkz. [`docs/qa/qa-plan.md`](qa/qa-plan.md).

---

## V1.0

### Vault FTS5 migration

Sayfa ve deneyim aramasını SQLite `LIKE` sorgularından **FTS5** tam metin indeksine taşımak.

**Gerekçe:** 800+ sayfalık indekslerde arama gecikmesi ve sıralama kalitesi `LIKE` ile yeterli ölçeklenmiyor. Geçici ara çözüm olarak [#70](https://github.com/sewox/Agent-Lounge-OS/issues/70)'te `LIKE` wildcard kaçışı ele alındı; kalıcı çözüm FTS5 geçişidir.

---

## Mimari notlar

- **NATS ileride sidecar, LMR opsiyonel kalır** — NATS süreci ayrı bir sidecar olarak yönetilebilir; Lounge Model Runner (LMR) yerel inference için isteğe bağlı kalır.
- **Global `i18n.t` yasak, `useTranslation` hook kullanılacak** — bileşenlerde doğrudan global çeviri erişimi yerine React hook tabanlı i18n; ESLint kuralı değerlendirilecek.
