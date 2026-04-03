use crate::application::commands;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "nexa",
    about = "Nexa language toolchain",
    version = env!("NEXA_BUILD_VERSION"),
    arg_required_else_help = true
)]
struct Cli {
    /// Enable debug logs (or set RUST_LOG for fine-grained control)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // ── Project ───────────────────────────────────────────────────────────────
    /// Create a new Nexa project in a new directory
    Init {
        /// Project name (also used as the directory name)
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Project author
        #[arg(long, value_name = "AUTHOR")]
        author: Option<String>,
        /// Initial version (default: 0.1.0)
        #[arg(long, value_name = "VERSION", default_value = "0.1.0")]
        version: String,
        /// Do not initialise a git repository
        #[arg(long)]
        no_git: bool,
    },

    /// Compile the project and start the dev server (also accepts a .nexa bundle)
    Run {
        /// .nexa bundle to run directly (optional)
        #[arg(value_name = "BUNDLE")]
        bundle: Option<PathBuf>,
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Server port (default: 3000)
        #[arg(short, long)]
        port: Option<u16>,
        /// Recompile and reload the browser on every save
        #[arg(long)]
        watch: bool,
    },

    /// Compile the project — writes output to <project>/src/dist/
    Build {
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
    },

    /// Package the project into a distributable .nexa bundle
    Package {
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Output path for the .nexa file (default: <name>.nexa)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    // ── Registry ──────────────────────────────────────────────────────────────
    /// Create an account on the registry
    Register {
        /// Registry URL (default: https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Log in to the registry
    Login {
        /// Registry URL (default: https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Publish the project to the registry
    Publish {
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Registry URL (default: credentials or https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Install dependencies from the registry
    Install {
        /// Package to install, e.g. my-lib or my-lib@1.0.0 (default: all deps from project.json)
        #[arg(value_name = "PACKAGE")]
        package: Option<String>,
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
    },

    /// Search for packages on the registry
    Search {
        /// Search query
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// Registry URL (default: config or https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
        /// Number of results per page (default: 20)
        #[arg(long, default_value = "20")]
        limit: u32,
    },

    /// Show details about a package
    Info {
        /// Package name
        #[arg(value_name = "PACKAGE")]
        package: String,
        /// Registry URL
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    // ── Config ────────────────────────────────────────────────────────────────
    /// Manage global CLI configuration (~/.nexa/config.json)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    // ── Themes ────────────────────────────────────────────────────────────────
    /// Manage CLI themes installed in ~/.nexa/themes/
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },

    // ── Toolchain ─────────────────────────────────────────────────────────────
    /// Update the Nexa CLI to the latest version
    Update {
        /// Update channel (stable | snapshot | latest)
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
    },

    /// Check that the Nexa environment is correctly set up
    Doctor,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show all configuration values
    List,
    /// Show the value of a key
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Set the value of a key
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// List installed themes
    List,
    /// Download and install a theme from the registry
    Add {
        #[arg(value_name = "NAME")]
        name: String,
        /// Registry URL
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    /// Uninstall a theme
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

pub async fn run() {
    let cli = Cli::parse();

    // Initialise the tracing subscriber.
    // Silent by default (WARN) to avoid polluting user output.
    // -v / --verbose → DEBUG. RUST_LOG overrides this if set.
    let default_directive = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(default_directive.parse().expect("valid directive"))
                .from_env_lossy(),
        )
        .with_target(cli.verbose) // show source module only in verbose mode
        .without_time() // no timestamp for a CLI
        .init();

    match cli.command {
        Commands::Init {
            name,
            author,
            version,
            no_git,
        } => commands::init(name, author, version, no_git),

        Commands::Run {
            bundle,
            project,
            port,
            watch,
        } => commands::run(bundle, project, port, watch).await,

        Commands::Build { project } => commands::build(project),

        Commands::Package { project, output } => commands::package(project, output),

        Commands::Register { registry } => commands::register(registry),
        Commands::Login { registry } => commands::login(registry),
        Commands::Publish { project, registry } => commands::publish(project, registry),
        Commands::Install { package, project } => commands::install(package, project),

        Commands::Search {
            query,
            registry,
            limit,
        } => commands::search(query, registry, limit),

        Commands::Info { package, registry } => commands::info(package, registry),

        Commands::Config { action } => match action {
            ConfigAction::List => commands::config_list(),
            ConfigAction::Get { key } => commands::config_get(key),
            ConfigAction::Set { key, value } => commands::config_set(key, value),
        },

        Commands::Theme { action } => match action {
            ThemeAction::List => commands::theme_list(),
            ThemeAction::Add { name, registry } => commands::theme_add(name, registry),
            ThemeAction::Remove { name } => commands::theme_remove(name),
        },

        Commands::Update { channel } => commands::update(channel),

        Commands::Doctor => commands::doctor(),
    }
}
