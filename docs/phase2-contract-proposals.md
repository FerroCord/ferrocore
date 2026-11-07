# Faz 2 MVP Sözleşme Önerileri (Karar Taslağı)

## Belge statüsü ve kapsam

Bu belge, `ferrocore` Faz 2 MVP uygulamasını engelleyen açık protokol ve veri sözleşmelerini değerlendirilebilir hale getiren **onay bekleyen karar taslağıdır**.

- **Karar statüsü:** Buradaki hiçbir öneri kabul edilmiş mimari karar, kesinleşmiş API veya uygulanmış özellik değildir. Uygulamaya geçmeden önce her başlık için kullanıcı onayı gerekir.
- **Mevcut durum:** Depodaki mevcut Rust ikilisi (`cargo run -- check`) yalnızca `ferrocord` `Context` ve `EventBus` entegrasyonunu doğrular; PTY, sentinel ayrıştırıcısı, LLM istemcisi ve RLM döngüsü henüz uygulanmamıştır.
- **Mimari sınırlar:** [31 Ekim 2026 ADR](reference/architecture-decisions-2026-10-31.md) kararları ve `ferrocord` çekirdeğinin genel amaçlı sınırı (AI, LLM, prompt ve terminal kavramlarını içermemesi) aynen korunur.
- **Kapsam sınırı:** Yalnızca Faz 2 MVP (`ferrocore-types`, `ferrocore-driver`, `ferrocore-runtime-pty`, `ferrocore-loop-rlm`) için gereken sözleşmeler ele alınır. Faz 3 (`internal-spawn` / UDS protokolü ve derinlik kotaları), Faz 4 (`ferrocore-loop-react`, MCP ve WASM adaptörleri) ve Faz 5+ döngüleri için yeni sözleşme icat edilmez; yalnızca Faz 2 veri sınırlarının ileri fazları kilitlememesi gözetilir.

İlgili kaynaklar: [Mimari](architecture.md) · [Yol haritası](roadmap.md) · [Açık kararlar](open-questions.md) · [Özgün ADR](reference/architecture-decisions-2026-10-31.md)

---

## (a) Kabuk, PTY ve `$FC_SCRATCH` yaşam döngüsü (Linux ve Windows)

### Kaynakta kesin olan

- Kalıcı kabuk yönetimi için `portable-pty` seçilmiştir (ADR-002).
- Geçici çalışma alanı kabuğa `$FC_SCRATCH` ortam değişkeniyle aktarılır; hedef yol Linux için `/dev/shm/ferrocore_{id}`, macOS ve diğer POSIX sistemler için `/tmp/ferrocore_{id}` olarak tanımlanmıştır (ADR-003).
- Linux ve Windows hedef platformlardır. Mevcut tanılama CLI'sı Windows üzerinde çalıştırılmış, Linux üzerinde henüz doğrulanmamıştır. Windows kabuk seçimi, geçici dizin yolu ve IPC davranışı kaynakta açık bırakılmıştır.

### Öneri ve gerekçe

1. **Kabuk seçimi ve başlatma bayrakları:**
   - **Linux:** `bash --noprofile --norc` adaydır; `bash` kurulu olmalıdır. Profil atlamak başlangıç etkilerini azaltır, ancak tek başına deterministik terminal davranışı garanti etmez.
   - **Windows:** `portable-pty` ve ConPTY üzerinde `pwsh.exe -NoLogo -NoProfile` adaydır; PowerShell ayrıca kurulmalıdır. Etkileşimli PTY, kodlama, VT dizileri ve `$LASTEXITCODE`/`$?` davranışı gerçek platformda sınanmadan sarmalayıcı kesinleştirilemez.
2. **Başarısız başlatma davranışı:**
   - Kabuk ikilisi bulunamazsa, PTY çifti açılamazsa veya kabuk açıldıktan sonra yapılandırılan başlangıç el sıkışma süresinde (ör. `5 saniye`) ilk doğrulama sentinel'i dönmezse `PtyInitError` üretilir.
   - Başlatma hatasında sessizce farklı semantikli bir kabuğa (ör. `bash` yerine `sh` veya `pwsh` yerine `cmd.exe`) otomatik geçiş yapılmaz; oluşturulmuş `$FC_SCRATCH` dizini ve yarım kalan çocuk süreç hemen temizlenir, LLM oturumu başlatılmaz.
3. **`$FC_SCRATCH` oluşturma, geri dönüş ve temizlik yaşam döngüsü:**
   - `{id}` için 128 bit rastgele kimlik önerilir; çakışma olasılığı sıfır değildir, dizin münhasır oluşturulur ve çakışmada yeni kimlik denenir.
   - **Linux:** Önce `/dev/shm` dizininin varlığı ve yazılabilirliği kontrol edilir; `/dev/shm/ferrocore_{id}` dizini yalnızca süreç sahibine açık izinle (`0700`) oluşturulur. `/dev/shm` yoksa, izin reddedilirse (`EACCES`) veya bellek diski doluysa (`ENOSPC`), oturumu düşürmek yerine `/tmp/ferrocore_{id}` (`0700`) dizinine geri dönülür ve yapılandırılmış bir uyarı olayı kaydedilir.
   - **Windows:** `std::env::temp_dir().join(format!("ferrocore_{id}"))` (tipik olarak `%LOCALAPPDATA%\Temp\ferrocore_{id}`) kullanılır.
   - **Temizlik:** Normal bitişte, iptalde ve başlatma hatasında yalnız oturuma ait dizin temizlenmeye çalışılır; izin veya açık dosya nedeniyle başarısızlık bildirilir. Zorla sonlandırmada dizin kalabilir ve sonraki oturumda sessiz toplu silinmez.

### Önemli bedel

Profil dosyalarının atlanması başlangıç etkilerini azaltabilir, ama kullanıcının profilinde yaptığı PATH değişiklikleri görünmeyebilir. Üst süreç PATH'ini aktarmak önerilir; bu tek başına çerçeveleme garantisi değildir.

### Örnek girdi / çıktı

