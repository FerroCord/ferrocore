# Yol haritası ve yapılacaklar

Kaynak: [ADR §5 ve döngü tablosu](reference/architecture-decisions-2026-10-31.md), [tarihsel ilerleme kaydı](reference/progress-2026-10-31.md). İşaretli maddeler mevcut belge ve yerel gözlemlere dayanır. Diğer maddeler uygulama hedefidir. Kabul ölçütleri, ADR kararlarını sınamak için önerilen doğrulama görevleridir.

## Hazırlık

- [x] `FerroCord` organizasyonu ve iki ana depo oluşturuldu (kaynak ilerleme kaydı).
- [x] `ferrocord` ve `ferrocore` yerel olarak klonlandı.
- [x] Mimari kaynakları ve yapılacakları depo içinde belgele.
- [ ] [Açık kararları](open-questions.md) ilgili uygulama başlamadan netleştir.
- [ ] Rust araç zinciri, asgari Rust sürümü ve desteklenecek platformları belirle.
- [ ] ferrocord için yayımlanmış sürüm ve yerel geliştirme yöntemini belirle; entegrasyon şimdilik `ae07423923b0934b0235d00fcd97c2bffeadd9ee` commit'ine sabitlendi.

## Faz 1 — ferrocord v0.1.0 bağımlılığı

Bu faz [ferrocord deposunda](https://github.com/FerroCord/ferrocord) yürütülür.

- [ ] Senkron kilit ve `Arc` tabanlı Context, tip tabanlı EventBus ve Plugin sözleşmesini tamamla.
- [ ] API ve birim testlerini kararlı hale getir.
- [x] ferrocore entegrasyonu için `ae07423923b0934b0235d00fcd97c2bffeadd9ee` commit'ini seç.

**Geçiş ölçütü:** Servis erişimi, olay tipi izolasyonu ve yaşam döngüsü testleri geçer; `.await` boyunca senkron kilit tutulmaz. `v0.1.0` kaynakta faz hedefidir; yayınlama ayrıca kullanıcı onayı gerektirir.

## Faz 2 — ferrocore MVP: driver, PTY ve RLM

Tanılama workspace'i ve `cargo run -- check` ile gerçek Context/EventBus entegrasyonu hazırdır. Windows üzerinde çalıştırıldı; Linux hedefi henüz doğrulanmadı. Bu adım Faz 2 MVP kabul ölçütlerini karşılamaz.

- [ ] Yalnızca gereken crate'lerle Cargo workspace ve ana CLI iskeletini oluştur.
- [ ] `ferrocore-types` içinde AgentLoop, Tool ve Session veri sözleşmelerini tanımla.
- [ ] `ferrocore-driver`: LLM akışı, retry/backoff, bütçe ve maliyet sınırları.
- [ ] Ortak Ctrl+C/zaman aşımı iptali ve JSONL kayıt akışını oluştur.
- [ ] `ferrocore-runtime-pty`: kalıcı kabuk ve netleşmiş sentinel ayrıştırıcısı.
- [ ] `$FC_SCRATCH`, tam çıktı kaydı ve 10 KB / 150 satır bağlam budamasını uygula.
- [ ] `ferrocore-loop-rlm`: ham kod ayrıştırma, canlı terminal bağlamı ve yürütme.
- [ ] `config.example.toml` ve `active_loop` seçimini belgele.
- [ ] Test ve ikili derleme CI hattını ekle; dağıtım bağımlılıklarını kontrol et.

**Kabul ölçütleri:** Ardışık komutlarda kabuk durumu korunur; başarısız komutun çıkış kodu doğru alınır. Sentinel her byte sınırında parçalanmış girişle sınanır. İptal, zaman aşımı, kabuk kapanışı, büyük çıktı, bozuk model yanıtı, LLM akış hatası ve bütçe tükenmesi test edilir. JSONL alanları ve ebeveyn ilişkileri doğrulanır. Çalışan bir RLM görevi uçtan uca gösterilir.

## Faz 3 — özyinelemeli alt ajan altyapısı

- [ ] UDS sunucusu ve alt ajan istek/yanıt sözleşmesini tanımla.
- [ ] Aynı ikilide `internal-spawn` ve POSIX `spawn-agent` girişini uygula.
- [ ] Aktif yürütme izinleri, derinlik bazlı kotalar ve iptal yayılımını uygula.
- [ ] Alt ajan beklerken ebeveynin iznini bıraktığını doğrula.
- [ ] Ana/alt ajan JSONL kayıtlarını tek oturum ağacına bağla.
- [ ] Döngü türü seçimini spawn protokolüne taşı.

**Kabul ölçütleri:** Düşük kota altında çok seviyeli spawn kilitlenmeden tamamlanır. Hata, bağlantı kopması, iptal, yetim süreç ve kota aşımı test edilir. RLM alt ajan yürütmesi doğrulanır. ReAct ile çapraz döngü testi Faz 4'e bağlıdır.

## Faz 4 — kardeş döngüler ve dış eklentiler

- [ ] `ferrocore-loop-react`: yapılandırılmış araç çağrıları ve araç kayıt sistemi.
- [ ] `ferrocore-tools`: MCP uyumlu JSON-RPC IPC adaptörü.
- [ ] Seçilen WASM motoruyla ortak Tool adaptörü ve izin modeli.
- [ ] RLM -> ReAct ve ReAct -> RLM alt görev akışları.
- [ ] Önerilen `ferrocore-loop-reflexion`: zorunlu doğrulama kapısı ve hata geri beslemesi.

**Kabul ölçütleri:** İki döngü ortak bütçe, retry, iptal ve kayıt mekanizmalarını paylaşır. Araç hatası, bozuk protokol, paralel çağrı ve iptal testleri geçer. Reflexion eklenirse başarısız doğrulama komutu tamamlanmayı engeller; tekrar sınırı ayrıca belirlenir.

## Faz 5 ve sonrası — ileri orkestrasyon

- [ ] Faz 5: `ferrocore-loop-plan`, salt okunur planlama ve Editor devri.
- [ ] Faz 5 / 5+: `ferrocore-loop-mcts`, izole Git worktree'leri, test/benchmark değerlendirmesi ve onaylı birleştirme/temizlik.
- [ ] Faz 5+: `ferrocore-loop-fsm`, durum geçişleri ve yan etkili adımlarda onay.
- [ ] İleri faz, numarası belirtilmemiş: `ferrocore-loop-ambient`, dosya/Git/log tetikleyicileri ve yama önerileri.

Kaynakta MCTS, faz tablosunda 5+, uygulama fazları bölümünde Faz 5 olarak geçer. Ayrıntılı takvim uygulama öncesinde netleştirilecektir. FSM ve ambient için ayrıntılı teslim sırası henüz belirlenmedi.

## Her uygulama adımında

- [ ] İlgili sözleşmeyi ve başarısızlık durumlarını belgeleyip test ekle.
- [ ] Gerçek manifestlere uygun `fmt`, `clippy` ve test kontrollerini çalıştır.
- [ ] Ayrı alt ajanla değişiklik incelemesi yap.
- [ ] Ölçülmüş darboğazları, kaynak sınırlarını ve doğrulanmamış noktaları raporla.
- [ ] Yayınlama, push, birleştirme ve silme gibi yan etkili işlemler için gerekli kullanıcı onayını al.

Mevcut test paketi gerçek CLI süreçlerini sınar; diğer fazların kabul ölçütleri henüz uygulanmadı.
