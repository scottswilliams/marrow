use std::fmt;

pub const PARSE_SYNTAX: &str = "parse.syntax";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    pub file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedSource {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceFile {
    pub module: Option<ModuleDecl>,
    pub uses: Vec<UseDecl>,
    pub declarations: Vec<Declaration>,
}

impl SourceFile {
    pub fn resource(&self, name: &str) -> Option<&ResourceDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) if resource.name == name => Some(resource),
                _ => None,
            })
    }

    pub fn function(&self, name: &str) -> Option<&FunctionDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == name => Some(function),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    Const(ConstDecl),
    Resource(ResourceDecl),
    Function(FunctionDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub store: Option<SavedRoot>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoot {
    pub root: String,
    pub keys: Vec<KeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMember {
    Field(FieldDecl),
    Group(GroupDecl),
    Index(IndexDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub required: bool,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub ty: TypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub args: Vec<String>,
    pub unique: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub return_type: Option<TypeRef>,
    pub body: SourceSpan,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    pub mode: Option<ParamMode>,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub kind: &'static str,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub span: SourceSpan,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}: {}",
            self.line,
            self.column,
            self.severity.as_str(),
            self.code,
            self.message
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

pub fn parse_source(source: &str) -> ParsedSource {
    Parser::new(source).parse()
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    index: usize,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    number: u32,
    start_byte: usize,
    end_byte: usize,
    text: &'a str,
    indent: usize,
    content: &'a str,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            lines: split_lines(source),
            index: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse(mut self) -> ParsedSource {
        let mut file = SourceFile::default();
        let mut docs = Vec::new();
        let mut saw_top_level_item = false;

        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if self.reject_tabs(line) {
                self.index += 1;
                continue;
            }
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if let Some(doc) = line.doc_comment() {
                docs.push(doc.to_string());
                self.index += 1;
                continue;
            }
            if line.indent != 0 {
                self.error(line, "expected a top-level declaration");
                self.index += 1;
                continue;
            }

            let content = line.content;
            if let Some(rest) = content.strip_prefix("module ") {
                if saw_top_level_item {
                    self.error(
                        line,
                        "module declaration must appear once at the start of the file",
                    );
                } else {
                    let name = rest.trim();
                    if is_qualified_name(name) {
                        file.module = Some(ModuleDecl {
                            name: name.to_string(),
                            span: line.span(),
                        });
                    } else {
                        self.error(line, "expected qualified module name");
                    }
                }
                saw_top_level_item = true;
                docs.clear();
                self.index += 1;
            } else if let Some(rest) = content.strip_prefix("use ") {
                let name = rest.trim();
                if is_qualified_name(name) {
                    file.uses.push(UseDecl {
                        name: name.to_string(),
                        span: line.span(),
                    });
                } else {
                    self.error(line, "expected qualified import name");
                }
                saw_top_level_item = true;
                docs.clear();
                self.index += 1;
            } else if content.starts_with("const ") {
                let declaration = self.parse_const(line, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Const(declaration));
                saw_top_level_item = true;
                self.index += 1;
            } else if content.starts_with("resource ") {
                let resource = self.parse_resource(line, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Resource(resource));
                saw_top_level_item = true;
            } else if starts_function(content)
                || content.starts_with("internal fn ")
                || content.starts_with("private fn ")
            {
                let function = self.parse_function(line, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Function(function));
                saw_top_level_item = true;
            } else {
                self.error(
                    line,
                    "expected module, use, const, resource, or fn declaration",
                );
                docs.clear();
                saw_top_level_item = true;
                self.index += 1;
            }
        }

        ParsedSource {
            file,
            diagnostics: self.diagnostics,
        }
    }

    fn parse_const(&mut self, line: Line<'a>, docs: Vec<String>) -> ConstDecl {
        let rest = line.content["const ".len()..].trim();
        let (head, value) = match split_once_trimmed(rest, '=') {
            Some((head, value)) if !value.is_empty() => (head, value),
            Some((head, _)) => {
                self.error(line, "const declarations require a value after `=`");
                (head, "")
            }
            None => {
                self.error(line, "const declarations require `=` and a value");
                (rest, "")
            }
        };
        let (name, ty) = parse_name_type(head);
        if !is_identifier(name) {
            self.error(line, "expected const name before type annotation");
        }
        if ty.is_some_and(|ty| !is_type_text(ty)) {
            self.error(line, "expected const type annotation");
        }
        ConstDecl {
            docs,
            name: name.to_string(),
            ty: ty.filter(|ty| is_type_text(ty)).map(type_ref),
            value: value.to_string(),
            span: line.span(),
        }
    }

    fn parse_resource(&mut self, line: Line<'a>, docs: Vec<String>) -> ResourceDecl {
        let (name, store) = match parse_resource_header(line.content) {
            Ok(header) => header,
            Err(message) => {
                self.error(line, message);
                ("".to_string(), None)
            }
        };
        self.index += 1;
        let members = if self.has_child_body(line.indent) {
            self.parse_resource_members(line.indent)
        } else {
            self.error(line, "expected an indented resource body");
            Vec::new()
        };

        ResourceDecl {
            docs,
            name,
            store,
            members,
            span: line.span(),
        }
    }

    fn parse_resource_members(&mut self, parent_indent: usize) -> Vec<ResourceMember> {
        let mut members = Vec::new();
        let mut docs = Vec::new();
        let mut stable_id = None;
        let Some(block_indent) = self.resource_block_indent(parent_indent) else {
            return members;
        };

        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if self.reject_tabs(line) {
                self.index += 1;
                continue;
            }
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if line.indent <= parent_indent {
                break;
            }
            if line.indent != block_indent {
                self.error(
                    line,
                    "unexpected indentation in resource body; only groups introduce nested resource members",
                );
                self.index += 1;
                self.skip_deeper_resource_lines(line.indent);
                continue;
            }
            if let Some(doc) = line.doc_comment() {
                docs.push(doc.to_string());
                self.index += 1;
                continue;
            }
            if line.content.starts_with("@id(") {
                stable_id = parse_stable_id(line.content).or_else(|| {
                    self.error(line, "expected @id(\"stable.id\")");
                    None
                });
                self.index += 1;
                continue;
            }

            if line.content.starts_with("index ") {
                match parse_index(line.content) {
                    Ok(index) => members.push(ResourceMember::Index(IndexDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        span: line.span(),
                        ..index
                    })),
                    Err(message) => self.error(line, message),
                }
                self.index += 1;
                continue;
            }

            match parse_field_or_group_head(line.content) {
                Ok(MemberHead::Field {
                    required,
                    name,
                    keys,
                    ty,
                }) => {
                    if !is_type_text(&ty.text) {
                        self.error(line, "expected field type annotation");
                    }
                    members.push(ResourceMember::Field(FieldDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        required,
                        name,
                        keys,
                        ty,
                        span: line.span(),
                    }));
                    self.index += 1;
                }
                Ok(MemberHead::Group { name, keys }) => {
                    self.index += 1;
                    let children = if self.has_child_body(line.indent) {
                        self.parse_resource_members(line.indent)
                    } else {
                        self.error(line, "expected an indented resource group body");
                        Vec::new()
                    };
                    members.push(ResourceMember::Group(GroupDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        name,
                        keys,
                        members: children,
                        span: line.span(),
                    }));
                }
                Err(message) => {
                    self.error(line, message);
                    self.index += 1;
                }
            }
        }

        members
    }

    fn resource_block_indent(&self, parent_indent: usize) -> Option<usize> {
        let mut index = self.index;
        while index < self.lines.len() {
            let line = self.lines[index];
            if line.is_blank() || line.is_comment() {
                index += 1;
                continue;
            }
            if line.indent <= parent_indent {
                return None;
            }
            return Some(line.indent);
        }
        None
    }

    fn skip_deeper_resource_lines(&mut self, bad_indent: usize) {
        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if line.indent > bad_indent {
                self.index += 1;
                continue;
            }
            break;
        }
    }

    fn parse_function(&mut self, line: Line<'a>, docs: Vec<String>) -> FunctionDecl {
        let header = match parse_function_header(line.content) {
            Ok(header) => header,
            Err(message) => {
                self.error(line, message);
                FunctionHead {
                    public: false,
                    name: String::new(),
                    params: Vec::new(),
                    return_type: None,
                }
            }
        };

        self.index += 1;
        let body_start = self.index;
        if self.has_child_body(line.indent) {
            while self.index < self.lines.len() {
                let next = self.lines[self.index];
                if self.reject_tabs(next) {
                    self.index += 1;
                    continue;
                }
                if next.is_blank() || next.is_comment() || next.doc_comment().is_some() {
                    self.index += 1;
                    continue;
                }
                if next.indent <= line.indent {
                    break;
                }
                self.index += 1;
            }
        } else {
            self.error(line, "expected an indented function body");
        }
        let body =
            span_for_lines(&self.lines, body_start, self.index).unwrap_or_else(|| line.span());

        FunctionDecl {
            docs,
            public: header.public,
            name: header.name,
            params: header.params,
            return_type: header.return_type,
            body,
            span: line.span(),
        }
    }

    fn has_child_body(&self, parent_indent: usize) -> bool {
        let mut index = self.index;
        while index < self.lines.len() {
            let line = self.lines[index];
            if line.is_blank() || line.is_comment() || line.doc_comment().is_some() {
                index += 1;
                continue;
            }
            return line.indent > parent_indent;
        }
        false
    }

    fn reject_tabs(&mut self, line: Line<'a>) -> bool {
        let Some(tab) = line.text.find('\t') else {
            return false;
        };
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: "tabs are not allowed in Marrow source; use spaces for indentation"
                .to_string(),
            help: Some("Replace the tab with spaces.".to_string()),
            span: SourceSpan {
                start_byte: line.start_byte + tab,
                end_byte: line.start_byte + tab + 1,
                line: line.number,
                column: (tab + 1) as u32,
            },
            line: line.number,
            column: (tab + 1) as u32,
        });
        true
    }

    fn error(&mut self, line: Line<'a>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span: line.span_at_content(),
            line: line.number,
            column: (line.indent + 1) as u32,
        });
    }
}

