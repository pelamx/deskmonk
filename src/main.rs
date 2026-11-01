#![cfg_attr(windows, windows_subsystem = "windows")] // no console window on Windows

use deskmonk::applock;
use deskmonk::focus;
use deskmonk::i18n::{self, Lang, t};
use deskmonk::tf;
use deskmonk::diskmap::{self, Node};
use deskmonk::hardware::{self, Component};
use deskmonk::scan::{Category, Item, Phase, human_size};
use deskmonk::telemetry::{self, Telemetry};
use deskmonk::plan::{self, Plan};
use deskmonk::update;
use deskmonk::vault;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke,
    Theme, Ui, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod access;
#[cfg(feature = "appstore")]
mod store;

/// Mac App Store build: sandboxed, Pro through Apple, no App Lock.
const APPSTORE: bool = cfg!(feature = "appstore");
const APPLE_EULA: &str = "https://www.apple.com/legal/internet-services/itunes/dev/stdeula/";
const PRIVACY_POLICY: &str = "https://deskmonk.app/privacy";

const ACCENT: Color32 = Color32::from_rgb(79, 110, 247);
const DANGER: Color32 = Color32::from_rgb(229, 72, 77);
/// Starts a status line that reports a failed cleaning; the footer shows such a line in red.
const FAILED: &str = "⚠";
const ROW_H: f32 = 46.0;
/// Size of the disk speed test file.
const SPEED_TEST: u64 = 1 << 30;
/// Terminal greens of the hardware view.
const TERM: Color32 = Color32::from_rgb(57, 255, 20);
const TERM_DIM: Color32 = Color32::from_rgb(31, 158, 69);
const TERM_BG: Color32 = Color32::from_rgb(7, 11, 8);
/// Box colors of the disk map, neon on the terminal background.
const NEON: [Color32; 8] = [
    TERM,
    Color32::from_rgb(0, 229, 255),
    Color32::from_rgb(255, 0, 170),
    Color32::from_rgb(255, 214, 0),
    Color32::from_rgb(255, 120, 0),
    Color32::from_rgb(170, 90, 255),
    Color32::from_rgb(60, 140, 255),
    Color32::from_rgb(255, 70, 70),
];

fn main() -> eframe::Result {
    deskmonk::use_real_home();
    i18n::init();
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("bundled icon is a valid PNG");
    // On macOS this is the Dock icon, which needs Apple's padded icon grid; elsewhere the full-bleed logo fits.
    let window_icon = if cfg!(target_os = "macos") {
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/dock-icon.png"))
            .expect("bundled Dock icon is a valid PNG")
    } else {
        icon.clone()
    };
    if let [_, flag, app] = &std::env::args().collect::<Vec<_>>()[..]
        && flag == UNLOCK_FLAG
    {
        return unlock_prompt(app.into(), window_icon);
    }
    let options = native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("deskMonk by pelamx")
            // Wayland compositors take the icon from the .desktop file matching this id.
            .with_app_id("deskmonk")
            .with_icon(Arc::new(window_icon)),
    );
    let result = eframe::run_native(
        "deskMonk",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            let mut app = App::default();
            app.telemetry.started("gui");
            let (tx, rx) = channel();
            let ctx = cc.egui_ctx.clone();
            // At start and every 12 hours, for copies that stay open for days: finds, downloads and installs a new version.
            std::thread::spawn(move || {
                // On Windows the copy this one replaced may still be closing; its old file can only go once it has.
                std::thread::sleep(Duration::from_secs(if cfg!(windows) { 10 } else { 0 }));
                update::tidy();
                loop {
                    if let Some(found) = update::check(true) {
                        let installed = matches!(found, update::Found::Installed(_));
                        if tx.send(found).is_err() || installed {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    std::thread::sleep(Duration::from_secs(12 * 3600));
                }
                ctx.request_repaint();
            });
            app.update_check = Some(rx);
            app.detect_hardware(cc.egui_ctx.clone());
            #[cfg(target_os = "macos")]
            if deskmonk::sandboxed() {
                access::restore();
            }
            #[cfg(feature = "appstore")]
            store::start(&cc.egui_ctx);
            // App Lock stops other apps, which the App Store sandbox does not allow.
            if cfg!(any(target_os = "macos", target_os = "linux")) && !APPSTORE {
                let (tx, rx) = channel();
                let ctx = cc.egui_ctx.clone();
                app.watcher = Some(applock::Watcher::start(app.lock.apps.clone(), move |watcher, blocked| {
                    // From this thread: the window's own code doesn't run while it is on another Wayland workspace.
                    if cfg!(target_os = "linux") {
                        ask_unlock(watcher.clone(), blocked);
                        return;
                    }
                    let _ = tx.send(blocked);
                    ctx.request_repaint();
                }));
                app.blocked = Some(rx);
            }
            let logo = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            app.logo = Some(
                cc.egui_ctx
                    .load_texture("logo", logo, egui::TextureOptions::LINEAR),
            );
            // `deskmonk FOLDER` scans that folder right away.
            if let Some(dir) = std::env::args().nth(1) {
                app.root = dir;
                app.autostart = true;
            }
            Ok(Box::new(app))
        }),
    );
    retry_without_opengl(&result);
    result
}

/// Palette per theme: (window bg, sidebar/card bg, border, muted text).
fn palette(dark: bool) -> (Color32, Color32, Color32, Color32) {
    if dark {
        (
            Color32::from_rgb(20, 21, 26),
            Color32::from_rgb(29, 31, 38),
            Color32::from_rgb(44, 47, 58),
            Color32::from_rgb(140, 145, 160),
        )
    } else {
        (
            Color32::from_rgb(246, 247, 250),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(226, 229, 236),
            Color32::from_rgb(110, 115, 130),
        )
    }
}

/// Keeps the input method out of this frame's text fields. On Linux, with one running (fcitx5, ibus), a focused field and
/// the input method keep waking each other, which redraws without end, lags, and loses or repeats keys.
/// It should not see passwords either. shortcut: no CJK input on Linux, add it back when that loop is gone.
fn no_input_method(ctx: &egui::Context) {
    if cfg!(target_os = "linux") {
        ctx.output_mut(|o| o.ime = None);
    }
}

/// `deskmonk --unlock-prompt APP`: asks for the password to open a locked app, and exits with this code if it was right.
/// Closing the window or a crash exits with anything else, which leaves the app locked.
const UNLOCKED: i32 = 10;
const UNLOCK_FLAG: &str = "--unlock-prompt";
/// A prompt left open on another workspace would make later launches of the app do nothing visible, so it gives up.
const UNLOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// On Linux the prompt is a program of its own: a new window opens on the workspace the user is on. Wayland does not
/// let deskMonk raise its own window, and its window code doesn't run at all while it sits on another workspace.
fn ask_unlock(watcher: applock::Watcher, app: PathBuf) {
    std::thread::spawn(move || {
        // /proc/self/exe still works after an update replaced the program file.
        let status = std::process::Command::new("/proc/self/exe").arg(UNLOCK_FLAG).arg(&app).status();
        if status.is_ok_and(|s| s.code() == Some(UNLOCKED)) {
            watcher.unlock(&app);
        } else {
            watcher.dismiss(&app);
        }
    });
}

struct UnlockPrompt {
    app: PathBuf,
    name: String,
    lock: applock::Config,
    icons: Icons,
    pw: String,
    error: String,
    opened: Instant,
}

impl eframe::App for UnlockPrompt {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if self.opened.elapsed() > UNLOCK_TIMEOUT {
            std::process::exit(1);
        }
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_secs(1));
        let icon = self.icons.get(&ctx, &self.app);
        let (bg, _, _, _) = palette(ctx.global_style().visuals.dark_mode);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(bg).inner_margin(Margin::same(28))).show(ui, |ui| {
            let name = self.name.clone();
            match app_password_prompt(ui, Some((icon, &name)), &tf!("{name} kilitli", "{name} is locked", "{name} está bloqueada"),
                t("Açmak için deskMonk şifresini girin.", "Enter the deskMonk password to open it.", "Introduzca la contraseña de deskMonk para abrirla."),
                t("Aç", "Open", "Abrir"), &mut self.pw, &self.error) {
                Some(true) if self.lock.check(&self.pw) => std::process::exit(UNLOCKED),
                Some(true) => {
                    self.error = wrong_password().into();
                    self.pw.clear();
                }
                Some(false) => std::process::exit(1),
                None => {}
            }
        });
        no_input_method(&ctx);
    }
}

/// On Linux: OpenGL without vsync. With Vulkan and vsync, a repaint while the window is on another Wayland workspace
/// spins a whole CPU core in the driver and stops the app's code until the window is shown again.
/// egui only draws when something changes, so no vsync does not mean more drawing.
fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    if cfg!(target_os = "linux") && std::env::var_os(NO_OPENGL).is_none() {
        options.renderer = eframe::Renderer::Glow;
        options.glow_options.vsync = false;
    }
    options
}

const NO_OPENGL: &str = "DESKMONK_NO_OPENGL";

/// A machine where OpenGL won't start may still have Vulkan: runs once more with the default renderer.
fn retry_without_opengl(result: &eframe::Result) {
    let Err(error) = result else { return };
    if !cfg!(target_os = "linux") || std::env::var_os(NO_OPENGL).is_some() {
        return;
    }
    eprintln!("OpenGL failed ({error}), trying the default renderer");
    let status = std::process::Command::new("/proc/self/exe").args(std::env::args_os().skip(1)).env(NO_OPENGL, "1").status();
    std::process::exit(status.ok().and_then(|s| s.code()).unwrap_or(1));
}

fn unlock_prompt(app: PathBuf, icon: egui::IconData) -> eframe::Result {
    let options = native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([376.0, 380.0])
            .with_resizable(false)
            .with_title("deskMonk")
            .with_app_id("deskmonk")
            .with_icon(Arc::new(icon)),
    );
    let result = eframe::run_native(
        "deskMonk",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            let name = applock::name(&app);
            Ok(Box::new(UnlockPrompt { app, name, lock: applock::Config::load(), icons: Icons { budget: 1, ..Default::default() }, pw: String::new(), error: String::new(), opened: Instant::now() }))
        }),
    );
    retry_without_opengl(&result);
    result
}

fn setup_style(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let dark = theme == Theme::Dark;
        let (bg, card, border, _) = palette(dark);
        ctx.style_mut_of(theme, |style| {
            use egui::{FontFamily::Proportional, TextStyle};
            style.text_styles = [
                (TextStyle::Heading, FontId::new(22.0, Proportional)),
                (TextStyle::Body, FontId::new(14.5, Proportional)),
                (TextStyle::Button, FontId::new(14.5, Proportional)),
                (TextStyle::Small, FontId::new(12.0, Proportional)),
                (
                    TextStyle::Monospace,
                    FontId::new(13.5, egui::FontFamily::Monospace),
                ),
            ]
            .into();
            style.spacing.item_spacing = vec2(8.0, 8.0);
            style.spacing.button_padding = vec2(14.0, 7.0);
            style.spacing.interact_size.y = 32.0;
            let v = &mut style.visuals;
            v.panel_fill = bg;
            v.window_fill = card;
            v.extreme_bg_color = if dark {
                Color32::from_rgb(16, 17, 21)
            } else {
                Color32::WHITE
            };
            v.faint_bg_color = card;
            v.window_corner_radius = CornerRadius::same(14);
            v.window_stroke = Stroke::new(1.0, border);
            v.selection.bg_fill = ACCENT;
            v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
            v.hyperlink_color = ACCENT;
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.noninteractive,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CornerRadius::same(8);
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
            v.widgets.inactive.weak_bg_fill = if dark {
                Color32::from_rgb(40, 43, 53)
            } else {
                Color32::from_rgb(236, 238, 243)
            };
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.hovered.weak_bg_fill = if dark {
                Color32::from_rgb(50, 54, 66)
            } else {
                Color32::from_rgb(226, 229, 237)
            };
            v.widgets.hovered.bg_stroke = Stroke::NONE;
        });
    }
}

fn icon(cat: Category) -> &'static str {
    match cat {
        Category::Duplicate => "📋",
        Category::Junk => "🗑",
        Category::OldDownload => "⬇",
        Category::Large => "📦",
    }
}

fn description(cat: Category) -> &'static str {
    match cat {
        Category::Duplicate => {
            t("İçeriği birebir aynı dosyalar. Her grubun ilk kopyası korunur, fazlası seçilir.",
              "Files with exactly the same content. The first copy in each group is kept, the rest are selected.",
              "Archivos con exactamente el mismo contenido. Se conserva la primera copia de cada grupo y se seleccionan las demás.")
        }
        Category::Junk => {
            t("Uygulama önbellekleri, çöp kutusu, geçici dosyalar, yedekler ve loglar. Önbellekler gerektiğinde yeniden oluşturulur.",
              "App caches, the trash, temporary files, backups and logs. Caches are rebuilt when needed.",
              "Cachés de apps, la papelera, archivos temporales, copias de seguridad y registros. Las cachés se regeneran cuando hace falta.")
        }
        Category::OldDownload => {
            t("İndirilenler klasöründe 90 günden uzun süredir değişmemiş dosyalar.",
              "Files in Downloads that have not changed for more than 90 days.",
              "Archivos en Descargas que no cambian desde hace más de 90 días.")
        }
        Category::Large => {
            t("500 MB ve üzeri dosyalar. Gerçekten gerekli olup olmadığını kontrol edin.",
              "Files of 500 MB or more. Check whether you really need them.",
              "Archivos de 500 MB o más. Compruebe si realmente los necesita.")
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Clean,
    Monk,
    Hardware,
    Map,
    Lock,
    Vault,
    License,
}

enum Row {
    Group(usize),
    /// The bool marks a group's first copy, which selecting everything leaves unticked.
    Item(usize, bool),
}

struct App {
    root: String,
    items: Vec<Item>,
    tab: Category,
    /// Messages are (phase, items): partial lists without duplicates, then the final one with phase None.
    scanning: Option<(Receiver<(Option<Phase>, Vec<Item>)>, Arc<AtomicUsize>)>,
    /// Phase of the partial results on screen.
    phase: Option<Phase>,
    scanned_files: usize,
    has_scanned: bool,
    confirm: bool,
    deleting: Option<deskmonk::Deletion>,
    status: String,
    telemetry: Telemetry,
    autostart: bool,
    logo: Option<egui::TextureHandle>,
    page: Page,
    /// When the hardware view was opened, for its typing animation.
    hw_since: f64,
    hardware: Option<Vec<Component>>,
    hw_rx: Option<Receiver<Vec<Component>>>,
    hw_sent: bool,
    /// Folder tree of `map_root`, and the child indexes of the folder being looked at.
    map: Option<Node>,
    map_root: PathBuf,
    map_path: Vec<usize>,
    map_scan: Option<(Receiver<Node>, Arc<AtomicUsize>)>,
    speed: Option<Result<deskmonk::speedtest::Speed, String>>,
    monk: focus::Timer,
    /// Running speed test and its bytes done (write then read, up to 2 × SPEED_TEST).
    speed_run: Option<(Receiver<std::io::Result<deskmonk::speedtest::Speed>>, Arc<AtomicU64>)>,
    lock: applock::Config,
    watcher: Option<applock::Watcher>,
    /// Locked apps that were stopped and wait for the password.
    blocked: Option<Receiver<PathBuf>>,
    /// The app the password prompt is for.
    lock_prompt: Option<PathBuf>,
    /// Typed password (prompts or new password) and its confirmation.
    lock_pw: String,
    lock_pw2: String,
    lock_error: String,
    /// Installed apps for the lock list, read when the settings open; and the search text.
    installed: Option<Vec<PathBuf>>,
    app_filter: String,
    /// A lock removal waiting for the password.
    unlock: Option<Unlock>,
    icons: Icons,
    /// Vault: the picked file, folder or vault; the typed passwords; the running job and its result.
    vault_target: Option<PathBuf>,
    vault_pw: String,
    vault_pw2: String,
    vault_error: String,
    /// Result (output path, error moving the source to the Trash), bytes done, bytes in total.
    vault_job: Option<(Receiver<Result<(PathBuf, Option<String>), String>>, Arc<AtomicU64>, Arc<AtomicU64>)>,
    /// Last finished job: what it made, whether it encrypted, the Trash error if any.
    vault_done: Option<(PathBuf, bool, Option<String>)>,
    /// Vaults made here, newest first (saved in the config folder).
    vaults: Vec<PathBuf>,
    /// The password pop-up for opening a vault from the list is shown.
    vault_prompt: bool,
    plan: Plan,
    pro_open: bool,
    /// When a checkout was opened: the license is re-checked every few seconds until Pro arrives.
    pro_waiting: Option<f64>,
    /// Typed license key; `pro_manual` = the running check is for it, so its errors are shown.
    pro_key: String,
    pro_manual: bool,
    pro_error: String,
    pro_check: Option<Receiver<Result<(String, u64), String>>>,
    /// When the last license check started; negative = not yet.
    pro_checked: f64,
    /// App Store build: localized prices (monthly, yearly) and whether a purchase sheet is open.
    store_prices: (Option<String>, Option<String>),
    store_busy: bool,
    /// A newer version on the website, found at startup; shown as an update button in the top bar.
    update_check: Option<Receiver<update::Found>>,
    new_version: Option<update::Found>,
    /// The update whose pop-up was answered (Restart/Download or Later); the top bar button stays.
    update_seen: Option<update::Found>,
    /// Contact form: open, typed email and message, the running send, and its outcome (Ok = sent).
    contact_open: bool,
    contact_email: String,
    contact_message: String,
    contact_send: Option<Receiver<Result<(), String>>>,
    contact_result: Option<Result<(), String>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().display().to_string(),
            items: Vec::new(),
            tab: Category::Duplicate,
            scanning: None,
            phase: None,
            scanned_files: 0,
            has_scanned: false,
            confirm: false,
            deleting: None,
            status: String::new(),
            telemetry: Telemetry::load(),
            autostart: false,
            logo: None,
            page: Page::Clean,
            hw_since: 0.0,
            hardware: None,
            hw_rx: None,
            hw_sent: false,
            map: None,
            map_root: PathBuf::new(),
            map_path: Vec::new(),
            map_scan: None,
            speed: None,
            monk: focus::Timer::default(),
            speed_run: None,
            lock: applock::Config::load(),
            watcher: None,
            blocked: None,
            lock_prompt: None,
            lock_pw: String::new(),
            lock_pw2: String::new(),
            lock_error: String::new(),
            installed: None,
            app_filter: String::new(),
            unlock: None,
            icons: Icons::default(),
            vault_target: None,
            vault_pw: String::new(),
            vault_pw2: String::new(),
            vault_error: String::new(),
            vault_job: None,
            vault_done: None,
            vaults: vault::list(),
            vault_prompt: false,
            plan: Plan::load(),
            pro_open: false,
            pro_waiting: None,
            pro_key: String::new(),
            pro_manual: false,
            pro_error: String::new(),
            pro_check: None,
            pro_checked: -1.0,
            store_prices: (None, None),
            store_busy: false,
            update_check: None,
            new_version: None,
            update_seen: None,
            contact_open: false,
            contact_email: String::new(),
            contact_message: String::new(),
            contact_send: None,
            contact_result: None,
        }
    }
}

