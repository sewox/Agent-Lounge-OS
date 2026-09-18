# 🖥️ AGENT_LOUNGE_OS_BLUEPRINT (v2.0 - Pure Rust Edition)

## 1. VİZYON VE FELSEFE
Agent Lounge OS, yerel ajanların (Claude, Cursor, Grok, Llama vb.) düşük gecikme ve yüksek güvenlikle haberleştiği, Rust ile geliştirilmiş, sistem seviyesinde bir orkestrasyon katmanıdır.

**Temel Prensipler:**
*   **High Performance:** Rust'ın Zero-cost abstractions ve Fearless Concurrency (Tokio) özelliklerini kullanarak ajan trafiğini yönetmek.
*   **Unified Memory:** Rust üzerinden `codebase-memory-mcp` (C) ile doğrudan ve hızlı veri alışverişi.
*   **Single Binary / Multi-Platform:** Bağımlılıklardan arınmış, Tauri ile paketlenmiş tek bir uygulama.
*   **Non-AI SLOOP UI:** Stitch tabanlı, mühendislik odaklı "Dashboard" yaklaşımı.

## 2. TEKNOLOJİ YIĞINI (RUST STACK)

### A. Core Engine (The Lounge Kernel) - RUST
*   **Runtime:** Tokio (Asenkron yönetim).
*   **Service Management:** Ollama ve NATS servislerinin yaşam döngüsü kontrolü (EchoMind mimarisi).
*   **Logic:** Dispatcher (Görev Dağıtıcı) mantığının Rust ile implementasyonu.
*   **HTTP/API:** Axum veya Actix-web (Gerektiğinde dış dünyaya veya UI'a API sunmak için).

### B. Messaging Bus - NATS (Local)
*   **Crate:** `nats` (Rust client).
*   **Görev:** Ajanlar arası asenkron Pub/Sub ve Request-Reply trafiği.

### C. Hafıza ve İndeksleme - RUST NATIVE / FFI
*   **C-Binary Bridge:** `codebase-memory-mcp` (C) ile `std::process::Command` veya doğrudan FFI üzerinden iletişim.
*   **Database:** `rusqlite` (İlişkisel veriler) ve `qdrant-client` veya `surrealdb` (Vektör/Graph hafıza).

### D. Arayüz (UI) - TAURI (Rust + Next.js)
*   **Frontend:** Next.js + Tailwind (Stitch üzerinden gelen tasarımlar).
*   **Backend (Tauri Commands):** UI'dan gelen talepleri Rust tarafındaki Kernel'e ileten katman.

## 3. SİSTEM MİMARİSİ (RUST WORKFLOW)
1.  **Bootstrapping:** Tauri uygulaması başladığında, Rust tarafı Ollama ve NATS'ın durumunu kontrol eder, gerekiyorsa başlatır.
2.  **Indexing:** `codebase-memory-mcp` üzerinden projeyi Rust katmanı tetikler ve çıktıları `rusqlite` üzerinde yapılandırır.
3.  **Task Orchestration:**
    *   Ajanlardan gelen talepler NATS üzerinden Rust Kernel'e düşer.
    *   Rust tarafındaki Dispatcher, Ollama API'sine (Llama 3.1) giderek karar alır.
    *   Karar sonucu ilgili ajan tetiklenir.
4.  **Experience Store:** Başarı ile tamamlanan her iş, Rust tarafından asenkron olarak hem ilişkisel hem de vektör veritabanına "Tecrübe" olarak kaydedilir.

## 4. KLASÖR YAPISI

```text
agent-lounge-os/
├── src-tauri/          # Rust: Ana OS Katmanı
│   ├── src/
│   │   ├── kernel/     # Görev dağıtıcı ve ajan mantığı
│   │   ├── services/   # Ollama, NATS, C-Binary yöneticileri
│   │   ├── db/         # SQLite ve Vektör DB entegrasyonu
│   │   └── main.rs     # Uygulama giriş noktası
├── src/                # Next.js: Stitch Dashboard UI
├── shared/             # Rust: Mesaj şemaları (Protobuf veya Structs)
├── bridge/             # C: codebase-memory-mcp binary'si ve wrapperları
└── experiences/        # Global Memory: ADR ve Knowledge kayıtları
```

## 5. GELİŞTİRİCİ (AI AGENT) TALİMATLARI
*   **Concurrency:** Ajan haberleşmeleri için Tokio kanallarını (channels) ve NATS Pub/Sub yapısını kullan.
*   **Memory Safety:** Veri paylaşımı için `Arc<Mutex<T>>` veya `RwLock` kullanarak thread-safe bir yapı kur.
*   **Zero-Copy Logic:** `codebase-memory-mcp` çıktılarını Rust tarafında işlerken gereksiz kopyalamadan kaçın, referanslarla çalış.
*   **Error Handling:** Her türlü servis hatasını (Ollama kapalıysa, NATS çöktüyse) UI'da teknik log olarak göster.
*   **Non-SLOOP Design:** UI tarafında animasyon yerine veri yoğunluklu mühendislik widget'ları kullan.