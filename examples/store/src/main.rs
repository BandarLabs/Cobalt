//! Cobalt's unprivileged app-store interface.
//!
//! Catalog downloads, signature checks and filesystem changes remain inside
//! `kobod`. This process receives display metadata and submits app identities;
//! it never receives a package URL or chooses an installation path.

use kobo_sdk::{
    action_id, ActionId, AppInfo, AppLinkState, BannerLevel, Capability, Context, DenyReason,
    DeviceError, DeviceRequest, DeviceResult, Glyph, Heartbeat, KoboApp, PictureHandle,
    RemoteInstallOutcome, RowLead, Screen, ScreenBuilder, TaskId, TaskOutcome, TilePicture,
};
use qrcodegen::{QrCode, QrCodeEcc};
use std::process::ExitCode;

const REFRESH: &str = "refresh";
const APP_LINK: &str = "app-link";
const BEGIN_LINK: &str = "begin-link";
const DISCONNECT_LINK: &str = "disconnect-link";
const PREVIOUS: &str = "previous";
const NEXT: &str = "next";
const UPDATE_COBALT: &str = "update-cobalt";
const CONFIRM_REMOVE: &str = "confirm-remove";
const CANCEL_REMOVE: &str = "cancel-remove";
const QR_HANDLE: PictureHandle = PictureHandle(1);
const QR_SCALE: u32 = 7;
const QR_QUIET_ZONE: i32 = 4;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Catalog,
    Detail(String),
    /// The detail screen with the uninstall question over it.
    ConfirmUninstall(String),
    Working {
        id: String,
        action: &'static str,
    },
    AppLink,
}

/// Which apps the catalog lists.
///
/// Forty-odd apps over ten pages is a list nobody reads to the end, and the
/// questions an owner brings to the Store are "what is new for the apps I
/// have", "what do I have" and "what else is there". Each is answered from
/// the installed and available versions the catalog already carries, so the
/// filter needs nothing from the runtime that it does not already send.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Filter {
    #[default]
    All,
    Installed,
    Updates,
    Available,
}

impl Filter {
    const ALL: [Self; 4] = [Self::All, Self::Installed, Self::Updates, Self::Available];

    const fn action(self) -> &'static str {
        match self {
            Self::All => "filter-all",
            Self::Installed => "filter-installed",
            Self::Updates => "filter-updates",
            Self::Available => "filter-available",
        }
    }

    fn admits(self, entry: &AppInfo) -> bool {
        match self {
            Self::All => true,
            Self::Installed => entry.is_installed() || is_system_app(&entry.id),
            Self::Updates => has_compatible_update(entry),
            Self::Available => !entry.is_installed() && !is_system_app(&entry.id),
        }
    }
}

/// A sentence for the top of whichever screen is showing.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    level: BannerLevel,
    text: String,
}

impl Notice {
    fn info(text: impl Into<String>) -> Self {
        Self {
            level: BannerLevel::Info,
            text: text.into(),
        }
    }

    fn attention(text: impl Into<String>) -> Self {
        Self {
            level: BannerLevel::Attention,
            text: text.into(),
        }
    }
}