impl<'a> Line<'a> {
    fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }

    fn is_comment(&self) -> bool {
        self.content.starts_with(';') && !self.content.starts_with(";;")
    }

    fn doc_comment(&self) -> Option<&'a str> {
        self.content.strip_prefix(";;").map(str::trim)
    }

    fn span(&self) -> SourceSpan {
        SourceSpan {
            start_byte: self.start_byte,
            end_byte: self.end_byte,
            line: self.number,
            column: 1,
        }
    }

    fn span_at_content(&self) -> SourceSpan {
        SourceSpan {
            start_byte: self.start_byte + self.indent,
            end_byte: self.end_byte,
            line: self.number,
            column: (self.indent + 1) as u32,
        }
    }
}

fn split_lines(source: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut number = 1;

    for segment in source.split_inclusive('\n') {
        let mut text = segment;
        if let Some(stripped) = text.strip_suffix('\n') {
            text = stripped;
        }
        if let Some(stripped) = text.strip_suffix('\r') {
            text = stripped;
        }
        lines.push(make_line(number, start, text));
        start += segment.len();
        number += 1;
    }

    if source.is_empty() || !source.ends_with('\n') {
        let text = &source[start..];
        if !text.is_empty() || source.is_empty() {
            lines.push(make_line(number, start, text));
        }
    }

    lines
}

