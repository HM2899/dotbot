mod attachments;
mod branding;
mod sqlite_storage;
mod accounts;
mod durable;
mod outbox;
mod event_setup;
mod recovery;
mod browser;
mod channels;
mod channel_diagnostics;
mod channel_media;
mod chats;
mod connections;
mod coordination;
mod desktop;
mod desktop_resources;
mod engine;
mod agent_mcp;
mod pi_agent;
mod pi_context;
mod web_tools;
mod features;
mod import;
mod history;
mod local_computer;
mod mcp;
mod mcp_store;
mod media_gen;
mod memory;
mod prompt;
mod providers;
mod routines;
mod secrets;
mod protocol;
mod server;
mod skills;
mod sidebar;
mod skill_sources;
mod skill_command;
mod skill_runtime;
mod learning;
mod relevance;
mod routing;
mod store;
mod team;
mod tasks;
mod workflows;
mod validation;
mod projects;
mod steering;
mod feedback;
mod triggers;
mod admin_auth;
mod tools;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

const BROWSER_COMMAND: &str = "npx -y @playwright/mcp@0.0.83";

#[derive(Parser)]
#[command(name = "dotbot", version, about = "A team of long-lived Bots on native Rust pi-agent, with MCP, skills and shared desktops")]
struct Cli {
    /// Team directory ($DOTBOT_HOME; reuses ~/.teambot if present, otherwise ~/.dotbot)
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Metadata backend; SQLite migrates existing JSON without deleting originals
    #[arg(long, global = true, value_parser = ["sqlite", "json"], default_value = "sqlite")]
    storage: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Set/reset the only administrator's credentials; run while the service is stopped
    AdminPassword {
        /// Administrator username; defaults to admin for new teams, otherwise keeps the current name
        #[arg(long)]
        username: Option<String>,
        /// Read one password line from standard input (for deployment automation)
        #[arg(long)]
        password_stdin: bool,
    },
    /// Start the server and print the address to open (the default)
    Serve {
        #[arg(long, default_value_t = 4870)]
        port: u16,
        /// Public HTTPS origin behind your reverse proxy (first installation opens administrator setup)
        #[arg(long)]
        public_url: Option<String>,
        /// Default workspace (~/dotbot-workspace; reuses existing ~/teambot-workspace)
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Disable the optional standalone browser in --no-desktop mode
        #[arg(long)]
        no_browser: bool,
        /// Show the optional standalone browser as a window (--no-desktop only)
        #[arg(long)]
        headed: bool,
        /// Playwright MCP command for explicit --no-desktop mode
        #[arg(long, default_value = BROWSER_COMMAND)]
        browser_command: String,
        /// Run without the Bots' Linux desktops (on by default when docker works)
        #[arg(long)]
        no_desktop: bool,
        /// The webtop image for the shared team computer (each Bot has an independent screen)
        #[arg(long, default_value = desktop::DEFAULT_IMAGE)]
        desktop_image: String,
        /// Revoke all administrator sessions
        #[arg(long)]
        rotate_login: bool,
    },
    /// The Bot tools as a stdio MCP server, for an external MCP client (needs DOTBOT_URL, DOTBOT_TOKEN, DOTBOT_CONVERSATION)
    Mcp,
    /// Copy a DS Bot team folder (state.json and memory) into the home
    Import { from: PathBuf },
    /// Install or list the team's shared skills
    #[command(subcommand)]
    Skill(SkillCommand),
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Install a skill: a store name, owner/repo[/path], a GitHub address, a local folder,
    /// or a pasted `npx skills add owner/repo --skill name` command (parsed, never run)
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Install {
        #[arg(required = true, num_args = 1..)]
        source: Vec<String>,
    },
    /// List installed skills
    List,
}

fn home_of(cli: &Cli) -> Result<PathBuf> {
    if let Some(home) = &cli.home {
        return Ok(home.clone());
    }
    if let Ok(home) = crate::branding::env("DOTBOT_HOME") {
        return Ok(PathBuf::from(home));
    }
    let user_home = PathBuf::from(std::env::var("HOME").context("HOME is not set")?);
    Ok(branding::default_directory(&user_home, ".dotbot", ".teambot"))
}

