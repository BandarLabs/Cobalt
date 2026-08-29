//! Cobalt's unprivileged app-store interface.
//!
//! Catalog downloads, signature checks and filesystem changes remain inside
//! `kobod`. This process receives display metadata and submits app identities;
//! it never receives a package URL or chooses an installation path.

use kobo_sdk::{
    action_id, ActionId, AppInfo, AppLinkState, Context, DenyReason, DeviceRequest, DeviceResult,
    Glyph, Heartbeat, KoboApp, PictureHandle, Position, RemoteInstallOutcome, RowLead, Screen,
    ScreenBuilder, TaskId, TaskOutcome, TilePicture,
};
use qrcodegen::{QrCode, QrCodeEcc};
use std::process::ExitCode;

const REFRESH: &str = "refresh";
const APP_LINK: &str = "app-link";
const BEGIN_LINK: &str = "begin-link";
const DISCONNECT_LINK: &str = "disconnect-link";
const PREVIOUS: &str = "previous";
const NEXT: &str = "next";
const QR_HANDLE: PictureHandle = PictureHandle(1);
const QR_SCALE: u32 = 7;
const QR_QUIET_ZONE: i32 = 4;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Catalog,
    Detail(String),
    Working {
        id: String,
        action: &'static str,
    },
    AppLink,
}

struct Store {
    entries: Vec<AppInfo>,
    view: View,
    page: usize,
    refreshing: bool,
    refresh_after_cache: bool,
    notice: Option<String>,
    app_link: AppLinkState,
    link_poll: Heartbeat,
    link_request_pending: bool,
    pairing_qr: Option<TilePicture>,
    pairing_qr_url: Option<String>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            view: View::Catalog,
            page: 0,
            refreshing: false,
            refresh_after_cache: false,
            notice: None,
            app_link: AppLinkState::Unpaired,
            link_poll: Heartbeat::every(5),
            link_request_pending: false,
            pairing_qr: None,
            pairing_qr_url: None,
        }
    }
}

