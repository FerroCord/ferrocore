# Faz 2 MVP Sözleşmeleri ve Kararları

## Belge statüsü ve kapsam

Kullanıcı (a)–(g) araştırma önerilerini onayladı. Bunlar Faz 2 tasarım kararlarıdır; henüz uygulanmış özellik veya doğrulanmış platform davranışı değildir. Sayısal varsayılanlar yapılandırılabilir başlangıç değerleridir. Platform kabul testleri ve aşağıda belirtilen kanıt eksikleri geçerlidir; kaynak ADR kopyası değiştirilmez.
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
   - **Linux:** `bash --noprofile --norc -i` kullanılır; `bash` kurulu olmalıdır. Profil atlamak başlangıç etkilerini azaltır, deterministik terminal davranışını garanti etmez.
   - **Windows:** `portable-pty` ve ConPTY üzerinde `pwsh.exe -NoLogo -NoProfile` kullanılır; PowerShell ayrıca kurulmalıdır. Kodlama, VT dizileri ve çıkış semantiği platform kabul testlerinde doğrulanmalıdır.
2. **Başarısız başlatma davranışı:**
   - Kabuk ikilisi bulunamazsa, PTY çifti açılamazsa veya kabuk açıldıktan sonra yapılandırılan başlangıç el sıkışma süresinde (ör. `5 saniye`) ilk doğrulama sentinel'i dönmezse `PtyInitError` üretilir.
   - Başlatma hatasında farklı kabuğa sessiz geçiş yapılmaz ve LLM başlatılmaz. Çocuk süreç durdurulmaya çalışılır; süreç yoksa veya duruş doğrulanmışsa yalnız oturuma ait scratch temizlenmeye çalışılır, aksi halde scratch korunur, yolu ve belirsizlik bildirilir.
3. **`$FC_SCRATCH` oluşturma, geri dönüş ve temizlik yaşam döngüsü:**
   - `{id}` için rastgele kimlik kullanılır; çakışma olasılığı sıfır değildir, dizin münhasır oluşturulur ve çakışmada yeni kimlik denenir. Windows'ta rastgele ad erişim izni değildir; ACL doğrulanmalıdır.
   - **Linux:** Önce `/dev/shm` dizininin varlığı ve yazılabilirliği kontrol edilir; `/dev/shm/ferrocore_{id}` dizini yalnızca süreç sahibine açık izinle (`0700`) oluşturulur. `/dev/shm` yoksa, izin reddedilirse (`EACCES`) veya bellek diski doluysa (`ENOSPC`), oturumu düşürmek yerine `/tmp/ferrocore_{id}` (`0700`) dizinine geri dönülür ve yapılandırılmış bir uyarı olayı kaydedilir.
   - **Windows:** `std::env::temp_dir().join(format!("ferrocore_{id}"))` (tipik olarak `%LOCALAPPDATA%\Temp\ferrocore_{id}`) kullanılır.
   - **Temizlik:** Normal bitiş, iptal ve başlatma hatasında ilgili süreçlerin duruşu doğrulanmışsa veya süreç hiç başlatılmamışsa yalnız oturuma ait dizin temizlenmeye çalışılır; aksi halde scratch korunur, yolu ve belirsizlik bildirilir ve yeni komut başlatılmaz. Temizlik hataları bildirilir; zorla sonlandırmada kalan dizinler sonraki oturumda sessizce silinmez. Süreç ağacının doğrulanması platform testlerine bağlıdır.

### Önemli bedel

Profil dosyalarının atlanması başlangıç etkilerini azaltabilir, ama kullanıcının profilinde yaptığı PATH değişiklikleri görünmeyebilir. Üst süreç PATH'ini aktarmak önerilir; bu tek başına çerçeveleme garantisi değildir.

### Örnek girdi / çıktı

