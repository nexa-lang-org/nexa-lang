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
    /// Active les logs de debug (ou définir RUST_LOG pour un contrôle fin)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // ── Project ───────────────────────────────────────────────────────────────
    /// Crée un nouveau projet Nexa dans un nouveau répertoire
    Init {
        /// Nom du projet (et du répertoire à créer)
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Auteur du projet
        #[arg(long, value_name = "AUTHOR")]
        author: Option<String>,
        /// Version initiale (défaut : 0.1.0)
        #[arg(long, value_name = "VERSION", default_value = "0.1.0")]
        version: String,
        /// Ne pas initialiser un dépôt git
        #[arg(long)]
        no_git: bool,
    },

    /// Compile le projet et démarre le dev server (accepte aussi un bundle .nexa)
    Run {
        /// Fichier .nexa à exécuter directement (optionnel)
        #[arg(value_name = "BUNDLE")]
        bundle: Option<PathBuf>,
        /// Répertoire racine du projet (défaut : répertoire courant)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Port du serveur (défaut : 3000)
        #[arg(short, long)]
        port: Option<u16>,
        /// Recompile et recharge le navigateur à chaque sauvegarde
        #[arg(long)]
        watch: bool,
    },

    /// Compile le projet — écrit la sortie dans <project>/src/dist/
    Build {
        /// Répertoire racine du projet (défaut : répertoire courant)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
    },

    /// Empaquète le projet dans un bundle distribuable .nexa
    Package {
        /// Répertoire racine du projet (défaut : répertoire courant)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Chemin de sortie du fichier .nexa (défaut : <name>.nexa)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    // ── Registry ──────────────────────────────────────────────────────────────
    /// Crée un compte sur le registry
    Register {
        /// URL du registry (défaut : https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Connexion au registry
    Login {
        /// URL du registry (défaut : https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Publie le projet sur le registry
    Publish {
        /// Répertoire racine du projet (défaut : répertoire courant)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// URL du registry (défaut : credentials ou https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    /// Installe des dépendances depuis le registry
    Install {
        /// Package à installer, ex: my-lib ou my-lib@1.0.0 (défaut : toutes les deps de project.json)
        #[arg(value_name = "PACKAGE")]
        package: Option<String>,
        /// Répertoire racine du projet (défaut : répertoire courant)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
    },

    /// Cherche des packages sur le registry
    Search {
        /// Terme de recherche
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// URL du registry (défaut : config ou https://registry.nexa-lang.org)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
        /// Nombre de résultats par page (défaut : 20)
        #[arg(long, default_value = "20")]
        limit: u32,
    },

    /// Affiche les détails d'un package
    Info {
        /// Nom du package
        #[arg(value_name = "PACKAGE")]
        package: String,
        /// URL du registry
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },

    // ── Config ────────────────────────────────────────────────────────────────
    /// Gère la configuration globale du CLI (~/.nexa/config.json)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    // ── Themes ────────────────────────────────────────────────────────────────
    /// Gère les thèmes CLI installés dans ~/.nexa/themes/
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },

    // ── Toolchain ─────────────────────────────────────────────────────────────
    /// Met à jour le CLI Nexa vers la dernière version
    Update {
        /// Canal de mise à jour (stable | snapshot | latest)
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
    },

    /// Vérifie que l'environnement Nexa est correctement configuré
    Doctor,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Affiche toutes les valeurs de configuration
    List,
    /// Affiche la valeur d'une clé
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Définit la valeur d'une clé
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// Liste les thèmes installés
    List,
    /// Télécharge et installe un thème depuis le registry
    Add {
        #[arg(value_name = "NAME")]
        name: String,
        /// URL du registry
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    /// Désinstalle un thème
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

pub async fn run() {
    let cli = Cli::parse();

    // Initialise le subscriber tracing.
    // Par défaut silencieux (WARN) pour ne pas polluer la sortie utilisateur.
    // -v / --verbose → DEBUG. RUST_LOG prend le dessus si défini.
    let default_directive = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(default_directive.parse().expect("valid directive"))
                .from_env_lossy(),
        )
        .with_target(cli.verbose) // affiche le module source seulement en verbose
        .without_time() // pas de timestamp pour un CLI
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
