mod borrow_check;
mod codegen;
mod compile;
mod fmt;
mod lexer;
mod mangle;
mod mir;
mod parser;
mod repl;
mod semantic;
mod span;

use clap::{Parser as ClapParser, Subcommand};
use compile::{compile_and_run, compile_and_test};
use fmt::{format_file, walk_and_format};
use repl::run_shell;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process};

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    package: Package,
}

#[derive(Serialize, Deserialize, Debug)]
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
    /// Build the current project (checks for errors)
    Build {
        #[arg(short, long)]
        time: bool,
    },
    Run {
        /// The file to run (optional if in a project)
        file: Option<String>,

        #[arg(short, long)]
        time: bool,

        #[arg(long)]
        emit_ast: bool,

        #[arg(long)]
        emit_mir: bool,
    },
    /// Format the current project or a specific file
    Fmt {
        /// The file to format (optional if in a project)
        file: Option<String>,
    },
    /// Run tests in the current project
    Test {
        #[arg(short, long)]
        time: bool,
    },
    /// Start an interactive shell session
    Shell,
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

            let config = Config {
                package: Package {
                    name: name.clone(),
                    version: "0.1.0".to_string(),
                    entry: "src/main.liv".to_string(),
                },
            };

            let toml = toml::to_string(&config).unwrap();
            fs::write(path.join("pit.toml"), toml).unwrap();

            let main_liv = "fn main():\n    print(\"Hello from Olive!\")\n\nmain()\n";
            fs::write(path.join("src/main.liv"), main_liv).unwrap();

            let gitignore = ".env\n.env.*\n*.secret\n";
            fs::write(path.join(".gitignore"), gitignore).unwrap();

            println!(
                "\x1b[1;32mCreated\x1b[0m binary (application) `{}` package",
                name
            );
        }
        Commands::Build { time } => {
            let config_path = Path::new("pit.toml");
            if !config_path.exists() {
                eprintln!("error: could not find `pit.toml` in current directory");
                process::exit(1);
            }

            let config_str = fs::read_to_string(config_path).unwrap();
            let config: Config = toml::from_str(&config_str).unwrap();

            compile_and_run(&config.package.entry, false, time, false, false);
        }
        Commands::Run {
            file,
            time,
            emit_ast,
            emit_mir,
        } => {
            if let Some(f) = file {
                compile_and_run(&f, true, time, emit_ast, emit_mir);
            } else {
                let config_path = Path::new("pit.toml");
                if !config_path.exists() {
                    eprintln!("error: no file specified and no `pit.toml` found");
                    process::exit(1);
                }

                let config_str = fs::read_to_string(config_path).unwrap();
                let config: Config = toml::from_str(&config_str).unwrap();

                compile_and_run(&config.package.entry, true, time, emit_ast, emit_mir);
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
            let config_path = Path::new("pit.toml");
            if !config_path.exists() {
                eprintln!("error: could not find `pit.toml` in current directory");
                process::exit(1);
            }

            let config_str = fs::read_to_string(config_path).unwrap();
            let config: Config = toml::from_str(&config_str).unwrap();

            compile_and_test(&config.package.entry, time);
        }
        Commands::Shell => {
            run_shell();
        }
    }
}
