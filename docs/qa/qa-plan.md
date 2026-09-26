# Agent Lounge OS: Uçtan Uca QA Planı (taslak v1)

| Alan | Değer |
|------|-------|
| Tarih | 2026-09-26 |
| Girdi | UI/fonksiyon audit'i, draft PR #45 (https://github.com/sewox/Agent-Lounge-OS/pull/45): `docs/qa/2026-09-26-ui-audit.md` ve 54 ekran görüntüsü |
| Mimar | Gemini (AI Studio), 2 tur. Ham yanıtlar: `gemini-qa-plan-reply.md` |
| Audit base / güncel main | Audit `716f42c` üzerinde koştu. Güncel main `15e19e9` (fontlar self-host, build artık Google Fonts istemiyor) |
| Durum | Sadece plan. Implementasyon ya da merge yok. PR #45 merge edilmeyecek |

## 0. Sercan'ın istekleri (kapsam)

1. Her sayfa tek tek gezilecek, bozuk UI tespit edilip temizlenecek.
2. Tecrübeler (Experiences): kullanıcı okuyabilmeli, düzenleyebilmeli ya da müdahale edebilmeli.
3. Dead Symbols: detaylar görünmeli, kullanıcı kolayca aksiyon alabilmeli.
4. "+ New Node": ya anlamlı hale gelecek ya da kaldırılacak.
5. "Çalışmayan hiçbir şey kalmasın."
6. **Ek (Sercan): layout doluluğu.** Knowledge Vault'ta (`mock__vault-selected__1280x800.png`) "Semantic Map + Experiences" paneli içerik alanının yalnızca orta yarısını kaplıyor. Sağ taraf ve alt kısım boş, sayfa yarım kalmış gibi görünüyor. Audit bunu yakalamamıştı. Her sayfa, her ekranda mevcut genişliği ve yüksekliği kullanmalı.

Süreç: bu plan, ardından testler (baseline koşusu), ardından sonuçlara göre geliştirme PR'ları.

## 1. Audit özeti (baseline)

- Yaklaşık 99 kontrol envanterlendi: **42 çalışıyor, 28 kısmi, 7 stub, 4 bozuk, 18 eksik.**
- P0 Tecrübeler: UI salt okunur ve `line-clamp-3` ile kesiliyor, `list_experiences` limiti 12. Düzenle, sil, pin, retag, yanlış işaretle yok. Tauri'de `get/update/delete_experience` yok. MCP'de yalnızca create ve search var.
- P0 Dead Symbols: en fazla 8 satır gösteriliyor, satırlar tıklanamaz `<div>`. Aç, kopyala, yoksay ya da görev aksiyonu yok. KPI bazen `MOCK_HEALTH` toplamı olan 127'yi gösterirken seçim "temiz" diyor.
- P0/P1 ölü chrome: `+ New Node` (handler yok, sidebar kenarından taşıyor), Quick Filter, Docs, API Keys, Bell.
- P1: Health ve Map boş olduğunda `MOCK_HEALTH` / `MOCK_NODES` gösteriyor. Palette'ten tecrübeye gidince metin yine kesik.
- P2: Vault paneli 1280'de ekranın altında kalıyor. Settings routing tablosu kırpılıyor. Uzun isimlerde tooltip yok. TR/EN karışık.
- Eksik backend: `get_experience`, `update_experience`, `delete_experience`, pin/status şeması, ignore tablosu, "editörde aç" komutu, dead-symbol'dan görev oluşturma, `DeadSymbol.last_ref`.
- Testler: cargo 261 geçti, workers 9 geçti, lint/types/unit geçti (yalnızca 6 unit test). **e2e testi yok.**

## 2. Ürün kararları (Gemini)