impl App {
    fn start_scan(&mut self, ctx: egui::Context) {
        let Some(root) = self.granted(PathBuf::from(self.root.trim())) else { return };
        if !root.is_dir() {
            self.status = tf!("Klasör bulunamadı: {}", "Folder not found: {}", "Carpeta no encontrada: {}", root.display());
            return;
        }
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let counter = progress.clone();
        std::thread::spawn(move || {
            let (partial_tx, partial_ctx) = (tx.clone(), ctx.clone());
            let items = deskmonk::scan_default(&root, &counter, move |items, phase| {
                let _ = partial_tx.send((Some(phase), items));
                partial_ctx.request_repaint();
            });
            let _ = tx.send((None, items));
            ctx.request_repaint();
        });
        self.items.clear();
        self.status.clear();
        self.phase = None;
        self.scanning = Some((rx, progress));
    }

    /// Reads the hardware in the background; again after a language change, since the details are worded per language.
    fn detect_hardware(&mut self, ctx: egui::Context) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(hardware::detect());
            ctx.request_repaint();
        });
        self.hw_rx = Some(rx);
    }

    fn start_map(&mut self, ctx: egui::Context) {
        let Some(root) = self.granted(PathBuf::from(self.root.trim())) else { return };
        if !root.is_dir() {
            self.status = tf!("Klasör bulunamadı: {}", "Folder not found: {}", "Carpeta no encontrada: {}", root.display());
            return;
        }
        let files = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let counter = files.clone();
        self.map_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(diskmap::build(&root, &counter));
            ctx.request_repaint();
        });
        self.map_scan = Some((rx, files));
    }

    /// Opens the Pro dialog; `from` (the button or limit that opened it) goes to the usage stats.
    fn open_pro(&mut self, from: &str) {
        self.pro_open = true;
        self.telemetry.used("gui", "pro", from);
    }

    fn delete_selected(&mut self, ctx: egui::Context) {
        self.deleting = Some(deskmonk::Deletion::start(
            &mut self.items,
            false,
            move || ctx.request_repaint(),
        ));
    }

    fn poll_deletion(&mut self) {
        let Some(done) = self.deleting.as_ref().and_then(|d| d.poll()) else {
            return;
        };
        let ok = self.deleting.take().unwrap().total - done.failed.len();
        self.telemetry.clean_done("gui", ok, done.freed);
        self.plan.record_clean(done.freed);
        let cleaned = tf!(
            "✔ {ok} öğe temizlendi, {} yer açıldı",
            "✔ {ok} items cleaned, {} freed",
            "✔ {ok} elementos limpiados, {} liberados",
            human_size(done.freed)
        );
        self.status = match done.first_error {
            // The failure comes first: the footer cuts off a long line.
            Some(err) => {
                let failed = tf!("{FAILED} {} öğe tamamen silinemedi: {err}", "{FAILED} {} items could not be fully deleted: {err}", "{FAILED} {} elementos no se pudieron borrar del todo: {err}", done.failed.len());
                if ok > 0 { format!("{failed} · {cleaned}") } else { failed }
            }
            None => cleaned,
        };
        self.items.extend(done.failed);
    }

    fn totals(&self, cat: Option<Category>, only_selected: bool) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| cat.is_none_or(|c| i.category == c) && (!only_selected || i.selected))
            .fold((0, 0), |(n, s), i| (n + 1, s + i.size))
    }

    fn sidebar(&mut self, ui: &mut Ui, height: f32) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        ui.add_space(18.0);
        ui.label(RichText::new(t("   KATEGORİLER", "   CATEGORIES", "   CATEGORÍAS")).small().color(muted));
        ui.add_space(2.0);
        for cat in Category::ALL {
            let (n, size) = self.totals(Some(cat), false);
            let selected = self.page == Page::Clean && self.tab == cat;
            if nav_item(ui, selected, icon(cat), cat.label(), n, &human_size(size)).clicked() {
                self.tab = cat;
                self.page = Page::Clean;
            }
        }
        ui.add_space(14.0);
        ui.label(RichText::new(t("   SİSTEM", "   SYSTEM", "   SISTEMA")).small().color(muted));
        ui.add_space(2.0);
        let now = Instant::now();
        let sub = match self.monk.phase {
            focus::Phase::Idle => t("Pomodoro zamanlayıcı", "Pomodoro timer", "Temporizador pomodoro").to_string(),
            focus::Phase::Break => tf!("☕ mola · {}", "☕ break · {}", "☕ descanso · {}", mmss(self.monk.remaining(now))),
            _ => tf!("odak · {}", "focus · {}", "enfoque · {}", mmss(self.monk.remaining(now))),
        };
        if nav_item(ui, self.page == Page::Monk, "⏱", t("Monk modu", "Monk mode", "Modo monje"), 0, &sub).clicked() {
            self.page = Page::Monk;
        }
        let cpu = self.hardware.iter().flatten().find(|c| c.kind == "İşlemci");
        let sub = cpu.map_or(t("Bileşenler ve markalar", "Components and brands", "Componentes y marcas"), |c| c.name.as_str());
        if nav_item(ui, self.page == Page::Hardware, "🖥", t("Donanım", "Hardware", "Hardware"), 0, sub).clicked()
            && self.page != Page::Hardware
        {
            self.page = Page::Hardware;
            self.hw_since = ui.input(|i| i.time);
        }
        let sub = self.map.as_ref().map_or(t("Klasör boyutları", "Folder sizes", "Tamaños de carpetas").into(), |m| human_size(m.size));
        if nav_item(ui, self.page == Page::Map, "🗺", t("Disk haritası", "Disk map", "Mapa del disco"), 0, &sub).clicked() {
            self.page = Page::Map;
            if self.map.is_none() && self.map_scan.is_none() {
                self.start_map(ui.ctx().clone());
            }
        }
        if self.watcher.is_some() {
            let n = self.lock.apps.len();
            let sub = match n {
                0 => t("Kapalı", "Off", "Desactivado").to_string(),
                1 => t("1 uygulama kilitli", "1 app locked", "1 app bloqueada").to_string(),
                _ => tf!("{n} uygulama kilitli", "{n} apps locked", "{n} apps bloqueadas"),
            };
            if nav_item(ui, self.page == Page::Lock, "🔒", t("Uygulama kilidi", "App lock", "Bloqueo de apps"), 0, &sub).clicked() {
                self.page = Page::Lock;
            }
        }
        let sub = match &self.vault_job {
            Some(_) => t("Çalışıyor…", "Working…", "Trabajando…").to_string(),
            None if self.vaults.is_empty() => t("Dosya ve klasör şifreleme", "Encrypt files and folders", "Cifrar archivos y carpetas").to_string(),
            None => tf!("{} şifreli öğe", "{} encrypted items", "{} elementos cifrados", self.vaults.len()),
        };
        if nav_item(ui, self.page == Page::Vault, "🔑", t("Kasa", "Vault", "Caja fuerte"), 0, &sub).clicked() {
            self.page = Page::Vault;
        }
        if telemetry::URL.is_some() {
            // At the bottom when there is room, otherwise right below the last item (never on top of it).
            ui.add_space((height - ui.min_rect().height() - 34.0).max(12.0));
            let mut on = self.telemetry.consent == Some(true);
            if ui
                .checkbox(
                    &mut on,
                    RichText::new(t("Anonim istatistik gönder", "Send anonymous stats", "Enviar estadísticas anónimas"))
                        .small()
                        .color(muted),
                )
                .changed()
            {
                self.telemetry.set_consent(on);
            }
            ui.add_space(14.0);
        }
    }

    fn header(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        // Disables the controls; the deletion would otherwise race with a new scan.
        let scanning = self.scanning.is_some() || self.deleting.is_some();
        card(ui, |ui| {
            let edit = egui::TextEdit::singleline(&mut self.root)
                .hint_text(t("Taranacak klasör", "Folder to scan", "Carpeta a analizar"))
                .margin(Margin::symmetric(10, 8));
            ui.add_sized([ui.available_width(), 34.0], edit);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let label = if self.scanning.is_some() {
                        t("Taranıyor…", "Scanning…", "Analizando…")
                    } else {
                        t("🔍  Tara", "🔍  Scan", "🔍  Analizar")
                    };
                    let scan =
                        egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                            .fill(ACCENT)
                            .min_size(vec2(110.0, 32.0));
                    if ui.add_enabled(!scanning, scan).clicked() {
                        self.start_scan(ctx.clone());
                    }
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new(t("Gözat…", "Browse…", "Examinar…")).min_size(vec2(0.0, 32.0)),
                        )
                        .clicked()
                        && let Some(dir) = rfd::FileDialog::new()
                            .set_directory(self.root.trim())
                            .pick_folder()
                    {
                        self.root = dir.display().to_string();
                    }
                    ui.add_space(8.0);
                    ui.with_layout(
                        egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
                        |ui| {
                            let quick = [
                                (t("Ana klasör", "Home", "Inicio"), dirs::home_dir()),
                                (t("İndirilenler", "Downloads", "Descargas"), dirs::download_dir()),
                                (t("Masaüstü", "Desktop", "Escritorio"), dirs::desktop_dir()),
                                (t("Belgeler", "Documents", "Documentos"), dirs::document_dir()),
                            ];
                            for (name, dir) in quick {
                                if let Some(dir) = dir
                                    && ui
                                        .add_enabled(
                                            !scanning,
                                            egui::Button::new(RichText::new(name).small())
                                                .wrap_mode(egui::TextWrapMode::Extend)
                                                .min_size(vec2(0.0, 32.0)),
                                        )
                                        .clicked()
                                {
                                    self.root = dir.display().to_string();
                                }
                            }
                        },
                    );
                });
            });
        });
        ui.add_space(12.0);

        let (found_n, found) = self.totals(None, false);
        let (sel_n, sel) = self.totals(None, true);
        let scanned = match &self.scanning {
            Some((_, p)) => p.load(Ordering::Relaxed),
            None => self.scanned_files,
        };
        ui.columns(3, |cols| {
            stat(
                &mut cols[0],
                t("Bulunan", "Found", "Encontrado"),
                &human_size(found),
                &tf!("{found_n} dosya", "{found_n} files", "{found_n} archivos"),
                None,
            );
            stat(
                &mut cols[1],
                t("Kazanılacak alan", "Space to free", "Espacio a liberar"),
                &human_size(sel),
                &tf!("{sel_n} dosya seçili", "{sel_n} files selected", "{sel_n} archivos seleccionados"),
                Some(ACCENT),
            );
            stat(
                &mut cols[2],
                t("Taranan dosya", "Files scanned", "Archivos analizados"),
                &scanned.to_string(),
                if self.scanning.is_some() {
                    t("tarama sürüyor…", "scan in progress…", "análisis en curso…")
                } else {
                    t("toplam", "total", "total")
                },
                None,
            );
        });
    }

    fn content(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        if let Some((_, progress)) = &self.scanning
            && self.phase.is_none()
        {
            let n = progress.load(Ordering::Relaxed);
            empty_state(ui, |ui| {
                ui.add(egui::Spinner::new().size(42.0).color(ACCENT));
                ui.add_space(10.0);
                ui.label(RichText::new(t("Taranıyor…", "Scanning…", "Analizando…")).size(20.0).strong());
                ui.label(
                    RichText::new(tf!("{n} dosya incelendi. Sonuçlar birazdan görünecek.", "{n} files checked. Results will appear shortly.", "{n} archivos revisados. Los resultados aparecerán en breve."))
                        .color(muted),
                );
            });
            return;
        }
        if !self.has_scanned {
            empty_state(ui, |ui| {
                if let Some(logo) = &self.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(96.0, 96.0)));
                }
                ui.add_space(6.0);
                ui.label(RichText::new(t("Diskinizde yer açın", "Free up space on your disk", "Libere espacio en su disco")).size(22.0).strong());
                ui.label(RichText::new(t("Bir klasör seçip Tara'ya basın. Silmeden önce ne kadar yer kazanacağınızı göreceksiniz.", "Pick a folder and press Scan. You will see how much space you gain before anything is deleted.", "Elija una carpeta y pulse Analizar. Verá cuánto espacio gana antes de borrar nada.")).color(muted));
            });
            return;
        }

        let tab = self.tab;
        // While scanning the list is incomplete (duplicates come last), so a click would miss most of it.
        let idle = self.scanning.is_none();
        // Buttons first (right to left), so the title gets only the space left and truncates instead of overlapping.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 36.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if ui.add_enabled(idle, egui::Button::new(t("Seçimi kaldır", "Deselect", "Quitar selección"))).clicked() {
                    deskmonk::set_selection(&mut self.items, Some(tab), false);
                }
                if ui.add_enabled(idle, egui::Button::new(t("Tümünü seç", "Select all", "Seleccionar todo"))).clicked() {
                    deskmonk::set_selection(&mut self.items, Some(tab), true);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let title = RichText::new(format!("{}  {}", icon(tab), tab.label()))
                        .size(20.0)
                        .strong();
                    ui.add(egui::Label::new(title).truncate());
                });
            },
        );
        ui.label(RichText::new(description(tab)).color(muted));
        if let Some(phase) = self.phase {
            let text = match phase {
                Phase::Walking => t(
                    "Taranıyor… Bulunanlar geldikçe listeye ekleniyor. Temizlik tarama bitince açılır.",
                    "Scanning… Findings are added as they come in. Cleaning is available once the scan ends.",
                    "Analizando… Los hallazgos se añaden a medida que llegan. La limpieza se habilita al terminar el análisis.",
                ),
                Phase::Comparing => t(
                    "Dosyalar listelendi, kopyalar karşılaştırılıyor… Temizlik tarama bitince açılır.",
                    "Files listed, comparing duplicates… Cleaning is available once the scan ends.",
                    "Archivos listados, comparando duplicados… La limpieza se habilita al terminar el análisis.",
                ),
            };
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
                ui.label(RichText::new(text).small().color(ACCENT));
            });
        }
        ui.add_space(6.0);

        // Duplicate items arrive sorted by group, so a header goes wherever the group number changes.
        let mut rows = Vec::new();
        let mut last_group = 0;
        for (i, item) in self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.category == tab)
        {
            let first_copy = item.group > 0 && item.group != last_group;
            if first_copy {
                rows.push(Row::Group(item.group));
                last_group = item.group;
            }
            rows.push(Row::Item(i, first_copy));
        }
        if rows.is_empty() && self.phase.is_some() {
            let (title, text) = match (self.phase, tab) {
                (_, Category::Duplicate) => (
                    t("Kopyalar aranıyor…", "Looking for duplicates…", "Buscando duplicados…"),
                    t(
                        "Dosya listesi bitince aynı boyuttaki dosyaların içerikleri karşılaştırılır. Diğer kategorilere şimdiden bakabilirsiniz.",
                        "Once the file list is done, files of the same size are compared by content. You can look at the other categories meanwhile.",
                        "Cuando termine la lista de archivos se comparará el contenido de los que tienen el mismo tamaño. Mientras tanto puede ver las demás categorías.",
                    ),
                ),
                _ => (
                    t("Taranıyor…", "Scanning…", "Analizando…"),
                    t(
                        "Bu kategoride henüz bir şey bulunmadı, tarama sürüyor.",
                        "Nothing found in this category yet, the scan is still running.",
                        "Aún no se encontró nada en esta categoría, el análisis sigue en curso.",
                    ),
                ),
            };
            empty_state(ui, |ui| {
                ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                ui.label(RichText::new(title).size(17.0).strong());
                ui.label(RichText::new(text).color(muted));
            });
            return;
        }
        if rows.is_empty() {
            empty_state(ui, |ui| {
                ui.label(
                    RichText::new("✔")
                        .size(44.0)
                        .color(Color32::from_rgb(52, 199, 89)),
                );
                ui.label(
                    RichText::new(t("Bu kategoride temizlenecek dosya yok", "Nothing to clean in this category", "No hay nada que limpiar en esta categoría"))
                        .size(17.0)
                        .strong(),
                );
            });
            return;
        }

        card(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                ROW_H,
                rows.len(),
                |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for row in &rows[range] {
                        match *row {
                            Row::Group(g) => group_row(ui, g, &self.items),
                            Row::Item(i, first_copy) => item_row(ui, &mut self.items[i], i, first_copy),
                        }
                    }
                },
            );
        });
    }

    /// Hardware as a terminal listing that types itself out.
    fn hardware_view(&mut self, ui: &mut Ui) {
        let mono = FontId::monospace(14.0);
        let mut lines: Vec<Vec<(String, Color32)>> = vec![vec![
            ("root@deskmonk:~$ ".into(), TERM_DIM),
            ("./hwinfo --brands".into(), TERM),
        ]];
        match &self.hardware {
            None => lines.push(vec![(t("[....] donanım taranıyor", "[....] scanning hardware", "[....] analizando hardware").into(), TERM_DIM)]),
            Some(parts) if parts.is_empty() => {
                lines.push(vec![(t("[FAIL] donanım bilgisi okunamadı", "[FAIL] could not read hardware info", "[FAIL] no se pudo leer la información del hardware").into(), Color32::from_rgb(255, 80, 80))])
            }
            Some(parts) => {
                lines.push(vec![(tf!("[ OK ] {} bileşen bulundu", "[ OK ] {} components found", "[ OK ] {} componentes encontrados", parts.len()), TERM_DIM)]);
                lines.push(Vec::new());
                for c in parts {
                    let kind = hardware::kind_label(c.kind).to_string();
                    // Turkish dotted i stays dotted in capitals (İŞLEMCİ, not IŞLEMCI).
                    let kind = if i18n::lang() == Lang::Tr { kind.replace('i', "İ") } else { kind };
                    let mut line = vec![(format!("{:<12} ", kind.to_uppercase()), TERM_DIM)];
                    if !c.brand.is_empty() {
                        let color = hardware::brand_of(&c.brand).map_or(TERM, |(_, [r, g, b])| Color32::from_rgb(r, g, b));
                        line.push((format!("[{}] ", c.brand.to_uppercase()), color));
                    }
                    line.push((c.name.clone(), TERM));
                    lines.push(line);
                    if !c.detail.is_empty() {
                        lines.push(vec![(format!("{:12} └─ {}", "", c.detail), TERM_DIM)]);
                    }
                }
            }
        }
        let speed_label = format!("{:<12} ", t("DİSK HIZI", "DISK SPEED", "VELOCIDAD"));
        match (&self.speed_run, &self.speed) {
            (Some((_, done)), _) => {
                let done = done.load(Ordering::Relaxed);
                let (step, part) = if done < SPEED_TEST { (t("yazılıyor", "writing", "escribiendo"), done) } else { (t("okunuyor", "reading", "leyendo"), done - SPEED_TEST) };
                let pct = part * 100 / SPEED_TEST;
                let bar = format!("[{}{}]", "#".repeat(pct as usize / 5), ".".repeat(20 - pct as usize / 5));
                lines.push(vec![(speed_label, TERM_DIM), (tf!("{bar} {step} %{pct}", "{bar} {step} {pct}%", "{bar} {step} {pct} %"), TERM)]);
            }
            (None, Some(Ok(s))) => {
                let rate = |b: f64| format!("{} MB/s", (b / 1e6).round());
                lines.push(vec![(speed_label, TERM_DIM), (tf!("yazma {} · okuma {}", "write {} · read {}", "escritura {} · lectura {}", rate(s.write), rate(s.read)), TERM)]);
                lines.push(vec![(tf!("{:12} └─ 1 GB sıralı yazma/okuma, sistem diski", "{:12} └─ 1 GB sequential write/read, system disk", "{:12} └─ 1 GB de escritura/lectura secuencial, disco del sistema", ""), TERM_DIM)]);
            }
            (None, Some(Err(e))) => {
                lines.push(vec![(speed_label, TERM_DIM), (format!("[FAIL] {e}"), Color32::from_rgb(255, 80, 80))]);
            }
            (None, None) => {}
        }
        lines.push(vec![("root@deskmonk:~$ ".into(), TERM_DIM)]);

        let now = ui.input(|i| i.time);
        let shown = ((now - self.hw_since) * 25.0) as usize + 1;
        // Blinking cursor; also keeps the typing animation and the "scanning" line moving.
        ui.ctx().request_repaint_after(Duration::from_millis(if shown < lines.len() { 16 } else { 500 }));
        let cursor = if (now * 2.0) as i64 % 2 == 0 { "█" } else { " " };
        egui::Frame::new()
            .fill(TERM_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(30, 92, 43)))
            .corner_radius(12)
            .inner_margin(Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
                    let count = shown.min(lines.len());
                    for (i, line) in lines.iter().take(count).enumerate() {
                        let mut job = egui::text::LayoutJob::default();
                        for (text, color) in line {
                            job.append(text, 0.0, egui::TextFormat::simple(mono.clone(), *color));
                        }
                        if i + 1 == count {
                            job.append(cursor, 0.0, egui::TextFormat::simple(mono.clone(), TERM));
                        }
                        ui.label(job);
                    }
                    if shown >= lines.len() {
                        ui.add_space(14.0);
                        let label = if self.speed.is_some() {
                            t("▶ Hız testini tekrarla", "▶ Run speed test again", "▶ Repetir prueba de velocidad")
                        } else {
                            t("▶ Disk hız testi", "▶ Disk speed test", "▶ Prueba de velocidad del disco")
                        };
                        let button = egui::Button::new(RichText::new(label).font(mono.clone()).color(TERM_BG)).fill(TERM);
                        if ui.add_enabled(self.speed_run.is_none(), button).clicked() {
                            let (tx, rx) = channel();
                            let done = Arc::new(AtomicU64::new(0));
                            let (counter, ctx) = (done.clone(), ui.ctx().clone());
                            std::thread::spawn(move || {
                                let _ = tx.send(deskmonk::speedtest::run(&deskmonk::speedtest::test_dir(), SPEED_TEST, &counter));
                                ctx.request_repaint();
                            });
                            self.speed_run = Some((rx, done));
                        }
                    }
                });
            });
    }

    /// Treemap of folder sizes: click a folder to go inside, right click to reveal it.
    fn map_view(&mut self, ui: &mut Ui) {
        let mono = FontId::monospace(13.0);
        let busy = self.map_scan.is_some();
        egui::Frame::new()
            .fill(TERM_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(30, 92, 43)))
            .corner_radius(12)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("root@deskmonk:~$ du --map").font(mono.clone()).color(TERM_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let go = egui::Button::new(RichText::new(t("▶ Haritala", "▶ Map", "▶ Mapear")).font(mono.clone()).color(TERM_BG)).fill(TERM);
                        if ui.add_enabled(!busy, go).clicked() {
                            self.start_map(ui.ctx().clone());
                        }
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new(t("Gözat…", "Browse…", "Examinar…")).font(mono.clone()))).clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_directory(self.root.trim()).pick_folder()
                        {
                            self.root = dir.display().to_string();
                            self.start_map(ui.ctx().clone());
                        }
                        let edit = egui::TextEdit::singleline(&mut self.root).font(mono.clone()).text_color(TERM);
                        ui.add_sized([ui.available_width(), 26.0], edit);
                    });
                });
                ui.add_space(6.0);
                let dir = if self.map.is_some() { self.map_root.clone() } else { PathBuf::from(self.root.trim()) };
                if let Some((total, free)) = deskmonk::disk_space(&dir).filter(|(total, _)| *total > 0) {
                    disk_bar(ui, total, free, &mono);
                    ui.add_space(6.0);
                }
                if let Some((_, files)) = &self.map_scan {
                    let n = files.load(Ordering::Relaxed);
                    let line = tf!("[....] {n} dosya ölçüldü {}", "[....] {n} files measured {}", "[....] {n} archivos medidos {}", if (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0 { "█" } else { " " });
                    ui.label(RichText::new(line).font(mono.clone()).color(TERM));
                    return;
                }
                let Some(tree) = &self.map else {
                    ui.label(RichText::new(format!("[FAIL] {}", self.status)).font(mono.clone()).color(DANGER));
                    return;
                };

                // Folder being looked at, and the chain of folders above it.
                let mut chain = vec![tree];
                for &i in &self.map_path {
                    chain.push(&chain[chain.len() - 1].children[i]);
                }
                let node = chain[chain.len() - 1];
                let folder = chain.iter().skip(1).fold(self.map_root.clone(), |p, n| p.join(&n.name));
                let mut go_up = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (depth, n) in chain.iter().enumerate() {
                        let last = depth + 1 == chain.len();
                        let text = RichText::new(&n.name).font(mono.clone()).color(if last { TERM } else { TERM_DIM });
                        if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            go_up = Some(depth);
                        }
                        if !last {
                            ui.label(RichText::new(" / ").font(mono.clone()).color(TERM_DIM));
                        }
                    }
                    ui.label(RichText::new(format!("  [{}]", human_size(node.size))).font(mono.clone()).color(TERM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new(reveal_label()).font(mono.clone())).on_hover_text(folder.display().to_string()).clicked() {
                            reveal_in_folder(&folder);
                        }
                    });
                });
                ui.add_space(8.0);

                let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                let sizes: Vec<u64> = node.children.iter().map(|c| c.size).collect();
                let boxes = diskmap::treemap(&sizes, [area.min.x, area.min.y, area.width(), area.height()]);
                let mut go_in = None;
                for (i, (child, b)) in node.children.iter().zip(boxes).enumerate() {
                    let r = Rect::from_min_size(Pos2::new(b[0], b[1]), vec2(b[2], b[3])).shrink(1.5);
                    if r.width() < 1.0 || r.height() < 1.0 {
                        continue;
                    }
                    let resp = ui.interact(r, ui.id().with(("map", self.map_path.len(), i)), Sense::click());
                    let color = NEON[i % NEON.len()];
                    let fill = color.gamma_multiply(if resp.hovered() { 0.45 } else { 0.22 });
                    ui.painter().rect(r, 4.0, fill, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                    if r.width() > 50.0 && r.height() > 34.0 {
                        let p = ui.painter().with_clip_rect(r.shrink(4.0));
                        p.text(r.min + vec2(7.0, 6.0), Align2::LEFT_TOP, &child.name, mono.clone(), Color32::WHITE);
                        p.text(r.min + vec2(7.0, 22.0), Align2::LEFT_TOP, human_size(child.size), FontId::monospace(11.5), color);
                    }
                    let can_enter = child.dir && !child.children.is_empty();
                    let pct = child.size as f64 / node.size.max(1) as f64 * 100.0;
                    let reveal = reveal_label();
                    let hint = if can_enter {
                        tf!("Tıkla: içine gir · Sağ tık: {reveal}", "Click: open · Right-click: {reveal}", "Clic: abrir · Clic derecho: {reveal}")
                    } else {
                        tf!("Sağ tık: {reveal}", "Right-click: {reveal}", "Clic derecho: {reveal}")
                    };
                    let resp = resp.on_hover_text(tf!("{}\n{} · %{pct:.1}\n{hint}", "{}\n{} · {pct:.1}%\n{hint}", "{}\n{} · {pct:.1} %\n{hint}", child.name, human_size(child.size)));
                    let resp = if can_enter { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
                    if resp.clicked() && can_enter {
                        go_in = Some(i);
                    }
                    resp.context_menu(|ui| {
                        if ui.button(reveal_label()).clicked() {
                            // The merged "small items" box has no path of its own.
                            let path = folder.join(&child.name);
                            reveal_in_folder(if path.exists() { &path } else { &folder });
                            ui.close();
                        }
                    });
                }
                if let Some(depth) = go_up {
                    self.map_path.truncate(depth);
                } else if let Some(i) = go_in {
                    self.map_path.push(i);
                }
            });
    }

    /// Advances the pomodoro and notifies when a focus or break ends. Called from `logic` too, so it runs while
    /// the window is hidden.
    fn monk_tick(&mut self, ctx: &egui::Context) {
        let event = self.monk.tick(Instant::now());
        match event {
            Some(focus::Event::FocusDone) => {
                self.telemetry.used("gui", "focus", "");
                notify(
                    t("Odak tamamlandı 🧘", "Focus done 🧘", "Enfoque terminado 🧘"),
                    t("Mola zamanı. Biraz dinlen.", "Time for a break. Rest a little.", "Hora del descanso. Descanse un poco."),
                )
            }
            Some(focus::Event::BreakDone) => notify(
                t("Mola bitti", "Break over", "Descanso terminado"),
                t("Yeni bir odak oturumuna hazır.", "Ready for another focus session.", "Listo para otra sesión de enfoque."),
            ),
            None => {}
        }
        if event.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        if self.monk.phase != focus::Phase::Idle && !self.monk.paused() {
            // Smooth animation on the page; elsewhere just keep the clock (and the sidebar) ticking.
            ctx.request_repaint_after(if self.page == Page::Monk { Duration::from_millis(16) } else { Duration::from_secs(1) });
        }
    }

    /// The sandboxed build may read a folder only after the user picked it, so the first scan of a folder opens
    /// the folder panel right there (one click on Open). Returns the folder to use, None if the user declined.
    fn granted(&mut self, dir: PathBuf) -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        if deskmonk::sandboxed() {
            if std::fs::read_dir(&dir).is_ok() {
                access::remember(&dir);
                return Some(dir);
            }
            let Some(picked) = rfd::FileDialog::new()
                .set_directory(&dir)
                .set_title(t("deskMonk'un taraması için klasörü onaylayın", "Confirm the folder for deskMonk to scan", "Confirme la carpeta que deskMonk analizará"))
                .pick_folder()
            else {
                self.status = t(
                    "macOS, deskMonk'un bir klasöre bakması için sizin seçmenizi istiyor. Tarama için klasörü seçip Aç'a basın.",
                    "macOS lets deskMonk look into a folder only after you pick it. Pick the folder and click Open to scan.",
                    "macOS solo deja a deskMonk mirar una carpeta después de que usted la elija. Elija la carpeta y pulse Abrir.",
                )
                .into();
                return None;
            };
            access::remember(&picked);
            self.root = picked.display().to_string();
            return Some(picked);
        }
        Some(dir)
    }

    /// Checks the license at startup, and every few seconds for an hour after a checkout was opened.
    fn poll_license(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "appstore")]
        for event in store::take() {
            match event {
                store::Event::ProUntil(until) => self.plan.set_license(String::new(), until),
                store::Event::Prices(monthly, yearly) => self.store_prices = (monthly, yearly),
                store::Event::Bought => {
                    self.store_busy = false;
                    self.telemetry.used("gui", "bought", "appstore");
                }
                store::Event::Cancelled => self.store_busy = false,
                store::Event::Pending => {
                    self.store_busy = false;
                    self.pro_error = t(
                        "Satın alma onay bekliyor (ör. Aile Paylaşımı). Onaylanınca Pro kendiliğinden açılır.",
                        "The purchase is waiting for approval (e.g. Family Sharing). Pro turns on by itself once approved.",
                        "La compra espera aprobación (p. ej., En familia). Pro se activa solo al aprobarse.",
                    )
                    .into();
                }
                store::Event::Failed => {
                    self.store_busy = false;
                    self.pro_error = t("Satın alma tamamlanamadı. Lütfen tekrar deneyin.", "The purchase could not be completed. Please try again.", "No se pudo completar la compra. Inténtelo de nuevo.").into();
                }
            }
        }
        if let Some(rx) = &self.pro_check
            && let Ok(result) = rx.try_recv()
        {
            self.pro_check = None;
            let manual = std::mem::take(&mut self.pro_manual);
            match result {
                Ok((key, until)) => {
                    if manual && until == 0 {
                        self.pro_error = t("Bu anahtarın etkin bir aboneliği yok", "This key has no active subscription", "Esta clave no tiene una suscripción activa").into();
                    } else if manual || !key.is_empty() || self.plan.key.is_empty() {
                        self.plan.set_license(key, until);
                    }
                    if self.plan.is_pro() {
                        self.pro_waiting = None;
                        self.pro_key.clear();
                    }
                }
                // Offline or server trouble: the saved license stays.
                Err(e) if manual => self.pro_error = e,
                Err(_) => {}
            }
        }
        let now = ctx.input(|i| i.time);
        let waiting = self.pro_waiting.is_some_and(|since| now - since < 3600.0);
        if plan::URL.is_some() && self.pro_check.is_none() && (self.pro_checked < 0.0 || waiting && now - self.pro_checked > 3.0) {
            self.check_license(ctx, self.plan.key.clone(), false);
        }
        if waiting || self.pro_check.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn check_license(&mut self, ctx: &egui::Context, key: String, manual: bool) {
        self.pro_checked = ctx.input(|i| i.time);
        self.pro_manual = manual;
        let (tx, rx) = channel();
        let (id, ctx) = (self.telemetry.install_id.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(plan::validate(&key, &id));
            ctx.request_repaint();
        });
        self.pro_check = Some(rx);
    }

    fn pro_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::new("pro")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(26))).show(ctx, |ui| {
            ui.set_width(400.0);
            self.pro_body(ui);
            ui.add_space(6.0);
            if wide_button(ui, t("Kapat", "Close", "Cerrar"), None) {
                ui.close();
            }
        });
        if modal.should_close() {
            self.pro_open = false;
            self.pro_error.clear();
        }
    }

    /// The License page, opened from the top bar: activation with a key, plan, limits and buying.
    fn license_view(&mut self, ui: &mut Ui) {
        centered_column(ui, 460.0, |ui| {
            ui.add_space(10.0);
            self.pro_body(ui);
        });
    }

    /// Plan, limits, buying, and the license key (enter, copy, manage, remove): shared by the Pro window and the License page.
    fn pro_body(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        let gold = Color32::from_rgb(245, 184, 46);
        let pro = self.plan.is_pro();
        let (clean_left, vaults_left) = (self.plan.clean_left(), self.plan.vaults_left());
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("⭐").size(40.0).color(gold));
            ui.label(RichText::new("deskMonk Pro").size(22.0).strong());
            let sub = if pro && self.plan.pro_until > 0 {
                tf!("Pro etkin · {} tarihine kadar", "Pro is active · until {}", "Pro activo · hasta {}", plan::date(self.plan.pro_until))
            } else if pro {
                t("Pro etkin", "Pro is active", "Pro activo").into()
            } else {
                t("Sınırları kaldırın, deskMonk'u destekleyin.", "Remove the limits and support deskMonk.", "Quite los límites y apoye a deskMonk.").into()
            };
            ui.label(RichText::new(sub).color(muted));
        });
        ui.add_space(14.0);
        if !pro && !APPSTORE {
            self.activation_card(ui, muted);
        }
        let rows = [
            (
                "🗑",
                match clean_left {
                    Some(left) => tf!("Temizlik: bugün {} kaldı (günde 2 GB)", "Cleaning: {} left today (2 GB a day)", "Limpieza: quedan {} hoy (2 GB al día)", human_size(left)),
                    None => t("Temizlik", "Cleaning", "Limpieza").into(),
                },
                t("sınırsız", "unlimited", "ilimitada"),
            ),
            (
                "🔑",
                match vaults_left {
                    Some(left) => tf!("Kasa: bugün {left}/3 şifreleme, en fazla 200 MB", "Vault: {left}/3 encryptions left today, up to 200 MB", "Caja fuerte: {left}/3 cifrados hoy, hasta 200 MB"),
                    None => t("Kasa", "Vault", "Caja fuerte").into(),
                },
                t("sınırsız, her boyut", "unlimited, any size", "ilimitada, cualquier tamaño"),
            ),
            (
                "🔒",
                if pro { t("Uygulama kilidi", "App lock", "Bloqueo de apps") } else { t("Uygulama kilidi: 1 uygulama", "App lock: 1 app", "Bloqueo de apps: 1 app") }.into(),
                t("istediğiniz kadar", "as many as you like", "todas las que quiera"),
            ),
        ];
        let shown = if APPSTORE { &rows[..2] } else { &rows[..] };
        for (icon, free, with_pro) in shown.iter().cloned() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).size(18.0));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(free).color(if pro { ui.visuals().text_color() } else { muted }));
                        ui.label(RichText::new(tf!("Pro: {with_pro}", "Pro: {with_pro}", "Pro: {with_pro}")).strong().color(gold));
                    });
                });
            });
            ui.add_space(6.0);
        }
        ui.add_space(8.0);
        if pro {
            if APPSTORE {
                if wide_button(ui, t("Aboneliği yönet", "Manage subscription", "Gestionar suscripción"), Some(ACCENT)) {
                    open_url("https://apps.apple.com/account/subscriptions");
                }
            } else if !self.plan.key.is_empty() {
                // The key turns Pro on on up to 3 computers, so it is shown here to copy.
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Lisans anahtarınız:", "Your license key:", "Su clave de licencia:")).color(muted));
                    ui.label(RichText::new(&self.plan.key).monospace().strong());
                    if ui.small_button(t("Kopyala", "Copy", "Copiar")).clicked() {
                        ui.ctx().copy_text(self.plan.key.clone());
                    }
                });
                ui.add_space(6.0);
            }
            if !APPSTORE && !self.plan.key.is_empty() && wide_button(ui, t("Aboneliği yönet", "Manage subscription", "Gestionar suscripción"), Some(ACCENT)) {
                let key = self.plan.key.clone();
                // shortcut: a failed portal request is only logged; the app shows nothing.
                std::thread::spawn(move || match plan::portal_url(&key) {
                    Ok(url) => open_url(&url),
                    Err(e) => eprintln!("portal: {e}"),
                });
            }
            // Like bubbleTranslate: frees one of the key's 3 computers for another one.
            if !APPSTORE && !self.plan.key.is_empty() {
                let remove = ui.add_sized([ui.available_width(), 32.0], egui::Button::new(t("Bu bilgisayardan kaldır", "Remove from this computer", "Quitar de este ordenador")).frame(false));
                if remove.on_hover_text(t(
                    "Anahtarın 3 bilgisayarlık yerinden birini boşaltır; bu bilgisayar Ücretsiz plana döner.",
                    "Frees one of the key's 3 computers; this computer goes back to the Free plan.",
                    "Libera uno de los 3 ordenadores de la clave; este vuelve al plan gratuito.",
                )).clicked() {
                    match plan::deactivate(&self.plan.key, &self.telemetry.install_id) {
                        Ok(()) => self.plan.set_license(String::new(), 0),
                        Err(e) => self.pro_error = e,
                    }
                }
            }
            if !self.pro_error.is_empty() {
                ui.label(RichText::new(&self.pro_error).color(DANGER));
            }
        } else {
            ui.columns(2, |cols| {
                for (col, yearly) in cols.iter_mut().zip([false, true]) {
                    // The App Store shows its own localized price; the website build shows the Paddle price.
                    let (store_m, store_y) = (self.store_prices.0.as_deref().unwrap_or("…"), self.store_prices.1.as_deref().unwrap_or("…"));
                    let label = if yearly {
                        tf!("{} / yıl", "{} / year", "{} / año", if APPSTORE { store_y } else { plan::PRICE_YEARLY })
                    } else {
                        tf!("{} / ay", "{} / month", "{} / mes", if APPSTORE { store_m } else { plan::PRICE_MONTHLY })
                    };
                    let button = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE)).fill(ACCENT).corner_radius(10);
                    if col.add_enabled(!self.store_busy, button.min_size(vec2(col.available_width(), 42.0))).clicked() {
                        self.pro_error.clear();
                        if APPSTORE {
                            self.store_busy = true;
                            store_buy(yearly);
                        } else if let Some(url) = plan::buy_url(&self.telemetry.install_id, yearly) {
                            open_url(&url);
                            self.pro_waiting = Some(col.input(|i| i.time));
                        }
                    }
                    col.vertical_centered(|ui| {
                        let note = t("istediğiniz zaman iptal", "cancel anytime", "cancele cuando quiera");
                        ui.label(RichText::new(note).small().color(muted));
                    });
                }
            });
            if self.store_busy {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(t("App Store bekleniyor…", "Waiting for the App Store…", "Esperando al App Store…")).color(muted));
                });
            }
            if self.pro_waiting.is_some() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(t(
                        "Ödeme bekleniyor… tamamlanınca Pro kendiliğinden açılır.",
                        "Waiting for the payment… Pro turns on by itself when it is done.",
                        "Esperando el pago… Pro se activa solo cuando termine.",
                    )).color(muted));
                });
            }
            if APPSTORE {
                ui.add_space(12.0);
                self.store_terms(ui, muted);
                if !self.pro_error.is_empty() {
                    ui.label(RichText::new(&self.pro_error).color(DANGER));
                }
            }
        }
    }

    /// Where a bought key is entered: first thing on the License page and in the Pro window (not in the App Store build).
    fn activation_card(&mut self, ui: &mut Ui, muted: Color32) {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t("🔑 Lisans anahtarı ile etkinleştir", "🔑 Activate with a license key", "🔑 Activar con una clave de licencia")).size(16.0).strong());
            ui.label(RichText::new(t(
                "Ödemeden sonra anahtar, ödeme sayfasında ve e-postanızda (DM-…) gelir. Buraya yapıştırın.",
                "After paying, your key (DM-…) is on the payment page and in your email. Paste it here.",
                "Tras el pago, su clave (DM-…) aparece en la página de pago y en su correo. Péguela aquí.",
            )).color(muted));
            ui.add_space(6.0);
            self.key_form(ui);
        });
        ui.add_space(10.0);
    }

    /// On every page until Pro: a buyer with a key from the payment page should not have to look for where it goes.
    /// The heading is the wording the payment page tells them to look for (worker/i18n, pay.step2).
    fn license_strip(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            let heading = RichText::new(t("🔑 Lisans anahtarım var", "🔑 Have a license key?", "🔑 ¿Tiene una clave de licencia?")).strong();
            // In a half-screen window the heading would leave the field too narrow to read a key.
            if ui.available_width() < 520.0 {
                ui.label(heading);
                ui.add_space(6.0);
                self.key_form(ui);
            } else {
                ui.horizontal(|ui| {
                    ui.label(heading);
                    ui.add_space(6.0);
                    self.key_form(ui);
                });
            }
        });
        ui.add_space(12.0);
    }

    /// The key field and Activate button, with the check's error below them.
    fn key_form(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let busy = self.pro_check.is_some() && self.pro_manual;
                let field = ui.add(egui::TextEdit::singleline(&mut self.pro_key).hint_text("DM-XXXX-XXXX-XXXX").desired_width(ui.available_width() - 110.0));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let activate = egui::Button::new(RichText::new(t("Etkinleştir", "Activate", "Activar")).strong().color(Color32::WHITE)).fill(ACCENT);
                if (ui.add_enabled(!busy, activate).clicked() || enter) && !busy && !self.pro_key.trim().is_empty() {
                    self.pro_error.clear();
                    self.check_license(&ui.ctx().clone(), self.pro_key.trim().to_string(), true);
                }
                if busy {
                    ui.add(egui::Spinner::new());
                }
            });
            if !self.pro_error.is_empty() {
                ui.label(RichText::new(&self.pro_error).color(DANGER));
            }
        });
    }

    /// Cleaning or a vault job is running: no restart for an update now.
    fn busy(&self) -> bool {
        self.deleting.is_some() || self.vault_job.is_some()
    }

    /// Asks once per update to restart into it (or, where it couldn't install itself, to download it); "Later" leaves
    /// only the top bar button, and an installed update opens at the next start anyway.
    fn update_dialog(&mut self, ctx: &egui::Context) {
        let (Some(found), Some(site)) = (self.new_version.clone(), plan::URL) else { return };
        let (_, _, _, muted) = palette(ctx.global_style().visuals.dark_mode);
        let (installed, v) = match &found {
            update::Found::Installed(v) => (true, v.clone()),
            update::Found::Available(v) => (false, v.clone()),
        };
        let busy = self.busy();
        let modal = egui::Modal::new(egui::Id::new("update")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(26))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("⬆").size(36.0).color(Color32::from_rgb(34, 160, 90)));
                if installed {
                    ui.label(RichText::new(tf!("deskMonk {v} kuruldu", "deskMonk {v} is installed", "deskMonk {v} está instalado")).size(20.0).strong());
                    ui.label(RichText::new(tf!(
                        "Yeni sürüm hazır. Şimdi yeniden başlatın ya da bir sonraki açılışta kendiliğinden gelsin. Ayarlarınız ve lisansınız korunur.",
                        "The new version is ready. Restart now, or it opens by itself next time. Your settings and license stay.",
                        "La nueva versión está lista. Reinicie ahora o se abrirá sola la próxima vez. Sus ajustes y su licencia se conservan.",
                    )).color(muted));
                } else {
                    ui.label(RichText::new(tf!("deskMonk {v} çıktı", "deskMonk {v} is available", "Ya está disponible deskMonk {v}")).size(20.0).strong());
                    ui.label(RichText::new(tf!(
                        "Sizdeki sürüm {}. Yeni sürümü indirip kurun; ayarlarınız ve lisansınız korunur.",
                        "You have {}. Download and install the new version; your settings and license stay.",
                        "Tiene la {}. Descargue e instale la nueva versión; sus ajustes y su licencia se conservan.",
                        env!("CARGO_PKG_VERSION")
                    )).color(muted));
                }
            });
            ui.add_space(14.0);
            if installed {
                if busy {
                    ui.label(RichText::new(t("Önce süren işlem bitsin.", "Let the running job finish first.", "Deje que termine la tarea en curso.")).color(muted));
                }
                if ui.add_enabled_ui(!busy, |ui| wide_button(ui, t("Şimdi yeniden başlat", "Restart now", "Reiniciar ahora"), Some(Color32::from_rgb(34, 160, 90)))).inner {
                    update::restart();
                }
            } else if wide_button(ui, t("İndir", "Download", "Descargar"), Some(Color32::from_rgb(34, 160, 90))) {
                open_url(&format!("{site}/#download"));
                ui.close();
            }
            if wide_button(ui, t("Daha sonra", "Later", "Más tarde"), None) {
                ui.close();
            }
        });
        if modal.should_close() {
            self.update_seen = Some(found);
        }
    }

    /// "Contact us": a message to the developer, mailed by deskmonk.app (worker/src/mail.ts).
    fn contact_dialog(&mut self, ctx: &egui::Context) {
        let (_, _, _, muted) = palette(ctx.global_style().visuals.dark_mode);
        if let Some(result) = self.contact_send.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.contact_send = None;
            if result.is_ok() {
                self.contact_message.clear();
            }
            self.contact_result = Some(result);
        }
        let modal = egui::Modal::new(egui::Id::new("contact")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(26))).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(t("Bize yazın", "Contact us", "Escríbanos")).size(20.0).strong());
            ui.label(RichText::new(t(
                "Soru, hata veya öneri: mesajınız doğrudan geliştiriciye gider.",
                "A question, a bug or an idea: your message goes straight to the developer.",
                "Una pregunta, un error o una idea: su mensaje llega directamente al desarrollador.",
            )).color(muted));
            ui.add_space(12.0);
            ui.label(t("E-postanız (yanıt için, isteğe bağlı)", "Your email (for a reply, optional)", "Su correo (para responderle, opcional)"));
            ui.add(egui::TextEdit::singleline(&mut self.contact_email).hint_text("you@example.com").desired_width(f32::INFINITY));
            ui.add_space(8.0);
            ui.label(t("Mesajınız", "Your message", "Su mensaje"));
            ui.add(egui::TextEdit::multiline(&mut self.contact_message).desired_rows(6).desired_width(f32::INFINITY).char_limit(5000));
            ui.add_space(10.0);
            let busy = self.contact_send.is_some();
            match &self.contact_result {
                Some(Ok(())) => {
                    ui.label(RichText::new(t("Gönderildi, teşekkürler!", "Sent, thank you!", "¡Enviado, gracias!")).color(Color32::from_rgb(34, 160, 90)));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(DANGER));
                }
                None => {}
            }
            if busy {
                ui.add(egui::Spinner::new());
            }
            let ready = !busy && !self.contact_message.trim().is_empty();
            if ui.add_enabled_ui(ready, |ui| wide_button(ui, t("Gönder", "Send", "Enviar"), Some(ACCENT))).inner {
                let (tx, rx) = channel();
                let (email, message, id, ctx) = (self.contact_email.trim().to_string(), self.contact_message.trim().to_string(), self.telemetry.install_id.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(plan::contact(&email, &message, &id));
                    ctx.request_repaint();
                });
                self.contact_send = Some(rx);
                self.contact_result = None;
            }
            if wide_button(ui, t("Kapat", "Close", "Cerrar"), None) {
                ui.close();
            }
        });
        if modal.should_close() {
            self.contact_open = false;
            self.contact_result = None;
        }
    }

    /// What Apple requires next to a subscription offer: renewal terms, restore, Terms of Use and privacy policy.
    fn store_terms(&mut self, ui: &mut Ui, muted: Color32) {
        if ui.add_enabled(!self.store_busy, egui::Button::new(t("Satın alımları geri yükle", "Restore purchases", "Restaurar compras"))).clicked() {
            self.pro_error.clear();
            store_restore();
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t(
            "deskMonk Pro otomatik yenilenen bir aboneliktir (aylık veya yıllık). Ödeme Apple Hesabınızdan alınır. Dönem bitmeden en az 24 saat önce iptal edilmezse kendiliğinden yenilenir; App Store hesap ayarlarınızdan yönetebilir veya iptal edebilirsiniz.",
            "deskMonk Pro is an auto-renewing subscription (monthly or yearly). Payment is charged to your Apple Account. It renews automatically unless cancelled at least 24 hours before the end of the period; manage or cancel it in your App Store account settings.",
            "deskMonk Pro es una suscripción de renovación automática (mensual o anual). El pago se carga a su cuenta de Apple. Se renueva automáticamente salvo que se cancele al menos 24 horas antes del final del periodo; gestiónela o cancélela en los ajustes de su cuenta del App Store.",
        )).small().color(muted));
        ui.horizontal(|ui| {
            ui.hyperlink_to(RichText::new(t("Kullanım Koşulları (EULA)", "Terms of Use (EULA)", "Términos de uso (EULA)")).small(), APPLE_EULA);
            ui.label(RichText::new("·").small().color(muted));
            ui.hyperlink_to(RichText::new(t("Gizlilik Politikası", "Privacy Policy", "Política de privacidad")).small(), PRIVACY_POLICY);
        });
    }

    /// Password pop-up for a vault clicked in the list; closes by itself once the vault is open.
    fn vault_unlock_dialog(&mut self, ctx: &egui::Context) {
        let Some(target) = self.vault_target.clone() else {
            self.vault_prompt = false;
            return;
        };
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let shown = name.strip_suffix(&format!(".{}", vault::EXT)).unwrap_or(&name).to_string();
        let icon = self.icons.get(ctx, &target);
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("vault_unlock")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(28))).show(ctx, |ui| {
            ui.set_width(320.0);
            form_header(ui, Some((icon, &shown)), &shown, t("Açmak için kasanın şifresini girin", "Enter the vault's password to open it", "Escriba la contraseña de la caja fuerte para abrirla"));
            if let Some((_, done, total)) = &self.vault_job {
                let total = total.load(Ordering::Relaxed);
                let frac = if total == 0 { 0.0 } else { (done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0) };
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(28.0));
                    ui.label(t("Açılıyor…", "Opening…", "Abriendo…"));
                });
                ui.add(egui::ProgressBar::new(frac).show_percentage());
                return;
            }
            let wrong = !self.vault_error.is_empty();
            pw_field(ui, &mut self.vault_pw, t("Kasa şifresi", "Vault password", "Contraseña de la caja fuerte"), wrong).request_focus();
            if wrong {
                ui.label(RichText::new(&self.vault_error).color(DANGER));
            }
            ui.add_space(6.0);
            // The field keeps focus every frame, so Enter is read directly rather than via lost_focus.
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (wide_button(ui, t("🔓  Kilidi aç", "🔓  Unlock", "🔓  Desbloquear"), Some(ACCENT)) || enter) && !self.vault_pw.is_empty() {
                self.start_vault(ctx.clone());
            }
            cancel = wide_button(ui, t("Vazgeç", "Cancel", "Cancelar"), None);
        });
        // Escape or a click outside cancels too, but not while the vault is being opened.
        if self.vault_job.is_none() && (cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.vault_prompt = false;
            self.vault_target = None;
            self.vault_pw.clear();
            self.vault_error.clear();
        }
    }

    fn vault_pick(&mut self, path: PathBuf) {
        // A vault dropped or chosen by hand joins the list, e.g. one made before the list existed.
        if vault::is_vault(&path) && !self.vaults.contains(&path) {
            self.vaults.insert(0, path.clone());
            if let Err(e) = vault::save_list(&self.vaults) {
                eprintln!("could not save the vault list: {e}");
            }
        }
        self.vault_target = Some(path);
        self.vault_pw.clear();
        self.vault_pw2.clear();
        self.vault_error.clear();
        self.vault_done = None;
    }

    fn start_vault(&mut self, ctx: egui::Context) {
        let Some(src) = self.vault_target.clone() else { return };
        let password = std::mem::take(&mut self.vault_pw);
        self.vault_pw2.clear();
        self.vault_error.clear();
        let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (tx, rx) = channel();
        let (d, n) = (done.clone(), total.clone());
        let max = (!self.plan.is_pro()).then_some(plan::FREE_VAULT_SIZE);
        std::thread::spawn(move || {
            let size = vault::work_size(&src);
            n.store(size, Ordering::Relaxed);
            let result = if vault::is_vault(&src) {
                vault::decrypt(&src, &password, &d)
            } else if max.is_some_and(|max| size > max) {
                Err(plan::vault_too_big(size))
            } else {
                vault::encrypt(&src, &password, &d)
            };
            // The vault replaces the readable copy, and the opened copy replaces the vault; the Trash keeps either recoverable.
            let _ = tx.send(result.map(|out| (out, deskmonk::to_trash(&src).err().map(|e| e.to_string()))));
            ctx.request_repaint();
        });
        self.vault_job = Some((rx, done, total));
    }

    fn vault_view(&mut self, ui: &mut Ui) {
        let (_, _, border, muted) = palette(ui.visuals().dark_mode);
        ui.label(RichText::new(t("🔑  Kasa", "🔑  Vault", "🔑  Caja fuerte")).size(20.0).strong());
        ui.label(RichText::new(t(
            "Dosya ve klasörleri bir şifreyle şifreleyin. Kasa dosyası yalnızca o şifreyle açılır; şifreyi unutursanız deskMonk dahil kimse açamaz.",
            "Encrypt files and folders with a password. The vault file opens only with that password; if you forget it, nobody can open it, not even deskMonk.",
            "Cifre archivos y carpetas con una contraseña. La caja fuerte solo se abre con esa contraseña; si la olvida, nadie podrá abrirla, ni siquiera deskMonk.",
        )).color(muted));
        ui.add_space(14.0);
        if let Some((out, encrypted, trash_error)) = &self.vault_done {
            let out = out.clone();
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let name = out.file_name().unwrap_or_default().to_string_lossy();
                    let text = if *encrypted {
                        tf!("✔  Şifrelendi: {name}", "✔  Encrypted: {name}", "✔  Cifrado: {name}")
                    } else {
                        tf!("✔  Açıldı: {name}", "✔  Opened: {name}", "✔  Abierto: {name}")
                    };
                    ui.label(RichText::new(text).size(15.0).strong().color(Color32::from_rgb(52, 199, 89)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(reveal_label()).clicked() {
                            reveal_in_folder(&out);
                        }
                    });
                });
                match trash_error {
                    Some(e) => {
                        ui.label(RichText::new(tf!("Kaynak çöp kutusuna taşınamadı: {e}", "Could not move the source to the Trash: {e}", "No se pudo mover el origen a la papelera: {e}")).color(DANGER));
                    }
                    None if *encrypted => {
                        ui.label(RichText::new(t(
                            "Şifresiz asıl dosya çöp kutusuna taşındı. Okunabilir kopya kalmasın diye çöp kutusunu boşaltın.",
                            "The unencrypted original went to the Trash. Empty the Trash so no readable copy is left.",
                            "El original sin cifrar se movió a la papelera. Vacíe la papelera para que no quede ninguna copia legible.",
                        )).color(muted));
                    }
                    None => {
                        ui.label(RichText::new(t("Kasa dosyası çöp kutusuna taşındı.", "The vault file went to the Trash.", "El archivo de la caja fuerte se movió a la papelera.")).color(muted));
                    }
                }
            });
            ui.add_space(12.0);
        }
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_space(10.0);
            if let Some((_, done, total)) = &self.vault_job {
                let encrypting = self.vault_target.as_deref().is_some_and(|p| !vault::is_vault(p));
                let total = total.load(Ordering::Relaxed);
                let frac = if total == 0 { 0.0 } else { (done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0) };
                centered_column(ui, 340.0, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new().size(36.0));
                        ui.add_space(8.0);
                        ui.label(RichText::new(if encrypting { t("Şifreleniyor…", "Encrypting…", "Cifrando…") } else { t("Açılıyor…", "Opening…", "Abriendo…") }).size(18.0).strong());
                    });
                    ui.add_space(8.0);
                    ui.add(egui::ProgressBar::new(frac).show_percentage());
                });
            } else if let Some(target) = self.vault_target.clone().filter(|_| !self.vault_prompt) {
                let encrypting = !vault::is_vault(&target);
                let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let icon = self.icons.get(ui.ctx(), &target);
                let wrong = !self.vault_error.is_empty();
                centered_column(ui, 340.0, |ui| {
                    let subtitle = match (encrypting, target.is_dir()) {
                        (true, true) => t("Klasör şifrelenecek", "This folder will be encrypted", "Esta carpeta se cifrará"),
                        (true, false) => t("Dosya şifrelenecek", "This file will be encrypted", "Este archivo se cifrará"),
                        _ => t("Açmak için kasanın şifresini girin", "Enter the vault's password to open it", "Escriba la contraseña de la caja fuerte para abrirla"),
                    };
                    form_header(ui, Some((icon, &name)), &name, subtitle);
                    let enter = if encrypting {
                        pw_field(ui, &mut self.vault_pw, t("Şifre (en az 4 karakter)", "Password (at least 4 characters)", "Contraseña (mínimo 4 caracteres)"), wrong);
                        pw_field(ui, &mut self.vault_pw2, t("Şifre tekrar", "Repeat password", "Repita la contraseña"), wrong)
                    } else {
                        pw_field(ui, &mut self.vault_pw, t("Kasa şifresi", "Vault password", "Contraseña de la caja fuerte"), wrong)
                    };
                    let enter = enter.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if wrong {
                        ui.label(RichText::new(&self.vault_error).color(DANGER));
                    }
                    ui.add_space(6.0);
                    let action = if encrypting { t("Şifrele", "Encrypt", "Cifrar") } else { t("Kasayı aç", "Open vault", "Abrir caja fuerte") };
                    let left = if encrypting { self.plan.vaults_left() } else { None };
                    if let Some(left) = left {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(tf!(
                                "Ücretsiz plan: bugün {left}/3 şifreleme kaldı, en fazla 200 MB",
                                "Free plan: {left}/3 encryptions left today, up to 200 MB each",
                                "Plan gratuito: quedan {left}/3 cifrados hoy, hasta 200 MB cada uno"
                            )).small().color(muted));
                        });
                    }
                    if left == Some(0) {
                        if wide_button(ui, t("⭐  Pro ile sınırsız şifrele", "⭐  Encrypt without limits with Pro", "⭐  Cifre sin límites con Pro"), Some(ACCENT)) {
                            self.open_pro("vault_limit");
                        }
                    } else if wide_button(ui, action, Some(ACCENT)) || enter {
                        if encrypting && self.vault_pw.chars().count() < 4 {
                            self.vault_error = t("Şifre en az 4 karakter olmalı", "The password needs at least 4 characters", "La contraseña necesita al menos 4 caracteres").into();
                        } else if encrypting && self.vault_pw != self.vault_pw2 {
                            self.vault_error = t("Şifreler aynı değil", "The passwords do not match", "Las contraseñas no coinciden").into();
                        } else {
                            self.start_vault(ui.ctx().clone());
                        }
                    }
                    if wide_button(ui, t("Başka bir şey seç", "Choose something else", "Elegir otra cosa"), None) {
                        self.vault_target = None;
                        self.vault_error.clear();
                    }
                    if encrypting {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(t(
                                "Şifreyi unutursanız dosyalar kurtarılamaz; bir yere not edin.",
                                "If you forget the password the files cannot be recovered; write it down somewhere.",
                                "Si olvida la contraseña no podrá recuperar los archivos; anótela en algún lugar.",
                            )).small().color(muted));
                        });
                    }
                });
            } else {
                let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
                // Smaller once there are vaults, so their list shows without scrolling.
                let compact = !self.vaults.is_empty();
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), if compact { 110.0 } else { 210.0 }), Sense::hover());
                let p = ui.painter();
                let stroke = if hovering { ACCENT } else { border };
                p.rect(rect.shrink(4.0), 14.0, ACCENT.gamma_multiply(if hovering { 0.14 } else { 0.05 }), Stroke::new(1.5, stroke), egui::StrokeKind::Inside);
                let text_y = if compact { -12.0 } else { 22.0 };
                if !compact {
                    let c = rect.center() - vec2(0.0, 38.0);
                    p.circle(c, 34.0, ACCENT.gamma_multiply(0.16), Stroke::new(1.5, ACCENT.gamma_multiply(0.45)));
                    paint_lock(p, c, 34.0, ACCENT, ui.visuals().panel_fill);
                }
                let strong = ui.visuals().strong_text_color();
                p.text(rect.center() + vec2(0.0, text_y), Align2::CENTER_CENTER, t("Dosya ya da klasörü buraya bırakın", "Drop a file or folder here", "Suelte aquí un archivo o una carpeta"), FontId::proportional(17.0), strong);
                p.text(rect.center() + vec2(0.0, text_y + 26.0), Align2::CENTER_CENTER, tf!("Açmak için bir .{} kasası da bırakabilirsiniz", "or a .{} vault to open it", "o una caja fuerte .{} para abrirla", vault::EXT), FontId::proportional(13.0), muted);
                ui.add_space(10.0);
                centered_column(ui, 340.0, |ui| {
                    ui.columns(2, |cols| {
                        if cols[0].add_sized([cols[0].available_width(), 36.0], egui::Button::new(t("📄  Dosya seç…", "📄  Choose file…", "📄  Elegir archivo…")).corner_radius(10)).clicked()
                            && let Some(path) = rfd::FileDialog::new().pick_file()
                        {
                            self.vault_pick(path);
                        }
                        if cols[1].add_sized([cols[1].available_width(), 36.0], egui::Button::new(t("📁  Klasör seç…", "📁  Choose folder…", "📁  Elegir carpeta…")).corner_radius(10)).clicked()
                            && let Some(path) = rfd::FileDialog::new().pick_folder()
                        {
                            self.vault_pick(path);
                        }
                    });
                });
            }
            ui.add_space(10.0);
        });
        if !self.vaults.is_empty() {
            ui.add_space(18.0);
            ui.label(RichText::new(t("Kasalarınız", "Your vaults", "Sus cajas fuertes")).size(16.0).strong());
            ui.label(RichText::new(t(
                "Burada şifrelediğiniz her şey. Geri açmak için Aç'a basıp şifreyi girin.",
                "Everything you encrypted here. Click Open and enter the password to get it back.",
                "Todo lo que cifró aquí. Pulse Abrir y escriba la contraseña para recuperarlo.",
            )).color(muted));
            ui.add_space(8.0);
            let idle = self.vault_job.is_none();
            let mut forget = None;
            for path in self.vaults.clone() {
                let meta = std::fs::metadata(&path).ok();
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let shown = name.strip_suffix(&format!(".{}", vault::EXT)).unwrap_or(&name).to_string();
                let icon = self.icons.get(ui.ctx(), &path);
                // The whole row is clickable; it is allocated before its buttons, so they keep their own clicks.
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 68.0), if idle && meta.is_some() { Sense::click() } else { Sense::hover() });
                let hovered = resp.hovered() && idle && meta.is_some();
                let (_, card_fill, border, _) = palette(ui.visuals().dark_mode);
                ui.painter().rect(
                    row,
                    12.0,
                    if hovered { ui.visuals().widgets.hovered.weak_bg_fill } else { card_fill },
                    Stroke::new(1.0, if hovered { ACCENT } else { border }),
                    egui::StrokeKind::Inside,
                );
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.vault_pick(path.clone());
                    self.vault_prompt = true;
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(row.shrink2(vec2(14.0, 12.0))), |ui| {
                    ui.horizontal_centered(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                        paint_icon(ui, rect, icon, &shown);
                        let badge = rect.right_bottom() - vec2(3.0, 3.0);
                        ui.painter().circle(badge, 9.0, ACCENT, Stroke::new(2.0, ui.visuals().panel_fill));
                        paint_lock(ui.painter(), badge, 11.0, Color32::WHITE, ACCENT);
                        ui.add_space(6.0);
                        // Leaves room for the buttons on the right; longer text is cut with "…".
                        let text_w = (ui.available_width() - 250.0).max(80.0);
                        ui.vertical(|ui| {
                            ui.set_max_width(text_w);
                            ui.add(egui::Label::new(RichText::new(&shown).size(15.0).strong()).truncate());
                            let folder = path.parent().unwrap_or(&path);
                            let folder = match dirs::home_dir().and_then(|h| folder.strip_prefix(h).ok().map(Path::to_path_buf)) {
                                Some(rel) => format!("~/{}", rel.display()),
                                None => folder.display().to_string(),
                            };
                            let detail = match &meta {
                                Some(m) => format!("{folder} · {}", human_size(m.len())),
                                None => folder,
                            };
                            ui.add(egui::Label::new(RichText::new(detail).small().color(muted)).truncate());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled_ui(idle, |ui| {
                                if meta.is_some() {
                                    let open = egui::Button::new(RichText::new(t("🔓  Aç", "🔓  Open", "🔓  Abrir")).strong().color(Color32::WHITE)).fill(ACCENT).corner_radius(8);
                                    if ui.add(open).clicked() {
                                        self.vault_pick(path.clone());
                                        self.vault_prompt = true;
                                    }
                                    if ui.button(reveal_label()).clicked() {
                                        reveal_in_folder(&path);
                                    }
                                } else {
                                    if ui.button(t("Listeden kaldır", "Remove from list", "Quitar de la lista")).clicked() {
                                        forget = Some(path.clone());
                                    }
                                    ui.label(RichText::new(t("Bulunamadı", "Not found", "No encontrado")).color(DANGER));
                                }
                            });
                        });
                    });
                });
                ui.add_space(6.0);
            }
            if let Some(path) = forget {
                self.vaults.retain(|v| *v != path);
                if let Err(e) = vault::save_list(&self.vaults) {
                    eprintln!("could not save the vault list: {e}");
                }
            }
        }
    }

    fn monk_view(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        let now = Instant::now();
        ui.label(RichText::new(t("⏱  Monk modu", "⏱  Monk mode", "⏱  Modo monje")).size(20.0).strong());
        ui.label(RichText::new(t(
            "Pomodoro: odak süresi boyunca keşiş meditasyon yapar, sonra mola verirsiniz. Süre, keşiş oturup odaklandığında başlar.",
            "Pomodoro: the monk meditates while you focus, then you take a break. The clock starts once the monk sits down to focus.",
            "Pomodoro: el monje medita mientras usted se concentra y luego toma un descanso. El reloj arranca cuando el monje se sienta a concentrarse.",
        )).color(muted));
        ui.add_space(14.0);
        let idle = self.monk.phase == focus::Phase::Idle;
        let paused = self.monk.paused();
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let d = (ui.available_width() * 0.5).clamp(220.0, 340.0);
                let (rect, _) = ui.allocate_exact_size(vec2(d, d), Sense::hover());
                let p = ui.painter();
                let ring_color = if self.monk.phase == focus::Phase::Break { Color32::from_rgb(52, 199, 89) } else { ACCENT };
                let (center, radius) = (rect.center(), d / 2.0 - 8.0);
                p.circle_filled(center, radius, ACCENT.gamma_multiply(0.06));
                p.circle_stroke(center, radius, Stroke::new(8.0, ui.visuals().widgets.inactive.weak_bg_fill));
                let progress = if self.monk.phase == focus::Phase::Settling { 0.0 } else { self.monk.progress(now) };
                if progress > 0.0 {
                    let n = (progress * 120.0).ceil() as usize + 1;
                    let arc = (0..n)
                        .map(|i| {
                            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * progress * i as f32 / (n - 1) as f32;
                            center + vec2(a.cos(), a.sin()) * radius
                        })
                        .collect();
                    p.add(egui::Shape::line(arc, Stroke::new(8.0, ring_color)));
                }
                let sit = match self.monk.phase {
                    focus::Phase::Idle | focus::Phase::Break => 0.0,
                    focus::Phase::Settling => self.monk.progress(now),
                    focus::Phase::Focus => 1.0,
                };
                let monk_rect = Rect::from_center_size(center + vec2(0.0, d * 0.03), vec2(d * 0.6, d * 0.8));
                paint_monk(p, monk_rect, sit, ui.input(|i| i.time) as f32, self.monk.phase == focus::Phase::Focus && !paused);
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_max_width(320.0);
                    ui.add_space(10.0);
                    let (label, color) = match self.monk.phase {
                        _ if paused => (t("Duraklatıldı", "Paused", "En pausa"), muted),
                        focus::Phase::Idle => (t("Hazır", "Ready", "Listo"), muted),
                        focus::Phase::Settling => (t("Keşiş odaklanıyor…", "The monk is settling in…", "El monje se está concentrando…"), ACCENT),
                        focus::Phase::Focus => (t("Odak", "Focus", "Enfoque"), ACCENT),
                        focus::Phase::Break => (t("☕ Mola", "☕ Break", "☕ Descanso"), Color32::from_rgb(52, 199, 89)),
                    };
                    ui.label(RichText::new(label).size(16.0).strong().color(color));
                    ui.label(RichText::new(mmss(self.monk.remaining(now))).size(54.0).monospace().strong());
                    ui.add_space(6.0);
                    let now = Instant::now();
                    if idle {
                        if wide_button(ui, t("▶  Odaklanmaya başla", "▶  Start focusing", "▶  Empezar a concentrarse"), Some(ACCENT)) {
                            self.monk.start(now);
                        }
                    } else {
                        if paused {
                            if wide_button(ui, t("▶  Devam et", "▶  Resume", "▶  Reanudar"), Some(ACCENT)) {
                                self.monk.resume(now);
                            }
                        } else if wide_button(ui, t("⏸  Duraklat", "⏸  Pause", "⏸  Pausar"), Some(ACCENT)) {
                            self.monk.pause(now);
                        }
                        if self.monk.phase == focus::Phase::Break && wide_button(ui, t("Molayı atla", "Skip break", "Saltar descanso"), None) {
                            self.monk.skip_break();
                        }
                        if wide_button(ui, t("Sıfırla", "Reset", "Reiniciar"), None) {
                            self.monk.reset();
                        }
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new(t("Süreler", "Durations", "Duraciones")).strong());
                    if !idle {
                        ui.label(RichText::new(t("Süreler zamanlayıcı dururken değiştirilebilir.", "Durations can be changed while the timer is stopped.", "Las duraciones se pueden cambiar con el temporizador detenido.")).small().color(muted));
                    }
                    ui.add_enabled_ui(idle, |ui| {
                        let unit = t(" dk", " min", " min");
                        let mut focus_min = self.monk.focus.as_secs() / 60;
                        ui.horizontal(|ui| {
                            ui.add_sized([70.0, 24.0], egui::Label::new(t("Odak", "Focus", "Enfoque")));
                            ui.add(egui::Slider::new(&mut focus_min, 1..=120).suffix(unit));
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(74.0);
                            for preset in [15, 25, 45, 60] {
                                if ui.selectable_label(focus_min == preset, format!("{preset}")).clicked() {
                                    focus_min = preset;
                                }
                            }
                        });
                        let mut rest_min = self.monk.rest.as_secs() / 60;
                        ui.horizontal(|ui| {
                            ui.add_sized([70.0, 24.0], egui::Label::new(t("Mola", "Break", "Descanso")));
                            ui.add(egui::Slider::new(&mut rest_min, 1..=30).suffix(unit));
                        });
                        self.monk.focus = Duration::from_secs(focus_min * 60);
                        self.monk.rest = Duration::from_secs(rest_min * 60);
                    });
                    ui.add_space(4.0);
                    let n = self.monk.sessions;
                    let done = tf!("Tamamlanan odak oturumu: {n}", "Focus sessions done: {n}", "Sesiones de enfoque completadas: {n}");
                    ui.label(RichText::new(format!("{done}  {}", "🍅".repeat(n.min(12) as usize))).color(muted));
                });
            });
        });
    }

    /// Password setup, then the locked apps and all installed apps; removing a lock asks for the password.
    fn lock_view(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        ui.label(RichText::new(t("🔒  Uygulama kilidi", "🔒  App lock", "🔒  Bloqueo de apps")).size(20.0).strong());
        ui.label(RichText::new(t(
            "Kilitli bir uygulama açılınca deskMonk onu durdurur ve şifre sorar. Doğru şifreyle uygulama açılır, kapatılınca yeniden kilitlenir.",
            "When a locked app opens, deskMonk stops it and asks for the password. With the right password the app opens, and it locks again when it quits.",
            "Cuando se abre una app bloqueada, deskMonk la detiene y pide la contraseña. Con la contraseña correcta la app se abre y se vuelve a bloquear al cerrarse.",
        )).color(muted));
        ui.add_space(14.0);
        let wrong = !self.lock_error.is_empty();
        if !self.lock.has_password() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(10.0);
                centered_column(ui, 320.0, |ui| {
                    form_header(
                        ui,
                        None,
                        t("Bir şifre belirleyin", "Set a password", "Elija una contraseña"),
                        t("Kilitli uygulamalar bu şifreyle açılır.", "Locked apps open with this password.", "Las apps bloqueadas se abren con esta contraseña."),
                    );
                    pw_field(ui, &mut self.lock_pw, t("Yeni şifre (en az 4 karakter)", "New password (at least 4 characters)", "Nueva contraseña (mínimo 4 caracteres)"), wrong);
                    let confirm = pw_field(ui, &mut self.lock_pw2, t("Şifre tekrar", "Repeat password", "Repita la contraseña"), wrong);
                    if wrong {
                        ui.label(RichText::new(&self.lock_error).color(DANGER));
                    }
                    ui.add_space(6.0);
                    let enter = confirm.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if wide_button(ui, t("Şifreyi kaydet", "Save password", "Guardar contraseña"), Some(ACCENT)) || enter {
                        if self.lock_pw.chars().count() < 4 {
                            self.lock_error = t("Şifre en az 4 karakter olmalı", "The password needs at least 4 characters", "La contraseña necesita al menos 4 caracteres").into();
                        } else if self.lock_pw != self.lock_pw2 {
                            self.lock_error = t("Şifreler aynı değil", "The passwords do not match", "Las contraseñas no coinciden").into();
                        } else {
                            self.lock.set_password(&self.lock_pw);
                            self.lock_error = self.lock.save().err().map(|e| tf!("Kaydedilemedi: {e}", "Could not save: {e}", "No se pudo guardar: {e}")).unwrap_or_default();
                            self.lock_pw.clear();
                            self.lock_pw2.clear();
                        }
                    }
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(t(
                            "Şifreyi unutursanız kilitler kaldırılamaz; bir yere not edin.",
                            "If you forget the password the locks cannot be removed; write it down somewhere.",
                            "Si olvida la contraseña no podrá quitar los bloqueos; anótela en algún lugar.",
                        )).small().color(muted));
                    });
                });
                ui.add_space(10.0);
            });
        } else {
            let mut changed = false;
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tf!("Kilitli uygulamalar ({})", "Locked apps ({})", "Apps bloqueadas ({})", self.lock.apps.len())).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t("+ Başka konumdan ekle…", "+ Add from elsewhere…", "+ Añadir desde otra ubicación…")).clicked()
                            && let Some(app) = rfd::FileDialog::new()
                                .set_directory(if cfg!(target_os = "linux") { "/usr/share/applications" } else { "/Applications" })
                                .add_filter(t("Uygulama", "App", "App"), &[applock::EXT])
                                .pick_file()
                        {
                            if !applock::can_lock(&app) {
                                self.lock_error = tf!(
                                    "{} kilitlenemez (sistem uygulaması veya deskMonk)",
                                    "{} cannot be locked (system app or deskMonk)",
                                    "{} no se puede bloquear (app del sistema o deskMonk)",
                                    app.display()
                                );
                            } else if !self.lock.apps.contains(&app) {
                                if self.plan.can_lock_more(self.lock.apps.len()) {
                                    self.lock.apps.push(app);
                                    changed = true;
                                } else {
                                    self.open_pro("applock_limit");
                                }
                            }
                        }
                    });
                });
                ui.label(RichText::new(t(
                    "Kilidini kaldırmak için uygulamaya tıklayın; şifre sorulur.",
                    "Click an app to remove its lock; the password is asked.",
                    "Haga clic en una app para quitar su bloqueo; se pedirá la contraseña.",
                )).small().color(muted));
                ui.add_space(6.0);
                if let Some(i) = self.locked_tiles(ui) {
                    self.ask_unlock(Unlock::App(self.lock.apps[i].clone()));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !self.lock.apps.is_empty() && ui.add(egui::Button::new(RichText::new(t("Tüm kilitleri kaldır", "Remove all locks", "Quitar todos los bloqueos")).color(DANGER))).clicked() {
                        self.ask_unlock(Unlock::All);
                    }
                    if ui.add(egui::Button::new(RichText::new(t("Kilit sistemini tamamen kapat", "Turn app lock off completely", "Desactivar el bloqueo por completo")).color(DANGER))).clicked() {
                        self.ask_unlock(Unlock::Off);
                    }
                });
            });
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(t("Tüm uygulamalar", "All apps", "Todas las apps")).strong());
                        ui.label(RichText::new(t(
                            "Açtığınız uygulamalar açılırken şifre sorulur. Kapatmak şifre ister.",
                            "Apps you switch on ask for the password when they open. Switching off needs the password.",
                            "Las apps que active pedirán la contraseña al abrirse. Desactivarlas requiere la contraseña.",
                        )).small().color(muted));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.app_filter).hint_text(t("🔍  Uygulama ara", "🔍  Search apps", "🔍  Buscar apps")).desired_width(220.0));
                    });
                });
                ui.add_space(6.0);
                let installed = self.installed.get_or_insert_with(applock::installed_apps);
                let needle = self.app_filter.to_lowercase();
                let mut toggle = None;
                let mut shown = 0;
                egui::ScrollArea::vertical().id_salt("installed").max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for app in installed.iter() {
                        let name = applock::name(app);
                        if !name.to_lowercase().contains(&needle) {
                            continue;
                        }
                        shown += 1;
                        let folder = app.parent().unwrap_or(app).display().to_string();
                        let icon = self.icons.get(ui.ctx(), app);
                        if app_row(ui, icon, &name, &folder, self.lock.apps.contains(app)).clicked() {
                            toggle = Some(app.clone());
                        }
                    }
                    if shown == 0 {
                        ui.label(RichText::new(t("Uygulama bulunamadı.", "No apps found.", "No se encontraron apps.")).color(muted));
                    }
                });
                if let Some(app) = toggle {
                    if self.lock.apps.contains(&app) {
                        self.ask_unlock(Unlock::App(app));
                    } else if self.plan.can_lock_more(self.lock.apps.len()) {
                        self.lock.apps.push(app);
                        changed = true;
                    } else {
                        self.open_pro("applock_limit");
                    }
                }
            });
            if changed {
                self.save_lock();
            }
            ui.add_space(8.0);
            ui.label(RichText::new(if cfg!(target_os = "macos") { t(
                "deskMonk açık olduğu sürece çalışır: pencereyi kapatmak sorun değil, ama Cmd+Q ile çıkılırsa kilit kalkar. Zaten açık olan bir uygulama, kapatılana kadar kilitlenmez.",
                "Works while deskMonk runs: closing the window is fine, but quitting with Cmd+Q lifts the lock. An app that is already open is not locked until it quits.",
                "Funciona mientras deskMonk esté abierto: cerrar la ventana no importa, pero salir con Cmd+Q quita el bloqueo. Una app que ya está abierta no se bloquea hasta que se cierre.",
            ) } else { t(
                "deskMonk açık olduğu sürece çalışır: pencereyi küçültmek sorun değil, ama deskMonk kapatılırsa kilit kalkar. Zaten açık olan bir uygulama, kapatılana kadar kilitlenmez.",
                "Works while deskMonk runs: minimizing the window is fine, but closing deskMonk lifts the lock. An app that is already open is not locked until it quits.",
                "Funciona mientras deskMonk esté abierto: minimizar la ventana no importa, pero cerrar deskMonk quita el bloqueo. Una app que ya está abierta no se bloquea hasta que se cierre.",
            ) }).small().color(muted));
            if !self.lock_error.is_empty() && self.lock_prompt.is_none() && self.unlock.is_none() {
                ui.label(RichText::new(&self.lock_error).color(DANGER));
            }
        }
        self.unlock_modal(ui);
    }

    fn save_lock(&mut self) {
        self.lock_error = self.lock.save().err().map(|e| tf!("Kaydedilemedi: {e}", "Could not save: {e}", "No se pudo guardar: {e}")).unwrap_or_default();
        if let Some(w) = &self.watcher {
            w.set_apps(self.lock.apps.clone());
        }
    }

    /// Locked apps as tiles; returns the index of the one clicked.
    fn locked_tiles(&mut self, ui: &mut Ui) -> Option<usize> {
        if self.lock.apps.is_empty() {
            let (_, _, _, muted) = palette(ui.visuals().dark_mode);
            ui.label(RichText::new(t("Henüz kilitli uygulama yok. Aşağıdan seçin.", "No locked apps yet. Pick some below.", "Aún no hay apps bloqueadas. Elija abajo.")).color(muted));
            return None;
        }
        let mut clicked = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for (i, app) in self.lock.apps.iter().enumerate() {
                let name = applock::name(app);
                let icon = self.icons.get(ui.ctx(), app);
                if app_tile(ui, icon, &name).on_hover_text(app.display().to_string()).clicked() {
                    clicked = Some(i);
                }
            }
        });
        clicked
    }

    fn ask_unlock(&mut self, action: Unlock) {
        self.unlock = Some(action);
        self.lock_error.clear();
        self.lock_pw.clear();
    }

    /// Password pop-up for removing locks; nothing changes without the right password.
    fn unlock_modal(&mut self, ui: &mut Ui) {
        let Some(action) = self.unlock.clone() else { return };
        let (name, icon) = match &action {
            Unlock::App(app) => (applock::name(app), self.icons.get(ui.ctx(), app)),
            _ => (String::new(), None),
        };
        let (title, subtitle, button) = match &action {
            Unlock::App(_) => (
                tf!("{name} kilidini kaldır", "Unlock {name}", "Desbloquear {name}"),
                t("Bundan sonra şifresiz açılır.", "It will open without a password from now on.", "A partir de ahora se abrirá sin contraseña."),
                t("Kilidi kaldır", "Remove lock", "Quitar bloqueo"),
            ),
            Unlock::All => (
                t("Tüm kilitleri kaldır", "Remove all locks", "Quitar todos los bloqueos").into(),
                t("Kilitli uygulamaların hepsi şifresiz açılır.", "All locked apps will open without a password.", "Todas las apps bloqueadas se abrirán sin contraseña."),
                t("Hepsinin kilidini kaldır", "Unlock all", "Desbloquear todas"),
            ),
            Unlock::Off => (
                t("Kilit sistemini kapat", "Turn app lock off", "Desactivar el bloqueo").into(),
                t("Şifre ve tüm kilitler silinir.", "The password and all locks are deleted.", "Se borran la contraseña y todos los bloqueos."),
                t("Kapat", "Turn off", "Desactivar"),
            ),
        };
        let badge = matches!(action, Unlock::App(_)).then_some((icon, name.as_str()));
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("unlock")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(28))).show(ui.ctx(), |ui| {
            match app_password_prompt(ui, badge, &title, subtitle, button, &mut self.lock_pw, &self.lock_error) {
                Some(true) => {
                    if self.lock.check(&self.lock_pw) {
                        match &action {
                            Unlock::App(app) => self.lock.apps.retain(|a| a != app),
                            Unlock::All => self.lock.apps.clear(),
                            Unlock::Off => self.lock = applock::Config::default(),
                        }
                        self.save_lock();
                        close = true;
                    } else {
                        self.lock_error = wrong_password().into();
                    }
                    self.lock_pw.clear();
                }
                Some(false) => close = true,
                None => {}
            }
        });
        if close || modal.should_close() {
            self.unlock = None;
            self.lock_pw.clear();
            if self.lock_error == wrong_password() {
                self.lock_error.clear();
            }
        }
    }

    fn footer(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        let (n, size) = self.totals(None, true);
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 44.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let text = RichText::new(tf!("🗑  {} temizle", "🗑  Clean {}", "🗑  Limpiar {}", human_size(size)))
                    .strong()
                    .size(15.0)
                    .color(Color32::WHITE);
                let button = egui::Button::new(text)
                    .fill(DANGER)
                    .min_size(vec2(170.0, 40.0))
                    .corner_radius(10);
                if let Some(d) = &self.deleting {
                    let done = d.done.load(Ordering::Relaxed);
                    let bar = egui::ProgressBar::new(done as f32 / d.total.max(1) as f32)
                        .desired_width(220.0)
                        .desired_height(40.0)
                        .fill(ACCENT)
                        .text(
                            RichText::new(tf!("Siliniyor… {done}/{}", "Deleting… {done}/{}", "Borrando… {done}/{}", d.total))
                                .color(Color32::WHITE),
                        )
                        .animate(true);
                    ui.add(bar);
                } else if ui
                    .add_enabled(n > 0 && self.scanning.is_none(), button)
                    .clicked()
                {
                    self.confirm = true;
                }
                ui.add_space(8.0);
                // Every category at once; the header's "Tümünü seç" covers only the open one.
                let all = egui::Button::new(RichText::new(t("☑  Hepsini seç", "☑  Select all", "☑  Seleccionar todo")).size(15.0))
                    .min_size(vec2(130.0, 40.0))
                    .corner_radius(10);
                if ui.add_enabled(!self.items.is_empty() && self.deleting.is_none() && self.scanning.is_none(), all).clicked() {
                    deskmonk::set_selection(&mut self.items, None, true);
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(tf!("{n} dosya seçildi", "{n} files selected", "{n} archivos seleccionados")).strong());
                    let status = if self.status.is_empty() {
                        t(
                            "Gereksiz dosyalar kalıcı silinir, diğerleri çöp kutusuna taşınır.",
                            "Junk is deleted for good, everything else goes to the trash.",
                            "Los archivos basura se borran definitivamente; el resto va a la papelera.",
                        )
                    } else {
                        &self.status
                    };
                    ui.label(RichText::new(status).small().color(if status.starts_with(FAILED) { DANGER } else { muted }));
                });
            },
        );
        ui.add_space(10.0);
    }
}