```text
Girdi (Linux oturum başlatma, id = "8f2a...c901", /dev/shm yazılabilir):
  spawn: bash --noprofile --norc
  env:   FC_SCRATCH=/dev/shm/ferrocore_8f2a...c901

Başlangıç el sıkışması (çekirdek -> PTY):
  printf '\036__FC_READY__:8f2a...c901__\037\n'

Beklenen sonuç:
  1. /dev/shm/ferrocore_8f2a...c901 (0700) mevcuttur.
  2. El sıkışma çerçevesi 5 sn içinde okunur ve ayrıştırıcı tarafından tüketilir (girdi yankısı terminal ayarına bağlıdır; platformda doğrulanmalıdır).
  3. Oturum kapanışında dizin kaldırılır.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| Konteyner ortamında `/dev/shm` yok veya salt okunur | `/tmp/ferrocore_{id}` (`0700`) dizinine düşülür; oturum kaydına `scratch_fallback` uyarısı yazılır. |
| Hedef kabuk ikilisi (`bash` veya `pwsh`) sistemde kurulu değil | Otomatik kabuk değiştirme yapılmaz; scratch dizini silinir ve açık `PtyInitError::ShellNotFound` döner. |
| Kabuk açıldı ancak el sıkışma çerçevesi zaman aşımına uğradı | Çocuk süreç sonlandırılır, scratch silinir, LLM çağrısı yapılmadan hata döner. |
| Oturum `SIGKILL` ile çöktü | `ferrocore_{id}` dizini inceleme/kurtarma için diskte kalır; sonraki normal oturum başka `{id}` alır ve eski dizinleri sessizce silmez. |
| Windows ConPTY altında PowerShell çıkış kodu ve VT dizileri | **Açık kanıt ihtiyacı:** Uygulama öncesi `portable-pty` ile gerçek ConPTY duman testi çalıştırılarak `$LASTEXITCODE`/`$?` sarmalayıcısı doğrulanmalıdır. |

### Uygulama öncesi kullanıcı kararı

- Linux'ta `bash` zorunluluğu (otomatik `/bin/sh` geri dönüşü olmaması) ve Windows'ta `pwsh` hedefi kabul ediliyor mu?
- `/dev/shm` kullanılamadığında `/tmp` geri dönüşü yapılması mı, yoksa yapılandırmaya göre sert hata verilmesi mi tercih ediliyor?

---

## (b) Sentinel çerçeveleme protokolü ve güven sınırı

### Kaynakta kesin olan

- Kalıcı kabuk her komutta kapanmadığı (`EOF` üretmediği) için her komutun sonuna çekirdek tarafından görünmez bir çerçeveleme komutu eklenir; `Stream Interceptor` bu diziyi yakaladığında komutun tamamlandığını ve kesin `exit_code` değerini anlar (ADR-002).
- **ADR `image1`:** Kaynakta gömülü PNG verisi vardır; bu çalışma için görselin içeriği güvenilir biçimde okunamadığından byte biçimi hakkında çıkarım yapılmaz. Aşağıdaki çerçeve kaynak alıntısı değil, onay bekleyen öneridir.

### Öneri ve gerekçe

1. **Byte çerçevesi, oturum gizli değeri ve komut kimliği:**
   - Her PTY oturumu başlarken 128-bit kriptografik rastgele `session_nonce` (`32` küçük harf hex karakter) üretilir.
   - Her komuta oturum içinde kesin artan `cmd_id: u64` (`1, 2, 3, ...`) verilir.
   - Ham sentinel byte çerçevesi ASCII kontrol karakterleri `0x1E` (`RS`, Record Separator) ve `0x1F` (`US`, Unit Separator) ile sınırlandırılır:
     ```text
     \x1e__FC_END__:<session_nonce>:<cmd_id>:<exit_code>__\x1f\n
     ```
   - `session_nonce` kabuk ortam değişkenine (`export`) **yazılmaz**; yalnızca çekirdeğin gönderdiği sarmalayıcı satırda anlık sabit olarak yer alır.
2. **Komut sarmalama (POSIX `bash`):**
   - Yorumla biten tamamlanmış bir komuttan sonra `\n` eklemek sarmalayıcının yorum içinde kalmasını önleyebilir; kapanmamış tırnak, heredoc veya çok satırlı sözdizimi için yeterli değildir. Sözdizimi ve etkileşimli kabuk davranışı platformda doğrulanmalıdır.
   - `printf` için sekizli kaçışlar kullanmak, girdi yankısında kontrol baytlarının doğrudan bulunmasını önlemeyi hedefler; terminal dönüşümleri ve yankı davranışı ayrıca doğrulanmalıdır:
     ```sh
     <kullanıcı_komutu>
     __fc_ec=$?; printf '\036__FC_END__:%s:%s:%d__\037\n' '<session_nonce>' '<cmd_id>' "$__fc_ec"
     ```
3. **Parçalı byte akışı:** Önerilen ayrıştırıcı her okuma sınırında aday öneki tutar; `128` baytı aşan veya geçersiz adayları sıradan çıktıya geri verir. İç içe başlayan adayları kaçırmamak için geri verilen baytlarda olası sonraki `0x1E` yeniden değerlendirilir. Tam eşleşme (`\n` veya `\r\n`) yalnız aktif kimlik ve geçerli çıkış kodunda bitiş adayı sayılır. EOF veya iptalde kısmi aday çıktı olarak korunur. Bu yaklaşımın terminal dönüşümlerinde doğrulanması gerekir.
4. **Kabuk kapanması ve zaman aşımı:**
   - Aktif komut sürerken PTY `EOF` döndürür veya çocuk kabuk süreci kapanırsa (`exit` komutu, `exec` hatası, çökme), bekleyen komut `PtyCommandOutcome::ShellExited { exit_status, partial_output }` olarak sonlanır; sürücü kabuğun kapandığını modele açıkça bildirir veya yapılandırmaya göre yeni kabuk başlatmadan hatayla durur.
5. **Güven sınırı (Trust Boundary):**
   - Sentinel sandbox veya kimlik doğrulama değildir; yanlış eşleşme olasılığını azaltma önerisidir. Nonce sarmalayıcıda göründüğünden aynı PTY'deki komut veya süreç tarafından öğrenilip geçerli çerçeve taklit edilebilir; böyle bir çerçeve gerçek tamamlanmadan ayırt edilemez. ADR'nin kesin çıkış kodu hedefi güvenilmeyen komutlar için bu yöntemle garanti edilemez.
   - Kabuk durumu bozulabilir (`set -e`, `trap`, `exit`) ve arka plan süreçleri yazabilir. Güvenilir izolasyon/ayrı kontrol kanalı gerekip gerekmediği için platform kanıtı ve kullanıcı kararı gerekir; işletim sistemi izolasyonu ayrıca değerlendirilmelidir.

### Önemli bedel

`set -e` sarmalayıcıya ulaşılmadan kabuğu kapatabilir. Alt kabuk kullanmak `cd` ve `export` değişikliklerini kaybettirir; öneri, kalıcı durumu koruyup erken çıkışı hata olarak bildirmektir. Kabuk kapanışı ve PTY EOF sıralaması platformda doğrulanmalıdır.

### Örnek girdi / çıktı

```text
Aktif oturum nonce: 0123456789abcdef0123456789abcdef
Aktif cmd_id:       7
Kullanıcı komutu:   ls olmayan_dosya # yorum

PTY'ye yazılan baytlar:
  ls olmayan_dosya # yorum\n
  __fc_ec=$?; printf '\036__FC_END__:%s:%s:%d__\037\n' '0123456789abcdef0123456789abcdef' '7' "$__fc_ec"\n

PTY'den 3 ayrı okumada (parçalı) gelen baytlar:
  Parça 1: b"ls: cannot access 'olmayan_dosya': No such file or directory\r\n\x1e__FC_"
  Parça 2: b"END__:0123456789abcdef0123456789abcdef:7:"
  Parça 3: b"2__\x1f\r\n"

