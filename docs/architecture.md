# ferrocore mimarisi

## Kaynak ve karar durumu

Kaynak: [31 Ekim 2026 ADR](reference/architecture-decisions-2026-10-31.md). Bu belge kabul edilen hedef mimariyi uygulanabilir başlıklara ayırır. Mevcut Rust workspace yalnızca `ferrocore` ikilisini içerir; `ferrocord` bağımlılığı `ae07423923b0934b0235d00fcd97c2bffeadd9ee` commit'ine sabitlenmiştir. `cargo run -- check`, Context ve EventBus entegrasyonunu sınar. Linux ve Windows hedeflenir; Windows üzerinde çalıştırıldı, Linux doğrulanmadı. Faz 2 MVP tamamlanmadı. Aşağıdaki dosya ağacı ve ajan sözleşmeleri hedef tasarımdır.

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
├── .github/workflows/ci.yml       # hedef: test ve ikili derleme
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

ADR-004 ve ADR-008 uyarınca `ferrocore-types`, ortak `AgentLoop`, `Tool` ve `Session` sözleşmelerini barındırır. Aktif döngü `config.toml` içindeki `active_loop` anahtarıyla seçilir. Alt ajanlara görev türüne göre farklı döngüler atanabilir. Config şeması ve doğrulama davranışı henüz tanımlanmadı.

`ferrocore-driver` içindeki ortak `TurnDriver` sorumlulukları:

- LLM yanıt akışı ve üstel geri çekilmeli yeniden deneme.
- Token bütçesi ve maliyet sayacı.
- Append-only JSONL oturum kaydı.
- Ctrl+C, zaman aşımı ve iptal yönetimi.

RLM ve ReAct aynı sürücü ilkellerini kullanan kardeş eklentilerdir. Her döngü kendi girdi ayrıştırıcısını ve yürütme davranışını taşır. Böylece ortak hata, bütçe ve kayıt davranışı tek katmanda paylaşılır.

| Döngü | Özel davranış |
| --- | --- |
| RLM | Ham kabuk kod bloklarını ayrıştırır; PTY akışını ve spawn iletişimini izler; çalışma dizini, son çıkış kodu, arka plan işleri ve scratch içeriğini bağlama ekler. |
| ReAct | JSON Schema tabanlı araç çağrılarını oluşturur; araç kayıt sistemi üzerinden MCP/WASM çağrılarını yürütür; paralel araç çağrılarını yönetir. |

Ham kod biçiminin kesin sözleşmesi açık karardır. ADR'deki `bash` blokları ve `<sh>...</sh>` örnek biçimlerdir. RLM adı kaynakta Prime Agent tarzı özyinelemeli terminal döngüsü için kullanılır; bu depoda algoritma uygulaması henüz yoktur.

## Kalıcı terminal ve çıktı (ADR-002, ADR-003)

`ferrocore-runtime-pty`, `portable-pty` ile kalıcı kabuk yönetimini hedefler. Kabuk, komutlar arasında çalışma dizinini ve oturum durumunu korur. Her komut sonuna eklenecek sentinel çerçevesi komutun bittiğini ve çıkış kodunu bildirir. Akış ayrıştırıcısı çerçeveyi parçalı byte akışından çıkarır. **Sentinel'in uygulanabilir metinsel biçimi kaynakta tanımlı değildir; Faz 2'yi engelleyen diğer açık sözleşmelerle birlikte [Faz 2 sözleşme önerileri taslağında](phase2-contract-proposals.md) onay bekleyen öneri olarak ele alınmıştır.**

Geçici çalışma alanı `$FC_SCRATCH` ile kabuğa aktarılır:

| Platform | ADR hedef yolu |
| --- | --- |
| Linux | `/dev/shm/ferrocore_{id}` |
| macOS ve diğer POSIX | `/tmp/ferrocore_{id}` |
| Windows | Kabuk, geçici dizin ve IPC sözleşmesi açık karar |

Çıktı 10 KB veya 150 satır eşiğini aştığında model bağlamına ilk 50 ve son 50 satır aktarılır. Tam çıktı `$FC_SCRATCH/last_output.log` içinde korunur. Bayt ölçümü, satır örtüşmesi, sınırsız tek satır ve paralel komutların dosya sahipliği henüz netleştirilmedi.

## Alt ajanlar ve eşzamanlılık (ADR-003, ADR-008)

Ana ikili `ferrocore internal-spawn` alt komutunu da çalıştırır. POSIX tasarımında `spawn-agent`, PATH üzerindeki sembolik bağ veya kısa kabuk sarmalayıcısıyla aynı ikiliye yönlendirilir. Alt ajan istekleri Unix domain socket (UDS) üzerinden ana süreçle iletişim kuracak şekilde planlanır. Windows karşılığı açık karardır.

İzinler yalnızca aktif LLM/komut yürütmesi boyunca tutulur. Ebeveyn alt ajanı beklerken iznini bırakır. Her derinlik seviyesi bağımsız eşzamanlılık kotası kullanır. Sayısal kota, azami derinlik ve aşım davranışı henüz tanımlanmadı.

Ortak `AgentLoop`, çapraz döngü görevlendirmesine olanak verir. Kaynaktaki hedef kullanım örneği `spawn-agent --loop react "görev"` biçimindedir. Mevcut CLI yalnız tanılama yapar; spawn uygulanmadı. Faz 3'te spawn altyapısı hazırlanır; RLM/ReAct birlikte çalışma doğrulaması Faz 4'te ReAct tamamlanınca yapılır.

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

Alan tipleri, kök kaydın ebeveyni, kimlik üretimi, sürümleme ve eşzamanlı yazma sözleşmesi henüz tanımlanmadı. Bunlar veri modeli netleştirilirken kararlaştırılmalıdır.

## İleri döngüler

- `ferrocore-loop-reflexion`: yapılandırılmış doğrulama komutu sıfır çıkış kodu vermeden tamamlanmayı engelleyen test kapısı; hata çıktısını modele geri besleme.
- `ferrocore-loop-plan`: salt okunur Architect aşamasıyla görev DAG'ı ve dosya bazlı değişiklik planı; Editor aşamasında RLM/ReAct ile uygulama.
- `ferrocore-loop-mcts`: ayrı Git worktree'lerinde hipotezler, paralel alt ajanlar, test/benchmark skoru ve kazanan çözüm seçimi. Birleştirme ve silme kullanıcı onayına tabidir.
- `ferrocore-loop-fsm`: yapılandırmadan tanımlanan sonlu durum makinesi; örnek akış diff, analiz, rapor ve onaylı PR yorumu.
- `ferrocore-loop-ambient`: dosya sistemi, Git kancası veya log olaylarıyla tetiklenen arka plan analizleri ve yama önerileri.

İlham kaynakları, alternatif proje isimleri ve kaynak belgedeki tüm karşılaştırmalar [özgün ADR kopyasında](reference/architecture-decisions-2026-10-31.md) korunur. Bunlara ait dış proje, maliyet ve doğruluk iddiaları burada doğrulanmış sonuçlar değildir.
