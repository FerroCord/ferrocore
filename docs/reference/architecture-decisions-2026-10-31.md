# **Ferrocore & Ferrocord: Sistem Mimarisi ve Karar Kayıtları (ADR)**

Bu doküman; **DeepSeek Harness**'ın modüler mikro-çekirdek felsefesi ile **Prime Agent**'ın özyinelemeli (RLM) kabuk çalışma zamanını Rust ekosisteminde birleştiren **Ferrocore** (ajan omurgası) ve **Ferrocord** (bağımsız mikro-çekirdek) projelerinin mimari kararlarını, teknik denetim (audit) düzeltmelerini, döngü (loop) topolojisini ve isim alternatiflerini içerir.

## **1\. Proje Kimliği ve İkili Ekosistem Felsefesi**

Sistem, sorumlulukların fiziksel olarak ayrılması (Separation of Concerns) ilkesiyle iki temel katmana bölünmüştür:

> 1. **ferrocord (Bağımsız Mikro-Çekirdek Kütüphanesi):**  
   * **Rolü:** DeepSeek Harness'ın altındaki *Cordis* framework'ünün Rust karşılığıdır.  
   * **Kapsamı:** İçinde "AI", "LLM", "Prompt" veya "Terminal" kavramları **bulunmaz**. Yalnızca tip-güvenli Servis Konteyneri (TypeMap), Jenerik Olay Yolu (Typed EventBus) ve Eklenti Yaşam Döngüsü (Plugin Trait) barındıran genel amaçlı bir Rust mikro-çekirdeğidir.  
> 2. **ferrocore (Otonom Ajan Omurgası / Harness):**  
   * **Rolü:** ferrocord çekirdeği üzerine inşa edilen; kalıcı terminal (PTY), Prime Agent (RLM) döngüsü, ReAct döngüsü, ileri seviye ajan döngüleri ve araç (MCP/WASM) yöneticisini barındıran ana ajan ürünüdür.  
   * **Hedefi:** Geliştiricinin makinesinde Node.js veya Python bağımlılığı gerektirmeyen, **gerçek anlamda** tek bir ikili dosya (single binary) olarak dağıtılan "Everything is a Plugin" mimarili yazılım geliştirme ajanı.

### **1.1. Genişletilmiş Alternatif İsim Havuzu**

| İsim | Kavramsal Odak | Terminal Ergonomisi (CLI) | Crates.io / Marka Özgünlüğü |
| :---- | :---- | :---- | :---- |
| **Ferrocore \+ Ferrocord** | Rust \+ Çekirdek \+ Cordis | ferrocore run (Güçlü) | Çekirdek (ferrocord) ve Ajan (ferrocore) ayrımını en iyi yansıtan ikili. |
| **Oxiharness** | DS Harness \+ Rust | oxiharness dev (Net) | Açık kaynakta DS Harness'ın resmi Rust portu intibası verir. |
| **Carapace** | Koruyucu Kabuk (Shell \+ Rust) | carapace exec (Çok doğal) | Popüler bir shell autocompletion kütüphanesi ile çakışabilir. |
| **Conch** | POSIX Shell \+ Otorite Kabuğu | conch run "görev" (Hafif) | Özgün, akılda kalıcı hikayesi olan bağımsız bir ürün hissi. |
| **Mitosis** | Recursive Alt Ajanlar | mitosis spawn (Doğal) | Ajan literatüründe taze ve ayırt edici bir biyolojik metafor. |
| **Sidero** | Antik Demir / Rust | sidero \--daemon (Kurumsal) | Temiz, karizmatik ve Rust dünyasına doğrudan göz kırpan isim. |
| **sharn** | Shell \+ Harness | sharn exec "..." (5 Harf) | Unix araç setine yakışır sadelikte. |

## **2\. Mimari Karar Kayıtları (Architecture Decision Records)**