Ayrıştırıcı çıktısı:
  stdout_bytes: b"ls: cannot access 'olmayan_dosya': No such file or directory\r\n"
  cmd_id:       7
  exit_code:    2
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| Sentinel çerçevesi 1'er baytlık okumalarla bölünür | Önerilen kayan tampon parçaları birleştirir; tam eşleşmede çerçeve çıktıdan ayrılır ve `exit_code` okunur (terminal dönüşümleriyle sınanmalıdır). |
| Komut çıktısı yanlış nonce ile `\x1e__FC_END__:sahte:7:0__\x1f\n` basar | `session_nonce` uyuşmadığı için bitiş sayılmaz, baytlar normal çıktıya aktarılır. |
| Aynı PTY'deki komut gerçek `session_nonce` ve `cmd_id` ile geçerli çerçeve basar | Bant içi (in-band) ayrıştırıcı bunu gerçek bitişten ayırt edemez; güvenilmeyen komutlar için açık sınır/kontrol kanalı kararı gerekir. |
| Bozuk alanlı çerçeve (`\x1e__FC_END__:<nonce>:7:abc__\x1f\n`) veya tek başına `\x1e` + 5 KB metin | Geçerli tam çerçeve oluşmadığında veya `128 bayt` sınırında tutulan baytlar sıradan çıktıya geri verilir; içteki sonraki `0x1E` kaçırılmaz. |
| Kabuk kapanırken (`EOF`) tamponda yarım sentinel adayı kalır | Kısmi aday baytları kaybolmadan `partial_output` sonuna eklenir ve `ShellExited` raporlanır. |
| Tamamlanmış komut `# yorum` ile biter | Araya yeni satır eklenmesi sarmalayıcıyı yorumdan ayırmayı hedefler; kabuk davranışı sınanmalıdır. |
| Kapanmamış tırnak veya heredoc | Sentinel gelmeyebilir; öneri zaman aşımında iptal ve belirsiz kabuk durumunu bildirmektir. Kurtarma yöntemi platform kanıtı ve kullanıcı kararı bekler. |
| `exit 42` veya `set -e; false` | Kabuk kapanırsa tamamlanmamış komut hata ve eldeki kısmi çıktı ile raporlanır; EOF davranışı sınanmalıdır. |

### Uygulama öncesi kullanıcı kararı

- `\x1e__FC_END__:<nonce>:<cmd_id>:<exit_code>__\x1f\n` çerçeve biçimi ve PTY girdi echosunun kapatılması (`ECHO` off) / korunması tercihi onaylanıyor mu?
- Zaman aşımı veya kapanmamış tırnak sonrası kabuk kurtarılamazsa oturumun hatayla durması mı, yoksa son bilinen `PWD` ile yeni bir PTY kabuğu açılması mı isteniyor?
- Güvenilmeyen komutların sentinel'i taklit edebilmesi karşısında ayrı kontrol kanalı veya açık güven varsayımı seçilecek mi? Kabuk ve PTY davranışına dair platform kanıtı gereklidir.

---

## (c) Çıktı budama (10 KB / 150 satır) ve tam çıktı sahipliği

### Kaynakta kesin olan

- PTY çıktısı `10 KB` veya `150 satır` eşiğini aştığında kesilir; tam çıktı `$FC_SCRATCH/last_output.log` dosyasına yazılır ve model bağlamına yalnızca ilk `50` + son `50` satır aktarılır (ADR-003 Madde 4).
- Bayt birimi (`10_000` vs `10_240`), uzun tek satır davranışı, UTF-8 karakter sınırı ve paralel/ardışık çıktı dosyası sahipliği kaynakta açık bırakılmıştır.

### Öneri ve gerekçe

1. **Önerilen eşik ve ölçümler:**
   - Bayt eşiği `10 KiB = 10_240 bayt` (sentinel ayıklandıktan sonraki ham bayt uzunluğu), satır eşiği `150 satır` olarak önerilir.
   - Budama koşulu: `raw_bytes.len() > 10_240 || line_count > 150`.
   - RLM adımlarında modelin her zaman aynı dosya yoluna başvurabilmesi için eşikler aşılmadığında da son tamamlanan komutun tam çıktısının `$FC_SCRATCH/last_output.log` olarak yayımlanması önerilir (yalnız budama olduğunda yazılması alternatifi kullanıcı kararına açıktır).
2. **Bağlam görünümü:** Satır ayıracı `LF`, sonlandırılmamış son parça bir satır sayılması adaydır; CRLF görünümde normalize edilebilir. Ham kayıt değiştirilmez. Geçersiz UTF-8 için görünümde kayıplı dönüşüm önerilir. Eşik aşıldığında ilk ve son 50 satır örtüşmeden sunulur. Uzun tek satırda 50+50 satır ile bağlam sınırı çatışır: karakter sınırında baş/son bayt dilimi önerisi ADR'deki tam satır şartını değiştirir; ADR yorumu veya değişikliği için kullanıcı kararı gereklidir, sessizce uygulanmaz.
3. **Tam çıktı sahipliği:** Tek PTY için komutları seri yürütmek ve tek okuyucu/yazıcı kullanmak önerilir. Alınan baytlar komut aralığına göre akış halinde geçici dosyaya yazılır; başarılı tamamlanma ve başarılı dosya yazımı sonrası `last_output.log` yayımlanır. Bu, eşzamanlı komutları desteklemez. Ortak PTY'de arka plan işi aktif komut sırasında yazarsa baytların gerçek üreticisi ayırt edilemez; kayıt zaman aralığına aittir, kaynak komuta kesin atıf değildir. Komutlar arası baytlar ayrıca tutulabilir; sonradan gelen baytlar daha önce yayımlanmış dosyayı değiştirmez.
4. **Yazma ve yayımlama hatası:** Disk dolması, izin veya I/O hatasında tam çıktı garantisi verilemez. Hata açıkça bildirilir, kısmi kayıt tam diye sunulmaz ve önceki başarılı `last_output.log` korunmaya çalışılır. Sınırsız çıktı kapasiteyi tüketebilir; isteğe bağlı durdurma politikası ancak çıktının eksik olduğunu açıkça bildirerek ve kullanıcı kararıyla uygulanabilir. Sabit boyutta sessiz kesme veya halka silme önerilmez.
5. **Platform atomikliği:** Geçici dosya ve değiştirme işleminin atomiklik ve üzerine yazma semantiği dosya sistemi ve platforma bağlıdır. Windows'ta `rename` veya kopyanın atomik olduğu varsayılmaz; uygun API ve dosya sistemi davranışı için gerçek Windows kanıtı gerekir. Kanıt yoksa eşzamanlı okurlar için atomiklik açık karardır.

### Önemli bedel

Tam çıktı depolama ve I/O tüketir; `/dev/shm` kapasitesi sınırlıdır. Tek PTY ve tek yazıcı paralelliği sınırlar. Ortak PTY arka plan çıktısının kökenini belirleyemez.

### Örnek girdi / çıktı