```text
Girdi (Linux oturum başlatma, id = "8f2a...c901", /dev/shm yazılabilir):
  spawn: bash --noprofile --norc -i
  env:   FC_SCRATCH=/dev/shm/ferrocore_8f2a...c901

Başlangıç el sıkışması (çekirdek -> PTY):
  printf '\036__FC_READY__:8f2a...c901__\037\n'

Beklenen sonuç:
  1. /dev/shm/ferrocore_8f2a...c901 (0700) mevcuttur.
  2. El sıkışma çerçevesi 5 sn içinde okunur ve ayrıştırıcı tarafından tüketilir (girdi yankısı terminal ayarına bağlıdır; platformda doğrulanmalıdır).
  3. İlgili süreçlerin duruşu doğrulanırsa oturum dizini temizlenmeye çalışılır.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| Konteyner ortamında `/dev/shm` yok veya salt okunur | `/tmp/ferrocore_{id}` (`0700`) dizinine düşülür; oturum kaydına `scratch_fallback` uyarısı yazılır. |
| Hedef kabuk ikilisi (`bash` veya `pwsh`) sistemde kurulu değil | Otomatik kabuk değiştirme yapılmaz; çocuk başlatılmadıysa scratch temizlenmeye çalışılır ve açık `PtyInitError::ShellNotFound` döner. |
| Kabuk açıldı ancak el sıkışma çerçevesi zaman aşımına uğradı | Çocuğu durdurma denenir; duruş doğrulanırsa scratch temizlenmeye çalışılır, aksi halde korunur, yolu ve belirsizlik bildirilir. LLM çağrısı yapılmaz. |
| Oturum `SIGKILL` ile çöktü | `ferrocore_{id}` dizini inceleme/kurtarma için diskte kalır; sonraki normal oturum başka `{id}` alır ve eski dizinleri sessizce silmez. |
| Windows ConPTY altında PowerShell çıkış kodu ve VT dizileri | **Açık kanıt ihtiyacı:** Uygulama öncesi `portable-pty` ile gerçek ConPTY duman testi çalıştırılarak `$LASTEXITCODE`/`$?` sarmalayıcısı doğrulanmalıdır. |

### Onaylanan karar ve doğrulama

`bash --noprofile --norc -i`, `pwsh -NoLogo -NoProfile`, farklı kabuğa geçmeme, yapılandırılabilir 5 saniyelik el sıkışması ve Linux `/dev/shm` → `/tmp` geri dönüşü seçildi. PTY açma ve spawn çağrıları bu el sıkışması süresiyle otomatik sınırlanmaz. Duruşu doğrulanamayan süreçlerde scratch korunur; süreç ağacı ve temizlik platformlarda sınanmalıdır.

---

## (b) Sentinel çerçeveleme protokolü ve güven sınırı

### Kaynakta kesin olan

- Kalıcı kabuk her komutta kapanmadığı (`EOF` üretmediği) için her komutun sonuna çekirdek tarafından bir çerçeveleme komutu eklenir; `Stream Interceptor` bu diziyi yakaladığında komutun tamamlandığını ve çıkış kodunu anlamayı hedefler (ADR-002).
- **ADR `image1`:** Görselden byte biçimi güvenilir biçimde çıkarılamamıştır. Linux RS/US ve Windows ASCII biçimleri kullanıcı tarafından platform kabul testleri koşuluyla seçilmiştir; bunlar kaynak alıntısı değildir.

### Öneri ve gerekçe

1. **Byte çerçevesi, oturum gizli değeri ve komut kimliği:**
   - Her PTY oturumu başlarken 128-bit kriptografik rastgele `session_nonce` (`32` küçük harf hex karakter) üretilir.
   - Her komuta oturum içinde kesin artan `cmd_id: u64` (`1, 2, 3, ...`) verilir.
   - Linux'ta ham sentinel byte çerçevesi `0x1E` (`RS`) ve `0x1F` (`US`) ile sınırlandırılır:
     ```text
     \x1e__FC_END__:<session_nonce>:<cmd_id>:<exit_code>__\x1f\n
     ```
   - `session_nonce` kabuk ortam değişkenine (`export`) **yazılmaz**; yalnızca çekirdeğin gönderdiği sarmalayıcı satırda anlık sabit olarak yer alır.
   - Windows için seçilen aday `__FC_END__:<session_nonce>:<cmd_id>:<exit_code>__` ASCII çerçevesi ve satır sonudur. Echo, VT, satır sarılması, çakışma, tüm parçalanma sınırları ve PowerShell çıkış semantiği kabul testlerinde doğrulanmalıdır; geçici deney üretim garantisi değildir.
   - PowerShell'de `$LASTEXITCODE` yerel programların çıkış durumuyla ilgilidir; cmdlet ve PowerShell deyimleri için `$?` ayrıca ele alınmalıdır. Bütün komut türleri için tek ve doğru sayısal çıkış kodu semantiği henüz kanıtlanmamıştır; sarmalayıcı platform kabul testlerine bağlıdır.
2. **Komut sarmalama (POSIX `bash`):**
   - Yorumla biten tamamlanmış bir komuttan sonra `\n` eklemek sarmalayıcının yorum içinde kalmasını önleyebilir; kapanmamış tırnak, heredoc veya çok satırlı sözdizimi için yeterli değildir. Sözdizimi ve etkileşimli kabuk davranışı platformda doğrulanmalıdır.
   - `printf` için sekizli kaçışlar kullanmak, girdi yankısında kontrol baytlarının doğrudan bulunmasını önlemeyi hedefler; terminal dönüşümleri ve yankı davranışı ayrıca doğrulanmalıdır:
     ```sh
     <kullanıcı_komutu>
     __fc_ec=$?; printf '\036__FC_END__:%s:%s:%d__\037\n' '<session_nonce>' '<cmd_id>' "$__fc_ec"
     ```
3. **Parçalı byte akışı:** Tek okuyuculu ayrıştırıcı hem Linux `0x1E` hem Windows ASCII `__FC_END__:` önekini her okuma sınırında sınırlı aday tamponunda izler. Geçersiz veya sınırı aşan adayın baytları kayıpsız sıradan çıktıya geri verilir; geri verilen dizide örtüşen veya iç içe başlayan sonraki ilgili platform önekleri yeniden değerlendirilir. Tam eşleşme (`\n` veya `\r\n`) yalnız aktif kimlik ve geçerli çıkış kodunda bitiş adayı sayılır. EOF veya iptalde kısmi aday çıktı olarak korunur. Her byte sınırı, örtüşme ve terminal dönüşümleri iki platformda da sınanmalıdır.
4. **Kabuk kapanması ve zaman aşımı:**
   - Aktif komut sürerken PTY `EOF` döndürür veya çocuk kabuk süreci kapanırsa (`exit` komutu, `exec` hatası, çökme), bekleyen komut `PtyCommandOutcome::ShellExited { exit_status, partial_output }` olarak sonlanır; sürücü kabuğun kapandığını modele açıkça bildirir veya yapılandırmaya göre yeni kabuk başlatmadan hatayla durur.
5. **Güven sınırı (Trust Boundary):**
   - **Seçilen varsayılan:** Kalıcı kabuk durumunu koruyan bant içi sentinel, komutların sentinel'i ve kabuk sarmalayıcısını kasıtlı olarak bozmaması varsayımıyla kullanılır. Linux RS/US, Windows ASCII platform kabul testlerine bağlıdır.
   - Sentinel sandbox değildir; nonce kimlik doğrulama sağlamaz, yalnızca kazara yanlış eşleşme olasılığını azaltmayı hedefler. Sarmalayıcıda görünen nonce aynı PTY'deki komut veya süreç tarafından öğrenilip geçerli çerçeve taklit edilebilir. Güvenilmeyen komutlarda sahte bitiş gerçek bitişten ayırt edilemez ve çıkış kodu kesin değildir. ADR-002'nin kesin çıkış kodu ifadesi ancak seçilen güven varsayımı altında hedeflenebilir; platform davranışı ayrıca doğrulanmalıdır.
   - Kabuk durumu bozulabilir (`set -e`, `trap`, `exit`) ve arka plan süreçleri yazabilir. Ayrı kontrol kanalı tek başına izolasyon sağlamaz; güvenilmeyen komutlar için işletim sistemi izolasyonu ve uygun kontrol mekanizması ayrıca değerlendirilmelidir.

### Platform deneyi: ön koşul sonuçları

Bu deneyler kullanıcı tarafından seçilen adayların üretimde doğrulandığını göstermez; sonuçlar yalnız belirtilen WSL ve Windows ortamlarındaki gözlemlerdir.

| Platform / echo modu | Sonuç |
| --- | --- |
| Windows 10.0.26100.9711, PowerShell 7.6.9 / varsayılan | **Gözlendi:** `GetConsoleMode` başarılı, mod `503`; normal çıktı ve girilen komut göründü. |
| Windows / açık | **Gözlendi:** `SetConsoleMode` başarılı, mod `503 → 503`; normal çıktı ve girilen komut göründü. |
| Windows / kapalı | **Kanıtsız:** `SetConsoleMode` başarılı, mod `503 → 499` geri okundu; buna rağmen girilen komut göründü. Echo kapatmanın etkisi gösterilemedi. |
| Linux WSL 2, Ubuntu-26.04 / varsayılan | **Gözlendi:** `stty -a` `echo` bildirdi; komut ve çıktısı göründü. |
| Linux WSL 2 / açık | **Gözlendi:** `stty echo` sonrasında `stty -a` `echo` bildirdi; komut ve çıktısı göründü. |
| Linux WSL 2 / kapalı | **Gözlendi:** `stty -echo` sonrasında `stty -a` `-echo` bildirdi; normal komutun ve sarmalayıcının yankısı görünmedi, çıktı göründü. |

Linux deneyi: Bash `5.3.9(1)`, Rust/Cargo `1.93.1`, `portable-pty 0.9.0`; ayrı 80×24 PTY oturumları `bash --noprofile --norc -i` ile açıldı. `printf 'NORMAL_%s\n' MARKER` çıktısı her modda `4e4f524d414c5f4d41524b45520d0a` (`NORMAL_MARKER` ve CRLF) idi. `true` ve `false` komutları ile taslaktaki `__fc_ec=$?; printf '\036__FC_END__:%s:%s:%d__\037\n' '0123456789abcdef0123456789abcdef' '7' "$__fc_ec"` sarmalayıcısı ayrı satırlarda gönderildi; gerçek durum hemen `$?` ile yakalandı. Nonce `0123456789abcdef0123456789abcdef`, kimlik `7` idi. Varsayılan ve açık modda sarmalayıcı yankılandı; kapalı modda yankılanmadı.

- Başarı çerçevesi: `1e5f5f46435f454e445f5f3a30313233343536373839616263646566303132333435363738396162636465663a373a305f5f1f0d0a` (çıkış `0`).
- Başarısızlık çerçevesi: `1e5f5f46435f454e445f5f3a30313233343536373839616263646566303132333435363738396162636465663a373a315f5f1f0d0a` (`false` çıkışı `1`). Her iki çerçevede RS `1e`, US `1f` ve CRLF `0d0a` gözlendi.
- Her modda `cd /tmp` sonrasında ayrı `printf 'PWD_%s\n' "$PWD"` komutu `PWD_/tmp` döndürdü; `exit` sonrasında kabuklar başarıyla kapandı. Zaman aşımı gözlenmedi.
- İlk, komut ve sarmalayıcının aynı satırda gönderildiği denemede açık modda yankılanan sarmalayıcı ve çerçeve bir okumada birleşti; ayrı satırlı denemede kapalı modda `false` çerçevesinin son CRLF baytları ayrı okumaya bölündü. Bu gözlemler bütün parçalanma koşullarının sınanması değildir. Üç mod tekrarlandığında zaman damgaları okuyucu iş parçacığında `read` dönüşünden hemen sonra alındı; bu, baytların üretim zamanı değildir. Önceki kayıtlarda damgalar kanal tüketiminde alındığından gecikmeli olabilir. Tekrarın ham kaydı `D:/pty-linux-experiment/experiment-read-timestamp.log`; kapanışta okuyucu beklenmediği için son baytların tamamı kaydedilmiş olmayabilir.
- Echo kapalıyken bile akışta `PTY_READY> ` istemi ve `1b5b3f323030346c0d0d0a` gibi terminal kontrol baytları görüldü; echo kapatmak tek başına temiz komut çıktısı sağlamadı.

Windows deneyi: `portable-pty 0.9.0`, 80×24 ConPTY ve `pwsh.exe -NoLogo -NoProfile`. İlk denemede üç oturum imleç konumu sorgusu `1b 5b 36 6e` sonrası 10 saniyede zaman aşımına uğradı. Tekrarda deneye terminal yanıtı `1b 5b 31 3b 31 52` eklendi; üç oturum tamamlandı ve `exit` ile kapandı. Bu yanıtın genel bir terminal uygulaması için yeterliliği sınanmadı. PTY açma ve süreç oluşturma çağrılarının kendisine ayrı zaman sınırı uygulanmadı.

Çocuk `pwsh.exe -NoLogo -NoProfile -Command 'exit 0'` ve `exit 2` sonrası `$LASTEXITCODE` hemen kaydedildi; üç modda sırasıyla `0` ve `2` gözlendi. `[Console]::Write` ile RS ve US üretilmesi denendi; ham okumalarda `1e` ve `1f` görülmedi. Örneğin başarısızlık çıktısında `EXIT_FAILURE=2` için `45 58 49 54 5f 46 41 49 4c 55 52 45 3d 32 0d`, sonraki okumada `0a 5f 5f 46 43 5f 45 4e 44 5f 5f ... 3a 37 3a 32 5f 5f`, ardından `0d 0a` görüldü. Dolayısıyla aday RS/US çerçevesi bu Windows denemesinde doğrulanmadı. Normal çıktı `NORMAL_OUTPUT_42` (`4e 4f 52 4d 41 4c 5f 4f 55 54 50 55 54 5f 34 32`) ve yankılanan sarmalayıcı gözlendi. `Set-Location $env:TEMP` sonrası sonraki komut geçici dizini gösterdi. ANSI dizileri ve parçalı okumalar gözlendi; tüm parçalanma koşulları sınanmadı. Okuma zamanları okuyucu iş parçacığında kaydedildi. Ham kayıtlar geçici `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/` dizinindedir.

**Windows RS/US araştırması — sınırlı deney:** Ayrı geçici `probe.rs` ve `frame.rs` programları `portable-pty` ile 120×24 ConPTY ve `pwsh.exe -NoLogo -NoProfile` kullandı; imleç sorgusuna deneysel terminal yanıtı verildi. `artifact://65` ham çalışma kaydında stdout modu `7`, üç yazma yöntemi için sırasıyla `ABC`, `DEF`, `GHI` çıktıları ve işlenmiş çıktı modu kapatıldığında `4a 1e 4b 1f 4c` görüldü. Bu bulgular `[Console]::Write`, `WriteConsole` ve `WriteFile` ile varsayılan modda RS/US kaybı gözlemiyle uyumludur; ancak mevcut geçici kaynakla kaydın birebir eşleşmesi doğrulanmadı. [Microsoft SetConsoleMode belgesi](https://learn.microsoft.com/en-us/windows/console/setconsolemode) bu bitin çıktı işleme modu olduğunu tanımlar; gözlem, kaybın bu ortamda modla ilişkili olduğuna işaret eder, genel bir nedensellik veya tüm Windows sürümleri için garanti değildir.

