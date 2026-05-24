mod borrow_check;
mod codegen;
mod commands;
mod compile;
mod fmt;
mod lexer;
mod mangle;
mod mir;
mod parser;
mod semantic;
mod span;
mod tooling;

use clap::{Parser as ClapParser, Subcommand};

#[derive(ClapParser, Debug)]
#[command(name = "pit", version, about = "The Olive programming language toolchain", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    New {
        name: String,
    },
    Build {
        path: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(short, long)]
        time: bool,
        #[arg(long)]
        release: bool,
    },
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
        #[arg(long)]
        hybrid: bool,
        #[arg(long)]
        release: bool,
    },
    Fmt {
        file: Option<String>,
    },
    Test {
        #[arg(short, long)]
        time: bool,
        #[arg(long)]
        release: bool,
    },
    Shell,
    Add {
        pod: String,
    },
    Remove {
        pod: String,
    },
    Install,
    Update {
        pod: Option<String>,
    },
    Publish,
    Upgrade,
}

fn main() {
    let cli = Cli::parse();

    if !matches!(cli.command, Commands::Shell) {
        ctrlc::set_handler(move || {
            std::process::exit(130);
        })
        .expect("Error setting Ctrl-C handler");
    }

    match cli.command {
        Commands::New { name } => commands::project::execute_new(&name),
        Commands::Build {
            path,
            output,
            time,
            release,
        } => commands::build::execute_build(path.as_ref(), output.as_ref(), time, release),
        Commands::Run {
            file,
            time,
            emit_ast,
            emit_mir,
            jit,
            aot,
            hybrid,
            release,
        } => commands::run::execute_run(
            file.as_ref(),
            time,
            emit_ast,
            emit_mir,
            jit,
            aot,
            hybrid,
            release,
        ),
        Commands::Fmt { file } => commands::project::execute_fmt(file.as_ref()),
        Commands::Test { time, release } => commands::build::execute_test(time, release),
        Commands::Shell => commands::project::execute_shell(),
        Commands::Add { pod } => commands::deps::execute_add(&pod),
        Commands::Remove { pod } => commands::deps::execute_remove(&pod),
        Commands::Install => commands::deps::execute_install(),
        Commands::Update { pod } => commands::deps::execute_update(pod.as_ref()),
        Commands::Publish => commands::project::execute_publish(),
        Commands::Upgrade => commands::project::execute_upgrade(),
    }
}
