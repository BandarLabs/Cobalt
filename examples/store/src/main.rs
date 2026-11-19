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
const DETAIL_PREVIOUS: &str = "detail-previous";
const DETAIL_NEXT: &str = "detail-next";
const CONFIRM_REMOVE: &str = "confirm-remove";
const CANCEL_REMOVE: &str = "cancel-remove";
/// One name for linking a browser, on the list's bar, its screen and its
/// button.
const LINK_LABEL: &str = "Link a browser";
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
        /// Started from a row's own button in the catalog, so the owner is
        /// put back on the list rather than on the app's page.
        from_catalog: bool,
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
    /// The app whose install or update just failed. Its banner carries Try
    /// again, and the app's page does not offer the same install a second
    /// time in its bar.
    retry: Option<String>,
}

impl Notice {
    fn new(level: BannerLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            retry: None,
        }
    }

    fn info(text: impl Into<String>) -> Self {
        Self::new(BannerLevel::Info, text)
    }

    fn attention(text: impl Into<String>) -> Self {
        Self::new(BannerLevel::Attention, text)
    }

    /// Something the owner asked for that is now done.
    fn success(text: impl Into<String>) -> Self {
        Self::new(BannerLevel::Success, text)
    }

    /// Something the owner asked for that did not happen.
    fn error(text: impl Into<String>) -> Self {
        Self::new(BannerLevel::Error, text)
    }

    /// An install or update of `id` that failed, with the way to try it
    /// again on the banner itself.
    fn retry(text: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            retry: Some(id.into()),
            ..Self::error(text)
        }
    }

    /// Draws this notice at the top of `screen`.
    fn draw(&self, screen: ScreenBuilder) -> ScreenBuilder {
        self.draw_with_action(screen, true)
    }

    /// Draws this notice, leaving Try again off the banner when the caller
    /// puts it somewhere else.
    fn draw_with_action(&self, screen: ScreenBuilder, action: bool) -> ScreenBuilder {
        let screen = screen.banner(self.level, self.text.clone());
        match &self.retry {
            Some(id) if action => screen.banner_action(install_action(id), "Try again"),
            _ => screen,
        }
    }
}

/// What a refresh that failed says, in the owner's words and with the one
/// thing to do next.
///
/// It used to append the runtime's own description, so an owner with Wi-Fi off
/// read "the device or network is unreachable" and then that the last
/// verified list was still shown, which is two sentences about the machinery
/// and none about what to do.
fn refresh_failure(error: DeviceError) -> String {
    match error {
        DeviceError::Unreachable | DeviceError::NotFound | DeviceError::TimedOut => {
            "The list could not be updated, so it may be out of date. Connect to Wi-Fi and refresh."
                .to_owned()
        }
        _ => "The list could not be updated, so it may be out of date. Refresh again later."
            .to_owned(),
    }
}

struct Store {
    entries: Vec<AppInfo>,
    view: View,
    filter: Filter,
    page: usize,
    /// The page last drawn and the first app on it, which keeps the owner
    /// on that app when a banner moves the folds. Cleared by a turn or a
    /// filter.
    anchor: Option<(usize, String)>,
    refreshing: bool,
    refresh_after_cache: bool,
    notice: Option<Notice>,
    app_link: AppLinkState,
    link_poll: Heartbeat,
    link_request_pending: bool,
    /// Why the runtime refused the browser link, until it answers one.
    link_refused: Option<DenyReason>,
    pairing_qr: Option<TilePicture>,
    pairing_qr_url: Option<String>,
    /// The page of an app's page the owner turned to, and which app it was,
    /// so another app's page always opens on its first.
    detail_page: Option<(String, usize)>,
    /// What each installed app was granted, as the reader lists it. `None`
    /// until the reader has answered, and then an update from a row opens
    /// the app's page, because nobody can yet say it asks for nothing new.
    granted: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            view: View::Catalog,
            filter: Filter::All,
            page: 0,
            anchor: None,
            refreshing: false,
            refresh_after_cache: false,
            notice: None,
            app_link: AppLinkState::Unpaired,
            link_poll: Heartbeat::every(5),
            link_request_pending: false,
            link_refused: None,
            pairing_qr: None,
            pairing_qr_url: None,
            detail_page: None,
            granted: None,
        }
    }
}

impl Store {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view.clone() {
            View::Catalog => self.catalog(context),
            View::Detail(id) => self.detail_for(context, &id),
            View::ConfirmUninstall(id) => self.confirm_uninstall_for(context, &id),
            View::Working { id, action, .. } => self.working(&id, action),
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
    fn catalog_head(
        &self,
        metrics: &kobo_ui::DisplayMetrics,
        page: usize,
        pages: usize,
        compact: bool,
    ) -> ScreenBuilder {
        let count = |filter: Filter| {
            self.entries
                .iter()
                .filter(|entry| filter.admits(entry))
                .count()
        };
        // The most counts the tabs can carry with every label whole.
        let counted = [Counts::All, Counts::Updates]
            .into_iter()
            .find(|counts| tabs_read_whole(metrics, &self.tab_labels(*counts)))
            .unwrap_or(Counts::None);
        let mut screen = catalog_bar(metrics);
        // Try again belongs to the app's page, where the permissions are,
        // so the list says what happened without offering it.
        if let Some(notice) = &self.notice {
            screen = notice.draw_with_action(screen, false);
        }
        let selected = Filter::ALL
            .iter()
            .position(|filter| *filter == self.filter)
            .unwrap_or(0);
        screen = screen.tabs(
            selected,
            Filter::ALL
                .iter()
                .zip(self.tab_labels(counted))
                .map(|(filter, label)| (filter.action(), label)),
        );
        let title = match (self.filter, self.refreshing) {
            (Filter::All, false) => "Apps",
            (Filter::All, true) => "Apps · refreshing",
            (Filter::Installed, _) => "Installed",
            (Filter::Updates, _) => "Updates",
            (Filter::Available, _) => "Available to install",
        };
        // Where the open tab could not carry its count, the count moves to
        // the heading of the list it filters.
        let title = if counted.shows(self.filter) || self.filter == Filter::All {
            title.to_owned()
        } else {
            format!("{title} · {}", count(self.filter))
        };
        if compact {
            return screen;
        }
        screen.section_with_value(title, format!("{} / {}", page + 1, pages.max(1)))
    }

    /// The four tab labels, each with its count where `counts` says so.
    ///
    /// Four tabs share the panel's width, and at the largest text sizes
    /// "Installed 21" no longer fits a quarter of it: it was cut to
    /// "Installed…", which hid the count and half the word. There the counts
    /// give way and the words stay whole, Updates' last, because it is the
    /// one that asks for a tap. The list's heading says the count of
    /// whichever tab is open, and the apps with an update still come first,
    /// each with its own Update button.
    fn tab_labels(&self, counts: Counts) -> [String; 4] {
        let label = |name: &str, filter: Filter| {
            if counts.shows(filter) {
                let count = self
                    .entries
                    .iter()
                    .filter(|entry| filter.admits(entry))
                    .count();
                format!("{name} {count}")
            } else {
                name.to_owned()
            }
        };
        [
            "All".to_owned(),
            label("Installed", Filter::Installed),
            label("Updates", Filter::Updates),
            label("Available", Filter::Available),
        ]
    }