impl eframe::App for App {
    // macOS: the close button hides the app instead of quitting, so a scan or cleaning keeps running and
    // clicking the Dock icon brings the window back. Cmd+Q / Dock → Quit still quit.
    // In `logic`, not `ui`: eframe skips `ui` while the window is hidden, and an uncancelled close quits.
    // Linux has no Dock to bring a hidden window back, so there closing quits and the lock prompt un-minimizes.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.monk_tick(ctx);
        // Runs while the window is hidden too, so a locked app's password prompt can bring deskMonk back.
        while let Some(app) = self.blocked.as_ref().and_then(|rx| rx.try_recv().ok()) {
            if self.lock_prompt.is_some() {
                // One prompt at a time; this app asks again on its next launch.
                if let Some(w) = &self.watcher {
                    w.dismiss(&app);
                }
                continue;
            }
            self.lock_prompt = Some(app);
            self.lock_error.clear();
            self.lock_pw.clear();
            #[cfg(target_os = "macos")]
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                ns_app.unhide(None);
                #[allow(deprecated)] // `activate` needs macOS 14; this app supports 11+.
                ns_app.activateIgnoringOtherApps(true);
            }
            // macOS 14+ ignores activation from a background app, so the prompt would sit behind the
            // frontmost window; keep the window on top until the prompt closes.
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        #[cfg(target_os = "macos")]
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                objc2_app_kit::NSApplication::sharedApplication(mtm).hide(None);
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.icons.budget = 12;
        self.monk_tick(&ctx);
        if std::mem::take(&mut self.autostart) {
            self.start_scan(ctx.clone());
        }
        self.poll_deletion();
        self.poll_license(&ctx);
        if let Some(rx) = &self.hw_rx
            && let Ok(parts) = rx.try_recv()
        {
            self.hardware = Some(parts);
            self.hw_rx = None;
        }
        // Waits for consent: the user may say yes only after the hardware is read.
        if !self.hw_sent
            && self.telemetry.consent == Some(true)
            && let Some(parts) = &self.hardware
        {
            self.telemetry.opened("gui", parts);
            self.hw_sent = true;
        }
        if self.deleting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        while let Some((rx, progress)) = &self.scanning
            && let Ok((phase, mut items)) = rx.try_recv()
        {
            let files = progress.load(Ordering::Relaxed);
            deskmonk::keep_selection(&self.items, &mut items);
            self.items = items;
            self.has_scanned = true;
            self.phase = phase;
            if phase.is_none() {
                self.scanned_files = files;
                self.telemetry.scan_done("gui", files, &self.items);
                self.scanning = None;
            }
            // Stay on the current category unless it is empty.
            if !self.items.iter().any(|i| i.category == self.tab)
                && let Some(cat) = Category::ALL
                    .into_iter()
                    .find(|&c| self.items.iter().any(|i| i.category == c))
            {
                self.tab = cat;
            }
        }
        if let Some((rx, _)) = &self.map_scan
            && let Ok(tree) = rx.try_recv()
        {
            self.map = Some(tree);
            self.map_path.clear();
            self.map_scan = None;
        }
        if let Some((rx, _)) = &self.speed_run
            && let Ok(result) = rx.try_recv()
        {
            self.speed = Some(result.map_err(|e| e.to_string()));
            self.speed_run = None;
        }
        if let Some((rx, _, _)) = &self.vault_job
            && let Ok(result) = rx.try_recv()
        {
            let encrypted = self.vault_target.as_deref().is_some_and(|p| !vault::is_vault(p));
            match result {
                Ok((out, trash_error)) => {
                    self.telemetry.used("gui", "vault", if encrypted { "encrypt" } else { "open" });
                    if encrypted {
                        self.plan.record_vault();
                        self.vaults.retain(|v| *v != out);
                        self.vaults.insert(0, out.clone());
                    } else if let Some(src) = &self.vault_target {
                        self.vaults.retain(|v| v != src);
                    }
                    if let Err(e) = vault::save_list(&self.vaults) {
                        eprintln!("could not save the vault list: {e}");
                    }
                    self.vault_done = Some((out, encrypted, trash_error));
                    self.vault_target = None;
                }
                Err(e) => self.vault_error = e,
            }
            self.vault_job = None;
        }
        if self.page == Page::Vault
            && self.vault_job.is_none()
            && let Some(path) = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()))
        {
            self.vault_pick(path);
        }
        if self.scanning.is_some() || self.map_scan.is_some() || self.speed_run.is_some() || self.vault_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.page != Page::Lock {
            // Re-read the installed apps on the next visit.
            self.installed = None;
        }
        let (bg, side, border, muted) = palette(ui.visuals().dark_mode);
        egui::Panel::top("topbar")
            .exact_size(58.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(18, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(32.0, 32.0)));
                    }
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new("deskMonk").size(19.0).strong());
                    ui.label(RichText::new("by pelamx").size(13.0).color(ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        let mut lang = i18n::lang();
                        egui::ComboBox::from_id_salt("lang")
                            .selected_text(format!("🌐 {}", lang.name()))
                            .show_ui(ui, |ui| {
                                for l in i18n::ALL {
                                    ui.selectable_value(&mut lang, l, l.name());
                                }
                            });
                        if lang != i18n::lang() {
                            i18n::set(lang);
                            self.detect_hardware(ctx.clone());
                        }
                        ui.add_space(6.0);
                        let pro_button = if self.plan.is_pro() {
                            egui::Button::new(RichText::new("⭐ Pro").strong().color(Color32::from_rgb(245, 184, 46))).frame(false)
                        } else {
                            egui::Button::new(RichText::new(t("⭐ Pro'ya geç", "⭐ Get Pro", "⭐ Pasar a Pro")).strong().color(Color32::WHITE)).fill(ACCENT).corner_radius(8)
                        };
                        // Get Pro opens the payment page on the website; the app shows its License page, where the key is
                        // entered (and where Pro turns on by itself when the payment was made from this computer).
                        if ui.add(pro_button).clicked() {
                            self.page = Page::License;
                            self.telemetry.used("gui", "pro", "button");
                            if !self.plan.is_pro() && !APPSTORE && let Some(url) = plan::buy_url(&self.telemetry.install_id, false) {
                                open_url(&url);
                                self.pro_waiting = Some(ctx.input(|i| i.time));
                            }
                        }
                        if !self.plan.is_pro() && !APPSTORE && plan::URL.is_some() {
                            let activate = egui::Button::new(RichText::new(t("🔑 Etkinleştir", "🔑 Activate", "🔑 Activar")).strong()).corner_radius(8);
                            if ui.add(activate).on_hover_text(t("Lisans anahtarını girin", "Enter your license key", "Introduzca su clave de licencia")).clicked() {
                                self.page = Page::License;
                            }
                        }
                        let contact = ui.add(egui::Button::new(RichText::new("✉").size(16.0)).frame(false));
                        if contact.on_hover_text(t("Bize yazın", "Contact us", "Escríbanos")).clicked() {
                            // Builds without the server (dev, Mac App Store) write a normal email instead.
                            if plan::URL.is_some() {
                                self.contact_open = true;
                            } else {
                                open_url("mailto:support@deskmonk.app");
                            }
                        }
                        if let Some(v) = self.update_check.as_ref().and_then(|rx| rx.try_recv().ok()) {
                            self.new_version = Some(v);
                        }
                        if let (Some(found), Some(site)) = (self.new_version.clone(), plan::URL) {
                            ui.add_space(6.0);
                            let (label, hint) = match &found {
                                update::Found::Installed(v) => (
                                    tf!("⟳ {v} için yeniden başlat", "⟳ Restart for {v}", "⟳ Reiniciar para {v}"),
                                    t("Yeni sürüm kuruldu; yeniden başlatınca açılır", "The new version is installed; it opens after a restart", "La nueva versión está instalada; se abre al reiniciar"),
                                ),
                                update::Found::Available(v) => (
                                    tf!("⬆ Güncelle: {v}", "⬆ Update to {v}", "⬆ Actualizar a {v}"),
                                    t("Yeni sürümü indir", "Download the new version", "Descargar la nueva versión"),
                                ),
                            };
                            let button = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(Color32::from_rgb(34, 160, 90)).corner_radius(8);
                            if ui.add_enabled(!self.busy(), button).on_hover_text(hint).clicked() {
                                match found {
                                    update::Found::Installed(_) => update::restart(),
                                    update::Found::Available(_) => open_url(&format!("{site}/#download")),
                                }
                            }
                        }
                        // Left out when only a cut-off piece of it would fit.
                        if ui.available_width() > 150.0 {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(t("Akıllı çözümler", "Smart solutions", "Soluciones inteligentes"))
                                    .small()
                                    .color(muted),
                            );
                        }
                    });
                });
            });
        egui::Panel::left("sidebar")
            .exact_size(230.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(12, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| {
                // Scrolls when the window is too short for every item.
                let height = ui.available_height();
                egui::ScrollArea::vertical().id_salt("sidebar").auto_shrink(false).show(ui, |ui| self.sidebar(ui, height));
            });
        egui::Panel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(24, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| self.footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin::same(24)))
            .show(ui, |ui| {
                if !self.plan.is_pro() && !APPSTORE && plan::URL.is_some() && self.page != Page::License {
                    self.license_strip(ui);
                }
                match self.page {
                    Page::Monk => {
                        egui::ScrollArea::vertical().id_salt("monk_page").auto_shrink(false).show(ui, |ui| self.monk_view(ui));
                    }
                    Page::Hardware => self.hardware_view(ui),
                    Page::Map => self.map_view(ui),
                    Page::Vault => {
                        egui::ScrollArea::vertical().id_salt("vault_page").auto_shrink(false).show(ui, |ui| self.vault_view(ui));
                    }
                    Page::Lock => {
                        egui::ScrollArea::vertical().id_salt("lock_page").auto_shrink(false).show(ui, |ui| self.lock_view(ui));
                    }
                    Page::License => {
                        egui::ScrollArea::vertical().id_salt("license_page").auto_shrink(false).show(ui, |ui| self.license_view(ui));
                    }
                    Page::Clean => {
                        self.header(ui, &ctx);
                        ui.add_space(18.0);
                        self.content(ui);
                    }
                }
            });

        if self.pro_open {
            self.pro_dialog(&ctx);
        }
        if self.contact_open {
            self.contact_dialog(&ctx);
        }
        if self.new_version.is_some() && self.new_version != self.update_seen && !self.pro_open {
            self.update_dialog(&ctx);
        }
        if self.vault_prompt {
            self.vault_unlock_dialog(&ctx);
        }
        if self.confirm {
            let (n, size) = self.totals(None, true);
            let clean_left = self.plan.clean_left();
            let over_limit = clean_left.is_some_and(|left| size > left);
            let modal = egui::Modal::new(egui::Id::new("confirm")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(t("🗑  Dosyalar temizlensin mi?", "🗑  Clean these files?", "🗑  ¿Limpiar estos archivos?")).size(19.0).strong());
                ui.add_space(4.0);
                let (perm_n, perm_size) = self.items.iter().filter(|i| i.selected && deskmonk::is_permanent(i, false)).fold((0, 0), |(c, s), i| (c + 1, s + i.size));
                ui.label(tf!("{n} öğe, toplam {}.", "{n} items, {} in total.", "{n} elementos, {} en total.", human_size(size)));
                if perm_n > 0 {
                    ui.label(RichText::new(tf!(
                        "• {perm_n} gereksiz öğe ({}) kalıcı olarak silinecek. Önbellekler gerektiğinde yeniden oluşur.",
                        "• {perm_n} junk items ({}) will be deleted for good. Caches are rebuilt when needed.",
                        "• {perm_n} elementos basura ({}) se borrarán definitivamente. Las cachés se regeneran cuando hace falta.",
                        human_size(perm_size)
                    )).color(DANGER));
                }
                if n > perm_n {
                    ui.label(tf!(
                        "• {} dosya ({}) çöp kutusuna taşınacak, geri alınabilir.",
                        "• {} files ({}) will go to the trash and can be restored.",
                        "• {} archivos ({}) irán a la papelera y se pueden recuperar.",
                        n - perm_n,
                        human_size(size - perm_size)
                    ));
                }
                if let Some(left) = clean_left.filter(|_| over_limit) {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tf!(
                        "Ücretsiz plan günde 2 GB temizler; bugün {} kaldı. Bazı dosyaların seçimini kaldırın ya da Pro'ya geçin.",
                        "The free plan cleans 2 GB a day; {} is left today. Deselect some files or get Pro.",
                        "El plan gratuito limpia 2 GB al día; quedan {} hoy. Desmarque algunos archivos o pase a Pro.",
                        human_size(left)
                    )).color(DANGER));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if over_limit {
                        if ui.add(egui::Button::new(RichText::new(t("⭐  Pro'ya geç", "⭐  Get Pro", "⭐  Pasar a Pro")).strong().color(Color32::WHITE)).fill(ACCENT)).clicked() {
                            self.open_pro("clean_limit");
                            self.confirm = false;
                        }
                    } else if ui.add(egui::Button::new(RichText::new(t("Temizle", "Clean", "Limpiar")).strong().color(Color32::WHITE)).fill(DANGER)).clicked() {
                        self.delete_selected(ctx.clone());
                        self.confirm = false;
                    }
                    if ui.button(t("Vazgeç", "Cancel", "Cancelar")).clicked() {
                        self.confirm = false;
                    }
                });
            });
            if modal.should_close() {
                self.confirm = false;
            }
        }

        if let Some(app) = self.lock_prompt.clone() {
            let name = applock::name(&app);
            let mut close = false;
            let icon = self.icons.get(&ctx, &app);
            egui::Modal::new(egui::Id::new("applock")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(28))).show(&ctx, |ui| {
                match app_password_prompt(ui, Some((icon, &name)), &tf!("{name} kilitli", "{name} is locked", "{name} está bloqueada"),
                    t("Açmak için deskMonk şifresini girin.", "Enter the deskMonk password to open it.", "Introduzca la contraseña de deskMonk para abrirla."),
                    t("Aç", "Open", "Abrir"), &mut self.lock_pw, &self.lock_error) {
                    Some(true) => {
                        if self.lock.check(&self.lock_pw) {
                            if let Some(w) = &self.watcher {
                                w.unlock(&app);
                            }
                            close = true;
                        } else {
                            self.lock_error = wrong_password().into();
                        }
                        self.lock_pw.clear();
                    }
                    Some(false) => {
                        if let Some(w) = &self.watcher {
                            w.dismiss(&app);
                        }
                        close = true;
                    }
                    None => {}
                }
            });
            if close {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
                self.lock_prompt = None;
                self.lock_error.clear();
                self.lock_pw.clear();
            }
        }

        if self.telemetry.needs_prompt() {
            egui::Modal::new(egui::Id::new("consent")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(t("📊  Anonim kullanım istatistikleri", "📊  Anonymous usage statistics", "📊  Estadísticas de uso anónimas")).size(19.0).strong());
                ui.add_space(4.0);
                ui.label(t(
                    "deskMonk'u geliştirmek için tarama ve temizleme sayıları (dosya sayısı, boyut, işletim sistemi) ve donanım bilgileri (işlemci, bellek, disk, ekran kartı, pil: marka, model, doluluk) gönderilsin mi?",
                    "To improve deskMonk, send scan and cleaning counts (number of files, size, operating system) and hardware info (processor, memory, disk, graphics card, battery: brand, model, usage)?",
                    "Para mejorar deskMonk, ¿enviar los recuentos de análisis y limpieza (número de archivos, tamaño, sistema operativo) y datos del hardware (procesador, memoria, disco, tarjeta gráfica, batería: marca, modelo, uso)?",
                ));
                ui.label(RichText::new(t(
                    "Dosya adları, yollar, seri numaraları ve kullanıcı adı asla gönderilmez.",
                    "File names, paths, serial numbers and the user name are never sent.",
                    "Nunca se envían nombres de archivo, rutas, números de serie ni el nombre de usuario.",
                )).strong());
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new(t("İzin ver", "Allow", "Permitir")).strong().color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        self.telemetry.set_consent(true);
                    }
                    if ui.button(t("Hayır", "No", "No")).clicked() {
                        self.telemetry.set_consent(false);
                    }
                });
            });
        }
        no_input_method(&ctx);
    }
}

