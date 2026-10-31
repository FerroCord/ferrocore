# Açık kararlar, belirsizlikler ve kaynak sınırları

Bu liste, [özgün ADR](reference/architecture-decisions-2026-10-31.md) ve üst çalışma rehberinde eksik bırakılan uygulama ayrıntılarını görünür kılar. Aşağıdaki test ve karar başlıkları uygulama hazırlığıdır; kesinleşmiş yeni mimari kararlar olarak değerlendirilmemelidir.

## Uygulama öncesi sözleşmeler

| Konu | Netleştirilecek ayrıntı | Etkilenen faz |
| --- | --- | --- |
| Sentinel | Byte/kaçış biçimi, komut kimliği, çakışma önleme, çıkış kodu, bozuk/yarım çerçeve davranışı | 2 öncesi |
| Kabuk ve platform | POSIX kabuk seçimi; Windows PTY, kabuk, IPC ve scratch davranışı; destek matrisi | 2–3 |
| Scratch | `/dev/shm` yokluğu veya yetersizliği, izinler, yaşam süresi ve temizlik | 2 |
| Çıktı budama | 10 KB ölçümü, uzun tek satır, UTF-8 sınırı, 50+50 satır örtüşmesi, eşzamanlı `last_output.log` sahipliği | 2 |
| Ortak API | AgentLoop, Tool, Session imzaları; async ve hata tipleri; config şeması | 2 |
| LLM | Sağlayıcı, akış protokolü, kimlik bilgisi yönetimi, yeniden deneme kapsamı ve üst sınırı | 2 |
| Bütçe | Token/maliyet hesaplama, limit aşımı ve iptal davranışı | 2 |
| JSONL | Alan tipleri, kök `parent_id`, kimlik/sürüm, yazma sahipliği, sıralama ve kısmi satır kurtarma | 2–3 |
| Spawn | Kimlik doğrulama/izinler, taşıma protokolü, derinlik ve kota değerleri, hata ve iptal yayılımı | 3 |
| Eklenti | MCP sürümü/taşıması, WASM motoru seçimi, dosya/ağ yetkileri, dış bağımlılıklar | 4 |
| İleri döngüler | Doğrulama tekrar sınırı, MCTS skor formülü, FSM hata geçişleri ve ambient kaynak sınırları | 4–5+ |
| Dağıtım | Rust araç zinciri, ferrocord sürümleme, hedef platformlar, CI matrisi ve yayın onayı | İlgili faz |

## Kaynakta eksik görseller

- `image1`: Sentinel çerçeve biçimi yerine yer tutucu bulunuyor.
- `image3`: Reflexion doğrulama akışı yerine yer tutucu bulunuyor.
- `image2` ve `image4`: Kota ve skor anlatımında gömülü görseller var. Makineyle uygulanabilir metinsel formül ayrıca tanımlanmalıdır.
- `image5`: Akış okları için kullanılan gömülü görsel; özgün kopyada korunur.

Kaynağın biçim bozulması içeren `spawn-agent`/wrapper satırı, uygulamada kullanılacak kesin komut sözleşmesi sağlamaz. Komut satırı ve kaçış davranışı uygulama öncesinde yazılı olarak netleştirilmelidir.

## Kaynak iddialarının doğrulama durumu

Özgün ADR; DeepSeek Harness, Cordis, Prime Agent, Aider, LangGraph ve diğer projeleri ilham veya karşılaştırma olarak anıyor. Bu depoda bu projelerin özellikleri, port durumu, API uyumu ve performansı bağımsız kaynaklarla doğrulanmadı.

Nanosaniye düzeyinde kilit süresi, sıfır ayrıştırma hatası, context drift'in tamamen giderilmesi, %40–60 maliyet azalması veya sonsuz döngü riskinin sıfır olması gibi ifadeler ölçülmüş sonuçlar değildir. Ürün garantisi vermek için ölçüm ve hata sınırı gerekir. Alternatif isimlerin marka ve crates.io uygunluğu da doğrulanmadı.

[İlerleme kaydı](reference/progress-2026-10-31.md), depo oluşturma ve o tarihteki görünürlük doğrulamasını içerir. Kaydın sonrasındaki yerel değişiklikler ayrıca gözlemlenmelidir.

## Tasarım riskleri ve olası darboğazlar

Bunlar tasarım incelemesinden çıkan olasılıklardır; performans ölçümü yapılmadı.

- Kalıcı PTY'nin paylaşılan durumunda komutların ve arka plan çıktısının eşleştirilmesi gerekir.
- Tek JSONL dosyasına paralel ajan yazıları sıralama, bütünlük ve disk kapasitesi baskısı yaratabilir.
- Tam çıktı kaydı ve bellek diski kullanımı depolama sınırlarına tabidir.
- Yanlış izin sahipliği, ebeveyn/çocuk beklemelerinde ilerlemeyi durdurabilir; LLM ve süreç maliyeti kota gerektirir.
- MCP süreçleri, WASM modülleri ve kabuk komutları için güven sınırı ve izin politikası belirlenmelidir.
- Git worktree çoğaltması disk ve test yürütme kapasitesini tüketir. Birleştirme ve silme adımları onay gerektirir.
- Tek ikili dağıtım hedefi, dış eklentilerin ve kullanıcının araçlarının bağımlılıklarını ayrıca değerlendirmeyi gerektirir.
