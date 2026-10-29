# deskMonk by pelamx — Akıllı depolama temizleyici

Bilgisayardaki gereksiz dosyaları, kopyaları, eski indirmeleri ve büyük dosyaları bulur.
Silmeden önce ne kadar alan kazanılacağını gösterir. macOS, Linux ve Windows.

| Binary | Ne için |
|---|---|
| `deskmonk` | Masaüstü uygulaması (egui) |
| `deskmonk-tui` | Terminal arayüzü — sunucular / cloud makineler için |
| `deskmonk-server` | Anonim kullanım istatistikleri API'si + admin paneli |

## Kategoriler

- **Kopyalar**: aynı boyut, aynı ilk 64 KB ve aynı SHA-256 içerik (32 KB üstü dosyalar). Her grubun ilk kopyası korunur.
  Fazla kopyalar sadece kullanıcı klasörlerinde (İndirilenler, Belgeler, Masaüstü, Resimler, Müzik, Videolar) otomatik seçilir.
  Gizli klasörler ve git repoları kopya aramasına girmez, çünkü bunlar uygulama ya da proje dosyalarıdır.
- **Gereksiz**: uygulama önbellekleri (`~/.cache`, `~/Library/Caches`, Windows'ta `%TEMP%`), çöp kutusu, npm önbelleği.
  Her biri uygulama başına tek satır olarak listelenir. Ayrıca `.tmp .temp .bak .old .log .crdownload .part .dmp`, `*~`,
  `.DS_Store`, `Thumbs.db` ve `desktop.ini` dosyaları da bu kategoriye girer.
- **Eski indirmeler**: İndirilenler klasöründe 90 günden eski dosyalar.
- **Büyük dosyalar**: 500 MB ve üzeri.

Her dosya tek bir kategoride listelenir, yani boyutlar iki kez sayılmaz. Sembolik linkler takip edilmez.
`.git` ve `node_modules` klasörlerine hiç girilmez. Silme işlemi arka planda çalışır ve arayüz donmaz.
**Gereksiz** kategorisi (önbellekler, çöp kutusu, geçici dosyalar) kalıcı silinir, çünkü çöpe taşımak yer açmaz.
Kopyalar, eski indirmeler ve büyük dosyalar geri alınabilsin diye çöp kutusuna taşınır. Salt okunur klasörler de silinir.
TUI'de `d` aynı kuralla temizler, `D` her şeyi kalıcı siler (sunucularda çöp kutusu yer açmaz).

## İndirme

[Releases](https://github.com/pelamx/deskmonk/releases) sayfasında:
- `deskMonk-<sürüm>-macos.dmg`: macOS, Apple Silicon (M1–M5) ve Intel için tek uygulama (macOS 10.13+)
- `deskMonk-<sürüm>-linux-x86_64.AppImage` / `-linux-aarch64.AppImage`: tek dosya, neredeyse her dağıtımda çalışır
  (glibc 2.28+: Ubuntu 18.10+, Debian 10+, Fedora, RHEL/Alma/Rocky 8+, Arch, openSUSE…)
- `deskmonk_<sürüm>_amd64.deb` / `_arm64.deb`: Debian, Ubuntu, Mint, Pop!_OS (`sudo apt install ./deskmonk_*.deb`)
- `deskmonk-<sürüm>-1.x86_64.rpm` / `.aarch64.rpm`: Fedora, RHEL/Alma/Rocky, openSUSE (`sudo dnf install ./deskmonk-*.rpm`)
- `deskmonk-<sürüm>-1-x86_64.pkg.tar.zst` / `-aarch64`: Arch, Manjaro, EndeavourOS (`sudo pacman -U deskmonk-*.pkg.tar.zst`)
- `deskmonk-linux-x86_64.tar.gz` / `-aarch64.tar.gz`: aynı uygulama ve TUI; `./install.sh` menüye ekler
  (kendi kullanıcın için `~/.local`, `sudo` ile `/usr/local`; `./install.sh --uninstall` kaldırır)
- `deskmonk-windows-x86_64.zip`: Windows uygulaması ve TUI
- `deskmonk-tui-linux-x86_64` / `-aarch64`: statik TUI, o işlemcili her Linux'ta çalışır (sunucular, cloud)

```sh
chmod +x deskMonk-*.AppImage && ./deskMonk-*.AppImage
```

Paketler `packaging/macos.sh` ve `packaging/linux.sh` ile yapılır; CI her `v*` etiketinde hepsini derleyip yayınlar.

## Derleme

```sh
cargo build --release                       # hepsi
cargo build --release --no-default-features --features tui --bin deskmonk-tui   # sadece TUI (sunucu, GUI bağımlılığı yok)
cargo build --release -p deskmonk-server
```

## Masaüstü

```sh
deskmonk [KLASÖR]   # klasör verilirse açılır açılmaz taranır
```
Kenar çubuğunda kategoriler, üstte klasör seçimi (Gözat… / hızlı klasörler) ve özet kartları bulunur.
Satıra tıklamak seçer, 📂 ise dosyayı klasöründe gösterir. Üstteki tema seçicisi **Sistem / Açık / Koyu** sunar:
sistem seçiliyken işletim sisteminin temasını izler, diğer ikisi seçimi kalıcı olarak saklar.
macOS'ta pencereyi kapatmak (✕ / Cmd+W) uygulamayı gizler: tarama ve temizleme arka planda sürer, Dock simgesine
tıklayınca pencere geri gelir. Çıkmak için Cmd+Q veya Dock → Çık.
Kenar çubuğundaki **Sistem → Donanım** terminal görünümünde bileşenleri markalarıyla (işlemci, bellek, ekran kartı,
ekran, disk, pil) ve doluluk/pil sağlığı/çalışma süresi gibi kullanım bilgileriyle gösterir.
**Sistem → Disk haritası** seçilen klasörün alanını kutular halinde (treemap) gösterir: kutuya tıklayınca içine girilir,
sağ tık klasörde gösterir, üstteki yol ile geri çıkılır.
Donanım ekranındaki **Disk hız testi** sistem diskine 1 GB'lık geçici dosya yazıp geri okur ve MB/s gösterir; dosya her
durumda silinir, yeterli boş alan (3 GB) yoksa test çalışmaz.
**Sistem → Uygulama kilidi** (macOS, Linux): şifre belirlenip uygulamalar seçilir; kilitli bir uygulama açılınca deskMonk onu
durdurur ve şifre sorar, doğru şifreyle açar, uygulama kapatılınca yeniden kilitler. Zaten açık olan uygulamalar (kaydedilmemiş
iş olabilir) kapatılana kadar durdurulmaz. deskMonk açıkken çalışır (pencereyi kapatmak sorun değil); Cmd+Q ile çıkılırsa
kilit kalkar, bu yüzden bir gizlilik kilidi olarak düşünülmeli, güvenlik duvarı olarak değil.
Linux'ta uygulamalar menüdeki `.desktop` kayıtlarıdır (Flatpak ve Snap dahil). Açılan uygulama programının yolundan
ya da masaüstünün (GNOME, KDE, uwsm, Flatpak) onu başlattığı systemd scope'undan tanınır; yalnızca kullanıcının kendi
süreçleri durdurulur. Pencereyi küçültmek sorun değil, deskMonk kapatılırsa kilit kalkar. Kabuk betiğiyle açılan ve
kendi scope'u olmayan uygulamalar tanınamaz, terminal uygulamaları listelenmez.

## TUI

```sh
deskmonk-tui [KLASÖR]   # varsayılan: ev dizini
```
`↑↓` gezin · `boşluk` seç · `a/n` tümü/hiçbiri · `←→` kategori · `d` temizle · `D` hepsini kalıcı sil · `r` yeniden tara · `t` istatistik aç/kapa · `q` çık

Masaüstü uygulaması olmayan sunucularda Pro, deskmonk.app/buy'dan alınan anahtarla açılır: `deskmonk-tui --key ANAHTAR`.
Abonelik yenilendiğinde TUI bunu kendisi fark eder (süre dolunca açılışta sunucuya sorar).

## Kullanım istatistikleri ve admin paneli

İstatistik gönderimi **opt-in**'dir: ilk açılışta kullanıcıya sorulur, istendiğinde kapatılabilir.
Sayılar (dosya sayısı, bayt toplamları, OS, sürüm, rastgele kurulum ID) ve her açılışta donanım bilgileri (bileşen, marka,
model, disk doluluğu, pil sağlığı) gider. **Dosya adı, yolu, seri numarası, bilgisayar veya kullanıcı adı asla gönderilmez.**
Donanım eklenmeden önce "evet" demiş kullanıcılara izin yeniden sorulur.
Uygulama `DESKMONK_TELEMETRY_URL` ile derlenmezse hiçbir veri gönderilmez ve izin de sorulmaz.

```sh
DESKMONK_TELEMETRY_URL=https://deskmonk.app/api/events cargo build --release
```

Canlı sunucu `worker/` içindeki Cloudflare Worker `deskmonk`dır (D1 veritabanı `deskmonk`, adres `deskmonk.app`): web sitesi (`docs/`), API, `/admin` ve `/buy` aynı adreste.
Admin şifresi `ADMIN_TOKEN` secret'ıdır:

```sh
cd worker && npm install && npm test
npx wrangler secret put ADMIN_TOKEN   # şifreyi değiştirmek için
npm run deploy
```

Kendi makinende çalıştırmak istersen `deskmonk-server` aynı API'yi ve paneli sunar:

```sh
DESKMONK_ADMIN_TOKEN='uzun-gizli-bir-token' \
DESKMONK_ADDR=0.0.0.0:8080 DESKMONK_DB=/var/lib/deskmonk.db \
deskmonk-server
```

Admin paneli `/admin` adresinde (HTTP Basic auth, kullanıcı adı `admin`, şifre token). Panelde şunlar var:
toplam/aktif kullanıcı, tarama/temizleme sayıları, bulunan/kazanılan alan, OS/arayüz/sürüm kırılımı,
bileşen marka dağılımı, kategori dağılımı, son 30 gün ve kullanıcı listesi (işlemci ve açılış sayısıyla). Her kurulumun
donanımı (terminal görünümünde) ve ayrıntılı olay geçmişi `/admin/user/<id>` adresinde.

> Sunucu düz HTTP konuşur, internete açarken önüne TLS yapan bir reverse proxy (Caddy/nginx) koyun.