    /// Breaks the listed apps into pages under the catalog's own head.
    ///
    /// Each row is measured with what it carries at its edge: an Install or
    /// Update button, the overflow mark, the version or state. Measuring
    /// every row as if it had the widest button took a column from each
    /// summary that its row never gives up, so a page held a row fewer than
    /// it could and the largest size showed one app per page. The catalog
    /// tests lay every page out at every text size, so if this and the layout
    /// engine ever disagree a row under the bottom bar fails a test rather
    /// than reaching a reader.
    fn pages(
        &self,
        context: &Context,
        rows: &[(&str, &str, kobo_ui::RowEdge<'_>)],
        nav_bar: bool,
        compact: bool,
    ) -> Vec<Vec<usize>> {
        let metrics = context.metrics();
        let head = self.catalog_head(&metrics, 0, 1, compact).build();
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
            kobo_ui::paginate_rows_with_edges(rows, &metrics, area)
        });
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// The catalog, with the heading over the list given up where a banner
    /// leaves no room under it for a single app, which is a panel held
    /// sideways at the largest sizes. The banner and every app stay; the
    /// heading's page count is what goes, and the bar still turns the pages.
    fn catalog(&mut self, context: &Context) -> Screen {
        let screen = self.catalog_shaped(context, false);
        if self.notice.is_some()
            && screen
                .diagnostics(&context.metrics(), &kobo_ui::Chrome::measuring(true))
                .has_errors()
        {
            return self.catalog_shaped(context, true);
        }
        screen
    }

    fn catalog_shaped(&mut self, context: &Context, compact: bool) -> Screen {
        if self.entries.is_empty() {
            let mut screen = catalog_bar(&context.metrics());
            if let Some(notice) = &self.notice {
                screen = notice.draw(screen);
            }
            // While the catalog is being fetched there is nothing to press:
            // a Refresh button then would only start the same fetch again.
            if self.refreshing {
                return screen
                    .splash(
                        Some(Glyph::Download),
                        "Refreshing apps",
                        "The list of apps is being fetched.",
                    )
                    .build();
            }
            return screen
                .splash(
                    Some(Glyph::Download),
                    "No apps available",
                    "Connect Wi-Fi and refresh the catalog.",
                )
                .bottom_action_marked(REFRESH, "Refresh", Glyph::Refresh)
                .build();
        }
        let listed = self.listed();
        let metrics = context.metrics();
        let shapes = listed
            .iter()
            .map(|entry| RowShape::of(entry, self.updates_quietly(entry), &metrics))
            .collect::<Vec<_>>();
        let rows = listed
            .iter()
            .zip(&shapes)
            .map(|(entry, shape)| (entry.title.as_str(), shape.summary.as_str(), shape.edge()))
            .collect::<Vec<_>>();
        // A one-page catalog draws no bottom bar and gets that room for apps.
        // Once it turns, measure again with the navigation bar it will draw.
        let without_controls = self.pages(context, &rows, false, compact);
        let page_indices = if without_controls.len() > 1 {
            self.pages(context, &rows, true, compact)
        } else {
            without_controls
        };
        let pages = page_indices.len();
        // A banner arriving or going moves the folds. The page that now
        // holds the app the owner was looking at is the one they stay on,
        // rather than the same page number over different apps.
        let anchored = self.anchor.as_ref().and_then(|(drawn, id)| {
            if *drawn != self.page {
                return None;
            }
            let index = listed.iter().position(|entry| &entry.id == id)?;
            page_indices.iter().position(|page| page.contains(&index))
        });
        let page = anchored.unwrap_or(self.page).min(pages - 1);
        let mut screen = self.catalog_head(&context.metrics(), page, pages, compact);
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
            let shown = page_indices[page]
                .iter()
                .filter_map(|index| Some((*listed.get(*index)?, shapes.get(*index)?)))
                .collect::<Vec<_>>();
            screen = catalog_rows(screen, &shown);
        }
        let anchor = page_indices[page]
            .first()
            .and_then(|index| listed.get(*index))
            .map(|entry| (page, entry.id.clone()));
        self.page = page;
        self.anchor = anchor;
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
            // One name for this everywhere it appears: the bar on the list,
            // this screen's title and the button that starts it. It used to
            // be "Link browser", "Install from a browser" and "Link this
            // Kobo" on three consecutive taps.
            .top_bar(LINK_LABEL)
            .owns_back(true);
        if let Some(notice) = &self.notice {
            screen = notice.draw(screen);
        }
        match &self.app_link {
            // A refusal says why on the splash itself, and offers to try
            // again only where waiting can change the answer.
            AppLinkState::Unpaired => match self.link_refused {
                Some(reason) => {
                    let screen = screen.splash(
                        Some(Glyph::Globe),
                        "Browser links are unavailable",
                        link_refusal(reason),
                    );
                    if matches!(reason, DenyReason::WithheldForBattery | DenyReason::Busy) {
                        screen.bottom_action_marked(BEGIN_LINK, "Try again", Glyph::Refresh)
                    } else {
                        screen
                    }
                    .build()
                }
                // Nothing is linked, so the one thing to do is link, whether
                // or not the last attempt failed.
                None => screen
                    .splash(
                        Some(Glyph::Globe),
                        "No browser linked",
                        "Link the browser on your computer or phone to install apps from Cobalt's app pages. What it sends is encrypted for this Kobo.",
                    )
                    .bottom_action_marked(BEGIN_LINK, LINK_LABEL, Glyph::Key)
                    .build(),
            },
            AppLinkState::Pairing {
                code,
                url,
                expires_in,
            } => {
                let minutes = (*expires_in).div_ceil(60);
                let verification = pairing_verification(url).unwrap_or_else(|| "Unavailable".into());
                let mut screen = screen.text(
                    "Scan the code with your phone, or open the address below in a browser and enter the code and key shown.",
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
                self.link_refused = None;
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
            // Said on the link's own screen only. The list reads the link
            // when it opens, and a refusal there is not news about the list.
            DeviceResult::Failed(error) => {
                self.link_request_pending = false;
                if self.view == View::AppLink {
                    self.notice = Some(Notice::error(link_failure(*error)));
                }
            }
            DeviceResult::Denied(reason) => {
                self.link_request_pending = false;
                self.link_refused = Some(*reason);
                if self.view == View::AppLink {
                    self.notice = None;
                }
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
    ///
    /// A failed install or update puts Try again on its banner. Held
    /// sideways at a large text size the page cannot spare the banner that
    /// extra line, so there Try again goes back to the bar at the foot,
    /// where it was before banners could carry an action.
    fn detail_for(&self, context: &Context, id: &str) -> Screen {
        self.detail_builder(context, id).build()
    }

    #[cfg(test)]
    fn detail(&self, id: &str) -> Screen {
        self.detail_for(&Context::default(), id)
    }

    /// The app's page, shaped for the panel and the text size.
    ///
    /// Everything on it is something the owner decides by, the permissions
    /// above all, so nothing is left off to make it fit. Where the full page
    /// is too tall, the large picture and the title it repeats from the top
    /// bar give way first, and the permissions move up to sit under the
    /// summary. Where that is still too tall, which is a page with a banner
    /// at the largest sizes, the facts go on to a second page.
    fn detail_builder(&self, context: &Context, id: &str) -> ScreenBuilder {
        let (shape, pages) = self.detail_layout(context, id);
        let page = self
            .detail_page
            .as_ref()
            .filter(|(shown, _)| shown == id)
            .map_or(0, |(_, page)| *page)
            .min(pages.len().saturating_sub(1));
        self.detail_screen(
            id,
            shape,
            pages.get(page).map_or(&[][..], Vec::as_slice),
            Some((page, pages.len())),
        )
    }

    /// How the app's page is shaped on this panel, and which parts go on
    /// each of its pages.
    fn detail_layout(&self, context: &Context, id: &str) -> (DetailShape, Vec<Vec<usize>>) {
        let metrics = context.metrics();
        let all = (0..self.detail_parts(id).len()).collect::<Vec<_>>();
        for shape in [DetailShape::Splash, DetailShape::Plain] {
            if shows_whole(&self.detail_screen(id, shape, &all, None).build(), &metrics) {
                return (shape, vec![all]);
            }
        }
        (DetailShape::Plain, self.paged_parts(context, id))
    }

    fn detail_pages(&self, context: &Context, id: &str) -> Vec<Vec<usize>> {
        self.detail_layout(context, id).1
    }

    /// The parts of an app's page under its heading, kept whole on whichever
    /// page they land: its summary, then one per fact. The permissions come
    /// first among the facts, because they are what the owner is asked to
    /// agree to by installing.
    fn detail_parts(&self, id: &str) -> Vec<DetailPart> {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return Vec::new();
        };
        let installed = entry.installed_version.as_deref();
        vec![
            DetailPart::Summary(entry.summary.clone()),
            DetailPart::Fact("Permissions", permissions(&entry.capabilities)),
            DetailPart::Fact("Available", entry.version.clone()),
            DetailPart::Fact("Installed", installed.unwrap_or("Not installed").to_owned()),
            DetailPart::Fact("Requires Cobalt", entry.minimum_cobalt_version.clone()),
            DetailPart::Fact(
                "Management",
                if is_system_app(id) {
                    "Built into Cobalt".to_owned()
                } else {
                    "Managed by App Store".to_owned()
                },
            ),
        ]
    }

    /// Which parts go on which page, for a page too tall to hold them all.
    /// Each part moves to the next page rather than being cut, and a page is
    /// measured with its banner, its bottom bar and its page strip.
    fn paged_parts(&self, context: &Context, id: &str) -> Vec<Vec<usize>> {
        let metrics = context.metrics();
        let count = self.detail_parts(id).len();
        let mut pages: Vec<Vec<usize>> = Vec::new();
        for part in 0..count {
            let page = pages.len().saturating_sub(1);
            let joined = pages
                .last()
                .map(|parts| [parts.as_slice(), &[part]].concat());
            match joined {
                Some(joined)
                    if shows_whole(
                        &self
                            .detail_screen(id, DetailShape::Plain, &joined, Some((page, 2)))
                            .build(),
                        &metrics,
                    ) =>
                {
                    *pages.last_mut().expect("a page") = joined;
                }
                _ => pages.push(vec![part]),
            }
        }
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        pages
    }

    fn detail_screen(
        &self,
        id: &str,
        shape: DetailShape,
        parts: &[usize],
        paged: Option<(usize, usize)>,
    ) -> ScreenBuilder {
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
        // A banner says what happened on the first page. The pages after it
        // are for the facts the owner turned to read, and where the banner
        // offered Try again, their bar offers it instead.
        let banner = paged.is_none_or(|(page, _)| page == 0);
        if let Some(notice) = self.notice.as_ref().filter(|_| banner) {
            screen = notice.draw(screen);
        }
        let every = self.detail_parts(id);
        let chosen = parts
            .iter()
            .filter_map(|&index| every.get(index))
            .collect::<Vec<_>>();
        let facts = chosen
            .iter()
            .filter_map(|part| match part {
                DetailPart::Fact(label, value) => Some((*label, value.clone())),
                DetailPart::Summary(_) => None,
            })
            .collect::<Vec<_>>();
        let summary = chosen.iter().find_map(|part| match part {
            DetailPart::Summary(summary) => Some(summary.clone()),
            DetailPart::Fact(..) => None,
        });
        screen = match (shape, summary) {
            (DetailShape::Splash, Some(summary)) => {
                // The full page keeps the order it has always had, with the
                // permissions last.
                let mut ordered = facts.clone();
                let first = usize::from(!ordered.is_empty());
                ordered.rotate_left(first);
                screen
                    .splash(Some(entry.glyph), entry.title.clone(), summary)
                    .facts(ordered)
            }
            (_, Some(summary)) => screen.text(summary).facts(facts),
            (_, None) => screen.facts(facts),
        };
        if let Some((page, pages)) = paged.filter(|(_, pages)| *pages > 1) {
            screen = screen
                .page_turns(DETAIL_PREVIOUS, DETAIL_NEXT)
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages).unwrap_or(u16::MAX),
                );
        }
        // A failed install or update is offered again by its banner, so the
        // bar does not offer the same thing a second time.
        let again = self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.retry.as_deref() == Some(id));
        let retry = again && banner;
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
            if entry.has_update() && !retry {
                actions.insert(
                    0,
                    (
                        install_action(id),
                        if again { "Try again" } else { "Update" },
                        Some(Glyph::Download),
                    ),
                );
            }
            screen.action_bar_marked(actions)
        } else if retry {
            screen
        } else if again {
            screen.bottom_action_marked(install_action(id), "Try again", Glyph::Refresh)
        } else {
            screen.bottom_action_marked(install_action(id), "Install", Glyph::Download)
        }
    }

