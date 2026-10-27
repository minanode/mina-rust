//! Admission limits before Juniper validation/execution or any node RPC.
//! Juniper 0.17 has no built-in depth/complexity limits. Use its lexer and AST,
//! including fragment expansion; text/alias counting alone is bypassable.
//! Its parser takes time quadratic in a document's tokens (it removes each
//! token from the front of a `Vec`), and a private request is parsed three
//! times (here, in OCaml's missing-variable check and in Juniper's execution)
//! and validated twice, so the token limit is also the CPU limit.
use std::collections::HashMap;

use juniper::{
    http::{GraphQLBatchRequest, GraphQLRequest},
    parser::{parse_document_source, Lexer, Token},
    Definition, Selection,
};

use super::graphql::Schema;

/// Size limits, by mode. The other limits bound execution and parser
/// recursion, which large literal values do not change, so both modes share
/// them.
#[derive(Clone, Copy, Debug)]
pub(super) struct SizeLimits {
    /// The HTTP body: every request of a batch, with its variables.
    pub(super) body_bytes: usize,
    /// One request's query text.
    pub(super) query_bytes: usize,
    /// Lexer tokens, every query of a batch together.
    pub(super) tokens: usize,
    /// One query's tokens outside parentheses: its operations, selections
    /// and fragments, without argument values or variable definitions.
    pub(super) structure_tokens: usize,
}

impl SizeLimits {
    pub(super) fn of(public: bool) -> Self {
        if public {
            PUBLIC
        } else {
            PRIVATE
        }
    }
}

/// Admits zkApp commands as o1js builds them, up to the largest a Mesa node
/// accepts, sent inline in the mutation text: at most 1,060,965 query bytes
/// and 1,082,925 body bytes (15 proofs), or 19,396 tokens (31 signed account
/// updates), every event and action holding an element; see
/// private implementation notes. The protocol counts event
/// and action elements, not events, so a valid command padded with many empty
/// events can exceed these limits. The 1.25 MiB body, with `graphql_body`,
/// bounds memory on a 1 GB node; the 24,576 tokens bound CPU, as a full answer
/// to that many takes about 1.3 s. The structure keeps the earlier token
/// limit, as a command's values are all inside parentheses.
pub(super) const PRIVATE: SizeLimits = SizeLimits {
    body_bytes: 1280 * 1024,
    query_bytes: 1280 * 1024,
    tokens: 24 * 1024,
    structure_tokens: 8192,
};

/// A public node refuses mutations and batches, so it keeps the earlier
/// limits.
pub(super) const PUBLIC: SizeLimits = SizeLimits {
    body_bytes: 256 * 1024,
    query_bytes: 64 * 1024,
    tokens: 8192,
    structure_tokens: 8192,
};

const MAX_SYNTAX_DEPTH: usize = 32;
const MAX_DEPTH: usize = 24;
const MAX_SELECTIONS: usize = 1024;
const MAX_ROOT_FIELDS: usize = 32;
const MAX_RECEIPT_PAGES: usize = 8;
const MAX_BATCH: usize = 10;

#[derive(Default, Debug)]
struct Budget {
    tokens: usize,
    selections: usize,
    root_fields: usize,
    receipt_pages: usize,
    depth: usize,
}

pub(super) fn check(
    request: &GraphQLBatchRequest,
    schema: &Schema,
    limits: SizeLimits,
) -> Result<(), String> {
    let requests = match request {
        GraphQLBatchRequest::Single(request) => std::slice::from_ref(request),
        GraphQLBatchRequest::Batch(requests) => requests.as_slice(),
    };
    if requests.is_empty() || requests.len() > MAX_BATCH {
        return Err(format!(
            "GraphQL batch must contain 1..{MAX_BATCH} requests"
        ));
    }
    // Shared across the entire batch, not a fresh allowance for each operation.
    let mut budget = Budget::default();
    for request in requests {
        check_query(request, schema, limits, &mut budget)?;
    }
    Ok(())
}