// ---------- widgets ----------

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (_, card, border, _) = palette(ui.visuals().dark_mode);
    egui::Frame::new()
        .fill(card)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(12)
        .inner_margin(Margin::same(14))
        .show(ui, add)
        .inner
}

fn wrong_password() -> &'static str {
    t("Yanlış şifre", "Wrong password", "Contraseña incorrecta")
}

/// A lock removal that waits for the password.
#[derive(Clone)]
enum Unlock {
    App(PathBuf),
    All,
    Off,
}

/// App icons as textures, loaded a few per frame so opening a long list doesn't stall.
#[derive(Default)]
struct Icons {
    cache: std::collections::HashMap<PathBuf, Option<egui::TextureHandle>>,
    budget: usize,
}

impl Icons {
    fn get(&mut self, ctx: &egui::Context, app: &Path) -> Option<egui::TextureId> {
        if !self.cache.contains_key(app) {
            if self.budget == 0 {
                ctx.request_repaint();
                return None;
            }
            self.budget -= 1;
            let tex = app_icon(app, 64).map(|img| ctx.load_texture(app.display().to_string(), img, egui::TextureOptions::LINEAR));
            self.cache.insert(app.to_path_buf(), tex);
        }
        self.cache[app].as_ref().map(|t| t.id())
    }
}