| # | Konu | Gemini kararı |
|---|------|---------------|
| K1 | Tecrübe CRUD | Tam CRUD. Şemaya `updated_at`, `is_pinned`, `status` eklenecek. Geçmiş için tam history yok; tek bir `original_content` alanı tutulacak. Silme **soft delete (archive)** olacak. |
| K2 | Kim düzenler | "Ajanlar sadece tecrübe önerir; kullanıcı onaylar, düzenler veya geçersiz sayar." Ajan kayıtları varsayılan **Draft** olacak ve onaylanmadan whisper olarak kullanılmayacak. MCP'ye update ve delete eklenmeyecek. |
| K3 | Migration | Mevcut tüm kayıtlar **Approved** olacak. Arşivlenenler Vault'ta "Show Archived" toggle'ı ile soluk görünecek. |
| K4 | Dead Symbols aksiyonları | OPEN_IN_EDITOR (dosya:satır), IGNORE (yeni `ignored_symbols` tablosu), FIX_WITH_AGENT (NATS üzerinden temizlik görevi). 8 satır limiti kalkacak, yerine infinite scroll ya da virtual list gelecek. |
| K5 | Referanslar | `last_ref` (dosya:satır) **P0**. Bridge, bu veriyi codebase-memory-mcp'den çekecek şekilde güncellenecek. |
| K6 | Ignore listesi | `/health` sayfasında yeni bir sekme olacak. |
| K7 | New Node | **Kaldırılacak.** "Add Project" / "Connect Worker" gibi işlevsel butonlar ileride yalnızca ilgili sayfada (`/health`, `/fleet`) olabilir. |
| K8 | Header chrome | Docs ve API Keys kaldırılacak. **Quick Filter kaldırılacak** (işi zaten Ctrl+K / ⌘K'da). **Bell**, son 5 kritik güvenlik/quota uyarısını gösteren bir "Alert History"ye bağlanacak. |
| K9 | MOCK politikası | `MOCK_HEALTH` ve `MOCK_NODES` "tamamen yasak". Boş durumda empty state gösterilecek: Health/Map için "No data found. [Index Workspace Now]". Daemon listesinde "Checking services…" sonrası "Disconnected" (kırmızı) ve "Restart Service". |
| K10 | Komut paleti | Yalnızca navigasyon ve arama. "Re-index" ya da "Clear Cache" eklenmeyecek; Re-index güvenlik nedeniyle sadece `/health` butonlarında kalacak. Arama sonuçları okunabilir uzunlukta olacak. |
| K11 | Otomasyon | Hibrit yaklaşım, "ONAYLANDI". Playwright yalnızca browser modunda çalışacak. Mac'te manuel checklist artı script'li ekran görüntüsü ve `QA_Report.md`. Pencereyi ekranlar arasında taşıma işi koordinatör ajanda. |
| K12 | Bloklama | P0 hatası (CRUD bozukluğu, veri kaybı, güvenlik, ana akış çökmesi) merge'ü engeller. Eşik üstü görsel regresyon engeller. MOCK_ verisi içeren kod main'e giremez. |
| K13 | Layout doluluğu | Ana içerik, sidebar hariç net genişliğin **en az %85**'ini kaplamalı. %15'ten fazla anlamsız boşluk P1 layout hatası sayılır. Dikey ekranda paneller alt alta dizilmeli, her biri `w-full`. Görsel regresyon eşiği %3 piksel. |

**Konsolidasyon notları.** Bunlar Gemini'nin kararlarını uygulanabilir hale getirmek için benim yorumlarım.

- Status adlarında tutarsızlık var: 1. turda Verified/Draft/Deprecated, 2. turda Approved/Archived. PR-1'de enum `draft | approved | deprecated` olarak önerilir; soft delete ayrı bir `archived_at` alanı olur.
- "MOCK_ içeren kod main'e giremez" kuralı şöyle yorumlandı: **Tauri ve canlı modda** mock fallback yasak. Browser dev-harness'teki mock IPC (`isTauri() === false`) ve test fixture'ları kalabilir, çünkü Playwright e2e'nin buna ihtiyacı var. Bu kural `src/` altında, canlı kod yolunda `MOCK_HEALTH` ve `MOCK_NODES` referansını yasaklayan bir lint/grep kapısıyla uygulanır.
- Gemini'nin önerdiği `toHaveJSProperty('clientWidth', x => …)` Playwright'ta geçerli bir API değil. Aşağıdaki `boundingBox()` tabanlı ölçüm kullanılacak.
- %85 kuralı (K13) ile Gemini'nin örnek kodundaki "viewport'un %80'i" aynı hedef olarak birleştirildi: ölçüt, **sidebar hariç içerik alanının %85'i**.
- 1. turdaki "sidebar'da sadece Dashboard, Vault, Health, Fleet, Settings kalsın" önerisi, "yalnızca çalışan kalsın" ilkesine göre düzeltildi. Audit'e göre `/stream`, `/telemetry`, `/quotas` de çalışıyor, bu yüzden 8 nav linkin hepsi kalıyor. Gemini 2. turda buna itiraz etmedi.
- PR #42'deki davranışa göre, Graph port değişince pencere **yeniden yaratılıyor**. Gemini'nin "kapandığını doğrula" test case'i buna göre "kapanıp yeni portla yeniden açıldığını doğrula" olarak yazıldı.

## 3. Test yüzeyleri ve otomasyon

| Yüzey | Araç | Kapsam | Ne zaman |
|-------|------|--------|----------|
| S1: UI e2e (CI) | Playwright, Next `next dev` veya static `out/`, Tauri IPC mock'u (`window.__TAURI_INTERNALS__.invoke` stub'ı; fixture ile "dolu" ve "boş" iki veri seti) | Tüm route'lar, kontrol davranışları, empty state'ler, layout doluluğu, 4 viewport'ta görsel regresyon | Her PR, CI gate |
| S2: Canlı Tauri (Mac) | Manuel checklist, `screencapture` script'i, pencereyi AppleScript/System Events ile 3 ekrana taşıma, `QA_Report.md` | Gerçek IPC, gerçek SQLite, Graph UI penceresi, native dialog'lar, HiDPI netliği, dikey ekran | Her UI PR'ında merge öncesi. Sercan'ın Mac'i müsaitken |
| S3: Backend entegrasyon | Rust integration testleri (ephemeral port, `MemoryBridgeConfig` enjeksiyonu), MCP HTTP sunucusu, NATS | `lounge_record_experience` ile Draft kaydı oluşup UI listesinde görünmesi; `lounge.task.requested` (FIX_WITH_AGENT); worker heartbeat ile Fleet durumu; `get_dead_symbols` + ignore filtresi | Her PR (cargo test) + nightly |
| S4: Statik kapılar | lint, types, unit, cargo fmt/clippy/test, workers pytest, **mock-yasağı grep** | Mevcut CI ve ek kapı | Her PR |

macOS'ta tauri-driver/WebDriver desteklenmediği için gerçek pencere Playwright ile sürülemez. S2 bu yüzden manuel ve script destekli (K11).

### 3.1 Layout doluluğu: otomatik ölçüm (S1)

Her route için, her viewport'ta çalışır:

