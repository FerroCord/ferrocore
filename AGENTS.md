# ferrocore ajan çalışma rehberi

## Durum ve kaynak

Bu depo tasarım aşamasındadır. İlk dokümantasyon hazırlığında Rust kaynakları, Cargo manifestleri, testler ve CI bulunmuyordu. Uygulamadan önce güncel dosyaları incele.

- Kaynak kararlar: `docs/reference/architecture-decisions-2026-10-31.md`.
- Tarihsel durum: `docs/reference/progress-2026-10-31.md`.
- Güncel uygulama rehberi: `docs/architecture.md`, `docs/roadmap.md`, `docs/open-questions.md`.
- Referans kopyalarını kaynak metin olarak koru. Kaynak iddiaları ve hedef tasarım ile gözlenmiş uygulamayı ayır.

## Mimari sınırlar

- ferrocord, ayrı depodaki genel amaçlı mikro-çekirdektir. Ajan, LLM, prompt ve terminal alanlarını ferrocore içinde tut.
- Birinci parti modüller Cargo workspace içinde; harici `ferrocore-plugin-*` eklentileri ayrı depolarda planlanır.
- `ferrocore-types`: AgentLoop, Tool, Session sözleşmeleri ve veri modelleri.
- `ferrocore-driver`: TurnDriver, LLM akışı, retry, bütçe, iptal ve JSONL kayıt.
- `ferrocore-runtime-pty`: kalıcı terminal, sentinel, çıktı ve spawn iletişimi.
- RLM ve ReAct, ortak sürücüyü kullanan kardeş döngülerdir. Ortak davranışı sürücüde paylaş.
- `ferrocore-tools`: MCP uyumlu IPC ve WASM adaptörleri.
- CLI ve `internal-spawn` aynı ikilide planlanır. Yalnızca istenen fazın modüllerini oluştur.

## Uygulama kuralları

- Açık ve doğal Türkçe iletişim kur. Önce ilgili kaynakları incele, kısa plan paylaş, uygula, ilgili kontrolleri çalıştır ve ayrı alt ajana diff incelet.
- SOLID, KISS, DRY ve YAGNI ilkelerine uy. İlgisiz kullanıcı çalışmalarını koru.
- Servis erişiminde kısa süreli senkron kilit ve `Arc` tasarımını koru; `.await` boyunca senkron kilit tutma. Çekirdek olay yolunu olay tipine göre jenerik kullan.
- Sentinel byte sözleşmesini uygulamadan önce netleştir. Parçalı akış, çıkış kodu, iptal ve süreç kapanışını test et.
- Alt ajan izinlerini aktif LLM/komut yürütmesiyle sınırla; çocuk beklenirken izni bırak. Derinlik kotalarını koru.
- JSONL alanları: `id`, `parent_id`, `agent_depth`, `loop_type`, `timestamp`, `payload`. Eşzamanlı yazma ve ebeveyn ilişkilerini test et.
- Geçici çıktıyı `$FC_SCRATCH` üzerinden yönet. Windows kabuk, IPC ve geçici dizin davranışını ayrıca netleştir.
- ADR çıktı eşiği 10 KB veya 150 satırdır; bağlama ilk 50 ve son 50 satır, tam çıktı `$FC_SCRATCH/last_output.log`. Paralel çıktı sahipliğini çöz.
- Mimari ağaçlar ve CLI örnekleri hedef tasarımdır. Eksik protokolü, doğrulanmamış performans iddiasını veya kesinleşmemiş API imzasını varsayma.
- Silme, push, yayınlama, birleştirme ve paylaşılan sistemlere yan etki için kullanıcı onayı al.

## Doğrulama ve rapor

Manifestler oluştuğunda gerçek araç zinciri ve CI üzerinden kapsam belirle. Aday komutlar `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` ve `cargo test --workspace` biçimindedir; henüz bu depoda doğrulanmadı.

PTY/sentinel, iptal, kota, JSONL ağacı ve döngüler arası davranış değişikliklerini ilgili testlerle kapsa. Darboğazları ve kaynak sınırlarını raporla. Web arayüzü eklenirse değişen akışı ve ortak durum kullanan rotaları tarayıcıda doğrula; düzen değişikliklerinde masaüstü/mobil görünümü değerlendir.

Sonuçta değişikliği, gerekçeyi, çalıştırılan kontrolleri, bağımsız inceleme durumunu ve doğrulanmamış noktaları belirt.