/// The app's Finder icon, `px` pixels square.
#[cfg(target_os = "macos")]
fn app_icon(app: &Path, px: usize) -> Option<egui::ColorImage> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&app.to_string_lossy()));
    let n = px as isize;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), n, n, 8, 4, true, false, NSDeviceRGBColorSpace, n * 4, 32,
        )
    }?;
    let gc = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&gc));
    image.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64)));
    gc.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let data = unsafe { std::slice::from_raw_parts(rep.bitmapData(), px * px * 4) };
    Some(egui::ColorImage::from_rgba_premultiplied([px, px], data))
}

/// The launcher's PNG icon from the hicolor theme or pixmaps.
// shortcut: SVG-only icons (common for GNOME core apps) show the lettered placeholder; needs an SVG renderer (resvg).
#[cfg(target_os = "linux")]
fn app_icon(app: &Path, _: usize) -> Option<egui::ColorImage> {
    let text = std::fs::read_to_string(app).ok()?;
    let icon = text.lines().find_map(|l| l.strip_prefix("Icon="))?.trim();
    let mut paths = vec![PathBuf::from(icon)];
    let roots = [dirs::data_dir().unwrap_or_default(), dirs::data_dir().unwrap_or_default().join("flatpak/exports/share")];
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or("/usr/local/share:/usr/share".into());
    for root in roots.into_iter().chain(system.split(':').map(PathBuf::from)).chain(["/var/lib/flatpak/exports/share".into()]) {
        paths.extend(["64x64", "128x128", "48x48", "256x256", "96x96", "512x512", "32x32"].map(|s| root.join(format!("icons/hicolor/{s}/apps/{icon}.png"))));
        paths.push(root.join(format!("pixmaps/{icon}.png")));
    }
    let png = paths.iter().filter(|p| p.extension().is_some_and(|e| e == "png")).find_map(|p| std::fs::read(p).ok())?;
    let img = eframe::icon_data::from_png_bytes(&png).ok()?;
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn app_icon(_: &Path, _: usize) -> Option<egui::ColorImage> {
    None
}

/// Draws the icon, or a lettered placeholder while it loads.
fn paint_icon(ui: &Ui, rect: Rect, icon: Option<egui::TextureId>, name: &str) {
    let p = ui.painter();
    match icon {
        Some(id) => {
            p.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(rect.shrink(rect.width() * 0.08), rect.width() * 0.22, ACCENT.gamma_multiply(0.25));
            let letter = name.chars().next().unwrap_or('?').to_uppercase().to_string();
            p.text(rect.center(), Align2::CENTER_CENTER, letter, FontId::proportional(rect.height() * 0.45), ACCENT);
        }
    }
}

const MONK_INK: Color32 = Color32::from_rgb(27, 21, 48);
const ROBE: Color32 = Color32::from_rgb(242, 140, 40);
const SASH: Color32 = Color32::from_rgb(200, 64, 31);
const SKIN: Color32 = Color32::from_rgb(245, 201, 155);
const GOLD: Color32 = Color32::from_rgb(255, 210, 90);

/// Filled ellipse with an outline.
fn oval(p: &egui::Painter, c: Pos2, r: egui::Vec2, fill: Color32, ink: Stroke) {
    p.add(egui::Shape::ellipse_filled(c, r, fill));
    p.add(egui::Shape::ellipse_stroke(c, r, ink));
}

/// A sleeve: a thick outlined bend through `pts`, ending in a hand.
fn sleeve(p: &egui::Painter, pts: &[Pos2], width: f32, fill: Color32, ink: Stroke) {
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(width + ink.width * 2.0, ink.color)));
    for pt in pts {
        p.circle_filled(*pt, width / 2.0 + ink.width, ink.color);
    }
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(width, fill)));
    for pt in pts {
        p.circle_filled(*pt, width / 2.0, fill);
    }
}

