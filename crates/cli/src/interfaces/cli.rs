use crate::application::commands;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nexa", about = "Nexa language compiler & dev server", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
}

pub async fn run() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { bundle, project, port, watch } => {
            commands::run(bundle, project, port, watch).await
        }
        Commands::Build { project }             => commands::build(project),
        Commands::Package { project, output }   => commands::package(project, output),
    }
}
