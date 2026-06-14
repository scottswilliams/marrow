use marrow_run::evolution::Approval;
use marrow_store::cell::CatalogId;

use crate::CheckFormat;

pub(super) enum ParseStop {
    Help,
    Usage,
}

pub(super) struct PreviewArgs {
    pub(super) format: CheckFormat,
    pub(super) scaffold: bool,
    pub(super) from_backup: Option<String>,
    pub(super) dir: String,
}

pub(super) struct ApplyArgs {
    pub(super) format: CheckFormat,
    pub(super) maintenance: bool,
    pub(super) approval: Option<Approval>,
    pub(super) backup: Option<String>,
    pub(super) no_backup: bool,
    pub(super) dir: String,
}

pub(super) fn preview_args(args: &[String]) -> Result<PreviewArgs, ParseStop> {
    let parsed = common(args, Command::Preview)?;
    Ok(PreviewArgs {
        format: parsed.format,
        scaffold: parsed.scaffold,
        from_backup: parsed.from_backup,
        dir: parsed.dir,
    })
}

pub(super) fn apply_args(args: &[String]) -> Result<ApplyArgs, ParseStop> {
    let parsed = common(args, Command::Apply)?;
    Ok(ApplyArgs {
        format: parsed.format,
        maintenance: parsed.maintenance,
        approval: parsed.approval,
        backup: parsed.backup,
        no_backup: parsed.no_backup,
        dir: parsed.dir,
    })
}

struct CommonArgs {
    format: CheckFormat,
    dir: String,
    maintenance: bool,
    approval: Option<Approval>,
    scaffold: bool,
    from_backup: Option<String>,
    backup: Option<String>,
    no_backup: bool,
}

#[derive(Clone, Copy)]
enum Command {
    Preview,
    Apply,
}

impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Preview => "evolve preview",
            Self::Apply => "evolve apply",
        }
    }
}

fn common(args: &[String], command: Command) -> Result<CommonArgs, ParseStop> {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut maintenance = false;
    let mut scaffold = false;
    let mut from_backup = None;
    let mut backup = None;
    let mut no_backup = false;
    let mut retires: Vec<(CatalogId, usize)> = Vec::new();
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match (command, args[index].as_str()) {
            (_, "--format") => {
                crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)
                    .map_err(|_| ParseStop::Usage)?;
            }
            (Command::Preview, "--scaffold") => scaffold = true,
            (Command::Preview, "--from-backup") => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --from-backup");
                    return Err(ParseStop::Usage);
                };
                if from_backup.replace(value.to_string()).is_some() {
                    eprintln!("duplicate --from-backup");
                    return Err(ParseStop::Usage);
                }
            }
            (Command::Apply, "--backup") => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --backup");
                    return Err(ParseStop::Usage);
                };
                if backup.replace(value.to_string()).is_some() {
                    eprintln!("duplicate --backup");
                    return Err(ParseStop::Usage);
                }
            }
            (Command::Apply, "--no-backup") => {
                if no_backup {
                    eprintln!("duplicate --no-backup");
                    return Err(ParseStop::Usage);
                }
                no_backup = true;
            }
            (Command::Apply, "--maintenance") => maintenance = true,
            (Command::Apply, "--approve-retire") => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --approve-retire");
                    return Err(ParseStop::Usage);
                };
                retires.push(parse_retire(value)?);
            }
            (Command::Preview, "--maintenance" | "--approve-retire") => {
                eprintln!(
                    "{} does not accept apply-only approval flags",
                    command.name()
                );
                return Err(ParseStop::Usage);
            }
            (Command::Preview, "--backup" | "--no-backup") => {
                eprintln!("{} does not accept apply-only backup flags", command.name());
                return Err(ParseStop::Usage);
            }
            (Command::Apply, "--scaffold") => {
                eprintln!(
                    "{} does not accept preview-only scaffold flags",
                    command.name()
                );
                return Err(ParseStop::Usage);
            }
            (Command::Apply, "--from-backup") => {
                eprintln!(
                    "{} does not accept preview-only backup flags",
                    command.name()
                );
                return Err(ParseStop::Usage);
            }
            (_, "--help" | "-h") => {
                super::print_help();
                return Err(ParseStop::Help);
            }
            (_, value) if value.starts_with('-') => {
                eprintln!("unknown {} option: {value}", command.name());
                return Err(ParseStop::Usage);
            }
            (_, value) => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("{} accepts one project directory", command.name());
                    return Err(ParseStop::Usage);
                }
            }
        }
        index += 1;
    }
    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return Err(ParseStop::Usage);
    };
    if backup.is_some() && no_backup {
        eprintln!("--backup and --no-backup are mutually exclusive");
        return Err(ParseStop::Usage);
    }
    Ok(CommonArgs {
        format,
        dir,
        maintenance,
        approval: build_approval(retires),
        scaffold,
        from_backup,
        backup,
        no_backup,
    })
}

/// Fold the repeated `--approve-retire` flags into one approval: each flag names one
/// retired catalog id and its populated count, which admission matches per id against
/// the witness. One approval covers a multi-id destructive evolution, and a wrong count
/// on any single id is rejected even if the counts across ids would sum the same.
fn build_approval(retires: Vec<(CatalogId, usize)>) -> Option<Approval> {
    if retires.is_empty() {
        return None;
    }
    Some(Approval { retires })
}

fn parse_retire(value: &str) -> Result<(CatalogId, usize), ParseStop> {
    let Some((catalog_id, populated)) = value.rsplit_once(':') else {
        eprintln!("--approve-retire expects <catalog-id>:<populated-count>");
        return Err(ParseStop::Usage);
    };
    let Ok(populated) = populated.parse::<usize>() else {
        eprintln!("--approve-retire populated count must be a non-negative integer");
        return Err(ParseStop::Usage);
    };
    let Ok(catalog_id) = CatalogId::new(catalog_id.to_string()) else {
        eprintln!("--approve-retire catalog id is malformed");
        return Err(ParseStop::Usage);
    };
    Ok((catalog_id, populated))
}