fn make_line(number: u32, start_byte: usize, text: &str) -> Line<'_> {
    let indent = text.bytes().take_while(|byte| *byte == b' ').count();
    Line {
        number,
        start_byte,
        end_byte: start_byte + text.len(),
        text,
        indent,
        content: &text[indent..],
    }
}

fn parse_resource_header(content: &str) -> Result<(String, Option<SavedRoot>), &'static str> {
    let rest = content
        .strip_prefix("resource ")
        .ok_or("expected resource declaration")?
        .trim();
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected resource name");
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok((name.to_string(), None));
    }
    let rest = rest
        .strip_prefix("at ")
        .ok_or("expected `at ^root` after resource name")?
        .trim();
    let rest = rest
        .strip_prefix('^')
        .ok_or("expected saved root beginning with `^`")?;
    let Some((root, rest)) = read_identifier(rest) else {
        return Err("expected saved root name");
    };
    let rest = rest.trim();
    let keys = if rest.is_empty() {
        Vec::new()
    } else {
        parse_key_params(rest)?
    };
    Ok((
        name.to_string(),
        Some(SavedRoot {
            root: root.to_string(),
            keys,
        }),
    ))
}

fn parse_function_header(content: &str) -> Result<FunctionHead, &'static str> {
    let (public, rest) = if let Some(rest) = content.strip_prefix("pub ") {
        (true, rest)
    } else if let Some(rest) = content.strip_prefix("internal ") {
        if rest.starts_with("fn ") {
            return Err("function visibility is only `pub` or module-private; remove `internal`");
        }
        (false, content)
    } else if let Some(rest) = content.strip_prefix("private ") {
        if rest.starts_with("fn ") {
            return Err("function visibility is only `pub` or module-private; remove `private`");
        }
        (false, content)
    } else {
        (false, content)
    };
    let rest = rest
        .strip_prefix("fn ")
        .ok_or("expected fn declaration")?
        .trim();
    let Some((name, after_name)) = read_identifier(rest) else {
        return Err("expected function name");
    };
    let after_name = after_name.trim_start();
    let (params_text, after_params) =
        parse_parenthesized_prefix(after_name).ok_or("expected function parameter list")?;
    let params = parse_params(params_text)?;
    let after_params = after_params.trim();
    let return_type = if after_params.is_empty() {
        None
    } else {
        let ty = after_params
            .strip_prefix(':')
            .ok_or("expected return type after `:`")?
            .trim();
        if ty.is_empty() {
            return Err("expected return type after `:`");
        }
        if !is_type_text(ty) {
            return Err("expected return type annotation");
        }
        Some(type_ref(ty))
    };

    Ok(FunctionHead {
        public,
        name: name.to_string(),
        params,
        return_type,
    })
}