`frame.rs` deneyinde çocuk `pwsh` çıkışları `0` ve `2` hemen `$LASTEXITCODE` ile alındı; kaydedilen stdout modunda yalnız çerçeve yazılırken bu bit temizlenip ardından önceki modun geri yüklenmesi çağrıldı. `artifact://69` içindeki ham kayıtta sırasıyla `1e 46 52 41 4d 45 5f 61 33 34 64 3a 37 3a 30 1f` ve aynı dizinin `30` yerine `32` içeren hali görüldü. Bu kısa deney çerçevesi yukarıdaki önerilen tam biçim değildir. Aynı kayıtta ASCII adayları `ASCII_a34d:7:0:END` ve `ASCII_a34d:7:2:END` de görüldü; bu adaylar mod geri yükleme çağrılarından sonra yazıldı, ancak gerçek mod geri okunmadığından normal moda dönüş doğrulanmadı. `GetConsoleMode` ve her iki `SetConsoleMode` çağrısının dönüşleri `frame.rs` içinde kontrol edilmedi; özellikle geri yüklemenin başarılı olduğu kanıtlanmadı. Kayıtta yankılanan komutlar ve terminal dizileri de vardır.


**Sonraki güçlendirilmiş deney:** Geçici `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/src/bin/frame.rs` ve UTF-16 `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/conpty-frame-evidence.log` incelendi. Bu kaynak dosyası yukarıdaki tarihsel kısa çerçeve deneyinden sonra değişmiştir; o deneyin mevcut kaynakla birebir eşleştiği varsayılmamalıdır. Yeni deney `pwsh.exe -NoProfile -NoLogo` ve 24 satır, **5000 sütunlu** ConPTY kullandı; bu sıra dışı genişlik normal terminal boyutlarında eşdeğer davranış göstermez. İmleç sorgusuna deneysel yanıt verilir; komut yankısı ve terminal dizileri hâlâ akıştadır.

