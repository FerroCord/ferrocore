# ferrocore mimarisi

## Kaynak ve karar durumu

Kaynak: [31 Ekim 2026 ADR](reference/architecture-decisions-2026-10-31.md). Workspace ortak tipler, JSONL yazıcısı, LLM/bütçe akışı, PTY, çıktı yönetimi ve RLM görev yürütmesi kodunu içerir. PTY kabul sınırları: Windows'ta yalnızca tek basit native/cmdlet/fonksiyon komutlarının çıkış durumu raporlanır; çok deyimli, koşullu veya kanıtlanamayan komutlar açık `AMBIGUOUS` çerçevesiyle fail-closed hata döndürür (`i32::MIN` dahil tüm `i32` native çıkış kodları korunur). Linux'ta ayrı ön plan ve ayrılmış süreçlerin durdurulduğu kanıtlanamaz; EOF alınamazsa eksik çıktı olasılığı hata olarak bildirilir ve süreç ağacı duruşu kesin doğrulanamadığında scratch korunur. `ferrocord` bağımlılığı `ae07423923b0934b0235d00fcd97c2bffeadd9ee` commit'ine sabitlenmiştir. Gerçek sağlayıcı ve platform kabul koşulları ayrıca doğrulanmalıdır.

## İki katman ve depo topolojisi

`ferrocord` genel amaçlı servis konteynerini, tip tabanlı olay yolunu ve eklenti yaşam döngüsünü sağlar (ADR-001). Ajan alanı, LLM, prompt, terminal ve araç davranışları ferrocore içinde kalır. Bağımlılık yönü `ferrocore -> ferrocord` olarak korunur.

ADR-006, iki ana depo ve ihtiyaç oldukça açılacak harici eklenti depolarını tanımlar:

| Depo | Sorumluluk |
| --- | --- |
| `FerroCord/ferrocord` | Bağımsız Rust mikro-çekirdek crate'i, kendi test ve CI hattı |
| `FerroCord/ferrocore` | Birinci parti ajan modülleri, CLI, tek ikili dağıtım ve workspace CI |
| `ferrocore-plugin-*` | Bağımsız MCP veya WASM eklentileri ve kendi sürüm hatları |

## Planlanan yerleşim

```text
ferrocore/
├── .github/workflows/ci.yml       # workspace CI
├── Cargo.toml                    # workspace ve ferrocord bağımlılığı
├── crates/
│   ├── ferrocore-types/
│   ├── ferrocore-driver/
│   ├── ferrocore-runtime-pty/
│   ├── ferrocore-loop-rlm/
│   ├── ferrocore-loop-react/
│   ├── ferrocore-loop-reflexion/  # ileri faz
│   ├── ferrocore-loop-plan/       # ileri faz
│   └── ferrocore-tools/
├── src/main.rs                   # CLI ve internal-spawn
└── config.example.toml
```

MCTS, FSM ve ambient modülleri ileri yol haritasındadır. Dizinler ilgili faza geçildiğinde oluşturulur.

## Sözleşmeler ve ortak sürücü

ADR-004 ve ADR-008 uyarınca `ferrocore-types`, ortak nesne güvenli async `AgentLoop`, `Tool` ve `Session` sözleşmelerini barındırır. `ferrocore run --config <dosya> <görev>` yapılandırma doğrulamasının ardından RLM, LLM, oturum kaydı ve PTY yolunu kullanır; yalnız `active_loop = "rlm"` kabul edilir. Örnek ayarlar `config.example.toml` içindedir. Alt ajanlar henüz uygulanmadı.

Tireyle başlayan dosya veya görev için `--` seçenek sonlandırıcısı kullanılabilir: `ferrocore run --config -- -config.toml -görev` ya da `ferrocore run --config config.toml -- -görev`.

`ferrocore-driver` içindeki ortak `TurnDriver` sorumlulukları:

- LLM yanıt akışı (`POST <base_url>/chat/completions`, HTTPS/loopback HTTP, çapraz origin kimlik koruması, `max_response_bytes` sınırı ve ilk delta öncesi üstel geri çekilme/full jitter yeniden deneme).
- Token ve tamsayı mikro-USD yukarı yuvarlamalı maliyet bütçesi (kullanım/fiyat veya bütçe hatasında yanıt kaydedilip yürütme durdurulur).
- Append-only JSONL oturum kaydı (`SessionLogWriter`).
- `CancellationToken`, Ctrl+C ve yapılandırılmış adım/oturum süreleri; platforma özgü sinyal yayılımı ayrıca doğrulanmalıdır.

