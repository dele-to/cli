use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{ArgAction, Args, Parser, Subcommand};
use deleto::api::{api_url_from_env, OpaqueClient};
use deleto::share::{create_share, delete_share, view_share, CreateOptions, CreatedShare};
use deleto::share_url::parse_share_url;
use serde_json::json;

mod analytics;
mod tui;

const BANNER: &str = "\x1b[1;31m
  ██████╗ ███████╗██╗     ███████╗    ████████╗ ██████╗
  ██╔══██╗██╔════╝██║     ██╔════╝    ╚══██╔══╝██╔═══██╗
  ██║  ██║█████╗  ██║     █████╗         ██║   ██║   ██║
  ██║  ██║██╔══╝  ██║     ██╔══╝         ██║   ██║   ██║
  ██████╔╝███████╗███████╗███████╗ ██╗   ██║   ╚██████╔╝
  ╚═════╝ ╚══════╝╚══════╝╚══════╝ ╚═╝   ╚═╝    ╚═════╝
\x1b[0;31m           encrypt locally · secrets that disappear
\x1b[0m";

const CLOUD_URL: &str = "https://dele.to/cloud";

const AFTER_HELP: &str = "\
Examples:
  deleto 'text here'
  echo 'test' | deleto
  deleto -f ./secret.txt
  deleto --expires 1h --views 1 'deployment token'
  deleto --receipt 'deployment token'
  deleto view 'https://dele.to/view/<id>#<fragment>'
  deleto delete --capability dlt_delete_v1_... https://dele.to/view/<id>
  deleto cloud
  deleto --v

Quotas (rolling hour):
  Anonymous       5 creates · 16 KB  · 1 view    · expires in ≤ 1h
  Free API key   20 creates · 64 KB  · 10 views  · expires in ≤ 24h
  Pro API key   120 creates · 256 KB · 100 views · expires in ≤ 30d

  Create a key in the portal:  https://dele.to/developers
  Sign in, open API Keys, then:
    export DELETO_API_KEY=dlt_v1_...

Dedicated Cloud:
  Isolated instance, your domain, managed for you.
    deleto cloud
    https://dele.to/cloud
    export DELETO_API_URL=https://your-instance.dele.to

Encryption happens on this machine. The API receives only an opaque
payload plus expires_in and max_views. The root secret stays in the
share URL fragment and is never sent to the server.

Environment:
  DELETO_API_URL        API origin (default https://dele.to)
  DELETO_API_KEY        optional dlt_v1_... key for authenticated limits
  DELETO_NO_ANALYTICS   set to 1 to disable anonymous events
";

fn clap_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Red.on_default() | Effects::BOLD)
        .usage(AnsiColor::Red.on_default() | Effects::BOLD)
        .literal(AnsiColor::White.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::BrightBlack.on_default())
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
        .valid(AnsiColor::White.on_default() | Effects::BOLD)
        .invalid(AnsiColor::Red.on_default() | Effects::BOLD)
}

#[derive(Parser, Debug)]
#[command(
    name = "deleto",
    version,
    about = "Share secrets that disappear",
    long_about = "Encrypt a secret on this machine and get a link that expires. \
The server never sees the plaintext.\n\n\
Open the link in a browser or with `deleto view`.\n\n\
Anonymous use is limited. For higher quotas, create an API key at \
https://dele.to/developers.",
    before_help = BANNER,
    after_help = AFTER_HELP,
    styles = clap_styles(),
    disable_help_subcommand = true,
    arg_required_else_help = false,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Print version
    #[arg(short = 'v', long = "v", action = ArgAction::Version)]
    _v: (),

    #[command(subcommand)]
    command: Option<Command>,

    /// Secret to share. Omit to read stdin, or open the TUI on a terminal.
    #[arg(num_args = 0.., value_name = "CONTENT")]
    content: Vec<String>,

    #[command(flatten)]
    share: ShareArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a share from stdin, a file, or an argument
    #[command(visible_alias = "create", after_help = AFTER_HELP)]
    Share {
        /// Secret to share
        #[arg(num_args = 0.., value_name = "CONTENT")]
        content: Vec<String>,
        #[command(flatten)]
        share: ShareArgs,
    },
    /// Decrypt and print a share in the terminal
    View {
        /// Full share URL including the #fragment
        url: String,
        /// Print plaintext only, with no TUI frame
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        api: ApiArgs,
    },
    /// Delete a share with the creator's delete capability
    Delete {
        /// Share URL or share id
        target: String,
        /// dlt_delete_v1_... capability from share creation
        #[arg(long, short = 'c', env = "DELETO_DELETE_CAPABILITY", hide_env_values = true)]
        capability: String,
        #[command(flatten)]
        api: ApiArgs,
    },
    /// Get a dedicated Deleto Cloud instance
    Cloud {
        /// Open https://dele.to/cloud in a browser
        #[arg(long)]
        open: bool,
    },
}