```text
Girdi 1 (220 satır derleyici çıktısı, toplam 8_400 bayt):
  satır 1..220

Model bağlamına giden çıktı:
  <satır 1..50>
  [... ferrocore: 120 satır (4_510 bayt) budandı; tam çıktı: $FC_SCRATCH/last_output.log ...]
  <satır 171..220>

Girdi 2 (Tek satır, 25_000 bayt, sonunda 4 baytlık "🦀" UTF-8 karakteri sınırda denk geliyor):
  {"data":"aaaa...🦀...zzzz"}

Karar bekleyen durum: Tam satırların iletilmesi bağlamı aşabilir. Bayt dilimleme ADR'ye istisna gerektirir; kullanıcı kararı olmadan kesin çıktı belirlenmez.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| 80 satır, 15 KB | İlk ve son 50 örtüşür; satırları tekrarlamama önerilir. Uzun satır için bağlam tavanı ADR yorumu/onayı bekler. |
| Çok baytlı UTF-8 karakter sınırda | Bayt dilimleme onaylanırsa karakter ortasından kesilmez; ham kayıt korunur. |
| İkili veya geçersiz UTF-8 | Ham kayıt baytları korur, model görünümü kayıplı olabilir. |
| Arka plan işi aktif komut sırasında yazar | Ortak PTY'de kaynağı belirlenemez; aralık kaydına karışabilir ve bu sınırlama bildirilir. |
| Disk dolu veya yazma başarısız | Kısmi dosya tam çıktı sayılmaz; hata döner ve önceki başarılı kayıt korunmaya çalışılır. |
| Windows'ta eşzamanlı okuma ve değiştirme | Atomiklik garanti edilmez; API ve dosya sistemi için platform kanıtı gerekir. |

### Uygulama öncesi kullanıcı kararı

- `10_240` bayt, satır sayımı ve örtüşmesiz görünüm kabul ediliyor mu? Uzun satırda ADR'nin 50+50 tam satır şartı mı, onaylı bayt sınırı mı öncelikli?
- Her komutta mı yalnız eşik aşımında mı `last_output.log` yayımlansın? Dosya sistemi dolması, arka plan çıktısı ve Windows atomikliği için hangi politika seçilsin?

---

## (d) LLM sağlayıcı, akış protokolü, kimlik bilgileri, retry, bütçe ve iptal

### Kaynakta kesin olan

- `ferrocore-driver` içindeki ortak `TurnDriver`; LLM yanıt akışı (streaming), üstel geri çekilmeli yeniden deneme (exponential backoff retry), token bütçesi ve maliyet sayacı (cost guard) ile `Ctrl+C` (`SIGINT`) ve zaman aşımı iptal yönetiminden sorumludur (ADR-008, `docs/architecture.md`).
- `ferrocord` çekirdeği AI, LLM ve prompt kavramlarını içermez; bu mantık tamamen `ferrocore` katmanında kalır (ADR-001).
- ADR'de örnek modeller olarak OpenAI ve DeepSeek ailesi (`deepseek-chat`, `deepseek-reasoner`, `o3`) anılır; ancak kesin HTTP/akış protokolü, kimlik bilgisi kaynağı, retry kapsamı ve üst sınırı, token/maliyet hesaplama yöntemi ve iptal davranışı kaynakta tanımlanmamıştır (`docs/open-questions.md`).

### Öneri ve gerekçe

1. **Sağlayıcı ve akış protokolü:**
   - Faz 2 MVP için tek bir birincil protokol olarak **OpenAI uyumlu Chat Completions SSE (`POST {base_url}/chat/completions`, `stream: true`, `stream_options: {"include_usage": true}`)** önerilir; `base_url` ve `model` `config.toml` içinden okunur. Böylece ek sağlayıcı crate'leri açılmadan (YAGNI) OpenAI uyumlu uç noktalar tek istemciyle sınanabilir.
   - SSE satırları UTF-8 akış çerçeveleyicisiyle parça sınırlarından bağımsız ayrıştırılır ve `data: [DONE]` ile kapanır.
   - **Açık kanıt ihtiyacı:** Hedef sağlayıcının SSE sonunda `usage` nesnesini her durumda (özellikle hata/erken kesme dışında normal bitişte) gönderip göndermediği gerçek uç noktaya karşı doğrulanmalıdır. Yanıtta `usage` yoksa sürücü uydurma kesin token sayısı üretmez; kullanımı `UsageStatus::Unavailable` olarak kaydeder.
2. **Kimlik bilgisi yönetimi:**
   - Gizli API anahtarı `config.toml` içine düz metin yazılmaz; yapılandırmada yalnızca ortam değişkeninin adı (`api_key_env = "FERROCORE_API_KEY"`) belirtilir.
   - Anahtar oturum başında okunur; hata mesajlarında, `Debug` çıktılarında ve `session.jsonl` kayıtlarında asla yer almaz.
   - PTY alt kabuğunda çalışan komutların anahtarı kaza eseri dışarı sızdırmasını azaltmak için ilgili ortam değişkeninin çocuk PTY sürecine aktarılmaması (`env_remove`) önerilir.
3. **Retry kapsamı ve üst sınırı:**
   - Yalnızca ilk içerik parçası (`delta`) tüketilmeden önce geçici ağ hataları veya HTTP `408`, `429`, `500`, `502`, `503`, `504` yeniden denenebilir. Sağlayıcı isteği işlemiş olabilir; delta gelmemesi maliyetsiz veya yinelenmemiş işlem garantisi değildir.
   - Kalıcı istemci, şema veya yetki hatalarında (`400`, `401`, `403`, `404`, `422`) yeniden deneme yapılmaz; doğrudan yapılandırılmış hata döner.
   - Önerilen üst sınır: en fazla `3` yeniden deneme (toplam `4` deneme), `500 ms` taban bekleme, `2x` üstel artış, `8 s` tavan ve rastgele jitter (`Retry-After` başlığı varsa ve kalan zaman aşımını aşmıyorsa dikkate alınır).
   - SSE akışı başladıktan ve ilk `delta` döngüye/kayda iletildikten sonra bağlantı koparsa, yarım yanıtın üzerine sessiz otomatik tekrar yapılmaz; kısmi içerik kaydedilip `DriverError::StreamInterrupted` döndürülür.
4. **Token bütçesi ve maliyet sayacı:**
   - Birincil bütçe kapısı `max_total_tokens` (`prompt_tokens + completion_tokens`) ve `max_turns` sınırlarıdır.
   - Maliyet sınırı (`max_cost_micros_usd`) yalnızca kullanıcı `config.toml` içinde `input_cost_per_million_micros` ve `output_cost_per_million_micros` değerlerini açıkça tanımladıysa hesaplanır; sağlayıcı fiyatları değişken olduğu için koda gömülü sabit fiyat listesi uydurulmaz.
   - Her tur öncesi kümülatif sayaç kontrol edilir; tur sonundaki kullanım eklenir. Limit aşılmışsa veya zorunlu kullanım bilgisi yoksa yanıt kaydedilir fakat içindeki komut yürütülmeden oturum durur. Sayaç yalnız tur sonunda kesinleştiğinden istek limitleri aşabilir; yanıt bayt tavanı token ya da maliyet için kesin sınır değildir. Kesin sınır için sağlayıcının token limiti ve fiyatlandırma kanıtı gerekir.
5. **İptal (`Ctrl+C` ve zaman aşımı):**
   - `TurnDriver`, oturum genelinde paylaşılan bir iptal sinyaliyle (`CancellationToken`) hem aktif HTTP/SSE çağrısını hem beklemedeki retry zamanlayıcısını hem de aktif PTY komut bekleyişini keser.
   - Aktif komutta yalnız bekleyişi kesmek yeterli değildir: PTY çalışma zamanı komutu ve ilgili süreçleri durdurmayı dener, sonucu gözlemler ve başarısızlığı açıkça bildirir. Durdurma doğrulanamazsa başarı veya güvenli temizlik varsayılmaz; scratch korunur ve yeni komut başlatılmaz. Linux süreç grupları ve Windows ConPTY/çocuk süreç ağacı için kesin mekanizma ile ayrılmış arka plan süreçlerinin kapsamı platform kanıtı ve kullanıcı kararı bekler.
   - Zaman aşımı iki düzeyli önerilir: tek adım için `step_timeout_secs` ve tüm oturum için `session_timeout_secs`.
   - **Açık kanıt ihtiyacı (Windows sinyal yayılımı):** Windows konsolunda `Ctrl+C` olayı aynı konsola bağlı çocuk süreçlere ve ConPTY katmanına işletim sistemi tarafından doğrudan iletilebilir. Düzenli iptal sırasında ana sürecin JSONL kapanış kaydını ve scratch temizliğini tamamlayabilmesi için Windows konsol kontrol işleyicisi (`Ctrl+C`) ile ConPTY çocuk süreç davranışının birlikte sınanması gerekir.

### Önemli bedel

Tek protokol diğer sağlayıcılar için adaptör gerektirebilir. Kısmi akışta tekrar yapmamak geçici ağ hatasında oturumu durdurabilir; ancak yinelenen yanıtları azaltır. İlk delta öncesi tekrar bile yinelenen maliyeti önlemez. Anahtarın PTY ortamından çıkarılması aynı değişkeni bekleyen CLI araçlarını etkiler; diğer gizli bilgi kaynakları ayrıca korunmalıdır.

### Örnek girdi / çıktı

```text
Yapılandırma özeti:
  base_url          = "https://api.example.com/v1"
  model             = "deepseek-chat"
  api_key_env       = "FERROCORE_API_KEY"
  max_retries       = 3
  max_total_tokens  = 50_000
  max_turns         = 20

