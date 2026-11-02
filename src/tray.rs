//! deskMonk's icon in the macOS menu bar, the Windows notification area and the Linux status tray: its menu opens
//! the window (also while it is hidden) or quits, and shows the version.

use deskmonk::i18n::t;

/// What the icon's menu asks for.
pub enum Action {
    Open,
    Quit,
}

/// Keeps the icon alive; it goes away with this.
pub struct Tray(#[allow(dead_code)] Box<dyn std::any::Any>);

const ICON: &[u8] = include_bytes!("../assets/icon.png");

// shortcut: the menu is worded in the language at start; a language change shows after the next start.
fn labels() -> [String; 3] {
    [
        t("deskMonk'ı aç", "Open deskMonk", "Abrir deskMonk").into(),
        concat!("deskMonk v", env!("CARGO_PKG_VERSION")).into(),
        t("Çık", "Quit", "Salir").into(),
    ]
}

/// Puts the icon up; `send` hears the menu clicks. None where there is no tray (logged).
#[cfg(any(target_os = "macos", windows))]
pub fn start(send: impl Fn(Action) + Send + Sync + 'static) -> Option<Tray> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    let [open, version, quit] = labels();
    let (open, version, quit) = (MenuItem::new(open, true, None), MenuItem::new(version, false, None), MenuItem::new(quit, true, None));
    let menu = Menu::new();
    menu.append_items(&[&open, &PredefinedMenuItem::separator(), &version, &PredefinedMenuItem::separator(), &quit])
        .map_err(|e| eprintln!("tray: {e}"))
        .ok()?;
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            send(Action::Open);
        } else if e.id == quit_id {
            send(Action::Quit);
        }
    }));
    let img = eframe::icon_data::from_png_bytes(ICON).ok()?;
    let icon = tray_icon::Icon::from_rgba(img.rgba, img.width, img.height).ok()?;
    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("deskMonk")
        .with_icon(icon)
        .build()
        .map_err(|e| eprintln!("tray: {e}"))
        .ok()?;
    Some(Tray(Box::new(tray)))
}

/// Puts the icon up through the desktop's StatusNotifierItem service (KDE, most others; GNOME needs the
/// AppIndicator extension). `send` hears the menu clicks. None without such a service (logged).
#[cfg(target_os = "linux")]
pub fn start(send: impl Fn(Action) + Send + Sync + 'static) -> Option<Tray> {
    use ksni::blocking::TrayMethods;
    struct Item {
        send: Box<dyn Fn(Action) + Send + Sync>,
        labels: [String; 3],
        icon: ksni::Icon,
    }
    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "deskmonk".into()
        }
        fn title(&self) -> String {
            "deskMonk".into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![self.icon.clone()]
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            (self.send)(Action::Open);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            vec![
                StandardItem { label: self.labels[0].clone(), activate: Box::new(|i: &mut Self| (i.send)(Action::Open)), ..Default::default() }.into(),
                ksni::MenuItem::Separator,
                StandardItem { label: self.labels[1].clone(), enabled: false, ..Default::default() }.into(),
                ksni::MenuItem::Separator,
                StandardItem { label: self.labels[2].clone(), activate: Box::new(|i: &mut Self| (i.send)(Action::Quit)), ..Default::default() }.into(),
            ]
        }
    }
    let img = eframe::icon_data::from_png_bytes(ICON).ok()?;
    // RGBA → ARGB, as the StatusNotifierItem spec wants it.
    let data = img.rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    let icon = ksni::Icon { width: img.width as i32, height: img.height as i32, data };
    let handle = Item { send: Box::new(send), labels: labels(), icon }.spawn().map_err(|e| eprintln!("tray: {e}")).ok()?;
    Some(Tray(Box::new(handle)))
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn start(_send: impl Fn(Action) + Send + Sync + 'static) -> Option<Tray> {
    None
}