    /// Asks before an uninstall, over the screen the owner tapped it on.
    ///
    /// Uninstalling is one tap away from Open on the same bar, and an app
    /// that took a USB trip and a key to set up should not be one brush of
    /// the panel from gone. The question also answers the thing an owner
    /// worries about: the app's saved data stays, because `kobod` keeps an
    /// app's state outside the package it removes.
    fn confirm_uninstall_for(&self, context: &Context, id: &str) -> Screen {
        let title = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or(id, |entry| entry.title.as_str());
        self.detail_builder(context, id)
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

    /// Turns the app's page one page, clamped at both ends.
    fn turn_detail_page(&mut self, context: &mut Context, forward: bool) {
        let (View::Detail(id) | View::ConfirmUninstall(id)) = self.view.clone() else {
            return;
        };
        let pages = self.detail_pages(context, &id).len();
        let page = self
            .detail_page
            .as_ref()
            .filter(|(shown, _)| *shown == id)
            .map_or(0, |(_, page)| *page);
        let page = if forward {
            (page + 1).min(pages.saturating_sub(1))
        } else {
            page.saturating_sub(1)
        };
        self.detail_page = Some((id, page));
        self.show(context);
    }

    /// Whether an update asks for nothing the installed version was not
    /// already granted, as the reader listed it. Unknown counts as no.
    fn updates_quietly(&self, entry: &AppInfo) -> bool {
        self.granted
            .as_ref()
            .and_then(|granted| granted.get(&entry.id))
            .is_some_and(|granted| {
                entry
                    .capabilities
                    .iter()
                    .all(|capability| granted.contains(capability))
            })
    }

    fn request_install(&mut self, context: &mut Context, id: &str) {
        let action = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .filter(|entry| entry.is_installed())
            .map_or("Installing", |_| "Updating");
        let from_catalog = self.view == View::Catalog;
        self.notice = None;
        self.view = View::Working {
            id: id.to_owned(),
            action,
            from_catalog,
        };
        self.show(context);
        if !context.applications().install(id.to_owned()) {
            self.notice = Some(Notice::attention("That application identity is invalid."));
            self.view = self.after_working();
            self.show(context);
        }
    }

    fn request_uninstall(&mut self, context: &mut Context, id: String) {
        self.notice = None;
        self.view = View::Working {
            id: id.clone(),
            action: "Removing",
            from_catalog: false,
        };
        self.show(context);
        if !context.applications().uninstall(id.clone()) {
            self.notice = Some(Notice::attention("That application identity is invalid."));
            self.view = View::Detail(id);
            self.show(context);
        }
    }

    /// Where a finished or refused change leaves the owner: back on the list
    /// if it was started from a row there, otherwise on the app it was
    /// about, unless nothing was in progress.
    fn after_working(&self) -> View {
        match &self.view {
            View::Working {
                from_catalog: true, ..
            } => View::Catalog,
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
        // What was just installed is what it was granted.
        if let (Some(granted), Some(entry)) = (
            self.granted.as_mut(),
            self.entries.iter().find(|entry| entry.id == id),
        ) {
            granted.insert(id.to_owned(), entry.capabilities.clone());
        }
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
            self.anchor = None;
            self.notice = None;
        }
        self.show(context);
        true
    }
}

impl KoboApp for Store {
    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        self.refreshing = true;
        self.refresh_after_cache = true;
        self.show(context);
        context.applications().cached_catalog();
        context.store().read_link();
        context.applications().installed();
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK || action == action_id(CANCEL_REMOVE) {
            if let View::ConfirmUninstall(id) = &self.view {
                self.view = View::Detail(id.clone());
            } else if action == ActionId::BACK {
                self.notice = None;
                self.detail_page = None;
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
        if action == action_id(DETAIL_PREVIOUS) || action == action_id(DETAIL_NEXT) {
            self.turn_detail_page(context, action == action_id(DETAIL_NEXT));
            return;
        }
        if action == action_id(PREVIOUS) || action == action_id(NEXT) {
            // The banner was read on the page it arrived on. Carrying it onto
            // every later page spends a row of apps on old news.
            self.notice = None;
            self.anchor = None;
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
                self.request_install(context, &id);
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
        if request == DeviceRequest::ListInstalledApps {
            // Without the list every update opens the app's page, which is
            // the careful way round, so a refusal needs no banner.
            if let DeviceResult::Apps { entries } = result {
                self.granted = Some(
                    entries
                        .into_iter()
                        .map(|entry| (entry.id, entry.capabilities))
                        .collect(),
                );
            }
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
                        ..
                    } if working_id == &id => "updated",
                    _ => "installed",
                };
                self.notice = Some(Notice::success(format!(
                    "{} is {outcome}.",
                    self.title_of(&id)
                )));
                self.mark_installed(&id);
                self.view = match self.after_working() {
                    View::Catalog => View::Catalog,
                    _ => View::Detail(id),
                };
                context.applications().cached_catalog();
            }
            (DeviceRequest::UninstallApp { id }, DeviceResult::Done) => {
                self.notice = Some(Notice::success(format!(
                    "{} is uninstalled. Its saved data is still on this reader.",
                    self.title_of(&id)
                )));
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                    entry.installed_version = None;
                }
                if let Some(granted) = self.granted.as_mut() {
                    granted.remove(&id);
                }
                self.view = View::Catalog;
                context.applications().cached_catalog();
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Failed(error)) => {
                self.refreshing = false;
                self.notice = Some(Notice::attention(refresh_failure(error)));
            }
            (DeviceRequest::ReadAppCatalog, DeviceResult::Failed(_)) => {
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (DeviceRequest::InstallApp { id }, DeviceResult::Failed(error)) => {
                // The app's own page, even for an install started from its
                // row: the page names the app and has the room for the reason
                // and the way to try again, which the list does not.
                self.notice = Some(Notice::retry(app_failure(error), id.clone()));
                self.view = View::Detail(id);
                // A write may have reached disk before its final flush failed.
                // Re-read verified installed metadata instead of promising rollback.
                context.applications().cached_catalog();
                context.applications().installed();
            }
            (DeviceRequest::UninstallApp { id }, DeviceResult::Failed(error)) => {
                self.notice = Some(Notice::error(app_failure(error)));
                self.view = View::Detail(id);
                context.applications().cached_catalog();
            }
            (_, DeviceResult::Denied(reason)) => {
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
            Some(Notice::success(format!("{} is installed.", name(id))))
        }
        RemoteInstallOutcome::Updated { id } => {
            Some(Notice::success(format!("{} is updated.", name(id))))
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

/// Why the runtime would not link a browser, in the owner's words. The
/// Store's own refusals are about installing, which is not what was asked.
const fn link_refusal(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::NotDeclared | DenyReason::PolicyRejected => {
            "Cobalt does not allow the App Store to link a browser."
        }
        DenyReason::Unsupported => "This version of Cobalt cannot link a browser on this reader.",
        DenyReason::WithheldForBattery => "Charge the reader, then try again.",
        DenyReason::Busy => "The link is busy. Try again in a moment.",
    }
}

/// What a link request that failed says, with what to do next.
const fn link_failure(error: DeviceError) -> &'static str {
    match error {
        DeviceError::Unreachable | DeviceError::NotFound | DeviceError::TimedOut => {
            "The link could not be reached. Connect to Wi-Fi, then try again."
        }
        _ => "The link could not be set up. Try again later.",
    }
}

fn app_failure(error: DeviceError) -> &'static str {
    match error {
        DeviceError::NotFound => "This app is no longer available. Refresh the app list.",
        DeviceError::Authentication => {
            "The download was refused. Refresh the app list and try again."
        }
        DeviceError::TimedOut => "The download took too long. Check Wi-Fi and try again.",
        DeviceError::Unreachable => "Couldn't reach the download. Check Wi-Fi and try again.",
        DeviceError::InvalidInput => {
            "This app needs a newer Cobalt. Refresh the app list and update Cobalt."
        }
        DeviceError::Backend => {
            "Couldn't save the change. Check free space and the installed version."
        }
        DeviceError::Integrity => {
            "The downloaded app could not be verified. Refresh the app list and try again."
        }
    }
}

/// The words at the edge of an app's catalog row that has no button.
///
/// An installed app shows only its version: the row opening the app already
/// says it is installed, and on the Installed tab the word would be on
/// every row.
fn app_state(entry: &AppInfo) -> String {
    if is_system_app(&entry.id) {
        return "Built in".to_owned();
    }
    if !entry.is_compatible_with(env!("CARGO_PKG_VERSION")) {
        return format!("Requires Cobalt {}", entry.minimum_cobalt_version);
    }
    match &entry.installed_version {
        None => "Available".to_owned(),
        Some(version) if entry.has_update() => format!("{version} → {}", entry.version),
        Some(version) => version.clone(),
    }
}

/// The rows of one catalog page.
///
/// A tap on an installed app's row opens the app, as it would on the home
/// screen, and the mark at its edge leads to the app's page, where it is
/// uninstalled. Any other row opens the app's page. Install and Update sit
/// on the row itself, so the owner does not open a page to do the one thing
/// the row is for. A row with no button says why in words: it needs a newer
/// Cobalt, or its app is already installed.
fn catalog_rows(screen: ScreenBuilder, shown: &[(&AppInfo, &RowShape)]) -> ScreenBuilder {
    let mut screen = screen.rows_with_trailing(shown.iter().map(|(entry, shape)| {
        (
            row_action(entry),
            entry.title.clone(),
            shape.summary.clone(),
            RowLead::from(entry.glyph),
            shape.trailing.clone(),
        )
    }));
    for (entry, shape) in shown {
        if let Some((action, label)) = &shape.button {
            screen = screen.row_button(row_action(entry), action, *label);
        }
        if shape.menu {
            screen = screen.row_menu(row_action(entry), app_action(&entry.id));
        }
    }
    screen
}

/// How many lines of its summary a catalog row shows.
///
/// The list is for finding an app, and its page has the whole summary. At
/// the largest sizes a summary of three sentences took five lines, and with
/// it a page held one app.
const ROW_SUMMARY_LINES: usize = 3;

/// One catalog row as it is drawn and measured: its summary cut to
/// [`ROW_SUMMARY_LINES`] at the width it will really have, and what stands
/// at its edge.
struct RowShape {
    summary: String,
    trailing: String,
    button: Option<(String, &'static str)>,
    menu: bool,
}

impl RowShape {
    fn of(entry: &AppInfo, quiet_update: bool, metrics: &kobo_ui::DisplayMetrics) -> Self {
        let button = row_button(entry, quiet_update);
        let trailing = if button.is_some() {
            String::new()
        } else {
            app_state(entry)
        };
        let mut shape = Self {
            summary: String::new(),
            trailing,
            button,
            menu: opens_from_row(entry),
        };
        shape.summary = kobo_ui::with_text_scale(metrics.text_scale, || {
            let area = metrics.prose_area_in(true, true, kobo_ui::Face::Text);
            kobo_ui::clamp_lines(
                &entry.summary,
                kobo_ui::row_title_width_at_edge(metrics, area, shape.edge()),
                kobo_ui::FontSize::Caption,
                ROW_SUMMARY_LINES,
            )
        });
        shape
    }

