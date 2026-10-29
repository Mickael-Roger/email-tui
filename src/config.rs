use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: Ui,
    pub viewer: Viewer,
    pub delivery: Delivery,
    pub llm: crate::llm::Config,
    pub accounts: Vec<AccountConfig>,
    pub calendar: CalendarConfig,
    pub calendars: CalendarsConfig,
    pub tasks: CalendarsConfig,
    pub contacts: ContactsConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarsConfig {
    pub enabled: bool,
    /// Parent directory whose immediate, non-hidden subdirectories are collections.
    pub path: PathBuf,
    pub sync_command: Vec<String>,
    pub sync_timeout_seconds: u64,
    #[serde(skip)]
    pub collections: Vec<PimCollection>,
}
impl Default for CalendarsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: PathBuf::new(),
            sync_command: vec![],
            sync_timeout_seconds: 120,
            collections: vec![],
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarConfig {
    pub enabled: bool,
    pub collections: Vec<PimCollection>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContactsConfig {
    pub enabled: bool,
    pub books: Vec<PimCollection>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PimCollection {
    pub id: String,
    pub label: String,
    /// A khal/vdirsyncer calendar collection (.ics) or vdir address book (.vcf).
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Delivery {
    pub command: Vec<String>,
    pub config: Option<PathBuf>,
    pub timeout_seconds: u64,
}
impl Default for Delivery {
    fn default() -> Self {
        Self {
            command: vec!["neverest".into()],
            config: None,
            timeout_seconds: 240,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub theme: String,
    /// Accepted for compatibility with early configs; the browser is now one tree.
    pub preview_pane: bool,
    /// Accepted for compatibility; directory rows are always part of Messages.
    pub directory_pane: bool,
}
impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: "k9s-dark".into(),
            preview_pane: false,
            directory_pane: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Viewer {
    pub mode: String,
    pub session_ttl_seconds: u64,
    pub browser_command: Vec<String>,
}
impl Default for Viewer {
    fn default() -> Self {
        Self {
            mode: "auto".into(),
            session_ttl_seconds: 600,
            browser_command: vec!["xdg-open".into(), "{url}".into()],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub id: String,
    pub label: String,
    pub email: String,
    pub store: PathBuf,
    #[serde(default)]
    pub neverest_account: Option<String>,
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = path.unwrap_or_else(|| {
            let user = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                })
                .join("mailtui/config.toml");
            if user.is_file() {
                user
            } else {
                std::env::split_paths(
                    &std::env::var_os("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into()),
                )
                .map(|dir| dir.join("mailtui/config.toml"))
                .find(|path| path.is_file())
                .unwrap_or(user)
            }
        });
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("Read {} (use --demo for fixtures)", path.display()))?,
        )
        .with_context(|| format!("Invalid configuration {}", path.display()))?;
        config.discover_calendars()?;
        if config.tasks.enabled {
            config.tasks.collections = discover_collections(&config.tasks.path, &[], "tasks")?;
        }
        config.validate()?;
        Ok(config)
    }
    fn discover_calendars(&mut self) -> Result<()> {
        if !self.calendars.enabled {
            return Ok(());
        }
        let discovered = discover_collections(
            &self.calendars.path,
            &self.calendar.collections,
            "calendars",
        )?;
        self.calendar.collections.extend(discovered);
        self.calendar.enabled = true;
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        for (kind, config) in [("calendars", &self.calendars), ("tasks", &self.tasks)] {
            if config.enabled && !config.path.is_absolute() {
                bail!("{kind}.path must be absolute");
            }
            if config.sync_timeout_seconds == 0
                || config
                    .sync_command
                    .first()
                    .is_some_and(|s| s.trim().is_empty())
            {
                bail!(
                    "{kind}: sync_command needs an executable and sync_timeout_seconds must be positive"
                );
            }
        }
        self.llm.validate()?;
        if self
            .delivery
            .command
            .first()
            .is_none_or(|command| command.is_empty())
            || self.delivery.timeout_seconds == 0
        {
            bail!(
                "delivery.command needs an executable and delivery.timeout_seconds must be positive"
            );
        }
        if !["k9s-dark", "high-contrast", "no-color"].contains(&self.ui.theme.as_str()) {
            bail!("Unknown UI theme");
        }
        if !["auto", "manual"].contains(&self.viewer.mode.as_str()) {
            bail!("viewer.mode must be auto or manual");
        }
        if !(1..=3600).contains(&self.viewer.session_ttl_seconds) {
            bail!("Viewer TTL must be 1..3600 seconds");
        }
        if self
            .viewer
            .browser_command
            .first()
            .is_none_or(|s| s.is_empty())
            || !self
                .viewer
                .browser_command
                .iter()
                .skip(1)
                .any(|s| s.contains("{url}"))
        {
            bail!("Browser command needs an executable and {{url}} argument");
        }
        let mut ids = std::collections::HashSet::new();
        for a in &self.accounts {
            if a.neverest_account
                .as_ref()
                .is_some_and(|name| name.trim().is_empty())
            {
                bail!("Account {}: neverest_account must not be empty", a.id);
            }
            if a.id.is_empty() || a.id == "all" || !ids.insert(&a.id) {
                bail!("Account IDs must be nonempty, unique, and not the reserved scope 'all'");
            }
            if !a.store.is_absolute() {
                bail!("Account {}: store must be an absolute path", a.id);
            }
        }
        for (kind, enabled, collections) in [
            (
                "calendar",
                self.calendar.enabled,
                &self.calendar.collections,
            ),
            ("contacts", self.contacts.enabled, &self.contacts.books),
            ("tasks", self.tasks.enabled, &self.tasks.collections),
        ] {
            let mut names = std::collections::HashSet::new();
            for collection in collections {
                if collection.id.trim().is_empty()
                    || collection.label.trim().is_empty()
                    || !names.insert(&collection.id)
                {
                    bail!("{kind}: collection IDs and labels must be nonempty and IDs unique");
                }
                if !collection.path.is_absolute() {
                    bail!("{kind} {}: path must be absolute", collection.id);
                }
            }
            if enabled && collections.is_empty() {
                bail!("{kind}.enabled requires at least one configured collection");
            }
        }
        Ok(())
    }
}

fn discover_collections(
    root: &std::path::Path,
    explicit: &[PimCollection],
    kind: &str,
) -> Result<Vec<PimCollection>> {
    if !root.is_absolute() {
        bail!("{kind}.path must be absolute");
    }
    let mut discovered = Vec::new();
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("Read {kind} directory {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("Read {kind} directory {}", root.display()))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .context("Collection directory name must be valid UTF-8")?;
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if !path
            .metadata()
            .with_context(|| format!("Read {}", path.display()))?
            .is_dir()
            || explicit.iter().any(|c| c.path == path)
        {
            continue;
        }
        discovered.push(PimCollection {
            id: name.into(),
            label: name.into(),
            path,
        });
    }
    discovered.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(discovered)
}
