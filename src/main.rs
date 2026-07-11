mod borrow_check;
#[cfg(test)]
mod builtin_tests;
mod codegen;
#[cfg(test)]
mod collection_method_tests;
mod commands;
mod compile;
mod diagnostics;
mod eq_tests;
mod fmt;
#[cfg(test)]
mod hash_tests;
#[cfg(test)]
mod iteration_tests;
#[cfg(test)]
mod lambda_tests;
mod lexer;
mod mangle;
mod mir;
#[cfg(test)]
mod narrow_tests;
#[cfg(test)]
mod numeric_underscore_tests;
#[cfg(test)]
mod opt_attr_tests;
mod parser;
#[cfg(test)]
mod power_tests;
#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod repeat_tests;
#[cfg(test)]
mod scalar_attr_tests;
mod semantic;
#[cfg(test)]
mod small_reflex_tests;
mod span;
#[cfg(test)]
mod starred_unpack_tests;
#[cfg(test)]
mod string_method_tests;
#[cfg(test)]
mod test_utils;
mod tooling;
#[cfg(test)]
mod type_alias_tests;

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
        #[arg(long)]
        pgo: Option<String>,
        #[arg(long)]
        explain_copies: bool,
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
        #[arg(long)]
        explain_copies: bool,
    },
    Fmt {
        file: Option<String>,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        diff: bool,
        #[arg(long)]
        stdin: bool,
    },
    Fix {
        file: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Explain {
        code: String,
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
            pgo,
            explain_copies,
        } => commands::build::execute_build(
            path.as_ref(),
            output.as_ref(),
            time,
            release,
            pgo.as_deref(),
            explain_copies,
        ),
        Commands::Run {
            file,
            time,
            emit_ast,
            emit_mir,
            jit,
            aot,
            hybrid,
            release,
            explain_copies,
        } => commands::run::execute_run(
            file.as_ref(),
            time,
            emit_ast,
            emit_mir,
            jit,
            aot,
            hybrid,
            release,
            explain_copies,
        ),
        Commands::Fmt {
            file,
            check,
            diff,
            stdin,
        } => commands::project::execute_fmt(file.as_ref(), check, diff, stdin),
        Commands::Fix { file, dry_run } => commands::fix::execute_fix(file.as_ref(), dry_run),
        Commands::Explain { code } => commands::explain::execute_explain(&code),
        Commands::Test { time, release } => commands::build::execute_test(time, release, false),
        Commands::Shell => commands::project::execute_shell(),
        Commands::Add { pod } => commands::deps::execute_add(&pod),
        Commands::Remove { pod } => commands::deps::execute_remove(&pod),
        Commands::Install => commands::deps::execute_install(),
        Commands::Update { pod } => commands::deps::execute_update(pod.as_ref()),
        Commands::Publish => commands::project::execute_publish(),
        Commands::Upgrade => commands::project::execute_upgrade(),
    }
}