    fn edge(&self) -> kobo_ui::RowEdge<'_> {
        let mut edge = kobo_ui::RowEdge::new().with_trailing(&self.trailing);
        if let Some((_, label)) = &self.button {
            edge = edge.with_button(label);
        }
        if self.menu {
            edge = edge.with_menu();
        }
        edge
    }
}

/// Whether a tap on the app's catalog row opens the app itself.
fn opens_from_row(entry: &AppInfo) -> bool {
    is_system_app(&entry.id) || entry.is_installed()
}

/// What a tap on the app's catalog row does: open the app, or its page.
fn row_action(entry: &AppInfo) -> String {
    if opens_from_row(entry) {
        open_action(&entry.id)
    } else {
        app_action(&entry.id)
    }
}

/// The button a catalog row carries, as its action and its label.
///
/// None for an installed app with nothing to update, whose row opens it,
/// and for an app that needs a newer Cobalt: its row says so instead, and a
/// tap on it opens the app's page, which offers the Cobalt update.
///
/// Install opens the app's page rather than installing, so what the app may
/// do is read before anything is installed; the page's own Install is the
/// one that installs. Update installs from the row only when the new
/// version asks for nothing the owner has not already granted; otherwise it
/// opens the page too, where the permissions are listed.
fn row_button(entry: &AppInfo, quiet_update: bool) -> Option<(String, &'static str)> {
    if is_system_app(&entry.id) || !entry.is_compatible_with(env!("CARGO_PKG_VERSION")) {
        return None;
    }
    if !entry.is_installed() {
        Some((app_action(&entry.id), "Install"))
    } else if entry.has_update() && quiet_update {
        Some((install_action(&entry.id), "Update"))
    } else if entry.has_update() {
        Some((app_action(&entry.id), "Update"))
    } else {
        None
    }
}

/// Which tabs carry their counts. See [`Store::tab_labels`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Counts {
    All,
    Updates,
    None,
}

impl Counts {
    fn shows(self, filter: Filter) -> bool {
        match self {
            Self::All => filter != Filter::All,
            Self::Updates => filter == Filter::Updates,
            Self::None => false,
        }
    }
}

/// How an app's page is laid out. See [`Store::detail_builder`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DetailShape {
    /// The app's picture and title over its summary, as at most sizes.
    Splash,
    /// The summary under the top bar that already names the app.
    Plain,
}

