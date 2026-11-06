# ferrocore

ferrocore, [ferrocord](https://github.com/FerroCord/ferrocord) mikro-çekirdeği üzerine kurulması planlanan otonom yazılım geliştirme ajanıdır. Hedef, ana ürünün Node.js veya Python çalışma zamanı gerektirmeyen tek bir Rust ikilisi olarak dağıtılmasıdır. Kullanıcının çalıştırdığı araçlar ve harici eklentiler kendi bağımlılıklarına sahip olabilir.

## Durum

Rust 2021 Cargo workspace ve gerçek ferrocord servis/olay yolunu sınayan küçük bir tanılama CLI'sı vardır. Bağımlılık `ae07423923b0934b0235d00fcd97c2bffeadd9ee` Git commit'ine sabitlenmiştir. Linux ve Windows hedeflenir; tanılama ve süreç testleri Windows üzerinde çalıştırılmıştır, Linux üzerinde henüz doğrulanmamıştır. Faz 2 MVP tamamlanmadı; LLM, PTY, sentinel ve RLM uygulanmadı. CI henüz yoktur.

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

Tanılama komutu:

```sh
cargo run -- check
```

Başarılı sonuç `ferrocord integration: ok` satırıdır. Doğrulama komutları:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commit, push, yayınlama ve dağıtım bu çalışmanın kapsamı dışındadır.