`pty.step_timeout_secs` her LLM isteği (retry bekleyişleri dahil) ve her PTY yürütmesi için ayrı başlatılır; etkin sınır bu adımın sonu ile oturum sonundan daha erken olanıdır. Süreyi temsil edemeyen yapılandırma açık hatadır. Kapanışta süreç ağacı duruşu kanıtlanamıyorsa scratch korunur ve kullanıcı uyarılır; görev ve günlük kapanışı birlikte hata verirse iki hata da bildirilir.

RLM ve ReAct aynı sürücü ilkellerini kullanan kardeş eklentilerdir. Her döngü kendi girdi ayrıştırıcısını ve yürütme davranışını taşır. Böylece ortak hata, bütçe ve kayıt davranışı tek katmanda paylaşılır.

| Döngü | Özel davranış |
| --- | --- |
| RLM | Ham kabuk kod bloklarını ayrıştırır; PTY akışını ve spawn iletişimini izler; çalışma dizini, son çıkış kodu, arka plan işleri ve scratch içeriğini bağlama ekler. |
| ReAct | JSON Schema tabanlı araç çağrılarını oluşturur; araç kayıt sistemi üzerinden MCP/WASM çağrılarını yürütür; paralel araç çağrılarını yönetir. |

RLM, Markdown çitleri dışında kendi satırında sıfır girintili `<sh>` veya `<final>` bloklarını kabul eder; tur başına tek eylem ve iki biçim düzeltme denemesi uygulanır. Komut gövdesindeki satır içi `</sh>` literal metindir; yalnız başlı başına sıfır girintili `</sh>` satırı bloğu kapatır. Kapanmamış blok, kesilmiş veya normal bitişi doğrulanmamış yanıt yürütülmez. Gerçek sağlayıcı/model biçim uyumu henüz doğrulanmadı.

## Kalıcı terminal ve çıktı (ADR-002, ADR-003)

`ferrocore-runtime-pty`, `portable-pty` ile komutlar arasında çalışma dizinini ve oturum durumunu koruyan bir kabuk çalıştırır. Linux'ta `bash`, Windows'ta `pwsh.exe` PATH üzerinde bulunmalıdır; görevde çağrılan araçlar da ayrıca gereklidir. Ana ürün için Node.js veya Python gerekmez. Sentinel çerçevesi ve parçalı akış ayrıştırıcısı uygulanmıştır; platform sınırları yukarıdaki kabul durumunda belirtilmiştir.

Geçici çalışma alanı `$FC_SCRATCH` ile kabuğa aktarılır. ADR'deki platform yolları tasarım hedefleridir; gerçek scratch konumu ve izin davranışı uygulamaya ve platforma bağlıdır. Scratch bir gizlilik sınırı veya sır kasası değildir.

Tamamlanan komutun ham çıktısı scratch içindeki münhasır geçici dosyaya akıtılır; başarılı flush ve eşitleme sonrası `$FC_SCRATCH/last_output.log` olarak yayımlanır. `10240` bayt veya `150` satır aşıldığında örtüşmesiz ilk ve son `50` satır seçilir; görünüm yapılandırılmış bayt sınırına sığdırılır (kesme/kayıplı UTF-8 bildirimlerini sığdırmak için `PtyRuntime::new` en az `192` bayt gerektirir). Ham kayıt redakte edilmez ve gizli veri içerebilir; ayrıca korunmalıdır. Görünümün bayt sınırı token garantisi değildir. Yazma veya yayımlama hatası bildirilir; Windows'ta eşzamanlı okuyucu altında atomik değiştirme ve eski dosyanın korunması garanti edilmez.

## Alt ajanlar ve eşzamanlılık (ADR-003, ADR-008)

Ana ikili `ferrocore internal-spawn` alt komutunu da çalıştırır. POSIX tasarımında `spawn-agent`, PATH üzerindeki sembolik bağ veya kısa kabuk sarmalayıcısıyla aynı ikiliye yönlendirilir. Alt ajan istekleri Unix domain socket (UDS) üzerinden ana süreçle iletişim kuracak şekilde planlanır. Windows karşılığı açık karardır.

İzinler yalnızca aktif LLM/komut yürütmesi boyunca tutulur. Ebeveyn alt ajanı beklerken iznini bırakır. Her derinlik seviyesi bağımsız eşzamanlılık kotası kullanır. Sayısal kota, azami derinlik ve aşım davranışı henüz tanımlanmadı.

Ortak `AgentLoop`, çapraz döngü görevlendirmesine olanak verir. Kaynaktaki hedef kullanım örneği `spawn-agent --loop react "görev"` biçimindedir. Mevcut CLI `check` ve RLM görev yürütmesini sunar; spawn uygulanmadı. Faz 3'te spawn altyapısı hazırlanır; RLM/ReAct birlikte çalışma doğrulaması Faz 4'te ReAct tamamlanınca yapılır.

