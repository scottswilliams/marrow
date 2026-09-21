//! The semantic queries: one closed enum from request parsing to reply construction.
//!
//! A query is decoded to fixed-size fields at admission, so a held query never retains
//! unbounded raw parameters, and it is answered against a ready snapshot by the one fact
//! projection its variant names. The method names are spelled once here, so admission
//! and answering cannot drift.

use lsp_types::{
    CompletionParams, DocumentFormattingParams, DocumentSymbolParams, GotoDefinitionParams,
    HoverParams, Position, SignatureHelpParams, TextDocumentIdentifier, TextDocumentPositionParams,
    Uri,
};
use marrow_compile::{AnalysisSnapshot, ProjectFile};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::facts;
use crate::outbound::ResponseResult;
use crate::protocol::decode_params;

/// One semantic request, parsed to its fixed-size target.
pub(crate) enum SemanticQuery {
    Hover(Position),
    Definition(Position),
    Formatting,
    Completion(Position),
    SignatureHelp(Position),
    DocumentSymbol,
}

/// A semantic request whose parameters did not decode.
pub(crate) struct MalformedParams;

/// Why a query could not be answered from a ready snapshot.
pub(crate) enum QueryRefusal {
    /// The candidate set or rendered display exceeded a per-query bound.
    ResourceLimit,
    /// A definition target could not be addressed.
    Internal,
}

impl SemanticQuery {
    /// Parse a request. `None` when `method` is not a semantic request; otherwise the
    /// query and the URI of the document it names, or [`MalformedParams`].
    pub(crate) fn parse(
        method: &str,
        params: Option<&RawValue>,
    ) -> Option<Result<(Self, Uri), MalformedParams>> {
        Some(match method {
            "textDocument/hover" => at(params, Self::Hover, |p: HoverParams| {
                p.text_document_position_params
            }),
            "textDocument/definition" => at(params, Self::Definition, |p: GotoDefinitionParams| {
                p.text_document_position_params
            }),
            "textDocument/completion" => at(params, Self::Completion, |p: CompletionParams| {
                p.text_document_position
            }),
            "textDocument/signatureHelp" => {
                at(params, Self::SignatureHelp, |p: SignatureHelpParams| {
                    p.text_document_position_params
                })
            }
            "textDocument/formatting" => {
                whole(params, Self::Formatting, |p: DocumentFormattingParams| {
                    p.text_document
                })
            }
            "textDocument/documentSymbol" => {
                whole(params, Self::DocumentSymbol, |p: DocumentSymbolParams| {
                    p.text_document
                })
            }
            _ => return None,
        })
    }

    /// Answer against a ready snapshot. `file` and `source` are the bound document's
    /// identity and current text; `target_uri` addresses a definition target's file.
    pub(crate) fn answer(
        &self,
        snapshot: &AnalysisSnapshot,
        file: &ProjectFile,
        source: &str,
        target_uri: impl Fn(&ProjectFile) -> Option<Uri>,
    ) -> Result<ResponseResult, QueryRefusal> {
        Ok(match *self {
            Self::Hover(position) => {
                ResponseResult::Hover(facts::hover(snapshot, file, source, position))
            }
            Self::Definition(position) => ResponseResult::Definition(
                facts::definition(snapshot, file, source, target_uri, position)
                    .map_err(|_| QueryRefusal::Internal)?,
            ),
            Self::Formatting => {
                ResponseResult::Formatting(facts::formatting(snapshot, file, source))
            }
            Self::Completion(position) => ResponseResult::Completion(
                facts::completion(snapshot, file, source, position)
                    .map_err(|facts::ResourceLimited| QueryRefusal::ResourceLimit)?,
            ),
            Self::SignatureHelp(position) => ResponseResult::SignatureHelp(
                facts::signature_help(snapshot, file, source, position)
                    .map_err(|facts::ResourceLimited| QueryRefusal::ResourceLimit)?,
            ),
            Self::DocumentSymbol => {
                ResponseResult::DocumentSymbol(facts::document_symbols(snapshot, file, source))
            }
        })
    }
}

/// A request addressed at one position of one document.
fn at<P: DeserializeOwned>(
    params: Option<&RawValue>,
    query: fn(Position) -> SemanticQuery,
    target: fn(P) -> TextDocumentPositionParams,
) -> Result<(SemanticQuery, Uri), MalformedParams> {
    let target = target(decode_params::<P>(params).ok_or(MalformedParams)?);
    Ok((query(target.position), target.text_document.uri))
}

/// A request addressed at one whole document.
fn whole<P: DeserializeOwned>(
    params: Option<&RawValue>,
    query: SemanticQuery,
    target: fn(P) -> TextDocumentIdentifier,
) -> Result<(SemanticQuery, Uri), MalformedParams> {
    let target = target(decode_params::<P>(params).ok_or(MalformedParams)?);
    Ok((query, target.uri))
}
