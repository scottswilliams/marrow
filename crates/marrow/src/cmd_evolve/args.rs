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
    /// The `--approve-retire` flags as parsed but not yet resolved: each spell names a retire
    /// target — either the human field path (`demo::books::Book::author`) or the internal catalog
    /// id — plus its populated count. The target is resolved to a catalog id once the checked
    /// program is loaded, so the everyday flow can spell the human path while the id form still
    /// works.
    pub(super) retires: Vec<RetireSpec>,
    pub(super) backup: Option<String>,
    pub(super) no_backup: bool,
    pub(super) dir: String,
}

#[derive(Clone)]
pub(super) struct RetireSpec {
    pub(super) target: String,
    pub(super) populated: usize,
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
        retires: parsed.retires,
        backup: parsed.backup,
        no_backup: parsed.no_backup,
        dir: parsed.dir,
    })
}

struct CommonArgs {
    format: CheckFormat,
    dir: String,
    maintenance: bool,
    retires: Vec<RetireSpec>,
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
    let mut retires: Vec<RetireSpec> = Vec::new();
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
                crate::unknown_option(command.name(), value);
                return Err(ParseStop::Usage);
            }
            (_, value) => {
                crate::take_single_target(&mut dir, value, command.name(), "project directory")
                    .map_err(|_| ParseStop::Usage)?;
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
        retires,
        scaffold,
        from_backup,
        backup,
        no_backup,
    })
}

/// Parse one `--approve-retire <target>:<count>` flag into a target spell and its populated count.
/// The target is the field path or catalog id verbatim; it is resolved to a catalog id against the
/// checked program later, so this stage only splits off the trailing count and validates it.
fn parse_retire(value: &str) -> Result<RetireSpec, ParseStop> {
    let Some((target, populated)) = value.rsplit_once(':') else {
        eprintln!("--approve-retire expects <field-path>:<populated-count>");
        return Err(ParseStop::Usage);
    };
    let Ok(populated) = populated.parse::<usize>() else {
        eprintln!("--approve-retire populated count must be a non-negative integer");
        return Err(ParseStop::Usage);
    };
    if target.is_empty() {
        eprintln!("--approve-retire expects <field-path>:<populated-count>");
        return Err(ParseStop::Usage);
    }
    Ok(RetireSpec {
        target: target.to_string(),
        populated,
    })
}