Çocuk `pwsh` süreçlerinin `0` ve `2` çıkışları hemen `$LASTEXITCODE` ile alındı. Kaydın ham `READ` baytlarında, tam nonce `0123456789abcdef0123456789abcdef` ve kimlik `7` ile sırasıyla `\x1e__FC_END__:0123456789abcdef0123456789abcdef:7:0__\x1f\n` ve `\x1e__FC_END__:0123456789abcdef0123456789abcdef:7:2__\x1f\n` görüldü. İki normal durumda `GetConsoleMode`, biti kapatan `SetConsoleMode`, `finally` içindeki geri yükleme ve sonraki `GetConsoleMode` başarılı bildirildi; öncesi ve sonrası `7`, karşılaştırma `True` idi. Çerçeve yazıldıktan sonra enjekte edilen PowerShell hatası da yakalandı; `finally` geri yüklemesi ve modun tekrar okunması başarılı bildirildi, mod yine `7` idi ve çıkış kodu `2` korundu. Üç durumun bitiş işaretleri alındı. Bu, önceki deneydeki kontrol edilmemiş geri yükleme çağrısından daha güçlü ama yalnız belirtilen koşullara özgü kanıttır.

API çağrılarının gerçek başarısızlığı, kritik aralıkta zorla öldürme ve eşzamanlı çıktı sınanmadı; enjekte edilen hata API hatası veya süreç ölümü değildir. Deney sürücüsü bitiş işaretlerinden sonra çocuk kabuğa `kill` çağırır; bu, kritik aralıkta öldürme sınaması değildir.

**80×24 tekrar deneyi:** Geçici `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/src/bin/frame80.rs`, `portable-pty` ile `pwsh.exe -NoProfile -NoLogo` için gerçek 80×24 ConPTY açtı ve imleç sorgusuna deneysel yanıt verdi. `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment` dizininde `cargo run --bin frame80 > frame80.log 2>&1` çalıştırıldı. Kaynak ve `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/frame80.log` ham kaydı **geçicidir**; kalıcı kanıt olarak saklanacakları garanti değildir. Mevcut kaynak dosyasının çalıştırma sırasında kullanılan sürümle birebir eşleştiği kanıtlanmamıştır.

- Kaydın 21 ve 35. satırlarındaki `READ` baytlarında, sırasıyla gerçek çocuk `pwsh` süreçlerinin `exit 0` ve `exit 2` değerleri `$LASTEXITCODE` ile hemen alındıktan sonra, nonce `0123456789abcdef0123456789abcdef` ve kimlik `7` ile tam `\x1e__FC_END__:0123456789abcdef0123456789abcdef:7:0__\x1f\n` ve `\x1e__FC_END__:0123456789abcdef0123456789abcdef:7:2__\x1f\n` çerçeveleri görüldü. 49. satırda sentetik hata durumunun `2` kodlu aynı çerçevesi vardır.
- Normal iki durumda ve çerçeveden sonra sentetik PowerShell hatası atılıp yakalanan durumda `GetConsoleMode`, biti kapatan `SetConsoleMode`, `finally` geri yüklemesi ve sonraki `GetConsoleMode` başarılı bildirildi; başlangıç ve son mod `7`, eşitlik `True`, yakalanan çıkış kodları sırasıyla `0`, `2`, `2` idi (21, 35 ve 49–50. satırlar). Bu sentetik hata gerçek bir API hatası veya süreç ölümü değildir. 35 ve 49–50. satırlarda terminal kaydırması ve VT dizileri sonuç metnini böler.
- Çerçeve `finally` içindeki geri yüklemeden önce yayınlanır. Bu nedenle bir tüketici bitiş çerçevesini gördüğünde geri yükleme henüz tamamlanmamış veya başarısız olmuş olabilir; çerçeve tek başına modun geri yüklendiğini göstermez. Deney sürücüsü `DONE` işaretlerini bekler; çerçeveyi veya API durumlarını otomatik doğrulamaz. Yukarıdaki bulgular ham kaydın elle değerlendirilmesine dayanır.
- Komut ve sarmalayıcı yankısı ile VT dizileri akışta görüldü (ör. 10, 16–19 ve 24–33. satırlar); echo kapalı durumu ve eşzamanlı çıktı bu tekrarda sınanmadı. Yankıdaki nonce gizli kabul edilemez. Çerçeve bütünlüğü, eşzamanlı yazma altında garanti edilmiş değildir.
- Her işaret bekleyişine 30 saniye sınırı kondu; PTY açma, süreç oluşturma, `kill` ve `wait` çağrılarının ayrı zaman sınırı yoktu. Okuyucu iş parçacığında zaman damgası alınmadı ve kapanışta okuyucu beklenmediğinden son baytların tamamı kaydedilmiş olmayabilir. Üç bitiş işareti alındıktan sonra `kill` başarılı bildirildi ve `wait` çıkış kodu `1` döndürdü (52–54. satırlar); bu, kritik aralıkta öldürme deneyi veya doğal çıkış kodu değildir.

**80×24 ASCII adayı:** `portable-pty` ve `pwsh.exe -NoProfile -NoLogo` ile geçici `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/src/bin/ascii80.rs` ve `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/ascii80-raw.log` kaydındaki deneyde imleç sorgusuna deneysel yanıt verildi. Gerçek çocuk süreçlerin `exit 0` ve `exit 2` sonuçları hemen `$LASTEXITCODE` ile alındı; sırasıyla `__FC_END__:0123456789abcdef0123456789abcdef:7:0__` ve `...:7:2__` CRLF ile görüldü (ham kayıt 21–22, 30–32). `GetConsoleMode` her iki durumda çocuk öncesinde ve çerçeveden önce başarılı ve `7` idi; çerçeveden sonra mod ölçülmedi, mod değiştirilmedi. Eksik önek ayrıca yazıldı (14); toplam önek sayısı üç, iki tam çerçevenin her birinin sayısı birdi (47–49). Bu sayım tek başına ayrıştırıcının eksik öneği reddettiğini kanıtlamaz. `Set-Location $env:TEMP` sonrası ayrı komut geçici dizini gösterdi (44).

Çerçeve metni kaynakta parçalara ayrılarak oluşturuldu; bu çalışmada işaret bekleyişleri boyunca toplanan ham akışta her tam çerçeve bir kez sayıldı. Sayım `exit` sırasında sonradan okunan baytları kapsamaz. Bu sınırlı echo negatif kontrolüdür: komut yankısı ve VT dizileri vardı; farklı yeniden çizimler veya komut çıktısı eşleşme üretebilir ve aynı PTY'deki komut çerçeveyi taklit edebilir. Sayaç yalnız belirli iki tam diziyi arar; genel çakışma, parçalanma ve güvenlik testi değildir.

**80×24 iki aşamalı deney adayı:** Geçici `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/src/bin/commit80.rs` ve `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/commit80.log` kaydında RS/US adayı, bit kapatılarak yazıldı; ardından başarılı geri yükleme ve eşit mod geri okuması koşuluyla kaynakta bölünerek kurulan ASCII commit yazıldı. Gerçek çocuk çıkışları `0` ve `2` için ham kayıtta aday sonrası sırasıyla `COMMIT_N0_0_OK` ve `COMMIT_N2_2_OK`, başarılı Get/Set/restore/readback ve `7 → 7` görüldü (21, 38–39). Deneyin basit sıralı bayt kontrolü ikisini kabul etti (23, 42). Restore çağrısının **atlanmasıyla simüle edilen** durumda `2` adayı vardı, restore `False`, geri okunan mod `6`, commit yoktu; kontrol adayı reddetti (55–56, 59). Bu gerçek API hatası değildir; ardından deney temizliği modu `7` olarak geri yükleyip okudu. Commit yokluğu yalnız `DONE_FAIL2` işaretine kadar toplanan baytlarda kontrol edildi. İki aşama üretim çözümü veya karar değildir; commit ve aday sahteciliğe dayanıklı değildir, echo ve eşzamanlı çıktı güvenliği kanıtlanmadı.

