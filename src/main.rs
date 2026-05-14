mod borrow_check;
mod codegen;
mod compile;
mod fmt;
mod lexer;
mod mangle;
mod mir;
mod packages;
mod parser;
mod publish;
mod registry;
mod repl;
mod semantic;
mod span;
mod upgrade;

use clap::{Parser as ClapParser, Subcommand};
use compile::{
    compile_and_emit, compile_and_run, compile_and_run_aot, compile_and_test, compile_hybrid,
};
use fmt::{format_file, walk_and_format};
use repl::run_shell;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{fs, path::Path, process};

#[derive(Serialize, Deserialize, Debug, Default)]
struct Config {
    package: Package,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    dependencies: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct Package {
    name: String,
    version: String,
    #[serde(default = "default_entry")]
    entry: String,
}

fn default_entry() -> String {
    "src/main.liv".to_string()
}

#[derive(ClapParser, Debug)]
#[command(name = "pit", version, about = "The Olive programming language toolchain", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new olive project
    New { name: String },
    /// Build the current project
    Build {
        #[arg(short, long)]
        time: bool,
    },
    /// Run the project or a file
    Run {
        file: Option<String>,
        #[arg(short, long)]
        time: bool,
        #[arg(long)]
        emit_ast: bool,
        #[arg(long)]
        emit_mir: bool,
        #[arg(long)]
        jit: bool,
        #[arg(long)]
        aot: bool,
    },
    /// Format the project or a file
    Fmt { file: Option<String> },
    /// Run tests
    Test {
        #[arg(short, long)]
        time: bool,
    },
    /// Start an interactive shell
    Shell,
    /// Add a package dependency
    Add {
        /// Package name, optionally with version: name or name@1.0.0
        package: String,
    },
    /// Remove a package dependency
    Remove { package: String },
    /// Install all dependencies
    Install,
    /// Update dependencies to their latest versions
    Update {
        /// Update only this package (optional)
        package: Option<String>,
    },
    /// Publish this package to the registry
    Publish,
    /// Update pit to the latest release
    Upgrade,
}

fn load_config() -> Config {
    let config_path = Path::new("pit.toml");
    if !config_path.exists() {
        eprintln!("error: could not find `pit.toml` in current directory");
        process::exit(1);
    }
    let content = fs::read_to_string(config_path).unwrap();
    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("error: invalid pit.toml: {}", e);
        process::exit(1);
    })
}

fn save_config(config: &Config) {
    let content = toml::to_string(config).unwrap();
    fs::write("pit.toml", content).unwrap();
}

