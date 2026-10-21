use clap::{Parser, Subcommand, ValueEnum};
use pr_consolidate::{
    campaign::Campaign, ci::SystemClock, context, error::Result, forge::Forge, native, plan,
    process::mask, project, transport::NativeTransport,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "pr-consolidate",
    about = "Repository-aware pull request consolidation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Forge {
        #[arg(long)]
        repository: Option<PathBuf>,
        #[command(subcommand)]
        command: ForgeCommand,
    },
    Lot {
        operation: String,
        path: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    Lotenv {
        path: PathBuf,
    },
    Native {
        operation: String,
        path: String,
    },
    ProjectCommands {
        path: PathBuf,
        phase: String,
        directory: PathBuf,
        log: PathBuf,
    },
}
#[derive(Subcommand)]
enum ForgeCommand {
    Context,
    Inventory {
        #[arg(long)]
        out: PathBuf,
    },
    Create {
        branch: String,
        title: String,
        description: PathBuf,
    },
    Desc {
        #[arg(value_parser = pr_id)]
        pr: u64,
        expected: PathBuf,
        new: PathBuf,
        #[arg(long)]
        check: bool,
    },
    WaitCi {
        sha: String,
    },
    Watch {
        #[arg(required = true, value_parser = pr_id)]
        prs: Vec<u64>,
    },
    VerifyMerge {
        #[arg(value_parser = pr_id)]
        pr: u64,
        candidate: String,
        push_clone: PathBuf,
    },
    #[command(alias = "decline")]
    Replace {
        plan: PathBuf,
        mode: Mode,
    },
    ReviewComment {
        #[arg(value_parser = pr_id)]
        pr: u64,
        head: String,
        body: PathBuf,
        mode: Mode,
    },
}
#[derive(Clone, ValueEnum)]
enum Mode {
    DryRun,
    Apply,
}
impl Mode {
    fn as_str(&self) -> &str {
        match self {
            Self::DryRun => "dry-run",
            Self::Apply => "apply",
        }
    }
}
fn pr_id(value: &str) -> std::result::Result<u64, String> {
    if !plan::numeric(value) || value.starts_with('0') {
        return Err("PR ID must be a positive integer".into());
    }
    value.parse().map_err(|_| "PR ID is too large".into())
}
fn run(command: Command) -> Result<i32> {
    match command {
        Command::Lot {
            operation,
            path,
            arguments,
        } => Campaign::new(plan::load(&path)?)?.execute(&operation, &arguments),
        Command::ProjectCommands {
            path,
            phase,
            directory,
            log,
        } => project::run_phase(&plan::read_json(&path)?, &phase, &directory, &log),
        Command::Lotenv { path } => {
            println!("{}", plan::exports(&plan::load(&path)?));
            Ok(0)
        }
        Command::Native { operation, path } => native::execute(&operation, &path),
        Command::Forge {
            repository,
            command,
        } => {
            let directory = match repository {
                Some(path) => path,
                None => std::env::current_dir()?,
            };
            let context = context::discover(&directory)?;
            let forge = Forge {
                transport: NativeTransport {
                    context: context.clone(),
                },
                context,
            };
            match command {
                ForgeCommand::Context => {
                    println!("{}", forge.context.document());
                    Ok(0)
                }
                ForgeCommand::Inventory { out } => forge.inventory(&out),
                ForgeCommand::Create {
                    branch,
                    title,
                    description,
                } => forge.create(&branch, &title, &description),
                ForgeCommand::Desc {
                    pr,
                    expected,
                    new,
                    check,
                } => forge.desc(pr, &expected, &new, check),
                ForgeCommand::WaitCi { sha } => {
                    forge.wait_ci(&sha, &SystemClock::default(), 3600, 60)
                }
                ForgeCommand::Watch { prs } => forge.watch(&prs, &SystemClock::default(), 5400, 60),
                ForgeCommand::VerifyMerge {
                    pr,
                    candidate,
                    push_clone,
                } => forge.verify_merge(pr, &candidate, &push_clone),
                ForgeCommand::Replace { plan, mode } => forge.replace(&plan, mode.as_str()),
                ForgeCommand::ReviewComment {
                    pr,
                    head,
                    body,
                    mode,
                } => forge.review_comment(pr, &head, &body, mode.as_str()),
            }
        }
    }
}
fn main() {
    let cli = Cli::parse();
    let forge = matches!(&cli.command, Command::Forge { .. });
    let code = match run(cli.command) {
        Ok(code) => code,
        Err(error) => {
            let message = format!("{}: {}", error.label(), mask(&error.to_string()));
            if forge {
                println!("{message}");
            } else {
                eprintln!("{message}");
            }
            error.code()
        }
    };
    std::process::exit(code);
}
