use marrow_run::evolution::Approval;
use marrow_store::cell::CatalogId;

use crate::CheckFormat;

pub(super) enum ParseStop {
    Help,
    Usage,
}

pub(super) struct PreviewArgs {
    pub(super) format: CheckFormat,
    pub(super) dir: String,
}

pub(super) struct ApplyArgs {
    pub(super) format: CheckFormat,
    pub(super) maintenance: bool,
    pub(super) approval: Option<Approval>,
    pub(super) dir: String,
}

pub(super) fn preview_args(args: &[String]) -> Result<PreviewArgs, ParseStop> {
    let parsed = common(args, "evolve preview", false)?;
    if parsed.maintenance || parsed.approval.is_some() {
        eprintln!("evolve preview does not accept apply-only approval flags");
        return Err(ParseStop::Usage);
    }
    Ok(PreviewArgs {
        format: parsed.format,
        dir: parsed.dir,
    })
}

pub(super) fn apply_args(args: &[String]) -> Result<ApplyArgs, ParseStop> {
    let parsed = common(args, "evolve apply", true)?;
    Ok(ApplyArgs {
        format: parsed.format,
        maintenance: parsed.maintenance,
        approval: parsed.approval,
        dir: parsed.dir,
    })
}

struct CommonArgs {
    format: CheckFormat,
    dir: String,
    maintenance: bool,
    approval: Option<Approval>,
}

fn common(
    args: &[String],
    command: &str,
    allow_apply_flags: bool,
) -> Result<CommonArgs, ParseStop> {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut maintenance = false;
    let mut retires: Vec<(CatalogId, usize)> = Vec::new();
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                if saw_format {
                    eprintln!("duplicate --format");
                    return Err(ParseStop::Usage);
                }
                saw_format = true;
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return Err(ParseStop::Usage);
                };
                let Some(parsed) = CheckFormat::parse(value) else {
                    eprintln!("unknown {command} format: {value}");
                    return Err(ParseStop::Usage);
                };
                format = parsed;
            }
            "--maintenance" if allow_apply_flags => maintenance = true,
            "--approve-retire" if allow_apply_flags => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --approve-retire");
                    return Err(ParseStop::Usage);
                };
                retires.push(parse_retire(value)?);
            }
            "--maintenance" | "--approve-retire" => {
                eprintln!("{command} does not accept apply-only approval flags");
                return Err(ParseStop::Usage);
            }
            "--help" | "-h" => {
                super::print_help();
                return Err(ParseStop::Help);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown {command} option: {value}");
                return Err(ParseStop::Usage);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("{command} accepts one project directory");
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
    Ok(CommonArgs {
        format,
        dir,
        maintenance,
        approval: build_approval(retires),
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