fn maybe_install_deps(deps: &HashMap<String, String>) {
    if deps.is_empty() {
        return;
    }
    if let Err(e) = packages::ensure_deps_installed(deps) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::New { name } => {
            let path = Path::new(&name);
            if path.exists() {
                eprintln!("error: directory `{}` already exists", name);
                process::exit(1);
            }

            fs::create_dir_all(path.join("src")).unwrap();
            fs::create_dir_all(path.join(".pit_modules")).unwrap();

            let config = Config {
                package: Package {
                    name: name.clone(),
                    version: "0.1.0".to_string(),
                    entry: "src/main.liv".to_string(),
                },
                dependencies: HashMap::new(),
            };

            fs::write(path.join("pit.toml"), toml::to_string(&config).unwrap()).unwrap();
            fs::write(
                path.join("src/main.liv"),
                "fn main():\n    print(\"Hello from Olive!\")\n\nmain()\n",
            )
            .unwrap();
            fs::write(
                path.join(".gitignore"),
                ".env\n.env.*\n*.secret\ntarget/\n.pit_modules/\n",
            )
            .unwrap();

            println!(
                "\x1b[1;32mCreated\x1b[0m binary (application) `{}` package",
                name
            );
        }

        Commands::Build { time } => {
            let config = load_config();
            maybe_install_deps(&config.dependencies);
            let out = format!("target/{}", config.package.name);
            compile_and_emit(&config.package.entry, &out, time);
        }

        Commands::Run {
            file,
            time,
            emit_ast,
            emit_mir,
            jit,
            aot,
        } => {
            let entry = if let Some(f) = file {
                f
            } else {
                let config = load_config();
                maybe_install_deps(&config.dependencies);
                config.package.entry
            };

            if jit || emit_ast || emit_mir {
                compile_and_run(&entry, true, time, emit_ast, emit_mir);
            } else if aot {
                compile_and_run_aot(&entry, time);
            } else {
                compile_hybrid(&entry, time);
            }
        }

        Commands::Fmt { file } => {
            if let Some(f) = file {
                let path = Path::new(&f);
                if path.is_dir() {
                    walk_and_format(path);
                } else {
                    format_file(&f);
                }
            } else {
                let config_path = Path::new("pit.toml");
                if config_path.exists() {
                    walk_and_format(Path::new("."));
                } else {
                    eprintln!("error: no file specified and no `pit.toml` found");
                    process::exit(1);
                }
            }
        }

        Commands::Test { time } => {
            let config = load_config();
            maybe_install_deps(&config.dependencies);
            compile_and_test(&config.package.entry, time);
        }

        Commands::Shell => {
            run_shell();
        }

        Commands::Add { package } => {
            let (name, version_req) = if let Some((n, v)) = package.split_once('@') {
                (n.to_string(), v.to_string())
            } else {
                (package.clone(), "latest".to_string())
            };

            let versions = registry::fetch_versions(&name).unwrap_or_else(|e| {
                eprintln!("error: {}", e);
                process::exit(1);
            });

            let pkg = registry::resolve_version(&versions, &version_req).unwrap_or_else(|| {
                eprintln!("error: no matching version for '{}@{}'", name, version_req);
                process::exit(1);
            });

            let resolved_version = pkg.vers.clone();
            let pkg = pkg.clone();

            if let Err(e) = packages::download_and_install(&pkg) {
                eprintln!("error: {}", e);
                process::exit(1);
            }
            if let Err(e) = packages::copy_to_modules(&pkg.name, &pkg.vers) {
                eprintln!("error: {}", e);
                process::exit(1);
            }

            let mut config = load_config();
            config
                .dependencies
                .insert(name.clone(), resolved_version.clone());
            save_config(&config);

            println!(
                "\x1b[1;32m    Added\x1b[0m {}@{}",
                name, resolved_version
            );
        }

        Commands::Remove { package } => {
            let mut config = load_config();
            if config.dependencies.remove(&package).is_none() {
                eprintln!("error: '{}' is not a dependency", package);
                process::exit(1);
            }
            save_config(&config);
            packages::remove_from_modules(&package);
            println!("\x1b[1;32m  Removed\x1b[0m {}", package);
        }

        Commands::Install => {
            let config = load_config();
            if config.dependencies.is_empty() {
                println!("No dependencies to install.");
                return;
            }
            if let Err(e) = packages::install_all_deps(&config.dependencies) {
                eprintln!("error: {}", e);
                process::exit(1);
            }
            println!("\x1b[1;32m   Installed\x1b[0m all dependencies");
        }

        Commands::Update { package } => {
            let mut config = load_config();
            if config.dependencies.is_empty() {
                println!("No dependencies to update.");
                return;
            }

            let targets: Vec<String> = if let Some(name) = package {
                if !config.dependencies.contains_key(&name) {
                    eprintln!("error: '{}' is not a dependency", name);
                    process::exit(1);
                }
                vec![name]
            } else {
                config.dependencies.keys().cloned().collect()
            };

            let mut updated = 0;
            for name in &targets {
                let current = config.dependencies[name].clone();
                let versions = registry::fetch_versions(name).unwrap_or_else(|e| {
                    eprintln!("error: {}", e);
                    process::exit(1);
                });
                let latest = match registry::resolve_version(&versions, "latest") {
                    Some(v) => v.clone(),
                    None => {
                        eprintln!("warning: no available version for '{}'", name);
                        continue;
                    }
                };
                if latest.vers == current {
                    println!("  {} already at {}", name, current);
                    continue;
                }
                if let Err(e) = packages::download_and_install(&latest) {
                    eprintln!("error: {}", e);
                    process::exit(1);
                }
                if let Err(e) = packages::copy_to_modules(&latest.name, &latest.vers) {
                    eprintln!("error: {}", e);
                    process::exit(1);
                }
                println!(
                    "\x1b[1;32m  Updated\x1b[0m {} {} → {}",
                    name, current, latest.vers
                );
                config.dependencies.insert(name.clone(), latest.vers);
                updated += 1;
            }

            if updated > 0 {
                save_config(&config);
            }
        }

        Commands::Publish => {
            let config = load_config();
            if let Err(e) = publish::publish(&config.package.name, &config.package.version) {
                eprintln!("error: {}", e);
                process::exit(1);
            }
        }

        Commands::Upgrade => {
            if let Err(e) = upgrade::upgrade() {
                eprintln!("error: {}", e);
                process::exit(1);
            }
        }
    }
}