```ts
const vp = page.viewportSize()!;
const sidebar = await page.locator('[data-qa="sidebar"]').boundingBox();
const main    = await page.locator('main').boundingBox();
const panels  = await page.locator('main [data-qa="panel"]').evaluateAll(els =>
  els.map(e => e.getBoundingClientRect()).map(r => ({x:r.x,y:r.y,w:r.width,h:r.height})));
const contentW = vp.width - (sidebar?.width ?? 0);
const union = { x0: Math.min(...panels.map(p=>p.x)), x1: Math.max(...panels.map(p=>p.x+p.w)),
                y1: Math.max(...panels.map(p=>p.y+p.h)) };
expect(union.x1 - union.x0).toBeGreaterThanOrEqual(0.85 * contentW);      // L1 genişlik
expect(union.y1).toBeGreaterThanOrEqual(0.85 * vp.height);               // L2 yükseklik (ya da sayfa scroll ediyor)
```

- **L1 genişlik:** Panellerin birleşik genişliği, sidebar hariç içerik alanının en az %85'i olmalı.
- **L2 yükseklik:** Son panelin alt kenarı viewport yüksekliğinin en az %85'inde olmalı ya da sayfa dikey scroll ediyor olmalı. Panel ekranın ortasında bitmemeli. Ayrıca **içerik doluluğu:** yüksekliği viewport’un ≥%50’si olan panellerde satır/içerik alt kenarı panel yüksekliğinin %45’inden azsa fail (stretched frame, boş iç — live Health/Fleet).
- **L3 boş bölge:** İçerik alanı 8x8'lik bir ızgaraya bölünür. Hiçbir panelle kesişmeyen ve bitişik boş hücrelerden oluşan en büyük dikdörtgen, içerik alanının %15'ini geçmemeli. Bu, "sağ yarı boş" durumunu yakalar.
- **L4 dar tek panel:** Tek panel genişliği içerik alanının %70'inden azsa fail — ortalanmış **veya** sola yaslı sabit genişlik (live G4: Health/Fleet/Telemetry ~30–63%).
- **L5 taşma:** `document` / `main` / panel `scrollWidth <= innerWidth` (portrait `/quotas` yatay taşması). Sidebar çocukları taşmamalı; **+ New Node** "AL-OS CORE" ile örtüşmemeli (live G1).
- **L6 dikey (1080x1920):** Paneller alt alta dizilmeli. Her panelin genişliği içerik alanının en az %95'i olmalı.
- Bu ölçümler için panellere `data-qa="panel"`, sidebar'a `data-qa="sidebar"` eklenmesi PR-2'nin işi.
- Otomatik ölçümün yanında **screenshot review** yapılır: her PR'da viewport × route ekran görüntüleri artifact olarak yüklenir, reviewer gözle kontrol eder.

### 3.2 Görsel regresyon

- `toHaveScreenshot` ile %3 piksel eşiği (K12/K13).
- Baseline, PR #45 ekran görüntülerinden değil, düzeltilmiş UI'dan (PR-2 sonrası) yeniden alınır. PR #45 görüntüleri "önce" kanıtı olarak kalır.
- Font render farkları için CI'da tek bir OS/Chromium sürümü sabitlenir.

## 4. Ekran / viewport matrisi