Her iki kaynak ve ham günlük geçicidir, kalıcı kanıt olarak saklanmaları garanti değildir. ASCII deneyinde her işaret ve doğal çıkış bekleyişi 10 saniyeyle sınırlıydı; doğal çocuk çıkışı `0` görüldü, okuyucu birleştirilmedi ve son baytlar eksik olabilir (53–54). İki aşamalı deneyde işaret bekleyişleri 30 saniye, dış çalıştırma 120 saniyeyle sınırlandı; `kill` başarılı ve `wait` kodu `1` idi, okuyucu birleştirilmedi. Her iki deneyde PTY açma, süreç oluşturma ve temizlik çağrılarının ayrı zaman sınırı yoktu; erken I/O hatalarında temizliğin tamamlanması garanti edilmedi. Gerçek API hatası, kritik anda süreç ölümü, eşzamanlı çıktı ve tüm terminal/okuma parçalanmaları sınanmadı.

Bu sonuçlar üretim doğrulaması değildir. Seçilen Linux RS/US ve Windows ASCII adayları echo, VT, satır sarılması, çakışmalar, parçalanma, eşzamanlı çıktı, süreç kapanışı ve hata yolları için platform kabul testlerinden geçmelidir. Nonce kimlik doğrulamaz; mod değişikliği deneyleri Windows ASCII seçiminden ayrı tarihsel kanıttır.

**Kanıt karşılaştırması ve sınır:** Geçici `D:/pty-linux-experiment/experiment-read-timestamp.log` Linux'ta `0` ve `1`, `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/ascii80-raw.log` Windows ASCII adayında `0` ve `2` gözlemleriyle uyumludur; ASCII çerçevesinden sonra mod ölçülmemiştir. `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/commit80.log` geri yükleme atlanarak yapılan simülasyonda commit yokluğu ve `false` kabul sonucuyla uyumludur. `%LOCALAPPDATA%/Temp/fc-pty-experiment/ascii80-raw.log` değişkeni literal okuma yolunda genişlemedi ve `Path not found` verdi; gerçek yol kullanıldı. Bu kayıtlar geçicidir ve deneylerde kullanılan kaynak sürümüyle birebir eşleşmeleri kanıtlanmamıştır; deneyler yeniden üretilmiş sayılmaz. Linux ve Windows bulguları genel platform garantisi değildir.

Literal `%LOCALAPPDATA%/Temp/fc-pty-experiment/commit80.log` yolu da genişlemedi ve `Path not found` hatası verdi; yukarıda belirtilen gerçek `C:/Users/Blue/AppData/Local/Temp/fc-pty-experiment/commit80.log` yoluna erişildi.

**Adaylar ve karar bağımlılığı:** Windows'ta RS/US mod değiştirmeye bağımlı olabilir; tek çerçeve geri yükleme başarısını kanıtlamaz. Seçilen Windows ASCII adayı sınırlı gözlenmiştir; echo, çakışma ve parçalı akış güvenliği kanıtlanmamıştır. Tarihsel iki aşamalı deney geri yükleme kontrolünden sonra commit kullanır; gerçek API hatası, kritik anda süreç ölümü ve eşzamanlı yazma sınanmamıştır. Seçilen Linux echo kapatma denemesi, Windows echo'nun kapandığını varsaymama, iptal ve belirsiz durumda oturumu durdurma politikaları platformda sınanmalıdır. Durdurma doğrulanamazsa scratch korunur, yolu ve belirsizlik bildirilir. Nonce kimlik doğrulamaz ve ortak PTY'deki arka plan çıktısının gerçek sahibi belirlenemez.

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
| Linux RS/US veya Windows ASCII sentinel çerçevesi birer baytlık okumalarla bölünür | Sınırlı tampon parçaları birleştirir; tam eşleşmede çerçeve çıktıdan ayrılır ve çıkış kodu okunur. Her iki platformda terminal dönüşümleri ve tüm byte sınırları sınanmalıdır. |
| Linux veya Windows çerçevesinde nonce yanlış | Bitiş sayılmaz; baytlar kayıpsız normal çıktıya aktarılır, bunlarda örtüşen sonraki platform önekleri yeniden değerlendirilir. |
| Aynı PTY'deki komut gerçek `session_nonce` ve `cmd_id` ile geçerli çerçeve basar | Bant içi ayrıştırıcı bunu gerçek bitişten ayırt edemez; güvenilmeyen komutlar için işletim sistemi izolasyonu gerekir. |
| Linux `0x1E` veya Windows `__FC_END__:` önekli bozuk, uzun ya da örtüşen aday | Geçersiz veya sınırı aşan aday baytları kayıpsız çıktı olur; Linux RS ve Windows ASCII öneğinin içeride veya örtüşerek başladığı konumlar yeniden değerlendirilir. |
| Kabuk kapanırken (`EOF`) tamponda yarım sentinel adayı kalır | Kısmi aday baytları kaybolmadan `partial_output` sonuna eklenir ve `ShellExited` raporlanır. |
| Tamamlanmış komut `# yorum` ile biter | Araya yeni satır eklenmesi sarmalayıcıyı yorumdan ayırmayı hedefler; kabuk davranışı sınanmalıdır. |
| Kapanmamış tırnak veya heredoc | Sentinel gelmeyebilir; zaman aşımında iptal ve belirsiz kabuk durumu bildirilir; güvenli duruş doğrulanamazsa scratch korunur, yolu bildirilir ve yeni komut başlatılmaz. |
| `exit 42` veya `set -e; false` | Kabuk kapanırsa tamamlanmamış komut hata ve eldeki kısmi çıktı ile raporlanır; EOF davranışı sınanmalıdır. |

### Onaylanan karar ve doğrulama

Linux RS/US, Windows ASCII; Linux'ta echo kapatılması ve Windows'ta echo'nun güvenilir biçimde kapandığı varsayılmadan çerçeveleme seçildi. Otomatik yeni kabukla sessiz kurtarma yerine hata ve güvenli duruş esastır. Her byte sınırında parçalanma, echo, VT, satır sarılması, çakışma, çıkış kodu ve iptal gerçek platformlarda sınanmalıdır. Güvenilmeyen komutların izolasyonu bu sentinel tarafından sağlanmaz.

---

## (c) Çıktı budama (10 KB / 150 satır) ve tam çıktı sahipliği

### Kaynakta kesin olan

- PTY çıktısı `10 KB` veya `150 satır` eşiğini aştığında kesilir; tam çıktı `$FC_SCRATCH/last_output.log` dosyasına yazılır ve model bağlamına yalnızca ilk `50` + son `50` satır aktarılır (ADR-003 Madde 4).
- Bayt birimi (`10_000` vs `10_240`), uzun tek satır davranışı, UTF-8 karakter sınırı ve paralel/ardışık çıktı dosyası sahipliği kaynakta açık bırakılmıştır.