impl Store {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view.clone() {
            View::Catalog => self.catalog(context),
            View::Detail(id) => self.detail(&id),
            View::Working { id, action } => self.working(&id, action),
            View::AppLink => self.app_link(),
        };
        context.set_screen(screen);
    }

    fn catalog(&mut self, context: &Context) -> Screen {
        let states = self.entries.iter().map(app_state).collect::<Vec<_>>();
        let rows = self
            .entries
            .iter()
            .zip(&states)
            .map(|(entry, state)| (entry.title.as_str(), entry.summary.as_str(), state.as_str()))
            .collect::<Vec<_>>();
        let without_controls =
            context.paginate_rows_with_trailing_at(&rows, false, Position::Elsewhere);
        // A one-page catalog draws no bottom bar and gets that room for apps.
        // Once it turns, measure again with the navigation bar it will draw.
        let page_indices = if without_controls.len() > 1 {
            context.paginate_rows_with_trailing_at(&rows, true, Position::Elsewhere)
        } else {
            without_controls
        };
        let pages = page_indices.len();
        self.page = self.page.min(pages - 1);
        let mut screen = ScreenBuilder::new("store-catalog")
            .top_bar("App Store")
            .top_bar_glyph(REFRESH, "Refresh", Glyph::Refresh)
            .top_bar_glyph(APP_LINK, "Install links", Glyph::Globe);
        if let Some(notice) = &self.notice {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, notice.clone());
        }
        if self.entries.is_empty() {
            screen = screen
                .splash(
                    Some(Glyph::Download),
                    if self.refreshing {
                        "Refreshing apps"
                    } else {
                        "No apps available"
                    },
                    if self.refreshing {
                        "The last verified catalog is shown first; the current GitHub release is being checked now."
                    } else {
                        "Connect Wi-Fi and refresh the catalog."
                    },
                )
                .bottom_action_marked(REFRESH, "Refresh", Glyph::Refresh);
            return screen.build();
        }
        screen = screen
            .section_with_value(
                if self.refreshing {
                    "Apps · refreshing"
                } else {
                    "Apps"
                },
                format!("{} / {pages}", self.page + 1),
            )
            .rows_with_trailing(
                page_indices[self.page]
                    .iter()
                    .filter_map(|index| self.entries.get(*index))
                    .map(|entry| {
                        (
                            app_action(&entry.id),
                            entry.title.clone(),
                            entry.summary.clone(),
                            RowLead::from(entry.glyph),
                            app_state(entry),
                        )
                    }),
            );
        if pages > 1 {
            let mut actions = Vec::new();
            if self.page > 0 {
                actions.push((PREVIOUS, "Previous", Some(Glyph::Previous)));
            }
            if self.page + 1 < pages {
                actions.push((NEXT, "More", Some(Glyph::Next)));
            }
            screen = if actions.len() == 1 {
                let (id, label, glyph) = actions[0];
                screen.bottom_action_marked(id, label, glyph.expect("page action glyph"))
            } else {
                screen.action_bar_marked(actions)
            };
        }
        screen.build()
    }

    fn app_link(&self) -> Screen {
        let mut screen = ScreenBuilder::new("store-app-link")
            .top_bar("Install links")
            .owns_back(true);
        if let Some(notice) = &self.notice {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, notice.clone());
        }
        match &self.app_link {
            AppLinkState::Unpaired => screen
                .splash(
                    Some(Glyph::Globe),
                    "Link this Kobo",
                    "Link a browser to install apps from Cobalt app pages. Requests are encrypted for this Kobo.",
                )
                .bottom_action_marked(
                    if self.notice.is_some() {
                        DISCONNECT_LINK
                    } else {
                        BEGIN_LINK
                    },
                    if self.notice.is_some() {
                        "Reset link"
                    } else {
                        "Link browser"
                    },
                    if self.notice.is_some() {
                        Glyph::Refresh
                    } else {
                        Glyph::Key
                    },
                )
                .build(),
            AppLinkState::Pairing {
                code,
                url,
                expires_in,
            } => {
                let minutes = (*expires_in).div_ceil(60);
                let verification = pairing_verification(url).unwrap_or_else(|| "Unavailable".into());
                let mut screen = screen.heading("Link this browser").text(
                    "Scan the QR code. To pair manually, open the address below and enter both values.",
                );
                if let Some(picture) = self.pairing_qr {
                    screen = screen.picture(picture, 58);
                }
                screen
                    .section("Enter manually")
                    .splash(
                        None,
                        format!("{} {}", &code[..4], &code[4..]),
                        format!("Verification key\n{verification}"),
                    )
                    .facts([
                        (
                            "Address",
                            url.split_once('#')
                                .map_or_else(|| url.clone(), |(base, _)| base.to_owned()),
                        ),
                        (
                            "Expires",
                            format!("in {minutes} minute{}", if minutes == 1 { "" } else { "s" }),
                        ),
                    ])
                    .bottom_action_marked(DISCONNECT_LINK, "Cancel", Glyph::Close)
                    .build()
            }
            AppLinkState::Paired { browsers } => screen
                .splash(
                    Some(Glyph::Check),
                    "Browser linked",
                    "Install requests are checked while App Store is open. Requests sent while this Kobo is offline remain queued for up to 72 hours.",
                )
                .facts([(
                    "Linked browsers",
                    format!("{browsers}"),
                )])
                .bottom_action_marked(DISCONNECT_LINK, "Disconnect all", Glyph::Trash)
                .build(),
        }
    }

    fn update_link_state(&mut self, context: &mut Context, state: AppLinkState) {
        self.link_request_pending = false;
        match &state {
            AppLinkState::Pairing { url, .. } if self.pairing_qr_url.as_deref() != Some(url) => {
                self.pairing_qr = qr_picture(context, url);
                self.pairing_qr_url = Some(url.clone());
            }
            AppLinkState::Pairing { .. } => {}
            _ if self.pairing_qr.take().is_some() => {
                context.drop_picture(QR_HANDLE);
                self.pairing_qr_url = None;
            }
            _ => {}
        }
        self.app_link = state;
        if matches!(
            self.app_link,
            AppLinkState::Pairing { .. } | AppLinkState::Paired { .. }
        ) {
            self.link_poll.start(context);
        } else {
            self.link_poll.stop(context);
        }
    }

    fn poll_link(&mut self, context: &mut Context) {
        if !self.link_request_pending {
            self.link_request_pending = true;
            context.store().poll_link();
        }
    }

    fn handle_link_result(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> bool {
        if !matches!(
            request,
            DeviceRequest::ReadAppLink
                | DeviceRequest::BeginAppLink
                | DeviceRequest::PollAppLink
                | DeviceRequest::DisconnectAppLink
        ) {
            return false;
        }
        match result {
            DeviceResult::AppLink(state) => {
                self.notice = None;
                self.update_link_state(context, state.clone());
                if *request == DeviceRequest::ReadAppLink
                    && matches!(self.app_link, AppLinkState::Paired { .. })
                {
                    self.poll_link(context);
                }
            }
            DeviceResult::RemoteInstall(outcome) if *request == DeviceRequest::PollAppLink => {
                self.link_request_pending = false;
                self.notice = remote_install_notice(outcome, &self.entries);
                if !matches!(outcome, RemoteInstallOutcome::None) {
                    context.applications().cached_catalog();
                }
            }
            DeviceResult::Failed(error) => {
                self.link_request_pending = false;
                self.notice = Some(format!(
                    "Install links are unavailable: {}. Connect Wi-Fi and try again.",
                    error.describe()
                ));
            }
            _ => return false,
        }
        true
    }

    fn detail(&self, id: &str) -> Screen {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return ScreenBuilder::new("store-missing")
                .top_bar("App Store")
                .owns_back(true)
                .error_state("This app is no longer in the verified catalog.")
                .build();
        };
        let installed = entry.installed_version.as_deref();
        let system = is_system_app(id);
        let mut screen = ScreenBuilder::new("store-detail")
            .top_bar(entry.title.clone())
            .owns_back(true)
            .splash(
                Some(entry.glyph),
                entry.title.clone(),
                entry.summary.clone(),
            )
            .facts([
                ("Available", entry.version.clone()),
                ("Installed", installed.unwrap_or("Not installed").to_owned()),
                (
                    "Management",
                    if system {
                        "Built into Cobalt".to_owned()
                    } else {
                        "Managed by App Store".to_owned()
                    },
                ),
                (
                    "Permissions",
                    if entry.capabilities.is_empty() {
                        "None".to_owned()
                    } else {
                        entry.capabilities.join(", ")
                    },
                ),
            ]);
        screen = if system {
            screen.bottom_action_marked(open_action(id), "Open", entry.glyph)
        } else if installed.is_some() {
            let mut actions = vec![
                (open_action(id), "Open", Some(entry.glyph)),
                (remove_action(id), "Uninstall", Some(Glyph::Trash)),
            ];
            if entry.has_update() {
                actions.insert(0, (install_action(id), "Update", Some(Glyph::Download)));
            }
            screen.action_bar_marked(actions)
        } else {
            screen.bottom_action_marked(install_action(id), "Install over Wi-Fi", Glyph::Download)
        };
        screen.build()
    }

    fn working(&self, id: &str, action: &str) -> Screen {
        let title = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or(id, |entry| entry.title.as_str());
        ScreenBuilder::new("store-working")
            .top_bar("App Store")
            .splash(
                Some(Glyph::Download),
                format!("{action} {title}"),
                "Keep Cobalt open. The verified app transaction is completed before the installed copy changes.",
            )
            .build()
    }

    fn replace_entries(&mut self, mut entries: Vec<AppInfo>) {
        entries.sort_by(|left, right| {
            left.title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        self.entries = entries;
    }

    fn request_install(&mut self, context: &mut Context, id: String) {
        let action = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .filter(|entry| entry.is_installed())
            .map_or("Installing", |_| "Updating");
        self.notice = None;
        self.view = View::Working {
            id: id.clone(),
            action,
        };
        self.show(context);
        if !context.applications().install(id) {
            self.notice = Some("That application identity is invalid.".to_owned());
            self.view = View::Catalog;
            self.show(context);
        }
    }

    fn request_uninstall(&mut self, context: &mut Context, id: String) {
        self.notice = None;
        self.view = View::Working {
            id: id.clone(),
            action: "Removing",
        };
        self.show(context);
        if !context.applications().uninstall(id) {
            self.notice = Some("That application identity is invalid.".to_owned());
            self.view = View::Catalog;
            self.show(context);
        }
    }
}

impl KoboApp for Store {
    fn on_start(&mut self, context: &mut Context) {
        self.refreshing = true;
        self.refresh_after_cache = true;
        self.show(context);
        context.applications().cached_catalog();
        context.store().read_link();
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.view = View::Catalog;
            self.show(context);
            return;
        }
        if action == action_id(APP_LINK) {
            self.view = View::AppLink;
            self.notice = None;
            self.show(context);
            context.store().read_link();
            return;
        }
        if action == action_id(BEGIN_LINK) {
            self.link_request_pending = true;
            context.store().begin_link();
            return;
        }
        if action == action_id(DISCONNECT_LINK) {
            self.link_request_pending = true;
            context.store().disconnect_link();
            return;
        }
        if action == action_id(REFRESH) {
            self.notice = None;
            self.refreshing = true;
            self.show(context);
            context.applications().refresh_catalog();
            return;
        }
        if action == action_id(PREVIOUS) || action == action_id(NEXT) {
            self.page = if action == action_id(NEXT) {
                self.page.saturating_add(1)
            } else {
                self.page.saturating_sub(1)
            };
            self.show(context);
            return;
        }
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| action == action_id(&app_action(&entry.id)))
        {
            self.view = View::Detail(entry.id.clone());
            self.show(context);
            return;
        }
        if let Some(entry) = self.entries.iter().find(|entry| {
            action == action_id(&install_action(&entry.id))
                || action == action_id(&open_action(&entry.id))
                || action == action_id(&remove_action(&entry.id))
        }) {
            let id = entry.id.clone();
            if action == action_id(&open_action(&id)) {
                context.launch(id);
            } else if action == action_id(&remove_action(&id)) {
                self.request_uninstall(context, id);
            } else {
                self.request_install(context, id);
            }
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if self.handle_link_result(context, &request, &result) {
            self.show(context);
            return;
        }
        let mut refresh_after_paint = false;
        match (request, result) {
            (DeviceRequest::ReadAppCatalog, DeviceResult::Apps { entries }) => {
                self.replace_entries(entries);
                if !matches!(self.view, View::Working { .. } | View::AppLink) {
                    self.view = View::Catalog;
                }
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Apps { entries }) => {
                self.replace_entries(entries);
                self.refreshing = false;
                if !matches!(self.view, View::Working { .. } | View::AppLink) {
                    self.notice = None;
                    self.view = View::Catalog;
                }
            }
            (DeviceRequest::InstallApp { id }, DeviceResult::Done) => {
                let title = self
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .map_or_else(|| id.clone(), |entry| entry.title.clone());
                let outcome = match &self.view {
                    View::Working {
                        id: working_id,
                        action: "Updating",
                    } if working_id == &id => "updated",
                    _ => "installed",
                };
                self.notice = Some(format!("{title} {outcome} successfully."));
                self.view = View::Catalog;
                context.applications().cached_catalog();
            }
            (DeviceRequest::UninstallApp { id }, DeviceResult::Done) => {
                let title = self
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .map_or_else(|| id.clone(), |entry| entry.title.clone());
                self.notice = Some(format!("{title} removed successfully."));
                self.view = View::Catalog;
                context.applications().cached_catalog();
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Failed(error)) => {
                self.refreshing = false;
                self.notice = Some(format!(
                    "The catalog could not be refreshed: {}. The last verified list is still shown.",
                    error.describe()
                ));
                if !matches!(self.view, View::Working { .. } | View::AppLink) {
                    self.view = View::Catalog;
                }
            }
            (DeviceRequest::ReadAppCatalog, DeviceResult::Failed(_)) => {
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (
                DeviceRequest::InstallApp { .. } | DeviceRequest::UninstallApp { .. },
                DeviceResult::Failed(error),
            ) => {
                self.notice = Some(format!("Nothing changed: {}.", error.describe()));
                self.view = View::Catalog;
            }
            (_, DeviceResult::Denied(reason)) => {
                self.link_request_pending = false;
                self.refreshing = false;
                self.notice = Some(denied(reason).to_owned());
                self.view = View::Catalog;
            }
            _ => {}
        }
        self.show(context);
        if refresh_after_paint {
            context.applications().refresh_catalog();
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.link_poll.on_task(context, task, &outcome) {
            if matches!(
                self.app_link,
                AppLinkState::Pairing { .. } | AppLinkState::Paired { .. }
            ) {
                self.poll_link(context);
            } else {
                self.link_poll.stop(context);
            }
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        self.link_poll.stop(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        context.store().read_link();
    }
}

fn qr_picture(context: &mut Context, value: &str) -> Option<TilePicture> {
    let qr = QrCode::encode_text(value, QrCodeEcc::Medium).ok()?;
    let side_modules = qr.size() + QR_QUIET_ZONE * 2;
    let side = u32::try_from(side_modules).ok()?.checked_mul(QR_SCALE)?;
    let mut grey = vec![255; usize::try_from(side.checked_mul(side)?).ok()?];
    for y in 0..side {
        for x in 0..side {
            let module_x = i32::try_from(x / QR_SCALE).ok()? - QR_QUIET_ZONE;
            let module_y = i32::try_from(y / QR_SCALE).ok()? - QR_QUIET_ZONE;
            if qr.get_module(module_x, module_y) {
                let offset = usize::try_from(y.checked_mul(side)?.checked_add(x)?).ok()?;
                grey[offset] = 0;
            }
        }
    }
    context.put_picture(QR_HANDLE, side, side, grey)
}

fn pairing_verification(url: &str) -> Option<String> {
    let fragment = url.split_once('#')?.1;
    let mut fingerprint = None;
    let mut secret = None;
    for entry in fragment.split('&') {
        let (name, value) = entry.split_once('=')?;
        match name {
            "k" if value.len() == 22 => fingerprint = Some(value),
            "s" if value.len() == 22 => secret = Some(value),
            _ => {}
        }
    }
    Some(format!("{}.{}", fingerprint?, secret?))
}

fn remote_install_notice(outcome: &RemoteInstallOutcome, entries: &[AppInfo]) -> Option<String> {
    let name = |id: &str| {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or_else(|| id.to_owned(), |entry| entry.title.clone())
    };
    match outcome {
        RemoteInstallOutcome::None => None,
        RemoteInstallOutcome::Installed { id } => {
            Some(format!("{} installed successfully.", name(id)))
        }
        RemoteInstallOutcome::Updated { id } => Some(format!("{} updated successfully.", name(id))),
        RemoteInstallOutcome::AlreadyInstalled { id } => {
            Some(format!("{} is already installed and up to date.", name(id)))
        }
        RemoteInstallOutcome::Included { id } => {
            Some(format!("{} is included with Cobalt.", name(id)))
        }
        RemoteInstallOutcome::Unavailable { id } => Some(format!(
            "{} is not available in the current catalog. Nothing changed.",
            name(id)
        )),
    }
}

fn denied(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::NotDeclared => "This application is not allowed to manage installed apps.",
        DenyReason::WithheldForBattery => {
            "Charge the reader before downloading or changing applications."
        }
        DenyReason::Unsupported => "This Cobalt build does not include app-store support.",
        DenyReason::Busy => "Another operation is still in progress.",
        DenyReason::PolicyRejected => "The runtime policy refused this operation.",
    }
}

fn app_state(entry: &AppInfo) -> String {
    if is_system_app(&entry.id) {
        return "Installed · system".to_owned();
    }
    match &entry.installed_version {
        None => "Available".to_owned(),
        Some(version) if entry.has_update() => format!("{version} → {}", entry.version),
        Some(version) => format!("Installed · {version}"),
    }
}

fn is_system_app(id: &str) -> bool {
    matches!(id, "settings" | "terminal")
}

fn app_action(id: &str) -> String {
    format!("app-{id}")
}

fn install_action(id: &str) -> String {
    format!("install-{id}")
}

fn remove_action(id: &str) -> String {
    format!("remove-{id}")
}

fn open_action(id: &str) -> String {
    format!("open-{id}")
}

fn main() -> ExitCode {
    match kobo_sdk::run("store", Store::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("store: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command};
    use kobo_ui::{
        Chrome, DisplayMetrics, LayoutIssueKind, LayoutKind, TextScale, CLARA_BW_METRICS,
    };
    use std::collections::BTreeSet;

    const ELIPSA_2E_METRICS: DisplayMetrics = DisplayMetrics {
        width: 1404,
        height: 1872,
        pixels_per_inch: 227,
        text_scale: TextScale::Default,
    };

    fn app(id: &str, installed: Option<&str>) -> AppInfo {
        AppInfo {
            id: id.to_owned(),
            title: format!("{id} app"),
            label: id.to_owned(),
            summary: "A useful public Cobalt application.".to_owned(),
            version: "1.1.0".to_owned(),
            glyph: Glyph::App,
            capabilities: vec!["network".to_owned()],
            installed_version: installed.map(str::to_owned),
        }
    }

    #[test]
    fn opening_store_reads_cache_then_refreshes() {
        let mut runner = AppRunner::new(Store::default());
        let commands = runner.start();
        let requests = commands
            .iter()
            .filter_map(|command| match command {
                Command::Device(request) => Some(request),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            requests,
            vec![&DeviceRequest::ReadAppCatalog, &DeviceRequest::ReadAppLink]
        );
        let commands = runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        assert!(runner.app().refreshing);
        let paint = commands
            .iter()
            .position(|command| matches!(command, Command::SetScreen(_)))
            .expect("cached catalog paints");
        let refresh = commands
            .iter()
            .position(|command| {
                matches!(command, Command::Device(DeviceRequest::RefreshAppCatalog))
            })
            .expect("refresh follows the cache");
        assert!(
            paint < refresh,
            "network refresh started before cached content painted"
        );
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        assert!(!runner.app().refreshing);
    }

    #[test]
    fn an_install_uses_only_the_app_transaction_request() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        runner.action(action_id(&app_action("notes")));
        let commands = runner.action(action_id(&install_action("notes")));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::InstallApp { id }) if id == "notes"
        )));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::Update { .. }))));
    }

    #[test]
    fn completed_transactions_use_clear_user_facing_messages() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", None)],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", None)],
        });
        runner.action(action_id(&app_action("sudoku")));
        runner.action(action_id(&install_action("sudoku")));
        runner.device_result(DeviceResult::Done);
        assert_eq!(
            runner.app().notice.as_deref(),
            Some("sudoku app installed successfully.")
        );

        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", Some("1.1.0"))],
        });
        runner.action(action_id(&app_action("sudoku")));
        runner.action(action_id(&remove_action("sudoku")));
        runner.device_result(DeviceResult::Done);
        assert_eq!(
            runner.app().notice.as_deref(),
            Some("sudoku app removed successfully.")
        );
    }

    #[test]
    fn updates_are_identified_in_the_success_message() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", Some("1.0.0"))],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", Some("1.0.0"))],
        });
        runner.action(action_id(&app_action("sudoku")));
        runner.action(action_id(&install_action("sudoku")));
        runner.device_result(DeviceResult::Done);
        assert_eq!(
            runner.app().notice.as_deref(),
            Some("sudoku app updated successfully.")
        );
    }

    #[test]
    fn a_late_refresh_does_not_hide_an_install_in_progress() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.action(action_id(&app_action("notes")));
        runner.action(action_id(&install_action("notes")));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
        assert!(matches!(runner.app().view, View::Working { .. }));
    }

    #[test]
    fn catalog_rows_and_controls_fit_the_clara_panel() {
        let mut store = Store::default();
        store.replace_entries(
            (0..12)
                .map(|index| app(&format!("app-{index}"), None))
                .collect(),
        );
        let context = AppRunner::new(Store::default()).context();
        let screen = store.catalog(&context);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::Row(..))));
        assert!(layout
            .nodes
            .iter()
            .all(|node| { node.rect.y + node.rect.height <= CLARA_BW_METRICS.height }));
    }

    #[test]
    fn catalog_uses_the_room_available_on_the_elipsa_panel() {
        let mut store = Store::default();
        store.replace_entries(
            (0..6)
                .map(|index| app(&format!("app-{index}"), None))
                .collect(),
        );
        let context = AppRunner::with_metrics(Store::default(), ELIPSA_2E_METRICS).context();
        let screen = store.catalog(&context);
        let layout = screen.layout_with(&ELIPSA_2E_METRICS, &Chrome::with_back(false));
        let rows = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Row(..)))
            .count();
        assert_eq!(rows, 6, "the sixth app was moved to another page");
    }

    #[test]
    fn measured_catalog_pages_show_every_app_without_clipping() {
        for metrics in [CLARA_BW_METRICS, ELIPSA_2E_METRICS] {
            for count in (1..=12).chain([30]) {
                let mut store = Store::default();
                store.replace_entries(
                    (0..count)
                        .map(|index| app(&format!("app-{index}"), None))
                        .collect(),
                );
                let context = AppRunner::with_metrics(Store::default(), metrics).context();
                let mut shown = BTreeSet::new();
                for requested_page in 0..store.entries.len() {
                    store.page = requested_page;
                    let screen = store.catalog(&context);
                    if store.page != requested_page {
                        break;
                    }
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::with_back(false));
                    assert!(
                        diagnostics.issues.iter().all(|issue| !matches!(
                            issue.kind,
                            LayoutIssueKind::ContentOverflow { .. } | LayoutIssueKind::Clipped
                        )),
                        "catalog of {count} apps, page {requested_page}, did not fit \
                         {metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    shown.extend(diagnostics.layout.nodes.iter().filter_map(
                        |node| match node.kind {
                            LayoutKind::Row(action) => Some(action),
                            _ => None,
                        },
                    ));
                }
                let expected = store
                    .entries
                    .iter()
                    .map(|entry| action_id(&app_action(&entry.id)))
                    .collect::<BTreeSet<_>>();
                assert_eq!(
                    shown, expected,
                    "a catalog of {count} apps lost entries on {metrics:?}"
                );
            }
        }
    }

    #[test]
    fn a_multi_page_catalog_can_be_sent_to_the_runtime() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: (0..14)
                .map(|index| app(&format!("app-{index}"), Some("1.1.0")))
                .collect(),
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        let commands = runner.device_result(DeviceResult::Apps {
            entries: (0..14)
                .map(|index| app(&format!("app-{index}"), Some("1.1.0")))
                .collect(),
        });
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::SetScreen(_))));
    }

    #[test]
    fn pairing_state_draws_a_qr_code_and_starts_polling() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", None)],
        });
        let commands = runner.device_result(DeviceResult::AppLink(AppLinkState::Pairing {
            code: "23456789".to_owned(),
            url: format!(
                "https://bandarlabs.github.io/Cobalt/pair/?code=23456789#k={}&s={}",
                "A".repeat(22),
                "B".repeat(22)
            ),
            expires_in: 600,
        }));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::PutPicture { .. })));
        assert!(runner.app().link_poll.is_running());
        let screen = runner.app().app_link();
        let display = format!("{screen:?}");
        assert!(display.contains("2345 6789"));
        assert!(display.contains(&format!("{}.{}", "A".repeat(22), "B".repeat(22))));
    }

    #[test]
    fn manual_pairing_verification_combines_the_fragment_values() {
        assert_eq!(
            pairing_verification(&format!(
                "https://example.test/pair#k={}&s={}",
                "A".repeat(22),
                "B".repeat(22)
            )),
            Some(format!("{}.{}", "A".repeat(22), "B".repeat(22)))
        );
        assert_eq!(pairing_verification("https://example.test/pair"), None);
    }

    #[test]
    fn remote_install_outcomes_use_state_specific_messages() {
        assert_eq!(
            remote_install_notice(
                &RemoteInstallOutcome::AlreadyInstalled {
                    id: "sudoku".to_owned()
                },
                &[app("sudoku", Some("1.1.0"))]
            )
            .as_deref(),
            Some("sudoku app is already installed and up to date.")
        );
        assert_eq!(
            remote_install_notice(
                &RemoteInstallOutcome::Included {
                    id: "settings".to_owned()
                },
                &[]
            )
            .as_deref(),
            Some("settings is included with Cobalt.")
        );
        assert_eq!(
            remote_install_notice(
                &RemoteInstallOutcome::Unavailable {
                    id: "removed-app".to_owned()
                },
                &[]
            )
            .as_deref(),
            Some("removed-app is not available in the current catalog. Nothing changed.")
        );
    }

    #[test]
    fn installed_apps_offer_open_and_uninstall() {
        let store = Store {
            entries: vec![app("notes", Some("1.1.0"))],
            view: View::Detail("notes".to_owned()),
            ..Store::default()
        };
        let screen = store.detail("notes");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout
            .rect_of_action(action_id(&open_action("notes")))
            .is_some());
        assert!(layout
            .rect_of_action(action_id(&remove_action("notes")))
            .is_some());
    }

    #[test]
    fn an_available_version_change_offers_an_in_place_update() {
        let mut notes = app("notes", Some("1.0.0"));
        notes.version = "1.1.0".to_owned();
        let store = Store {
            entries: vec![notes],
            view: View::Detail("notes".to_owned()),
            ..Store::default()
        };
        let screen = store.detail("notes");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout
            .rect_of_action(action_id(&install_action("notes")))
            .is_some());
        assert_eq!(app_state(&store.entries[0]), "1.0.0 → 1.1.0");
    }

    #[test]
    fn system_apps_are_marked_installed_and_cannot_be_removed() {
        let mut settings = app("settings", Some("0.2.0"));
        settings.version = "0.2.0".to_owned();
        let store = Store {
            entries: vec![settings],
            view: View::Detail("settings".to_owned()),
            ..Store::default()
        };
        assert_eq!(app_state(&store.entries[0]), "Installed · system");
        let screen = store.detail("settings");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout
            .rect_of_action(action_id(&open_action("settings")))
            .is_some());
        assert!(layout
            .rect_of_action(action_id(&remove_action("settings")))
            .is_none());
    }
}