struct Store {
    entries: Vec<AppInfo>,
    view: View,
    filter: Filter,
    page: usize,
    refreshing: bool,
    refresh_after_cache: bool,
    notice: Option<Notice>,
    /// The app whose last install or update failed, so its detail screen
    /// offers to try again rather than repeating the word that just failed.
    failed: Option<String>,
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
            filter: Filter::All,
            page: 0,
            refreshing: false,
            refresh_after_cache: false,
            notice: None,
            failed: None,
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
            View::ConfirmUninstall(id) => self.confirm_uninstall(&id),
            View::Working { id, action } => self.working(&id, action),
            View::AppLink => self.app_link(),
        };
        context.set_screen(screen);
    }

    /// The apps the current filter lists, in the order an owner wants them.
    ///
    /// Updates come first because they are the one thing on the page that
    /// asks for a tap, then what is already installed, then everything else.
    /// Built-in apps go last: nothing about them can be changed here, so the
    /// top of the first page is no place for them.
    fn listed(&self) -> Vec<&AppInfo> {
        let mut listed = self
            .entries
            .iter()
            .filter(|entry| self.filter.admits(entry))
            .collect::<Vec<_>>();
        listed.sort_by_key(|entry| standing(entry));
        listed
    }

    /// Everything on the catalog screen above the list itself.
    ///
    /// Drawn and measured from the same builder, so the list is paginated
    /// against exactly the room the screen leaves it.
    fn catalog_head(&self, page: usize, pages: usize) -> ScreenBuilder {
        let count = |filter: Filter| {
            self.entries
                .iter()
                .filter(|entry| filter.admits(entry))
                .count()
        };
        let mut screen = ScreenBuilder::new("store-catalog")
            .top_bar("App Store")
            .top_bar_glyph(REFRESH, "Refresh", Glyph::Refresh)
            .top_bar_action(APP_LINK, "Link browser");
        if let Some(notice) = &self.notice {
            screen = screen.banner(notice.level, notice.text.clone());
        }
        let selected = Filter::ALL
            .iter()
            .position(|filter| *filter == self.filter)
            .unwrap_or(0);
        screen = screen.tabs(
            selected,
            [
                (Filter::All.action(), "All".to_owned()),
                (
                    Filter::Installed.action(),
                    format!("Installed {}", count(Filter::Installed)),
                ),
                (
                    Filter::Updates.action(),
                    format!("Updates {}", count(Filter::Updates)),
                ),
                (
                    Filter::Available.action(),
                    format!("Available {}", count(Filter::Available)),
                ),
            ],
        );
        let title = match (self.filter, self.refreshing) {
            (Filter::All, false) => "Apps",
            (Filter::All, true) => "Apps · refreshing",
            (Filter::Installed, _) => "Installed",
            (Filter::Updates, _) => "Updates",
            (Filter::Available, _) => "Available to install",
        };
        screen.section_with_value(title, format!("{} / {}", page + 1, pages.max(1)))
    }

    /// Breaks the listed apps into pages under the catalog's own head.
    ///
    /// The SDK measures rows with a trailing value below a section, and rows
    /// under an arbitrary head, but not both at once, and the filter strip is
    /// what makes this screen need both. So the room is worked out here the
    /// way the SDK works it out, from the head as the layout engine actually
    /// draws it. The catalog tests lay every page out at every text size, so
    /// if the two ever disagree a row under the bottom bar fails a test
    /// rather than reaching a reader.
    fn pages(
        &self,
        context: &Context,
        rows: &[(&str, &str, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<usize>> {
        let metrics = context.metrics();
        let head = self.catalog_head(0, 1).build();
        let pages = kobo_ui::with_text_scale(metrics.text_scale, || {
            let used = head
                .layout_with(&metrics, &kobo_ui::Chrome::measuring(true))
                .content_used();
            let mut area = metrics.prose_area_in(true, nav_bar, kobo_ui::Face::Text);
            area.height = area
                .height
                .saturating_sub(metrics.status_band_height())
                .saturating_sub(used.saturating_add(area.gap))
                .max(1);
            kobo_ui::paginate_rows_with_trailing(rows, &metrics, area)
        });
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn catalog(&mut self, context: &Context) -> Screen {
        if self.entries.is_empty() {
            let mut screen = ScreenBuilder::new("store-catalog")
                .top_bar("App Store")
                .top_bar_glyph(REFRESH, "Refresh", Glyph::Refresh)
                .top_bar_action(APP_LINK, "Link browser");
            if let Some(notice) = &self.notice {
                screen = screen.banner(notice.level, notice.text.clone());
            }
            return screen
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
                .bottom_action_marked(REFRESH, "Refresh", Glyph::Refresh)
                .build();
        }
        let listed = self.listed();
        let states = listed
            .iter()
            .map(|entry| app_state(entry))
            .collect::<Vec<_>>();
        let rows = listed
            .iter()
            .zip(&states)
            .map(|(entry, state)| (entry.title.as_str(), entry.summary.as_str(), state.as_str()))
            .collect::<Vec<_>>();
        // A one-page catalog draws no bottom bar and gets that room for apps.
        // Once it turns, measure again with the navigation bar it will draw.
        let without_controls = self.pages(context, &rows, false);
        let page_indices = if without_controls.len() > 1 {
            self.pages(context, &rows, true)
        } else {
            without_controls
        };
        let pages = page_indices.len();
        let page = self.page.min(pages - 1);
        let mut screen = self.catalog_head(page, pages);
        if listed.is_empty() {
            screen = screen.text(match self.filter {
                Filter::All => "No apps are in the catalog.",
                Filter::Installed => {
                    "No apps are installed yet. Choose All to see what you can install."
                }
                Filter::Updates => "Every installed app is up to date.",
                Filter::Available => "Every app in the catalog is installed.",
            });
        } else {
            screen = screen.rows_with_trailing(
                page_indices[page]
                    .iter()
                    .filter_map(|index| listed.get(*index))
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
        }
        self.page = page;
        if pages > 1 {
            let mut actions = Vec::new();
            if page > 0 {
                actions.push((PREVIOUS, "Previous", Some(Glyph::Previous)));
            }
            if page + 1 < pages {
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
            .top_bar("Install from a browser")
            .owns_back(true);
        if let Some(notice) = &self.notice {
            screen = screen.banner(notice.level, notice.text.clone());
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
                self.notice = Some(Notice::attention(format!(
                    "Install links are unavailable: {}. Connect Wi-Fi and try again.",
                    error.describe()
                )));
            }
            _ => return false,
        }
        true
    }

    /// One app, with what it can do and the one or two things to do with it.
    ///
    /// The permissions are said in words because this is the screen where an
    /// owner decides whether to let an app onto the reader, and
    /// `keep-awake, scheduled-wake` answers a question only a developer would
    /// ask.
    fn detail(&self, id: &str) -> Screen {
        self.detail_screen(id).build()
    }

    fn detail_screen(&self, id: &str) -> ScreenBuilder {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return ScreenBuilder::new("store-missing")
                .top_bar("App Store")
                .owns_back(true)
                .error_state("This app is no longer in the verified catalog.");
        };
        let installed = entry.installed_version.as_deref();
        let system = is_system_app(id);
        let compatible = entry.is_compatible_with(env!("CARGO_PKG_VERSION"));
        let mut screen = ScreenBuilder::new("store-detail")
            .top_bar(entry.title.clone())
            .owns_back(true);
        if let Some(notice) = &self.notice {
            screen = screen.banner(notice.level, notice.text.clone());
        }
        screen = screen
            .splash(
                Some(entry.glyph),
                entry.title.clone(),
                entry.summary.clone(),
            )
            .facts([
                ("Available", entry.version.clone()),
                ("Installed", installed.unwrap_or("Not installed").to_owned()),
                ("Requires Cobalt", entry.minimum_cobalt_version.clone()),
                (
                    "Management",
                    if system {
                        "Built into Cobalt".to_owned()
                    } else {
                        "Managed by App Store".to_owned()
                    },
                ),
                ("Permissions", permissions(&entry.capabilities)),
            ]);
        let retry = self.failed.as_deref() == Some(id);
        if system {
            screen.bottom_action_marked(open_action(id), "Open", entry.glyph)
        } else if !compatible {
            if installed.is_some() {
                screen.action_bar_marked(vec![
                    (
                        UPDATE_COBALT.to_owned(),
                        "Update Cobalt",
                        Some(Glyph::Refresh),
                    ),
                    (open_action(id), "Open", Some(entry.glyph)),
                    (remove_action(id), "Uninstall", Some(Glyph::Trash)),
                ])
            } else {
                screen.bottom_action_marked(UPDATE_COBALT, "Update Cobalt", Glyph::Refresh)
            }
        } else if installed.is_some() {
            let mut actions = vec![
                (open_action(id), "Open", Some(entry.glyph)),
                (remove_action(id), "Uninstall", Some(Glyph::Trash)),
            ];
            if entry.has_update() {
                actions.insert(
                    0,
                    (
                        install_action(id),
                        if retry { "Try again" } else { "Update" },
                        Some(Glyph::Download),
                    ),
                );
            }
            screen.action_bar_marked(actions)
        } else {
            screen.bottom_action_marked(
                install_action(id),
                if retry { "Try again" } else { "Install" },
                if retry {
                    Glyph::Refresh
                } else {
                    Glyph::Download
                },
            )
        }
    }

    /// Asks before an uninstall, over the screen the owner tapped it on.
    ///
    /// Uninstalling is one tap away from Open on the same bar, and an app
    /// that took a USB trip and a key to set up should not be one brush of
    /// the panel from gone. The question also answers the thing an owner
    /// worries about: the app's saved data stays, because `kobod` keeps an
    /// app's state outside the package it removes.
    fn confirm_uninstall(&self, id: &str) -> Screen {
        let title = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or(id, |entry| entry.title.as_str());
        self.detail_screen(id)
            .confirm(
                format!("Uninstall {title}?"),
                "Its saved data stays on this reader, so installing it again picks up where you left off.",
                (CONFIRM_REMOVE, "Uninstall"),
                (CANCEL_REMOVE, "Cancel"),
            )
            .build()
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
                "Keep Cobalt open until this finishes.",
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

    fn title_of(&self, id: &str) -> String {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or_else(|| id.to_owned(), |entry| entry.title.clone())
    }

    fn request_install(&mut self, context: &mut Context, id: String) {
        let action = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .filter(|entry| entry.is_installed())
            .map_or("Installing", |_| "Updating");
        self.notice = None;
        self.failed = None;
        self.view = View::Working {
            id: id.clone(),
            action,
        };
        self.show(context);
        if !context.applications().install(id.clone()) {
            self.notice = Some(Notice::attention("That application identity is invalid."));
            self.view = View::Detail(id);
            self.show(context);
        }
    }

    fn request_uninstall(&mut self, context: &mut Context, id: String) {
        self.notice = None;
        self.failed = None;
        self.view = View::Working {
            id: id.clone(),
            action: "Removing",
        };
        self.show(context);
        if !context.applications().uninstall(id.clone()) {
            self.notice = Some(Notice::attention("That application identity is invalid."));
            self.view = View::Detail(id);
            self.show(context);
        }
    }

    /// Where a finished or refused change leaves the owner: on the app it was
    /// about, unless nothing was in progress.
    fn after_working(&self) -> View {
        match &self.view {
            View::Working { id, .. } => View::Detail(id.clone()),
            other => other.clone(),
        }
    }

    /// What an install that just finished changed, written into the list
    /// before the runtime's own reply arrives.
    ///
    /// The detail screen is drawn from the entry, so without this it would
    /// offer Install again for the second it takes the catalog to come back.
    fn mark_installed(&mut self, id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.installed_version = Some(entry.version.clone());
        }
    }

    fn take_filter(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(filter) = Filter::ALL
            .into_iter()
            .find(|filter| action == action_id(filter.action()))
        else {
            return false;
        };
        if filter != self.filter {
            self.filter = filter;
            self.page = 0;
            self.notice = None;
        }
        self.show(context);
        true
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
        if action == ActionId::BACK || action == action_id(CANCEL_REMOVE) {
            if let View::ConfirmUninstall(id) = &self.view {
                self.view = View::Detail(id.clone());
            } else if action == ActionId::BACK {
                self.notice = None;
                self.failed = None;
                self.view = View::Catalog;
            } else {
                return;
            }
            self.show(context);
            return;
        }
        if action == action_id(CONFIRM_REMOVE) {
            if let View::ConfirmUninstall(id) = self.view.clone() {
                self.request_uninstall(context, id);
            }
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
        if action == action_id(UPDATE_COBALT) {
            context.launch("settings");
            return;
        }
        if self.take_filter(context, action) {
            return;
        }
        if action == action_id(PREVIOUS) || action == action_id(NEXT) {
            // The banner was read on the page it arrived on. Carrying it onto
            // every later page spends a row of apps on old news.
            self.notice = None;
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
            self.notice = None;
            self.failed = None;
            self.show(context);
            return;
        }
        if let Some(entry) = self.entries.iter().find(|entry| {
            action == action_id(&install_action(&entry.id))
                || action == action_id(&open_action(&entry.id))
                || action == action_id(&remove_action(&entry.id))
        }) {
            let id = entry.id.clone();
            let compatible = entry.is_compatible_with(env!("CARGO_PKG_VERSION"));
            if action == action_id(&open_action(&id)) {
                context.launch(id);
            } else if action == action_id(&remove_action(&id)) {
                self.view = View::ConfirmUninstall(id);
                self.show(context);
            } else if !compatible {
                context.launch("settings");
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
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Apps { entries }) => {
                self.replace_entries(entries);
                self.refreshing = false;
                if self.view == View::Catalog {
                    self.notice = None;
                }
            }
            (DeviceRequest::InstallApp { id }, DeviceResult::Done) => {
                let outcome = match &self.view {
                    View::Working {
                        id: working_id,
                        action: "Updating",
                    } if working_id == &id => "updated",
                    _ => "installed",
                };
                self.notice = Some(Notice::info(format!(
                    "{} is {outcome}.",
                    self.title_of(&id)
                )));
                self.mark_installed(&id);
                self.view = View::Detail(id);
                context.applications().cached_catalog();
            }
            (DeviceRequest::UninstallApp { id }, DeviceResult::Done) => {
                self.notice = Some(Notice::info(format!(
                    "{} is uninstalled. Its saved data is still on this reader.",
                    self.title_of(&id)
                )));
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                    entry.installed_version = None;
                }
                self.view = View::Catalog;
                context.applications().cached_catalog();
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Failed(error)) => {
                self.refreshing = false;
                self.notice = Some(Notice::attention(format!(
                    "The catalog could not be refreshed: {}. The last verified list is still shown.",
                    error.describe()
                )));
            }
            (DeviceRequest::ReadAppCatalog, DeviceResult::Failed(_)) => {
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (DeviceRequest::InstallApp { id }, DeviceResult::Failed(error)) => {
                self.notice = Some(Notice::attention(app_failure(error)));
                self.failed = Some(id.clone());
                self.view = View::Detail(id);
                // A write may have reached disk before its final flush failed.
                // Re-read verified installed metadata instead of promising rollback.
                context.applications().cached_catalog();
            }
            (DeviceRequest::UninstallApp { id }, DeviceResult::Failed(error)) => {
                self.notice = Some(Notice::attention(app_failure(error)));
                self.view = View::Detail(id);
                context.applications().cached_catalog();
            }
            (_, DeviceResult::Denied(reason)) => {
                self.link_request_pending = false;
                self.refreshing = false;
                self.notice = Some(Notice::attention(denied(reason)));
                self.view = self.after_working();
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

fn remote_install_notice(outcome: &RemoteInstallOutcome, entries: &[AppInfo]) -> Option<Notice> {
    let name = |id: &str| {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or_else(|| id.to_owned(), |entry| entry.title.clone())
    };
    match outcome {
        RemoteInstallOutcome::None => None,
        RemoteInstallOutcome::Installed { id } => {
            Some(Notice::info(format!("{} is installed.", name(id))))
        }
        RemoteInstallOutcome::Updated { id } => {
            Some(Notice::info(format!("{} is updated.", name(id))))
        }
        RemoteInstallOutcome::AlreadyInstalled { id } => Some(Notice::info(format!(
            "{} is already installed and up to date.",
            name(id)
        ))),
        RemoteInstallOutcome::Included { id } => Some(Notice::info(format!(
            "{} is included with Cobalt.",
            name(id)
        ))),
        RemoteInstallOutcome::Unavailable { id } => Some(Notice::attention(format!(
            "{} is not available in the current catalog. Nothing changed.",
            name(id)
        ))),
        RemoteInstallOutcome::RequiresCobalt {
            id,
            minimum_cobalt_version,
        } => Some(Notice::attention(format!(
            "{} requires Cobalt {}. Update Cobalt, then try again.",
            name(id),
            minimum_cobalt_version
        ))),
    }
}

/// An app's permissions as sentences an owner can weigh.
///
/// A name this build of the Store does not recognise belongs to a newer
/// Cobalt; it is shown rather than dropped, because a permission that
/// silently vanished from this screen is worse than one shown by its name.
fn permissions(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        return "None".to_owned();
    }
    capabilities
        .iter()
        .map(|name| {
            Capability::parse(name).map_or_else(
                || format!("Another permission ({name})."),
                |capability| format!("{}.", capability.plain_wording()),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn denied(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::NotDeclared => "App management isn't available.",
        DenyReason::WithheldForBattery => {
            "Charge the reader before downloading or changing applications."
        }
        DenyReason::Unsupported => "This version of Cobalt cannot install apps.",
        DenyReason::Busy => {
            "Another install or update is still running. Try again when it finishes."
        }
        DenyReason::PolicyRejected => "Cobalt does not allow this change.",
    }
}

fn app_failure(error: DeviceError) -> &'static str {
    match error {
        DeviceError::NotFound => "This app is no longer available. Refresh the app list.",
        DeviceError::Authentication => "The download was refused. Refresh the app list and try again.",
        DeviceError::TimedOut => "The download took too long. Check Wi-Fi and try again.",
        DeviceError::Unreachable => "Couldn't reach the download. Check Wi-Fi and try again.",
        DeviceError::InvalidInput => "This app needs a compatible Cobalt version. Refresh the app list and check for a Cobalt update.",
        DeviceError::Backend => "Couldn't finish saving the change. Check free space and the installed version before trying again.",
        DeviceError::Integrity => "The downloaded app could not be verified. Refresh the app list and try again.",
    }
}

fn app_state(entry: &AppInfo) -> String {
    if is_system_app(&entry.id) {
        return "Installed · system".to_owned();
    }
    if !entry.is_compatible_with(env!("CARGO_PKG_VERSION")) {
        return format!("Requires Cobalt {}", entry.minimum_cobalt_version);
    }
    match &entry.installed_version {
        None => "Available".to_owned(),
        Some(version) if entry.has_update() => format!("{version} → {}", entry.version),
        Some(version) => format!("Installed · {version}"),
    }
}

/// An update this reader can actually take now.
///
/// One that needs a newer Cobalt is not offered as an update, because the
/// tap would lead to Settings instead, and an Updates list that cannot be
/// cleared from the Store is a list of nagging.
fn has_compatible_update(entry: &AppInfo) -> bool {
    !is_system_app(&entry.id)
        && entry.has_update()
        && entry.is_compatible_with(env!("CARGO_PKG_VERSION"))
}

/// Where an app sorts in the catalog: updates, installed, the rest, built in.
fn standing(entry: &AppInfo) -> u8 {
    if is_system_app(&entry.id) {
        3
    } else if has_compatible_update(entry) {
        0
    } else if entry.is_installed() {
        1
    } else {
        2
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

    fn notice(runner: &AppRunner<Store>) -> Option<&str> {
        runner
            .app()
            .notice
            .as_ref()
            .map(|notice| notice.text.as_str())
    }

    fn words(screen: &Screen, metrics: &DisplayMetrics) -> String {
        screen
            .layout_with(metrics, &Chrome::with_back(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A store that has already painted its cached catalog and link state.
    fn opened(entries: &[AppInfo]) -> AppRunner<Store> {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: entries.to_vec(),
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: entries.to_vec(),
        });
        runner
    }

    fn painted(commands: &[Command]) -> Screen {
        commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a screen was painted")
    }

    fn app(id: &str, installed: Option<&str>) -> AppInfo {
        AppInfo {
            id: id.to_owned(),
            title: format!("{id} app"),
            label: id.to_owned(),
            summary: "A useful public Cobalt application.".to_owned(),
            version: "1.1.0".to_owned(),
            minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
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
    fn an_incompatible_app_opens_the_cobalt_updater_instead_of_installing() {
        let mut incompatible = app("notes", None);
        incompatible.minimum_cobalt_version = "9.0.0".to_owned();
        assert_eq!(app_state(&incompatible), "Requires Cobalt 9.0.0");

        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![incompatible.clone()],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![incompatible],
        });
        runner.action(action_id(&app_action("notes")));
        let commands = runner.action(action_id(&install_action("notes")));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Launch(id) if id == "settings")));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::InstallApp { .. }))));
    }

    #[test]
    fn an_incompatible_installed_app_can_still_open_or_be_removed() {
        let mut incompatible = app("notes", Some("1.0.0"));
        incompatible.minimum_cobalt_version = "9.0.0".to_owned();

        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![incompatible.clone()],
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: vec![incompatible],
        });
        runner.action(action_id(&app_action("notes")));
        let open = runner.action(action_id(&open_action("notes")));
        assert!(open
            .iter()
            .any(|command| matches!(command, Command::Launch(id) if id == "notes")));

        runner.action(action_id(&remove_action("notes")));
        let remove = runner.action(action_id(CONFIRM_REMOVE));
        assert!(remove.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::UninstallApp { id }) if id == "notes"
        )));
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
        assert_eq!(notice(&runner), Some("sudoku app is installed."));

        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", Some("1.1.0"))],
        });
        runner.action(action_id(&app_action("sudoku")));
        runner.action(action_id(&remove_action("sudoku")));
        runner.action(action_id(CONFIRM_REMOVE));
        runner.device_result(DeviceResult::Done);
        assert_eq!(
            notice(&runner),
            Some("sudoku app is uninstalled. Its saved data is still on this reader.")
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
        assert_eq!(notice(&runner), Some("sudoku app is updated."));
    }

    #[test]
    fn uncertain_write_refreshes_verified_state_without_claiming_rollback() {
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
        let commands = runner.device_result(DeviceResult::Failed(DeviceError::Backend));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ReadAppCatalog))));
        let notice = runner.app().notice.clone();
        assert!(notice.as_ref().unwrap().text.contains("installed version"));
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("sudoku", Some("1.1.0"))],
        });
        assert_eq!(
            runner.app().entries[0].installed_version.as_deref(),
            Some("1.1.0")
        );
        assert_eq!(runner.app().notice, notice);
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
    fn actual_catalog_and_result_banners_fit_at_each_interface_size() {
        let entries = kobo_catalog::bundled()
            .unwrap()
            .into_iter()
            .map(|entry| AppInfo {
                id: entry.id,
                title: entry.title,
                label: entry.label,
                summary: entry.summary,
                version: entry.version,
                minimum_cobalt_version: env!("CARGO_PKG_VERSION").into(),
                glyph: Glyph::App,
                capabilities: entry.capabilities,
                installed_version: None,
            })
            .collect::<Vec<_>>();
        for panel in [CLARA_BW_METRICS, ELIPSA_2E_METRICS] {
            for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
                let metrics = DisplayMetrics {
                    text_scale: scale,
                    ..panel
                };
                for notice in [
                    None,
                    Some(app_failure(DeviceError::Backend)),
                    Some("Quality fixture installed."),
                ] {
                    let context = AppRunner::with_metrics(Store::default(), metrics).context();
                    let mut store = Store::default();
                    store.replace_entries(entries.clone());
                    store.notice = notice.map(Notice::attention);
                    let mut shown = BTreeSet::new();
                    for page in 0..entries.len() {
                        store.page = page;
                        let screen = store.catalog(&context);
                        let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(!report.has_errors(), "{metrics:?}: {:?}", report.issues);
                        for node in report.layout.nodes {
                            if let LayoutKind::Row(action, ..) = node.kind {
                                shown.insert(action);
                            }
                        }
                        if store.page < page {
                            break;
                        }
                    }
                    assert_eq!(shown.len(), entries.len());
                }
            }
        }
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
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(false));
                    assert!(
                        diagnostics.issues.iter().all(|issue| !matches!(
                            issue.kind,
                            LayoutIssueKind::ContentOverflow { .. } | LayoutIssueKind::Clipped
                        )),
                        "catalog of {count} apps, page {requested_page}, did not fit \
                         {metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    if screen.nav_bar.is_some() || screen.bottom_action.is_some() {
                        let content_bottom = metrics.height - metrics.nav_bar_height();
                        assert!(
                            diagnostics.layout.nodes.iter().all(|node| {
                                !matches!(node.kind, LayoutKind::Row(..))
                                    || node.rect.y + node.rect.height <= content_bottom
                            }),
                            "catalog of {count} apps, page {requested_page}, put a row under \
                             the bottom controls on {metrics:?}"
                        );
                    }
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
            .map(|notice| notice.text)
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
            .map(|notice| notice.text)
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
            .map(|notice| notice.text)
            .as_deref(),
            Some("removed-app is not available in the current catalog. Nothing changed.")
        );
        assert_eq!(
            remote_install_notice(
                &RemoteInstallOutcome::RequiresCobalt {
                    id: "sudoku".to_owned(),
                    minimum_cobalt_version: "0.4.0".to_owned(),
                },
                &[app("sudoku", None)]
            )
            .map(|notice| notice.text)
            .as_deref(),
            Some("sudoku app requires Cobalt 0.4.0. Update Cobalt, then try again.")
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

    #[test]
    fn an_install_stays_on_the_app_and_offers_to_open_it() {
        let mut runner = opened(&[app("birds", None), app("notes", None)]);
        runner.action(action_id(&app_action("birds")));
        runner.action(action_id(&install_action("birds")));
        let commands = runner.device_result(DeviceResult::Done);
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));
        let screen = painted(&commands);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(
            layout
                .rect_of_action(action_id(&open_action("birds")))
                .is_some(),
            "the app that was just installed cannot be opened from its own page"
        );
        assert!(
            layout
                .rect_of_action(action_id(&install_action("birds")))
                .is_none(),
            "the page still offered to install what had just been installed"
        );
        assert!(words(&screen, &CLARA_BW_METRICS).contains("birds app is installed."));
        // The runtime's own answer arriving afterwards must not throw the
        // owner back to the catalog.
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("birds", Some("1.1.0")), app("notes", None)],
        });
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));
    }

    #[test]
    fn uninstalling_asks_first_and_cancel_keeps_the_app() {
        let mut runner = opened(&[app("birds", Some("1.1.0"))]);
        runner.action(action_id(&app_action("birds")));
        let commands = runner.action(action_id(&remove_action("birds")));
        assert!(
            !commands.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::UninstallApp { .. })
            )),
            "one tap on Uninstall removed the app without asking"
        );
        let question = words(&painted(&commands), &CLARA_BW_METRICS);
        assert!(question.contains("Uninstall birds app?"), "{question}");
        assert!(question.contains("saved data stays"), "{question}");

        let commands = runner.action(action_id(CANCEL_REMOVE));
        assert!(commands
            .iter()
            .all(|command| !matches!(command, Command::Device(_))));
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));

        runner.action(action_id(&remove_action("birds")));
        let commands = runner.action(action_id(CONFIRM_REMOVE));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::UninstallApp { id }) if id == "birds"
        )));
    }

    #[test]
    fn a_failed_install_stays_on_the_app_and_offers_to_try_again() {
        let mut runner = opened(&[app("birds", None)]);
        runner.action(action_id(&app_action("birds")));
        runner.action(action_id(&install_action("birds")));
        let commands = runner.device_result(DeviceResult::Failed(DeviceError::Unreachable));
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));
        let screen = painted(&commands);
        let text = words(&screen, &CLARA_BW_METRICS);
        assert!(text.contains("Check Wi-Fi and try again."), "{text}");
        assert!(text.contains("Try again"), "{text}");
        let retry = runner.action(action_id(&install_action("birds")));
        assert!(retry.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::InstallApp { id }) if id == "birds"
        )));
    }

    #[test]
    fn permissions_are_described_in_words_not_identifiers() {
        let mut birds = app("birds", None);
        birds.capabilities = vec!["keep-awake".to_owned(), "scheduled-wake".to_owned()];
        let store = Store {
            entries: vec![birds],
            ..Store::default()
        };
        let text = words(&store.detail("birds"), &CLARA_BW_METRICS);
        assert!(text.contains("Can keep the reader awake"), "{text}");
        assert!(!text.contains("keep-awake"), "{text}");
        assert!(!text.contains("scheduled-wake"), "{text}");
        assert_eq!(
            permissions(&["a-future-permission".to_owned()]),
            "Another permission (a-future-permission)."
        );
    }

    #[test]
    fn updates_and_installed_apps_come_first_on_the_first_page() {
        let mut stale = app("zebra", Some("1.0.0"));
        stale.version = "1.1.0".to_owned();
        let entries = vec![
            app("apple", None),
            app("banana", None),
            app("settings", Some("0.3.0")),
            app("yak", Some("1.1.0")),
            stale,
        ];
        let mut store = Store::default();
        store.replace_entries(entries);
        let order = store
            .listed()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, ["zebra", "yak", "apple", "banana", "settings"]);
    }

    #[test]
    fn each_filter_lists_only_the_apps_it_names() {
        let mut stale = app("zebra", Some("1.0.0"));
        stale.version = "1.1.0".to_owned();
        let mut runner = opened(&[app("apple", None), app("yak", Some("1.1.0")), stale]);
        let shown = |screen: &Screen| -> Vec<&str> {
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
            let mut rows = ["zebra", "yak", "apple"]
                .into_iter()
                .filter_map(|id| {
                    layout
                        .rect_of_action(action_id(&app_action(id)))
                        .map(|rect| (rect.y, id))
                })
                .collect::<Vec<_>>();
            rows.sort_unstable();
            rows.into_iter().map(|(_, id)| id).collect()
        };
        let installed = painted(&runner.action(action_id(Filter::Installed.action())));
        assert_eq!(shown(&installed), ["zebra", "yak"]);
        let updates = painted(&runner.action(action_id(Filter::Updates.action())));
        assert_eq!(shown(&updates), ["zebra"]);
        let available = painted(&runner.action(action_id(Filter::Available.action())));
        assert_eq!(shown(&available), ["apple"]);
        let all = painted(&runner.action(action_id(Filter::All.action())));
        assert_eq!(shown(&all), ["zebra", "yak", "apple"]);
    }

    #[test]
    fn an_empty_filter_says_so_rather_than_drawing_a_blank_page() {
        let mut runner = opened(&[app("apple", None)]);
        let screen = painted(&runner.action(action_id(Filter::Updates.action())));
        assert!(words(&screen, &CLARA_BW_METRICS).contains("Every installed app is up to date."));
    }

    #[test]
    fn the_browser_link_is_labelled_in_words() {
        let mut runner = opened(&[app("apple", None)]);
        let screen = painted(&runner.action(action_id(Filter::Installed.action())));
        let bar = screen.top_bar.as_ref().expect("the catalog has a top bar");
        let link = bar
            .actions
            .iter()
            .find(|action| action.action == action_id(APP_LINK))
            .expect("the catalog offers browser installs");
        assert!(
            link.glyph.is_none(),
            "the browser link is an unlabelled icon"
        );
        assert!(words(&screen, &CLARA_BW_METRICS).contains("Link browser"));
    }

    #[test]
    fn every_bundled_app_page_fits_at_each_interface_size_with_a_banner() {
        let entries = kobo_catalog::bundled()
            .unwrap()
            .into_iter()
            .map(|entry| AppInfo {
                id: entry.id,
                title: entry.title,
                label: entry.label,
                summary: entry.summary,
                version: entry.version,
                minimum_cobalt_version: env!("CARGO_PKG_VERSION").into(),
                glyph: Glyph::App,
                capabilities: entry.capabilities,
                installed_version: Some("0.0.1".to_owned()),
            })
            .collect::<Vec<_>>();
        for panel in [CLARA_BW_METRICS, ELIPSA_2E_METRICS] {
            for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
                let metrics = DisplayMetrics {
                    text_scale: scale,
                    ..panel
                };
                for entry in &entries {
                    let mut store = Store::default();
                    store.replace_entries(entries.clone());
                    store.notice = Some(Notice::attention(app_failure(DeviceError::Backend)));
                    store.failed = Some(entry.id.clone());
                    for screen in [store.detail(&entry.id), store.confirm_uninstall(&entry.id)] {
                        let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(
                            !report.has_errors(),
                            "{} on {metrics:?}: {:?}",
                            entry.id,
                            report.issues
                        );
                    }
                }
            }
        }
    }
}