| ID | Ortam | Boyut (CSS px) | Yön | Yüzey | Not |
|----|-------|----------------|-----|-------|-----|
| D0 | CI viewport | 1280x800 | yatay | S1 | Audit baseline. En dar desteklenen masaüstü |
| D1 | 14" MacBook Retina | ~1512x982 (3024x1964 native, DPR 2) | yatay | S1 (1512x982, DPR 2) + S2 | HiDPI font ve 1px border netliği |
| D2 | 32" Samsung | 1920x1080 (DPR 1) | yatay | S1 + S2 | Geniş ekranda boşluk yönetimi, max-width tuzakları |
| D3 | 27" F27G3xTF | 1080x1920 (DPR 1) | **dikey** | S1 + S2 | Paneller alt alta, `w-full`, yatay scroll yok |
| D4 | UI scale | D0–D3 × %90 / %100 / %130 | — | S1 (seçili route'lar) + S2 (Settings) | Settings'teki UI scale radio'ları. Gemini zoom aralığı olarak %90–%130 dedi |

S2'de her UI PR'ında Tauri penceresi D1, D2 ve D3'te sırayla tam ekran (maximize) açılır. Her route'un ekran görüntüsü alınır, L1–L6 checklist'i işaretlenir, sonuç `QA_Report.md`'ye yazılır.

## 5. Sayfa bazlı test case'ler ve kabul kriterleri

Etiketler: **[B]** bloklayıcı (merge'ü engeller), **[A]** otomatik (S1/S3), **[M]** Mac'te manuel (S2). Her sayfa için **LAYOUT** satırı D0–D3'te L1–L6'yı kapsar.

### 5.1 Global shell ve header (`app-shell.tsx`)

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| SH-01 | 8 nav linki | Her link doğru route'a gider; aktif link vurgulanır | A, B |
| SH-02 | Ölü chrome | `+ New Node`, Quick Filter, Docs, API Keys DOM'da yok. Handler'ı olmayan hiçbir `button` / `[role=button]` kalmaz (e2e'de "her buton tıklanınca bir etki üretir" taraması) | A, B |
| SH-03 | Bell | Tıklanınca son 5 kritik security/quota uyarısını listeleyen popover açılır. Uyarı yoksa "Uyarı yok" empty state gösterilir | A, M |
| SH-04 | Header arama / kısayol | Win/Linux **Ctrl+K**, macOS **⌘K** ve arama kutusu paleti açar; UI etiketi platforma uyarlanır; Esc kapatır (bkz. §10.2) | A |
| SH-05 | Model select | Tauri'de `set_kernel_model` çağrılır ve seçim yeniden açılışta korunur. Browser'da açıklayıcı disabled durum gösterilir | A, M |
| SH-06 | Index Workspace | Klasör seçilir, `index_workspace` çağrılır. Dead listesi ve Map dolar, KPI gerçek sayıyı gösterir | M, B |
| SH-07 | Daemon satırları | "Checking services…" sonrası ya "Running" ya da "Disconnected" (kırmızı) ve "Restart Service" gösterilir. Açıklamasız `—` kalmaz | A, M |
| SH-08 | Taşma | Hiçbir header/sidebar elemanı kendi kabından taşmaz (L5); **+ New Node** "AL-OS CORE" ile örtüşmez (live G1) | A, B |
| SH-LAYOUT | L1–L6 | D0–D3 | A, M, B |

### 5.2 Dashboard (`/dashboard`)

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| DB-01 | KPI Dead Symbols | Değer, SQLite'daki gerçek sayıya (ignore hariç) eşit. Hiçbir koşulda 127 ya da MOCK toplamı gösterilmez. Index yoksa "—" ve "Index Workspace" CTA'sı | A, B |
| DB-02 | KPI ile seçim tutarlılığı | KPI > 0 iken Vault seçimi "temiz" demez; KPI = 0 iken liste boştur | A, B |
| DB-03 | Event Stream | all/task/exp filtreleri, Probe bus ve Prev/Next mevcut davranışı korur (regresyon) | A |
| DB-04 | Gömülü Vault | Map ve Experiences 1280x800'de ilk ekranda görünür (V2). Panel ekranın ortasında bitmez | A, B |
| DB-05 | Critical Quotas | Satırlar gösterilir, link `/quotas`'a gider | A |
| DB-06 | Collapse kalıcılığı | Panel collapse durumu reload sonrası korunur | A |
| DB-LAYOUT | L1–L6 | D0–D3 | A, M, B |

### 5.3 Vault / Tecrübeler (`/vault`), P0

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| EX-01 | Tam okuma | Karta tıklanınca detay drawer'ı ya da modal açılır: `line-clamp` olmadan tam ADR, agent, tags, topic, related task, outcome, status, created/updated | A, B |
| EX-02 | Düzenleme | ADR, tags, outcome ve project düzenlenip kaydedilir (`update_experience`). `updated_at` değişir, ilk içerik `original_content`'te korunur. Reload sonrası değişiklik yerinde | A, S3, B |
| EX-03 | Soft delete / archive | Onay diyaloğu çıkar. Kayıt listeden kalkar, "Show Archived" açıkken soluk görünür, geri alınabilir. DB'de satır fiziksel olarak silinmez | A, S3, B |
| EX-04 | Pin | Pinlenen kayıt listenin başına çıkar ve reload sonrası orada kalır | A, B |
| EX-05 | Status akışı | MCP `lounge_record_experience` ile gelen kayıt **Draft** görünür. Kullanıcı Approve edince Approved olur. Draft kayıt whisper olarak önerilmez | S3, A, B |
| EX-06 | Yanlış işaretle | Kayıt Deprecated'a çekilir ve whisper/aramada öne çıkmaz | A |
| EX-07 | Migration | Eski DB açıldığında mevcut tüm kayıtlar Approved olur, veri kaybı olmaz. Kayıt sayısı migration öncesi ve sonrası aynı | S3, B |
| EX-08 | Liste limiti | 12 limiti kalkar: infinite scroll ya da en az 50 kayıt ve toplam sayı görünür | A |
| EX-09 | Palette'ten odak | Ctrl+K / ⌘K'da seçilen tecrübe doğrudan detay drawer'ında açılır (kesik görünüm yok) | A, B |
| EX-10 | MCP yüzeyi | MCP'de update/delete tool'u **yok**; ajan sadece create ve search yapabilir | S3 |
| EX-11 | SemanticMap | Ağaç seçimi ve pager regresyonsuz çalışır. Uzun isimlerde tooltip var (V6) | A |
| EX-12 | Graph UI butonu | Tauri'de Enable/Open görünür ve çalışır. Browser'da açıklayıcı bir placeholder gösterilir, header boş görünmez (V11) | M |
| EX-14 | Toplam = bridge | Vault’taki nodes/edges/files toplamı memory bridge toplamıyla aynı; query `LIMIT` (ör. 400/800) toplam diye gösterilmez (live S2) | A, M, B |
| EX-15 | Log sanitizasyonu | Experience Log ham markdown/prompt dump (`## … ### Tecrübeler`) ve iç TR hata (`memory_bridge hata:`) göstermez; özetler kullanıcıya uygun | A, B |
| EX-LAYOUT | L1–L6 | **Sercan'ın eki:** "Semantic Map + Experiences" içerik alanının ≥%85'ini kaplar. Sağ yarı ve alt kısım boş kalmaz. Dikeyde paneller alt alta | A, M, B |

### 5.4 Dead Symbols (Vault + Health), P0

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| DS-01 | Tüm liste | 8 limiti kalkar. Virtual list ya da infinite scroll ile tümü görünür, toplam sayı gösterilir | A, B |
| DS-02 | Detay | Satıra tıklanınca name, kind, file:line, detail, project ve `last_ref` görünür | A, B |
| DS-03 | Editörde aç | OPEN_IN_EDITOR ilgili dosyayı doğru satırda açar: macOS `open` · Windows `start` · Linux `xdg-open` (veya Tauri opener); Settings Editor override (bkz. O3 / §10.2) | M, B |
| DS-04 | Yolu kopyala | Pano içeriği `file:line` olur | A |
| DS-05 | Ignore | Sembol listeden anında kalkar, KPI 1 azalır, `ignored_symbols`'a yazılır. Reload ve yeniden index sonrası da gizli kalır | A, S3, B |
| DS-06 | Ignore listesi | `/health` → "Ignore List" sekmesinde görünür ve oradan geri alınabilir | A |
| DS-07 | Ajan ile düzelt | FIX_WITH_AGENT, NATS'e `lounge.task.requested` yayınlar. Payload'da file, line, symbol ve project var. UI'da görev oluşturuldu bildirimi çıkar | S3, A, B |
| DS-08 | Referans verisi | Bridge `last_ref`'i codebase-memory-mcp'den alır. Veri yoksa açıkça "referans yok" yazar | S3 |
| DS-LAYOUT | L1–L6 | Liste ve detay paneli D0–D3'te | A, M, B |

### 5.5 Health (`/health`) ve Map boş durumları

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| HM-01 | Boş DB | Health'te `MOCK_HEALTH`'e ait 12/41/74 değerleri **yok**. "No data found." mesajı ve [Index Workspace Now] CTA'sı var | A, B |
| HM-02 | Boş Map | `MOCK_NODES` yok, aynı empty state ve CTA var | A, B |
| HM-03 | Dolu DB | Satırlar gerçek index health'ini gösterir. Dead sayısına tıklanınca ilgili projenin dead listesine gidilir (drill-down) | A, B |
| HM-04 | Re-index | Re-index yalnızca `/health`'teki butonda var, palette'te yok | A |
| HM-05 | Mock yasağı | Canlı kod yolunda `MOCK_HEALTH` ve `MOCK_NODES` referansı yok (S4 grep kapısı) | S4, B |
| HM-LAYOUT | L1–L6 | D0–D3 | A, M, B |

### 5.6 Settings (`/settings`)

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| ST-01 | Routing tablosu | 1280x800'de ve UI scale %130'da tablo kırpılmaz; responsive yerleşim ya da görünür scroll ipucu var (V8) | A, B |
| ST-02 | Routing policy kaydı | Kartlar ve input'lar `set_routing_policy` ile kaydedilir, reload sonrası korunur | A, M |
| ST-03 | UI scale | %90/%100/%130 root rem'i değiştirir, localStorage'da kalır. Hiçbir sayfada L5 ihlali olmaz | A |
| ST-04 | Graph port | Kaydedilince `set_graph_ui_port` çağrılır. Açık graph penceresi kapanır ve yeni portla yeniden yaratılır | M, B |
| ST-05 | Kilitli approval checkbox | Neden kilitli olduğunu anlatan tooltip ya da metin var (disabled ama açıklamalı) | A |
| ST-06 | Yeniden tara | `/onboarding`'e gider | A |
| ST-LAYOUT | L1–L6 | D0–D3 | A, M, B |

### 5.7 Graph UI penceresi (Tauri)

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| GR-01 | Enable / Open | `ask()` onayı sonrası graph penceresi açılır. Durum `get_graph_ui_status` ile tutarlı | M, B |
| GR-02 | Port değişimi | ST-04 ile aynı. Aynı label ile yeniden yaratmada yarış ya da hata yok | M |
| GR-03 | Main kapanışı | Main pencere kapanınca graph-window ve child process (cbm UI) sonlanır. `ps` ile doğrulanır, yetim process kalmaz | M, B |
| GR-04 | 3 ekran | Graph penceresi D1–D3'te açılır ve boyutlanır, dikeyde kullanılabilir kalır | M |

### 5.8 Onay banner'ları

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| AP-01 | Security Onayla / Reddet | `resolve_routing` doğru kararla çağrılır, banner kapanır | A (`?demo=` ve mock), M |
| AP-02 | Quota Yerel / Kapat | Vote `resolve_routing`'e gider | A, M |
| AP-03 | Routing approve / deny / local | `?demo=routing-banner` ile 3 aksiyonun her biri doğru payload'ı üretir | A |
| AP-04 | Laya enable / decline / hide | `enable_decision_gate` / `decline_decision_gate` / local dismiss çalışır, reload sonrası tutarlı | A, M |
| AP-05 | Erişilebilirlik ve yerleşim | `role="alertdialog"`, focus trap ve Esc davranışı. Banner dikey ekranda içeriği taşırmaz | A |
| AP-06 | Yıkıcı işlem onayı | DB drop/truncate/delete/migrate-down, `rm -rf`, reset **ve** Windows (`del /s`, `rd /s`, `Remove-Item -Recurse`, `format`) her zaman onay ister (bkz. §10.2) | A, S3, B |
| AP-07 | Never Ask atlayamaz | "Never Ask" açıkken bile yıkıcı işlem onayını atlayamaz | A, S3, B |
| AP-08 | Onay sesi | Bekleyen onayda ses; arka plan/gizli pencerede de; varsayılan 60 sn tekrar; karar sonrası durur (bkz. §10.1) | A, M, B |
| AP-09 | Ses ayarları | Settings: aç/kapa, yerleşik sesler, özel wav/mp3/ogg/aiff, volume, aralık, Dinle; kalıcı + anında (bkz. §10.1 / §10.2) | A, B |
| AP-10 | OS bildirimi | Bekleyen onayda Tauri notification (macOS/Windows/Linux); tıklanınca app + banner odak (bkz. §10.1 / §10.2) | M, B |

### 5.9 Komut paleti (Ctrl+K / ⌘K)

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| CP-01 | Arama | Tauri'de `search_experiences` ve `search_index_nodes` çağrılır. Sonuç özetleri okunabilir uzunlukta | A, M |
| CP-02 | Tecrübe seçimi | EX-09 | A, B |
| CP-03 | Switch Project | `/vault`'a gider, proje odaklanmış olur | A |
| CP-04 | Trigger Grok Test | NATS'e `lounge.task.requested` gider. Browser'da açıklamalı disabled | S3, A |
| CP-05 | Kapsam | Re-index ya da Clear Cache komutu yok (K10) | A |

### 5.10 Fleet, Stream, Telemetry, Quotas, Onboarding

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| FL-01 | Worker offline | Bir Python worker (Grok) kapatıldığında 45 sn içinde status "Offline" olur (heartbeat) | S3, M |
| FL-02 | Worker online | Heartbeat gelince worker Dashboard'da ve Fleet'te anında görünür | S3 |
| SR-01 | `/stream` | Dashboard EventStream kontrolleriyle aynı davranır (regresyon) | A |
| SR-02 | Heartbeat filtresi | Varsayılan görünümde `lounge.*.heartbeat` satırları filtrelenir veya collapse edilir; boş `Decision: —` chip’i gerçek trafiği gömmez (live S2) | A, B |
| TL-01 | `/telemetry` | Weekly/project filtreleri ve "Markdown indir" dosyası beklenen içerikle iner | A |
| QT-01 | `/quotas` | Filtre sekmeleri ve satırlar çalışır | A |
| OB-01 | Onboarding: seçimsiz | Hiç tool seçilmemişken "Sistemi Başlat / Finish" disabled | A, B |
| OB-02 | Onboarding: kayıt | `save_selected_tools` sonrası `/dashboard`'a gidilir, seçim kalıcı olur | A, M |
| OB-03 | Yeniden tara / HF pull | `get_discovery_report` ve `pull_lmr_model` çalışır, hata mesajı anlaşılır | M |
| *-LAYOUT | L1–L6 | Her biri için D0–D3 | A, M, B |

### 5.11 Çapraz kesen kontroller

| ID | Test | Kabul kriteri | Etiket |
|----|------|---------------|--------|
| X-01 | Dil birliği | Tüm UI tek dilde (dil kararı Sercan'da, bkz. O1). Hardcoded karışık string yok (V7) | A |
| X-02 | Dotted İ | `lang="tr"` + CSS `uppercase` İngilizce etiketleri "TİME"/"SEMANTİC" yapmamalı (live G2); EN UI’da `lang=en` veya uppercase güvenli | A, B |
| X-02 | Min font | 12px'ten küçük font yok (`text-meta` = 0.75rem). `text-[9|10|10.5|11px]` grep'i 0 | S4 |
| X-03 | Konsol hataları | Her route'ta uncaught error ya da React warning yok | A, B |
| X-04 | Handler taraması | Görünür her tıklanabilir eleman bir etki üretir: navigasyon, IPC çağrısı, state değişimi ya da açıklamalı disabled | A, B |

## 6. Bloklama kuralları

1. **Bloklayıcı [B]:**
   - [B] işaretli bir case'in fail etmesi.
   - P0 sınıfı hata: CRUD bozukluğu, veri kaybı, güvenlik açığı, ana akış çökmesi.
   - Görsel regresyonun %3 eşiğini aşması (bilinçli değişiklikte baseline, reviewer onayıyla güncellenir).
   - Herhangi bir sayfa/viewport'ta L1–L5 ihlali (P1 layout hatası). Dikey ekranda L6 ihlali.
   - Canlı kod yolunda `MOCK_HEALTH` / `MOCK_NODES` (S4).
   - Mevcut CI'nın (lint, types, unit, cargo, workers) kırılması.
2. **Bloklayıcı değil, issue açılır:** P2/P3 görsel kusurlar (layout dışı), tooltip ya da metin cilası.
3. **Mac canlı testi (S2):** UI değiştiren her PR'da D1–D3 checklist'i ve ekran görüntüleri PR'a eklenmeden merge yok. Mac müsait değilse PR bekler, bypass yok.
4. Her PR mevcut akışlarda regresyon olmadığını kanıtlar: ilgili S1 suite'i yeşil olmalı.

## 7. Uygulama sırası ve PR bölümü

Gemini'nin 5 PR'ı temel alındı. Test altyapısını ve baseline koşusunu başa ekledim (Sercan'ın istediği sıra: plan, testler, geliştirme).

| Sıra | PR | Kapsam | Bağımlılık | Kabul testleri |
|------|----|--------|------------|----------------|
| 0 | **PR-0 QA harness** | Playwright kurulumu, IPC mock fixture'ı (dolu/boş), `data-qa` hook'ları, L1–L6 yardımcıları, 4 viewport, screenshot artifact, mock-yasağı grep (başta uyarı modunda), Mac için `screencapture` + ekran taşıma script'i ve `QA_Report.md` şablonu. **Ürün kodu değişmez.** | — | Suite çalışır. Bugünkü durumda beklenen fail'ler (audit ile uyumlu) raporlanır: **baseline koşusu** |
| 1 | **PR-1 Backend-Core** | Experience şeması (`status`, `is_pinned`, `updated_at`, `original_content`, `archived_at`, `reviewed`, `use_count`, `last_used_at`) ve migration; Tauri experience komutları; `ignored_symbols`; dead-symbol → görev; OPEN_IN_EDITOR; MCP create → approved+`reviewed=false`; arşiv search fallback; yıkıcı-işlem gate; **onay sesi/bildirim tetikleyicisi** (§10.1) | PR-0 | EX-05, EX-07, EX-10, EX-13 (backend), DS-05/07 (S3), AP-06…08/10 (tetik), cargo test |
| 2 | **PR-2 Cleanup-Shell** | New Node, Quick Filter, Docs, API Keys kaldırılır; Bell → Alert History; MOCK fallback yerine empty state; KPI düzeltmesi; daemon durumları; **layout doluluğu düzeltmeleri** (Vault yarım panel, Dashboard 1280 fold, dikey stack, Settings tablo); dil birliği (O1 kararından sonra) | PR-0 (PR-1'den bağımsız, paralel gidebilir) | SH-*, DB-01/02/04, HM-01/02/05, ST-01, X-*, tüm *-LAYOUT. Mock grep kapısı burada zorunluya çevrilir |
| 3 | **PR-3 Vault-Experience** | Detay drawer, edit/archive/pin/approve UI, Show Archived, liste limiti, palette odağı | PR-1, PR-2 | EX-01…EX-09, EX-LAYOUT, CP-02 |
| 4 | **PR-4 Health-Symbols** | Dead Symbols tam liste, detay, aksiyonlar (aç, kopyala, ignore, ajanla düzelt), Ignore List sekmesi, `last_ref` bridge güncellemesi, Health drill-down | PR-1, PR-2 | DS-*, HM-03/04, DS-LAYOUT |
| 5 | **PR-5 System-Settings** | Settings onarımları (i18n dili, Editor, **onay sesi prefs + Dinle**, TTL), Graph UI lifecycle, onboarding, Fleet heartbeat; bildirim tıklanınca odak (§10.1 AP-09/10) | PR-2 | ST-*, GR-*, OB-*, FL-*, AP-08/09/10 (UI) |
| 6 | **Final grand test** | Tüm S1 suite'i, D1–D3'te tam S2 turu, "Proje A'dan Proje B'ye tecrübe aktarımı" demosu (Gemini) | PR-1…5 | Tüm [B] case'ler yeşil, `QA_Report.md` Sercan'a sunulur |

Not: Gemini "NATS heartbeat iyileştirmesi"ni PR-1'e koymuştu. Fleet test'leri (FL-*) PR-5'te olduğu için heartbeat işini PR-5'e taşımayı öneriyorum; bu küçük bir teknik tercih.

## 8. Sercan'ın kararı gereken açık konular

| # | Konu | Gemini'nin önerisi | Neden Sercan'da |
|---|------|--------------------|-----------------|
| O1 | **UI dili**: tamamen İngilizce mi, tamamen Türkçe mi? | İngilizce ("Global Open Source vizyonu") | UX tercihi. Sercan Türkçe yazıyor, bugünkü UI karışık (V7) |
| O2 | **Kalıcı silme (hard delete/purge)**: kullanıcıda olsun mu? | Hayır, sadece Archive/soft delete | Veri silme kararı |
| O3 | **"Editörde aç" hedefi**: VS Code, Cursor ya da sistem varsayılanı? | Soru olarak bıraktı | Kişisel araç tercihi |
| O4 | **DecisionGate granülerliği**: "Always Ask" / "Never Ask" dışında "sadece kritik dosyalarda sor" modu istenir mi? | Soru olarak bıraktı | Güvenlik/UX tercihi |
| O5 | **Otomatik deprecate**: eski ya da hiç kullanılmamış tecrübeler otomatik "deprecated" işaretlensin mi? | Soru olarak bıraktı | Veri yaşam döngüsü |
| O6 | **Draft onay yükü**: Ajan kayıtları Draft gelecek, onaylanmadan whisper'da kullanılmayacak. Bu, kullanıcıya düzenli bir onay işi getirir. Kabul mü? | Evet (K2) | İş akışını değiştiren UX kararı. Gemini karar verdi, Sercan'ın teyidi önerilir |
| O7 | **New Node'un kaldırılması**: Sercan "anlamlı olsun ya da kalksın" demişti. Gemini kaldırmayı seçti. İleride `/fleet`'te "Connect Worker" istenir mi? | Kaldır (K7) | Teyit |
| O8 | **Mac canlı test takvimi**: S2 her UI PR'ında Sercan'ın Mac'ini ve 3 ekranını kullanır, pencere taşıma ekranları meşgul eder. Hangi saatler uygun? | — | Sercan'ın cihazı ve zamanı |

## 9. Sonraki adım

1. Sercan O1–O8'i yanıtlar.
2. PR-0 (QA harness + baseline koşusu) açılır; ürün kodu değişmez.
3. Baseline sonuçları (beklenen fail listesi) ile bu plan güncellenir ve geliştirme planına (PR-1…PR-5) dönüştürülür.

## 10. Sercan'ın kararları (26 Eylül 2026, 18:21)

| # | Karar | Plana etkisi |
|---|-------|--------------|
| O1 | UI'da **Türkçe / İngilizce seçimi** olacak (i18n) | X-01 "tek dil" yerine: tüm string'ler i18n sözlüğünden gelir, TR/EN geçişi Settings'te, seçim kalıcı; hardcoded string yok. PR-2'ye i18n altyapısı eklenir |
| O2 | Kalıcı silme yok, **arşiv**. Ajan aktif tecrübelerde sonuç bulamazsa **arşive de bakar** | EX-03 aynen. Yeni S3 case: `lounge_search_experience` aktifte sonuç yoksa arşivden döner ve sonucu "archived" diye işaretler (PR-1) |
| O3 | "Editörde aç" **sistem varsayılan editörü** ile; kullanıcı Settings'ten başka editör seçebilir | DS-03: macOS `open` · Windows `start` · Linux `xdg-open` (veya Tauri opener); Settings'e "Editor" seçimi (Varsayılan / VS Code / Cursor / özel komut) eklenir (PR-4/PR-5). Bkz. §10.2 |
| O4 | Veritabanı vb. **yıkıcı işlemlerde kullanıcıya sorulmalı ve teyit alınmalı** | Yeni özellik: destructive-operation gate. POSIX (`rm -rf`, …) **ve** Windows (`del /s`, `rd /s`, `Remove-Item -Recurse`, `format`) + DB drop/truncate/delete/migrate-down/reset DecisionGate'te her zaman onay ister, "Never Ask" ile bile atlanamaz. AP-06/AP-07 (PR-1 + PR-5). Bkz. §10.2 |
| O5 | **TTL + kullanım sayısı** ile otomatik arşiv | Şemaya `use_count`, `last_used_at`; TTL ve eşik Settings'te ayarlanabilir; arşive giden kayıt geri alınabilir. Yeni case EX-13 (PR-1/PR-3) |
| O6 | Ajan tecrübeleri **otomatik onaylanır**; incelenmemiş olanların sayısı **rozetle** gösterilir | K2 (Draft) değişti: MCP kaydı doğrudan aktif olur ama `reviewed=false`; sidebar Knowledge Vault'ta rozet = incelenmemiş sayısı; detayı açmak ya da "incelendi" demek rozeti azaltır. EX-05 buna göre güncellenir |
| O7 | New Node **kaldırılır** | K7 teyit |
| O8 | Canlı test için ekranlar şu an boş | S2 baseline koşusu 18:21'de başlatıldı |

### 10.1 Onay beklerken ses + OS bildirimi (Sercan, aynı gün)

Routing, security, quota ve yıkıcı-işlem onayları beklerken kullanıcıyı kaçırmamak için ses ve **yerel OS bildirimi** (macOS / Windows / Linux — bkz. §10.2).

| ID | Test | Kabul kriteri | Etiket | PR |
|----|------|---------------|--------|-----|
| AP-08 | Onay sesi | Routing / security / quota / destructive onay beklerken ses çalar (webview `HTMLAudioElement` veya Rust `rodio`; paketlenen wav/mp3/ogg). Uygulama arka planda veya pencere gizliyken de çalar. Kullanıcı karar verene kadar varsayılan **60 sn** aralıkla tekrar eder; karar sonrası durur. Otomasyon: audio playback spy | A, M, B | Backend tetik PR-1; UI/prefs PR-5 |
| AP-09 | Ses ayarları | Settings'te: aç/kapa, yerleşik ses seçimi, özel dosya yükleme (**wav/mp3/ogg/aiff**), ses seviyesi, tekrar aralığı, **Dinle** (preview). Seçim kalıcıdır ve anında uygulanır | A, B | PR-5 |
| AP-10 | OS bildirimi | Onay beklerken **Tauri notification plugin** ile yerel bildirim (üç OS). Tıklanınca uygulama öne gelir ve ilgili onay banner'ı odaklanır | M, B | Tetik PR-1; odak PR-5 |

Not: Ses, görünürlük API'sine bağlı olmamalı (background/hidden). Tekrar aralığı ve ses dosyası prefs'te saklanır.

### 10.2 Cross-platform ilkesi

Uygulama **macOS, Windows ve Linux** üzerinde çalışır. Platforma özel varsayımlar (yalnızca `open`, yalnızca ⌘K, yalnızca macOS bildirimi) kabul edilmez.

| Alan | İlke |
|------|------|
| Bildirim (AP-10) | Tauri **notification** plugin; üç OS'ta yerel bildirim + tıklayınca odak |
| Ses (AP-08/09) | Webview HTML Audio **veya** Rust `rodio`; paketlenen **wav/mp3/ogg** (+ aiff yükleme opsiyonel) |
| Editörde aç (DS-03) | macOS `open` · Windows `start` · Linux `xdg-open` — veya Tauri **opener** plugin; Settings override |
| Yıkıcı komut algılama (AP-06/07) | POSIX (`rm -rf`, …) **ve** Windows (`del /s`, `rd /s`, `Remove-Item -Recurse`, `format`, …) |
| Yollar | `/` ve `\` ayırıcıları + Windows sürücü harfleri (`C:\…`) doğru işlenir |
| Palette kısayolu | Windows/Linux **Ctrl+K**, macOS **⌘K**; UI etiketi platforma göre uyarlanır (SH-04) |
| CI | Linux: ana Playwright runner. Ayrıca `windows-latest` ve `macos-latest` üzerinde en az `cargo test` + frontend build — **ayrı non-blocking** (`qa-cross-platform.yml`). S2-Linux paketi: **non-blocking** `linux-bundle.yml` (AppImage + `.deb` → artifact `agent-lounge-linux`; yalnızca `main` + `workflow_dispatch`) |
| Canlı test (S2) | Mac script'leri Mac'e özel kalır. Windows / Linux checklist: `scripts/qa/windows/`, `scripts/qa/linux/` (Linux: 1280×800 + portrait window resize; runtime deps `RUNTIME_DEPS.md`) |

Canlı test şablonları: `scripts/qa/mac/QA_Report.md`, `scripts/qa/windows/QA_Report.md`, `scripts/qa/linux/{README,CHECKLIST,QA_Report,RUNTIME_DEPS}.md`.