#[derive(Args, Debug, Clone)]
struct ShareArgs {
    /// Read secret from a file instead of an argument or stdin
    #[arg(short, long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// How long the share lives (for example 15m, 1h). Minimum 60s.
    #[arg(short, long, default_value = "1h", value_name = "DURATION")]
    expires: String,
    /// Number of times the share can be viewed
    #[arg(short = 'n', long, default_value_t = 1, value_name = "COUNT")]
    views: u32,
    /// Print JSON instead of the share URL
    #[arg(long)]
    json: bool,
    /// Also print expiry and the private delete capability
    #[arg(short = 'r', long, visible_aliases = ["explain", "details"])]
    receipt: bool,
    /// Open the interactive TUI even when stdin is not a terminal
    #[arg(long)]
    tui: bool,
    #[command(flatten)]
    api: ApiArgs,
}

#[derive(Args, Debug, Clone)]
struct ApiArgs {
    /// Opaque API origin
    #[arg(long, env = "DELETO_API_URL", default_value_t = api_url_from_env())]
    api_url: String,
    /// Optional API key (dlt_v1_...). Create one at https://dele.to/developers
    #[arg(long, env = "DELETO_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
}

fn main() {
    let result = run();
    analytics::flush();
    if let Err(error) = result {
        let _ = writeln!(io::stderr(), "error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse_from(argv_with_implicit_share());
    match cli.command {
        None => dispatch_create(cli.content, cli.share),
        Some(Command::Share { content, share }) => dispatch_create(content, share),
        Some(Command::View { url, raw, api }) => dispatch_view(&url, raw, &api),
        Some(Command::Delete { target, capability, api }) => dispatch_delete(&target, &capability, &api),
        Some(Command::Cloud { open }) => dispatch_cloud(open),
    }
}

fn argv_with_implicit_share() -> Vec<String> {
    with_implicit_share(std::env::args().collect())
}

fn with_implicit_share(mut args: Vec<String>) -> Vec<String> {
    if args.len() >= 2 {
        let first = args[1].as_str();
        if !first.starts_with('-') && !is_subcommand(first) {
            args.insert(1, "share".into());
        }
    }
    args
}

fn is_subcommand(value: &str) -> bool {
    matches!(value, "view" | "delete" | "share" | "create" | "cloud" | "help")
}

fn dispatch_create(content: Vec<String>, args: ShareArgs) -> Result<()> {
    let stdin = io::stdin();
    let stdin_tty = stdin.is_terminal();

    if args.tui || (stdin_tty && args.file.is_none() && content.is_empty()) {
        analytics::track(
            "cli_tui_opened",
            analytics::props(&[("source", json!("tui")), ("command", json!("create"))]),
        );
        return tui::run(args);
    }

    let source = if args.file.is_some() {
        "file"
    } else if !content.is_empty() {
        "arg"
    } else {
        "stdin"
    };
    analytics::track(
        "share_creation_started",
        analytics::props(&[
            ("source", json!("cli")),
            ("input", json!(source)),
            ("max_views", json!(args.views)),
            ("expires", json!(args.expires.clone())),
        ]),
    );

    let plaintext = match read_plaintext(content, args.file.as_ref(), stdin_tty) {
        Ok(value) => value,
        Err(error) => {
            track_cli_error("create", &error.to_string());
            return Err(error);
        }
    };
    if plaintext.trim().is_empty() {
        track_cli_error("create", "empty content");
        bail!("refusing to share empty content");
    }
    match create_from_args(&plaintext, &args) {
        Ok(created) => {
            analytics::track(
                "share_created_successfully",
                analytics::props(&[
                    ("source", json!("cli")),
                    ("input", json!(source)),
                    ("max_views", json!(args.views)),
                    ("expires", json!(args.expires.clone())),
                    ("content_length", json!(plaintext.len())),
                    ("json", json!(args.json)),
                ]),
            );
            write_created(&created, &args)
        }
        Err(error) => {
            track_cli_error("create", &error.to_string());
            Err(error)
        }
    }
}

fn dispatch_view(url: &str, raw: bool, api: &ApiArgs) -> Result<()> {
    match view_share(url, Some(api.api_url.as_str())).context("failed to view share") {
        Ok(viewed) => {
            analytics::track(
                "cli_view_opened",
                analytics::props(&[
                    ("source", json!(if raw || !io::stdout().is_terminal() {
                        "cli"
                    } else {
                        "tui"
                    })),
                    ("raw", json!(raw)),
                    ("content_length", json!(viewed.plaintext.len())),
                    ("remaining_views", json!(viewed.remaining_views)),
                ]),
            );
            if raw || !io::stdout().is_terminal() {
                print!("{}", viewed.plaintext);
                if !viewed.plaintext.ends_with('\n') {
                    println!();
                }
                return Ok(());
            }
            tui::show_viewed(&viewed)
        }
        Err(error) => {
            track_cli_error("view", &error.to_string());
            Err(error)
        }
    }
}

fn dispatch_delete(target: &str, capability: &str, api: &ApiArgs) -> Result<()> {
    let (api_url, id) = if let Ok(link) = parse_share_url(target) {
        (api.api_url.clone(), link.id)
    } else if target.contains("://") {
        let parsed = url::Url::parse(target).context("invalid share URL")?;
        let id = parsed
            .path_segments()
            .and_then(|mut s| s.nth(1))
            .context("URL is not a /view/<id> share")?
            .to_string();
        (api.api_url.clone(), id)
    } else {
        (api.api_url.clone(), target.to_string())
    };
    match delete_share(&api_url, &id, capability).context("failed to delete share") {
        Ok(()) => {
            analytics::track(
                "cli_share_deleted",
                analytics::props(&[("source", json!("cli"))]),
            );
            let _ = writeln!(io::stderr(), "deleted {id}");
            Ok(())
        }
        Err(error) => {
            track_cli_error("delete", &error.to_string());
            Err(error)
        }
    }
}

fn dispatch_cloud(open: bool) -> Result<()> {
    analytics::track(
        "cli_cloud_opened",
        analytics::props(&[("source", json!("cli")), ("open", json!(open))]),
    );
    println!(
        "\
Deleto Cloud is your own isolated instance of dele.to.

  • Dedicated deployment, walled off from other customers
  • Your own domain, managed and monitored for you
  • Same client-side encryption — just your infrastructure

  {CLOUD_URL}

When your instance is ready:
  export DELETO_API_URL=https://your-instance.dele.to
"
    );
    if open {
        match open_browser(CLOUD_URL) {
            Ok(()) => {
                let _ = writeln!(io::stderr(), "opening {CLOUD_URL}");
            }
            Err(error) => {
                let _ = writeln!(io::stderr(), "could not open a browser ({error:#}); open {CLOUD_URL}");
            }
        }
    }
    Ok(())
}

fn open_browser(url: &str) -> Result<()> {
    let status = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(url).status()
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open").arg(url).status()
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .status()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            anyhow::bail!("no browser launcher for this platform")
        }
    }
    .context("failed to launch a browser")?;
    if !status.success() {
        bail!("browser launcher exited with {status}");
    }
    Ok(())
}

