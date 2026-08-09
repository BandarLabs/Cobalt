//! Cobalt's unprivileged app-store interface.
//!
//! Catalog downloads, signature checks and filesystem changes remain inside
//! `kobod`. This process receives display metadata and submits app identities;
//! it never receives a package URL or chooses an installation path.

use kobo_sdk::{
    action_id, ActionId, AppInfo, Context, DenyReason, DeviceRequest, DeviceResult, Glyph, KoboApp,
    RowLead, Screen, ScreenBuilder,
};
use std::process::ExitCode;

const PAGE_SIZE: usize = 5;
const REFRESH: &str = "refresh";
const PREVIOUS: &str = "previous";
const NEXT: &str = "next";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Catalog,
    Detail(String),
    Working {
        id: String,
        action: &'static str,
    },
}

#[derive(Default)]
struct Store {
    entries: Vec<AppInfo>,
    view: View,
    page: usize,
    refreshing: bool,
    refresh_after_cache: bool,
    notice: Option<String>,
}

impl Store {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view.clone() {
            View::Catalog => self.catalog(),
            View::Detail(id) => self.detail(&id),
            View::Working { id, action } => self.working(&id, action),
        };
        context.set_screen(screen);
    }

    fn catalog(&mut self) -> Screen {
        let pages = self.entries.len().max(1).div_ceil(PAGE_SIZE);
        self.page = self.page.min(pages - 1);
        let mut screen = ScreenBuilder::new("store-catalog")
            .top_bar("App Store")
            .top_bar_glyph(REFRESH, "Refresh", Glyph::Refresh);
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
        let start = self.page * PAGE_SIZE;
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
                self.entries
                    .iter()
                    .skip(start)
                    .take(PAGE_SIZE)
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
            screen = screen.action_bar_marked(actions);
        }
        screen.build()
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
                    "Permissions",
                    if entry.capabilities.is_empty() {
                        "None".to_owned()
                    } else {
                        entry.capabilities.join(", ")
                    },
                ),
            ]);
        screen = if installed.is_some() {
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
        self.page = self
            .page
            .min(self.entries.len().max(1).div_ceil(PAGE_SIZE) - 1);
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
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.view = View::Catalog;
            self.show(context);
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
        let mut refresh_after_paint = false;
        match (request, result) {
            (DeviceRequest::ReadAppCatalog, DeviceResult::Apps { entries }) => {
                self.replace_entries(entries);
                if !matches!(self.view, View::Working { .. }) {
                    self.view = View::Catalog;
                }
                refresh_after_paint = self.refresh_after_cache;
                self.refresh_after_cache = false;
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Apps { entries }) => {
                self.replace_entries(entries);
                self.refreshing = false;
                if !matches!(self.view, View::Working { .. }) {
                    self.notice = None;
                    self.view = View::Catalog;
                }
            }
            (
                DeviceRequest::InstallApp { id } | DeviceRequest::UninstallApp { id },
                DeviceResult::Done,
            ) => {
                self.notice = Some(format!("{id} changed successfully."));
                self.view = View::Catalog;
                context.applications().cached_catalog();
            }
            (DeviceRequest::RefreshAppCatalog, DeviceResult::Failed(error)) => {
                self.refreshing = false;
                self.notice = Some(format!(
                    "The catalog could not be refreshed: {}. The last verified list is still shown.",
                    error.describe()
                ));
                if !matches!(self.view, View::Working { .. }) {
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
    match &entry.installed_version {
        None => "Available".to_owned(),
        Some(version) if entry.has_update() => format!("{version} → {}", entry.version),
        Some(version) => format!("Installed {version}"),
    }
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
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

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
        assert_eq!(requests, vec![&DeviceRequest::ReadAppCatalog]);
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
    fn a_late_refresh_does_not_hide_an_install_in_progress() {
        let mut runner = AppRunner::new(Store::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![app("notes", None)],
        });
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
        let screen = store.catalog();
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
}