Akış senaryosu (Tur 4, önceki toplam kullanım = 48_900 token):
  1. Deneme 1 -> HTTP 503 Service Unavailable (delta yok)
     -> 500ms + jitter bekle, Deneme 2 başlat.
  2. Deneme 2 -> HTTP 200 text/event-stream:
     data: {"choices":[{"delta":{"content":"<sh>\ncargo check\n</sh>"}}]}
     data: {"choices":[],"usage":{"prompt_tokens":1200,"completion_tokens":18,"total_tokens":1218}}
     data: [DONE]
  3. Yeni toplam kullanım = 50_118 > 50_000:
     -> Tur 4 yanıtı JSONL'e yazılır, komut yürütülmez; BudgetExceeded ile durur.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| `FERROCORE_API_KEY` tanımlı değil veya boş | İlk ağ çağrısı ve PTY başlatması yapılmadan yapılandırma/kimlik hatası döner; gizli değer loglanmaz. |
| HTTP `401 Unauthorized` veya `400 Bad Request` | Kalıcı hata sayılır; retry yapılmadan oturum hata kaydıyla durur. |
| HTTP `429` + `Retry-After: 120` ama kalan `session_timeout` `15 sn` | Bekleme süresi oturum zaman aşımını aştığı için beklenmez; zaman aşımı/oran sınırı hatasıyla çıkılır. |
| İlk `delta` alındıktan sonra TCP bağlantısı kopar | Otomatik tekrar yapılmaz; kısmi çıktı `StreamInterrupted` olarak kaydedilir ve görev durur. |
| Sağlayıcı `usage` vermez | Zorunlu token veya maliyet bütçesinde öneri `UsageUnavailable` ile durmak ve komutu yürütmemektir; farklı politika kullanıcı kararı gerektirir. |
| Retry beklemesi veya aktif SSE sırasında kullanıcı `Ctrl+C` basar | Bekleme/istek anında iptal edilir, yeni deneme açılmaz, `Cancelled` kaydı yazılıp scratch temizlenir (Windows ConPTY davranışı sınanmalıdır). |
| Aktif komut iptalden sonra çalışmayı sürdürür veya durdurma doğrulanamaz | İptal tamamlandı diye sunulmaz; hata ve belirsiz süreç durumu bildirilir, scratch korunur. |

### Uygulama öncesi kullanıcı kararı

- Faz 2 MVP için yalnızca OpenAI uyumlu Chat Completions SSE protokolünün hedeflenmesi ve API anahtarının yalnız ortam değişkeninden okunup PTY ortamından çıkarılması uygun mu?
- Sağlayıcı `usage` verisi döndürmediğinde ve bütçe sınırı açık olduğunda oturum sert hatayla mı durmalı, yoksa yalnız `max_turns` / `max_response_bytes` sınırıyla devam mı etmeli?
- Akış ortası (`delta` sonrası) ağ kopmalarında otomatik tekrar yapılmaması kuralı onaylanıyor mu?

---

## (e) `AgentLoop`, `Tool`, `Session` ve `TurnDriver` sorumluluk sınırları

### Kaynakta kesin olan

- `ferrocord`, içinde ajan/LLM/terminal kavramı bulunmayan; senkron `RwLock` + `Arc<dyn Any + Send + Sync>` tabanlı `Context`, tip tabanlı jenerik `EventBus` ve `Plugin` yaşam döngüsü sunan bağımsız mikro-çekirdektir. `.await` noktaları boyunca senkron kilit tutulmaması zorunludur (ADR-001, `AGENTS.md`).
- `ferrocore-types`; ortak `AgentLoop`, `Tool` ve `Session` sözleşmelerini barındırır. Aktif döngü `config.toml` içindeki `active_loop` anahtarıyla seçilir (ADR-004).
- RLM ve ReAct birbirinin üzerine kurulmaz (`RLM extends ReAct` reddedilmiştir); her ikisi de `ferrocore-driver` içindeki ortak `TurnDriver` altyapısını kullanan kardeş eklentilerdir (ADR-008).
- Faz 2 MVP kapsamı `ferrocore-types`, `ferrocore-driver`, `ferrocore-runtime-pty` ve `ferrocore-loop-rlm` ile sınırlıdır; `internal-spawn` Faz 3'te, `ferrocore-loop-react` ve MCP/WASM `ferrocore-tools` adaptörleri Faz 4'te ele alınacaktır (`docs/roadmap.md`). Kesin Rust trait imzaları ve `config.toml` şeması kaynakta henüz tanımlanmamıştır (`docs/open-questions.md`).

### Öneri ve gerekçe

