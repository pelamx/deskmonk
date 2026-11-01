//! Terminal UI for servers / cloud machines: `deskmonk-tui [FOLDER]` (default: home).
//! `deskmonk-tui --key KEY` turns Pro on where the desktop app can't run.

use deskmonk::i18n::{self, Lang, t};
use deskmonk::plan::{self, Plan};
use deskmonk::scan::{Category, Item, Phase, human_size};
use deskmonk::tf;
use deskmonk::telemetry::{self, Telemetry};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Tabs};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

#[derive(Clone, PartialEq)]
enum Prompt {
    None,
    Consent,
    Trash,
    Delete,
    /// Typing a license key (p).
    Key(String),
}

struct App {
    root: PathBuf,
    items: Vec<Item>,
    tab: usize,
    list: ListState,
    /// Messages are (final, items): first the partial list without duplicates, then the final one.
    scanning: Option<(Receiver<(Option<Phase>, Vec<Item>)>, Arc<AtomicUsize>)>,
    /// Phase of the partial results on screen.
    phase: Option<Phase>,
    prompt: Prompt,
    deleting: Option<deskmonk::Deletion>,
    status: String,
    telemetry: Telemetry,
    plan: Plan,
    /// A newer version on the website, found at startup; shown in the header.
    update_check: Receiver<String>,
    new_version: Option<String>,
}

fn main() -> io::Result<()> {
    i18n::init();
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--key") {
        activate(std::env::args().nth(2).unwrap_or_default().trim());
    }
    let root = arg
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    if !root.is_dir() {
        let root = root.display();
        eprintln!("{}", tf!("Klasör bulunamadı: {root}", "Folder not found: {root}", "Carpeta no encontrada: {root}"));
        std::process::exit(1);
    }
    let telemetry = Telemetry::load();
    telemetry.started("tui");
    let mut plan = Plan::load();
    // The paid period ran out: the subscription may have renewed. Asked only then, so offline starts stay instant.
    if plan::URL.is_some() && !plan.key.is_empty() && !plan.is_pro()
        && let Ok((key, until)) = plan::validate(&plan.key, &telemetry.install_id)
    {
        plan.set_license(key, until);
    }
    let (tx, update_check) = channel();
    std::thread::spawn(move || {
        // Only announced here: the terminal version may be the static Linux build, which the archives don't replace.
        if let Some(deskmonk::update::Found::Available(v) | deskmonk::update::Found::Installed(v)) = deskmonk::update::check(false) {
            let _ = tx.send(v);
        }
    });
    let prompt = if telemetry.needs_prompt() {
        Prompt::Consent
    } else {
        Prompt::None
    };
    let mut app = App {
        root,
        items: Vec::new(),
        tab: 0,
        list: ListState::default(),
        scanning: None,
        phase: None,
        prompt,
        deleting: None,
        status: String::new(),
        telemetry,
        plan,
        update_check,
        new_version: None,
    };
    app.start_scan();
    app.send_opened();
    ratatui::run(|t| app.run(t))
}

impl App {
    /// p: checks a typed key with the license server, like the desktop app's "Activate". Blocks the screen while it asks.
    fn activate_key(&mut self, key: &str) {
        self.status = match plan::validate(key, &self.telemetry.install_id) {
            Ok((key, until)) if until > 0 => {
                self.plan.set_license(key, until);
                let date = plan::date(until);
                tf!("Pro açık, {date} tarihine kadar.", "Pro is on until {date}.", "Pro activo hasta el {date}.")
            }
            Ok(_) => t("Bu anahtarın etkin bir aboneliği yok", "This key has no active subscription", "Esta clave no tiene una suscripción activa").into(),
            Err(e) => e,
        };
    }

    /// Hardware detection spawns system tools, so it runs off the UI thread.
    fn send_opened(&self) {
        if self.telemetry.consent == Some(true) {
            let telemetry = self.telemetry.clone();
            std::thread::spawn(move || telemetry.opened("tui", &deskmonk::hardware::detect()));
        }
    }

