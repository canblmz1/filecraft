use std::{io::Write, path::PathBuf, process::ExitCode, sync::Arc};

use clap::{Parser, Subcommand};
use filecraft_core::{
    Control, InstallStage, Recipe, Status, capabilities, inspect, install_video_engine, run,
    video_engine,
};

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
    /// Show or install the FFmpeg video engine shared with the desktop app
    Engine {
        #[command(subcommand)]
        action: EngineAction,
    },
}

#[derive(Subcommand)]
enum EngineAction {
    /// Report where FFmpeg was found and what it supports
    Status,
    /// Download the pinned FFmpeg build into FileCraft's data folder (Windows x64)
    Install,
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

fn cancellable() -> Result<Arc<Control>, ctrlc::Error> {
    let control = Arc::new(Control::default());
    let signal = Arc::clone(&control);
    ctrlc::set_handler(move || signal.cancel())?;
    Ok(control)
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
            let control = cancellable()?;
            let report = run(&inputs, &output, &recipe, &control, |_| {});
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(match report.status {
                Status::Completed => ExitCode::SUCCESS,
                Status::Cancelled => ExitCode::from(130),
                Status::Partial | Status::Failed => ExitCode::FAILURE,
            });
        }
        Command::Capabilities => println!("{}", serde_json::to_string_pretty(&capabilities())?),
        Command::Engine {
            action: EngineAction::Status,
        } => println!("{}", serde_json::to_string_pretty(&video_engine())?),
        Command::Engine {
            action: EngineAction::Install,
        } => {
            let control = cancellable()?;
            let mut shown = None;
            let status = install_video_engine(&control, |progress| {
                let step = (
                    progress.stage,
                    progress.received_bytes * 20 / progress.total_bytes.max(1),
                );
                if shown == Some(step) {
                    return;
                }
                shown = Some(step);
                let stage = match progress.stage {
                    InstallStage::Downloading => "Downloading",
                    InstallStage::Verifying => "Verifying SHA-256",
                    InstallStage::Extracting => "Extracting",
                    InstallStage::Testing => "Testing",
                };
                eprintln!(
                    "{stage} FFmpeg: {:.1} / {:.1} MB",
                    progress.received_bytes as f64 / 1e6,
                    progress.total_bytes as f64 / 1e6
                );
                let _ = std::io::stderr().flush();
            })?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}