fn curve(p: &egui::Painter, a: Pos2, ctrl: Pos2, b: Pos2, stroke: Stroke) {
    p.add(egui::epaint::QuadraticBezierShape::from_points_stroke([a, ctrl, b], false, Color32::TRANSPARENT, stroke));
}

/// The monk in the logo's style, drawn in a 300×400 box scaled into `rect`. `sit` blends standing (0) into the
/// seated focus pose (1); `t` drives breathing and the aura while `alive`.
fn paint_monk(p: &egui::Painter, rect: Rect, sit: f32, t: f32, alive: bool) {
    let s = rect.height() / 400.0;
    let origin = rect.center_top() - vec2(150.0 * s, 0.0);
    let pt = |x: f32, y: f32| origin + vec2(x, y) * s;
    p.add(egui::Shape::ellipse_filled(pt(150.0, 384.0), vec2(90.0, 10.0) * s, Color32::from_black_alpha(70)));
    // Sitting down: the standing monk fades out, then the seated one fades in.
    let stand_a = (1.0 - sit * 2.0).clamp(0.0, 1.0);
    let sit_a = (sit * 2.0 - 1.0).clamp(0.0, 1.0);
    let ink = |a: f32| Stroke::new(4.0 * s, MONK_INK.gamma_multiply(a));
    let head = |cx: f32, cy: f32, a: f32, closed: bool| {
        let c = |col: Color32| col.gamma_multiply(a);
        for side in [-1.0, 1.0] {
            p.circle(pt(cx + 46.0 * side, cy + 4.0), 11.0 * s, c(SKIN), ink(a));
        }
        p.circle(pt(cx, cy), 46.0 * s, c(SKIN), ink(a));
        p.add(egui::Shape::ellipse_filled(pt(cx - 16.0, cy - 26.0), vec2(13.0, 7.0) * s, Color32::WHITE.gamma_multiply(0.7 * a)));
        let thin = Stroke::new(3.2 * s, MONK_INK.gamma_multiply(a));
        for side in [-1.0, 1.0] {
            let ex = cx + 17.0 * side;
            if closed {
                curve(p, pt(ex - 9.0, cy), pt(ex, cy + 9.0), pt(ex + 9.0, cy), thin);
            } else {
                p.circle_filled(pt(ex, cy + 2.0), 4.5 * s, c(MONK_INK));
            }
            p.circle_filled(pt(cx + 27.0 * side, cy + 16.0), 7.0 * s, Color32::from_rgb(255, 143, 143).gamma_multiply(0.6 * a));
        }
        curve(p, pt(cx - 11.0, cy + 20.0), pt(cx, cy + 29.0), pt(cx + 11.0, cy + 20.0), thin);
    };

    if stand_a > 0.0 {
        let a = stand_a;
        let c = |col: Color32| col.gamma_multiply(a);
        for x in [128.0, 172.0] {
            oval(p, pt(x, 376.0), vec2(18.0, 8.0) * s, c(SKIN), ink(a));
        }
        p.add(egui::Shape::convex_polygon(
            vec![pt(112.0, 165.0), pt(188.0, 165.0), pt(206.0, 192.0), pt(220.0, 372.0), pt(80.0, 372.0), pt(94.0, 192.0)],
            c(ROBE),
            ink(a),
        ));
        p.add(egui::Shape::convex_polygon(vec![pt(114.0, 167.0), pt(142.0, 165.0), pt(214.0, 338.0), pt(208.0, 364.0)], c(SASH), ink(a)));
        for side in [-1.0, 1.0] {
            let x = |v: f32| 150.0 + (v - 150.0) * side;
            sleeve(p, &[pt(x(104.0), 190.0), pt(x(92.0), 288.0)], 26.0 * s, c(ROBE), ink(a));
            p.circle(pt(x(92.0), 302.0), 12.0 * s, c(SKIN), ink(a));
        }
        head(150.0, 118.0, a, false);
    }

    if sit_a > 0.0 {
        let a = sit_a;
        let c = |col: Color32| col.gamma_multiply(a);
        // Slow breathing: the upper body rises and falls over 4 s.
        let b = if alive { (t * std::f32::consts::TAU / 4.0).sin() * 3.0 } else { 0.0 };
        if alive {
            for k in 0..3 {
                let f = (t * 0.3 + k as f32 / 3.0).fract();
                p.circle_stroke(pt(150.0, 250.0), (95.0 + 95.0 * f) * s, Stroke::new(3.0 * s, GOLD.gamma_multiply((1.0 - f) * 0.45 * a)));
            }
            for k in 0..7 {
                let k = k as f32;
                let f = (t * 0.12 + k * 0.29).fract();
                let x = 55.0 + 190.0 * (k * 0.37).fract() + (t + k).sin() * 6.0;
                p.circle_filled(pt(x, 340.0 - 280.0 * f), (2.0 + (k % 3.0)) * s, GOLD.gamma_multiply((f * (1.0 - f) * 3.0).min(1.0) * a));
            }
        }
        oval(p, pt(150.0, 348.0), vec2(100.0, 34.0) * s, c(ROBE), ink(a));
        for x in [112.0, 188.0] {
            oval(p, pt(x, 334.0), vec2(15.0, 7.0) * s, c(SKIN), ink(a));
        }
        p.add(egui::Shape::convex_polygon(
            vec![pt(114.0, 212.0 + b), pt(186.0, 212.0 + b), pt(204.0, 238.0 + b), pt(210.0, 336.0), pt(90.0, 336.0), pt(96.0, 238.0 + b)],
            c(ROBE),
            ink(a),
        ));
        p.add(egui::Shape::convex_polygon(vec![pt(116.0, 214.0 + b), pt(142.0, 212.0 + b), pt(204.0, 318.0), pt(198.0, 336.0)], c(SASH), ink(a)));
        // Hands pressed together at the chest, with a soft glow that pulses while focusing.
        let hands = pt(150.0, 262.0 + b);
        if alive {
            p.circle_filled(hands, (30.0 + 5.0 * (t * 2.0).sin()) * s, GOLD.gamma_multiply(0.22 * a));
        }
        for side in [-1.0, 1.0] {
            let x = |v: f32| 150.0 + (v - 150.0) * side;
            sleeve(p, &[pt(x(103.0), 236.0 + b), pt(x(100.0), 300.0), pt(x(136.0), 284.0 + b)], 24.0 * s, c(ROBE), ink(a));
        }
        oval(p, hands, vec2(13.0, 27.0) * s, c(SKIN), ink(a));
        p.line_segment([hands - vec2(0.0, 22.0 * s), hands + vec2(0.0, 22.0 * s)], Stroke::new(2.5 * s, MONK_INK.gamma_multiply(a)));
        head(150.0, 165.0 + b, a, true);
    }
}