struct FunctionHead {
    public: bool,
    name: String,
    params: Vec<ParamDecl>,
    return_type: Option<TypeRef>,
}

enum MemberHead {
    Field {
        required: bool,
        name: String,
        keys: Vec<KeyParam>,
        ty: TypeRef,
    },
    Group {
        name: String,
        keys: Vec<KeyParam>,
    },
}

fn parse_field_or_group_head(content: &str) -> Result<MemberHead, &'static str> {
    let (required, rest) = if let Some(rest) = content.strip_prefix("required ") {
        (true, rest.trim())
    } else {
        (false, content.trim())
    };
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected resource member name");
    };
    let mut rest = rest.trim_start();
    let keys = if rest.starts_with('(') {
        let (inside, tail) = parse_parenthesized_prefix(rest)
            .ok_or("expected closing `)` in keyed resource member")?;
        rest = tail.trim_start();
        parse_key_params_inside(inside)?
    } else {
        Vec::new()
    };
    if let Some(ty) = rest.strip_prefix(':') {
        let ty = ty.trim();
        if !is_type_text(ty) {
            return Err("expected field type after `:`");
        }
        return Ok(MemberHead::Field {
            required,
            name: name.to_string(),
            keys,
            ty: type_ref(ty),
        });
    }
    if required {
        return Err("required resource members must declare a field type");
    }
    if rest.is_empty() {
        return Ok(MemberHead::Group {
            name: name.to_string(),
            keys,
        });
    }
    Err("expected resource field, keyed field, group, or index")
}

fn parse_index(content: &str) -> Result<IndexDecl, &'static str> {
    let rest = content
        .strip_prefix("index ")
        .ok_or("expected index declaration")?
        .trim();
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected index name");
    };
    let rest = rest.trim_start();
    let (args_text, tail) =
        parse_parenthesized_prefix(rest).ok_or("expected index argument list")?;
    if args_text.trim().is_empty() {
        return Err("expected at least one index argument");
    }
    let args = split_commas(args_text)?;
    if !args.iter().all(|arg| is_field_path(arg)) {
        return Err("expected index field path");
    }
    let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
    let tail = tail.trim();
    let unique = match tail {
        "" => false,
        "unique" => true,
        _ => return Err("expected `unique` or end of index declaration"),
    };
    Ok(IndexDecl {
        docs: Vec::new(),
        stable_id: None,
        name: name.to_string(),
        args,
        unique,
        span: SourceSpan::default(),
    })
}

fn parse_key_params(text: &str) -> Result<Vec<KeyParam>, &'static str> {
    let (inside, tail) = parse_parenthesized_prefix(text).ok_or("expected key parameter list")?;
    if !tail.trim().is_empty() {
        return Err("unexpected text after key parameter list");
    }
    parse_key_params_inside(inside)
}