fn check_query(
    request: &GraphQLRequest,
    schema: &Schema,
    limits: SizeLimits,
    budget: &mut Budget,
) -> Result<(), String> {
    if request.query.len() > limits.query_bytes {
        return Err(format!(
            "GraphQL query exceeds {} bytes",
            limits.query_bytes
        ));
    }
    // Bound recursive parser input first, including input objects and type lists.
    // Juniper's lexer correctly ignores punctuation in comments and strings.
    // Tokens inside parentheses are argument values and variable definitions;
    // the rest is the document's structure.
    let (mut nesting, mut parens, mut structure) = (0usize, 0usize, 0usize);
    for token in Lexer::new(&request.query) {
        budget.tokens += 1;
        if budget.tokens > limits.tokens {
            return Err(format!("GraphQL request exceeds {} tokens", limits.tokens));
        }
        if parens == 0 {
            structure += 1;
            if structure > limits.structure_tokens {
                return Err(format!(
                    "GraphQL query exceeds {} tokens outside arguments",
                    limits.structure_tokens
                ));
            }
        }
        match token
            .map_err(|e| format!("Invalid GraphQL token: {e:?}"))?
            .item
        {
            token @ (Token::CurlyOpen | Token::BracketOpen | Token::ParenOpen) => {
                nesting += 1;
                if nesting > MAX_SYNTAX_DEPTH {
                    return Err(format!("GraphQL syntax nesting exceeds {MAX_SYNTAX_DEPTH}"));
                }
                if token == Token::ParenOpen {
                    parens += 1;
                }
            }
            token @ (Token::CurlyClose | Token::BracketClose | Token::ParenClose) => {
                nesting = nesting.saturating_sub(1);
                if token == Token::ParenClose {
                    parens = parens.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
    let document = parse_document_source(&request.query, &schema.schema)
        .map_err(|e| format!("Invalid GraphQL query: {e}"))?;
    let mut fragments = HashMap::new();
    for definition in &document {
        if let Definition::Fragment(fragment) = definition {
            if fragments
                .insert(
                    fragment.item.name.item,
                    fragment.item.selection_set.as_slice(),
                )
                .is_some()
            {
                return Err("Duplicate GraphQL fragment".into());
            }
        }
    }
    // Inspect even unselected operations and unused fragments, since Juniper
    // validates the whole document. Directives never exempt work from the budget.
    for definition in &document {
        let selections = match definition {
            Definition::Operation(operation) => &operation.item.selection_set,
            Definition::Fragment(fragment) => &fragment.item.selection_set,
        };
        walk(
            selections,
            &fragments,
            &mut Vec::new(),
            1,
            0,
            matches!(definition, Definition::Operation(_)),
            budget,
        )?;
    }
    Ok(())
}

fn walk<'a>(
    selections: &[Selection<'a>],
    fragments: &HashMap<&'a str, &[Selection<'a>]>,
    active: &mut Vec<&'a str>,
    depth: usize,
    expansion_depth: usize,
    count_roots: bool,
    budget: &mut Budget,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("GraphQL field depth exceeds {MAX_DEPTH}"));
    }
    if expansion_depth > MAX_SYNTAX_DEPTH {
        return Err(format!(
            "GraphQL fragment nesting exceeds {MAX_SYNTAX_DEPTH}"
        ));
    }
    for selection in selections {
        budget.selections += 1;
        if budget.selections > MAX_SELECTIONS {
            return Err(format!(
                "GraphQL expanded selection count exceeds {MAX_SELECTIONS}"
            ));
        }
        match selection {
            Selection::Field(field) => {
                budget.depth = budget.depth.max(depth);
                if count_roots && depth == 1 {
                    budget.root_fields += 1;
                    if budget.root_fields > MAX_ROOT_FIELDS {
                        return Err(format!(
                            "GraphQL root field count exceeds {MAX_ROOT_FIELDS}"
                        ));
                    }
                }
                if field.item.name.item == "receipts" {
                    budget.receipt_pages += 1;
                    if budget.receipt_pages > MAX_RECEIPT_PAGES {
                        return Err(format!(
                            "GraphQL receipt page count exceeds {MAX_RECEIPT_PAGES}"
                        ));
                    }
                }
                if let Some(children) = &field.item.selection_set {
                    walk(
                        children,
                        fragments,
                        active,
                        depth + 1,
                        expansion_depth,
                        count_roots,
                        budget,
                    )?;
                }
            }
            Selection::InlineFragment(fragment) => {
                walk(
                    &fragment.item.selection_set,
                    fragments,
                    active,
                    depth,
                    expansion_depth + 1,
                    count_roots,
                    budget,
                )?;
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.item.name.item;
                if active.contains(&name) {
                    return Err("Cyclic GraphQL fragment".into());
                }
                let children = fragments.get(name).ok_or("Unknown GraphQL fragment")?;
                active.push(name);
                walk(
                    children,
                    fragments,
                    active,
                    depth,
                    expansion_depth + 1,
                    count_roots,
                    budget,
                )?;
                active.pop();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
