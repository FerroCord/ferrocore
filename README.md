# ferrocore

ferrocore, [ferrocord](https://github.com/FerroCord/ferrocord) mikro-çekirdeği üzerine kurulması planlanan otonom yazılım geliştirme ajanıdır. Hedef, ana ürünün Node.js veya Python çalışma zamanı gerektirmeyen tek bir Rust ikilisi olarak dağıtılmasıdır. Kullanıcının çalıştırdığı araçlar ve harici eklentiler kendi bağımlılıklarına sahip olabilir.

## Durum

31 Ekim 2026 itibarıyla depo oluşturuldu ve yerel olarak klonlandı. Bu depo mimari ve uygulama planını içerir. Rust kaynakları, Cargo manifestleri, çalışan CLI, testler ve CI henüz oluşturulmadı. Kaynak belgede kabul edilen kararlar uygulama hedefidir; tamamlanan özellikler olarak okunmamalıdır.

- Ana depo: <https://github.com/FerroCord/ferrocore>
- Bağımsız çekirdek: <https://github.com/FerroCord/ferrocord>
- Organizasyon: <https://github.com/FerroCord>
- Kaynak ilerleme kaydında iki deponun başlangıç görünürlüğü `PRIVATE` olarak doğrulanmıştır. Görünürlük değişikliği ayrıca onay gerektirir.

## Mimari özeti

- `ferrocore-types`: `AgentLoop`, `Tool`, `Session` sözleşmeleri ve veri modelleri.
- `ferrocore-driver`: ortak LLM (büyük dil modeli) akışı, yeniden deneme, bütçe, iptal ve oturum kaydı.
- `ferrocore-runtime-pty`: kalıcı terminal, komut bitiş çerçevesi ve alt ajan iletişimi.
- `ferrocore-loop-rlm`: ham kod blokları ve canlı terminal bağlamıyla çalışan özyinelemeli döngü.
- `ferrocore-loop-react`: yapılandırılmış araç çağrılarıyla çalışan kardeş döngü.
- `ferrocore-tools`: MCP uyumlu süreçler arası iletişim ve WebAssembly adaptörleri.

Birinci parti modüller bu depoda Cargo workspace içinde geliştirilecek. Harici `ferrocore-plugin-*` depoları ihtiyaç oldukça ayrı açılacak.

## Belgeler

- [Ana mimari ve karar haritası](docs/architecture.md)
- [Fazlar, yapılacaklar ve kabul ölçütleri](docs/roadmap.md)
- [Açık kararlar ve kaynak sınırları](docs/open-questions.md)
- [Ajan çalışma rehberi](AGENTS.md)
- [Özgün mimari karar kaydı](docs/reference/architecture-decisions-2026-10-31.md)
- [Özgün ilerleme kaydı](docs/reference/progress-2026-10-31.md)

`docs/reference/` dosyaları üst çalışma alanındaki kaynak belgelerin aynen korunmuş kopyalarıdır. Tarihsel ilerleme kaydındaki yerel dizin durumu, kaydın yazıldığı ana aittir. Özgün ADR; isim alternatiflerini, harici projelerle karşılaştırmaları ve görsel verilerini de içerir. Harici iddialar ve performans beklentileri bu depoda doğrulanmış değildir. Eksik görseller ve protokol sözleşmeleri [açık kararlarda](docs/open-questions.md) izlenir.

## Geliştirmeye başlama

İlk bağımlılık ferrocord Faz 1 sözleşmeleridir. Sonraki adım, açık protokol kararlarını netleştirip Faz 2 için gereken workspace ve modülleri oluşturmaktır. Yalnızca aktif fazın gerektirdiği modüller eklenmelidir.

Manifestler ve araç zinciri belirlendiğinde doğrulama adayları:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Bu komutlar henüz bu depoda çalıştırılabilir bir proje üzerinde doğrulanmadı. Commit, push, yayınlama ve dağıtım bu dokümantasyon hazırlığının kapsamı dışındadır.