### Öneri ve gerekçe

1. **Eşik ve ölçümler:**
   - Budama, sentinel çıkarıldıktan sonra ham çıktı `10_240` baytı **veya** `150` satırı aştığında yapılır; eşitlikte yapılmaz.
   - Her tamamlanan komutun ham çıktısı geçici dosyaya akıtılır ve başarılı yazım sonrası `last_output.log` olarak yayımlanır.
2. **Bağlam görünümü:** `LF` ayırıcıdır, CRLF tek satır sonudur, sonlandırılmamış son parça bir satır, boş çıktı sıfır satırdır. Mümkünse örtüşmeden ilk ve son 50 satır gösterilir. Kesilme bildirimi dahil yapılandırılabilir başlangıç sınırı **16 KiB**'dır; gerektiğinde uzun satır karakter sınırında kesilir. Geçersiz UTF-8 kayıplı ve açıkça belirtilmiş gösterilir; ham kayıt değişmez. Kullanıcı ADR-003'ün tam satır yorumuna bu istisnayı açıkça onayladı; kaynak ADR değiştirilmez. Bayt sınırı token garantisi değildir.
3. **Tam çıktı sahipliği:** Tek PTY için komutlar seri yürütülür ve tek okuyucu/yazıcı kullanılır. Alınan baytlar komut aralığına göre geçici dosyaya yazılır; başarılı tamamlanma ve dosya yazımı sonrası `last_output.log` yayımlanır. Ortak PTY'de arka plan baytlarının gerçek üreticisi ayırt edilemez; kayıt zaman aralığına aittir. Sonradan gelen baytlar yayımlanmış dosyayı değiştirmez.
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

Model bağlamına giden sınırlı görünüm (bu örnekte satır görünüm sınırını aşıyor):
  <satırın karakter sınırında kesilmiş başı>
  [... ferrocore: çıktı kesildi; tam ham çıktı: $FC_SCRATCH/last_output.log ...]
  <satırın karakter sınırında kesilmiş sonu>