1. **Sorumluluk ayrımı tablosu (SRP ve DIP):**
   - **`ferrocore-types` (`AgentLoop`, `Tool`, `Session`):** Yalnızca saf veri tiplerini, hata türlerini ve soyut trait sözleşmelerini içerir; HTTP, PTY veya dosya I/O bağımlılığı taşımaz.
   - **`Session` (`ferrocore-types`):** Oturum kimliği, aktif `loop_type`, `agent_depth` (Faz 2'de sabit `0`), konuşma mesaj geçmişi, kümülatif token/tur sayaçları ve son JSONL `record_id` değerini tutan bellek içi durum yapısıdır; açık soket, dosya kolu veya çocuk süreç barındırmaz.
   - **`TurnDriver` (`ferrocore-driver`):** Tur döngüsünün tek orkestratörüdür. Bütçe kapısını, LLM akış çağrısını, geçici hata retry'larını, `CancellationToken` yayılımını ve `SessionLogWriter` üzerinden `session.jsonl` yazımını tek elden yürütür. Döngünün iç ayrıştırma kurallarını veya PTY protokolünü bilmez.
   - **`AgentLoop` (`ferrocore-loop-rlm`):** Canlı kabuk durumundan istem hazırlar ve model yanıtını eylem, tamamlanma veya biçim hatasına ayrıştırır. Komut eylemi sürücüye döner; sürücü komut niyetinin kayıt yazıcısınca başarıyla yazılmasını bekledikten sonra PTY yürütmesini başlatır. Sonuç ayrıca kaydedilir. Döngü LLM istemcisini veya JSONL dosyasını doğrudan kullanmaz.
   - **`Tool` (`ferrocore-types`):** Faz 2'de RLM döngüsü JSON `Tool` çağrısı kullanmadığı için (ADR-008), `Tool` yalnızca Faz 4 ile ileri uyumluluğu koruyan yalın veri/trait çekirdeği (`ToolSpec` + async `Tool` çağrı sözleşmesi) olarak tanımlanır; Faz 2'de `ToolRegistry`, MCP istemcisi veya WASM çalışma zamanı eklenmez (YAGNI).
2. **`ferrocord` entegrasyonu ve kilit disiplini:**
   - Paylaşılan çalışma zamanı servisleri (ör. `Arc<PtySessionManager>`, `Arc<LlmProvider>`) `ferrocord::Context` içine kaydedilir.
   - Servis erişiminde yalnızca paylaşılan referans alınır; senkron kilit kapsamı `.await` başlamadan biter. Kesin çağrı imzası kaynak API incelenmeden varsayılmaz.
   - **Açık kanıt ihtiyacı:** Sabitlenmiş `ferrocord` (`ae07423...`) commit'indeki `Context`, `EventBus` ve `Plugin` trait imzaları (`Send + Sync`, yaşam döngüsü kancaları) crate bölünmesi sırasında doğrudan kaynak koda bakılarak eşleştirilmeli; `ferrocord` API'sini değiştirmeden sarmalanmalıdır.
3. **Asgari `config.toml` sınırı (Faz 2):**
   - Yalnızca Faz 2 bileşenlerinin ihtiyaç duyduğu bölümler (`active_loop`, `[llm]`, `[budget]`, `[pty]`, `[session]`) tanımlanır; bilinmeyen `active_loop` değerinde (ör. henüz uygulanmamış `"react"`) sessiz geri dönüş yapılmaz, açık yapılandırma hatası verilir.

### Önemli bedel

Döngülerin LLM çağrılarını sürücü üzerinden yürütmesi adım dönüşlü tasarım gerektirir; ortak politikaların tek yerde uygulanmasını kolaylaştırır, kendi başına doğru uygulamayı garanti etmez.

### Örnek girdi / çıktı

```text
Girdi (config.toml + CLI görev başlatma):
  active_loop = "rlm"
  görev       = "Projeyi derle ve uyarıları özetle"

Tek turluk kontrol akışı:
  1. TurnDriver -> bütçe ve iptal kontrolü (OK)
  2. TurnDriver -> RlmLoop::prepare_turn(&session)
     <- Mesaj listesi + canlı başlık (PWD, son exit_code, scratch özeti), tools = []
  3. TurnDriver -> LlmClient::stream_chat(...) + SessionLogWriter::append(llm_request/response)
  4. TurnDriver -> RlmLoop::on_model_response(&mut session, model_text, cancel)
     <- komut eylemi (henüz yürütülmedi)
  5. TurnDriver -> SessionLogWriter::append(pty_command); yazma onayı beklenir.
     Yazma başarısızsa PTY'ye komut gönderilmez.
  6. TurnDriver -> PTY komutunu yürüt; pty_result kaydını ekle, sonraki tura geç.
     Çökme niyet kaydından sonra olursa yürütmenin gerçekleşip gerçekleşmediği belirsiz kalabilir; otomatik tekrar yapılmaz.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| `config.toml` içinde `active_loop = "react"` veya tanımsız döngü adı verilir | Faz 2'de ReAct uygulanmadığı için oturum başlamadan `ConfigError::UnsupportedLoop("react")` döner; RLM'e sessizce düşülmez. |
| PTY yürütmesi sırasında hata veya iptal oluşur | Çalışma zamanı sonucu sürücüye bildirir; sürücü mümkünse hata/iptal kaydını ekler. Süreç durdurma doğrulanamazsa scratch korunur. |
| Komut niyeti yazılamaz veya yazma onayından sonra süreç çöker | İlk durumda komut başlamaz; ikinci durumda niyet tek başına yürütme kanıtı değildir, sonuç belirsizliği bildirilir. Güç kaybına dayanıklılık eşitleme politikasına bağlıdır. |
| Döngü tamamlanma üretmeden sürekli `TurnOutcome::Continue` döndürür | `TurnDriver` üzerindeki `max_turns` veya `max_total_tokens` sınırı devreye girer; sonsuz döngü önlenir. |
| `ferrocord::Context` içinden servis alınırken `.await` çağrısı gerekir | Servis referansı önce yerel `Arc<T>` değişkenine klonlanır, kilit kapsamı kapanır, `.await` yalnızca klonlanan `Arc<T>` üzerinde çalıştırılır. |

### Uygulama öncesi kullanıcı kararı

- `AgentLoop` uygulamasının doğrudan LLM ve JSONL erişimi yapmaması, tüm LLM/bütçe/kayıt akışının `TurnDriver` mülkiyetinde kalması kabul ediliyor mu?
- Faz 2'de `ferrocore-types` içinde `Tool` sözleşmesinin yalnız soyut çekirdek tanım olarak bırakılıp kayıt/adaptör katmanının Faz 4'e ertelenmesi onaylanıyor mu?

---

## (f) `session.jsonl` veri şeması, ebeveyn ağacı ve tek yazıcı sahipliği

### Kaynakta kesin olan

- Hem ana ajan adımları hem de ileride dallanacak alt ajanların geçmişi tek bir append-only `session.jsonl` dosyasında ağaç/DAG yapısını koruyarak saklanır (ADR-007).
- Her JSONL satırı şu altı alanı içerir: `id`, `parent_id`, `agent_depth`, `loop_type`, `timestamp`, `payload` (ADR-007, `docs/architecture.md`, `AGENTS.md`).
- Alanların kesin veri tipleri, kök kaydın `parent_id` gösterimi, kimlik üretimi, sürümleme, atomiklik ve eşzamanlı yazma sahipliği kaynakta açık bırakılmıştır (`docs/open-questions.md`).

### Öneri ve gerekçe

1. **Sabit üst seviye alanlar ve tipleri:**
   - ADR-007'deki 6 alan değiştirilmeden ve üst seviyeye fazladan alan eklenmeden şu tipler önerilir:
     - `id`: `String` — oturum içinde benzersiz kayıt kimliği; UUIDv7 adaydır, rastgele çakışma olasılığı sıfır değildir ve yinelenen kimlik reddedilmelidir.
     - `parent_id`: `String | null` (`Option<String>`) — bu kaydın doğrudan nedensel öncülü olan kaydın `id` değeri.
     - `agent_depth`: `u32` — Faz 2 MVP'de kök ajan için her zaman `0` (Faz 3 alt ajanlarında `1, 2, ...` değerlerini almaya hazır, ancak Faz 3 spawn sözleşmesi burada tanımlanmaz).
     - `loop_type`: `String` — kaydı üreten döngü kimliği (Faz 2'de `"rlm"`).
     - `timestamp`: `String` — UTC RFC 3339 zaman damgası (ör. `"2026-11-07T12:00:01.250Z"`).
     - `payload`: `Object` — şema evrimi ve olay ayrımı için `{"v": 1, "kind": "<olay_türü>", "data": { ... }}` zarfını taşır (`session_start`, `turn_input`, `llm_output`, `pty_exec`, `pty_result`, `error`, `session_end`).
2. **Kök ve ebeveyn-çocuk zinciri:**
   - Bir `session.jsonl` dosyasındaki **ilk kayıt** oturum köküdür (`kind = "session_start"`); yalnızca kökün `parent_id` değeri `null` olur. Faz 2'de tüm kayıtların `agent_depth` değeri `0` olur.
   - Aynı ajan içindeki her yeni kayıt, bir önceki tamamlanmış kaydın `id` değerini `parent_id` olarak alır.
   - Veri modeli aynı `parent_id` değerine birden çok çocuğun bağlanmasına (ağaç dallanmasına) izin verir; böylece şema Faz 3 alt ajan dallanmasını kırmadan destekler.
3. **Eşzamanlı yazıcı sahipliği (Tek yazıcı kuyruğu):**
   - Farklı asenkron görevlerin veya süreçlerin aynı `session.jsonl` dosyasına doğrudan paralel yazması **önerilmez**; ne POSIX `O_APPEND` (boru/dosya tampon sınırları aşıldığında) ne de Windows dosya ekleme modları keyfi uzunluktaki JSON satırlarının birbirine karışmayacağını taşınabilir biçimde garanti eder.
   - Dosya tanımlayıcısının tek sahibi `ferrocore-driver` içindeki **`SessionLogWriter`** görevidir. Üreticiler kayıt nesnelerini sınırlı bir bellek içi kanala iletir; `SessionLogWriter` her kaydı tek satırlık UTF-8 JSON'a (içerideki yeni satırlar `\n` olarak JSON kaçışlı, satır sonu tek `LF` `\n`) serileştirip sırayla yazar.
4. **Atomiklik sınırı ve kısmi satır davranışı:**
   - Standart dosya sistemlerinde çok baytlı satır ekleme işlemi süreç çökmesi (`SIGKILL`) veya güç kaybına karşı **atomik değildir**; en sondaki satır diske yarım yazılmış kalabilir.
   - Her kayıt tamponlanıp sırayla yazılır ve `flush` edilir; `session_start` ve `session_end` / hata kapanış kayıtlarında `sync_data` çağrılması önerilir. Kısmi yazma ve eşitleme hataları bildirilir; `flush` veya `sync_data` her platform ve depolama aygıtında güç kaybına karşı mutlak garanti değildir.
   - Dosya ortasında bozuk JSON veya tanımsız `parent_id` veri bozulmasıdır. En sondaki satır `\n` içermiyorsa kesilmiş olabilir ve açıkça raporlanır; son satır yeni satırla bitiyor ama JSON geçersizse bozulmadır. Dosya sessizce değiştirilmez.

### Önemli bedel

Tek yazıcı yüksek olay hızında kuyruk ve geri basınç oluşturabilir. Daha sık eşitleme veri kaybı riskini azaltabilir fakat gecikme getirebilir; etki ölçülmemiştir ve dayanıklılık platforma bağlıdır.

### Örnek girdi / çıktı

```jsonl
{"id":"019a5f10-0001-7000-8000-000000000001","parent_id":null,"agent_depth":0,"loop_type":"rlm","timestamp":"2026-11-07T12:00:00.100Z","payload":{"v":1,"kind":"session_start","data":{"task":"cargo check çalıştır"}}}
{"id":"019a5f10-0002-7000-8000-000000000002","parent_id":"019a5f10-0001-7000-8000-000000000001","agent_depth":0,"loop_type":"rlm","timestamp":"2026-11-07T12:00:01.420Z","payload":{"v":1,"kind":"llm_output","data":{"raw_text":"<sh>\ncargo check\n</sh>","usage":{"prompt_tokens":310,"completion_tokens":12}}}}
{"id":"019a5f10-0004-7000-8000-000000000004","parent_id":"019a5f10-0002-7000-8000-000000000002","agent_depth":0,"loop_type":"rlm","timestamp":"2026-11-07T12:00:01.500Z","payload":{"v":1,"kind":"pty_exec","data":{"cmd_id":1,"command":"cargo check"}}}
{"id":"019a5f10-0003-7000-8000-000000000003","parent_id":"019a5f10-0004-7000-8000-000000000004","agent_depth":0,"loop_type":"rlm","timestamp":"2026-11-07T12:00:03.005Z","payload":{"v":1,"kind":"pty_result","data":{"cmd_id":1,"exit_code":0,"pruned":false,"output_preview":"Finished `dev` profile\n"}}}
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| İlk kaydın `parent_id` alanı `null` değil veya ikinci bir `parent_id: null` satırı gelir | Şema doğrulayıcı tek kök kuralı ihlali (`MultipleOrMissingRoot`) döndürür. |
| Kayıt `parent_id` değeri dosyada daha önce görülmemiş bir `id` gösterir | Nedensel sıra bozulduğu için `DanglingParentId` hatası raporlanır. |
| PTY çıktısı çok satırlı metin ve tırnak karakterleri içerir | JSON serileştirmesi `\n` ve `\"` kaçışlarını uygular; fiziksel dosyada her kayıt tam olarak tek satır kalır. |
| Süreç `SIGKILL` alırken son satır yarım yazılır (`{"id":"019...` ve sonunda `\n` yok) | Okuyucu bunu sessizce yok saymaz; `TruncatedTailLine` olarak bildirir ve önceki tam kayıtları ayrıştırılabilir tutar. |
| Yazma sırasında disk dolar (`ENOSPC`) veya izin hatası oluşur | `SessionLogWriter` hatayı `TurnDriver`'a iletir; kayıtsız sessiz yürütme yapmamak için oturum kayıt hatasıyla durdurulur. |

### Uygulama öncesi kullanıcı kararı

- `id` için UUIDv7 dizgesi, kök kayıtta `parent_id: null` ve `payload` içinde `{"v": 1, "kind": ..., "data": ...}` zarfı onaylanıyor mu?
- `session.jsonl` yazma hatası (`ENOSPC` vb.) oluştuğunda oturumun derhal durdurulması mı, yoksa bellek içi uyarı verip kayıtsız devam etmesi mi tercih ediliyor?
- Dayanıklılık için `fsync` yalnızca oturum açılış/kapanışında mı, yoksa her tur sonunda mı çalıştırılsın?

---

## (g) RLM ham kod bloğu ayrıştırması, canlı başlık ve hatalı yanıt davranışı

### Kaynakta kesin olan

- ADR-008'e göre `ferrocore-loop-rlm`, JSON Function Calling kaçış hatalarından kaçınmak için modelden doğrudan ham kabuk kod blokları alır ve bunları PTY'ye aktarır.
- ADR-008'de Markdown `bash` bloğu ve `<sh>...</sh>` örnek biçimler olarak verilmiştir; kesin etiket sözleşmesi, çoklu blok kuralı, tamamlanma bildirimi ve bozuk model yanıtı davranışı kaynakta açık bırakılmıştır (`docs/architecture.md`, `docs/roadmap.md`).
- RLM döngüsü her adımda kabuğun canlı durumunu (`PWD`, son çıkış kodu `$?`, arka plan işleri `jobs` ve `$FC_SCRATCH` içeriği) bağlama dinamik olarak ekler (ADR-008). Prime Agent kaynakta ilham olarak anılır; depoda uygulanmış RLM kodu yoktur.

### Öneri ve gerekçe

1. **Kanonik eylem ve tamamlanma etiketleri:**
   - Açıklama amaçlı Markdown kod çitlerinin yanlışlıkla çalışmasını azaltmak için yürütülebilir biçim olarak kendi satırında açılıp kapanan **`<sh>\n...\n</sh>`**, bitiş için **`<final>\n...\n</final>`** önerilir. Açıklama içindeki etiketler de ayırt edilemeyebilir; açık dilbilgisi ve model uyumu sınanmalıdır.
   - Etiket dışındaki düz açıklama metni JSONL kaydında saklanır, PTY'ye gönderilmez. Eylem etiketleri yalnız Markdown kod çitleri dışında tanınır; çitin içindeki `<sh>` ve `<final>` örnekleri çalıştırılmaz. Kapanmamış veya belirsiz çitte hata verilir, eylem yürütülmez.
   - **Etiket kaçış kuralı:** Komut gövdesinde literal `</sh>` metni geçmesi gerekiyorsa satır içi kullanımlar (ör. `echo "</sh>"`) kapanış sayılmaz; yalnızca başında/sonunda boşluk olmadan tek başına satır oluşturan `\n</sh>\n` (veya yanıt sonundaki `\n</sh>`) kapanış kabul edilir.
   - **Açık kanıt ihtiyacı:** Hedef modelin (`deepseek-chat` vb.) sistem istemiyle `<sh>...</sh>` ve `<final>...</final>` biçimine uyum oranı bu depoda henüz ölçülmemiştir; uygulama başında küçük bir istem doğrulama setiyle sınanmalıdır.
2. **Tur başına tek eylem kuralı:**
   - Kalıcı kabukta bir komutun çıktısı ve çıkış kodu bir sonraki komutu doğrudan etkilediği için her turda **yalnızca tek bir `<sh>...</sh>` bloğu** veya **tek bir `<final>...</final>` bloğu** geçerli kabul edilir.
   - Aynı yanıtta birden fazla `<sh>` bloğu ya da hem `<sh>` hem `<final>` bulunursa **hiçbir komut PTY'ye gönderilmez**; modele biçim hatası bildirilir.
3. **Canlı durum başlığı (Stateful Header) üretimi:**
   - Her başarılı/başarısız PTY adımından sonra bir sonraki tur girdisine deterministik bir durum özeti eklenir: son bilinen `PWD`, son `exit_code`, varsa kabuktaki arka plan iş sayısı (`jobs`) ve `$FC_SCRATCH` içindeki üst düzey dosya adları (çok sayıda dosya varsa bağlamı şişirmemek için ilk `N` giriş + toplam sayı özeti).
   - Kabuk süreci kapanmışsa (`ShellExited`) canlı kabuk sorgusu çalıştırılamaz; başlıkta `shell_status: exited` ve son bilinen değerler gösterilir.
4. **Hatalı ve yarım model yanıtı davranışı:**
   - Şu durumlar **hatalı model yanıtı (`MalformedModelOutput`)** sayılır: (a) `<sh>` açılıp `</sh>` ile kapanmaması (ör. `max_tokens` nedeniyle kesilmiş yanıt), (b) yalnızca boşluk içeren `<sh></sh>` bloğu, (c) yanıtta ne `<sh>` ne `<final>` bulunması, (d) aynı yanıtta birden fazla eylem/bitiş bloğu bulunması.
   - **Önerilen güvenlik kuralı:** Kapanmamış veya belirsiz bir `<sh>` bloğu kısmen PTY'de çalıştırılmaz.
   - Hatalı yanıtta döngü `TurnDriver` üzerinden modele kısa ve yapılandırılmış bir biçim düzeltme mesajı döndürür. Ardışık düzeltme denemesi `max_format_retries` (öneri: `2`) ile sınırlandırılır; bu turlar da normal token bütçesine ve `session.jsonl` kaydına dahildir. Sınır aşılırsa oturum `StopReason::MalformedModelOutput` ile sonlandırılır.

### Önemli bedel

Yalnız `<sh>` kabul etmek Markdown çiti üreten modellerde düzeltme turu ve token maliyeti doğurabilir. Açıklama içindeki etiketlerin de eylem sayılması riski tamamen ortadan kalkmaz; ayrıştırma kuralları sınanmalıdır.

### Örnek girdi / çıktı

```text
Örnek 1 — Geçerli RLM turu:
  Model yanıtı:
    Çalışma dizinindeki Cargo manifestini kontrol ediyorum.
    <sh>
    cargo metadata --no-deps --format-version 1 | head -c 300
    </sh>

  Ayrıştırıcı sonucu:
    Action::ExecShell("cargo metadata --no-deps --format-version 1 | head -c 300")

  Sonraki tura eklenen canlı başlık + çıktı:
    [ferrocore-state pwd="/workspace/ferrocore" last_exit=0 bg_jobs=0 scratch=["last_output.log"]]
    <komut çıktısı veya budanmış görünüm>

Örnek 2 — Kapanmamış (max_tokens ile kesilmiş) model yanıtı:
  Model yanıtı:
    Geçici derleme dosyalarını temizleyelim:
    <sh>
    rm -rf target/debug/build/

  Ayrıştırıcı sonucu:
    Action::Malformed("Unclosed <sh> block; nothing was executed.")
    -> PTY'ye hiçbir bayt yazılmaz; modele düzeltme uyarısı gönderilir (deneme 1/2).
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| Yanıt `finish_reason = "length"` ile `<sh>` ortasında kesilir | Yarım komut çalıştırılmaz; modele kapanmamış blok uyarısı iletilir. |
| Model Markdown kod çiti ve ayrıca `<final>...</final>` verir | Çit çalıştırılmaz; `<final>` geçerli bitiş olarak kabul edilir. |
| Model yalnız Markdown kod çiti yazar | Komut çalıştırılmaz; modele `<sh>...</sh>` veya `<final>...</final>` biçimi hatırlatılır. |
| Markdown kod çiti içinde tek başına satırlarda `<sh>`, komut ve `</sh>` bulunur; çit dışında eylem yoktur | Etiketler örnek metindir; hiçbir komut yürütülmez ve biçim hatası bildirilir. |
| Model aynı yanıtta iki ayrı `<sh>...</sh>` bloğu üretir | İkisi de çalıştırılmaz; tur başına tek blok kuralı hatırlatılır. |
| Komut içinde heredoc veya metin olarak `echo "</sh>"` geçer | Satır içi `</sh>` kapanış sayılmaz; yalnız tek başına satır olan `</sh>` bloğu kapatır. Komut tek satırda `</sh>` yazmaya çalışırsa kapanış çakışması nedeniyle kullanıcı/model kaçışı gerekir. |
| Model ardışık `3` tur bozuk yanıt üretir (`max_format_retries = 2` aşılır) | Oturum `StopReason::MalformedModelOutput` ile durdurulur ve hata `session.jsonl`'e kaydedilir. |

### Uygulama öncesi kullanıcı kararı

- Yalnızca `<sh>...</sh>` ve `<final>...</final>` etiketlerinin kabul edilmesi (serbest ` ```bash ` bloklarının otomatik çalıştırılmaması) onaylanıyor mu?
- Bozuk model yanıtlarında ardışık düzeltme sınırı (`max_format_retries = 2`) ve yarım bloğun hiçbir koşulda çalıştırılmaması politikası uygun mu?