/// One part of an app's page.
enum DetailPart {
    Summary(String),
    Fact(&'static str, String),
}

/// Whether a page fits the panel with every fact it was given drawn, since a
/// list of facts too long for the room left is shortened without a word.
fn shows_whole(screen: &Screen, metrics: &kobo_ui::DisplayMetrics) -> bool {
    let report = screen.diagnostics(metrics, &kobo_ui::Chrome::measuring(true));
    let given = screen
        .nodes
        .iter()
        .map(|node| match node {
            kobo_ui::Node::Facts { entries, .. } => entries.len(),
            _ => 0,
        })
        .sum::<usize>();
    let drawn = report
        .layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, kobo_ui::LayoutKind::FactLabel))
        .count();
    !report.has_errors() && drawn == given
}

/// The catalog's top bar: its title, Refresh, and the way to link a browser.
///
/// The link is offered in words where they leave the title whole. At the
/// largest sizes the words took the bar and cut the title to "App…", so
/// there the link is drawn as the globe its own screen opens with, and keeps
/// its name as the control's label.
fn catalog_bar(metrics: &kobo_ui::DisplayMetrics) -> ScreenBuilder {
    let bar = || {
        ScreenBuilder::new("store-catalog")
            .top_bar("App Store")
            .top_bar_glyph(REFRESH, "Refresh", Glyph::Refresh)
    };
    let worded = bar().top_bar_action(APP_LINK, LINK_LABEL);
    if bar_title_reads_whole(&worded.clone().build(), metrics) {
        worded
    } else {
        bar().top_bar_glyph(APP_LINK, LINK_LABEL, Glyph::Globe)
    }
}