Tam satır modele aktarılmaz; tamamı ham kayıtta saklanır. Başlangıç görünüm sınırı bildirim dahil 16 KiB'dır.
```

### Sınır durumları ve beklenen sonuçlar

| Sınır durumu | Beklenen sonuç / karar durumu |
| --- | --- |
| 80 satır, 15 KB | İlk ve son 50 örtüşür; satırlar tekrarlanmaz. Görünüm sınırı aşılırsa karakter sınırında baş/son gösterimi ve kesilme bildirimi kullanılır; tam ham çıktı saklanır. |
| Çok baytlı UTF-8 karakter sınırda | Görünüm karakter ortasından kesilmez; ham kayıt korunur. |
| İkili veya geçersiz UTF-8 | Ham kayıt baytları korur; model görünümü kayıplı dönüştürülür ve dönüşüm açıkça bildirilir. |
| Arka plan işi aktif komut sırasında yazar | Ortak PTY'de kaynağı belirlenemez; aralık kaydına karışabilir ve bu sınırlama bildirilir. |
| Disk dolu veya yazma başarısız | Kısmi dosya tam çıktı sayılmaz; hata döner ve önceki başarılı kayıt korunmaya çalışılır. |
| Windows'ta eşzamanlı okuma ve değiştirme | Atomiklik garanti edilmez; API ve dosya sistemi için platform kanıtı gerekir. |

### Onaylanan karar ve doğrulama

10.240 bayt/150 satır aşımı, LF sayımı, her tamamlanan komutun ham kaydı ve 16 KiB sınırlı görünüm seçildi; ADR-003 tam satır istisnası onaylandı. Disk ve I/O hatalarında kısmi dosya tam veya yeni çıktı gibi sunulmaz; önceki dosya korunmaya çalışılır. Windows'ta eşzamanlı okur atomikliği platform ve dosya sistemi üzerinde sınanmadan garanti edilmez.

---

## (d) LLM sağlayıcı, akış protokolü, kimlik bilgileri, retry, bütçe ve iptal

### Kaynakta kesin olan

- `ferrocore-driver` içindeki ortak `TurnDriver`; LLM yanıt akışı (streaming), üstel geri çekilmeli yeniden deneme (exponential backoff retry), token bütçesi ve maliyet sayacı (cost guard) ile `Ctrl+C` (`SIGINT`) ve zaman aşımı iptal yönetiminden sorumludur (ADR-008, `docs/architecture.md`).
- `ferrocord` çekirdeği AI, LLM ve prompt kavramlarını içermez; bu mantık tamamen `ferrocore` katmanında kalır (ADR-001).
- ADR'de örnek modeller olarak OpenAI ve DeepSeek ailesi (`deepseek-chat`, `deepseek-reasoner`, `o3`) anılır; ancak kesin HTTP/akış protokolü, kimlik bilgisi kaynağı, retry kapsamı ve üst sınırı, token/maliyet hesaplama yöntemi ve iptal davranışı kaynakta tanımlanmamıştır (`docs/open-questions.md`).

### Öneri ve gerekçe

1. **Sağlayıcı ve akış protokolü:**
   - Faz 2 için yapılandırılabilir **Chat Completions SSE** profili seçildi; `base_url` ve `model` yapılandırılır. Hedef sağlayıcı ve modelle gerçek uyumluluk sınanmalıdır; bütün sağlayıcıların uyumu varsayılmaz.
   - SSE UTF-8, parçalı giriş, satır sonları, yorumlar ve çoklu `data` satırlarını ele alır. `data: [DONE]` genel SSE standardı değil sağlayıcı sözleşmesidir; hedef sağlayıcı ile doğrulanmalıdır.
   - **Açık kanıt ihtiyacı:** Hedef sağlayıcının SSE sonunda `usage` nesnesini her durumda (özellikle hata/erken kesme dışında normal bitişte) gönderip göndermediği gerçek uç noktaya karşı doğrulanmalıdır. Yanıtta `usage` yoksa sürücü uydurma kesin token sayısı üretmez; kullanımı `UsageStatus::Unavailable` olarak kaydeder.
2. **Kimlik bilgisi yönetimi:**
   - Gizli API anahtarı `config.toml` içine düz metin yazılmaz; yapılandırmada yalnızca ortam değişkeninin adı (`api_key_env = "FERROCORE_API_KEY"`) belirtilir.
   - Anahtar oturum başında okunur; hata mesajlarında, `Debug` çıktılarında ve `session.jsonl` kayıtlarında asla yer almaz.
   - Anahtarın ortam değişkeni çocuk PTY sürecine aktarılmaz; bu diğer gizli kaynaklara erişimi engelleyen sandbox değildir.
3. **Retry kapsamı ve üst sınırı:**
   - Yalnızca ilk içerik parçası (`delta`) tüketilmeden önce geçici ağ hataları veya HTTP `408`, `429`, `500`, `502`, `503`, `504` yeniden denenebilir. Sağlayıcı isteği işlemiş olabilir; delta gelmemesi maliyetsiz veya yinelenmemiş işlem garantisi değildir.
   - Kalıcı istemci, şema veya yetki hatalarında (`400`, `401`, `403`, `404`, `422`) yeniden deneme yapılmaz; doğrudan yapılandırılmış hata döner.
   - Yapılandırılabilir başlangıç üst sınırı: en fazla `3` yeniden deneme (toplam `4` deneme), `500 ms` taban bekleme, `2x` üstel artış, `8 s` tavan ve rastgele jitter (`Retry-After` başlığı varsa ve kalan zaman aşımını aşmıyorsa dikkate alınır).
   - SSE akışı başladıktan ve ilk `delta` döngüye/kayda iletildikten sonra bağlantı koparsa, yarım yanıtın üzerine sessiz otomatik tekrar yapılmaz; kısmi içerik kaydedilip `DriverError::StreamInterrupted` döndürülür.
4. **Token bütçesi ve maliyet sayacı:**
   - Birincil bütçe kapısı `max_total_tokens` (`prompt_tokens + completion_tokens`) ve `max_turns` sınırlarıdır.
   - Maliyet sınırı (`max_cost_micros_usd`) yalnızca kullanıcı `config.toml` içinde `input_cost_per_million_micros` ve `output_cost_per_million_micros` değerlerini açıkça tanımladıysa hesaplanır; sağlayıcı fiyatları değişken olduğu için koda gömülü sabit fiyat listesi uydurulmaz.
   - Her tur öncesi kümülatif sayaç kontrol edilir; tur sonundaki kullanım eklenir. Limit aşılmışsa veya zorunlu kullanım bilgisi yoksa yanıt kaydedilir fakat içindeki komut yürütülmeden oturum durur. Sayaç yalnız tur sonunda kesinleştiğinden istek limitleri aşabilir; yanıt bayt tavanı token ya da maliyet için kesin sınır değildir. Kesin sınır için sağlayıcının token limiti ve fiyatlandırma kanıtı gerekir.
   - Yapılandırılabilir başlangıç değerleri `max_turns = 20`, `max_total_tokens = 50_000`, `max_response_bytes = 1 MiB`'dır; ölçülmüş optimum veya kesin fatura sınırı değildir. Kullanıcı fiyatları yapılandırır; bilinmeyen ücretler kesin maliyet diye sunulmaz.
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
| Sağlayıcı `usage` vermez | Zorunlu token veya maliyet bütçesinde `UsageUnavailable` ile durulur, yanıt kaydedilir ve komut yürütülmez. |
| Retry beklemesi veya aktif SSE sırasında kullanıcı `Ctrl+C` basar | Bekleme/istek iptal edilir, yeni deneme açılmaz, `Cancelled` kaydedilir; ilgili süreçler durduysa scratch temizlenmeye çalışılır, doğrulanamazsa korunur. Windows davranışı sınanmalıdır. |
| Aktif komut iptalden sonra çalışmayı sürdürür veya durdurma doğrulanamaz | İptal tamamlandı diye sunulmaz; hata ve belirsiz süreç durumu bildirilir, scratch korunur ve yolu bildirilir. |

### Onaylanan karar ve doğrulama

Chat Completions SSE, ortamdan anahtar, delta öncesi seçili geçici hatalarda 3 retry / 500 ms / 2× / 8 s ve jitter, kısmi içerikten sonra tekrar yapmama, zorunlu kullanım eksikliğinde durma ve ortak iptal seçildi. `Retry-After` ve kalan süre dikkate alınır; önceki deneme ücret doğurmuş olabilir. Adım ve oturum sürelerinin sayısal değerleri burada seçilmedi. Yerel HTTP iptali sağlayıcının üretimi veya ücretlendirmeyi durdurduğunu kanıtlamaz. Sağlayıcı/model uyumu ve süreç ağacı platformlarda doğrulanmalıdır.

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

### Onaylanan karar ve doğrulama

`types` I/O'suz sözleşmeleri, `TurnDriver` LLM/bütçe/iptal/kayıt sahipliğini, döngü komut/bitiş/biçim hatası üretimini üstlenir; `Tool` asgari soyut sözleşmedir. PTY arayüzü yürütme ve canlı durum görüntüsünü sağlar; bilinmeyen değerler açıkça gösterilir. Kesin async imzalar ve trait nesnesi uyumu, `ferrocord` kaynak API'si ve derleme ile doğrulanmalıdır.

---

## (f) `session.jsonl` veri şeması, ebeveyn ağacı ve tek yazıcı sahipliği

### Kaynakta kesin olan

- Hem ana ajan adımları hem de ileride dallanacak alt ajanların geçmişi tek bir append-only `session.jsonl` dosyasında ağaç/DAG yapısını koruyarak saklanır (ADR-007).
- Her JSONL satırı şu altı alanı içerir: `id`, `parent_id`, `agent_depth`, `loop_type`, `timestamp`, `payload` (ADR-007, `docs/architecture.md`, `AGENTS.md`).
- Alanların kesin veri tipleri, kök kaydın `parent_id` gösterimi, kimlik üretimi, sürümleme, atomiklik ve eşzamanlı yazma sahipliği kaynakta açık bırakılmıştır (`docs/open-questions.md`).

### Öneri ve gerekçe

1. **Sabit üst seviye alanlar ve tipleri:**
   - ADR-007'deki 6 alan değiştirilmeden ve üst seviyeye fazladan alan eklenmeden şu tipler önerilir:
     - `id`: `String` — oturum içinde benzersiz UUIDv7 kayıt kimliği; rastgele çakışma olasılığı sıfır değildir ve yinelenen kimlik reddedilmelidir.
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
   - Her kayıt tamponlanıp sırayla yazılır ve `flush` edilir; başlangıç, kapanış ve her tur sonunda `sync_data` seçildi. Kısmi yazma ve eşitleme hataları bildirilir; `flush` veya `sync_data` güç kaybına karşı mutlak garanti değildir.
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

### Onaylanan karar ve doğrulama

UUIDv7, yalnız kökte null ebeveyn, altı alan ve `payload` v1 zarfı, sınırlı kuyruklu tek yazıcı ve başlangıç/kapanış ile her tur sonunda `sync_data` seçildi. Yazma hatası yeni yan etkileri durdurur; kısmi satır raporlanır ve sessizce silinmez. UUID sırası nedensellik kanıtı değildir; ebeveyn ve dosya sırası esastır. Niyet kaydı yürütme veya güç kaybına dayanıklılık kanıtı değildir; kayıt izinleri ve redaksiyon uygulamada netleştirilmelidir.

---

## (g) RLM ham kod bloğu ayrıştırması, canlı başlık ve hatalı yanıt davranışı

### Kaynakta kesin olan

- ADR-008'e göre `ferrocore-loop-rlm`, JSON Function Calling kaçış hatalarından kaçınmak için modelden doğrudan ham kabuk kod blokları alır ve bunları PTY'ye aktarır.
- ADR-008'de Markdown `bash` bloğu ve `<sh>...</sh>` örnek biçimler olarak verilmiştir; kesin etiket sözleşmesi, çoklu blok kuralı, tamamlanma bildirimi ve bozuk model yanıtı davranışı kaynakta açık bırakılmıştır (`docs/architecture.md`, `docs/roadmap.md`).
- RLM döngüsü her adımda kabuğun canlı durumunu (`PWD`, son çıkış kodu `$?`, arka plan işleri `jobs` ve `$FC_SCRATCH` içeriği) bağlama dinamik olarak ekler (ADR-008). Prime Agent kaynakta ilham olarak anılır; depoda uygulanmış RLM kodu yoktur.

### Öneri ve gerekçe

1. **Kanonik eylem ve tamamlanma etiketleri:**
   - Yürütülebilir biçim olarak kendi satırında ve sıfır girintili **`<sh>\n...\n</sh>`**, bitiş için **`<final>\n...\n</final>`** seçildi. Açıklama içindeki etiketler de ayırt edilemeyebilir; model uyumu sınanmalıdır.
   - Etiket dışındaki düz açıklama metni JSONL kaydında saklanır, PTY'ye gönderilmez. Eylem etiketleri yalnız Markdown kod çitleri dışında tanınır; çitin içindeki `<sh>` ve `<final>` örnekleri çalıştırılmaz. Kapanmamış veya belirsiz çitte hata verilir, eylem yürütülmez.
   - **Etiket kaçış kuralı:** Komut gövdesinde literal `</sh>` metni geçmesi gerekiyorsa satır içi kullanımlar (ör. `echo "</sh>"`) kapanış sayılmaz; yalnızca başında/sonunda boşluk olmadan tek başına satır oluşturan `\n</sh>\n` (veya yanıt sonundaki `\n</sh>`) kapanış kabul edilir.
   - Tek başına `</sh>` satırı gerekiyorsa modele kabukta eşdeğer farklı yazım üretmesi söylenir; ayrıştırıcı kodu sessizce değiştirmez.
   - **Açık kanıt ihtiyacı:** Hedef modelin (`deepseek-chat` vb.) sistem istemiyle `<sh>...</sh>` ve `<final>...</final>` biçimine uyum oranı bu depoda henüz ölçülmemiştir; uygulama başında küçük bir istem doğrulama setiyle sınanmalıdır.
2. **Tur başına tek eylem kuralı:**
   - Kalıcı kabukta bir komutun çıktısı ve çıkış kodu bir sonraki komutu doğrudan etkilediği için her turda **yalnızca tek bir `<sh>...</sh>` bloğu** veya **tek bir `<final>...</final>` bloğu** geçerli kabul edilir.
   - Aynı yanıtta birden fazla `<sh>` bloğu ya da hem `<sh>` hem `<final>` bulunursa **hiçbir komut PTY'ye gönderilmez**; modele biçim hatası bildirilir.
3. **Canlı durum başlığı (Stateful Header) üretimi:**
   - Her PTY adımından sonra `PWD`, son `exit_code`, `jobs` ve scratch dosyaları durumu eklenir; bilinmeyen ve son bilinen değerler ayrılır. Scratch için sıralı ilk `20` giriş ve toplam sayı yapılandırılabilir başlangıçtır. Başlık sorgusunun `$?` ve kabuk durumunu koruması platformda sınanmalıdır.
   - Kabuk süreci kapanmışsa (`ShellExited`) canlı kabuk sorgusu çalıştırılamaz; başlıkta `shell_status: exited` ve son bilinen değerler gösterilir.
4. **Hatalı ve yarım model yanıtı davranışı:**
   - Şu durumlar **hatalı model yanıtı (`MalformedModelOutput`)** sayılır: (a) `<sh>` açılıp `</sh>` ile kapanmaması (ör. `max_tokens` nedeniyle kesilmiş yanıt), (b) yalnızca boşluk içeren `<sh></sh>` bloğu, (c) yanıtta ne `<sh>` ne `<final>` bulunması, (d) aynı yanıtta birden fazla eylem/bitiş bloğu bulunması.
   - Yanıt bütünü ve bitiş nedeni kontrol edilmeden, yarım veya belirsiz bloktan hiçbir eylem çalıştırılmaz.
   - Bozuk yanıtta en fazla `2` ardışık biçim düzeltme turu uygulanır; bunlar bütçeye ve kayda dahildir. Sınır aşılırsa `StopReason::MalformedModelOutput` ile durulur.

### Önemli bedel

Yalnız `<sh>` kabul etmek Markdown çiti üreten modellerde düzeltme turu ve token maliyeti doğurabilir. Açıklama içindeki etiketlerin de eylem sayılması riski tamamen ortadan kalkmaz; ayrıştırma kuralları sınanmalıdır.

### Örnek girdi / çıktı

```text
Örnek 1 — Geçerli RLM turu (aşağıdaki model yanıtı satırları gerçek sıfır girintili girdidir):
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