/// System notification when a focus or break ends; best effort.
fn notify(title: &str, body: &str) {
    // shortcut: the sandboxed App Store build gets no notification banner (UNUserNotificationCenter would add it);
    // the Dock icon still bounces.
    #[cfg(target_os = "macos")]
    if !deskmonk::sandboxed() {
        let _ = std::process::Command::new("osascript")
        .arg("-e")
            .arg(format!("display notification {body:?} with title {title:?}"))
            .spawn();
    }
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("notify-send").args([title, body]).spawn();
    // shortcut: no Windows toast (needs the WinRT API); the Dock/taskbar attention request still fires.
    let _ = (title, body);
}

fn mmss(d: Duration) -> String {
    let secs = d.as_secs_f32().ceil() as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// A padlock drawn with shapes (the emoji font's lock is tiny and unclear); `hole` is the keyhole color.
fn paint_lock(p: &egui::Painter, center: Pos2, size: f32, color: Color32, hole: Color32) {
    let body = Rect::from_center_size(center + vec2(0.0, size * 0.17), vec2(size * 0.78, size * 0.58));
    let r = size * 0.24;
    let top = body.top() - size * 0.06;
    let mut shackle = vec![Pos2::new(center.x - r, body.top())];
    shackle.extend((0..=16).map(|i| {
        let a = std::f32::consts::PI * i as f32 / 16.0;
        Pos2::new(center.x - r * a.cos(), top - r * a.sin())
    }));
    shackle.push(Pos2::new(center.x + r, body.top()));
    p.add(egui::Shape::line(shackle, Stroke::new(size * 0.11, color)));
    p.rect_filled(body, size * 0.12, color);
    p.circle_filled(body.center() - vec2(0.0, size * 0.04), size * 0.07, hole);
    p.rect_filled(Rect::from_center_size(body.center() + vec2(0.0, size * 0.06), vec2(size * 0.06, size * 0.14)), 1.0, hole);
}

/// Top of a password form: the app's icon with a lock badge, or a big lock.
fn lock_badge(ui: &mut Ui, app: Option<(Option<egui::TextureId>, &str)>) {
    let (rect, _) = ui.allocate_exact_size(vec2(76.0, 76.0), Sense::hover());
    let fill = ui.visuals().window_fill;
    match app {
        Some((icon, name)) => {
            paint_icon(ui, rect.shrink(4.0), icon, name);
            let c = rect.right_bottom() - vec2(10.0, 10.0);
            ui.painter().circle(c, 15.0, ACCENT, Stroke::new(3.0, fill));
            paint_lock(ui.painter(), c, 17.0, Color32::WHITE, ACCENT);
        }
        None => {
            ui.painter().circle(rect.center(), 38.0, ACCENT.gamma_multiply(0.16), Stroke::new(1.5, ACCENT.gamma_multiply(0.45)));
            paint_lock(ui.painter(), rect.center(), 38.0, ACCENT, fill);
        }
    }
}

/// Roomy password input; the outline turns accent on focus and red after a wrong try.
fn pw_field(ui: &mut Ui, text: &mut String, hint: &str, error: bool) -> egui::Response {
    let (_, _, border, _) = palette(ui.visuals().dark_mode);
    let id = ui.make_persistent_id(hint);
    let stroke = if error {
        DANGER
    } else if ui.memory(|m| m.has_focus(id)) {
        ACCENT
    } else {
        border
    };
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.5, stroke))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(text).id(id).password(true).hint_text(hint).frame(egui::Frame::NONE).font(FontId::proportional(16.0)).desired_width(f32::INFINITY))
        })
        .inner
}