    fn start_scan(&mut self) {
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (root, counter) = (self.root.clone(), progress.clone());
        std::thread::spawn(move || {
            let partial_tx = tx.clone();
            let items = deskmonk::scan_default(&root, &counter, move |items, phase| {
                let _ = partial_tx.send((Some(phase), items));
            });
            tx.send((None, items))
        });
        self.items.clear();
        self.scanning = Some((rx, progress));
    }

    fn visible(&self) -> Vec<usize> {
        let cat = Category::ALL[self.tab];
        (0..self.items.len())
            .filter(|&i| self.items[i].category == cat)
            .collect()
    }

    fn selected_totals(&self) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| i.selected)
            .fold((0, 0), |(c, s), i| (c + 1, s + i.size))
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            while let Some((rx, progress)) = &self.scanning
                && let Ok((phase, mut items)) = rx.try_recv()
            {
                self.phase = phase;
                if phase.is_none() {
                    self.telemetry
                        .scan_done("tui", progress.load(Ordering::Relaxed), &items);
                    self.scanning = None;
                }
                deskmonk::keep_selection(&self.items, &mut items);
                if self.items.is_empty() {
                    self.list.select(Some(0));
                }
                self.items = items;
            }
            if let Some(done) = self.deleting.as_ref().and_then(|d| d.poll()) {
                let ok = self.deleting.take().unwrap().total - done.failed.len();
                self.telemetry.clean_done("tui", ok, done.freed);
                self.plan.record_clean(done.freed);
                let freed = human_size(done.freed);
                self.status = tf!("{ok} öğe temizlendi ({freed})", "{ok} items cleaned ({freed})", "{ok} elementos limpiados ({freed})");
                if let Some(err) = done.first_error {
                    let n = done.failed.len();
                    self.status += &tf!(", {n} tamamen silinemedi: {err}", ", {n} not fully deleted: {err}", ", {n} no se borraron del todo: {err}");
                }
                self.items.extend(done.failed);
            }
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Yes/no keys of the UI language: evet/hayır, yes/no, sí/no.
            let (y, n) = match i18n::lang() {
                Lang::Tr => ('e', 'h'),
                Lang::En => ('y', 'n'),
                Lang::Es => ('s', 'n'),
            };
            let yes = matches!(key.code, KeyCode::Char(c) if c.to_ascii_lowercase() == y);
            let no = matches!(key.code, KeyCode::Char(c) if c.to_ascii_lowercase() == n) || key.code == KeyCode::Esc;
            match std::mem::replace(&mut self.prompt, Prompt::None) {
                Prompt::Consent => match key.code {
                    _ if yes => {
                        self.telemetry.set_consent(true);
                        self.send_opened();
                    }
                    _ if no => self.telemetry.set_consent(false),
                    _ => self.prompt = Prompt::Consent,
                },
                Prompt::Key(mut typed) => match key.code {
                    KeyCode::Enter if !typed.trim().is_empty() => self.activate_key(typed.trim()),
                    KeyCode::Esc => {}
                    KeyCode::Backspace => {
                        typed.pop();
                        self.prompt = Prompt::Key(typed);
                    }
                    KeyCode::Char(c) if typed.len() < 32 => {
                        typed.push(c.to_ascii_uppercase());
                        self.prompt = Prompt::Key(typed);
                    }
                    _ => self.prompt = Prompt::Key(typed),
                },
                p @ (Prompt::Trash | Prompt::Delete) => {
                    if yes {
                        // Runs in the background; the screen keeps updating with progress.
                        self.deleting = Some(deskmonk::Deletion::start(
                            &mut self.items,
                            p == Prompt::Delete,
                            || {},
                        ));
                        self.status.clear();
                    } else {
                        self.status = t("Vazgeçildi", "Cancelled", "Cancelado").into();
                    }
                }
                // Quitting, rescanning or a second deletion must wait for the running one.
                Prompt::None
                    if self.deleting.is_some()
                        && matches!(
                            key.code,
                            KeyCode::Char('q' | 'd' | 'D' | 'r') | KeyCode::Esc
                        ) =>
                {
                    self.status = t(
                        "Silme sürüyor, lütfen bekleyin…",
                        "Deleting, please wait…",
                        "Borrando, espera por favor…",
                    )
                    .into();
                }
                Prompt::None => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Tab | KeyCode::Right => {
                        self.tab = (self.tab + 1) % Category::ALL.len();
                        self.list.select(Some(0));
                    }
                    KeyCode::BackTab | KeyCode::Left => {
                        self.tab = (self.tab + Category::ALL.len() - 1) % Category::ALL.len();
                        self.list.select(Some(0));
                    }
                    KeyCode::Down | KeyCode::Char('j') => self.list.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => self.list.select_previous(),
                    KeyCode::PageDown => self.list.scroll_down_by(20),
                    KeyCode::PageUp => self.list.scroll_up_by(20),
                    KeyCode::Char(' ') => {
                        if let Some(i) = self
                            .list
                            .selected()
                            .and_then(|s| self.visible().get(s).copied())
                        {
                            self.items[i].selected ^= true;
                            self.list.select_next();
                        }
                    }
                    KeyCode::Char(c @ ('a' | 'n')) => deskmonk::set_selection(
                        &mut self.items,
                        Some(Category::ALL[self.tab]),
                        c == 'a',
                    ),
                    // The final scan list would bring deleted items back, so deleting waits for it.
                    KeyCode::Char('d' | 'D') if self.scanning.is_some() => {
                        self.status = t(
                            "Kopyalar hâlâ karşılaştırılıyor, tarama bitince silebilirsiniz.",
                            "Duplicates are still being compared; you can delete when the scan ends.",
                            "Todavía se comparan los duplicados; podrás borrar cuando termine el análisis.",
                        )
                        .into()
                    }
                    KeyCode::Char('d' | 'D') if self.plan.clean_left().is_some_and(|left| self.selected_totals().1 > left) => {
                        let left = human_size(self.plan.clean_left().unwrap_or(0));
                        self.telemetry.used("tui", "pro", "clean_limit");
                        self.status = tf!(
                            "Ücretsiz plan günde 2 GB temizler; bugün {left} kaldı. Sınırsız: deskmonk.app/buy, sonra anahtar için p",
                            "The free plan cleans 2 GB a day; {left} left today. No limit: deskmonk.app/buy, then p to enter the key",
                            "El plan gratuito limpia 2 GB al día; quedan {left} hoy. Sin límite: deskmonk.app/buy y luego p para la clave"
                        );
                    }
                    KeyCode::Char('d') if self.selected_totals().0 > 0 => {
                        self.prompt = Prompt::Trash
                    }
                    KeyCode::Char('D') if self.selected_totals().0 > 0 => {
                        self.prompt = Prompt::Delete
                    }
                    KeyCode::Char('r') if self.scanning.is_none() => self.start_scan(),
                    KeyCode::Char('p') if plan::URL.is_some() => self.prompt = Prompt::Key(String::new()),
                    KeyCode::Char('t') if telemetry::URL.is_some() => {
                        let on = self.telemetry.consent != Some(true);
                        self.telemetry.set_consent(on);
                        self.status = if on {
                            t("Anonim istatistik: açık", "Anonymous stats: on", "Estadísticas anónimas: activadas")
                        } else {
                            t("Anonim istatistik: kapalı", "Anonymous stats: off", "Estadísticas anónimas: desactivadas")
                        }
                        .into();
                    }
                    KeyCode::Char('l') => {
                        let next = i18n::ALL.iter().position(|l| *l == i18n::lang()).map_or(0, |i| i + 1);
                        i18n::set(i18n::ALL[next % i18n::ALL.len()]);
                    }
                    _ => {}
                },
            }
        }
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let [header, tabs, body, footer, help] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(f.area());

        let scanning = match (&self.scanning, &self.deleting) {
            (Some(_), _) if self.phase == Some(Phase::Comparing) => {
                t("  — kopyalar karşılaştırılıyor…", "  — comparing duplicates…", "  — comparando duplicados…").to_string()
            }
            (Some((_, p)), _) => {
                let n = p.load(Ordering::Relaxed);
                tf!("  — taranıyor… {n} dosya", "  — scanning… {n} files", "  — analizando… {n} archivos")
            }
            (_, Some(d)) => {
                let (done, total) = (d.done.load(Ordering::Relaxed), d.total);
                tf!("  — siliniyor… {done}/{total}", "  — deleting… {done}/{total}", "  — borrando… {done}/{total}")
            }
            _ => String::new(),
        };
        if let Ok(v) = self.update_check.try_recv() {
            self.new_version = Some(v);
        }
        let update = match (&self.new_version, plan::URL) {
            (Some(v), Some(site)) => tf!("  ⬆ Yeni sürüm {v}: {site}", "  ⬆ New version {v}: {site}", "  ⬆ Nueva versión {v}: {site}"),
            _ => String::new(),
        };
        f.render_widget(
            Paragraph::new(format!(
                "deskMonk by pelamx  {}{scanning}{update}",
                self.root.display()
            ))
            .bold(),
            header,
        );

        let titles = Category::ALL.map(|cat| {
            let (n, size) = deskmonk::removable(&self.items, Some(cat));
            format!("{} ({n}, {})", cat.label(), human_size(size))
        });
        f.render_widget(
            Tabs::new(titles)
                .select(self.tab)
                .highlight_style(Style::new().reversed()),
            tabs,
        );

        // shortcut: builds every row each frame; fine up to ~100k items, switch to a windowed render if lists get bigger.
        let rows: Vec<ListItem> = self
            .visible()
            .into_iter()
            .map(|i| {
                let it = &self.items[i];
                let group = if it.group > 0 {
                    format!("#{:<4} ", it.group)
                } else {
                    String::new()
                };
                ListItem::new(format!(
                    "[{}] {:>10}  {group}{}",
                    if it.selected { "x" } else { " " },
                    human_size(it.size),
                    it.path.display()
                ))
            })
            .collect();
        let list = List::new(rows)
            .block(Block::bordered())
            .highlight_style(Style::new().reversed());
        f.render_stateful_widget(list, body, &mut self.list);

        let (n, size) = self.selected_totals();
        f.render_widget(
            Paragraph::new({
                let (gain, status) = (human_size(size), &self.status);
                tf!(
                    "Seçili: {n} dosya — Kazanılacak alan: {gain}   {status}",
                    "Selected: {n} files — Space to free: {gain}   {status}",
                    "Seleccionados: {n} archivos — Espacio a liberar: {gain}   {status}"
                )
            })
            .bold(),
            footer,
        );
        let tkey = if telemetry::URL.is_some() {
            t("  t istatistik", "  t stats", "  t estadísticas")
        } else {
            ""
        };
        let pkey = if plan::URL.is_some() {
            t("  p lisans anahtarı", "  p license key", "  p clave de licencia")
        } else {
            ""
        };
        let keys = tf!(
            "↑↓ gezin  boşluk seç  a/n tümü/hiçbiri  ←→ kategori  d temizle  D hepsini kalıcı sil  r yeniden tara{tkey}{pkey}  l dil  q çık",
            "↑↓ move  space select  a/n all/none  ←→ category  d clean  D delete all for good  r rescan{tkey}{pkey}  l language  q quit",
            "↑↓ mover  espacio elegir  a/n todo/nada  ←→ categoría  d limpiar  D borrar todo definitivamente  r reanalizar{tkey}{pkey}  l idioma  q salir"
        );
        f.render_widget(Paragraph::new(keys).dim(), help);

        let (text, title) = (match &self.prompt {
            Prompt::None => return,
            Prompt::Key(typed) => {
                let pro = if self.plan.is_pro() {
                    tf!("Pro açık, {} tarihine kadar.\n", "Pro is on until {}.\n", "Pro activo hasta el {}.\n", plan::date(self.plan.pro_until))
                } else {
                    String::new()
                };
                tf!(
                    "{pro}Lisans anahtarı (deskmonk.app/buy): {typed}_\nEnter etkinleştir, Esc vazgeç",
                    "{pro}License key (deskmonk.app/buy): {typed}_\nEnter activates, Esc cancels",
                    "{pro}Clave de licencia (deskmonk.app/buy): {typed}_\nEnter activa, Esc cancela"
                )
            }
            Prompt::Consent => t(
                "Anonim kullanım istatistikleri (dosya sayısı, boyut, işletim sistemi) ve donanım bilgileri\n(işlemci, bellek, disk, ekran kartı, pil: marka, model, doluluk) gönderilsin mi?\nDosya adları, yollar, seri numaraları ve kullanıcı adı asla gönderilmez.  [e]vet / [h]ayır",
                "Send anonymous usage stats (file count, size, operating system) and hardware info\n(processor, memory, disk, graphics, battery: brand, model, usage)?\nFile names, paths, serial numbers and user names are never sent.  [y]es / [n]o",
                "¿Enviar estadísticas de uso anónimas (cantidad de archivos, tamaño, sistema operativo) e información\nde hardware (procesador, memoria, disco, gráficos, batería: marca, modelo, uso)?\nNunca se envían nombres de archivo, rutas, números de serie ni nombres de usuario.  [s]í / [n]o",
            )
            .to_string(),
            Prompt::Trash => {
                let size = human_size(size);
                tf!(
                    "{n} öğe ({size}) temizlenecek: gereksiz dosyalar kalıcı silinir, diğerleri çöp kutusuna taşınır. Emin misiniz? [e/h]",
                    "{n} items ({size}) will be cleaned: junk is deleted for good, the rest goes to the trash. Are you sure? [y/n]",
                    "Se limpiarán {n} elementos ({size}): los archivos basura se borran definitivamente y el resto va a la papelera. ¿Seguro? [s/n]"
                )
            }
            Prompt::Delete => {
                let size = human_size(size);
                tf!(
                    "{n} dosya ({size}) KALICI olarak silinecek, geri alınamaz! Emin misiniz? [e/h]",
                    "{n} files ({size}) will be deleted FOR GOOD, this cannot be undone! Are you sure? [y/n]",
                    "Se borrarán DEFINITIVAMENTE {n} archivos ({size}), ¡no se puede deshacer! ¿Seguro? [s/n]"
                )
            }
        }, if matches!(self.prompt, Prompt::Key(_)) { " Pro " } else { t(" Onay ", " Confirm ", " Confirmar ") });
        let area = centered(f.area(), 70, 5);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .block(Block::bordered().title(title)),
            area,
        );
    }
}

fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, mid, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Fill(1),
    ])
    .areas(mid);
    mid
}

/// `--key KEY`: checks the key with the license server and saves it, like the desktop app's "Enter key".
fn activate(key: &str) -> ! {
    let fail = |msg: String| -> ! {
        eprintln!("{msg}");
        std::process::exit(1)
    };
    if plan::URL.is_none() {
        println!("{}", t("Bu sürümde Pro sınırı yok.", "This build has no Pro limits.", "Esta versión no tiene límites Pro."));
        std::process::exit(0);
    }
    if key.is_empty() {
        fail(t("Kullanım: deskmonk-tui --key ANAHTAR", "Usage: deskmonk-tui --key KEY", "Uso: deskmonk-tui --key CLAVE").into());
    }
    match plan::validate(key, &Telemetry::load().install_id) {
        Ok((key, until)) if until > 0 => {
            Plan::load().set_license(key, until);
            let date = plan::date(until);
            println!("{}", tf!("Pro açık, {date} tarihine kadar.", "Pro is on until {date}.", "Pro activo hasta el {date}."));
            std::process::exit(0)
        }
        Ok(_) => fail(t("Bu anahtarın etkin bir aboneliği yok", "This key has no active subscription", "Esta clave no tiene una suscripción activa").into()),
        Err(e) => fail(e),
    }
}