/// Whether the top bar of `screen` draws its title whole.
fn bar_title_reads_whole(screen: &Screen, metrics: &kobo_ui::DisplayMetrics) -> bool {
    let title = screen.top_bar.as_ref().map_or("", |bar| bar.title.as_str());
    kobo_ui::with_text_scale(metrics.text_scale, || {
        screen
            .layout_with(metrics, &kobo_ui::Chrome::measuring(true))
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, kobo_ui::LayoutKind::TopBarTitle))
            .all(|node| node.text_lines.first().map(String::as_str) == Some(title))
    })
}

/// Whether a strip of these tab labels is drawn with every label whole,
/// measured by laying the strip out as the runtime will.
fn tabs_read_whole(metrics: &kobo_ui::DisplayMetrics, labels: &[String]) -> bool {
    let screen = ScreenBuilder::new("store-tabs")
        .top_bar("App Store")
        .tabs(
            0,
            Filter::ALL
                .iter()
                .zip(labels)
                .map(|(filter, label)| (filter.action(), label.clone())),
        )
        .build();
    kobo_ui::with_text_scale(metrics.text_scale, || {
        screen
            .layout_with(metrics, &kobo_ui::Chrome::measuring(true))
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, kobo_ui::LayoutKind::Tab(..)))
            .zip(labels)
            .all(|(node, label)| node.text_lines.first() == Some(label))
    })
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

    impl Store {
        /// A copy of what the catalog is drawn from, to draw it again
        /// without moving the runner's own store.
        fn clone_for_test(&self) -> Self {
            Self {
                entries: self.entries.clone(),
                granted: self.granted.clone(),
                ..Self::default()
            }
        }
    }

    /// A store that has already painted its cached catalog and link state.
    fn opened(entries: &[AppInfo]) -> AppRunner<Store> {
        opened_with(entries, &[])
    }

    /// The same, with the reader listing `installed` as what is installed
    /// and what each was granted.
    fn opened_with(entries: &[AppInfo], installed: &[AppInfo]) -> AppRunner<Store> {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: entries.to_vec(),
        });
        runner.device_result(DeviceResult::AppLink(AppLinkState::Unpaired));
        runner.device_result(DeviceResult::Apps {
            entries: installed.to_vec(),
        });
        runner.device_result(DeviceResult::Apps {
            entries: entries.to_vec(),
        });
        runner
    }

    /// An installed app one version behind the catalog, and the version it
    /// has installed, granted `granted`.
    fn behind(id: &str, granted: &[&str]) -> (AppInfo, AppInfo) {
        let mut listed = app(id, Some("1.0.0"));
        listed.version = "1.1.0".to_owned();
        let mut installed = app(id, Some("1.0.0"));
        installed.version = "1.0.0".to_owned();
        installed.capabilities = granted.iter().map(|&name| name.to_owned()).collect();
        (listed, installed)
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
            vec![
                &DeviceRequest::ReadAppCatalog,
                &DeviceRequest::ReadAppLink,
                &DeviceRequest::ListInstalledApps
            ]
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
            entries: Vec::new(),
        });
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
        runner.device_result(DeviceResult::Apps {
            entries: Vec::new(),
        });
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
        for metrics in every_size_held_either_way() {
            {
                for notice in [
                    None,
                    Some(Notice::attention(app_failure(DeviceError::Backend))),
                    Some(Notice::success("Quality fixture is installed.")),
                    Some(Notice::retry(
                        app_failure(DeviceError::Backend),
                        "quality-fixture",
                    )),
                ] {
                    let context = AppRunner::with_metrics(Store::default(), metrics).context();
                    let mut store = Store::default();
                    store.replace_entries(entries.clone());
                    store.notice = notice;
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
    fn the_catalog_title_reads_whole_at_every_size() {
        // At the largest sizes "Link a browser" took the bar and left "App…".
        for metrics in every_size_held_either_way() {
            let context = AppRunner::with_metrics(Store::default(), metrics).context();
            let mut store = Store::default();
            store.replace_entries(bundled(None));
            let screen = store.catalog(&context);
            assert!(bar_title_reads_whole(&screen, &metrics), "{metrics:?}");
            let bar = screen.top_bar.as_ref().expect("a top bar");
            assert!(
                bar.actions.iter().any(|action| action.label == LINK_LABEL),
                "{metrics:?}: the link lost its name"
            );
        }
    }

    #[test]
    fn a_banner_that_moves_the_folds_keeps_the_owner_on_the_apps_they_were_reading() {
        // A failed refresh on the last page turned "11 / 11" into "11 / 14",
        // over apps the owner had not been looking at.
        let metrics = CLARA_BW_METRICS;
        let context = AppRunner::with_metrics(Store::default(), metrics).context();
        let mut store = Store::default();
        store.replace_entries(bundled(None));
        store.page = usize::MAX;
        let rows = |screen: &Screen| {
            screen
                .layout_with(&metrics, &Chrome::measuring(true))
                .nodes
                .iter()
                .filter_map(|node| match node.kind {
                    LayoutKind::Row(action, ..) => Some(action),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let first = |screen: &Screen| rows(screen).first().copied();
        let before = store.catalog(&context);
        store.notice = Some(Notice::attention(refresh_failure(DeviceError::Unreachable)));
        let after = store.catalog(&context);
        let top = first(&before).expect("a row on the last page");
        assert!(
            rows(&after).contains(&top),
            "the app the page began with moved away"
        );
    }

    #[test]
    fn a_refused_browser_link_says_why_in_the_links_words_and_offers_no_reset() {
        let mut store = Store {
            view: View::AppLink,
            ..Store::default()
        };
        let mut context = kobo_sdk::Context::default();
        kobo_sdk::KoboApp::on_device_result(
            &mut store,
            &mut context,
            DeviceRequest::ReadAppLink,
            DeviceResult::Denied(DenyReason::Unsupported),
        );
        let said = words(&store.app_link(), &CLARA_BW_METRICS);
        assert!(said.contains("cannot link a browser"), "{said}");
        assert!(!said.contains("install apps"), "{said}");
        assert!(!said.contains("Reset link"), "{said}");
        // A link that failed for want of Wi-Fi still offers to link, not
        // to reset a link that does not exist.
        let mut store = Store {
            view: View::AppLink,
            ..Store::default()
        };
        kobo_sdk::KoboApp::on_device_result(
            &mut store,
            &mut context,
            DeviceRequest::BeginAppLink,
            DeviceResult::Failed(DeviceError::Unreachable),
        );
        let said = words(&store.app_link(), &CLARA_BW_METRICS);
        assert!(said.contains(LINK_LABEL), "{said}");
        assert!(!said.contains("Reset link"), "{said}");
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
        runner.device_result(DeviceResult::Apps {
            entries: Vec::new(),
        });
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
    fn system_apps_are_marked_built_in_and_cannot_be_removed() {
        let mut settings = app("settings", Some("0.2.0"));
        settings.version = "0.2.0".to_owned();
        let store = Store {
            entries: vec![settings],
            view: View::Detail("settings".to_owned()),
            ..Store::default()
        };
        assert_eq!(app_state(&store.entries[0]), "Built in");
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
        // Try again is the error banner's own action, and the bar does not
        // offer Install beside it.
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            kobo_ui::Node::Banner {
                level: BannerLevel::Error,
                action: Some(action),
                ..
            } if action.action == action_id(&install_action("birds")) && action.label == "Try again"
        )));
        assert!(screen.bottom_action.is_none() && screen.nav_bar.is_none());
        let retry = runner.action(action_id(&install_action("birds")));
        assert!(retry.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::InstallApp { id }) if id == "birds"
        )));
    }

    fn bundled(installed: Option<&str>) -> Vec<AppInfo> {
        kobo_catalog::bundled()
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
                installed_version: installed.map(str::to_owned),
            })
            .collect()
    }

    /// The default and the largest text size, held either way.
    fn sizes_and_poses() -> Vec<DisplayMetrics> {
        [TextScale::Default, TextScale::Largest]
            .into_iter()
            .flat_map(|text_scale| {
                [
                    DisplayMetrics {
                        text_scale,
                        ..CLARA_BW_METRICS
                    },
                    DisplayMetrics {
                        width: CLARA_BW_METRICS.height,
                        height: CLARA_BW_METRICS.width,
                        text_scale,
                        ..CLARA_BW_METRICS
                    },
                ]
            })
            .collect()
    }

    /// Every text size on a Clara, held either way, and on an Elipsa 2E
    /// upright.
    fn every_size_held_either_way() -> Vec<DisplayMetrics> {
        let mut every = Vec::new();
        for text_scale in TextScale::STEPS {
            let clara = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            every.push(clara);
            every.push(DisplayMetrics {
                width: clara.height,
                height: clara.width,
                ..clara
            });
            every.push(DisplayMetrics {
                text_scale,
                ..ELIPSA_2E_METRICS
            });
        }
        every
    }

    /// Every page of an app's page, as the owner turns through it.
    fn detail_pages_of(store: &mut Store, context: &Context, id: &str) -> Vec<Screen> {
        store.detail_page = None;
        let pages = store.detail_pages(context, id).len();
        (0..pages)
            .map(|page| {
                store.detail_page = Some((id.to_owned(), page));
                store.detail_for(context, id)
            })
            .collect()
    }

    /// Each catalog row's own action and the action of its overflow mark.
    fn row_taps(screen: &Screen) -> Vec<(ActionId, Option<ActionId>)> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_ui::Node::Rows { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .map(|row| (row.action, row.menu))
            .collect()
    }

    fn row_buttons(screen: &Screen) -> Vec<(ActionId, String)> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_ui::Node::Rows { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .filter_map(|row| {
                row.button
                    .as_ref()
                    .map(|button| (button.action, button.label.clone()))
            })
            .collect()
    }

    #[test]
    fn a_catalog_row_offers_install_or_update_and_an_installed_app_opens_from_its_row() {
        let (zebra, zebra_installed) = behind("zebra", &["network"]);
        let (moth, moth_installed) = behind("moth", &[]);
        let mut future = app("future", None);
        future.minimum_cobalt_version = "9.0.0".to_owned();
        let runner = opened_with(
            &[
                app("apple", None),
                app("yak", Some("1.1.0")),
                zebra,
                moth,
                future,
                app("settings", Some("0.3.0")),
            ],
            &[zebra_installed, moth_installed],
        );
        let screen = runner.app().clone_for_test().catalog(&runner.context());
        let buttons = row_buttons(&screen);
        for (action, label) in [
            // Install opens the app's page, where its permissions are.
            (app_action("apple"), "Install"),
            // An update asking for nothing new installs from the row.
            (install_action("zebra"), "Update"),
            // One asking for a new permission opens the page first.
            (app_action("moth"), "Update"),
        ] {
            assert!(
                buttons.contains(&(action_id(&action), label.to_owned())),
                "{action}: {buttons:?}"
            );
        }
        // An installed app with nothing to update has no button: its row
        // opens it, and the mark at its edge leads to its page.
        assert_eq!(buttons.len(), 3, "{buttons:?}");
        let mut later = runner.app().clone_for_test();
        later.page = 1;
        let taps = [screen.clone(), later.catalog(&runner.context())]
            .iter()
            .flat_map(row_taps)
            .collect::<Vec<_>>();
        for id in ["yak", "settings", "zebra"] {
            assert!(
                taps.contains(&(
                    action_id(&open_action(id)),
                    Some(action_id(&app_action(id)))
                )),
                "{id}: {taps:?}"
            );
        }
        assert!(taps.contains(&(action_id(&app_action("apple")), None)));
        assert!(taps.contains(&(action_id(&app_action("future")), None)));
        assert!(!buttons
            .iter()
            .any(|(action, _)| *action == action_id(&install_action("apple"))
                || *action == action_id(&install_action("moth"))
                || *action == action_id(&install_action("future"))));
        assert!(words(&screen, &CLARA_BW_METRICS).contains("Requires Cobalt 9.0.0"));
        // A tap on a row's button is the button, not the row.
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        let rect = layout
            .rect_of_action(action_id(&install_action("zebra")))
            .expect("the button is drawn");
        assert_eq!(
            layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
            Some(action_id(&install_action("zebra")))
        );
    }

    #[test]
    fn install_on_a_row_opens_the_app_page_so_its_permissions_are_read_before_anything_installs() {
        let mut runner = opened(&[app("birds", None), app("notes", None)]);
        runner.action(action_id(Filter::Available.action()));
        let commands = runner.action(action_id(&app_action("birds")));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::InstallApp { .. }))));
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));
        let page = painted(&commands);
        assert!(words(&page, &CLARA_BW_METRICS).contains("Connects to online services"));
        // The page's own Install is the one that installs, and the owner is
        // left on the page it was started from.
        let commands = runner.action(action_id(&install_action("birds")));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::InstallApp { id }) if id == "birds"
        )));
        runner.device_result(DeviceResult::Done);
        assert_eq!(runner.app().view, View::Detail("birds".to_owned()));
        assert_eq!(notice(&runner), Some("birds app is installed."));
    }

    #[test]
    fn an_update_asking_for_nothing_new_installs_from_its_row_and_stays_on_the_list() {
        let (zebra, installed) = behind("zebra", &["network"]);
        let mut runner = opened_with(&[zebra, app("notes", None)], &[installed]);
        runner.action(action_id(Filter::Updates.action()));
        runner.action(action_id(&install_action("zebra")));
        assert!(matches!(
            runner.app().view,
            View::Working {
                from_catalog: true,
                ..
            }
        ));
        let commands = runner.device_result(DeviceResult::Done);
        assert_eq!(runner.app().view, View::Catalog);
        assert_eq!(runner.app().filter, Filter::Updates);
        let screen = painted(&commands);
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            kobo_ui::Node::Banner { level: BannerLevel::Success, text, .. }
                if text == "zebra app is updated."
        )));
    }

    #[test]
    fn an_update_asking_for_a_new_permission_opens_the_app_page_from_its_row() {
        let (moth, installed) = behind("moth", &[]);
        let mut runner = opened_with(&[moth.clone()], &[installed]);
        let screen = runner.app().clone_for_test().catalog(&runner.context());
        assert!(row_buttons(&screen).contains(&(action_id(&app_action("moth")), "Update".into())));
        let commands = runner.action(action_id(&app_action("moth")));
        assert_eq!(runner.app().view, View::Detail("moth".to_owned()));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::InstallApp { .. }))));
        // Until the reader has said what is installed, nobody can say an
        // update asks for nothing new, so the row opens the page.
        let unknown = Store {
            entries: vec![moth],
            ..Store::default()
        };
        let mut unknown = unknown;
        let screen = unknown.catalog(&runner.context());
        assert!(row_buttons(&screen).contains(&(action_id(&app_action("moth")), "Update".into())));
    }

    #[test]
    fn a_failed_update_from_a_row_opens_the_app_with_the_reason_and_try_again() {
        let (zebra, installed) = behind("zebra", &["network"]);
        let mut runner = opened_with(&[zebra], &[installed]);
        runner.action(action_id(&install_action("zebra")));
        runner.device_result(DeviceResult::Failed(DeviceError::TimedOut));
        assert_eq!(runner.app().view, View::Detail("zebra".to_owned()));
        let notice = runner.app().notice.clone().expect("a notice");
        assert_eq!(notice.level, BannerLevel::Error);
        assert_eq!(notice.retry.as_deref(), Some("zebra"));
    }

    #[test]
    fn every_catalog_page_with_row_buttons_fits_at_the_default_and_largest_size_held_either_way() {
        for installed in [None, Some("0.0.1")] {
            let entries = bundled(installed);
            for metrics in sizes_and_poses() {
                for notice in [None, Some(Notice::success("Quality fixture is installed."))] {
                    let runner = AppRunner::with_metrics(Store::default(), metrics);
                    let context = runner.context();
                    let mut store = Store::default();
                    store.replace_entries(entries.clone());
                    store.notice = notice;
                    let mut shown = BTreeSet::new();
                    for page in 0..entries.len() {
                        store.page = page;
                        let screen = store.catalog(&context);
                        let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(!report.has_errors(), "{metrics:?}: {:?}", report.issues);
                        shown.extend(row_taps(&screen).into_iter().map(|(action, _)| action));
                        if store.page < page {
                            break;
                        }
                    }
                    assert_eq!(shown.len(), entries.len(), "{metrics:?}");
                }
            }
        }
    }

    /// Try again on the banner takes a line the bar did not. Wherever an
    /// app's page fitted with a plain banner, it still fits with the failed
    /// install's banner, because the page falls back to the bar where the
    /// extra line does not fit.
    #[test]
    fn a_failed_install_page_fits_at_every_size_and_offers_try_again_on_its_banner() {
        let entries = bundled(None);
        for metrics in sizes_and_poses() {
            let context = AppRunner::with_metrics(Store::default(), metrics).context();
            for error in [DeviceError::Backend, DeviceError::Unreachable] {
                for entry in &entries {
                    let mut store = Store::default();
                    store.replace_entries(entries.clone());
                    store.notice = Some(Notice::retry(app_failure(error), entry.id.clone()));
                    for screen in detail_pages_of(&mut store, &context, &entry.id) {
                        let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(
                            !report.has_errors(),
                            "{} on {metrics:?}: {:?}",
                            entry.id,
                            report.issues
                        );
                        assert!(
                            report
                                .layout
                                .rect_of_action(action_id(&install_action(&entry.id)))
                                .is_some(),
                            "{} on {metrics:?} has no way to try again",
                            entry.id
                        );
                    }
                }
            }
        }
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
        assert!(words(&screen, &CLARA_BW_METRICS).contains("Link a browser"));
    }

    #[test]
    fn linking_a_browser_has_one_name_from_the_list_to_its_own_screen() {
        let mut runner = opened(&[app("apple", None)]);
        let screen = painted(&runner.action(action_id(APP_LINK)));
        assert_eq!(
            screen.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Link a browser")
        );
        let drawn = format!("{screen:?}");
        for other in ["Install from a browser", "Link this Kobo", "Link browser\""] {
            assert!(!drawn.contains(other), "{other}: {drawn}");
        }
    }

    #[test]
    fn a_refresh_that_could_not_reach_the_network_says_what_to_do_in_plain_words() {
        let offline = super::refresh_failure(DeviceError::Unreachable);
        assert!(offline.contains("Connect to Wi-Fi"), "{offline}");
        assert!(!offline.contains("unreachable"), "{offline}");
        assert!(!super::refresh_failure(DeviceError::Backend).contains("Wi-Fi"));
    }

    #[test]
    fn every_bundled_app_page_shows_every_fact_at_every_size_with_or_without_a_banner() {
        for installed in [None, Some("0.0.1")] {
            let entries = bundled(installed);
            for metrics in every_size_held_either_way() {
                // A runner installs the face the runtime measures with.
                let context = AppRunner::with_metrics(Store::default(), metrics).context();
                for entry in &entries {
                    for notice in [
                        None,
                        Some(Notice::retry(
                            app_failure(DeviceError::Backend),
                            entry.id.clone(),
                        )),
                        Some(Notice::attention(app_failure(DeviceError::InvalidInput))),
                    ] {
                        let mut store = Store::default();
                        store.replace_entries(entries.clone());
                        store.notice = notice;
                        let mut facts = 0;
                        let mut said = String::new();
                        for screen in detail_pages_of(&mut store, &context, &entry.id) {
                            let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
                            assert!(
                                !report.has_errors(),
                                "{} on {metrics:?}: {:?}",
                                entry.id,
                                report.issues
                            );
                            facts += report
                                .layout
                                .nodes
                                .iter()
                                .filter(|node| matches!(node.kind, LayoutKind::FactLabel))
                                .count();
                            said.push_str(&words(&screen, &metrics));
                        }
                        assert_eq!(facts, 5, "{} on {metrics:?}: {said}", entry.id);
                        let packed = |text: &str| {
                            text.chars()
                                .filter(|letter| letter.is_alphanumeric())
                                .collect::<String>()
                        };
                        assert!(
                            packed(&said).contains(&packed(&permissions(&entry.capabilities))),
                            "{} on {metrics:?}: {said}",
                            entry.id
                        );
                        let confirm = store.confirm_uninstall_for(&context, &entry.id);
                        let report = confirm.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(
                            !report.has_errors(),
                            "uninstall {} on {metrics:?}: {:?}",
                            entry.id,
                            report.issues
                        );
                    }
                }
            }
        }
    }
}