fn token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = home_of(&cli)?;
    let storage = cli.storage.clone();
    let command = cli.command.unwrap_or(Command::Serve { port: 4870, public_url: None, workspace: None, no_browser: false, headed: false, browser_command: BROWSER_COMMAND.into(), no_desktop: false, desktop_image: desktop::DEFAULT_IMAGE.into(), rotate_login: false });
    match command {
        Command::AdminPassword { password_stdin, username } => {
            use std::io::{BufRead, Read};
            std::fs::create_dir_all(&home)?;
            use std::os::unix::fs::OpenOptionsExt;
            let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(home.join("server.lock"))?;
            lock.try_lock().context("请先停止服务，再设置或重置管理员密码")?;
            let password = if password_stdin {
                let mut line = String::new();
                std::io::stdin().lock().take(1025).read_line(&mut line)?;
                line.trim_end_matches(['\r', '\n']).to_string()
            } else {
                let password = rpassword::prompt_password("管理员新密码（至少 6 个字符）：")?;
                let confirmation = rpassword::prompt_password("再次输入密码：")?;
                anyhow::ensure!(password == confirmation, "两次输入的密码不一致");
                password
            };
            admin_auth::AdminAuth::set_password(&home, &password, username.as_deref())?;
            println!("管理员登录凭据已设置，旧网页登录已全部撤销。启动服务后在登录页登录。");
            Ok(())
        }
        Command::Mcp => mcp::serve(mcp::Link::from_env()?).await,
        Command::Import { from } => {
            println!("{}", import::import(&from, &home)?);
            Ok(())
        }
        Command::Skill(command) => {
            std::fs::create_dir_all(&home)?;
            let skills = skills::Skills::open(&home)?;
            match command {
                SkillCommand::Install { source } => {
                    let name = skill_command::install(&skills, &source.join(" "), true).await.map_err(|error| anyhow::anyhow!(skill_sources::install_error(&error)))?;
                    println!("Installed {name}. A running dotbot shows it after the page refreshes; Bots can load it from their next turn.");
                }
                SkillCommand::List => {
                    for skill in skills.list() {
                        println!("{}\t{}\t{}", skill["name"].as_str().unwrap_or(""), if skill["enabled"] == true { "enabled" } else { "disabled" }, skill["origin"]["source"].as_str().unwrap_or("local"));
                    }
                }
            }
            Ok(())
        }
        Command::Serve { port, public_url, workspace, no_browser, headed, browser_command, no_desktop, desktop_image, rotate_login } => {
            std::fs::create_dir_all(&home)?;
            use std::os::unix::fs::OpenOptionsExt;
            let instance_lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(home.join("server.lock"))?;
            instance_lock.try_lock().context("此团队目录已有运行中的服务，请先关闭或升级原实例，避免重复写入")?;
            if storage == "sqlite" { sqlite_storage::initialize(&home)?; }
            else { anyhow::ensure!(!home.join(sqlite_storage::FILE).exists(), "此团队已使用 SQLite，不能直接回退到旧 JSON；请保留 --storage sqlite"); }
            if public_url.is_some() {
                anyhow::ensure!(public_url.as_ref().is_some_and(|url| url.starts_with("https://")), "公网地址须使用 HTTPS，请在反向代理配置 TLS");
            }
            let (admin, initial_password) = admin_auth::AdminAuth::initialize(&home)?;
            if rotate_login { admin.revoke_all()?; }
            if let Some(password) = initial_password {
                println!("首次安装：管理员用户名为 admin");
                println!("初始管理员密码（仅首次显示）：{password}");
                println!("请使用初始密码登录并完成安装引导；引导完成前无法访问聊天或桌面。");
            } else if admin.setup_required() {
                println!("请使用首次启动时显示的初始密码完成安装引导；如已遗失，可停止服务后用 admin-password 重置。");
            }
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.with_context(|| format!("port {port} is taken; pass --port"))?;
            let port = listener.local_addr()?.port();
            let url = format!("http://127.0.0.1:{port}");
            let workspace = workspace.unwrap_or_else(|| branding::default_directory(&PathBuf::from(std::env::var("HOME").unwrap_or_default()), "dotbot-workspace", "teambot-workspace"));
            std::fs::create_dir_all(&workspace)?;
            let browsers = (no_desktop && !no_browser).then(|| {
                let command: Vec<String> = browser_command.split_whitespace().map(String::from).collect();
                browser::Browsers::new(browser::BrowserConfig { command, headless: !headed }, home.join("browsers"))
            });
            let public_url = public_url.map(|raw| -> Result<String> {
                let parsed = reqwest::Url::parse(&raw).context("--public-url must be an http(s) origin")?;
                anyhow::ensure!(["https", "http"].contains(&parsed.scheme()) && parsed.host_str().is_some() && parsed.username().is_empty() && parsed.password().is_none() && parsed.query().is_none() && parsed.fragment().is_none() && parsed.path() == "/", "--public-url must be an http(s) origin without credentials, path or query");
                Ok(parsed.origin().ascii_serialization())
            }).transpose()?;
            let cfg = team::Config { home: home.clone(), self_exe: std::env::current_exe()?, url: url.clone(), public_url, internal_token: token(), workspace };
            let desktops = if !no_desktop && desktop::Desktops::available().await {
                Some(desktop::Desktops::new(desktop_image, home.join("desktops")))
            } else {
                if !no_desktop {
                    eprintln!("dotbot: Docker 无法连接，桌面浏览器和应用暂不可用。请启动 Docker 后重启服务。");
                }
                None
            };
            let team = team::Team::open(cfg, browsers, desktops)?;
            team.start_background();
            let login_origin = team.cfg.public_url.as_deref().unwrap_or(&url).to_string();
            let app = server::router(server::Server { team: team.clone(), admin });
            println!("dotbot {} on pi-agent Rust", env!("CARGO_PKG_VERSION"));
            println!("home: {}", home.display());
            println!("storage: {storage}");
            println!("listen: {url}");
            println!("open: {login_origin}/login");
            // SIGUSR2 replaces the service while preserving the shared computer
            // and user-owned windows. Active tasks retain interruption receipts.
            let restarting = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let restart_signal = restarting.clone();
            let (finished_tx, mut finished_rx) = tokio::sync::watch::channel(false);
            let executable = std::env::current_exe()?;
            let shutdown = async move {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
                let mut restart = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()).expect("SIGUSR2 handler");
                let preserve = tokio::select! {
                    _ = tokio::signal::ctrl_c() => false,
                    _ = term.recv() => false,
                    _ = restart.recv() => true,
                };
                restart_signal.store(preserve, std::sync::atomic::Ordering::Relaxed);
                team.shutdown_for_restart(preserve).await;
                let _ = finished_tx.send(true);
            };
            {
                use std::future::IntoFuture;
                let serving = axum::serve(listener, app).with_graceful_shutdown(shutdown).into_future();
                tokio::pin!(serving);
                tokio::select! {
                    result = &mut serving => result?,
                    _ = finished_rx.changed() => {
                        // Desktop websocket clients must not hold an upgrade
                        // open indefinitely after tasks have checkpointed.
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut serving).await;
                    }
                }
            }
            if restarting.load(std::sync::atomic::Ordering::Relaxed) {
                use std::os::unix::process::CommandExt;
                let executable = executable.to_string_lossy().trim_end_matches(" (deleted)").to_string();
                let mut args: Vec<_> = std::env::args_os().skip(1).filter(|arg| arg != "--rotate-login").collect();
                for i in 0..args.len() {
                    if args[i] == "--port=0" { args[i] = format!("--port={port}").into(); }
                    if i > 0 && args[i-1] == "--port" && args[i] == "0" { args[i] = port.to_string().into(); }
                }
                let error = std::process::Command::new(executable).args(args).exec();
                return Err(error).context("升级重启失败；共享电脑及浏览器仍然保留，请重新启动服务");
            }
            Ok(())
        }
    }
}