## Araç ve eklenti motoru (ADR-005)

Ortak `Tool` sözleşmesi arkasında iki adaptör hedeflenir:

- Dış süreç / IPC: Model Context Protocol (MCP) uyumlu JSON-RPC iletişimi.
- WebAssembly (WASM): Extism / Wasmtime tabanlı çalışma seçeneği.

Kesin protokol sürümü, taşıma, WASM motor seçimi ve izin modeli uygulama öncesi kararlardır. Tek ikili hedefi ana ürüne aittir; dış süreç eklentilerinin çalışma zamanı ihtiyaçları ayrıca belgelenir.

## Oturum kaydı (ADR-007)

Ana ajan ve alt ajan adımları tek append-only `session.jsonl` dosyasında ebeveyn bağlantılı ağaç/DAG modeliyle saklanır. Her satırın ADR'de tanımlı alanları:

| Alan | Anlam |
| --- | --- |
| `id` | Kaydın kimliği |
| `parent_id` | Ebeveyn kayıt bağlantısı |
| `agent_depth` | Ajan derinliği |
| `loop_type` | Döngü türü |
| `timestamp` | Zaman damgası |
| `payload` | Olay veya adım içeriği |

Faz 2 sözleşmesinde UUIDv7 kayıt kimlikleri, yalnızca ilk kayıtta null ebeveyn, sıfır ajan derinliği, UTC RFC3339 zaman damgası ve `v`, `kind`, `data` içeren sürümlü payload kullanılır. Tek yazıcı sınırlı kuyruk üzerinden her kayıt için yazma onayı verir; satırlar flush edilir, açılış, tur sonu ve kapanışta `sync_data` uygulanır. Bilinen API anahtarının tam eşleşmeleri kayıtlarda redakte edilir; bu, diğer gizli bilgileri korumayı garanti etmez. Kısmi veya bozuk kayıtlar sessizce düzeltilmez. İzinleri doğrulanamayan Windows kayıtları hata verir. Bu kayıt katmanı tek başına görev, PTY veya LLM yürütmez.

Gizli anahtar zorunlu şema değerleriyle veya `[REDACTED]` işaretiyle çakışırsa hem sabit şemayı hem anahtar gizliliğini koruyarak söz konusu satırı yazmak mümkün değildir; kayıt sessizce değiştirilmez, hata verilir. `append` onayını bekleyen görev gönderimden sonra iptal edilirse kayıt yazılmış olabilir; bu belirsiz durumda çağıran kaydı yeniden denememeli ve yeni yan etki başlatmamalıdır. Yazıcı kayıt işlemini başlattıktan sonra onayı iletemezse kalıcı hata durumuna geçer ve sonraki işlemleri reddeder. Eşzamanlı üreticiler, ebeveyn kaydın onayını almadan ona bağlı kayıt göndermemelidir.

Windows'ta kalıtılan DACL, dosya ve oturum dizininin açık tanıtıcıları üzerinden doğrulanır; yalnız geçerli süreç kullanıcısı, SYSTEM ve Administrators SID'lerine izin veren ve doğrulanabilir ACE'ler kabul edilir. Güvenli olabilecek fakat değerlendirilemeyen ACE türleri de reddedilir. Üst yolun güvenilirliği, sonradan ACL değişiklikleri ve ayrıcalıklı yönetici erişimi bu kontrolün dışındadır.

## İleri döngüler

- `ferrocore-loop-reflexion`: yapılandırılmış doğrulama komutu sıfır çıkış kodu vermeden tamamlanmayı engelleyen test kapısı; hata çıktısını modele geri besleme.
- `ferrocore-loop-plan`: salt okunur Architect aşamasıyla görev DAG'ı ve dosya bazlı değişiklik planı; Editor aşamasında RLM/ReAct ile uygulama.
- `ferrocore-loop-mcts`: ayrı Git worktree'lerinde hipotezler, paralel alt ajanlar, test/benchmark skoru ve kazanan çözüm seçimi. Birleştirme ve silme kullanıcı onayına tabidir.
- `ferrocore-loop-fsm`: yapılandırmadan tanımlanan sonlu durum makinesi; örnek akış diff, analiz, rapor ve onaylı PR yorumu.
- `ferrocore-loop-ambient`: dosya sistemi, Git kancası veya log olaylarıyla tetiklenen arka plan analizleri ve yama önerileri.

İlham kaynakları, alternatif proje isimleri ve kaynak belgedeki tüm karşılaştırmalar [özgün ADR kopyasında](reference/architecture-decisions-2026-10-31.md) korunur. Bunlara ait dış proje, maliyet ve doğruluk iddiaları burada doğrulanmış sonuçlar değildir.