fn wide_button(ui: &mut Ui, text: &str, fill: Option<Color32>) -> bool {
    let label = RichText::new(text).size(15.0);
    let button = match fill {
        Some(fill) => egui::Button::new(label.strong().color(Color32::WHITE)).fill(fill),
        None => egui::Button::new(label).frame(false),
    };
    ui.add_sized([ui.available_width(), 40.0], button.corner_radius(10)).clicked()
}

/// A column `width` wide, centered in the available space.
fn centered_column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    let pad = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width.min(ui.available_width()));
            add(ui);
        });
    });
}

/// Badge, title and subtitle of a password form, centered.
fn form_header(ui: &mut Ui, app: Option<(Option<egui::TextureId>, &str)>, title: &str, subtitle: &str) {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    ui.vertical_centered(|ui| {
        lock_badge(ui, app);
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(20.0).strong());
        ui.label(RichText::new(subtitle).color(muted));
    });
    ui.add_space(12.0);
}

/// Password pop-up body, with the app's icon when it is about one app. Returns Some(true) when submitted, Some(false) when cancelled.
fn app_password_prompt(ui: &mut Ui, app: Option<(Option<egui::TextureId>, &str)>, title: &str, subtitle: &str, action: &str, pw: &mut String, error: &str) -> Option<bool> {
    ui.set_width(320.0);
    form_header(ui, app, title, subtitle);
    pw_field(ui, pw, t("deskMonk şifresi", "deskMonk password", "Contraseña de deskMonk"), !error.is_empty()).request_focus();
    if !error.is_empty() {
        ui.label(RichText::new(error).color(DANGER));
    }
    ui.add_space(6.0);
    // The field keeps focus every frame, so Enter is read directly rather than via lost_focus.
    if wide_button(ui, action, Some(ACCENT)) || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        return Some(true);
    }
    wide_button(ui, t("Vazgeç", "Cancel", "Cancelar"), None).then_some(false)
}

/// A locked app as a clickable tile: icon with a lock badge, name below.
fn app_tile(ui: &mut Ui, icon: Option<egui::TextureId>, name: &str) -> egui::Response {
    let (_, _, border, muted) = palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 112.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    p.rect(
        rect,
        14.0,
        if hovered { ui.visuals().widgets.hovered.weak_bg_fill } else { ACCENT.gamma_multiply(0.08) },
        Stroke::new(1.0, if hovered { ACCENT } else { border }),
        egui::StrokeKind::Inside,
    );
    let icon_rect = Rect::from_center_size(rect.center_top() + vec2(0.0, 40.0), vec2(52.0, 52.0));
    paint_icon(ui, icon_rect, icon, name);
    let badge = icon_rect.right_bottom() - vec2(4.0, 4.0);
    p.circle(badge, 11.0, ACCENT, Stroke::new(2.0, ui.visuals().panel_fill));
    paint_lock(p, badge, 13.0, Color32::WHITE, ACCENT);
    let galley = single_line(ui, name.to_string(), FontId::proportional(13.0), ui.visuals().strong_text_color(), rect.width() - 16.0);
    p.galley(Pos2::new(rect.center().x - galley.size().x / 2.0, rect.bottom() - 30.0), galley, Color32::WHITE);
    if hovered {
        p.text(rect.center_bottom() - vec2(0.0, 10.0), Align2::CENTER_CENTER, t("Kilidi kaldır", "Remove lock", "Quitar bloqueo"), FontId::proportional(10.5), muted);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One installed app with an on/off switch; the whole row toggles.
fn app_row(ui: &mut Ui, icon: Option<egui::TextureId>, name: &str, folder: &str, on: bool) -> egui::Response {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    let p = ui.painter();
    let bg = if resp.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else if on {
        ACCENT.gamma_multiply(0.10)
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 10.0, bg);
    paint_icon(ui, Rect::from_center_size(rect.left_center() + vec2(26.0, 0.0), vec2(34.0, 34.0)), icon, name);
    let text_w = rect.width() - 130.0;
    let galley = single_line(ui, name.to_string(), FontId::proportional(14.5), ui.visuals().strong_text_color(), text_w);
    p.galley(rect.min + vec2(54.0, 7.0), galley, Color32::WHITE);
    let galley = single_line(ui, folder.to_string(), FontId::proportional(11.5), muted, text_w);
    p.galley(rect.min + vec2(54.0, 27.0), galley, muted);
    let t = ui.ctx().animate_bool_responsive(resp.id, on);
    let track = Rect::from_center_size(rect.right_center() - vec2(34.0, 0.0), vec2(42.0, 24.0));
    p.rect_filled(track, 12.0, ui.visuals().widgets.inactive.weak_bg_fill.lerp_to_gamma(ACCENT, t));
    let knob = Pos2::new(egui::lerp(track.left() + 12.0..=track.right() - 12.0, t), track.center().y);
    p.circle_filled(knob, 9.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn empty_state(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space((ui.available_height() * 0.25).max(20.0));
    ui.vertical_centered(add);
}

fn stat(ui: &mut Ui, title: &str, value: &str, sub: &str, color: Option<Color32>) {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.label(RichText::new(title).small().color(muted));
        let size = if ui.available_width() > 150.0 {
            26.0
        } else {
            19.0
        };
        let value = RichText::new(value).size(size).strong();
        let value = egui::Label::new(match color {
            Some(c) => value.color(c),
            None => value,
        });
        ui.add(value.wrap_mode(egui::TextWrapMode::Extend));
        ui.label(RichText::new(sub).small().color(muted));
    });
}

fn nav_item(
    ui: &mut Ui,
    selected: bool,
    icon: &str,
    label: &str,
    count: usize,
    sub: &str,
) -> egui::Response {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        ACCENT.gamma_multiply(0.16)
    } else if resp.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 10.0, bg);
    if selected {
        p.rect_filled(
            Rect::from_min_size(rect.min + vec2(0.0, 14.0), vec2(3.0, rect.height() - 28.0)),
            2.0,
            ACCENT,
        );
    }
    let text = ui.visuals().strong_text_color();
    p.text(
        rect.left_center() + vec2(18.0, 0.0),
        Align2::LEFT_CENTER,
        icon,
        FontId::proportional(19.0),
        if selected { ACCENT } else { text },
    );
    p.text(
        rect.min + vec2(50.0, 10.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(14.5),
        text,
    );
    p.text(
        rect.min + vec2(50.0, 30.0),
        Align2::LEFT_TOP,
        sub,
        FontId::proportional(12.0),
        muted,
    );
    if count > 0 {
        let color = if selected { Color32::WHITE } else { text };
        let galley = p.layout_no_wrap(count.to_string(), FontId::proportional(12.0), color);
        let pill = Rect::from_center_size(
            rect.right_center() - vec2(20.0 + galley.size().x / 2.0, 0.0),
            galley.size() + vec2(16.0, 6.0),
        );
        p.rect_filled(
            pill,
            10.0,
            if selected {
                ACCENT
            } else {
                ui.visuals().widgets.inactive.weak_bg_fill
            },
        );
        p.galley(pill.center() - galley.size() / 2.0, galley, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn single_line(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        ..Default::default()
    };
    ui.painter().layout_job(job)
}

/// Single line that drops characters from the start ("…/end/of/path") so the most specific part stays visible.
fn tail_line(ui: &Ui, text: String, font: FontId, color: Color32, width: f32) -> Arc<egui::Galley> {
    let p = ui.painter();
    let mut galley = p.layout_no_wrap(text.clone(), font.clone(), color);
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while galley.size().x > width && keep > 1 {
        keep = (keep as f32 * width / galley.size().x) as usize - 1;
        let tail: String = chars[chars.len() - keep.max(1)..].iter().collect();
        galley = p.layout_no_wrap(format!("…{tail}"), font.clone(), color);
    }
    galley
}

fn group_row(ui: &mut Ui, group: usize, items: &[Item]) {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    let members: Vec<&Item> = items.iter().filter(|i| i.group == group).collect();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let each = members[0].size;
    let text = tf!(
        "GRUP {group}  ·  {} kopya  ·  her biri {}  ·  {} kazanılabilir",
        "GROUP {group}  ·  {} copies  ·  {} each  ·  {} to gain",
        "GRUPO {group}  ·  {} copias  ·  {} cada una  ·  {} a ganar",
        members.len(),
        human_size(each),
        human_size(each * (members.len() as u64 - 1))
    );
    ui.painter().text(
        rect.left_bottom() + vec2(4.0, -10.0),
        Align2::LEFT_BOTTOM,
        text,
        FontId::proportional(11.5),
        muted,
    );
}

fn item_row(ui: &mut Ui, item: &mut Item, idx: usize, first_copy: bool) {
    let dark = ui.visuals().dark_mode;
    let (_, _, border, muted) = palette(dark);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    // Reveal-in-folder button, shown on hover. Registered after the row so it wins the click.
    let reveal_rect =
        Rect::from_center_size(rect.right_center() - vec2(118.0, 0.0), vec2(30.0, 30.0));
    let reveal = ui
        .interact(reveal_rect, ui.id().with(("reveal", idx)), Sense::click())
        .on_hover_text(reveal_label());
    if reveal.clicked() {
        reveal_in_folder(&item.path);
    } else if resp.clicked() {
        item.selected ^= true;
    }

    let p = ui.painter();
    let hovered = resp.hovered() || reveal.hovered();
    if item.selected {
        p.rect_filled(
            rect,
            8.0,
            ACCENT.gamma_multiply(if dark { 0.10 } else { 0.07 }),
        );
    } else if hovered {
        p.rect_filled(
            rect,
            8.0,
            ui.visuals()
                .widgets
                .hovered
                .weak_bg_fill
                .gamma_multiply(0.6),
        );
    }
    p.hline(
        rect.x_range().shrink(6.0),
        rect.bottom(),
        Stroke::new(1.0, border.gamma_multiply(0.6)),
    );

    let cb = Rect::from_center_size(rect.left_center() + vec2(20.0, 0.0), vec2(18.0, 18.0));
    if item.selected {
        p.rect_filled(cb, 5.0, ACCENT);
        p.text(
            cb.center(),
            Align2::CENTER_CENTER,
            "✔",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    } else {
        p.rect_stroke(cb, 5.0, Stroke::new(1.5, muted), egui::StrokeKind::Inside);
    }

    let text_x = rect.left() + 44.0;
    let text_w = (rect.right() - 150.0 - text_x).max(40.0);
    let name = item.name();
    let dir = item
        .path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let name_g = single_line(
        ui,
        name,
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
        text_w,
    );
    let dir_g = tail_line(ui, dir, FontId::proportional(11.5), muted, text_w);
    p.galley(Pos2::new(text_x, rect.top() + 6.0), name_g, muted);
    p.galley(Pos2::new(text_x, rect.top() + 25.0), dir_g, muted);

    if hovered {
        let fill = if reveal.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().widgets.inactive.weak_bg_fill
        };
        p.rect_filled(reveal_rect, 7.0, fill);
        p.text(
            reveal_rect.center(),
            Align2::CENTER_CENTER,
            "📂",
            FontId::proportional(14.0),
            ui.visuals().text_color(),
        );
    }
    // Says why this row stays unticked after "Select all". On hover this spot holds the reveal button.
    if first_copy && !item.selected && !hovered {
        p.text(
            rect.right_center() - vec2(96.0, 0.0),
            Align2::RIGHT_CENTER,
            t("Korunur", "Kept", "Conservada"),
            FontId::proportional(11.5),
            ACCENT,
        );
    }
    p.text(
        rect.right_center() - vec2(16.0, 0.0),
        Align2::RIGHT_CENTER,
        human_size(item.size),
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
    );
}

/// `df`-style line for the disk map: a used/free bar and the numbers.
fn disk_bar(ui: &mut Ui, total: u64, free: u64, mono: &FontId) {
    let used = total.saturating_sub(free);
    let frac = used as f32 / total as f32;
    // Nearly full disks are what a cleaner is for: show them in red.
    let color = if frac > 0.9 { DANGER } else { TERM };
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("Disk", "Disk", "Disco")).font(mono.clone()).color(TERM_DIM));
        let (rect, _) = ui.allocate_exact_size(vec2((ui.available_width() * 0.35).clamp(120.0, 320.0), 14.0), Sense::hover());
        ui.painter().rect(rect, 3.0, Color32::TRANSPARENT, Stroke::new(1.0, TERM_DIM), egui::StrokeKind::Inside);
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * frac, rect.height())), 3.0, color.gamma_multiply(0.8));
        let (pct, used, total, free) = (frac * 100.0, human_size(used), human_size(total), human_size(free));
        let text = tf!("%{pct:.0} dolu · {used} / {total} · {free} boş", "{pct:.0}% used · {used} / {total} · {free} free", "{pct:.0} % usado · {used} / {total} · {free} libre");
        ui.label(RichText::new(text).font(mono.clone()).color(color));
    });
}

/// Label for [`reveal_in_folder`], named after the platform's file manager.
fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        t("Finder'da göster", "Show in Finder", "Mostrar en Finder")
    } else if cfg!(windows) {
        t("Dosya Gezgini'nde göster", "Show in File Explorer", "Mostrar en el Explorador de archivos")
    } else {
        t("Klasörde göster", "Show in folder", "Mostrar en la carpeta")
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSString, NSURL};
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
            objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let result = if cfg!(windows) {
            std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(url).spawn()
        };
        if let Err(e) = result {
            eprintln!("could not open {url}: {e}");
        }
    }
}

fn reveal_in_folder(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSArray, NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        objc2_app_kit::NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if cfg!(windows) {
            if let Err(e) = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn() {
                eprintln!("could not open folder: {e}");
            }
            return;
        }
        // The file manager selects the file through the freedesktop FileManager1 D-Bus API (Nautilus, Dolphin,
        // Nemo, Caja, Thunar…); without one, its folder opens. In a thread: dbus-send waits for the reply.
        #[cfg(unix)]
        let url = {
            use std::os::unix::ffi::OsStrExt;
            // Percent-encoded, which also keeps commas from splitting dbus-send's array.
            let safe = |b: u8| b.is_ascii_alphanumeric() || b"/-._~".contains(&b);
            path.as_os_str().as_bytes().iter().fold("file://".to_string(), |u, &b| if safe(b) { u + &(b as char).to_string() } else { u + &format!("%{b:02X}") })
        };
        #[cfg(not(unix))]
        let url = String::new();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let shown = std::process::Command::new("dbus-send")
                .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems"])
                .arg(format!("array:string:{url}"))
                .arg("string:")
                .output()
                .is_ok_and(|o| o.status.success());
            if !shown && let Err(e) = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(&path)).status() {
                eprintln!("could not open folder: {e}");
            }
        });
    }
}

#[cfg(feature = "appstore")]
fn store_buy(yearly: bool) {
    store::buy(yearly);
}

#[cfg(not(feature = "appstore"))]
fn store_buy(_: bool) {}

#[cfg(feature = "appstore")]
fn store_restore() {
    store::restore();
}

#[cfg(not(feature = "appstore"))]
fn store_restore() {}
