use anyhow::{Context, Result, bail};

pub const COMMANDS: &[&str] = &[
    "ai",
    "accounts",
    "folders",
    "inbox",
    "unified",
    "mail",
    "search",
    "unread",
    "flagged",
    "outbox",
    "status",
    "account",
    "ns",
    "folder",
    "sort",
    "theme",
    "help",
    "history",
    "reload",
    "more",
    "quit",
    "directories",
    "directory",
    "dirs",
    "dir",
    "compose",
    "mkdir",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Ai(String),
    View(&'static str),
    Account(String),
    Folder(String),
    Search(String),
    Sort(String, bool),
    Theme(String),
    Reload,
    More,
    Quit,
}
pub fn parse(input: &str) -> Result<Command> {
    let input = input.trim().trim_start_matches(':');
    if let Some(query) = input.strip_prefix("search ") {
        if query.trim().is_empty() {
            bail!("Usage: :search <query>");
        }
        return Ok(Command::Search(query.trim().into()));
    }
    let words = shell_words::split(input).context("Unclosed quote or invalid escape in command")?;
    let name = words
        .first()
        .context("Enter a command; Tab completes it")?
        .as_str();
    let arg = || words.get(1).cloned().context("Missing command argument");
    let c = match name {
        "ai" => Command::Ai(words.get(1).cloned().unwrap_or_default()),
        "acc" | "accounts" | "account" | "ns" if words.len() == 1 => Command::View("accounts"),
        "acc" | "accounts" | "account" | "ns" => Command::Account(arg()?),
        "fo" | "folders" => Command::View("folders"),
        "directory" | "directories" | "dirs" | "dir" if words.len() == 1 => {
            Command::View("folders")
        }
        "directory" | "dir" => Command::Folder(arg()?),
        "inbox" => Command::View("inbox"),
        "all" | "unified" => Command::View("unified"),
        "messages" | "mail" => Command::View("mail"),
        "unread" => Command::View("unread"),
        "flagged" => Command::View("flagged"),
        "outbox" => Command::View("outbox"),
        "status" => Command::View("status"),
        "help" => Command::View("help"),
        "history" => Command::View("history"),
        "folder" => Command::Folder(arg()?),
        "sort" => {
            let field = arg()?;
            if !["date", "from", "subject", "size", "account", "status"].contains(&field.as_str()) {
                bail!("Sort fields: date, from, subject, size, account, status");
            }
            if words
                .get(2)
                .is_some_and(|s| !["asc", "desc"].contains(&s.as_str()))
            {
                bail!("Sort direction: asc or desc");
            }
            Command::Sort(field, words.get(2).is_none_or(|s| s == "desc"))
        }
        "theme" => {
            let theme = arg()?;
            if !["k9s-dark", "high-contrast", "no-color"].contains(&theme.as_str()) {
                bail!("Themes: k9s-dark, high-contrast, no-color");
            }
            Command::Theme(theme)
        }
        "reload" => Command::Reload,
        "more" => Command::More,
        "q" | "quit" => Command::Quit,
        "compose" => Command::View("compose"),
        "mkdir" => Command::View("mkdir"),
        "search-clear" | "search-rebuild" => {
            bail!("The local index is maintained automatically; use / or :search <query>")
        }
        _ => bail!("Unknown command. Press ? for help or Tab for completion"),
    };
    let max = match c {
        Command::Ai(_) => 2,
        Command::Account(_) | Command::Folder(_) | Command::Theme(_) => 2,
        Command::Sort(..) => 3,
        _ => 1,
    };
    if words.len() > max {
        bail!("Too many arguments; quote folder/account names containing spaces");
    }
    Ok(c)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_quotes_and_rejection() {
        assert!(parse("sync").is_err());
        assert!(parse("sync all").is_err());
        assert!(!COMMANDS.contains(&"sync"));
        assert_eq!(parse("fo").unwrap(), Command::View("folders"));
        assert_eq!(parse("directories").unwrap(), Command::View("folders"));
        assert_eq!(parse("dir").unwrap(), Command::View("folders"));
        assert_eq!(
            parse(r#"dir "Projects/Rust mail""#).unwrap(),
            Command::Folder("Projects/Rust mail".into())
        );
        assert_eq!(parse("ns").unwrap(), Command::View("accounts"));
        assert_eq!(parse("ns work").unwrap(), Command::Account("work".into()));
        assert_eq!(parse("ns all").unwrap(), Command::Account("all".into()));
        assert!(parse("ns work extra").is_err());
        assert_eq!(
            parse(r#"folder "Projects/Rust mail""#).unwrap(),
            Command::Folder("Projects/Rust mail".into())
        );
        assert_eq!(
            parse(r"folder Projects/Rust\ mail").unwrap(),
            Command::Folder("Projects/Rust mail".into())
        );
        assert!(parse("folder 'broken").is_err());
        assert!(parse("sort typo").is_err());
        assert!(parse("q extra").is_err());
    }
}
