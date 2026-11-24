use std::{path::PathBuf, process::ExitCode, sync::Arc};

use clap::{Parser, Subcommand};
use filecraft_core::{Control, Recipe, Status, capabilities, inspect, run};

#[derive(Parser)]
#[command(version, about = "The private workbench for any file")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Inspect {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
    },
    Run {
        #[arg(short, long)]
        recipe: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
    },
    Capabilities,
}

fn main() -> ExitCode {
    match execute() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<ExitCode, Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Inspect { inputs } => {
            let reports = inputs
                .iter()
                .map(|path| inspect(path))
                .collect::<Result<Vec<_>, _>>()?;
            println!("{}", serde_json::to_string_pretty(&reports)?);
        }
        Command::Run {
            recipe,
            output,
            inputs,
        } => {
            let recipe = Recipe::read(&recipe)?;
            let control = Arc::new(Control::default());
            let signal = Arc::clone(&control);
            ctrlc::set_handler(move || signal.cancel())?;
            let report = run(&inputs, &output, &recipe, &control, |_| {});
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(match report.status {
                Status::Completed => ExitCode::SUCCESS,
                Status::Cancelled => ExitCode::from(130),
                Status::Partial | Status::Failed => ExitCode::FAILURE,
            });
        }
        Command::Capabilities => println!("{}", serde_json::to_string_pretty(&capabilities())?),
    }
    Ok(ExitCode::SUCCESS)
}