fn parse_key_params_inside(text: &str) -> Result<Vec<KeyParam>, &'static str> {
    if text.trim().is_empty() {
        return Err("expected at least one key parameter");
    }
    let mut params = Vec::new();
    for part in split_commas(text)? {
        let (name, ty) = parse_name_type(part);
        let Some(ty) = ty else {
            return Err("expected key type annotation");
        };
        if !is_identifier(name) {
            return Err("expected key name");
        }
        if !is_type_text(ty) {
            return Err("expected key type annotation");
        }
        params.push(KeyParam {
            name: name.to_string(),
            ty: type_ref(ty),
        });
    }
    Ok(params)
}

fn parse_params(text: &str) -> Result<Vec<ParamDecl>, &'static str> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for part in split_commas(text)? {
        let (mode, rest) = if let Some(rest) = part.strip_prefix("out ") {
            (Some(ParamMode::Out), rest.trim())
        } else if let Some(rest) = part.strip_prefix("inout ") {
            (Some(ParamMode::InOut), rest.trim())
        } else {
            (None, part)
        };
        let (name, ty) = parse_name_type(rest);
        let Some(ty) = ty else {
            return Err("expected parameter type annotation");
        };
        if !is_identifier(name) {
            return Err("expected parameter name");
        }
        if !is_type_text(ty) {
            return Err("expected parameter type annotation");
        }
        params.push(ParamDecl {
            mode,
            name: name.to_string(),
            ty: type_ref(ty),
        });
    }
    Ok(params)
}

fn parse_name_type(text: &str) -> (&str, Option<&str>) {
    match split_once_trimmed(text, ':') {
        Some((name, ty)) => (name, Some(ty)),
        None => (text.trim(), None),
    }
}

fn parse_stable_id(content: &str) -> Option<String> {
    let rest = content.strip_prefix("@id(")?.strip_suffix(')')?.trim();
    let body = rest.strip_prefix('"')?.strip_suffix('"')?;
    Some(body.to_string())
}

fn parse_parenthesized_prefix(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if !text.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&text[1..index], &text[index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn read_identifier(text: &str) -> Option<(&str, &str)> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = first.len_utf8();
    for (index, ch) in chars {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            end = index + ch.len_utf8();
        } else {
            return Some((&text[..index], &text[index..]));
        }
    }
    Some((&text[..end], &text[end..]))
}

fn is_identifier(text: &str) -> bool {
    let Some((ident, rest)) = read_identifier(text) else {
        return false;
    };
    ident == text && rest.is_empty()
}

fn is_qualified_name(text: &str) -> bool {
    let mut parts = text.split("::");
    let Some(first) = parts.next() else {
        return false;
    };
    is_identifier(first) && parts.all(is_identifier)
}

fn is_type_text(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('=') {
        return false;
    }
    if let Some(inner) = text
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return is_type_text(inner);
    }
    is_qualified_name(text)
}

fn is_field_path(text: &str) -> bool {
    let mut parts = text.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    is_identifier(first) && parts.all(is_identifier)
}

fn split_commas(text: &str) -> Result<Vec<&str>, &'static str> {
    let raw = text.split(',').collect::<Vec<_>>();
    let mut parts = Vec::new();
    for (index, part) in raw.iter().enumerate() {
        let part = part.trim();
        if part.is_empty() {
            if index + 1 == raw.len() {
                continue;
            }
            return Err("expected item between commas");
        }
        parts.push(part);
    }
    Ok(parts)
}

fn split_once_trimmed(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let (left, right) = text.split_once(delimiter)?;
    Some((left.trim(), right.trim()))
}

fn type_ref(text: &str) -> TypeRef {
    TypeRef {
        text: text.trim().to_string(),
    }
}

fn starts_function(content: &str) -> bool {
    content.starts_with("fn ") || content.starts_with("pub fn ")
}

fn span_for_lines(lines: &[Line<'_>], start: usize, end: usize) -> Option<SourceSpan> {
    if start >= end {
        return None;
    }
    let first = lines[start];
    let last = lines[end - 1];
    Some(SourceSpan {
        start_byte: first.start_byte,
        end_byte: last.end_byte,
        line: first.number,
        column: 1,
    })
}