### **ADR-001: Çekirdek Tasarımı — Bağımsız ferrocord Mikro-Çekirdek Modeli**

> * **Durum:** Kabul Edildi (Denetim Sonrası Güncellendi)  
> * **Bağlam:** DeepSeek Harness, TypeScript tabanlı Cordis framework'ü üzerine kuruludur. Cordis; Servis Konteyneri (IoC / Dependency Injection), Olay Yolu (Event Bus) ve Yaşam Döngüsü (Lifecycle) yönetimini sağlar. Resmi bir Rust portu bulunmamaktadır.  
> * **Karar:**  
  * Bu çekirdek **ferrocord** adıyla **ayrı ve bağımsız bir GitHub reposu / crate** olarak inşa edilecektir.  
  * **Senkron Kilit ve Arc\<T\> Tasarımı:** .await noktaları arasında kilit tutulmasından kaynaklanabilecek kilitlenmeleri (deadlock) önlemek için tokio::sync::RwLock yerine std::sync::RwLock\<HashMap\<TypeId, Arc\<dyn Any \+ Send \+ Sync\>\>\> (veya DashMap) kullanılacaktır. Servis okumaları yalnızca referans sayacını (Arc::clone) artırıp kilidi nanosaniyeler içinde bırakacaktır.  
  * **Jenerik Olay Yolu (Typed EventBus):** ferrocord ajan domain'ini bilmediği için sabit bir Event enum'ı barındırmaz; her olay tipinin (E: 'static \+ Send \+ Sync \+ Clone) kendi broadcast::Sender\<E\> kanalına sahip olduğu tip-tabanlı bir olay yolu sunar.

### **ADR-002: Yürütme Motoru — Kalıcı Shell (PTY) ve Sentinel Protokolü**

> * **Durum:** Kabul Edildi (Denetim Sonrası Güncellendi)  
> * **Karar:**  
  * Yürütme motoru olarak portable-pty kütüphanesi ile yönetilen **Kalıcı Shell (Terminal / PTY)** seçilmiştir.  
  * **Komut Bitiş ve Çıkış Kodu Algılama (Sentinel Framing):** Kalıcı bir bash/zsh süreci tekil komut bittiğinde kapanmadığı (EOF göndermediği) için, çalıştırılan her komutun sonuna çekirdek tarafından görünmez bir çerçeveleme komutu eklenir:  
    ![][image1]  
  * Stream Interceptor bu sentinel dizisini yakaladığı anda komutun tamamlandığını ve kesin çıkış kodunu (exit\_code) deterministik olarak anlar.

### **ADR-003: PTY Zayıflıklarının POSIX Primitifleriyle Çözümlenmesi**

> * **Durum:** Kabul Edildi (Denetim Sonrası Güncellendi)  
> * **Karar ve Çözüm Mekanizmaları:**  
  1. **Taşınabilir Bellek Diski (\$FC\_SCRATCH):** Sadece Linux'ta bulunan /dev/shm yoluna doğrudan bağımlılık kaldırılmıştır. Başlangıçta işletim sistemi kontrol edilerek Linux'ta /dev/shm/ferrocore\_{id}, macOS ve diğer POSIX sistemlerde /tmp/ferrocore\_{id} otomatik seçilir ve PTY içine \$FC\_SCRATCH ortam değişkeni olarak enjekte edilir.  
  2. **Tek-Binary Alt Ajan Dağıtımı (ferrocore internal-spawn):** Dağıtım bütünlüğünü korumak adına ikinci bir binary dosyası derlenmez. Ana ferrocore binary'si BusyBox/Multicall desenini uygular; PTY içindeki $PATHdizininespawn-agentadıylasemboliklink(veya2satırlıkshellwrapper:execferrocoreinternal-spawn"$@") bırakılır.  
  3. **Kilitlenmesiz (Deadlock-Free) Özyineleme Kotası:** Üst ajanların alt ajanları beklerken tüm global semafor izinlerini tüketip sistemi kilitlemesini (waiting deadlock) önlemek için izinler केवल **aktif LLM/Komut yürütmesi** sırasında tutulur (await child\_agent beklemesi sırasında izin serbest bırakılır) ve her derinlik seviyesi (![][image2]) kendi bağımsız eşzamanlılık kotasına sahip olur.  
  4. **Bağlam Filtreleme (Context Pruning):** PTY stdout akışı 10 KB veya 150 satırı aştığında kesilir, tam çıktı \$FC\_SCRATCH/last\_output.log dosyasına yazılır ve modele sadece ilk 50 \+ son 50 satır iletilir.

### **ADR-004: Değiştirilebilir Döngü Sözleşmesi (AgentLoop Trait)**

> * **Durum:** Kabul Edildi  
> * **Karar:** ferrocore, ferrocord üzerine jenerik bir AgentLoop servis trait'i tanımlar. Aktif döngü config.toml içindeki active\_loop anahtarıyla seçilir veya çalışma zamanında alt ajanlara görev türüne göre farklı döngü tipleri atanabilir.

### **ADR-005: Kullanıcı Düzeyi Eklenti Desteği (IPC \+ WASM Birlikteliği)**

> * **Durum:** Kabul Edildi  
> * **Karar:** Ortak Tool trait'i arkasında hem **Dış Süreç / IPC (MCP Uyumlu JSON-RPC)** hem de **WebAssembly (Extism / Wasmtime)** adaptörleri aynı anda desteklenir.

### **ADR-006: Repo Topolojisi ve CI/CD Mimarisi — Katmanlı Hibrit Model (2 \+ N Repo)**

> * **Durum:** Kabul Edildi  
> * **Karar:**  
  * **REPO 1 (ferrocord):** Bağımsız mikro-çekirdek crate'i.  
  * **REPO 2 (ferrocore):** Birinci parti ajan crate'lerini barındıran Cargo Workspace monoreposu.  
  * **REPO 3..N (ferrocore-plugin-\*):** Harici MCP ve WASM eklenti repoları.

### **ADR-007: Oturum ve Yörünge Depolama Modeli (Append-Only JSONL Tree)**

> * **Durum:** Kabul Edildi  
> * **Karar:** Hem ana ajan adımları hem de spawn-agent ile dallanan alt ajanların geçmişi, tek bir dosyada ağaç (DAG/Tree) yapısını koruyan **Append-Only JSONL** (session.jsonl) formatında saklanır. Her satır id, parent\_id, agent\_depth, loop\_type, timestamp ve payload alanlarını içerir.

### **ADR-008: Döngü Mimarisi İlişkisi (Ayrı mı, Birbiri Üzerine mi?) ve Genişletilebilir Döngü Paradigmaları**

> * **Durum:** Kabul Edildi  
> * **Bağlam:** ferrocore-loop-rlm (Prime Agent tarzı kalıcı PTY döngüsü) ile ferrocore-loop-react (Standart çoklu-araç JSON Function Calling döngüsü) inşa edilirken şu temel mimari soru ortaya çıkmıştır: *Bu iki döngü tamamen ayrı mı inşa edilmeli, yoksa RLM döngüsü ReAct döngüsünün üzerine ("sadece tek bir bash aracına sahip bir ReAct") olarak mı kurulmalı?*

#### **1\. Mimari Analiz: Neden Biri Diğeri Üzerine Kurulmamalı?**

İlk bakışta RLM'i, ReAct döngüsünün sadece tek bir pty\_bash aracına sahip özel bir konfigürasyonu gibi (RLM extends ReAct) inşa etmek kod tekrarını azaltacak cazip bir kısayol gibi görünür. Ancak bu yaklaşım **3 ciddi mühendislik tuzağı** barındırır:

> 1. **JSON Kaçış Karakteri Cehennemi (JSON Escaping Hell):**  
   * ReAct döngüsü, OpenAI/DeepSeek standart tools (JSON Schema Function Calling) protokolünü kullanır. Model her komutu {"command": "..."} JSON string'i içine sığdırmak zorundadır.  
   * Model terminalde çok satırlı bir Rust/Python kodu, karmaşık bir sed/awk regex'i veya cat \<\< 'EOF' bloğu yazarken çift tırnak ("), ters eğik çizgi (\\) ve yeni satır (\\n) kaçış karakterlerinde sık sık JSON parse hatası yapar ve jeton (token) israf eder.  
   * **RLM (ve CodeAct) Doğası:** RLM döngüsünde model JSON şemasına hapsolmaz; yanıtında doğrudan ham kod blokları (bash ... veya \<sh\>...\</sh\>) üretir. Ayrıştırıcı (parser) bunu sıfır JSON serileştirme hatasıyla doğrudan PTY'ye akıtır.  
> 2. **Dinamik Durum Başlığı Enjeksiyonu (Stateful Prompt Header):**  
   * ReAct döngüsü geçmişe yalnızca statik role: "tool" mesajları ekler.  
   * RLM döngüsü ise her adımda kabuğun canlı durumunu (PWD, son çıkış kodu \$?, aktif arka plan işleri jobs, \$FC\_SCRATCH içindeki anahtarlar) sistem/kullanıcı çerçevesine dinamik olarak enjekte eder.  
> 3. **Doğrudan Akış Kesme (Stream Interception):**  
   * ReAct ayrık (discrete) fonksiyon çağrıları beklerken, RLM doğrudan sürekli akan bir PTY byte akışıyla (ve içindeki spawn-agent UDS kesmeleriyle) senkronize çalışır.

#### **2\. Doğru Mimari Model: Ortak TurnDriver Altyapısı \+ Kardeş (Sibling) Döngü Eklentileri**

Ne tamamen sıfırdan kopyala-yapıştır (DRY ihlali) yapılmalı, ne de RLM zorla ReAct'in içine sıkıştırılmalıdır. Doğru çözüm **İki Katmanlı Kardeş Eklenti (Shared Driver \+ Sibling Loops)** mimarisidir:  
                ┌──────────────────────────────────────────────────┐  
                │          ferrocore-driver (Ortak Altyapı)        │  
                │  • LLM Streaming & Retry / Exponential Backoff   │  
                │  • Token Bütçesi & Maliyet Sayacı (Cost Guard)   │  
                │  • JSONL Trajectory Kaydedici (Session Logger)   │  
                │  • Ctrl+C (SIGINT) ve Timeout İptal Yönetimi     │  
                └────────────────────────┬─────────────────────────┘  
                                         │ (Ortak Primitifleri Kullanır)  
        ┌────────────────────────────────┼────────────────────────────────┐  
        ▼                                ▼                                ▼  
┌───────────────────────┐    ┌───────────────────────┐    ┌───────────────────────┐  
│  ferrocore-loop-rlm   │    │ ferrocore-loop-react  │    │ ferrocore-loop-\<yeni\> │  
│  (Kardeş Eklenti 1\)   │    │  (Kardeş Eklenti 2\)   │    │  (Kardeş Eklenti N)   │  
│                       │    │                       │    │                       │  
│ • Raw Block Parser    │    │ • JSON Schema Builder │    │ • Özel Durum Makinesi │  
│ • Canlı PTY Bağlamı   │◄──►│ • ToolRegistry Çağrısı│    │   veya AğaçArama      │  
│ • UDS Spawn Köprüsü   │    │ • Paralel MCP/WASM    │    │   Algoritması         │  
└───────────────────────┘    └───────────────────────┘    └───────────────────────┘

> * **Çapraz Döngü Çağrısı (Cross-Loop Spawning):** Her iki döngü de aynı AgentLoop trait'ini uyguladığı için **birbiriyle yardımlaşabilir**. Örneğin; ana ajan loop-rlm (terminal) modunda çalışırken, yapılandırılmış bir Jira/Postgres MCP sorgusu gerektiğinde spawn-agent \--loop react "Jira'daki açık bugları getir" diyerek alt görevi ReAct döngüsüne devredebilir (veya tam tersi).

## **3\. Sisteme Eklenebilecek Diğer Ajan Döngü (ferrocore-loop-\*) Yapıları**

AgentLoop bir eklenti sözleşmesi olduğu için, RLM ve ReAct dışında modern yapay zekâ mühendisliğindeki en güçlü 5 döngü paradigması da bağımsız sandıklar (crates) olarak sisteme eklenebilir:

### **3.1. ferrocore-loop-plan (Architect / Editor & Plan-and-Solve Döngüsü)**

> * **İlham Kaynağı:** Aider (Architect/Editor modu), LangGraph Plan-and-Execute.  
> * **Çalışma Mantığı:** İki farklı modeli veya iki farklı aşamayı sırayla koşturur:  
  1. **Architect (Planlayıcı) Aşaması:** Akıl yürütme gücü yüksek model (örn. deepseek-reasoner / o3) kod tabanını salt-okunur (read-only) inceler, hiçbir dosyayı değiştirmez ve deterministik bir **Görev DAG'ı (Directed Acyclic Graph)** ile dosya bazlı değişiklik spesifikasyonu çıkarır.  
  2. **Editor (Uygulayıcı) Aşaması:** Hızlı ve ucuz model (örn. deepseek-chat) bu spesifikasyonu alıp loop-rlm veya loop-react kullanarak adımları tek tek uygular.  
> * **Neden Eklenmeli?** Büyük refactoring işlerinde modelin kod yazarken asıl hedefi unutmasını (context drift) tamamen engeller ve API maliyetini %40–60 düşürür.

### **3.2. ferrocore-loop-reflexion (TDD / Doğrulayıcı-Güdümlü Öz-İyileştirme Döngüsü)**

> * **İlham Kaynağı:** Reflexion makalesi (Shinn et al.), SWE-agent doğrulama döngüleri.  
> * **Çalışma Mantığı:** Serbest zamanlı ajanların en büyük sorunu, kodu yazıp derlemeden veya testleri yarım bırakarak "İşi bitirdim\!" demeleridir. Bu döngü **Deterministik Kapı (Deterministic Gatekeeper)** zorunluluğu getirir:  
>   ![][image3]  
> * **Özelliği:** Model "bitti" dese bile, yapılandırılan doğrulama komutu (verifier\_cmd \= "cargo test") exit\_code \== 0 dönmeden döngü sonlanmaz; derleyici hataları otomatik olarak modele geri beslenir.

### **3.3. ferrocore-loop-mcts (Turnuva / Paralel Git Worktree Dallanma Döngüsü)**

> * **İlham Kaynağı:** Tree-of-Thoughts (ToT), AlphaCode, Monte Carlo Tree Search (MCTS).  
> * **Çalışma Mantığı:** Çözümü çok zor olan algoritmik veya mimari bir hatada tek bir yoldan gitmek yerine:  
  1. Git üzerinde 3 ayrı geçici çalışma ağacı (git worktree add /tmp/wt\_1..3) açar.  
  2. 3 paralel alt ajanı (loop-rlm) farklı hipotezlerle bu izole dizinlerde aynı anda koşturur.  
  3. Her dalda test paketini (cargo test \+ benchmark) çalıştırarak deterministik bir ödül skoru (![][image4]) hesaplar.  
  4. Kazanan dalı ana dala birleştirir (git merge), başarısız çalışma ağaçlarını siler.

### **3.4. ferrocore-loop-fsm (Deterministik İş Akışı / Sonlu Durum Makinesi Döngüsü)**

> * **İlham Kaynağı:** CI/CD boru hatları, GitHub Actions ajanları.  
> * **Çalışma Mantığı:** Modelin hangi adımı atacağına kendisinin karar vermesinin **istenmediği** kurumsal otomasyonlar içindir (Örn: Otomatik PR İnceleyici, Güvenlik Denetçisi, Sürüm Notu Oluşturucu).  
> * Adımlar config.toml içinde katı bir durum makinesi (Git Diff Çek ![][image5] Statik Analiz Koştur ![][image5] LLM ile Güvenlik Raporu Üret ![][image5] PR Yorumu At) olarak tanımlanır. Sonsuz döngü riski sıfırdır.

### **3.5. ferrocore-loop-ambient (Olay-Güdümlü / Arka Plan Bekçi Döngüsü)**

> * **İlham Kaynağı:** Unix Daemon'ları, Proactive Agents.  
> * **Çalışma Mantığı:** Kullanıcının sohbet satırından (stdin) komut yazmasını beklemez. Dosya sistemi olaylarını (notify / inotify), Git kancalarını (post-commit) veya log dosyalarını dinler.  
> * Geliştirici IDE'de dosyayı kaydettiği anda arka planda sessizce uyanır, kırılan testi veya tip hatasını tespit eder ve geliştiriciye hazır bir yama (diff) önerisi sunar.

### **Döngü Eklentilerinin Karşılaştırma Tablosu**

| Döngü Crate'i | Tetikleyici & Çıktı Biçimi | Birincil Kullanım Senaryosu | Öncelik / Faz |
| :---- | :---- | :---- | :---- |
| **ferrocore-loop-rlm** | Ham Kod Bloğu ![][image5] Kalıcı PTY | Derin yazılım geliştirme, sistem yönetimi, kabuk otomasyonu. | **Faz 2 (MVP)** |
| **ferrocore-loop-react** | JSON Tool Call ![][image5] MCP/WASM | Harici API'ler, veritabanı sorguları, yapılandırılmış araçlar. | **Faz 4** |
| **ferrocore-loop-reflexion** | Yama ![][image5] Zorunlu Test Kapısı | TDD, derleyici hatası düzeltme, garantili bug-fix görevleri. | **Faz 4 (Önerilen)** |
| **ferrocore-loop-plan** | Read-Only DAG ![][image5] Alt Döngü | Büyük ölçekli çok dosyalı mimari dönüşümler (Refactoring). | **Faz 5** |
| **ferrocore-loop-mcts** | Paralel git worktree ![][image5] Test Skoru | Kritik ve tek denemede çözülemeyen karmaşık hatalar. | **Faz 5+** |
| **ferrocore-loop-fsm** | Katı Durum Makinesi (State Machine) | CI/CD entegrasyonu, PR inceleme, deterministik boru hatları. | **Faz 5+** |

## **4\. Fiziksel Repo ve Dizin Topolojisi (Güncellenmiş)**

\================================================================================  
REPO 1: github.com/org/ferrocord  (Bağımsız Mikro-Çekirdek Crate'i)  
\================================================================================  
ferrocord/  
├── .github/workflows/ci.yml        \# Bağımsız, hızlı CI ve Crates.io publish hattı  
├── Cargo.toml                      \# \[package\] name \= "ferrocord"  
└── src/  
    ├── lib.rs  
    ├── context.rs                  \# Senkron RwLock \+ Arc\<dyn Any\> tabanlı DI Konteyneri  
    ├── event.rs                    \# TypeId tabanlı jenerik Tokio broadcast EventBus  
    └── plugin.rs                   \# \#\[async\_trait\] pub trait Plugin

\================================================================================  
REPO 2: github.com/org/ferrocore  (Ana Ajan Motoru \- Cargo Workspace)  
\================================================================================  
ferrocore/  
├── .github/workflows/ci.yml        \# Tüm ajan modüllerini test eden ve binary derleyen CI  
├── Cargo.toml                      \# \[workspace\] \+ ferrocord bağımlılığı  
├── crates/  
│   ├── ferrocore-types/            \# AgentLoop, Tool, Session trait ve veri modelleri  
│   ├── ferrocore-driver/           \# Ortak TurnDriver (LLM akışı, JSONL kayıt, Bütçe, Retry)  
│   ├── ferrocore-runtime-pty/      \# portable-pty, Sentinel, Interceptor ve UDS yöneticisi  
│   ├── ferrocore-loop-rlm/         \# Prime Agent (Single-Tool PTY kardeş döngü eklentisi)  
│   ├── ferrocore-loop-react/       \# Standart Tool-Calling (ReAct kardeş döngü eklentisi)  
│   ├── ferrocore-loop-reflexion/   \# (Opsiyonel) Doğrulayıcı-kapılı TDD döngü eklentisi  
│   ├── ferrocore-loop-plan/        \# (Opsiyonel) Architect/Editor DAG döngü eklentisi  
│   └── ferrocore-tools/            \# MCP uyumlu IPC ve Extism/WASM adaptörleri  
├── src/                            \# ferrocore-cli (Ana binary \+ \`internal-spawn\` alt komutu)  
│   └── main.rs  
└── config.example.toml             \# Aktif döngü seçimi ve eklenti konfigürasyonu

\================================================================================  
REPO 3..N: github.com/org/ferrocore-plugin-\*  (Harici Eklenti Repoları)  
\================================================================================  
ferrocore-plugin-sqlite/            \# Örnek: WASM veya MCP olarak derlenen bağımsız repo  
├── .github/workflows/release.yml   \# .wasm veya binary üreten bağımsız CI/CD  
└── ...

## **5\. Uygulama Fazları (Yol Haritası)**

> 1. **Faz 1 (ferrocord v0.1.0):** ferrocord reposunun açılması; senkron Arc tabanlı Context, jenerik EventBus ve Plugin trait'lerinin yazılıp birim testleriyle dondurulması.  
> 2. **Faz 2 (ferrocore MVP \- Driver, PTY & RLM):** ferrocore workspace reposunun açılması, ferrocore-driver (ortak LLM/JSONL altyapısı), ferrocore-runtime-pty (Sentinel protokolü \+ Interceptor) ve ilk çalışan ferrocore-loop-rlm döngüsünün ayağa kaldırılması.  
> 3. **Faz 3 (Recursive Spawning & Cross-Loop):** UDS soket sunucusu, ferrocore internal-spawn alt komutu ve kilitlenmesiz (deadlock-free) derinlik kotalarının bağlanması.  
> 4. **Faz 4 (Kardeş Döngüler & Dış Eklenti Motoru):** ferrocore-loop-react ve ferrocore-loop-reflexion kardeş eklentilerinin, ayrıca IPC (MCP) ve WASM adaptörlerinin eklenmesi.  
> 5. **Faz 5 (İleri Seviye Orkestrasyon):** ferrocore-loop-plan (Architect/Editor) ve ferrocore-loop-mcts (Paralel Git Worktree turnuvası) eklentilerinin devreye alınması.

[image1]: <data:image/png;base64,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>

[image2]: <data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGoAAAAWCAYAAAAowQktAAACrElEQVR4Xu2YjU0DMQxGbwZWYAZWYAVWYAVWYANGYAQ2YAM2YAEGgDxVllwrdpwfyhXdkyJVzZ1j+3OctNt2cHBwcLBD7sq4L+PWTlwR+M/4t3yW8VXG23bdgb6U8VHGdxk3Zm4Jr2W8b6eEXZqH7SRQFNjKSmXnzpDxhXkEWw6GqQYq4dJEuwjxKKDn7VRM7DqEHQGBWGs0Rt6nkPEHEfDl8eyJc1hntiiqjARBIkmiHT0QeE0oBMGfJ/M9CepFzj3s9cYoIJL2kzix5fnDXC2uaVgwGwSBIyzOW5FWCSWFY+d4fvTCMSMU7+mjAb/EnvURvO+nYItimCS0oNJxONr2PXhCsUYtWJ4fbX+zQtF+BbrJrwvFIixKMthJfMZwZjfwzsre6wnlJUHOrBFmhLK7WFozw85BzfcuRBx9y8oGUHNoBuxFPb4W7F8JZcEOufRuq8zrHdiNNUCy+C7T9khaZrTgGVonInnr7lkoxOHmFxWuXDaiW20IL+seT8L4LhM8C5Ko1mjBM1Qjwwtiz0IhkreTBDlespvgDLk0aMSYTUiNzDM9RL/d5BZq1yRoimuEFULJnwMCu6omGv5TiEPnOUatulrxTKWylVfC2lYMQAx8s7+jbKJJRMZv8ITCRmun4ofeSfLfZM0e1IqsC1QWZIdR2XbOg4qaOiQNnlCAP/qvGJJlEyO/t6LzAiSp9n0QG5Edmbcjugx5caUgeJJDskmC/DWDs1FFaRAWO6MtSBMJJX/b4Je0Sesj5y3+ezbAJpehO4tctSM79v2aHc20UIAB3Tv53NtLeV6SZ0cPkVAC4rCbvGpnvmUjwyo7sESoPZERqkXrqpxllR34d0KxK2daKId7bzewyO+iWTsahKrdBq8WguFsoO2MJGpFMlaIDdjhvKNLDBXfDxVn/iAGzsVXAAAAAElFTkSuQmCC>

[image3]: <data:image/png;base64,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>

[image4]: <data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAFIAAAAWCAYAAABT5cvhAAACBElEQVR4Xu2Y4U3EMAxGMwMrMAMrsAIr3AqswAaMwAhswAZscAswAPSpZ7AsO+ckbblKfZL/pG3s74uTC5RycHBw8C/cTfF4ia25L3PeB/tgjyDka4oP+2ADTlN8ls7cL2X+0Mb75RmrtCUYGQmRTl2iJjrfm6eWvwpt/DTFd5knkGIx8XwZf/19e30iIXTLW/mri4XuAQOZi65nLkuUPwWTY5i3QkzKs1ZkMXRk8ISw3RCueS7t56jooxY0eTV5+dPQkayyBwKyRrIgdK90zBJGsmNkt2h4jw7tYTUjEe8VxaQkzBpJ51AchvZihbANIyPJ18NqRlIQBWswg3ES2mcecvaMYoWIaCtOFrmHVYyUHxq9BZnI69Aa0dHQihWyGyPZ1l5BUaIIzlKKqEUGK2Q3Rsr2tXjF15BLdC0yWCG7MFKuPV5BXvE1vDl6sEK45ni18N7ZjGVZ3EjZ1t55yHjLRZwCRn6tBU8Ihtl7JCaczBhXrszfyYsaiWgKjCbURmYN4pjIvhvhCcEw3fFSu0bum15TaPgWw6N3vfwhsl10MKZhjGIRkb2v8S7fsDCZzvCIhLCoBHNTj1fTtX92SCfaIKcQ5e8GI0hM6ETX4M8wFoVibMEZakKkHq5sEdG3WWr5d8WIEAz2tmsLI/lvihEhoybCSP6bAiGcfy3HyVLoY6nKD+XJzbpM1WMdAAAAAElFTkSuQmCC>

[image5]: <data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABIAAAASCAYAAABWzo5XAAAAU0lEQVR4XmNgGAWjYODASnQBcsEzIFZGFyQHdEExVYApA8RlKADkTDcycCUUCzFAAciZJ8jEIFeBDKMIgGIP7hpyAcgQipPBTiAORxckB3xGFwAA0x0ZgFi7y5gAAAAASUVORK5CYII=>