Örnek 2 — Kapanmamış model yanıtı (gerçek sıfır girintili girdi):
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

### Onaylanan karar ve doğrulama

Sıfır girintili tek `sh` veya `final`, Markdown çitlerinin dışlanması, yarım/bloke yanıtın yürütülmemesi, iki düzeltme turu ve sıralı ilk 20 scratch girdisi seçildi. Model uyumu ve canlı durum sorgusunun kabuk durumunu koruması doğrulanmalıdır.

## Faz 2 karar paketi

Kullanıcı aşağıdaki seçimleri onayladı; sayılar yapılandırılabilir başlangıç değerleri, kanıt ihtiyaçları ise devam eden kabul koşullarıdır. Onay uygulamanın veya platform garantisinin kanıtı değildir.

| Konu | Onaylanan seçim | Kalan kanıt ve sınırlar |
| --- | --- | --- |
| (a) Kabuk ve scratch | `bash --noprofile --norc -i`, `pwsh -NoLogo -NoProfile`, 5 sn; `/dev/shm` → `/tmp`; belirsiz duruşta scratch korunur | Windows ACL, süreç ağacı, temizleme doğrulanmalı |
| (b) Sentinel | Güvenilir komut varsayımı; Linux RS/US ve Windows ASCII, tek okuyucu, Linux echo kapatma denemesi | Her iki aday platform kabul testlerine bağlı; nonce kimlik doğrulamaz, echo ve VT sınanmalı |
| (c) Çıktı | >10240 bayt veya >150 satır, LF; 50+50 ve 16 KiB görünüm; her komutta ham dosya; ADR-003 istisnası onaylı | Windows atomikliği ve I/O; arka plan çıktısı sahibinin bilinmemesi |
| (d) LLM ve iptal | Chat Completions SSE; 3 retry/500 ms/2×/8 s+jitter; usage yokluğunda duruş; 20 tur/50000 token/1 MiB örnekleri; ortak iptal | Sağlayıcı uyumu, ücretler ve süreç ağacı doğrulanmalı |
| (e) API | I/O'suz types, ortak driver, eylem üreten loop ve PTY durum arayüzü | Trait imzaları ve ferrocord uyumu doğrulanmalı |
| (f) JSONL | Altı alan, UUIDv7, tek yazıcı, her tur `sync_data` | Güç kaybı ve tam bir kez yürütme garantisi yok |
| (g) RLM | Sıfır girintili tek sh/final, iki düzeltme, canlı başlık ilk 20 | Model uyumu ve canlı durum sorgusu sınanmalı |

Bağımlılıklar karşılıklı olabilir; tablo çözüm gerektiren sınırları gösterir, kesin uygulama sırası değildir. Faz 2 dışındaki spawn/IPC ve eklenti sözleşmeleri bu paketle karara bağlanmaz.