fn track_cli_error(command: &str, error: &str) {
    analytics::track(
        "cli_command_failed",
        analytics::props(&[
            ("source", json!("cli")),
            ("command", json!(command)),
            ("reason", json!(analytics::error_reason(error))),
        ]),
    );
}

fn create_from_args(plaintext: &str, args: &ShareArgs) -> Result<CreatedShare> {
    let expires_in = parse_expires(&args.expires)?;
    let client = OpaqueClient::new(&args.api.api_url, args.api.api_key.clone())?;
    create_share(
        &client,
        plaintext,
        CreateOptions {
            expires_in,
            max_views: args.views,
        },
    )
    .context("failed to create share")
}

pub(crate) fn parse_expires(value: &str) -> Result<u64> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(seconds.max(60));
    }
    let duration: Duration = value.parse::<humantime::Duration>()?.into();
    let seconds = duration.as_secs().max(60);
    Ok(seconds)
}

fn read_plaintext(content: Vec<String>, file: Option<&PathBuf>, stdin_tty: bool) -> Result<String> {
    if let Some(path) = file {
        return std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()));
    }
    if !content.is_empty() {
        return Ok(content.join(" "));
    }
    if stdin_tty {
        bail!("no secret provided; pass text, --file, pipe stdin, or run in a terminal for the TUI");
    }
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn write_created(created: &CreatedShare, args: &ShareArgs) -> Result<()> {
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "id": created.id,
                "share_url": created.share_url,
                "expires_at": created.expires_at,
                "delete_capability": created.delete_capability,
            })
        );
        return Ok(());
    }
    println!("{}", created.share_url);
    if args.receipt {
        let _ = writeln!(
            io::stderr(),
            "expires {}\ndelete capability (keep private):\n{}",
            created.expires_at,
            created.delete_capability
        );
    }
    Ok(())
}

