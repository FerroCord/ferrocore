# Açık kararlar, belirsizlikler ve kaynak sınırları

Bu liste, [özgün ADR](reference/architecture-decisions-2026-10-31.md) ve üst çalışma rehberinde eksik bırakılan uygulama ayrıntılarını görünür kılar. Aşağıdaki test ve karar başlıkları uygulama hazırlığıdır; kesinleşmiş yeni mimari kararlar olarak değerlendirilmemelidir.

Mevcut tanılama CLI'sı `cargo run -- check` ile çalıştırılır ve ferrocord `ae07423923b0934b0235d00fcd97c2bffeadd9ee` commit'ine bağlıdır. Linux ve Windows hedeflenir; Windows üzerinde çalıştırıldı, Linux doğrulanmadı. Faz 2 MVP tamamlanmadı; aşağıdaki LLM, sentinel ve platform sözleşmeleri açık kalır.

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

## Marketplace fikirleri — karar bekliyor

Bu bölüm, eklentilerin keşfi ve dağıtımı hakkında tartışılan fikirleri kaydeder. Marketplace'in geliştirilmesi, kapsamı, takvimi ve dağıtım biçimi açık kararlardır. Aşağıdaki seçenekler değerlendirme içindir; uygulama taahhüdü içermez.

### Olası kapsam ve aşamalar

- Git üzerinde JSON/TOML manifestleri, açıklamalar ve kurulum belgeleri tutan küçük bir eklenti kataloğuyla başlanabilir.
- İhtiyaç oluşursa CLI üzerinden arama, ayrıntı görüntüleme, kullanıcı onaylı kurulum, sürüm sabitleme, güncelleme ve kaldırma eklenebilir.
- Katalog büyüdüğünde arama, kategori, uyumluluk ve izin bilgileri sunan bir web arayüzü değerlendirilebilir.
- Düzenli üçüncü taraf katkısı ve kullanıcı talebi oluşursa yayıncı hesapları, inceleme, raporlama ve ücretlendirme seçenekleri ele alınabilir. Talep ve ticari karşılık henüz doğrulanmadı.

Bu sıralama olası bir ilerleme yoludur; yol haritasındaki fazlara atanmış değildir. İlk katalog için özel sunucu, hesap sistemi ve veritabanı gereksinimi ayrıca değerlendirilebilir.

### Kaldırılabilir marketplace eklentisi seçeneği

Marketplace'in isteğe bağlı kurulabilen ve kaldırılabilen bir Ferrocore eklentisi olması tartışılan seçeneklerden biridir. Ürünle birlikte sunulması veya ayrı bir araç olarak dağıtılması da değerlendirmeye açıktır.

Kaldırılabilir eklenti seçeneğinde şu sorumluluk ayrımı düşünülebilir:

| Bileşen adayı | Olası sorumluluk |
| --- | --- |
| Ferrocore eklenti yönetimi | Kurulu eklentileri takip etme, izinleri uygulama, etkinleştirme, devre dışı bırakma ve kaldırma |
| Marketplace eklentisi | Katalogda arama, ayrıntıları gösterme ve kurulum/güncelleme isteklerini eklenti yönetimine iletme |

Bu seçenek benimsenirse aşağıdaki davranışların kararlaştırılması gerekir:

- Marketplace kaldırıldığında diğer kurulu eklentilerin korunması ve çalışmayı sürdürmesi.
- Marketplace olmadan yerel CLI üzerinden eklenti yönetimi ve marketplace'i yeniden kurma yolu.
- Marketplace kaldırıldığında katalog tarama ve güncelleme keşfinin nasıl etkileneceği.
- Önbellek ve kullanıcı ayarlarını temizlemenin ayrı bir kullanıcı seçimi olması.
- Marketplace'in kendi izinlerinin, yönetim yetkilerinin ve kullanıcı onaylarının sınırları.

### Güvenlik, dağıtım ve mimari soruları

- Manifestte yayıncı kimliği, kaynak depo, sürüm, desteklenen Ferrocore sürümleri ve platformlar nasıl belirtilecek?
- WASM ve dış süreç eklentilerinin ek çalışma zamanı ihtiyaçları nasıl gösterilecek?
- Dosya sistemi, ağ, terminal ve gizli bilgilere erişim izinleri nasıl bildirilecek ve çalışma anında nasıl uygulanacak? Manifestte izin listelemek tek başına erişim denetimi sağlamaz; dış süreçlerin izolasyon sınırı ayrıca tanımlanmalıdır.
- Sürüm sabitleme ve indirilen dosyanın bütünlük doğrulaması nasıl sağlanacak? Güvenlik açığı bulunan sürümler nasıl işaretlenecek ve kullanıcıya bildirilecek?
- Katalog bakımı, yayıncı doğrulama ve güvenlik bildirimleri için gereken insan ve altyapı kaynakları nasıl karşılanacak?
- Kullanıcıya dönük katalog ve kurulum deneyimi Ferrocore tarafında konumlandırılabilir. Ferrocord'un genel amaçlı servis, olay ve eklenti yaşam döngüsü sınırı korunmalıdır.
- Birinci parti döngü modülleri ile harici MCP/WASM araçlarının dağıtım ve uyumluluk kuralları nasıl ayrılacak? Her eklenti türünün aynı yöntemle indirilebilir olması henüz tanımlanmadı.
- Kaldırılabilir marketplace seçilirse gerekli yönetim yetkileri MCP/WASM sözleşmelerine nasıl açılacak?

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
