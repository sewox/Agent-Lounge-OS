<!--
PR başlığı Conventional Commits formatında olmalı; squash merge'de commit mesajı olur.
The PR title must follow Conventional Commits; it becomes the squash commit message.
  feat: …   fix(ui): …   docs: …   chore: …   refactor: …   test: …   ci: …
-->

## Ne değişti? / What changed?

<!-- Kısa özet ve nedeni. / Short summary and why. -->

## İlgili issue / Related issue

<!-- Closes #123 -->

## Değişiklik türü / Type of change

- [ ] 🐞 Hata düzeltmesi / Bug fix
- [ ] ✨ Yeni özellik / New feature
- [ ] ♻️ Refactor (davranış değişmez / no behavior change)
- [ ] 📝 Dokümantasyon / Documentation
- [ ] 🔧 CI / build / bağımlılık / dependencies

## Nasıl test edildi? / How was it tested?

<!-- Çalıştırdığınız komutlar ve manuel test adımları. / Commands run and manual test steps. -->

- [ ] `npm run lint` · `npm run types:check` · `npm run test:unit`
- [ ] `npm run qa:gates` (statik kapılar / static gates)
- [ ] `npm run test:e2e` (arayüz değiştiyse / if UI changed)
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`, `cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings` ve/and `cargo test --manifest-path src-tauri/Cargo.toml --locked` (Rust değiştiyse / if Rust changed)
- [ ] `pytest workers/tests -q` (workers değiştiyse / if workers changed)
- [ ] Uygulamada manuel olarak denendi / Manually verified in the app — OS: <!-- macOS / Windows / Linux -->

## Kontrol listesi / Checklist

- [ ] PR tek bir konuya odaklı / The PR is focused on one topic
- [ ] Davranış değişikliği için test eklendi veya güncellendi / Tests added or updated for behavior changes
- [ ] Yeni arayüz metinleri i18n üzerinden ve TR/EN'de var / New UI strings go through i18n and exist in TR/EN
- [ ] Arayüz değiştiyse ekran görüntüsü eklendi / Screenshots attached for UI changes
- [ ] API anahtarı, token, özel yol veya kişisel veri yok / No API keys, tokens, private paths or personal data
- [ ] Güvenlik etkisi düşünüldü (politika kapısı, onay akışı, komut allowlist) / Security impact considered (policy gate, approval flow, command allowlist)

## Ekran görüntüleri / Screenshots

<!-- Arayüz değişikliklerinde önce/sonra. / Before/after for UI changes. -->
