# ferrocore

ferrocore, [ferrocord](https://github.com/FerroCord/ferrocord) mikro-çekirdeği üzerine kurulu bir otonom yazılım geliştirme ajanıdır. Ana ürün için Node.js veya Python çalışma zamanı gerekmez; görev yürütmek için ayrıca platform kabuğu ve görevin kullandığı kullanıcı araçları gerekir.

## Durum

Workspace, ferrocord tanılaması ve LLM, oturum kaydı, kalıcı PTY ve RLM üzerinden görev yürütmesi içerir. Bağımlılık `ae07423923b0934b0235d00fcd97c2bffeadd9ee` Git commit'ine sabitlenmiştir. Linux ve Windows hedeflenir; gerçek sağlayıcı/model uyumu ve her iki platformun tüm kabul koşulları ayrıca doğrulanmalıdır. Alt ajanlar, ReAct ve dış eklenti adaptörleri henüz uygulanmadı.

- Ana depo: <https://github.com/FerroCord/ferrocore>
- Bağımsız çekirdek: <https://github.com/FerroCord/ferrocord>
- Organizasyon: <https://github.com/FerroCord>
- Kaynak ilerleme kaydında iki deponun başlangıç görünürlüğü `PRIVATE` olarak doğrulanmıştır. Görünürlük değişikliği ayrıca onay gerektirir.

## Mimari özeti ve hedefler

- `ferrocore-types`: `AgentLoop`, `Tool`, `Session` sözleşmeleri ve veri modelleri.
- `ferrocore-driver`: ortak LLM (büyük dil modeli) akışı, yeniden deneme, bütçe, iptal ve oturum kaydı.
- `ferrocore-runtime-pty`: kalıcı terminal, komut bitiş çerçevesi ve alt ajan iletişimi.
- `ferrocore-loop-rlm`: ham kod blokları ve canlı terminal bağlamıyla çalışan özyinelemeli döngü.
- `ferrocore-loop-react`: yapılandırılmış araç çağrılarıyla çalışan kardeş döngü.
- `ferrocore-tools`: MCP uyumlu süreçler arası iletişim ve WebAssembly adaptörleri.

Listede uygulanan modüllerin yanında planlanan modüller de vardır. Birinci parti modüller Cargo workspace içinde; harici `ferrocore-plugin-*` depoları ihtiyaç oldukça ayrı planlanır.

## Belgeler

- [Ana mimari ve karar haritası](docs/architecture.md)
- [Fazlar, yapılacaklar ve kabul ölçütleri](docs/roadmap.md)
- [Açık kararlar ve kaynak sınırları](docs/open-questions.md)
- [Ajan çalışma rehberi](AGENTS.md)
- [Özgün mimari karar kaydı](docs/reference/architecture-decisions-2026-10-31.md)
- [Özgün ilerleme kaydı](docs/reference/progress-2026-10-31.md)

`docs/reference/` dosyaları üst çalışma alanındaki kaynak belgelerin aynen korunmuş kopyalarıdır. Tarihsel ilerleme kaydındaki yerel dizin durumu, kaydın yazıldığı ana aittir. Özgün ADR; isim alternatiflerini, harici projelerle karşılaştırmaları ve görsel verilerini de içerir. Harici iddialar ve performans beklentileri bu depoda doğrulanmış değildir. Eksik görseller ve protokol sözleşmeleri [açık kararlarda](docs/open-questions.md) izlenir.

## Geliştirmeye başlama

Rust stable ile derleme için Cargo ve bağımlılıkların erişilebilir olması gerekir. `run`, Linux'ta PATH üzerinde `bash`, Windows'ta PATH üzerinde PowerShell 7 (`pwsh.exe`) gerektirir; modelin çalıştırdığı komutların araçları ayrıca kurulmalıdır. `check` yalnız ferrocord Context/EventBus entegrasyonunu sınar; sağlayıcı, kabuk veya görev yürütmesini sınamaz:

```sh
cargo run -- check
```

Başarılı sonuç `ferrocord integration: ok` satırıdır. Görev çalıştırmak için `config.example.toml` dosyasını kopyalayıp `[llm]` içindeki `base_url` ve `model` değerlerini sağlayıcınıza göre düzenleyin. `api_key_env` değerinin adlandırdığı ortam değişkenine anahtarı koyun; anahtarı yapılandırma dosyasına yazmayın. Yalnız `active_loop = "rlm"` desteklenir. Bütçe, PTY ve oturum günlüğü ayarları örnek dosyadadır. Örnek URL gerçek bir sağlayıcı değildir.

Örnek yapılandırmadaki `./sessions` dizinini çalıştırmadan önce oluşturun (`mkdir sessions`). Windows'ta bu dizinin kalıtılan erişim izinleri yalnız kullanıcı, SYSTEM ve Administrators için güvenli biçimde doğrulanabilmelidir; doğrulanamayan ACL oturumu başlatmaz.

```sh
cargo run -- run --config config.toml "Görev açıklaması"
```

Bu komut modelin ürettiği kabuk komutlarını çalıştırabilir; yalnız güvendiğiniz ortamda kullanın. `--` seçenek sonlandırıcısıyla tireyle başlayan görev veya dosya adı verilebilir. Tam komut çıktısı `$FC_SCRATCH/last_output.log` içinde ham ve redakte edilmeden saklanır; gizli veriler içerebilir. Süreç ağacının durduğu kanıtlanamazsa scratch korunabilir; erişimini ve temizliğini ayrıca yönetin. JSONL kaydında bilinen API anahtarının tam eşleşmelerinin redakte edilmesi diğer gizli verilerin korunacağını garanti etmez.

Yerel doğrulama komutları:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commit, push, yayınlama ve dağıtım bu çalışmanın kapsamı dışındadır.