pub(crate) fn share_from_tui(plaintext: &str, args: &ShareArgs) -> Result<CreatedShare> {
    create_from_args(plaintext, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn help_text_covers_core_workflows() {
        let mut command = Cli::command();
        let mut buf = Vec::new();
        command.write_long_help(&mut buf).unwrap();
        let help = String::from_utf8(buf).unwrap();
        assert!(help.contains("--file"));
        assert!(help.contains("--expires"));
        assert!(help.contains("view"));
        assert!(help.contains("echo 'test' | deleto"));
        assert!(help.contains("Encryption happens"));
        assert!(help.contains("██████╗"));
        assert!(help.contains("https://dele.to/developers"));
        assert!(help.contains("Anonymous"));
        assert!(help.contains("Free API key"));
        assert!(help.contains("20 creates"));
        assert!(help.contains("Pro API key"));
        assert!(help.contains("120 creates"));
        assert!(help.contains("30d"));
        assert!(help.contains("--v"));
        assert!(help.contains("--receipt"));
        assert!(help.contains("--explain"));
        assert!(help.contains("https://dele.to/cloud"));
        assert!(help.contains("deleto cloud"));
        assert!(!help.contains("DELTO_API_URL"));
        assert!(help.contains("dlt_v1_..."));
        assert!(!help.contains("[env: DELETO_API_KEY="));
    }

    #[test]
    fn cloud_is_a_subcommand() {
        let cli = Cli::try_parse_from(["deleto", "cloud"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Cloud { open: false })));
        let cli = Cli::try_parse_from(["deleto", "cloud", "--open"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Cloud { open: true })));
        let argv = with_implicit_share(vec!["deleto".into(), "cloud".into()]);
        assert_eq!(argv, ["deleto", "cloud"]);
    }

    #[test]
    fn receipt_flag_parses_and_aliases() {
        for flag in ["--receipt", "--explain", "--details", "-r"] {
            let cli = Cli::try_parse_from(["deleto", flag, "secret"]).unwrap();
            assert!(cli.share.receipt, "flag {flag}");
            assert_eq!(cli.content, ["secret"]);
        }
        let quiet = Cli::try_parse_from(["deleto", "secret"]).unwrap();
        assert!(!quiet.share.receipt);
    }

    #[test]
    fn version_flag_accepts_v_alias() {
        use clap::error::ErrorKind;
        for flag in ["-v", "-V", "--v", "--version"] {
            let err = Cli::try_parse_from(["deleto", flag]).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::DisplayVersion, "flag {flag}");
        }
    }

    #[test]
    fn view_is_a_subcommand_not_content() {
        let cli = Cli::try_parse_from(["deleto", "view", "https://dele.to/view/id#frag"]).unwrap();
        assert!(matches!(cli.command, Some(Command::View { .. })));
    }

    #[test]
    fn quoted_text_becomes_a_share_command() {
        let argv = with_implicit_share(vec!["deleto".into(), "text here".into()]);
        assert_eq!(argv, ["deleto", "share", "text here"]);
        let cli = Cli::try_parse_from(argv).unwrap();
        assert!(matches!(cli.command, Some(Command::Share { content, .. }) if content == ["text here"]));
    }

    #[test]
    fn flags_before_text_stay_on_the_default_create_path() {
        let cli = Cli::try_parse_from(["deleto", "--expires", "15m", "deployment token"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.content, ["deployment token"]);
        assert_eq!(cli.share.expires, "15m");
    }

    #[test]
    fn flags_after_the_secret_are_not_content() {
        let key = "dlt_v1_zKbFX_iAO04bNr48_QW2TYak7lPBFYvs272GDILAf4GX3F93XvzvDpkcM6-g";
        let cli = Cli::try_parse_from([
            "deleto",
            "--expires",
            "1h",
            "--views",
            "1",
            "deployment token",
            "--api-key",
            key,
        ])
        .unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.content, ["deployment token"]);
        assert_eq!(cli.share.views, 1);
        assert_eq!(cli.share.expires, "1h");
        assert_eq!(cli.share.api.api_key.as_deref(), Some(key));
    }

    #[test]
    fn share_subcommand_also_keeps_flags_after_the_secret() {
        let key = "dlt_v1_zKbFX_iAO04bNr48_QW2TYak7lPBFYvs272GDILAf4GX3F93XvzvDpkcM6-g";
        let cli = Cli::try_parse_from([
            "deleto",
            "share",
            "deployment token",
            "--expires",
            "1h",
            "--api-key",
            key,
        ])
        .unwrap();
        match cli.command {
            Some(Command::Share { content, share }) => {
                assert_eq!(content, ["deployment token"]);
                assert_eq!(share.expires, "1h");
                assert_eq!(share.api.api_key.as_deref(), Some(key));
            }
            other => panic!("expected share command, got {other:?}"),
        }
    }

    #[test]
    fn parse_expires_accepts_hours_minutes_and_seconds() {
        assert_eq!(parse_expires("1h").unwrap(), 3600);
        assert_eq!(parse_expires("15m").unwrap(), 900);
        assert_eq!(parse_expires("120").unwrap(), 120);
        assert_eq!(parse_expires("10").unwrap(), 60);
    }
}
